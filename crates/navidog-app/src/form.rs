use std::time::{SystemTime, UNIX_EPOCH};

use navidog_core::{ConnectionProfile, DriverId};

#[derive(Clone, Copy, PartialEq, Eq)]
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

pub struct ConnectionForm {
    pub name: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub save_password: bool,
    pub database: String,
}

impl Default for ConnectionForm {
    fn default() -> Self {
        Self {
            name: "Local MySQL".to_string(),
            host: "127.0.0.1".to_string(),
            port: "3306".to_string(),
            username: "root".to_string(),
            password: String::new(),
            save_password: true,
            database: String::new(),
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
        }
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}
