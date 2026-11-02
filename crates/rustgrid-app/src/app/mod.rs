use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use gpui::{
    Anchor, AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle,
    DispatchPhase, Div, Entity, FocusHandle, FontWeight, HighlightStyle, ImageSource, KeyBinding,
    KeyDownEvent, Keystroke, ListHorizontalSizingBehavior, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, OperatingSystem, Pixels, Point, Render, Resource, ScrollDelta,
    ScrollEvent, ScrollHandle, ScrollStrategy, SharedString, Stateful, StyledText, Subscription,
    Svg, TextLayout, UniformListScrollHandle, WeakEntity, Window, WindowBounds, WindowHandle,
    WindowId, WindowOptions, anchored, canvas, deferred, div, img, prelude::*, px, relative, rgb,
    rgba, size, svg, uniform_list,
};
use rustgrid_config::{
    AppSettings, ConfigStore, ConnectionGroup, DEFAULT_EDITOR_FONT_SIZE, FieldLayout, GroupEntry,
    LanguageSetting, ThemeSetting,
};
use rustgrid_core::{
    AccountKind, BackupObjectKind, CellValue, ColumnInfo, Connection, ConnectionConfig,
    ConnectionFieldChoice, ConnectionFieldKind, ConnectionFieldSpec, ConnectionHomePage,
    ConnectionPage, ConnectionStandardField, DatabaseEditorSpec, DatabaseEditorTab,
    DatabaseOptions, DefaultObjectType, DefaultPrivilege, DriverCapability, DriverDialect,
    DriverIconStyle, DriverId, DriverRegistry, Error, FilterCondition, FilterConjunction,
    FilterGroup, FilterNode, FilterOperator, ObjectGrant, ObjectPrivilegeRow, PageRequest,
    PrivilegeCatalog, PrivilegeId, PrivilegePreset, PrivilegeScope, QueryResult, RoutineDetails,
    RoutineEdit, RoutineInfo, RoutineKind, RowInsert, RowUpdate, SavedBackup, SavedQuery,
    SavedTransfer, ServerSecurableGrant, TableStatus, TlsMode, TunnelAuth, TunnelKind, TunnelLayer,
    UserAccount, UserDetails, UserEdit, UserEditSection, UserEditorSpec, UserMapping, ViewEdit,
};
use rustgrid_export::ExportFormat;

use self::user::UserScope;
use crate::form::{ConnectionForm, FORM_FIELDS, FormField};
use crate::list_select::{ListSelection, MarqueeDrag, SelectMode, rects_intersect, selection_mode};
use crate::runtime::Runtime;
use crate::session::{
    Category, CategoryExpansion, CellRange, CellSelection, ConnectionNode, ConnectionStatus,
    DatabaseNode, EditAction, GridFieldLayout, GridState, Loadable, QueryResultPlan,
    QueryResultSummary, QueryTab, RoutineTab, RoutineTabState, RunEntry, RunScope, SortRule,
    ViewExplainTab, ViewTab, ViewTabState, compute_column_widths, page_column_widths,
};
use crate::sql::{self, SqlToken};
use crate::theme::{ConnectionColor, Theme};

// The in-place date/time picker is built from gpui-kit: the calendar supplies the date half
// (day/month/year views, navigation, selection and localization) and the `TimeField` the time
// half (segmented hours/minutes/seconds with keyboard editing).
use gpui_kit::component::Sizable;
use gpui_kit::component::calendar::CalendarState;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::time_field::{TimeField, TimeFieldState, TimePrecision};

use ui::{
    ButtonKind, ColumnGrid, ComboBox, ComboOption, DetailColumns, DetailScroll, DetailSort,
    SortKey, TextInput, TextInputOptions, ViewMode, checkbox_box, main_separator,
    scrollbar_fractions, scrollbar_thumb, toolbar_separator,
};

// Cell-navigation actions for the in-place grid editor. gpui-kit's `Root` binds `tab`/`shift-tab`
// to focus traversal in the `"Root"` context, and key bindings run before `on_key_down`; binding
// the same keys in a deeper context (the editing cell) takes precedence and moves the editor.
// Up/Down are additionally bound to `GridCell > Input` because the kit's own `MoveUp`/`MoveDown`
// handlers consume those keys for a single-line field without propagating.
gpui::actions!(grid, [NextCell, PrevCell, MoveCellUp, MoveCellDown]);

// Backup-list actions. `Root` binds `ctrl-c` to a copy action in the `"Root"` context, which
// would swallow the keystroke before `on_key_down`; binding our own actions in a deeper context
// (the backup list) takes precedence, like the grid's Tab binding above. `SelectAllBackups` lets
// Ctrl+A select every row, matching Explorer's list behavior.
gpui::actions!(
    backup,
    [
        CopyBackupFile,
        PasteBackupFile,
        RenameBackupFile,
        SelectAllBackups
    ]
);

// Saved-query file-list actions, mirroring the backup list's F2 / Ctrl+C / Ctrl+V / Ctrl+A
// handling.
gpui::actions!(
    queryfile,
    [
        CopyQueryFile,
        PasteQueryFile,
        RenameQueryFile,
        SelectAllQueries
    ]
);

// Transfer-configuration list actions: Enter opens the selected configuration, Delete deletes it
// and Ctrl+A selects every visible row, matching the other main-tab lists.
gpui::actions!(
    transferlist,
    [OpenTransferConfig, DeleteTransferConfig, SelectAllTransfers]
);

// Query-editor actions. The editor's own undo/redo bindings handle Ctrl+Z; `RunQuery` and
// `RunSelectedQuery` are the 运行 / 运行已选 pair (their keystrokes come from the settings, Navicat's
// Ctrl+R / Ctrl+Shift+R by default), and `SaveQuery` from Ctrl+S. `AcceptCompletion` lets Tab
// confirm the completion popup exactly like Enter does (the kit only handles Enter).
// `SelectAllOccurrences` (Ctrl+Shift+L) puts a cursor on every occurrence of the current word or
// selection, using gpui-kit 0.7.3's `InputState::add_selection`; `ShowCompletions` (Ctrl+Space) is
// the kit's editor action for requesting the completion menu on demand.
gpui::actions!(
    queryeditor,
    [
        RunQuery,
        RunSelectedQuery,
        SaveQuery,
        AcceptCompletion,
        SelectAllOccurrences,
        ToggleComment,
        DuplicateLines,
        SwapLineUp,
        MoveLinesUp,
        MoveLinesDown
    ]
);

// Workspace-wide commands, handled on the main window's root (`WORKSPACE_CONTEXT`), so they work
// from any pane.
gpui::actions!(
    workspace,
    [
        NewQueryTab,
        CloseActiveTab,
        CloseOtherTabs,
        CloseAllTabs,
        NextTab,
        PrevTab
    ]
);

/// The keymap's marker for a workspace-wide command: bound with no key context, so it fires
/// whatever has focus (or nothing), and handled at app level (`AppView::new`).
const WORKSPACE_CONTEXT: &str = "Workspace";

// Table-grid keyboard commands (the arrow keys stay on the grid's `on_key_down`). Shift variants
// extend the selection.
gpui::actions!(
    gridnav,
    [
        GridRefresh,
        GridEditValue,
        GridPageUp,
        GridPageDown,
        GridSelectPageUp,
        GridSelectPageDown,
        GridRowStart,
        GridRowEnd,
        GridFirstCell,
        GridLastCell,
        GridNextCell,
        GridPrevCell
    ]
);

/// Key context on the table grid (the in-place cell editor sits deeper, in `GridCell`, so its
/// own Tab / Home / End keep working while a cell is being edited).
const GRID_CONTEXT: &str = "TableGrid";

// The grid status bar's selectable SQL line. `Root` binds `ctrl-c` (copy) and `ctrl-a`
// (select-all), which would otherwise consume the keystroke before the line sees it; binding our
// own actions in a deeper context takes precedence, like the backup list above.
gpui::actions!(gridstatus, [CopyStatusSql, SelectAllStatusSql]);

// The table designer's own actions: 保存 from Ctrl+S, and Tab / Shift+Tab walking the fields grid's
// cells (`Root` binds `tab` to `focus_next`, and a bound keystroke never reaches an element's own
// `on_key_down`, so the grid claims it the same way the data grid does).
gpui::actions!(
    designer,
    [
        SaveDesign,
        NextDesignerCell,
        PrevDesignerCell,
        DesignerRowUp,
        DesignerRowDown,
        DesignerSelectAll,
        DesignerCopy
    ]
);

// Connection-tree keyboard navigation (see `sidebar.rs`). They are keymap commands, so every key is
// rebindable; typing a name to jump to it stays on the tree's `on_key_down`.
gpui::actions!(
    tree,
    [
        TreeUp,
        TreeDown,
        TreePageUp,
        TreePageDown,
        TreeHome,
        TreeEnd,
        TreeExpand,
        TreeCollapse,
        TreeOpen,
        TreeRename,
        TreeSelectAll
    ]
);

/// Key context applied to the connection tree's scroll area, for its navigation commands. The
/// in-place rename editor sits deeper (`ConnectionTree > Input`), so its own keys win while it is
/// open.
const TREE_CONTEXT: &str = "ConnectionTree";

// Object-list (Tables / Views / Functions pane) keyboard navigation (see `objects.rs`), in both
// the 详细列表 and the column-major 平铺网格 layouts. Keymap commands, like the tree's.
gpui::actions!(
    objectlist,
    [
        ObjectUp,
        ObjectDown,
        ObjectLeft,
        ObjectRight,
        ObjectPageUp,
        ObjectPageDown,
        ObjectHome,
        ObjectEnd,
        ObjectOpen,
        ObjectRename,
        ObjectSelectAll
    ]
);

/// Key context applied to the object list, for its navigation commands.
const OBJECT_LIST_CONTEXT: &str = "ObjectList";

/// How long keyboard navigation waits after the last move before it refreshes the rest of the app
/// (info pane, toolbar). Holding an arrow key then only redraws the list itself, like VS Code,
/// which fires its expensive selection events only once the focus settles.
const NAV_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

/// Key context applied to the table designer, so Ctrl+S saves it and Tab walks its cells while the
/// designer (or one of its cell editors) holds focus.
const DESIGNER_CONTEXT: &str = "TableDesigner";

/// Key context applied to a designer cell that owns the in-place editor.
const DESIGNER_CELL_CONTEXT: &str = "TableDesignerCell";

/// The designer cell editor's own `Input` context, for bindings that must outrank the kit's
/// text-input `MoveUp`/`MoveDown` (up/down move the selected row, like the table grid).
const DESIGNER_CELL_INPUT_CONTEXT: &str = "TableDesignerCell > Input";

/// Key context applied to the cell that owns the in-place editor.
const GRID_CELL_CONTEXT: &str = "GridCell";

/// The in-place editor's own `Input` context as seen from the editing cell, for key bindings that
/// must outrank the kit's text-input bindings (up/down cell movement).
const GRID_CELL_INPUT_CONTEXT: &str = "GridCell > Input";

/// Key context applied to the backup list, so F2 / Ctrl+C / Ctrl+V reach it.
const BACKUP_LIST_CONTEXT: &str = "BackupList";

/// Key context applied to the saved-query file list.
const QUERY_LIST_CONTEXT: &str = "QueryList";

/// Key context applied to the Transfer main tab's saved-configuration list.
const TRANSFER_LIST_CONTEXT: &str = "TransferList";

/// Key context applied to the grid status bar's selectable SQL line, so Ctrl+C / Ctrl+A reach it
/// instead of the `Root` context's copy/select-all.
const GRID_STATUS_CONTEXT: &str = "GridStatus";

/// Key context applied to the SQL query editor, so Ctrl+Z / Ctrl+Y / Ctrl+S reach it.
const QUERY_EDITOR_CONTEXT: &str = "QueryEditor";

