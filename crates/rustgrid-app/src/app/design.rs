use super::*;

use std::collections::BTreeSet;

use gpui_kit::component::Size;
use rustgrid_core::{
    CheckDef, ColumnDef, DesignerColumnOption, DesignerFieldColumn, DesignerIndexColumn,
    DesignerOptionField, DesignerTab, ExcludeDef, ForeignKeyDef, IndexColumnDef, IndexDef, RuleDef,
    TableDesignerSpec, TableSchema, TriggerDef, UniqueKeyDef,
};

/// A row of the field grid, shared by the header and every row.
pub(super) const DESIGN_ROW_HEIGHT: f32 = 24.0;
pub(super) const DESIGN_HEADER_HEIGHT: f32 = 24.0;
/// The row-header strip of every designer grid: the row marker's column, as narrow as the marker
/// needs (the same 16px the table grid's gutter uses, `GRID_GUTTER_WIDTH`).
pub(super) const DESIGN_GUTTER_WIDTH: f32 = 16.0;
/// The x every designer label starts on: the sub-tab strip's first tab (字段) and the grid's first
/// column — its header (列名), its values and the in-place editor a cell swaps in.
///
/// The tab reaches it through the strip's own `px_2` padding, a tab's 1px left border and its
/// `px_4` padding (8 + 1 + 16, see `render_subtabs`); the grid through the row-header gutter plus
/// [`DESIGN_CELL_INSET`]. Both sides derive from this one number so they cannot drift apart.
pub(super) const DESIGN_LABEL_INSET: f32 = 25.0;
/// The left inset of a designer grid's cells *and* their header labels: the room
/// [`DESIGN_LABEL_INSET`] leaves after the row-header gutter. A column's header, its value and the
/// in-place editor that replaces the value all start on this inset — one constant, so a cell's
/// text cannot drift away from its header.
pub(super) const DESIGN_CELL_INSET: f32 = DESIGN_LABEL_INSET - DESIGN_GUTTER_WIDTH;
pub(super) const FIELD_NAME_WIDTH: f32 = 246.0;
pub(super) const FIELD_TYPE_WIDTH: f32 = 178.0;
pub(super) const FIELD_LENGTH_WIDTH: f32 = 90.0;
pub(super) const FIELD_DECIMALS_WIDTH: f32 = 90.0;
pub(super) const FIELD_NOTNULL_WIDTH: f32 = 70.0;
pub(super) const FIELD_VIRTUAL_WIDTH: f32 = 65.0;
pub(super) const FIELD_KEY_WIDTH: f32 = 100.0;
pub(super) const FIELD_COMMENT_WIDTH: f32 = 200.0;
pub(super) const FIELD_COLLATION_WIDTH: f32 = 150.0;
pub(super) const FIELD_SPARSE_WIDTH: f32 = 60.0;
pub(super) const DESIGN_DETAIL_HEIGHT: f32 = 152.0;

// The unique-key grid's columns.
pub(super) const UQ_NAME_WIDTH: f32 = 240.0;
pub(super) const UQ_FIELDS_WIDTH: f32 = 420.0;

// The check-constraint grid's columns.
pub(super) const CHECK_NAME_WIDTH: f32 = 240.0;
pub(super) const CHECK_EXPRESSION_WIDTH: f32 = 520.0;

// The trigger grid's columns.
pub(super) const TRIGGER_NAME_WIDTH: f32 = 240.0;
pub(super) const TRIGGER_TIMING_WIDTH: f32 = 110.0;
pub(super) const TRIGGER_EVENT_WIDTH: f32 = 110.0;

// The PostgreSQL exclusion-constraint and rule grids' columns.
pub(super) const EXCLUDE_NAME_WIDTH: f32 = 220.0;
pub(super) const EXCLUDE_DEFINITION_WIDTH: f32 = 560.0;
pub(super) const EXCLUDE_COMMENT_WIDTH: f32 = 200.0;
pub(super) const RULE_NAME_WIDTH: f32 = 220.0;
pub(super) const RULE_DEFINITION_WIDTH: f32 = 760.0;

// The index grid's columns.
pub(super) const INDEX_NAME_WIDTH: f32 = 200.0;
pub(super) const INDEX_FIELDS_WIDTH: f32 = 340.0;
pub(super) const INDEX_KIND_WIDTH: f32 = 130.0;
pub(super) const INDEX_METHOD_WIDTH: f32 = 120.0;
pub(super) const INDEX_COMMENT_WIDTH: f32 = 240.0;

// The index field editor's per-row controls.
pub(super) const INDEX_FIELD_NAME_WIDTH: f32 = 200.0;
pub(super) const INDEX_FIELD_OPTION_WIDTH: f32 = 80.0;
/// What a row leads its name with, after the panel's own padding: the drag handle (14), the check
/// box (14) and the `gap_2` spacing around them. The heading row offsets its 排序 / 子部分 labels by the
/// same amount, so they line up with the rows' cells.
pub(super) const INDEX_FIELD_LEAD_WIDTH: f32 = 44.0;
/// The room the heading row's `2/8` count needs at the panel's right edge.
pub(super) const INDEX_FIELDS_COUNT_WIDTH: f32 = 44.0;

// The index field editor's panel metrics. Its list is as tall as the window allows (up to
// `INDEX_FIELDS_LIST_HEIGHT`) instead of a fixed 240px that cut the footer off on a short window;
// `INDEX_FIELDS_CHROME_HEIGHT` is everything around the list — the heading row, the search box and
// the footer.
pub(super) const INDEX_FIELDS_LIST_HEIGHT: f32 = 420.0;
pub(super) const INDEX_FIELDS_LIST_MIN_HEIGHT: f32 = 120.0;
pub(super) const INDEX_FIELDS_CHROME_HEIGHT: f32 = 100.0;

// The foreign-key grid's columns.
pub(super) const FK_NAME_WIDTH: f32 = 180.0;
pub(super) const FK_COLUMNS_WIDTH: f32 = 200.0;
pub(super) const FK_SCHEMA_WIDTH: f32 = 130.0;
pub(super) const FK_REF_TABLE_WIDTH: f32 = 180.0;
pub(super) const FK_REF_COLUMNS_WIDTH: f32 = 200.0;
pub(super) const FK_DELETE_WIDTH: f32 = 120.0;
pub(super) const FK_UPDATE_WIDTH: f32 = 120.0;

// The sub-tabs of the table designer are [`rustgrid_core::DesignerTab`]: which of them an engine
// shows comes from its [`TableDesignerSpec`], so nothing here matches on an engine id.

/// The editable text columns of the field grid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FieldColumn {
    Name,
    Length,
    Decimals,
    Comment,
}

/// A cell of the field grid the caret can rest in, in the order Tab walks them: 列名, 类型, then the
/// engine's own text cells. The toggles (不是 null / 主键 / 虚拟 / 稀疏) are click-only, so the chain
/// skips them rather than stranding the caret on something that cannot take it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FieldCell {
    Name,
    Type,
    Text(FieldColumn),
}

/// The height of one row of the type drop-down, so its highlight can be scrolled into view.
pub(super) const TYPE_OPTION_HEIGHT: f32 = 20.0;

/// The chain entry a text cell occupies: 列名 is its own entry, the engine's other text cells wrap
/// their [`FieldColumn`].
pub(super) fn field_cell_of(column: FieldColumn) -> FieldCell {
    match column {
        FieldColumn::Name => FieldCell::Name,
        other => FieldCell::Text(other),
    }
}

/// The cell Tab (or Shift+Tab) moves to from `cell` of `row` in a grid of `rows` rows: the next cell
/// of `chain`, wrapped onto the neighbouring row at either end. `None` means the keystroke leaves the
/// grid (the window's own focus chain takes over).
pub(super) fn field_cell_neighbor(
    chain: &[FieldCell],
    row: usize,
    rows: usize,
    cell: FieldCell,
    shift: bool,
) -> Option<(usize, FieldCell)> {
    let position = chain.iter().position(|entry| *entry == cell)?;
    if rows == 0 || row >= rows || chain.is_empty() {
        return None;
    }
    if shift {
        match position.checked_sub(1) {
            Some(previous) => Some((row, chain[previous])),
            None if row > 0 => Some((row - 1, chain[chain.len() - 1])),
            None => None,
        }
    } else {
        match chain.get(position + 1) {
            Some(next) => Some((row, *next)),
            None if row + 1 < rows => Some((row + 1, chain[0])),
            None => None,
        }
    }
}

/// The type list's next highlight row: one step down (`down`) or up, wrapping at both ends. `None`
/// when the list is empty; `current` is clamped so a search that narrowed the list cannot strand it.
pub(super) fn next_type_highlight(
    count: usize,
    current: Option<usize>,
    down: bool,
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let current = current.unwrap_or(0).min(count - 1);
    Some(if down {
        if current + 1 >= count { 0 } else { current + 1 }
    } else if current == 0 {
        count - 1
    } else {
        current - 1
    })
}

/// The width of one engine-declared field-list column. The variants that describe the same cell
/// under different engines share their width.
pub(super) fn field_column_width(kind: DesignerFieldColumn) -> f32 {
    match kind {
        DesignerFieldColumn::Length | DesignerFieldColumn::Size => FIELD_LENGTH_WIDTH,
        DesignerFieldColumn::Decimals | DesignerFieldColumn::Scale => FIELD_DECIMALS_WIDTH,
        DesignerFieldColumn::NotNull => FIELD_NOTNULL_WIDTH,
        DesignerFieldColumn::Generated | DesignerFieldColumn::Computed => FIELD_VIRTUAL_WIDTH,
        DesignerFieldColumn::Key => FIELD_KEY_WIDTH,
        DesignerFieldColumn::Comment => FIELD_COMMENT_WIDTH,
        DesignerFieldColumn::Collation => FIELD_COLLATION_WIDTH,
        DesignerFieldColumn::Sparse => FIELD_SPARSE_WIDTH,
    }
}

/// A stable id fragment for one field-list column, used to keep every cell's element id unique.
pub(super) fn field_column_id(kind: DesignerFieldColumn) -> &'static str {
    match kind {
        DesignerFieldColumn::Length | DesignerFieldColumn::Size => "size",
        DesignerFieldColumn::Decimals | DesignerFieldColumn::Scale => "scale",
        DesignerFieldColumn::NotNull => "notnull",
        DesignerFieldColumn::Generated | DesignerFieldColumn::Computed => "computed",
        DesignerFieldColumn::Key => "key",
        DesignerFieldColumn::Comment => "comment",
        DesignerFieldColumn::Collation => "collation",
        DesignerFieldColumn::Sparse => "sparse",
    }
}

/// The text cell a [`DesignerFieldColumn`] edits, or `None` when the cell is a toggle or a
/// read-only display.
pub(super) fn field_column_text(kind: DesignerFieldColumn) -> Option<FieldColumn> {
    match kind {
        DesignerFieldColumn::Length | DesignerFieldColumn::Size => Some(FieldColumn::Length),
        DesignerFieldColumn::Decimals | DesignerFieldColumn::Scale => Some(FieldColumn::Decimals),
        DesignerFieldColumn::Comment => Some(FieldColumn::Comment),
        _ => None,
    }
}

/// The text one editable [`FieldColumn`] cell shows for a column definition.
pub(super) fn field_column_value(column: &ColumnDef, text: FieldColumn) -> String {
    match text {
        FieldColumn::Name => column.name.clone(),
        FieldColumn::Length => column.length.clone(),
        FieldColumn::Decimals => column.decimals.clone(),
        FieldColumn::Comment => column.comment.clone(),
    }
}

/// The value a read-only [`DesignerFieldColumn`] shows for one column.
pub(super) fn field_column_display(kind: DesignerFieldColumn, column: &ColumnDef) -> String {
    match kind {
        DesignerFieldColumn::Collation => column.collation.clone(),
        _ => String::new(),
    }
}

/// The auxiliary inputs owned by a designer, identified by their callback tag.
#[derive(Clone, PartialEq, Eq)]
enum InputTag {
    Default,
    Engine,
    Charset,
    Collation,
    /// The selected column's own collation (the detail panel's 排序规则 control).
    ColumnCollation,
    AutoIncrement,
    /// The selected identity column's generation mode (`ALWAYS` / `BY DEFAULT`).
    IdentityGeneration,
    IdentityStart,
    IdentityIncrement,
    Comment,
    /// The index field editor's search box.
    IndexFieldSearch,
    /// One field's 子部分 (index prefix length) in the index field editor, by field name — keyed by
    /// name rather than by row so a re-order neither drops the editor nor its text.
    IndexPrefix(String),
}

/// A live in-place editor over one cell of the field grid.
pub(super) struct FieldEdit {
    pub(super) row: usize,
    pub(super) column: FieldColumn,
    pub(super) input: Entity<TextInput>,
}

/// The editable text columns of the unique-key grid. Its field list is picked through the same
/// "..." popup the index grid uses.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UniqueKeyColumn {
    Name,
}

/// A live in-place editor over one text cell of the unique-key grid.
pub(super) struct UniqueKeyEdit {
    pub(super) row: usize,
    pub(super) column: UniqueKeyColumn,
    pub(super) input: Entity<TextInput>,
}

/// The editable text columns of the check-constraint grid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CheckColumn {
    Name,
    Expression,
}

/// A live in-place editor over one text cell of the check-constraint grid.
pub(super) struct CheckEdit {
    pub(super) row: usize,
    pub(super) column: CheckColumn,
    pub(super) input: Entity<TextInput>,
}

/// The editable text columns of the trigger grid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TriggerColumn {
    Name,
    Timing,
    Event,
}

/// A live in-place editor over one text cell of the trigger grid.
pub(super) struct TriggerEdit {
    pub(super) row: usize,
    pub(super) column: TriggerColumn,
    pub(super) input: Entity<TextInput>,
}

/// The editable text columns of the PostgreSQL exclusion-constraint grid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ExcludeColumn {
    Name,
    Definition,
    Comment,
}

