use std::collections::BTreeSet;
use std::ptr::null_mut;
use std::sync::Mutex;
use std::sync::MutexGuard;

use async_trait::async_trait;
use rustgrid_core::{
    BackupObjectKind, CellValue, ColumnInfo, Connection, ConnectionConfig, DatabaseInfo,
    DatabaseOptions, DriverDialect, DriverId, Error, FilterCondition, FilterConjunction,
    FilterNode, FilterOperator, ObjectDump, ObjectKind, PageRequest, QueryResult, Result,
    RowInsert, RowUpdate, SortColumn, TableInfo, TablePage, TableSchema, TableStatus, ViewDetails,
    ViewEdit, ViewInfo,
};

use crate::api::{self, Hdbc, type_name};

/// A raw ODBC connection handle. It is only ever touched behind [`OdbcConnection`]'s `Mutex`, so it
/// is safe to move between threads.
struct ConnHandle(Hdbc);

// SAFETY: the handle is only accessed through the `Mutex<ConnHandle>`; a raw pointer is just an
// address and carries no thread affinity of its own here.
unsafe impl Send for ConnHandle {}

/// A connection backed by one runtime-loaded ODBC connection handle. ODBC is synchronous and the
/// handle is not `Sync`, so it is guarded by a `Mutex` and each operation holds that guard for its
/// whole duration — the connection's statement state is shared by every query.
pub struct OdbcConnection {
    handle: Mutex<ConnHandle>,
    /// The underlying engine hint (`odbc.engine`), used to pick quoting and paging syntax.
    engine: String,
    /// The identifier quote pair and catalog separator the driver reports (or the engine's).
    quote: (char, char),
    catalog_separator: char,
    /// `SQL_IC_SENSITIVE` means column names are matched exactly.
    identifier_case: u16,
}

impl OdbcConnection {
    pub(crate) fn new(handle: Hdbc, config: &ConnectionConfig) -> Self {
        let mut engine = config
            .options
            .get("odbc.engine")
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        // When the user did not set the engine hint, infer it from the connected DBMS name so
        // paging, renaming and database listing pick the right dialect. A free-text hint that is
        // not one of our exact names is normalized the same way.
        if !engine.is_empty() {
            let detected = detect_engine(&engine);
            if !detected.is_empty() {
                engine = detected;
            }
        }
        if engine.is_empty()
            && let Ok(api) = api::api()
            && let Some(name) = api.dbms_name(handle)
        {
            engine = detect_engine(&name);
        }
        // A driver that reports no DBMS name at all still usually names its engine in its own
        // description ("MySQL ODBC 8.0 Unicode Driver"), which is what the form picked, so fall
        // back to that before giving up on the dialect.
        if engine.is_empty()
            && let Some(driver) = config.options.get("odbc.driver")
        {
            engine = detect_engine(driver);
        }
        let (quote, catalog_separator, identifier_case) = quote_style(handle, &engine);
        Self {
            handle: Mutex::new(ConnHandle(handle)),
            engine,
            quote,
            catalog_separator,
            identifier_case,
        }
    }

