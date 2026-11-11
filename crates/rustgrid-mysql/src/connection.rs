use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use futures_util::TryStreamExt;
use rustgrid_core::{
    BackupObjectKind, CellValue, CheckDef, ColumnDef, ColumnInfo, Connection, DatabaseInfo,
    DatabaseOptions, DriverId, Error, FilterCondition, FilterConjunction, FilterNode,
    FilterOperator, ForeignKeyDef, IndexColumnDef, IndexDef, ObjectDump, ObjectKind,
    ObjectPrivilegeRow, PageRequest, PrivilegeCatalog, QueryResult, Result, RoutineDetails,
    RoutineEdit, RoutineInfo, RoutineKind, RowInsert, RowUpdate, ScriptSession, TableInfo,
    TableOptions, TablePage, TableSchema, TableStatus, TriggerDef, UserAccount, UserDetails,
    UserEdit, ViewDetails, ViewEdit,
};
use sqlx::mysql::{MySqlColumn, MySqlRow};
use sqlx::{
    Acquire, AssertSqlSafe, Column, Either, Executor, Row, SqlSafeStr, Statement, ValueRef,
};
use sqlx::{MySql, MySqlConnection, MySqlPool, pool::PoolConnection};

use crate::engine::Engine;

pub struct MysqlConnection {
    pub(crate) pool: MySqlPool,
    /// Which MySQL-protocol engine (MySQL, MariaDB or StarRocks) this connection targets.
    engine: Engine,
    /// Whether `INFORMATION_SCHEMA.COLUMNS` has `GENERATION_EXPRESSION` (MySQL 5.7.6+ / MariaDB
    /// 10.2+). Older servers reject the whole column query with `1054`, so the first failure flips
    /// this off and later schema loads leave the column out instead of failing again.
    generation_expression: AtomicBool,
    /// `true` once a catalog query was rejected with `1047 ER_UNKNOWN_COM_ERROR`. MySQL-protocol
    /// middlewares (UDAL/TeleDB DBProxy, MyCAT, ...) run their own SQL parser and can refuse
    /// server-side prepares altogether, so this session then runs every generated statement
    /// through the text protocol (and reads the catalog with the classic `SHOW` statements).
    text_protocol: AtomicBool,
    /// `true` once a statement was rejected with `1046 ER_NO_DB_ERROR` ("No database selected").
    /// MySQL-protocol middlewares (UDAL/TeleDB DBProxy) parse and route each statement themselves
    /// and refuse it while their session has no current database — even a fully qualified
    /// `information_schema` query. Once seen, every database-scoped statement selects its
    /// database on the session first (`USE`).
    needs_database: AtomicBool,
    /// The tunnel forwarding this connection, kept alive for as long as the connection is.
    _tunnel: Option<rustgrid_tunnel::Tunnel>,
}

impl MysqlConnection {
    pub fn new(pool: MySqlPool, engine: Engine, tunnel: Option<rustgrid_tunnel::Tunnel>) -> Self {
        Self {
            pool,
            engine,
            generation_expression: AtomicBool::new(true),
            text_protocol: AtomicBool::new(false),
            needs_database: AtomicBool::new(false),
            _tunnel: tunnel,
        }
    }

    /// Whether this session already met a server that rejects server-side prepares (a
    /// MySQL-protocol proxy): every generated statement is rendered for the text protocol instead.
    fn text_protocol(&self) -> bool {
        self.text_protocol.load(Ordering::Relaxed)
    }

    fn mark_text_protocol(&self) {
        self.text_protocol.store(true, Ordering::Relaxed);
    }

    /// Whether this session already met a middleware that rejects statements while it has no
    /// current database: every database-scoped operation then selects its database first.
    fn needs_database(&self) -> bool {
        self.needs_database.load(Ordering::Relaxed)
    }

    fn mark_needs_database(&self) {
        self.needs_database.store(true, Ordering::Relaxed);
    }

    /// Acquire one pooled connection for an operation on `database`, selecting the database first
    /// when a middleware already demanded one (see [`Self::needs_database`]).
    async fn database_connection(&self, database: &str) -> Result<PoolConnection<MySql>> {
        let mut connection = self.pool.acquire().await.map_err(map_query_error)?;
        if self.needs_database() {
            use_database(&mut connection, database)
                .await
                .map_err(map_query_error)?;
        }
        Ok(connection)
    }

    /// Run a prepared query on `connection`. A middleware that answers `1046 ER_NO_DB_ERROR`
    /// ("No database selected") gets the database selected on that session and the statement
    /// retried once; the middleware is remembered, so later operations select the database up
    /// front instead of paying for a failed attempt first.
    async fn prepared_rows(
        &self,
        connection: &mut MySqlConnection,
        database: &str,
        sql: &str,
        binds: &[&str],
    ) -> std::result::Result<Vec<MySqlRow>, sqlx::Error> {
        let build = || {
            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.to_string()));
            for bind in binds {
                query = query.bind(*bind);
            }
            query
        };

        let rows = build().fetch_all(&mut *connection).await;
        if matches!(&rows, Err(error) if is_no_database_error(error)) {
            self.mark_needs_database();
            use_database(connection, database).await?;
            return build().fetch_all(&mut *connection).await;
        }
        rows
    }

    /// Run a text statement on `connection`. A middleware that answers `1046 ER_NO_DB_ERROR`
    /// ("No database selected") — even to a `SHOW` — gets the database selected and the statement
    /// retried once, like [`Self::prepared_rows`].
    async fn text_rows(
        &self,
        connection: &mut MySqlConnection,
        database: &str,
        sql: String,
    ) -> Result<Vec<MySqlRow>> {
        let rows = sqlx::raw_sql(sqlx::AssertSqlSafe(sql.clone()))
            .fetch_all(&mut *connection)
            .await;
        let rows = match rows {
            Err(error) if is_no_database_error(&error) => {
                self.mark_needs_database();
                use_database(connection, database)
                    .await
                    .map_err(map_query_error)?;
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                    .fetch_all(&mut *connection)
                    .await
            }
            rows => rows,
        };
        rows.map_err(map_query_error)
    }

    /// The `INFORMATION_SCHEMA.COLUMNS` rows of one table, ordered by position. `with_generation`
    /// adds `generation_expression`, which only MySQL 5.7.6+ and MariaDB 10.2+ report.
    async fn schema_column_rows(
        &self,
        connection: &mut MySqlConnection,
        database: &str,
        table: &str,
        with_generation: bool,
    ) -> std::result::Result<Vec<MySqlRow>, sqlx::Error> {
        let expression = if with_generation {
            ", generation_expression"
        } else {
            ""
        };
        let sql = format!(
            "SELECT column_name, data_type, column_type, is_nullable, column_default, extra, \
                    column_key, column_comment, character_set_name, collation_name, \
                    character_maximum_length, numeric_precision, numeric_scale{expression} \
             FROM information_schema.columns \
             WHERE table_schema = ? AND table_name = ? \
             ORDER BY ordinal_position"
        );
        self.prepared_rows(connection, database, &sql, &[database, table])
            .await
    }

    async fn scalar_u64(
        &self,
        connection: &mut MySqlConnection,
        sql: String,
        binds: &[String],
    ) -> Result<u64> {
        let row = if self.text_protocol() {
            // A proxy that rejected the catalog's prepared statements rejects every
            // COM_STMT_PREPARE, so the count runs as one text statement.
            let values: Vec<BoundValue> = binds.iter().map(|bind| BoundValue::Text(bind)).collect();
            let sql = text_statement(&sql, &values);
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                .fetch_one(&mut *connection)
                .await
                .map_err(map_query_error)?
        } else {
            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for bind in binds {
                query = query.bind(bind);
            }
            query
                .fetch_one(&mut *connection)
                .await
                .map_err(map_query_error)?
        };

        if let Ok(value) = row.try_get::<i64, _>(0) {
            return Ok(value.max(0) as u64);
        }

        if let Ok(value) = row.try_get::<u64, _>(0) {
            return Ok(value);
        }

        Err(Error::Query("unexpected COUNT result".to_string()))
    }

    /// Run a table-scoped DDL statement (`DROP TABLE` / `TRUNCATE TABLE`) through the text
    /// protocol, which MySQL requires for DDL.
    async fn run_table_ddl(&self, verb: &str, database: &str, table: &str) -> Result<()> {
        let sql = format!(
            "{verb} {}.{}",
            quote_identifier(database),
            quote_identifier(table)
        );
        let mut connection = self.database_connection(database).await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut *connection)
            .await
            .map_err(map_query_error)?;
        Ok(())
    }

    /// The `CREATE` statement an engine's `SHOW CREATE <verb>` reports, e.g. `SHOW CREATE TABLE`.
    /// The DDL lives in the result column whose name starts with "Create" (or contains
    /// "Statement" for `SHOW CREATE TRIGGER`).
    async fn show_create(&self, database: &str, name: &str, verb: &str) -> Result<String> {
        let sql = format!(
            "SHOW CREATE {verb} {}.{}",
            quote_identifier(database),
            quote_identifier(name)
        );
        let result = self.execute_query(Some(database), &sql).await?;
        let index = result
            .columns
            .iter()
            .position(|column| {
                let name = column.name.to_ascii_lowercase();
                name.contains("create") || name.contains("statement")
            })
            .unwrap_or_else(|| usize::from(result.columns.len() > 1));
        let row = result.rows.into_iter().next().unwrap_or_default();
        match row.into_iter().nth(index) {
            Some(CellValue::Text(text)) => Ok(text),
            Some(other) => Ok(other.as_display()),
            None => Err(Error::Query(format!(
                "SHOW CREATE {verb} returned no result"
            ))),
        }
    }

    /// The metadata of a table for a backup: `CREATE TABLE`, column names and trigger DDL. Its
    /// rows are streamed separately by `stream_table_rows`.
    async fn table_metadata(&self, database: &str, table: &str) -> Result<ObjectDump> {
        let ddl = self.show_create(database, table, "TABLE").await?;
        let fields: Vec<String> = self
            .columns(database, table)
            .await?
            .into_iter()
            .map(|column| column.name)
            .collect();

        let mut trigger_ddl = Vec::new();
        if let Ok(schema) = self.table_schema(database, table).await {
            for trigger in &schema.triggers {
                if let Ok(ddl) = self.show_create(database, &trigger.name, "TRIGGER").await {
                    trigger_ddl.push(ddl);
                }
            }
        }

        Ok(ObjectDump {
            name: table.to_string(),
            kind: BackupObjectKind::Table,
            ddl,
            fields,
            trigger_ddl,
            rows: Vec::new(),
        })
    }

    /// The table list through `SHOW FULL TABLES`, which MySQL-protocol proxies translate without
    /// touching `INFORMATION_SCHEMA`. A view's updatability only exists in
    /// `INFORMATION_SCHEMA.VIEWS` (rejected by those proxies), so it stays `false` here.
    async fn list_tables_via_show(
        &self,
        connection: &mut MySqlConnection,
        database: &str,
    ) -> Result<Vec<TableInfo>> {
        let sql = format!("SHOW FULL TABLES FROM {}", quote_identifier(database));
        let rows = self.text_rows(connection, database, sql).await?;

        let mut tables = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            let table_type: String = row.try_get(1).map_err(map_query_error)?;
            let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                ObjectKind::View
            } else {
                ObjectKind::Table
            };
            tables.push(TableInfo {
                name,
                kind,
                updatable: false,
            });
        }
        tables.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(tables)
    }

    /// The columns through `SHOW FULL COLUMNS`, the form proxies translate themselves. Its `Type`
    /// is `column_type` (length/unsigned included), and `Extra` carries the auto-increment and
    /// generated markers the catalog query reads from `EXTRA`.
    async fn columns_via_show(
        &self,
        connection: &mut MySqlConnection,
        database: &str,
        table: &str,
    ) -> Result<Vec<ColumnInfo>> {
        let sql = format!(
            "SHOW FULL COLUMNS FROM {}.{}",
            quote_identifier(database),
            quote_identifier(table)
        );
        let rows = self.text_rows(connection, database, sql).await?;

        let mut columns = Vec::with_capacity(rows.len());
        for row in rows {
            let cell = |name: &str| -> Option<String> {
                row.columns()
                    .iter()
                    .position(|column| column.name().eq_ignore_ascii_case(name))
                    .and_then(|index| row.try_get::<Option<String>, _>(index).ok().flatten())
            };
            let extra = cell("Extra").unwrap_or_default().to_ascii_lowercase();
            let tokens: Vec<&str> = extra.split_whitespace().collect();
            columns.push(ColumnInfo {
                name: cell("Field").unwrap_or_default(),
                data_type: cell("Type").unwrap_or_default(),
                nullable: cell("Null").is_some_and(|value| value.eq_ignore_ascii_case("YES")),
                primary_key: cell("Key").is_some_and(|value| value.eq_ignore_ascii_case("PRI")),
                auto_increment: tokens.contains(&"auto_increment"),
                generated: tokens
                    .iter()
                    .any(|token| matches!(*token, "virtual" | "stored" | "persistent")),
                comment: cell("Comment").unwrap_or_default(),
            });
        }
        Ok(columns)
    }

    /// The `INFORMATION_SCHEMA.TABLES` overview used when the server answers that catalog.
    async fn table_statuses_from_catalog(
        &self,
        connection: &mut MySqlConnection,
        database: &str,
    ) -> std::result::Result<Vec<(String, TableStatus)>, sqlx::Error> {
        let rows = self
            .prepared_rows(
                connection,
                database,
                "SELECT table_name, engine, table_rows, auto_increment, update_time, data_length, \
                        table_comment \
                 FROM information_schema.tables \
                 WHERE table_schema = ? AND table_type <> 'VIEW' \
                 ORDER BY table_name",
                &[database],
            )
            .await?;

        let mut statuses = Vec::with_capacity(rows.len());
        for row in &rows {
            let name: String = row.try_get(0)?;
            let engine: Option<String> = row.try_get(1).unwrap_or(None);
            let updated = row
                .try_get::<Option<DateTime<Utc>>, _>(4)
                .ok()
                .flatten()
                .map(|value| value.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string());
            statuses.push((
                name,
                TableStatus {
                    engine,
                    // `information_schema.tables.table_rows` is an InnoDB estimate and MySQL 8
                    // caches it for `information_schema_stats_expiry` seconds. It is routinely 0
                    // for a table with thousands of rows (and a refresh returns the same cached
                    // value), so the exact count is filled in by the caller.
                    rows: optional_u64(row, 2),
                    auto_increment: optional_u64(row, 3),
                    updated,
                    data_length: optional_u64(row, 5),
                    comment: row.try_get(6).unwrap_or(None),
                    ..TableStatus::default()
                },
            ));
        }
        Ok(statuses)
    }

    /// The same overview through `SHOW TABLE STATUS`, which proxies translate themselves. A proxy
    /// may route it to one shard, so the names are matched against the object list by name.
    async fn table_statuses_via_show(
        &self,
        connection: &mut MySqlConnection,
        database: &str,
    ) -> Result<Vec<(String, TableStatus)>> {
        let sql = format!("SHOW TABLE STATUS FROM {}", quote_identifier(database));
        let rows = self.text_rows(connection, database, sql).await?;
        let mut statuses = Vec::with_capacity(rows.len());
        for row in &rows {
            let columns = columns_from_row(row);
            let values: Vec<CellValue> = (0..columns.len())
                .map(|index| decode_cell(row, index))
                .collect();
            let Some(name) = columns
                .iter()
                .position(|column| column.name.eq_ignore_ascii_case("Name"))
                .and_then(|index| values.get(index))
                .and_then(text_value)
            else {
                continue;
            };
            statuses.push((name, table_status_from(&columns, &values)));
        }
        Ok(statuses)
    }

    /// One single-column name catalog (`information_schema.routines` / `.events`). A server that
    /// rejected prepares gets a plain text query without `ORDER BY` — some proxies reject sorting
    /// `INFORMATION_SCHEMA` — and the names are ordered here instead.
    async fn catalog_names(
        &self,
        sql: &str,
        order_by: &str,
        database: &str,
    ) -> Result<Vec<String>> {
        let mut connection = self.database_connection(database).await?;
        let mut names: Vec<String> = if self.text_protocol() {
            let sql = text_statement(sql, &[BoundValue::Text(database)]);
            let rows = sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                .fetch_all(&mut *connection)
                .await
                .map_err(map_query_error)?;
            rows.iter()
                .map(|row| row.try_get(0).map_err(map_query_error))
                .collect::<Result<Vec<_>>>()?
        } else {
            let sql = format!("{sql} ORDER BY {order_by}");
            let rows = self
                .prepared_rows(&mut connection, database, &sql, &[database])
                .await
                .map_err(map_query_error)?;
            rows.iter()
                .map(|row| row.try_get(0).map_err(map_query_error))
                .collect::<Result<Vec<_>>>()?
        };
        if self.text_protocol() {
            names.sort_by_key(|name| name.to_lowercase());
        }
        Ok(names)
    }
}

