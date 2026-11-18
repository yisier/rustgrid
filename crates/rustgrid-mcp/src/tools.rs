use std::sync::Arc;
use std::time::Instant;

use rustgrid_config::ConfigStore;
use rustgrid_core::{
    ConnectionConfig, ConnectionProfile, DriverRegistry, ObjectKind, PageRequest, TablePage,
};
use serde_json::{Value, json};

use crate::config::McpConfig;
use crate::format::format_query_result_markdown;
use crate::protocol::{CallToolResult, McpTool};
use crate::security::{
    MCP_OPT_AI_CONTEXT_PROMPT, MCP_OPT_DATABASE_SCOPE, MCP_OPT_ENABLED, MCP_OPT_READ_ONLY,
    McpConnectionSettings, is_database_allowed, is_table_blacklisted, sanitize_identifier,
    validate_read_only_sql,
};

pub fn list_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "list_connections".to_string(),
            description: "Lists all database connections currently enabled for AI access in RustGrid, including their database engine, default database, access permissions (read-only vs read-write), and human-authored business context instructions.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        McpTool {
            name: "get_schema".to_string(),
            description: "Retrieves complete schema information for an authorized database connection (databases, tables, views, column data types, nullability, primary/foreign keys, and column comments). Sensitive tables on the blacklist are automatically filtered out. Specify 'database' to scope inspection to a single database, or omit it to inspect all accessible databases.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "connection": {
                        "type": "string",
                        "description": "The connection name or connection ID from list_connections."
                    },
                    "database": {
                        "type": "string",
                        "description": "Optional database name to scope schema inspection."
                    }
                },
                "required": ["connection"],
                "additionalProperties": false
            }),
        },
        McpTool {
            name: "describe_table".to_string(),
            description: "Retrieves the full structural definition and DDL (CREATE TABLE) for a specific table in an authorized database connection. Useful when writing complex SQL queries requiring exact foreign keys, check constraints, or indexes.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "connection": {
                        "type": "string",
                        "description": "The connection name or connection ID."
                    },
                    "database": {
                        "type": "string",
                        "description": "The database or schema containing the table."
                    },
                    "table": {
                        "type": "string",
                        "description": "The exact name of the table to inspect."
                    }
                },
                "required": ["connection", "table"],
                "additionalProperties": false
            }),
        },
        McpTool {
            name: "execute_query".to_string(),
            description: "Executes a SQL query against an authorized database connection. By default, only read-only statements (SELECT, SHOW, EXPLAIN, DESCRIBE) are permitted. Results are formatted as a compact Markdown table. Pass max_rows: 0 for unlimited rows, or specify a desired limit.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "connection": {
                        "type": "string",
                        "description": "The connection name or connection ID."
                    },
                    "database": {
                        "type": "string",
                        "description": "Optional database name to execute the query against."
                    },
                    "sql": {
                        "type": "string",
                        "description": "The SQL statement to execute."
                    },
                    "max_rows": {
                        "type": "integer",
                        "description": "Optional row count cap. Pass 0 for unlimited rows without truncation. If omitted, uses system setting."
                    }
                },
                "required": ["connection", "sql"],
                "additionalProperties": false
            }),
        },
        McpTool {
            name: "sample_data".to_string(),
            description: "Retrieves a small representative sample of records (default: 3 to 5 rows) from a specified table. Helps the AI confirm column formats, datetime styles, and sample values before drafting queries.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "connection": {
                        "type": "string",
                        "description": "The connection name or connection ID."
                    },
                    "database": {
                        "type": "string",
                        "description": "The database containing the table."
                    },
                    "table": {
                        "type": "string",
                        "description": "The table to sample records from."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of sample rows to retrieve (default: 5, maximum: 20)."
                    }
                },
                "required": ["connection", "table"],
                "additionalProperties": false
            }),
        },
    ]
}

