use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, CursorStyle, DispatchPhase, Div,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, FontWeight, HighlightStyle,
    ImageSource, KeyBinding, KeyDownEvent, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Resource, ScrollDelta,
    ScrollHandle, ScrollStrategy, ScrollWheelEvent, SharedString, Stateful, StyledText,
    Subscription, Svg, TextLayout, UTF16Selection, UniformListScrollHandle, WeakEntity, Window,
    WindowControlArea, canvas, deferred, div, img, prelude::*, px, rgb, svg, uniform_list,
};
use rustgrid_config::{AppSettings, ConfigStore, LanguageSetting, ThemeSetting};
use rustgrid_core::{
    CellValue, Connection, ConnectionConfig, DriverRegistry, Error, FilterCondition,
    FilterConjunction, FilterGroup, FilterNode, FilterOperator, PageRequest, QueryResult,
    RowInsert, RowUpdate, SavedQuery,
};

use crate::form::{ConnectionForm, FORM_FIELDS, FormField};
use crate::runtime::Runtime;
use crate::session::{
    Category, CategoryExpansion, CellRange, CellSelection, ConnectionNode, ConnectionStatus,
    DatabaseNode, GridState, Loadable, QueryTab, SortRule, compute_column_widths,
};
use crate::sql::{self, SqlSpan, SqlToken};
use crate::theme::Theme;

// The date half of the in-place date/time picker is gpui-kit's calendar (day, month and year
// views, navigation, selection and localization); only the time-of-day row is app-drawn.
use gpui_kit::component::calendar::{CalendarEvent, CalendarState};

use ui::{
    ButtonKind, ComboBox, ComboOption, TextInput, TextInputOptions, checkbox_box, form_tab,
    main_separator, scrollbar_fractions, scrollbar_thumb, toolbar_separator,
};

// Cell-navigation actions for the in-place grid editor. gpui-kit's `Root` binds `tab`/`shift-tab`
// to focus traversal in the `"Root"` context, and key bindings run before `on_key_down`; binding
// the same keys in a deeper context (the editing cell) takes precedence and moves the editor.
gpui::actions!(grid, [NextCell, PrevCell]);

/// Key context applied to the cell that owns the in-place editor.
const GRID_CELL_CONTEXT: &str = "GridCell";

/// The six text inputs of the connection form, created when the form opens. Order follows
/// [`FORM_FIELDS`] so `FormField as usize` indexes the array.
struct FormInputs {
    fields: [Entity<TextInput>; 6],
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
                placeholder: SharedString::default(),
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
    error: Option<String>,
}

/// Which pane's row owns the in-place rename editor. The object list and the connection tree can
/// both list the same table, so the editor is drawn in exactly one of them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowPane {
    Objects,
    Tree,
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
                    if let Some(DbDialog::New { name, .. }) = app.db_dialog.as_mut() {
                        *name = text.to_string();
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
    QueryEditor,
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
    Sql,
}

enum DbDialog {
    New {
        connection_index: usize,
        name: String,
        error: Option<String>,
    },
    Edit {
        connection_index: usize,
        database_index: usize,
        name: String,
        original_charset: String,
        original_collation: String,
        charset: String,
        collation: String,
        charsets: Vec<String>,
        collations: Vec<String>,
        tab: DbTab,
        loading: bool,
        error: Option<String>,
    },
    Delete {
        connection_index: usize,
        database_index: usize,
        name: String,
        error: Option<String>,
    },
}

/// The Tables/Views object browser for one database. Owns its own list state and renders the
/// list body independently of `AppView`; the surrounding toolbar/search stays on `AppView`.
struct ObjectPane {
    app: WeakEntity<AppView>,
    connection_index: usize,
    database_index: usize,
    category: Category,
    selected: Option<String>,
    scroll: ScrollHandle,
    hscroll_grab: Option<f32>,
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

