use async_trait::async_trait;
use navidog_core::{Connection, ConnectionConfig, Driver, DriverId, Error, Result};
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};

use crate::connection::MysqlConnection;

#[derive(Debug, Default)]
pub struct MysqlDriver;

impl MysqlDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Driver for MysqlDriver {
    fn id(&self) -> DriverId {
        DriverId::new("mysql")
    }

    fn display_name(&self) -> String {
        "MySQL".to_string()
    }

    fn default_port(&self) -> u16 {
        3306
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        let mut options = MySqlConnectOptions::new()
            .host(&config.host)
            .port(config.port)
            .username(&config.username);

        if let Some(database) = &config.database {
            options = options.database(database);
        }

        if let Some(password) = &config.password {
            options = options.password(password);
        }

        let pool = MySqlPoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(|error| Error::Connection(error.to_string()))?;

        Ok(Box::new(MysqlConnection::new(pool)))
    }
}
