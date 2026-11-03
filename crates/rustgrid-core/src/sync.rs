//! Engine-agnostic schema and row comparison for the 数据传输/同步 workbench.
//!
//! The workbench reads both sides through [`Connection`](crate::Connection) and hands the decoded
//! rows to [`diff_rows`]. The resulting [`RowChange`]s are applied through the target connection's
//! ordinary `insert_rows` / `update_rows` / `delete_rows` methods, so this module never needs to
//! know an engine's DML dialect. [`render_change_sql`] exists only for the SQL preview.

use crate::dialect::DriverDialect;
use crate::model::{CellValue, ColumnDef, ColumnInfo, TableOptions, TableSchema};

/// The columns a row comparison can key on.
///
/// A primary key is always preferred. When a table has none, the first unique key whose columns
/// are all `NOT NULL` is usable: a comparable key must identify exactly one row, and a nullable
/// unique column does not (most engines allow several `NULL`s). Navicat's data synchronization
/// likewise requires a key and lets the user pick another one; without a UI for that choice this
/// falls back to the narrowest usable unique key, then to no key at all.
pub fn usable_key_columns(columns: &[ColumnInfo], schema: Option<&TableSchema>) -> Vec<String> {
    let primary: Vec<String> = columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.clone())
        .collect();
    if !primary.is_empty() {
        return primary;
    }
    let Some(schema) = schema else {
        return Vec::new();
    };
    let mut candidates: Vec<Vec<String>> = schema
        .indexes
        .iter()
        .filter(|index| index.unique && !index.primary)
        .map(|index| {
            index
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect()
        })
        .collect();
    candidates.extend(schema.unique_keys.iter().map(|key| key.columns.clone()));
    // Prefer the narrowest key, which keeps the generated WHERE clause and the in-memory map
    // small; equal-width keys keep the catalog's order.
    candidates.sort_by_key(Vec::len);
    for candidate in candidates {
        if candidate.is_empty() {
            continue;
        }
        let resolved: Option<Vec<String>> = candidate
            .iter()
            .map(|name| {
                columns
                    .iter()
                    .find(|column| column.name.eq_ignore_ascii_case(name))
                    .filter(|column| !column.nullable)
                    .map(|column| column.name.clone())
            })
            .collect();
        if let Some(resolved) = resolved {
            return resolved;
        }
    }
    Vec::new()
}

/// How one column differs between the source and target table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaChangeKind {
    /// The source has the column and the target does not.
    Added,
    /// The target has the column and the source does not.
    Removed,
    /// Both have the column, but its definition differs.
    Modified,
}

/// One structural difference between a source and a target table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaChange {
    pub kind: SchemaChangeKind,
    pub column: String,
    /// The source column's rendered type (empty for [`SchemaChangeKind::Removed`]).
    pub source_type: String,
    /// The target column's rendered type (empty for [`SchemaChangeKind::Added`]).
    pub target_type: String,
}

impl SchemaChange {
    /// A one-line description such as `email varchar(255) → varchar(120)`.
    pub fn summary(&self) -> String {
        // A table-level entry (an index, a constraint, the options) carries no type text; name
        // the object instead of printing an empty `→`.
        if self.source_type.is_empty() && self.target_type.is_empty() {
            return match self.kind {
                SchemaChangeKind::Added => format!("+ {}", self.column),
                SchemaChangeKind::Removed => format!("- {}", self.column),
                SchemaChangeKind::Modified => format!("~ {}", self.column),
            };
        }
        match self.kind {
            SchemaChangeKind::Added => format!("+ {} {}", self.column, self.source_type),
            SchemaChangeKind::Removed => format!("- {} {}", self.column, self.target_type),
            SchemaChangeKind::Modified => format!(
                "~ {} {} → {}",
                self.column, self.target_type, self.source_type
            ),
        }
    }
}

/// The rendered type of a designer column (`varchar(255)`, `decimal(10,2) unsigned`, ...).
pub fn column_type_text(column: &ColumnDef) -> String {
    let mut text = column.data_type.trim().to_string();
    let length = column.length.trim();
    let decimals = column.decimals.trim();
    if !length.is_empty() && length != "0" {
        text.push('(');
        text.push_str(length);
        if !decimals.is_empty() && decimals != "0" {
            text.push(',');
            text.push_str(decimals);
        }
        text.push(')');
    }
    if column.unsigned {
        text.push_str(" unsigned");
    }
    text
}

