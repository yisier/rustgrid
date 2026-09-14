use async_trait::async_trait;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use navidog_core::{
    CellValue, ColumnInfo, Connection, DatabaseInfo, DriverId, Error, FilterCondition,
    FilterConjunction, FilterOperator, ObjectKind, PageRequest, QueryResult, Result, RowUpdate,
    TableInfo, TablePage,
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

    async fn close(&self) -> Result<()> {
        self.pool.close().await;
        Ok(())
    }
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
    use super::{filter_clause, order_clause, returns_result_set};
    use navidog_core::{
        FilterCondition, FilterConjunction, FilterOperator, PageRequest, SortColumn,
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
