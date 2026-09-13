use async_trait::async_trait;

use crate::error::Result;
use crate::model::{
    ColumnInfo, ConnectionConfig, DatabaseInfo, DriverId, PageRequest, QueryResult, RowUpdate,
    TableInfo, TablePage,
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

    /// Delete the rows identified by `keys`: each entry is one row's key columns
    /// (`(column, value)` pairs). Implementations should delete atomically.
    async fn delete_rows(
        &self,
        database: &str,
        table: &str,
        keys: &[Vec<(String, String)>],
    ) -> Result<()>;

    /// Run an arbitrary SQL statement (or script) in the context of `database`, if given.
    ///
    /// Implementations must use the text protocol so that DDL and other statements that
    /// cannot be prepared still execute.
    async fn execute_query(&self, database: Option<&str>, sql: &str) -> Result<QueryResult>;

    async fn create_database(&self, name: &str) -> Result<()>;

    async fn drop_database(&self, name: &str) -> Result<()>;

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

    async fn close(&self) -> Result<()>;
}