/// The rendered type of `column` plus each attribute in which it differs from `other`, so a
/// `MODIFY COLUMN` summary names what actually changes (a collation-only difference would
/// otherwise read `varchar(200) → varchar(200)`).
fn column_difference(column: &ColumnDef, other: &ColumnDef, type_text: &str) -> String {
    let mut text = type_text.to_string();
    if column.nullable != other.nullable {
        text.push_str(if column.nullable {
            " NULL"
        } else {
            " NOT NULL"
        });
    }
    if column.primary_key != other.primary_key && column.primary_key {
        text.push_str(" PRIMARY KEY");
    }
    if column.auto_increment != other.auto_increment && column.auto_increment {
        text.push_str(" AUTO_INCREMENT");
    }
    if column.charset != other.charset && !column.charset.trim().is_empty() {
        text.push_str(" CHARSET ");
        text.push_str(column.charset.trim());
    }
    if column.collation != other.collation && !column.collation.trim().is_empty() {
        text.push_str(" COLLATE ");
        text.push_str(column.collation.trim());
    }
    if column.default != other.default && !column.default.trim().is_empty() {
        text.push_str(" DEFAULT ");
        text.push_str(column.default.trim());
    }
    if column.extra != other.extra && !column.extra.trim().is_empty() {
        text.push(' ');
        text.push_str(column.extra.trim());
    }
    if column.comment != other.comment && !column.comment.trim().is_empty() {
        text.push_str(" COMMENT '");
        text.push_str(&column.comment.replace('\'', "''"));
        text.push('\'');
    }
    if column.generated != other.generated && !column.generated.trim().is_empty() {
        text.push_str(" GENERATED (");
        text.push_str(column.generated.trim());
        text.push(')');
    }
    if !column.identity.trim().is_empty() && !column.identity.eq_ignore_ascii_case(&other.identity)
    {
        text.push_str(" GENERATED ");
        text.push_str(column.identity.trim());
    }
    if !column.identity_start.trim().is_empty()
        && column.identity_start.trim() != other.identity_start.trim()
    {
        text.push_str(" START WITH ");
        text.push_str(column.identity_start.trim());
    }
    if !column.identity_increment.trim().is_empty()
        && column.identity_increment.trim() != other.identity_increment.trim()
    {
        text.push_str(" INCREMENT BY ");
        text.push_str(column.identity_increment.trim());
    }
    if column.zerofill != other.zerofill && column.zerofill {
        text.push_str(" ZEROFILL");
    }
    if column.stored != other.stored && column.stored {
        text.push_str(" STORED");
    }
    text
}

/// Compare two table definitions, returning every added, removed or modified column.
///
/// Column names are matched case-insensitively (engines differ in how they fold unquoted
/// identifiers). Every attribute that changes the column's definition is compared — including its
/// character set, collation and comment — because [`Connection::save_table_schema`] emits a
/// `MODIFY COLUMN` for exactly those attributes. Skipping them made a table whose collations
/// differed look different forever while the sync claimed there was nothing to change.
///
/// [`Connection::save_table_schema`]: crate::Connection::save_table_schema
pub fn diff_columns(source: &[ColumnDef], target: &[ColumnDef]) -> Vec<SchemaChange> {
    let mut changes = Vec::new();
    for column in source {
        match target
            .iter()
            .find(|candidate| candidate.name.eq_ignore_ascii_case(&column.name))
        {
            None => changes.push(SchemaChange {
                kind: SchemaChangeKind::Added,
                column: column.name.clone(),
                source_type: column_type_text(column),
                target_type: String::new(),
            }),
            Some(existing) => {
                let source_type = column_type_text(column);
                let target_type = column_type_text(existing);
                let differs = source_type != target_type
                    || column.nullable != existing.nullable
                    || column.primary_key != existing.primary_key
                    || column.auto_increment != existing.auto_increment
                    || column.default != existing.default
                    || column.generated != existing.generated
                    || column.stored != existing.stored
                    || column.identity != existing.identity
                    || column.identity_start != existing.identity_start
                    || column.identity_increment != existing.identity_increment
                    || column.unsigned != existing.unsigned
                    || column.zerofill != existing.zerofill
                    || column.charset != existing.charset
                    || column.collation != existing.collation
                    || column.comment != existing.comment
                    || column.extra != existing.extra;
                if differs {
                    changes.push(SchemaChange {
                        kind: SchemaChangeKind::Modified,
                        column: column.name.clone(),
                        // Show the attributes that changed, not just the type: a collation- or
                        // comment-only difference otherwise reads `varchar(200) → varchar(200)`.
                        source_type: column_difference(column, existing, &source_type),
                        target_type: column_difference(existing, column, &target_type),
                    });
                }
            }
        }
    }
    for column in target {
        if !source
            .iter()
            .any(|candidate| candidate.name.eq_ignore_ascii_case(&column.name))
        {
            changes.push(SchemaChange {
                kind: SchemaChangeKind::Removed,
                column: column.name.clone(),
                source_type: String::new(),
                target_type: column_type_text(column),
            });
        }
    }
    changes
}

