//! Permission checks shared by every MCP tool.
//!
//! This module is deliberately conservative: the read-only check parses SQL with the engine's
//! `sqlparser` dialect and rejects everything that is not a single `SELECT`/`WITH … SELECT`
//! (or an `EXPLAIN` of one). It is a guardrail, not a database security boundary — the account
//! a connection uses remains the authority. The UI says so as well.

use std::ops::ControlFlow;

use rustgrid_core::DriverDialect;
use sqlparser::ast::{
    Expr, ObjectName, ObjectNamePart, Query, Select, Statement, TableFactor, Visit, Visitor,
};
use sqlparser::dialect::{
    ClickHouseDialect, Dialect, GenericDialect, MsSqlDialect, MySqlDialect, OracleDialect,
    PostgreSqlDialect, SQLiteDialect,
};
use sqlparser::parser::Parser;

/// What kind of object model the connection speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineFamily {
    /// A SQL database. `execute_query` gets SQL validation.
    Sql(SqlNaming),
    /// A key-value or document store. The driver's own read-only flag is the guard; SQL keyword
    /// validation would reject every command.
    NonSql,
}

/// How a SQL engine qualifies a relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlNaming {
    /// `database.table` (MySQL, ClickHouse, generic).
    Flat,
    /// `schema.table` inside the connection's database (PostgreSQL, SQL Server, Oracle).
    SchemaFirst,
    /// SQLite's `main.table` / `temp.table`, plus attached database aliases.
    Sqlite,
}

/// The policy one `execute_query` call is checked against.
#[derive(Debug, Clone, Copy)]
pub struct SqlPolicy<'a> {
    pub dialect: DriverDialect,
    pub naming: SqlNaming,
    /// The database the connection runs in when the request does not name one.
    pub default_database: Option<&'a str>,
    /// Empty means every database the account can see.
    pub allowed_databases: &'a [String],
    pub blacklisted_tables: &'a [String],
}

/// The engine family for a connection, derived from the driver's capabilities and dialect.
pub fn engine_family(
    dialect: DriverDialect,
    capabilities: &rustgrid_core::DriverCapabilities,
) -> EngineFamily {
    use rustgrid_core::DriverCapability;

    if capabilities.has(DriverCapability::KeyValue) || capabilities.has(DriverCapability::Documents)
    {
        return EngineFamily::NonSql;
    }
    EngineFamily::Sql(match dialect {
        DriverDialect::Sqlite => SqlNaming::Sqlite,
        DriverDialect::Postgres | DriverDialect::SqlServer | DriverDialect::Oracle => {
            SqlNaming::SchemaFirst
        }
        DriverDialect::Mysql | DriverDialect::ClickHouse | DriverDialect::Generic => {
            SqlNaming::Flat
        }
    })
}

/// The `sqlparser` dialect matching an engine.
pub fn parser_dialect(dialect: DriverDialect) -> Box<dyn Dialect> {
    match dialect {
        DriverDialect::Mysql => Box::new(MySqlDialect {}),
        DriverDialect::Postgres => Box::new(PostgreSqlDialect {}),
        DriverDialect::Oracle => Box::new(OracleDialect {}),
        DriverDialect::SqlServer => Box::new(MsSqlDialect {}),
        DriverDialect::Sqlite => Box::new(SQLiteDialect {}),
        DriverDialect::ClickHouse => Box::new(ClickHouseDialect {}),
        DriverDialect::Generic => Box::new(GenericDialect {}),
    }
}

/// Normalize one identifier for the blacklist/allowlist comparisons: drop a surrounding quote
/// pair and compare case-insensitively.
pub fn sanitize_identifier(ident: &str) -> String {
    ident
        .trim()
        .trim_matches(|c| c == '`' || c == '"' || c == '[' || c == ']')
        .to_lowercase()
}

fn normalize_parts(name: &str) -> Vec<String> {
    name.split('.')
        .map(sanitize_identifier)
        .filter(|part| !part.is_empty())
        .collect()
}

