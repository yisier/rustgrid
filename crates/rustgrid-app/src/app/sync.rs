//! The 数据传输/同步 main tab: compare a source database with a target database and apply the
//! difference (structure, data, or both).
//!
//! The page follows Navicat's 数据传输/同步 workbench: a source → target topology bar, one
//! segmented control for the four modes (全部 / 仅结构 / 仅数据 / 全量传输), a table list with a
//! per-table status pill, and an inspector with 结构比对 / 数据比对 / SQL 预览 tabs. Schema-aware
//! engines (PostgreSQL, SQL Server, Oracle) also pick a source and target schema, so a cross-engine
//! transfer maps `public.users` onto the target's `dbo.users` instead of a literal `public` schema.
//! Comparisons run
//! through [`rustgrid_core::Connection`] (so every driver works) and the engine-agnostic diff lives
//! in [`rustgrid_core::sync`]. Applying a plan reuses the drivers' own write paths
//! (`save_table_schema`, `insert_rows`, `update_rows`, `delete_rows`), never raw SQL written here,
//! so this module stays engine-independent.
//!
//! Every database call is awaited through [`Runtime::spawn`]: gpui's executor is not Tokio, so
//! awaiting a driver future directly panics with "there is no reactor running".

use std::cmp::Ordering as CmpOrdering;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rustgrid_core::{
    CellValue, ColumnInfo, DataDiff, ObjectKind, PageRequest, RowChange, RowChangeKind,
    SchemaChange, SortColumn, TableSchema, TransferMode, delete_row_change, diff_rows, diff_schema,
    insert_row_change, map_schema, map_table_name, render_change_sql, update_row_change,
    usable_key_columns,
};

use super::backup::{format_hms, now_timestamp};
use super::*;
use gpui_kit::base::SelectableText;

