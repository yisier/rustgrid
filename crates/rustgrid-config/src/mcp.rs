//! Settings for the built-in Model Context Protocol (MCP) server.
//!
//! RustGrid exposes its saved connections to AI clients through a small stdio MCP server
//! (`RustGrid --mcp-stdio`). This file owns the one document those settings live in:
//! `mcp.json` under the configuration directory. It is deliberately separate from
//! `connections.json`: AI permission is machine-local and must not travel with an exported
//! `.rgc` connection bundle.
//!
//! Permissions are keyed by the connection profile id. A connection with no entry — or an entry
//! whose `enabled` is false — is invisible to every MCP tool.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The current `mcp.json` schema version.
pub const MCP_CONFIG_VERSION: u32 = 1;

/// The file name under the configuration directory.
pub const MCP_CONFIG_FILE: &str = "mcp.json";

/// The default number of rows a tool returns. `0` means no row cap.
pub const DEFAULT_MCP_MAX_ROWS: u32 = 50;

/// The default per-request timeout for a database operation.
pub const DEFAULT_MCP_TIMEOUT_SECONDS: u32 = 30;

fn default_max_rows() -> u32 {
    DEFAULT_MCP_MAX_ROWS
}

fn default_timeout_seconds() -> u32 {
    DEFAULT_MCP_TIMEOUT_SECONDS
}

fn default_true() -> bool {
    true
}

/// What one saved connection allows an MCP client to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConnectionPermission {
    /// Whether the connection is exposed at all.
    #[serde(default)]
    pub enabled: bool,
    /// Read-only mode. This is a guardrail, not a database security boundary: the account's own
    /// grants remain the authority. The MCP server additionally refuses writes for SQL engines.
    #[serde(default = "default_true")]
    pub read_only: bool,
    /// Databases the client may access. Empty means every database the account can see.
    #[serde(default)]
    pub allowed_databases: Vec<String>,
    /// Tables hidden from discovery and refused in queries. A bare name matches the table in any
    /// schema; a qualified `schema.table` entry matches that qualified name.
    #[serde(default)]
    pub blacklisted_tables: Vec<String>,
    /// Optional business context included in `list_connections` and handed to the model.
    #[serde(default)]
    pub context_prompt: String,
}

impl Default for McpConnectionPermission {
    fn default() -> Self {
        Self {
            enabled: false,
            read_only: true,
            allowed_databases: Vec::new(),
            blacklisted_tables: Vec::new(),
            context_prompt: String::new(),
        }
    }
}

impl McpConnectionPermission {
    /// The permission to use for a profile id: its saved entry, or the safe default.
    pub fn for_profile<'a>(connections: &'a BTreeMap<String, Self>, profile_id: &str) -> &'a Self {
        connections.get(profile_id).unwrap_or(&EMPTY_PERMISSION)
    }
}

static EMPTY_PERMISSION: McpConnectionPermission = McpConnectionPermission {
    enabled: false,
    read_only: true,
    allowed_databases: Vec::new(),
    blacklisted_tables: Vec::new(),
    context_prompt: String::new(),
};

/// The complete MCP settings document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    /// Schema version; written by [`ConfigStore::save_mcp`](crate::ConfigStore::save_mcp).
    #[serde(default)]
    pub version: u32,
    /// The global switch. A disabled server still speaks the protocol but refuses every tool with a
    /// clear "MCP is disabled" error, so a client can stay connected while the user turns it off.
    #[serde(default)]
    pub enabled: bool,
    /// The default row cap. `0` means no cap; a tool may lower it but never raise it.
    #[serde(default = "default_max_rows")]
    pub max_rows: u32,
    /// The maximum time one database operation may take.
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u32,
    /// Per-connection permissions, keyed by [`ConnectionProfile::id`](rustgrid_core::ConnectionProfile).
    #[serde(default)]
    pub connections: BTreeMap<String, McpConnectionPermission>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            version: MCP_CONFIG_VERSION,
            enabled: false,
            max_rows: DEFAULT_MCP_MAX_ROWS,
            timeout_seconds: DEFAULT_MCP_TIMEOUT_SECONDS,
            connections: BTreeMap::new(),
        }
    }
}

