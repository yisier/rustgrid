//! The `Users` main tab: the server's account list, its toolbar and the actions that open the
//! user editor and the privilege manager.

use super::*;

/// The window-space key of one account, used by the Users list's multi-selection.
fn user_key(account: &UserAccount) -> String {
    format!("{}@{}", account.user, account.host)
}

impl AppView {
    /// The connection whose accounts the Users tab shows: the connection tree's current selection
    /// (walking a database/category/table row up to its connection), or the first open connection.
    pub(super) fn users_connection_index(&self, cx: &App) -> Option<usize> {
        let tree = self.tree_pane.read(cx);
        if let Some((connection_index, _, _, _)) = tree.selected_table.clone() {
            return Some(connection_index);
        }
        if let Some(selected) = tree.selected.as_deref() {
            for prefix in ["conn-", "db-", "cat-"] {
                if let Some(rest) = selected.strip_prefix(prefix) {
                    let end = rest.find('-').unwrap_or(rest.len());
                    if let Ok(index) = rest[..end].parse::<usize>() {
                        return Some(index);
                    }
                }
            }
        }
        self.connections
            .iter()
            .position(|node| matches!(node.status, ConnectionStatus::Connected(_)))
    }

    /// Keep the Users list in step with the connection tree selection. Called every render; only
    /// does work when the target connection changed.
    pub(super) fn sync_users(&mut self, cx: &mut Context<'_, Self>) {
        if self.main_tab != MainTab::Users {
            return;
        }
        let current = self.users_connection_index(cx);
        if current != self.users_connection {
            self.refresh_users(cx);
        }
    }

