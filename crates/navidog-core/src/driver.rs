use async_trait::async_trait;

use crate::error::Result;
use crate::model::{
    ColumnInfo, ConnectionConfig, DatabaseInfo, DriverId, PageRequest, RowUpdate, TableInfo,
    TablePage,
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

    async fn fetch_page(&self, database: &str, table: &str, page: PageRequest)
    -> Result<TablePage>;

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()>;

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