/// Compare two complete table definitions: the columns (see [`diff_columns`]) plus everything the
/// driver's `table_schema_sql` also applies — indexes, unique keys, checks, foreign keys,
/// exclusion constraints, rules, partitioning and the table options. An object-level entry names
/// the object in `column` (`INDEX \`idx\``) and carries no type text.
///
/// Without this, a table whose only difference was an index or a collation reported "no changes",
/// so the sync skipped it while the DDL pane kept highlighting the difference.
pub fn diff_schema(source: &TableSchema, target: &TableSchema) -> Vec<SchemaChange> {
    let mut changes = diff_columns(&source.columns, &target.columns);
    // A primary key is already covered by the `primary_key` flag of each column.
    diff_named(
        &source.indexes,
        &target.indexes,
        |index| index.name.clone(),
        |index| !index.primary,
        "INDEX",
        &mut changes,
    );
    diff_named(
        &source.unique_keys,
        &target.unique_keys,
        |key| key.name.clone(),
        |_| true,
        "UNIQUE",
        &mut changes,
    );
    diff_named(
        &source.checks,
        &target.checks,
        |check| check.name.clone(),
        |_| true,
        "CHECK",
        &mut changes,
    );
    diff_named(
        &source.foreign_keys,
        &target.foreign_keys,
        |key| key.name.clone(),
        |_| true,
        "FOREIGN KEY",
        &mut changes,
    );
    diff_named(
        &source.excludes,
        &target.excludes,
        |exclude| exclude.name.clone(),
        |_| true,
        "EXCLUDE",
        &mut changes,
    );
    diff_named(
        &source.rules,
        &target.rules,
        |rule| rule.name.clone(),
        |_| true,
        "RULE",
        &mut changes,
    );
    if source.partitioning.trim() != target.partitioning.trim() {
        changes.push(SchemaChange {
            kind: SchemaChangeKind::Modified,
            column: "PARTITIONING".to_string(),
            source_type: String::new(),
            target_type: String::new(),
        });
    }
    // `AUTO_INCREMENT` is a counter, not a definition: the server may report a different next
    // value on each side without the tables differing, and lowering it is impossible once the rows
    // exist. Comparing it made such a table look different forever.
    if table_options_differ(&source.options, &target.options) {
        changes.push(SchemaChange {
            kind: SchemaChangeKind::Modified,
            column: "OPTIONS".to_string(),
            source_type: String::new(),
            target_type: String::new(),
        });
    }
    changes
}

/// Add one entry per added, removed or changed named object.
fn diff_named<T: PartialEq>(
    source: &[T],
    target: &[T],
    name: impl Fn(&T) -> String,
    include: impl Fn(&T) -> bool,
    label: &str,
    changes: &mut Vec<SchemaChange>,
) {
    let source: Vec<&T> = source.iter().filter(|item| include(item)).collect();
    let target: Vec<&T> = target.iter().filter(|item| include(item)).collect();
    for item in &source {
        let item_name = name(item);
        match target.iter().find(|candidate| name(candidate) == item_name) {
            None => changes.push(SchemaChange {
                kind: SchemaChangeKind::Added,
                column: format!("{label} `{item_name}`"),
                source_type: String::new(),
                target_type: String::new(),
            }),
            Some(existing) if *existing != *item => changes.push(SchemaChange {
                kind: SchemaChangeKind::Modified,
                column: format!("{label} `{item_name}`"),
                source_type: String::new(),
                target_type: String::new(),
            }),
            Some(_) => {}
        }
    }
    for item in &target {
        let item_name = name(item);
        if !source.iter().any(|candidate| name(candidate) == item_name) {
            changes.push(SchemaChange {
                kind: SchemaChangeKind::Removed,
                column: format!("{label} `{item_name}`"),
                source_type: String::new(),
                target_type: String::new(),
            });
        }
    }
}

