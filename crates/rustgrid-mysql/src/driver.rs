use async_trait::async_trait;
use rustgrid_core::{Connection, ConnectionConfig, Driver, DriverId, Error, Result};
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
            .map_err(map_connect_error)?;

        Ok(Box::new(MysqlConnection::new(pool)))
    }
}

fn map_connect_error(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(database_error) = &error
        && let Some(mysql_error) =
            database_error.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
        && matches!(mysql_error.number(), 1044 | 1045 | 1698)
    {
        return Error::Authentication(error.to_string());
    }

    Error::Connection(error.to_string())
}