/// Width of the table list column.
const SYNC_LIST_WIDTH: f32 = 296.0;
/// Height of one table row.
const SYNC_ROW_HEIGHT: f32 = 26.0;
/// Rows fetched per page while comparing or copying.
const SYNC_PAGE_SIZE: u64 = 500;
/// Rows fetched per page by the streaming merge comparator. Larger pages keep the number of
/// `COUNT`-free round trips down without holding a whole table in memory.
const SYNC_STREAM_PAGE: u64 = 2_000;
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

    /// The hover tooltip of the mode switch. The tab labels stay short in every language (a long
    /// translation is what pushed the run controls off the toolbar), so the full meaning of each
    /// mode lives here.
    fn hint_key(self) -> &'static str {
        match self {
            SyncMode::All => "sync.mode.all.hint",
            SyncMode::Schema => "sync.mode.schema.hint",
            SyncMode::Data => "sync.mode.data.hint",
            SyncMode::Transfer => "sync.mode.transfer.hint",
        }
    }

    /// The inspector tab a newly chosen mode opens on.
    fn default_sub_tab(self) -> SyncSubTab {
        match self {
            SyncMode::Schema | SyncMode::All => SyncSubTab::Schema,
            SyncMode::Data => SyncSubTab::Data,
            SyncMode::Transfer => SyncSubTab::Sql,
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
    /// The target's own name for this table (schema-qualified on PostgreSQL/SQL Server/Oracle,
    /// bare on MySQL/SQLite).
    pub target_name: String,
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
    /// The comparison was stopped before it finished.
    pub stopped: bool,
    /// The table was written by this session, so its diff is empty.
    pub synced: bool,
    /// Per-table 全量覆写, offered for a table without a primary key.
    pub force_transfer: bool,
    pub error: Option<String>,
}

impl SyncTable {
    fn new(name: String, target_name: String, in_target: bool) -> Self {
        Self {
            name,
            target_name,
            checked: true,
            in_target,
            source_schema: None,
            target_schema: None,
            schema_changes: Vec::new(),
            source_ddl: None,
            target_ddl: None,
            data: DataDiff::default(),
            comparing: true,
            stopped: false,
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
        if self.error.is_some() || self.comparing || self.stopped {
            return false;
        }
        let schema = !self.in_target || !self.schema_changes.is_empty();
        // 仅数据 writes into an existing target table only. Creating a missing table belongs to
        // 全部 / 仅结构 / 全量传输, so a data-only run never changes the target's structure.
        let data = if self.has_key() {
            self.in_target && !self.data.is_empty()
        } else {
            self.force_transfer && self.in_target
        };
        match mode {
            // A missing table can be created across dialects from the mapped source definition;
            // an existing one is only altered within one dialect.
            SyncMode::Schema => schema && (same_engine || !self.in_target),
            SyncMode::Data => data,
            SyncMode::All => data || (schema && (same_engine || !self.in_target)),
            SyncMode::Transfer => self.in_target || self.source_schema.is_some(),
        }
    }

    /// The tables a data-only run can actually write: the target table must already exist.
    fn data_target_exists(&self, mode: SyncMode) -> bool {
        mode != SyncMode::Data || self.in_target
    }

    /// Whether the current mode finds nothing to do for this table, so 隐藏一致 can drop its row.
    /// A table without a primary key is never "identical" for a data mode: its rows were not
    /// compared at all.
    fn is_identical(&self, mode: SyncMode, same_engine: bool) -> bool {
        if self.comparing || self.stopped || self.error.is_some() {
            return false;
        }
        // A definition that could not be read is not proof of equality.
        if self.source_schema.is_none() || (self.in_target && self.target_schema.is_none()) {
            return false;
        }
        if mode.uses_data() && !self.has_key() {
            return false;
        }
        // A table the target does not have is not "equal" under 仅数据; it is simply skipped.
        if !self.data_target_exists(mode) {
            return false;
        }
        !self.has_changes(mode, same_engine)
    }

    /// The status pill's `(text, tone)` for the current mode.
    fn status(&self, mode: SyncMode) -> (String, PillTone) {
        if self.error.is_some() {
            return (t!("sync.status.error").to_string(), PillTone::Bad);
        }
        if self.comparing {
            return (t!("sync.status.comparing").to_string(), PillTone::Neutral);
        }
        if self.stopped {
            return (t!("sync.status.stopped").to_string(), PillTone::Neutral);
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
        if mode == SyncMode::Data && !self.in_target {
            return (t!("sync.status.missing").to_string(), PillTone::Bad);
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
    /// The selected source schema (`public`, `dbo`, ...). Empty for a schema-less engine.
    pub source_schema: String,
    pub target_connection: Option<usize>,
    pub target_database: String,
    /// The selected target schema. Empty for a schema-less engine.
    pub target_schema: String,
    /// The schemas the source/target connection offers for the selected database, as returned by
    /// `list_schemas` (or derived from the table list when the driver does not report them).
    pub source_schemas: Vec<String>,
    pub target_schemas: Vec<String>,
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
    /// The table list's vertical scrollbar/drag state.
    pub list_detail: DetailScroll,
    pub inspector_scroll: ScrollHandle,
    pub source_combo: Option<Entity<ComboBox>>,
    pub source_database_combo: Option<Entity<ComboBox>>,
    pub source_schema_combo: Option<Entity<ComboBox>>,
    pub target_combo: Option<Entity<ComboBox>>,
    pub target_database_combo: Option<Entity<ComboBox>>,
    pub target_schema_combo: Option<Entity<ComboBox>>,
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
    /// Hide the table rows whose comparison found nothing to sync (persisted in `settings.json`).
    pub hide_same: bool,
    /// Waiting for the user to confirm closing a window with an unsaved configuration.
    pub close_confirm: bool,
    /// The user confirmed the close, so the window's should-close guard may let it go.
    pub close_allowed: bool,
    /// Waiting for the user to confirm the 同步 action before writing to the target.
    pub run_confirm: bool,
    /// The saved table selection to re-apply when the comparison installs its table list.
    pub pending_tables: Option<HashSet<String>>,
    /// The table to select again once a comparison installs its table list (the full re-compare).
    pub pending_selected: Option<String>,
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
            source_schema: String::new(),
            target_connection: None,
            target_database: String::new(),
            target_schema: String::new(),
            source_schemas: Vec::new(),
            target_schemas: Vec::new(),
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
            list_detail: DetailScroll::default(),
            inspector_scroll: ScrollHandle::new(),
            source_combo: None,
            source_database_combo: None,
            source_schema_combo: None,
            target_combo: None,
            target_database_combo: None,
            target_schema_combo: None,
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
            hide_same: true,
            close_confirm: false,
            close_allowed: false,
            run_confirm: false,
            pending_tables: None,
            pending_selected: None,
            generation: 0,
        }
    }

    /// The selected table, if the selection is still valid.
    fn selected_table(&self) -> Option<&SyncTable> {
        self.selected.and_then(|index| self.tables.get(index))
    }

    /// The table indices the list shows: the search filter plus 隐藏一致. The inspector selection
    /// is reconciled separately by [`SyncState::keep_selection_visible`].
    fn visible_indices(&self, mode: SyncMode, same_engine: bool, hide_same: bool) -> Vec<usize> {
        let needle = self.search.trim().to_lowercase();
        self.tables
            .iter()
            .enumerate()
            .filter(|(_index, table)| {
                if !needle.is_empty() && !table.name.to_lowercase().contains(&needle) {
                    return false;
                }
                if !hide_same {
                    return true;
                }
                !table.is_identical(mode, same_engine)
            })
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

    /// Move the inspector off a row that 隐藏一致 just removed, so the list and the detail pane
    /// never point at different tables. With nothing left to show the selection becomes `None`.
    fn keep_selection_visible(&mut self, same_engine: bool) {
        if !self.hide_same {
            return;
        }
        let visible = self.visible_indices(self.mode, same_engine, true);
        if self
            .selected
            .is_some_and(|selected| visible.contains(&selected))
        {
            return;
        }
        self.selected = visible.first().copied();
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
#[derive(Clone)]
struct SyncPlan {
    name: String,
    /// The name the target engine should use (a schema-less target drops the source schema).
    target_name: String,
    in_target: bool,
    has_key: bool,
    source_schema: Option<TableSchema>,
    target_schema: Option<TableSchema>,
    schema_changes: Vec<SchemaChange>,
    data: DataDiff,
    force_transfer: bool,
    /// Columns the target must generate itself: computed/generated columns on every engine, and
    /// Oracle's `GENERATED ALWAYS AS IDENTITY` columns (Oracle cannot override them).
    skip_write: Vec<String>,
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
        if self.sync.source_schema_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 200.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ =
                            weak.update(cx, |app, cx| app.sync_source_schema_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/layers.svg", theme.icon_database, cx);
            });
            self.sync.source_schema_combo = Some(combo);
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
        if self.sync.target_schema_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 200.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ =
                            weak.update(cx, |app, cx| app.sync_target_schema_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/layers.svg", theme.icon_database, cx);
            });
            self.sync.target_schema_combo = Some(combo);
        }
    }

    /// Push the connection/database lists and the current selection into the dropdowns.
    pub(super) fn sync_sync_combos(&mut self, cx: &mut Context<'_, Self>) {
        let connections: Vec<ComboOption> = self
            .connections
            .iter()
            .enumerate()
            .map(|(index, node)| {
                // A leading engine mark tells the MySQL connection from the PostgreSQL one at a
                // glance, both in the trigger and in the dropdown.
                let (icon, color) = self.sync_engine_icon(Some(index));
                ComboOption::new(index.to_string(), node.profile.name.clone())
                    .with_icon(icon, color)
            })
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
        let mut source_schemas: Vec<ComboOption> = self
            .sync
            .source_schemas
            .iter()
            .cloned()
            .map(|schema| ComboOption::new(schema.clone(), schema))
            .collect();
        let mut target_schemas: Vec<ComboOption> = self
            .sync
            .target_schemas
            .iter()
            .cloned()
            .map(|schema| ComboOption::new(schema.clone(), schema))
            .collect();
        // An empty database still has an engine default schema; keep it in the list so the
        // trigger names the schema the run will write into.
        if !self.sync.source_schema.is_empty()
            && !source_schemas
                .iter()
                .any(|option| option.value.eq_ignore_ascii_case(&self.sync.source_schema))
        {
            source_schemas.push(ComboOption::new(
                self.sync.source_schema.clone(),
                self.sync.source_schema.clone(),
            ));
        }
        if !self.sync.target_schema.is_empty()
            && !target_schemas
                .iter()
                .any(|option| option.value.eq_ignore_ascii_case(&self.sync.target_schema))
        {
            target_schemas.push(ComboOption::new(
                self.sync.target_schema.clone(),
                self.sync.target_schema.clone(),
            ));
        }
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
        let schema_placeholder = t!("sync.schema").to_string();
        let (source_icon, source_icon_color) = self.sync_engine_icon(self.sync.source_connection);
        let (target_icon, target_icon_color) = self.sync_engine_icon(self.sync.target_connection);
        if let Some(combo) = self.sync.source_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(connections.clone(), cx);
                combo.set_selected(source_connection, cx);
                combo.set_placeholder(placeholder.clone(), cx);
                combo.set_icon(source_icon, source_icon_color, cx);
            });
        }
        if let Some(combo) = self.sync.target_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(connections, cx);
                combo.set_selected(target_connection, cx);
                combo.set_placeholder(placeholder, cx);
                combo.set_icon(target_icon, target_icon_color, cx);
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
        if let Some(combo) = self.sync.source_schema_combo.clone() {
            let enabled = self.sync.source_connection.is_some() && !source_schemas.is_empty();
            combo.update(cx, |combo, cx| {
                combo.set_options(source_schemas, cx);
                combo.set_selected(self.sync.source_schema.clone(), cx);
                combo.set_placeholder(schema_placeholder.clone(), cx);
                combo.set_enabled(enabled, cx);
            });
        }
        if let Some(combo) = self.sync.target_schema_combo.clone() {
            let enabled = self.sync.target_connection.is_some() && !target_schemas.is_empty();
            combo.update(cx, |combo, cx| {
                combo.set_options(target_schemas, cx);
                combo.set_selected(self.sync.target_schema.clone(), cx);
                combo.set_placeholder(schema_placeholder, cx);
                combo.set_enabled(enabled, cx);
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

    /// The checked tables a run would actually write: unchanged tables are skipped, and
    /// 只同步目标已存在的表 drops the ones the target does not have.
    fn planned_sync_tables(&self) -> usize {
        let cross_engine = !self.sync_same_engine();
        self.sync
            .checked_indices()
            .into_iter()
            .filter(|index| {
                self.sync.tables.get(*index).is_some_and(|table| {
                    if self.sync.existing_only && !table.in_target {
                        return false;
                    }
                    table.has_changes(self.sync.mode, !cross_engine)
                })
            })
            .count()
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
        self.sync.source_schema.clear();
        self.sync.source_schemas.clear();
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
        self.sync.target_schema.clear();
        self.sync.target_schemas.clear();
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
        self.sync.source_schema.clear();
        self.sync.source_schemas.clear();
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
        self.sync.target_schema.clear();
        self.sync.target_schemas.clear();
        self.sync.config_saved = false;
        self.sync_refresh(cx);
        cx.notify();
    }

    pub(super) fn sync_source_schema_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if self.sync.source_schema == value {
            return;
        }
        self.sync.source_schema = value.to_string();
        self.sync.config_saved = false;
        self.sync_refresh(cx);
        cx.notify();
    }

    pub(super) fn sync_target_schema_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if self.sync.target_schema == value {
            return;
        }
        self.sync.target_schema = value.to_string();
        self.sync.config_saved = false;
        self.sync_refresh(cx);
        cx.notify();
    }

    /// Whether both sides use the same SQL dialect. Structure sync needs it: the DDL one engine
    /// reports is not valid on another. MySQL and MariaDB share `DriverDialect::Mysql` and their
    /// catalog/DDL syntax, so a pair of them is structure-syncable.
    fn sync_same_engine(&self) -> bool {
        match (self.sync.source_connection, self.sync.target_connection) {
            (Some(source), Some(target)) => {
                self.driver_dialect(source) == self.driver_dialect(target)
            }
            _ => true,
        }
    }

    /// The schema a schema-aware connection defaults to for a fresh scope. PostgreSQL uses
    /// `public`, SQL Server `dbo` and Oracle the login's own schema; other engines have none.
    fn sync_preferred_schema(&self, connection: usize) -> String {
        if !self.driver_supports(connection, DriverCapability::Schemas) {
            return String::new();
        }
        match self.driver_dialect(connection) {
            DriverDialect::Postgres => "public".to_string(),
            DriverDialect::SqlServer => "dbo".to_string(),
            DriverDialect::Oracle => self
                .connections
                .get(connection)
                .map(|node| node.profile.username.clone())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn sync_engine_name(&self, connection: Option<usize>) -> String {
        connection
            .and_then(|index| self.connections.get(index))
            .and_then(|node| self.registry.get(&node.profile.driver))
            .map(|driver| driver.display_name())
            .unwrap_or_default()
    }

    /// The engine mark to draw for a connection (the driver descriptor's icon and colour). Used by
    /// the source/target pickers so a connection is recognisable by its database type.
    fn sync_engine_icon(&self, connection: Option<usize>) -> (&'static str, u32) {
        let fallback = ("icons/connection.svg", self.theme.icon_connection);
        let Some(index) = connection else {
            return fallback;
        };
        let Some(node) = self.connections.get(index) else {
            return fallback;
        };
        let Some(driver) = self.registry.get(&node.profile.driver) else {
            return fallback;
        };
        let descriptor = driver.descriptor();
        let color = match descriptor.icon_style {
            DriverIconStyle::Brand(color) => color,
            _ => self.theme.icon_connection,
        };
        (descriptor.icon, color)
    }

    // ----- Saved transfer configurations ---------------------------------------------------------

    /// Explicit 刷新 of the Sync page's saved-configuration list: re-read `transfers.json` and
    /// drop selections whose configuration disappeared.
    pub(super) fn refresh_transfer_configs(&mut self, cx: &mut Context<'_, Self>) {
        self.transfer_configs = self.config.load_transfers().unwrap_or_default();
        let keys: std::collections::HashSet<String> = self
            .transfer_configs
            .iter()
            .enumerate()
            .filter_map(|(index, _)| self.transfer_key(index))
            .collect();
        self.transfers_selection.retain(|key| keys.contains(key));
        self.refresh_toast(ToastKind::Success, t!("grid.refreshed").to_string(), cx);
        cx.notify();
    }

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
                1 => SortKey::text(&self.transfer_scope_label(
                    &config.source_connection_id,
                    &config.source_database,
                    &config.source_schema,
                )),
                2 => SortKey::text(&self.transfer_scope_label(
                    &config.target_connection_id,
                    &config.target_database,
                    &config.target_schema,
                )),
                3 => SortKey::text(&sync_mode_label(SyncMode::from_saved(config.mode))),
                _ => SortKey::text(&config.name),
            }
        });
        indices
    }

    /// `connection · database · schema` for a saved scope, or a muted placeholder when unset.
    fn transfer_scope_label(&self, connection_id: &str, database: &str, schema: &str) -> String {
        if connection_id.is_empty() && database.is_empty() {
            return String::new();
        }
        let connection = self
            .connections
            .iter()
            .find(|node| node.profile.id == connection_id)
            .map(|node| node.profile.name.clone())
            .unwrap_or_else(|| connection_id.to_string());
        let mut label = connection;
        for part in [database, schema] {
            if !part.is_empty() {
                label.push_str(" · ");
                label.push_str(part);
            }
        }
        label
    }

    pub(super) fn transfer_key(&self, index: usize) -> Option<String> {
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

    /// Enter on the Transfer list: open the single selected configuration.
    fn open_selected_transfer(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self
            .transfers_selection
            .single()
            .and_then(|key| self.transfer_index_by_key(key))
        else {
            return;
        };
        self.open_transfer_config(index, cx);
    }

    /// Ctrl+A on the Transfer list: select every configuration the list currently shows.
    pub(super) fn select_all_transfers(&mut self, cx: &mut Context<'_, Self>) {
        let visible = self.visible_transfer_keys();
        self.transfers_selection.select_all(&visible);
        cx.notify();
    }

    /// Open the 新建传输 window with a fresh scope. The source starts at the tree's current
    /// database, but the target deliberately starts empty: a transfer must name its destination
    /// rather than silently default to the source or to another open connection.
    pub(super) fn open_new_transfer(&mut self, cx: &mut Context<'_, Self>) {
        // One transfer window at a time. A window with unsaved edits is only raised; a clean one
        // switches to the requested configuration (its view observes this state).
        let already_open = self.raise_sync_window(cx);
        if already_open && self.sync_config_dirty() {
            return;
        }
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
        // Start in the schema the user is already browsing, when there is one; the comparison
        // falls back to the engine default (`public` / `dbo`) otherwise.
        let source_schema = self
            .default_query_schema(cx, source, Some(source_database.as_str()))
            .unwrap_or_default();
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
        self.sync.source_schema = source_schema;
        self.sync.source_schemas.clear();
        self.sync.target_connection = None;
        self.sync.target_database = String::new();
        self.sync.target_schema.clear();
        self.sync.target_schemas.clear();
        self.reset_sync_run(cx);
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
        let already_open = self.raise_sync_window(cx);
        if already_open && self.sync_config_dirty() {
            return;
        }
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
        self.sync.source_schema = source
            .map(|_| config.source_schema.clone())
            .unwrap_or_default();
        self.sync.target_connection = target;
        self.sync.target_database = target
            .map(|_| config.target_database.clone())
            .unwrap_or_default();
        self.sync.target_schema = target
            .map(|_| config.target_schema.clone())
            .unwrap_or_default();
        for index in [source, target].into_iter().flatten() {
            if !matches!(
                self.connections.get(index).map(|node| &node.status),
                Some(ConnectionStatus::Connected(_))
            ) {
                self.connect(index, cx);
            }
        }
        self.reset_sync_run(cx);
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

    /// Clear the previous run's progress/log and the previous configuration's view state, so a
    /// freshly opened configuration starts clean (its tab, data filter and search match it).
    fn reset_sync_run(&mut self, cx: &mut Context<'_, Self>) {
        self.sync.running = false;
        self.sync.progress = 0.0;
        self.sync.log.clear();
        self.sync.log_open = false;
        self.sync.started = None;
        self.sync.elapsed = None;
        self.sync.error = None;
        self.sync.tables.clear();
        self.sync.selected = None;
        self.sync.pending_selected = None;
        self.sync.source_schemas.clear();
        self.sync.target_schemas.clear();
        self.sync.sub_tab = self.sync.mode.default_sub_tab();
        self.sync.data_filter = SyncDataFilter::All;
        self.sync.search.clear();
        let search_input = self.sync.search_input.clone();
        search_input.update(cx, |input, cx| input.set_text(String::new(), cx));
        self.sync.compared = 0;
        self.sync.compare_total = 0;
        self.sync.options_open = false;
        self.sync.close_confirm = false;
        self.sync.close_allowed = false;
        self.sync.run_confirm = false;
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
            source_schema: self.sync.source_schema.clone(),
            target_connection_id: self
                .connections
                .get(target)
                .map(|node| node.profile.id.clone())
                .unwrap_or_default(),
            target_database: self.sync.target_database.clone(),
            target_schema: self.sync.target_schema.clone(),
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
            let guard_weak = weak.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // The two-row toolbar and the two endpoint groups need room; below this the
                    // second row wraps the run controls onto a third line.
                    window_min_size: Some(size(px(980.0), px(600.0))),
                    ..ui::window_options(title)
                },
                move |window, cx| {
                    #[cfg(target_os = "windows")]
                    crate::win_resize::install(window);
                    window.activate_window();
                    let view = cx.new(|cx| TransferWindow::new(view_weak.clone(), &app_entity, cx));
                    // Closing a configuration with unsaved edits asks first (the in-window
                    // confirmation layers over this window). This also catches the OS close and
                    // Alt+F4, not just the titlebar ✕.
                    window.on_window_should_close(cx, move |_window, cx| {
                        let mut allow = true;
                        let _ = guard_weak.update(cx, |app, cx| {
                            allow = app.sync_close_allowed(cx);
                        });
                        allow
                    });
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

    /// Bring the open transfer window to the front. Returns whether one was already open.
    ///
    /// The 新建/编辑 actions used to reset the transfer state even when the window existed (the
    /// open then returned early), silently replacing the configuration the user was editing.
    fn raise_sync_window(&mut self, cx: &mut Context<'_, Self>) -> bool {
        let Some(handle) = self.sync_window.as_ref() else {
            return false;
        };
        if handle
            .update(cx, |_root, window, cx| {
                window.activate_window();
                cx.notify();
            })
            .is_err()
        {
            // The handle outlived its window (no close notification reached us): forget it so a
            // new configuration can open a fresh one.
            self.sync_window = None;
            return false;
        }
        true
    }

    /// Whether the window's configuration has edits that 保存配置 has not written yet.
    fn sync_config_dirty(&self) -> bool {
        // An unnamed new transfer has nothing saveable yet, so closing it does not lose a
        // configuration; a named one (new or opened) does.
        !self.sync.config_saved && !self.sync.config_name.trim().is_empty()
    }

    /// The transfer window's in-app close path (titlebar ✕ / Esc): remove it directly when there
    /// is nothing to lose, otherwise open the confirmation.
    fn request_sync_close(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.sync.close_allowed || !self.sync_config_dirty() {
            window.remove_window();
            return;
        }
        if !self.sync.close_confirm {
            self.sync.close_confirm = true;
            cx.notify();
        }
    }

    /// The transfer window's close guard. `true` lets the window go; `false` keeps it open and
    /// shows the in-window confirmation when there are unsaved edits.
    fn sync_close_allowed(&mut self, cx: &mut Context<'_, Self>) -> bool {
        if self.sync.close_allowed || !self.sync_config_dirty() {
            return true;
        }
        if !self.sync.close_confirm {
            self.sync.close_confirm = true;
            cx.notify();
        }
        false
    }

    /// The transfer window is gone: stop both the run and a comparison that is still walking the
    /// tables, and drop the preview editors built for it.
    pub(super) fn sync_window_closed(&mut self, cx: &mut Context<'_, Self>) {
        self.sync.cancel.store(true, Ordering::Relaxed);
        self.sync.running = false;
        // Bumped so the comparison task stops at its next table instead of querying on in the
        // background after the window is gone.
        self.sync.generation = self.sync.generation.wrapping_add(1);
        self.sync.comparing = false;
        self.sync.options_open = false;
        self.sync.close_confirm = false;
        self.sync.close_allowed = false;
        self.sync.run_confirm = false;
        self.sync.config_name_input = None;
        self.sync.needs_refresh = false;
        self.sync_purge_previews();
        cx.notify();
    }

    /// Stop the running comparison; rows that were still being compared are marked 已停止.
    pub(super) fn cancel_sync_compare(&mut self, cx: &mut Context<'_, Self>) {
        if !self.sync.comparing {
            return;
        }
        self.sync.generation = self.sync.generation.wrapping_add(1);
        self.sync.comparing = false;
        self.sync.pending_tables = None;
        self.sync.pending_selected = None;
        for table in &mut self.sync.tables {
            if table.comparing {
                table.comparing = false;
                table.stopped = true;
            }
        }
        cx.notify();
    }

    /// Drop the cached DDL/SQL preview editors the transfer workbench created. They are keyed per
    /// table (and mode), so keeping them all would retain two SQL editors for every table the user
    /// inspected. The current table's editors are rebuilt on the next render.
    fn sync_purge_previews(&mut self) {
        self.preview_editors
            .retain(|key, _| !key.starts_with("sync-"));
        self.preview_decorations
            .retain(|key, _| !key.starts_with("sync-"));
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
        let supports_source_schemas = self.driver_supports(source_index, DriverCapability::Schemas);
        let supports_target_schemas = self.driver_supports(target_index, DriverCapability::Schemas);
        let source_schema = self.sync.source_schema.clone();
        let target_schema = self.sync.target_schema.clone();
        let source_default_schema = self.sync_preferred_schema(source_index);
        let target_default_schema = self.sync_preferred_schema(target_index);
        let mode = self.sync.mode;
        let same_engine = self.sync_same_engine();
        let source_dialect = self.driver_dialect(source_index);
        let target_dialect = self.driver_dialect(target_index);
        // A saved configuration carries its table selection; apply it to the list this run builds.
        // The full re-compare also passes its selection so a refresh keeps the user's choices.
        let pending_tables = self.sync.pending_tables.take();
        let pending_selected = self.sync.pending_selected.take();
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
            // Schema-aware engines scope a sync to one schema. Ask the driver for the database's
            // real schema list (so an empty schema is still selectable) and fall back to the
            // prefixes the table list exposes when it cannot answer.
            let mut source_schemas = if supports_source_schemas {
                list_schemas(runtime.clone(), source.clone(), source_database.clone())
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if source_schemas.is_empty() {
                source_schemas = schema_names(&source_tables);
            }
            let mut target_schemas = if supports_target_schemas {
                list_schemas(runtime.clone(), target.clone(), target_database.clone())
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if target_schemas.is_empty() {
                target_schemas = schema_names(&target_tables);
            }
            let source_schema = choose_schema(
                &source_schema,
                &source_schemas,
                &source_default_schema,
                supports_source_schemas,
            );
            let target_schema = choose_schema(
                &target_schema,
                &target_schemas,
                &target_default_schema,
                supports_target_schemas,
            );
            let target_names: Vec<String> = target_tables
                .into_iter()
                .filter(|table| matches!(table.kind, ObjectKind::Table))
                .filter(|table| in_schema(&table.name, &target_schema))
                .map(|table| table.name)
                .collect();
            let names: Vec<String> = source_tables
                .into_iter()
                .filter(|table| matches!(table.kind, ObjectKind::Table))
                .filter(|table| in_schema(&table.name, &source_schema))
                .map(|table| table.name)
                .collect();
            let total = names.len();
            let _ = this.update(cx, |app, cx| {
                if generation != app.sync.generation {
                    return;
                }
                app.sync.source_schemas = source_schemas;
                app.sync.target_schemas = target_schemas;
                app.sync.source_schema = source_schema.clone();
                app.sync.target_schema = target_schema.clone();
                app.sync.tables = names
                    .iter()
                    .map(|name| {
                        let (target_name, in_target) = target_table_name(
                            name,
                            &target_names,
                            target_dialect,
                            &target_schema,
                            supports_target_schemas,
                        );
                        let mut table = SyncTable::new(name.clone(), target_name, in_target);
                        if let Some(saved) = pending_tables.as_ref() {
                            table.checked = saved.contains(name);
                        }
                        table
                    })
                    .collect();
                app.sync.compare_total = total;
                app.sync.selected = pending_selected
                    .as_ref()
                    .and_then(|name| names.iter().position(|candidate| candidate == name))
                    .or_else(|| (!names.is_empty()).then_some(0));
                // Push the schema list into the combos now: the transfer window may repaint
                // without the main window (which normally syncs them) rendering first.
                app.sync_sync_combos(cx);
                cx.notify();
            });

            for (index, name) in names.iter().enumerate() {
                let still_current = this
                    .update(cx, |app, _cx| generation == app.sync.generation)
                    .unwrap_or(false);
                if !still_current {
                    return;
                }
                let (target_name, in_target) = target_table_name(
                    name,
                    &target_names,
                    target_dialect,
                    &target_schema,
                    supports_target_schemas,
                );
                let compared = compare_table(
                    runtime.clone(),
                    source.clone(),
                    source_database.clone(),
                    target.clone(),
                    target_database.clone(),
                    name.clone(),
                    target_name,
                    in_target,
                    same_engine,
                    mode,
                    source_dialect,
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
        let same_engine = self.sync_same_engine();
        let Some(table) = self.sync.tables.get_mut(index) else {
            return;
        };
        table.comparing = false;
        // A fresh comparison supersedes the "written by this session" marker: the target may still
        // differ (an ALTER the engine normalized differently), and the status must say so.
        table.synced = false;
        table.stopped = false;
        table.source_schema = compared.source_schema;
        table.target_schema = compared.target_schema;
        table.schema_changes = compared.schema_changes;
        table.source_ddl = compared.source_ddl;
        table.target_ddl = compared.target_ddl;
        table.data = compared.data;
        table.error = compared.error;
        self.sync.keep_selection_visible(same_engine);
    }

    /// Re-compare every table (the toolbar's refresh action), keeping the checked set and the
    /// inspected table so a refresh does not undo the user's selection.
    pub(super) fn sync_recompare_all(&mut self, cx: &mut Context<'_, Self>) {
        if self.sync.comparing || self.sync.running {
            return;
        }
        let checked: HashSet<String> = self
            .sync
            .checked_indices()
            .into_iter()
            .filter_map(|index| self.sync.tables.get(index).map(|table| table.name.clone()))
            .collect();
        self.sync.pending_selected = self.sync.selected_table().map(|table| table.name.clone());
        self.sync.pending_tables = (!self.sync.tables.is_empty()).then_some(checked);
        self.sync_refresh(cx);
    }

    /// Re-read the given tables' comparison after a run, so the page reports what the target really
    /// holds — a table whose ALTER the engine normalized differently is not left looking synced.
    fn sync_recompare_tables(&mut self, names: Vec<String>, cx: &mut Context<'_, Self>) {
        if names.is_empty() {
            return;
        }
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
        let generation = self.sync.generation;
        let source_database = self.sync.source_database.clone();
        let target_database = self.sync.target_database.clone();
        let mode = self.sync.mode;
        let same_engine = self.sync_same_engine();
        let source_dialect = self.driver_dialect(source_index);
        let runtime = self.runtime.clone();
        let mut entries: Vec<(usize, String, String)> = Vec::new();
        for name in names {
            if let Some(index) = self.sync.tables.iter().position(|table| table.name == name) {
                let target_name = self.sync.tables[index].target_name.clone();
                if let Some(table) = self.sync.tables.get_mut(index) {
                    table.comparing = true;
                }
                entries.push((index, name, target_name));
            }
        }
        if entries.is_empty() {
            return;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            for (index, name, target_name) in entries {
                let compared = compare_table(
                    runtime.clone(),
                    source.clone(),
                    source_database.clone(),
                    target.clone(),
                    target_database.clone(),
                    name,
                    target_name,
                    // Every table in this list either already existed or was just created.
                    true,
                    same_engine,
                    mode,
                    source_dialect,
                )
                .await;
                let _ = this.update(cx, |app, cx| {
                    if generation == app.sync.generation {
                        app.apply_sync_comparison(index, compared);
                        // The run counted these as written; if the engine still reports a
                        // difference the change did not land as intended, so say so instead of
                        // leaving the log reading as an unqualified success.
                        let remaining = app.sync.tables.get(index).and_then(|table| {
                            // 全量传输 always "has changes" (it overwrites again by design).
                            (mode != SyncMode::Transfer && table.has_changes(mode, same_engine))
                                .then(|| {
                                    let count = match mode {
                                        SyncMode::Schema => table.schema_changes.len(),
                                        SyncMode::Data => table.data.total(),
                                        _ => table.schema_changes.len() + table.data.total(),
                                    };
                                    (table.name.clone(), count)
                                })
                        });
                        if let Some((name, count)) = remaining {
                            let message = t!("sync.log.still_differs", name = name, count = count)
                                .to_string();
                            app.sync_log(message, cx);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
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
        let target_name = table.target_name.clone();
        let in_target = table.in_target;
        table.comparing = true;
        table.error = None;
        let generation = self.sync.generation;
        let source_database = self.sync.source_database.clone();
        let target_database = self.sync.target_database.clone();
        let same_engine = self.sync_same_engine();
        let mode = self.sync.mode;
        let source_dialect = self.driver_dialect(source_index);
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
                target_name,
                in_target,
                same_engine,
                mode,
                source_dialect,
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

    /// 同步 opens the confirmation first; the run only starts from the dialog's 开始同步. A run
    /// writes to the target — structure DDL, row DML and, in 全量传输, a DROP of the target table —
    /// so a single click must not be enough.
    pub(super) fn request_sync_run(&mut self, cx: &mut Context<'_, Self>) {
        if self.sync.running || self.sync.comparing || self.planned_sync_tables() == 0 {
            return;
        }
        self.sync.run_confirm = true;
        cx.notify();
    }

    /// Columns that must not be written by the sync: generated/computed columns, which the target
    /// computes itself, and (when the catalog exposes it) Oracle identity columns declared
    /// `GENERATED ALWAYS`, which reject an explicit value. MySQL/MariaDB/SQLite accept
    /// auto-increment values and PostgreSQL is handled by the driver's `OVERRIDING SYSTEM VALUE`,
    /// so those keep their source values.
    fn skipped_write_columns(&self, table: &SyncTable, connection: usize) -> Vec<String> {
        let Some(schema) = table.source_schema.as_ref() else {
            return Vec::new();
        };
        let oracle = matches!(
            self.driver_dialect(connection),
            rustgrid_core::DriverDialect::Oracle
        );
        schema
            .columns
            .iter()
            .filter(|column| {
                !column.generated.trim().is_empty()
                    || (oracle && column.identity.eq_ignore_ascii_case("ALWAYS"))
                    || (oracle && is_oracle_lob(&column.data_type))
            })
            .map(|column| column.name.clone())
            .collect()
    }

    /// Apply the checked tables' plans, sequentially, streaming the log.
    pub(super) fn run_sync(&mut self, cx: &mut Context<'_, Self>) {
        self.sync.run_confirm = false;
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
            // Within one database, syncing two different schemas (e.g. public → archive) is a
            // real transfer. Only the same database *and* the same schema is a self-sync.
            let same_schema = self
                .sync
                .source_schema
                .eq_ignore_ascii_case(&self.sync.target_schema);
            if same_schema {
                self.sync.error = Some(t!("sync.error.same").to_string());
                cx.notify();
                return;
            }
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
        let source_dialect = self.driver_dialect(source_index);
        let target_dialect = self.driver_dialect(target_index);
        let plans: Vec<SyncPlan> = self
            .sync
            .checked_indices()
            .into_iter()
            .filter_map(|index| self.sync.tables.get(index))
            .filter(|table| table.has_changes(mode, same_engine))
            .map(|table| SyncPlan {
                name: table.name.clone(),
                target_name: table.target_name.clone(),
                in_target: table.in_target,
                has_key: table.has_key(),
                source_schema: table.source_schema.clone(),
                target_schema: table.target_schema.clone(),
                schema_changes: table.schema_changes.clone(),
                data: table.data.clone(),
                force_transfer: table.force_transfer,
                skip_write: self.skipped_write_columns(table, source_index),
            })
            .collect();
        if plans.is_empty() {
            self.sync.error = Some(t!("sync.error.nothing").to_string());
            cx.notify();
            return;
        }
        let plans = order_plans_for_creation(plans, mode);
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
            let mut synced_names: Vec<String> = Vec::new();
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
                    source_dialect,
                    target_dialect,
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
                            synced_names.push(plan.name.clone());
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
                // Re-read what was written: a table the engine normalized differently (or a data
                // write that did not land) must not keep the local "Synced ✓" state.
                if !cancelled {
                    app.sync_recompare_tables(synced_names, cx);
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
        let same_engine = self.sync_same_engine();
        let source_dialect = self
            .sync
            .source_connection
            .map(|index| self.driver_dialect(index))
            .unwrap_or_default();
        let target_name = table.target_name.clone();
        let connection = self
            .sync
            .target_connection
            .and_then(|index| self.connection_arc(index));
        let mut lines: Vec<String> = Vec::new();
        if mode.uses_schema() {
            lines.push(t!("sync.sql.schema_stage").to_string());
            if let Some(source) = table.source_schema.as_ref() {
                let mapped = if same_engine {
                    source.clone()
                } else {
                    map_schema(source, source_dialect, dialect)
                };
                match connection.as_ref() {
                    Some(connection) => {
                        let sql = if mode == SyncMode::Transfer || !table.in_target {
                            connection.table_schema_sql(&database, &target_name, None, &mapped)
                        } else if same_engine {
                            connection.table_schema_sql(
                                &database,
                                &target_name,
                                table.target_schema.as_ref(),
                                &mapped,
                            )
                        } else {
                            t!("sync.sql.cross_engine").to_string()
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
                // 全部/全量传输 create the table in the structure stage, then copy every row;
                // 仅数据 has no structure stage, so it cannot write into a missing target.
                if mode.uses_schema() {
                    lines.push(t!("sync.sql.full_copy", table = table.name.clone()).to_string());
                } else {
                    lines.push(t!("sync.sql.missing_target").to_string());
                }
            } else if mode == SyncMode::Transfer || table.force_transfer {
                if mode == SyncMode::Transfer && same_engine {
                    lines.push(format!(
                        "DROP TABLE IF EXISTS {};",
                        dialect.quote_table_name(&target_name)
                    ));
                } else {
                    lines.push(format!(
                        "DELETE FROM {};",
                        dialect.quote_table_name(&target_name)
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
                    lines.push(render_change_sql(&target_name, change, dialect));
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
        // With no saved configurations the page stays blank, exactly like the Queries and Backup
        // tabs: a centred illustration here made the three lists look like different pages. The
        // toolbar's 新建传输 is the way in.
        let body: AnyElement = if self.transfer_configs.is_empty() {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .into_any_element()
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
                    .key_context(TRANSFER_LIST_CONTEXT)
                    .on_action(cx.listener(|this, _: &OpenTransferConfig, _window, cx| {
                        this.open_selected_transfer(cx);
                    }))
                    .on_action(cx.listener(|this, _: &DeleteTransferConfig, _window, cx| {
                        this.confirm_delete_transfer(cx);
                    }))
                    .on_action(cx.listener(|this, _: &SelectAllTransfers, _window, cx| {
                        this.select_all_transfers(cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            window.focus(&this.sync.transfer_focus, cx);
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
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            // The blank area offers 新建传输; a row's own handler stops propagation
                            // and opens its menu instead.
                            this.transfers_selection.clear();
                            this.context_menu = Some(ContextMenu {
                                target: ContextTarget::TransferList,
                                position: event.position,
                            });
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
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "transfer-refresh",
                        "icons/refresh.svg",
                        t!("common.refresh").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| this.refresh_transfer_configs(cx)),
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
            longest[1] = longest[1].max(ui::approx_text_width(&self.transfer_scope_label(
                &config.source_connection_id,
                &config.source_database,
                &config.source_schema,
            )));
            longest[2] = longest[2].max(ui::approx_text_width(&self.transfer_scope_label(
                &config.target_connection_id,
                &config.target_database,
                &config.target_schema,
            )));
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
        let source = self.transfer_scope_label(
            &config.source_connection_id,
            &config.source_database,
            &config.source_schema,
        );
        let target = self.transfer_scope_label(
            &config.target_connection_id,
            &config.target_database,
            &config.target_schema,
        );
        let mode = sync_mode_label(SyncMode::from_saved(config.mode));
        let click_key = key.clone();
        let open_key = key.clone();
        let menu_key = key.clone();
        let row = ui::detail_row(
            SharedString::from(format!("transfer-row-{index}")),
            selected,
            theme,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            this.hit_transfer(&click_key, event.modifiers(), cx);
            if this.click_opens(event.click_count(), event.modifiers())
                && let Some(index) = this.transfer_index_by_key(&open_key)
            {
                this.open_transfer_config(index, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                this.hit_transfer(&menu_key, Modifiers::default(), cx);
                this.context_menu = Some(ContextMenu {
                    target: ContextTarget::TransferConfig { index },
                    position: event.position,
                });
                cx.notify();
            }),
        )
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
            let menu_key = key.clone();
            let tile = ui::grid_item_sized(
                SharedString::from(format!("transfer-tile-{index}")),
                selected,
                theme,
                width,
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                this.hit_transfer(&click_key, event.modifiers(), cx);
                if this.click_opens(event.click_count(), event.modifiers())
                    && let Some(index) = this.transfer_index_by_key(&open_key)
                {
                    this.open_transfer_config(index, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.hit_transfer(&menu_key, Modifiers::default(), cx);
                    this.context_menu = Some(ContextMenu {
                        target: ContextTarget::TransferConfig { index },
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
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
                .scroller("transfer-grid-scroll", columns),
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
        let close_weak = cx.weak_entity();
        let titlebar = ui::window_title_bar(
            title,
            theme,
            Some(Rc::new(
                move |_event: &ClickEvent, window: &mut Window, cx: &mut App| {
                    let _ = close_weak.update(cx, |app, cx| app.request_sync_close(window, cx));
                },
            )),
        );
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
                    if this.sync.run_confirm {
                        this.sync.run_confirm = false;
                        cx.notify();
                    } else if this.sync.close_confirm {
                        this.sync.close_confirm = false;
                        cx.notify();
                    } else if this.sync.options_open {
                        this.sync.options_open = false;
                        cx.notify();
                    } else {
                        // Opening the unsaved-changes confirmation instead of dropping the edits.
                        this.request_sync_close(window, cx);
                    }
                }
            }))
            .child(titlebar)
            .child(self.render_sync_workbench(cx))
            .child(self.render_sync_footer(cx));
        if self.sync.options_open {
            root = root.child(self.render_sync_options(cx));
        }
        if self.sync.run_confirm {
            root = root.child(self.render_sync_run_confirm(cx));
        }
        if self.sync.close_confirm {
            root = root.child(self.render_sync_close_confirm(cx));
        }
        root.into_any_element()
    }

    /// The in-window confirmation shown before 同步 writes to the target. It sits over the whole
    /// workbench like the unsaved-changes panel (the window is its own OS window, so the main
    /// window's dialog machinery is not involved).
    fn render_sync_run_confirm(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let weak = cx.weak_entity();
        let mode = sync_mode_label(self.sync.mode);
        let tables = self.planned_sync_tables();
        let source = self.sync_scope_label(true);
        let target = self.sync_scope_label(false);
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000066))
            .occlude()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(470.0))
                    .p_4()
                    .rounded(px(8.0))
                    .bg(rgb(theme.dialog_face))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .shadow(ui::dialog_shadow())
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text))
                            .child(t!("sync.confirm.title").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text_muted))
                            .child(
                                t!("sync.confirm.body", mode = mode, tables = tables).to_string(),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .px_2()
                            .py_1p5()
                            .rounded(px(4.0))
                            .bg(rgb(theme.header_bg))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .font_family("Consolas")
                            .text_size(px(11.5))
                            .text_color(rgb(theme.text))
                            .child(
                                t!("sync.confirm.scope", source = source, target = target)
                                    .to_string(),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(theme.danger))
                            .child(t!("sync.confirm.warning").to_string()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(ui::button(
                                "sync-run-cancel",
                                t!("sync.confirm.cancel").to_string(),
                                ButtonKind::Normal,
                                theme,
                                {
                                    let weak = weak.clone();
                                    move |_event: &ClickEvent, _window, cx: &mut App| {
                                        let _ = weak.update(cx, |app, cx| {
                                            app.sync.run_confirm = false;
                                            cx.notify();
                                        });
                                    }
                                },
                            ))
                            .child(ui::button(
                                "sync-run-start",
                                t!("sync.confirm.run").to_string(),
                                ButtonKind::Default,
                                theme,
                                move |_event: &ClickEvent, _window, cx: &mut App| {
                                    let _ = weak.update(cx, |app, cx| app.run_sync(cx));
                                },
                            )),
                    ),
            )
            .into_any_element()
    }

    /// `connection · database · schema` for one side of the current scope, for the confirmation's
    /// summary.
    fn sync_scope_label(&self, source: bool) -> String {
        let (connection, database, schema) = if source {
            (
                self.sync.source_connection,
                &self.sync.source_database,
                &self.sync.source_schema,
            )
        } else {
            (
                self.sync.target_connection,
                &self.sync.target_database,
                &self.sync.target_schema,
            )
        };
        let name = connection
            .and_then(|index| self.connections.get(index))
            .map(|node| node.profile.name.clone())
            .unwrap_or_else(|| t!("sync.select_connection").to_string());
        let mut label = name;
        for part in [database, schema] {
            if !part.is_empty() {
                label.push_str(" · ");
                label.push_str(part);
            }
        }
        label
    }

    /// The in-window confirmation shown when the window is closed with unsaved configuration
    /// edits. It layers over the whole workbench (the window is its own OS window, so the main
    /// window's dialog machinery is not involved).
    fn render_sync_close_confirm(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let error = self.sync.config_error.clone();
        let weak = cx.weak_entity();
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000066))
            .occlude()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(440.0))
                    .p_4()
                    .rounded(px(8.0))
                    .bg(rgb(theme.dialog_face))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .shadow(ui::dialog_shadow())
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text))
                            .child(t!("unsaved.title").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!("sync.close.body").to_string()),
                    )
                    .when_some(error, |panel, error| {
                        panel.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(theme.danger))
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(ui::button(
                                "sync-close-cancel",
                                t!("sync.close.cancel").to_string(),
                                ButtonKind::Normal,
                                theme,
                                {
                                    let weak = weak.clone();
                                    move |_event: &ClickEvent, _window, cx: &mut App| {
                                        let _ = weak.update(cx, |app, cx| {
                                            app.sync.close_confirm = false;
                                            cx.notify();
                                        });
                                    }
                                },
                            ))
                            .child(ui::button(
                                "sync-close-discard",
                                t!("sync.close.discard").to_string(),
                                ButtonKind::Normal,
                                theme,
                                {
                                    let weak = weak.clone();
                                    move |_event: &ClickEvent, window: &mut Window, cx: &mut App| {
                                        let _ = weak.update(cx, |app, cx| {
                                            app.sync.close_allowed = true;
                                            app.sync.close_confirm = false;
                                            cx.notify();
                                            window.remove_window();
                                        });
                                    }
                                },
                            ))
                            .child(ui::button(
                                "sync-close-save",
                                t!("sync.close.save").to_string(),
                                ButtonKind::Default,
                                theme,
                                move |_event: &ClickEvent, window: &mut Window, cx: &mut App| {
                                    let _ = weak.update(cx, |app, cx| {
                                        app.save_transfer_config(cx);
                                        if app.sync.config_saved {
                                            app.sync.close_allowed = true;
                                            app.sync.close_confirm = false;
                                            window.remove_window();
                                        }
                                        cx.notify();
                                    });
                                },
                            )),
                    ),
            )
            .into_any_element()
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

    /// The topology + mode switch + actions bar. It uses two rows: the source → target pickers and
    /// the sync-log toggle on the first, the mode switch and the run controls on the second, so the
    /// target picker is never squeezed or overlapped in a narrow window. The second row wraps its
    /// action group onto a new line when a long translation does not fit, instead of clipping the
    /// run button off the window edge.
    fn render_sync_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let cross_engine = !self.sync_same_engine();
        let mode = self.sync.mode;
        // The run button counts what a run would actually touch, not every checked row: unchanged
        // tables are skipped, so "Sync (23)" with one changed table was misleading.
        let planned = self.planned_sync_tables();
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
                    .tooltip(ui::text_tooltip(t!(mode.hint_key()).to_string()))
                    .when(enabled, |style| {
                        style.on_click(move |_event, _window, cx| {
                            let _ = weak.update(cx, |app, cx| {
                                if app.sync.mode != mode {
                                    app.sync.config_saved = false;
                                    app.sync.mode = mode;
                                    app.sync.sub_tab = mode.default_sub_tab();
                                    let same_engine = app.sync_same_engine();
                                    app.sync.keep_selection_visible(same_engine);
                                    // Structure-only comparison deliberately does not read rows
                                    // (a target key column may be missing until the structure is
                                    // applied). Re-run the comparison on a mode switch so a table
                                    // that only failed while reading data does not keep a stale
                                    // error, and so switching back to a data mode fills its diff.
                                    // An in-flight comparison was started with the old mode, so
                                    // stop it before starting the new one.
                                    if app.sync.comparing {
                                        app.cancel_sync_compare(cx);
                                    }
                                    app.sync_recompare_all(cx);
                                }
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
        // Re-run the comparison for every table, not just the inspected one.
        let recompare = ui::toolbar_item(
            "sync-recompare-all",
            "icons/refresh.svg",
            t!("common.refresh").to_string(),
            !running && !self.sync.comparing && ready,
            theme,
            cx.listener(|this, _event, _window, cx| this.sync_recompare_all(cx)),
        );
        // Bring the run log back after its panel was closed; it stays available for the last run.
        let log_toggle = ui::toolbar_item(
            "sync-log-toggle",
            "icons/activity.svg",
            t!("sync.log.title").to_string(),
            !self.sync.log.is_empty(),
            theme,
            cx.listener(|this, _event, _window, cx| {
                this.sync.log_open = !this.sync.log_open;
                cx.notify();
            }),
        );
        let stop_compare = self.sync.comparing.then(|| {
            ui::button(
                "sync-stop-compare",
                t!("sync.stop_compare").to_string(),
                ButtonKind::Normal,
                theme,
                cx.listener(|this, _event, _window, cx| this.cancel_sync_compare(cx)),
            )
        });

        let run_label = if mode == SyncMode::Transfer {
            t!("sync.transfer", count = planned).to_string()
        } else {
            t!("sync.run", count = planned).to_string()
        };
        let run = ui::button(
            "sync-run",
            run_label,
            if planned == 0 || running || self.sync.comparing || !ready {
                ButtonKind::Disabled
            } else {
                ButtonKind::Default
            },
            theme,
            cx.listener(|this, _event, _window, cx| this.request_sync_run(cx)),
        );

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
                    .child(div().flex_1().min_w(px(0.0)))
                    // The log toggle is a read-only view of the last run, so it rides on the first
                    // row's free space instead of competing with the run controls for width.
                    .child(log_toggle),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_2()
                    .pb_2()
                    .child(modes)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .flex_none()
                            .child(recompare)
                            .children(stop_compare)
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

    /// One side of the source → target picker: a label, the connection dropdown, the database
    /// dropdown and (on a schema-aware engine) the schema dropdown in a single bordered group.
    fn sync_endpoint_group(&self, source: bool, theme: Theme) -> Div {
        let (connection, database, schema, connection_index) = if source {
            (
                &self.sync.source_combo,
                &self.sync.source_database_combo,
                &self.sync.source_schema_combo,
                self.sync.source_connection,
            )
        } else {
            (
                &self.sync.target_combo,
                &self.sync.target_database_combo,
                &self.sync.target_schema_combo,
                self.sync.target_connection,
            )
        };
        let supports_schemas = connection_index
            .is_some_and(|index| self.driver_supports(index, DriverCapability::Schemas));
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
            // Each selector already draws its own trigger field; a second bordered box around the
            // group made the pickers look nested inside a text input.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(
                        div().w(px(130.0)).h(px(24.0)).child(
                            connection
                                .clone()
                                .map(|combo| combo.into_any_element())
                                .unwrap_or_else(|| div().into_any_element()),
                        ),
                    )
                    .child(
                        div().w(px(105.0)).h(px(24.0)).child(
                            database
                                .clone()
                                .map(|combo| combo.into_any_element())
                                .unwrap_or_else(|| div().into_any_element()),
                        ),
                    )
                    .when(supports_schemas, |group| {
                        group.child(
                            div().w(px(90.0)).h(px(24.0)).child(
                                schema
                                    .clone()
                                    .map(|combo| combo.into_any_element())
                                    .unwrap_or_else(|| div().into_any_element()),
                            ),
                        )
                    }),
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
                div().flex_1().min_w(px(0.0)).child(
                    t!(
                        "sync.hetero",
                        source = self.sync_engine_name(self.sync.source_connection),
                        target = self.sync_engine_name(self.sync.target_connection)
                    )
                    .to_string(),
                ),
            )
    }

    /// The table list column.
    fn render_sync_list(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let mode = self.sync.mode;
        let same_engine = self.sync_same_engine();
        let visible = self
            .sync
            .visible_indices(mode, same_engine, self.sync.hide_same);
        let total = self.sync.tables.len();
        let checked = self.sync.checked_indices().len();
        // 全选 applies to the rows the list actually shows: a search filter and 隐藏一致 both narrow
        // what the user can see, and selecting invisible rows from this checkbox was surprising.
        let all_checked = !visible.is_empty()
            && visible.iter().all(|index| {
                self.sync
                    .tables
                    .get(*index)
                    .is_some_and(|table| table.checked)
            });
        let select_indices = visible.clone();
        let weak = cx.weak_entity();

        let mut list = ui::DetailList::new(
            "sync-table-list",
            &self.sync.list_detail,
            SYNC_LIST_WIDTH - 14.0,
            div(),
        )
        .header_height(0.0);
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
        } else if visible.is_empty() {
            list = list.child(
                div()
                    .px_3()
                    .py_3()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(if self.sync.search.trim().is_empty() {
                        t!("sync.all_same").to_string()
                    } else {
                        t!("sync.no_matches").to_string()
                    }),
            );
        }
        for index in visible.iter().copied() {
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
                            // The data diff's scroll is shared by every table; a new table starts
                            // at its top instead of inheriting the previous one's offset.
                            app.sync.inspector_scroll.set_offset(Point::default());
                            app.sync_purge_previews();
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
                                            for index in &select_indices {
                                                if let Some(table) = app.sync.tables.get_mut(*index)
                                                {
                                                    table.checked = !all_checked;
                                                }
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
                    .child(self.render_hide_same_toggle(cx))
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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.context_menu = Some(ContextMenu {
                                target: ContextTarget::SyncList,
                                position: event.position,
                            });
                            cx.notify();
                        }),
                    )
                    .child(list.render(theme)),
            )
    }

    /// The 隐藏一致 switch under the table search: drop every row whose comparison found nothing
    /// to sync. Persisted in `settings.json`, on by default.
    fn render_hide_same_toggle(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let weak = cx.weak_entity();
        let same_engine = self.sync_same_engine();
        // How many rows the switch is currently taking out of the list, so its effect is visible
        // before the user hides them.
        let identical = self
            .sync
            .tables
            .iter()
            .filter(|table| table.is_identical(self.sync.mode, same_engine))
            .count();
        div()
            .id("sync-hide-same")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(theme.text))
            .on_click(move |_event, _window, cx| {
                let _ = weak.update(cx, |app, cx| {
                    app.sync.hide_same = !app.sync.hide_same;
                    let same_engine = app.sync_same_engine();
                    app.sync.keep_selection_visible(same_engine);
                    app.persist_settings();
                    cx.notify();
                });
            })
            .child(ui::checkbox_box(self.sync.hide_same, theme))
            .child(t!("sync.hide_same").to_string())
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(format!("({identical})")),
            )
            .into_any_element()
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
        let table_comparing = self
            .sync
            .selected_table()
            .is_some_and(|table| table.comparing);
        let mut tabs = div()
            .flex()
            .flex_row()
            .items_center()
            .h_full()
            .gap_4()
            .flex_none();
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
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .id("sync-inspector-name")
                            .min_w(px(0.0))
                            .truncate()
                            .tooltip(ui::text_tooltip(name.clone()))
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
                        !self.sync.comparing && !table_comparing,
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
    ///
    /// Each side is a read-only `ui::SqlEditor` (the same surface as the SQL 预览 panes): long
    /// lines wrap instead of being clipped, the body scrolls, and a mouse drag selects text that
    /// Ctrl+C copies. The changed lines are painted through the editor's decorations.
    fn render_sync_schema_tab(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(table) = self.sync.selected_table() else {
            return div().into_any_element();
        };
        if table.comparing {
            return sync_placeholder(t!("common.loading").to_string(), theme);
        }
        if table.stopped {
            return sync_placeholder(t!("sync.compare_stopped").to_string(), theme);
        }
        if let Some(error) = table.error.as_ref() {
            return sync_placeholder(error.clone(), theme);
        }
        let exists = table.in_target;
        let name = table.name.clone();
        let schema_changes = table.schema_changes.clone();
        let source_ddl = table.source_ddl.clone().unwrap_or_default();
        let target_ddl = table.target_ddl.clone().unwrap_or_default();
        // The DDL panes and the structure diff must tell the same story. When the snapshot diff
        // finds nothing (the tables are structurally equal, or a cross-engine pair has no
        // comparable structure), show the two definitions without red/green line marks instead of
        // highlighting formatting differences the sync will never apply.
        let (source_decorations, target_decorations) = if schema_changes.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let (source_lines, target_lines) = ddl_line_marks(&source_ddl, &target_ddl);
            (
                ddl_decorations(&source_ddl, &source_lines, theme),
                ddl_decorations(&target_ddl, &target_lines, theme),
            )
        };
        let source_text = if source_ddl.is_empty() {
            "--".to_string()
        } else {
            source_ddl
        };
        let target_text = if target_ddl.is_empty() {
            "--".to_string()
        } else {
            target_ddl
        };
        let source_engine = self.sync_engine_name(self.sync.source_connection);
        let target_engine = if exists {
            self.sync_engine_name(self.sync.target_connection)
        } else {
            t!("sync.not_exists").to_string()
        };
        let source_title = t!("sync.source_ddl").to_string();
        let target_title = t!("sync.target_ddl").to_string();

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(self.sync_ddl_pane(
                format!("sync-ddl-source-{name}"),
                source_title,
                source_engine,
                source_text,
                source_decorations,
                theme,
                cx,
            ))
            .child(self.sync_ddl_pane(
                format!("sync-ddl-target-{name}"),
                target_title,
                target_engine,
                target_text,
                target_decorations,
                theme,
                cx,
            ));
        // The changed lines are marked directly in the DDL panes; the old list of change summaries
        // above them only repeated what the red/green lines already show.
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(body)
            .into_any_element()
    }

    /// One side of the 结构比对 tab: a title bar over a read-only DDL editor.
    #[allow(clippy::too_many_arguments)]
    fn sync_ddl_pane(
        &mut self,
        key: String,
        title: String,
        engine: String,
        text: String,
        decorations: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> Div {
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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_hidden()
                    .child(self.render_sql_preview_marked(&key, &text, decorations, cx)),
            )
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
        if table.stopped {
            return sync_placeholder(t!("sync.compare_stopped").to_string(), theme);
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
            .child(div().flex_none().child(pills))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_align(gpui::TextAlign::Right)
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
                    .child(self.render_sync_log_lines(theme)),
            )
    }

    /// The log lines as one plain, selectable text run. A single run (instead of one selectable
    /// element per line) keeps a long log cheap to lay out, lets a drag select across lines, and
    /// copies through the window selection (Ctrl+C) as well as the 复制日志 button.
    fn render_sync_log_lines(&self, theme: Theme) -> Div {
        let text = self.sync.log.join("\n");
        let mut panel = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .mx_3()
            .mb_1()
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(theme.border))
            .overflow_hidden();
        if !text.is_empty() {
            let copy_text = text.clone();
            panel = panel.child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .items_center()
                    .px_1()
                    .pt_1()
                    .child(ui::popup_button(
                        "sync-log-copy",
                        t!("sqlfile.copy_log").to_string(),
                        false,
                        theme,
                        move |_event, _window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                        },
                    )),
            );
        }
        let body: AnyElement = if self.sync.log.is_empty() {
            div()
                .p_2()
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child("--")
                .into_any_element()
        } else {
            div()
                .p_2()
                .w_full()
                .font_family("Consolas")
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .cursor_text()
                .child(
                    SelectableText::with_handle(
                        "sync-log-text",
                        self.sync.log_handle.clone(),
                        text,
                    )
                    .selection_color(rgba((theme.grid_selection_bg << 8) | 0x59).into()),
                )
                .into_any_element()
        };
        panel
            .child(
                div()
                    .id("sync-log-lines")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.sync.log_scroll)
                    .child(body),
            )
            .child(
                Scrollbar::vertical(&self.sync.log_scroll)
                    .mode(ScrollbarMode::Always)
                    .id("sync-log-scrollbar"),
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
                    // A label longer than the popup wraps onto a second line: the check box then
                    // stays level with the label's first line instead of centring against the
                    // middle of the block, and the padding keeps a single-line row at the 24px
                    // control height.
                    .items_start()
                    .justify_between()
                    .gap_2()
                    .py_1()
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
                    .child(div().min_w(px(0.0)).child(label))
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

/// List a database's schemas through the runtime.
async fn list_schemas(
    runtime: Arc<Runtime>,
    connection: Arc<dyn Connection>,
    database: String,
) -> Result<Vec<String>, String> {
    match runtime
        .spawn(async move { connection.list_schemas(&database).await })
        .await
    {
        Ok(Ok(schemas)) => Ok(schemas),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// Compare one table: its definitions, DDL and rows. Every database call goes through the runtime.
#[allow(clippy::too_many_arguments)]
async fn compare_table(
    runtime: Arc<Runtime>,
    source: Arc<dyn Connection>,
    source_database: String,
    target: Arc<dyn Connection>,
    target_database: String,
    name: String,
    target_name: String,
    in_target: bool,
    same_engine: bool,
    mode: SyncMode,
    source_dialect: rustgrid_core::DriverDialect,
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
            target_name.clone(),
        )
        .await
        .ok()
        .flatten();
        compared.target_ddl = load_ddl(
            runtime.clone(),
            target.clone(),
            target_database.clone(),
            target_name.clone(),
        )
        .await
        .ok()
        .flatten();
        load_columns(
            runtime.clone(),
            target.clone(),
            target_database.clone(),
            target_name.clone(),
        )
        .await
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    compared.schema_changes = match (&compared.source_schema, &compared.target_schema) {
        // A cross-engine target's native type names (`bigint` vs `NUMBER`) always differ
        // textually, so comparing their DDL would report a permanent difference. Structure
        // changes are only compared within one dialect; a missing table is still all-new.
        (Some(source_schema), Some(target_schema)) if same_engine => {
            diff_schema(source_schema, target_schema)
        }
        (Some(source_schema), None) if !in_target => {
            diff_schema(source_schema, &TableSchema::default())
        }
        _ => Vec::new(),
    };

    // Structure-only comparison must not read rows at all. Besides being wasted work, the target
    // structure may legitimately differ (a missing key column is exactly what is about to be
    // synced), so ordering the target by the source key produced errors such as MySQL's
    // "1054 - Unknown column ... in 'order clause'" and made the structure comparison fail.
    // A primary key is preferred; a table without one can still be compared through a unique
    // NOT NULL key (Navicat asks for such a key; the workbench has no picker, so it takes the
    // narrowest usable one).
    let keys = if mode.uses_data() {
        usable_key_columns(&source_columns, compared.source_schema.as_ref())
    } else {
        Vec::new()
    };
    let order_by: Vec<SortColumn> = keys
        .iter()
        .map(|column| SortColumn {
            column: column.clone(),
            descending: false,
        })
        .collect();
    // A table the target does not have is compared as if its columns matched the source: every
    // source row is an insert, and the values shown in the inspector are the values that will be
    // written after the table is created.
    let compare_target_columns = if in_target {
        target_columns
    } else {
        source_columns.clone()
    };
    if mode.uses_data()
        && in_target
        && !keys.is_empty()
        && key_positions(&compare_target_columns, &keys).is_none()
    {
        // The fallback path used to order the target by the source key even when the target did
        // not have that column (a structure difference), which surfaced as a raw driver error.
        // A clear message lets the user run a structure sync first.
        let missing: Vec<&str> = keys
            .iter()
            .filter(|key| {
                !compare_target_columns
                    .iter()
                    .any(|column| column.name.eq_ignore_ascii_case(key))
            })
            .map(String::as_str)
            .collect();
        compared.error =
            Some(t!("sync.error.missing_key", columns = missing.join(", ")).to_string());
        return compared;
    }
    if !keys.is_empty() {
        if streamable_keys(&source_columns, &keys)
            && streamable_keys(&compare_target_columns, &keys)
        {
            match compare_rows_streaming(
                runtime.clone(),
                source.clone(),
                source_database.clone(),
                target.clone(),
                target_database.clone(),
                name.clone(),
                target_name.clone(),
                &source_columns,
                &compare_target_columns,
                &keys,
                in_target,
            )
            .await
            {
                Ok(data) => compared.data = data,
                Err(error) => {
                    compared.error = Some(error);
                    return compared;
                }
            }
        } else {
            // Text keys depend on the server's collation for their order; a merge join would be
            // wrong when the two sides sort differently. Keep the bounded hash comparison for
            // those, and stream numeric keys (the usual large-table case).
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
            let (target_rows, target_truncated) = if in_target {
                match fetch_all_rows(runtime, target, target_database, target_name, order_by).await
                {
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
                &compare_target_columns,
                &target_rows,
                &keys,
                truncated,
            );
        }
    }
    if let Some(schema) = compared.source_schema.as_ref() {
        prune_generated_changes(&mut compared.data, schema);
    }
    if matches!(source_dialect, rustgrid_core::DriverDialect::Oracle) {
        // Navicat's data synchronization skips Oracle LOBs (BLOB/CLOB/NCLOB/LONG/LONG RAW).
        // Comparing them would either stream huge values or report a difference the sync never
        // writes, so leave them out of the plan and the preview.
        let lobs: Vec<&str> = source_columns
            .iter()
            .filter(|column| is_oracle_lob(&column.data_type))
            .map(|column| column.name.as_str())
            .collect();
        prune_skipped_changes(&mut compared.data, &lobs);
    }
    compared
}

fn is_oracle_lob(data_type: &str) -> bool {
    let base = data_type
        .split('(')
        .next()
        .unwrap_or(data_type)
        .trim()
        .to_ascii_uppercase();
    matches!(
        base.as_str(),
        "BLOB" | "CLOB" | "NCLOB" | "LONG" | "LONG RAW"
    )
}

/// The schema part of a possibly qualified `schema.table` name.
fn schema_of(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(schema, _)| schema)
}

/// The bare table name of a possibly qualified `schema.table` name.
fn bare_name(name: &str) -> &str {
    name.rsplit_once('.')
        .map(|(_, table)| table)
        .unwrap_or(name)
}

/// Whether `name` belongs to `schema`. An empty schema (a schema-less engine) matches everything.
fn in_schema(name: &str, schema: &str) -> bool {
    schema.is_empty() || schema_of(name).is_some_and(|part| part.eq_ignore_ascii_case(schema))
}

/// The distinct schema prefixes the table list exposes, sorted case-insensitively.
fn schema_names(tables: &[rustgrid_core::TableInfo]) -> Vec<String> {
    let mut names: Vec<String> = tables
        .iter()
        .filter_map(|table| schema_of(&table.name).map(|schema| schema.to_string()))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

/// Keep an existing schema when it is still offered, otherwise prefer the engine's default and
/// finally the first listed schema. A schema-aware engine with an empty list keeps the default so
/// an empty target database still maps into `dbo`/`public`.
fn choose_schema(previous: &str, available: &[String], preferred: &str, supported: bool) -> String {
    if !supported {
        return String::new();
    }
    if let Some(found) = available
        .iter()
        .find(|schema| schema.eq_ignore_ascii_case(previous))
    {
        return found.clone();
    }
    if let Some(found) = available
        .iter()
        .find(|schema| schema.eq_ignore_ascii_case(preferred))
    {
        return found.clone();
    }
    available
        .first()
        .cloned()
        .unwrap_or_else(|| preferred.to_string())
}

/// The target's name for one source table, plus whether it already exists in the selected target
/// schema. A schema-aware target always receives the selected target schema (so PostgreSQL's
/// `public.users` becomes SQL Server's `dbo.users` rather than a literal `public` schema), while a
/// schema-less target receives the bare name.
fn target_table_name(
    source: &str,
    target_names: &[String],
    target_dialect: rustgrid_core::DriverDialect,
    target_schema: &str,
    target_has_schemas: bool,
) -> (String, bool) {
    let bare = bare_name(source);
    let existing = target_names
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(source))
        .or_else(|| {
            target_names
                .iter()
                .find(|candidate| bare_name(candidate).eq_ignore_ascii_case(bare))
        });
    match existing {
        Some(found) => {
            let name = if target_has_schemas && !target_schema.is_empty() {
                format!("{target_schema}.{}", bare_name(found))
            } else {
                found.clone()
            };
            (name, true)
        }
        None => {
            let name = if target_has_schemas && !target_schema.is_empty() {
                format!("{target_schema}.{bare}")
            } else {
                map_table_name(source, target_dialect)
            };
            (name, false)
        }
    }
}

/// Whether a merge join over the key's server-side order is safe.
///
/// Numeric keys have one total order on every engine, so the two ordered streams can be merged
/// without loading either table. Text and binary keys use the server's collation/byte order, which
/// is not exposed through `PageRequest`; those keep the bounded hash comparison.
fn streamable_keys(columns: &[ColumnInfo], keys: &[String]) -> bool {
    keys.iter().all(|key| {
        columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(key))
            .is_some_and(|column| {
                let data_type = column.data_type.to_ascii_lowercase();
                data_type.contains("int")
                    || data_type.contains("serial")
                    || data_type.contains("number")
                    || data_type.contains("numeric")
                    || data_type.contains("decimal")
                    || data_type.contains("float")
                    || data_type.contains("double")
                    || data_type.contains("real")
                    || data_type.contains("money")
                    || data_type.contains("year")
                    || data_type.contains("bit")
            })
    })
}

/// A forward-only page reader used by [`compare_rows_streaming`].
struct RowStream {
    runtime: Arc<Runtime>,
    connection: Arc<dyn Connection>,
    database: String,
    table: String,
    order_by: Vec<SortColumn>,
    page: u64,
    rows: std::vec::IntoIter<Vec<CellValue>>,
    done: bool,
}

impl RowStream {
    fn new(
        runtime: Arc<Runtime>,
        connection: Arc<dyn Connection>,
        database: String,
        table: String,
        order_by: Vec<SortColumn>,
    ) -> Self {
        Self {
            runtime,
            connection,
            database,
            table,
            order_by,
            page: 0,
            rows: Vec::new().into_iter(),
            done: false,
        }
    }

    async fn next(&mut self) -> Result<Option<Vec<CellValue>>, String> {
        loop {
            if let Some(row) = self.rows.next() {
                return Ok(Some(row));
            }
            if self.done {
                return Ok(None);
            }
            let request = PageRequest::new(self.page, SYNC_STREAM_PAGE)
                .with_order_by(self.order_by.clone())
                .with_skip_total(true);
            let connection = self.connection.clone();
            let database = self.database.clone();
            let table = self.table.clone();
            let fetched = match self
                .runtime
                .spawn(async move { connection.fetch_page(&database, &table, request).await })
                .await
            {
                Ok(Ok(page)) => page,
                Ok(Err(error)) => return Err(error.to_string()),
                Err(error) => return Err(error.to_string()),
            };
            self.page += 1;
            if fetched.rows.len() < SYNC_STREAM_PAGE as usize {
                self.done = true;
            }
            if fetched.rows.is_empty() {
                return Ok(None);
            }
            self.rows = fetched.rows.into_iter();
        }
    }
}

/// Compare two key-ordered row streams without holding either table in memory.
///
/// The merge emits inserts for source-only keys, deletes for target-only keys, and updates for
/// keys on both sides. Only the differences are retained, so a table larger than the old 5000-row
/// cap compares completely.
#[allow(clippy::too_many_arguments)]
async fn compare_rows_streaming(
    runtime: Arc<Runtime>,
    source: Arc<dyn Connection>,
    source_database: String,
    target: Arc<dyn Connection>,
    target_database: String,
    source_table: String,
    target_table: String,
    source_columns: &[ColumnInfo],
    target_columns: &[ColumnInfo],
    keys: &[String],
    in_target: bool,
) -> Result<DataDiff, String> {
    let Some(source_indexes) = key_positions(source_columns, keys) else {
        return Ok(DataDiff {
            key_columns: keys.to_vec(),
            ..Default::default()
        });
    };
    if in_target && key_positions(target_columns, keys).is_none() {
        return Ok(DataDiff {
            key_columns: keys.to_vec(),
            ..Default::default()
        });
    }
    let target_indexes = key_positions(target_columns, keys).unwrap_or_default();
    let order_by: Vec<SortColumn> = keys
        .iter()
        .map(|column| SortColumn {
            column: column.clone(),
            descending: false,
        })
        .collect();
    let mut source_stream = RowStream::new(
        runtime.clone(),
        source,
        source_database,
        source_table,
        order_by.clone(),
    );
    let mut target_stream = if in_target {
        Some(RowStream::new(
            runtime,
            target,
            target_database,
            target_table,
            order_by,
        ))
    } else {
        None
    };

    let mut data = DataDiff {
        key_columns: keys.to_vec(),
        ..Default::default()
    };
    let mut source_row = source_stream.next().await?;
    let mut target_row = match target_stream.as_mut() {
        Some(stream) => stream.next().await?,
        None => None,
    };
    loop {
        match (source_row.take(), target_row.take()) {
            (Some(source_values), Some(target_values)) => {
                let ordering = compare_key_rows(
                    &source_values,
                    &target_values,
                    &source_indexes,
                    &target_indexes,
                );
                match ordering {
                    CmpOrdering::Less => {
                        data.inserts += 1;
                        data.rows.push(insert_row_change(
                            source_columns,
                            target_columns,
                            &source_values,
                            keys,
                        ));
                        source_row = source_stream.next().await?;
                        target_row = Some(target_values);
                    }
                    CmpOrdering::Greater => {
                        data.deletes += 1;
                        data.rows
                            .push(delete_row_change(target_columns, &target_values, keys));
                        target_row = match target_stream.as_mut() {
                            Some(stream) => stream.next().await?,
                            None => None,
                        };
                        source_row = Some(source_values);
                    }
                    CmpOrdering::Equal => {
                        if let Some(change) = update_row_change(
                            source_columns,
                            target_columns,
                            &source_values,
                            &target_values,
                            keys,
                        ) {
                            data.updates += 1;
                            data.rows.push(change);
                        }
                        source_row = source_stream.next().await?;
                        target_row = match target_stream.as_mut() {
                            Some(stream) => stream.next().await?,
                            None => None,
                        };
                    }
                }
            }
            (Some(source_values), None) => {
                data.inserts += 1;
                data.rows.push(insert_row_change(
                    source_columns,
                    target_columns,
                    &source_values,
                    keys,
                ));
                source_row = source_stream.next().await?;
            }
            (None, Some(target_values)) => {
                data.deletes += 1;
                data.rows
                    .push(delete_row_change(target_columns, &target_values, keys));
                target_row = match target_stream.as_mut() {
                    Some(stream) => stream.next().await?,
                    None => None,
                };
            }
            (None, None) => break,
        }
    }
    Ok(data)
}

fn key_positions(columns: &[ColumnInfo], keys: &[String]) -> Option<Vec<usize>> {
    keys.iter()
        .map(|key| {
            columns
                .iter()
                .position(|column| column.name.eq_ignore_ascii_case(key))
        })
        .collect()
}

fn compare_key_rows(
    source: &[CellValue],
    target: &[CellValue],
    source_indexes: &[usize],
    target_indexes: &[usize],
) -> CmpOrdering {
    for (source_index, target_index) in source_indexes.iter().zip(target_indexes) {
        let ordering = compare_key_cells(source.get(*source_index), target.get(*target_index));
        if ordering != CmpOrdering::Equal {
            return ordering;
        }
    }
    CmpOrdering::Equal
}

fn compare_key_cells(source: Option<&CellValue>, target: Option<&CellValue>) -> CmpOrdering {
    match (source, target) {
        (None, None) | (Some(CellValue::Null), Some(CellValue::Null)) => CmpOrdering::Equal,
        (None, _) | (Some(CellValue::Null), _) => CmpOrdering::Less,
        (_, None) | (_, Some(CellValue::Null)) => CmpOrdering::Greater,
        (Some(CellValue::Bool(left)), Some(CellValue::Bool(right))) => left.cmp(right),
        (Some(CellValue::Int(left)), Some(CellValue::Int(right))) => left.cmp(right),
        (Some(CellValue::Uint(left)), Some(CellValue::Uint(right))) => left.cmp(right),
        (Some(CellValue::Int(left)), Some(CellValue::Uint(right))) => {
            if *left < 0 {
                CmpOrdering::Less
            } else {
                (*left as u64).cmp(right)
            }
        }
        (Some(CellValue::Uint(left)), Some(CellValue::Int(right))) => {
            if *right < 0 {
                CmpOrdering::Greater
            } else {
                left.cmp(&(*right as u64))
            }
        }
        (Some(CellValue::Float(left)), Some(CellValue::Float(right))) => {
            left.partial_cmp(right).unwrap_or(CmpOrdering::Equal)
        }
        (Some(CellValue::Float(left)), Some(CellValue::Int(right))) => left
            .partial_cmp(&(*right as f64))
            .unwrap_or(CmpOrdering::Equal),
        (Some(CellValue::Int(left)), Some(CellValue::Float(right))) => (*left as f64)
            .partial_cmp(right)
            .unwrap_or(CmpOrdering::Equal),
        (Some(CellValue::Float(left)), Some(CellValue::Uint(right))) => left
            .partial_cmp(&(*right as f64))
            .unwrap_or(CmpOrdering::Equal),
        (Some(CellValue::Uint(left)), Some(CellValue::Float(right))) => (*left as f64)
            .partial_cmp(right)
            .unwrap_or(CmpOrdering::Equal),
        (Some(left), Some(right)) => left.as_display().cmp(&right.as_display()),
    }
}

/// Remove generated/computed columns from the diff: the target computes them itself, so a value
/// difference there is not an update the sync can (or should) write. An update left with no
/// writable column disappears; an insert stays, because it may still create a default row.
fn prune_generated_changes(data: &mut DataDiff, schema: &TableSchema) {
    let generated: Vec<&str> = schema
        .columns
        .iter()
        .filter(|column| !column.generated.trim().is_empty())
        .map(|column| column.name.as_str())
        .collect();
    prune_skipped_changes(data, &generated);
}

/// Remove the named columns from a diff; an update with nothing writable left disappears.
fn prune_skipped_changes(data: &mut DataDiff, skipped: &[&str]) {
    if skipped.is_empty() {
        return;
    }
    data.rows.retain_mut(|change| {
        change
            .values
            .retain(|(column, _)| !skipped.iter().any(|name| name.eq_ignore_ascii_case(column)));
        change.changes.retain(|change| {
            !skipped
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&change.column))
        });
        change.kind != RowChangeKind::Update || !change.values.is_empty()
    });
    data.inserts = data
        .rows
        .iter()
        .filter(|change| change.kind == RowChangeKind::Insert)
        .count();
    data.updates = data
        .rows
        .iter()
        .filter(|change| change.kind == RowChangeKind::Update)
        .count();
    data.deletes = data
        .rows
        .iter()
        .filter(|change| change.kind == RowChangeKind::Delete)
        .count();
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

/// Drop the columns the target must generate itself from one row's write set.
fn values_without_skipped(
    values: &[(String, Option<String>)],
    skipped: &[String],
) -> Vec<(String, Option<String>)> {
    values
        .iter()
        .filter(|(column, _)| {
            !skipped
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(column))
        })
        .cloned()
        .collect()
}

/// Order the plans so a table is created after the tables its foreign keys reference.
///
/// A `CREATE TABLE` carries its foreign keys, so creating a child before its parent fails on
/// engines that validate references immediately. Navicat avoids this by deploying the structure
/// scripts in dependency order; the workbench only lists tables, so ordering the plans is enough.
/// A cycle (or a reference to a table outside the checked set) cannot be ordered and is left in
/// its original position rather than dropped.
fn order_plans_for_creation(plans: Vec<SyncPlan>, mode: SyncMode) -> Vec<SyncPlan> {
    if plans.len() < 2 {
        return plans;
    }
    // A new table, a full transfer, and an existing table whose structure changes can all emit a
    // `CREATE TABLE`/`ADD CONSTRAINT` that references another table, so all three participate in
    // the order.
    let recreated: Vec<bool> = plans
        .iter()
        .map(|plan| {
            mode == SyncMode::Transfer
                || !plan.in_target
                || (mode.uses_schema() && !plan.schema_changes.is_empty())
        })
        .collect();
    // Every plan name is matched both exactly and by its bare (schema-less) name, because MySQL
    // reports `users` while PostgreSQL/SQL Server report `public.users`.
    let mut by_name: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (index, plan) in plans.iter().enumerate() {
        by_name.entry(plan.name.to_lowercase()).or_insert(index);
        if let Some((_, bare)) = plan.name.rsplit_once('.') {
            by_name.entry(bare.to_lowercase()).or_insert(index);
        }
    }
    let mut dependencies: Vec<std::collections::HashSet<usize>> =
        vec![std::collections::HashSet::new(); plans.len()];
    for (index, plan) in plans.iter().enumerate() {
        if !recreated[index] {
            continue;
        }
        let Some(schema) = plan.source_schema.as_ref() else {
            continue;
        };
        for foreign_key in &schema.foreign_keys {
            let referenced = foreign_key.referenced_table.to_lowercase();
            let Some(&parent) = by_name.get(&referenced) else {
                continue;
            };
            if parent != index && recreated[parent] {
                dependencies[index].insert(parent);
            }
        }
    }
    let mut ordered: Vec<SyncPlan> = Vec::with_capacity(plans.len());
    let mut placed = vec![false; plans.len()];
    loop {
        let mut progressed = false;
        for index in 0..plans.len() {
            if placed[index] || !dependencies[index].iter().all(|parent| placed[*parent]) {
                continue;
            }
            placed[index] = true;
            ordered.push(plans[index].clone());
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    // Cycles and any plan left behind keep their original relative order.
    for (index, plan) in plans.into_iter().enumerate() {
        if !placed[index] {
            ordered.push(plan);
        }
    }
    ordered
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
    source_dialect: rustgrid_core::DriverDialect,
    target_dialect: rustgrid_core::DriverDialect,
    include_delete: bool,
    cancel: &Arc<AtomicBool>,
) -> Result<String, String> {
    let name = plan.name.clone();
    let target_name = plan.target_name.clone();
    let mut created = false;

    // ----- Structure -------------------------------------------------------------------------
    // Within one dialect the target driver generates an `ALTER` from the two snapshots. Across
    // dialects only a missing table is created, and its definition is first translated into the
    // target's native types; an existing cross-engine table is never altered (the two type systems
    // cannot be compared textually).
    if mode.uses_schema() && (same_engine || !plan.in_target) {
        let Some(source_schema) = plan.source_schema.clone() else {
            return Err(t!("sync.error.no_schema").to_string());
        };
        let mapped_schema = if same_engine {
            source_schema.clone()
        } else {
            map_schema(&source_schema, source_dialect, target_dialect)
        };
        if mode == SyncMode::Transfer && plan.in_target && same_engine {
            let target_connection = target.clone();
            let database = target_database.clone();
            let table = target_name.clone();
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
            let table = target_name.clone();
            match runtime
                .spawn(async move {
                    target_connection
                        .save_table_schema(&database, &table, None, &mapped_schema)
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
            let table = target_name.clone();
            match runtime
                .spawn(async move {
                    target_connection
                        .save_table_schema(
                            &database,
                            &table,
                            target_schema.as_ref(),
                            &mapped_schema,
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
    // Data-only mode still creates a missing target table when the pair shares a dialect, which is
    // how Navicat's transfer treats its "Create tables" option. Without this, 仅数据 could not
    // populate a fresh database at all (the structure stage is skipped in that mode).
    // A data-only run never creates a table. Structures that can create one do it above; this
    // guard only catches a stale plan (for example a target table dropped after the comparison).
    if mode.uses_data() && !plan.in_target && !created {
        return Err(t!("sync.error.missing_target").to_string());
    }
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
            let table = target_name.clone();
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
            target_name.clone(),
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
                        .filter(|(_, column)| {
                            !plan
                                .skip_write
                                .iter()
                                .any(|skipped| skipped.eq_ignore_ascii_case(&column.name))
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
            let table = target_name.clone();
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
            values: values_without_skipped(&change.values, &plan.skip_write),
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
        let table = target_name.clone();
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
        .filter_map(|change| {
            let set = values_without_skipped(&change.values, &plan.skip_write);
            (!set.is_empty()).then(|| RowUpdate {
                set,
                keys: change.keys.clone(),
            })
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
        let table = target_name.clone();
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
            let table = target_name.clone();
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
    let source_keys: Vec<String> = source_lines
        .iter()
        .map(|line| comparable_ddl_line(line))
        .collect();
    let target_keys: Vec<String> = target_lines
        .iter()
        .map(|line| comparable_ddl_line(line))
        .collect();
    let mut source_marks = Vec::with_capacity(source_lines.len());
    let mut target_marks = Vec::with_capacity(target_lines.len());

    let mut target_counts: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for key in &target_keys {
        *target_counts.entry(key.as_str()).or_default() += 1;
    }
    for (line, key) in source_lines.iter().zip(&source_keys) {
        match target_counts.get_mut(key.as_str()) {
            Some(count) if *count > 0 => {
                *count -= 1;
                source_marks.push((line.clone(), LineMark::Same));
            }
            _ => source_marks.push((line.clone(), LineMark::Add)),
        }
    }
    let mut source_counts: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for key in &source_keys {
        *source_counts.entry(key.as_str()).or_default() += 1;
    }
    for (line, key) in target_lines.iter().zip(&target_keys) {
        match source_counts.get_mut(key.as_str()) {
            Some(count) if *count > 0 => {
                *count -= 1;
                target_marks.push((line.clone(), LineMark::Same));
            }
            _ => target_marks.push((line.clone(), LineMark::Del)),
        }
    }
    (source_marks, target_marks)
}

/// Normalize one DDL line for the line diff. `AUTO_INCREMENT` is a counter, not a definition (the
/// structure comparison ignores it), so a differing next id must not keep the options line
/// highlighted forever.
fn comparable_ddl_line(line: &str) -> String {
    const TOKEN: &str = "AUTO_INCREMENT=";
    let upper = line.to_ascii_uppercase();
    let mut out = String::with_capacity(line.len());
    let mut from = 0usize;
    while let Some(offset) = upper[from..].find(TOKEN) {
        let start = from + offset;
        out.push_str(&line[from..start]);
        let mut end = start + TOKEN.len();
        while end < line.len() && line.as_bytes()[end].is_ascii_digit() {
            end += 1;
        }
        from = end;
        // Removing the option leaves the two surrounding spaces; keep one.
        if out.ends_with(' ') && line[from..].starts_with(' ') {
            from += 1;
        }
    }
    out.push_str(&line[from..]);
    out.trim().to_string()
}

/// The diff highlight of each non-identical DDL line, as byte ranges into `text` (the editor's
/// content). The ranges line up with `marks`, which [`ddl_line_marks`] built from the same text.
fn ddl_decorations(
    text: &str,
    marks: &[(String, LineMark)],
    theme: Theme,
) -> Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> {
    let mut decorations = Vec::new();
    let mut offset = 0usize;
    for (segment, (_, mark)) in text.split_inclusive('\n').zip(marks.iter()) {
        let range = offset..offset + segment.len();
        offset += segment.len();
        let style = match mark {
            LineMark::Same => None,
            LineMark::Add => Some(gpui::HighlightStyle {
                color: Some(rgb(theme.icon_table).into()),
                background_color: Some(rgba((theme.icon_table << 8) | 0x18).into()),
                ..Default::default()
            }),
            LineMark::Del => Some(gpui::HighlightStyle {
                color: Some(rgb(theme.danger).into()),
                background_color: Some(rgba((theme.danger << 8) | 0x18).into()),
                strikethrough: Some(gpui::StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(rgb(theme.danger).into()),
                }),
                ..Default::default()
            }),
        };
        if let Some(style) = style {
            decorations.push((range, style));
        }
    }
    decorations
}

/// The change list above the 结构比对 DDL panes.
///
/// Every line keeps its own height (`flex_none`): a flex column shrinks its children by default, so
/// a table with more changes than the 220px cap squeezed all 22 of them into the cap and clipped
/// each line to its top few pixels — the list looked like unreadable fragments and nothing could be
/// scrolled. The list scrolls instead, behind a real scrollbar, so the rows past the cap are
/// reachable. A table the target does not have gets one line: every column of it counts as
/// "added", and the DDL pane below already shows that definition.
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
    fn ddl_marks_ignore_the_auto_increment_counter() {
        let source = ") ENGINE=InnoDB AUTO_INCREMENT=103 DEFAULT CHARSET=utf8mb4;";
        let target = ") ENGINE=InnoDB AUTO_INCREMENT=108 DEFAULT CHARSET=utf8mb4;";
        let (source_lines, target_lines) = ddl_line_marks(source, target);
        assert_eq!(source_lines[0].1, LineMark::Same);
        assert_eq!(target_lines[0].1, LineMark::Same);

        // A real options difference is still marked.
        let changed = ") ENGINE=InnoDB AUTO_INCREMENT=103 DEFAULT CHARSET=latin1;";
        let (source_lines, _) = ddl_line_marks(source, changed);
        assert_eq!(source_lines[0].1, LineMark::Add);
    }

    #[test]
    fn ddl_decorations_cover_the_changed_lines() {
        let theme = Theme::dark();
        let (source_lines, target_lines) = ddl_line_marks("a\nb\n", "a\nc\n");
        let source = ddl_decorations("a\nb\n", &source_lines, theme);
        assert_eq!(source.len(), 1);
        assert_eq!(&"a\nb\n"[source[0].0.clone()], "b\n");
        let target = ddl_decorations("a\nc\n", &target_lines, theme);
        assert_eq!(target.len(), 1);
        assert_eq!(&"a\nc\n"[target[0].0.clone()], "c\n");
    }

    #[test]
    fn a_source_table_maps_into_the_selected_target_schema() {
        // PostgreSQL's public schema becomes SQL Server's dbo schema, not a literal `public`.
        assert_eq!(
            target_table_name(
                "public.users",
                &[],
                rustgrid_core::DriverDialect::SqlServer,
                "dbo",
                true,
            ),
            ("dbo.users".to_string(), false)
        );
        assert_eq!(
            target_table_name(
                "public.users",
                &["dbo.users".to_string()],
                rustgrid_core::DriverDialect::SqlServer,
                "dbo",
                true,
            ),
            ("dbo.users".to_string(), true)
        );
        // A schema-less target keeps its bare name.
        assert_eq!(
            target_table_name(
                "public.users",
                &["users".to_string()],
                rustgrid_core::DriverDialect::Mysql,
                "",
                false,
            ),
            ("users".to_string(), true)
        );
    }

    #[test]
    fn schema_selection_keeps_the_previous_value_or_picks_the_engine_default() {
        let schemas = vec!["archive".to_string(), "dbo".to_string()];
        assert_eq!(choose_schema("archive", &schemas, "dbo", true), "archive");
        assert_eq!(choose_schema("missing", &schemas, "dbo", true), "dbo");
        assert_eq!(choose_schema("", &schemas, "public", true), "archive");
        // An empty target database still maps into the engine's default schema.
        assert_eq!(choose_schema("", &[], "dbo", true), "dbo");
        assert_eq!(choose_schema("dbo", &schemas, "dbo", false), "");

        // Only the selected schema's tables take part in the comparison.
        assert!(in_schema("public.users", "public"));
        assert!(!in_schema("public.users", "dbo"));
        assert!(in_schema("users", ""));
    }

    #[test]
    fn numeric_keys_stream_but_text_keys_do_not() {
        let numeric = vec![rustgrid_core::ColumnInfo {
            name: "id".to_string(),
            data_type: "bigint".to_string(),
            nullable: false,
            primary_key: true,
            auto_increment: false,
            generated: false,
            comment: String::new(),
        }];
        assert!(streamable_keys(&numeric, &["id".to_string()]));

        let text = vec![rustgrid_core::ColumnInfo {
            name: "code".to_string(),
            data_type: "varchar(32)".to_string(),
            nullable: false,
            primary_key: true,
            auto_increment: false,
            generated: false,
            comment: String::new(),
        }];
        assert!(!streamable_keys(&text, &["code".to_string()]));
    }

    #[test]
    fn key_comparison_is_numeric_not_lexicographic() {
        let source = vec![CellValue::Int(10), CellValue::Text("x".to_string())];
        let target = vec![CellValue::Int(9), CellValue::Text("x".to_string())];
        assert_eq!(
            compare_key_rows(&source, &target, &[0], &[0]),
            CmpOrdering::Greater
        );
        assert_eq!(
            compare_key_rows(&source, &target, &[1], &[1]),
            CmpOrdering::Equal
        );
    }

    #[test]
    fn data_only_never_plans_a_missing_table() {
        let mut table = SyncTable::new("users".to_string(), "users".to_string(), false);
        table.comparing = false;
        table.source_schema = Some(TableSchema {
            columns: vec![rustgrid_core::ColumnDef {
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                primary_key: true,
                ..Default::default()
            }],
            ..Default::default()
        });
        table.data.key_columns = vec!["id".to_string()];

        // 仅数据 has no structure stage, so a missing target is not a data change. The table is
        // still not "identical" (the inspector keeps it visible with the "target missing" pill).
        assert!(!table.has_changes(SyncMode::Data, true));
        assert!(!table.is_identical(SyncMode::Data, true));
        // 全部 / 仅结构 / 全量传输 may create it from the mapped source definition.
        assert!(table.has_changes(SyncMode::All, true));
        assert!(table.has_changes(SyncMode::Schema, true));
        assert!(table.has_changes(SyncMode::Transfer, true));
    }

    #[test]
    fn generated_columns_are_pruned_from_the_diff() {
        let schema = TableSchema {
            columns: vec![
                rustgrid_core::ColumnDef {
                    name: "id".to_string(),
                    ..Default::default()
                },
                rustgrid_core::ColumnDef {
                    name: "total".to_string(),
                    generated: "price * qty".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut data = DataDiff {
            key_columns: vec!["id".to_string()],
            rows: vec![
                RowChange {
                    kind: RowChangeKind::Update,
                    keys: vec![("id".to_string(), Some("1".to_string()))],
                    values: vec![("total".to_string(), Some("10".to_string()))],
                    changes: vec![rustgrid_core::CellChange {
                        column: "total".to_string(),
                        before: Some("9".to_string()),
                        after: Some("10".to_string()),
                    }],
                },
                RowChange {
                    kind: RowChangeKind::Insert,
                    keys: vec![("id".to_string(), Some("2".to_string()))],
                    values: vec![
                        ("id".to_string(), Some("2".to_string())),
                        ("total".to_string(), Some("20".to_string())),
                    ],
                    changes: Vec::new(),
                },
            ],
            inserts: 1,
            updates: 1,
            ..Default::default()
        };
        prune_generated_changes(&mut data, &schema);
        assert_eq!(data.rows.len(), 1);
        assert_eq!(data.rows[0].kind, RowChangeKind::Insert);
        assert_eq!(data.rows[0].values.len(), 1);
        assert_eq!(data.inserts, 1);
        assert_eq!(data.updates, 0);
    }

    #[test]
    fn creation_order_follows_foreign_keys() {
        fn plan(name: &str, referenced: Option<&str>) -> SyncPlan {
            let mut schema = TableSchema::default();
            if let Some(referenced) = referenced {
                schema.foreign_keys.push(rustgrid_core::ForeignKeyDef {
                    name: format!("fk_{name}"),
                    columns: vec!["parent_id".to_string()],
                    referenced_table: referenced.to_string(),
                    referenced_columns: vec!["id".to_string()],
                    ..Default::default()
                });
            }
            SyncPlan {
                name: name.to_string(),
                target_name: name.to_string(),
                in_target: false,
                has_key: true,
                source_schema: Some(schema),
                target_schema: None,
                schema_changes: Vec::new(),
                data: DataDiff::default(),
                force_transfer: false,
                skip_write: Vec::new(),
            }
        }

        let ordered = order_plans_for_creation(
            vec![
                plan("orders", Some("customers")),
                plan("order_items", Some("orders")),
                plan("customers", None),
            ],
            SyncMode::Schema,
        );
        let names: Vec<&str> = ordered.iter().map(|plan| plan.name.as_str()).collect();
        assert_eq!(names, vec!["customers", "orders", "order_items"]);
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
