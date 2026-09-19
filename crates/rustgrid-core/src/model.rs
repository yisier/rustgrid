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

/// A named SQL query saved by the user. Queries are filed under a connection (by profile id, so
/// they survive reordering/renaming the connection) and a database name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub sql: String,
    /// The `ConnectionProfile::id` the query belongs to.
    pub connection_id: String,
    /// The database the query is filed under. Empty when the connection has no database.
    #[serde(default)]
    pub database: String,
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

    /// The value as an edit parameter: `None` for SQL `NULL`, otherwise the display string. Used
    /// for key columns so a `NULL` key becomes `IS NULL` instead of binding an empty string (which
    /// MySQL rejects for numeric columns).
    pub fn as_edit_value(&self) -> Option<String> {
        match self {
            CellValue::Null => None,
            _ => Some(self.as_display()),
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

/// A node of the filter tree: a single comparison or a parenthesized sub-group. Siblings are
/// combined by each node's own conjunction (so a group may mix `AND` and `OR`), and a group
/// renders as `( ... )` in the `WHERE` clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterNode {
    Condition(FilterCondition),
    Group(FilterGroup),
}

impl FilterNode {
    /// How this node joins the sibling before it.
    pub fn conjunction(&self) -> FilterConjunction {
        match self {
            FilterNode::Condition(condition) => condition.conjunction,
            FilterNode::Group(group) => group.conjunction,
        }
    }

    pub fn set_conjunction(&mut self, conjunction: FilterConjunction) {
        match self {
            FilterNode::Condition(condition) => condition.conjunction = conjunction,
            FilterNode::Group(group) => group.conjunction = conjunction,
        }
    }

    /// Whether this node (and, for a group, its whole subtree) participates in the filter.
    pub fn enabled(&self) -> bool {
        match self {
            FilterNode::Condition(condition) => condition.enabled,
            FilterNode::Group(group) => group.enabled,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        match self {
            FilterNode::Condition(condition) => condition.enabled = enabled,
            FilterNode::Group(group) => group.enabled = enabled,
        }
    }
}

/// A parenthesized set of filter nodes. `conjunction` joins the group to its previous sibling;
/// the children are joined with each child's own conjunction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterGroup {
    pub conjunction: FilterConjunction,
    pub enabled: bool,
    pub children: Vec<FilterNode>,
}

impl FilterGroup {
    pub fn new(conjunction: FilterConjunction) -> Self {
        Self {
            conjunction,
            enabled: true,
            children: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PageRequest {
    pub page: u64,
    pub page_size: u64,
    /// Applied `ORDER BY`, in priority order. Empty means the engine's natural order.
    pub order_by: Vec<SortColumn>,
    /// Applied `WHERE`, as a tree of conditions and groups. Empty means no filtering.
    pub filter: Vec<FilterNode>,
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

    pub fn with_filter(mut self, filter: Vec<FilterNode>) -> Self {
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
/// A `None` key value means SQL `NULL` (rendered as `IS NULL`).
#[derive(Debug, Clone)]
pub struct RowUpdate {
    pub set: Vec<(String, Option<String>)>,
    pub keys: Vec<(String, Option<String>)>,
}

/// A single-row insert: `(column, value)` assignments. A `None` value is an explicit SQL `NULL`;
/// columns not listed take their default (or stay unset, e.g. for `AUTO_INCREMENT`).
#[derive(Debug, Clone)]
pub struct RowInsert {
    pub values: Vec<(String, Option<String>)>,
}

/// One column of a table as shown by the table designer. Engine-agnostic: the driver translates
/// this to (and from) its own catalog and DDL syntax.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColumnDef {
    pub name: String,
    /// The base type, e.g. `bigint`, `varchar`, `datetime`.
    pub data_type: String,
    /// Display length. Empty or `"0"` means the length is not part of the definition.
    pub length: String,
    /// Decimal places. Empty or `"0"` means the length is not part of the definition.
    pub decimals: String,
    pub nullable: bool,
    /// Default value expression, exactly as it should appear after `DEFAULT`.
    pub default: String,
    pub primary_key: bool,
    pub auto_increment: bool,
    pub unsigned: bool,
    pub zerofill: bool,
    pub comment: String,
    pub charset: String,
    pub collation: String,
    /// Server-side extra clause, e.g. `on update CURRENT_TIMESTAMP`.
    pub extra: String,
    /// Generation expression for a generated column; empty for a normal column.
    pub generated: String,
    /// `true` for a `STORED` generated column, `false` for `VIRTUAL` (or a normal column).
    pub stored: bool,
}

/// One index of a table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
    /// `BTREE`, `HASH`, `FULLTEXT` or `SPATIAL`.
    pub index_type: String,
    pub comment: String,
}

/// One foreign key constraint of a table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ForeignKeyDef {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
    /// `CASCADE`, `SET NULL`, `RESTRICT`, `NO ACTION`, ... Empty means unspecified.
    pub on_delete: String,
    /// Same vocabulary as `on_delete`.
    pub on_update: String,
}

/// One trigger of a table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TriggerDef {
    pub name: String,
    /// `BEFORE` or `AFTER`.
    pub timing: String,
    /// `INSERT`, `UPDATE` or `DELETE`.
    pub event: String,
    pub statement: String,
}

/// Table-level storage options shown by the designer's *Options* tab.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableOptions {
    pub engine: String,
    pub charset: String,
    pub collation: String,
    pub comment: String,
    /// The next auto-increment value, as text.
    pub auto_increment: String,
}

/// A complete table definition: columns plus indexes, foreign keys, triggers and options.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableSchema {
    pub columns: Vec<ColumnDef>,
    pub indexes: Vec<IndexDef>,
    pub foreign_keys: Vec<ForeignKeyDef>,
    pub triggers: Vec<TriggerDef>,
    pub options: TableOptions,
}

