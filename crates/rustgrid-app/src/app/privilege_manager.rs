//! 对象权限 (Object Privileges): the OS window for granting privileges on one database or table to
//! accounts.
//!
//! It mirrors the account editor's 数据库权限 page, transposed. The object list is on the left and,
//! on the right, the quick presets, the accounts that hold a grant on the object (every account is
//! listed, so a new one is granted by ticking it) and the fine-grained privileges of the selected
//! account. The account-centric view lives in the account editor; this is the object-centric one.

use std::collections::{BTreeMap, BTreeSet};

use super::user_create::{
    PrivilegeTooltip, catalog_label, preset_privileges, privilege_label, privilege_summary, section,
};
use super::*;

/// Height of one account row in the account list.
const PM_ROW_HEIGHT: f32 = 22.0;
/// Height of one object row (database/table) in the left list.
const PM_OBJECT_ROW_HEIGHT: f32 = 22.0;
/// Width of the left object list.
const PM_OBJECT_WIDTH: f32 = 240.0;
/// Left padding of one table row. The database row is `px_2` (8px) + a 16px chevron + a 4px gap
/// before its icon, i.e. the icon starts at 28px; a table adds one 18px indent step on top so it
/// nests under its database instead of lining up with it.
const PM_TABLE_INDENT: f32 = 46.0;
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
    /// The engine's privilege catalog (groups, privileges and presets).
    catalog: PrivilegeCatalog,
    /// The window root's focus, so ESC closes the window.
    focus: FocusHandle,

    databases: Vec<String>,
    /// Tables per database, keyed by their `schema.name` (or bare name) — the table grants.
    tables: BTreeMap<String, Vec<String>>,
    /// Schemas per database — the schema grants (loaded only for engines that support them).
    schemas: BTreeMap<String, Vec<String>>,
    /// The expanded database in the object list.
    expanded: Option<String>,
    /// The selected object: `(database, name)` where `name` is empty for a database-wide grant,
    /// `schema.*` for a schema grant, else the table/routine name.
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
    /// Whether the change-preview popup is open (Save shows it before writing anything).
    confirm_open: bool,
    /// Whether the "discard unsaved changes?" prompt is open.
    discard_open: bool,
}

