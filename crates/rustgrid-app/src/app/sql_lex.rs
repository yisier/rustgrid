//! A small SQL lexer for the query editor: one scan that both colours the text and decides what
//! is a comment before a query is sent, so what looks commented out is exactly what does not run.
//!
//! Comments follow Navicat and the common editors rather than any one engine: `--` (with or without
//! a following space), `//`, `/* … */` (an unclosed one runs to the end of the text) and, on MySQL
//! / MariaDB, `#`. A comment inside a string, a quoted identifier or a PostgreSQL dollar-quoted
//! body is not one. [`prepare_for_server`] then rewrites the comments the engine itself would not
//! accept, and [`split_script`] cuts a script into the statements that run one by one (honouring
//! the MySQL client's `DELIMITER` command and routine bodies' `BEGIN … END`).

use std::ops::Range;

/// What a highlighted span is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenKind {
    Keyword,
    Function,
    String,
    Number,
    Comment,
}

/// The comment forms, as the server-side rewrite needs to tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Comment {
    /// `-- …`; `spaced` when a whitespace or control character (or the end) follows the dashes,
    /// which MySQL requires.
    Dash { spaced: bool },
    /// `# …` (MySQL / MariaDB).
    Hash,
    /// `// …`: RustGrid's and Navicat's, no engine accepts it.
    Slash,
    /// `/* … */`; `closed` when its `*/` exists.
    Block { closed: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    Keyword,
    Function,
    String,
    Number,
    Comment(Comment),
    /// A top-level statement delimiter (`;`, or the one a `DELIMITER` line set).
    Delimiter,
    /// A MySQL client `DELIMITER xx` line (the whole line); it is never sent to the server.
    DelimiterCommand,
}

/// The keywords coloured as such (upper case, sorted for the binary search).
const KEYWORDS: &[&str] = &[
    "ADD",
    "AFTER",
    "ALL",
    "ALTER",
    "ANALYZE",
    "AND",
    "ANY",
    "AS",
    "ASC",
    "AUTO_INCREMENT",
    "BEFORE",
    "BEGIN",
    "BETWEEN",
    "BIGINT",
    "BINARY",
    "BLOB",
    "BOOLEAN",
    "BOTH",
    "BY",
    "CALL",
    "CASCADE",
    "CASE",
    "CHANGE",
    "CHAR",
    "CHARACTER",
    "CHARSET",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "COLUMNS",
    "COMMENT",
    "COMMIT",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURSOR",
    "DATABASE",
    "DATABASES",
    "DATE",
    "DATETIME",
    "DECIMAL",
    "DECLARE",
    "DEFAULT",
    "DELETE",
    "DELIMITER",
    "DESC",
    "DESCRIBE",
    "DISTINCT",
    "DIV",
    "DO",
    "DOUBLE",
    "DROP",
    "DUPLICATE",
    "EACH",
    "ELSE",
    "ELSEIF",
    "END",
    "ENGINE",
    "ENUM",
    "ESCAPE",
    "EVENT",
    "EXCEPT",
    "EXEC",
    "EXECUTE",
    "EXISTS",
    "EXPLAIN",
    "FALSE",
    "FETCH",
    "FIRST",
    "FLOAT",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "FUNCTION",
    "GRANT",
    "GROUP",
    "HANDLER",
    "HAVING",
    "IF",
    "IGNORE",
    "ILIKE",
    "IN",
    "INDEX",
    "INNER",
    "INOUT",
    "INSERT",
    "INT",
    "INTEGER",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "IS",
    "JOIN",
    "JSON",
    "KEY",
    "KEYS",
    "LANGUAGE",
    "LAST",
    "LEADING",
    "LEAVE",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LOCK",
    "LONGTEXT",
    "LOOP",
    "MEDIUMTEXT",
    "MOD",
    "MODIFY",
    "NATURAL",
    "NOT",
    "NULL",
    "NULLS",
    "NUMERIC",
    "OF",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUT",
    "OUTER",
    "OVER",
    "PARTITION",
    "PRIMARY",
    "PROCEDURE",
    "PROCESSLIST",
    "REAL",
    "RECURSIVE",
    "REFERENCES",
    "REGEXP",
    "RENAME",
    "REPEAT",
    "REPLACE",
    "RETURN",
    "RETURNING",
    "RETURNS",
    "REVOKE",
    "RIGHT",
    "RLIKE",
    "ROLLBACK",
    "ROW",
    "ROWS",
    "SAVEPOINT",
    "SCHEMA",
    "SCHEMAS",
    "SELECT",
    "SEQUENCE",
    "SESSION",
    "SET",
    "SHOW",
    "SIGNAL",
    "SMALLINT",
    "START",
    "STATUS",
    "TABLE",
    "TABLES",
    "TEMPORARY",
    "TEXT",
    "THEN",
    "TIME",
    "TIMESTAMP",
    "TINYINT",
    "TO",
    "TOP",
    "TRAILING",
    "TRANSACTION",
    "TRIGGER",
    "TRUE",
    "TRUNCATE",
    "TYPE",
    "UNION",
    "UNIQUE",
    "UNLOCK",
    "UNSIGNED",
    "UNTIL",
    "UPDATE",
    "USE",
    "USING",
    "VALUES",
    "VARCHAR",
    "VARIABLES",
    "VIEW",
    "WHEN",
    "WHERE",
    "WHILE",
    "WINDOW",
    "WITH",
    "XOR",
];

