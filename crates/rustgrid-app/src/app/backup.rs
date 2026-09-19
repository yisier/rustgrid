//! The Backup main tab: RustGrid `.rgbak` backups.
//!
//! `AppView` owns the state (the backup list, the "New Backup" and "Restore Backup" dialogs);
//! this module renders it and drives the work through [`rustgrid_backup`] and the
//! [`rustgrid_core::Connection`] trait. Files are written under the app config dir's
//! `backups/<connection-id>/<database>/` tree.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

use super::*;
use gpui_kit::component::WindowExt;
use rustgrid_backup::BackupManifest;
use rustgrid_core::{BackupObjectKind, ObjectKind, SavedBackup};

use super::dialogs::centered_margin_top;

/// Rows in the New Backup dialog's object tree are `20px` tall.
const BACKUP_OBJECT_ROW_HEIGHT: f32 = 19.0;

impl AppView {
    // ----- List state -------------------------------------------------------------------------

    /// Rescan the backup directory tree for `.rgbak` files. Synchronous: listing involves reading
    /// each file's manifest, which is acceptable for a user-driven refresh.
    pub(super) fn refresh_backups(&mut self, _cx: &mut Context<'_, Self>) {
        let root = self.config.backups_dir();
        let mut files = Vec::new();
        if let Ok(connections) = std::fs::read_dir(&root) {
            for connection in connections.flatten() {
                if !connection
                    .file_type()
                    .map(|kind| kind.is_dir())
                    .unwrap_or(false)
                {
                    continue;
                }
                let connection_id = connection.file_name().to_string_lossy().into_owned();
                let Ok(databases) = std::fs::read_dir(connection.path()) else {
                    continue;
                };
                for database in databases.flatten() {
                    if !database
                        .file_type()
                        .map(|kind| kind.is_dir())
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let database_name = database.file_name().to_string_lossy().into_owned();
                    let Ok(entries) = std::fs::read_dir(database.path()) else {
                        continue;
                    };
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|ext| ext.to_str())
                            != Some(rustgrid_backup::FILE_EXTENSION)
                        {
                            continue;
                        }
                        let Ok(manifest) = rustgrid_backup::read_manifest(&path) else {
                            continue;
                        };
                        let metadata = entry.metadata().ok();
                        files.push(BackupFileInfo {
                            name: path
                                .file_stem()
                                .map(|stem| stem.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            connection_id: connection_id.clone(),
                            database: database_name.clone(),
                            manifest,
                            size: metadata.as_ref().map(|meta| meta.len()).unwrap_or(0),
                            modified: metadata.and_then(|meta| meta.modified().ok()),
                            path,
                        });
                    }
                }
            }
        }
        files.sort_by(|left, right| right.name.cmp(&left.name));
        self.backup_files = files;
        self.clamp_backup_selection();
    }

    fn clamp_backup_selection(&mut self) {
        match self.backup_selected {
            Some(BackupSelection::File(index)) if index >= self.backup_files.len() => {
                self.backup_selected = None;
            }
            Some(BackupSelection::Config(index)) if index >= self.backup_configs.len() => {
                self.backup_selected = None;
            }
            _ => {}
        }
    }

    fn backup_dir(&self, connection_id: &str, database: &str) -> PathBuf {
        self.config
            .backups_dir()
            .join(sanitize_component(connection_id))
            .join(sanitize_component(database))
    }

    fn connection_name_by_id(&self, connection_id: &str) -> String {
        self.connections
            .iter()
            .find(|node| node.profile.id == connection_id)
            .map(|node| node.profile.name.clone())
            .unwrap_or_else(|| connection_id.to_string())
    }

    /// The connected connections, as `(index, name)` options for the dialogs.
    fn connected_connection_options(&self) -> Vec<(String, String)> {
        self.connections
            .iter()
            .enumerate()
            .filter(|(_, node)| matches!(node.status, ConnectionStatus::Connected(_)))
            .map(|(index, node)| (index.to_string(), node.profile.name.clone()))
            .collect()
    }

    // ----- Rendering --------------------------------------------------------------------------

    pub(super) fn render_backups(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(self.render_backup_toolbar(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(px(300.0))
                            .flex_none()
                            .h_full()
                            .border_r_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.sidebar_bg))
                            .child(self.render_backup_list(cx)),
                    )
                    .child(
                        div()
                            .id("backup-details")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .overflow_y_scroll()
                            .bg(rgb(theme.editor_bg))
                            .child(self.render_backup_details(cx)),
                    ),
            )
            .into_any_element()
    }

    fn render_backup_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let has_file = matches!(self.backup_selected, Some(BackupSelection::File(_)));
        let has_selection = self.backup_selected.is_some();
        let can_create = self.default_query_connection(cx).is_some();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.toolbar_item(
                "backup-restore",
                "icons/import.svg",
                t!("backup.restore").to_string(),
                has_file,
                cx.listener(|this, _event, _window, cx| this.restore_selected_backup(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(self.toolbar_item(
                "backup-new",
                "icons/backups.svg",
                t!("backup.new").to_string(),
                can_create,
                cx.listener(|this, _event, _window, cx| this.open_new_backup(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(self.toolbar_item(
                "backup-delete",
                "icons/delete_table.svg",
                t!("backup.delete").to_string(),
                has_selection,
                cx.listener(|this, _event, _window, cx| this.confirm_delete_backup(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(self.toolbar_item(
                "backup-extract",
                "icons/export.svg",
                t!("backup.extract_sql").to_string(),
                has_file,
                cx.listener(|this, _event, _window, cx| this.extract_selected_backup(cx)),
            ))
    }

    fn render_backup_list(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut list = div()
            .id("backup-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .py_1();

        let mut has_any = false;
        for (index, file) in self.backup_files.iter().enumerate() {
            has_any = true;
            let selected = self.backup_selected == Some(BackupSelection::File(index));
            let subtitle = format!(
                "{} / {}",
                self.connection_name_by_id(&file.connection_id),
                file.database
            );
            list = list.child(backup_row(
                SharedString::from(format!("backup-file-{index}")),
                "icons/backups.svg",
                theme.icon_backups,
                file.name.clone(),
                subtitle,
                selected,
                theme,
                cx.listener(move |this, event, _window, cx| {
                    this.backup_selected = Some(BackupSelection::File(index));
                    if matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2) {
                        this.open_restore_backup(index, cx);
                    }
                    cx.notify();
                }),
            ));
        }
        for (index, config) in self.backup_configs.iter().enumerate() {
            has_any = true;
            let selected = self.backup_selected == Some(BackupSelection::Config(index));
            let subtitle = format!(
                "{} / {}",
                self.connection_name_by_id(&config.connection_id),
                config.database
            );
            list = list.child(backup_row(
                SharedString::from(format!("backup-config-{index}")),
                "icons/save.svg",
                theme.icon_queries,
                config.name.clone(),
                subtitle,
                selected,
                theme,
                cx.listener(move |this, event, _window, cx| {
                    this.backup_selected = Some(BackupSelection::Config(index));
                    if matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2) {
                        this.open_backup_config(index, cx);
                    }
                    cx.notify();
                }),
            ));
        }

        if !has_any {
            list = list.child(
                div()
                    .p_3()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("backup.empty").to_string()),
            );
        }
        list.into_any_element()
    }

    fn render_backup_details(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let _ = cx;
        let theme = self.theme;
        let Some(selected) = self.backup_selected else {
            return div()
                .p_4()
                .text_color(rgb(theme.text_muted))
                .child(t!("backup.select_hint").to_string())
                .into_any_element();
        };

        match selected {
            BackupSelection::File(index) => {
                let Some(file) = self.backup_files.get(index) else {
                    return div().into_any_element();
                };
                let mut column = div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(backup_detail_header(
                        "icons/backups.svg",
                        theme.icon_backups,
                        file.name.clone(),
                        t!("common.backup").to_string(),
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.file").to_string(),
                        file.path.display().to_string(),
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.size").to_string(),
                        format_size(file.size),
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.created").to_string(),
                        format_unix_seconds(file.manifest.created_unix),
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.modified").to_string(),
                        file.modified
                            .map(format_system_time)
                            .unwrap_or_else(|| "--".to_string()),
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.version").to_string(),
                        file.manifest.backup_version.clone(),
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.comment").to_string(),
                        if file.manifest.comment.is_empty() {
                            "--".to_string()
                        } else {
                            file.manifest.comment.clone()
                        },
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.database").to_string(),
                        file.database.clone(),
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.connection").to_string(),
                        self.connection_name_by_id(&file.connection_id),
                        theme,
                    ));
                column = column.child(backup_detail_field(
                    t!("backup.field.objects").to_string(),
                    backup_object_counts(&file.manifest),
                    theme,
                ));
                column.into_any_element()
            }
            BackupSelection::Config(index) => {
                let Some(config) = self.backup_configs.get(index) else {
                    return div().into_any_element();
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(backup_detail_header(
                        "icons/save.svg",
                        theme.icon_queries,
                        config.name.clone(),
                        t!("backup.config").to_string(),
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.connection").to_string(),
                        self.connection_name_by_id(&config.connection_id),
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.database").to_string(),
                        config.database.clone(),
                        theme,
                    ))
                    .child(backup_detail_field(
                        t!("backup.field.objects").to_string(),
                        saved_config_counts(config),
                        theme,
                    ))
                    .into_any_element()
            }
        }
    }

    // ----- Actions ----------------------------------------------------------------------------

    fn restore_selected_backup(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(BackupSelection::File(index)) = self.backup_selected {
            self.open_restore_backup(index, cx);
        }
    }

    fn extract_selected_backup(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(BackupSelection::File(index)) = self.backup_selected {
            self.extract_backup_sql(index, cx);
        }
    }

    fn confirm_delete_backup(&mut self, cx: &mut Context<'_, Self>) {
        match self.backup_selected {
            Some(BackupSelection::File(index)) => {
                self.delete_confirm = Some(DeleteConfirm::BackupFile { index });
            }
            Some(BackupSelection::Config(index)) => {
                self.delete_confirm = Some(DeleteConfirm::BackupConfig { index });
            }
            None => {}
        }
        cx.notify();
    }

    pub(super) fn delete_backup_file(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(file) = self.backup_files.get(index) {
            let _ = std::fs::remove_file(&file.path);
        }
        self.backup_selected = None;
        self.refresh_backups(cx);
        cx.notify();
    }

    pub(super) fn delete_backup_config(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index < self.backup_configs.len() {
            self.backup_configs.remove(index);
        }
        let _ = self.config.save_backups(&self.backup_configs);
        self.backup_selected = None;
        self.clamp_backup_selection();
        cx.notify();
    }

    /// Extract a backup's objects into a `.sql` file beside it and open it in a query tab.
    fn extract_backup_sql(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(file) = self.backup_files.get(index) else {
            return;
        };
        let path = file.path.clone();
        let connection_index = self
            .connections
            .iter()
            .position(|node| node.profile.id == file.connection_id);
        let database = file.manifest.schema.clone();
        match rustgrid_backup::read_backup(&path) {
            Ok(archive) => {
                let mut sql = String::new();
                for object in &archive.objects {
                    sql.push_str(&rustgrid_backup::object_sql(object));
                    sql.push('\n');
                }
                let sql_path = path.with_extension("sql");
                let _ = std::fs::write(&sql_path, &sql);
                self.open_extracted_sql(connection_index, database, sql, cx);
            }
            Err(error) => {
                self.error_dialog = Some(error.to_string());
                cx.notify();
            }
        }
    }

    fn open_extracted_sql(
        &mut self,
        connection_index: Option<usize>,
        database: String,
        sql: String,
        cx: &mut Context<'_, Self>,
    ) {
        let id = self.next_query_id;
        self.next_query_id += 1;
        let mut tab = QueryTab::new(id);
        tab.connection_index = connection_index;
        tab.database = (!database.is_empty()).then_some(database);
        tab.caret = sql.len();
        tab.anchor = sql.len();
        tab.sql = sql;
        self.queries.push(tab);
        self.active_query = Some(self.queries.len() - 1);
        self.active_grid = None;
        self.active_design = None;
        self.query_completion = None;
        self.query_focus_pending = true;
        self.main_tab = MainTab::Queries;
        cx.notify();
    }

    // ----- New Backup dialog ------------------------------------------------------------------

    pub(super) fn open_new_backup(&mut self, cx: &mut Context<'_, Self>) {
        let connection_index = self.default_query_connection(cx);
        let database = connection_index
            .and_then(|index| self.default_query_database(index, cx))
            .unwrap_or_default();
        self.open_new_backup_dialog_with(None, connection_index, database, cx);
    }

    /// Open the New Backup dialog for a saved configuration, pre-filling its selection.
    fn open_backup_config(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(config) = self.backup_configs.get(index).cloned() else {
            return;
        };
        let connection_index = self
            .connections
            .iter()
            .position(|node| {
                node.profile.id == config.connection_id
                    && matches!(node.status, ConnectionStatus::Connected(_))
            })
            .or_else(|| self.default_query_connection(cx));
        let database = config.database.clone();
        self.open_new_backup_dialog_with(Some(config), connection_index, database, cx);
    }

    fn open_new_backup_dialog_with(
        &mut self,
        apply: Option<SavedBackup>,
        connection_index: Option<usize>,
        database: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection_index) = connection_index.or_else(|| self.default_query_connection(cx))
        else {
            self.error_dialog = Some(t!("backup.no_connection").to_string());
            cx.notify();
            return;
        };
        let database = if database.is_empty() {
            self.default_query_database(connection_index, cx)
                .unwrap_or_default()
        } else {
            database
        };
        let config_name = apply
            .as_ref()
            .map(|saved| saved.name.clone())
            .unwrap_or_default();
        let weak = cx.weak_entity();
        let input = make_backup_name_input(self.theme, config_name.clone(), &weak, cx);
        self.backup_name_input = Some(input);
        self.new_backup_dialog = Some(NewBackupDialog {
            connection_index,
            database,
            tab: BackupDialogTab::Objects,
            objects: Vec::new(),
            config_name,
            apply,
            loading: false,
            running: false,
            log: Vec::new(),
            error: None,
        });
        self.backup_name_focus_pending = true;
        self.load_backup_objects(cx);
        cx.notify();
    }

    /// Load tables/views/functions/events for the dialog's selected database.
    pub(super) fn load_backup_objects(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.new_backup_dialog.as_ref() else {
            return;
        };
        if dialog.loading {
            return;
        }
        let connection_index = dialog.connection_index;
        let database = dialog.database.clone();
        if database.is_empty() {
            if let Some(dialog) = self.new_backup_dialog.as_mut() {
                dialog.error = Some(t!("backup.no_database").to_string());
            }
            cx.notify();
            return;
        }
        let Some(connection) = self.connection_arc(connection_index) else {
            if let Some(dialog) = self.new_backup_dialog.as_mut() {
                dialog.error = Some(t!("backup.not_connected").to_string());
            }
            cx.notify();
            return;
        };
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.loading = true;
            dialog.error = None;
        }
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let tables = {
                let connection = connection.clone();
                let database = database.clone();
                runtime
                    .spawn(async move { connection.list_tables(&database).await })
                    .await
            };
            let routines = {
                let connection = connection.clone();
                let database = database.clone();
                runtime
                    .spawn(async move { connection.list_routines(&database).await })
                    .await
            };
            let events = {
                let connection = connection.clone();
                let database = database.clone();
                runtime
                    .spawn(async move { connection.list_events(&database).await })
                    .await
            };

            let mut objects: Vec<BackupObjectEntry> = Vec::new();
            let mut error = None;
            match tables {
                Ok(Ok(tables)) => {
                    for table in tables {
                        let kind = if matches!(table.kind, ObjectKind::View) {
                            BackupObjectKind::View
                        } else {
                            BackupObjectKind::Table
                        };
                        objects.push(BackupObjectEntry {
                            kind,
                            name: table.name,
                            selected: true,
                        });
                    }
                }
                Ok(Err(failure)) => error = Some(failure.to_string()),
                Err(failure) => error = Some(failure.to_string()),
            }
            if error.is_none() {
                match routines {
                    Ok(Ok(names)) => {
                        for name in names {
                            objects.push(BackupObjectEntry {
                                kind: BackupObjectKind::Function,
                                name,
                                selected: true,
                            });
                        }
                    }
                    Ok(Err(failure)) => error = Some(failure.to_string()),
                    Err(failure) => error = Some(failure.to_string()),
                }
            }
            if error.is_none() {
                match events {
                    Ok(Ok(names)) => {
                        for name in names {
                            objects.push(BackupObjectEntry {
                                kind: BackupObjectKind::Event,
                                name,
                                selected: true,
                            });
                        }
                    }
                    Ok(Err(failure)) => error = Some(failure.to_string()),
                    Err(failure) => error = Some(failure.to_string()),
                }
            }

            let _ = this.update(cx, |app, cx| {
                if let Some(dialog) = app.new_backup_dialog.as_mut() {
                    dialog.loading = false;
                    if let Some(error) = error {
                        dialog.error = Some(error);
                    } else {
                        dialog.objects = objects;
                        if let Some(saved) = dialog.apply.take() {
                            apply_saved_selection(dialog, &saved);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn backup_toggle_object(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.new_backup_dialog.as_mut()
            && let Some(entry) = dialog.objects.get_mut(index)
        {
            entry.selected = !entry.selected;
        }
        cx.notify();
    }

    pub(super) fn backup_toggle_kind(
        &mut self,
        kind: BackupObjectKind,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            let all = dialog
                .objects
                .iter()
                .filter(|entry| entry.kind == kind)
                .all(|entry| entry.selected);
            for entry in dialog.objects.iter_mut().filter(|entry| entry.kind == kind) {
                entry.selected = !all;
            }
        }
        cx.notify();
    }

    pub(super) fn backup_set_all(&mut self, selected: bool, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            for entry in dialog.objects.iter_mut() {
                entry.selected = selected;
            }
        }
        cx.notify();
    }

    /// Save the current object selection as a reusable configuration.
    pub(super) fn save_backup_config(&mut self, cx: &mut Context<'_, Self>) {
        let Some((name, connection_index, database, objects)) =
            self.new_backup_dialog.as_ref().map(|dialog| {
                (
                    dialog.config_name.trim().to_string(),
                    dialog.connection_index,
                    dialog.database.clone(),
                    dialog.objects.clone(),
                )
            })
        else {
            return;
        };
        if name.is_empty() {
            if let Some(dialog) = self.new_backup_dialog.as_mut() {
                dialog.error = Some(t!("backup.name_required").to_string());
            }
            cx.notify();
            return;
        }
        let connection_id = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.id.clone())
            .unwrap_or_default();
        let mut saved = SavedBackup {
            name,
            connection_id,
            database,
            comment: String::new(),
            tables: rustgrid_core::SavedBackupSelection::default(),
            views: rustgrid_core::SavedBackupSelection::default(),
            functions: rustgrid_core::SavedBackupSelection::default(),
            events: rustgrid_core::SavedBackupSelection::default(),
        };
        for kind in BackupObjectKind::ALL {
            let items: Vec<&BackupObjectEntry> =
                objects.iter().filter(|entry| entry.kind == kind).collect();
            let select_all = !items.is_empty() && items.iter().all(|entry| entry.selected);
            let selection = saved.selection_mut(kind);
            selection.select_all = select_all;
            selection.selected = items
                .iter()
                .filter(|entry| entry.selected)
                .map(|entry| entry.name.clone())
                .collect();
        }
        if let Some(index) = self.backup_configs.iter().position(|existing| {
            existing.name == saved.name
                && existing.connection_id == saved.connection_id
                && existing.database == saved.database
        }) {
            self.backup_configs[index] = saved;
            self.backup_selected = Some(BackupSelection::Config(index));
        } else {
            self.backup_configs.push(saved);
            self.backup_selected = Some(BackupSelection::Config(self.backup_configs.len() - 1));
        }
        let _ = self.config.save_backups(&self.backup_configs);
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.error = None;
            dialog
                .log
                .push(t!("backup.log.saved_config", name = dialog.config_name.trim()).to_string());
        }
        cx.notify();
    }

    /// Run a one-time backup of the selected objects, writing an NB3 file.
    pub(super) fn run_backup(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.new_backup_dialog.as_ref() else {
            return;
        };
        if dialog.running {
            return;
        }
        let selected: Vec<(BackupObjectKind, String)> = dialog
            .objects
            .iter()
            .filter(|entry| entry.selected)
            .map(|entry| (entry.kind, entry.name.clone()))
            .collect();
        let connection_index = dialog.connection_index;
        let database = dialog.database.clone();
        let comment = dialog.config_name.trim().to_string();
        if selected.is_empty() {
            if let Some(dialog) = self.new_backup_dialog.as_mut() {
                dialog.error = Some(t!("backup.no_objects").to_string());
            }
            cx.notify();
            return;
        }
        if database.is_empty() {
            if let Some(dialog) = self.new_backup_dialog.as_mut() {
                dialog.error = Some(t!("backup.no_database").to_string());
            }
            cx.notify();
            return;
        }
        let Some(connection) = self.connection_arc(connection_index) else {
            if let Some(dialog) = self.new_backup_dialog.as_mut() {
                dialog.error = Some(t!("backup.not_connected").to_string());
            }
            cx.notify();
            return;
        };
        let connection_id = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.id.clone())
            .unwrap_or_default();
        let path = self.backup_dir(&connection_id, &database).join(format!(
            "{}.{}",
            backup_timestamp(),
            rustgrid_backup::FILE_EXTENSION
        ));
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }

        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.running = true;
            dialog.error = None;
            dialog.tab = BackupDialogTab::Log;
            dialog.log.clear();
            dialog.log.push(
                t!(
                    "backup.log.start",
                    count = selected.len(),
                    database = database.clone()
                )
                .to_string(),
            );
        }

        let runtime = self.runtime.clone();
        let schema = database.clone();
        let writer = match rustgrid_backup::BackupWriter::create(&path, &schema, &comment) {
            Ok(writer) => Arc::new(Mutex::new(writer)),
            Err(error) => {
                if let Some(dialog) = self.new_backup_dialog.as_mut() {
                    dialog.running = false;
                    dialog.error = Some(error.to_string());
                }
                cx.notify();
                return;
            }
        };

        cx.spawn(async move |this, cx| {
            let mut failure: Option<String> = None;
            let mut written = 0usize;
            for (kind, name) in selected {
                // Metadata first (DDL, columns, triggers), then stream the table's rows.
                let meta = {
                    let connection = connection.clone();
                    let database = database.clone();
                    let call_name = name.clone();
                    match runtime
                        .spawn(async move {
                            connection
                                .backup_object_metadata(&database, kind, &call_name)
                                .await
                        })
                        .await
                    {
                        Ok(Ok(meta)) => meta,
                        Ok(Err(error)) => {
                            failure = Some(error.to_string());
                            break;
                        }
                        Err(error) => {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                };
                if let Err(error) = writer.lock().unwrap().begin_object(&meta) {
                    failure = Some(error.to_string());
                    break;
                }

                let mut rows = 0u64;
                if kind == BackupObjectKind::Table {
                    let connection = connection.clone();
                    let database = database.clone();
                    let call_name = name.clone();
                    // The writer is shared so the driver's synchronous row callback can write
                    // straight through, while the future is `'static` and `Send`.
                    let sink = Arc::clone(&writer);
                    let result = runtime
                        .spawn(async move {
                            let mut on_row = |tuple: &str| {
                                sink.lock().unwrap().write_row(tuple).map_err(Error::other)
                            };
                            connection
                                .stream_table_rows(&database, &call_name, &mut on_row)
                                .await
                        })
                        .await;
                    match result {
                        Ok(Ok(count)) => rows = count,
                        Ok(Err(error)) => {
                            failure = Some(error.to_string());
                            break;
                        }
                        Err(error) => {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                }

                if let Err(error) = writer.lock().unwrap().end_object() {
                    failure = Some(error.to_string());
                    break;
                }
                written += 1;
                let message = t!("backup.log.object", name = name.clone(), rows = rows).to_string();
                let _ = this.update(cx, |app, cx| {
                    app.push_backup_log(message);
                    cx.notify();
                });
            }

            if failure.is_none()
                && let Err(error) = writer.lock().unwrap().finish()
            {
                failure = Some(error.to_string());
            }

            let result = match failure {
                Some(error) => Err(error),
                None => Ok(written),
            };
            let _ = this.update(cx, |app, cx| app.finish_backup(result, cx));
        })
        .detach();
    }

    fn push_backup_log(&mut self, message: String) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.log.push(message);
        }
    }

    fn finish_backup(&mut self, result: Result<usize, String>, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.running = false;
            match &result {
                Ok(count) => dialog
                    .log
                    .push(t!("backup.log.done", count = *count).to_string()),
                Err(error) => {
                    dialog.error = Some(error.clone());
                    dialog.log.push(error.clone());
                }
            }
        }
        if result.is_ok() {
            self.refresh_backups(cx);
            if !self.backup_files.is_empty() {
                self.backup_selected = Some(BackupSelection::File(0));
            }
        }
        cx.notify();
    }

    pub(super) fn close_new_backup(&mut self, cx: &mut Context<'_, Self>) {
        self.new_backup_dialog = None;
        self.backup_name_input = None;
        cx.notify();
    }

    // ----- Restore dialog ---------------------------------------------------------------------

    pub(super) fn open_restore_backup(&mut self, file_index: usize, cx: &mut Context<'_, Self>) {
        let Some(file) = self.backup_files.get(file_index) else {
            return;
        };
        let connection_index = self
            .connections
            .iter()
            .position(|node| {
                node.profile.id == file.connection_id
                    && matches!(node.status, ConnectionStatus::Connected(_))
            })
            .or_else(|| self.default_query_connection(cx));
        let database = file.manifest.schema.clone();
        let objects: Vec<BackupObjectEntry> = file
            .manifest
            .objects
            .iter()
            .map(|object| BackupObjectEntry {
                kind: object.kind,
                name: object.name.clone(),
                selected: true,
            })
            .collect();
        self.restore_dialog = Some(RestoreBackupDialog {
            file_index,
            connection_index,
            database,
            tab: BackupDialogTab::Objects,
            objects,
            running: false,
            log: Vec::new(),
            error: None,
        });
        cx.notify();
    }

    pub(super) fn restore_toggle_object(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.restore_dialog.as_mut()
            && let Some(entry) = dialog.objects.get_mut(index)
        {
            entry.selected = !entry.selected;
        }
        cx.notify();
    }

    pub(super) fn restore_toggle_kind(
        &mut self,
        kind: BackupObjectKind,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.restore_dialog.as_mut() {
            let all = dialog
                .objects
                .iter()
                .filter(|entry| entry.kind == kind)
                .all(|entry| entry.selected);
            for entry in dialog.objects.iter_mut().filter(|entry| entry.kind == kind) {
                entry.selected = !all;
            }
        }
        cx.notify();
    }

    pub(super) fn restore_set_all(&mut self, selected: bool, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.restore_dialog.as_mut() {
            for entry in dialog.objects.iter_mut() {
                entry.selected = selected;
            }
        }
        cx.notify();
    }

    pub(super) fn run_restore(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.restore_dialog.as_ref() else {
            return;
        };
        if dialog.running {
            return;
        }
        let selected: Vec<(BackupObjectKind, String)> = dialog
            .objects
            .iter()
            .filter(|entry| entry.selected)
            .map(|entry| (entry.kind, entry.name.clone()))
            .collect();
        let file_index = dialog.file_index;
        let connection_index = dialog.connection_index;
        let database = dialog.database.clone();
        if selected.is_empty() {
            if let Some(dialog) = self.restore_dialog.as_mut() {
                dialog.error = Some(t!("backup.no_objects").to_string());
            }
            cx.notify();
            return;
        }
        if database.is_empty() {
            if let Some(dialog) = self.restore_dialog.as_mut() {
                dialog.error = Some(t!("backup.no_database").to_string());
            }
            cx.notify();
            return;
        }
        let Some(connection) = connection_index.and_then(|index| self.connection_arc(index)) else {
            if let Some(dialog) = self.restore_dialog.as_mut() {
                dialog.error = Some(t!("backup.not_connected").to_string());
            }
            cx.notify();
            return;
        };
        let Some(file) = self.backup_files.get(file_index) else {
            return;
        };
        let path = file.path.clone();

        if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog.running = true;
            dialog.error = None;
            dialog.tab = BackupDialogTab::Log;
            dialog.log.clear();
            dialog.log.push(
                t!(
                    "backup.log.restore_start",
                    count = selected.len(),
                    database = database.clone()
                )
                .to_string(),
            );
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            // Open the archive once; each object is read (and decompressed) on demand so peak
            // memory stays at one table instead of every selected table.
            let reader_path = path.clone();
            let reader = match runtime
                .spawn(async move { rustgrid_backup::BackupReader::open(&reader_path) })
                .await
            {
                Ok(Ok(reader)) => Arc::new(reader),
                Ok(Err(error)) => {
                    let _ = this.update(cx, |app, cx| {
                        app.finish_restore(Err(error.to_string()), connection_index, cx)
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        app.finish_restore(Err(error.to_string()), connection_index, cx)
                    });
                    return;
                }
            };

            let mut restored = 0usize;
            for (kind, name) in selected {
                let reader = Arc::clone(&reader);
                let read_name = name.clone();
                let object = match runtime
                    .spawn(async move { reader.read_object(kind, &read_name) })
                    .await
                {
                    Ok(Ok(Some(object))) => object,
                    // The manifest listed it, so a missing object is not fatal; skip it.
                    Ok(Ok(None)) => continue,
                    Ok(Err(error)) => {
                        let _ = this.update(cx, |app, cx| {
                            app.finish_restore(Err(error.to_string()), connection_index, cx)
                        });
                        return;
                    }
                    Err(error) => {
                        let _ = this.update(cx, |app, cx| {
                            app.finish_restore(Err(error.to_string()), connection_index, cx)
                        });
                        return;
                    }
                };
                let dump = object.to_object_dump();
                let connection = connection.clone();
                let database = database.clone();
                let result = match runtime
                    .spawn(async move { connection.restore_object(&database, &dump).await })
                    .await
                {
                    Ok(inner) => inner,
                    Err(error) => Err(Error::other(error)),
                };
                match result {
                    Ok(()) => {
                        restored += 1;
                        let message = t!("backup.log.restored", name = name.clone()).to_string();
                        let _ = this.update(cx, |app, cx| {
                            app.push_restore_log(message);
                            cx.notify();
                        });
                    }
                    Err(error) => {
                        let _ = this.update(cx, |app, cx| {
                            app.finish_restore(Err(error.to_string()), connection_index, cx)
                        });
                        return;
                    }
                }
            }
            let _ = this.update(cx, |app, cx| {
                app.finish_restore(Ok(restored), connection_index, cx)
            });
        })
        .detach();
    }

    fn push_restore_log(&mut self, message: String) {
        if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog.log.push(message);
        }
    }

    fn finish_restore(
        &mut self,
        result: Result<usize, String>,
        connection_index: Option<usize>,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog.running = false;
            match &result {
                Ok(count) => dialog
                    .log
                    .push(t!("backup.log.restore_done", count = *count).to_string()),
                Err(error) => {
                    dialog.error = Some(error.clone());
                    dialog.log.push(error.clone());
                }
            }
        }
        // Refresh the tree so restored tables appear (or the schema reflects the change).
        if let Some(connection_index) = connection_index {
            self.load_databases(connection_index, cx);
        }
        cx.notify();
    }

    pub(super) fn close_restore(&mut self, cx: &mut Context<'_, Self>) {
        self.restore_dialog = None;
        cx.notify();
    }

    // ----- Combined dropdowns -----------------------------------------------------------------

    pub(super) fn ensure_backup_combos(&mut self, cx: &mut Context<'_, Self>) {
        let theme = self.theme;
        if self.backup_connection_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 220.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ =
                            weak.update(cx, |app, cx| app.backup_connection_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/connection.svg", theme.icon_connection, cx);
            });
            self.backup_connection_combo = Some(combo);
        }
        if self.backup_database_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 220.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.backup_database_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/database.svg", theme.icon_database, cx);
            });
            self.backup_database_combo = Some(combo);
        }
        if self.restore_connection_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 220.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ =
                            weak.update(cx, |app, cx| app.restore_connection_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/connection.svg", theme.icon_connection, cx);
            });
            self.restore_connection_combo = Some(combo);
        }
        if self.restore_database_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 220.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.restore_database_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/database.svg", theme.icon_database, cx);
            });
            self.restore_database_combo = Some(combo);
        }
    }

    pub(super) fn sync_backup_combos(&mut self, cx: &mut Context<'_, Self>) {
        if let Some((connection_index, database)) = self
            .new_backup_dialog
            .as_ref()
            .map(|dialog| (dialog.connection_index, dialog.database.clone()))
        {
            let connection_options = self
                .connected_connection_options()
                .into_iter()
                .map(|(value, label)| ComboOption::new(value, label))
                .collect();
            let database_options = self
                .query_database_options(connection_index)
                .into_iter()
                .map(|(value, label)| ComboOption::new(value, label))
                .collect();
            let selected = connection_index.to_string();
            if let Some(combo) = self.backup_connection_combo.clone() {
                combo.update(cx, |combo, cx| {
                    combo.set_options(connection_options, cx);
                    combo.set_selected(selected, cx);
                });
            }
            if let Some(combo) = self.backup_database_combo.clone() {
                combo.update(cx, |combo, cx| {
                    combo.set_options(database_options, cx);
                    combo.set_selected(database, cx);
                });
            }
        }
        if let Some((connection_index, database)) = self
            .restore_dialog
            .as_ref()
            .map(|dialog| (dialog.connection_index, dialog.database.clone()))
        {
            let connection_options = self
                .connected_connection_options()
                .into_iter()
                .map(|(value, label)| ComboOption::new(value, label))
                .collect();
            let database_options = connection_index
                .map(|index| self.query_database_options(index))
                .unwrap_or_default()
                .into_iter()
                .map(|(value, label)| ComboOption::new(value, label))
                .collect();
            let selected = connection_index
                .map(|index| index.to_string())
                .unwrap_or_default();
            if let Some(combo) = self.restore_connection_combo.clone() {
                combo.update(cx, |combo, cx| {
                    combo.set_options(connection_options, cx);
                    combo.set_selected(selected, cx);
                });
            }
            if let Some(combo) = self.restore_database_combo.clone() {
                combo.update(cx, |combo, cx| {
                    combo.set_options(database_options, cx);
                    combo.set_selected(database, cx);
                });
            }
        }
    }

    pub(super) fn backup_connection_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        let Ok(index) = value.parse::<usize>() else {
            return;
        };
        let database = self.default_query_database(index, cx).unwrap_or_default();
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.connection_index = index;
            dialog.database = database;
            dialog.objects.clear();
        }
        self.load_backup_objects(cx);
        cx.notify();
    }

    pub(super) fn backup_database_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.database = value.to_string();
            dialog.objects.clear();
        }
        self.load_backup_objects(cx);
        cx.notify();
    }

    pub(super) fn restore_connection_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        let Ok(index) = value.parse::<usize>() else {
            return;
        };
        let database = self.default_query_database(index, cx).unwrap_or_default();
        if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog.connection_index = Some(index);
            dialog.database = database;
        }
        cx.notify();
    }

    pub(super) fn restore_database_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog.database = value.to_string();
        }
        cx.notify();
    }

    // ----- Dialog chrome ----------------------------------------------------------------------

    pub(super) fn open_new_backup_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (title, footer) = app.update(cx, |app, cx| {
                (
                    t!("backup.new_title").to_string(),
                    app.new_backup_dialog_footer(cx).into_any_element(),
                )
            });
            let on_close = app.downgrade();
            let content_app = app.clone();
            dialog
                .title(title)
                .w(px(560.0))
                .margin_top(centered_margin_top(window, 470.0))
                .content(move |content, _window, cx| {
                    let body = content_app.update(cx, |app, cx| {
                        app.new_backup_dialog_body(cx).into_any_element()
                    });
                    content.child(body)
                })
                .footer(footer)
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.close_new_backup(cx));
                })
        });
    }

    pub(super) fn open_restore_backup_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (title, footer) = app.update(cx, |app, cx| {
                let name = app
                    .backup_files
                    .get(
                        app.restore_dialog
                            .as_ref()
                            .map(|d| d.file_index)
                            .unwrap_or(0),
                    )
                    .map(|file| file.name.clone())
                    .unwrap_or_default();
                (
                    format!("{} - {}", name, t!("backup.restore_title")),
                    app.restore_dialog_footer(cx).into_any_element(),
                )
            });
            let on_close = app.downgrade();
            let content_app = app.clone();
            dialog
                .title(title)
                .w(px(560.0))
                .margin_top(centered_margin_top(window, 470.0))
                .content(move |content, _window, cx| {
                    let body = content_app
                        .update(cx, |app, cx| app.restore_dialog_body(cx).into_any_element());
                    content.child(body)
                })
                .footer(footer)
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.close_restore(cx));
                })
        });
    }

    fn new_backup_dialog_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let active = self
            .new_backup_dialog
            .as_ref()
            .map(|dialog| dialog.tab)
            .unwrap_or(BackupDialogTab::Objects);
        let tabs = div()
            .flex()
            .flex_row()
            .w_full()
            .gap_0p5()
            .px_2()
            .pt_2()
            .bg(rgb(theme.dialog_face))
            .child(self.backup_tab_button(
                "backup-tab-objects",
                t!("backup.tab.objects").to_string(),
                active == BackupDialogTab::Objects,
                cx.listener(|this, _event, _window, cx| {
                    this.set_backup_tab(BackupDialogTab::Objects, cx)
                }),
            ))
            .child(self.backup_tab_button(
                "backup-tab-log",
                t!("backup.tab.log").to_string(),
                active == BackupDialogTab::Log,
                cx.listener(|this, _event, _window, cx| {
                    this.set_backup_tab(BackupDialogTab::Log, cx)
                }),
            ));

        let content = match active {
            BackupDialogTab::Objects => self.render_backup_picker_body(cx),
            BackupDialogTab::Log => render_backup_log(
                self.new_backup_dialog
                    .as_ref()
                    .map(|dialog| dialog.log.as_slice())
                    .unwrap_or(&[]),
                self.new_backup_dialog
                    .as_ref()
                    .map(|dialog| dialog.running)
                    .unwrap_or(false),
                theme,
            ),
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .child(tabs)
            .child(content)
            .into_any_element()
    }

    fn render_backup_picker_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.new_backup_dialog.as_ref() else {
            return div().into_any_element();
        };
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("backup.connection").to_string()),
            )
            .child(match self.backup_connection_combo.clone() {
                Some(combo) => div().w(px(200.0)).child(combo).into_any_element(),
                None => div().into_any_element(),
            })
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("backup.database").to_string()),
            )
            .child(match self.backup_database_combo.clone() {
                Some(combo) => div().w(px(200.0)).child(combo).into_any_element(),
                None => div().into_any_element(),
            });

        let picker = render_object_picker(&dialog.objects, dialog.loading, theme, false, cx);

        let buttons = div()
            .flex()
            .flex_row()
            .gap_2()
            .mx_2()
            .mb_2()
            .child(self.dialog_button(
                "backup-select-all",
                t!("backup.select_all").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.backup_set_all(true, cx)),
            ))
            .child(self.dialog_button(
                "backup-select-none",
                t!("backup.select_none").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.backup_set_all(false, cx)),
            ));

        let mut body = div().flex().flex_col().w_full().child(header);
        body = body.child(picker);
        if let Some(error) = dialog.error.as_ref() {
            body = body.child(
                div()
                    .px_2()
                    .pb_1()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.danger))
                    .child(error.clone()),
            );
        }
        body.child(buttons).into_any_element()
    }

    fn new_backup_dialog_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let name_input = match self.backup_name_input.clone() {
            Some(input) => div()
                .w(px(200.0))
                .h(px(24.0))
                .child(input)
                .into_any_element(),
            None => div().into_any_element(),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .h(px(46.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!("backup.config_name").to_string()),
                    )
                    .child(name_input),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_none()
                    .child(self.dialog_button(
                        "backup-save-config",
                        t!("backup.save").to_string(),
                        false,
                        cx.listener(|this, _event, _window, cx| this.save_backup_config(cx)),
                    ))
                    .child(self.dialog_button(
                        "backup-run",
                        t!("backup.backup").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| {
                            if !this
                                .new_backup_dialog
                                .as_ref()
                                .map(|dialog| dialog.running)
                                .unwrap_or(false)
                            {
                                this.run_backup(cx);
                            }
                        }),
                    ))
                    .child(self.dialog_button(
                        "backup-close",
                        t!("backup.close").to_string(),
                        false,
                        cx.listener(|this, _event, _window, cx| this.close_new_backup(cx)),
                    )),
            )
            .into_any_element()
    }

    fn restore_dialog_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let active = self
            .restore_dialog
            .as_ref()
            .map(|dialog| dialog.tab)
            .unwrap_or(BackupDialogTab::Objects);
        let tabs = div()
            .flex()
            .flex_row()
            .w_full()
            .gap_0p5()
            .px_2()
            .pt_2()
            .bg(rgb(theme.dialog_face))
            .child(self.backup_tab_button(
                "restore-tab-objects",
                t!("backup.tab.objects").to_string(),
                active == BackupDialogTab::Objects,
                cx.listener(|this, _event, _window, cx| {
                    this.set_restore_tab(BackupDialogTab::Objects, cx)
                }),
            ))
            .child(self.backup_tab_button(
                "restore-tab-log",
                t!("backup.tab.log").to_string(),
                active == BackupDialogTab::Log,
                cx.listener(|this, _event, _window, cx| {
                    this.set_restore_tab(BackupDialogTab::Log, cx)
                }),
            ));

        let content = match active {
            BackupDialogTab::Objects => self.render_restore_picker_body(cx),
            BackupDialogTab::Log => render_backup_log(
                self.restore_dialog
                    .as_ref()
                    .map(|dialog| dialog.log.as_slice())
                    .unwrap_or(&[]),
                self.restore_dialog
                    .as_ref()
                    .map(|dialog| dialog.running)
                    .unwrap_or(false),
                theme,
            ),
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .child(tabs)
            .child(content)
            .into_any_element()
    }

    fn render_restore_picker_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.restore_dialog.as_ref() else {
            return div().into_any_element();
        };
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_2()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("backup.restore_to").to_string()),
            )
            .child(match self.restore_connection_combo.clone() {
                Some(combo) => div().w(px(200.0)).child(combo).into_any_element(),
                None => div().into_any_element(),
            })
            .child(match self.restore_database_combo.clone() {
                Some(combo) => div().w(px(200.0)).child(combo).into_any_element(),
                None => div().into_any_element(),
            });

        let picker = render_object_picker(&dialog.objects, false, theme, true, cx);

        let buttons = div()
            .flex()
            .flex_row()
            .gap_2()
            .mx_2()
            .mb_2()
            .child(self.dialog_button(
                "restore-select-all",
                t!("backup.select_all").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.restore_set_all(true, cx)),
            ))
            .child(self.dialog_button(
                "restore-select-none",
                t!("backup.select_none").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.restore_set_all(false, cx)),
            ));

        let mut body = div().flex().flex_col().w_full().child(header).child(picker);
        if let Some(error) = dialog.error.as_ref() {
            body = body.child(
                div()
                    .px_2()
                    .pb_1()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.danger))
                    .child(error.clone()),
            );
        }
        body.child(buttons).into_any_element()
    }

    fn restore_dialog_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .w_full()
            .h(px(46.0))
            .gap_2()
            .child(self.dialog_button(
                "restore-run",
                t!("backup.restore_button").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| {
                    if !this
                        .restore_dialog
                        .as_ref()
                        .map(|dialog| dialog.running)
                        .unwrap_or(false)
                    {
                        this.run_restore(cx);
                    }
                }),
            ))
            .child(self.dialog_button(
                "restore-close",
                t!("backup.close").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.close_restore(cx)),
            ))
            .into_any_element()
    }

    pub(super) fn set_backup_tab(&mut self, tab: BackupDialogTab, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.tab = tab;
        }
        cx.notify();
    }

    pub(super) fn set_restore_tab(&mut self, tab: BackupDialogTab, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog.tab = tab;
        }
        cx.notify();
    }

    fn backup_tab_button(
        &self,
        id: &'static str,
        label: String,
        active: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .px_4()
            .h(px(24.0))
            .text_size(px(12.0))
            .cursor_pointer()
            .when(active, move |style| {
                style
                    .bg(rgb(theme.dialog_bg))
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text))
                    .font_weight(FontWeight::SEMIBOLD)
                    .mb(px(-1.0))
            })
            .when(!active, move |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.text_color(rgb(theme.text)))
            })
            .on_click(on_click)
            .child(label)
    }
}