/// The editor's own `Input` context as seen from the query editor, for key bindings that must tie
/// with the kit's text-input bindings and outrank them (later bindings win at equal depth).
const QUERY_EDITOR_INPUT_CONTEXT: &str = "QueryEditor > Input";

/// The stable settings key for the Queries tab's remembered list layout.
pub(super) const VIEW_PAGE_QUERIES: &str = "queries";

/// The stable settings key for the Users tab's remembered list layout.
pub(super) const VIEW_PAGE_USERS: &str = "users";

/// The stable settings key for the Backup tab's remembered list layout.
pub(super) const VIEW_PAGE_BACKUPS: &str = "backups";
/// The 传输 page's own list layout key.
pub(super) const VIEW_PAGE_TRANSFERS: &str = "transfers";

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

/// The Redis "新建键" modal (the prototype's `modalNewKey`).
struct NewKeyDialog {
    connection_index: usize,
    database_index: usize,
    name: String,
    /// `string` / `hash` / `list` / `set` / `zset` / `stream`.
    kind: String,
    ttl: String,
    /// The key's content (`string`/`list`/`set`), or the value half of a `hash`/`stream` entry.
    value: String,
    /// The field name of a `hash`/`stream` entry, or a sorted set's member.
    field: String,
    /// A sorted set member's score.
    score: String,
    error: Option<String>,
    saving: bool,
}

/// The Redis "设置 TTL" modal (the prototype's `modalTtl`).
struct KeyTtlDialog {
    connection_index: usize,
    database_index: usize,
    key: String,
    ttl: String,
    error: Option<String>,
    saving: bool,
}

/// State of the "export connections" window: the checked connections, whether passwords are
/// included, and an inline validation/write error.
struct ConnectionExportDialog {
    /// Indices into `AppView::connections` that are checked for export.
    selected: BTreeSet<usize>,
    /// Whether each connection's stored password is written into the file.
    include_passwords: bool,
    /// The last validation/write error, shown inline.
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
    /// Set by 暂停 and cleared by 继续; the run loop parks between objects while it is set.
    pause: Arc<std::sync::atomic::AtomicBool>,
    /// UI mirror of `pause`, so the footer shows 暂停 vs 继续 without reading the atomic.
    paused: bool,
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
    /// when the name or the object selection changes, or a run starts.
    config_saved: bool,
    /// The name and per-object selection the dialog opened with (after a saved configuration was
    /// applied), aligned with `objects`. The footer's 保存 stays disabled while both still match,
    /// so it only lights up once there is something to save.
    baseline: Option<(String, Vec<bool>)>,
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

/// Which parts of a database a *Dump SQL File* writes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DumpMode {
    /// Both the object `CREATE` DDL and the table data.
    StructureAndData,
    /// Only the object `CREATE` DDL.
    Structure,
    /// Only the table data (`INSERT` statements).
    Data,
}

impl DumpMode {
    fn includes_structure(self) -> bool {
        matches!(self, DumpMode::StructureAndData | DumpMode::Structure)
    }

    fn includes_data(self) -> bool {
        matches!(self, DumpMode::StructureAndData | DumpMode::Data)
    }
}

/// State of the "Dump SQL File" dialog: a whole-database `CREATE`+data dump written straight to a
/// `.sql` file, with the same async log/progress view as the backup dialogs.
struct DumpSqlDialog {
    connection_index: usize,
    database_index: usize,
    mode: DumpMode,
    /// The `.sql` file the dump writes to, chosen through the system save dialog.
    output_path: Option<std::path::PathBuf>,
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
    rows_total: usize,
    rows_done: usize,
    started: Option<std::time::Instant>,
    elapsed: Option<std::time::Duration>,
    error: Option<String>,
}

/// The encodings offered by the Run SQL File dialog's 编码 dropdown, paired with the
/// `encoding_rs` codec used to decode the script.
struct RunSqlEncoding {
    /// The label shown in the dropdown, e.g. `65001 (UTF-8)`.
    label: &'static str,
    encoding: &'static encoding_rs::Encoding,
}

/// The script encodings the Run SQL File dialog offers, in display order. UTF-8 is the default.
const RUN_SQL_ENCODINGS: [RunSqlEncoding; 6] = [
    RunSqlEncoding {
        label: "65001 (UTF-8)",
        encoding: encoding_rs::UTF_8,
    },
    RunSqlEncoding {
        label: "936 (GBK)",
        encoding: encoding_rs::GBK,
    },
    RunSqlEncoding {
        label: "54936 (GB18030)",
        encoding: encoding_rs::GB18030,
    },
    RunSqlEncoding {
        label: "950 (Big5)",
        encoding: encoding_rs::BIG5,
    },
    RunSqlEncoding {
        label: "932 (Shift-JIS)",
        encoding: encoding_rs::SHIFT_JIS,
    },
    RunSqlEncoding {
        label: "1252 (Windows-1252)",
        encoding: encoding_rs::WINDOWS_1252,
    },
];

