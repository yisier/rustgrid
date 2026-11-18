use std::collections::BTreeMap;

pub const MCP_OPT_ENABLED: &str = "mcp.enabled";
pub const MCP_OPT_READ_ONLY: &str = "mcp.read_only";
pub const MCP_OPT_DATABASE_SCOPE: &str = "mcp.database_scope";
pub const MCP_OPT_TABLE_BLACKLIST: &str = "mcp.table_blacklist";
pub const MCP_OPT_AI_CONTEXT_PROMPT: &str = "mcp.ai_context_prompt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConnectionSettings {
    pub enabled: bool,
    pub read_only: bool,
    pub allowed_databases: Vec<String>,
    pub table_blacklist: Vec<String>,
    pub ai_context_prompt: String,
}

impl Default for McpConnectionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            read_only: true,
            allowed_databases: Vec::new(),
            table_blacklist: Vec::new(),
            ai_context_prompt: String::new(),
        }
    }
}

impl McpConnectionSettings {
    pub fn from_options(options: &BTreeMap<String, String>) -> Self {
        let enabled = options
            .get(MCP_OPT_ENABLED)
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        let read_only = options
            .get(MCP_OPT_READ_ONLY)
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true);

        let allowed_databases = options
            .get(MCP_OPT_DATABASE_SCOPE)
            .map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let table_blacklist = options
            .get(MCP_OPT_TABLE_BLACKLIST)
            .map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(sanitize_identifier)
                    .collect()
            })
            .unwrap_or_default();

        let ai_context_prompt = options
            .get(MCP_OPT_AI_CONTEXT_PROMPT)
            .cloned()
            .unwrap_or_default();

        Self {
            enabled,
            read_only,
            allowed_databases,
            table_blacklist,
            ai_context_prompt,
        }
    }

    pub fn write_to_options(&self, options: &mut BTreeMap<String, String>) {
        if self.enabled {
            options.insert(MCP_OPT_ENABLED.to_string(), "true".to_string());
        } else {
            options.remove(MCP_OPT_ENABLED);
        }

        if !self.read_only {
            options.insert(MCP_OPT_READ_ONLY.to_string(), "false".to_string());
        } else {
            options.remove(MCP_OPT_READ_ONLY);
        }

        if !self.allowed_databases.is_empty() {
            options.insert(
                MCP_OPT_DATABASE_SCOPE.to_string(),
                self.allowed_databases.join(", "),
            );
        } else {
            options.remove(MCP_OPT_DATABASE_SCOPE);
        }

        if !self.table_blacklist.is_empty() {
            options.insert(
                MCP_OPT_TABLE_BLACKLIST.to_string(),
                self.table_blacklist.join(", "),
            );
        } else {
            options.remove(MCP_OPT_TABLE_BLACKLIST);
        }

        if !self.ai_context_prompt.trim().is_empty() {
            options.insert(
                MCP_OPT_AI_CONTEXT_PROMPT.to_string(),
                self.ai_context_prompt.clone(),
            );
        } else {
            options.remove(MCP_OPT_AI_CONTEXT_PROMPT);
        }
    }
}

pub fn sanitize_identifier(ident: &str) -> String {
    ident
        .trim()
        .trim_matches(|c| c == '`' || c == '"' || c == '[' || c == ']')
        .to_lowercase()
}

pub fn is_table_blacklisted(table: &str, blacklist: &[String]) -> bool {
    let clean = sanitize_identifier(table);
    blacklist.iter().any(|b| sanitize_identifier(b) == clean)
}

pub fn is_database_allowed(database: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let db_lower = database.trim().to_lowercase();
    allowed.iter().any(|d| d.trim().to_lowercase() == db_lower)
}

/// Strip leading SQL comments (-- ..., /* ... */, # ...) and whitespace.
pub fn strip_leading_comments(mut sql: &str) -> &str {
    loop {
        sql = sql.trim_start();
        if sql.starts_with("--") {
            if let Some(pos) = sql.find('\n') {
                sql = &sql[pos + 1..];
                continue;
            } else {
                return "";
            }
        } else if sql.starts_with('#') {
            if let Some(pos) = sql.find('\n') {
                sql = &sql[pos + 1..];
                continue;
            } else {
                return "";
            }
        } else if sql.starts_with("/*") {
            if let Some(pos) = sql.find("*/") {
                sql = &sql[pos + 2..];
                continue;
            } else {
                return "";
            }
        }
        break;
    }
    sql
}

