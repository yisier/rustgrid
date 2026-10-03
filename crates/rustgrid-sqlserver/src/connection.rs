use std::borrow::Cow;
use std::collections::BTreeSet;

use async_trait::async_trait;
use futures_util::TryStreamExt;
use rustgrid_core::{
    BackupObjectKind, CellValue, ColumnDef, ColumnInfo, Connection, DatabaseInfo, DriverId, Error,
    ForeignKeyDef, IndexDef, ObjectDump, ObjectKind, ObjectPrivilegeRow, PageRequest, QueryResult,
    Result, RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind, RowInsert, RowUpdate, TableInfo,
    TableOptions, TablePage, TableSchema, TableStatus, TriggerDef, UserAccount, UserDetails,
    UserEdit, UserEditSection, ViewDetails, ViewEdit, ViewInfo,
};
use tiberius::{QueryItem, Row, ToSql};

use crate::driver::map_query_error;
use crate::helpers::{
    decode_cell, filter_clause, is_dml, leading_keyword, order_clause, qualified_name,
    quote_identifier, render_literal, returns_result_set, split_statements,
};
use crate::pool::{ConnectionManager, SqlClient};
use crate::user;

/// The default schema SQL Server objects live in.
const DEFAULT_SCHEMA: &str = "dbo";

/// A connected SQL Server session pool, implementing the engine-agnostic [`Connection`].
pub struct SqlServerConnection {
    pub(crate) pool: bb8::Pool<ConnectionManager>,
}

impl SqlServerConnection {
    pub(crate) fn new(pool: bb8::Pool<ConnectionManager>) -> Self {
        Self { pool }
    }

    /// Check out a pooled client for the duration of one operation.
    pub(crate) async fn acquire(&self) -> Result<bb8::PooledConnection<'_, ConnectionManager>> {
        self.pool
            .get()
            .await
            .map_err(|error| Error::Connection(error.to_string()))
    }

    /// Run one statement on its own pooled client, returning its result.
    pub(crate) async fn run(
        &self,
        database: Option<&str>,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<QueryResult> {
        let mut client = self.acquire().await?;
        if let Some(database) = database {
            use_database(&mut client, database).await?;
        }
        run_statement(&mut client, sql, params).await
    }

    /// The first column of the first row of a query, as text.
    async fn scalar_text(
        &self,
        database: Option<&str>,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<Option<String>> {
        let result = self.run(database, sql, params).await?;
        Ok(result
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|value| match value {
                CellValue::Null => None,
                other => Some(other.as_display()),
            }))
    }

    /// Resolve a possibly schema-qualified object name into `(schema, bare_name)`.
    ///
    /// `list_tables` returns names as `schema.name` (e.g. `dbo.users`) so the tree can show
    /// which schema a table lives in. A name without a dot falls back to the schema that owns
    /// it, preferring `dbo`.
    pub(crate) async fn resolve_object(
        &self,
        database: &str,
        name: &str,
    ) -> Result<(String, String)> {
        if let Some((schema, bare)) = split_schema(name) {
            return Ok((schema.to_string(), bare.to_string()));
        }
        Ok((self.lookup_schema(database, name).await?, name.to_string()))
    }

    /// The schema owning `table` in `database`, preferring `dbo`.
    async fn lookup_schema(&self, database: &str, table: &str) -> Result<String> {
        let sql = format!(
            "SELECT TOP 1 s.name FROM {db}.sys.objects o \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.name = @P1 AND o.type IN ('U', 'V') \
             ORDER BY CASE WHEN s.name = 'dbo' THEN 0 ELSE 1 END, s.name",
            db = quote_identifier(database)
        );
        let schema = self
            .scalar_text(None, &sql, &[Some(table.to_string())])
            .await?;
        Ok(schema
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_SCHEMA.to_string()))
    }

    /// Whether a table has an `IDENTITY` column, needed for `IDENTITY_INSERT` on restore.
    pub(crate) async fn has_identity(
        &self,
        database: &str,
        schema: &str,
        table: &str,
    ) -> Result<bool> {
        let sql = format!(
            "SELECT COUNT(*) FROM {db}.sys.columns c \
             JOIN {db}.sys.objects o ON o.object_id = c.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.name = @P1 AND s.name = @P2 AND c.is_identity = 1",
            db = quote_identifier(database)
        );
        let count = self
            .scalar_text(
                None,
                &sql,
                &[Some(table.to_string()), Some(schema.to_string())],
            )
            .await?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        Ok(count > 0)
    }

    /// Execute one non-query statement on a fresh client (DDL, maintenance).
    pub(crate) async fn execute_batch(&self, database: Option<&str>, sql: &str) -> Result<()> {
        self.run(database, sql, &[]).await.map(|_| ())
    }
}

