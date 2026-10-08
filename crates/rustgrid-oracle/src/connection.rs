use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oracledb::{Connection as OracleRawConnection, ToDbValue};
use rustgrid_core::{
    BackupObjectKind, ColumnInfo, Connection, DatabaseInfo, DatabaseOptions, DriverId, Error,
    ObjectDump, ObjectKind, ObjectPrivilegeRow, PageRequest, PrivilegeCatalog, QueryResult, Result,
    RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind, RowInsert, RowUpdate, TableInfo,
    TablePage, TableSchema, TableStatus, UserAccount, UserDetails, UserEdit, UserEditSection,
    ViewDetails, ViewEdit,
};

use crate::helpers::{
    bind_expression, decode_cell, filter_clause, metadata_column, order_clause, prepare_statement,
    qualify, quote_identifier, resolve_object, returns_result_set, split_statements,
};

/// An Oracle connection. Oracle is a schema-first engine: one pooled connection can see every
/// schema of its service/PDB, so a single pool serves every operation. `oracledb` is synchronous,
/// so each operation runs inside a blocking task that checks a connection out of the pool.
pub struct OracleConnection {
    /// The pool. `oracledb::Pool` is not `Clone`, so it is shared through an `Arc`.
    pool: Arc<oracledb::Pool>,
    /// The schema that owns unqualified names (the session's `CURRENT_SCHEMA`).
    default_schema: String,
    /// Session initialization SQL run on every pooled connection.
    init_sql: String,
    /// Per-statement call timeout, when configured.
    query_timeout: Option<Duration>,
    /// The tunnel forwarding this connection, kept alive for as long as the connection is.
    _tunnel: Option<rustgrid_tunnel::Tunnel>,
}

impl OracleConnection {
    pub fn new(
        pool: Arc<oracledb::Pool>,
        default_schema: String,
        init_sql: String,
        query_timeout: Option<Duration>,
        tunnel: Option<rustgrid_tunnel::Tunnel>,
    ) -> Self {
        Self {
            pool,
            default_schema,
            init_sql,
            query_timeout,
            _tunnel: tunnel,
        }
    }

    pub(crate) fn default_schema_name(&self) -> &str {
        &self.default_schema
    }

    /// The pool, for the sibling modules that issue catalog queries.
    pub(crate) fn pool(&self) -> Arc<oracledb::Pool> {
        Arc::clone(&self.pool)
    }

    /// Run `f` on a blocking task with a pooled connection, applying the session settings first.
    pub(crate) async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&OracleRawConnection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let pool = Arc::clone(&self.pool);
        let init_sql = self.init_sql.clone();
        let query_timeout = self.query_timeout;
        tokio::task::spawn_blocking(move || {
            let connection = pool.acquire().map_err(map_query_error)?;
            if let Some(timeout) = query_timeout {
                connection
                    .set_call_timeout(Some(timeout))
                    .map_err(map_query_error)?;
            }
            if !init_sql.trim().is_empty() {
                run_statements(&connection, &split_statements(&init_sql), true)?;
            }
            f(&connection)
        })
        .await
        .map_err(|error| Error::Query(format!("oracle worker panicked: {error}")))?
    }

    /// Resolve a possibly schema-qualified name against the default schema.
    pub(crate) fn resolve(&self, name: &str) -> (String, String) {
        resolve_object(&self.default_schema, name)
    }

    /// Run one statement, returning its first result set (or its affected-row count).
    pub(crate) async fn run_text(&self, sql: &str) -> Result<QueryResult> {
        let sql = sql.to_string();
        self.with_conn(move |connection| {
            let mut results = run_statements(connection, &split_statements(&sql), true)?;
            if let Some(index) = results.iter().position(|result| result.has_result_set) {
                return Ok(results.swap_remove(index));
            }
            Ok(results.into_iter().next().unwrap_or_else(|| QueryResult {
                statement: sql.trim().to_string(),
                columns: Vec::new(),
                rows: Vec::new(),
                rows_affected: 0,
                has_result_set: false,
                last_insert_id: None,
            }))
        })
        .await
    }

    /// The scalar `i64` of a count-style query.
    async fn scalar_i64(&self, sql: &str, binds: Vec<String>) -> Result<i64> {
        let sql = sql.to_string();
        self.with_conn(move |connection| {
            let refs = bind_refs(&binds);
            let mut cursor = connection.query(&sql, &refs).map_err(map_query_error)?;
            if let Some(row) = cursor.next() {
                let row = row.map_err(map_query_error)?;
                if let Ok(Some(value)) = row.get::<Option<oracledb::OracleNumber>>(0)
                    && let Ok(value) = value.to_string().parse::<i64>()
                {
                    return Ok(value);
                }
                if let Ok(Some(value)) = row.get::<Option<String>>(0)
                    && let Ok(value) = value.parse::<i64>()
                {
                    return Ok(value);
                }
            }
            Ok(0)
        })
        .await
    }
}

