//! A generic SQL script splitter shared by the core `Connection` default and any driver that has
//! no engine-specific scanner of its own.
//!
//! It walks the script byte by byte and cuts on top-level semicolons, skipping over:
//!
//! - single-quoted string literals (`'...'`, with `''` and backslash escapes),
//! - double-quoted strings/quoted identifiers (`"..."`, with `""`),
//! - MySQL-style backtick identifiers (`` `...` ``, with ` `` `),
//! - SQL Server bracket identifiers (`[...]`, with `]]`),
//! - PostgreSQL dollar-quoted strings (`$tag$ ... $tag$`),
//! - line comments (`-- ...`, `# ...`) and block comments (`/* ... */`). A MySQL conditional
//!   comment (`/*!40014 SET ... */`) is *not* a comment to the server — it executes what it
//!   wraps — so a fragment holding one is kept as a statement.
//!
//! It deliberately does **not** track `BEGIN … END` block depth: dialects with block bodies
//! (Oracle PL/SQL, SQL Server T-SQL, PostgreSQL functions) and engines whose own scanner is tuned
//! to their syntax (MySQL, SQLite) override `Connection::split_statements` instead. Keeping the
//! default depth-free means a `BEGIN;` transaction is never mistaken for the start of a block.

/// Split `sql` into its individual statements, each trimmed and free of empty/comment-only
/// fragments.
pub fn split_statements(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut statements: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut balanced = true;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i = skip_single_quote(bytes, i, &mut balanced);
            }
            b'"' => {
                i = skip_doubled_quote(bytes, i, b'"');
            }
            b'`' => {
                i = skip_doubled_quote(bytes, i, b'`');
            }
            b'[' => {
                i = skip_bracket(bytes, i);
            }
            b'$' => {
                if let Some(end) = skip_dollar_quote(bytes, i) {
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i = skip_line_comment(bytes, i + 2);
            }
            b'#' => {
                i = skip_line_comment(bytes, i + 1);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = skip_block_comment(bytes, i, &mut balanced);
            }
            b';' => {
                push_fragment(&mut statements, &sql[start..i]);
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }

    if !balanced {
        // An unterminated literal/comment: hand the whole remainder back untouched rather than
        // guessing at a split inside it.
        return vec![sql.trim().to_string()];
    }
    push_fragment(&mut statements, &sql[start..]);
    statements
}

/// Skip a `'...'` literal from the opening quote, honouring `''` and backslash escapes. Returns
/// the index just past the literal.
fn skip_single_quote(bytes: &[u8], start: usize, balanced: &mut bool) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'\'' => {
                if bytes.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                return i + 1;
            }
            _ => i += 1,
        }
    }
    *balanced = false;
    bytes.len()
}

/// Skip a quoted run opened by `quote` (a `"` or a backtick), honouring a doubled quote as an
/// escaped one. Returns the index just past the closing quote.
fn skip_doubled_quote(bytes: &[u8], start: usize, quote: u8) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == quote {
            if bytes.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

/// Skip a `[...]` bracket identifier from the opening bracket, honouring `]]`. Returns the index
/// just past the closing bracket.
fn skip_bracket(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b']' {
            if bytes.get(i + 1) == Some(&b']') {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

/// If position `start` opens a `$tag$ ... $tag$` dollar-quoted string, return the index just past
/// its closing tag. `None` when it is a bare `$` (e.g. a PostgreSQL placeholder or Oracle bind).
fn skip_dollar_quote(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if bytes.get(i) != Some(&b'$') {
        return None;
    }
    let tag = &bytes[start..=i];
    let mut j = i + 1;
    while j + tag.len() <= bytes.len() {
        if &bytes[j..j + tag.len()] == tag {
            return Some(j + tag.len());
        }
        j += 1;
    }
    Some(bytes.len())
}

/// Skip a line comment body from just after its `--`/`#`, returning the index of the newline.
fn skip_line_comment(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i] != b'\n' {
        i += 1;
    }
    i
}

/// Skip a `/* ... */` block comment from its opening slash. Returns the index just past `*/`.
fn skip_block_comment(bytes: &[u8], start: usize, balanced: &mut bool) -> usize {
    let mut i = start + 2;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            return i + 2;
        }
        i += 1;
    }
    *balanced = false;
    bytes.len()
}

/// Push `fragment` when it holds anything besides whitespace and comments.
fn push_fragment(statements: &mut Vec<String>, fragment: &str) {
    let trimmed = fragment.trim();
    if has_sql_content(trimmed) {
        statements.push(trimmed.to_string());
    }
}

/// Whether `fragment` contains anything besides whitespace and comments.
fn has_sql_content(fragment: &str) -> bool {
    let bytes = fragment.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            byte if byte.is_ascii_whitespace() => i += 1,
            b'-' if bytes.get(i + 1) == Some(&b'-') => i = skip_line_comment(bytes, i + 2),
            b'#' => i = skip_line_comment(bytes, i + 1),
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                // MySQL executes the SQL inside a conditional comment, so a fragment that holds
                // one is a statement rather than a comment-only fragment.
                if bytes.get(i + 2) == Some(&b'!') {
                    return true;
                }
                let mut balanced = true;
                i = skip_block_comment(bytes, i, &mut balanced);
            }
            _ => return true,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::split_statements;

    #[test]
    fn splits_plain_statements() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2;"),
            vec!["SELECT 1", "SELECT 2"]
        );
        assert_eq!(split_statements("  SELECT 1 ; \n"), vec!["SELECT 1"]);
    }

    #[test]
    fn ignores_terminators_in_literals_and_comments() {
        assert_eq!(
            split_statements("SELECT ';' AS a; SELECT 2"),
            vec!["SELECT ';' AS a", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT 1 -- a; b\n; SELECT 2"),
            vec!["SELECT 1 -- a; b", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT /* a;b */ 1; SELECT 2"),
            vec!["SELECT /* a;b */ 1", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT \"a;b\"; SELECT 2"),
            vec!["SELECT \"a;b\"", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT `a;b`; SELECT 2"),
            vec!["SELECT `a;b`", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT [a;b]; SELECT 2"),
            vec!["SELECT [a;b]", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT $tag$a;b$tag$; SELECT 2"),
            vec!["SELECT $tag$a;b$tag$", "SELECT 2"]
        );
    }

    #[test]
    fn drops_empty_and_comment_only_fragments() {
        assert!(split_statements("-- just a comment\n").is_empty());
        assert!(split_statements("").is_empty());
        assert!(split_statements("/* only */").is_empty());
    }

    #[test]
    fn keeps_mysql_conditional_comments() {
        // A mysqldump wraps its session settings in `/*!nnnnn ... */`, which the server executes.
        assert_eq!(
            split_statements(
                "/*!40014 SET @OLD_UNIQUE_CHECKS=@@UNIQUE_CHECKS, UNIQUE_CHECKS=0 */;\n\
                 /*!40000 ALTER TABLE `t` DISABLE KEYS */;\nSELECT 1;"
            ),
            vec![
                "/*!40014 SET @OLD_UNIQUE_CHECKS=@@UNIQUE_CHECKS, UNIQUE_CHECKS=0 */",
                "/*!40000 ALTER TABLE `t` DISABLE KEYS */",
                "SELECT 1",
            ]
        );
    }
}