#[async_trait]
impl Connection for SqlServerConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new("sqlserver")
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>> {
        // `database_id > 4` hides the system databases (master/tempdb/model/msdb), matching
        // what other clients show. A user can still pick one explicitly as the profile's
        // default database.
        let result = self
            .run(
                None,
                "SELECT name FROM sys.databases WHERE state = 0 AND database_id > 4 ORDER BY name",
                &[],
            )
            .await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|row| row.first().map(CellValue::as_display))
            .map(|name| DatabaseInfo { name })
            .collect())
    }

    async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>> {
        // Names are returned as `schema.name` (e.g. `dbo.users`); `resolve_object` splits the
        // schema back off for every table operation.
        let sql = format!(
            "SELECT t.TABLE_SCHEMA, t.TABLE_NAME, t.TABLE_TYPE, v.IS_UPDATABLE \
             FROM {db}.INFORMATION_SCHEMA.TABLES t \
             LEFT JOIN {db}.INFORMATION_SCHEMA.VIEWS v \
               ON v.TABLE_SCHEMA = t.TABLE_SCHEMA AND v.TABLE_NAME = t.TABLE_NAME \
             ORDER BY t.TABLE_SCHEMA, t.TABLE_NAME",
            db = quote_identifier(database)
        );
        let result = self.run(None, &sql, &[]).await?;
        let mut tables = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            let name = format!("{}.{}", text_cell(row, 0), text_cell(row, 1));
            let table_type = text_cell(row, 2);
            let updatable = text_cell(row, 3).eq_ignore_ascii_case("YES");
            let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                ObjectKind::View
            } else {
                ObjectKind::Table
            };
            tables.push(TableInfo {
                name,
                kind,
                updatable: kind == ObjectKind::View && updatable,
            });
        }
        Ok(tables)
    }

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        let (schema, table) = self.resolve_object(database, table).await?;
        let sql = format!(
            "SELECT c.COLUMN_NAME, c.DATA_TYPE, c.IS_NULLABLE, \
                    c.CHARACTER_MAXIMUM_LENGTH, c.NUMERIC_PRECISION, c.NUMERIC_SCALE, \
                    c.DATETIME_PRECISION, \
                    CASE WHEN pk.COLUMN_NAME IS NULL THEN 0 ELSE 1 END AS is_pk \
             FROM {db}.INFORMATION_SCHEMA.COLUMNS c \
             LEFT JOIN ( \
                 SELECT ku.TABLE_SCHEMA, ku.TABLE_NAME, ku.COLUMN_NAME \
                 FROM {db}.INFORMATION_SCHEMA.KEY_COLUMN_USAGE ku \
                 JOIN {db}.INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc \
                   ON tc.CONSTRAINT_NAME = ku.CONSTRAINT_NAME \
                  AND tc.TABLE_SCHEMA = ku.TABLE_SCHEMA AND tc.TABLE_NAME = ku.TABLE_NAME \
                 WHERE tc.CONSTRAINT_TYPE = 'PRIMARY KEY' \
             ) pk ON pk.TABLE_SCHEMA = c.TABLE_SCHEMA \
                 AND pk.TABLE_NAME = c.TABLE_NAME AND pk.COLUMN_NAME = c.COLUMN_NAME \
             WHERE c.TABLE_SCHEMA = @P1 AND c.TABLE_NAME = @P2 \
             ORDER BY c.ORDINAL_POSITION",
            db = quote_identifier(database)
        );
        let result = self.run(None, &sql, &[Some(schema), Some(table)]).await?;
        let mut columns = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            let data_type = text_cell(row, 1);
            let length = text_cell(row, 3);
            let precision = text_cell(row, 4);
            let scale = text_cell(row, 5);
            let datetime_precision = text_cell(row, 6);
            columns.push(ColumnInfo {
                name: text_cell(row, 0),
                data_type: display_type(
                    &data_type,
                    &length,
                    &precision,
                    &scale,
                    &datetime_precision,
                ),
                nullable: text_cell(row, 2).eq_ignore_ascii_case("YES"),
                primary_key: text_cell(row, 7) == "1",
                comment: String::new(),
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
        let qualified = qualified_name(database, &schema, &bare);
        let (where_clause, filter_values) = filter_clause(&page.filter);
        let filter_params: Vec<Option<String>> = filter_values.into_iter().map(Some).collect();

        let total_rows = self
            .scalar_text(
                None,
                &format!("SELECT COUNT(*) FROM {qualified}{where_clause}"),
                &filter_params,
            )
            .await
            .ok()
            .flatten()
            .and_then(|value| value.parse::<u64>().ok());

        let order = if page.order_by.is_empty() {
            let primary_keys: Vec<String> = columns
                .iter()
                .filter(|column| column.primary_key)
                .map(|column| quote_identifier(&column.name))
                .collect();
            if primary_keys.is_empty() {
                " ORDER BY (SELECT NULL)".to_string()
            } else {
                format!(" ORDER BY {}", primary_keys.join(", "))
            }
        } else {
            order_clause(&page)
        };

        let sql = format!(
            "SELECT * FROM {qualified}{where_clause}{order} \
             OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
            page.offset(),
            page.page_size
        );
        let result = self.run(None, &sql, &filter_params).await?;

        Ok(TablePage {
            columns,
            rows: result.rows,
            page: page.page,
            page_size: page.page_size,
            total_rows,
        })
    }

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let (schema, bare) = self.resolve_object(database, table).await?;
        let qualified = qualified_name(database, &schema, &bare);
        let mut client = self.acquire().await?;
        use_database(&mut client, database).await?;
        client.begin_transaction().await.map_err(map_query_error)?;

        for update in updates {
            if update.set.is_empty() {
                continue;
            }
            let (assignments, params) = assignment_clause(&update.set);
            let (keys, key_params) = key_match_clause(&update.keys, params.len());
            let sql = format!("UPDATE {qualified} SET {assignments} WHERE {keys}");
            if let Err(error) =
                run_statement(&mut client, &sql, &[params, key_params].concat()).await
            {
                let _ = client.rollback_transaction().await;
                return Err(error);
            }
        }
        if let Err(error) = client.commit_transaction().await {
            let _ = client.rollback_transaction().await;
            return Err(map_query_error(error));
        }
        Ok(())
    }

    async fn insert_rows(&self, database: &str, table: &str, rows: &[RowInsert]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let (schema, bare) = self.resolve_object(database, table).await?;
        let qualified = qualified_name(database, &schema, &bare);
        let mut client = self.acquire().await?;
        use_database(&mut client, database).await?;

        for insert in rows {
            if insert.values.is_empty() {
                let sql = format!("INSERT INTO {qualified} DEFAULT VALUES");
                run_statement(&mut client, &sql, &[]).await?;
                continue;
            }
            let columns = insert
                .values
                .iter()
                .map(|(column, _)| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ");
            let placeholders = (1..=insert.values.len())
                .map(|index| format!("@P{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let params: Vec<Option<String>> = insert
                .values
                .iter()
                .map(|(_, value)| value.clone())
                .collect();
            let sql = format!("INSERT INTO {qualified} ({columns}) VALUES ({placeholders})");
            run_statement(&mut client, &sql, &params).await?;
        }
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
        let (schema, bare) = self.resolve_object(database, table).await?;
        let qualified = qualified_name(database, &schema, &bare);
        let mut client = self.acquire().await?;
        use_database(&mut client, database).await?;
        client.begin_transaction().await.map_err(map_query_error)?;

        for key in keys {
            if key.is_empty() {
                continue;
            }
            let (clause, params) = key_match_clause(key, 0);
            let sql = format!("DELETE FROM {qualified} WHERE {clause}");
            if let Err(error) = run_statement(&mut client, &sql, &params).await {
                let _ = client.rollback_transaction().await;
                return Err(error);
            }
        }
        if let Err(error) = client.commit_transaction().await {
            let _ = client.rollback_transaction().await;
            return Err(map_query_error(error));
        }
        Ok(())
    }

    async fn execute_query(&self, database: Option<&str>, sql: &str) -> Result<QueryResult> {
        let mut client = self.acquire().await?;
        if let Some(database) = database {
            use_database(&mut client, database).await?;
        }
        let statements = split_statements(sql);
        if statements.is_empty() {
            return Ok(empty_result(sql.trim().to_string()));
        }
        if needs_single_batch(&statements) {
            let results = run_batch_sets(&mut client, sql).await?;
            return Ok(results
                .into_iter()
                .find(|result| result.has_result_set)
                .unwrap_or_else(|| empty_result(sql.trim().to_string())));
        }

        let mut result_set: Option<QueryResult> = None;
        let mut rows_affected = 0u64;
        let mut last_statement = sql.trim().to_string();
        for statement in &statements {
            last_statement = statement.clone();
            let result = run_statement(&mut client, statement, &[]).await?;
            if result.has_result_set && result_set.is_none() {
                result_set = Some(result);
            } else {
                rows_affected += result.rows_affected;
            }
        }

        Ok(result_set.unwrap_or(QueryResult {
            statement: last_statement,
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected,
            has_result_set: false,
            last_insert_id: None,
        }))
    }

    async fn execute_query_many(
        &self,
        database: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        let mut client = self.acquire().await?;
        if let Some(database) = database {
            use_database(&mut client, database).await?;
        }
        let statements = split_statements(sql);
        if statements.is_empty() {
            return Ok(vec![empty_result(sql.trim().to_string())]);
        }
        if needs_single_batch(&statements) {
            // A script that declares batch-scoped variables cannot be split across
            // round-trips; run it whole and return one result per result set.
            return run_batch_sets(&mut client, sql).await;
        }
        let mut results = Vec::with_capacity(statements.len());
        for statement in &statements {
            results.push(run_statement(&mut client, statement, &[]).await?);
        }
        Ok(results)
    }

    async fn create_database(
        &self,
        name: &str,
        _charset: Option<&str>,
        collation: Option<&str>,
    ) -> Result<()> {
        self.execute_batch(None, &create_database_sql(name, collation))
            .await
    }

    fn create_database_sql(
        &self,
        name: &str,
        _charset: Option<&str>,
        collation: Option<&str>,
    ) -> String {
        create_database_sql(name, collation)
    }

    async fn drop_database(&self, name: &str) -> Result<()> {
        let name = quote_identifier(name);
        let sql = format!(
            "ALTER DATABASE {name} SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE {name}"
        );
        self.execute_batch(None, &sql).await
    }

    async fn drop_table(&self, database: &str, table: &str) -> Result<()> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        let sql = format!("DROP TABLE {}", qualified_name(database, &schema, &bare));
        self.execute_batch(None, &sql).await
    }

    async fn empty_table(&self, database: &str, table: &str) -> Result<()> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        let sql = format!("DELETE FROM {}", qualified_name(database, &schema, &bare));
        self.execute_batch(None, &sql).await
    }

    async fn truncate_table(&self, database: &str, table: &str) -> Result<()> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        let sql = format!(
            "TRUNCATE TABLE {}",
            qualified_name(database, &schema, &bare)
        );
        self.execute_batch(None, &sql).await
    }

    async fn rename_table(&self, database: &str, table: &str, new_name: &str) -> Result<()> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        // sp_rename takes `schema.old` and a bare new name; names cannot be parameterized.
        let sql = format!(
            "EXEC sp_rename N'{}.{}', N'{}'",
            escape_literal_part(&schema),
            escape_literal_part(&bare),
            escape_literal_part(new_name)
        );
        self.execute_batch(Some(database), &sql).await
    }

    async fn database_defaults(&self, name: &str) -> Result<(String, String)> {
        let collation = self
            .scalar_text(
                None,
                "SELECT collation_name FROM sys.databases WHERE name = @P1",
                &[Some(name.to_string())],
            )
            .await?
            .unwrap_or_default();
        // SQL Server selects a charset through the collation, so there is no separate one.
        Ok((String::new(), collation))
    }

    async fn server_version(&self) -> Result<String> {
        Ok(self
            .scalar_text(
                None,
                "SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128))",
                &[],
            )
            .await?
            .unwrap_or_default())
    }

    async fn session_count(&self) -> Result<u64> {
        Ok(self
            .scalar_text(None, "SELECT COUNT(*) FROM sys.dm_exec_sessions", &[])
            .await?
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0))
    }

    async fn table_status(&self, database: &str, table: &str) -> Result<TableStatus> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        let sql = format!(
            "SELECT SUM(ps.row_count), SUM(ps.used_page_count) * 8192, \
                    CONVERT(nvarchar(19), o.create_date, 120), \
                    CONVERT(nvarchar(19), o.modify_date, 120) \
             FROM {db}.sys.dm_db_partition_stats ps \
             JOIN {db}.sys.objects o ON o.object_id = ps.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.name = @P1 AND s.name = @P2 AND ps.index_id IN (0, 1)",
            db = quote_identifier(database)
        );
        let result = self.run(None, &sql, &[Some(bare), Some(schema)]).await?;
        let row = result.rows.first();
        Ok(TableStatus {
            engine: Some("SQL Server".to_string()),
            rows: row.and_then(|row| u64_cell(row, 0)),
            data_length: row.and_then(|row| u64_cell(row, 1)),
            created: row
                .map(|row| text_cell(row, 2))
                .filter(|value| !value.is_empty()),
            updated: row
                .map(|row| text_cell(row, 3))
                .filter(|value| !value.is_empty()),
            ..Default::default()
        })
    }

    async fn table_statuses(&self, database: &str) -> Result<Vec<(String, TableStatus)>> {
        let sql = format!(
            "SELECT s.name, o.name, SUM(ps.row_count), SUM(ps.used_page_count) * 8192, \
                    CONVERT(nvarchar(19), o.create_date, 120), \
                    CONVERT(nvarchar(19), o.modify_date, 120) \
             FROM {db}.sys.dm_db_partition_stats ps \
             JOIN {db}.sys.objects o ON o.object_id = ps.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.type = 'U' AND ps.index_id IN (0, 1) \
             GROUP BY s.name, o.name, o.create_date, o.modify_date \
             ORDER BY s.name, o.name",
            db = quote_identifier(database)
        );
        let result = self.run(None, &sql, &[]).await?;
        Ok(result
            .rows
            .iter()
            .map(|row| {
                (
                    format!("{}.{}", text_cell(row, 0), text_cell(row, 1)),
                    TableStatus {
                        engine: Some("SQL Server".to_string()),
                        rows: u64_cell(row, 1),
                        data_length: u64_cell(row, 2),
                        created: Some(text_cell(row, 3)).filter(|value| !value.is_empty()),
                        updated: Some(text_cell(row, 4)).filter(|value| !value.is_empty()),
                        ..Default::default()
                    },
                )
            })
            .collect())
    }

    async fn character_sets(&self) -> Result<Vec<String>> {
        // SQL Server encodes characters through the collation; there is no independent
        // charset list, so the dialog leaves the charset unset and lists collations.
        Ok(Vec::new())
    }

    async fn collations(&self) -> Result<Vec<String>> {
        let result = self
            .run(
                None,
                "SELECT name FROM sys.fn_helpcollations() ORDER BY name",
                &[],
            )
            .await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|row| row.first().map(CellValue::as_display))
            .collect())
    }

    async fn alter_database_defaults(
        &self,
        name: &str,
        _charset: Option<&str>,
        collation: Option<&str>,
    ) -> Result<()> {
        let sql = alter_database_sql(name, collation);
        if sql.is_empty() {
            return Ok(());
        }
        self.execute_batch(None, &sql).await
    }

    fn alter_database_sql(
        &self,
        name: &str,
        _charset: Option<&str>,
        collation: Option<&str>,
    ) -> String {
        alter_database_sql(name, collation)
    }

    fn column_types(&self) -> Vec<&'static str> {
        SQL_SERVER_TYPES.to_vec()
    }

    async fn list_routines(&self, database: &str) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT ROUTINE_NAME FROM {db}.INFORMATION_SCHEMA.ROUTINES ORDER BY ROUTINE_NAME",
            db = quote_identifier(database)
        );
        let result = self.run(None, &sql, &[]).await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|row| row.first().map(CellValue::as_display))
            .collect())
    }

    async fn list_events(&self, _database: &str) -> Result<Vec<String>> {
        // SQL Server has no scheduled events.
        Ok(Vec::new())
    }

    async fn backup_object_metadata(
        &self,
        database: &str,
        kind: BackupObjectKind,
        name: &str,
    ) -> Result<ObjectDump> {
        match kind {
            BackupObjectKind::Table => {
                let schema = self.table_schema(database, name).await?;
                let ddl = self.table_schema_sql(database, name, None, &schema);
                let fields = self
                    .columns(database, name)
                    .await?
                    .into_iter()
                    .map(|column| column.name)
                    .collect();
                let trigger_ddl = schema
                    .triggers
                    .iter()
                    .map(|trigger| trigger.statement.clone())
                    .collect();
                Ok(ObjectDump {
                    name: name.to_string(),
                    kind,
                    ddl,
                    fields,
                    trigger_ddl,
                    rows: Vec::new(),
                })
            }
            BackupObjectKind::View => Ok(ObjectDump {
                name: name.to_string(),
                kind,
                ddl: self.view_details(database, name).await?.definition,
                fields: Vec::new(),
                trigger_ddl: Vec::new(),
                rows: Vec::new(),
            }),
            BackupObjectKind::Function => {
                let definition = match self
                    .routine_details(database, RoutineKind::Procedure, name)
                    .await
                {
                    Ok(details) => details.definition,
                    Err(_) => {
                        self.routine_details(database, RoutineKind::Function, name)
                            .await?
                            .definition
                    }
                };
                Ok(ObjectDump {
                    name: name.to_string(),
                    kind,
                    ddl: definition,
                    fields: Vec::new(),
                    trigger_ddl: Vec::new(),
                    rows: Vec::new(),
                })
            }
            BackupObjectKind::Event => Ok(ObjectDump {
                name: name.to_string(),
                kind,
                ddl: String::new(),
                fields: Vec::new(),
                trigger_ddl: Vec::new(),
                rows: Vec::new(),
            }),
        }
    }

    async fn stream_table_rows(
        &self,
        database: &str,
        table: &str,
        on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<u64> {
        let (schema, bare) = self.resolve_object(database, table).await?;
        let qualified = qualified_name(database, &schema, &bare);
        let mut client = self.acquire().await?;
        use_database(&mut client, database).await?;
        let mut stream = client
            .query(Cow::Owned(format!("SELECT * FROM {qualified}")), &[])
            .await
            .map_err(map_query_error)?;
        let mut count = 0u64;
        while let Some(item) = stream.try_next().await.map_err(map_query_error)? {
            if let QueryItem::Row(row) = item {
                let tuple = render_row_tuple(&row);
                on_row(&tuple)?;
                count += 1;
            }
        }
        Ok(count)
    }

    async fn restore_object(&self, database: &str, object: &ObjectDump) -> Result<()> {
        // `object.name` may be schema-qualified (`dbo.users`); split it back apart.
        let (schema, object_name) = match object.kind {
            BackupObjectKind::Table | BackupObjectKind::View => self
                .resolve_object(database, &object.name)
                .await
                .unwrap_or_else(|_| (DEFAULT_SCHEMA.to_string(), object.name.clone())),
            _ => (DEFAULT_SCHEMA.to_string(), object.name.clone()),
        };
        let qualified = qualified_name(database, &schema, &object_name);
        let mut client = self.acquire().await?;
        use_database(&mut client, database).await?;

        match object.kind {
            BackupObjectKind::Table => {
                if object.ddl.trim().is_empty() {
                    return Err(Error::Query(format!(
                        "no CREATE TABLE statement for {}",
                        object.name
                    )));
                }
                run_statement(
                    &mut client,
                    &format!("DROP TABLE IF EXISTS {qualified}"),
                    &[],
                )
                .await?;
                run_statement(&mut client, &object.ddl, &[]).await?;
                if !object.rows.is_empty() {
                    let has_identity = self
                        .has_identity(database, &schema, &object_name)
                        .await
                        .unwrap_or(false);
                    restore_table_rows(
                        &mut client,
                        &qualified,
                        &object.fields,
                        &object.rows,
                        has_identity,
                    )
                    .await?;
                }
            }
            BackupObjectKind::View => {
                // `DROP VIEW` rejects a database-qualified name; `USE` was applied above.
                run_statement(
                    &mut client,
                    &format!(
                        "DROP VIEW IF EXISTS {}.{}",
                        quote_identifier(&schema),
                        quote_identifier(&object_name)
                    ),
                    &[],
                )
                .await?;
                if !object.ddl.trim().is_empty() {
                    run_statement(&mut client, &object.ddl, &[]).await?;
                }
            }
            BackupObjectKind::Function => {
                if !object.ddl.trim().is_empty() {
                    let keyword = routine_keyword(&object.ddl);
                    let _ = run_statement(
                        &mut client,
                        &format!(
                            "DROP {keyword} IF EXISTS {}.{}",
                            quote_identifier(&schema),
                            quote_identifier(&object_name)
                        ),
                        &[],
                    )
                    .await;
                    run_statement(&mut client, &object.ddl, &[]).await?;
                }
            }
            BackupObjectKind::Event => {}
        }
        Ok(())
    }

    fn storage_engines(&self) -> Vec<&'static str> {
        Vec::new()
    }

    async fn table_schema(&self, database: &str, table: &str) -> Result<TableSchema> {
        let (schema_name, table) = self.resolve_object(database, table).await?;
        let db = quote_identifier(database);

        let column_sql = format!(
            "SELECT c.name, ty.name, c.max_length, c.precision, c.scale, c.is_nullable, \
                    c.is_identity, dc.definition, c.collation_name, \
                    CAST(ep.value AS nvarchar(max)), c.is_computed, cc.definition, \
                    CASE WHEN pk.column_id IS NULL THEN 0 ELSE 1 END \
             FROM {db}.sys.columns c \
             JOIN {db}.sys.objects o ON o.object_id = c.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             JOIN {db}.sys.types ty ON ty.user_type_id = c.user_type_id \
             LEFT JOIN {db}.sys.default_constraints dc \
               ON dc.parent_object_id = c.object_id AND dc.parent_column_id = c.column_id \
             LEFT JOIN {db}.sys.extended_properties ep \
               ON ep.major_id = c.object_id AND ep.minor_id = c.column_id AND ep.name = N'MS_Description' \
             LEFT JOIN {db}.sys.computed_columns cc ON cc.object_id = c.object_id AND cc.column_id = c.column_id \
             LEFT JOIN (SELECT ic.object_id, ic.column_id FROM {db}.sys.indexes i \
                        JOIN {db}.sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
                        WHERE i.is_primary_key = 1) pk \
               ON pk.object_id = c.object_id AND pk.column_id = c.column_id \
             WHERE o.name = @P1 AND s.name = @P2 \
             ORDER BY c.column_id"
        );
        let column_result = self
            .run(
                None,
                &column_sql,
                &[Some(table.clone()), Some(schema_name.clone())],
            )
            .await?;

        let mut columns = Vec::with_capacity(column_result.rows.len());
        for row in &column_result.rows {
            let type_name = text_cell(row, 1);
            let max_length = int_cell(row, 2).unwrap_or(0);
            let precision = int_cell(row, 3).unwrap_or(0);
            let scale = int_cell(row, 4).unwrap_or(0);
            let (data_type, length, decimals) =
                parse_sys_type(&type_name, max_length, precision, scale);
            columns.push(ColumnDef {
                name: text_cell(row, 0),
                data_type,
                length,
                decimals,
                nullable: bool_cell(row, 5),
                default: text_cell(row, 7),
                primary_key: text_cell(row, 12) == "1",
                auto_increment: bool_cell(row, 6),
                unsigned: false,
                zerofill: false,
                comment: text_cell(row, 9),
                charset: String::new(),
                collation: text_cell(row, 8),
                extra: String::new(),
                generated: if bool_cell(row, 10) {
                    text_cell(row, 11)
                } else {
                    String::new()
                },
                stored: false,
            });
        }

        let index_sql = format!(
            "SELECT i.name, i.is_unique, i.is_primary_key, i.type_desc \
             FROM {db}.sys.indexes i \
             JOIN {db}.sys.objects o ON o.object_id = i.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.name = @P1 AND s.name = @P2 AND i.name IS NOT NULL \
             ORDER BY i.index_id"
        );
        let index_result = self
            .run(
                None,
                &index_sql,
                &[Some(table.clone()), Some(schema_name.clone())],
            )
            .await?;
        let mut indexes = Vec::with_capacity(index_result.rows.len());
        for row in &index_result.rows {
            let name = text_cell(row, 0);
            let columns = self
                .index_columns(database, &name)
                .await
                .unwrap_or_default();
            indexes.push(IndexDef {
                name,
                columns,
                unique: bool_cell(row, 1),
                primary: bool_cell(row, 2),
                index_type: text_cell(row, 3),
                comment: String::new(),
            });
        }

        let foreign_keys = self.foreign_keys(database, &schema_name, &table).await?;
        let triggers = self.triggers(database, &schema_name, &table).await?;

        let collation = self
            .scalar_text(
                None,
                &format!(
                    "SELECT MAX(c.collation_name) FROM {db}.sys.columns c \
                     JOIN {db}.sys.objects o ON o.object_id = c.object_id \
                     JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
                     WHERE o.name = @P1 AND s.name = @P2"
                ),
                &[Some(table), Some(schema_name)],
            )
            .await?
            .unwrap_or_default();

        Ok(TableSchema {
            columns,
            indexes,
            foreign_keys,
            triggers,
            options: TableOptions {
                engine: String::new(),
                charset: String::new(),
                collation,
                comment: String::new(),
                auto_increment: String::new(),
            },
        })
    }

    fn table_schema_sql(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> String {
        // Schema information is not available synchronously; an explicit `schema.name` is
        // honoured and a bare name defaults to `dbo`.
        let _ = database;
        let (schema, bare) = split_schema(table)
            .map(|(schema, bare)| (schema.to_string(), bare.to_string()))
            .unwrap_or_else(|| (DEFAULT_SCHEMA.to_string(), table.to_string()));
        table_schema_sql(&quote_identifier(&schema), &bare, original, modified)
    }

    async fn view_details(&self, database: &str, name: &str) -> Result<ViewDetails> {
        let (schema, bare) = self.resolve_object(database, name).await?;
        let db = quote_identifier(database);
        let definition = self
            .scalar_text(
                None,
                &format!(
                    "SELECT m.definition FROM {db}.sys.sql_modules m \
                     JOIN {db}.sys.views v ON v.object_id = m.object_id \
                     JOIN {db}.sys.schemas s ON s.schema_id = v.schema_id \
                     WHERE v.name = @P1 AND s.name = @P2"
                ),
                &[Some(bare.clone()), Some(schema.clone())],
            )
            .await?
            .ok_or_else(|| Error::Query(format!("view {name} not found")))?;
        let updatable = self
            .scalar_text(
                None,
                &format!(
                    "SELECT IS_UPDATABLE FROM {db}.INFORMATION_SCHEMA.VIEWS \
                     WHERE TABLE_NAME = @P1 AND TABLE_SCHEMA = @P2"
                ),
                &[Some(bare), Some(schema)],
            )
            .await?
            .map(|value| value.eq_ignore_ascii_case("YES"))
            .unwrap_or(false);
        Ok(ViewDetails {
            info: ViewInfo {
                name: name.to_string(),
                updatable,
                ..Default::default()
            },
            definition,
            character_set_client: String::new(),
            collation_connection: String::new(),
        })
    }

    fn view_sql(&self, _database: &str, original: Option<&str>, edit: &ViewEdit) -> String {
        let mut statements = Vec::new();
        if let Some(original) = original {
            let (schema, bare) = split_schema(original)
                .map(|(schema, bare)| (schema.to_string(), bare.to_string()))
                .unwrap_or_else(|| (DEFAULT_SCHEMA.to_string(), original.to_string()));
            statements.push(format!(
                "DROP VIEW IF EXISTS {}.{}",
                quote_identifier(&schema),
                quote_identifier(&bare)
            ));
        }
        let definition = trim_statement(&edit.definition);
        if !definition.is_empty() {
            statements.push(definition);
        }
        statements.join(";\n")
    }

    async fn save_view(
        &self,
        database: &str,
        original: Option<&str>,
        edit: &ViewEdit,
    ) -> Result<()> {
        let (schema, original) = match original {
            Some(name) => self
                .resolve_object(database, name)
                .await
                .unwrap_or_else(|_| (DEFAULT_SCHEMA.to_string(), name.to_string())),
            None => (DEFAULT_SCHEMA.to_string(), String::new()),
        };
        let mut client = self.acquire().await?;
        use_database(&mut client, database).await?;
        if !original.is_empty() {
            // `DROP VIEW` rejects a database-qualified name; `USE` was applied above.
            let sql = format!(
                "DROP VIEW IF EXISTS {}.{}",
                quote_identifier(&schema),
                quote_identifier(&original)
            );
            run_statement(&mut client, &sql, &[]).await?;
        }
        let definition = trim_statement(&edit.definition);
        if !definition.is_empty() {
            run_statement(&mut client, &definition, &[]).await?;
        }
        Ok(())
    }

    async fn drop_view(&self, database: &str, name: &str) -> Result<()> {
        let (schema, name) = self
            .resolve_object(database, name)
            .await
            .unwrap_or_else(|_| (DEFAULT_SCHEMA.to_string(), name.to_string()));
        // `DROP VIEW` rejects a database-qualified name, so select the database and use a
        // two-part name instead.
        let sql = format!(
            "DROP VIEW {}.{}",
            quote_identifier(&schema),
            quote_identifier(&name)
        );
        self.execute_batch(Some(database), &sql).await
    }

    async fn list_routine_infos(&self, database: &str) -> Result<Vec<RoutineInfo>> {
        let db = quote_identifier(database);
        // Names are schema-qualified (`dbo.f`), matching how tables are listed.
        let sql = format!(
            "SELECT ROUTINE_SCHEMA, ROUTINE_NAME, ROUTINE_TYPE, ISNULL(ROUTINE_DEFINITION, ''), \
                    ISNULL(DATA_TYPE, ''), IS_DETERMINISTIC, SQL_DATA_ACCESS, \
                    CONVERT(nvarchar(19), CREATED, 120), CONVERT(nvarchar(19), LAST_ALTERED, 120) \
             FROM {db}.INFORMATION_SCHEMA.ROUTINES ORDER BY ROUTINE_SCHEMA, ROUTINE_NAME"
        );
        let result = self.run(None, &sql, &[]).await?;
        let mut routines = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            let kind =
                RoutineKind::from_sql_name(&text_cell(row, 2)).unwrap_or(RoutineKind::Procedure);
            routines.push(RoutineInfo {
                name: format!("{}.{}", text_cell(row, 0), text_cell(row, 1)),
                kind,
                comment: String::new(),
                return_type: text_cell(row, 4),
                definer: String::new(),
                deterministic: bool_cell(row, 5),
                data_access: text_cell(row, 6),
                security_type: String::new(),
                created: Some(text_cell(row, 7)).filter(|value| !value.is_empty()),
                modified: Some(text_cell(row, 8)).filter(|value| !value.is_empty()),
            });
        }
        Ok(routines)
    }

    async fn routine_details(
        &self,
        database: &str,
        kind: RoutineKind,
        name: &str,
    ) -> Result<RoutineDetails> {
        let (schema, bare) = self.resolve_object(database, name).await?;
        let db = quote_identifier(database);
        let sql = format!(
            "SELECT ISNULL(ROUTINE_DEFINITION, '') FROM {db}.INFORMATION_SCHEMA.ROUTINES \
             WHERE ROUTINE_SCHEMA = @P1 AND ROUTINE_NAME = @P2 AND ROUTINE_TYPE = @P3"
        );
        let definition = self
            .scalar_text(
                None,
                &sql,
                &[
                    Some(schema),
                    Some(bare.clone()),
                    Some(kind.sql_name().to_string()),
                ],
            )
            .await?
            .ok_or_else(|| Error::Query(format!("routine {name} not found")))?;
        Ok(RoutineDetails {
            info: RoutineInfo {
                name: name.to_string(),
                kind,
                ..Default::default()
            },
            definition,
            sql_mode: String::new(),
            character_set_client: String::new(),
            collation_connection: String::new(),
            database_collation: String::new(),
        })
    }

    fn routine_sql(
        &self,
        _database: &str,
        original: Option<(&str, RoutineKind)>,
        edit: &RoutineEdit,
    ) -> String {
        let mut statements = Vec::new();
        if let Some((name, kind)) = original {
            let (schema, bare) = split_schema(name)
                .map(|(schema, bare)| (schema.to_string(), bare.to_string()))
                .unwrap_or_else(|| (DEFAULT_SCHEMA.to_string(), name.to_string()));
            statements.push(format!(
                "DROP {} IF EXISTS {}.{}",
                kind.sql_name(),
                quote_identifier(&schema),
                quote_identifier(&bare)
            ));
        }
        let definition = trim_statement(&edit.definition);
        if !definition.is_empty() {
            statements.push(definition);
        }
        statements.join(";\n")
    }

    async fn save_routine(
        &self,
        database: &str,
        original: Option<(&str, RoutineKind)>,
        edit: &RoutineEdit,
    ) -> Result<()> {
        let mut client = self.acquire().await?;
        use_database(&mut client, database).await?;
        if let Some((name, kind)) = original {
            let (schema, name) = self
                .resolve_object(database, name)
                .await
                .unwrap_or_else(|_| (DEFAULT_SCHEMA.to_string(), name.to_string()));
            // `DROP {PROCEDURE|FUNCTION}` rejects a database-qualified name; `USE` applies.
            let sql = format!(
                "DROP {} IF EXISTS {}.{}",
                kind.sql_name(),
                quote_identifier(&schema),
                quote_identifier(&name)
            );
            run_statement(&mut client, &sql, &[]).await?;
        }
        let definition = trim_statement(&edit.definition);
        if !definition.is_empty() {
            run_statement(&mut client, &definition, &[]).await?;
        }
        Ok(())
    }

    async fn drop_routine(&self, database: &str, kind: RoutineKind, name: &str) -> Result<()> {
        let (schema, name) = self
            .resolve_object(database, name)
            .await
            .unwrap_or_else(|_| (DEFAULT_SCHEMA.to_string(), name.to_string()));
        // `DROP {PROCEDURE|FUNCTION}` rejects a database-qualified name.
        let sql = format!(
            "DROP {} {}.{}",
            kind.sql_name(),
            quote_identifier(&schema),
            quote_identifier(&name)
        );
        self.execute_batch(Some(database), &sql).await
    }

    async fn close(&self) -> Result<()> {
        // bb8 has no explicit close; the pooled connections are dropped with the pool.
        Ok(())
    }

    // ----- Account management (implemented in `user.rs`) ----------------------------------------

    async fn list_users(&self) -> Result<Vec<UserAccount>> {
        user::list_users(self).await
    }

    async fn user_details(&self, user_name: &str, host: &str) -> Result<UserDetails> {
        user::user_details(self, user_name, host).await
    }

    fn user_edit_sql(&self, edit: &UserEdit) -> String {
        user::user_edit_sql(edit)
    }

    fn user_edit_groups(&self, edit: &UserEdit) -> Vec<(UserEditSection, Vec<String>)> {
        user::user_edit_groups(edit)
    }

    async fn save_user(&self, edit: &UserEdit) -> Result<()> {
        user::save_user(self, edit).await
    }

    async fn drop_user(&self, user_name: &str, host: &str) -> Result<()> {
        user::drop_user(self, user_name, host).await
    }

    async fn rename_user(
        &self,
        user_name: &str,
        host: &str,
        new_user: &str,
        new_host: &str,
    ) -> Result<()> {
        user::rename_user(self, user_name, host, new_user, new_host).await
    }

    fn authentication_plugins(&self) -> Vec<&'static str> {
        user::authentication_plugins()
    }

    async fn object_privilege_matrix(
        &self,
        _database: &str,
        _name: &str,
    ) -> Result<Vec<ObjectPrivilegeRow>> {
        Ok(Vec::new())
    }

    async fn set_object_privileges(
        &self,
        _database: &str,
        _name: &str,
        _rows: &[ObjectPrivilegeRow],
    ) -> Result<()> {
        Err(Error::Query(
            "SQL Server object privilege management is not supported yet".to_string(),
        ))
    }
}

