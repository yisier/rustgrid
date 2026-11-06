//! The MongoDB driver: descriptor, connection form and connection factory.

use async_trait::async_trait;
use rustgrid_core::{
    Connection, ConnectionConfig, ConnectionFieldKind, ConnectionFieldLabel, ConnectionFieldSpec,
    ConnectionFormSpec, ConnectionHomePage, ConnectionPage, ConnectionStandardField, Driver,
    DriverCapabilities, DriverCapability, DriverDescriptor, DriverIconStyle, DriverId, Result,
};

use crate::connection::MongoConnection;

/// The `ConnectionProfile::options` key of the authentication database.
pub const OPT_AUTH_SOURCE: &str = "mongodb.auth_source";
/// The `ConnectionProfile::options` key of the authentication mechanism (`scram` / `none`).
pub const OPT_AUTH_MECHANISM: &str = "mongodb.auth_mechanism";
/// The `ConnectionProfile::options` key of the replica-set name.
pub const OPT_REPLICA_SET: &str = "mongodb.replica_set";
/// The `ConnectionProfile::options` key of the read preference.
pub const OPT_READ_PREFERENCE: &str = "mongodb.read_preference";

/// The MongoDB driver. A connection is a document store: databases hold collections, a collection
/// is shown as a table, and one document is one grid row.
#[derive(Default)]
pub struct MongoDriver;

impl MongoDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Driver for MongoDriver {
    fn id(&self) -> DriverId {
        DriverId::new("mongodb")
    }

    fn display_name(&self) -> String {
        "MongoDB".to_string()
    }

    fn default_port(&self) -> u16 {
        27017
    }

    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            id: self.id(),
            display_name: self.display_name(),
            default_port: self.default_port(),
            is_file_based: false,
            icon: "icons/mongodb.svg",
            icon_style: DriverIconStyle::Brand(0x13AA52),
            capabilities: DriverCapabilities::none()
                .with(DriverCapability::DatabaseManagement)
                .with(DriverCapability::Documents),
            database_editor: Default::default(),
            connection_form: ConnectionFormSpec {
                home: ConnectionHomePage::Network,
                tabs: vec![
                    ConnectionPage::Tls,
                    ConnectionPage::Tunnel,
                    ConnectionPage::Advanced,
                ],
                optional_fields: Vec::new(),
                labels: vec![ConnectionFieldLabel {
                    field: ConnectionStandardField::Database,
                    label_key: "mongodb.database",
                    hint_key: Some("mongodb.database.hint"),
                    visible_when: None,
                }],
                options: vec![
                    ConnectionFieldSpec {
                        key: OPT_AUTH_SOURCE,
                        label_key: "mongodb.auth_source",
                        hint_key: Some("mongodb.auth_source.hint"),
                        kind: ConnectionFieldKind::Text,
                        page: ConnectionPage::General,
                        visible_when: None,
                    },
                    ConnectionFieldSpec {
                        key: OPT_AUTH_MECHANISM,
                        label_key: "mongodb.auth_mechanism",
                        hint_key: Some("mongodb.auth_mechanism.hint"),
                        kind: ConnectionFieldKind::Select(vec![
                            rustgrid_core::ConnectionFieldChoice {
                                value: "scram",
                                label_key: "mongodb.auth_mechanism.scram",
                            },
                            rustgrid_core::ConnectionFieldChoice {
                                value: "none",
                                label_key: "mongodb.auth_mechanism.none",
                            },
                        ]),
                        page: ConnectionPage::General,
                        visible_when: None,
                    },
                    ConnectionFieldSpec {
                        key: OPT_REPLICA_SET,
                        label_key: "mongodb.replica_set",
                        hint_key: Some("mongodb.replica_set.hint"),
                        kind: ConnectionFieldKind::Text,
                        page: ConnectionPage::Advanced,
                        visible_when: None,
                    },
                    ConnectionFieldSpec {
                        key: OPT_READ_PREFERENCE,
                        label_key: "mongodb.read_preference",
                        hint_key: Some("mongodb.read_preference.hint"),
                        kind: ConnectionFieldKind::Select(vec![
                            rustgrid_core::ConnectionFieldChoice {
                                value: "primary",
                                label_key: "mongodb.read_preference.primary",
                            },
                            rustgrid_core::ConnectionFieldChoice {
                                value: "primaryPreferred",
                                label_key: "mongodb.read_preference.primary_preferred",
                            },
                            rustgrid_core::ConnectionFieldChoice {
                                value: "secondary",
                                label_key: "mongodb.read_preference.secondary",
                            },
                            rustgrid_core::ConnectionFieldChoice {
                                value: "secondaryPreferred",
                                label_key: "mongodb.read_preference.secondary_preferred",
                            },
                            rustgrid_core::ConnectionFieldChoice {
                                value: "nearest",
                                label_key: "mongodb.read_preference.nearest",
                            },
                        ]),
                        page: ConnectionPage::Advanced,
                        visible_when: None,
                    },
                ],
                advanced: rustgrid_core::descriptor::AdvancedFields {
                    connect_timeout: true,
                    query_timeout: false,
                    keepalive: false,
                    read_only: true,
                    init_sql: false,
                },
            },
            table_designer: Default::default(),
            order: 70,
        }
    }

    /// A collection has no relational schema for the SQL table designer; collections are created
    /// and altered through JSON commands until a document-specific designer exists.
    fn supports_table_designer(&self, _profile: &rustgrid_core::ConnectionProfile) -> bool {
        false
    }

    /// `.rgbak` stores SQL DDL and value tuples, which a document store cannot replay.
    fn supports_backup(&self, _profile: &rustgrid_core::ConnectionProfile) -> bool {
        false
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        let connection = MongoConnection::open(
            &config.host,
            config.port,
            &config.username,
            config.password.as_deref(),
            config.database.as_deref().unwrap_or(""),
            &config.settings,
            &config.options,
        )
        .await?;
        Ok(Box::new(connection))
    }
}
