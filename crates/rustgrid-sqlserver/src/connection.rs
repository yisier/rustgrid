use std::borrow::Cow;
use std::collections::BTreeSet;

use async_trait::async_trait;
use futures_util::TryStreamExt;
use rustgrid_core::{
    BackupObjectKind, CellValue, CheckDef, ColumnDef, ColumnInfo, Connection, DatabaseInfo,
    DatabaseOptions, DriverId, Error, ForeignKeyDef, IndexColumnDef, IndexDef, ObjectDump,
    ObjectKind, ObjectPrivilegeRow, PageRequest, PrivilegeCatalog, QueryResult, Result,
    RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind, RowInsert, RowUpdate,
    ServerSecurableGrant, TableInfo, TableOptions, TablePage, TableSchema, TableStatus, TriggerDef,
    UniqueKeyDef, UserAccount, UserDetails, UserEdit, UserEditSection, UserMapping, ViewDetails,
    ViewEdit, ViewInfo,
};
use tiberius::{QueryItem, Row, ToSql};

use crate::driver::map_query_error;
use crate::helpers::{
    decode_cell, filter_clause, is_dml, leading_keyword, order_clause, qualified_name,
    quote_identifier, quote_literal, render_literal, returns_result_set, split_statements,
};
use crate::pool::{ConnectionManager, SqlClient};
use crate::user;

/// The default schema SQL Server objects live in.
const DEFAULT_SCHEMA: &str = "dbo";

/// A connected SQL Server session pool, implementing the engine-agnostic [`Connection`].
pub struct SqlServerConnection {
    pub(crate) pool: bb8::Pool<ConnectionManager>,
    /// The tunnel forwarding this connection, kept alive for as long as the connection is.
    _tunnel: Option<rustgrid_tunnel::Tunnel>,
}

impl SqlServerConnection {
    pub(crate) fn new(
        pool: bb8::Pool<ConnectionManager>,
        tunnel: Option<rustgrid_tunnel::Tunnel>,
    ) -> Self {
        Self {
            pool,
            _tunnel: tunnel,
        }
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

    /// The table's `IDENTITY` column name, when it has one. Needed both by restore (to wrap
    /// `IDENTITY_INSERT`) and by inserts that explicitly carry the identity value.
    pub(crate) async fn identity_column(
        &self,
        database: &str,
        schema: &str,
        table: &str,
    ) -> Result<Option<String>> {
        let sql = format!(
            "SELECT TOP 1 c.name FROM {db}.sys.columns c \
             JOIN {db}.sys.objects o ON o.object_id = c.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.name = @P1 AND s.name = @P2 AND c.is_identity = 1",
            db = quote_identifier(database)
        );
        self.scalar_text(
            None,
            &sql,
            &[Some(table.to_string()), Some(schema.to_string())],
        )
        .await
        .map(|value| value.filter(|value| !value.is_empty()))
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

    /// Change a database's owner. `ALTER AUTHORIZATION` requires the target to be bracket-quoted
    /// by literal, because identifiers cannot be parameterized here.
    async fn apply_database_owner(&self, database: &str, owner: &str) -> Result<()> {
        let sql = format!(
            "ALTER AUTHORIZATION ON DATABASE::{} TO {}",
            quote_identifier(database),
            quote_identifier(owner)
        );
        self.execute_batch(None, &sql).await
    }

    /// Rebuild a table's `CREATE TABLE` script plus its non-clustered indexes from catalog
    /// metadata, since SQL Server does not store a table's original DDL.
    pub(crate) async fn table_ddl(
        &self,
        database: &str,
        schema: &str,
        table: &str,
        is_view: bool,
    ) -> Result<String> {
        if is_view {
            return Ok(format!(
                "CREATE VIEW {}.{} AS\n/* definition unavailable */\nGO",
                quote_identifier(schema),
                quote_identifier(table)
            ));
        }
        let status = self
            .table_status(database, &format!("{schema}.{table}"))
            .await?;
        let db = quote_identifier(database);
        let column_sql = format!(
            "SELECT c.name, t.name, c.max_length, c.precision, c.scale, c.is_nullable, \
                    c.is_identity, dc.definition, c.is_computed, cc.definition \
             FROM {db}.sys.columns c \
             JOIN {db}.sys.types t ON t.user_type_id = c.user_type_id \
             JOIN {db}.sys.objects o ON o.object_id = c.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             LEFT JOIN {db}.sys.default_constraints dc ON dc.parent_object_id = c.object_id \
                  AND dc.parent_column_id = c.column_id \
             LEFT JOIN {db}.sys.computed_columns cc ON cc.object_id = c.object_id \
                  AND cc.column_id = c.column_id \
             WHERE o.name = @P1 AND s.name = @P2 \
             ORDER BY c.column_id",
            db = db
        );
        let columns = self
            .run(
                None,
                &column_sql,
                &[Some(table.to_string()), Some(schema.to_string())],
            )
            .await?;

        let mut definitions: Vec<String> = Vec::new();
        let mut primary_columns: Vec<String> = Vec::new();
        for row in &columns.rows {
            let name = quote_identifier(&text_cell(row, 0));
            if bool_cell(row, 8) {
                definitions.push(format!("  {name} AS {}", text_cell(row, 9)));
                continue;
            }
            let type_sql = render_column_type(
                &text_cell(row, 1),
                u64_cell(row, 2),
                u64_cell(row, 3),
                u64_cell(row, 4),
            );
            let mut definition = format!("  {name} {type_sql}");
            if bool_cell(row, 6) {
                definition.push_str(" IDENTITY(1,1)");
            }
            definition.push_str(if bool_cell(row, 5) {
                " NULL"
            } else {
                " NOT NULL"
            });
            let default = text_cell(row, 7);
            if !default.is_empty() {
                definition.push_str(&format!(" DEFAULT {default}"));
            }
            definitions.push(definition);
        }

        let pk_sql = format!(
            "SELECT c.name FROM {db}.sys.indexes i \
             JOIN {db}.sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
             JOIN {db}.sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
             JOIN {db}.sys.objects o ON o.object_id = i.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE i.is_primary_key = 1 AND o.name = @P1 AND s.name = @P2 \
             ORDER BY ic.key_ordinal",
            db = db
        );
        let pk = self
            .run(
                None,
                &pk_sql,
                &[Some(table.to_string()), Some(schema.to_string())],
            )
            .await?;
        for row in &pk.rows {
            primary_columns.push(quote_identifier(&text_cell(row, 0)));
        }
        if !primary_columns.is_empty() {
            definitions.push(format!("  PRIMARY KEY ({})", primary_columns.join(", ")));
        }

        let mut ddl = format!(
            "CREATE TABLE {}.{} (\n{}\n)\nGO",
            quote_identifier(schema),
            quote_identifier(table),
            definitions.join(",\n")
        );
        if let Some(comment) = status.comment.filter(|value| !value.is_empty()) {
            ddl = format!("-- {comment}\n{ddl}");
        }

        let index_sql = format!(
            "SELECT i.name, i.is_unique, c.name, ic.is_descending_key, i.type \
             FROM {db}.sys.indexes i \
             JOIN {db}.sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
             JOIN {db}.sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
             JOIN {db}.sys.objects o ON o.object_id = i.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE i.is_primary_key = 0 AND i.is_unique_constraint = 0 \
               AND i.type IN (1, 2) AND i.name IS NOT NULL \
               AND ic.is_included_column = 0 AND o.name = @P1 AND s.name = @P2 \
             ORDER BY i.name, ic.key_ordinal",
            db = db
        );
        let indexes = self
            .run(
                None,
                &index_sql,
                &[Some(table.to_string()), Some(schema.to_string())],
            )
            .await?;
        let mut current: Option<(String, bool, bool, Vec<String>)> = None;
        let mut index_scripts: Vec<String> = Vec::new();
        let flush = |current: Option<(String, bool, bool, Vec<String>)>, out: &mut Vec<String>| {
            if let Some((name, unique, clustered, columns)) = current
                && !columns.is_empty()
            {
                out.push(format!(
                    "CREATE {}{}INDEX {} ON {}.{} ({})\nGO",
                    if unique { "UNIQUE " } else { "" },
                    if clustered { "CLUSTERED " } else { "" },
                    quote_identifier(&name),
                    quote_identifier(schema),
                    quote_identifier(table),
                    columns.join(", ")
                ));
            }
        };
        for row in &indexes.rows {
            let index_name = text_cell(row, 0);
            let unique = bool_cell(row, 1);
            let clustered = int_cell(row, 4) == Some(1);
            let column = if bool_cell(row, 3) {
                format!("{} DESC", quote_identifier(&text_cell(row, 2)))
            } else {
                quote_identifier(&text_cell(row, 2))
            };
            match current.as_mut() {
                Some((name, _, _, columns)) if *name == index_name => columns.push(column),
                _ => {
                    flush(current.take(), &mut index_scripts);
                    current = Some((index_name, unique, clustered, vec![column]));
                }
            }
        }
        flush(current.take(), &mut index_scripts);
        for script in index_scripts {
            ddl.push_str(&format!("\n\n{script}"));
        }
        Ok(ddl)
    }
}

#[async_trait]
impl Connection for SqlServerConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new("sqlserver")
    }

