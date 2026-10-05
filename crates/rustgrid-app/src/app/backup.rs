//! The Backup main tab: RustGrid `.rgbak` backups.
//!
//! `AppView` owns the state (the backup list, the "New Backup" and "Restore Backup" dialogs);
//! this module renders it and drives the work through [`rustgrid_backup`] and the
//! [`rustgrid_core::Connection`] trait. Files are written under the app config dir's
//! `backups/<connection-id>/<database>/` tree.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

use super::info_pane::info_panel;
use super::*;
use rustgrid_backup::BackupManifest;
use rustgrid_core::{BackupObjectKind, ObjectKind, SavedBackup};

/// Rows in the New Backup dialog's object tree are `26px` tall.
const BACKUP_OBJECT_ROW_HEIGHT: f32 = 26.0;

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

    /// The database the Backup tab is scoped to, taken from the connection tree's selection.
    /// Backups are tied to a database, so this only resolves when the tree has a selected (or
    /// Backups-category) database row whose connection is open and whose database is opened.
    pub(super) fn backup_scope(&self, cx: &App) -> Option<(usize, usize)> {
        let selected = self.tree_pane.read(cx).selected.clone()?;
        // A database row scopes directly; the Backups category row scopes its database.
        let parsed: Option<(usize, usize)> = if let Some(rest) = selected.strip_prefix("db-") {
            let mut parts = rest.splitn(2, '-');
            Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
        } else if let Some(rest) = selected.strip_prefix("cat-") {
            let mut parts = rest.splitn(3, '-');
            let pair = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
            (parts.next()? == "b").then_some(pair)
        } else {
            None
        };
        let (connection_index, database_index) = parsed?;
        let node = self.connections.get(connection_index)?;
        if !matches!(node.status, ConnectionStatus::Connected(_)) {
            return None;
        }
        let Loadable::Loaded(databases) = &node.databases else {
            return None;
        };
        let database = databases.get(database_index)?;
        database
            .opened
            .then_some((connection_index, database_index))
    }

    /// `(connection_id, database)` of the current scope, for filtering the scanned files.
    fn backup_scope_names(&self, cx: &App) -> Option<(String, String)> {
        let (connection_index, database_index) = self.backup_scope(cx)?;
        let connection_id = self.connections.get(connection_index)?.profile.id.clone();
        let database = self.database_name(connection_index, database_index)?;
        Some((connection_id, database))
    }

    /// Indices into `backup_files` that belong to the current scope, in list order.
    pub(super) fn visible_backup_files(&self, cx: &App) -> Vec<usize> {
        let Some((connection_id, database)) = self.backup_scope_names(cx) else {
            return Vec::new();
        };
        let needle = self.backup_search.trim().to_lowercase();
        self.backup_files
            .iter()
            .enumerate()
            .filter(|(_, file)| {
                file.connection_id == connection_id
                    && file.database == database
                    && (needle.is_empty() || file.name.to_lowercase().contains(&needle))
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Indices into `backup_configs` that belong to the current scope, in list order.
    pub(super) fn visible_backup_configs(&self, cx: &App) -> Vec<usize> {
        let Some((connection_id, database)) = self.backup_scope_names(cx) else {
            return Vec::new();
        };
        let needle = self.backup_search.trim().to_lowercase();
        self.backup_configs
            .iter()
            .enumerate()
            .filter(|(_, config)| {
                config.connection_id == connection_id
                    && config.database == database
                    && (needle.is_empty() || config.name.to_lowercase().contains(&needle))
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// One backup row hit (click) under the click's modifier mode.
    pub(super) fn hit_backup(
        &mut self,
        key: &str,
        _click_count: usize,
        modifiers: Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        if self.backup_rename.is_some() {
            return;
        }
        let visible: Vec<String> = self.backup_visible_keys(cx).into_iter().collect();
        if modifiers.shift {
            self.backups_selection.extend_to(&visible, key);
        } else {
            self.backups_selection
                .hit(&visible, key, selection_mode(modifiers));
        }
        self.sync_backup_selected();
        cx.notify();
    }

    /// The visible backup entries' selection keys, in list order.
    fn backup_visible_keys(&self, cx: &App) -> Vec<String> {
        let mut keys = Vec::new();
        for index in self.visible_backup_files(cx) {
            keys.push(backup_file_key(&self.backup_files[index].name));
        }
        for index in self.visible_backup_configs(cx) {
            keys.push(backup_config_key(&self.backup_configs[index].name));
        }
        keys
    }

    /// Keep the single-selection mirror (`backup_selected`) in step with the multi-selection.
    fn sync_backup_selected(&mut self) {
        self.backup_selected = match self.backups_selection.single() {
            Some(key) => match backup_entry_from_key(key) {
                Some(BackupEntry::File(_)) => self
                    .backup_files
                    .iter()
                    .position(|file| backup_file_key(&file.name) == key)
                    .map(BackupSelection::File),
                Some(BackupEntry::Config(_)) => self
                    .backup_configs
                    .iter()
                    .position(|config| backup_config_key(&config.name) == key)
                    .map(BackupSelection::Config),
                None => None,
            },
            None => None,
        };
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
                    .bg(rgb(theme.editor_bg))
                    .child(self.render_backup_list(cx)),
            )
            .into_any_element()
    }

    /// Build the object-info body for a selected backup file or saved profile.
    pub(super) fn backup_info_body(&self, selected: BackupSelection, theme: Theme) -> AnyElement {
        match selected {
            BackupSelection::File(index) => {
                let Some(file) = self.backup_files.get(index) else {
                    return div().into_any_element();
                };
                let fields = [
                    (
                        t!("backup.field.file").to_string(),
                        file.path.display().to_string(),
                    ),
                    (t!("backup.field.size").to_string(), format_size(file.size)),
                    (
                        t!("backup.field.created").to_string(),
                        format_unix_seconds(file.manifest.created_unix),
                    ),
                    (
                        t!("backup.field.modified").to_string(),
                        file.modified
                            .map(format_system_time)
                            .unwrap_or_else(|| "--".to_string()),
                    ),
                    (
                        t!("backup.field.version").to_string(),
                        file.manifest.backup_version.clone(),
                    ),
                    (
                        t!("backup.field.comment").to_string(),
                        if file.manifest.comment.is_empty() {
                            "--".to_string()
                        } else {
                            file.manifest.comment.clone()
                        },
                    ),
                    (
                        t!("backup.field.database").to_string(),
                        file.database.clone(),
                    ),
                    (
                        t!("backup.field.connection").to_string(),
                        self.connection_name_by_id(&file.connection_id),
                    ),
                    (
                        t!("backup.field.objects").to_string(),
                        backup_object_counts(&file.manifest),
                    ),
                ];
                info_panel(
                    "icons/backups.svg",
                    theme.icon_backups,
                    &file.name,
                    &t!("common.backup"),
                    &fields,
                )
            }
            BackupSelection::Config(index) => {
                let Some(config) = self.backup_configs.get(index) else {
                    return div().into_any_element();
                };
                let fields = [
                    (
                        t!("backup.field.connection").to_string(),
                        self.connection_name_by_id(&config.connection_id),
                    ),
                    (
                        t!("backup.field.database").to_string(),
                        config.database.clone(),
                    ),
                    (
                        t!("backup.field.objects").to_string(),
                        saved_config_counts(config),
                    ),
                ];
                info_panel(
                    "icons/save.svg",
                    theme.icon_queries,
                    &config.name,
                    &t!("backup.config"),
                    &fields,
                )
            }
        }
    }

    fn render_backup_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let in_scope = self.backup_scope(cx).is_some();
        let has_file = in_scope && self.backup_single_selection().is_some_and(|s| s.is_file());
        let has_selection = in_scope && !self.backups_selection.is_empty();
        let can_create = in_scope;
        let weak = cx.weak_entity();
        let on_select = Rc::new(
            move |mode: ViewMode, _event: &ClickEvent, _window: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |app, cx| app.set_view_mode(VIEW_PAGE_BACKUPS, mode, cx));
            },
        );
        let view_mode = self.view_mode(VIEW_PAGE_BACKUPS);
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
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
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(ui::view_mode_toggle(theme, view_mode, on_select))
                    .child(
                        div()
                            .w(px(220.0))
                            .h(px(24.0))
                            .child(self.backup_search_input.clone()),
                    ),
            )
    }

    /// The single selected backup entry, or `None` when zero or several are selected.
    fn backup_single_selection(&self) -> Option<BackupEntry> {
        self.backups_selection
            .single()
            .and_then(backup_entry_from_key)
    }

    fn render_backup_list(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        // Backups only belong to an opened (and selected) database, so without one the list is
        // empty and every toolbar action is disabled.
        if self.backup_scope(cx).is_none() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .into_any_element();
        }

        let body: AnyElement = match self.view_mode(VIEW_PAGE_BACKUPS) {
            ViewMode::Detail => self.render_backup_detail(cx),
            ViewMode::Grid => self.render_backup_tiles(cx),
        };

        let mut list = div()
            .id("backup-list")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.backup_focus)
            .key_context(BACKUP_LIST_CONTEXT)
            .on_action(cx.listener(|this, _: &RenameBackupFile, window, cx| {
                if let Some(BackupEntry::File(index)) = this.backup_single_selection() {
                    this.begin_rename_backup(index, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopyBackupFile, _window, cx| {
                if let Some(BackupEntry::File(index)) = this.backup_single_selection() {
                    this.copy_backup_file(index);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &PasteBackupFile, _window, cx| {
                this.paste_backup(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if this.backup_rename.is_some() {
                        return;
                    }
                    window.focus(&this.backup_focus, cx);
                    this.begin_marquee(MarqueeTarget::Backups, event.position, event.modifiers, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.context_menu = Some(ContextMenu {
                        target: ContextTarget::BackupList,
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.backup_columns.borrow_mut().drag_resize(event) {
                    cx.notify();
                }
                this.drag_marquee(MarqueeTarget::Backups, event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.backup_columns.borrow_mut().end_resize() {
                        cx.notify();
                    }
                    this.end_marquee(cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .child(body),
            );
        if let Some(rect) = self.marquee_rect_for(MarqueeTarget::Backups) {
            list = list.child(rect);
        }
        list.into_any_element()
    }

    /// The content-fitted widths of the Backup 详细列表 columns (名称 / 修改日期 / 文件大小 / 备注).
    fn backup_detail_widths(&self, cx: &App) -> Vec<f32> {
        let mut longest = [
            ui::approx_text_width(&t!("common.name")) + 30.0,
            ui::approx_text_width(&t!("backup.field.modified")),
            ui::approx_text_width(&t!("backup.field.size")),
            ui::approx_text_width(&t!("backup.field.comment")),
        ];
        for index in self.visible_backup_files(cx) {
            let file = &self.backup_files[index];
            longest[0] = longest[0].max(ui::approx_text_width(&file.name) + 30.0);
            longest[1] = longest[1].max(ui::approx_text_width(
                &file
                    .modified
                    .map(format_file_time)
                    .unwrap_or_else(|| "--".to_string()),
            ));
            longest[2] = longest[2].max(ui::approx_text_width(&format_size_only(file.size)));
            longest[3] =
                longest[3].max(ui::approx_text_width(&single_line(&file.manifest.comment)));
        }
        for index in self.visible_backup_configs(cx) {
            let config = &self.backup_configs[index];
            longest[0] = longest[0].max(ui::approx_text_width(&config.name) + 30.0);
            longest[3] = longest[3].max(ui::approx_text_width(&saved_config_counts(config)));
        }
        vec![
            ui::detail_column_width(longest[0], 200.0),
            ui::detail_column_width(longest[1], 140.0),
            ui::detail_column_width(longest[2], 80.0),
            ui::detail_column_width(longest[3], 160.0),
        ]
    }

    /// The Backup 详细列表: 名称 / 修改日期 / 文件大小 / 备注.
    fn render_backup_detail(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let fitted = self.backup_detail_widths(cx);
        let widths = self.backup_columns.borrow_mut().resolve(&fitted);
        let mut header = ui::detail_header_row(theme);
        for (index, label) in [
            t!("common.name").to_string(),
            t!("backup.field.modified").to_string(),
            t!("backup.field.size").to_string(),
            t!("backup.field.comment").to_string(),
        ]
        .into_iter()
        .enumerate()
        {
            let width = widths[index];
            header = header.child(ui::detail_header_column(
                SharedString::from(format!("backup-resize-{index}")),
                width,
                self.backup_columns.borrow().resizing(index),
                theme,
                ui::detail_header_cell_plain(label, theme),
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.backup_columns
                        .borrow_mut()
                        .begin_resize(index, event.position.x, width);
                    cx.notify();
                }),
            ));
        }

        let mut list = ui::DetailList::new(
            "backup-detail",
            &self.backup_detail_scroll,
            ui::detail_content_width(&widths),
            header,
        );
        for index in self.visible_backup_files(cx) {
            let file = &self.backup_files[index];
            let key = backup_file_key(&file.name);
            let selected = self.backups_selection.contains(&key);
            let rename = self
                .backup_rename
                .as_ref()
                .filter(|edit| edit.index == index)
                .map(|edit| edit.input.clone());
            let name: AnyElement = match rename {
                Some(input) => div()
                    .w(px(widths[0]))
                    .flex_none()
                    .h(px(22.0))
                    .child(ui::rename_field(theme, input))
                    .into_any_element(),
                None => div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w(px(widths[0]))
                    .flex_none()
                    .overflow_hidden()
                    .text_color(rgb(theme.text))
                    .child(ui::leading_icon_badge(
                        "icons/backups.svg",
                        theme.icon_backups,
                        22.0,
                    ))
                    .child(ui::detail_cell_text(
                        SharedString::from(format!("backup-file-cell-{index}-0")),
                        file.name.clone(),
                    ))
                    .into_any_element(),
            };
            let modified = file
                .modified
                .map(format_file_time)
                .unwrap_or_else(|| "--".to_string());
            let muted = theme.text_muted;
            let click_key = key.clone();
            let menu_key = key.clone();
            let row = ui::detail_row(
                SharedString::from(format!("backup-file-{index}")),
                selected,
                theme,
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                this.hit_backup(&click_key, event.click_count(), event.modifiers(), cx);
                if event.click_count() >= 2
                    && let Some(BackupEntry::File(index)) = this.backup_single_selection()
                {
                    this.open_restore_backup(index, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    if !this.backups_selection.contains(&menu_key) {
                        this.backups_selection.select_one(menu_key.clone());
                        this.backup_selected = Some(BackupSelection::File(index));
                    }
                    this.context_menu = Some(ContextMenu {
                        target: ContextTarget::BackupFile { index },
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            .child(name)
            .child(
                div()
                    .w(px(widths[1]))
                    .flex_none()
                    .text_color(rgb(muted))
                    .child(ui::detail_cell_text(
                        SharedString::from(format!("backup-file-cell-{index}-1")),
                        modified,
                    )),
            )
            .child(
                div()
                    .w(px(widths[2]))
                    .flex_none()
                    .text_color(rgb(muted))
                    .child(ui::detail_cell_text(
                        SharedString::from(format!("backup-file-cell-{index}-2")),
                        format_size_only(file.size),
                    )),
            )
            .child(
                div()
                    .w(px(widths[3]))
                    .flex_none()
                    .text_color(rgb(muted))
                    .child(ui::detail_cell_text(
                        SharedString::from(format!("backup-file-cell-{index}-3")),
                        single_line(&file.manifest.comment),
                    )),
            );
            list = list.child(backup_row_with_rect(
                cx.weak_entity(),
                row,
                MarqueeTarget::Backups,
                key.clone(),
            ));
        }
        for index in self.visible_backup_configs(cx) {
            let config = &self.backup_configs[index];
            let key = backup_config_key(&config.name);
            let selected = self.backups_selection.contains(&key);
            let click_key = key.clone();
            let menu_key = key.clone();
            let row = ui::detail_row(
                SharedString::from(format!("backup-config-{index}")),
                selected,
                theme,
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                this.hit_backup(&click_key, event.click_count(), event.modifiers(), cx);
                if event.click_count() >= 2 {
                    this.open_backup_config(index, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    if !this.backups_selection.contains(&menu_key) {
                        this.backups_selection.select_one(menu_key.clone());
                        this.backup_selected = Some(BackupSelection::Config(index));
                    }
                    this.context_menu = Some(ContextMenu {
                        target: ContextTarget::BackupConfig { index },
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w(px(widths[0]))
                    .flex_none()
                    .overflow_hidden()
                    .text_color(rgb(theme.text))
                    .child(ui::leading_icon_badge(
                        "icons/save.svg",
                        theme.icon_queries,
                        22.0,
                    ))
                    .child(ui::detail_cell_text(
                        SharedString::from(format!("backup-config-cell-{index}-0")),
                        config.name.clone(),
                    )),
            )
            .child(
                div()
                    .w(px(widths[1]))
                    .flex_none()
                    .text_color(rgb(theme.text_muted))
                    .child(""),
            )
            .child(
                div()
                    .w(px(widths[2]))
                    .flex_none()
                    .text_color(rgb(theme.text_muted))
                    .child(""),
            )
            .child(
                div()
                    .w(px(widths[3]))
                    .flex_none()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(theme.text_muted))
                    .child(saved_config_counts(config)),
            );
            list = list.child(backup_row_with_rect(
                cx.weak_entity(),
                row,
                MarqueeTarget::Backups,
                key.clone(),
            ));
        }
        list.render(theme)
    }

    /// The Backup 平铺网格: the backup files and saved profiles in a column-major grid.
    fn render_backup_tiles(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut entries: Vec<(String, String, &'static str, u32, BackupEntry)> = Vec::new();
        for index in self.visible_backup_files(cx) {
            let file = &self.backup_files[index];
            entries.push((
                backup_file_key(&file.name),
                file.name.clone(),
                "icons/backups.svg",
                theme.icon_backups,
                BackupEntry::File(index),
            ));
        }
        for index in self.visible_backup_configs(cx) {
            let config = &self.backup_configs[index];
            entries.push((
                backup_config_key(&config.name),
                config.name.clone(),
                "icons/save.svg",
                theme.icon_queries,
                BackupEntry::Config(index),
            ));
        }
        if entries.is_empty() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .into_any_element();
        }
        let width = ui::grid_item_width(
            entries
                .iter()
                .map(|(_, name, _, _, _)| ui::approx_text_width(name))
                .fold(0.0, f32::max),
        );
        let rows = self.backup_grid.rows_per_column();
        let mut columns = ui::grid_columns();
        let mut column = ui::grid_column();
        let mut count = 0usize;
        for (key, name, icon, color, entry) in entries {
            if count == rows {
                columns = columns.child(column);
                column = ui::grid_column();
                count = 0;
            }
            let selected = self.backups_selection.contains(&key);
            let click_key = key.clone();
            let menu_key = key.clone();
            let tile = ui::grid_item_sized(
                SharedString::from(format!("backup-tile-{key}")),
                selected,
                theme,
                width,
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                this.hit_backup(&click_key, event.click_count(), event.modifiers(), cx);
                if event.click_count() >= 2 {
                    match entry {
                        BackupEntry::File(index) => this.open_restore_backup(index, cx),
                        BackupEntry::Config(index) => this.open_backup_config(index, cx),
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    if !this.backups_selection.contains(&menu_key) {
                        this.backups_selection.select_one(menu_key.clone());
                        this.sync_backup_selected();
                    }
                    this.context_menu = Some(ContextMenu {
                        target: match entry {
                            BackupEntry::File(index) => ContextTarget::BackupFile { index },
                            BackupEntry::Config(index) => ContextTarget::BackupConfig { index },
                        },
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            .child(ui::leading_icon_badge(icon, color, 16.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(theme.text))
                    .child(name),
            );
            column = column.child(backup_row_with_rect(
                cx.weak_entity(),
                tile,
                MarqueeTarget::Backups,
                key.clone(),
            ));
            count += 1;
        }
        if count > 0 {
            columns = columns.child(column);
        }

        let mut grid = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden();
        grid = grid.child(
            self.backup_grid
                .scroller("backup-grid-scroll")
                .child(columns),
        );
        if self.backup_grid.overflows() {
            grid = grid.child(
                self.backup_grid
                    .scrollbar("backup-grid-hscrollbar", theme)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            if this.backup_grid.begin(event.position.x) {
                                cx.notify();
                            }
                        }),
                    ),
            );
        }
        grid.into_any_element()
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

    /// Copy a backup file into the app's backup clipboard, so Ctrl+V can drop it into any
    /// database's backup folder.
    pub(super) fn copy_backup_file(&mut self, index: usize) {
        let Some(file) = self.backup_files.get(index) else {
            return;
        };
        self.backup_clipboard = Some(BackupClipboard {
            name: file.name.clone(),
            path: file.path.clone(),
        });
    }

    /// Paste the copied backup file into the current scope's database folder.
    pub(super) fn paste_backup(&mut self, cx: &mut Context<'_, Self>) {
        let Some(clipboard) = self.backup_clipboard.clone() else {
            return;
        };
        let Some((connection_index, database_index)) = self.backup_scope(cx) else {
            return;
        };
        let Some(connection_id) = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.id.clone())
        else {
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        let dir = self.backup_dir(&connection_id, &database);
        let _ = std::fs::create_dir_all(&dir);
        let suffix = t!("backup.copy_suffix").to_string();
        let dest = unique_backup_path(&dir, &clipboard.name, &suffix);
        match std::fs::copy(&clipboard.path, &dest) {
            Ok(_) => {
                self.refresh_backups(cx);
                self.backup_selected = self
                    .backup_files
                    .iter()
                    .position(|file| file.path == dest)
                    .map(BackupSelection::File);
            }
            Err(error) => self.error_dialog = Some(error.to_string()),
        }
        cx.notify();
    }

    /// Start the in-place "rename backup" editor on the selected file's row.
    pub(super) fn begin_rename_backup(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.backup_rename.is_some() {
            return;
        }
        let Some(file) = self.backup_files.get(index) else {
            return;
        };
        let old_name = file.name.clone();
        self.backup_selected = Some(BackupSelection::File(index));
        let theme = self.theme;
        let weak = cx.weak_entity();
        let change = weak.clone();
        let submit = weak.clone();
        let cancel = weak.clone();
        let initial = old_name.clone();
        let input = cx.new(move |cx| {
            TextInput::new(
                theme,
                initial,
                TextInputOptions {
                    bare: true,
                    text_size: Some(12.0),
                    // 20px, matching the list row so the framed editor never outgrows it.
                    size: Some(gpui_kit::component::Size::XSmall),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| {
                    if let Some(edit) = app.backup_rename.as_mut() {
                        edit.new_name = text.to_string();
                    }
                    cx.notify();
                });
            }))
            .on_submit(Rc::new(move |window, cx| {
                let _ = submit.update(cx, |app, cx| {
                    let focus = app.backup_focus.clone();
                    app.submit_backup_rename(cx);
                    window.focus(&focus, cx);
                });
            }))
            .on_cancel(Rc::new(move |window, cx| {
                let _ = cancel.update(cx, |app, cx| {
                    let focus = app.backup_focus.clone();
                    app.backup_rename = None;
                    app.backup_rename_blur = None;
                    window.focus(&focus, cx);
                    cx.notify();
                });
            }))
        });
        let focus = input.read(cx).focus_handle();
        self.backup_rename = Some(BackupRenameEdit {
            index,
            old_name: old_name.clone(),
            new_name: old_name,
            input,
        });
        self.backup_rename_blur = Some(cx.on_blur(&focus, window, |app, _window, cx| {
            if app.backup_rename.is_some() {
                app.submit_backup_rename(cx);
            }
        }));
        self.backup_rename_focus_pending = true;
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Commit the in-place rename: reject an empty/unchanged name or a name that already exists.
    pub(super) fn submit_backup_rename(&mut self, cx: &mut Context<'_, Self>) {
        let Some(edit) = self.backup_rename.take() else {
            return;
        };
        self.backup_rename_blur = None;
        let new_name = edit.new_name.trim().to_string();
        if new_name.is_empty() || new_name == edit.old_name {
            cx.notify();
            return;
        }
        let Some(file) = self.backup_files.get(edit.index) else {
            cx.notify();
            return;
        };
        let old_path = file.path.clone();
        let Some(dir) = old_path.parent() else {
            cx.notify();
            return;
        };
        let new_path = dir.join(format!("{new_name}.{}", rustgrid_backup::FILE_EXTENSION));
        if new_path.exists() {
            self.error_dialog = Some(t!("backup.rename_exists", name = new_name).to_string());
            cx.notify();
            return;
        }
        match std::fs::rename(&old_path, &new_path) {
            Ok(()) => {
                self.refresh_backups(cx);
                self.backup_selected = self
                    .backup_files
                    .iter()
                    .position(|file| file.path == new_path)
                    .map(BackupSelection::File);
            }
            Err(error) => self.error_dialog = Some(error.to_string()),
        }
        cx.notify();
    }

    /// Reveal a backup file in the OS file manager.
    pub(super) fn reveal_backup(&mut self, index: usize) {
        if let Some(file) = self.backup_files.get(index) {
            reveal_in_file_manager(&file.path);
        }
    }

    /// Reveal the current scope's backup folder in the OS file manager.
    pub(super) fn reveal_backup_folder(&mut self, cx: &mut Context<'_, Self>) {
        let Some((connection_index, database_index)) = self.backup_scope(cx) else {
            return;
        };
        let Some(connection_id) = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.id.clone())
        else {
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        let dir = self.backup_dir(&connection_id, &database);
        let _ = std::fs::create_dir_all(&dir);
        reveal_in_file_manager(&dir);
        cx.notify();
    }

    /// Open the object-info pane for a backup file.
    pub(super) fn open_backup_info(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.backup_selected = Some(BackupSelection::File(index));
        self.set_info_open(true, cx);
        cx.notify();
    }

    /// Refresh the backup list from disk (also called from the row context menu).
    pub(super) fn refresh_selected_backup(&mut self, cx: &mut Context<'_, Self>) {
        self.refresh_backups(cx);
        cx.notify();
    }

    pub(super) fn confirm_delete_backup(&mut self, cx: &mut Context<'_, Self>) {
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
    pub(super) fn extract_backup_sql(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(file) = self.backup_files.get(index) else {
            return;
        };
        let path = file.path.clone();
        let connection_index = self
            .connections
            .iter()
            .position(|node| node.profile.id == file.connection_id);
        let database = if file.database.is_empty() {
            file.manifest.schema.clone()
        } else {
            file.database.clone()
        };
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
        self.query_focus_pending = true;
        self.main_tab = MainTab::Queries;
        cx.notify();
    }

    // ----- New Backup dialog ------------------------------------------------------------------

    pub(super) fn open_new_backup(&mut self, cx: &mut Context<'_, Self>) {
        // A new backup belongs to the database the Backup tab is scoped to; there is nothing to
        // choose, so warn when no database is open.
        let Some((connection_index, database_index)) = self.backup_scope(cx) else {
            self.error_dialog = Some(t!("backup.no_scope").to_string());
            cx.notify();
            return;
        };
        let database = self
            .database_name(connection_index, database_index)
            .unwrap_or_default();
        self.open_new_backup_dialog_with(None, Some(connection_index), database, cx);
    }

    /// Open the New Backup dialog for a saved configuration, pre-filling its selection.
    fn open_backup_config(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(config) = self.backup_configs.get(index).cloned() else {
            return;
        };
        let connection_index = self
            .connections
            .iter()
            .position(|node| node.profile.id == config.connection_id)
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
            log_scroll: ScrollHandle::new(),
            total: 0,
            success: 0,
            failed: 0,
            rows_total: 0,
            rows_done: 0,
            started: None,
            elapsed: None,
            error: None,
        });
        self.backup_name_focus_pending = true;
        self.load_backup_objects(cx);
        self.open_backup_window(t!("backup.new_title").to_string(), cx);
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
    pub(super) fn save_backup_config(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
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
            // Put the caret in the name field so the user can fix it right away.
            if let Some(input) = self.backup_name_input.clone() {
                input.update(cx, |input, cx| input.focus_state(window, cx));
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
            dialog.total = selected.len();
            dialog.success = 0;
            dialog.failed = 0;
            dialog.rows_total = 0;
            dialog.rows_done = 0;
            dialog.elapsed = None;
            dialog.started = Some(std::time::Instant::now());
            dialog
                .log
                .push(t!("backup.log.started", time = now_timestamp()).to_string());
            dialog.log.push(
                t!(
                    "backup.log.start",
                    count = selected.len(),
                    database = database.clone()
                )
                .to_string(),
            );
            dialog.log_scroll.scroll_to_bottom();
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
            let mut fatal: Option<String> = None;
            for (kind, name) in selected {
                // Metadata first (DDL, columns, triggers), then stream the table's rows. A single
                // object's failure is logged and skipped so the rest of the selection still runs.
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
                            let message = t!(
                                "backup.log.object_failed",
                                name = name.clone(),
                                error = error.to_string()
                            )
                            .to_string();
                            let _ = this.update(cx, |app, cx| {
                                app.record_backup_result(false, message, 0, cx)
                            });
                            continue;
                        }
                        Err(error) => {
                            let message = t!(
                                "backup.log.object_failed",
                                name = name.clone(),
                                error = error.to_string()
                            )
                            .to_string();
                            let _ = this.update(cx, |app, cx| {
                                app.record_backup_result(false, message, 0, cx)
                            });
                            continue;
                        }
                    }
                };
                if let Err(error) = writer.lock().unwrap().begin_object(&meta) {
                    fatal = Some(error.to_string());
                    break;
                }

                let mut rows = 0u64;
                let mut row_error: Option<String> = None;
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
                        Ok(Err(error)) => row_error = Some(error.to_string()),
                        Err(error) => row_error = Some(error.to_string()),
                    }
                }

                if let Err(error) = writer.lock().unwrap().end_object() {
                    fatal = Some(error.to_string());
                    break;
                }
                let _ = this.update(cx, |app, cx| match &row_error {
                    Some(error) => {
                        let message = t!(
                            "backup.log.object_failed",
                            name = name.clone(),
                            error = error.clone()
                        )
                        .to_string();
                        app.record_backup_result(false, message, 0, cx);
                    }
                    None => {
                        let message =
                            t!("backup.log.object", name = name.clone(), rows = rows).to_string();
                        app.record_backup_result(true, message, rows as usize, cx);
                    }
                });
            }

            if fatal.is_none()
                && let Err(error) = writer.lock().unwrap().finish()
            {
                fatal = Some(error.to_string());
            }

            let _ = this.update(cx, |app, cx| app.finish_backup(fatal, cx));
        })
        .detach();
    }

    /// Record one object's outcome for the running backup and keep its log scrolled.
    fn record_backup_result(
        &mut self,
        ok: bool,
        message: String,
        rows: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            if ok {
                dialog.success += 1;
                dialog.rows_done += rows;
            } else {
                dialog.failed += 1;
            }
            dialog.log.push(message);
            dialog.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn finish_backup(&mut self, fatal: Option<String>, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.running = false;
            if let Some(error) = &fatal {
                dialog.error = Some(error.clone());
                dialog.log.push(error.clone());
            }
            if let Some(started) = dialog.started.take() {
                let elapsed = started.elapsed();
                dialog.elapsed = Some(elapsed);
                dialog
                    .log
                    .push(t!("backup.log.elapsed", time = format_hms(elapsed)).to_string());
            }
            dialog.log.push(
                t!(
                    "backup.log.summary",
                    total = dialog.total,
                    success = dialog.success,
                    failed = dialog.failed
                )
                .to_string(),
            );
            // A backup cannot know the record total up front; report what was written.
            dialog.rows_total = dialog.rows_total.max(dialog.rows_done);
            dialog.log_scroll.scroll_to_bottom();
        }
        let success = self
            .new_backup_dialog
            .as_ref()
            .map(|dialog| dialog.success)
            .unwrap_or(0);
        if fatal.is_none() && success > 0 {
            self.refresh_backups(cx);
            if !self.backup_files.is_empty() {
                self.backup_selected = Some(BackupSelection::File(0));
            }
        }
        cx.notify();
    }

    /// Close the Backup window. Called from its footer button, which has the window at hand, so
    /// the window is removed directly (updating it through its handle would re-borrow it).
    pub(super) fn close_new_backup(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.new_backup_dialog = None;
        self.backup_name_input = None;
        self.backup_window = None;
        window.remove_window();
        cx.notify();
    }

    // ----- Restore dialog ---------------------------------------------------------------------

    pub(super) fn open_restore_backup(&mut self, file_index: usize, cx: &mut Context<'_, Self>) {
        let Some(file) = self.backup_files.get(file_index) else {
            return;
        };
        // A backup file is bound to the database it currently lives under, so the restore target is
        // fixed: the connection the file belongs to and that database. Neither is editable. (A
        // pasted copy lives under a different database folder than the schema recorded inside the
        // archive, and the folder is what the file now belongs to.)
        let connection_index = self
            .connections
            .iter()
            .position(|node| node.profile.id == file.connection_id);
        let database = if file.database.is_empty() {
            file.manifest.schema.clone()
        } else {
            file.database.clone()
        };
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
            log_scroll: ScrollHandle::new(),
            total: 0,
            success: 0,
            failed: 0,
            rows_total: 0,
            rows_done: 0,
            started: None,
            elapsed: None,
            error: None,
        });
        let name = self
            .backup_files
            .get(file_index)
            .map(|file| file.name.clone())
            .unwrap_or_default();
        self.open_backup_window(format!("{} - {}", name, t!("backup.restore_title")), cx);
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
        // Planned record count comes from the archive manifest for the selected objects.
        let rows_total: usize = file
            .manifest
            .objects
            .iter()
            .filter(|object| {
                selected
                    .iter()
                    .any(|(kind, name)| *kind == object.kind && *name == object.name)
            })
            .map(|object| object.row_count)
            .sum();

        if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog.running = true;
            dialog.error = None;
            dialog.tab = BackupDialogTab::Log;
            dialog.log.clear();
            dialog.total = selected.len();
            dialog.success = 0;
            dialog.failed = 0;
            dialog.rows_total = rows_total;
            dialog.rows_done = 0;
            dialog.elapsed = None;
            dialog.started = Some(std::time::Instant::now());
            dialog
                .log
                .push(t!("backup.log.started", time = now_timestamp()).to_string());
            dialog.log.push(
                t!(
                    "backup.log.restore_start",
                    count = selected.len(),
                    database = database.clone()
                )
                .to_string(),
            );
            dialog.log_scroll.scroll_to_bottom();
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
                        app.finish_restore(Some(error.to_string()), connection_index, cx)
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        app.finish_restore(Some(error.to_string()), connection_index, cx)
                    });
                    return;
                }
            };

            for (kind, name) in selected {
                let reader = Arc::clone(&reader);
                let read_name = name.clone();
                let object = match runtime
                    .spawn(async move { reader.read_object(kind, &read_name) })
                    .await
                {
                    Ok(Ok(Some(object))) => object,
                    // A manifest entry the archive does not contain counts as a failure.
                    Ok(Ok(None)) => {
                        let message = t!(
                            "backup.log.object_restore_failed",
                            name = name.clone(),
                            error = t!("backup.log.missing").to_string()
                        )
                        .to_string();
                        let _ = this.update(cx, |app, cx| {
                            app.record_restore_result(false, message, 0, cx)
                        });
                        continue;
                    }
                    Ok(Err(error)) => {
                        let message = t!(
                            "backup.log.object_restore_failed",
                            name = name.clone(),
                            error = error.to_string()
                        )
                        .to_string();
                        let _ = this.update(cx, |app, cx| {
                            app.record_restore_result(false, message, 0, cx)
                        });
                        continue;
                    }
                    Err(error) => {
                        let message = t!(
                            "backup.log.object_restore_failed",
                            name = name.clone(),
                            error = error.to_string()
                        )
                        .to_string();
                        let _ = this.update(cx, |app, cx| {
                            app.record_restore_result(false, message, 0, cx)
                        });
                        continue;
                    }
                };
                let dump = object.to_object_dump();
                let rows = dump.rows.len();
                let connection = connection.clone();
                let database = database.clone();
                let result = match runtime
                    .spawn(async move { connection.restore_object(&database, &dump).await })
                    .await
                {
                    Ok(inner) => inner,
                    Err(error) => Err(Error::other(error)),
                };
                let _ = this.update(cx, |app, cx| match result {
                    Ok(()) => {
                        let message = t!("backup.log.restored", name = name.clone()).to_string();
                        app.record_restore_result(true, message, rows, cx);
                    }
                    Err(error) => {
                        let message = t!(
                            "backup.log.object_restore_failed",
                            name = name.clone(),
                            error = error.to_string()
                        )
                        .to_string();
                        app.record_restore_result(false, message, 0, cx);
                    }
                });
            }
            let _ = this.update(cx, |app, cx| app.finish_restore(None, connection_index, cx));
        })
        .detach();
    }

    /// Record one object's outcome for the running restore and keep its log scrolled.
    fn record_restore_result(
        &mut self,
        ok: bool,
        message: String,
        rows: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.restore_dialog.as_mut() {
            if ok {
                dialog.success += 1;
                dialog.rows_done += rows;
            } else {
                dialog.failed += 1;
            }
            dialog.log.push(message);
            dialog.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn finish_restore(
        &mut self,
        fatal: Option<String>,
        connection_index: Option<usize>,
        cx: &mut Context<'_, Self>,
    ) {
        let database = self
            .restore_dialog
            .as_ref()
            .map(|dialog| dialog.database.clone());
        if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog.running = false;
            if let Some(error) = &fatal {
                dialog.error = Some(error.clone());
                dialog.log.push(error.clone());
            }
            if let Some(started) = dialog.started.take() {
                let elapsed = started.elapsed();
                dialog.elapsed = Some(elapsed);
                dialog
                    .log
                    .push(t!("backup.log.elapsed", time = format_hms(elapsed)).to_string());
            }
            dialog.log.push(
                t!(
                    "backup.log.summary",
                    total = dialog.total,
                    success = dialog.success,
                    failed = dialog.failed
                )
                .to_string(),
            );
            dialog.log_scroll.scroll_to_bottom();
        }
        // Refresh only the restored database's tables so the other open databases stay open.
        if let Some(connection_index) = connection_index {
            if let Some(database) = database
                && let Some(database_index) =
                    self.database_index_by_name(connection_index, &database)
            {
                self.reload_tables(connection_index, database_index, cx);
            } else {
                self.load_databases(connection_index, cx);
            }
        }
        cx.notify();
    }

    /// Close the Restore window (see [`AppView::close_new_backup`]).
    pub(super) fn close_restore(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.restore_dialog = None;
        self.backup_window = None;
        window.remove_window();
        cx.notify();
    }

    // ----- Dialog chrome ----------------------------------------------------------------------

    // ----- Backup / Restore OS window ----------------------------------------------------------

    /// The contents of the Backup/Restore OS window: a custom titlebar (the OS titlebar is hidden
    /// so it matches the app's theme), the tab body and the footer.
    pub(super) fn backup_window_contents(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let title: String = if self.new_backup_dialog.is_some() {
            t!("backup.new_title").to_string()
        } else if let Some(name) = self
            .restore_dialog
            .as_ref()
            .and_then(|dialog| self.backup_files.get(dialog.file_index))
            .map(|file| file.name.clone())
        {
            format!("{} - {}", name, t!("backup.restore_title"))
        } else {
            return div().into_any_element();
        };
        let (body, footer): (AnyElement, AnyElement) = if self.new_backup_dialog.is_some() {
            (
                self.new_backup_dialog_body(cx),
                self.new_backup_dialog_footer(cx).into_any_element(),
            )
        } else if self.restore_dialog.is_some() {
            (
                self.restore_dialog_body(cx),
                self.restore_dialog_footer(cx).into_any_element(),
            )
        } else {
            return div().into_any_element();
        };
        // The error is shown as a banner above the footer so it is visible on whichever tab is
        // open (a validation error on the Objects tab would otherwise be hidden by the Log tab).
        let error = self
            .new_backup_dialog
            .as_ref()
            .and_then(|dialog| dialog.error.clone())
            .or_else(|| {
                self.restore_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.error.clone())
            });
        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.dialog_face))
            .text_color(rgb(theme.text))
            .child(backup_window_titlebar(title, theme))
            .child(body);
        if let Some(error) = error {
            root = root.child(backup_error_banner(error, theme));
        }
        root.child(footer).into_any_element()
    }

    /// Open the OS window that hosts the Backup/Restore UI. It is a normal top-level window, so it
    /// can be moved outside the main window and appears in the taskbar.
    ///
    /// The open is deferred: `open_window` renders the new window immediately, and that render
    /// reads `AppView`, so it must not run while an `AppView` borrow is held.
    fn open_backup_window(&mut self, title: String, cx: &mut Context<'_, Self>) {
        if self.backup_window.is_some() {
            return;
        }
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(560.0), px(600.0)), cx);
            let view_weak = weak.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.clone().into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    #[cfg(target_os = "windows")]
                    crate::win_resize::install(window);
                    // `open_window` does not raise what it opens, so the window can otherwise
                    // appear behind the main one.
                    window.activate_window();
                    let view = cx.new(|cx| BackupWindow::new(view_weak.clone(), &app_entity, cx));
                    cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
                },
            );
            match opened {
                Ok(handle) => {
                    app.update(cx, |app, cx| {
                        app.backup_window = Some(handle);
                        cx.notify();
                    });
                }
                Err(error) => {
                    app.update(cx, |app, cx| {
                        app.error_dialog = Some(error.to_string());
                        cx.notify();
                    });
                }
            }
        });
    }

    /// The OS window was closed by the user: drop its dialog state as well. Closing the main
    /// window quits the app, which also disposes of the Backup/Restore window.
    pub(super) fn on_window_closed(&mut self, id: WindowId, cx: &mut Context<'_, Self>) {
        if self.main_window_id == Some(id) {
            self.backup_window = None;
            self.new_backup_dialog = None;
            self.backup_name_input = None;
            self.restore_dialog = None;
            self.export_window = None;
            self.export_wizard = None;
            self.import_window = None;
            self.import_wizard = None;
            self.create_user_window = None;
            self.create_user_dialog = None;
            self.object_privileges = None;
            self.object_privileges_window = None;
            self.options_window = None;
            self.connection_window = None;
            cx.quit();
            return;
        }
        if self
            .backup_window
            .as_ref()
            .is_some_and(|handle| handle.window_id() == id)
        {
            self.backup_window = None;
            self.new_backup_dialog = None;
            self.backup_name_input = None;
            self.restore_dialog = None;
            cx.notify();
            return;
        }
        if self
            .export_window
            .as_ref()
            .is_some_and(|handle| handle.window_id() == id)
        {
            self.export_window = None;
            self.export_wizard = None;
            cx.notify();
            return;
        }
        if self
            .import_window
            .as_ref()
            .is_some_and(|handle| handle.window_id() == id)
        {
            self.import_window = None;
            self.import_wizard = None;
            cx.notify();
            return;
        }
        if self
            .create_user_window
            .as_ref()
            .is_some_and(|handle| handle.window_id() == id)
        {
            // The user closed the account window from the OS chrome: drop its state too, so the
            // toolbar buttons are not left permanently short-circuited by a stale window handle.
            self.cancel_create_user(cx);
        }
        if self
            .object_privileges_window
            .as_ref()
            .is_some_and(|handle| handle.window_id() == id)
        {
            self.close_privilege_manager(cx);
        }
        if self
            .options_window
            .as_ref()
            .is_some_and(|handle| handle.window_id() == id)
        {
            // The user closed the Options window from the OS chrome.
            self.options_window = None;
            cx.notify();
        }
        if self
            .connection_window
            .as_ref()
            .is_some_and(|handle| handle.window_id() == id)
        {
            // The user closed the connection window from the OS chrome.
            self.connection_window = None;
            self.clear_form_state();
            cx.notify();
        }
    }

    fn new_backup_dialog_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let active = self
            .new_backup_dialog
            .as_ref()
            .map(|dialog| dialog.tab)
            .unwrap_or(BackupDialogTab::Objects);
        let connection_name = self
            .new_backup_dialog
            .as_ref()
            .and_then(|dialog| self.connections.get(dialog.connection_index))
            .map(|node| node.profile.name.clone())
            .unwrap_or_else(|| "--".to_string());
        let database_name = self
            .new_backup_dialog
            .as_ref()
            .map(|dialog| dialog.database.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "--".to_string());
        let header = backup_target_line(
            t!("backup.target").to_string(),
            connection_name,
            database_name,
            theme,
        );
        let tabs = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .w_full()
            .px_4()
            .border_b_1()
            .border_color(rgb(theme.border))
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
            BackupDialogTab::Log => {
                let scroll = self
                    .new_backup_dialog
                    .as_ref()
                    .map(|dialog| dialog.log_scroll.clone())
                    .unwrap_or_default();
                let stats = self
                    .new_backup_dialog
                    .as_ref()
                    .map(|dialog| {
                        log_stats(
                            dialog.total,
                            dialog.success,
                            dialog.failed,
                            dialog.rows_total,
                            dialog.rows_done,
                            dialog.running,
                            dialog.started,
                            dialog.elapsed,
                        )
                    })
                    .unwrap_or_else(|| log_stats(0, 0, 0, 0, 0, false, None, None));
                render_backup_log(
                    self.new_backup_dialog
                        .as_ref()
                        .map(|dialog| dialog.log.as_slice())
                        .unwrap_or(&[]),
                    &scroll,
                    &stats,
                    theme,
                )
            }
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(header)
            .child(tabs)
            .child(content)
            .into_any_element()
    }

    fn render_backup_picker_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.new_backup_dialog.as_ref() else {
            return div().into_any_element();
        };
        let total = dialog.objects.len();
        let selected = dialog.objects.iter().filter(|entry| entry.selected).count();
        let summary = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px_4()
            .pt_2()
            .pb_1()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(
                        t!("backup.selected_count", selected = selected, total = total).to_string(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(backup_link_button(
                        "backup-select-all",
                        t!("backup.select_all").to_string(),
                        theme,
                        cx.listener(|this, _event, _window, cx| this.backup_set_all(true, cx)),
                    ))
                    .child(backup_link_button(
                        "backup-select-none",
                        t!("backup.select_none").to_string(),
                        theme,
                        cx.listener(|this, _event, _window, cx| this.backup_set_all(false, cx)),
                    )),
            );

        let picker = render_object_picker(&dialog.objects, dialog.loading, theme, false, cx);

        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(summary)
            .child(picker);
        body.into_any_element()
    }

    fn new_backup_dialog_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let name_input = match self.backup_name_input.clone() {
            Some(input) => div()
                .w(px(220.0))
                .h(px(28.0))
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
            .gap_3()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.dialog_face))
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
                            .flex_none()
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
                    .child(self.backup_status_indicator(theme))
                    .child(self.dialog_button(
                        "backup-save-config",
                        t!("backup.save").to_string(),
                        false,
                        cx.listener(|this, _event, window, cx| this.save_backup_config(window, cx)),
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
                        cx.listener(|this, _event, window, cx| this.close_new_backup(window, cx)),
                    )),
            )
            .into_any_element()
    }

    /// The small status indicator shown left of the footer's action buttons.
    fn backup_status_indicator(&self, theme: Theme) -> impl IntoElement {
        let (label, color) = match self.new_backup_dialog.as_ref() {
            Some(dialog) if dialog.running => (t!("backup.status.running").to_string(), 0x22b14c),
            Some(dialog) if dialog.error.is_some() => {
                (t!("backup.status.failed").to_string(), theme.danger)
            }
            Some(dialog) if dialog.total > 0 => (t!("backup.status.done").to_string(), 0x22b14c),
            _ => (t!("backup.status.idle").to_string(), theme.neutral),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .flex_none()
            .pr_2()
            .child(
                div()
                    .w(px(7.0))
                    .h(px(7.0))
                    .flex_none()
                    .rounded(px(9999.0))
                    .bg(rgb(color)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(label),
            )
    }

    fn restore_dialog_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let active = self
            .restore_dialog
            .as_ref()
            .map(|dialog| dialog.tab)
            .unwrap_or(BackupDialogTab::Objects);
        let connection_name = self
            .restore_dialog
            .as_ref()
            .and_then(|dialog| dialog.connection_index)
            .and_then(|index| self.connections.get(index))
            .map(|node| node.profile.name.clone())
            .unwrap_or_else(|| "--".to_string());
        let database_name = self
            .restore_dialog
            .as_ref()
            .map(|dialog| dialog.database.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "--".to_string());
        let header = backup_target_line(
            t!("backup.restore_to").to_string(),
            connection_name,
            database_name,
            theme,
        );
        let tabs = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .w_full()
            .px_4()
            .border_b_1()
            .border_color(rgb(theme.border))
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
            BackupDialogTab::Log => {
                let scroll = self
                    .restore_dialog
                    .as_ref()
                    .map(|dialog| dialog.log_scroll.clone())
                    .unwrap_or_default();
                let stats = self
                    .restore_dialog
                    .as_ref()
                    .map(|dialog| {
                        log_stats(
                            dialog.total,
                            dialog.success,
                            dialog.failed,
                            dialog.rows_total,
                            dialog.rows_done,
                            dialog.running,
                            dialog.started,
                            dialog.elapsed,
                        )
                    })
                    .unwrap_or_else(|| log_stats(0, 0, 0, 0, 0, false, None, None));
                render_backup_log(
                    self.restore_dialog
                        .as_ref()
                        .map(|dialog| dialog.log.as_slice())
                        .unwrap_or(&[]),
                    &scroll,
                    &stats,
                    theme,
                )
            }
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(header)
            .child(tabs)
            .child(content)
            .into_any_element()
    }

    fn render_restore_picker_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.restore_dialog.as_ref() else {
            return div().into_any_element();
        };
        let total = dialog.objects.len();
        let selected = dialog.objects.iter().filter(|entry| entry.selected).count();
        let summary = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px_4()
            .pt_2()
            .pb_1()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(
                        t!("backup.selected_count", selected = selected, total = total).to_string(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(backup_link_button(
                        "restore-select-all",
                        t!("backup.select_all").to_string(),
                        theme,
                        cx.listener(|this, _event, _window, cx| this.restore_set_all(true, cx)),
                    ))
                    .child(backup_link_button(
                        "restore-select-none",
                        t!("backup.select_none").to_string(),
                        theme,
                        cx.listener(|this, _event, _window, cx| this.restore_set_all(false, cx)),
                    )),
            );

        let picker = render_object_picker(&dialog.objects, false, theme, true, cx);

        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(summary)
            .child(picker);
        body.into_any_element()
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
                cx.listener(|this, _event, window, cx| this.close_restore(window, cx)),
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
            .h(px(34.0))
            .px_1()
            .mb(px(-1.0))
            .flex_none()
            .text_size(px(12.5))
            .cursor_pointer()
            .border_b_2()
            .when(active, move |style| {
                style
                    .border_color(rgb(theme.primary))
                    .text_color(rgb(theme.text))
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(!active, move |style| {
                style
                    .border_color(rgba(0x00000000))
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.text_color(rgb(theme.text)))
            })
            .on_click(on_click)
            .child(label)
    }
}

/// The root view of the Backup/Restore OS window. It re-renders whenever `AppView` changes, so the
/// window stays in sync while the operation runs in the background.
pub(super) struct BackupWindow {
    app: WeakEntity<AppView>,
    _subscription: Subscription,
}

impl BackupWindow {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        app_entity: &Entity<AppView>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let subscription = cx.observe(app_entity, |_, _, cx| cx.notify());
        Self {
            app,
            _subscription: subscription,
        }
    }
}