/// A pinned MySQL session for running a script statement by statement (see
/// [`rustgrid_core::ScriptSession`]). Holding the `PoolConnection` keeps `SET` statements and
/// `USE` in effect for every statement in the run.
struct MySqlScriptSession {
    connection: PoolConnection<MySql>,
}

#[async_trait]
impl ScriptSession for MySqlScriptSession {
    async fn execute(&mut self, sql: &str) -> Result<QueryResult> {
        execute_one(&mut self.connection, sql).await
    }

    async fn execute_many(&mut self, sql: &str) -> Result<Vec<QueryResult>> {
        fetch_results(&mut self.connection, sql).await
    }
}

#[async_trait]
impl Connection for MysqlConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new(self.engine.driver_id())
    }

    /// MySQL's own scanner: top-level semicolons, honouring `'`/`"`/`` ` `` literals, `--`/`#`
    /// line comments and `/* */` block comments.
    fn split_statements(&self, sql: &str) -> Vec<String> {
        split_statements(sql)
    }

    /// Pin one pooled connection for the whole script, so `SET` statements (e.g.
    /// `SET FOREIGN_KEY_CHECKS=0`) and `USE` apply to every statement the caller runs.
    async fn script_session(
        &self,
        database: Option<&str>,
    ) -> Result<Option<Box<dyn ScriptSession>>> {
        let mut connection = self.pool.acquire().await.map_err(map_query_error)?;
        if let Some(database) = database {
            use_database(&mut connection, database)
                .await
                .map_err(map_query_error)?;
        }
        Ok(Some(Box::new(MySqlScriptSession { connection })))
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>> {
        // SHOW is not preparable on every MySQL-protocol server (e.g. StarRocks).
        let rows = sqlx::raw_sql("SHOW DATABASES")
            .fetch_all(&self.pool)
            .await
            .map_err(map_query_error)?;

        let mut databases = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            databases.push(DatabaseInfo { name });
        }
        Ok(databases)
    }

    async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>> {
        let mut connection = self.database_connection(database).await?;
        if self.text_protocol() {
            return self.list_tables_via_show(&mut connection, database).await;
        }

        let rows = self
            .prepared_rows(
                &mut connection,
                database,
                "SELECT t.table_name, t.table_type, v.is_updatable \
                 FROM information_schema.tables AS t \
                 LEFT JOIN information_schema.views AS v \
                   ON v.table_schema = t.table_schema AND v.table_name = t.table_name \
                 WHERE t.table_schema = ? \
                 ORDER BY t.table_name",
                &[database],
            )
            .await;
        let rows = match rows {
            Ok(rows) => rows,
            Err(error) if is_unknown_command_error(&error) => {
                self.mark_text_protocol();
                return self.list_tables_via_show(&mut connection, database).await;
            }
            Err(error) => return Err(map_query_error(error)),
        };

        let mut tables = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            let table_type: String = row.try_get(1).map_err(map_query_error)?;
            let updatable: Option<String> = row.try_get(2).unwrap_or(None);
            let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                ObjectKind::View
            } else {
                ObjectKind::Table
            };
            tables.push(TableInfo {
                name,
                kind,
                updatable: updatable.is_some_and(|value| value.eq_ignore_ascii_case("YES")),
            });
        }
        Ok(tables)
    }

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        let mut connection = self.database_connection(database).await?;
        if self.text_protocol() {
            return self
                .columns_via_show(&mut connection, database, table)
                .await;
        }

        // `EXTRA` is a MySQL/MariaDB column. StarRocks' information_schema is only a subset, so it
        // keeps the plain catalog query (it has no auto-increment/generated columns to report).
        let starrocks = self.engine == Engine::StarRocks;
        let sql = if starrocks {
            "SELECT column_name, column_type, is_nullable, column_key, column_comment \
             FROM information_schema.columns \
             WHERE table_schema = ? AND table_name = ? \
             ORDER BY ordinal_position"
        } else {
            "SELECT column_name, column_type, is_nullable, column_key, column_comment, extra \
             FROM information_schema.columns \
             WHERE table_schema = ? AND table_name = ? \
             ORDER BY ordinal_position"
        };
        let rows = self
            .prepared_rows(&mut connection, database, sql, &[database, table])
            .await;
        let rows = match rows {
            Ok(rows) => rows,
            Err(error) if is_unknown_command_error(&error) => {
                self.mark_text_protocol();
                return self
                    .columns_via_show(&mut connection, database, table)
                    .await;
            }
            Err(error) => return Err(map_query_error(error)),
        };

        let mut columns = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            let data_type: String = row.try_get(1).map_err(map_query_error)?;
            let nullable: String = row.try_get(2).map_err(map_query_error)?;
            let key: String = row.try_get(3).map_err(map_query_error)?;
            let comment: String = row.try_get(4).unwrap_or_default();
            let extra = if starrocks {
                String::new()
            } else {
                row.try_get(5).unwrap_or_default()
            };
            let extra = extra.to_ascii_lowercase();
            let tokens: Vec<&str> = extra.split_whitespace().collect();
            columns.push(ColumnInfo {
                name,
                data_type,
                nullable: nullable.eq_ignore_ascii_case("YES"),
                primary_key: key.eq_ignore_ascii_case("PRI"),
                // `AUTO_INCREMENT`; MariaDB reports it in the same `EXTRA` column.
                auto_increment: tokens.contains(&"auto_increment"),
                // Generated columns read `VIRTUAL GENERATED` / `STORED GENERATED` (MariaDB up to
                // 10.1: `VIRTUAL` / `PERSISTENT`). A default expression is `DEFAULT_GENERATED`,
                // one underscore-separated token, so it never matches.
                generated: tokens
                    .iter()
                    .any(|token| matches!(*token, "virtual" | "stored" | "persistent")),
                comment,
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
        let mut connection = self.database_connection(database).await?;
        let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(table));

        let (where_clause, binds) = filter_clause(&page.filter);

        let total_rows = if page.skip_total {
            None
        } else {
            self.scalar_u64(
                &mut connection,
                format!("SELECT COUNT(*) FROM {qualified}{where_clause}"),
                &binds,
            )
            .await
            .ok()
        };

        // StarRocks rejects a bound parameter in LIMIT/OFFSET ("using parameter(?) as limit or
        // offset not supported"). The values are internal page numbers, never user SQL, so write
        // them into the statement for that engine; MySQL/MariaDB keep the prepared form.
        let starrocks = self.engine == Engine::StarRocks;
        let page_sql = if starrocks {
            format!(
                "SELECT * FROM {qualified}{where_clause}{} LIMIT {} OFFSET {}",
                order_clause(&page),
                page.page_size,
                page.offset()
            )
        } else {
            format!(
                "SELECT * FROM {qualified}{where_clause}{} LIMIT ? OFFSET ?",
                order_clause(&page)
            )
        };
        let rows = if self.text_protocol() {
            // The same proxy restriction: every bound statement becomes one text statement with
            // the page's filter values and LIMIT/OFFSET inlined.
            let mut values: Vec<BoundValue> =
                binds.iter().map(|bind| BoundValue::Text(bind)).collect();
            if !starrocks {
                values.push(BoundValue::Number(page.page_size as i64));
                values.push(BoundValue::Number(page.offset() as i64));
            }
            let sql = text_statement(&page_sql, &values);
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                .fetch_all(&mut *connection)
                .await
                .map_err(map_query_error)?
        } else {
            let mut query = sqlx::query(sqlx::AssertSqlSafe(page_sql));
            for bind in &binds {
                query = query.bind(bind);
            }
            if !starrocks {
                query = query.bind(page.page_size as i64).bind(page.offset() as i64);
            }
            query
                .fetch_all(&mut *connection)
                .await
                .map_err(map_query_error)?
        };

        let mut decoded = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut values = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                values.push(decode_cell(row, index));
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

    async fn count_rows(
        &self,
        database: &str,
        table: &str,
        filter: &[FilterNode],
    ) -> Result<Option<u64>> {
        let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(table));
        let (where_clause, binds) = filter_clause(filter);
        let mut connection = self.database_connection(database).await?;
        let total = self
            .scalar_u64(
                &mut connection,
                format!("SELECT COUNT(*) FROM {qualified}{where_clause}"),
                &binds,
            )
            .await?;
        Ok(Some(total))
    }

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(table));

        // StarRocks rejects prepared INSERT/UPDATE/DELETE (`1295 ... not supported in the
        // prepared statement protocol yet`), and a MySQL-protocol proxy can refuse server-side
        // prepares altogether. Run each row as a plain text statement for both: the values are
        // already decoded cell strings, so quoting them is safe and keeps the same semantics.
        if self.engine == Engine::StarRocks || self.text_protocol() {
            let mut connection = self.database_connection(database).await?;
            for update in updates {
                if update.set.is_empty() {
                    continue;
                }
                let set_clause = update
                    .set
                    .iter()
                    .map(|(column, value)| {
                        format!("{} = {}", quote_identifier(column), edit_value_sql(value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let where_clause = if update.keys.is_empty() {
                    "1 = 1".to_string()
                } else {
                    update
                        .keys
                        .iter()
                        .map(|(column, value)| match value {
                            Some(_) => {
                                format!("{} = {}", quote_identifier(column), edit_value_sql(value))
                            }
                            None => format!("{} IS NULL", quote_identifier(column)),
                        })
                        .collect::<Vec<_>>()
                        .join(" AND ")
                };
                let sql = format!("UPDATE {qualified} SET {set_clause} WHERE {where_clause}");
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                    .execute(&mut *connection)
                    .await
                    .map_err(map_query_error)?;
            }
            return Ok(());
        }

        let mut connection = self.database_connection(database).await?;
        let mut transaction = connection.begin().await.map_err(map_query_error)?;

        for update in updates {
            if update.set.is_empty() {
                continue;
            }

            let set_clause = update
                .set
                .iter()
                .map(|(column, _)| format!("{} = ?", quote_identifier(column)))
                .collect::<Vec<_>>()
                .join(", ");
            let where_clause = if update.keys.is_empty() {
                "1 = 1".to_string()
            } else {
                update
                    .keys
                    .iter()
                    .map(|(column, value)| match value {
                        Some(_) => format!("{} = ?", quote_identifier(column)),
                        None => format!("{} IS NULL", quote_identifier(column)),
                    })
                    .collect::<Vec<_>>()
                    .join(" AND ")
            };
            let sql = format!("UPDATE {qualified} SET {set_clause} WHERE {where_clause}");

            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for (_, value) in &update.set {
                query = query.bind(value.clone());
            }
            for (_, value) in &update.keys {
                if let Some(value) = value {
                    query = query.bind(value.clone());
                }
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

        let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(table));

        // See `update_rows`: both StarRocks and a proxy that rejects prepares need the text
        // protocol with quoted literals.
        if self.engine == Engine::StarRocks || self.text_protocol() {
            let mut connection = self.database_connection(database).await?;
            for row in rows {
                if row.values.is_empty() {
                    let sql = format!("INSERT INTO {qualified} () VALUES ()");
                    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                        .execute(&mut *connection)
                        .await
                        .map_err(map_query_error)?;
                    continue;
                }
                let columns = row
                    .values
                    .iter()
                    .map(|(column, _)| quote_identifier(column))
                    .collect::<Vec<_>>()
                    .join(", ");
                let values = row
                    .values
                    .iter()
                    .map(|(_, value)| edit_value_sql(value))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!("INSERT INTO {qualified} ({columns}) VALUES ({values})");
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                    .execute(&mut *connection)
                    .await
                    .map_err(map_query_error)?;
            }
            return Ok(());
        }

        let mut connection = self.database_connection(database).await?;
        let mut transaction = connection.begin().await.map_err(map_query_error)?;

        for row in rows {
            if row.values.is_empty() {
                // Every column takes its default (`AUTO_INCREMENT`, `DEFAULT`, ...).
                let sql = format!("INSERT INTO {qualified} () VALUES ()");
                sqlx::query(sqlx::AssertSqlSafe(sql))
                    .execute(&mut *transaction)
                    .await
                    .map_err(map_query_error)?;
                continue;
            }

            let columns = row
                .values
                .iter()
                .map(|(column, _)| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ");
            let placeholders = vec!["?"; row.values.len()].join(", ");
            let sql = format!("INSERT INTO {qualified} ({columns}) VALUES ({placeholders})");

            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for (_, value) in &row.values {
                query = query.bind(value.clone());
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

        let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(table));

        // See `update_rows`: use the text protocol for StarRocks and for a proxy that rejects
        // prepares.
        if self.engine == Engine::StarRocks || self.text_protocol() {
            let mut connection = self.database_connection(database).await?;
            for row_keys in keys {
                if row_keys.is_empty() {
                    return Err(Error::Query("delete requires key columns".to_string()));
                }
                let where_clause = row_keys
                    .iter()
                    .map(|(column, value)| match value {
                        Some(_) => {
                            format!("{} = {}", quote_identifier(column), edit_value_sql(value))
                        }
                        None => format!("{} IS NULL", quote_identifier(column)),
                    })
                    .collect::<Vec<_>>()
                    .join(" AND ");
                // StarRocks rejects `DELETE ... LIMIT`; the key columns already identify the row.
                let sql = if self.engine == Engine::StarRocks {
                    format!("DELETE FROM {qualified} WHERE {where_clause}")
                } else {
                    format!("DELETE FROM {qualified} WHERE {where_clause} LIMIT 1")
                };
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                    .execute(&mut *connection)
                    .await
                    .map_err(map_query_error)?;
            }
            return Ok(());
        }

        let mut connection = self.database_connection(database).await?;
        let mut transaction = connection.begin().await.map_err(map_query_error)?;

        for row_keys in keys {
            if row_keys.is_empty() {
                return Err(Error::Query("delete requires key columns".to_string()));
            }
            let where_clause = row_keys
                .iter()
                .map(|(column, value)| match value {
                    Some(_) => format!("{} = ?", quote_identifier(column)),
                    None => format!("{} IS NULL", quote_identifier(column)),
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            let sql = format!("DELETE FROM {qualified} WHERE {where_clause} LIMIT 1");

            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for (_, value) in row_keys {
                if let Some(value) = value {
                    query = query.bind(value.clone());
                }
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
        let mut connection = self.pool.acquire().await.map_err(map_query_error)?;

        if let Some(database) = database {
            use_database(&mut connection, database)
                .await
                .map_err(map_query_error)?;
        }

        execute_one(&mut connection, sql).await
    }

    async fn execute_query_many(
        &self,
        database: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        let mut connection = self.pool.acquire().await.map_err(map_query_error)?;

        if let Some(database) = database {
            use_database(&mut connection, database)
                .await
                .map_err(map_query_error)?;
        }

        fetch_results(&mut connection, sql).await
    }

    async fn create_database(&self, name: &str, options: &DatabaseOptions) -> Result<()> {
        let sql = self.create_database_sql(name, options);
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&self.pool)
            .await
            .map_err(map_query_error)?;
        Ok(())
    }

    fn create_database_sql(&self, name: &str, options: &DatabaseOptions) -> String {
        let mut sql = format!("CREATE DATABASE {}", quote_identifier(name));
        if !options.charset.trim().is_empty() {
            sql.push_str(" CHARACTER SET ");
            sql.push_str(options.charset.trim());
        }
        if !options.collation.trim().is_empty() {
            sql.push_str(" COLLATE ");
            sql.push_str(options.collation.trim());
        }
        sql.push(';');
        sql
    }

    async fn drop_database(&self, name: &str) -> Result<()> {
        let sql = format!("DROP DATABASE {}", quote_identifier(name));
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&self.pool)
            .await
            .map_err(map_query_error)?;
        Ok(())
    }

    async fn drop_table(&self, database: &str, table: &str) -> Result<()> {
        self.run_table_ddl("DROP TABLE", database, table).await
    }

    async fn empty_table(&self, database: &str, table: &str) -> Result<()> {
        let sql = format!(
            "DELETE FROM {}.{}",
            quote_identifier(database),
            quote_identifier(table)
        );
        let mut connection = self.database_connection(database).await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut *connection)
            .await
            .map_err(map_query_error)?;
        Ok(())
    }

    async fn truncate_table(&self, database: &str, table: &str) -> Result<()> {
        self.run_table_ddl("TRUNCATE TABLE", database, table).await
    }

    async fn rename_table(&self, database: &str, table: &str, new_name: &str) -> Result<()> {
        let sql = format!(
            "RENAME TABLE {}.{} TO {}.{}",
            quote_identifier(database),
            quote_identifier(table),
            quote_identifier(database),
            quote_identifier(new_name)
        );
        let mut connection = self.database_connection(database).await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut *connection)
            .await
            .map_err(map_query_error)?;
        Ok(())
    }

    async fn database_options(&self, name: &str) -> Result<DatabaseOptions> {
        let mut connection = self.database_connection(name).await?;
        let row = if self.text_protocol() {
            let sql = text_statement(
                "SELECT default_character_set_name, default_collation_name \
                 FROM information_schema.schemata \
                 WHERE schema_name = ?",
                &[BoundValue::Text(name)],
            );
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                .fetch_all(&mut *connection)
                .await
                .map_err(map_query_error)?
        } else {
            self.prepared_rows(
                &mut connection,
                name,
                "SELECT default_character_set_name, default_collation_name \
                 FROM information_schema.schemata \
                 WHERE schema_name = ?",
                &[name],
            )
            .await
            .map_err(map_query_error)?
        }
        .into_iter()
        .next()
        .ok_or_else(|| Error::Query(format!("database {name} does not exist")))?;

        let charset: String = row.try_get(0).map_err(map_query_error)?;
        let collation: String = row.try_get(1).map_err(map_query_error)?;
        Ok(DatabaseOptions {
            charset,
            collation,
            ..Default::default()
        })
    }

    async fn server_version(&self) -> Result<String> {
        // No parameters: the text protocol works on servers and on proxies that reject prepares.
        let row = sqlx::raw_sql("SELECT VERSION()")
            .fetch_one(&self.pool)
            .await
            .map_err(map_query_error)?;
        row.try_get(0).map_err(map_query_error)
    }

    async fn session_count(&self) -> Result<u64> {
        let mut connection = self.pool.acquire().await.map_err(map_query_error)?;
        self.scalar_u64(
            &mut connection,
            "SELECT COUNT(*) FROM information_schema.PROCESSLIST".to_string(),
            &[],
        )
        .await
    }

    async fn table_status(&self, database: &str, table: &str) -> Result<TableStatus> {
        let sql = format!(
            "SHOW TABLE STATUS FROM {} LIKE '{}'",
            quote_identifier(database),
            escape_string_literal(table)
        );
        let result = self.execute_query(Some(database), &sql).await?;
        let Some(row) = result.rows.into_iter().next() else {
            return Ok(TableStatus::default());
        };
        Ok(table_status_from(&result.columns, &row))
    }

    async fn table_statuses(&self, database: &str) -> Result<Vec<(String, TableStatus)>> {
        let mut connection = self.database_connection(database).await?;
        let mut statuses = if self.text_protocol() {
            self.table_statuses_via_show(&mut connection, database)
                .await?
        } else {
            match self
                .table_statuses_from_catalog(&mut connection, database)
                .await
            {
                Ok(statuses) => statuses,
                Err(error) if is_unknown_command_error(&error) => {
                    self.mark_text_protocol();
                    self.table_statuses_via_show(&mut connection, database)
                        .await?
                }
                Err(error) => return Err(map_query_error(error)),
            }
        };

        // One `COUNT(*)` per table, batched into a single UNION ALL statement. The object list
        // shows the real row count instead of the optimizer's estimate; a database with many
        // tables pays one round trip and one scan per table.
        if !statuses.is_empty() {
            let mut sql = String::new();
            for (index, (name, _)) in statuses.iter().enumerate() {
                if index > 0 {
                    sql.push_str(" UNION ALL ");
                }
                sql.push_str(&format!(
                    "SELECT {} AS table_name, COUNT(*) AS exact_rows FROM {}.{}",
                    quote_literal(name),
                    quote_identifier(database),
                    quote_identifier(name)
                ));
            }
            // Best-effort: if a table disappears between the catalog read and the count (or the
            // user's database forbids reading one), keep the catalog estimate instead of failing
            // the whole object list. The statement has no parameters, so the text protocol (which
            // a MySQL-protocol proxy also understands) is enough.
            let count_rows = self
                .text_rows(&mut connection, database, sql)
                .await
                .unwrap_or_default();
            for row in &count_rows {
                let name: String = row.try_get(0).map_err(map_query_error)?;
                let count = optional_u64(row, 1).unwrap_or(0);
                if let Some((_, status)) = statuses
                    .iter_mut()
                    .find(|(candidate, _)| candidate == &name)
                {
                    status.rows = Some(count);
                }
            }
        }
        Ok(statuses)
    }

    async fn object_ddl(
        &self,
        database: &str,
        name: &str,
        is_view: bool,
    ) -> Result<Option<String>> {
        // `SHOW CREATE TABLE` reports both tables and views, so the flag is informational.
        let _ = is_view;
        match self.show_create(database, name, "TABLE").await {
            Ok(ddl) => Ok(Some(ddl)),
            Err(_) => Ok(None),
        }
    }

    async fn character_sets(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT character_set_name FROM information_schema.character_sets \
             ORDER BY character_set_name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;

        rows.iter()
            .map(|row| row.try_get(0).map_err(map_query_error))
            .collect()
    }

    async fn collations(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT collation_name FROM information_schema.collations \
             ORDER BY collation_name",
        )
        .fetch_all(&self.pool)
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
        let sql = alter_database_sql(name, original, modified);
        if sql.is_empty() {
            return Ok(());
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&self.pool)
            .await
            .map_err(map_query_error)?;
        Ok(())
    }

    fn alter_database_sql(
        &self,
        name: &str,
        original: &DatabaseOptions,
        modified: &DatabaseOptions,
    ) -> String {
        alter_database_sql(name, original, modified)
    }

    fn column_types(&self) -> Vec<&'static str> {
        MYSQL_COLUMN_TYPES.to_vec()
    }

    fn storage_engines(&self) -> Vec<&'static str> {
        MYSQL_STORAGE_ENGINES.to_vec()
    }

    async fn list_routines(&self, database: &str) -> Result<Vec<String>> {
        self.catalog_names(
            "SELECT routine_name FROM information_schema.routines WHERE routine_schema = ?",
            "routine_name",
            database,
        )
        .await
    }

    async fn list_events(&self, database: &str) -> Result<Vec<String>> {
        self.catalog_names(
            "SELECT event_name FROM information_schema.events WHERE event_schema = ?",
            "event_name",
            database,
        )
        .await
    }

    async fn backup_object_metadata(
        &self,
        database: &str,
        kind: BackupObjectKind,
        name: &str,
    ) -> Result<ObjectDump> {
        let ddl = match kind {
            BackupObjectKind::Table => return self.table_metadata(database, name).await,
            BackupObjectKind::View => self.show_create(database, name, "VIEW").await?,
            BackupObjectKind::Function => {
                let mut connection = self.database_connection(database).await?;
                let routine_type: Option<String> = self
                    .prepared_rows(
                        &mut connection,
                        database,
                        "SELECT routine_type FROM information_schema.routines \
                         WHERE routine_schema = ? AND routine_name = ?",
                        &[database, name],
                    )
                    .await
                    .map_err(map_query_error)?
                    .into_iter()
                    .next()
                    .and_then(|row| row.try_get(0).ok());
                let verb = if routine_type
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("PROCEDURE"))
                {
                    "PROCEDURE"
                } else {
                    "FUNCTION"
                };
                self.show_create(database, name, verb).await?
            }
            BackupObjectKind::Event => self.show_create(database, name, "EVENT").await?,
        };
        Ok(ObjectDump {
            name: name.to_string(),
            kind,
            ddl,
            fields: Vec::new(),
            trigger_ddl: Vec::new(),
            rows: Vec::new(),
        })
    }

    async fn stream_table_rows(
        &self,
        database: &str,
        table: &str,
        on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<u64> {
        let columns = self.columns(database, table).await?;
        let types: Vec<String> = columns
            .iter()
            .map(|column| column.data_type.clone())
            .collect();

        let sql = format!(
            "SELECT * FROM {}.{}",
            quote_identifier(database),
            quote_identifier(table)
        );
        // `fetch` yields a boxed stream, so rows are decoded and handed to `on_row` one at a
        // time instead of materialising the whole table. `poll_fn` drives it without pulling in
        // the whole `futures-util` surface. No parameters: the text protocol keeps the backup
        // working on a MySQL-protocol proxy too.
        let mut connection = self.database_connection(database).await?;
        let mut stream = sqlx::raw_sql(sqlx::AssertSqlSafe(sql)).fetch(&mut *connection);
        let mut count = 0u64;
        while let Some(item) = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            let row = item.map_err(map_query_error)?;
            let mut tuple = String::from("(");
            for index in 0..columns.len() {
                if index > 0 {
                    tuple.push_str(", ");
                }
                tuple.push_str(&render_literal(
                    &decode_cell(&row, index),
                    types.get(index).map(String::as_str).unwrap_or(""),
                ));
            }
            tuple.push(')');
            on_row(&tuple)?;
            count += 1;
        }
        Ok(count)
    }

    async fn restore_object(&self, database: &str, object: &ObjectDump) -> Result<()> {
        // One pinned connection for the whole object: it keeps the session-scoped
        // `SET FOREIGN_KEY_CHECKS=0` and carries every batch inside a single transaction, which
        // is dramatically faster than one connection and one implicit commit per statement.
        let mut connection = self.pool.acquire().await.map_err(map_query_error)?;
        let result = restore_object_on(&mut connection, database, object).await;
        if result.is_err() {
            // A failed batch must not leave an open transaction on a connection that is about
            // to go back to the pool.
            let _ = exec_sql(&mut connection, "ROLLBACK").await;
        }
        // Always restore the session default: the connection returns to the pool, and a leaked
        // `FOREIGN_KEY_CHECKS=0` would silently disable integrity checks for later work.
        let _ = exec_sql(&mut connection, "SET FOREIGN_KEY_CHECKS=1").await;
        result
    }

    async fn table_schema(&self, database: &str, table: &str) -> Result<TableSchema> {
        let mut connection = self.database_connection(database).await?;
        let mut with_generation = self.generation_expression.load(Ordering::Relaxed);
        let column_rows = match self
            .schema_column_rows(&mut connection, database, table, with_generation)
            .await
        {
            Ok(rows) => rows,
            // MySQL before 5.7.6, MariaDB before 10.2 and a few MySQL-compatible servers have no
            // `GENERATION_EXPRESSION` column and reject the whole statement with `1054`. Those
            // servers have no generated columns either, so retry without it — failing the designer
            // (and structure sync) over an optional metadata column is worse than not reporting it.
            Err(error) if with_generation && is_unknown_column_error(&error) => {
                match self
                    .schema_column_rows(&mut connection, database, table, false)
                    .await
                {
                    Ok(rows) => {
                        with_generation = false;
                        self.generation_expression.store(false, Ordering::Relaxed);
                        rows
                    }
                    Err(_) => return Err(map_query_error(error)),
                }
            }
            Err(error) => return Err(map_query_error(error)),
        };

        let mut columns = Vec::with_capacity(column_rows.len());
        for row in &column_rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            let data_type: String = row.try_get(1).map_err(map_query_error)?;
            let column_type: String = row.try_get(2).map_err(map_query_error)?;
            let nullable: String = row.try_get(3).unwrap_or_else(|_| "YES".to_string());
            let default: Option<String> = row.try_get(4).unwrap_or(None);
            let extra: String = row.try_get(5).unwrap_or_default();
            let key: String = row.try_get(6).unwrap_or_default();
            let comment: String = row.try_get(7).unwrap_or_default();
            let charset: Option<String> = row.try_get(8).unwrap_or(None);
            let collation: Option<String> = row.try_get(9).unwrap_or(None);
            let char_length = optional_u64(row, 10);
            let precision = optional_u64(row, 11);
            let scale = optional_u64(row, 12);
            let generated: Option<String> = if with_generation {
                row.try_get(13).unwrap_or(None)
            } else {
                None
            };

            let numeric = data_type.eq_ignore_ascii_case("decimal")
                || data_type.eq_ignore_ascii_case("numeric");
            // `TEXT`/`BLOB` columns report their byte capacity as `character_maximum_length` (65535
            // for `text`, 16777215 for `mediumtext`). That is not a definition length: rendering it
            // back as `text(65535)` makes MySQL pick the smallest type that fits 65535 *characters*,
            // which under utf8mb4 is `mediumtext` — so the column could never converge after a sync.
            let length = if type_ignores_length(&data_type) {
                "0".to_string()
            } else if let Some(length) = char_length {
                length.to_string()
            } else if numeric {
                precision
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "0".to_string())
            } else {
                "0".to_string()
            };
            let decimals = if numeric {
                scale
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "0".to_string())
            } else {
                "0".to_string()
            };

            let extra_lower = extra.to_ascii_lowercase();
            let column_type_lower = column_type.to_ascii_lowercase();
            let on_update = extra_lower
                .find("on update")
                .map(|index| extra[index..].trim().to_string())
                .unwrap_or_default();

            // `information_schema` reports both an explicit empty-string default and no default as
            // an empty string; only `None` means SQL NULL. Keeping the empty string as-is made
            // `DEFAULT ''` indistinguishable from `DEFAULT NULL`, so the structure diff reported
            // "no changes" while the DDL panes still showed the difference. Store the quoted empty
            // string explicitly so both the diff and `format_default` see a real literal.
            let default = default
                .map(|value| {
                    if value.is_empty() {
                        "''".to_string()
                    } else {
                        value
                    }
                })
                .unwrap_or_default();

            columns.push(ColumnDef {
                name,
                data_type,
                length,
                decimals,
                nullable: nullable.eq_ignore_ascii_case("YES"),
                default,
                primary_key: key.eq_ignore_ascii_case("PRI"),
                auto_increment: extra_lower.contains("auto_increment"),
                identity: String::new(),
                identity_start: String::new(),
                identity_increment: String::new(),
                unsigned: column_type_lower.contains("unsigned"),
                zerofill: column_type_lower.contains("zerofill"),
                sparse: false,
                comment,
                charset: charset.unwrap_or_default(),
                collation: collation.unwrap_or_default(),
                extra: on_update,
                generated: generated.unwrap_or_default(),
                stored: extra_lower.contains("stored"),
            });
        }

        let index_rows = sqlx::query(
            "SELECT index_name, non_unique, index_type, column_name, comment, collation, sub_part \
             FROM information_schema.statistics \
             WHERE table_schema = ? AND table_name = ? \
             ORDER BY index_name, seq_in_index",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&mut *connection)
        .await
        .map_err(map_query_error)?;

        let mut indexes: Vec<IndexDef> = Vec::new();
        for row in &index_rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            let non_unique: i64 = row.try_get(1).unwrap_or(0);
            let index_type: String = row.try_get(2).unwrap_or_default();
            let column: Option<String> = row.try_get(3).unwrap_or(None);
            let comment: String = row.try_get(4).unwrap_or_default();
            // `collation` is `A`/`D`/NULL (binary or unsorted); `sub_part` is the prefix length.
            let collation: Option<String> = row.try_get(5).unwrap_or(None);
            let sub_part: Option<i64> = row.try_get(6).unwrap_or(None);
            let index_column = column.map(|name| IndexColumnDef {
                name,
                descending: collation
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case("D")),
                prefix_length: sub_part
                    .map(|length| length.to_string())
                    .unwrap_or_default(),
            });
            if let Some(existing) = indexes.iter_mut().find(|index| index.name == name) {
                if let Some(column) = index_column {
                    existing.columns.push(column);
                }
            } else {
                indexes.push(IndexDef {
                    name: name.clone(),
                    columns: index_column.into_iter().collect(),
                    unique: non_unique == 0,
                    primary: name.eq_ignore_ascii_case("PRIMARY"),
                    index_type,
                    comment,
                });
            }
        }

        let fk_rows = sqlx::query(
            "SELECT k.constraint_name, k.column_name, k.referenced_table_schema, \
                    k.referenced_table_name, k.referenced_column_name, r.update_rule, \
                    r.delete_rule \
             FROM information_schema.key_column_usage AS k \
             JOIN information_schema.referential_constraints AS r \
               ON r.constraint_schema = k.constraint_schema \
              AND r.constraint_name = k.constraint_name \
             WHERE k.table_schema = ? AND k.table_name = ? \
               AND k.referenced_table_name IS NOT NULL \
             ORDER BY k.constraint_name, k.ordinal_position",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&mut *connection)
        .await
        .map_err(map_query_error)?;

        let mut foreign_keys: Vec<ForeignKeyDef> = Vec::new();
        for row in &fk_rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            let column: Option<String> = row.try_get(1).unwrap_or(None);
            let ref_schema: Option<String> = row.try_get(2).unwrap_or(None);
            let ref_table: Option<String> = row.try_get(3).unwrap_or(None);
            let ref_column: Option<String> = row.try_get(4).unwrap_or(None);
            let update_rule: String = row.try_get(5).unwrap_or_default();
            let delete_rule: String = row.try_get(6).unwrap_or_default();
            if let Some(existing) = foreign_keys.iter_mut().find(|fk| fk.name == name) {
                if let Some(column) = column {
                    existing.columns.push(column);
                }
                if let Some(column) = ref_column {
                    existing.referenced_columns.push(column);
                }
            } else {
                foreign_keys.push(ForeignKeyDef {
                    name,
                    columns: column.into_iter().collect(),
                    // A same-database reference stays bare (the database is MySQL's schema);
                    // a cross-database one keeps its database name so the DDL can qualify it.
                    referenced_schema: ref_schema
                        .filter(|schema| !schema.eq_ignore_ascii_case(database))
                        .unwrap_or_default(),
                    referenced_table: ref_table.unwrap_or_default(),
                    referenced_columns: ref_column.into_iter().collect(),
                    on_delete: normalize_rule(&delete_rule),
                    on_update: normalize_rule(&update_rule),
                });
            }
        }

        let trigger_rows = sqlx::query(
            "SELECT trigger_name, action_timing, event_manipulation, action_statement \
             FROM information_schema.triggers \
             WHERE event_object_schema = ? AND event_object_table = ? \
             ORDER BY trigger_name",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&mut *connection)
        .await
        .map_err(map_query_error)?;

        let mut triggers = Vec::with_capacity(trigger_rows.len());
        for row in &trigger_rows {
            triggers.push(TriggerDef {
                name: row.try_get(0).unwrap_or_default(),
                timing: row.try_get(1).unwrap_or_default(),
                event: row.try_get(2).unwrap_or_default(),
                statement: row.try_get(3).unwrap_or_default(),
            });
        }

        let option_row = sqlx::query(
            "SELECT engine, table_collation, table_comment, auto_increment \
             FROM information_schema.tables \
             WHERE table_schema = ? AND table_name = ?",
        )
        .bind(database)
        .bind(table)
        .fetch_optional(&mut *connection)
        .await
        .map_err(map_query_error)?;

        let options = match option_row {
            Some(row) => {
                let engine: Option<String> = row.try_get(0).unwrap_or(None);
                let collation: Option<String> = row.try_get(1).unwrap_or(None);
                let comment: String = row.try_get(2).unwrap_or_default();
                let auto_increment = optional_u64(&row, 3);
                let collation = collation.unwrap_or_default();
                let charset = collation
                    .split_once('_')
                    .map(|(charset, _)| charset.to_string())
                    .unwrap_or_default();
                TableOptions {
                    engine: engine.unwrap_or_default(),
                    charset,
                    collation,
                    comment,
                    auto_increment: auto_increment.map(|v| v.to_string()).unwrap_or_default(),
                }
            }
            None => TableOptions::default(),
        };

        // MySQL 8.0.16+/MariaDB report CHECK constraints in `information_schema`; an older server
        // (or one without the view) simply has none, so the query failure is treated as an empty
        // list rather than failing the whole designer load.
        let check_rows = sqlx::query(
            "SELECT tc.constraint_name, cc.check_clause \
             FROM information_schema.table_constraints AS tc \
             JOIN information_schema.check_constraints AS cc \
               ON cc.constraint_schema = tc.constraint_schema \
              AND cc.constraint_name = tc.constraint_name \
             WHERE tc.table_schema = ? AND tc.table_name = ? \
               AND tc.constraint_type = 'CHECK' \
             ORDER BY tc.constraint_name",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&mut *connection)
        .await
        .unwrap_or_default();
        let checks = check_rows
            .iter()
            .map(|row| CheckDef {
                name: row.try_get(0).unwrap_or_default(),
                expression: row.try_get(1).unwrap_or_default(),
            })
            .collect();

        let partition_rows = sqlx::query(
            "SELECT partition_name, partition_method, partition_expression, \
                    partition_description, subpartition_name, subpartition_method, \
                    subpartition_expression \
             FROM information_schema.partitions \
             WHERE table_schema = ? AND table_name = ? AND partition_name IS NOT NULL \
             ORDER BY partition_ordinal_position, subpartition_ordinal_position",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&mut *connection)
        .await
        .unwrap_or_default();
        let partitioning = partition_clause(&partition_rows);

        Ok(TableSchema {
            columns,
            indexes,
            unique_keys: Vec::new(),
            checks,
            excludes: Vec::new(),
            rules: Vec::new(),
            foreign_keys,
            triggers,
            partitioning,
            options,
        })
    }

    fn table_schema_sql(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> String {
        schema_sql(self.engine, database, table, original, modified)
    }

    fn table_schema_trigger_sql(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> Vec<String> {
        trigger_sql(database, table, original, modified)
    }

    async fn close(&self) -> Result<()> {
        self.pool.close().await;
        Ok(())
    }

    async fn list_users(&self) -> Result<Vec<UserAccount>> {
        crate::user::list_users(&self.pool).await
    }

    async fn roles_supported(&self) -> bool {
        crate::user::roles_supported(&self.pool).await
    }

    async fn user_details(&self, user: &str, host: &str) -> Result<UserDetails> {
        crate::user::user_details(&self.pool, user, host).await
    }

    fn user_edit_sql(&self, edit: &UserEdit) -> String {
        crate::user::edit_sql(edit)
    }

    fn user_edit_groups(
        &self,
        edit: &UserEdit,
    ) -> Vec<(rustgrid_core::UserEditSection, Vec<String>)> {
        crate::user::edit_groups(edit)
    }

    async fn save_user(&self, edit: &UserEdit) -> Result<()> {
        crate::user::save_user(&self.pool, edit).await
    }

    async fn drop_user(&self, user: &str, host: &str) -> Result<()> {
        crate::user::drop_user(&self.pool, user, host).await
    }

    async fn rename_user(
        &self,
        user: &str,
        host: &str,
        new_user: &str,
        new_host: &str,
    ) -> Result<()> {
        crate::user::rename_user(&self.pool, user, host, new_user, new_host).await
    }

    fn privilege_catalog(&self) -> PrivilegeCatalog {
        crate::user::privilege_catalog()
    }

    fn authentication_plugins(&self) -> Vec<&'static str> {
        crate::user::authentication_plugins()
    }

    fn ssl_types(&self) -> Vec<&'static str> {
        crate::user::ssl_types()
    }

    async fn object_privilege_matrix(
        &self,
        database: &str,
        name: &str,
    ) -> Result<Vec<ObjectPrivilegeRow>> {
        crate::user::object_privilege_matrix(&self.pool, database, name).await
    }

    async fn set_object_privileges(
        &self,
        database: &str,
        name: &str,
        rows: &[ObjectPrivilegeRow],
    ) -> Result<()> {
        crate::user::set_object_privileges(&self.pool, database, name, rows).await
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
        crate::routine::list_routine_infos(&self.pool, database).await
    }

    async fn routine_details(
        &self,
        database: &str,
        kind: RoutineKind,
        name: &str,
    ) -> Result<RoutineDetails> {
        crate::routine::routine_details(&self.pool, database, kind, name).await
    }

    fn routine_sql(
        &self,
        database: &str,
        original: Option<(&str, RoutineKind)>,
        edit: &RoutineEdit,
    ) -> String {
        crate::routine::routine_sql(database, original, edit)
    }

    async fn save_routine(
        &self,
        database: &str,
        original: Option<(&str, RoutineKind)>,
        edit: &RoutineEdit,
    ) -> Result<()> {
        crate::routine::save_routine(&self.pool, database, original, edit).await
    }

    async fn drop_routine(&self, database: &str, kind: RoutineKind, name: &str) -> Result<()> {
        crate::routine::drop_routine(&self.pool, database, kind, name).await
    }

    async fn view_details(&self, database: &str, name: &str) -> Result<ViewDetails> {
        crate::view::view_details(&self.pool, database, name).await
    }

    fn view_sql(&self, database: &str, original: Option<&str>, edit: &ViewEdit) -> String {
        crate::view::view_sql(database, original, edit)
    }

    async fn save_view(
        &self,
        database: &str,
        original: Option<&str>,
        edit: &ViewEdit,
    ) -> Result<()> {
        crate::view::save_view(&self.pool, database, original, edit).await
    }

    async fn drop_view(&self, database: &str, name: &str) -> Result<()> {
        crate::view::drop_view(&self.pool, database, name).await
    }
}

