use async_trait::async_trait;
use rustgrid_core::{
    AdvancedFields, Connection, ConnectionConfig, ConnectionFieldKind, ConnectionFieldLabel,
    ConnectionFieldPlacement, ConnectionFieldSpec, ConnectionFormSpec, ConnectionHomePage,
    ConnectionPage, ConnectionStandardField, Driver, DriverCapabilities, DriverCapability,
    DriverDescriptor, DriverIconStyle, DriverId, Result, TableDesignerSpec,
};

use crate::connection::RedisConnection;

/// How the client reaches the server.
pub const OPT_MODE: &str = "redis.mode";
/// The master name registered with the sentinels (Sentinel mode).
pub const OPT_SENTINEL_MASTER: &str = "redis.sentinel_master";
/// `host:port` sentinel addresses, comma separated (Sentinel mode).
pub const OPT_SENTINEL_HOSTS: &str = "redis.sentinel_hosts";
/// The password the sentinels themselves require, when any (Sentinel mode).
pub const OPT_SENTINEL_PASSWORD: &str = "redis.sentinel_password";
/// `host:port` cluster seed nodes, comma separated (Cluster mode). Empty uses the main host/port.
pub const OPT_CLUSTER_NODES: &str = "redis.cluster_nodes";
/// How many keys a database lists at most (SCAN stops there).
pub const OPT_KEY_LIMIT: &str = "redis.key_limit";

/// The default key-list cap. Redis keyspaces can hold millions of keys; listing them all would
/// freeze the tree, so `SCAN` stops once the cap is reached.
pub const DEFAULT_KEY_LIMIT: usize = 1000;

/// The Redis driver.
#[derive(Debug, Default)]
pub struct RedisDriver;

impl RedisDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Driver for RedisDriver {
    fn id(&self) -> DriverId {
        DriverId::new("redis")
    }

    fn display_name(&self) -> String {
        "Redis".to_string()
    }

    fn default_port(&self) -> u16 {
        6379
    }

    fn supports_database_management(&self) -> bool {
        // The numbered databases exist or not per server configuration; they are selected, not
        // created. (CREATE/DROP DATABASE has no Redis equivalent for `db0`..`dbN`.)
        false
    }

    fn supports_users(&self) -> bool {
        // ACL users exist from Redis 6, but the shared Users editor models SQL-style accounts and
        // privileges; the Redis ACL sections are a follow-up.
        false
    }

    fn supports_routines(&self) -> bool {
        false
    }

    fn supports_schemas(&self) -> bool {
        false
    }

    fn supports_table_designer(&self, _profile: &rustgrid_core::ConnectionProfile) -> bool {
        // There is no table DDL: a "table" is a key, and its type cannot be altered.
        false
    }

