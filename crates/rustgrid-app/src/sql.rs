//! SQL language helpers built on mature third-party crates: `sqlparser` for tokenizing /
//! keyword metadata and `sqlformat` for beautifying, so the app does not hand-roll a SQL lexer.

use sqlparser::dialect::MySqlDialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer, Whitespace};

use rustgrid_core::RoutineKind;

/// A coarse token category used only for syntax coloring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SqlToken {
    Keyword,
    Identifier,
    String,
    Number,
    Comment,
}

/// A byte range of `sql` together with the category used to color it.
#[derive(Clone, Copy, Debug)]
pub struct SqlSpan {
    pub start: usize,
    pub end: usize,
    pub token: SqlToken,
}

/// Tokenize `sql` with the MySQL dialect and return the colorable spans.
pub fn highlight(sql: &str) -> Vec<SqlSpan> {
    if sql.is_empty() {
        return Vec::new();
    }

    let dialect = MySqlDialect {};
    let mut tokenizer = Tokenizer::new(&dialect, sql);
    let Ok(tokens) = tokenizer.tokenize_with_location() else {
        return Vec::new();
    };

    let starts = line_starts(sql);
    let mut spans = Vec::new();
    for TokenWithSpan { token, span } in tokens {
        let Some(kind) = classify(&token) else {
            continue;
        };
        let start = location_to_offset(sql, &starts, span.start);
        let end = location_to_offset(sql, &starts, span.end);
        if start < end && end <= sql.len() {
            spans.push(SqlSpan {
                start,
                end,
                token: kind,
            });
        }
    }
    spans
}

/// Beautify `sql` using `sqlformat` (uppercasing keywords).
pub fn format(sql: &str) -> String {
    sqlformat::format(
        sql,
        &sqlformat::QueryParams::None,
        &sqlformat::FormatOptions {
            uppercase: Some(true),
            ..Default::default()
        },
    )
}

/// Every keyword understood by `sqlparser`, used as completion candidates.
pub fn keywords() -> &'static [&'static str] {
    sqlparser::keywords::ALL_KEYWORDS
}

/// If `sql` is a simple `SELECT ... FROM <one table>` with no joins, return the table's
/// optional schema and name. Used to decide whether a query result can be edited in place.
pub fn infer_single_table(sql: &str) -> Option<(Option<String>, String)> {
    let dialect = MySqlDialect {};
    let mut tokenizer = Tokenizer::new(&dialect, sql);
    let tokens = tokenizer.tokenize().ok()?;
    let meaningful: Vec<&Token> = tokens
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)) && !matches!(token, Token::EOF))
        .collect();

    if meaningful
        .iter()
        .filter(|token| is_keyword(token, "FROM"))
        .count()
        != 1
    {
        return None;
    }
    if meaningful.iter().any(|token| is_keyword(token, "UNION")) {
        return None;
    }

    let from = meaningful
        .iter()
        .position(|token| is_keyword(token, "FROM"))?;

    if meaningful[from..]
        .iter()
        .any(|token| is_keyword(token, "JOIN") || matches!(token, Token::Comma))
    {
        return None;
    }

    let first = match meaningful.get(from + 1)? {
        Token::Word(word) => word,
        _ => return None,
    };
    if matches!(meaningful.get(from + 2), Some(Token::LParen)) {
        return None;
    }

    if matches!(meaningful.get(from + 2), Some(Token::Period)) {
        let second = match meaningful.get(from + 3)? {
            Token::Word(word) => word,
            _ => return None,
        };
        Some((Some(first.value.clone()), second.value.clone()))
    } else {
        Some((None, first.value.clone()))
    }
}

fn is_keyword(token: &Token, keyword: &str) -> bool {
    matches!(token, Token::Word(word) if word.keyword != Keyword::NoKeyword && word.value.eq_ignore_ascii_case(keyword))
}

