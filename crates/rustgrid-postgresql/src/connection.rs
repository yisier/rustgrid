use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::TryStreamExt;
use rustgrid_core::{
    BackupObjectKind, CellValue, ColumnInfo, Connection, DatabaseInfo, DatabaseOptions, DriverId,
    Error, ObjectDump, ObjectKind, ObjectPrivilegeRow, PageRequest, QueryResult, Result,
    RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind, RowInsert, RowUpdate, TableInfo,
    TablePage, TableSchema, TableStatus, UserAccount, UserDetails, UserEdit, UserEditSection,
    ViewDetails, ViewEdit,
};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{AssertSqlSafe, Column, Either, Executor, Row, SqlSafeStr, Statement, TypeInfo};

use crate::helpers::{
    cell_from_format_type, decode_text_cell, filter_clause, order_clause, qualify,
    quote_identifier, split_qualified, split_statements,
};

/// The largest number of lazily-created (non-default) per-database pools kept alive at once.
const MAX_SHARDED_POOLS: usize = 16;

/// A PostgreSQL connection. PostgreSQL is connection-scoped to one database, so this type keeps a
/// lazily-populated pool per database and routes every operation to the right one.
pub struct PostgresConnection {
    /// Pool template: host/port/user/password/TLS/timeouts/session settings.
    options: sqlx::postgres::PgConnectOptions,
    /// The profile's default database (falling back to `postgres`).
    default_database: String,
    /// Per-database pools, created on demand.
    pools: Arc<tokio::sync::Mutex<HashMap<String, PgPool>>>,
    /// The pool options used to create sharded pools.
    pool_options: PgPoolOptions,
    /// Cache of `current_schema()` per database, used to resolve unqualified names.
    schemas: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    /// The tunnel forwarding this connection, kept alive for as long as the connection is.
    _tunnel: Option<rustgrid_tunnel::Tunnel>,
}

impl PostgresConnection {
    pub fn new(
        options: sqlx::postgres::PgConnectOptions,
        default_database: String,
        default_pool: PgPool,
        pool_options: PgPoolOptions,
        tunnel: Option<rustgrid_tunnel::Tunnel>,
    ) -> Self {
        let mut pools = HashMap::new();
        pools.insert(default_database.clone(), default_pool);
        Self {
            options,
            default_database,
            pools: Arc::new(tokio::sync::Mutex::new(pools)),
            pool_options,
            schemas: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            _tunnel: tunnel,
        }
    }

    /// The pool for `database` (or the default database when `None`/empty), created on demand.
    async fn pool(&self, database: Option<&str>) -> Result<PgPool> {
        let target = database
            .map(str::trim)
            .filter(|database| !database.is_empty())
            .unwrap_or(&self.default_database)
            .to_string();

        {
            let pools = self.pools.lock().await;
            if let Some(pool) = pools.get(&target) {
                return Ok(pool.clone());
            }
        }

        let options = self.options.clone().database(&target);
        let builder = if target == self.default_database {
            self.pool_options.clone()
        } else {
            // Sharded pools stay small and short-lived so expanding many databases in the tree
            // does not exhaust the server's `max_connections`.
            self.pool_options
                .clone()
                .max_connections(2)
                .idle_timeout(Duration::from_secs(60))
        };
        let pool = builder
            .connect_with(options)
            .await
            .map_err(map_query_error)?;

        let mut pools = self.pools.lock().await;
        if pools.len() >= MAX_SHARDED_POOLS
            && !pools.contains_key(&target)
            && let Some(key) = pools
                .keys()
                .find(|key| key.as_str() != self.default_database)
                .cloned()
        {
            pools.remove(&key);
        }
        pools.insert(target, pool.clone());
        Ok(pool)
    }

    /// The pool for a database, exposed to the sibling modules that issue catalog queries.
    pub(crate) async fn pool_for(&self, database: &str) -> Result<PgPool> {
        self.pool(Some(database)).await
    }

    /// The profile's default database name (roles are server-wide, so most account queries use it).
    pub(crate) fn default_database_name(&self) -> &str {
        &self.default_database
    }

