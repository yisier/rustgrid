use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use rustgrid_core::{ConnectionOptions, ConnectionProfile, DriverId};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FormField {
    Name,
    Host,
    Port,
    Username,
    Password,
    Database,
    OdbcDriver,
    OdbcDsn,
    OdbcConnectionString,
    OdbcEngine,
}

pub const FORM_FIELDS: [FormField; 10] = [
    FormField::Name,
    FormField::Host,
    FormField::Port,
    FormField::Username,
    FormField::Password,
    FormField::Database,
    FormField::OdbcDriver,
    FormField::OdbcDsn,
    FormField::OdbcConnectionString,
    FormField::OdbcEngine,
];

/// Default query timeout (seconds) pre-filled on the 高级 page.
const DEFAULT_QUERY_TIMEOUT: u64 = 30;

/// The four pages of the connection window.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum FormTab {
    #[default]
    General,
    Tls,
    Tunnel,
    Advanced,
}

impl FormTab {
    /// Every tab, in the order the tab strip shows them.
    pub const ALL: [FormTab; 4] = [
        FormTab::General,
        FormTab::Tls,
        FormTab::Tunnel,
        FormTab::Advanced,
    ];

    pub fn label_key(self) -> &'static str {
        match self {
            FormTab::General => "form.tab.general",
            FormTab::Tls => "form.tab.tls",
            FormTab::Tunnel => "form.tab.tunnel",
            FormTab::Advanced => "form.tab.advanced",
        }
    }
}

#[derive(Clone)]
pub struct ConnectionForm {
    /// The engine this connection targets. Chosen from the New Connection dropdown and preserved
    /// when an existing connection is edited.
    pub driver: DriverId,
    /// Whether the engine is file-based (SQLite): the general page then edits a single database
    /// file path instead of host/port/username/password. Resolved from the driver registry.
    pub file_based: bool,
    pub name: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub save_password: bool,
    pub database: String,
    /// Whether the engine uses the ODBC fields below (resolved from the driver registry).
    pub odbc: bool,
    pub odbc_driver: String,
    pub odbc_dsn: String,
    pub odbc_connection_string: String,
    pub odbc_engine: String,
    /// The TLS / tunnel / timeout / read-only settings edited on the other tabs.
    pub settings: ConnectionOptions,
    /// The page currently shown.
    pub tab: FormTab,
}

impl Default for ConnectionForm {
    fn default() -> Self {
        Self {
            driver: DriverId::new("mysql"),
            file_based: false,
            name: String::new(),
            host: "127.0.0.1".to_string(),
            port: "3306".to_string(),
            username: "root".to_string(),
            password: String::new(),
            save_password: true,
            database: String::new(),
            odbc: false,
            odbc_driver: String::new(),
            odbc_dsn: String::new(),
            odbc_connection_string: String::new(),
            odbc_engine: String::new(),
            // Pre-fill the 高级 page with sensible defaults so a new connection is not blank there.
            settings: ConnectionOptions {
                connect_timeout: Some(30),
                query_timeout: Some(DEFAULT_QUERY_TIMEOUT),
                keepalive: Some(60),
                ..Default::default()
            },
            tab: FormTab::General,
        }
    }
}

impl ConnectionForm {
    /// A fresh form for a newly chosen engine, pre-filled with that engine's default port.
    pub fn for_driver(driver: DriverId, default_port: u16) -> Self {
        Self {
            driver,
            port: if default_port == 0 {
                String::new()
            } else {
                default_port.to_string()
            },
            ..Self::default()
        }
    }

    pub fn from_profile(
        profile: &ConnectionProfile,
        password: Option<String>,
        password_saved: bool,
    ) -> Self {
        let mut settings = profile.settings.clone();
        // `Some(0)` is the old "unset" placeholder for the query timeout (the field maps a typed
        // `0` back to `None`), so show the current default instead of a bare 0.
        if settings.query_timeout == Some(0) {
            settings.query_timeout = Some(DEFAULT_QUERY_TIMEOUT);
        }
        Self {
            driver: profile.driver.clone(),
            file_based: false,
            name: profile.name.clone(),
            host: profile.host.clone(),
            port: profile.port.to_string(),
            username: profile.username.clone(),
            password: password.unwrap_or_default(),
            save_password: password_saved,
            database: profile.database.clone().unwrap_or_default(),
            odbc: false,
            odbc_driver: profile
                .options
                .get("odbc.driver")
                .cloned()
                .unwrap_or_default(),
            odbc_dsn: profile.options.get("odbc.dsn").cloned().unwrap_or_default(),
            odbc_connection_string: profile
                .options
                .get("odbc.connection_string")
                .cloned()
                .unwrap_or_default(),
            odbc_engine: profile
                .options
                .get("odbc.engine")
                .cloned()
                .unwrap_or_default(),
            settings,
            tab: FormTab::General,
        }
    }