/// The significant (non-whitespace, non-EOF) tokens of `sql`, in order.
fn meaningful_tokens(sql: &str) -> Vec<Token> {
    let dialect = MySqlDialect {};
    let mut tokenizer = Tokenizer::new(&dialect, sql);
    match tokenizer.tokenize() {
        Ok(tokens) => tokens
            .into_iter()
            .filter(|token| !matches!(token, Token::Whitespace(_) | Token::EOF))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// The routine kind of a `Word` token, when it is the `FUNCTION`/`PROCEDURE` keyword.
fn routine_kind_of(word: &sqlparser::tokenizer::Word) -> Option<RoutineKind> {
    match word.keyword {
        Keyword::PROCEDURE => Some(RoutineKind::Procedure),
        Keyword::FUNCTION => Some(RoutineKind::Function),
        _ => None,
    }
}

/// Parse a routine's kind and name from its `CREATE ... FUNCTION|PROCEDURE name(...)` statement.
/// Returns `None` when the statement does not name a routine.
pub fn routine_identity(sql: &str) -> Option<(RoutineKind, String)> {
    let tokens = meaningful_tokens(sql);
    let position = tokens
        .iter()
        .position(|token| matches!(token, Token::Word(word) if routine_kind_of(word).is_some()))?;
    let kind = match &tokens[position] {
        Token::Word(word) => routine_kind_of(word)?,
        _ => return None,
    };
    let name = tokens[position + 1..]
        .iter()
        .find_map(|token| match token {
            Token::Word(word) => Some(word.value.clone()),
            _ => None,
        })?;
    Some((kind, name))
}

/// Parse a routine's parameter names from its `CREATE ... name(...)` statement, in order. The
/// parameter list is the first parenthesized group after the routine name; a leading mode keyword
/// (`IN`/`OUT`/`INOUT`) is skipped, so `IN a int` and `a int` both yield `a`.
pub fn routine_parameters(sql: &str) -> Vec<String> {
    let tokens = meaningful_tokens(sql);
    let Some(position) = tokens
        .iter()
        .position(|token| matches!(token, Token::Word(word) if routine_kind_of(word).is_some()))
    else {
        return Vec::new();
    };
    let Some(open) = tokens[position..]
        .iter()
        .position(|token| matches!(token, Token::LParen))
        .map(|offset| position + offset)
    else {
        return Vec::new();
    };

    let mut parameters = Vec::new();
    let mut current: Vec<&Token> = Vec::new();
    let mut depth = 0usize;
    for token in &tokens[open + 1..] {
        match token {
            Token::LParen => {
                depth += 1;
                current.push(token);
            }
            Token::RParen => {
                if depth == 0 {
                    parameters.extend(parameter_name(&current));
                    break;
                }
                depth -= 1;
                current.push(token);
            }
            Token::Comma if depth == 0 => {
                parameters.extend(parameter_name(&current));
                current.clear();
            }
            _ => current.push(token),
        }
    }
    parameters
}

/// The name of one parameter group: the first word, skipping a leading `IN`/`OUT`/`INOUT`.
fn parameter_name(tokens: &[&Token]) -> Option<String> {
    let mut words = tokens.iter().filter_map(|token| match token {
        Token::Word(word) => Some(word.value.clone()),
        _ => None,
    });
    let first = words.next()?;
    if ["IN", "OUT", "INOUT"]
        .iter()
        .any(|mode| first.eq_ignore_ascii_case(mode))
    {
        words.next()
    } else {
        Some(first)
    }
}

/// The view name of a `CREATE ... VIEW \`name\` AS ...` statement, or `None` when the statement
/// does not name a view.
pub fn view_identity(sql: &str) -> Option<String> {
    let tokens = meaningful_tokens(sql);
    let position = tokens
        .iter()
        .position(|token| matches!(token, Token::Word(word) if word.keyword == Keyword::VIEW))?;
    tokens[position + 1..].iter().find_map(|token| match token {
        Token::Word(word) => Some(word.value.clone()),
        _ => None,
    })
}

/// The `SELECT` of a `CREATE ... VIEW name AS <select>` statement — the first `AS` keyword after
/// the `VIEW` keyword — used by the designer's 解释 to `EXPLAIN` the view's query.
pub fn view_select(sql: &str) -> Option<String> {
    let dialect = MySqlDialect {};
    let mut tokenizer = Tokenizer::new(&dialect, sql);
    let tokens = tokenizer.tokenize_with_location().ok()?;
    let starts = line_starts(sql);
    let mut seen_view = false;
    for TokenWithSpan { token, span } in &tokens {
        match token {
            Token::Word(word) if !seen_view && word.keyword == Keyword::VIEW => seen_view = true,
            Token::Word(word) if seen_view && word.keyword == Keyword::AS => {
                let offset = location_to_offset(sql, &starts, span.end);
                let select = sql[offset..].trim().trim_end_matches(';').trim();
                return (!select.is_empty()).then(|| select.to_string());
            }
            _ => {}
        }
    }
    None
}

/// One table referenced by a statement (its `FROM`/`JOIN`/`INTO`/`UPDATE` target), with the
/// database qualifier and alias the user wrote. Used to offer a referenced table's columns and to
/// resolve `alias.` completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqlTableRef {
    /// The database/schema qualifier written before the table, if any.
    pub database: Option<String>,
    pub name: String,
    pub alias: Option<String>,
}

/// Whether the caret is where a table name is expected (after `FROM`/`JOIN`/`INTO`/`UPDATE`) or
/// where a column/expression is expected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SqlCompletionKind {
    Table,
    Column,
}

