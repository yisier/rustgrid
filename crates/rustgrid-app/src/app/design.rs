use super::*;

use std::collections::BTreeSet;

use gpui_kit::component::Size;
use rustgrid_core::{ColumnDef, ForeignKeyDef, IndexDef, TableSchema};

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
pub(super) const FIELD_KEY_WIDTH: f32 = 100.0;
pub(super) const FIELD_COMMENT_WIDTH: f32 = 200.0;
pub(super) const DESIGN_DETAIL_HEIGHT: f32 = 152.0;

// The index grid's columns.
pub(super) const INDEX_NAME_WIDTH: f32 = 200.0;
pub(super) const INDEX_FIELDS_WIDTH: f32 = 340.0;
pub(super) const INDEX_KIND_WIDTH: f32 = 130.0;
pub(super) const INDEX_METHOD_WIDTH: f32 = 120.0;
pub(super) const INDEX_COMMENT_WIDTH: f32 = 240.0;

// The foreign-key grid's columns.
pub(super) const FK_NAME_WIDTH: f32 = 180.0;
pub(super) const FK_COLUMNS_WIDTH: f32 = 200.0;
pub(super) const FK_SCHEMA_WIDTH: f32 = 130.0;
pub(super) const FK_REF_TABLE_WIDTH: f32 = 180.0;
pub(super) const FK_REF_COLUMNS_WIDTH: f32 = 200.0;
pub(super) const FK_DELETE_WIDTH: f32 = 120.0;
pub(super) const FK_UPDATE_WIDTH: f32 = 120.0;

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

/// The rows selected via a designer grid's left gutter, plus the Shift-range anchor. Plain click
/// selects one row, Ctrl/Cmd toggles a row, Shift extends from the anchor — the same scheme the
/// table view uses.
#[derive(Clone, Default)]
pub(super) struct RowSelection {
    pub(super) rows: BTreeSet<usize>,
    pub(super) anchor: Option<usize>,
}

impl RowSelection {
    pub(super) fn contains(&self, row: usize) -> bool {
        self.rows.contains(&row)
    }
}

/// Apply a gutter click to a row selection, following the table view's plain/Ctrl/Shift scheme.
/// `active` is updated to the clicked row so the detail panel and editors follow it.
fn apply_row_selection(
    selection: &mut RowSelection,
    active: &mut Option<usize>,
    row: usize,
    modifiers: &Modifiers,
) {
    if modifiers.shift {
        let anchor = selection.anchor.unwrap_or(row);
        let (low, high) = if anchor <= row {
            (anchor, row)
        } else {
            (row, anchor)
        };
        selection.rows = (low..=high).collect();
        *active = Some(row);
    } else if modifiers.control || modifiers.platform {
        if !selection.rows.remove(&row) {
            selection.rows.insert(row);
        }
        selection.anchor = Some(row);
        *active = Some(row);
    } else {
        selection.rows.clear();
        selection.rows.insert(row);
        selection.anchor = Some(row);
        *active = Some(row);
    }
}

/// The editable text columns of the index grid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum IndexColumn {
    Name,
    Comment,
}

/// Which dropdown of an index row a popup is open for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum IndexComboField {
    Kind,
    Method,
}

/// Per-(row, dropdown) prepaint anchors for the index grid's dropdown cells.
pub(super) type IndexComboAnchors = Rc<RefCell<BTreeMap<(usize, IndexComboField), Point<Pixels>>>>;

/// The index kind shown in the index grid's "Index Type" column. It is stored as the combination
/// of `IndexDef::unique` and `IndexDef::index_type`, so the model needs no extra field.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum IndexKind {
    Normal,
    Unique,
    Fulltext,
    Spatial,
}

impl IndexKind {
    pub(super) const ALL: [IndexKind; 4] = [
        IndexKind::Normal,
        IndexKind::Unique,
        IndexKind::Fulltext,
        IndexKind::Spatial,
    ];

    pub(super) fn label_key(self) -> &'static str {
        match self {
            IndexKind::Normal => "design.index.kind.normal",
            IndexKind::Unique => "design.index.kind.unique",
            IndexKind::Fulltext => "design.index.kind.fulltext",
            IndexKind::Spatial => "design.index.kind.spatial",
        }
    }

    /// The stable value round-tripped through the dropdown callback.
    pub(super) fn value(self) -> &'static str {
        match self {
            IndexKind::Normal => "normal",
            IndexKind::Unique => "unique",
            IndexKind::Fulltext => "fulltext",
            IndexKind::Spatial => "spatial",
        }
    }

    pub(super) fn of(index: &IndexDef) -> IndexKind {
        match index.index_type.to_ascii_uppercase().as_str() {
            "FULLTEXT" => IndexKind::Fulltext,
            "SPATIAL" => IndexKind::Spatial,
            _ if index.unique => IndexKind::Unique,
            _ => IndexKind::Normal,
        }
    }
}