impl SqlServerConnection {
    async fn index_columns(&self, database: &str, index: &str) -> Result<Vec<String>> {
        let db = quote_identifier(database);
        let sql = format!(
            "SELECT c.name FROM {db}.sys.index_columns ic \
             JOIN {db}.sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
             JOIN {db}.sys.indexes i ON i.object_id = ic.object_id AND i.index_id = ic.index_id \
             WHERE i.name = @P1 ORDER BY ic.key_ordinal",
        );
        let result = self.run(None, &sql, &[Some(index.to_string())]).await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|row| row.first().map(CellValue::as_display))
            .collect())
    }

    async fn foreign_keys(
        &self,
        database: &str,
        schema: &str,
        table: &str,
    ) -> Result<Vec<ForeignKeyDef>> {
        let db = quote_identifier(database);
        let sql = format!(
            "SELECT fk.name, fk.object_id, \
                    OBJECT_NAME(fk.referenced_object_id), \
                    fk.delete_referential_action_desc, fk.update_referential_action_desc \
             FROM {db}.sys.foreign_keys fk \
             JOIN {db}.sys.objects o ON o.object_id = fk.parent_object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.name = @P1 AND s.name = @P2 ORDER BY fk.name"
        );
        let result = self
            .run(
                None,
                &sql,
                &[Some(table.to_string()), Some(schema.to_string())],
            )
            .await?;
        let mut foreign_keys = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            let name = text_cell(row, 0);
            let object_id = text_cell(row, 1);
            let columns_sql = format!(
                "SELECT pc.name, rc.name FROM {db}.sys.foreign_key_columns fkc \
                 JOIN {db}.sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
                 JOIN {db}.sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
                 WHERE fkc.constraint_object_id = @P1 ORDER BY fkc.constraint_column_id"
            );
            let pairs = self.run(None, &columns_sql, &[Some(object_id)]).await?;
            let mut columns = Vec::new();
            let mut referenced_columns = Vec::new();
            for pair in &pairs.rows {
                columns.push(text_cell(pair, 0));
                referenced_columns.push(text_cell(pair, 1));
            }
            foreign_keys.push(ForeignKeyDef {
                name,
                columns,
                referenced_table: text_cell(row, 2),
                referenced_columns,
                on_delete: normalize_action(&text_cell(row, 3)),
                on_update: normalize_action(&text_cell(row, 4)),
            });
        }
        Ok(foreign_keys)
    }

    async fn triggers(&self, database: &str, schema: &str, table: &str) -> Result<Vec<TriggerDef>> {
        let db = quote_identifier(database);
        let sql = format!(
            "SELECT tr.name, ISNULL(m.definition, '') FROM {db}.sys.triggers tr \
             JOIN {db}.sys.objects o ON o.object_id = tr.parent_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             LEFT JOIN {db}.sys.sql_modules m ON m.object_id = tr.object_id \
             WHERE o.name = @P1 AND s.name = @P2 ORDER BY tr.name"
        );
        let result = self
            .run(
                None,
                &sql,
                &[Some(table.to_string()), Some(schema.to_string())],
            )
            .await?;
        Ok(result
            .rows
            .iter()
            .map(|row| TriggerDef {
                name: text_cell(row, 0),
                timing: String::new(),
                event: String::new(),
                statement: text_cell(row, 1),
            })
            .collect())
    }
}

