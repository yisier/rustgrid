use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, DispatchPhase, Div,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, FontWeight, HighlightStyle,
    ImageSource, KeyBinding, KeyDownEvent, ListHorizontalSizingBehavior, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Resource, ScrollDelta,
    ScrollHandle, ScrollStrategy, ScrollWheelEvent, SharedString, Stateful, StyledText,
    Subscription, Svg, TextLayout, TitlebarOptions, UTF16Selection, UniformListScrollHandle,
    WeakEntity, Window, WindowBounds, WindowControlArea, WindowHandle, WindowId, WindowOptions,
    canvas, deferred, div, img, prelude::*, px, rgb, rgba, size, svg, uniform_list,
};
use rustgrid_config::{
    AppSettings, ConfigStore, DEFAULT_EDITOR_FONT_SIZE, LanguageSetting, ThemeSetting,
};
use rustgrid_core::{
    BackupObjectKind, CellValue, Connection, ConnectionConfig, DatabaseEditorSpec,
    DatabaseEditorTab, DatabaseOptions, DefaultObjectType, DefaultPrivilege, DriverCapability,
    DriverIconStyle, DriverId, DriverRegistry, Error, FilterCondition, FilterConjunction,
    FilterGroup, FilterNode, FilterOperator, ObjectGrant, ObjectPrivilegeRow, PageRequest,
    PrivilegeCatalog, PrivilegeId, PrivilegePreset, PrivilegeScope, QueryResult, RoutineDetails,
    RoutineEdit, RoutineInfo, RoutineKind, RowInsert, RowUpdate, SavedBackup, SavedQuery,
    ServerSecurableGrant, TableStatus, TlsMode, TunnelAuth, TunnelKind, TunnelLayer, UserAccount,
    UserDetails, UserEdit, UserEditSection, UserEditorSpec, UserMapping, ViewEdit,
};
use rustgrid_export::ExportFormat;

use crate::form::{ConnectionForm, FORM_FIELDS, FormField, FormTab};
use crate::list_select::{ListSelection, MarqueeDrag, SelectMode, rects_intersect, selection_mode};
use crate::runtime::Runtime;
use crate::session::{
    Category, CategoryExpansion, CellRange, CellSelection, ConnectionNode, ConnectionStatus,
    DatabaseNode, EditAction, GridState, Loadable, QueryResultPlan, QueryResultSummary, QueryTab,
    RoutineTab, RoutineTabState, SortRule, ViewExplainTab, ViewTab, ViewTabState,
    compute_column_widths,
};
use crate::sql::{self, SqlToken};
use crate::theme::Theme;

// The in-place date/time picker is built from gpui-kit: the calendar supplies the date half
// (day/month/year views, navigation, selection and localization) and the `TimeField` the time
// half (segmented hours/minutes/seconds with keyboard editing).
use gpui_kit::component::Sizable;
use gpui_kit::component::calendar::CalendarState;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::time_field::{TimeField, TimeFieldState, TimePrecision};

use ui::{
    ButtonKind, ColumnGrid, ComboBox, ComboOption, DetailColumns, DetailScroll, TextInput,
    TextInputOptions, ViewMode, checkbox_box, main_separator, scrollbar_fractions, scrollbar_thumb,
    toolbar_separator,
};

// Cell-navigation actions for the in-place grid editor. gpui-kit's `Root` binds `tab`/`shift-tab`
// to focus traversal in the `"Root"` context, and key bindings run before `on_key_down`; binding
// the same keys in a deeper context (the editing cell) takes precedence and moves the editor.
// Up/Down are additionally bound to `GridCell > Input` because the kit's own `MoveUp`/`MoveDown`
// handlers consume those keys for a single-line field without propagating.
gpui::actions!(grid, [NextCell, PrevCell, MoveCellUp, MoveCellDown]);

// Backup-list actions. `Root` binds `ctrl-c` to a copy action in the `"Root"` context, which
// would swallow the keystroke before `on_key_down`; binding our own actions in a deeper context
// (the backup list) takes precedence, like the grid's Tab binding above.
gpui::actions!(backup, [CopyBackupFile, PasteBackupFile, RenameBackupFile]);

// Saved-query file-list actions, mirroring the backup list's F2 / Ctrl+C / Ctrl+V handling.
gpui::actions!(queryfile, [CopyQueryFile, PasteQueryFile, RenameQueryFile]);

// Query-editor actions. The editor's own undo/redo bindings handle Ctrl+Z; `RunSelectedQuery`
// is dispatched from the editor's right-click menu and Ctrl+Enter, and `SaveQuery` from Ctrl+S.
gpui::actions!(queryeditor, [RunSelectedQuery, SaveQuery]);

/// Key context applied to the cell that owns the in-place editor.
const GRID_CELL_CONTEXT: &str = "GridCell";

/// The in-place editor's own `Input` context as seen from the editing cell, for key bindings that
/// must outrank the kit's text-input bindings (up/down cell movement).
const GRID_CELL_INPUT_CONTEXT: &str = "GridCell > Input";

/// Key context applied to the backup list, so F2 / Ctrl+C / Ctrl+V reach it.
const BACKUP_LIST_CONTEXT: &str = "BackupList";

/// Key context applied to the saved-query file list.
const QUERY_LIST_CONTEXT: &str = "QueryList";

/// Key context applied to the SQL query editor, so Ctrl+Z / Ctrl+Y / Ctrl+S reach it.
const QUERY_EDITOR_CONTEXT: &str = "QueryEditor";

/// The stable settings key for the Queries tab's remembered list layout.
pub(super) const VIEW_PAGE_QUERIES: &str = "queries";

/// The stable settings key for the Users tab's remembered list layout.
pub(super) const VIEW_PAGE_USERS: &str = "users";

/// The stable settings key for the Backup tab's remembered list layout.
pub(super) const VIEW_PAGE_BACKUPS: &str = "backups";

/// The stable settings key for the Tables object list's remembered layout.
pub(super) const VIEW_PAGE_TABLES: &str = "tables";

/// The stable settings key for the Views object list's remembered layout.
pub(super) const VIEW_PAGE_VIEWS: &str = "views";

/// The stable settings key for the Functions object list's remembered layout.
pub(super) const VIEW_PAGE_FUNCTIONS: &str = "functions";

/// The ten text inputs of the connection form, created when the form opens. Order follows
/// [`FORM_FIELDS`] so `FormField as usize` indexes the array.
struct FormInputs {
    fields: [Entity<TextInput>; 10],
}

impl FormInputs {
    fn get(&self, field: FormField) -> &Entity<TextInput> {
        &self.fields[field as usize]
    }
}

/// Build one connection-form field. Edits write straight into `ConnectionForm` so the dialog
/// title and the save/test paths always see the latest text without reading the entity back
/// (which would re-enter a borrowed entity during the change callback).
fn make_form_input(
    theme: Theme,
    value: String,
    masked: bool,
    field: FormField,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    let submit = app.clone();
    let tab = app.clone();
    cx.new(move |cx| {
        TextInput::new(
            theme,
            value,
            TextInputOptions {
                masked,
                placeholder: form_field_placeholder(field).into(),
                size: Some(gpui_kit::component::Size::Medium),
                text_size: Some(13.0),
                accepts: None,
                ..Default::default()
            },
            cx,
        )
        .on_change(Rc::new(move |text, _window, cx| {
            let _ = change.update(cx, |app, cx| app.set_form_field(field, text, cx));
        }))
        .on_submit(Rc::new(move |_window, cx| {
            let _ = submit.update(cx, |app, cx| app.save_form(cx));
        }))
        .on_tab(Rc::new(move |shift, window, cx| {
            let _ = tab.update(cx, |app, cx| {
                if let Some(handle) = app.form_neighbor(field, shift, cx) {
                    window.focus(&handle, cx);
                }
            });
        }))
    })
}

/// The placeholder of one connection-form field.
fn form_field_placeholder(field: FormField) -> String {
    match field {
        FormField::Name => t!("form.alias_placeholder").to_string(),
        FormField::Password => t!("form.password_placeholder").to_string(),
        FormField::Database => t!("form.database_placeholder").to_string(),
        FormField::Host
        | FormField::Port
        | FormField::Username
        | FormField::OdbcDriver
        | FormField::OdbcDsn
        | FormField::OdbcConnectionString
        | FormField::OdbcEngine => String::new(),
    }
}

struct PasswordPrompt {
    index: usize,
    input: Entity<TextInput>,
    password: String,
    save_password: bool,
}

/// State of the "save query" dialog: the tab being saved, its (editable) name, the chosen save
/// location, and an inline validation error.
struct SaveQueryDialog {
    /// Index into `AppView::queries` of the tab being saved.
    tab_index: usize,
    name: String,
    /// The selected connection, indexed into `AppView::connections`.
    connection_index: Option<usize>,
    database: String,
    /// The schema to file the query under, when the query's run target has one selected.
    schema: Option<String>,
    error: Option<String>,
}

/// State of the "new table" name prompt, shown when saving a brand-new table designer.
struct CreateTableDialog {
    /// `TableDesignView::id` of the designer that requested the name.
    design_id: u64,
    name: String,
    error: Option<String>,
}

/// Which tab of a backup dialog is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BackupDialogTab {
    Objects,
    Log,
}

/// One object row in a backup or restore dialog's object selection.
#[derive(Clone)]
struct BackupObjectEntry {
    kind: BackupObjectKind,
    name: String,
    selected: bool,
}

/// A backup file discovered under the config dir's `backups/` tree.
struct BackupFileInfo {
    path: std::path::PathBuf,
    /// The file stem, e.g. `20260919225552`.
    name: String,
    /// The `ConnectionProfile::id` whose folder holds the file.
    connection_id: String,
    /// The database folder holding the file.
    database: String,
    manifest: rustgrid_backup::BackupManifest,
    size: u64,
    modified: Option<std::time::SystemTime>,
}

/// Which entry of the backup list is selected.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BackupSelection {
    File(usize),
    Config(usize),
}

/// An in-app copy of a backup file, so Ctrl+C / Ctrl+V works like copying a file in Explorer.
/// Only the source path is held (the bytes are read on paste), keeping memory bounded.
#[derive(Clone)]
struct BackupClipboard {
    /// The file stem (without the extension) at copy time.
    name: String,
    /// The copied file's path.
    path: std::path::PathBuf,
}

/// A saved-query `.sql` file discovered under the config dir's `queries/` tree.
struct QueryFileInfo {
    path: std::path::PathBuf,
    /// The file stem, e.g. `清理库存`.
    name: String,
    /// The connection folder holding the file.
    connection_id: String,
    /// The database folder holding the file.
    database: String,
    /// The schema folder holding the file, when the query was filed under one.
    schema: Option<String>,
    size: u64,
    created: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
}

/// A saved-query column the Queries 详细列表 can sort by.
#[derive(Clone, Copy, PartialEq, Eq)]
enum QuerySortColumn {
    Name,
    Modified,
    Size,
}

/// The Queries list's sort: which column and whether descending.
#[derive(Clone, Copy)]
struct QuerySort {
    column: QuerySortColumn,
    descending: bool,
}

impl Default for QuerySort {
    fn default() -> Self {
        Self {
            column: QuerySortColumn::Name,
            descending: false,
        }
    }
}

/// An in-app copy of a query file, so Ctrl+C / Ctrl+V works like copying a file in Explorer.
#[derive(Clone)]
struct QueryClipboard {
    /// The file stem (without the extension) at copy time.
    name: String,
    /// The copied file's path.
    path: std::path::PathBuf,
}

/// The in-place "rename query" editor, drawn in the row it started from.
struct QueryRenameEdit {
    /// Index into `AppView::query_files`.
    index: usize,
    old_name: String,
    new_name: String,
    input: Entity<TextInput>,
}

/// The in-place "rename backup" editor, drawn in the row it started from. `new_name` mirrors the
/// input's text so `submit_backup_rename` never reads the entity back during its change callback.
struct BackupRenameEdit {
    /// Which list entry is being renamed (a backup file or a saved configuration).
    target: BackupSelection,
    old_name: String,
    new_name: String,
    input: Entity<TextInput>,
}

/// State of the "New Backup" dialog.
struct NewBackupDialog {
    connection_index: usize,
    database: String,
    tab: BackupDialogTab,
    objects: Vec<BackupObjectEntry>,
    /// Name the configuration is saved under by the *Save* button.
    config_name: String,
    /// A saved configuration to apply to the object selection once it has finished loading.
    apply: Option<SavedBackup>,
    /// Whether the dialog is editing an existing saved configuration (drives the window title).
    /// Kept after `apply` is consumed so the title stays "Edit Backup".
    editing: bool,
    loading: bool,
    running: bool,
    /// Set by 停止 to abort the run loop early (checked between objects/rows).
    cancel: Arc<std::sync::atomic::AtomicBool>,
    log: Vec<String>,
    /// Keeps the info log scrolled to the newest line while the operation runs.
    log_scroll: ScrollHandle,
    /// Progress counters for the run's summary.
    total: usize,
    success: usize,
    failed: usize,
    /// Records: the planned total (0 when unknown, e.g. a backup) and processed so far.
    rows_total: usize,
    rows_done: usize,
    /// When the run started, and its final elapsed time.
    started: Option<std::time::Instant>,
    elapsed: Option<std::time::Duration>,
    error: Option<String>,
    /// Set after the Save button writes the configuration, so the footer can confirm it. Cleared
    /// when the name changes or a run starts.
    config_saved: bool,
}

/// State of the "Restore Backup" dialog.
struct RestoreBackupDialog {
    /// Index into `AppView::backup_files`.
    file_index: usize,
    connection_index: Option<usize>,
    database: String,
    tab: BackupDialogTab,
    objects: Vec<BackupObjectEntry>,
    running: bool,
    /// Set by 停止 to abort the run loop early (checked between objects/rows).
    cancel: Arc<std::sync::atomic::AtomicBool>,
    log: Vec<String>,
    /// Keeps the info log scrolled to the newest line while the operation runs.
    log_scroll: ScrollHandle,
    /// Progress counters for the run's summary.
    total: usize,
    success: usize,
    failed: usize,
    /// Records: the planned total (sum of the manifest's row counts) and processed so far.
    rows_total: usize,
    rows_done: usize,
    /// When the run started, and its final elapsed time.
    started: Option<std::time::Instant>,
    elapsed: Option<std::time::Duration>,
    error: Option<String>,
}

/// State of the "Extract SQL" dialog (mirrors the restore dialog, plus the chosen output file).
struct ExtractSqlDialog {
    /// Index into `AppView::backup_files`.
    file_index: usize,
    tab: BackupDialogTab,
    objects: Vec<BackupObjectEntry>,
    /// The `.sql` file the extract writes to, chosen through the system save dialog.
    output_path: Option<std::path::PathBuf>,
    running: bool,
    /// Set by 停止 to abort the run loop early (checked between objects).
    cancel: Arc<std::sync::atomic::AtomicBool>,
    log: Vec<String>,
    /// Keeps the info log scrolled to the newest line while the operation runs.
    log_scroll: ScrollHandle,
    /// Progress counters for the run's summary.
    total: usize,
    success: usize,
    failed: usize,
    /// Records: the planned total (sum of the manifest's row counts) and processed so far.
    rows_total: usize,
    rows_done: usize,
    /// When the run started, and its final elapsed time.
    started: Option<std::time::Instant>,
    elapsed: Option<std::time::Duration>,
    error: Option<String>,
}

