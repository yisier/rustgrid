//! The `Users` main tab: the server's account list, its toolbar and the actions that open the
//! user editor and the privilege manager.

use super::*;

/// The window-space key of one account, used by the Users list's multi-selection.
fn user_key(account: &UserAccount) -> String {
    account.label()
}

/// One column of the Users 详细列表. Which columns are shown depends on the engine's
/// [`UserEditorSpec`]: MySQL adds the resource limits and the super-user flag, other engines only
/// the columns they actually report.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UsersColumn {
    Name,
    /// 用户 / 登录名 / 组 / 角色, for engines that separate them.
    Kind,
    MaxQuestions,
    MaxUpdates,
    MaxConnections,
    MaxUserConnections,
    SuperUser,
}

impl UsersColumn {
    /// The column's header label.
    fn header(self) -> String {
        match self {
            UsersColumn::Name => t!("common.name").to_string(),
            UsersColumn::Kind => t!("user.col.kind").to_string(),
            UsersColumn::MaxQuestions => t!("user.col.max_questions").to_string(),
            UsersColumn::MaxUpdates => t!("user.col.max_updates").to_string(),
            UsersColumn::MaxConnections => t!("user.col.max_connections").to_string(),
            UsersColumn::MaxUserConnections => t!("user.col.max_user_connections").to_string(),
            UsersColumn::SuperUser => t!("user.col.superuser").to_string(),
        }
    }

    /// The narrowest the column may be (its content-fitted width can be larger).
    fn min_width(self) -> f32 {
        match self {
            UsersColumn::Name => 180.0,
            UsersColumn::SuperUser => 64.0,
            _ => 72.0,
        }
    }

    /// The account's value for the column as a sort key: the resource limits compare numerically,
    /// everything else as text.
    fn sort_key(self, account: &UserAccount) -> SortKey {
        match self {
            UsersColumn::Name => SortKey::text(&account.label()),
            UsersColumn::Kind => SortKey::text(&t!(account.kind.label_key())),
            UsersColumn::MaxQuestions => SortKey::number(Some(account.max_questions as f64)),
            UsersColumn::MaxUpdates => SortKey::number(Some(account.max_updates as f64)),
            UsersColumn::MaxConnections => SortKey::number(Some(account.max_connections as f64)),
            UsersColumn::MaxUserConnections => {
                SortKey::number(Some(account.max_user_connections as f64))
            }
            UsersColumn::SuperUser => SortKey::text(&if account.is_super_user() {
                t!("common.yes").to_string()
            } else {
                t!("common.no").to_string()
            }),
        }
    }

    /// The account's value for the column. 名称 renders a badge plus this text, so it returns the
    /// account label; every other column is plain text.
    fn value(self, account: &UserAccount) -> String {
        match self {
            UsersColumn::Name => account.label(),
            UsersColumn::Kind => t!(account.kind.label_key()).to_string(),
            UsersColumn::MaxQuestions => account.max_questions.to_string(),
            UsersColumn::MaxUpdates => account.max_updates.to_string(),
            UsersColumn::MaxConnections => account.max_connections.to_string(),
            UsersColumn::MaxUserConnections => account.max_user_connections.to_string(),
            UsersColumn::SuperUser => if account.is_super_user() {
                t!("common.yes")
            } else {
                t!("common.no")
            }
            .to_string(),
        }
    }
}

/// Which principals the Users tab lists. Engines that separate roles from users get a 用户 / 角色
/// switch (Navicat's 用户 dropdown), so a role is never mistaken for an account that can log in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UserScope {
    Users,
    Roles,
}

impl UserScope {
    /// The segment's i18n label.
    fn label_key(self) -> &'static str {
        match self {
            UserScope::Users => "user.scope.users",
            UserScope::Roles => "user.scope.roles",
        }
    }

    /// The account kind the scope lists; the 用户 scope also keeps groups (SQL Server's Windows
    /// groups are logins, not roles).
    fn matches(self, kind: AccountKind) -> bool {
        match self {
            UserScope::Users => kind != AccountKind::Role,
            UserScope::Roles => kind == AccountKind::Role,
        }
    }
}