/// The `DROP`/`CREATE TRIGGER` statements that turn `original.triggers` into `modified.triggers`.
///
/// MySQL has no `ALTER TRIGGER`: a changed trigger is dropped and created again. The statements are
/// kept separate from the table DDL because a trigger body may contain semicolons and must be sent
/// to the server as one query.
fn trigger_sql(
    database: &str,
    table: &str,
    original: Option<&TableSchema>,
    modified: &TableSchema,
) -> Vec<String> {
    let mut statements = Vec::new();
    let original_triggers = original
        .map(|schema| schema.triggers.as_slice())
        .unwrap_or(&[]);
    for trigger in original_triggers {
        if trigger.name.trim().is_empty() {
            continue;
        }
        let unchanged = modified
            .triggers
            .iter()
            .find(|candidate| candidate.name == trigger.name)
            .is_some_and(|candidate| candidate == trigger);
        if !unchanged {
            statements.push(format!(
                "DROP TRIGGER IF EXISTS {}.{}",
                quote_identifier(database),
                quote_identifier(&trigger.name)
            ));
        }
    }
    for trigger in &modified.triggers {
        if trigger.name.trim().is_empty() || trigger.statement.trim().is_empty() {
            continue;
        }
        let unchanged = original
            .map(|schema| &schema.triggers)
            .is_some_and(|triggers| {
                triggers
                    .iter()
                    .find(|candidate| candidate.name == trigger.name)
                    .is_some_and(|candidate| candidate == trigger)
            });
        if unchanged {
            continue;
        }
        let timing = trigger.timing.trim().to_ascii_uppercase();
        let event = trigger.event.trim().to_ascii_uppercase();
        if timing.is_empty() || event.is_empty() {
            continue;
        }
        let body = trigger.statement.trim().trim_end_matches(';').trim_end();
        statements.push(format!(
            "CREATE TRIGGER {}.{} {timing} {event} ON {}.{} FOR EACH ROW {body}",
            quote_identifier(database),
            quote_identifier(&trigger.name),
            quote_identifier(database),
            quote_identifier(table),
        ));
    }
    statements
}