/// Which page of the export wizard (P1..P4) is showing. P4 merges Navicat's options page with the
/// run/log page, so the wizard is four steps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExportStep {
    Format,
    Tables,
    Fields,
    Options,
}

impl ExportStep {
    /// The 1-based page number and the total, for the page heading.
    fn index(self) -> (usize, usize) {
        match self {
            ExportStep::Format => (1, 4),
            ExportStep::Tables => (2, 4),
            ExportStep::Fields => (3, 4),
            ExportStep::Options => (4, 4),
        }
    }
}

/// One table row of the export wizard's P2 list.
struct ExportTablePlan {
    name: String,
    selected: bool,
    /// The full output path for this table.
    path: String,
}

/// One table's resolved export run: the output path, the selected columns (names and engine types)
/// and the primary-key ordering used for paged reads.
struct ExportPlan {
    name: String,
    path: String,
    columns: Vec<String>,
    column_types: Vec<String>,
    order_by: Vec<rustgrid_core::SortColumn>,
}

/// The P3 field selection of one table.
struct ExportFields {
    /// Every column of the table, in catalog order.
    columns: Vec<String>,
    /// Column engine types, parallel to `columns`; the Oracle `.sql` writer wraps temporal values.
    data_types: Vec<String>,
    /// Whether each column is exported; parallel to `columns`.
    selected: Vec<bool>,
    /// The table's primary-key columns, used to order the export's paged reads.
    primary_key: Vec<String>,
    loading: bool,
    error: Option<String>,
}

impl ExportFields {
    /// A freshly loaded table: every column selected.
    fn all(columns: Vec<rustgrid_core::ColumnInfo>) -> Self {
        let primary_key = columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| column.name.clone())
            .collect();
        let data_types = columns
            .iter()
            .map(|column| column.data_type.clone())
            .collect();
        Self {
            selected: vec![true; columns.len()],
            columns: columns.into_iter().map(|column| column.name).collect(),
            data_types,
            primary_key,
            loading: false,
            error: None,
        }
    }

    /// The exported column names, in catalog order.
    fn selected_columns(&self) -> Vec<String> {
        self.columns
            .iter()
            .zip(&self.selected)
            .filter(|(_, selected)| **selected)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The exported column types, parallel to [`Self::selected_columns`].
    fn selected_column_types(&self) -> Vec<String> {
        self.data_types
            .iter()
            .zip(&self.selected)
            .filter(|(_, selected)| **selected)
            .map(|(data_type, _)| data_type.clone())
            .collect()
    }

    fn all_selected(&self) -> bool {
        !self.selected.is_empty() && self.selected.iter().all(|value| *value)
    }
}

/// State of the "Export Wizard" window.
struct ExportWizard {
    connection_index: usize,
    database: String,
    step: ExportStep,
    format: ExportFormat,
    /// The directory new output paths default into; editable on P2.
    output_dir: String,
    tables: Vec<ExportTablePlan>,
    /// P3's available fields per selected table, keyed by table name.
    fields: BTreeMap<String, ExportFields>,
    /// The table whose fields P3 shows, as an index into `tables`.
    field_table: usize,
    /// The P3 source-table dropdown (drawn only when more than one table is selected).
    field_combo: Option<Entity<ComboBox>>,
    /// The P2 output-directory field.
    dir_input: Option<Entity<TextInput>>,
    include_header: bool,
    continue_on_error: bool,
    running: bool,
    /// Set by 停止 to abort the run loop early (checked between tables/pages).
    cancel: Arc<std::sync::atomic::AtomicBool>,
    log: Vec<String>,
    log_scroll: ScrollHandle,
    rows_total: usize,
    rows_done: usize,
    started: Option<std::time::Instant>,
    elapsed: Option<std::time::Duration>,
    error: Option<String>,
}

/// The source kinds offered by the import wizard's first page: Excel workbooks (one table per
/// worksheet), and comma/tab delimited text (one table named after the file).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportFormat {
    Excel,
    Csv,
    Text,
}

impl ImportFormat {
    /// Every kind, in the order the wizard presents them.
    const ALL: [ImportFormat; 3] = [ImportFormat::Excel, ImportFormat::Csv, ImportFormat::Text];

    /// The i18n key for the kind's label.
    fn label_key(self) -> &'static str {
        match self {
            ImportFormat::Excel => "import.format.xlsx",
            ImportFormat::Csv => "import.format.csv",
            ImportFormat::Text => "import.format.txt",
        }
    }

    /// The reader this kind uses.
    fn source_kind(self) -> rustgrid_import::SourceKind {
        match self {
            ImportFormat::Excel => rustgrid_import::SourceKind::Excel,
            ImportFormat::Csv => rustgrid_import::SourceKind::Csv,
            ImportFormat::Text => rustgrid_import::SourceKind::Text,
        }
    }
}

/// Which page of the import wizard is showing: choose a kind (P1), pick the file and its target
/// tables (P2, Navicat's P2+P3 merged), map fields (P3), then run (P4).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportStep {
    Format,
    Source,
    Mapping,
    Run,
}

impl ImportStep {
    /// The 1-based page number and the total, for the page heading.
    fn index(self) -> (usize, usize) {
        match self {
            ImportStep::Format => (1, 4),
            ImportStep::Source => (2, 4),
            ImportStep::Mapping => (3, 4),
            ImportStep::Run => (4, 4),
        }
    }
}

/// One worksheet row of the import wizard's P2 list.
struct ImportSheetPlan {
    /// The worksheet name.
    name: String,
    /// Whether this sheet is imported.
    selected: bool,
    /// The destination table name, editable on P2.
    target: String,
    /// Whether the destination table is created before inserting (auto-set when no table with the
    /// sheet's name exists).
    create: bool,
    /// The editable destination-table field.
    target_input: Option<Entity<TextInput>>,
}

/// The loaded source/destination columns and rows of one worksheet, shared by the mapping page and
/// the run.
struct ImportSheetFields {
    /// The worksheet's column names, in order.
    source: Vec<String>,
    /// The worksheet's data rows, read once when the sheet loads.
    rows: Vec<Vec<Option<String>>>,
    /// Every destination column: the existing table's columns, or the source names for a table the
    /// wizard creates.
    target_columns: Vec<String>,
    /// The destination column's type, parallel to `target_columns`, used to normalise date/time
    /// values before insert.
    target_types: Vec<String>,
    /// The destination column each source column maps to; parallel to `source`. Empty means the
    /// source column is not imported.
    mapped: Vec<String>,
    /// The destination table's primary-key columns.
    primary_key: Vec<String>,
    loading: bool,
    error: Option<String>,
}

impl ImportSheetFields {
    fn loading() -> Self {
        Self {
            source: Vec::new(),
            rows: Vec::new(),
            target_columns: Vec::new(),
            target_types: Vec::new(),
            mapped: Vec::new(),
            primary_key: Vec::new(),
            loading: true,
            error: None,
        }
    }

    /// The `(source column index, destination column, destination type)` triples to import.
    fn mapping_columns(&self) -> Vec<(usize, String, String)> {
        self.mapped
            .iter()
            .enumerate()
            .filter(|(_, target)| !target.is_empty())
            .map(|(index, target)| {
                let data_type = self
                    .target_columns
                    .iter()
                    .position(|column| column == target)
                    .and_then(|position| self.target_types.get(position))
                    .cloned()
                    .unwrap_or_default();
                (index, target.clone(), data_type)
            })
            .collect()
    }
}

/// One row of the mapping page: a source column and its destination-column dropdown.
struct ImportMappingRow {
    source: String,
    combo: Entity<ComboBox>,
}

/// State of the "Import Wizard" window.
struct ImportWizard {
    connection_index: usize,
    database_index: usize,
    database: String,
    step: ImportStep,
    format: ImportFormat,
    /// The chosen source file, empty until picked.
    file: String,
    /// The source-file field on P2.
    file_input: Option<Entity<TextInput>>,
    /// One row per worksheet in the file.
    sheets: Vec<ImportSheetPlan>,
    /// Whether the chosen source file is being read (worksheet list loading).
    loading_sheets: bool,
    /// When the wizard was opened from a table grid, the destination table a single source table
    /// should default to (instead of the source name).
    target_table: Option<String>,
    /// The database's table names at load time, so a typed target can auto-toggle "create".
    existing_tables: Vec<String>,
    /// Loaded source/destination data per sheet, keyed by sheet name.
    fields: BTreeMap<String, ImportSheetFields>,
    /// The sheet whose mapping P3 shows, as an index into `sheets`.
    mapping_sheet: usize,
    /// The P3 source-sheet dropdown (drawn when more than one sheet is selected).
    sheet_combo: Option<Entity<ComboBox>>,
    /// The P3 destination-column dropdowns, keyed by sheet name and built for every selected sheet
    /// up front, so switching sheets never creates entities during a combo callback.
    mapping_rows: BTreeMap<String, Vec<ImportMappingRow>>,
    running: bool,
    /// Set by 停止 to abort the run loop early (checked between sheets/batches).
    cancel: Arc<std::sync::atomic::AtomicBool>,
    log: Vec<String>,
    log_scroll: ScrollHandle,
    rows_total: usize,
    rows_done: usize,
    imported_tables: usize,
    added: usize,
    updated: usize,
    deleted: usize,
    errors: usize,
    started: Option<std::time::Instant>,
    elapsed: Option<std::time::Duration>,
    error: Option<String>,
}

/// Which pane's row owns the in-place rename editor. The object list and the connection tree can
/// both list the same table, so the editor is drawn in exactly one of them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowPane {
    Objects,
    Tree,
}

/// Which flat list a marquee drag (or its row rectangles) belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MarqueeTarget {
    Users,
    Backups,
    Queries,
    Objects,
}

/// The "rename table" in-place editor. `new_name` mirrors the input's text so `submit_rename`
/// never reads the entity back during its own change callback. `AppView` owns the state (both
/// panes render `input` in the row named by [`RowPane`]) so the two lists share one commit path.
struct RenameEdit {
    pane: RowPane,
    connection_index: usize,
    database_index: usize,
    old_name: String,
    new_name: String,
    input: Entity<TextInput>,
}

/// What a pane needs to draw the editor in the right row; `None` while the editor belongs to the
/// other pane.
#[derive(Clone)]
struct RenameRow {
    connection_index: usize,
    database_index: usize,
    old_name: String,
    input: Entity<TextInput>,
}

/// Build the masked password field for the password prompt. The plaintext mirrors into
/// `PasswordPrompt::password` so `submit_password` never reads the entity back during its own
/// change callback.
fn make_password_input(
    theme: Theme,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    let submit = app.clone();
    cx.new(move |cx| {
        TextInput::new(
            theme,
            "",
            TextInputOptions {
                masked: true,
                placeholder: SharedString::default(),
                accepts: None,
                ..Default::default()
            },
            cx,
        )
        .on_change(Rc::new(move |text, _window, cx| {
            let _ = change.update(cx, |app, cx| {
                if let Some(prompt) = app.password_prompt.as_mut() {
                    prompt.password = text.to_string();
                }
                cx.notify();
            });
        }))
        .on_submit(Rc::new(move |_window, cx| {
            let _ = submit.update(cx, |app, cx| app.submit_password(cx));
        }))
    })
}

/// Build the database-name field of the "new database" dialog.
fn make_db_name_input(
    theme: Theme,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    let submit = app.clone();
    cx.new(move |cx| {
        TextInput::new(theme, "", TextInputOptions::default(), cx)
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| {
                    if let Some(DbDialog::Edit(form)) = app.db_dialog.as_mut()
                        && form.database_index.is_none()
                    {
                        form.name = text.to_string();
                    }
                    cx.notify();
                });
            }))
            .on_submit(Rc::new(move |_window, cx| {
                let _ = submit.update(cx, |app, cx| app.db_submit(cx));
            }))
            .on_cancel(Rc::new({
                let cancel = app.clone();
                move |_window, cx| {
                    let _ = cancel.update(cx, |app, cx| {
                        app.db_dialog = None;
                        app.db_name_input = None;
                        cx.notify();
                    });
                }
            }))
    })
}

/// Build the name field of the "new schema" dialog.
fn make_schema_name_input(
    theme: Theme,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    let submit = app.clone();
    let options = TextInputOptions {
        placeholder: t!("database.schema_name_placeholder").to_string().into(),
        ..Default::default()
    };
    cx.new(move |cx| {
        TextInput::new(theme, "", options, cx)
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| app.schema_name_changed(text, cx));
            }))
            .on_submit(Rc::new(move |_window, cx| {
                let _ = submit.update(cx, |app, cx| app.schema_submit(cx));
            }))
            .on_cancel(Rc::new({
                let cancel = app.clone();
                move |_window, cx| {
                    let _ = cancel.update(cx, |app, cx| app.schema_cancel(cx));
                }
            }))
    })
}

/// Build the name field of the "new table" prompt, pre-filled with `initial`.
fn make_create_table_input(
    theme: Theme,
    initial: String,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    let submit = app.clone();
    cx.new(move |cx| {
        TextInput::new(theme, initial, TextInputOptions::default(), cx)
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| {
                    if let Some(dialog) = app.create_table_dialog.as_mut() {
                        dialog.name = text.to_string();
                    }
                    cx.notify();
                });
            }))
            .on_submit(Rc::new(move |_window, cx| {
                let _ = submit.update(cx, |app, cx| app.submit_create_table(cx));
            }))
            .on_cancel(Rc::new({
                let cancel = app.clone();
                move |_window, cx| {
                    let _ = cancel.update(cx, |app, cx| {
                        app.create_table_dialog = None;
                        app.create_table_input = None;
                        cx.notify();
                    });
                }
            }))
    })
}

enum TestStatus {
    Idle,
    Testing,
    Success,
    Failed(String),
}

enum ContextTarget {
    Connection(usize),
    Database {
        connection_index: usize,
        database_index: usize,
    },
    Table {
        connection_index: usize,
        database_index: usize,
        name: String,
        is_view: bool,
        /// Which pane the row was right-clicked in, so Rename edits the name in place there.
        pane: RowPane,
    },
    /// The Tables category node in the connection tree (新建表 / 导入向导 / 导出向导 / 刷新).
    TableCategory {
        connection_index: usize,
        database_index: usize,
        schema: Option<String>,
    },
    /// A non-Tables category node in the connection tree (刷新).
    ObjectCategory {
        connection_index: usize,
        database_index: usize,
        category: Category,
    },
    /// A stored routine in the connection tree's Functions category.
    Routine {
        connection_index: usize,
        database_index: usize,
        name: String,
        kind: RoutineKind,
    },
    /// The Functions toolbar's "New" menu: pick a function or a procedure.
    NewRoutine {
        connection_index: usize,
        database_index: usize,
    },
    /// A schema node under a database (SQL Server).
    Schema {
        connection_index: usize,
        database_index: usize,
        schema: String,
    },
    /// A backup file in the Backup main tab's list.
    BackupFile {
        index: usize,
    },
    /// A saved backup profile in the Backup main tab's list.
    BackupConfig {
        index: usize,
    },
    /// A saved-query `.sql` file in the Queries main tab's list.
    QueryFile {
        index: usize,
    },
    /// The blank area of the Backup main tab's list.
    BackupList,
    /// The blank area of the Queries main tab's list.
    QueryList,
    /// A data/result grid (right-click → 删除记录 / 复制 / 复制为 INSERT / 粘贴 / 刷新).
    Grid {
        grid_id: u64,
    },
}

