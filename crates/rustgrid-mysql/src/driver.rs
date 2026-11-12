use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use rustgrid_core::{
    Connection, ConnectionConfig, ConnectionFieldChoice, ConnectionFieldKind, ConnectionFieldSpec,
    ConnectionFormSpec, ConnectionPage, ConnectionProfile, DatabaseEditorSpec, DesignerTab, Driver,
    DriverCapabilities, DriverCapability, DriverDescriptor, DriverDialect, DriverIconStyle,
    DriverId, Error, Result, TableDesignerSpec, TlsMode,
};
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};

use crate::connection::{MysqlConnection, is_prepare_sticky_proxy};
use crate::engine::Engine;
use rustgrid_tunnel::Tunnel;

/// The MySQL-protocol designer shape: MySQL/MariaDB's default (indexes carry uniqueness, so no
/// 唯一键 tab) plus the 检查 tab both engines support since MySQL 8.0.16 / MariaDB 10.2 and the
/// 触发器 tab (the shared trigger model plus the MySQL `DROP`/`CREATE TRIGGER` diff).
fn mysql_table_designer() -> TableDesignerSpec {
    let mut spec = TableDesignerSpec::default();
    let position = spec
        .tabs
        .iter()
        .position(|tab| *tab == DesignerTab::Options)
        .unwrap_or(spec.tabs.len());
    spec.tabs.insert(position, DesignerTab::Checks);
    spec.tabs.insert(position + 1, DesignerTab::Triggers);
    spec.tabs.insert(position + 2, DesignerTab::Partition);
    spec
}

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

    /// MySQL's conventional administrative account.
    fn default_username(&self) -> &'static str {
        "root"
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
            // MySQL's own shape is the designer's default (a unique index is an 索引, so there is
            // no 唯一键 tab), plus the 检查 tab MySQL 8.0.16+/MariaDB supports.
            table_designer: mysql_table_designer(),
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
            connection_form: ConnectionFormSpec {
                options: vec![ConnectionFieldSpec {
                    key: "mysql.compatibility",
                    label_key: "form.mysql.compatibility",
                    hint_key: Some("form.mysql.compatibility_hint"),
                    kind: ConnectionFieldKind::Select(vec![
                        ConnectionFieldChoice {
                            value: "default",
                            label_key: "form.mysql.compatibility_default",
                        },
                        ConnectionFieldChoice {
                            value: "starrocks",
                            label_key: "form.mysql.compatibility_starrocks",
                        },
                    ]),
                    page: ConnectionPage::Advanced,
                    visible_when: None,
                }],
                ..Default::default()
            },
            order: 10,
        }
    }

    fn dialect(&self) -> DriverDialect {
        DriverDialect::Mysql
    }

    /// StarRocks keeps the MySQL wire protocol but cannot execute RustGrid's MySQL account model,
    /// stored-routine catalog or table DDL. Its compatibility mode therefore advertises only the
    /// features that actually work and hides the rest in the UI.
    fn capabilities_for(&self, profile: &ConnectionProfile) -> DriverCapabilities {
        if is_starrocks(&profile.options) {
            DriverCapabilities::none().with(DriverCapability::DatabaseManagement)
        } else {
            self.descriptor().capabilities
        }
    }

    fn supports_table_designer(&self, profile: &ConnectionProfile) -> bool {
        !is_starrocks(&profile.options)
    }

    fn supports_backup(&self, profile: &ConnectionProfile) -> bool {
        !is_starrocks(&profile.options)
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

    /// MariaDB, like MySQL, ships a `root` account.
    fn default_username(&self) -> &'static str {
        "root"
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
            table_designer: mysql_table_designer(),
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
    let mut engine = engine;
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

    if engine == Engine::MySql && is_starrocks(&config.options) {
        // StarRocks rejects SQLx's SET sql_mode=(SELECT CONCAT(...)) before after_connect
        // can run. Disable both additions, leaving the server's existing sql_mode intact.
        options = options.pipes_as_concat(false).no_engine_substitution(false);
        engine = Engine::StarRocks;
    }

    if let Some(database) = &config.database {
        options = options.database(database);
    }

    if let Some(password) = &config.password {
        options = options.password(password);
    }

    // sqlx defaults to forcing `time_zone='+00:00'` so `DateTime<Utc>` values are unambiguous.
    // RustGrid shows the values the server reports, like any plain client: forcing UTC made every
    // `TIMESTAMP` column read differently from Navicat and the server's own `SELECT` whenever the
    // server's time zone is not UTC. Keep the server's session time zone instead (and still let
    // the profile's init SQL override it).
    options = options.timezone(None::<String>);

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
    // Without a configured value, close pooled connections after a short idle time: a server
    // with a short `wait_timeout` (or a NAT / firewall) drops idle sessions silently, and
    // reusing such a dead socket can stall a query for minutes before the pool gives up.
    let idle_seconds = settings
        .keepalive
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_IDLE_SECONDS);
    pool_options = pool_options.idle_timeout(Duration::from_secs(idle_seconds));

    // Per-session settings applied to every pooled connection.
    let query_timeout = settings.query_timeout.filter(|seconds| *seconds > 0);
    let read_only = settings.read_only;
    let init_sql = settings.init_sql.trim().to_string();
    // Shared by every pooled connection: once one greeting names UDAL/TeleDB's DBProxy, the
    // whole pool must stay on the text protocol (its first COM_STMT_PREPARE makes every later
    // text query return binary rows — issue #56). The greeting already carries the version, so
    // no extra query or connection is needed.
    let text_protocol = Arc::new(AtomicBool::new(false));
    let needs_database = Arc::new(AtomicBool::new(false));
    let after_text_protocol = Arc::clone(&text_protocol);
    let after_needs_database = Arc::clone(&needs_database);
    pool_options = pool_options.after_connect(move |connection, _metadata| {
        let init_sql = init_sql.clone();
        let text_protocol = Arc::clone(&after_text_protocol);
        let needs_database = Arc::clone(&after_needs_database);
        Box::pin(async move {
            if is_prepare_sticky_proxy(connection.server_version_string()) {
                text_protocol.store(true, Ordering::Relaxed);
                needs_database.store(true, Ordering::Relaxed);
            }
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
                let applied =
                    sqlx::raw_sql(sqlx::AssertSqlSafe(engine.statement_timeout_sql(seconds)))
                        .execute(&mut *connection)
                        .await;
                // MySQL before 5.7 and MariaDB before 10.1 have no per-statement timeout
                // and reject the variable as unknown: run without the cap there rather
                // than failing every connection to an older server.
                if let Err(error) = applied
                    && !is_unknown_system_variable(&error)
                {
                    return Err(error);
                }
            }
            Ok(())
        })
    });

    let pool = pool_options
        .connect_with(options)
        .await
        .map_err(map_connect_error)?;

    Ok(Box::new(MysqlConnection::new(
        pool,
        engine,
        tunnel,
        text_protocol,
        needs_database,
    )))
}