/// Execute a set of already-split statements on one connection, decoding the result sets.
fn run_statements(
    connection: &OracleRawConnection,
    statements: &[String],
    commit: bool,
) -> Result<Vec<QueryResult>> {
    let mut results = Vec::with_capacity(statements.len());
    for statement in statements {
        let prepared = prepare_statement(statement);
        if prepared.is_empty() {
            continue;
        }
        let result = if returns_result_set(&prepared) {
            run_query(connection, &prepared, statement)?
        } else {
            let executed = connection
                .execute(&prepared, &[])
                .map_err(map_query_error)?;
            QueryResult {
                statement: statement.clone(),
                columns: Vec::new(),
                rows: Vec::new(),
                rows_affected: executed.rows_affected(),
                has_result_set: false,
                last_insert_id: None,
            }
        };
        results.push(result);
    }
    if commit {
        // Oracle commits DDL implicitly but not DML, so commit whatever ran.
        let _ = connection.commit();
    }
    Ok(results)
}

/// Run a (possibly multi-statement) script on one connection, discarding the results. Used by the
/// sibling modules that replay a `CREATE`/`DROP` script.
pub(crate) fn run_text_public(connection: &OracleRawConnection, sql: &str) -> Result<()> {
    run_statements(connection, &split_statements(sql), true).map(|_| ())
}

/// Run one query statement, decoding every row.
fn run_query(connection: &OracleRawConnection, sql: &str, statement: &str) -> Result<QueryResult> {
    let cursor = connection.query(sql, &[]).map_err(map_query_error)?;
    let columns: Vec<ColumnInfo> = cursor.columns().iter().map(metadata_column).collect();
    let mut rows = Vec::new();
    for row in cursor {
        let row = row.map_err(map_query_error)?;
        rows.push(
            (0..columns.len())
                .map(|index| decode_cell(&row, index))
                .collect(),
        );
    }
    Ok(QueryResult {
        statement: statement.to_string(),
        columns,
        rows,
        rows_affected: 0,
        has_result_set: true,
        last_insert_id: None,
    })
}

