//! The drag-resizable side panes: the connection-tree sidebar on the left and the object-info
//! pane on the right.
//!
//! Each pane is a small child `Entity` that owns its own width and its resize divider, so a drag
//! re-renders only that pane (and re-lays-out its neighbours) instead of the whole `AppView`.
//! The info pane observes `AppView`, so it refreshes whenever the selection changes.

use super::*;
use gpui_kit::component::text::TextView;
use rustgrid_core::DatabaseOptions;

/// What the info pane is currently describing.
#[derive(Clone, PartialEq, Eq)]
enum InfoTarget {
    Connection(usize),
    Database(usize, usize),
    Table(usize, usize, String),
    Routine(usize, usize, String, RoutineKind),
    Query(usize),
    BackupFile(usize),
    BackupConfig(usize),
    User(usize, usize),
    None,
}

impl InfoTarget {
    /// A stable key identifying the target, used to load its data only when it changes.
    fn key(&self) -> String {
        match self {
            InfoTarget::Connection(index) => format!("conn:{index}"),
            InfoTarget::Database(connection, database) => format!("db:{connection}:{database}"),
            InfoTarget::Table(connection, database, name) => {
                format!("tbl:{connection}:{database}:{name}")
            }
            InfoTarget::Routine(connection, database, name, kind) => {
                format!("rtn:{connection}:{database}:{}:{name}", kind.sql_name())
            }
            InfoTarget::Query(index) => format!("query:{index}"),
            InfoTarget::BackupFile(index) => format!("bfile:{index}"),
            InfoTarget::BackupConfig(index) => format!("bconfig:{index}"),
            InfoTarget::User(connection, index) => format!("user:{connection}:{index}"),
            InfoTarget::None => "none".to_string(),
        }
    }
}

// ----- Left sidebar ---------------------------------------------------------------------------

/// The connection-tree pane: the cached tree plus a drag handle on its right edge.
pub(super) struct SidebarHost {
    app: WeakEntity<AppView>,
    width: f32,
    /// `(pointer x, width)` captured when the drag started.
    drag: Option<(f32, f32)>,
    /// Keeps the host observing `AppView` so theme changes propagate.
    _subscription: Subscription,
}

impl SidebarHost {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        app_entity: &Entity<AppView>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let subscription = cx.observe(app_entity, |_, _, cx| cx.notify());
        Self {
            app,
            width: SIDEBAR_DEFAULT_WIDTH,
            drag: None,
            _subscription: subscription,
        }
    }

    pub(super) fn drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some((start_x, start_width)) = self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        let delta = f32::from(event.position.x) - start_x;
        self.width = (start_width + delta).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        cx.notify();
    }

    pub(super) fn end_drag(&mut self, cx: &mut Context<'_, Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }
}

impl Render for SidebarHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (theme, tree) = match self.app.upgrade() {
            Some(app) => {
                let app = app.read(cx);
                (app.theme, app.tree_pane.clone())
            }
            None => return div().into_any_element(),
        };

        let start = cx.listener(|this, event: &MouseDownEvent, _window, cx| {
            this.drag = Some((f32::from(event.position.x), this.width));
            cx.notify();
        });

        div()
            .flex()
            .flex_row()
            .h_full()
            .flex_none()
            .child(
                div()
                    .w(px(self.width))
                    .h_full()
                    .flex_none()
                    .bg(rgb(theme.sidebar_bg))
                    .border_r_1()
                    .border_color(rgb(theme.border))
                    .child(tree.cached(cached_style(|d| {
                        d.flex().flex_col().flex_1().min_h(px(0.0)).w_full()
                    }))),
            )
            .child(
                ui::pane_resize_divider("pane-divider-sidebar", theme)
                    .on_mouse_down(MouseButton::Left, start),
            )
            .into_any_element()
    }
}

// ----- Right info pane ------------------------------------------------------------------------

/// The object-info pane: a drag handle on its left edge and the selected object's details.
pub(super) struct InfoPane {
    app: WeakEntity<AppView>,
    width: f32,
    /// `(pointer x, width)` captured when the drag started.
    drag: Option<(f32, f32)>,
    /// Keeps the info pane observing `AppView` and the connection tree, so selection changes
    /// refresh it.
    _subscription: Subscription,
    _tree_subscription: Subscription,
}

