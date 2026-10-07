//! Oracle dialect helpers: identifier/literal quoting, qualified-name handling, `:n` placeholders,
//! script splitting (PL/SQL blocks, `q'[...]'` quotes) and value decoding. Kept free of any app
//! dependency so it can be unit tested without a server.

use oracledb::{
    JsonValue, Metadata, OracleIntervalDS, OracleIntervalYM, OracleNumber, OracleTimestamp, Row,
    Vector,
};
use rustgrid_core::{
    CellValue, ColumnInfo, FilterCondition, FilterConjunction, FilterNode, FilterOperator,
};

/// Quote an identifier with double quotes, escaping an embedded `"` as `""`.
///
/// Oracle folds an unquoted identifier to upper case, so the catalog's own spelling (already upper
/// case for objects created without quotes) must be preserved verbatim — never lower-cased.
pub(crate) fn quote_identifier(identifier: &str) -> String {
    let mut out = String::with_capacity(identifier.len() + 2);
    out.push('"');
    for character in identifier.chars() {
        if character == '"' {
            out.push('"');
        }
        out.push(character);
    }
    out.push('"');
    out
}

/// Quote a string as a standard `'...'` literal, escaping an embedded `'` as `''`.
pub(crate) fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// A fully qualified `"schema"."object"` name.
pub(crate) fn qualify(schema: &str, name: &str) -> String {
    format!("{}.{}", quote_identifier(schema), quote_identifier(name))
}

/// The positional placeholder for the `index`-th bound parameter (1-based), matching `oracledb`'s
/// `:1`/`:2` binding style.
pub(crate) fn placeholder(index: usize) -> String {
    format!(":{index}")
}

/// Split an optional `schema.object` name at the first dot, returning the schema and the remainder.
/// A name without a usable prefix (or with an empty part) is returned whole with `None`.
pub(crate) fn split_qualified(name: &str) -> Option<(&str, &str)> {
    match name.split_once('.') {
        Some((schema, object)) if !schema.is_empty() && !object.is_empty() => {
            Some((schema, object))
        }
        _ => None,
    }
}

/// Resolve a possibly schema-qualified object name into `(schema, bare_name)`, falling back to the
/// connection's default schema when the name carries no schema prefix.
pub(crate) fn resolve_object(default_schema: &str, name: &str) -> (String, String) {
    match split_qualified(name) {
        Some((schema, object)) => (schema.to_string(), object.to_string()),
        None => (default_schema.to_string(), name.to_string()),
    }
}

/// The first keyword of a statement, skipping leading whitespace and comments.
pub(crate) fn leading_keyword(sql: &str) -> String {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(stripped) = rest.strip_prefix("--") {
            rest = stripped
                .split_once('\n')
                .map(|(_, after)| after)
                .unwrap_or("");
        } else if let Some(stripped) = rest.strip_prefix("/*") {
            rest = stripped
                .split_once("*/")
                .map(|(_, after)| after)
                .unwrap_or("");
        } else {
            break;
        }
    }
    rest.chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Whether a statement is expected to produce a result set. Oracle cannot tell the client ahead of
