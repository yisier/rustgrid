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
}
