use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustgrid_config::{ConfigStore, McpConfig, McpConnectionPermission};
use rustgrid_core::{
    Connection, ConnectionConfig, ConnectionProfile, DriverDialect, DriverRegistry, ObjectKind,
    PageRequest, TablePage,
};
use serde_json::{Value, json};

use crate::format::format_query_result_markdown;
use crate::protocol::{CallToolResult, McpTool};
use crate::security::{
    EngineFamily, SqlNaming, SqlPolicy, check_database, check_table_reference, engine_family,
    is_table_blacklisted, validate_read_only_sql,
};

/// A tool result is chat context, not a file dump. Stop building Markdown past this size.
const MAX_TOOL_TEXT_BYTES: usize = 200 * 1024;

pub fn list_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "list_connections".to_string(),
            description: "Lists the database connections the user has authorized for AI access in RustGrid, including the engine, default database, access mode (read-only or read-write) and any business context the user provided. Use a connection's name or id in the other tools.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        McpTool {
            name: "get_schema".to_string(),
            description: "Retrieves the schema of an authorized connection: accessible databases, tables/views, column data types, nullability, primary keys and comments. Sensitive tables are omitted. Pass 'database' to limit the result to one database; omit it to inspect every database in the connection's scope.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "connection": {
                        "type": "string",
                        "description": "The connection name or id from list_connections."
                    },
                    "database": {
                        "type": "string",
                        "description": "Optional database (or schema, for schema-first engines) to inspect."
                    }
                },
                "required": ["connection"],
                "additionalProperties": false
            }),
        },
        McpTool {
            name: "describe_table".to_string(),
            description: "Retrieves a table's or view's columns and exact DDL (CREATE statement) from an authorized connection. Sensitive tables are refused. Use get_schema first when the database name is unknown.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "connection": {
                        "type": "string",
                        "description": "The connection name or id."
                    },
                    "database": {
                        "type": "string",
                        "description": "The database (or schema) containing the table."
                    },
                    "table": {
                        "type": "string",
                        "description": "The table or view name, optionally schema-qualified."
                    }
                },
                "required": ["connection", "table"],
                "additionalProperties": false
            }),
        },
        McpTool {
            name: "sample_data".to_string(),
            description: "Returns a few representative rows from a table (default 5, maximum 20). Use it to verify column formats, dates and enum values before writing a query. Sensitive tables are refused.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "connection": {
                        "type": "string",
                        "description": "The connection name or id."
                    },
                    "database": {
                        "type": "string",
                        "description": "The database (or schema) containing the table."
                    },
                    "table": {
                        "type": "string",
                        "description": "The table to sample."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Number of rows (default 5, maximum 20)."
                    }
                },
                "required": ["connection", "table"],
                "additionalProperties": false
            }),
        },
        McpTool {
            name: "execute_query".to_string(),
            description: "Executes one SQL statement against an authorized SQL connection. In read-only mode only a single SELECT, WITH … SELECT or EXPLAIN of a SELECT is accepted; writes, DDL, multiple statements, locking reads and external file/table functions are refused. Read-write mode permits arbitrary SQL and is an explicit user choice. Key-value/document connections use get_schema and sample_data in read-only mode; their execute_query is available only in read-write mode because their commands cannot be inspected for the sensitive-object list. Results are a compact Markdown table capped by the server's row limit (and a 200 KB payload limit).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "connection": {
                        "type": "string",
                        "description": "The connection name or id."
                    },
                    "database": {
                        "type": "string",
                        "description": "Optional database (or schema) to run the statement in."
                    },
                    "sql": {
                        "type": "string",
                        "description": "One SQL statement."
                    },
                    "max_rows": {
                        "type": "integer",
                        "description": "Optional row cap. It can only lower the server's configured cap; 0 means the server cap."
                    }
                },
                "required": ["connection", "sql"],
                "additionalProperties": false
            }),
        },
    ]
}