struct ContextMenu {
    target: ContextTarget,
    position: Point<Pixels>,
}

#[derive(Clone, Copy)]
enum TabTarget {
    Grid(usize),
    Query(usize),
    Design(usize),
}

struct TabMenu {
    target: TabTarget,
    position: Point<Pixels>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DbTab {
    General,
    /// Informational sub-tabs shown only by engines that declare them (SQL Server).
    Extra(DatabaseEditorTab),
    Sql,
}

/// The editable fields shared by the "New Database" and "Edit Database" dialogs. `database_index`
/// is `None` while creating (so the name field is editable) and `Some` while editing an existing
/// database (name read-only, SQL preview shows the `ALTER`). The `original_*` fields hold the
/// engine-loaded values, so the SQL preview and the apply path can diff.
struct DatabaseForm {
    connection_index: usize,
    database_index: Option<usize>,
    name: String,
    original_charset: String,
    original_collation: String,
    original_owner: String,
    original_recovery_model: String,
    original_compatibility_level: String,
    charset: String,
    collation: String,
    owner: String,
    recovery_model: String,
    compatibility_level: String,
    charsets: Vec<String>,
    collations: Vec<String>,
    owners: Vec<String>,
    /// Which fields the engine's `DatabaseEditorSpec` enables for this dialog.
    spec: DatabaseEditorSpec,
    tab: DbTab,
    loading: bool,
    error: Option<String>,
}

impl DatabaseForm {
    /// The current editable options, as the engine-agnostic model.
    fn options(&self) -> DatabaseOptions {
        DatabaseOptions {
            charset: self.charset.clone(),
            collation: self.collation.clone(),
            owner: self.owner.clone(),
            recovery_model: self.recovery_model.clone(),
            compatibility_level: self.compatibility_level.clone(),
        }
    }

    /// The engine-loaded options, as the engine-agnostic model.
    fn original_options(&self) -> DatabaseOptions {
        DatabaseOptions {
            charset: self.original_charset.clone(),
            collation: self.original_collation.clone(),
            owner: self.original_owner.clone(),
            recovery_model: self.original_recovery_model.clone(),
            compatibility_level: self.original_compatibility_level.clone(),
        }
    }
}

enum DbDialog {
    Edit(Box<DatabaseForm>),
    Delete {
        connection_index: usize,
        name: String,
        /// True while the drop request is in flight, so a second OK click is ignored.
        submitting: bool,
        error: Option<String>,
    },
}

/// The "New Schema" dialog (SQL Server): a name field over one (connection, database) scope.
struct SchemaDialog {
    connection_index: usize,
    database_index: usize,
    name: String,
    submitting: bool,
    error: Option<String>,
}

/// The Tables/Views object browser for one database. Owns its own list state and renders the
/// list body independently of `AppView`; the surrounding toolbar/search stays on `AppView`.
struct ObjectPane {
    app: WeakEntity<AppView>,
    connection_index: usize,
    database_index: usize,
    /// The schema the list is scoped to (SQL Server), or `None` for the whole database.
    schema: Option<String>,
    category: Category,
    selected: Option<String>,
    /// The kind of the selected routine, when `category` is [`Category::Functions`].
    selected_routine: Option<RoutineKind>,
    /// The column-major 平铺网格's horizontal scroll state.
    grid: ColumnGrid,
    /// The 详细列表's scroll state: rows scroll vertically, the header and rows together scroll
    /// horizontally.
    detail_scroll: DetailScroll,
    /// The Tables 详细列表's content-fitted / user-resized column widths.
    table_columns: Rc<RefCell<DetailColumns>>,
    /// The Functions 详细列表's content-fitted / user-resized column widths.
    routine_columns: Rc<RefCell<DetailColumns>>,
    /// The visible row keys of the last render, in display order (Shift-extend / marquee base).
    visible_keys: Vec<String>,
    /// Focus target for the list, so F2 reaches [`AppView::begin_rename_table`].
    focus: FocusHandle,
    theme: Theme,
}

/// The tab strip above the content pane (Object / tables / queries). Owns its own horizontal
/// scroll state and renders independently; tab actions are dispatched to `AppView`.
struct TabBar {
    app: WeakEntity<AppView>,
    scroll: ScrollHandle,
}

/// The left connection tree (connections -> databases -> categories -> tables). Owns the tree
/// selection and vertical scroll; renders from a lightweight snapshot of `AppView`'s
/// connections and dispatches tree actions back to `AppView`.
struct TreePane {
    app: WeakEntity<AppView>,
    scroll: ScrollHandle,
    selected: Option<String>,
    /// The selected table row (`connection_index`, `database_index`, name, is_view), so F2 knows
    /// what to rename. `None` when the selection is a connection/database/category row.
    selected_table: Option<(usize, usize, String, bool)>,
    /// The in-place rename editor to draw in the matching row this frame, if it belongs here.
    rename_row: Option<RenameRow>,
    /// Focus target for the tree, so F2 reaches [`AppView::begin_rename_table`].
    focus: FocusHandle,
    theme: Theme,
    /// The ids of the currently visible nodes, in draw order, for keyboard navigation.
    visible_ids: Vec<String>,
}

/// Which text field of a grid owns the platform text input (IME / `WM_CHAR`). Grids keep several
/// bespoke single-line editors whose focus handles are dynamic (filter values in particular), so
/// rather than wrapping each in `TextInput`, `GridView` implements `EntityInputHandler` itself
/// and routes input to whichever field is focused.
#[derive(Clone, PartialEq, Eq)]
enum GridTextField {
    PageInput,
    PageSize,
    /// A filter value input, addressed by its node path in the filter tree and which value slot.
    Filter(Vec<usize>, u8),
}

/// A single open grid (a table page or a SQL result) as an isolated child view. Owns the grid
/// data (`GridState`) and every piece of interaction state (scroll handles, cell editor, date
/// picker, sort popup, page inputs), so grid-local interactions re-render only this view.
///
/// App-level overlays that must cover the whole window (delete confirmation, error dialog) stay
/// on `AppView`; this view asks for them through `WeakEntity<AppView>`.
struct GridView {
    state: GridState,
    app: WeakEntity<AppView>,
    runtime: Arc<Runtime>,
    theme: Theme,

    hscroll: ScrollHandle,
    hscroll_grab: Option<f32>,
    list_scroll: UniformListScrollHandle,
    vscroll_grab: Option<f32>,
    focus: FocusHandle,
    selecting_cells: bool,
    /// The cell pressed on mouse-down; if the button is released without dragging to another
    /// cell, a single click opens its editor.
    cell_press: Option<(usize, usize)>,
    cell_dragged: bool,

    cell_editor: Option<CellEditor>,
    cell_editor_blur_subscription: Option<Subscription>,
    cell_editor_focus_pending: bool,
    date_picker: Option<DatePicker>,

    /// Pending rows added with the "+" button that have not been written to the database yet.
    /// They render after `state.rows` (gutter `*`) and are flushed by Save. Each row maps a
    /// column index to its staged value: `None` is an explicit SQL `NULL`, an absent entry means
    /// the column is left to its default.
    inserts: Vec<BTreeMap<usize, Option<String>>>,

    /// The column edge being dragged, with the pointer x and width captured on mouse-down.
    column_resize: Option<ColumnResize>,

    sort_hover: Option<usize>,
    /// The shared column dropdowns of the sort panel, keyed by draft rule index.
    sort_field_combos: BTreeMap<usize, Entity<ComboBox>>,
    /// Rows copied with Ctrl+C / the context menu's 复制, for 粘贴 to write into the selected
    /// records. Each cell is `None` for SQL `NULL`.
    clipboard: Option<Vec<Vec<Option<String>>>>,
    /// The exact text last written to the OS clipboard by a cell copy, so paste can tell an
    /// external clipboard change from our own and fall back to `clipboard` (which keeps NULLs).
    clipboard_text: Option<String>,

    filter_value_focus: Vec<(Vec<usize>, FocusHandle)>,
    filter_value2_focus: Vec<(Vec<usize>, FocusHandle)>,
    filter_active: Option<(Vec<usize>, u8)>,
    /// The shared dropdown entities for each condition row, keyed by the node path. Rebuilt after
    /// any structural change so the paths stay valid.
    filter_field_combos: BTreeMap<Vec<usize>, Entity<ComboBox>>,
    filter_operator_combos: BTreeMap<Vec<usize>, Entity<ComboBox>>,
    /// Width of the filter-condition block. `None` fills the pane (responsive); `Some` is a width
    /// pinned by dragging the divider on its right edge.
    filter_width: Option<f32>,
    /// The pane width available to the filter list, measured each frame; drives the responsive
    /// `None` width.
    filter_available_width: f32,
    /// The block's last rendered width, used as the anchor when a drag starts.
    filter_rendered_width: f32,
    /// `(pointer x, width)` captured when the filter-width drag started.
    filter_resize: Option<(f32, f32)>,

    page_input: String,
    page_input_focus: FocusHandle,
    page_input_focused: bool,
    page_size_menu_open: bool,
    page_size_input: String,
    page_size_focus: FocusHandle,
    page_size_focus_pending: bool,
    page_size_focused: bool,

    caret_visible: bool,
    caret_blink_running: bool,

    /// A weak handle to this view, used to register the input handler from a paint callback
    /// without forming an `Entity` reference cycle.
    self_weak: WeakEntity<GridView>,
    /// The field the platform IME is currently driving and its composing byte range.
    ime_field: Option<GridTextField>,
    ime_marked: Option<std::ops::Range<usize>>,
    /// Cache of the status bar's numeric selection sum, keyed by a cheap state signature, so a
    /// huge selection is not re-summed on every render.
    sum_cache: RefCell<Option<(u64, Option<String>)>>,
}

/// The in-place editor over one grid cell. The text itself lives in a shared [`TextInput`] entity
/// (so caret rendering, selection, clipboard and IME are the same as every other field); `value`
/// mirrors its text because a multi-cell edit previews the same string in every selected cell.
struct CellEditor {
    row: usize,
    col: usize,
    cells: Vec<(usize, usize)>,
    value: String,
    input: Entity<TextInput>,
}

/// Column metadata for a referenced table, loaded on demand by the query editor's completion.
enum ColumnCacheEntry {
    Loading,
    Loaded(Vec<rustgrid_core::ColumnInfo>),
    Failed,
}

/// Where a pointer position lands inside the data grid.
#[derive(Clone, Copy)]
enum GridHit {
    Gutter(usize),
    Cell(usize, usize),
}

struct DatePicker {
    row: usize,
    col: usize,
    /// Whether the column carries a time of day (`datetime`/`timestamp`).
    has_time: bool,
    /// The date half, owned by gpui-kit's calendar: day/month/year views, navigation and the
    /// selected date all live here (the calendar is the source of truth for the date).
    calendar: Entity<CalendarState>,
    /// The time half, owned by gpui-kit's `TimeField`. `None` for a `date`-only column.
    time_field: Option<Entity<TimeFieldState>>,
}

/// An in-progress drag of a grid column's right edge.
#[derive(Clone, Copy)]
struct ColumnResize {
    col: usize,
    /// Pointer x and column width when the drag started.
    start_x: f32,
    start_width: f32,
}

/// Pending destructive action that needs confirmation before it runs.
enum DeleteConfirm {
    /// Delete the selected rows of a grid.
    Rows { grid_id: u64, rows: Vec<usize> },
    /// Delete a connection (and its open grids/query tabs).
    Connection { index: usize },
    /// A destructive table operation.
    Table {
        connection_index: usize,
        database_index: usize,
        name: String,
        operation: TableOperation,
    },
    /// Delete a saved query from the saved-query list.
    SavedQuery { index: usize },
    /// Delete a backup file from disk.
    BackupFile { index: usize },
    /// Delete a saved backup configuration.
    BackupConfig { index: usize },
    /// Delete the selected row(s) of a table designer's fields/indexes/foreign-keys grid.
    DesignRows {
        design_id: u64,
        kind: DesignDeleteKind,
    },
    /// Drop a server account from the Users tab.
    User {
        connection_index: usize,
        user: String,
        host: String,
        label: String,
    },
    /// Drop a stored routine from the Functions tab or the connection tree.
    Routine {
        connection_index: usize,
        database_index: usize,
        name: String,
        kind: RoutineKind,
        label: String,
    },
    /// Drop a view from the Views tab or the connection tree.
    View {
        connection_index: usize,
        database_index: usize,
        name: String,
        label: String,
    },
    /// Drop a schema from a database (SQL Server).
    Schema {
        connection_index: usize,
        database_index: usize,
        schema: String,
    },
}

/// Which designer grid a [`DeleteConfirm::DesignRows`] confirmation applies to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DesignDeleteKind {
    Fields,
    Indexes,
    ForeignKeys,
}

#[derive(Clone, Copy)]
enum TableOperation {
    Drop,
    Empty,
    Truncate,
}

/// Which app dialog `Root` is currently hosting. AppView state stays the source of truth; this
/// only tracks the last kind handed to `window.open_dialog` so the transition fires once.
///
/// The Backup and Restore windows are deliberately **not** here: they are drawn as non-modal
/// floating panels by `AppView` so the rest of the app stays usable while they are open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DialogKind {
    DbDialog,
    Schema,
    CreateTable,
    Password,
    SaveQuery,
    Error,
    Confirm,
    /// "Discard unsaved changes?" confirmation guarding a close/navigate action.
    Unsaved,
}

/// A transient notification queued by a path that has no `Window`, flushed through `Root` on the
/// next render.
#[derive(Clone, Copy)]
enum ToastKind {
    Success,
    Info,
    Error,
}

/// The action to run once the user confirms discarding unsaved changes.
#[derive(Clone)]
enum PendingAction {
    /// Quit the application.
    Quit,
    /// Disconnect `index` and close its grids/designers.
    Disconnect { index: usize },
    /// Close the query tab at `index`.
    CloseQuery { index: usize },
    /// Close the table designer at `index`.
    CloseDesign { index: usize },
    /// Close every query/design tab (the tab menu's Close All).
    CloseAllTabs,
    /// Close every tab except `keep` (the tab menu's Close Others).
    CloseOtherTabs { keep: TabTarget },
    /// Discard a grid's pending edits and run one navigation action.
    GridDiscard { grid_id: u64, nav: GridNav },
}

