use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oracledb::PoolConfig;
use rustgrid_core::{
    Connection, ConnectionConfig, ConnectionFieldChoice, ConnectionFieldKind, ConnectionFieldLabel,
    ConnectionFieldSpec, ConnectionFormSpec, ConnectionHomePage, ConnectionPage,
    ConnectionStandardField, DatabaseEditorSpec, Driver, DriverCapabilities, DriverCapability,
    DriverDescriptor, DriverDialect, DriverIconStyle, DriverId, Error, Result, TlsMode,
    UserEditorSpec,
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

    fn user_editor(&self) -> UserEditorSpec {
        UserEditorSpec {
            // Oracle users have no host, plugin, password expiry or resource limits. The account
            // list has no super-user concept (`is_super_user` is always false).
            host: false,
            authentication_plugin: false,
            password_expiry: false,
            password_valid_until: false,
            account_lock: true,
            account_enabled: false,
            ssl: false,
            max_questions: false,
            max_updates: false,
            max_connections: false,
            max_user_connections: false,
            profile: true,
            default_tablespace: true,
            tablespace_quota: true,
            list_super_user: false,
            server_privileges: true,
            object_privileges: true,
            default_privileges: false,
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
            connection_form: ConnectionFormSpec {
                home: ConnectionHomePage::Network,
                tabs: vec![
                    ConnectionPage::Tls,
                    ConnectionPage::Tunnel,
                    ConnectionPage::Advanced,
                ],
                // The universal "default database" field is Oracle's service/SID target; 连接类型
                // decides which. The wallet (TCPS credential) lives on TLS, the tnsnames.ora
                // directory on 高级.
                labels: vec![ConnectionFieldLabel {
                    field: ConnectionStandardField::Database,
                    label_key: "form.oracle.database",
                    hint_key: Some("form.oracle.database_hint"),
                }],
                options: vec![
                    ConnectionFieldSpec {
                        key: "oracle.connect_type",
                        label_key: "form.oracle.connect_type",
                        hint_key: None,
                        kind: ConnectionFieldKind::Select(vec![
                            ConnectionFieldChoice {
                                value: "service",
                                label_key: "form.oracle.type.service",
                            },
                            ConnectionFieldChoice {
                                value: "sid",
                                label_key: "form.oracle.type.sid",
                            },
                            ConnectionFieldChoice {
                                value: "tns",
                                label_key: "form.oracle.type.tns",
                            },
                            ConnectionFieldChoice {
                                value: "connect_string",
                                label_key: "form.oracle.type.connect_string",
                            },
                        ]),
                        page: ConnectionPage::General,
                        visible_when: None,
                    },
                    ConnectionFieldSpec {
                        key: "oracle.tns_alias",
                        label_key: "form.oracle.tns_alias",
                        hint_key: Some("form.oracle.tns_alias_hint"),
                        kind: ConnectionFieldKind::Text,
                        page: ConnectionPage::General,
                        visible_when: Some(("oracle.connect_type", "tns")),
                    },
                    ConnectionFieldSpec {
                        key: "oracle.connect_string",
                        label_key: "form.oracle.connect_string",
                        hint_key: Some("form.oracle.connect_string_hint"),
                        kind: ConnectionFieldKind::Text,
                        page: ConnectionPage::General,
                        visible_when: Some(("oracle.connect_type", "connect_string")),
                    },
                    ConnectionFieldSpec {
                        key: "oracle.wallet_location",
                        label_key: "form.oracle.wallet_location",
                        hint_key: Some("form.oracle.wallet_location_hint"),
                        kind: ConnectionFieldKind::Folder,
                        page: ConnectionPage::Tls,
                        visible_when: None,
                    },
                    ConnectionFieldSpec {
                        key: "oracle.wallet_password",
                        label_key: "form.oracle.wallet_password",
                        hint_key: None,
                        kind: ConnectionFieldKind::Secret,
                        page: ConnectionPage::Tls,
                        visible_when: None,
                    },
                    ConnectionFieldSpec {
                        key: "oracle.config_dir",
                        label_key: "form.oracle.config_dir",
                        hint_key: Some("form.oracle.config_dir_hint"),
                        kind: ConnectionFieldKind::Folder,
                        page: ConnectionPage::Advanced,
                        visible_when: None,
                    },
                ],
            },
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
    let protocol = if config.settings.tls.mode == TlsMode::Disabled {
        "tcp"
    } else {
        "tcps"
    };
    // 连接类型 = SID: Easy Connect only takes a service name, so build a connect descriptor.
    if option(config, "oracle.connect_type").as_deref() == Some("sid") {
        let sid = option(config, "oracle.sid").or_else(|| {
            config
                .database
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        });
        if let Some(sid) = sid {
            return format!(
                "(DESCRIPTION=(ADDRESS=(PROTOCOL={protocol})(HOST={})(PORT={port}))(CONNECT_DATA=(SID={sid})))",
                host.trim()
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every declared field must map to an `oracle.*` option key the connect path reads, so a
    /// field can never be a dead knob.
    #[test]
    fn declared_connection_fields_are_honored_options() {
        let descriptor = OracleDriver::new().descriptor();
        let form = &descriptor.connection_form;
        let keys: Vec<_> = form.options.iter().map(|field| field.key).collect();
        assert_eq!(
            keys,
            vec![
                "oracle.connect_type",
                "oracle.tns_alias",
                "oracle.connect_string",
                "oracle.wallet_location",
                "oracle.wallet_password",
                "oracle.config_dir",
            ]
        );
        // 连接类型 on 常规, the TCPS wallet on TLS, the tnsnames.ora directory on 高级.
        assert_eq!(form.options[0].page, ConnectionPage::General);
        assert_eq!(form.options[3].page, ConnectionPage::Tls);
        assert_eq!(form.options[5].page, ConnectionPage::Advanced);
        // The TNS alias / connect string are only shown for their connection type.
        assert_eq!(
            form.options[1].visible_when,
            Some(("oracle.connect_type", "tns"))
        );
        assert_eq!(
            form.options[2].visible_when,
            Some(("oracle.connect_type", "connect_string"))
        );
    }

    #[test]
    fn builds_easy_connect_and_sid_connect_strings() {
        let mut config = ConnectionConfig {
            driver: DriverId::new("oracle"),
            host: "db.example.com".to_string(),
            port: 1521,
            username: "scott".to_string(),
            password: None,
            database: Some("FREEPDB1".to_string()),
            options: std::collections::BTreeMap::new(),
            settings: rustgrid_core::ConnectionOptions::default(),
        };
        assert_eq!(
            connect_string(&config, "db.example.com", 1521),
            "db.example.com:1521/FREEPDB1"
        );

        config
            .options
            .insert("oracle.connect_type".to_string(), "sid".to_string());
        config.database = Some("ORCL".to_string());
        assert_eq!(
            connect_string(&config, "db.example.com", 1521),
            "(DESCRIPTION=(ADDRESS=(PROTOCOL=tcp)(HOST=db.example.com)(PORT=1521))(CONNECT_DATA=(SID=ORCL)))"
        );

        // A TNS alias wins over the SID type (legacy profiles keep working).
        config
            .options
            .insert("oracle.tns_alias".to_string(), "ORCL_ALIAS".to_string());
        assert_eq!(
            connect_string(&config, "db.example.com", 1521),
            "ORCL_ALIAS"
        );
    }
}