pub fn resolve_connection_settings(
    profile: &ConnectionProfile,
    mcp_cfg: &McpConfig,
) -> McpConnectionSettings {
    // 1. If options on the profile has explicit mcp settings, prefer profile options
    let profile_settings = McpConnectionSettings::from_options(&profile.options);
    if profile.options.contains_key(MCP_OPT_ENABLED)
        || profile.options.contains_key(MCP_OPT_READ_ONLY)
        || profile.options.contains_key(MCP_OPT_DATABASE_SCOPE)
    {
        return profile_settings;
    }
    // 2. Otherwise fall back to mcp.json per-connection overrides if present
    if let Some(perm) = mcp_cfg.connections.get(&profile.id) {
        return McpConnectionSettings {
            enabled: perm.enabled,
            read_only: perm.read_only,
            allowed_databases: perm.allowed_databases.clone(),
            table_blacklist: Vec::new(),
            ai_context_prompt: profile
                .options
                .get(MCP_OPT_AI_CONTEXT_PROMPT)
                .cloned()
                .unwrap_or_default(),
        };
    }
    profile_settings
}

pub fn find_profile<'a>(
    profiles: &'a [ConnectionProfile],
    name_or_id: &str,
) -> Option<&'a ConnectionProfile> {
    profiles
        .iter()
        .find(|p| p.id == name_or_id || p.name.eq_ignore_ascii_case(name_or_id))
}

pub fn handle_list_connections(config: &ConfigStore, mcp_cfg: &McpConfig) -> CallToolResult {
    let profiles = match config.load_profiles() {
        Ok(p) => p,
        Err(e) => return CallToolResult::error(format!("Failed to load profiles: {e}")),
    };

    if !mcp_cfg.enabled {
        return CallToolResult::error(
            "MCP service is globally disabled in RustGrid preferences.".to_string(),
        );
    }

    let mut out = String::from("# Authorized Database Connections\n\n");
    let mut count = 0;

    for profile in &profiles {
        let ai_settings = resolve_connection_settings(profile, &mcp_cfg);
        if !ai_settings.enabled {
            continue;
        }

        count += 1;
        out.push_str(&format!(
            "- **{}** (ID: `{}` | Engine: `{}`)\n",
            profile.name, profile.id, profile.driver
        ));
        out.push_str(&format!(
            "  - Access Mode: {}\n",
            if ai_settings.read_only {
                "Strict Read-Only"
            } else {
                "Read-Write"
            }
        ));
        if let Some(db) = &profile.database {
            out.push_str(&format!("  - Default Database: `{}`\n", db));
        }
        if !ai_settings.allowed_databases.is_empty() {
            out.push_str(&format!(
                "  - Allowed Databases: {}\n",
                ai_settings.allowed_databases.join(", ")
            ));
        }
        if !ai_settings.table_blacklist.is_empty() {
            out.push_str(&format!(
                "  - Hidden/Blacklisted Tables: {}\n",
                ai_settings.table_blacklist.join(", ")
            ));
        }
        if !ai_settings.ai_context_prompt.trim().is_empty() {
            out.push_str(&format!(
                "  - Business Instructions: {}\n",
                ai_settings.ai_context_prompt.trim()
            ));
        }
        out.push('\n');
    }

    if count == 0 {
        return CallToolResult::text(
            "No database connections are currently authorized for AI access.\nTo grant access, open RustGrid Options -> AI / MCP and enable the target connection.",
        );
    }

    CallToolResult::text(out)
}

