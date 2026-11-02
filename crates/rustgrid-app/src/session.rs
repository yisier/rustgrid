use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui::ScrollHandle;
use rustgrid_core::{
    CellValue, ColumnInfo, Connection, ConnectionProfile, DriverDialect, FilterCondition,
    FilterConjunction, FilterNode, FilterOperator, QueryResult, RoutineDetails, RoutineInfo,
    RoutineKind, SortColumn, TableInfo, TableStatus, ViewDetails,
};

#[derive(Default)]
pub enum Loadable<T> {
    #[default]
    Idle,
    Loading,
    Loaded(T),
    Failed(String),
}

pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected(Arc<dyn Connection>),
    Failed(String),
}

pub struct ConnectionNode {
    pub profile: ConnectionProfile,
    pub password: Option<String>,
    pub password_saved: bool,
    pub status: ConnectionStatus,
    pub databases: Loadable<Vec<DatabaseNode>>,
    pub expanded: bool,
}

pub struct DatabaseNode {
    pub name: String,
    pub tables: Loadable<Vec<TableInfo>>,
    /// The database's `SHOW TABLE STATUS`-style table overviews, for the Tables 详细列表.
    pub table_statuses: Loadable<Vec<(String, TableStatus)>>,
    /// The database's stored routines (functions and procedures), loaded lazily when the
    /// Functions tab or the connection tree's Functions category needs them.
    pub routines: Loadable<Vec<RoutineInfo>>,
    pub opened: bool,
    pub expanded: bool,
    pub categories: CategoryExpansion,
    /// The schemas the connection tree has expanded (SQL Server). Empty for schema-less
    /// engines, where the tree renders the database's categories directly.
    pub expanded_schemas: BTreeSet<String>,
    /// The schemas the user has opened (SQL Server): their tree icon is highlighted and their
    /// objects are shown in the content pane. Empty for schema-less engines.
    pub opened_schemas: BTreeSet<String>,
    /// The database's schemas, loaded when it is opened so a freshly created (still empty) schema
    /// is visible in the tree. `None` for schema-less engines.
    pub schemas: Option<Loadable<Vec<String>>>,
    /// The next object page's cursor for a key-value engine, when one is left over.
    pub object_cursor: Option<String>,
    /// The namespace filter (`user:*`) applied to the object list, if any.
    pub object_pattern: Option<String>,
    /// The key-value namespaces (key prefixes) of this database, with their key counts.
    pub namespaces: Loadable<Vec<rustgrid_core::ObjectNamespace>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Tables,
    Views,
    Functions,
    Queries,
    Backups,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Tables,
        Category::Views,
        Category::Functions,
        Category::Queries,
        Category::Backups,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Tables => "common.tables",
            Category::Views => "common.views",
            Category::Functions => "common.functions",
            Category::Queries => "common.queries",
            Category::Backups => "common.backups",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Category::Tables => "t",
            Category::Views => "v",
            Category::Functions => "f",
            Category::Queries => "q",
            Category::Backups => "b",
        }
    }

    pub fn icon_path(self) -> &'static str {
        match self {
            Category::Tables => "icons/tables.svg",
            Category::Views => "icons/views.svg",
            Category::Functions => "icons/functions.svg",
            Category::Queries => "icons/queries.svg",
            Category::Backups => "icons/backups.svg",
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct CategoryExpansion {
    pub tables: bool,
    pub views: bool,
    pub functions: bool,
    pub queries: bool,
    pub backups: bool,
}

impl CategoryExpansion {
    pub fn get(&self, category: Category) -> bool {
        match category {
            Category::Tables => self.tables,
            Category::Views => self.views,
            Category::Functions => self.functions,
            Category::Queries => self.queries,
            Category::Backups => self.backups,
        }
    }

    pub fn toggle(&mut self, category: Category) {
        let slot = match category {
            Category::Tables => &mut self.tables,
            Category::Views => &mut self.views,
            Category::Functions => &mut self.functions,
            Category::Queries => &mut self.queries,
            Category::Backups => &mut self.backups,
        };
        *slot = !*slot;
    }
}

/// One rectangular block of cells (a drag/shift selection).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellRange {
    pub anchor: (usize, usize),
    pub cursor: (usize, usize),
}

impl CellRange {
    pub fn new(row: usize, col: usize) -> Self {
        Self {
            anchor: (row, col),
            cursor: (row, col),
        }
    }

    pub fn rows(&self) -> (usize, usize) {
        (
            self.anchor.0.min(self.cursor.0),
            self.anchor.0.max(self.cursor.0),
        )
    }

    pub fn cols(&self) -> (usize, usize) {
        (
            self.anchor.1.min(self.cursor.1),
            self.anchor.1.max(self.cursor.1),
        )
    }

    pub fn contains(&self, row: usize, col: usize) -> bool {
        let (start_row, end_row) = self.rows();
        let (start_col, end_col) = self.cols();
        row >= start_row && row <= end_row && col >= start_col && col <= end_col
    }
}

/// An Excel-like selection: one or more rectangular ranges. `active` indexes the range that a
/// shift-click or drag extends, and whose `cursor` is the "current" cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellSelection {
    pub ranges: Vec<CellRange>,
    pub active: usize,
}

impl CellSelection {
    pub fn new(row: usize, col: usize) -> Self {
        Self {
            ranges: vec![CellRange::new(row, col)],
            active: 0,
        }
    }

    pub fn single(anchor: (usize, usize), cursor: (usize, usize)) -> Self {
        Self {
            ranges: vec![CellRange { anchor, cursor }],
            active: 0,
        }
    }

    /// The range that shift/drag extends.
    pub fn active(&self) -> &CellRange {
        &self.ranges[self.active.min(self.ranges.len().saturating_sub(1))]
    }

    pub fn active_cursor(&self) -> (usize, usize) {
        self.active().cursor
    }

    pub fn contains(&self, row: usize, col: usize) -> bool {
        self.ranges.iter().any(|range| range.contains(row, col))
    }

    /// Overall bounding rows across every range (used for status/headers).
    pub fn rows(&self) -> (usize, usize) {
        self.ranges
            .iter()
            .fold((usize::MAX, 0), |(min, max), range| {
                let (start, end) = range.rows();
                (min.min(start), max.max(end))
            })
    }

    /// Overall bounding columns across every range.
    pub fn cols(&self) -> (usize, usize) {
        self.ranges
            .iter()
            .fold((usize::MAX, 0), |(min, max), range| {
                let (start, end) = range.cols();
                (min.min(start), max.max(end))
            })
    }