    filter_value_focus: Vec<(Vec<usize>, FocusHandle)>,
    filter_value2_focus: Vec<(Vec<usize>, FocusHandle)>,
    filter_active: Option<(Vec<usize>, u8)>,
    /// The shared dropdown entities for each condition row, keyed by the node path. Rebuilt after
    /// any structural change so the paths stay valid.
    filter_field_combos: BTreeMap<Vec<usize>, Entity<ComboBox>>,
    filter_operator_combos: BTreeMap<Vec<usize>, Entity<ComboBox>>,

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

/// The SQL completion popup state for the active query editor.
struct Completion {
    candidates: Vec<String>,
    selected: usize,
    start: usize,
    end: usize,
}

/// The lowercased completion candidates `(lowercase, original)` for one connection's loaded
/// tables plus the SQL keyword list, reused across keystrokes until the tables change.
struct CompletionCache {
    connection_index: Option<usize>,
    generation: u64,
    items: Rc<Vec<(String, String)>>,
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
    hour: u32,
    minute: u32,
    second: u32,
    has_time: bool,
    /// The date half, owned by gpui-kit's calendar: day/month/year views, navigation and the
    /// selected date all live here (the calendar is the source of truth for the date).
    calendar: Entity<CalendarState>,
    /// Keeps the calendar's selection event wired for as long as the picker is open.
    _subscription: Subscription,
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
}

#[derive(Clone, Copy)]
enum TableOperation {
    Drop,
    Empty,
    Truncate,
}

/// Which app dialog `Root` is currently hosting. AppView state stays the source of truth; this
/// only tracks the last kind handed to `window.open_dialog` so the transition fires once.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DialogKind {
    ConnectionForm,
    DbDialog,
    Password,
    SaveQuery,
    Error,
    Confirm,
    Options,
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

/// A sortable column of the saved-query list under the Queries main tab.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SavedQueryColumn {
    Name,
    Connection,
    Database,
}

const MAIN_TABS: [(MainTab, &str, &str); 6] = [
    (MainTab::Tables, "icons/tables.svg", "main.tables"),
    (MainTab::Views, "icons/views.svg", "main.views"),
    (MainTab::Functions, "icons/functions.svg", "main.functions"),
    (MainTab::Users, "icons/user.svg", "main.users"),
    (MainTab::Queries, "icons/queries.svg", "main.queries"),
    (MainTab::Backups, "icons/backups.svg", "main.backups"),
];