    /// The schema that owns unqualified names in `database` (its `current_schema()`), cached.
    pub(crate) async fn default_schema(&self, database: &str) -> Result<String> {
        {
            let schemas = self.schemas.lock().await;
            if let Some(schema) = schemas.get(database) {
                return Ok(schema.clone());
            }
        }
        let pool = self.pool(Some(database)).await?;
        let row = sqlx::query("SELECT current_schema()")
            .fetch_one(&pool)
            .await
            .map_err(map_query_error)?;
        let schema: Option<String> = row.try_get(0).unwrap_or(None);
        let schema = schema
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "public".to_string());
        self.schemas
            .lock()
            .await
            .insert(database.to_string(), schema.clone());
        Ok(schema)
    }

    /// Resolve a possibly schema-qualified object name into `(schema, bare_name)`.
    pub(crate) async fn resolve_object(
        &self,
        database: &str,
        name: &str,
    ) -> Result<(String, String)> {
        if let Some((schema, bare)) = split_qualified(name) {
            return Ok((schema.to_string(), bare.to_string()));
        }
        Ok((self.default_schema(database).await?, name.to_string()))
    }

    /// The `relkind` of an object (`r` table, `p` partitioned table, `v` view, `m` materialized
    /// view, `f` foreign table), if it exists.
    pub(crate) async fn relkind(
        &self,
        database: &str,
        schema: &str,
        name: &str,
    ) -> Result<Option<char>> {
        let pool = self.pool(Some(database)).await?;
        let row = sqlx::query(
            "SELECT c.relkind FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2",
        )
        .bind(schema)
        .bind(name)
        .fetch_optional(&pool)
        .await
        .map_err(map_query_error)?;
        Ok(row.and_then(|row| {
            row.try_get::<String, _>(0)
                .ok()
                .and_then(|value| value.chars().next())
        }))
    }

    /// Run one statement on a pinned connection and return its first result set (or its row count).
    pub(crate) async fn run_text(&self, database: Option<&str>, sql: &str) -> Result<QueryResult> {
        let mut results = self.execute_query_many(database, sql).await?;
        if let Some(index) = results.iter().position(|result| result.has_result_set) {
            return Ok(results.swap_remove(index));
        }
        let rows_affected = results.iter().map(|result| result.rows_affected).sum();
        let statement = results
            .last()
            .map(|result| result.statement.clone())
            .unwrap_or_else(|| sql.trim().to_string());
        Ok(QueryResult {
            statement,
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected,
            has_result_set: false,
            last_insert_id: None,
        })
    }

    /// The scalar count of a query with bound values.
    async fn scalar_i64(&self, database: &str, sql: &str, binds: &[String]) -> Result<i64> {
        let pool = self.pool(Some(database)).await?;
        let mut query = sqlx::query(AssertSqlSafe(sql.to_string()));
        for bind in binds {
            query = query.bind(bind);
        }
        let row = query.fetch_one(&pool).await.map_err(map_query_error)?;
        row.try_get::<i64, _>(0).map_err(map_query_error)
    }
}

