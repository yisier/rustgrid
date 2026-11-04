//! Connection import/export: the 导入连接 submenu, the import file picker and the 导出连接 dialog.
//!
//! The file formats themselves live in `rustgrid_config::transfer`; this module only maps their
//! entries onto this app (`ConnectionProfile` + the encrypted secret store) and drives the UI.

use std::collections::HashSet;

use super::file_dialog::open_dialog;
use super::*;
use crate::form::now_nanos;
use rustgrid_config::transfer::{self, ParsedConnections, SourceFormat, TransferConnection};

/// Which external format the 导入连接 submenu entry chose.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectionImportKind {
    RustGrid,
    Navicat,
    Dbx,
}

impl ConnectionImportKind {
    fn source_format(self) -> SourceFormat {
        match self {
            ConnectionImportKind::RustGrid => SourceFormat::RustGrid,
            ConnectionImportKind::Navicat => SourceFormat::Navicat,
            ConnectionImportKind::Dbx => SourceFormat::Dbx,
        }
    }

    /// The i18n key of both the submenu label and the picker's title.
    fn label_key(self) -> &'static str {
        match self {
            ConnectionImportKind::RustGrid => "connection.import_rustgrid",
            ConnectionImportKind::Navicat => "connection.import_navicat",
            ConnectionImportKind::Dbx => "connection.import_dbx",
        }
    }

    /// The picker's filter name and the extensions it shows.
    fn filter(self) -> (&'static str, &'static [&'static str]) {
        match self {
            ConnectionImportKind::RustGrid => ("RustGrid", &["rgc"]),
            ConnectionImportKind::Navicat => ("Navicat", &["ncx"]),
            ConnectionImportKind::Dbx => ("DBX", &["json", "dbx"]),
        }
    }
}

impl AppView {
    // ----- import ------------------------------------------------------------------------------