pub async fn handle_get_schema(
    config: &ConfigStore,
    mcp_cfg: &McpConfig,
    registry: &Arc<DriverRegistry>,
    args: &Value,
) -> CallToolResult {
    let conn_name = match args.get("connection").and_then(Value::as_str) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: 'connection'"),
    };
    let target_db = args.get("database").and_then(Value::as_str);

    let profiles = match config.load_profiles() {
        Ok(p) => p,
        Err(e) => return CallToolResult::error(format!("Failed to load profiles: {e}")),
    };

    let profile = match find_profile(&profiles, conn_name) {
        Some(p) => p,
        None => return CallToolResult::error(format!("Connection '{conn_name}' not found.")),
    };

    let ai_settings = resolve_connection_settings(profile, mcp_cfg);
    if !ai_settings.enabled {
        return CallToolResult::error(format!(
            "Connection '{}' is not authorized for AI / MCP access.",
            profile.name
        ));
    }

    let driver = match registry.get(&profile.driver) {
        Some(d) => d,
        None => {
            return CallToolResult::error(format!(
                "Driver '{}' not supported or registered.",
                profile.driver
            ));
        }
    };

    let secrets = config.load_secrets().unwrap_or_default();
    let password = secrets.get(&profile.id).cloned();

    let mut conn_settings = profile.settings.clone();
    if ai_settings.read_only {
        conn_settings.read_only = true;
    }

    let conn_config = ConnectionConfig {
        driver: profile.driver.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        password,
        database: target_db
            .map(str::to_string)
            .or_else(|| profile.database.clone()),
        options: profile.options.clone(),
        settings: conn_settings,
    };

    let conn = match driver.connect(&conn_config).await {
        Ok(c) => c,
        Err(e) => {
            return CallToolResult::error(format!("Failed to connect to '{}': {e}", profile.name));
        }
    };

    let databases: Vec<String> = match conn.list_databases().await {
        Ok(dbs) => dbs.into_iter().map(|d| d.name).collect(),
        Err(_) => vec![
            profile
                .database
                .clone()
                .unwrap_or_else(|| "default".to_string()),
        ],
    };

    let filtered_dbs: Vec<String> = databases
        .into_iter()
        .filter(|db| {
            if let Some(target) = target_db {
                db.eq_ignore_ascii_case(target)
            } else {
                is_database_allowed(db, &ai_settings.allowed_databases)
            }
        })
        .collect();

    if filtered_dbs.is_empty() {
        return CallToolResult::error(format!(
            "No accessible databases found for connection '{}'.",
            profile.name
        ));
    }

    let mut out = format!("# Schema Definition: {}\n\n", profile.name);
    for db in &filtered_dbs {
        out.push_str(&format!("## Database: `{}`\n\n", db));

        let tables = match conn.list_tables(db).await {
            Ok(tbls) => tbls,
            Err(e) => {
                out.push_str(&format!("*Error loading tables: {e}*\n\n"));
                continue;
            }
        };

        if tables.is_empty() {
            out.push_str("*No tables or views found.*\n\n");
            continue;
        }

        for table in tables {
            if is_table_blacklisted(&table.name, &ai_settings.table_blacklist) {
                continue;
            }

            let kind = if table.kind == ObjectKind::View {
                "View"
            } else {
                "Table"
            };
            out.push_str(&format!("### {} `{}`\n", kind, table.name));

            let columns = match conn.columns(db, &table.name).await {
                Ok(cols) => cols,
                Err(_) => Vec::new(),
            };

            if columns.is_empty() {
                out.push_str("*Columns information unavailable.*\n\n");
                continue;
            }

            for col in columns {
                let mut traits = vec![col.data_type.to_uppercase()];
                if col.primary_key {
                    traits.push("PRIMARY KEY".to_string());
                }
                if !col.nullable {
                    traits.push("NOT NULL".to_string());
                }
                if col.auto_increment {
                    traits.push("AUTO INCREMENT".to_string());
                }
                let comment_desc = if !col.comment.is_empty() {
                    format!(" - {}", col.comment)
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "- `{}` ({}){}\n",
                    col.name,
                    traits.join(", "),
                    comment_desc
                ));
            }
            out.push('\n');
        }
    }

    CallToolResult::text(out)
}