const PANEL_WIDTH: f32 = 620.0;
const FIELD_LABEL_WIDTH: f32 = 96.0;
const OBJECT_ROW_HEIGHT: f32 = 20.0;
const OBJECT_BOTTOM_MARGIN: f32 = 20.0;
/// Fixed column widths of the saved-query list (query name / connection / database).
const SAVED_QUERY_NAME_WIDTH: f32 = 200.0;
const SAVED_QUERY_CONNECTION_WIDTH: f32 = 160.0;
const SAVED_QUERY_DATABASE_WIDTH: f32 = 160.0;
const GRID_ROW_HEIGHT: f32 = 24.0;
const GRID_COLUMN_WIDTH: f32 = 120.0;
/// Fixed column widths of a filter condition row. The filter builder's group boundary row reuses
/// them to line its controls up under the operator column.
const FILTER_TOGGLE_WIDTH: f32 = 78.0;
const FILTER_FIELD_WIDTH: f32 = 170.0;
const FILTER_OPERATOR_WIDTH: f32 = 150.0;
const FILTER_VALUE_WIDTH: f32 = 200.0;
/// Clamp for a column dragged to its narrowest/widest.
const MIN_COLUMN_WIDTH: f32 = 32.0;
const MAX_COLUMN_WIDTH: f32 = 1200.0;
const GRID_GUTTER_WIDTH: f32 = 22.0;
/// Thickness of the app-drawn grid scrollbars (matches `ui::vscrollbar_track` / `hscrollbar_track`).
const GRID_SCROLLBAR_THICKNESS: f32 = 14.0;
const QUERY_EDITOR_PAD: f32 = 6.0;
/// Cap used when "Limit Records" is unchecked, standing in for an unlimited fetch.
const NO_LIMIT_PAGE_SIZE: u64 = 1_000_000;

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
    /// Saved, named queries loaded from `queries.json`.
    saved_queries: Vec<SavedQuery>,
    /// The saved query highlighted in the Queries list, indexed into `saved_queries`.
    saved_query_selected: Option<usize>,
    /// The saved-query list sort: which column and whether it is descending. `None` keeps the
    /// stored (insertion) order.
    saved_query_sort: Option<(SavedQueryColumn, bool)>,
    /// The open "save query" dialog, if any.
    save_query_dialog: Option<SaveQueryDialog>,
    query_name_input: Option<Entity<TextInput>>,
    save_location_combo: Option<Entity<ComboBox>>,
    save_query_focus_pending: bool,
    query_focus: FocusHandle,
    query_focus_pending: bool,
    query_editor_focused: bool,
    /// The SQL editor's IME composing byte range, if any.
    query_ime_marked: Option<std::ops::Range<usize>>,
    query_editor_layout: RefCell<TextLayout>,
    query_editor_text: RefCell<String>,
    query_editor_measured: bool,
    query_result_scroll: UniformListScrollHandle,
    query_connection_combo: Option<Entity<ComboBox>>,
    query_database_combo: Option<Entity<ComboBox>>,
    query_completion: Option<Completion>,
    query_completion_cache: RefCell<Option<CompletionCache>>,
    /// Bumped whenever the loaded table sets change, invalidating the completion cache.
    completion_generation: u64,
    /// Tokenizer spans for the SQL editor, cached by the exact text so each frame does not
    /// re-tokenize the whole document (see `styled_sql`).
    sql_highlight_cache: RefCell<Option<(String, Rc<Vec<SqlSpan>>)>>,
    active_grid: Option<usize>,
    next_grid_id: u64,
    form: Option<ConnectionForm>,
    editing: Option<usize>,
    test_status: TestStatus,
    context_menu: Option<ContextMenu>,
    tab_menu: Option<TabMenu>,
    tools_menu_open: bool,
    menu_popup_anchor: Rc<RefCell<Point<Pixels>>>,
    object_pane: Option<Entity<ObjectPane>>,
    tab_bar: Entity<TabBar>,
    tree_pane: Entity<TreePane>,
    main_tab: MainTab,
    db_dialog: Option<DbDialog>,
    db_charset_combo: Option<Entity<ComboBox>>,
    db_collation_combo: Option<Entity<ComboBox>>,
    db_name_input: Option<Entity<TextInput>>,
    db_sql_focus: FocusHandle,
    db_sql_layout: RefCell<TextLayout>,
    db_sql_text: RefCell<String>,
    db_sql_anchor: usize,
    db_sql_cursor: usize,
    db_sql_selecting: bool,
    form_inputs: Option<FormInputs>,
    caret_visible: bool,
    caret_blink_running: bool,
    password_prompt: Option<PasswordPrompt>,
    password_focus_pending: bool,
    rename_edit: Option<RenameEdit>,
    rename_blur: Option<Subscription>,
    rename_focus_pending: bool,
    page_size: u64,
    limit_records: bool,
    object_search: String,
    object_search_input: Entity<TextInput>,
    delete_confirm: Option<DeleteConfirm>,
    error_dialog: Option<String>,
    /// The kind of dialog last opened through `Root`, if any.
    opened_dialog: Option<DialogKind>,
    window_bounds_subscription: Option<Subscription>,
    theme_setting: ThemeSetting,
    theme: Theme,
    /// Theme last pushed into the managed inputs/combos, so `sync_input_themes` can skip when
    /// nothing changed instead of updating every input on every frame.
    synced_input_theme: Option<Theme>,
    language: LanguageSetting,
    options_open: bool,
    options_theme: ThemeSetting,
    options_language: LanguageSetting,
    language_combo: Option<Entity<ComboBox>>,
    /// Last data revision handed to the cached `TreePane` / `TabBar`, so they re-render only when
    /// what they read from `AppView` actually changed (they are embedded with `.cached`, which
    /// otherwise freezes them until they are explicitly notified).
    tree_revision: u64,
    tab_revision: u64,
}

mod database;
mod db_dialog;
mod design;
mod design_view;
mod dialogs;
mod form;
mod grid;
mod grid_cell;
mod grid_commit;
mod grid_ime;
mod grid_input;
mod grid_scroll;
mod grid_toolbar;
mod grid_view;
mod objects;
mod options;
mod query;
mod query_editor;
mod query_view;
mod shell;
mod sidebar;
mod tabs;
mod toolbar;
mod tree;
mod ui;
mod widgets;

pub use shell::AppShell;

