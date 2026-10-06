//! PostgreSQL dialect helpers: identifier/literal quoting, qualified-name handling, script
//! splitting, filter translation and value decoding. Kept free of any app dependency so it can be
//! unit tested without a server.

use rustgrid_core::{
    CellValue, ColumnInfo, FilterCondition, FilterConjunction, FilterNode, FilterOperator,
    PageRequest,
};
use sqlx::{Column, Row, TypeInfo, ValueRef};

/// Quote an identifier with double quotes, escaping an embedded `"` as `""`.
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

/// Quote a string as a standard `'...'` literal, escaping an embedded `'` as `''`. A backslash is
/// literal under PostgreSQL's default `standard_conforming_strings = on`.
pub(crate) fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// A fully qualified `"schema"."object"` name.
pub(crate) fn qualify(schema: &str, name: &str) -> String {
    format!("{}.{}", quote_identifier(schema), quote_identifier(name))
}

/// The positional placeholder for the `index`-th bound parameter (1-based).
pub(crate) fn placeholder(index: usize) -> String {
    format!("${index}")
}

/// Split an optional `schema.object` name at the first dot. A name without a usable prefix (or
/// with an empty part) is returned whole with `None`.
pub(crate) fn split_qualified(name: &str) -> Option<(&str, &str)> {
    match name.split_once('.') {
        Some((schema, object)) if !schema.is_empty() && !object.is_empty() => {
            Some((schema, object))
        }
        _ => None,
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

/// Whether a statement is expected to produce a result set. PostgreSQL cannot tell the client
/// before executing on the text protocol, so the leading keyword decides.
pub(crate) fn returns_result_set(sql: &str) -> bool {
    let keyword = leading_keyword(sql);
    if matches!(
        keyword.as_str(),
        "SELECT" | "WITH" | "VALUES" | "TABLE" | "SHOW" | "EXPLAIN" | "DESCRIBE" | "FETCH" | "CALL"
    ) {
        return true;
    }
    matches!(keyword.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE")
        && sql.to_ascii_uppercase().contains("RETURNING")
}

/// Split a SQL script into top-level statements on semicolons, respecting string literals,
/// quoted identifiers, dollar-quoted strings (`$$...$$`, `$tag$...$tag$`) and comments.
///
/// `BEGIN; INSERT ...; COMMIT;` is split into three statements: a bare `BEGIN` opens a
/// transaction, not a PL/pgSQL block (those live inside dollar quotes and are covered by the
/// dollar-quote rule). If a string, identifier or dollar quote is left unterminated the whole
/// script is returned as one statement.
///
/// This is only used to pair result sets with their statement text and to prepare zero-row
/// result sets; execution still hands the whole script to the server.
pub(crate) fn split_statements(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut statements = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
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
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                let mut depth = 1usize;
                while i + 1 < bytes.len() && depth > 0 {
                    if bytes[i] == b'/' && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if depth > 0 {
                    balanced = false;
                }
            }
            b'$' => {
                if let Some((tag, end)) = dollar_quote_at(bytes, i) {
                    // Find the matching closing `$tag$`.
                    let close = format!("${tag}$");
                    let close_bytes = close.as_bytes();
                    let mut j = end;
                    let mut found = false;
                    while j + close_bytes.len() <= bytes.len() {
                        if &bytes[j..j + close_bytes.len()] == close_bytes {
                            j += close_bytes.len();
                            found = true;
                            break;
                        }
                        j += 1;
                    }
                    if found {
                        i = j;
                    } else {
                        balanced = false;
                        i = bytes.len();
                    }
                } else {
                    i += 1;
                }
            }
            b';' => {
                let fragment = sql[start..i].trim();
                if has_sql_content(fragment) {
                    statements.push(fragment.to_string());
                }
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }

    if !balanced {
        return vec![sql.trim().to_string()];
    }

    let fragment = sql[start..].trim();
    if has_sql_content(fragment) {
        statements.push(fragment.to_string());
    }
    statements
}

/// If a dollar-quote opener (`$$` or `$tag$`) starts at `index`, return its tag and the index just
/// past the opener. `$1`/`$2` placeholders are not dollar quotes because a tag cannot start with a
/// digit.
fn dollar_quote_at(bytes: &[u8], index: usize) -> Option<(String, usize)> {
    debug_assert_eq!(bytes[index], b'$');
    let mut j = index + 1;
    if bytes.get(j) == Some(&b'$') {
        return Some((String::new(), j + 1));
    }
    let tag_start = j;
    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
        j += 1;
    }
    if j == tag_start || bytes[tag_start].is_ascii_digit() {
        return None;
    }
    if bytes.get(j) == Some(&b'$') {
        let tag = String::from_utf8_lossy(&bytes[tag_start..j]).into_owned();
        Some((tag, j + 1))
    } else {
        None
    }
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

/// The `ORDER BY` clause for a page, or an empty string when unsorted.
pub(crate) fn order_clause(page: &PageRequest) -> String {
    if page.order_by.is_empty() {
        return String::new();
    }
    let terms = page
        .order_by
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

/// Translate the filter tree into a `WHERE` fragment plus its ordered bind values. Every value is
/// cast to the column's `format_type` (when the column is known) so PostgreSQL's strong typing
/// accepts the text parameter.
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

fn column_type<'a>(columns: &'a [ColumnInfo], name: &str) -> Option<&'a str> {
    columns
        .iter()
        .find(|column| column.name == name)
        .map(|column| column.data_type.as_str())
}

/// A placeholder cast to `column_type`, or a bare placeholder when the type is unknown.
fn cast_placeholder(values: &mut Vec<String>, value: String, data_type: Option<&str>) -> String {
    values.push(value);
    let index = values.len();
    match data_type {
        Some(data_type) if !data_type.trim().is_empty() => {
            format!("CAST({} AS {})", placeholder(index), data_type.trim())
        }
        _ => placeholder(index),
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
    let data_type = column_type(columns, &condition.column);
    // Pattern matches compare the column's text form, so no cast is needed on the value.
    let text_column = format!("{column}::text");
    let piece = match operator {
        FilterOperator::Equal => {
            let value = cast_placeholder(values, condition.value.clone(), data_type);
            format!("{column} = {value}")
        }
        FilterOperator::NotEqual => {
            let value = cast_placeholder(values, condition.value.clone(), data_type);
            format!("{column} <> {value}")
        }
        FilterOperator::LessThan => {
            let value = cast_placeholder(values, condition.value.clone(), data_type);
            format!("{column} < {value}")
        }
        FilterOperator::LessOrEqual => {
            let value = cast_placeholder(values, condition.value.clone(), data_type);
            format!("{column} <= {value}")
        }
        FilterOperator::GreaterThan => {
            let value = cast_placeholder(values, condition.value.clone(), data_type);
            format!("{column} > {value}")
        }
        FilterOperator::GreaterOrEqual => {
            let value = cast_placeholder(values, condition.value.clone(), data_type);
            format!("{column} >= {value}")
        }
        FilterOperator::Contains => {
            let value = cast_placeholder(values, format!("%{}%", condition.value), None);
            format!("{text_column} LIKE {value}")
        }
        FilterOperator::NotContains => {
            let value = cast_placeholder(values, format!("%{}%", condition.value), None);
            format!("{text_column} NOT LIKE {value}")
        }
        FilterOperator::StartsWith => {
            let value = cast_placeholder(values, format!("{}%", condition.value), None);
            format!("{text_column} LIKE {value}")
        }
        FilterOperator::NotStartsWith => {
            let value = cast_placeholder(values, format!("{}%", condition.value), None);
            format!("{text_column} NOT LIKE {value}")
        }
        FilterOperator::EndsWith => {
            let value = cast_placeholder(values, format!("%{}", condition.value), None);
            format!("{text_column} LIKE {value}")
        }
        FilterOperator::NotEndsWith => {
            let value = cast_placeholder(values, format!("%{}", condition.value), None);
            format!("{text_column} NOT LIKE {value}")
        }
        FilterOperator::IsNull => format!("{column} IS NULL"),
        FilterOperator::IsNotNull => format!("{column} IS NOT NULL"),
        FilterOperator::IsEmpty => format!("({column} IS NULL OR {text_column} = '')"),
        FilterOperator::IsNotEmpty => format!("({column} IS NOT NULL AND {text_column} <> '')"),
        FilterOperator::Between => {
            let first = cast_placeholder(values, condition.value.clone(), data_type);
            let second = cast_placeholder(values, condition.value2.clone(), data_type);
            format!("{column} BETWEEN {first} AND {second}")
        }
        FilterOperator::NotBetween => {
            let first = cast_placeholder(values, condition.value.clone(), data_type);
            let second = cast_placeholder(values, condition.value2.clone(), data_type);
            format!("{column} NOT BETWEEN {first} AND {second}")
        }
        FilterOperator::InList => {
            let list = condition.list_values();
            if list.is_empty() {
                return None;
            }
            let placeholders = list
                .into_iter()
                .map(|value| cast_placeholder(values, value, data_type))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{column} IN ({placeholders})")
        }
        FilterOperator::NotInList => {
            let list = condition.list_values();
            if list.is_empty() {
                return None;
            }
            let placeholders = list
                .into_iter()
                .map(|value| cast_placeholder(values, value, data_type))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{column} NOT IN ({placeholders})")
        }
    };
    Some(piece)
}

/// Map a text-format cell (raw SQL protocol, where every value arrives as text) to a
/// [`CellValue`], using the wire type name.
pub(crate) fn decode_text_cell(row: &sqlx::postgres::PgRow, index: usize) -> CellValue {
    let is_null = row
        .try_get_raw(index)
        .map(|raw| raw.is_null())
        .unwrap_or(true);
    if is_null {
        return CellValue::Null;
    }
    let type_name = row
        .columns()
        .get(index)
        .map(|column| column.type_info().name().to_ascii_uppercase())
        .unwrap_or_default();
    match row.try_get_unchecked::<String, _>(index) {
        Ok(text) => cell_from_text(&type_name, text),
        Err(_) => match row.try_get_unchecked::<Vec<u8>, _>(index) {
            Ok(bytes) => CellValue::Bytes(bytes),
            Err(_) => CellValue::Null,
        },
    }
}

/// Map a value read as text to a [`CellValue`], dispatching on the Postgres type name (e.g.
/// `INT4`, `BOOL`, `FLOAT8`, `BYTEA`). Everything else stays text, which is exact for `numeric`,
/// `json(b)`, `uuid`, `inet`, arrays and the temporal types.
pub(crate) fn cell_from_text(type_name: &str, text: String) -> CellValue {
    match type_name {
        "BOOL" => CellValue::Bool(text == "t" || text.eq_ignore_ascii_case("true")),
        "INT2" | "INT4" | "INT8" => match text.trim().parse::<i64>() {
            Ok(value) => CellValue::Int(value),
            Err(_) => CellValue::Text(text),
        },
        "OID" | "XID" | "CID" => match text.trim().parse::<u64>() {
            Ok(value) => CellValue::Uint(value),
            Err(_) => CellValue::Text(text),
        },
        "FLOAT4" | "FLOAT8" => match text.trim().parse::<f64>() {
            Ok(value) => CellValue::Float(value),
            Err(_) => CellValue::Text(text),
        },
        "BYTEA" => decode_bytea_text(&text),
        _ => CellValue::Text(text),
    }
}

/// Map a value read from a `::text` projection to a [`CellValue`], using the column's
/// `format_type` (e.g. `integer`, `boolean`, `numeric(10,2)`, `timestamp with time zone`).
pub(crate) fn cell_from_format_type(format_type: &str, text: String) -> CellValue {
    let base = format_type.trim();
    let base = base.split('(').next().unwrap_or(base).trim();
    match base {
        "boolean" => CellValue::Bool(text == "t" || text.eq_ignore_ascii_case("true")),
        "smallint" | "integer" | "bigint" => match text.trim().parse::<i64>() {
            Ok(value) => CellValue::Int(value),
            Err(_) => CellValue::Text(text),
        },
        "oid" | "xid" | "cid" => match text.trim().parse::<u64>() {
            Ok(value) => CellValue::Uint(value),
            Err(_) => CellValue::Text(text),
        },
        "real" | "double precision" => match text.trim().parse::<f64>() {
            Ok(value) => CellValue::Float(value),
            Err(_) => CellValue::Text(text),
        },
        "bytea" => decode_bytea_text(&text),
        _ => CellValue::Text(text),
    }
}

/// Decode a PostgreSQL `bytea` text representation (`\x...`) into raw bytes.
fn decode_bytea_text(text: &str) -> CellValue {
    if let Some(hex) = text.strip_prefix("\\x")
        && hex.len() % 2 == 0
    {
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        let chars: Vec<char> = hex.chars().collect();
        let mut ok = true;
        for pair in chars.chunks(2) {
            let value = u8::from_str_radix(&format!("{}{}", pair[0], pair[1]), 16);
            match value {
                Ok(byte) => bytes.push(byte),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return CellValue::Bytes(bytes);
        }
    }
    CellValue::Text(text.to_string())
}

/// Render one decoded cell as a PostgreSQL SQL literal, for the backup container.
pub(crate) fn render_literal(value: &CellValue) -> String {
    match value {
        CellValue::Null => "NULL".to_string(),
        CellValue::Bool(value) => {
            if *value {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        CellValue::Int(value) => value.to_string(),
        CellValue::Uint(value) => value.to_string(),
        CellValue::Float(value) => {
            if value.is_finite() {
                value.to_string()
            } else {
                "NULL".to_string()
            }
        }
        CellValue::Text(text) => quote_literal(text),
        CellValue::Bytes(bytes) => {
            let mut hex = String::with_capacity(bytes.len() * 2 + 3);
            hex.push_str("'\\x");
            for byte in bytes {
                hex.push_str(&format!("{byte:02x}"));
            }
            hex.push('\'');
            hex
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn quotes_identifiers_and_escapes_quotes() {
        assert_eq!(quote_identifier("users"), "\"users\"");
        assert_eq!(quote_identifier("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn quotes_literals() {
        assert_eq!(quote_literal("O'Brien"), "'O''Brien'");
        assert_eq!(quote_literal("中文"), "'中文'");
    }

    #[test]
    fn splits_qualified_names() {
        assert_eq!(split_qualified("public.users"), Some(("public", "users")));
        assert_eq!(split_qualified("users"), None);
        assert_eq!(split_qualified("a.b.c"), Some(("a", "b.c")));
    }

    #[test]
    fn splits_simple_statements() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2;"),
            vec!["SELECT 1", "SELECT 2"]
        );
        assert_eq!(
            split_statements("BEGIN; INSERT INTO t VALUES (1); COMMIT;"),
            vec!["BEGIN", "INSERT INTO t VALUES (1)", "COMMIT"]
        );
    }

    #[test]
    fn does_not_split_inside_dollar_quotes() {
        let script =
            "CREATE FUNCTION f() RETURNS int AS $$ BEGIN; RETURN 1; END; $$ LANGUAGE plpgsql;";
        assert_eq!(
            split_statements(script),
            vec!["CREATE FUNCTION f() RETURNS int AS $$ BEGIN; RETURN 1; END; $$ LANGUAGE plpgsql"]
        );
        let tagged = "DO $tag$ BEGIN; PERFORM 1; END; $tag$";
        assert_eq!(split_statements(tagged), vec![tagged]);
    }

    #[test]
    fn placeholders_are_not_dollar_quotes() {
        assert_eq!(
            split_statements("SELECT $1; SELECT $2"),
            vec!["SELECT $1", "SELECT $2"]
        );
    }

    #[test]
    fn does_not_split_inside_strings_or_comments() {
        assert_eq!(
            split_statements("SELECT ';' -- ; comment\n; SELECT 2"),
            vec!["SELECT ';' -- ; comment", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT /* a;b */ 1; SELECT 2"),
            vec!["SELECT /* a;b */ 1", "SELECT 2"]
        );
    }

    #[test]
    fn filter_numbers_placeholders_and_casts() {
        let columns = vec![
            column("name", "character varying(50)"),
            column("age", "integer"),
        ];
        let mut equals = FilterCondition::new("name");
        equals.value = "ann".to_string();
        let mut between = FilterCondition::new("age");
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
        assert_eq!(
            clause,
            " WHERE \"name\" = CAST($1 AS character varying(50)) \
             AND \"age\" BETWEEN CAST($2 AS integer) AND CAST($3 AS integer)"
        );
        assert_eq!(values, vec!["ann", "18", "30"]);
    }

    #[test]
    fn filter_casts_in_lists() {
        let columns = vec![column("id", "integer")];
        let condition = FilterCondition {
            column: "id".to_string(),
            operator: FilterOperator::InList,
            value: "1, 2 ,3".to_string(),
            value2: String::new(),
            conjunction: FilterConjunction::And,
            enabled: true,
        };
        let (clause, values) = filter_clause(&[FilterNode::Condition(condition)], &columns);
        assert_eq!(
            clause,
            " WHERE \"id\" IN (CAST($1 AS integer), CAST($2 AS integer), CAST($3 AS integer))"
        );
        assert_eq!(values, vec!["1", "2", "3"]);
    }

    #[test]
    fn maps_text_values_by_type() {
        assert!(matches!(
            cell_from_text("BOOL", "t".into()),
            CellValue::Bool(true)
        ));
        assert!(matches!(
            cell_from_text("INT4", "42".into()),
            CellValue::Int(42)
        ));
        assert!(matches!(
            cell_from_text("FLOAT8", "1.5".into()),
            CellValue::Float(_)
        ));
        assert!(matches!(
            cell_from_text("NUMERIC", "12.30".into()),
            CellValue::Text(text) if text == "12.30"
        ));
        assert!(matches!(
            cell_from_text("BYTEA", "\\x0aff".into()),
            CellValue::Bytes(bytes) if bytes == vec![0x0a, 0xff]
        ));
    }

    #[test]
    fn maps_format_types() {
        assert!(matches!(
            cell_from_format_type("boolean", "f".into()),
            CellValue::Bool(false)
        ));
        assert!(matches!(
            cell_from_format_type("numeric(10,2)", "1.00".into()),
            CellValue::Text(text) if text == "1.00"
        ));
        assert!(matches!(
            cell_from_format_type("integer", "7".into()),
            CellValue::Int(7)
        ));
    }

    #[test]
    fn classifies_statements() {
        assert!(returns_result_set("SELECT 1"));
        assert!(returns_result_set(
            "  -- x\n WITH c AS (SELECT 1) SELECT * FROM c"
        ));
        assert!(returns_result_set("INSERT INTO t VALUES (1) RETURNING id"));
        assert!(!returns_result_set("UPDATE t SET a = 1"));
    }
}