impl InfoPane {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        app_entity: &Entity<AppView>,
        tree_entity: &Entity<TreePane>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let subscription = cx.observe(app_entity, |_, _, cx| cx.notify());
        let tree_subscription = cx.observe(tree_entity, |_, _, cx| cx.notify());
        Self {
            app,
            width: INFO_DEFAULT_WIDTH,
            drag: None,
            _subscription: subscription,
            _tree_subscription: tree_subscription,
        }
    }

    pub(super) fn drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some((start_x, start_width)) = self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        // The divider is on the pane's left edge, so dragging right narrows the pane.
        let delta = f32::from(event.position.x) - start_x;
        self.width = (start_width - delta).clamp(INFO_MIN_WIDTH, INFO_MAX_WIDTH);
        cx.notify();
    }

    pub(super) fn end_drag(&mut self, cx: &mut Context<'_, Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }
}

impl Render for InfoPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (theme, body) = match self.app.upgrade() {
            Some(app) => {
                let weak = self.app.clone();
                let app = app.read(cx);
                (app.theme, app.info_body(&weak, cx))
            }
            None => return div().into_any_element(),
        };

        let start = cx.listener(|this, event: &MouseDownEvent, _window, cx| {
            this.drag = Some((f32::from(event.position.x), this.width));
            cx.notify();
        });

        div()
            .flex()
            .flex_row()
            .h_full()
            .flex_none()
            .child(
                ui::pane_resize_divider("pane-divider-info", theme)
                    .on_mouse_down(MouseButton::Left, start),
            )
            .child(
                div()
                    .id("info-scroll")
                    .w(px(self.width))
                    .h_full()
                    .flex_none()
                    .overflow_y_scroll()
                    .bg(rgb(theme.editor_bg))
                    .child(body),
            )
            .into_any_element()
    }
}

impl AppView {
    /// Compute what the info pane should describe from the current selection. The Backup and
    /// Queries main tabs own their own selections; otherwise the connection tree decides.
    fn info_target(&self, cx: &App) -> InfoTarget {
        if self.main_tab == MainTab::Backups {
            match self.backup_selected {
                Some(BackupSelection::File(index)) if index < self.backup_files.len() => {
                    return InfoTarget::BackupFile(index);
                }
                Some(BackupSelection::Config(index)) if index < self.backup_configs.len() => {
                    return InfoTarget::BackupConfig(index);
                }
                _ => {}
            }
        }
        if self.main_tab == MainTab::Queries
            && let Some(index) = self.saved_query_selected
            && index < self.query_files.len()
        {
            return InfoTarget::Query(index);
        }

        // The Users tab describes the selected account.
        if self.main_tab == MainTab::Users
            && let (Some(connection), Some(index)) = (self.users_connection, self.selected_user)
            && matches!(&self.users, Loadable::Loaded(users) if index < users.len())
        {
            return InfoTarget::User(connection, index);
        }

        // The routine selection (set by the object list or the connection tree) describes a routine
        // while the Functions tab is showing.
        if self.main_tab == MainTab::Functions
            && let Some((connection, database, name, kind)) = self.info_routine_selected.clone()
        {
            return InfoTarget::Routine(connection, database, name, kind);
        }

        // The table selection (set by the object list or the connection tree) describes a table
        // while the Tables/Views tabs are showing.
        if matches!(self.main_tab, MainTab::Tables | MainTab::Views)
            && let Some((connection, database, name)) = self.info_table_selected.clone()
        {
            return InfoTarget::Table(connection, database, name);
        }

        let tree = self.tree_pane.read(cx);
        if let Some((connection_index, database_index, name, _)) = tree.selected_table.clone() {
            return InfoTarget::Table(connection_index, database_index, name);
        }
        let Some(selected) = tree.selected.clone() else {
            return InfoTarget::None;
        };
        if let Some(rest) = selected.strip_prefix("conn-") {
            if let Ok(index) = rest.parse() {
                return InfoTarget::Connection(index);
            }
        } else if let Some(rest) = selected.strip_prefix("db-") {
            let mut parts = rest.splitn(2, '-');
            if let (Some(connection), Some(database)) = (parts.next(), parts.next())
                && let (Ok(connection), Ok(database)) =
                    (connection.parse::<usize>(), database.parse::<usize>())
            {
                return InfoTarget::Database(connection, database);
            }
        } else if let Some(rest) = selected.strip_prefix("cat-") {
            // A category row describes its database.
            let mut parts = rest.splitn(3, '-');
            if let (Some(connection), Some(database)) = (parts.next(), parts.next())
                && let (Ok(connection), Ok(database)) =
                    (connection.parse::<usize>(), database.parse::<usize>())
            {
                return InfoTarget::Database(connection, database);
            }
        } else if let Some(rest) = selected.strip_prefix("rtn-") {
            // A routine leaf (`rtn-<connection>-<database>-<index>`) describes that routine.
            let mut parts = rest.splitn(3, '-');
            if let (Some(connection), Some(database), Some(routine_index)) =
                (parts.next(), parts.next(), parts.next())
                && let (Ok(connection), Ok(database), Ok(routine_index)) = (
                    connection.parse::<usize>(),
                    database.parse::<usize>(),
                    routine_index.parse::<usize>(),
                )
                && let Some((name, kind)) =
                    self.routine_identity(connection, database, routine_index)
            {
                return InfoTarget::Routine(connection, database, name, kind);
            }
        }
        InfoTarget::None
    }

