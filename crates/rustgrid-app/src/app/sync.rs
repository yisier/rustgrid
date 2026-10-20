//! The 数据传输/同步 main tab: compare a source database with a target database and apply the
//! difference (structure, data, or both).
//!
//! The page follows Navicat's 数据传输/同步 workbench: a source → target topology bar, one
//! segmented control for the four modes (全部 / 仅结构 / 仅数据 / 全量传输), a table list with a
//! per-table status pill, and an inspector with 结构比对 / 数据比对 / SQL 预览 tabs. Comparisons run
//! through [`rustgrid_core::Connection`] (so every driver works) and the engine-agnostic diff lives
//! in [`rustgrid_core::sync`]. Applying a plan reuses the drivers' own write paths
//! (`save_table_schema`, `insert_rows`, `update_rows`, `delete_rows`), never raw SQL written here,
//! so this module stays engine-independent.
//!
//! Every database call is awaited through [`Runtime::spawn`]: gpui's executor is not Tokio, so
//! awaiting a driver future directly panics with "there is no reactor running".

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rustgrid_core::{
    CellValue, ColumnInfo, DataDiff, ObjectKind, PageRequest, RowChange, RowChangeKind,
    SchemaChange, SchemaChangeKind, SortColumn, TableSchema, TransferMode, diff_columns, diff_rows,
    render_change_sql,
};

use super::backup::{format_hms, now_timestamp, render_log_lines};
use super::*;

/// Width of the table list column.
const SYNC_LIST_WIDTH: f32 = 296.0;
/// Height of one table row.
const SYNC_ROW_HEIGHT: f32 = 26.0;
/// Rows fetched per page while comparing or copying.
const SYNC_PAGE_SIZE: u64 = 500;
/// The most rows compared per side per table. A larger table reports a truncated (partial) diff
/// rather than loading everything into memory.
const SYNC_MAX_ROWS: u64 = 5_000;
/// Rows written per `insert_rows` / `update_rows` / `delete_rows` call.
const SYNC_WRITE_BATCH: usize = 500;
/// Statements rendered in the SQL preview before it summarizes the rest.
const SYNC_PREVIEW_MAX_ROWS: usize = 200;
/// Changed rows rendered in the 数据比对 grid before it summarizes the rest.
const SYNC_DISPLAY_MAX_ROWS: usize = 500;
/// The 选项 popover's backdrop and panel paint priorities.
const SYNC_BACKDROP_PRIORITY: usize = 96;
const SYNC_POPUP_PRIORITY: usize = 97;
const SYNC_POPUP_WIDTH: f32 = 264.0;
/// Height of the run log panel.
const SYNC_LOG_HEIGHT: f32 = 196.0;

/// What a sync run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SyncMode {
    /// Structure and data.
    All,
    /// Structure only.
    Schema,
    /// Data only.
    Data,
    /// Recreate the table and copy every row (a full overwrite).
    Transfer,
}

impl SyncMode {
    fn label_key(self) -> &'static str {
        match self {
            SyncMode::All => "sync.mode.all",
            SyncMode::Schema => "sync.mode.schema",
            SyncMode::Data => "sync.mode.data",
            SyncMode::Transfer => "sync.mode.transfer",
        }
    }

    fn uses_schema(self) -> bool {
        matches!(self, SyncMode::All | SyncMode::Schema | SyncMode::Transfer)
    }

    fn uses_data(self) -> bool {
        matches!(self, SyncMode::All | SyncMode::Data | SyncMode::Transfer)
    }

    /// The persisted form of the mode (`transfers.json`).
    fn to_saved(self) -> TransferMode {
        match self {
            SyncMode::All => TransferMode::All,
            SyncMode::Schema => TransferMode::Schema,
            SyncMode::Data => TransferMode::Data,
            SyncMode::Transfer => TransferMode::Transfer,
        }
    }

    fn from_saved(mode: TransferMode) -> Self {
        match mode {
            TransferMode::All => SyncMode::All,
            TransferMode::Schema => SyncMode::Schema,
            TransferMode::Data => SyncMode::Data,
            TransferMode::Transfer => SyncMode::Transfer,
        }
    }
}

/// The inspector's sub-tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SyncSubTab {
    Schema,
    Data,
    Sql,
}

/// The 数据比对 tab's action filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SyncDataFilter {
    All,
    Insert,
    Update,
    Delete,
}

impl SyncDataFilter {
    fn matches(self, kind: RowChangeKind) -> bool {
        match self {
            SyncDataFilter::All => true,
            SyncDataFilter::Insert => kind == RowChangeKind::Insert,
            SyncDataFilter::Update => kind == RowChangeKind::Update,
            SyncDataFilter::Delete => kind == RowChangeKind::Delete,
        }
    }
}

/// A status pill's tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PillTone {
    Neutral,
    Accent,
    Good,
    Bad,
}

impl PillTone {
    fn color(self, theme: Theme) -> u32 {
        match self {
            PillTone::Neutral => theme.text_muted,
            PillTone::Accent => theme.brand,
            PillTone::Good => theme.icon_table,
            PillTone::Bad => theme.danger,
        }
    }
}

/// One source table and what a sync would change on the target.
pub(super) struct SyncTable {
    pub name: String,
    pub checked: bool,
    /// Whether the target database already has a table with this name.
    pub in_target: bool,
    pub source_schema: Option<TableSchema>,
    pub target_schema: Option<TableSchema>,
    pub schema_changes: Vec<SchemaChange>,
    pub source_ddl: Option<String>,
    pub target_ddl: Option<String>,
    pub data: DataDiff,
    /// The comparison is still running.
    pub comparing: bool,
    /// The table was written by this session, so its diff is empty.
    pub synced: bool,
    /// Per-table 全量覆写, offered for a table without a primary key.
    pub force_transfer: bool,
    pub error: Option<String>,
}

impl SyncTable {
    fn new(name: String, in_target: bool) -> Self {
        Self {
            name,
            checked: true,
            in_target,
            source_schema: None,
            target_schema: None,
            schema_changes: Vec::new(),
            source_ddl: None,
            target_ddl: None,
            data: DataDiff::default(),
            comparing: true,
            synced: false,
            force_transfer: false,
            error: None,
        }
    }

    /// Whether the table has a primary key to compare rows by.
    fn has_key(&self) -> bool {
        !self.data.key_columns.is_empty()
    }

    /// Whether the selected mode would change anything on the target.
    fn has_changes(&self, mode: SyncMode, same_engine: bool) -> bool {
        if self.error.is_some() || self.comparing {
            return false;
        }
        let schema = !self.in_target || !self.schema_changes.is_empty();
        let data = if self.has_key() {
            self.in_target && !self.data.is_empty()
        } else {
            self.force_transfer && self.in_target
        };
        match mode {
            SyncMode::Schema => schema && same_engine,
            SyncMode::Data => data,
            SyncMode::All => data || (schema && same_engine),
            // Cross-engine transfer still copies rows into an existing target table.
            SyncMode::Transfer => self.in_target || (same_engine && self.source_schema.is_some()),
        }
    }

    /// The status pill's `(text, tone)` for the current mode.
    fn status(&self, mode: SyncMode) -> (String, PillTone) {
        if self.error.is_some() {
            return (t!("sync.status.error").to_string(), PillTone::Bad);
        }
        if self.comparing {
            return (t!("sync.status.comparing").to_string(), PillTone::Neutral);
        }
        if self.synced {
            return (t!("sync.status.synced").to_string(), PillTone::Neutral);
        }
        if mode == SyncMode::Transfer || self.force_transfer {
            return (t!("sync.status.overwrite").to_string(), PillTone::Accent);
        }
        if mode.uses_data() && !self.has_key() {
            return (t!("sync.status.no_key").to_string(), PillTone::Bad);
        }
        if !self.in_target {
            return (
                if mode.uses_data() {
                    t!("sync.status.new", rows = self.data.inserts).to_string()
                } else {
                    t!("sync.status.new_table").to_string()
                },
                PillTone::Good,
            );
        }
        let schema = !self.schema_changes.is_empty();
        let data = self.has_key() && !self.data.is_empty();
        match mode {
            SyncMode::Schema => {
                if schema {
                    (
                        t!("sync.status.schema", count = self.schema_changes.len()).to_string(),
                        PillTone::Accent,
                    )
                } else {
                    (t!("sync.status.same").to_string(), PillTone::Neutral)
                }
            }
            SyncMode::Data => {
                if data {
                    (
                        t!("sync.status.data", count = self.data.total()).to_string(),
                        PillTone::Accent,
                    )
                } else {
                    (t!("sync.status.same").to_string(), PillTone::Neutral)
                }
            }
            _ => {
                if schema && data {
                    (
                        t!("sync.status.both", count = self.data.total()).to_string(),
                        PillTone::Accent,
                    )
                } else if schema {
                    (
                        t!("sync.status.schema", count = self.schema_changes.len()).to_string(),
                        PillTone::Accent,
                    )
                } else if data {
                    (
                        t!("sync.status.data", count = self.data.total()).to_string(),
                        PillTone::Accent,
                    )
                } else {
                    (t!("sync.status.same").to_string(), PillTone::Neutral)
                }
            }
        }
    }

    /// The 数据比对 grid's rows after the action filter.
    fn filtered_rows(&self, filter: SyncDataFilter) -> Vec<&RowChange> {
        self.data
            .rows
            .iter()
            .filter(|change| filter.matches(change.kind))
            .collect()
    }
}

/// The 数据传输/同步 page's state.
pub(super) struct SyncState {
    pub source_connection: Option<usize>,
    pub source_database: String,
    pub target_connection: Option<usize>,
    pub target_database: String,
    pub mode: SyncMode,
    pub tables: Vec<SyncTable>,
    pub selected: Option<usize>,
    pub sub_tab: SyncSubTab,
    pub data_filter: SyncDataFilter,
    pub search: String,
    pub include_delete: bool,
    pub existing_only: bool,
    pub options_open: bool,
    pub options_anchor: Rc<RefCell<Point<Pixels>>>,
    pub comparing: bool,
    pub compared: usize,
    pub compare_total: usize,
    pub error: Option<String>,
    pub running: bool,
    pub cancel: Arc<AtomicBool>,
    pub progress: f32,
    pub log: Vec<String>,
    pub log_open: bool,
    pub log_scroll: ScrollHandle,
    pub log_handle: gpui_kit::base::TextSelectionHandle,
    pub started: Option<Instant>,
    pub elapsed: Option<Duration>,
    pub list_scroll: ScrollHandle,
    pub inspector_scroll: ScrollHandle,
    pub source_combo: Option<Entity<ComboBox>>,
    pub source_database_combo: Option<Entity<ComboBox>>,
    pub target_combo: Option<Entity<ComboBox>>,
    pub target_database_combo: Option<Entity<ComboBox>>,
    pub search_input: Entity<TextInput>,
    /// The 传输 configurations list's search text and its field.
    pub transfer_search: String,
    pub transfer_search_input: Entity<TextInput>,
    /// The 传输 list's layout state (column widths, sort, scroll and tile grid).
    pub transfer_columns: Rc<RefCell<DetailColumns>>,
    pub transfer_sort: Rc<RefCell<DetailSort>>,
    pub transfer_detail_scroll: DetailScroll,
    pub transfer_grid: ColumnGrid,
    /// Focus target for the 传输 list, so Ctrl+A / Delete reach it.
    pub transfer_focus: FocusHandle,
    /// The configuration name typed in the transfer window's footer.
    pub config_name: String,
    /// The configuration-name field of the transfer window.
    pub config_name_input: Option<Entity<TextInput>>,
    /// The current settings were written back to `transfers.json` since the last edit.
    pub config_saved: bool,
    /// Why the last 保存配置 was rejected (shown in the window footer).
    pub config_error: Option<String>,
    /// A scope was chosen while a side was still connecting: compare once both are ready.
    pub needs_refresh: bool,
    /// The saved table selection to re-apply when the comparison installs its table list.
    pub pending_tables: Option<HashSet<String>>,
    /// Bumped by every new comparison, so a stale background task cannot write into the page.
    pub generation: u64,
}

