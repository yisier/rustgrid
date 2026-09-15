use async_trait::async_trait;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use navidog_core::{
    CellValue, ColumnDef, ColumnInfo, Connection, DatabaseInfo, DriverId, Error, FilterCondition,
    FilterConjunction, FilterOperator, ForeignKeyDef, IndexDef, ObjectKind, PageRequest,
    QueryResult, Result, RowUpdate, TableInfo, TableOptions, TablePage, TableSchema, TriggerDef,
};
use sqlx::mysql::{MySqlColumn, MySqlRow};
use sqlx::{
    AssertSqlSafe, Column, Executor, MySqlConnection, MySqlPool, Row, SqlSafeStr, Statement,
    ValueRef,
};

pub struct MysqlConnection {
    pool: MySqlPool,
}

impl MysqlConnection {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    async fn scalar_u64(&self, sql: String, binds: &[String]) -> Result<u64> {
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for bind in binds {
            query = query.bind(bind);
        }
        let row = query.fetch_one(&self.pool).await.map_err(map_query_error)?;

        if let Ok(value) = row.try_get::<i64, _>(0) {
            return Ok(value.max(0) as u64);
        }

        if let Ok(value) = row.try_get::<u64, _>(0) {
            return Ok(value);
        }

        Err(Error::Query("unexpected COUNT result".to_string()))
    }
}