/// What the query editor should offer at the caret: the identifier fragment completion replaces,
/// the dotted qualifier before it, whether a table name is expected, and the statement's
/// referenced tables.
#[derive(Clone, Debug)]
pub struct SqlCompletionContext {
    /// The lowercased fragment from the identifier start to the caret.
    pub prefix: String,
    /// The byte offset where `prefix` begins (the identifier start). Completion must replace
    /// `prefix_start..caret`, so the editor's completion accept range is anchored here.
    pub prefix_start: usize,
    /// The dotted qualifier parts before the fragment (`["db", "table"]`, `["alias"]`, ...).
    pub qualifier: Vec<String>,
    pub kind: SqlCompletionKind,
    pub tables: Vec<SqlTableRef>,
    /// Whether the caret is inside a string literal or comment, where completion is suppressed.
    pub suppress: bool,
}

/// Analyze `sql` at `caret` (a byte offset) for context-aware completion. The whole statement
/// containing the caret is scanned for table references (so a `SELECT |FROM t` already offers
/// `t`'s columns), while the fragment and qualifier come from the text just before the caret.
pub fn completion_context(sql: &str, caret: usize) -> SqlCompletionContext {
    let end = caret.min(sql.len());
    let mut start = end;
    while start > 0 {
        let previous = previous_boundary(sql, start);
        if is_identifier_char(sql[previous..start].chars().next().unwrap_or(' ')) {
            start = previous;
        } else {
            break;
        }
    }
    let prefix = sql[start..end].to_lowercase();
    let qualifier = qualifier_parts(sql, start);
    let statement = current_statement(sql, end);
    let tables = referenced_tables(&statement);
    let kind = if qualifier.is_empty() && expects_table_name(sql, start) {
        SqlCompletionKind::Table
    } else {
        SqlCompletionKind::Column
    };
    SqlCompletionContext {
        prefix,
        prefix_start: start,
        qualifier,
        kind,
        tables,
        suppress: in_string_or_comment(sql, end),
    }
}

/// The built-in function names offered by completion. MySQL is the bundled engine; a second
/// engine would add its own list behind the driver boundary.
pub fn functions() -> &'static [&'static str] {
    BUILTIN_FUNCTIONS
}

fn is_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$'
}

/// The byte offset of the character before `index` (which must be a char boundary).
fn previous_boundary(text: &str, index: usize) -> usize {
    let mut previous = index.saturating_sub(1);
    while previous > 0 && !text.is_char_boundary(previous) {
        previous -= 1;
    }
    previous
}

/// The dotted qualifier immediately before byte offset `start`, handling `` ` ``/`"`/`[]` quoting.
fn qualifier_parts(sql: &str, start: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut index = start;
    while index > 0 {
        let dot = previous_boundary(sql, index);
        if &sql[dot..index] != "." {
            break;
        }
        let (name_start, name) = identifier_before(sql, dot);
        if name.is_empty() {
            break;
        }
        parts.push(name);
        index = name_start;
    }
    parts.reverse();
    parts
}

/// Read the identifier ending at `end`, unquoting `` ` ``/`"`/`[...]` when present.
fn identifier_before(sql: &str, end: usize) -> (usize, String) {
    if end == 0 {
        return (0, String::new());
    }
    let last = previous_boundary(sql, end);
    let last_char = sql[last..end].chars().next().unwrap_or(' ');
    match last_char {
        '`' | '"' => {
            let mut index = last;
            while index > 0 {
                let previous = previous_boundary(sql, index);
                if sql[previous..index] == sql[last..end] {
                    return (previous, sql[previous + 1..last].to_string());
                }
                index = previous;
            }
            (0, String::new())
        }
        ']' => {
            let mut index = last;
            while index > 0 {
                let previous = previous_boundary(sql, index);
                if &sql[previous..index] == "[" {
                    return (previous, sql[previous + 1..last].to_string());
                }
                index = previous;
            }
            (0, String::new())
        }
        character if is_identifier_char(character) => {
            let mut index = last;
            while index > 0 {
                let previous = previous_boundary(sql, index);
                if is_identifier_char(sql[previous..index].chars().next().unwrap_or(' ')) {
                    index = previous;
                } else {
                    break;
                }
            }
            (index, sql[index..end].to_string())
        }
        _ => (end, String::new()),
    }
}