    fn handle(&self) -> Result<MutexGuard<'_, ConnHandle>> {
        self.handle
            .lock()
            .map_err(|_| Error::Connection("the ODBC connection is poisoned".to_string()))
    }

    /// Run `operation` with the connection guard held, so no other request can use the same ODBC
    /// connection concurrently. (ODBC connection handles are not thread-safe, and the previous
    /// implementation dropped the guard before running the statement.)
    fn with_handle<T>(
        &self,
        operation: impl FnOnce(&'static api::OdbcApi, Hdbc) -> Result<T>,
    ) -> Result<T> {
        let api = api::api().map_err(Error::Connection)?;
        let guard = self.handle()?;
        if guard.0.is_null() {
            return Err(Error::Connection(
                "the ODBC connection is closed".to_string(),
            ));
        }
        operation(api, guard.0)
    }

    /// Run one statement, returning `(columns, rows, rows_affected)`. `columns` is non-empty only
    /// when the statement produced a result set.
    #[allow(clippy::type_complexity)]
    fn run(&self, sql: &str) -> Result<(Vec<ColumnInfo>, Vec<Vec<CellValue>>, Option<u64>)> {
        self.with_handle(|api, dbc| run_text(api, dbc, sql))
    }

    /// Run one prepared statement with `?` placeholders bound as text.
    #[allow(clippy::type_complexity)]
    fn run_prepared(
        &self,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<(Vec<ColumnInfo>, Vec<Vec<CellValue>>, Option<u64>)> {
        self.with_handle(|api, dbc| run_prepared(api, dbc, sql, params))
    }

    /// Quote an identifier with the pair the driver reports (or the engine's own).
    fn quote(&self, name: &str) -> String {
        let (open, close) = self.quote;
        let mut quoted = String::with_capacity(name.len() + 2);
        quoted.push(open);
        for character in name.chars() {
            if character == close {
                quoted.push(close);
            }
            quoted.push(character);
        }
        quoted.push(close);
        quoted
    }

    /// Compare two metadata identifiers. `SQL_IC_SENSITIVE` keeps the exact match first; every
    /// other mode falls back to an ASCII-case-insensitive comparison (column names from
    /// `SELECT *` and from the catalog can differ in case on some drivers).
    fn same_identifier(&self, left: &str, right: &str) -> bool {
        left == right || (self.identifier_case != 3 && left.eq_ignore_ascii_case(right))
    }

    /// The SQL dialect the connected engine speaks.
    fn dialect(&self) -> DriverDialect {
        dialect_for_engine(&self.engine)
    }

    /// Whether the engine accepts a catalog qualifier (`database.schema.object`). PostgreSQL and
    /// Oracle cannot reference another database from a connection, and SQLite has no catalog, so
    /// those use the (already schema-qualified) table name alone.
    fn prefixes_catalog(&self) -> bool {
        !matches!(
            self.engine.to_ascii_lowercase().as_str(),
            "postgresql" | "postgres" | "oracle" | "sqlite"
        )
    }

    /// A fully-qualified, quoted object name: `database` (when the engine takes a catalog) plus the
    /// dotted `table`. The catalog separator comes from the driver for unknown engines.
    fn qualify(&self, database: &str, table: &str) -> String {
        let mut parts = Vec::new();
        for part in table.split('.') {
            if !part.is_empty() {
                parts.push(self.quote(part));
            }
        }
        let name = parts.join(".");
        if !database.is_empty() && self.prefixes_catalog() {
            format!("{}{}{}", self.quote(database), self.catalog_separator, name)
        } else {
            name
        }
    }

    /// Prepare `sql` with `?` placeholders, bind `params` (text; `None` is SQL NULL), and execute.
    fn execute_script(&self, sql: &str, params: &[Option<String>]) -> Result<Option<u64>> {
        self.with_handle(|api, dbc| {
            let statement = api.prepare(dbc, sql).map_err(Error::Query)?;
            let executed = api.execute_bound(statement, params);
            let affected = api.rows_affected(statement);
            api.free_statement(statement);
            executed.map_err(Error::Query)?;
            Ok(affected)
        })
    }

    /// Run a statement that returns no result set.
    fn run_statement(&self, sql: &str) -> Result<()> {
        self.run(sql).map(|_| ())
    }

    /// Switch the connection's current catalog/schema when the engine has a session-level verb for
    /// it, so a query tab opened on another database reads that database.
    fn use_database(&self, api: &api::OdbcApi, dbc: Hdbc, database: &str) -> Result<()> {
        let lower = self.engine.to_ascii_lowercase();
        let sql = match lower.as_str() {
            "mysql" | "mariadb" | "sqlserver" => {
                format!("USE {}", self.quote(database))
            }
            "oracle" => format!(
                "ALTER SESSION SET CURRENT_SCHEMA = {}",
                self.quote(database)
            ),
            _ => return Ok(()),
        };
        let statement = api.execute(dbc, &sql).map_err(Error::Query)?;
        api.free_statement(statement);
        Ok(())
    }

    /// The `RENAME` statement for the target engine.
    fn rename_table_sql(&self, database: &str, table: &str, new_name: &str) -> String {
        match self.engine.to_ascii_lowercase().as_str() {
            "sqlserver" => format!(
                "EXEC sp_rename N'{}', N'{}'",
                table.replace('\'', "''"),
                new_name.replace('\'', "''")
            ),
            "mysql" | "mariadb" => format!(
                "RENAME TABLE {} TO {}",
                self.qualify(database, table),
                self.qualify(database, new_name)
            ),
            _ => format!(
                "ALTER TABLE {} RENAME TO {}",
                self.qualify(database, table),
                self.quote(new_name)
            ),
        }
    }

    /// The first column of `sql` as text, skipping NULLs and empty values (used for the catalog
    /// queries — `sys.databases` / `SHOW DATABASES` — that list the server's databases).
    fn first_column(&self, sql: &str) -> Result<Vec<String>> {
        let (_columns, rows, _affected) = self.run(sql)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| row.into_iter().next())
            .filter_map(|value| match value {
                CellValue::Text(text) if !text.is_empty() => Some(text),
                _ => None,
            })
            .collect())
    }

    /// The catalogs the driver's own `SQLTables` reports, for the engines that have no catalog
    /// query of their own.
    fn table_catalogs(&self) -> Result<Vec<String>> {
        let (columns, rows) =
            self.with_handle(|api, dbc| api.tables(dbc, "", "", "", "").map_err(Error::Query))?;
        let index = column_index(&columns, "TABLE_CAT");
        let mut names = BTreeSet::new();
        for row in rows {
            if let Some(CellValue::Text(name)) = index.and_then(|index| row.get(index))
                && !name.is_empty()
            {
                names.insert(name.clone());
            }
        }
        Ok(names.into_iter().collect())
    }

    /// The schemas the driver's own `SQLTables` reports. Oracle has no catalogs but every object
    /// lives in a schema, so those become the connection tree's "databases".
    fn table_schemas(&self) -> Result<Vec<String>> {
        let (columns, rows) =
            self.with_handle(|api, dbc| api.tables(dbc, "", "", "", "").map_err(Error::Query))?;
        let index = column_index(&columns, "TABLE_SCHEM");
        let mut names = BTreeSet::new();
        for row in rows {
            if let Some(CellValue::Text(name)) = index.and_then(|index| row.get(index))
                && !name.is_empty()
            {
                names.insert(name.clone());
            }
        }
        Ok(names.into_iter().collect())
    }

    /// The connection's current catalog name, as a last-resort database list (SQLite, a DSN with a
    /// single database).
    fn current_database(&self) -> Result<Option<String>> {
        self.with_handle(|api, dbc| Ok(api.database_name(dbc)))
    }

    /// The engine-specific `SHOW CREATE`/`OBJECT_DEFINITION` path, when the engine has one.
    async fn native_ddl(
        &self,
        database: &str,
        table: &str,
        is_view: bool,
    ) -> Result<Option<String>> {
        let qualified = self.qualify(database, table);
        match self.engine.to_ascii_lowercase().as_str() {
            "mysql" | "mariadb" => {
                let object = if is_view { "VIEW" } else { "TABLE" };
                let (columns, rows, _) = self.run(&format!("SHOW CREATE {object} {qualified}"))?;
                let index = if columns.len() > 1 { 1 } else { 0 };
                Ok(rows
                    .into_iter()
                    .next()
                    .and_then(|row| row.get(index).map(CellValue::as_display))
                    .filter(|ddl| !ddl.trim().is_empty()))
            }
            "sqlserver" if is_view => {
                let literal = qualified.replace('\'', "''");
                let (columns, rows, _) = self.run(&format!(
                    "SELECT OBJECT_DEFINITION(OBJECT_ID(N'{literal}'))"
                ))?;
                let index = column_index(&columns, "OBJECT_DEFINITION")
                    .or_else(|| columns.first().map(|_| 0));
                Ok(rows
                    .into_iter()
                    .next()
                    .and_then(|row| index.and_then(|index| row.get(index).cloned()))
                    .filter(|value| !matches!(value, CellValue::Null))
                    .map(|value| value.as_display())
                    .filter(|ddl| !ddl.trim().is_empty()))
            }
            "sqlite" => {
                // SQLite stores the original `CREATE` text for both tables and views.
                let kind = if is_view { "view" } else { "table" };
                let name = table.rsplit('.').next().unwrap_or(table);
                let literal = quote_string(name, "sqlite");
                let Ok((_columns, rows, _)) = self.run(&format!(
                    "SELECT sql FROM sqlite_master \
                     WHERE type = '{kind}' AND name = {literal}"
                )) else {
                    return Ok(None);
                };
                Ok(rows
                    .first()
                    .and_then(|row| row.first())
                    .map(CellValue::as_display)
                    .filter(|ddl| !ddl.trim().is_empty()))
            }
            "postgresql" | "postgres" if is_view => {
                let literal = quote_string(table, "postgresql");
                let Ok((_columns, rows, _)) =
                    self.run(&format!("SELECT pg_get_viewdef({literal}, true)"))
                else {
                    return Ok(None);
                };
                Ok(rows
                    .first()
                    .and_then(|row| row.first())
                    .map(CellValue::as_display)
                    .filter(|body| !body.trim().is_empty())
                    .map(|body| {
                        format!(
                            "CREATE VIEW {qualified} AS {}",
                            body.trim().trim_end_matches(';').trim_end()
                        )
                    }))
            }
            "oracle" if is_view => {
                let (_, schema, bare) = catalog_parts(&self.engine, database, table);
                let name_literal = quote_string(&bare, "oracle");
                let sql = if schema.is_empty() {
                    format!("SELECT DBMS_METADATA.GET_DDL('VIEW', {name_literal}) FROM DUAL")
                } else {
                    format!(
                        "SELECT DBMS_METADATA.GET_DDL('VIEW', {name_literal}, {}) FROM DUAL",
                        quote_string(&schema, "oracle")
                    )
                };
                let Ok((_columns, rows, _)) = self.run(&sql) else {
                    return Ok(None);
                };
                Ok(rows
                    .first()
                    .and_then(|row| row.first())
                    .map(CellValue::as_display)
                    .filter(|ddl| !ddl.trim().is_empty()))
            }
            _ => Ok(None),
        }
    }

    /// Build a portable `CREATE TABLE` from the driver's `SQLColumns`/`SQLPrimaryKeys` catalogs.
    fn catalog_ddl(&self, database: &str, table: &str) -> Result<Option<String>> {
        let (catalog, schema, bare) = catalog_parts(&self.engine, database, table);
        let (columns, rows) = self.with_handle(|api, dbc| {
            api.columns(dbc, &catalog, &schema, &bare)
                .map_err(Error::Query)
        })?;
        if rows.is_empty() {
            return Ok(None);
        }

        let name_index = column_index(&columns, "COLUMN_NAME");
        let type_index = column_index(&columns, "TYPE_NAME");
        let code_index = column_index(&columns, "DATA_TYPE");
        let size_index = column_index(&columns, "COLUMN_SIZE");
        let digits_index = column_index(&columns, "DECIMAL_DIGITS");
        let nullable_index = column_index(&columns, "NULLABLE");
        let default_index = column_index(&columns, "COLUMN_DEF");
        let Some(name_index) = name_index else {
            return Ok(None);
        };

        let mut definitions = Vec::new();
        for row in &rows {
            let Some(name) = row.get(name_index).and_then(cell_text) else {
                continue;
            };
            let code = row
                .get(code_index.unwrap_or(usize::MAX))
                .and_then(cell_int)
                .unwrap_or(0) as i16;
            let type_name = type_index
                .and_then(|index| row.get(index))
                .and_then(cell_text)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| type_name(code).to_string());
            let size = size_index
                .and_then(|index| row.get(index))
                .and_then(cell_int)
                .unwrap_or(0);
            let digits = digits_index
                .and_then(|index| row.get(index))
                .and_then(cell_int)
                .unwrap_or(0);
            let mut definition = format!(
                "  {} {}",
                self.quote(name),
                column_type_sql(code, &type_name, size, digits)
            );
            let nullable = nullable_index
                .and_then(|index| row.get(index))
                .and_then(cell_int)
                .is_none_or(|value| value != 0);
            if !nullable {
                definition.push_str(" NOT NULL");
            }
            if let Some(default) = default_index
                .and_then(|index| row.get(index))
                .and_then(cell_text)
                .filter(|value| !value.trim().is_empty())
            {
                definition.push_str(" DEFAULT ");
                definition.push_str(default);
            }
            definitions.push(definition);
        }

        if let Ok(keys) = self.with_handle(|api, dbc| {
            api.primary_keys(dbc, &catalog, &schema, &bare)
                .map_err(Error::Query)
        }) && !keys.is_empty()
        {
            let keys = keys
                .iter()
                .map(|key| self.quote(key))
                .collect::<Vec<_>>()
                .join(", ");
            definitions.push(format!("  PRIMARY KEY ({keys})"));
        }

        if definitions.is_empty() {
            return Ok(None);
        }
        Ok(Some(format!(
            "CREATE TABLE {} (\n{}\n)",
            self.qualify(database, table),
            definitions.join(",\n")
        )))
    }

    /// The `CREATE TABLE` script the Import Wizard's Create Table step runs.
    fn create_table_sql(&self, database: &str, table: &str, schema: &TableSchema) -> String {
        let mut definitions = Vec::new();
        let mut primary_key = Vec::new();
        for column in &schema.columns {
            if column.name.trim().is_empty() {
                continue;
            }
            let mut definition = format!(
                "  {} {}",
                self.quote(&column.name),
                column_type_from_def(column)
            );
            if !column.nullable {
                definition.push_str(" NOT NULL");
            }
            if !column.default.trim().is_empty() {
                definition.push_str(" DEFAULT ");
                definition.push_str(column.default.trim());
            }
            if column.primary_key {
                primary_key.push(self.quote(&column.name));
            }
            definitions.push(definition);
        }
        if !primary_key.is_empty() {
            definitions.push(format!("  PRIMARY KEY ({})", primary_key.join(", ")));
        }
        if definitions.is_empty() {
            return String::new();
        }
        format!(
            "CREATE TABLE {} (\n{}\n)",
            self.qualify(database, table),
            definitions.join(",\n")
        )
    }

    /// Per-column comments and identity/generated flags from the engine's own catalog.
    fn engine_column_flags(
        &self,
        database: &str,
        table: &str,
    ) -> Result<Vec<(String, String, bool, bool)>> {
        match self.engine.to_ascii_lowercase().as_str() {
            "mysql" | "mariadb" => self.mysql_column_flags(database, table),
            "sqlserver" => self.sqlserver_column_flags(database, table),
            "postgresql" | "postgres" => self.postgresql_column_flags(database, table),
            "oracle" => self.oracle_column_flags(database, table),
            "sqlite" => self.sqlite_column_flags(database, table),
            _ => Ok(Vec::new()),
        }
    }

    /// The MySQL/MariaDB per-column flags from `information_schema.COLUMNS`:
    /// `(name, comment, auto_increment, generated)`.
    fn mysql_column_flags(
        &self,
        database: &str,
        table: &str,
    ) -> Result<Vec<(String, String, bool, bool)>> {
        if !matches!(
            self.engine.to_ascii_lowercase().as_str(),
            "mysql" | "mariadb"
        ) || database.is_empty()
        {
            return Ok(Vec::new());
        }
        let name = table.rsplit('.').next().unwrap_or(table);
        let schema = quote_string(database, "mysql");
        let name = quote_string(name, "mysql");
        let (columns, rows, _) = self.run(&format!(
            "SELECT COLUMN_NAME, COLUMN_COMMENT, EXTRA FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = {schema} AND TABLE_NAME = {name} \
             ORDER BY ORDINAL_POSITION"
        ))?;
        let name_index = column_index(&columns, "COLUMN_NAME");
        let comment_index = column_index(&columns, "COLUMN_COMMENT");
        let extra_index = column_index(&columns, "EXTRA");
        let mut flags = Vec::new();
        for row in rows {
            let Some(name) = name_index
                .and_then(|index| row.get(index))
                .and_then(cell_text)
            else {
                continue;
            };
            let comment = comment_index
                .and_then(|index| row.get(index))
                .and_then(cell_text)
                .unwrap_or_default()
                .to_string();
            let extra = extra_index
                .and_then(|index| row.get(index))
                .and_then(cell_text)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let auto_increment = extra.contains("auto_increment");
            // `DEFAULT_GENERATED` is a default expression, not a computed column.
            let generated = extra.contains("virtual generated")
                || extra.contains("stored generated")
                || extra.contains("generated always");
            flags.push((name.to_string(), comment, auto_increment, generated));
        }
        Ok(flags)
    }

    /// SQL Server identity/computed flags and MS_Description comments.
    fn sqlserver_column_flags(
        &self,
        database: &str,
        table: &str,
    ) -> Result<Vec<(String, String, bool, bool)>> {
        let qualified = match (database.is_empty(), table.contains('.')) {
            // A bare table with a catalog uses the login's default schema (`db..table`).
            (false, false) => format!("{database}..{table}"),
            (false, true) => format!("{database}.{table}"),
            (true, _) => table.to_string(),
        };
        let literal = quote_string(&qualified, "sqlserver");
        let (columns, rows, _) = self.run(&format!(
            "SELECT c.name AS COLUMN_NAME, \
                    COALESCE(CONVERT(nvarchar(4000), ep.value), '') AS COLUMN_COMMENT, \
                    c.is_identity, c.is_computed \
             FROM sys.columns c \
             LEFT JOIN sys.extended_properties ep \
               ON ep.class = 1 AND ep.major_id = c.object_id AND ep.minor_id = c.column_id \
              AND ep.name = 'MS_Description' \
             WHERE c.object_id = OBJECT_ID(N'{literal}') \
             ORDER BY c.column_id"
        ))?;
        Ok(flags_from_catalog(
            &columns,
            rows,
            "COLUMN_NAME",
            "COLUMN_COMMENT",
            "is_identity",
            "is_computed",
        ))
    }

    /// PostgreSQL identity/serial/generated flags and column comments.
    fn postgresql_column_flags(
        &self,
        _database: &str,
        table: &str,
    ) -> Result<Vec<(String, String, bool, bool)>> {
        let (_, schema, bare) = catalog_parts(&self.engine, "", table);
        let table_literal = quote_string(&bare, "postgresql");
        let schema_clause = if schema.is_empty() {
            "c.table_schema = current_schema()".to_string()
        } else {
            format!("c.table_schema = {}", quote_string(&schema, "postgresql"))
        };
        let query = |generated: &str| {
            format!(
                "SELECT c.column_name AS COLUMN_NAME, \
                        COALESCE(col_description((quote_ident(c.table_schema) || '.' || \
                         quote_ident(c.table_name))::regclass::oid, c.ordinal_position), '') \
                         AS COLUMN_COMMENT, \
                        CASE WHEN c.is_identity = 'YES' OR c.column_default LIKE 'nextval(%' \
                             THEN 1 ELSE 0 END AS IS_AUTO, \
                        {generated} AS IS_GENERATED \
                 FROM information_schema.columns c \
                 WHERE {schema_clause} AND c.table_name = {table_literal} \
                 ORDER BY c.ordinal_position"
            )
        };
        // `is_generated` only exists from PostgreSQL 12; retry without it on an older server.
        let (columns, rows, _) = match self.run(&query(
            "CASE WHEN c.is_generated = 'ALWAYS' THEN 1 ELSE 0 END",
        )) {
            Ok(result) => result,
            Err(_) => self.run(&query("0"))?,
        };
        Ok(flags_from_catalog(
            &columns,
            rows,
            "COLUMN_NAME",
            "COLUMN_COMMENT",
            "IS_AUTO",
            "IS_GENERATED",
        ))
    }

    /// Oracle identity (12c+)/virtual column flags and comments.
    fn oracle_column_flags(
        &self,
        database: &str,
        table: &str,
    ) -> Result<Vec<(String, String, bool, bool)>> {
        let (_, schema, bare) = catalog_parts(&self.engine, database, table);
        let schema_literal = quote_string(&schema, "oracle");
        let table_literal = quote_string(&bare, "oracle");
        let (columns, rows, _) = self.run(&format!(
            "SELECT c.COLUMN_NAME, NVL(m.COMMENTS, '') AS COLUMN_COMMENT, \
                    NVL(c.IDENTITY_COLUMN, 'NO') AS IS_AUTO, \
                    NVL(c.VIRTUAL_COLUMN, 'NO') AS IS_GENERATED \
             FROM ALL_TAB_COLUMNS c \
             LEFT JOIN ALL_COL_COMMENTS m \
               ON m.OWNER = c.OWNER AND m.TABLE_NAME = c.TABLE_NAME \
              AND m.COLUMN_NAME = c.COLUMN_NAME \
             WHERE c.OWNER = UPPER({schema_literal}) AND c.TABLE_NAME = UPPER({table_literal}) \
             ORDER BY c.COLUMN_ID"
        ))?;
        Ok(flags_from_catalog(
            &columns,
            rows,
            "COLUMN_NAME",
            "COLUMN_COMMENT",
            "IS_AUTO",
            "IS_GENERATED",
        ))
    }

    /// SQLite: a lone `INTEGER PRIMARY KEY` is a rowid alias and is filled in by the engine.
    /// (`PRAGMA table_info` does not report generated columns on older SQLite versions.)
    fn sqlite_column_flags(
        &self,
        _database: &str,
        table: &str,
    ) -> Result<Vec<(String, String, bool, bool)>> {
        let name = table.rsplit('.').next().unwrap_or(table);
        let literal = quote_string(name, "sqlite");
        let (columns, rows, _) = self.run(&format!("PRAGMA table_info({literal})"))?;
        let name_index = column_index(&columns, "name");
        let type_index = column_index(&columns, "type");
        let pk_index = column_index(&columns, "pk");
        let Some(name_index) = name_index else {
            return Ok(Vec::new());
        };
        let is_pk = |row: &[CellValue]| {
            pk_index
                .and_then(|index| row.get(index))
                .and_then(cell_int)
                .is_some_and(|value| value > 0)
        };
        let pk_rows: Vec<&Vec<CellValue>> = rows.iter().filter(|row| is_pk(row)).collect();
        let auto = if pk_rows.len() == 1 {
            let row = pk_rows[0];
            type_index
                .and_then(|index| row.get(index))
                .and_then(cell_text)
                .filter(|type_name| type_name.eq_ignore_ascii_case("INTEGER"))
                .and_then(|_| row.get(name_index))
                .and_then(cell_text)
                .map(str::to_string)
        } else {
            None
        };
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let name = row.get(name_index).and_then(cell_text)?;
                Some((
                    name.to_string(),
                    String::new(),
                    auto.as_deref().is_some_and(|auto| auto == name),
                    false,
                ))
            })
            .collect())
    }

    /// The `CREATE TRIGGER` statements of one MySQL/MariaDB table. `SHOW CREATE TABLE` does not
    /// include them, so a dump without this silently lost every trigger.
    fn mysql_triggers(&self, database: &str, table: &str) -> Result<Vec<String>> {
        if !matches!(
            self.engine.to_ascii_lowercase().as_str(),
            "mysql" | "mariadb"
        ) {
            return Ok(Vec::new());
        }
        let schema = quote_string(database, "mysql");
        let table = quote_string(table, "mysql");
        let (columns, rows, _) = self.run(&format!(
            "SELECT TRIGGER_NAME FROM information_schema.TRIGGERS \
             WHERE EVENT_OBJECT_SCHEMA = {schema} AND EVENT_OBJECT_TABLE = {table} \
             ORDER BY EVENT_MANIPULATION, ACTION_TIMING, ACTION_ORDER"
        ))?;
        let index = column_index(&columns, "TRIGGER_NAME");
        let mut triggers = Vec::new();
        for row in rows {
            let Some(CellValue::Text(name)) = index.and_then(|index| row.get(index)) else {
                continue;
            };
            let qualified = format!("{}.{}", self.quote(database), self.quote(name));
            let Ok((columns, rows, _)) = self.run(&format!("SHOW CREATE TRIGGER {qualified}"))
            else {
                continue;
            };
            let statement = column_index(&columns, "SQL Original Statement")
                .or_else(|| columns.len().checked_sub(1))
                .unwrap_or(0);
            if let Some(CellValue::Text(ddl)) = rows.first().and_then(|row| row.get(statement)) {
                triggers.push(ddl.clone());
            }
        }
        Ok(triggers)
    }

    /// Render one decoded value as a SQL literal for a dump.
    fn literal(&self, value: &CellValue) -> String {
        literal_for_engine(value, &self.engine)
    }

    /// The `DROP VIEW` that precedes a save, or `None` when the engine replaces the view itself.
    fn view_drop_sql(&self, database: &str, original: &str) -> Option<String> {
        if self.engine.eq_ignore_ascii_case("oracle") {
            // Oracle's `DBMS_METADATA` definition is `CREATE OR REPLACE`; dropping would
            // invalidate dependent views for no reason.
            return None;
        }
        let qualified = self.qualify(database, original);
        if self.engine.eq_ignore_ascii_case("sqlserver") {
            // `DROP VIEW IF EXISTS` needs SQL Server 2016; `OBJECT_ID` works on every version.
            return Some(format!(
                "IF OBJECT_ID(N'{}', N'V') IS NOT NULL DROP VIEW {qualified};",
                qualified.replace('\'', "''")
            ));
        }
        Some(format!("DROP VIEW IF EXISTS {qualified};"))
    }
}