/// Whether a table name matches an entry of the sensitive-table blacklist.
///
/// A bare entry matches the table's last name in any schema; a qualified entry matches its exact
/// dotted name. Matching is intentionally broad: a table named `public.secrets` is hidden when the
/// user blacklists `secrets`, and vice versa.
pub fn is_table_blacklisted(table: &str, blacklist: &[String]) -> bool {
    let table_parts = normalize_parts(table);
    let Some(table_last) = table_parts.last() else {
        return false;
    };
    blacklist.iter().any(|entry| {
        let entry_parts = normalize_parts(entry);
        let Some(entry_last) = entry_parts.last() else {
            return false;
        };
        if entry_parts.len() == 1 {
            table_last == entry_last
        } else {
            table_parts == entry_parts || table_last == entry_last
        }
    })
}

/// Whether a database is inside the permission's scope. An empty list means every database.
pub fn is_database_allowed(database: Option<&str>, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let Some(database) = database.map(str::trim).filter(|db| !db.is_empty()) else {
        return false;
    };
    allowed
        .iter()
        .any(|entry| entry.trim().eq_ignore_ascii_case(database))
}

/// Read-only system catalogs a flat (database-first) connection may always inspect.
fn is_system_database(database: &str) -> bool {
    matches!(
        sanitize_identifier(database).as_str(),
        "information_schema" | "performance_schema" | "system"
    )
}

fn is_sqlite_builtin_schema(schema: &str) -> bool {
    matches!(sanitize_identifier(schema).as_str(), "main" | "temp")
}

/// Functions/table functions that read or write outside the database, or can execute arbitrary
/// code. Quoted names are already unquoted by the parser, so `"pg_read_file"` is matched too.
fn is_denied_function(name: &str) -> bool {
    let lower = sanitize_identifier(name);
    if lower.contains('@') {
        return true;
    }
    if lower.starts_with("pg_")
        || lower.starts_with("utl_")
        || lower.starts_with("dbms_")
        || lower.starts_with("lo_")
    {
        return true;
    }
    matches!(
        lower.as_str(),
        // PostgreSQL
        "setval"
            | "nextval"
            | "set_config"
            | "dblink"
            | "dblink_exec"
            | "dblink_connect"
            | "dblink_connect_u"
            | "dblink_disconnect"
            | "query_to_xml" // execs the query string; not a read of a table
            | "pg_sleep"
            // MySQL / MariaDB
            | "load_file"
            | "load_extension"
            | "benchmark"
            | "get_lock"
            | "release_lock"
            | "sys_exec"
            | "sys_eval"
            // SQL Server
            | "openrowset"
            | "openquery"
            | "opendatasource"
            | "openxml"
            | "xp_cmdshell"
            // ClickHouse external table functions
            | "file"
            | "url"
            | "s3"
            | "remote"
            | "remote_secure"
            | "remotesecure"
            | "cluster"
            | "mysql"
            | "postgresql"
            | "sqlite"
            | "jdbc"
            | "odbc"
            | "hdfs"
            | "azureblobstorage"
            | "hudi"
            | "deltalake"
            | "generate_random"
    )
}

/// Names that cannot be table relations. These are table-valued functions or external scanners
/// that a SELECT could otherwise use to read files or perform remote work.
fn is_denied_relation(name: &str) -> bool {
    let lower = sanitize_identifier(name);
    if lower.contains('@') {
        return true;
    }
    matches!(
        lower.as_str(),
        "openrowset"
            | "openquery"
            | "opendatasource"
            | "dblink"
            | "dblink_exec"
            | "dblink_connect"
            | "dblink_connect_u"
            | "query_to_xml"
            | "file"
            | "url"
            | "s3"
            | "remote"
            | "remote_secure"
            | "remotesecure"
            | "cluster"
            | "mysql"
            | "postgresql"
            | "sqlite"
            | "jdbc"
            | "odbc"
            | "hdfs"
            | "azureblobstorage"
            | "hudi"
            | "deltalake"
            | "input"
    )
}