    /// Remove `span` from every range, splitting a range the span cuts through into the pieces
    /// around it. Returns whether any cell was actually removed. Ctrl+click uses this so a click
    /// inside 全选's single big range deselects just the clicked cell (like Excel) instead of
    /// dropping the whole selection.
    pub fn subtract(&mut self, span: CellRange) -> bool {
        let (span_start_row, span_end_row) = span.rows();
        let (span_start_col, span_end_col) = span.cols();
        let mut pieces = Vec::new();
        let mut changed = false;
        for range in &self.ranges {
            let (start_row, end_row) = range.rows();
            let (start_col, end_col) = range.cols();
            let hit_start_row = start_row.max(span_start_row);
            let hit_end_row = end_row.min(span_end_row);
            let hit_start_col = start_col.max(span_start_col);
            let hit_end_col = end_col.min(span_end_col);
            if hit_start_row > hit_end_row || hit_start_col > hit_end_col {
                pieces.push(*range);
                continue;
            }
            changed = true;
            if start_row < hit_start_row {
                pieces.push(CellRange {
                    anchor: (start_row, start_col),
                    cursor: (hit_start_row - 1, end_col),
                });
            }
            if hit_end_row < end_row {
                pieces.push(CellRange {
                    anchor: (hit_end_row + 1, start_col),
                    cursor: (end_row, end_col),
                });
            }
            if start_col < hit_start_col {
                pieces.push(CellRange {
                    anchor: (hit_start_row, start_col),
                    cursor: (hit_end_row, hit_start_col - 1),
                });
            }
            if hit_end_col < end_col {
                pieces.push(CellRange {
                    anchor: (hit_start_row, hit_end_col + 1),
                    cursor: (hit_end_row, end_col),
                });
            }
        }
        if changed {
            self.ranges = pieces;
            if self.ranges.is_empty() {
                self.active = 0;
            } else {
                self.active = self.active.min(self.ranges.len() - 1);
            }
        }
        changed
    }

    /// Every selected cell, de-duplicated across ranges.
    pub fn cells(&self) -> Vec<(usize, usize)> {
        let mut cells: Vec<(usize, usize)> = Vec::new();
        for range in &self.ranges {
            let (start_row, end_row) = range.rows();
            let (start_col, end_col) = range.cols();
            for row in start_row..=end_row {
                for col in start_col..=end_col {
                    if !cells.contains(&(row, col)) {
                        cells.push((row, col));
                    }
                }
            }
        }
        cells
    }

    /// The distinct row indices touched by any range, sorted ascending.
    pub fn row_indices(&self) -> Vec<usize> {
        let mut rows = std::collections::BTreeSet::new();
        for range in &self.ranges {
            let (start, end) = range.rows();
            for row in start..=end {
                rows.insert(row);
            }
        }
        rows.into_iter().collect()
    }

    /// The distinct column indices touched by any range, sorted ascending.
    pub fn col_indices(&self) -> Vec<usize> {
        let mut cols = std::collections::BTreeSet::new();
        for range in &self.ranges {
            let (start, end) = range.cols();
            for col in start..=end {
                cols.insert(col);
            }
        }
        cols.into_iter().collect()
    }
}

/// Which sub-tab of a routine editor is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RoutineTab {
    /// The editable `CREATE ...` statement.
    Definition,
    /// Read-only metadata.
    Info,
    /// The SQL the Save button runs.
    Sql,
}

impl RoutineTab {
    /// Every sub-tab, in the order the editor presents them.
    pub const ALL: [RoutineTab; 3] = [RoutineTab::Definition, RoutineTab::Info, RoutineTab::Sql];

    /// The i18n key for the sub-tab's label.
    pub fn label_key(self) -> &'static str {
        match self {
            RoutineTab::Definition => "routine.tab.definition",
            RoutineTab::Info => "routine.tab.info",
            RoutineTab::Sql => "routine.tab.sql",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            RoutineTab::Definition => "routine-tab-definition",
            RoutineTab::Info => "routine-tab-info",
            RoutineTab::Sql => "routine-tab-sql",
        }
    }
}

/// The routine-editor state carried by a [`QueryTab`] whose `routine` is set. A routine editor
/// reuses the SQL editor (and its tab) but adds the routine's identity, metadata and the
/// 定义/信息/SQL 预览 sub-tabs.
pub struct RoutineTabState {
    pub kind: RoutineKind,
    /// The routine's current name (the name Save will create).
    pub name: String,
    /// The routine's name in the database, `None` until a new routine is saved.
    pub original_name: Option<String>,
    pub original_kind: Option<RoutineKind>,
    /// The routine's full loaded state (metadata + session settings), for the 信息 tab.
    pub details: Option<RoutineDetails>,
    pub tab: RoutineTab,
    /// Whether the definition editor wraps long lines.
    pub word_wrap: bool,
    /// Whether the find bar is showing.
    pub find_open: bool,
    pub find_query: String,
    /// A save is in flight.
    pub saving: bool,
}

impl RoutineTabState {
    pub fn new(kind: RoutineKind, name: String) -> Self {
        Self {
            kind,
            name,
            original_name: None,
            original_kind: None,
            details: None,
            tab: RoutineTab::Definition,
            word_wrap: true,
            find_open: false,
            find_query: String::new(),
            saving: false,
        }
    }
}

/// Which sub-tab of a view designer is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewTab {
    /// The editable `CREATE ... VIEW` statement.
    Definition,
    /// Read-only creation settings and metadata.
    Advanced,
    /// The SQL the Save button runs.
    Sql,
}

impl ViewTab {
    /// Every sub-tab, in the order the designer presents them.
    pub const ALL: [ViewTab; 3] = [ViewTab::Definition, ViewTab::Advanced, ViewTab::Sql];

    /// The i18n key for the sub-tab's label.
    pub fn label_key(self) -> &'static str {
        match self {
            ViewTab::Definition => "view.tab.definition",
            ViewTab::Advanced => "view.tab.advanced",
            ViewTab::Sql => "view.tab.sql",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            ViewTab::Definition => "view-tab-definition",
            ViewTab::Advanced => "view-tab-advanced",
            ViewTab::Sql => "view-tab-sql",
        }
    }
}

/// Which sub-tab of the view designer's bottom explain panel is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewExplainTab {
    /// The executed `EXPLAIN` statement and its status.
    Info,
    /// The `EXPLAIN` result table.
    Result,
}

impl ViewExplainTab {
    /// The i18n key for the sub-tab's label.
    pub fn label_key(self) -> &'static str {
        match self {
            ViewExplainTab::Info => "view.explain.tab.info",
            ViewExplainTab::Result => "view.explain.tab.result",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            ViewExplainTab::Info => "view-explain-tab-info",
            ViewExplainTab::Result => "view-explain-tab-result",
        }
    }
}

