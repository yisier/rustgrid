use std::sync::Arc;

use navidog_core::{CellValue, ColumnInfo, Connection, ConnectionProfile, TableInfo};

#[derive(Default)]
pub enum Loadable<T> {
    #[default]
    Idle,
    Loading,
    Loaded(T),
    Failed(String),
}

pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected(Arc<dyn Connection>),
    Failed(String),
}

pub struct ConnectionNode {
    pub profile: ConnectionProfile,
    pub password: Option<String>,
    pub status: ConnectionStatus,
    pub databases: Loadable<Vec<DatabaseNode>>,
    pub expanded: bool,
}

pub struct DatabaseNode {
    pub name: String,
    pub tables: Loadable<Vec<TableInfo>>,
    pub expanded: bool,
}

pub struct GridState {
    pub connection: Arc<dyn Connection>,
    pub database: String,
    pub table: String,
    pub page_index: u64,
    pub page_size: u64,
    pub loading: bool,
    pub error: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<CellValue>>,
    pub total_rows: Option<u64>,
}

impl GridState {
    pub fn has_next(&self) -> bool {
        match self.total_rows {
            Some(total) => (self.page_index + 1) * self.page_size < total,
            None => !self.rows.is_empty(),
        }
    }
}