/// A grid navigation/refresh blocked by pending edits, deferred until the user confirms.
#[derive(Clone)]
enum GridNav {
    First,
    Prev,
    Next,
    Last,
    Page(u64),
    Sort,
    /// A header-column sort toggle (not expressed through the sort draft).
    HeaderSort {
        column: String,
        descending: bool,
    },
    Filter,
    Refresh,
    PageSize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MainTab {
    Tables,
    Views,
    Functions,
    Users,
    Queries,
    Backups,
}

const MAIN_TABS: [(MainTab, &str, &str); 6] = [
    (MainTab::Tables, "icons/tables.svg", "main.tables"),
    (MainTab::Views, "icons/views.svg", "main.views"),
    (MainTab::Functions, "icons/functions.svg", "main.functions"),
    (MainTab::Users, "icons/user.svg", "main.users"),
    (MainTab::Queries, "icons/queries.svg", "main.queries"),
    (MainTab::Backups, "icons/backups.svg", "main.backups"),
];

/// Default and clamp widths of the drag-resizable side panes.
pub(super) const SIDEBAR_DEFAULT_WIDTH: f32 = 260.0;
pub(super) const SIDEBAR_MIN_WIDTH: f32 = 150.0;
pub(super) const SIDEBAR_MAX_WIDTH: f32 = 560.0;
pub(super) const INFO_DEFAULT_WIDTH: f32 = 230.0;
pub(super) const INFO_MIN_WIDTH: f32 = 180.0;
pub(super) const INFO_MAX_WIDTH: f32 = 640.0;
/// Width of a pane's drag-to-resize divider.
pub(super) const PANE_DIVIDER_WIDTH: f32 = 5.0;
/// Default and clamp heights of the query page's bottom result panel (the Navicat-style splitter
/// between the SQL editor and the 结果 tabs).
pub(super) const QUERY_RESULT_DEFAULT_HEIGHT: f32 = 280.0;
pub(super) const QUERY_RESULT_MIN_HEIGHT: f32 = 100.0;
pub(super) const QUERY_RESULT_MAX_HEIGHT: f32 = 1000.0;
/// The query editor keeps at least this height when the result splitter is dragged up.
pub(super) const QUERY_EDITOR_MIN_HEIGHT: f32 = 80.0;
const GRID_ROW_HEIGHT: f32 = 24.0;
const GRID_COLUMN_WIDTH: f32 = 120.0;
/// Filter-builder metrics, a compact take on the reference design: 24px controls, an 80px left
/// rail for the in-group `并且/或者` pill, 22px row actions, and a 30px between-groups bar carrying
/// the floating pill with 20px actions in a balanced slot.
const FILTER_CONTROL_HEIGHT: f32 = 24.0;
const FILTER_CONTROL_TEXT_SIZE: f32 = 12.0;
const FILTER_CONTROL_RADIUS: f32 = 6.0;
const FILTER_RAIL_WIDTH: f32 = 80.0;
const FILTER_FIELD_WIDTH: f32 = 120.0;
const FILTER_OPERATOR_WIDTH: f32 = 90.0;
/// Minimum width of the value field (it stretches to fill the rest of the row).
const FILTER_VALUE_WIDTH: f32 = 110.0;
const FILTER_ROW_ACTION_SIZE: f32 = 22.0;
const FILTER_ROW_ACTION_RADIUS: f32 = 6.0;
const FILTER_BOUNDARY_BAR_HEIGHT: f32 = 30.0;
const FILTER_BOUNDARY_ACTION_SIZE: f32 = 20.0;
const FILTER_BOUNDARY_ACTION_RADIUS: f32 = 4.0;
/// One side of the boundary bar's balanced slot (two 20px actions + their 4px gap).
const FILTER_BOUNDARY_ACTION_SLOT: f32 = 44.0;
/// The filter content fills the pane (responsive) up to this cap so a very wide window does not
/// stretch the value field and fling the row actions to the far edge. The divider on the block's
/// right edge drags a manual width between the min and max; double-clicking it restores auto-fit.
const FILTER_CONTENT_WIDTH_DEFAULT: f32 = 700.0;
const FILTER_CONTENT_WIDTH_MIN: f32 = 520.0;
const FILTER_CONTENT_WIDTH_MAX: f32 = 700.0;
/// Clamp for a column dragged to its narrowest/widest.
const MIN_COLUMN_WIDTH: f32 = 32.0;
const MAX_COLUMN_WIDTH: f32 = 1200.0;
const GRID_GUTTER_WIDTH: f32 = 22.0;
/// Thickness of the app-drawn grid scrollbars (matches `ui::vscrollbar_track` / `hscrollbar_track`).
const GRID_SCROLLBAR_THICKNESS: f32 = 14.0;
/// Cap used when "Limit Records" is unchecked, standing in for an unlimited fetch.
/// The page size used when "Limit Records" is off. A finite cap rather than a truly unbounded
/// fetch, so a huge table cannot exhaust memory in one query; the status bar reports the loaded
/// count so the user can tell when the cap is hit.
const NO_LIMIT_PAGE_SIZE: u64 = 200_000;

/// The Options window's left-nav pages.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OptionsSection {
    General,
    Editor,
    About,
}