impl SyncState {
    pub(super) fn new(cx: &mut Context<'_, AppView>) -> Self {
        let weak = cx.weak_entity();
        let search_input = cx.new(move |cx| {
            TextInput::new(
                Theme::dark(),
                "",
                TextInputOptions {
                    placeholder: t!("sync.search").to_string().into(),
                    icon: Some("icons/search.svg"),
                    clearable: true,
                    size: Some(gpui_kit::component::Size::XSmall),
                    text_size: Some(12.0),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = weak.update(cx, |app, cx| {
                    app.sync.search = text.to_string();
                    cx.notify();
                });
            }))
        });
        let weak = cx.weak_entity();
        let transfer_search_input = cx.new(move |cx| {
            TextInput::new(
                Theme::dark(),
                "",
                TextInputOptions {
                    placeholder: t!("sync.config.search").to_string().into(),
                    icon: Some("icons/search.svg"),
                    clearable: true,
                    size: Some(gpui_kit::component::Size::XSmall),
                    text_size: Some(12.0),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = weak.update(cx, |app, cx| {
                    app.sync.transfer_search = text.to_string();
                    cx.notify();
                });
            }))
        });
        Self {
            source_connection: None,
            source_database: String::new(),
            target_connection: None,
            target_database: String::new(),
            mode: SyncMode::All,
            tables: Vec::new(),
            selected: None,
            sub_tab: SyncSubTab::Schema,
            data_filter: SyncDataFilter::All,
            search: String::new(),
            include_delete: true,
            existing_only: false,
            options_open: false,
            options_anchor: Rc::new(RefCell::new(Point::default())),
            comparing: false,
            compared: 0,
            compare_total: 0,
            error: None,
            running: false,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: 0.0,
            log: Vec::new(),
            log_open: false,
            log_scroll: ScrollHandle::new(),
            log_handle: gpui_kit::base::TextSelectionHandle::new(String::new(), cx),
            started: None,
            elapsed: None,
            list_scroll: ScrollHandle::new(),
            inspector_scroll: ScrollHandle::new(),
            source_combo: None,
            source_database_combo: None,
            target_combo: None,
            target_database_combo: None,
            search_input,
            transfer_search: String::new(),
            transfer_search_input,
            transfer_columns: Rc::new(RefCell::new(DetailColumns::default())),
            transfer_sort: Rc::new(RefCell::new(DetailSort::default())),
            transfer_detail_scroll: DetailScroll::default(),
            transfer_grid: ColumnGrid::default(),
            transfer_focus: cx.focus_handle(),
            config_name: String::new(),
            config_name_input: None,
            config_saved: false,
            config_error: None,
            needs_refresh: false,
            pending_tables: None,
            generation: 0,
        }
    }

    /// The selected table, if the selection is still valid.
    fn selected_table(&self) -> Option<&SyncTable> {
        self.selected.and_then(|index| self.tables.get(index))
    }

    /// The visible table indices after the search filter.
    fn visible_indices(&self) -> Vec<usize> {
        let needle = self.search.trim().to_lowercase();
        self.tables
            .iter()
            .enumerate()
            .filter(|(_, table)| needle.is_empty() || table.name.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect()
    }

    /// The checked tables, in list order.
    pub(super) fn checked_indices(&self) -> Vec<usize> {
        self.tables
            .iter()
            .enumerate()
            .filter(|(_, table)| table.checked)
            .map(|(index, _)| index)
            .collect()
    }

    fn log_line(&mut self, line: String) {
        self.log.push(line);
        self.log_scroll.scroll_to_bottom();
    }
}

/// The comparison result for one table, produced off the UI thread.
struct ComparedTable {
    source_schema: Option<TableSchema>,
    target_schema: Option<TableSchema>,
    schema_changes: Vec<SchemaChange>,
    source_ddl: Option<String>,
    target_ddl: Option<String>,
    data: DataDiff,
    error: Option<String>,
}

/// One checked table's plan, captured before the run starts.
struct SyncPlan {
    name: String,
    in_target: bool,
    has_key: bool,
    source_schema: Option<TableSchema>,
    target_schema: Option<TableSchema>,
    schema_changes: Vec<SchemaChange>,
    data: DataDiff,
    force_transfer: bool,
}

impl AppView {
    // ----- Setup ---------------------------------------------------------------------------------