/// Rebuild the `PARTITION BY …` clause from `information_schema.partitions`.
///
/// Subpartitioned tables are deliberately left as an empty clause: the top-level rows alone cannot
/// describe the subpartition values, and writing a partial clause would repartition (and could lose)
/// the table when some unrelated property is saved. An empty clause means "leave partitioning
/// alone" — an existing partitioned table keeps its partitioning under a normal `ALTER TABLE`.
fn partition_clause(rows: &[MySqlRow]) -> String {
    let mut method = String::new();
    let mut expression = String::new();
    let mut subpartitioned = false;
    let mut partitions: Vec<(String, String)> = Vec::new();
    for row in rows {
        let name: Option<String> = row.try_get(0).unwrap_or(None);
        let Some(name) = name else { continue };
        if method.is_empty() {
            method = row
                .try_get::<Option<String>, _>(1)
                .unwrap_or(None)
                .unwrap_or_default();
            expression = row
                .try_get::<Option<String>, _>(2)
                .unwrap_or(None)
                .unwrap_or_default();
        }
        if row
            .try_get::<Option<String>, _>(4)
            .unwrap_or(None)
            .is_some()
        {
            subpartitioned = true;
        }
        if !partitions.iter().any(|(candidate, _)| candidate == &name) {
            let description: Option<String> = row.try_get(3).unwrap_or(None);
            partitions.push((name, description.unwrap_or_default()));
        }
    }
    if method.is_empty() || subpartitioned {
        return String::new();
    }
    let mut clause = format!("PARTITION BY {method} ({})", expression.trim());
    if method.contains("RANGE") || method.contains("LIST") {
        let definitions = partitions
            .iter()
            .map(|(name, description)| {
                let values = if method.contains("LIST") {
                    format!("IN ({description})")
                } else {
                    format!("LESS THAN ({description})")
                };
                format!("PARTITION {} VALUES {values}", quote_identifier(name))
            })
            .collect::<Vec<_>>()
            .join(", ");
        clause.push_str(&format!(" ({definitions})"));
    } else {
        clause.push_str(&format!(" PARTITIONS {}", partitions.len()));
    }
    clause
}

fn partition_by_sql(clause: &str) -> String {
    let clause = clause.trim().trim_end_matches(';').trim();
    if clause.to_ascii_uppercase().starts_with("PARTITION BY") {
        clause.to_string()
    } else {
        format!("PARTITION BY {clause}")
    }
}