    /// Record the table the info pane should describe. Called by the object list (single click)
    /// and the connection tree (table selection).
    pub(super) fn set_info_table(
        &mut self,
        connection_index: usize,
        database_index: usize,
        name: String,
    ) {
        self.info_table_selected = Some((connection_index, database_index, name));
        self.info_routine_selected = None;
    }

    /// Record the stored routine the info pane should describe. Called by the object list and the
    /// connection tree.
    pub(super) fn set_info_routine(
        &mut self,
        connection_index: usize,
        database_index: usize,
        name: String,
        kind: RoutineKind,
    ) {
        self.info_routine_selected = Some((connection_index, database_index, name, kind));
        self.info_table_selected = None;
    }

    /// Resolve a connection-tree routine leaf's index to its name and kind, for the info pane.
    fn routine_identity(
        &self,
        connection_index: usize,
        database_index: usize,
        routine_index: usize,
    ) -> Option<(String, RoutineKind)> {
        self.connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index),
                _ => None,
            })
            .and_then(|database| match &database.routines {
                Loadable::Loaded(routines) => routines.get(routine_index),
                _ => None,
            })
            .map(|routine| (routine.name.clone(), routine.kind))
    }

    /// Drop both the table and routine info selections when a connection/database/category row is
    /// selected.
    pub(super) fn clear_info_selection(&mut self) {
        self.info_table_selected = None;
        self.info_routine_selected = None;
    }

    /// Drop only the routine info selection, leaving any table selection in place. Used when the
    /// object list clears its highlight (switching main tabs).
    pub(super) fn clear_info_routine(&mut self) {
        self.info_routine_selected = None;
    }

    /// Kick off the async introspection the newly-selected object needs. Called every render but
    /// only does work when the selection changed.
    pub(super) fn sync_info(&mut self, cx: &mut Context<'_, Self>) {
        let target = self.info_target(cx);
        let key = target.key();
        if self.info_loaded_for.as_deref() == Some(key.as_str()) {
            return;
        }
        self.info_loaded_for = Some(key);
        self.info_server = Loadable::Idle;
        self.info_database = Loadable::Idle;
        self.info_table_status = Loadable::Idle;
        self.info_table_ddl = Loadable::Idle;
        self.info_table_ddl_view = false;
        self.info_routine = Loadable::Idle;
        self.info_user = Loadable::Idle;

        match target {
            InfoTarget::Connection(connection_index) => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    self.info_server = Loadable::Failed(t!("info.not_connected").to_string());
                    return;
                };
                let runtime = self.runtime.clone();
                cx.spawn(async move |this, cx| {
                    let version = {
                        let connection = connection.clone();
                        runtime
                            .spawn(async move { connection.server_version().await })
                            .await
                    };
                    let sessions = {
                        let connection = connection.clone();
                        runtime
                            .spawn(async move { connection.session_count().await })
                            .await
                    };
                    let _ = this.update(cx, |app, cx| {
                        app.info_server = combine_server_info(
                            version.map_err(Error::other),
                            sessions.map_err(Error::other),
                        );
                        cx.notify();
                    });
                })
                .detach();
            }
            InfoTarget::Database(connection_index, database_index) => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    self.info_database = Loadable::Failed(t!("info.not_connected").to_string());
                    return;
                };
                let Some(database) = self.database_name(connection_index, database_index) else {
                    return;
                };
                let runtime = self.runtime.clone();
                cx.spawn(async move |this, cx| {
                    let result = runtime
                        .spawn(async move { connection.database_options(&database).await })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        app.info_database = match result {
                            Ok(inner) => loadable(inner),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                        cx.notify();
                    });
                })
                .detach();
            }
            InfoTarget::Table(connection_index, database_index, name) => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    self.info_table_status = Loadable::Failed(t!("info.not_connected").to_string());
                    return;
                };
                let Some(database) = self.database_name(connection_index, database_index) else {
                    return;
                };
                // A view is scripted with the view verb; a table with the table verb.
                let is_view = self.table_is_view(connection_index, database_index, &name);
                let runtime = self.runtime.clone();
                cx.spawn(async move |this, cx| {
                    let status_connection = connection.clone();
                    let ddl_connection = connection.clone();
                    let status_database = database.clone();
                    let ddl_database = database.clone();
                    let status_name = name.clone();
                    let ddl_name = name.clone();
                    let status = runtime
                        .spawn(async move {
                            status_connection
                                .table_status(&status_database, &status_name)
                                .await
                        })
                        .await;
                    let ddl = runtime
                        .spawn(async move {
                            ddl_connection
                                .object_ddl(&ddl_database, &ddl_name, is_view)
                                .await
                        })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        app.info_table_status = match status {
                            Ok(inner) => loadable(inner),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                        app.info_table_ddl = match ddl {
                            Ok(inner) => loadable(inner),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                        cx.notify();
                    });
                })
                .detach();
            }
            InfoTarget::Routine(connection_index, database_index, name, kind) => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    self.info_routine = Loadable::Failed(t!("info.not_connected").to_string());
                    return;
                };
                let Some(database) = self.database_name(connection_index, database_index) else {
                    return;
                };
                let runtime = self.runtime.clone();
                cx.spawn(async move |this, cx| {
                    let result = runtime
                        .spawn(
                            async move { connection.routine_details(&database, kind, &name).await },
                        )
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        app.info_routine = match result {
                            Ok(inner) => loadable(inner),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                        cx.notify();
                    });
                })
                .detach();
            }
            InfoTarget::User(connection_index, index) => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    self.info_user = Loadable::Failed(t!("info.not_connected").to_string());
                    return;
                };
                let account = match &self.users {
                    Loadable::Loaded(users) => users.get(index).cloned(),
                    _ => None,
                };
                let Some(account) = account else {
                    return;
                };
                let runtime = self.runtime.clone();
                cx.spawn(async move |this, cx| {
                    let result = runtime
                        .spawn(async move {
                            connection.user_details(&account.user, &account.host).await
                        })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        app.info_user = match result {
                            Ok(inner) => loadable(inner),
                            Err(error) => Loadable::Failed(error.to_string()),
                        };
                        cx.notify();
                    });
                })
                .detach();
            }
            _ => {}
        }
    }

    /// Build the info pane's contents for the current selection.
    pub(super) fn info_body(&self, app: &WeakEntity<AppView>, cx: &App) -> AnyElement {
        let theme = self.theme;
        match self.info_target(cx) {
            InfoTarget::None => div().into_any_element(),
            InfoTarget::BackupFile(index) => {
                self.backup_info_body(BackupSelection::File(index), theme)
            }
            InfoTarget::BackupConfig(index) => {
                self.backup_info_body(BackupSelection::Config(index), theme)
            }
            InfoTarget::Query(index) => self.saved_query_info(index, theme),
            InfoTarget::Connection(index) => self.connection_info(index, theme),
            InfoTarget::Database(connection, database) => {
                self.database_info(connection, database, theme)
            }
            InfoTarget::Table(connection, database, name) => {
                self.table_info(connection, database, &name, app, theme)
            }
            InfoTarget::Routine(_, _, name, kind) => self.routine_info(&name, kind, theme),
            InfoTarget::User(_, index) => self.user_info(index, theme),
        }
    }

    /// The details pane of the Users main tab: the selected account's attributes, plus whether it
    /// holds the server-wide `SUPER` privilege (loaded with the rest of its details).
    fn user_info(&self, index: usize, theme: Theme) -> AnyElement {
        let account = match &self.users {
            Loadable::Loaded(users) => users.get(index),
            _ => None,
        };
        let Some(account) = account else {
            return div().into_any_element();
        };
        let superuser = match &self.info_user {
            Loadable::Loaded(details) => {
                Some(details.server_privileges.contains(&Privilege::Super))
            }
            _ => None,
        };
        let yes_no = |value: bool| t!(if value { "common.yes" } else { "common.no" }).to_string();
        let fields = vec![
            (
                t!("user.field.ssl_type").to_string(),
                if account.ssl_type.is_empty() {
                    "--".to_string()
                } else {
                    account.ssl_type.clone()
                },
            ),
            (
                t!("user.field.max_questions").to_string(),
                account.max_questions.to_string(),
            ),
            (
                t!("user.field.max_updates").to_string(),
                account.max_updates.to_string(),
            ),
            (
                t!("user.field.max_connections").to_string(),
                account.max_connections.to_string(),
            ),
            (
                t!("user.field.max_user_connections").to_string(),
                account.max_user_connections.to_string(),
            ),
            (
                t!("user.field.superuser").to_string(),
                superuser.map(yes_no).unwrap_or_else(|| "--".to_string()),
            ),
        ];
        info_panel(
            "icons/user.svg",
            theme.icon_users,
            &account.label(),
            &t!("common.user"),
            &fields,
        )
    }

    fn connection_info(&self, index: usize, theme: Theme) -> AnyElement {
        let Some(node) = self.connections.get(index) else {
            return div().into_any_element();
        };
        let profile = &node.profile;
        let queries = self
            .query_files
            .iter()
            .filter(|file| file.connection_id == profile.id)
            .count();
        let (version, sessions) = match &self.info_server {
            Loadable::Loaded((version, sessions)) => (version.clone(), format_count(*sessions)),
            Loadable::Loading | Loadable::Idle => ("--".to_string(), "--".to_string()),
            Loadable::Failed(_) => ("--".to_string(), "--".to_string()),
        };
        let fields = vec![
            (t!("info.server_version").to_string(), version),
            (t!("info.sessions").to_string(), sessions),
            (t!("info.host").to_string(), profile.host.clone()),
            (t!("info.port").to_string(), profile.port.to_string()),
            (t!("info.username").to_string(), profile.username.clone()),
            (
                t!("info.settings_location").to_string(),
                self.config.root().display().to_string(),
            ),
            (
                t!("info.encoding").to_string(),
                t!("info.encoding_auto").to_string(),
            ),
            (t!("info.ssh_host").to_string(), "--".to_string()),
            (t!("info.http_tunnel").to_string(), "--".to_string()),
        ];
        info_panel(
            "icons/connection.svg",
            theme.icon_connection,
            &profile.name,
            &format!("{} {}", queries, t!("common.query")),
            &fields,
        )
    }

    fn database_info(
        &self,
        connection_index: usize,
        database_index: usize,
        theme: Theme,
    ) -> AnyElement {
        let Some(name) = self.database_name(connection_index, database_index) else {
            return div().into_any_element();
        };
        let query_count = self
            .connections
            .get(connection_index)
            .map(|node| {
                self.query_files
                    .iter()
                    .filter(|file| file.connection_id == node.profile.id && file.database == name)
                    .count()
            })
            .unwrap_or(0);
        let options = match &self.info_database {
            Loadable::Loaded(options) => options.clone(),
            _ => DatabaseOptions::default(),
        };
        let mut fields = Vec::new();
        if !options.charset.is_empty() {
            fields.push((t!("info.charset").to_string(), options.charset.clone()));
        }
        if !options.collation.is_empty() {
            fields.push((t!("info.collation").to_string(), options.collation.clone()));
        }
        if !options.owner.is_empty() {
            fields.push((t!("database.owner").to_string(), options.owner.clone()));
        }
        if !options.recovery_model.is_empty() {
            fields.push((
                t!("database.recovery_model").to_string(),
                options.recovery_model.clone(),
            ));
        }
        if !options.compatibility_level.is_empty() {
            fields.push((
                t!("database.compatibility_level").to_string(),
                options.compatibility_level.clone(),
            ));
        }
        info_panel(
            "icons/database.svg",
            theme.icon_database_active,
            &name,
            &format!("{} {}", query_count, t!("common.query")),
            &fields,
        )
    }

    fn table_info(
        &self,
        connection_index: usize,
        database_index: usize,
        name: &str,
        app: &WeakEntity<AppView>,
        theme: Theme,
    ) -> AnyElement {
        // A view reads as a view; a table's status is loaded from the server.
        let is_view = self.table_is_view(connection_index, database_index, name);
        if self.info_table_ddl_view {
            return self.table_ddl_view(name, is_view, theme);
        }
        let empty = TableStatus::default();
        let status = match &self.info_table_status {
            Loadable::Loaded(status) => Some(status),
            _ => None,
        };
        let status = status.unwrap_or(&empty);
        let field = |label: &str, value: Option<String>| {
            (label.to_string(), value.unwrap_or_else(|| "--".to_string()))
        };
        let fields = vec![
            (
                t!("info.rows").to_string(),
                status
                    .rows
                    .map(format_count)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            field(&t!("info.engine"), status.engine.clone()),
            (
                t!("info.auto_increment").to_string(),
                status
                    .auto_increment
                    .map(format_count)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            field(&t!("info.row_format"), status.row_format.clone()),
            field(&t!("info.updated"), status.updated.clone()),
            field(&t!("info.created"), status.created.clone()),
            field(&t!("info.checked"), status.checked.clone()),
            (
                t!("info.index_length").to_string(),
                status
                    .index_length
                    .map(format_size)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            (
                t!("info.data_length").to_string(),
                status
                    .data_length
                    .map(format_size)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            (
                t!("info.max_data_length").to_string(),
                status
                    .max_data_length
                    .map(format_size)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            (
                t!("info.data_free").to_string(),
                status
                    .data_free
                    .map(format_size)
                    .unwrap_or_else(|| "--".to_string()),
            ),
            field(&t!("info.collation"), status.collation.clone()),
            field(&t!("info.create_options"), status.create_options.clone()),
            field(&t!("info.comment"), status.comment.clone()),
        ];
        let kind = if is_view {
            t!("common.view")
        } else {
            t!("common.table")
        };
        let icon = if is_view {
            "icons/views.svg"
        } else {
            "icons/tables.svg"
        };
        let color = if is_view {
            theme.icon_view
        } else {
            theme.icon_table
        };
        let body = info_panel(icon, color, name, &kind, &fields);
        self.table_info_view_bar(app, theme)
            .child(body)
            .into_any_element()
    }

    /// Whether the table/view `name` is a view in its database's loaded catalog.
    fn table_is_view(&self, connection_index: usize, database_index: usize, name: &str) -> bool {
        self.connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index),
                _ => None,
            })
            .is_some_and(|database| {
                matches!(&database.tables, Loadable::Loaded(tables)
                    if tables.iter().any(|table| table.name == name && matches!(table.kind, rustgrid_core::ObjectKind::View)))
            })
    }

    /// The table info pane's two view buttons (详细信息 / DDL), Navicat's icon pair above the
    /// object title. The active one is filled.
    fn table_info_view_bar(&self, app: &WeakEntity<AppView>, theme: Theme) -> Div {
        let details = !self.info_table_ddl_view;
        let ddl = self.info_table_ddl_view;
        let weak_details = app.clone();
        let weak_ddl = app.clone();
        let button = |id: &'static str, icon: &'static str, active: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(px(28.0))
                .h(px(22.0))
                .rounded_sm()
                .cursor_pointer()
                .when(active, |style| style.bg(rgb(theme.button_bg)))
                .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                .child(
                    svg()
                        .path(icon)
                        .w(px(15.0))
                        .h(px(15.0))
                        .text_color(rgb(if active { theme.text } else { theme.text_muted })),
                )
        };
        let mut tabs = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .child(
                button("info-view-details", "icons/database.svg", details).on_click(
                    move |_event, _window, cx| {
                        let _ = weak_details.update(cx, |app, cx| {
                            app.info_table_ddl_view = false;
                            cx.notify();
                        });
                    },
                ),
            )
            .child(button("info-view-ddl", "icons/queries.svg", ddl).on_click(
                move |_event, _window, cx| {
                    let _ = weak_ddl.update(cx, |app, cx| {
                        app.info_table_ddl_view = true;
                        cx.notify();
                    });
                },
            ));
        // A copy button for the script, only while the DDL view is showing.
        if let (true, Loadable::Loaded(Some(ddl))) =
            (self.info_table_ddl_view, &self.info_table_ddl)
        {
            let ddl = ddl.clone();
            tabs = tabs.child(
                div()
                    .id("info-ddl-copy")
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(22.0))
                    .px_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .child(svg().path("icons/queries.svg").w(px(13.0)).h(px(13.0)))
                    .child(t!("query.copy").to_string())
                    .on_click(move |_event, _window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(ddl.clone()));
                    }),
            );
        }
        div().flex().flex_col().w_full().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_3()
                .pt_3()
                .child(tabs),
        )
    }

    /// The table info pane's DDL view: the `CREATE` script in a copyable code block.
    fn table_ddl_view(&self, name: &str, is_view: bool, theme: Theme) -> AnyElement {
        let _ = (name, is_view, theme);
        let script: AnyElement = match &self.info_table_ddl {
            Loadable::Loaded(Some(ddl)) => div()
                .id("info-ddl")
                .p_3()
                .child(
                    TextView::markdown("info-ddl-text", format!("```sql\n{ddl}\n```"))
                        .selectable(true)
                        .text_size(px(12.0)),
                )
                .into_any_element(),
            Loadable::Loaded(None) => div()
                .p_4()
                .text_color(rgb(self.theme.text_muted))
                .child(t!("info.ddl_unavailable").to_string())
                .into_any_element(),
            Loadable::Failed(error) => div()
                .p_4()
                .text_color(rgb(self.theme.danger))
                .child(error.clone())
                .into_any_element(),
            Loadable::Idle | Loadable::Loading => div()
                .p_4()
                .text_color(rgb(self.theme.text_muted))
                .child(t!("common.loading").to_string())
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(script)
            .into_any_element()
    }

    /// The details pane for a stored routine (the Functions tab's selection): its kind, timestamps,
    /// definer, access characteristics and comment, mirroring Navicat's object details page. The
    /// full `CREATE` statement lives in the routine editor's 定义 tab.
    fn routine_info(&self, name: &str, kind: RoutineKind, theme: Theme) -> AnyElement {
        let details = match &self.info_routine {
            Loadable::Loaded(details) => Some(details),
            _ => None,
        };
        let info = details.map(|details| &details.info);
        let field = |label: &str, value: Option<String>| {
            (label.to_string(), value.unwrap_or_else(|| "--".to_string()))
        };
        let mut fields = vec![(
            t!("routine.field.kind").to_string(),
            kind.sql_name().to_string(),
        )];
        if kind == RoutineKind::Function {
            fields.push(field(
                &t!("routine.field.returns"),
                info.map(|info| info.return_type.clone())
                    .filter(|value| !value.is_empty()),
            ));
        }
        fields.push(field(
            &t!("routine.field.definer"),
            info.map(|info| info.definer.clone()),
        ));
        fields.push(field(
            &t!("routine.field.created"),
            info.and_then(|info| info.created.clone()),
        ));
        fields.push(field(
            &t!("routine.field.modified"),
            info.and_then(|info| info.modified.clone()),
        ));
        fields.push(field(
            &t!("routine.field.security"),
            info.map(|info| info.security_type.clone()),
        ));
        fields.push(field(
            &t!("routine.field.data_access"),
            info.map(|info| info.data_access.clone()),
        ));
        fields.push((
            t!("routine.field.deterministic").to_string(),
            info.map(|info| {
                if info.deterministic {
                    t!("common.yes").to_string()
                } else {
                    t!("common.no").to_string()
                }
            })
            .unwrap_or_else(|| "--".to_string()),
        ));
        fields.push(field(
            &t!("routine.field.comment"),
            info.map(|info| info.comment.clone()),
        ));
        if let Some(details) = details {
            fields.push(field(
                &t!("routine.field.sql_mode"),
                Some(details.sql_mode.clone()).filter(|value| !value.is_empty()),
            ));
            fields.push(field(
                &t!("routine.field.charset"),
                Some(details.character_set_client.clone()).filter(|value| !value.is_empty()),
            ));
            fields.push(field(
                &t!("routine.field.collation"),
                Some(details.collation_connection.clone()).filter(|value| !value.is_empty()),
            ));
        }
        info_panel(
            routine_icon(kind),
            routine_icon_color(kind, theme),
            name,
            t!(kind.label_key()).as_ref(),
            &fields,
        )
    }

    fn saved_query_info(&self, index: usize, theme: Theme) -> AnyElement {
        let Some(file) = self.query_files.get(index) else {
            return div().into_any_element();
        };
        let connection = self.query_connection_name(&file.connection_id);
        let created = file.created.map(format_system_time);
        let modified = file.modified.map(format_system_time);
        let fields = vec![
            (
                t!("backup.field.file").to_string(),
                file.path.display().to_string(),
            ),
            (t!("backup.field.size").to_string(), format_size(file.size)),
            (
                t!("backup.field.created").to_string(),
                created.unwrap_or_else(|| "--".to_string()),
            ),
            (
                t!("backup.field.modified").to_string(),
                modified.unwrap_or_else(|| "--".to_string()),
            ),
            (t!("info.connection").to_string(), connection),
            (
                t!("info.database").to_string(),
                if file.database.is_empty() {
                    "--".to_string()
                } else {
                    file.database.clone()
                },
            ),
        ];
        info_panel(
            "icons/queries.svg",
            theme.icon_queries,
            &file.name,
            &t!("common.query"),
            &fields,
        )
    }
}

