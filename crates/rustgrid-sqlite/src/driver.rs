use std::time::Duration;

use async_trait::async_trait;
use rustgrid_core::{
    Connection, ConnectionConfig, DatabaseEditorSpec, Driver, DriverId, Error, Result,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::connection::SqliteConnection;

/// The SQLite driver. SQLite is a file-based engine: the database is a single file and there is no
/// server, user or password. The profile's `database` field holds the file path.
#[derive(Debug, Default)]
pub struct SqliteDriver;

impl SqliteDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Driver for SqliteDriver {
    fn id(&self) -> DriverId {
        DriverId::new("sqlite")
    }

    fn display_name(&self) -> String {
        "SQLite".to_string()
    }

    fn default_port(&self) -> u16 {
        0
    }

    fn is_file_based(&self) -> bool {
        true
    }

    fn supports_database_management(&self) -> bool {
        false
    }

    fn supports_users(&self) -> bool {
        false
    }

    fn supports_routines(&self) -> bool {
        false
    }

    fn database_editor(&self) -> DatabaseEditorSpec {
        DatabaseEditorSpec::default()
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        // The file path is carried in `database`; fall back to `host` so an older/hand-made profile
        // (or a copied MySQL one) still resolves.
        let path = config
            .database
            .clone()
            .filter(|path| !path.trim().is_empty())
            .or_else(|| (!config.host.trim().is_empty()).then(|| config.host.trim().to_string()))
            .ok_or_else(|| Error::Connection("a SQLite database file is required".to_string()))?;

        let mut options = SqliteConnectOptions::new()
            .filename(&path)
            // Only open an existing file. Creating a new database is a separate, explicit flow so a
            // typo cannot silently produce an empty file.
            .create_if_missing(false)
            .foreign_keys(true)
            .read_only(config.settings.read_only);
        if let Some(seconds) = config
            .settings
            .connect_timeout
            .filter(|seconds| *seconds > 0)
        {
            // SQLite has no connection timeout; the busy timeout is the closest equivalent (how
            // long it waits on a locked database before failing).
            options = options.busy_timeout(Duration::from_secs(seconds));
        }

        let init_sql = config.settings.init_sql.trim().to_string();
        let pool_options = SqlitePoolOptions::new().max_connections(5);
        let pool_options = if init_sql.is_empty() {
            pool_options
        } else {
            pool_options.after_connect(move |connection, _metadata| {
                let init_sql = init_sql.clone();
                Box::pin(async move {
                    sqlx::query(sqlx::AssertSqlSafe(init_sql))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
        };

        let pool = pool_options
            .connect_with(options)
            .await
            .map_err(map_connect_error)?;

        Ok(Box::new(SqliteConnection::new(pool)))
    }
}

fn map_connect_error(error: sqlx::Error) -> Error {
    Error::Connection(error.to_string())
}