    pub fn set_value(&mut self, field: FormField, value: String) {
        *match field {
            FormField::Name => &mut self.name,
            FormField::Host => &mut self.host,
            FormField::Port => &mut self.port,
            FormField::Username => &mut self.username,
            FormField::Password => &mut self.password,
            FormField::Database => &mut self.database,
            FormField::OdbcDriver => &mut self.odbc_driver,
            FormField::OdbcDsn => &mut self.odbc_dsn,
            FormField::OdbcConnectionString => &mut self.odbc_connection_string,
            FormField::OdbcEngine => &mut self.odbc_engine,
        } = value;
    }

    /// The current text of one field.
    pub fn field_value(&self, field: FormField) -> &str {
        match field {
            FormField::Name => &self.name,
            FormField::Host => &self.host,
            FormField::Port => &self.port,
            FormField::Username => &self.username,
            FormField::Password => &self.password,
            FormField::Database => &self.database,
            FormField::OdbcDriver => &self.odbc_driver,
            FormField::OdbcDsn => &self.odbc_dsn,
            FormField::OdbcConnectionString => &self.odbc_connection_string,
            FormField::OdbcEngine => &self.odbc_engine,
        }
    }

    /// The required general-page fields that are currently empty. A network engine needs alias,
    /// host, port and username; a file-based engine (SQLite) needs the alias and its file path.
    /// Password and default database stay optional.
    pub fn missing_required(&self) -> Vec<FormField> {
        let mut missing = Vec::new();
        if self.name.trim().is_empty() {
            missing.push(FormField::Name);
        }
        if self.file_based {
            if self.database.trim().is_empty() {
                missing.push(FormField::Database);
            }
        } else if self.odbc {
            // An ODBC connection needs a driver name, a DSN or a raw connection string.
            if self.odbc_driver.trim().is_empty()
                && self.odbc_dsn.trim().is_empty()
                && self.odbc_connection_string.trim().is_empty()
            {
                missing.push(FormField::OdbcDriver);
            }
        } else {
            if self.host.trim().is_empty() {
                missing.push(FormField::Host);
            }
            if self.port.trim().is_empty() {
                missing.push(FormField::Port);
            }
            if self.username.trim().is_empty() {
                missing.push(FormField::Username);
            }
        }
        missing
    }

    pub fn to_profile(&self) -> ConnectionProfile {
        let name = self.name.trim();
        let database = self.database.trim();
        let database = if database.is_empty() {
            None
        } else {
            Some(database.to_string())
        };
        // A file-based engine stores the chosen path as the database and has no network identity.
        let (host, port, username) = if self.file_based {
            (String::new(), 0, String::new())
        } else {
            (
                self.host.trim().to_string(),
                self.port
                    .trim()
                    .parse()
                    .unwrap_or(if self.odbc { 0 } else { 3306 }),
                self.username.trim().to_string(),
            )
        };

        let mut options = BTreeMap::new();
        if self.odbc {
            if !self.odbc_driver.trim().is_empty() {
                options.insert(
                    "odbc.driver".to_string(),
                    self.odbc_driver.trim().to_string(),
                );
            }
            if !self.odbc_dsn.trim().is_empty() {
                options.insert("odbc.dsn".to_string(), self.odbc_dsn.trim().to_string());
            }
            if !self.odbc_connection_string.trim().is_empty() {
                options.insert(
                    "odbc.connection_string".to_string(),
                    self.odbc_connection_string.trim().to_string(),
                );
            }
            if !self.odbc_engine.trim().is_empty() {
                options.insert(
                    "odbc.engine".to_string(),
                    self.odbc_engine.trim().to_string(),
                );
            }
        }

        ConnectionProfile {
            id: format!("conn-{}", now_nanos()),
            name: if name.is_empty() {
                let fallback = self.host.trim();
                if fallback.is_empty() {
                    self.database.trim().to_string()
                } else {
                    fallback.to_string()
                }
            } else {
                name.to_string()
            },
            driver: self.driver.clone(),
            host,
            port,
            username,
            database,
            options,
            settings: self.settings.clone(),
        }
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}
