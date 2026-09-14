use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DriverId(pub String);

impl DriverId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DriverId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<&str> for DriverId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub driver: DriverId,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub driver: DriverId,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub database: Option<String>,
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseInfo {
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectKind {
    Table,
    View,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub kind: ObjectKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum CellValue {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
}

impl CellValue {
    pub fn as_display(&self) -> String {
        use std::fmt::Write;

        match self {
            CellValue::Null => "NULL".to_string(),
            CellValue::Bool(value) => value.to_string(),
            CellValue::Int(value) => value.to_string(),
            CellValue::Uint(value) => value.to_string(),
            CellValue::Float(value) => value.to_string(),
            CellValue::Text(value) => value.clone(),
            CellValue::Bytes(bytes) => {
                let mut rendered = String::with_capacity(bytes.len() * 2);
                for byte in bytes {
                    let _ = write!(rendered, "{byte:02x}");
                }
                rendered
            }
        }
    }

    pub fn as_edit_string(&self) -> String {
        match self {
            CellValue::Null => String::new(),
            _ => self.as_display(),
        }
    }
}

/// One column of an `ORDER BY` clause. `descending` selects `DESC` vs `ASC`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortColumn {
    pub column: String,
    pub descending: bool,
}

/// A comparison operator available in the filter builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOperator {
    Equal,
    NotEqual,
    LessThan,
    LessOrEqual,
    GreaterThan,
    GreaterOrEqual,
    Contains,
    NotContains,
    StartsWith,
    NotStartsWith,
    EndsWith,
    NotEndsWith,
    IsNull,
    IsNotNull,
    IsEmpty,
    IsNotEmpty,
    Between,
    NotBetween,
    InList,
    NotInList,
}

impl FilterOperator {
    /// Every operator, in the order the UI presents them.
    pub const ALL: [FilterOperator; 20] = [
        FilterOperator::Equal,
        FilterOperator::NotEqual,
        FilterOperator::LessThan,
        FilterOperator::LessOrEqual,
        FilterOperator::GreaterThan,
        FilterOperator::GreaterOrEqual,
        FilterOperator::Contains,
        FilterOperator::NotContains,
        FilterOperator::StartsWith,
        FilterOperator::NotStartsWith,
        FilterOperator::EndsWith,
        FilterOperator::NotEndsWith,
        FilterOperator::IsNull,
        FilterOperator::IsNotNull,
        FilterOperator::IsEmpty,
        FilterOperator::IsNotEmpty,
        FilterOperator::Between,
        FilterOperator::NotBetween,
        FilterOperator::InList,
        FilterOperator::NotInList,
    ];

    /// The i18n key for the operator's label.
    pub fn label_key(self) -> &'static str {
        match self {
            FilterOperator::Equal => "filter.op.equal",
            FilterOperator::NotEqual => "filter.op.not_equal",
            FilterOperator::LessThan => "filter.op.less",
            FilterOperator::LessOrEqual => "filter.op.less_equal",
            FilterOperator::GreaterThan => "filter.op.greater",
            FilterOperator::GreaterOrEqual => "filter.op.greater_equal",
            FilterOperator::Contains => "filter.op.contains",
            FilterOperator::NotContains => "filter.op.not_contains",
            FilterOperator::StartsWith => "filter.op.starts_with",
            FilterOperator::NotStartsWith => "filter.op.not_starts_with",
            FilterOperator::EndsWith => "filter.op.ends_with",
            FilterOperator::NotEndsWith => "filter.op.not_ends_with",
            FilterOperator::IsNull => "filter.op.is_null",
            FilterOperator::IsNotNull => "filter.op.is_not_null",
            FilterOperator::IsEmpty => "filter.op.is_empty",
            FilterOperator::IsNotEmpty => "filter.op.is_not_empty",
            FilterOperator::Between => "filter.op.between",
            FilterOperator::NotBetween => "filter.op.not_between",
            FilterOperator::InList => "filter.op.in_list",
            FilterOperator::NotInList => "filter.op.not_in_list",
        }
    }

    /// Whether the operator consumes a value.
    pub fn needs_value(self) -> bool {
        !matches!(
            self,
            FilterOperator::IsNull
                | FilterOperator::IsNotNull
                | FilterOperator::IsEmpty
                | FilterOperator::IsNotEmpty
        )
    }

    /// Whether the operator consumes a second value (`value2`).
    pub fn needs_second_value(self) -> bool {
        matches!(self, FilterOperator::Between | FilterOperator::NotBetween)
    }
}

/// How a filter condition joins to the condition before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterConjunction {
    #[default]
    And,
    Or,
}

impl FilterConjunction {
    pub fn toggled(self) -> Self {
        match self {
            FilterConjunction::And => FilterConjunction::Or,
            FilterConjunction::Or => FilterConjunction::And,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            FilterConjunction::And => "filter.conjunction.and",
            FilterConjunction::Or => "filter.conjunction.or",
        }
    }
}

/// One row of the filter builder: a column, a comparison and up to two values. Engine-agnostic;
/// each driver translates it into its own `WHERE` syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterCondition {
    pub column: String,
    pub operator: FilterOperator,
    pub value: String,
    pub value2: String,
    pub conjunction: FilterConjunction,
    pub enabled: bool,
}

impl FilterCondition {
    pub fn new(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            operator: FilterOperator::Equal,
            value: String::new(),
            value2: String::new(),
            conjunction: FilterConjunction::And,
            enabled: true,
        }
    }

    /// The comma-separated `value` split into an `IN` list, trimming blanks.
    pub fn list_values(&self) -> Vec<String> {
        self.value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    }
}

impl Default for FilterCondition {
    fn default() -> Self {
        Self::new("")
    }
}

#[derive(Debug, Clone)]
pub struct PageRequest {
    pub page: u64,
    pub page_size: u64,
    /// Applied `ORDER BY`, in priority order. Empty means the engine's natural order.
    pub order_by: Vec<SortColumn>,
    /// Applied `WHERE`, as a conjunction of conditions. Empty means no filtering.
    pub filter: Vec<FilterCondition>,
}

impl PageRequest {
    pub fn new(page: u64, page_size: u64) -> Self {
        Self {
            page,
            page_size: page_size.max(1),
            order_by: Vec::new(),
            filter: Vec::new(),
        }
    }

    pub fn with_order_by(mut self, order_by: Vec<SortColumn>) -> Self {
        self.order_by = order_by;
        self
    }

    pub fn with_filter(mut self, filter: Vec<FilterCondition>) -> Self {
        self.filter = filter;
        self
    }

    pub fn offset(&self) -> u64 {
        self.page.saturating_mul(self.page_size)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TablePage {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<CellValue>>,
    pub page: u64,
    pub page_size: u64,
    pub total_rows: Option<u64>,
}

/// A single-row update: `SET` assignments plus the key columns used in the `WHERE` clause.
#[derive(Debug, Clone)]
pub struct RowUpdate {
    pub set: Vec<(String, Option<String>)>,
    pub keys: Vec<(String, String)>,
}

/// The outcome of running an arbitrary SQL statement from the query editor.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Columns of the first result set. Empty when the statement produced no rows.
    pub columns: Vec<ColumnInfo>,
    /// Rows of the first result set, in column order. Empty when there was no result set.
    pub rows: Vec<Vec<CellValue>>,
    /// Rows changed by a DML statement; `0` for statements that return a result set.
    pub rows_affected: u64,
    /// Whether the statement produced a result set (a `SELECT`-like statement).
    pub has_result_set: bool,
    /// Auto-increment value generated by the last `INSERT`, when applicable.
    pub last_insert_id: Option<u64>,
}
