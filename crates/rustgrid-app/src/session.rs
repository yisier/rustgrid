use std::collections::BTreeMap;
use std::sync::Arc;

use rustgrid_core::{
    CellValue, ColumnInfo, Connection, ConnectionProfile, FilterCondition, FilterConjunction,
    FilterNode, FilterOperator, QueryResult, RoutineDetails, RoutineInfo, RoutineKind, SortColumn,
    TableInfo, TableStatus, ViewDetails,
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
            Category::Tables => "common.table",
            Category::Views => "common.view",
            Category::Functions => "common.function",
            Category::Queries => "common.query",
            Category::Backups => "common.backup",
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

/// An open SQL editor tab. `sql` is the editable document; `caret` and `anchor` are byte
/// offsets into it, so they can be matched against `TextLayout` indices directly.
pub struct QueryTab {
    pub id: u64,
    /// The saved query name, once this tab was saved to (or opened from) the saved-query list.
    pub name: Option<String>,
    pub connection_index: Option<usize>,
    pub database: Option<String>,
    pub sql: String,
    pub caret: usize,
    pub anchor: usize,
    pub selecting: bool,
    pub running: bool,
    pub result: Loadable<QueryResult>,
    /// The id of the grid in `AppView::grids` that shows this tab's result, if any.
    pub grid_id: Option<u64>,
    /// Undo history for the editor: `(sql, caret, anchor)` snapshots before each edit.
    pub undo: Vec<(String, usize, usize)>,
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
            connection_index: None,
            database: None,
            sql: String::new(),
            caret: 0,
            anchor: 0,
            selecting: false,
            running: false,
            result: Loadable::Idle,
            grid_id: None,
            undo: Vec::new(),
            routine: None,
            view: None,
        }
    }

    /// The byte range of the current selection, normalised to `start..end`.
    pub fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }
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

pub struct GridState {
    pub id: u64,
    pub connection: Arc<dyn Connection>,
    pub connection_name: String,
    pub database: String,
    pub table: String,
    pub is_view: bool,
    pub page_index: u64,
    pub page_size: u64,
    pub loading: bool,
    pub error: Option<String>,
    pub columns: Vec<ColumnInfo>,
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
    /// Wall-clock execution time of the query that produced this grid, when it is a query result.
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

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|character| if (character as u32) >= 0x1100 { 2 } else { 1 })
        .sum()
}

fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// A human-readable ` WHERE ...` for the status bar. Values are inlined here, unlike the
/// parameterized query the driver builds.
fn filter_display_clause(filter: &[FilterNode]) -> String {
    match filter_display_fragment(filter) {
        Some(expr) => format!(" WHERE {expr}"),
        None => String::new(),
    }
}

fn filter_display_fragment(filter: &[FilterNode]) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    for node in filter {
        let piece = match node {
            FilterNode::Condition(condition) => condition_display(condition),
            FilterNode::Group(group) => {
                if !group.enabled {
                    None
                } else {
                    filter_display_fragment(&group.children).map(|inner| format!("({inner})"))
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

fn condition_display(condition: &FilterCondition) -> Option<String> {
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
    let column = format!("`{}`", condition.column.replace('`', "``"));
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
        let mut sql = format!("SELECT * FROM `{}`.`{}`", self.database, self.table);
        sql.push_str(&filter_display_clause(&self.filters));
        let order = self
            .sort_columns()
            .into_iter()
            .map(|sort| {
                format!(
                    "`{}` {}",
                    sort.column,
                    if sort.descending { "DESC" } else { "ASC" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        if !order.is_empty() {
            sql.push_str(" ORDER BY ");
            sql.push_str(&order);
        }
        sql.push_str(&format!(" LIMIT {offset},{}", self.page_size));
        sql
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
}
