//! The drag-resizable side panes: the connection-tree sidebar on the left and the object-info
//! pane on the right.
//!
//! Each pane is a small child `Entity` that owns its own width and its resize divider, so a drag
//! re-renders only that pane (and re-lays-out its neighbours) instead of the whole `AppView`.
//! The info pane observes `AppView`, so it refreshes whenever the selection changes.

use super::*;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::text::TextView;
use rustgrid_core::DatabaseOptions;

/// What the info pane is currently describing.
#[derive(Clone, PartialEq, Eq)]
enum InfoTarget {
    Connection(usize),
    Database(usize, usize),
    Table(usize, usize, String),
    Routine(usize, usize, String, RoutineKind),
    Query(usize),
    BackupFile(usize),
    BackupConfig(usize),
    User(usize, usize),
    None,
}

impl InfoTarget {
    /// A stable key identifying the target, used to load its data only when it changes.
    fn key(&self) -> String {
        match self {
            InfoTarget::Connection(index) => format!("conn:{index}"),
            InfoTarget::Database(connection, database) => format!("db:{connection}:{database}"),
            InfoTarget::Table(connection, database, name) => {
                format!("tbl:{connection}:{database}:{name}")
            }
            InfoTarget::Routine(connection, database, name, kind) => {
                format!("rtn:{connection}:{database}:{}:{name}", kind.sql_name())
            }
            InfoTarget::Query(index) => format!("query:{index}"),
            InfoTarget::BackupFile(index) => format!("bfile:{index}"),
            InfoTarget::BackupConfig(index) => format!("bconfig:{index}"),
            InfoTarget::User(connection, index) => format!("user:{connection}:{index}"),
            InfoTarget::None => "none".to_string(),
        }
    }
}

/// The active table grid the info pane's row form reads its fields from.
struct FormGrid {
    /// The grid's own id, used to notice a switch to another tab's grid.
    id: u64,
    entity: Entity<GridView>,
}

/// One field of the row form: its display column's metadata plus the value to show, with any
/// pending edit already applied.
struct FormField {
    name: String,
    data_type: String,
    comment: String,
    value: String,
    /// The page's own value, ignoring a staged edit. Escape restores this.
    original: String,
    /// Whether the value is SQL `NULL` (shown as an empty field with a `NULL` placeholder).
    null: bool,
}

/// A form field's editor: a single-line input for ordinary values, or a resizable multi-line
/// editor for long text and JSON.
#[derive(Clone)]
enum FormInput {
    Single(Entity<TextInput>),
    Multi(Entity<ui::TextArea>),
}

impl FormInput {
    fn set_text(&self, text: impl Into<String>, cx: &mut App) {
        let text = text.into();
        match self {
            Self::Single(input) => input.update(cx, |input, cx| input.set_text(text, cx)),
            Self::Multi(input) => input.update(cx, |input, cx| input.set_text(text, cx)),
        }
    }

    fn current_text(&self, cx: &App) -> Option<String> {
        match self {
            Self::Single(input) => input.read(cx).current_text(cx),
            Self::Multi(input) => input.read(cx).current_text(cx),
        }
    }

    fn is_focused(&self, window: &Window, cx: &App) -> bool {
        match self {
            Self::Single(input) => input.read(cx).is_focused(window, cx),
            Self::Multi(input) => input.read(cx).is_focused(window, cx),
        }
    }

    fn set_placeholder(&self, placeholder: impl Into<SharedString>, cx: &mut App) {
        let placeholder = placeholder.into();
        match self {
            Self::Single(input) => {
                input.update(cx, |input, cx| input.set_placeholder(placeholder, cx))
            }
            Self::Multi(input) => {
                input.update(cx, |input, cx| input.set_placeholder(placeholder, cx))
            }
        }
    }

    fn set_theme(&self, theme: Theme, cx: &mut App) {
        if let Self::Single(input) = self {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
    }

    fn focus_state(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Single(input) => input.update(cx, |input, cx| input.focus_state(window, cx)),
            Self::Multi(input) => input.update(cx, |input, cx| input.focus_state(window, cx)),
        }
    }

    fn element(&self) -> AnyElement {
        match self {
            Self::Single(input) => input.clone().into_any_element(),
            Self::Multi(input) => input.clone().into_any_element(),
        }
    }
}

/// The default height of a multi-line form editor: three compact input rows.
const FORM_TEXTAREA_HEIGHT: f32 = 72.0;
/// The height of a multi-line editor's header strip (the `json` label + 格式化 switch).
const FORM_JSON_HEADER_HEIGHT: f32 = 28.0;
/// The height of a single-line form editor.
const FORM_INPUT_HEIGHT: f32 = 24.0;

/// Whether a form value should be presented as a JSON code block.
///
/// A JSON/JSONB column counts as one — except when it stores a scalar (`123`, `"text"`, `true`,
/// `null`): those are ordinary values and must keep the plain editor. A non-JSON column also gets
/// the code block when its value is a JSON object or array, regardless of the declared type.
fn form_field_is_json(field: &FormField) -> bool {
    let value = field.value.trim();
    let column_is_json = field.data_type.to_ascii_lowercase().contains("json");
    if value.is_empty() {
        // An empty JSON column is waiting for structured data; an empty ordinary column is not.
        return column_is_json;
    }
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(serde_json::Value::Object(_) | serde_json::Value::Array(_)) => true,
        // A populated JSON column holding valid scalar JSON (the number in the report) is still
        // just a value; a malformed JSON column stays in the code editor so it can be fixed.
        Ok(_) => false,
        Err(_) => column_is_json,
    }
}

fn form_field_is_multiline(field: &FormField) -> bool {
    if form_field_is_json(field) {
        return true;
    }
    if field.value.contains('\n') || field.value.chars().count() > 80 {
        return true;
    }
    let data_type = field.data_type.to_ascii_lowercase();
    ["text", "blob", "clob", "ntext"]
        .iter()
        .any(|kind| data_type.contains(kind))
}

/// Whether the value is already in serde's pretty-printed JSON form (so the 格式化 switch starts
/// on for a field that was stored formatted).
fn form_field_is_pretty(field: &FormField) -> bool {
    if !form_field_is_json(field) {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&field.value) else {
        return false;
    };
    serde_json::to_string_pretty(&value)
        .map(|pretty| pretty.trim() == field.value.trim())
        .unwrap_or(false)
}

/// The row form's field editors, rebuilt when the form moves to another row or grid or the field
/// layout changes. The values themselves are re-read from the grid on every render, so a staged
/// edit or a page reload never leaves the editors out of date.
struct FormState {
    grid_id: u64,
    row: usize,
    /// The grid the edits are staged into (the form's inputs and date picker write through it).
    grid: WeakEntity<GridView>,
    /// Display-column names, in form order.
    columns: Vec<String>,
    inputs: Vec<FormInput>,
    /// Each multi-line editor's dragged height; single-line fields keep [`FORM_INPUT_HEIGHT`].
    heights: Vec<f32>,
    /// Whether each JSON field's 格式化 switch is on (its value is pretty-printed).
    json_formatted: Vec<bool>,
    /// Whether each editor's field is `NULL`, to update its placeholder only when that changes.
    nulls: Vec<bool>,
}

/// The inline date/time picker of one form field, for a temporal column. It mirrors the grid's
/// in-place picker: gpui-kit's calendar is the source of truth for the date, the `TimeField` owns
/// the time of day, and only the footer is app-drawn.
struct FormPicker {
    /// The display column of the field it belongs to.
    col: usize,
    /// Whether the column carries a time of day (`datetime`/`timestamp`).
    has_time: bool,
    calendar: Entity<CalendarState>,
    time_field: Option<Entity<TimeFieldState>>,
}

// ----- Left sidebar ---------------------------------------------------------------------------

/// The connection-tree pane: the cached tree plus a drag handle on its right edge.
pub(super) struct SidebarHost {
    app: WeakEntity<AppView>,
    width: f32,
    /// `(pointer x, width)` captured when the drag started.
    drag: Option<(f32, f32)>,
    /// Keeps the host observing `AppView` so theme changes propagate.
    _subscription: Subscription,
}

impl SidebarHost {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        app_entity: &Entity<AppView>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let subscription = cx.observe(app_entity, |_, _, cx| cx.notify());
        Self {
            app,
            width: SIDEBAR_DEFAULT_WIDTH,
            drag: None,
            _subscription: subscription,
        }
    }

    pub(super) fn drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some((start_x, start_width)) = self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        let delta = f32::from(event.position.x) - start_x;
        self.width = (start_width + delta).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        cx.notify();
    }

    pub(super) fn end_drag(&mut self, cx: &mut Context<'_, Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }
}

impl Render for SidebarHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (theme, tree, search) = match self.app.upgrade() {
            Some(app) => {
                let app = app.read(cx);
                (
                    app.theme,
                    app.tree_pane.clone(),
                    app.tree_search_input.clone(),
                )
            }
            None => return div().into_any_element(),
        };

        let start = cx.listener(|this, event: &MouseDownEvent, _window, cx| {
            this.drag = Some((f32::from(event.position.x), this.width));
            cx.notify();
        });

        // Right-clicking the blank space below the tree opens the tree's blank-area context menu.
        // Rows stop propagation, so only the blank area reaches here.
        let menu = cx.listener(|this, event: &MouseDownEvent, _window, cx| {
            if let Some(app) = this.app.upgrade() {
                app.update(cx, |app, cx| {
                    app.dump_submenu = None;
                    app.move_submenu = None;
                    app.context_menu = Some(ContextMenu {
                        target: ContextTarget::TreeBlank,
                        position: event.position,
                    });
                    cx.notify();
                });
            }
        });

        div()
            .flex()
            .flex_row()
            .h_full()
            .flex_none()
            .child(
                div()
                    .id("sidebar-column")
                    .flex()
                    .flex_col()
                    .w(px(self.width))
                    .h_full()
                    .flex_none()
                    // Keep the tree's vertical scrollbar clear of the resize divider: the
                    // scrollbar is painted at this column's content edge, and without the gutter
                    // it overlapped the separator.
                    .pr(px(6.0))
                    .bg(rgb(theme.sidebar_bg))
                    .border_r_1()
                    .border_color(rgb(theme.border))
                    .on_mouse_down(MouseButton::Right, menu)
                    // The connection tree's search box sits at the top of the sidebar (Navicat's
                    // layout): it filters 连接 → 数据库 → 模式 → 表 (see `sidebar`), and belongs
                    // here rather than inside the scroll area so it stays put while the tree
                    // scrolls.
                    .child(
                        div()
                            .flex_none()
                            .w_full()
                            .px_1()
                            .pt_1()
                            // The tree's blank-area right-click menu belongs to the tree below, not
                            // to the search field above it.
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|_, _, _, cx| cx.stop_propagation()),
                            )
                            .child(div().w_full().h(px(24.0)).child(search)),
                    )
                    .child(tree.cached(cached_style(|d| {
                        d.flex().flex_col().flex_1().min_h(px(0.0)).w_full()
                    }))),
            )
            .child(
                ui::pane_resize_divider("pane-divider-sidebar", theme)
                    .on_mouse_down(MouseButton::Left, start),
            )
            .into_any_element()
    }
}

// ----- Right info pane ------------------------------------------------------------------------

/// The right-hand pane: a drag handle on its left edge, showing either the selected object's
/// details or — while the grid footer's 编辑表单 button is on — the selected row as an editable
/// form (the first selected row when several are selected). The two never show together.
/// How the row form shows the selected row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormMode {
    /// One editor per column (every engine).
    Fields,
    /// The whole row as one JSON document (document stores only).
    Json,
}