/// Whether the last word before `before` is a clause that expects a table name next.
fn expects_table_name(sql: &str, before: usize) -> bool {
    let mut index = before;
    while index > 0 {
        let previous = previous_boundary(sql, index);
        if sql[previous..index]
            .chars()
            .next()
            .unwrap_or(' ')
            .is_whitespace()
        {
            index = previous;
        } else {
            break;
        }
    }
    let (_, word) = identifier_before(sql, index);
    matches!(
        word.to_ascii_uppercase().as_str(),
        "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE"
    )
}

/// Every table referenced by `statement` (across `FROM`/`JOIN`/`INTO`/`UPDATE`), with aliases.
pub fn referenced_tables(statement: &str) -> Vec<SqlTableRef> {
    let tokens = meaningful_tokens(statement);
    let mut tables: Vec<SqlTableRef> = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let trigger = matches!(
            &tokens[index],
            Token::Word(word)
                if word.keyword != Keyword::NoKeyword
                    && matches!(
                        word.value.to_ascii_uppercase().as_str(),
                        "FROM" | "JOIN" | "INTO" | "UPDATE"
                    )
        );
        if trigger && let Some((table, next)) = table_reference_at(&tokens, index + 1) {
            if !tables.contains(&table) {
                tables.push(table);
            }
            index = next;
            continue;
        }
        index += 1;
    }
    tables
}

/// Read one table reference starting at `start` (which must be a `Word`, not a subquery), plus its
/// optional `AS alias`/bare alias. Returns the reference and the index after it.
fn table_reference_at(tokens: &[Token], start: usize) -> Option<(SqlTableRef, usize)> {
    if matches!(tokens.get(start), Some(Token::LParen)) {
        return None;
    }
    let mut parts = Vec::new();
    let mut index = start;
    loop {
        match tokens.get(index) {
            Some(Token::Word(word)) => {
                parts.push(word.value.clone());
                index += 1;
            }
            _ => return None,
        }
        if matches!(tokens.get(index), Some(Token::Period)) {
            index += 1;
        } else {
            break;
        }
    }
    let name = parts.pop()?;
    let database = parts.pop();
    let mut alias = None;
    let mut next = index;
    if let Some(Token::Word(word)) = tokens.get(index) {
        if word.keyword == Keyword::AS {
            if let Some(Token::Word(alias_word)) = tokens.get(index + 1) {
                alias = Some(alias_word.value.clone());
                next = index + 2;
            }
        } else if word.keyword == Keyword::NoKeyword {
            alias = Some(word.value.clone());
            next = index + 1;
        }
    }
    Some((
        SqlTableRef {
            database,
            name,
            alias,
        },
        next,
    ))
}

/// The `;`-delimited statement containing `caret`, quotation- and comment-aware. Falls back to the
/// whole text when no boundary is found.
pub fn current_statement(sql: &str, caret: usize) -> String {
    let bytes = sql.as_bytes();
    let mut start = 0usize;
    let mut index = 0usize;
    let mut state = ScanState::Normal;
    while index < bytes.len() {
        let byte = bytes[index];
        match state {
            ScanState::Normal => match byte {
                b'\'' => state = ScanState::Single,
                b'"' => state = ScanState::Double,
                b'`' => state = ScanState::Backtick,
                b'#' => state = ScanState::LineComment,
                b'-' if bytes.get(index + 1) == Some(&b'-') => {
                    state = ScanState::LineComment;
                    index += 1;
                }
                b'/' if bytes.get(index + 1) == Some(&b'*') => {
                    state = ScanState::BlockComment;
                    index += 1;
                }
                b';' => {
                    if index >= caret {
                        return sql[start..index].to_string();
                    }
                    start = index + 1;
                }
                _ => {}
            },
            ScanState::Single => {
                if byte == b'\\' {
                    index += 1;
                } else if byte == b'\'' {
                    state = ScanState::Normal;
                }
            }
            ScanState::Double => {
                if byte == b'\\' {
                    index += 1;
                } else if byte == b'"' {
                    state = ScanState::Normal;
                }
            }
            ScanState::Backtick => {
                if byte == b'`' {
                    state = ScanState::Normal;
                }
            }
            ScanState::LineComment => {
                if byte == b'\n' {
                    state = ScanState::Normal;
                }
            }
            ScanState::BlockComment => {
                if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    state = ScanState::Normal;
                    index += 1;
                }
            }
        }
        index += 1;
    }
    sql[start..].to_string()
}