    /// Open the native file picker for `kind`, then parse and import the chosen file. The picker
    /// runs on rfd's own thread (never gpui's), and the file is read on tokio's blocking pool.
    pub(super) fn start_connection_import(
        &mut self,
        kind: ConnectionImportKind,
        cx: &mut Context<'_, Self>,
    ) {
        self.connect_menu_open = false;
        self.context_menu = None;
        self.import_submenu = false;
        let (filter_name, extensions) = kind.filter();
        let title = t!(kind.label_key()).to_string();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let Some(handle) = open_dialog(title, filter_name, extensions)
                .pick_file()
                .await
            else {
                return;
            };
            let path = handle.path().to_path_buf();
            let read = runtime
                .spawn_blocking(move || std::fs::read_to_string(&path))
                .await;
            let content = match read {
                Ok(Ok(content)) => content,
                Ok(Err(error)) => {
                    let message =
                        t!("connection.import_failed", error = error.to_string()).to_string();
                    let _ = this.update(cx, move |app, cx| {
                        app.error_dialog = Some(message.clone());
                        cx.notify();
                    });
                    return;
                }
                Err(error) => {
                    let message =
                        t!("connection.import_failed", error = error.to_string()).to_string();
                    let _ = this.update(cx, move |app, cx| {
                        app.error_dialog = Some(message.clone());
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = this.update(cx, move |app, cx| {
                app.finish_connection_import(kind, &content, cx)
            });
        })
        .detach();
    }

    fn finish_connection_import(
        &mut self,
        kind: ConnectionImportKind,
        content: &str,
        cx: &mut Context<'_, Self>,
    ) {
        match transfer::parse(content, kind.source_format()) {
            Ok(parsed) => self.apply_imported_connections(parsed, cx),
            Err(error) => {
                self.error_dialog =
                    Some(t!("connection.import_failed", error = error.to_string()).to_string());
                cx.notify();
            }
        }
    }

    /// Turn parsed transfer entries into live, disconnected connections and persist them. Entries
    /// whose engine is not registered (or that the parser skipped) are counted and reported, and a
    /// file that carried a tree layout restores its groups and ordering.
    fn apply_imported_connections(
        &mut self,
        parsed: ParsedConnections,
        cx: &mut Context<'_, Self>,
    ) {
        let ParsedConnections {
            connections,
            groups,
            skipped,
            passwords_failed,
        } = parsed;
        let mut names: HashSet<String> = self
            .connections
            .iter()
            .map(|node| node.profile.name.clone())
            .collect();
        let mut imported = 0usize;
        let mut unsupported = skipped;
        // Source profile id -> freshly generated id, so a carried tree layout can be remapped.
        let mut id_map: BTreeMap<String, String> = BTreeMap::new();
        let mut created: Vec<String> = Vec::new();

        for connection in &connections {
            let driver = DriverId::new(connection.driver.clone());
            let Some(descriptor) = self.registry.get(&driver).map(|driver| driver.descriptor())
            else {
                unsupported += 1;
                continue;
            };
            let mut connection = connection.clone();
            // A source that omitted the port still gets the engine's default, unless the engine is
            // file-based (where 0 is correct and the path lives in `database`).
            if connection.port == 0 && !descriptor.is_file_based {
                connection.port = descriptor.default_port;
            }
            let name = unique_connection_name(&connection.name, &mut names);
            let id = format!("conn-{}", now_nanos());
            let profile = transfer::to_profile(&connection, id.clone(), name);
            let password = connection
                .password
                .clone()
                .filter(|value| !value.is_empty());
            let password_saved = password.is_some();
            if let Some(source_id) = connection.id.as_ref() {
                id_map.insert(source_id.clone(), id.clone());
            }
            self.connections.push(ConnectionNode {
                profile,
                password,
                password_saved,
                status: ConnectionStatus::Disconnected,
                databases: Loadable::Idle,
                expanded: false,
            });
            created.push(id.clone());
            if let Some(color) = connection
                .color
                .as_deref()
                .filter(|color| ConnectionColor::from_id(color).is_some())
            {
                self.connection_colors.insert(id, color.to_string());
            }
            imported += 1;
        }

        if imported == 0 {
            let message = if unsupported > 0 {
                t!("connection.import_none_supported").to_string()
            } else {
                t!("connection.import_none").to_string()
            };
            self.error_dialog = Some(message);
            cx.notify();
            return;
        }

        // Place the connections, restoring the file's groups and ordering when it carried a layout.
        if groups.is_empty() {
            for id in &created {
                self.add_connection_to_tree(id.clone(), None);
            }
        } else {
            let mut placed = HashSet::new();
            let mut next = super::groups::next_group_id(&self.tree_layout);
            let entries = import_group_entries(&groups, &id_map, &mut placed, &mut next);
            self.tree_layout.extend(entries);
            for id in &created {
                if !placed.contains(id) {
                    self.add_connection_to_tree(id.clone(), None);
                }
            }
        }

        self.persist_profiles(cx);
        self.persist_secrets(cx);
        self.persist_settings();

        let mut message = t!("connection.import_done", count = imported).to_string();
        if unsupported > 0 {
            message.push(' ');
            message.push_str(&t!("connection.import_skipped", count = unsupported));
        }
        if passwords_failed > 0 {
            message.push(' ');
            message.push_str(&t!(
                "connection.import_password_failed",
                count = passwords_failed
            ));
        }
        self.toast(ToastKind::Success, message, cx);
    }

    // ----- export ------------------------------------------------------------------------------

    /// Open the 导出连接 sub-window (all connections checked, no passwords). Does nothing when
    /// there are no connections to export.
    pub(super) fn open_export_connections_window(&mut self, cx: &mut Context<'_, Self>) {
        if self.connections.is_empty() {
            self.toast(
                ToastKind::Info,
                t!("connection.export_no_connections").to_string(),
                cx,
            );
            return;
        }
        if self.export_connections_window.is_some() {
            return;
        }
        self.export_connections_dialog = Some(ConnectionExportDialog {
            selected: (0..self.connections.len()).collect(),
            include_passwords: false,
            error: None,
        });
        self.open_export_connections_os_window(cx);
        cx.notify();
    }

    /// Open the OS window hosting the 导出连接 dialog (mirrors the Export/Import wizards). Deferred
    /// because `open_window` renders the new window immediately.
    fn open_export_connections_os_window(&mut self, cx: &mut Context<'_, Self>) {
        if self.export_connections_window.is_some() {
            return;
        }
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        let title = t!("connection.export_title").to_string();
        let focus = self.export_connections_focus.clone();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(460.0), px(470.0)), cx);
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
                    let view = cx
                        .new(|cx| ExportConnectionsWindow::new(view_weak.clone(), &app_entity, cx));
                    let root = cx.new(|cx| gpui_kit::component::Root::new(view, window, cx));
                    // Focus the window root so ESC closes it before any control is focused.
                    window.focus(&focus, cx);
                    root
                },
            );
            match opened {
                Ok(handle) => app.update(cx, |app, cx| {
                    app.export_connections_window = Some(handle);
                    cx.notify();
                }),
                Err(error) => app.update(cx, |app, cx| {
                    app.export_connections_dialog = None;
                    app.error_dialog = Some(error.to_string());
                    cx.notify();
                }),
            }
        });
    }

    /// Close the 导出连接 window (called from its footer / ESC, where the window is at hand).
    pub(super) fn export_connections_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.export_connections_dialog = None;
        self.export_connections_window = None;
        window.remove_window();
        cx.notify();
    }

    /// 确定: pick a destination with the OS save dialog, then write the checked connections there.
    /// The picker runs on rfd's own thread via `spawn_in`, so the window can be closed once the
    /// file is written.
    pub(super) fn export_connections_confirm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((selected, include_passwords)) =
            self.export_connections_dialog.as_ref().map(|dialog| {
                (
                    dialog.selected.iter().copied().collect::<Vec<usize>>(),
                    dialog.include_passwords,
                )
            })
        else {
            return;
        };
        if selected.is_empty() {
            self.set_export_connections_error(
                t!("connection.export_none_selected").to_string(),
                cx,
            );
            return;
        }

        // Export in connection-tree order and carry the groups, so an import restores the layout.
        let order = self.export_connection_order(&selected);
        let selected_ids: HashSet<String> = order
            .iter()
            .filter_map(|index| self.connections.get(*index))
            .map(|node| node.profile.id.clone())
            .collect();
        let mut connections = Vec::with_capacity(order.len());
        for index in &order {
            let Some(node) = self.connections.get(*index) else {
                continue;
            };
            connections.push(TransferConnection {
                id: Some(node.profile.id.clone()),
                name: node.profile.name.clone(),
                driver: node.profile.driver.as_str().to_string(),
                host: node.profile.host.clone(),
                port: node.profile.port,
                username: node.profile.username.clone(),
                database: node.profile.database.clone(),
                options: node.profile.options.clone(),
                settings: node.profile.settings.clone(),
                password: if include_passwords {
                    node.password.clone()
                } else {
                    None
                },
                color: self.connection_colors.get(&node.profile.id).cloned(),
            });
        }
        let groups = export_groups(&self.tree_layout, &selected_ids);
        let count = connections.len();
        let file = transfer::build_rustgrid_file(connections, groups);
        let text = match transfer::serialize_rustgrid(&file) {
            Ok(text) => text,
            Err(error) => {
                self.set_export_connections_error(error.to_string(), cx);
                return;
            }
        };

        let title = t!("connection.export_title").to_string();
        let directory = super::export::default_export_dir();
        cx.spawn_in(window, async move |this, cx| {
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_title(title)
                .add_filter("RustGrid", &["rgc"]);
            if !directory.is_empty() {
                dialog = dialog.set_directory(&directory);
            }
            dialog = dialog.set_file_name(transfer::RUSTGRID_FILE_NAME);
            let Some(handle) = dialog.save_file().await else {
                return;
            };
            let path = handle.path().to_path_buf();
            let write_result = std::fs::write(&path, text);
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| match write_result {
                    Ok(()) => {
                        app.export_connections_close(window, cx);
                        app.toast(
                            ToastKind::Success,
                            t!("connection.export_done", count = count).to_string(),
                            cx,
                        );
                    }
                    Err(error) => app.set_export_connections_error(
                        t!("connection.export_failed", error = error.to_string()).to_string(),
                        cx,
                    ),
                });
            });
        })
        .detach();
    }

    fn set_export_connections_error(&mut self, message: String, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.export_connections_dialog.as_mut() {
            dialog.error = Some(message);
        }
        cx.notify();
    }

    /// The selected connection indices in connection-tree order: groups are walked depth-first so
    /// the exported file keeps the order the user sees. Any selected connection the layout does not
    /// mention is appended in index order.
    fn export_connection_order(&self, selected: &[usize]) -> Vec<usize> {
        use std::collections::HashMap;

        fn walk(
            entries: &[GroupEntry],
            selected: &HashSet<usize>,
            by_id: &HashMap<&str, usize>,
            seen: &mut HashSet<usize>,
            order: &mut Vec<usize>,
        ) {
            for entry in entries {
                match entry {
                    GroupEntry::Connection(id) => {
                        if let Some(index) = by_id.get(id.as_str())
                            && selected.contains(index)
                            && seen.insert(*index)
                        {
                            order.push(*index);
                        }
                    }
                    GroupEntry::Group(group) => {
                        walk(&group.children, selected, by_id, seen, order);
                    }
                }
            }
        }

        let selected_set: HashSet<usize> = selected.iter().copied().collect();
        let by_id: HashMap<&str, usize> = self
            .connections
            .iter()
            .enumerate()
            .map(|(index, node)| (node.profile.id.as_str(), index))
            .collect();
        let mut order = Vec::new();
        let mut seen = HashSet::new();
        walk(
            &self.tree_layout,
            &selected_set,
            &by_id,
            &mut seen,
            &mut order,
        );
        for index in selected {
            if seen.insert(*index) {
                order.push(*index);
            }
        }
        order
    }

    /// Toggle one connection's check box in the export dialog.
    pub(super) fn toggle_export_connection(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.export_connections_dialog.as_mut()
            && !dialog.selected.remove(&index)
        {
            dialog.selected.insert(index);
        }
        cx.notify();
    }

    pub(super) fn set_all_export_connections(
        &mut self,
        selected: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.export_connections_dialog.as_mut() {
            if selected {
                dialog.selected = (0..self.connections.len()).collect();
            } else {
                dialog.selected.clear();
            }
        }
        cx.notify();
    }

    /// The 导出连接 window contents: titlebar, the checkable connection list, the password option,
    /// the error line and the footer. Rebuilt from `AppView` each frame.
    pub(super) fn export_connections_window_contents(
        &self,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        if self.export_connections_dialog.is_none() {
            return div().into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.dialog_face))
            .text_color(rgb(theme.text))
            .track_focus(&self.export_connections_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.export_connections_close(window, cx);
                }
            }))
            .child(ui::window_title_bar(
                t!("connection.export_title").to_string(),
                theme,
                None,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(self.render_export_connections_body(cx)),
            )
            .child(self.export_connections_footer(cx).into_any_element())
            .into_any_element()
    }

    /// The 导出连接 body: the connection list, its select-all buttons and the password option.
    fn render_export_connections_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.export_connections_dialog.as_ref() else {
            return div().into_any_element();
        };

        let mut list = div()
            .id("export-connections-list")
            .flex()
            .flex_col()
            .w_full()
            .h(px(220.0))
            .overflow_y_scroll()
            .border_1()
            .border_color(rgb(theme.border))
            .rounded_sm()
            .bg(rgb(theme.input_bg))
            .p_1();
        for (index, node) in self.connections.iter().enumerate() {
            let selected = dialog.selected.contains(&index);
            let icon = self
                .registry
                .get(&node.profile.driver)
                .map(|driver| {
                    let descriptor = driver.descriptor();
                    (descriptor.icon, descriptor.icon_style)
                })
                .unwrap_or(("icons/connection.svg", DriverIconStyle::Plain));
            list = list.child(
                div()
                    .id(SharedString::from(format!("export-connection-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .h(px(24.0))
                    .px_1()
                    .rounded_sm()
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.toggle_export_connection(index, cx);
                    }))
                    .child(checkbox_box(selected, theme))
                    .child(tree_driver_icon(icon.0, icon.1, theme.text_muted, false))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(node.profile.name.clone()),
                    ),
            );
        }

        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(
                div()
                    .text_size(px(12.0))
                    .child(t!("connection.export_list_label").to_string()),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(self.win_button(
                        "export-select-all",
                        t!("connection.export_select_all").to_string(),
                        ButtonKind::Normal,
                        cx.listener(|this, _event, _window, cx| {
                            this.set_all_export_connections(true, cx);
                        }),
                    ))
                    .child(self.win_button(
                        "export-deselect-all",
                        t!("connection.export_deselect_all").to_string(),
                        ButtonKind::Normal,
                        cx.listener(|this, _event, _window, cx| {
                            this.set_all_export_connections(false, cx);
                        }),
                    )),
            );

        let include_passwords = dialog.include_passwords;
        body = body.child(
            div()
                .id("export-include-passwords")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .on_click(cx.listener(|this, _event, _window, cx| {
                    if let Some(dialog) = this.export_connections_dialog.as_mut() {
                        dialog.include_passwords = !dialog.include_passwords;
                    }
                    cx.notify();
                }))
                .child(checkbox_box(include_passwords, theme))
                .child(t!("connection.export_password").to_string()),
        );

        if let Some(error) = dialog.error.as_ref() {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.danger))
                    .child(error.clone()),
            );
        }
        body.into_any_element()
    }

    fn export_connections_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .w_full()
            .gap_2()
            .h(px(52.0))
            .px_3()
            .child(ui::dialog_button(
                "export-connections-cancel",
                t!("form.cancel").to_string(),
                false,
                self.theme,
                cx.listener(|this, _event, window, cx| this.export_connections_close(window, cx)),
            ))
            .child(ui::dialog_button(
                "export-connections-ok",
                t!("form.ok").to_string(),
                true,
                self.theme,
                cx.listener(|this, _event, window, cx| this.export_connections_confirm(window, cx)),
            ))
    }

    // ----- the 导入连接 submenu ----------------------------------------------------------------

    /// The 导入连接 row (a submenu trigger): a right-hand flyout with the three formats. Shared by
    /// the New Connection dropdown and the connection tree's blank-area menu.
    pub(super) fn import_connections_submenu_item(
        &self,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let open = self.import_submenu;
        let zone = self.import_zone.clone();
        let bounds_slot = self.import_zone.clone();
        let mut base = div()
            .relative()
            .flex()
            .items_center()
            .w_full()
            .h(px(26.0))
            .px_3()
            .gap_1()
            .text_size(px(12.5));
        if open {
            base = base.on_children_prepainted(move |bounds, _window, _cx| {
                bounds_slot.borrow_mut().flyout = bounds.last().copied();
            });
        } else {
            bounds_slot.borrow_mut().flyout = None;
        }
        let row = base
            .id("import-connections")
            .cursor_pointer()
            .hover(move |style| {
                style
                    .bg(rgb(theme.tree_hover_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                // Open on hover, and close again once the pointer leaves the row and its flyout.
                if zone.borrow().stays_open(*hovered, window) {
                    if !this.import_submenu {
                        this.import_submenu = true;
                        cx.notify();
                    }
                } else if this.import_submenu {
                    this.import_submenu = false;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.import_submenu = true;
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(t!("connection.import_connection").to_string()),
            )
            .child(
                svg()
                    .path("icons/chevron-right.svg")
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );
        let row = if open {
            row.child(self.import_connections_flyout(theme, cx))
        } else {
            row
        };
        self.submenu_row(row, self.import_zone.clone())
    }

    fn import_connections_flyout(&self, theme: Theme, cx: &mut Context<'_, Self>) -> AnyElement {
        let entries = [
            (
                "import-connections-rustgrid",
                ConnectionImportKind::RustGrid,
            ),
            ("import-connections-navicat", ConnectionImportKind::Navicat),
            ("import-connections-dbx", ConnectionImportKind::Dbx),
        ];
        let zone = self.import_zone.clone();
        let mut flyout = ui::popup_panel(theme)
            // Abut the trigger row so the pointer never crosses a gap on its way to the flyout.
            .left(relative(1.0))
            .top(px(0.0))
            .w(px(200.0))
            .p_0p5()
            .id("import-connections-flyout")
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if !zone.borrow().stays_open(*hovered, window) && this.import_submenu {
                    this.import_submenu = false;
                    cx.notify();
                }
            }));
        for (id, kind) in entries {
            flyout = flyout.child(
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .w_full()
                    .h(px(26.0))
                    .px_3()
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .hover(move |style| {
                        style
                            .bg(rgb(theme.tree_hover_bg))
                            .text_color(rgb(theme.tree_selected_text))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.start_connection_import(kind, cx);
                    }))
                    .child(t!(kind.label_key()).to_string()),
            );
        }
        flyout.into_any_element()
    }
}