impl Render for BackupWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        app.update(cx, |app, cx| app.backup_window_contents(cx))
    }
}

// ----- Free helpers ---------------------------------------------------------------------------

/// The child window's titlebar. The OS titlebar is suppressed (`appears_transparent`) so this
/// chrome matches the app's theme; dragging and the min/max/close controls mirror the main window.
fn backup_window_titlebar(title: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(px(30.0))
        .flex_none()
        .bg(rgb(theme.titlebar_bg))
        .border_b_1()
        .border_color(rgb(theme.border))
        .child(
            div()
                .id("backup-titlebar-drag")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .h_full()
                .px_3()
                .text_sm()
                .window_control_area(WindowControlArea::Drag)
                .child(
                    img(ImageSource::Resource(Resource::Embedded("logo.png".into())))
                        .w(px(16.0))
                        .h(px(16.0))
                        .flex_none(),
                )
                .child(title),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h_full()
                .child(backup_titlebar_button(
                    "backup-titlebar-min",
                    "—",
                    theme,
                    |window, _cx| window.minimize_window(),
                ))
                .child(backup_titlebar_button(
                    "backup-titlebar-max",
                    "□",
                    theme,
                    |window, _cx| toggle_maximize(window),
                ))
                .child(backup_titlebar_button(
                    "backup-titlebar-close",
                    "✕",
                    theme,
                    |window, _cx| window.remove_window(),
                )),
        )
}