pub(super) struct InfoPane {
    app: WeakEntity<AppView>,
    width: f32,
    /// `(pointer x, width)` captured when the drag started.
    drag: Option<(f32, f32)>,
    /// Keeps the info pane observing `AppView` and the connection tree, so selection changes
    /// refresh it.
    _subscription: Subscription,
    _tree_subscription: Subscription,
    /// The table grid observed for the row form. Kept while no row is selected too: a selection
    /// happens inside `GridView` and does not re-render `AppView`, so the observation is what
    /// brings the newly selected row to the pane.
    form_grid: Option<(u64, Subscription)>,
    /// The observed grid itself, for the form's save/cancel actions (the JSON view has no field
    /// inputs to read from).
    form_grid_entity: Option<WeakEntity<GridView>>,
    /// The page row the form is showing, captured on every form render.
    form_row_index: Option<usize>,
    /// The row form's editors while a table row is selected.
    form: Option<FormState>,
    /// Which representation the form shows. A document-store connection starts on JSON because
    /// that is the row's real shape; the field view stays one click away.
    form_mode: FormMode,
    /// The JSON view's editor, created on its first render.
    form_json_editor: Option<Entity<ui::SqlEditor>>,
    /// The editor's live text, mirrored on every keystroke so the form can stage it per row.
    form_json_text: String,
    /// The page row the editor currently belongs to.
    form_json_row: Option<usize>,
    /// The document text the editor was last synchronized with, so typing is never overwritten.
    form_json_loaded: Option<String>,
    /// The form field whose inline date/time picker is open, if any.
    form_picker: Option<FormPicker>,
    /// The form's multi-line editor being resized: `(col, pointer y at start, height at start)`.
    form_resize: Option<(usize, f32, f32)>,
    /// The field list's scroll state; gpui paints no scrollbar for an overflow container, so the
    /// form draws its own from this handle.
    form_scroll: ScrollHandle,
}