#[derive(Clone, Copy)]
enum ScanState {
    Normal,
    Single,
    Double,
    Backtick,
    LineComment,
    BlockComment,
}

/// Whether byte offset `caret` sits inside a string literal or comment — a context where the
/// editor must not offer completion.
fn in_string_or_comment(sql: &str, caret: usize) -> bool {
    let bytes = sql.as_bytes();
    let end = caret.min(bytes.len());
    let mut index = 0usize;
    let mut state = ScanState::Normal;
    while index < end {
        let byte = bytes[index];
        match state {
            ScanState::Normal => match byte {
                b'\'' => state = ScanState::Single,
                b'"' => state = ScanState::Double,
                b'`' => state = ScanState::Backtick,
                b'#' => state = ScanState::LineComment,
                b'-' if bytes.get(index + 1) == Some(&b'-') => {
                    state = ScanState::LineComment;
                    index += 1;
                }
                b'/' if bytes.get(index + 1) == Some(&b'*') => {
                    state = ScanState::BlockComment;
                    index += 1;
                }
                _ => {}
            },
            ScanState::Single => {
                if byte == b'\\' {
                    index += 1;
                } else if byte == b'\'' {
                    state = ScanState::Normal;
                }
            }
            ScanState::Double => {
                if byte == b'\\' {
                    index += 1;
                } else if byte == b'"' {
                    state = ScanState::Normal;
                }
            }
            ScanState::Backtick => {
                if byte == b'`' {
                    state = ScanState::Normal;
                }
            }
            ScanState::LineComment => {
                if byte == b'\n' {
                    state = ScanState::Normal;
                }
            }
            ScanState::BlockComment => {
                if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    state = ScanState::Normal;
                    index += 1;
                }
            }
        }
        index += 1;
    }
    matches!(
        state,
        ScanState::Single | ScanState::Double | ScanState::LineComment | ScanState::BlockComment
    )
}