/// time (POST over the wire), so the leading keyword decides.
pub(crate) fn returns_result_set(sql: &str) -> bool {
    let keyword = leading_keyword(sql);
    if matches!(
        keyword.as_str(),
        "SELECT" | "WITH" | "VALUES" | "TABLE" | "EXPLAIN" | "DESCRIBE" | "DESC"
    ) {
        return true;
    }
    matches!(keyword.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE")
        && sql.to_ascii_uppercase().contains("RETURNING")
}

/// Remove one trailing statement terminator (`;`) plus surrounding whitespace. `oracledb` rejects a
/// plain SQL statement that still carries its terminator.
pub(crate) fn strip_trailing_semicolon(statement: &str) -> String {
    let trimmed = statement.trim();
    let trimmed = trimmed
        .strip_suffix(';')
        .map(str::trim_end)
        .unwrap_or(trimmed);
    trimmed.to_string()
}

/// Prepare a statement for `oracledb`: a plain SQL statement loses its trailing `;`, a PL/SQL block
/// keeps (or gains) it, since the closing `END;` is part of the block.
pub(crate) fn prepare_statement(statement: &str) -> String {
    let trimmed = statement.trim();
    if is_plsql_start(trimmed) {
        if trimmed.ends_with(';') {
            trimmed.to_string()
        } else {
            format!("{trimmed};")
        }
    } else {
        strip_trailing_semicolon(trimmed)
    }
}

/// Whether a statement begins a PL/SQL block: an anonymous block (`BEGIN`/`DECLARE`) or a
/// `CREATE [OR REPLACE] [EDITIONABLE|NONEDITIONABLE] PROCEDURE|FUNCTION|PACKAGE [BODY]|TRIGGER|TYPE`
/// definition. Such a statement contains semicolons of its own and must not be split at them.
///
/// The object kind is compared as a whole word after the `CREATE` modifiers, so a table or column
/// whose name merely contains `TYPE`/`FUNCTION`/`PROCEDURE` (e.g. `user_types`, `type_id`,
/// `order_procedure_log`) is not mistaken for a PL/SQL definition.
pub(crate) fn is_plsql_start(sql: &str) -> bool {
    let keyword = leading_keyword(sql);
    if keyword == "BEGIN" || keyword == "DECLARE" {
        return true;
    }
    if keyword != "CREATE" {
        return false;
    }
    let mut position = 0usize;
    let mut saw_create = false;
    // The object kind is one of the first few words; bound the scan so a large body is not walked.
    for _ in 0..8 {
        let Some((word, next)) = next_word(sql, position) else {
            return false;
        };
        position = next;
        if !saw_create {
            saw_create = true;
            if word != "CREATE" {
                return false;
            }
            continue;
        }
        match word.as_str() {
            "OR" | "REPLACE" | "NO" | "FORCE" | "EDITIONABLE" | "NONEDITIONABLE" => continue,
            "PROCEDURE" | "FUNCTION" | "PACKAGE" | "TRIGGER" | "TYPE" => return true,
            _ => return false,
        }
    }
    false
}

/// The next SQL word at or after `from`, uppercased, with the index just past it. Whitespace,
/// comments and other non-word characters are skipped.
fn next_word(sql: &str, from: usize) -> Option<(String, usize)> {
    let bytes = sql.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            byte if byte.is_ascii_whitespace() => i += 1,
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            byte if is_word_start(byte) => {
                let (word, end) = read_word(sql, i);
                return Some((word.to_ascii_uppercase(), end));
            }
            _ => i += 1,
        }
    }
    None
}

/// Split a SQL script into top-level statements on semicolons, respecting string literals, quoted
/// identifiers, `q'[...]'` alternative quotes, comments and PL/SQL blocks (which contain their own
/// semicolons and must be sent whole).
///
/// A standalone `/` on its own line (SQL\*Plus's terminator) also ends a statement and is dropped.
/// If a string or comment is left unterminated the whole script is returned as one statement.
pub(crate) fn split_statements(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut statements: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut depth: i32 = 0;
    let mut balanced = true;

    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i += 1;
                let mut closed = false;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if i + 1 < bytes.len() && bytes[i + 1] == quote {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                if !closed {
                    balanced = false;
                }
            }
            b'n' | b'N' if bytes.get(i + 1) == Some(&b'\'') => {
                // National string literal: the quote machinery below handles it once we step on.
                i += 1;
            }
            b'q' | b'Q' if bytes.get(i + 1) == Some(&b'\'') => match skip_q_quote(bytes, i) {
                Some(end) => i = end,
                None => {
                    balanced = false;
                    i = bytes.len();
                }
            },
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                let mut closed = false;
                while i + 1 < bytes.len() {
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        i += 2;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                if !closed {
                    balanced = false;
                }
            }
            b'/' if is_line_terminator(bytes, i) => {
                push_fragment(&mut statements, &sql[start..i]);
                // Skip to the end of the line.
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                start = i;
                depth = 0;
            }
            b';' => {
                let fragment = &sql[start..i];
                let next_starts = next_token_starts_statement(bytes, i + 1);
                let plsql = depth > 0 || is_plsql_start(fragment);
                if depth <= 0 && (!plsql || next_starts) {
                    // A plain SQL statement must not carry its terminator; a PL/SQL block needs the
                    // closing `END;` kept intact.
                    if plsql {
                        push_fragment(&mut statements, &sql[start..=i]);
                    } else {
                        push_fragment(&mut statements, fragment);
                    }
                    start = i + 1;
                }
                depth = 0;
                i += 1;
            }
            byte if is_word_start(byte) => {
                let (word, end) = read_word(sql, i);
                match word.to_ascii_uppercase().as_str() {
                    "BEGIN" | "CASE" | "LOOP" | "IF" => depth += 1,
                    "END" => depth = (depth - 1).max(0),
                    _ => {}
                }
                i = end;
            }
            _ => i += 1,
        }
    }

    if !balanced {
        return vec![sql.trim().to_string()];
    }
    push_fragment(&mut statements, &sql[start..]);
    statements
}

/// Push a non-empty, non-comment statement fragment.
fn push_fragment(statements: &mut Vec<String>, fragment: &str) {
    let trimmed = fragment.trim();
    if has_sql_content(trimmed) {
        statements.push(trimmed.to_string());
    }
}