/// State of the "Run SQL File" dialog: run a `.sql` script against a connection/database one
/// statement at a time, with a log and per-statement progress.
struct RunSqlDialog {
    connection_index: usize,
    database_index: usize,
    /// The schema the script runs under, for schema-qualified engines; empty otherwise.
    schema: String,
    /// The chosen script file.
    file_path: Option<std::path::PathBuf>,
    /// The index into [`RUN_SQL_ENCODINGS`] of the chosen encoding.
    encoding: usize,
    /// Whether a failing statement is logged and skipped rather than aborting the run.
    continue_on_error: bool,
    tab: BackupDialogTab,
    running: bool,
    /// Set by 停止 to abort the run loop early (checked between statements).
    cancel: Arc<std::sync::atomic::AtomicBool>,
    log: Vec<String>,
    /// Keeps the info log scrolled to the newest line while the operation runs.
    log_scroll: ScrollHandle,
    /// The number of statements the script split into (查询).
    total: usize,
    success: usize,
    failed: usize,
    /// The total number of rows the statements affected (记录).
    records: usize,
    started: Option<std::time::Instant>,
    elapsed: Option<std::time::Duration>,
    error: Option<String>,
    /// The dialog's 文件 field, built when the dialog opens.
    file_input: Option<Entity<TextInput>>,
    /// The dialog's 编码 dropdown, built when the dialog opens.
    encoding_combo: Option<Entity<ComboBox>>,
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

/// One table's resolved export run: the output path, the selected columns (names and engine types),
/// the primary-key ordering used for paged reads, and — when the source was a grid page or a query
/// result — the rows already in memory instead of a server read.
struct ExportPlan {
    name: String,
    path: String,
    columns: Vec<String>,
    column_types: Vec<String>,
    order_by: Vec<rustgrid_core::SortColumn>,
    snapshot: Option<ExportSnapshot>,
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

/// Rows already in hand for an export source — a table grid's current page, or one query result.
/// The wizard writes these straight to the file instead of paging the server again, so the
/// exported rows are exactly the ones the user was looking at (filtered, sorted, current page).
#[derive(Clone)]
struct ExportSnapshot {
    /// The source name, used in P2/P3 and as the default file stem.
    name: String,
    columns: Vec<ColumnInfo>,
    rows: Arc<Vec<Vec<CellValue>>>,
}

/// The 导出 confirmation shown before the wizard opens: a table grid chooses between the whole
/// table and the rows currently on screen; a query tab chooses one or more result sets.
struct ExportChoice {
    message: String,
    /// The choice labels, in order.
    options: Vec<String>,
    /// One flag per option. Single-select choices keep exactly one true; the query-result choice
    /// is a multi-select and may have several.
    selected: Vec<bool>,
    /// Whether the options are a multi-select dropdown (query results) instead of a radio list.
    multiple: bool,
    target: ExportChoiceTarget,
}

enum ExportChoiceTarget {
    TablePage {
        connection_index: usize,
        database_index: usize,
        table: String,
        page: ExportSnapshot,
    },
    QueryResults {
        connection_index: Option<usize>,
        database: String,
        dialect: DriverDialect,
        results: Vec<ExportSnapshot>,
    },
}

/// State of the "Export Wizard" window.
struct ExportWizard {
    connection_index: Option<usize>,
    /// The engine dialect (for `.sql` identifier quoting); kept even when no live connection is
    /// needed because every source is an in-memory [`ExportSnapshot`].
    dialect: DriverDialect,
    database: String,
    step: ExportStep,
    format: ExportFormat,
    /// The directory new output paths default into; editable on P2.
    output_dir: String,
    tables: Vec<ExportTablePlan>,
    /// P3's available fields per selected table, keyed by table name.
    fields: BTreeMap<String, ExportFields>,
    /// In-memory sources, keyed by the same name as `tables`. A table without an entry is paged
    /// from the server while the export runs.
    snapshots: BTreeMap<String, ExportSnapshot>,
    /// Several result sets opened from one query tab. When the format is `.xlsx` they are written
    /// into one workbook, one sheet per result, instead of one file each.
    combined: bool,
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
    Transfers,
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

/// The in-place "rename group" editor, drawn in the group's tree row. `name` mirrors the input's
/// text so `submit_group_rename` never reads the entity back during its own change callback.
struct GroupRenameEdit {
    id: u64,
    name: String,
    input: Entity<TextInput>,
}

/// What is being dragged over the connection tree.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum TreeDragPayload {
    /// A connection, by profile id.
    Connection(String),
    /// A group, by id.
    Group(u64),
}

/// An in-progress drag of a connection-tree row (one row, or the whole multi-selection).
#[derive(Clone)]
pub(super) struct TreeDrag {
    /// The entries being dragged: a single row, or every row of the tree's multi-selection.
    pub(super) payloads: Vec<TreeDragPayload>,
    /// The current pointer position, in window space.
    pub(super) position: Point<Pixels>,
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
    /// A connection group in the tree (新建连接 / 删除分组 / 重命名 / 刷新).
    Group {
        id: u64,
    },
    /// A multi-selection of connection and/or group rows in the tree (打开连接 / 关闭连接 /
    /// 导入连接 / 删除连接 / 刷新). Only shown when more than one row is selected.
    TreeSelection,
    /// The blank area of the connection tree (新建分组 / 刷新).
    TreeBlank,
    Database {
        connection_index: usize,
        database_index: usize,
    },
    /// A Redis namespace folder under a database (新建键 / 刷新). The key dialog starts with
    /// the folder's prefix in its name field.
    KeyNamespace {
        connection_index: usize,
        database_index: usize,
        namespace: String,
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
    /// A saved transfer configuration in the Transfer main tab's list.
    TransferConfig {
        index: usize,
    },
    /// The blank area of the Transfer main tab's list.
    TransferList,
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

/// A stable identity for an open tab (its grid / query / design id), so the opening order survives
/// the index shifts of closing other tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TabKey {
    Grid(u64),
    Query(u64),
    Design(u64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TabTarget {
    Grid(usize),
    Query(usize),
    Design(usize),
}

struct TabMenu {
    target: TabTarget,
    position: Point<Pixels>,
}

/// The window-space rectangles that keep an open submenu flyout (颜色, 管理分组, 转储 SQL 文件,
/// 导入连接) alive: the row that opened it and the flyout drawn beside that row.
///
/// A `popup_panel` occludes what is behind it, so the moment the pointer crosses from the trigger
/// row onto the flyout the row reports `hovered = false` — the row alone therefore cannot tell
/// "the pointer is on my flyout" from "the pointer left the menu", and the flyout used to stay up
/// until the whole menu was dismissed. Both rectangles together answer that question:
/// [`SubmenuZone::stays_open`] keeps the flyout up while the pointer is inside either one — they
/// abut, so the pair is one contiguous zone — and it closes as soon as the pointer leaves both.
#[derive(Clone, Copy, Default)]
struct SubmenuZone {
    /// The trigger row, recorded by a wrapper around it (a row's own bounds are not observable).
    row: Option<Bounds<Pixels>>,
    /// The flyout, the trigger row's last child, so its bounds are the ones that overflow the menu.
    flyout: Option<Bounds<Pixels>>,
}

impl SubmenuZone {
    /// Whether `position` is inside the trigger row or the flyout.
    fn contains(&self, position: &Point<Pixels>) -> bool {
        self.row.is_some_and(|bounds| bounds.contains(position))
            || self.flyout.is_some_and(|bounds| bounds.contains(position))
    }

    /// Whether the submenu stays open after its trigger row or its flyout reported `hovered`.
    /// Either element reports `false` while the pointer is on the other one (the flyout occludes
    /// the row), so the zone is what decides.
    fn stays_open(&self, hovered: bool, window: &Window) -> bool {
        hovered || self.contains(&window.mouse_position())
    }
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
    /// Whether the connection is a key-value engine (Redis): the Tables list is a key list with
    /// type/TTL/memory columns and its grid defaults to the detailed layout.
    key_value: bool,
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
    /// The Tables/Views 详细列表's active sort (点击表头 column).
    table_sort: DetailSort,
    /// The Functions 详细列表's active sort.
    routine_sort: DetailSort,
    /// The visible row keys of the last render, in display order (Shift-extend / marquee base).
    visible_keys: Vec<String>,
    /// Focus target for the list, so F2 reaches [`AppView::begin_rename_table`].
    focus: FocusHandle,
    theme: Theme,
    /// The layout of the last render, which decides what the arrow keys do.
    mode: ViewMode,
    /// The name typed so far to jump to an object, and when its last key arrived.
    typeahead: String,
    typeahead_at: Option<std::time::Instant>,
    /// Bumped by every keyboard move; the deferred info-pane / toolbar sync runs only for the
    /// latest.
    nav_generation: u64,
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
    /// Whether a connection-group rename editor is open somewhere in this tree. Typing then
    /// belongs to that editor, not to the tree's type-ahead.
    group_renaming: bool,
    /// Focus target for the tree, so F2 reaches [`AppView::begin_rename_table`].
    focus: FocusHandle,
    theme: Theme,
    /// The ids of the currently visible nodes, in draw order, for keyboard navigation.
    visible_ids: Vec<String>,
    /// The multi-selected connection/group row keys this frame, mirrored from `AppView` for row
    /// highlighting.
    selection: BTreeSet<String>,
    /// Every drawn row of the last frame, in draw order: what keyboard navigation walks (see
    /// `sidebar::NavRow`). Rebuilt each render from `nav_build`, which the row renderers fill.
    nav_rows: Vec<sidebar::NavRow>,
    nav_build: std::cell::RefCell<Vec<sidebar::NavRow>>,
    /// The nesting depth of the rows being drawn, for `NavRow::depth`.
    nav_depth: std::cell::Cell<usize>,
    /// Each drawn row's window-space bounds from the last prepaint, keyed by row id, so a
    /// keyboard move can scroll the selection into view.
    row_bounds: Rc<std::cell::RefCell<std::collections::HashMap<String, Bounds<Pixels>>>>,
    /// The name typed so far to jump to a row, and when its last key arrived.
    typeahead: String,
    typeahead_at: Option<std::time::Instant>,
    /// Bumped by every keyboard move; the deferred app-wide refresh runs only for the latest.
    nav_generation: u64,
}

/// The app-wide cell clipboard: the last block copied from any grid, with the SQL `NULL`s the OS
/// clipboard text cannot represent, plus the exact text written with it so a paste can tell our
/// own copy from an external one. Shared by every grid (`Rc`), so copying a record in one table
/// and pasting it in another keeps its `NULL`s.
#[derive(Default)]
struct GridClipboard {
    block: Option<Vec<Vec<Option<String>>>>,
    text: Option<String>,
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
    /// Whether the pressed cell was already the single selected cell (a click then edits it).
    cell_press_reselect: bool,
    /// F2's value editor panel, when open (see `grid_value.rs`).
    value_editor: Option<grid_value::ValueEditor>,
    cell_dragged: bool,

    cell_editor: Option<CellEditor>,
    cell_editor_blur_subscription: Option<Subscription>,
    cell_editor_focus_pending: bool,
    date_picker: Option<DatePicker>,

    /// The dedicated key page's string editor (RAW), the pretty-printed JSON preview, and their
    /// state. Only used while this grid shows a `string` / `json` key.
    key_string_format: KeyStringFormat,
    key_string_editor: Option<Entity<ui::SqlEditor>>,
    key_json_editor: Option<Entity<ui::SqlEditor>>,
    key_string_text: String,
    key_string_loaded: Option<String>,
    key_string_dirty: bool,
    key_string_saving: bool,
    key_string_error: Option<String>,

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
    /// records. Each cell is `None` for SQL `NULL`. Shared by every grid, so a copy survives a
    /// tab switch and keeps its `NULL`s when pasted into another table.
    clipboard: Rc<RefCell<GridClipboard>>,
    /// Set by an explicit 刷新 (toolbar / context menu / F5) so the fetch that follows confirms
    /// itself with a toast; page turns and post-write reloads leave it off.
    refresh_feedback: bool,

    /// The single-line editors of the filter builder's value fields, keyed by `(node path, slot)`.
    /// Rebuilt after any structural change so the paths stay valid. A real [`TextInput`] gives the
    /// value fields caret/mouse selection and clipboard support for free.
    filter_value_inputs: BTreeMap<(Vec<usize>, u8), Entity<TextInput>>,
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

    /// The pager's fields: the model text each one commits on Enter, and the shared
    /// [`TextInput`]s that own caret, selection, clipboard and IME.
    page_input: String,
    page_input_view: Entity<TextInput>,
    page_size_menu_open: bool,
    page_size_input: String,
    page_size_view: Entity<TextInput>,
    page_size_focus_pending: bool,

    /// Whether the 字段筛选 popup is open.
    field_filter_open: bool,
    /// Focus the popup's search field on the next frame, once it is on screen.
    field_filter_focus_pending: bool,
    /// The popup's search text and its shared field.
    field_filter_search: String,
    field_filter_search_input: Entity<TextInput>,
    /// The popup's working layout (display order — every field, hidden ones included — plus the
    /// hidden set) while it is open. Edits land here, never on the grid, until 确定 installs them.
    field_filter_draft: GridFieldLayout,
    /// The field being dragged to a new position, with the pointer y captured on mouse-down.
    field_filter_drag: Option<FieldFilterDrag>,
    /// Where the 字段筛选 trigger sits (window space), so its popup can be anchored under it.
    field_filter_anchor: Rc<RefCell<Point<Pixels>>>,
    /// The grid pane's own window-space bounds, so the popup can be clamped inside it.
    grid_pane_bounds: Rc<RefCell<Bounds<Pixels>>>,

    /// The status bar's SQL line is hand-selectable (drag, double-click to select all, Ctrl+C),
    /// mirroring the database dialog's SQL preview: the laid-out text is kept so a pointer
    /// position maps to a byte offset.
    status_sql_focus: FocusHandle,
    status_sql_layout: TextLayout,
    status_sql_text: String,
    status_sql_anchor: usize,
    status_sql_cursor: usize,
    status_sql_selecting: bool,

    /// Height of the body area left for rows, measured each frame. When the page has no rows the
    /// grid paints an empty table (gutter + column separators) tall enough to fill it.
    empty_grid_height: f32,

    /// A weak handle to this view, used for the pager's field callbacks and the toolbar's
    /// deferred work without forming an `Entity` reference cycle.
    self_weak: WeakEntity<GridView>,
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

/// Which representation the string key page shows: the editable raw value, or a read-only
/// pretty-printed JSON view of it (the prototype's RAW / JSON switch).
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyStringFormat {
    Raw,
    Json,
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

/// An in-progress drag of a 字段筛选 row, reordering the grid's fields. The list is a fixed row
/// height, so the target position comes from how many rows the pointer has travelled from where
/// the drag began — no row rectangles are needed.
#[derive(Clone, Copy)]
struct FieldFilterDrag {
    /// Where the dragged field started, in the working order.
    from: usize,
    /// Where it currently sits.
    current: usize,
    start_y: f32,
}

/// Pending destructive action that needs confirmation before it runs.
enum DeleteConfirm {
    /// Delete the selected rows of a grid.
    Rows { grid_id: u64, rows: Vec<usize> },
    /// Delete a connection (and its open grids/query tabs).
    Connection { index: usize },
    /// Delete a connection-tree group (its connections are kept, only ungrouped).
    Group { id: u64 },
    /// Delete the connection tree's multi-selection: the named connections plus the named groups
    /// (each group is removed with everything left inside it).
    TreeSelection {
        connections: Vec<String>,
        groups: Vec<u64>,
    },
    /// A destructive table operation.
    Table {
        connection_index: usize,
        database_index: usize,
        name: String,
        operation: TableOperation,
    },
    /// Drop several selected tables at once.
    Tables {
        connection_index: usize,
        database_index: usize,
        names: Vec<String>,
    },
    /// Empty or truncate several selected tables at once.
    TablesOperation {
        connection_index: usize,
        database_index: usize,
        names: Vec<String>,
        operation: TableOperation,
    },
    /// Delete a saved query from the saved-query list.
    SavedQuery { index: usize },
    /// Delete several saved queries at once (the list's multi-selection).
    SavedQueries { indices: Vec<usize> },
    /// Delete a backup file from disk.
    BackupFile { index: usize },
    /// Delete a saved backup configuration.
    BackupConfig { index: usize },
    /// Delete one or more saved 传输 configurations.
    TransferConfigs { indices: Vec<usize> },
    /// Delete several backup files and/or saved configurations at once (the list's
    /// multi-selection).
    BackupEntries {
        files: Vec<usize>,
        configs: Vec<usize>,
    },
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
        /// Whether the account is a role, so the confirmation asks about a 角色 rather than a 用户.
        role: bool,
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
    /// SQL Server's 唯一键 grid.
    UniqueKeys,
    /// The check-constraint grid.
    Checks,
    /// PostgreSQL's exclusion-constraint grid.
    Excludes,
    /// PostgreSQL's rewrite-rule grid.
    Rules,
    /// The trigger grid.
    Triggers,
}

#[derive(Clone, Copy)]
enum TableOperation {
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
    /// The 导出 scope choice (all records vs the grid's page, or which query result).
    ExportChoice,
    /// The Redis 新建键 modal.
    NewKey,
    /// The Redis 设置 TTL modal.
    KeyTtl,
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
    /// Disconnect several connections at once (the tree's multi-selection).
    DisconnectMany { indexes: Vec<usize> },
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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MainTab {
    Tables,
    Views,
    Functions,
    Users,
    Queries,
    Backups,
    Sync,
}

const MAIN_TABS: [(MainTab, &str, &str); 7] = [
    (MainTab::Tables, "icons/tables.svg", "main.tables"),
    (MainTab::Views, "icons/views.svg", "main.views"),
    (MainTab::Functions, "icons/functions.svg", "main.functions"),
    (MainTab::Users, "icons/user.svg", "main.users"),
    (MainTab::Queries, "icons/queries.svg", "main.queries"),
    (MainTab::Backups, "icons/backups.svg", "main.backups"),
    (MainTab::Sync, "icons/sync.svg", "main.sync"),
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
/// Filter-builder metrics, a compact take on the reference design: 24px controls, a 66px left
/// rail for the in-group conjunction control, 22px row actions, and a 24px between-groups bar
/// carrying the floating control with 18px actions in a balanced slot.
const FILTER_CONTROL_HEIGHT: f32 = 24.0;
const FILTER_CONTROL_TEXT_SIZE: f32 = 12.0;
const FILTER_CONTROL_RADIUS: f32 = 6.0;
const FILTER_RAIL_WIDTH: f32 = 66.0;
const FILTER_FIELD_WIDTH: f32 = 112.0;
const FILTER_OPERATOR_WIDTH: f32 = 84.0;
/// Minimum width of the value field (it stretches to fill the rest of the row).
const FILTER_VALUE_WIDTH: f32 = 110.0;
const FILTER_ROW_ACTION_SIZE: f32 = 22.0;
const FILTER_ROW_ACTION_RADIUS: f32 = 6.0;
const FILTER_BOUNDARY_BAR_HEIGHT: f32 = 24.0;
const FILTER_BOUNDARY_ACTION_SIZE: f32 = 18.0;
const FILTER_BOUNDARY_ACTION_RADIUS: f32 = 4.0;
/// One side of the boundary bar's balanced slot (two 18px actions + their 4px gap).
const FILTER_BOUNDARY_ACTION_SLOT: f32 = 40.0;
/// The filter content fills the pane (responsive) up to this cap so a very wide window does not
/// stretch the value field and fling the row actions to the far edge. The divider on the block's
/// right edge drags a manual width between the min and max; double-clicking it restores auto-fit.
const FILTER_CONTENT_WIDTH_DEFAULT: f32 = 700.0;
const FILTER_CONTENT_WIDTH_MIN: f32 = 520.0;
const FILTER_CONTENT_WIDTH_MAX: f32 = 700.0;
/// Clamp for a column dragged to its narrowest/widest.
const MIN_COLUMN_WIDTH: f32 = 32.0;
const MAX_COLUMN_WIDTH: f32 = 1200.0;
/// The row-header strip the table grid draws before its first column. It holds the current row's
/// marker and a full-row selection's tint and nothing else, so it stays as narrow as the marker
/// needs (the designer grids use the same 16px, `DESIGN_GUTTER_WIDTH`).
const GRID_GUTTER_WIDTH: f32 = 16.0;
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
    /// The 快捷键 page: the query editor's run shortcuts (Navicat's Ctrl+R / Ctrl+Shift+R).
    Shortcuts,
    /// The 数据 page: grid display preferences (the field-comment toggle).
    Data,
    About,
}

/// The height of one extra grid-header line: a field's type or comment painted under its name.
const GRID_HEADER_LINE_HEIGHT: f32 = 14.0;
/// The height of the grid's toolbar row.
const GRID_TOOLBAR_HEIGHT: f32 = 28.0;
/// The deferred paint priority of the 字段筛选 popup and its click-away backdrop. Both are painted
/// above the grid body; the panel sits above its backdrop.
const FIELD_FILTER_BACKDROP_PRIORITY: usize = 98;
const FIELD_FILTER_POPUP_PRIORITY: usize = 99;
/// The width of the 字段筛选 popup panel.
const FIELD_FILTER_POPUP_WIDTH: f32 = 252.0;
/// The height of one row of the 字段筛选 popup's field list.
const FIELD_FILTER_ROW_HEIGHT: f32 = 24.0;
/// The tallest the 字段筛选 popup's field list grows before it scrolls.
const FIELD_FILTER_LIST_MAX_HEIGHT: f32 = 620.0;
/// The popup's fixed chrome (header, search box, footer and the borders between them), reserved
/// out of the pane height when sizing the field list.
const FIELD_FILTER_CHROME_HEIGHT: f32 = 124.0;

pub struct AppView {
    registry: Arc<DriverRegistry>,
    config: Arc<ConfigStore>,
    /// The last session's tables not yet reopened (their connection is not connected yet), and
    /// its active tab; see `workspace.rs`.
    pending_session_tables: Vec<rustgrid_config::SessionTable>,
    pending_session_active: Option<rustgrid_config::SessionActive>,
    /// The last session's open databases and current database, until their connection connects.
    pending_session_databases: Vec<rustgrid_config::SessionDatabase>,
    pending_session_current: Option<rustgrid_config::SessionDatabase>,
    /// Whether the last session was restored (before that, nothing is saved, so an early save
    /// cannot overwrite it), the fingerprint last written, and whether a save is scheduled.
    session_restored: bool,
    session_signature_saved: u64,
    session_save_pending: bool,
    runtime: Arc<Runtime>,
    connections: Vec<ConnectionNode>,
    grids: Vec<Entity<GridView>>,
    designs: Vec<Entity<design::TableDesignView>>,
    /// The open tabs in the order they were opened, shared by the tab bar and Ctrl+Tab. Reconciled
    /// on read (`ordered_tabs`): closed tabs drop out, new ones join at the end.
    tab_sequence: RefCell<Vec<TabKey>>,
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
    /// A repaint is scheduled for the running query's timer.
    query_timer_scheduled: bool,
    /// The Queries 详细列表's current sort column and direction.
    query_sort: QuerySort,
    /// The Queries 平铺网格's horizontal scroll state.
    query_grid: ColumnGrid,
    /// Saved backup configurations loaded from `backups.json`.
    backup_configs: Vec<SavedBackup>,
    /// Saved 传输 configurations loaded from `transfers.json`.
    transfer_configs: Vec<SavedTransfer>,
    /// Backup files found under the config dir's `backups/` tree.
    backup_files: Vec<BackupFileInfo>,
    /// The 数据传输/同步 page's state.
    sync: sync::SyncState,
    /// The backup-list entry highlighted in the Backup main tab.
    backup_selected: Option<BackupSelection>,
    /// The open "new backup" dialog, if any.
    new_backup_dialog: Option<NewBackupDialog>,
    /// The open "restore backup" dialog, if any.
    restore_dialog: Option<RestoreBackupDialog>,
    /// The open "extract SQL" dialog, if any.
    extract_dialog: Option<ExtractSqlDialog>,
    /// The open "Dump SQL File" dialog, if any.
    dump_dialog: Option<DumpSqlDialog>,
    /// The open "Run SQL File" dialog, if any.
    run_sql_dialog: Option<RunSqlDialog>,
    /// The database context menu's 转储 SQL 文件 flyout target `(connection, database)`, while open.
    dump_submenu: Option<(usize, usize)>,
    /// The 转储 SQL 文件 flyout's zone while it is shown. The flyout overflows past the menu
    /// panel, so the panel's `on_mouse_down_out` would otherwise close the menu on a click that
    /// actually lands on the flyout (before the item's click can complete); it also keeps the
    /// flyout up while the pointer moves between the trigger row and the flyout.
    dump_zone: Rc<RefCell<SubmenuZone>>,
    /// Whether the 导入连接 submenu (in the New Connection dropdown and the tree's blank menu) is
    /// open, and the zone of its flyout (mirrors `dump_zone`).
    import_submenu: bool,
    import_zone: Rc<RefCell<SubmenuZone>>,
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
    /// The open "export connections" window, if any.
    export_connections_dialog: Option<ConnectionExportDialog>,
    /// The OS window hosting the "export connections" dialog, if open.
    export_connections_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// The open "new table" name prompt, if any, and its shared name input.
    create_table_dialog: Option<CreateTableDialog>,
    create_table_input: Option<Entity<TextInput>>,
    /// The Redis 新建键 modal and its fields.
    new_key_dialog: Option<NewKeyDialog>,
    new_key_name_input: Option<Entity<TextInput>>,
    new_key_ttl_input: Option<Entity<TextInput>>,
    new_key_type_combo: Option<Entity<ComboBox>>,
    /// The modal's value / field / score fields; which of them shows depends on the key type.
    new_key_value_input: Option<Entity<TextInput>>,
    new_key_field_input: Option<Entity<TextInput>>,
    new_key_score_input: Option<Entity<TextInput>>,
    /// The Redis 设置 TTL modal and its TTL field.
    key_ttl_dialog: Option<KeyTtlDialog>,
    key_ttl_input: Option<Entity<TextInput>>,
    /// The save-query dialog's connection and database pickers.
    save_connection_combo: Option<Entity<ComboBox>>,
    save_database_combo: Option<Entity<ComboBox>>,
    /// The last database names seen per connection profile id, so the save dialog can offer
    /// databases without opening the connection.
    database_cache: BTreeMap<String, Vec<String>>,
    save_query_focus_pending: bool,
    query_focus: FocusHandle,
    query_focus_pending: bool,
    /// Frames waited for a new query's editor to exist before focusing it.
    query_focus_attempts: u8,
    /// Give the active table grid the keyboard focus on the next frame (after a tab switch).
    grid_focus_pending: bool,
    /// One gpui-kit editor per query tab, index-aligned with `queries` and `None` until built.
    /// `ui::SqlEditor` owns the text, undo/redo, multi-cursor and search; `QueryTab::sql` mirrors
    /// it for the run/save/format paths.
    query_editors: Vec<Option<Entity<ui::SqlEditor>>>,
    /// Read-only SQL preview editors (routine/view ?? tabs), keyed by tab fingerprint.
    preview_editors: std::collections::HashMap<String, Entity<ui::SqlEditor>>,
    /// The diff highlights last applied to each read-only preview (the transfer window's DDL
    /// panes), so they are pushed again only when the text or the marks change.
    preview_decorations:
        std::collections::HashMap<String, Vec<(std::ops::Range<usize>, gpui::HighlightStyle)>>,
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
    /// The engine-specific connection inputs, keyed by their `ConnectionProfile::options` key.
    form_engine_inputs: BTreeMap<String, Entity<TextInput>>,
    /// The engine-specific `Select` field whose dropdown is open, keyed by its option key.
    form_engine_select_open: Option<String>,
    /// Where the open engine `Select`'s trigger sits, so its menu can be positioned.
    form_engine_select_anchor: Rc<RefCell<Point<Pixels>>>,
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
    /// Whether the grid header shows each column's comment under its name. Set from the 数据 page
    /// of the Options window; off by default.
    grid_show_comments: bool,
    /// Whether the grid header shows each column's data type between its name and comment. Set from
    /// the 数据 page of the Options window; off by default.
    grid_show_types: bool,
    /// The remembered field layout (字段筛选) per table grid, keyed by connection/database/table,
    /// persisted in settings.
    grid_field_layouts: BTreeMap<String, FieldLayout>,
    /// The user's keymap overrides (command id -> keystrokes), edited on the Options window's
    /// 快捷键 page; see [`keymap`].
    shortcuts: BTreeMap<String, Vec<String>>,
    /// Keep the object search when switching tabs or databases (Options → General).
    preserve_object_search: bool,
    /// Restore the previous session at startup: connections, query tabs, open databases and table
    /// tabs (Options → General, 启动时恢复). Read once at startup, like the settings themselves.
    restore_session: bool,
    /// Open connections, databases, tables and the other tree/list objects with a single click
    /// (Options → General, 单击模式; off means double-click, the default).
    single_click_open: bool,
    /// The most statements one query run may execute, 0 for no limit (Options → General).
    max_statements_per_run: u32,
    /// Stop a script at its first failing statement (Options → General).
    stop_on_error: bool,
    /// Each query tab's 信息 text, in a read-only editor (select, Shift+arrows, Ctrl+C), by tab id.
    query_info_editors: std::collections::HashMap<u64, Entity<ui::SqlEditor>>,
    /// The tab shown last frame, so a tab change (by click, Ctrl+Tab or a close) can move the
    /// keyboard focus to the newly shown pane and, unless the search is kept, clear it.
    last_shown_tab: Option<Option<TabTarget>>,
    /// The keystrokes (and contexts) the keymap was last bound with, so a change in the Options
    /// window can shadow the old ones (the keymap has no removal API).
    bound_keys: keymap::BoundKeys,
    /// The search text of each list that shares the object search box (表 / 视图 / 函数 / 查询),
    /// keyed by its page id. Every list keeps its own search, so 表's text never filters 视图 or
    /// 查询; 保留对象搜索 keeps them while the shown page, database or connection changes, and with
    /// it off a switch clears all of them.
    object_searches: BTreeMap<String, String>,
    /// The page whose text `object_search_input` currently shows, so switching pages swaps the
    /// field's content to that list's own search (see [`AppView::sync_object_search`]).
    search_box_page: Option<&'static str>,
    object_search_input: Entity<TextInput>,
    /// Scroll position of the backup/restore object picker, so it can show a vertical scrollbar.
    backup_objects_scroll: ScrollHandle,
    delete_confirm: Option<DeleteConfirm>,
    error_dialog: Option<String>,
    /// The cell clipboard every grid shares (see [`GridClipboard`]).
    grid_clipboard: Rc<RefCell<GridClipboard>>,
    /// Transient notifications queued for the next frame, which has the window needed to hand
    /// them to `Root`.
    pending_toasts: Vec<(ToastKind, String)>,
    /// A close/navigate action waiting on the "discard unsaved changes?" confirmation.
    unsaved_confirm: Option<PendingAction>,
    /// Whether the currently-open modal dialog has already given its first field the focus.
    ///
    /// `Root` re-invokes a dialog's builder every frame, so a one-shot "focus the name field"
    /// guard must live outside the builder. Held here (one dialog is open at a time) so the
    /// guard survives across frames instead of re-stealing focus from the dialog's other
    /// controls — most visibly the search box of a combo box.
    dialog_focus_done: bool,
    /// The kind of dialog last opened through `Root`, if any.
    opened_dialog: Option<DialogKind>,
    /// Shared text-selection handle for every `[Msg]` log view (backup/restore/extract/dump/run
    /// SQL). One handle per window is enough — only one log is on screen at a time — and reusing it
    /// keeps a selection alive across the dialog's per-frame rebuilds while letting a drag select
    /// and copy across log lines.
    log_handle: gpui_kit::base::TextSelectionHandle,
    /// Where the log box's right-click menu (复制 / 全选) is open, in the backup window's space.
    /// Drawn by `backup_window_contents` so it survives that window's per-frame rebuilds.
    log_menu: Option<Point<Pixels>>,
    /// Whether the log's 全选 is active: every line is tinted as selected and 复制 takes the whole
    /// log. Cleared by a plain click in the log.
    log_select_all: bool,
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
    /// Whether Ctrl + mouse wheel resizes the SQL editor's font (Options → 编辑器).
    editor_ctrl_wheel_zoom: bool,
    /// The Options window's selected left-nav page.
    options_section: OptionsSection,
    /// Staged editor settings, applied on OK so Cancel discards them.
    options_editor_font_family: String,
    options_editor_font_size: u32,
    options_editor_line_numbers: bool,
    options_editor_word_wrap: bool,
    /// The staged 编辑器 page toggle (Ctrl + wheel zooms the editor font), applied on OK.
    options_editor_ctrl_wheel_zoom: bool,
    /// The staged 数据 page toggle (grid field comments), applied on OK.
    options_grid_show_comments: bool,
    /// The staged 数据 page toggle (grid field types), applied on OK.
    options_grid_show_types: bool,
    /// The staged 常规 page toggle (keep the object search across tab switches), applied on OK.
    options_preserve_object_search: bool,
    /// The staged 常规 page toggle (restore the session at the next start), applied on OK.
    options_restore_session: bool,
    /// The staged 常规 page toggle (single-click opens objects), applied on OK.
    options_single_click_open: bool,
    /// The Users 详细列表's active sort (点击表头 column).
    users_sort: DetailSort,
    /// The Backup 详细列表's active sort (点击表头 column).
    backup_sort: DetailSort,
    /// The staged 常规 page statement limit (as typed) and stop-on-error toggle, applied on OK.
    options_max_statements: String,
    options_max_statements_input: Option<Entity<TextInput>>,
    options_stop_on_error: bool,
    /// The Options window's right-click menu on a setting: where, and the setting's label.
    options_copy_menu: Option<(Point<Pixels>, String)>,
    /// The setting the Options window points out (e.g. from the query's statement-limit notice).
    options_highlight: Option<&'static str>,
    /// The staged 快捷键 page overrides, applied on OK.
    options_shortcuts: BTreeMap<String, Vec<String>>,
    /// The id of the 快捷键 row that is waiting for a keystroke, if any.
    recording_shortcut: Option<&'static str>,
    /// One focus target per [`keymap::COMMANDS`] row, so clicking a capture field starts recording.
    options_shortcut_focus: Vec<FocusHandle>,
    /// The 快捷键 page's search box and its current text.
    options_shortcut_search: Option<Entity<TextInput>>,
    options_shortcut_filter: String,
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
    /// The newer release's notes (its GitHub release body), which the check reads along with the
    /// version. Only the release can carry them: the running build has no entry for a version newer
    /// than itself. Drives the changelog at the foot of 关于, which appears exactly while this is
    /// `Some` (see `update::changelog_panel`).
    update_notes: Option<String>,
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
    /// The connection tree's layout: nested groups holding connections (by profile id), in display
    /// order. Persisted in settings; the `connections` vector holds the live nodes themselves.
    tree_layout: Vec<GroupEntry>,
    /// The per-connection environment colour, keyed by profile id (see [`ConnectionColor`]).
    /// Persisted in settings; an absent entry means no colour.
    connection_colors: BTreeMap<String, String>,
    /// The group whose in-place rename editor is open, if any.
    group_rename: Option<GroupRenameEdit>,
    group_rename_focus_pending: bool,
    /// The group a to-be-created connection is filed under (set by the group menu's 新建连接).
    pending_group: Option<u64>,
    /// The connection whose 管理分组 flyout is open, if any.
    move_submenu: Option<usize>,
    /// The 管理分组 flyout's zone while it is shown, so an outside click is not mistaken for a
    /// click on the flyout (mirrors `dump_zone`).
    move_zone: Rc<RefCell<SubmenuZone>>,
    /// The connection whose 颜色 flyout is open, if any.
    color_submenu: Option<usize>,
    /// The 颜色 flyout's zone while it is shown (mirrors `move_zone`).
    color_zone: Rc<RefCell<SubmenuZone>>,
    /// When the New Connection engine picker was opened from a group, the group the new connection
    /// should be filed under. `None` when opened from the toolbar.
    connect_menu_group: Option<u64>,
    /// A pending row press that may become a drag once the pointer moves far enough. The payloads
    /// are the pressed row alone, or the whole tree multi-selection when the pressed row is part
    /// of one.
    tree_press: Option<(Vec<TreeDragPayload>, Point<Pixels>)>,
    /// The in-progress drag over the connection tree, if any.
    tree_drag: Option<TreeDrag>,
    /// Window-space rectangles of the draggable tree rows (connections and groups), for the
    /// drag-and-drop hit test.
    tree_row_rects: std::collections::HashMap<String, Bounds<Pixels>>,
    /// The connection tree's multi-selection of connection/group rows (Explorer-style). Kept
    /// separate from the tree pane's active row (`TreePane::selected`), which still drives the
    /// content-pane scoping.
    tree_selection: ListSelection,
    /// The in-progress rubber-band drag over the connection tree, if any.
    tree_marquee: Option<MarqueeDrag>,
    /// The connection tree's window-space bounds (published by the tree pane), so the rubber band
    /// is clamped to it.
    tree_bounds: Option<Bounds<Pixels>>,
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
    /// Which principals the Users tab lists: the engine's users/logins or its roles.
    users_scope: UserScope,
    /// Whether the **server** supports roles, as reported when the account list loaded. A server
    /// without them (MySQL ≤ 5.7) hides the 用户 / 角色 switch instead of offering an empty list.
    users_roles_supported: bool,
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
    /// The 传输 configurations list's multi-selection (Explorer-style).
    transfers_selection: ListSelection,
    /// The 传输 configurations list's row rectangles in window space, for the marquee.
    transfers_row_rects: std::collections::HashMap<String, Bounds<Pixels>>,
    /// The OS window hosting the 传输 workbench, if open.
    sync_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// Focus target for the 传输 window, so ESC closes it before any control is focused.
    sync_window_focus: FocusHandle,
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
    /// The dialog's 类型 dropdown (用户 / 组 / 角色), for engines that separate them.
    create_user_kind_combo: Option<Entity<ComboBox>>,
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
    /// The 导出 scope confirmation waiting for the user (table page vs all records, or which
    /// query results), shown before the wizard window opens.
    export_choice: Option<ExportChoice>,
    /// The multi-select dropdown of [`ExportChoice`] when the choice is over query results. It is
    /// an entity of its own so the popup keeps its state across the dialog's per-frame renders.
    export_choice_combo: Option<Entity<ComboBox>>,
    /// The OS window hosting the export wizard, if open.
    export_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// The open import wizard, if any.
    import_wizard: Option<ImportWizard>,
    /// The OS window hosting the import wizard, if open.
    import_window: Option<WindowHandle<gpui_kit::component::Root>>,
    /// Focus targets for the child windows, so ESC closes them before any control is focused.
    export_focus: FocusHandle,
    import_focus: FocusHandle,
    export_connections_focus: FocusHandle,
    backup_window_focus: FocusHandle,
    /// The main window's id, so closing it also closes the Backup/Restore window.
    main_window_id: Option<WindowId>,
    /// Keeps the window-closed listener alive, so closing the OS window clears the dialog state.
    _window_closed: Subscription,
}

mod backup;
mod changelog;
mod connection_form;
mod database;
mod db_dialog;
mod design;
mod design_view;
mod dialogs;
mod dump;
mod export;
mod form;
mod grid;
mod grid_cell;
mod grid_commit;
mod grid_fields;
mod grid_input;
mod grid_key;
mod grid_menu;
mod grid_scroll;
mod grid_toolbar;
mod grid_value;
mod grid_view;
mod groups;
mod import;
mod info_pane;
mod keymap;
pub(crate) mod line_edit;
mod list_ops;
mod objects;
mod options;
mod privilege_manager;
mod query;
mod query_editor;
mod query_view;
pub(crate) mod redis_lex;
mod routine;
mod routine_view;
mod runsql;
mod sidebar;
mod sql_completion;
pub(crate) mod sql_lex;
mod sync;
mod tabs;
mod toolbar;
mod transfer;
mod tree;
mod ui;
mod update;
mod user;
mod user_create;
mod view;
mod view_view;
mod widgets;
mod workspace;

use info_pane::{InfoPane, SidebarHost};

/// A configured shortcut, formatted for display: `"secondary-r"` reads `Ctrl+R` on Windows/Linux
/// and `⌘R` on macOS. The `secondary` modifier is platform-dependent, so the keystroke is parsed
/// against `os` — `Keystroke::parse` only takes a `'static` string.
fn shortcut_label(shortcut: &str, os: OperatingSystem) -> String {
    Keystroke::parse_with_os(shortcut, os)
        .map_or_else(|_| shortcut.to_string(), |key| Kbd::format(&key))
}

impl AppView {
    /// The first keystroke bound to keymap command `id`, formatted for display (a toolbar
    /// tooltip), or `None` when the command is unbound.
    fn command_shortcut_label(&self, id: &str, os: OperatingSystem) -> Option<String> {
        let command = keymap::command(id)?;
        keymap::keys_for(command, &self.shortcuts)
            .first()
            .map(|key| shortcut_label(key, os))
    }
}

/// The language to start in on a first launch: a Russian or Chinese system locale maps to that
/// language, every other locale to English.
fn detect_system_language() -> LanguageSetting {
    match sys_locale::get_locale() {
        Some(locale) if locale.to_ascii_lowercase().starts_with("zh") => LanguageSetting::ZhCn,
        Some(locale) if locale.to_ascii_lowercase().starts_with("ru") => LanguageSetting::Ru,
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
        // 启动时恢复 owns every bit of restored tree state — the open connections/databases as well
        // as the expansion of connections, databases and groups. With it off, the tree starts
        // collapsed rather than reopening the groups the previous launch left expanded.
        let restore_session = settings.restore_session;

        let connections: Vec<ConnectionNode> = config
            .load_profiles()
            .unwrap_or_default()
            .into_iter()
            .map(|profile| {
                let password = secrets.get(&profile.id).cloned();
                let password_saved = password.is_some();
                let expanded =
                    restore_session && settings.expanded_connections.contains(&profile.id);
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

        let remembered_db_expansion = if restore_session {
            settings.expanded_databases.clone()
        } else {
            BTreeMap::new()
        };
        let theme_setting = settings.theme;
        let language = settings.language;
        // Reconcile the persisted tree layout with the profiles that actually loaded: drop entries
        // for removed connections and append any connection the layout does not mention yet.
        let mut tree_layout = settings.groups.clone();
        groups::reconcile_tree(&mut tree_layout, &connections);
        if !restore_session {
            groups::collapse_all(&mut tree_layout);
        }
        let _ = config.migrate_legacy_queries();
        let query_files = query::scan_query_files(&config);
        let backup_configs = config.load_backups().unwrap_or_default();
        let transfer_configs = config.load_transfers().unwrap_or_default();
        let database_cache = config.load_database_cache().unwrap_or_default();
        // Fixed editing mechanics, not user commands (those are the keymap's, below). Claim Tab
        // inside the cell editor so it advances to the next cell rather than moving window focus
        // (which is what the `Root` context binds it to).
        cx.bind_keys([
            KeyBinding::new("tab", NextCell, Some(GRID_CELL_CONTEXT)),
            KeyBinding::new("shift-tab", PrevCell, Some(GRID_CELL_CONTEXT)),
            // Vertical cell movement while the in-place editor holds focus. The `GridCell > Input`
            // predicate ties with the kit's own `Input` binding at the same depth, and later
            // bindings win, so the grid's action takes over the keystroke.
            KeyBinding::new("up", MoveCellUp, Some(GRID_CELL_INPUT_CONTEXT)),
            KeyBinding::new("down", MoveCellDown, Some(GRID_CELL_INPUT_CONTEXT)),
            // Tab / Shift+Tab walking the table designer's fields grid.
            KeyBinding::new("tab", NextDesignerCell, Some(DESIGNER_CONTEXT)),
            KeyBinding::new("shift-tab", PrevDesignerCell, Some(DESIGNER_CONTEXT)),
            // Up/Down in a designer cell editor commit and move the selected row, like the grid's
            // cell editor. They are bound at the input context because the kit's own MoveUp/MoveDown
            // consumes those keys for a single-line field.
            KeyBinding::new("up", DesignerRowUp, Some(DESIGNER_CELL_INPUT_CONTEXT)),
            KeyBinding::new("down", DesignerRowDown, Some(DESIGNER_CELL_INPUT_CONTEXT)),
            // Tab confirms the highlighted completion item exactly like Enter does. gpui-kit only
            // handles Enter (its completion menu checks `input::Enter`), so this binding ties with
            // the kit's `Tab -> IndentInline` at the same context depth and, being registered
            // later, wins; the handler lets the keystroke fall through to the kit when no menu is
            // open (see `AppView::render_query_editor`).
            KeyBinding::new("tab", AcceptCompletion, Some(QUERY_EDITOR_INPUT_CONTEXT)),
        ]);
        // Every user command's keystrokes: built-in defaults, replaced by the settings' overrides,
        // and re-bound when the Options window applies a change.
        let bound_keys = keymap::bind_keymap(cx, &settings.shortcuts, &[]);
        // Restore the last session once the view exists — unless 启动时恢复 is off, in which case
        // nothing is restored *and* nothing is saved (a session file is only written once
        // `restore_session` has run), so the next start is a clean one.
        let restore_session = settings.restore_session;
        if restore_session {
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |view, cx| view.restore_session(cx));
            })
            .detach();
        }
        cx.on_app_quit(|view, cx| {
            view.save_session_now(cx);
            async {}
        })
        .detach();
        // Workspace-wide commands, handled at app level so they work wherever the focus is.
        {
            let weak = cx.weak_entity();
            (**cx).on_action(move |_: &NewQueryTab, cx: &mut App| {
                let _ = weak.update(cx, |view, cx| view.open_new_query(cx));
            });
            let weak = cx.weak_entity();
            (**cx).on_action(move |_: &CloseActiveTab, cx: &mut App| {
                let _ = weak.update(cx, |view, cx| view.close_active_tab(cx));
            });
            // The keyboard's 关闭其他选项卡 / 全部关闭 act on the tab that is active, like their
            // menu rows act on the tab that was right-clicked.
            let weak = cx.weak_entity();
            (**cx).on_action(move |_: &CloseOtherTabs, cx: &mut App| {
                let _ = weak.update(cx, |view, cx| {
                    if let Some(target) = view.current_tab(cx) {
                        view.close_other_tabs(target, cx);
                    }
                });
            });
            let weak = cx.weak_entity();
            (**cx).on_action(move |_: &CloseAllTabs, cx: &mut App| {
                let _ = weak.update(cx, |view, cx| view.close_all_tabs(cx));
            });
            let weak = cx.weak_entity();
            (**cx).on_action(move |_: &NextTab, cx: &mut App| {
                let _ = weak.update(cx, |view, cx| view.cycle_tab(1, cx));
            });
            let weak = cx.weak_entity();
            (**cx).on_action(move |_: &PrevTab, cx: &mut App| {
                let _ = weak.update(cx, |view, cx| view.cycle_tab(-1, cx));
            });
        }
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
            pending_session_tables: Vec::new(),
            pending_session_active: None,
            pending_session_databases: Vec::new(),
            pending_session_current: None,
            session_restored: false,
            session_signature_saved: 0,
            session_save_pending: false,
            runtime,
            connections,
            grids: Vec::new(),
            designs: Vec::new(),
            queries: Vec::new(),
            active_design: None,
            tab_sequence: RefCell::new(Vec::new()),
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
            query_timer_scheduled: false,
            query_sort: QuerySort::default(),
            query_grid: ColumnGrid::default(),
            backup_configs,
            transfer_configs,
            backup_files: Vec::new(),
            sync: {
                let mut sync = sync::SyncState::new(cx);
                sync.hide_same = settings.sync_hide_same;
                sync
            },
            backup_selected: None,
            new_backup_dialog: None,
            restore_dialog: None,
            extract_dialog: None,
            dump_dialog: None,
            run_sql_dialog: None,
            dump_submenu: None,
            dump_zone: Rc::new(RefCell::new(SubmenuZone::default())),
            import_submenu: false,
            import_zone: Rc::new(RefCell::new(SubmenuZone::default())),
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
            export_connections_dialog: None,
            export_connections_window: None,
            create_table_dialog: None,
            create_table_input: None,
            new_key_dialog: None,
            new_key_name_input: None,
            new_key_ttl_input: None,
            new_key_type_combo: None,
            new_key_value_input: None,
            new_key_field_input: None,
            new_key_score_input: None,
            key_ttl_dialog: None,
            key_ttl_input: None,
            save_connection_combo: None,
            save_database_combo: None,
            database_cache,
            save_query_focus_pending: false,
            query_focus: cx.focus_handle(),
            query_focus_pending: false,
            query_focus_attempts: 0,
            grid_focus_pending: false,
            query_editors: Vec::new(),
            preview_editors: std::collections::HashMap::new(),
            preview_decorations: std::collections::HashMap::new(),
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
            form_engine_inputs: BTreeMap::new(),
            form_engine_select_open: None,
            form_engine_select_anchor: Rc::new(RefCell::new(Point::default())),
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
            grid_show_comments: settings.grid_show_comments,
            grid_show_types: settings.grid_show_types,
            grid_field_layouts: settings.grid_field_layouts.clone(),
            shortcuts: settings.shortcuts.clone(),
            preserve_object_search: settings.preserve_object_search,
            restore_session: settings.restore_session,
            single_click_open: settings.single_click_open,
            max_statements_per_run: settings.max_statements_per_run,
            stop_on_error: settings.stop_on_error,
            query_info_editors: std::collections::HashMap::new(),
            last_shown_tab: None,
            bound_keys,
            object_searches: BTreeMap::new(),
            search_box_page: None,
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
                    .on_change(Rc::new({
                        let weak = weak.clone();
                        move |text, _window, cx| {
                            let _ = weak.update(cx, |app, cx| {
                                app.set_object_search(text.to_string(), cx);
                            });
                        }
                    }))
                    // Esc clears the search and lists every object again. Deferred: this runs
                    // inside the field's own key handler, and clearing writes back to the field,
                    // which must not be updated while it is handling the key (that aborted the app).
                    .on_cancel(Rc::new(move |_window, cx| {
                        let weak = weak.clone();
                        cx.defer(move |cx| {
                            let _ = weak.update(cx, |app, cx| app.clear_object_search(cx));
                        });
                    }))
                })
            },
            delete_confirm: None,
            error_dialog: None,
            grid_clipboard: Rc::new(RefCell::new(GridClipboard::default())),
            pending_toasts: Vec::new(),
            unsaved_confirm: None,
            dialog_focus_done: false,
            opened_dialog: None,
            log_handle: gpui_kit::base::TextSelectionHandle::new(String::new(), cx),
            log_menu: None,
            log_select_all: false,
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
            editor_ctrl_wheel_zoom: settings.editor_ctrl_wheel_zoom,
            options_section: OptionsSection::General,
            options_editor_font_family: settings.editor_font_family.clone(),
            options_editor_font_size: settings.editor_font_size,
            options_editor_line_numbers: settings.editor_line_numbers,
            options_editor_word_wrap: settings.editor_word_wrap,
            options_editor_ctrl_wheel_zoom: settings.editor_ctrl_wheel_zoom,
            options_grid_show_comments: settings.grid_show_comments,
            options_grid_show_types: settings.grid_show_types,
            options_preserve_object_search: settings.preserve_object_search,
            options_restore_session: settings.restore_session,
            options_single_click_open: settings.single_click_open,
            users_sort: DetailSort::default(),
            backup_sort: DetailSort::default(),
            options_max_statements: settings.max_statements_per_run.to_string(),
            options_max_statements_input: None,
            options_stop_on_error: settings.stop_on_error,
            options_copy_menu: None,
            options_highlight: None,
            options_shortcuts: settings.shortcuts.clone(),
            recording_shortcut: None,
            options_shortcut_focus: keymap::COMMANDS.iter().map(|_| cx.focus_handle()).collect(),
            options_shortcut_search: None,
            options_shortcut_filter: String::new(),
            editor_font_combo: None,
            editor_size_combo: None,
            options_window: None,
            options_focus: cx.focus_handle(),
            update_status: update::UpdateStatus::Idle,
            update_version: None,
            update_notes: None,
            update_progress: None,
            update_checked: false,
            tree_revision: 0,
            tab_revision: 0,
            tree_layout,
            connection_colors: settings.connection_colors.clone(),
            group_rename: None,
            group_rename_focus_pending: false,
            pending_group: None,
            move_submenu: None,
            move_zone: Rc::new(RefCell::new(SubmenuZone::default())),
            color_submenu: None,
            color_zone: Rc::new(RefCell::new(SubmenuZone::default())),
            connect_menu_group: None,
            tree_press: None,
            tree_drag: None,
            tree_row_rects: std::collections::HashMap::new(),
            tree_selection: ListSelection::default(),
            tree_marquee: None,
            tree_bounds: None,
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
            users_scope: UserScope::Users,
            users_roles_supported: false,
            users_row_rects: std::collections::HashMap::new(),
            users_focus: cx.focus_handle(),
            users_grid: ColumnGrid::default(),
            backups_selection: ListSelection::default(),
            backups_row_rects: std::collections::HashMap::new(),
            transfers_selection: ListSelection::default(),
            transfers_row_rects: std::collections::HashMap::new(),
            sync_window: None,
            sync_window_focus: cx.focus_handle(),
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
            create_user_kind_combo: None,
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
            export_choice: None,
            export_choice_combo: None,
            export_window: None,
            import_wizard: None,
            import_window: None,
            export_focus: cx.focus_handle(),
            import_focus: cx.focus_handle(),
            export_connections_focus: cx.focus_handle(),
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
        // The resolved palette needs the window's appearance, so it is computed at the next render;
        // that render pushes it into the cached children (see `sync_input_themes`). Notifying the
        // object pane here would send it the *old* palette for one frame.
        cx.notify();
    }

    fn set_language(&mut self, language: LanguageSetting, cx: &mut Context<'_, Self>) {
        if self.language == language {
            return;
        }
        self.language = language;
        rust_i18n::set_locale(language.locale());
        // The newer release's notes are fetched in the UI language (see `changelog`), so an
        // available release's notes have to be re-read in the new one.
        self.refresh_update_notes(cx);
        self.sync_search_placeholders(cx);
        self.persist_settings();
        self.notify_object_pane(cx);
        cx.notify();
    }

    /// Re-translate the search fields' baked-in placeholders after a language change. Most
    /// placeholders resolve through `t!` on every render (or when their transient dialog/window is
    /// created), but the shared search boxes live in `AppView` (or a long-lived child state) and
    /// are built once with the locale of that moment, so their text has to be pushed explicitly or
    /// it stays in the previous language. The Options window's shortcut search and the Sync
    /// window's table search are in that set even though their windows are re-opened later: the
    /// entities survive the window (the Options one is even built before the language is applied),
    /// so a language change has to retranslate them too.
    fn sync_search_placeholders(&mut self, cx: &mut Context<'_, Self>) {
        let key_list = self
            .object_pane
            .as_ref()
            .is_some_and(|pane| pane.read(cx).key_value);
        self.object_search_input.update(cx, |input, cx| {
            input.set_placeholder(
                if key_list {
                    t!("redis.key.search").to_string()
                } else {
                    t!("object.search").to_string()
                },
                cx,
            );
        });
        self.user_search_input.update(cx, |input, cx| {
            input.set_placeholder(t!("user.search").to_string(), cx);
        });
        self.backup_search_input.update(cx, |input, cx| {
            input.set_placeholder(t!("backup.search").to_string(), cx);
        });
        self.routine_find_input.update(cx, |input, cx| {
            input.set_placeholder(t!("routine.find").to_string(), cx);
        });
        self.sync.search_input.update(cx, |input, cx| {
            input.set_placeholder(t!("sync.search").to_string(), cx);
        });
        self.sync.transfer_search_input.update(cx, |input, cx| {
            input.set_placeholder(t!("sync.config.search").to_string(), cx);
        });
        if let Some(input) = self.sync.config_name_input.clone() {
            input.update(cx, |input, cx| {
                input.set_placeholder(t!("sync.config.name_placeholder").to_string(), cx);
            });
        }
        if let Some(input) = self.options_shortcut_search.clone() {
            input.update(cx, |input, cx| {
                input.set_placeholder(t!("keymap.search").to_string(), cx);
            });
        }
        // The grids' 字段筛选 search box is built with each grid, not per popup, so it does not pick
        // up a locale change by itself either.
        for grid in &self.grids {
            grid.update(cx, |grid, cx| grid.sync_search_placeholder(cx));
        }
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

    /// The environment colour of the connection at `index`, if the user set one.
    pub(super) fn connection_color(&self, index: usize) -> Option<ConnectionColor> {
        let id = &self.connections.get(index)?.profile.id;
        self.connection_colors
            .get(id)
            .and_then(|value| ConnectionColor::from_id(value))
    }

    /// Set (or clear, with `None`) a connection's environment colour and persist the choice.
    pub(super) fn set_connection_color(
        &mut self,
        index: usize,
        color: Option<ConnectionColor>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(id) = self
            .connections
            .get(index)
            .map(|node| node.profile.id.clone())
        else {
            return;
        };
        match color {
            Some(color) => {
                self.connection_colors.insert(id, color.id().to_string());
            }
            None => {
                self.connection_colors.remove(&id);
            }
        }
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
            editor_ctrl_wheel_zoom: self.editor_ctrl_wheel_zoom,
            expanded_connections,
            expanded_databases,
            groups: self.tree_layout.clone(),
            connection_colors: self.connection_colors.clone(),
            grid_field_layouts: self.grid_field_layouts.clone(),
            grid_show_comments: self.grid_show_comments,
            grid_show_types: self.grid_show_types,
            sync_hide_same: self.sync.hide_same,
            shortcuts: self.shortcuts.clone(),
            preserve_object_search: self.preserve_object_search,
            restore_session: self.restore_session,
            single_click_open: self.single_click_open,
            max_statements_per_run: self.max_statements_per_run,
            stop_on_error: self.stop_on_error,
        }) {
            // Preferences failing to persist is not worth a modal; note it in the log line only.
            let _ = error;
        }
    }

    /// Whether this click performs a row's "open" action (双击打开 normally, 单击打开 when
    /// Options → General enables it). In single-click mode a Ctrl/Shift click only changes the
    /// selection, and the second click of a double-click does nothing, so an object is never opened
    /// twice.
    pub(super) fn click_opens(&self, click_count: usize, modifiers: Modifiers) -> bool {
        if self.single_click_open {
            click_count == 1 && !modifiers.shift && !modifiers.control && !modifiers.platform
        } else {
            click_count >= 2
        }
    }

    /// Whether a click should still run the row's plain single-click behaviour (showing info,
    /// expanding a closed node). In single-click mode the extra click of a double-click is
    /// ignored.
    pub(super) fn click_is_plain(&self, click_count: usize) -> bool {
        !self.single_click_open || click_count == 1
    }

    /// Whether the grid header shows each column's comment under its name.
    pub(super) fn show_column_comments(&self) -> bool {
        self.grid_show_comments
    }

    /// Whether the grid header shows each column's data type between its name and comment.
    pub(super) fn show_column_types(&self) -> bool {
        self.grid_show_types
    }

    /// The persisted field layout for one table grid, if the user has configured one.
    pub(super) fn field_layout(
        &self,
        connection_id: &str,
        database: &str,
        table: &str,
    ) -> FieldLayout {
        self.grid_field_layouts
            .get(&field_layout_key(connection_id, database, table))
            .cloned()
            .unwrap_or_default()
    }

    /// Remember (or, when the layout is back to default, forget) one table grid's field layout.
    /// Returns `true` when the stored value changed, so the caller can skip a redundant write.
    pub(super) fn set_field_layout(
        &mut self,
        connection_id: &str,
        database: &str,
        table: &str,
        layout: FieldLayout,
    ) -> bool {
        let key = field_layout_key(connection_id, database, table);
        let current = self.grid_field_layouts.get(&key);
        if current == Some(&layout) {
            return false;
        }
        if layout.order.is_empty() && layout.hidden.is_empty() {
            self.grid_field_layouts.remove(&key);
        } else {
            self.grid_field_layouts.insert(key, layout);
        }
        self.persist_settings();
        true
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

    /// Ctrl + wheel over a SQL editor: step the font size to the next/previous size the 编辑器
    /// page offers (`step > 0` zooms in), apply it to every open editor and remember it. The step
    /// keeps the size inside the dropdown's list, so the 字号 field always shows what is in use.
    /// Returns whether the gesture was consumed (an unconsumed wheel still scrolls the editor).
    pub(super) fn zoom_editor_font(&mut self, step: f32, cx: &mut Context<'_, Self>) -> bool {
        if !self.editor_ctrl_wheel_zoom || step == 0.0 {
            return false;
        }
        let sizes = options::EDITOR_FONT_SIZES;
        let current = self.editor_font_size;
        let next = if step > 0.0 {
            sizes.iter().copied().find(|size| *size > current)
        } else {
            sizes.iter().rev().copied().find(|size| *size < current)
        };
        let Some(next) = next else {
            return false;
        };
        self.editor_font_size = next;
        // Mirror the change into the staged value, so the Options window's live preview follows the
        // zoom and a later 确定 does not put the old size back.
        self.options_editor_font_size = next;
        self.apply_editor_font(cx);
        self.persist_settings();
        cx.notify();
        true
    }

    /// The SQL editors the app owns in the main window: the query editors and the routine/view
    /// SQL 预览 panes. The result 信息 log is excluded because it keeps a fixed 12px font.
    fn open_sql_editors(&self) -> Vec<Entity<ui::SqlEditor>> {
        self.query_editors
            .iter()
            .flatten()
            // The result/信息 log is deliberately not zoomed; it stays at its fixed 12px.
            .chain(self.preview_editors.values())
            .cloned()
            .collect()
    }

    /// Push the current editor font into every open SQL editor (the Ctrl + wheel zoom path; the
    /// font is what the editor renders, so no window is needed).
    fn apply_editor_font(&mut self, cx: &mut Context<'_, Self>) {
        let font: SharedString = self.editor_font().into();
        let size = self.editor_font_size();
        for editor in self.open_sql_editors() {
            editor.update(cx, |editor, cx| editor.set_font(font.clone(), size, cx));
        }
    }

    /// Apply the 编辑器 settings (font, size, 显示行号, 自动换行) to every open SQL editor, so a
    /// change takes effect at once instead of only in editors opened afterwards. The 信息 log and
    /// the SQL 预览 panes keep their line numbers off, exactly as they are created.
    pub(super) fn apply_editor_display_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let font: SharedString = self.editor_font().into();
        let size = self.editor_font_size();
        let word_wrap = self.editor_word_wrap;
        let line_numbers = self.editor_line_numbers;
        let query_editors = self.query_editors.clone();
        for editor in query_editors.into_iter().flatten() {
            editor.update(cx, |editor, cx| {
                editor.set_display_options(font.clone(), size, line_numbers, word_wrap, window, cx)
            });
        }
        // The result/信息 log is fixed at 12px; only the SQL 预览 panes follow the editor font.
        for editor in self.query_info_editors.values() {
            editor.update(cx, |editor, cx| {
                editor.set_display_options(font.clone(), 12.0, false, word_wrap, window, cx)
            });
        }
        for editor in self.preview_editors.values() {
            editor.update(cx, |editor, cx| {
                editor.set_display_options(font.clone(), size, false, word_wrap, window, cx)
            });
        }
    }

    /// The remembered list layout for a page, defaulting to 详细列表.
    pub(super) fn view_mode(&self, page: &str) -> ViewMode {
        self.view_modes
            .get(page)
            .map(|value| ViewMode::from_id(value))
            .unwrap_or_default()
    }

    /// A page's list layout, using `default` until the user picks one explicitly. Key-value
    /// engines default to the detailed key table instead of the tile grid.
    pub(super) fn view_mode_or(&self, page: &str, default: ViewMode) -> ViewMode {
        self.view_modes
            .get(page)
            .map(|value| ViewMode::from_id(value))
            .unwrap_or(default)
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
        driver.capabilities_for(&node.profile).has(capability)
    }

    /// Whether the connection may open the table designer (the MySQL driver's StarRocks
    /// compatibility mode cannot run MySQL table DDL, so it turns this off).
    fn driver_supports_table_designer(&self, connection_index: usize) -> bool {
        let Some(node) = self.connections.get(connection_index) else {
            return true;
        };
        let Some(driver) = self.registry.get(&node.profile.driver) else {
            return true;
        };
        driver.supports_table_designer(&node.profile)
    }

    /// Whether the connection may use RustGrid's `.rgbak` backup/restore.
    fn driver_supports_backup(&self, connection_index: usize) -> bool {
        let Some(node) = self.connections.get(connection_index) else {
            return true;
        };
        let Some(driver) = self.registry.get(&node.profile.driver) else {
            return true;
        };
        driver.supports_backup(&node.profile)
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
        self.tree_layout.hash(&mut hasher);
        // Environment colours change the tree node/subtree backgrounds, so a colour edit must
        // invalidate the cached tree even when nothing else about the connection changed.
        self.connection_colors.hash(&mut hasher);
        self.group_rename
            .as_ref()
            .map(|edit| edit.id)
            .hash(&mut hasher);
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
        // The multi-selection changes the row highlight, so it must invalidate the cached tree.
        self.tree_selection.items().hash(&mut hasher);
        // Saved queries and backups are tree leaves: a save, rename or delete must redraw them,
        // and the leaves paint the same selection their list does.
        for file in &self.query_files {
            file.name.hash(&mut hasher);
            file.database.hash(&mut hasher);
            file.schema.hash(&mut hasher);
        }
        self.query_selection.items().hash(&mut hasher);
        for file in &self.backup_files {
            file.name.hash(&mut hasher);
            file.database.hash(&mut hasher);
            file.connection_id.hash(&mut hasher);
        }
        for config in &self.backup_configs {
            config.name.hash(&mut hasher);
            config.database.hash(&mut hasher);
            config.connection_id.hash(&mut hasher);
        }
        self.backups_selection.items().hash(&mut hasher);
        self.main_tab.hash(&mut hasher);
        self.query_rename
            .as_ref()
            .map(|edit| edit.index)
            .hash(&mut hasher);
        self.backup_rename
            .as_ref()
            .map(|edit| match edit.target {
                BackupSelection::File(index) => (0u8, index),
                BackupSelection::Config(index) => (1u8, index),
            })
            .hash(&mut hasher);
        hasher.finish()
    }

    /// A cheap revision of everything [`TabBar`] reads from `AppView`.
    fn tab_revision(&self, cx: &App) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.theme.is_dark().hash(&mut hasher);
        self.connection_colors.hash(&mut hasher);
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
        self.sync
            .search_input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        self.sync
            .transfer_search_input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        if let Some(input) = self.sync.config_name_input.clone() {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
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
            self.sync.source_combo.as_ref(),
            self.sync.source_database_combo.as_ref(),
            self.sync.target_combo.as_ref(),
            self.sync.target_database_combo.as_ref(),
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
        } else if self.main_tab == MainTab::Sync {
            let count = self.visible_transfer_indices().len();
            div()
                .flex_1()
                .min_w(px(0.0))
                .px_2()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(t!("sync.status_bar_configs", count = count).to_string())
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

/// The `(connection_index, database_index, schema)` a connection-tree row addresses, for rows that
/// scope a database: a database (`db-…`), a schema (`schema-…`) or a category (`cat-…`, which may
/// carry a `<schema>` segment — or none, for a synthetic id). Any other row returns `None`.
pub(super) fn parse_tree_scope(selected: &str) -> Option<(usize, usize, Option<String>)> {
    if let Some(rest) = selected.strip_prefix("db-") {
        let mut parts = rest.splitn(2, '-');
        return Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            None,
        ));
    }
    if let Some(rest) = selected.strip_prefix("schema-") {
        let mut parts = rest.splitn(3, '-');
        let connection_index = parts.next()?.parse().ok()?;
        let database_index = parts.next()?.parse().ok()?;
        let schema = parts.next()?;
        return Some((
            connection_index,
            database_index,
            (!schema.is_empty()).then(|| schema.to_string()),
        ));
    }
    // Query and backup leaves scope like their database row; their own schema (a query may be
    // filed under one) is not encoded in the id, so the list's database scope is used.
    if let Some(rest) = selected
        .strip_prefix("qry-")
        .or_else(|| selected.strip_prefix("bak-"))
    {
        let mut parts = rest.splitn(3, '-');
        return Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            None,
        ));
    }