pub struct AppView {
    registry: Arc<DriverRegistry>,
    config: Arc<ConfigStore>,
    runtime: Arc<Runtime>,
    connections: Vec<ConnectionNode>,
    grids: Vec<Entity<GridView>>,
    designs: Vec<Entity<design::TableDesignView>>,
    queries: Vec<QueryTab>,
    active_design: Option<usize>,
    next_design_id: u64,
    active_query: Option<usize>,
    next_query_id: u64,
    /// Saved queries, as individual `.sql` files under the config dir's `queries/` tree.
    query_files: Vec<QueryFileInfo>,
    /// The saved query highlighted in the Queries list, indexed into `query_files`. This is the
    /// single-selection mirror of `query_selection`.
    saved_query_selected: Option<usize>,
    /// The Queries list's Explorer-style multi-selection (plain click / Ctrl / Shift / marquee).
    query_selection: ListSelection,
    /// The Queries list's row rectangles in window space, for the marquee.
    query_row_rects: std::collections::HashMap<String, Bounds<Pixels>>,
    /// The in-app copied query file, for Ctrl+C / Ctrl+V.
    query_clipboard: Option<QueryClipboard>,
    /// The in-place rename editor for a query file, if any.
    query_rename: Option<QueryRenameEdit>,
    query_rename_blur: Option<Subscription>,
    query_rename_focus_pending: bool,
    /// Focus target for the query file list, so F2 / Ctrl+C / Ctrl+V reach it.
    query_list_focus: FocusHandle,
    /// The Queries 详细列表's current sort column and direction.
    query_sort: QuerySort,
    /// The Queries 平铺网格's horizontal scroll state.
    query_grid: ColumnGrid,
    /// Saved backup configurations loaded from `backups.json`.
    backup_configs: Vec<SavedBackup>,
    /// Backup files found under the config dir's `backups/` tree.
    backup_files: Vec<BackupFileInfo>,
    /// The backup-list entry highlighted in the Backup main tab.
    backup_selected: Option<BackupSelection>,
    /// The open "new backup" dialog, if any.
    new_backup_dialog: Option<NewBackupDialog>,
    /// The open "restore backup" dialog, if any.
    restore_dialog: Option<RestoreBackupDialog>,
    /// The open "extract SQL" dialog, if any.
    extract_dialog: Option<ExtractSqlDialog>,
    /// The configuration-name field of the "new backup" dialog.
    backup_name_input: Option<Entity<TextInput>>,
    backup_name_focus_pending: bool,
    /// The in-app copied backup file, for Ctrl+C / Ctrl+V.
    backup_clipboard: Option<BackupClipboard>,
    /// The in-place rename editor for a backup file, if any.
    backup_rename: Option<BackupRenameEdit>,
    backup_rename_blur: Option<Subscription>,
    backup_rename_focus_pending: bool,
    /// Focus target for the backup list, so F2 / Ctrl+C / Ctrl+V reach it.
    backup_focus: FocusHandle,
    /// Keeps the Backup list's scroll position across re-renders, plus the 详细列表's horizontal
    /// scroll (the header scrolls with the rows) and its scrollbar-thumb drags.
    backup_detail_scroll: DetailScroll,
    /// The Backup 详细列表's content-fitted / user-resized column widths.
    backup_columns: Rc<RefCell<DetailColumns>>,
    /// The Backup list's 平铺网格 scroll state.
    backup_grid: ColumnGrid,
    /// The Backup-list search text and its shared field.
    backup_search: String,
    backup_search_input: Entity<TextInput>,
    /// The open "save query" dialog, if any.
    save_query_dialog: Option<SaveQueryDialog>,
    query_name_input: Option<Entity<TextInput>>,
    /// The open "new table" name prompt, if any, and its shared name input.
    create_table_dialog: Option<CreateTableDialog>,
    create_table_input: Option<Entity<TextInput>>,
    /// The save-query dialog's connection and database pickers.
    save_connection_combo: Option<Entity<ComboBox>>,
    save_database_combo: Option<Entity<ComboBox>>,
    /// The last database names seen per connection profile id, so the save dialog can offer
    /// databases without opening the connection.
    database_cache: BTreeMap<String, Vec<String>>,
    save_query_focus_pending: bool,
    query_focus: FocusHandle,
    query_focus_pending: bool,
    /// One gpui-kit editor per query tab, index-aligned with `queries` and `None` until built.
    /// `ui::SqlEditor` owns the text, undo/redo, multi-cursor and search; `QueryTab::sql` mirrors
    /// it for the run/save/format paths.
    query_editors: Vec<Option<Entity<ui::SqlEditor>>>,
    /// Read-only SQL preview editors (routine/view ?? tabs), keyed by tab fingerprint.
    preview_editors: std::collections::HashMap<String, Entity<ui::SqlEditor>>,
    /// Shared completion catalog, refreshed as the connection tree loads tables and columns.
    completion_catalog: Arc<std::sync::RwLock<sql_completion::CompletionCatalog>>,
    /// The saved-query 详细列表's scroll state (vertical rows, horizontal header + rows) and its
    /// scrollbar-thumb drags.
    query_detail_scroll: DetailScroll,
    /// The saved-query 详细列表's content-fitted / user-resized column widths.
    query_detail_columns: Rc<RefCell<DetailColumns>>,
    query_connection_combo: Option<Entity<ComboBox>>,
    query_database_combo: Option<Entity<ComboBox>>,
    query_schema_combo: Option<Entity<ComboBox>>,
    /// The query page's bottom result-panel height, set by dragging the splitter between the
    /// editor and the 结果 tabs.
    query_result_height: f32,
    /// `(pointer y, panel height)` captured while that splitter is being dragged.
    query_split_drag: Option<(f32, f32)>,
    /// Column metadata keyed by `(connection index, database, table)`, loaded on demand for
    /// completion.
    query_column_cache:
        RefCell<std::collections::HashMap<(usize, String, String), ColumnCacheEntry>>,
    /// Bumped whenever the loaded table sets change, invalidating the completion cache.
    completion_generation: u64,
    active_grid: Option<usize>,
    next_grid_id: u64,
    form: Option<ConnectionForm>,
    editing: Option<usize>,
    test_status: TestStatus,
    context_menu: Option<ContextMenu>,
    tab_menu: Option<TabMenu>,
    /// Whether the titlebar theme dropdown is open, and where its button sits (so the popup can be
    /// anchored under it).
    theme_menu_open: bool,
    theme_menu_anchor: Rc<RefCell<Point<Pixels>>>,
    /// Whether the New Connection engine dropdown is open, and where its button sits.
    connect_menu_open: bool,
    connect_menu_anchor: Rc<RefCell<Point<Pixels>>>,
    object_pane: Option<Entity<ObjectPane>>,
    tab_bar: Entity<TabBar>,
    tree_pane: Entity<TreePane>,
    main_tab: MainTab,
    db_dialog: Option<DbDialog>,
    db_charset_combo: Option<Entity<ComboBox>>,
    db_collation_combo: Option<Entity<ComboBox>>,
    /// SQL Server owner dropdown (server logins) in the database dialog.
    db_owner_combo: Option<Entity<ComboBox>>,
    /// SQL Server recovery-model dropdown in the database dialog.
    db_recovery_combo: Option<Entity<ComboBox>>,
    /// SQL Server compatibility-level dropdown in the database dialog.
    db_compat_combo: Option<Entity<ComboBox>>,
    db_name_input: Option<Entity<TextInput>>,
    /// The schema name typed in the "New Schema" dialog.
    schema_name_input: Option<Entity<TextInput>>,
    /// The pending "New Schema" dialog; `None` when it is closed.
    schema_dialog: Option<SchemaDialog>,
    /// Focus the schema name field on the next frame, once its inner state exists.
    schema_focus_pending: bool,
    db_sql_focus: FocusHandle,
    db_sql_layout: RefCell<TextLayout>,
    db_sql_text: RefCell<String>,
    db_sql_anchor: usize,
    db_sql_cursor: usize,
    db_sql_selecting: bool,
    form_inputs: Option<FormInputs>,
    /// The ODBC driver dropdown, built when the form opens for an ODBC connection.
    form_odbc_driver: Option<Entity<ComboBox>>,
    /// The required general-page fields that were empty the last time 测试连接 / 保存并连接 ran, so
    /// the form can flag them inline. Cleared as soon as the user edits the offending field.
    form_errors: BTreeSet<FormField>,
    /// The tunnel-layer fields flagged as required by the same validation pass.
    form_tunnel_errors: BTreeSet<connection_form::TunnelField>,
    /// The TLS / advanced text inputs of the connection window, keyed by field.
    form_extra_inputs: BTreeMap<connection_form::FormExtra, Entity<TextInput>>,
    /// The per-layer text inputs of the tunnel/proxy chain, parallel to `form.settings.tunnel`.
    form_tunnel_inputs: Vec<connection_form::TunnelInputs>,
    /// The connection window's custom select (TLS mode): whether its menu is open and where its
    /// trigger sits.
    form_select_open: bool,
    form_select_anchor: Rc<RefCell<Point<Pixels>>>,
    /// The tunnel page's SSH auth-method select: whether its menu is open and where its trigger sits.
    form_tunnel_select_open: bool,
    form_tunnel_select_anchor: Rc<RefCell<Point<Pixels>>>,
    /// The form as it was when the window opened, so 重置 can restore it.
    form_initial: Option<ConnectionForm>,
    /// The OS window hosting the New/Edit Connection form, if open.
    connection_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// Focus target for the connection window, so ESC works before any field is focused.
    form_focus: FocusHandle,
    caret_visible: bool,
    password_prompt: Option<PasswordPrompt>,
    password_focus_pending: bool,
    rename_edit: Option<RenameEdit>,
    rename_blur: Option<Subscription>,
    rename_focus_pending: bool,
    page_size: u64,
    limit_records: bool,
    object_search: String,
    object_search_input: Entity<TextInput>,
    /// Scroll position of the backup/restore object picker, so it can show a vertical scrollbar.
    backup_objects_scroll: ScrollHandle,
    delete_confirm: Option<DeleteConfirm>,
    error_dialog: Option<String>,
    /// Transient notifications queued for the next frame, which has the window needed to hand
    /// them to `Root`.
    pending_toasts: Vec<(ToastKind, String)>,
    /// A close/navigate action waiting on the "discard unsaved changes?" confirmation.
    unsaved_confirm: Option<PendingAction>,
    /// The kind of dialog last opened through `Root`, if any.
    opened_dialog: Option<DialogKind>,
    window_bounds_subscription: Option<Subscription>,
    theme_setting: ThemeSetting,
    theme: Theme,
    /// Theme last pushed into the managed inputs/combos, so `sync_input_themes` can skip when
    /// nothing changed instead of updating every input on every frame.
    synced_input_theme: Option<Theme>,
    language: LanguageSetting,
    options_language: LanguageSetting,
    language_combo: Option<Entity<ComboBox>>,
    /// The SQL editor font family; empty means the built-in default.
    editor_font_family: String,
    editor_font_size: u32,
    editor_line_numbers: bool,
    editor_word_wrap: bool,
    /// The Options window's selected left-nav page.
    options_section: OptionsSection,
    /// Staged editor settings, applied on OK so Cancel discards them.
    options_editor_font_family: String,
    options_editor_font_size: u32,
    options_editor_line_numbers: bool,
    options_editor_word_wrap: bool,
    editor_font_combo: Option<Entity<ComboBox>>,
    editor_size_combo: Option<Entity<ComboBox>>,
    /// The OS window hosting the Options dialog, if open.
    options_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// Focus target for the Options window, so ESC works before any control is focused.
    options_focus: FocusHandle,
    /// Update-check state, driving the titlebar update button and the 关于 page.
    update_status: update::UpdateStatus,
    /// The newest release's version, once a check has found one.
    update_version: Option<String>,
    /// Live download progress for the in-flight install: `(bytes_downloaded, total_bytes)`.
    /// `total_bytes` is `None` until the server's `Content-Length` is known. Cleared when the
    /// install ends. Drives the titlebar percentage and the 关于 progress bar.
    update_progress: Option<(u64, Option<u64>)>,
    /// Whether the once-per-launch startup update check has been kicked off.
    update_checked: bool,
    /// Last data revision handed to the cached `TreePane` / `TabBar`, so they re-render only when
    /// what they read from `AppView` actually changed (they are embedded with `.cached`, which
    /// otherwise freezes them until they are explicitly notified).
    tree_revision: u64,
    tab_revision: u64,
    /// The connection-tree host, which owns its own (drag-resizable) width so resizing only
    /// re-renders it, not the whole app.
    sidebar_host: Entity<SidebarHost>,
    /// The right-hand object-info pane, which likewise owns its width and its resize divider.
    info_pane: Entity<InfoPane>,
    /// Whether the connection-tree sidebar is shown (bottom-right toggle).
    sidebar_open: bool,
    /// Whether the right-hand object-info pane is shown (bottom-right toggle).
    info_open: bool,
    /// The remembered list layout per page (详细列表 / 平铺网格), keyed by a stable page id and
    /// persisted in settings so each page keeps its own choice.
    view_modes: BTreeMap<String, String>,
    /// Remembered expanded databases per connection-profile id, restored when a connection's
    /// database list first loads.
    remembered_db_expansion: BTreeMap<String, Vec<String>>,
    /// The selection the info pane is currently loaded for, so async loads fire once per change.
    info_loaded_for: Option<String>,
    /// The connected server's `(version, sessions)` for the connection info pane.
    info_server: Loadable<(String, u64)>,
    /// The selected database's `(charset, collation)` for the database info pane.
    info_database: Loadable<DatabaseOptions>,
    /// The selected table `(connection, database, name)` driving the table info pane, set by the
    /// connection tree and the object list.
    info_table_selected: Option<(usize, usize, String)>,
    /// The selected table's status for the table info pane.
    info_table_status: Loadable<TableStatus>,
    /// The selected table's `CREATE` script for the table info pane's DDL view.
    info_table_ddl: Loadable<Option<String>>,
    /// Whether the table info pane shows the details or the DDL script.
    info_table_ddl_view: bool,
    /// The selected stored routine `(connection, database, name, kind)` driving the routine info
    /// pane, set by the connection tree and the Functions object list.
    info_routine_selected: Option<(usize, usize, String, RoutineKind)>,
    /// The selected routine's details for the routine info pane.
    info_routine: Loadable<RoutineDetails>,
    /// The Users main tab's loaded accounts for `users_connection`.
    users: Loadable<Vec<UserAccount>>,
    /// The connection whose users the Users tab shows.
    users_connection: Option<usize>,
    /// The Users-list search text.
    user_search: String,
    user_search_input: Entity<TextInput>,
    /// The Users list's multi-selection (Explorer-style).
    users_selection: ListSelection,
    /// The Users list's row rectangles in window space, keyed by selection key, for the marquee.
    users_row_rects: std::collections::HashMap<String, Bounds<Pixels>>,
    /// The Users list's focus target.
    users_focus: FocusHandle,
    /// The Users list's 平铺网格 scroll state.
    users_grid: ColumnGrid,
    /// The Backup list's multi-selection (Explorer-style).
    backups_selection: ListSelection,
    /// The Backup list's row rectangles in window space, for the marquee.
    backups_row_rects: std::collections::HashMap<String, Bounds<Pixels>>,
    /// The table/view object list's multi-selection (Explorer-style).
    objects_selection: ListSelection,
    /// The object list's row rectangles in window space, for the marquee.
    objects_row_rects: std::collections::HashMap<String, Bounds<Pixels>>,
    /// The in-progress rubber-band drag over one of the flat lists, if any.
    marquee: Option<(MarqueeTarget, MarqueeDrag)>,
    /// The routine editor's find bar field.
    routine_find_input: Entity<TextInput>,
    /// The account highlighted in the Users list, as an index into `users`.
    selected_user: Option<usize>,
    /// Keeps the Users list's scroll position across re-renders, plus the 详细列表's horizontal
    /// scroll (the header scrolls with the rows) and its scrollbar-thumb drags.
    users_detail_scroll: DetailScroll,
    /// The Users 详细列表's content-fitted / user-resized column widths.
    users_columns: Rc<RefCell<DetailColumns>>,
    /// The selected account's details, for the info pane.
    info_user: Loadable<UserDetails>,
    /// The open account window, if any: the create/edit flow launched from the Users toolbar.
    create_user_dialog: Option<user_create::UserCreateDialog>,
    /// The dialog's identity fields.
    create_user_user: Option<Entity<TextInput>>,
    create_user_host: Option<Entity<TextInput>>,
    create_user_password: Option<Entity<TextInput>>,
    create_user_confirm: Option<Entity<TextInput>>,
    /// The dialog's plugin and password-expiry dropdowns.
    create_user_plugin_combo: Option<Entity<ComboBox>>,
    create_user_expiry_combo: Option<Entity<ComboBox>>,
    /// The dialog's password-expiry interval (days) field, shown for the INTERVAL policy.
    create_user_expiry_days: Option<Entity<TextInput>>,
    /// The dialog's password-valid-until field (PostgreSQL's `VALID UNTIL`).
    create_user_password_valid_until: Option<Entity<TextInput>>,
    /// The dialog's Oracle DEFAULT TABLESPACE field.
    create_user_default_tablespace: Option<Entity<TextInput>>,
    /// The dialog's Oracle PROFILE field.
    create_user_profile: Option<Entity<TextInput>>,
    /// The dialog's Oracle tablespace-quota field.
    create_user_tablespace_quota: Option<Entity<TextInput>>,
    /// The dialog's resource-limit fields.
    create_user_max_questions: Option<Entity<TextInput>>,
    create_user_max_updates: Option<Entity<TextInput>>,
    create_user_max_connections: Option<Entity<TextInput>>,
    create_user_max_user_connections: Option<Entity<TextInput>>,
    /// The 权限 database filter.
    create_user_database_search: Option<Entity<TextInput>>,
    /// The 用户映射 section's fields for the active database.
    create_user_mapping_user: Option<Entity<TextInput>>,
    create_user_mapping_schema: Option<Entity<TextInput>>,
    /// The SQL Server 常规 section's verification / default-database dropdowns.
    create_user_verification_combo: Option<Entity<ComboBox>>,
    create_user_default_database_combo: Option<Entity<ComboBox>>,
    /// The SQL Server 常规 section's remaining fields.
    create_user_old_password: Option<Entity<TextInput>>,
    create_user_default_language: Option<Entity<TextInput>>,
    create_user_certificate: Option<Entity<TextInput>>,
    create_user_asymmetric_key: Option<Entity<TextInput>>,
    create_user_credential: Option<Entity<TextInput>>,
    /// The MySQL 高级 section's SSL type dropdown and its cipher/issuer/subject fields.
    create_user_ssl_combo: Option<Entity<ComboBox>>,
    create_user_ssl_cipher: Option<Entity<TextInput>>,
    create_user_ssl_issuer: Option<Entity<TextInput>>,
    create_user_ssl_subject: Option<Entity<TextInput>>,
    /// The OS window hosting the "New User" dialog, if open.
    create_user_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// Focus target for the account window: focusing it at open keeps ESC (and any key handler on
    /// the window root) working even when no field has the focus.
    create_user_focus: FocusHandle,
    /// The open 对象权限 window's manager entity (kept so theme changes reach it).
    object_privileges: Option<Entity<privilege_manager::PrivilegeManager>>,
    /// The OS window hosting the 对象权限 manager, if open.
    object_privileges_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// The OS window hosting the Backup/Restore UI, if open.
    backup_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// The open export wizard, if any.
    export_wizard: Option<ExportWizard>,
    /// The OS window hosting the export wizard, if open.
    export_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// The open import wizard, if any.
    import_wizard: Option<ImportWizard>,
    /// The OS window hosting the import wizard, if open.
    import_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// Focus targets for the child windows, so ESC closes them before any control is focused.
    export_focus: FocusHandle,
    import_focus: FocusHandle,
    backup_window_focus: FocusHandle,
    /// The main window's id, so closing it also closes the Backup/Restore window.
    main_window_id: Option<WindowId>,
    /// Keeps the window-closed listener alive, so closing the OS window clears the dialog state.
    _window_closed: Subscription,
}

mod backup;
mod connection_form;
mod database;
mod db_dialog;
mod design;
mod design_view;
mod dialogs;
mod export;
mod form;
mod grid;
mod grid_cell;
mod grid_commit;
mod grid_ime;
mod grid_input;
mod grid_menu;
mod grid_scroll;
mod grid_toolbar;
mod grid_view;
mod import;
mod info_pane;
mod list_ops;
mod objects;
mod options;
mod privilege_manager;
mod query;
mod query_editor;
mod query_view;
mod routine;
mod routine_view;
mod sidebar;
mod sql_completion;
mod tabs;
mod toolbar;
mod tree;
mod ui;
mod update;
mod user;
mod user_create;
mod view;
mod view_view;
mod widgets;

use info_pane::{InfoPane, SidebarHost};

/// The language to start in on a first launch: a Chinese system locale maps to `zh-CN`, every
/// other locale to English.
fn detect_system_language() -> LanguageSetting {
    match sys_locale::get_locale() {
        Some(locale) if locale.to_ascii_lowercase().starts_with("zh") => LanguageSetting::ZhCn,
        _ => LanguageSetting::En,
    }
}

/// Load the persisted settings, applying the OS language on a first launch (before `settings.json`
/// exists) and persisting it. Later launches keep whatever the user last chose.
pub(crate) fn load_startup_settings(config: &ConfigStore) -> AppSettings {
    let mut settings = config.load_settings().unwrap_or_default();
    if !config.settings_path().exists() {
        settings.language = detect_system_language();
        let _ = config.save_settings(&settings);
    }
    settings
}

