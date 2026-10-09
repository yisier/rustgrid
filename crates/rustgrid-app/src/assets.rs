use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "logo.png" => Some(include_bytes!("../assets/logo.png")),
            "icons/connection.svg" => Some(include_bytes!("../assets/icons/connection.svg")),
            "icons/mysql.svg" => Some(include_bytes!("../assets/icons/mysql.svg")),
            "icons/mariadb.svg" => Some(include_bytes!("../assets/icons/mariadb.svg")),
            "icons/sqlite.svg" => Some(include_bytes!("../assets/icons/sqlite.svg")),
            "icons/sqlserver.svg" => Some(include_bytes!("../assets/icons/sqlserver.svg")),
            "icons/postgresql.svg" => Some(include_bytes!("../assets/icons/postgresql.svg")),
            "icons/oracle.svg" => Some(include_bytes!("../assets/icons/oracle.svg")),
            "icons/database.svg" => Some(include_bytes!("../assets/icons/database.svg")),
            "icons/tables.svg" => Some(include_bytes!("../assets/icons/tables.svg")),
            "icons/design_table.svg" => Some(include_bytes!("../assets/icons/design_table.svg")),
            "icons/add_field.svg" => Some(include_bytes!("../assets/icons/add_field.svg")),
            "icons/insert_field.svg" => Some(include_bytes!("../assets/icons/insert_field.svg")),
            "icons/delete_field.svg" => Some(include_bytes!("../assets/icons/delete_field.svg")),
            "icons/primary_key.svg" => Some(include_bytes!("../assets/icons/primary_key.svg")),
            "icons/new_table.svg" => Some(include_bytes!("../assets/icons/new_table.svg")),
            "icons/delete_table.svg" => Some(include_bytes!("../assets/icons/delete_table.svg")),
            "icons/import.svg" => Some(include_bytes!("../assets/icons/import.svg")),
            "icons/export.svg" => Some(include_bytes!("../assets/icons/export.svg")),
            "icons/user.svg" => Some(include_bytes!("../assets/icons/user.svg")),
            "icons/views.svg" => Some(include_bytes!("../assets/icons/views.svg")),
            "icons/functions.svg" => Some(include_bytes!("../assets/icons/functions.svg")),
            "icons/function.svg" => Some(include_bytes!("../assets/icons/function.svg")),
            "icons/procedure.svg" => Some(include_bytes!("../assets/icons/procedure.svg")),
            "icons/view-detail.svg" => Some(include_bytes!("../assets/icons/view-detail.svg")),
            "icons/view-grid.svg" => Some(include_bytes!("../assets/icons/view-grid.svg")),
            "icons/queries.svg" => Some(include_bytes!("../assets/icons/queries.svg")),
            "icons/new_query.svg" => Some(include_bytes!("../assets/icons/new_query.svg")),
            "icons/backups.svg" => Some(include_bytes!("../assets/icons/backups.svg")),
            "icons/events.svg" => Some(include_bytes!("../assets/icons/events.svg")),
            "icons/chevron-right.svg" => Some(include_bytes!("../assets/icons/chevron-right.svg")),
            "icons/chevron-down.svg" => Some(include_bytes!("../assets/icons/chevron-down.svg")),
            "icons/arrow-up.svg" => Some(include_bytes!("../assets/icons/arrow-up.svg")),
            "icons/arrow-down.svg" => Some(include_bytes!("../assets/icons/arrow-down.svg")),
            "icons/update.svg" => Some(include_bytes!("../assets/icons/update.svg")),
            "icons/sort-none.svg" => Some(include_bytes!("../assets/icons/sort-none.svg")),
            "icons/tab-prev.svg" => Some(include_bytes!("../assets/icons/tab-prev.svg")),
            "icons/tab-next.svg" => Some(include_bytes!("../assets/icons/tab-next.svg")),
            "icons/warning.svg" => Some(include_bytes!("../assets/icons/warning.svg")),
            "icons/warning_mark.svg" => Some(include_bytes!("../assets/icons/warning_mark.svg")),
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
            "icons/github.svg" => Some(include_bytes!("../assets/icons/github.svg")),
            "icons/sun.svg" => Some(include_bytes!("../assets/icons/sun.svg")),
            "icons/moon.svg" => Some(include_bytes!("../assets/icons/moon.svg")),
            "icons/monitor.svg" => Some(include_bytes!("../assets/icons/monitor.svg")),
            "icons/folder.svg" => Some(include_bytes!("../assets/icons/folder.svg")),
            "icons/search.svg" => Some(include_bytes!("../assets/icons/search.svg")),
            "icons/row_marker.svg" => Some(include_bytes!("../assets/icons/row_marker.svg")),
            "icons/save.svg" => Some(include_bytes!("../assets/icons/save.svg")),
            "icons/format_sql.svg" => Some(include_bytes!("../assets/icons/format_sql.svg")),
            "icons/query_builder.svg" => Some(include_bytes!("../assets/icons/query_builder.svg")),
            "icons/explain.svg" => Some(include_bytes!("../assets/icons/explain.svg")),
            "icons/panel-left.svg" => Some(include_bytes!("../assets/icons/panel-left.svg")),
            "icons/panel-right.svg" => Some(include_bytes!("../assets/icons/panel-right.svg")),
            "icons/run.svg" => Some(include_bytes!("../assets/icons/run.svg")),
            "icons/activity.svg" => Some(include_bytes!("../assets/icons/activity.svg")),
            "icons/lock.svg" => Some(include_bytes!("../assets/icons/lock.svg")),
            "icons/zap.svg" => Some(include_bytes!("../assets/icons/zap.svg")),
            _ => None,
        };
        match bytes {
            Some(bytes) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