impl InfoPane {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        app_entity: &Entity<AppView>,
        tree_entity: &Entity<TreePane>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let subscription = cx.observe(app_entity, |_, _, cx| cx.notify());
        let tree_subscription = cx.observe(tree_entity, |_, _, cx| cx.notify());
        Self {
            app,
            width: INFO_DEFAULT_WIDTH,
            drag: None,
            _subscription: subscription,
            _tree_subscription: tree_subscription,
            form_grid: None,
            form_grid_entity: None,
            form_row_index: None,
            form: None,
            form_mode: FormMode::Fields,
            form_json_editor: None,
            form_json_text: String::new(),
            form_json_row: None,
            form_json_loaded: None,
            form_picker: None,
            form_resize: None,
            form_scroll: ScrollHandle::new(),
        }
    }

    pub(super) fn drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some((start_x, start_width)) = self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        // The divider is on the pane's left edge, so dragging right narrows the pane.
        let delta = f32::from(event.position.x) - start_x;
        self.width = (start_width - delta).clamp(INFO_MIN_WIDTH, INFO_MAX_WIDTH);
        cx.notify();
    }

    pub(super) fn end_drag(&mut self, cx: &mut Context<'_, Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }

    /// The pane's body: the selected row's edit form while the grid footer's 编辑表单 button is on,
    /// the object's details otherwise.
    fn pane_body(
        &mut self,
        app_entity: &Entity<AppView>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> (AnyElement, bool) {
        let (form_open, grid) = {
            let app = app_entity.read(cx);
            (app.form_open, app.form_grid(cx))
        };
        if form_open {
            self.sync_form_grid(grid.as_ref(), cx);
            let row = grid.as_ref().and_then(|grid| form_row(grid, cx));
            self.form_row_index = row;
            if let Some(grid) = grid.as_ref()
                && let Some(row) = row
                && let Some(body) = self.form_body(app_entity, grid, row, window, cx)
            {
                // The form keeps its header fixed and scrolls only its field list.
                return (body, true);
            }
            // Nothing left to edit (the selection was cleared, or its tab closed): turn the form
            // off so the footer button and the pane agree. The details pane keeps the user's own
            // choice and does not pop open.
            self.form = None;
            self.form_picker = None;
            self.form_resize = None;
            if let Some(app) = self.app.upgrade() {
                // Leave the update for after this frame: the pane is being rendered and the grids
                // may still be busy drawing.
                cx.defer(move |cx| {
                    app.update(cx, |app, cx| {
                        if app.form_open {
                            app.form_open = false;
                            // Keep the grid footer's 编辑表单 button in step.
                            app.notify_grids(cx);
                            cx.notify();
                        }
                    });
                });
            }
            return (div().into_any_element(), true);
        }
        self.sync_form_grid(None, cx);
        self.form = None;
        self.form_picker = None;
        self.form_resize = None;
        let app = app_entity.read(cx);
        let weak = self.app.clone();
        (app.info_body(&weak, cx), false)
    }

    /// Keep an observation on the active table grid so a row selection (which happens inside
    /// `GridView`, without re-rendering `AppView`) reaches this pane. The observation stays even
    /// while no row is selected, so selecting one later shows the form.
    fn sync_form_grid(&mut self, grid: Option<&FormGrid>, cx: &mut Context<'_, Self>) {
        let id = grid.map(|grid| grid.id);
        if self.form_grid.as_ref().map(|(current, _)| *current) == id {
            return;
        }
        self.form = None;
        self.form_picker = None;
        self.form_resize = None;
        self.form_json_loaded = None;
        if let Some(grid) = grid {
            // A document-store grid opens on the JSON view (the row is a document); other
            // engines keep the per-field form.
            self.form_mode = if grid.entity.read(cx).state.documents {
                FormMode::Json
            } else {
                FormMode::Fields
            };
        }
        self.form_grid = grid.map(|grid| {
            let subscription = cx.observe(&grid.entity, |_, _, cx| cx.notify());
            (grid.id, subscription)
        });
        self.form_grid_entity = grid.map(|grid| grid.entity.downgrade());
    }

    /// Build the editable form for the selected row, or `None` when there is nothing to show yet
    /// (the page is still loading its columns).
    fn form_body(
        &mut self,
        app_entity: &Entity<AppView>,
        grid: &FormGrid,
        row: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (theme, show_types, show_comments) = {
            let app = app_entity.read(cx);
            (app.theme, app.grid_show_types, app.grid_show_comments)
        };
        let (documents, table, dirty) = {
            let view = grid.entity.read(cx);
            (
                view.state.documents,
                view.state.table.clone(),
                view.has_pending_changes(),
            )
        };
        if !documents {
            // Only a document store has a JSON representation of a row.
            self.form_mode = FormMode::Fields;
        }
        if documents && self.form_mode == FormMode::Json {
            return self.json_form_body(
                app_entity,
                grid,
                row,
                theme,
                show_types,
                show_comments,
                dirty,
                cx,
            );
        }
        let view = grid.entity.read(cx);
        let state = &view.state;
        if row >= state.rows.len() || state.visible_columns == 0 {
            return None;
        }
        let values = &state.rows[row];
        let fields: Vec<FormField> = (0..state.visible_columns)
            .filter_map(|col| {
                let column = state.columns.get(col)?;
                // The page's own value is what Escape restores; a staged edit wins over it for
                // display, exactly like the grid's own cells.
                let (original, page_null) = match values.get(state.source_col(col)) {
                    Some(CellValue::Null) | None => (String::new(), true),
                    Some(cell) => (cell.as_edit_string(), false),
                };
                let (value, null) = match state.edits.get(&(row, col)) {
                    Some(Some(value)) => (value.clone(), false),
                    Some(None) => (String::new(), true),
                    None => (original.clone(), page_null),
                };
                Some(FormField {
                    name: column.name.clone(),
                    data_type: column.data_type.clone(),
                    comment: column.comment.clone(),
                    value,
                    original,
                    null,
                })
            })
            .collect();
        if fields.is_empty() {
            return None;
        }

        let columns: Vec<String> = fields.iter().map(|field| field.name.clone()).collect();
        let rebuild = self.form.as_ref().is_none_or(|form| {
            form.grid_id != grid.id || form.row != row || form.columns != columns
        });
        if rebuild {
            let inputs = build_form_inputs(theme, &fields, &grid.entity, row, cx);
            let heights = fields
                .iter()
                .map(|field| {
                    if form_field_is_json(field) {
                        FORM_TEXTAREA_HEIGHT + FORM_JSON_HEADER_HEIGHT
                    } else if form_field_is_multiline(field) {
                        FORM_TEXTAREA_HEIGHT
                    } else {
                        FORM_INPUT_HEIGHT
                    }
                })
                .collect();
            let json_formatted = fields.iter().map(form_field_is_pretty).collect();
            self.form = Some(FormState {
                grid_id: grid.id,
                row,
                grid: grid.entity.downgrade(),
                columns,
                inputs,
                heights,
                json_formatted,
                nulls: fields.iter().map(|field| field.null).collect(),
            });
            // The picker belonged to the previous row/field layout.
            self.form_picker = None;
            self.form_resize = None;
        }
        let form = self.form.as_mut()?;
        // Mirror the model into the editors. Typing already staged the same value, so this only
        // touches a field whose value changed elsewhere (a reload, undo, the grid's editor).
        // Never programmatically rewrite the field that holds focus: its edit callback is
        // dispatched on the wrapper's next render, so the model can legitimately lag one
        // keystroke behind — setting the text then would drop that keystroke and, because the kit
        // input clears its history on `set_value`, the field's undo stack with it.
        for (index, field) in fields.iter().enumerate() {
            let Some(input) = form.inputs.get(index).cloned() else {
                continue;
            };
            let focused = input.is_focused(window, cx);
            if !focused && input.current_text(cx).as_deref() != Some(field.value.as_str()) {
                input.set_text(field.value.clone(), cx);
            }
            if form.nulls.get(index).copied() != Some(field.null) {
                if let Some(null) = form.nulls.get_mut(index) {
                    *null = field.null;
                }
                let placeholder: SharedString = if field.null { "NULL".into() } else { "".into() };
                input.set_placeholder(placeholder, cx);
            }
            if !focused && let Some(formatted) = form.json_formatted.get_mut(index) {
                let pretty = form_field_is_pretty(field);
                if *formatted != pretty {
                    *formatted = pretty;
                    if let FormInput::Multi(ref editor) = input {
                        editor.update(cx, |editor, cx| editor.set_line_numbers(pretty, cx));
                    }
                }
            }
            input.set_theme(theme, cx);
        }

        let style = FormStyle {
            theme,
            table: &table,
            width: self.width,
            show_types,
            show_comments,
            dirty,
            documents,
            mode: FormMode::Fields,
            scroll: &self.form_scroll,
        };
        let weak = cx.weak_entity();
        Some(render_row_form(
            &style,
            FormBody::Fields {
                fields: &fields,
                form,
                picker: self.form_picker.as_ref(),
            },
            &weak,
        ))
    }

    /// The document-store view of the row form: the selected row as one JSON document, loaded by
    /// its primary key and saved as a whole. Only reached while [`FormMode::Json`] is selected.
    #[allow(clippy::too_many_arguments)]
    fn json_form_body(
        &mut self,
        app_entity: &Entity<AppView>,
        grid: &FormGrid,
        row: usize,
        theme: Theme,
        show_types: bool,
        show_comments: bool,
        dirty: bool,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        grid.entity
            .update(cx, |grid, cx| grid.ensure_row_document(row, cx));
        let (documents, document, staged, loading, error, table) = {
            let view = grid.entity.read(cx);
            (
                view.state.documents,
                view.state.document_json.clone(),
                view.state.document_edits.get(&row).cloned(),
                view.state.document_loading,
                view.state.document_error.clone(),
                view.state.table.clone(),
            )
        };
        if !documents {
            return None;
        }
        let editor = self.form_json_editor(app_entity, cx);
        // The editor follows the row: a staged edit wins over the stored text, and a refreshed
        // document replaces the text only while no edit is staged for this row.
        if let Some(target) = staged.clone().or(document.clone()) {
            let row_changed = self.form_json_row != Some(row);
            let document_changed =
                staged.is_none() && self.form_json_loaded.as_deref() != Some(target.as_str());
            if row_changed || document_changed {
                editor.update(cx, |editor, cx| editor.set_text(target.clone(), cx));
                self.form_json_loaded = Some(target.clone());
                self.form_json_text = target;
            }
        }
        self.form_json_row = Some(row);
        let dirty = dirty || self.document_is_dirty(&editor, document.as_deref(), cx);
        let style = FormStyle {
            theme,
            table: &table,
            width: self.width,
            show_types,
            show_comments,
            dirty,
            documents: true,
            mode: FormMode::Json,
            scroll: &self.form_scroll,
        };
        let weak = cx.weak_entity();
        Some(render_row_form(
            &style,
            FormBody::Json {
                editor,
                loading,
                error: error.as_deref(),
            },
            &weak,
        ))
    }

    /// Switch the form between its field editors and the whole-document JSON view. A pending edit
    /// in the current view blocks the switch: the two representations cannot both be staged
    /// without one overwriting the other.
    fn set_form_mode(&mut self, mode: FormMode, cx: &mut Context<'_, Self>) {
        if mode == self.form_mode {
            return;
        }
        let dirty = self
            .form_grid_entity
            .as_ref()
            .and_then(|grid| grid.upgrade())
            .is_some_and(|grid| grid.read(cx).has_pending_changes());
        if dirty {
            if let Some(app) = self.app.upgrade() {
                app.update(cx, |app, cx| {
                    app.toast(ToastKind::Info, t!("info.form.save_first").to_string(), cx)
                });
            }
            return;
        }
        self.form_mode = mode;
        cx.notify();
    }

    /// The JSON view's editor, created on first use with the app's editor font and the JSON
    /// grammar (the same read-only previews use).
    fn form_json_editor(
        &mut self,
        app_entity: &Entity<AppView>,
        cx: &mut Context<'_, Self>,
    ) -> Entity<ui::SqlEditor> {
        if let Some(editor) = self.form_json_editor.clone() {
            return editor;
        }
        let (font, size) = {
            let app = app_entity.read(cx);
            (app.editor_font(), app.editor_font_size())
        };
        let weak = cx.weak_entity();
        let editor = cx.new(|cx| {
            ui::SqlEditor::new(cx)
                .options(ui::SqlEditorOptions {
                    language: "json".into(),
                    font_family: font.into(),
                    font_size: size,
                    line_number: true,
                    readonly: false,
                })
                .on_change(Rc::new(move |text, cx| {
                    let _ = weak.update(cx, |pane, cx| pane.on_json_editor_changed(text, cx));
                }))
        });
        self.form_json_editor = Some(editor.clone());
        editor
    }

    /// Mirror the JSON editor's text: notify this pane for the dirty state and stage the text on
    /// the current row so switching rows (or tabs) does not lose it.
    fn on_json_editor_changed(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        self.form_json_text = text.to_string();
        if let Some(row) = self.form_json_row
            && let Some(grid) = self
                .form_grid_entity
                .as_ref()
                .and_then(|grid| grid.upgrade())
        {
            grid.update(cx, |grid, _cx| grid.stage_document_edit(row, text));
        }
        cx.notify();
    }

    /// Whether the JSON editor differs from the stored document.
    fn document_is_dirty(
        &self,
        editor: &Entity<ui::SqlEditor>,
        stored: Option<&str>,
        cx: &App,
    ) -> bool {
        stored.is_some_and(|stored| editor.read(cx).live_text(cx) != stored)
    }

    /// Drop an open form date/time picker. Used when the form is closed while the pane is not in
    /// the element tree and therefore would not render its own cleanup.
    pub(super) fn clear_form_picker(&mut self, cx: &mut Context<'_, Self>) {
        if self.form_picker.take().is_some() {
            cx.notify();
        }
    }

    /// Ctrl+S / the form header's 保存: stage every field's live text first (the focused field's
    /// change callback is dispatched on the wrapper's next render, so the grid may not have seen
    /// the last keystroke yet), then save through the grid's normal commit path.
    fn save_form(&mut self, cx: &mut Context<'_, Self>) {
        if self.form_mode == FormMode::Json {
            let Some(grid) = self
                .form_grid_entity
                .as_ref()
                .and_then(|grid| grid.upgrade())
            else {
                return;
            };
            let Some(row) = self.form_row_index else {
                return;
            };
            let Some(editor) = self.form_json_editor.clone() else {
                return;
            };
            let text = editor.read(cx).live_text(cx);
            grid.update(cx, |grid, cx| grid.save_row_document(row, text, cx));
            cx.notify();
            return;
        }
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let Some(grid) = form.grid.upgrade() else {
            return;
        };
        let row = form.row;
        let values: Vec<(usize, String)> = form
            .inputs
            .iter()
            .enumerate()
            .filter_map(|(col, input)| input.current_text(cx).map(|text| (col, text)))
            .collect();
        grid.update(cx, |grid, cx| {
            for (col, text) in &values {
                grid.stage_form_edit(row, *col, text, false, cx);
            }
            grid.save_grid(cx);
        });
        cx.notify();
    }

    /// The form header's 回滚: discard every pending edit (the form and the grid share one staged
    /// edit set) and close the form with the grid's own rollback.
    fn cancel_form(&mut self, cx: &mut Context<'_, Self>) {
        if self.form_mode == FormMode::Json {
            let Some(grid) = self
                .form_grid_entity
                .as_ref()
                .and_then(|grid| grid.upgrade())
            else {
                return;
            };
            let document = grid.read(cx).state.document_json.clone();
            if let (Some(editor), Some(document)) = (self.form_json_editor.clone(), document) {
                editor.update(cx, |editor, cx| editor.set_text(document.clone(), cx));
                self.form_json_text = document.clone();
                self.form_json_loaded = Some(document);
            }
            if let Some(row) = self.form_row_index {
                grid.update(cx, |grid, cx| grid.cancel_document_edit(row, cx));
            }
            cx.notify();
            return;
        }
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let Some(grid) = form.grid.upgrade() else {
            return;
        };
        grid.update(cx, |grid, cx| grid.cancel_edits(cx));
        self.form = None;
        self.form_picker = None;
        cx.notify();
    }

    /// Begin dragging a multi-line field's bottom grip.
    fn start_form_resize(&mut self, col: usize, y: f32, cx: &mut Context<'_, Self>) {
        let Some(height) = self
            .form
            .as_ref()
            .and_then(|form| form.heights.get(col).copied())
        else {
            return;
        };
        self.form_resize = Some((col, y, height));
        cx.notify();
    }

    /// Follow the pointer while a field grip is being dragged, clamped to a usable range.
    fn form_resize_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some((col, start_y, start_height)) = self.form_resize else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.form_resize = None;
            cx.notify();
            return;
        }
        let delta = f32::from(event.position.y) - start_y;
        let height = (start_height + delta).clamp(FORM_INPUT_HEIGHT, 600.0);
        if let Some(form) = self.form.as_mut()
            && let Some(slot) = form.heights.get_mut(col)
        {
            *slot = height;
        }
        cx.notify();
    }

    fn form_resize_end(&mut self, cx: &mut Context<'_, Self>) {
        if self.form_resize.take().is_some() {
            cx.notify();
        }
    }

    /// Pretty-print the JSON in a form field and stage the formatted text into the grid.
    /// The JSON code block's 格式化 switch: on pretty-prints the value, off compacts it. Both forms
    /// stage the rewritten text into the grid; an invalid JSON value leaves the switch untouched.
    fn set_form_json_formatted(&mut self, col: usize, formatted: bool, cx: &mut Context<'_, Self>) {
        let Some((editor, row, grid)) =
            self.form
                .as_ref()
                .and_then(|form| match form.inputs.get(col) {
                    Some(FormInput::Multi(editor)) => {
                        Some((editor.clone(), form.row, form.grid.clone()))
                    }
                    _ => None,
                })
        else {
            return;
        };
        let text = editor.read(cx).current_text(cx).unwrap_or_default();
        if text.trim().is_empty() {
            // Nothing to format yet; just let the switch move so the field does not report an
            // invalid JSON error before the user has typed anything.
            if let Some(form) = self.form.as_mut()
                && let Some(slot) = form.json_formatted.get_mut(col)
            {
                *slot = formatted;
            }
            cx.notify();
            return;
        }
        let value = match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => value,
            Err(_) => {
                if let Some(app) = self.app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.toast(
                            ToastKind::Error,
                            t!("info.form.json_invalid").to_string(),
                            cx,
                        );
                    });
                }
                return;
            }
        };
        let rendered = if formatted {
            serde_json::to_string_pretty(&value)
        } else {
            serde_json::to_string(&value)
        };
        let Ok(rendered) = rendered else {
            return;
        };
        editor.update(cx, |editor, cx| {
            editor.set_text(rendered.clone(), cx);
            editor.set_line_numbers(formatted, cx);
        });
        if let Some(grid) = grid.upgrade() {
            grid.update(cx, |grid, cx| {
                grid.stage_form_edit(row, col, &rendered, true, cx)
            });
        }
        if let Some(form) = self.form.as_mut()
            && let Some(slot) = form.json_formatted.get_mut(col)
        {
            *slot = formatted;
        }
        cx.notify();
    }

    /// Open (or apply-and-close) the inline date/time picker of one form field. Mirrors the grid's
    /// "…" toggle: the picker starts from the field's current text, or the current moment when
    /// that is empty or not a date.
    fn toggle_form_picker(
        &mut self,
        col: usize,
        value: String,
        has_time: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self
            .form_picker
            .as_ref()
            .is_some_and(|picker| picker.col == col)
        {
            self.form_picker_ok(window, cx);
            return;
        }
        let base = parse_datetime(&value).unwrap_or_else(|| chrono::Local::now().naive_local());
        let date = NaiveDate::from_ymd_opt(base.year(), base.month(), base.day())
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let calendar = cx.new(|cx| {
            let mut state = CalendarState::new(window, cx);
            state.set_date(date, window, cx);
            state
        });
        let time_field = has_time.then(|| {
            let time = NaiveTime::from_hms_opt(base.hour(), base.minute(), base.second())
                .unwrap_or_default();
            cx.new(|cx| {
                let mut state = TimeFieldState::new(window, cx).precision(TimePrecision::Second);
                state.set_time(time, window, cx);
                state
            })
        });
        self.form_picker = Some(FormPicker {
            col,
            has_time,
            calendar,
            time_field,
        });
        cx.notify();
    }

    /// Write the picker's date/time into the field's editor and stage it, then close the picker.
    /// The editor is focused again so the form behaves like the grid's cell editor.
    fn form_picker_ok(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(picker) = self.form_picker.take() else {
            return;
        };
        let Some(value) = form_picker_value(&picker, cx) else {
            return;
        };
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let Some(input) = form.inputs.get(picker.col).cloned() else {
            return;
        };
        let (row, col) = (form.row, picker.col);
        if let Some(grid) = form.grid.upgrade() {
            let text = value.clone();
            grid.update(cx, |grid, cx| {
                grid.stage_form_edit(row, col, &text, true, cx)
            });
        }
        input.set_text(value.clone(), cx);
        input.focus_state(window, cx);
        cx.notify();
    }

    /// Move the open picker to today (and the current time), leaving it open to confirm.
    fn form_picker_today(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let now = chrono::Local::now().naive_local();
        let Some(picker) = self.form_picker.as_ref() else {
            return;
        };
        if let Some(date) = NaiveDate::from_ymd_opt(now.year(), now.month(), now.day()) {
            picker
                .calendar
                .update(cx, |calendar, cx| calendar.set_date(date, window, cx));
        }
        if let Some(field) = picker.time_field.as_ref()
            && let Some(time) = NaiveTime::from_hms_opt(now.hour(), now.minute(), now.second())
        {
            field.update(cx, |field, cx| field.set_time(time, window, cx));
        }
        cx.notify();
    }

    /// Close the picker without touching the field's text and focus the editor again.
    fn form_picker_cancel(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(picker) = self.form_picker.take() else {
            return;
        };
        if let Some(input) = self
            .form
            .as_ref()
            .and_then(|form| form.inputs.get(picker.col))
            .cloned()
        {
            input.focus_state(window, cx);
        }
        cx.notify();
    }
}