impl Drop for OdbcConnection {
    fn drop(&mut self) {
        let Ok(api) = api::api() else {
            return;
        };
        let Ok(handle) = self.handle.get_mut() else {
            return;
        };
        if !handle.0.is_null() {
            api.disconnect(handle.0);
            api.free_connection(handle.0);
        }
    }
}

#[async_trait]
impl Connection for OdbcConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new("odbc")
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>> {
        // ODBC's `SQLTables` catalog list only reports the *current* catalog on several drivers —
        // SQL Server, and MySQL, which does not even fill `TABLE_CAT` in (so it reported no
        // database at all) — so those engines ask the server directly. The rest fall back to the
        // catalogs `SQLTables` exposes.
        let names: Vec<String> = match self.engine.to_ascii_lowercase().as_str() {
            "sqlserver" => self.first_column(
                "SELECT name FROM sys.databases WHERE database_id > 4 ORDER BY name",
            )?,
            "mysql" | "mariadb" => self.first_column("SHOW DATABASES")?,
            // Oracle has no catalogs; each schema is what the tree calls a database. `ALL_USERS`
            // lists them without scanning every object; fall back to `SQLTables` if it is not
            // readable.
            "oracle" => match self.first_column("SELECT username FROM all_users ORDER BY username")
            {
                Ok(names) if !names.is_empty() => names,
                _ => self.table_schemas()?,
            },
            _ => self.table_catalogs()?,
        };
        let names = if names.is_empty() {
            self.current_database()?.into_iter().collect()
        } else {
            names
        };
        Ok(names
            .into_iter()
            .map(|name| DatabaseInfo { name })
            .collect())
    }

    async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>> {
        let (catalog, schema) = if self.engine.eq_ignore_ascii_case("oracle") {
            (String::new(), database.to_string())
        } else if self.prefixes_catalog() {
            (database.to_string(), String::new())
        } else {
            (String::new(), String::new())
        };
        let (columns, rows) = self.with_handle(|api, dbc| {
            api.tables(dbc, &catalog, &schema, "", "")
                .map_err(Error::Query)
        })?;
        let name_index = column_index(&columns, "TABLE_NAME");
        let schema_index = column_index(&columns, "TABLE_SCHEM");
        let type_index = column_index(&columns, "TABLE_TYPE");

        let mut tables = Vec::new();
        let mut seen = BTreeSet::new();
        for row in rows {
            let Some(name) = name_index
                .and_then(|index| row.get(index))
                .and_then(cell_text)
            else {
                continue;
            };
            let table_type = type_index
                .and_then(|index| row.get(index))
                .and_then(cell_text);
            // `SQLTables` may also report system objects; they are not user objects. A driver that
            // calls its tables "BASE TABLE" (rather than "TABLE") is still a table.
            let kind = match table_type {
                Some(value) if value.eq_ignore_ascii_case("VIEW") => ObjectKind::View,
                Some(value)
                    if value.eq_ignore_ascii_case("SYSTEM TABLE")
                        || value.eq_ignore_ascii_case("SYSTEM VIEW")
                        || value.eq_ignore_ascii_case("ALIAS") =>
                {
                    continue;
                }
                _ => ObjectKind::Table,
            };
            let schema = schema_index
                .and_then(|index| row.get(index))
                .and_then(cell_text)
                .unwrap_or_default();
            let name = if schema.is_empty() {
                name.to_string()
            } else {
                format!("{schema}.{name}")
            };
            if seen.insert(name.clone()) {
                tables.push(TableInfo {
                    name,
                    kind,
                    updatable: false,
                });
            }
        }
        Ok(tables)
    }

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        let qualified = self.qualify(database, table);
        let (mut columns, _rows, _affected) =
            self.run(&format!("SELECT * FROM {qualified} WHERE 1 = 0"))?;
        // SQLDescribeCol does not report primary keys; the grid uses them as the row identity, so
        // read them from the driver's catalog. Without this every column was used in the WHERE
        // clause, and a text/blob column made updating (or deleting) the row fail.
        let (catalog, schema, bare) = catalog_parts(&self.engine, database, table);
        let mut has_key = false;
        if let Ok(keys) = self.with_handle(|api, dbc| {
            api.primary_keys(dbc, &catalog, &schema, &bare)
                .map_err(Error::Query)
        }) {
            for column in &mut columns {
                column.primary_key = keys
                    .iter()
                    .any(|key| self.same_identifier(key, &column.name));
            }
            has_key = columns.iter().any(|column| column.primary_key);
        }
        if !has_key {
            // No declared primary key: `SQLSpecialColumns` may still find a unique row identifier.
            // Pseudo columns (an Oracle `ROWID`) are skipped because `SELECT *` never returns one.
            if let Ok(keys) = self.with_handle(|api, dbc| {
                api.best_rowid(dbc, &catalog, &schema, &bare)
                    .map_err(Error::Query)
            }) {
                for column in &mut columns {
                    column.primary_key = keys
                        .iter()
                        .any(|key| self.same_identifier(key, &column.name));
                }
            }
        }
        // Identity/generated flags and comments live in the engine's catalog; without them a new
        // grid row would write into an AUTO_INCREMENT/IDENTITY column.
        if let Ok(flags) = self.engine_column_flags(database, table) {
            for column in &mut columns {
                if let Some((_, comment, auto_increment, generated)) = flags
                    .iter()
                    .find(|(name, ..)| self.same_identifier(name, &column.name))
                {
                    column.comment = comment.clone();
                    column.auto_increment = *auto_increment;
                    column.generated = *generated;
                }
            }
        }
        Ok(columns)
    }

    async fn fetch_page(
        &self,
        database: &str,
        table: &str,
        page: PageRequest,
    ) -> Result<TablePage> {
        let table = self.qualify(database, table);
        let page_size = page.page_size.max(1);
        let offset = page.offset();
        let quote = |name: &str| self.quote(name);
        let (where_sql, params) = filter_clause(&page.filter, &quote);
        let order_sql = order_clause(&page.order_by, &quote);
        let dialect = self.dialect();
        // `paging_clause` matches the engine's own syntax; engines that reject `OFFSET … FETCH`
        // without an `ORDER BY` (SQL Server, Oracle) get one even when the user did not sort.
        let mut sql = format!("SELECT * FROM {table}{where_sql}{order_sql}");
        if order_sql.is_empty() && dialect.paging_needs_order() {
            sql.push_str(" ORDER BY 1");
        }
        sql.push(' ');
        sql.push_str(&dialect.paging_clause(offset, page_size));

        let (columns, rows, _affected) = match self.run_prepared(&sql, &params) {
            Ok(result) => result,
            // The generic dialect uses `LIMIT`, which DB2 and some other ODBC engines reject;
            // retry with the SQL:2008 form before giving up.
            Err(error) if dialect == DriverDialect::Generic => {
                let alternate = format!(
                    "SELECT * FROM {table}{where_sql}{order_sql} \
                     OFFSET {offset} ROWS FETCH NEXT {page_size} ROWS ONLY"
                );
                self.run_prepared(&alternate, &params).map_err(|_| error)?
            }
            Err(error) => return Err(error),
        };

        let total_rows = if page.skip_total {
            None
        } else {
            let count_sql = format!("SELECT COUNT(*) FROM {table}{where_sql}");
            self.run_prepared(&count_sql, &params)
                .ok()
                .and_then(|(_columns, rows, _affected)| {
                    rows.first()
                        .and_then(|row| row.first())
                        .and_then(|value| value.as_display().trim().parse::<u64>().ok())
                })
        };

        Ok(TablePage {
            columns,
            rows,
            page: page.page,
            page_size,
            total_rows,
        })
    }

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()> {
        let table = self.qualify(database, table);
        for update in updates {
            let mut assignments = Vec::new();
            let mut params: Vec<Option<String>> = Vec::new();
            for (column, value) in &update.set {
                assignments.push(format!("{} = ?", self.quote(column)));
                params.push(value.clone());
            }
            if assignments.is_empty() {
                continue;
            }
            let mut conditions = Vec::new();
            for (column, value) in &update.keys {
                match value {
                    Some(_) => {
                        conditions.push(format!("{} = ?", self.quote(column)));
                        params.push(value.clone());
                    }
                    None => conditions.push(format!("{} IS NULL", self.quote(column))),
                }
            }
            if conditions.is_empty() {
                return Err(Error::Query(
                    "no key columns to identify the row".to_string(),
                ));
            }
            let sql = format!(
                "UPDATE {table} SET {} WHERE {}",
                assignments.join(", "),
                conditions.join(" AND ")
            );
            self.execute_script(&sql, &params)?;
        }
        Ok(())
    }

    async fn insert_rows(&self, database: &str, table: &str, rows: &[RowInsert]) -> Result<()> {
        let table = self.qualify(database, table);
        for insert in rows {
            if insert.values.is_empty() {
                continue;
            }
            let columns: Vec<String> = insert
                .values
                .iter()
                .map(|(column, _)| self.quote(column))
                .collect();
            let placeholders = vec!["?"; insert.values.len()].join(", ");
            let params: Vec<Option<String>> = insert
                .values
                .iter()
                .map(|(_, value)| value.clone())
                .collect();
            let sql = format!(
                "INSERT INTO {table} ({}) VALUES ({placeholders})",
                columns.join(", ")
            );
            self.execute_script(&sql, &params)?;
        }
        Ok(())
    }

    async fn delete_rows(
        &self,
        database: &str,
        table: &str,
        keys: &[Vec<(String, Option<String>)>],
    ) -> Result<()> {
        let table = self.qualify(database, table);
        for key in keys {
            let mut conditions = Vec::new();
            let mut params: Vec<Option<String>> = Vec::new();
            for (column, value) in key {
                match value {
                    Some(_) => {
                        conditions.push(format!("{} = ?", self.quote(column)));
                        params.push(value.clone());
                    }
                    None => conditions.push(format!("{} IS NULL", self.quote(column))),
                }
            }
            if conditions.is_empty() {
                continue;
            }
            let sql = format!("DELETE FROM {table} WHERE {}", conditions.join(" AND "));
            self.execute_script(&sql, &params)?;
        }
        Ok(())
    }

    async fn execute_query(&self, database: Option<&str>, sql: &str) -> Result<QueryResult> {
        let result = self.with_handle(|api, dbc| {
            if let Some(database) = database.filter(|database| !database.is_empty()) {
                self.use_database(api, dbc, database)?;
            }
            run_text(api, dbc, sql)
        })?;
        let (columns, rows, affected) = result;
        let has_result_set = !columns.is_empty();
        Ok(QueryResult {
            statement: sql.to_string(),
            columns,
            rows,
            rows_affected: affected.unwrap_or(0),
            has_result_set,
            last_insert_id: None,
        })
    }

    async fn create_database(&self, name: &str, options: &DatabaseOptions) -> Result<()> {
        self.run_statement(&self.create_database_sql(name, options))
    }

    fn create_database_sql(&self, name: &str, options: &DatabaseOptions) -> String {
        let mut sql = format!("CREATE DATABASE {}", self.quote(name));
        if matches!(
            self.engine.to_ascii_lowercase().as_str(),
            "mysql" | "mariadb"
        ) {
            if !options.charset.trim().is_empty() {
                sql.push_str(" CHARACTER SET ");
                sql.push_str(options.charset.trim());
            }
            if !options.collation.trim().is_empty() {
                sql.push_str(" COLLATE ");
                sql.push_str(options.collation.trim());
            }
        }
        sql
    }

    async fn drop_database(&self, name: &str) -> Result<()> {
        self.run_statement(&format!("DROP DATABASE {}", self.quote(name)))
    }

    async fn drop_table(&self, database: &str, table: &str) -> Result<()> {
        self.run_statement(&format!("DROP TABLE {}", self.qualify(database, table)))
    }

    async fn empty_table(&self, database: &str, table: &str) -> Result<()> {
        self.run_statement(&format!("DELETE FROM {}", self.qualify(database, table)))
    }

    async fn truncate_table(&self, database: &str, table: &str) -> Result<()> {
        // SQLite has no TRUNCATE.
        let sql = if self.engine.eq_ignore_ascii_case("sqlite") {
            format!("DELETE FROM {}", self.qualify(database, table))
        } else {
            format!("TRUNCATE TABLE {}", self.qualify(database, table))
        };
        self.run_statement(&sql)
    }

    async fn rename_table(&self, database: &str, table: &str, new_name: &str) -> Result<()> {
        self.run_statement(&self.rename_table_sql(database, table, new_name))
    }

    async fn database_options(&self, _name: &str) -> Result<DatabaseOptions> {
        Ok(DatabaseOptions::default())
    }

    async fn server_version(&self) -> Result<String> {
        Ok(self
            .with_handle(|api, dbc| Ok(api.dbms_version(dbc)))?
            .or_else(|| {
                self.with_handle(|api, dbc| Ok(api.dbms_name(dbc)))
                    .ok()
                    .flatten()
            })
            .unwrap_or_default())
    }

    async fn session_count(&self) -> Result<u64> {
        Ok(0)
    }

    async fn table_status(&self, database: &str, table: &str) -> Result<TableStatus> {
        Ok(self
            .table_statuses(database)
            .await?
            .into_iter()
            .find(|(name, _)| name == table)
            .map(|(_, status)| status)
            .unwrap_or_default())
    }

    async fn table_statuses(&self, database: &str) -> Result<Vec<(String, TableStatus)>> {
        if !matches!(
            self.engine.to_ascii_lowercase().as_str(),
            "mysql" | "mariadb"
        ) {
            return Ok(Vec::new());
        }
        let sql = if database.is_empty() {
            "SHOW TABLE STATUS".to_string()
        } else {
            format!("SHOW TABLE STATUS FROM {}", self.quote(database))
        };
        let (columns, rows, _affected) = self.run(&sql)?;
        let name_index = column_index(&columns, "Name");
        let engine_index = column_index(&columns, "Engine");
        let rows_index = column_index(&columns, "Rows");
        let auto_index = column_index(&columns, "Auto_increment");
        let format_index = column_index(&columns, "Row_format");
        let created_index = column_index(&columns, "Create_time");
        let updated_index = column_index(&columns, "Update_time");
        let checked_index = column_index(&columns, "Check_time");
        let index_length_index = column_index(&columns, "Index_length");
        let data_length_index = column_index(&columns, "Data_length");
        let max_data_length_index = column_index(&columns, "Max_data_length");
        let data_free_index = column_index(&columns, "Data_free");
        let collation_index = column_index(&columns, "Collation");
        let options_index = column_index(&columns, "Create_options");
        let comment_index = column_index(&columns, "Comment");
        let mut statuses = Vec::new();
        for row in rows {
            let Some(name) = name_index
                .and_then(|index| row.get(index))
                .and_then(cell_text)
            else {
                continue;
            };
            let text = |index: Option<usize>| {
                index
                    .and_then(|index| row.get(index))
                    .and_then(cell_text)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            };
            statuses.push((
                name.to_string(),
                TableStatus {
                    engine: text(engine_index),
                    rows: text(rows_index).and_then(|value| value.parse().ok()),
                    auto_increment: text(auto_index).and_then(|value| value.parse().ok()),
                    row_format: text(format_index),
                    created: text(created_index),
                    updated: text(updated_index),
                    checked: text(checked_index),
                    index_length: text(index_length_index).and_then(|value| value.parse().ok()),
                    data_length: text(data_length_index).and_then(|value| value.parse().ok()),
                    max_data_length: text(max_data_length_index)
                        .and_then(|value| value.parse().ok()),
                    data_free: text(data_free_index).and_then(|value| value.parse().ok()),
                    collation: text(collation_index),
                    create_options: text(options_index),
                    comment: text(comment_index),
                    ..TableStatus::default()
                },
            ));
        }
        Ok(statuses)
    }

    async fn character_sets(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn collations(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn alter_database_options(
        &self,
        _name: &str,
        _original: &DatabaseOptions,
        _modified: &DatabaseOptions,
    ) -> Result<()> {
        Err(not_supported("database management"))
    }

    fn alter_database_sql(
        &self,
        _name: &str,
        _original: &DatabaseOptions,
        _modified: &DatabaseOptions,
    ) -> String {
        String::new()
    }

    fn column_types(&self) -> Vec<&'static str> {
        vec![
            "integer",
            "bigint",
            "smallint",
            "tinyint",
            "decimal",
            "numeric",
            "real",
            "double precision",
            "boolean",
            "date",
            "time",
            "timestamp",
            "char",
            "varchar",
            "text",
            "binary",
            "varbinary",
            "blob",
        ]
    }

    async fn list_routines(&self, _database: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn list_events(&self, _database: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn object_ddl(
        &self,
        database: &str,
        name: &str,
        is_view: bool,
    ) -> Result<Option<String>> {
        if let Some(ddl) = self.native_ddl(database, name, is_view).await? {
            return Ok(Some(ddl));
        }
        if is_view {
            return Ok(None);
        }
        self.catalog_ddl(database, name)
    }

    async fn backup_object_metadata(
        &self,
        database: &str,
        kind: BackupObjectKind,
        name: &str,
    ) -> Result<ObjectDump> {
        let is_view = match kind {
            BackupObjectKind::Table => false,
            BackupObjectKind::View => true,
            BackupObjectKind::Function | BackupObjectKind::Event => {
                return Err(not_supported("backup"));
            }
        };
        let ddl = self
            .object_ddl(database, name, is_view)
            .await?
            .ok_or_else(|| not_supported("reading the object definition"))?;
        let fields = if is_view {
            Vec::new()
        } else {
            self.columns(database, name)
                .await?
                .into_iter()
                .map(|column| column.name)
                .collect()
        };
        let trigger_ddl = if !is_view {
            self.mysql_triggers(database, name).unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(ObjectDump {
            name: name.to_string(),
            kind,
            ddl,
            fields,
            trigger_ddl,
            rows: Vec::new(),
        })
    }

    async fn stream_table_rows(
        &self,
        database: &str,
        table: &str,
        on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<u64> {
        let qualified = self.qualify(database, table);
        let sql = format!("SELECT * FROM {qualified}");
        self.with_handle(|api, dbc| {
            let statement = api.execute(dbc, &sql).map_err(Error::Query)?;
            let mut failure: Option<Error> = None;
            let mut count = 0u64;
            let streamed = (|| -> Result<()> {
                let (_columns, types) = api.describe_columns(statement).map_err(Error::Query)?;
                api.stream_rows(statement, &types, &mut |values| {
                    let tuple = format!(
                        "({})",
                        values
                            .iter()
                            .map(|value| self.literal(value))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    match on_row(&tuple) {
                        Ok(()) => {
                            count += 1;
                            Ok(())
                        }
                        Err(error) => {
                            failure = Some(error);
                            Err("the row sink rejected a row".to_string())
                        }
                    }
                })
                .map_err(Error::Query)?;
                Ok(())
            })();
            api.free_statement(statement);
            if let Some(error) = failure {
                return Err(error);
            }
            streamed?;
            Ok(count)
        })
    }

    async fn restore_object(&self, database: &str, object: &ObjectDump) -> Result<()> {
        let verb = if object.kind == BackupObjectKind::View {
            "VIEW"
        } else {
            "TABLE"
        };
        let lower = self.engine.to_ascii_lowercase();
        let qualified = self.qualify(database, &object.name);
        if matches!(lower.as_str(), "oracle") {
            // Oracle has no `DROP ... IF EXISTS`; catch ORA-00942 (object does not exist) so a
            // restore into an empty schema still works, while a real failure (privileges) is
            // reported by the following `CREATE`.
            let literal = qualified.replace('\'', "''");
            let block = format!(
                "BEGIN EXECUTE IMMEDIATE 'DROP {verb} {literal}'; \
                 EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF; END;"
            );
            let _ = self.run_statement(&block);
        } else {
            let _ = self.run_statement(&format!("DROP {verb} IF EXISTS {qualified}"));
        }
        if !object.ddl.trim().is_empty() {
            // The DDL MySQL reports (`SHOW CREATE TABLE`) has no database prefix, so switch the
            // session first; otherwise it would create the object in the connection's default
            // database instead of the restore target.
            self.execute_query(Some(database), &object.ddl).await?;
        }
        if object.kind == BackupObjectKind::Table && !object.rows.is_empty() {
            if object.fields.is_empty() {
                return Err(not_supported("restoring rows without column names"));
            }
            let columns = object
                .fields
                .iter()
                .map(|column| self.quote(column))
                .collect::<Vec<_>>()
                .join(", ");
            let qualified = self.qualify(database, &object.name);
            for tuple in &object.rows {
                self.run_statement(&format!(
                    "INSERT INTO {qualified} ({columns}) VALUES {tuple}"
                ))?;
            }
        }
        Ok(())
    }

    fn storage_engines(&self) -> Vec<&'static str> {
        Vec::new()
    }

    async fn table_schema(&self, _database: &str, _table: &str) -> Result<TableSchema> {
        Err(not_supported("the table designer"))
    }

    fn table_schema_sql(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> String {
        // The designer is not offered for ODBC, but the Import Wizard creates tables through this
        // path, so a new table still gets a real `CREATE TABLE`.
        if original.is_some() {
            return String::new();
        }
        self.create_table_sql(database, table, modified)
    }

    async fn view_details(&self, database: &str, name: &str) -> Result<ViewDetails> {
        if !matches!(
            self.engine.to_ascii_lowercase().as_str(),
            "mysql" | "mariadb"
        ) {
            // SQL Server, PostgreSQL, SQLite and Oracle all have an authoritative `CREATE VIEW`
            // (object definition / pg_get_viewdef / sqlite_master / DBMS_METADATA); the advanced
            // fields stay empty but the editable definition is complete.
            let definition = self
                .native_ddl(database, name, true)
                .await?
                .ok_or_else(|| not_supported("view management"))?;
            return Ok(ViewDetails {
                info: ViewInfo {
                    name: name.to_string(),
                    ..ViewInfo::default()
                },
                definition,
                character_set_client: String::new(),
                collation_connection: String::new(),
            });
        }
        let qualified = self.qualify(database, name);
        let (columns, rows, _) = self.run(&format!("SHOW CREATE VIEW {qualified}"))?;
        let Some(row) = rows.into_iter().next() else {
            return Err(Error::Query(format!("view {name} has no definition")));
        };
        let definition = column_index(&columns, "Create View")
            .or_else(|| columns.len().checked_sub(3))
            .and_then(|index| row.get(index))
            .filter(|value| !matches!(value, CellValue::Null))
            .map(CellValue::as_display)
            .unwrap_or_default();
        if definition.is_empty() {
            return Err(Error::Query(format!("view {name} has no definition")));
        }
        let text = |name: &str| {
            column_index(&columns, name)
                .and_then(|index| row.get(index))
                .and_then(cell_text)
                .unwrap_or_default()
                .to_string()
        };
        let info = ViewInfo {
            name: name.to_string(),
            ..ViewInfo::default()
        };
        Ok(ViewDetails {
            info,
            definition,
            character_set_client: text("character_set_client"),
            collation_connection: text("collation_connection"),
        })
    }

    fn view_sql(&self, database: &str, original: Option<&str>, edit: &ViewEdit) -> String {
        let mut sql = String::new();
        if let Some(original) = original
            && let Some(drop) = self.view_drop_sql(database, original)
        {
            sql.push_str(&drop);
            sql.push('\n');
        }
        sql.push_str(edit.definition.trim().trim_end_matches(';').trim_end());
        sql.push(';');
        sql
    }

    async fn save_view(
        &self,
        database: &str,
        original: Option<&str>,
        edit: &ViewEdit,
    ) -> Result<()> {
        if let Some(original) = original
            && let Some(drop) = self.view_drop_sql(database, original)
        {
            let _ = self.run_statement(&drop);
        }
        self.execute_query(
            Some(database),
            edit.definition.trim().trim_end_matches(';').trim_end(),
        )
        .await?;
        Ok(())
    }

    async fn drop_view(&self, database: &str, name: &str) -> Result<()> {
        self.run_statement(&format!("DROP VIEW {}", self.qualify(database, name)))
    }

    async fn close(&self) -> Result<()> {
        let Ok(api) = api::api() else {
            return Ok(());
        };
        let Ok(mut handle) = self.handle.lock() else {
            return Err(Error::Connection(
                "the ODBC connection is poisoned".to_string(),
            ));
        };
        if !handle.0.is_null() {
            api.disconnect(handle.0);
            api.free_connection(handle.0);
            handle.0 = null_mut();
        }
        Ok(())
    }
}

/// Run `sql` through the text path.
#[allow(clippy::type_complexity)]
fn run_text(
    api: &api::OdbcApi,
    dbc: Hdbc,
    sql: &str,
) -> Result<(Vec<ColumnInfo>, Vec<Vec<CellValue>>, Option<u64>)> {
    let statement = api.execute(dbc, sql).map_err(Error::Query)?;
    let result = api.read_result(statement);
    let affected = api.rows_affected(statement);
    api.free_statement(statement);
    let (columns, rows) = result.map_err(Error::Query)?;
    Ok((columns, rows, affected))
}

/// Run `sql` with the given text binds. With no binds the text path is used, because some drivers
/// handle a prepared `SELECT` differently from an executed one.
#[allow(clippy::type_complexity)]
fn run_prepared(
    api: &api::OdbcApi,
    dbc: Hdbc,
    sql: &str,
    params: &[Option<String>],
) -> Result<(Vec<ColumnInfo>, Vec<Vec<CellValue>>, Option<u64>)> {
    if params.is_empty() {
        return run_text(api, dbc, sql);
    }
    let statement = api.prepare(dbc, sql).map_err(Error::Query)?;
    let executed = api.execute_bound(statement, params);
    let result = executed.and_then(|()| api.read_result(statement));
    let affected = api.rows_affected(statement);
    api.free_statement(statement);
    let (columns, rows) = result.map_err(Error::Query)?;
    Ok((columns, rows, affected))
}

/// The index of the column named `name` (case-insensitive).
fn column_index(columns: &[ColumnInfo], name: &str) -> Option<usize> {
    columns
        .iter()
        .position(|column| column.name.eq_ignore_ascii_case(name))
}

/// The text of a non-null cell.
fn text(value: &CellValue) -> Option<&str> {
    match value {
        CellValue::Text(text) => Some(text),
        _ => None,
    }
}

fn cell_text(value: &CellValue) -> Option<&str> {
    text(value)
}

fn cell_int(value: &CellValue) -> Option<i64> {
    match value {
        CellValue::Int(value) => Some(*value),
        CellValue::Uint(value) => i64::try_from(*value).ok(),
        CellValue::Float(value) => Some(*value as i64),
        CellValue::Text(value) => value.trim().parse().ok(),
        _ => None,
    }
}

/// Whether a catalog value means "yes": numeric 1 or a textual YES/TRUE.
fn flag_true(value: &CellValue) -> bool {
    match value {
        CellValue::Bool(value) => *value,
        CellValue::Int(value) => *value != 0,
        CellValue::Uint(value) => *value != 0,
        CellValue::Float(value) => *value != 0.0,
        CellValue::Text(value) => {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("yes") || value.eq_ignore_ascii_case("true")
        }
        CellValue::Null | CellValue::Bytes(_) => false,
    }
}

/// Build `(name, comment, auto_increment, generated)` entries from a catalog result set.
fn flags_from_catalog(
    columns: &[ColumnInfo],
    rows: Vec<Vec<CellValue>>,
    name: &str,
    comment: &str,
    auto_increment: &str,
    generated: &str,
) -> Vec<(String, String, bool, bool)> {
    let name_index = column_index(columns, name);
    let comment_index = column_index(columns, comment);
    let auto_index = column_index(columns, auto_increment);
    let generated_index = column_index(columns, generated);
    let Some(name_index) = name_index else {
        return Vec::new();
    };
    rows.into_iter()
        .filter_map(|row| {
            let name = row.get(name_index).and_then(cell_text)?.to_string();
            let comment = comment_index
                .and_then(|index| row.get(index))
                .and_then(cell_text)
                .unwrap_or_default()
                .to_string();
            let auto_increment = auto_index
                .and_then(|index| row.get(index))
                .is_some_and(flag_true);
            let generated = generated_index
                .and_then(|index| row.get(index))
                .is_some_and(flag_true);
            Some((name, comment, auto_increment, generated))
        })
        .collect()
}

fn not_supported(what: &str) -> Error {
    Error::Query(format!("{what} is not supported by the ODBC driver yet"))
}

/// Map the DBMS name reported by the driver to our engine hint.
pub(crate) fn detect_engine(dbms_name: &str) -> String {
    let name = dbms_name.to_ascii_lowercase();
    if name.contains("sql server") {
        "sqlserver"
    } else if name.contains("mariadb") {
        "mariadb"
    } else if name.contains("mysql") {
        "mysql"
    } else if name.contains("postgres") {
        "postgresql"
    } else if name.contains("oracle") {
        "oracle"
    } else if name.contains("sqlite") {
        "sqlite"
    } else {
        ""
    }
    .to_string()
}

/// The SQL dialect for an engine hint.
pub(crate) fn dialect_for_engine(engine: &str) -> DriverDialect {
    match engine.to_ascii_lowercase().as_str() {
        "mysql" | "mariadb" => DriverDialect::Mysql,
        "sqlserver" => DriverDialect::SqlServer,
        "sqlite" => DriverDialect::Sqlite,
        "postgresql" | "postgres" => DriverDialect::Postgres,
        "oracle" => DriverDialect::Oracle,
        _ => DriverDialect::Generic,
    }
}

/// `(identifier quote, catalog separator, SQL_IC_* case)` for the connection. `SQLGetInfo` is used
/// for every engine except SQL Server, whose reported `"` only works with `QUOTED_IDENTIFIER ON`;
/// T-SQL brackets always work.
fn quote_style(handle: Hdbc, engine: &str) -> ((char, char), char, u16) {
    let dialect = dialect_for_engine(engine);
    let Ok(api) = api::api() else {
        return (dialect.identifier_quote(), '.', 4);
    };
    let sqlserver = engine.eq_ignore_ascii_case("sqlserver");
    let quote = if sqlserver {
        dialect.identifier_quote()
    } else {
        api.identifier_quote_char(handle)
            .and_then(|value| {
                let mut characters = value.chars();
                let open = characters.next()?;
                // ODBC uses a space when the driver has no quoted-identifier character.
                if open.is_whitespace() {
                    return None;
                }
                let close = characters.next().unwrap_or(open);
                Some((open, close))
            })
            .unwrap_or_else(|| dialect.identifier_quote())
    };
    let catalog_separator = api.catalog_separator(handle).unwrap_or('.');
    let identifier_case = api.identifier_case(handle).unwrap_or(4);
    (quote, catalog_separator, identifier_case)
}

/// `(catalog, schema, bare_table)` for the driver's metadata functions.
fn catalog_parts(engine: &str, database: &str, table: &str) -> (String, String, String) {
    let prefix_catalog = !matches!(
        engine.to_ascii_lowercase().as_str(),
        "postgresql" | "postgres" | "oracle" | "sqlite"
    );
    let (schema, bare) = match table.rsplit_once('.') {
        Some((schema, bare)) => (schema.to_string(), bare.to_string()),
        None => (String::new(), table.to_string()),
    };
    if engine.eq_ignore_ascii_case("oracle") {
        return (String::new(), database.to_string(), bare);
    }
    if prefix_catalog {
        (database.to_string(), schema, bare)
    } else {
        (String::new(), schema, bare)
    }
}

/// Render a type name with its length/precision when the driver reports one.
fn column_type_sql(code: i16, type_label: &str, size: i64, digits: i64) -> String {
    let name = type_label.trim();
    let name = if name.is_empty() {
        type_name(code)
    } else {
        name
    };
    // Long character/binary types (TEXT, BLOB, CLOB, …) take no `(length)` in DDL.
    let takes_size = matches!(code, 1 | 2 | 3 | 12 | -2 | -3 | -8 | -9);
    if !takes_size || size <= 0 {
        return name.to_string();
    }
    if matches!(code, 2 | 3) && digits > 0 {
        return format!("{name}({size},{digits})");
    }
    format!("{name}({size})")
}

/// Render a designer column's type from its `data_type`/`length`/`decimals`.
fn column_type_from_def(column: &rustgrid_core::ColumnDef) -> String {
    let data_type = column.data_type.trim();
    let length = column.length.trim();
    let decimals = column.decimals.trim();
    if length.is_empty() || length == "0" {
        return data_type.to_string();
    }
    if !decimals.is_empty() && decimals != "0" {
        return format!("{data_type}({length},{decimals})");
    }
    format!("{data_type}({length})")
}

/// Build the ` WHERE ...` fragment for a page request's filter tree, together with the values to
/// bind in placeholder order.
fn filter_clause(
    filter: &[FilterNode],
    quote: &dyn Fn(&str) -> String,
) -> (String, Vec<Option<String>>) {
    let mut binds: Vec<Option<String>> = Vec::new();
    match filter_fragment(filter, quote, &mut binds) {
        Some(expr) => (format!(" WHERE {expr}"), binds),
        None => (String::new(), Vec::new()),
    }
}

/// Render one sibling list as an `AND`/`OR` joined expression, or `None` when nothing enabled is
/// left to filter on. Binds are appended in placeholder order; a skipped node never adds any.
fn filter_fragment(
    filter: &[FilterNode],
    quote: &dyn Fn(&str) -> String,
    binds: &mut Vec<Option<String>>,
) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();

    for node in filter {
        let piece = match node {
            FilterNode::Condition(condition) => condition_piece(condition, quote, binds),
            FilterNode::Group(group) => {
                if !group.enabled {
                    None
                } else {
                    filter_fragment(&group.children, quote, binds).map(|inner| format!("({inner})"))
                }
            }
        };
        let Some(piece) = piece else {
            continue;
        };

        if clauses.is_empty() {
            clauses.push(piece);
        } else {
            let conjunction = match node.conjunction() {
                FilterConjunction::And => "AND",
                FilterConjunction::Or => "OR",
            };
            clauses.push(format!("{conjunction} {piece}"));
        }
    }

    (!clauses.is_empty()).then(|| clauses.join(" "))
}

/// Render a single condition, or `None` when it is disabled/incomplete.
fn condition_piece(
    condition: &FilterCondition,
    quote: &dyn Fn(&str) -> String,
    binds: &mut Vec<Option<String>>,
) -> Option<String> {
    if !condition.enabled || condition.column.is_empty() {
        return None;
    }
    let operator = condition.operator;
    if operator.needs_value() && condition.value.is_empty() {
        return None;
    }
    if operator.needs_second_value() && condition.value2.is_empty() {
        return None;
    }

    let column = quote(&condition.column);
    let piece = match operator {
        FilterOperator::Equal => {
            binds.push(Some(condition.value.clone()));
            format!("{column} = ?")
        }
        FilterOperator::NotEqual => {
            binds.push(Some(condition.value.clone()));
            format!("{column} <> ?")
        }
        FilterOperator::LessThan => {
            binds.push(Some(condition.value.clone()));
            format!("{column} < ?")
        }
        FilterOperator::LessOrEqual => {
            binds.push(Some(condition.value.clone()));
            format!("{column} <= ?")
        }
        FilterOperator::GreaterThan => {
            binds.push(Some(condition.value.clone()));
            format!("{column} > ?")
        }
        FilterOperator::GreaterOrEqual => {
            binds.push(Some(condition.value.clone()));
            format!("{column} >= ?")
        }
        FilterOperator::Contains => {
            binds.push(Some(format!("%{}%", condition.value)));
            format!("{column} LIKE ?")
        }
        FilterOperator::NotContains => {
            binds.push(Some(format!("%{}%", condition.value)));
            format!("{column} NOT LIKE ?")
        }
        FilterOperator::StartsWith => {
            binds.push(Some(format!("{}%", condition.value)));
            format!("{column} LIKE ?")
        }
        FilterOperator::NotStartsWith => {
            binds.push(Some(format!("{}%", condition.value)));
            format!("{column} NOT LIKE ?")
        }
        FilterOperator::EndsWith => {
            binds.push(Some(format!("%{}", condition.value)));
            format!("{column} LIKE ?")
        }
        FilterOperator::NotEndsWith => {
            binds.push(Some(format!("%{}", condition.value)));
            format!("{column} NOT LIKE ?")
        }
        FilterOperator::IsNull => format!("{column} IS NULL"),
        FilterOperator::IsNotNull => format!("{column} IS NOT NULL"),
        FilterOperator::IsEmpty => format!("({column} IS NULL OR {column} = '')"),
        FilterOperator::IsNotEmpty => format!("({column} IS NOT NULL AND {column} <> '')"),
        FilterOperator::Between => {
            binds.push(Some(condition.value.clone()));
            binds.push(Some(condition.value2.clone()));
            format!("{column} BETWEEN ? AND ?")
        }
        FilterOperator::NotBetween => {
            binds.push(Some(condition.value.clone()));
            binds.push(Some(condition.value2.clone()));
            format!("{column} NOT BETWEEN ? AND ?")
        }
        FilterOperator::InList => {
            let values = condition.list_values();
            if values.is_empty() {
                return None;
            }
            let placeholders = vec!["?"; values.len()].join(", ");
            binds.extend(values.into_iter().map(Some));
            format!("{column} IN ({placeholders})")
        }
        FilterOperator::NotInList => {
            let values = condition.list_values();
            if values.is_empty() {
                return None;
            }
            let placeholders = vec!["?"; values.len()].join(", ");
            binds.extend(values.into_iter().map(Some));
            format!("{column} NOT IN ({placeholders})")
        }
    };

    Some(piece)
}

/// Build the ` ORDER BY ...` fragment for a page request.
fn order_clause(order_by: &[SortColumn], quote: &dyn Fn(&str) -> String) -> String {
    if order_by.is_empty() {
        return String::new();
    }
    let terms = order_by
        .iter()
        .map(|sort| {
            format!(
                "{} {}",
                quote(&sort.column),
                if sort.descending { "DESC" } else { "ASC" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(" ORDER BY {terms}")
}

/// Render one decoded cell as a SQL value literal.
fn literal_for_engine(value: &CellValue, engine: &str) -> String {
    match value {
        CellValue::Null => "NULL".to_string(),
        CellValue::Bool(value) => u8::from(*value).to_string(),
        CellValue::Int(value) => value.to_string(),
        CellValue::Uint(value) => value.to_string(),
        CellValue::Float(value) => {
            if value.is_finite() {
                value.to_string()
            } else {
                "NULL".to_string()
            }
        }
        CellValue::Text(text) => quote_string(text, engine),
        CellValue::Bytes(bytes) => {
            let mut hex = String::with_capacity(bytes.len() * 2 + 2);
            let engine = engine.to_ascii_lowercase();
            let hex_digits = |hex: &mut String| {
                for byte in bytes {
                    hex.push_str(&format!("{byte:02X}"));
                }
            };
            match engine.as_str() {
                "mysql" | "mariadb" | "sqlserver" => {
                    hex.push_str("0x");
                    hex_digits(&mut hex);
                }
                "postgresql" | "postgres" => {
                    hex.push_str("decode('");
                    hex_digits(&mut hex);
                    hex.push_str("', 'hex')");
                }
                "oracle" => {
                    hex.push_str("HEXTORAW('");
                    hex_digits(&mut hex);
                    hex.push_str("')");
                }
                _ => {
                    hex.push_str("X'");
                    hex_digits(&mut hex);
                    hex.push('\'');
                }
            }
            hex
        }
    }
}

/// Quote a string literal. MySQL also honours backslash escapes by default, so it gets its own
/// escaping; the other engines use the standard doubled quote.
fn quote_string(value: &str, engine: &str) -> String {
    let mysql = matches!(engine.to_ascii_lowercase().as_str(), "mysql" | "mariadb");
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for character in value.chars() {
        if mysql {
            match character {
                '\0' => out.push_str("\\0"),
                '\'' => out.push_str("\\'"),
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{1a}' => out.push_str("\\Z"),
                _ => out.push(character),
            }
        } else if character == '\'' {
            out.push_str("''");
        } else {
            out.push(character);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgrid_core::{ColumnDef, FilterCondition};

    #[test]
    fn catalog_qualification_follows_the_engine() {
        // MySQL and SQL Server accept a catalog qualifier; PostgreSQL, Oracle and SQLite do not.
        assert_eq!(
            catalog_parts("mysql", "shop", "users"),
            ("shop".to_string(), String::new(), "users".to_string())
        );
        assert_eq!(
            catalog_parts("sqlserver", "shop", "dbo.users"),
            ("shop".to_string(), "dbo".to_string(), "users".to_string())
        );
        assert_eq!(
            catalog_parts("postgresql", "shop", "public.users"),
            (String::new(), "public".to_string(), "users".to_string())
        );
        assert_eq!(
            catalog_parts("oracle", "SCOTT", "EMP"),
            (String::new(), "SCOTT".to_string(), "EMP".to_string())
        );
    }

    #[test]
    fn a_long_type_takes_no_length() {
        // MySQL's TEXT arrives as SQL_LONGVARCHAR with a 65535 size; `TEXT(65535)` is invalid.
        assert_eq!(column_type_sql(-1, "TEXT", 65535, 0), "TEXT");
        assert_eq!(column_type_sql(12, "VARCHAR", 255, 0), "VARCHAR(255)");
        assert_eq!(column_type_sql(3, "DECIMAL", 20, 6), "DECIMAL(20,6)");
    }

    #[test]
    fn the_filter_tree_builds_binds_in_order() {
        let mut first = FilterCondition::new("name");
        first.operator = FilterOperator::Contains;
        first.value = "ann".to_string();
        let mut second = FilterCondition::new("age");
        second.operator = FilterOperator::GreaterThan;
        second.value = "18".to_string();
        second.conjunction = FilterConjunction::Or;
        let (clause, binds) = filter_clause(
            &[FilterNode::Condition(first), FilterNode::Condition(second)],
            &|name| format!("`{name}`"),
        );
        assert_eq!(clause, " WHERE `name` LIKE ? OR `age` > ?");
        assert_eq!(
            binds,
            vec![Some("%ann%".to_string()), Some("18".to_string())]
        );
    }

    #[test]
    fn a_quote_doubles_inside_a_string_literal() {
        assert_eq!(quote_string("O'Brien", "postgresql"), "'O''Brien'");
        assert_eq!(quote_string("back\\slash", "mysql"), "'back\\\\slash'");
        assert_eq!(
            literal_for_engine(&CellValue::Text("中文".into()), "mysql"),
            "'中文'"
        );
        assert_eq!(
            literal_for_engine(&CellValue::Bool(true), "postgresql"),
            "1"
        );
        assert_eq!(
            literal_for_engine(&CellValue::Bytes(vec![0xDE, 0xAD]), "postgresql"),
            "decode('DEAD', 'hex')"
        );
        assert_eq!(
            literal_for_engine(&CellValue::Bytes(vec![0xDE, 0xAD]), "oracle"),
            "HEXTORAW('DEAD')"
        );
    }

    #[test]
    fn a_designer_column_keeps_its_length() {
        let column = ColumnDef {
            name: "name".to_string(),
            data_type: "varchar".to_string(),
            length: "255".to_string(),
            ..Default::default()
        };
        assert_eq!(column_type_from_def(&column), "varchar(255)");
        let column = ColumnDef {
            data_type: "longtext".to_string(),
            ..Default::default()
        };
        assert_eq!(column_type_from_def(&column), "longtext");
    }

    #[test]
    fn catalog_flags_accept_yes_and_numeric_flags() {
        let columns = vec![
            ColumnInfo {
                name: "COLUMN_NAME".to_string(),
                data_type: "varchar".to_string(),
                nullable: false,
                primary_key: false,
                auto_increment: false,
                generated: false,
                comment: String::new(),
            },
            ColumnInfo {
                name: "COLUMN_COMMENT".to_string(),
                data_type: "varchar".to_string(),
                nullable: true,
                primary_key: false,
                auto_increment: false,
                generated: false,
                comment: String::new(),
            },
            ColumnInfo {
                name: "IS_AUTO".to_string(),
                data_type: "varchar".to_string(),
                nullable: true,
                primary_key: false,
                auto_increment: false,
                generated: false,
                comment: String::new(),
            },
            ColumnInfo {
                name: "IS_GENERATED".to_string(),
                data_type: "varchar".to_string(),
                nullable: true,
                primary_key: false,
                auto_increment: false,
                generated: false,
                comment: String::new(),
            },
        ];
        let rows = vec![vec![
            CellValue::Text("id".to_string()),
            CellValue::Text("主键".to_string()),
            CellValue::Text("YES".to_string()),
            CellValue::Int(0),
        ]];
        assert_eq!(
            flags_from_catalog(
                &columns,
                rows,
                "COLUMN_NAME",
                "COLUMN_COMMENT",
                "IS_AUTO",
                "IS_GENERATED"
            ),
            vec![("id".to_string(), "主键".to_string(), true, false)]
        );
        assert!(flag_true(&CellValue::Text("1".to_string())));
        assert!(flag_true(&CellValue::Bool(true)));
        assert!(!flag_true(&CellValue::Text("NO".to_string())));
    }

    #[test]
    fn identifiers_follow_the_case_mode() {
        // `SQL_IC_SENSITIVE` keeps exact matches; every other mode falls back to ignoring case.
        let connection = |identifier_case| OdbcConnection {
            handle: Mutex::new(ConnHandle(std::ptr::null_mut())),
            engine: "mysql".to_string(),
            quote: ('`', '`'),
            catalog_separator: '.',
            identifier_case,
        };
        let sensitive = connection(3);
        assert!(sensitive.same_identifier("Name", "Name"));
        assert!(!sensitive.same_identifier("Name", "name"));
        let mixed = connection(4);
        assert!(mixed.same_identifier("Name", "name"));
        assert_eq!(mixed.quote("we`ird"), "`we``ird`");
    }
}
