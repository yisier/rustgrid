//! The `Users` main tab: the server's account list, its toolbar and the actions that open the
//! user editor and the privilege manager.

use super::*;

/// The height of one account row in the Users list.
const USER_ROW_HEIGHT: f32 = 22.0;

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
                        "user-edit",
                        "icons/insert_field.svg",
                        t!("user.edit").to_string(),
                        has_selection,
                        cx.listener(|this, _event, _window, cx| this.open_selected_user(cx)),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "user-new",
                        "icons/add_field.svg",
                        t!("user.new").to_string(),
                        has_connection,
                        cx.listener(|this, _event, _window, cx| this.open_new_user(cx)),
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
                    .w(px(220.0))
                    .h(px(24.0))
                    .child(self.user_search_input.clone()),
            )
    }

    fn render_users_list(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let rows = self.filtered_users();
        let selected = self.selected_user;

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
            Loadable::Loaded(users) => {
                let mut list = div().flex().flex_col();
                for (row, index) in rows.iter().enumerate() {
                    let Some(account) = users.get(*index) else {
                        continue;
                    };
                    let is_selected = selected == Some(*index);
                    let background = if row % 2 == 1 {
                        theme.row_alt_bg
                    } else {
                        theme.editor_bg
                    };
                    let row_index = *index;
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("user-row-{row}")))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .h(px(USER_ROW_HEIGHT))
                            .px_2()
                            .cursor_pointer()
                            .text_size(px(12.5))
                            .bg(rgb(if is_selected {
                                theme.tree_selected_bg
                            } else {
                                background
                            }))
                            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                                this.select_user(row_index, cx);
                                if event.click_count() == 2 {
                                    this.open_selected_user(cx);
                                }
                            }))
                            .child(tree_icon("icons/user.svg", theme.icon_users))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(account.label()),
                            ),
                    );
                }
                list.into_any_element()
            }
        };

        div()
            .id("users-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.users_scroll)
            .child(body)
            .into_any_element()
    }

    /// Select one account in the Users list, refreshing the details pane.
    pub(super) fn select_user(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.selected_user = Some(index);
        // Force the details pane to reload for the new account.
        self.info_loaded_for = None;
        cx.notify();
    }

    /// Open a blank account in a new user editor.
    pub(super) fn open_new_user(&mut self, cx: &mut Context<'_, Self>) {
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
        self.push_user_editor(connection, connection_name, None, cx);
    }

    /// Open the selected account in a new user editor.
    pub(super) fn open_selected_user(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.selected_user else {
            return;
        };
        let Some(connection_index) = self.users_connection else {
            return;
        };
        let account = match &self.users {
            Loadable::Loaded(users) => users.get(index).cloned(),
            _ => None,
        };
        let Some(account) = account else {
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
        self.push_user_editor(
            connection,
            connection_name,
            Some((account.user, account.host)),
            cx,
        );
    }

    /// Create a user-editor tab and make it the active content. An account already open in an
    /// editor is re-activated instead of opened twice.
    fn push_user_editor(
        &mut self,
        connection: Arc<dyn Connection>,
        connection_name: String,
        account: Option<(String, String)>,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some((user, host)) = account.as_ref()
            && let Some(position) = self.user_editors.iter().position(|editor| {
                let editor = editor.read(cx);
                Arc::ptr_eq(&editor.connection, &connection)
                    && editor
                        .original_user()
                        .is_some_and(|(open_user, open_host)| {
                            open_user == *user && open_host == *host
                        })
            })
        {
            self.activate_user_editor(Some(position), cx);
            return;
        }

        let id = self.next_user_id;
        self.next_user_id += 1;
        let theme = self.theme;
        let app = cx.weak_entity();
        let runtime = self.runtime.clone();
        let editor = cx.new(|cx| {
            user_editor::UserEditor::new(
                id,
                connection,
                connection_name,
                account,
                app,
                runtime,
                theme,
                cx,
            )
        });
        editor.update(cx, |editor, cx| editor.load(cx));
        self.user_editors.push(editor);
        self.active_user_editor = Some(self.user_editors.len() - 1);
        self.active_grid = None;
        self.active_query = None;
        self.active_design = None;
        self.main_tab = MainTab::Users;
        self.privilege_manager = None;
        cx.notify();
    }

    pub(super) fn activate_user_editor(
        &mut self,
        index: Option<usize>,
        cx: &mut Context<'_, Self>,
    ) {
        self.active_user_editor = index;
        self.active_grid = None;
        self.active_query = None;
        self.active_design = None;
        cx.notify();
    }

    pub(super) fn close_user_editor(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.user_editors.len() {
            return;
        }
        self.user_editors.remove(index);
        self.active_user_editor = match self.active_user_editor {
            Some(active) if active == index => None,
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
        cx.notify();
    }

    /// Close every user editor bound to a connection that is going away.
    pub(super) fn close_connection_user_editors(
        &mut self,
        connection: &Arc<dyn Connection>,
        cx: &App,
    ) {
        let active_id = self
            .active_user_editor
            .and_then(|index| self.user_editors.get(index))
            .map(|editor| editor.read(cx).id);
        self.user_editors
            .retain(|editor| !Arc::ptr_eq(&editor.read(cx).connection, connection));
        self.active_user_editor = active_id.and_then(|id| {
            self.user_editors
                .iter()
                .position(|editor| editor.read(cx).id == id)
        });
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

    /// Show the privilege manager for the current connection.
    pub(super) fn open_privilege_manager(&mut self, cx: &mut Context<'_, Self>) {
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
        let app = cx.weak_entity();
        let runtime = self.runtime.clone();
        let manager = cx.new(|cx| {
            privilege_manager::PrivilegeManager::new(
                connection,
                connection_name,
                app,
                runtime,
                theme,
                cx,
            )
        });
        manager.update(cx, |manager, cx| manager.load(cx));
        self.privilege_manager = Some(manager);
        self.active_user_editor = None;
        self.active_grid = None;
        self.active_query = None;
        self.active_design = None;
        self.selected_user = None;
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
