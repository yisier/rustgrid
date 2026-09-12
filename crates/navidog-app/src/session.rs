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
    pub password_saved: bool,
    pub status: ConnectionStatus,
    pub databases: Loadable<Vec<DatabaseNode>>,
    pub expanded: bool,
}

pub struct DatabaseNode {
    pub name: String,
    pub tables: Loadable<Vec<TableInfo>>,
    pub opened: bool,
    pub expanded: bool,
    pub categories: CategoryExpansion,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Tables,
    Views,
    Functions,
    Queries,
    Backups,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Tables,
        Category::Views,
        Category::Functions,
        Category::Queries,
        Category::Backups,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Tables => "common.table",
            Category::Views => "common.view",
            Category::Functions => "common.function",
            Category::Queries => "common.query",
            Category::Backups => "common.backup",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Category::Tables => "t",
            Category::Views => "v",
            Category::Functions => "f",
            Category::Queries => "q",
            Category::Backups => "b",
        }
    }

    pub fn icon_path(self) -> &'static str {
        match self {
            Category::Tables => "icons/tables.svg",
            Category::Views => "icons/views.svg",
            Category::Functions => "icons/functions.svg",
            Category::Queries => "icons/queries.svg",
            Category::Backups => "icons/backups.svg",
        }
    }
}

#[derive(Default)]
pub struct CategoryExpansion {
    pub tables: bool,
    pub views: bool,
    pub functions: bool,
    pub queries: bool,
    pub backups: bool,
}

impl CategoryExpansion {
    pub fn get(&self, category: Category) -> bool {
        match category {
            Category::Tables => self.tables,
            Category::Views => self.views,
            Category::Functions => self.functions,
            Category::Queries => self.queries,
            Category::Backups => self.backups,
        }
    }

    pub fn toggle(&mut self, category: Category) {
        let slot = match category {
            Category::Tables => &mut self.tables,
            Category::Views => &mut self.views,
            Category::Functions => &mut self.functions,
            Category::Queries => &mut self.queries,
            Category::Backups => &mut self.backups,
        };
        *slot = !*slot;
    }
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