#[async_trait]
impl Connection for MysqlConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new("mysql")
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>> {
        let rows = sqlx::query("SHOW DATABASES")
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
        let rows = sqlx::query(
            "SELECT table_name, table_type \
             FROM information_schema.tables \
             WHERE table_schema = ? \
             ORDER BY table_name",
        )
        .bind(database)
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;

        let mut tables = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            let table_type: String = row.try_get(1).map_err(map_query_error)?;
            let kind = if table_type.eq_ignore_ascii_case("VIEW") {
                ObjectKind::View
            } else {
                ObjectKind::Table
            };
            tables.push(TableInfo { name, kind });
        }
        Ok(tables)
    }

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        let rows = sqlx::query(
            "SELECT column_name, column_type, is_nullable, column_key \
             FROM information_schema.columns \
             WHERE table_schema = ? AND table_name = ? \
             ORDER BY ordinal_position",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;

        let mut columns = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            let data_type: String = row.try_get(1).map_err(map_query_error)?;
            let nullable: String = row.try_get(2).map_err(map_query_error)?;
            let key: String = row.try_get(3).map_err(map_query_error)?;
            columns.push(ColumnInfo {
                name,
                data_type,
                nullable: nullable.eq_ignore_ascii_case("YES"),
                primary_key: key.eq_ignore_ascii_case("PRI"),
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
        let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(table));

        let (where_clause, binds) = filter_clause(&page.filter);

        let total_rows = self
            .scalar_u64(
                format!("SELECT COUNT(*) FROM {qualified}{where_clause}"),
                &binds,
            )
            .await
            .ok();

        let page_sql = format!(
            "SELECT * FROM {qualified}{where_clause}{} LIMIT ? OFFSET ?",
            order_clause(&page)
        );
        let mut query = sqlx::query(sqlx::AssertSqlSafe(page_sql));
        for bind in &binds {
            query = query.bind(bind);
        }
        let rows = query
            .bind(page.page_size as i64)
            .bind(page.offset() as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(map_query_error)?;

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

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(table));
        let mut transaction = self.pool.begin().await.map_err(map_query_error)?;

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
                    .map(|(column, _)| format!("{} = ?", quote_identifier(column)))
                    .collect::<Vec<_>>()
                    .join(" AND ")
            };
            let sql = format!("UPDATE {qualified} SET {set_clause} WHERE {where_clause}");

            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
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

    async fn delete_rows(
        &self,
        database: &str,
        table: &str,
        keys: &[Vec<(String, String)>],
    ) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }

        let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(table));
        let mut transaction = self.pool.begin().await.map_err(map_query_error)?;

        for row_keys in keys {
            if row_keys.is_empty() {
                return Err(Error::Query("delete requires key columns".to_string()));
            }
            let where_clause = row_keys
                .iter()
                .map(|(column, _)| format!("{} = ?", quote_identifier(column)))
                .collect::<Vec<_>>()
                .join(" AND ");
            let sql = format!("DELETE FROM {qualified} WHERE {where_clause} LIMIT 1");

            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for (_, value) in row_keys {
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

    async fn execute_query(&self, database: Option<&str>, sql: &str) -> Result<QueryResult> {
        let mut connection = self.pool.acquire().await.map_err(map_query_error)?;

        if let Some(database) = database {
            let use_sql = format!("USE {}", quote_identifier(database));
            sqlx::raw_sql(sqlx::AssertSqlSafe(use_sql))
                .execute(&mut *connection)
                .await
                .map_err(map_query_error)?;
        }

        if returns_result_set(sql) {
            let rows = sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string()))
                .fetch_all(&mut *connection)
                .await
                .map_err(map_query_error)?;

            let columns = match rows.first() {
                Some(first) => columns_from_row(first),
                None => describe_columns(&mut connection, sql)
                    .await
                    .unwrap_or_default(),
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
                columns: Vec::new(),
                rows: Vec::new(),
                rows_affected: result.rows_affected(),
                has_result_set: false,
                last_insert_id: Some(result.last_insert_id()).filter(|id| *id != 0),
            })
        }
    }

    async fn create_database(&self, name: &str) -> Result<()> {
        let sql = format!("CREATE DATABASE {}", quote_identifier(name));
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&self.pool)
            .await
            .map_err(map_query_error)?;
        Ok(())
    }

    async fn drop_database(&self, name: &str) -> Result<()> {
        let sql = format!("DROP DATABASE {}", quote_identifier(name));
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&self.pool)
            .await
            .map_err(map_query_error)?;
        Ok(())
    }

    async fn database_defaults(&self, name: &str) -> Result<(String, String)> {
        let row = sqlx::query(
            "SELECT default_character_set_name, default_collation_name \
             FROM information_schema.schemata \
             WHERE schema_name = ?",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(map_query_error)?;

        let charset: String = row.try_get(0).map_err(map_query_error)?;
        let collation: String = row.try_get(1).map_err(map_query_error)?;
        Ok((charset, collation))
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

    async fn alter_database_defaults(
        &self,
        name: &str,
        charset: Option<&str>,
        collation: Option<&str>,
    ) -> Result<()> {
        let mut sql = format!("ALTER DATABASE {}", quote_identifier(name));
        if let Some(charset) = charset {
            sql.push_str(" CHARACTER SET ");
            sql.push_str(charset);
        }
        if let Some(collation) = collation {
            sql.push_str(" COLLATE ");
            sql.push_str(collation);
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
        charset: Option<&str>,
        collation: Option<&str>,
    ) -> String {
        let mut clauses = Vec::new();
        if let Some(charset) = charset {
            clauses.push(format!("CHARACTER SET {charset}"));
        }
        if let Some(collation) = collation {
            clauses.push(format!("COLLATE {collation}"));
        }

        let mut sql = format!("ALTER DATABASE {}", quote_identifier(name));
        if !clauses.is_empty() {
            sql.push(' ');
            sql.push_str(&clauses.join(" "));
        }
        sql.push(';');
        sql
    }

    fn column_types(&self) -> Vec<&'static str> {
        MYSQL_COLUMN_TYPES.to_vec()
    }

    async fn table_schema(&self, database: &str, table: &str) -> Result<TableSchema> {
        let column_rows = sqlx::query(
            "SELECT column_name, data_type, column_type, is_nullable, column_default, extra, \
                    column_key, column_comment, character_set_name, collation_name, \
                    character_maximum_length, numeric_precision, numeric_scale, \
                    generation_expression \
             FROM information_schema.columns \
             WHERE table_schema = ? AND table_name = ? \
             ORDER BY ordinal_position",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;

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
            let generated: Option<String> = row.try_get(13).unwrap_or(None);

            let numeric = data_type.eq_ignore_ascii_case("decimal")
                || data_type.eq_ignore_ascii_case("numeric");
            let length = if let Some(length) = char_length {
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

            columns.push(ColumnDef {
                name,
                data_type,
                length,
                decimals,
                nullable: nullable.eq_ignore_ascii_case("YES"),
                default: default.unwrap_or_default(),
                primary_key: key.eq_ignore_ascii_case("PRI"),
                auto_increment: extra_lower.contains("auto_increment"),
                unsigned: column_type_lower.contains("unsigned"),
                zerofill: column_type_lower.contains("zerofill"),
                comment,
                charset: charset.unwrap_or_default(),
                collation: collation.unwrap_or_default(),
                extra: on_update,
                generated: generated.unwrap_or_default(),
                stored: extra_lower.contains("stored"),
            });
        }

        let index_rows = sqlx::query(
            "SELECT index_name, non_unique, index_type, column_name, comment \
             FROM information_schema.statistics \
             WHERE table_schema = ? AND table_name = ? \
             ORDER BY index_name, seq_in_index",
        )
        .bind(database)
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;

        let mut indexes: Vec<IndexDef> = Vec::new();
        for row in &index_rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            let non_unique: i64 = row.try_get(1).unwrap_or(0);
            let index_type: String = row.try_get(2).unwrap_or_default();
            let column: Option<String> = row.try_get(3).unwrap_or(None);
            let comment: String = row.try_get(4).unwrap_or_default();
            if let Some(existing) = indexes.iter_mut().find(|index| index.name == name) {
                if let Some(column) = column {
                    existing.columns.push(column);
                }
            } else {
                indexes.push(IndexDef {
                    name: name.clone(),
                    columns: column.into_iter().collect(),
                    unique: non_unique == 0,
                    primary: name.eq_ignore_ascii_case("PRIMARY"),
                    index_type,
                    comment,
                });
            }
        }

        let fk_rows = sqlx::query(
            "SELECT k.constraint_name, k.column_name, k.referenced_table_name, \
                    k.referenced_column_name, r.update_rule, r.delete_rule \
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
        .fetch_all(&self.pool)
        .await
        .map_err(map_query_error)?;

        let mut foreign_keys: Vec<ForeignKeyDef> = Vec::new();
        for row in &fk_rows {
            let name: String = row.try_get(0).map_err(map_query_error)?;
            let column: Option<String> = row.try_get(1).unwrap_or(None);
            let ref_table: Option<String> = row.try_get(2).unwrap_or(None);
            let ref_column: Option<String> = row.try_get(3).unwrap_or(None);
            let update_rule: String = row.try_get(4).unwrap_or_default();
            let delete_rule: String = row.try_get(5).unwrap_or_default();
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
        .fetch_all(&self.pool)
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
        .fetch_optional(&self.pool)
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

        Ok(TableSchema {
            columns,
            indexes,
            foreign_keys,
            triggers,
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
        schema_sql(database, table, original, modified)
    }

    async fn close(&self) -> Result<()> {
        self.pool.close().await;
        Ok(())
    }
}

/// Build the DDL that turns `original` into `modified`; exposed free so it can be unit tested
/// without a live pool.
fn schema_sql(
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
        for foreign_key in &modified.foreign_keys {
            parts.push(foreign_key_sql(foreign_key));
        }
        let mut sql = format!("CREATE TABLE {qualified} (\n  {}\n)", parts.join(",\n  "));
        let options = table_options_sql(&modified.options);
        if !options.is_empty() {
            sql.push(' ');
            sql.push_str(&options);
        }
        sql.push(';');
        return sql;
    };

    let mut clauses: Vec<String> = Vec::new();

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
            Some(existing) if existing != column => {
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

    let original_primary = primary_columns(original);
    let modified_primary = primary_columns(modified);
    if original_primary != modified_primary {
        if !original_primary.is_empty() {
            clauses.push("DROP PRIMARY KEY".to_string());
        }
        if !modified_primary.is_empty() {
            let columns = modified_primary
                .iter()
                .map(|column| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ");
            clauses.push(format!("ADD PRIMARY KEY ({columns})"));
        }
    }

    for index in modified.indexes.iter().filter(|index| !index.primary) {
        match original
            .indexes
            .iter()
            .find(|candidate| candidate.name == index.name && !candidate.primary)
        {
            None => clauses.push(format!("ADD {}", index_sql(index))),
            Some(existing) if existing != index => {
                clauses.push(format!("DROP INDEX {}", quote_identifier(&index.name)));
                clauses.push(format!("ADD {}", index_sql(index)));
            }
            Some(_) => {}
        }
    }
    for index in original.indexes.iter().filter(|index| !index.primary) {
        if !modified
            .indexes
            .iter()
            .any(|candidate| candidate.name == index.name && !candidate.primary)
        {
            clauses.push(format!("DROP INDEX {}", quote_identifier(&index.name)));
        }
    }

    for foreign_key in &modified.foreign_keys {
        match original
            .foreign_keys
            .iter()
            .find(|candidate| candidate.name == foreign_key.name)
        {
            None => clauses.push(format!("ADD {}", foreign_key_sql(foreign_key))),
            Some(existing) if existing != foreign_key => {
                clauses.push(format!(
                    "DROP FOREIGN KEY {}",
                    quote_identifier(&foreign_key.name)
                ));
                clauses.push(format!("ADD {}", foreign_key_sql(foreign_key)));
            }
            Some(_) => {}
        }
    }
    for foreign_key in &original.foreign_keys {
        if !modified
            .foreign_keys
            .iter()
            .any(|candidate| candidate.name == foreign_key.name)
        {
            clauses.push(format!(
                "DROP FOREIGN KEY {}",
                quote_identifier(&foreign_key.name)
            ));
        }
    }

    if original.options != modified.options {
        let options = table_options_sql(&modified.options);
        if !options.is_empty() {
            clauses.push(options);
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

fn primary_columns(schema: &TableSchema) -> Vec<String> {
    schema
        .columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.clone())
        .collect()
}

/// Render one column definition for `CREATE`/`ALTER TABLE`.
fn column_sql(column: &ColumnDef) -> String {
    let mut sql = quote_identifier(&column.name);
    sql.push(' ');
    sql.push_str(column.data_type.trim());

    let length = column.length.trim();
    let decimals = column.decimals.trim();
    if !length.is_empty() && length != "0" {
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
        if let Some(default) = format_default(&column.default) {
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
    if !column.comment.trim().is_empty() {
        sql.push_str(" COMMENT ");
        sql.push_str(&quote_literal(column.comment.trim()));
    }
    sql
}

fn index_sql(index: &IndexDef) -> String {
    let columns = index
        .columns
        .iter()
        .map(|column| quote_identifier(column))
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
    let mut sql = format!(
        "CONSTRAINT {} FOREIGN KEY ({columns}) REFERENCES {} ({referenced})",
        quote_identifier(&foreign_key.name),
        quote_identifier(&foreign_key.referenced_table),
    );
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
    if !options.comment.trim().is_empty() {
        parts.push(format!("COMMENT={}", quote_literal(options.comment.trim())));
    }
    parts.join(" ")
}

/// Render a `DEFAULT ...` fragment for a column, quoting values and leaving expressions and
/// numbers bare.
fn format_default(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let upper = value.to_ascii_uppercase();
    if upper == "NULL" {
        return Some("DEFAULT NULL".to_string());
    }
    let expression = upper.starts_with("CURRENT_TIMESTAMP")
        || upper.starts_with("CURRENT_DATE")
        || upper.starts_with("CURRENT_TIME")
        || upper.starts_with("NOW(")
        || upper.starts_with("UUID(")
        || value.starts_with('(')
        || value.parse::<f64>().is_ok()
        || (value.len() >= 2 && value.starts_with('\'') && value.ends_with('\''));
    if expression {
        Some(format!("DEFAULT {value}"))
    } else {
        Some(format!("DEFAULT {}", quote_literal(value)))
    }
}

fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
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

/// Build the ` WHERE ...` fragment for a page request's filter, together with the values to bind
/// in placeholder order.
fn filter_clause(filter: &[FilterCondition]) -> (String, Vec<String>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    for condition in filter {
        if !condition.enabled || condition.column.is_empty() {
            continue;
        }
        let operator = condition.operator;
        if operator.needs_value() && condition.value.is_empty() {
            continue;
        }
        if operator.needs_second_value() && condition.value2.is_empty() {
            continue;
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
                    continue;
                }
                let placeholders = vec!["?"; values.len()].join(", ");
                binds.extend(values);
                format!("{column} IN ({placeholders})")
            }
            FilterOperator::NotInList => {
                let values = condition.list_values();
                if values.is_empty() {
                    continue;
                }
                let placeholders = vec!["?"; values.len()].join(", ");
                binds.extend(values);
                format!("{column} NOT IN ({placeholders})")
            }
        };

        if clauses.is_empty() {
            clauses.push(piece);
        } else {
            let conjunction = match condition.conjunction {
                FilterConjunction::And => "AND",
                FilterConjunction::Or => "OR",
            };
            clauses.push(format!("{conjunction} {piece}"));
        }
    }

    if clauses.is_empty() {
        (String::new(), Vec::new())
    } else {
        (format!(" WHERE {}", clauses.join(" ")), binds)
    }
}

/// Map a sqlx error to a Navicat-style message: `<code> - <message>` for MySQL server errors.
fn map_query_error(error: sqlx::Error) -> Error {
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

fn columns_from_row(row: &MySqlRow) -> Vec<ColumnInfo> {
    row.columns().iter().map(column_info).collect()
}

fn column_info(column: &MySqlColumn) -> ColumnInfo {
    ColumnInfo {
        name: column.name().to_string(),
        data_type: column.type_info().to_string(),
        nullable: true,
        primary_key: false,
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

    if let Ok(value) = row.try_get::<NaiveDateTime, _>(index) {
        return CellValue::Text(value.to_string());
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
        column_sql, filter_clause, format_default, order_clause, returns_result_set, schema_sql,
    };
    use navidog_core::{
        ColumnDef, FilterCondition, FilterConjunction, FilterOperator, PageRequest, SortColumn,
        TableSchema,
    };

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
        assert_eq!(format_default("").as_deref(), None);
        assert_eq!(format_default("NULL").as_deref(), Some("DEFAULT NULL"));
        assert_eq!(
            format_default("CURRENT_TIMESTAMP").as_deref(),
            Some("DEFAULT CURRENT_TIMESTAMP")
        );
        assert_eq!(format_default("5").as_deref(), Some("DEFAULT 5"));
        assert_eq!(format_default("abc").as_deref(), Some("DEFAULT 'abc'"));
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
        let sql = schema_sql("db", "t", None, &schema);
        assert!(sql.starts_with("CREATE TABLE `db`.`t`"));
        assert!(sql.contains("`id` int NOT NULL"));
        assert!(sql.contains("PRIMARY KEY (`id`)"));
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

        let sql = schema_sql("db", "t", Some(&original), &modified);
        assert!(sql.starts_with("ALTER TABLE `db`.`t`"));
        assert!(sql.contains("MODIFY COLUMN `name` varchar(100) NULL AFTER `id`"));
        assert!(sql.contains("ADD COLUMN `age` int NULL AFTER `name`"));
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
        assert!(schema_sql("db", "t", Some(&schema), &schema).is_empty());
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
            condition("name", FilterOperator::Contains, "ali"),
            condition("age", FilterOperator::GreaterOrEqual, "18"),
        ];
        filter[1].conjunction = FilterConjunction::Or;
        let (clause, binds) = filter_clause(&filter);
        assert_eq!(clause, " WHERE `name` LIKE ? OR `age` >= ?".to_string());
        assert_eq!(binds, vec!["%ali%".to_string(), "18".to_string()]);
    }

    #[test]
    fn filter_skips_disabled_and_valueless_conditions() {
        let mut filter = vec![
            condition("a", FilterOperator::IsNull, ""),
            condition("b", FilterOperator::Equal, ""),
        ];
        filter[0].enabled = false;
        assert_eq!(filter_clause(&filter), (String::new(), Vec::new()));

        let between = FilterCondition {
            column: "c".to_string(),
            operator: FilterOperator::Between,
            value: "1".to_string(),
            value2: "2".to_string(),
            conjunction: FilterConjunction::And,
            enabled: true,
        };
        let (clause, binds) = filter_clause(&[between]);
        assert_eq!(clause, " WHERE `c` BETWEEN ? AND ?".to_string());
        assert_eq!(binds, vec!["1".to_string(), "2".to_string()]);
    }

    #[test]
    fn filter_in_list_splits_values() {
        let condition = condition("id", FilterOperator::InList, "1, 2 ,3");
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
}
