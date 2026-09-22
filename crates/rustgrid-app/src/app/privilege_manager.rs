//! 对象权限 (Object Privileges): the OS window for granting privileges on one database or table to
//! accounts.
//!
//! It mirrors the account editor's 数据库权限 page, transposed. The object list is on the left and,
//! on the right, the quick presets, the accounts that hold a grant on the object (every account is
//! listed, so a new one is granted by ticking it) and the fine-grained privileges of the selected
//! account. The account-centric view lives in the account editor; this is the object-centric one.

use std::collections::{BTreeMap, BTreeSet};

use super::user_create::{DbTemplate, db_privilege_groups, privilege_summary, section};
use super::*;

/// Height of one account row in the account list.
const PM_ROW_HEIGHT: f32 = 26.0;
/// Width of the left object list.
const PM_OBJECT_WIDTH: f32 = 240.0;
/// Max height of the account list before it scrolls.
const PM_LIST_MAX_HEIGHT: f32 = 170.0;

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
            PmTab::General => "op-tab-general",
            PmTab::Sql => "op-tab-sql",
        }
    }
}

pub(super) struct PrivilegeManager {
    runtime: Arc<Runtime>,
    pub(super) theme: Theme,
    connection: Arc<dyn Connection>,
    pub(super) connection_name: String,
    /// The window root's focus, so ESC closes the window.
    focus: FocusHandle,

