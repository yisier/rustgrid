use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::MutexGuard;

use async_trait::async_trait;
use rustgrid_core::{
    BackupObjectKind, CellValue, ColumnInfo, Connection, ConnectionConfig, DatabaseInfo,
    DatabaseOptions, DriverId, Error, ObjectDump, ObjectKind, PageRequest, QueryResult, Result,
    RowInsert, RowUpdate, TableInfo, TablePage, TableSchema, TableStatus,
};

use crate::api::{self, Hdbc};

/// A raw ODBC connection handle. It is only ever touched behind [`OdbcConnection`]'s `Mutex`, so it
/// is safe to move between threads.
struct ConnHandle(Hdbc);

// SAFETY: the handle is only accessed through the `Mutex<ConnHandle>`; a raw pointer is just an
// address and carries no thread affinity of its own here.
unsafe impl Send for ConnHandle {}

/// A connection backed by one runtime-loaded ODBC connection handle. ODBC is synchronous and the
/// handle is not `Sync`, so it is guarded by a `Mutex` and each operation runs on the calling thread.
pub struct OdbcConnection {
    handle: Mutex<ConnHandle>,
    /// The underlying engine hint (`odbc.engine`), used to pick paging syntax.
    engine: String,
}

impl OdbcConnection {
    pub(crate) fn new(handle: Hdbc, config: &ConnectionConfig) -> Self {
        let mut engine = config
            .options
            .get("odbc.engine")
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
        // When the user did not set the engine hint, infer it from the connected DBMS name so
        // paging, renaming and database listing pick the right dialect.
        if engine.is_empty()
            && let Ok(api) = api::api()
            && let Some(name) = api.dbms_name(handle)
        {
            engine = detect_engine(&name);
        }
        Self {
            handle: Mutex::new(ConnHandle(handle)),
            engine,
        }
    }