/// The view-designer state carried by a [`QueryTab`] whose `view` is set. A view designer reuses
/// the SQL editor (and its tab) but adds the view's identity, metadata and the
/// 定义/高级/SQL 预览 sub-tabs.
#[derive(Clone)]
pub struct ViewTabState {
    /// The view's current name (the name Save will create).
    pub name: String,
    /// The view's name in the database, `None` until a new view is saved.
    pub original_name: Option<String>,
    /// The view's full loaded state (metadata + definition), for the 高级 tab.
    pub details: Option<ViewDetails>,
    pub tab: ViewTab,
    /// A save is in flight.
    pub saving: bool,

    /// Whether the bottom 信息/解释 panel is showing.
    pub explain_open: bool,
    /// Which bottom panel sub-tab is active.
    pub explain_tab: ViewExplainTab,
    /// The `EXPLAIN` statement that was run (shown on the 信息 tab).
    pub explain_sql: String,
    /// An explain run is in flight.
    pub explain_running: bool,
    pub explain_elapsed: Option<std::time::Duration>,
    pub explain_error: Option<String>,
    /// The id of the grid in `AppView::grids` showing the explain plan, if any.
    pub explain_grid_id: Option<u64>,
}

impl ViewTabState {
    pub fn new(name: String) -> Self {
        Self {
            name,
            original_name: None,
            details: None,
            tab: ViewTab::Definition,
            saving: false,
            explain_open: false,
            explain_tab: ViewExplainTab::Info,
            explain_sql: String::new(),
            explain_running: false,
            explain_elapsed: None,
            explain_error: None,
            explain_grid_id: None,
        }
    }
}

/// The query result panel's tab strip (信息 plus one 结果 tag per result set): its horizontal
/// scroll, and whether the last layout found more tabs than fit. When they do not fit, the strip
/// shows ◀ / ▶ paging buttons (see `AppView::render_query_result_tabs`), like the main tab bar.
#[derive(Default)]
pub struct QueryResultTabs {
    pub scroll: ScrollHandle,
    /// Written by the strip's measurement canvas after each layout and read on the next render.
    pub overflow: Rc<Cell<bool>>,
}

/// An open SQL editor tab. `sql` is the editable document; `caret` and `anchor` are byte
/// offsets into it, so they can be matched against `TextLayout` indices directly.
pub struct QueryTab {
    pub id: u64,
    /// The saved query name, once this tab was saved to (or opened from) the saved-query list.
    pub name: Option<String>,
    /// The `.sql` file this tab is bound to, once it was saved (or opened from the list). While this
    /// is set, Save overwrites the file in place instead of asking for a name and location again.
    pub saved_path: Option<std::path::PathBuf>,
    pub connection_index: Option<usize>,
    pub database: Option<String>,
    /// The schema the editor is scoped to (SQL Server). `None` leaves unqualified objects to the
    /// engine's default schema; it filters completion and resolves unqualified table references.
    pub schema: Option<String>,
    pub sql: String,
    /// The text as last saved/loaded. `sql != baseline` means there are unsaved edits, which
    /// drives the tab's dirty marker and the close confirmation.
    pub baseline: String,
    pub caret: usize,
    pub anchor: usize,
    pub selecting: bool,
    pub running: bool,
    /// Bumped on every run and on stop. A late result whose generation no longer matches the
    /// tab's is discarded, so "Stop" is not silently overwritten by the query finishing.
    pub run_generation: u64,
    /// One entry per statement the last run executed, in order. A lightweight summary (not the
    /// rows), so the 信息 tab can report each statement without duplicating the grid's data.
    pub results: Vec<QueryResultSummary>,
    /// The grid id in `AppView::grids` for each `results` entry (`None` for a statement that
    /// produced no result set).
    pub result_grids: Vec<Option<u64>>,
    /// The bottom result panel's active tab: `0` is 信息, `1..=n` is the `n`-th result set.
    pub active_result: usize,
    /// The result tab strip's scroll state (it pages with ◀ / ▶ when the tabs do not fit).
    pub result_tabs: QueryResultTabs,
    /// The last run's error, if it failed.
    pub result_error: Option<String>,
    /// The script that was run, shown on the 信息 tab.
    pub last_sql: String,
    /// How long the last run took, shown on the 信息 tab and the toolbar.
    pub last_elapsed: Option<std::time::Duration>,
    /// When the running query started, for the toolbar's live timer.
    pub run_started: Option<std::time::Instant>,
    /// The text before and after the last 美化SQL, so formatting again with the formatted text
    /// untouched restores the original.
    pub format_undo: Option<(String, String)>,
    /// Statements the last run skipped because of the statement limit (the 信息 tab says so).
    pub skipped_statements: usize,
    /// Set by Stop: the running script runs no further statement.
    pub run_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The id of the grid in `AppView::grids` that shows this tab's active result set, if any.
    pub grid_id: Option<u64>,
    /// `Some` when this tab edits a stored routine instead of a free-form query.
    pub routine: Option<RoutineTabState>,
    /// `Some` when this tab designs a database view instead of a free-form query.
    pub view: Option<ViewTabState>,
}

impl QueryTab {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            name: None,
            saved_path: None,
            connection_index: None,
            database: None,
            schema: None,
            sql: String::new(),
            baseline: String::new(),
            caret: 0,
            anchor: 0,
            selecting: false,
            running: false,
            run_generation: 0,
            results: Vec::new(),
            result_grids: Vec::new(),
            active_result: 0,
            result_tabs: QueryResultTabs::default(),
            result_error: None,
            last_sql: String::new(),
            last_elapsed: None,
            run_started: None,
            run_cancel: None,
            skipped_statements: 0,
            format_undo: None,
            grid_id: None,
            routine: None,
            view: None,
        }
    }

    /// The byte range of the current selection, normalised to `start..end`.
    pub fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// Whether this tab has edits that have not been saved (or that were never persisted).
    pub fn is_dirty(&self) -> bool {
        self.sql != self.baseline
    }
}

/// A result set from a query run, with the editability and target table resolved from the
/// statement it came from. Built on the async side (where the catalog lookup happens) and handed to
/// `AppView::apply_query_results` to create one grid per entry.
pub struct QueryResultPlan {
    pub result: QueryResult,
    pub editable: bool,
    /// The database the grid's edits should target (the statement's schema, else the query's).
    pub database: String,
    /// The single table inferred from the statement, empty when it is not editable.
    pub table: String,
}

/// A cheap snapshot of one executed statement, enough for the 信息 tab. Kept separate from
/// [`QueryResult`] so the rows are not held twice (the grid owns them).
#[derive(Clone)]
pub struct QueryResultSummary {
    /// The statement this entry reports on, as sent.
    pub statement: String,
    /// How long that statement took.
    pub elapsed: std::time::Duration,
    /// The statement's error, when it failed (then there is no result set).
    pub error: Option<String>,
    pub has_result_set: bool,
    pub row_count: usize,
    pub rows_affected: u64,
}

