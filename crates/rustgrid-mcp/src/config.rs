use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chacha20poly1305::Key;
use chacha20poly1305::aead::Generate;
use serde::{Deserialize, Serialize};

pub const MCP_CONFIG_FILE: &str = "mcp.json";
pub const DEFAULT_MAX_ROWS: u32 = 50;
pub const DEFAULT_TIMEOUT_SECONDS: u32 = 30;

fn default_max_rows() -> u32 {
    DEFAULT_MAX_ROWS
}

fn default_timeout_seconds() -> u32 {
    DEFAULT_TIMEOUT_SECONDS
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionMcpPermission {
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub read_only: bool,
    #[serde(default)]
    pub allowed_databases: Vec<String>,
    #[serde(default)]
    pub allowed_tables: Vec<String>,
}

impl Default for ConnectionMcpPermission {
    fn default() -> Self {
        Self {
            enabled: false,
            read_only: true,
            allowed_databases: Vec::new(),
            allowed_tables: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub http_enabled: bool,
    #[serde(default)]
    pub allow_lan: bool,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub api_token: String,
    #[serde(default = "default_max_rows")]
    pub max_rows: u32,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u32,
    #[serde(default)]
    pub connections: BTreeMap<String, ConnectionMcpPermission>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            http_enabled: false,
            allow_lan: false,
            port: 0,
            api_token: String::new(),
            max_rows: DEFAULT_MAX_ROWS,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            connections: BTreeMap::new(),
        }
    }
}

pub fn generate_secure_token() -> String {
    let key = Key::generate();
    format!("rg_mcp_{}", hex::encode(&key[..16]))
}

pub fn mcp_config_path(root: &Path) -> PathBuf {
    root.join(MCP_CONFIG_FILE)
}

pub fn load_mcp_config(root: &Path) -> McpConfig {
    let path = mcp_config_path(root);
    if !path.exists() {
        let mut cfg = McpConfig::default();
        cfg.api_token = generate_secure_token();
        let _ = save_mcp_config(root, &cfg);
        return cfg;
    }

    match fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<McpConfig>(&contents) {
            Ok(mut cfg) => {
                let mut dirty = false;
                if cfg.api_token.trim().is_empty() {
                    cfg.api_token = generate_secure_token();
                    dirty = true;
                }
                if dirty {
                    let _ = save_mcp_config(root, &cfg);
                }
                cfg
            }
            Err(e) => {
                tracing::warn!("Failed to parse {}: {}, using defaults", path.display(), e);
                let mut cfg = McpConfig::default();
                cfg.api_token = generate_secure_token();
                cfg
            }
        },
        Err(e) => {
            tracing::warn!("Failed to read {}: {}, using defaults", path.display(), e);
            let mut cfg = McpConfig::default();
            cfg.api_token = generate_secure_token();
            cfg
        }
    }
}

pub fn save_mcp_config(root: &Path, config: &McpConfig) -> std::io::Result<()> {
    if !root.exists() {
        fs::create_dir_all(root)?;
    }
    let path = mcp_config_path(root);
    let contents = serde_json::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&path, contents)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpDatabaseScopeState {
    pub conn_id: String,
    pub conn_name: String,
    pub driver: String,
    pub is_all: bool,
    pub selected_dbs: std::collections::BTreeSet<String>,
    pub manual_input: String,
    pub available_dbs: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpOptionsState {
    pub global_enabled: bool,
    pub allow_lan: bool,
    pub port: String,
    pub default_max_rows: String,
    pub copied_toast: Option<String>,
    pub editing_scope: Option<McpDatabaseScopeState>,
}

impl McpOptionsState {
    pub fn load(config_dir: &Path, active_port: u16) -> Self {
        let mcp_cfg = load_mcp_config(config_dir);
        let port_to_show = if mcp_cfg.port > 0 {
            mcp_cfg.port
        } else if active_port > 0 {
            active_port
        } else {
            0
        };
        Self {
            global_enabled: mcp_cfg.enabled,
            allow_lan: mcp_cfg.allow_lan,
            port: port_to_show.to_string(),
            default_max_rows: mcp_cfg.max_rows.to_string(),
            copied_toast: None,
            editing_scope: None,
        }
    }

    /// Applies the staged options back to config file. Returns true if the server should be restarted.
    pub fn apply(&self, config_dir: &Path) -> bool {
        let mut mcp_cfg = load_mcp_config(config_dir);
        let old_enabled = mcp_cfg.enabled;
        let old_lan = mcp_cfg.allow_lan;
        let old_port = mcp_cfg.port;

        mcp_cfg.enabled = self.global_enabled;
        mcp_cfg.http_enabled = self.global_enabled;
        mcp_cfg.allow_lan = self.allow_lan;
        if let Ok(port) = self.port.trim().parse::<u16>() {
            mcp_cfg.port = port;
        }
        if let Ok(limit) = self.default_max_rows.trim().parse::<u32>() {
            mcp_cfg.max_rows = limit;
        }
        let _ = save_mcp_config(config_dir, &mcp_cfg);

        old_enabled != mcp_cfg.enabled || old_lan != mcp_cfg.allow_lan || old_port != mcp_cfg.port
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_roundtrip() {
        let temp_dir =
            std::env::temp_dir().join(format!("rustgrid_mcp_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        let _ = fs::create_dir_all(&temp_dir);

        let mut cfg = load_mcp_config(&temp_dir);
        assert!(!cfg.api_token.is_empty());
        assert!(!cfg.enabled);

        cfg.enabled = true;
        cfg.port = 60087;
        cfg.connections.insert(
            "test_conn".to_string(),
            ConnectionMcpPermission {
                enabled: true,
                read_only: true,
                allowed_databases: vec!["db1".to_string()],
                allowed_tables: vec!["tbl1".to_string()],
            },
        );

        save_mcp_config(&temp_dir, &cfg).unwrap();
        let loaded = load_mcp_config(&temp_dir);
        assert_eq!(loaded.port, 60087);
        assert!(loaded.enabled);
        assert_eq!(loaded.connections.len(), 1);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