    /// Create the four topology dropdowns on the first render.
    pub(super) fn ensure_sync_combos(&mut self, cx: &mut Context<'_, Self>) {
        let theme = self.theme;
        if self.sync.source_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 200.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.sync_source_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/connection.svg", theme.icon_connection, cx);
            });
            self.sync.source_combo = Some(combo);
        }
        if self.sync.source_database_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 200.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ =
                            weak.update(cx, |app, cx| app.sync_source_database_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/database.svg", theme.icon_database, cx);
            });
            self.sync.source_database_combo = Some(combo);
        }
        if self.sync.target_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 200.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.sync_target_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/connection.svg", theme.icon_connection, cx);
            });
            self.sync.target_combo = Some(combo);
        }
        if self.sync.target_database_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 200.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ =
                            weak.update(cx, |app, cx| app.sync_target_database_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/database.svg", theme.icon_database, cx);
            });
            self.sync.target_database_combo = Some(combo);
        }
    }

    /// Push the connection/database lists and the current selection into the dropdowns.
    pub(super) fn sync_sync_combos(&mut self, cx: &mut Context<'_, Self>) {
        let connections: Vec<ComboOption> = self
            .connections
            .iter()
            .enumerate()
            .map(|(index, node)| ComboOption::new(index.to_string(), node.profile.name.clone()))
            .collect();
        let source_databases: Vec<ComboOption> = self
            .sync
            .source_connection
            .map(|index| self.query_database_options(index))
            .unwrap_or_default()
            .into_iter()
            .map(|(value, label)| ComboOption::new(value, label))
            .collect();
        let target_databases: Vec<ComboOption> = self
            .sync
            .target_connection
            .map(|index| self.query_database_options(index))
            .unwrap_or_default()
            .into_iter()
            .map(|(value, label)| ComboOption::new(value, label))
            .collect();
        // A connection's database list arrives after it connects; pick the first one so the page
        // starts comparing without another click.
        let mut picked = false;
        if self.sync.source_database.is_empty()
            && let Some(first) = source_databases.first()
        {
            self.sync.source_database = first.value.clone();
            picked = true;
        }
        if self.sync.target_database.is_empty()
            && let Some(first) = target_databases.first()
        {
            self.sync.target_database = first.value.clone();
            picked = true;
        }
        if picked && self.sync.source_connection.is_some() && self.sync.target_connection.is_some()
        {
            self.sync_refresh(cx);
        }
        let source_connection = self
            .sync
            .source_connection
            .map(|index| index.to_string())
            .unwrap_or_default();
        let target_connection = self
            .sync
            .target_connection
            .map(|index| index.to_string())
            .unwrap_or_default();
        let placeholder = t!("sync.select_connection").to_string();
        let database_placeholder = t!("database.name").to_string();
        if let Some(combo) = self.sync.source_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(connections.clone(), cx);
                combo.set_selected(source_connection, cx);
                combo.set_placeholder(placeholder.clone(), cx);
            });
        }
        if let Some(combo) = self.sync.target_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(connections, cx);
                combo.set_selected(target_connection, cx);
                combo.set_placeholder(placeholder, cx);
            });
        }
        if let Some(combo) = self.sync.source_database_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(source_databases, cx);
                combo.set_selected(self.sync.source_database.clone(), cx);
                combo.set_placeholder(database_placeholder.clone(), cx);
                combo.set_enabled(self.sync.source_connection.is_some(), cx);
            });
        }
        if let Some(combo) = self.sync.target_database_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(target_databases, cx);
                combo.set_selected(self.sync.target_database.clone(), cx);
                combo.set_placeholder(database_placeholder, cx);
                combo.set_enabled(self.sync.target_connection.is_some(), cx);
            });
        }
        // A configuration opened before its connections were open only becomes comparable once
        // both sides finish connecting; start the comparison then.
        if self.sync.needs_refresh
            && self.sync_window.is_some()
            && self.sync_connections_ready()
            && !self.sync.source_database.is_empty()
            && !self.sync.target_database.is_empty()
        {
            self.sync_refresh(cx);
        }
    }

    /// Whether both sides of the transfer are connected, so a comparison can run.
    fn sync_connections_ready(&self) -> bool {
        let (Some(source), Some(target)) =
            (self.sync.source_connection, self.sync.target_connection)
        else {
            return false;
        };
        [source, target].iter().all(|index| {
            matches!(
                self.connections.get(*index).map(|node| &node.status),
                Some(ConnectionStatus::Connected(_))
            )
        })
    }

    fn first_database_name(&self, connection_index: usize) -> Option<String> {
        match self
            .connections
            .get(connection_index)
            .map(|node| &node.databases)
        {
            Some(Loadable::Loaded(databases)) => databases.first().map(|db| db.name.clone()),
            _ => None,
        }
    }

    pub(super) fn sync_source_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        let index = value.parse::<usize>().ok();
        self.sync.source_connection = index;
        self.sync.source_database = index
            .and_then(|index| self.first_database_name(index))
            .unwrap_or_default();
        self.sync.config_saved = false;
        if let Some(index) = index
            && !matches!(
                self.connections.get(index).map(|node| &node.status),
                Some(ConnectionStatus::Connected(_))
            )
        {
            self.connect(index, cx);
        }
        self.sync_refresh(cx);
        cx.notify();
    }

    pub(super) fn sync_target_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        let index = value.parse::<usize>().ok();
        self.sync.target_connection = index;
        self.sync.target_database = index
            .and_then(|index| self.first_database_name(index))
            .unwrap_or_default();
        self.sync.config_saved = false;
        if let Some(index) = index
            && !matches!(
                self.connections.get(index).map(|node| &node.status),
                Some(ConnectionStatus::Connected(_))
            )
        {
            self.connect(index, cx);
        }
        self.sync_refresh(cx);
        cx.notify();
    }

    pub(super) fn sync_source_database_selected(
        &mut self,
        value: &str,
        cx: &mut Context<'_, Self>,
    ) {
        self.sync.source_database = value.to_string();
        self.sync.config_saved = false;
        self.sync_refresh(cx);
        cx.notify();
    }

    pub(super) fn sync_target_database_selected(
        &mut self,
        value: &str,
        cx: &mut Context<'_, Self>,
    ) {
        self.sync.target_database = value.to_string();
        self.sync.config_saved = false;
        self.sync_refresh(cx);
        cx.notify();
    }

    /// Whether both sides use the same engine. Structure sync needs it: the DDL one engine reports
    /// is not valid on another.
    fn sync_same_engine(&self) -> bool {
        match (self.sync.source_connection, self.sync.target_connection) {
            (Some(source), Some(target)) => {
                let source = self
                    .connections
                    .get(source)
                    .map(|node| &node.profile.driver);
                let target = self
                    .connections
                    .get(target)
                    .map(|node| &node.profile.driver);
                source == target
            }
            _ => true,
        }
    }

    fn sync_engine_name(&self, connection: Option<usize>) -> String {
        connection
            .and_then(|index| self.connections.get(index))
            .and_then(|node| self.registry.get(&node.profile.driver))
            .map(|driver| driver.display_name())
            .unwrap_or_default()
    }

    // ----- Saved transfer configurations ---------------------------------------------------------

    /// The saved configurations after the list's search filter, in model order.
    pub(super) fn visible_transfer_indices(&self) -> Vec<usize> {
        let needle = self.sync.transfer_search.trim().to_lowercase();
        self.transfer_configs
            .iter()
            .enumerate()
            .filter(|(_, config)| needle.is_empty() || config.name.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect()
    }

    /// The visible configuration indices in the 详细列表's active sort order.
    fn sorted_transfer_indices(&self) -> Vec<usize> {
        let mut indices = self.visible_transfer_indices();
        let sort = *self.sync.transfer_sort.borrow();
        sort.sort_by(&mut indices, |index| {
            let Some(config) = self.transfer_configs.get(*index) else {
                return SortKey::Empty;
            };
            match sort.column {
                1 => {
                    SortKey::text(&self.transfer_scope_label(
                        &config.source_connection_id,
                        &config.source_database,
                    ))
                }
                2 => {
                    SortKey::text(&self.transfer_scope_label(
                        &config.target_connection_id,
                        &config.target_database,
                    ))
                }
                3 => SortKey::text(&sync_mode_label(SyncMode::from_saved(config.mode))),
                _ => SortKey::text(&config.name),
            }
        });
        indices
    }

    /// `connection · database` for a saved scope, or a muted placeholder when unset.
    fn transfer_scope_label(&self, connection_id: &str, database: &str) -> String {
        if connection_id.is_empty() && database.is_empty() {
            return String::new();
        }
        let connection = self
            .connections
            .iter()
            .find(|node| node.profile.id == connection_id)
            .map(|node| node.profile.name.clone())
            .unwrap_or_else(|| connection_id.to_string());
        if database.is_empty() {
            connection
        } else {
            format!("{connection} · {database}")
        }
    }

    fn transfer_key(&self, index: usize) -> Option<String> {
        self.transfer_configs
            .get(index)
            .map(|config| format!("transfer:{}", config.name))
    }

    fn transfer_index_by_key(&self, key: &str) -> Option<usize> {
        let name = key.strip_prefix("transfer:")?;
        self.transfer_configs
            .iter()
            .position(|config| config.name == name)
    }

    /// The visible rows' selection keys, in the order the list shows them.
    fn visible_transfer_keys(&self) -> Vec<String> {
        let indices = if self.view_mode(VIEW_PAGE_TRANSFERS) == ViewMode::Detail {
            self.sorted_transfer_indices()
        } else {
            self.visible_transfer_indices()
        };
        indices
            .into_iter()
            .filter_map(|index| self.transfer_key(index))
            .collect()
    }

    /// The selected configurations, resolved back from the selection's name keys.
    fn selected_transfer_indices(&self) -> Vec<usize> {
        self.transfers_selection
            .items()
            .iter()
            .filter_map(|key| self.transfer_index_by_key(key))
            .collect()
    }

    /// Apply one list-row click under the click's modifier mode.
    pub(super) fn hit_transfer(
        &mut self,
        key: &str,
        modifiers: Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        let visible = self.visible_transfer_keys();
        if modifiers.shift {
            self.transfers_selection.extend_to(&visible, key);
        } else {
            self.transfers_selection
                .hit(&visible, key, selection_mode(modifiers));
        }
        cx.notify();
    }

    /// Open the 新建传输 window with a fresh scope. The source starts at the tree's current
    /// database, but the target deliberately starts empty: a transfer must name its destination
    /// rather than silently default to the source or to another open connection.
    pub(super) fn open_new_transfer(&mut self, cx: &mut Context<'_, Self>) {
        let Some(source) = self.default_query_connection(cx) else {
            self.error_dialog = Some(t!("sync.error.not_connected").to_string());
            cx.notify();
            return;
        };
        // A file-based engine's `profile.database` is the file path, not a database name; leave
        // the scope empty so the combos pick the file's own database (`main`) after connecting.
        let file_based = self
            .connections
            .get(source)
            .and_then(|node| self.registry.get(&node.profile.driver))
            .is_some_and(|driver| driver.is_file_based());
        let source_database = if file_based {
            String::new()
        } else {
            self.default_query_database(source, cx).unwrap_or_default()
        };
        // Reset the page for the new configuration.
        self.sync.config_name = String::new();
        self.sync.config_saved = false;
        self.sync.config_error = None;
        self.sync.pending_tables = None;
        self.sync.mode = SyncMode::All;
        self.sync.include_delete = true;
        self.sync.existing_only = false;
        self.sync.source_connection = Some(source);
        self.sync.source_database = source_database;
        self.sync.target_connection = None;
        self.sync.target_database = String::new();
        self.reset_sync_run();
        if !matches!(
            self.connections.get(source).map(|node| &node.status),
            Some(ConnectionStatus::Connected(_))
        ) {
            self.connect(source, cx);
        }
        let weak = cx.weak_entity();
        self.sync.config_name_input = Some(make_transfer_name_input(
            self.theme,
            String::new(),
            &weak,
            cx,
        ));
        self.sync.needs_refresh = false;
        self.sync_refresh(cx);
        self.open_sync_window(t!("sync.window.new").to_string(), cx);
        cx.notify();
    }

    /// Open a saved configuration in the 传输 window.
    pub(super) fn open_transfer_config(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(config) = self.transfer_configs.get(index).cloned() else {
            return;
        };
        self.sync.config_name = config.name.clone();
        self.sync.config_saved = true;
        self.sync.config_error = None;
        self.sync.mode = SyncMode::from_saved(config.mode);
        self.sync.include_delete = config.include_delete;
        self.sync.existing_only = config.existing_only;
        self.sync.pending_tables = (!config.tables.is_empty())
            .then(|| config.tables.iter().cloned().collect::<HashSet<String>>());
        let source = self
            .connections
            .iter()
            .position(|node| node.profile.id == config.source_connection_id);
        let target = self
            .connections
            .iter()
            .position(|node| node.profile.id == config.target_connection_id);
        self.sync.source_connection = source;
        self.sync.source_database = source
            .map(|_| config.source_database.clone())
            .unwrap_or_default();
        self.sync.target_connection = target;
        self.sync.target_database = target
            .map(|_| config.target_database.clone())
            .unwrap_or_default();
        for index in [source, target].into_iter().flatten() {
            if !matches!(
                self.connections.get(index).map(|node| &node.status),
                Some(ConnectionStatus::Connected(_))
            ) {
                self.connect(index, cx);
            }
        }
        self.reset_sync_run();
        let weak = cx.weak_entity();
        self.sync.config_name_input = Some(make_transfer_name_input(
            self.theme,
            config.name.clone(),
            &weak,
            cx,
        ));
        self.sync.needs_refresh = false;
        if self.sync_connections_ready() {
            self.sync_refresh(cx);
        } else if self.sync.source_connection.is_some() && self.sync.target_connection.is_some() {
            self.sync.needs_refresh = true;
        }
        self.open_sync_window(
            t!("sync.window.edit", name = config.name.clone()).to_string(),
            cx,
        );
        cx.notify();
    }

    /// Clear the previous run's progress/log so a freshly opened configuration starts clean.
    fn reset_sync_run(&mut self) {
        self.sync.running = false;
        self.sync.progress = 0.0;
        self.sync.log.clear();
        self.sync.log_open = false;
        self.sync.started = None;
        self.sync.elapsed = None;
        self.sync.error = None;
        self.sync.tables.clear();
        self.sync.selected = None;
        self.sync.compared = 0;
        self.sync.compare_total = 0;
        self.sync.options_open = false;
    }

    /// Write the window's current scope, mode, options and table selection into `transfers.json`.
    /// An existing name is replaced in place, so 保存配置 doubles as 另存为 when the name changes.
    pub(super) fn save_transfer_config(&mut self, cx: &mut Context<'_, Self>) {
        let name = self.sync.config_name.trim().to_string();
        if name.is_empty() {
            self.sync.config_error = Some(t!("sync.config.error_name").to_string());
            cx.notify();
            return;
        }
        let (Some(source), Some(target)) =
            (self.sync.source_connection, self.sync.target_connection)
        else {
            self.sync.config_error = Some(t!("sync.config.error_scope").to_string());
            cx.notify();
            return;
        };
        if self.sync.source_database.is_empty() || self.sync.target_database.is_empty() {
            self.sync.config_error = Some(t!("sync.config.error_scope").to_string());
            cx.notify();
            return;
        }
        let checked: Vec<String> = self
            .sync
            .checked_indices()
            .into_iter()
            .filter_map(|index| self.sync.tables.get(index).map(|table| table.name.clone()))
            .collect();
        if !self.sync.tables.is_empty() && checked.is_empty() {
            self.sync.config_error = Some(t!("sync.config.error_tables").to_string());
            cx.notify();
            return;
        }
        let all_tables = !self.sync.tables.is_empty() && checked.len() == self.sync.tables.len();
        let config = SavedTransfer {
            name: name.clone(),
            source_connection_id: self
                .connections
                .get(source)
                .map(|node| node.profile.id.clone())
                .unwrap_or_default(),
            source_database: self.sync.source_database.clone(),
            target_connection_id: self
                .connections
                .get(target)
                .map(|node| node.profile.id.clone())
                .unwrap_or_default(),
            target_database: self.sync.target_database.clone(),
            mode: self.sync.mode.to_saved(),
            include_delete: self.sync.include_delete,
            existing_only: self.sync.existing_only,
            tables: if all_tables { Vec::new() } else { checked },
            comment: String::new(),
        };
        let mut configs = std::mem::take(&mut self.transfer_configs);
        match configs.iter().position(|existing| existing.name == name) {
            Some(index) => configs[index] = config,
            None => configs.push(config),
        }
        self.transfer_configs = configs;
        match self.config.save_transfers(&self.transfer_configs) {
            Ok(()) => {
                self.sync.config_saved = true;
                self.sync.config_error = None;
            }
            Err(error) => {
                self.sync.config_error = Some(error.to_string());
                self.sync.config_saved = false;
            }
        }
        cx.notify();
    }

    /// Ask before deleting the selected configurations.
    pub(super) fn confirm_delete_transfer(&mut self, cx: &mut Context<'_, Self>) {
        let mut indices = self.selected_transfer_indices();
        if indices.is_empty() {
            return;
        }
        indices.sort_unstable();
        indices.dedup();
        self.delete_confirm = Some(DeleteConfirm::TransferConfigs { indices });
        cx.notify();
    }

    pub(super) fn delete_transfer_configs(
        &mut self,
        indices: Vec<usize>,
        cx: &mut Context<'_, Self>,
    ) {
        let mut indices = indices;
        indices.sort_unstable();
        indices.dedup();
        let mut removed = false;
        for index in indices.into_iter().rev() {
            if index < self.transfer_configs.len() {
                self.transfer_configs.remove(index);
                removed = true;
            }
        }
        if removed && let Err(error) = self.config.save_transfers(&self.transfer_configs) {
            self.warn_persist("transfer configurations", error, cx);
        }
        self.transfers_selection.clear();
        cx.notify();
    }

    /// Open (or focus) the OS window hosting the 传输 workbench. Like the Backup window, it is a
    /// normal top-level window so the page keeps the main window free; the open is deferred because
    /// the new window renders immediately and reads `AppView`.
    pub(super) fn open_sync_window(&mut self, title: String, cx: &mut Context<'_, Self>) {
        if self.sync_window.is_some() {
            return;
        }
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        let focus = self.sync_window_focus.clone();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(1120.0), px(720.0)), cx);
            let view_weak = weak.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.clone().into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    #[cfg(target_os = "windows")]
                    crate::win_resize::install(window);
                    window.activate_window();
                    let view = cx.new(|cx| TransferWindow::new(view_weak.clone(), &app_entity, cx));
                    let root = cx.new(|cx| gpui_kit::component::Root::new(view, window, cx));
                    window.focus(&focus, cx);
                    root
                },
            );
            match opened {
                Ok(handle) => {
                    app.update(cx, |app, cx| {
                        app.sync_window = Some(handle);
                        cx.notify();
                    });
                }
                Err(error) => {
                    app.update(cx, |app, cx| {
                        app.error_dialog = Some(error.to_string());
                        cx.notify();
                    });
                }
            }
        });
    }

    // ----- Comparison ----------------------------------------------------------------------------

    /// Reload the source's tables and compare every one against the target.
    pub(super) fn sync_refresh(&mut self, cx: &mut Context<'_, Self>) {
        let (Some(source_index), Some(target_index)) =
            (self.sync.source_connection, self.sync.target_connection)
        else {
            self.sync.tables.clear();
            self.sync.selected = None;
            self.sync.comparing = false;
            cx.notify();
            return;
        };
        if self.sync.source_database.is_empty() || self.sync.target_database.is_empty() {
            self.sync.tables.clear();
            self.sync.selected = None;
            self.sync.comparing = false;
            cx.notify();
            return;
        }
        // A saved configuration can name connections that are still opening; retry from
        // `sync_sync_combos` once both report connected.
        if !self.sync_connections_ready() {
            self.sync.tables.clear();
            self.sync.selected = None;
            self.sync.comparing = false;
            self.sync.needs_refresh = true;
            cx.notify();
            return;
        }
        self.sync.needs_refresh = false;
        let (Some(source), Some(target)) = (
            self.connection_arc(source_index),
            self.connection_arc(target_index),
        ) else {
            self.sync.error = Some(t!("sync.error.not_connected").to_string());
            self.sync.comparing = false;
            cx.notify();
            return;
        };

        self.sync.generation = self.sync.generation.wrapping_add(1);
        let generation = self.sync.generation;
        self.sync.tables.clear();
        self.sync.selected = None;
        self.sync.error = None;
        self.sync.comparing = true;
        self.sync.compared = 0;
        self.sync.compare_total = 0;
        let source_database = self.sync.source_database.clone();
        let target_database = self.sync.target_database.clone();
        // A saved configuration carries its table selection; apply it to the list this run builds.
        let pending_tables = self.sync.pending_tables.take();
        let runtime = self.runtime.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let source_tables =
                match list_tables(runtime.clone(), source.clone(), source_database.clone()).await {
                    Ok(tables) => tables,
                    Err(error) => {
                        let _ = this.update(cx, |app, cx| {
                            if generation == app.sync.generation {
                                app.sync.error = Some(error);
                                app.sync.comparing = false;
                            }
                            cx.notify();
                        });
                        return;
                    }
                };
            let target_tables =
                list_tables(runtime.clone(), target.clone(), target_database.clone())
                    .await
                    .unwrap_or_default();
            let target_names: HashSet<String> = target_tables
                .into_iter()
                .filter(|table| matches!(table.kind, ObjectKind::Table))
                .map(|table| table.name)
                .collect();
            let names: Vec<String> = source_tables
                .into_iter()
                .filter(|table| matches!(table.kind, ObjectKind::Table))
                .map(|table| table.name)
                .collect();
            let total = names.len();
            let _ = this.update(cx, |app, cx| {
                if generation != app.sync.generation {
                    return;
                }
                app.sync.tables = names
                    .iter()
                    .map(|name| {
                        let mut table = SyncTable::new(name.clone(), target_names.contains(name));
                        if let Some(saved) = pending_tables.as_ref() {
                            table.checked = saved.contains(name);
                        }
                        table
                    })
                    .collect();
                app.sync.compare_total = total;
                app.sync.selected = (!names.is_empty()).then_some(0);
                cx.notify();
            });

            for (index, name) in names.iter().enumerate() {
                let still_current = this
                    .update(cx, |app, _cx| generation == app.sync.generation)
                    .unwrap_or(false);
                if !still_current {
                    return;
                }
                let in_target = target_names.contains(name);
                let compared = compare_table(
                    runtime.clone(),
                    source.clone(),
                    source_database.clone(),
                    target.clone(),
                    target_database.clone(),
                    name.clone(),
                    in_target,
                )
                .await;
                let _ = this.update(cx, |app, cx| {
                    if generation != app.sync.generation {
                        return;
                    }
                    app.apply_sync_comparison(index, compared);
                    cx.notify();
                });
            }

            let _ = this.update(cx, |app, cx| {
                if generation == app.sync.generation {
                    app.sync.comparing = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Install one table's comparison result.
    fn apply_sync_comparison(&mut self, index: usize, compared: ComparedTable) {
        self.sync.compared += 1;
        let Some(table) = self.sync.tables.get_mut(index) else {
            return;
        };
        table.comparing = false;
        table.source_schema = compared.source_schema;
        table.target_schema = compared.target_schema;
        table.schema_changes = compared.schema_changes;
        table.source_ddl = compared.source_ddl;
        table.target_ddl = compared.target_ddl;
        table.data = compared.data;
        table.error = compared.error;
    }

    /// Re-compare one table (the inspector's refresh action).
    pub(super) fn sync_recompare_table(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let (Some(source_index), Some(target_index)) =
            (self.sync.source_connection, self.sync.target_connection)
        else {
            return;
        };
        let (Some(source), Some(target)) = (
            self.connection_arc(source_index),
            self.connection_arc(target_index),
        ) else {
            return;
        };
        let Some(table) = self.sync.tables.get_mut(index) else {
            return;
        };
        let name = table.name.clone();
        let in_target = table.in_target;
        table.comparing = true;
        table.error = None;
        let generation = self.sync.generation;
        let source_database = self.sync.source_database.clone();
        let target_database = self.sync.target_database.clone();
        let runtime = self.runtime.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let compared = compare_table(
                runtime,
                source,
                source_database,
                target,
                target_database,
                name,
                in_target,
            )
            .await;
            let _ = this.update(cx, |app, cx| {
                if generation == app.sync.generation {
                    app.apply_sync_comparison(index, compared);
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ----- Running -------------------------------------------------------------------------------

    /// Apply the checked tables' plans, sequentially, streaming the log.
    pub(super) fn run_sync(&mut self, cx: &mut Context<'_, Self>) {
        if self.sync.running {
            return;
        }
        let (Some(source_index), Some(target_index)) =
            (self.sync.source_connection, self.sync.target_connection)
        else {
            self.sync.error = Some(t!("sync.error.no_scope").to_string());
            cx.notify();
            return;
        };
        if self.sync.source_database == self.sync.target_database && source_index == target_index {
            self.sync.error = Some(t!("sync.error.same").to_string());
            cx.notify();
            return;
        }
        let (Some(source), Some(target)) = (
            self.connection_arc(source_index),
            self.connection_arc(target_index),
        ) else {
            self.sync.error = Some(t!("sync.error.not_connected").to_string());
            cx.notify();
            return;
        };
        let mode = self.sync.mode;
        let same_engine = self.sync_same_engine();
        if mode.uses_schema() && !same_engine && mode != SyncMode::Transfer {
            self.sync.error = Some(t!("sync.error.cross_engine_schema").to_string());
            cx.notify();
            return;
        }
        let plans: Vec<SyncPlan> = self
            .sync
            .checked_indices()
            .into_iter()
            .filter_map(|index| self.sync.tables.get(index))
            .filter(|table| table.has_changes(mode, same_engine))
            .map(|table| SyncPlan {
                name: table.name.clone(),
                in_target: table.in_target,
                has_key: table.has_key(),
                source_schema: table.source_schema.clone(),
                target_schema: table.target_schema.clone(),
                schema_changes: table.schema_changes.clone(),
                data: table.data.clone(),
                force_transfer: table.force_transfer,
            })
            .collect();
        if plans.is_empty() {
            self.sync.error = Some(t!("sync.error.nothing").to_string());
            cx.notify();
            return;
        }
        let total = plans.len();
        self.sync.running = true;
        self.sync.log_open = true;
        self.sync.log.clear();
        self.sync.progress = 0.0;
        self.sync.started = Some(Instant::now());
        self.sync.elapsed = None;
        self.sync.error = None;
        self.sync.cancel.store(false, Ordering::Relaxed);
        let line = t!("sync.log.started", time = now_timestamp()).to_string();
        self.sync.log_line(line);
        cx.notify();

        let cancel = self.sync.cancel.clone();
        let source_database = self.sync.source_database.clone();
        let target_database = self.sync.target_database.clone();
        let include_delete = self.sync.include_delete;
        let existing_only = self.sync.existing_only;
        let runtime = self.runtime.clone();

        cx.spawn(async move |this, cx| {
            let mut done = 0usize;
            let mut failures = 0usize;
            for plan in plans {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if existing_only && !plan.in_target {
                    let message =
                        t!("sync.log.skipped_missing", name = plan.name.clone()).to_string();
                    let _ = this.update(cx, |app, cx| app.sync_log(message, cx));
                    done += 1;
                    continue;
                }
                let outcome = run_sync_table(
                    runtime.clone(),
                    source.clone(),
                    source_database.clone(),
                    target.clone(),
                    target_database.clone(),
                    &plan,
                    mode,
                    same_engine,
                    include_delete,
                    &cancel,
                )
                .await;
                done += 1;
                let progress = done as f32 / total as f32;
                let failed = outcome.is_err();
                let _ = this.update(cx, |app, cx| {
                    app.sync.progress = progress;
                    match outcome {
                        Ok(summary) => {
                            if let Some(table) = app
                                .sync
                                .tables
                                .iter_mut()
                                .find(|table| table.name == plan.name)
                            {
                                table.synced = true;
                                table.in_target = true;
                                table.schema_changes.clear();
                                table.data = DataDiff::default();
                                table.force_transfer = false;
                            }
                            app.sync_log(summary, cx);
                        }
                        Err(error) => {
                            let message =
                                t!("sync.log.failed", name = plan.name.clone(), error = error)
                                    .to_string();
                            app.sync_log(message, cx);
                        }
                    }
                    cx.notify();
                });
                if failed {
                    failures += 1;
                }
            }
            let cancelled = cancel.load(Ordering::Relaxed);
            let _ = this.update(cx, |app, cx| {
                app.sync.running = false;
                app.sync.elapsed = app.sync.started.map(|started| started.elapsed());
                let elapsed = app
                    .sync
                    .elapsed
                    .map(format_hms)
                    .unwrap_or_else(|| "--".to_string());
                let message = if cancelled {
                    t!("sync.log.cancelled").to_string()
                } else if failures > 0 {
                    t!(
                        "sync.log.done_failed",
                        tables = done.saturating_sub(failures),
                        failed = failures,
                        elapsed = elapsed
                    )
                    .to_string()
                } else {
                    t!("sync.log.done", tables = done, elapsed = elapsed).to_string()
                };
                app.sync.log_line(message);
                if let Some(index) = app.sync.target_connection {
                    app.load_databases(index, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_log(&mut self, line: String, cx: &mut Context<'_, Self>) {
        self.sync.log_line(line);
        cx.notify();
    }

    pub(super) fn cancel_sync(&mut self, cx: &mut Context<'_, Self>) {
        self.sync.cancel.store(true, Ordering::Relaxed);
        cx.notify();
    }

    // ----- SQL preview ---------------------------------------------------------------------------

    /// The script a run of `table` would execute for the current mode. The structure half comes
    /// from the target driver's own `table_schema_sql`, so the preview matches what Save runs.
    fn sync_script(&self, table: &SyncTable, mode: SyncMode) -> String {
        let dialect = self
            .sync
            .target_connection
            .map(|index| self.driver_dialect(index))
            .unwrap_or_default();
        let database = self.sync.target_database.clone();
        let connection = self
            .sync
            .target_connection
            .and_then(|index| self.connection_arc(index));
        let mut lines: Vec<String> = Vec::new();
        if mode.uses_schema() {
            lines.push(t!("sync.sql.schema_stage").to_string());
            if !self.sync_same_engine() {
                lines.push(t!("sync.sql.cross_engine").to_string());
            } else if let Some(source) = table.source_schema.as_ref() {
                match connection {
                    Some(connection) => {
                        let sql = if mode == SyncMode::Transfer || !table.in_target {
                            connection.table_schema_sql(&database, &table.name, None, source)
                        } else {
                            connection.table_schema_sql(
                                &database,
                                &table.name,
                                table.target_schema.as_ref(),
                                source,
                            )
                        };
                        lines.push(if sql.trim().is_empty() {
                            t!("sync.sql.no_schema_change").to_string()
                        } else {
                            sql
                        });
                    }
                    None => lines.push(t!("sync.error.not_connected").to_string()),
                }
            } else {
                lines.push(t!("sync.sql.no_schema_change").to_string());
            }
        }
        if mode.uses_data() {
            lines.push(String::new());
            lines.push(t!("sync.sql.data_stage").to_string());
            if !table.in_target {
                lines.push(t!("sync.sql.missing_target").to_string());
            } else if mode == SyncMode::Transfer || table.force_transfer {
                if mode == SyncMode::Transfer && self.sync_same_engine() {
                    lines.push(format!(
                        "DROP TABLE IF EXISTS {};",
                        dialect.quote_identifier(&table.name)
                    ));
                } else {
                    lines.push(format!(
                        "DELETE FROM {};",
                        dialect.quote_identifier(&table.name)
                    ));
                }
                lines.push(t!("sync.sql.full_copy", table = table.name.clone()).to_string());
            } else if !table.has_key() {
                lines.push(t!("sync.sql.no_key").to_string());
            } else if table.data.is_empty() {
                lines.push(t!("sync.sql.no_data_change").to_string());
            } else {
                let mut rendered = 0usize;
                for change in &table.data.rows {
                    if change.kind == RowChangeKind::Delete && !self.sync.include_delete {
                        continue;
                    }
                    if rendered >= SYNC_PREVIEW_MAX_ROWS {
                        lines.push(
                            t!(
                                "sync.sql.more",
                                count = table.data.rows.len().saturating_sub(rendered)
                            )
                            .to_string(),
                        );
                        break;
                    }
                    lines.push(render_change_sql(&table.name, change, dialect));
                    rendered += 1;
                }
            }
        }
        lines.join("\n")
    }

    // ----- Rendering -----------------------------------------------------------------------------

    // ----- Rendering: the saved-transfer list ----------------------------------------------------

    /// The 传输 main tab. It lists the saved configurations exactly like the Backup tab; a
    /// configuration opens in a child window that owns the comparison workbench, so the page
    /// itself stays a list.
    pub(super) fn render_sync(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let empty = self.transfer_configs.is_empty();
        let body: AnyElement = if empty {
            self.render_transfer_empty(cx)
        } else if self.view_mode(VIEW_PAGE_TRANSFERS) == ViewMode::Detail {
            self.render_transfer_detail(cx)
        } else {
            self.render_transfer_tiles(cx)
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(self.render_transfer_toolbar(cx))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .bg(rgb(theme.editor_bg))
                    .track_focus(&self.sync.transfer_focus)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.begin_marquee(
                                MarqueeTarget::Transfers,
                                event.position,
                                event.modifiers,
                                cx,
                            );
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                            // A right-click on the blank area just clears the highlight; the
                            // toolbar's 删除 is the only destructive path.
                            this.transfers_selection.clear();
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        if this.sync.transfer_columns.borrow_mut().drag_resize(event) {
                            cx.notify();
                        }
                        this.drag_marquee(MarqueeTarget::Transfers, event, cx);
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                            if this.sync.transfer_columns.borrow_mut().end_resize() {
                                cx.notify();
                            }
                            this.end_marquee(cx);
                        }),
                    )
                    .child(body)
                    .when_some(
                        self.marquee_rect_for(MarqueeTarget::Transfers),
                        |pane, rect| pane.child(rect),
                    ),
            )
            .into_any_element()
    }

    fn render_transfer_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let single = self
            .transfers_selection
            .single()
            .and_then(|key| self.transfer_index_by_key(key));
        let has_selection = !self.transfers_selection.is_empty();
        let weak = cx.weak_entity();
        let on_select = Rc::new(
            move |mode: ViewMode, _event: &ClickEvent, _window: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |app, cx| {
                    app.set_view_mode(VIEW_PAGE_TRANSFERS, mode, cx)
                });
            },
        );
        let view_mode = self.view_mode(VIEW_PAGE_TRANSFERS);
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(self.toolbar_item(
                        "transfer-new",
                        "icons/plus.svg",
                        t!("sync.new").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| this.open_new_transfer(cx)),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "transfer-edit",
                        "icons/design_table.svg",
                        t!("sync.edit").to_string(),
                        single.is_some(),
                        cx.listener(move |this, _event, _window, cx| {
                            if let Some(index) = single {
                                this.open_transfer_config(index, cx);
                            }
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "transfer-delete",
                        "icons/delete_table.svg",
                        t!("sync.delete").to_string(),
                        has_selection,
                        cx.listener(|this, _event, _window, cx| this.confirm_delete_transfer(cx)),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(ui::view_mode_toggle(theme, view_mode, on_select))
                    .child(
                        div()
                            .w(px(220.0))
                            .h(px(24.0))
                            .child(self.sync.transfer_search_input.clone()),
                    ),
            )
    }

    /// A centred hint shown while no transfer configuration exists.
    fn render_transfer_empty(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                svg()
                    .path("icons/sync.svg")
                    .w(px(32.0))
                    .h(px(32.0))
                    .text_color(rgb(theme.text_muted)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("sync.config.empty").to_string()),
            )
            .child(ui::button(
                "transfer-empty-new",
                t!("sync.new").to_string(),
                ButtonKind::Default,
                theme,
                cx.listener(|this, _event, _window, cx| this.open_new_transfer(cx)),
            ))
            .into_any_element()
    }

    /// The content-fitted widths of the 传输 columns (名称 / 源 / 目标 / 模式).
    fn transfer_detail_widths(&self) -> Vec<f32> {
        let mut longest = [
            ui::approx_text_width(&t!("common.name")) + 30.0,
            ui::approx_text_width(&t!("sync.config.source")),
            ui::approx_text_width(&t!("sync.config.target")),
            ui::approx_text_width(&t!("sync.config.mode")),
        ];
        for index in self.visible_transfer_indices() {
            let Some(config) = self.transfer_configs.get(index) else {
                continue;
            };
            longest[0] = longest[0].max(ui::approx_text_width(&config.name) + 30.0);
            longest[1] = longest[1].max(ui::approx_text_width(
                &self.transfer_scope_label(&config.source_connection_id, &config.source_database),
            ));
            longest[2] = longest[2].max(ui::approx_text_width(
                &self.transfer_scope_label(&config.target_connection_id, &config.target_database),
            ));
            longest[3] = longest[3].max(ui::approx_text_width(&sync_mode_label(
                SyncMode::from_saved(config.mode),
            )));
        }
        vec![
            ui::detail_column_width(longest[0], 220.0),
            ui::detail_column_width(longest[1], 170.0),
            ui::detail_column_width(longest[2], 170.0),
            ui::detail_column_width(longest[3], 110.0),
        ]
    }

    /// The 传输 详细列表: 名称 / 源 / 目标 / 模式.
    fn render_transfer_detail(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let fitted = self.transfer_detail_widths();
        let widths = self.sync.transfer_columns.borrow_mut().resolve(&fitted);
        let mut header = ui::detail_header_row(theme);
        for (index, label) in [
            t!("common.name").to_string(),
            t!("sync.config.source").to_string(),
            t!("sync.config.target").to_string(),
            t!("sync.config.mode").to_string(),
        ]
        .into_iter()
        .enumerate()
        {
            let width = widths[index];
            let cell = ui::detail_header_cell(
                SharedString::from(format!("transfer-sort-{index}")),
                label,
                self.sync.transfer_sort.borrow().indicator(index),
                theme,
                cx.listener(move |this, _event, _window, cx| {
                    this.sync.transfer_sort.borrow_mut().toggle(index);
                    cx.notify();
                }),
            );
            header = header.child(ui::detail_header_column(
                SharedString::from(format!("transfer-resize-{index}")),
                width,
                self.sync.transfer_columns.borrow().resizing(index),
                theme,
                cell,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.sync.transfer_columns.borrow_mut().begin_resize(
                        index,
                        event.position.x,
                        width,
                    );
                    cx.notify();
                }),
            ));
        }

        let mut list = ui::DetailList::new(
            "transfer-detail",
            &self.sync.transfer_detail_scroll,
            ui::detail_content_width(&widths),
            header,
        );
        for index in self.sorted_transfer_indices() {
            list = list.child(self.transfer_detail_row(index, &widths, theme, cx));
        }
        list.render(theme)
    }

    fn transfer_detail_row(
        &self,
        index: usize,
        widths: &[f32],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let Some(config) = self.transfer_configs.get(index) else {
            return div().into_any_element();
        };
        let key = format!("transfer:{}", config.name);
        let selected = self.transfers_selection.contains(&key);
        let name = config.name.clone();
        let source =
            self.transfer_scope_label(&config.source_connection_id, &config.source_database);
        let target =
            self.transfer_scope_label(&config.target_connection_id, &config.target_database);
        let mode = sync_mode_label(SyncMode::from_saved(config.mode));
        let click_key = key.clone();
        let open_key = key.clone();
        let row = ui::detail_row(
            SharedString::from(format!("transfer-row-{index}")),
            selected,
            theme,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            this.hit_transfer(&click_key, event.modifiers(), cx);
            if event.click_count() >= 2
                && let Some(index) = this.transfer_index_by_key(&open_key)
            {
                this.open_transfer_config(index, cx);
            }
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .w(px(widths[0]))
                .flex_none()
                .overflow_hidden()
                .text_color(rgb(theme.text))
                .child(ui::leading_icon_badge(
                    "icons/sync.svg",
                    theme.brand,
                    ui::DETAIL_BADGE_SIZE,
                ))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("transfer-cell-{index}-0")),
                    name,
                )),
        )
        .child(
            div()
                .w(px(widths[1]))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("transfer-cell-{index}-1")),
                    source,
                )),
        )
        .child(
            div()
                .w(px(widths[2]))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("transfer-cell-{index}-2")),
                    target,
                )),
        )
        .child(
            div()
                .w(px(widths[3]))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("transfer-cell-{index}-3")),
                    mode,
                )),
        );
        super::list_ops::row_with_rect(cx.weak_entity(), row, MarqueeTarget::Transfers, key)
    }

    /// The 传输 平铺网格: saved configurations in a column-major grid.
    fn render_transfer_tiles(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let indices = self.visible_transfer_indices();
        if indices.is_empty() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .into_any_element();
        }
        let width = ui::grid_item_width(
            indices
                .iter()
                .filter_map(|index| self.transfer_configs.get(*index))
                .map(|config| ui::approx_text_width(&config.name))
                .fold(0.0, f32::max),
        );
        let rows = self.sync.transfer_grid.rows_per_column();
        let mut columns = ui::grid_columns();
        let mut column = ui::grid_column();
        let mut count = 0usize;
        for index in indices {
            if count == rows {
                columns = columns.child(column);
                column = ui::grid_column();
                count = 0;
            }
            let Some(config) = self.transfer_configs.get(index) else {
                continue;
            };
            let key = format!("transfer:{}", config.name);
            let selected = self.transfers_selection.contains(&key);
            let name = config.name.clone();
            let click_key = key.clone();
            let open_key = key.clone();
            let tile = ui::grid_item_sized(
                SharedString::from(format!("transfer-tile-{index}")),
                selected,
                theme,
                width,
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                this.hit_transfer(&click_key, event.modifiers(), cx);
                if event.click_count() >= 2
                    && let Some(index) = this.transfer_index_by_key(&open_key)
                {
                    this.open_transfer_config(index, cx);
                }
            }))
            .child(ui::leading_icon_badge("icons/sync.svg", theme.brand, 16.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(theme.text))
                    .child(name),
            );
            column = column.child(super::list_ops::row_with_rect(
                cx.weak_entity(),
                tile,
                MarqueeTarget::Transfers,
                key,
            ));
            count += 1;
        }
        if count > 0 {
            columns = columns.child(column);
        }

        let mut grid = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden();
        grid = grid.child(
            self.sync
                .transfer_grid
                .scroller("transfer-grid-scroll")
                .child(columns),
        );
        if self.sync.transfer_grid.overflows() {
            grid = grid.child(
                self.sync
                    .transfer_grid
                    .scrollbar("transfer-grid-hscrollbar", theme)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            if this.sync.transfer_grid.begin(event.position.x) {
                                cx.notify();
                            }
                        }),
                    ),
            );
        }
        grid.into_any_element()
    }

    // ----- Rendering: the transfer window -------------------------------------------------------

    /// The child window's contents: a themed titlebar, the comparison workbench and the footer that
    /// names and saves the configuration.
    pub(super) fn sync_window_contents(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let title = if self.sync.config_name.trim().is_empty() {
            t!("sync.window.new").to_string()
        } else {
            t!("sync.window.edit", name = self.sync.config_name.clone()).to_string()
        };
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.dialog_face))
            .text_color(rgb(theme.text))
            .track_focus(&self.sync_window_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    if this.sync.options_open {
                        this.sync.options_open = false;
                        cx.notify();
                    } else {
                        window.remove_window();
                    }
                }
            }))
            .child(super::backup::backup_window_titlebar(title, theme))
            .child(self.render_sync_workbench(cx))
            .child(self.render_sync_footer(cx));
        if self.sync.options_open {
            root = root.child(self.render_sync_options(cx));
        }
        root.into_any_element()
    }

    /// The window footer: the configuration name, its save state and 保存配置.
    fn render_sync_footer(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let name_input: AnyElement = match self.sync.config_name_input.clone() {
            Some(input) => div()
                .w(px(260.0))
                .h(px(26.0))
                .child(input)
                .into_any_element(),
            None => div().into_any_element(),
        };
        let (status, color) = if let Some(error) = self.sync.config_error.as_ref() {
            (error.clone(), theme.danger)
        } else if self.sync.config_saved {
            (t!("sync.config.saved").to_string(), theme.brand)
        } else {
            (t!("sync.config.status_idle").to_string(), theme.text_muted)
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .gap_3()
            .px_3()
            .py_2()
            .flex_none()
            .border_t_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.dialog_face))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!("sync.config.name").to_string()),
                    )
                    .child(name_input),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .flex_none()
                    .child(
                        div()
                            .max_w(px(430.0))
                            .truncate()
                            .text_size(px(11.0))
                            .text_color(rgb(color))
                            .child(status),
                    )
                    .child(ui::button(
                        "sync-config-save",
                        t!("sync.config.save").to_string(),
                        ButtonKind::Normal,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.save_transfer_config(cx)),
                    )),
            )
    }

    /// The 数据传输/同步 comparison workbench, shown inside the transfer window.
    pub(super) fn render_sync_workbench(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut page = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .bg(rgb(theme.editor_bg))
            .child(self.render_sync_toolbar(cx));
        if !self.sync_same_engine() {
            page = page.child(self.render_sync_notice());
        }
        page = page.child(
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h(px(0.0))
                .child(self.render_sync_list(cx))
                .child(self.render_sync_inspector(cx)),
        );
        if self.sync.log_open {
            page = page.child(self.render_sync_log(cx));
        }
        page.into_any_element()
    }

    /// The topology + mode switch + actions bar. It uses two rows: the source → target pickers on
    /// the first and the mode switch with the run controls on the second, so the target picker is
    /// never squeezed or overlapped in a narrow window.
    fn render_sync_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let cross_engine = !self.sync_same_engine();
        let checked = self.sync.checked_indices().len();
        let running = self.sync.running;
        let ready = self.sync_connections_ready();

        let mut modes = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.header_bg))
            .flex_none();
        for mode in [
            SyncMode::All,
            SyncMode::Schema,
            SyncMode::Data,
            SyncMode::Transfer,
        ] {
            let active = self.sync.mode == mode;
            let enabled = !cross_engine || !mode.uses_schema() || mode == SyncMode::Transfer;
            let weak = cx.weak_entity();
            modes = modes.child(
                div()
                    .id(SharedString::from(format!(
                        "sync-mode-{}",
                        mode.label_key()
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(24.0))
                    .px_2()
                    .rounded(px(4.0))
                    .text_size(px(12.0))
                    .when(active, |style| {
                        style
                            .bg(rgb(theme.primary))
                            .text_color(rgb(theme.editor_bg))
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!active, |style| style.text_color(rgb(theme.text_muted)))
                    .when(enabled && !active, |style| {
                        style.cursor_pointer().hover(move |style| {
                            style
                                .bg(rgb(theme.tree_hover_bg))
                                .text_color(rgb(theme.text))
                        })
                    })
                    .when(!enabled, |style| style.opacity(0.4))
                    .when(enabled, |style| {
                        style.on_click(move |_event, _window, cx| {
                            let _ = weak.update(cx, |app, cx| {
                                if app.sync.mode != mode {
                                    app.sync.config_saved = false;
                                }
                                app.sync.mode = mode;
                                app.sync.sub_tab = match mode {
                                    SyncMode::Schema => SyncSubTab::Schema,
                                    SyncMode::Data => SyncSubTab::Data,
                                    SyncMode::Transfer => SyncSubTab::Sql,
                                    SyncMode::All => SyncSubTab::Schema,
                                };
                                cx.notify();
                            });
                        })
                    })
                    .child(t!(mode.label_key()).to_string()),
            );
        }

        let anchor = self.sync.options_anchor.clone();
        let options_button = div()
            .on_children_prepainted(move |bounds, _window, _cx| {
                if let Some(bounds) = bounds.first() {
                    *anchor.borrow_mut() = Point::new(bounds.left(), bounds.bottom());
                }
            })
            .child(ui::button(
                "sync-options",
                t!("sync.options").to_string(),
                ButtonKind::Normal,
                theme,
                cx.listener(|this, _event, _window, cx| {
                    this.sync.options_open = !this.sync.options_open;
                    cx.notify();
                }),
            ));

        let run_label = if self.sync.mode == SyncMode::Transfer {
            t!("sync.transfer", count = checked).to_string()
        } else {
            t!("sync.run", count = checked).to_string()
        };
        let run = ui::button(
            "sync-run",
            run_label,
            if checked == 0 || running || self.sync.comparing || !ready {
                ButtonKind::Disabled
            } else {
                ButtonKind::Default
            },
            theme,
            cx.listener(|this, _event, _window, cx| this.run_sync(cx)),
        );

        let target_hint = if self.sync.target_connection.is_none() {
            t!("sync.select_target").to_string()
        } else {
            String::new()
        };

        div()
            .flex()
            .flex_col()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .child(self.sync_endpoint_group(true, theme))
                    .child(
                        div()
                            .flex_none()
                            .px_1()
                            .text_size(px(13.0))
                            .text_color(rgb(theme.text_muted))
                            .child("→"),
                    )
                    .child(self.sync_endpoint_group(false, theme))
                    .child(div().flex_1().min_w(px(0.0))),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_2()
                    .pb_2()
                    .child(modes)
                    .child(div().flex_1().min_w(px(0.0)))
                    .when(!target_hint.is_empty(), |row| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(px(11.0))
                                .text_color(rgb(theme.warning))
                                .child(target_hint),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .flex_none()
                            .child(options_button)
                            .child(run),
                    ),
            )
            .when(self.sync.error.is_some(), |bar| {
                bar.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.danger))
                        .child(self.sync.error.clone().unwrap_or_default()),
                )
            })
    }

    /// One side of the source → target picker: a label, the connection dropdown and the database
    /// dropdown in a single bordered group.
    fn sync_endpoint_group(&self, source: bool, theme: Theme) -> Div {
        let (connection, database) = if source {
            (&self.sync.source_combo, &self.sync.source_database_combo)
        } else {
            (&self.sync.target_combo, &self.sync.target_database_combo)
        };
        let side = if source {
            t!("sync.config.source").to_string()
        } else {
            t!("sync.config.target").to_string()
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .flex_none()
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(side),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(26.0))
                    .px_2()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.editor_bg))
                    .child(
                        div().w(px(150.0)).h(px(24.0)).child(
                            connection
                                .clone()
                                .map(|combo| combo.into_any_element())
                                .unwrap_or_else(|| div().into_any_element()),
                        ),
                    )
                    .child(
                        div().w(px(130.0)).h(px(24.0)).child(
                            database
                                .clone()
                                .map(|combo| combo.into_any_element())
                                .unwrap_or_else(|| div().into_any_element()),
                        ),
                    ),
            )
    }

    /// The cross-engine banner: structure sync is unavailable, so say why.
    fn render_sync_notice(&self) -> Div {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .flex_none()
            .px_3()
            .py_1()
            .bg(rgba((theme.warning << 8) | 0x14))
            .border_b_1()
            .border_color(rgb(theme.border))
            .text_size(px(12.0))
            .text_color(rgb(theme.warning))
            .child(
                svg()
                    .path("icons/warning.svg")
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_none()
                    .text_color(rgb(theme.warning)),
            )
            .child(
                t!(
                    "sync.hetero",
                    source = self.sync_engine_name(self.sync.source_connection),
                    target = self.sync_engine_name(self.sync.target_connection)
                )
                .to_string(),
            )
    }

    /// The table list column.
    fn render_sync_list(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let visible = self.sync.visible_indices();
        let total = self.sync.tables.len();
        let checked = self.sync.checked_indices().len();
        let all_checked = total > 0 && checked == total;
        let weak = cx.weak_entity();

        let mut list = div()
            .id("sync-table-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.sync.list_scroll);
        if self.sync.tables.is_empty() {
            list = list.child(
                div()
                    .px_3()
                    .py_3()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(if self.sync.comparing {
                        t!("common.loading").to_string()
                    } else if self.sync.target_connection.is_none() {
                        t!("sync.select_target").to_string()
                    } else {
                        t!("sync.no_tables").to_string()
                    }),
            );
        }
        for index in visible {
            let Some(table) = self.sync.tables.get(index) else {
                continue;
            };
            let selected = self.sync.selected == Some(index);
            let (status, tone) = table.status(self.sync.mode);
            let is_checked = table.checked;
            let name = table.name.clone();
            let row_weak = weak.clone();
            let check_weak = weak.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("sync-row-{name}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(SYNC_ROW_HEIGHT))
                    .px_2()
                    .mx_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(selected, |style| style.bg(rgb(theme.tree_selected_bg)))
                    .when(!selected, |style| {
                        style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    })
                    .on_click(move |_event, _window, cx| {
                        let _ = row_weak.update(cx, |app, cx| {
                            app.sync.selected = Some(index);
                            app.sync.data_filter = SyncDataFilter::All;
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("sync-check-{name}")))
                            .flex()
                            .items_center()
                            .flex_none()
                            .cursor_pointer()
                            .on_click(move |_event, _window, cx| {
                                cx.stop_propagation();
                                let _ = check_weak.update(cx, |app, cx| {
                                    if let Some(table) = app.sync.tables.get_mut(index) {
                                        table.checked = !table.checked;
                                    }
                                    app.sync.config_saved = false;
                                    cx.notify();
                                });
                            })
                            .child(ui::checkbox_box(is_checked, theme)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text))
                            .child(name.clone()),
                    )
                    .child(sync_pill(&status, tone, theme)),
            );
        }

        div()
            .flex()
            .flex_col()
            .w(px(SYNC_LIST_WIDTH))
            .flex_none()
            .min_h(px(0.0))
            .bg(rgb(theme.sidebar_bg))
            .border_r_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .flex_none()
                    .px_2()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(theme.border))
                    .child(
                        div()
                            .w_full()
                            .h(px(24.0))
                            .child(self.sync.search_input.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .id("sync-select-all")
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .text_size(px(12.0))
                                    .text_color(rgb(theme.text))
                                    .on_click(move |_event, _window, cx| {
                                        let _ = weak.update(cx, |app, cx| {
                                            for table in &mut app.sync.tables {
                                                table.checked = !all_checked;
                                            }
                                            app.sync.config_saved = false;
                                            cx.notify();
                                        });
                                    })
                                    .child(ui::checkbox_box(all_checked, theme))
                                    .child(t!("sync.select_all").to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(
                                        t!("sync.summary", selected = checked, total = total)
                                            .to_string(),
                                    ),
                            ),
                    )
                    .when(self.sync.comparing, |column| {
                        column.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(theme.text_muted))
                                .child(
                                    t!(
                                        "sync.comparing",
                                        done = self.sync.compared,
                                        total = self.sync.compare_total
                                    )
                                    .to_string(),
                                ),
                        )
                    }),
            )
            .child(list)
    }

    /// The inspector: table header, sub-tabs and the active body.
    fn render_sync_inspector(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some((name, status, tone, schema_count, data_count)) =
            self.sync.selected_table().map(|table| {
                let (status, tone) = table.status(self.sync.mode);
                (
                    table.name.clone(),
                    status,
                    tone,
                    table.schema_changes.len(),
                    table.data.total(),
                )
            })
        else {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .items_center()
                .justify_center()
                .bg(rgb(theme.editor_bg))
                .child(
                    div()
                        .max_w(px(420.0))
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(if self.sync.target_connection.is_none() {
                            t!("sync.select_target").to_string()
                        } else {
                            t!("sync.select_table").to_string()
                        }),
                )
                .into_any_element();
        };
        let sub_tab = self.sync.sub_tab;
        let mut tabs = div().flex().flex_row().items_center().h_full().gap_4();
        for (tab, label, count) in [
            (
                SyncSubTab::Schema,
                t!("sync.tab.schema").to_string(),
                schema_count,
            ),
            (
                SyncSubTab::Data,
                t!("sync.tab.data").to_string(),
                data_count,
            ),
            (SyncSubTab::Sql, t!("sync.tab.sql").to_string(), 0usize),
        ] {
            let active = sub_tab == tab;
            let weak = cx.weak_entity();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("sync-tab-{}", label)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h_full()
                    .px_0p5()
                    .border_b_2()
                    .border_color(if active {
                        rgb(theme.text)
                    } else {
                        rgb(theme.editor_bg)
                    })
                    .text_size(px(12.0))
                    .font_weight(if active {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if active {
                        rgb(theme.text)
                    } else {
                        rgb(theme.text_muted)
                    })
                    .cursor_pointer()
                    .on_click(move |_event, _window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            app.sync.sub_tab = tab;
                            cx.notify();
                        });
                    })
                    .child(label)
                    .when(tab != SyncSubTab::Sql, |item| {
                        item.child(
                            div()
                                .px_1()
                                .rounded_full()
                                .border_1()
                                .border_color(rgb(theme.border))
                                .bg(rgb(theme.header_bg))
                                .text_size(px(10.0))
                                .text_color(rgb(theme.text_muted))
                                .child(count.to_string()),
                        )
                    }),
            );
        }

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(38.0))
            .flex_none()
            .px_3()
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .font_family("Consolas")
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text))
                            .child(name),
                    )
                    .child(sync_pill(&status, tone, theme))
                    .child(ui::toolbar_item(
                        "sync-recompare",
                        "icons/refresh.svg",
                        t!("sync.recompare").to_string(),
                        !self.sync.comparing,
                        theme,
                        cx.listener(|this, _event, _window, cx| {
                            if let Some(index) = this.sync.selected {
                                this.sync_recompare_table(index, cx);
                            }
                        }),
                    )),
            )
            .child(tabs);

        let body: AnyElement = match sub_tab {
            SyncSubTab::Schema => self.render_sync_schema_tab(cx),
            SyncSubTab::Data => self.render_sync_data_tab(cx),
            SyncSubTab::Sql => {
                let Some(table) = self.sync.selected_table() else {
                    return div().into_any_element();
                };
                let script = self.sync_script(table, self.sync.mode);
                let key = format!("sync-sql-{}-{:?}", table.name, self.sync.mode);
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .child(self.render_sql_preview(&key, &script, cx))
                    .into_any_element()
            }
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// The 结构比对 tab: source and target DDL side by side, changed lines marked.
    fn render_sync_schema_tab(&self, _cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(table) = self.sync.selected_table() else {
            return div().into_any_element();
        };
        if table.comparing {
            return sync_placeholder(t!("common.loading").to_string(), theme);
        }
        if let Some(error) = table.error.as_ref() {
            return sync_placeholder(error.clone(), theme);
        }
        let source_ddl = table.source_ddl.clone().unwrap_or_default();
        let target_ddl = table.target_ddl.clone().unwrap_or_default();
        let (source_lines, target_lines) = ddl_line_marks(&source_ddl, &target_ddl);
        let exists = table.in_target;

        let column = |title: String, engine: String, lines: Vec<(String, LineMark)>| {
            let mut code = div()
                .id(SharedString::from(format!("sync-ddl-{title}")))
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .overflow_x_scroll()
                .py_2();
            if lines.is_empty() {
                code = code.child(
                    div()
                        .px_3()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child("--"),
                );
            }
            for (line, mark) in lines {
                let (background, color, strike) = match mark {
                    LineMark::Same => (None, theme.text, false),
                    LineMark::Add => (
                        Some((theme.icon_table << 8) | 0x18),
                        theme.icon_table,
                        false,
                    ),
                    LineMark::Del => (Some((theme.danger << 8) | 0x18), theme.danger, true),
                };
                let mut row = div()
                    .flex()
                    .flex_row()
                    .h(px(19.0))
                    .px_3()
                    .whitespace_nowrap()
                    .font_family("Consolas")
                    .text_size(px(12.0))
                    .text_color(rgb(color))
                    .when_some(background, |style, background| style.bg(rgba(background)));
                if strike {
                    row = row.line_through();
                }
                code = code.child(row.child(line));
            }
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .overflow_hidden()
                .border_r_1()
                .border_color(rgb(theme.border))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .h(px(28.0))
                        .flex_none()
                        .px_3()
                        .bg(rgb(theme.header_bg))
                        .border_b_1()
                        .border_color(rgb(theme.border))
                        .text_size(px(11.0))
                        .text_color(rgb(theme.text_muted))
                        .child(title)
                        .child(engine),
                )
                .child(code)
        };

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(column(
                t!("sync.source_ddl").to_string(),
                self.sync_engine_name(self.sync.source_connection),
                source_lines,
            ))
            .child(column(
                t!("sync.target_ddl").to_string(),
                if exists {
                    self.sync_engine_name(self.sync.target_connection)
                } else {
                    t!("sync.not_exists").to_string()
                },
                target_lines,
            ));
        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden();
        if !table.schema_changes.is_empty() {
            let mut summary = div()
                .flex()
                .flex_col()
                .flex_none()
                .gap_0p5()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(theme.border));
            for change in &table.schema_changes {
                let color = match change.kind {
                    SchemaChangeKind::Added => theme.icon_table,
                    SchemaChangeKind::Removed => theme.danger,
                    SchemaChangeKind::Modified => theme.warning,
                };
                summary = summary.child(
                    div()
                        .font_family("Consolas")
                        .text_size(px(11.0))
                        .text_color(rgb(color))
                        .child(change.summary()),
                );
            }
            content = content.child(summary);
        }
        content.child(body).into_any_element()
    }

    /// The 数据比对 tab: action filters plus the changed rows.
    fn render_sync_data_tab(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(table) = self.sync.selected_table() else {
            return div().into_any_element();
        };
        if table.comparing {
            return sync_placeholder(t!("common.loading").to_string(), theme);
        }
        if let Some(error) = table.error.as_ref() {
            return sync_placeholder(error.clone(), theme);
        }
        if !table.has_key() {
            let weak = cx.weak_entity();
            let enabled = table.force_transfer;
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .max_w(px(460.0))
                        .p_6()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(rgb(theme.border))
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(div().text_size(px(14.0)).text_color(rgb(theme.text)).child(
                            if enabled {
                                t!("sync.force_transfer_title").to_string()
                            } else {
                                t!("sync.no_key_title").to_string()
                            },
                        ))
                        .child(div().text_align(gpui::TextAlign::Center).child(if enabled {
                            t!("sync.force_transfer_body").to_string()
                        } else {
                            t!("sync.no_key_body").to_string()
                        }))
                        .child(ui::button(
                            "sync-force-transfer",
                            if enabled {
                                t!("sync.force_transfer_cancel").to_string()
                            } else {
                                t!("sync.force_transfer").to_string()
                            },
                            if enabled {
                                ButtonKind::Normal
                            } else {
                                ButtonKind::Default
                            },
                            theme,
                            move |_event, _window, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    if let Some(index) = app.sync.selected
                                        && let Some(table) = app.sync.tables.get_mut(index)
                                    {
                                        table.force_transfer = !table.force_transfer;
                                    }
                                    app.sync.config_saved = false;
                                    cx.notify();
                                });
                            },
                        )),
                )
                .into_any_element();
        }
        if table.data.is_empty() {
            return sync_placeholder(t!("sync.same_body").to_string(), theme);
        }

        let counts = (
            table.data.inserts,
            table.data.updates,
            table.data.deletes,
            table.data.total(),
        );
        let filter = self.sync.data_filter;
        let rows = table.filtered_rows(filter);
        let mut pills = div().flex().flex_row().items_center().gap_1p5();
        for (value, label) in [
            (
                SyncDataFilter::All,
                t!("sync.filter.all", count = counts.3).to_string(),
            ),
            (
                SyncDataFilter::Insert,
                t!("sync.filter.insert", count = counts.0).to_string(),
            ),
            (
                SyncDataFilter::Update,
                t!("sync.filter.update", count = counts.1).to_string(),
            ),
            (
                SyncDataFilter::Delete,
                t!("sync.filter.delete", count = counts.2).to_string(),
            ),
        ] {
            let active = filter == value;
            let weak = cx.weak_entity();
            pills = pills.child(
                div()
                    .id(SharedString::from(format!("sync-filter-{label}")))
                    .flex()
                    .items_center()
                    .h(px(22.0))
                    .px_2()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(if active {
                        rgb(theme.text_muted)
                    } else {
                        rgb(theme.border)
                    })
                    .bg(rgb(if active {
                        theme.button_bg
                    } else {
                        theme.editor_bg
                    }))
                    .text_size(px(11.0))
                    .text_color(if active {
                        rgb(theme.text)
                    } else {
                        rgb(theme.text_muted)
                    })
                    .cursor_pointer()
                    .on_click(move |_event, _window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            app.sync.data_filter = value;
                            cx.notify();
                        });
                    })
                    .child(label),
            );
        }

        let keys = table.data.key_columns.join(", ");
        let mut toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .flex_none()
            .px_3()
            .py_2()
            .child(pills)
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("sync.pk_anchor", columns = keys).to_string()),
            );
        if table.data.truncated {
            toolbar = toolbar.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.warning))
                    .child(
                        svg()
                            .path("icons/warning.svg")
                            .w(px(12.0))
                            .h(px(12.0))
                            .flex_none()
                            .text_color(rgb(theme.warning)),
                    )
                    .child(t!("sync.truncated", rows = SYNC_MAX_ROWS).to_string()),
            );
        }

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(px(26.0))
            .px_3()
            .bg(rgb(theme.header_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .text_size(px(11.0))
            .text_color(rgb(theme.text_muted))
            .child(
                div()
                    .w(px(90.0))
                    .flex_none()
                    .child(t!("sync.col.action").to_string()),
            )
            .child(
                div()
                    .w(px(150.0))
                    .flex_none()
                    .child(t!("sync.col.pk").to_string()),
            )
            .child(div().flex_1().child(t!("sync.col.changes").to_string()));

        let mut body = div()
            .id("sync-data-rows")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.sync.inspector_scroll);
        for (position, change) in rows.iter().copied().take(SYNC_DISPLAY_MAX_ROWS).enumerate() {
            let background = match change.kind {
                RowChangeKind::Insert => (theme.icon_table << 8) | 0x12,
                RowChangeKind::Delete => (theme.danger << 8) | 0x12,
                RowChangeKind::Update => (theme.warning << 8) | 0x10,
            };
            let tone = match change.kind {
                RowChangeKind::Insert => PillTone::Good,
                RowChangeKind::Delete => PillTone::Bad,
                RowChangeKind::Update => PillTone::Accent,
            };
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .h(px(26.0))
                    .px_3()
                    .bg(rgba(background))
                    .when(position % 2 == 1, |style| style.opacity(0.92))
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text))
                    .child(div().w(px(90.0)).flex_none().child(sync_pill(
                        t!(change.kind.label_key()).as_ref(),
                        tone,
                        theme,
                    )))
                    .child(
                        div()
                            .w(px(150.0))
                            .flex_none()
                            .truncate()
                            .font_family("Consolas")
                            .text_size(px(11.0))
                            .child(render_row_keys(change)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .font_family("Consolas")
                            .text_size(px(11.0))
                            .child(render_row_summary(change)),
                    ),
            );
        }
        if rows.len() > SYNC_DISPLAY_MAX_ROWS {
            body = body.child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(
                        t!(
                            "sync.more_rows",
                            count = rows.len().saturating_sub(SYNC_DISPLAY_MAX_ROWS)
                        )
                        .to_string(),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(toolbar)
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// The run log panel under the page.
    fn render_sync_log(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let percent = if self.sync.running {
            (self.sync.progress * 100.0).round() as u32
        } else if self.sync.started.is_some() {
            100
        } else {
            0
        };
        div()
            .flex()
            .flex_col()
            .h(px(SYNC_LOG_HEIGHT))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_1()
                    .flex_none()
                    .child(div().text_size(px(12.0)).text_color(rgb(theme.text)).child(
                        if self.sync.running {
                            t!("sync.running").to_string()
                        } else {
                            t!("sync.log.title").to_string()
                        },
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(format!("{percent}%")),
                            )
                            .when(self.sync.running, |row| {
                                row.child(ui::button(
                                    "sync-cancel",
                                    t!("sync.cancel").to_string(),
                                    ButtonKind::Normal,
                                    theme,
                                    cx.listener(|this, _event, _window, cx| this.cancel_sync(cx)),
                                ))
                            })
                            .when(!self.sync.running, |row| {
                                row.child(ui::button(
                                    "sync-log-close",
                                    t!("sync.close").to_string(),
                                    ButtonKind::Normal,
                                    theme,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.sync.log_open = false;
                                        cx.notify();
                                    }),
                                ))
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .mx_3()
                    .h(px(6.0))
                    .rounded_full()
                    .bg(rgb(theme.button_hover_bg))
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(rgb(theme.primary))
                            .w(relative(percent as f32 / 100.0)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .mt_1()
                    .child(render_log_lines(
                        &self.sync.log,
                        &self.sync.log_scroll,
                        &self.sync.log_handle,
                        theme,
                        false,
                    )),
            )
    }

    /// The 选项 popover (backdrop + panel), anchored under its toolbar button. It is added to the
    /// window root (like the connection menu), so the recorded window-space anchor is exact.
    pub(super) fn render_sync_options(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let anchor = *self.sync.options_anchor.borrow();
        let left = (f32::from(anchor.x) - SYNC_POPUP_WIDTH + 8.0).max(8.0);
        let top = f32::from(anchor.y) + 2.0;

        let backdrop = deferred(
            div()
                .id("sync-options-backdrop")
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.sync.options_open = false;
                        cx.notify();
                    }),
                ),
        )
        .with_priority(SYNC_BACKDROP_PRIORITY);

        let mut panel = ui::popup_panel(theme)
            .left(px(left))
            .top(px(top))
            .w(px(SYNC_POPUP_WIDTH))
            .p_3()
            .gap_2()
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme.text_muted))
                    .child(t!("sync.options.title").to_string()),
            );
        for (id, label, value) in [
            (
                "sync-opt-delete",
                t!("sync.options.include_delete").to_string(),
                self.sync.include_delete,
            ),
            (
                "sync-opt-existing",
                t!("sync.options.existing_only").to_string(),
                self.sync.existing_only,
            ),
        ] {
            let weak = cx.weak_entity();
            panel = panel.child(
                div()
                    .id(id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text))
                    .on_click(move |_event, _window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            if id == "sync-opt-delete" {
                                app.sync.include_delete = !app.sync.include_delete;
                            } else {
                                app.sync.existing_only = !app.sync.existing_only;
                            }
                            app.sync.config_saved = false;
                            cx.notify();
                        });
                    })
                    .child(div().min_w_0().child(label))
                    .child(ui::checkbox_box(value, theme)),
            );
        }
        panel = panel.child(
            div()
                .pt_1()
                .border_t_1()
                .border_color(rgb(theme.border))
                .text_size(px(11.0))
                .text_color(rgb(theme.text_muted))
                .child(t!("sync.options.limit", rows = SYNC_MAX_ROWS).to_string()),
        );
        let popup = deferred(panel).with_priority(SYNC_POPUP_PRIORITY);
        div()
            .absolute()
            .inset_0()
            .child(backdrop)
            .child(popup)
            .into_any_element()
    }
}

