use async_trait::async_trait;

use crate::error::Result;
use crate::model::{
    ColumnInfo, ConnectionConfig, DatabaseInfo, DriverId, PageRequest, QueryResult, RowInsert,
    RowUpdate, TableInfo, TablePage, TableSchema,
};

#[async_trait]
pub trait Driver: Send + Sync {
    fn id(&self) -> DriverId;

    fn display_name(&self) -> String;

    fn default_port(&self) -> u16;

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>>;
}

#[async_trait]
pub trait Connection: Send + Sync {
    fn driver_id(&self) -> DriverId;

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>>;

    async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>>;

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>>;

    /// Fetch one page of a table. `page.order_by` is engine-agnostic: implementations must
    /// translate it to their own ordering syntax (e.g. SQL `ORDER BY`) and must keep row order
    /// stable across pages so pagination stays coherent.
    async fn fetch_page(&self, database: &str, table: &str, page: PageRequest)
    -> Result<TablePage>;

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()>;

    /// Insert new rows into `table`. Each row carries the columns to set; columns not listed take
    /// their default (or stay unset, e.g. for `AUTO_INCREMENT`). A `None` value is an explicit
    /// `NULL`. Implementations should insert atomically.
    async fn insert_rows(&self, database: &str, table: &str, rows: &[RowInsert]) -> Result<()>;

    /// Delete the rows identified by `keys`: each entry is one row's key columns
    /// (`(column, value)` pairs, `None` meaning SQL `NULL`). Implementations should delete
    /// atomically.
    async fn delete_rows(
        &self,
        database: &str,
        table: &str,
        keys: &[Vec<(String, Option<String>)>],
    ) -> Result<()>;

    /// Run an arbitrary SQL statement (or script) in the context of `database`, if given.
    ///
    /// Implementations must use the text protocol so that DDL and other statements that
    /// cannot be prepared still execute.
    async fn execute_query(&self, database: Option<&str>, sql: &str) -> Result<QueryResult>;

    async fn create_database(&self, name: &str) -> Result<()>;

    async fn drop_database(&self, name: &str) -> Result<()>;

    /// Drop a table.
    async fn drop_table(&self, database: &str, table: &str) -> Result<()>;

    /// Delete every row of a table (DML, so it is transactional/logged and can be rolled back).
    async fn empty_table(&self, database: &str, table: &str) -> Result<()>;

    /// Truncate a table (DDL): much faster than [`Connection::empty_table`] but cannot be rolled
    /// back on most engines.
    async fn truncate_table(&self, database: &str, table: &str) -> Result<()>;

    /// Rename a table within its database.
    async fn rename_table(&self, database: &str, table: &str, new_name: &str) -> Result<()>;

    async fn database_defaults(&self, name: &str) -> Result<(String, String)>;

    async fn character_sets(&self) -> Result<Vec<String>>;

    async fn collations(&self) -> Result<Vec<String>>;

    async fn alter_database_defaults(
        &self,
        name: &str,
        charset: Option<&str>,
        collation: Option<&str>,
    ) -> Result<()>;

    fn alter_database_sql(
        &self,
        name: &str,
        charset: Option<&str>,
        collation: Option<&str>,
    ) -> String;

    /// The column types this engine offers in the table designer's type list, in display order.
    fn column_types(&self) -> Vec<&'static str>;

    /// Introspect the full definition of an existing table (or view).
    async fn table_schema(&self, database: &str, table: &str) -> Result<TableSchema>;

    /// Build the DDL script that turns `original` into `modified`. When `original` is `None`
    /// the script creates the table; when it is `Some` it alters the table to match `modified`.
    /// The script is empty when nothing changed. It is used both for the designer's SQL preview
    /// and, via [`Connection::save_table_schema`], to apply the change.
    fn table_schema_sql(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> String;

    /// Apply the change described by [`Connection::table_schema_sql`].
    async fn save_table_schema(
        &self,
        database: &str,
        table: &str,
        original: Option<&TableSchema>,
        modified: &TableSchema,
    ) -> Result<()> {
        let sql = self.table_schema_sql(database, table, original, modified);
        if sql.trim().is_empty() {
            return Ok(());
        }
        self.execute_query(Some(database), &sql).await.map(|_| ())
    }

    async fn close(&self) -> Result<()>;
}