fn is_keyword(word: &str) -> bool {
    word.len() <= 17
        && KEYWORDS
            .binary_search_by(|keyword| {
                keyword
                    .bytes()
                    .cmp(word.bytes().map(|byte| byte.to_ascii_uppercase()))
            })
            .is_ok()
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80
}

/// Whether the delimiter starting at `at` ends a statement there. A word-like delimiter (`GO`,
/// `END`) only does so when no word character follows it, so `CATEGORY` does not end one for
/// `DELIMITER GO` (the `mysql` client reads its delimiter at the end of a line); a symbolic one
/// (`//`, `$$`) matches anywhere, as `1$$` is how a routine's last line often reads.
fn delimiter_ends_here(delimiter: &str, bytes: &[u8], at: usize) -> bool {
    let word_like = delimiter
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if !word_like {
        return true;
    }
    bytes
        .get(at + delimiter.len())
        .is_none_or(|next| !(next.is_ascii_alphanumeric() || *next == b'_'))
}

/// Whether a custom delimiter (not the default `;`, which the scanner handles itself) starts at
/// `at`, so the word / number / variable scans stop there instead of swallowing it (`END$$`).
fn delimiter_starts(delimiter: &str, bytes: &[u8], at: usize) -> bool {
    delimiter != ";"
        && bytes[at..].starts_with(delimiter.as_bytes())
        && delimiter_ends_here(delimiter, bytes, at)
}

