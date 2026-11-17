//! SQL construction, result decoding and type mapping for the ClickHouse driver.
//!
//! Everything here is engine-specific but free of I/O, so it is unit-tested without a server.
//! ClickHouse's HTTP interface speaks plain SQL: identifiers are backtick-quoted and values are
//! rendered as literals (the driver never uses the client's `?` binding, so a query's own `?`
//! characters stay untouched).

use rustgrid_core::{
    CellValue, ColumnDef, ColumnInfo, Error, FilterCondition, FilterConjunction, FilterNode,
    FilterOperator, PageRequest, Result, RowInsert, TableSchema,
};

/// The `FORMAT` every result-producing statement is asked for: one JSON array per line, preceded
/// by the column names and their ClickHouse types. That is the only way the HTTP interface reports
/// a result set's schema together with its rows in one request.
pub(crate) const ROW_FORMAT: &str = "JSONCompactEachRowWithNamesAndTypes";

/// How many rows one `INSERT` statement carries at most while importing.
pub(crate) const INSERT_BATCH_ROWS: usize = 500;

/// Quote one identifier with backticks, ClickHouse's own quoting style.
pub(crate) fn quote_identifier(name: &str) -> String {
    let mut quoted = String::with_capacity(name.len() + 2);
    quoted.push('`');
    for character in name.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '`' => quoted.push_str("\\`"),
            _ => quoted.push(character),
        }
    }
    quoted.push('`');
    quoted
}

/// Quote a string literal. ClickHouse honours backslash escapes and doubled quotes; backslashes are
/// escaped so a value can never end the literal early.
pub(crate) fn quote_literal(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '\'' => quoted.push_str("\\'"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\0' => quoted.push_str("\\0"),
            _ => quoted.push(character),
        }
    }
    quoted.push('\'');
    quoted
}

/// A possibly database-qualified table reference.
pub(crate) fn table_ref(database: &str, table: &str) -> String {
    if database.trim().is_empty() {
        quote_identifier(table)
    } else {
        format!("{}.{}", quote_identifier(database), quote_identifier(table))
    }
}

/// The first keyword of a statement, skipping comments and leading whitespace.
pub(crate) fn leading_keyword(sql: &str) -> Option<String> {
    let mut rest = sql.trim_start();
    loop {
        if rest.starts_with("--") {
            rest = rest.split_once('\n').map(|(_, tail)| tail).unwrap_or("");
        } else if rest.starts_with('#') {
            rest = rest.split_once('\n').map(|(_, tail)| tail).unwrap_or("");
        } else if rest.starts_with("/*") {
            rest = rest.split_once("*/").map(|(_, tail)| tail).unwrap_or("");
        } else {
            break;
        }
        rest = rest.trim_start();
    }
    let keyword: String = rest
        .chars()
        .take_while(|character| character.is_ascii_alphabetic())
        .collect();
    (!keyword.is_empty()).then(|| keyword.to_ascii_uppercase())
}

