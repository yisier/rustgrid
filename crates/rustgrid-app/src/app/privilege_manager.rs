//! The privilege manager: pick a database or table on the left and edit the privileges of the
//! accounts that hold grants on it as a check-box matrix. Matching Navicat, the matrix lists only
//! accounts with an existing grant on the selected object; 添加权限 adds another account and
//! 删除权限 removes the selected ones (revoking them on Save).

use std::collections::{BTreeMap, BTreeSet};

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PmTab {
    General,
    Sql,
}

impl PmTab {
    const ALL: [PmTab; 2] = [PmTab::General, PmTab::Sql];

    fn label_key(self) -> &'static str {
        match self {
            PmTab::General => "user.tab.general",
            PmTab::Sql => "user.tab.sql",
        }
    }

    fn id(self) -> &'static str {
        match self {
            PmTab::General => "pm-tab-general",
            PmTab::Sql => "pm-tab-sql",
        }
    }
}

const PM_ROW_HEIGHT: f32 = 22.0;

pub(super) struct PrivilegeManager {
    runtime: Arc<Runtime>,
    pub(super) theme: Theme,
    connection: Arc<dyn Connection>,
    pub(super) connection_name: String,

    databases: Vec<String>,
    tables: BTreeMap<String, Vec<String>>,
    expanded: Option<String>,
    /// The selected object: `(database, name)`; the name is empty for a database-wide grant.
    selected: Option<(String, String)>,
    /// Accounts holding a grant on `selected`, with their pending privileges.
    rows: Vec<ObjectPrivilegeRow>,
    /// The rows as loaded, for the SQL preview diff.
    original: Vec<ObjectPrivilegeRow>,
    /// Every account on the server, for the 添加权限 picker.
    accounts: Vec<UserAccount>,
    /// The matrix rows selected for 删除权限.
    selected_rows: BTreeSet<usize>,
    /// Whether the 添加权限 account picker is open.
    add_open: bool,
    /// Keeps the picker's scroll position.
    add_scroll: ScrollHandle,
    tab: PmTab,
    loading: bool,
    saving: bool,
    dirty: bool,
    error: Option<String>,
    sql_scroll: ScrollHandle,
}

impl PrivilegeManager {
    pub(super) fn new(
        connection: Arc<dyn Connection>,
        connection_name: String,
        runtime: Arc<Runtime>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let _ = cx;
        Self {
            runtime,
            theme,
            connection,
            connection_name,
            databases: Vec::new(),
            tables: BTreeMap::new(),
            expanded: None,
            selected: None,
            rows: Vec::new(),
            original: Vec::new(),
            accounts: Vec::new(),
            selected_rows: BTreeSet::new(),
            add_open: false,
            add_scroll: ScrollHandle::new(),
            tab: PmTab::General,
            loading: false,
            saving: false,
            dirty: false,
            error: None,
            sql_scroll: ScrollHandle::new(),
        }
    }

