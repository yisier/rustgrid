//! The key types Redis supports and how each one becomes a row set.
//!
//! A key is presented as a small table: its identity column (field, member, index, entry ID) is
//! marked as the primary key so the shared grid can edit and delete single rows.

use rustgrid_core::ColumnInfo;

/// One Redis value type, as reported by `TYPE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyType {
    String,
    List,
    Set,
    ZSet,
    Hash,
    Stream,
    /// A ReJSON key; `TYPE` reports `ReJSON-RL` (RedisJSON 1.x) or `json` (RedisJSON 2.x).
    Json,
    /// A module type this build does not know (`TYPE` returned something else).
    Unknown(String),
}

impl KeyType {
    /// Maps the raw `TYPE` reply.
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "string" => Self::String,
            "list" => Self::List,
            "set" => Self::Set,
            "zset" => Self::ZSet,
            "hash" => Self::Hash,
            "stream" => Self::Stream,
            "ReJSON-RL" | "json" => Self::Json,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// The label shown in the object list's type column.
    pub fn label(&self) -> &str {
        match self {
            Self::String => "string",
            Self::List => "list",
            Self::Set => "set",
            Self::ZSet => "zset",
            Self::Hash => "hash",
            Self::Stream => "stream",
            Self::Json => "json",
            Self::Unknown(raw) => raw,
        }
    }

    /// The columns of the key's row set, or `None` for a type that cannot be shown.
    pub fn columns(&self) -> Option<Vec<ColumnInfo>> {
        let column = |name: &str, data_type: &str, primary_key: bool| ColumnInfo {
            name: name.to_string(),
            data_type: data_type.to_string(),
            nullable: true,
            primary_key,
            auto_increment: false,
            generated: false,
            comment: String::new(),
        };
        let columns = match self {
            Self::String | Self::Json => vec![column("value", "text", true)],
            Self::List => vec![
                column("index", "integer", true),
                column("value", "text", false),
            ],
            Self::Set => vec![column("member", "text", true)],
            Self::ZSet => vec![
                column("member", "text", true),
                column("score", "double", false),
            ],
            Self::Hash => vec![
                column("field", "text", true),
                column("value", "text", false),
            ],
            Self::Stream => vec![
                column("id", "text", true),
                column("field", "text", false),
                column("value", "text", false),
            ],
            Self::Unknown(_) => return None,
        };
        Some(columns)
    }

    /// The command that returns the number of entries, used for the object list's row count.
    ///
    /// A string has no entries (its `STRLEN` is a byte count, shown as the value's size instead).
    pub fn length_command(&self) -> Option<&'static str> {
        match self {
            Self::List => Some("LLEN"),
            Self::Set => Some("SCARD"),
            Self::ZSet => Some("ZCARD"),
            Self::Hash => Some("HLEN"),
            Self::Stream => Some("XLEN"),
            Self::String | Self::Json | Self::Unknown(_) => None,
        }
    }

    /// Whether the key's rows can be edited in place.
    ///
    /// Streams are append-only (a stream entry's ID is assigned by the server and its fields are
    /// immutable), and module values (ReJSON) are edited through their module's own commands, so
    /// both stay read-only in the grid.
    pub fn is_editable(&self) -> bool {
        matches!(
            self,
            Self::String | Self::List | Self::Set | Self::ZSet | Self::Hash
        )
    }

    /// Whether the type is supported at all (an unknown module type is not).
    pub fn is_supported(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_documented_type() {
        for (raw, expected) in [
            ("string", KeyType::String),
            ("list", KeyType::List),
            ("set", KeyType::Set),
            ("zset", KeyType::ZSet),
            ("hash", KeyType::Hash),
            ("stream", KeyType::Stream),
            ("ReJSON-RL", KeyType::Json),
            ("json", KeyType::Json),
        ] {
            assert_eq!(KeyType::parse(raw), expected, "raw type {raw}");
        }
        assert_eq!(
            KeyType::parse("mymodule"),
            KeyType::Unknown("mymodule".to_string())
        );
    }

    #[test]
    fn identity_columns_are_primary_keys() {
        for (kind, identity) in [
            (KeyType::Hash, "field"),
            (KeyType::ZSet, "member"),
            (KeyType::Set, "member"),
            (KeyType::List, "index"),
            (KeyType::Stream, "id"),
        ] {
            let columns = kind.columns().expect("columns");
            let keys: Vec<_> = columns
                .iter()
                .filter(|column| column.primary_key)
                .map(|column| column.name.as_str())
                .collect();
            assert_eq!(keys, vec![identity], "type {}", kind.label());
        }
        assert!(KeyType::Unknown("x".into()).columns().is_none());
    }

    #[test]
    fn streams_and_json_are_read_only() {
        assert!(!KeyType::Stream.is_editable());
        assert!(!KeyType::Json.is_editable());
        assert!(KeyType::Hash.is_editable());
    }
}