/// The SQL Server types offered by the table designer.
const SQL_SERVER_TYPES: [&str; 33] = [
    "bigint",
    "int",
    "smallint",
    "tinyint",
    "bit",
    "decimal",
    "numeric",
    "money",
    "smallmoney",
    "float",
    "real",
    "date",
    "time",
    "datetime",
    "datetime2",
    "smalldatetime",
    "datetimeoffset",
    "char",
    "varchar",
    "nchar",
    "nvarchar",
    "text",
    "ntext",
    "binary",
    "varbinary",
    "image",
    "uniqueidentifier",
    "xml",
    "sql_variant",
    "timestamp",
    "rowversion",
    "geography",
    "geometry",
];

fn create_database_sql(name: &str, collation: Option<&str>) -> String {
    let mut sql = format!("CREATE DATABASE {}", quote_identifier(name));
    if let Some(collation) = collation.filter(|value| !value.trim().is_empty()) {
        sql.push_str(" COLLATE ");
        sql.push_str(collation.trim());
    }
    sql
}

fn alter_database_sql(name: &str, collation: Option<&str>) -> String {
    match collation.filter(|value| !value.trim().is_empty()) {
        Some(collation) => format!(
            "ALTER DATABASE {} COLLATE {}",
            quote_identifier(name),
            collation.trim()
        ),
        None => String::new(),
    }
}

