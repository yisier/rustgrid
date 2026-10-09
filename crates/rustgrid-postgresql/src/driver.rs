use std::time::Duration;

use async_trait::async_trait;
use rustgrid_core::{
    Connection, ConnectionConfig, DatabaseEditorSpec, Driver, DriverCapabilities, DriverCapability,
    DriverDescriptor, DriverDialect, DriverIconStyle, DriverId, Error, Result, TlsMode,
    UserEditorSpec,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};

use crate::connection::PostgresConnection;

/// The PostgreSQL driver.
#[derive(Debug, Default)]
pub struct PostgresDriver;

impl PostgresDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Driver for PostgresDriver {
    fn id(&self) -> DriverId {
        DriverId::new("postgresql")
    }

    fn display_name(&self) -> String {
        "PostgreSQL".to_string()
    }

    fn default_port(&self) -> u16 {
        5432
    }

    fn database_editor(&self) -> DatabaseEditorSpec {
        DatabaseEditorSpec {
            // `charset` maps to ENCODING, `collation` to LC_COLLATE, `owner` to OWNER TO.
            charset: true,
            collation: true,
            owner: true,
            ..Default::default()
        }
    }

    fn user_editor(&self) -> UserEditorSpec {
        UserEditorSpec {
            // PostgreSQL roles have no host, plugin, password expiry or MySQL resource limits;
            // only `rolconnlimit` (最大连接数) is honoured.
            host: false,
            authentication_plugin: false,
            password_expiry: false,
            password_valid_until: true,
            account_lock: true,
            account_enabled: false,
            ssl: false,
            max_questions: false,
            max_updates: false,
            max_connections: true,
            max_user_connections: false,
            profile: false,
            default_tablespace: false,
            tablespace_quota: false,
            list_super_user: true,
            server_privileges: true,
            object_privileges: true,
            default_privileges: true,
            object_privilege_manager: true,
            flush_privileges: false,
            roles: true,
            user_mapping: false,
            verification_type: false,
            endpoint_permissions: false,
            login_permissions: false,
        }
    }

    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            id: DriverId::new("postgresql"),
            display_name: "PostgreSQL".to_string(),
            default_port: 5432,
            is_file_based: false,
            icon: "icons/postgresql.svg",
            icon_style: DriverIconStyle::Brand(0x336791),
            capabilities: DriverCapabilities::none()
                .with(DriverCapability::DatabaseManagement)
                .with(DriverCapability::Users)
                .with(DriverCapability::Routines)
                .with(DriverCapability::Schemas),
            database_editor: self.database_editor(),
            connection_form: Default::default(),
            order: 20,
        }
    }

    fn dialect(&self) -> DriverDialect {
        DriverDialect::Postgres
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        let settings = &config.settings;

        // Route through the tunnel (if any): the connection then points at the local forwarder,
        // which is kept alive alongside the pools.
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

        let mut options = PgConnectOptions::new()
            .host(&host)
            .port(port)
            .username(&config.username)
            .application_name("RustGrid");

        if let Some(password) = &config.password {
            options = options.password(password);
        }

        options = options.ssl_mode(match settings.tls.mode {
            TlsMode::Disabled => PgSslMode::Disable,
            TlsMode::Preferred => PgSslMode::Prefer,
            TlsMode::Required => PgSslMode::Require,
            TlsMode::VerifyCa => PgSslMode::VerifyCa,
            TlsMode::VerifyIdentity => PgSslMode::VerifyFull,
        });
        if !settings.tls.ca.trim().is_empty() {
            options = options.ssl_root_cert(settings.tls.ca.trim());
        }
        if !settings.tls.cert.trim().is_empty() {
            options = options.ssl_client_cert(settings.tls.cert.trim());
        }
        if !settings.tls.key.trim().is_empty() {
            options = options.ssl_client_key(settings.tls.key.trim());
        }

        let default_database = config
            .database
            .clone()
            .filter(|database| !database.trim().is_empty())
            .unwrap_or_else(|| "postgres".to_string());

        // Session settings applied to every pooled connection.
        let query_timeout = settings.query_timeout.filter(|seconds| *seconds > 0);
        let read_only = settings.read_only;
        let init_sql = settings.init_sql.trim().to_string();

        let mut pool_options = PgPoolOptions::new().max_connections(5);
        if let Some(seconds) = settings.connect_timeout.filter(|seconds| *seconds > 0) {
            pool_options = pool_options.acquire_timeout(Duration::from_secs(seconds));
        }
        if let Some(seconds) = settings.keepalive.filter(|seconds| *seconds > 0) {
            pool_options = pool_options.idle_timeout(Duration::from_secs(seconds));
        }
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
                            "SET default_transaction_read_only = on".to_string(),
                        ))
                        .execute(&mut *connection)
                        .await?;
                    }
                    if let Some(seconds) = query_timeout {
                        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                            "SET statement_timeout = {}",
                            seconds.saturating_mul(1000)
                        )))
                        .execute(&mut *connection)
                        .await?;
                    }
                    Ok(())
                })
            });
        }

        // Probe the default database so a bad password/port surfaces at connect time.
        let default_options = options.clone().database(&default_database);
        let default_pool = pool_options
            .clone()
            .connect_with(default_options)
            .await
            .map_err(map_connect_error)?;

        Ok(Box::new(PostgresConnection::new(
            options,
            default_database,
            default_pool,
            pool_options,
            tunnel,
        )))
    }
}

fn map_connect_error(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(database_error) = &error
        && let Some(pg_error) = database_error.try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
        && matches!(pg_error.code(), "28P01" | "28000" | "28P02")
    {
        return Error::Authentication(error.to_string());
    }
    Error::Connection(error.to_string())
}