fn backup_titlebar_button(
    id: &'static str,
    label: &'static str,
    theme: Theme,
    action: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(40.0))
        .h_full()
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
        .on_click(move |_event, window, cx| action(window, cx))
        .child(label.to_string())
}

/// Build the configuration-name field of the New Backup dialog.
fn make_backup_name_input(
    theme: Theme,
    initial: String,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    let placeholder: SharedString = t!("backup.config_name_placeholder").to_string().into();
    cx.new(move |cx| {
        TextInput::new(
            theme,
            initial,
            TextInputOptions {
                placeholder,
                ..Default::default()
            },
            cx,
        )
        .on_change(Rc::new(move |text, _window, cx| {
            let _ = change.update(cx, |app, cx| {
                if let Some(dialog) = app.new_backup_dialog.as_mut() {
                    dialog.config_name = text.to_string();
                }
                cx.notify();
            });
        }))
    })
}

/// The dialog's target line: a small muted label plus the fixed connection and database, shown
/// above the tabs.
fn backup_target_line(
    label: String,
    connection_name: String,
    database_name: String,
    theme: Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .px_4()
        .pt_3()
        .pb_2()
        .child(
            div()
                .flex_none()
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child(label),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(tree_icon("icons/connection.svg", theme.icon_connection))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(theme.text))
                        .child(connection_name),
                ),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(12.5))
                .text_color(rgb(theme.text_muted))
                .child("·"),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(tree_icon("icons/database.svg", theme.icon_database))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(theme.text))
                        .child(database_name),
                ),
        )
}