/// Build the DDL that turns `original` into `modified`; exposed free so it can be unit tested
/// without a live pool.
fn schema_sql(
    engine: Engine,
    database: &str,
    table: &str,
    original: Option<&TableSchema>,
    modified: &TableSchema,
) -> String {
    let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(table));

    let Some(original) = original else {
        let mut parts: Vec<String> = modified.columns.iter().map(column_sql).collect();
        let primary = primary_columns(modified);
        if !primary.is_empty() {
            let columns = primary
                .iter()
                .map(|column| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("PRIMARY KEY ({columns})"));
        }
        for index in modified.indexes.iter().filter(|index| !index.primary) {
            parts.push(index_sql(index));
        }
        for check in &modified.checks {
            if !check.expression.trim().is_empty() {
                parts.push(check_sql(check));
            }
        }
        for foreign_key in &modified.foreign_keys {
            parts.push(foreign_key_sql(foreign_key));
        }
        let mut sql = format!("CREATE TABLE {qualified} (\n  {}\n)", parts.join(",\n  "));
        let options = table_options_sql(&modified.options);
        if !options.is_empty() {
            sql.push(' ');
            sql.push_str(&options);
        }
        if !modified.partitioning.trim().is_empty() {
            sql.push(' ');
            sql.push_str(&partition_by_sql(&modified.partitioning));
        }
        sql.push(';');
        return sql;
    };

    let mut clauses: Vec<String> = Vec::new();

    // Constraints are dropped before any column: MySQL refuses to drop a column a foreign key or
    // check still references. New/changed ones are re-added at the end, after the columns exist.
    for foreign_key in &original.foreign_keys {
        let unchanged = modified
            .foreign_keys
            .iter()
            .find(|candidate| candidate.name == foreign_key.name)
            .is_some_and(|candidate| candidate == foreign_key);
        if !unchanged {
            clauses.push(format!(
                "DROP FOREIGN KEY {}",
                quote_identifier(&foreign_key.name)
            ));
        }
    }
    for check in &original.checks {
        if check.name.trim().is_empty() {
            continue;
        }
        let unchanged = modified
            .checks
            .iter()
            .find(|candidate| candidate.name == check.name)
            .is_some_and(|candidate| candidate == check);
        if !unchanged {
            clauses.push(drop_check_sql(engine, &check.name));
        }
    }

    let original_primary = primary_columns(original);
    let modified_primary = primary_columns(modified);
    if original_primary != modified_primary && !original_primary.is_empty() {
        clauses.push("DROP PRIMARY KEY".to_string());
    }
    // An index that disappears or changes is dropped before its columns; otherwise MySQL may have
    // already removed it with `DROP COLUMN` and the explicit `DROP INDEX` would fail.
    for index in original.indexes.iter().filter(|index| !index.primary) {
        let unchanged = modified
            .indexes
            .iter()
            .find(|candidate| candidate.name == index.name && !candidate.primary)
            .is_some_and(|candidate| candidate == index);
        if !unchanged {
            clauses.push(format!("DROP INDEX {}", quote_identifier(&index.name)));
        }
    }

    let reordered = reordered_column_names(original, modified);

    for (position, column) in modified.columns.iter().enumerate() {
        let existing = original
            .columns
            .iter()
            .find(|candidate| candidate.name == column.name);
        let definition = column_sql(column);
        match existing {
            None => {
                let mut clause = format!("ADD COLUMN {definition}");
                clause.push_str(&position_clause(modified, position));
                clauses.push(clause);
            }
            Some(existing) if existing != column || reordered.contains(column.name.as_str()) => {
                // A pure re-order leaves the definition untouched, so the position clause is what
                // makes the move reach the server (`MODIFY ... AFTER` / `FIRST`).
                let mut clause = format!("MODIFY COLUMN {definition}");
                clause.push_str(&position_clause(modified, position));
                clauses.push(clause);
            }
            Some(_) => {}
        }
    }
    for column in &original.columns {
        if !modified.columns.iter().any(|c| c.name == column.name) {
            clauses.push(format!("DROP COLUMN {}", quote_identifier(&column.name)));
        }
    }

    if original_primary != modified_primary && !modified_primary.is_empty() {
        let columns = modified_primary
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        clauses.push(format!("ADD PRIMARY KEY ({columns})"));
    }

    for index in modified.indexes.iter().filter(|index| !index.primary) {
        let unchanged = original
            .indexes
            .iter()
            .find(|candidate| candidate.name == index.name && !candidate.primary)
            .is_some_and(|candidate| candidate == index);
        if !unchanged {
            clauses.push(format!("ADD {}", index_sql(index)));
        }
    }

    for foreign_key in &modified.foreign_keys {
        let unchanged = original
            .foreign_keys
            .iter()
            .find(|candidate| candidate.name == foreign_key.name)
            .is_some_and(|candidate| candidate == foreign_key);
        if !unchanged {
            clauses.push(format!("ADD {}", foreign_key_sql(foreign_key)));
        }
    }

    for check in &modified.checks {
        if check.name.trim().is_empty() || check.expression.trim().is_empty() {
            continue;
        }
        let unchanged = original
            .checks
            .iter()
            .find(|candidate| candidate.name == check.name)
            .is_some_and(|candidate| candidate == check);
        if !unchanged {
            clauses.push(format!("ADD {}", check_sql(check)));
        }
    }

    if original.options != modified.options {
        let options = table_options_sql(&modified.options);
        if !options.is_empty() {
            clauses.push(options);
        }
    }

    if original.partitioning != modified.partitioning {
        if modified.partitioning.trim().is_empty() {
            clauses.push("REMOVE PARTITIONING".to_string());
        } else {
            clauses.push(partition_by_sql(&modified.partitioning));
        }
    }

    if clauses.is_empty() {
        return String::new();
    }
    format!("ALTER TABLE {qualified}\n  {};", clauses.join(",\n  "))
}

/// The MySQL types offered by the table designer's type list.
const MYSQL_COLUMN_TYPES: [&str; 37] = [
    "bigint",
    "int",
    "mediumint",
    "smallint",
    "tinyint",
    "bit",
    "decimal",
    "numeric",
    "float",
    "double",
    "char",
    "varchar",
    "tinytext",
    "text",
    "mediumtext",
    "longtext",
    "binary",
    "varbinary",
    "tinyblob",
    "blob",
    "mediumblob",
    "longblob",
    "date",
    "time",
    "datetime",
    "timestamp",
    "year",
    "enum",
    "set",
    "json",
    "geometry",
    "point",
    "linestring",
    "polygon",
    "multipoint",
    "multilinestring",
    "multipolygon",
];

/// The MySQL storage engines offered by the table designer's Options tab. `InnoDB` is the default.
const MYSQL_STORAGE_ENGINES: [&str; 9] = [
    "InnoDB",
    "MyISAM",
    "MEMORY",
    "CSV",
    "ARCHIVE",
    "BLACKHOLE",
    "MRG_MYISAM",
    "FEDERATED",
    "PERFORMANCE_SCHEMA",
];

/// Read a nullable numeric `information_schema` column, tolerating both its signed and unsigned
/// `BIGINT` representations.
fn optional_u64(row: &MySqlRow, index: usize) -> Option<u64> {
    if let Ok(value) = row.try_get::<Option<u64>, _>(index) {
        return value;
    }
    if let Ok(value) = row.try_get::<Option<i64>, _>(index) {
        return value.filter(|value| *value >= 0).map(|value| value as u64);
    }
    None
}

/// Normalize a referential action (`NO ACTION` becomes an empty, i.e. default, rule).
fn normalize_rule(rule: &str) -> String {
    if rule.eq_ignore_ascii_case("NO ACTION") {
        String::new()
    } else {
        rule.to_string()
    }
}

/// The ` FIRST` / ` AFTER \`prev\`` suffix placing a column at `position` in `schema`.
fn position_clause(schema: &TableSchema, position: usize) -> String {
    if position == 0 {
        return " FIRST".to_string();
    }
    match schema.columns.get(position - 1) {
        Some(previous) => format!(" AFTER {}", quote_identifier(&previous.name)),
        None => String::new(),
    }
}

/// Columns present in both schemas whose relative order changed. Added or dropped columns do not
/// count: their own `ADD`/`DROP` clauses already establish their place.
fn reordered_column_names(
    original: &TableSchema,
    modified: &TableSchema,
) -> std::collections::HashSet<String> {
    let modified_names: std::collections::HashSet<&str> = modified
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    let shared: std::collections::HashSet<&str> = original
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .filter(|name| modified_names.contains(name))
        .collect();
    let original_prev = column_predecessors(&original.columns, &shared);
    let modified_prev = column_predecessors(&modified.columns, &shared);
    shared
        .into_iter()
        .filter(|name| original_prev.get(*name) != modified_prev.get(*name))
        .map(str::to_string)
        .collect()
}

/// For every shared column, the shared column immediately before it (owned, so the two schemas'
/// lifetimes do not have to line up).
fn column_predecessors(
    columns: &[ColumnDef],
    shared: &std::collections::HashSet<&str>,
) -> std::collections::HashMap<String, Option<String>> {
    let mut map = std::collections::HashMap::new();
    let mut previous: Option<String> = None;
    for column in columns {
        if shared.contains(column.name.as_str()) {
            map.insert(column.name.clone(), previous.clone());
            previous = Some(column.name.clone());
        }
    }
    map
}

fn primary_columns(schema: &TableSchema) -> Vec<String> {
    schema
        .columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.clone())
        .collect()
}

/// Whether a column type's `(length)` is part of its definition.
///
/// MySQL treats `TEXT(M)`/`BLOB(M)` as "create the smallest type that fits M characters/bytes", so
/// `text(65535)` under utf8mb4 becomes `mediumtext`. Those types therefore never carry a length:
/// their catalog bytes (`character_maximum_length`) are not a definition the server accepts back.
fn type_ignores_length(data_type: &str) -> bool {
    matches!(
        data_type.trim().to_ascii_lowercase().as_str(),
        "tinytext"
            | "text"
            | "mediumtext"
            | "longtext"
            | "tinyblob"
            | "blob"
            | "mediumblob"
            | "longblob"
    )
}

/// Render one column definition for `CREATE`/`ALTER TABLE`.
fn column_sql(column: &ColumnDef) -> String {
    let mut sql = quote_identifier(&column.name);
    sql.push(' ');
    sql.push_str(column.data_type.trim());

    let length = column.length.trim();
    let decimals = column.decimals.trim();
    if !length.is_empty() && length != "0" && !type_ignores_length(&column.data_type) {
        sql.push('(');
        sql.push_str(length);
        if !decimals.is_empty() && decimals != "0" {
            sql.push(',');
            sql.push_str(decimals);
        }
        sql.push(')');
    }

    if column.unsigned {
        sql.push_str(" unsigned");
    }
    if column.zerofill {
        sql.push_str(" zerofill");
    }
    if !column.charset.trim().is_empty() {
        sql.push_str(" CHARACTER SET ");
        sql.push_str(column.charset.trim());
    }
    if !column.collation.trim().is_empty() {
        sql.push_str(" COLLATE ");
        sql.push_str(column.collation.trim());
    }

    let generated = column.generated.trim();
    if !generated.is_empty() {
        sql.push_str(" GENERATED ALWAYS AS (");
        sql.push_str(generated);
        sql.push_str(if column.stored {
            ") STORED"
        } else {
            ") VIRTUAL"
        });
    }

    sql.push_str(if column.nullable {
        " NULL"
    } else {
        " NOT NULL"
    });

    if generated.is_empty() {
        if let Some(default) = format_default(column, &column.default) {
            sql.push(' ');
            sql.push_str(&default);
        }
        if column.auto_increment {
            sql.push_str(" AUTO_INCREMENT");
        }
    }
    if !column.extra.trim().is_empty() {
        sql.push(' ');
        sql.push_str(column.extra.trim());
    }
    if !column.comment.is_empty() {
        // A comment is compared verbatim by the structure diff, so its own leading/trailing
        // spaces are part of the definition and must be written back unchanged.
        sql.push_str(" COMMENT ");
        sql.push_str(&quote_literal(&column.comment));
    }
    sql
}

/// One column of an index with MySQL's per-column options: a prefix length (子部分) and/or a
/// descending sort.
pub(crate) fn index_column_sql(column: &IndexColumnDef) -> String {
    let mut sql = quote_identifier(&column.name);
    let prefix = column.prefix_length.trim();
    if !prefix.is_empty() {
        sql.push('(');
        sql.push_str(prefix);
        sql.push(')');
    }
    if column.descending {
        sql.push_str(" DESC");
    }
    sql
}

fn index_sql(index: &IndexDef) -> String {
    let columns = index
        .columns
        .iter()
        .map(index_column_sql)
        .collect::<Vec<_>>()
        .join(", ");
    if index.primary {
        return format!("PRIMARY KEY ({columns})");
    }

    let kind = match index.index_type.to_ascii_uppercase().as_str() {
        "FULLTEXT" => "FULLTEXT ",
        "SPATIAL" => "SPATIAL ",
        _ => "",
    };
    let unique = if index.unique && kind.is_empty() {
        "UNIQUE "
    } else {
        ""
    };
    let mut sql = format!(
        "{unique}{kind}INDEX {} ({columns})",
        quote_identifier(&index.name)
    );
    if !index.comment.trim().is_empty() {
        sql.push_str(" COMMENT ");
        sql.push_str(&quote_literal(index.comment.trim()));
    }
    sql
}

/// One `[CONSTRAINT name] CHECK (...)` clause of a `CREATE TABLE`.
fn check_sql(check: &CheckDef) -> String {
    let body = format!("CHECK ({})", check.expression.trim());
    if check.name.trim().is_empty() {
        body
    } else {
        format!("CONSTRAINT {} {body}", quote_identifier(&check.name))
    }
}

/// Drop a named check. MySQL spells it `DROP CHECK`; MariaDB spells it `DROP CONSTRAINT`.
fn drop_check_sql(engine: Engine, name: &str) -> String {
    let verb = match engine {
        Engine::MySql | Engine::StarRocks => "DROP CHECK",
        Engine::MariaDb => "DROP CONSTRAINT",
    };
    format!("{verb} {}", quote_identifier(name))
}

fn foreign_key_sql(foreign_key: &ForeignKeyDef) -> String {
    let columns = foreign_key
        .columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let referenced = foreign_key
        .referenced_columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let referenced_table = if foreign_key.referenced_schema.trim().is_empty() {
        quote_identifier(&foreign_key.referenced_table)
    } else {
        format!(
            "{}.{}",
            quote_identifier(foreign_key.referenced_schema.trim()),
            quote_identifier(&foreign_key.referenced_table)
        )
    };
    let constraint = if foreign_key.name.trim().is_empty() {
        String::new()
    } else {
        format!("CONSTRAINT {} ", quote_identifier(&foreign_key.name))
    };
    let mut sql =
        format!("{constraint}FOREIGN KEY ({columns}) REFERENCES {referenced_table} ({referenced})");
    if !foreign_key.on_delete.trim().is_empty() {
        sql.push_str(" ON DELETE ");
        sql.push_str(foreign_key.on_delete.trim());
    }
    if !foreign_key.on_update.trim().is_empty() {
        sql.push_str(" ON UPDATE ");
        sql.push_str(foreign_key.on_update.trim());
    }
    sql
}

fn table_options_sql(options: &TableOptions) -> String {
    let mut parts = Vec::new();
    if !options.engine.trim().is_empty() {
        parts.push(format!("ENGINE={}", options.engine.trim()));
    }
    if !options.charset.trim().is_empty() {
        parts.push(format!("DEFAULT CHARACTER SET {}", options.charset.trim()));
    }
    if !options.collation.trim().is_empty() {
        parts.push(format!("COLLATE {}", options.collation.trim()));
    }
    if !options.auto_increment.trim().is_empty() && options.auto_increment.trim() != "0" {
        parts.push(format!("AUTO_INCREMENT={}", options.auto_increment.trim()));
    }
    if !options.comment.is_empty() {
        // Like a column comment, the table comment's own spaces are part of the definition.
        parts.push(format!("COMMENT={}", quote_literal(&options.comment)));
    }
    parts.join(" ")
}

/// Render a `DEFAULT ...` fragment for a column, quoting values and leaving expressions and
/// numeric literals bare.
///
/// Whether a value is numeric comes from the **column's type**, not from the characters of the
/// value: MySQL reports a `varchar` default `'00'` as the text `00`, and writing it back unquoted
/// (`DEFAULT 00`) makes the server store the number 0 — so the target kept `'0'` and a structure
/// sync could never converge on a column whose string default was all digits.
///
/// The value is used verbatim: a default of a single space is a real default, and trimming it
/// dropped the `DEFAULT` clause entirely.
fn format_default(column: &ColumnDef, value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let upper = value.to_ascii_uppercase();
    if upper == "NULL" {
        return Some("DEFAULT NULL".to_string());
    }
    // `CURRENT_TIMESTAMP` & co. are only expressions for temporal columns; a `varchar` default
    // with that text is a literal.
    let temporal = matches!(
        column.data_type.trim().to_ascii_lowercase().as_str(),
        "timestamp" | "datetime" | "date" | "time"
    );
    let expression = (temporal
        && (upper.starts_with("CURRENT_TIMESTAMP")
            || upper.starts_with("CURRENT_DATE")
            || upper.starts_with("CURRENT_TIME")
            || upper.starts_with("NOW(")))
        || upper.starts_with("UUID(")
        || value.starts_with('(')
        || upper.starts_with("B'")
        || upper.starts_with("0X")
        // MariaDB reports a string default already quoted; MySQL does not.
        || (value.len() >= 2 && value.starts_with('\'') && value.ends_with('\''));
    if expression {
        return Some(format!("DEFAULT {value}"));
    }
    if type_is_numeric(&column.data_type) && value.trim().parse::<f64>().is_ok() {
        return Some(format!("DEFAULT {}", value.trim()));
    }
    Some(format!("DEFAULT {}", quote_literal(value)))
}

