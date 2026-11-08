use std::time::Duration;

use async_trait::async_trait;
use rustgrid_core::{
    AccountKind, AdvancedFields, AutoIncrementRule, Connection, ConnectionConfig,
    ConnectionFieldChoice, ConnectionFieldKind, ConnectionFieldLabel, ConnectionFieldSpec,
    ConnectionFormSpec, ConnectionHomePage, ConnectionPage, ConnectionStandardField,
    DatabaseEditorSpec, DatabaseEditorTab, DefaultSuggestion, DesignerColumnOption,
    DesignerFieldColumn, DesignerIndexColumn, DesignerIndexKind, DesignerTab, Driver,
    DriverCapabilities, DriverCapability, DriverDescriptor, DriverDialect, DriverIconStyle,
    DriverId, Error, Result, TableDesignerSpec, TlsMode, TriggerDefinition, UserEditorSpec,
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

    /// SQL Server's built-in system administrator login.
    fn default_username(&self) -> &'static str {
        "sa"
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
            ..Default::default()
        }
    }

    fn user_editor(&self) -> UserEditorSpec {
        UserEditorSpec {
            // Logins (SQL Server / Windows), Windows groups and server roles are all server-level
            // principals, so the 常规 page offers all three kinds.
            kinds: vec![AccountKind::User, AccountKind::Group, AccountKind::Role],
            // SQL Server logins have no user@host split, authentication plugin, password expiry or
            // resource limits, and object-level privilege management is not implemented yet.
            host: false,
            authentication_plugin: false,
            password_expiry: false,
            password_valid_until: false,
            account_lock: true,
            account_enabled: true,
            ssl: false,
            max_questions: false,
            max_updates: false,
            max_connections: false,
            max_user_connections: false,
            profile: false,
            default_tablespace: false,
            tablespace_quota: false,
            list_super_user: true,
            server_privileges: true,
            // The account editor's 权限 section needs per-database object grants the login's user
            // owns; object grants are managed through the 对象权限管理器 instead.
            object_privileges: false,
            default_privileges: false,
            object_privilege_manager: true,
            flush_privileges: false,
            roles: true,
            // SQL Server logins are mapped into databases as users, with database roles.
            user_mapping: true,
            // SQL Server logins have a verification type and endpoint/login securable permissions.
            verification_type: true,
            endpoint_permissions: true,
            login_permissions: true,
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

    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            // SQL Server keeps unique and check constraints apart from plain indexes, so the
            // designer gives each its own tab (MySQL reports both through 索引). Its column extras
            // are 排序规则 and 稀疏 — 无符号/填充零 are MySQL's, and 自动递增 is an identity column.
            table_designer: TableDesignerSpec {
                tabs: vec![
                    DesignerTab::Fields,
                    DesignerTab::Indexes,
                    DesignerTab::ForeignKeys,
                    DesignerTab::UniqueKeys,
                    DesignerTab::Checks,
                    DesignerTab::Triggers,
                    DesignerTab::Comment,
                    DesignerTab::Sql,
                ],
                column_options: vec![
                    DesignerColumnOption::Default,
                    DesignerColumnOption::AutoIncrement,
                    DesignerColumnOption::Collation,
                    DesignerColumnOption::Sparse,
                ],
                field_columns: vec![
                    DesignerFieldColumn::Size,
                    DesignerFieldColumn::Scale,
                    DesignerFieldColumn::NotNull,
                    DesignerFieldColumn::Computed,
                    DesignerFieldColumn::Key,
                    DesignerFieldColumn::Comment,
                    DesignerFieldColumn::Collation,
                    DesignerFieldColumn::Sparse,
                ],
                option_fields: Vec::new(),
                default_suggestions: vec![
                    DefaultSuggestion::for_types(
                        "GETDATE()",
                        &["datetime", "datetime2", "smalldatetime", "date", "time"],
                    ),
                    DefaultSuggestion::for_types(
                        "SYSDATETIME()",
                        &["datetime2", "datetimeoffset", "date", "time"],
                    ),
                    DefaultSuggestion::for_types(
                        "CURRENT_TIMESTAMP",
                        &["datetime", "datetime2", "smalldatetime"],
                    ),
                    DefaultSuggestion::for_types("NEWID()", &["uniqueidentifier"]),
                    DefaultSuggestion::for_types(
                        "0",
                        &[
                            "tinyint",
                            "smallint",
                            "int",
                            "bigint",
                            "decimal",
                            "numeric",
                            "money",
                            "smallmoney",
                            "float",
                            "real",
                            "bit",
                        ],
                    ),
                    DefaultSuggestion::for_types(
                        "''",
                        &[
                            "char", "varchar", "nchar", "nvarchar", "text", "ntext", "sysname",
                        ],
                    ),
                    DefaultSuggestion::for_types("1", &["bit"]),
                    DefaultSuggestion::every("NULL"),
                ],
                // SQL Server has at most one `IDENTITY` column per table, only a numeric one, and
                // `ALTER COLUMN` cannot give an existing column an `IDENTITY`: it is offered on a
                // new column (or a new table) and shown read-only on one the table already has.
                auto_increment: AutoIncrementRule {
                    types: vec!["tinyint", "smallint", "int", "bigint", "decimal", "numeric"],
                    one_per_table: true,
                    change_existing: false,
                },
                // SQL Server indexes sort per column; 子部分 is MySQL's.
                index_columns: vec![DesignerIndexColumn::Sort],
                // SQL Server's own kinds: an index is clustered or not, and either may be unique
                // (`CREATE [UNIQUE] [CLUSTERED|NONCLUSTERED] INDEX`). It has no index comment.
                index_kinds: vec![
                    DesignerIndexKind {
                        label: "NONCLUSTERED",
                        unique: false,
                        types: Some(&["NONCLUSTERED"]),
                    },
                    DesignerIndexKind {
                        label: "UNIQUE NONCLUSTERED",
                        unique: true,
                        types: Some(&["NONCLUSTERED"]),
                    },
                    DesignerIndexKind {
                        label: "CLUSTERED",
                        unique: false,
                        types: Some(&["CLUSTERED"]),
                    },
                    DesignerIndexKind {
                        label: "UNIQUE CLUSTERED",
                        unique: true,
                        types: Some(&["CLUSTERED"]),
                    },
                ],
                // `MS_Description` on the index's `sys.extended_properties` row.
                index_comment: true,
                // `sys.sql_modules.definition` is the complete `CREATE TRIGGER` statement.
                trigger_definition: TriggerDefinition::Full,
            },
            id: DriverId::new("sqlserver"),
            display_name: "SQL Server".to_string(),
            default_port: 1433,
            is_file_based: false,
            icon: "icons/sqlserver.svg",
            icon_style: DriverIconStyle::Brand(0xcc2927),
            capabilities: DriverCapabilities::none()
                .with(DriverCapability::DatabaseManagement)
                .with(DriverCapability::Users)
                .with(DriverCapability::Routines)
                .with(DriverCapability::Schemas),
            database_editor: self.database_editor(),
            connection_form: ConnectionFormSpec {
                // 常规 + TLS + 隧道 + 高级. SQL Server's own field is 验证 on 常规 plus the
                // application name on 高级; both option keys are honored by `connect` below. There
                // is no 命名实例 field: SQL Browser resolution is not compiled in, so a named
                // instance is reached through its real TCP port (the 端口 field). Navicat's Active
                // Directory modes need an MSAL/OAuth flow tiberius does not provide.
                home: ConnectionHomePage::Network,
                tabs: vec![
                    ConnectionPage::Tls,
                    ConnectionPage::Tunnel,
                    ConnectionPage::Advanced,
                ],
                optional_fields: Vec::new(),
                labels: vec![ConnectionFieldLabel {
                    field: ConnectionStandardField::Database,
                    label_key: "form.sqlserver.database",
                    hint_key: None,
                    visible_when: None,
                }],
                options: vec![
                    ConnectionFieldSpec {
                        key: "sqlserver.authentication",
                        label_key: "form.sqlserver.authentication",
                        hint_key: None,
                        kind: ConnectionFieldKind::Select(authentication_choices()),
                        page: ConnectionPage::General,
                        visible_when: None,
                    },
                    // SQL Server handshakes TLS for the login packet even with TLS/SSL off. Some
                    // servers (old builds, or a network path that cuts the handshake) close it,
                    // which surfaces as `tls handshake eof`; this lets the profile skip the
                    // handshake entirely. Ignored while TLS/SSL is on.
                    ConnectionFieldSpec {
                        key: "sqlserver.encryption",
                        label_key: "form.sqlserver.encryption",
                        hint_key: Some("form.sqlserver.encryption_hint"),
                        kind: ConnectionFieldKind::Select(vec![
                            ConnectionFieldChoice {
                                value: "login",
                                label_key: "form.sqlserver.encryption.login",
                            },
                            ConnectionFieldChoice {
                                value: "none",
                                label_key: "form.sqlserver.encryption.none",
                            },
                        ]),
                        page: ConnectionPage::Advanced,
                        visible_when: None,
                    },
                    ConnectionFieldSpec {
                        key: "sqlserver.application_name",
                        label_key: "form.sqlserver.application_name",
                        hint_key: Some("form.sqlserver.application_name_hint"),
                        kind: ConnectionFieldKind::Text,
                        page: ConnectionPage::Advanced,
                        visible_when: None,
                    },
                ],
                // SQL Server honors every standard 高级 field.
                advanced: AdvancedFields::ALL,
            },
            order: 50,
        }
    }

    fn dialect(&self) -> DriverDialect {
        DriverDialect::SqlServer
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

        let (host, instance) = split_host(&host);

        let mut server = Config::new();
        server.host(host);
        server.port(port.max(1));
        server.application_name(
            option(config, "sqlserver.application_name").unwrap_or_else(|| "RustGrid".to_string()),
        );
        // `host\instance` names the instance for the prelogin handshake. SQL Browser resolution
        // is not compiled in, so a named instance must still be reached through its real TCP port.
        if let Some(instance) = instance {
            server.instance_name(instance);
        }
        if let Some(database) = config.database.as_deref().filter(|value| !value.is_empty()) {
            server.database(database);
        }
        let user = config.username.as_str();
        let password = config.password.as_deref().unwrap_or("");
        server.authentication(
            if option(config, "sqlserver.authentication").as_deref() == Some("windows") {
                windows_authentication(user, password)
            } else {
                AuthMethod::sql_server(user, password)
            },
        );
        // SQL Server encrypts the login packet with TLS unless `ENCRYPT_NOT_SUP` is advertised;
        // the profile may opt out of that handshake while TLS/SSL itself is off.
        let skip_login_encryption = settings.tls.mode == TlsMode::Disabled
            && option(config, "sqlserver.encryption").as_deref() == Some("none");
        server.encryption(encryption_level(settings.tls.mode, skip_login_encryption));
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
        Ok(Box::new(SqlServerConnection::new(pool, tunnel)))
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

/// The authentication choices the connection form offers. `windows` is only offered where a
/// Windows-auth backend is compiled in (see [`windows_authentication`]).
fn authentication_choices() -> Vec<ConnectionFieldChoice> {
    let mut choices = vec![ConnectionFieldChoice {
        value: "sqlserver",
        label_key: "form.sqlserver.auth.sqlserver",
    }];
    if cfg!(windows) {
        choices.push(ConnectionFieldChoice {
            value: "windows",
            label_key: "form.sqlserver.auth.windows",
        });
    }
    choices
}

/// A trimmed non-empty profile option.
fn option(config: &ConnectionConfig, key: &str) -> Option<String> {
    config
        .options
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Windows (SSPI) authentication with an explicit Windows account. tiberius exposes
/// `AuthMethod::Windows` through its Windows-only `winauth` feature, enabled per target in this
/// crate's `Cargo.toml`.
#[cfg(windows)]
fn windows_authentication(user: &str, password: &str) -> AuthMethod {
    AuthMethod::windows(user, password)
}

/// Fallback for a platform with no Windows-auth backend. tiberius' Unix backend (`sspi-rs`)
/// conflicts with the workspace's `russh` dependency, so Unix has no Windows authentication; the
/// form does not offer it there (`descriptor()` adds the choice only on Windows). This path exists
/// for a hand-edited profile that names it anyway.
#[cfg(not(windows))]
fn windows_authentication(user: &str, password: &str) -> AuthMethod {
    AuthMethod::sql_server(user, password)
}

/// Map a tiberius error to a core error, flagging login failures as authentication errors.
pub(crate) fn map_connect_error(error: tiberius::error::Error) -> Error {
    if is_authentication_error(&error) {
        Error::Authentication(error.to_string())
    } else if is_tls_handshake_error(&error) {
        Error::Connection(format!(
            "{error}. The server's TLS is older than 1.2 (or the connection cut the handshake). \
             Enable TLS 1.2 on the server, or set Encryption (Advanced) to No encryption — the \
             login is then sent in clear text."
        ))
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

/// The server cut the TLS handshake that wraps the login packet. rustls reports this as an
/// unexpected EOF ("tls handshake eof"); it is almost always a server whose TLS version is older
/// than the client supports (rustls requires TLS 1.2+).
fn is_tls_handshake_error(error: &tiberius::error::Error) -> bool {
    matches!(
        error,
        tiberius::error::Error::Io {
            kind: std::io::ErrorKind::UnexpectedEof,
            message,
        } if message.contains("tls handshake")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every declared field maps to a `sqlserver.*` option the connect path reads.
    #[test]
    fn declared_connection_fields_are_honored_options() {
        let descriptor = SqlServerDriver::new().descriptor();
        let form = &descriptor.connection_form;

        let keys: Vec<_> = form.options.iter().map(|field| field.key).collect();
        assert_eq!(
            keys,
            vec![
                "sqlserver.authentication",
                "sqlserver.encryption",
                "sqlserver.application_name",
            ]
        );

        let ConnectionFieldKind::Select(choices) = &form.options[0].kind else {
            panic!("authentication must be a select");
        };
        assert_eq!(
            choices.first().map(|choice| choice.value),
            Some("sqlserver")
        );
        // Windows authentication is only offered where its backend is compiled in.
        assert_eq!(
            choices.iter().any(|choice| choice.value == "windows"),
            cfg!(windows)
        );
        // Encryption chooses between the login-only handshake (default) and no encryption at all.
        let ConnectionFieldKind::Select(encryption) = &form.options[1].kind else {
            panic!("encryption must be a select");
        };
        assert_eq!(
            encryption
                .iter()
                .map(|choice| choice.value)
                .collect::<Vec<_>>(),
            vec!["login", "none"]
        );
        // 验证 and the application name's page placement; the encryption option joins 高级.
        assert_eq!(form.options[0].page, ConnectionPage::General);
        assert_eq!(form.options[1].page, ConnectionPage::Advanced);
        assert_eq!(form.options[2].page, ConnectionPage::Advanced);
        assert_eq!(
            form.labels.first().map(|label| label.field),
            Some(ConnectionStandardField::Database)
        );
    }

    #[test]
    fn tls_handshake_eof_gets_an_actionable_message() {
        let error = tiberius::error::Error::Io {
            kind: std::io::ErrorKind::UnexpectedEof,
            message: "tls handshake eof".to_string(),
        };
        let Error::Connection(message) = map_connect_error(error) else {
            panic!("a handshake failure is a connection error");
        };
        assert!(message.contains("tls handshake eof"), "{message}");
        assert!(message.contains("Encryption"), "{message}");
        assert!(message.contains("TLS 1.2"), "{message}");
    }

    #[test]
    fn splits_host_instance() {
        assert_eq!(
            split_host("db\\SQLEXPRESS"),
            ("db".to_string(), Some("SQLEXPRESS".to_string()))
        );
        assert_eq!(split_host("db"), ("db".to_string(), None));
        assert_eq!(split_host("db\\"), ("db\\".to_string(), None));
    }
}