impl AppView {
    pub fn new(
        registry: Arc<DriverRegistry>,
        config: Arc<ConfigStore>,
        runtime: Arc<Runtime>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let secrets = config.load_secrets().unwrap_or_default();
        let settings = load_startup_settings(&config);

        let connections = config
            .load_profiles()
            .unwrap_or_default()
            .into_iter()
            .map(|profile| {
                let password = secrets.get(&profile.id).cloned();
                let password_saved = password.is_some();
                let expanded = settings.expanded_connections.contains(&profile.id);
                ConnectionNode {
                    profile,
                    password,
                    password_saved,
                    status: ConnectionStatus::Disconnected,
                    databases: Loadable::Idle,
                    expanded,
                }
            })
            .collect();

        let remembered_db_expansion = settings.expanded_databases.clone();
        let theme_setting = settings.theme;
        let language = settings.language;
        let _ = config.migrate_legacy_queries();
        let query_files = query::scan_query_files(&config);
        let backup_configs = config.load_backups().unwrap_or_default();
        let database_cache = config.load_database_cache().unwrap_or_default();
        // Claim Tab inside the cell editor so it advances to the next cell rather than moving
        // window focus (which is what the `Root` context binds it to).
        cx.bind_keys([
            KeyBinding::new("tab", NextCell, Some(GRID_CELL_CONTEXT)),
            KeyBinding::new("shift-tab", PrevCell, Some(GRID_CELL_CONTEXT)),
            // Vertical cell movement while the in-place editor holds focus. The `GridCell > Input`
            // predicate ties with the kit's own `Input` binding at the same depth, and later
            // bindings win, so the grid's action takes over the keystroke.
            KeyBinding::new("up", MoveCellUp, Some(GRID_CELL_INPUT_CONTEXT)),
            KeyBinding::new("down", MoveCellDown, Some(GRID_CELL_INPUT_CONTEXT)),
            KeyBinding::new("f2", RenameBackupFile, Some(BACKUP_LIST_CONTEXT)),
            KeyBinding::new("ctrl-c", CopyBackupFile, Some(BACKUP_LIST_CONTEXT)),
            KeyBinding::new("cmd-c", CopyBackupFile, Some(BACKUP_LIST_CONTEXT)),
            KeyBinding::new("ctrl-v", PasteBackupFile, Some(BACKUP_LIST_CONTEXT)),
            KeyBinding::new("cmd-v", PasteBackupFile, Some(BACKUP_LIST_CONTEXT)),
            KeyBinding::new("f2", RenameQueryFile, Some(QUERY_LIST_CONTEXT)),
            KeyBinding::new("ctrl-c", CopyQueryFile, Some(QUERY_LIST_CONTEXT)),
            KeyBinding::new("cmd-c", CopyQueryFile, Some(QUERY_LIST_CONTEXT)),
            KeyBinding::new("ctrl-v", PasteQueryFile, Some(QUERY_LIST_CONTEXT)),
            KeyBinding::new("cmd-v", PasteQueryFile, Some(QUERY_LIST_CONTEXT)),
            // Run the editor's selection (or whole script) and save it from the editor.
            KeyBinding::new("ctrl-enter", RunSelectedQuery, Some(QUERY_EDITOR_CONTEXT)),
            KeyBinding::new("cmd-enter", RunSelectedQuery, Some(QUERY_EDITOR_CONTEXT)),
            KeyBinding::new("ctrl-s", SaveQuery, Some(QUERY_EDITOR_CONTEXT)),
            KeyBinding::new("cmd-s", SaveQuery, Some(QUERY_EDITOR_CONTEXT)),
        ]);
        let app = cx.weak_entity();
        let app_entity = cx.entity();
        let tree_pane = cx.new(|cx| TreePane::new(app.clone(), cx));
        let sidebar_host = cx.new(|cx| SidebarHost::new(app.clone(), &app_entity, cx));
        let info_pane = cx.new(|cx| InfoPane::new(app.clone(), &app_entity, &tree_pane, cx));
        let window_closed = cx.on_window_closed({
            let weak = app.clone();
            move |cx, id| {
                if let Some(app) = weak.upgrade() {
                    app.update(cx, |app, cx| app.on_window_closed(id, cx));
                }
            }
        });

        Self {
            registry,
            config,
            runtime,
            connections,
            grids: Vec::new(),
            designs: Vec::new(),
            queries: Vec::new(),
            active_design: None,
            next_design_id: 0,
            active_query: None,
            next_query_id: 0,
            query_files,
            saved_query_selected: None,
            query_selection: ListSelection::default(),
            query_row_rects: std::collections::HashMap::new(),
            query_clipboard: None,
            query_rename: None,
            query_rename_blur: None,
            query_rename_focus_pending: false,
            query_list_focus: cx.focus_handle(),
            query_sort: QuerySort::default(),
            query_grid: ColumnGrid::default(),
            backup_configs,
            backup_files: Vec::new(),
            backup_selected: None,
            new_backup_dialog: None,
            restore_dialog: None,
            extract_dialog: None,
            backup_name_input: None,
            backup_name_focus_pending: false,
            backup_clipboard: None,
            backup_rename: None,
            backup_rename_blur: None,
            backup_rename_focus_pending: false,
            backup_focus: cx.focus_handle(),
            backup_detail_scroll: DetailScroll::default(),
            backup_columns: Rc::new(RefCell::new(DetailColumns::default())),
            backup_grid: ColumnGrid::default(),
            backup_search: String::new(),
            backup_search_input: {
                let weak = app.clone();
                cx.new(move |cx| {
                    TextInput::new(
                        Theme::dark(),
                        "",
                        TextInputOptions {
                            placeholder: t!("backup.search").to_string().into(),
                            icon: Some("icons/search.svg"),
                            clearable: true,
                            ..Default::default()
                        },
                        cx,
                    )
                    .on_change(Rc::new(move |text, _window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            app.backup_search = text.to_string();
                            cx.notify();
                        });
                    }))
                })
            },
            save_query_dialog: None,
            query_name_input: None,
            create_table_dialog: None,
            create_table_input: None,
            save_connection_combo: None,
            save_database_combo: None,
            database_cache,
            save_query_focus_pending: false,
            query_focus: cx.focus_handle(),
            query_focus_pending: false,
            query_editors: Vec::new(),
            preview_editors: std::collections::HashMap::new(),
            completion_catalog: Arc::new(std::sync::RwLock::new(
                sql_completion::CompletionCatalog::default(),
            )),
            query_detail_scroll: DetailScroll::default(),
            query_detail_columns: Rc::new(RefCell::new(DetailColumns::default())),
            query_connection_combo: None,
            query_database_combo: None,
            query_schema_combo: None,
            query_result_height: QUERY_RESULT_DEFAULT_HEIGHT,
            query_split_drag: None,
            query_column_cache: RefCell::new(std::collections::HashMap::new()),
            completion_generation: 0,
            active_grid: None,
            next_grid_id: 0,
            form: None,
            editing: None,
            test_status: TestStatus::Idle,
            context_menu: None,
            tab_menu: None,
            theme_menu_open: false,
            theme_menu_anchor: Rc::new(RefCell::new(Point::default())),
            connect_menu_open: false,
            connect_menu_anchor: Rc::new(RefCell::new(Point::default())),
            object_pane: None,
            tab_bar: cx.new(|_| TabBar::new(app.clone())),
            tree_pane,
            main_tab: MainTab::Tables,
            db_dialog: None,
            db_charset_combo: None,
            db_collation_combo: None,
            db_owner_combo: None,
            db_recovery_combo: None,
            db_compat_combo: None,
            db_name_input: None,
            schema_name_input: None,
            schema_dialog: None,
            schema_focus_pending: false,
            db_sql_focus: cx.focus_handle(),
            db_sql_layout: RefCell::new(TextLayout::default()),
            db_sql_text: RefCell::new(String::new()),
            db_sql_anchor: 0,
            db_sql_cursor: 0,
            db_sql_selecting: false,
            form_inputs: None,
            form_odbc_driver: None,
            form_errors: BTreeSet::new(),
            form_tunnel_errors: BTreeSet::new(),
            form_extra_inputs: BTreeMap::new(),
            form_tunnel_inputs: Vec::new(),
            form_select_open: false,
            form_select_anchor: Rc::new(RefCell::new(Point::default())),
            form_tunnel_select_open: false,
            form_tunnel_select_anchor: Rc::new(RefCell::new(Point::default())),
            form_initial: None,
            connection_window: None,
            form_focus: cx.focus_handle(),
            caret_visible: true,
            password_prompt: None,
            password_focus_pending: false,
            rename_edit: None,
            rename_blur: None,
            rename_focus_pending: false,
            page_size: 1000,
            limit_records: true,
            object_search: String::new(),
            backup_objects_scroll: ScrollHandle::new(),
            object_search_input: {
                let weak = app.clone();
                cx.new(move |cx| {
                    TextInput::new(
                        Theme::dark(),
                        "",
                        TextInputOptions {
                            placeholder: t!("object.search").to_string().into(),
                            icon: Some("icons/search.svg"),
                            clearable: true,
                            ..Default::default()
                        },
                        cx,
                    )
                    .on_change(Rc::new(move |text, _window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            app.object_search = text.to_string();
                            app.notify_object_pane(cx);
                            cx.notify();
                        });
                    }))
                })
            },
            delete_confirm: None,
            error_dialog: None,
            pending_toasts: Vec::new(),
            unsaved_confirm: None,
            opened_dialog: None,
            window_bounds_subscription: None,
            theme_setting,
            theme: Theme::dark(),
            synced_input_theme: None,
            language,
            options_language: language,
            language_combo: None,
            editor_font_family: settings.editor_font_family.clone(),
            editor_font_size: settings.editor_font_size,
            editor_line_numbers: settings.editor_line_numbers,
            editor_word_wrap: settings.editor_word_wrap,
            options_section: OptionsSection::General,
            options_editor_font_family: settings.editor_font_family.clone(),
            options_editor_font_size: settings.editor_font_size,
            options_editor_line_numbers: settings.editor_line_numbers,
            options_editor_word_wrap: settings.editor_word_wrap,
            editor_font_combo: None,
            editor_size_combo: None,
            options_window: None,
            options_focus: cx.focus_handle(),
            update_status: update::UpdateStatus::Idle,
            update_version: None,
            update_progress: None,
            update_checked: false,
            tree_revision: 0,
            tab_revision: 0,
            sidebar_host,
            info_pane,
            sidebar_open: true,
            // The info pane stays hidden until the user reveals it; the choice is remembered in
            // settings, so it is restored here instead of being forced open on the Users tab.
            info_open: settings.show_info_pane,
            view_modes: settings.view_modes.clone(),
            remembered_db_expansion,
            info_loaded_for: None,
            info_server: Loadable::Idle,
            info_database: Loadable::Idle,
            info_table_selected: None,
            info_table_status: Loadable::Idle,
            info_table_ddl: Loadable::Idle,
            info_table_ddl_view: false,
            info_routine_selected: None,
            info_routine: Loadable::Idle,
            users: Loadable::Idle,
            users_connection: None,
            user_search: String::new(),
            user_search_input: {
                let weak = app.clone();
                cx.new(move |cx| {
                    TextInput::new(
                        Theme::dark(),
                        "",
                        TextInputOptions {
                            placeholder: t!("user.search").to_string().into(),
                            icon: Some("icons/search.svg"),
                            clearable: true,
                            ..Default::default()
                        },
                        cx,
                    )
                    .on_change(Rc::new(move |text, _window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            app.user_search = text.to_string();
                            cx.notify();
                        });
                    }))
                })
            },
            routine_find_input: {
                let weak = app.clone();
                let submit = weak.clone();
                cx.new(move |cx| {
                    TextInput::new(
                        Theme::dark(),
                        "",
                        TextInputOptions {
                            placeholder: t!("routine.find").to_string().into(),
                            icon: Some("icons/search.svg"),
                            clearable: true,
                            ..Default::default()
                        },
                        cx,
                    )
                    .on_change(Rc::new(move |text, _window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            if let Some(routine) = app
                                .active_query
                                .and_then(|index| app.queries.get_mut(index))
                                .and_then(|tab| tab.routine.as_mut())
                            {
                                routine.find_query = text.to_string();
                            }
                            cx.notify();
                        });
                    }))
                    .on_submit(Rc::new(move |_window, cx| {
                        let _ = submit.update(cx, |app, cx| app.routine_find_next(cx));
                    }))
                })
            },
            selected_user: None,
            users_detail_scroll: DetailScroll::default(),
            users_columns: Rc::new(RefCell::new(DetailColumns::default())),
            users_selection: ListSelection::default(),
            users_row_rects: std::collections::HashMap::new(),
            users_focus: cx.focus_handle(),
            users_grid: ColumnGrid::default(),
            backups_selection: ListSelection::default(),
            backups_row_rects: std::collections::HashMap::new(),
            objects_selection: ListSelection::default(),
            objects_row_rects: std::collections::HashMap::new(),
            marquee: None,
            info_user: Loadable::Idle,
            create_user_dialog: None,
            create_user_user: None,
            create_user_host: None,
            create_user_password: None,
            create_user_confirm: None,
            create_user_plugin_combo: None,
            create_user_expiry_combo: None,
            create_user_expiry_days: None,
            create_user_password_valid_until: None,
            create_user_default_tablespace: None,
            create_user_profile: None,
            create_user_tablespace_quota: None,
            create_user_max_questions: None,
            create_user_max_updates: None,
            create_user_max_connections: None,
            create_user_max_user_connections: None,
            create_user_database_search: None,
            create_user_mapping_user: None,
            create_user_mapping_schema: None,
            create_user_verification_combo: None,
            create_user_default_database_combo: None,
            create_user_old_password: None,
            create_user_default_language: None,
            create_user_certificate: None,
            create_user_asymmetric_key: None,
            create_user_credential: None,
            create_user_ssl_combo: None,
            create_user_ssl_cipher: None,
            create_user_ssl_issuer: None,
            create_user_ssl_subject: None,
            create_user_window: None,
            create_user_focus: cx.focus_handle(),
            object_privileges: None,
            object_privileges_window: None,
            backup_window: None,
            export_wizard: None,
            export_window: None,
            import_wizard: None,
            import_window: None,
            export_focus: cx.focus_handle(),
            import_focus: cx.focus_handle(),
            backup_window_focus: cx.focus_handle(),
            main_window_id: None,
            _window_closed: window_closed,
        }
    }

    fn set_theme(&mut self, setting: ThemeSetting, cx: &mut Context<'_, Self>) {
        if self.theme_setting == setting {
            return;
        }
        self.theme_setting = setting;
        self.persist_settings();
        self.notify_object_pane(cx);
        cx.notify();
    }

    fn set_language(&mut self, language: LanguageSetting, cx: &mut Context<'_, Self>) {
        if self.language == language {
            return;
        }
        self.language = language;
        rust_i18n::set_locale(language.locale());
        self.persist_settings();
        self.notify_object_pane(cx);
        cx.notify();
    }

    /// Show or hide the right-hand object-info pane and remember the choice, so the next launch
    /// restores it. This is the only way the pane is revealed from the chrome; it is never shown
    /// automatically.
    pub(super) fn set_info_open(&mut self, open: bool, cx: &mut Context<'_, Self>) {
        if self.info_open == open {
            return;
        }
        self.info_open = open;
        self.persist_settings();
        cx.notify();
    }

    /// Write the current settings to disk. Every preference change funnels through here so new
    /// fields are persisted once.
    fn persist_settings(&self) {
        // Derive the expanded connection/database sets from the live tree so the next launch can
        // restore them.
        let expanded_connections: Vec<String> = self
            .connections
            .iter()
            .filter(|node| node.expanded)
            .map(|node| node.profile.id.clone())
            .collect();
        let mut expanded_databases: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for node in &self.connections {
            if let Loadable::Loaded(databases) = &node.databases {
                let names: Vec<String> = databases
                    .iter()
                    .filter(|database| database.expanded)
                    .map(|database| database.name.clone())
                    .collect();
                if !names.is_empty() {
                    expanded_databases.insert(node.profile.id.clone(), names);
                }
            }
        }
        if let Err(error) = self.config.save_settings(&AppSettings {
            theme: self.theme_setting,
            language: self.language,
            show_info_pane: self.info_open,
            view_modes: self.view_modes.clone(),
            editor_font_family: self.editor_font_family.clone(),
            editor_font_size: self.editor_font_size,
            editor_line_numbers: self.editor_line_numbers,
            editor_word_wrap: self.editor_word_wrap,
            expanded_connections,
            expanded_databases,
        }) {
            // Preferences failing to persist is not worth a modal; note it in the log line only.
            let _ = error;
        }
    }

    /// The SQL editor's font family (the built-in default when the setting is empty).
    pub(super) fn editor_font(&self) -> String {
        if self.editor_font_family.is_empty() {
            default_editor_font().to_string()
        } else {
            self.editor_font_family.clone()
        }
    }

    /// The SQL editor's font size in px, clamped to a usable range.
    pub(super) fn editor_font_size(&self) -> f32 {
        self.editor_font_size.clamp(8, 48) as f32
    }

    /// The remembered list layout for a page, defaulting to 详细列表.
    pub(super) fn view_mode(&self, page: &str) -> ViewMode {
        self.view_modes
            .get(page)
            .map(|value| ViewMode::from_id(value))
            .unwrap_or_default()
    }

    /// Remember a page's list layout and re-render it.
    pub(super) fn set_view_mode(&mut self, page: &str, mode: ViewMode, cx: &mut Context<'_, Self>) {
        if self.view_mode(page) == mode {
            return;
        }
        self.view_modes
            .insert(page.to_string(), mode.id().to_string());
        self.persist_settings();
        cx.notify();
        // The 平铺 grid scrolls horizontally, and its scroll extents are only known after a layout
        // pass; schedule one more frame so the scrollbar appears without waiting for a resize.
        if mode == ViewMode::Grid {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(32))
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    /// Re-render the object pane (if any) after state it reads from `AppView` — loaded tables,
    /// theme, search text — changed.
    fn notify_object_pane(&self, cx: &mut Context<'_, Self>) {
        if let Some(pane) = self.object_pane.as_ref() {
            let theme = self.theme;
            pane.update(cx, |pane, cx| {
                pane.theme = theme;
                cx.notify();
            });
        }
    }

    /// Whether the driver behind `connection_index` supports `capability`. An unknown connection or
    /// driver keeps the historical "supported" behaviour so nothing is hidden by accident.
    fn driver_supports(&self, connection_index: usize, capability: DriverCapability) -> bool {
        let Some(node) = self.connections.get(connection_index) else {
            return true;
        };
        let Some(driver) = self.registry.get(&node.profile.driver) else {
            return true;
        };
        driver.descriptor().capabilities.has(capability)
    }

    /// The SQL dialect of the driver behind `connection_index` (generic when unknown).
    fn driver_dialect(&self, connection_index: usize) -> rustgrid_core::DriverDialect {
        self.connections
            .get(connection_index)
            .and_then(|node| self.registry.get(&node.profile.driver))
            .map(|driver| driver.dialect())
            .unwrap_or_default()
    }

    /// The engine's database-dialog layout for `connection_index` (falls back to MySQL's
    /// charset + collation).
    fn database_editor_spec(&self, connection_index: usize) -> DatabaseEditorSpec {
        self.connections
            .get(connection_index)
            .and_then(|node| self.registry.get(&node.profile.driver))
            .map(|driver| driver.database_editor())
            .unwrap_or_default()
    }

    /// The engine's user-editor layout for `connection_index` (falls back to MySQL's model).
    fn user_editor_spec(&self, connection_index: usize) -> UserEditorSpec {
        self.connections
            .get(connection_index)
            .and_then(|node| self.registry.get(&node.profile.driver))
            .map(|driver| driver.user_editor())
            .unwrap_or_else(UserEditorSpec::mysql)
    }

    /// The privilege catalog of the connection behind `connection_index` (empty when unknown).
    fn user_privilege_catalog(&self, connection_index: usize) -> PrivilegeCatalog {
        self.connection_arc(connection_index)
            .map(|connection| connection.privilege_catalog())
            .unwrap_or_default()
    }

    /// The recovery models offered by the database dialog for `connection_index`.
    fn database_recovery_models(&self, connection_index: usize) -> Vec<&'static str> {
        self.connections
            .get(connection_index)
            .and_then(|node| self.registry.get(&node.profile.driver))
            .map(|driver| driver.database_recovery_models())
            .unwrap_or_default()
    }

    /// The compatibility levels offered by the database dialog for `connection_index`.
    fn database_compatibility_levels(&self, connection_index: usize) -> Vec<&'static str> {
        self.connections
            .get(connection_index)
            .and_then(|node| self.registry.get(&node.profile.driver))
            .map(|driver| driver.database_compatibility_levels())
            .unwrap_or_default()
    }

    /// The rename editor's row data for `pane`, or `None` while the editor belongs to the other
    /// pane (or no editor is open).
    fn rename_row(&self, pane: RowPane) -> Option<RenameRow> {
        self.rename_edit
            .as_ref()
            .filter(|edit| edit.pane == pane)
            .map(|edit| RenameRow {
                connection_index: edit.connection_index,
                database_index: edit.database_index,
                old_name: edit.old_name.clone(),
                input: edit.input.clone(),
            })
    }

    /// Re-render the pane that draws the rename editor. Deferred on purpose: the rename can be
    /// committed from inside that same pane's update (a row click while editing), where notifying
    /// it synchronously would re-enter the borrowed entity.
    fn notify_rename_pane(&self, pane: RowPane, cx: &mut Context<'_, Self>) {
        match pane {
            RowPane::Objects => {
                if let Some(target) = self.object_pane.clone() {
                    cx.defer(move |cx| target.update(cx, |_, cx| cx.notify()));
                }
            }
            RowPane::Tree => {
                let target = self.tree_pane.clone();
                cx.defer(move |cx| target.update(cx, |_, cx| cx.notify()));
            }
        }
    }

    /// The focus handle of the pane drawing the rename editor, so the list keeps the keyboard and
    /// F2 keeps working once the editor closes.
    fn rename_owner_focus(&self, cx: &App) -> Option<FocusHandle> {
        match self.rename_edit.as_ref()?.pane {
            RowPane::Objects => self
                .object_pane
                .as_ref()
                .map(|pane| pane.read(cx).focus_handle()),
            RowPane::Tree => Some(self.tree_pane.read(cx).focus_handle()),
        }
    }

    /// Re-render every open grid. Their cached subtrees don't observe `AppView`, so changes they
    /// read from it (theme, window size) must be pushed explicitly.
    fn notify_grids(&self, cx: &mut Context<'_, Self>) {
        for grid in &self.grids {
            grid.update(cx, |_, cx| cx.notify());
        }
    }

    /// A cheap revision of everything [`TreePane`] reads from `AppView`. Notifying the tree only
    /// when this changes lets it stay cached across unrelated frames (e.g. grid scrolling).
    fn tree_revision(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.theme.is_dark().hash(&mut hasher);
        self.connections.len().hash(&mut hasher);
        for node in &self.connections {
            node.profile.name.hash(&mut hasher);
            match &node.status {
                ConnectionStatus::Connected(_) => 0u8.hash(&mut hasher),
                ConnectionStatus::Connecting => 1u8.hash(&mut hasher),
                ConnectionStatus::Failed(error) => {
                    2u8.hash(&mut hasher);
                    error.hash(&mut hasher);
                }
                ConnectionStatus::Disconnected => 3u8.hash(&mut hasher),
            }
            node.expanded.hash(&mut hasher);
            match &node.databases {
                Loadable::Idle => 0u8.hash(&mut hasher),
                Loadable::Loading => 1u8.hash(&mut hasher),
                Loadable::Failed(error) => {
                    2u8.hash(&mut hasher);
                    error.hash(&mut hasher);
                }
                Loadable::Loaded(databases) => {
                    3u8.hash(&mut hasher);
                    databases.len().hash(&mut hasher);
                    for database in databases {
                        database.name.hash(&mut hasher);
                        database.opened.hash(&mut hasher);
                        database.expanded.hash(&mut hasher);
                        for category in Category::ALL {
                            database.categories.get(category).hash(&mut hasher);
                        }
                        if database.expanded {
                            match &database.tables {
                                Loadable::Idle => 0u8.hash(&mut hasher),
                                Loadable::Loading => 1u8.hash(&mut hasher),
                                Loadable::Failed(error) => {
                                    2u8.hash(&mut hasher);
                                    error.hash(&mut hasher);
                                }
                                Loadable::Loaded(tables) => {
                                    3u8.hash(&mut hasher);
                                    tables.len().hash(&mut hasher);
                                    for table in tables {
                                        table.name.hash(&mut hasher);
                                    }
                                }
                            }
                            match &database.routines {
                                Loadable::Idle => 0u8.hash(&mut hasher),
                                Loadable::Loading => 1u8.hash(&mut hasher),
                                Loadable::Failed(error) => {
                                    2u8.hash(&mut hasher);
                                    error.hash(&mut hasher);
                                }
                                Loadable::Loaded(routines) => {
                                    3u8.hash(&mut hasher);
                                    routines.len().hash(&mut hasher);
                                    for routine in routines {
                                        routine.name.hash(&mut hasher);
                                        routine.kind.hash(&mut hasher);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        hasher.finish()
    }

    /// A cheap revision of everything [`TabBar`] reads from `AppView`.
    fn tab_revision(&self, cx: &App) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.theme.is_dark().hash(&mut hasher);
        self.active_grid.hash(&mut hasher);
        self.active_design.hash(&mut hasher);
        self.active_query.hash(&mut hasher);
        self.grids.len().hash(&mut hasher);
        for grid in &self.grids {
            let grid = grid.read(cx);
            grid.state.id.hash(&mut hasher);
            grid.state.sql.is_some().hash(&mut hasher);
        }
        self.designs.len().hash(&mut hasher);
        for design in &self.designs {
            let design = design.read(cx);
            design.id.hash(&mut hasher);
            design.dirty.hash(&mut hasher);
            design.is_view.hash(&mut hasher);
        }
        self.queries.len().hash(&mut hasher);
        for query in &self.queries {
            query.id.hash(&mut hasher);
            query.name.hash(&mut hasher);
            if let Some(routine) = &query.routine {
                routine.name.hash(&mut hasher);
                routine.kind.hash(&mut hasher);
            }
            if let Some(view) = &query.view {
                view.name.hash(&mut hasher);
                view.original_name.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Re-render the cached long-lived children only when their inputs changed.
    fn sync_cached_children(&mut self, cx: &mut Context<'_, Self>) {
        let tree_revision = self.tree_revision();
        if tree_revision != self.tree_revision {
            self.tree_revision = tree_revision;
            self.tree_pane.update(cx, |_, cx| cx.notify());
        }
        let tab_revision = self.tab_revision(cx);
        if tab_revision != self.tab_revision {
            self.tab_revision = tab_revision;
            self.tab_bar.update(cx, |_, cx| cx.notify());
        }
    }

    /// Push the resolved theme into every managed [`TextInput`] so the fields track light/dark
    /// changes. `TextInput::set_theme` only notifies when the value actually changed.
    fn sync_input_themes(&mut self, cx: &mut Context<'_, Self>) {
        let theme = self.theme;
        if let Some(inputs) = self.form_inputs.as_ref() {
            for input in &inputs.fields {
                input.update(cx, |input, cx| input.set_theme(theme, cx));
            }
        }
        self.object_search_input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        self.user_search_input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        self.backup_search_input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        self.routine_find_input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        if let Some(input) = self.db_name_input.as_ref() {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
        if let Some(input) = self.schema_name_input.as_ref() {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
        if let Some(prompt) = self.password_prompt.as_ref() {
            prompt
                .input
                .update(cx, |input, cx| input.set_theme(theme, cx));
        }
        if let Some(edit) = self.rename_edit.as_ref() {
            edit.input
                .update(cx, |input, cx| input.set_theme(theme, cx));
        }
        if let Some(input) = self.query_name_input.as_ref() {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
        if let Some(input) = self.backup_name_input.as_ref() {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
        if let Some(input) = self
            .export_wizard
            .as_ref()
            .and_then(|wizard| wizard.dir_input.as_ref())
        {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
        if let Some(combo) = self
            .export_wizard
            .as_ref()
            .and_then(|wizard| wizard.field_combo.as_ref())
        {
            combo.update(cx, |combo, cx| combo.set_theme(theme, cx));
        }
        if let Some(edit) = self.backup_rename.as_ref() {
            edit.input
                .update(cx, |input, cx| input.set_theme(theme, cx));
        }
        if let Some(edit) = self.query_rename.as_ref() {
            edit.input
                .update(cx, |input, cx| input.set_theme(theme, cx));
        }
        for design in &self.designs {
            design.update(cx, |design, cx| design.set_theme(theme, cx));
        }
        if let Some(manager) = self.object_privileges.as_ref() {
            manager.update(cx, |manager, cx| manager.set_theme(theme, cx));
        }
        for combo in [
            self.db_charset_combo.as_ref(),
            self.db_collation_combo.as_ref(),
            self.db_owner_combo.as_ref(),
            self.db_recovery_combo.as_ref(),
            self.db_compat_combo.as_ref(),
            self.query_connection_combo.as_ref(),
            self.query_database_combo.as_ref(),
            self.query_schema_combo.as_ref(),
            self.save_connection_combo.as_ref(),
            self.save_database_combo.as_ref(),
            self.language_combo.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            combo.update(cx, |combo, cx| combo.set_theme(theme, cx));
        }
    }

    // ----- Bottom status bar --------------------------------------------------------------------

    /// A small toggle in the status bar's bottom-right corner, mirroring Navicat's pane controls:
    /// a filled "panel" icon whose column is on the left (navigation pane) or right (info pane),
    /// so the two buttons are mirror images.
    fn pane_toggle(
        &self,
        open: bool,
        icon: &'static str,
        id: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = self.theme;
        let color = if open {
            theme.icon_connection
        } else {
            theme.text_muted
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(20.0))
            .h(px(18.0))
            .flex_none()
            .rounded(px(2.0))
            .cursor_pointer()
            .text_color(rgb(color))
            .when(open, move |style| style.bg(rgb(theme.tree_selected_bg)))
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(on_click)
            .child(
                svg()
                    .path(icon)
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .text_color(rgb(color)),
            )
    }

    /// The window's fixed bottom status bar: the active view's status in the middle and the
    /// side-pane toggles in the bottom-right corner.
    fn render_status_bar(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let center: AnyElement = if self.main_tab == MainTab::Backups {
            let count = self.visible_backup_files(cx).len() + self.visible_backup_configs(cx).len();
            div()
                .flex_1()
                .min_w(px(0.0))
                .px_2()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(format!("{count} {}", t!("common.backup")))
                .into_any_element()
        } else if self.main_tab == MainTab::Users {
            div()
                .flex_1()
                .min_w(px(0.0))
                .child(self.render_users_status())
                .into_any_element()
        } else if self.active_grid.is_none()
            && self.active_query.is_none()
            && self.active_design.is_none()
            && let Some(pane) = self.object_pane.clone()
        {
            div()
                .flex_1()
                .min_w(px(0.0))
                .child(self.render_object_status(&pane, cx))
                .into_any_element()
        } else {
            div().flex_1().min_w(px(0.0)).into_any_element()
        };

        let bar = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(24.0))
            .flex_none()
            .px_2()
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .text_size(px(12.0))
            .child(center)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_none()
                    .child(self.pane_toggle(
                        self.sidebar_open,
                        "icons/panel-left.svg",
                        "pane-toggle-sidebar",
                        cx.listener(|this, _event, _window, cx| {
                            this.sidebar_open = !this.sidebar_open;
                            cx.notify();
                        }),
                    ))
                    .child(self.pane_toggle(
                        self.info_open,
                        "icons/panel-right.svg",
                        "pane-toggle-info",
                        cx.listener(|this, _event, _window, cx| {
                            this.set_info_open(!this.info_open, cx);
                        }),
                    )),
            );
        bar.into_any_element()
    }
}

/// A layout style for `Entity::cached`, built from the same `div()` DSL used elsewhere. Caching
/// skips measuring the child, so the caller must describe its slot (e.g. `flex_1`/`size_full`).
fn cached_style(build: impl FnOnce(Div) -> Div) -> gpui::StyleRefinement {
    let mut element = build(div());
    std::mem::take(element.style())
}

fn is_valid_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// The blue rounded "+" badge used by the sort panel's add bar.
fn sort_plus_badge(theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(15.0))
        .h(px(15.0))
        .flex_none()
        .rounded_sm()
        .bg(rgb(theme.primary))
        .child(
            svg()
                .path("icons/plus.svg")
                .w(px(10.0))
                .h(px(10.0))
                .flex_none()
                .text_color(rgb(0xffffff)),
        )
}

/// The byte offset for a UTF-16 offset (used by the platform IME protocol).
fn offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf8 = 0;
    let mut utf16 = 0;
    for character in text.chars() {
        if utf16 >= offset {
            break;
        }
        utf16 += character.len_utf16();
        utf8 += character.len_utf8();
    }
    utf8
}

/// The UTF-16 offset for a byte offset (used by the platform IME protocol).
fn offset_to_utf16(text: &str, offset: usize) -> usize {
    let mut utf8 = 0;
    let mut utf16 = 0;
    for character in text.chars() {
        if utf8 >= offset {
            break;
        }
        utf8 += character.len_utf8();
        utf16 += character.len_utf16();
    }
    utf16
}

/// The built-in SQL editor font, matching VS Code's default monospace family for the platform.
pub(super) fn default_editor_font() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Consolas"
    }
    #[cfg(target_os = "macos")]
    {
        "Menlo"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "Droid Sans Mono"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        "Consolas"
    }
}

/// Install the app's editor chrome (background, foreground, gutter, active line) into gpui-kit's
/// editor highlight theme. The wrapped SQL editor (gpui-kit's `Editor`) takes these from the global
/// `highlight_theme`. The SQL syntax colors are intentionally left at gpui-kit's default palette so
/// the editor matches the table info pane's DDL view, which renders with that same palette.
fn install_editor_highlight_theme(theme: Theme, cx: &mut App) {
    use gpui::Hsla;
    use gpui_kit::component::ThemeMode;
    use gpui_kit::component::highlighter::HighlightTheme;

    let dark = theme.is_dark();
    let base = if dark {
        HighlightTheme::default_dark()
    } else {
        HighlightTheme::default_light()
    };
    let mut style_set = base.style.clone();
    style_set.editor_background = Some(Hsla::from(rgb(theme.editor_bg)));
    style_set.editor_foreground = Some(Hsla::from(rgb(theme.text)));
    style_set.editor_line_number = Some(Hsla::from(rgb(theme.text_muted)));
    style_set.editor_active_line_number = Some(Hsla::from(rgb(theme.text)));
    style_set.editor_active_line = Some(Hsla::from(rgb(theme.row_alt_bg)));
    // The gutter (line-number column) background, so it reads as a distinct column.
    style_set.editor_gutter_background = Some(Hsla::from(rgb(theme.header_bg)));
    // The editor's SQL syntax colors deliberately stay at gpui-kit's default palette: the table
    // info pane's DDL view renders its script with that same palette, so the query editor and the
    // DDL view read identically. Overriding the syntax map here (as we once did with `Theme::sql_*`)
    // gave the editor different keyword/identifier colors than the DDL script. Only the editor
    // chrome (background, foreground, gutter, active line) is themed below.

    gpui_kit::component::Theme::global_mut(cx).highlight_theme = Arc::new(HighlightTheme {
        name: if dark {
            "RustGrid Dark"
        } else {
            "RustGrid Light"
        }
        .to_string(),
        appearance: if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        style: style_set,
    });
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.theme = Theme::resolve(self.theme_setting, window.appearance());
        // Keep gpui-kit's own theme in lockstep with the app palette so its components (and the
        // shadcn tokens they read) follow the user's Light/Dark/System setting.
        let want_dark = self.theme.is_dark();
        if gpui_kit::component::Theme::global(cx).is_dark() != want_dark {
            gpui_kit::component::Theme::change(
                if want_dark {
                    gpui_kit::component::ThemeMode::Dark
                } else {
                    gpui_kit::component::ThemeMode::Light
                },
                Some(window),
                cx,
            );
        }
        if self.synced_input_theme != Some(self.theme) {
            self.sync_input_themes(cx);
            // Cached grids/designers don't see a parent re-render, so push the new theme to them.
            self.notify_grids(cx);
            // The editor's syntax colors and line-number gutter are owned by gpui-kit's highlight
            // theme, so install the app's palette over the kit default (see the helper).
            install_editor_highlight_theme(self.theme, cx);
            self.synced_input_theme = Some(self.theme);
        }
        self.sync_cached_children(cx);
        self.ensure_query_combos(cx);
        self.sync_db_combos(cx);
        self.sync_db_option_combos(cx);
        self.sync_query_combos(cx);
        self.sync_save_dialog_combos(cx);
        self.sync_language_combo(cx);
        self.sync_editor_combos(cx);
        self.sync_info(cx);
        self.sync_users(cx);
        // Check for a newer release once per launch. The result only becomes visible if there is
        // one (a titlebar button) or the user opens 选项 → 关于.
        if !self.update_checked {
            self.update_checked = true;
            self.check_for_updates(false, cx);
        }
        // Remember the main window so closing it can take the Backup/Restore window with it.
        if self.main_window_id.is_none() {
            self.main_window_id = Some(window.window_handle().window_id());
        }
        let theme = self.theme;

        // gpui does not re-run `render` when the window is resized, so the object list would
        // keep its initial rows-per-column. Observe bounds and force a follow-up frame so the
        // count is recomputed from the freshly measured scroll viewport.
        if self.window_bounds_subscription.is_none() {
            self.window_bounds_subscription =
                Some(cx.observe_window_bounds(window, |this, window, cx| {
                    // The cached object pane / grids don't re-render on their own when the
                    // viewport changes, so notify them (the pane's rows-per-column and the
                    // grids' scrollbar extents depend on it).
                    this.notify_object_pane(cx);
                    this.notify_grids(cx);
                    cx.notify();
                    cx.on_next_frame(window, |this, _window, cx| {
                        this.notify_object_pane(cx);
                        this.notify_grids(cx);
                        cx.notify();
                    });
                }));
        }

        if self.password_focus_pending {
            if let Some(prompt) = self.password_prompt.as_ref() {
                let handle = prompt.input.read(cx).focus_handle();
                window.focus(&handle, cx);
            }
            self.password_focus_pending = false;
        }

        if self.rename_focus_pending {
            if let Some(edit) = self.rename_edit.as_ref() {
                let handle = edit.input.read(cx).focus_handle();
                window.focus(&handle, cx);
            }
            self.rename_focus_pending = false;
        }

        if self.save_query_focus_pending {
            self.save_query_focus_pending = false;
            // The dialog opens on a later frame, so focus the field once more after that frame;
            // by then its lazily-created inner input state exists and will actually take input.
            if let Some(input) = self.query_name_input.clone() {
                cx.on_next_frame(window, move |_this, window, cx| {
                    input.update(cx, |input, cx| input.focus_state(window, cx));
                });
            }
        }

        if self.schema_focus_pending {
            self.schema_focus_pending = false;
            // Same as the save-query dialog: the field's inner gpui-kit state is created on the
            // dialog's first render, so focus it again on the next frame.
            if let Some(input) = self.schema_name_input.clone() {
                cx.on_next_frame(window, move |_this, window, cx| {
                    input.update(cx, |input, cx| input.focus_state(window, cx));
                });
            }
        }

        if self.query_focus_pending {
            window.focus(&self.query_focus, cx);
            self.query_focus_pending = false;
        }

        if self.backup_name_focus_pending {
            self.backup_name_focus_pending = false;
            if let Some(input) = self.backup_name_input.clone() {
                cx.on_next_frame(window, move |_this, window, cx| {
                    input.update(cx, |input, cx| input.focus_state(window, cx));
                });
            }
        }

        if self.backup_rename_focus_pending {
            if let Some(edit) = self.backup_rename.as_ref() {
                let handle = edit.input.read(cx).focus_handle();
                window.focus(&handle, cx);
            }
            self.backup_rename_focus_pending = false;
        }

        if self.query_rename_focus_pending {
            if let Some(edit) = self.query_rename.as_ref() {
                let handle = edit.input.read(cx).focus_handle();
                window.focus(&handle, cx);
            }
            self.query_rename_focus_pending = false;
        }

        // The wrapped gpui-kit editor owns its own caret blink, so the app no longer tracks an
        // editor blink loop here.

        let mut body = div().flex().flex_row().flex_1().w_full().overflow_hidden();
        if self.sidebar_open {
            body = body.child(self.sidebar_host.clone());
        }
        body = body.child(self.render_content(window, cx));
        if self.info_open {
            body = body.child(self.info_pane.clone());
        }

        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.window_bg))
            .text_color(rgb(theme.text))
            .text_size(px(12.5))
            // A side-pane divider sits on its pane's edge, so the pointer leaves the pane while
            // dragging. These root-level handlers keep the drag alive anywhere in the window and
            // update only the pane entities, so the whole app does not re-render per mouse move.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let host = this.sidebar_host.clone();
                host.update(cx, |host, cx| host.drag_move(event, cx));
                let pane = this.info_pane.clone();
                pane.update(cx, |pane, cx| pane.drag_move(event, cx));
                this.drag_query_split(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    let host = this.sidebar_host.clone();
                    host.update(cx, |host, cx| host.end_drag(cx));
                    let pane = this.info_pane.clone();
                    pane.update(cx, |pane, cx| pane.end_drag(cx));
                    this.end_query_split_drag(cx);
                }),
            )
            .child(self.render_titlebar(cx))
            .child(self.render_main_toolbar(cx))
            .child(body)
            .child(self.render_status_bar(cx));

        if self.theme_menu_open {
            root = root.child(self.render_theme_menu(cx));
        }

        if self.connect_menu_open {
            root = root.child(self.render_connect_menu(cx));
        }

        if let Some(menu) = self.context_menu.as_ref() {
            root = root.child(self.render_context_menu(menu, cx));
        }

        if let Some(menu) = self.tab_menu.as_ref() {
            root = root.child(self.render_tab_menu(menu, cx));
        }

        // Dialogs are hosted by `Root` (opened imperatively); AppView state remains the source
        // of truth and this reconciles it with the Root dialog stack.
        self.sync_dialog(window, cx);
        // Deliver any transient notifications queued during this frame.
        self.flush_toasts(window, cx);

        root
    }
}

fn tree_chevron(expanded: bool, color: u32) -> AnyElement {
    svg()
        .path(if expanded {
            "icons/chevron-down.svg"
        } else {
            "icons/chevron-right.svg"
        })
        .w(px(14.0))
        .h(px(14.0))
        .flex_none()
        .text_color(rgb(color))
        .into_any_element()
}

fn chevron_spacer() -> AnyElement {
    div().w(px(14.0)).flex_none().into_any_element()
}

/// A connection icon, drawn from the driver's descriptor: an engine brand mark, a white glyph on a
/// solid badge (whose colour carries the connection status), or a plain status-tinted glyph.
fn tree_driver_icon(
    icon: &'static str,
    style: DriverIconStyle,
    color: u32,
    badge: u32,
    connected: bool,
) -> AnyElement {
    match style {
        // Brand marks keep the engine colour only while the connection is open; a closed one is
        // greyed out (like the badge/plain styles) instead of staying colourful.
        DriverIconStyle::Brand(brand) => {
            tree_icon(icon, if connected { brand } else { color }).into_any_element()
        }
        DriverIconStyle::SolidBadge => div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(16.0))
            .h(px(16.0))
            .flex_none()
            .bg(rgb(badge))
            .child(
                svg()
                    .path(icon)
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(0xffffff)),
            )
            .into_any_element(),
        DriverIconStyle::Plain => tree_icon(icon, color).into_any_element(),
    }
}

/// A compact human-readable byte size for list columns, e.g. `512 B`, `2 KB`, `1.5 MB`.
pub(super) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 10.0 || value.fract() < 0.05 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A filesystem timestamp for list columns, e.g. `2026-09-18 11:20:15`.
pub(super) fn format_file_time(time: std::time::SystemTime) -> String {
    let time: chrono::DateTime<chrono::Local> = time.into();
    time.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Collapse free text (routine/table comments, which may contain newlines) into one line for a
/// single-line list cell.
pub(super) fn single_line(text: &str) -> String {
    text.replace(['\r', '\n', '\t'], " ").trim().to_string()
}

fn tree_icon(path: &'static str, color: u32) -> Svg {
    svg()
        .path(path)
        .w(px(16.0))
        .h(px(16.0))
        .flex_none()
        .text_color(rgb(color))
}

/// The asset path for a stored routine's icon: Navicat shows `fx` for functions and a distinct
/// code-block glyph for procedures.
pub(super) fn routine_icon(kind: RoutineKind) -> &'static str {
    match kind {
        RoutineKind::Function => "icons/function.svg",
        RoutineKind::Procedure => "icons/procedure.svg",
    }
}

/// The tint for a stored routine's icon, matched to [`routine_icon`].
pub(super) fn routine_icon_color(kind: RoutineKind, theme: Theme) -> u32 {
    match kind {
        RoutineKind::Function => theme.icon_functions,
        RoutineKind::Procedure => theme.icon_procedure,
    }
}

fn tree_message(text: String, indent: f32, color: u32) -> impl IntoElement {
    div()
        .w_full()
        .pl(px(indent))
        .pr_2()
        .py_0p5()
        .text_color(rgb(color))
        .child(text)
}

/// Toggle the window between maximized and restored.
///
/// gpui's `Window::zoom_window()` only maximizes on Windows (it calls `SW_MAXIMIZE`, a no-op when
/// the window is already maximized), so a second click on the maximize button did nothing. Use the
/// Windows helper, which calls `SW_RESTORE`, there; other platforms' `zoom()` already toggles.
pub(super) fn toggle_maximize(window: &mut Window) {
    #[cfg(target_os = "windows")]
    crate::win_resize::toggle_maximize(window);
    #[cfg(not(target_os = "windows"))]
    window.zoom_window();
}

pub(super) fn titlebar_button(
    id: &'static str,
    label: &str,
    theme: Theme,
    action: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(40.0))
        .h_full()
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
        .on_click(move |_event, window, cx| action(window, cx))
        .child(label.to_string())
}