    /// T-SQL's scanner, tracking `BEGIN`/`CASE …END` so a block body is not split on its inner
    /// semicolons.
    fn split_statements(&self, sql: &str) -> Vec<String> {
        split_statements(sql)
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
                    CASE WHEN pk.COLUMN_NAME IS NULL THEN 0 ELSE 1 END AS is_pk, \
                    CASE WHEN sc.is_identity IS NULL THEN 0 ELSE sc.is_identity END AS is_identity, \
                    CASE WHEN sc.is_computed IS NULL THEN 0 ELSE sc.is_computed END AS is_computed \
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
             LEFT JOIN ( \
                 SELECT s.name AS schema_name, o.name AS table_name, col.name AS column_name, \
                        col.is_identity, col.is_computed \
                 FROM {db}.sys.objects o \
                 JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
                 JOIN {db}.sys.columns col ON col.object_id = o.object_id \
             ) sc ON sc.schema_name = c.TABLE_SCHEMA \
                 AND sc.table_name = c.TABLE_NAME AND sc.column_name = c.COLUMN_NAME \
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
                auto_increment: text_cell(row, 8) == "1",
                generated: text_cell(row, 9) == "1",
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

        let total_rows = if page.skip_total {
            None
        } else {
            self.scalar_text(
                None,
                &format!("SELECT COUNT(*) FROM {qualified}{where_clause}"),
                &filter_params,
            )
            .await
            .ok()
            .flatten()
            .and_then(|value| value.parse::<u64>().ok())
        };

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

        // A transferred row carries the source's identity value. SQL Server rejects an explicit
        // value unless `IDENTITY_INSERT` is on, but turning it on also *requires* every insert to
        // name the identity column, so only do it when this batch actually carries that column.
        // The plain batch form is required because a `SET` sent through `sp_executesql` only
        // lives inside that nested batch.
        let identity_column = self.identity_column(database, &schema, &bare).await?;
        let writes_identity = identity_column.as_ref().is_some_and(|identity| {
            rows.iter().any(|insert| {
                insert
                    .values
                    .iter()
                    .any(|(column, _)| column.eq_ignore_ascii_case(identity))
            })
        });
        if writes_identity {
            run_plain_batch(&mut client, &format!("SET IDENTITY_INSERT {qualified} ON")).await?;
        }
        let outcome: Result<()> = async {
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
        .await;
        if writes_identity {
            let _ =
                run_plain_batch(&mut client, &format!("SET IDENTITY_INSERT {qualified} OFF")).await;
        }
        outcome
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
            // `GO` is a client-side separator, so the batch is rebuilt from the split
            // statements rather than resending the source (which would still contain it).
            let results = run_batch_sets(&mut client, &statements.join(";\n")).await?;
            return Ok(results
                .into_iter()
                .find(|result| result.has_result_set)
                .unwrap_or_else(|| empty_result(sql.trim().to_string())));
        }
        // A script the splitter could not break into several statements —e.g. several `SELECT`s
        // without semicolons, which SQL Server runs as one batch —carries multiple result sets.
        // The batch path returns them all and this keeps only the first, matching the single
        // result `execute_query` promises (the query editor uses `execute_query_many`).
        if statements.len() == 1 && returns_result_set(&statements[0]) {
            let results = run_batch_sets(&mut client, &statements[0]).await?;
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
            // round-trips; run it whole (without the client-side `GO` lines) and return one
            // result per result set.
            return run_batch_sets(&mut client, &statements.join(";\n")).await;
        }
        // A single statement the splitter could not divide —several `SELECT`s without
        // semicolons, which SQL Server runs as one batch —still carries every result set.
        if statements.len() == 1 && returns_result_set(&statements[0]) {
            return run_batch_sets(&mut client, &statements[0]).await;
        }
        let mut results = Vec::with_capacity(statements.len());
        for statement in &statements {
            results.push(run_statement(&mut client, statement, &[]).await?);
        }
        Ok(results)
    }

    async fn create_database(&self, name: &str, options: &DatabaseOptions) -> Result<()> {
        let sql = create_database_sql(name, options);
        if !sql.is_empty() {
            self.execute_batch(None, &sql).await?;
        }
        // `CREATE DATABASE ... COLLATE` leaves the owner at the server default; set it explicitly
        // when the dialog named one.
        if let Some(owner) = non_empty(&options.owner) {
            self.apply_database_owner(name, owner).await?;
        }
        Ok(())
    }