/// Scan `sql`, reporting each coloured span in order. `mysql`: MySQL / MariaDB rules (`#`
/// comments, backslash escapes and `"…"` strings; no dollar quoting).
fn scan(sql: &str, mysql: bool, mut emit: impl FnMut(Range<usize>, Token)) {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let line_end = |from: usize| sql[from..].find('\n').map_or(len, |at| from + at);
    // The end of a quoted run opened at `start` by `quote` (just past the closing quote, or the
    // end of the text when it is unterminated).
    let quoted_end = |start: usize, quote: u8, backslash: bool| {
        let mut i = start + 1;
        while i < len {
            if backslash && bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                if bytes.get(i + 1) == Some(&quote) {
                    i += 2;
                    continue;
                }
                return i + 1;
            }
            i += 1;
        }
        len
    };
    // The statement delimiter; a MySQL `DELIMITER` line changes it.
    let mut delimiter = String::from(";");
    let mut i = 0;
    while i < len {
        let byte = bytes[i];
        // A custom delimiter (`$$`, `//`, ...) wins over everything but strings and comments
        // already being skipped, so `END //` ends the routine rather than starting a comment.
        if delimiter_starts(&delimiter, bytes, i) {
            emit(i..i + delimiter.len(), Token::Delimiter);
            i += delimiter.len();
            continue;
        }
        match byte {
            b';' if delimiter == ";" => {
                emit(i..i + 1, Token::Delimiter);
                i += 1;
            }
            b'\'' => {
                let end = quoted_end(i, b'\'', mysql);
                emit(i..end, Token::String);
                i = end;
            }
            b'"' => {
                // A string on MySQL; a quoted identifier elsewhere (not coloured).
                let end = quoted_end(i, b'"', mysql);
                if mysql {
                    emit(i..end, Token::String);
                }
                i = end;
            }
            b'`' => i = quoted_end(i, b'`', false),
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                let spaced = bytes
                    .get(i + 2)
                    .is_none_or(|next| next.is_ascii_whitespace() || next.is_ascii_control());
                let end = line_end(i);
                emit(i..end, Token::Comment(Comment::Dash { spaced }));
                i = end;
            }
            b'#' if mysql => {
                let end = line_end(i);
                emit(i..end, Token::Comment(Comment::Hash));
                i = end;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                let end = line_end(i);
                emit(i..end, Token::Comment(Comment::Slash));
                i = end;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let (end, closed) = sql[i + 2..]
                    .find("*/")
                    .map_or((len, false), |at| (i + 2 + at + 2, true));
                emit(i..end, Token::Comment(Comment::Block { closed }));
                i = end;
            }
            b'$' if !mysql => {
                // PostgreSQL dollar quoting: $tag$ … $tag$ (the tag may be empty).
                let tag_end = sql[i + 1..]
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map(|at| i + 1 + at);
                match tag_end.filter(|&end| bytes.get(end) == Some(&b'$')) {
                    Some(end) => {
                        let tag = &sql[i..=end];
                        let close = sql[end + 1..]
                            .find(tag)
                            .map_or(len, |at| end + 1 + at + tag.len());
                        emit(i..close, Token::String);
                        i = close;
                    }
                    None => i += 1,
                }
            }
            b'0'..=b'9' => {
                let start = i;
                while i < len
                    && (is_word_byte(bytes[i]) || bytes[i] == b'.')
                    && !delimiter_starts(&delimiter, bytes, i)
                {
                    // An exponent's sign: 1e-5.
                    if matches!(bytes[i], b'e' | b'E')
                        && matches!(bytes.get(i + 1), Some(b'+' | b'-'))
                    {
                        i += 1;
                    }
                    i += 1;
                }
                emit(start..i, Token::Number);
            }
            b'@' => {
                // A variable (@x, @@global.x): never a keyword.
                i += 1;
                while i < len
                    && (is_word_byte(bytes[i]) || matches!(bytes[i], b'@' | b'.'))
                    && !delimiter_starts(&delimiter, bytes, i)
                {
                    i += 1;
                }
            }
            byte if is_word_byte(byte) => {
                let start = i;
                // A delimiter that is made of word characters (`$$`) ends the word here: `END$$`
                // would otherwise be read as one identifier.
                while i < len && is_word_byte(bytes[i]) && !delimiter_starts(&delimiter, bytes, i) {
                    i += 1;
                }
                let word = &sql[start..i];
                let line_start = sql[..start].rfind('\n').map_or(0, |at| at + 1);
                if mysql
                    && word.eq_ignore_ascii_case("delimiter")
                    && sql[line_start..start].trim().is_empty()
                {
                    // The MySQL client's `DELIMITER xx`: the rest of the line is the new one.
                    let end = line_end(start);
                    if let Some(next) = sql[i..end].split_whitespace().next() {
                        delimiter = next.to_string();
                    }
                    emit(start..end, Token::DelimiterCommand);
                    i = end;
                } else if is_keyword(word) {
                    emit(start..i, Token::Keyword);
                } else if sql[i..].trim_start_matches([' ', '\t']).starts_with('(') {
                    emit(start..i, Token::Function);
                }
            }
            _ => i += 1,
        }
    }
}