fn is_temporal_type(data_type: &str) -> bool {
    let data_type = data_type.to_ascii_lowercase();
    data_type == "date" || data_type == "datetime" || data_type == "timestamp"
}

fn parse_datetime(value: &str) -> Option<NaiveDateTime> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    for format in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Some(parsed);
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return date.and_hms_opt(0, 0, 0);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_range_selection_unions_cells() {
        let mut selection = CellSelection::new(0, 0);
        selection.ranges.push(CellRange::new(2, 2));
        selection.active = selection.ranges.len() - 1;
        assert!(selection.contains(0, 0));
        assert!(selection.contains(2, 2));
        assert!(!selection.contains(1, 1));
        assert_eq!(selection.row_indices(), vec![0, 2]);
        assert_eq!(selection.col_indices(), vec![0, 2]);
        assert_eq!(selection.cells().len(), 2);
        assert_eq!(selection.active_cursor(), (2, 2));
    }

    #[test]
    fn rectangular_selection_covers_block() {
        let selection = CellSelection::single((1, 1), (2, 3));
        assert_eq!(selection.rows(), (1, 2));
        assert_eq!(selection.cols(), (1, 3));
        assert_eq!(selection.cells().len(), 6);
        assert!(selection.contains(2, 3));
        assert!(!selection.contains(0, 1));
    }
}