fn empty_result(statement: String) -> QueryResult {
    QueryResult {
        statement,
        columns: Vec::new(),
        rows: Vec::new(),
        rows_affected: 0,
        has_result_set: false,
        last_insert_id: None,
    }
}

/// Switch the session's current database.
pub(crate) async fn use_database(client: &mut SqlClient, database: &str) -> Result<()> {
    if database.is_empty() {
        return Ok(());
    }
    client
        .simple_query(format!("USE {}", quote_identifier(database)))
        .await
        .map_err(map_query_error)?
        .into_results()
        .await
        .map_err(map_query_error)?;
    Ok(())
}

/// Run one statement with optional bound values and turn it into a [`QueryResult`].
///
/// Result-set statements use the parameterized RPC path, DML uses the RPC path to obtain
/// a row count, and everything else (DDL, `CREATE PROCEDURE`, ...) is sent whole as a text
/// batch so it is not split or prepared.
pub(crate) async fn run_statement(
    client: &mut SqlClient,
    sql: &str,
    params: &[Option<String>],
) -> Result<QueryResult> {
    let binds: Vec<&dyn ToSql> = params.iter().map(|value| value as &dyn ToSql).collect();
    if returns_result_set(sql) {
        let mut stream = client
            .query(Cow::Borrowed(sql), &binds)
            .await
            .map_err(map_query_error)?;
        let mut columns: Option<Vec<ColumnInfo>> = None;
        let mut rows = Vec::new();
        while let Some(item) = stream.try_next().await.map_err(map_query_error)? {
            match item {
                QueryItem::Metadata(metadata) => {
                    if columns.is_none() {
                        columns = Some(metadata.columns().iter().map(column_info).collect());
                    } else {
                        // Only the first result set is returned; stop before its rows
                        // would be mixed with the next one's.
                        break;
                    }
                }
                QueryItem::Row(row) => {
                    if columns.is_some() {
                        rows.push(decode_row(&row));
                    }
                }
            }
        }
        let columns = columns.unwrap_or_default();
        Ok(QueryResult {
            statement: sql.to_string(),
            columns,
            rows,
            rows_affected: 0,
            has_result_set: true,
            last_insert_id: None,
        })
    } else if is_dml(sql) || !params.is_empty() {
        let result = client
            .execute(Cow::Borrowed(sql), &binds)
            .await
            .map_err(map_query_error)?;
        Ok(QueryResult {
            statement: sql.to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: result.total(),
            has_result_set: false,
            last_insert_id: None,
        })
    } else {
        // DDL and other batches: send as one text batch so `CREATE PROCEDURE`/`VIEW`
        // reach the server intact.
        let mut stream = client
            .simple_query(Cow::Borrowed(sql))
            .await
            .map_err(map_query_error)?;
        let mut rows = Vec::new();
        let mut columns: Option<Vec<ColumnInfo>> = None;
        while let Some(item) = stream.try_next().await.map_err(map_query_error)? {
            match item {
                QueryItem::Metadata(metadata) => {
                    if columns.is_none() {
                        columns = Some(metadata.columns().iter().map(column_info).collect());
                    }
                }
                QueryItem::Row(row) => rows.push(decode_row(&row)),
            }
        }
        Ok(QueryResult {
            statement: sql.to_string(),
            columns: columns.unwrap_or_default(),
            rows,
            rows_affected: 0,
            has_result_set: false,
            last_insert_id: None,
        })
    }
}