/// The localized label of a mode, for the saved-configuration list's 模式 column.
fn sync_mode_label(mode: SyncMode) -> String {
    t!(mode.label_key()).to_string()
}

/// The child window that hosts the 传输 workbench. It re-renders whenever `AppView` changes, so the
/// comparison, run progress and log stay live while the work happens in the background.
pub(super) struct TransferWindow {
    app: WeakEntity<AppView>,
    _subscription: Subscription,
}

impl TransferWindow {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        app_entity: &Entity<AppView>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let subscription = cx.observe(app_entity, |_, _, cx| cx.notify());
        Self {
            app,
            _subscription: subscription,
        }
    }
}

impl Render for TransferWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        app.update(cx, |app, cx| app.sync_window_contents(cx))
    }
}

/// Build the configuration-name field of the transfer window's footer.
fn make_transfer_name_input(
    theme: Theme,
    initial: String,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    let placeholder: SharedString = t!("sync.config.name_placeholder").to_string().into();
    cx.new(move |cx| {
        TextInput::new(
            theme,
            initial,
            TextInputOptions {
                placeholder,
                ..Default::default()
            },
            cx,
        )
        .on_change(Rc::new(move |text, _window, cx| {
            let _ = change.update(cx, |app, cx| {
                app.sync.config_name = text.to_string();
                app.sync.config_saved = false;
                app.sync.config_error = None;
                cx.notify();
            });
        }))
    })
}