#[async_trait]
impl Connection for PostgresConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new("postgresql")
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>> {
        let pool = self.pool(None).await?;
        let rows = sqlx::query(
            "SELECT datname FROM pg_database \
             WHERE datistemplate = false AND datallowconn \
             ORDER BY datname",
        )
        .fetch_all(&pool)
        .await
        .map_err(map_query_error)?;
        let mut databases = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            databases.push(DatabaseInfo { name });
        }
        Ok(databases)
    }

    async fn list_schemas(&self, database: &str) -> Result<Vec<String>> {
        let pool = self.pool(Some(database)).await?;
        let rows = sqlx::query(
            "SELECT nspname FROM pg_namespace \
             WHERE nspname NOT LIKE 'pg\\_%' AND nspname <> 'information_schema' \
             ORDER BY nspname",
        )
        .fetch_all(&pool)
        .await
        .map_err(map_query_error)?;
        rows.iter()
            .map(|row| row.try_get(0).map_err(map_query_error))
            .collect()
    }

    async fn create_schema(&self, database: &str, schema: &str) -> Result<()> {
        let sql = format!("CREATE SCHEMA {}", quote_identifier(schema));
        self.run_text(Some(database), &sql).await.map(|_| ())
    }

    async fn drop_schema(&self, database: &str, schema: &str) -> Result<()> {
        let sql = format!("DROP SCHEMA {}", quote_identifier(schema));
        self.run_text(Some(database), &sql).await.map(|_| ())
    }

    async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>> {
        // Query the catalog rather than `information_schema.tables`, which only lists objects the
        // current user has privileges on. Names are returned `schema.name`.
        let pool = self.pool(Some(database)).await?;
        let rows = sqlx::query(
            "SELECT n.nspname, c.relname, c.relkind, \
                    COALESCE(v.is_updatable, 'NO') \
             FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN information_schema.views v \
               ON v.table_schema = n.nspname AND v.table_name = c.relname \
             WHERE n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema' \
               AND c.relkind IN ('r','p','v','m','f') \
             ORDER BY n.nspname, c.relname",
        )
        .fetch_all(&pool)
        .await
        .map_err(map_query_error)?;

        let mut tables = Vec::with_capacity(rows.len());
        for row in rows {
            let schema: String = row.try_get(0).map_err(map_query_error)?;
            let name: String = row.try_get(1).map_err(map_query_error)?;
            let relkind: String = row.try_get(2).map_err(map_query_error)?;
            let updatable: String = row.try_get(3).unwrap_or_default();
            let kind = match relkind.as_str() {
                "v" | "m" | "f" => ObjectKind::View,
                _ => ObjectKind::Table,
            };
            tables.push(TableInfo {
                name: format!("{schema}.{name}"),
                kind,
                updatable: kind == ObjectKind::View && updatable.eq_ignore_ascii_case("YES"),
            });
        }
        Ok(tables)
    }

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        let (schema, table) = self.resolve_object(database, table).await?;
        let pool = self.pool(Some(database)).await?;
        let rows = sqlx::query(
            "SELECT a.attname, \
                    format_type(a.atttypid, a.atttypmod) AS data_type, \
                    NOT a.attnotnull AS nullable, \
                    col_description(a.attrelid, a.attnum) AS comment, \
                    EXISTS (SELECT 1 FROM pg_index i \
                            WHERE i.indrelid = a.attrelid AND i.indisprimary \
                              AND a.attnum = ANY(i.indkey)) AS primary_key \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2 \
               AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
        )
        .bind(&schema)
        .bind(&table)
        .fetch_all(&pool)
        .await
        .map_err(map_query_error)?;

        let mut columns = Vec::with_capacity(rows.len());
        for row in &rows {
            columns.push(ColumnInfo {
                name: row.try_get(0).map_err(map_query_error)?,
                data_type: row.try_get(1).map_err(map_query_error)?,
                nullable: row.try_get(2).unwrap_or(true),
                primary_key: row.try_get(4).unwrap_or(false),
                comment: row.try_get(3).unwrap_or(None).unwrap_or_default(),
            });
        }
        Ok(columns)
    }

    async fn fetch_page(
        &self,
        database: &str,
        table: &str,
        page: PageRequest,
    ) -> Result<TablePage> {
        let columns = self.columns(database, table).await?;
        let (schema, bare) = self.resolve_object(database, table).await?;
        let qualified = qualify(&schema, &bare);
        let (where_clause, binds) = filter_clause(&page.filter, &columns);

        let total_rows = self
            .scalar_i64(
                database,
                &format!("SELECT count(*) FROM {qualified}{where_clause}"),
                &binds,
            )
            .await
            .ok()
            .map(|value| value.max(0) as u64);

        // Project every column to text so the binary protocol path (which is taken because the
        // filter is bound) yields readable `numeric`/`uuid`/`json`/array values.
        let projection = if columns.is_empty() {
            "*".to_string()
        } else {
            columns
                .iter()
                .map(|column| {
                    let name = quote_identifier(&column.name);
                    format!("{name}::text AS {name}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        };

        let order = if !page.order_by.is_empty() {
            order_clause(&page)
        } else {
            let primary: Vec<String> = columns
                .iter()
                .filter(|column| column.primary_key)
                .map(|column| quote_identifier(&column.name))
                .collect();
            if !primary.is_empty() {
                format!(" ORDER BY {}", primary.join(", "))
            } else {
                // A stable order without a primary key: base tables, partitioned tables and
                // materialized views have `ctid`; views and foreign tables do not.
                match self.relkind(database, &schema, &bare).await? {
                    Some('r') | Some('p') | Some('m') => " ORDER BY ctid".to_string(),
                    _ => String::new(),
                }
            }
        };

        let sql = format!(
            "SELECT {projection} FROM {qualified}{where_clause}{order} LIMIT {} OFFSET {}",
            page.page_size,
            page.offset()
        );
        let pool = self.pool(Some(database)).await?;
        let mut query = sqlx::query(AssertSqlSafe(sql));
        for bind in &binds {
            query = query.bind(bind);
        }
        let rows = query.fetch_all(&pool).await.map_err(map_query_error)?;

        let mut decoded = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut values = Vec::with_capacity(columns.len());
            for (index, column) in columns.iter().enumerate() {
                let value = row
                    .try_get::<Option<String>, _>(index)
                    .unwrap_or(None)
                    .map(|text| cell_from_format_type(&column.data_type, text))
                    .unwrap_or(CellValue::Null);
                values.push(value);
            }
            decoded.push(values);
        }

        Ok(TablePage {
            columns,
            rows: decoded,
            page: page.page,
            page_size: page.page_size,
            total_rows,
        })
    }

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let columns = self.columns(database, table).await?;
        let (schema, bare) = self.resolve_object(database, table).await?;
        let qualified = qualify(&schema, &bare);
        let pool = self.pool(Some(database)).await?;
        let mut transaction = pool.begin().await.map_err(map_query_error)?;

        for update in updates {
            if update.set.is_empty() {
                continue;
            }
            let mut binds: Vec<Option<String>> = Vec::new();
            let assignments = update
                .set
                .iter()
                .map(|(column, value)| {
                    let name = quote_identifier(column);
                    match value {
                        Some(value) => {
                            binds.push(Some(value.clone()));
                            format!("{name} = {}", cast(&binds.len(), column, &columns))
                        }
                        None => format!("{name} = NULL"),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            let where_clause = key_clause(&update.keys, &columns, &mut binds);
            let sql = format!("UPDATE {qualified} SET {assignments} WHERE {where_clause}");
            let mut query = sqlx::query(AssertSqlSafe(sql));
            for bind in &binds {
                query = query.bind(bind.clone());
            }
            query
                .execute(&mut *transaction)
                .await
                .map_err(map_query_error)?;
        }

        transaction.commit().await.map_err(map_query_error)?;
        Ok(())
    }

    async fn insert_rows(&self, database: &str, table: &str, rows: &[RowInsert]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let columns = self.columns(database, table).await?;
        let (schema, bare) = self.resolve_object(database, table).await?;
        let qualified = qualify(&schema, &bare);
        let pool = self.pool(Some(database)).await?;
        let mut transaction = pool.begin().await.map_err(map_query_error)?;

        for row in rows {
            if row.values.is_empty() {
                let sql = format!("INSERT INTO {qualified} DEFAULT VALUES");
                sqlx::query(AssertSqlSafe(sql))
                    .execute(&mut *transaction)
                    .await
                    .map_err(map_query_error)?;
                continue;
            }
            let names = row
                .values
                .iter()
                .map(|(column, _)| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ");
            let mut binds: Vec<Option<String>> = Vec::new();
            let placeholders = row
                .values
                .iter()
                .map(|(column, value)| {
                    binds.push(value.clone());
                    cast(&binds.len(), column, &columns)
                })
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("INSERT INTO {qualified} ({names}) VALUES ({placeholders})");
            let mut query = sqlx::query(AssertSqlSafe(sql));
            for bind in &binds {
                query = query.bind(bind.clone());
            }
            query
                .execute(&mut *transaction)
                .await
                .map_err(map_query_error)?;
        }

        transaction.commit().await.map_err(map_query_error)?;
        Ok(())
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
        let (schema, bare) = self.resolve_object(database, table).await?;
        let qualified = qualify(&schema, &bare);
        let pool = self.pool(Some(database)).await?;
        let mut transaction = pool.begin().await.map_err(map_query_error)?;

        for row_keys in keys {
            if row_keys.is_empty() {
                return Err(Error::Query("delete requires key columns".to_string()));
            }
            let mut binds: Vec<Option<String>> = Vec::new();
            let where_clause = key_clause(row_keys, &columns, &mut binds);
            let sql = format!("DELETE FROM {qualified} WHERE {where_clause}");
            let mut query = sqlx::query(AssertSqlSafe(sql));
            for bind in &binds {
                query = query.bind(bind.clone());
            }
            query
                .execute(&mut *transaction)
                .await
                .map_err(map_query_error)?;
        }

        transaction.commit().await.map_err(map_query_error)?;
        Ok(())
    }

    async fn execute_query(&self, database: Option<&str>, sql: &str) -> Result<QueryResult> {
        self.run_text(database, sql).await
    }

    async fn execute_query_many(
        &self,
        database: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        let pool = self.pool(database).await?;
        let mut connection = pool.acquire().await.map_err(map_query_error)?;
        // One pinned connection for the whole script so `BEGIN`/`COMMIT`, `SET LOCAL` and
        // temporary tables behave correctly. The server splits the script (including dollar-quoted
        // function bodies) itself via the simple query protocol.
        let mut stream = sqlx::raw_sql(AssertSqlSafe(sql.to_string())).fetch_many(&mut *connection);
        let mut results: Vec<QueryResult> = Vec::new();
        let mut columns: Vec<ColumnInfo> = Vec::new();
        let mut rows: Vec<Vec<CellValue>> = Vec::new();
        while let Some(item) = stream.try_next().await.map_err(map_query_error)? {
            match item {
                Either::Left(done) => {
                    let has_result_set = !columns.is_empty() || !rows.is_empty();
                    results.push(QueryResult {
                        statement: String::new(),
                        columns: std::mem::take(&mut columns),
                        rows: std::mem::take(&mut rows),
                        rows_affected: done.rows_affected(),
                        has_result_set,
                        last_insert_id: None,
                    });
                }
                Either::Right(row) => {
                    if columns.is_empty() && rows.is_empty() {
                        columns = row.columns().iter().map(column_info).collect();
                    }
                    let values = (0..columns.len())
                        .map(|index| decode_text_cell(&row, index))
                        .collect();
                    rows.push(values);
                }
            }
        }
        drop(stream);

        let statements = split_statements(sql);
        if results.len() == statements.len() {
            for (result, statement) in results.iter_mut().zip(statements.iter()) {
                result.statement = statement.clone();
                // A zero-row result set exposes no columns through `fetch_many`; prepare the single
                // statement to recover its metadata (not `describe`, which is `offline`-gated).
                if !result.has_result_set
                    && crate::helpers::returns_result_set(statement)
                    && let Ok(described) = prepare_columns(&pool, statement).await
                {
                    result.columns = described;
                    result.has_result_set = true;
                }
            }
        } else {
            for result in results.iter_mut() {
                result.statement = sql.to_string();
            }
        }

        Ok(results)
    }

    async fn create_database(&self, name: &str, options: &DatabaseOptions) -> Result<()> {
        let sql = self.create_database_sql(name, options);
        let database = self.maintenance_database(name);
        self.run_text(Some(&database), &sql).await.map(|_| ())
    }

    fn create_database_sql(&self, name: &str, options: &DatabaseOptions) -> String {
        let mut sql = format!("CREATE DATABASE {}", quote_identifier(name));
        sql.push_str(" WITH TEMPLATE template0");
        if let Some(charset) = non_empty(&options.charset) {
            sql.push_str(&format!(
                " ENCODING {}",
                crate::helpers::quote_literal(charset)
            ));
        }
        if let Some(collation) = non_empty(&options.collation) {
            let literal = crate::helpers::quote_literal(collation);
            sql.push_str(&format!(" LC_COLLATE {literal} LC_CTYPE {literal}"));
        }
        if let Some(owner) = non_empty(&options.owner) {
            sql.push_str(&format!(" OWNER {}", quote_identifier(owner)));
        }
        sql
    }

    async fn drop_database(&self, name: &str) -> Result<()> {
        let sql = format!("DROP DATABASE {}", quote_identifier(name));
        let database = self.maintenance_database(name);
        self.run_text(Some(&database), &sql).await.map(|_| ())
    }

    async fn drop_table(&self, database: &str, table: &str) -> Result<()> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        let sql = format!("DROP TABLE {}", qualify(&schema, &bare));
        self.run_text(Some(database), &sql).await.map(|_| ())
    }

    async fn empty_table(&self, database: &str, table: &str) -> Result<()> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        let sql = format!("DELETE FROM {}", qualify(&schema, &bare));
        self.run_text(Some(database), &sql).await.map(|_| ())
    }

    async fn truncate_table(&self, database: &str, table: &str) -> Result<()> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        let sql = format!("TRUNCATE TABLE {}", qualify(&schema, &bare));
        self.run_text(Some(database), &sql).await.map(|_| ())
    }

    async fn rename_table(&self, database: &str, table: &str, new_name: &str) -> Result<()> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        // The new name may carry a schema prefix; only the bare name is used by `RENAME TO`.
        let new_bare = split_qualified(new_name)
            .map(|(_, bare)| bare)
            .unwrap_or(new_name);
        let sql = format!(
            "ALTER TABLE {} RENAME TO {}",
            qualify(&schema, &bare),
            quote_identifier(new_bare)
        );
        self.run_text(Some(database), &sql).await.map(|_| ())
    }

    async fn database_options(&self, name: &str) -> Result<DatabaseOptions> {
        let pool = self.pool(None).await?;
        let row = sqlx::query(
            "SELECT pg_encoding_to_char(encoding), datcollate, datctype, \
                    pg_get_userbyid(datdba) \
             FROM pg_database WHERE datname = $1",
        )
        .bind(name)
        .fetch_one(&pool)
        .await
        .map_err(map_query_error)?;
        Ok(DatabaseOptions {
            charset: row.try_get(0).unwrap_or_default(),
            collation: row.try_get(1).unwrap_or_default(),
            owner: row.try_get(3).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn database_owners(&self) -> Result<Vec<String>> {
        let pool = self.pool(None).await?;
        let rows = sqlx::query("SELECT rolname FROM pg_roles ORDER BY rolname")
            .fetch_all(&pool)
            .await
            .map_err(map_query_error)?;
        rows.iter()
            .map(|row| row.try_get(0).map_err(map_query_error))
            .collect()
    }

    async fn server_version(&self) -> Result<String> {
        let pool = self.pool(None).await?;
        let row = sqlx::query("SHOW server_version")
            .fetch_one(&pool)
            .await
            .map_err(map_query_error)?;
        row.try_get(0).map_err(map_query_error)
    }

    async fn session_count(&self) -> Result<u64> {
        let pool = self.pool(None).await?;
        let row = sqlx::query(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE datname IS NOT NULL AND pid <> pg_backend_pid()",
        )
        .fetch_one(&pool)
        .await
        .map_err(map_query_error)?;
        Ok(row.try_get::<i64, _>(0).unwrap_or(0).max(0) as u64)
    }

    async fn table_status(&self, database: &str, table: &str) -> Result<TableStatus> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        let pool = self.pool(Some(database)).await?;
        let row = sqlx::query(
            "SELECT GREATEST(c.reltuples, 0)::bigint, \
                    pg_total_relation_size(c.oid), \
                    pg_relation_size(c.oid), \
                    pg_indexes_size(c.oid), \
                    obj_description(c.oid, 'pg_class') \
             FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2",
        )
        .bind(&schema)
        .bind(&bare)
        .fetch_optional(&pool)
        .await
        .map_err(map_query_error)?;
        let Some(row) = row else {
            return Ok(TableStatus::default());
        };
        Ok(TableStatus {
            engine: Some("PostgreSQL".to_string()),
            rows: row
                .try_get::<i64, _>(0)
                .ok()
                .map(|value| value.max(0) as u64),
            data_length: row
                .try_get::<i64, _>(1)
                .ok()
                .map(|value| value.max(0) as u64),
            index_length: row
                .try_get::<i64, _>(3)
                .ok()
                .map(|value| value.max(0) as u64),
            comment: row.try_get(4).unwrap_or(None),
            ..Default::default()
        })
    }

    async fn table_statuses(&self, database: &str) -> Result<Vec<(String, TableStatus)>> {
        let pool = self.pool(Some(database)).await?;
        let rows = sqlx::query(
            "SELECT n.nspname, c.relname, GREATEST(c.reltuples, 0)::bigint, \
                    pg_total_relation_size(c.oid), pg_relation_size(c.oid), \
                    pg_indexes_size(c.oid), obj_description(c.oid, 'pg_class') \
             FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('r','p') \
               AND n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema' \
             ORDER BY n.nspname, c.relname",
        )
        .fetch_all(&pool)
        .await
        .map_err(map_query_error)?;
        Ok(rows
            .iter()
            .map(|row| {
                let schema: String = row.try_get(0).unwrap_or_default();
                let name: String = row.try_get(1).unwrap_or_default();
                (
                    format!("{schema}.{name}"),
                    TableStatus {
                        engine: Some("PostgreSQL".to_string()),
                        rows: row
                            .try_get::<i64, _>(2)
                            .ok()
                            .map(|value| value.max(0) as u64),
                        data_length: row
                            .try_get::<i64, _>(3)
                            .ok()
                            .map(|value| value.max(0) as u64),
                        index_length: row
                            .try_get::<i64, _>(5)
                            .ok()
                            .map(|value| value.max(0) as u64),
                        comment: row.try_get(6).unwrap_or(None),
                        ..Default::default()
                    },
                )
            })
            .collect())
    }

    async fn object_ddl(
        &self,
        database: &str,
        name: &str,
        is_view: bool,
    ) -> Result<Option<String>> {
        if is_view {
            let definition = crate::view::view_definition(self, database, name).await?;
            return Ok(definition.map(|definition| {
                let (schema, bare) = split_qualified(name)
                    .map(|(schema, bare)| (schema.to_string(), bare.to_string()))
                    .unwrap_or_else(|| ("public".to_string(), name.to_string()));
                format!("CREATE VIEW {} AS\n{definition}", qualify(&schema, &bare))
            }));
        }
        match self.table_schema(database, name).await {
            Ok(schema) => Ok(Some(self.table_schema_sql(database, name, None, &schema))),
            Err(_) => Ok(None),
        }
    }

    async fn character_sets(&self) -> Result<Vec<String>> {
        let pool = self.pool(None).await?;
        let rows = sqlx::query(
            "SELECT pg_encoding_to_char(i) FROM generate_series(0, 50) AS i \
             WHERE pg_encoding_to_char(i) <> '' ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .map_err(map_query_error)?;
        rows.iter()
            .map(|row| row.try_get(0).map_err(map_query_error))
            .collect()
    }

    async fn collations(&self) -> Result<Vec<String>> {
        // `LC_COLLATE` wants a libc locale string (`collcollate`), not `collname`. ICU entries are
        // excluded because their locale names are not valid `LC_COLLATE` values.
        let pool = self.pool(None).await?;
        let rows = sqlx::query(
            "SELECT DISTINCT collcollate FROM pg_collation \
             WHERE collcollate <> '' AND collprovider = 'c' ORDER BY 1",
        )
        .fetch_all(&pool)
        .await
        .map_err(map_query_error)?;
        rows.iter()
            .map(|row| row.try_get(0).map_err(map_query_error))
            .collect()
    }

    async fn alter_database_options(
        &self,
        name: &str,
        original: &DatabaseOptions,
        modified: &DatabaseOptions,
    ) -> Result<()> {
        if modified.charset != original.charset && !modified.charset.trim().is_empty() {
            return Err(Error::Query(
                "PostgreSQL does not allow changing a database's encoding after creation"
                    .to_string(),
            ));
        }
        if modified.collation != original.collation && !modified.collation.trim().is_empty() {
            return Err(Error::Query(
                "PostgreSQL does not allow changing a database's collation after creation"
                    .to_string(),
            ));
        }
        if modified.owner != original.owner
            && let Some(owner) = non_empty(&modified.owner)
        {
            let sql = format!(
                "ALTER DATABASE {} OWNER TO {}",
                quote_identifier(name),
                quote_identifier(owner)
            );
            self.run_text(None, &sql).await?;
        }
        Ok(())
    }

    fn alter_database_sql(
        &self,
        name: &str,
        original: &DatabaseOptions,
        modified: &DatabaseOptions,
    ) -> String {
        if modified.owner != original.owner
            && let Some(owner) = non_empty(&modified.owner)
        {
            return format!(
                "ALTER DATABASE {} OWNER TO {}",
                quote_identifier(name),
                quote_identifier(owner)
            );
        }
        String::new()
    }

    fn column_types(&self) -> Vec<&'static str> {
        POSTGRES_COLUMN_TYPES.to_vec()
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
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> String {
        let (schema, bare) = split_qualified(table)
            .map(|(schema, bare)| (schema.to_string(), bare.to_string()))
            .unwrap_or_else(|| ("public".to_string(), table.to_string()));
        let _ = database;
        crate::schema::table_schema_sql(&schema, &bare, original, modified)
    }

    async fn close(&self) -> Result<()> {
        let pools: Vec<PgPool> = self.pools.lock().await.values().cloned().collect();
        for pool in pools {
            pool.close().await;
        }
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
        user: &str,
        _host: &str,
        new_user: &str,
        _new_host: &str,
    ) -> Result<()> {
        crate::user::rename_user(self, user, new_user).await
    }

    fn authentication_plugins(&self) -> Vec<&'static str> {
        vec!["scram-sha-256", "md5", "password"]
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

impl PostgresConnection {
    /// The database used to run `CREATE`/`DROP DATABASE` for `target`: `postgres` when possible,
    /// then the default database, then `template1`. The target itself is never used.
    fn maintenance_database(&self, target: &str) -> String {
        if target != "postgres" {
            return "postgres".to_string();
        }
        if self.default_database != target {
            return self.default_database.clone();
        }
        "template1".to_string()
    }
}

/// The `CAST($n AS <type>)` expression for a column, or a bare placeholder when its type is
/// unknown.
fn cast(index: &usize, column: &str, columns: &[ColumnInfo]) -> String {
    let placeholder = format!("${index}");
    match columns
        .iter()
        .find(|candidate| candidate.name == column)
        .map(|candidate| candidate.data_type.trim())
        .filter(|data_type| !data_type.is_empty())
    {
        Some(data_type) => format!("CAST({placeholder} AS {data_type})"),
        None => placeholder,
    }
}

/// A `WHERE` fragment for key columns, appending bound values (a `None` key becomes `IS NULL`).
fn key_clause(
    keys: &[(String, Option<String>)],
    columns: &[ColumnInfo],
    binds: &mut Vec<Option<String>>,
) -> String {
    keys.iter()
        .map(|(column, value)| {
            let name = quote_identifier(column);
            match value {
                Some(value) => {
                    binds.push(Some(value.clone()));
                    format!("{name} = {}", cast(&binds.len(), column, columns))
                }
                None => format!("{name} IS NULL"),
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn column_info(column: &sqlx::postgres::PgColumn) -> ColumnInfo {
    ColumnInfo {
        name: column.name().to_string(),
        data_type: column.type_info().name().to_string(),
        nullable: true,
        primary_key: false,
        comment: String::new(),
    }
}

/// Prepare one statement to recover its column metadata when a result set had zero rows.
async fn prepare_columns(pool: &PgPool, sql: &str) -> Result<Vec<ColumnInfo>> {
    let statement = pool
        .prepare(AssertSqlSafe(sql.to_string()).into_sql_str())
        .await
        .map_err(map_query_error)?;
    Ok(statement.columns().iter().map(column_info).collect())
}

/// Map a sqlx error to a Navicat-style message: `<SQLSTATE> - <message>` for server errors.
pub(crate) fn map_query_error(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(database_error) = &error
        && let Some(pg_error) = database_error.try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
    {
        return Error::Query(format!("{} - {}", pg_error.code(), pg_error.message()));
    }
    Error::Query(error.to_string())
}

/// The PostgreSQL types offered by the table designer's type list.
const POSTGRES_COLUMN_TYPES: [&str; 34] = [
    "smallint",
    "integer",
    "bigint",
    "smallserial",
    "serial",
    "bigserial",
    "numeric",
    "real",
    "double precision",
    "boolean",
    "text",
    "varchar",
    "char",
    "bytea",
    "date",
    "time",
    "timetz",
    "timestamp",
    "timestamptz",
    "interval",
    "uuid",
    "json",
    "jsonb",
    "xml",
    "money",
    "inet",
    "cidr",
    "macaddr",
    "macaddr8",
    "bit",
    "varbit",
    "tsvector",
    "tsquery",
    "point",
];
