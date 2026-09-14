use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "logo.png" => Some(include_bytes!("../assets/logo.png")),
            "icons/connection.svg" => Some(include_bytes!("../assets/icons/connection.svg")),
            "icons/mysql.svg" => Some(include_bytes!("../assets/icons/mysql.svg")),
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
            "icons/arrow-up.svg" => Some(include_bytes!("../assets/icons/arrow-up.svg")),
            "icons/arrow-down.svg" => Some(include_bytes!("../assets/icons/arrow-down.svg")),
            "icons/sort-none.svg" => Some(include_bytes!("../assets/icons/sort-none.svg")),
            "icons/tab-prev.svg" => Some(include_bytes!("../assets/icons/tab-prev.svg")),
            "icons/tab-next.svg" => Some(include_bytes!("../assets/icons/tab-next.svg")),
            "icons/warning.svg" => Some(include_bytes!("../assets/icons/warning.svg")),
            "icons/transaction.svg" => Some(include_bytes!("../assets/icons/transaction.svg")),
            "icons/text.svg" => Some(include_bytes!("../assets/icons/text.svg")),
            "icons/filter.svg" => Some(include_bytes!("../assets/icons/filter.svg")),
            "icons/sort.svg" => Some(include_bytes!("../assets/icons/sort.svg")),
            "icons/refresh.svg" => Some(include_bytes!("../assets/icons/refresh.svg")),
            "icons/stop.svg" => Some(include_bytes!("../assets/icons/stop.svg")),
            "icons/plus.svg" => Some(include_bytes!("../assets/icons/plus.svg")),
            "icons/minus.svg" => Some(include_bytes!("../assets/icons/minus.svg")),
            "icons/check.svg" => Some(include_bytes!("../assets/icons/check.svg")),
            "icons/cross.svg" => Some(include_bytes!("../assets/icons/cross.svg")),
            "icons/first.svg" => Some(include_bytes!("../assets/icons/first.svg")),
            "icons/prev.svg" => Some(include_bytes!("../assets/icons/prev.svg")),
            "icons/next.svg" => Some(include_bytes!("../assets/icons/next.svg")),
            "icons/last.svg" => Some(include_bytes!("../assets/icons/last.svg")),
            "icons/gear.svg" => Some(include_bytes!("../assets/icons/gear.svg")),
            "icons/search.svg" => Some(include_bytes!("../assets/icons/search.svg")),
            "icons/row_marker.svg" => Some(include_bytes!("../assets/icons/row_marker.svg")),
            "icons/save.svg" => Some(include_bytes!("../assets/icons/save.svg")),
            "icons/query_builder.svg" => Some(include_bytes!("../assets/icons/query_builder.svg")),
            "icons/format_sql.svg" => Some(include_bytes!("../assets/icons/format_sql.svg")),
            "icons/snippets.svg" => Some(include_bytes!("../assets/icons/snippets.svg")),
            "icons/run.svg" => Some(include_bytes!("../assets/icons/run.svg")),
            "icons/explain.svg" => Some(include_bytes!("../assets/icons/explain.svg")),
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