    databases: Vec<String>,
    tables: BTreeMap<String, Vec<String>>,
    /// The expanded database in the object list.
    expanded: Option<String>,
    /// The selected object: `(database, name)`; `name` is empty for a database-wide grant.
    selected: Option<(String, String)>,
    /// The accounts to grant on `selected`, with their pending privileges.
    rows: Vec<ObjectPrivilegeRow>,
    /// The grants as loaded, for the SQL preview diff.
    original: Vec<ObjectPrivilegeRow>,
    /// Every account on the server, for the account list.
    accounts: Vec<UserAccount>,
    /// The account whose privileges the 细粒度特权分配 panel edits.
    active: Option<(String, String)>,
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
        Self {
            runtime,
            theme,
            connection,
            connection_name,
            focus: cx.focus_handle(),
            databases: Vec::new(),
            tables: BTreeMap::new(),
            expanded: None,
            selected: None,
            rows: Vec::new(),
            original: Vec::new(),
            accounts: Vec::new(),
            active: None,
            tab: PmTab::General,
            loading: false,
            saving: false,
            dirty: false,
            error: None,
            sql_scroll: ScrollHandle::new(),
        }
    }

    /// The window root's focus handle, focused when the window opens.
    pub(super) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Load the database list for the object tree and the account list for the account panel.
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

    /// Select an object and load the grants on it. The active account becomes the first grantee (if
    /// any), so the 细粒度特权分配 panel always has a target.
    fn select_node(&mut self, node: (String, String), cx: &mut Context<'_, Self>) {
        self.selected = Some(node.clone());
        self.rows.clear();
        self.original.clear();
        self.active = None;
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
                        manager.active = manager
                            .rows
                            .first()
                            .map(|row| (row.user.clone(), row.host.clone()));
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

    fn row_index(&self, user: &str, host: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.user == user && row.host == host)
    }

    /// Whether the account currently has a (pending) grant on the selected object.
    fn account_granted(&self, user: &str, host: &str) -> bool {
        self.row_index(user, host).is_some()
    }

    /// Tick an account into the grant set, or untick it. Unticking the active account moves the
    /// active selection to the first remaining grantee.
    fn toggle_account(&mut self, user: String, host: String, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.row_index(&user, &host) {
            self.rows.remove(index);
            if self.active.as_ref() == Some(&(user.clone(), host.clone())) {
                self.active = self
                    .rows
                    .first()
                    .map(|row| (row.user.clone(), row.host.clone()));
            }
        } else {
            self.rows
                .push(ObjectPrivilegeRow::new(user.clone(), host.clone()));
            self.active = Some((user, host));
        }
        self.dirty = true;
        cx.notify();
    }

    /// Make one account the active target of the 细粒度特权分配 panel, granting it if needed.
    fn activate_account(&mut self, user: String, host: String, cx: &mut Context<'_, Self>) {
        if self.row_index(&user, &host).is_none() {
            self.rows
                .push(ObjectPrivilegeRow::new(user.clone(), host.clone()));
            self.dirty = true;
        }
        self.active = Some((user, host));
        cx.notify();
    }

    /// The privilege set the 细粒度特权分配 panel edits.
    fn active_privileges(&self) -> BTreeSet<Privilege> {
        let Some((user, host)) = self.active.as_ref() else {
            return BTreeSet::new();
        };
        self.row_index(user, host)
            .map(|index| self.rows[index].privileges.clone())
            .unwrap_or_default()
    }

    fn active_privileges_mut(&mut self) -> Option<&mut BTreeSet<Privilege>> {
        let (user, host) = self.active.clone()?;
        self.rows
            .iter_mut()
            .find(|row| row.user == user && row.host == host)
            .map(|row| &mut row.privileges)
    }

    /// Apply a quick preset to the active account.
    fn apply_template(&mut self, template: DbTemplate, cx: &mut Context<'_, Self>) {
        let privileges = template.privileges();
        if let Some(target) = self.active_privileges_mut() {
            *target = privileges;
            self.dirty = true;
        }
        cx.notify();
    }

    fn toggle_privilege(&mut self, privilege: Privilege, cx: &mut Context<'_, Self>) {
        if let Some(target) = self.active_privileges_mut()
            && !target.remove(&privilege)
        {
            target.insert(privilege);
        }
        self.dirty = true;
        cx.notify();
    }

    fn toggle_all_privileges(&mut self, cx: &mut Context<'_, Self>) {
        let all: BTreeSet<Privilege> = Privilege::OBJECT.into_iter().collect();
        if let Some(target) = self.active_privileges_mut() {
            if *target == all {
                target.clear();
            } else {
                *target = all;
            }
            self.dirty = true;
        }
        cx.notify();
    }

    /// Tick every account into the grant set, or untick them all.
    fn set_all_accounts(&mut self, all: bool, cx: &mut Context<'_, Self>) {
        if all {
            for account in &self.accounts {
                if self.row_index(&account.user, &account.host).is_none() {
                    self.rows.push(ObjectPrivilegeRow::new(
                        account.user.clone(),
                        account.host.clone(),
                    ));
                }
            }
            if self.active.is_none() {
                self.active = self
                    .rows
                    .first()
                    .map(|row| (row.user.clone(), row.host.clone()));
            }
        } else {
            self.rows.clear();
            self.active = None;
        }
        self.dirty = true;
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
        let title = format!(
            "{} - {}",
            self.connection_name,
            t!("user.privilege_manager")
        );
        div()
            .id("object-privileges")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.editor_bg))
            .text_color(rgb(theme.text))
            .text_size(px(12.5))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_this, event: &KeyDownEvent, window, _cx| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            }))
            .child(export::child_window_titlebar(title, theme))
            .child(self.render_toolbar(cx))
            .child(self.render_subtabs(cx))
            .child(self.render_body(cx))
    }
}