const MUTATING_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "TRUNCATE", "CREATE", "RENAME", "REPLACE",
    "MERGE", "UPSERT", "GRANT", "REVOKE", "LOCK", "UNLOCK", "FLUSH", "KILL", "SHUTDOWN",
    "OPTIMIZE", "REPAIR", "VACUUM", "CALL", "EXEC", "EXECUTE", "DO", "COPY", "LOAD", "SET", "INTO",
];

/// Ambiguous SQL keywords that are commonly used as column or table aliases (e.g. `SELECT 1 AS set`).
/// These are permitted only when immediately preceded by the `AS` keyword.
/// Destructive keywords like `DELETE`, `UPDATE`, `INSERT`, `DROP`, `INTO` can NEVER be aliases.
const AMBIGUOUS_ALIAS_KEYWORDS: &[&str] = &[
    "SET", "CALL", "DO", "EXEC", "EXECUTE", "LOAD", "COPY", "LOCK", "UNLOCK",
];

const DANGEROUS_FUNCTIONS: &[&str] = &[
    "PG_READ_FILE",
    "PG_READ_BINARY_FILE",
    "PG_WRITE_FILE",
    "PG_READ_FILE_OLD",
    "PG_STAT_FILE",
    "PG_LS_DIR",
    "PG_EXECUTE_SERVER_PROGRAM",
    "PG_TERMINATE_BACKEND",
    "PG_CANCEL_BACKEND",
    "PG_RELOAD_CONF",
    "PG_ROTATE_LOGFILE",
    "PG_SWITCH_WAL",
    "PG_CREATE_RESTORE_POINT",
    "PG_PROMOTE",
    "PG_LOGICAL_EMIT_MESSAGE",
    "PG_IMPORT_SYSTEM_COLLATIONS",
    "SETVAL",
    "SET_CONFIG",
    "LO_CREATE",
    "LO_PUT",
    "LOWRITE",
    "LO_UNLINK",
    "LO_TRUNCATE",
    "LO_IMPORT",
    "LO_EXPORT",
    "LOAD_FILE",
    "INTO_OUTFILE",
    "INTO_DUMPFILE",
];

const ALLOWED_START_KEYWORDS: &[&str] = &[
    "SELECT", "SHOW", "EXPLAIN", "DESCRIBE", "DESC", "WITH", "PRAGMA", "INFO", "PING",
];

