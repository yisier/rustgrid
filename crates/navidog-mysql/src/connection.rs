use async_trait::async_trait;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use navidog_core::{
    CellValue, ColumnInfo, Connection, DatabaseInfo, DriverId, Error, ObjectKind, PageRequest,
    Result, RowUpdate, TableInfo, TablePage,
};
use sqlx::mysql::MySqlRow;
use sqlx::{MySqlPool, Row, ValueRef};

pub struct MysqlConnection {
    pool: MySqlPool,
}

impl MysqlConnection {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    async fn scalar_u64(&self, sql: String) -> Result<u64> {
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_one(&self.pool)
            .await
            .map_err(|error| Error::Query(error.to_string()))?;

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
            .map_err(|error| Error::Query(error.to_string()))?;

        let mut databases = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row
                .try_get(0)
                .map_err(|error| Error::Query(error.to_string()))?;
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
        .map_err(|error| Error::Query(error.to_string()))?;

        let mut tables = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row
                .try_get(0)
                .map_err(|error| Error::Query(error.to_string()))?;
            let table_type: String = row
                .try_get(1)
                .map_err(|error| Error::Query(error.to_string()))?;
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
        .map_err(|error| Error::Query(error.to_string()))?;

        let mut columns = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row
                .try_get(0)
                .map_err(|error| Error::Query(error.to_string()))?;
            let data_type: String = row
                .try_get(1)
                .map_err(|error| Error::Query(error.to_string()))?;
            let nullable: String = row
                .try_get(2)
                .map_err(|error| Error::Query(error.to_string()))?;
            let key: String = row
                .try_get(3)
                .map_err(|error| Error::Query(error.to_string()))?;
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

        let total_rows = self
            .scalar_u64(format!("SELECT COUNT(*) FROM {qualified}"))
            .await
            .ok();

        let page_sql = format!("SELECT * FROM {qualified} LIMIT ? OFFSET ?");
        let rows = sqlx::query(sqlx::AssertSqlSafe(page_sql))
            .bind(page.page_size as i64)
            .bind(page.offset() as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| Error::Query(error.to_string()))?;

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
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| Error::Query(error.to_string()))?;

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
                .map_err(|error| Error::Query(error.to_string()))?;
        }

        transaction
            .commit()
            .await
            .map_err(|error| Error::Query(error.to_string()))?;
        Ok(())
    }

    async fn create_database(&self, name: &str) -> Result<()> {
        let sql = format!("CREATE DATABASE {}", quote_identifier(name));
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&self.pool)
            .await
            .map_err(|error| Error::Query(error.to_string()))?;
        Ok(())
    }

    async fn drop_database(&self, name: &str) -> Result<()> {
        let sql = format!("DROP DATABASE {}", quote_identifier(name));
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&self.pool)
            .await
            .map_err(|error| Error::Query(error.to_string()))?;
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
        .map_err(|error| Error::Query(error.to_string()))?;

        let charset: String = row
            .try_get(0)
            .map_err(|error| Error::Query(error.to_string()))?;
        let collation: String = row
            .try_get(1)
            .map_err(|error| Error::Query(error.to_string()))?;
        Ok((charset, collation))
    }

    async fn character_sets(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT character_set_name FROM information_schema.character_sets \
             ORDER BY character_set_name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| Error::Query(error.to_string()))?;

        rows.iter()
            .map(|row| {
                row.try_get(0)
                    .map_err(|error| Error::Query(error.to_string()))
            })
            .collect()
    }

    async fn collations(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT collation_name FROM information_schema.collations \
             ORDER BY collation_name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| Error::Query(error.to_string()))?;

        rows.iter()
            .map(|row| {
                row.try_get(0)
                    .map_err(|error| Error::Query(error.to_string()))
            })
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
            .map_err(|error| Error::Query(error.to_string()))?;
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

    async fn close(&self) -> Result<()> {
        self.pool.close().await;
        Ok(())
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
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