// ----- Free helpers ---------------------------------------------------------------------------

/// Build the configuration-name field of the New Backup dialog.
fn make_backup_name_input(
    theme: Theme,
    initial: String,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    cx.new(move |cx| {
        TextInput::new(theme, initial, TextInputOptions::default(), cx).on_change(Rc::new(
            move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| {
                    if let Some(dialog) = app.new_backup_dialog.as_mut() {
                        dialog.config_name = text.to_string();
                    }
                    cx.notify();
                });
            },
        ))
    })
}

/// Apply a saved configuration's selection to a freshly loaded object list.
fn apply_saved_selection(dialog: &mut NewBackupDialog, saved: &SavedBackup) {
    for entry in dialog.objects.iter_mut() {
        entry.selected = saved.selection(entry.kind).contains(&entry.name);
    }
}

/// One list row of the backup list: an icon, a title and a muted subtitle.
#[allow(clippy::too_many_arguments)]
fn backup_row(
    id: SharedString,
    icon: &'static str,
    icon_color: u32,
    title: String,
    subtitle: String,
    selected: bool,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .w_full()
        .h(px(26.0))
        .px_2()
        .cursor_pointer()
        .when(selected, move |style| {
            style
                .bg(rgb(theme.tree_selected_bg))
                .text_color(rgb(theme.tree_selected_text))
        })
        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
        .on_click(on_click)
        .child(tree_icon(icon, icon_color))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(theme.text_muted))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(subtitle),
                ),
        )
}

