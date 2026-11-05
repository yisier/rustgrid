//! The Backup main tab: RustGrid `.rgbak` backups.
//!
//! `AppView` owns the state (the backup list, the "New Backup" and "Restore Backup" dialogs);
//! this module renders it and drives the work through [`rustgrid_backup`] and the
//! [`rustgrid_core::Connection`] trait. Files are written under the app config dir's
//! `backups/<connection-id>/<database>/` tree.

use std::io::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

use super::info_pane::info_panel;
use super::*;
use gpui_kit::base::SelectableText;
use rustgrid_backup::BackupManifest;
use rustgrid_core::{BackupObjectKind, ObjectKind, SavedBackup};

/// Rows in the New Backup dialog's object tree are `26px` tall.
const BACKUP_OBJECT_ROW_HEIGHT: f32 = 26.0;

/// How often the Backup/Restore window repaints its live elapsed clock while a run is going.
const BACKUP_CLOCK_TICK_MS: u64 = 1000;

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
        // Backups list ascending by creation time (oldest first), so the newest file sits at the
        // bottom just above the saved-configuration rows. Fall back to the name (a timestamp) to
        // break ties and to order manifests with no recorded creation time.
        files.sort_by(|left, right| {
            left.manifest
                .created_unix
                .cmp(&right.manifest.created_unix)
                .then_with(|| left.name.cmp(&right.name))
        });
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
        // A database, schema or category row scopes its database; backups are per database.
        let (connection_index, database_index, _schema) = parse_tree_scope(&selected)?;
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
        self.sync_backup_selected(cx);
        cx.notify();
    }

    /// The visible backup entries' selection keys, in the order the list shows them: the
    /// 详细列表's active sort, or the 平铺 grid's files-then-configurations order. Shift-extend and
    /// Ctrl+A read that order, so it must match what is drawn.
    fn backup_visible_keys(&self, cx: &App) -> Vec<String> {
        let mut rows: Vec<BackupEntry> = self
            .visible_backup_files(cx)
            .into_iter()
            .map(BackupEntry::File)
            .chain(
                self.visible_backup_configs(cx)
                    .into_iter()
                    .map(BackupEntry::Config),
            )
            .collect();
        if self.view_mode(VIEW_PAGE_BACKUPS) == ViewMode::Detail {
            let sort = self.backup_sort;
            sort.sort_by(&mut rows, |entry| backup_sort_key(entry, self, sort.column));
        }
        rows.iter()
            .filter_map(|entry| match entry {
                BackupEntry::File(index) => self
                    .backup_files
                    .get(*index)
                    .map(|file| backup_file_key(&file.name)),
                BackupEntry::Config(index) => self
                    .backup_configs
                    .get(*index)
                    .map(|config| backup_config_key(&config.name)),
            })
            .collect()
    }

    /// Keep the single-selection mirror (`backup_selected`) in step with the multi-selection.
    /// It resolves through the visible entries (see [`Self::selected_backup_entries`]), so the
    /// mirror never names a file the list is not showing.
    fn sync_backup_selected(&mut self, cx: &App) {
        let (files, configs) = self.selected_backup_entries(cx);
        self.backup_selected = match (files.as_slice(), configs.as_slice()) {
            ([file], []) => Some(BackupSelection::File(*file)),
            ([], [config]) => Some(BackupSelection::Config(*config)),
            _ => None,
        };
    }

    /// Whether one of the backup window's runs is in progress. The log summary measures the
    /// elapsed time live from the run's start instant, so the window has to repaint for the clock
    /// to move; a run can spend minutes inside a single large table without emitting progress.
    pub(super) fn backup_run_in_progress(&self) -> bool {
        self.new_backup_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.running)
            || self
                .restore_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.running)
            || self
                .extract_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.running)
            || self
                .dump_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.running)
            || self
                .run_sql_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.running)
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
        let has_file = in_scope
            && self
                .backup_single_selection(cx)
                .is_some_and(|s| s.is_file());
        let (selected_files, selected_configs) = self.selected_backup_entries(cx);
        let has_selection = in_scope && !(selected_files.is_empty() && selected_configs.is_empty());
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
    ///
    /// The selection key only names the row (`file:<name>` / `config:<name>`), so its index has to
    /// be resolved back into `backup_files` / `backup_configs`. The resolution goes through the
    /// *visible* (scoped) entries, so a key left over from another database — or one whose name
    /// also exists in another scope (backup names are timestamps to the second) — can never point
    /// an action at a file this list does not show.
    fn backup_single_selection(&self, cx: &App) -> Option<BackupEntry> {
        let (files, configs) = self.selected_backup_entries(cx);
        match (files.as_slice(), configs.as_slice()) {
            ([file], []) => Some(BackupEntry::File(*file)),
            ([], [config]) => Some(BackupEntry::Config(*config)),
            _ => None,
        }
    }

    /// Ctrl/Cmd+F while the Backup list holds focus moves the caret into the toolbar's search
    /// box, so filtering the list never needs the mouse. The box sits above the list's key
    /// context, so taking focus also leaves that context — like clicking it does.
    pub(super) fn focus_backup_search(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.backup_search_input
            .update(cx, |input, cx| input.focus_state(window, cx));
        cx.notify();
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
            .on_action(cx.listener(|this, _: &BackupFocusSearch, window, cx| {
                this.focus_backup_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RenameBackupFile, window, cx| {
                if let Some(entry) = this.backup_single_selection(cx) {
                    let target = match entry {
                        BackupEntry::File(index) => BackupSelection::File(index),
                        BackupEntry::Config(index) => BackupSelection::Config(index),
                    };
                    this.begin_backup_rename(target, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopyBackupFile, _window, cx| {
                if let Some(BackupEntry::File(index)) = this.backup_single_selection(cx) {
                    this.copy_backup_file(index);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &PasteBackupFile, _window, cx| {
                this.paste_backup(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAllBackups, _window, cx| {
                this.select_all_backups(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    // Clicking the list background commits an in-place rename (see the query list).
                    if this.backup_rename.is_some() {
                        this.submit_backup_rename(cx);
                        window.focus(&this.backup_focus, cx);
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
            let cell = ui::detail_header_cell(
                SharedString::from(format!("backup-sort-{index}")),
                label,
                self.backup_sort.indicator(index),
                theme,
                cx.listener(move |this, _event, _window, cx| {
                    this.backup_sort.toggle(index);
                    cx.notify();
                }),
            );
            header = header.child(ui::detail_header_column(
                SharedString::from(format!("backup-resize-{index}")),
                width,
                self.backup_columns.borrow().resizing(index),
                theme,
                cell,
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
        // The files and the saved configurations share the list; the active sort orders both
        // together (a configuration has no date/size, so it sorts into the empty group).
        let mut rows: Vec<BackupEntry> = self
            .visible_backup_files(cx)
            .into_iter()
            .map(BackupEntry::File)
            .chain(
                self.visible_backup_configs(cx)
                    .into_iter()
                    .map(BackupEntry::Config),
            )
            .collect();
        let sort = self.backup_sort;
        sort.sort_by(&mut rows, |entry| backup_sort_key(entry, self, sort.column));
        for entry in rows {
            list = list.child(match entry {
                BackupEntry::File(index) => self.backup_file_detail_row(index, &widths, theme, cx),
                BackupEntry::Config(index) => {
                    self.backup_config_detail_row(index, &widths, theme, cx)
                }
            });
        }
        list.render(theme)
    }

    /// One backup file of the 详细列表.
    fn backup_file_detail_row(
        &self,
        index: usize,
        widths: &[f32],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let file = &self.backup_files[index];
        let key = backup_file_key(&file.name);
        let selected = self.backups_selection.contains(&key);
        let rename = self
            .backup_rename
            .as_ref()
            .filter(|edit| edit.target == BackupSelection::File(index))
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
                    ui::DETAIL_BADGE_SIZE,
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
            if this.click_opens(event.click_count(), event.modifiers())
                && let Some(BackupEntry::File(index)) = this.backup_single_selection(cx)
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
        backup_row_with_rect(cx.weak_entity(), row, MarqueeTarget::Backups, key)
    }

    /// One saved backup configuration of the 详细列表 (it has no date or size of its own).
    fn backup_config_detail_row(
        &self,
        index: usize,
        widths: &[f32],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let config = &self.backup_configs[index];
        let key = backup_config_key(&config.name);
        let selected = self.backups_selection.contains(&key);
        let click_key = key.clone();
        let menu_key = key.clone();
        let rename = self
            .backup_rename
            .as_ref()
            .filter(|edit| edit.target == BackupSelection::Config(index))
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
                    "icons/save.svg",
                    theme.icon_queries,
                    ui::DETAIL_BADGE_SIZE,
                ))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("backup-config-cell-{index}-0")),
                    config.name.clone(),
                ))
                .into_any_element(),
        };
        let row = ui::detail_row(
            SharedString::from(format!("backup-config-{index}")),
            selected,
            theme,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            this.hit_backup(&click_key, event.click_count(), event.modifiers(), cx);
            if this.click_opens(event.click_count(), event.modifiers()) {
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
        .child(name)
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
        backup_row_with_rect(cx.weak_entity(), row, MarqueeTarget::Backups, key)
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
            // The row being renamed draws the in-place editor instead of its label (the 详细列表
            // does this too; the 平铺网格 must as well, or F2/context-menu rename looks inert).
            let rename = match entry {
                BackupEntry::File(index) => self
                    .backup_rename
                    .as_ref()
                    .filter(|edit| edit.target == BackupSelection::File(index))
                    .map(|edit| edit.input.clone()),
                BackupEntry::Config(index) => self
                    .backup_rename
                    .as_ref()
                    .filter(|edit| edit.target == BackupSelection::Config(index))
                    .map(|edit| edit.input.clone()),
            };
            let title: AnyElement = match rename {
                Some(input) => div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(20.0))
                    .child(ui::rename_field(theme, input))
                    .into_any_element(),
                None => div()
                    .id(SharedString::from(format!("backup-tile-label-{key}")))
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .truncate()
                    .tooltip(ui::text_tooltip(name.clone()))
                    .text_color(rgb(theme.text))
                    .child(name)
                    .into_any_element(),
            };
            let tile = ui::grid_item_sized(
                SharedString::from(format!("backup-tile-{key}")),
                selected,
                theme,
                width,
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                this.hit_backup(&click_key, event.click_count(), event.modifiers(), cx);
                if this.click_opens(event.click_count(), event.modifiers()) {
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
                        this.sync_backup_selected(cx);
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
            .child(title);
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
        grid = grid.child(self.backup_grid.scroller("backup-grid-scroll", columns));
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
        if let Some(BackupEntry::File(index)) = self.backup_single_selection(cx) {
            self.open_restore_backup(index, cx);
        }
    }

    fn extract_selected_backup(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(BackupEntry::File(index)) = self.backup_single_selection(cx) {
            self.open_extract_sql(index, cx);
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
                self.select_backup_file(&dest, cx);
            }
            Err(error) => self.error_dialog = Some(error.to_string()),
        }
        cx.notify();
    }

    /// Select one backup file by its path — after a run wrote it, or a paste copied it — so both
    /// the multi-selection (the highlighted row and the toolbar's 删除备份) and the
    /// single-selection mirror (the info pane) name it. The file only becomes selectable once it
    /// is part of the visible scope's list, which is what the toolbar reads.
    fn select_backup_file(&mut self, path: &std::path::Path, cx: &App) {
        let Some(index) = self.backup_files.iter().position(|file| file.path == path) else {
            return;
        };
        let key = backup_file_key(&self.backup_files[index].name);
        self.backups_selection.select_one(key);
        self.sync_backup_selected(cx);
    }

    /// Start the in-place "rename" editor on the given entry's row.
    pub(super) fn begin_backup_rename(
        &mut self,
        target: BackupSelection,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.backup_rename.is_some() {
            return;
        }
        let old_name = match target {
            BackupSelection::File(index) => {
                self.backup_files.get(index).map(|file| file.name.clone())
            }
            BackupSelection::Config(index) => self
                .backup_configs
                .get(index)
                .map(|config| config.name.clone()),
        };
        let Some(old_name) = old_name else {
            return;
        };
        self.backup_selected = Some(target);
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
            target,
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
        match edit.target {
            BackupSelection::File(index) => self.submit_backup_file_rename(index, new_name, cx),
            BackupSelection::Config(index) => self.submit_backup_config_rename(index, new_name),
        }
        cx.notify();
    }

    /// Rename a backup file on disk.
    fn submit_backup_file_rename(
        &mut self,
        index: usize,
        new_name: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(file) = self.backup_files.get(index) else {
            return;
        };
        let old_path = file.path.clone();
        let Some(dir) = old_path.parent() else {
            return;
        };
        let new_path = dir.join(format!("{new_name}.{}", rustgrid_backup::FILE_EXTENSION));
        if new_path.exists() {
            self.error_dialog = Some(t!("backup.rename_exists", name = new_name).to_string());
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
    }

    /// Rename a saved backup configuration (the name is its key, kept unique per scope).
    fn submit_backup_config_rename(&mut self, index: usize, new_name: String) {
        let Some(config) = self.backup_configs.get(index) else {
            return;
        };
        let connection_id = config.connection_id.clone();
        let database = config.database.clone();
        let exists = self
            .backup_configs
            .iter()
            .enumerate()
            .any(|(other_index, other)| {
                other_index != index
                    && other.name == new_name
                    && other.connection_id == connection_id
                    && other.database == database
            });
        if exists {
            self.error_dialog = Some(t!("backup.rename_exists", name = new_name).to_string());
            return;
        }
        self.backup_configs[index].name = new_name;
        let _ = self.config.save_backups(&self.backup_configs);
        self.backup_selected = Some(BackupSelection::Config(index));
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
        self.refresh_toast(ToastKind::Success, t!("grid.refreshed").to_string(), cx);
        cx.notify();
    }

    /// Select every visible backup row (Ctrl+A), keeping the single-selection mirror in step.
    pub(super) fn select_all_backups(&mut self, cx: &mut Context<'_, Self>) {
        let visible = self.backup_visible_keys(cx);
        self.backups_selection.select_all(&visible);
        self.sync_backup_selected(cx);
        cx.notify();
    }

    /// The selected backup files and saved profiles that belong to the current scope. A scope
    /// change can leave stale keys in the selection, so the toolbar and delete always re-filter.
    ///
    /// Each key is resolved **inside the visible (scoped) list** rather than against the whole
    /// `backup_files` / `backup_configs` arrays. A key is only the file's name, and a backup is
    /// named after the second it was created, so two databases can hold a same-named file; looking
    /// the key up globally picked the first of them, which put the selection outside the scope and
    /// left 删除备份 greyed out for the row that was actually selected.
    fn selected_backup_entries(&self, cx: &App) -> (Vec<usize>, Vec<usize>) {
        let visible_files = self.visible_backup_files(cx);
        let visible_configs = self.visible_backup_configs(cx);
        let mut files = Vec::new();
        let mut configs = Vec::new();
        for key in self.backups_selection.items() {
            if let Some(index) = visible_files.iter().copied().find(|index| {
                self.backup_files
                    .get(*index)
                    .is_some_and(|file| backup_file_key(&file.name) == key)
            }) {
                files.push(index);
            } else if let Some(index) = visible_configs.iter().copied().find(|index| {
                self.backup_configs
                    .get(*index)
                    .is_some_and(|config| backup_config_key(&config.name) == key)
            }) {
                configs.push(index);
            }
        }
        (files, configs)
    }

    /// Ask for confirmation before deleting every selected backup entry (files and/or saved
    /// profiles). A lone selection keeps the specific "delete this file/config" wording.
    pub(super) fn confirm_delete_backup(&mut self, cx: &mut Context<'_, Self>) {
        let (mut files, mut configs) = self.selected_backup_entries(cx);
        // The context menu's 删除 sets the single-selection mirror instead of clicking the row, so
        // fall back to it — but only for an entry the list actually shows.
        if files.is_empty() && configs.is_empty() {
            let visible_files = self.visible_backup_files(cx);
            let visible_configs = self.visible_backup_configs(cx);
            match self.backup_selected {
                Some(BackupSelection::File(index)) if visible_files.contains(&index) => {
                    files.push(index)
                }
                Some(BackupSelection::Config(index)) if visible_configs.contains(&index) => {
                    configs.push(index)
                }
                _ => return,
            }
        }
        self.delete_confirm = if files.len() == 1 && configs.is_empty() {
            Some(DeleteConfirm::BackupFile { index: files[0] })
        } else if files.is_empty() && configs.len() == 1 {
            Some(DeleteConfirm::BackupConfig { index: configs[0] })
        } else {
            Some(DeleteConfirm::BackupEntries { files, configs })
        };
        cx.notify();
    }

    /// Delete several backup files and/or saved profiles at once.
    pub(super) fn delete_backups(
        &mut self,
        files: Vec<usize>,
        configs: Vec<usize>,
        cx: &mut Context<'_, Self>,
    ) {
        for index in &files {
            if let Some(file) = self.backup_files.get(*index) {
                let _ = std::fs::remove_file(&file.path);
            }
        }
        let mut configs = configs;
        configs.sort_unstable();
        configs.dedup();
        let mut removed_config = false;
        // Remove from the end so the earlier indices stay valid.
        for index in configs.into_iter().rev() {
            if index < self.backup_configs.len() {
                self.backup_configs.remove(index);
                removed_config = true;
            }
        }
        if removed_config && let Err(error) = self.config.save_backups(&self.backup_configs) {
            self.warn_persist("backup profiles", error, cx);
        }
        self.backups_selection.clear();
        self.backup_selected = None;
        self.refresh_backups(cx);
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
        if let Err(error) = self.config.save_backups(&self.backup_configs) {
            self.warn_persist("backup profiles", error, cx);
        }
        self.backup_selected = None;
        self.clamp_backup_selection();
        cx.notify();
    }

    /// Open the "Extract SQL" dialog for a backup file. Mirrors the Restore window: pick the
    /// objects, choose an output file, then watch the extraction log and progress.
    pub(super) fn open_extract_sql(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(file) = self.backup_files.get(index) else {
            return;
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
        self.new_backup_dialog = None;
        self.restore_dialog = None;
        self.extract_dialog = Some(ExtractSqlDialog {
            file_index: index,
            tab: BackupDialogTab::Objects,
            objects,
            output_path: None,
            running: false,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            .get(index)
            .map(|file| file.name.clone())
            .unwrap_or_default();
        self.open_backup_window(format!("{} - {}", name, t!("backup.extract_sql")), cx);
        cx.notify();
    }

    pub(super) fn extract_toggle_object(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.extract_dialog.as_mut()
            && let Some(entry) = dialog.objects.get_mut(index)
        {
            entry.selected = !entry.selected;
        }
        cx.notify();
    }

    pub(super) fn extract_toggle_kind(
        &mut self,
        kind: BackupObjectKind,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.extract_dialog.as_mut() {
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

    pub(super) fn extract_set_all(&mut self, selected: bool, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.extract_dialog.as_mut() {
            for entry in dialog.objects.iter_mut() {
                entry.selected = selected;
            }
        }
        cx.notify();
    }

    /// Run the extraction: choose the output `.sql` file through the system dialog, then stream
    /// each selected object's SQL into it while updating the log and progress live.
    pub(super) fn run_extract(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.extract_dialog.as_ref() else {
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
        if selected.is_empty() {
            if let Some(dialog) = self.extract_dialog.as_mut() {
                dialog.error = Some(t!("backup.no_objects").to_string());
            }
            cx.notify();
            return;
        }
        let Some(file) = self.backup_files.get(file_index) else {
            return;
        };
        let path = file.path.clone();
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
        let default_name = path
            .file_stem()
            .map(|stem| format!("{}.sql", stem.to_string_lossy()))
            .unwrap_or_else(|| "backup.sql".to_string());
        let default_dir = path.parent().map(|parent| parent.to_path_buf());
        let title = t!("backup.extract_sql").to_string();
        let runtime = self.runtime.clone();
        let cancel = self
            .extract_dialog
            .as_ref()
            .map(|dialog| dialog.cancel.clone())
            .unwrap_or_default();

        cx.spawn(async move |this, cx| {
            // Choose the output file first (system dialog on rfd's own thread).
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_title(title)
                .add_filter("SQL", &["sql"])
                .set_file_name(default_name);
            if let Some(parent) = &default_dir {
                dialog = dialog.set_directory(parent);
            }
            let Some(handle) = dialog.save_file().await else {
                return;
            };
            let sql_path = handle.path().to_path_buf();

            let count = selected.len();
            let _ = this.update(cx, |app, cx| {
                if let Some(dialog) = app.extract_dialog.as_mut() {
                    dialog.output_path = Some(sql_path.clone());
                    dialog.running = true;
                    dialog
                        .cancel
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    dialog.error = None;
                    dialog.tab = BackupDialogTab::Log;
                    dialog.log.clear();
                    dialog.total = count;
                    dialog.success = 0;
                    dialog.failed = 0;
                    dialog.rows_total = rows_total;
                    dialog.rows_done = 0;
                    dialog.elapsed = None;
                    dialog.started = Some(std::time::Instant::now());
                    dialog
                        .log
                        .push(t!("backup.log.started", time = now_timestamp()).to_string());
                    dialog
                        .log
                        .push(t!("backup.log.extract_start", count = count).to_string());
                    dialog.log_scroll.scroll_to_bottom();
                }
                cx.notify();
            });

            // Open the archive once; each object is read on demand so peak memory stays bounded.
            let reader_path = path.clone();
            let reader = match runtime
                .spawn(async move { rustgrid_backup::BackupReader::open(&reader_path) })
                .await
            {
                Ok(Ok(reader)) => Arc::new(reader),
                Ok(Err(error)) => {
                    let _ = this.update(cx, |app, cx| {
                        app.finish_extract(Some(error.to_string()), cx)
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        app.finish_extract(Some(error.to_string()), cx)
                    });
                    return;
                }
            };

            let mut output = match std::fs::File::create(&sql_path) {
                Ok(file) => file,
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        app.finish_extract(Some(error.to_string()), cx)
                    });
                    return;
                }
            };

            for (kind, name) in selected {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let reader = Arc::clone(&reader);
                let read_name = name.clone();
                let object = match runtime
                    .spawn(async move { reader.read_object(kind, &read_name) })
                    .await
                {
                    Ok(Ok(Some(object))) => object,
                    Ok(Ok(None)) => {
                        let message = t!(
                            "backup.log.object_extract_failed",
                            name = name.clone(),
                            error = t!("backup.log.missing").to_string()
                        )
                        .to_string();
                        let _ = this.update(cx, |app, cx| {
                            app.record_extract_result(false, message, 0, cx)
                        });
                        continue;
                    }
                    Ok(Err(error)) => {
                        let message = t!(
                            "backup.log.object_extract_failed",
                            name = name.clone(),
                            error = error.to_string()
                        )
                        .to_string();
                        let _ = this.update(cx, |app, cx| {
                            app.record_extract_result(false, message, 0, cx)
                        });
                        continue;
                    }
                    Err(error) => {
                        let message = t!(
                            "backup.log.object_extract_failed",
                            name = name.clone(),
                            error = error.to_string()
                        )
                        .to_string();
                        let _ = this.update(cx, |app, cx| {
                            app.record_extract_result(false, message, 0, cx)
                        });
                        continue;
                    }
                };
                let rows = object.data.lines().count();
                let sql = rustgrid_backup::object_sql(&object);
                if let Err(error) = output.write_all(sql.as_bytes()) {
                    let _ = this.update(cx, |app, cx| {
                        app.finish_extract(Some(error.to_string()), cx)
                    });
                    return;
                }
                let message = t!("backup.log.extracted", name = name.clone()).to_string();
                let _ = this.update(cx, |app, cx| {
                    app.record_extract_result(true, message, rows, cx)
                });
            }
            let _ = output.flush();
            let cancelled = cancel.load(std::sync::atomic::Ordering::Relaxed);
            let _ = this.update(cx, |app, cx| {
                if cancelled {
                    app.backup_log(t!("backup.log.cancelled").to_string(), cx);
                }
                app.finish_extract(None, cx)
            });
        })
        .detach();
    }

    /// Record one object's outcome for the running extract and keep its log scrolled.
    fn record_extract_result(
        &mut self,
        ok: bool,
        message: String,
        rows: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.extract_dialog.as_mut() {
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

    fn finish_extract(&mut self, fatal: Option<String>, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.extract_dialog.as_mut() {
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
        cx.notify();
    }

    /// Close the Extract window.
    pub(super) fn close_extract(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.extract_dialog.as_mut() {
            dialog
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.extract_dialog = None;
        self.backup_window = None;
        window.remove_window();
        cx.notify();
    }

    /// Open the extracted `.sql` file with the OS default application (the 打开 button).
    pub(super) fn open_extract_output(&mut self, cx: &mut Context<'_, Self>) {
        let Some(path) = self
            .extract_dialog
            .as_ref()
            .and_then(|dialog| dialog.output_path.clone())
        else {
            return;
        };
        open_with_default_app(&path);
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
    pub(super) fn open_backup_config(&mut self, index: usize, cx: &mut Context<'_, Self>) {
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
        // Editing an existing configuration shows the "edit" title; a fresh one shows "new".
        let apply_is_edit = apply.is_some();
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
            editing: apply_is_edit,
            loading: false,
            running: false,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pause: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            paused: false,
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
            config_saved: false,
            baseline: None,
        });
        self.backup_name_focus_pending = true;
        self.load_backup_objects(cx);
        let title = if apply_is_edit {
            t!("backup.edit_title").to_string()
        } else {
            t!("backup.new_title").to_string()
        };
        self.open_backup_window(title, cx);
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
                        // The freshly-loaded (and, for a saved configuration, applied) state is
                        // what 保存 compares against.
                        capture_backup_baseline(dialog);
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
            dialog.config_saved = false;
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
            dialog.config_saved = false;
        }
        cx.notify();
    }

    pub(super) fn backup_set_all(&mut self, selected: bool, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            for entry in dialog.objects.iter_mut() {
                entry.selected = selected;
            }
            dialog.config_saved = false;
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
        if let Err(error) = self.config.save_backups(&self.backup_configs) {
            self.warn_persist("backup profiles", error, cx);
        }
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.error = None;
            // Confirm the save in the footer: the log line alone is invisible on the Objects tab.
            dialog.config_saved = true;
            // The saved state is the new baseline, so 保存 disables until the next change.
            capture_backup_baseline(dialog);
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
            dialog.config_saved = false;
            dialog
                .cancel
                .store(false, std::sync::atomic::Ordering::Relaxed);
            dialog
                .pause
                .store(false, std::sync::atomic::Ordering::Relaxed);
            dialog.paused = false;
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
        let cancel = self
            .new_backup_dialog
            .as_ref()
            .map(|dialog| dialog.cancel.clone())
            .unwrap_or_default();
        let pause = self
            .new_backup_dialog
            .as_ref()
            .map(|dialog| dialog.pause.clone())
            .unwrap_or_default();
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

        let executor = cx.background_executor().clone();
        let created_path = path.clone();
        cx.spawn(async move |this, cx| {
            let mut fatal: Option<String> = None;
            for (kind, name) in selected {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                // Pause parks the loop here, between objects, until Resume (or Stop) is pressed.
                // The short sleep keeps the task cooperative instead of spinning on the atomic.
                while pause.load(std::sync::atomic::Ordering::Relaxed)
                    && !cancel.load(std::sync::atomic::Ordering::Relaxed)
                {
                    executor.timer(Duration::from_millis(120)).await;
                }
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
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

            let cancelled = cancel.load(std::sync::atomic::Ordering::Relaxed);
            let _ = this.update(cx, |app, cx| {
                if cancelled {
                    app.backup_log(t!("backup.log.cancelled").to_string(), cx);
                }
                app.finish_backup(fatal, Some(created_path.clone()), cx)
            });
        })
        .detach();
    }

    /// Append a line to whichever backup/restore/extract/dump/run-SQL dialog is open, keeping it
    /// scrolled.
    pub(super) fn backup_log(&mut self, message: String, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.log.push(message.clone());
            dialog.log_scroll.scroll_to_bottom();
        } else if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog.log.push(message.clone());
            dialog.log_scroll.scroll_to_bottom();
        } else if let Some(dialog) = self.extract_dialog.as_mut() {
            dialog.log.push(message.clone());
            dialog.log_scroll.scroll_to_bottom();
        } else if let Some(dialog) = self.dump_dialog.as_mut() {
            dialog.log.push(message.clone());
            dialog.log_scroll.scroll_to_bottom();
        } else if let Some(dialog) = self.run_sql_dialog.as_mut() {
            dialog.log.push(message.clone());
            dialog.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// Pause the running backup: the run loop parks between objects until 继续.
    pub(super) fn backup_pause(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog
                .pause
                .store(true, std::sync::atomic::Ordering::Relaxed);
            dialog.paused = true;
            dialog.log.push(t!("backup.log.paused").to_string());
            dialog.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// Resume a paused backup.
    pub(super) fn backup_resume(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog
                .pause
                .store(false, std::sync::atomic::Ordering::Relaxed);
            dialog.paused = false;
            dialog.log.push(t!("backup.log.resumed").to_string());
            dialog.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// Ask the running restore to stop after the current object.
    pub(super) fn restore_abort(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        cx.notify();
    }

    /// Ask the running extract to stop after the current object.
    pub(super) fn extract_abort(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.extract_dialog.as_mut() {
            dialog
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        cx.notify();
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

    fn finish_backup(
        &mut self,
        fatal: Option<String>,
        created: Option<PathBuf>,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog.running = false;
            dialog.paused = false;
            dialog
                .pause
                .store(false, std::sync::atomic::Ordering::Relaxed);
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
            // Select the file this run wrote. Picking index 0 (the oldest backup on disk) left the
            // info pane on a different backup than the one just created.
            if let Some(created) = created {
                self.select_backup_file(&created, cx);
            }
        }
        cx.notify();
    }

    /// Close the Backup window. Called from its footer button, which has the window at hand, so
    /// the window is removed directly (updating it through its handle would re-borrow it).
    pub(super) fn close_new_backup(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        // Abort a running backup so it stops after the current object instead of continuing to
        // write the archive with no window left to report to.
        if let Some(dialog) = self.new_backup_dialog.as_mut() {
            dialog
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
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
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            dialog
                .cancel
                .store(false, std::sync::atomic::Ordering::Relaxed);
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
        let cancel = self
            .restore_dialog
            .as_ref()
            .map(|dialog| dialog.cancel.clone())
            .unwrap_or_default();
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
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
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
            let cancelled = cancel.load(std::sync::atomic::Ordering::Relaxed);
            let _ = this.update(cx, |app, cx| {
                if cancelled {
                    app.backup_log(t!("backup.log.cancelled").to_string(), cx);
                }
                app.finish_restore(None, connection_index, cx)
            });
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
        if let Some(dialog) = self.restore_dialog.as_mut() {
            dialog
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
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
        let title: String = if let Some(dialog) = self.new_backup_dialog.as_ref() {
            if dialog.editing {
                t!("backup.edit_title").to_string()
            } else {
                t!("backup.new_title").to_string()
            }
        } else if let Some(name) = self
            .restore_dialog
            .as_ref()
            .and_then(|dialog| self.backup_files.get(dialog.file_index))
            .map(|file| file.name.clone())
        {
            format!("{} - {}", name, t!("backup.restore_title"))
        } else if let Some(name) = self
            .extract_dialog
            .as_ref()
            .and_then(|dialog| self.backup_files.get(dialog.file_index))
            .map(|file| file.name.clone())
        {
            format!("{} - {}", name, t!("backup.extract_sql"))
        } else if self.dump_dialog.is_some() {
            t!("sqlfile.dump_title").to_string()
        } else if self.run_sql_dialog.is_some() {
            t!("sqlfile.run_title").to_string()
        } else {
            // No dialog is open: drop any log menu/tint left over from the previous one so the
            // next dialog does not open with a stale menu on screen.
            self.log_menu = None;
            self.log_select_all = false;
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
        } else if self.extract_dialog.is_some() {
            (
                self.extract_dialog_body(cx),
                self.extract_dialog_footer(cx).into_any_element(),
            )
        } else if self.dump_dialog.is_some() {
            (self.dump_dialog_body(cx), self.dump_dialog_footer(cx))
        } else if self.run_sql_dialog.is_some() {
            (self.run_sql_dialog_body(cx), self.run_sql_dialog_footer(cx))
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
            })
            .or_else(|| {
                self.extract_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.error.clone())
            })
            .or_else(|| {
                self.dump_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.error.clone())
            })
            .or_else(|| {
                self.run_sql_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.error.clone())
            });
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.dialog_face))
            .text_color(rgb(theme.text))
            .track_focus(&self.backup_window_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    if this.new_backup_dialog.is_some() {
                        this.close_new_backup(window, cx);
                    } else if this.extract_dialog.is_some() {
                        this.close_extract(window, cx);
                    } else if this.dump_dialog.is_some() {
                        this.close_dump(window, cx);
                    } else if this.run_sql_dialog.is_some() {
                        this.close_run_sql(window, cx);
                    } else {
                        this.close_restore(window, cx);
                    }
                }
            }))
            .child(ui::window_title_bar(title, theme, None))
            .child(body);
        if let Some(error) = error {
            root = root.child(backup_error_banner(error, theme));
        }
        let mut root = root.child(footer);
        // The log's 复制 / 全选 menu floats over the whole window (not the log's clipped scroll
        // container), so it is appended at the root and positioned in window space.
        if let Some(position) = self.log_menu {
            root = root.child(self.render_log_context_menu(position, theme, cx));
        }
        root.into_any_element()
    }

    /// The log box's right-click menu: 复制 (the selection, or the whole log when nothing is
    /// selected) and 全选 (tint every line, so 复制 takes the whole log).
    fn render_log_context_menu(
        &self,
        position: Point<Pixels>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let log_text = self.active_log_text();
        let items = div()
            .flex()
            .flex_col()
            .w(px(170.0))
            .p_0p5()
            .child(self.context_item(
                "log-menu-copy",
                t!("backup.copy").to_string(),
                cx.listener(move |app, _event, window, cx| {
                    let selected = gpui_kit::base::TextSelection::selected_text(window, cx);
                    let text = if selected.trim().is_empty() {
                        log_text.clone()
                    } else {
                        selected
                    };
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    app.log_menu = None;
                    app.log_select_all = false;
                    cx.notify();
                }),
            ))
            .child(self.context_item(
                "log-menu-select-all",
                t!("backup.select_all").to_string(),
                cx.listener(|app, _event, _window, cx| {
                    app.log_select_all = true;
                    app.log_menu = None;
                    cx.notify();
                }),
            ));
        ui::popup_panel(theme)
            .id("log-menu")
            .left(position.x)
            .top(position.y)
            .on_mouse_down_out(cx.listener(|app, _event: &MouseDownEvent, _window, cx| {
                app.log_menu = None;
                app.log_select_all = false;
                cx.notify();
            }))
            .child(items)
            .into_any_element()
    }

    /// The log currently shown in the backup window, joined for the 复制 menu item.
    fn active_log_text(&self) -> String {
        let log: Option<&[String]> = if let Some(dialog) = self.new_backup_dialog.as_ref() {
            Some(&dialog.log)
        } else if let Some(dialog) = self.restore_dialog.as_ref() {
            Some(&dialog.log)
        } else if let Some(dialog) = self.extract_dialog.as_ref() {
            Some(&dialog.log)
        } else if let Some(dialog) = self.dump_dialog.as_ref() {
            Some(&dialog.log)
        } else if let Some(dialog) = self.run_sql_dialog.as_ref() {
            Some(&dialog.log)
        } else {
            None
        };
        log.map(|lines| lines.join("\n")).unwrap_or_default()
    }

    /// Open the OS window that hosts the Backup/Restore UI. It is a normal top-level window, so it
    /// can be moved outside the main window and appears in the taskbar.
    ///
    /// The open is deferred: `open_window` renders the new window immediately, and that render
    /// reads `AppView`, so it must not run while an `AppView` borrow is held.
    pub(super) fn open_backup_window(&mut self, title: String, cx: &mut Context<'_, Self>) {
        if self.backup_window.is_some() {
            return;
        }
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        let focus = self.backup_window_focus.clone();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(560.0), px(600.0)), cx);
            let view_weak = weak.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..ui::window_options(title)
                },
                move |window, cx| {
                    #[cfg(target_os = "windows")]
                    crate::win_resize::install(window);
                    // `open_window` does not raise what it opens, so the window can otherwise
                    // appear behind the main one.
                    window.activate_window();
                    let view = cx.new(|cx| BackupWindow::new(view_weak.clone(), &app_entity, cx));
                    let root = cx.new(|cx| gpui_kit::component::Root::new(view, window, cx));
                    // Focus the window root so ESC closes it before any control is focused.
                    window.focus(&focus, cx);
                    root
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
            self.sync_window = None;
            self.sync.options_open = false;
            self.sync.config_name_input = None;
            self.new_backup_dialog = None;
            self.backup_name_input = None;
            self.restore_dialog = None;
            self.extract_dialog = None;
            self.dump_dialog = None;
            self.run_sql_dialog = None;
            self.export_window = None;
            self.export_wizard = None;
            self.export_connections_window = None;
            self.export_connections_dialog = None;
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
            // Abort whatever is running so its task stops after the current object.
            if let Some(dialog) = self.new_backup_dialog.as_mut() {
                dialog
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(dialog) = self.restore_dialog.as_mut() {
                dialog
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(dialog) = self.extract_dialog.as_mut() {
                dialog
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(dialog) = self.dump_dialog.as_mut() {
                dialog
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(dialog) = self.run_sql_dialog.as_mut() {
                dialog
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.backup_window = None;
            self.new_backup_dialog = None;
            self.backup_name_input = None;
            self.restore_dialog = None;
            self.extract_dialog = None;
            self.dump_dialog = None;
            self.run_sql_dialog = None;
            cx.notify();
            return;
        }
        if self
            .sync_window
            .as_ref()
            .is_some_and(|handle| handle.window_id() == id)
        {
            // Closing the transfer window stops its run and any comparison still walking the
            // tables, and drops the preview editors it created.
            self.sync_window_closed(cx);
            self.sync_window = None;
            cx.notify();
            return;
        }
        if self
            .export_window
            .as_ref()
            .is_some_and(|handle| handle.window_id() == id)
        {
            if let Some(wizard) = self.export_wizard.as_ref() {
                wizard
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
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
            if let Some(wizard) = self.import_wizard.as_ref() {
                wizard
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.import_window = None;
            self.import_wizard = None;
            cx.notify();
            return;
        }
        if self
            .export_connections_window
            .as_ref()
            .is_some_and(|handle| handle.window_id() == id)
        {
            // The user closed the 导出连接 window from the OS chrome.
            self.export_connections_window = None;
            self.export_connections_dialog = None;
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
                    &self.log_handle,
                    &stats,
                    theme,
                    self.log_select_all,
                    cx,
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

        let picker = render_object_picker(
            &dialog.objects,
            dialog.loading,
            &self.backup_objects_scroll,
            theme,
            ObjectPickerAction::Backup,
            cx,
        );

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

    /// Whether the backup configuration has changed since the dialog opened (or was last saved),
    /// which is what enables the footer's 保存 button.
    fn backup_config_dirty(&self) -> bool {
        let Some(dialog) = self.new_backup_dialog.as_ref() else {
            return false;
        };
        if dialog.loading || dialog.config_name.trim().is_empty() {
            return false;
        }
        let Some((name, flags)) = dialog.baseline.as_ref() else {
            return false;
        };
        name != dialog.config_name.trim()
            || *flags
                != dialog
                    .objects
                    .iter()
                    .map(|entry| entry.selected)
                    .collect::<Vec<_>>()
    }

    fn new_backup_dialog_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let (running, paused) = self
            .new_backup_dialog
            .as_ref()
            .map(|dialog| (dialog.running, dialog.paused))
            .unwrap_or((false, false));
        // The run control is one button whose label follows the run state: 开始 before a run,
        // 暂停 while it runs and 继续 while it is parked — exactly one of the three is ever shown
        // (matching Navicat). 停止 and 关闭 were both removed: closing the window already cancels
        // the run, so a stop button had nothing left to do.
        let run_button: AnyElement = if !running {
            self.dialog_button(
                "backup-run",
                t!("backup.start").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| this.run_backup(cx)),
            )
            .into_any_element()
        } else if paused {
            self.dialog_button(
                "backup-resume",
                t!("backup.resume").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| this.backup_resume(cx)),
            )
            .into_any_element()
        } else {
            self.dialog_button(
                "backup-pause",
                t!("backup.pause").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| this.backup_pause(cx)),
            )
            .into_any_element()
        };
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
                    .child(self.win_button(
                        "backup-save-config",
                        t!("backup.save").to_string(),
                        if self.backup_config_dirty() {
                            ButtonKind::Normal
                        } else {
                            ButtonKind::Disabled
                        },
                        cx.listener(|this, _event, window, cx| this.save_backup_config(window, cx)),
                    ))
                    .child(run_button),
            )
            .into_any_element()
    }

    /// The small status indicator shown left of the footer's action buttons.
    fn backup_status_indicator(&self, theme: Theme) -> impl IntoElement {
        let (label, color) = match self.new_backup_dialog.as_ref() {
            Some(dialog) if dialog.running && dialog.paused => {
                (t!("backup.status.paused").to_string(), 0xf0a020)
            }
            Some(dialog) if dialog.running => (t!("backup.status.running").to_string(), 0x22b14c),
            Some(dialog) if dialog.error.is_some() => {
                (t!("backup.status.failed").to_string(), theme.danger)
            }
            Some(dialog) if dialog.config_saved => {
                (t!("backup.status.saved").to_string(), 0x22b14c)
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
                    &self.log_handle,
                    &stats,
                    theme,
                    self.log_select_all,
                    cx,
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

        let picker = render_object_picker(
            &dialog.objects,
            false,
            &self.backup_objects_scroll,
            theme,
            ObjectPickerAction::Restore,
            cx,
        );

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
            .when(
                self.restore_dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.running),
                |row| {
                    row.child(self.dialog_button(
                        "restore-stop",
                        t!("backup.stop").to_string(),
                        false,
                        cx.listener(|this, _event, _window, cx| this.restore_abort(cx)),
                    ))
                },
            )
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

    fn extract_dialog_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let active = self
            .extract_dialog
            .as_ref()
            .map(|dialog| dialog.tab)
            .unwrap_or(BackupDialogTab::Objects);
        let output = self
            .extract_dialog
            .as_ref()
            .and_then(|dialog| dialog.output_path.clone())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| t!("backup.extract_no_output").to_string());
        let header = div()
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
                    .child(t!("backup.extract_to").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(12.5))
                    .text_color(rgb(theme.text))
                    .child(output),
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
                "extract-tab-objects",
                t!("backup.tab.objects").to_string(),
                active == BackupDialogTab::Objects,
                cx.listener(|this, _event, _window, cx| {
                    this.set_extract_tab(BackupDialogTab::Objects, cx)
                }),
            ))
            .child(self.backup_tab_button(
                "extract-tab-log",
                t!("backup.tab.log").to_string(),
                active == BackupDialogTab::Log,
                cx.listener(|this, _event, _window, cx| {
                    this.set_extract_tab(BackupDialogTab::Log, cx)
                }),
            ));

        let content = match active {
            BackupDialogTab::Objects => self.render_extract_picker_body(cx),
            BackupDialogTab::Log => {
                let scroll = self
                    .extract_dialog
                    .as_ref()
                    .map(|dialog| dialog.log_scroll.clone())
                    .unwrap_or_default();
                let stats = self
                    .extract_dialog
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
                    self.extract_dialog
                        .as_ref()
                        .map(|dialog| dialog.log.as_slice())
                        .unwrap_or(&[]),
                    &scroll,
                    &self.log_handle,
                    &stats,
                    theme,
                    self.log_select_all,
                    cx,
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

    fn render_extract_picker_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.extract_dialog.as_ref() else {
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
                        "extract-select-all",
                        t!("backup.select_all").to_string(),
                        theme,
                        cx.listener(|this, _event, _window, cx| this.extract_set_all(true, cx)),
                    ))
                    .child(backup_link_button(
                        "extract-select-none",
                        t!("backup.select_none").to_string(),
                        theme,
                        cx.listener(|this, _event, _window, cx| this.extract_set_all(false, cx)),
                    )),
            );

        let picker = render_object_picker(
            &dialog.objects,
            false,
            &self.backup_objects_scroll,
            theme,
            ObjectPickerAction::Extract,
            cx,
        );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(summary)
            .child(picker)
            .into_any_element()
    }

    fn extract_dialog_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .h(px(46.0))
            .px_4()
            .child(self.dialog_button(
                "extract-open",
                t!("backup.open").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.open_extract_output(cx)),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.dialog_button(
                        "extract-run",
                        t!("backup.extract_button").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| {
                            if !this
                                .extract_dialog
                                .as_ref()
                                .map(|dialog| dialog.running)
                                .unwrap_or(false)
                            {
                                this.run_extract(cx);
                            }
                        }),
                    ))
                    .when(
                        self.extract_dialog
                            .as_ref()
                            .is_some_and(|dialog| dialog.running),
                        |row| {
                            row.child(self.dialog_button(
                                "extract-stop",
                                t!("backup.stop").to_string(),
                                false,
                                cx.listener(|this, _event, _window, cx| this.extract_abort(cx)),
                            ))
                        },
                    )
                    .child(self.dialog_button(
                        "extract-close",
                        t!("backup.close").to_string(),
                        false,
                        cx.listener(|this, _event, window, cx| this.close_extract(window, cx)),
                    )),
            )
            .into_any_element()
    }

    pub(super) fn set_extract_tab(&mut self, tab: BackupDialogTab, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.extract_dialog.as_mut() {
            dialog.tab = tab;
        }
        cx.notify();
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

    pub(super) fn backup_tab_button(
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
        // The log summary reads the elapsed time live from the run's start instant, so it only
        // advances when the window repaints. A run can spend minutes inside one large table with
        // no progress event to repaint on, which froze the clock; tick once a second while
        // something is running. The task is tied to this window, so closing it stops the ticking.
        let executor = cx.background_executor().clone();
        let clock_app = app.clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor
                    .timer(Duration::from_millis(BACKUP_CLOCK_TICK_MS))
                    .await;
                let Some(app) = clock_app.upgrade() else {
                    return;
                };
                let running = app.update(cx, |app, _| app.backup_run_in_progress());
                // `this.update` fails once the window is released, which ends the task.
                if this
                    .update(cx, |_, cx| {
                        if running {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
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
                    // The saved confirmation no longer applies once the name is edited.
                    dialog.config_saved = false;
                }
                cx.notify();
            });
        }))
    })
}

/// The dialog's target line: a small muted label plus the fixed connection and database, shown
/// above the tabs.
pub(super) fn backup_target_line(
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
pub(super) fn backup_error_banner(error: String, theme: Theme) -> impl IntoElement {
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

/// Remember the dialog's current name/selection as the state 保存 compares against.
fn capture_backup_baseline(dialog: &mut NewBackupDialog) {
    let flags = dialog.objects.iter().map(|entry| entry.selected).collect();
    dialog.baseline = Some((dialog.config_name.trim().to_string(), flags));
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
pub(super) fn backup_file_key(name: &str) -> String {
    format!("file:{name}")
}

/// The selection key of a saved backup configuration row.
pub(super) fn backup_config_key(name: &str) -> String {
    format!("config:{name}")
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
/// Which dialog a shared object picker belongs to, so its rows toggle the right dialog's selection.
#[derive(Clone, Copy)]
enum ObjectPickerAction {
    Backup,
    Restore,
    Extract,
}

fn render_object_picker(
    objects: &[BackupObjectEntry],
    loading: bool,
    scroll: &ScrollHandle,
    theme: Theme,
    action: ObjectPickerAction,
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
                    .on_click(cx.listener(move |this, _event, _window, cx| match action {
                        ObjectPickerAction::Backup => this.backup_toggle_kind(kind, cx),
                        ObjectPickerAction::Restore => this.restore_toggle_kind(kind, cx),
                        ObjectPickerAction::Extract => this.extract_toggle_kind(kind, cx),
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
                        .when(selected, move |style| {
                            style
                                .bg(rgb(theme.tree_selected_bg))
                                .text_color(rgb(theme.tree_selected_text))
                        })
                        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                        .on_click(cx.listener(move |this, _event, _window, cx| match action {
                            ObjectPickerAction::Backup => this.backup_toggle_object(index, cx),
                            ObjectPickerAction::Restore => this.restore_toggle_object(index, cx),
                            ObjectPickerAction::Extract => this.extract_toggle_object(index, cx),
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
        .track_scroll(scroll)
        .child(list);
    div()
        .relative()
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
        // gpui does not paint a scrollbar for an overflow container, so the picker draws its own
        // (always visible) vertical scrollbar over the list.
        .child(
            Scrollbar::vertical(scroll)
                .mode(ScrollbarMode::Always)
                .id("backup-object-picker-scrollbar"),
        )
        .into_any_element()
}

/// The information-log tab body shared by both dialogs: a Navicat-style summary block, the
/// `[Msg]` log lines, and a green progress bar.
pub(super) fn render_backup_log(
    log: &[String],
    scroll: &ScrollHandle,
    handle: &gpui_kit::base::TextSelectionHandle,
    stats: &LogStats,
    theme: Theme,
    select_all: bool,
    cx: &mut Context<'_, AppView>,
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

    let progress = stats.progress.clamp(0.0, 1.0);
    // The log box opens the 复制 / 全选 menu on right-click and clears a 全选 tint on a plain
    // click. The menu itself is drawn by `backup_window_contents` at the window root so it is not
    // clipped by the log's scroll container.
    let log_box = render_log_lines(log, scroll, handle, theme, select_all)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|app, _event, _window, cx| {
                if app.log_select_all || app.log_menu.is_some() {
                    app.log_select_all = false;
                    app.log_menu = None;
                    cx.notify();
                }
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|app, event: &MouseDownEvent, _window, cx| {
                app.log_menu = Some(event.position);
                app.log_select_all = false;
                cx.notify();
            }),
        );
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
        .child(log_box)
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

/// The scrollable `[Msg]` log box: one selectable line per entry (all sharing `handle`, so a drag
/// selects and copies across lines) plus an always-visible vertical scrollbar — gpui paints none
/// for an `overflow_y_scroll` container.
///
/// Shared by the backup/restore/extract/dump logs and the Run SQL File log, so every log view
/// behaves the same. `select_all` tints every line as selected (the app's 全选); the caller adds
/// the right-click menu that turns it on. Returned as a `Div` so `render_backup_log` can attach
/// those handlers to the same panel.
pub(super) fn render_log_lines(
    log: &[String],
    scroll: &ScrollHandle,
    handle: &gpui_kit::base::TextSelectionHandle,
    theme: Theme,
    select_all: bool,
) -> Div {
    let mut content = div().flex().flex_col().gap_0p5().p_2().w_full();
    if log.is_empty() {
        content = content.child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child("--"),
        );
    } else {
        for (index, line) in log.iter().enumerate() {
            let mut row = div()
                .w_full()
                .text_size(px(12.0))
                .text_color(rgb(theme.text));
            if select_all {
                // A 全选 "selection": the kit cannot program-select across the runs, so tint
                // every line and let 复制 take the whole log.
                row = row
                    .bg(rgba((theme.grid_selection_bg << 8) | 0x59))
                    .text_color(rgb(theme.grid_selection_text));
            }
            content = content.child(
                row.child(
                    SelectableText::with_handle(
                        ("log-line", index as u64),
                        handle.clone(),
                        format!("[Msg] {line}"),
                    )
                    .document_order(index as u64)
                    // A visible selection tint even when the kit's global theme selection
                    // token is not initialised for this window.
                    .selection_color(rgba((theme.grid_selection_bg << 8) | 0x59).into()),
                ),
            );
        }
    }
    let mut panel = div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .mx_3()
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(theme.border))
        .overflow_hidden();
    // The 复制日志 button sits at the log box's top-right corner, so it copies the whole log
    // without needing a text selection.
    if !log.is_empty() {
        let text = log.join("\n");
        panel = panel.child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .items_center()
                .px_1()
                .pt_1()
                .child(ui::popup_button(
                    "log-copy",
                    t!("sqlfile.copy_log").to_string(),
                    false,
                    theme,
                    move |_event, _window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    },
                )),
        );
    }
    panel
        .child(
            div()
                .id("log-lines")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_y_scroll()
                .track_scroll(scroll)
                .child(content),
        )
        .child(
            Scrollbar::vertical(scroll)
                .mode(ScrollbarMode::Always)
                .id("log-lines-scrollbar"),
        )
}

/// One `label    value` row of the log summary block.
pub(super) fn log_summary_row(label: String, value: String, theme: Theme) -> impl IntoElement {
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
pub(super) fn format_count(value: usize) -> String {
    format_thousands(value as u64)
}

/// The local wall-clock timestamp used by the backup/restore logs.
pub(super) fn now_timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Format an elapsed duration as `HH:MM:SS`.
pub(super) fn format_hms(elapsed: std::time::Duration) -> String {
    let total = elapsed.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    )
}

/// The summary shown above the info log, mirroring Navicat's backup/restore window.
pub(super) struct LogStats {
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
pub(super) fn log_stats(
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

/// The sort value of one Backup 详细列表 row: 名称 / 修改日期 / 文件大小 / 备注. A saved
/// configuration has no date or size of its own, so it sorts into the empty group there.
fn backup_sort_key(entry: &BackupEntry, app: &AppView, column: usize) -> SortKey {
    match entry {
        BackupEntry::File(index) => {
            let Some(file) = app.backup_files.get(*index) else {
                return SortKey::Empty;
            };
            match column {
                1 => SortKey::text(
                    &file
                        .modified
                        .map(format_file_time)
                        .unwrap_or_else(|| "--".to_string()),
                ),
                2 => SortKey::number(Some(file.size as f64)),
                3 => SortKey::text(&single_line(&file.manifest.comment)),
                _ => SortKey::text(&file.name),
            }
        }
        BackupEntry::Config(index) => {
            let Some(config) = app.backup_configs.get(*index) else {
                return SortKey::Empty;
            };
            match column {
                1 | 2 => SortKey::Empty,
                3 => SortKey::text(&saved_config_counts(config)),
                _ => SortKey::text(&config.name),
            }
        }
    }
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

/// Open `path` with the OS default application (the Extract window's 打开 button).
pub(super) fn open_with_default_app(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    {
        // `start` is a cmd builtin; the empty "" fills its window-title slot.
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod log_selection_tests {
    use super::*;
    use gpui::{
        Bounds, MouseButton, Point, TestAppContext, VisualTestContext, WindowBounds, WindowOptions,
        point, size,
    };
    use gpui_kit::base::{TextSelection, TextSelectionHandle};

    struct LogHarness {
        handle: TextSelectionHandle,
        scroll: ScrollHandle,
    }

    impl Render for LogHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(render_log_lines(
                &["hello world".to_string()],
                &self.scroll,
                &self.handle,
                Theme::light(),
                false,
            ))
        }
    }

    /// Mirrors the Backup/Run-SQL window: a `gpui_kit::open_window` window (whose root is the base
    /// `Root`, carrying the text-selection layer). A drag over the log must select its text.
    #[gpui_kit::test]
    fn log_lines_can_be_selected(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window_handle = cx.update(|cx| {
            let view = cx.new(|cx| LogHarness {
                handle: TextSelectionHandle::new(String::new(), cx),
                scroll: ScrollHandle::new(),
            });
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(600.), px(300.)),
                    })),
                    ..Default::default()
                },
                cx,
                |_, _| view,
            )
            .expect("open test window")
            .0
        });
        let mut cx = VisualTestContext::from_window(window_handle, cx);
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_mouse_down(
            point(px(4.), px(45.)),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(300.), px(45.)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(px(300.), px(45.)),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(
                TextSelection::selected_text(window, cx).contains("hello world"),
                "log text should be selectable"
            );
        });
    }
}
