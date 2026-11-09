use async_trait::async_trait;
use rustgrid_core::{
    AdvancedFields, Connection, ConnectionConfig, ConnectionFormSpec, ConnectionHomePage,
    ConnectionProfile, DatabaseEditorSpec, Driver, DriverCapabilities, DriverDescriptor,
    DriverDialect, DriverIconStyle, DriverId, Error, Result, TableDesignerSpec,
};

use crate::api;
use crate::connection::{OdbcConnection, detect_engine, dialect_for_engine};

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
            // The generic ODBC driver does not implement the table designer at all, so it keeps the
            // default shape; the panel is never opened for it.
            table_designer: TableDesignerSpec::default(),
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
                // ODBC fields on the 常规 page, nothing engine-specific beyond that. It reads no
                // TLS, tunnel or advanced settings (they all live inside its connection string),
                // so 常规 is the only page.
                home: ConnectionHomePage::Odbc,
                tabs: Vec::new(),
                advanced: AdvancedFields::NONE,
                ..Default::default()
            },
            order: 80,
        }
    }

    fn dialect(&self) -> DriverDialect {
        DriverDialect::Generic
    }

    fn dialect_for(&self, profile: &ConnectionProfile) -> DriverDialect {
        dialect_for_engine(&engine_hint(profile))
    }

    fn supports_table_designer(&self, _profile: &ConnectionProfile) -> bool {
        // The generic driver cannot reproduce a table's full definition (indexes, keys, engine
        // options) well enough to edit it, so the designer and 新建表 stay unavailable instead of
        // opening a page whose save would produce the wrong DDL. `object_ddl` still shows the
        // server's own DDL in the object info pane.
        false
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
    let engine = config
        .options
        .get("odbc.engine")
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();

    if let Some(value) = config
        .options
        .get("odbc.connection_string")
        .filter(|value| !value.trim().is_empty())
    {
        // A user-supplied string may come from Navicat/DBX without a character set. MySQL's older
        // Connector/ODBC releases default to `latin1`, which makes the *server* replace every
        // non-Latin character with `?`; the charset below keeps Chinese/Japanese text intact.
        return Ok(with_mysql_charset(
            value.clone(),
            is_mysql(value, &engine),
            value,
        ));
    }

    let password = escape_attribute_value(config.password.as_deref().unwrap_or(""));
    let driver_hint = config
        .options
        .get("odbc.driver")
        .map(|value| value.trim())
        .unwrap_or_default();

    if let Some(dsn) = config
        .options
        .get("odbc.dsn")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        let string = format!(
            "DSN={};UID={};PWD={password};",
            escape_attribute_value(dsn),
            escape_attribute_value(config.username.trim())
        );
        return Ok(with_mysql_charset(
            string,
            is_mysql(driver_hint, &engine),
            driver_hint,
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

    // The engine hint the user set is free text ("sqlserver"), so every match below is loose.
    let mysql = is_mysql(driver, &engine);
    let mut parts = vec![format!("Driver={{{}}}", driver.replace('}', "}}"))];
    let host = config.host.trim();
    if !host.is_empty() {
        // Where the port goes depends on the driver. SQL Server's ODBC drivers want it inside the
        // server attribute (`Server=host,port`); for every other driver a comma there means a
        // *list* — MySQL Connector/ODBC ≥ 8.0.28 reads `Server=host,port` as two hosts and refuses
        // it with "Missing option MULTI_HOST=1", and psqlODBC reads the number as a second host
        // name — so those get the port as its own attribute (`Server=host;Port=port`).
        let port_in_server = config.port > 0 && server_takes_port(driver, &engine);
        let server = if port_in_server {
            format!("{host},{}", config.port)
        } else {
            host.to_string()
        };
        parts.push(format!("Server={server}"));
        if config.port > 0 && !port_in_server {
            parts.push(format!("Port={}", config.port));
        }
        // A hand-typed multi-host list is what MySQL's driver needs to be told about explicitly.
        if !port_in_server && host.contains(',') && mysql {
            parts.push("MULTI_HOST=1".to_string());
        }
    }
    if !config.username.trim().is_empty() {
        parts.push(format!(
            "UID={}",
            escape_attribute_value(config.username.trim())
        ));
    }
    parts.push(format!("PWD={password}"));
    if let Some(database) = config
        .database
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!(
            "Database={}",
            escape_attribute_value(database.trim())
        ));
    }
    if mysql {
        parts.push(format!("CHARSET={}", mysql_charset(driver)));
    }
    Ok(parts.join(";") + ";")
}