/// Whether `position` starts a standalone `/` line (SQL\*Plus statement terminator).
fn is_line_terminator(bytes: &[u8], position: usize) -> bool {
    let line_start = bytes[..position]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    if bytes[line_start..position]
        .iter()
        .any(|byte| !byte.is_ascii_whitespace())
    {
        return false;
    }
    let rest = &bytes[position + 1..];
    let line_end = rest
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(rest.len());
    rest[..line_end]
        .iter()
        .all(|byte| byte.is_ascii_whitespace())
}

/// Skip a `q'X...X'` alternative-quoted string starting at `index` (`q'`), returning the index just
/// past the closing `'`. Bracket delimiters pair (`q'[a]'` -> `]`), others repeat (`q'!a!'` -> `!`).
fn skip_q_quote(bytes: &[u8], index: usize) -> Option<usize> {
    let delimiter = *bytes.get(index + 2)?;
    let closer = match delimiter {
        b'[' => b']',
        b'{' => b'}',
        b'(' => b')',
        b'<' => b'>',
        other => other,
    };
    let mut i = index + 3;
    while i + 1 < bytes.len() {
        if bytes[i] == closer && bytes[i + 1] == b'\'' {
            return Some(i + 2);
        }
        i += 1;
    }
    None
}

/// Whether the byte can start a SQL word (used to count `BEGIN`/`END` and friends).
fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Read the SQL word starting at `index`, returning it and the index just past it.
fn read_word(sql: &str, index: usize) -> (String, usize) {
    let bytes = sql.as_bytes();
    let mut end = index;
    while end < bytes.len() && is_word_char(bytes[end]) {
        end += 1;
    }
    (sql[index..end].to_string(), end)
}

/// Whether the next meaningful token after `position` starts a new statement (or there is none).
/// Used to tell a package body's inner `END q;` apart from its terminating `END p;`.
fn next_token_starts_statement(bytes: &[u8], position: usize) -> bool {
    let mut i = position;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < bytes.len() && bytes[i] == b'-' && bytes[i + 1] == b'-' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        break;
    }
    if i >= bytes.len() {
        return true;
    }
    if !is_word_start(bytes[i]) {
        // A `/` terminator or anything else non-word: treat as the end.
        return true;
    }
    let mut end = i;
    while end < bytes.len() && is_word_char(bytes[end]) {
        end += 1;
    }
    matches!(
        String::from_utf8_lossy(&bytes[i..end])
            .to_ascii_uppercase()
            .as_str(),
        "BEGIN"
            | "DECLARE"
            | "SELECT"
            | "WITH"
            | "INSERT"
            | "UPDATE"
            | "DELETE"
            | "MERGE"
            | "CREATE"
            | "ALTER"
            | "DROP"
            | "TRUNCATE"
            | "GRANT"
            | "REVOKE"
            | "COMMIT"
            | "ROLLBACK"
            | "SAVEPOINT"
            | "SET"
            | "CALL"
            | "EXECUTE"
            | "EXEC"
            | "COMMENT"
            | "ANALYZE"
            | "LOCK"
            | "RENAME"
            | "PURGE"
            | "FLASHBACK"
            | "AUDIT"
            | "NOAUDIT"
    )
}

/// Whether a SQL fragment contains anything besides whitespace and comments.
fn has_sql_content(fragment: &str) -> bool {
    let bytes = fragment.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            byte if byte.is_ascii_whitespace() => i += 1,
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            _ => return true,
        }
    }
    false
}