impl PrivilegeManager {
    fn render_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
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
            .child(ui::toolbar_item(
                "op-save",
                "icons/save.svg",
                t!("design.save").to_string(),
                self.selected.is_some() && !self.saving,
                theme,
                cx.listener(|this, _event, _window, cx| this.save(cx)),
            ))
            .child(self.render_status())
    }

    /// The toolbar's right cell: the error, or a brief hint reporting what is selected.
    fn render_status(&self) -> AnyElement {
        let theme = self.theme;
        if let Some(error) = self.error.as_ref() {
            return div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(11.5))
                .text_color(rgb(theme.danger))
                .child(error.clone())
                .into_any_element();
        }
        let text = if self.saving {
            Some(t!("user.create.saving").to_string())
        } else if self.dirty {
            Some(format!("● {}", t!("user.privilege.unsaved")))
        } else {
            None
        };
        div()
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(11.5))
            .text_color(rgb(theme.text_muted))
            .child(text.unwrap_or_default())
            .into_any_element()
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
                .child(self.render_object_list(cx))
                .child(self.render_detail(cx))
                .into_any_element(),
            PmTab::Sql => {
                let sql = self.preview_sql();
                let text = if sql.trim().is_empty() {
                    t!("design.no_changes").to_string()
                } else {
                    sql
                };
                div()
                    .id("op-sql-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.sql_scroll)
                    .p_2()
                    .child(
                        div()
                            .font_family("Consolas")
                            .text_size(px(12.5))
                            .text_color(rgb(self.theme.text))
                            .child(text),
                    )
                    .into_any_element()
            }
        }
    }

    /// The left object list: every database, expandable into its tables.
    fn render_object_list(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut list = div().flex().flex_col().p_2().gap_1();
        if self.loading && self.databases.is_empty() {
            list = list.child(tree_message(
                t!("common.loading").to_string(),
                8.0,
                theme.text_muted,
            ));
        }
        for (index, database) in self.databases.iter().enumerate() {
            let expanded = self.expanded.as_deref() == Some(database.as_str());
            let selected = self
                .selected
                .as_ref()
                .is_some_and(|(db, name)| db == database && name.is_empty());
            let toggle_db = database.clone();
            let select_db = database.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("op-db-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(28.0))
                    .px_2()
                    .flex_none()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .when(selected, move |style| {
                        style
                            .bg(rgb(theme.tree_selected_bg))
                            .text_color(rgb(theme.tree_selected_text))
                    })
                    .when(!selected, move |style| {
                        style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.select_node((select_db.clone(), String::new()), cx)
                    }))
                    .child(
                        div()
                            .id(SharedString::from(format!("op-db-toggle-{index}")))
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(16.0))
                            .h_full()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                cx.stop_propagation();
                                this.toggle_database(toggle_db.clone(), cx);
                            }))
                            .child(tree_chevron(expanded, theme.chevron)),
                    )
                    .child(tree_icon("icons/database.svg", theme.icon_database_active))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(database.clone()),
                    ),
            );
            if expanded {
                for (table_index, table) in self
                    .tables
                    .get(database)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    let selected = self
                        .selected
                        .as_ref()
                        .is_some_and(|(db, name)| db == database && name == table);
                    let node = (database.clone(), table.clone());
                    list = list.child(
                        div()
                            .id(SharedString::from(format!(
                                "op-table-{index}-{table_index}"
                            )))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .h(px(28.0))
                            .pl(px(30.0))
                            .pr_2()
                            .flex_none()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .when(selected, move |style| {
                                style
                                    .bg(rgb(theme.tree_selected_bg))
                                    .text_color(rgb(theme.tree_selected_text))
                            })
                            .when(!selected, move |style| {
                                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                            })
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.select_node(node.clone(), cx)
                            }))
                            .child(tree_icon("icons/table.svg", theme.icon_table))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(table.clone()),
                            ),
                    );
                }
            }
        }
        div()
            .id("op-objects")
            .flex()
            .flex_col()
            .w(px(PM_OBJECT_WIDTH))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(rgb(theme.sidebar_bg))
            .border_r_1()
            .border_color(rgb(theme.border))
            .child(list)
            .into_any_element()
    }

    /// The right detail pane for the selected object.
    fn render_detail(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some((database, name)) = self.selected.clone() else {
            return tree_message(
                t!("user.privilege.select_object").to_string(),
                8.0,
                theme.text_muted,
            )
            .into_any_element();
        };
        let is_table = !name.is_empty();
        let active = self.active.clone();
        let privileges = self.active_privileges();
        let granted = self.rows.len();

        let (icon, kind) = if is_table {
            (
                tree_icon("icons/table.svg", theme.icon_table),
                t!("user.create.table_level").to_string(),
            )
        } else {
            (
                tree_icon("icons/database.svg", theme.icon_database_active),
                t!("user.create.database_level").to_string(),
            )
        };
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .child(icon)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if is_table {
                                name.clone()
                            } else {
                                database.clone()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(kind),
                    ),
            );
        let target = div()
            .text_size(px(11.0))
            .text_color(rgb(theme.text_muted))
            .child(format!(
                "{} {}",
                t!("user.create.grant_applies_to"),
                object_spec(&database, &name)
            ));

        // The quick presets, matched against the active account's own set.
        let mut presets = div().flex().flex_row().items_center().gap_2().w_full();
        for template in DbTemplate::ALL {
            let active_preset = if template == DbTemplate::None {
                privileges.is_empty()
            } else {
                !privileges.is_empty() && privileges == template.privileges()
            };
            presets = presets.child(
                div()
                    .id(SharedString::from(format!(
                        "op-template-{}",
                        template.label_key()
                    )))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(28.0))
                    .px_3()
                    .rounded(px(4.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(active_preset, move |style| {
                        style
                            .bg(rgb(theme.primary))
                            .text_color(rgb(if theme.is_dark() {
                                theme.window_bg
                            } else {
                                0xffffff
                            }))
                    })
                    .when(!active_preset, move |style| {
                        style
                            .bg(rgb(theme.button_bg))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .text_color(rgb(theme.text_muted))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.apply_template(template, cx)
                    }))
                    .child(t!(template.label_key()).to_string()),
            );
        }
        let fine_grained_hint = active.as_ref().map(|(user, host)| {
            t!(
                "user.privilege.accounts_for",
                account = format!("{user}@{host}")
            )
            .to_string()
        });

        let mut column = div()
            .id("op-detail")
            .flex()
            .flex_col()
            .gap_4()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .h_full()
            .overflow_y_scroll()
            .p_4()
            .child(header)
            .child(target)
            .child(section(
                t!("user.create.db_template").to_string(),
                Some(t!("user.create.db_template_hint").to_string()),
                presets.into_any_element(),
                theme,
            ))
            .child(section(
                t!("user.privilege.accounts").to_string(),
                Some(t!("user.privilege.accounts_selected", count = granted).to_string()),
                self.render_accounts(&active, cx),
                theme,
            ));
        if active.is_some() {
            column = column.child(section(
                t!("user.create.fine_grained").to_string(),
                fine_grained_hint,
                self.render_fine_grained(&privileges, cx),
                theme,
            ));
        } else {
            column = column.child(section(
                t!("user.create.fine_grained").to_string(),
                None,
                tree_message(
                    t!("user.privilege.select_account_first").to_string(),
                    8.0,
                    theme.text_muted,
                )
                .into_any_element(),
                theme,
            ));
        }
        column
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("user.create.batch_hint").to_string())
                    .child(
                        t!("user.create.selected_privileges", count = privileges.len()).to_string(),
                    ),
            )
            .into_any_element()
    }

    /// The account list of the selected object: every account, ticking one grants it.
    fn render_accounts(
        &self,
        active: &Option<(String, String)>,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let mut list = div()
            .id("op-accounts")
            .flex()
            .flex_col()
            .max_h(px(PM_LIST_MAX_HEIGHT))
            .overflow_y_scroll()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg));
        for (index, account) in self.accounts.iter().enumerate() {
            let key = (account.user.clone(), account.host.clone());
            let checked = self.account_granted(&key.0, &key.1);
            let is_active = active.as_ref() == Some(&key);
            let (badge, authorized) = self
                .row_index(&key.0, &key.1)
                .map(|row| privilege_summary(&self.rows[row].privileges))
                .unwrap_or_else(|| (t!("user.create.unauthorized").to_string(), false));
            let activate_key = key.clone();
            let toggle_key = key.clone();
            let mut entry = div()
                .id(SharedString::from(format!("op-account-{index}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(PM_ROW_HEIGHT))
                .px_2()
                .flex_none()
                .rounded(px(4.0))
                .cursor_pointer()
                .when(is_active, move |style| {
                    style.bg(rgb(theme.tree_selected_bg))
                })
                .when(!is_active, move |style| {
                    style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                })
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.activate_account(activate_key.0.clone(), activate_key.1.clone(), cx)
                }))
                .child(
                    div()
                        .id(SharedString::from(format!("op-account-check-{index}")))
                        .flex()
                        .items_center()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_account(toggle_key.0.clone(), toggle_key.1.clone(), cx);
                        }))
                        .child(checkbox_box(checked, theme)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.0))
                        .text_color(rgb(if is_active {
                            theme.tree_selected_text
                        } else {
                            theme.text
                        }))
                        .child(account.label()),
                );
            if authorized {
                entry = entry.child(
                    div()
                        .flex_none()
                        .px_2()
                        .py_0p5()
                        .rounded(px(9.0))
                        .text_size(px(10.5))
                        .bg(rgb(theme.tree_hover_bg))
                        .text_color(rgb(theme.primary))
                        .child(badge),
                );
            } else {
                entry = entry.child(
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(rgb(theme.text_muted))
                        .child(badge),
                );
            }
            list = list.child(entry);
        }
        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("op-accounts-all")
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.primary))
                    .hover(move |style| style.text_color(rgb(theme.text)))
                    .on_click(
                        cx.listener(|this, _event, _window, cx| this.set_all_accounts(true, cx)),
                    )
                    .child(t!("user.create.select_all").to_string()),
            )
            .child(
                div()
                    .id("op-accounts-none")
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.primary))
                    .hover(move |style| style.text_color(rgb(theme.text)))
                    .on_click(
                        cx.listener(|this, _event, _window, cx| this.set_all_accounts(false, cx)),
                    )
                    .child(t!("user.create.clear_all").to_string()),
            );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .child(list)
            .child(actions)
            .into_any_element()
    }

    /// The grouped fine-grained privileges of the active account.
    fn render_fine_grained(
        &self,
        privileges: &BTreeSet<Privilege>,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let mut groups = div().flex().flex_col().gap_3().w_full();
        for (group_key, group_privileges) in db_privilege_groups() {
            let mut grid = div().flex().flex_row().flex_wrap().w_full();
            for (position, privilege) in group_privileges.into_iter().enumerate() {
                let checked = privileges.contains(&privilege);
                grid = grid.child(
                    div()
                        .id(SharedString::from(format!(
                            "op-priv-{group_key}-{position}"
                        )))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .w(px(210.0))
                        .h(px(PM_ROW_HEIGHT))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.toggle_privilege(privilege, cx)
                        }))
                        .child(checkbox_box(checked, theme))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(theme.text))
                                .child(t!(privilege.label_key()).to_string()),
                        ),
                );
            }
            groups = groups.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .w_full()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!(group_key).to_string()),
                    )
                    .child(grid),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(
                div().flex().flex_row().justify_end().w_full().child(
                    div()
                        .id("op-priv-toggle-all")
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .text_color(rgb(theme.primary))
                        .hover(move |style| style.text_color(rgb(theme.text)))
                        .on_click(
                            cx.listener(|this, _event, _window, cx| this.toggle_all_privileges(cx)),
                        )
                        .child(t!("user.create.toggle_all").to_string()),
                ),
            )
            .child(groups)
            .into_any_element()
    }
}

/// The `db`.*` / `db`.`table` spec an object's grant applies to.
fn object_spec(database: &str, name: &str) -> String {
    if name.is_empty() {
        format!("`{database}`.*")
    } else {
        format!("`{database}`.`{name}`")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_spec_covers_databases_and_tables() {
        assert_eq!(object_spec("shop", ""), "`shop`.*");
        assert_eq!(object_spec("shop", "orders"), "`shop`.`orders`");
    }
}