/// List a database's tables through the runtime.
async fn list_tables(
    runtime: Arc<Runtime>,
    connection: Arc<dyn Connection>,
    database: String,
) -> Result<Vec<rustgrid_core::TableInfo>, String> {
    match runtime
        .spawn(async move { connection.list_tables(&database).await })
        .await
    {
        Ok(Ok(tables)) => Ok(tables),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// Compare one table: its definitions, DDL and rows. Every database call goes through the runtime.
async fn compare_table(
    runtime: Arc<Runtime>,
    source: Arc<dyn Connection>,
    source_database: String,
    target: Arc<dyn Connection>,
    target_database: String,
    name: String,
    in_target: bool,
) -> ComparedTable {
    let mut compared = ComparedTable {
        source_schema: None,
        target_schema: None,
        schema_changes: Vec::new(),
        source_ddl: None,
        target_ddl: None,
        data: DataDiff::default(),
        error: None,
    };
    let source_columns = match load_columns(
        runtime.clone(),
        source.clone(),
        source_database.clone(),
        name.clone(),
    )
    .await
    {
        Ok(columns) => columns,
        Err(error) => {
            compared.error = Some(error);
            return compared;
        }
    };
    compared.source_schema = load_schema(
        runtime.clone(),
        source.clone(),
        source_database.clone(),
        name.clone(),
    )
    .await
    .ok()
    .flatten();
    compared.source_ddl = load_ddl(
        runtime.clone(),
        source.clone(),
        source_database.clone(),
        name.clone(),
    )
    .await
    .ok()
    .flatten();
    let target_columns: Vec<ColumnInfo> = if in_target {
        compared.target_schema = load_schema(
            runtime.clone(),
            target.clone(),
            target_database.clone(),
            name.clone(),
        )
        .await
        .ok()
        .flatten();
        compared.target_ddl = load_ddl(
            runtime.clone(),
            target.clone(),
            target_database.clone(),
            name.clone(),
        )
        .await
        .ok()
        .flatten();
        load_columns(
            runtime.clone(),
            target.clone(),
            target_database.clone(),
            name.clone(),
        )
        .await
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    compared.schema_changes = match (&compared.source_schema, &compared.target_schema) {
        (Some(source_schema), Some(target_schema)) => {
            diff_columns(&source_schema.columns, &target_schema.columns)
        }
        // A table that does not exist yet is all-new. When the target has the table but its
        // definition could not be read, report nothing rather than calling every column new.
        (Some(source_schema), None) if !in_target => diff_columns(&source_schema.columns, &[]),
        _ => Vec::new(),
    };

    let keys: Vec<String> = source_columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.clone())
        .collect();
    let order_by: Vec<SortColumn> = keys
        .iter()
        .map(|column| SortColumn {
            column: column.clone(),
            descending: false,
        })
        .collect();
    let (source_rows, mut truncated) = match fetch_all_rows(
        runtime.clone(),
        source,
        source_database,
        name.clone(),
        order_by.clone(),
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            compared.error = Some(error);
            return compared;
        }
    };
    let (target_rows, target_truncated) = if in_target && !keys.is_empty() {
        match fetch_all_rows(runtime, target, target_database, name, order_by).await {
            Ok(result) => result,
            Err(error) => {
                compared.error = Some(error);
                return compared;
            }
        }
    } else {
        (Vec::new(), false)
    };
    truncated |= target_truncated;
    compared.data = diff_rows(
        &source_columns,
        &source_rows,
        &target_columns,
        &target_rows,
        &keys,
        truncated,
    );
    compared
}

async fn load_columns(
    runtime: Arc<Runtime>,
    connection: Arc<dyn Connection>,
    database: String,
    table: String,
) -> Result<Vec<ColumnInfo>, String> {
    match runtime
        .spawn(async move { connection.columns(&database, &table).await })
        .await
    {
        Ok(Ok(columns)) => Ok(columns),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

async fn load_schema(
    runtime: Arc<Runtime>,
    connection: Arc<dyn Connection>,
    database: String,
    table: String,
) -> Result<Option<TableSchema>, String> {
    match runtime
        .spawn(async move { connection.table_schema(&database, &table).await })
        .await
    {
        Ok(Ok(schema)) => Ok(Some(schema)),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

async fn load_ddl(
    runtime: Arc<Runtime>,
    connection: Arc<dyn Connection>,
    database: String,
    table: String,
) -> Result<Option<String>, String> {
    match runtime
        .spawn(async move { connection.object_ddl(&database, &table, false).await })
        .await
    {
        Ok(Ok(ddl)) => Ok(ddl),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// Read every row of a table up to [`SYNC_MAX_ROWS`], returning the rows and whether the cap was
/// hit. `order_by` is the primary key, so pages stay coherent.
async fn fetch_all_rows(
    runtime: Arc<Runtime>,
    connection: Arc<dyn Connection>,
    database: String,
    table: String,
    order_by: Vec<SortColumn>,
) -> Result<(Vec<Vec<CellValue>>, bool), String> {
    let mut rows = Vec::new();
    let mut page = 0u64;
    loop {
        let request = PageRequest::new(page, SYNC_PAGE_SIZE).with_order_by(order_by.clone());
        let connection_call = connection.clone();
        let database_call = database.clone();
        let table_call = table.clone();
        let fetched = match runtime
            .spawn(async move {
                connection_call
                    .fetch_page(&database_call, &table_call, request)
                    .await
            })
            .await
        {
            Ok(Ok(page)) => page,
            Ok(Err(error)) => return Err(error.to_string()),
            Err(error) => return Err(error.to_string()),
        };
        let count = fetched.rows.len();
        rows.extend(fetched.rows);
        if count < SYNC_PAGE_SIZE as usize {
            return Ok((rows, false));
        }
        if rows.len() as u64 >= SYNC_MAX_ROWS {
            rows.truncate(SYNC_MAX_ROWS as usize);
            return Ok((rows, true));
        }
        page += 1;
    }
}

/// Apply one table's plan and return a one-line summary for the log.
#[allow(clippy::too_many_arguments)]
async fn run_sync_table(
    runtime: Arc<Runtime>,
    source: Arc<dyn Connection>,
    source_database: String,
    target: Arc<dyn Connection>,
    target_database: String,
    plan: &SyncPlan,
    mode: SyncMode,
    same_engine: bool,
    include_delete: bool,
    cancel: &Arc<AtomicBool>,
) -> Result<String, String> {
    let name = plan.name.clone();
    let mut created = false;

    // ----- Structure -------------------------------------------------------------------------
    // A cross-engine pair can still transfer rows, but the source's DDL is not valid on the
    // target, so the structure stage (including Transfer's DROP + CREATE) is skipped.
    if mode.uses_schema() && same_engine {
        if !same_engine {
            return Err(t!("sync.error.cross_engine_schema").to_string());
        }
        let Some(source_schema) = plan.source_schema.clone() else {
            return Err(t!("sync.error.no_schema").to_string());
        };
        if mode == SyncMode::Transfer && plan.in_target {
            let target_connection = target.clone();
            let database = target_database.clone();
            let table = name.clone();
            match runtime
                .spawn(async move { target_connection.drop_table(&database, &table).await })
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error.to_string()),
                Err(error) => return Err(error.to_string()),
            }
        }
        let recreate = mode == SyncMode::Transfer || !plan.in_target;
        if recreate {
            let target_connection = target.clone();
            let database = target_database.clone();
            let table = name.clone();
            match runtime
                .spawn(async move {
                    target_connection
                        .save_table_schema(&database, &table, None, &source_schema)
                        .await
                })
                .await
            {
                Ok(Ok(())) => created = true,
                Ok(Err(error)) => return Err(error.to_string()),
                Err(error) => return Err(error.to_string()),
            }
        } else {
            let target_schema = plan.target_schema.clone();
            let target_connection = target.clone();
            let database = target_database.clone();
            let table = name.clone();
            match runtime
                .spawn(async move {
                    target_connection
                        .save_table_schema(
                            &database,
                            &table,
                            target_schema.as_ref(),
                            &source_schema,
                        )
                        .await
                })
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error.to_string()),
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    if !mode.uses_data() {
        return Ok(t!(
            "sync.log.table_schema",
            name = name,
            count = plan.schema_changes.len()
        )
        .to_string());
    }

    // ----- Data ------------------------------------------------------------------------------
    // A table that did not exist was just created empty, so every source row is copied; an
    // existing one is emptied first.
    let full_copy = mode == SyncMode::Transfer || plan.force_transfer || !plan.in_target;
    if full_copy {
        if !plan.in_target && !created {
            return Err(t!("sync.error.missing_target").to_string());
        }
        if !created {
            let target_connection = target.clone();
            let database = target_database.clone();
            let table = name.clone();
            match runtime
                .spawn(async move { target_connection.empty_table(&database, &table).await })
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error.to_string()),
                Err(error) => return Err(error.to_string()),
            }
        }
        let target_columns = load_columns(
            runtime.clone(),
            target.clone(),
            target_database.clone(),
            name.clone(),
        )
        .await
        .unwrap_or_default();
        let source_columns = load_columns(
            runtime.clone(),
            source.clone(),
            source_database.clone(),
            name.clone(),
        )
        .await
        .unwrap_or_default();
        let keys: Vec<String> = source_columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| column.name.clone())
            .collect();
        let order_by: Vec<SortColumn> = keys
            .iter()
            .map(|column| SortColumn {
                column: column.clone(),
                descending: false,
            })
            .collect();
        let mut page = 0u64;
        let mut copied = 0usize;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(t!("sync.log.cancelled").to_string());
            }
            let request = PageRequest::new(page, SYNC_PAGE_SIZE).with_order_by(order_by.clone());
            let source_connection = source.clone();
            let database = source_database.clone();
            let table = name.clone();
            let fetched = match runtime
                .spawn(async move {
                    source_connection
                        .fetch_page(&database, &table, request)
                        .await
                })
                .await
            {
                Ok(Ok(page)) => page,
                Ok(Err(error)) => return Err(error.to_string()),
                Err(error) => return Err(error.to_string()),
            };
            let count = fetched.rows.len();
            if count == 0 {
                break;
            }
            let rows: Vec<RowInsert> = fetched
                .rows
                .iter()
                .map(|row| RowInsert {
                    values: fetched
                        .columns
                        .iter()
                        .enumerate()
                        .filter(|(_, column)| {
                            target_columns
                                .iter()
                                .any(|target| target.name == column.name)
                        })
                        .filter_map(|(index, column)| {
                            let value = row.get(index)?.as_edit_value();
                            Some((column.name.clone(), value))
                        })
                        .collect(),
                })
                .collect();
            // A table whose columns share no name with the target cannot be written row by row;
            // report that instead of sending an empty `VALUES ()`.
            if rows.iter().all(|row| row.values.is_empty()) {
                return Err(t!("sync.error.no_shared_columns").to_string());
            }
            copied += rows.len();
            let target_connection = target.clone();
            let database = target_database.clone();
            let table = name.clone();
            match runtime
                .spawn(async move {
                    target_connection
                        .insert_rows(&database, &table, &rows)
                        .await
                })
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error.to_string()),
                Err(error) => return Err(error.to_string()),
            }
            if count < SYNC_PAGE_SIZE as usize {
                break;
            }
            page += 1;
        }
        return Ok(t!("sync.log.table_transfer", name = name, rows = copied).to_string());
    }

    // Incremental data.
    if !plan.has_key {
        return Ok(t!("sync.log.table_no_key", name = name).to_string());
    }
    if plan.data.is_empty() {
        return Ok(t!("sync.log.table_same", name = name).to_string());
    }
    let mut inserts = 0usize;
    let inserts_plan: Vec<RowInsert> = plan
        .data
        .rows
        .iter()
        .filter(|change| change.kind == RowChangeKind::Insert)
        .map(|change| RowInsert {
            values: change.values.clone(),
        })
        .collect();
    for chunk in inserts_plan.chunks(SYNC_WRITE_BATCH) {
        if cancel.load(Ordering::Relaxed) {
            return Err(t!("sync.log.cancelled").to_string());
        }
        let rows = chunk.to_vec();
        let count = rows.len();
        let target_connection = target.clone();
        let database = target_database.clone();
        let table = name.clone();
        match runtime
            .spawn(async move {
                target_connection
                    .insert_rows(&database, &table, &rows)
                    .await
            })
            .await
        {
            Ok(Ok(())) => inserts += count,
            Ok(Err(error)) => return Err(error.to_string()),
            Err(error) => return Err(error.to_string()),
        }
    }
    let mut updates = 0usize;
    let updates_plan: Vec<RowUpdate> = plan
        .data
        .rows
        .iter()
        .filter(|change| change.kind == RowChangeKind::Update)
        .map(|change| RowUpdate {
            set: change.values.clone(),
            keys: change.keys.clone(),
        })
        .collect();
    for chunk in updates_plan.chunks(SYNC_WRITE_BATCH) {
        if cancel.load(Ordering::Relaxed) {
            return Err(t!("sync.log.cancelled").to_string());
        }
        let updates_batch = chunk.to_vec();
        let count = updates_batch.len();
        let target_connection = target.clone();
        let database = target_database.clone();
        let table = name.clone();
        match runtime
            .spawn(async move {
                target_connection
                    .update_rows(&database, &table, &updates_batch)
                    .await
            })
            .await
        {
            Ok(Ok(())) => updates += count,
            Ok(Err(error)) => return Err(error.to_string()),
            Err(error) => return Err(error.to_string()),
        }
    }
    let mut deletes = 0usize;
    if include_delete {
        let delete_plan: Vec<Vec<(String, Option<String>)>> = plan
            .data
            .rows
            .iter()
            .filter(|change| change.kind == RowChangeKind::Delete)
            .map(|change| change.keys.clone())
            .collect();
        for chunk in delete_plan.chunks(SYNC_WRITE_BATCH) {
            if cancel.load(Ordering::Relaxed) {
                return Err(t!("sync.log.cancelled").to_string());
            }
            let keys = chunk.to_vec();
            let count = keys.len();
            let target_connection = target.clone();
            let database = target_database.clone();
            let table = name.clone();
            match runtime
                .spawn(async move {
                    target_connection
                        .delete_rows(&database, &table, &keys)
                        .await
                })
                .await
            {
                Ok(Ok(())) => deletes += count,
                Ok(Err(error)) => return Err(error.to_string()),
                Err(error) => return Err(error.to_string()),
            }
        }
    }
    Ok(t!(
        "sync.log.table_data",
        name = name,
        inserts = inserts,
        updates = updates,
        deletes = deletes
    )
    .to_string())
}