impl AppView {
    pub fn new(
        registry: Arc<DriverRegistry>,
        config: Arc<ConfigStore>,
        runtime: Arc<Runtime>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let secrets = config.load_secrets().unwrap_or_default();

        let connections = config
            .load_profiles()
            .unwrap_or_default()
            .into_iter()
            .map(|profile| {
                let password = secrets.get(&profile.id).cloned();
                let password_saved = password.is_some();
                ConnectionNode {
                    profile,
                    password,
                    password_saved,
                    status: ConnectionStatus::Disconnected,
                    databases: Loadable::Idle,
                    expanded: false,
                }
            })
            .collect();

        let settings = config.load_settings().unwrap_or_default();
        let theme_setting = settings.theme;
        let language = settings.language;
        let saved_queries = config.load_queries().unwrap_or_default();
        // Claim Tab inside the cell editor so it advances to the next cell rather than moving
        // window focus (which is what the `Root` context binds it to).
        cx.bind_keys([
            KeyBinding::new("tab", NextCell, Some(GRID_CELL_CONTEXT)),
            KeyBinding::new("shift-tab", PrevCell, Some(GRID_CELL_CONTEXT)),
        ]);
        let app = cx.weak_entity();

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
            saved_queries,
            saved_query_selected: None,
            saved_query_sort: None,
            save_query_dialog: None,
            query_name_input: None,
            save_location_combo: None,
            save_query_focus_pending: false,
            query_focus: cx.focus_handle(),
            query_focus_pending: false,
            query_editor_focused: false,
            query_ime_marked: None,
            query_editor_layout: RefCell::new(TextLayout::default()),
            query_editor_text: RefCell::new(String::new()),
            query_editor_measured: false,
            query_result_scroll: UniformListScrollHandle::new(),
            query_connection_combo: None,
            query_database_combo: None,
            query_completion: None,
            query_completion_cache: RefCell::new(None),
            completion_generation: 0,
            sql_highlight_cache: RefCell::new(None),
            active_grid: None,
            next_grid_id: 0,
            form: None,
            editing: None,
            test_status: TestStatus::Idle,
            context_menu: None,
            tab_menu: None,
            tools_menu_open: false,
            menu_popup_anchor: Rc::new(RefCell::new(Point::default())),
            object_pane: None,
            tab_bar: cx.new(|_| TabBar::new(app.clone())),
            tree_pane: cx.new(|cx| TreePane::new(app.clone(), cx)),
            main_tab: MainTab::Tables,
            db_dialog: None,
            db_charset_combo: None,
            db_collation_combo: None,
            db_name_input: None,
            db_sql_focus: cx.focus_handle(),
            db_sql_layout: RefCell::new(TextLayout::default()),
            db_sql_text: RefCell::new(String::new()),
            db_sql_anchor: 0,
            db_sql_cursor: 0,
            db_sql_selecting: false,
            form_inputs: None,
            caret_visible: true,
            caret_blink_running: false,
            password_prompt: None,
            password_focus_pending: false,
            rename_edit: None,
            rename_blur: None,
            rename_focus_pending: false,
            page_size: 1000,
            limit_records: true,
            object_search: String::new(),
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
            opened_dialog: None,
            window_bounds_subscription: None,
            theme_setting,
            theme: Theme::dark(),
            synced_input_theme: None,
            language,
            options_open: false,
            options_theme: theme_setting,
            options_language: language,
            language_combo: None,
            tree_revision: 0,
            tab_revision: 0,
        }
    }

    fn set_theme(&mut self, setting: ThemeSetting, cx: &mut Context<'_, Self>) {
        if self.theme_setting == setting {
            return;
        }
        self.theme_setting = setting;
        let _ = self.config.save_settings(&AppSettings {
            theme: setting,
            language: self.language,
        });
        self.notify_object_pane(cx);
        cx.notify();
    }

    fn set_language(&mut self, language: LanguageSetting, cx: &mut Context<'_, Self>) {
        if self.language == language {
            return;
        }
        self.language = language;
        rust_i18n::set_locale(language.locale());
        let _ = self.config.save_settings(&AppSettings {
            theme: self.theme_setting,
            language,
        });
        self.notify_object_pane(cx);
        cx.notify();
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
        if let Some(input) = self.db_name_input.as_ref() {
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
        for design in &self.designs {
            design.update(cx, |design, cx| design.set_theme(theme, cx));
        }
        for combo in [
            self.db_charset_combo.as_ref(),
            self.db_collation_combo.as_ref(),
            self.query_connection_combo.as_ref(),
            self.query_database_combo.as_ref(),
            self.save_location_combo.as_ref(),
            self.language_combo.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            combo.update(cx, |combo, cx| combo.set_theme(theme, cx));
        }
    }
}

/// A layout style for `Entity::cached`, built from the same `div()` DSL used elsewhere. Caching
/// skips measuring the child, so the caller must describe its slot (e.g. `flex_1`/`size_full`).
fn cached_style(build: impl FnOnce(Div) -> Div) -> gpui::StyleRefinement {
    let mut element = build(div());
    std::mem::take(element.style())
}