    fn supports_backup(&self, _profile: &rustgrid_core::ConnectionProfile) -> bool {
        // The Backup tab writes SQL and replays it; Redis keys are restored from their own values,
        // which is a different pipeline.
        false
    }

    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            id: self.id(),
            display_name: self.display_name(),
            default_port: self.default_port(),
            is_file_based: false,
            icon: "icons/redis.svg",
            // Redis red, the same brand colour the connection badge uses.
            icon_style: DriverIconStyle::Brand(0xDC382D),
            capabilities: DriverCapabilities::none().with(DriverCapability::KeyValue),
            database_editor: Default::default(),
            connection_form: ConnectionFormSpec {
                home: ConnectionHomePage::Network,
                tabs: vec![
                    ConnectionPage::Tls,
                    ConnectionPage::Tunnel,
                    ConnectionPage::Advanced,
                ],
                // A Redis user name is an ACL account (Redis 6+); password-only AUTH and the
                // `default` user need no user name, so an empty field is valid.
                optional_fields: vec![ConnectionStandardField::Username],
                labels: vec![
                    ConnectionFieldLabel {
                        field: ConnectionStandardField::Database,
                        label_key: "redis.database",
                        hint_key: Some("redis.database.hint"),
                        // A cluster has a single keyspace (db0); only standalone and sentinel
                        // servers expose numbered databases.
                        visible_when: Some((OPT_MODE, &["standalone", "sentinel"])),
                        required: false,
                    },
                    ConnectionFieldLabel {
                        field: ConnectionStandardField::Username,
                        label_key: "form.username",
                        hint_key: Some("redis.username.hint"),
                        visible_when: None,
                        required: false,
                    },
                ],
                options: vec![
                    ConnectionFieldSpec {
                        key: OPT_MODE,
                        label_key: "redis.type",
                        hint_key: None,
                        kind: ConnectionFieldKind::Select(vec![
                            rustgrid_core::ConnectionFieldChoice {
                                value: "standalone",
                                label_key: "redis.type.standalone",
                            },
                            rustgrid_core::ConnectionFieldChoice {
                                value: "sentinel",
                                label_key: "redis.type.sentinel",
                            },
                            rustgrid_core::ConnectionFieldChoice {
                                value: "cluster",
                                label_key: "redis.type.cluster",
                            },
                        ]),
                        page: ConnectionPage::General,
                        visible_when: None,
                        placement: ConnectionFieldPlacement::Trailing,
                        required: false,
                    },
                    ConnectionFieldSpec {
                        key: OPT_SENTINEL_MASTER,
                        label_key: "redis.sentinel.master",
                        hint_key: Some("redis.sentinel.master.hint"),
                        kind: ConnectionFieldKind::Text,
                        page: ConnectionPage::General,
                        visible_when: Some((OPT_MODE, "sentinel")),
                        placement: ConnectionFieldPlacement::Trailing,
                        required: false,
                    },
                    ConnectionFieldSpec {
                        key: OPT_SENTINEL_HOSTS,
                        label_key: "redis.sentinel.hosts",
                        hint_key: Some("redis.sentinel.hosts.hint"),
                        kind: ConnectionFieldKind::Text,
                        page: ConnectionPage::General,
                        visible_when: Some((OPT_MODE, "sentinel")),
                        placement: ConnectionFieldPlacement::Trailing,
                        required: false,
                    },
                    ConnectionFieldSpec {
                        key: OPT_SENTINEL_PASSWORD,
                        label_key: "redis.sentinel.password",
                        hint_key: None,
                        kind: ConnectionFieldKind::Secret,
                        page: ConnectionPage::General,
                        visible_when: Some((OPT_MODE, "sentinel")),
                        placement: ConnectionFieldPlacement::Trailing,
                        required: false,
                    },
                    ConnectionFieldSpec {
                        key: OPT_CLUSTER_NODES,
                        label_key: "redis.cluster.nodes",
                        hint_key: Some("redis.cluster.nodes.hint"),
                        kind: ConnectionFieldKind::Text,
                        page: ConnectionPage::General,
                        visible_when: Some((OPT_MODE, "cluster")),
                        placement: ConnectionFieldPlacement::Trailing,
                        required: false,
                    },
                    ConnectionFieldSpec {
                        key: OPT_KEY_LIMIT,
                        label_key: "redis.key_limit",
                        hint_key: Some("redis.key_limit.hint"),
                        kind: ConnectionFieldKind::Text,
                        page: ConnectionPage::Advanced,
                        visible_when: None,
                        placement: ConnectionFieldPlacement::Trailing,
                        required: false,
                    },
                ],
                advanced: AdvancedFields {
                    connect_timeout: true,
                    query_timeout: true,
                    // redis-rs has no TCP keep-alive knob and Redis has no session init SQL.
                    keepalive: false,
                    read_only: true,
                    init_sql: false,
                },
            },
            table_designer: TableDesignerSpec::default(),
            order: 65,
        }
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        let connection = RedisConnection::open(config).await?;
        Ok(Box::new(connection))
    }
}