pub async fn handle_describe_table(
    config: &ConfigStore,
    mcp_cfg: &McpConfig,
    registry: &Arc<DriverRegistry>,
    args: &Value,
) -> CallToolResult {
    let conn_name = match args.get("connection").and_then(Value::as_str) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: 'connection'"),
    };
    let table = match args.get("table").and_then(Value::as_str) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: 'table'"),
    };
    let target_db = args.get("database").and_then(Value::as_str);

    let profiles = match config.load_profiles() {
        Ok(p) => p,
        Err(e) => return CallToolResult::error(format!("Failed to load profiles: {e}")),
    };

    let profile = match find_profile(&profiles, conn_name) {
        Some(p) => p,
        None => return CallToolResult::error(format!("Connection '{conn_name}' not found.")),
    };

    let ai_settings = resolve_connection_settings(profile, mcp_cfg);
    if !ai_settings.enabled {
        return CallToolResult::error(format!(
            "Connection '{}' is not authorized for AI access.",
            profile.name
        ));
    }

    if is_table_blacklisted(table, &ai_settings.table_blacklist) {
        return CallToolResult::error(format!(
            "Access Denied: Table '{table}' is protected by sensitive blacklist."
        ));
    }

    let driver = match registry.get(&profile.driver) {
        Some(d) => d,
        None => {
            return CallToolResult::error(format!("Driver '{}' not supported.", profile.driver));
        }
    };

    let secrets = config.load_secrets().unwrap_or_default();
    let password = secrets.get(&profile.id).cloned();

    let mut conn_settings = profile.settings.clone();
    if ai_settings.read_only {
        conn_settings.read_only = true;
    }

    let conn_config = ConnectionConfig {
        driver: profile.driver.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        password,
        database: target_db
            .map(str::to_string)
            .or_else(|| profile.database.clone()),
        options: profile.options.clone(),
        settings: conn_settings,
    };

    let conn = match driver.connect(&conn_config).await {
        Ok(c) => c,
        Err(e) => return CallToolResult::error(format!("Connection error: {e}")),
    };

    let db = target_db.unwrap_or_else(|| profile.database.as_deref().unwrap_or(""));
    let columns = match conn.columns(db, table).await {
        Ok(cols) => cols,
        Err(e) => return CallToolResult::error(format!("Failed to read table columns: {e}")),
    };
    if columns.is_empty() {
        return CallToolResult::error(format!("Table '{table}' does not exist in database '{db}'"));
    }

    let ddl = match conn.object_ddl(db, table, false).await {
        Ok(Some(sql)) => sql,
        Ok(None) => "-- DDL not available for this engine/table".to_string(),
        Err(e) => format!("-- Failed to generate exact DDL: {e}"),
    };

    let mut out = format!("# Table Definition: `{}`\n\n", table);
    out.push_str("```sql\n");
    out.push_str(&ddl);
    out.push_str("\n```\n\n### Column Details\n\n");

    for col in columns {
        let mut flags = vec![col.data_type.to_uppercase()];
        if col.primary_key {
            flags.push("PRIMARY KEY".to_string());
        }
        if !col.nullable {
            flags.push("NOT NULL".to_string());
        }
        if col.auto_increment {
            flags.push("AUTO INCREMENT".to_string());
        }
        let comment_str = if !col.comment.is_empty() {
            format!(" - {}", col.comment)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "- `{}` ({}){}\n",
            col.name,
            flags.join(", "),
            comment_str
        ));
    }

    CallToolResult::text(out)
}