/// A connection name unique among `used`: the base name, or `base (2)`, `base (3)`, ... on a clash.
fn unique_connection_name(base: &str, used: &mut HashSet<String>) -> String {
    let base = base.trim();
    let base = if base.is_empty() { "Connection" } else { base };
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base} ({suffix})");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

/// The connection-tree layout restricted to `selected_ids`: only groups that still hold a selected
/// connection are kept, so an exported file never carries an empty group.
fn export_groups(layout: &[GroupEntry], selected_ids: &HashSet<String>) -> Vec<GroupEntry> {
    let mut out = Vec::new();
    for entry in layout {
        match entry {
            GroupEntry::Connection(id) => {
                if selected_ids.contains(id) {
                    out.push(GroupEntry::Connection(id.clone()));
                }
            }
            GroupEntry::Group(group) => {
                let children = export_groups(&group.children, selected_ids);
                if !children.is_empty() {
                    out.push(GroupEntry::Group(ConnectionGroup {
                        id: group.id,
                        name: group.name.clone(),
                        expanded: group.expanded,
                        children,
                    }));
                }
            }
        }
    }
    out
}

/// Rebuild a file's tree layout with fresh group ids and remapped connection ids. `placed` collects
/// every connection that ended up inside a group; `next` hands out the new group ids.
fn import_group_entries(
    entries: &[GroupEntry],
    id_map: &BTreeMap<String, String>,
    placed: &mut HashSet<String>,
    next: &mut u64,
) -> Vec<GroupEntry> {
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            GroupEntry::Connection(source) => {
                if let Some(id) = id_map.get(source) {
                    placed.insert(id.clone());
                    out.push(GroupEntry::Connection(id.clone()));
                }
            }
            GroupEntry::Group(group) => {
                let children = import_group_entries(&group.children, id_map, placed, next);
                if children.is_empty() {
                    continue;
                }
                let id = *next;
                *next += 1;
                out.push(GroupEntry::Group(ConnectionGroup {
                    id,
                    name: group.name.clone(),
                    expanded: group.expanded,
                    children,
                }));
            }
        }
    }
    out
}

/// The root view of the 导出连接 OS window. It re-renders whenever `AppView` changes so the check
/// boxes, the password option and any error stay live.
pub(super) struct ExportConnectionsWindow {
    app: WeakEntity<AppView>,
    _subscription: Subscription,
}

impl ExportConnectionsWindow {
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

impl Render for ExportConnectionsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        app.update(cx, |app, cx| app.export_connections_window_contents(cx))
    }
}