/// Whether two tables' storage options differ in a way a sync can apply. The auto-increment
/// counter is deliberately ignored.
fn table_options_differ(source: &TableOptions, target: &TableOptions) -> bool {
    source.engine != target.engine
        || source.charset != target.charset
        || source.collation != target.collation
        || source.comment != target.comment
}

/// The kind of change a row needs on the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowChangeKind {
    Insert,
    Update,
    Delete,
}

impl RowChangeKind {
    /// The i18n key of the row's action badge.
    pub fn label_key(self) -> &'static str {
        match self {
            RowChangeKind::Insert => "sync.action.insert",
            RowChangeKind::Update => "sync.action.update",
            RowChangeKind::Delete => "sync.action.delete",
        }
    }
}

/// One changed column of an updated row. `None` is SQL `NULL`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellChange {
    pub column: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// One row-level difference between the source and target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowChange {
    pub kind: RowChangeKind,
    /// The row's key columns, used by the `WHERE` clause of an update/delete.
    pub keys: Vec<(String, Option<String>)>,
    /// The values to write: every copied column for an insert, the changed columns for an update.
    pub values: Vec<(String, Option<String>)>,
    /// The changed columns with both values, for the inspector's diff display.
    pub changes: Vec<CellChange>,
}

/// The row-level comparison of one table.
#[derive(Debug, Clone, Default)]
pub struct DataDiff {
    /// The key columns the comparison used. Empty when the table has no primary key.
    pub key_columns: Vec<String>,
    pub rows: Vec<RowChange>,
    pub inserts: usize,
    pub updates: usize,
    pub deletes: usize,
    /// Whether one side hit the comparison row cap, so the diff is incomplete.
    pub truncated: bool,
}