/// The details pane's heading: a large icon, the name and its kind.
fn backup_detail_header(
    icon: &'static str,
    icon_color: u32,
    name: String,
    kind: String,
    theme: Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .child(
            svg()
                .path(icon)
                .w(px(48.0))
                .h(px(48.0))
                .flex_none()
                .text_color(rgb(icon_color)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(kind),
                ),
        )
}

/// One label/value block of the details pane.
fn backup_detail_field(label: String, value: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(theme.text_muted))
                .child(label),
        )
        .child(div().text_size(px(12.5)).child(value))
}

/// The shared object-selection tree of the New Backup and Restore dialogs.
fn render_object_picker(
    objects: &[BackupObjectEntry],
    loading: bool,
    theme: Theme,
    restore: bool,
    cx: &mut Context<'_, AppView>,
) -> AnyElement {
    let mut list = div().flex().flex_col().gap_0p5().p_2();
    if loading {
        list = list.child(
            div()
                .p_2()
                .text_color(rgb(theme.text_muted))
                .child(t!("common.loading").to_string()),
        );
    } else if objects.is_empty() {
        list = list.child(
            div()
                .p_2()
                .text_color(rgb(theme.text_muted))
                .child(t!("common.empty").to_string()),
        );
    } else {
        for (kind_index, kind) in BackupObjectKind::ALL.into_iter().enumerate() {
            let items: Vec<(usize, &BackupObjectEntry)> = objects
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.kind == kind)
                .collect();
            if items.is_empty() {
                continue;
            }
            let total = items.len();
            let selected = items.iter().filter(|(_, entry)| entry.selected).count();
            let all = selected == total;
            list = list.child(
                div()
                    .id(SharedString::from(format!("backup-kind-{kind_index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(BACKUP_OBJECT_ROW_HEIGHT))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        if restore {
                            this.restore_toggle_kind(kind, cx);
                        } else {
                            this.backup_toggle_kind(kind, cx);
                        }
                    }))
                    .child(checkbox_box(all, theme))
                    .child(tree_icon(
                        backup_object_icon(kind),
                        backup_object_color(kind, theme),
                    ))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child(format!("{} ({selected}/{total})", t!(kind.label_key()))),
                    ),
            );
            for (index, entry) in items {
                let name = entry.name.clone();
                let selected = entry.selected;
                list = list.child(
                    div()
                        .id(SharedString::from(format!("backup-object-{index}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .h(px(BACKUP_OBJECT_ROW_HEIGHT))
                        .pl(px(22.0))
                        .cursor_pointer()
                        .when(selected, move |style| style.bg(rgb(theme.tree_hover_bg)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            if restore {
                                this.restore_toggle_object(index, cx);
                            } else {
                                this.backup_toggle_object(index, cx);
                            }
                        }))
                        .child(checkbox_box(selected, theme))
                        .child(tree_icon(
                            backup_object_icon(kind),
                            backup_object_color(kind, theme),
                        ))
                        .child(div().text_size(px(12.0)).child(name)),
                );
            }
        }
    }

    let inner = div()
        .id("backup-object-picker")
        .flex()
        .flex_col()
        .h(px(340.0))
        .overflow_y_scroll()
        .child(list);
    div()
        .flex()
        .flex_col()
        .mx_2()
        .mb_2()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.dialog_bg))
        .child(inner)
        .into_any_element()
}

