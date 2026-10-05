use std::time::Duration;

use async_trait::async_trait;
use rustgrid_core::{
    Connection, ConnectionConfig, DatabaseEditorSpec, Driver, DriverCapabilities, DriverCapability,
    DriverDescriptor, DriverDialect, DriverIconStyle, DriverId, Error, Result, TlsMode,
};
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};

use crate::connection::MysqlConnection;
use crate::engine::Engine;
use crate::tunnel::Tunnel;

/// The MySQL driver.
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

    fn database_editor(&self) -> DatabaseEditorSpec {
        DatabaseEditorSpec {
            charset: true,
            collation: true,
            ..Default::default()
        }
    }

    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            id: DriverId::new("mysql"),
            display_name: "MySQL".to_string(),
            default_port: 3306,
            is_file_based: false,
            icon: "icons/mysql.svg",
            icon_style: DriverIconStyle::SolidBadge,
            capabilities: DriverCapabilities::none()
                .with(DriverCapability::DatabaseManagement)
                .with(DriverCapability::Users)
                .with(DriverCapability::Routines),
            database_editor: self.database_editor(),
            connection_form: Default::default(),
            order: 10,
        }
    }

    fn dialect(&self) -> DriverDialect {
        DriverDialect::Mysql
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        connect(config, Engine::MySql).await
    }
}

/// The MariaDB driver. MariaDB speaks the MySQL wire protocol and shares almost all catalog and
/// DDL syntax with MySQL, so it reuses [`MysqlConnection`]; [`Engine`] carries the few dialect
/// differences (driver id, session-variable names, ...).
#[derive(Debug, Default)]
pub struct MariaDbDriver;

impl MariaDbDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Driver for MariaDbDriver {
    fn id(&self) -> DriverId {
        DriverId::new("mariadb")
    }

    fn display_name(&self) -> String {
        "MariaDB".to_string()
    }

    fn default_port(&self) -> u16 {
        3306
    }

    fn database_editor(&self) -> DatabaseEditorSpec {
        DatabaseEditorSpec {
            charset: true,
            collation: true,
            ..Default::default()
        }
    }

    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            id: DriverId::new("mariadb"),
            display_name: "MariaDB".to_string(),
            default_port: 3306,
            is_file_based: false,
            icon: "icons/mariadb.svg",
            icon_style: DriverIconStyle::SolidBadge,
            capabilities: DriverCapabilities::none()
                .with(DriverCapability::DatabaseManagement)
                .with(DriverCapability::Users)
                .with(DriverCapability::Routines),
            database_editor: self.database_editor(),
            connection_form: Default::default(),
            order: 60,
        }
    }

    fn dialect(&self) -> DriverDialect {
        DriverDialect::Mysql
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        connect(config, Engine::MariaDb).await
    }
}

/// Open a pooled connection to `config` for the requested engine flavour. Shared by both drivers:
/// the pool setup, TLS, tunnel and session initialization are identical; only the engine-specific
/// session SQL differs.
async fn connect(config: &ConnectionConfig, engine: Engine) -> Result<Box<dyn Connection>> {
    let settings = &config.settings;

    // Route through the tunnel (if any): the connection then points at the local forwarder,
    // which is kept alive alongside the pool.
    let (host, port, tunnel) = if settings.tunnel.is_empty() {
        (config.host.clone(), config.port, None)
    } else {
        let tunnel = Tunnel::start(settings.tunnel.clone(), config.host.clone(), config.port)
            .await
            .map_err(Error::Connection)?;
        ("127.0.0.1".to_string(), tunnel.local_port(), Some(tunnel))
    };

    let mut options = MySqlConnectOptions::new()
        .host(&host)
        .port(port)
        .username(&config.username);

    if let Some(database) = &config.database {
        options = options.database(database);
    }

    if let Some(password) = &config.password {
        options = options.password(password);
    }

    // TLS / SSL.
    options = options.ssl_mode(match settings.tls.mode {
        TlsMode::Disabled => MySqlSslMode::Disabled,
        TlsMode::Preferred => MySqlSslMode::Preferred,
        TlsMode::Required => MySqlSslMode::Required,
        TlsMode::VerifyCa => MySqlSslMode::VerifyCa,
        TlsMode::VerifyIdentity => MySqlSslMode::VerifyIdentity,
    });
    if !settings.tls.ca.trim().is_empty() {
        options = options.ssl_ca(settings.tls.ca.trim());
    }
    if !settings.tls.cert.trim().is_empty() {
        options = options.ssl_client_cert(settings.tls.cert.trim());
    }
    if !settings.tls.key.trim().is_empty() {
        options = options.ssl_client_key(settings.tls.key.trim());
    }
    if settings.cleartext_password {
        options = options.enable_cleartext_plugin(true);
    }

    // Pool: connection timeout and keep-alive.
    let mut pool_options = MySqlPoolOptions::new().max_connections(5);
    if let Some(seconds) = settings.connect_timeout.filter(|seconds| *seconds > 0) {
        pool_options = pool_options.acquire_timeout(Duration::from_secs(seconds));
    }
    if let Some(seconds) = settings.keepalive.filter(|seconds| *seconds > 0) {
        pool_options = pool_options.idle_timeout(Duration::from_secs(seconds));
    }

    // Per-session settings applied to every pooled connection.
    let query_timeout = settings.query_timeout.filter(|seconds| *seconds > 0);
    let read_only = settings.read_only;
    let init_sql = settings.init_sql.trim().to_string();
    if query_timeout.is_some() || read_only || !init_sql.is_empty() {
        pool_options = pool_options.after_connect(move |connection, _metadata| {
            let init_sql = init_sql.clone();
            Box::pin(async move {
                if !init_sql.is_empty() {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(init_sql))
                        .execute(&mut *connection)
                        .await?;
                }
                if read_only {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(
                        "SET SESSION TRANSACTION READ ONLY".to_string(),
                    ))
                    .execute(&mut *connection)
                    .await?;
                }
                if let Some(seconds) = query_timeout {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(engine.statement_timeout_sql(seconds)))
                        .execute(&mut *connection)
                        .await?;
                }
                Ok(())
            })
        });
    }

    let pool = pool_options
        .connect_with(options)
        .await
        .map_err(map_connect_error)?;

    Ok(Box::new(MysqlConnection::new(pool, engine, tunnel)))
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