/// The access method shown in the index grid's "Index Method" column, for normal/unique indexes.
pub(super) const INDEX_METHODS: [&str; 2] = ["BTREE", "HASH"];

/// The method of an index, or `None` for fulltext/spatial indexes where it does not apply.
pub(super) fn index_method(index: &IndexDef) -> Option<&'static str> {
    match IndexKind::of(index) {
        IndexKind::Normal | IndexKind::Unique => {
            Some(if index.index_type.eq_ignore_ascii_case("HASH") {
                "HASH"
            } else {
                "BTREE"
            })
        }
        _ => None,
    }
}

/// Add `value` to `list`, or remove it if it is already present.
fn toggle_list(list: &mut Vec<String>, value: &str) {
    if let Some(position) = list.iter().position(|existing| existing == value) {
        list.remove(position);
    } else {
        list.push(value.to_string());
    }
}

/// A live in-place editor over one text cell of the index grid.
pub(super) struct IndexEdit {
    pub(super) row: usize,
    pub(super) column: IndexColumn,
    pub(super) input: Entity<TextInput>,
}

/// The editable text columns of the foreign-key grid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FkColumn {
    Name,
    ReferencedTable,
}

/// Which dropdown of a foreign-key row a popup is open for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FkComboField {
    OnDelete,
    OnUpdate,
}

/// Which picker of a foreign-key row is open: the child columns or the referenced columns.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FkPicker {
    Columns,
    Referenced,
}

/// Per-(row, dropdown) prepaint anchors for the foreign-key grid's dropdown cells.
pub(super) type FkComboAnchors = Rc<RefCell<BTreeMap<(usize, FkComboField), Point<Pixels>>>>;

/// Referential actions offered by the foreign-key dropdowns; the empty string means the engine
/// default (`RESTRICT`).
pub(super) const FK_RULES: [&str; 5] = ["", "CASCADE", "SET NULL", "RESTRICT", "NO ACTION"];

/// A live in-place editor over one text cell of the foreign-key grid.
pub(super) struct FkEdit {
    pub(super) row: usize,
    pub(super) column: FkColumn,
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
    /// True while designing a brand-new table that has not been created yet; the table name is
    /// requested on the first save.
    pub(super) is_new: bool,

    pub(super) schema: TableSchema,
    pub(super) original: Option<TableSchema>,
    pub(super) tab: DesignTab,
    pub(super) selected_field: Option<usize>,
    pub(super) selected_index: Option<usize>,
    /// Multi-row selections from each grid's left gutter.
    pub(super) field_rows: RowSelection,
    pub(super) index_rows: RowSelection,
    pub(super) fk_rows: RowSelection,
    pub(super) index_edit: Option<IndexEdit>,
    index_edit_blur: Option<Subscription>,
    /// Which index row's kind/method dropdown is open.
    pub(super) index_combo: Option<(usize, IndexComboField)>,
    pub(super) index_combo_anchor: IndexComboAnchors,
    /// The "choose fields" popup for the selected index.
    pub(super) index_fields_open: bool,
    pub(super) index_fields_anchor: Rc<RefCell<Point<Pixels>>>,
    pub(super) selected_fk: Option<usize>,
    pub(super) fk_edit: Option<FkEdit>,
    fk_edit_blur: Option<Subscription>,
    /// Which foreign-key row's on-delete/on-update dropdown is open.
    pub(super) fk_combo: Option<(usize, FkComboField)>,
    pub(super) fk_combo_anchor: FkComboAnchors,
    /// The child-columns and referenced-columns pickers for the selected foreign key.
    pub(super) fk_fields_open: bool,
    pub(super) fk_fields_anchor: Rc<RefCell<Point<Pixels>>>,
    pub(super) fk_ref_open: bool,
    pub(super) fk_ref_anchor: Rc<RefCell<Point<Pixels>>>,
    /// Columns of the referenced table, loaded when its picker opens.
    pub(super) fk_ref_columns: Vec<String>,

