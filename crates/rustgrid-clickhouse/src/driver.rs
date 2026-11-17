//! The ClickHouse driver: descriptor, connection form and connection factory.

use async_trait::async_trait;
use rustgrid_core::{
    AdvancedFields, AutoIncrementRule, Connection, ConnectionConfig, ConnectionFieldLabel,
    ConnectionFormSpec, ConnectionHomePage, ConnectionPage, ConnectionStandardField,
    DatabaseEditorSpec, DefaultSuggestion, DesignerColumnOption, DesignerFieldColumn, DesignerTab,
    Driver, DriverCapabilities, DriverCapability, DriverDescriptor, DriverDialect, DriverIconStyle,
    DriverId, Result, TableDesignerSpec, TriggerDefinition,
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
            // ClickHouse has no secondary indexes, foreign keys or identity columns, and the
            // designer does not edit MergeTree's engine/sorting key/partition clauses. It does
            // cover the fields, their types/defaults/nullability/comments and the table comment.
            table_designer: TableDesignerSpec {
                tabs: vec![DesignerTab::Fields, DesignerTab::Comment, DesignerTab::Sql],
                column_options: vec![DesignerColumnOption::Default],
                field_columns: vec![DesignerFieldColumn::NotNull, DesignerFieldColumn::Comment],
                option_fields: Vec::new(),
                default_suggestions: vec![
                    DefaultSuggestion::for_types("now()", &["DateTime", "DateTime64"]),
                    DefaultSuggestion::for_types("today()", &["Date", "Date32"]),
                    DefaultSuggestion::for_types("''", &["String", "FixedString"]),
                    DefaultSuggestion::for_types(
                        "0",
                        &[
                            "Int8",
                            "Int16",
                            "Int32",
                            "Int64",
                            "Int128",
                            "Int256",
                            "UInt8",
                            "UInt16",
                            "UInt32",
                            "UInt64",
                            "UInt128",
                            "UInt256",
                            "Float32",
                            "Float64",
                            "Decimal",
                            "Decimal32",
                            "Decimal64",
                            "Decimal128",
                            "Decimal256",
                        ],
                    ),
                    DefaultSuggestion::for_types("false", &["Bool"]),
                    DefaultSuggestion::every("NULL"),
                ],
                index_columns: Vec::new(),
                index_kinds: Vec::new(),
                index_comment: false,
                auto_increment: AutoIncrementRule::default(),
                trigger_definition: TriggerDefinition::Full,
            },
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
            order: 55,
        }
    }

    /// ClickHouse tables can be created and redesigned in the shared designer (columns, defaults
    /// and comments). MergeTree's engine/sorting key/partition clauses are not editable yet, and a
    /// read-only connection cannot change the structure at all.
    fn supports_table_designer(&self, profile: &rustgrid_core::ConnectionProfile) -> bool {
        !profile.settings.read_only
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