/// The information-log tab body shared by both dialogs.
fn render_backup_log(log: &[String], running: bool, theme: Theme) -> AnyElement {
    let mut content = div().flex().flex_col().gap_0p5().p_2();
    if running {
        content = content.child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme.primary))
                .child(t!("backup.running").to_string()),
        );
    }
    if log.is_empty() {
        content = content.child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child("--"),
        );
    } else {
        for line in log {
            content = content.child(div().text_size(px(12.0)).child(line.clone()));
        }
    }
    div()
        .flex()
        .flex_col()
        .mx_2()
        .mb_2()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.dialog_bg))
        .child(
            div()
                .id("backup-log")
                .flex()
                .flex_col()
                .h(px(360.0))
                .overflow_y_scroll()
                .child(content),
        )
        .into_any_element()
}

fn backup_object_icon(kind: BackupObjectKind) -> &'static str {
    match kind {
        BackupObjectKind::Table => "icons/tables.svg",
        BackupObjectKind::View => "icons/views.svg",
        BackupObjectKind::Function => "icons/functions.svg",
        BackupObjectKind::Event => "icons/events.svg",
    }
}

fn backup_object_color(kind: BackupObjectKind, theme: Theme) -> u32 {
    match kind {
        BackupObjectKind::Table => theme.icon_table,
        BackupObjectKind::View => theme.icon_view,
        BackupObjectKind::Function => theme.icon_functions,
        BackupObjectKind::Event => theme.icon_backups,
    }
}

