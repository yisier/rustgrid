//! SQL Server dialect helpers: identifier/literal quoting, script splitting, filter
//! translation and value decoding. Kept free of any app dependency so it can be unit
//! tested without a server.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
use rustgrid_core::{
    CellValue, FilterCondition, FilterConjunction, FilterNode, FilterOperator, PageRequest,
};
use tiberius::{ColumnData, FromSql, Row};

/// Quote an identifier with T-SQL brackets, escaping an embedded `]` as `]]`.
pub(crate) fn quote_identifier(identifier: &str) -> String {
    let mut out = String::with_capacity(identifier.len() + 2);
    out.push('[');
    for character in identifier.chars() {
        if character == ']' {
            out.push(']');
        }
        out.push(character);
    }
    out.push(']');
    out
}

/// Quote a string as a T-SQL `N'...'` Unicode literal.
pub(crate) fn quote_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 3);
    out.push_str("N'");
    for character in value.chars() {
        if character == '\'' {
            out.push('\'');
        }
        out.push(character);
    }
    out.push('\'');
    out
}

/// A fully qualified `[database].[schema].[object]` name.
pub(crate) fn qualified_name(database: &str, schema: &str, name: &str) -> String {
    format!(
        "{}.{}.{}",
        quote_identifier(database),
        quote_identifier(schema),
        quote_identifier(name)
    )
}

/// Render one decoded cell as a T-SQL literal, for the backup container.
pub(crate) fn render_literal(value: &CellValue) -> String {
    match value {
        CellValue::Null => "NULL".to_string(),
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
        CellValue::Text(text) => quote_literal(text),
        CellValue::Bytes(bytes) => {
            let mut hex = String::with_capacity(bytes.len() * 2 + 2);
            hex.push_str("0x");
            for byte in bytes {
                hex.push_str(&format!("{byte:02X}"));
            }
            hex
        }
    }
}

/// Whether a statement is expected to produce a result set, decided from its leading
/// keyword (SQL Server cannot tell the client before executing).
pub(crate) fn returns_result_set(sql: &str) -> bool {
    let keyword = leading_keyword(sql);
    matches!(
        keyword.as_str(),
        "SELECT" | "WITH" | "VALUES" | "EXEC" | "EXECUTE" | "DECLARE" | "SHOWPLAN" | "SET"
    )
}