/// The coloured spans of `sql`, for the editor (see [`scan`] for `mysql`).
pub(crate) fn tokens(sql: &str, mysql: bool) -> Vec<(Range<usize>, TokenKind)> {
    let mut out = Vec::new();
    scan(sql, mysql, |range, token| {
        let kind = match token {
            Token::Keyword => TokenKind::Keyword,
            Token::Function => TokenKind::Function,
            Token::String => TokenKind::String,
            Token::Number => TokenKind::Number,
            Token::Comment(_) => TokenKind::Comment,
            Token::DelimiterCommand => TokenKind::Keyword,
            Token::Delimiter => return,
        };
        out.push((range, kind));
    });
    out
}

/// `sql` as sent to the server: every comment the editor shows stays a comment there. `//`
/// comments and an unclosed `/*` (to the end) are removed, since no engine accepts them; on
/// MySQL / MariaDB a `--` not followed by a space gets one, because MySQL only treats `-- ` as a
/// comment. Valid comments are kept as written (they may document a routine body).
pub(crate) fn prepare_for_server(sql: &str, mysql: bool) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut from = 0;
    scan(sql, mysql, |range, token| {
        let Token::Comment(comment) = token else {
            return;
        };
        match comment {
            Comment::Slash | Comment::Block { closed: false } => {
                out.push_str(&sql[from..range.start]);
                from = range.end;
            }
            Comment::Dash { spaced: false } if mysql => {
                out.push_str(&sql[from..range.start + 2]);
                out.push(' ');
                from = range.start + 2;
            }
            _ => {}
        }
    });
    out.push_str(&sql[from..]);
    out
}

/// Whether `sql` holds anything but whitespace and comments (a comment-only fragment is not a
/// statement: MySQL would answer it with "Query was empty"). On MySQL / MariaDB a conditional
/// comment (`/*!40101 SET ... */`) is executed by the server, so it counts as code.
pub(crate) fn has_code(sql: &str, mysql: bool) -> bool {
    let mut comments: Vec<Range<usize>> = Vec::new();
    let mut executable = false;
    scan(sql, mysql, |range, token| {
        if matches!(token, Token::Comment(_) | Token::DelimiterCommand) {
            if mysql && sql[range.clone()].starts_with("/*!") {
                executable = true;
            }
            comments.push(range);
        }
    });
    if executable {
        return true;
    }
    let mut from = 0;
    for range in comments {
        if !sql[from..range.start].trim().is_empty() {
            return true;
        }
        from = range.end;
    }
    !sql[from..].trim().is_empty()
}