pub fn find_profile<'a>(
    profiles: &'a [ConnectionProfile],
    name_or_id: &str,
) -> Option<&'a ConnectionProfile> {
    profiles
        .iter()
        .find(|profile| profile.id == name_or_id || profile.name.eq_ignore_ascii_case(name_or_id))
}

struct Access {
    profile: ConnectionProfile,
    permission: McpConnectionPermission,
    driver: Arc<dyn rustgrid_core::Driver>,
    dialect: DriverDialect,
    family: EngineFamily,
    /// The database named by the request, or the profile's default.
    effective_database: Option<String>,
    timeout: Duration,
}

impl Access {
    fn is_read_only(&self) -> bool {
        self.permission.read_only
    }

    fn naming(&self) -> SqlNaming {
        match self.family {
            EngineFamily::Sql(naming) => naming,
            EngineFamily::NonSql => SqlNaming::Flat,
        }
    }

    fn policy(&self) -> SqlPolicy<'_> {
        SqlPolicy {
            dialect: self.dialect,
            naming: self.naming(),
            default_database: self.effective_database.as_deref(),
            allowed_databases: &self.permission.allowed_databases,
            blacklisted_tables: &self.permission.blacklisted_tables,
        }
    }

    fn is_sql(&self) -> bool {
        matches!(self.family, EngineFamily::Sql(_))
    }
}

async fn open_access(
    config: &ConfigStore,
    mcp_cfg: &McpConfig,
    registry: &Arc<DriverRegistry>,
    connection: &str,
    requested_database: Option<&str>,
) -> Result<Access, CallToolResult> {
    if !mcp_cfg.enabled {
        return Err(CallToolResult::error(
            "MCP service is disabled in RustGrid preferences. Enable Options -> AI / MCP first.",
        ));
    }

    let profiles = config.load_profiles().map_err(|error| {
        CallToolResult::error(format!("Failed to load the saved connections: {error}"))
    })?;
    let profile = find_profile(&profiles, connection)
        .cloned()
        .ok_or_else(|| CallToolResult::error(format!("Connection '{connection}' not found.")))?;
    let permission = mcp_cfg.permission(&profile.id);
    if !permission.enabled {
        return Err(CallToolResult::error(format!(
            "Connection '{}' is not authorized for AI / MCP access.",
            profile.name
        )));
    }
    let driver = registry.get(&profile.driver).ok_or_else(|| {
        CallToolResult::error(format!(
            "Driver '{}' is not registered in this RustGrid build.",
            profile.driver
        ))
    })?;
    let dialect = driver.dialect_for(&profile);
    let family = engine_family(dialect, &driver.capabilities_for(&profile));
    let effective_database = requested_database
        .map(str::to_string)
        .or_else(|| {
            // Oracle's profile database is the service name, not a schema: the tree's "databases"
            // are schemas, so the profile value must not be treated as one.
            if dialect == DriverDialect::Oracle {
                None
            } else if dialect == DriverDialect::Sqlite {
                // SQLite has one database (`main`); the profile's value is the file path.
                Some("main".to_string())
            } else {
                profile.database.clone()
            }
        })
        .map(|database| database.trim().to_string())
        .filter(|database| !database.is_empty());

    Ok(Access {
        profile,
        permission,
        driver,
        dialect,
        family,
        effective_database,
        timeout: mcp_cfg.timeout(),
    })
}

async fn connect(access: &Access, config: &ConfigStore) -> Result<Box<dyn Connection>, String> {
    let secrets = config.load_secrets().unwrap_or_default();
    let password = secrets.get(&access.profile.id).cloned();
    let mut settings = access.profile.settings.clone();
    if access.permission.read_only {
        settings.read_only = true;
    }
    let conn_config = ConnectionConfig {
        driver: access.profile.driver.clone(),
        host: access.profile.host.clone(),
        port: access.profile.port,
        username: access.profile.username.clone(),
        password,
        // The profile's database is the connection's default. A tool's `database` argument is
        // passed to the individual operation instead, which is what lets a schema-first engine
        // (Oracle) inspect one schema without trying to connect to it as a service.
        database: access.profile.database.clone(),
        options: access.profile.options.clone(),
        settings,
    };
    run_with_timeout(access.timeout, access.driver.connect(&conn_config))
        .await
        .map_err(|error| format!("Failed to connect to '{}': {error}", access.profile.name))
}