/// Add MySQL's client character set when the connection string does not set one. Older
/// Connector/ODBC releases default to `latin1`; the server then converts a `utf8mb4` column to
/// Latin-1 and every non-Latin character arrives as `?`.
///
/// A **Unicode** driver converts to UTF-16 itself, so it gets full `utf8mb4`. An **ANSI** driver
/// returns bytes that the Windows driver manager converts with the system ANSI code page, so it
/// must ask the server for exactly that code page (`gbk` on a Chinese system); asking for UTF-8
/// bytes there would turn every Chinese character into mojibake.
fn with_mysql_charset(connection_string: String, mysql: bool, driver: &str) -> String {
    if !mysql || connection_string.to_ascii_lowercase().contains("charset") {
        return connection_string;
    }
    let mut string = connection_string.trim_end().to_string();
    if !string.ends_with(';') {
        string.push(';');
    }
    string.push_str(&format!("CHARSET={};", mysql_charset(driver)));
    string
}

/// The MySQL character set matching an ODBC driver's text encoding.
fn mysql_charset(driver: &str) -> &'static str {
    if driver.to_ascii_lowercase().contains("ansi") {
        ansi_charset()
    } else {
        "utf8mb4"
    }
}

/// The MySQL character set name for the system ANSI code page (the encoding a Windows ANSI ODBC
/// driver and the driver manager exchange text in).
#[cfg(target_os = "windows")]
fn ansi_charset() -> &'static str {
    // SAFETY: `GetACP` takes no arguments and has no preconditions.
    let code_page = unsafe { windows::Win32::Globalization::GetACP() };
    match code_page {
        936 => "gbk",
        54936 => "gb18030",
        950 => "big5",
        932 => "sjis",
        949 => "euckr",
        874 => "tis620",
        1250 => "latin2",
        1251 => "cp1251",
        1252 => "latin1",
        1253 => "greek",
        1254 => "latin5",
        1255 => "hebrew",
        1256 => "cp1256",
        1257 => "latin7",
        1258 => "cp1258",
        65001 => "utf8mb4",
        _ => "utf8mb4",
    }
}

/// Non-Windows driver managers pass bytes through unchanged and the locale is normally UTF-8.
#[cfg(not(target_os = "windows"))]
fn ansi_charset() -> &'static str {
    "utf8mb4"
}

/// The engine a saved ODBC profile reaches, inferred from the hint, the driver name or the
/// connection string. Used for the dialect and capability checks before a connection exists.
fn engine_hint(profile: &ConnectionProfile) -> String {
    if let Some(hint) = profile
        .options
        .get("odbc.engine")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return detect_engine(hint);
    }
    for key in ["odbc.driver", "odbc.dsn", "odbc.connection_string"] {
        if let Some(value) = profile.options.get(key) {
            let detected = detect_engine(value);
            if !detected.is_empty() {
                return detected;
            }
        }
    }
    String::new()
}

/// Escape an ODBC connection-string attribute value. A value containing the `;` separator (or
/// leading/trailing whitespace) must be wrapped in `{...}`, with any `}` doubled.
fn escape_attribute_value(value: &str) -> String {
    if value.contains(';') || value.contains('}') || value != value.trim() {
        format!("{{{}}}", value.replace('}', "}}"))
    } else {
        value.to_string()
    }
}

