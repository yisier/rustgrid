use std::collections::HashMap;

use async_trait::async_trait;
use rustgrid_core::{
    BackupObjectKind, CellValue, ColumnDef, ColumnInfo, Connection, DatabaseInfo, DatabaseOptions,
    DriverId, Error, FilterCondition, FilterConjunction, FilterNode, FilterOperator, ForeignKeyDef,
    IndexDef, ObjectDump, ObjectKind, PageRequest, QueryResult, Result, RowInsert, RowUpdate,
    TableInfo, TableOptions, TablePage, TableSchema, TableStatus, TriggerDef, ViewDetails,
    ViewEdit, ViewInfo,
};
use sqlx::error::DatabaseError;
use sqlx::sqlite::{SqliteColumn, SqliteRow};
use sqlx::{AssertSqlSafe, Column, Executor, Row, SqlSafeStr, SqlitePool, Statement, ValueRef};

/// Rows fetched into one `INSERT` batch while restoring a backup. Kept moderate so a statement
/// stays well under SQLite's limits even for wide rows.
const RESTORE_INSERT_BATCH_ROWS: usize = 200;
/// A soft byte cap for the same batch.
const RESTORE_INSERT_BATCH_BYTES: usize = 1 << 20;

/// A SQLite connection. SQLite is a single-file engine: there is no server, schema selection or
/// session `USE`, so the `database` arguments of the trait are ignored (`main` is always the
/// attached database the file opened).
pub struct SqliteConnection {
    pub(crate) pool: SqlitePool,
}

impl SqliteConnection {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn scalar_u64(&self, sql: String, binds: &[String]) -> Result<u64> {
        let mut query = sqlx::query(AssertSqlSafe(sql));
        for bind in binds {
            query = query.bind(bind.clone());
        }
        let row = query.fetch_one(&self.pool).await.map_err(map_query_error)?;
        if let Ok(value) = row.try_get::<i64, _>(0) {
            return Ok(value.max(0) as u64);
        }
        Ok(0)
    }

    /// The `CREATE` statement of one object, as SQLite stores it in `sqlite_master`.
    async fn object_ddl(&self, kind: &str, name: &str) -> Result<String> {
        let row = sqlx::query("SELECT sql FROM sqlite_master WHERE type = ? AND name = ?")
            .bind(kind)
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_query_error)?;
        Ok(row
            .and_then(|row| row.try_get::<Option<String>, _>(0).ok().flatten())
            .unwrap_or_default())
    }

    async fn table_metadata(&self, name: &str) -> Result<ObjectDump> {
        let ddl = self.object_ddl("table", name).await?;
        let fields: Vec<String> = self
            .columns("main", name)
            .await?
            .into_iter()
            .map(|column| column.name)
            .collect();

        let trigger_rows = sqlx::query(
            "SELECT sql FROM sqlite_master \
             WHERE type = 'trigger' AND tbl_name = ? AND sql IS NOT NULL ORDER BY name",
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;
        let trigger_ddl = trigger_rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>(0).ok())
            .collect();

        Ok(ObjectDump {
            name: name.to_string(),
            kind: BackupObjectKind::Table,
            ddl,
            fields,
            trigger_ddl,
            rows: Vec::new(),
        })
    }

    /// The columns of one index, in index order.
    async fn index_columns(&self, index: &str) -> Result<Vec<String>> {
        let sql = format!("PRAGMA index_info({})", quote_identifier(index));
        let rows = sqlx::query(AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await
            .map_err(map_query_error)?;
        Ok(rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>("name").ok())
            .collect())
    }
}