async fn run_with_timeout<T>(
    timeout: Duration,
    future: impl Future<Output = rustgrid_core::Result<T>>,
) -> Result<T, String> {
    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!(
            "the database did not answer within {} seconds",
            timeout.as_secs()
        )),
    }
}

fn tool_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(message)
}

fn table_or_bare_allowed(
    access: &Access,
    table: &str,
    database: Option<&str>,
) -> Result<(), String> {
    if access.is_sql() {
        check_table_reference(table, database, &access.policy())
    } else if is_table_blacklisted(table, &access.permission.blacklisted_tables) {
        Err(format!(
            "Permission Denied: '{table}' is on the sensitive-table blacklist."
        ))
    } else {
        Ok(())
    }
}

fn database_allowed(access: &Access, database: Option<&str>) -> Result<(), String> {
    check_database(database, &access.permission.allowed_databases)
}

pub fn handle_list_connections(config: &ConfigStore, mcp_cfg: &McpConfig) -> CallToolResult {
    let profiles = match config.load_profiles() {
        Ok(profiles) => profiles,
        Err(error) => return tool_error(format!("Failed to load profiles: {error}")),
    };
    if !mcp_cfg.enabled {
        return tool_error(
            "MCP service is disabled in RustGrid preferences. Enable Options -> AI / MCP first.",
        );
    }

    let mut out = String::from("# Authorized Database Connections\n\n");
    let mut count = 0;
    for profile in &profiles {
        let permission = mcp_cfg.permission(&profile.id);
        if !permission.enabled {
            continue;
        }
        count += 1;
        out.push_str(&format!(
            "- **{}** (ID: `{}` | Engine: `{}`)\n",
            profile.name, profile.id, profile.driver
        ));
        out.push_str(&format!(
            "  - Access Mode: {}\n",
            if permission.read_only {
                "Read-Only (guardrail; the database account is the real boundary)"
            } else {
                "Read-Write (the user explicitly allowed writes)"
            }
        ));
        if let Some(database) = &profile.database {
            out.push_str(&format!("  - Default Database: `{database}`\n"));
        }
        if !permission.allowed_databases.is_empty() {
            out.push_str(&format!(
                "  - Allowed Databases: {}\n",
                permission.allowed_databases.join(", ")
            ));
        }
        if !permission.blacklisted_tables.is_empty() {
            // The names themselves are deliberately not disclosed: the blacklist hides sensitive
            // objects, so listing their names would defeat it.
            out.push_str("  - Sensitive tables are hidden from discovery and queries.\n");
        }
        if !permission.context_prompt.trim().is_empty() {
            out.push_str(&format!(
                "  - Business Instructions: {}\n",
                permission.context_prompt.trim()
            ));
        }
        out.push('\n');
    }

    if count == 0 {
        return CallToolResult::text(
            "No database connections are currently authorized for AI access. To grant access, open RustGrid Options -> AI / MCP and enable the target connection.",
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
    let Some(connection) = args.get("connection").and_then(Value::as_str) else {
        return tool_error("Missing required parameter: 'connection'");
    };
    let target_database = args.get("database").and_then(Value::as_str);
    let access = match open_access(config, mcp_cfg, registry, connection, target_database).await {
        Ok(access) => access,
        Err(result) => return result,
    };
    if let Some(database) = target_database
        && let Err(error) = database_allowed(&access, Some(database))
    {
        return tool_error(error);
    }

    let conn = match connect(&access, config).await {
        Ok(conn) => conn,
        Err(message) => return tool_error(message),
    };

    let databases = if let Some(target) = target_database {
        vec![target.to_string()]
    } else {
        let listed = run_with_timeout(access.timeout, conn.list_databases()).await;
        let databases = match listed {
            Ok(databases) => databases
                .into_iter()
                .map(|database| database.name)
                .collect::<Vec<_>>(),
            Err(message) => {
                return tool_error(format!(
                    "Failed to list databases for '{}': {message}",
                    access.profile.name
                ));
            }
        };
        databases
            .into_iter()
            .filter(|database| database_allowed(&access, Some(database)).is_ok())
            .collect()
    };

    if databases.is_empty() {
        return tool_error(format!(
            "No accessible databases found for connection '{}'.",
            access.profile.name
        ));
    }

    let mut out = format!("# Schema Definition: {}\n\n", access.profile.name);
    let mut truncated = false;
    'databases: for database in &databases {
        if out.len() >= MAX_TOOL_TEXT_BYTES {
            truncated = true;
            break;
        }
        out.push_str(&format!("## Database: `{database}`\n\n"));
        let tables = match run_with_timeout(access.timeout, conn.list_tables(database)).await {
            Ok(tables) => tables,
            Err(message) => {
                out.push_str(&format!("*Error loading tables: {message}*\n\n"));
                continue;
            }
        };
        if tables.is_empty() {
            out.push_str("*No tables or views found.*\n\n");
            continue;
        }

        // The listed database, not the profile default, is the schema an unqualified table name
        // belongs to (the two differ when the profile has no database or the request names one).
        let policy = SqlPolicy {
            dialect: access.dialect,
            naming: access.naming(),
            default_database: Some(database.as_str()),
            allowed_databases: &access.permission.allowed_databases,
            blacklisted_tables: &access.permission.blacklisted_tables,
        };
        for table in tables {
            if out.len() >= MAX_TOOL_TEXT_BYTES {
                truncated = true;
                break 'databases;
            }
            if access.is_sql() {
                if check_table_reference(&table.name, Some(database), &policy).is_err() {
                    continue;
                }
            } else if is_table_blacklisted(&table.name, &access.permission.blacklisted_tables) {
                continue;
            }

            let kind = if table.kind == ObjectKind::View {
                "View"
            } else {
                "Table"
            };
            out.push_str(&format!("### {kind} `{}`\n", table.name));
            let columns = run_with_timeout(access.timeout, conn.columns(database, &table.name))
                .await
                .unwrap_or_default();
            if columns.is_empty() {
                out.push_str("*Column information unavailable.*\n\n");
                continue;
            }
            for column in columns {
                if out.len() >= MAX_TOOL_TEXT_BYTES {
                    truncated = true;
                    break 'databases;
                }
                let mut traits = vec![column.data_type.to_uppercase()];
                if column.primary_key {
                    traits.push("PRIMARY KEY".to_string());
                }
                if !column.nullable {
                    traits.push("NOT NULL".to_string());
                }
                if column.auto_increment {
                    traits.push("AUTO INCREMENT".to_string());
                }
                let comment = if column.comment.is_empty() {
                    String::new()
                } else {
                    format!(" - {}", column.comment)
                };
                out.push_str(&format!(
                    "- `{}` ({}){}\n",
                    column.name,
                    traits.join(", "),
                    comment
                ));
            }
            out.push('\n');
        }
    }
    if truncated {
        out.push_str(
            "\n*Output truncated at the tool's size limit. Narrow the request with a `database` argument.*\n",
        );
    }
    CallToolResult::text(out)
}

pub async fn handle_describe_table(
    config: &ConfigStore,
    mcp_cfg: &McpConfig,
    registry: &Arc<DriverRegistry>,
    args: &Value,
) -> CallToolResult {
    let Some(connection) = args.get("connection").and_then(Value::as_str) else {
        return tool_error("Missing required parameter: 'connection'");
    };
    let Some(table) = args.get("table").and_then(Value::as_str) else {
        return tool_error("Missing required parameter: 'table'");
    };
    let target_database = args.get("database").and_then(Value::as_str);
    let access = match open_access(config, mcp_cfg, registry, connection, target_database).await {
        Ok(access) => access,
        Err(result) => return result,
    };
    let database = access.effective_database.clone().unwrap_or_default();
    if let Err(message) = database_allowed(&access, Some(&database)) {
        return tool_error(message);
    }
    if let Err(message) = table_or_bare_allowed(&access, table, Some(&database)) {
        return tool_error(message);
    }

    let conn = match connect(&access, config).await {
        Ok(conn) => conn,
        Err(message) => return tool_error(message),
    };
    let columns = match run_with_timeout(access.timeout, conn.columns(&database, table)).await {
        Ok(columns) => columns,
        Err(message) => return tool_error(format!("Failed to read table columns: {message}")),
    };
    if columns.is_empty() {
        return tool_error(format!(
            "Table '{table}' does not exist in database '{database}'"
        ));
    }
    let ddl = match run_with_timeout(access.timeout, conn.object_ddl(&database, table, false)).await
    {
        Ok(Some(sql)) => sql,
        Ok(None) => "-- DDL is not available for this engine/table".to_string(),
        Err(message) => format!("-- Failed to generate exact DDL: {message}"),
    };

    let mut out = format!("# Table Definition: `{table}`\n\n```sql\n");
    if ddl.len() > MAX_TOOL_TEXT_BYTES {
        let cut = ddl
            .char_indices()
            .take_while(|(index, _)| *index < MAX_TOOL_TEXT_BYTES)
            .last()
            .map_or(0, |(index, _)| index);
        out.push_str(&ddl[..cut]);
        out.push_str("\n-- DDL truncated at the tool's size limit\n");
    } else {
        out.push_str(&ddl);
    }
    out.push_str("\n```\n\n### Column Details\n\n");
    for column in columns {
        let mut flags = vec![column.data_type.to_uppercase()];
        if column.primary_key {
            flags.push("PRIMARY KEY".to_string());
        }
        if !column.nullable {
            flags.push("NOT NULL".to_string());
        }
        if column.auto_increment {
            flags.push("AUTO INCREMENT".to_string());
        }
        if column.generated {
            flags.push("GENERATED".to_string());
        }
        let comment = if column.comment.is_empty() {
            String::new()
        } else {
            format!(" - {}", column.comment)
        };
        out.push_str(&format!(
            "- `{}` ({}){}\n",
            column.name,
            flags.join(", "),
            comment
        ));
    }
    CallToolResult::text(out)
}

pub async fn handle_sample_data(
    config: &ConfigStore,
    mcp_cfg: &McpConfig,
    registry: &Arc<DriverRegistry>,
    args: &Value,
) -> CallToolResult {
    let Some(connection) = args.get("connection").and_then(Value::as_str) else {
        return tool_error("Missing required parameter: 'connection'");
    };
    let Some(table) = args.get("table").and_then(Value::as_str) else {
        return tool_error("Missing required parameter: 'table'");
    };
    let target_database = args.get("database").and_then(Value::as_str);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| (value as usize).clamp(1, 20))
        .unwrap_or(5);

    let access = match open_access(config, mcp_cfg, registry, connection, target_database).await {
        Ok(access) => access,
        Err(result) => return result,
    };
    let database = access.effective_database.clone().unwrap_or_default();
    if let Err(message) = database_allowed(&access, Some(&database)) {
        return tool_error(message);
    }
    if let Err(message) = table_or_bare_allowed(&access, table, Some(&database)) {
        return tool_error(message);
    }

    let conn = match connect(&access, config).await {
        Ok(conn) => conn,
        Err(message) => return tool_error(message),
    };
    let mut page = PageRequest::new(0, limit as u64);
    page.skip_total = true;
    let started = Instant::now();
    match run_with_timeout(access.timeout, conn.fetch_page(&database, table, page)).await {
        Ok(TablePage { columns, rows, .. }) => {
            let result = rustgrid_core::QueryResult {
                statement: format!("SELECT * FROM {table}"),
                columns,
                rows,
                rows_affected: 0,
                has_result_set: true,
                last_insert_id: None,
            };
            CallToolResult::text(format_query_result_markdown(
                &access.profile.name,
                &database,
                &result,
                limit,
                started.elapsed(),
            ))
        }
        Err(message) => tool_error(format!("Failed to sample data from '{table}': {message}")),
    }
}