/// One entry of a query run, built on the async side: a statement's result set (or its
/// outcome), or its error. A statement that returned several result sets yields several entries.
pub struct RunEntry {
    pub statement: String,
    pub elapsed: std::time::Duration,
    pub outcome: Result<QueryResultPlan, String>,
}

/// What a run executes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunScope {
    /// The whole editor.
    All,
    /// The selection when there is one, else the whole editor (Navicat's Run, Ctrl+R).
    SelectionOrAll,
    /// The selection when there is one, else the statement at the caret (Run Selected).
    SelectionOrStatement,
}

/// One undo step: the touched cells and their previous pending edit value
/// (`None` means there was no pending edit for that cell).
pub type EditAction = Vec<((usize, usize), Option<Option<String>>)>;

/// One row of the sort criteria panel: which column, which direction, and whether it is
/// currently part of the query's `ORDER BY`.
#[derive(Clone, PartialEq, Eq)]
pub struct SortRule {
    pub column: String,
    pub descending: bool,
    pub enabled: bool,
}

impl SortRule {
    pub fn new(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            descending: false,
            enabled: true,
        }
    }
}

/// A table grid's field layout: the order every field is displayed in, and which of them are
/// hidden. Applied by projecting `GridState::columns`/`column_widths` out of the table's natural
/// column order; `GridState::rows` stay in natural order, so changing the layout re-projects
/// metadata only (no refetch and no row copy).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct GridFieldLayout {
    /// Every field name in display order (fields the layout does not know are appended).
    pub order: Vec<String>,
    /// The hidden field names.
    pub hidden: BTreeSet<String>,
}

impl GridFieldLayout {
    /// The layout's display order resolved against the engine's natural `columns`: the layout's own
    /// order first (restricted to fields the table actually has), then every remaining field in
    /// table order.
    pub fn resolved_order(&self, columns: &[ColumnInfo]) -> Vec<String> {
        let mut order: Vec<String> = Vec::with_capacity(columns.len());
        for name in &self.order {
            if !order.contains(name) && columns.iter().any(|column| &column.name == name) {
                order.push(name.clone());
            }
        }
        for column in columns {
            if !order.contains(&column.name) {
                order.push(column.name.clone());
            }
        }
        order
    }

    /// Whether the layout leaves the table exactly as the engine returns it: no field hidden and no
    /// field out of place. Drives the toolbar icon's "configured" state.
    pub fn is_default(&self, columns: &[ColumnInfo]) -> bool {
        if !self.hidden.is_empty() {
            return false;
        }
        if self.order.is_empty() {
            return true;
        }
        self.order.len() == columns.len()
            && self
                .order
                .iter()
                .zip(columns)
                .all(|(name, column)| *name == column.name)
    }

    /// The permutation from display position to the index into `columns` (natural order), with the
    /// visible fields first, and how many leading entries are visible. A layout that would hide
    /// every field falls back to the table's own order so a grid can never end up blank.
    pub fn permutation(&self, columns: &[ColumnInfo]) -> (Vec<usize>, usize) {
        let order = self.resolved_order(columns);
        let mut visible: Vec<usize> = Vec::with_capacity(columns.len());
        let mut hidden: Vec<usize> = Vec::new();
        for name in &order {
            let Some(index) = columns.iter().position(|column| &column.name == name) else {
                continue;
            };
            if self.hidden.contains(name) {
                hidden.push(index);
            } else {
                visible.push(index);
            }
        }
        if visible.is_empty() {
            return ((0..columns.len()).collect(), columns.len());
        }
        let visible_count = visible.len();
        visible.extend(hidden);
        (visible, visible_count)
    }

    /// Show or hide one field.
    pub fn set_hidden(&mut self, name: &str, hidden: bool) {
        if hidden {
            self.hidden.insert(name.to_string());
        } else {
            self.hidden.remove(name);
        }
    }

    /// Move the field at `from` to `to` in the resolved display order.
    pub fn move_field(&mut self, order: &[String], from: usize, to: usize) {
        if from == to || from >= order.len() || to >= order.len() {
            return;
        }
        let mut next = order.to_vec();
        let name = next.remove(from);
        next.insert(to.min(next.len()), name);
        self.order = next;
    }

    /// Flip every field's visibility (反选). The inversion never hides the last visible field: when
    /// every field was shown the first one stays, so a grid can never end up with no columns.
    pub fn invert_visibility(&mut self) {
        if self.order.len() <= 1 {
            return;
        }
        let mut inverted: BTreeSet<String> = self
            .order
            .iter()
            .filter(|name| !self.hidden.contains(*name))
            .cloned()
            .collect();
        if inverted.len() >= self.order.len() {
            inverted.remove(&self.order[0]);
        }
        self.hidden = inverted;
    }
}

/// Apply `order` (the display position each element's *source* index belongs at) to `values` in
/// place, walking the permutation's cycles so no element is copied. A non-permutation is ignored,
/// which also keeps the cycle walk from looping forever.
pub fn permute_in_place<T>(values: &mut [T], order: &[usize]) {
    if values.len() != order.len() {
        return;
    }
    // The cycle walk needs where each element goes, i.e. the inverse of `order`.
    let mut dest = vec![usize::MAX; order.len()];
    for (slot, source) in order.iter().enumerate() {
        if *source >= order.len() || dest[*source] != usize::MAX {
            return;
        }
        dest[*source] = slot;
    }
    for index in 0..dest.len() {
        while dest[index] != index {
            let other = dest[index];
            values.swap(index, other);
            dest.swap(index, other);
        }
    }
}

/// The permutation to apply to the currently displayed columns (whose display order is
/// `current_order`, each entry indexing the table's natural columns) to put them into
/// `next_order`. Composing through the current positions is what lets `reproject_fields` re-order
/// `columns`/`column_widths` in place without knowing which field each entry is.
fn field_permutation(current_order: &[usize], next_order: &[usize], count: usize) -> Vec<usize> {
    let mut position = vec![0usize; count];
    for (slot, source) in current_order.iter().enumerate() {
        if *source < count {
            position[*source] = slot;
        }
    }
    next_order
        .iter()
        .map(|source| position.get(*source).copied().unwrap_or(*source))
        .collect()
}