/// Lexically tokenizes SQL by stripping comments and string literals, splitting on delimiters.
pub fn tokenize_sql(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        // Single-line comments (-- or #)
        if c == '-' && i + 1 < chars.len() && chars[i + 1] == '-' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // Multi-line comments (/* ... */)
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // String literals ('...', "...", `...`)
        if c == '\'' || c == '"' || c == '`' {
            let quote = c;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 2;
                } else if chars[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }

        if c.is_alphanumeric() || c == '_' {
            current.push(c);
        } else {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            if c == ';' {
                tokens.push(";".to_string());
            }
        }
        i += 1;
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Validates that a SQL statement is strictly read-only and free from mutating keywords and dangerous functions.
pub fn validate_read_only_sql(sql: &str) -> Result<(), String> {
    let tokens = tokenize_sql(sql);
    if tokens.is_empty() {
        return Err("SQL statement is empty".to_string());
    }

    // Split tokens into individual statements by semicolon
    let mut statements: Vec<Vec<String>> = Vec::new();
    let mut current_stmt = Vec::new();
    for token in tokens {
        if token == ";" {
            if !current_stmt.is_empty() {
                statements.push(std::mem::take(&mut current_stmt));
            }
        } else {
            current_stmt.push(token);
        }
    }
    if !current_stmt.is_empty() {
        statements.push(current_stmt);
    }

    for statement in statements {
        let first_word = match statement.first() {
            Some(w) => w.to_uppercase(),
            None => continue,
        };

        if !ALLOWED_START_KEYWORDS.contains(&first_word.as_str()) {
            return Err(format!(
                "Permission Denied: Statement starting with '{}' is not permitted in Read-Only mode.",
                first_word
            ));
        }

        let mut prev_is_as = false;
        for token in statement {
            let upper = token.to_uppercase();

            // P2-D: If the token is immediately preceded by "AS" and is an ambiguous keyword (like set/call/do),
            // it is treated as a column or table alias (e.g. SELECT 1 AS set).
            // Destructive keywords (DELETE, UPDATE, INSERT, DROP, INTO, etc.) can NEVER be bypassed.
            let is_permitted_alias =
                prev_is_as && AMBIGUOUS_ALIAS_KEYWORDS.contains(&upper.as_str());

            if !is_permitted_alias {
                if MUTATING_KEYWORDS.contains(&upper.as_str()) {
                    return Err(format!(
                        "Permission Denied: Mutating keyword '{}' is prohibited in Read-Only mode.",
                        upper
                    ));
                }
            }

            if DANGEROUS_FUNCTIONS.contains(&upper.as_str()) {
                return Err(format!(
                    "Permission Denied: Dangerous system function '{}' is prohibited.",
                    upper
                ));
            }

            prev_is_as = upper == "AS";
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_connection_settings_serde() {
        let mut opts = BTreeMap::new();
        opts.insert(MCP_OPT_ENABLED.to_string(), "true".to_string());
        opts.insert(MCP_OPT_READ_ONLY.to_string(), "false".to_string());
        opts.insert(
            MCP_OPT_DATABASE_SCOPE.to_string(),
            "db1, db2, db3".to_string(),
        );
        opts.insert(
            MCP_OPT_TABLE_BLACKLIST.to_string(),
            "`passwords`, [salaries], tokens".to_string(),
        );
        opts.insert(
            MCP_OPT_AI_CONTEXT_PROMPT.to_string(),
            "Ecommerce testing database".to_string(),
        );

        let settings = McpConnectionSettings::from_options(&opts);
        assert!(settings.enabled);
        assert!(!settings.read_only);
        assert_eq!(settings.allowed_databases, vec!["db1", "db2", "db3"]);
        assert_eq!(
            settings.table_blacklist,
            vec!["passwords", "salaries", "tokens"]
        );
        assert_eq!(settings.ai_context_prompt, "Ecommerce testing database");

        let mut roundtrip_opts = BTreeMap::new();
        settings.write_to_options(&mut roundtrip_opts);
        assert_eq!(roundtrip_opts.get(MCP_OPT_ENABLED).unwrap(), "true");
        assert_eq!(roundtrip_opts.get(MCP_OPT_READ_ONLY).unwrap(), "false");
        assert_eq!(
            roundtrip_opts.get(MCP_OPT_DATABASE_SCOPE).unwrap(),
            "db1, db2, db3"
        );
        assert_eq!(
            roundtrip_opts.get(MCP_OPT_TABLE_BLACKLIST).unwrap(),
            "passwords, salaries, tokens"
        );

        let empty_settings = McpConnectionSettings::from_options(&BTreeMap::new());
        assert!(!empty_settings.enabled);
        assert!(empty_settings.read_only);
        assert!(empty_settings.allowed_databases.is_empty());
        assert!(empty_settings.table_blacklist.is_empty());
        assert!(empty_settings.ai_context_prompt.is_empty());

        let mut cleared_opts = roundtrip_opts.clone();
        empty_settings.write_to_options(&mut cleared_opts);
        assert!(!cleared_opts.contains_key(MCP_OPT_ENABLED));
        assert!(!cleared_opts.contains_key(MCP_OPT_READ_ONLY));
        assert!(!cleared_opts.contains_key(MCP_OPT_DATABASE_SCOPE));
        assert!(!cleared_opts.contains_key(MCP_OPT_TABLE_BLACKLIST));
        assert!(!cleared_opts.contains_key(MCP_OPT_AI_CONTEXT_PROMPT));
    }

    #[test]
    fn test_sanitize_and_blacklist() {
        let blacklist = vec!["user_secrets".to_string(), "credit_cards".to_string()];
        assert!(is_table_blacklisted("user_secrets", &blacklist));
        assert!(is_table_blacklisted("`user_secrets`", &blacklist));
        assert!(is_table_blacklisted("\"USER_SECRETS\"", &blacklist));
        assert!(is_table_blacklisted("[Credit_Cards]", &blacklist));
        assert!(!is_table_blacklisted("orders", &blacklist));
        assert!(!is_table_blacklisted("public.orders", &blacklist));
    }

    #[test]
    fn test_database_allowed() {
        let allowed = vec!["dev_shop".to_string(), "analytics".to_string()];
        assert!(is_database_allowed("dev_shop", &allowed));
        assert!(is_database_allowed("DEV_SHOP", &allowed));
        assert!(is_database_allowed("analytics", &allowed));
        assert!(!is_database_allowed("prod_secrets", &allowed));

        assert!(is_database_allowed("any_db", &[]));
    }

    #[test]
    fn test_strip_comments() {
        let sql = "-- single line comment\nSELECT * FROM t;";
        assert_eq!(strip_leading_comments(sql).trim(), "SELECT * FROM t;");

        let sql2 = "/* multi\nline\ncomment */ SELECT 1;";
        assert_eq!(strip_leading_comments(sql2).trim(), "SELECT 1;");

        let sql3 = "# mysql comment\nSELECT 2;";
        assert_eq!(strip_leading_comments(sql3).trim(), "SELECT 2;");

        let sql4 = "-- unclosed";
        assert_eq!(strip_leading_comments(sql4), "");
    }

    #[test]
    fn test_validate_read_only_allowed() {
        assert!(validate_read_only_sql("SELECT * FROM users").is_ok());
        assert!(validate_read_only_sql("  select id, name from products where id = 1;").is_ok());
        assert!(validate_read_only_sql("EXPLAIN SELECT 1").is_ok());
        assert!(validate_read_only_sql("DESCRIBE orders").is_ok());
        assert!(validate_read_only_sql("SHOW TABLES;").is_ok());
        assert!(
            validate_read_only_sql("/* comment */ WITH cte AS (SELECT 1 AS val) SELECT * FROM cte")
                .is_ok()
        );
        // P2-D: Column aliases named set, call, do should not be falsely blocked
        assert!(validate_read_only_sql("SELECT 1 AS set").is_ok());
        assert!(validate_read_only_sql("SELECT 1 AS call").is_ok());
        assert!(validate_read_only_sql("SELECT 1 AS do").is_ok());
        assert!(validate_read_only_sql("SELECT 1 AS copy, 2 AS lock").is_ok());
    }

    #[test]
    fn test_validate_read_only_blocked() {
        assert!(validate_read_only_sql("").is_err());
        assert!(validate_read_only_sql("   ").is_err());
        assert!(validate_read_only_sql("INSERT INTO users VALUES (1)").is_err());
        assert!(validate_read_only_sql("UPDATE users SET name = 'foo'").is_err());
        assert!(validate_read_only_sql("DELETE FROM users WHERE id = 1").is_err());
        assert!(validate_read_only_sql("DROP TABLE users").is_err());
        assert!(validate_read_only_sql("ALTER TABLE users ADD COLUMN age INT").is_err());
        assert!(validate_read_only_sql("TRUNCATE TABLE users").is_err());
        assert!(validate_read_only_sql("CREATE TABLE foo (id INT)").is_err());
        assert!(validate_read_only_sql("GRANT ALL PRIVILEGES ON *.* TO 'root'").is_err());

        // P1-B: SELECT ... INTO is mutating table creation
        assert!(validate_read_only_sql("SELECT 1 AS x INTO TEMP _rg_tmp_probe").is_err());
        assert!(validate_read_only_sql("SELECT * INTO new_table FROM old_table").is_err());

        // P2-C: Dangerous system functions
        assert!(validate_read_only_sql("SELECT pg_terminate_backend(1234)").is_err());
        assert!(validate_read_only_sql("SELECT pg_cancel_backend(1234)").is_err());
        assert!(validate_read_only_sql("SELECT setval('seq', 1)").is_err());
        assert!(validate_read_only_sql("SELECT set_config('timeout', '1s', false)").is_err());
        assert!(validate_read_only_sql("SELECT (pg_stat_file('/etc/hosts')).size").is_err());

        // Comment evasion and multi-statement injection
        assert!(validate_read_only_sql("-- comment\nDROP TABLE users;").is_err());
        assert!(validate_read_only_sql("SELECT 1; DROP TABLE users;").is_err());
        assert!(validate_read_only_sql("SELECT 1; UPDATE users SET admin = 1;").is_err());

        // P0-1 regression: CTE wrapper mutations
        assert!(
            validate_read_only_sql("WITH x AS (DELETE FROM users RETURNING *) SELECT * FROM x;")
                .is_err()
        );
        assert!(
            validate_read_only_sql(
                "WITH x AS (UPDATE users SET name='x' RETURNING *) SELECT * FROM x;"
            )
            .is_err()
        );
        assert!(
            validate_read_only_sql("WITH x AS (INSERT INTO users VALUES(1) RETURNING *) SELECT 1;")
                .is_err()
        );

        // P0-2 regression: Dangerous file read and server program functions
        assert!(validate_read_only_sql("SELECT pg_read_file('/etc/hosts');").is_err());
        assert!(validate_read_only_sql("SELECT pg_read_binary_file('/etc/passwd');").is_err());
        assert!(validate_read_only_sql("SELECT load_file('/etc/passwd');").is_err());
    }
}