impl Render for InfoPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let Some(app_entity) = self.app.upgrade() else {
            return div().into_any_element();
        };
        let theme = app_entity.read(cx).theme;
        let (body, self_scrolling) = self.pane_body(&app_entity, window, cx);

        let start = cx.listener(|this, event: &MouseDownEvent, _window, cx| {
            this.drag = Some((f32::from(event.position.x), this.width));
            cx.notify();
        });

        // The row form scrolls only its field list, under a fixed header. The details pane keeps
        // scrolling as a whole.
        let pane_body: AnyElement = if self_scrolling {
            div()
                .w(px(self.width))
                .h_full()
                .flex_none()
                .flex()
                .flex_col()
                .bg(rgb(theme.editor_bg))
                .child(body)
                .into_any_element()
        } else {
            div()
                .id("info-scroll")
                .w(px(self.width))
                .h_full()
                .flex_none()
                .overflow_y_scroll()
                .bg(rgb(theme.editor_bg))
                .child(body)
                .into_any_element()
        };

        div()
            .flex()
            .flex_row()
            .h_full()
            .flex_none()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.form_resize_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.form_resize_end(cx)),
            )
            .child(
                ui::pane_resize_divider("pane-divider-info", theme)
                    .on_mouse_down(MouseButton::Left, start),
            )
            .child(pane_body)
            .into_any_element()
    }
}

impl AppView {
    /// Compute what the info pane should describe from the current selection. The Backup and
    /// Queries main tabs own their own selections; otherwise the connection tree decides.
    fn info_target(&self, cx: &App) -> InfoTarget {
        if self.main_tab == MainTab::Sync {
            // The sync page owns its own inspector; the tree selection must not describe a table
            // that the page is not showing.
            return InfoTarget::None;
        }
        if self.main_tab == MainTab::Backups {
            match self.backup_selected {
                Some(BackupSelection::File(index)) if index < self.backup_files.len() => {
                    return InfoTarget::BackupFile(index);
                }
                Some(BackupSelection::Config(index)) if index < self.backup_configs.len() => {
                    return InfoTarget::BackupConfig(index);
                }
                _ => {}
            }
        }
        if self.main_tab == MainTab::Queries
            && let Some(index) = self.saved_query_selected
            && index < self.query_files.len()
        {
            return InfoTarget::Query(index);
        }

        // The Users tab describes the selected account.
        if self.main_tab == MainTab::Users
            && let (Some(connection), Some(index)) = (self.users_connection, self.selected_user)
            && matches!(&self.users, Loadable::Loaded(users) if index < users.len())
        {
            return InfoTarget::User(connection, index);
        }

        // The routine selection (set by the object list or the connection tree) describes a routine
        // while the Functions tab is showing.
        if self.main_tab == MainTab::Functions
            && let Some((connection, database, name, kind)) = self.info_routine_selected.clone()
        {
            return InfoTarget::Routine(connection, database, name, kind);
        }

        // The table selection (set by the object list or the connection tree) describes a table
        // while the Tables/Views tabs are showing.
        if matches!(self.main_tab, MainTab::Tables | MainTab::Views)
            && let Some((connection, database, name)) = self.info_table_selected.clone()
        {
            return InfoTarget::Table(connection, database, name);
        }

        let tree = self.tree_pane.read(cx);
        if let Some((connection_index, database_index, name, _)) = tree.selected_table.clone() {
            return InfoTarget::Table(connection_index, database_index, name);
        }
        let Some(selected) = tree.selected.clone() else {
            return InfoTarget::None;
        };
        if let Some(rest) = selected.strip_prefix("conn-") {
            if let Ok(index) = rest.parse() {
                return InfoTarget::Connection(index);
            }
        } else if let Some(rest) = selected.strip_prefix("db-") {
            let mut parts = rest.splitn(2, '-');
            if let (Some(connection), Some(database)) = (parts.next(), parts.next())
                && let (Ok(connection), Ok(database)) =
                    (connection.parse::<usize>(), database.parse::<usize>())
            {
                return InfoTarget::Database(connection, database);
            }
        } else if let Some(rest) = selected.strip_prefix("cat-") {
            // A category row describes its database.
            let mut parts = rest.splitn(3, '-');
            if let (Some(connection), Some(database)) = (parts.next(), parts.next())
                && let (Ok(connection), Ok(database)) =
                    (connection.parse::<usize>(), database.parse::<usize>())
            {
                return InfoTarget::Database(connection, database);
            }
        } else if let Some(rest) = selected.strip_prefix("rtn-") {
            // A routine leaf (`rtn-<connection>-<database>-<index>`) describes that routine.
            let mut parts = rest.splitn(3, '-');
            if let (Some(connection), Some(database), Some(routine_index)) =
                (parts.next(), parts.next(), parts.next())
                && let (Ok(connection), Ok(database), Ok(routine_index)) = (
                    connection.parse::<usize>(),
                    database.parse::<usize>(),
                    routine_index.parse::<usize>(),
                )
                && let Some((name, kind)) =
                    self.routine_identity(connection, database, routine_index)
            {
                return InfoTarget::Routine(connection, database, name, kind);
            }
        }
        InfoTarget::None
    }

    /// Record the table the info pane should describe. Called by the object list (single click)
    /// and the connection tree (table selection).
    pub(super) fn set_info_table(
        &mut self,
        connection_index: usize,
        database_index: usize,
        name: String,
    ) {
        self.info_table_selected = Some((connection_index, database_index, name));
        self.info_routine_selected = None;
    }

    /// Record the stored routine the info pane should describe. Called by the object list and the
    /// connection tree.
    pub(super) fn set_info_routine(
        &mut self,
        connection_index: usize,
        database_index: usize,
        name: String,
        kind: RoutineKind,
    ) {
        self.info_routine_selected = Some((connection_index, database_index, name, kind));
        self.info_table_selected = None;
    }

