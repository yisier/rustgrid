use super::*;

use navidog_core::{ColumnDef, TableSchema};

/// A row of the field grid, shared by the header and every row.
pub(super) const DESIGN_ROW_HEIGHT: f32 = 24.0;
pub(super) const DESIGN_HEADER_HEIGHT: f32 = 24.0;
pub(super) const DESIGN_GUTTER_WIDTH: f32 = 22.0;
pub(super) const FIELD_NAME_WIDTH: f32 = 246.0;
pub(super) const FIELD_TYPE_WIDTH: f32 = 178.0;
pub(super) const FIELD_LENGTH_WIDTH: f32 = 90.0;
pub(super) const FIELD_DECIMALS_WIDTH: f32 = 90.0;
pub(super) const FIELD_NOTNULL_WIDTH: f32 = 70.0;
pub(super) const FIELD_VIRTUAL_WIDTH: f32 = 65.0;
pub(super) const FIELD_KEY_WIDTH: f32 = 86.0;
pub(super) const FIELD_COMMENT_WIDTH: f32 = 200.0;
pub(super) const DESIGN_DETAIL_HEIGHT: f32 = 152.0;

/// The sub-tabs of the table designer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DesignTab {
    Fields,
    Indexes,
    ForeignKeys,
    Triggers,
    Options,
    Comment,
    Sql,
}

impl DesignTab {
    pub(super) const ALL: [DesignTab; 7] = [
        DesignTab::Fields,
        DesignTab::Indexes,
        DesignTab::ForeignKeys,
        DesignTab::Triggers,
        DesignTab::Options,
        DesignTab::Comment,
        DesignTab::Sql,
    ];

    pub(super) fn label_key(self) -> &'static str {
        match self {
            DesignTab::Fields => "design.tab.fields",
            DesignTab::Indexes => "design.tab.indexes",
            DesignTab::ForeignKeys => "design.tab.foreign_keys",
            DesignTab::Triggers => "design.tab.triggers",
            DesignTab::Options => "design.tab.options",
            DesignTab::Comment => "design.tab.comment",
            DesignTab::Sql => "design.tab.sql",
        }
    }

    pub(super) fn id(self) -> &'static str {
        match self {
            DesignTab::Fields => "design-tab-fields",
            DesignTab::Indexes => "design-tab-indexes",
            DesignTab::ForeignKeys => "design-tab-foreign-keys",
            DesignTab::Triggers => "design-tab-triggers",
            DesignTab::Options => "design-tab-options",
            DesignTab::Comment => "design-tab-comment",
            DesignTab::Sql => "design-tab-sql",
        }
    }
}

/// The editable text columns of the field grid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FieldColumn {
    Name,
    Length,
    Decimals,
    Comment,
}

/// The auxiliary inputs owned by a designer, identified by their callback tag.
#[derive(Clone, Copy)]
enum InputTag {
    Default,
    Engine,
    Charset,
    Collation,
    AutoIncrement,
    Comment,
}

/// A live in-place editor over one cell of the field grid.
pub(super) struct FieldEdit {
    pub(super) row: usize,
    pub(super) column: FieldColumn,
    pub(super) input: Entity<TextInput>,
}

/// The table designer page: owns a working copy of a table's schema, the field grid state and
/// every editor. App-level overlays stay on `AppView`; this view reaches it through a weak handle.
pub(super) struct TableDesignView {
    pub(super) app: WeakEntity<AppView>,
    runtime: Arc<Runtime>,
    pub(super) theme: Theme,
    pub(super) id: u64,
    pub(super) connection: Arc<dyn Connection>,
    pub(super) connection_name: String,
    pub(super) database: String,
    pub(super) table: String,
    pub(super) is_view: bool,

    pub(super) schema: TableSchema,
    pub(super) original: Option<TableSchema>,
    pub(super) tab: DesignTab,
    pub(super) selected_field: Option<usize>,

