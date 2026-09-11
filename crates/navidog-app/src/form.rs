use std::time::{SystemTime, UNIX_EPOCH};

use gpui::KeyDownEvent;
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

pub enum FormAction {
    Changed,
    Submit,
    FocusNext,
    FocusPrev,
    Ignore,
}

pub struct ConnectionForm {
    pub name: String,
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
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
            database: String::new(),
        }
    }
}

impl ConnectionForm {
    pub fn value(&self, field: FormField) -> &str {
        match field {
            FormField::Name => &self.name,
            FormField::Host => &self.host,
            FormField::Port => &self.port,
            FormField::Username => &self.username,
            FormField::Password => &self.password,
            FormField::Database => &self.database,
        }
    }

    pub fn value_mut(&mut self, field: FormField) -> &mut String {
        match field {
            FormField::Name => &mut self.name,
            FormField::Host => &mut self.host,
            FormField::Port => &mut self.port,
            FormField::Username => &mut self.username,
            FormField::Password => &mut self.password,
            FormField::Database => &mut self.database,
        }
    }

    pub fn apply_key(&mut self, field: FormField, event: &KeyDownEvent) -> FormAction {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return FormAction::Ignore;
        }

        match keystroke.key.as_str() {
            "backspace" => {
                self.value_mut(field).pop();
                FormAction::Changed
            }
            "space" => {
                self.value_mut(field).push(' ');
                FormAction::Changed
            }
            "enter" => FormAction::Submit,
            "tab" => {
                if keystroke.modifiers.shift {
                    FormAction::FocusPrev
                } else {
                    FormAction::FocusNext
                }
            }
            _ => match keystroke.key_char.as_ref() {
                Some(text) => {
                    self.value_mut(field).push_str(text);
                    FormAction::Changed
                }
                None => FormAction::Ignore,
            },
        }
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