    pub(super) loading: bool,
    pub(super) saving: bool,
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
    /// Pick-lists for the Options tab, fed by `engines`/`charsets`/`collations`.
    pub(super) engine_combo: Entity<ComboBox>,
    pub(super) charset_combo: Entity<ComboBox>,
    pub(super) collation_combo: Entity<ComboBox>,
    pub(super) engines: Vec<&'static str>,
    pub(super) charsets: Vec<String>,
    pub(super) collations: Vec<String>,
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

/// The width shared by the Options tab's dropdowns (and the database dialog's).
pub(super) const OPTION_COMBO_WIDTH: f32 = 300.0;

/// A pick-list for the Options tab. The chosen value flows through the same
/// [`TableDesignView::input_changed`] path as the old text inputs.
fn option_combo(
    theme: Theme,
    weak: WeakEntity<TableDesignView>,
    tag: InputTag,
    cx: &mut Context<'_, TableDesignView>,
) -> Entity<ComboBox> {
    cx.new(move |cx| {
        ComboBox::new(theme, Vec::new(), String::new(), OPTION_COMBO_WIDTH, cx)
            .field_width(OPTION_COMBO_WIDTH)
            .on_select(Rc::new(move |value, _window, cx| {
                let _ = weak.update(cx, |view, cx| view.input_changed(tag, value, cx));
            }))
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
        is_new: bool,
        app: WeakEntity<AppView>,
        runtime: Arc<Runtime>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let weak = cx.weak_entity();
        let column_types = connection.column_types();
        let engines = connection.storage_engines();
        let default_input = text_input(theme, String::new(), weak.clone(), InputTag::Default, cx);
        let engine_combo = option_combo(theme, weak.clone(), InputTag::Engine, cx);
        let charset_combo = option_combo(theme, weak.clone(), InputTag::Charset, cx);
        let collation_combo = option_combo(theme, weak.clone(), InputTag::Collation, cx);
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
            is_new,
            schema: TableSchema::default(),
            original: None,
            tab: DesignTab::Fields,
            selected_field: None,
            selected_index: None,
            field_rows: RowSelection::default(),
            index_rows: RowSelection::default(),
            fk_rows: RowSelection::default(),
            index_edit: None,
            index_edit_blur: None,
            index_combo: None,
            index_combo_anchor: Rc::new(RefCell::new(BTreeMap::new())),
            index_fields_open: false,
            index_fields_anchor: Rc::new(RefCell::new(Point::default())),
            selected_fk: None,
            fk_edit: None,
            fk_edit_blur: None,
            fk_combo: None,
            fk_combo_anchor: Rc::new(RefCell::new(BTreeMap::new())),
            fk_fields_open: false,
            fk_fields_anchor: Rc::new(RefCell::new(Point::default())),
            fk_ref_open: false,
            fk_ref_anchor: Rc::new(RefCell::new(Point::default())),
            fk_ref_columns: Vec::new(),
            loading: false,
            saving: false,
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
            engine_combo,
            charset_combo,
            collation_combo,
            engines,
            charsets: Vec::new(),
            collations: Vec::new(),
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
                        view.is_new = false;
                        view.dirty = false;
                        view.selected_field = if view.schema.columns.is_empty() {
                            None
                        } else {
                            Some(0)
                        };
                        view.sync_selected_input(cx);
                        view.sync_option_inputs(cx);
                    }
                    Err(error) => view.report_error(error.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Surface a designer error through the app's modal error dialog. Reporting errors inline used
    /// to replace the whole editor body, leaving the user unable to get back to it.
    fn report_error(&self, message: String, cx: &mut Context<'_, Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.error_dialog = Some(message);
                cx.notify();
            });
        }
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
            InputTag::Charset => {
                self.schema.options.charset = text.to_string();
                // Keep the collation consistent with the chosen character set.
                let prefix = format!("{text}_");
                if let Some(first) = self
                    .collations
                    .iter()
                    .find(|candidate| candidate.starts_with(&prefix))
                    .cloned()
                {
                    self.schema.options.collation = first;
                } else {
                    self.schema.options.collation.clear();
                }
            }
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
        self.auto_increment_input
            .update(cx, |input, cx| input.set_text(options.auto_increment, cx));
        self.comment_input
            .update(cx, |input, cx| input.set_text(options.comment, cx));
    }

