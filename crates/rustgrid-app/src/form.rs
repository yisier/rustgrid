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
}

pub const FORM_FIELDS: [FormField; 6] = [
    FormField::Name,
    FormField::Host,
    FormField::Port,
    FormField::Username,
    FormField::Password,
    FormField::Database,
];

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
    pub name: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub save_password: bool,
    pub database: String,
    /// The TLS / tunnel / timeout / read-only settings edited on the other tabs.
    pub settings: ConnectionOptions,
    /// The page currently shown.
    pub tab: FormTab,
}

impl Default for ConnectionForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: "127.0.0.1".to_string(),
            port: "3306".to_string(),
            username: "root".to_string(),
            password: String::new(),
            save_password: true,
            database: String::new(),
            // Pre-fill the 高级 page with sensible defaults so a new connection is not blank there.
            settings: ConnectionOptions {
                connect_timeout: Some(30),
                query_timeout: Some(0),
                keepalive: Some(60),
                ..Default::default()
            },
            tab: FormTab::General,
        }
    }
}

impl ConnectionForm {
    pub fn from_profile(
        profile: &ConnectionProfile,
        password: Option<String>,
        password_saved: bool,
    ) -> Self {
        Self {
            name: profile.name.clone(),
            host: profile.host.clone(),
            port: profile.port.to_string(),
            username: profile.username.clone(),
            password: password.unwrap_or_default(),
            save_password: password_saved,
            database: profile.database.clone().unwrap_or_default(),
            settings: profile.settings.clone(),
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
        }
    }

    /// The required general-page fields that are currently empty (alias, host, port, username).
    /// Password and default database stay optional.
    pub fn missing_required(&self) -> Vec<FormField> {
        let mut missing = Vec::new();
        if self.name.trim().is_empty() {
            missing.push(FormField::Name);
        }
        if self.host.trim().is_empty() {
            missing.push(FormField::Host);
        }
        if self.port.trim().is_empty() {
            missing.push(FormField::Port);
        }
        if self.username.trim().is_empty() {
            missing.push(FormField::Username);
        }
        missing
    }

    pub fn to_profile(&self) -> ConnectionProfile {
        let name = self.name.trim();
        let database = self.database.trim();

        ConnectionProfile {
            id: format!("conn-{}", now_nanos()),
            name: if name.is_empty() {
                self.host.trim().to_string()
            } else {
                name.to_string()
            },
            driver: DriverId::new("mysql"),
            host: self.host.trim().to_string(),
            port: self.port.trim().parse().unwrap_or(3306),
            username: self.username.trim().to_string(),
            database: if database.is_empty() {
                None
            } else {
                Some(database.to_string())
            },
            options: Default::default(),
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