/// A red alert banner shown above the dialog footer while an error is set, so the reason is
/// visible on whichever tab is open (a validation error would otherwise hide on the Log tab).
fn backup_error_banner(error: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .mx_4()
        .mb_2()
        .px_3()
        .py_2()
        .flex_none()
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(theme.danger))
        .bg(rgba((theme.danger << 8) | 0x14))
        .child(
            svg()
                .path("icons/warning.svg")
                .w(px(14.0))
                .h(px(14.0))
                .flex_none()
                .text_color(rgb(theme.danger)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(12.0))
                .text_color(rgb(theme.danger))
                .child(error),
        )
}

/// A muted text-link style action, used for the picker's inline 全选 / 取消全选.
fn backup_link_button(
    id: &'static str,
    label: String,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(20.0))
        .px_1()
        .flex_none()
        .rounded_sm()
        .text_size(px(12.0))
        .text_color(rgb(theme.text_muted))
        .cursor_pointer()
        .hover(move |style| {
            style
                .text_color(rgb(theme.text))
                .bg(rgb(theme.tree_hover_bg))
        })
        .on_click(on_click)
        .child(label)
}

/// Apply a saved configuration's selection to a freshly loaded object list.
fn apply_saved_selection(dialog: &mut NewBackupDialog, saved: &SavedBackup) {
    for entry in dialog.objects.iter_mut() {
        entry.selected = saved.selection(entry.kind).contains(&entry.name);
    }
}