/// The kind of database object a backup can contain. Mirrors Navicat's object-selection
/// categories (tables, views, functions, events). Stored procedures are grouped with
/// functions, exactly as Navicat's backup profile does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupObjectKind {
    Table,
    View,
    Function,
    Event,
}

impl BackupObjectKind {
    /// Every kind, in the order the backup object tree presents them.
    pub const ALL: [BackupObjectKind; 4] = [
        BackupObjectKind::Table,
        BackupObjectKind::View,
        BackupObjectKind::Function,
        BackupObjectKind::Event,
    ];

    /// The i18n key for the kind's plural label.
    pub fn label_key(self) -> &'static str {
        match self {
            BackupObjectKind::Table => "backup.kind.tables",
            BackupObjectKind::View => "backup.kind.views",
            BackupObjectKind::Function => "backup.kind.functions",
            BackupObjectKind::Event => "backup.kind.events",
        }
    }
}

/// One object dumped for a backup: its `CREATE` statement plus (for tables) the column names
/// and one pre-rendered SQL value tuple per row. The tuple rendering lives in the driver so the
/// container stays engine-agnostic and the values stay valid literals for that engine.
#[derive(Debug, Clone)]
pub struct ObjectDump {
    pub name: String,
    pub kind: BackupObjectKind,
    /// The object's `CREATE` statement, without a trailing semicolon (as engines report it).
    pub ddl: String,
    /// Column names, used to build the `INSERT` column list. Empty for non-tables.
    pub fields: Vec<String>,
    /// `CREATE TRIGGER` statements belonging to the object. Empty for non-tables.
    pub trigger_ddl: Vec<String>,
    /// One SQL value tuple per row, e.g. `(1, 'a', NULL)`. Empty for non-tables.
    pub rows: Vec<String>,
}

/// One category's selection in a saved backup configuration. `select_all` means "every object of
/// this kind at backup time" (Navicat's *Run-time selected*); otherwise `selected` names the
/// objects to include.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedBackupSelection {
    #[serde(default)]
    pub select_all: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected: Vec<String>,
}

impl SavedBackupSelection {
    pub fn all() -> Self {
        Self {
            select_all: true,
            selected: Vec::new(),
        }
    }

    /// Whether `name` is included.
    pub fn contains(&self, name: &str) -> bool {
        self.select_all || self.selected.iter().any(|item| item == name)
    }
}

/// A named backup configuration saved from the "New Backup" dialog so the same object selection
/// can be reused. Filed under a connection (by profile id) and a database name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedBackup {
    pub name: String,
    /// The `ConnectionProfile::id` the configuration belongs to.
    pub connection_id: String,
    pub database: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    #[serde(default)]
    pub tables: SavedBackupSelection,
    #[serde(default)]
    pub views: SavedBackupSelection,
    #[serde(default)]
    pub functions: SavedBackupSelection,
    #[serde(default)]
    pub events: SavedBackupSelection,
}

impl SavedBackup {
    pub fn selection(&self, kind: BackupObjectKind) -> &SavedBackupSelection {
        match kind {
            BackupObjectKind::Table => &self.tables,
            BackupObjectKind::View => &self.views,
            BackupObjectKind::Function => &self.functions,
            BackupObjectKind::Event => &self.events,
        }
    }

    pub fn selection_mut(&mut self, kind: BackupObjectKind) -> &mut SavedBackupSelection {
        match kind {
            BackupObjectKind::Table => &mut self.tables,
            BackupObjectKind::View => &mut self.views,
            BackupObjectKind::Function => &mut self.functions,
            BackupObjectKind::Event => &mut self.events,
        }
    }
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