/// A live in-place editor over one text cell of the exclusion-constraint grid.
pub(super) struct ExcludeEdit {
    pub(super) row: usize,
    pub(super) column: ExcludeColumn,
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

/// The destination indices of a whole-group field move: every selected row swaps with its
/// neighbour in `delta`'s direction, preserving the group's internal order. `None` when the group
/// is already against the edge in that direction (the move is refused as a whole, never partially).
fn moved_field_rows(rows: &[usize], total: usize, delta: isize) -> Option<Vec<usize>> {
    let delta = delta.signum();
    if rows.is_empty() || total == 0 || delta == 0 {
        return None;
    }
    if (delta < 0 && rows[0] == 0) || (delta > 0 && rows[rows.len() - 1] + 1 >= total) {
        return None;
    }
    Some(if delta < 0 {
        rows.iter().map(|row| row - 1).collect()
    } else {
        rows.iter().map(|row| row + 1).collect()
    })
}

/// Swap the selected field rows one slot in `delta`'s direction. Callers check `moved_field_rows`
/// first, so every selected row has a neighbour to swap with.
fn move_columns(columns: &mut [ColumnDef], rows: &[usize], delta: isize) {
    if delta < 0 {
        for row in rows {
            columns.swap(*row, *row - 1);
        }
    } else {
        for row in rows.iter().rev() {
            columns.swap(*row, *row + 1);
        }
    }
}

/// One row of the index field editor: a column of the table, whether it is a member of the index
/// being edited, and — for a member — its own per-column options.
#[derive(Clone)]
pub(super) struct IndexFieldRow {
    pub(super) name: String,
    /// Whether the column is one of the index's fields (the editor's check box).
    pub(super) member: bool,
    pub(super) descending: bool,
    pub(super) prefix_length: String,
}

/// The index field editor's open draft.
///
/// The editor is the grid's 字段筛选 popup applied to an index: one row per *column of the table*
/// with a check box, a search box above them, and drag handles to order them. The ticked rows, in
/// list order, are the index's fields.
///
/// Every edit (the ticks, 排序, 子部分, the order) lands here and only 确定 installs it; 取消 or a click
/// outside discards it.
#[derive(Clone)]
pub(super) struct IndexFieldsDraft {
    /// The index row being edited.
    pub(super) row: usize,
    /// Every column of the table, in the order the editor lists them (the index's own field order
    /// for its fields first, then the table's remaining columns, re-ordered by dragging a handle).
    pub(super) fields: Vec<IndexFieldRow>,
}

impl IndexFieldsDraft {
    /// The ticked fields, in list order: what 确定 installs.
    pub(super) fn members(&self) -> Vec<IndexColumnDef> {
        self.fields
            .iter()
            .filter(|field| field.member)
            .map(|field| IndexColumnDef {
                name: field.name.clone(),
                descending: field.descending,
                prefix_length: field.prefix_length.clone(),
            })
            .collect()
    }
}

/// The field row being dragged by its handle, as the grid's 字段筛选 popup tracks it: the row it
/// started on, where it is now, and where the press began — a re-order needs no more than the
/// pointer's travel over fixed-height rows.
#[derive(Clone, Copy)]
pub(super) struct IndexFieldDrag {
    pub(super) from: usize,
    pub(super) current: usize,
    pub(super) start_y: f32,
}

/// The text editor columns of the index grid.
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

/// The index grid's columns after the gutter: 名称 and 字段 always, then whatever the engine
/// declares — its own 索引类型, the 索引方法 cell only for an engine whose kinds carry more than one
/// type (MySQL/MariaDB's `BTREE`/`HASH`), and 注释 only for an engine whose indexes carry one.
pub(super) fn index_grid_columns(designer: &TableDesignerSpec) -> Vec<(&'static str, f32)> {
    let mut columns = vec![
        ("design.index.name", INDEX_NAME_WIDTH),
        ("design.index.fields", INDEX_FIELDS_WIDTH),
        ("design.index.type", index_kind_width(designer)),
    ];
    if designer.index_methods().is_some() {
        columns.push(("design.index.method", INDEX_METHOD_WIDTH));
    }
    if designer.index_comment {
        columns.push(("design.index.comment", INDEX_COMMENT_WIDTH));
    }
    columns
}

/// The 索引类型 column's width: wide enough for the engine's longest kind label — SQL Server's
/// `UNIQUE NONCLUSTERED` does not fit MySQL's 130px — and never narrower than the default. The cell
/// and its drop-down share the width, so a long label is clipped in neither.
pub(super) fn index_kind_width(designer: &TableDesignerSpec) -> f32 {
    let longest = designer
        .index_kinds
        .iter()
        .map(|kind| ui::approx_text_width(kind.label))
        .fold(0.0, f32::max);
    // The cell's own cell padding and trailing chevron, plus the drop-down row's check mark.
    INDEX_KIND_WIDTH.max(longest + 34.0)
}

/// The 索引类型 cell's text for one index row: the engine's declared kind (`NORMAL`, `UNIQUE`,
/// `FULLTEXT`, `SPATIAL`, `NONCLUSTERED`, ...), the primary key's own marker, or — for a type the
/// engine did not declare (Oracle's `FUNCTION-BASED NORMAL`, SQL Server's columnstore, ...) — the
/// engine's own `index_type` text, so nothing is ever mislabelled as another engine's.
pub(super) fn index_kind_label(designer: &TableDesignerSpec, index: &IndexDef) -> String {
    if index.primary {
        return "PRIMARY".to_string();
    }
    match designer.index_kind(index) {
        Some(kind) => kind.label.to_string(),
        None => index.index_type.trim().to_string(),
    }
}

/// The 索引方法 cell's text for one index row: the kind's own type list when it has more than one
/// entry, with the index's stored type as the current one. `None` for a row the cell does not
/// apply to (the primary key, the kinds with a single type).
pub(super) fn index_method_label(
    designer: &TableDesignerSpec,
    index: &IndexDef,
) -> Option<&'static str> {
    if index.primary {
        return None;
    }
    designer.index_method(index)
}

/// A brand-new index row starts as the engine's first kind, so its type is that kind's own value
/// (`BTREE` on MySQL/MariaDB, `NONCLUSTERED` on SQL Server, nothing where the engine has no type)
/// rather than a fixed MySQL value on every engine.
pub(super) fn default_index_kind(designer: &TableDesignerSpec) -> (bool, String) {
    designer
        .index_kinds
        .first()
        .map(|kind| (kind.unique, kind.value().to_string()))
        .unwrap_or((false, String::new()))
}

/// Move the primary key to the end of a schema's index list, where the designer keeps it.
///
/// The 索引 grid lists the table's *secondary* indexes: Navicat hides the primary key there, and its
/// name and kind are the key's own (not editable) while its columns are the 字段 tab's 主键 flags. The
/// model still carries the entry — the DDL diff needs its name — with the key pushed last, which is
/// what makes a row of the grid a row of the model (see `TableDesignView::index_row_count`).
pub(super) fn push_primary_index_last(schema: &mut TableSchema) {
    if let Some(position) = schema.indexes.iter().position(|index| index.primary) {
        let primary = schema.indexes.remove(position);
        schema.indexes.push(primary);
    }
}

/// The 字段 column's width in the index field editor: the table's longest column name plus a little
/// slack, so a long field (`CUSTOMER_GRP_SCH_DETAIL_ID`) is drawn in full instead of being wrapped
/// or clipped.
pub(super) fn index_fields_name_width(columns: &[ColumnDef]) -> f32 {
    let longest = columns
        .iter()
        .map(|column| ui::approx_text_width(&column.name))
        .fold(0.0, f32::max);
    INDEX_FIELD_NAME_WIDTH.max(longest + 8.0)
}

/// The width of the whole editor: the 字段 column, the drag handle and check box the rows lead with,
/// the engine's per-column option cells (MySQL's 排序 + 子部分), the heading row's count and the
/// panel's own padding.
pub(super) fn index_fields_panel_width(designer: &TableDesignerSpec, columns: &[ColumnDef]) -> f32 {
    let options = designer.index_columns.len() as f32;
    index_fields_name_width(columns)
        + INDEX_FIELD_LEAD_WIDTH
        + options * (INDEX_FIELD_OPTION_WIDTH + 8.0)
        + INDEX_FIELDS_COUNT_WIDTH
        + 24.0
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
    ReferencedSchema,
    ReferencedTable,
}

/// Which dropdown of a foreign-key row a popup is open for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FkComboField {
    OnDelete,
    OnUpdate,
}

/// Per-(row, dropdown) prepaint anchors for the foreign-key grid's dropdown cells.
pub(super) type FkComboAnchors = Rc<RefCell<BTreeMap<(usize, FkComboField), Point<Pixels>>>>;

/// Each row's field drop-down anchor (the bottom-left of its field cell) in window coordinates,
/// keyed by row. A single shared anchor cannot work here: every visible row prepaints and would
/// overwrite it, so the list would open under whichever row happened to paint last.
pub(super) type DesignPickerAnchors = Rc<RefCell<BTreeMap<usize, Point<Pixels>>>>;

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
    /// How this engine's designer is shaped: which sub-tabs, per-column controls, field-list
    /// columns and table options it shows. Read from the connection's driver descriptor, so no
    /// view code matches on an engine id.
    pub(super) designer: TableDesignerSpec,

    pub(super) schema: TableSchema,
    pub(super) original: Option<TableSchema>,
    pub(super) tab: DesignerTab,
    pub(super) selected_field: Option<usize>,
    /// The fields grid's active cell (the caret's cell), used for the detail panel and keyboard
    /// navigation. Painting comes from `field_selection`, exactly like the table grid.
    pub(super) selected_cell: Option<(usize, FieldCell)>,
    /// The fields grid's Excel-like selection: rectangular ranges of `(row, column)` cells, the
    /// same model the table grid uses (click selects, drag/shift extend, Ctrl toggles, Ctrl+A all).
    pub(super) field_selection: Option<CellSelection>,
    /// A press is driving a fields-grid range extension.
    pub(super) field_selecting: bool,
    /// The cell the current press landed on (a click opens its editor on mouse-up when it was a
    /// re-click or a double-click, like the table grid).
    field_cell_press: Option<(usize, usize)>,
    /// The press moved to another cell, so mouse-up must not open an editor.
    field_cell_dragged: bool,
    /// The pressed cell was already selected: a second click edits it.
    field_cell_reselect: bool,
    pub(super) selected_index: Option<usize>,
    /// Multi-row selections from the designer's other grids' left gutters (index, foreign keys,
    /// ...). The fields grid keeps its cell selection in `field_selection`.
    pub(super) index_rows: RowSelection,
    pub(super) fk_rows: RowSelection,
    pub(super) index_edit: Option<IndexEdit>,
    index_edit_blur: Option<Subscription>,
    /// Which index row's kind/method dropdown is open.
    pub(super) index_combo: Option<(usize, IndexComboField)>,
    pub(super) index_combo_anchor: IndexComboAnchors,
    /// The 字段 editor of the selected index, as a draft (see [`IndexFieldsDraft`]).
    pub(super) index_fields_draft: Option<IndexFieldsDraft>,
    pub(super) index_fields_anchor: DesignPickerAnchors,
    /// The editor's search text, which narrows the field list.
    pub(super) index_field_query: String,
    /// The 子部分 inputs of the index field editor, keyed by field name.
    pub(super) index_field_prefix_inputs: BTreeMap<String, Entity<TextInput>>,
    pub(super) index_field_drag: Option<IndexFieldDrag>,
    /// The editor's search field, kept across opens so its inner state survives.
    pub(super) index_field_search_input: Entity<TextInput>,
    /// The unique-key grid: its own selection, in-place name editor and field picker. An engine
    /// only shows the tab when its [`TableDesignerSpec`] declares it.
    pub(super) selected_unique_key: Option<usize>,
    pub(super) unique_key_rows: RowSelection,
    pub(super) unique_key_edit: Option<UniqueKeyEdit>,
    unique_key_edit_blur: Option<Subscription>,
    /// The unique-key row whose "choose fields" popup is open.
    pub(super) unique_key_fields_row: Option<usize>,
    pub(super) unique_key_fields_anchor: DesignPickerAnchors,
    /// The check-constraint grid: selection and in-place name/expression editor.
    pub(super) selected_check: Option<usize>,
    pub(super) check_rows: RowSelection,
    pub(super) check_edit: Option<CheckEdit>,
    check_edit_blur: Option<Subscription>,
    /// PostgreSQL exclusion constraints: selection and in-place name/definition/comment editors.
    pub(super) selected_exclude: Option<usize>,
    pub(super) exclude_rows: RowSelection,
    pub(super) exclude_edit: Option<ExcludeEdit>,
    exclude_edit_blur: Option<Subscription>,
    /// PostgreSQL rewrite rules: selection and the shared definition editor of the selected row.
    pub(super) selected_rule: Option<usize>,
    pub(super) rule_rows: RowSelection,
    pub(super) rule_editor: Option<Entity<ui::SqlEditor>>,
    pub(super) rule_editor_row: Option<usize>,
    /// The trigger grid and its body editor. The body editor is one shared SQL editor that shows
    /// the selected trigger; `trigger_editor_row` remembers which trigger it currently holds, so a
    /// render does not reset the caret while the user types.
    pub(super) selected_trigger: Option<usize>,
    pub(super) trigger_rows: RowSelection,
    pub(super) trigger_edit: Option<TriggerEdit>,
    trigger_edit_blur: Option<Subscription>,
    pub(super) trigger_editor: Option<Entity<ui::SqlEditor>>,
    pub(super) trigger_editor_row: Option<usize>,
    /// The MySQL/MariaDB partition clause editor. Recreated after a schema reload.
    pub(super) partition_editor: Option<Entity<ui::SqlEditor>>,
    pub(super) selected_fk: Option<usize>,
    pub(super) fk_edit: Option<FkEdit>,
    fk_edit_blur: Option<Subscription>,
    /// Which foreign-key row's on-delete/on-update dropdown is open.
    pub(super) fk_combo: Option<(usize, FkComboField)>,
    pub(super) fk_combo_anchor: FkComboAnchors,
    /// The child-columns and referenced-columns pickers for the selected foreign key.
    pub(super) fk_fields_open: bool,
    pub(super) fk_fields_anchor: DesignPickerAnchors,
    pub(super) fk_ref_open: bool,
    pub(super) fk_ref_anchor: DesignPickerAnchors,
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
    /// Which row of the filtered type list the keyboard highlight sits on (Up/Down, Enter picks it).
    /// Reset when the drop-down opens or its search text changes.
    pub(super) type_highlight: Option<usize>,
    /// The type list's own scroll offset, so the highlight can be kept in view.
    pub(super) type_scroll: ScrollHandle,
    /// Each row's type-cell bottom-left in window coordinates, keyed by field index. A shared
    /// single anchor cannot work here because every visible row prepaints and would overwrite it.
    pub(super) combo_anchor: Rc<RefCell<BTreeMap<usize, Point<Pixels>>>>,
    /// The design view's own window-space bounds, so the type dropdown can convert the
    /// (window-absolute) prepaint anchor into view-local coordinates — and so the index field
    /// editor can size itself to the room the view actually has.
    pub(super) root_bounds: Rc<RefCell<Bounds<Pixels>>>,
    pub(super) default_input: Entity<TextInput>,
    /// The list of common default expressions, opened from the chevron beside 默认.
    pub(super) default_menu_open: bool,
    /// The 默认 field's top-left in window coordinates, so its suggestions can open above it when
    /// the detail panel sits at the bottom of the designer.
    pub(super) default_anchor: Rc<RefCell<Option<Point<Pixels>>>>,
    /// The identity controls shown under 自动递增 (PostgreSQL).
    pub(super) identity_generation_combo: Entity<ComboBox>,
    pub(super) identity_start_input: Entity<TextInput>,
    pub(super) identity_increment_input: Entity<TextInput>,
    /// Pick-lists for the Options tab, fed by `engines`/`charsets`/`collations`.
    pub(super) engine_combo: Entity<ComboBox>,
    pub(super) charset_combo: Entity<ComboBox>,
    pub(super) collation_combo: Entity<ComboBox>,
    pub(super) engines: Vec<&'static str>,
    pub(super) charsets: Vec<String>,
    pub(super) collations: Vec<String>,
    pub(super) auto_increment_input: Entity<TextInput>,
    pub(super) comment_input: Entity<TextInput>,
    /// The detail panel's per-column 排序规则 picker (only shown by engines that declare it).
    pub(super) column_collation_combo: Entity<ComboBox>,

    pub(super) hscroll: ScrollHandle,
    pub(super) hscroll_grab: Option<f32>,
    pub(super) list_scroll: UniformListScrollHandle,
    pub(super) vscroll_grab: Option<f32>,
    /// The designer's own focus handle: it carries the `TableDesigner` key context (Ctrl+S), so
    /// something inside the designer must hold focus for those keys to fire.
    pub(super) focus: FocusHandle,
    /// Set when the designer is opened or brought to the front; the next render focuses `focus`.
    pub(super) focus_pending: bool,
    /// The 预览 tab's read-only, syntax-highlighted editor. Kept across renders so its scroll
    /// position and selection survive.
    pub(super) preview_editor: Option<Entity<ui::SqlEditor>>,
    pub(super) self_weak: WeakEntity<TableDesignView>,
    /// The shared field clipboard (see `AppView::design_clipboard`): 复制 stores the copied
    /// columns here and 粘贴 reads them back, so a field can move between two designers while
    /// keeping its exact `ColumnDef` rather than a lossy TSV round-trip.
    design_clipboard: Rc<RefCell<Vec<ColumnDef>>>,
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
                let _ = weak.update(cx, |view, cx| view.input_changed(tag.clone(), text, cx));
            },
        ))
    })
}