/// How one DDL line compares with the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineMark {
    Same,
    Add,
    Del,
}

/// One DDL line and how it compares with the other side.
type MarkedLines = Vec<(String, LineMark)>;

/// Mark the source and target DDL line by line. A line that appears on both sides (counting
/// duplicates) is unchanged; a source-only line is an addition and a target-only line a removal.
fn ddl_line_marks(source: &str, target: &str) -> (MarkedLines, MarkedLines) {
    let source_lines: Vec<String> = source.lines().map(str::to_string).collect();
    let target_lines: Vec<String> = target.lines().map(str::to_string).collect();
    let mut source_marks = Vec::with_capacity(source_lines.len());
    let mut target_marks = Vec::with_capacity(target_lines.len());

    let mut target_counts: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for line in &target_lines {
        *target_counts.entry(line.trim()).or_default() += 1;
    }
    for line in &source_lines {
        let key = line.trim();
        match target_counts.get_mut(key) {
            Some(count) if *count > 0 => {
                *count -= 1;
                source_marks.push((line.clone(), LineMark::Same));
            }
            _ => source_marks.push((line.clone(), LineMark::Add)),
        }
    }
    let mut source_counts: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for line in &source_lines {
        *source_counts.entry(line.trim()).or_default() += 1;
    }
    for line in &target_lines {
        let key = line.trim();
        match source_counts.get_mut(key) {
            Some(count) if *count > 0 => {
                *count -= 1;
                target_marks.push((line.clone(), LineMark::Same));
            }
            _ => target_marks.push((line.clone(), LineMark::Del)),
        }
    }
    (source_marks, target_marks)
}