/// Whether a relation is inside the permission's database scope.
fn relation_allowed(parts: &[String], policy: &SqlPolicy<'_>) -> bool {
    if parts.is_empty() {
        return false;
    }
    if policy.allowed_databases.is_empty() {
        return true;
    }

    // Oracle's tree's "databases" are schemas, so a two-part name checks its own qualifier and an
    // unqualified name uses the schema the operation named (or the connection's default, if the
    // scope is unrestricted).
    if policy.dialect == DriverDialect::Oracle {
        return match parts.len() {
            1 => is_database_allowed(policy.default_database, policy.allowed_databases),
            2 => is_database_allowed(Some(parts[0].as_str()), policy.allowed_databases),
            _ => false,
        };
    }

    match policy.naming {
        SqlNaming::SchemaFirst => match parts.len() {
            // schema.table (or a bare table): the connection's own database must be allowed.
            1 | 2 => is_database_allowed(policy.default_database, policy.allowed_databases),
            // SQL Server's database.schema.table; PostgreSQL/Oracle reject a three-part name.
            3 if policy.dialect == DriverDialect::SqlServer => {
                is_database_allowed(Some(parts[0].as_str()), policy.allowed_databases)
            }
            _ => false,
        },
        SqlNaming::Flat => match parts.len() {
            1 => is_database_allowed(policy.default_database, policy.allowed_databases),
            2 => {
                is_database_allowed(Some(parts[0].as_str()), policy.allowed_databases)
                    || is_system_database(&parts[0])
            }
            _ => false,
        },
        SqlNaming::Sqlite => match parts.len() {
            1 => is_database_allowed(policy.default_database, policy.allowed_databases),
            2 => {
                is_sqlite_builtin_schema(&parts[0])
                    || is_database_allowed(Some(parts[0].as_str()), policy.allowed_databases)
            }
            _ => false,
        },
    }
}

/// Validate one read-only query. `Ok` means the statement is a single `SELECT`/`WITH … SELECT`
/// (or an `EXPLAIN` of one) whose relations are inside the permission's scope and outside its
/// sensitive-table blacklist. The engine's own read-only session/flag is the second line of
/// defense.
pub fn validate_read_only_sql(sql: &str, policy: &SqlPolicy<'_>) -> Result<(), String> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err("SQL statement is empty".to_string());
    }
    // MySQL executes the contents of `/*! … */` (and MariaDB `/*M! … */`) as SQL. The parser
    // treats both as comments, so a mutation could hide there. Reject them outright.
    if sql.contains("/*!") || sql.contains("/*M!") {
        return Err(
            "Permission Denied: MySQL executable comments are not allowed in Read-Only mode."
                .to_string(),
        );
    }
    // `MySqlDialect` assumes backslash escapes. A server started with `NO_BACKSLASH_ESCAPES`
    // would end the string at `\'` and execute what the parser sees as a comment/string. Refuse
    // escape sequences that can change the statement boundary.
    if policy.dialect == DriverDialect::Mysql
        && (sql.contains("\\'") || sql.contains("\\\"") || sql.contains("\\`"))
    {
        return Err(
            "Permission Denied: backslash-quoted strings are not allowed in Read-Only mode."
                .to_string(),
        );
    }

    let dialect = parser_dialect(policy.dialect);
    let statements = Parser::parse_sql(dialect.as_ref(), sql)
        .map_err(|error| format!("Permission Denied: invalid SQL: {error}"))?;
    if statements.len() != 1 {
        return Err(
            "Permission Denied: only one statement is allowed in Read-Only mode.".to_string(),
        );
    }
    if !is_read_only_statement(&statements[0]) {
        return Err(
            "Permission Denied: only SELECT, WITH … SELECT and EXPLAIN of a SELECT are allowed in Read-Only mode."
                .to_string(),
        );
    }

    let mut guard = ReadOnlyGuard::new(policy);
    match statements.visit(&mut guard) {
        ControlFlow::Break(error) => Err(error),
        ControlFlow::Continue(()) => Ok(()),
    }
}

fn is_read_only_statement(statement: &Statement) -> bool {
    match statement {
        Statement::Query(_) => true,
        Statement::Explain { statement, .. } => is_read_only_statement(statement),
        _ => false,
    }
}

struct ReadOnlyGuard<'a> {
    policy: &'a SqlPolicy<'a>,
}

impl<'a> ReadOnlyGuard<'a> {
    fn new(policy: &'a SqlPolicy<'a>) -> Self {
        Self { policy }
    }

    fn object_parts(&self, name: &ObjectName) -> Result<Vec<String>, String> {
        let mut parts = Vec::with_capacity(name.0.len());
        for part in &name.0 {
            match part {
                ObjectNamePart::Identifier(ident) => parts.push(sanitize_identifier(&ident.value)),
                ObjectNamePart::Function(_) => {
                    // A dynamically-built identifier cannot be checked before execution.
                    return Err(
                        "Permission Denied: dynamic object names are not allowed in Read-Only mode."
                            .to_string(),
                    );
                }
            }
        }
        if parts.is_empty() || parts.iter().any(String::is_empty) {
            return Err("Permission Denied: invalid object name.".to_string());
        }
        Ok(parts)
    }

