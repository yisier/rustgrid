use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "logo.png" => Some(include_bytes!("../assets/logo.png")),
            "icons/connection.svg" => Some(include_bytes!("../assets/icons/connection.svg")),
            "icons/database.svg" => Some(include_bytes!("../assets/icons/database.svg")),
            "icons/tables.svg" => Some(include_bytes!("../assets/icons/tables.svg")),
            "icons/design_table.svg" => Some(include_bytes!("../assets/icons/design_table.svg")),
            "icons/new_table.svg" => Some(include_bytes!("../assets/icons/new_table.svg")),
            "icons/delete_table.svg" => Some(include_bytes!("../assets/icons/delete_table.svg")),
            "icons/import.svg" => Some(include_bytes!("../assets/icons/import.svg")),
            "icons/export.svg" => Some(include_bytes!("../assets/icons/export.svg")),
            "icons/user.svg" => Some(include_bytes!("../assets/icons/user.svg")),
            "icons/views.svg" => Some(include_bytes!("../assets/icons/views.svg")),
            "icons/functions.svg" => Some(include_bytes!("../assets/icons/functions.svg")),
            "icons/queries.svg" => Some(include_bytes!("../assets/icons/queries.svg")),
            "icons/backups.svg" => Some(include_bytes!("../assets/icons/backups.svg")),
            "icons/chevron-right.svg" => Some(include_bytes!("../assets/icons/chevron-right.svg")),
            "icons/chevron-down.svg" => Some(include_bytes!("../assets/icons/chevron-down.svg")),
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