/// Format a filesystem timestamp for the info pane.
fn format_system_time(time: std::time::SystemTime) -> String {
    let time: chrono::DateTime<chrono::Local> = time.into();
    time.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Load an inner `Result` into a `Loadable`, stringifying the error.
fn loadable<T>(result: rustgrid_core::Result<T>) -> Loadable<T> {
    match result {
        Ok(value) => Loadable::Loaded(value),
        Err(error) => Loadable::Failed(error.to_string()),
    }
}

fn combine_server_info(
    version: rustgrid_core::Result<rustgrid_core::Result<String>>,
    sessions: rustgrid_core::Result<rustgrid_core::Result<u64>>,
) -> Loadable<(String, u64)> {
    let version = match version {
        Ok(Ok(version)) => version,
        Ok(Err(error)) => return Loadable::Failed(error.to_string()),
        Err(error) => return Loadable::Failed(error.to_string()),
    };
    let sessions = match sessions {
        Ok(Ok(sessions)) => sessions,
        Ok(Err(error)) => return Loadable::Failed(error.to_string()),
        Err(error) => return Loadable::Failed(error.to_string()),
    };
    Loadable::Loaded((version, sessions))
}

/// The selectable body of the info pane: a big icon, a title and the label/value fields, all
/// rendered through a `TextView` so the values can be selected and copied with the mouse.
pub(super) fn info_panel(
    icon: &'static str,
    icon_color: u32,
    title: &str,
    subtitle: &str,
    fields: &[(String, String)],
) -> AnyElement {
    let title_markdown = format!(
        "**{}**\n\n{}",
        escape_markdown(title),
        escape_markdown(subtitle)
    );
    let mut fields_markdown = String::new();
    for (label, value) in fields {
        fields_markdown.push_str(&format!(
            "**{}**\n\n{}\n\n",
            escape_markdown(label),
            escape_markdown(value)
        ));
    }
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    svg()
                        .path(icon)
                        .w(px(40.0))
                        .h(px(40.0))
                        .flex_none()
                        .text_color(rgb(icon_color)),
                )
                .child(
                    TextView::markdown("info-pane-title", title_markdown)
                        .selectable(true)
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(15.0)),
                ),
        )
        .child(
            TextView::markdown("info-pane-fields", fields_markdown)
                .selectable(true)
                .text_size(px(12.5)),
        )
        .into_any_element()
}

/// Escape the Markdown control characters that can appear in a value (Windows paths in
/// particular), so the info pane's text view renders it verbatim.
pub(super) fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#'
        ) {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

/// Group an integer with thousands separators.
fn format_count(value: u64) -> String {
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

/// Human-readable byte size, matching the backup details pane.
fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!(
            "{:.2} GB ({})",
            bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            format_count(bytes)
        )
    } else if bytes >= 1024 * 1024 {
        format!(
            "{:.2} MB ({})",
            bytes as f64 / (1024.0 * 1024.0),
            format_count(bytes)
        )
    } else if bytes >= 1024 {
        format!("{:.2} KB ({})", bytes as f64 / 1024.0, format_count(bytes))
    } else {
        format!("{bytes} B ({bytes})")
    }
}