pub struct GridState {
    pub id: u64,
    pub connection: Arc<dyn Connection>,
    /// The engine's SQL dialect, for the status line's shown statement (identifier quoting and
    /// paging syntax), so it reads the engine's own SQL instead of MySQL's.
    pub dialect: DriverDialect,
    pub connection_name: String,
    /// The connection profile's id, which keys the persisted field layout. Empty for a grid whose
    /// columns are not a table's (a query result), so no field layout is applied to it.
    pub connection_id: String,
    pub database: String,
    pub table: String,
    pub is_view: bool,
    /// The key-value object's overview (type / TTL / memory), when this grid shows a key. `None`
    /// for SQL tables and query results.
    pub key_status: Option<rustgrid_core::TableStatus>,
    pub page_index: u64,
    pub page_size: u64,
    pub loading: bool,
    pub error: Option<String>,
    /// The displayed columns: the table's columns re-ordered by [`Self::field_layout`], with the
    /// hidden ones kept at the tail so unhiding them needs no refetch.
    pub columns: Vec<ColumnInfo>,
    /// The table's columns exactly as the engine returns them (natural order). The 字段筛选 popup
    /// lists these, and a layout change is resolved against them.
    pub all_columns: Vec<ColumnInfo>,
    /// The applied field layout (display order plus the hidden set).
    pub field_layout: GridFieldLayout,
    /// Display position -> index into [`Self::all_columns`], for the whole column list (hidden
    /// fields included).
    pub column_order: Vec<usize>,
    /// How many leading entries of [`Self::columns`] are visible; the rest are hidden fields.
    pub visible_columns: usize,
    pub rows: Arc<Vec<Vec<CellValue>>>,
    pub column_widths: Vec<f32>,
    /// Set once the user drags a column edge. Keeps the widths for the rest of this grid's life
    /// so paging/refreshing does not snap them back to the auto-fitted values.
    pub manual_column_widths: bool,
    pub total_rows: Option<u64>,
    pub selection: Option<CellSelection>,
    pub edits: BTreeMap<(usize, usize), Option<String>>,
    /// Undo history: each entry restores the prior edit value (or lack thereof) for its cells.
    pub undo: Vec<EditAction>,
    /// Redo history: inverse actions pushed by undo, cleared on any new edit.
    pub redo: Vec<EditAction>,
    /// `Some` when this grid presents an ad-hoc query result instead of a table page.
    pub sql: Option<String>,
    /// Whether the grid toolbar (transaction/filter/sort/...) is shown.
    pub show_toolbar: bool,
    /// Whether the grid's bottom action bar and status line are shown. Off for grids embedded in
    /// another view (e.g. the view designer's explain panel), where they would be redundant.
    pub show_footer: bool,
    /// Whether cells may be edited. Query grids only allow this with an inferred table.
    pub editable: bool,
    /// The `ORDER BY` applied to the table when pages are fetched.
    pub sort_rules: Vec<SortRule>,
    /// Whether the sort criteria panel is expanded below the toolbar.
    pub sort_open: bool,
    /// The panel's working copy of the rules while it is open; applied on `Apply`.
    pub sort_draft: Vec<SortRule>,
    /// The draft rule highlighted for reordering, if any.
    pub sort_selected: Option<usize>,
    /// The `WHERE` tree applied to the table when pages are fetched.
    pub filters: Vec<FilterNode>,
    /// Whether the filter builder panel is expanded below the toolbar.
    pub filter_open: bool,
    /// The panel's working copy of the tree while it is open; applied on `Apply`.
    pub filter_draft: Vec<FilterNode>,
    /// Wall-clock execution time of the statement that produced the rows currently on screen
    /// (a query result, or the table page's `fetch_page`), shown beside the pager.
    pub elapsed: Option<std::time::Duration>,
}

pub fn compute_column_widths(columns: &[ColumnInfo], rows: &[Vec<CellValue>]) -> Vec<f32> {
    let mut widths: Vec<usize> = columns
        .iter()
        .map(|column| display_width(&column.name))
        .collect();

    for row in rows.iter().take(200) {
        for (index, cell) in row.iter().enumerate() {
            let Some(slot) = widths.get_mut(index) else {
                break;
            };
            let text = cell.as_display();
            let first_line: String = text.lines().next().unwrap_or("").chars().take(48).collect();
            *slot = (*slot).max(display_width(&first_line));
        }
    }

    widths
        .into_iter()
        .map(|width| (width as f32 * 7.2 + 22.0 + SORT_BADGE_ALLOWANCE).clamp(56.0, 260.0))
        .collect()
}

/// Horizontal room the header reserves for its sort badge (icon + gap), so column names are
/// not truncated when the badge is revealed.
const SORT_BADGE_ALLOWANCE: f32 = 20.0;

/// The natural-order widths for a freshly fetched page: content-fitted, unless the user has dragged
/// column edges — then each column keeps the width it had, matched by name so paging/sorting does
/// not snap the widths back. The same carry-over applies when the page keeps the same column set:
/// sorting changes the rows, not the schema, so a column must not shrink because the new first page
/// happens to hold shorter values (its later values then clipped).
pub fn page_column_widths(
    columns: &[ColumnInfo],
    rows: &[Vec<CellValue>],
    previous: &[ColumnInfo],
    previous_widths: &[f32],
    manual: bool,
) -> Vec<f32> {
    let mut widths = compute_column_widths(columns, rows);
    let same_columns = previous.len() == columns.len()
        && columns.iter().all(|column| {
            previous
                .iter()
                .any(|candidate| candidate.name == column.name)
        });
    if manual || same_columns {
        for (index, column) in columns.iter().enumerate() {
            let carried = previous
                .iter()
                .position(|candidate| candidate.name == column.name)
                .and_then(|slot| previous_widths.get(slot).copied());
            if let Some(width) = carried {
                widths[index] = width;
            }
        }
    }
    widths
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|character| if (character as u32) >= 0x1100 { 2 } else { 1 })
        .sum()
}

fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Quote a possibly schema-qualified name (`public.users`) part by part, so the display shows
/// `"public"."users"` rather than one identifier with a dot inside it.
fn quote_qualified(dialect: DriverDialect, name: &str) -> String {
    name.split('.')
        .map(|part| dialect.quote_identifier(part))
        .collect::<Vec<_>>()
        .join(".")
}