pub async fn handle_execute_query(
    config: &ConfigStore,
    mcp_cfg: &McpConfig,
    registry: &Arc<DriverRegistry>,
    args: &Value,
) -> CallToolResult {
    let conn_name = match args.get("connection").and_then(Value::as_str) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: 'connection'"),
    };
    let sql = match args.get("sql").and_then(Value::as_str) {
        Some(s) => s.trim(),
        None => return CallToolResult::error("Missing required parameter: 'sql'"),
    };
    let target_db = args.get("database").and_then(Value::as_str);

    let profiles = match config.load_profiles() {
        Ok(p) => p,
        Err(e) => return CallToolResult::error(format!("Failed to load profiles: {e}")),
    };

    let profile = match find_profile(&profiles, conn_name) {
        Some(p) => p,
        None => return CallToolResult::error(format!("Connection '{conn_name}' not found.")),
    };

    let ai_settings = resolve_connection_settings(profile, mcp_cfg);
    if !ai_settings.enabled {
        return CallToolResult::error(format!(
            "Connection '{}' is not authorized for AI access.",
            profile.name
        ));
    }

    if let Some(db) = target_db {
        if !is_database_allowed(db, &ai_settings.allowed_databases) {
            return CallToolResult::error(format!(
                "Permission Denied: Access to database '{db}' is not permitted."
            ));
        }
    }

    // Safety: enforce read-only SQL validation
    if ai_settings.read_only {
        if let Err(reason) = validate_read_only_sql(sql) {
            return CallToolResult::error(reason);
        }
    }

    // Safety: check blacklist mentions in SQL
    let lower_sql = sql.to_lowercase();
    let sql_tokens: Vec<&str> = lower_sql
        .split(|c: char| {
            c.is_whitespace()
                || c == ';'
                || c == ','
                || c == '('
                || c == ')'
                || c == '`'
                || c == '"'
                || c == '\''
                || c == '['
                || c == ']'
        })
        .filter(|t| !t.is_empty())
        .collect();

    for blacklisted in &ai_settings.table_blacklist {
        let clean_b = sanitize_identifier(blacklisted);
        if sql_tokens
            .iter()
            .any(|&token| sanitize_identifier(token) == clean_b)
        {
            return CallToolResult::error(format!(
                "Permission Denied: Query accesses blacklisted sensitive table '{}'.",
                blacklisted
            ));
        }
    }

    let driver = match registry.get(&profile.driver) {
        Some(d) => d,
        None => {
            return CallToolResult::error(format!("Driver '{}' not supported.", profile.driver));
        }
    };

    let max_rows = args
        .get("max_rows")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(mcp_cfg.max_rows as usize);

    let secrets = config.load_secrets().unwrap_or_default();
    let password = secrets.get(&profile.id).cloned();

    let mut conn_settings = profile.settings.clone();
    if ai_settings.read_only {
        conn_settings.read_only = true;
    }

    let conn_config = ConnectionConfig {
        driver: profile.driver.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        password,
        database: target_db
            .map(str::to_string)
            .or_else(|| profile.database.clone()),
        options: profile.options.clone(),
        settings: conn_settings,
    };

    let conn = match driver.connect(&conn_config).await {
        Ok(c) => c,
        Err(e) => return CallToolResult::error(format!("Connection error: {e}")),
    };

    let start = Instant::now();
    let query_db = target_db.or_else(|| profile.database.as_deref());

    // P0-1 & P0-1': Enforce database session-level transaction read-only mode
    if ai_settings.read_only {
        let read_only_init = match profile.driver.as_str() {
            "postgresql" | "postgres" => Some(
                "SET default_transaction_read_only = on; SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY;",
            ),
            "mysql" | "mariadb" => Some("SET SESSION TRANSACTION READ ONLY;"),
            "sqlite" => Some("PRAGMA query_only = ON;"),
            _ => None,
        };
        if let Some(init_sql) = read_only_init {
            let _ = conn.execute_query(query_db, init_sql).await;
        }
    }

    match conn.execute_query(query_db, sql).await {
        Ok(res) => {
            let elapsed = start.elapsed();
            let md = format_query_result_markdown(
                &profile.name,
                query_db.unwrap_or(""),
                &res,
                max_rows,
                elapsed,
            );
            CallToolResult::text(md)
        }
        Err(e) => CallToolResult::error(format!("Query execution error: {e}")),
    }
}