    pub(super) loading: bool,
    pub(super) saving: bool,
    pub(super) error: Option<String>,
    pub(super) dirty: bool,

    pub(super) column_types: Vec<&'static str>,
    pub(super) edit: Option<FieldEdit>,
    edit_blur: Option<Subscription>,
    pub(super) type_combo: Option<usize>,
    /// The search field shown at the top of the type drop-down; its text filters the list.
    pub(super) type_search: Option<Entity<TextInput>>,
    pub(super) type_query: String,
    /// Each row's type-cell bottom-left in window coordinates, keyed by field index. A shared
    /// single anchor cannot work here because every visible row prepaints and would overwrite it.
    pub(super) combo_anchor: Rc<RefCell<BTreeMap<usize, Point<Pixels>>>>,
    /// The design view's own window-space origin, so the type dropdown can convert the
    /// (window-absolute) prepaint anchor into view-local coordinates.
    pub(super) root_anchor: Rc<RefCell<Point<Pixels>>>,
    pub(super) default_input: Entity<TextInput>,
    pub(super) engine_input: Entity<TextInput>,
    pub(super) charset_input: Entity<TextInput>,
    pub(super) collation_input: Entity<TextInput>,
    pub(super) auto_increment_input: Entity<TextInput>,
    pub(super) comment_input: Entity<TextInput>,

    pub(super) hscroll: ScrollHandle,
    pub(super) hscroll_grab: Option<f32>,
    pub(super) list_scroll: UniformListScrollHandle,
    pub(super) vscroll_grab: Option<f32>,
    pub(super) self_weak: WeakEntity<TableDesignView>,
}

/// A case-insensitive subsequence test used by the type drop-down's search field.
fn is_fuzzy_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|needle| chars.any(|haystack| haystack == needle))
}

fn text_input(
    theme: Theme,
    value: String,
    weak: WeakEntity<TableDesignView>,
    tag: InputTag,
    cx: &mut Context<'_, TableDesignView>,
) -> Entity<TextInput> {
    cx.new(move |cx| {
        TextInput::new(theme, value, TextInputOptions::default(), cx).on_change(Rc::new(
            move |text, _window, cx| {
                let _ = weak.update(cx, |view, cx| view.input_changed(tag, text, cx));
            },
        ))
    })
}