/// Whether a multi-statement script must be sent as one batch to preserve batch-scoped
/// state (local `DECLARE` variables), which splitting would lose.
fn needs_single_batch(statements: &[String]) -> bool {
    statements.len() > 1
        && statements
            .iter()
            .any(|sql| leading_keyword(sql) == "DECLARE")
}

/// Run a whole script as one text batch and return one [`QueryResult`] per result set.
/// `DONE` row counts are not exposed by the batch path, so DML-only scripts yield a single
/// empty result.
async fn run_batch_sets(client: &mut SqlClient, sql: &str) -> Result<Vec<QueryResult>> {
    let mut stream = client
        .simple_query(Cow::Borrowed(sql))
        .await
        .map_err(map_query_error)?;
    let mut results: Vec<QueryResult> = Vec::new();
    let mut current: Option<QueryResult> = None;
    while let Some(item) = stream.try_next().await.map_err(map_query_error)? {
        match item {
            QueryItem::Metadata(metadata) => {
                if let Some(previous) = current.take() {
                    results.push(previous);
                }
                current = Some(QueryResult {
                    statement: sql.to_string(),
                    columns: metadata.columns().iter().map(column_info).collect(),
                    rows: Vec::new(),
                    rows_affected: 0,
                    has_result_set: true,
                    last_insert_id: None,
                });
            }
            QueryItem::Row(row) => {
                if let Some(current) = current.as_mut() {
                    current.rows.push(decode_row(&row));
                }
            }
        }
    }
    if let Some(previous) = current.take() {
        results.push(previous);
    }
    if results.is_empty() {
        results.push(empty_result(sql.trim().to_string()));
    }
    Ok(results)
}