    /// Resolve a connection-tree routine leaf's index to its name and kind, for the info pane.
    fn routine_identity(
        &self,
        connection_index: usize,
        database_index: usize,
        routine_index: usize,
    ) -> Option<(String, RoutineKind)> {
        self.connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index),
                _ => None,
            })
            .and_then(|database| match &database.routines {
                Loadable::Loaded(routines) => routines.get(routine_index),
                _ => None,
            })
            .map(|routine| (routine.name.clone(), routine.kind))
    }

    /// Drop both the table and routine info selections when a connection/database/category row is
    /// selected.
    pub(super) fn clear_info_selection(&mut self) {
        self.info_table_selected = None;
        self.info_routine_selected = None;
    }

    /// Drop only the routine info selection, leaving any table selection in place. Used when the
    /// object list clears its highlight (switching main tabs).
    pub(super) fn clear_info_routine(&mut self) {
        self.info_routine_selected = None;
    }

    /// Kick off the async introspection the newly-selected object needs. Called every render but
    /// only does work when the selection changed.
    pub(super) fn sync_info(&mut self, cx: &mut Context<'_, Self>) {
        let target = self.info_target(cx);
        let key = target.key();
        if self.info_loaded_for.as_deref() == Some(key.as_str()) {
            return;
        }
        self.info_loaded_for = Some(key);
        self.info_server = Loadable::Idle;
        self.info_database = Loadable::Idle;
        self.info_table_status = Loadable::Idle;
        self.info_table_ddl = Loadable::Idle;
        self.info_table_ddl_view = false;
        self.info_table_ddl_copied = false;
        self.info_routine = Loadable::Idle;
        self.info_user = Loadable::Idle;

        match target {
            InfoTarget::Connection(connection_index) => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    self.info_server = Loadable::Failed(t!("info.not_connected").to_string());
                    return;
                };
                let runtime = self.runtime.clone();
                cx.spawn(async move |this, cx| {
                    let version = {
                        let connection = connection.clone();
                        runtime
                            .spawn(async move { connection.server_version().await })
                            .await
                    };
                    let sessions = {
                        let connection = connection.clone();
                        runtime
                            .spawn(async move { connection.session_count().await })
                            .await
                    };
                    let _ = this.update(cx, |app, cx| {
                        app.info_server = combine_server_info(
                            version.map_err(Error::other),
                            sessions.map_err(Error::other),
                        );
                        cx.notify();
                    });
                })
                .detach();
            }
            InfoTarget::Database(connection_index, database_index) => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    self.info_database = Loadable::Failed(t!("info.not_connected").to_string());
                    return;
                };
                let Some(database) = self.database_name(connection_index, database_index) else {
                    return;
                };
                let runtime = self.runtime.clone();
                cx.spawn(async move |this, cx| {
                    let result = runtime
                        .spawn(async move { connection.database_options(&database).await })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        app.info_database = match result {
                            Ok(inner) => loadable(inner),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                        cx.notify();
                    });
                })
                .detach();
            }
            InfoTarget::Table(connection_index, database_index, name) => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    self.info_table_status = Loadable::Failed(t!("info.not_connected").to_string());
                    return;
                };
                let Some(database) = self.database_name(connection_index, database_index) else {
                    return;
                };
                // A view is scripted with the view verb; a table with the table verb.
                let is_view = self.table_is_view(connection_index, database_index, &name);
                let runtime = self.runtime.clone();
                cx.spawn(async move |this, cx| {
                    let status_connection = connection.clone();
                    let ddl_connection = connection.clone();
                    let status_database = database.clone();
                    let ddl_database = database.clone();
                    let status_name = name.clone();
                    let ddl_name = name.clone();
                    let status = runtime
                        .spawn(async move {
                            status_connection
                                .table_status(&status_database, &status_name)
                                .await
                        })
                        .await;
                    let ddl = runtime
                        .spawn(async move {
                            ddl_connection
                                .object_ddl(&ddl_database, &ddl_name, is_view)
                                .await
                        })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        app.info_table_status = match status {
                            Ok(inner) => loadable(inner),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                        app.info_table_ddl = match ddl {
                            Ok(inner) => loadable(inner),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                        cx.notify();
                    });
                })
                .detach();
            }
            InfoTarget::Routine(connection_index, database_index, name, kind) => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    self.info_routine = Loadable::Failed(t!("info.not_connected").to_string());
                    return;
                };
                let Some(database) = self.database_name(connection_index, database_index) else {
                    return;
                };
                let runtime = self.runtime.clone();
                cx.spawn(async move |this, cx| {
                    let result = runtime
                        .spawn(
                            async move { connection.routine_details(&database, kind, &name).await },
                        )
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        app.info_routine = match result {
                            Ok(inner) => loadable(inner),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                        cx.notify();
                    });
                })
                .detach();
            }
            InfoTarget::User(connection_index, index) => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    self.info_user = Loadable::Failed(t!("info.not_connected").to_string());
                    return;
                };
                let account = match &self.users {
                    Loadable::Loaded(users) => users.get(index).cloned(),
                    _ => None,
                };
                let Some(account) = account else {
                    return;
                };
                let runtime = self.runtime.clone();
                cx.spawn(async move |this, cx| {
                    let result = runtime
                        .spawn(async move {
                            connection.user_details(&account.user, &account.host).await
                        })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        app.info_user = match result {
                            Ok(inner) => loadable(inner),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                        cx.notify();
                    });
                })
                .detach();
            }
            _ => {}
        }
    }

    /// The active grid, when it is a table data page whose selected row the info pane can show as
    /// a form. Query results, Redis key pages and the designer's preview grids have their own
    /// layouts and never get one.
    fn form_grid(&self, cx: &App) -> Option<FormGrid> {
        if self.active_query.is_some() || self.active_design.is_some() {
            return None;
        }
        let grid = self.active_grid.and_then(|index| self.grids.get(index))?;
        let state = &grid.read(cx).state;
        let table_page = state.sql.is_none() && state.key_status.is_none() && state.show_toolbar;
        table_page.then(|| FormGrid {
            id: state.id,
            entity: grid.clone(),
        })
    }

    /// The grid footer's 编辑表单 button: show or hide the selected row's form in the right-hand
    /// pane. The button is only enabled while a data row is selected, so no grid is read here:
    /// this runs inside that grid's own update, and reading or notifying it re-entrantly aborts.
    /// The pane itself closes the form again if its row disappears. Opening the form closes the
    /// details pane, since both share the right-hand side; closing leaves it as the user had it.
    pub(super) fn toggle_form(&mut self, cx: &mut Context<'_, Self>) {
        if self.form_open {
            self.form_open = false;
            // The pane may leave the element tree without rendering again, so drop its picker here
            // rather than letting it reappear the next time the form opens.
            let pane = self.info_pane.clone();
            pane.update(cx, |pane, cx| pane.clear_form_picker(cx));
        } else {
            self.form_open = true;
            if self.info_open {
                self.info_open = false;
                self.persist_settings();
            }
        }
        cx.notify();
    }

    /// Build the info pane's contents for the current selection.
    pub(super) fn info_body(&self, app: &WeakEntity<AppView>, cx: &App) -> AnyElement {
        let theme = self.theme;
        match self.info_target(cx) {
            InfoTarget::None => div().into_any_element(),
            InfoTarget::BackupFile(index) => {
                self.backup_info_body(BackupSelection::File(index), theme)
            }
            InfoTarget::BackupConfig(index) => {
                self.backup_info_body(BackupSelection::Config(index), theme)
            }
            InfoTarget::Query(index) => self.saved_query_info(index, theme),
            InfoTarget::Connection(index) => self.connection_info(index, theme),
            InfoTarget::Database(connection, database) => {
                self.database_info(connection, database, theme)
            }
            InfoTarget::Table(connection, database, name) => {
                let documents = app.upgrade().is_some_and(|app| {
                    app.read(cx)
                        .driver_supports(connection, DriverCapability::Documents)
                });
                self.table_info(connection, database, &name, app, theme, documents)
            }
            InfoTarget::Routine(_, _, name, kind) => self.routine_info(&name, kind, theme),
            InfoTarget::User(_, index) => self.user_info(index, theme),
        }
    }

    /// The details pane of the Users main tab: the selected account's attributes, plus whether it
    /// holds the server-wide `SUPER` privilege (loaded with the rest of its details).
    fn user_info(&self, index: usize, theme: Theme) -> AnyElement {
        let account = match &self.users {
            Loadable::Loaded(users) => users.get(index),
            _ => None,
        };
        let Some(account) = account else {
            return div().into_any_element();
        };
        let superuser = match &self.info_user {
            Loadable::Loaded(details) => Some(details.account.is_super_user),
            _ => None,
        };
        let yes_no = |value: bool| t!(if value { "common.yes" } else { "common.no" }).to_string();
        let fields = vec![
            (
                t!("user.field.ssl_type").to_string(),
                if account.ssl_type.is_empty() {
                    "--".to_string()
                } else {
                    account.ssl_type.clone()
                },
            ),
            (
                t!("user.field.max_questions").to_string(),
                account.max_questions.to_string(),
            ),
            (
                t!("user.field.max_updates").to_string(),
                account.max_updates.to_string(),
            ),
            (
                t!("user.field.max_connections").to_string(),
                account.max_connections.to_string(),
            ),
            (
                t!("user.field.max_user_connections").to_string(),
                account.max_user_connections.to_string(),
            ),
            (
                t!("user.field.superuser").to_string(),
                superuser.map(yes_no).unwrap_or_else(|| "--".to_string()),
            ),
        ];
        info_panel(
            "icons/user.svg",
            theme.icon_users,
            &account.label(),
            &t!("common.user"),
            &fields,
        )
    }

    fn connection_info(&self, index: usize, theme: Theme) -> AnyElement {
        let Some(node) = self.connections.get(index) else {
            return div().into_any_element();
        };
        let profile = &node.profile;
        let queries = self
            .query_files
            .iter()
            .filter(|file| file.connection_id == profile.id)
            .count();
        let (version, sessions) = match &self.info_server {
            Loadable::Loaded((version, sessions)) => (version.clone(), format_count(*sessions)),
            Loadable::Loading | Loadable::Idle => ("--".to_string(), "--".to_string()),
            Loadable::Failed(_) => ("--".to_string(), "--".to_string()),
        };
        let fields = vec![
            (t!("info.server_version").to_string(), version),
            (t!("info.sessions").to_string(), sessions),
            (t!("info.host").to_string(), profile.host.clone()),
            (t!("info.port").to_string(), profile.port.to_string()),
            (t!("info.username").to_string(), profile.username.clone()),
            (
                t!("info.settings_location").to_string(),
                self.config.root().display().to_string(),
            ),
            (
                t!("info.encoding").to_string(),
                t!("info.encoding_auto").to_string(),
            ),
            (t!("info.ssh_host").to_string(), "--".to_string()),
            (t!("info.http_tunnel").to_string(), "--".to_string()),
        ];
        info_panel(
            "icons/connection.svg",
            theme.icon_connection,
            &profile.name,
            &format!("{} {}", queries, t!("common.query")),
            &fields,
        )
    }

    fn database_info(
        &self,
        connection_index: usize,
        database_index: usize,
        theme: Theme,
    ) -> AnyElement {
        let Some(name) = self.database_name(connection_index, database_index) else {
            return div().into_any_element();
        };
        let query_count = self
            .connections
            .get(connection_index)
            .map(|node| {
                self.query_files
                    .iter()
                    .filter(|file| file.connection_id == node.profile.id && file.database == name)
                    .count()
            })
            .unwrap_or(0);
        let options = match &self.info_database {
            Loadable::Loaded(options) => options.clone(),
            _ => DatabaseOptions::default(),
        };
        let mut fields = Vec::new();
        if !options.charset.is_empty() {
            fields.push((t!("info.charset").to_string(), options.charset.clone()));
        }
        if !options.collation.is_empty() {
            fields.push((t!("info.collation").to_string(), options.collation.clone()));
        }
        if !options.owner.is_empty() {
            fields.push((t!("database.owner").to_string(), options.owner.clone()));
        }
        if !options.recovery_model.is_empty() {
            fields.push((
                t!("database.recovery_model").to_string(),
                options.recovery_model.clone(),
            ));
        }
        if !options.compatibility_level.is_empty() {
            fields.push((
                t!("database.compatibility_level").to_string(),
                options.compatibility_level.clone(),
            ));
        }
        info_panel(
            "icons/database.svg",
            theme.icon_database_active,
            &name,
            &format!("{} {}", query_count, t!("common.query")),
            &fields,
        )
    }

    fn table_info(
        &self,
        connection_index: usize,
        database_index: usize,
        name: &str,
        app: &WeakEntity<AppView>,
        theme: Theme,
        documents: bool,
    ) -> AnyElement {
        // A view reads as a view; a table's status is loaded from the server.
        let is_view = self.table_is_view(connection_index, database_index, name);
        // The DDL view keeps the same 详细信息 / DDL toggle bar above it, so the pane can be
        // switched back from the script. Returning the bare script here used to drop the bar and
        // strand the user in the DDL view. A document store has no table DDL, so it always shows
        // the details.
        if self.info_table_ddl_view && !documents {
            let body = self.table_ddl_view(name, is_view, theme);
            return self
                .table_info_view_bar(app, theme, false)
                .child(body)
                .into_any_element();
        }
        let empty = TableStatus::default();
        let status = match &self.info_table_status {
            Loadable::Loaded(status) => Some(status),
            _ => None,
        };
        let status = status.unwrap_or(&empty);
        let field = |label: &str, value: Option<String>| {
            (label.to_string(), value.unwrap_or_else(|| "--".to_string()))
        };
        let fields = vec![
            (
                t!("info.rows").to_string(),
                status
                    .rows
                    .map(format_count)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            field(&t!("info.engine"), status.engine.clone()),
            (
                t!("info.auto_increment").to_string(),
                status
                    .auto_increment
                    .map(format_count)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            field(&t!("info.row_format"), status.row_format.clone()),
            field(&t!("info.updated"), status.updated.clone()),
            field(&t!("info.created"), status.created.clone()),
            field(&t!("info.checked"), status.checked.clone()),
            (
                t!("info.index_length").to_string(),
                status
                    .index_length
                    .map(format_size)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            (
                t!("info.data_length").to_string(),
                status
                    .data_length
                    .map(format_size)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            (
                t!("info.max_data_length").to_string(),
                status
                    .max_data_length
                    .map(format_size)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            (
                t!("info.data_free").to_string(),
                status
                    .data_free
                    .map(format_size)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            field(&t!("info.collation"), status.collation.clone()),
            field(&t!("info.create_options"), status.create_options.clone()),
            field(&t!("info.comment"), status.comment.clone()),
        ];
        let kind = if documents {
            t!("common.collection")
        } else if is_view {
            t!("common.view")
        } else {
            t!("common.table")
        };
        let icon = if is_view {
            "icons/views.svg"
        } else {
            "icons/tables.svg"
        };
        let color = if is_view {
            theme.icon_view
        } else {
            theme.icon_table
        };
        let body = info_panel(icon, color, name, &kind, &fields);
        self.table_info_view_bar(app, theme, documents)
            .child(body)
            .into_any_element()
    }

    /// Whether the table/view `name` is a view in its database's loaded catalog.
    fn table_is_view(&self, connection_index: usize, database_index: usize, name: &str) -> bool {
        self.connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index),
                _ => None,
            })
            .is_some_and(|database| {
                matches!(&database.tables, Loadable::Loaded(tables)
                    if tables.iter().any(|table| table.name == name && matches!(table.kind, rustgrid_core::ObjectKind::View)))
            })
    }

    /// The table info pane's two view buttons (详细信息 / DDL), Navicat's icon pair above the
    /// object title. The active one is filled.
    fn table_info_view_bar(&self, app: &WeakEntity<AppView>, theme: Theme, documents: bool) -> Div {
        let details = !self.info_table_ddl_view;
        let ddl = self.info_table_ddl_view;
        let weak_details = app.clone();
        let weak_ddl = app.clone();
        let button = |id: &'static str, icon: &'static str, active: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(px(28.0))
                .h(px(22.0))
                .rounded_sm()
                .cursor_pointer()
                .when(active, |style| style.bg(rgb(theme.button_bg)))
                .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                .child(
                    svg()
                        .path(icon)
                        .w(px(15.0))
                        .h(px(15.0))
                        .text_color(rgb(if active { theme.text } else { theme.text_muted })),
                )
        };
        let mut tabs = div().flex().flex_row().items_center().gap_1().child(
            button("info-view-details", "icons/database.svg", details).on_click(
                move |_event, _window, cx| {
                    let _ = weak_details.update(cx, |app, cx| {
                        app.info_table_ddl_view = false;
                        app.info_table_ddl_copied = false;
                        cx.notify();
                    });
                },
            ),
        );
        if !documents {
            tabs = tabs.child(button("info-view-ddl", "icons/queries.svg", ddl).on_click(
                move |_event, _window, cx| {
                    let _ = weak_ddl.update(cx, |app, cx| {
                        app.info_table_ddl_view = true;
                        app.info_table_ddl_copied = false;
                        cx.notify();
                    });
                },
            ));
        }
        // A copy button for the script, only while the DDL view is showing. It confirms the copy
        // by turning into a green check + 已复制 for a moment, then goes back to 复制.
        if let (true, Loadable::Loaded(Some(ddl))) =
            (self.info_table_ddl_view && !documents, &self.info_table_ddl)
        {
            let ddl = ddl.clone();
            let copied = self.info_table_ddl_copied;
            let weak_copy = app.clone();
            tabs = tabs.child(
                div()
                    .id("info-ddl-copy")
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(22.0))
                    .px_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(rgb(if copied {
                        theme.icon_connection
                    } else {
                        theme.text_muted
                    }))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .child(
                        svg()
                            .path(if copied {
                                "icons/check.svg"
                            } else {
                                "icons/queries.svg"
                            })
                            .w(px(13.0))
                            .h(px(13.0))
                            .text_color(rgb(if copied {
                                theme.icon_connection
                            } else {
                                theme.text_muted
                            })),
                    )
                    .child(if copied {
                        t!("info.ddl_copied").to_string()
                    } else {
                        t!("query.copy").to_string()
                    })
                    .on_click(move |_event, _window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(ddl.clone()));
                        let _ = weak_copy.update(cx, |app, cx| {
                            app.info_table_ddl_copied = true;
                            cx.notify();
                            cx.spawn(async move |app, cx| {
                                cx.background_executor()
                                    .timer(Duration::from_millis(1500))
                                    .await;
                                let _ = app.update(cx, |app, cx| {
                                    if app.info_table_ddl_copied {
                                        app.info_table_ddl_copied = false;
                                        cx.notify();
                                    }
                                });
                            })
                            .detach();
                        });
                    }),
            );
        }
        div().flex().flex_col().w_full().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_3()
                .pt_3()
                .child(tabs),
        )
    }

    /// The table info pane's DDL view: the `CREATE` script in a copyable code block.
    fn table_ddl_view(&self, name: &str, is_view: bool, theme: Theme) -> AnyElement {
        let _ = (name, is_view, theme);
        let script: AnyElement = match &self.info_table_ddl {
            Loadable::Loaded(Some(ddl)) => div()
                .id("info-ddl")
                .p_3()
                .child(
                    TextView::markdown("info-ddl-text", format!("```sql\n{ddl}\n```"))
                        .selectable(true)
                        .text_size(px(12.0)),
                )
                .into_any_element(),
            Loadable::Loaded(None) => div()
                .p_4()
                .text_color(rgb(self.theme.text_muted))
                .child(t!("info.ddl_unavailable").to_string())
                .into_any_element(),
            Loadable::Failed(error) => div()
                .p_4()
                .text_color(rgb(self.theme.danger))
                .child(error.clone())
                .into_any_element(),
            Loadable::Idle | Loadable::Loading => div()
                .p_4()
                .text_color(rgb(self.theme.text_muted))
                .child(t!("common.loading").to_string())
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(script)
            .into_any_element()
    }

    /// The details pane for a stored routine (the Functions tab's selection): its kind, timestamps,
    /// definer, access characteristics and comment, mirroring Navicat's object details page. The
    /// full `CREATE` statement lives in the routine editor's 定义 tab.
    fn routine_info(&self, name: &str, kind: RoutineKind, theme: Theme) -> AnyElement {
        let details = match &self.info_routine {
            Loadable::Loaded(details) => Some(details),
            _ => None,
        };
        let info = details.map(|details| &details.info);
        let field = |label: &str, value: Option<String>| {
            (label.to_string(), value.unwrap_or_else(|| "--".to_string()))
        };
        let mut fields = vec![(
            t!("routine.field.kind").to_string(),
            kind.sql_name().to_string(),
        )];
        if kind == RoutineKind::Function {
            fields.push(field(
                &t!("routine.field.returns"),
                info.map(|info| info.return_type.clone())
                    .filter(|value| !value.is_empty()),
            ));
        }
        fields.push(field(
            &t!("routine.field.definer"),
            info.map(|info| info.definer.clone()),
        ));
        fields.push(field(
            &t!("routine.field.created"),
            info.and_then(|info| info.created.clone()),
        ));
        fields.push(field(
            &t!("routine.field.modified"),
            info.and_then(|info| info.modified.clone()),
        ));
        fields.push(field(
            &t!("routine.field.security"),
            info.map(|info| info.security_type.clone()),
        ));
        fields.push(field(
            &t!("routine.field.data_access"),
            info.map(|info| info.data_access.clone()),
        ));
        fields.push((
            t!("routine.field.deterministic").to_string(),
            info.map(|info| {
                if info.deterministic {
                    t!("common.yes").to_string()
                } else {
                    t!("common.no").to_string()
                }
            })
            .unwrap_or_else(|| "--".to_string()),
        ));
        fields.push(field(
            &t!("routine.field.comment"),
            info.map(|info| info.comment.clone()),
        ));
        if let Some(details) = details {
            fields.push(field(
                &t!("routine.field.sql_mode"),
                Some(details.sql_mode.clone()).filter(|value| !value.is_empty()),
            ));
            fields.push(field(
                &t!("routine.field.charset"),
                Some(details.character_set_client.clone()).filter(|value| !value.is_empty()),
            ));
            fields.push(field(
                &t!("routine.field.collation"),
                Some(details.collation_connection.clone()).filter(|value| !value.is_empty()),
            ));
        }
        info_panel(
            routine_icon(kind),
            routine_icon_color(kind, theme),
            name,
            t!(kind.label_key()).as_ref(),
            &fields,
        )
    }

    fn saved_query_info(&self, index: usize, theme: Theme) -> AnyElement {
        let Some(file) = self.query_files.get(index) else {
            return div().into_any_element();
        };
        let connection = self.query_connection_name(&file.connection_id);
        let created = file.created.map(format_system_time);
        let modified = file.modified.map(format_system_time);
        let fields = vec![
            (
                t!("backup.field.file").to_string(),
                file.path.display().to_string(),
            ),
            (t!("backup.field.size").to_string(), format_size(file.size)),
            (
                t!("backup.field.created").to_string(),
                created.unwrap_or_else(|| "--".to_string()),
            ),
            (
                t!("backup.field.modified").to_string(),
                modified.unwrap_or_else(|| "--".to_string()),
            ),
            (t!("info.connection").to_string(), connection),
            (
                t!("info.database").to_string(),
                if file.database.is_empty() {
                    "--".to_string()
                } else {
                    file.database.clone()
                },
            ),
        ];
        info_panel(
            "icons/queries.svg",
            theme.icon_queries,
            &file.name,
            &t!("common.query"),
            &fields,
        )
    }
}