impl TableDesignView {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        id: u64,
        connection: Arc<dyn Connection>,
        connection_name: String,
        database: String,
        table: String,
        is_view: bool,
        app: WeakEntity<AppView>,
        runtime: Arc<Runtime>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let weak = cx.weak_entity();
        let column_types = connection.column_types();
        let default_input = text_input(theme, String::new(), weak.clone(), InputTag::Default, cx);
        let engine_input = text_input(theme, String::new(), weak.clone(), InputTag::Engine, cx);
        let charset_input = text_input(theme, String::new(), weak.clone(), InputTag::Charset, cx);
        let collation_input =
            text_input(theme, String::new(), weak.clone(), InputTag::Collation, cx);
        let auto_increment_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::AutoIncrement,
            cx,
        );
        let comment_input = text_input(theme, String::new(), weak.clone(), InputTag::Comment, cx);

        Self {
            app,
            runtime,
            theme,
            id,
            connection,
            connection_name,
            database,
            table,
            is_view,
            schema: TableSchema::default(),
            original: None,
            tab: DesignTab::Fields,
            selected_field: None,
            loading: false,
            saving: false,
            error: None,
            dirty: false,
            column_types,
            edit: None,
            edit_blur: None,
            type_combo: None,
            type_search: None,
            type_query: String::new(),
            combo_anchor: Rc::new(RefCell::new(BTreeMap::new())),
            root_anchor: Rc::new(RefCell::new(Point::default())),
            default_input,
            engine_input,
            charset_input,
            collation_input,
            auto_increment_input,
            comment_input,
            hscroll: ScrollHandle::new(),
            hscroll_grab: None,
            list_scroll: UniformListScrollHandle::new(),
            vscroll_grab: None,
            self_weak: cx.weak_entity(),
        }
    }

    pub(super) fn load_schema(&mut self, cx: &mut Context<'_, Self>) {
        self.loading = true;
        self.error = None;
        let connection = self.connection.clone();
        let database = self.database.clone();
        let table = self.table.clone();
        let runtime = self.runtime.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.table_schema(&database, &table).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                view.loading = false;
                match result {
                    Ok(schema) => {
                        view.schema = schema.clone();
                        view.original = Some(schema);
                        view.dirty = false;
                        view.selected_field = if view.schema.columns.is_empty() {
                            None
                        } else {
                            Some(0)
                        };
                        view.sync_selected_input(cx);
                        view.sync_option_inputs(cx);
                    }
                    Err(error) => view.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn input_changed(&mut self, tag: InputTag, text: &str, cx: &mut Context<'_, Self>) {
        match tag {
            InputTag::Default => {
                if let Some(index) = self.selected_field
                    && let Some(column) = self.schema.columns.get_mut(index)
                {
                    column.default = text.to_string();
                }
            }
            InputTag::Engine => self.schema.options.engine = text.to_string(),
            InputTag::Charset => self.schema.options.charset = text.to_string(),
            InputTag::Collation => self.schema.options.collation = text.to_string(),
            InputTag::AutoIncrement => self.schema.options.auto_increment = text.to_string(),
            InputTag::Comment => self.schema.options.comment = text.to_string(),
        }
        self.dirty = true;
        cx.notify();
    }

    /// Push the selected field's default into the detail panel's input.
    fn sync_selected_input(&mut self, cx: &mut Context<'_, Self>) {
        let default = self
            .selected_field
            .and_then(|index| self.schema.columns.get(index))
            .map(|column| column.default.clone())
            .unwrap_or_default();
        self.default_input
            .update(cx, |input, cx| input.set_text(default, cx));
    }

    fn sync_option_inputs(&mut self, cx: &mut Context<'_, Self>) {
        let options = self.schema.options.clone();
        self.engine_input
            .update(cx, |input, cx| input.set_text(options.engine, cx));
        self.charset_input
            .update(cx, |input, cx| input.set_text(options.charset, cx));
        self.collation_input
            .update(cx, |input, cx| input.set_text(options.collation, cx));
        self.auto_increment_input
            .update(cx, |input, cx| input.set_text(options.auto_increment, cx));
        self.comment_input
            .update(cx, |input, cx| input.set_text(options.comment, cx));
    }

    fn mark_dirty(&mut self, cx: &mut Context<'_, Self>) {
        self.dirty = true;
        cx.notify();
    }

    fn field_value(&self, row: usize, column: FieldColumn) -> String {
        let Some(column_def) = self.schema.columns.get(row) else {
            return String::new();
        };
        match column {
            FieldColumn::Name => column_def.name.clone(),
            FieldColumn::Length => column_def.length.clone(),
            FieldColumn::Decimals => column_def.decimals.clone(),
            FieldColumn::Comment => column_def.comment.clone(),
        }
    }

    fn set_field_value(
        &mut self,
        row: usize,
        column: FieldColumn,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(column_def) = self.schema.columns.get_mut(row) {
            match column {
                FieldColumn::Name => column_def.name = text.to_string(),
                FieldColumn::Length => column_def.length = text.to_string(),
                FieldColumn::Decimals => column_def.decimals = text.to_string(),
                FieldColumn::Comment => column_def.comment = text.to_string(),
            }
            self.dirty = true;
        }
        cx.notify();
    }

    fn select_field(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.selected_field = Some(index);
        self.sync_selected_input(cx);
        cx.notify();
    }

    pub(super) fn select_tab(&mut self, tab: DesignTab, cx: &mut Context<'_, Self>) {
        self.tab = tab;
        self.edit = None;
        self.edit_blur = None;
        self.type_combo = None;
        if matches!(tab, DesignTab::Options | DesignTab::Comment) {
            self.sync_option_inputs(cx);
        }
        cx.notify();
    }

    pub(super) fn begin_edit(
        &mut self,
        row: usize,
        column: FieldColumn,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if row >= self.schema.columns.len() {
            return;
        }
        let value = self.field_value(row, column);
        let weak = self.self_weak.clone();
        let theme = self.theme;
        let input = cx.new(move |cx| {
            TextInput::new(theme, value, TextInputOptions::default(), cx).on_change(Rc::new(
                move |text, _window, cx| {
                    let _ = weak.update(cx, |view, cx| view.set_field_value(row, column, text, cx));
                },
            ))
        });
        let focus = input.read(cx).focus_handle();
        input.update(cx, |input, cx| input.set_padding_left(0.0, cx));
        self.type_combo = None;
        self.edit = None;
        self.edit_blur = None;
        self.select_field(row, cx);
        self.edit_blur = Some(cx.on_blur(&focus, window, |this, _window, cx| this.finish_edit(cx)));
        self.edit = Some(FieldEdit { row, column, input });
        window.focus(&focus, cx);
        cx.notify();
    }

    fn finish_edit(&mut self, cx: &mut Context<'_, Self>) {
        if self.edit.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn open_type_combo(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.edit = None;
        self.edit_blur = None;
        self.select_field(row, cx);
        if self.type_combo == Some(row) {
            self.type_combo = None;
            cx.notify();
            return;
        }
        self.type_combo = Some(row);
        self.type_query.clear();
        let theme = self.theme;
        let weak = self.self_weak.clone();
        let input = self
            .type_search
            .get_or_insert_with(|| {
                let change = weak.clone();
                let submit = weak.clone();
                let cancel = weak;
                cx.new(move |cx| {
                    TextInput::new(theme, "", TextInputOptions::default(), cx)
                        .on_change(Rc::new(move |text, _window, cx| {
                            let _ = change.update(cx, |view, cx| {
                                view.type_query = text.to_string();
                                cx.notify();
                            });
                        }))
                        .on_submit(Rc::new(move |_window, cx| {
                            let _ = submit.update(cx, |view, cx| view.select_first_type(cx));
                        }))
                        .on_cancel(Rc::new(move |_window, cx| {
                            let _ = cancel.update(cx, |view, cx| {
                                view.type_combo = None;
                                cx.notify();
                            });
                        }))
                })
            })
            .clone();
        input.update(cx, |input, cx| input.set_text("", cx));
        let focus = input.read(cx).focus_handle();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// The type names that match the current search text, best matches first.
    pub(super) fn filtered_types(&self) -> Vec<&'static str> {
        let query = self.type_query.trim().to_lowercase();
        if query.is_empty() {
            return self.column_types.clone();
        }
        let mut substring = Vec::new();
        let mut fuzzy = Vec::new();
        for &data_type in &self.column_types {
            let lower = data_type.to_lowercase();
            if lower.contains(&query) {
                substring.push(data_type);
            } else if is_fuzzy_subsequence(&query, &lower) {
                fuzzy.push(data_type);
            }
        }
        substring.extend(fuzzy);
        substring
    }

    fn select_first_type(&mut self, cx: &mut Context<'_, Self>) {
        let Some(row) = self.type_combo else {
            return;
        };
        if let Some(data_type) = self.filtered_types().first().copied() {
            self.select_type(row, data_type, cx);
        }
    }

    pub(super) fn select_type(&mut self, row: usize, data_type: &str, cx: &mut Context<'_, Self>) {
        if let Some(column) = self.schema.columns.get_mut(row) {
            column.data_type = data_type.to_string();
        }
        self.type_combo = None;
        self.mark_dirty(cx);
    }

    pub(super) fn toggle_not_null(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        if let Some(column) = self.schema.columns.get_mut(row) {
            column.nullable = !column.nullable;
        }
        self.mark_dirty(cx);
    }

    pub(super) fn toggle_auto_increment(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.selected_field
            && let Some(column) = self.schema.columns.get_mut(index)
        {
            column.auto_increment = !column.auto_increment;
            if column.auto_increment {
                column.nullable = false;
            }
        }
        self.mark_dirty(cx);
    }

    pub(super) fn toggle_unsigned(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.selected_field
            && let Some(column) = self.schema.columns.get_mut(index)
        {
            column.unsigned = !column.unsigned;
        }
        self.mark_dirty(cx);
    }

    pub(super) fn toggle_zerofill(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.selected_field
            && let Some(column) = self.schema.columns.get_mut(index)
        {
            column.zerofill = !column.zerofill;
        }
        self.mark_dirty(cx);
    }

    fn unique_field_name(&self) -> String {
        let base = t!("design.new_field").to_string();
        if !self.schema.columns.iter().any(|column| column.name == base) {
            return base;
        }
        let mut counter = 1;
        loop {
            let candidate = format!("{base}_{counter}");
            if !self
                .schema
                .columns
                .iter()
                .any(|column| column.name == candidate)
            {
                return candidate;
            }
            counter += 1;
        }
    }

    fn insert_field_at(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let column = ColumnDef {
            name: self.unique_field_name(),
            data_type: "int".to_string(),
            length: "0".to_string(),
            decimals: "0".to_string(),
            nullable: true,
            ..ColumnDef::default()
        };
        let index = index.min(self.schema.columns.len());
        self.schema.columns.insert(index, column);
        self.selected_field = Some(index);
        self.sync_selected_input(cx);
        self.mark_dirty(cx);
    }

    pub(super) fn add_field(&mut self, cx: &mut Context<'_, Self>) {
        let index = self.schema.columns.len();
        self.insert_field_at(index, cx);
    }

    pub(super) fn insert_field(&mut self, cx: &mut Context<'_, Self>) {
        let index = self.selected_field.map(|index| index + 1).unwrap_or(0);
        self.insert_field_at(index, cx);
    }

    pub(super) fn delete_field(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.selected_field else {
            return;
        };
        if index >= self.schema.columns.len() {
            return;
        }
        self.schema.columns.remove(index);
        self.selected_field = if self.schema.columns.is_empty() {
            None
        } else {
            Some(index.min(self.schema.columns.len() - 1))
        };
        self.sync_selected_input(cx);
        self.mark_dirty(cx);
    }

    pub(super) fn toggle_primary_key(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.selected_field else {
            return;
        };
        if let Some(column) = self.schema.columns.get_mut(index) {
            column.primary_key = !column.primary_key;
            if column.primary_key {
                column.nullable = false;
            }
        }
        self.mark_dirty(cx);
    }

    pub(super) fn move_field(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        let Some(index) = self.selected_field else {
            return;
        };
        let target = index as isize + delta;
        if target < 0 || target as usize >= self.schema.columns.len() {
            return;
        }
        let target = target as usize;
        self.schema.columns.swap(index, target);
        self.selected_field = Some(target);
        self.mark_dirty(cx);
    }

    pub(super) fn preview_sql(&self) -> String {
        self.connection.table_schema_sql(
            &self.database,
            &self.table,
            self.original.as_ref(),
            &self.schema,
        )
    }

    pub(super) fn save(&mut self, cx: &mut Context<'_, Self>) {
        if self.saving {
            return;
        }
        let Some(original) = self.original.clone() else {
            return;
        };
        let sql = self.connection.table_schema_sql(
            &self.database,
            &self.table,
            Some(&original),
            &self.schema,
        );
        if sql.trim().is_empty() {
            self.error = None;
            self.dirty = false;
            cx.notify();
            return;
        }

        self.saving = true;
        self.error = None;
        let connection = self.connection.clone();
        let database = self.database.clone();
        let table = self.table.clone();
        let schema = self.schema.clone();
        let app = self.app.clone();
        let runtime = self.runtime.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn({
                    let connection = connection.clone();
                    let database = database.clone();
                    let table = table.clone();
                    let schema = schema.clone();
                    let original = original.clone();
                    async move {
                        connection
                            .save_table_schema(&database, &table, Some(&original), &schema)
                            .await
                    }
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let succeeded = result.is_ok();
            let _ = this.update(cx, |view, cx| {
                view.saving = false;
                match result {
                    Ok(()) => {
                        view.dirty = false;
                        view.load_schema(cx);
                    }
                    Err(error) => view.error = Some(error.to_string()),
                }
                cx.notify();
            });

            if succeeded && let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| {
                    app.refresh_table_grids(&connection, &database, &table, cx);
                });
            }
        })
        .detach();
    }

    pub(super) fn set_theme(&mut self, theme: Theme, cx: &mut Context<'_, Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        if let Some(input) = self.type_search.clone() {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
        for input in [
            &self.default_input,
            &self.engine_input,
            &self.charset_input,
            &self.collation_input,
            &self.auto_increment_input,
            &self.comment_input,
        ] {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
        cx.notify();
    }
}

impl AppView {
    /// Open (or focus) the designer page for a table.
    pub(super) fn open_design_table(
        &mut self,
        connection_index: usize,
        database: String,
        table: String,
        is_view: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };

        if let Some(index) = self.designs.iter().position(|design| {
            let design = design.read(cx);
            design.database == database
                && design.table == table
                && Arc::ptr_eq(&design.connection, &connection)
        }) {
            self.activate_design(Some(index), cx);
            return;
        }

        let connection_name = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let id = self.next_design_id;
        self.next_design_id += 1;
        let app = cx.weak_entity();
        let runtime = self.runtime.clone();
        let theme = self.theme;
        let entity = cx.new(|cx| {
            TableDesignView::new(
                id,
                connection,
                connection_name,
                database,
                table,
                is_view,
                app,
                runtime,
                theme,
                cx,
            )
        });
        self.designs.push(entity.clone());
        self.active_design = Some(self.designs.len() - 1);
        self.active_grid = None;
        self.active_query = None;
        entity.update(cx, |design, cx| design.load_schema(cx));
        cx.notify();
    }

    pub(super) fn activate_design(&mut self, index: Option<usize>, cx: &mut Context<'_, Self>) {
        self.active_design = index;
        self.active_query = None;
        self.active_grid = None;
        self.query_completion = None;
        cx.notify();
    }

    pub(super) fn close_design(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.designs.len() {
            return;
        }
        self.designs.remove(index);
        match self.active_design {
            Some(active) if active == index => {
                self.active_design = if self.designs.is_empty() {
                    None
                } else {
                    Some(index.min(self.designs.len() - 1))
                };
            }
            Some(active) if active > index => self.active_design = Some(active - 1),
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn close_connection_designs(&mut self, connection: &Arc<dyn Connection>, cx: &App) {
        let active_id = self
            .active_design
            .and_then(|index| self.designs.get(index))
            .map(|design| design.read(cx).id);
        self.designs
            .retain(|design| !Arc::ptr_eq(&design.read(cx).connection, connection));
        self.active_design = active_id.and_then(|id| {
            self.designs
                .iter()
                .position(|design| design.read(cx).id == id)
        });
    }

    /// Re-fetch the open data grids for a table after its structure changed in the designer.
    pub(super) fn refresh_table_grids(
        &mut self,
        connection: &Arc<dyn Connection>,
        database: &str,
        table: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let targets: Vec<Entity<GridView>> = self
            .grids
            .iter()
            .filter(|grid| {
                let grid = grid.read(cx);
                grid.state.sql.is_none()
                    && grid.state.database == database
                    && grid.state.table == table
                    && Arc::ptr_eq(&grid.state.connection, connection)
            })
            .cloned()
            .collect();
        for grid in targets {
            grid.update(cx, |grid, cx| grid.load_page(cx));
        }
    }
}
