//! Redis command highlighting for the query editor.
//!
//! Redis has no SQL grammar: the editor sends one command per line, with `#` comment lines. This
//! scanner therefore colours the command words, quoted arguments, numeric arguments and `#`
//! comments without trying to parse the line as SQL. [`COMMANDS`] is also the source of the
//! editor's command completion.

use std::ops::Range;

use super::sql_lex::TokenKind;

/// Redis commands offered by completion and coloured as keywords. Common commands lead, so an
/// empty fragment (the popup opened on the first keystroke) lists the everyday ones first.
pub(crate) const COMMANDS: &[&str] = &[
    "GET",
    "SET",
    "DEL",
    "EXISTS",
    "EXPIRE",
    "TTL",
    "PERSIST",
    "TYPE",
    "KEYS",
    "SCAN",
    "HGET",
    "HSET",
    "HDEL",
    "HGETALL",
    "HMGET",
    "HMSET",
    "HLEN",
    "HKEYS",
    "HVALS",
    "HEXISTS",
    "HINCRBY",
    "HINCRBYFLOAT",
    "HSCAN",
    "HRANDFIELD",
    "LPUSH",
    "RPUSH",
    "LPOP",
    "RPOP",
    "LRANGE",
    "LLEN",
    "LINDEX",
    "LSET",
    "LREM",
    "LTRIM",
    "LINSERT",
    "LPOS",
    "LMOVE",
    "RPOPLPUSH",
    "SADD",
    "SREM",
    "SMEMBERS",
    "SISMEMBER",
    "SCARD",
    "SPOP",
    "SRANDMEMBER",
    "SMOVE",
    "SINTER",
    "SUNION",
    "SDIFF",
    "SINTERSTORE",
    "SUNIONSTORE",
    "SDIFFSTORE",
    "SSCAN",
    "ZADD",
    "ZREM",
    "ZRANGE",
    "ZREVRANGE",
    "ZSCORE",
    "ZINCRBY",
    "ZCARD",
    "ZRANK",
    "ZREVRANK",
    "ZRANGEBYSCORE",
    "ZREVRANGEBYSCORE",
    "ZRANGEBYLEX",
    "ZREVRANGEBYLEX",
    "ZCOUNT",
    "ZLEXCOUNT",
    "ZSCAN",
    "ZPOPMIN",
    "ZPOPMAX",
    "ZRANDMEMBER",
    "XADD",
    "XREAD",
    "XRANGE",
    "XREVRANGE",
    "XLEN",
    "XDEL",
    "XTRIM",
    "XACK",
    "XPENDING",
    "XCLAIM",
    "XGROUP",
    "XINFO",
    "INFO",
    "PING",
    "ECHO",
    "SELECT",
    "AUTH",
    "HELLO",
    "RESET",
    "DBSIZE",
    "FLUSHDB",
    "FLUSHALL",
    "CONFIG",
    "CLIENT",
    "CLUSTER",
    "COMMAND",
    "MONITOR",
    "SUBSCRIBE",
    "UNSUBSCRIBE",
    "PSUBSCRIBE",
    "PUNSUBSCRIBE",
    "PUBLISH",
    "PUBSUB",
    "MULTI",
    "EXEC",
    "DISCARD",
    "WATCH",
    "UNWATCH",
    "EVAL",
    "EVALSHA",
    "EVAL_RO",
    "EVALSHA_RO",
    "SCRIPT",
    "FUNCTION",
    "FCALL",
    "FCALL_RO",
    "OBJECT",
    "MEMORY",
    "RANDOMKEY",
    "RENAME",
    "RENAMENX",
    "MOVE",
    "COPY",
    "DUMP",
    "RESTORE",
    "SORT",
    "SORT_RO",
    "WAIT",
    "LASTSAVE",
    "SAVE",
    "BGSAVE",
    "BGREWRITEAOF",
    "SLOWLOG",
    "LATENCY",
    "ACL",
    "MODULE",
    "GETRANGE",
    "SETRANGE",
    "GETSET",
    "GETDEL",
    "GETEX",
    "SETNX",
    "SETEX",
    "PSETEX",
    "APPEND",
    "STRLEN",
    "INCR",
    "DECR",
    "INCRBY",
    "DECRBY",
    "INCRBYFLOAT",
    "MSET",
    "MGET",
    "MSETNX",
    "BITCOUNT",
    "BITFIELD",
    "BITOP",
    "BITPOS",
    "GETBIT",
    "SETBIT",
    "PFADD",
    "PFCOUNT",
    "PFMERGE",
    "GEOADD",
    "GEODIST",
    "GEOHASH",
    "GEOPOS",
    "GEOSEARCH",
    "GEORADIUS",
    "JSON.GET",
    "JSON.SET",
    "JSON.DEL",
    "JSON.FORGET",
    "JSON.MGET",
    "JSON.MSET",
    "JSON.TYPE",
    "JSON.NUMINCRBY",
    "JSON.NUMMULTBY",
    "JSON.STRAPPEND",
    "JSON.STRLEN",
    "JSON.ARRAPPEND",
    "JSON.ARRINDEX",
    "JSON.ARRINSERT",
    "JSON.ARRLEN",
    "JSON.ARRPOP",
    "JSON.ARRTRIM",
    "JSON.OBJKEYS",
    "JSON.OBJLEN",
    "JSON.TOGGLE",
    "JSON.CLEAR",
    "JSON.RESP",
    "JSON.DEBUG",
];