    fn check_relation(&self, name: &ObjectName) -> Result<(), String> {
        let parts = self.object_parts(name)?;
        let full = parts.join(".");
        if full.contains('@') {
            return Err(format!(
                "Permission Denied: database links are not allowed ('{full}')."
            ));
        }
        if is_denied_relation(&parts[parts.len() - 1]) {
            return Err(format!(
                "Permission Denied: external table function '{}' is not allowed in Read-Only mode.",
                parts[parts.len() - 1]
            ));
        }
        if is_table_blacklisted(&full, self.policy.blacklisted_tables) {
            return Err(format!(
                "Permission Denied: table '{full}' is on the sensitive-table blacklist."
            ));
        }
        if !relation_allowed(&parts, self.policy) {
            return Err(format!(
                "Permission Denied: '{full}' is outside the allowed database scope."
            ));
        }
        Ok(())
    }

    fn check_function(&self, name: &ObjectName) -> Result<(), String> {
        let parts = self.object_parts(name)?;
        let last = &parts[parts.len() - 1];
        if is_denied_function(last) {
            return Err(format!(
                "Permission Denied: function '{}' is not allowed in Read-Only mode.",
                parts.join(".")
            ));
        }
        Ok(())
    }
}

impl Visitor for ReadOnlyGuard<'_> {
    type Break = String;

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Self::Break> {
        if is_read_only_statement(statement) {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(format!(
                "Permission Denied: '{}' is not allowed in Read-Only mode.",
                statement_kind(statement)
            ))
        }
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if !query.locks.is_empty() {
            return ControlFlow::Break(
                "Permission Denied: locking reads (FOR UPDATE / FOR SHARE) are not allowed."
                    .to_string(),
            );
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_select(&mut self, select: &Select) -> ControlFlow<Self::Break> {
        if select.into.is_some() {
            return ControlFlow::Break(
                "Permission Denied: SELECT ... INTO is not allowed in Read-Only mode.".to_string(),
            );
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<Self::Break> {
        match self.check_relation(relation) {
            Ok(()) => ControlFlow::Continue(()),
            Err(error) => ControlFlow::Break(error),
        }
    }

    fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<Self::Break> {
        let name = match factor {
            TableFactor::Table { name, args, .. } => {
                if args.is_some() {
                    Some(name)
                } else {
                    None
                }
            }
            TableFactor::Function { name, .. } => Some(name),
            _ => None,
        };
        if let Some(name) = name
            && let Err(error) = self.check_relation(name)
        {
            return ControlFlow::Break(error);
        }
        // A set-returning function in FROM (`SELECT * FROM pg_ls_dir(...)`, `dblink(...)`) is a
        // table factor, not an expression, so the function deny list must be applied here too.
        if let Some(name) = name
            && let Err(error) = self.check_function(name)
        {
            return ControlFlow::Break(error);
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        match expr {
            Expr::Function(function) => {
                if let Err(error) = self.check_function(&function.name) {
                    return ControlFlow::Break(error);
                }
            }
            // Oracle sequences are selected as `sequence.nextval`; the expression is an identifier
            // pair, not a function call, and `nextval` still changes database state.
            Expr::CompoundIdentifier(parts)
                if parts
                    .last()
                    .is_some_and(|part| part.value.eq_ignore_ascii_case("nextval")) =>
            {
                return ControlFlow::Break(
                    "Permission Denied: sequence nextval is not allowed in Read-Only mode."
                        .to_string(),
                );
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

fn statement_kind(statement: &Statement) -> &'static str {
    match statement {
        Statement::Query(_) => "SELECT",
        Statement::Insert(_) => "INSERT",
        Statement::Update { .. } => "UPDATE",
        Statement::Delete(_) => "DELETE",
        Statement::CreateTable(_) => "CREATE TABLE",
        Statement::CreateView { .. } => "CREATE VIEW",
        Statement::CreateIndex(_) => "CREATE INDEX",
        Statement::Drop { .. } => "DROP",
        Statement::AlterTable(_) => "ALTER TABLE",
        Statement::Truncate { .. } => "TRUNCATE",
        Statement::Merge { .. } => "MERGE",
        Statement::Set(_) => "SET",
        Statement::Use(_) => "USE",
        Statement::Copy { .. } => "COPY",
        Statement::Call(_) => "CALL",
        Statement::Execute { .. } => "EXECUTE",
        Statement::Explain { .. } => "EXPLAIN",
        _ => "this statement",
    }
}

/// The database reference check used by the metadata tools. It accepts a bare table or a
/// qualified `schema.table` / `database.table` and applies the same scope/blacklist rules as the
/// SQL validator.
pub fn check_table_reference(
    table: &str,
    database: Option<&str>,
    policy: &SqlPolicy<'_>,
) -> Result<(), String> {
    if is_table_blacklisted(table, policy.blacklisted_tables) {
        return Err(format!(
            "Permission Denied: table '{table}' is on the sensitive-table blacklist."
        ));
    }
    if !policy.allowed_databases.is_empty()
        && !is_database_allowed(database, policy.allowed_databases)
    {
        return Err(format!(
            "Permission Denied: database '{}' is outside the allowed scope.",
            database.unwrap_or("")
        ));
    }

    let parts = normalize_parts(table);
    if parts.is_empty() {
        return Err("Permission Denied: invalid table name.".to_string());
    }
    if parts.iter().any(|part| part.contains('@')) {
        return Err("Permission Denied: database links are not allowed.".to_string());
    }
    if is_denied_relation(&parts[parts.len() - 1]) {
        return Err(format!(
            "Permission Denied: external table function '{}' is not allowed.",
            parts[parts.len() - 1]
        ));
    }
    // A qualified name is checked against the same rules as a relation in a query.
    if parts.len() > 1 && !relation_allowed(&parts, policy) {
        return Err(format!(
            "Permission Denied: '{table}' is outside the allowed database scope."
        ));
    }
    Ok(())
}

/// The database reference check used by `get_schema`, `describe_table` and `sample_data`.
pub fn check_database(database: Option<&str>, allowed: &[String]) -> Result<(), String> {
    if is_database_allowed(database, allowed) {
        Ok(())
    } else {
        Err(format!(
            "Permission Denied: database '{}' is outside the allowed scope.",
            database.unwrap_or("")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgrid_core::{DriverCapabilities, DriverCapability};

    fn policy<'a>(naming: SqlNaming, dialect: DriverDialect) -> SqlPolicy<'a> {
        SqlPolicy {
            dialect,
            naming,
            default_database: Some("app"),
            allowed_databases: &[],
            blacklisted_tables: &[],
        }
    }

    #[test]
    fn allows_plain_select_and_explain() {
        let p = policy(SqlNaming::Flat, DriverDialect::Mysql);
        assert!(validate_read_only_sql("SELECT * FROM users", &p).is_ok());
        assert!(validate_read_only_sql("WITH x AS (SELECT 1) SELECT * FROM x", &p).is_ok());
        assert!(validate_read_only_sql("EXPLAIN SELECT * FROM users", &p).is_ok());
    }

    #[test]
    fn rejects_writes_and_multiple_statements() {
        let p = policy(SqlNaming::Flat, DriverDialect::Mysql);
        for sql in [
            "INSERT INTO users VALUES (1)",
            "UPDATE users SET name = 'x'",
            "DELETE FROM users",
            "DROP TABLE users",
            "CREATE TABLE x (id INT)",
            "SELECT 1; DROP TABLE users",
            "WITH x AS (DELETE FROM users RETURNING *) SELECT * FROM x",
            "SELECT * INTO copy FROM users",
            "SELECT * FROM users FOR UPDATE",
        ] {
            assert!(validate_read_only_sql(sql, &p).is_err(), "allowed: {sql}");
        }
    }

    #[test]
    fn rejects_mysql_executable_comments_and_escape_tricks() {
        let p = policy(SqlNaming::Flat, DriverDialect::Mysql);
        assert!(validate_read_only_sql("SELECT 1 /*! INTO OUTFILE '/tmp/x' */", &p).is_err());
        assert!(validate_read_only_sql("SELECT 'a\\'; DROP TABLE users; --'", &p).is_err());
    }

    #[test]
    fn rejects_backslash_boundary_on_postgres() {
        let p = policy(SqlNaming::SchemaFirst, DriverDialect::Postgres);
        let sql = "SELECT 'a\\'; DROP TABLE users; --'";
        assert!(validate_read_only_sql(sql, &p).is_err());
    }

    #[test]
    fn rejects_quoted_dangerous_functions() {
        let p = policy(SqlNaming::SchemaFirst, DriverDialect::Postgres);
        assert!(validate_read_only_sql("SELECT \"pg_read_file\"('/etc/passwd')", &p).is_err());
        assert!(validate_read_only_sql("SELECT pg_catalog.setval('s', 1)", &p).is_err());
        // A set-returning function in FROM is a table factor, not `Expr::Function`.
        assert!(validate_read_only_sql("SELECT * FROM pg_ls_dir('/')", &p).is_err());
        assert!(validate_read_only_sql("SELECT * FROM generate_series(1, 3)", &p).is_ok());
        let p = policy(SqlNaming::Flat, DriverDialect::Mysql);
        assert!(validate_read_only_sql("SELECT `LOAD_FILE`('/etc/passwd')", &p).is_err());
    }

    #[test]
    fn rejects_sql_server_external_table_functions() {
        let p = policy(SqlNaming::SchemaFirst, DriverDialect::SqlServer);
        assert!(
            validate_read_only_sql(
                "SELECT * FROM OPENROWSET(BULK 'C:\\secret', SINGLE_CLOB) AS x",
                &p
            )
            .is_err()
        );
        assert!(validate_read_only_sql("SELECT * FROM OPENQUERY(lnk, 'SELECT 1')", &p).is_err());
    }

    #[test]
    fn blacklist_matches_qualified_and_bare_names() {
        let blacklist = vec!["secret".to_string()];
        assert!(is_table_blacklisted("public.secret", &blacklist));
        assert!(is_table_blacklisted("secret", &blacklist));
        assert!(!is_table_blacklisted("public.orders", &blacklist));

        let blacklist = vec!["public.secret".to_string()];
        assert!(is_table_blacklisted("public.secret", &blacklist));
        assert!(is_table_blacklisted("secret", &blacklist));
    }

    #[test]
    fn scope_rejects_qualified_databases() {
        let allowed = vec!["app".to_string()];
        let p = SqlPolicy {
            dialect: DriverDialect::Mysql,
            naming: SqlNaming::Flat,
            default_database: Some("app"),
            allowed_databases: &allowed,
            blacklisted_tables: &[],
        };
        assert!(validate_read_only_sql("SELECT * FROM users", &p).is_ok());
        assert!(validate_read_only_sql("SELECT * FROM other.users", &p).is_err());
        assert!(validate_read_only_sql("SELECT * FROM information_schema.tables", &p).is_ok());

        let p = SqlPolicy {
            dialect: DriverDialect::Postgres,
            naming: SqlNaming::SchemaFirst,
            default_database: Some("app"),
            allowed_databases: &allowed,
            blacklisted_tables: &[],
        };
        assert!(validate_read_only_sql("SELECT * FROM public.users", &p).is_ok());
    }

    #[test]
    fn oracle_scope_uses_schema_as_the_database_unit() {
        let allowed = vec!["hr".to_string()];
        let p = SqlPolicy {
            dialect: DriverDialect::Oracle,
            naming: SqlNaming::SchemaFirst,
            default_database: None,
            allowed_databases: &allowed,
            blacklisted_tables: &[],
        };
        assert!(validate_read_only_sql("SELECT * FROM hr.employees", &p).is_ok());
        assert!(validate_read_only_sql("SELECT * FROM scott.emp", &p).is_err());
        // Without a schema the connection's current schema is unknown; the restricted scope
        // refuses rather than guessing.
        assert!(validate_read_only_sql("SELECT * FROM employees", &p).is_err());
        assert!(validate_read_only_sql("SELECT seq.nextval FROM hr.employees", &p).is_err());
    }

    #[test]
    fn engine_family_uses_capabilities() {
        let caps = DriverCapabilities::none().with(DriverCapability::KeyValue);
        assert_eq!(
            engine_family(DriverDialect::Generic, &caps),
            EngineFamily::NonSql
        );
        let caps = DriverCapabilities::none();
        assert_eq!(
            engine_family(DriverDialect::Postgres, &caps),
            EngineFamily::Sql(SqlNaming::SchemaFirst)
        );
    }
}