/// Whether a column of this type takes a numeric default, so it is written without quotes.
fn type_is_numeric(data_type: &str) -> bool {
    let data_type = data_type.trim().to_ascii_lowercase();
    // Listed explicitly rather than by suffix: `point`/`multipoint` are geometry types, and a
    // suffix test would read them as integers.
    matches!(
        data_type.as_str(),
        "tinyint"
            | "smallint"
            | "mediumint"
            | "int"
            | "integer"
            | "bigint"
            | "serial"
            | "decimal"
            | "numeric"
            | "dec"
            | "fixed"
            | "float"
            | "double"
            | "real"
            | "bit"
            | "year"
    )
}

fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub(crate) fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

/// Build the `ALTER DATABASE` script for the charset/collation that changed between `original`
/// and `modified`. Empty when neither changed (or the new value is blank).
fn alter_database_sql(
    name: &str,
    original: &DatabaseOptions,
    modified: &DatabaseOptions,
) -> String {
    let mut clauses = Vec::new();
    if modified.charset != original.charset && !modified.charset.trim().is_empty() {
        clauses.push(format!("CHARACTER SET {}", modified.charset.trim()));
    }
    if modified.collation != original.collation && !modified.collation.trim().is_empty() {
        clauses.push(format!("COLLATE {}", modified.collation.trim()));
    }
    if clauses.is_empty() {
        return String::new();
    }
    format!(
        "ALTER DATABASE {} {};",
        quote_identifier(name),
        clauses.join(" ")
    )
}

/// Read a text column by any of its candidate names (case-insensitive), tolerating a missing
/// column (older servers) or a `NULL` value. Temporal columns (`CREATED`, `LAST_ALTERED`) are
/// decoded as datetimes and formatted.
pub(crate) fn column_text(row: &MySqlRow, names: &[&str]) -> Option<String> {
    for name in names {
        let Some(index) = row
            .columns()
            .iter()
            .position(|column| column.name().eq_ignore_ascii_case(name))
        else {
            continue;
        };
        if let Ok(value) = row.try_get::<Option<String>, _>(index) {
            return value;
        }
        if let Ok(value) = row.try_get::<Option<DateTime<Utc>>, _>(index) {
            return value.map(|value| value.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string());
        }
    }
    None
}

/// Escape a string for use inside a single-quoted SQL literal (used by `SHOW ... LIKE '<name>'`,
/// which cannot bind parameters).
fn escape_string_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "''")
}

/// The display text of a cell, or `None` for `NULL`/binary values.
fn text_value(cell: &CellValue) -> Option<String> {
    match cell {
        CellValue::Null | CellValue::Bytes(_) => None,
        other => Some(other.as_display()),
    }
}

/// A `SHOW TABLE STATUS` row as the object list's overview. The column names are the server's own
/// (`Name`, `Engine`, `Rows`, ...), so both the single-table info pane and the bulk listing parse
/// them the same way.
fn table_status_from(columns: &[ColumnInfo], row: &[CellValue]) -> TableStatus {
    let value = |name: &str| -> Option<&CellValue> {
        columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case(name))
            .and_then(|index| row.get(index))
    };
    TableStatus {
        engine: value("Engine").and_then(text_value),
        rows: value("Rows").and_then(u64_value),
        auto_increment: value("Auto_increment").and_then(u64_value),
        row_format: value("Row_format").and_then(text_value),
        created: value("Create_time").and_then(text_value),
        updated: value("Update_time").and_then(text_value),
        checked: value("Check_time").and_then(text_value),
        index_length: value("Index_length").and_then(u64_value),
        data_length: value("Data_length").and_then(u64_value),
        max_data_length: value("Max_data_length").and_then(u64_value),
        data_free: value("Data_free").and_then(u64_value),
        collation: value("Collation").and_then(text_value),
        create_options: value("Create_options").and_then(text_value),
        comment: value("Comment").and_then(text_value),
        ttl: None,
        preview: None,
    }
}

/// The numeric value of a cell, or `None` when it is not a number.
fn u64_value(cell: &CellValue) -> Option<u64> {
    match cell {
        CellValue::Uint(value) => Some(*value),
        CellValue::Int(value) => u64::try_from(*value).ok(),
        CellValue::Float(value) => Some(*value as u64),
        CellValue::Text(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Rows per `INSERT` statement while restoring a table.
const RESTORE_INSERT_BATCH_ROWS: usize = 1_000;

/// Byte budget per `INSERT` statement while restoring a table, keeping a statement comfortably
/// under the server's `max_allowed_packet`.
const RESTORE_INSERT_BATCH_BYTES: usize = 4 * 1024 * 1024;

/// Drop a trailing semicolon and surrounding whitespace from a stored statement.
fn trim_statement(sql: &str) -> String {
    sql.trim().trim_end_matches(';').trim_end().to_string()
}

/// Run one statement on a pinned connection.
async fn exec_sql(connection: &mut MySqlConnection, sql: &str) -> Result<()> {
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string()))
        .execute(&mut *connection)
        .await
        .map_err(map_query_error)?;
    Ok(())
}

/// Restore one object over an already-pinned connection. Foreign key checks are disabled for the
/// duration and the inserts run inside one transaction; the caller resets the session afterwards.
async fn restore_object_on(
    connection: &mut MySqlConnection,
    database: &str,
    object: &ObjectDump,
) -> Result<()> {
    let qualified = format!(
        "{}.{}",
        quote_identifier(database),
        quote_identifier(&object.name)
    );
    let ddl = trim_statement(&object.ddl);

    // Function/view/event DDL from `SHOW CREATE` is unqualified, so pin the schema context.
    exec_sql(connection, &format!("USE {}", quote_identifier(database))).await?;

    match object.kind {
        BackupObjectKind::Table => {
            exec_sql(connection, "SET FOREIGN_KEY_CHECKS=0").await?;
            exec_sql(connection, &format!("DROP TABLE IF EXISTS {qualified}")).await?;
            if !ddl.is_empty() {
                exec_sql(connection, &ddl).await?;
            }
            if !object.fields.is_empty() && !object.rows.is_empty() {
                let columns = object
                    .fields
                    .iter()
                    .map(|field| quote_identifier(field))
                    .collect::<Vec<_>>()
                    .join(", ");
                exec_sql(connection, "START TRANSACTION").await?;
                let mut batch: Vec<&str> = Vec::new();
                let mut bytes = 0usize;
                for row in &object.rows {
                    if !batch.is_empty()
                        && (batch.len() >= RESTORE_INSERT_BATCH_ROWS
                            || bytes + row.len() + 2 > RESTORE_INSERT_BATCH_BYTES)
                    {
                        insert_batch(connection, &qualified, &columns, &batch).await?;
                        batch.clear();
                        bytes = 0;
                    }
                    bytes += row.len() + 2;
                    batch.push(row);
                }
                if !batch.is_empty() {
                    insert_batch(connection, &qualified, &columns, &batch).await?;
                }
                exec_sql(connection, "COMMIT").await?;
            }
            for trigger in &object.trigger_ddl {
                let trigger = trim_statement(trigger);
                if !trigger.is_empty() {
                    exec_sql(connection, &trigger).await?;
                }
            }
        }
        BackupObjectKind::View => {
            exec_sql(connection, &format!("DROP VIEW IF EXISTS {qualified}")).await?;
            if !ddl.is_empty() {
                exec_sql(connection, &ddl).await?;
            }
        }
        BackupObjectKind::Function => {
            // The DDL says whether this is a function or a procedure, and the correct kind must
            // be dropped; `DROP FUNCTION` on a procedure (and vice versa) errors.
            let is_procedure = ddl.to_ascii_uppercase().contains(" PROCEDURE ");
            let verb = if is_procedure {
                "PROCEDURE"
            } else {
                "FUNCTION"
            };
            exec_sql(connection, &format!("DROP {verb} IF EXISTS {qualified}")).await?;
            if !ddl.is_empty() {
                exec_sql(connection, &ddl).await?;
            }
        }
        BackupObjectKind::Event => {
            exec_sql(connection, &format!("DROP EVENT IF EXISTS {qualified}")).await?;
            if !ddl.is_empty() {
                exec_sql(connection, &ddl).await?;
            }
        }
    }
    Ok(())
}

/// Run one multi-row `INSERT` built from a batch of value tuples.
async fn insert_batch(
    connection: &mut MySqlConnection,
    qualified: &str,
    columns: &str,
    rows: &[&str],
) -> Result<()> {
    let sql = format!(
        "INSERT INTO {qualified} ({columns}) VALUES {}",
        rows.join(", ")
    );
    exec_sql(connection, &sql).await
}

/// Render one decoded cell as a SQL value literal. The column type is needed only for `BIT`,
/// whose literals are written as `b'...'`.
fn render_literal(value: &CellValue, column_type: &str) -> String {
    if column_type.to_ascii_lowercase().starts_with("bit") {
        let bits = match value {
            CellValue::Null => return "NULL".to_string(),
            CellValue::Bool(value) => u64::from(*value),
            CellValue::Int(value) => (*value).max(0) as u64,
            CellValue::Uint(value) => *value,
            CellValue::Float(value) => (*value).max(0.0) as u64,
            CellValue::Bytes(bytes) => bytes
                .iter()
                .fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte)),
            CellValue::Text(text) => text.parse().unwrap_or(0),
        };
        return format!("b'{bits:b}'");
    }

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
        CellValue::Text(text) => quote_mysql_string(text),
        CellValue::Bytes(bytes) => {
            let mut hex = String::with_capacity(bytes.len() * 2 + 2);
            hex.push_str("0x");
            for byte in bytes {
                hex.push_str(&format!("{byte:02X}"));
            }
            hex
        }
    }
}

/// Render one already-decoded cell string as a SQL literal for the text protocol. `None` is SQL
/// `NULL` (never an empty string), matching the grid's `CellValue::as_edit_value` contract.
fn edit_value_sql(value: &Option<String>) -> String {
    value
        .as_deref()
        .map(quote_mysql_string)
        .unwrap_or_else(|| "NULL".to_string())
}

/// Quote a string as a MySQL literal, escaping the characters MySQL treats specially inside
/// single quotes. The result is valid both in a restore `INSERT` and in an extracted script.
fn quote_mysql_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for character in value.chars() {
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
    }
    out.push('\'');
    out
}

/// One value in a statement generated by this module. The binary protocol binds it as a typed
/// parameter; the text protocol (used for MySQL-protocol proxies that reject `COM_STMT_PREPARE`)
/// renders it as a literal.
enum BoundValue<'a> {
    Text(&'a str),
    Number(i64),
}

impl BoundValue<'_> {
    fn literal(&self) -> String {
        match self {
            Self::Text(value) => bound_text_literal(value),
            Self::Number(value) => value.to_string(),
        }
    }
}