/// A status pill: a tinted rounded label.
fn sync_pill(text: &str, tone: PillTone, theme: Theme) -> AnyElement {
    let color = tone.color(theme);
    div()
        .flex_none()
        .h(px(18.0))
        .flex()
        .items_center()
        .px_1p5()
        .rounded(px(4.0))
        .bg(rgba((color << 8) | 0x1a))
        .text_size(px(10.5))
        .text_color(rgb(color))
        .child(text.to_string())
        .into_any_element()
}

/// A centred placeholder message filling the inspector body.
fn sync_placeholder(message: String, theme: Theme) -> AnyElement {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .p_6()
        .text_size(px(12.0))
        .text_color(rgb(theme.text_muted))
        .child(message)
        .into_any_element()
}

/// The key columns of a changed row as `id=7, tenant=2`.
fn render_row_keys(change: &RowChange) -> String {
    change
        .keys
        .iter()
        .map(|(column, value)| match value {
            Some(value) => format!("{column}={value}"),
            None => format!("{column}=NULL"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A one-line description of what an insert/update carries.
fn render_row_summary(change: &RowChange) -> String {
    match change.kind {
        RowChangeKind::Insert => change
            .values
            .iter()
            .take(4)
            .map(|(column, value)| match value {
                Some(value) => format!("{column}={value}"),
                None => format!("{column}=NULL"),
            })
            .collect::<Vec<_>>()
            .join(", "),
        RowChangeKind::Update => change
            .changes
            .iter()
            .map(|cell| {
                let before = cell.before.clone().unwrap_or_else(|| "NULL".to_string());
                let after = cell.after.clone().unwrap_or_else(|| "NULL".to_string());
                format!("{}: {before} → {after}", cell.column)
            })
            .collect::<Vec<_>>()
            .join("; "),
        RowChangeKind::Delete => "--".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_marks_lines_present_on_only_one_side() {
        let source = "CREATE TABLE `users` (\n  `id` bigint,\n  `mfa` varchar(64)\n);";
        let target = "CREATE TABLE `users` (\n  `id` bigint\n);";
        let (source_lines, target_lines) = ddl_line_marks(source, target);
        assert_eq!(source_lines[0].1, LineMark::Same);
        assert_eq!(source_lines[1].1, LineMark::Add);
        assert_eq!(source_lines[2].1, LineMark::Add);
        assert_eq!(source_lines[3].1, LineMark::Same);
        // The target's `id` line has no trailing comma, so it reads as changed on that side too.
        assert_eq!(target_lines[0].1, LineMark::Same);
        assert_eq!(target_lines[1].1, LineMark::Del);
        assert_eq!(target_lines[2].1, LineMark::Same);
    }

    #[test]
    fn ddl_marks_duplicate_lines_by_count() {
        let (source_lines, target_lines) = ddl_line_marks("a\nb", "a\na");
        assert_eq!(source_lines[1].1, LineMark::Add);
        assert_eq!(target_lines[1].1, LineMark::Del);
    }

    #[test]
    fn row_keys_and_summary_read_naturally() {
        let change = RowChange {
            kind: RowChangeKind::Update,
            keys: vec![("id".to_string(), Some("7".to_string()))],
            values: vec![("role".to_string(), Some("admin".to_string()))],
            changes: vec![rustgrid_core::CellChange {
                column: "role".to_string(),
                before: Some("member".to_string()),
                after: Some("admin".to_string()),
            }],
        };
        assert_eq!(render_row_keys(&change), "id=7");
        assert_eq!(render_row_summary(&change), "role: member → admin");
    }
}