/// The `ORDER BY` clause for a sort request, or an empty string when unsorted.
pub(crate) fn order_clause(order_by: &[rustgrid_core::SortColumn]) -> String {
    if order_by.is_empty() {
        return String::new();
    }
    let terms = order_by
        .iter()
        .map(|sort| {
            format!(
                "{} {}",
                quote_identifier(&sort.column),
                if sort.descending { "DESC" } else { "ASC" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(" ORDER BY {terms}")
}

/// A `:n` placeholder, wrapped in a conversion function when the target column is temporal so the
/// text value does not depend on the session's `NLS_DATE_FORMAT`.
pub(crate) fn bind_expression(index: usize, data_type: &str) -> String {
    let base = data_type.trim().to_ascii_uppercase();
    let placeholder = placeholder(index);
    if base.starts_with("TIMESTAMP") {
        format!("TO_TIMESTAMP({placeholder}, 'YYYY-MM-DD HH24:MI:SS.FF6')")
    } else if base.starts_with("DATE") {
        format!("TO_DATE({placeholder}, 'YYYY-MM-DD HH24:MI:SS')")
    } else {
        placeholder
    }
}

fn column_type<'a>(columns: &'a [ColumnInfo], name: &str) -> &'a str {
    columns
        .iter()
        .find(|column| column.name == name)
        .map(|column| column.data_type.as_str())
        .unwrap_or("")
}

fn bind_placeholder(values: &mut Vec<String>, value: String, data_type: &str) -> String {
    values.push(value);
    bind_expression(values.len(), data_type)
}

/// Translate the filter tree into a `WHERE` fragment plus its ordered bind values, in the same
/// order as the `:n` placeholders.
pub(crate) fn filter_clause(
    filter: &[FilterNode],
    columns: &[ColumnInfo],
) -> (String, Vec<String>) {
    let mut values: Vec<String> = Vec::new();
    match filter_fragment(filter, columns, &mut values) {
        Some(expression) => (format!(" WHERE {expression}"), values),
        None => (String::new(), Vec::new()),
    }
}

fn filter_fragment(
    filter: &[FilterNode],
    columns: &[ColumnInfo],
    values: &mut Vec<String>,
) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    for node in filter {
        let piece = match node {
            FilterNode::Condition(condition) => condition_piece(condition, columns, values),
            FilterNode::Group(group) => {
                if !group.enabled {
                    None
                } else {
                    filter_fragment(&group.children, columns, values)
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

fn condition_piece(
    condition: &FilterCondition,
    columns: &[ColumnInfo],
    values: &mut Vec<String>,
) -> Option<String> {
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

    let column = quote_identifier(&condition.column);
    let data_type = column_type(columns, &condition.column).to_string();
    let piece = match operator {
        FilterOperator::Equal => {
            let value = bind_placeholder(values, condition.value.clone(), &data_type);
            format!("{column} = {value}")
        }
        FilterOperator::NotEqual => {
            let value = bind_placeholder(values, condition.value.clone(), &data_type);
            format!("{column} <> {value}")
        }
        FilterOperator::LessThan => {
            let value = bind_placeholder(values, condition.value.clone(), &data_type);
            format!("{column} < {value}")
        }
        FilterOperator::LessOrEqual => {
            let value = bind_placeholder(values, condition.value.clone(), &data_type);
            format!("{column} <= {value}")
        }
        FilterOperator::GreaterThan => {
            let value = bind_placeholder(values, condition.value.clone(), &data_type);
            format!("{column} > {value}")
        }
        FilterOperator::GreaterOrEqual => {
            let value = bind_placeholder(values, condition.value.clone(), &data_type);
            format!("{column} >= {value}")
        }
        FilterOperator::Contains => {
            let value = bind_placeholder(values, format!("%{}%", condition.value), "");
            format!("{column} LIKE {value}")
        }
        FilterOperator::NotContains => {
            let value = bind_placeholder(values, format!("%{}%", condition.value), "");
            format!("{column} NOT LIKE {value}")
        }
        FilterOperator::StartsWith => {
            let value = bind_placeholder(values, format!("{}%", condition.value), "");
            format!("{column} LIKE {value}")
        }
        FilterOperator::NotStartsWith => {
            let value = bind_placeholder(values, format!("{}%", condition.value), "");
            format!("{column} NOT LIKE {value}")
        }
        FilterOperator::EndsWith => {
            let value = bind_placeholder(values, format!("%{}", condition.value), "");
            format!("{column} LIKE {value}")
        }
        FilterOperator::NotEndsWith => {
            let value = bind_placeholder(values, format!("%{}", condition.value), "");
            format!("{column} NOT LIKE {value}")
        }
        // Oracle has no empty string: `''` is stored as `NULL`, so "empty" means "null".
        FilterOperator::IsNull | FilterOperator::IsEmpty => format!("{column} IS NULL"),
        FilterOperator::IsNotNull | FilterOperator::IsNotEmpty => format!("{column} IS NOT NULL"),
        FilterOperator::Between => {
            let first = bind_placeholder(values, condition.value.clone(), &data_type);
            let second = bind_placeholder(values, condition.value2.clone(), &data_type);
            format!("{column} BETWEEN {first} AND {second}")
        }
        FilterOperator::NotBetween => {
            let first = bind_placeholder(values, condition.value.clone(), &data_type);
            let second = bind_placeholder(values, condition.value2.clone(), &data_type);
            format!("{column} NOT BETWEEN {first} AND {second}")
        }
        FilterOperator::InList | FilterOperator::NotInList => {
            let list = condition.list_values();
            if list.is_empty() {
                return None;
            }
            let placeholders = list
                .into_iter()
                .map(|value| bind_placeholder(values, value, &data_type))
                .collect::<Vec<_>>()
                .join(", ");
            if operator == FilterOperator::InList {
                format!("{column} IN ({placeholders})")
            } else {
                format!("{column} NOT IN ({placeholders})")
            }
        }
    };
    Some(piece)
}

/// The display type for a column, dropping a `(p[,s])` qualifier and collapsing a `TIMESTAMP(...)`
/// family to `TIMESTAMP` so the UI's temporal detection (`data_type == "timestamp"`) matches.
pub(crate) fn display_type(data_type: &str) -> String {
    let upper = data_type.trim().to_ascii_uppercase();
    if upper.starts_with("TIMESTAMP") {
        return "TIMESTAMP".to_string();
    }
    match data_type.split_once('(') {
        Some((head, _)) => head.trim().to_string(),
        None => data_type.trim().to_string(),
    }
}

/// Map an `oracledb` column's metadata to a [`ColumnInfo`] (no primary-key/comment knowledge; the
/// catalog query supplies those where needed).
pub(crate) fn metadata_column(metadata: &Metadata) -> ColumnInfo {
    ColumnInfo {
        name: metadata.name().to_string(),
        data_type: display_type(&metadata.data_type()),
        nullable: metadata.nullable(),
        primary_key: false,
        comment: String::new(),
    }
}

/// Decode one cell of a row into a [`CellValue`], using the column's reported type.
pub(crate) fn decode_cell(row: &Row, index: usize) -> CellValue {
    let metadata = row.columns().get(index);
    if let Some(metadata) = metadata {
        let db_type = metadata.db_type();
        if db_type == oracledb::DB_TYPE_NUMBER {
            if let Ok(Some(number)) = row.get::<Option<OracleNumber>>(index) {
                return number_cell(number, metadata.scale());
            }
        } else if db_type == oracledb::DB_TYPE_BINARY_FLOAT {
            if let Ok(Some(value)) = row.get::<Option<f32>>(index) {
                return CellValue::Float(f64::from(value));
            }
        } else if db_type == oracledb::DB_TYPE_BINARY_DOUBLE {
            if let Ok(Some(value)) = row.get::<Option<f64>>(index) {
                return CellValue::Float(value);
            }
        } else if db_type == oracledb::DB_TYPE_BOOLEAN {
            if let Ok(Some(value)) = row.get::<Option<bool>>(index) {
                return CellValue::Bool(value);
            }
        } else if db_type.is_binary_type() {
            if let Ok(Some(value)) = row.get::<Option<Vec<u8>>>(index) {
                return CellValue::Bytes(value);
            }
        } else if db_type.is_date_type() {
            if let Ok(Some(value)) = row.get::<Option<OracleTimestamp>>(index) {
                return CellValue::Text(value.to_string());
            }
        } else if db_type == oracledb::DB_TYPE_INTERVAL_DS {
            if let Ok(Some(value)) = row.get::<Option<OracleIntervalDS>>(index) {
                return CellValue::Text(value.to_string());
            }
        } else if db_type == oracledb::DB_TYPE_INTERVAL_YM {
            if let Ok(Some(value)) = row.get::<Option<OracleIntervalYM>>(index) {
                return CellValue::Text(value.to_string());
            }
        } else if db_type == oracledb::DB_TYPE_JSON
            && let Ok(Some(value)) = row.get::<Option<JsonValue>>(index)
        {
            return CellValue::Text(format!("{value:?}"));
        } else if db_type == oracledb::DB_TYPE_VECTOR
            && let Ok(Some(value)) = row.get::<Option<Vector>>(index)
        {
            return CellValue::Text(format!("{value:?}"));
        } else if db_type.is_string_type()
            && let Ok(Some(value)) = row.get::<Option<String>>(index)
        {
            return CellValue::Text(value);
        }
    }

    // Unknown type (or the type-specific decode failed): try the common representations in order.
    if let Ok(Some(value)) = row.get::<Option<String>>(index) {
        return CellValue::Text(value);
    }
    if let Ok(Some(value)) = row.get::<Option<Vec<u8>>>(index) {
        return CellValue::Bytes(value);
    }
    if let Ok(Some(value)) = row.get::<Option<OracleTimestamp>>(index) {
        return CellValue::Text(value.to_string());
    }
    if let Ok(Some(value)) = row.get::<Option<OracleNumber>>(index) {
        return CellValue::Text(value.to_string());
    }
    if let Ok(Some(value)) = row.get::<Option<f64>>(index) {
        return CellValue::Float(value);
    }
    if let Ok(Some(value)) = row.get::<Option<bool>>(index) {
        return CellValue::Bool(value);
    }
    CellValue::Null
}

/// A NUMBER: an integral value (scale ≤ 0) that fits `i64` becomes [`CellValue::Int`], everything
/// else keeps its exact decimal string (never `f64`, which would lose precision).
pub(crate) fn number_cell(number: OracleNumber, scale: i8) -> CellValue {
    let text = number.to_string();
    if scale <= 0
        && let Ok(value) = text.parse::<i64>()
    {
        return CellValue::Int(value);
    }
    CellValue::Text(text)
}

/// Render one decoded cell as an Oracle SQL literal, for the backup container and `.sql` export.
pub(crate) fn render_literal(value: &CellValue) -> String {
    match value {
        CellValue::Null => "NULL".to_string(),
        // Oracle 23ai accepts `0`/`1` for a BOOLEAN column; matching the exporter's numeric form
        // keeps the two Oracle SQL renderers consistent.
        CellValue::Bool(value) => u8::from(*value).to_string(),
        CellValue::Int(value) => value.to_string(),
        CellValue::Uint(value) => value.to_string(),
        CellValue::Float(value) => {
            if value.is_finite() {
                value.to_string()
            } else {
                "NULL".to_string()
            }
        }
        // Oracle cannot store an empty string: `''` is `NULL`. Render it as `NULL` so a replay
        // keeps the same semantics.
        CellValue::Text(text) if text.is_empty() => "NULL".to_string(),
        CellValue::Text(text) => quote_literal(text),
        CellValue::Bytes(bytes) => {
            if bytes.is_empty() {
                return "NULL".to_string();
            }
            let mut hex = String::with_capacity(bytes.len() * 2 + 10);
            hex.push_str("HEXTORAW('");
            for byte in bytes {
                hex.push_str(&format!("{byte:02X}"));
            }
            hex.push_str("')");
            hex
        }
    }
}

/// Render one cell of `row` as an Oracle SQL literal suitable for replay. A temporal column is
/// wrapped in `TO_DATE`/`TO_TIMESTAMP`/`TO_TIMESTAMP_TZ` (built from the value's own components)
/// so the result does not depend on the session's `NLS_DATE_FORMAT`/`NLS_TIMESTAMP_FORMAT`.
pub(crate) fn render_cell_literal(row: &Row, index: usize) -> String {
    if let Some(metadata) = row.columns().get(index)
        && metadata.db_type().is_date_type()
        && let Ok(Some(value)) = row.get::<Option<OracleTimestamp>>(index)
    {
        return render_temporal_literal(&value, &metadata.data_type());
    }
    render_literal(&decode_cell(row, index))
}

/// Render an `OracleTimestamp` as an NLS-independent Oracle literal for its declared type.
fn render_temporal_literal(value: &OracleTimestamp, data_type: &str) -> String {
    let base = data_type.trim().to_ascii_uppercase();
    if !base.starts_with("TIMESTAMP") {
        let datetime = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            value.year(),
            value.month(),
            value.day(),
            value.hour(),
            value.minute(),
            value.second()
        );
        return format!("TO_DATE('{datetime}', 'YYYY-MM-DD HH24:MI:SS')");
    }
    let datetime = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:09}",
        value.year(),
        value.month(),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
        value.nanoseconds()
    );
    if base.contains("TIME ZONE") {
        let hour = i16::from(value.tz_hour_offset());
        let (sign, hour) = if hour < 0 { ('-', -hour) } else { ('+', hour) };
        let minute = value.tz_minute_offset().unsigned_abs();
        format!(
            "TO_TIMESTAMP_TZ('{datetime} {sign}{hour:02}:{minute:02}', \
             'YYYY-MM-DD HH24:MI:SS.FF9 TZH:TZM')"
        )
    } else {
        format!("TO_TIMESTAMP('{datetime}', 'YYYY-MM-DD HH24:MI:SS.FF9')")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgrid_core::{FilterCondition, SortColumn};

    fn column(name: &str, data_type: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: data_type.to_string(),
            nullable: true,
            primary_key: false,
            comment: String::new(),
        }
    }

    #[test]
    fn quotes_identifiers_verbatim() {
        assert_eq!(quote_identifier("USERS"), "\"USERS\"");
        assert_eq!(quote_identifier("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(quote_identifier("users"), "\"users\"");
    }

    #[test]
    fn resolves_qualified_names() {
        assert_eq!(
            resolve_object("SYS", "USERS"),
            ("SYS".into(), "USERS".into())
        );
        assert_eq!(
            resolve_object("SYS", "APP.USERS"),
            ("APP".into(), "USERS".into())
        );
        assert_eq!(
            resolve_object("SYS", "APP.PKG.F"),
            ("APP".into(), "PKG.F".into())
        );
    }

    #[test]
    fn splits_simple_statements_and_strips_terminators() {
        assert_eq!(
            split_statements("SELECT 1 FROM dual; SELECT 2 FROM dual;"),
            vec!["SELECT 1 FROM dual", "SELECT 2 FROM dual"]
        );
        assert_eq!(strip_trailing_semicolon("SELECT 1;"), "SELECT 1");
        assert_eq!(strip_trailing_semicolon("SELECT 1 ;  "), "SELECT 1");
    }

    #[test]
    fn prepares_statements_for_the_driver() {
        assert_eq!(
            prepare_statement("SELECT 1 FROM dual;"),
            "SELECT 1 FROM dual"
        );
        assert_eq!(prepare_statement("BEGIN NULL; END;"), "BEGIN NULL; END;");
        assert_eq!(prepare_statement("BEGIN NULL; END"), "BEGIN NULL; END;");
    }

    #[test]
    fn does_not_split_inside_strings_comments_or_q_quotes() {
        assert_eq!(
            split_statements("SELECT ';' FROM dual -- ; x\n; SELECT 2 FROM dual"),
            vec!["SELECT ';' FROM dual -- ; x", "SELECT 2 FROM dual"]
        );
        assert_eq!(
            split_statements("SELECT q'[a;b]' FROM dual; SELECT 2 FROM dual"),
            vec!["SELECT q'[a;b]' FROM dual", "SELECT 2 FROM dual"]
        );
        assert_eq!(
            split_statements("SELECT q'!x;y!' FROM dual; SELECT 2 FROM dual"),
            vec!["SELECT q'!x;y!' FROM dual", "SELECT 2 FROM dual"]
        );
        assert_eq!(
            split_statements("SELECT q'{a;b}' FROM dual; SELECT 2 FROM dual"),
            vec!["SELECT q'{a;b}' FROM dual", "SELECT 2 FROM dual"]
        );
    }

    #[test]
    fn keeps_plsql_blocks_whole() {
        // A PL/SQL block keeps its closing `END;` (the driver needs it); plain SQL keeps none.
        let block = "BEGIN INSERT INTO t VALUES (1); COMMIT; END;";
        assert_eq!(split_statements(block), vec![block]);
        let procedure = "CREATE OR REPLACE PROCEDURE p AS BEGIN NULL; END;";
        assert_eq!(
            split_statements(procedure),
            vec!["CREATE OR REPLACE PROCEDURE p AS BEGIN NULL; END;"]
        );
        let with_following = "CREATE OR REPLACE FUNCTION f RETURN NUMBER AS BEGIN RETURN 1; END;\nSELECT 1 FROM dual";
        assert_eq!(
            split_statements(with_following),
            vec![
                "CREATE OR REPLACE FUNCTION f RETURN NUMBER AS BEGIN RETURN 1; END;",
                "SELECT 1 FROM dual"
            ]
        );
    }

    #[test]
    fn does_not_mistake_end_if_for_a_block_end() {
        let block = "BEGIN IF x THEN NULL; END IF; END;";
        let pieces = split_statements(block);
        assert_eq!(pieces.len(), 1);
        assert!(pieces[0].ends_with("END;"));
    }

    #[test]
    fn treats_standalone_slash_as_a_terminator() {
        assert_eq!(
            split_statements("SELECT 1 FROM dual\n/\nSELECT 2 FROM dual"),
            vec!["SELECT 1 FROM dual", "SELECT 2 FROM dual"]
        );
    }

    #[test]
    fn translates_filters_in_placeholder_order() {
        let columns = vec![column("NAME", "VARCHAR2"), column("AGE", "NUMBER")];
        let mut equals = FilterCondition::new("NAME");
        equals.value = "ann".to_string();
        let mut between = FilterCondition::new("AGE");
        between.operator = FilterOperator::Between;
        between.value = "18".to_string();
        between.value2 = "30".to_string();
        let (clause, values) = filter_clause(
            &[
                FilterNode::Condition(equals),
                FilterNode::Condition(between),
            ],
            &columns,
        );
        assert_eq!(clause, " WHERE \"NAME\" = :1 AND \"AGE\" BETWEEN :2 AND :3");
        assert_eq!(values, vec!["ann", "18", "30"]);
    }

    #[test]
    fn wraps_temporal_binds() {
        let columns = vec![column("HIRED", "DATE")];
        let mut condition = FilterCondition::new("HIRED");
        condition.value = "2025-01-02".to_string();
        let (clause, values) = filter_clause(&[FilterNode::Condition(condition)], &columns);
        assert_eq!(
            clause,
            " WHERE \"HIRED\" = TO_DATE(:1, 'YYYY-MM-DD HH24:MI:SS')"
        );
        assert_eq!(values, vec!["2025-01-02"]);
    }

    #[test]
    fn orders_by_quoted_columns() {
        let order = order_clause(&[
            SortColumn {
                column: "NAME".into(),
                descending: false,
            },
            SortColumn {
                column: "AGE".into(),
                descending: true,
            },
        ]);
        assert_eq!(order, " ORDER BY \"NAME\" ASC, \"AGE\" DESC");
    }

    #[test]
    fn renders_oracle_literals() {
        assert_eq!(
            render_literal(&CellValue::Text("O'Brien".into())),
            "'O''Brien'"
        );
        assert_eq!(render_literal(&CellValue::Text(String::new())), "NULL");
        assert_eq!(
            render_literal(&CellValue::Bytes(vec![0x0a, 0xff])),
            "HEXTORAW('0AFF')"
        );
        assert_eq!(render_literal(&CellValue::Null), "NULL");
        assert_eq!(render_literal(&CellValue::Int(42)), "42");
        // A BOOLEAN renders numerically, matching the `.sql` exporter.
        assert_eq!(render_literal(&CellValue::Bool(true)), "1");
        assert_eq!(render_literal(&CellValue::Bool(false)), "0");
    }

    #[test]
    fn does_not_mistake_create_table_names_for_plsql() {
        assert!(!is_plsql_start("CREATE TABLE user_types (id NUMBER)"));
        assert!(!is_plsql_start(
            "CREATE TABLE t (type_id NUMBER, function_id NUMBER)"
        ));
        assert!(!is_plsql_start(
            "CREATE TABLE order_procedure_log (id NUMBER)"
        ));
        assert!(!is_plsql_start("CREATE TABLE package_log (id NUMBER)"));
        // A genuine definition is still recognized, modifiers and all.
        assert!(is_plsql_start("CREATE OR REPLACE PACKAGE BODY pkg AS END;"));
        assert!(is_plsql_start(
            "CREATE OR REPLACE EDITIONABLE TYPE t AS OBJECT (x NUMBER);"
        ));
        assert!(is_plsql_start(
            "CREATE FUNCTION f RETURN NUMBER AS BEGIN RETURN 1; END;"
        ));
    }

    #[test]
    fn wraps_temporal_literals_for_replay() {
        let date = OracleTimestamp::new_date(2025, 1, 2);
        assert_eq!(
            render_temporal_literal(&date, "DATE"),
            "TO_DATE('2025-01-02 00:00:00', 'YYYY-MM-DD HH24:MI:SS')"
        );
        let timestamp = OracleTimestamp::new_timestamp(2025, 1, 2, 3, 4, 5, 123_456_789);
        assert_eq!(
            render_temporal_literal(&timestamp, "TIMESTAMP(6)"),
            "TO_TIMESTAMP('2025-01-02 03:04:05.123456789', 'YYYY-MM-DD HH24:MI:SS.FF9')"
        );
        let with_tz = OracleTimestamp::new_timestamp_tz(2025, 1, 2, 3, 4, 5, 0, 8, 30);
        assert_eq!(
            render_temporal_literal(&with_tz, "TIMESTAMP(6) WITH TIME ZONE"),
            "TO_TIMESTAMP_TZ('2025-01-02 03:04:05.000000000 +08:30', \
             'YYYY-MM-DD HH24:MI:SS.FF9 TZH:TZM')"
        );
    }

    #[test]
    fn normalizes_display_types() {
        assert_eq!(display_type("NUMBER"), "NUMBER");
        assert_eq!(display_type("VARCHAR2"), "VARCHAR2");
        assert_eq!(display_type("TIMESTAMP(6)"), "TIMESTAMP");
        assert_eq!(display_type("TIMESTAMP(6) WITH TIME ZONE"), "TIMESTAMP");
        assert_eq!(display_type("INTERVAL DAY(3) TO SECOND(2)"), "INTERVAL DAY");
    }

    #[test]
    fn classifies_statements() {
        assert!(returns_result_set("SELECT 1 FROM dual"));
        assert!(returns_result_set(
            "  -- x\n WITH c AS (SELECT 1 FROM dual) SELECT * FROM c"
        ));
        assert!(returns_result_set(
            "INSERT INTO t VALUES (1) RETURNING id INTO :1"
        ));
        assert!(!returns_result_set("UPDATE t SET a = 1"));
        assert!(!returns_result_set("BEGIN NULL; END;"));
        assert!(is_plsql_start("BEGIN NULL; END;"));
        assert!(is_plsql_start("CREATE OR REPLACE PACKAGE BODY pkg AS END;"));
        assert!(!is_plsql_start("CREATE TABLE t (id NUMBER)"));
    }
}