/// A bound string as a text-protocol literal. A UTF-8 hex literal with a charset introducer is
/// valid whatever the server's `sql_mode` says about quotes and backslashes, and behaves like the
/// bound string it replaces (a non-binary string in the connection's utf8mb4 charset).
fn bound_text_literal(value: &str) -> String {
    use std::fmt::Write as _;

    if value.is_empty() {
        return "''".to_string();
    }
    let mut out = String::with_capacity(value.len() * 2 + 10);
    out.push_str("_utf8mb4 0x");
    for byte in value.as_bytes() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Replace every `?` of a generated statement with the next bound value, leaving `?` inside
/// backtick-quoted identifiers or single-quoted literals alone. Only statements built by this
/// module are rendered here, so the placeholders are always the binds given alongside them.
fn text_statement(sql: &str, binds: &[BoundValue<'_>]) -> String {
    let mut out = String::with_capacity(sql.len() + 16);
    let mut binds = binds.iter();
    let mut quote: Option<char> = None;
    for character in sql.chars() {
        if let Some(delimiter) = quote {
            out.push(character);
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '`' => {
                quote = Some(character);
                out.push(character);
            }
            '?' => match binds.next() {
                Some(bind) => out.push_str(&bind.literal()),
                None => out.push(character),
            },
            _ => out.push(character),
        }
    }
    out
}

/// Build the ` ORDER BY ...` fragment for a page request, or an empty string when unsorted.
fn order_clause(page: &PageRequest) -> String {
    if page.order_by.is_empty() {
        return String::new();
    }
    let terms = page
        .order_by
        .iter()
        .map(|sort| {
            format!(
                "{} {}",
                quote_identifier(&sort.column),
                if sort.descending { "DESC" } else { "ASC" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(" ORDER BY {terms}")
}

/// Build the ` WHERE ...` fragment for a page request's filter tree, together with the values to
/// bind in placeholder order.
fn filter_clause(filter: &[FilterNode]) -> (String, Vec<String>) {
    let mut binds: Vec<String> = Vec::new();
    match filter_fragment(filter, &mut binds) {
        Some(expr) => (format!(" WHERE {expr}"), binds),
        None => (String::new(), Vec::new()),
    }
}

/// Render one sibling list as an `AND`/`OR` joined expression, or `None` when nothing enabled is
/// left to filter on. Binds are appended in placeholder order; a skipped node never adds any.
fn filter_fragment(filter: &[FilterNode], binds: &mut Vec<String>) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();

    for node in filter {
        let piece = match node {
            FilterNode::Condition(condition) => condition_piece(condition, binds),
            FilterNode::Group(group) => {
                if !group.enabled {
                    None
                } else {
                    filter_fragment(&group.children, binds).map(|inner| format!("({inner})"))
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

/// Render a single condition, or `None` when it is disabled/incomplete (in which case nothing is
/// bound either).
fn condition_piece(condition: &FilterCondition, binds: &mut Vec<String>) -> Option<String> {
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

    let column = quote_identifier(&condition.column);
    let piece = match operator {
        FilterOperator::Equal => {
            binds.push(condition.value.clone());
            format!("{column} = ?")
        }
        FilterOperator::NotEqual => {
            binds.push(condition.value.clone());
            format!("{column} <> ?")
        }
        FilterOperator::LessThan => {
            binds.push(condition.value.clone());
            format!("{column} < ?")
        }
        FilterOperator::LessOrEqual => {
            binds.push(condition.value.clone());
            format!("{column} <= ?")
        }
        FilterOperator::GreaterThan => {
            binds.push(condition.value.clone());
            format!("{column} > ?")
        }
        FilterOperator::GreaterOrEqual => {
            binds.push(condition.value.clone());
            format!("{column} >= ?")
        }
        FilterOperator::Contains => {
            binds.push(format!("%{}%", condition.value));
            format!("{column} LIKE ?")
        }
        FilterOperator::NotContains => {
            binds.push(format!("%{}%", condition.value));
            format!("{column} NOT LIKE ?")
        }
        FilterOperator::StartsWith => {
            binds.push(format!("{}%", condition.value));
            format!("{column} LIKE ?")
        }
        FilterOperator::NotStartsWith => {
            binds.push(format!("{}%", condition.value));
            format!("{column} NOT LIKE ?")
        }
        FilterOperator::EndsWith => {
            binds.push(format!("%{}", condition.value));
            format!("{column} LIKE ?")
        }
        FilterOperator::NotEndsWith => {
            binds.push(format!("%{}", condition.value));
            format!("{column} NOT LIKE ?")
        }
        FilterOperator::IsNull => format!("{column} IS NULL"),
        FilterOperator::IsNotNull => format!("{column} IS NOT NULL"),
        FilterOperator::IsEmpty => format!("({column} IS NULL OR {column} = '')"),
        FilterOperator::IsNotEmpty => format!("({column} IS NOT NULL AND {column} <> '')"),
        FilterOperator::Between => {
            binds.push(condition.value.clone());
            binds.push(condition.value2.clone());
            format!("{column} BETWEEN ? AND ?")
        }
        FilterOperator::NotBetween => {
            binds.push(condition.value.clone());
            binds.push(condition.value2.clone());
            format!("{column} NOT BETWEEN ? AND ?")
        }
        FilterOperator::InList => {
            let values = condition.list_values();
            if values.is_empty() {
                return None;
            }
            let placeholders = vec!["?"; values.len()].join(", ");
            binds.extend(values);
            format!("{column} IN ({placeholders})")
        }
        FilterOperator::NotInList => {
            let values = condition.list_values();
            if values.is_empty() {
                return None;
            }
            let placeholders = vec!["?"; values.len()].join(", ");
            binds.extend(values);
            format!("{column} NOT IN ({placeholders})")
        }
    };

    Some(piece)
}

/// Map a sqlx error to a Navicat-style message: `<code> - <message>` for MySQL server errors.
pub(crate) fn map_query_error(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(database_error) = &error
        && let Some(mysql_error) =
            database_error.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
    {
        return Error::Query(format!(
            "{} - {}",
            mysql_error.number(),
            mysql_error.message()
        ));
    }
    Error::Query(error.to_string())
}

/// Whether the server rejected a statement because it names a column the server does not have
/// (MySQL error 1054).
fn is_unknown_column_error(error: &sqlx::Error) -> bool {
    let sqlx::Error::Database(database_error) = error else {
        return false;
    };
    database_error
        .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
        .is_some_and(|mysql_error| mysql_error.number() == 1054)
}

/// Whether a MySQL-protocol middleware rejected the statement with
/// `1047 ER_UNKNOWN_COM_ERROR` ("Unknown command"). Proxies such as UDAL/TeleDB's DBProxy answer
/// statements their own parser cannot route with this generic error, which `map_query_error`
/// renders as `<code> - <message>`.
fn is_unknown_command_error(error: &sqlx::Error) -> bool {
    mysql_error_number(error) == Some(1047)
}

/// Whether the server (a middleware that routes each statement itself, such as UDAL/TeleDB's
/// DBProxy) refused the statement with `1046 ER_NO_DB_ERROR` because its session has no current
/// database. It answers that even for a fully qualified `information_schema` query, so the
/// database has to be selected first.
fn is_no_database_error(error: &sqlx::Error) -> bool {
    mysql_error_number(error) == Some(1046)
}

fn mysql_error_number(error: &sqlx::Error) -> Option<u16> {
    let sqlx::Error::Database(database_error) = error else {
        return None;
    };
    database_error
        .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
        .map(|mysql_error| mysql_error.number())
}

fn columns_from_row(row: &MySqlRow) -> Vec<ColumnInfo> {
    row.columns().iter().map(column_info).collect()
}

fn column_info(column: &MySqlColumn) -> ColumnInfo {
    ColumnInfo {
        name: column.name().to_string(),
        data_type: column.type_info().to_string(),
        nullable: true,
        primary_key: false,
        // Result-set metadata carries no catalog knowledge; an editable query grid re-attaches the
        // table's flags (`run_query` / the reload path).
        auto_increment: false,
        generated: false,
        comment: String::new(),
    }
}

async fn describe_columns(connection: &mut MySqlConnection, sql: &str) -> Result<Vec<ColumnInfo>> {
    let statement = connection
        .prepare(AssertSqlSafe(sql.to_string()).into_sql_str())
        .await
        .map_err(map_query_error)?;
    Ok(statement.columns().iter().map(column_info).collect())
}

/// Whether a statement is expected to produce a result set. MySQL has no way to know this
/// up front on the text protocol, so the leading keyword decides which execution path is used.
fn returns_result_set(sql: &str) -> bool {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(stripped) = rest.strip_prefix("--") {
            rest = stripped
                .split_once('\n')
                .map(|(_, after)| after)
                .unwrap_or("");
        } else if let Some(stripped) = rest.strip_prefix('#') {
            rest = stripped
                .split_once('\n')
                .map(|(_, after)| after)
                .unwrap_or("");
        } else if let Some(stripped) = rest.strip_prefix("/*") {
            rest = stripped
                .split_once("*/")
                .map(|(_, after)| after)
                .unwrap_or("");
        } else {
            break;
        }
    }

    let keyword: String = rest
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
        .collect();

    matches!(
        keyword.to_ascii_uppercase().as_str(),
        "SELECT"
            | "SHOW"
            | "DESCRIBE"
            | "DESC"
            | "EXPLAIN"
            | "WITH"
            | "VALUES"
            | "TABLE"
            | "CALL"
            | "ANALYZE"
            | "CHECK"
            | "HELP"
    )
}

/// Select `database` on an already-acquired connection so the following statement runs in its
/// context (and a middleware has the current database it demands).
async fn use_database(
    connection: &mut MySqlConnection,
    database: &str,
) -> std::result::Result<(), sqlx::Error> {
    let use_sql = format!("USE {}", quote_identifier(database));
    sqlx::raw_sql(sqlx::AssertSqlSafe(use_sql))
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// Run `sql` (one statement or a script) and decode every result set it returns, pairing each with
/// its statement when the counts line up.
async fn fetch_results(connection: &mut MySqlConnection, sql: &str) -> Result<Vec<QueryResult>> {
    // MySQL's protocol reports `SERVER_MORE_RESULTS_EXISTS` after each statement, which sqlx
    // surfaces through `fetch_many` as an `Either::Left` boundary between result sets. That is
    // what lets one script return several result sets without us having to split it (the server
    // still parses compound statements like `CREATE PROCEDURE ... BEGIN ...; ... END` itself).
    let mut stream =
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string())).fetch_many(&mut *connection);
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
                    last_insert_id: Some(done.last_insert_id()).filter(|id| *id != 0),
                });
            }
            Either::Right(row) => {
                if columns.is_empty() && rows.is_empty() {
                    columns = columns_from_row(&row);
                }
                let mut values = Vec::with_capacity(columns.len());
                for index in 0..columns.len() {
                    values.push(decode_cell(&row, index));
                }
                rows.push(values);
            }
        }
    }
    drop(stream);

    // Pair each result set with its originating statement when the counts line up, so a grid
    // can reload just its own statement. A zero-row `SELECT` yields no rows for sqlx to expose
    // its columns, so describe it (the `returns_result_set` keyword decides which sets need it).
    let statements = split_statements(sql);
    if results.len() == statements.len() {
        for (result, statement) in results.iter_mut().zip(statements.iter()) {
            result.statement = statement.clone();
            if !result.has_result_set
                && returns_result_set(statement)
                && let Ok(described) = describe_columns(connection, statement).await
            {
                result.columns = described;
                result.has_result_set = true;
            }
        }
    } else {
        // A `CALL` (one statement, several result sets) cannot be split per set; keep the whole
        // script as the source so a refresh re-runs the procedure.
        for result in results.iter_mut() {
            result.statement = sql.to_string();
        }
    }

    Ok(results)
}

/// Run one statement (the caller has already selected the database) and decode its single result.
async fn execute_one(connection: &mut MySqlConnection, sql: &str) -> Result<QueryResult> {
    if returns_result_set(sql) {
        let rows = sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string()))
            .fetch_all(&mut *connection)
            .await
            .map_err(map_query_error)?;

        let columns = match rows.first() {
            Some(first) => columns_from_row(first),
            None => describe_columns(connection, sql).await.unwrap_or_default(),
        };

        let mut decoded = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut values = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                values.push(decode_cell(row, index));
            }
            decoded.push(values);
        }

        Ok(QueryResult {
            statement: sql.to_string(),
            columns,
            rows: decoded,
            rows_affected: 0,
            has_result_set: true,
            last_insert_id: None,
        })
    } else {
        let result = sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string()))
            .execute(&mut *connection)
            .await
            .map_err(map_query_error)?;
        Ok(QueryResult {
            statement: sql.to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: result.rows_affected(),
            has_result_set: false,
            last_insert_id: Some(result.last_insert_id()).filter(|id| *id != 0),
        })
    }
}

