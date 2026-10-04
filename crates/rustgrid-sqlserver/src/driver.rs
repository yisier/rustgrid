use std::time::Duration;

use async_trait::async_trait;
use rustgrid_core::{
    Connection, ConnectionConfig, DatabaseEditorSpec, DatabaseEditorTab, Driver, DriverId, Error,
    Result, TlsMode,
};
use tiberius::{AuthMethod, Config};

use crate::connection::SqlServerConnection;
use crate::pool::{ConnectionManager, encryption_level};

/// The Microsoft SQL Server driver.
#[derive(Debug, Default)]
pub struct SqlServerDriver;

impl SqlServerDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Driver for SqlServerDriver {
    fn id(&self) -> DriverId {
        DriverId::new("sqlserver")
    }

    fn display_name(&self) -> String {
        "SQL Server".to_string()
    }

    fn default_port(&self) -> u16 {
        1433
    }

    fn is_file_based(&self) -> bool {
        false
    }

    fn supports_database_management(&self) -> bool {
        true
    }

    fn supports_users(&self) -> bool {
        true
    }

    fn supports_routines(&self) -> bool {
        true
    }

    fn supports_schemas(&self) -> bool {
        true
    }

    fn database_editor(&self) -> DatabaseEditorSpec {
        DatabaseEditorSpec {
            charset: false,
            collation: true,
            owner: true,
            recovery_model: true,
            compatibility_level: true,
            extra_tabs: vec![
                DatabaseEditorTab::Filegroups,
                DatabaseEditorTab::Files,
                DatabaseEditorTab::Advanced,
                DatabaseEditorTab::Comment,
            ],
        }
    }

    fn database_recovery_models(&self) -> Vec<&'static str> {
        vec!["SIMPLE", "FULL", "BULK_LOGGED"]
    }

    fn database_compatibility_levels(&self) -> Vec<&'static str> {
        vec![
            "80", "90", "100", "110", "120", "130", "140", "150", "160", "170",
        ]
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        let settings = &config.settings;
        let (host, instance) = split_host(&config.host);

        let mut server = Config::new();
        server.host(host);
        server.port(config.port.max(1));
        server.application_name("RustGrid");
        if let Some(instance) = instance {
            server.instance_name(instance);
        }
        if let Some(database) = config.database.as_deref().filter(|value| !value.is_empty()) {
            server.database(database);
        }
        server.authentication(AuthMethod::sql_server(
            config.username.as_str(),
            config.password.as_deref().unwrap_or(""),
        ));
        server.encryption(encryption_level(settings.tls.mode));
        server.readonly(settings.read_only);

        // Certificate trust.
        match settings.tls.mode {
            // Disabled only encrypts the login handshake; Preferred/Required encrypt all
            // traffic. None of them verify the (usually self-signed) server certificate.
            TlsMode::Disabled | TlsMode::Preferred | TlsMode::Required => server.trust_cert(),
            // VerifyCa / VerifyIdentity validate against the system store, optionally
            // extended with a configured CA. tiberius (rustls) always verifies the host
            // name too, so VerifyCa is stricter than its MySQL counterpart.
            TlsMode::VerifyCa | TlsMode::VerifyIdentity => {
                if !settings.tls.ca.trim().is_empty() {
                    server.trust_cert_ca(settings.tls.ca.trim());
                }
            }
        }
        if !settings.tls.cert.trim().is_empty() && !settings.tls.key.trim().is_empty() {
            server.client_certificate(settings.tls.cert.trim(), settings.tls.key.trim());
        }

        if let Some(seconds) = settings.connect_timeout.filter(|seconds| *seconds > 0) {
            server.handshake_timeout(Some(Duration::from_secs(seconds)));
        }
        if let Some(seconds) = settings.query_timeout.filter(|seconds| *seconds > 0) {
            server.command_timeout(Some(Duration::from_secs(seconds)));
        }

        let manager = ConnectionManager::new(server, settings.init_sql.clone());
        // `min_idle(1)` makes `build` establish (and therefore validate) one connection up
        // front, so a bad password surfaces from `connect` rather than on first use.
        let mut builder = bb8::Pool::builder()
            .max_size(5)
            .min_idle(Some(1))
            // A failed login will not succeed on retry; fail fast so the password prompt
            // appears immediately instead of after the connection timeout.
            .retry_connection(false);
        if let Some(seconds) = settings.connect_timeout.filter(|seconds| *seconds > 0) {
            builder = builder.connection_timeout(Duration::from_secs(seconds));
        }
        if let Some(seconds) = settings.keepalive.filter(|seconds| *seconds > 0) {
            builder = builder.idle_timeout(Some(Duration::from_secs(seconds)));
        }

        let pool = builder.build(manager).await.map_err(map_connect_error)?;
        Ok(Box::new(SqlServerConnection::new(pool)))
    }
}

/// Split `host\instance` into its parts. SQL Browser named-instance resolution is not
/// compiled in, so the instance name is only informational.
fn split_host(host: &str) -> (String, Option<String>) {
    match host.split_once('\\') {
        Some((host, instance)) if !instance.is_empty() => {
            (host.to_string(), Some(instance.to_string()))
        }
        _ => (host.to_string(), None),
    }
}

/// Map a tiberius error to a core error, flagging login failures as authentication errors.
pub(crate) fn map_connect_error(error: tiberius::error::Error) -> Error {
    if is_authentication_error(&error) {
        Error::Authentication(error.to_string())
    } else {
        Error::Connection(error.to_string())
    }
}

/// Map a query-time tiberius error to a core error.
pub(crate) fn map_query_error(error: tiberius::error::Error) -> Error {
    if is_authentication_error(&error) {
        Error::Authentication(error.to_string())
    } else {
        Error::Query(error.to_string())
    }
}

/// SQL Server login/authentication failure error codes.
fn is_authentication_error(error: &tiberius::error::Error) -> bool {
    // 18456 login failed, 18452 login from untrusted domain, 18470 account disabled,
    // 4060 cannot open the requested database, 916 insufficient permission on login.
    matches!(error.code(), Some(18456 | 18452 | 18470 | 4060 | 916))
}