/// Common MySQL functions. Kept static so the editor can offer them without a database round-trip.
const BUILTIN_FUNCTIONS: &[&str] = &[
    "ABS",
    "ACOS",
    "ADDDATE",
    "ADDTIME",
    "AES_DECRYPT",
    "AES_ENCRYPT",
    "ASCII",
    "ASIN",
    "ATAN",
    "ATAN2",
    "AVG",
    "BENCHMARK",
    "BIN",
    "BIN_TO_UUID",
    "BIT_AND",
    "BIT_LENGTH",
    "BIT_OR",
    "BIT_XOR",
    "CAST",
    "CEIL",
    "CEILING",
    "CHAR",
    "CHAR_LENGTH",
    "CHARACTER_LENGTH",
    "CHARSET",
    "COALESCE",
    "COERCIBILITY",
    "COLLATION",
    "COMPRESS",
    "CONCAT",
    "CONCAT_WS",
    "CONNECTION_ID",
    "CONV",
    "CONVERT",
    "CONVERT_TZ",
    "COS",
    "COT",
    "COUNT",
    "CRC32",
    "CUME_DIST",
    "CURDATE",
    "CURRENT_DATE",
    "CURRENT_ROLE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "CURTIME",
    "DATABASE",
    "DATE",
    "DATE_ADD",
    "DATE_FORMAT",
    "DATE_SUB",
    "DATEDIFF",
    "DAY",
    "DAYNAME",
    "DAYOFMONTH",
    "DAYOFWEEK",
    "DAYOFYEAR",
    "DEGREES",
    "DENSE_RANK",
    "ELT",
    "EXP",
    "EXPORT_SET",
    "EXTRACT",
    "FIELD",
    "FIND_IN_SET",
    "FIRST_VALUE",
    "FLOOR",
    "FORMAT",
    "FOUND_ROWS",
    "FROM_BASE64",
    "FROM_DAYS",
    "FROM_UNIXTIME",
    "GET_FORMAT",
    "GET_LOCK",
    "GREATEST",
    "GROUP_CONCAT",
    "HEX",
    "HOUR",
    "IF",
    "IFNULL",
    "INET6_ATON",
    "INET6_NTOA",
    "INET_ATON",
    "INET_NTOA",
    "INSERT",
    "INSTR",
    "IS_FREE_LOCK",
    "IS_IPV4",
    "IS_IPV6",
    "IS_USED_LOCK",
    "IS_UUID",
    "ISNULL",
    "JSON_ARRAY",
    "JSON_ARRAYAGG",
    "JSON_ARRAY_APPEND",
    "JSON_ARRAY_INSERT",
    "JSON_CONTAINS",
    "JSON_CONTAINS_PATH",
    "JSON_DEPTH",
    "JSON_EXTRACT",
    "JSON_INSERT",
    "JSON_KEYS",
    "JSON_LENGTH",
    "JSON_MERGE",
    "JSON_MERGE_PATCH",
    "JSON_MERGE_PRESERVE",
    "JSON_OBJECT",
    "JSON_OBJECTAGG",
    "JSON_OVERLAPS",
    "JSON_PRETTY",
    "JSON_QUOTE",
    "JSON_REMOVE",
    "JSON_REPLACE",
    "JSON_SEARCH",
    "JSON_SET",
    "JSON_STORAGE_FREE",
    "JSON_STORAGE_SIZE",
    "JSON_TYPE",
    "JSON_UNQUOTE",
    "JSON_VALID",
    "JSON_VALUE",
    "LAG",
    "LAST_DAY",
    "LAST_INSERT_ID",
    "LAST_VALUE",
    "LCASE",
    "LEAD",
    "LEAST",
    "LEFT",
    "LENGTH",
    "LN",
    "LOAD_FILE",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "LOCATE",
    "LOG",
    "LOG10",
    "LOG2",
    "LOWER",
    "LPAD",
    "LTRIM",
    "MAKEDATE",
    "MAKETIME",
    "MAX",
    "MD5",
    "MICROSECOND",
    "MID",
    "MIN",
    "MINUTE",
    "MOD",
    "MONTH",
    "MONTHNAME",
    "NAME_CONST",
    "NOW",
    "NTH_VALUE",
    "NTILE",
    "NULLIF",
    "OCT",
    "OCTET_LENGTH",
    "ORD",
    "PERCENT_RANK",
    "PERIOD_ADD",
    "PERIOD_DIFF",
    "PI",
    "POSITION",
    "POW",
    "POWER",
    "QUARTER",
    "QUOTE",
    "RADIANS",
    "RAND",
    "RANDOM_BYTES",
    "RANK",
    "REGEXP_INSTR",
    "REGEXP_LIKE",
    "REGEXP_REPLACE",
    "REGEXP_SUBSTR",
    "RELEASE_ALL_LOCKS",
    "RELEASE_LOCK",
    "REPEAT",
    "REPLACE",
    "REVERSE",
    "RIGHT",
    "ROUND",
    "ROW_COUNT",
    "ROW_NUMBER",
    "RPAD",
    "RTRIM",
    "SCHEMA",
    "SEC_TO_TIME",
    "SECOND",
    "SESSION_USER",
    "SHA1",
    "SHA2",
    "SIGN",
    "SIN",
    "SLEEP",
    "SOUNDEX",
    "SPACE",
    "SQRT",
    "STD",
    "STDDEV",
    "STDDEV_POP",
    "STDDEV_SAMP",
    "STR_TO_DATE",
    "SUBDATE",
    "SUBSTR",
    "SUBSTRING",
    "SUBSTRING_INDEX",
    "SUBTIME",
    "SUM",
    "SYSDATE",
    "SYSTEM_USER",
    "TAN",
    "TIME",
    "TIME_FORMAT",
    "TIME_TO_SEC",
    "TIMEDIFF",
    "TIMESTAMP",
    "TIMESTAMPADD",
    "TIMESTAMPDIFF",
    "TO_BASE64",
    "TO_DAYS",
    "TO_SECONDS",
    "TRIM",
    "TRUNCATE",
    "UCASE",
    "UNCOMPRESS",
    "UNCOMPRESSED_LENGTH",
    "UNHEX",
    "UNIX_TIMESTAMP",
    "UPPER",
    "USER",
    "UTC_DATE",
    "UTC_TIME",
    "UTC_TIMESTAMP",
    "UUID",
    "UUID_SHORT",
    "UUID_TO_BIN",
    "VALIDATE_PASSWORD_STRENGTH",
    "VAR_POP",
    "VAR_SAMP",
    "VARIANCE",
    "VERSION",
    "WEEK",
    "WEEKDAY",
    "WEEKOFYEAR",
    "WEIGHT_STRING",
    "YEAR",
    "YEARWEEK",
];