impl McpConfig {
    /// The effective permission for a profile id.
    pub fn permission(&self, profile_id: &str) -> McpConnectionPermission {
        self.connections
            .get(profile_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Whether the global switch and the connection's own entry both allow access.
    pub fn is_enabled_for(&self, profile_id: &str) -> bool {
        self.enabled && self.permission(profile_id).enabled
    }

    /// A per-request timeout, clamped to a sane range so a bad file cannot hang or spin.
    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_seconds.clamp(1, 600) as u64)
    }

    /// The configured row cap. A tool may ask for fewer rows but not more; `0` means unlimited.
    pub fn row_cap(&self) -> usize {
        self.max_rows as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_and_read_only() {
        let cfg = McpConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.permission("missing").enabled);
        assert!(cfg.permission("missing").read_only);
        assert!(!cfg.is_enabled_for("missing"));
    }

    #[test]
    fn permission_round_trip() {
        let mut cfg = McpConfig {
            enabled: true,
            max_rows: 10,
            timeout_seconds: 5,
            ..Default::default()
        };
        cfg.connections.insert(
            "conn".to_string(),
            McpConnectionPermission {
                enabled: true,
                read_only: false,
                allowed_databases: vec!["dev".to_string()],
                blacklisted_tables: vec!["secrets".to_string()],
                context_prompt: "shop data".to_string(),
            },
        );
        let text = serde_json::to_string(&cfg).unwrap();
        let back: McpConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(back, cfg);
        assert!(back.is_enabled_for("conn"));
        assert!(!back.is_enabled_for("other"));
        assert_eq!(back.timeout().as_secs(), 5);
    }

    #[test]
    fn missing_fields_use_safe_defaults() {
        let cfg: McpConfig = serde_json::from_str("{}").unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.version, 0);
        assert_eq!(cfg.max_rows, DEFAULT_MCP_MAX_ROWS);
        assert_eq!(cfg.timeout_seconds, DEFAULT_MCP_TIMEOUT_SECONDS);
    }

    #[test]
    fn store_round_trips_and_migrates_an_unversioned_file() {
        let root = std::env::temp_dir().join(format!(
            "rustgrid_mcp_config_{}_{}",
            std::process::id(),
            "roundtrip"
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = crate::ConfigStore::with_root(&root);

        let missing = store.load_mcp().expect("missing file");
        assert!(!missing.enabled);
        assert!(
            !store.mcp_path().exists(),
            "loading must not create the file"
        );

        let config = McpConfig {
            enabled: true,
            max_rows: 7,
            ..Default::default()
        };
        store.save_mcp(&config).expect("save");
        let loaded = store.load_mcp().expect("load");
        assert!(loaded.enabled);
        assert_eq!(loaded.max_rows, 7);
        assert_eq!(loaded.version, MCP_CONFIG_VERSION);

        // A pre-version file (including the old HTTP-shaped one) keeps its known fields and is
        // rewritten with the current version; unknown fields are ignored.
        std::fs::write(
            store.mcp_path(),
            r#"{"enabled":true,"port":8080,"api_token":"old","connections":{}}"#,
        )
        .expect("write old file");
        let migrated = store.load_mcp().expect("migrate");
        assert!(migrated.enabled);
        assert_eq!(migrated.version, MCP_CONFIG_VERSION);
        let rewritten = std::fs::read_to_string(store.mcp_path()).expect("rewritten");
        assert!(rewritten.contains("\"version\": 1"));
        assert!(!rewritten.contains("api_token"));

        let _ = std::fs::remove_dir_all(root);
    }
}
