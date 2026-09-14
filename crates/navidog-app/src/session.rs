use std::collections::BTreeMap;
use std::sync::Arc;

use navidog_core::{
    CellValue, ColumnInfo, Connection, ConnectionProfile, FilterCondition, FilterConjunction,
    FilterOperator, QueryResult, SortColumn, TableInfo,
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

#[derive(Clone, Copy, Debug, Default)]
pub struct CellSelection {
    pub anchor: (usize, usize),
    pub cursor: (usize, usize),
}

impl CellSelection {
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

    pub fn cells(&self) -> Vec<(usize, usize)> {
        let (start_row, end_row) = self.rows();
        let (start_col, end_col) = self.cols();
        let mut cells = Vec::new();
        for row in start_row..=end_row {
            for col in start_col..=end_col {
                cells.push((row, col));
            }
        }
        cells
    }
}

/// An open SQL editor tab. `sql` is the editable document; `caret` and `anchor` are byte
/// offsets into it, so they can be matched against `TextLayout` indices directly.
pub struct QueryTab {
    pub id: u64,
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
}

impl QueryTab {
    pub fn new(id: u64) -> Self {
        Self {
            id,
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

/// Which dropdown of a filter condition row is open.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilterCombo {
    Field,
    Operator,
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
    pub total_rows: Option<u64>,
    pub selection: Option<CellSelection>,
    pub edits: BTreeMap<(usize, usize), Option<String>>,
    /// Undo history: each entry restores the prior edit value (or lack thereof) for its cells.
    pub undo: Vec<EditAction>,
    /// `Some` when this grid presents an ad-hoc query result instead of a table page.
    pub sql: Option<String>,
    /// Whether the grid toolbar (transaction/filter/sort/...) is shown.
    pub show_toolbar: bool,
    /// Whether cells may be edited. Query grids only allow this with an inferred table.
    pub editable: bool,
    /// The `ORDER BY` applied to the table when pages are fetched.
    pub sort_rules: Vec<SortRule>,
    /// Whether the sort criteria panel is expanded below the toolbar.
    pub sort_open: bool,
    /// The panel's working copy of the rules while it is open; applied on `Apply`.
    pub sort_draft: Vec<SortRule>,
    /// The open column-list popup: which draft rule it edits plus the highlighted candidate.
    pub sort_combo: Option<(usize, String)>,
    /// The draft rule highlighted for reordering, if any.
    pub sort_selected: Option<usize>,
    /// The `WHERE` applied to the table when pages are fetched.
    pub filters: Vec<FilterCondition>,
    /// Whether the filter builder panel is expanded below the toolbar.
    pub filter_open: bool,
    /// The panel's working copy of the conditions while it is open; applied on `Apply`.
    pub filter_draft: Vec<FilterCondition>,
    /// The open dropdown in the filter panel: which condition and which combo.
    pub filter_combo: Option<(usize, FilterCombo)>,
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
fn filter_display_clause(filter: &[FilterCondition]) -> String {
    let mut clauses: Vec<String> = Vec::new();
    for condition in filter {
        if !condition.enabled || condition.column.is_empty() {
            continue;
        }
        let operator = condition.operator;
        if operator.needs_value() && condition.value.is_empty() {
            continue;
        }
        if operator.needs_second_value() && condition.value2.is_empty() {
            continue;
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
            FilterOperator::Contains => format!(
                "{column} LIKE {}",
                quote_literal(&format!("%{}%", condition.value))
            ),
            FilterOperator::NotContains => format!(
                "{column} NOT LIKE {}",
                quote_literal(&format!("%{}%", condition.value))
            ),
            FilterOperator::StartsWith => format!(
                "{column} LIKE {}",
                quote_literal(&format!("{}%", condition.value))
            ),
            FilterOperator::NotStartsWith => format!(
                "{column} NOT LIKE {}",
                quote_literal(&format!("{}%", condition.value))
            ),
            FilterOperator::EndsWith => format!(
                "{column} LIKE {}",
                quote_literal(&format!("%{}", condition.value))
            ),
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
                    continue;
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
                    continue;
                }
                let list = values
                    .iter()
                    .map(|item| quote_literal(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{column} NOT IN ({list})")
            }
        };
        if clauses.is_empty() {
            clauses.push(piece);
        } else {
            let conjunction = match condition.conjunction {
                FilterConjunction::And => "AND",
                FilterConjunction::Or => "OR",
            };
            clauses.push(format!("{conjunction} {piece}"));
        }
    }

    if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" "))
    }
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

    /// The enabled filter conditions, in panel order, as an engine-agnostic `WHERE`.
    pub fn filter_conditions(&self) -> Vec<FilterCondition> {
        self.filters
            .iter()
            .filter(|condition| {
                condition.enabled
                    && !condition.column.is_empty()
                    && (!condition.operator.needs_value() || !condition.value.is_empty())
                    && (!condition.operator.needs_second_value() || !condition.value2.is_empty())
            })
            .cloned()
            .collect()
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
