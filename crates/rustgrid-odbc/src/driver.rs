use async_trait::async_trait;
use rustgrid_core::{
    Connection, ConnectionConfig, ConnectionFormSpec, ConnectionHomePage, DatabaseEditorSpec,
    Driver, DriverCapabilities, DriverDescriptor, DriverDialect, DriverIconStyle, DriverId, Error,
    Result,
};

use crate::api;
use crate::connection::OdbcConnection;

/// The generic ODBC driver. It connects through a user-installed ODBC driver (or a raw connection
/// string) and speaks the engine's SQL, so it is the fallback for engines RustGrid has no native
/// Rust driver for.
#[derive(Debug, Default)]
pub struct OdbcDriver;

impl OdbcDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Driver for OdbcDriver {
    fn id(&self) -> DriverId {
        DriverId::new("odbc")
    }

    fn display_name(&self) -> String {
        "ODBC".to_string()
    }

    fn default_port(&self) -> u16 {
        0
    }

    fn is_file_based(&self) -> bool {
        false
    }

    // The capabilities below are engine-dependent for ODBC; the conservative defaults keep the
    // engine-specific tabs (Users/Routines/schema management) hidden.
    fn supports_database_management(&self) -> bool {
        false
    }

    fn supports_users(&self) -> bool {
        false
    }

    fn supports_routines(&self) -> bool {
        false
    }

    fn supports_schemas(&self) -> bool {
        false
    }

    fn database_editor(&self) -> DatabaseEditorSpec {
        DatabaseEditorSpec::default()
    }

    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            id: DriverId::new("odbc"),
            display_name: "ODBC".to_string(),
            default_port: 0,
            is_file_based: false,
            icon: "icons/connection.svg",
            icon_style: DriverIconStyle::Plain,
            capabilities: DriverCapabilities::none(),
            database_editor: DatabaseEditorSpec::default(),
            connection_form: ConnectionFormSpec {
                // The generic ODBC driver is the one engine whose form IS the plain form: the
                // ODBC fields on the 常规 page, nothing engine-specific beyond that.
                home: ConnectionHomePage::Odbc,
                ..Default::default()
            },
            order: 80,
        }
    }

    fn dialect(&self) -> DriverDialect {
        DriverDialect::Generic
    }

    fn connection_drivers(&self) -> Vec<String> {
        api::api()
            .ok()
            .and_then(|api| api.driver_descriptions().ok())
            .unwrap_or_default()
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        let connection_string = connection_string(config)?;
        let api = api::api().map_err(Error::Connection)?;
        let handle = api
            .connect(&connection_string)
            .map_err(classify_connect_error)?;
        Ok(Box::new(OdbcConnection::new(handle, config)))
    }
}

/// Build the ODBC connection string from a profile. A full connection string wins; otherwise a DSN
/// plus credentials, or a driver name plus the profile's server/credentials.
fn connection_string(config: &ConnectionConfig) -> Result<String> {
    if let Some(value) = config
        .options
        .get("odbc.connection_string")
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(value.clone());
    }

    let password = escape_attribute_value(config.password.as_deref().unwrap_or(""));

    if let Some(dsn) = config
        .options
        .get("odbc.dsn")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return Ok(format!(
            "DSN={dsn};UID={};PWD={password};",
            config.username.trim()
        ));
    }

    let driver = config
        .options
        .get("odbc.driver")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let Some(driver) = driver else {
        return Err(Error::Connection(
            "an ODBC driver name or connection string is required".to_string(),
        ));
    };

    let mut parts = vec![format!("Driver={{{driver}}}")];
    if !config.host.trim().is_empty() {
        let server = if config.port > 0 {
            format!("{},{}", config.host.trim(), config.port)
        } else {
            config.host.trim().to_string()
        };
        parts.push(format!("Server={server}"));
    }
    if !config.username.trim().is_empty() {
        parts.push(format!("UID={}", config.username.trim()));
    }
    parts.push(format!("PWD={password}"));
    if let Some(database) = config
        .database
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!("Database={}", database.trim()));
    }
    Ok(parts.join(";") + ";")
}

/// Escape an ODBC connection-string attribute value (a `}` inside a `{...}` value is doubled).
fn escape_attribute_value(value: &str) -> String {
    value.replace('}', "}}")
}

/// Turn a driver-manager error message into an authentication error when it looks like a login
/// failure, so the app offers the password prompt instead of a generic connection error.
fn classify_connect_error(message: String) -> Error {
    let lower = message.to_lowercase();
    if message.contains("28000")
        || lower.contains("login failed")
        || lower.contains("authentication")
        || lower.contains("access denied")
    {
        Error::Authentication(message)
    } else {
        Error::Connection(message)
    }
}