#[async_trait]
impl Connection for SqliteConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new("sqlite")
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>> {
        // `database_list` reports the main file (and any attached databases) as `main`.
        let rows = sqlx::query("PRAGMA database_list")
            .fetch_all(&self.pool)
            .await
            .map_err(map_query_error)?;
        let mut databases = Vec::new();
        for row in &rows {
            let name: String = row.try_get("name").map_err(map_query_error)?;
            if name == "temp" {
                continue;
            }
            databases.push(DatabaseInfo { name });
        }
        if databases.is_empty() {
            databases.push(DatabaseInfo {
                name: "main".to_string(),
            });
        }
        Ok(databases)
    }

    async fn list_tables(&self, _database: &str) -> Result<Vec<TableInfo>> {
        let rows = sqlx::query(
            "SELECT name, type FROM sqlite_master \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;

        let mut tables = Vec::with_capacity(rows.len());
        for row in &rows {
            let name: String = row.try_get("name").map_err(map_query_error)?;
            let kind: String = row.try_get("type").map_err(map_query_error)?;
            tables.push(TableInfo {
                name,
                kind: if kind == "view" {
                    ObjectKind::View
                } else {
                    ObjectKind::Table
                },
                // SQLite has no cheap "is updatable" flag; the designer reports it as false.
                updatable: false,
            });
        }
        Ok(tables)
    }

    async fn columns(&self, _database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        let sql = format!("PRAGMA table_info({})", quote_identifier(table));
        let rows = sqlx::query(AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await
            .map_err(map_query_error)?;

        let mut columns = Vec::with_capacity(rows.len());
        for row in &rows {
            let name: String = row.try_get("name").map_err(map_query_error)?;
            let data_type: String = row
                .try_get::<Option<String>, _>("type")
                .map_err(map_query_error)?
                .unwrap_or_default();
            let not_null: i64 = row.try_get("notnull").unwrap_or(0);
            let primary_key: i64 = row.try_get("pk").unwrap_or(0);
            columns.push(ColumnInfo {
                name,
                data_type,
                // SQLite reports `notnull = 0` even for a primary-key column, so treat key columns
                // as not-null explicitly.
                nullable: not_null == 0 && primary_key == 0,
                primary_key: primary_key > 0,
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
        let qualified = quote_identifier(table);
        let (where_clause, binds) = filter_clause(&page.filter);
        let order = order_clause(&page);

        let total_rows = self
            .scalar_u64(
                format!("SELECT COUNT(*) FROM {qualified}{where_clause}"),
                &binds,
            )
            .await
            .ok();

        let sql = format!("SELECT * FROM {qualified}{where_clause}{order} LIMIT ? OFFSET ?");
        let mut query = sqlx::query(AssertSqlSafe(sql));
        for bind in &binds {
            query = query.bind(bind.clone());
        }
        query = query.bind(page.page_size as i64).bind(page.offset() as i64);
        let rows = query.fetch_all(&self.pool).await.map_err(map_query_error)?;

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

    async fn update_rows(&self, _database: &str, table: &str, updates: &[RowUpdate]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let qualified = quote_identifier(table);
        let mut transaction = self.pool.begin().await.map_err(map_query_error)?;
        for update in updates {
            if update.keys.is_empty() {
                return Err(Error::Query(
                    "refusing to update rows without key columns".to_string(),
                ));
            }
            let sets = update
                .set
                .iter()
                .map(|(column, _)| format!("{} = ?", quote_identifier(column)))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "UPDATE {qualified} SET {sets} WHERE {}",
                key_clause(&update.keys)
            );
            let mut query = sqlx::query(AssertSqlSafe(sql));
            for (_, value) in &update.set {
                query = query.bind(value.clone());
            }
            for (_, value) in &update.keys {
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

    async fn insert_rows(&self, _database: &str, table: &str, rows: &[RowInsert]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let qualified = quote_identifier(table);
        let mut transaction = self.pool.begin().await.map_err(map_query_error)?;
        for insert in rows {
            if insert.values.is_empty() {
                let sql = format!("INSERT INTO {qualified} DEFAULT VALUES");
                sqlx::query(AssertSqlSafe(sql))
                    .execute(&mut *transaction)
                    .await
                    .map_err(map_query_error)?;
                continue;
            }
            let columns = insert
                .values
                .iter()
                .map(|(column, _)| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ");
            let placeholders = vec!["?"; insert.values.len()].join(", ");
            let sql = format!("INSERT INTO {qualified} ({columns}) VALUES ({placeholders})");
            let mut query = sqlx::query(AssertSqlSafe(sql));
            for (_, value) in &insert.values {
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
        _database: &str,
        table: &str,
        keys: &[Vec<(String, Option<String>)>],
    ) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let qualified = quote_identifier(table);
        let mut transaction = self.pool.begin().await.map_err(map_query_error)?;
        for key in keys {
            if key.is_empty() {
                continue;
            }
            let sql = format!("DELETE FROM {qualified} WHERE {}", key_clause(key));
            let mut query = sqlx::query(AssertSqlSafe(sql));
            for (_, value) in key {
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

    async fn execute_query(&self, _database: Option<&str>, sql: &str) -> Result<QueryResult> {
        // One pinned connection for the whole script: session state (temp tables, pragmas) must not
        // leak across pooled connections between statements.
        let mut connection = self.pool.acquire().await.map_err(map_query_error)?;
        let statements = split_statements(sql);
        if statements.is_empty() {
            return Ok(empty_result(sql.trim().to_string()));
        }

        let mut result_set: Option<QueryResult> = None;
        let mut rows_affected = 0u64;
        let mut last_insert_id = None;
        let mut last_statement = sql.trim().to_string();
        for statement in &statements {
            last_statement = statement.clone();
            let result = execute_one(&mut connection, statement).await?;
            if result.has_result_set && result_set.is_none() {
                result_set = Some(result);
            } else {
                rows_affected += result.rows_affected;
                if result.last_insert_id.is_some() {
                    last_insert_id = result.last_insert_id;
                }
            }
        }

        Ok(result_set.unwrap_or(QueryResult {
            statement: last_statement,
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected,
            has_result_set: false,
            last_insert_id,
        }))
    }

    async fn execute_query_many(
        &self,
        _database: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        let mut connection = self.pool.acquire().await.map_err(map_query_error)?;
        let statements = split_statements(sql);
        if statements.is_empty() {
            return Ok(vec![empty_result(sql.trim().to_string())]);
        }
        let mut results = Vec::with_capacity(statements.len());
        for statement in &statements {
            results.push(execute_one(&mut connection, statement).await?);
        }
        Ok(results)
    }

    async fn create_database(&self, _name: &str, _options: &DatabaseOptions) -> Result<()> {
        Err(Error::Query(
            "SQLite does not support creating databases; each database is a file".to_string(),
        ))
    }

    fn create_database_sql(&self, _name: &str, _options: &DatabaseOptions) -> String {
        String::new()
    }

    async fn drop_database(&self, _name: &str) -> Result<()> {
        Err(Error::Query(
            "SQLite does not support dropping databases".to_string(),
        ))
    }

    async fn drop_table(&self, _database: &str, table: &str) -> Result<()> {
        let sql = format!("DROP TABLE {}", quote_identifier(table));
        self.run_sql(&sql).await
    }

    async fn empty_table(&self, _database: &str, table: &str) -> Result<()> {
        let sql = format!("DELETE FROM {}", quote_identifier(table));
        self.run_sql(&sql).await
    }

    async fn truncate_table(&self, _database: &str, table: &str) -> Result<()> {
        // SQLite has no TRUNCATE. `DELETE` empties the table; reset the auto-increment counter the
        // way MySQL's TRUNCATE does, when this database has a `sqlite_sequence` table at all.
        self.run_sql(&format!("DELETE FROM {}", quote_identifier(table)))
            .await?;
        let has_sequence = sqlx::query(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'sqlite_sequence'",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(map_query_error)?
        .is_some();
        if has_sequence {
            sqlx::query("DELETE FROM sqlite_sequence WHERE name = ?")
                .bind(table)
                .execute(&self.pool)
                .await
                .map_err(map_query_error)?;
        }
        Ok(())
    }

    async fn rename_table(&self, _database: &str, table: &str, new_name: &str) -> Result<()> {
        let sql = format!(
            "ALTER TABLE {} RENAME TO {}",
            quote_identifier(table),
            quote_identifier(new_name)
        );
        self.run_sql(&sql).await
    }

    async fn database_options(&self, _name: &str) -> Result<DatabaseOptions> {
        Ok(DatabaseOptions {
            charset: "UTF-8".to_string(),
            collation: "BINARY".to_string(),
            ..Default::default()
        })
    }

    async fn server_version(&self) -> Result<String> {
        let row = sqlx::query("SELECT sqlite_version()")
            .fetch_one(&self.pool)
            .await
            .map_err(map_query_error)?;
        row.try_get::<String, _>(0).map_err(map_query_error)
    }

    async fn session_count(&self) -> Result<u64> {
        Ok(0)
    }

    async fn table_status(&self, database: &str, table: &str) -> Result<TableStatus> {
        let rows = self
            .scalar_u64(
                format!("SELECT COUNT(*) FROM {}", quote_identifier(table)),
                &[],
            )
            .await
            .ok();
        let _ = database;
        Ok(TableStatus {
            engine: Some("SQLite".to_string()),
            rows,
            ..Default::default()
        })
    }

    async fn table_statuses(&self, database: &str) -> Result<Vec<(String, TableStatus)>> {
        let mut statuses = Vec::new();
        for table in self.list_tables(database).await? {
            if table.kind != ObjectKind::Table {
                continue;
            }
            let status = self.table_status(database, &table.name).await?;
            statuses.push((table.name, status));
        }
        Ok(statuses)
    }

    async fn character_sets(&self) -> Result<Vec<String>> {
        Ok(vec!["UTF-8".to_string()])
    }

    async fn collations(&self) -> Result<Vec<String>> {
        let rows = sqlx::query("PRAGMA collation_list")
            .fetch_all(&self.pool)
            .await
            .map_err(map_query_error)?;
        let mut collations: Vec<String> = rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>("name").ok())
            .collect();
        if collations.is_empty() {
            collations = vec![
                "BINARY".to_string(),
                "NOCASE".to_string(),
                "RTRIM".to_string(),
            ];
        }
        Ok(collations)
    }

    async fn alter_database_options(
        &self,
        _name: &str,
        _original: &DatabaseOptions,
        _modified: &DatabaseOptions,
    ) -> Result<()> {
        Ok(())
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
        // SQLite has no fixed column types: a column's declared type only selects one of these
        // five type affinities, which is all SQLite actually distinguishes. Anything else
        // (`VARCHAR(50)`, `BIGINT`, `DATETIME`, ...) is just a declared name that SQLite maps onto
        // one of these by its affinity rules, so the designer offers the real set only.
        vec!["INTEGER", "TEXT", "REAL", "BLOB", "NUMERIC"]
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
        kind: BackupObjectKind,
        name: &str,
    ) -> Result<ObjectDump> {
        match kind {
            BackupObjectKind::Table => self.table_metadata(name).await,
            BackupObjectKind::View => Ok(ObjectDump {
                name: name.to_string(),
                kind,
                ddl: self.object_ddl("view", name).await?,
                fields: Vec::new(),
                trigger_ddl: Vec::new(),
                rows: Vec::new(),
            }),
            // SQLite has no stored routines or scheduled events.
            BackupObjectKind::Function | BackupObjectKind::Event => Ok(ObjectDump {
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
        _database: &str,
        table: &str,
        on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<u64> {
        let sql = format!("SELECT * FROM {}", quote_identifier(table));
        let mut stream = sqlx::query(AssertSqlSafe(sql)).fetch(&self.pool);
        let mut count = 0u64;
        let mut width: Option<usize> = None;
        loop {
            let Some(item) = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await else {
                break;
            };
            let row = item.map_err(map_query_error)?;
            let width = *width.get_or_insert_with(|| row.columns().len());
            let mut tuple = String::from("(");
            for index in 0..width {
                if index > 0 {
                    tuple.push_str(", ");
                }
                tuple.push_str(&render_literal(&decode_cell(&row, index)));
            }
            tuple.push(')');
            on_row(&tuple)?;
            count += 1;
        }
        Ok(count)
    }

    async fn restore_object(&self, _database: &str, object: &ObjectDump) -> Result<()> {
        let mut connection = self.pool.acquire().await.map_err(map_query_error)?;
        restore_object_on(&mut connection, object).await
    }

    fn storage_engines(&self) -> Vec<&'static str> {
        Vec::new()
    }

    async fn table_schema(&self, _database: &str, table: &str) -> Result<TableSchema> {
        let create_sql = self.object_ddl("table", table).await?;
        let has_autoincrement = create_sql.to_ascii_uppercase().contains("AUTOINCREMENT");

        let column_rows = sqlx::query(AssertSqlSafe(format!(
            "PRAGMA table_info({})",
            quote_identifier(table)
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;

        let mut columns = Vec::with_capacity(column_rows.len());
        for row in &column_rows {
            let name: String = row.try_get("name").map_err(map_query_error)?;
            let declared = optional_text(row, "type").unwrap_or_default();
            let not_null: i64 = row.try_get("notnull").unwrap_or(0);
            let default = optional_text(row, "dflt_value").unwrap_or_default();
            let primary_key: i64 = row.try_get("pk").unwrap_or(0);
            let (data_type, length, decimals) = parse_declared_type(&declared);
            let auto_increment =
                has_autoincrement && primary_key > 0 && data_type.eq_ignore_ascii_case("integer");
            columns.push(ColumnDef {
                name,
                data_type,
                length,
                decimals,
                nullable: not_null == 0 && primary_key == 0,
                default,
                primary_key: primary_key > 0,
                auto_increment,
                unsigned: false,
                zerofill: false,
                comment: String::new(),
                charset: String::new(),
                collation: String::new(),
                extra: String::new(),
                generated: String::new(),
                stored: false,
            });
        }

        let mut indexes = Vec::new();
        let index_rows = sqlx::query(AssertSqlSafe(format!(
            "PRAGMA index_list({})",
            quote_identifier(table)
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;
        for row in &index_rows {
            // Only user-created indexes (`origin = 'c'`) round-trip: primary-key and UNIQUE
            // constraint indexes are implicit and carry reserved `sqlite_autoindex_*` names.
            let origin: String = row.try_get("origin").unwrap_or_default();
            if origin != "c" {
                continue;
            }
            let name: String = row.try_get("name").map_err(map_query_error)?;
            let unique: i64 = row.try_get("unique").unwrap_or(0);
            let columns = self.index_columns(&name).await?;
            indexes.push(IndexDef {
                name,
                columns,
                unique: unique == 1,
                primary: false,
                index_type: "BTREE".to_string(),
                comment: String::new(),
            });
        }

        let fk_rows = sqlx::query(AssertSqlSafe(format!(
            "PRAGMA foreign_key_list({})",
            quote_identifier(table)
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;
        let mut fk_index: HashMap<i64, usize> = HashMap::new();
        let mut foreign_keys: Vec<ForeignKeyDef> = Vec::new();
        for row in &fk_rows {
            let id: i64 = row.try_get("id").unwrap_or(0);
            let referenced_table: String = row.try_get("table").map_err(map_query_error)?;
            let from: String = row.try_get("from").map_err(map_query_error)?;
            let to = optional_text(row, "to");
            let on_update = normalize_rule(&optional_text(row, "on_update").unwrap_or_default());
            let on_delete = normalize_rule(&optional_text(row, "on_delete").unwrap_or_default());

            let position = fk_index.entry(id).or_insert_with(|| {
                foreign_keys.push(ForeignKeyDef {
                    name: format!("fk_{id}"),
                    columns: Vec::new(),
                    referenced_table: referenced_table.clone(),
                    referenced_columns: Vec::new(),
                    on_delete: on_delete.clone(),
                    on_update: on_update.clone(),
                });
                foreign_keys.len() - 1
            });
            let foreign_key = &mut foreign_keys[*position];
            foreign_key.columns.push(from);
            if let Some(to) = to {
                foreign_key.referenced_columns.push(to);
            }
        }

        let trigger_rows = sqlx::query(
            "SELECT name, sql FROM sqlite_master \
             WHERE type = 'trigger' AND tbl_name = ? ORDER BY name",
        )
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;
        let mut triggers = Vec::new();
        for row in &trigger_rows {
            let name: String = row.try_get("name").map_err(map_query_error)?;
            let statement = optional_text(row, "sql").unwrap_or_default();
            triggers.push(TriggerDef {
                name,
                timing: String::new(),
                event: String::new(),
                statement,
            });
        }

        Ok(TableSchema {
            columns,
            indexes,
            foreign_keys,
            triggers,
            options: TableOptions {
                engine: "SQLite".to_string(),
                charset: "UTF-8".to_string(),
                collation: String::new(),
                comment: String::new(),
                auto_increment: String::new(),
            },
        })
    }

    fn table_schema_sql(
        &self,
        _database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> String {
        match original {
            None => {
                let mut statements = vec![create_table_statement(table, modified)];
                statements.extend(create_index_statements(table, &modified.indexes));
                statements.join(";\n")
            }
            Some(original) => {
                let mut statements: Vec<String> = Vec::new();
                let original_columns: HashMap<&str, &ColumnDef> = original
                    .columns
                    .iter()
                    .map(|column| (column.name.as_str(), column))
                    .collect();
                let modified_columns: HashMap<&str, &ColumnDef> = modified
                    .columns
                    .iter()
                    .map(|column| (column.name.as_str(), column))
                    .collect();

                for column in &modified.columns {
                    if !original_columns.contains_key(column.name.as_str()) {
                        statements.push(format!(
                            "ALTER TABLE {} ADD COLUMN {}",
                            quote_identifier(table),
                            column_definition(column, false)
                        ));
                    }
                }
                for column in &original.columns {
                    if !modified_columns.contains_key(column.name.as_str()) {
                        statements.push(format!(
                            "ALTER TABLE {} DROP COLUMN {}",
                            quote_identifier(table),
                            quote_identifier(&column.name)
                        ));
                    }
                }
                for column in &modified.columns {
                    if let Some(previous) = original_columns.get(column.name.as_str())
                        && (previous.data_type != column.data_type
                            || previous.nullable != column.nullable
                            || previous.default != column.default
                            || previous.primary_key != column.primary_key
                            || previous.auto_increment != column.auto_increment)
                    {
                        // SQLite only supports adding, dropping and renaming columns in place; a
                        // definition change needs a table rebuild, which the designer does not
                        // generate yet.
                        statements.push(format!(
                            "-- SQLite cannot alter column {} in place; rebuild the table to change it",
                            quote_identifier(&column.name)
                        ));
                    }
                }

                let original_indexes: HashMap<&str, &IndexDef> = original
                    .indexes
                    .iter()
                    .map(|index| (index.name.as_str(), index))
                    .collect();
                let modified_indexes: HashMap<&str, &IndexDef> = modified
                    .indexes
                    .iter()
                    .map(|index| (index.name.as_str(), index))
                    .collect();
                for index in &modified.indexes {
                    if !index.primary && !original_indexes.contains_key(index.name.as_str()) {
                        statements
                            .extend(create_index_statements(table, std::slice::from_ref(index)));
                    }
                }
                for index in &original.indexes {
                    if !index.primary && !modified_indexes.contains_key(index.name.as_str()) {
                        statements.push(format!("DROP INDEX {}", quote_identifier(&index.name)));
                    }
                }

                statements.join(";\n")
            }
        }
    }

    async fn view_details(&self, _database: &str, name: &str) -> Result<ViewDetails> {
        let row = sqlx::query("SELECT sql FROM sqlite_master WHERE type = 'view' AND name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_query_error)?;
        let definition = row
            .and_then(|row| row.try_get::<Option<String>, _>(0).ok().flatten())
            .ok_or_else(|| Error::Query(format!("view {name} not found")))?;
        Ok(ViewDetails {
            info: ViewInfo {
                name: name.to_string(),
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
            statements.push(format!(
                "DROP VIEW IF EXISTS {}",
                quote_identifier(original)
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
        let sql = self.view_sql(database, original, edit);
        if sql.trim().is_empty() {
            return Ok(());
        }
        self.execute_query(Some(database), &sql).await.map(|_| ())
    }

    async fn drop_view(&self, _database: &str, name: &str) -> Result<()> {
        let sql = format!("DROP VIEW {}", quote_identifier(name));
        self.run_sql(&sql).await
    }

    async fn close(&self) -> Result<()> {
        self.pool.close().await;
        Ok(())
    }
}

impl SqliteConnection {
    /// Run one statement through the text path (no binds) on a fresh pooled connection.
    async fn run_sql(&self, sql: &str) -> Result<()> {
        sqlx::query(AssertSqlSafe(sql.to_string()))
            .execute(&self.pool)
            .await
            .map_err(map_query_error)?;
        Ok(())
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

/// The `WHERE` fragment that identifies one row from its key columns. A `None` value means SQL
/// `NULL` and must be matched with `IS NULL`, never bound as an empty string.
fn key_clause(keys: &[(String, Option<String>)]) -> String {
    keys.iter()
        .map(|(column, value)| {
            let column = quote_identifier(column);
            if value.is_none() {
                format!("{column} IS NULL")
            } else {
                format!("{column} = ?")
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Run one statement (already split from a script) on a pinned connection and decode its single
/// result.
async fn execute_one(connection: &mut sqlx::SqliteConnection, sql: &str) -> Result<QueryResult> {
    if returns_result_set(sql) {
        let rows = sqlx::query(AssertSqlSafe(sql.to_string()))
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
        let result = sqlx::query(AssertSqlSafe(sql.to_string()))
            .execute(&mut *connection)
            .await
            .map_err(map_query_error)?;
        Ok(QueryResult {
            statement: sql.to_string(),
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: result.rows_affected(),
            has_result_set: false,
            last_insert_id: Some(result.last_insert_rowid() as u64).filter(|id| *id != 0),
        })
    }
}

fn columns_from_row(row: &SqliteRow) -> Vec<ColumnInfo> {
    row.columns().iter().map(column_info).collect()
}

fn column_info(column: &SqliteColumn) -> ColumnInfo {
    ColumnInfo {
        name: column.name().to_string(),
        data_type: column.type_info().to_string(),
        nullable: true,
        primary_key: false,
        comment: String::new(),
    }
}

async fn describe_columns(
    connection: &mut sqlx::SqliteConnection,
    sql: &str,
) -> Result<Vec<ColumnInfo>> {
    let statement = connection
        .prepare(AssertSqlSafe(sql.to_string()).into_sql_str())
        .await
        .map_err(map_query_error)?;
    Ok(statement.columns().iter().map(column_info).collect())
}

/// Whether a statement is expected to produce a result set. SQLite has no way to know this before
/// preparing it, so the leading keyword decides which path is used. A statement with a
/// `RETURNING` clause is the one exception this classifier misses.
fn returns_result_set(sql: &str) -> bool {
    let mut rest = sql;
    loop {
        rest = rest.trim_start();
        if let Some(stripped) = rest.strip_prefix("--") {
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
        "SELECT" | "WITH" | "VALUES" | "EXPLAIN" | "PRAGMA" | "TABLE"
    )
}

/// Split a SQL script into statements on top-level semicolons, respecting string literals, quoted
/// identifiers and comments. Comment-only and empty fragments are dropped.
///
/// SQLite is executed client-side one statement at a time, so scripts must be split here. A
/// `CREATE TRIGGER ... BEGIN ...; ... END` body contains semicolons the splitter cannot see; run
/// such a trigger through its own statement (e.g. the backup restore path), which passes it whole.
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
                    if bytes[i] == quote {
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
            b'[' => {
                // SQLite also accepts `[identifier]` quoting.
                i += 1;
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
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

/// Read a column that may be `NULL` or may hold a value of any SQLite storage class as text.
fn optional_text(row: &SqliteRow, name: &str) -> Option<String> {
    if let Ok(value) = row.try_get::<Option<String>, _>(name) {
        return value;
    }
    if let Ok(value) = row.try_get::<Option<i64>, _>(name) {
        return value.map(|value| value.to_string());
    }
    if let Ok(value) = row.try_get::<Option<f64>, _>(name) {
        return value.map(|value| value.to_string());
    }
    None
}

fn decode_cell(row: &SqliteRow, index: usize) -> CellValue {
    let is_null = row
        .try_get_raw(index)
        .map(|value| value.is_null())
        .unwrap_or(true);
    if is_null {
        return CellValue::Null;
    }

    // SQLite is dynamically typed, so the storage class of the value (not the column's declared
    // type) decides how it decodes. Compatibility checks make each `try_get` fail fast when the
    // value is a different class.
    if let Ok(value) = row.try_get::<i64, _>(index) {
        return CellValue::Int(value);
    }
    if let Ok(value) = row.try_get::<f64, _>(index) {
        return CellValue::Float(value);
    }
    if let Ok(value) = row.try_get::<String, _>(index) {
        return CellValue::Text(value);
    }
    if let Ok(bytes) = row.try_get::<Vec<u8>, _>(index) {
        return match String::from_utf8(bytes) {
            Ok(text) => CellValue::Text(text),
            Err(error) => CellValue::Bytes(error.into_bytes()),
        };
    }
    CellValue::Null
}

fn quote_identifier(identifier: &str) -> String {
    let mut out = String::with_capacity(identifier.len() + 2);
    out.push('"');
    for character in identifier.chars() {
        if character == '"' {
            out.push('"');
        }
        out.push(character);
    }
    out.push('"');
    out
}

fn quote_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for character in value.chars() {
        if character == '\'' {
            out.push('\'');
        }
        out.push(character);
    }
    out.push('\'');
    out
}

fn render_literal(value: &CellValue) -> String {
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
        CellValue::Text(text) => quote_literal(text),
        CellValue::Bytes(bytes) => {
            let mut hex = String::with_capacity(bytes.len() * 2 + 3);
            hex.push_str("X'");
            for byte in bytes {
                hex.push_str(&format!("{byte:02X}"));
            }
            hex.push('\'');
            hex
        }
    }
}

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

fn filter_clause(filter: &[FilterNode]) -> (String, Vec<String>) {
    let mut binds: Vec<String> = Vec::new();
    match filter_fragment(filter, &mut binds) {
        Some(expression) => (format!(" WHERE {expression}"), binds),
        None => (String::new(), Vec::new()),
    }
}

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

fn parse_declared_type(declared: &str) -> (String, String, String) {
    let declared = declared.trim();
    if declared.is_empty() {
        return (String::new(), String::new(), String::new());
    }
    match declared.split_once('(') {
        Some((base, rest)) => {
            let inner = rest.trim_end_matches(')');
            let mut parts = inner.splitn(2, ',');
            let length = parts.next().unwrap_or("").trim().to_string();
            let decimals = parts.next().unwrap_or("").trim().to_string();
            (base.trim().to_string(), length, decimals)
        }
        None => (declared.to_string(), String::new(), String::new()),
    }
}

fn normalize_rule(rule: &str) -> String {
    let trimmed = rule.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("NO ACTION") {
        String::new()
    } else {
        trimmed.to_ascii_uppercase()
    }
}

fn column_definition(column: &ColumnDef, composite_primary_key: bool) -> String {
    let mut out = quote_identifier(&column.name);
    let data_type = if column.data_type.trim().is_empty() {
        "BLOB"
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

    if !column.generated.is_empty() {
        out.push_str(" GENERATED ALWAYS AS (");
        out.push_str(&column.generated);
        out.push(')');
        out.push_str(if column.stored { " STORED" } else { " VIRTUAL" });
        return out;
    }

    if column.primary_key && !composite_primary_key {
        out.push_str(" PRIMARY KEY");
        if column.auto_increment && data_type.eq_ignore_ascii_case("integer") {
            out.push_str(" AUTOINCREMENT");
        }
    }
    if !column.nullable && !column.primary_key {
        out.push_str(" NOT NULL");
    }
    if !column.default.is_empty() {
        out.push_str(" DEFAULT ");
        out.push_str(&column.default);
    }
    out
}

fn foreign_key_definition(foreign_key: &ForeignKeyDef) -> String {
    let columns = foreign_key
        .columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = format!(
        "FOREIGN KEY ({columns}) REFERENCES {}",
        quote_identifier(&foreign_key.referenced_table)
    );
    if !foreign_key.referenced_columns.is_empty() {
        let referenced = foreign_key
            .referenced_columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(" ({referenced})"));
    }
    if !foreign_key.on_delete.is_empty() {
        out.push_str(&format!(" ON DELETE {}", foreign_key.on_delete));
    }
    if !foreign_key.on_update.is_empty() {
        out.push_str(&format!(" ON UPDATE {}", foreign_key.on_update));
    }
    out
}

fn create_table_statement(table: &str, schema: &TableSchema) -> String {
    let primary_columns: Vec<&str> = schema
        .columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.as_str())
        .collect();
    let composite_primary_key = primary_columns.len() > 1;

    let mut definitions: Vec<String> = schema
        .columns
        .iter()
        .map(|column| column_definition(column, composite_primary_key))
        .collect();
    if composite_primary_key {
        let columns = primary_columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        definitions.push(format!("PRIMARY KEY ({columns})"));
    }
    for foreign_key in &schema.foreign_keys {
        definitions.push(foreign_key_definition(foreign_key));
    }

    format!(
        "CREATE TABLE {} (\n  {}\n)",
        quote_identifier(table),
        definitions.join(",\n  ")
    )
}

fn create_index_statements(table: &str, indexes: &[IndexDef]) -> Vec<String> {
    indexes
        .iter()
        .filter(|index| !index.primary && !index.columns.is_empty())
        .map(|index| {
            let unique = if index.unique { "UNIQUE " } else { "" };
            let columns = index
                .columns
                .iter()
                .map(|column| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "CREATE {unique}INDEX {} ON {} ({columns})",
                quote_identifier(&index.name),
                quote_identifier(table)
            )
        })
        .collect()
}

fn trim_statement(sql: &str) -> String {
    sql.trim().trim_end_matches(';').trim_end().to_string()
}

async fn exec_sql(connection: &mut sqlx::SqliteConnection, sql: &str) -> Result<()> {
    sqlx::query(AssertSqlSafe(sql.to_string()))
        .execute(&mut *connection)
        .await
        .map_err(map_query_error)?;
    Ok(())
}

async fn insert_batch(
    connection: &mut sqlx::SqliteConnection,
    table: &str,
    columns: &str,
    rows: &[&str],
) -> Result<()> {
    let sql = format!("INSERT INTO {table} ({columns}) VALUES {}", rows.join(", "));
    exec_sql(connection, &sql).await
}

async fn restore_object_on(
    connection: &mut sqlx::SqliteConnection,
    object: &ObjectDump,
) -> Result<()> {
    let table = quote_identifier(&object.name);

    if object.kind == BackupObjectKind::View {
        exec_sql(connection, &format!("DROP VIEW IF EXISTS {table}")).await?;
        let ddl = trim_statement(&object.ddl);
        if !ddl.is_empty() {
            exec_sql(connection, &ddl).await?;
        }
        return Ok(());
    }

    if object.kind != BackupObjectKind::Table {
        return Ok(());
    }

    // Foreign keys are disabled while the object is dropped and recreated, then always restored so
    // the pooled connection is not left with them off.
    exec_sql(connection, "PRAGMA foreign_keys=OFF").await?;
    let result = async {
        exec_sql(connection, &format!("DROP TABLE IF EXISTS {table}")).await?;
        let ddl = trim_statement(&object.ddl);
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
            let mut batch: Vec<&str> = Vec::new();
            let mut bytes = 0usize;
            for row in &object.rows {
                if !batch.is_empty()
                    && (batch.len() >= RESTORE_INSERT_BATCH_ROWS
                        || bytes + row.len() + 2 > RESTORE_INSERT_BATCH_BYTES)
                {
                    insert_batch(connection, &table, &columns, &batch).await?;
                    batch.clear();
                    bytes = 0;
                }
                bytes += row.len() + 2;
                batch.push(row);
            }
            if !batch.is_empty() {
                insert_batch(connection, &table, &columns, &batch).await?;
            }
        }
        for trigger in &object.trigger_ddl {
            let trigger = trim_statement(trigger);
            if !trigger.is_empty() {
                exec_sql(connection, &trigger).await?;
            }
        }
        Ok(())
    }
    .await;
    let _ = exec_sql(connection, "PRAGMA foreign_keys=ON").await;
    result
}

fn map_query_error(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(database_error) = &error
        && let Some(sqlite_error) = database_error.try_downcast_ref::<sqlx::sqlite::SqliteError>()
    {
        return Error::Query(format!(
            "{} - {}",
            sqlite_error.code().unwrap_or_default(),
            sqlite_error.message()
        ));
    }
    Error::Query(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        create_table_statement, filter_clause, order_clause, parse_declared_type, quote_identifier,
        returns_result_set, split_statements,
    };
    use rustgrid_core::{
        ColumnDef, FilterCondition, FilterConjunction, FilterGroup, FilterNode, FilterOperator,
        PageRequest, SortColumn, TableSchema,
    };

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

    #[test]
    fn quotes_identifiers_with_doubled_quotes() {
        assert_eq!(quote_identifier("user"), "\"user\"");
        assert_eq!(quote_identifier("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn parses_declared_types() {
        assert_eq!(
            parse_declared_type("varchar(50)"),
            ("varchar".into(), "50".into(), String::new())
        );
        assert_eq!(
            parse_declared_type("decimal(10, 2)"),
            ("decimal".into(), "10".into(), "2".into())
        );
        assert_eq!(
            parse_declared_type("INTEGER"),
            ("INTEGER".into(), String::new(), String::new())
        );
    }

    #[test]
    fn builds_order_by_from_sort_columns() {
        let page = PageRequest::new(0, 10).with_order_by(vec![
            SortColumn {
                column: "id".into(),
                descending: false,
            },
            SortColumn {
                column: "name".into(),
                descending: true,
            },
        ]);
        assert_eq!(order_clause(&page), " ORDER BY \"id\" ASC, \"name\" DESC");
    }

    #[test]
    fn builds_where_with_bound_values() {
        let filter = vec![FilterNode::Condition(condition(
            "id",
            FilterOperator::GreaterThan,
            "7",
        ))];
        let (clause, binds) = filter_clause(&filter);
        assert_eq!(clause, " WHERE \"id\" > ?");
        assert_eq!(binds, vec!["7".to_string()]);
    }

    #[test]
    fn builds_where_with_nested_groups() {
        let group = FilterGroup {
            conjunction: FilterConjunction::And,
            enabled: true,
            children: vec![
                FilterNode::Condition(condition("a", FilterOperator::Equal, "1")),
                FilterNode::Condition(FilterCondition {
                    conjunction: FilterConjunction::Or,
                    ..condition("b", FilterOperator::Equal, "2")
                }),
            ],
        };
        let filter = vec![
            FilterNode::Condition(condition("id", FilterOperator::GreaterThan, "0")),
            FilterNode::Group(group),
        ];
        let (clause, binds) = filter_clause(&filter);
        assert_eq!(clause, " WHERE \"id\" > ? AND (\"a\" = ? OR \"b\" = ?)");
        assert_eq!(binds, vec!["0", "1", "2"]);
    }

    #[test]
    fn creates_a_table_with_key_and_index() {
        let schema = TableSchema {
            columns: vec![
                ColumnDef {
                    name: "id".into(),
                    data_type: "integer".into(),
                    primary_key: true,
                    auto_increment: true,
                    nullable: false,
                    ..Default::default()
                },
                ColumnDef {
                    name: "name".into(),
                    data_type: "text".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let sql = create_table_statement("users", &schema);
        assert!(sql.contains("\"id\" integer PRIMARY KEY AUTOINCREMENT"));
        assert!(sql.contains("\"name\" text"));
    }

    #[test]
    fn detects_result_set_statements() {
        assert!(returns_result_set("SELECT 1"));
        assert!(returns_result_set(
            "-- comment\nWITH cte AS (SELECT 1) SELECT * FROM cte"
        ));
        assert!(returns_result_set("PRAGMA table_info(\"t\")"));
        assert!(!returns_result_set("INSERT INTO t VALUES (1)"));
        assert!(!returns_result_set("CREATE TABLE t (id INTEGER)"));
    }

    #[test]
    fn splits_statements_on_top_level_semicolons() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2"),
            vec!["SELECT 1", "SELECT 2"]
        );
        assert_eq!(split_statements("  SELECT 1 ; \n"), vec!["SELECT 1"]);
        assert_eq!(
            split_statements("SELECT ';'; SELECT \"a;b\""),
            vec!["SELECT ';'", "SELECT \"a;b\""]
        );
        assert_eq!(
            split_statements("SELECT 1 -- a; b\n; SELECT 2"),
            vec!["SELECT 1 -- a; b", "SELECT 2"]
        );
        assert_eq!(
            split_statements("SELECT 'it''s'; SELECT \"x;y\""),
            vec!["SELECT 'it''s'", "SELECT \"x;y\""]
        );
    }
}