impl AppView {
    /// Whether the Users tab offers the 用户 / 角色 switch: the driver's editor must declare roles
    /// **and** the server must report that it has them (MySQL ≤ 5.7 has none, and offering an empty
    /// 角色 list whose every write would fail with a syntax error is worse than not offering it).
    fn users_scope_switch(&self) -> bool {
        self.users_roles_supported
            && self.users_connection.is_some_and(|index| {
                self.user_editor_spec(index)
                    .kinds
                    .contains(&AccountKind::Role)
            })
    }

    /// The scope the Users tab is actually showing: the chosen one, unless the server has no roles,
    /// in which case there is nothing to switch to.
    fn effective_users_scope(&self) -> UserScope {
        if self.users_scope_switch() {
            self.users_scope
        } else {
            UserScope::Users
        }
    }

    /// The account kind a **new** account gets from the scope the list is showing, so opening the
    /// create dialog from the 角色 list produces a role rather than a user.
    pub(super) fn scope_account_kind(&self) -> AccountKind {
        match self.effective_users_scope() {
            UserScope::Users => AccountKind::User,
            UserScope::Roles => AccountKind::Role,
        }
    }

    /// The badge icon for one account kind: a person for a user/login, a pair for a group, a shield
    /// for a role — so the three never look alike in either layout.
    fn user_kind_icon(kind: AccountKind) -> &'static str {
        match kind {
            AccountKind::User => "icons/user.svg",
            AccountKind::Group => "icons/users.svg",
            AccountKind::Role => "icons/role.svg",
        }
    }

    /// The badge tint for one account kind: roles take the accent so they stand out from the
    /// accounts that can log in.
    fn user_kind_color(theme: Theme, kind: AccountKind) -> u32 {
        match kind {
            AccountKind::Role => theme.brand,
            _ => theme.icon_users,
        }
    }

    /// Switch the Users tab between users and roles.
    pub(super) fn set_users_scope(&mut self, scope: UserScope, cx: &mut Context<'_, Self>) {
        if self.users_scope == scope {
            return;
        }
        self.users_scope = scope;
        // The selection belongs to the list that was on screen; dropping it keeps 编辑/删除 from
        // firing at a row the new scope no longer shows.
        self.users_selection.clear();
        self.selected_user = None;
        cx.notify();
    }

    /// The 用户 / 角色 switch, styled like the 详细列表/平铺网格 toggle so the toolbar reads as
    /// one set of controls.
    fn render_users_scope_switch(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let scope = self.effective_users_scope();
        let weak = cx.weak_entity();
        let mut group = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .flex_none()
            .rounded(px(7.0))
            .bg(rgb(theme.button_bg))
            .border_1()
            .border_color(rgb(theme.border));
        for option in [UserScope::Users, UserScope::Roles] {
            let active = option == scope;
            let weak = weak.clone();
            group = group.child(
                div()
                    .id(SharedString::from(format!(
                        "user-scope-{}",
                        option.label_key()
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .h(px(22.0))
                    .px_2()
                    .flex_none()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .text_size(px(12.0))
                    .when(active, move |style| {
                        style
                            .bg(rgb(theme.dialog_bg))
                            .text_color(rgb(theme.text))
                            .font_weight(FontWeight::MEDIUM)
                            .border_1()
                            .border_color(rgb(theme.border))
                    })
                    .when(!active, move |style| {
                        style
                            .text_color(rgb(theme.text_muted))
                            .hover(move |style| style.text_color(rgb(theme.text)))
                    })
                    .on_click(move |_event, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.set_users_scope(option, cx));
                    })
                    .child(t!(option.label_key()).to_string()),
            );
        }
        group
    }

    /// The connection whose accounts the Users tab shows: the connection tree's current selection
    /// (walking a database/category/table row up to its connection), or the first open connection.
    pub(super) fn users_connection_index(&self, cx: &App) -> Option<usize> {
        let tree = self.tree_pane.read(cx);
        if let Some((connection_index, _, _, _)) = tree.selected_table.clone() {
            return Some(connection_index);
        }
        if let Some(selected) = tree.selected.as_deref() {
            for prefix in ["conn-", "db-", "cat-", "keys-"] {
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
            self.finish_users_refresh(true, cx);
            cx.notify();
            return;
        };
        // An engine without accounts (SQLite) reports an empty list instead of an error.
        if !self.driver_supports(connection_index, DriverCapability::Users) {
            self.users = Loadable::Loaded(Vec::new());
            self.finish_users_refresh(true, cx);
            cx.notify();
            return;
        }
        let Some(connection) = self.connection_arc(connection_index) else {
            self.users = Loadable::Failed(t!("info.not_connected").to_string());
            self.finish_users_refresh(false, cx);
            cx.notify();
            return;
        };
        self.users = Loadable::Loading;
        self.users_roles_supported = false;
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            // Ask for the role capability in the same round trip as the list, so the 用户 / 角色
            // switch only appears once we know the server can actually fill the 角色 side.
            let result = runtime
                .spawn(async move {
                    let users = connection.list_users().await;
                    let roles = users.is_ok() && connection.roles_supported().await;
                    (users, roles)
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                let ok = matches!(&result, Ok((Ok(_), _)));
                match result {
                    Ok((users, roles)) => {
                        app.users_roles_supported = roles;
                        app.users = match users {
                            Ok(users) => Loadable::Loaded(users),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                    }
                    Err(error) => {
                        app.users_roles_supported = false;
                        app.users = Loadable::Failed(error.to_string());
                    }
                }
                if let Loadable::Loaded(users) = &app.users
                    && app.selected_user.is_some_and(|index| index >= users.len())
                {
                    app.selected_user = None;
                }
                app.finish_users_refresh(ok, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Explicit 刷新 of the Users list: reload the accounts and confirm the click with a toast.
    pub(super) fn refresh_users_feedback(&mut self, cx: &mut Context<'_, Self>) {
        self.users_refresh_feedback = true;
        self.refresh_toast(ToastKind::Info, t!("grid.refreshing").to_string(), cx);
        self.refresh_users(cx);
    }

    /// Report an explicit Users-list refresh as finished, if one is waiting.
    fn finish_users_refresh(&mut self, ok: bool, cx: &mut Context<'_, Self>) {
        if std::mem::take(&mut self.users_refresh_feedback) {
            let (kind, message) = if ok {
                (ToastKind::Success, t!("grid.refreshed").to_string())
            } else {
                (ToastKind::Error, t!("grid.refresh_failed").to_string())
            };
            self.refresh_toast(kind, message, cx);
        }
    }

    /// The indices of the accounts matching the current search text.
    fn filtered_users(&self) -> Vec<usize> {
        let Loadable::Loaded(users) = &self.users else {
            return Vec::new();
        };
        let needle = self.user_search.trim().to_ascii_lowercase();
        let scope = self.effective_users_scope();
        users
            .iter()
            .enumerate()
            .filter(|(_, account)| {
                // The scope decides which principals this list is about: a 角色 list must not mix
                // in the accounts that can log in, and vice versa.
                scope.matches(account.kind)
                    && (needle.is_empty() || account.label().to_ascii_lowercase().contains(&needle))
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// The visible accounts as `(index, selection key, account)`, in display order (the active
    /// 详细列表 sort, by name unless a header was clicked).
    fn visible_users(&self) -> Vec<(usize, String, UserAccount)> {
        let Loadable::Loaded(users) = &self.users else {
            return Vec::new();
        };
        let mut visible: Vec<(usize, String, UserAccount)> = self
            .filtered_users()
            .into_iter()
            .filter_map(|index| {
                users
                    .get(index)
                    .map(|account| (index, user_key(account), account.clone()))
            })
            .collect();
        // The sort column is an index into the columns the 详细列表 currently shows.
        if let Some(column) = self
            .users_detail_columns()
            .get(self.users_sort.column)
            .copied()
        {
            self.users_sort
                .sort_by(&mut visible, |(_, _, account)| column.sort_key(account));
        }
        visible
    }

    /// Ctrl/Cmd+F while the Users list holds focus moves the caret into the toolbar's search box,
    /// so filtering the list never needs the mouse. The box sits above the list's key context, so
    /// taking focus also leaves that context — like clicking it does.
    pub(super) fn focus_user_search(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.user_search_input
            .update(cx, |input, cx| input.focus_state(window, cx));
        cx.notify();
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
        // The 用户 / 角色 switch decides what the whole tab — list, toolbar labels, new dialog —
        // is about, exactly like Navicat's 用户 dropdown.
        let scope = self.effective_users_scope();
        let (new_label, edit_label, delete_label) = match scope {
            UserScope::Users => ("user.new", "user.edit", "user.delete"),
            UserScope::Roles => ("user.new_role", "user.edit_role", "user.delete_role"),
        };
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
                    .when(self.users_scope_switch(), |row| {
                        row.child(self.render_users_scope_switch(cx))
                            .child(toolbar_separator(theme))
                    })
                    .child(self.toolbar_item(
                        "user-new",
                        "icons/add_field.svg",
                        t!(new_label).to_string(),
                        has_connection,
                        cx.listener(|this, _event, _window, cx| this.open_create_user(cx)),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "user-edit",
                        "icons/insert_field.svg",
                        t!(edit_label).to_string(),
                        has_selection,
                        cx.listener(|this, _event, _window, cx| this.open_selected_user(cx)),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "user-delete",
                        "icons/delete_field.svg",
                        t!(delete_label).to_string(),
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
                        self.users_connection.is_some_and(|index| {
                            self.user_editor_spec(index).object_privilege_manager
                        }),
                        cx.listener(|this, _event, _window, cx| this.open_privilege_manager(cx)),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "user-refresh",
                        "icons/refresh.svg",
                        t!("common.refresh").to_string(),
                        self.users_connection.is_some(),
                        cx.listener(|this, _event, _window, cx| this.refresh_users_feedback(cx)),
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
            .key_context(USER_LIST_CONTEXT)
            .on_action(cx.listener(|this, _: &UserFocusSearch, window, cx| {
                this.focus_user_search(window, cx);
            }))
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

    /// The Users 详细列表 columns for the current connection's engine, in render order.
    fn users_detail_columns(&self) -> Vec<UsersColumn> {
        let spec = self
            .users_connection
            .map(|index| self.user_editor_spec(index))
            .unwrap_or_else(UserEditorSpec::mysql);
        let mut columns = vec![UsersColumn::Name];
        // Only engines that separate users from roles (or groups) have anything to put in a 类型
        // column; everywhere else it would repeat the same word down the list.
        if spec.kinds.len() > 1 {
            columns.push(UsersColumn::Kind);
        }
        if spec.max_questions {
            columns.push(UsersColumn::MaxQuestions);
        }
        if spec.max_updates {
            columns.push(UsersColumn::MaxUpdates);
        }
        if spec.max_connections {
            columns.push(UsersColumn::MaxConnections);
        }
        if spec.max_user_connections {
            columns.push(UsersColumn::MaxUserConnections);
        }
        if spec.list_super_user {
            columns.push(UsersColumn::SuperUser);
        }
        columns
    }

    /// The content-fitted widths of the Users 详细列表 columns, in render order.
    fn users_detail_widths(
        &self,
        visible: &[(usize, String, UserAccount)],
        columns: &[UsersColumn],
    ) -> Vec<f32> {
        columns
            .iter()
            .map(|column| {
                let mut longest = ui::approx_text_width(&column.header());
                if *column == UsersColumn::Name {
                    longest += 30.0;
                }
                for (_, _, account) in visible {
                    let mut value = ui::approx_text_width(&column.value(account));
                    if *column == UsersColumn::Name {
                        value += 30.0;
                    }
                    longest = longest.max(value);
                }
                ui::detail_column_width(longest, column.min_width())
            })
            .collect()
    }

    /// The Users 详细列表: the account's name plus its resource limits and super-user flag.
    fn render_users_detail(
        &self,
        visible: &[(usize, String, UserAccount)],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let columns = self.users_detail_columns();
        let fitted = self.users_detail_widths(visible, &columns);
        let widths = self.users_columns.borrow_mut().resolve(&fitted);
        let mut header = ui::detail_header_row(theme);
        for (index, column) in columns.iter().enumerate() {
            let width = widths[index];
            let cell = ui::detail_header_cell(
                SharedString::from(format!("users-sort-{index}")),
                column.header(),
                self.users_sort.indicator(index),
                theme,
                cx.listener(move |this, _event, _window, cx| {
                    this.users_sort.toggle(index);
                    cx.notify();
                }),
            );
            header = header.child(ui::detail_header_column(
                SharedString::from(format!("users-resize-{index}")),
                width,
                self.users_columns.borrow().resizing(index),
                theme,
                cell,
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
            let value = |text: String| {
                div()
                    .w_full()
                    .text_align(gpui::TextAlign::Left)
                    .text_color(rgb(theme.text_muted))
                    .child(text)
            };
            let mut row = ui::detail_row(
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
                        this.context_menu = Some(ContextMenu {
                            target: ContextTarget::UserList,
                            position: event.position,
                        });
                        cx.notify();
                    }
                }),
            );
            for (column_index, column) in columns.iter().enumerate() {
                let width = widths[column_index];
                let cell: AnyElement = match column {
                    UsersColumn::Name => div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .w(px(width))
                        .flex_none()
                        .overflow_hidden()
                        .text_color(rgb(theme.text))
                        .child(ui::leading_icon_badge(
                            Self::user_kind_icon(account.kind),
                            Self::user_kind_color(theme, account.kind),
                            ui::DETAIL_BADGE_SIZE,
                        ))
                        .child(ui::detail_cell_text(
                            SharedString::from(format!("user-cell-{index}-0")),
                            label.clone(),
                        ))
                        .into_any_element(),
                    _ => div()
                        .w(px(width))
                        .flex_none()
                        .whitespace_nowrap()
                        .child(value(column.value(account)))
                        .into_any_element(),
                };
                row = row.child(cell);
            }
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
                    move |this, event: &MouseDownEvent, _window, cx| {
                        if !this.users_selection.contains(&key) {
                            this.set_user_selection_one(&key);
                        }
                        this.context_menu = Some(ContextMenu {
                            target: ContextTarget::UserList,
                            position: event.position,
                        });
                        cx.notify();
                    }
                }),
            )
            .child(ui::leading_icon_badge(
                Self::user_kind_icon(account.kind),
                Self::user_kind_color(theme, account.kind),
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
        grid = grid.child(self.users_grid.scroller("users-grid-scroll", columns));
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
        if self.click_opens(click_count, modifiers) {
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
            role: account.kind == AccountKind::Role,
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
        // The object-privilege manager only makes sense for engines whose driver implements the
        // object-grant matrix (the toolbar item is disabled otherwise).
        if !self
            .user_editor_spec(connection_index)
            .object_privilege_manager
        {
            return;
        }
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
        let catalog = self.user_privilege_catalog(connection_index);
        let manager = cx.new(|cx| {
            privilege_manager::PrivilegeManager::new(
                connection,
                connection_name.clone(),
                catalog,
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
                    ..ui::window_options(title)
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

    /// Close the 对象权限 window because the connection whose grants it edits was disconnected (see
    /// `AppView::disconnect`): every action in it would fail against the closed connection.
    pub(super) fn close_privilege_manager_for_connection(
        &mut self,
        connection: &Arc<dyn Connection>,
        cx: &mut Context<'_, Self>,
    ) {
        let belongs = self
            .object_privileges
            .as_ref()
            .is_some_and(|manager| Arc::ptr_eq(manager.read(cx).connection(), connection));
        if !belongs {
            return;
        }
        // Deferred for the same reason as `close_connection_window`: removing a window fires
        // gpui's window-closed observers, which re-enter `AppView` from inside its own update.
        let handle = self.object_privileges_window.take();
        self.close_privilege_manager(cx);
        if let Some(handle) = handle {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
    }

    /// The Users status shown in the window's bottom bar.
    pub(super) fn render_users_status(&self) -> AnyElement {
        let theme = self.theme;
        let scope = self.effective_users_scope();
        // The count follows the scope, so the bar always describes the list on screen.
        let count = match &self.users {
            Loadable::Loaded(users) => users
                .iter()
                .filter(|account| scope.matches(account.kind))
                .count(),
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
                    .child(format!("{count} {}", t!(scope.label_key()))),
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