pub async fn handle_sample_data(
    config: &ConfigStore,
    mcp_cfg: &McpConfig,
    registry: &Arc<DriverRegistry>,
    args: &Value,
) -> CallToolResult {
    let conn_name = match args.get("connection").and_then(Value::as_str) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: 'connection'"),
    };
    let table = match args.get("table").and_then(Value::as_str) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: 'table'"),
    };
    let target_db = args.get("database").and_then(Value::as_str);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| (v as usize).clamp(1, 20))
        .unwrap_or(5);

    let profiles = match config.load_profiles() {
        Ok(p) => p,
        Err(e) => return CallToolResult::error(format!("Failed to load profiles: {e}")),
    };

    let profile = match find_profile(&profiles, conn_name) {
        Some(p) => p,
        None => return CallToolResult::error(format!("Connection '{conn_name}' not found.")),
    };

    let ai_settings = resolve_connection_settings(profile, mcp_cfg);
    if !ai_settings.enabled {
        return CallToolResult::error(format!(
            "Connection '{}' is not authorized for AI access.",
            profile.name
        ));
    }

    if is_table_blacklisted(table, &ai_settings.table_blacklist) {
        return CallToolResult::error(format!(
            "Access Denied: Table '{table}' is on the sensitive table blacklist."
        ));
    }

    let driver = match registry.get(&profile.driver) {
        Some(d) => d,
        None => {
            return CallToolResult::error(format!("Driver '{}' not supported.", profile.driver));
        }
    };

    let secrets = config.load_secrets().unwrap_or_default();
    let password = secrets.get(&profile.id).cloned();

    let mut conn_settings = profile.settings.clone();
    if ai_settings.read_only {
        conn_settings.read_only = true;
    }

    let conn_config = ConnectionConfig {
        driver: profile.driver.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        password,
        database: target_db
            .map(str::to_string)
            .or_else(|| profile.database.clone()),
        options: profile.options.clone(),
        settings: conn_settings,
    };

    let conn = match driver.connect(&conn_config).await {
        Ok(c) => c,
        Err(e) => return CallToolResult::error(format!("Connection error: {e}")),
    };

    let db = target_db.unwrap_or_else(|| profile.database.as_deref().unwrap_or(""));
    let mut page_req = PageRequest::new(0, limit as u64);
    page_req.skip_total = true;

    let start = Instant::now();
    match conn.fetch_page(db, table, page_req).await {
        Ok(TablePage { columns, rows, .. }) => {
            let elapsed = start.elapsed();
            let query_result = rustgrid_core::QueryResult {
                statement: format!("SELECT * FROM {table}"),
                columns,
                rows,
                rows_affected: 0,
                has_result_set: true,
                last_insert_id: None,
            };
            let md = format_query_result_markdown(&profile.name, db, &query_result, limit, elapsed);
            CallToolResult::text(md)
        }
        Err(e) => CallToolResult::error(format!("Failed to sample data from '{table}': {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_tools_completeness() {
        let tools = list_tools();
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"list_connections"));
        assert!(names.contains(&"get_schema"));
        assert!(names.contains(&"describe_table"));
        assert!(names.contains(&"execute_query"));
        assert!(names.contains(&"sample_data"));

        for tool in tools {
            assert!(!tool.description.is_empty());
            assert!(tool.input_schema.is_object());
        }
    }

    #[test]
    fn test_find_profile() {
        let profiles = vec![
            ConnectionProfile {
                id: "id_1".to_string(),
                name: "MySQL Prod".to_string(),
                driver: "mysql".into(),
                host: "localhost".to_string(),
                port: 3306,
                username: "root".to_string(),
                database: Some("shop".to_string()),
                options: std::collections::BTreeMap::new(),
                settings: Default::default(),
            },
            ConnectionProfile {
                id: "id_2".to_string(),
                name: "Postgres Dev".to_string(),
                driver: "postgresql".into(),
                host: "localhost".to_string(),
                port: 5432,
                username: "pg".to_string(),
                database: None,
                options: std::collections::BTreeMap::new(),
                settings: Default::default(),
            },
        ];

        assert_eq!(find_profile(&profiles, "id_1").unwrap().name, "MySQL Prod");
        assert_eq!(find_profile(&profiles, "mysql prod").unwrap().id, "id_1");
        assert_eq!(find_profile(&profiles, "POSTGRES DEV").unwrap().id, "id_2");
        assert!(find_profile(&profiles, "non_existent").is_none());
    }
}