pub async fn handle_execute_query(
    config: &ConfigStore,
    mcp_cfg: &McpConfig,
    registry: &Arc<DriverRegistry>,
    args: &Value,
) -> CallToolResult {
    let Some(connection) = args.get("connection").and_then(Value::as_str) else {
        return tool_error("Missing required parameter: 'connection'");
    };
    let Some(sql) = args.get("sql").and_then(Value::as_str) else {
        return tool_error("Missing required parameter: 'sql'");
    };
    let sql = sql.trim();
    if sql.is_empty() {
        return tool_error("SQL statement is empty");
    }
    let target_database = args.get("database").and_then(Value::as_str);
    let access = match open_access(config, mcp_cfg, registry, connection, target_database).await {
        Ok(access) => access,
        Err(result) => return result,
    };

    if access.is_read_only() {
        if let EngineFamily::Sql(_) = access.family {
            // The parser checks the effective/default database for unqualified relations and each
            // qualified name, so a restricted scope does not need a separate default check here.
            if let Err(message) = validate_read_only_sql(sql, &access.policy()) {
                return tool_error(message);
            }
        } else {
            // The driver enforces the read-only flag on Redis/Mongo commands, but a command's
            // target key/collection cannot be checked against the sensitive-object list reliably.
            // Keep the read-only surface to the metadata tools and require an explicit read-write
            // grant for arbitrary commands.
            return tool_error(
                "Read-Only query execution is not available for key-value/document connections. Use get_schema and sample_data, or enable Read-Write explicitly.",
            );
        }
    } else {
        // Read-write is an explicit user decision. Scope still applies to the selected database,
        // but a qualified name in arbitrary SQL (or a DDL statement) cannot be fully inspected;
        // require an explicit in-scope database so the target is at least known.
        if access.effective_database.is_none() && !access.permission.allowed_databases.is_empty() {
            return tool_error(
                "Permission Denied: pass the database name when this connection has a restricted scope.",
            );
        }
        if let Err(message) = database_allowed(&access, access.effective_database.as_deref()) {
            return tool_error(message);
        }
    }

    let row_cap = mcp_cfg.row_cap();
    let requested = args
        .get("max_rows")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    let max_rows = match (row_cap, requested) {
        (0, None) => 0,
        (0, Some(value)) => value,
        (cap, None) => cap,
        (cap, Some(0)) => cap,
        (cap, Some(value)) => value.min(cap),
    };

    let conn = match connect(&access, config).await {
        Ok(conn) => conn,
        Err(message) => return tool_error(message),
    };
    let started = Instant::now();
    let query_database = access.effective_database.as_deref();
    match run_with_timeout(access.timeout, conn.execute_query(query_database, sql)).await {
        Ok(result) => CallToolResult::text(format_query_result_markdown(
            &access.profile.name,
            query_database.unwrap_or(""),
            &result,
            max_rows,
            started.elapsed(),
        )),
        Err(message) => tool_error(format!("Query execution error: {message}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_complete() {
        let tools = list_tools();
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        for expected in [
            "list_connections",
            "get_schema",
            "describe_table",
            "sample_data",
            "execute_query",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
        for tool in tools {
            assert!(!tool.description.is_empty());
            assert!(tool.input_schema.is_object());
        }
    }

    #[test]
    fn find_profile_matches_id_name_and_case() {
        let profile = ConnectionProfile {
            id: "id_1".to_string(),
            name: "MySQL Prod".to_string(),
            driver: rustgrid_core::DriverId::new("mysql"),
            host: "localhost".to_string(),
            port: 3306,
            username: "root".to_string(),
            database: Some("shop".to_string()),
            options: Default::default(),
            settings: Default::default(),
        };
        let profiles = vec![profile];
        assert_eq!(find_profile(&profiles, "id_1").unwrap().name, "MySQL Prod");
        assert_eq!(find_profile(&profiles, "mysql prod").unwrap().id, "id_1");
        assert!(find_profile(&profiles, "missing").is_none());
    }
}