    /// Load the database list for the tree and the account list for the 添加权限 picker.
    pub(super) fn load(&mut self, cx: &mut Context<'_, Self>) {
        let connection = self.connection.clone();
        let runtime = self.runtime.clone();
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move {
                    let databases = connection.list_databases().await;
                    let accounts = connection.list_users().await;
                    (databases, accounts)
                })
                .await;
            let _ = this.update(cx, |manager, cx| {
                manager.loading = false;
                match result {
                    Ok((databases, accounts)) => {
                        match databases {
                            Ok(databases) => {
                                manager.databases =
                                    databases.into_iter().map(|db| db.name).collect();
                            }
                            Err(error) => manager.error = Some(error.to_string()),
                        }
                        manager.accounts = accounts.unwrap_or_default();
                    }
                    Err(error) => manager.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn set_theme(&mut self, theme: Theme, cx: &mut Context<'_, Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        cx.notify();
    }

    fn select_tab(&mut self, tab: PmTab, cx: &mut Context<'_, Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn toggle_database(&mut self, database: String, cx: &mut Context<'_, Self>) {
        let load = if self.expanded.as_deref() == Some(database.as_str()) {
            self.expanded = None;
            false
        } else {
            self.expanded = Some(database.clone());
            !self.tables.contains_key(&database)
        };
        if !load {
            cx.notify();
            return;
        }
        let connection = self.connection.clone();
        let runtime = self.runtime.clone();
        let query = database.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.list_tables(&query).await })
                .await;
            let _ = this.update(cx, |manager, cx| {
                match result {
                    Ok(Ok(tables)) => {
                        manager.tables.insert(
                            database,
                            tables.into_iter().map(|table| table.name).collect(),
                        );
                    }
                    Ok(Err(error)) => manager.error = Some(error.to_string()),
                    Err(error) => manager.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Select an object and load the accounts that hold a grant on it. Matching Navicat, accounts
    /// without an explicit grant are not listed; use 添加权限 to add one.
    fn select_node(&mut self, node: (String, String), cx: &mut Context<'_, Self>) {
        self.selected = Some(node.clone());
        self.rows.clear();
        self.original.clear();
        self.selected_rows.clear();
        self.add_open = false;
        self.dirty = false;
        self.loading = true;
        let connection = self.connection.clone();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let (database, name) = node;
            let result = {
                let connection = connection.clone();
                let database = database.clone();
                let name = name.clone();
                runtime
                    .spawn(
                        async move { connection.object_privilege_matrix(&database, &name).await },
                    )
                    .await
            };
            let _ = this.update(cx, |manager, cx| {
                manager.loading = false;
                if manager.selected.as_ref() != Some(&(database.clone(), name.clone())) {
                    return;
                }
                match result {
                    Ok(Ok(matrix)) => {
                        manager.original = matrix.clone();
                        manager.rows = matrix;
                    }
                    Ok(Err(error)) => manager.error = Some(error.to_string()),
                    Err(error) => manager.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn toggle_cell(&mut self, row: usize, privilege: Privilege, cx: &mut Context<'_, Self>) {
        if let Some(entry) = self.rows.get_mut(row) {
            if !entry.privileges.remove(&privilege) {
                entry.privileges.insert(privilege);
            }
            self.dirty = true;
            cx.notify();
        }
    }

    /// Select/deselect a matrix row for 删除权限.
    fn toggle_row(&mut self, row: usize, cx: &mut Context<'_, Self>) {
        if row >= self.rows.len() {
            return;
        }
        if !self.selected_rows.remove(&row) {
            self.selected_rows.insert(row);
        }
        cx.notify();
    }

    /// Drop the selected accounts from the matrix; Save then revokes their grants.
    fn remove_selected(&mut self, cx: &mut Context<'_, Self>) {
        if self.selected_rows.is_empty() {
            return;
        }
        for index in std::mem::take(&mut self.selected_rows).into_iter().rev() {
            if index < self.rows.len() {
                self.rows.remove(index);
            }
        }
        self.dirty = true;
        cx.notify();
    }

    /// The accounts that do not yet have a row in the matrix, for the 添加权限 picker.
    fn addable_accounts(&self) -> Vec<UserAccount> {
        self.accounts
            .iter()
            .filter(|account| {
                !self
                    .rows
                    .iter()
                    .any(|row| row.user == account.user && row.host == account.host)
            })
            .cloned()
            .collect()
    }

    fn toggle_add(&mut self, cx: &mut Context<'_, Self>) {
        self.add_open = !self.add_open;
        cx.notify();
    }

    fn close_add(&mut self, cx: &mut Context<'_, Self>) {
        if self.add_open {
            self.add_open = false;
            cx.notify();
        }
    }

    /// Add one account to the matrix with no privileges yet; ticking its boxes then Save grants them.
    fn add_account(&mut self, user: String, host: String, cx: &mut Context<'_, Self>) {
        self.add_open = false;
        if !self
            .rows
            .iter()
            .any(|row| row.user == user && row.host == host)
        {
            self.rows.push(ObjectPrivilegeRow::new(user, host));
            self.selected_rows.clear();
            self.selected_rows.insert(self.rows.len() - 1);
            self.dirty = true;
        }
        cx.notify();
    }

    fn preview_sql(&self) -> String {
        let Some((database, name)) = self.selected.as_ref() else {
            return String::new();
        };
        self.connection
            .object_privileges_sql(database, name, &self.original, &self.rows)
    }

    fn save(&mut self, cx: &mut Context<'_, Self>) {
        let Some((database, name)) = self.selected.clone() else {
            return;
        };
        if self.saving {
            return;
        }
        self.saving = true;
        let connection = self.connection.clone();
        let runtime = self.runtime.clone();
        let rows = self.rows.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move {
                    connection
                        .set_object_privileges(&database, &name, &rows)
                        .await
                })
                .await;
            let _ = this.update(cx, |manager, cx| {
                manager.saving = false;
                match result {
                    Ok(Ok(())) => {
                        manager.dirty = false;
                        manager.original = manager.rows.clone();
                    }
                    Ok(Err(error)) => manager.error = Some(error.to_string()),
                    Err(error) => manager.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl Render for PrivilegeManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.editor_bg))
            .child(self.render_toolbar(cx))
            .child(self.render_subtabs(cx))
            .child(self.render_body(cx));
        if self.add_open {
            // A full-pane backdrop closes the picker on any outside click; it also intercepts the
            // 添加权限 button itself, so a second click there closes rather than reopening.
            root = root
                .child(div().absolute().inset_0().occlude().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| this.close_add(cx)),
                ))
                .child(self.render_add_menu(cx));
        }
        root
    }
}

impl PrivilegeManager {
    fn render_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(ui::toolbar_item(
                "pm-save",
                "icons/save.svg",
                t!("design.save").to_string(),
                self.selected.is_some() && !self.saving,
                theme,
                cx.listener(|this, _event, _window, cx| this.save(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(ui::toolbar_item(
                "pm-add",
                "icons/add_field.svg",
                t!("user.privilege.add").to_string(),
                self.selected.is_some(),
                theme,
                cx.listener(|this, _event, _window, cx| this.toggle_add(cx)),
            ))
            .child(ui::toolbar_item(
                "pm-remove",
                "icons/delete_field.svg",
                t!("user.privilege.remove").to_string(),
                !self.selected_rows.is_empty(),
                theme,
                cx.listener(|this, _event, _window, cx| this.remove_selected(cx)),
            ))
    }

    /// The 添加权限 account picker: the server's accounts not already in the matrix.
    fn render_add_menu(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let addable = self.addable_accounts();
        let mut list = ui::popup_panel(theme)
            .id("pm-add-menu")
            .left(px(8.0))
            .top(px(58.0))
            .w(px(260.0))
            .max_h(px(260.0))
            .overflow_y_scroll()
            .track_scroll(&self.add_scroll)
            .p_1();
        if addable.is_empty() {
            list = list.child(tree_message(
                t!("user.privilege.all_granted").to_string(),
                4.0,
                theme.text_muted,
            ));
            return list;
        }
        for (index, account) in addable.into_iter().enumerate() {
            let user = account.user.clone();
            let host = account.host.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("pm-add-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(PM_ROW_HEIGHT))
                    .px_2()
                    .flex_none()
                    .rounded(px(2.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.add_account(user.clone(), host.clone(), cx)
                    }))
                    .child(tree_icon("icons/user.svg", theme.icon_users))
                    .child(account.label()),
            );
        }
        list
    }

    fn render_subtabs(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let mut bar = div()
            .flex()
            .flex_row()
            .items_end()
            .gap_1()
            .h(px(26.0))
            .flex_none()
            .px_2()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border));
        for tab in PmTab::ALL {
            let active = self.tab == tab;
            bar = bar.child(
                div()
                    .id(tab.id())
                    .flex()
                    .items_center()
                    .justify_center()
                    .px_3()
                    .h(px(24.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(active, move |style| {
                        style
                            .bg(rgb(theme.editor_bg))
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
                    .on_click(
                        cx.listener(move |this, _event, _window, cx| this.select_tab(tab, cx)),
                    )
                    .child(t!(tab.label_key()).to_string()),
            );
        }
        bar
    }

    fn render_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        match self.tab {
            PmTab::General => div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h(px(0.0))
                .overflow_hidden()
                .child(self.render_tree(cx))
                .child(self.render_matrix(cx))
                .into_any_element(),
            PmTab::Sql => {
                let sql = self.preview_sql();
                let text = if sql.trim().is_empty() {
                    t!("design.no_changes").to_string()
                } else {
                    sql
                };
                div()
                    .id("pm-sql-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.sql_scroll)
                    .p_2()
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(self.theme.text))
                            .child(text),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_tree(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut list = div().flex().flex_col();
        if self.loading {
            list = list.child(tree_message(
                t!("common.loading").to_string(),
                8.0,
                theme.text_muted,
            ));
        }
        if let Some(error) = self.error.as_ref() {
            list = list.child(tree_message(error.clone(), 8.0, theme.danger));
        }
        for (index, database) in self.databases.iter().enumerate() {
            let expanded = self.expanded.as_deref() == Some(database.as_str());
            let selected = self
                .selected
                .as_ref()
                .is_some_and(|(db, name)| db == database && name.is_empty());
            let db = database.clone();
            let db_select = database.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("pm-db-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(PM_ROW_HEIGHT))
                    .cursor_pointer()
                    .when(selected, move |style| style.bg(rgb(theme.tree_selected_bg)))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .child(
                        div()
                            .id(SharedString::from(format!("pm-db-toggle-{index}")))
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(16.0))
                            .h_full()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.toggle_database(db.clone(), cx)
                            }))
                            .child(tree_chevron(expanded, theme.chevron)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("pm-db-select-{index}")))
                            .flex_1()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.select_node((db_select.clone(), String::new()), cx)
                            }))
                            .child(tree_icon("icons/database.svg", theme.icon_database_active))
                            .child(database.clone()),
                    ),
            );
            if expanded {
                let tables = self.tables.get(database).cloned().unwrap_or_default();
                for (table_index, table) in tables.iter().enumerate() {
                    let selected = self
                        .selected
                        .as_ref()
                        .is_some_and(|(db, name)| db == database && name == table);
                    let node = (database.clone(), table.clone());
                    list = list.child(
                        div()
                            .id(SharedString::from(format!(
                                "pm-table-{index}-{table_index}"
                            )))
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(PM_ROW_HEIGHT))
                            .pl(px(28.0))
                            .cursor_pointer()
                            .when(selected, move |style| style.bg(rgb(theme.tree_selected_bg)))
                            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.select_node(node.clone(), cx)
                            }))
                            .child(tree_icon("icons/tables.svg", theme.icon_tables))
                            .child(table.clone()),
                    );
                }
            }
        }

        div()
            .id("pm-tree")
            .flex()
            .flex_col()
            .w(px(240.0))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(rgb(theme.border))
            .child(list)
            .into_any_element()
    }

    fn render_matrix(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let name_width = 200.0;
        let priv_width = 74.0;
        let content_width = name_width + priv_width * Privilege::OBJECT.len() as f32;

        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(24.0))
            .flex_none()
            .bg(rgb(theme.header_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .text_size(px(11.0))
            .child(
                div()
                    .w(px(name_width))
                    .flex_none()
                    .px_2()
                    .child(t!("user.privilege.account").to_string()),
            );
        for privilege in Privilege::OBJECT {
            header = header.child(
                div()
                    .w(px(priv_width))
                    .flex_none()
                    .px_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(t!(privilege.label_key()).to_string()),
            );
        }

        let rows = self.rows.clone();
        let mut body = div().flex().flex_col();
        if rows.is_empty() {
            body = body.child(tree_message(
                t!("user.privilege.empty").to_string(),
                8.0,
                theme.text_muted,
            ));
        }
        for (row, entry) in rows.iter().enumerate() {
            let selected = self.selected_rows.contains(&row);
            let mut line = div()
                .id(SharedString::from(format!("pm-row-{row}")))
                .flex()
                .flex_row()
                .items_center()
                .h(px(PM_ROW_HEIGHT))
                .when(row % 2 == 1, move |style| style.bg(rgb(theme.row_alt_bg)))
                .when(selected, move |style| style.bg(rgb(theme.tree_selected_bg)))
                .child(
                    div()
                        .id(SharedString::from(format!("pm-row-name-{row}")))
                        .w(px(name_width))
                        .flex_none()
                        .px_2()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .when(selected, move |style| {
                            style.text_color(rgb(theme.tree_selected_text))
                        })
                        .on_click(
                            cx.listener(move |this, _event, _window, cx| this.toggle_row(row, cx)),
                        )
                        .child(entry.label()),
                );
            for privilege in Privilege::OBJECT {
                let checked = entry.privileges.contains(&privilege);
                line = line.child(
                    div()
                        .id(SharedString::from(format!(
                            "pm-cell-{row}-{}",
                            privilege.sql_name()
                        )))
                        .w(px(priv_width))
                        .flex_none()
                        .flex()
                        .justify_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.toggle_cell(row, privilege, cx)
                        }))
                        .child(checkbox_box(checked, theme)),
                );
            }
            body = body.child(line);
        }

        div()
            .id("pm-matrix-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_x_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(content_width))
                    .child(header)
                    .child(
                        div()
                            .id("pm-matrix-rows")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .child(body),
                    ),
            )
            .into_any_element()
    }
}