/// Whether a statement produces a result set, so it is worth asking for [`ROW_FORMAT`]. ClickHouse
/// cannot report this ahead of time, so the leading keyword decides (the same approach MySQL's
/// driver uses).
pub(crate) fn returns_result_set(sql: &str) -> bool {
    matches!(
        leading_keyword(sql).as_deref(),
        Some("SELECT" | "WITH" | "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "EXISTS")
    )
}

/// Whether the statement already ends with its own `FORMAT …` clause. Appending a second one would
/// be a syntax error, and a query that formats its own output must be left verbatim.
pub(crate) fn has_format_clause(sql: &str) -> bool {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut characters = sql.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(character) = characters.next() {
        if let Some(active) = quote {
            if active == '`' {
                if character == '`' {
                    if characters.peek() == Some(&'`') {
                        characters.next();
                    } else {
                        quote = None;
                    }
                }
                continue;
            }
            match character {
                '\\' => {
                    characters.next();
                }
                character if character == active => quote = None,
                _ => {}
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => {
                quote = Some(character);
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            '#' => {
                for next in characters.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            '-' if characters.peek() == Some(&'-') => {
                for next in characters.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            '/' if characters.peek() == Some(&'*') => {
                characters.next();
                let mut previous = '\0';
                for next in characters.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            character if character.is_ascii_alphanumeric() || character == '_' => {
                word.push(character.to_ascii_uppercase());
            }
            _ => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    // `FORMAT` is the last clause: it is immediately followed by the format's name and nothing
    // else. Checking the final pair keeps `SELECT format FROM t` (a column named `format`) from
    // looking like a clause.
    words.len() >= 2 && words[words.len() - 2] == "FORMAT"
}

/// Append the row format to a result-producing statement (unless the statement brings its own).
pub(crate) fn ensure_row_format(sql: &str) -> String {
    let trimmed = sql.trim().trim_end_matches(';').trim_end();
    if has_format_clause(trimmed) {
        trimmed.to_string()
    } else {
        format!("{trimmed}\nFORMAT {ROW_FORMAT}")
    }
}

/// Strip ClickHouse's `Nullable(…)` / `LowCardinality(…)` wrappers (repeatedly, so
/// `LowCardinality(Nullable(String))` unwraps too).
pub(crate) fn strip_wrappers(data_type: &str) -> &str {
    let mut current = data_type.trim();
    loop {
        let lower = current.to_ascii_lowercase();
        let wrapped = lower.starts_with("nullable(") || lower.starts_with("lowcardinality(");
        if !wrapped || !current.ends_with(')') {
            return current;
        }
        let Some(open) = current.find('(') else {
            return current;
        };
        current = current[open + 1..current.len() - 1].trim();
    }
}

/// The base type name without arguments: `Decimal(20, 6)` → `Decimal`.
pub(crate) fn base_type(data_type: &str) -> &str {
    let unwrapped = strip_wrappers(data_type);
    unwrapped.split('(').next().unwrap_or(unwrapped).trim()
}

/// The scale of a ClickHouse decimal type.
///
/// `Decimal(P, S)` carries it as its second argument; the fixed-width `Decimal32(S)` …
/// `Decimal256(S)` forms carry it as their only one.
fn decimal_scale(data_type: &str) -> Option<u32> {
    let unwrapped = strip_wrappers(data_type);
    let base = base_type(unwrapped);
    let arguments = unwrapped
        .split_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or_default();
    let scale = if base.eq_ignore_ascii_case("Decimal") {
        arguments.split(',').nth(1)?.trim()
    } else if ["Decimal32", "Decimal64", "Decimal128", "Decimal256"]
        .iter()
        .any(|name| base.eq_ignore_ascii_case(name))
    {
        arguments.trim()
    } else {
        return None;
    };
    scale.parse().ok()
}

/// Keep a decimal's declared scale in its text form.
///
/// ClickHouse's JSON output drops trailing fractional zeros (`12.340000` arrives as `"12.34"`),
/// unlike the other engines whose drivers show the full scale. Padding restores it without ever
/// rounding: a value with at least the declared number of decimals is returned untouched.
fn pad_decimal(text: &str, scale: u32) -> String {
    let scale = scale as usize;
    let (integer, fraction) = match text.split_once('.') {
        Some(parts) => parts,
        None => {
            if scale == 0 {
                return text.to_string();
            }
            let numeric = text
                .strip_prefix(['+', '-'])
                .unwrap_or(text)
                .chars()
                .all(|ch| ch.is_ascii_digit());
            if !numeric || text.is_empty() {
                return text.to_string();
            }
            return format!("{text}.{}", "0".repeat(scale));
        }
    };
    let numeric = integer
        .strip_prefix(['+', '-'])
        .unwrap_or(integer)
        .chars()
        .all(|ch| ch.is_ascii_digit())
        && fraction.chars().all(|ch| ch.is_ascii_digit());
    if !numeric || fraction.len() >= scale {
        return text.to_string();
    }
    format!("{integer}.{fraction}{}", "0".repeat(scale - fraction.len()))
}

/// Whether the type is a ClickHouse `Nullable(…)`, also when it is wrapped in
/// `LowCardinality(…)` (`LowCardinality(Nullable(String))` can hold `NULL`).
pub(crate) fn is_nullable(data_type: &str) -> bool {
    let mut current = data_type.trim();
    loop {
        let lower = current.to_ascii_lowercase();
        if lower.starts_with("nullable(") {
            return true;
        }
        if lower.starts_with("lowcardinality(") && current.ends_with(')') {
            let Some(open) = current.find('(') else {
                return false;
            };
            current = current[open + 1..current.len() - 1].trim();
            continue;
        }
        return false;
    }
}

/// The `ORDER BY` fragment for a page, or an empty string when the request has no sort.
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

/// The `SELECT` that fetches one page. ClickHouse has no unique row identity, so pages are only as
/// stable as the requested order (or the engine's natural block order) allows.
pub(crate) fn page_sql(
    database: &str,
    table: &str,
    columns: &[ColumnInfo],
    page: &PageRequest,
) -> String {
    let projection = if columns.is_empty() {
        "*".to_string()
    } else {
        columns
            .iter()
            .map(|column| quote_identifier(&column.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "SELECT {projection} FROM {}{}{} LIMIT {} OFFSET {}",
        table_ref(database, table),
        filter_clause(&page.filter, columns),
        order_clause(page),
        page.page_size.max(1),
        page.offset()
    )
}

/// The `SELECT count()` for a page request's filter.
pub(crate) fn count_sql(
    database: &str,
    table: &str,
    filter: &[FilterNode],
    columns: &[ColumnInfo],
) -> String {
    format!(
        "SELECT count() AS total FROM {}{}",
        table_ref(database, table),
        filter_clause(filter, columns)
    )
}

/// The ` WHERE …` fragment for a filter tree (empty when nothing enabled remains).
pub(crate) fn filter_clause(filter: &[FilterNode], columns: &[ColumnInfo]) -> String {
    match filter_fragment(filter, columns) {
        Some(expression) => format!(" WHERE {expression}"),
        None => String::new(),
    }
}

fn filter_fragment(filter: &[FilterNode], columns: &[ColumnInfo]) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    for node in filter {
        let piece = match node {
            FilterNode::Condition(condition) => condition_piece(condition, columns),
            FilterNode::Group(group) => {
                if !group.enabled {
                    None
                } else {
                    filter_fragment(&group.children, columns).map(|inner| format!("({inner})"))
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

fn column_type<'a>(columns: &'a [ColumnInfo], name: &str) -> Option<&'a str> {
    columns
        .iter()
        .find(|column| column.name == name)
        .map(|column| column.data_type.as_str())
}

/// A filter value as a ClickHouse literal: bare when the column is numeric and the text parses,
/// quoted otherwise so `DateTime`, `UUID` and `String` columns compare correctly.
fn filter_literal(value: &str, data_type: Option<&str>) -> String {
    match data_type {
        Some(data_type) if is_numeric(data_type) => {
            let trimmed = value.trim();
            if !trimmed.is_empty() && trimmed.parse::<f64>().is_ok() {
                trimmed.to_string()
            } else {
                quote_literal(value)
            }
        }
        _ => quote_literal(value),
    }
}

/// Whether a type is one ClickHouse casts from a bare numeric literal.
fn is_numeric(data_type: &str) -> bool {
    let base = base_type(data_type);
    base.starts_with("Int")
        || base.starts_with("UInt")
        || base.starts_with("Float")
        || base.starts_with("Decimal")
        || base == "Bool"
}

fn condition_piece(condition: &FilterCondition, columns: &[ColumnInfo]) -> Option<String> {
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
    let text_column = format!("toString({column})");
    let piece = match operator {
        FilterOperator::Equal => {
            format!("{column} = {}", filter_literal(&condition.value, data_type))
        }
        FilterOperator::NotEqual => format!(
            "{column} != {}",
            filter_literal(&condition.value, data_type)
        ),
        FilterOperator::LessThan => {
            format!("{column} < {}", filter_literal(&condition.value, data_type))
        }
        FilterOperator::LessOrEqual => format!(
            "{column} <= {}",
            filter_literal(&condition.value, data_type)
        ),
        FilterOperator::GreaterThan => {
            format!("{column} > {}", filter_literal(&condition.value, data_type))
        }
        FilterOperator::GreaterOrEqual => format!(
            "{column} >= {}",
            filter_literal(&condition.value, data_type)
        ),
        FilterOperator::Contains => format!(
            "{text_column} LIKE {}",
            quote_literal(&format!("%{}%", condition.value))
        ),
        FilterOperator::NotContains => format!(
            "{text_column} NOT LIKE {}",
            quote_literal(&format!("%{}%", condition.value))
        ),
        FilterOperator::StartsWith => format!(
            "{text_column} LIKE {}",
            quote_literal(&format!("{}%", condition.value))
        ),
        FilterOperator::NotStartsWith => format!(
            "{text_column} NOT LIKE {}",
            quote_literal(&format!("{}%", condition.value))
        ),
        FilterOperator::EndsWith => format!(
            "{text_column} LIKE {}",
            quote_literal(&format!("%{}", condition.value))
        ),
        FilterOperator::NotEndsWith => format!(
            "{text_column} NOT LIKE {}",
            quote_literal(&format!("%{}", condition.value))
        ),
        FilterOperator::IsNull => format!("{column} IS NULL"),
        FilterOperator::IsNotNull => format!("{column} IS NOT NULL"),
        FilterOperator::IsEmpty => format!("({column} IS NULL OR {text_column} = '')"),
        FilterOperator::IsNotEmpty => format!("({column} IS NOT NULL AND {text_column} != '')"),
        FilterOperator::Between => format!(
            "{column} BETWEEN {} AND {}",
            filter_literal(&condition.value, data_type),
            filter_literal(&condition.value2, data_type)
        ),
        FilterOperator::NotBetween => format!(
            "{column} NOT BETWEEN {} AND {}",
            filter_literal(&condition.value, data_type),
            filter_literal(&condition.value2, data_type)
        ),
        FilterOperator::InList => {
            let values = condition.list_values();
            if values.is_empty() {
                return None;
            }
            format!(
                "{column} IN ({})",
                values
                    .iter()
                    .map(|value| filter_literal(value, data_type))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        FilterOperator::NotInList => {
            let values = condition.list_values();
            if values.is_empty() {
                return None;
            }
            format!(
                "{column} NOT IN ({})",
                values
                    .iter()
                    .map(|value| filter_literal(value, data_type))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    };
    Some(piece)
}

/// One result set decoded from [`ROW_FORMAT`].
pub(crate) struct DecodedResult {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<CellValue>>,
}

/// Decode a [`ROW_FORMAT`] body: a names line, a types line, then one JSON array per row.
pub(crate) fn decode_body(body: &[u8]) -> Result<DecodedResult> {
    let text = std::str::from_utf8(body)
        .map_err(|error| Error::Query(format!("ClickHouse returned non-UTF-8 data: {error}")))?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let Some(names_line) = lines.next() else {
        return Ok(DecodedResult {
            columns: Vec::new(),
            rows: Vec::new(),
        });
    };
    let names: Vec<serde_json::Value> = parse_json_line(names_line)?;
    let types: Vec<serde_json::Value> = match lines.next() {
        Some(line) => parse_json_line(line)?,
        None => Vec::new(),
    };

    let mut columns = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        let name = name.as_str().unwrap_or_default().to_string();
        let data_type = types
            .get(index)
            .and_then(|value| value.as_str())
            .unwrap_or("String")
            .to_string();
        columns.push(column_for(&name, &data_type));
    }

    let mut rows = Vec::new();
    for line in lines {
        let values: Vec<serde_json::Value> = parse_json_line(line)?;
        let mut row = Vec::with_capacity(columns.len());
        for (index, column) in columns.iter().enumerate() {
            let value = values.get(index).unwrap_or(&serde_json::Value::Null);
            row.push(cell_from_json(&column.data_type, value));
        }
        rows.push(row);
    }

    Ok(DecodedResult { columns, rows })
}

fn parse_json_line(line: &str) -> Result<Vec<serde_json::Value>> {
    serde_json::from_str(line).map_err(|error| {
        let preview: String = line.chars().take(200).collect();
        Error::Query(format!(
            "ClickHouse returned a malformed JSON row ({error}): {preview}"
        ))
    })
}

/// A column as the response header describes it. ClickHouse reports no comment here.
pub(crate) fn column_for(name: &str, data_type: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.to_string(),
        data_type: data_type.to_string(),
        nullable: is_nullable(data_type),
        primary_key: false,
        auto_increment: false,
        generated: false,
        comment: String::new(),
    }
}

/// Map one JSON value of a ClickHouse-typed column onto the grid's cell model.
pub(crate) fn cell_from_json(data_type: &str, value: &serde_json::Value) -> CellValue {
    if value.is_null() {
        return CellValue::Null;
    }
    let base = base_type(data_type);
    match base {
        "Int8" | "Int16" | "Int32" | "Int64" => int_cell(value),
        "UInt8" | "UInt16" | "UInt32" | "UInt64" => uint_cell(value),
        // 128/256-bit integers exceed the cell model's range; their exact decimal text is kept.
        "Int128" | "Int256" | "UInt128" | "UInt256" => CellValue::Text(json_text(value)),
        "Float32" | "Float64" => match value.as_f64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<f64>().ok())
        }) {
            Some(number) => CellValue::Float(number),
            None => CellValue::Text(json_text(value)),
        },
        "Decimal" | "Decimal32" | "Decimal64" | "Decimal128" | "Decimal256" => {
            let text = json_text(value);
            CellValue::Text(match decimal_scale(data_type) {
                Some(scale) => pad_decimal(&text, scale),
                None => text,
            })
        }
        "Bool" => match value {
            serde_json::Value::Bool(flag) => CellValue::Bool(*flag),
            serde_json::Value::Number(number) => CellValue::Bool(number.as_i64().unwrap_or(0) != 0),
            _ => CellValue::Text(json_text(value)),
        },
        "Date" | "Date32" | "DateTime" | "DateTime64" => CellValue::Text(json_text(value)),
        "String" | "FixedString" | "UUID" | "IPv4" | "IPv6" | "Enum8" | "Enum16" => {
            CellValue::Text(json_text(value))
        }
        // Nested containers are shown as compact JSON, which is exact and readable.
        _ => match value {
            serde_json::Value::String(text) => CellValue::Text(text.clone()),
            serde_json::Value::Bool(flag) => CellValue::Bool(*flag),
            serde_json::Value::Number(number) => {
                if let Some(value) = number.as_i64() {
                    CellValue::Int(value)
                } else if let Some(value) = number.as_u64() {
                    CellValue::Uint(value)
                } else {
                    CellValue::Float(number.as_f64().unwrap_or_default())
                }
            }
            _ => CellValue::Text(json_text(value)),
        },
    }
}

fn int_cell(value: &serde_json::Value) -> CellValue {
    if let Some(number) = value.as_i64() {
        return CellValue::Int(number);
    }
    match value
        .as_str()
        .and_then(|text| text.trim().parse::<i64>().ok())
    {
        Some(number) => CellValue::Int(number),
        None => CellValue::Text(json_text(value)),
    }
}

fn uint_cell(value: &serde_json::Value) -> CellValue {
    if let Some(number) = value.as_u64() {
        return CellValue::Uint(number);
    }
    match value
        .as_str()
        .and_then(|text| text.trim().parse::<u64>().ok())
    {
        Some(number) => CellValue::Uint(number),
        None => CellValue::Text(json_text(value)),
    }
}

fn json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// The ClickHouse type to create for a designer/import column. Types that already look like
/// ClickHouse's own are kept; the MySQL-shaped names `rustgrid_import` infers are mapped.
pub(crate) fn clickhouse_type(data_type: &str, length: &str, decimals: &str) -> String {
    let raw = data_type.trim();
    if raw.is_empty() {
        return "String".to_string();
    }
    let base = raw
        .split('(')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    // A type that is already ClickHouse's own (or a wrapper around one) is kept as written;
    // only the MySQL-shaped names the import inference produces are translated.
    let native = matches!(
        base.as_str(),
        "int8"
            | "int16"
            | "int32"
            | "int64"
            | "int128"
            | "int256"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uint128"
            | "uint256"
            | "float32"
            | "float64"
            | "string"
            | "uuid"
            | "ipv4"
            | "ipv6"
            | "date32"
            | "fixedstring"
            | "datetime64"
            | "decimal32"
            | "decimal64"
            | "decimal128"
            | "decimal256"
            | "enum8"
            | "enum16"
            | "array"
            | "tuple"
            | "map"
            | "nested"
            | "json"
            | "variant"
            | "dynamic"
            | "nullable"
            | "lowcardinality"
    ) || (matches!(base.as_str(), "decimal" | "date" | "datetime")
        && raw.contains('('));
    if native {
        return raw.to_string();
    }
    let length = length.trim().parse::<u32>().ok().filter(|value| *value > 0);
    let scale = decimals.trim().parse::<u32>().ok().unwrap_or(0);
    match base.as_str() {
        "bool" | "boolean" => "Bool".to_string(),
        "tinyint" if length == Some(1) => "Bool".to_string(),
        "tinyint" => "Int8".to_string(),
        "smallint" => "Int16".to_string(),
        "mediumint" => "Int32".to_string(),
        "int" | "integer" => "Int32".to_string(),
        "bigint" => "Int64".to_string(),
        "decimal" | "numeric" | "dec" => {
            let precision = length.unwrap_or(18).clamp(1, 76);
            format!("Decimal({precision}, {})", scale.min(precision))
        }
        "float" | "real" => "Float32".to_string(),
        "double" | "double precision" => "Float64".to_string(),
        "date" => "Date".to_string(),
        "datetime" | "timestamp" => "DateTime".to_string(),
        "uuid" => "UUID".to_string(),
        // ClickHouse has neither TIME nor a length-limited string: keep the value as text.
        _ => "String".to_string(),
    }
}

/// A whole column definition: the mapped type, `Nullable(…)` and a `DEFAULT` when the source
/// carries a literal one.
fn column_definition(column: &ColumnDef) -> String {
    let mapped = clickhouse_type(&column.data_type, &column.length, &column.decimals);
    let data_type = if column.nullable && !mapped.starts_with("Nullable(") {
        format!("Nullable({mapped})")
    } else {
        mapped
    };
    let mut definition = format!("{} {data_type}", quote_identifier(&column.name));
    let default = column.default.trim();
    if !default.is_empty() {
        definition.push_str(&format!(" DEFAULT {default}"));
    }
    if !column.comment.trim().is_empty() {
        definition.push_str(&format!(" COMMENT {}", quote_literal(&column.comment)));
    }
    definition
}

/// The `CREATE TABLE` script the import wizard runs for a new ClickHouse table. ClickHouse needs a
/// table engine, so a plain `MergeTree` ordered by `tuple()` is used (no column is an identity).
pub(crate) fn create_table_sql(database: &str, table: &str, schema: &TableSchema) -> String {
    if schema.columns.is_empty() {
        return String::new();
    }
    let columns = schema
        .columns
        .iter()
        .map(column_definition)
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!(
        "CREATE TABLE {} ({columns}) ENGINE = MergeTree ORDER BY tuple()",
        table_ref(database, table)
    );
    if !schema.options.comment.trim().is_empty() {
        sql.push_str(&format!(
            " COMMENT {}",
            quote_literal(&schema.options.comment)
        ));
    }
    sql
}

/// One `INSERT` statement for a batch of rows.
pub(crate) fn insert_sql(
    database: &str,
    table: &str,
    columns: &[ColumnInfo],
    rows: &[RowInsert],
) -> String {
    // MATERIALIZED/ALIAS/EPHEMERAL columns are computed by ClickHouse and must not appear in an
    // INSERT at all (even as `DEFAULT`).
    let columns = columns
        .iter()
        .filter(|column| !column.generated)
        .collect::<Vec<_>>();
    if rows.is_empty() || columns.is_empty() {
        return String::new();
    }
    let names = columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let values = rows
        .iter()
        .map(|row| {
            let cells = columns
                .iter()
                .map(|column| {
                    let value = row
                        .values
                        .iter()
                        .find(|(name, _)| name == &column.name)
                        .map(|(_, value)| value);
                    match value {
                        None => "DEFAULT".to_string(),
                        Some(None) => "NULL".to_string(),
                        Some(Some(text)) => literal_for_type(&column.data_type, text),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("({cells})")
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {} ({names}) VALUES {values}",
        table_ref(database, table)
    )
}

/// A value as a ClickHouse literal, bare for numeric columns and quoted for everything else.
fn literal_for_type(data_type: &str, text: &str) -> String {
    if is_numeric(data_type) {
        let trimmed = text.trim();
        if !trimmed.is_empty() && trimmed.parse::<f64>().is_ok() {
            return trimmed.to_string();
        }
        if trimmed.eq_ignore_ascii_case("true") || trimmed.eq_ignore_ascii_case("false") {
            return trimmed.to_ascii_lowercase();
        }
    }
    quote_literal(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, data_type: &str) -> ColumnInfo {
        column_for(name, data_type)
    }

    fn condition(column: &str, operator: FilterOperator, value: &str) -> FilterNode {
        let mut condition = FilterCondition::new(column);
        condition.operator = operator;
        condition.value = value.to_string();
        FilterNode::Condition(condition)
    }

    #[test]
    fn identifiers_and_literals_are_escaped() {
        assert_eq!(quote_identifier("weird`name"), "`weird\\`name`");
        assert_eq!(quote_literal("a'b\\c\n"), "'a\\'b\\\\c\\n'");
        assert_eq!(table_ref("db", "t"), "`db`.`t`",);
    }

    #[test]
    fn result_set_detection_and_format_appending() {
        assert!(returns_result_set("SELECT 1"));
        assert!(returns_result_set(
            "-- note\n  WITH x AS (SELECT 1) SELECT * FROM x"
        ));
        assert!(returns_result_set("SHOW TABLES"));
        assert!(!returns_result_set("INSERT INTO t VALUES (1)"));
        assert!(!returns_result_set(
            "CREATE TABLE t (a Int64) ENGINE = Memory"
        ));

        assert_eq!(
            ensure_row_format("SELECT 1;"),
            format!("SELECT 1\nFORMAT {ROW_FORMAT}")
        );
        let explicit = "SELECT 1 FORMAT JSON";
        assert_eq!(ensure_row_format(explicit), explicit);
        assert!(!has_format_clause("SELECT 'FORMAT'"));
        assert!(has_format_clause("SELECT 1 FORMAT TabSeparated"));
    }

    #[test]
    fn wrappers_and_base_types() {
        assert_eq!(strip_wrappers("Nullable(String)"), "String");
        assert_eq!(strip_wrappers("LowCardinality(Nullable(String))"), "String");
        assert_eq!(base_type("Nullable(Decimal(20, 6))"), "Decimal");
        assert_eq!(base_type("Array(Nullable(Int32))"), "Array");
        assert!(is_nullable("Nullable(Int32)"));
        assert!(is_nullable("LowCardinality(Nullable(String))"));
        assert!(!is_nullable("LowCardinality(String)"));
        assert!(!is_nullable("Array(Nullable(String))"));
    }

    #[test]
    fn page_sql_orders_filters_and_pages() {
        let columns = vec![column("id", "UInt64"), column("name", "String")];
        let filter = vec![
            condition("id", FilterOperator::GreaterOrEqual, "10"),
            condition("name", FilterOperator::Contains, "ab"),
        ];
        let page = PageRequest::new(2, 100)
            .with_filter(filter)
            .with_order_by(vec![rustgrid_core::SortColumn {
                column: "id".to_string(),
                descending: true,
            }]);
        let sql = page_sql("db", "t", &columns, &page);
        assert_eq!(
            sql,
            "SELECT `id`, `name` FROM `db`.`t` WHERE `id` >= 10 AND toString(`name`) LIKE \
             '%ab%' ORDER BY `id` DESC LIMIT 100 OFFSET 200"
        );
    }

    #[test]
    fn filter_quotes_text_columns_and_keeps_numbers_bare() {
        let columns = vec![column("id", "Nullable(Int64)"), column("at", "DateTime")];
        let filter = vec![
            condition("id", FilterOperator::Equal, "7"),
            condition("at", FilterOperator::Equal, "2024-01-01 00:00:00"),
        ];
        let where_clause = filter_clause(&filter, &columns);
        assert_eq!(
            where_clause,
            " WHERE `id` = 7 AND `at` = '2024-01-01 00:00:00'"
        );
        assert_eq!(
            filter_clause(
                &[condition("missing", FilterOperator::Equal, "7")],
                &columns
            ),
            " WHERE `missing` = '7'"
        );
    }

    #[test]
    fn decodes_compact_json_rows() {
        let body = br#"["id","name","tags","amount","at"]
["UInt64","Nullable(String)","Array(String)","Decimal(20, 6)","DateTime"]
["18446744073709551615","x",["a","b"],"12.340000","2024-01-01 00:00:00"]
["2",null,[],"-0.500000","2024-01-02 03:04:05"]
"#;
        let decoded = decode_body(body).expect("decodes");
        assert_eq!(decoded.columns.len(), 5);
        assert_eq!(decoded.columns[0].data_type, "UInt64");
        assert!(decoded.columns[1].nullable);
        assert_eq!(decoded.rows.len(), 2);
        assert_eq!(decoded.rows[0][0], CellValue::Uint(u64::MAX));
        assert_eq!(decoded.rows[0][2], CellValue::Text("[\"a\",\"b\"]".into()));
        assert_eq!(decoded.rows[0][3], CellValue::Text("12.340000".into()));
        assert_eq!(decoded.rows[1][1], CellValue::Null);
        assert_eq!(decoded.rows[1][2], CellValue::Text("[]".into()));
    }

    #[test]
    fn zero_row_results_still_carry_the_header() {
        let body = b"[\"a\"]\n[\"UInt8\"]\n";
        let decoded = decode_body(body).expect("decodes");
        assert_eq!(decoded.columns.len(), 1);
        assert!(decoded.rows.is_empty());
    }

    #[test]
    fn values_are_mapped_by_declared_type() {
        assert_eq!(
            cell_from_json("Int32", &serde_json::json!(-3)),
            CellValue::Int(-3)
        );
        assert_eq!(
            cell_from_json("UInt64", &serde_json::json!("18446744073709551615")),
            CellValue::Uint(u64::MAX)
        );
        assert_eq!(
            cell_from_json(
                "Int128",
                &serde_json::json!("170141183460469231731687303715884105727")
            ),
            CellValue::Text("170141183460469231731687303715884105727".into())
        );
        assert_eq!(
            cell_from_json("Nullable(Decimal(20, 6))", &serde_json::json!("12.34")),
            CellValue::Text("12.340000".into())
        );
        assert_eq!(
            cell_from_json("Decimal32(3)", &serde_json::json!("-0.5")),
            CellValue::Text("-0.500".into())
        );
        assert_eq!(
            cell_from_json("Float64", &serde_json::json!(1.5)),
            CellValue::Float(1.5)
        );
        assert_eq!(
            cell_from_json("Bool", &serde_json::json!(true)),
            CellValue::Bool(true)
        );
        assert_eq!(
            cell_from_json("Nullable(Int8)", &serde_json::json!(null)),
            CellValue::Null
        );
        assert_eq!(
            cell_from_json("Map(String, UInt8)", &serde_json::json!({"a": 1})),
            CellValue::Text("{\"a\":1}".into())
        );
    }

    #[test]
    fn import_types_and_create_table_sql() {
        assert_eq!(clickhouse_type("bigint", "", ""), "Int64");
        assert_eq!(clickhouse_type("decimal", "20", "6"), "Decimal(20, 6)");
        assert_eq!(clickhouse_type("tinyint", "1", ""), "Bool");
        assert_eq!(clickhouse_type("datetime", "", ""), "DateTime");
        assert_eq!(clickhouse_type("Int64", "", ""), "Int64");

        let schema = TableSchema {
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    data_type: "bigint".to_string(),
                    nullable: false,
                    ..ColumnDef::default()
                },
                ColumnDef {
                    name: "name".to_string(),
                    data_type: "varchar".to_string(),
                    length: "100".to_string(),
                    nullable: true,
                    ..ColumnDef::default()
                },
            ],
            ..TableSchema::default()
        };
        assert_eq!(
            create_table_sql("db", "people", &schema),
            "CREATE TABLE `db`.`people` (`id` Int64, `name` Nullable(String)) ENGINE = \
             MergeTree ORDER BY tuple()"
        );
    }

    #[test]
    fn insert_uses_default_null_and_type_aware_literals() {
        let columns = vec![
            column("id", "UInt64"),
            column("name", "String"),
            column("missing", "Int32"),
        ];
        let rows = vec![RowInsert {
            values: vec![
                ("id".to_string(), Some("7".to_string())),
                ("name".to_string(), Some("o'brien".to_string())),
            ],
        }];
        assert_eq!(
            insert_sql("db", "t", &columns, &rows),
            "INSERT INTO `db`.`t` (`id`, `name`, `missing`) VALUES (7, 'o\\'brien', DEFAULT)"
        );
    }
}