/// The statements of a script, as byte ranges (trimmed, delimiters and `DELIMITER` lines
/// excluded, comment-only fragments dropped), in the order they run.
///
/// Statements end at a top-level delimiter. On MySQL / MariaDB a `DELIMITER xx` line changes it,
/// as in the `mysql` client and Navicat, and the body of a `CREATE PROCEDURE / FUNCTION / TRIGGER
/// / EVENT` is kept whole without one: its `BEGIN … END` (and `CASE … END`) blocks are tracked,
/// so the `;` inside them do not cut it.
pub(crate) fn split_script(sql: &str, mysql: bool) -> Vec<Range<usize>> {
    let mut events: Vec<(Range<usize>, Token)> = Vec::new();
    scan(sql, mysql, |range, token| {
        if matches!(
            token,
            Token::Keyword | Token::Delimiter | Token::DelimiterCommand
        ) {
            events.push((range, token));
        }
    });
    let mut statements = Vec::new();
    let mut push = |range: Range<usize>| {
        let text = &sql[range.clone()];
        let start = range.start + (text.len() - text.trim_start().len());
        let end = range.start + text.trim_end().len();
        if start < end && has_code(&sql[start..end], mysql) {
            statements.push(start..end);
        }
    };
    let word = |range: &Range<usize>| sql[range.clone()].to_ascii_uppercase();
    let mut start = 0;
    // Whether the statement so far is `CREATE … PROCEDURE/FUNCTION/TRIGGER/EVENT`, its first
    // keyword, and the depth of the routine body's blocks.
    let mut first_keyword: Option<String> = None;
    let mut routine = false;
    let mut depth = 0usize;
    let mut index = 0;
    while index < events.len() {
        let (range, token) = events[index].clone();
        match token {
            Token::DelimiterCommand => {
                push(start..range.start);
                start = range.end;
                first_keyword = None;
                routine = false;
                depth = 0;
            }
            Token::Delimiter if depth == 0 => {
                push(start..range.start);
                start = range.end;
                first_keyword = None;
                routine = false;
            }
            Token::Keyword if mysql => {
                let keyword = word(&range);
                if first_keyword.is_none() {
                    first_keyword = Some(keyword.clone());
                }
                let creating = first_keyword.as_deref() == Some("CREATE");
                match keyword.as_str() {
                    "PROCEDURE" | "FUNCTION" | "TRIGGER" | "EVENT" if creating && depth == 0 => {
                        routine = true;
                    }
                    "BEGIN" if routine => depth += 1,
                    "CASE" if routine && depth > 0 => depth += 1,
                    "END" if routine && depth > 0 => {
                        // `END IF` / `END LOOP` / `END WHILE` / `END REPEAT` close blocks that
                        // were never counted; `END` and `END CASE` close a counted one.
                        let next = events
                            .get(index + 1)
                            .filter(|(_, token)| *token == Token::Keyword)
                            .map(|(range, _)| word(range));
                        match next.as_deref() {
                            Some("IF" | "LOOP" | "WHILE" | "REPEAT") => index += 1,
                            Some("CASE") => {
                                index += 1;
                                depth -= 1;
                            }
                            _ => depth -= 1,
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        index += 1;
    }
    push(start..sql.len());
    statements
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comments(sql: &str, mysql: bool) -> Vec<&str> {
        tokens(sql, mysql)
            .into_iter()
            .filter(|(_, kind)| *kind == TokenKind::Comment)
            .map(|(range, _)| &sql[range])
            .collect()
    }

    #[test]
    fn every_comment_form_runs_to_the_end_of_its_line() {
        let sql = "a --x\nb -- y\nc //z\nd #w\ne /* v */ f";
        assert_eq!(comments(sql, true), ["--x", "-- y", "//z", "#w", "/* v */"]);
        // `#` is an operator outside MySQL / MariaDB (PostgreSQL's XOR, SQL Server's #temp).
        assert_eq!(comments(sql, false), ["--x", "-- y", "//z", "/* v */"]);
    }

    #[test]
    fn an_unclosed_block_comment_runs_to_the_end() {
        let sql = "SELECT 1 /* open\nSELECT 2";
        assert_eq!(comments(sql, true), ["/* open\nSELECT 2"]);
        assert_eq!(prepare_for_server(sql, true), "SELECT 1 ");
    }

    #[test]
    fn comment_markers_inside_strings_and_identifiers_are_text() {
        let sql = "SELECT 'http://x -- #', `a//b`, \"c/*d\" FROM t";
        assert!(comments(sql, true).is_empty());
        assert_eq!(prepare_for_server(sql, true), sql);
        assert!(comments("SELECT $$ // $$", false).is_empty());
        assert!(comments(r"SELECT 'it\'s // x'", true).is_empty());
        // Without backslash escapes (PostgreSQL) a backslash is an ordinary character.
        assert_eq!(comments(r"SELECT 'C:\' // x", false), ["// x"]);
    }

    #[test]
    fn the_server_gets_only_comments_it_accepts() {
        assert_eq!(
            prepare_for_server("SELECT 1; // note\nSELECT 2; --tight\n", true),
            "SELECT 1; \nSELECT 2; -- tight\n"
        );
        // Elsewhere `--x` already is a comment.
        assert_eq!(prepare_for_server("SELECT 1 --x", false), "SELECT 1 --x");
        // Valid comments stay (routine bodies keep their documentation).
        let kept = "SELECT 1 -- a\n# b\n/* c */";
        assert_eq!(prepare_for_server(kept, true), kept);
        assert_eq!(
            prepare_for_server("SELECT 'zażółć' // x", true),
            "SELECT 'zażółć' "
        );
    }

    #[test]
    fn keywords_functions_strings_and_numbers_are_coloured() {
        let sql = "select count(*), 'x', 1.5e-3, @v from t";
        let kinds: Vec<(&str, TokenKind)> = tokens(sql, true)
            .into_iter()
            .map(|(range, kind)| (&sql[range], kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("select", TokenKind::Keyword),
                ("count", TokenKind::Function),
                ("'x'", TokenKind::String),
                ("1.5e-3", TokenKind::Number),
                ("from", TokenKind::Keyword),
            ]
        );
    }

    fn split(sql: &str) -> Vec<&str> {
        split_script(sql, true)
            .into_iter()
            .map(|range| &sql[range])
            .collect()
    }

    #[test]
    fn a_script_splits_on_top_level_semicolons_only() {
        assert_eq!(
            split("SELECT 1;\n SELECT ';' ; -- only a comment;\n# another\nSELECT 2"),
            [
                "SELECT 1",
                "SELECT ';'",
                "-- only a comment;\n# another\nSELECT 2"
            ]
        );
        assert!(split("-- nothing\n;").is_empty());
    }

    #[test]
    fn mysql_conditional_comments_are_statements() {
        // A mysqldump's session settings live in `/*!nnnnn ... */`, which the server executes.
        assert_eq!(
            split("/*!40101 SET NAMES utf8 */;\n-- note\nSELECT 1;"),
            ["/*!40101 SET NAMES utf8 */", "-- note\nSELECT 1"]
        );
        assert!(has_code("/*!40101 SET NAMES utf8 */", true));
        assert!(!has_code("/*!40101 SET NAMES utf8 */", false));
        assert!(!has_code("/* plain comment */", true));
    }

    #[test]
    fn a_routine_body_stays_one_statement() {
        let sql = "CREATE PROCEDURE p()\nBEGIN\n  IF 1 THEN SELECT 1; END IF;\n  \
                   CASE WHEN 1 THEN SELECT 2; END CASE;\n  SELECT CASE 1 WHEN 1 THEN 3 END;\n\
                   END;\nCALL p();";
        let statements = split(sql);
        assert_eq!(statements.len(), 2, "{statements:?}");
        assert!(statements[0].ends_with("END"));
        assert_eq!(statements[1], "CALL p()");
        // A transaction's BEGIN is not a block.
        assert_eq!(
            split("BEGIN; SELECT 1; COMMIT;"),
            ["BEGIN", "SELECT 1", "COMMIT"]
        );
    }

    #[test]
    fn the_delimiter_command_changes_the_delimiter() {
        let sql =
            "DELIMITER //\nCREATE PROCEDURE p() BEGIN SELECT 1; END //\nDELIMITER ;\nSELECT 2;";
        assert_eq!(
            split(sql),
            ["CREATE PROCEDURE p() BEGIN SELECT 1; END", "SELECT 2"]
        );
        // `//` is the delimiter there, not a comment.
        assert!(comments(sql, true).is_empty());
    }

    #[test]
    fn a_word_delimiter_only_ends_a_statement_at_a_word_boundary() {
        // `GO` inside `CATEGORY` is not the delimiter; the one at the end of the line is.
        let sql = "DELIMITER GO\nSELECT CATEGORY, 1 FROM t GO\nDELIMITER ;\nSELECT 2;";
        assert_eq!(split(sql), ["SELECT CATEGORY, 1 FROM t", "SELECT 2"]);
        // A symbolic delimiter still matches without a space before it.
        let sql = "DELIMITER $$\nSELECT 1$$\nDELIMITER ;\nSELECT 2;";
        assert_eq!(split(sql), ["SELECT 1", "SELECT 2"]);
        // The common dump idiom: `END$$` with no space.
        let sql =
            "DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END$$\nDELIMITER ;\nSELECT 2;";
        assert_eq!(
            split(sql),
            ["CREATE PROCEDURE p() BEGIN SELECT 1; END", "SELECT 2"]
        );
    }

    #[test]
    fn keywords_are_sorted_for_the_binary_search() {
        assert!(KEYWORDS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(KEYWORDS.iter().all(|keyword| keyword.len() <= 17));
    }
}