/// Split a SQL script into individual statements on top-level semicolons, respecting string
/// literals, quoted identifiers and comments. Fragments that carry no executable SQL (empty or
/// comment-only) are dropped, so the count matches the number of statements MySQL executes.
///
/// This is only used to pair result sets with their statement text; execution still hands the whole
/// script to the server.
fn split_statements(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut statements = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < bytes.len() {
                    let byte = bytes[i];
                    if byte == b'\\' && quote != b'`' {
                        i += 2;
                        continue;
                    }
                    if byte == quote {
                        // A doubled quote is an escaped quote, not the end of the literal.
                        if i + 1 < bytes.len() && bytes[i + 1] == quote {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'#' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            b';' => {
                let fragment = sql[start..i].trim();
                if has_sql_content(fragment) {
                    statements.push(fragment.to_string());
                }
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    let fragment = sql[start..].trim();
    if has_sql_content(fragment) {
        statements.push(fragment.to_string());
    }
    statements
}

/// Whether a SQL fragment contains anything besides whitespace and comments.
fn has_sql_content(fragment: &str) -> bool {
    let bytes = fragment.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            byte if byte.is_ascii_whitespace() => i += 1,
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'#' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            _ => return true,
        }
    }
    false
}

fn decode_cell(row: &MySqlRow, index: usize) -> CellValue {
    let is_null = row
        .try_get_raw(index)
        .map(|raw| raw.is_null())
        .unwrap_or(true);

    if is_null {
        return CellValue::Null;
    }

    if let Ok(value) = row.try_get::<i64, _>(index) {
        return CellValue::Int(value);
    }

    if let Ok(value) = row.try_get::<u64, _>(index) {
        return CellValue::Uint(value);
    }

    if let Ok(value) = row.try_get::<f64, _>(index) {
        return CellValue::Float(value);
    }

    // `DateTime<Utc>` accepts both `DATETIME` and `TIMESTAMP`, while sqlx only considers
    // `NaiveDateTime` compatible with `DATETIME`. Without this, a `TIMESTAMP` read over the binary
    // protocol (the table grid's `fetch_page`) falls through to the raw bytes and shows as hex; the
    // query editor's text protocol hides that by returning an ASCII date. `naive_utc()` is the
    // wall-clock value the session returned, which is what the grid must show.
    if let Ok(value) = row.try_get::<DateTime<Utc>, _>(index) {
        return CellValue::Text(value.naive_utc().to_string());
    }

    if let Ok(value) = row.try_get::<NaiveDate, _>(index) {
        return CellValue::Text(value.to_string());
    }

    if let Ok(value) = row.try_get::<NaiveTime, _>(index) {
        return CellValue::Text(value.to_string());
    }

    if let Ok(value) = row.try_get::<String, _>(index) {
        return CellValue::Text(value);
    }

    match row.try_get_unchecked::<Vec<u8>, _>(index) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => CellValue::Text(text),
            Err(error) => CellValue::Bytes(error.into_bytes()),
        },
        Err(_) => CellValue::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Engine, column_sql, filter_clause, format_default, index_sql, order_clause,
        returns_result_set, schema_sql, split_statements, table_options_sql, trigger_sql,
    };
    use rustgrid_core::{
        CheckDef, ColumnDef, FilterCondition, FilterConjunction, FilterGroup, FilterNode,
        FilterOperator, IndexColumnDef, IndexDef, PageRequest, SortColumn, TableOptions,
        TableSchema, TriggerDef,
    };

    #[test]
    fn renders_index_columns_with_their_own_options() {
        let index = IndexDef {
            name: "idx_code".to_string(),
            columns: vec![
                IndexColumnDef {
                    name: "code".to_string(),
                    descending: true,
                    prefix_length: "10".to_string(),
                },
                IndexColumnDef::new("id"),
            ],
            unique: true,
            primary: false,
            index_type: "BTREE".to_string(),
            comment: String::new(),
        };
        assert_eq!(
            index_sql(&index),
            "UNIQUE INDEX `idx_code` (`code`(10) DESC, `id`)"
        );
    }

    #[test]
    fn renders_column_definitions() {
        let column = ColumnDef {
            name: "name".to_string(),
            data_type: "varchar".to_string(),
            length: "50".to_string(),
            nullable: false,
            default: "x".to_string(),
            comment: "the name".to_string(),
            ..Default::default()
        };
        assert_eq!(
            column_sql(&column),
            "`name` varchar(50) NOT NULL DEFAULT 'x' COMMENT 'the name'"
        );

        let unsigned = ColumnDef {
            name: "qty".to_string(),
            data_type: "int".to_string(),
            length: "11".to_string(),
            nullable: true,
            unsigned: true,
            auto_increment: true,
            ..Default::default()
        };
        assert_eq!(
            column_sql(&unsigned),
            "`qty` int(11) unsigned NULL AUTO_INCREMENT"
        );
    }

    #[test]
    fn formats_default_values() {
        let column = |data_type: &str| ColumnDef {
            name: "c".to_string(),
            data_type: data_type.to_string(),
            ..Default::default()
        };
        let varchar = column("varchar");
        let int = column("int");
        let timestamp = column("timestamp");

        assert_eq!(format_default(&varchar, ""), None);
        assert_eq!(
            format_default(&varchar, "NULL").as_deref(),
            Some("DEFAULT NULL")
        );
        assert_eq!(
            format_default(&timestamp, "CURRENT_TIMESTAMP").as_deref(),
            Some("DEFAULT CURRENT_TIMESTAMP")
        );
        assert_eq!(format_default(&int, "5").as_deref(), Some("DEFAULT 5"));
        assert_eq!(
            format_default(&varchar, "abc").as_deref(),
            Some("DEFAULT 'abc'")
        );
        // A numeric-looking string default stays a string: MySQL stores `DEFAULT 00` as `'0'`,
        // which left a synced varchar column different from its source forever.
        assert_eq!(
            format_default(&varchar, "00").as_deref(),
            Some("DEFAULT '00'")
        );
        assert_eq!(
            format_default(&varchar, "0").as_deref(),
            Some("DEFAULT '0'")
        );
        // A default that is only whitespace is a value, not "no default".
        assert_eq!(
            format_default(&varchar, " ").as_deref(),
            Some("DEFAULT ' '")
        );
    }

    #[test]
    fn keeps_comment_spaces_in_the_definition() {
        let column = ColumnDef {
            name: "flow".to_string(),
            data_type: "bigint".to_string(),
            comment: "monthly b ".to_string(),
            ..Default::default()
        };
        assert_eq!(
            column_sql(&column),
            "`flow` bigint NOT NULL COMMENT 'monthly b '"
        );

        let options = TableOptions {
            comment: " table note ".to_string(),
            ..Default::default()
        };
        assert_eq!(table_options_sql(&options), "COMMENT=' table note '");
    }

    #[test]
    fn alters_string_defaults_and_comments_so_they_converge() {
        // The pair a structure sync could never converge on: a `varchar` default of `00` (written
        // back as the number 0) and a comment whose trailing space was trimmed away.
        let original = TableSchema {
            columns: vec![
                ColumnDef {
                    name: "user_type".to_string(),
                    data_type: "varchar".to_string(),
                    length: "2".to_string(),
                    nullable: true,
                    default: "0".to_string(),
                    comment: "用户类型".to_string(),
                    ..Default::default()
                },
                ColumnDef {
                    name: "free_flow".to_string(),
                    data_type: "bigint".to_string(),
                    nullable: true,
                    comment: "月度流量 单位 b".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut modified = original.clone();
        modified.columns[0].default = "00".to_string();
        modified.columns[1].comment = "月度流量 单位 b ".to_string();

        let sql = schema_sql(Engine::MySql, "db", "t", Some(&original), &modified);
        assert!(
            sql.contains(
                "MODIFY COLUMN `user_type` varchar(2) NULL DEFAULT '00' COMMENT '用户类型'"
            ),
            "{sql}"
        );
        assert!(sql.contains("COMMENT '月度流量 单位 b '"), "{sql}");
    }

    #[test]
    fn builds_create_table_from_schema() {
        let schema = TableSchema {
            columns: vec![ColumnDef {
                name: "id".to_string(),
                data_type: "int".to_string(),
                nullable: false,
                primary_key: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let sql = schema_sql(Engine::MySql, "db", "t", None, &schema);
        assert!(sql.starts_with("CREATE TABLE `db`.`t`"));
        assert!(sql.contains("`id` int NOT NULL"));
        assert!(sql.contains("PRIMARY KEY (`id`)"));
    }

    #[test]
    fn distinguishes_an_empty_string_default_from_no_default() {
        let column = ColumnDef {
            name: "name".to_string(),
            data_type: "varchar".to_string(),
            length: "64".to_string(),
            default: "''".to_string(),
            ..Default::default()
        };
        assert_eq!(
            format_default(&column, &column.default),
            Some("DEFAULT ''".to_string())
        );
        assert_eq!(format_default(&column, ""), None);
    }

    #[test]
    fn creates_a_table_mapped_from_postgres() {
        use rustgrid_core::{DriverDialect, map_schema};

        let source = TableSchema {
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    data_type: "bigint".to_string(),
                    nullable: false,
                    primary_key: true,
                    auto_increment: true,
                    ..Default::default()
                },
                ColumnDef {
                    name: "name".to_string(),
                    data_type: "varchar".to_string(),
                    length: "255".to_string(),
                    ..Default::default()
                },
                ColumnDef {
                    name: "payload".to_string(),
                    data_type: "bytea".to_string(),
                    ..Default::default()
                },
                ColumnDef {
                    name: "created".to_string(),
                    data_type: "timestamptz".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mapped = map_schema(&source, DriverDialect::Postgres, DriverDialect::Mysql);
        let sql = schema_sql(Engine::MySql, "db", "users", None, &mapped);
        assert!(sql.contains("`id` bigint NOT NULL AUTO_INCREMENT"), "{sql}");
        assert!(sql.contains("`name` varchar(255)"), "{sql}");
        assert!(sql.contains("`payload` longblob"), "{sql}");
        assert!(sql.contains("`created` timestamp"), "{sql}");
    }

    #[test]
    fn renders_no_length_for_text_and_blob_types() {
        let column = |data_type: &str, length: &str| ColumnDef {
            name: "c".to_string(),
            data_type: data_type.to_string(),
            length: length.to_string(),
            nullable: true,
            ..Default::default()
        };
        // `text(65535)` makes MySQL choose the smallest type that fits 65535 characters, which is
        // `mediumtext` under utf8mb4 — the sync could never converge on it.
        assert_eq!(column_sql(&column("text", "65535")), "`c` text NULL");
        assert_eq!(
            column_sql(&column("mediumtext", "16777215")),
            "`c` mediumtext NULL"
        );
        assert_eq!(column_sql(&column("blob", "65535")), "`c` blob NULL");
        assert_eq!(
            column_sql(&column("varchar", "255")),
            "`c` varchar(255) NULL"
        );
    }

    #[test]
    fn builds_alter_for_schema_changes() {
        let original = TableSchema {
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    data_type: "int".to_string(),
                    nullable: false,
                    primary_key: true,
                    ..Default::default()
                },
                ColumnDef {
                    name: "name".to_string(),
                    data_type: "varchar".to_string(),
                    length: "50".to_string(),
                    nullable: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut modified = original.clone();
        modified.columns[1].length = "100".to_string();
        modified.columns.push(ColumnDef {
            name: "age".to_string(),
            data_type: "int".to_string(),
            nullable: true,
            ..Default::default()
        });

        let sql = schema_sql(Engine::MySql, "db", "t", Some(&original), &modified);
        assert!(sql.starts_with("ALTER TABLE `db`.`t`"));
        assert!(sql.contains("MODIFY COLUMN `name` varchar(100) NULL AFTER `id`"));
        assert!(sql.contains("ADD COLUMN `age` int NULL AFTER `name`"));
    }

    #[test]
    fn builds_alter_for_a_pure_column_reorder() {
        let original = TableSchema {
            columns: ["id", "password", "uid", "sex", "avatar"]
                .into_iter()
                .map(|name| ColumnDef {
                    name: name.to_string(),
                    data_type: "varchar".to_string(),
                    length: "10".to_string(),
                    nullable: true,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let mut modified = original.clone();
        modified.columns = vec![
            original.columns[0].clone(),
            original.columns[2].clone(),
            original.columns[3].clone(),
            original.columns[4].clone(),
            original.columns[1].clone(),
        ];

        // The definitions are identical, so only the position change can make the SQL non-empty.
        let sql = schema_sql(Engine::MySql, "db", "t", Some(&original), &modified);
        assert!(!sql.trim().is_empty());
        assert!(sql.contains("MODIFY COLUMN `uid` varchar(10) NULL AFTER `id`"));
        assert!(sql.contains("MODIFY COLUMN `password` varchar(10) NULL AFTER `avatar`"));

        // Adding a column in front moves no existing one, so it must not emit MODIFY clauses.
        let mut added = original.clone();
        added.columns.insert(
            0,
            ColumnDef {
                name: "age".to_string(),
                data_type: "int".to_string(),
                nullable: true,
                ..Default::default()
            },
        );
        let sql = schema_sql(Engine::MySql, "db", "t", Some(&original), &added);
        assert!(sql.contains("ADD COLUMN"));
        assert!(!sql.contains("MODIFY COLUMN"), "{sql}");
    }

    #[test]
    fn builds_trigger_diff() {
        let original = TableSchema {
            triggers: vec![TriggerDef {
                name: "trg".to_string(),
                timing: "BEFORE".to_string(),
                event: "INSERT".to_string(),
                statement: "SET @x = 1".to_string(),
            }],
            ..Default::default()
        };
        let mut modified = original.clone();
        modified.triggers[0].statement = "SET @x = 2".to_string();

        let sql = trigger_sql("db", "t", Some(&original), &modified);
        assert_eq!(sql[0], "DROP TRIGGER IF EXISTS `db`.`trg`");
        assert!(
            sql[1].starts_with(
                "CREATE TRIGGER `db`.`trg` BEFORE INSERT ON `db`.`t` FOR EACH ROW SET @x = 2"
            ),
            "{sql:?}"
        );

        // A new trigger is created without a preceding drop.
        let added = trigger_sql("db", "t", None, &modified);
        assert_eq!(added.len(), 1);
        assert!(added[0].starts_with("CREATE TRIGGER"));
    }

    #[test]
    fn creates_alters_and_removes_partitioning() {
        let mut schema = TableSchema {
            columns: vec![ColumnDef {
                name: "id".to_string(),
                data_type: "int".to_string(),
                nullable: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        schema.partitioning =
            "PARTITION BY RANGE (`id`) (PARTITION p0 VALUES LESS THAN (10))".to_string();

        let sql = schema_sql(Engine::MySql, "db", "t", None, &schema);
        assert!(
            sql.contains("PARTITION BY RANGE (`id`) (PARTITION p0 VALUES LESS THAN (10))"),
            "{sql}"
        );

        let original = schema.clone();
        let mut modified = original.clone();
        modified.partitioning = "PARTITION BY HASH (`id`) PARTITIONS 4".to_string();
        let sql = schema_sql(Engine::MySql, "db", "t", Some(&original), &modified);
        assert!(
            sql.contains("PARTITION BY HASH (`id`) PARTITIONS 4"),
            "{sql}"
        );

        modified.partitioning.clear();
        let sql = schema_sql(Engine::MySql, "db", "t", Some(&original), &modified);
        assert!(sql.contains("REMOVE PARTITIONING"), "{sql}");

        // A clause typed without the leading PARTITION BY still renders correctly.
        modified.partitioning = "HASH (`id`) PARTITIONS 2".to_string();
        let sql = schema_sql(Engine::MySql, "db", "t", Some(&original), &modified);
        assert!(
            sql.contains("PARTITION BY HASH (`id`) PARTITIONS 2"),
            "{sql}"
        );
    }

    #[test]
    fn creates_and_alters_check_constraints() {
        let mut schema = TableSchema {
            columns: vec![ColumnDef {
                name: "age".to_string(),
                data_type: "int".to_string(),
                nullable: false,
                ..Default::default()
            }],
            checks: vec![CheckDef {
                name: "ck_age".to_string(),
                expression: "`age` >= 0".to_string(),
            }],
            ..Default::default()
        };
        let sql = schema_sql(Engine::MySql, "db", "t", None, &schema);
        assert!(
            sql.contains("CONSTRAINT `ck_age` CHECK (`age` >= 0)"),
            "{sql}"
        );

        let original = schema.clone();
        schema.checks[0].expression = "`age` > 0".to_string();
        let sql = schema_sql(Engine::MySql, "db", "t", Some(&original), &schema);
        assert!(sql.contains("DROP CHECK `ck_age`"), "{sql}");
        assert!(
            sql.contains("ADD CONSTRAINT `ck_age` CHECK (`age` > 0)"),
            "{sql}"
        );

        // MariaDB uses `DROP CONSTRAINT` for the same operation.
        let sql = schema_sql(Engine::MariaDb, "db", "t", Some(&original), &schema);
        assert!(sql.contains("DROP CONSTRAINT `ck_age`"), "{sql}");
    }

    #[test]
    fn emits_nothing_when_schema_unchanged() {
        let schema = TableSchema {
            columns: vec![ColumnDef {
                name: "id".to_string(),
                data_type: "int".to_string(),
                nullable: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(schema_sql(Engine::MySql, "db", "t", Some(&schema), &schema).is_empty());
    }

    fn condition(column: &str, operator: FilterOperator, value: &str) -> FilterCondition {
        FilterCondition {
            column: column.to_string(),
            operator,
            value: value.to_string(),
            value2: String::new(),
            conjunction: FilterConjunction::And,
            enabled: true,
        }
    }

    fn node(column: &str, operator: FilterOperator, value: &str) -> FilterNode {
        FilterNode::Condition(condition(column, operator, value))
    }

    #[test]
    fn builds_order_by_from_sort_columns() {
        let sorted = PageRequest::new(0, 10).with_order_by(vec![
            SortColumn {
                column: "id".to_string(),
                descending: false,
            },
            SortColumn {
                column: "select".to_string(),
                descending: true,
            },
        ]);
        assert_eq!(order_clause(&sorted), " ORDER BY `id` ASC, `select` DESC");
    }

    #[test]
    fn order_by_is_empty_when_unsorted() {
        assert_eq!(order_clause(&PageRequest::new(0, 10)), "");
    }

    #[test]
    fn builds_where_with_bound_values() {
        let mut filter = vec![
            node("name", FilterOperator::Contains, "ali"),
            node("age", FilterOperator::GreaterOrEqual, "18"),
        ];
        filter[1].set_conjunction(FilterConjunction::Or);
        let (clause, binds) = filter_clause(&filter);
        assert_eq!(clause, " WHERE `name` LIKE ? OR `age` >= ?".to_string());
        assert_eq!(binds, vec!["%ali%".to_string(), "18".to_string()]);
    }

    #[test]
    fn builds_where_with_nested_groups() {
        let mut first = FilterGroup::new(FilterConjunction::And);
        first.children = vec![
            node("groupName", FilterOperator::Equal, "A"),
            node("type", FilterOperator::Equal, "字符"),
        ];
        let mut second = FilterGroup::new(FilterConjunction::Or);
        second.children = vec![
            node("groupName", FilterOperator::Equal, "B"),
            node("type", FilterOperator::Equal, "整数"),
        ];
        let filter = vec![FilterNode::Group(first), FilterNode::Group(second)];
        let (clause, binds) = filter_clause(&filter);
        assert_eq!(
            clause,
            " WHERE (`groupName` = ? AND `type` = ?) OR (`groupName` = ? AND `type` = ?)"
        );
        assert_eq!(binds, vec!["A", "字符", "B", "整数"]);
    }

    #[test]
    fn nested_group_children_keep_binds_in_order() {
        let mut group = FilterGroup::new(FilterConjunction::And);
        group.children = vec![
            node("a", FilterOperator::Equal, "1"),
            node("b", FilterOperator::IsNull, ""),
            node("c", FilterOperator::Between, "2"),
        ];
        if let FilterNode::Condition(condition) = &mut group.children[2] {
            condition.value2 = "3".to_string();
        }
        let (clause, binds) = filter_clause(&[FilterNode::Group(group)]);
        assert_eq!(
            clause,
            " WHERE (`a` = ? AND `b` IS NULL AND `c` BETWEEN ? AND ?)"
        );
        assert_eq!(binds, vec!["1", "2", "3"]);
    }

    #[test]
    fn filter_skips_disabled_and_valueless_conditions() {
        let mut filter = vec![
            node("a", FilterOperator::IsNull, ""),
            node("b", FilterOperator::Equal, ""),
        ];
        filter[0].set_enabled(false);
        assert_eq!(filter_clause(&filter), (String::new(), Vec::new()));

        let between = FilterCondition {
            column: "c".to_string(),
            operator: FilterOperator::Between,
            value: "1".to_string(),
            value2: "2".to_string(),
            conjunction: FilterConjunction::And,
            enabled: true,
        };
        let (clause, binds) = filter_clause(&[FilterNode::Condition(between)]);
        assert_eq!(clause, " WHERE `c` BETWEEN ? AND ?".to_string());
        assert_eq!(binds, vec!["1".to_string(), "2".to_string()]);
    }

    #[test]
    fn empty_group_is_dropped() {
        let group = FilterGroup::new(FilterConjunction::And);
        let filter = vec![
            node("a", FilterOperator::Equal, "1"),
            FilterNode::Group(group),
        ];
        let (clause, binds) = filter_clause(&filter);
        assert_eq!(clause, " WHERE `a` = ?".to_string());
        assert_eq!(binds, vec!["1"]);
    }

    #[test]
    fn filter_in_list_splits_values() {
        let condition = node("id", FilterOperator::InList, "1, 2 ,3");
        let (clause, binds) = filter_clause(&[condition]);
        assert_eq!(clause, " WHERE `id` IN (?, ?, ?)".to_string());
        assert_eq!(binds, vec!["1", "2", "3"]);
    }

    #[test]
    fn detects_result_set_statements() {
        assert!(returns_result_set("SELECT 1"));
        assert!(returns_result_set("  select * from t"));
        assert!(returns_result_set("-- comment\nSELECT 1"));
        assert!(returns_result_set("/* block */ SHOW TABLES"));
        assert!(returns_result_set("# comment\nEXPLAIN SELECT 1"));
        assert!(returns_result_set(
            "WITH cte AS (SELECT 1) SELECT * FROM cte"
        ));

        assert!(!returns_result_set("UPDATE t SET a = 1"));
        assert!(!returns_result_set("INSERT INTO t VALUES (1)"));
        assert!(!returns_result_set("DELETE FROM t"));
        assert!(!returns_result_set("CREATE TABLE t (a INT)"));
        assert!(!returns_result_set("USE db"));
    }

    #[test]
    fn splits_statements_on_top_level_semicolons() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2"),
            vec!["SELECT 1", "SELECT 2"]
        );
        // A trailing semicolon and surrounding whitespace do not add an empty statement.
        assert_eq!(split_statements("  SELECT 1 ; \n"), vec!["SELECT 1"]);
        // Semicolons inside string literals, quoted identifiers and comments are not boundaries.
        assert_eq!(
            split_statements("SELECT ';'; SELECT `a;b`"),
            vec!["SELECT ';'", "SELECT `a;b`"]
        );
        assert_eq!(
            split_statements("SELECT 1 -- a; b\n; SELECT 2"),
            vec!["SELECT 1 -- a; b", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT 'it''s'; SELECT \"x;y\""),
            vec!["SELECT 'it''s'", "SELECT \"x;y\""]
        );
        // Comment-only fragments are dropped.
        assert_eq!(
            split_statements("-- just a comment\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            split_statements("SELECT 1; -- trailing\n"),
            vec!["SELECT 1"]
        );
    }
}