/// The `SELECT` a table page runs, written in `dialect`'s own SQL so the grid's status line is
/// truthful for every engine: identifiers quoted the engine's way, and paging in the engine's
/// syntax (MySQL's `LIMIT o,n`, PostgreSQL/SQLite's `LIMIT n OFFSET o`, SQL Server/Oracle's
/// `OFFSET … FETCH NEXT`).
///
/// A schema-qualified engine's table name already carries its schema (`public.users`), and the
/// database is the connection's context rather than a name prefix (PostgreSQL/Oracle reject a
/// three-part name). MySQL qualifies with the database and SQLite with nothing, since its
/// "database" is the file path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_display_sql(
    dialect: DriverDialect,
    database: &str,
    table: &str,
    filters: &[FilterNode],
    sorts: &[SortColumn],
    columns: &[ColumnInfo],
    offset: u64,
    page_size: u64,
) -> String {
    let relation = match dialect {
        DriverDialect::Mysql => {
            if database.is_empty() {
                dialect.quote_identifier(table)
            } else {
                format!(
                    "{}.{}",
                    dialect.quote_identifier(database),
                    dialect.quote_identifier(table)
                )
            }
        }
        DriverDialect::Sqlite => dialect.quote_identifier(table),
        DriverDialect::SqlServer => {
            if database.is_empty() {
                quote_qualified(dialect, table)
            } else {
                format!(
                    "{}.{}",
                    dialect.quote_identifier(database),
                    quote_qualified(dialect, table)
                )
            }
        }
        DriverDialect::Postgres | DriverDialect::Oracle => quote_qualified(dialect, table),
        DriverDialect::Generic => {
            if database.is_empty() {
                dialect.quote_identifier(table)
            } else {
                format!(
                    "{}.{}",
                    dialect.quote_identifier(database),
                    dialect.quote_identifier(table)
                )
            }
        }
    };

    let mut sql = format!("SELECT * FROM {relation}");
    sql.push_str(&filter_display_clause(filters, dialect));

    let mut order = sorts
        .iter()
        .map(|sort| {
            format!(
                "{} {}",
                dialect.quote_identifier(&sort.column),
                if sort.descending { "DESC" } else { "ASC" }
            )
        })
        .collect::<Vec<_>>();
    // `OFFSET … FETCH NEXT` is only legal after an `ORDER BY`. With no sort of the user's, the
    // engine orders by the primary key (both drivers do), and without one SQL Server accepts a
    // constant ordering while Oracle takes a positional one.
    if order.is_empty() && dialect.paging_needs_order() {
        order = columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| dialect.quote_identifier(&column.name))
            .collect();
        if order.is_empty() {
            order.push(match dialect {
                DriverDialect::Oracle => "1".to_string(),
                _ => "(SELECT NULL)".to_string(),
            });
        }
    }
    if !order.is_empty() {
        sql.push_str(" ORDER BY ");
        sql.push_str(&order.join(", "));
    }

    sql.push(' ');
    sql.push_str(&dialect.paging_clause(offset, page_size));
    sql
}

/// A human-readable ` WHERE ...` for the status bar. Values are inlined here, unlike the
/// parameterized query the driver builds.
fn filter_display_clause(filter: &[FilterNode], dialect: DriverDialect) -> String {
    match filter_display_fragment(filter, dialect) {
        Some(expr) => format!(" WHERE {expr}"),
        None => String::new(),
    }
}

fn filter_display_fragment(filter: &[FilterNode], dialect: DriverDialect) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    for node in filter {
        let piece = match node {
            FilterNode::Condition(condition) => condition_display(condition, dialect),
            FilterNode::Group(group) => {
                if !group.enabled {
                    None
                } else {
                    filter_display_fragment(&group.children, dialect)
                        .map(|inner| format!("({inner})"))
                }
            }
        };
        let Some(piece) = piece else {
            continue;
        };
        if clauses.is_empty() {
            clauses.push(piece);
        } else {
            let conjunction = match node.conjunction() {
                FilterConjunction::And => "AND",
                FilterConjunction::Or => "OR",
            };
            clauses.push(format!("{conjunction} {piece}"));
        }
    }
    (!clauses.is_empty()).then(|| clauses.join(" "))
}

fn condition_display(condition: &FilterCondition, dialect: DriverDialect) -> Option<String> {
    if !condition.enabled || condition.column.is_empty() {
        return None;
    }
    let operator = condition.operator;
    if operator.needs_value() && condition.value.is_empty() {
        return None;
    }
    if operator.needs_second_value() && condition.value2.is_empty() {
        return None;
    }
    let column = dialect.quote_identifier(&condition.column);
    let value = quote_literal(&condition.value);
    let piece = match operator {
        FilterOperator::Equal => format!("{column} = {value}"),
        FilterOperator::NotEqual => format!("{column} <> {value}"),
        FilterOperator::LessThan => format!("{column} < {value}"),
        FilterOperator::LessOrEqual => format!("{column} <= {value}"),
        FilterOperator::GreaterThan => format!("{column} > {value}"),
        FilterOperator::GreaterOrEqual => format!("{column} >= {value}"),
        FilterOperator::Contains => {
            format!(
                "{column} LIKE {}",
                quote_literal(&format!("%{}%", condition.value))
            )
        }
        FilterOperator::NotContains => format!(
            "{column} NOT LIKE {}",
            quote_literal(&format!("%{}%", condition.value))
        ),
        FilterOperator::StartsWith => {
            format!(
                "{column} LIKE {}",
                quote_literal(&format!("{}%", condition.value))
            )
        }
        FilterOperator::NotStartsWith => format!(
            "{column} NOT LIKE {}",
            quote_literal(&format!("{}%", condition.value))
        ),
        FilterOperator::EndsWith => {
            format!(
                "{column} LIKE {}",
                quote_literal(&format!("%{}", condition.value))
            )
        }
        FilterOperator::NotEndsWith => format!(
            "{column} NOT LIKE {}",
            quote_literal(&format!("%{}", condition.value))
        ),
        FilterOperator::IsNull => format!("{column} IS NULL"),
        FilterOperator::IsNotNull => format!("{column} IS NOT NULL"),
        FilterOperator::IsEmpty => format!("({column} IS NULL OR {column} = '')"),
        FilterOperator::IsNotEmpty => format!("({column} IS NOT NULL AND {column} <> '')"),
        FilterOperator::Between => format!(
            "{column} BETWEEN {value} AND {}",
            quote_literal(&condition.value2)
        ),
        FilterOperator::NotBetween => format!(
            "{column} NOT BETWEEN {value} AND {}",
            quote_literal(&condition.value2)
        ),
        FilterOperator::InList => {
            let values = condition.list_values();
            if values.is_empty() {
                return None;
            }
            let list = values
                .iter()
                .map(|item| quote_literal(item))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{column} IN ({list})")
        }
        FilterOperator::NotInList => {
            let values = condition.list_values();
            if values.is_empty() {
                return None;
            }
            let list = values
                .iter()
                .map(|item| quote_literal(item))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{column} NOT IN ({list})")
        }
    };
    Some(piece)
}

impl GridState {
    pub fn has_next(&self) -> bool {
        match self.total_rows {
            Some(total) => (self.page_index + 1) * self.page_size < total,
            None => !self.rows.is_empty(),
        }
    }

    pub fn last_page(&self) -> Option<u64> {
        self.total_rows.map(|total| {
            if total == 0 {
                0
            } else {
                (total - 1) / self.page_size
            }
        })
    }

    pub fn sql(&self) -> String {
        if let Some(sql) = &self.sql {
            return sql.clone();
        }
        let offset = self.page_index.saturating_mul(self.page_size);
        page_display_sql(
            self.dialect,
            &self.database,
            &self.table,
            &self.filters,
            &self.sort_columns(),
            &self.all_columns,
            offset,
            self.page_size,
        )
    }