/// The index field editor's search box: the grid's 字段筛选 field, with the magnifier icon, the
/// placeholder and a clear button.
fn make_index_field_search_input(
    theme: Theme,
    weak: WeakEntity<TableDesignView>,
    cx: &mut Context<'_, TableDesignView>,
) -> Entity<TextInput> {
    let options = TextInputOptions {
        placeholder: t!("design.index.fields_search").to_string().into(),
        icon: Some("icons/search.svg"),
        clearable: true,
        text_size: Some(12.0),
        size: Some(Size::XSmall),
        ..Default::default()
    };
    let change = weak.clone();
    let cancel = weak;
    cx.new(move |cx| {
        TextInput::new(theme, "", options, cx)
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |view, cx| {
                    view.input_changed(InputTag::IndexFieldSearch, text, cx);
                });
            }))
            .on_cancel(Rc::new(move |_window, cx| {
                // Escape clears the search and closes the editor.
                let _ = cancel.update(cx, |view, cx| view.close_index_fields(cx));
            }))
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
                let _ = weak.update(cx, |view, cx| view.input_changed(tag.clone(), value, cx));
            }))
    })
}

/// A column name not already in `existing`, appending `_copy` / `_copy2` / … and recording the
/// chosen name so a later paste in the same batch does not repeat it.
fn unique_column_name(base: &str, existing: &mut Vec<String>) -> String {
    let taken = |candidate: &str| {
        existing
            .iter()
            .any(|name| name.eq_ignore_ascii_case(candidate))
    };
    if !taken(base) {
        existing.push(base.to_string());
        return base.to_string();
    }
    let mut candidate = format!("{base}_copy");
    let mut n = 2;
    while taken(&candidate) {
        candidate = format!("{base}_copy{n}");
        n += 1;
    }
    existing.push(candidate.clone());
    candidate
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
        designer: TableDesignerSpec,
        app: WeakEntity<AppView>,
        runtime: Arc<Runtime>,
        theme: Theme,
        design_clipboard: Rc<RefCell<Vec<ColumnDef>>>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let weak = cx.weak_entity();
        let column_types = connection.column_types();
        let engines = connection.storage_engines();
        let default_input = text_input(theme, String::new(), weak.clone(), InputTag::Default, cx);
        let identity_generation_combo =
            option_combo(theme, weak.clone(), InputTag::IdentityGeneration, cx);
        let identity_start_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::IdentityStart,
            cx,
        );
        let identity_increment_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::IdentityIncrement,
            cx,
        );
        let engine_combo = option_combo(theme, weak.clone(), InputTag::Engine, cx);
        let charset_combo = option_combo(theme, weak.clone(), InputTag::Charset, cx);
        let collation_combo = option_combo(theme, weak.clone(), InputTag::Collation, cx);
        let column_collation_combo =
            option_combo(theme, weak.clone(), InputTag::ColumnCollation, cx);
        let auto_increment_input = text_input(
            theme,
            String::new(),
            weak.clone(),
            InputTag::AutoIncrement,
            cx,
        );
        let comment_input = text_input(theme, String::new(), weak.clone(), InputTag::Comment, cx);
        let index_field_search_input = make_index_field_search_input(theme, weak.clone(), cx);

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
            designer,
            design_clipboard,
            schema: TableSchema::default(),
            original: None,
            tab: DesignerTab::Fields,
            selected_field: None,
            selected_cell: None,
            field_selection: None,
            field_selecting: false,
            field_cell_press: None,
            field_cell_dragged: false,
            field_cell_reselect: false,
            selected_index: None,
            index_rows: RowSelection::default(),
            fk_rows: RowSelection::default(),
            index_edit: None,
            index_edit_blur: None,
            index_combo: None,
            index_combo_anchor: Rc::new(RefCell::new(BTreeMap::new())),
            index_fields_draft: None,
            index_fields_anchor: Rc::new(RefCell::new(BTreeMap::new())),
            index_field_query: String::new(),
            index_field_prefix_inputs: BTreeMap::new(),
            index_field_drag: None,
            index_field_search_input,
            selected_unique_key: None,
            unique_key_rows: RowSelection::default(),
            unique_key_edit: None,
            unique_key_edit_blur: None,
            unique_key_fields_row: None,
            unique_key_fields_anchor: Rc::new(RefCell::new(BTreeMap::new())),
            selected_check: None,
            check_rows: RowSelection::default(),
            check_edit: None,
            check_edit_blur: None,
            selected_exclude: None,
            exclude_rows: RowSelection::default(),
            exclude_edit: None,
            exclude_edit_blur: None,
            selected_rule: None,
            rule_rows: RowSelection::default(),
            rule_editor: None,
            rule_editor_row: None,
            selected_trigger: None,
            trigger_rows: RowSelection::default(),
            trigger_edit: None,
            trigger_edit_blur: None,
            trigger_editor: None,
            trigger_editor_row: None,
            partition_editor: None,
            selected_fk: None,
            fk_edit: None,
            fk_edit_blur: None,
            fk_combo: None,
            fk_combo_anchor: Rc::new(RefCell::new(BTreeMap::new())),
            fk_fields_open: false,
            fk_fields_anchor: Rc::new(RefCell::new(BTreeMap::new())),
            fk_ref_open: false,
            fk_ref_anchor: Rc::new(RefCell::new(BTreeMap::new())),
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
            type_highlight: None,
            type_scroll: ScrollHandle::new(),
            combo_anchor: Rc::new(RefCell::new(BTreeMap::new())),
            root_bounds: Rc::new(RefCell::new(Bounds::default())),
            default_input,
            default_menu_open: false,
            default_anchor: Rc::new(RefCell::new(None)),
            identity_generation_combo,
            identity_start_input,
            identity_increment_input,
            engine_combo,
            charset_combo,
            collation_combo,
            column_collation_combo,
            engines,
            charsets: Vec::new(),
            collations: Vec::new(),
            auto_increment_input,
            comment_input,
            hscroll: ScrollHandle::new(),
            hscroll_grab: None,
            list_scroll: UniformListScrollHandle::new(),
            vscroll_grab: None,
            focus: cx.focus_handle(),
            // The designer opens as the active tab, so it takes focus on its first render.
            focus_pending: true,
            preview_editor: None,
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
                    Ok(mut schema) => {
                        // The 索引 grid does not list the primary key (see `push_primary_index_last`).
                        push_primary_index_last(&mut schema);
                        view.schema = schema.clone();
                        view.original = Some(schema);
                        view.is_new = false;
                        view.dirty = false;
                        view.partition_editor = None;
                        view.rule_editor_row = None;
                        view.trigger_editor_row = None;
                        view.selected_field = if view.schema.columns.is_empty() {
                            None
                        } else {
                            Some(0)
                        };
                        view.selected_cell = view.selected_field.map(|row| (row, FieldCell::Name));
                        view.field_selection =
                            view.selected_field.map(|row| CellSelection::new(row, 0));
                        view.field_selecting = false;
                        view.field_cell_press = None;
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
            InputTag::ColumnCollation => {
                if let Some(index) = self.selected_field
                    && let Some(column) = self.schema.columns.get_mut(index)
                {
                    column.collation = text.to_string();
                }
            }
            InputTag::AutoIncrement => self.schema.options.auto_increment = text.to_string(),
            InputTag::IdentityGeneration => {
                if let Some(index) = self.selected_field
                    && let Some(column) = self.schema.columns.get_mut(index)
                {
                    column.identity = text.to_string();
                }
            }
            InputTag::IdentityStart => {
                if let Some(index) = self.selected_field
                    && let Some(column) = self.schema.columns.get_mut(index)
                {
                    column.identity_start = text.to_string();
                }
            }
            InputTag::IdentityIncrement => {
                if let Some(index) = self.selected_field
                    && let Some(column) = self.schema.columns.get_mut(index)
                {
                    column.identity_increment = text.to_string();
                }
            }
            InputTag::Comment => self.schema.options.comment = text.to_string(),
            InputTag::IndexFieldSearch => {
                // The search only narrows the editor's list; it changes nothing in the draft.
                self.index_field_query = text.to_string();
                cx.notify();
                return;
            }
            InputTag::IndexPrefix(field) => {
                // The draft owns the value, so it does not go through `mark_dirty`: only 确定 does.
                self.set_index_draft_prefix(&field, text, cx);
                return;
            }
        }
        self.dirty = true;
        cx.notify();
    }

    /// The engine's common default expressions that apply to `column`'s type. The list is filtered
    /// per column, so a `varchar` column is not offered timestamp functions and a numeric column is
    /// not offered an empty string.
    pub(super) fn default_suggestions_for(
        &self,
        column: Option<&ColumnDef>,
    ) -> Vec<&rustgrid_core::DefaultSuggestion> {
        let data_type = column
            .map(|column| column.data_type.as_str())
            .unwrap_or_default();
        self.designer
            .default_suggestions
            .iter()
            .filter(|suggestion| suggestion.applies(data_type))
            .collect()
    }

    /// Take one of the engine's common default expressions from the 默认 suggestions.
    pub(super) fn select_default_suggestion(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.selected_field
            && let Some(column) = self.schema.columns.get_mut(index)
        {
            column.default = value.to_string();
        }
        self.default_input
            .update(cx, |input, cx| input.set_text(value.to_string(), cx));
        self.default_menu_open = false;
        self.dirty = true;
        cx.notify();
    }

    /// Push the selected field's default and identity controls into the detail panel.
    fn sync_selected_input(&mut self, cx: &mut Context<'_, Self>) {
        self.default_menu_open = false;
        let column = self
            .selected_field
            .and_then(|index| self.schema.columns.get(index));
        let default = column
            .map(|column| column.default.clone())
            .unwrap_or_default();
        self.default_input
            .update(cx, |input, cx| input.set_text(default, cx));
        let identity =
            column.filter(|column| column.auto_increment && !column.identity.trim().is_empty());
        let generation = identity
            .map(|column| column.identity.clone())
            .filter(|identity| !identity.trim().is_empty())
            .unwrap_or_else(|| "BY DEFAULT".to_string());
        let start = identity
            .map(|column| column.identity_start.clone())
            .unwrap_or_default();
        let increment = identity
            .map(|column| column.identity_increment.clone())
            .unwrap_or_default();
        let enabled = identity.is_some();
        self.identity_start_input
            .update(cx, |input, cx| input.set_text(start, cx));
        self.identity_increment_input
            .update(cx, |input, cx| input.set_text(increment, cx));
        self.identity_generation_combo.update(cx, |combo, cx| {
            combo.set_options(
                vec![
                    ComboOption::plain("BY DEFAULT"),
                    ComboOption::plain("ALWAYS"),
                ],
                cx,
            );
            combo.set_selected(generation, cx);
            combo.set_enabled(enabled, cx);
        });
        // The detail panel's per-column collation picker follows the selected field.
        if self
            .designer
            .column_options
            .contains(&DesignerColumnOption::Collation)
        {
            let collation = self
                .selected_field
                .and_then(|index| self.schema.columns.get(index))
                .map(|column| column.collation.clone())
                .unwrap_or_default();
            let enabled = self.selected_field.is_some();
            let options: Vec<ComboOption> = self
                .collations
                .iter()
                .cloned()
                .map(ComboOption::plain)
                .collect();
            self.column_collation_combo.update(cx, |combo, cx| {
                combo.set_options(options, cx);
                combo.set_selected(collation, cx);
                combo.set_enabled(enabled, cx);
            });
        }
    }

    fn sync_option_inputs(&mut self, cx: &mut Context<'_, Self>) {
        let options = self.schema.options.clone();
        self.auto_increment_input
            .update(cx, |input, cx| input.set_text(options.auto_increment, cx));
        self.comment_input
            .update(cx, |input, cx| input.set_text(options.comment, cx));
    }

    pub(super) fn set_partition_clause(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        if self.schema.partitioning != text {
            self.schema.partitioning = text.to_string();
            self.dirty = true;
            cx.notify();
        }
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
        field_column_value(column_def, column)
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

    /// The fields grid's column count: 列名, 类型 and the engine's own field columns.
    pub(super) fn field_column_count(&self) -> usize {
        2 + self.designer.field_columns.len()
    }

    /// The display column of an editable field cell.
    fn field_cell_column(&self, cell: FieldCell) -> usize {
        match cell {
            FieldCell::Name => 0,
            FieldCell::Type => 1,
            FieldCell::Text(column) => {
                2 + self
                    .designer
                    .field_columns
                    .iter()
                    .position(|kind| field_column_text(*kind) == Some(column))
                    .unwrap_or(0)
            }
        }
    }

    /// The editable cell at a display column; `None` for a toggle or read-only column.
    fn field_cell_at(&self, column: usize) -> Option<FieldCell> {
        match column {
            0 => Some(FieldCell::Name),
            1 => Some(FieldCell::Type),
            _ => self
                .designer
                .field_columns
                .get(column - 2)
                .and_then(|kind| field_column_text(*kind))
                .map(FieldCell::Text),
        }
    }

    /// The field rows the current cells cover — the rows 删除字段 / 上移 / 下移 act on. Falls back to
    /// the active row when nothing is selected (a freshly loaded table starts that way).
    pub(super) fn selected_field_rows(&self) -> Vec<usize> {
        let rows = match &self.field_selection {
            Some(selection) => selection.row_indices(),
            None => self.selected_field.into_iter().collect(),
        };
        rows.into_iter()
            .filter(|row| *row < self.schema.columns.len())
            .collect()
    }

    /// Point `selected_field` / `selected_cell` at the selection's active cell and load the detail
    /// panel for its row, exactly like the table grid's active cell.
    fn sync_field_active(&mut self, cx: &mut Context<'_, Self>) {
        let cursor = self
            .field_selection
            .as_ref()
            .map(|selection| selection.active_cursor());
        match cursor {
            Some((row, column)) => {
                self.selected_field = Some(row);
                self.selected_cell = self.field_cell_at(column).map(|cell| (row, cell));
            }
            None => {
                self.selected_field = None;
                self.selected_cell = None;
            }
        }
        self.sync_selected_input(cx);
    }

    /// Make one field row active from row-level navigation (the wheel, a gutter action) without
    /// opening an editor; the cell selection lands on its 列名 cell.
    pub(super) fn select_field(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        self.selected_field = Some(row);
        self.selected_cell = Some((row, FieldCell::Name));
        self.field_selection = Some(CellSelection::new(
            row,
            self.field_cell_column(FieldCell::Name),
        ));
        self.default_menu_open = false;
        self.sync_selected_input(cx);
        cx.notify();
    }

    /// Make one cell active (an editor opened or a keyboard move landed there). A cell inside the
    /// current selection keeps the rest of the range; a cell outside it becomes the new selection.
    pub(super) fn select_field_cell(
        &mut self,
        row: usize,
        cell: FieldCell,
        cx: &mut Context<'_, Self>,
    ) {
        self.selected_field = Some(row);
        self.selected_cell = Some((row, cell));
        let column = self.field_cell_column(cell);
        if !self
            .field_selection
            .as_ref()
            .is_some_and(|selection| selection.contains(row, column))
        {
            self.field_selection = Some(CellSelection::new(row, column));
        }
        self.default_menu_open = false;
        self.sync_selected_input(cx);
        cx.notify();
    }

    /// Copy the selected cells as TSV, like the table grid: the selection's bounding rectangle,
    /// with cells outside every range left empty. The text of one cell is what its editor shows
    /// (a toggle contributes `1` when checked).
    pub(super) fn copy_field_selection(&mut self, cx: &mut Context<'_, Self>) {
        if self.tab != DesignerTab::Fields {
            return;
        }
        let Some(selection) = self.field_selection.clone() else {
            return;
        };
        if selection.ranges.is_empty() || self.schema.columns.is_empty() {
            return;
        }
        let (start_row, end_row) = selection.rows();
        let (start_col, end_col) = selection.cols();
        let mut lines = Vec::new();
        for row in start_row..=end_row {
            let Some(column) = self.schema.columns.get(row) else {
                continue;
            };
            let mut values = Vec::new();
            for col in start_col..=end_col {
                values.push(if selection.contains(row, col) {
                    self.field_cell_copy_text(column, col)
                } else {
                    String::new()
                });
            }
            lines.push(values.join("\t"));
        }
        cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
        // Also keep the whole copied columns (not just the selected cells) in the shared field
        // clipboard, so 粘贴 in another designer reproduces each field exactly.
        let copied: Vec<ColumnDef> = (start_row..=end_row)
            .filter_map(|row| self.schema.columns.get(row).cloned())
            .collect();
        *self.design_clipboard.borrow_mut() = copied;
        cx.notify();
    }

    /// Ctrl/Cmd+V in the 字段 grid: append the columns copied in any designer (see
    /// [`Self::copy_field_selection`]) to the end of the field list. Each pasted field keeps its
    /// copied definition but drops the primary-key, auto-increment and identity flags, so it never
    /// collides with the target table's own key, and takes a unique name when the target already has
    /// that column.
    pub(super) fn paste_fields(&mut self, cx: &mut Context<'_, Self>) {
        if self.tab != DesignerTab::Fields {
            return;
        }
        let copied: Vec<ColumnDef> = self.design_clipboard.borrow().clone();
        if copied.is_empty() {
            return;
        }
        let at = self.schema.columns.len();
        let mut existing: Vec<String> = self
            .schema
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect();
        let mut last = at;
        for (offset, mut column) in copied.into_iter().enumerate() {
            column.primary_key = false;
            column.auto_increment = false;
            column.identity.clear();
            column.identity_start.clear();
            column.identity_increment.clear();
            if !column.name.is_empty() {
                column.name = unique_column_name(&column.name, &mut existing);
            }
            last = at + offset;
            self.schema.columns.insert(last, column);
        }
        self.selected_field = Some(last);
        self.selected_cell = Some((last, FieldCell::Name));
        let last_column = self.field_column_count().saturating_sub(1);
        self.field_selection = Some(CellSelection::single((at, 0), (last, last_column)));
        self.reveal_design_row(last);
        self.sync_selected_input(cx);
        self.mark_dirty(cx);
    }

    /// The text a fields-grid cell contributes to a copy: the edited value for text cells, `1`
    /// for a checked toggle (a generated column copies its expression instead).
    fn field_cell_copy_text(&self, column: &ColumnDef, display_column: usize) -> String {
        let Some(kind) = self
            .designer
            .field_columns
            .get(display_column.wrapping_sub(2))
        else {
            return match display_column {
                0 => column.name.clone(),
                1 => column.data_type.clone(),
                _ => String::new(),
            };
        };
        if let Some(text_column) = field_column_text(*kind) {
            return field_column_value(column, text_column);
        }
        match kind {
            DesignerFieldColumn::NotNull => (if column.nullable { 0 } else { 1 }).to_string(),
            DesignerFieldColumn::Generated | DesignerFieldColumn::Computed => {
                column.generated.clone()
            }
            DesignerFieldColumn::Sparse => (if column.sparse { 1 } else { 0 }).to_string(),
            DesignerFieldColumn::Key => (if column.primary_key { 1 } else { 0 }).to_string(),
            _ => field_column_display(*kind, column),
        }
    }

    /// Ctrl+A over the fields grid: select every field cell, like the table grid.
    pub(super) fn select_all_fields(&mut self, cx: &mut Context<'_, Self>) {
        if self.tab != DesignerTab::Fields || self.schema.columns.is_empty() {
            return;
        }
        self.edit = None;
        self.edit_blur = None;
        self.type_combo = None;
        self.default_menu_open = false;
        let rows = self.schema.columns.len();
        let last_column = self.field_column_count().saturating_sub(1);
        let active = self.selected_field.filter(|row| *row < rows).unwrap_or(0);
        self.field_selection = Some(CellSelection::single((0, 0), (rows - 1, last_column)));
        self.selected_field = Some(active);
        self.selected_cell = Some((active, FieldCell::Name));
        self.field_selecting = false;
        self.field_cell_press = None;
        self.field_cell_dragged = false;
        self.sync_selected_input(cx);
        cx.notify();
    }

    /// Press on one cell of the fields grid: click selects, Shift extends the active range, Ctrl
    /// toggles a cell in/out — the table grid's mouse-down logic. A press that is a re-click or a
    /// double-click opens the editor on mouse-up.
    pub(super) fn field_cell_mouse_down(
        &mut self,
        row: usize,
        column: usize,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.tab != DesignerTab::Fields || row >= self.schema.columns.len() {
            return;
        }
        // The in-place editor owns mouse input inside its own cell (caret and text drag).
        if self.edit.as_ref().is_some_and(|edit| {
            edit.row == row && self.field_cell_column(field_cell_of(edit.column)) == column
        }) {
            self.field_selecting = false;
            self.field_cell_press = None;
            return;
        }
        self.edit = None;
        self.edit_blur = None;
        if self
            .type_combo
            .is_some_and(|open| open != row || column != 1)
        {
            self.type_combo = None;
        }
        self.default_menu_open = false;
        self.field_cell_dragged = false;
        let ctrl = modifiers.control || modifiers.platform;
        self.field_selecting = true;
        if modifiers.shift {
            self.field_cell_press = None;
            if let Some(selection) = self.field_selection.as_mut() {
                let active = selection.active.min(selection.ranges.len() - 1);
                selection.ranges[active].cursor = (row, column);
            } else {
                self.field_selection = Some(CellSelection::new(row, column));
            }
        } else if ctrl {
            self.field_cell_press = None;
            self.add_or_remove_field_range(row, column, row, column);
        } else {
            self.field_cell_reselect = self.field_selection.as_ref().is_some_and(|selection| {
                selection.ranges.len() == 1
                    && selection.active().anchor == (row, column)
                    && selection.active().cursor == (row, column)
            });
            self.field_selection = Some(CellSelection::new(row, column));
            self.field_cell_press = Some((row, column));
        }
        self.sync_field_active(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Press on a row of the fields grid's left gutter: the whole row (all columns) is selected,
    /// and dragging down the gutter extends whole rows — the same gesture as the table grid.
    pub(super) fn field_gutter_mouse_down(
        &mut self,
        row: usize,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.tab != DesignerTab::Fields || row >= self.schema.columns.len() {
            return;
        }
        self.edit = None;
        self.edit_blur = None;
        self.type_combo = None;
        self.default_menu_open = false;
        self.field_cell_dragged = false;
        self.field_cell_press = None;
        let last = self.field_column_count().saturating_sub(1);
        let ctrl = modifiers.control || modifiers.platform;
        self.field_selecting = true;
        if modifiers.shift {
            if let Some(selection) = self.field_selection.as_mut() {
                let active = selection.active.min(selection.ranges.len() - 1);
                selection.ranges[active].cursor = (row, last);
            } else {
                self.field_selection = Some(CellSelection::single((row, 0), (row, last)));
            }
        } else if ctrl {
            self.add_or_remove_field_range(row, 0, row, last);
        } else {
            self.field_selection = Some(CellSelection::single((row, 0), (row, last)));
        }
        self.sync_field_active(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Extend the active range to the cell under the pointer while the button is down.
    pub(super) fn field_cell_mouse_move(
        &mut self,
        row: usize,
        column: usize,
        event: &MouseMoveEvent,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.field_selecting
            || event.pressed_button != Some(MouseButton::Left)
            || row >= self.schema.columns.len()
        {
            return;
        }
        let mut changed = false;
        if let Some(selection) = self.field_selection.as_mut()
            && let Some(range) = selection.ranges.get_mut(selection.active)
            && range.cursor != (row, column)
        {
            range.cursor = (row, column);
            changed = true;
        }
        if self
            .field_cell_press
            .is_some_and(|press| press != (row, column))
            && !self.field_cell_dragged
        {
            self.field_cell_dragged = true;
            changed = true;
        }
        if changed {
            self.sync_field_active(cx);
            cx.notify();
        }
    }

    /// Finish a fields-grid press: a re-click / double-click on the pressed cell opens its editor,
    /// a drag leaves the range selected.
    pub(super) fn field_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.field_selecting && self.field_cell_press.is_none() {
            return;
        }
        self.field_selecting = false;
        let press = self.field_cell_press.take();
        let dragged = self.field_cell_dragged;
        let reselect = std::mem::replace(&mut self.field_cell_reselect, false);
        if let Some((row, column)) = press
            && !dragged
            && (event.click_count >= 2 || reselect)
        {
            self.begin_field_cell_edit(row, column, window, cx);
        }
        cx.notify();
    }

    /// Whether a drag is in progress, so a checkbox's click must not toggle under the range.
    pub(super) fn field_drag_in_progress(&self) -> bool {
        self.field_cell_dragged
    }

    /// Open the editor of one display column, if that column is editable.
    fn begin_field_cell_edit(
        &mut self,
        row: usize,
        column: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match column {
            0 => self.begin_edit(row, FieldColumn::Name, window, cx),
            1 => self.open_type_combo(row, window, cx),
            _ => {
                if let Some(FieldCell::Text(column)) = self.field_cell_at(column) {
                    self.begin_edit(row, column, window, cx);
                }
            }
        }
    }

    /// Add a range to the selection, or remove the range that already contains its anchor
    /// (Ctrl-click toggle), exactly like the table grid.
    fn add_or_remove_field_range(
        &mut self,
        anchor_row: usize,
        anchor_col: usize,
        cursor_row: usize,
        cursor_col: usize,
    ) {
        if self.field_selection.is_none() {
            self.field_selection = Some(CellSelection::single(
                (anchor_row, anchor_col),
                (cursor_row, cursor_col),
            ));
            return;
        }
        let empty = {
            let Some(selection) = self.field_selection.as_mut() else {
                return;
            };
            if let Some(position) = selection
                .ranges
                .iter()
                .position(|range| range.contains(anchor_row, anchor_col))
            {
                selection.ranges.remove(position);
            } else {
                selection.ranges.push(CellRange {
                    anchor: (anchor_row, anchor_col),
                    cursor: (cursor_row, cursor_col),
                });
                selection.active = selection.ranges.len() - 1;
            }
            if !selection.ranges.is_empty() {
                selection.active = selection.active.min(selection.ranges.len() - 1);
            }
            selection.ranges.is_empty()
        };
        if empty {
            self.field_selection = None;
        }
    }

    /// Drop every in-place editor and transient popup state of the designer. Scrolling and switching
    /// tabs both end an edit this way (the text is already in the working copy).
    pub(super) fn clear_editors(&mut self) {
        self.edit = None;
        self.edit_blur = None;
        self.type_combo = None;
        self.default_menu_open = false;
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_combo = None;
        self.index_fields_draft = None;
        self.discard_index_field_inputs();
        self.unique_key_edit = None;
        self.unique_key_edit_blur = None;
        self.unique_key_fields_row = None;
        self.check_edit = None;
        self.check_edit_blur = None;
        self.exclude_edit = None;
        self.exclude_edit_blur = None;
        self.rule_editor_row = None;
        self.trigger_edit = None;
        self.trigger_edit_blur = None;
        self.trigger_editor_row = None;
        self.fk_edit = None;
        self.fk_edit_blur = None;
        self.fk_combo = None;
        self.fk_fields_open = false;
        self.fk_ref_open = false;
    }

    /// Take focus on the next render: called when the designer opens or becomes the active tab, so
    /// its key context (Ctrl+S) is in the focused element's path straight away.
    pub(super) fn request_focus(&mut self, cx: &mut Context<'_, Self>) {
        self.focus_pending = true;
        cx.notify();
    }

    pub(super) fn select_tab(&mut self, tab: DesignerTab, cx: &mut Context<'_, Self>) {
        self.tab = tab;
        self.field_selecting = false;
        self.field_cell_press = None;
        self.field_cell_dragged = false;
        self.clear_editors();
        if matches!(tab, DesignerTab::Options | DesignerTab::Comment) {
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
        let submit = weak.clone();
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
            // Enter commits and moves one row down, as it does in the table grid.
            .on_submit(Rc::new(move |window, cx| {
                let _ = submit.update(cx, |view, cx| view.move_designer_row(1, window, cx));
            }))
        });
        let focus = input.read(cx).focus_handle();
        input.update(cx, |input, cx| input.set_padding_left(0.0, cx));
        self.type_combo = None;
        self.edit = None;
        self.edit_blur = None;
        self.select_field_cell(row, field_cell_of(column), cx);
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
        self.select_field_cell(row, FieldCell::Type, cx);
        if self.type_combo == Some(row) {
            self.type_combo = None;
            cx.notify();
            return;
        }
        self.type_combo = Some(row);
        self.type_query.clear();
        // Start the highlight on the type the column already has, so Up/Down walk the list from
        // where the user is rather than from the top.
        let current = self
            .schema
            .columns
            .get(row)
            .map(|column| column.data_type.clone())
            .unwrap_or_default();
        self.type_highlight = self
            .filtered_types()
            .iter()
            .position(|data_type| *data_type == current)
            .or(Some(0));
        self.type_scroll.set_offset(Point::default());
        let theme = self.theme;
        let weak = self.self_weak.clone();
        let input = self
            .type_search
            .get_or_insert_with(|| {
                let change = weak.clone();
                let submit = weak.clone();
                let cancel = weak.clone();
                let arrows = weak;
                cx.new(move |cx| {
                    TextInput::new(
                        theme,
                        "",
                        TextInputOptions {
                            size: Some(Size::XSmall),
                            text_size: Some(12.0),
                            placeholder: t!("design.type_search").to_string().into(),
                            icon: Some("icons/search.svg"),
                            icon_color: Some(theme.text_muted),
                            clearable: true,
                            bare: true,
                            ..Default::default()
                        },
                        cx,
                    )
                    .on_change(Rc::new(move |text, _window, cx| {
                        let _ = change.update(cx, |view, cx| {
                            view.type_query = text.to_string();
                            // The filtered list changed under the highlight: start from its top.
                            view.type_highlight = Some(0);
                            view.type_scroll.set_offset(Point::default());
                            cx.notify();
                        });
                    }))
                    .on_submit(Rc::new(move |_window, cx| {
                        let _ = submit.update(cx, |view, cx| view.select_highlighted_type(cx));
                    }))
                    .on_cancel(Rc::new(move |_window, cx| {
                        let _ = cancel.update(cx, |view, cx| {
                            view.type_combo = None;
                            cx.notify();
                        });
                    }))
                    // Up/Down move the list's highlight. Tab is claimed by the designer's own key
                    // binding (see `tab_designer_cell`), so it needs no callback here.
                    .on_arrow(Rc::new(move |down, _window, cx| {
                        let _ = arrows.update(cx, |view, cx| view.move_type_highlight(down, cx));
                    }))
                })
            })
            .clone();
        input.update(cx, |input, cx| input.set_text("", cx));
        let focus = input.read(cx).focus_handle();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Tab / Shift+Tab inside the designer, from its `NextDesignerCell` / `PrevDesignerCell` bindings.
    ///
    /// A `tab` keystroke is claimed by a key binding — the `Root` context binds it to `focus_next`,
    /// and gpui dispatches a keystroke a binding matches as an action instead of running an element's
    /// own `on_key_down` — so the fields grid takes it here: the caret walks the row's cells
    /// (列名 → 类型 → the engine's text cells) and rolls onto the neighbouring row, instead of dropping
    /// straight to the detail panel below the grid. Anywhere else in the designer — the detail panel's
    /// own fields, the other tabs — the keystroke is left to the window's focus chain.
    pub(super) fn tab_designer_cell(
        &mut self,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((row, cell)) = self.editing_field_cell() else {
            if shift {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            return;
        };
        self.tab_field_cell(row, cell, shift, window, cx);
    }

    /// Arrow/Enter handling for the fields grid while the designer itself holds focus. Editing
    /// keystrokes stay with the cell editor (see `move_designer_row` and the input binding).
    pub(super) fn designer_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.tab != DesignerTab::Fields
            || self.edit.is_some()
            || self.type_combo.is_some()
            || !self.focus.is_focused(window)
        {
            return;
        }
        if event.keystroke.modifiers.control || event.keystroke.modifiers.platform {
            return;
        }
        let Some((row, cell)) = self
            .selected_cell
            .or_else(|| self.selected_field.map(|row| (row, FieldCell::Name)))
        else {
            return;
        };
        match event.keystroke.key.as_str() {
            "up" => {
                self.move_designer_cell(row, cell, -1, 0, cx);
                cx.stop_propagation();
            }
            "down" => {
                self.move_designer_cell(row, cell, 1, 0, cx);
                cx.stop_propagation();
            }
            "left" => {
                self.move_designer_cell(row, cell, 0, -1, cx);
                cx.stop_propagation();
            }
            "right" => {
                self.move_designer_cell(row, cell, 0, 1, cx);
                cx.stop_propagation();
            }
            "enter" | "f2" => {
                self.open_field_cell(row, cell, window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// Move the selected fields-grid cell one step, clamping at the grid edges. Vertical movement
    /// keeps the cell kind, so a 长度 stays a 长度 when moving up/down.
    fn move_designer_cell(
        &mut self,
        row: usize,
        cell: FieldCell,
        d_row: isize,
        d_col: isize,
        cx: &mut Context<'_, Self>,
    ) {
        let rows = self.schema.columns.len();
        if rows == 0 {
            return;
        }
        let mut target_row = row;
        let mut target_cell = cell;
        if d_row != 0 {
            target_row = (row as isize + d_row).clamp(0, rows as isize - 1) as usize;
        }
        if d_col != 0 {
            let chain = self.field_cell_chain();
            if let Some(position) = chain.iter().position(|candidate| *candidate == cell) {
                let position =
                    (position as isize + d_col).clamp(0, chain.len() as isize - 1) as usize;
                target_cell = chain[position];
            }
        }
        self.selected_field = Some(target_row);
        self.selected_cell = Some((target_row, target_cell));
        self.field_selection = Some(CellSelection::new(
            target_row,
            self.field_cell_column(target_cell),
        ));
        self.field_selecting = false;
        self.field_cell_press = None;
        self.default_menu_open = false;
        self.reveal_design_row(target_row);
        self.sync_selected_input(cx);
        cx.notify();
    }

    /// Up/Down from a designer cell editor: commit its text (already in the working copy through
    /// `on_change`), close the editor, move the selection and hand focus back to the designer so
    /// the next arrow key keeps navigating, exactly like the table grid.
    pub(super) fn move_designer_row(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        let (row, cell) = (edit.row, field_cell_of(edit.column));
        self.edit_blur = None;
        self.type_combo = None;
        self.move_designer_cell(row, cell, delta, 0, cx);
        window.focus(&self.focus, cx);
    }

    /// The fields-grid cell the caret is in, if any: the open type drop-down counts as 类型, and the
    /// in-place text editor reports its own row and column.
    fn editing_field_cell(&self) -> Option<(usize, FieldCell)> {
        if let Some(row) = self.type_combo {
            return Some((row, FieldCell::Type));
        }
        self.edit
            .as_ref()
            .map(|edit| (edit.row, field_cell_of(edit.column)))
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

    /// The cells Tab walks through, in the order the fields grid shows them: 列名, 类型, then the
    /// engine's own text cells (长度/小数位/备注 for MySQL, 大小/比例/排序规则 for SQL Server).
    pub(super) fn field_cell_chain(&self) -> Vec<FieldCell> {
        let mut chain = vec![FieldCell::Name, FieldCell::Type];
        for kind in &self.designer.field_columns {
            // Two engines' columns can name the same cell (`size`/`length`), so the chain keeps one.
            if let Some(column) = field_column_text(*kind) {
                let cell = FieldCell::Text(column);
                if !chain.contains(&cell) {
                    chain.push(cell);
                }
            }
        }
        chain
    }

    /// Tab (or Shift+Tab) from one cell of the fields grid: hand the caret to the neighbouring cell
    /// of that row, wrap to the previous/next row at either end, and let the window's own focus
    /// chain take over beyond the grid's first and last cell.
    pub(super) fn tab_field_cell(
        &mut self,
        row: usize,
        cell: FieldCell,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let rows = self.schema.columns.len();
        let chain = self.field_cell_chain();
        if !chain.contains(&cell) {
            return;
        }
        match field_cell_neighbor(&chain, row, rows, cell, shift) {
            Some((row, cell)) => self.open_field_cell(row, cell, window, cx),
            // Beyond the grid's first (Shift+Tab) or last (Tab) cell the window's own focus chain
            // takes over — the detail panel below the grid is where the caret goes next.
            None if shift => window.focus_prev(cx),
            None => window.focus_next(cx),
        }
    }

    /// Put the caret in one cell of the fields grid, opening whichever editor that cell uses.
    fn open_field_cell(
        &mut self,
        row: usize,
        cell: FieldCell,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        // Moving on ends the editor we came from: its text already reached the working copy through
        // `on_change`, and the drop-down we came from must not stay open over the next cell.
        self.edit = None;
        self.edit_blur = None;
        self.type_combo = None;
        match cell {
            FieldCell::Name => self.begin_edit(row, FieldColumn::Name, window, cx),
            FieldCell::Type => self.open_type_combo(row, window, cx),
            FieldCell::Text(column) => self.begin_edit(row, column, window, cx),
        }
    }

    /// Move the type drop-down's highlight by one row (Up/Down), wrapping at both ends.
    pub(super) fn move_type_highlight(&mut self, down: bool, cx: &mut Context<'_, Self>) {
        let count = self.filtered_types().len();
        self.type_highlight = next_type_highlight(count, self.type_highlight, down);
        self.keep_type_highlight_visible();
        cx.notify();
    }

    /// Scroll the type list so the highlighted row is inside the visible part of the drop-down.
    pub(super) fn keep_type_highlight_visible(&self) {
        let Some(index) = self.type_highlight else {
            return;
        };
        let viewport = f32::from(self.type_scroll.bounds().size.height);
        if viewport <= 0.0 {
            return;
        }
        let max = f32::from(self.type_scroll.max_offset().y);
        let top = index as f32 * TYPE_OPTION_HEIGHT;
        let current = -f32::from(self.type_scroll.offset().y);
        let next = if top < current {
            top
        } else if top + TYPE_OPTION_HEIGHT > current + viewport {
            top + TYPE_OPTION_HEIGHT - viewport
        } else {
            current
        };
        let next = next.clamp(0.0, max);
        if (next - current).abs() >= 0.5 {
            self.type_scroll.set_offset(Point::new(px(0.0), px(-next)));
        }
    }

    /// The type the drop-down's highlight is on: the row the user moved to, or the first match when
    /// the search text just narrowed the list and nothing has been moved yet.
    fn highlighted_type(&self) -> Option<&'static str> {
        let types = self.filtered_types();
        let index = self
            .type_highlight
            .unwrap_or(0)
            .min(types.len().checked_sub(1)?);
        types.get(index).copied()
    }

    /// Enter in the type search: take the highlighted type (or the best match when the search just
    /// narrowed the list).
    fn select_highlighted_type(&mut self, cx: &mut Context<'_, Self>) {
        let Some(row) = self.type_combo else {
            return;
        };
        if let Some(data_type) = self.highlighted_type() {
            self.select_type(row, data_type, cx);
        }
    }

    pub(super) fn select_type(&mut self, row: usize, data_type: &str, cx: &mut Context<'_, Self>) {
        if let Some(column) = self.schema.columns.get_mut(row) {
            column.data_type = data_type.to_string();
            // A type that cannot carry the engine's auto-increment (SQL Server's `IDENTITY` is
            // numeric-only) drops it, so the preview never emits an invalid column.
            if column.auto_increment && !self.designer.auto_increment.allows(data_type) {
                column.auto_increment = false;
            }
        }
        self.type_combo = None;
        self.selected_cell = Some((row, FieldCell::Type));
        // The popup's search field goes away with the list; hand the keys back to the designer so
        // the arrow keys keep moving the selected cell.
        self.focus_pending = true;
        self.mark_dirty(cx);
    }

    pub(super) fn toggle_not_null(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        if let Some(column) = self.schema.columns.get_mut(row) {
            column.nullable = !column.nullable;
        }
        self.mark_dirty(cx);
    }

    /// Toggle the selected column's auto-increment (identity). Engines allow one such column per
    /// table at most, so turning it on moves it here — the flags are a `bool` per column and only
    /// one engine-level rule decides which of them may be set.
    pub(super) fn toggle_auto_increment(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.selected_field else {
            return;
        };
        let Some(column) = self.schema.columns.get(index) else {
            return;
        };
        // Only a type the engine accepts may carry it (SQL Server's `IDENTITY` is numeric-only).
        if !column.auto_increment && !self.designer.auto_increment.allows(&column.data_type) {
            return;
        }
        let turning_on = !self.schema.columns[index].auto_increment;
        if turning_on && self.designer.auto_increment.one_per_table {
            // One identity column per table: moving it here takes it off whichever column had it.
            for (position, other) in self.schema.columns.iter_mut().enumerate() {
                if position != index {
                    other.auto_increment = false;
                    other.identity.clear();
                    other.identity_start.clear();
                    other.identity_increment.clear();
                }
            }
        }
        // Only engines that declare the identity sequence controls (PostgreSQL) use these fields,
        // and only they must keep a default off an identity column.
        let identity_controls = self
            .designer
            .column_options
            .iter()
            .any(|option| matches!(option, DesignerColumnOption::IdentityGeneration));
        if let Some(column) = self.schema.columns.get_mut(index) {
            column.auto_increment = !column.auto_increment;
            if column.auto_increment {
                column.nullable = false;
                if identity_controls {
                    column.default.clear();
                    // Start as PostgreSQL's `BY DEFAULT` mode with the sequence's usual 1/1
                    // options; both are editable below.
                    column.identity = "BY DEFAULT".to_string();
                    column.identity_start = "1".to_string();
                    column.identity_increment = "1".to_string();
                }
            } else if identity_controls {
                column.identity.clear();
                column.identity_start.clear();
                column.identity_increment.clear();
            }
        }
        self.sync_selected_input(cx);
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

    /// Toggle the selected column's `SPARSE` flag (SQL Server).
    pub(super) fn toggle_sparse(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.selected_field
            && let Some(column) = self.schema.columns.get_mut(index)
        {
            column.sparse = !column.sparse;
        }
        self.mark_dirty(cx);
    }

    /// Whether the engine's designer shows a control fed by the character-set/collation catalogues,
    /// so the (otherwise pointless) catalogue queries are skipped for engines without one.
    pub(super) fn needs_option_catalogs(&self) -> bool {
        self.designer.option_fields.iter().any(|field| {
            matches!(
                field,
                DesignerOptionField::Charset | DesignerOptionField::Collation
            )
        }) || self
            .designer
            .column_options
            .contains(&DesignerColumnOption::Collation)
    }

    // -- Unique keys (SQL Server's 唯一键; MySQL reports a unique index as an 索引) ----------------

    pub(super) fn add_unique_key(&mut self, cx: &mut Context<'_, Self>) {
        // A new row carries no invented name (`new_unique_key`): the name is the user's, and the
        // row that 添加唯一键 creates opens straight into its name cell for typing.
        let key = UniqueKeyDef {
            name: String::new(),
            columns: Vec::new(),
        };
        self.schema.unique_keys.push(key);
        self.selected_unique_key = Some(self.schema.unique_keys.len() - 1);
        self.unique_key_rows.rows.clear();
        self.unique_key_rows
            .rows
            .insert(self.schema.unique_keys.len() - 1);
        self.unique_key_rows.anchor = self.selected_unique_key;
        self.reveal_design_row(self.schema.unique_keys.len() - 1);
        self.mark_dirty(cx);
    }

    pub(super) fn delete_unique_key(&mut self, cx: &mut Context<'_, Self>) {
        // Delete every gutter-selected row, falling back to the active one.
        let mut targets: BTreeSet<usize> = self.unique_key_rows.rows.clone();
        if let Some(active) = self.selected_unique_key {
            targets.insert(active);
        }
        targets.retain(|row| *row < self.schema.unique_keys.len());
        if targets.is_empty() {
            return;
        }
        let first = *targets.iter().next().unwrap();
        for row in targets.iter().rev() {
            self.schema.unique_keys.remove(*row);
        }
        self.selected_unique_key = if self.schema.unique_keys.is_empty() {
            None
        } else {
            Some(first.min(self.schema.unique_keys.len() - 1))
        };
        self.unique_key_rows.rows.clear();
        if let Some(active) = self.selected_unique_key {
            self.unique_key_rows.rows.insert(active);
            self.unique_key_rows.anchor = Some(active);
        }
        self.unique_key_edit = None;
        self.unique_key_edit_blur = None;
        self.unique_key_fields_row = None;
        self.mark_dirty(cx);
    }

    pub(super) fn select_unique_key(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        self.selected_unique_key = Some(row);
        self.unique_key_rows.rows.clear();
        self.unique_key_rows.rows.insert(row);
        self.unique_key_rows.anchor = Some(row);
        cx.notify();
    }

    /// Select one row (or extend/toggle the selection) from the unique-key grid's left gutter.
    pub(super) fn select_unique_key_row(
        &mut self,
        row: usize,
        modifiers: &Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        self.unique_key_edit = None;
        self.unique_key_edit_blur = None;
        self.unique_key_fields_row = None;
        apply_row_selection(
            &mut self.unique_key_rows,
            &mut self.selected_unique_key,
            row,
            modifiers,
        );
        cx.notify();
    }

    fn set_unique_key_value(
        &mut self,
        row: usize,
        column: UniqueKeyColumn,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(key) = self.schema.unique_keys.get_mut(row) {
            match column {
                UniqueKeyColumn::Name => key.name = text.to_string(),
            }
            self.dirty = true;
        }
        cx.notify();
    }

    pub(super) fn begin_unique_key_edit(
        &mut self,
        row: usize,
        column: UniqueKeyColumn,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(key) = self.schema.unique_keys.get(row) else {
            return;
        };
        let value = match column {
            UniqueKeyColumn::Name => key.name.clone(),
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
                let _ = weak.update(cx, |view, cx| {
                    view.set_unique_key_value(row, column, text, cx)
                });
            }))
        });
        let focus = input.read(cx).focus_handle();
        input.update(cx, |input, cx| input.set_padding_left(0.0, cx));
        self.unique_key_edit = None;
        self.unique_key_edit_blur = None;
        self.unique_key_fields_row = None;
        self.select_unique_key(row, cx);
        self.unique_key_edit_blur = Some(cx.on_blur(&focus, window, |this, _window, cx| {
            this.finish_unique_key_edit(cx)
        }));
        self.unique_key_edit = Some(UniqueKeyEdit { row, column, input });
        window.focus(&focus, cx);
        cx.notify();
    }

    fn finish_unique_key_edit(&mut self, cx: &mut Context<'_, Self>) {
        if self.unique_key_edit.take().is_some() {
            cx.notify();
        }
    }

    /// Open the "choose fields" popup for one unique key.
    pub(super) fn open_unique_key_fields(
        &mut self,
        row: usize,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.schema.unique_keys.get(row).is_none() {
            return;
        }
        self.unique_key_edit = None;
        self.unique_key_edit_blur = None;
        self.select_unique_key(row, cx);
        self.unique_key_fields_row = Some(row);
        cx.notify();
    }

    /// Add or remove `column` from a unique key's column list.
    pub(super) fn toggle_unique_key_column(
        &mut self,
        row: usize,
        column: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(key) = self.schema.unique_keys.get_mut(row) {
            toggle_list(&mut key.columns, column);
        }
        self.mark_dirty(cx);
    }

    /// Append a unique key (toolbar's 添加唯一键 or the grid's blank placeholder row), then edit its
    /// name.
    pub(super) fn begin_empty_unique_key(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let row = self.schema.unique_keys.len();
        self.add_unique_key(cx);
        self.begin_unique_key_edit(row, UniqueKeyColumn::Name, window, cx);
    }

    // -- Check constraints (SQL Server's 检查) ---------------------------------------------------

    pub(super) fn add_check(&mut self, cx: &mut Context<'_, Self>) {
        // Like 添加字段, a new check starts unnamed and opens its name cell for editing.
        let check = CheckDef {
            name: String::new(),
            expression: String::new(),
        };
        self.schema.checks.push(check);
        self.selected_check = Some(self.schema.checks.len() - 1);
        self.check_rows.rows.clear();
        self.check_rows.rows.insert(self.schema.checks.len() - 1);
        self.check_rows.anchor = self.selected_check;
        self.reveal_design_row(self.schema.checks.len() - 1);
        self.mark_dirty(cx);
    }

    pub(super) fn delete_check(&mut self, cx: &mut Context<'_, Self>) {
        let mut targets: BTreeSet<usize> = self.check_rows.rows.clone();
        if let Some(active) = self.selected_check {
            targets.insert(active);
        }
        targets.retain(|row| *row < self.schema.checks.len());
        if targets.is_empty() {
            return;
        }
        let first = *targets.iter().next().unwrap();
        for row in targets.iter().rev() {
            self.schema.checks.remove(*row);
        }
        self.selected_check = if self.schema.checks.is_empty() {
            None
        } else {
            Some(first.min(self.schema.checks.len() - 1))
        };
        self.check_rows.rows.clear();
        if let Some(active) = self.selected_check {
            self.check_rows.rows.insert(active);
            self.check_rows.anchor = Some(active);
        }
        self.check_edit = None;
        self.check_edit_blur = None;
        self.mark_dirty(cx);
    }

    pub(super) fn select_check(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        self.selected_check = Some(row);
        self.check_rows.rows.clear();
        self.check_rows.rows.insert(row);
        self.check_rows.anchor = Some(row);
        cx.notify();
    }

    /// Select one row (or extend/toggle the selection) from the check grid's left gutter.
    pub(super) fn select_check_row(
        &mut self,
        row: usize,
        modifiers: &Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        self.check_edit = None;
        self.check_edit_blur = None;
        apply_row_selection(
            &mut self.check_rows,
            &mut self.selected_check,
            row,
            modifiers,
        );
        cx.notify();
    }

    fn set_check_value(
        &mut self,
        row: usize,
        column: CheckColumn,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(check) = self.schema.checks.get_mut(row) {
            match column {
                CheckColumn::Name => check.name = text.to_string(),
                CheckColumn::Expression => check.expression = text.to_string(),
            }
            self.dirty = true;
        }
        cx.notify();
    }

    pub(super) fn begin_check_edit(
        &mut self,
        row: usize,
        column: CheckColumn,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(check) = self.schema.checks.get(row) else {
            return;
        };
        let value = match column {
            CheckColumn::Name => check.name.clone(),
            CheckColumn::Expression => check.expression.clone(),
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
                let _ = weak.update(cx, |view, cx| view.set_check_value(row, column, text, cx));
            }))
        });
        let focus = input.read(cx).focus_handle();
        input.update(cx, |input, cx| input.set_padding_left(0.0, cx));
        self.check_edit = None;
        self.check_edit_blur = None;
        self.select_check(row, cx);
        self.check_edit_blur = Some(cx.on_blur(&focus, window, |this, _window, cx| {
            this.finish_check_edit(cx)
        }));
        self.check_edit = Some(CheckEdit { row, column, input });
        window.focus(&focus, cx);
        cx.notify();
    }

    fn finish_check_edit(&mut self, cx: &mut Context<'_, Self>) {
        if self.check_edit.take().is_some() {
            cx.notify();
        }
    }

    // -- PostgreSQL exclusion constraints -------------------------------------------------------

    pub(super) fn add_exclude(&mut self, cx: &mut Context<'_, Self>) {
        self.schema.excludes.push(ExcludeDef::default());
        let row = self.schema.excludes.len() - 1;
        self.selected_exclude = Some(row);
        self.exclude_rows.rows.clear();
        self.exclude_rows.rows.insert(row);
        self.exclude_rows.anchor = Some(row);
        self.reveal_design_row(row);
        self.mark_dirty(cx);
    }

    pub(super) fn begin_empty_exclude(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let row = self.schema.excludes.len();
        self.add_exclude(cx);
        self.begin_exclude_edit(row, ExcludeColumn::Name, window, cx);
    }

    pub(super) fn delete_exclude(&mut self, cx: &mut Context<'_, Self>) {
        let mut targets: BTreeSet<usize> = self.exclude_rows.rows.clone();
        if let Some(active) = self.selected_exclude {
            targets.insert(active);
        }
        targets.retain(|row| *row < self.schema.excludes.len());
        if targets.is_empty() {
            return;
        }
        let first = *targets.iter().next().unwrap();
        for row in targets.iter().rev() {
            self.schema.excludes.remove(*row);
        }
        self.selected_exclude = if self.schema.excludes.is_empty() {
            None
        } else {
            Some(first.min(self.schema.excludes.len() - 1))
        };
        self.exclude_rows.rows.clear();
        if let Some(active) = self.selected_exclude {
            self.exclude_rows.rows.insert(active);
            self.exclude_rows.anchor = Some(active);
        }
        self.exclude_edit = None;
        self.exclude_edit_blur = None;
        self.mark_dirty(cx);
    }

    pub(super) fn select_exclude(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        self.selected_exclude = Some(row);
        self.exclude_rows.rows.clear();
        self.exclude_rows.rows.insert(row);
        self.exclude_rows.anchor = Some(row);
        cx.notify();
    }

    pub(super) fn select_exclude_row(
        &mut self,
        row: usize,
        modifiers: &Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        self.exclude_edit = None;
        self.exclude_edit_blur = None;
        apply_row_selection(
            &mut self.exclude_rows,
            &mut self.selected_exclude,
            row,
            modifiers,
        );
        cx.notify();
    }

    fn set_exclude_value(
        &mut self,
        row: usize,
        column: ExcludeColumn,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(exclude) = self.schema.excludes.get_mut(row) {
            match column {
                ExcludeColumn::Name => exclude.name = text.to_string(),
                ExcludeColumn::Definition => exclude.definition = text.to_string(),
                ExcludeColumn::Comment => exclude.comment = text.to_string(),
            }
            self.dirty = true;
        }
        cx.notify();
    }

    pub(super) fn begin_exclude_edit(
        &mut self,
        row: usize,
        column: ExcludeColumn,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(exclude) = self.schema.excludes.get(row) else {
            return;
        };
        let value = match column {
            ExcludeColumn::Name => exclude.name.clone(),
            ExcludeColumn::Definition => exclude.definition.clone(),
            ExcludeColumn::Comment => exclude.comment.clone(),
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
                let _ = weak.update(cx, |view, cx| view.set_exclude_value(row, column, text, cx));
            }))
        });
        let focus = input.read(cx).focus_handle();
        input.update(cx, |input, cx| input.set_padding_left(0.0, cx));
        self.exclude_edit = None;
        self.exclude_edit_blur = None;
        self.select_exclude(row, cx);
        self.exclude_edit_blur = Some(cx.on_blur(&focus, window, |this, _window, cx| {
            this.finish_exclude_edit(cx)
        }));
        self.exclude_edit = Some(ExcludeEdit { row, column, input });
        window.focus(&focus, cx);
        cx.notify();
    }

    fn finish_exclude_edit(&mut self, cx: &mut Context<'_, Self>) {
        if self.exclude_edit.take().is_some() {
            cx.notify();
        }
    }

    // -- PostgreSQL rewrite rules ---------------------------------------------------------------

    pub(super) fn add_rule(&mut self, cx: &mut Context<'_, Self>) {
        self.schema.rules.push(RuleDef::default());
        let row = self.schema.rules.len() - 1;
        self.selected_rule = Some(row);
        self.rule_rows.rows.clear();
        self.rule_rows.rows.insert(row);
        self.rule_rows.anchor = Some(row);
        self.reveal_design_row(row);
        self.mark_dirty(cx);
    }

    pub(super) fn begin_empty_rule(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) {
        self.add_rule(cx);
    }

    pub(super) fn delete_rule(&mut self, cx: &mut Context<'_, Self>) {
        let mut targets: BTreeSet<usize> = self.rule_rows.rows.clone();
        if let Some(active) = self.selected_rule {
            targets.insert(active);
        }
        targets.retain(|row| *row < self.schema.rules.len());
        if targets.is_empty() {
            return;
        }
        let first = *targets.iter().next().unwrap();
        for row in targets.iter().rev() {
            self.schema.rules.remove(*row);
        }
        self.selected_rule = if self.schema.rules.is_empty() {
            None
        } else {
            Some(first.min(self.schema.rules.len() - 1))
        };
        self.rule_rows.rows.clear();
        if let Some(active) = self.selected_rule {
            self.rule_rows.rows.insert(active);
            self.rule_rows.anchor = Some(active);
        }
        self.mark_dirty(cx);
    }

    pub(super) fn select_rule(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        self.selected_rule = Some(row);
        self.rule_rows.rows.clear();
        self.rule_rows.rows.insert(row);
        self.rule_rows.anchor = Some(row);
        cx.notify();
    }

    pub(super) fn select_rule_row(
        &mut self,
        row: usize,
        modifiers: &Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        apply_row_selection(&mut self.rule_rows, &mut self.selected_rule, row, modifiers);
        cx.notify();
    }

    pub(super) fn set_rule_definition(
        &mut self,
        row: usize,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(rule) = self.schema.rules.get_mut(row)
            && rule.definition != text
        {
            rule.definition = text.to_string();
            self.dirty = true;
        }
        cx.notify();
    }

    /// The rule editor is reused as the selection moves, so its callback must resolve the row when
    /// the text arrives rather than capturing the row it was created for.
    pub(super) fn set_selected_rule_definition(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        if let Some(row) = self.selected_rule {
            self.set_rule_definition(row, text, cx);
        }
    }

    /// Append a check constraint (toolbar's 添加检查 or the grid's blank placeholder row), then edit
    /// its name.
    pub(super) fn begin_empty_check(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let row = self.schema.checks.len();
        self.add_check(cx);
        self.begin_check_edit(row, CheckColumn::Name, window, cx);
    }

    pub(super) fn add_trigger(&mut self, cx: &mut Context<'_, Self>) {
        self.schema.triggers.push(TriggerDef {
            timing: "BEFORE".to_string(),
            event: "INSERT".to_string(),
            ..TriggerDef::default()
        });
        let row = self.schema.triggers.len() - 1;
        self.selected_trigger = Some(row);
        self.trigger_rows.rows.clear();
        self.trigger_rows.rows.insert(row);
        self.trigger_rows.anchor = Some(row);
        self.trigger_editor_row = None;
        self.reveal_design_row(row);
        self.mark_dirty(cx);
    }

    pub(super) fn delete_trigger(&mut self, cx: &mut Context<'_, Self>) {
        let mut targets: BTreeSet<usize> = self.trigger_rows.rows.clone();
        if let Some(active) = self.selected_trigger {
            targets.insert(active);
        }
        targets.retain(|row| *row < self.schema.triggers.len());
        if targets.is_empty() {
            return;
        }
        let first = *targets.iter().next().unwrap();
        for row in targets.iter().rev() {
            self.schema.triggers.remove(*row);
        }
        self.selected_trigger = if self.schema.triggers.is_empty() {
            None
        } else {
            Some(first.min(self.schema.triggers.len() - 1))
        };
        self.trigger_rows.rows.clear();
        if let Some(active) = self.selected_trigger {
            self.trigger_rows.rows.insert(active);
            self.trigger_rows.anchor = Some(active);
        }
        self.trigger_edit = None;
        self.trigger_edit_blur = None;
        self.trigger_editor_row = None;
        self.mark_dirty(cx);
    }

    pub(super) fn select_trigger(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        self.selected_trigger = Some(row);
        self.trigger_rows.rows.clear();
        self.trigger_rows.rows.insert(row);
        self.trigger_rows.anchor = Some(row);
        self.trigger_edit = None;
        self.trigger_edit_blur = None;
        cx.notify();
    }

    pub(super) fn select_trigger_row(
        &mut self,
        row: usize,
        modifiers: &Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        self.trigger_edit = None;
        self.trigger_edit_blur = None;
        apply_row_selection(
            &mut self.trigger_rows,
            &mut self.selected_trigger,
            row,
            modifiers,
        );
        self.trigger_editor_row = None;
        cx.notify();
    }

    fn set_trigger_value(
        &mut self,
        row: usize,
        column: TriggerColumn,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(trigger) = self.schema.triggers.get_mut(row) {
            match column {
                TriggerColumn::Name => trigger.name = text.to_string(),
                TriggerColumn::Timing => trigger.timing = text.to_string(),
                TriggerColumn::Event => trigger.event = text.to_string(),
            }
            self.dirty = true;
        }
        cx.notify();
    }

    pub(super) fn begin_trigger_edit(
        &mut self,
        row: usize,
        column: TriggerColumn,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(trigger) = self.schema.triggers.get(row) else {
            return;
        };
        let value = match column {
            TriggerColumn::Name => trigger.name.clone(),
            TriggerColumn::Timing => trigger.timing.clone(),
            TriggerColumn::Event => trigger.event.clone(),
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
                let _ = weak.update(cx, |view, cx| view.set_trigger_value(row, column, text, cx));
            }))
        });
        let focus = input.read(cx).focus_handle();
        input.update(cx, |input, cx| input.set_padding_left(0.0, cx));
        self.trigger_edit = None;
        self.trigger_edit_blur = None;
        self.select_trigger(row, cx);
        self.trigger_edit_blur = Some(cx.on_blur(&focus, window, |this, _window, cx| {
            this.finish_trigger_edit(cx)
        }));
        self.trigger_edit = Some(TriggerEdit { row, column, input });
        window.focus(&focus, cx);
        cx.notify();
    }

    fn finish_trigger_edit(&mut self, cx: &mut Context<'_, Self>) {
        if self.trigger_edit.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn begin_empty_trigger(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let row = self.schema.triggers.len();
        self.add_trigger(cx);
        self.begin_trigger_edit(row, TriggerColumn::Name, window, cx);
    }

    pub(super) fn set_trigger_body(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        let Some(row) = self.selected_trigger else {
            return;
        };
        let Some(trigger) = self.schema.triggers.get_mut(row) else {
            return;
        };
        if trigger.statement != text {
            trigger.statement = text.to_string();
            self.dirty = true;
            cx.notify();
        }
    }

    fn insert_field_at(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        // Default to `int` when the engine offers it (MySQL); otherwise use the engine's first
        // type, so SQLite defaults to `INTEGER`. The name stays empty — the row is either opened
        // for editing straight away (添加字段 / 插入字段) or typed into by hand — and
        // length/decimals stay empty (unset) too, matching a column loaded from an existing table.
        let data_type = self
            .column_types
            .iter()
            .find(|data_type| data_type.eq_ignore_ascii_case("int"))
            .or_else(|| self.column_types.first())
            .copied()
            .unwrap_or("int")
            .to_string();
        let column = ColumnDef {
            name: String::new(),
            data_type,
            nullable: true,
            ..ColumnDef::default()
        };
        let index = index.min(self.schema.columns.len());
        self.schema.columns.insert(index, column);
        self.selected_field = Some(index);
        self.selected_cell = Some((index, FieldCell::Name));
        self.field_selection = Some(CellSelection::new(index, 0));
        self.reveal_design_row(index);
        self.sync_selected_input(cx);
        self.mark_dirty(cx);
    }

    /// Append a blank field — the first row of a brand-new table (there is no window to open an
    /// editor with). The toolbar's 添加字段 goes through [`Self::begin_empty_field`], which also
    /// puts the caret in the new row's 列名 cell.
    pub(super) fn add_field(&mut self, cx: &mut Context<'_, Self>) {
        let index = self.schema.columns.len();
        self.insert_field_at(index, cx);
    }

    /// Create a field just below the selected one (插入字段), then edit its name.
    pub(super) fn begin_inserted_field(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let index = self.selected_field.map(|index| index + 1).unwrap_or(0);
        self.insert_field_at(index, cx);
        self.begin_edit(index, FieldColumn::Name, window, cx);
    }

    pub(super) fn delete_field(&mut self, cx: &mut Context<'_, Self>) {
        // Delete every field row the cell selection touches (the active row is the fallback when
        // nothing is selected).
        let mut targets: BTreeSet<usize> = self.selected_field_rows().into_iter().collect();
        targets.retain(|row| *row < self.schema.columns.len());
        if targets.is_empty() {
            return;
        }
        let first = *targets.iter().next().unwrap();
        let removed: Vec<String> = targets
            .iter()
            .filter_map(|row| self.schema.columns.get(*row))
            .map(|column| column.name.clone())
            .collect();
        for row in targets.iter().rev() {
            self.schema.columns.remove(*row);
        }

        // A deleted field must not leave an index, unique key or foreign key pointing at a column
        // that no longer exists: the DDL diff would otherwise emit a broken `ADD CONSTRAINT` (or
        // fail on the column drop). Remove the field from each object and drop an object left with
        // no columns. Check expressions are free-form SQL and are left to the user.
        for index in &mut self.schema.indexes {
            index
                .columns
                .retain(|column| !removed.iter().any(|name| name == &column.name));
        }
        self.schema
            .indexes
            .retain(|index| index.primary || !index.columns.is_empty());
        for key in &mut self.schema.unique_keys {
            key.columns.retain(|column| !removed.contains(column));
        }
        self.schema
            .unique_keys
            .retain(|key| !key.columns.is_empty());
        for foreign_key in &mut self.schema.foreign_keys {
            foreign_key
                .columns
                .retain(|column| !removed.contains(column));
        }
        self.schema
            .foreign_keys
            .retain(|foreign_key| !foreign_key.columns.is_empty());

        // The objects above may have lost rows; clear the selections/editors that pointed at them.
        self.selected_index = None;
        self.index_rows.rows.clear();
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_fields_draft = None;
        self.index_combo = None;
        self.selected_unique_key = None;
        self.unique_key_rows.rows.clear();
        self.unique_key_edit = None;
        self.unique_key_edit_blur = None;
        self.unique_key_fields_row = None;
        self.selected_fk = None;
        self.fk_rows.rows.clear();
        self.fk_edit = None;
        self.fk_edit_blur = None;
        self.fk_combo = None;
        self.fk_fields_open = false;
        self.fk_ref_open = false;

        self.selected_field = if self.schema.columns.is_empty() {
            None
        } else {
            Some(first.min(self.schema.columns.len() - 1))
        };
        self.selected_cell = self.selected_field.map(|row| (row, FieldCell::Name));
        self.field_selection = self
            .selected_field
            .map(|row| CellSelection::new(row, self.field_cell_column(FieldCell::Name)));
        self.field_selecting = false;
        self.field_cell_press = None;
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

    /// Move the selected field(s) one row up/down. A multi-row selection moves as one group:
    /// every selected row swaps with its neighbour in that direction (so the group keeps its
    /// internal order and steps over unselected rows), and the move is refused when the group is
    /// already against that edge.
    pub(super) fn move_field(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        let total = self.schema.columns.len();
        let delta = delta.signum();
        let rows = self.selected_field_rows();
        if rows.is_empty() || delta == 0 {
            return;
        }
        let Some(targets) = moved_field_rows(&rows, total, delta) else {
            return;
        };
        move_columns(&mut self.schema.columns, &rows, delta);
        let moved: BTreeSet<usize> = targets.into_iter().collect();

        // Keep the active row on the same column it was on, and the cell selection with it.
        let active = self
            .selected_field
            .map(|row| {
                if delta < 0 {
                    row.saturating_sub(1)
                } else {
                    row + 1
                }
            })
            .filter(|row| moved.contains(row))
            .or_else(|| moved.iter().next().copied());
        self.selected_field = active;
        if let Some((row, cell)) = self.selected_cell {
            let target = if delta < 0 {
                row.saturating_sub(1)
            } else {
                row + 1
            };
            self.selected_cell = (total > 0 && target < total).then_some((target, cell));
        }
        // The selected cells moved with their rows, so shift every range instead of collapsing the
        // selection: the user keeps the same block on the moved fields.
        if let Some(selection) = self.field_selection.as_mut() {
            for range in &mut selection.ranges {
                if delta < 0 {
                    range.anchor.0 -= 1;
                    range.cursor.0 -= 1;
                } else {
                    range.anchor.0 += 1;
                    range.cursor.0 += 1;
                }
            }
        }
        if let Some(row) = active {
            self.reveal_design_row(row);
        }
        self.mark_dirty(cx);
    }

    /// The rows the 索引 grid shows: the model's index list without its trailing primary key, which
    /// the grid never draws (see [`push_primary_index_last`]). A grid row is therefore a model row.
    pub(super) fn index_row_count(&self) -> usize {
        self.schema.indexes.len()
            - usize::from(
                self.schema
                    .indexes
                    .last()
                    .is_some_and(|index| index.primary),
            )
    }

    pub(super) fn add_index(&mut self, cx: &mut Context<'_, Self>) {
        let (unique, index_type) = default_index_kind(&self.designer);
        let index = IndexDef {
            name: String::new(),
            columns: Vec::new(),
            unique,
            primary: false,
            index_type,
            comment: String::new(),
        };
        // A new index goes before the primary key, which stays last so the grid keeps its rows and
        // the model's rows in step; the new row is also the last one the grid shows, so it can be
        // revealed by its own index.
        let row = self.index_row_count();
        self.schema.indexes.insert(row, index);
        self.selected_index = Some(row);
        self.index_rows.rows.clear();
        self.index_rows.rows.insert(row);
        self.index_rows.anchor = Some(row);
        self.reveal_design_row(row);
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
        self.selected_index = if self.index_row_count() == 0 {
            None
        } else {
            Some(first.min(self.index_row_count() - 1))
        };
        self.index_rows.rows.clear();
        if let Some(active) = self.selected_index {
            self.index_rows.rows.insert(active);
            self.index_rows.anchor = Some(active);
        }
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_combo = None;
        self.index_fields_draft = None;
        self.discard_index_field_inputs();
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
        self.index_fields_draft = None;
        self.discard_index_field_inputs();
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
        self.index_fields_draft = None;
        self.discard_index_field_inputs();
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
        if index.primary
            || (field == IndexComboField::Method
                && index_method_label(&self.designer, index).is_none())
        {
            return;
        }
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_fields_draft = None;
        self.discard_index_field_inputs();
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
        let kind = match field {
            IndexComboField::Kind => self
                .designer
                .index_kinds
                .iter()
                .find(|kind| kind.label == value)
                .copied(),
            IndexComboField::Method => None,
        };
        if let Some(index) = self.schema.indexes.get_mut(row) {
            match field {
                // The kind carries both halves of the model: uniqueness and the type it stores.
                IndexComboField::Kind => {
                    if let Some(kind) = kind {
                        index.unique = kind.unique;
                        index.index_type = kind.value().to_string();
                    }
                }
                IndexComboField::Method => index.index_type = value.to_string(),
            }
        }
        self.index_combo = None;
        self.mark_dirty(cx);
    }

    /// Open the 字段 editor for one index row as a draft. The primary key row is editable too: the
    /// model keeps its columns as the 主键 flags on the columns, so its draft edits those flags.
    ///
    /// The list is every column of the table in its own order, with the index's fields ticked — the
    /// shape Navicat uses, which also means a long column name is shown in full instead of being
    /// typed.
    pub(super) fn open_index_fields(
        &mut self,
        row: usize,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.schema.indexes.get(row).is_none() {
            return;
        }
        let fields = self.index_field_rows(row);
        self.index_edit = None;
        self.index_edit_blur = None;
        self.index_combo = None;
        self.index_field_query.clear();
        self.index_field_search_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.select_index(row, cx);
        self.index_fields_draft = Some(IndexFieldsDraft { row, fields });
        self.rebuild_index_field_inputs(cx);
        cx.notify();
    }

    /// The editor's rows for one index: the index's own fields first, in the order the index holds
    /// them, then every other column of the table in its own order. Ticking is what makes a column
    /// a field; the list's order is what the index stores.
    fn index_field_rows(&self, row: usize) -> Vec<IndexFieldRow> {
        let Some(index) = self.schema.indexes.get(row) else {
            return Vec::new();
        };
        let members: Vec<IndexColumnDef> = index.columns.clone();
        let mut fields: Vec<IndexFieldRow> = members
            .iter()
            .map(|member| IndexFieldRow {
                name: member.name.clone(),
                member: true,
                descending: member.descending,
                prefix_length: member.prefix_length.clone(),
            })
            .collect();
        fields.extend(
            self.schema
                .columns
                .iter()
                .filter(|column| !members.iter().any(|member| member.name == column.name))
                .map(|column| IndexFieldRow {
                    name: column.name.clone(),
                    member: false,
                    descending: false,
                    prefix_length: String::new(),
                }),
        );
        fields
    }

    /// 重置: put the draft back to the index's own fields (and its own order).
    pub(super) fn index_fields_reset(&mut self, cx: &mut Context<'_, Self>) {
        let Some(row) = self.index_fields_draft.as_ref().map(|draft| draft.row) else {
            return;
        };
        let fields = self.index_field_rows(row);
        if let Some(draft) = self.index_fields_draft.as_mut() {
            draft.fields = fields;
        }
        self.rebuild_index_field_inputs(cx);
        cx.notify();
    }

    /// Close the field draft without applying it.
    pub(super) fn close_index_fields(&mut self, cx: &mut Context<'_, Self>) {
        if self.index_fields_draft.take().is_some() {
            self.discard_index_field_inputs();
            cx.notify();
        }
    }

    /// Apply the draft: the index takes the draft's field list.
    pub(super) fn confirm_index_fields(&mut self, cx: &mut Context<'_, Self>) {
        let Some(draft) = self.index_fields_draft.take() else {
            return;
        };
        let fields = draft.members();
        if let Some(index) = self.schema.indexes.get_mut(draft.row) {
            index.columns = fields;
        }
        self.discard_index_field_inputs();
        self.mark_dirty(cx);
    }

    /// Drop the draft's per-field controls (`None` draft, or fields changed under them).
    fn discard_index_field_inputs(&mut self) {
        self.index_field_prefix_inputs.clear();
        self.index_field_drag = None;
    }

    /// Bring the per-field controls in line with the draft: one 子部分 editor per field that is part
    /// of the index. The editors are keyed by field name, so a re-order keeps them (and the text the
    /// user may be typing) as they are.
    fn rebuild_index_field_inputs(&mut self, cx: &mut Context<'_, Self>) {
        let Some(draft) = self.index_fields_draft.clone() else {
            self.discard_index_field_inputs();
            return;
        };
        // 子部分 only applies to a field that is part of the index — an unticked row shows an empty
        // cell, so no editor is built for it.
        let has_prefix = self
            .designer
            .index_columns
            .contains(&DesignerIndexColumn::PrefixLength);
        let members: Vec<&IndexFieldRow> = if has_prefix {
            draft.fields.iter().filter(|field| field.member).collect()
        } else {
            Vec::new()
        };
        self.index_field_prefix_inputs
            .retain(|name, _| members.iter().any(|field| &field.name == name));
        let theme = self.theme;
        for field in members {
            if self.index_field_prefix_inputs.contains_key(&field.name) {
                continue;
            }
            let weak = self.self_weak.clone();
            let input = text_input(
                theme,
                field.prefix_length.clone(),
                weak,
                InputTag::IndexPrefix(field.name.clone()),
                cx,
            );
            self.index_field_prefix_inputs
                .insert(field.name.clone(), input);
        }
    }

    /// Tick (or untick) one row's check box, which is what adds or removes the field.
    pub(super) fn toggle_index_draft_member(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        if let Some(draft) = self.index_fields_draft.as_mut()
            && let Some(field) = draft.fields.get_mut(row)
        {
            field.member = !field.member;
        }
        self.rebuild_index_field_inputs(cx);
        cx.notify();
    }

    /// Flip one draft field between `ASC` and `DESC`.
    pub(super) fn toggle_index_draft_sort(&mut self, draft_row: usize, cx: &mut Context<'_, Self>) {
        if let Some(draft) = self.index_fields_draft.as_mut()
            && let Some(field) = draft.fields.get_mut(draft_row)
        {
            field.descending = !field.descending;
        }
        cx.notify();
    }

    fn set_index_draft_prefix(&mut self, field: &str, text: &str, cx: &mut Context<'_, Self>) {
        if let Some(draft) = self.index_fields_draft.as_mut()
            && let Some(row) = draft.fields.iter_mut().find(|row| row.name == field)
        {
            row.prefix_length = text.to_string();
        }
        cx.notify();
    }

    /// Start dragging one field row by its handle. Only the handle starts a drag, so clicking the
    /// row (or its check box) still ticks the field.
    pub(super) fn index_field_drag_start(
        &mut self,
        row: usize,
        start_y: f32,
        cx: &mut Context<'_, Self>,
    ) {
        self.index_field_drag = Some(IndexFieldDrag {
            from: row,
            current: row,
            start_y,
        });
        cx.notify();
    }

    /// Re-order the dragged field as the pointer crosses row boundaries — in the draft only, so 取消
    /// still throws the whole order away.
    pub(super) fn index_field_drag_move(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(drag) = self.index_field_drag else {
            return;
        };
        let Some(draft) = self.index_fields_draft.as_mut() else {
            self.index_field_drag = None;
            return;
        };
        let len = draft.fields.len();
        if len == 0 {
            self.index_field_drag = None;
            return;
        }
        let steps = ((f32::from(event.position.y) - drag.start_y) / DESIGN_ROW_HEIGHT).round();
        let target = (drag.from as isize + steps as isize).clamp(0, len as isize - 1) as usize;
        if target == drag.current {
            return;
        }
        let field = draft.fields.remove(drag.current);
        draft.fields.insert(target, field);
        self.index_field_drag = Some(IndexFieldDrag {
            current: target,
            ..drag
        });
        cx.notify();
    }

    /// Finish a drag.
    pub(super) fn index_field_drag_end(&mut self, cx: &mut Context<'_, Self>) {
        if self.index_field_drag.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn add_foreign_key(&mut self, cx: &mut Context<'_, Self>) {
        let foreign_key = ForeignKeyDef::default();
        self.schema.foreign_keys.push(foreign_key);
        self.selected_fk = Some(self.schema.foreign_keys.len() - 1);
        self.fk_rows.rows.clear();
        self.fk_rows.rows.insert(self.schema.foreign_keys.len() - 1);
        self.fk_rows.anchor = self.selected_fk;
        self.reveal_design_row(self.schema.foreign_keys.len() - 1);
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
                FkColumn::ReferencedSchema => {
                    if foreign_key.referenced_schema != text {
                        // A table in another schema is a different table; its column list no
                        // longer applies.
                        foreign_key.referenced_columns.clear();
                    }
                    foreign_key.referenced_schema = text.to_string();
                }
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
            FkColumn::ReferencedSchema => foreign_key.referenced_schema.clone(),
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
        let schema = foreign_key.referenced_schema.trim().to_string();
        let reference = if table.is_empty() {
            String::new()
        } else if schema.is_empty() {
            table
        } else {
            // SQL Server/PostgreSQL/Oracle resolve a schema-qualified object name; a same-database
            // MySQL reference stays bare (its database is the schema), while a cross-database one
            // is qualified as well.
            format!("{schema}.{table}")
        };
        self.fk_edit = None;
        self.fk_edit_blur = None;
        self.fk_combo = None;
        self.fk_fields_open = false;
        self.select_fk(row, cx);
        self.fk_ref_open = true;
        self.fk_ref_columns.clear();
        if reference.is_empty() {
            cx.notify();
            return;
        }

        let connection = self.connection.clone();
        let database = self.database.clone();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.columns(&database, &reference).await })
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
            DesignDeleteKind::Fields => !self.selected_field_rows().is_empty(),
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
            DesignDeleteKind::UniqueKeys => {
                !self.unique_key_rows.rows.is_empty() || self.selected_unique_key.is_some()
            }
            DesignDeleteKind::Checks => {
                !self.check_rows.rows.is_empty() || self.selected_check.is_some()
            }
            DesignDeleteKind::Excludes => {
                !self.exclude_rows.rows.is_empty() || self.selected_exclude.is_some()
            }
            DesignDeleteKind::Rules => {
                !self.rule_rows.rows.is_empty() || self.selected_rule.is_some()
            }
            DesignDeleteKind::Triggers => {
                !self.trigger_rows.rows.is_empty() || self.selected_trigger.is_some()
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
            DesignDeleteKind::UniqueKeys => self.delete_unique_key(cx),
            DesignDeleteKind::Checks => self.delete_check(cx),
            DesignDeleteKind::Excludes => self.delete_exclude(cx),
            DesignDeleteKind::Rules => self.delete_rule(cx),
            DesignDeleteKind::Triggers => self.delete_trigger(cx),
        }
    }

    /// Append a field — from the toolbar's 添加字段 or the grid's blank placeholder row — and edit
    /// its name straight away, so the blank row the user just created is ready to type into.
    pub(super) fn begin_empty_field(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let row = self.schema.columns.len();
        self.insert_field_at(row, cx);
        self.begin_edit(row, FieldColumn::Name, window, cx);
    }

    /// Append an index — from the toolbar's 添加索引 or the index grid's blank placeholder row —
    /// then edit its name.
    pub(super) fn begin_empty_index(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let row = self.index_row_count();
        self.add_index(cx);
        self.begin_index_edit(row, IndexColumn::Name, window, cx);
    }

    /// Append a foreign key (toolbar's 添加外键 or the grid's blank placeholder row), then edit its
    /// name.
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
        // A brand-new table has no name until the app asks for one on save, but the columns and
        // indexes the user is already defining are worth previewing, so the statement is built
        // against a placeholder name (`new_table`) — it is not stored anywhere and the preview
        // switches to the real name as soon as the create has run.
        let placeholder = t!("design.unnamed_table").to_string();
        let table = if self.table.is_empty() {
            placeholder.as_str()
        } else {
            self.table.as_str()
        };
        self.connection.table_schema_script(
            &self.database,
            table,
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
        let sql = self.connection.table_schema_script(
            &self.database,
            &self.table,
            Some(&original),
            &self.schema,
        );
        if sql.trim().is_empty() {
            // Nothing to apply. 保存 stays enabled even with a clean working copy, so pressing it a
            // second time must not look like a dropped click: close the edit that asked for the save
            // and say there was nothing to do.
            self.dirty = false;
            self.clear_editors();
            self.request_focus(cx);
            if let Some(app) = self.app.upgrade() {
                app.update(cx, |app, cx| {
                    app.toast(ToastKind::Info, t!("design.no_changes").to_string(), cx)
                });
            }
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
                        // The in-place editor that started the save is still open — pressing the
                        // toolbar's 保存 does not move focus off it — and the reload below replaces
                        // the working copy under it, so close the editor first (the text is already
                        // in the working copy; every editor writes on change).
                        view.clear_editors();
                        view.request_focus(cx);
                        view.load_schema(cx);
                    }
                    Err(error) => view.report_error(error.to_string(), cx),
                }
                cx.notify();
            });

            if succeeded && let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| {
                    app.refresh_table_grids(&connection, &database, &table, cx);
                    app.toast(ToastKind::Success, t!("design.saved").to_string(), cx);
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
                        // Same as an existing table's save: close the in-place editor before the
                        // reload replaces the working copy, and keep Ctrl+S working afterwards.
                        view.clear_editors();
                        view.request_focus(cx);
                        view.load_schema(cx);
                    }
                    Err(error) => view.report_error(error.to_string(), cx),
                }
                cx.notify();
            });

            if succeeded && let Some(app) = app.upgrade() {
                app.update(cx, |app, cx| {
                    app.reload_database_tables(&connection, &database, cx);
                    app.toast(ToastKind::Success, t!("design.saved").to_string(), cx);
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
            &self.column_collation_combo,
        ] {
            combo.update(cx, |combo, cx| combo.set_theme(theme, cx));
        }
        cx.notify();
    }
}

