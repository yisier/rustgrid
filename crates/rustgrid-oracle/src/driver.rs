use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oracledb::PoolConfig;
use rustgrid_core::{
    Connection, ConnectionConfig, DatabaseEditorSpec, Driver, DriverCapabilities, DriverCapability,
    DriverDescriptor, DriverDialect, DriverIconStyle, DriverId, Error, Result, TlsMode,
};

use crate::connection::OracleConnection;

/// The Oracle driver, built on Oracle's official pure-Rust thin driver (`oracledb`).
#[derive(Debug, Default)]
pub struct OracleDriver;

impl OracleDriver {
    pub fn new() -> Self {
        Self
    }
}

/// The default service/PDB used when the profile names neither a database nor a service.
const DEFAULT_SERVICE: &str = "FREEPDB1";

#[async_trait]
impl Driver for OracleDriver {
    fn id(&self) -> DriverId {
        DriverId::new("oracle")
    }

    fn display_name(&self) -> String {
        "Oracle".to_string()
    }

    fn default_port(&self) -> u16 {
        1521
    }

    fn is_file_based(&self) -> bool {
        false
    }

    // Oracle databases are created by DBAs outside a session (init.ora, datafiles, tablespaces), so
    // the app hides the database-management actions (same trade-off as the ODBC driver).
    fn supports_database_management(&self) -> bool {
        false
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
        DatabaseEditorSpec::default()
    }

    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            id: DriverId::new("oracle"),
            display_name: "Oracle".to_string(),
            default_port: 1521,
            is_file_based: false,
            icon: "icons/oracle.svg",
            icon_style: DriverIconStyle::Brand(0xC74634),
            capabilities: DriverCapabilities::none()
                .with(DriverCapability::Users)
                .with(DriverCapability::Routines)
                .with(DriverCapability::Schemas),
            database_editor: self.database_editor(),
            connection_form: Default::default(),
            order: 30,
        }
    }

    fn dialect(&self) -> DriverDialect {
        DriverDialect::Oracle
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        let settings = &config.settings;

        // Route through the tunnel (if any): the pool then points at the local forwarder, which is
        // kept alive alongside the pool.
        let (host, port, tunnel) = if settings.tunnel.is_empty() {
            (config.host.clone(), config.port, None)
        } else {
            let tunnel = rustgrid_tunnel::Tunnel::start(
                settings.tunnel.clone(),
                config.host.clone(),
                config.port,
            )
            .await
            .map_err(Error::Connection)?;
            ("127.0.0.1".to_string(), tunnel.local_port(), Some(tunnel))
        };

        let connect_string = connect_string(config, &host, port);
        let user = config.username.clone();
        let password = config.password.clone().unwrap_or_default();
        let wallet_location = option(config, "oracle.wallet_location");
        let wallet_password = option(config, "oracle.wallet_password");
        let config_dir = option(config, "oracle.config_dir");

        // `set_config_dir` must precede `set_connect_string`, which resolves a TNS alias using the
        // configured directory.
        let mut builder = PoolConfig::default().set_credentials(&user, &password);
        if let Some(dir) = &config_dir {
            builder = builder.set_config_dir(dir);
        }
        let mut pool_config = builder
            .set_connect_string(&connect_string)
            .map_err(|error| Error::Connection(error.to_string()))?
            .set_min_connections(1)
            .set_max_connections(5);
        if let Some(wallet) = &wallet_location {
            pool_config = pool_config.set_wallet_location(wallet.clone());
        }
        if let Some(password) = &wallet_password {
            pool_config = pool_config.set_wallet_password(password);
        }

        // `create_pool` blocks (it spins up a background thread and opens the minimum connections),
        // so keep it off the async worker. The probe resolves the current schema and lets a bad
        // password surface at connect time rather than on the first query.
        let (pool, default_schema) = tokio::task::spawn_blocking(move || {
            let pool = oracledb::create_pool(pool_config).map_err(map_connect_error)?;
            let schema = {
                let connection = pool.acquire().map_err(map_connect_error)?;
                let row = connection
                    .query_row(
                        "SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM dual",
                        &[],
                    )
                    .map_err(map_connect_error)?;
                row.get::<Option<String>>(0).ok().flatten()
            };
            Ok::<_, Error>((pool, schema))
        })
        .await
        .map_err(|error| Error::Connection(format!("oracle worker panicked: {error}")))??;

        let default_schema = default_schema
            .filter(|schema| !schema.is_empty())
            .unwrap_or_else(|| user.to_ascii_uppercase());

        Ok(Box::new(OracleConnection::new(
            Arc::new(pool),
            default_schema,
            settings.init_sql.trim().to_string(),
            settings
                .query_timeout
                .filter(|seconds| *seconds > 0)
                .map(Duration::from_secs),
            tunnel,
        )))
    }
}

/// The Easy Connect string (or TNS alias / full descriptor) for a profile. A profile supplies the
/// service through `oracle.service_name` or the database field; TCPS is used whenever TLS is on.
fn connect_string(config: &ConnectionConfig, host: &str, port: u16) -> String {
    if let Some(value) = option(config, "oracle.connect_string") {
        return value;
    }
    if let Some(alias) = option(config, "oracle.tns_alias") {
        return alias;
    }
    let service = option(config, "oracle.service_name")
        .or_else(|| config.database.clone())
        .filter(|service| !service.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SERVICE.to_string());
    if config.settings.tls.mode == TlsMode::Disabled {
        format!("{}:{}/{}", host.trim(), port, service.trim())
    } else {
        format!("tcps://{}:{}/{}", host.trim(), port, service.trim())
    }
}

/// A trimmed non-empty profile option.
fn option(config: &ConnectionConfig, key: &str) -> Option<String> {
    config
        .options
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Map a connect-time error: an invalid credential/account becomes [`Error::Authentication`] so the
/// app prompts for a password instead of showing a generic connection error.
fn map_connect_error(error: oracledb::Error) -> Error {
    if let oracledb::ErrorKind::DbError(db_error) = error.kind() {
        if matches!(db_error.code(), 1017 | 28000 | 28001 | 28002 | 28003) {
            return Error::Authentication(format!(
                "ORA-{:05} - {}",
                db_error.code(),
                db_error.message()
            ));
        }
        return Error::Connection(format!(
            "ORA-{:05} - {}",
            db_error.code(),
            db_error.message()
        ));
    }
    Error::Connection(error.to_string())
}