    let rest = selected.strip_prefix("cat-")?;
    let mut parts = rest.splitn(3, '-');
    let connection_index = parts.next()?.parse().ok()?;
    let database_index = parts.next()?.parse().ok()?;
    // The tail is `<schema>-<category>` or just `<category>` (a synthetic, scope-less id).
    let tail = parts.next()?;
    let scope = tail.rsplit_once('-').map(|(scope, _)| scope).unwrap_or("");
    let schema = (!scope.is_empty()).then(|| scope.to_string());
    Some((connection_index, database_index, schema))
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
        self.track_session(cx);
        // A disabled main tab (Users/Functions on an engine without them, Backups on a StarRocks
        // compatibility connection) must not stay active after the connection scope changes.
        let main_tab = self.main_tab;
        if !self.main_tab_usable(main_tab, cx) {
            self.main_tab = MainTab::Tables;
        }
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
            // The object list draws from its own palette; repaint it with the freshly resolved one.
            self.notify_object_pane(cx);
            // The editor's syntax colors and line-number gutter are owned by gpui-kit's highlight
            // theme, so install the app's palette over the kit default (see the helper).
            install_editor_highlight_theme(self.theme, cx);
            self.synced_input_theme = Some(self.theme);
        }
        self.sync_cached_children(cx);
        // The one search box is shared by 表 / 视图 / 函数 / 查询: swap in the shown page's own text
        // before the content pane renders its toolbar.
        self.sync_object_search(cx);
        self.ensure_query_combos(cx);
        self.sync_db_combos(cx);
        self.sync_db_option_combos(cx);
        self.sync_query_combos(cx);
        self.sync_save_dialog_combos(cx);
        self.sync_language_combo(cx);
        self.sync_editor_combos(cx);
        self.ensure_sync_combos(cx);
        self.sync_sync_combos(cx);
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