    /// The filter tree as an engine-agnostic `WHERE`. Disabled/incomplete nodes are pruned here;
    /// the driver also skips them and drops any group left empty.
    pub fn filter_conditions(&self) -> Vec<FilterNode> {
        self.filters.clone()
    }

    /// The enabled sort rules, in panel order, as an engine-agnostic `ORDER BY`.
    pub fn sort_columns(&self) -> Vec<SortColumn> {
        self.sort_rules
            .iter()
            .filter(|rule| rule.enabled && !rule.column.is_empty())
            .map(|rule| SortColumn {
                column: rule.column.clone(),
                descending: rule.descending,
            })
            .collect()
    }

    /// The natural (engine) column index a display column reads its cell values from.
    pub fn source_col(&self, col: usize) -> usize {
        self.column_order.get(col).copied().unwrap_or(col)
    }

    /// Whether the 字段筛选 popup holds a layout that differs from the table's natural columns.
    pub fn field_layout_configured(&self) -> bool {
        !self.field_layout.is_default(&self.all_columns)
    }

    /// Install a freshly fetched page in the table's natural order and project the field layout on
    /// top of it. `widths` are the natural-order widths; they are re-ordered alongside the columns.
    pub fn install_page(
        &mut self,
        columns: Vec<ColumnInfo>,
        rows: Vec<Vec<CellValue>>,
        widths: Vec<f32>,
    ) {
        self.all_columns = columns;
        self.column_order = (0..self.all_columns.len()).collect();
        self.columns = self.all_columns.clone();
        self.column_widths = widths;
        self.visible_columns = self.columns.len();
        self.rows = Arc::new(rows);
        self.reproject_fields();
    }