impl AppView {
    /// The shape this connection's engine gives the table designer. It comes from the driver's
    /// descriptor through the registry, so the designer never matches on an engine id (and an
    /// unknown driver falls back to the default shape).
    pub(super) fn table_designer(&self, connection_index: usize) -> TableDesignerSpec {
        self.connections
            .get(connection_index)
            .and_then(|node| self.registry.get(&node.profile.driver))
            .map(|driver| driver.descriptor().table_designer)
            .unwrap_or_default()
    }

    /// Open (or focus) the designer page for a table.
    pub(super) fn open_design_table(
        &mut self,
        connection_index: usize,
        database: String,
        table: String,
        is_view: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.driver_supports_table_designer(connection_index) {
            return;
        }
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
        let designer = self.table_designer(connection_index);
        let design_clipboard = self.design_clipboard.clone();
        let entity = cx.new(|cx| {
            TableDesignView::new(
                id,
                connection,
                connection_name,
                database,
                table,
                is_view,
                false,
                designer,
                app,
                runtime,
                theme,
                design_clipboard,
                cx,
            )
        });
        self.designs.push(entity.clone());
        self.active_design = Some(self.designs.len() - 1);
        self.active_grid = None;
        self.active_query = None;
        entity.update(cx, |design, cx| {
            design.load_schema(cx);
            if design.needs_option_catalogs() {
                design.load_option_catalogs(cx);
            }
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
        if !self.driver_supports_table_designer(connection_index) {
            return;
        }
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
        let designer = self.table_designer(connection_index);
        let design_clipboard = self.design_clipboard.clone();
        let entity = cx.new(|cx| {
            TableDesignView::new(
                id,
                connection,
                connection_name,
                database,
                String::new(),
                false,
                true,
                designer,
                app,
                runtime,
                theme,
                design_clipboard,
                cx,
            )
        });
        self.designs.push(entity.clone());
        self.active_design = Some(self.designs.len() - 1);
        self.active_grid = None;
        self.active_query = None;
        // Start with one field so the grid is not empty.
        entity.update(cx, |design, cx| {
            design.add_field(cx);
            if design.needs_option_catalogs() {
                design.load_option_catalogs(cx);
            }
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
        if index.is_some() {
            self.main_tab = MainTab::Tables;
        }
        // The designer's key context only applies while it (or a cell editor inside it) holds focus,
        // so bringing it to the front takes focus for it.
        if let Some(design) = index.and_then(|index| self.designs.get(index)).cloned() {
            design.update(cx, |design, cx| design.request_focus(cx));
        }
        cx.notify();
    }

    /// Close a table designer, asking first when it has unsaved structure changes.
    pub(super) fn close_design(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.designs.len() {
            return;
        }
        if self.designs[index].read(cx).dirty {
            self.unsaved_confirm = Some(PendingAction::CloseDesign { index });
            cx.notify();
            return;
        }
        self.close_design_now(index, cx);
    }

    pub(super) fn close_design_now(&mut self, index: usize, cx: &mut Context<'_, Self>) {
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
        self.settle_after_tab_closed(cx);
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

    /// Close the table/routine/view designers opened from one database.
    pub(super) fn close_database_designs(
        &mut self,
        connection: &Arc<dyn Connection>,
        database: &str,
        cx: &App,
    ) {
        let active_id = self
            .active_design
            .and_then(|index| self.designs.get(index))
            .map(|design| design.read(cx).id);
        self.designs.retain(|design| {
            let design = design.read(cx);
            !(design.database == database && Arc::ptr_eq(&design.connection, connection))
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The MySQL field grid's chain: 列名 → 类型 → 长度 → 小数位 → 备注.
    fn chain() -> Vec<FieldCell> {
        vec![
            FieldCell::Name,
            FieldCell::Type,
            FieldCell::Text(FieldColumn::Length),
            FieldCell::Text(FieldColumn::Decimals),
            FieldCell::Text(FieldColumn::Comment),
        ]
    }

    /// The designer's labels must share one left edge: the sub-tab strip's first tab (字段) and the
    /// grid's first column — its header (列名), its values and the in-place editor a cell swaps in.
    ///
    /// The elements hereby mirror the two chains that build them, `render_subtabs` (the strip's
    /// `px_2` padding, a tab's 1px left border and its `px_4` padding) and the grid's header/value
    /// cells (the row-header gutter plus `DESIGN_CELL_INSET`), so a change on either side that
    /// forgets `DESIGN_LABEL_INSET` fails here.
    #[gpui_kit::test]
    fn the_sub_tab_label_and_the_grid_labels_share_one_left_edge(cx: &mut gpui::TestAppContext) {
        use gpui::prelude::*;
        use gpui::{Bounds, IntoElement, Point, Render, Window, WindowBounds, WindowOptions, size};
        use gpui_kit::TestSupportExt as _;
        use gpui_kit::test::TestWindowExt as _;

        struct Probe;
        impl Render for Probe {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                let theme = Theme::light();
                div()
                    .size_full()
                    .bg(rgb(theme.editor_bg))
                    .text_color(rgb(theme.text))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_end()
                            .gap_1()
                            .h(px(26.0))
                            .flex_none()
                            .px_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .px_4()
                                    .h(px(24.0))
                                    .text_size(px(12.0))
                                    .border_t_1()
                                    .border_l_1()
                                    .border_r_1()
                                    .child(div().id("tab-label").test_support().child("字段")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_none()
                            .h(px(DESIGN_HEADER_HEIGHT))
                            .w_full()
                            .child(
                                div()
                                    .w(px(DESIGN_GUTTER_WIDTH))
                                    .h(px(DESIGN_HEADER_HEIGHT))
                                    .flex_none()
                                    .border_r_1(),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .h(px(DESIGN_HEADER_HEIGHT))
                                    .w(px(200.0))
                                    .flex_none()
                                    .px(px(DESIGN_CELL_INSET))
                                    .child(div().id("head-label").test_support().child("列名")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_none()
                            .h(px(DESIGN_ROW_HEIGHT))
                            .child(
                                div()
                                    .w(px(DESIGN_GUTTER_WIDTH))
                                    .h(px(DESIGN_ROW_HEIGHT))
                                    .flex_none()
                                    .border_r_1(),
                            )
                            .child(
                                div()
                                    .id("value-cell")
                                    .flex()
                                    .items_center()
                                    .h(px(DESIGN_ROW_HEIGHT))
                                    .w(px(200.0))
                                    .flex_none()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .border_r_1()
                                    .border_b_1()
                                    .pl(px(DESIGN_CELL_INSET))
                                    .pr(px(4.0))
                                    .child(div().id("value-label").test_support().child("id")),
                            ),
                    )
            }
        }

        cx.update(gpui_kit::init);
        let (handle, _) = cx.update(|cx| {
            let probe = cx.new(|_| Probe);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(800.0), px(600.0)),
                    })),
                    ..Default::default()
                },
                cx,
                |_, _| probe.clone(),
            )
            .expect("open test window")
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        let (tab_x, head_x, value_x) = cx
            .update_window(handle, |_, window, _cx| {
                (
                    f32::from(window.find("tab-label").bounds().origin.x),
                    f32::from(window.find("head-label").bounds().origin.x),
                    f32::from(window.find("value-label").bounds().origin.x),
                )
            })
            .unwrap();
        assert_eq!(tab_x, DESIGN_LABEL_INSET, "the first sub-tab's label");
        assert_eq!(head_x, DESIGN_LABEL_INSET, "a column's header label");
        assert_eq!(value_x, DESIGN_LABEL_INSET, "a column's value");
    }

    #[test]
    fn tab_walks_the_row_then_wraps_to_the_next_row() {
        let chain = chain();
        // 列名 → 类型: the detail panel must not take the caret before the row's own cells have.
        assert_eq!(
            field_cell_neighbor(&chain, 0, 3, FieldCell::Name, false),
            Some((0, FieldCell::Type))
        );
        assert_eq!(
            field_cell_neighbor(&chain, 0, 3, FieldCell::Text(FieldColumn::Length), false),
            Some((0, FieldCell::Text(FieldColumn::Decimals)))
        );
        // The row's last cell rolls onto the next row's 列名 …
        assert_eq!(
            field_cell_neighbor(&chain, 0, 3, FieldCell::Text(FieldColumn::Comment), false),
            Some((1, FieldCell::Name))
        );
        // … and the grid's very last cell hands the keystroke back to the window.
        assert_eq!(
            field_cell_neighbor(&chain, 2, 3, FieldCell::Text(FieldColumn::Comment), false),
            None
        );
    }

    #[test]
    fn shift_tab_walks_backwards_and_stops_at_the_first_cell() {
        let chain = chain();
        assert_eq!(
            field_cell_neighbor(&chain, 1, 3, FieldCell::Name, true),
            Some((0, FieldCell::Text(FieldColumn::Comment)))
        );
        assert_eq!(
            field_cell_neighbor(&chain, 0, 3, FieldCell::Text(FieldColumn::Comment), true),
            Some((0, FieldCell::Text(FieldColumn::Decimals)))
        );
        assert_eq!(
            field_cell_neighbor(&chain, 0, 3, FieldCell::Name, true),
            None
        );
    }

    #[test]
    fn tab_ignores_a_cell_that_is_not_in_the_chain() {
        let chain = chain();
        // A single-row grid has nothing to wrap onto.
        assert_eq!(
            field_cell_neighbor(&chain, 0, 1, FieldCell::Text(FieldColumn::Comment), false),
            None
        );
        // A cell the engine does not show (its field list has no 备注 column) is not a Tab stop.
        assert_eq!(
            field_cell_neighbor(
                &[FieldCell::Name, FieldCell::Type],
                0,
                2,
                FieldCell::Text(FieldColumn::Comment),
                false
            ),
            None
        );
    }

    #[test]
    fn the_type_highlight_wraps_and_clamps_to_a_narrowed_list() {
        assert_eq!(next_type_highlight(0, Some(0), true), None);
        assert_eq!(next_type_highlight(3, Some(2), true), Some(0));
        assert_eq!(next_type_highlight(3, Some(0), false), Some(2));
        assert_eq!(next_type_highlight(3, Some(1), true), Some(2));
        // The list shrank under the highlight (the user typed): the index is clamped to the list, so
        // Down wraps to its top and Up steps to the row before its last.
        assert_eq!(next_type_highlight(2, Some(7), true), Some(0));
        assert_eq!(next_type_highlight(2, Some(7), false), Some(0));
    }

    #[test]
    fn a_group_move_shifts_every_selected_field_one_slot() {
        assert_eq!(moved_field_rows(&[2, 3], 5, -1), Some(vec![1, 2]));
        assert_eq!(moved_field_rows(&[1, 3], 5, 1), Some(vec![2, 4]));
        // Against the edge the whole group stays put instead of moving part of itself.
        assert_eq!(moved_field_rows(&[0, 3], 5, -1), None);
        assert_eq!(moved_field_rows(&[1, 4], 5, 1), None);
        assert_eq!(moved_field_rows(&[], 5, 1), None);
    }

    #[test]
    fn a_group_move_swaps_the_selected_columns() {
        let mut columns: Vec<ColumnDef> = ["id", "password", "uid", "sex", "avatar"]
            .into_iter()
            .map(|name| ColumnDef {
                name: name.to_string(),
                ..ColumnDef::default()
            })
            .collect();
        // uid / sex / avatar move above password, keeping their own order.
        move_columns(&mut columns, &[2, 3, 4], -1);
        let names: Vec<&str> = columns.iter().map(|column| column.name.as_str()).collect();
        assert_eq!(names, ["id", "uid", "sex", "avatar", "password"]);

        move_columns(&mut columns, &[1, 2, 3], 1);
        let names: Vec<&str> = columns.iter().map(|column| column.name.as_str()).collect();
        assert_eq!(names, ["id", "password", "uid", "sex", "avatar"]);
    }

    #[test]
    fn a_cell_selection_reports_the_field_rows_it_touches() {
        let selection = CellSelection::single((2, 1), (4, 1));
        assert_eq!(selection.row_indices(), vec![2, 3, 4]);
        assert!(selection.contains(3, 1));
        assert!(!selection.contains(3, 0));
    }
}