/// The `WHERE` fragment for a set of key columns, appending binds (a `None` key becomes `IS NULL`).
fn key_clause(
    keys: &[(String, Option<String>)],
    columns: &[ColumnInfo],
    binds: &mut Vec<String>,
) -> String {
    keys.iter()
        .map(|(column, value)| {
            let name = quote_identifier(column);
            match value {
                Some(value) => {
                    binds.push(value.clone());
                    let data_type = columns
                        .iter()
                        .find(|candidate| candidate.name == *column)
                        .map(|candidate| candidate.data_type.as_str())
                        .unwrap_or("");
                    format!("{name} = {}", bind_expression(binds.len(), data_type))
                }
                None => format!("{name} IS NULL"),
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Borrow a list of text binds as trait objects for `oracledb`.
pub(crate) fn bind_refs(binds: &[String]) -> Vec<&dyn ToDbValue> {
    binds.iter().map(|value| value as &dyn ToDbValue).collect()
}

/// Map an `oracledb` error to a Navicat-style message: `ORA-nnnnn - <message>` for server errors.
pub(crate) fn map_query_error(error: oracledb::Error) -> Error {
    if let oracledb::ErrorKind::DbError(db_error) = error.kind() {
        return Error::Query(format!(
            "ORA-{:05} - {}",
            db_error.code(),
            db_error.message()
        ));
    }
    Error::Query(error.to_string())
}

#[async_trait]
impl Connection for OracleConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new("oracle")
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>> {
        // Oracle has no database list; the tree shows the current service/PDB as the one database.
        let name = self
            .with_conn(|connection| {
                let row = connection
                    .query_row(
                        "SELECT COALESCE(SYS_CONTEXT('USERENV', 'CON_NAME'), \
                         SYS_CONTEXT('USERENV', 'DB_NAME')) FROM dual",
                        &[],
                    )
                    .map_err(map_query_error)?;
                Ok(row
                    .get::<Option<String>>(0)
                    .ok()
                    .flatten()
                    .filter(|value| !value.is_empty()))
            })
            .await?
            .unwrap_or_else(|| "ORCL".to_string());
        Ok(vec![DatabaseInfo { name }])
    }

    async fn list_schemas(&self, _database: &str) -> Result<Vec<String>> {
        self.with_conn(|connection| {
            let cursor = connection
                .query("SELECT username FROM all_users ORDER BY username", &[])
                .map_err(map_query_error)?;
            let mut schemas = Vec::new();
            for row in cursor {
                let row = row.map_err(map_query_error)?;
                if let Ok(Some(name)) = row.get::<Option<String>>(0)
                    && !is_maintenance_schema(&name)
                {
                    schemas.push(name);
                }
            }
            Ok(schemas)
        })
        .await
    }

    async fn create_schema(&self, _database: &str, schema: &str) -> Result<()> {
        // Oracle's schema is its user, and `Connection::create_schema` carries no password, so this
        // creates an authentication-less user (a pure schema).
        let sql = format!("CREATE USER {} NO AUTHENTICATION", quote_identifier(schema));
        self.run_text(&sql).await.map(|_| ())
    }

    async fn drop_schema(&self, _database: &str, schema: &str) -> Result<()> {
        // Deliberately no `CASCADE`: dropping a non-empty schema must fail with ORA-01922, matching
        // the trait's contract that engines reject dropping a non-empty schema.
        let sql = format!("DROP USER {}", quote_identifier(schema));
        self.run_text(&sql).await.map(|_| ())
    }

    async fn list_tables(&self, _database: &str) -> Result<Vec<TableInfo>> {
        self.with_conn(|connection| {
            let cursor = connection
                .query(
                    "SELECT owner, table_name, 'TABLE' FROM all_tables \
                     UNION ALL \
                     SELECT owner, view_name, 'VIEW' FROM all_views \
                     ORDER BY 1, 2",
                    &[],
                )
                .map_err(map_query_error)?;
            let mut tables = Vec::new();
            for row in cursor {
                let row = row.map_err(map_query_error)?;
                let owner = row
                    .get::<Option<String>>(0)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let name = row
                    .get::<Option<String>>(1)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let kind = row
                    .get::<Option<String>>(2)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                if owner.is_empty() || name.is_empty() {
                    continue;
                }
                let is_view = kind.eq_ignore_ascii_case("VIEW");
                tables.push(TableInfo {
                    name: format!("{owner}.{name}"),
                    kind: if is_view {
                        ObjectKind::View
                    } else {
                        ObjectKind::Table
                    },
                    updatable: false,
                });
            }
            Ok(tables)
        })
        .await
    }

    async fn columns(&self, _database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        let (schema, bare) = self.resolve(table);
        self.with_conn(move |connection| {
            let binds: Vec<String> = vec![schema.clone(), bare.clone()];
            let refs = bind_refs(&binds);
            let primary = primary_key_columns(connection, &schema, &bare)?;
            let cursor = connection
                .query(
                    "SELECT c.column_name, c.data_type, c.nullable, cc.comments \
                     FROM all_tab_columns c \
                     LEFT JOIN all_col_comments cc \
                       ON cc.owner = c.owner AND cc.table_name = c.table_name \
                      AND cc.column_name = c.column_name \
                     WHERE c.owner = :1 AND c.table_name = :2 \
                     ORDER BY c.column_id",
                    &refs,
                )
                .map_err(map_query_error)?;
            let mut columns = Vec::new();
            for row in cursor {
                let row = row.map_err(map_query_error)?;
                let name = row
                    .get::<Option<String>>(0)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let data_type = row
                    .get::<Option<String>>(1)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let nullable = row
                    .get::<Option<String>>(2)
                    .ok()
                    .flatten()
                    .map(|value| value.eq_ignore_ascii_case("Y"))
                    .unwrap_or(true);
                let comment = row
                    .get::<Option<String>>(3)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                columns.push(ColumnInfo {
                    primary_key: primary.iter().any(|column| column == &name),
                    name,
                    data_type: crate::helpers::display_type(&data_type),
                    nullable,
                    comment,
                });
            }
            Ok(columns)
        })
        .await
    }

    async fn fetch_page(
        &self,
        database: &str,
        table: &str,
        page: PageRequest,
    ) -> Result<TablePage> {
        let columns = self.columns(database, table).await?;
        let (schema, bare) = self.resolve(table);
        let qualified = qualify(&schema, &bare);
        let (where_clause, binds) = filter_clause(&page.filter, &columns);

        let total_rows = self
            .scalar_i64(
                &format!("SELECT count(*) FROM {qualified}{where_clause}"),
                binds.clone(),
            )
            .await
            .ok()
            .map(|value| value.max(0) as u64);

        let page_size = page.page_size.max(1);
        let offset = page.offset();
        let filter_sql = where_clause.clone();
        let page_binds = binds.clone();
        let columns_for_order = columns.clone();
        let explicit_order = order_clause(&page.order_by);

        let rows = self
            .with_conn(move |connection| {
                let order = if !explicit_order.is_empty() {
                    explicit_order.clone()
                } else {
                    let primary: Vec<String> = columns_for_order
                        .iter()
                        .filter(|column| column.primary_key)
                        .map(|column| quote_identifier(&column.name))
                        .collect();
                    if !primary.is_empty() {
                        format!(" ORDER BY {}", primary.join(", "))
                    } else if has_rowid(connection, &qualified) {
                        " ORDER BY ROWID".to_string()
                    } else {
                        // Views and rowid-less objects: a positional order keeps `OFFSET/FETCH`
                        // legal (the page is stable only within one query, which is documented).
                        " ORDER BY 1".to_string()
                    }
                };
                let sql = format!(
                    "SELECT * FROM {qualified}{filter_sql}{order} \
                     OFFSET {offset} ROWS FETCH NEXT {page_size} ROWS ONLY"
                );
                let refs = bind_refs(&page_binds);
                let cursor = connection.query(&sql, &refs).map_err(map_query_error)?;
                let mut rows = Vec::new();
                for row in cursor {
                    let row = row.map_err(map_query_error)?;
                    rows.push(
                        (0..columns_for_order.len())
                            .map(|index| decode_cell(&row, index))
                            .collect(),
                    );
                }
                Ok(rows)
            })
            .await?;

        Ok(TablePage {
            columns,
            rows,
            page: page.page,
            page_size,
            total_rows,
        })
    }

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let columns = self.columns(database, table).await?;
        let (schema, bare) = self.resolve(table);
        let qualified = qualify(&schema, &bare);
        let updates = updates.to_vec();
        self.with_conn(move |connection| {
            for update in &updates {
                if update.set.is_empty() {
                    continue;
                }
                let mut binds: Vec<String> = Vec::new();
                let assignments = update
                    .set
                    .iter()
                    .map(|(column, value)| {
                        let name = quote_identifier(column);
                        match value {
                            Some(value) => {
                                binds.push(value.clone());
                                let data_type = column_type(&columns, column);
                                format!("{name} = {}", bind_expression(binds.len(), data_type))
                            }
                            None => format!("{name} = NULL"),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let where_clause = key_clause(&update.keys, &columns, &mut binds);
                let sql = format!("UPDATE {qualified} SET {assignments} WHERE {where_clause}");
                let refs = bind_refs(&binds);
                connection.execute(&sql, &refs).map_err(map_query_error)?;
            }
            connection.commit().map_err(map_query_error)?;
            Ok(())
        })
        .await
    }

    async fn insert_rows(&self, database: &str, table: &str, rows: &[RowInsert]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let columns = self.columns(database, table).await?;
        let (schema, bare) = self.resolve(table);
        let qualified = qualify(&schema, &bare);
        let rows = rows.to_vec();
        self.with_conn(move |connection| {
            for row in &rows {
                if row.values.is_empty() {
                    connection
                        .execute(&format!("INSERT INTO {qualified} DEFAULT VALUES"), &[])
                        .map_err(map_query_error)?;
                    continue;
                }
                let names = row
                    .values
                    .iter()
                    .map(|(column, _)| quote_identifier(column))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut binds: Vec<String> = Vec::new();
                let placeholders = row
                    .values
                    .iter()
                    .map(|(column, value)| {
                        let data_type = column_type(&columns, column);
                        match value {
                            Some(value) => {
                                binds.push(value.clone());
                                bind_expression(binds.len(), data_type)
                            }
                            // An explicit NULL is emitted literally, so no value is bound for it.
                            None => "NULL".to_string(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!("INSERT INTO {qualified} ({names}) VALUES ({placeholders})");
                let refs = bind_refs(&binds);
                connection.execute(&sql, &refs).map_err(map_query_error)?;
            }
            connection.commit().map_err(map_query_error)?;
            Ok(())
        })
        .await
    }

    async fn delete_rows(
        &self,
        database: &str,
        table: &str,
        keys: &[Vec<(String, Option<String>)>],
    ) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let columns = self.columns(database, table).await?;
        let (schema, bare) = self.resolve(table);
        let qualified = qualify(&schema, &bare);
        let keys = keys.to_vec();
        self.with_conn(move |connection| {
            for row_keys in &keys {
                if row_keys.is_empty() {
                    return Err(Error::Query("delete requires key columns".to_string()));
                }
                let mut binds: Vec<String> = Vec::new();
                let where_clause = key_clause(row_keys, &columns, &mut binds);
                let sql = format!("DELETE FROM {qualified} WHERE {where_clause}");
                let refs = bind_refs(&binds);
                connection.execute(&sql, &refs).map_err(map_query_error)?;
            }
            connection.commit().map_err(map_query_error)?;
            Ok(())
        })
        .await
    }

    async fn execute_query(&self, _database: Option<&str>, sql: &str) -> Result<QueryResult> {
        self.run_text(sql).await
    }

    async fn execute_query_many(
        &self,
        _database: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        let statements = split_statements(sql);
        if statements.is_empty() {
            return Ok(Vec::new());
        }
        self.with_conn(move |connection| run_statements(connection, &statements, true))
            .await
    }

    async fn create_database(&self, _name: &str, _options: &DatabaseOptions) -> Result<()> {
        Err(Error::Query(
            "Oracle does not support creating databases from a client connection".to_string(),
        ))
    }

    fn create_database_sql(&self, _name: &str, _options: &DatabaseOptions) -> String {
        String::new()
    }

    async fn drop_database(&self, _name: &str) -> Result<()> {
        Err(Error::Query(
            "Oracle does not support dropping databases from a client connection".to_string(),
        ))
    }

    async fn drop_table(&self, _database: &str, table: &str) -> Result<()> {
        let (schema, bare) = self.resolve(table);
        let sql = format!("DROP TABLE {}", qualify(&schema, &bare));
        self.run_text(&sql).await.map(|_| ())
    }

    async fn empty_table(&self, _database: &str, table: &str) -> Result<()> {
        let (schema, bare) = self.resolve(table);
        let sql = format!("DELETE FROM {}", qualify(&schema, &bare));
        self.run_text(&sql).await.map(|_| ())
    }

    async fn truncate_table(&self, _database: &str, table: &str) -> Result<()> {
        let (schema, bare) = self.resolve(table);
        let sql = format!("TRUNCATE TABLE {}", qualify(&schema, &bare));
        self.run_text(&sql).await.map(|_| ())
    }

    async fn rename_table(&self, _database: &str, table: &str, new_name: &str) -> Result<()> {
        let (schema, bare) = self.resolve(table);
        let new_bare = crate::helpers::split_qualified(new_name)
            .map(|(_, bare)| bare)
            .unwrap_or(new_name);
        let sql = format!(
            "ALTER TABLE {} RENAME TO {}",
            qualify(&schema, &bare),
            quote_identifier(new_bare)
        );
        self.run_text(&sql).await.map(|_| ())
    }

    async fn database_options(&self, _name: &str) -> Result<DatabaseOptions> {
        Ok(DatabaseOptions::default())
    }

    async fn server_version(&self) -> Result<String> {
        self.with_conn(|connection| {
            let mut cursor = connection
                .query("SELECT banner FROM v$version WHERE rownum = 1", &[])
                .or_else(|_| {
                    connection.query(
                        "SELECT version FROM product_component_version WHERE rownum = 1",
                        &[],
                    )
                })
                .map_err(map_query_error)?;
            if let Some(row) = cursor.next() {
                let row = row.map_err(map_query_error)?;
                return Ok(row
                    .get::<Option<String>>(0)
                    .ok()
                    .flatten()
                    .unwrap_or_default());
            }
            Ok(String::new())
        })
        .await
    }

    async fn session_count(&self) -> Result<u64> {
        // `v$session` needs a privilege most users lack; a failure means "unknown", reported as 0.
        let count = self
            .with_conn(|connection| {
                let cursor = connection
                    .query("SELECT COUNT(*) FROM v$session", &[])
                    .map_err(map_query_error)?;
                for row in cursor {
                    let row = row.map_err(map_query_error)?;
                    if let Ok(Some(value)) = row.get::<Option<oracledb::OracleNumber>>(0)
                        && let Ok(value) = value.to_string().parse::<u64>()
                    {
                        return Ok(value);
                    }
                }
                Ok(0)
            })
            .await
            .unwrap_or(0);
        Ok(count)
    }

    async fn table_status(&self, _database: &str, table: &str) -> Result<TableStatus> {
        let (schema, bare) = self.resolve(table);
        self.with_conn(move |connection| table_status_for(connection, &schema, &bare))
            .await
    }

    async fn table_statuses(&self, _database: &str) -> Result<Vec<(String, TableStatus)>> {
        self.with_conn(|connection| {
            let cursor = connection
                .query(
                    "SELECT t.owner, t.table_name, t.num_rows, c.comments \
                     FROM all_tables t \
                     LEFT JOIN all_tab_comments c \
                       ON c.owner = t.owner AND c.table_name = t.table_name \
                     ORDER BY t.owner, t.table_name",
                    &[],
                )
                .map_err(map_query_error)?;
            let mut statuses = Vec::new();
            for row in cursor {
                let row = row.map_err(map_query_error)?;
                let owner = row
                    .get::<Option<String>>(0)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let name = row
                    .get::<Option<String>>(1)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                if owner.is_empty() || name.is_empty() {
                    continue;
                }
                statuses.push((
                    format!("{owner}.{name}"),
                    TableStatus {
                        rows: number_from_row(&row, 2),
                        comment: row.get::<Option<String>>(3).ok().flatten(),
                        ..Default::default()
                    },
                ));
            }
            Ok(statuses)
        })
        .await
    }

    async fn object_ddl(
        &self,
        _database: &str,
        name: &str,
        is_view: bool,
    ) -> Result<Option<String>> {
        let (schema, bare) = self.resolve(name);
        self.with_conn(move |connection| {
            let object_type = if is_view { "VIEW" } else { "TABLE" };
            fetch_ddl(connection, object_type, &bare, &schema)
        })
        .await
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
        Err(Error::Query(
            "Oracle database options cannot be altered from a client connection".to_string(),
        ))
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
        ORACLE_COLUMN_TYPES.to_vec()
    }

    fn storage_engines(&self) -> Vec<&'static str> {
        Vec::new()
    }

    async fn list_routines(&self, database: &str) -> Result<Vec<String>> {
        crate::routine::list_routines(self, database).await
    }

    async fn list_events(&self, _database: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn backup_object_metadata(
        &self,
        database: &str,
        kind: BackupObjectKind,
        name: &str,
    ) -> Result<ObjectDump> {
        crate::backup::backup_object_metadata(self, database, kind, name).await
    }

    async fn stream_table_rows(
        &self,
        database: &str,
        table: &str,
        on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<u64> {
        crate::backup::stream_table_rows(self, database, table, on_row).await
    }

    async fn restore_object(&self, database: &str, object: &ObjectDump) -> Result<()> {
        crate::backup::restore_object(self, database, object).await
    }

    async fn table_schema(&self, database: &str, table: &str) -> Result<TableSchema> {
        crate::schema::table_schema(self, database, table).await
    }

    fn table_schema_sql(
        &self,
        _database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> String {
        let (schema, bare) = self.resolve(table);
        crate::schema::table_schema_sql(&schema, &bare, original, modified)
    }

    async fn close(&self) -> Result<()> {
        // `oracledb::Pool::close` needs `&mut self`, which an `async fn close(&self)` behind an
        // `Arc` cannot provide. Dropping the last `Arc` closes the pool (its `Drop` calls `close`),
        // so there is nothing to do here.
        Ok(())
    }

    // ----- Account management (implemented in `user.rs`) ----------------------------------------

    async fn list_users(&self) -> Result<Vec<UserAccount>> {
        crate::user::list_users(self).await
    }

    async fn user_details(&self, user: &str, _host: &str) -> Result<UserDetails> {
        crate::user::user_details(self, user).await
    }

    fn user_edit_sql(&self, edit: &UserEdit) -> String {
        crate::user::edit_sql(edit)
    }

    fn user_edit_groups(&self, edit: &UserEdit) -> Vec<(UserEditSection, Vec<String>)> {
        crate::user::edit_groups(edit)
    }

    async fn save_user(&self, edit: &UserEdit) -> Result<()> {
        crate::user::save_user(self, edit).await
    }

    async fn drop_user(&self, user: &str, _host: &str) -> Result<()> {
        crate::user::drop_user(self, user).await
    }

    async fn rename_user(
        &self,
        _user: &str,
        _host: &str,
        _new_user: &str,
        _new_host: &str,
    ) -> Result<()> {
        // Oracle has no `RENAME USER`; renaming needs creating a user and migrating its objects.
        Err(Error::Query(
            "Oracle does not support renaming a user".to_string(),
        ))
    }

    fn privilege_catalog(&self) -> PrivilegeCatalog {
        crate::user::privilege_catalog()
    }

    fn authentication_plugins(&self) -> Vec<&'static str> {
        Vec::new()
    }

    fn ssl_types(&self) -> Vec<&'static str> {
        Vec::new()
    }

    async fn object_privilege_matrix(
        &self,
        database: &str,
        name: &str,
    ) -> Result<Vec<ObjectPrivilegeRow>> {
        crate::user::object_privilege_matrix(self, database, name).await
    }

    async fn set_object_privileges(
        &self,
        database: &str,
        name: &str,
        rows: &[ObjectPrivilegeRow],
    ) -> Result<()> {
        crate::user::set_object_privileges(self, database, name, rows).await
    }

    fn object_privileges_sql(
        &self,
        database: &str,
        name: &str,
        original: &[ObjectPrivilegeRow],
        rows: &[ObjectPrivilegeRow],
    ) -> String {
        crate::user::object_privileges_sql(database, name, original, rows)
    }

    // ----- Stored routines (implemented in `routine.rs`) ----------------------------------------

    async fn list_routine_infos(&self, database: &str) -> Result<Vec<RoutineInfo>> {
        crate::routine::list_routine_infos(self, database).await
    }

    async fn routine_details(
        &self,
        database: &str,
        kind: RoutineKind,
        name: &str,
    ) -> Result<RoutineDetails> {
        crate::routine::routine_details(self, database, kind, name).await
    }

    fn routine_sql(
        &self,
        _database: &str,
        original: Option<(&str, RoutineKind)>,
        edit: &RoutineEdit,
    ) -> String {
        crate::routine::routine_sql(original, edit)
    }

    async fn save_routine(
        &self,
        database: &str,
        original: Option<(&str, RoutineKind)>,
        edit: &RoutineEdit,
    ) -> Result<()> {
        crate::routine::save_routine(self, database, original, edit).await
    }

    async fn drop_routine(&self, database: &str, kind: RoutineKind, name: &str) -> Result<()> {
        crate::routine::drop_routine(self, database, kind, name).await
    }

    // ----- Views (implemented in `view.rs`) -----------------------------------------------------

    async fn view_details(&self, database: &str, name: &str) -> Result<ViewDetails> {
        crate::view::view_details(self, database, name).await
    }

    fn view_sql(&self, _database: &str, original: Option<&str>, edit: &ViewEdit) -> String {
        crate::view::view_sql(original, edit)
    }

    async fn save_view(
        &self,
        database: &str,
        original: Option<&str>,
        edit: &ViewEdit,
    ) -> Result<()> {
        crate::view::save_view(self, database, original, edit).await
    }

    async fn drop_view(&self, database: &str, name: &str) -> Result<()> {
        crate::view::drop_view(self, database, name).await
    }
}

/// Fetch `DBMS_METADATA.GET_DDL` for one object, after cleaning the transform so the script omits
/// storage/tablespace details. Returns `None` when the object has no DDL (or the call errors).
pub(crate) fn fetch_ddl(
    connection: &OracleRawConnection,
    object_type: &str,
    name: &str,
    schema: &str,
) -> Result<Option<String>> {
    let _ = connection.execute(
        "BEGIN \
           DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'STORAGE', FALSE); \
           DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'TABLESPACE', FALSE); \
           DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'SEGMENT_ATTRIBUTES', FALSE); \
           DBMS_METADATA.SET_TRANSFORM_PARAM(DBMS_METADATA.SESSION_TRANSFORM, 'PRETTY', TRUE); \
         END;",
        &[],
    );
    let binds: Vec<String> = vec![
        object_type.to_string(),
        name.to_string(),
        schema.to_string(),
    ];
    let refs = bind_refs(&binds);
    let mut cursor = connection
        .query("SELECT DBMS_METADATA.GET_DDL(:1, :2, :3) FROM dual", &refs)
        .map_err(map_query_error)?;
    if let Some(row) = cursor.next() {
        let row = row.map_err(map_query_error)?;
        if let Ok(Some(ddl)) = row.get::<Option<String>>(0)
            && !ddl.trim().is_empty()
        {
            return Ok(Some(ddl.trim().to_string()));
        }
    }
    Ok(None)
}

/// The primary-key column names of a table, in key order.
pub(crate) fn primary_key_columns(
    connection: &OracleRawConnection,
    schema: &str,
    table: &str,
) -> Result<Vec<String>> {
    let binds: Vec<String> = vec![schema.to_string(), table.to_string()];
    let refs = bind_refs(&binds);
    let cursor = connection
        .query(
            "SELECT cc.column_name \
             FROM all_constraints c \
             JOIN all_cons_columns cc \
               ON c.owner = cc.owner AND c.constraint_name = cc.constraint_name \
             WHERE c.constraint_type = 'P' AND c.owner = :1 AND c.table_name = :2 \
             ORDER BY cc.position",
            &refs,
        )
        .map_err(map_query_error)?;
    let mut columns = Vec::new();
    for row in cursor {
        let row = row.map_err(map_query_error)?;
        if let Ok(Some(name)) = row.get::<Option<String>>(0) {
            columns.push(name);
        }
    }
    Ok(columns)
}

/// Whether `SELECT ROWID` works for the object (false for views and rowid-less objects).
fn has_rowid(connection: &OracleRawConnection, qualified: &str) -> bool {
    connection
        .query(
            &format!("SELECT ROWID FROM {qualified} WHERE ROWNUM = 1"),
            &[],
        )
        .is_ok()
}

/// A table's estimated row count, data length and comment, for the object info pane.
fn table_status_for(
    connection: &OracleRawConnection,
    schema: &str,
    table: &str,
) -> Result<TableStatus> {
    let binds: Vec<String> = vec![schema.to_string(), table.to_string()];
    let refs = bind_refs(&binds);
    let mut cursor = connection
        .query(
            "SELECT t.num_rows, c.comments, s.bytes \
             FROM all_tables t \
             LEFT JOIN all_tab_comments c \
               ON c.owner = t.owner AND c.table_name = t.table_name \
             LEFT JOIN all_segments s \
               ON s.owner = t.owner AND s.segment_name = t.table_name \
             WHERE t.owner = :1 AND t.table_name = :2",
            &refs,
        )
        .map_err(map_query_error)?;
    if let Some(row) = cursor.next() {
        let row = row.map_err(map_query_error)?;
        return Ok(TableStatus {
            rows: number_from_row(&row, 0),
            comment: row.get::<Option<String>>(1).ok().flatten(),
            data_length: number_from_row(&row, 2),
            ..Default::default()
        });
    }
    Ok(TableStatus::default())
}

/// Read a NUMBER-ish column as a `u64` (Oracle reports counts/bytes as `NUMBER`).
fn number_from_row(row: &oracledb::Row, index: usize) -> Option<u64> {
    if let Ok(Some(value)) = row.get::<Option<oracledb::OracleNumber>>(index)
        && let Ok(value) = value.to_string().parse::<u64>()
    {
        return Some(value);
    }
    if let Ok(Some(value)) = row.get::<Option<String>>(index)
        && let Ok(value) = value.parse::<u64>()
    {
        return Some(value);
    }
    None
}

fn column_type<'a>(columns: &'a [ColumnInfo], name: &str) -> &'a str {
    columns
        .iter()
        .find(|column| column.name == name)
        .map(|column| column.data_type.as_str())
        .unwrap_or("")
}

/// The Oracle maintenance accounts hidden from the connection tree (mirrors Navicat's default).
fn is_maintenance_schema(name: &str) -> bool {
    const HIDDEN: &[&str] = &[
        "SYS",
        "SYSTEM",
        "XDB",
        "MDSYS",
        "CTXSYS",
        "ORDSYS",
        "ORDDATA",
        "WMSYS",
        "OUTLN",
        "DBSNMP",
        "APPQOSSYS",
        "ANONYMOUS",
        "XS$NULL",
        "DVSYS",
        "LBACSYS",
        "OJVMSYS",
        "AUDSYS",
        "GSMADMIN_INTERNAL",
        "DBSFWUSER",
        "GGSYS",
        "REMOTE_SCHEDULER_AGENT",
        "SI_INFORMTN_SCHEMA",
        "SYSBACKUP",
        "SYSDG",
        "SYSKM",
        "SYSRAC",
        "SYS$UMF",
        "OLAPSYS",
        "MDDATA",
        "SPATIAL_CSW_ADMIN_USR",
        "SPATIAL_WFS_ADMIN_USR",
    ];
    HIDDEN.contains(&name.to_ascii_uppercase().as_str())
}

/// The Oracle types offered by the table designer's type list. The first entry is the default for a
/// newly added column (`app/design.rs` falls back to the first list item).
const ORACLE_COLUMN_TYPES: [&str; 24] = [
    "NUMBER",
    "INTEGER",
    "FLOAT",
    "BINARY_FLOAT",
    "BINARY_DOUBLE",
    "VARCHAR2",
    "NVARCHAR2",
    "CHAR",
    "NCHAR",
    "CLOB",
    "NCLOB",
    "BLOB",
    "RAW",
    "LONG",
    "DATE",
    "TIMESTAMP",
    "TIMESTAMP WITH TIME ZONE",
    "TIMESTAMP WITH LOCAL TIME ZONE",
    "INTERVAL YEAR TO MONTH",
    "INTERVAL DAY TO SECOND",
    "ROWID",
    "XMLTYPE",
    "JSON",
    "BOOLEAN",
];
