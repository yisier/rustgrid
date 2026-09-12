use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DriverId(pub String);

impl DriverId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DriverId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<&str> for DriverId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub driver: DriverId,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub driver: DriverId,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: Option<String>,
    pub database: Option<String>,
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseInfo {
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectKind {
    Table,
    View,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub kind: ObjectKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
pub enum CellValue {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
}

impl CellValue {
    pub fn as_display(&self) -> String {
        use std::fmt::Write;

        match self {
            CellValue::Null => "NULL".to_string(),
            CellValue::Bool(value) => value.to_string(),
            CellValue::Int(value) => value.to_string(),
            CellValue::Uint(value) => value.to_string(),
            CellValue::Float(value) => value.to_string(),
            CellValue::Text(value) => value.clone(),
            CellValue::Bytes(bytes) => {
                let mut rendered = String::with_capacity(bytes.len() * 2);
                for byte in bytes {
                    let _ = write!(rendered, "{byte:02x}");
                }
                rendered
            }
        }
    }

    pub fn as_edit_string(&self) -> String {
        match self {
            CellValue::Null => String::new(),
            _ => self.as_display(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PageRequest {
    pub page: u64,
    pub page_size: u64,
}

impl PageRequest {
    pub fn new(page: u64, page_size: u64) -> Self {
        Self {
            page,
            page_size: page_size.max(1),
        }
    }

    pub fn offset(&self) -> u64 {
        self.page.saturating_mul(self.page_size)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TablePage {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<CellValue>>,
    pub page: u64,
    pub page_size: u64,
    pub total_rows: Option<u64>,
}

/// A single-row update: `SET` assignments plus the key columns used in the `WHERE` clause.
#[derive(Debug, Clone)]
pub struct RowUpdate {
    pub set: Vec<(String, Option<String>)>,
    pub keys: Vec<(String, String)>,
}