/// One backup-list entry, parsed from its selection key (`file:<name>` / `config:<name>`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BackupEntry {
    File(usize),
    Config(usize),
}

impl BackupEntry {
    pub(super) fn is_file(self) -> bool {
        matches!(self, BackupEntry::File(_))
    }
}

/// The selection key of a backup file row.
fn backup_file_key(name: &str) -> String {
    format!("file:{name}")
}

/// The selection key of a saved backup configuration row.
fn backup_config_key(name: &str) -> String {
    format!("config:{name}")
}

/// The kind of a backup selection key, if it is well formed.
fn backup_entry_from_key(key: &str) -> Option<BackupEntry> {
    let (kind, _) = key.split_once(':')?;
    match kind {
        "file" => Some(BackupEntry::File(0)),
        "config" => Some(BackupEntry::Config(0)),
        _ => None,
    }
}

/// Wrap a row element so it publishes its window-space rectangle for the marquee.
fn backup_row_with_rect(
    app: WeakEntity<AppView>,
    row: impl IntoElement + 'static,
    target: MarqueeTarget,
    key: String,
) -> AnyElement {
    super::list_ops::row_with_rect(app, row, target, key)
}

/// The shared object-selection tree of the New Backup and Restore dialogs.
fn render_object_picker(
    objects: &[BackupObjectEntry],
    loading: bool,
    theme: Theme,
    restore: bool,
    cx: &mut Context<'_, AppView>,
) -> AnyElement {
    let mut list = div().flex().flex_col().py_1();
    if loading {
        list = list.child(
            div()
                .p_3()
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child(t!("common.loading").to_string()),
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
                    .gap_2()
                    .h(px(28.0))
                    .px_2()
                    .mx_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
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
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(theme.text))
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
                        .gap_2()
                        .h(px(BACKUP_OBJECT_ROW_HEIGHT))
                        .pl(px(30.0))
                        .pr_2()
                        .mx_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(selected, move |style| style.bg(rgb(theme.tree_hover_bg)))
                        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
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
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(12.0))
                                .text_color(rgb(theme.text))
                                .child(name),
                        ),
                );
            }
        }
    }

    let inner = div()
        .id("backup-object-picker")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .child(list);
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .mx_4()
        .mb_3()
        .rounded(px(8.0))
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.dialog_bg))
        .overflow_hidden()
        .child(inner)
        .into_any_element()
}