    fn handle(&self) -> Result<MutexGuard<'_, ConnHandle>> {
        self.handle
            .lock()
            .map_err(|_| Error::Connection("the ODBC connection is poisoned".to_string()))
    }

    /// Run one statement, returning `(columns, rows, rows_affected)`. `columns` is non-empty only
    /// when the statement produced a result set.
    #[allow(clippy::type_complexity)]
    fn run(&self, sql: &str) -> Result<(Vec<ColumnInfo>, Vec<Vec<CellValue>>, Option<u64>)> {
        let api = api::api().map_err(Error::Connection)?;
        let dbc = self.handle()?.0;
        let statement = api.execute(dbc, sql).map_err(Error::Query)?;
        let result = api.read_result(statement);
        let affected = api.rows_affected(statement);
        api.free_statement(statement);
        let (columns, rows) = result.map_err(Error::Query)?;
        Ok((columns, rows, affected))
    }

    /// Quote an identifier for the target engine.
    fn quote(&self, name: &str) -> String {
        match self.engine.to_ascii_lowercase().as_str() {
            "sqlserver" => format!("[{}]", name.replace(']', "]]")),
            "mysql" | "mariadb" => format!("`{}`", name.replace('`', "``")),
            _ => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    /// A fully-qualified, quoted object name: `database` (when given) plus the dotted `table`.
    fn qualify(&self, database: &str, table: &str) -> String {
        let mut parts = Vec::new();
        if !database.is_empty() {
            parts.push(self.quote(database));
        }
        for part in table.split('.') {
            if !part.is_empty() {
                parts.push(self.quote(part));
            }
        }
        parts.join(".")
    }

    /// Prepare `sql` with `?` placeholders, bind `params` (text; `None` is SQL NULL), and execute.
    fn execute_script(&self, sql: &str, params: &[Option<String>]) -> Result<Option<u64>> {
        let api = api::api().map_err(Error::Connection)?;
        let dbc = self.handle()?.0;
        let statement = api.prepare(dbc, sql).map_err(Error::Query)?;
        let executed = api.execute_bound(statement, params);
        let affected = api.rows_affected(statement);
        api.free_statement(statement);
        executed.map_err(Error::Query)?;
        Ok(affected)
    }

    /// Run a statement that returns no result set.
    fn run_statement(&self, sql: &str) -> Result<()> {
        self.run(sql).map(|_| ())
    }

    /// The `RENAME` statement for the target engine.
    fn rename_table_sql(&self, table: &str, new_name: &str) -> String {
        match self.engine.to_ascii_lowercase().as_str() {
            "sqlserver" => format!(
                "EXEC sp_rename N'{}', N'{}'",
                table.replace('\'', "''"),
                new_name.replace('\'', "''")
            ),
            "mysql" | "mariadb" => format!("RENAME TABLE {table} TO {new_name}"),
            _ => format!("ALTER TABLE {table} RENAME TO {new_name}"),
        }
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
        // SQL Server's ODBC `SQLTables` only reports the current catalog, so list every database
        // explicitly; other engines fall back to the catalogs `SQLTables` exposes.
        if self.engine.eq_ignore_ascii_case("sqlserver") {
            let (_columns, rows, _affected) =
                self.run("SELECT name FROM sys.databases ORDER BY name")?;
            let mut names = Vec::new();
            for row in rows {
                if let Some(CellValue::Text(name)) = row.first() {
                    names.push(DatabaseInfo { name: name.clone() });
                }
            }
            return Ok(names);
        }

        let api = api::api().map_err(Error::Connection)?;
        let dbc = self.handle()?.0;
        let (columns, rows) = api.tables(dbc, "", "", "", "").map_err(Error::Query)?;
        let index = column_index(&columns, "TABLE_CAT");
        let mut names = BTreeSet::new();
        for row in rows {
            if let Some(CellValue::Text(name)) = index.and_then(|index| row.get(index))
                && !name.is_empty()
            {
                names.insert(name.clone());
            }
        }
        Ok(names
            .into_iter()
            .map(|name| DatabaseInfo { name })
            .collect())
    }

    async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>> {
        let api = api::api().map_err(Error::Connection)?;
        let dbc = self.handle()?.0;
        let (columns, rows) = api
            .tables(dbc, database, "", "", "")
            .map_err(Error::Query)?;
        let name_index = column_index(&columns, "TABLE_NAME");
        let schema_index = column_index(&columns, "TABLE_SCHEM");
        let type_index = column_index(&columns, "TABLE_TYPE");

        let mut tables = Vec::new();
        for row in rows {
            let Some(name) = name_index.and_then(|index| row.get(index)).and_then(text) else {
                continue;
            };
            let schema = schema_index
                .and_then(|index| row.get(index))
                .and_then(text)
                .unwrap_or_default();
            let kind = match type_index.and_then(|index| row.get(index)).and_then(text) {
                Some(table_type) if table_type.eq_ignore_ascii_case("VIEW") => ObjectKind::View,
                _ => ObjectKind::Table,
            };
            let name = if schema.is_empty() {
                name.to_string()
            } else {
                format!("{schema}.{name}")
            };
            tables.push(TableInfo {
                name,
                kind,
                updatable: false,
            });
        }
        Ok(tables)
    }

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        let table = self.qualify(database, table);
        let (columns, _rows, _affected) =
            self.run(&format!("SELECT * FROM {table} WHERE 1 = 0"))?;
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
        let sql = if self.engine.eq_ignore_ascii_case("sqlserver") {
            format!(
                "SELECT * FROM {table} ORDER BY (SELECT NULL) OFFSET {offset} ROWS \
                 FETCH NEXT {page_size} ROWS ONLY"
            )
        } else {
            format!("SELECT * FROM {table} LIMIT {page_size} OFFSET {offset}")
        };
        let (columns, rows, _affected) = self.run(&sql)?;
        Ok(TablePage {
            columns,
            rows,
            page: page.page,
            page_size,
            total_rows: None,
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

    async fn execute_query(&self, _database: Option<&str>, sql: &str) -> Result<QueryResult> {
        let (columns, rows, affected) = self.run(sql)?;
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

    fn create_database_sql(&self, name: &str, _options: &DatabaseOptions) -> String {
        format!("CREATE DATABASE {}", self.quote(name))
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
        self.run_statement(&format!("TRUNCATE TABLE {}", self.qualify(database, table)))
    }

    async fn rename_table(&self, _database: &str, table: &str, new_name: &str) -> Result<()> {
        self.run_statement(&self.rename_table_sql(table, new_name))
    }

    async fn database_options(&self, _name: &str) -> Result<DatabaseOptions> {
        Ok(DatabaseOptions::default())
    }

    async fn server_version(&self) -> Result<String> {
        let api = api::api().map_err(Error::Connection)?;
        let dbc = self.handle()?.0;
        Ok(api.dbms_name(dbc).unwrap_or_default())
    }

    async fn session_count(&self) -> Result<u64> {
        Ok(0)
    }

    async fn table_status(&self, _database: &str, _table: &str) -> Result<TableStatus> {
        Ok(TableStatus::default())
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

    async fn backup_object_metadata(
        &self,
        _database: &str,
        _kind: BackupObjectKind,
        _name: &str,
    ) -> Result<ObjectDump> {
        Err(not_supported("backup"))
    }

    async fn stream_table_rows(
        &self,
        _database: &str,
        _table: &str,
        _on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<u64> {
        Err(not_supported("backup"))
    }

    async fn restore_object(&self, _database: &str, _object: &ObjectDump) -> Result<()> {
        Err(not_supported("restore"))
    }

    fn storage_engines(&self) -> Vec<&'static str> {
        Vec::new()
    }

    async fn table_schema(&self, _database: &str, _table: &str) -> Result<TableSchema> {
        Err(not_supported("the table designer"))
    }

    fn table_schema_sql(
        &self,
        _database: &str,
        _table: &str,
        _original: Option<&TableSchema>,
        _modified: &TableSchema,
    ) -> String {
        String::new()
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
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

fn not_supported(what: &str) -> Error {
    Error::Query(format!("{what} is not supported by the ODBC driver yet"))
}

/// Map the DBMS name reported by the driver to our engine hint.
fn detect_engine(dbms_name: &str) -> String {
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