        if self.group_rename_focus_pending {
            self.group_rename_focus_pending = false;
            if let Some(edit) = self.group_rename.as_ref() {
                // Focus the inner gpui-kit input so it emits `Blur` on click-away.
                edit.input
                    .update(cx, |input, cx| input.focus_state(window, cx));
            }
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

        // When the shown tab changed (click, Ctrl+Tab, a close): give the newly shown pane the
        // keyboard focus, and clear the object search unless the user keeps it.
        let shown = self.current_tab(cx);
        if self.last_shown_tab != Some(shown) {
            let first = self.last_shown_tab.is_none();
            self.last_shown_tab = Some(shown);
            if !first {
                self.clear_object_search_unless_kept(cx);
                match shown {
                    None => {
                        if let Some(pane) = self.object_pane.as_ref() {
                            let focus = pane.read(cx).focus.clone();
                            window.focus(&focus, cx);
                        }
                    }
                    Some(TabTarget::Grid(_)) => self.grid_focus_pending = true,
                    Some(TabTarget::Query(_)) => self.query_focus_pending = true,
                    Some(TabTarget::Design(_)) => {}
                }
            }
        }
        if self.grid_focus_pending {
            self.grid_focus_pending = false;
            if self.active_query.is_none()
                && let Some(grid) = self.active_grid.and_then(|index| self.grids.get(index))
            {
                let focus = grid.read(cx).focus.clone();
                window.focus(&focus, cx);
            }
        }
        if self.query_focus_pending {
            // Focus the editor itself so the caret is there and typing goes in. The editor is built
            // on its first render, so a just-opened tab gets it on the next frame.
            let editor = self
                .active_query
                .and_then(|index| self.query_editors.get(index).cloned().flatten());
            let focused = editor
                .is_some_and(|editor| editor.update(cx, |editor, cx| editor.focus(window, cx)));
            if focused || self.active_query.is_none() || self.query_focus_attempts >= 5 {
                if !focused {
                    window.focus(&self.query_focus, cx);
                }
                self.query_focus_pending = false;
                self.query_focus_attempts = 0;
            } else {
                self.query_focus_attempts += 1;
                cx.notify();
            }
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
                this.tree_drag_move(event, cx);
                this.drag_tree_marquee(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    let host = this.sidebar_host.clone();
                    host.update(cx, |host, cx| host.end_drag(cx));
                    let pane = this.info_pane.clone();
                    pane.update(cx, |pane, cx| pane.end_drag(cx));
                    this.end_query_split_drag(cx);
                    this.tree_drag_end(cx);
                    this.end_tree_marquee(cx);
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
            root = root.child(self.render_context_menu(menu, window, cx));
        }

        // The connection-tree drag-and-drop drop indicator, drawn in window coordinates so it
        // follows the pointer as `AppView` re-renders on each drag move.
        if let Some(hint) = self.tree_drop_indicator() {
            root = root.child(sidebar::tree_drop_indicator_element(hint, theme));
        }

        // The connection tree's rubber-band selection rectangle, in window coordinates.
        if let Some((origin, current, bounds)) = self.tree_marquee_rect() {
            root = root.child(sidebar::tree_marquee_element(
                origin, current, bounds, theme,
            ));
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

/// A connection icon, drawn from the driver's descriptor: the engine's mark as a white glyph on a
/// solid badge. The badge keeps the mark legible on a tinted — environment-coloured — row, which a
/// bare glyph would blend into.
fn tree_driver_icon(
    icon: &'static str,
    style: DriverIconStyle,
    badge: u32,
    connected: bool,
) -> AnyElement {
    let block = match style {
        // Brand marks keep the engine colour only while the connection is open; a closed one uses
        // the status badge colour (grey) instead of staying colourful.
        DriverIconStyle::Brand(brand) if connected => brand,
        DriverIconStyle::Brand(_) | DriverIconStyle::SolidBadge | DriverIconStyle::Plain => badge,
    };
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(16.0))
        .h(px(16.0))
        .flex_none()
        .bg(rgb(block))
        .child(
            svg()
                .path(icon)
                .w(px(12.0))
                .h(px(12.0))
                .flex_none()
                .text_color(rgb(0xffffff)),
        )
        .into_any_element()
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

/// The settings key of one table grid's remembered field layout. The unit separator cannot appear in
/// an identifier, so it keeps the three parts apart even when a name contains the others' characters.
fn field_layout_key(connection_id: &str, database: &str, table: &str) -> String {
    format!("{connection_id}\u{1f}{database}\u{1f}{table}")
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

    #[test]
    fn tree_scope_parses_databases_schemas_and_categories() {
        // A database row, a schema row, a schema-qualified category and both category id shapes
        // (the tree's own `cat-<ci>-<di>-<scope>-<id>` and a synthetic scope-less one).
        assert_eq!(parse_tree_scope("db-0-2"), Some((0, 2, None)));
        assert_eq!(
            parse_tree_scope("schema-0-2-dbo"),
            Some((0, 2, Some("dbo".to_string())))
        );
        assert_eq!(
            parse_tree_scope("cat-0-2-dbo-q"),
            Some((0, 2, Some("dbo".to_string())))
        );
        assert_eq!(parse_tree_scope("cat-0-2--q"), Some((0, 2, None)));
        assert_eq!(parse_tree_scope("cat-0-2-q"), Some((0, 2, None)));
        assert_eq!(parse_tree_scope("cat-0-2-t"), Some((0, 2, None)));
        // Query and backup leaves scope their database, like the list row they stand for.
        assert_eq!(parse_tree_scope("qry-0-2-7"), Some((0, 2, None)));
        assert_eq!(parse_tree_scope("bak-0-2-f-7"), Some((0, 2, None)));
        // A table row does not scope a database.
        assert_eq!(parse_tree_scope("tbl-0-2-dbo.users"), None);
        assert_eq!(parse_tree_scope("conn-1"), None);
    }

    #[test]
    fn connection_color_ids_round_trip() {
        for color in ConnectionColor::ALL {
            assert_eq!(ConnectionColor::from_id(color.id()), Some(color));
        }
        assert_eq!(ConnectionColor::from_id("purple"), None);
    }
}