/// The information-log tab body shared by both dialogs: a Navicat-style summary block, the
/// `[Msg]` log lines, and a green progress bar.
fn render_backup_log(
    log: &[String],
    scroll: &ScrollHandle,
    stats: &LogStats,
    theme: Theme,
) -> AnyElement {
    let summary = div()
        .flex()
        .flex_col()
        .gap_0p5()
        .px_2()
        .pt_2()
        .pb_2()
        .child(log_summary_row(
            t!("backup.log.field.objects").to_string(),
            format_count(stats.objects),
            theme,
        ))
        .child(log_summary_row(
            t!("backup.log.field.records").to_string(),
            format_count(stats.records),
            theme,
        ))
        .child(log_summary_row(
            t!("backup.log.field.processed_objects").to_string(),
            format_count(stats.processed_objects),
            theme,
        ))
        .child(log_summary_row(
            t!("backup.log.field.processed_records").to_string(),
            format_count(stats.processed_records),
            theme,
        ))
        .child(log_summary_row(
            t!("backup.log.field.time").to_string(),
            stats.elapsed.clone(),
            theme,
        ));

    let mut content = div().flex().flex_col().gap_0p5().p_2();
    if log.is_empty() {
        content = content.child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child("--"),
        );
    } else {
        for line in log {
            content = content.child(div().text_size(px(12.0)).child(format!("[Msg] {line}")));
        }
    }

    let progress = stats.progress.clamp(0.0, 1.0);
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .mx_4()
        .mb_3()
        .rounded(px(8.0))
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.dialog_bg))
        .overflow_hidden()
        .child(summary)
        .child(
            div()
                .id("backup-log")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .mx_3()
                .rounded(px(6.0))
                .border_1()
                .border_color(rgb(theme.border))
                .overflow_y_scroll()
                .track_scroll(scroll)
                .child(content),
        )
        .child(
            // Green progress bar (matches Navicat's restore/backup window).
            div()
                .h(px(6.0))
                .mx_3()
                .mt_3()
                .mb_3()
                .flex_none()
                .rounded(px(9999.0))
                .bg(rgb(theme.scroll_track))
                .child(
                    div()
                        .h_full()
                        .w(gpui::relative(progress))
                        .rounded(px(9999.0))
                        .bg(rgb(0x22b14c)),
                ),
        )
        .into_any_element()
}