impl PrivilegeManager {
    pub(super) fn new(
        connection: Arc<dyn Connection>,
        connection_name: String,
        catalog: PrivilegeCatalog,
        runtime: Arc<Runtime>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self {
            runtime,
            theme,
            connection,
            connection_name,
            catalog,
            focus: cx.focus_handle(),
            databases: Vec::new(),
            tables: BTreeMap::new(),
            schemas: BTreeMap::new(),
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
            confirm_open: false,
            discard_open: false,
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
        let load_schemas = self.catalog.has_scope(PrivilegeScope::Schema);
        let query = database.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move {
                    let tables = connection.list_tables(&query).await;
                    let schemas = if load_schemas {
                        connection.list_schemas(&query).await
                    } else {
                        Ok(Vec::new())
                    };
                    (tables, schemas)
                })
                .await;
            let _ = this.update(cx, |manager, cx| {
                match result {
                    Ok((tables, schemas)) => {
                        match tables {
                            Ok(tables) => {
                                manager.tables.insert(
                                    database.clone(),
                                    tables.into_iter().map(|table| table.name).collect(),
                                );
                            }
                            Err(error) => manager.error = Some(error.to_string()),
                        }
                        match schemas {
                            Ok(schemas) => {
                                manager.schemas.insert(database, schemas);
                            }
                            Err(error) => manager.error = Some(error.to_string()),
                        }
                    }
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
    fn active_privileges(&self) -> BTreeSet<PrivilegeId> {
        let Some((user, host)) = self.active.as_ref() else {
            return BTreeSet::new();
        };
        self.row_index(user, host)
            .map(|index| self.rows[index].privileges.clone())
            .unwrap_or_default()
    }

    fn active_privileges_mut(&mut self) -> Option<&mut BTreeSet<PrivilegeId>> {
        let (user, host) = self.active.clone()?;
        self.rows
            .iter_mut()
            .find(|row| row.user == user && row.host == host)
            .map(|row| &mut row.privileges)
    }

    /// The denied (DENY) privilege set of the active account.
    fn active_denied(&self) -> BTreeSet<PrivilegeId> {
        let Some((user, host)) = self.active.as_ref() else {
            return BTreeSet::new();
        };
        self.row_index(user, host)
            .map(|index| self.rows[index].denied.clone())
            .unwrap_or_default()
    }

    fn active_denied_mut(&mut self) -> Option<&mut BTreeSet<PrivilegeId>> {
        let (user, host) = self.active.clone()?;
        self.rows
            .iter_mut()
            .find(|row| row.user == user && row.host == host)
            .map(|row| &mut row.denied)
    }

    /// Apply a quick preset (index into `catalog.object_presets`) to the active account.
    fn apply_template(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let privileges = preset_privileges(&self.catalog.object_presets, index);
        if let Some(target) = self.active_privileges_mut() {
            *target = privileges;
            self.dirty = true;
        }
        if let Some(denied) = self.active_denied_mut() {
            denied.clear();
        }
        cx.notify();
    }

    /// Toggle a grant. Granting clears any deny of the same privilege.
    fn toggle_privilege(&mut self, privilege: PrivilegeId, cx: &mut Context<'_, Self>) {
        let granted_now = match self.active_privileges_mut() {
            Some(target) => {
                if target.remove(&privilege) {
                    false
                } else {
                    target.insert(privilege.clone());
                    true
                }
            }
            None => false,
        };
        if granted_now && let Some(denied) = self.active_denied_mut() {
            denied.remove(&privilege);
        }
        self.dirty = true;
        cx.notify();
    }

    /// Toggle a deny. Denying clears any grant of the same privilege.
    fn toggle_privilege_deny(&mut self, privilege: PrivilegeId, cx: &mut Context<'_, Self>) {
        let denied_now = match self.active_denied_mut() {
            Some(target) => {
                if target.remove(&privilege) {
                    false
                } else {
                    target.insert(privilege.clone());
                    true
                }
            }
            None => false,
        };
        if denied_now && let Some(granted) = self.active_privileges_mut() {
            granted.remove(&privilege);
        }
        self.dirty = true;
        cx.notify();
    }

    /// The scope the 细粒度特权分配 grid edits: whole-database permissions for a selected
    /// database when the engine has them (SQL Server), otherwise object privileges.
    fn active_scope(&self) -> PrivilegeScope {
        let candidate = match self.selected.as_ref().map(|(_, name)| name.as_str()) {
            Some("") => PrivilegeScope::Database,
            Some(name) if name.ends_with(".*") => PrivilegeScope::Schema,
            _ => PrivilegeScope::Object,
        };
        if self.catalog.has_scope(candidate) {
            candidate
        } else {
            PrivilegeScope::Object
        }
    }

    fn toggle_all_privileges(&mut self, cx: &mut Context<'_, Self>) {
        let scope = self.active_scope();
        let all: BTreeSet<PrivilegeId> =
            self.catalog.at(scope).map(|info| info.id.clone()).collect();
        if let Some(target) = self.active_privileges_mut() {
            if *target == all {
                target.clear();
            } else {
                *target = all;
            }
            self.dirty = true;
        }
        if let Some(denied) = self.active_denied_mut() {
            denied.clear();
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

    /// The annotated script the change-preview popup shows: a header naming the object, then the
    /// statements that [`Self::execute_save`] would run. Mirrors the account editor's change diff.
    fn confirm_preview(&self) -> String {
        let Some((database, name)) = self.selected.as_ref() else {
            return String::new();
        };
        let mut out = String::new();
        out.push_str(&format!(
            "-- {}\n",
            t!(
                "user.privilege.preview_header",
                object = object_spec(database, name)
            )
        ));
        let sql = self.preview_sql();
        if sql.trim().is_empty() {
            out.push_str(&format!("-- {}\n", t!("design.no_changes")));
        } else {
            out.push('\n');
            out.push_str(&sql);
            out.push('\n');
        }
        out
    }

    fn copy_sql(&mut self, cx: &mut Context<'_, Self>) {
        let sql = self.preview_sql();
        if !sql.trim().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(sql));
        }
    }

    /// Open the change-preview popup; the actual write happens in [`Self::execute_save`].
    fn save(&mut self, cx: &mut Context<'_, Self>) {
        if self.selected.is_none() || self.saving {
            return;
        }
        self.confirm_open = true;
        self.error = None;
        cx.notify();
    }

    /// Close the change-preview popup without saving.
    fn close_confirm(&mut self, cx: &mut Context<'_, Self>) {
        self.confirm_open = false;
        cx.notify();
    }

    fn execute_save(&mut self, cx: &mut Context<'_, Self>) {
        let Some((database, name)) = self.selected.clone() else {
            return;
        };
        if self.saving {
            return;
        }
        self.saving = true;
        self.confirm_open = false;
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
        let mut root = div()
            .id("object-privileges")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.editor_bg))
            .text_color(rgb(theme.text))
            .text_size(px(12.5))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key != "escape" || this.saving {
                    return;
                }
                // ESC dismisses the open change preview first, then closes the window.
                if this.confirm_open {
                    this.close_confirm(cx);
                } else if this.discard_open {
                    this.discard_open = false;
                    cx.notify();
                } else if this.dirty {
                    this.discard_open = true;
                    cx.notify();
                } else {
                    window.remove_window();
                }
            }))
            .child(export::child_window_titlebar(title, theme))
            .child(self.render_subtabs(cx))
            .child(self.render_body(cx))
            .child(self.render_footer(cx));
        if self.confirm_open {
            root = root.child(self.render_confirm(cx));
        }
        if self.discard_open {
            root = root.child(self.render_discard(cx));
        }
        root
    }
}