    /// (Re)load the account list for the currently selected connection.
    pub(super) fn refresh_users(&mut self, cx: &mut Context<'_, Self>) {
        let connection_index = self.users_connection_index(cx);
        self.users_connection = connection_index;
        self.selected_user = None;
        let Some(connection_index) = connection_index else {
            self.users = Loadable::Loaded(Vec::new());
            cx.notify();
            return;
        };
        let Some(connection) = self.connection_arc(connection_index) else {
            self.users = Loadable::Failed(t!("info.not_connected").to_string());
            cx.notify();
            return;
        };
        self.users = Loadable::Loading;
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.list_users().await })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.users = match result {
                    Ok(Ok(users)) => Loadable::Loaded(users),
                    Ok(Err(error)) => Loadable::Failed(error.to_string()),
                    Err(error) => Loadable::Failed(error.to_string()),
                };
                if let Loadable::Loaded(users) = &app.users
                    && app.selected_user.is_some_and(|index| index >= users.len())
                {
                    app.selected_user = None;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The indices of the accounts matching the current search text.
    fn filtered_users(&self) -> Vec<usize> {
        let Loadable::Loaded(users) = &self.users else {
            return Vec::new();
        };
        let needle = self.user_search.trim().to_ascii_lowercase();
        users
            .iter()
            .enumerate()
            .filter(|(_, account)| {
                needle.is_empty() || account.label().to_ascii_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// The visible accounts as `(index, selection key, account)`, in display order.
    fn visible_users(&self) -> Vec<(usize, String, UserAccount)> {
        let Loadable::Loaded(users) = &self.users else {
            return Vec::new();
        };
        self.filtered_users()
            .into_iter()
            .filter_map(|index| {
                users
                    .get(index)
                    .map(|account| (index, user_key(account), account.clone()))
            })
            .collect()
    }

    /// The Users main tab body: the toolbar plus the account list.
    pub(super) fn render_users(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .bg(rgb(theme.editor_bg))
            .child(self.render_users_toolbar(cx))
            .child(self.render_users_list(cx))
            .into_any_element()
    }

    fn render_users_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let has_selection = self.selected_user.is_some();
        let has_connection =
            matches!(&self.users, Loadable::Loaded(_)) && self.users_connection.is_some();
        let weak = cx.weak_entity();
        let on_select = Rc::new(
            move |mode: ViewMode, _event: &ClickEvent, _window: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |app, cx| app.set_view_mode(VIEW_PAGE_USERS, mode, cx));
            },
        );
        let view_mode = self.view_mode(VIEW_PAGE_USERS);
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(self.toolbar_item(
                        "user-new",
                        "icons/add_field.svg",
                        t!("user.new").to_string(),
                        has_connection,
                        cx.listener(|this, _event, _window, cx| this.open_create_user(cx)),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "user-edit",
                        "icons/insert_field.svg",
                        t!("user.edit").to_string(),
                        has_selection,
                        cx.listener(|this, _event, _window, cx| this.open_selected_user(cx)),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "user-delete",
                        "icons/delete_field.svg",
                        t!("user.delete").to_string(),
                        has_selection,
                        cx.listener(|this, _event, _window, cx| {
                            if let Some(index) = this.selected_user {
                                this.confirm_delete_user(index, cx);
                            }
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "user-privilege-manager",
                        "icons/gear.svg",
                        t!("user.privilege_manager").to_string(),
                        self.users_connection.is_some(),
                        cx.listener(|this, _event, _window, cx| this.open_privilege_manager(cx)),
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
                            .child(self.user_search_input.clone()),
                    ),
            )
    }

    fn render_users_list(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let visible = self.visible_users();

        let body: AnyElement = match &self.users {
            Loadable::Idle | Loadable::Loading => {
                tree_message(t!("common.loading").to_string(), 8.0, theme.text_muted)
                    .into_any_element()
            }
            Loadable::Failed(error) => {
                tree_message(error.clone(), 8.0, theme.danger).into_any_element()
            }
            Loadable::Loaded(users) if users.is_empty() => {
                tree_message(t!("user.empty").to_string(), 8.0, theme.text_muted).into_any_element()
            }
            Loadable::Loaded(_) => {
                let keys: Vec<String> = visible.iter().map(|(_, key, _)| key.clone()).collect();
                match self.view_mode(VIEW_PAGE_USERS) {
                    ViewMode::Detail => self.render_users_detail(&visible, theme, cx),
                    ViewMode::Grid => self.render_users_tiles(&visible, &keys, theme, cx),
                }
            }
        };

        let mut container = div()
            .id("users-list")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.users_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.users_focus, cx);
                    this.begin_marquee(MarqueeTarget::Users, event.position, event.modifiers, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.users_columns.borrow_mut().drag_resize(event) {
                    cx.notify();
                }
                this.drag_marquee(MarqueeTarget::Users, event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.users_columns.borrow_mut().end_resize() {
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
        if let Some(rect) = self.marquee_rect_for(MarqueeTarget::Users) {
            container = container.child(rect);
        }
        container.into_any_element()
    }

    /// The content-fitted widths of the Users 详细列表 columns (名称 + the four resource limits +
    /// 超级用户), in render order.
    fn users_detail_widths(&self, visible: &[(usize, String, UserAccount)]) -> Vec<f32> {
        let yes = t!("common.yes").to_string();
        let no = t!("common.no").to_string();
        let mut longest = [
            ui::approx_text_width(&t!("common.name")) + 30.0,
            ui::approx_text_width(&t!("user.col.max_questions")),
            ui::approx_text_width(&t!("user.col.max_updates")),
            ui::approx_text_width(&t!("user.col.max_connections")),
            ui::approx_text_width(&t!("user.col.max_user_connections")),
            ui::approx_text_width(&t!("user.col.superuser")),
        ];
        for (_, _, account) in visible {
            longest[0] = longest[0].max(ui::approx_text_width(&account.label()) + 30.0);
            longest[1] = longest[1].max(ui::approx_text_width(&account.max_questions.to_string()));
            longest[2] = longest[2].max(ui::approx_text_width(&account.max_updates.to_string()));
            longest[3] =
                longest[3].max(ui::approx_text_width(&account.max_connections.to_string()));
            longest[4] = longest[4].max(ui::approx_text_width(
                &account.max_user_connections.to_string(),
            ));
            longest[5] = longest[5].max(ui::approx_text_width(if account.is_super_user() {
                &yes
            } else {
                &no
            }));
        }
        vec![
            ui::detail_column_width(longest[0], 180.0),
            ui::detail_column_width(longest[1], 72.0),
            ui::detail_column_width(longest[2], 72.0),
            ui::detail_column_width(longest[3], 72.0),
            ui::detail_column_width(longest[4], 72.0),
            ui::detail_column_width(longest[5], 64.0),
        ]
    }

    /// The Users 详细列表: the account's name plus its resource limits and super-user flag.
    fn render_users_detail(
        &self,
        visible: &[(usize, String, UserAccount)],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let fitted = self.users_detail_widths(visible);
        let widths = self.users_columns.borrow_mut().resolve(&fitted);
        let mut header = ui::detail_header_row(theme);
        for (index, label) in [
            t!("common.name").to_string(),
            t!("user.col.max_questions").to_string(),
            t!("user.col.max_updates").to_string(),
            t!("user.col.max_connections").to_string(),
            t!("user.col.max_user_connections").to_string(),
            t!("user.col.superuser").to_string(),
        ]
        .into_iter()
        .enumerate()
        {
            let width = widths[index];
            header = header.child(ui::detail_header_column(
                SharedString::from(format!("users-resize-{index}")),
                width,
                self.users_columns.borrow().resizing(index),
                theme,
                ui::detail_header_cell_plain(label, theme),
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.users_columns
                        .borrow_mut()
                        .begin_resize(index, event.position.x, width);
                    cx.notify();
                }),
            ));
        }

        let mut list = ui::DetailList::new(
            "users-detail",
            &self.users_detail_scroll,
            ui::detail_content_width(&widths),
            header,
        );
        for (index, key, account) in visible {
            let selected = self.users_selection.contains(key);
            let label = account.label();
            let is_super = account.is_super_user();
            let value = |text: String| {
                div()
                    .w_full()
                    .text_align(gpui::TextAlign::Left)
                    .text_color(rgb(theme.text_muted))
                    .child(text)
            };
            let row = ui::detail_row(
                SharedString::from(format!("user-row-{index}")),
                selected,
                theme,
            )
            .on_click(cx.listener({
                let key = key.clone();
                move |this, event: &ClickEvent, _window, cx| {
                    this.hit_user(&key, event.click_count(), event.modifiers(), cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let key = key.clone();
                    move |this, event: &MouseDownEvent, _window, cx| {
                        if !this.users_selection.contains(&key) {
                            this.set_user_selection_one(&key);
                        }
                        cx.notify();
                        let _ = event;
                    }
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
                        "icons/user.svg",
                        theme.icon_users,
                        22.0,
                    ))
                    .child(ui::detail_cell_text(
                        SharedString::from(format!("user-cell-{index}-0")),
                        label,
                    )),
            )
            .child(
                div()
                    .w(px(widths[1]))
                    .flex_none()
                    .child(value(account.max_questions.to_string())),
            )
            .child(
                div()
                    .w(px(widths[2]))
                    .flex_none()
                    .child(value(account.max_updates.to_string())),
            )
            .child(
                div()
                    .w(px(widths[3]))
                    .flex_none()
                    .child(value(account.max_connections.to_string())),
            )
            .child(
                div()
                    .w(px(widths[4]))
                    .flex_none()
                    .child(value(account.max_user_connections.to_string())),
            )
            .child(
                div()
                    .w(px(widths[5]))
                    .flex_none()
                    .whitespace_nowrap()
                    .text_align(gpui::TextAlign::Left)
                    .child(if is_super {
                        t!("common.yes").to_string()
                    } else {
                        t!("common.no").to_string()
                    }),
            );
            list = list.child(list_ops::row_with_rect(
                cx.weak_entity(),
                row,
                MarqueeTarget::Users,
                key.clone(),
            ));
        }
        list.render(theme)
    }

    /// The Users 平铺网格: the accounts in a column-major grid.
    fn render_users_tiles(
        &self,
        visible: &[(usize, String, UserAccount)],
        keys: &[String],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        if visible.is_empty() {
            return div()
                .p_3()
                .text_color(rgb(theme.text_muted))
                .child(t!("common.empty").to_string())
                .into_any_element();
        }
        let width = ui::grid_item_width(
            visible
                .iter()
                .map(|(_, _, account)| ui::approx_text_width(&account.label()))
                .fold(0.0, f32::max),
        );
        let rows = self.users_grid.rows_per_column();
        let mut columns = ui::grid_columns();
        let mut column = ui::grid_column();
        let mut count = 0usize;
        for ((index, key, account), visible_index) in visible.iter().zip(0..) {
            if count == rows {
                columns = columns.child(column);
                column = ui::grid_column();
                count = 0;
            }
            let selected = self.users_selection.contains(key);
            let tile = ui::grid_item_sized(
                SharedString::from(format!("user-tile-{index}")),
                selected,
                theme,
                width,
            )
            .on_click(cx.listener({
                let key = key.clone();
                move |this, event: &ClickEvent, _window, cx| {
                    this.hit_user(&key, event.click_count(), event.modifiers(), cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let key = key.clone();
                    move |this, _event: &MouseDownEvent, _window, cx| {
                        if !this.users_selection.contains(&key) {
                            this.set_user_selection_one(&key);
                        }
                        cx.notify();
                    }
                }),
            )
            .child(ui::leading_icon_badge(
                "icons/user.svg",
                theme.icon_users,
                16.0,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(theme.text))
                    .child(account.label()),
            );
            let _ = visible_index;
            column = column.child(list_ops::row_with_rect(
                cx.weak_entity(),
                tile,
                MarqueeTarget::Users,
                key.clone(),
            ));
            count += 1;
        }
        let _ = keys;
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
        grid = grid.child(self.users_grid.scroller("users-grid-scroll").child(columns));
        if self.users_grid.overflows() {
            grid = grid.child(
                self.users_grid
                    .scrollbar("users-grid-hscrollbar", theme)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            if this.users_grid.begin(event.position.x) {
                                cx.notify();
                            }
                        }),
                    ),
            );
        }
        grid.into_any_element()
    }

    /// The loaded account index for a selection key.
    pub(super) fn user_index_by_key(&self, key: &str) -> Option<usize> {
        match &self.users {
            Loadable::Loaded(users) => users.iter().position(|account| user_key(account) == key),
            _ => None,
        }
    }

    /// One visible account row's selection key.
    fn user_visible_keys(&self) -> Vec<String> {
        self.visible_users()
            .into_iter()
            .map(|(_, key, _)| key)
            .collect()
    }

    /// Apply one row hit (click) under the click's modifier mode.
    pub(super) fn hit_user(
        &mut self,
        key: &str,
        click_count: usize,
        modifiers: Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        let visible = self.user_visible_keys();
        let mode = selection_mode(modifiers);
        let mode = if click_count >= 2 && mode == SelectMode::Replace {
            SelectMode::Replace
        } else {
            mode
        };
        // A Shift-click extends from the anchor rather than adding just the hit row.
        if modifiers.shift {
            self.users_selection.extend_to(&visible, key);
        } else {
            self.users_selection.hit(&visible, key, mode);
        }
        self.on_selection_changed(MarqueeTarget::Users, cx);
        if click_count >= 2 {
            self.open_selected_user(cx);
        }
    }

    /// Replace the Users selection with one row (used by the right-click handler).
    pub(super) fn set_user_selection_one(&mut self, key: &str) {
        self.users_selection.select_one(key.to_string());
        self.selected_user = self.user_index_by_key(key);
        self.info_loaded_for = None;
    }

    /// Open the selected account in the account window.
    pub(super) fn open_selected_user(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.selected_user else {
            return;
        };
        let account = match &self.users {
            Loadable::Loaded(users) => users.get(index).cloned(),
            _ => None,
        };
        let Some(account) = account else {
            return;
        };
        self.open_edit_user((account.user, account.host), cx);
    }

    /// Ask for confirmation before dropping the account at `index`.
    pub(super) fn confirm_delete_user(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(connection_index) = self.users_connection else {
            return;
        };
        let Some(account) = (match &self.users {
            Loadable::Loaded(users) => users.get(index).cloned(),
            _ => None,
        }) else {
            return;
        };
        let label = account.label();
        self.delete_confirm = Some(DeleteConfirm::User {
            connection_index,
            user: account.user,
            host: account.host,
            label,
        });
        cx.notify();
    }

    /// Drop an account and reload the list.
    pub(super) fn delete_user(
        &mut self,
        connection_index: usize,
        user: String,
        host: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.drop_user(&user, &host).await })
                .await;
            let _ = this.update(cx, |app, cx| match result {
                Ok(Ok(())) => app.refresh_users(cx),
                Ok(Err(error)) => {
                    app.error_dialog = Some(error.to_string());
                    cx.notify();
                }
                Err(error) => {
                    app.error_dialog = Some(error.to_string());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Open the 对象权限 window for the current connection (an object-centric view of the grants
    /// the account editor manages from the account side).
    pub(super) fn open_privilege_manager(&mut self, cx: &mut Context<'_, Self>) {
        if self.object_privileges_window.is_some() {
            self.focus_object_privileges(cx);
            return;
        }
        let Some(connection_index) = self.users_connection_index(cx) else {
            return;
        };
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let connection_name = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let theme = self.theme;
        let runtime = self.runtime.clone();
        let manager = cx.new(|cx| {
            privilege_manager::PrivilegeManager::new(
                connection,
                connection_name.clone(),
                runtime,
                theme,
                cx,
            )
        });
        manager.update(cx, |manager, cx| manager.load(cx));
        self.object_privileges = Some(manager.clone());

        let focus = manager.read(cx).focus_handle();
        let title = format!("{} - {}", connection_name, t!("user.privilege_manager"));
        let weak = cx.weak_entity();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(980.0), px(680.0)), cx);
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
                    window.activate_window();
                    let root = cx.new(|cx| gpui_kit::component::Root::new(manager, window, cx));
                    window.focus(&focus, cx);
                    root
                },
            );
            match opened {
                Ok(handle) => app.update(cx, |app, cx| {
                    app.object_privileges_window = Some(handle);
                    cx.notify();
                }),
                Err(error) => app.update(cx, |app, cx| {
                    app.error_dialog = Some(error.to_string());
                    cx.notify();
                }),
            }
        });
        cx.notify();
    }

    /// Raise the already-open 对象权限 window.
    fn focus_object_privileges(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(handle) = self.object_privileges_window {
            let _ = handle.update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            });
        }
    }

    /// Drop the 对象权限 window's state (the OS window was closed).
    pub(super) fn close_privilege_manager(&mut self, cx: &mut Context<'_, Self>) {
        self.object_privileges = None;
        self.object_privileges_window = None;
        cx.notify();
    }

    /// The Users status shown in the window's bottom bar.
    pub(super) fn render_users_status(&self) -> AnyElement {
        let theme = self.theme;
        let count = match &self.users {
            Loadable::Loaded(users) => users.len(),
            _ => 0,
        };
        let connection_name = self
            .users_connection
            .and_then(|index| self.connections.get(index))
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px_2()
            .text_size(px(12.0))
            .child(
                div()
                    .text_color(rgb(theme.text))
                    .child(format!("{count} {}", t!("common.user"))),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_color(rgb(theme.text))
                    .child(tree_icon("icons/connection.svg", theme.icon_connection))
                    .child(connection_name),
            )
            .into_any_element()
    }
}