/// Format a filesystem timestamp for the info pane.
fn format_system_time(time: std::time::SystemTime) -> String {
    let time: chrono::DateTime<chrono::Local> = time.into();
    time.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Load an inner `Result` into a `Loadable`, stringifying the error.
fn loadable<T>(result: rustgrid_core::Result<T>) -> Loadable<T> {
    match result {
        Ok(value) => Loadable::Loaded(value),
        Err(error) => Loadable::Failed(error.to_string()),
    }
}

fn combine_server_info(
    version: rustgrid_core::Result<rustgrid_core::Result<String>>,
    sessions: rustgrid_core::Result<rustgrid_core::Result<u64>>,
) -> Loadable<(String, u64)> {
    let version = match version {
        Ok(Ok(version)) => version,
        Ok(Err(error)) => return Loadable::Failed(error.to_string()),
        Err(error) => return Loadable::Failed(error.to_string()),
    };
    let sessions = match sessions {
        Ok(Ok(sessions)) => sessions,
        Ok(Err(error)) => return Loadable::Failed(error.to_string()),
        Err(error) => return Loadable::Failed(error.to_string()),
    };
    Loadable::Loaded((version, sessions))
}

/// The first selected row of a table grid that actually holds data (a pending insert row has no
/// page values to show), as the page row index.
fn form_row(grid: &FormGrid, cx: &App) -> Option<usize> {
    let view = grid.entity.read(cx);
    let state = &view.state;
    let rows = state.rows.len();
    state
        .selection
        .as_ref()?
        .row_indices()
        .into_iter()
        .find_map(|row| match view.row_at(row) {
            Some(GridRow::Data(data)) if data < rows => Some(data),
            _ => None,
        })
}

/// Build one editor per form field: a [`TextInput`] for ordinary values, a [`ui::TextArea`] for long
/// text and JSON. Every edit is staged straight into the grid, so the form and the grid's own cells
/// always agree on the pending value; Enter saves the staged edits (multi-line fields insert a
/// newline instead) and Escape restores the page's value for this field.
///
/// Typing stages **without notifying** the grid: a per-keystroke repaint of the grid (which clones
/// its whole page) and of the pane (which rebuilds every field) made typing, and IME composition
/// in particular, lag badly. The grid still repaints once when the edit first appears or is
/// dropped, and again when the field loses focus.
fn build_form_inputs(
    theme: Theme,
    fields: &[FormField],
    grid: &Entity<GridView>,
    row: usize,
    cx: &mut Context<'_, InfoPane>,
) -> Vec<FormInput> {
    fields
        .iter()
        .enumerate()
        .map(|(col, field)| {
            let value = field.value.clone();
            let original = field.original.clone();
            let null = field.null;
            let placeholder: SharedString = if null { "NULL".into() } else { "".into() };
            let changed = grid.downgrade();
            let committed = changed.clone();
            let cancelled = changed.clone();
            if form_field_is_multiline(field) {
                // The field's own entity, so Escape can put its text back before dropping the
                // staged edit (the pane skips a focused field when it mirrors the model).
                let input_slot: Rc<std::cell::RefCell<Option<WeakEntity<ui::TextArea>>>> =
                    Rc::new(std::cell::RefCell::new(None));
                let cancel_slot = input_slot.clone();
                let input = cx.new(|cx| {
                    ui::TextArea::new(
                        theme,
                        value.clone(),
                        ui::TextAreaOptions {
                            json: form_field_is_json(field),
                            line_numbers: form_field_is_pretty(field),
                            placeholder: placeholder.clone(),
                            text_size: 12.0,
                        },
                        cx,
                    )
                    .on_change(Rc::new(move |text, _window, cx| {
                        let _ = changed.clone().update(cx, |grid, cx| {
                            grid.stage_form_edit(row, col, text, false, cx)
                        });
                    }))
                    .on_blur(Rc::new(move |_window, cx| {
                        let _ = committed
                            .clone()
                            .update(cx, |grid, cx| grid.form_field_committed(cx));
                    }))
                    .on_cancel(Rc::new(move |_window, cx| {
                        let Some(input) = cancel_slot.borrow().as_ref().cloned() else {
                            return;
                        };
                        let grid = cancelled.clone();
                        let fallback = original.clone();
                        cx.defer(move |cx| {
                            let value = grid
                                .update(cx, |grid, _cx| grid.form_original_text(row, col))
                                .unwrap_or(fallback);
                            let _ = input.update(cx, |input, cx| input.set_text(value, cx));
                            let _ = grid.update(cx, |grid, cx| grid.cancel_form_edit(row, col, cx));
                        });
                    }))
                });
                *input_slot.borrow_mut() = Some(input.downgrade());
                FormInput::Multi(input)
            } else {
                let submitted = changed.clone();
                // The field's own entity, so Escape can put its text back before dropping the
                // staged edit (the pane skips a focused field when it mirrors the model).
                let input_slot: Rc<std::cell::RefCell<Option<WeakEntity<TextInput>>>> =
                    Rc::new(std::cell::RefCell::new(None));
                let cancel_slot = input_slot.clone();
                let input = cx.new(|cx| {
                    TextInput::new(
                        theme,
                        value.clone(),
                        TextInputOptions {
                            text_size: Some(12.0),
                            placeholder: placeholder.clone(),
                            ..Default::default()
                        },
                        cx,
                    )
                    .on_change(Rc::new(move |text, _window, cx| {
                        let _ = changed.clone().update(cx, |grid, cx| {
                            grid.stage_form_edit(row, col, text, false, cx)
                        });
                    }))
                    .on_submit(Rc::new(move |_window, cx| {
                        let _ = submitted
                            .clone()
                            .update(cx, |grid, cx| grid.submit_form(cx));
                    }))
                    .on_blur(Rc::new(move |_window, cx| {
                        let _ = committed
                            .clone()
                            .update(cx, |grid, cx| grid.form_field_committed(cx));
                    }))
                    .on_cancel(Rc::new(move |_window, cx| {
                        let Some(input) = cancel_slot.borrow().as_ref().cloned() else {
                            return;
                        };
                        let grid = cancelled.clone();
                        let fallback = original.clone();
                        // Escape arrives through this field's own `handle_key_down`, so rewriting
                        // the field must wait until that update has ended; doing it inline would
                        // re-enter the entity. The deferred rewrite also clears the change callback
                        // still queued for the keystroke being undone.
                        cx.defer(move |cx| {
                            let value = grid
                                .update(cx, |grid, _cx| grid.form_original_text(row, col))
                                .unwrap_or(fallback);
                            let _ = input.update(cx, |input, cx| input.set_text(value, cx));
                            let _ = grid.update(cx, |grid, cx| grid.cancel_form_edit(row, col, cx));
                        });
                    }))
                });
                *input_slot.borrow_mut() = Some(input.downgrade());
                FormInput::Single(input)
            }
        })
        .collect()
}

/// The row form's shared layout inputs: the palette, the table it edits and the width the header
/// lines have to fit into.
struct FormStyle<'a> {
    theme: Theme,
    table: &'a str,
    width: f32,
    show_types: bool,
    show_comments: bool,
    /// Whether the grid has something to save or discard; the header's buttons colour up while
    /// it does.
    dirty: bool,
    /// Whether the row can be shown as one document (document stores).
    documents: bool,
    /// Which representation this form is rendering; drives the 表单 / JSON chips.
    mode: FormMode,
    /// The field list's scroll state, for the always-visible scrollbar.
    scroll: &'a ScrollHandle,
}