fn classify(token: &Token) -> Option<SqlToken> {
    match token {
        Token::Word(word) => Some(if word.quote_style.is_some() {
            SqlToken::Identifier
        } else if word.keyword != Keyword::NoKeyword {
            SqlToken::Keyword
        } else {
            SqlToken::Identifier
        }),
        Token::Number(_, _) => Some(SqlToken::Number),
        Token::SingleQuotedString(_)
        | Token::DoubleQuotedString(_)
        | Token::TripleSingleQuotedString(_)
        | Token::TripleDoubleQuotedString(_)
        | Token::DollarQuotedString(_)
        | Token::SingleQuotedByteStringLiteral(_)
        | Token::DoubleQuotedByteStringLiteral(_)
        | Token::TripleSingleQuotedByteStringLiteral(_)
        | Token::TripleDoubleQuotedByteStringLiteral(_)
        | Token::SingleQuotedRawStringLiteral(_)
        | Token::DoubleQuotedRawStringLiteral(_)
        | Token::TripleSingleQuotedRawStringLiteral(_)
        | Token::TripleDoubleQuotedRawStringLiteral(_)
        | Token::NationalStringLiteral(_)
        | Token::QuoteDelimitedStringLiteral(_)
        | Token::NationalQuoteDelimitedStringLiteral(_)
        | Token::EscapedStringLiteral(_)
        | Token::UnicodeStringLiteral(_)
        | Token::HexStringLiteral(_) => Some(SqlToken::String),
        Token::Whitespace(
            Whitespace::SingleLineComment { .. } | Whitespace::MultiLineComment(_),
        ) => Some(SqlToken::Comment),
        _ => None,
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

/// Convert a `sqlparser` `Location` (1-based line/column, counted in characters, end-exclusive)
/// into a byte offset in `text`.
fn location_to_offset(text: &str, starts: &[usize], location: Location) -> usize {
    let line = location.line as usize;
    if line == 0 || line > starts.len() {
        return text.len();
    }
    let start = starts[line - 1];
    let line_end = starts.get(line).copied().unwrap_or(text.len());
    let line_text = &text[start..line_end];

    let mut offset = start;
    for (column, character) in (1u64..).zip(line_text.chars()) {
        if column >= location.column {
            break;
        }
        offset += character.len_utf8();
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_keywords_strings_and_comments() {
        let sql = "SELECT 'a' -- hi\nFROM t WHERE id = 1";
        let spans = highlight(sql);
        let kinds: Vec<SqlToken> = spans.iter().map(|span| span.token).collect();
        assert!(kinds.contains(&SqlToken::Keyword));
        assert!(kinds.contains(&SqlToken::String));
        assert!(kinds.contains(&SqlToken::Comment));
        assert!(kinds.contains(&SqlToken::Number));
        for span in &spans {
            assert!(span.start < span.end);
            assert!(span.end <= sql.len());
            assert!(sql.is_char_boundary(span.start));
            assert!(sql.is_char_boundary(span.end));
        }
    }

    #[test]
    fn formatting_uppercases_keywords() {
        let formatted = format("select id from users where a = 1");
        assert!(formatted.contains("SELECT"));
        assert!(formatted.contains("FROM"));
    }

    #[test]
    fn keyword_list_is_populated() {
        assert!(keywords().contains(&"SELECT"));
    }

    #[test]
    fn parses_routine_identity_and_parameters() {
        let procedure = "CREATE DEFINER=`root`@`localhost` PROCEDURE `123`(IN a int, b varchar(10))\nBEGIN\nEND";
        assert_eq!(
            routine_identity(procedure),
            Some((RoutineKind::Procedure, "123".to_string()))
        );
        assert_eq!(routine_parameters(procedure), vec!["a", "b"]);

        let function = "CREATE FUNCTION new_function()\nRETURNS int\nBEGIN\nRETURN 0;\nEND";
        assert_eq!(
            routine_identity(function),
            Some((RoutineKind::Function, "new_function".to_string()))
        );
        assert!(routine_parameters(function).is_empty());

        assert_eq!(routine_identity("SELECT 1"), None);
    }

    #[test]
    fn parses_view_identity_and_select() {
        let definition = "CREATE ALGORITHM=UNDEFINED DEFINER=`root`@`localhost` SQL SECURITY DEFINER \
                          VIEW `view_wms_count_inventory` AS SELECT `a` AS `id` FROM `t`";
        assert_eq!(
            view_identity(definition),
            Some("view_wms_count_inventory".to_string())
        );
        assert_eq!(
            view_select(definition),
            Some("SELECT `a` AS `id` FROM `t`".to_string())
        );
        assert_eq!(view_identity("SELECT 1"), None);
        assert_eq!(view_select("SELECT 1"), None);
    }

    #[test]
    fn infers_editable_single_table_queries() {
        assert_eq!(
            infer_single_table("SELECT * FROM users"),
            Some((None, "users".to_string()))
        );
        assert_eq!(
            infer_single_table("select id from `app`.`users` where id = 1"),
            Some((Some("app".to_string()), "users".to_string()))
        );
        assert_eq!(
            infer_single_table("SELECT a FROM t1 JOIN t2 ON t1.id = t2.id"),
            None
        );
        assert_eq!(infer_single_table("SELECT a FROM t1, t2"), None);
        assert_eq!(infer_single_table("SELECT 1"), None);
        assert_eq!(infer_single_table("UPDATE users SET a = 1"), None);
    }

    #[test]
    fn lists_builtin_functions() {
        assert!(functions().contains(&"COUNT"));
        assert!(functions().contains(&"JSON_EXTRACT"));
    }

    #[test]
    fn extracts_referenced_tables_and_aliases() {
        let tables =
            referenced_tables("SELECT a.id FROM `app`.`users` AS a JOIN orders o ON o.uid = a.id");
        assert_eq!(
            tables,
            vec![
                SqlTableRef {
                    database: Some("app".to_string()),
                    name: "users".to_string(),
                    alias: Some("a".to_string()),
                },
                SqlTableRef {
                    database: None,
                    name: "orders".to_string(),
                    alias: Some("o".to_string()),
                },
            ]
        );
    }

    #[test]
    fn analyzes_completion_context() {
        // The table after the caret is still offered (mirrors `SELECT |FROM t`).
        let sql = "SELECT \nFROM ams_item";
        let context = completion_context(sql, "SELECT \n".len());
        assert_eq!(context.prefix, "");
        assert_eq!(context.kind, SqlCompletionKind::Column);
        assert_eq!(context.tables.len(), 1);
        assert_eq!(context.tables[0].name, "ams_item");

        // A partial identifier after FROM expects a table name.
        let sql = "SELECT * FROM ams";
        let context = completion_context(sql, sql.len());
        assert_eq!(context.prefix, "ams");
        assert_eq!(context.prefix_start, "SELECT * FROM ".len());
        assert_eq!(context.kind, SqlCompletionKind::Table);

        // `alias.` and `db.table.` qualifiers are split out.
        let sql = "SELECT a. FROM t a";
        let context = completion_context(sql, "SELECT a.".len());
        assert_eq!(context.prefix, "");
        assert_eq!(context.qualifier, vec!["a".to_string()]);

        let sql = "SELECT app.us.";
        let context = completion_context(sql, sql.len());
        assert_eq!(context.qualifier, vec!["app".to_string(), "us".to_string()]);

        // Completion is suppressed inside strings and comments.
        assert!(completion_context("SELECT 'abc", "SELECT 'abc".len()).suppress);
        assert!(completion_context("SELECT 1 -- x", "SELECT 1 -- x".len()).suppress);
        assert!(!completion_context("SELECT a.", "SELECT a.".len()).suppress);
    }

    #[test]
    fn scopes_statements_around_the_caret() {
        assert_eq!(current_statement("SELECT 1; SELECT 2", 5), "SELECT 1");
        assert_eq!(current_statement("SELECT 1; SELECT 2", 12), " SELECT 2");
        assert_eq!(
            current_statement("SELECT ';' FROM t", 5),
            "SELECT ';' FROM t"
        );
    }
}