fn db_error(error: &Option<String>, theme: Theme) -> AnyElement {
    match error {
        Some(message) => div()
            .text_size(px(12.0))
            .text_color(rgb(theme.danger))
            .child(message.clone())
            .into_any_element(),
        None => div().into_any_element(),
    }
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

/// The color used to render a token category in the SQL editor.
fn sql_token_color(theme: Theme, token: SqlToken) -> u32 {
    match token {
        SqlToken::Keyword => theme.sql_keyword,
        SqlToken::Identifier => theme.text,
        SqlToken::String => theme.sql_string,
        SqlToken::Number => theme.sql_number,
        SqlToken::Comment => theme.sql_comment,
    }
}

/// The byte offset of the character boundary preceding `offset`.
fn previous_boundary(text: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let mut index = offset - 1;
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The byte offset of the character boundary following `offset`.
fn next_boundary(text: &str, offset: usize) -> usize {
    if offset >= text.len() {
        return text.len();
    }
    let mut index = offset + 1;
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
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

/// The byte range of the line containing `offset`, excluding the trailing newline.
fn line_bounds(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let start = text[..offset]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let end = text[offset..]
        .find('\n')
        .map(|index| offset + index)
        .unwrap_or(text.len());
    (start, end)
}

/// The byte range of the identifier-like word around `offset` (alphanumerics and `_`).
fn word_bounds(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let is_word = |character: char| character.is_alphanumeric() || character == '_';
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, character)| is_word(*character))
        .last()
        .map(|(index, _)| index)
        .unwrap_or(offset);
    let end = text[offset..]
        .char_indices()
        .take_while(|(_, character)| is_word(*character))
        .last()
        .map(|(index, character)| offset + index + character.len_utf8())
        .unwrap_or(offset);
    (start, end)
}

/// The byte offset of the first character of every line in `text`, starting at 0.
fn sql_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

/// Move the caret one line up (`delta < 0`) or down, keeping the character column.
fn move_vertical(text: &str, offset: usize, delta: isize) -> usize {
    let (line_start, _) = line_bounds(text, offset);
    let column = text[line_start..offset.min(text.len())].chars().count();

    if delta < 0 {
        if line_start == 0 {
            return 0;
        }
        let previous_end = line_start - 1;
        let (previous_start, _) = line_bounds(text, previous_end);
        let mut boundary = previous_start;
        for _ in 0..column {
            if boundary >= previous_end {
                break;
            }
            boundary = next_boundary(text, boundary);
        }
        boundary
    } else {
        let (_, line_end) = line_bounds(text, offset);
        if line_end >= text.len() {
            return text.len();
        }
        let next_start = line_end + 1;
        let (_, next_end) = line_bounds(text, next_start);
        let mut boundary = next_start;
        for _ in 0..column {
            if boundary >= next_end {
                break;
            }
            boundary = next_boundary(text, boundary);
        }
        boundary
    }
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
            self.synced_input_theme = Some(self.theme);
        }
        self.sync_cached_children(cx);
        self.ensure_query_combos(cx);
        self.sync_db_combos(cx);
        self.sync_query_combos(cx);
        self.sync_save_location_combo(cx);
        self.sync_language_combo(cx);
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

        if self.query_focus_pending {
            window.focus(&self.query_focus, cx);
            self.query_focus_pending = false;
        }

        self.query_editor_focused = self.query_focus.is_focused(window);

        if self.query_editor_focused && !self.caret_blink_running {
            self.caret_blink_running = true;
            let executor = cx.background_executor().clone();
            cx.spawn(async move |this, cx| {
                loop {
                    executor.timer(Duration::from_millis(530)).await;
                    let keep_going = this.update(cx, |view, cx| {
                        if view.query_editor_focused {
                            view.caret_visible = !view.caret_visible;
                            cx.notify();
                            true
                        } else {
                            view.caret_blink_running = false;
                            view.caret_visible = true;
                            false
                        }
                    });
                    if !matches!(keep_going, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
        }

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .overflow_hidden()
            .child(self.render_sidebar())
            .child(self.render_content(window, cx));

        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.window_bg))
            .text_color(rgb(theme.text))
            .text_size(px(12.5))
            .child(render_titlebar(theme))
            .child(self.render_menu_bar(cx))
            .child(self.render_main_toolbar(cx))
            .child(body);

        if self.tools_menu_open {
            root = root.child(self.render_tools_menu(cx));
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

        self.query_editor_measured = self.active_query.is_some();

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

/// The icon that represents a connection, keyed by its engine id.
fn driver_icon(driver: &str) -> &'static str {
    match driver {
        "mysql" => "icons/mysql.svg",
        _ => "icons/connection.svg",
    }
}

/// A connection icon. Engine logos with fine detail are drawn as a white glyph on a solid badge
/// so they stay legible on both light and dark backgrounds; `badge` also carries the connection
/// status color.
fn tree_driver_icon(driver: &str, color: u32, badge: u32) -> AnyElement {
    let path = driver_icon(driver);
    if driver == "mysql" {
        return div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(16.0))
            .h(px(16.0))
            .flex_none()
            .bg(rgb(badge))
            .child(
                svg()
                    .path(path)
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(0xffffff)),
            )
            .into_any_element();
    }
    tree_icon(path, color).into_any_element()
}

fn tree_icon(path: &'static str, color: u32) -> Svg {
    svg()
        .path(path)
        .w(px(16.0))
        .h(px(16.0))
        .flex_none()
        .text_color(rgb(color))
}

fn tree_message(text: String, indent: f32, color: u32) -> impl IntoElement {
    div()
        .pl(px(indent))
        .pr_2()
        .py_0p5()
        .text_color(rgb(color))
        .child(text)
}

fn render_titlebar(theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(px(32.0))
        .flex_none()
        .bg(rgb(theme.titlebar_bg))
        .border_b_1()
        .border_color(rgb(theme.border))
        .child(
            div()
                .id("titlebar-drag")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .h_full()
                .px_3()
                .text_sm()
                .window_control_area(WindowControlArea::Drag)
                .child(
                    img(ImageSource::Resource(Resource::Embedded("logo.png".into())))
                        .w(px(18.0))
                        .h(px(18.0))
                        .flex_none(),
                )
                .child(t!("app.title").to_string()),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h_full()
                .child(titlebar_button(
                    "titlebar-min",
                    "—",
                    theme,
                    |window, _cx| {
                        window.minimize_window();
                    },
                ))
                .child(titlebar_button(
                    "titlebar-max",
                    "□",
                    theme,
                    |window, _cx| {
                        window.zoom_window();
                    },
                ))
                .child(titlebar_button(
                    "titlebar-close",
                    "✕",
                    theme,
                    |window, _cx| {
                        window.remove_window();
                    },
                )),
        )
}

fn titlebar_button(
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

fn wrap_unit(value: u32, delta: i32, modulus: u32) -> u32 {
    (value as i32 + delta).rem_euclid(modulus as i32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_respect_utf8() {
        let text = "a你好b";
        assert_eq!(previous_boundary(text, text.len()), 7);
        assert_eq!(previous_boundary(text, 7), 4);
        assert_eq!(previous_boundary(text, 4), 1);
        assert_eq!(previous_boundary(text, 1), 0);
        assert_eq!(next_boundary(text, 0), 1);
        assert_eq!(next_boundary(text, 1), 4);
        assert_eq!(next_boundary(text, 4), 7);
        assert_eq!(next_boundary(text, 7), 8);
    }

    #[test]
    fn line_bounds_find_current_line() {
        let text = "select 1\nfrom t\nwhere x";
        assert_eq!(line_bounds(text, 0), (0, 8));
        assert_eq!(line_bounds(text, 8), (0, 8));
        assert_eq!(line_bounds(text, 9), (9, 15));
        assert_eq!(line_bounds(text, 16), (16, 23));
    }

    #[test]
    fn line_starts_cover_every_line() {
        assert_eq!(sql_line_starts(""), vec![0]);
        assert_eq!(sql_line_starts("select 1"), vec![0]);
        assert_eq!(sql_line_starts("select 1\nfrom t\n"), vec![0, 9, 16]);
    }

    #[test]
    fn vertical_movement_keeps_column() {
        let text = "abcd\nef\nghij";
        assert_eq!(move_vertical(text, 0, 1), 5);
        assert_eq!(move_vertical(text, 3, 1), 7);
        assert_eq!(move_vertical(text, 9, -1), 6);
        assert_eq!(move_vertical(text, 5, -1), 0);
        assert_eq!(move_vertical(text, 10, 1), text.len());
    }

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
