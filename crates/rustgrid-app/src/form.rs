use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use rustgrid_core::{
    ConnectionFieldLabel, ConnectionFieldSpec, ConnectionFormSpec, ConnectionHomePage,
    ConnectionOptions, ConnectionPage, ConnectionProfile, ConnectionStandardField, DriverId,
};

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

#[derive(Clone)]
pub struct ConnectionForm {
    /// The engine this connection targets. Chosen from the New Connection dropdown and preserved
    /// when an existing connection is edited.
    pub driver: DriverId,
    /// Whether the engine is file-based (SQLite): the 常规 page then edits a single database file
    /// path instead of host/port/username/password. Derived from the driver's home page.
    pub file_based: bool,
    pub name: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub save_password: bool,
    pub database: String,
    /// Whether the engine uses the ODBC fields below (derived from the driver's home page).
    pub odbc: bool,
    pub odbc_driver: String,
    pub odbc_dsn: String,
    pub odbc_connection_string: String,
    pub odbc_engine: String,
    /// The TLS / tunnel / timeout / read-only settings edited on the other pages.
    pub settings: ConnectionOptions,
    /// Engine-specific fields declared by the driver's `ConnectionFormSpec`, keyed by their
    /// `ConnectionProfile::options` key. Preserved across an edit even for keys the current driver
    /// does not declare, so switching engines never silently drops an option.
    pub extra: BTreeMap<String, String>,
    /// The shape of the 常规 page, resolved from the driver's spec.
    pub home: ConnectionHomePage,
    /// The pages to show, in tab order (`pages[0]` is always the 常规 page), resolved from the
    /// driver's spec.
    pub pages: Vec<ConnectionPage>,
    /// The page currently shown.
    pub page: ConnectionPage,
    /// Per-engine renames of the standard fields, resolved from the driver's spec.
    pub labels: Vec<ConnectionFieldLabel>,
    /// The engine's option fields (each with its page), resolved from the driver's spec.
    pub option_fields: Vec<ConnectionFieldSpec>,
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
            extra: BTreeMap::new(),
            // The standard network layout until a driver's spec is applied.
            home: ConnectionHomePage::Network,
            pages: vec![
                ConnectionPage::General,
                ConnectionPage::Tls,
                ConnectionPage::Tunnel,
                ConnectionPage::Advanced,
            ],
            page: ConnectionPage::General,
            labels: Vec::new(),
            option_fields: Vec::new(),
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
            // The ODBC keys have dedicated fields above; every other option is an engine-specific
            // field owned by `extra` and round-tripped untouched.
            extra: profile
                .options
                .iter()
                .filter(|(key, _)| !key.starts_with("odbc."))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            settings,
            // The driver's spec is applied by `show_form` once the registry is consulted.
            home: ConnectionHomePage::Network,
            pages: vec![
                ConnectionPage::General,
                ConnectionPage::Tls,
                ConnectionPage::Tunnel,
                ConnectionPage::Advanced,
            ],
            page: ConnectionPage::General,
            labels: Vec::new(),
            option_fields: Vec::new(),
        }
    }

    /// Adopt a driver's connection-form spec: the 常规 page's shape, which pages (tabs) are shown,
    /// the standard-field renames and the engine's option fields with their placement.
    pub fn apply_spec(&mut self, spec: &ConnectionFormSpec) {
        self.home = spec.home;
        self.file_based = spec.home == ConnectionHomePage::File;
        self.odbc = spec.home == ConnectionHomePage::Odbc;
        self.labels = spec.labels.clone();
        self.option_fields = spec.options.clone();
        self.pages = std::iter::once(ConnectionPage::General)
            .chain(spec.tabs.iter().copied())
            .collect();
        if !self.pages.contains(&self.page) {
            self.page = ConnectionPage::General;
        }
    }

    /// The per-engine rename/hint of a standard field, if the driver declares one.
    pub fn label_override(&self, field: ConnectionStandardField) -> Option<&ConnectionFieldLabel> {
        self.labels.iter().find(|label| label.field == field)
    }

    /// The option fields shown on `page`, in declaration order.
    pub fn page_option_fields(
        &self,
        page: ConnectionPage,
    ) -> impl Iterator<Item = &ConnectionFieldSpec> {
        self.option_fields
            .iter()
            .filter(move |field| field.page == page)
    }

    /// Whether an option field is currently visible. A field with a `visible_when` condition is
    /// shown while its controlling option has the matching value — or whenever the field itself
    /// already holds a value, so a legacy profile can never hide an active option.
    pub fn option_visible(&self, field: &ConnectionFieldSpec) -> bool {
        match field.visible_when {
            None => true,
            Some((key, value)) => {
                self.option_value(key) == value || !self.option_value(field.key).is_empty()
            }
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

    /// The current text of one engine-specific option field.
    pub fn option_value(&self, key: &str) -> &str {
        self.extra.get(key).map(String::as_str).unwrap_or("")
    }

    /// Store one engine-specific option field.
    pub fn set_option(&mut self, key: &str, value: String) {
        self.extra.insert(key.to_string(), value);
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

        let mut options: BTreeMap<String, String> = self
            .extra
            .iter()
            // Drop blank values so an empty field never overrides a driver's own default (e.g.
            // `oracle.service_name` falls back to the database field when it is absent).
            .filter(|(key, value)| !key.starts_with("odbc.") && !value.trim().is_empty())
            .map(|(key, value)| (key.clone(), value.trim().to_string()))
            .collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(options: BTreeMap<String, String>) -> ConnectionProfile {
        ConnectionProfile {
            id: "conn-1".to_string(),
            name: "oracle".to_string(),
            driver: DriverId::new("oracle"),
            host: "127.0.0.1".to_string(),
            port: 1521,
            username: "scott".to_string(),
            database: Some("FREEPDB1".to_string()),
            options,
            settings: ConnectionOptions::default(),
        }
    }

    /// Engine options survive a load/save round-trip; the ODBC keys stay in their own fields.
    #[test]
    fn engine_options_round_trip_without_odbc_keys() {
        let mut options = BTreeMap::new();
        options.insert("oracle.tns_alias".to_string(), "ORCL".to_string());
        options.insert("odbc.driver".to_string(), "SQLite3".to_string());

        let form = ConnectionForm::from_profile(&profile(options), None, false);
        assert_eq!(form.option_value("oracle.tns_alias"), "ORCL");
        assert_eq!(form.option_value("odbc.driver"), "");

        let saved = form.to_profile();
        assert_eq!(
            saved.options.get("oracle.tns_alias").map(String::as_str),
            Some("ORCL")
        );
        assert!(!saved.options.contains_key("odbc.driver"));
    }

    /// A blank engine field is dropped, so it cannot override the driver's own default.
    #[test]
    fn blank_engine_options_are_not_persisted() {
        let mut form = ConnectionForm::default();
        form.set_option("oracle.wallet_password", "   ".to_string());
        form.set_option("oracle.config_dir", " /etc/oracle ".to_string());

        let options = form.to_profile().options;
        assert!(!options.contains_key("oracle.wallet_password"));
        assert_eq!(
            options.get("oracle.config_dir").map(String::as_str),
            Some("/etc/oracle")
        );
    }

    /// The driver's spec is applied to the form: the page list, the 常规 shape, the standard-field
    /// renames, the option placement and the `visible_when` gating.
    #[test]
    fn apply_spec_resolves_pages_labels_and_option_visibility() {
        use rustgrid_core::{ConnectionFieldKind, ConnectionFieldSpec};

        let spec = ConnectionFormSpec {
            home: ConnectionHomePage::Odbc,
            tabs: vec![ConnectionPage::Tls, ConnectionPage::Advanced],
            labels: vec![ConnectionFieldLabel {
                field: ConnectionStandardField::Database,
                label_key: "test.database",
                hint_key: Some("test.database_hint"),
            }],
            options: vec![
                ConnectionFieldSpec {
                    key: "test.mode",
                    label_key: "test.mode",
                    hint_key: None,
                    kind: ConnectionFieldKind::Select(Vec::new()),
                    page: ConnectionPage::General,
                    visible_when: None,
                },
                ConnectionFieldSpec {
                    key: "test.gated",
                    label_key: "test.gated",
                    hint_key: None,
                    kind: ConnectionFieldKind::Text,
                    page: ConnectionPage::Tls,
                    visible_when: Some(("test.mode", "on")),
                },
            ],
        };

        let mut form = ConnectionForm::default();
        form.apply_spec(&spec);

        // The spec's page list and 常规 shape win over the default.
        assert_eq!(
            form.pages,
            vec![
                ConnectionPage::General,
                ConnectionPage::Tls,
                ConnectionPage::Advanced
            ]
        );
        assert_eq!(form.home, ConnectionHomePage::Odbc);
        assert!(form.odbc);
        assert!(!form.file_based);
        assert_eq!(form.page, ConnectionPage::General);

        // Label overrides are found by standard field; unlisted fields keep the app default.
        assert_eq!(
            form.label_override(ConnectionStandardField::Database)
                .map(|label| label.label_key),
            Some("test.database")
        );
        assert!(form.label_override(ConnectionStandardField::Host).is_none());

        // Options belong to their page.
        let general: Vec<_> = form
            .page_option_fields(ConnectionPage::General)
            .map(|field| field.key)
            .collect();
        assert_eq!(general, vec!["test.mode"]);
        let tls: Vec<_> = form
            .page_option_fields(ConnectionPage::Tls)
            .map(|field| field.key)
            .collect();
        assert_eq!(tls, vec!["test.gated"]);

        // `visible_when`: hidden until the control matches — but never hidden once it holds a value.
        let gated = form
            .option_fields
            .iter()
            .find(|field| field.key == "test.gated")
            .cloned()
            .expect("gated field");
        assert!(!form.option_visible(&gated));
        form.set_option("test.mode", "on".to_string());
        assert!(form.option_visible(&gated));
        form.set_option("test.mode", "off".to_string());
        assert!(!form.option_visible(&gated));
        form.set_option("test.gated", "/tmp/x".to_string());
        assert!(form.option_visible(&gated));
    }
}