impl PrivilegeManager {
    /// The window's bottom bar: the error / 未保存 state on the left, the Save button on the right.
    /// Save opens the change-preview popup instead of writing straight away.
    fn render_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let enabled = self.selected.is_some() && !self.saving;
        let label = if self.saving {
            t!("user.create.saving").to_string()
        } else {
            t!("design.save").to_string()
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .w_full()
            .flex_none()
            .px_4()
            .py_2()
            .bg(rgb(theme.dialog_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(self.render_status())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_none()
                    .child(ui::button(
                        "op-save",
                        label,
                        if enabled {
                            ButtonKind::Default
                        } else {
                            ButtonKind::Disabled
                        },
                        theme,
                        cx.listener(|this, _event, _window, cx| {
                            if !this.saving {
                                this.save(cx);
                            }
                        }),
                    )),
            )
            .into_any_element()
    }

    /// The change-preview popup: the annotated script plus 关闭 / 确认并执行.
    fn render_confirm(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let preview = self.confirm_preview();

        let mut code = div()
            .id("op-confirm-sql")
            .flex()
            .flex_col()
            .max_h(px(360.0))
            .overflow_y_scroll()
            .rounded(px(6.0))
            .bg(rgb(0x0d1117))
            .p_3();
        for line in preview.lines() {
            let comment = line.trim_start().starts_with("--");
            code = code.child(
                div()
                    .font_family("Consolas")
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(if comment { 0x8b949e } else { 0xc9d1d9 }))
                    .child(line.to_string()),
            );
        }

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("user.create.confirm_title").to_string()),
            )
            .child(
                div()
                    .id("op-confirm-copy")
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.primary))
                    .hover(move |style| style.text_color(rgb(theme.text)))
                    .on_click(cx.listener(|this, _event, _window, cx| this.copy_sql(cx)))
                    .child(t!("user.create.copy_sql").to_string()),
            );

        let panel = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .w(px(720.0))
            .max_h(px(560.0))
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .rounded(px(8.0))
            .shadow(ui::dialog_shadow())
            .child(header)
            .child(code)
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("user.create.confirm_hint").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .w_full()
                    .child(ui::dialog_button(
                        "op-confirm-close",
                        t!("user.create.confirm_close").to_string(),
                        false,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.close_confirm(cx)),
                    ))
                    .child(ui::dialog_button(
                        "op-confirm-execute",
                        t!("user.create.confirm_execute").to_string(),
                        true,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.execute_save(cx)),
                    )),
            )
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            });

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .bg(rgba(0x00000040))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.close_confirm(cx)),
            )
            .child(panel)
            .into_any_element()
    }

    /// The "discard unsaved changes?" prompt shown when Esc closes a dirty manager window.
    fn render_discard(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let panel = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .w(px(420.0))
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .rounded(px(8.0))
            .shadow(ui::dialog_shadow())
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(t!("unsaved.title").to_string()),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("unsaved.message").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .w_full()
                    .child(ui::dialog_button(
                        "op-discard-cancel",
                        t!("form.cancel").to_string(),
                        false,
                        theme,
                        cx.listener(|this, _event, _window, cx| {
                            this.discard_open = false;
                            cx.notify();
                        }),
                    ))
                    .child(ui::dialog_button(
                        "op-discard-ok",
                        t!("unsaved.discard").to_string(),
                        true,
                        theme,
                        cx.listener(|this, _event, window, _cx| {
                            this.discard_open = false;
                            window.remove_window();
                        }),
                    )),
            )
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            });

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .bg(rgba(0x00000040))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.discard_open = false;
                    cx.notify();
                }),
            )
            .child(panel)
            .into_any_element()
    }

    /// The footer's left cell: the error, or a brief hint reporting what is selected.
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
        let mut list = div().flex().flex_col().py_1().px_1().gap_0p5();
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
                    .h(px(PM_OBJECT_ROW_HEIGHT))
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
                // Schema grants (PostgreSQL): the schema is selected as `schema.*`.
                if self.catalog.has_scope(PrivilegeScope::Schema) {
                    for (schema_index, schema) in self
                        .schemas
                        .get(database)
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .enumerate()
                    {
                        let key = format!("{schema}.*");
                        let selected = self
                            .selected
                            .as_ref()
                            .is_some_and(|(db, name)| db == database && name == &key);
                        let node = (database.clone(), key);
                        list = list.child(
                            div()
                                .id(SharedString::from(format!(
                                    "op-schema-{index}-{schema_index}"
                                )))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .h(px(PM_OBJECT_ROW_HEIGHT))
                                .pl(px(PM_TABLE_INDENT))
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
                                .child(tree_icon("icons/database.svg", theme.icon_database_active))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(schema.clone()),
                                ),
                        );
                    }
                }
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
                            .h(px(PM_OBJECT_ROW_HEIGHT))
                            .pl(px(PM_TABLE_INDENT))
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
                            .child(tree_icon("icons/tables.svg", theme.icon_table))
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
        let is_schema = name.ends_with(".*");
        let is_table = !name.is_empty() && !is_schema;
        let active = self.active.clone();
        let privileges = self.active_privileges();
        let granted = self.rows.len();

        let (icon, kind, title) = if is_schema {
            (
                tree_icon("icons/database.svg", theme.icon_database_active),
                t!("user.create.schema_level").to_string(),
                name.trim_end_matches(".*").to_string(),
            )
        } else if is_table {
            (
                tree_icon("icons/tables.svg", theme.icon_table),
                t!("user.create.table_level").to_string(),
                name.clone(),
            )
        } else {
            (
                tree_icon("icons/database.svg", theme.icon_database_active),
                t!("user.create.database_level").to_string(),
                database.clone(),
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
                            .child(title),
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
        for (index, preset) in self.catalog.object_presets.iter().enumerate() {
            let preset_set = preset_privileges(&self.catalog.object_presets, index);
            let active_preset = if preset_set.is_empty() {
                privileges.is_empty()
            } else {
                !privileges.is_empty() && privileges == preset_set
            };
            let label = catalog_label(&preset.label_key, &preset.label);
            presets =
                presets.child(
                    div()
                        .id(SharedString::from(format!("op-template-{index}")))
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
                            this.apply_template(index, cx)
                        }))
                        .child(label),
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
            .gap_3()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .h_full()
            .overflow_y_scroll()
            .p_3()
            .child(header)
            .child(target);
        if !self.catalog.object_presets.is_empty() {
            column = column.child(section(
                t!("user.create.db_template").to_string(),
                Some(t!("user.create.db_template_hint").to_string()),
                presets.into_any_element(),
                theme,
            ));
        }
        column = column.child(section(
            t!("user.privilege.accounts").to_string(),
            Some(t!("user.privilege.accounts_selected", count = granted).to_string()),
            self.render_accounts(&active, cx),
            theme,
        ));
        if active.is_some() {
            column = column.child(section(
                t!("user.create.fine_grained").to_string(),
                fine_grained_hint,
                self.render_fine_grained(
                    &privileges,
                    &self.active_denied(),
                    self.active_scope(),
                    cx,
                ),
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
                .map(|row| {
                    privilege_summary(&self.rows[row].privileges, &self.catalog.object_presets)
                })
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
        privileges: &BTreeSet<PrivilegeId>,
        denied: &BTreeSet<PrivilegeId>,
        scope: PrivilegeScope,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let mut groups = div().flex().flex_col().gap_3().w_full();
        for group in &self.catalog.groups {
            let offered = self.catalog.group_privileges(&group.id, scope);
            if offered.is_empty() {
                continue;
            }
            let group_label = catalog_label(&group.label_key, &group.label);
            let group_key = group.id.clone();
            let mut grid = div().flex().flex_row().flex_wrap().w_full();
            for (position, privilege) in offered.into_iter().enumerate() {
                let checked = privileges.contains(&privilege);
                let is_denied = denied.contains(&privilege);
                let keyword = privilege.as_str().to_string();
                let description = privilege_label(&self.catalog, &privilege);
                let deny_key = privilege.clone();
                let mut cell = div()
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
                    .tooltip(move |_, cx| cx.new(|_| PrivilegeTooltip(description.clone())).into())
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.toggle_privilege(privilege.clone(), cx)
                    }))
                    .child(checkbox_box(checked, theme))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text))
                            .child(keyword),
                    );
                if self.catalog.deny_supported {
                    let label = t!("user.privilege.deny").to_string();
                    cell = cell.child(
                        div()
                            .id(SharedString::from(format!(
                                "op-deny-{group_key}-{position}"
                            )))
                            .flex_none()
                            .px_1()
                            .rounded(px(3.0))
                            .text_size(px(10.0))
                            .cursor_pointer()
                            .when(is_denied, move |style| {
                                style.bg(rgb(theme.danger)).text_color(rgb(0xffffff))
                            })
                            .when(!is_denied, move |style| {
                                style
                                    .text_color(rgb(theme.text_muted))
                                    .hover(move |style| style.text_color(rgb(theme.danger)))
                            })
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                cx.stop_propagation();
                                this.toggle_privilege_deny(deny_key.clone(), cx);
                            }))
                            .child(label),
                    );
                }
                grid = grid.child(cell);
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
                            .child(group_label),
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

/// The spec an object's grant applies to: `` `db`.* `` for the database, `` `db`.`schema` `` for a
/// schema (`name` = `schema.*`), or `` `db`.`obj` `` for a table.
fn object_spec(database: &str, name: &str) -> String {
    if name.is_empty() {
        format!("`{database}`.*")
    } else if let Some(schema) = name.strip_suffix(".*") {
        format!("`{database}`.`{schema}`")
    } else {
        format!("`{database}`.`{name}`")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_spec_covers_databases_schemas_and_tables() {
        assert_eq!(object_spec("shop", ""), "`shop`.*");
        assert_eq!(object_spec("shop", "public.*"), "`shop`.`public`");
        assert_eq!(object_spec("shop", "orders"), "`shop`.`orders`");
    }
}
