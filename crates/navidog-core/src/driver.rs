use async_trait::async_trait;

use crate::error::Result;
use crate::model::{
    ColumnInfo, ConnectionConfig, DatabaseInfo, DriverId, PageRequest, TableInfo, TablePage,
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

    async fn close(&self) -> Result<()>;
}