    /// Load the Options tab's character-set/collation catalogues (storage engines are static).
    pub(super) fn load_option_catalogs(&mut self, cx: &mut Context<'_, Self>) {
        let connection = self.connection.clone();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let charsets = match runtime
                .spawn({
                    let connection = connection.clone();
                    async move { connection.character_sets().await }
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let collations = match runtime
                .spawn(async move { connection.collations().await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                match charsets {
                    Ok(values) => view.charsets = values,
                    Err(error) => view.report_error(error.to_string(), cx),
                }
                match collations {
                    Ok(values) => view.collations = values,
                    Err(error) => view.report_error(error.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Push the Options tab's option lists and current values into the three dropdowns. Idempotent,
    /// so it can run every render.
    pub(super) fn sync_option_combos(&self, cx: &mut Context<'_, Self>) {
        let options = &self.schema.options;
        let engine = options.engine.clone();
        let charset = options.charset.clone();
        let collation = options.collation.clone();

        let engine_options: Vec<ComboOption> = self
            .engines
            .iter()
            .cloned()
            .map(ComboOption::plain)
            .collect();
        let charset_options: Vec<ComboOption> = self
            .charsets
            .iter()
            .cloned()
            .map(ComboOption::plain)
            .collect();
        let prefix = format!("{charset}_");
        let collation_options: Vec<ComboOption> = if charset.is_empty() {
            self.collations
                .iter()
                .cloned()
                .map(ComboOption::plain)
                .collect()
        } else {
            self.collations
                .iter()
                .filter(|candidate| candidate.starts_with(&prefix))
                .cloned()
                .map(ComboOption::plain)
                .collect()
        };

        self.engine_combo.update(cx, |combo, cx| {
            combo.set_options(engine_options, cx);
            combo.set_selected(engine, cx);
        });
        self.charset_combo.update(cx, |combo, cx| {
            combo.set_options(charset_options, cx);
            combo.set_selected(charset, cx);
        });
        self.collation_combo.update(cx, |combo, cx| {
            combo.set_options(collation_options, cx);
            combo.set_selected(collation, cx);
        });
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
        self.field_rows.rows.clear();
        self.field_rows.rows.insert(index);
        self.field_rows.anchor = Some(index);
        self.sync_selected_input(cx);
        cx.notify();
    }

    /// Select one row (or extend/toggle the selection) from the field grid's left gutter.
    pub(super) fn select_field_row(
        &mut self,
        row: usize,
        modifiers: &Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        self.edit = None;
        self.edit_blur = None;
        self.type_combo = None;
        apply_row_selection(
            &mut self.field_rows,
            &mut self.selected_field,
            row,
            modifiers,
        );
        self.sync_selected_input(cx);
        cx.notify();
    }

    pub(super) fn select_tab(&mut self, tab: DesignTab, cx: &mut Context<'_, Self>) {
        self.tab = tab;
        self.edit = None;
        self.edit_blur = None;
        self.type_combo = None;
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_combo = None;
        self.index_fields_open = false;
        self.fk_edit = None;
        self.fk_edit_blur = None;
        self.fk_combo = None;
        self.fk_fields_open = false;
        self.fk_ref_open = false;
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
            TextInput::new(
                theme,
                value,
                TextInputOptions {
                    bare: true,
                    text_size: Some(12.0),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = weak.update(cx, |view, cx| view.set_field_value(row, column, text, cx));
            }))
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
                    TextInput::new(
                        theme,
                        "",
                        TextInputOptions {
                            size: Some(Size::XSmall),
                            text_size: Some(12.0),
                            ..Default::default()
                        },
                        cx,
                    )
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
        self.field_rows.rows.clear();
        self.field_rows.rows.insert(index);
        self.field_rows.anchor = Some(index);
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
        // Delete every gutter-selected row, falling back to the active one.
        let mut targets: BTreeSet<usize> = self.field_rows.rows.clone();
        if let Some(active) = self.selected_field {
            targets.insert(active);
        }
        targets.retain(|row| *row < self.schema.columns.len());
        if targets.is_empty() {
            return;
        }
        let first = *targets.iter().next().unwrap();
        for row in targets.iter().rev() {
            self.schema.columns.remove(*row);
        }
        self.selected_field = if self.schema.columns.is_empty() {
            None
        } else {
            Some(first.min(self.schema.columns.len() - 1))
        };
        self.field_rows.rows.clear();
        if let Some(active) = self.selected_field {
            self.field_rows.rows.insert(active);
            self.field_rows.anchor = Some(active);
        }
        self.sync_selected_input(cx);
        self.mark_dirty(cx);
    }

    pub(super) fn toggle_primary_key(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.selected_field {
            self.toggle_field_primary_key(index, cx);
        }
    }

    /// Toggle the primary-key flag of one row (the grid's "Key" checkbox).
    pub(super) fn toggle_field_primary_key(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        if let Some(column) = self.schema.columns.get_mut(row) {
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

    pub(super) fn add_index(&mut self, cx: &mut Context<'_, Self>) {
        let index = IndexDef {
            name: String::new(),
            columns: Vec::new(),
            unique: false,
            primary: false,
            index_type: "BTREE".to_string(),
            comment: String::new(),
        };
        self.schema.indexes.push(index);
        self.selected_index = Some(self.schema.indexes.len() - 1);
        self.index_rows.rows.clear();
        self.index_rows.rows.insert(self.schema.indexes.len() - 1);
        self.index_rows.anchor = self.selected_index;
        self.mark_dirty(cx);
    }

    pub(super) fn delete_index(&mut self, cx: &mut Context<'_, Self>) {
        // Delete every gutter-selected non-primary index, falling back to the active one.
        let mut targets: BTreeSet<usize> = self.index_rows.rows.clone();
        if let Some(active) = self.selected_index {
            targets.insert(active);
        }
        let targets: Vec<usize> = targets
            .into_iter()
            .filter(|row| {
                self.schema
                    .indexes
                    .get(*row)
                    .is_some_and(|index| !index.primary)
            })
            .collect();
        if targets.is_empty() {
            return;
        }
        let first = targets[0];
        for row in targets.iter().rev() {
            self.schema.indexes.remove(*row);
        }
        self.selected_index = if self.schema.indexes.is_empty() {
            None
        } else {
            Some(first.min(self.schema.indexes.len() - 1))
        };
        self.index_rows.rows.clear();
        if let Some(active) = self.selected_index {
            self.index_rows.rows.insert(active);
            self.index_rows.anchor = Some(active);
        }
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_combo = None;
        self.index_fields_open = false;
        self.mark_dirty(cx);
    }

    pub(super) fn select_index(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        self.selected_index = Some(row);
        self.index_rows.rows.clear();
        self.index_rows.rows.insert(row);
        self.index_rows.anchor = Some(row);
        cx.notify();
    }

    /// Select one row (or extend/toggle the selection) from the index grid's left gutter.
    pub(super) fn select_index_row(
        &mut self,
        row: usize,
        modifiers: &Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_combo = None;
        self.index_fields_open = false;
        apply_row_selection(
            &mut self.index_rows,
            &mut self.selected_index,
            row,
            modifiers,
        );
        cx.notify();
    }

    fn set_index_value(
        &mut self,
        row: usize,
        column: IndexColumn,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(index) = self.schema.indexes.get_mut(row) {
            match column {
                IndexColumn::Name => index.name = text.to_string(),
                IndexColumn::Comment => index.comment = text.to_string(),
            }
            self.dirty = true;
        }
        cx.notify();
    }

    pub(super) fn begin_index_edit(
        &mut self,
        row: usize,
        column: IndexColumn,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(index) = self.schema.indexes.get(row) else {
            return;
        };
        if index.primary {
            return;
        }
        let value = match column {
            IndexColumn::Name => index.name.clone(),
            IndexColumn::Comment => index.comment.clone(),
        };
        let weak = self.self_weak.clone();
        let theme = self.theme;
        let input = cx.new(move |cx| {
            TextInput::new(
                theme,
                value,
                TextInputOptions {
                    bare: true,
                    text_size: Some(12.0),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = weak.update(cx, |view, cx| view.set_index_value(row, column, text, cx));
            }))
        });
        let focus = input.read(cx).focus_handle();
        input.update(cx, |input, cx| input.set_padding_left(0.0, cx));
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_combo = None;
        self.index_fields_open = false;
        self.select_index(row, cx);
        self.index_edit_blur = Some(cx.on_blur(&focus, window, |this, _window, cx| {
            this.finish_index_edit(cx)
        }));
        self.index_edit = Some(IndexEdit { row, column, input });
        window.focus(&focus, cx);
        cx.notify();
    }

    fn finish_index_edit(&mut self, cx: &mut Context<'_, Self>) {
        if self.index_edit.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn open_index_combo(
        &mut self,
        row: usize,
        field: IndexComboField,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(index) = self.schema.indexes.get(row) else {
            return;
        };
        if index.primary || (field == IndexComboField::Method && index_method(index).is_none()) {
            return;
        }
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_fields_open = false;
        self.select_index(row, cx);
        self.index_combo = if self.index_combo == Some((row, field)) {
            None
        } else {
            Some((row, field))
        };
        cx.notify();
    }

    pub(super) fn select_index_combo(
        &mut self,
        row: usize,
        field: IndexComboField,
        value: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(index) = self.schema.indexes.get_mut(row) {
            match field {
                IndexComboField::Kind => match value {
                    "unique" => {
                        index.unique = true;
                        index.index_type = "BTREE".to_string();
                    }
                    "fulltext" => {
                        index.unique = false;
                        index.index_type = "FULLTEXT".to_string();
                    }
                    "spatial" => {
                        index.unique = false;
                        index.index_type = "SPATIAL".to_string();
                    }
                    _ => {
                        index.unique = false;
                        index.index_type = "BTREE".to_string();
                    }
                },
                IndexComboField::Method => index.index_type = value.to_string(),
            }
        }
        self.index_combo = None;
        self.mark_dirty(cx);
    }

    /// Open the "choose fields" popup for one index row.
    pub(super) fn open_index_fields(
        &mut self,
        row: usize,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(index) = self.schema.indexes.get(row) else {
            return;
        };
        if index.primary {
            return;
        }
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_combo = None;
        self.select_index(row, cx);
        self.index_fields_open = true;
        cx.notify();
    }

    /// Add or remove `column` from the selected index's column list.
    pub(super) fn toggle_index_column(
        &mut self,
        row: usize,
        column: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(index) = self.schema.indexes.get_mut(row) {
            if let Some(position) = index.columns.iter().position(|existing| existing == column) {
                index.columns.remove(position);
            } else {
                index.columns.push(column.to_string());
            }
        }
        self.mark_dirty(cx);
    }

    pub(super) fn add_foreign_key(&mut self, cx: &mut Context<'_, Self>) {
        let foreign_key = ForeignKeyDef::default();
        self.schema.foreign_keys.push(foreign_key);
        self.selected_fk = Some(self.schema.foreign_keys.len() - 1);
        self.fk_rows.rows.clear();
        self.fk_rows.rows.insert(self.schema.foreign_keys.len() - 1);
        self.fk_rows.anchor = self.selected_fk;
        self.mark_dirty(cx);
    }

    pub(super) fn delete_foreign_key(&mut self, cx: &mut Context<'_, Self>) {
        // Delete every gutter-selected foreign key, falling back to the active one.
        let mut targets: BTreeSet<usize> = self.fk_rows.rows.clone();
        if let Some(active) = self.selected_fk {
            targets.insert(active);
        }
        targets.retain(|row| *row < self.schema.foreign_keys.len());
        if targets.is_empty() {
            return;
        }
        let first = *targets.iter().next().unwrap();
        for row in targets.iter().rev() {
            self.schema.foreign_keys.remove(*row);
        }
        self.selected_fk = if self.schema.foreign_keys.is_empty() {
            None
        } else {
            Some(first.min(self.schema.foreign_keys.len() - 1))
        };
        self.fk_rows.rows.clear();
        if let Some(active) = self.selected_fk {
            self.fk_rows.rows.insert(active);
            self.fk_rows.anchor = Some(active);
        }
        self.fk_edit = None;
        self.fk_edit_blur = None;
        self.fk_combo = None;
        self.fk_fields_open = false;
        self.fk_ref_open = false;
        self.mark_dirty(cx);
    }

    pub(super) fn select_fk(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        self.selected_fk = Some(row);
        self.fk_rows.rows.clear();
        self.fk_rows.rows.insert(row);
        self.fk_rows.anchor = Some(row);
        cx.notify();
    }

    /// Select one row (or extend/toggle the selection) from the foreign-key grid's left gutter.
    pub(super) fn select_fk_row(
        &mut self,
        row: usize,
        modifiers: &Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        self.fk_edit = None;
        self.fk_edit_blur = None;
        self.fk_combo = None;
        self.fk_fields_open = false;
        self.fk_ref_open = false;
        apply_row_selection(&mut self.fk_rows, &mut self.selected_fk, row, modifiers);
        cx.notify();
    }

    fn set_fk_value(
        &mut self,
        row: usize,
        column: FkColumn,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(foreign_key) = self.schema.foreign_keys.get_mut(row) {
            match column {
                FkColumn::Name => foreign_key.name = text.to_string(),
                FkColumn::ReferencedTable => {
                    if foreign_key.referenced_table != text {
                        // The old column list no longer applies to the new table.
                        foreign_key.referenced_columns.clear();
                    }
                    foreign_key.referenced_table = text.to_string();
                }
            }
            self.dirty = true;
        }
        cx.notify();
    }

    pub(super) fn begin_fk_edit(
        &mut self,
        row: usize,
        column: FkColumn,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(foreign_key) = self.schema.foreign_keys.get(row) else {
            return;
        };
        let value = match column {
            FkColumn::Name => foreign_key.name.clone(),
            FkColumn::ReferencedTable => foreign_key.referenced_table.clone(),
        };
        let weak = self.self_weak.clone();
        let theme = self.theme;
        let input = cx.new(move |cx| {
            TextInput::new(
                theme,
                value,
                TextInputOptions {
                    bare: true,
                    text_size: Some(12.0),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = weak.update(cx, |view, cx| view.set_fk_value(row, column, text, cx));
            }))
        });
        let focus = input.read(cx).focus_handle();
        input.update(cx, |input, cx| input.set_padding_left(0.0, cx));
        self.fk_edit = None;
        self.fk_edit_blur = None;
        self.fk_combo = None;
        self.fk_fields_open = false;
        self.fk_ref_open = false;
        self.select_fk(row, cx);
        self.fk_edit_blur =
            Some(cx.on_blur(&focus, window, |this, _window, cx| this.finish_fk_edit(cx)));
        self.fk_edit = Some(FkEdit { row, column, input });
        window.focus(&focus, cx);
        cx.notify();
    }

    fn finish_fk_edit(&mut self, cx: &mut Context<'_, Self>) {
        if self.fk_edit.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn open_fk_combo(
        &mut self,
        row: usize,
        field: FkComboField,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.schema.foreign_keys.get(row).is_none() {
            return;
        }
        self.fk_edit = None;
        self.fk_edit_blur = None;
        self.fk_fields_open = false;
        self.fk_ref_open = false;
        self.select_fk(row, cx);
        self.fk_combo = if self.fk_combo == Some((row, field)) {
            None
        } else {
            Some((row, field))
        };
        cx.notify();
    }

    pub(super) fn select_fk_combo(
        &mut self,
        row: usize,
        field: FkComboField,
        value: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(foreign_key) = self.schema.foreign_keys.get_mut(row) {
            match field {
                FkComboField::OnDelete => foreign_key.on_delete = value.to_string(),
                FkComboField::OnUpdate => foreign_key.on_update = value.to_string(),
            }
        }
        self.fk_combo = None;
        self.mark_dirty(cx);
    }

    /// Open the child-columns picker for one foreign key.
    pub(super) fn open_fk_fields(
        &mut self,
        row: usize,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.schema.foreign_keys.get(row).is_none() {
            return;
        }
        self.fk_edit = None;
        self.fk_edit_blur = None;
        self.fk_combo = None;
        self.fk_ref_open = false;
        self.select_fk(row, cx);
        self.fk_fields_open = true;
        cx.notify();
    }

    /// Open the referenced-columns picker, loading the referenced table's columns first.
    pub(super) fn open_fk_ref(
        &mut self,
        row: usize,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(foreign_key) = self.schema.foreign_keys.get(row) else {
            return;
        };
        let table = foreign_key.referenced_table.clone();
        self.fk_edit = None;
        self.fk_edit_blur = None;
        self.fk_combo = None;
        self.fk_fields_open = false;
        self.select_fk(row, cx);
        self.fk_ref_open = true;
        self.fk_ref_columns.clear();
        if table.is_empty() {
            cx.notify();
            return;
        }

        let connection = self.connection.clone();
        let database = self.database.clone();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.columns(&database, &table).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(columns) => {
                        view.fk_ref_columns =
                            columns.into_iter().map(|column| column.name).collect();
                    }
                    Err(error) => view.report_error(error.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn toggle_fk_column(
        &mut self,
        row: usize,
        column: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(foreign_key) = self.schema.foreign_keys.get_mut(row) {
            toggle_list(&mut foreign_key.columns, column);
        }
        self.mark_dirty(cx);
    }

    pub(super) fn toggle_fk_ref_column(
        &mut self,
        row: usize,
        column: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(foreign_key) = self.schema.foreign_keys.get_mut(row) {
            toggle_list(&mut foreign_key.referenced_columns, column);
        }
        self.mark_dirty(cx);
    }

    /// Ask the app to confirm deleting the current selection in one designer grid before it runs.
    pub(super) fn request_delete(&mut self, kind: DesignDeleteKind, cx: &mut Context<'_, Self>) {
        let deletable = match kind {
            DesignDeleteKind::Fields => {
                !self.field_rows.rows.is_empty() || self.selected_field.is_some()
            }
            DesignDeleteKind::Indexes => {
                let mut targets: BTreeSet<usize> = self.index_rows.rows.clone();
                if let Some(active) = self.selected_index {
                    targets.insert(active);
                }
                targets.iter().any(|row| {
                    self.schema
                        .indexes
                        .get(*row)
                        .is_some_and(|index| !index.primary)
                })
            }
            DesignDeleteKind::ForeignKeys => {
                !self.fk_rows.rows.is_empty() || self.selected_fk.is_some()
            }
        };
        if !deletable {
            return;
        }
        let design_id = self.id;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.delete_confirm = Some(DeleteConfirm::DesignRows { design_id, kind });
                cx.notify();
            });
        }
    }

    /// Run the confirmed delete for one of the designer grids.
    pub(super) fn delete_selected(&mut self, kind: DesignDeleteKind, cx: &mut Context<'_, Self>) {
        match kind {
            DesignDeleteKind::Fields => self.delete_field(cx),
            DesignDeleteKind::Indexes => self.delete_index(cx),
            DesignDeleteKind::ForeignKeys => self.delete_foreign_key(cx),
        }
    }

    /// Create a field from the fields grid's blank placeholder row, then edit its name.
    pub(super) fn begin_empty_field(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let row = self.schema.columns.len();
        self.insert_field_at(row, cx);
        self.begin_edit(row, FieldColumn::Name, window, cx);
    }

    /// Create an index from the index grid's blank placeholder row, then edit its name.
    pub(super) fn begin_empty_index(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let row = self.schema.indexes.len();
        self.add_index(cx);
        self.begin_index_edit(row, IndexColumn::Name, window, cx);
    }

    /// Create a foreign key from its grid's blank placeholder row, then edit its name.
    pub(super) fn begin_empty_foreign_key(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let row = self.schema.foreign_keys.len();
        self.add_foreign_key(cx);
        self.begin_fk_edit(row, FkColumn::Name, window, cx);
    }

    pub(super) fn preview_sql(&self) -> String {
        // A brand-new table has no name until the first save, so there is nothing to preview yet.
        if self.is_new && self.table.is_empty() {
            return String::new();
        }
        self.connection.table_schema_sql(
            &self.database,
            &self.table,
            self.original.as_ref(),
            &self.schema,
        )
    }

    /// Save. For an existing table this builds an `ALTER`; for a brand-new table it first asks the
    /// app for a name (the actual create runs in [`TableDesignView::save_new`]).
    pub(super) fn save(&mut self, cx: &mut Context<'_, Self>) {
        if self.saving {
            return;
        }
        if self.is_new {
            if let Some(app) = self.app.upgrade() {
                let design_id = self.id;
                let initial = self.table.clone();
                app.update(cx, |app, cx| {
                    app.open_create_table_dialog(design_id, initial, cx)
                });
            }
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
            self.dirty = false;
            cx.notify();
            return;
        }

        self.saving = true;
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
                    Err(error) => view.report_error(error.to_string(), cx),
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

    /// Create a brand-new table under `name` (chosen in the app's name prompt), then reload the
    /// designer as an ordinary existing table.
    pub(super) fn save_new(&mut self, name: String, cx: &mut Context<'_, Self>) {
        if self.saving {
            return;
        }
        self.table = name;
        self.saving = true;
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
                    async move {
                        connection
                            .save_table_schema(&database, &table, None, &schema)
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
                    Err(error) => view.report_error(error.to_string(), cx),
                }
                cx.notify();
            });

            if succeeded && let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| {
                    app.reload_database_tables(&connection, &database, cx);
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
            &self.auto_increment_input,
            &self.comment_input,
        ] {
            input.update(cx, |input, cx| input.set_theme(theme, cx));
        }
        for combo in [
            &self.engine_combo,
            &self.charset_combo,
            &self.collation_combo,
        ] {
            combo.update(cx, |combo, cx| combo.set_theme(theme, cx));
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
                false,
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
        self.privilege_manager_active = false;
        entity.update(cx, |design, cx| {
            design.load_schema(cx);
            design.load_option_catalogs(cx);
        });
        cx.notify();
    }

    /// Open a blank designer for a brand-new table in `database`. The table is not created until
    /// the user saves and supplies a name.
    pub(super) fn open_new_table(
        &mut self,
        connection_index: usize,
        database: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
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
                String::new(),
                false,
                true,
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
        self.privilege_manager_active = false;
        // Start with one field so the grid is not empty.
        entity.update(cx, |design, cx| {
            design.add_field(cx);
            design.load_option_catalogs(cx);
        });
        cx.notify();
    }

    /// Re-fetch a database's table list after an object was created there, locating it by the live
    /// connection rather than an index the designer does not carry.
    pub(super) fn reload_database_tables(
        &mut self,
        connection: &Arc<dyn Connection>,
        database: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection_index) = self.connections.iter().position(|node| {
            matches!(
                &node.status,
                ConnectionStatus::Connected(active) if Arc::ptr_eq(active, connection)
            )
        }) else {
            return;
        };
        let database_index =
            self.connections
                .get(connection_index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => databases
                        .iter()
                        .position(|existing| existing.name == database),
                    _ => None,
                });
        if let Some(database_index) = database_index {
            self.reload_tables(connection_index, database_index, cx);
        }
    }

    pub(super) fn activate_design(&mut self, index: Option<usize>, cx: &mut Context<'_, Self>) {
        self.active_design = index;
        self.active_query = None;
        self.active_grid = None;
        self.privilege_manager_active = false;
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