/// What the form shows below its header.
enum FormBody<'a> {
    /// One editor per column.
    Fields {
        fields: &'a [FormField],
        form: &'a FormState,
        picker: Option<&'a FormPicker>,
    },
    /// The selected row as one JSON document (document stores only).
    Json {
        editor: Entity<ui::SqlEditor>,
        loading: bool,
        error: Option<&'a str>,
    },
}

/// The row form: a compact title (table and a 表单 / JSON switch) over either the per-field editors
/// or the whole selected document as JSON. The right pane is narrow, so each field puts
/// `字段 · 类型 · 注释` on one line with its editor underneath instead of Navicat's three columns.
fn render_row_form(
    style: &FormStyle<'_>,
    body: FormBody<'_>,
    pane: &WeakEntity<InfoPane>,
) -> AnyElement {
    let theme = style.theme;
    // The form's own actions, inside its `RowForm` context so they work while one of its fields
    // holds focus: Ctrl+S saves through the grid's normal commit path, and Escape is the field's
    // own cancel (see `build_form_inputs`).
    let save_pane = pane.clone();
    let cancel_pane = pane.clone();
    let save_weak = pane.clone();
    // Both buttons stay grey until the grid holds a staged edit (or a pending insert) and pick up
    // their action colour only while there is something to save or discard.
    let save_color = if style.dirty {
        theme.icon_connection
    } else {
        theme.text_muted
    };
    let rollback_color = if style.dirty {
        theme.danger
    } else {
        theme.text_muted
    };
    let title = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .child(
            div()
                .id("info-form-table")
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme.text))
                .tooltip(ui::text_tooltip(style.table.to_string()))
                .child(style.table.to_string()),
        )
        .child(
            ui::icon_button(
                "info-form-save",
                "icons/check.svg",
                save_color,
                28.0,
                26.0,
                false,
                theme,
                move |_event, _window, cx| {
                    let _ = save_pane.update(cx, |pane, cx| pane.save_form(cx));
                },
            )
            .tooltip(ui::text_tooltip(t!("info.form.save").to_string())),
        )
        .child(
            ui::icon_button(
                "info-form-rollback",
                "icons/cross.svg",
                rollback_color,
                28.0,
                26.0,
                false,
                theme,
                move |_event, _window, cx| {
                    let _ = cancel_pane.update(cx, |pane, cx| pane.cancel_form(cx));
                },
            )
            .tooltip(ui::text_tooltip(t!("info.form.rollback").to_string())),
        )
        .child({
            // A document store can show the same row as fields or as one JSON document; other
            // engines keep the single 表单 badge.
            let mut chips = div().flex().flex_row().items_center().gap_1();
            if style.documents {
                for (id, label, mode) in [
                    (
                        "info-form-mode-fields",
                        t!("info.form").to_string(),
                        FormMode::Fields,
                    ),
                    ("info-form-mode-json", "JSON".to_string(), FormMode::Json),
                ] {
                    let active = style.mode == mode;
                    let weak = pane.clone();
                    chips = chips.child(
                        div()
                            .id(id)
                            .flex_none()
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_size(px(10.0))
                            .when(active, move |chip| {
                                chip.bg(rgb(theme.button_bg))
                                    .text_color(rgb(theme.text))
                                    .font_weight(FontWeight::SEMIBOLD)
                            })
                            .when(!active, move |chip| {
                                chip.text_color(rgb(theme.text_muted))
                                    .hover(move |style| style.text_color(rgb(theme.text)))
                            })
                            .on_click(move |_event, _window, cx| {
                                let _ = weak.update(cx, |pane, cx| pane.set_form_mode(mode, cx));
                            })
                            .child(label),
                    );
                }
            } else {
                chips = chips.child(
                    div()
                        .flex_none()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .bg(rgb(theme.button_bg))
                        .text_size(px(10.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("info.form").to_string()),
                );
            }
            chips
        });

    let content: AnyElement = match body {
        FormBody::Fields {
            fields,
            form,
            picker,
        } => {
            let mut list = div().flex().flex_col().w_full();
            for (index, field) in fields.iter().enumerate() {
                list = list.child(render_form_field(
                    style,
                    index,
                    field,
                    form.inputs.get(index),
                    form.heights
                        .get(index)
                        .copied()
                        .unwrap_or(FORM_INPUT_HEIGHT),
                    form.json_formatted.get(index).copied().unwrap_or(false),
                    picker.filter(|picker| picker.col == index),
                    pane,
                ));
            }
            div()
                .id("info-form-fields")
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_hidden()
                .child(
                    div()
                        .id("info-form-fields-scroll")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full()
                        .overflow_y_scroll()
                        .track_scroll(style.scroll)
                        .child(div().flex().flex_col().w_full().px_3().child(list)),
                )
                // gpui paints no scrollbar for an overflow container, so the form draws its own.
                .child(
                    Scrollbar::vertical(style.scroll)
                        .mode(ScrollbarMode::Always)
                        .id("info-form-fields-scrollbar"),
                )
                .into_any_element()
        }
        FormBody::Json {
            editor,
            loading,
            error,
        } => {
            let mut body = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .px_3()
                .py_2()
                .gap_1();
            if let Some(error) = error {
                body = body.child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(rgb(theme.danger))
                        .child(error.to_string()),
                );
            }
            if loading {
                body = body.child(
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("common.loading").to_string()),
                );
            } else {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .border_1()
                        .border_color(rgb(theme.border))
                        .rounded_md()
                        .overflow_hidden()
                        .child(editor),
                );
            }
            body.into_any_element()
        }
    };

    div()
        .key_context(ROW_FORM_CONTEXT)
        .on_action(move |_: &SaveRowForm, _window, cx| {
            let _ = save_weak.update(cx, |pane, cx| pane.save_form(cx));
        })
        .flex()
        .flex_col()
        .w_full()
        .h_full()
        // The table name and the ✓ / ✕ stay put; only the field list below scrolls.
        .child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap_1()
                .px_3()
                .pt_3()
                .pb_2()
                .border_b_1()
                .border_color(rgb(theme.border))
                .bg(rgb(theme.editor_bg))
                .child(title),
        )
        .child(content)
        .into_any_element()
}

/// One field of the row form: the `字段 · 类型 · 注释` line above its value editor. Each segment is
/// clipped with an ellipsis when the pane is too narrow, and shows its full text in a hover
/// tooltip in that case. Type and comment only appear while the 表格 options are on.
#[allow(clippy::too_many_arguments)]
fn render_form_field(
    style: &FormStyle<'_>,
    index: usize,
    field: &FormField,
    input: Option<&FormInput>,
    height: f32,
    json_formatted: bool,
    picker: Option<&FormPicker>,
    pane: &WeakEntity<InfoPane>,
) -> AnyElement {
    let theme = style.theme;
    // The pane's own padding (p_3 on both the header and the list) and the divider.
    let content_width = (style.width - 24.0).max(80.0);
    let name_max = content_width * 0.55;
    let type_shown = style.show_types && !field.data_type.is_empty();
    let comment_shown = style.show_comments && !field.comment.is_empty();
    let type_max = content_width * 0.35;

    let name_cut = ui::approx_text_width(&field.name) > name_max;
    let mut name = div()
        .id(SharedString::from(format!("info-form-name-{index}")))
        .flex_none()
        .max_w(px(name_max))
        .truncate()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme.text));
    if name_cut {
        name = name.tooltip(ui::text_tooltip(field.name.clone()));
    }
    let mut header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .w_full()
        .min_w(px(0.0))
        .overflow_hidden()
        .child(name.child(field.name.clone()));

    let separator = || {
        div()
            .flex_none()
            .text_size(px(10.0))
            .text_color(rgb(theme.text_muted))
            .child("·")
    };

    let mut used = ui::approx_text_width(&field.name).min(name_max);
    if type_shown {
        let type_cut = ui::approx_text_width(&field.data_type) > type_max;
        let mut data_type = div()
            .id(SharedString::from(format!("info-form-type-{index}")))
            .flex_none()
            .max_w(px(type_max))
            .truncate()
            .text_size(px(11.0))
            .text_color(rgb(theme.brand));
        if type_cut {
            data_type = data_type.tooltip(ui::text_tooltip(field.data_type.clone()));
        }
        header = header
            .child(separator())
            .child(data_type.child(field.data_type.clone()));
        used += ui::approx_text_width(&field.data_type).min(type_max) + 10.0;
    }
    if comment_shown {
        // The comment takes the room that is left; narrower than its text means it is cut.
        let comment_max = (content_width - used - 10.0).max(20.0);
        let comment_cut = ui::approx_text_width(&field.comment) > comment_max;
        let mut comment = div()
            .id(SharedString::from(format!("info-form-comment-{index}")))
            .flex_1()
            .min_w(px(0.0))
            .truncate()
            .text_size(px(11.0))
            .text_color(rgb(theme.text_muted));
        if comment_cut {
            comment = comment.tooltip(ui::text_tooltip(field.comment.clone()));
        }
        header = header
            .child(separator())
            .child(comment.child(field.comment.clone()));
    }

    if let Some(input) = input {
        // Clicking the label line focuses the value editor, like a form field label.
        let focus_input = input.clone();
        header = header.on_mouse_down(MouseButton::Left, move |_event, window, cx| {
            focus_input.focus_state(window, cx);
        });
    }

    let mut block = div()
        .flex()
        .flex_col()
        .gap_1()
        .w_full()
        .py_2()
        .border_b_1()
        .border_color(rgb(theme.grid_line))
        .child(header);
    match input {
        Some(input) => {
            let temporal = is_temporal_type(&field.data_type);
            let has_time = {
                let data_type = field.data_type.to_ascii_lowercase();
                data_type.contains("datetime") || data_type.contains("timestamp")
            };
            let multiline = matches!(input, FormInput::Multi(_));
            let json = form_field_is_json(field);
            let editor_height = if multiline { height } else { FORM_INPUT_HEIGHT };
            let mut editor = div().flex_1().min_w(px(0.0)).h(px(editor_height));
            if json {
                // A code block: the `json` language label and 格式化 switch sit in a header strip,
                // above the editor's line numbers and syntax colours.
                editor = editor
                    .flex()
                    .flex_col()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.button_border))
                    .bg(rgb(theme.input_bg))
                    .overflow_hidden()
                    .child(form_json_code_header(theme, index, json_formatted, pane))
                    .child(div().flex_1().min_h(px(0.0)).p_1().child(input.element()));
            } else if multiline {
                editor = editor
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.button_border))
                    .bg(rgb(theme.input_bg))
                    .p_1()
                    .overflow_hidden()
                    .child(input.element());
            } else {
                editor = editor.child(input.element());
            }
            let mut value_row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .w_full()
                .child(editor);
            if temporal {
                value_row = value_row.child(form_picker_toggle(
                    theme,
                    index,
                    field.value.clone(),
                    has_time,
                    picker.is_some(),
                    pane,
                ));
            }
            block = block.child(value_row);
            if multiline {
                block = block.child(form_resize_grip(theme, index, pane));
            }
            if let Some(picker) = picker {
                block = block.child(render_form_picker(theme, picker, pane));
            }
        }
        None => {
            block = block.child(
                div()
                    .w_full()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(field.value.clone()),
            );
        }
    }
    block.into_any_element()
}