    fn create_database_sql(&self, name: &str, options: &DatabaseOptions) -> String {
        create_database_sql(name, options)
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

    async fn database_options(&self, name: &str) -> Result<DatabaseOptions> {
        let sql = "SELECT collation_name, SUSER_NAME(owner_sid), \
                          CONVERT(nvarchar(20), recovery_model_desc), \
                          CONVERT(nvarchar(20), compatibility_level) \
                   FROM sys.databases WHERE name = @P1";
        let result = self.run(None, sql, &[Some(name.to_string())]).await?;
        let row = result.rows.first();
        Ok(DatabaseOptions {
            // SQL Server selects a charset through the collation, so there is no separate one.
            charset: String::new(),
            collation: row.map(|row| text_cell(row, 0)).unwrap_or_default(),
            owner: row.map(|row| text_cell(row, 1)).unwrap_or_default(),
            recovery_model: row.map(|row| text_cell(row, 2)).unwrap_or_default(),
            compatibility_level: row.map(|row| text_cell(row, 3)).unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn database_owners(&self) -> Result<Vec<String>> {
        let result = self
            .run(
                None,
                "SELECT name FROM sys.server_principals \
                 WHERE type IN ('S', 'U', 'G') AND is_disabled = 0 AND name NOT LIKE '##%' \
                 ORDER BY CASE WHEN name = SUSER_SNAME() THEN 0 ELSE 1 END, name",
                &[],
            )
            .await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|row| row.first().map(CellValue::as_display))
            .collect())
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
             WHERE o.name = @P1 AND s.name = @P2 AND ps.index_id IN (0, 1) \
             GROUP BY o.create_date, o.modify_date",
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

    async fn object_ddl(
        &self,
        database: &str,
        name: &str,
        is_view: bool,
    ) -> Result<Option<String>> {
        let (schema, bare) = self.resolve_object(database, name).await?;
        // A view's source lives in `sys.sql_modules`; `OBJECT_DEFINITION` returns it and `NULL`
        // for a table, which has no original DDL to recover.
        let definition = self
            .scalar_text(
                None,
                &format!(
                    "USE {}; SELECT OBJECT_DEFINITION(OBJECT_ID(N'{}.{}'))",
                    quote_identifier(database),
                    escape_literal_part(&schema),
                    escape_literal_part(&bare)
                ),
                &[],
            )
            .await?;
        if let Some(definition) = definition.filter(|text| !text.trim().is_empty()) {
            return Ok(Some(definition));
        }
        Ok(Some(
            self.table_ddl(database, &schema, &bare, is_view).await?,
        ))
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

    async fn alter_database_options(
        &self,
        name: &str,
        original: &DatabaseOptions,
        modified: &DatabaseOptions,
    ) -> Result<()> {
        let sql = alter_database_sql(name, original, modified);
        if !sql.is_empty() {
            self.execute_batch(None, &sql).await?;
        }
        // `ALTER DATABASE ... SET OWNER` is not valid T-SQL; ownership moves through
        // `ALTER AUTHORIZATION`.
        if modified.owner != original.owner
            && let Some(owner) = non_empty(&modified.owner)
        {
            self.apply_database_owner(name, owner).await?;
        }
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

    async fn list_schemas(&self, database: &str) -> Result<Vec<String>> {
        // `sys.schemas` is database-scoped, so fully qualify it with the target database (a pooled
        // session's current database cannot be assumed). Fixed roles are excluded so the tree only
        // shows user schemas.
        let sql = format!(
            "SELECT name FROM {db}.sys.schemas \
             WHERE name NOT IN ('sys', 'INFORMATION_SCHEMA', 'guest', \
                                'db_owner', 'db_accessadmin', 'db_securityadmin', \
                                'db_ddladmin', 'db_backupoperator', 'db_datareader', \
                                'db_datawriter', 'db_denydatareader', 'db_denydatawriter') \
             ORDER BY name",
            db = quote_identifier(database)
        );
        let result = self.run(None, &sql, &[]).await?;
        Ok(result
            .rows
            .into_iter()
            .filter_map(|row| row.first().map(CellValue::as_display))
            .collect())
    }

    async fn create_schema(&self, database: &str, schema: &str) -> Result<()> {
        let sql = format!(
            "USE {}; EXEC('CREATE SCHEMA {}')",
            quote_identifier(database),
            quote_identifier(schema)
        );
        self.execute_batch(None, &sql).await
    }

    async fn drop_schema(&self, database: &str, schema: &str) -> Result<()> {
        let sql = format!(
            "USE {}; EXEC('DROP SCHEMA {}')",
            quote_identifier(database),
            quote_identifier(schema)
        );
        self.execute_batch(None, &sql).await
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
                    CASE WHEN pk.column_id IS NULL THEN 0 ELSE 1 END, c.is_sparse \
             FROM {db}.sys.columns c \
             JOIN {db}.sys.objects o ON o.object_id = c.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             JOIN {db}.sys.types ty ON ty.user_type_id = c.user_type_id \
             LEFT JOIN {db}.sys.default_constraints dc \
               ON dc.parent_object_id = c.object_id AND dc.parent_column_id = c.column_id \
             LEFT JOIN {db}.sys.extended_properties ep \
               ON ep.major_id = c.object_id AND ep.minor_id = c.column_id \
                  AND ep.name = N'MS_Description' AND ep.class = 1 \
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
                identity: String::new(),
                identity_start: String::new(),
                identity_increment: String::new(),
                unsigned: false,
                zerofill: false,
                sparse: bool_cell(row, 13),
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
            "SELECT i.name, i.is_unique, i.is_primary_key, i.type_desc, \
                    (SELECT CAST(ep.value AS nvarchar(max)) FROM {db}.sys.extended_properties ep \
                     WHERE ep.major_id = i.object_id AND ep.minor_id = i.index_id \
                       AND ep.name = N'MS_Description' AND ep.class = 7) AS comment \
             FROM {db}.sys.indexes i \
             JOIN {db}.sys.objects o ON o.object_id = i.object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.name = @P1 AND s.name = @P2 AND i.name IS NOT NULL \
               AND i.is_unique_constraint = 0 \
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
                comment: text_cell(row, 4),
            });
        }

        let unique_keys = self.unique_keys(database, &schema_name, &table).await?;
        let checks = self.checks(database, &schema_name, &table).await?;
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
                &[Some(table.clone()), Some(schema_name.clone())],
            )
            .await?
            .unwrap_or_default();

        let table_comment = self
            .scalar_text(
                None,
                &format!(
                    "SELECT CAST(ep.value AS nvarchar(max)) FROM {db}.sys.extended_properties ep \
                     JOIN {db}.sys.tables t ON t.object_id = ep.major_id \
                     JOIN {db}.sys.schemas s ON s.schema_id = t.schema_id \
                     WHERE s.name = @P1 AND t.name = @P2 AND ep.minor_id = 0 \
                       AND ep.name = N'MS_Description' AND ep.class = 1"
                ),
                &[Some(schema_name), Some(table)],
            )
            .await?
            .unwrap_or_default();

        Ok(TableSchema {
            columns,
            indexes,
            unique_keys,
            checks,
            excludes: Vec::new(),
            rules: Vec::new(),
            foreign_keys,
            triggers,
            partitioning: String::new(),
            options: TableOptions {
                engine: String::new(),
                charset: String::new(),
                collation,
                comment: table_comment,
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
        table_schema_sql(&schema, &bare, original, modified)
    }

    fn table_schema_trigger_sql(
        &self,
        _database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> Vec<String> {
        let (schema, _) = split_schema(table)
            .map(|(schema, bare)| (schema.to_string(), bare.to_string()))
            .unwrap_or_else(|| (DEFAULT_SCHEMA.to_string(), table.to_string()));
        trigger_sql(&schema, original, modified)
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

    async fn user_mappings(&self, user_name: &str, _host: &str) -> Result<Vec<UserMapping>> {
        user::user_mappings(self, user_name).await
    }

    async fn user_securables(
        &self,
        user_name: &str,
        _host: &str,
    ) -> Result<Vec<ServerSecurableGrant>> {
        user::user_securables(self, user_name).await
    }

    fn login_types(&self) -> Vec<&'static str> {
        user::login_types()
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

    fn privilege_catalog(&self) -> PrivilegeCatalog {
        user::privilege_catalog()
    }

    fn authentication_plugins(&self) -> Vec<&'static str> {
        user::authentication_plugins()
    }

    async fn object_privilege_matrix(
        &self,
        database: &str,
        name: &str,
    ) -> Result<Vec<ObjectPrivilegeRow>> {
        user::object_privilege_matrix(self, database, name).await
    }

    async fn set_object_privileges(
        &self,
        database: &str,
        name: &str,
        rows: &[ObjectPrivilegeRow],
    ) -> Result<()> {
        user::set_object_privileges(self, database, name, rows).await
    }

    fn object_privileges_sql(
        &self,
        database: &str,
        name: &str,
        original: &[ObjectPrivilegeRow],
        rows: &[ObjectPrivilegeRow],
    ) -> String {
        user::object_privileges_sql(database, name, original, rows)
    }
}

impl SqlServerConnection {
    async fn index_columns(&self, database: &str, index: &str) -> Result<Vec<IndexColumnDef>> {
        let db = quote_identifier(database);
        let sql = format!(
            "SELECT c.name, ic.is_descending_key FROM {db}.sys.index_columns ic \
             JOIN {db}.sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
             JOIN {db}.sys.indexes i ON i.object_id = ic.object_id AND i.index_id = ic.index_id \
             WHERE i.name = @P1 ORDER BY ic.key_ordinal",
        );
        let result = self.run(None, &sql, &[Some(index.to_string())]).await?;
        Ok(result
            .rows
            .into_iter()
            .map(|row| IndexColumnDef {
                name: text_cell(&row, 0),
                descending: bool_cell(&row, 1),
                // SQL Server indexes have no prefix length (子部分 is MySQL's).
                prefix_length: String::new(),
            })
            .collect())
    }

    /// The table's `UNIQUE` constraints. SQL Server keeps them apart from plain unique indexes
    /// (`sys.indexes.is_unique_constraint = 1`), which is why the designer has a 唯一键tab.
    async fn unique_keys(
        &self,
        database: &str,
        schema: &str,
        table: &str,
    ) -> Result<Vec<UniqueKeyDef>> {
        let db = quote_identifier(database);
        let sql = format!(
            "SELECT kc.name, c.name FROM {db}.sys.key_constraints kc \
             JOIN {db}.sys.objects o ON o.object_id = kc.parent_object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             JOIN {db}.sys.index_columns ic \
               ON ic.object_id = kc.parent_object_id AND ic.index_id = kc.unique_index_id \
             JOIN {db}.sys.columns c \
               ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
             WHERE kc.type = 'UQ' AND o.name = @P1 AND s.name = @P2 \
             ORDER BY kc.name, ic.key_ordinal"
        );
        let result = self
            .run(
                None,
                &sql,
                &[Some(table.to_string()), Some(schema.to_string())],
            )
            .await?;
        let mut unique_keys: Vec<UniqueKeyDef> = Vec::new();
        for row in &result.rows {
            let name = text_cell(row, 0);
            let column = text_cell(row, 1);
            match unique_keys.iter_mut().find(|key| key.name == name) {
                Some(key) => key.columns.push(column),
                None => unique_keys.push(UniqueKeyDef {
                    name,
                    columns: vec![column],
                }),
            }
        }
        Ok(unique_keys)
    }

    /// The table's check constraints, with the definition's outer parentheses removed (the catalog
    /// stores `([qty]>(0))`; the designer edits `[qty]>(0)` and the DDL adds the parentheses back).
    async fn checks(&self, database: &str, schema: &str, table: &str) -> Result<Vec<CheckDef>> {
        let db = quote_identifier(database);
        let sql = format!(
            "SELECT cc.name, cc.definition FROM {db}.sys.check_constraints cc \
             JOIN {db}.sys.objects o ON o.object_id = cc.parent_object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.name = @P1 AND s.name = @P2 ORDER BY cc.name"
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
            .map(|row| CheckDef {
                name: text_cell(row, 0),
                expression: strip_outer_parens(&text_cell(row, 1)),
            })
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
            "SELECT fk.name, fk.object_id, rs.name, ro.name, \
                    fk.delete_referential_action_desc, fk.update_referential_action_desc \
             FROM {db}.sys.foreign_keys fk \
             JOIN {db}.sys.objects o ON o.object_id = fk.parent_object_id \
             JOIN {db}.sys.schemas s ON s.schema_id = o.schema_id \
             JOIN {db}.sys.objects ro ON ro.object_id = fk.referenced_object_id \
             JOIN {db}.sys.schemas rs ON rs.schema_id = ro.schema_id \
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
                referenced_schema: text_cell(row, 2),
                referenced_table: text_cell(row, 3),
                referenced_columns,
                on_delete: normalize_action(&text_cell(row, 4)),
                on_update: normalize_action(&text_cell(row, 5)),
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

/// A trimmed non-empty value, or `None`.
fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn create_database_sql(name: &str, options: &DatabaseOptions) -> String {
    let mut sql = format!("CREATE DATABASE {}", quote_identifier(name));
    if let Some(collation) = non_empty(&options.collation) {
        sql.push_str(" COLLATE ");
        sql.push_str(collation);
    }
    sql
}

fn alter_database_sql(
    name: &str,
    original: &DatabaseOptions,
    modified: &DatabaseOptions,
) -> String {
    let mut statements = Vec::new();
    if modified.collation != original.collation
        && let Some(collation) = non_empty(&modified.collation)
    {
        statements.push(format!(
            "ALTER DATABASE {} COLLATE {}",
            quote_identifier(name),
            collation
        ));
    }
    if modified.recovery_model != original.recovery_model
        && let Some(model) = non_empty(&modified.recovery_model)
    {
        // `SIMPLE` | `FULL` | `BULK_LOGGED` are keywords, not literals; they come from a fixed
        // dropdown, so appending the value verbatim is safe.
        statements.push(format!(
            "ALTER DATABASE {} SET RECOVERY {}",
            quote_identifier(name),
            model.to_ascii_uppercase()
        ));
    }
    if modified.compatibility_level != original.compatibility_level
        && let Some(level) = non_empty(&modified.compatibility_level)
    {
        statements.push(format!(
            "ALTER DATABASE {} SET COMPATIBILITY_LEVEL = {}",
            quote_identifier(name),
            level
        ));
    }
    statements.join("; ")
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
        // Result-set metadata only; an editable query grid re-attaches the catalog's flags.
        auto_increment: false,
        generated: false,
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

/// Render a column's T-SQL type: length-qualified for character/binary types (in characters for
/// `n`-prefixed, bytes otherwise), precision/scale for `decimal`/`numeric`, and bare otherwise.
fn render_column_type(
    type_name: &str,
    max_length: Option<u64>,
    precision: Option<u64>,
    scale: Option<u64>,
) -> String {
    let max = || match max_length {
        // tiberius reports `MAX` as a huge sentinel; treat anything over 8000 as `MAX`.
        Some(value) if value >= 8000 => "MAX".to_string(),
        Some(value) => value.to_string(),
        None => "MAX".to_string(),
    };
    match type_name.to_ascii_lowercase().as_str() {
        "varchar" | "char" | "varbinary" | "binary" => format!("{type_name}({})", max()),
        "nvarchar" | "nchar" => {
            let length = match max_length {
                Some(value) if value >= 8000 => "MAX".to_string(),
                Some(value) => (value / 2).to_string(),
                None => "MAX".to_string(),
            };
            format!("{type_name}({length})")
        }
        "decimal" | "numeric" => {
            format!(
                "{type_name}({},{})",
                precision.unwrap_or(18),
                scale.unwrap_or(0)
            )
        }
        "datetime2" | "datetimeoffset" | "time" => {
            format!("{type_name}({})", scale.unwrap_or(7))
        }
        _ => type_name.to_string(),
    }
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
    // A computed column carries an expression instead of a type.
    if !column.generated.trim().is_empty() {
        out.push_str(" AS ");
        out.push_str(&wrap_parens(column.generated.trim()));
        if column.stored {
            out.push_str(" PERSISTED");
        }
        return out;
    }
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
    if !column.collation.trim().is_empty() {
        out.push_str(" COLLATE ");
        out.push_str(column.collation.trim());
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
    if column.sparse {
        out.push_str(" SPARSE");
    }
    out
}

/// One `sp_addextendedproperty` / `sp_updateextendedproperty` / `sp_dropextendedproperty` call for
/// SQL Server's `MS_Description` extended property (its table/column comment).
fn extended_property_statement(
    action: &str,
    schema: &str,
    table: &str,
    level2: Option<(&str, &str)>,
    value: &str,
) -> String {
    let mut sql = format!("EXEC sp_{action}extendedproperty @name=N'MS_Description'");
    if action != "drop" {
        sql.push_str(", @value=");
        sql.push_str(&quote_literal(value));
    }
    sql.push_str(", @level0type=N'SCHEMA', @level0name=");
    sql.push_str(&quote_literal(schema));
    sql.push_str(", @level1type=N'TABLE', @level1name=");
    sql.push_str(&quote_literal(table));
    if let Some((kind, name)) = level2 {
        sql.push_str(", @level2type=");
        sql.push_str(&quote_literal(kind));
        sql.push_str(", @level2name=");
        sql.push_str(&quote_literal(name));
    }
    sql
}

/// The extended-property statements that reconcile table/column comments between `original` and
/// `modified`. SQL Server stores comments as `MS_Description`, not in the column definition, so they
/// are emitted as separate statements after the table/index DDL.
fn comment_statements(
    schema: &str,
    table: &str,
    original: Option<&TableSchema>,
    modified: &TableSchema,
) -> Vec<String> {
    let mut statements = Vec::new();
    let original_comment = original
        .map(|schema| schema.options.comment.as_str())
        .unwrap_or_default();
    if original_comment != modified.options.comment {
        let action = if original_comment.is_empty() {
            "add"
        } else if modified.options.comment.is_empty() {
            "drop"
        } else {
            "update"
        };
        statements.push(extended_property_statement(
            action,
            schema,
            table,
            None,
            &modified.options.comment,
        ));
    }

    let original_columns: std::collections::HashMap<&str, &ColumnDef> = original
        .map(|schema| {
            schema
                .columns
                .iter()
                .map(|column| (column.name.as_str(), column))
                .collect()
        })
        .unwrap_or_default();
    for column in &modified.columns {
        let previous = original_columns
            .get(column.name.as_str())
            .map(|column| column.comment.as_str())
            .unwrap_or_default();
        if previous != column.comment {
            let action = if previous.is_empty() {
                "add"
            } else if column.comment.is_empty() {
                "drop"
            } else {
                "update"
            };
            statements.push(extended_property_statement(
                action,
                schema,
                table,
                Some(("COLUMN", &column.name)),
                &column.comment,
            ));
        }
    }
    statements
}

/// `MS_Description` for a newly created (or recreated) index. SQL Server stores index comments in
/// `sys.extended_properties` with `minor_id = index_id`, so a dropped/recreated index loses its
/// comment and it must be written again.
fn index_comment_statement(schema: &str, table: &str, index: &IndexDef) -> Option<String> {
    (!index.comment.trim().is_empty()).then(|| {
        extended_property_statement(
            "add",
            schema,
            table,
            Some(("INDEX", &index.name)),
            index.comment.trim(),
        )
    })
}

/// `DROP`/`CREATE TRIGGER` statements for SQL Server's full trigger definitions.
fn trigger_sql(
    schema: &str,
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
                quote_identifier(schema),
                quote_identifier(&trigger.name)
            ));
        }
    }
    for trigger in &modified.triggers {
        // A new full definition has no name yet (the grid's name cell is read-only until the
        // reload); the statement itself carries the trigger name.
        if trigger.statement.trim().is_empty() {
            continue;
        }
        let unchanged = original
            .map(|schema| schema.triggers.as_slice())
            .is_some_and(|triggers| {
                triggers
                    .iter()
                    .find(|candidate| candidate.name == trigger.name)
                    .is_some_and(|candidate| candidate == trigger)
            });
        if !unchanged {
            statements.push(
                trigger
                    .statement
                    .trim()
                    .trim_end_matches(';')
                    .trim_end()
                    .to_string(),
            );
        }
    }
    statements
}

fn table_schema_sql(
    schema: &str,
    table: &str,
    original: Option<&TableSchema>,
    modified: &TableSchema,
) -> String {
    let qualified = format!("{}.{}", quote_identifier(schema), quote_identifier(table));
    let comments = comment_statements(schema, table, original, modified);
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
            definitions.extend(unique_key_definitions(&modified.unique_keys));
            definitions.extend(check_definitions(&modified.checks));
            definitions.extend(foreign_key_definitions(&modified.foreign_keys));
            let mut statements = vec![format!(
                "CREATE TABLE {qualified} (\n    {}\n)",
                definitions.join(",\n    ")
            )];
            statements.extend(create_index_statements(&qualified, &modified.indexes));
            statements.extend(
                modified
                    .indexes
                    .iter()
                    .filter(|index| !index.primary)
                    .filter_map(|index| index_comment_statement(schema, table, index)),
            );
            statements.extend(comments);
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

            // Foreign keys are dropped before columns and re-added at the end: dropping a column
            // that a foreign key still references fails, and adding one needs the new column first.
            let original_foreign_keys: std::collections::HashMap<&str, &ForeignKeyDef> = original
                .foreign_keys
                .iter()
                .filter(|foreign_key| !foreign_key.name.trim().is_empty())
                .map(|foreign_key| (foreign_key.name.as_str(), foreign_key))
                .collect();
            let modified_foreign_keys: std::collections::HashMap<&str, &ForeignKeyDef> = modified
                .foreign_keys
                .iter()
                .filter(|foreign_key| !foreign_key.name.trim().is_empty())
                .map(|foreign_key| (foreign_key.name.as_str(), foreign_key))
                .collect();
            for foreign_key in &original.foreign_keys {
                if foreign_key.name.trim().is_empty() {
                    continue;
                }
                let unchanged = modified_foreign_keys
                    .get(foreign_key.name.as_str())
                    .is_some_and(|candidate| *candidate == foreign_key);
                if !unchanged {
                    statements.push(drop_constraint_statement(&qualified, &foreign_key.name));
                }
            }

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
                    && column.generated.trim().is_empty()
                    && (previous.data_type != column.data_type
                        || previous.length != column.length
                        || previous.decimals != column.decimals
                        || previous.nullable != column.nullable
                        || previous.collation != column.collation
                        || previous.sparse != column.sparse)
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
                if index.primary {
                    continue;
                }
                match original_indexes.get(index.name.as_str()) {
                    None => {
                        statements.extend(create_index_statements(
                            &qualified,
                            std::slice::from_ref(index),
                        ));
                        if let Some(comment) = index_comment_statement(schema, table, index) {
                            statements.push(comment);
                        }
                    }
                    // SQL Server cannot alter an index: a changed one is dropped and created again —
                    // otherwise an edit (a new column, a `DESC`) would be silently lost. Dropping the
                    // index also drops its `MS_Description`, so the comment is written again.
                    Some(previous) if **previous != *index => {
                        statements.push(format!(
                            "DROP INDEX {} ON {qualified}",
                            quote_identifier(&index.name)
                        ));
                        statements.extend(create_index_statements(
                            &qualified,
                            std::slice::from_ref(index),
                        ));
                        if let Some(comment) = index_comment_statement(schema, table, index) {
                            statements.push(comment);
                        }
                    }
                    Some(_) => {}
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

            // Unique constraints. SQL Server cannot alter one in place, so a changed column list
            // is dropped and re-added; an unnamed row is skipped (it was created without a name
            // and cannot be referenced afterwards).
            let original_keys: std::collections::HashMap<&str, &UniqueKeyDef> = original
                .unique_keys
                .iter()
                .filter(|key| !key.name.trim().is_empty())
                .map(|key| (key.name.as_str(), key))
                .collect();
            for key in &modified.unique_keys {
                if key.name.trim().is_empty() || key.columns.is_empty() {
                    continue;
                }
                let unchanged = original_keys
                    .get(key.name.as_str())
                    .is_some_and(|previous| previous.columns == key.columns);
                if unchanged {
                    continue;
                }
                if original_keys.contains_key(key.name.as_str()) {
                    statements.push(drop_constraint_statement(&qualified, &key.name));
                }
                statements.push(format!(
                    "ALTER TABLE {qualified} ADD CONSTRAINT {} UNIQUE ({})",
                    quote_identifier(&key.name),
                    column_list(&key.columns)
                ));
            }
            for key in &original.unique_keys {
                if key.name.trim().is_empty() {
                    continue;
                }
                if !modified
                    .unique_keys
                    .iter()
                    .any(|candidate| candidate.name == key.name)
                {
                    statements.push(drop_constraint_statement(&qualified, &key.name));
                }
            }

            // Check constraints, reconciled the same way.
            let original_checks: std::collections::HashMap<&str, &CheckDef> = original
                .checks
                .iter()
                .filter(|check| !check.name.trim().is_empty())
                .map(|check| (check.name.as_str(), check))
                .collect();
            for check in &modified.checks {
                if check.name.trim().is_empty() || check.expression.trim().is_empty() {
                    continue;
                }
                let unchanged = original_checks
                    .get(check.name.as_str())
                    .is_some_and(|previous| previous.expression == check.expression);
                if unchanged {
                    continue;
                }
                if original_checks.contains_key(check.name.as_str()) {
                    statements.push(drop_constraint_statement(&qualified, &check.name));
                }
                statements.push(format!(
                    "ALTER TABLE {qualified} ADD CONSTRAINT {} CHECK ({})",
                    quote_identifier(&check.name),
                    check.expression.trim()
                ));
            }
            for check in &original.checks {
                if check.name.trim().is_empty() {
                    continue;
                }
                if !modified
                    .checks
                    .iter()
                    .any(|candidate| candidate.name == check.name)
                {
                    statements.push(drop_constraint_statement(&qualified, &check.name));
                }
            }

            // Re-add new and changed foreign keys (removed/changed ones were dropped above, before
            // any column they reference could be dropped).
            for foreign_key in &modified.foreign_keys {
                if foreign_key.name.trim().is_empty()
                    || foreign_key.columns.is_empty()
                    || foreign_key.referenced_table.trim().is_empty()
                {
                    continue;
                }
                let unchanged = original_foreign_keys
                    .get(foreign_key.name.as_str())
                    .is_some_and(|previous| *previous == foreign_key);
                if !unchanged {
                    statements.push(format!(
                        "ALTER TABLE {qualified} ADD {}",
                        foreign_key_body(foreign_key)
                    ));
                }
            }

            statements.extend(comments);
            statements.join(";\n")
        }
    }
}

fn create_index_statements(table: &str, indexes: &[IndexDef]) -> Vec<String> {
    indexes
        .iter()
        .filter(|index| !index.primary && !index.columns.is_empty())
        .map(|index| {
            // SQL Server's own vocabulary: an index is clustered or nonclustered and either kind
            // may be unique — `CREATE [UNIQUE] [CLUSTERED|NONCLUSTERED] INDEX`. Anything else in
            // `index_type` (a columnstore index, say) is left to the engine's default.
            let kind = match index.index_type.trim().to_ascii_uppercase().as_str() {
                "CLUSTERED" => "CLUSTERED ",
                "NONCLUSTERED" => "NONCLUSTERED ",
                _ => "",
            };
            format!(
                "CREATE {}{}INDEX {} ON {table} ({})",
                if index.unique { "UNIQUE " } else { "" },
                kind,
                quote_identifier(&index.name),
                index_column_list(&index.columns)
            )
        })
        .collect()
}

/// A quoted, comma-separated index column list, each column with its own `DESC` where it sorts
/// downwards.
fn index_column_list(columns: &[IndexColumnDef]) -> String {
    columns
        .iter()
        .map(|column| {
            let mut sql = quote_identifier(&column.name);
            if column.descending {
                sql.push_str(" DESC");
            }
            sql
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A quoted, comma-separated column list.
fn column_list(columns: &[String]) -> String {
    columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `UNIQUE` constraint clauses of a `CREATE TABLE`, before the check constraints.
fn unique_key_definitions(unique_keys: &[UniqueKeyDef]) -> Vec<String> {
    unique_keys
        .iter()
        .filter(|key| !key.columns.is_empty())
        .map(|key| {
            let body = format!("UNIQUE ({})", column_list(&key.columns));
            if key.name.trim().is_empty() {
                body
            } else {
                format!("CONSTRAINT {} {body}", quote_identifier(&key.name))
            }
        })
        .collect()
}

/// The `CHECK` constraint clauses of a `CREATE TABLE`.
fn check_definitions(checks: &[CheckDef]) -> Vec<String> {
    checks
        .iter()
        .filter(|check| !check.expression.trim().is_empty())
        .map(|check| {
            let body = format!("CHECK ({})", check.expression.trim());
            if check.name.trim().is_empty() {
                body
            } else {
                format!("CONSTRAINT {} {body}", quote_identifier(&check.name))
            }
        })
        .collect()
}

/// The `FOREIGN KEY` constraint clauses of a `CREATE TABLE`.
///
/// SQL Server used to introspect foreign keys but never emit them in the designer's DDL, so editing
/// the 外键 tab silently did nothing. They are now part of the create statement and reconciled by
/// `table_schema_sql` on alter.
fn foreign_key_definitions(foreign_keys: &[ForeignKeyDef]) -> Vec<String> {
    foreign_keys
        .iter()
        .filter(|foreign_key| {
            !foreign_key.columns.is_empty() && !foreign_key.referenced_table.trim().is_empty()
        })
        .map(foreign_key_body)
        .collect()
}

/// One `[CONSTRAINT name] FOREIGN KEY (...) REFERENCES [schema].[table] (...)` clause.
fn foreign_key_body(foreign_key: &ForeignKeyDef) -> String {
    let mut body = String::new();
    if !foreign_key.name.trim().is_empty() {
        body.push_str("CONSTRAINT ");
        body.push_str(&quote_identifier(&foreign_key.name));
        body.push(' ');
    }
    body.push_str("FOREIGN KEY (");
    body.push_str(&column_list(&foreign_key.columns));
    body.push_str(") REFERENCES ");
    if !foreign_key.referenced_schema.trim().is_empty() {
        body.push_str(&quote_identifier(foreign_key.referenced_schema.trim()));
        body.push('.');
    }
    body.push_str(&quote_identifier(&foreign_key.referenced_table));
    body.push_str(" (");
    body.push_str(&column_list(&foreign_key.referenced_columns));
    body.push(')');
    if !foreign_key.on_delete.trim().is_empty() {
        body.push_str(" ON DELETE ");
        body.push_str(foreign_key.on_delete.trim());
    }
    if !foreign_key.on_update.trim().is_empty() {
        body.push_str(" ON UPDATE ");
        body.push_str(foreign_key.on_update.trim());
    }
    body
}

/// Drop one named constraint (`DROP CONSTRAINT` covers unique, check, default and foreign keys).
fn drop_constraint_statement(table: &str, name: &str) -> String {
    format!(
        "ALTER TABLE {table} DROP CONSTRAINT {}",
        quote_identifier(name)
    )
}

/// Wrap an expression in parentheses unless it already is parenthesized.
fn wrap_parens(expression: &str) -> String {
    if expression.starts_with('(') && expression.ends_with(')') {
        expression.to_string()
    } else {
        format!("({expression})")
    }
}

/// Remove one pair of outer parentheses from a check definition, as the catalog stores it.
fn strip_outer_parens(definition: &str) -> String {
    let trimmed = definition.trim();
    if let Some(inner) = trimmed
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    {
        inner.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Normalize reported permission names into privilege ids (used by `user.rs`). SQL Server's
/// permission names are the grant keywords themselves, so no translation is needed; unknown
/// permissions are kept so an untouched grant is never silently dropped on save.
pub(crate) fn privilege_set(
    names: impl IntoIterator<Item = String>,
) -> BTreeSet<rustgrid_core::PrivilegeId> {
    names
        .into_iter()
        .map(|name| rustgrid_core::PrivilegeId::new(name.trim().to_ascii_uppercase()))
        .filter(|id| !id.as_str().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_table_mapped_from_mysql() {
        use rustgrid_core::{ColumnDef, DriverDialect, TableSchema, map_schema};

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
                    data_type: "longblob".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mapped = map_schema(&source, DriverDialect::Mysql, DriverDialect::SqlServer);
        let sql = table_schema_sql("dbo", "users", None, &mapped);
        assert!(sql.contains("[id] bigint IDENTITY(1,1)"), "{sql}");
        assert!(sql.contains("[name] nvarchar(255)"), "{sql}");
        assert!(sql.contains("[payload] varbinary(max)"), "{sql}");
    }

    #[test]
    fn renders_column_types_with_their_qualifiers() {
        // Character lengths are bytes for non-`n` types and characters for `n` types.
        assert_eq!(
            render_column_type("varchar", Some(50), None, None),
            "varchar(50)"
        );
        assert_eq!(
            render_column_type("nvarchar", Some(100), None, None),
            "nvarchar(50)"
        );
        assert_eq!(
            render_column_type("varbinary", Some(u64::MAX), None, None),
            "varbinary(MAX)"
        );
        assert_eq!(
            render_column_type("nvarchar", Some(u64::MAX), None, None),
            "nvarchar(MAX)"
        );
        assert_eq!(
            render_column_type("decimal", Some(9), Some(20), Some(6)),
            "decimal(20,6)"
        );
        assert_eq!(
            render_column_type("datetime2", Some(8), None, Some(7)),
            "datetime2(7)"
        );
        // Types without a qualifier are rendered bare.
        assert_eq!(render_column_type("int", Some(4), None, None), "int");
        assert_eq!(render_column_type("bit", Some(1), None, None), "bit");
    }

    /// A one-column `dbo.t` with `id int NOT NULL`; the callers add what they test.
    fn base_schema() -> TableSchema {
        TableSchema {
            columns: vec![ColumnDef {
                name: "id".to_string(),
                data_type: "int".to_string(),
                nullable: false,
                ..ColumnDef::default()
            }],
            ..TableSchema::default()
        }
    }

    #[test]
    fn creates_unique_key_and_check_constraints() {
        let mut schema = base_schema();
        schema.unique_keys.push(UniqueKeyDef {
            name: "uq_code".to_string(),
            columns: vec!["id".to_string(), "code".to_string()],
        });
        schema.checks.push(CheckDef {
            name: "ck_positive".to_string(),
            expression: "[id]>(0)".to_string(),
        });

        let sql = table_schema_sql("dbo", "t", None, &schema);
        assert!(
            sql.contains("CONSTRAINT [uq_code] UNIQUE ([id], [code])"),
            "{sql}"
        );
        assert!(
            sql.contains("CONSTRAINT [ck_positive] CHECK ([id]>(0))"),
            "{sql}"
        );
    }

    #[test]
    fn alters_unique_key_and_check_constraints_by_recreating_them() {
        let mut original = base_schema();
        original.unique_keys.push(UniqueKeyDef {
            name: "uq_code".to_string(),
            columns: vec!["id".to_string()],
        });
        original.checks.push(CheckDef {
            name: "ck_positive".to_string(),
            expression: "[id]>(0)".to_string(),
        });

        // A changed column list/expression is dropped and re-added; SQL Server cannot alter one.
        let mut modified = base_schema();
        modified.unique_keys.push(UniqueKeyDef {
            name: "uq_code".to_string(),
            columns: vec!["id".to_string(), "code".to_string()],
        });
        modified.checks.push(CheckDef {
            name: "ck_positive".to_string(),
            expression: "[id]>=(0)".to_string(),
        });

        let sql = table_schema_sql("dbo", "t", Some(&original), &modified);
        assert!(
            sql.contains("ALTER TABLE [dbo].[t] DROP CONSTRAINT [uq_code]"),
            "{sql}"
        );
        assert!(
            sql.contains("ALTER TABLE [dbo].[t] ADD CONSTRAINT [uq_code] UNIQUE ([id], [code])"),
            "{sql}"
        );
        assert!(
            sql.contains("ALTER TABLE [dbo].[t] DROP CONSTRAINT [ck_positive]"),
            "{sql}"
        );
        assert!(
            sql.contains("ALTER TABLE [dbo].[t] ADD CONSTRAINT [ck_positive] CHECK ([id]>=(0))"),
            "{sql}"
        );
    }

    #[test]
    fn builds_full_trigger_diff() {
        let original = TableSchema {
            triggers: vec![TriggerDef {
                name: "trg".to_string(),
                statement: "CREATE TRIGGER [trg] ON [dbo].[t] AFTER INSERT AS SELECT 1".to_string(),
                ..TriggerDef::default()
            }],
            ..TableSchema::default()
        };
        let mut modified = original.clone();
        modified.triggers[0].statement =
            "CREATE TRIGGER [trg] ON [dbo].[t] AFTER INSERT AS SELECT 2".to_string();

        let sql = trigger_sql("dbo", Some(&original), &modified);
        assert_eq!(sql[0], "DROP TRIGGER IF EXISTS [dbo].[trg]");
        assert!(sql[1].contains("SELECT 2"), "{sql:?}");
    }

    #[test]
    fn emits_index_comments_as_extended_properties() {
        let mut schema = base_schema();
        schema.indexes.push(IndexDef {
            name: "ix_id".to_string(),
            columns: vec![IndexColumnDef::new("id")],
            unique: false,
            primary: false,
            index_type: "NONCLUSTERED".to_string(),
            comment: "lookup".to_string(),
        });

        let sql = table_schema_sql("dbo", "t", None, &schema);
        assert!(
            sql.contains(
                "EXEC sp_addextendedproperty @name=N'MS_Description', @value=N'lookup', \
                 @level0type=N'SCHEMA', @level0name=N'dbo', @level1type=N'TABLE', @level1name=N't', \
                 @level2type=N'INDEX', @level2name=N'ix_id'"
            ),
            "{sql}"
        );

        let mut modified = schema.clone();
        modified.indexes[0].comment = "new lookup".to_string();
        let sql = table_schema_sql("dbo", "t", Some(&schema), &modified);
        assert!(sql.contains("DROP INDEX [ix_id] ON [dbo].[t]"), "{sql}");
        assert!(sql.contains("@value=N'new lookup'"), "{sql}");
    }

    #[test]
    fn emits_table_and_column_comments_as_extended_properties() {
        let mut schema = base_schema();
        schema.options.comment = "people".to_string();
        schema.columns[0].comment = "identifier".to_string();

        let sql = table_schema_sql("dbo", "t", None, &schema);
        assert!(
            sql.contains(
                "EXEC sp_addextendedproperty @name=N'MS_Description', @value=N'people', \
                 @level0type=N'SCHEMA', @level0name=N'dbo', @level1type=N'TABLE', @level1name=N't'"
            ),
            "{sql}"
        );
        assert!(
            sql.contains(
                "EXEC sp_addextendedproperty @name=N'MS_Description', @value=N'identifier', \
                 @level0type=N'SCHEMA', @level0name=N'dbo', @level1type=N'TABLE', @level1name=N't', \
                 @level2type=N'COLUMN', @level2name=N'id'"
            ),
            "{sql}"
        );

        let mut modified = schema.clone();
        modified.options.comment.clear();
        modified.columns[0].comment = "new identifier".to_string();
        let sql = table_schema_sql("dbo", "t", Some(&schema), &modified);
        assert!(sql.contains("EXEC sp_dropextendedproperty"), "{sql}");
        assert!(sql.contains("EXEC sp_updateextendedproperty"), "{sql}");
    }

    #[test]
    fn creates_and_reconciles_foreign_keys() {
        let mut schema = base_schema();
        schema.columns.push(ColumnDef {
            name: "parent_id".to_string(),
            data_type: "int".to_string(),
            nullable: false,
            ..ColumnDef::default()
        });
        schema.foreign_keys.push(ForeignKeyDef {
            name: "fk_parent".to_string(),
            columns: vec!["parent_id".to_string()],
            referenced_schema: "dbo".to_string(),
            referenced_table: "parent".to_string(),
            referenced_columns: vec!["id".to_string()],
            on_delete: "CASCADE".to_string(),
            on_update: String::new(),
        });

        // A new table carries the foreign key in its CREATE statement.
        let sql = table_schema_sql("dbo", "t", None, &schema);
        assert!(
            sql.contains(
                "CONSTRAINT [fk_parent] FOREIGN KEY ([parent_id]) REFERENCES [dbo].[parent] ([id]) ON DELETE CASCADE"
            ),
            "{sql}"
        );

        // A cross-schema reference is qualified; a changed reference is dropped before the columns
        // and re-added at the end.
        let mut modified = schema.clone();
        modified.foreign_keys[0].referenced_schema = "audit".to_string();
        let sql = table_schema_sql("dbo", "t", Some(&schema), &modified);
        assert!(
            sql.contains("ALTER TABLE [dbo].[t] DROP CONSTRAINT [fk_parent]"),
            "{sql}"
        );
        assert!(sql.contains("REFERENCES [audit].[parent]"), "{sql}");

        // Removing it drops the constraint.
        let sql = table_schema_sql("dbo", "t", Some(&schema), &base_schema());
        assert!(
            sql.contains("ALTER TABLE [dbo].[t] DROP CONSTRAINT [fk_parent]"),
            "{sql}"
        );
    }

    #[test]
    fn drops_removed_unique_keys_and_checks() {
        let mut original = base_schema();
        original.unique_keys.push(UniqueKeyDef {
            name: "uq_code".to_string(),
            columns: vec!["id".to_string()],
        });
        original.checks.push(CheckDef {
            name: "ck_positive".to_string(),
            expression: "[id]>(0)".to_string(),
        });

        let sql = table_schema_sql("dbo", "t", Some(&original), &base_schema());
        assert!(sql.contains("DROP CONSTRAINT [uq_code]"), "{sql}");
        assert!(sql.contains("DROP CONSTRAINT [ck_positive]"), "{sql}");
    }

    #[test]
    fn strips_the_catalog_parentheses_around_a_check_definition() {
        assert_eq!(strip_outer_parens("([id]>(0))"), "[id]>(0)");
        // Only the outermost pair goes, so a compound expression keeps its own grouping.
        assert_eq!(
            strip_outer_parens("(([a]>(0)) OR ([b]<(1)))"),
            "([a]>(0)) OR ([b]<(1))"
        );
        assert_eq!(strip_outer_parens("[id]>(0)"), "[id]>(0)");
    }

    #[test]
    fn renders_sparse_and_collated_columns() {
        let column = ColumnDef {
            name: "code".to_string(),
            data_type: "varchar".to_string(),
            length: "10".to_string(),
            nullable: true,
            collation: "Latin1_General_CI_AS".to_string(),
            sparse: true,
            ..ColumnDef::default()
        };
        assert_eq!(
            column_definition(&column),
            "[code] varchar(10) COLLATE Latin1_General_CI_AS SPARSE"
        );
    }

    #[test]
    fn renders_computed_columns() {
        let column = ColumnDef {
            name: "total".to_string(),
            generated: "[qty] * [price]".to_string(),
            ..ColumnDef::default()
        };
        assert_eq!(column_definition(&column), "[total] AS ([qty] * [price])");
    }

    #[test]
    fn renders_index_columns_with_their_own_sort_order() {
        let index = IndexDef {
            name: "ix_code".to_string(),
            columns: vec![
                IndexColumnDef {
                    name: "code".to_string(),
                    descending: true,
                    prefix_length: String::new(),
                },
                IndexColumnDef::new("id"),
            ],
            unique: true,
            primary: false,
            index_type: "NONCLUSTERED".to_string(),
            comment: String::new(),
        };
        assert_eq!(
            create_index_statements("dbo.[t]", &[index]),
            vec![
                "CREATE UNIQUE NONCLUSTERED INDEX [ix_code] ON dbo.[t] ([code] DESC, [id])"
                    .to_string()
            ]
        );
    }

    #[test]
    fn renders_a_clustered_index_as_the_engine_writes_it() {
        let index = IndexDef {
            name: "ix_code".to_string(),
            columns: vec![IndexColumnDef::new("id")],
            unique: false,
            primary: false,
            index_type: "CLUSTERED".to_string(),
            comment: String::new(),
        };
        assert_eq!(
            create_index_statements("dbo.[t]", &[index]),
            vec!["CREATE CLUSTERED INDEX [ix_code] ON dbo.[t] ([id])".to_string()]
        );
    }
}