/// One `label    value` row of the log summary block.
fn log_summary_row(label: String, value: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .child(
            div()
                .w(px(120.0))
                .flex_none()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(value),
        )
}

/// Format a count with thousands separators.
fn format_count(value: usize) -> String {
    format_thousands(value as u64)
}

/// The local wall-clock timestamp used by the backup/restore logs.
fn now_timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Format an elapsed duration as `HH:MM:SS`.
fn format_hms(elapsed: std::time::Duration) -> String {
    let total = elapsed.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    )
}

/// The summary shown above the info log, mirroring Navicat's backup/restore window.
struct LogStats {
    objects: usize,
    records: usize,
    processed_objects: usize,
    processed_records: usize,
    elapsed: String,
    /// `processed_objects / objects`, clamped to `[0, 1]`.
    progress: f32,
}

/// Build the log summary from a dialog's progress counters. While running, elapsed is measured
/// live from `started`; otherwise the recorded final `elapsed` is shown.
#[allow(clippy::too_many_arguments)]
fn log_stats(
    total: usize,
    success: usize,
    failed: usize,
    rows_total: usize,
    rows_done: usize,
    running: bool,
    started: Option<std::time::Instant>,
    elapsed: Option<std::time::Duration>,
) -> LogStats {
    let processed = success + failed;
    let elapsed = if running {
        started.map(|started| started.elapsed())
    } else {
        elapsed
    };
    LogStats {
        objects: total,
        records: rows_total.max(rows_done),
        processed_objects: processed,
        processed_records: rows_done,
        elapsed: elapsed.map(format_hms).unwrap_or_else(|| "--".to_string()),
        progress: if total == 0 {
            0.0
        } else {
            (processed as f32 / total as f32).clamp(0.0, 1.0)
        },
    }
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
    if bytes < 1024 {
        return format!("{bytes} B ({bytes})");
    }
    format!("{} ({})", format_size_only(bytes), format_thousands(bytes))
}

/// A compact human-readable size such as `6.01 MB`, without the exact byte count.
fn format_size_only(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
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

/// A path for a backup named `name` under `dir` that does not collide with an existing file.
/// Mirrors Explorer's "keep both" behaviour: `<name>.rgbak`, `<name> - 副本.rgbak`,
/// `<name> - 副本 (2).rgbak`, ... (`suffix` is the localized "Copy" word).
fn unique_backup_path(dir: &std::path::Path, name: &str, suffix: &str) -> PathBuf {
    let extension = rustgrid_backup::FILE_EXTENSION;
    let first = dir.join(format!("{name}.{extension}"));
    if !first.exists() {
        return first;
    }
    let copy = format!("{name} - {suffix}");
    let candidate = dir.join(format!("{copy}.{extension}"));
    if !candidate.exists() {
        return candidate;
    }
    for number in 2u64.. {
        let candidate = dir.join(format!("{copy} ({number}).{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("an unused backup name always exists")
}

/// Reveal `path` in the OS file manager, selecting it where the platform supports it.
pub(super) fn reveal_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(target).spawn();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        let _ = path;
    }
}