/// Whether the driver reads the port from the server attribute (`Server=host,port`), which is the
/// SQL Server ODBC drivers' convention. Every other driver takes the port as its own attribute.
fn server_takes_port(driver: &str, engine: &str) -> bool {
    let driver = driver.to_ascii_lowercase();
    [engine, driver.as_str()].iter().any(|name| {
        name.contains("sqlserver") || name.contains("sql server") || name.contains("mssql")
    })
}

/// Whether the driver is a MySQL/MariaDB one, whose `Server` may carry a comma-separated host list
/// only once `MULTI_HOST=1` is set (Connector/ODBC ≥ 8.0.28 rejects it otherwise).
fn is_mysql(driver: &str, engine: &str) -> bool {
    let driver = driver.to_ascii_lowercase();
    [engine, driver.as_str()]
        .iter()
        .any(|name| name.contains("mysql") || name.contains("mariadb"))
}

/// Turn a driver-manager error message into an authentication error when it looks like a login
/// failure, so the app offers the password prompt instead of a generic connection error.
fn classify_connect_error(message: String) -> Error {
    let lower = message.to_lowercase();
    if message.contains("28000")
        || message.contains("08S01")
        || lower.contains("login failed")
        || lower.contains("authentication")
        || lower.contains("access denied")
        || lower.contains("invalid username/password")
        || lower.contains("invalid authorization")
        || lower.contains("password authentication failed")
        || lower.contains("ora-01017")
        || lower.contains("not authorized")
        || lower.contains("logon denied")
    {
        Error::Authentication(message)
    } else {
        Error::Connection(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use rustgrid_core::ConnectionOptions;

    /// A profile that fills the form's fields (no DSN, no connection string) for `driver`.
    fn config(driver: &str, host: &str, port: u16, engine: &str) -> ConnectionConfig {
        let mut options = BTreeMap::from([("odbc.driver".to_string(), driver.to_string())]);
        if !engine.is_empty() {
            options.insert("odbc.engine".to_string(), engine.to_string());
        }
        ConnectionConfig {
            driver: DriverId::new("odbc"),
            host: host.to_string(),
            port,
            username: "root".to_string(),
            password: Some("secret".to_string()),
            database: Some("app".to_string()),
            options,
            settings: ConnectionOptions::default(),
        }
    }

    #[test]
    fn mysql_takes_the_port_as_its_own_attribute() {
        // MySQL Connector/ODBC reads `Server=host,port` as a two-host failover list and rejects it
        // with "Missing option MULTI_HOST=1", so the port must be its own attribute.
        let string = connection_string(&config(
            "MySQL ODBC 8.0 Unicode Driver",
            "127.0.0.1",
            3306,
            "",
        ))
        .expect("build the connection string");
        assert_eq!(
            string,
            "Driver={MySQL ODBC 8.0 Unicode Driver};Server=127.0.0.1;Port=3306;UID=root;PWD=secret;Database=app;CHARSET=utf8mb4;"
        );
    }

    #[test]
    fn the_engine_hint_is_enough_to_recognize_mysql() {
        // A renamed driver must still be recognized through 底层数据库类型.
        let string = connection_string(&config("Some Renamed Driver", "db.local", 3307, "MariaDB"))
            .expect("build the connection string");
        assert!(string.contains("Server=db.local;Port=3307;"), "{string}");
    }

    #[test]
    fn sqlserver_keeps_the_port_inside_the_server_attribute() {
        let string = connection_string(&config(
            "ODBC Driver 17 for SQL Server",
            "10.0.0.5",
            1433,
            "",
        ))
        .expect("build the connection string");
        assert!(
            string.contains("Server=10.0.0.5,1433;"),
            "SQL Server wants the port inside Server: {string}"
        );
        assert!(!string.contains("Port="), "{string}");
    }

    #[test]
    fn a_portless_profile_has_no_port_attribute() {
        let string = connection_string(&config("PostgreSQL Unicode", "db.local", 0, ""))
            .expect("build the connection string");
        assert!(string.contains("Server=db.local;"), "{string}");
        assert!(!string.contains("Port="), "{string}");
    }

    #[test]
    fn a_typed_multi_host_list_asks_mysql_for_multi_host() {
        let string = connection_string(&config(
            "MySQL ODBC 8.0 Unicode Driver",
            "db1.local,db2.local",
            3306,
            "",
        ))
        .expect("build the connection string");
        assert!(
            string.contains("Server=db1.local,db2.local;Port=3306;MULTI_HOST=1;"),
            "{string}"
        );
    }

    #[test]
    fn a_full_connection_string_wins_over_the_fields() {
        let mut profile = config("MySQL ODBC 8.0 Unicode Driver", "127.0.0.1", 3306, "");
        profile.options.insert(
            "odbc.connection_string".to_string(),
            "Driver={MySQL ODBC 8.0 Unicode Driver};Server=other;".to_string(),
        );
        let string = connection_string(&profile).expect("build the connection string");
        // The fields never override a typed string, but a MySQL string still gains the character
        // set that keeps non-Latin text from arriving as `?`.
        assert_eq!(
            string,
            "Driver={MySQL ODBC 8.0 Unicode Driver};Server=other;CHARSET=utf8mb4;"
        );
    }

    #[test]
    fn an_ansi_driver_asks_for_the_system_code_page() {
        let string =
            connection_string(&config("MySQL ODBC 8.0 ANSI Driver", "127.0.0.1", 3306, ""))
                .expect("build the connection string");
        let charset = mysql_charset("MySQL ODBC 8.0 ANSI Driver");
        assert!(string.contains(&format!("CHARSET={charset};")), "{string}");
    }

    #[test]
    fn an_explicit_character_set_is_kept() {
        let mut profile = config("MySQL ODBC 8.0 Unicode Driver", "127.0.0.1", 3306, "");
        profile.options.insert(
            "odbc.connection_string".to_string(),
            "Driver={MySQL ODBC 8.0 ANSI Driver};Server=other;CHARSET=gbk;".to_string(),
        );
        let string = connection_string(&profile).expect("build the connection string");
        assert_eq!(
            string,
            "Driver={MySQL ODBC 8.0 ANSI Driver};Server=other;CHARSET=gbk;"
        );
    }

    #[test]
    fn a_password_with_a_semicolon_is_braced() {
        let mut profile = config("MySQL ODBC 8.0 Unicode Driver", "127.0.0.1", 3306, "");
        profile.password = Some("p;a}ss".to_string());
        let string = connection_string(&profile).expect("build the connection string");
        assert!(string.contains("PWD={p;a}}ss};"), "{string}");
    }

    #[test]
    fn a_brace_inside_a_password_is_doubled_once() {
        let mut profile = config("MySQL ODBC 8.0 Unicode Driver", "127.0.0.1", 3306, "");
        profile.password = Some("p}a".to_string());
        let string = connection_string(&profile).expect("build the connection string");
        // The value is braced, so its `}` is doubled once; doubling it without braces would
        // change the password the driver reads.
        assert!(string.contains("PWD={p}}a};"), "{string}");
    }

    #[test]
    fn a_dsn_carries_the_credentials_alone() {
        let mut profile = config("MySQL ODBC 8.0 Unicode Driver", "127.0.0.1", 3306, "");
        profile
            .options
            .insert("odbc.dsn".to_string(), "erp".to_string());
        let string = connection_string(&profile).expect("build the connection string");
        assert_eq!(string, "DSN=erp;UID=root;PWD=secret;CHARSET=utf8mb4;");
    }

    #[test]
    fn a_missing_driver_name_is_an_error() {
        let mut profile = config("MySQL ODBC 8.0 Unicode Driver", "127.0.0.1", 3306, "");
        profile.options.remove("odbc.driver");
        assert!(connection_string(&profile).is_err());
    }
}