/// How long a pooled connection may sit idle when the profile sets no keep-alive. The pool's
/// reaper runs on this period, so a connection is closed within twice this time, below the
/// short `wait_timeout` settings some servers use (under a minute).
const DEFAULT_IDLE_SECONDS: u64 = 20;

/// Whether the connection options select the StarRocks compatibility mode.
fn is_starrocks(options: &std::collections::BTreeMap<String, String>) -> bool {
    options.get("mysql.compatibility").map(String::as_str) == Some("starrocks")
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

/// Whether `error` is MySQL's `ER_UNKNOWN_SYSTEM_VARIABLE` (1193).
fn is_unknown_system_variable(error: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(database_error) = error
        && let Some(mysql_error) =
            database_error.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
    {
        return mysql_error.number() == 1193;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgrid_core::ConnectionOptions;

    fn profile(mode: Option<&str>) -> ConnectionProfile {
        let mut options = std::collections::BTreeMap::new();
        if let Some(mode) = mode {
            options.insert("mysql.compatibility".to_string(), mode.to_string());
        }
        ConnectionProfile {
            id: "test".to_string(),
            name: "test".to_string(),
            driver: DriverId::new("mysql"),
            host: "127.0.0.1".to_string(),
            port: 9030,
            username: "root".to_string(),
            database: None,
            options,
            settings: ConnectionOptions::default(),
        }
    }

    #[test]
    fn starrocks_compatibility_hides_the_unsupported_features() {
        let driver = MysqlDriver::new();
        let starrocks = profile(Some("starrocks"));
        assert!(
            driver
                .capabilities_for(&starrocks)
                .has(DriverCapability::DatabaseManagement)
        );
        assert!(
            !driver
                .capabilities_for(&starrocks)
                .has(DriverCapability::Users)
        );
        assert!(
            !driver
                .capabilities_for(&starrocks)
                .has(DriverCapability::Routines)
        );
        assert!(!driver.supports_table_designer(&starrocks));
        assert!(!driver.supports_backup(&starrocks));

        let mysql = profile(None);
        assert!(driver.capabilities_for(&mysql).has(DriverCapability::Users));
        assert!(
            driver
                .capabilities_for(&mysql)
                .has(DriverCapability::Routines)
        );
        assert!(driver.supports_table_designer(&mysql));
        assert!(driver.supports_backup(&mysql));
    }
}