/// Whether `word` is a known Redis command (case-insensitive).
pub(crate) fn is_command(word: &str) -> bool {
    COMMANDS
        .iter()
        .any(|command| command.eq_ignore_ascii_case(word))
}

/// Colour spans for a Redis command script. Each line is independent (Redis commands cannot span
/// lines): a line whose first non-space character is `#` is a comment, the command words are
/// keywords, quoted arguments are strings, and bare numeric arguments are numbers.
pub(crate) fn tokens(text: &str) -> Vec<(Range<usize>, TokenKind)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut tokens = Vec::new();
    let mut line_start = true;
    let mut index = 0;

    while index < chars.len() {
        let (start, character) = chars[index];
        match character {
            '\n' => {
                line_start = true;
                index += 1;
            }
            ' ' | '\t' | '\r' => index += 1,
            '#' if line_start => {
                let end = text[start..]
                    .find('\n')
                    .map(|offset| start + offset)
                    .unwrap_or(text.len());
                tokens.push((start..end, TokenKind::Comment));
                while index < chars.len() && chars[index].0 < end {
                    index += 1;
                }
            }
            '\'' | '"' => {
                let quote = character;
                let mut cursor = index + 1;
                while cursor < chars.len() {
                    match chars[cursor].1 {
                        '\\' => cursor += 2,
                        character if character == quote => {
                            cursor += 1;
                            break;
                        }
                        '\n' => break,
                        _ => cursor += 1,
                    }
                }
                let end = chars
                    .get(cursor)
                    .map(|(offset, _)| *offset)
                    .unwrap_or(text.len());
                tokens.push((start..end, TokenKind::String));
                index = cursor;
                line_start = false;
            }
            character if character.is_ascii_digit() || is_signed_number(&chars, index) => {
                let mut cursor = index + 1;
                while cursor < chars.len()
                    && (chars[cursor].1.is_ascii_digit() || chars[cursor].1 == '.')
                {
                    cursor += 1;
                }
                let end = chars
                    .get(cursor)
                    .map(|(offset, _)| *offset)
                    .unwrap_or(text.len());
                tokens.push((start..end, TokenKind::Number));
                index = cursor;
                line_start = false;
            }
            _ => {
                let mut cursor = index;
                while cursor < chars.len()
                    && !chars[cursor].1.is_whitespace()
                    && !matches!(chars[cursor].1, '\'' | '"')
                {
                    cursor += 1;
                }
                let end = chars
                    .get(cursor)
                    .map(|(offset, _)| *offset)
                    .unwrap_or(text.len());
                if is_command(&text[start..end]) {
                    tokens.push((start..end, TokenKind::Keyword));
                }
                index = cursor;
                line_start = false;
            }
        }
    }
    tokens
}

/// Whether the `-`/`+` at `index` starts a signed number (`-1`, `+0.5`).
fn is_signed_number(chars: &[(usize, char)], index: usize) -> bool {
    matches!(chars[index].1, '-' | '+')
        && chars
            .get(index + 1)
            .is_some_and(|(_, character)| character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(String, TokenKind)> {
        tokens(text)
            .into_iter()
            .map(|(range, kind)| (text[range].to_string(), kind))
            .collect()
    }

    #[test]
    fn colours_commands_strings_numbers_and_comments() {
        let text = "# a comment\nEVAL \"return 1\" 1 myhash2\nHGETALL user:1";
        assert_eq!(
            kinds(text),
            vec![
                ("# a comment".to_string(), TokenKind::Comment),
                ("EVAL".to_string(), TokenKind::Keyword),
                ("\"return 1\"".to_string(), TokenKind::String),
                ("1".to_string(), TokenKind::Number),
                ("HGETALL".to_string(), TokenKind::Keyword),
            ]
        );
    }

    #[test]
    fn recognises_dotted_and_case_insensitive_commands() {
        assert!(is_command("JSON.GET"));
        assert!(is_command("json.get"));
        assert!(is_command("get"));
        assert!(!is_command("not-a-command"));
    }

    #[test]
    fn a_hash_inside_an_argument_is_not_a_comment() {
        let text = "SET user:1 a#b";
        assert_eq!(kinds(text), vec![("SET".to_string(), TokenKind::Keyword)]);
    }
}