fn decode_row(row: &Row) -> Vec<CellValue> {
    (0..row.len())
        .map(|index| decode_cell(row, index))
        .collect()
}

/// Render a row as a T-SQL value tuple for a backup insert.
fn render_row_tuple(row: &Row) -> String {
    let mut tuple = String::from("(");
    for index in 0..row.len() {
        if index > 0 {
            tuple.push_str(", ");
        }
        tuple.push_str(&render_literal(&decode_cell(row, index)));
    }
    tuple.push(')');
    tuple
}

fn column_info(column: &tiberius::Column) -> ColumnInfo {
    ColumnInfo {
        name: column.name().to_string(),
        data_type: column_type_name(column.column_type()),
        nullable: true,
        primary_key: false,
        comment: String::new(),
    }
}

fn column_type_name(column_type: tiberius::ColumnType) -> String {
    use tiberius::ColumnType::*;
    match column_type {
        Null => "",
        Bit | Bitn => "bit",
        Int1 => "tinyint",
        Int2 => "smallint",
        Int4 | Intn => "int",
        Int8 => "bigint",
        Datetime4 | Datetime | Datetimen => "datetime",
        Float4 => "real",
        Float8 | Floatn => "float",
        Money | Money4 => "money",
        Guid => "uniqueidentifier",
        Decimaln | Numericn => "decimal",
        Daten => "date",
        Timen => "time",
        Datetime2 => "datetime2",
        DatetimeOffsetn => "datetimeoffset",
        BigVarBin | BigBinary | Image => "varbinary",
        BigVarChar | BigChar => "varchar",
        NVarchar | NChar => "nvarchar",
        Xml => "xml",
        Udt => "udt",
        Text => "text",
        NText => "ntext",
        SSVariant => "sql_variant",
    }
    .to_string()
}

fn assignment_clause(set: &[(String, Option<String>)]) -> (String, Vec<Option<String>>) {
    let mut params = Vec::new();
    let mut parts = Vec::new();
    for (column, value) in set {
        let column = quote_identifier(column);
        match value {
            Some(value) => {
                params.push(Some(value.clone()));
                parts.push(format!("{column} = @P{}", params.len()));
            }
            None => parts.push(format!("{column} = NULL")),
        }
    }
    (parts.join(", "), params)
}

/// A `WHERE` fragment for key columns whose placeholders start after `offset` bound values.
fn key_match_clause(
    keys: &[(String, Option<String>)],
    offset: usize,
) -> (String, Vec<Option<String>>) {
    let mut params = Vec::new();
    let mut parts = Vec::new();
    for (column, value) in keys {
        let column = quote_identifier(column);
        match value {
            Some(value) => {
                params.push(Some(value.clone()));
                parts.push(format!("{column} = @P{}", offset + params.len()));
            }
            None => parts.push(format!("{column} IS NULL")),
        }
    }
    (parts.join(" AND "), params)
}

async fn restore_table_rows(
    client: &mut SqlClient,
    qualified: &str,
    fields: &[String],
    rows: &[String],
    has_identity: bool,
) -> Result<()> {
    let columns = fields
        .iter()
        .map(|field| quote_identifier(field))
        .collect::<Vec<_>>()
        .join(", ");
    // SQL Server allows at most 1000 value tuples per INSERT.
    for batch in rows.chunks(500) {
        let values = batch.join(", ");
        // `SET IDENTITY_INSERT` sent through `sp_executesql` does not carry over to the
        // next request, so when the table has an identity column it is wrapped around the
        // INSERT in the same plain text batch.
        let sql = if has_identity {
            format!(
                "SET IDENTITY_INSERT {qualified} ON; \
                 INSERT INTO {qualified} ({columns}) VALUES {values}; \
                 SET IDENTITY_INSERT {qualified} OFF"
            )
        } else {
            format!("INSERT INTO {qualified} ({columns}) VALUES {values}")
        };
        run_plain_batch(client, &sql).await?;
    }
    Ok(())
}

/// Send a statement (or script) as one text batch and drain it, ignoring any result sets.
/// Used where session-scoped SET options must stay in the same batch as the DML.
async fn run_plain_batch(client: &mut SqlClient, sql: &str) -> Result<()> {
    client
        .simple_query(Cow::Borrowed(sql))
        .await
        .map_err(map_query_error)?
        .into_results()
        .await
        .map_err(map_query_error)?;
    Ok(())
}

fn routine_keyword(definition: &str) -> &'static str {
    if crate::helpers::leading_keyword(definition).contains("PROCEDURE")
        || definition.to_ascii_uppercase().contains("PROCEDURE")
    {
        "PROCEDURE"
    } else {
        "FUNCTION"
    }
}

fn trim_statement(sql: &str) -> String {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    trimmed.to_string()
}

fn escape_literal_part(value: &str) -> String {
    value.replace('\'', "''")
}

/// Split an optional `schema.object` name at the first dot. A name without a usable prefix
/// (or with an empty part) is returned whole with `None`.
fn split_schema(name: &str) -> Option<(&str, &str)> {
    match name.split_once('.') {
        Some((schema, object)) if !schema.is_empty() && !object.is_empty() => {
            Some((schema, object))
        }
        _ => None,
    }
}

fn normalize_action(action: &str) -> String {
    let action = action.replace('_', " ").to_ascii_uppercase();
    if action == "NO ACTION" {
        String::new()
    } else {
        action
    }
}

fn text_cell(row: &[CellValue], index: usize) -> String {
    match row.get(index) {
        None | Some(CellValue::Null) => String::new(),
        Some(value) => value.as_display(),
    }
}

fn u64_cell(row: &[CellValue], index: usize) -> Option<u64> {
    match row.get(index) {
        Some(CellValue::Int(value)) => u64::try_from(*value).ok(),
        Some(CellValue::Uint(value)) => Some(*value),
        Some(CellValue::Float(value)) if *value >= 0.0 => Some(*value as u64),
        Some(CellValue::Text(value)) => value.parse::<u64>().ok(),
        _ => None,
    }
}

