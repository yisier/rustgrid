//! The ClickHouse driver: descriptor, connection form and connection factory.

use async_trait::async_trait;
use rustgrid_core::{
    AdvancedFields, Connection, ConnectionConfig, ConnectionFieldLabel, ConnectionFormSpec,
    ConnectionHomePage, ConnectionPage, ConnectionStandardField, DatabaseEditorSpec, Driver,
    DriverCapabilities, DriverCapability, DriverDescriptor, DriverDialect, DriverIconStyle,
    DriverId, Result,
};

use crate::connection::ClickHouseConnection;

/// The ClickHouse driver. A connection browses databases and tables over the HTTP interface
/// (default port 8123); the data grid is read-only and writes go through the SQL editor.
#[derive(Debug, Default)]
pub struct ClickHouseDriver;

impl ClickHouseDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Driver for ClickHouseDriver {
    fn id(&self) -> DriverId {
        DriverId::new("clickhouse")
    }

    fn display_name(&self) -> String {
        "ClickHouse".to_string()
    }

    fn default_port(&self) -> u16 {
        // The HTTP interface. TLS (8443) and the native protocol (9000) are not defaults: the
        // driver speaks HTTP, and TLS is selected by the connection's 使用 TLS/SSL switch.
        8123
    }

    /// Every ClickHouse server ships the `default` account, which password-less installs use.
    fn default_username(&self) -> &'static str {
        "default"
    }

    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            id: self.id(),
            display_name: self.display_name(),
            default_port: self.default_port(),
            is_file_based: false,
            icon: "icons/clickhouse.svg",
            icon_style: DriverIconStyle::Brand(0xFFCC01),
            capabilities: DriverCapabilities::none()
                // CREATE/DROP DATABASE are plain statements; ClickHouse has no editable database
                // options (charset/collation), so the dialog only asks for a name.
                .with(DriverCapability::DatabaseManagement)
                // A MergeTree table has no unique key; `ALTER … UPDATE/DELETE` is an asynchronous
                // mutation that may touch several rows, so grid edits are not offered.
                .with(DriverCapability::ReadOnlyGrid),
            database_editor: self.database_editor(),
            connection_form: ConnectionFormSpec {
                home: ConnectionHomePage::Network,
                tabs: vec![
                    ConnectionPage::Tls,
                    ConnectionPage::Tunnel,
                    ConnectionPage::Advanced,
                ],
                // The database is the server's default context; a blank field means `default`.
                optional_fields: vec![ConnectionStandardField::Database],
                labels: vec![ConnectionFieldLabel {
                    field: ConnectionStandardField::Database,
                    label_key: "clickhouse.database",
                    hint_key: Some("clickhouse.database.hint"),
                    visible_when: None,
                }],
                options: Vec::new(),
                advanced: AdvancedFields {
                    connect_timeout: true,
                    query_timeout: true,
                    keepalive: true,
                    read_only: true,
                    // ClickHouse's HTTP interface is stateless: a session initialisation statement
                    // would only affect its own request, so the field is not offered.
                    init_sql: false,
                },
            },
            table_designer: Default::default(),
            order: 55,
        }
    }

    /// The table designer would have to model MergeTree's engine, `PARTITION BY`, `ORDER BY`, `TTL`
    /// and the sampling/settings clauses; it is a later phase.
    fn supports_table_designer(&self, _profile: &rustgrid_core::ConnectionProfile) -> bool {
        false
    }

    /// `.rgbak` stores SQL DDL and value tuples; ClickHouse tables are recreated from their
    /// `CREATE TABLE` in the SQL editor instead.
    fn supports_backup(&self, _profile: &rustgrid_core::ConnectionProfile) -> bool {
        false
    }

    /// ClickHouse databases carry no charset, collation, owner or recovery model.
    fn database_editor(&self) -> DatabaseEditorSpec {
        DatabaseEditorSpec::default()
    }

    fn dialect(&self) -> DriverDialect {
        DriverDialect::ClickHouse
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        let connection = ClickHouseConnection::open(
            &config.host,
            config.port,
            &config.username,
            config.password.as_deref(),
            config.database.as_deref().unwrap_or(""),
            &config.settings,
        )
        .await?;
        Ok(Box::new(connection))
    }
}