/// The drag handle under a multi-line form field: dragging it changes that field's editor height.
fn form_resize_grip(theme: Theme, col: usize, pane: &WeakEntity<InfoPane>) -> Stateful<Div> {
    let weak = pane.clone();
    div()
        .id(SharedString::from(format!("info-form-resize-{col}")))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .w_full()
        .h(px(7.0))
        .cursor_row_resize()
        .child(
            div()
                .w(px(28.0))
                .h(px(2.0))
                .rounded_full()
                .bg(rgb(theme.border)),
        )
        .on_mouse_down(MouseButton::Left, move |event, _window, cx| {
            let y = f32::from(event.position.y);
            let _ = weak.update(cx, |pane, cx| pane.start_form_resize(col, y, cx));
            cx.stop_propagation();
        })
}

/// The header strip of a JSON code block: the `json` language label on the left and the 格式化
/// switch on the right. The switch pretty-prints the value when on and compacts it when off.
fn form_json_code_header(
    theme: Theme,
    col: usize,
    formatted: bool,
    pane: &WeakEntity<InfoPane>,
) -> impl IntoElement {
    let weak = pane.clone();
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .w_full()
        .h(px(FORM_JSON_HEADER_HEIGHT))
        .flex_none()
        .px_2()
        .bg(rgb(theme.toolbar_bg))
        .border_b_1()
        .border_color(rgb(theme.button_border))
        .child(
            div()
                .font_family("Consolas")
                .text_size(px(11.0))
                .text_color(rgb(theme.text_muted))
                .child("json"),
        )
        .child(
            Switch::new(SharedString::from(format!("info-form-json-format-{col}")))
                .xsmall()
                .checked(formatted)
                .label(t!("info.form.format").to_string())
                .tooltip(t!("info.form.format_json_hint").to_string())
                .on_change(move |checked, _window, cx| {
                    let checked = *checked;
                    let _ = weak.update(cx, |pane, cx| {
                        pane.set_form_json_formatted(col, checked, cx)
                    });
                }),
        )
}

/// The "…" button a temporal form field shows next to its editor: it opens the field's inline
/// date/time picker (or applies and closes it when already open), like the grid's cell button.
fn form_picker_toggle(
    theme: Theme,
    col: usize,
    value: String,
    has_time: bool,
    open: bool,
    pane: &WeakEntity<InfoPane>,
) -> Stateful<Div> {
    let weak = pane.clone();
    div()
        .id(SharedString::from(format!("info-form-date-{col}")))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(24.0))
        .h(px(24.0))
        .rounded_sm()
        .cursor_pointer()
        .border_1()
        .border_color(rgb(theme.button_border))
        .bg(rgb(if open {
            theme.button_hover_bg
        } else {
            theme.button_bg
        }))
        .text_size(px(11.0))
        .text_color(rgb(if open { theme.primary } else { theme.text }))
        .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
        .child("…")
        .on_click(move |_event, window, cx| {
            let text = value.clone();
            let _ = weak.update(cx, |pane, cx| {
                pane.toggle_form_picker(col, text, has_time, window, cx);
            });
        })
}

/// The inline calendar of a form field's date/time picker: gpui-kit's compact calendar, the
/// optional time-of-day row, and the app-drawn 今天 / 取消 / 确定 footer.
fn render_form_picker(
    theme: Theme,
    picker: &FormPicker,
    pane: &WeakEntity<InfoPane>,
) -> AnyElement {
    let now = chrono::Local::now().naive_local();
    // The mouse-down capture keeps focus where it is (the editor or the time field) when a
    // calendar cell is pressed, exactly like the grid's picker.
    let calendar = div()
        .capture_any_mouse_down(|_event, window, _cx| window.prevent_default())
        .child(ui::compact_calendar(&picker.calendar));
    let time_row: Option<AnyElement> = picker.time_field.as_ref().map(|field| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .pb_1()
            .child(TimeField::new(field).small())
            .into_any_element()
    });

    let today = pane.clone();
    let cancel = pane.clone();
    let ok = pane.clone();
    let footer = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(
            div()
                .id("info-form-date-today")
                .px_1()
                .rounded_sm()
                .text_size(px(11.0))
                .text_color(rgb(theme.text))
                .cursor_pointer()
                .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                .on_click(move |_event, window, cx| {
                    let _ = today.update(cx, |pane, cx| pane.form_picker_today(window, cx));
                })
                .child(format!(
                    "{}: {}/{}/{}",
                    t!("grid.today"),
                    now.year(),
                    now.month(),
                    now.day()
                )),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(ui::popup_button(
                    "info-form-date-cancel",
                    t!("form.cancel").to_string(),
                    false,
                    theme,
                    move |_event, window, cx| {
                        let _ = cancel.update(cx, |pane, cx| pane.form_picker_cancel(window, cx));
                    },
                ))
                .child(ui::popup_button(
                    "info-form-date-ok",
                    t!("form.ok").to_string(),
                    true,
                    theme,
                    move |_event, window, cx| {
                        let _ = ok.update(cx, |pane, cx| pane.form_picker_ok(window, cx));
                    },
                )),
        );

    div()
        .flex()
        .flex_col()
        .gap_1()
        .mt_1()
        .p_1()
        .rounded_md()
        .bg(rgb(theme.dialog_bg))
        .border_1()
        .border_color(rgb(theme.border))
        .child(calendar)
        .children(time_row)
        .child(footer)
        .into_any_element()
}

/// The picker's accepted value: `YYYY-MM-DD` for a date column, `YYYY-MM-DD HH:MM:SS` when the
/// column carries a time of day (the same formats the grid's picker writes).
fn form_picker_value(picker: &FormPicker, cx: &App) -> Option<String> {
    let date = picker.calendar.read(cx).date().start()?;
    Some(if picker.has_time {
        let time = picker
            .time_field
            .as_ref()
            .map(|field| field.read(cx).time())
            .unwrap_or_default();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            date.year(),
            date.month(),
            date.day(),
            time.hour(),
            time.minute(),
            time.second()
        )
    } else {
        format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
    })
}

/// The selectable body of the info pane: a big icon, a title and the label/value fields, all
/// rendered through a `TextView` so the values can be selected and copied with the mouse.
pub(super) fn info_panel(
    icon: &'static str,
    icon_color: u32,
    title: &str,
    subtitle: &str,
    fields: &[(String, String)],
) -> AnyElement {
    let title_markdown = format!(
        "**{}**\n\n{}",
        escape_markdown(title),
        escape_markdown(subtitle)
    );
    let mut fields_markdown = String::new();
    for (label, value) in fields {
        fields_markdown.push_str(&format!(
            "**{}**\n\n{}\n\n",
            escape_markdown(label),
            escape_markdown(value)
        ));
    }
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    svg()
                        .path(icon)
                        .w(px(40.0))
                        .h(px(40.0))
                        .flex_none()
                        .text_color(rgb(icon_color)),
                )
                .child(
                    TextView::markdown("info-pane-title", title_markdown)
                        .selectable(true)
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(15.0)),
                ),
        )
        .child(
            TextView::markdown("info-pane-fields", fields_markdown)
                .selectable(true)
                .text_size(px(12.5)),
        )
        .into_any_element()
}

/// Escape the Markdown control characters that can appear in a value (Windows paths in
/// particular), so the info pane's text view renders it verbatim.
pub(super) fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#'
        ) {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

/// Group an integer with thousands separators.
fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let length = digits.len();
    let mut out = String::with_capacity(length + length / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (length - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(character);
    }
    out
}

/// Human-readable byte size, matching the backup details pane.
fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!(
            "{:.2} GB ({})",
            bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            format_count(bytes)
        )
    } else if bytes >= 1024 * 1024 {
        format!(
            "{:.2} MB ({})",
            bytes as f64 / (1024.0 * 1024.0),
            format_count(bytes)
        )
    } else if bytes >= 1024 {
        format!("{:.2} KB ({})", bytes as f64 / 1024.0, format_count(bytes))
    } else {
        format!("{bytes} B ({bytes})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(data_type: &str, value: &str) -> FormField {
        FormField {
            name: "value".to_string(),
            data_type: data_type.to_string(),
            comment: String::new(),
            value: value.to_string(),
            original: value.to_string(),
            null: false,
        }
    }

    #[test]
    fn json_columns_and_json_values_get_the_json_editor() {
        assert!(form_field_is_json(&field("json", "not checked")));
        assert!(form_field_is_json(&field(
            "varchar",
            "{\"date\": \"2026-11-04\", \"week\": 45}"
        )));
        assert!(form_field_is_json(&field("json", "")));
        assert!(!form_field_is_json(&field("varchar", "hello")));
        // A JSON column that holds a scalar is an ordinary value, not a code block.
        assert!(!form_field_is_json(&field("json", "3312312")));
        assert!(!form_field_is_json(&field("json", "\"a string\"")));
        assert!(!form_field_is_json(&field("json", "null")));
    }

    #[test]
    fn long_text_gets_the_multi_line_editor_and_short_values_do_not() {
        assert!(form_field_is_multiline(&field("varchar", &"x".repeat(81))));
        assert!(form_field_is_multiline(&field("varchar", "a\nb")));
        assert!(form_field_is_multiline(&field("longtext", "")));
        assert!(!form_field_is_multiline(&field("varchar", "short")));
    }
}
