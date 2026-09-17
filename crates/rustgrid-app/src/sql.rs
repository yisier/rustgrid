//! SQL language helpers built on mature third-party crates: `sqlparser` for tokenizing /
//! keyword metadata and `sqlformat` for beautifying, so the app does not hand-roll a SQL lexer.

use sqlparser::dialect::MySqlDialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer, Whitespace};

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