fn backup_object_counts(manifest: &BackupManifest) -> String {
    let mut counts = [0usize; 4];
    for object in &manifest.objects {
        counts[kind_index(object.kind)] += 1;
    }
    format_counts(&counts)
}

fn saved_config_counts(config: &SavedBackup) -> String {
    BackupObjectKind::ALL
        .into_iter()
        .map(|kind| {
            let selection = config.selection(kind);
            let value = if selection.select_all {
                "*".to_string()
            } else {
                selection.selected.len().to_string()
            };
            format!("{}: {value}", t!(kind.label_key()))
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn format_counts(counts: &[usize; 4]) -> String {
    BackupObjectKind::ALL
        .into_iter()
        .map(|kind| format!("{}: {}", t!(kind.label_key()), counts[kind_index(kind)]))
        .collect::<Vec<_>>()
        .join("  ")
}

fn kind_index(kind: BackupObjectKind) -> usize {
    match kind {
        BackupObjectKind::Table => 0,
        BackupObjectKind::View => 1,
        BackupObjectKind::Function => 2,
        BackupObjectKind::Event => 3,
    }
}

fn backup_timestamp() -> String {
    chrono::Local::now().format("%Y%m%d%H%M%S").to_string()
}

/// Format a unix-seconds timestamp for the details pane.
fn format_unix_seconds(seconds: u64) -> String {
    let Ok(value) = i64::try_from(seconds) else {
        return seconds.to_string();
    };
    match chrono::DateTime::from_timestamp(value, 0) {
        Some(time) => time
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        None => seconds.to_string(),
    }
}

fn format_system_time(time: std::time::SystemTime) -> String {
    let time: chrono::DateTime<chrono::Local> = time.into();
    time.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn format_size(bytes: u64) -> String {
    let (value, unit) = if bytes >= 1024 * 1024 * 1024 {
        (bytes as f64 / (1024.0 * 1024.0 * 1024.0), "GB")
    } else if bytes >= 1024 * 1024 {
        (bytes as f64 / (1024.0 * 1024.0), "MB")
    } else if bytes >= 1024 {
        (bytes as f64 / 1024.0, "KB")
    } else {
        return format!("{bytes} B ({bytes})");
    };
    format!("{value:.2} {unit} ({})", format_thousands(bytes))
}

fn format_thousands(value: u64) -> String {
    let digits = value.to_string();
    let length = digits.len();
    let mut out = String::with_capacity(length + length / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (length - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(character);
    }
    out
}

/// Make a filesystem-safe path component from a connection id or database name.
fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => character,
        })
        .collect()
}