    /// Re-order `columns`/`column_widths` into the field layout's display order (visible fields
    /// first). `rows` are untouched: they keep the engine's order and are read through
    /// [`Self::source_col`].
    pub fn reproject_fields(&mut self) {
        if self.all_columns.is_empty() {
            return;
        }
        let (permutation, visible) = self.field_layout.permutation(&self.all_columns);
        if permutation.len() != self.columns.len() || self.column_order.len() != permutation.len() {
            // A stale permutation (e.g. the first page after the layout changed shape): start from
            // the natural order instead of composing with a mapping that no longer applies. The
            // widths are dropped too — they belong to the order being discarded — so the render
            // falls back to the default width per column.
            self.column_order = (0..self.all_columns.len()).collect();
            self.columns = self.all_columns.clone();
            self.column_widths.clear();
        }
        let composed = field_permutation(&self.column_order, &permutation, self.all_columns.len());
        permute_in_place(&mut self.columns, &composed);
        permute_in_place(&mut self.column_widths, &composed);
        self.column_order = permutation;
        self.visible_columns = visible;
        self.field_layout.order = self.field_layout.resolved_order(&self.all_columns);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(names: &[&str]) -> Vec<ColumnInfo> {
        names
            .iter()
            .map(|name| ColumnInfo {
                name: name.to_string(),
                data_type: "int".to_string(),
                nullable: true,
                primary_key: false,
                comment: String::new(),
            })
            .collect()
    }

    fn hidden(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn sorting_keeps_the_widths_of_the_same_column_set() {
        let previous = columns(&["BatchID", "LineID"]);
        let previous_widths = vec![130.0, 80.0];
        let rows = vec![vec![
            CellValue::Text("x".to_string()),
            CellValue::Text("0E".to_string()),
        ]];
        let widths = page_column_widths(&previous, &rows, &previous, &previous_widths, false);
        assert_eq!(widths, previous_widths);
    }

    #[test]
    fn permuting_places_each_element_at_its_sources_position() {
        let mut values = vec!['a', 'b', 'c', 'd'];
        permute_in_place(&mut values, &[3, 1, 0, 2]);
        assert_eq!(values, vec!['d', 'b', 'a', 'c']);
    }

    #[test]
    fn permuting_ignores_a_non_permutation() {
        let mut values = vec!['a', 'b', 'c'];
        permute_in_place(&mut values, &[1, 1, 0]);
        assert_eq!(values, vec!['a', 'b', 'c']);
    }

    #[test]
    fn a_layout_orders_the_visible_fields_first() {
        let layout = GridFieldLayout {
            order: vec!["c".to_string(), "a".to_string(), "b".to_string()],
            hidden: hidden(&["a"]),
        };
        let (permutation, visible) = layout.permutation(&columns(&["a", "b", "c"]));
        // `c` and `b` are visible and lead; the hidden `a` is kept after them.
        assert_eq!(permutation, vec![2, 1, 0]);
        assert_eq!(visible, 2);
    }

    #[test]
    fn a_layout_that_hides_every_field_falls_back_to_the_table_order() {
        let layout = GridFieldLayout {
            order: vec!["a".to_string(), "b".to_string()],
            hidden: hidden(&["a", "b"]),
        };
        let (permutation, visible) = layout.permutation(&columns(&["a", "b"]));
        assert_eq!(permutation, vec![0, 1]);
        assert_eq!(visible, 2);
    }

    #[test]
    fn resolved_order_appends_fields_the_layout_does_not_know() {
        let layout = GridFieldLayout {
            order: vec!["b".to_string(), "gone".to_string()],
            hidden: BTreeSet::new(),
        };
        assert_eq!(
            layout.resolved_order(&columns(&["a", "b", "c"])),
            vec!["b", "a", "c"]
        );
    }

    #[test]
    fn only_the_tables_own_order_is_the_default() {
        let table = columns(&["a", "b"]);
        assert!(GridFieldLayout::default().is_default(&table));
        assert!(
            GridFieldLayout {
                order: vec!["a".to_string(), "b".to_string()],
                hidden: BTreeSet::new(),
            }
            .is_default(&table)
        );
        assert!(
            !GridFieldLayout {
                order: vec!["b".to_string(), "a".to_string()],
                hidden: BTreeSet::new(),
            }
            .is_default(&table)
        );
        assert!(
            !GridFieldLayout {
                order: vec!["a".to_string(), "b".to_string()],
                hidden: hidden(&["a"]),
            }
            .is_default(&table)
        );
    }

    #[test]
    fn reprojecting_composes_with_the_order_already_on_screen() {
        // The columns are currently displayed as `b, a, c`; the layout wants `c, b, a`.
        let composed = field_permutation(&[1, 0, 2], &[2, 1, 0], 3);
        let mut values = vec!["b", "a", "c"];
        permute_in_place(&mut values, &composed);
        assert_eq!(values, vec!["c", "b", "a"]);
    }

    #[test]
    fn moving_a_field_reorders_the_working_list() {
        let mut layout = GridFieldLayout::default();
        let order = ["a".to_string(), "b".to_string(), "c".to_string()];
        layout.move_field(&order, 0, 2);
        assert_eq!(layout.order, vec!["b", "c", "a"]);
        let moved = layout.order.clone();
        layout.move_field(&moved, 2, 0);
        assert_eq!(layout.order, vec!["a", "b", "c"]);
    }

    #[test]
    fn inverting_a_field_layout_never_hides_every_field() {
        let table = columns(&["a", "b", "c"]);
        let mut layout = GridFieldLayout::default();
        layout.order = layout.resolved_order(&table);
        // Everything was shown, so the inversion hides all but the first field.
        layout.invert_visibility();
        assert_eq!(layout.hidden, hidden(&["b", "c"]));
        // Inverting back shows exactly the field the first inversion kept.
        layout.invert_visibility();
        assert_eq!(layout.hidden, hidden(&["a"]));
        // A single-field table has nothing to invert.
        let single = columns(&["a"]);
        let mut only = GridFieldLayout::default();
        only.order = only.resolved_order(&single);
        only.invert_visibility();
        assert!(only.hidden.is_empty());
    }

    /// Each engine's paging statement is shown with its own quoting and paging syntax, so a
    /// PostgreSQL page no longer reads as MySQL's `LIMIT 0,1000`.
    #[test]
    fn the_status_statement_speaks_the_connections_dialect() {
        let mysql = page_display_sql(
            DriverDialect::Mysql,
            "test",
            "gen_table",
            &[],
            &[],
            &[],
            0,
            1000,
        );
        assert_eq!(mysql, "SELECT * FROM `test`.`gen_table` LIMIT 0,1000");

        let postgres = page_display_sql(
            DriverDialect::Postgres,
            "test",
            "public.gen_table",
            &[],
            &[],
            &[],
            0,
            1000,
        );
        assert_eq!(
            postgres,
            "SELECT * FROM \"public\".\"gen_table\" LIMIT 1000 OFFSET 0"
        );

        let sqlite = page_display_sql(
            DriverDialect::Sqlite,
            "C:\\data\\app.db",
            "users",
            &[],
            &[],
            &[],
            20,
            10,
        );
        assert_eq!(sqlite, "SELECT * FROM `users` LIMIT 10 OFFSET 20");

        let oracle = page_display_sql(
            DriverDialect::Oracle,
            "FREEPDB1",
            "SCOTT.EMP",
            &[],
            &[],
            &[],
            0,
            100,
        );
        assert_eq!(
            oracle,
            "SELECT * FROM \"SCOTT\".\"EMP\" ORDER BY 1 OFFSET 0 ROWS FETCH NEXT 100 ROWS ONLY"
        );
    }

    /// SQL Server's `OFFSET … FETCH NEXT` needs an `ORDER BY`; the display orders by the primary
    /// key when the table has one (as the driver does) and falls back to a constant order.
    #[test]
    fn a_sql_server_page_orders_before_fetching() {
        let mut keyed = columns(&["id", "name"]);
        keyed[0].primary_key = true;
        assert_eq!(
            page_display_sql(
                DriverDialect::SqlServer,
                "app",
                "dbo.users",
                &[],
                &[],
                &keyed,
                40,
                20,
            ),
            "SELECT * FROM [app].[dbo].[users] ORDER BY [id] OFFSET 40 ROWS FETCH NEXT 20 ROWS ONLY"
        );
        assert_eq!(
            page_display_sql(
                DriverDialect::SqlServer,
                "app",
                "dbo.logs",
                &[],
                &[],
                &columns(&["message"]),
                0,
                20,
            ),
            "SELECT * FROM [app].[dbo].[logs] ORDER BY (SELECT NULL) \
             OFFSET 0 ROWS FETCH NEXT 20 ROWS ONLY"
        );
    }

    /// The shown `WHERE` quotes its columns the engine's way and keeps the applied sort order.
    #[test]
    fn the_status_statement_quotes_filters_and_sorts_per_dialect() {
        let filter = vec![FilterNode::Condition(FilterCondition {
            column: "name".to_string(),
            operator: FilterOperator::Equal,
            value: "ann".to_string(),
            ..FilterCondition::default()
        })];
        let sorts = vec![SortColumn {
            column: "id".to_string(),
            descending: true,
        }];
        assert_eq!(
            page_display_sql(
                DriverDialect::SqlServer,
                "app",
                "dbo.users",
                &filter,
                &sorts,
                &columns(&["id", "name"]),
                0,
                20,
            ),
            "SELECT * FROM [app].[dbo].[users] WHERE [name] = 'ann' ORDER BY [id] DESC \
             OFFSET 0 ROWS FETCH NEXT 20 ROWS ONLY"
        );
    }

    #[test]
    fn subtracting_a_cell_splits_a_select_all_range_in_four() {
        let mut selection = CellSelection::single((0, 0), (9, 2));
        assert!(selection.subtract(CellRange {
            anchor: (5, 1),
            cursor: (5, 1),
        }));
        assert_eq!(selection.ranges.len(), 4);
        assert!(!selection.contains(5, 1));
        assert_eq!(selection.cells().len(), 30 - 1);
        assert_eq!(
            selection.ranges,
            vec![
                CellRange {
                    anchor: (0, 0),
                    cursor: (4, 2),
                },
                CellRange {
                    anchor: (6, 0),
                    cursor: (9, 2),
                },
                CellRange {
                    anchor: (5, 0),
                    cursor: (5, 0),
                },
                CellRange {
                    anchor: (5, 2),
                    cursor: (5, 2),
                },
            ]
        );
    }

    #[test]
    fn subtracting_a_cell_drops_a_single_cell_range() {
        let mut selection = CellSelection::single((2, 2), (2, 2));
        assert!(selection.subtract(CellRange {
            anchor: (2, 2),
            cursor: (2, 2),
        }));
        assert!(selection.ranges.is_empty());
        assert!(!selection.contains(2, 2));
    }

    #[test]
    fn subtracting_a_row_span_removes_the_row_from_select_all() {
        let mut selection = CellSelection::single((0, 0), (3, 2));
        assert!(selection.subtract(CellRange {
            anchor: (1, 0),
            cursor: (1, 2),
        }));
        assert!(!selection.contains(1, 0));
        assert!(!selection.contains(1, 2));
        assert_eq!(selection.cells().len(), 12 - 3);
    }

    #[test]
    fn subtracting_something_unselected_changes_nothing() {
        let mut selection = CellSelection::single((0, 0), (1, 1));
        assert!(!selection.subtract(CellRange {
            anchor: (5, 5),
            cursor: (5, 5),
        }));
        assert_eq!(
            selection.ranges,
            vec![CellRange {
                anchor: (0, 0),
                cursor: (1, 1),
            }]
        );
    }
}