impl DataDiff {
    /// The total number of changed rows.
    pub fn total(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Compare the rows of a source and target table by primary key.
///
/// `key_columns` are the source's primary-key column names; a table without one yields an empty
/// diff (the UI offers a full overwrite instead). A row present only in the source becomes an
/// insert, a row present on both sides with differing values becomes an update, and a row present
/// only in the target becomes a delete. `truncated` records that the caller hit its row cap, so the
/// UI can say the comparison is partial.
pub fn diff_rows(
    source_columns: &[ColumnInfo],
    source_rows: &[Vec<CellValue>],
    target_columns: &[ColumnInfo],
    target_rows: &[Vec<CellValue>],
    key_columns: &[String],
    truncated: bool,
) -> DataDiff {
    let mut diff = DataDiff {
        key_columns: key_columns.to_vec(),
        truncated,
        ..Default::default()
    };
    if key_columns.is_empty() {
        return diff;
    }
    let Some(source_indexes) = key_indexes(source_columns, key_columns) else {
        return diff;
    };
    // A table the target does not have (yet) has no columns to key on: every source row is an
    // insert. Once the target does have rows, a missing key column means the two sides are not
    // comparable, so report nothing rather than guessing.
    let target_indexes = match key_indexes(target_columns, key_columns) {
        Some(indexes) => Some(indexes),
        None if target_rows.is_empty() => None,
        None => return diff,
    };

    let mut target_by_key = std::collections::HashMap::with_capacity(target_rows.len());
    if let Some(target_indexes) = target_indexes.as_ref() {
        for (index, row) in target_rows.iter().enumerate() {
            if let Some(key) = key_of(row, target_indexes) {
                target_by_key.insert(key, index);
            }
        }
    }

    let mut seen = std::collections::HashSet::with_capacity(source_rows.len());
    for source_row in source_rows.iter() {
        let Some(key) = key_of(source_row, &source_indexes) else {
            continue;
        };
        seen.insert(key.clone());
        match target_by_key.get(&key) {
            None => {
                diff.inserts += 1;
                diff.rows.push(insert_row_change(
                    source_columns,
                    target_columns,
                    source_row,
                    key_columns,
                ));
            }
            Some(target_index) => {
                let Some(target_row) = target_rows.get(*target_index) else {
                    continue;
                };
                if let Some(change) = update_row_change(
                    source_columns,
                    target_columns,
                    source_row,
                    target_row,
                    key_columns,
                ) {
                    diff.updates += 1;
                    diff.rows.push(change);
                }
            }
        }
    }

    for target_row in target_rows {
        let Some(target_indexes) = target_indexes.as_ref() else {
            break;
        };
        let Some(key) = key_of(target_row, target_indexes) else {
            continue;
        };
        if seen.contains(&key) {
            continue;
        }
        diff.deletes += 1;
        diff.rows
            .push(delete_row_change(target_columns, target_row, key_columns));
    }
    diff
}

/// The change for a source row the target does not have.
pub fn insert_row_change(
    source_columns: &[ColumnInfo],
    target_columns: &[ColumnInfo],
    source_row: &[CellValue],
    key_columns: &[String],
) -> RowChange {
    RowChange {
        kind: RowChangeKind::Insert,
        keys: row_keys(source_columns, source_row, key_columns),
        values: mapped_values(source_columns, target_columns, source_row),
        changes: Vec::new(),
    }
}

/// The change for a row present on both sides, or `None` when every mapped value matches.
pub fn update_row_change(
    source_columns: &[ColumnInfo],
    target_columns: &[ColumnInfo],
    source_row: &[CellValue],
    target_row: &[CellValue],
    key_columns: &[String],
) -> Option<RowChange> {
    let mut changes = Vec::new();
    let mut values = Vec::new();
    for (source_position, column) in source_columns.iter().enumerate() {
        let Some(target_position) = find_column(target_columns, &column.name) else {
            continue;
        };
        let before = display_value(target_row.get(target_position));
        let after = display_value(source_row.get(source_position));
        if before == after {
            continue;
        }
        values.push((
            column.name.clone(),
            source_row
                .get(source_position)
                .and_then(|value| value.as_edit_value()),
        ));
        changes.push(CellChange {
            column: column.name.clone(),
            before,
            after,
        });
    }
    (!changes.is_empty()).then(|| RowChange {
        kind: RowChangeKind::Update,
        keys: row_keys(source_columns, source_row, key_columns),
        values,
        changes,
    })
}

/// The change for a target row the source no longer has.
pub fn delete_row_change(
    target_columns: &[ColumnInfo],
    target_row: &[CellValue],
    key_columns: &[String],
) -> RowChange {
    RowChange {
        kind: RowChangeKind::Delete,
        keys: row_keys(target_columns, target_row, key_columns),
        values: Vec::new(),
        changes: Vec::new(),
    }
}

/// Render one row change as a single SQL statement, for the inspector's preview.
///
/// The values are the edit strings the connection methods would bind, so the preview shows the
/// same data the run writes. `None` renders as `NULL`; string values are quoted per `dialect`.
pub fn render_change_sql(table: &str, change: &RowChange, dialect: DriverDialect) -> String {
    let target = dialect.quote_table_name(table);
    match change.kind {
        RowChangeKind::Insert => {
            let columns = change
                .values
                .iter()
                .map(|(column, _)| dialect.quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ");
            let values = change
                .values
                .iter()
                .map(|(_, value)| render_literal(value, dialect))
                .collect::<Vec<_>>()
                .join(", ");
            format!("INSERT INTO {target} ({columns}) VALUES ({values});")
        }
        RowChangeKind::Update => {
            let assignments = change
                .values
                .iter()
                .map(|(column, value)| {
                    format!(
                        "{} = {}",
                        dialect.quote_identifier(column),
                        render_literal(value, dialect)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "UPDATE {target} SET {assignments} WHERE {};",
                render_keys(&change.keys, dialect)
            )
        }
        RowChangeKind::Delete => format!(
            "DELETE FROM {target} WHERE {};",
            render_keys(&change.keys, dialect)
        ),
    }
}

/// Render a bind value as a SQL literal for the preview.
pub fn render_literal(value: &Option<String>, dialect: DriverDialect) -> String {
    let Some(text) = value else {
        return "NULL".to_string();
    };
    match dialect {
        DriverDialect::Mysql => quote_string(text, true),
        DriverDialect::Sqlite => quote_string(text, false),
        DriverDialect::SqlServer => format!("N{}", quote_string(text, false)),
        DriverDialect::Postgres | DriverDialect::Oracle | DriverDialect::Generic => {
            quote_string(text, false)
        }
    }
}

fn quote_string(value: &str, escape_backslash: bool) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for character in value.chars() {
        match character {
            '\'' => out.push_str("''"),
            '\\' if escape_backslash => out.push_str("\\\\"),
            '\0' if escape_backslash => out.push_str("\\0"),
            '\n' if escape_backslash => out.push_str("\\n"),
            '\r' if escape_backslash => out.push_str("\\r"),
            '\t' if escape_backslash => out.push_str("\\t"),
            _ => out.push(character),
        }
    }
    out.push('\'');
    out
}

fn render_keys(keys: &[(String, Option<String>)], dialect: DriverDialect) -> String {
    if keys.is_empty() {
        return "1 = 0".to_string();
    }
    keys.iter()
        .map(|(column, value)| match value {
            None => format!("{} IS NULL", dialect.quote_identifier(column)),
            Some(_) => format!(
                "{} = {}",
                dialect.quote_identifier(column),
                render_literal(value, dialect)
            ),
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn find_column(columns: &[ColumnInfo], name: &str) -> Option<usize> {
    columns
        .iter()
        .position(|column| column.name == name)
        .or_else(|| {
            columns
                .iter()
                .position(|column| column.name.eq_ignore_ascii_case(name))
        })
}

fn key_indexes(columns: &[ColumnInfo], keys: &[String]) -> Option<Vec<usize>> {
    keys.iter().map(|key| find_column(columns, key)).collect()
}

/// A comparable, collision-resistant representation of one row's key. `NULL` and an empty string
/// are different keys.
fn key_of(row: &[CellValue], indexes: &[usize]) -> Option<String> {
    let mut key = String::new();
    for index in indexes {
        let value = row.get(*index)?;
        key.push('\u{1f}');
        match value.as_edit_value() {
            Some(text) => {
                key.push('v');
                key.push_str(&text);
            }
            None => key.push('n'),
        }
    }
    Some(key)
}

fn row_keys(
    columns: &[ColumnInfo],
    row: &[CellValue],
    keys: &[String],
) -> Vec<(String, Option<String>)> {
    keys.iter()
        .filter_map(|name| {
            let index = find_column(columns, name)?;
            let value = row.get(index)?.as_edit_value();
            Some((name.clone(), value))
        })
        .collect()
}

fn mapped_values(
    source_columns: &[ColumnInfo],
    target_columns: &[ColumnInfo],
    row: &[CellValue],
) -> Vec<(String, Option<String>)> {
    source_columns
        .iter()
        .enumerate()
        .filter(|(_, column)| find_column(target_columns, &column.name).is_some())
        .filter_map(|(index, column)| {
            let value = row.get(index)?.as_edit_value();
            Some((column.name.clone(), value))
        })
        .collect()
}

fn display_value(value: Option<&CellValue>) -> Option<String> {
    match value {
        None | Some(CellValue::Null) => None,
        Some(value) => Some(value.as_display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, data_type: &str, primary_key: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: data_type.to_string(),
            nullable: !primary_key,
            primary_key,
            auto_increment: false,
            generated: false,
            comment: String::new(),
        }
    }

    fn columns() -> Vec<ColumnInfo> {
        vec![
            column("id", "bigint", true),
            column("email", "varchar(255)", false),
            column("role", "varchar(32)", false),
        ]
    }

    fn row(id: i64, email: &str, role: &str) -> Vec<CellValue> {
        vec![
            CellValue::Int(id),
            CellValue::Text(email.to_string()),
            CellValue::Text(role.to_string()),
        ]
    }

    #[test]
    fn key_columns_fall_back_to_a_non_null_unique_key() {
        use crate::model::{IndexColumnDef, IndexDef};

        let mut columns = vec![
            column("id", "bigint", false),
            column("email", "varchar(255)", false),
            column("tenant", "bigint", false),
        ];
        for column in &mut columns {
            column.nullable = false;
        }
        let schema = TableSchema {
            indexes: vec![
                IndexDef {
                    name: "idx_email".to_string(),
                    unique: true,
                    columns: vec![IndexColumnDef::new("email")],
                    ..Default::default()
                },
                IndexDef {
                    name: "idx_tenant".to_string(),
                    unique: true,
                    columns: vec![IndexColumnDef::new("tenant")],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let keys = usable_key_columns(&columns, Some(&schema));
        assert_eq!(keys, vec!["email".to_string()]);

        // A nullable unique column cannot identify a row, so it is skipped.
        let mut nullable = columns.clone();
        nullable[1].nullable = true;
        let keys = usable_key_columns(&nullable, Some(&schema));
        assert_eq!(keys, vec!["tenant".to_string()]);
    }

    #[test]
    fn key_columns_prefer_the_primary_key() {
        let columns = vec![column("id", "bigint", true), column("email", "text", false)];
        let schema = TableSchema {
            unique_keys: vec![crate::model::UniqueKeyDef {
                name: "uq_email".to_string(),
                columns: vec!["email".to_string()],
            }],
            ..Default::default()
        };
        assert_eq!(
            usable_key_columns(&columns, Some(&schema)),
            vec!["id".to_string()]
        );
        // Without a schema there is nothing but the primary key to go on.
        assert!(usable_key_columns(&[column("email", "text", false)], None).is_empty());
    }

    #[test]
    fn row_diff_finds_inserts_updates_and_deletes() {
        let source = vec![
            row(1, "a@example.com", "admin"),
            row(2, "b@example.com", "member"),
        ];
        let target = vec![
            row(1, "a@example.com", "member"),
            row(3, "gone@example.com", "member"),
        ];
        let diff = diff_rows(
            &columns(),
            &source,
            &columns(),
            &target,
            &["id".to_string()],
            false,
        );
        assert_eq!(diff.inserts, 1);
        assert_eq!(diff.updates, 1);
        assert_eq!(diff.deletes, 1);
        assert_eq!(diff.total(), 3);
        let update = diff
            .rows
            .iter()
            .find(|change| change.kind == RowChangeKind::Update)
            .expect("update");
        assert_eq!(update.keys, vec![("id".to_string(), Some("1".to_string()))]);
        assert_eq!(
            update.values,
            vec![("role".to_string(), Some("admin".to_string()))]
        );
        assert_eq!(update.changes[0].before.as_deref(), Some("member"));
        assert_eq!(update.changes[0].after.as_deref(), Some("admin"));
    }

    #[test]
    fn a_missing_target_table_makes_every_row_an_insert() {
        let source = vec![row(1, "a@example.com", "admin")];
        let diff = diff_rows(&columns(), &source, &[], &[], &["id".to_string()], false);
        assert_eq!(diff.inserts, 1);
        assert_eq!(diff.updates, 0);
        assert_eq!(diff.deletes, 0);
    }

    #[test]
    fn row_diff_requires_a_key() {
        let diff = diff_rows(
            &columns(),
            &[row(1, "a", "b")],
            &columns(),
            &[row(2, "c", "d")],
            &[],
            false,
        );
        assert!(diff.is_empty());
        assert!(diff.key_columns.is_empty());
    }

    #[test]
    fn row_diff_treats_null_and_empty_string_as_distinct() {
        let columns = vec![column("id", "bigint", true), column("note", "text", false)];
        let source = vec![vec![CellValue::Int(1), CellValue::Null]];
        let target = vec![vec![CellValue::Int(1), CellValue::Text(String::new())]];
        let diff = diff_rows(
            &columns,
            &source,
            &columns,
            &target,
            &["id".to_string()],
            false,
        );
        assert_eq!(diff.updates, 1);
        assert_eq!(diff.rows[0].values[0].1, None);
    }

    #[test]
    fn schema_diff_reports_added_removed_and_modified_columns() {
        let source = vec![
            ColumnDef {
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                primary_key: true,
                ..Default::default()
            },
            ColumnDef {
                name: "email".to_string(),
                data_type: "varchar".to_string(),
                length: "255".to_string(),
                ..Default::default()
            },
            ColumnDef {
                name: "mfa".to_string(),
                data_type: "varchar".to_string(),
                length: "64".to_string(),
                ..Default::default()
            },
        ];
        let target = vec![
            ColumnDef {
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                primary_key: true,
                ..Default::default()
            },
            ColumnDef {
                name: "email".to_string(),
                data_type: "varchar".to_string(),
                length: "120".to_string(),
                ..Default::default()
            },
            ColumnDef {
                name: "legacy".to_string(),
                data_type: "int".to_string(),
                ..Default::default()
            },
        ];
        let changes = diff_columns(&source, &target);
        assert!(
            changes
                .iter()
                .any(|change| { change.kind == SchemaChangeKind::Added && change.column == "mfa" })
        );
        assert!(changes.iter().any(|change| {
            change.kind == SchemaChangeKind::Removed && change.column == "legacy"
        }));
        assert!(changes.iter().any(|change| {
            change.kind == SchemaChangeKind::Modified && change.column == "email"
        }));
    }

    #[test]
    fn schema_diff_reports_collation_and_comment_changes() {
        let source = vec![ColumnDef {
            name: "note".to_string(),
            data_type: "text".to_string(),
            charset: "utf8mb4".to_string(),
            collation: "utf8mb4_general_ci".to_string(),
            comment: "memo".to_string(),
            ..Default::default()
        }];
        let mut target = source.clone();
        target[0].collation = "utf8mb4_0900_ai_ci".to_string();
        let changes = diff_columns(&source, &target);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].summary().contains("utf8mb4_0900_ai_ci"));
        assert!(changes[0].summary().contains("utf8mb4_general_ci"));

        target[0].collation = "utf8mb4_general_ci".to_string();
        target[0].comment.clear();
        let changes = diff_columns(&source, &target);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].summary().contains("COMMENT 'memo'"));
    }

    #[test]
    fn schema_diff_reports_indexes_and_table_options() {
        use crate::model::{IndexColumnDef, IndexDef};

        let mut source = TableSchema {
            columns: vec![ColumnDef {
                name: "id".to_string(),
                data_type: "int".to_string(),
                primary_key: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        source.indexes.push(IndexDef {
            name: "idx_name".to_string(),
            columns: vec![IndexColumnDef {
                name: "name".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        });
        source.options.comment = "source comment".to_string();
        let mut target = source.clone();
        target.indexes.clear();
        target.options.comment.clear();

        let changes = diff_schema(&source, &target);
        assert!(changes.iter().any(|change| {
            change.kind == SchemaChangeKind::Added && change.column == "INDEX `idx_name`"
        }));
        assert!(changes.iter().any(|change| {
            change.kind == SchemaChangeKind::Modified && change.column == "OPTIONS"
        }));
        assert!(
            changes
                .iter()
                .any(|change| change.summary() == "+ INDEX `idx_name`")
        );

        // The auto-increment counter alone is not a definition difference.
        let mut other = target.clone();
        other.options.auto_increment = "99".to_string();
        assert!(diff_schema(&other, &target).is_empty());
    }

    #[test]
    fn sql_preview_quotes_identifiers_and_literals() {
        let change = RowChange {
            kind: RowChangeKind::Update,
            keys: vec![("id".to_string(), Some("7".to_string()))],
            values: vec![("name".to_string(), Some("O'Brien".to_string()))],
            changes: Vec::new(),
        };
        let sql = render_change_sql("users", &change, DriverDialect::Mysql);
        assert_eq!(
            sql,
            "UPDATE `users` SET `name` = 'O''Brien' WHERE `id` = '7';"
        );
        let delete = RowChange {
            kind: RowChangeKind::Delete,
            keys: vec![("id".to_string(), None)],
            values: Vec::new(),
            changes: Vec::new(),
        };
        assert_eq!(
            render_change_sql("users", &delete, DriverDialect::SqlServer),
            "DELETE FROM [users] WHERE [id] IS NULL;"
        );
        // A schema-qualified name is quoted one part at a time, so the dot stays a separator.
        assert_eq!(
            render_change_sql("public.users", &delete, DriverDialect::Postgres),
            "DELETE FROM \"public\".\"users\" WHERE \"id\" IS NULL;"
        );
    }
}