/// Whether a statement is DML that reports a row count (`INSERT`/`UPDATE`/`DELETE`/`MERGE`).
pub(crate) fn is_dml(sql: &str) -> bool {
    let keyword = leading_keyword(sql);
    matches!(keyword.as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE")
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

/// Split a T-SQL script into top-level statements on semicolons, respecting string
/// literals, quoted identifiers, comments and `BEGIN`/`CASE` ... `END` blocks.
///
/// `GO` lines (the client-side batch separator `sqlcmd`/SSMS use, optionally with a repeat
/// count and a trailing `--` comment) are removed and start a new batch: SQL Server itself
/// rejects `GO`, so it must never reach the server. Each batch is split independently.
///
/// If a block is left open at the end of a batch (unbalanced `BEGIN`/`END`, or a `BEGIN
/// TRANSACTION` that is closed by `COMMIT` rather than `END`), that batch is returned as one
/// statement so the server receives it intact.
pub(crate) fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    for batch in split_batches(sql) {
        let batch_statements = split_batch_statements(&batch.text);
        if batch.repeat > 1 {
            // `GO n` runs its batch n times, the way sqlcmd/SSMS do.
            let repeated = batch_statements
                .iter()
                .cloned()
                .cycle()
                .take(batch_statements.len() * batch.repeat);
            statements.extend(repeated);
        } else {
            statements.extend(batch_statements);
        }
    }
    statements
}

/// One `GO`-delimited batch, with the repeat count of `GO n`.
struct Batch {
    text: String,
    repeat: usize,
}

/// Split a script on `GO` lines. Strings, quoted identifiers and comments are skipped whole,
/// so a `GO` inside any of them is not a separator; only a line whose first token is `GO`
/// (followed by an optional count and an optional `--` comment) separates batches.
fn split_batches(sql: &str) -> Vec<Batch> {
    let bytes = sql.as_bytes();
    let mut batches = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut line_start = true;

    while i < bytes.len() {
        // Newlines and leading indentation keep the scanner at the start of a line; any other
        // byte ends that state before the construct skippers below consume it.
        if bytes[i] == b'\n' {
            i += 1;
            line_start = true;
            continue;
        }
        if line_start && matches!(bytes[i], b' ' | b'\t' | b'\r') {
            i += 1;
            continue;
        }
        if line_start {
            line_start = false;
            if let Some((end, repeat)) = go_separator(bytes, i) {
                let text = sql[start..i].trim();
                if has_sql_content(text) {
                    batches.push(Batch {
                        text: text.to_string(),
                        repeat,
                    });
                }
                i = end;
                start = i;
                line_start = true;
                continue;
            }
        }

        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if i + 1 < bytes.len() && bytes[i + 1] == quote {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'[' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b']' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b']' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
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
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            _ => i += 1,
        }
    }

    let text = sql[start..].trim();
    if has_sql_content(text) {
        batches.push(Batch {
            text: text.to_string(),
            repeat: 1,
        });
    }
    batches
}

/// The `GO [count] [-- comment]` separator starting at `index`, if this really is one.
/// Returns the index just past its line and the repeat count. `GOTO` and identifiers such as
/// `GO_OBJECT` are not separators.
fn go_separator(bytes: &[u8], index: usize) -> Option<(usize, usize)> {
    let tail = &bytes[index..];
    if tail.len() < 2
        || !tail[0].eq_ignore_ascii_case(&b'G')
        || !tail[1].eq_ignore_ascii_case(&b'O')
    {
        return None;
    }
    if tail.get(2).is_some_and(|byte| is_ident_byte(*byte)) {
        return None;
    }

    let mut i = index + 2;
    while matches!(bytes.get(i), Some(b' ' | b'\t' | b'\r')) {
        i += 1;
    }
    let digits_start = i;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    let repeat = if i > digits_start {
        std::str::from_utf8(&bytes[digits_start..i])
            .ok()
            .and_then(|digits| digits.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 1000)
    } else {
        1
    };
    while matches!(bytes.get(i), Some(b' ' | b'\t' | b'\r')) {
        i += 1;
    }
    match bytes.get(i) {
        None => Some((i, repeat)),
        Some(b'\n') => Some((i + 1, repeat)),
        Some(b'-') if bytes.get(i + 1) == Some(&b'-') => {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            Some(((i + 1).min(bytes.len()), repeat))
        }
        _ => None,
    }
}

/// Split one `GO`-delimited batch on top-level semicolons.
fn split_batch_statements(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut statements = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut depth: i32 = 0;
    let mut balanced = true;

    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if i + 1 < bytes.len() && bytes[i + 1] == quote {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'[' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b']' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b']' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
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
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            b';' if depth == 0 => {
                let fragment = sql[start..i].trim();
                if has_sql_content(fragment) {
                    statements.push(fragment.to_string());
                }
                i += 1;
                start = i;
            }
            _ => {
                if let Some(word) = word_at(bytes, i) {
                    match word {
                        "BEGIN" => {
                            depth += 1;
                            i += word.len();
                            continue;
                        }
                        "CASE" => {
                            depth += 1;
                            i += word.len();
                            continue;
                        }
                        "END" => {
                            depth -= 1;
                            if depth < 0 {
                                balanced = false;
                            }
                            i += word.len();
                            continue;
                        }
                        _ => {}
                    }
                }
                i += 1;
            }
        }
    }

    if depth != 0 {
        balanced = false;
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

/// The uppercase SQL word starting at `index`, if any. A keyword only counts when it is a
/// whole word (bounded by non-identifier characters).
fn word_at(bytes: &[u8], index: usize) -> Option<&'static str> {
    let before_ok = index == 0 || !is_ident_byte(bytes[index - 1]);
    if !before_ok {
        return None;
    }
    if bytes[index].is_ascii_alphabetic() {
        let mut end = index;
        while end < bytes.len() && is_ident_byte(bytes[end]) {
            end += 1;
        }
        let word = &bytes[index..end];
        if bytes.len() > index && (end == bytes.len() || !is_ident_byte(bytes[end])) {
            return match word {
                b"BEGIN" | b"begin" | b"Begin" => Some("BEGIN"),
                b"CASE" | b"case" | b"Case" => Some("CASE"),
                b"END" | b"end" | b"End" => Some("END"),
                _ => None,
            };
        }
    }
    None
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte == b'@' || byte == b'#'
}

/// Whether a fragment contains anything besides whitespace and comments.
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

/// The `ORDER BY` clause for a page. `OFFSET`/`FETCH` requires an ordering; when none is
/// requested the caller supplies `ORDER BY (SELECT NULL)`.
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

/// Translate the filter tree into a `WHERE` fragment plus its ordered bind values.
pub(crate) fn filter_clause(filter: &[FilterNode]) -> (String, Vec<String>) {
    let mut values: Vec<String> = Vec::new();
    match filter_fragment(filter, &mut values) {
        Some(expression) => (format!(" WHERE {expression}"), values),
        None => (String::new(), Vec::new()),
    }
}

fn filter_fragment(filter: &[FilterNode], values: &mut Vec<String>) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    for node in filter {
        let piece = match node {
            FilterNode::Condition(condition) => condition_piece(condition, values),
            FilterNode::Group(group) => {
                if !group.enabled {
                    None
                } else {
                    filter_fragment(&group.children, values).map(|inner| format!("({inner})"))
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

fn condition_piece(condition: &FilterCondition, values: &mut Vec<String>) -> Option<String> {
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
    let piece = match operator {
        FilterOperator::Equal => {
            values.push(condition.value.clone());
            format!("{column} = @P{}", values.len())
        }
        FilterOperator::NotEqual => {
            values.push(condition.value.clone());
            format!("{column} <> @P{}", values.len())
        }
        FilterOperator::LessThan => {
            values.push(condition.value.clone());
            format!("{column} < @P{}", values.len())
        }
        FilterOperator::LessOrEqual => {
            values.push(condition.value.clone());
            format!("{column} <= @P{}", values.len())
        }
        FilterOperator::GreaterThan => {
            values.push(condition.value.clone());
            format!("{column} > @P{}", values.len())
        }
        FilterOperator::GreaterOrEqual => {
            values.push(condition.value.clone());
            format!("{column} >= @P{}", values.len())
        }
        FilterOperator::Contains => {
            values.push(format!("%{}%", condition.value));
            format!("{column} LIKE @P{}", values.len())
        }
        FilterOperator::NotContains => {
            values.push(format!("%{}%", condition.value));
            format!("{column} NOT LIKE @P{}", values.len())
        }
        FilterOperator::StartsWith => {
            values.push(format!("{}%", condition.value));
            format!("{column} LIKE @P{}", values.len())
        }
        FilterOperator::NotStartsWith => {
            values.push(format!("{}%", condition.value));
            format!("{column} NOT LIKE @P{}", values.len())
        }
        FilterOperator::EndsWith => {
            values.push(format!("%{}", condition.value));
            format!("{column} LIKE @P{}", values.len())
        }
        FilterOperator::NotEndsWith => {
            values.push(format!("%{}", condition.value));
            format!("{column} NOT LIKE @P{}", values.len())
        }
        FilterOperator::IsNull => format!("{column} IS NULL"),
        FilterOperator::IsNotNull => format!("{column} IS NOT NULL"),
        FilterOperator::IsEmpty => format!("({column} IS NULL OR {column} = N'')"),
        FilterOperator::IsNotEmpty => format!("({column} IS NOT NULL AND {column} <> N'')"),
        FilterOperator::Between => {
            values.push(condition.value.clone());
            let first = values.len();
            values.push(condition.value2.clone());
            format!("{column} BETWEEN @P{first} AND @P{}", values.len())
        }
        FilterOperator::NotBetween => {
            values.push(condition.value.clone());
            let first = values.len();
            values.push(condition.value2.clone());
            format!("{column} NOT BETWEEN @P{first} AND @P{}", values.len())
        }
        FilterOperator::InList => {
            let list = condition.list_values();
            if list.is_empty() {
                return None;
            }
            let placeholders = list
                .into_iter()
                .map(|value| {
                    values.push(value);
                    format!("@P{}", values.len())
                })
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
                .map(|value| {
                    values.push(value);
                    format!("@P{}", values.len())
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{column} NOT IN ({placeholders})")
        }
    };
    Some(piece)
}

/// Decode one cell of a row into an engine-agnostic value. Dispatching on the wire
/// `ColumnData` (rather than the reported column type) also handles `sql_variant`, whose
/// reported type is opaque but whose value decodes to its intrinsic variant.
pub(crate) fn decode_cell(row: &Row, index: usize) -> CellValue {
    let Ok(data) = row.get_column_data(index) else {
        return CellValue::Null;
    };
    decode_column(data)
}

pub(crate) fn decode_column(data: &ColumnData<'static>) -> CellValue {
    match data {
        ColumnData::U8(value) => {
            value.map_or(CellValue::Null, |value| CellValue::Uint(value as u64))
        }
        ColumnData::I16(value) => {
            value.map_or(CellValue::Null, |value| CellValue::Int(value as i64))
        }
        ColumnData::I32(value) => {
            value.map_or(CellValue::Null, |value| CellValue::Int(value as i64))
        }
        ColumnData::I64(value) => value.map_or(CellValue::Null, CellValue::Int),
        ColumnData::F32(value) => {
            value.map_or(CellValue::Null, |value| CellValue::Float(value as f64))
        }
        ColumnData::F64(value) => value.map_or(CellValue::Null, CellValue::Float),
        ColumnData::Bit(value) => value.map_or(CellValue::Null, CellValue::Bool),
        ColumnData::String(value) => match value {
            Some(value) => CellValue::Text(value.to_string()),
            None => CellValue::Null,
        },
        ColumnData::Guid(value) => {
            value.map_or(CellValue::Null, |value| CellValue::Text(value.to_string()))
        }
        ColumnData::Binary(value) => match value {
            Some(value) => CellValue::Bytes(value.to_vec()),
            None => CellValue::Null,
        },
        ColumnData::Numeric(value) => match value {
            Some(value) => CellValue::Text(value.to_string()),
            None => CellValue::Null,
        },
        ColumnData::Xml(value) => match value {
            Some(value) => CellValue::Text(value.to_string()),
            None => CellValue::Null,
        },
        ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => {
            temporal::<NaiveDateTime>(data, format_datetime)
        }
        ColumnData::Date(_) => temporal::<NaiveDate>(data, |value| value.to_string()),
        ColumnData::Time(_) => temporal::<NaiveTime>(data, |value| value.to_string()),
        ColumnData::DateTimeOffset(_) => {
            temporal::<DateTime<FixedOffset>>(data, |value| value.to_rfc3339())
        }
    }
}

fn temporal<T>(data: &ColumnData<'static>, format: fn(T) -> String) -> CellValue
where
    T: for<'a> FromSql<'a>,
{
    match T::from_sql(data) {
        Ok(Some(value)) => CellValue::Text(format(value)),
        _ => CellValue::Null,
    }
}

/// Format a `datetime`-family value with its fractional seconds only when non-zero.
pub(crate) fn format_datetime(value: NaiveDateTime) -> String {
    use chrono::Timelike;
    if value.nanosecond() == 0 {
        value.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        value.format("%Y-%m-%d %H:%M:%S%.f").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgrid_core::{FilterCondition, PageRequest, SortColumn};

    #[test]
    fn quotes_identifiers_and_escapes_closing_bracket() {
        assert_eq!(quote_identifier("users"), "[users]");
        assert_eq!(quote_identifier("we]ird"), "[we]]ird]");
    }

    #[test]
    fn quotes_unicode_literals() {
        assert_eq!(quote_literal("O'Brien"), "N'O''Brien'");
        assert_eq!(quote_literal("中文"), "N'中文'");
    }

    #[test]
    fn renders_literals() {
        assert_eq!(render_literal(&CellValue::Null), "NULL");
        assert_eq!(render_literal(&CellValue::Bool(true)), "1");
        assert_eq!(render_literal(&CellValue::Int(-5)), "-5");
        assert_eq!(
            render_literal(&CellValue::Text("a'b".to_string())),
            "N'a''b'"
        );
        assert_eq!(
            render_literal(&CellValue::Bytes(vec![0x0a, 0xff])),
            "0x0AFF"
        );
    }

    #[test]
    fn classifies_statements() {
        assert!(returns_result_set("SELECT 1"));
        assert!(returns_result_set(
            "  -- x\n WITH c AS (SELECT 1) SELECT * FROM c"
        ));
        assert!(!returns_result_set("INSERT INTO t VALUES (1)"));
        assert!(is_dml("update t set a = 1"));
        assert!(is_dml("DELETE FROM t"));
        assert!(!is_dml("CREATE TABLE t (a int)"));
    }

    #[test]
    fn splits_statements_on_top_level_semicolons() {
        let statements = split_statements("SELECT 1; SELECT 2;");
        assert_eq!(statements, vec!["SELECT 1", "SELECT 2"]);
    }

    #[test]
    fn does_not_split_inside_strings_or_comments() {
        let statements = split_statements("SELECT ';' AS a -- ; comment\n; SELECT 2");
        assert_eq!(statements, vec!["SELECT ';' AS a -- ; comment", "SELECT 2"]);
    }

    #[test]
    fn keeps_a_procedure_body_intact() {
        let script = "CREATE PROCEDURE p AS BEGIN SELECT 1; SELECT 2; END";
        assert_eq!(split_statements(script), vec![script]);
    }

    #[test]
    fn falls_back_to_whole_script_when_unbalanced() {
        let script = "BEGIN TRAN; SELECT 1";
        assert_eq!(split_statements(script), vec![script]);
    }

    #[test]
    fn splits_on_go_batch_separators() {
        // The DDL the table designer shows ends with `GO`; SQL Server rejects it, so the
        // splitter drops it instead of sending it.
        let script = "CREATE TABLE [dbo].[t] (\n  [id] int\n)\nGO";
        assert_eq!(
            split_statements(script),
            vec!["CREATE TABLE [dbo].[t] (\n  [id] int\n)"]
        );
        // A `GO` between statements (and a lower-case one) separates them too.
        assert_eq!(
            split_statements("SELECT 1;\nGO\nselect 2\n  go  "),
            vec!["SELECT 1", "select 2"]
        );
    }

    #[test]
    fn go_with_a_count_repeats_the_batch() {
        assert_eq!(
            split_statements("INSERT INTO t VALUES (1)\nGO 3\n"),
            vec![
                "INSERT INTO t VALUES (1)",
                "INSERT INTO t VALUES (1)",
                "INSERT INTO t VALUES (1)"
            ]
        );
    }

    #[test]
    fn go_inside_a_string_or_comment_is_not_a_separator() {
        let script = "SELECT 'a\nGO\nb' AS value /*\nGO\n*/";
        assert_eq!(split_statements(script), vec![script]);
        assert_eq!(
            split_statements("-- GO\nSELECT 1\nGO -- done\nSELECT 2"),
            vec!["-- GO\nSELECT 1", "SELECT 2"]
        );
    }

    #[test]
    fn goto_is_not_a_go_separator() {
        assert_eq!(
            split_statements("GOTO done;\nSELECT 1;\ndone:"),
            vec!["GOTO done", "SELECT 1", "done:"]
        );
    }

    #[test]
    fn order_clause_quotes_and_orders() {
        let page = PageRequest::new(0, 10).with_order_by(vec![SortColumn {
            column: "name".to_string(),
            descending: true,
        }]);
        assert_eq!(order_clause(&page), " ORDER BY [name] DESC");
        assert_eq!(order_clause(&PageRequest::new(0, 10)), "");
    }

    #[test]
    fn filter_clause_numbers_parameters() {
        let mut equals = FilterCondition::new("name");
        equals.value = "ann".to_string();
        let mut between = FilterCondition::new("age");
        between.operator = FilterOperator::Between;
        between.value = "18".to_string();
        between.value2 = "30".to_string();
        between.conjunction = FilterConjunction::And;
        let (clause, values) = filter_clause(&[
            FilterNode::Condition(equals),
            FilterNode::Condition(between),
        ]);
        assert_eq!(clause, " WHERE [name] = @P1 AND [age] BETWEEN @P2 AND @P3");
        assert_eq!(values, vec!["ann", "18", "30"]);
    }
}