fn int_cell(row: &[CellValue], index: usize) -> Option<i64> {
    match row.get(index) {
        Some(CellValue::Int(value)) => Some(*value),
        Some(CellValue::Uint(value)) => i64::try_from(*value).ok(),
        Some(CellValue::Text(value)) => value.parse::<i64>().ok(),
        _ => None,
    }
}

fn bool_cell(row: &[CellValue], index: usize) -> bool {
    match row.get(index) {
        Some(CellValue::Bool(value)) => *value,
        Some(CellValue::Int(value)) => *value != 0,
        Some(CellValue::Uint(value)) => *value != 0,
        Some(CellValue::Text(value)) => {
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
        }
        _ => false,
    }
}

/// Format a SQL Server type from `INFORMATION_SCHEMA.COLUMNS`.
fn display_type(
    data_type: &str,
    length: &str,
    precision: &str,
    scale: &str,
    datetime_precision: &str,
) -> String {
    let lower = data_type.to_ascii_lowercase();
    match lower.as_str() {
        "decimal" | "numeric" => {
            if precision.is_empty() {
                data_type.to_string()
            } else if scale.is_empty() || scale == "0" {
                format!("{data_type}({precision})")
            } else {
                format!("{data_type}({precision},{scale})")
            }
        }
        "datetime2" | "datetimeoffset" | "time" => {
            if datetime_precision.is_empty() || datetime_precision == "7" {
                data_type.to_string()
            } else {
                format!("{data_type}({datetime_precision})")
            }
        }
        "char" | "varchar" | "nchar" | "nvarchar" | "binary" | "varbinary" => {
            if length.is_empty() {
                data_type.to_string()
            } else if length == "-1" {
                format!("{data_type}(max)")
            } else {
                format!("{data_type}({length})")
            }
        }
        _ => data_type.to_string(),
    }
}

/// Reconstruct a column's `(base type, length, scale)` from `sys.types` metadata.
fn parse_sys_type(
    type_name: &str,
    max_length: i64,
    precision: i64,
    scale: i64,
) -> (String, String, String) {
    let lower = type_name.to_ascii_lowercase();
    match lower.as_str() {
        "decimal" | "numeric" => (
            type_name.to_string(),
            precision.to_string(),
            scale.to_string(),
        ),
        "nvarchar" | "nchar" => (
            type_name.to_string(),
            if max_length < 0 {
                "max".to_string()
            } else {
                (max_length / 2).to_string()
            },
            String::new(),
        ),
        "varchar" | "char" | "varbinary" | "binary" => (
            type_name.to_string(),
            if max_length < 0 {
                "max".to_string()
            } else {
                max_length.to_string()
            },
            String::new(),
        ),
        _ => (type_name.to_string(), String::new(), String::new()),
    }
}

fn column_definition(column: &ColumnDef) -> String {
    let mut out = quote_identifier(&column.name);
    let data_type = if column.data_type.trim().is_empty() {
        "nvarchar"
    } else {
        column.data_type.trim()
    };
    out.push(' ');
    out.push_str(data_type);
    if !column.length.is_empty() && column.length != "0" {
        out.push('(');
        out.push_str(&column.length);
        if !column.decimals.is_empty() && column.decimals != "0" {
            out.push_str(", ");
            out.push_str(&column.decimals);
        }
        out.push(')');
    }
    if column.auto_increment {
        out.push_str(" IDENTITY(1,1)");
    }
    if !column.nullable {
        out.push_str(" NOT NULL");
    }
    if !column.default.trim().is_empty() {
        out.push_str(" DEFAULT ");
        out.push_str(column.default.trim());
    }
    out
}

fn table_schema_sql(
    schema: &str,
    table: &str,
    original: Option<&TableSchema>,
    modified: &TableSchema,
) -> String {
    let qualified = format!("{schema}.{}", quote_identifier(table));
    match original {
        None => {
            let mut definitions: Vec<String> =
                modified.columns.iter().map(column_definition).collect();
            if !modified.columns.is_empty() {
                let primary: Vec<String> = modified
                    .columns
                    .iter()
                    .filter(|column| column.primary_key)
                    .map(|column| quote_identifier(&column.name))
                    .collect();
                if !primary.is_empty() {
                    definitions.push(format!(
                        "CONSTRAINT {} PRIMARY KEY ({})",
                        quote_identifier(&format!("PK_{table}")),
                        primary.join(", ")
                    ));
                }
            }
            let mut statements = vec![format!(
                "CREATE TABLE {qualified} (\n    {}\n)",
                definitions.join(",\n    ")
            )];
            statements.extend(create_index_statements(&qualified, &modified.indexes));
            statements.join(";\n")
        }
        Some(original) => {
            let mut statements: Vec<String> = Vec::new();
            let original_columns: std::collections::HashMap<&str, &ColumnDef> = original
                .columns
                .iter()
                .map(|column| (column.name.as_str(), column))
                .collect();
            let modified_columns: std::collections::HashMap<&str, &ColumnDef> = modified
                .columns
                .iter()
                .map(|column| (column.name.as_str(), column))
                .collect();

            for column in &modified.columns {
                if !original_columns.contains_key(column.name.as_str()) {
                    statements.push(format!(
                        "ALTER TABLE {qualified} ADD {}",
                        column_definition(column)
                    ));
                }
            }
            for column in &original.columns {
                if !modified_columns.contains_key(column.name.as_str()) {
                    statements.push(format!(
                        "ALTER TABLE {qualified} DROP COLUMN {}",
                        quote_identifier(&column.name)
                    ));
                }
            }
            for column in &modified.columns {
                if let Some(previous) = original_columns.get(column.name.as_str())
                    && (previous.data_type != column.data_type
                        || previous.length != column.length
                        || previous.decimals != column.decimals
                        || previous.nullable != column.nullable)
                {
                    let mut definition = column_definition(column);
                    // `ALTER COLUMN` cannot carry DEFAULT/IDENTITY clauses.
                    if let Some(index) = definition.find(" DEFAULT ") {
                        definition.truncate(index);
                    }
                    if let Some(index) = definition.find(" IDENTITY") {
                        definition.truncate(index);
                    }
                    // A NOT NULL column needs an explicit nullability clause.
                    if !definition.to_ascii_uppercase().contains("NOT NULL") {
                        definition.push_str(" NULL");
                    }
                    statements.push(format!("ALTER TABLE {qualified} ALTER COLUMN {definition}"));
                }
            }

            let original_indexes: std::collections::HashMap<&str, &IndexDef> = original
                .indexes
                .iter()
                .map(|index| (index.name.as_str(), index))
                .collect();
            let modified_indexes: std::collections::HashMap<&str, &IndexDef> = modified
                .indexes
                .iter()
                .map(|index| (index.name.as_str(), index))
                .collect();
            for index in &modified.indexes {
                if !index.primary && !original_indexes.contains_key(index.name.as_str()) {
                    statements.extend(create_index_statements(
                        &qualified,
                        std::slice::from_ref(index),
                    ));
                }
            }
            for index in &original.indexes {
                if index.name == "PRIMARY" || index.primary {
                    continue;
                }
                if !modified_indexes.contains_key(index.name.as_str()) {
                    statements.push(format!(
                        "DROP INDEX {} ON {qualified}",
                        quote_identifier(&index.name)
                    ));
                }
            }

            statements.join(";\n")
        }
    }
}

fn create_index_statements(table: &str, indexes: &[IndexDef]) -> Vec<String> {
    indexes
        .iter()
        .filter(|index| !index.primary && !index.columns.is_empty())
        .map(|index| {
            format!(
                "CREATE {}INDEX {} ON {table} ({})",
                if index.unique { "UNIQUE " } else { "" },
                quote_identifier(&index.name),
                index
                    .columns
                    .iter()
                    .map(|column| quote_identifier(column))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect()
}

/// Convert a `UserAccount`'s contiguous decoded rows into the model's `BTreeSet` of
/// privileges (used by `user.rs`).
pub(crate) fn privilege_set(
    names: impl IntoIterator<Item = String>,
) -> BTreeSet<rustgrid_core::Privilege> {
    names
        .into_iter()
        .filter_map(|name| rustgrid_core::Privilege::from_sql_name(&name))
        .collect()
}
