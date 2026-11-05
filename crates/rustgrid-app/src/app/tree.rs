use super::*;

impl AppView {
    pub(super) fn connection_arc(&self, index: usize) -> Option<Arc<dyn Connection>> {
        match &self.connections.get(index)?.status {
            ConnectionStatus::Connected(connection) => Some(connection.clone()),
            _ => None,
        }
    }

    /// The connection-tree index of an active connection, matched by identity. Lets a grid that
    /// only holds the shared `Connection` handle find its tree row (and database index).
    pub(super) fn connection_index_of(&self, connection: &Arc<dyn Connection>) -> Option<usize> {
        self.connections.iter().position(|node| match &node.status {
            ConnectionStatus::Connected(active) => Arc::ptr_eq(active, connection),
            _ => false,
        })
    }

    /// Set the connection tree's search text. The tree filters what is already loaded — its
    /// connections, databases, schemas and tables (a table matches its bare name,
    /// `schema.table` and `database.schema.table`) — and never issues a catalog query of its own:
    /// a database has to be opened (which loads its objects) before its tables are searchable.
    pub(super) fn set_tree_search(&mut self, text: String, cx: &mut Context<'_, Self>) {
        if self.tree_search == text {
            return;
        }
        self.tree_search = text;
        self.tree_pane.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    /// Clear the tree search (Esc, or the field's clear button once it has reported `""`), showing
    /// the whole tree again.
    pub(super) fn clear_tree_search(&mut self, cx: &mut Context<'_, Self>) {
        self.tree_search.clear();
        self.tree_search_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.tree_pane.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    /// Ctrl/Cmd+F while the tree holds focus moves the caret into the sidebar's search box, so
    /// filtering the tree never needs the mouse. The box sits above the tree's key context, so
    /// taking focus also leaves that context — like clicking it does.
    pub(super) fn focus_tree_search(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.tree_search_input
            .update(cx, |input, cx| input.focus_state(window, cx));
        cx.notify();
    }

    /// The connection the active tab belongs to, falling back to the tree's current selection.
    /// The main ribbon's key-value gating uses this so a restored key tab (whose tree row may not
    /// be selected yet) still hides the SQL-only tabs.
    pub(super) fn active_connection_index(&self, cx: &App) -> Option<usize> {
        if let Some(grid) = self.active_grid.and_then(|index| self.grids.get(index))
            && let Some(index) = self.connection_index_of(&grid.read(cx).state.connection)
        {
            return Some(index);
        }
        if let Some(design) = self.active_design.and_then(|index| self.designs.get(index))
            && let Some(index) = self.connection_index_of(&design.read(cx).connection)
        {
            return Some(index);
        }
        if let Some(index) = self
            .active_query
            .and_then(|index| self.queries.get(index))
            .and_then(|query| query.connection_index)
        {
            return Some(index);
        }
        self.users_connection_index(cx)
    }

    /// Resolve a grid's `(connection, database name)` back to the tree's indices, so per-table
    /// actions (import/export) can reuse the connection-tree-scoped wizards.
    pub(super) fn table_scope(
        &self,
        connection: &Arc<dyn Connection>,
        database: &str,
    ) -> Option<(usize, usize)> {
        let connection_index = self.connection_index_of(connection)?;
        let database_index = self.database_index_by_name(connection_index, database)?;
        Some((connection_index, database_index))
    }

    pub(super) fn toggle_expand(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let mut changed = false;
        if let Some(node) = self.connections.get_mut(index)
            && matches!(&node.status, ConnectionStatus::Connected(_))
        {
            node.expanded = !node.expanded;
            changed = true;
        }
        if changed {
            self.persist_settings();
            cx.notify();
        }
    }

    pub(super) fn open_new_form(
        &mut self,
        driver: DriverId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.editing = None;
        let (default_port, default_username) = self
            .registry
            .get(&driver)
            .map(|driver| (driver.default_port(), driver.default_username()))
            .unwrap_or((3306, "root"));
        self.show_form(
            ConnectionForm::for_driver(driver, default_port, default_username),
            window,
            cx,
        );
    }

    pub(super) fn open_edit_form(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let (profile, password, password_saved) = match self.connections.get(index) {
            Some(node) => (
                node.profile.clone(),
                node.password.clone(),
                node.password_saved,
            ),
            None => return,
        };

        self.editing = Some(index);
        self.pending_group = None;
        self.show_form(
            ConnectionForm::from_profile(&profile, password, password_saved),
            window,
            cx,
        );
    }

    /// Open the new-connection dialog pre-filled with an existing connection's settings. Saving
    /// it creates a separate connection (name-uniqueness is validated on save).
    pub(super) fn open_copy_form(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let (profile, password, password_saved) = match self.connections.get(index) {
            Some(node) => (
                node.profile.clone(),
                node.password.clone(),
                node.password_saved,
            ),
            None => return,
        };

        self.editing = None;
        self.pending_group = None;
        self.show_form(
            ConnectionForm::from_profile(&profile, password, password_saved),
            window,
            cx,
        );
    }

    fn show_form(
        &mut self,
        mut form: ConnectionForm,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let descriptor = self
            .registry
            .get(&form.driver)
            .map(|driver| driver.descriptor());
        // The driver's spec decides the tabs, the 常规 page's shape, the standard-field renames and
        // where the engine's own option fields go.
        if let Some(descriptor) = descriptor.as_ref() {
            form.apply_spec(&descriptor.connection_form);
        }
        let weak = cx.weak_entity();
        let theme = self.theme;
        let inputs = FormInputs {
            fields: [
                make_form_input(theme, form.name.clone(), false, FormField::Name, &weak, cx),
                make_form_input(theme, form.host.clone(), false, FormField::Host, &weak, cx),
                make_form_input(theme, form.port.clone(), false, FormField::Port, &weak, cx),
                make_form_input(
                    theme,
                    form.username.clone(),
                    false,
                    FormField::Username,
                    &weak,
                    cx,
                ),
                make_form_input(
                    theme,
                    form.password.clone(),
                    true,
                    FormField::Password,
                    &weak,
                    cx,
                ),
                make_form_input(
                    theme,
                    form.database.clone(),
                    false,
                    FormField::Database,
                    &weak,
                    cx,
                ),
                make_form_input(
                    theme,
                    form.odbc_driver.clone(),
                    false,
                    FormField::OdbcDriver,
                    &weak,
                    cx,
                ),
                make_form_input(
                    theme,
                    form.odbc_dsn.clone(),
                    false,
                    FormField::OdbcDsn,
                    &weak,
                    cx,
                ),
                make_form_input(
                    theme,
                    form.odbc_connection_string.clone(),
                    false,
                    FormField::OdbcConnectionString,
                    &weak,
                    cx,
                ),
                make_form_input(
                    theme,
                    form.odbc_engine.clone(),
                    false,
                    FormField::OdbcEngine,
                    &weak,
                    cx,
                ),
            ],
        };
        let name_focus = inputs.get(FormField::Name).read(cx).focus_handle();
        let odbc_driver = if form.odbc {
            let options: Vec<ComboOption> = self
                .registry
                .get(&form.driver)
                .map(|driver| driver.connection_drivers())
                .unwrap_or_default()
                .into_iter()
                .map(ComboOption::plain)
                .collect();
            // An empty list means the system has no ODBC driver manager (or no driver) installed.
            // The free-text driver field takes over in that case — see `render_general_page`.
            if options.is_empty() {
                None
            } else {
                let selected = form.odbc_driver.clone();
                let app = weak.clone();
                Some(cx.new(|cx| {
                    ComboBox::new(theme, options, selected, 260.0, cx)
                        .full_width()
                        .on_select(Rc::new(move |value, _window, cx| {
                            let _ = app.update(cx, |app, cx| {
                                app.set_form_field(FormField::OdbcDriver, value, cx)
                            });
                        }))
                }))
            }
        } else {
            None
        };
        self.form_odbc_driver = odbc_driver;
        self.form_initial = Some(form.clone());
        self.form_inputs = Some(inputs);
        self.form = Some(form);
        self.test_status = TestStatus::Idle;
        self.context_menu = None;
        self.rebuild_form_extra_inputs(cx);
        self.rebuild_form_engine_inputs(cx);
        self.rebuild_tunnel_inputs(cx);
        self.open_connection_window(name_focus, cx);
        cx.notify();
    }

    pub(super) fn delete_connection(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.connections.len() {
            return;
        }

        self.disconnect(index, cx);
        let removed_id = self
            .connections
            .get(index)
            .map(|node| node.profile.id.clone());
        self.connections.remove(index);
        if let Some(id) = removed_id {
            groups::remove_connection_entry(&mut self.tree_layout, &id);
        }

        let active_query_id = self
            .active_query
            .and_then(|active| self.queries.get(active))
            .map(|tab| tab.id);
        self.queries
            .retain(|tab| tab.connection_index != Some(index));
        for tab in self.queries.iter_mut() {
            if let Some(connection_index) = tab.connection_index
                && connection_index > index
            {
                tab.connection_index = Some(connection_index - 1);
            }
        }
        self.active_query =
            active_query_id.and_then(|id| self.queries.iter().position(|tab| tab.id == id));

        if let Some(editing) = self.editing {
            match editing.cmp(&index) {
                std::cmp::Ordering::Equal => {
                    self.editing = None;
                    self.form = None;
                    self.form_inputs = None;
                }
                std::cmp::Ordering::Greater => self.editing = Some(editing - 1),
                std::cmp::Ordering::Less => {}
            }
        }

        let profiles: Vec<_> = self
            .connections
            .iter()
            .map(|node| node.profile.clone())
            .collect();
        if let Err(error) = self.config.save_profiles(&profiles) {
            self.warn_persist("connections", error, cx);
        }
        self.persist_secrets(cx);
        self.persist_settings();

        // Connection indices shift when one is removed, so any `conn-*` selection is now stale.
        self.tree_selection.clear();
        cx.notify();
    }

    /// Persist the connection profiles, reporting a write failure instead of dropping it.
    pub(super) fn persist_profiles(&mut self, cx: &mut Context<'_, Self>) {
        let profiles: Vec<_> = self
            .connections
            .iter()
            .map(|node| node.profile.clone())
            .collect();
        if let Err(error) = self.config.save_profiles(&profiles) {
            self.warn_persist("connections", error, cx);
        }
    }

    pub(super) fn persist_secrets(&mut self, cx: &mut Context<'_, Self>) {
        let secrets: BTreeMap<String, String> = self
            .connections
            .iter()
            .filter(|node| node.password_saved)
            .filter_map(|node| {
                node.password
                    .clone()
                    .map(|password| (node.profile.id.clone(), password))
            })
            .collect();
        if let Err(error) = self.config.save_secrets(&secrets) {
            self.warn_persist("passwords", error, cx);
        }
    }

    pub(super) fn connect(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(profile) = self.connections.get(index).map(|node| node.profile.clone()) else {
            return;
        };
        let password = self
            .connections
            .get(index)
            .and_then(|node| node.password.clone());

        let Some(driver) = self.registry.get(&profile.driver) else {
            let message = t!("error.driver_missing").to_string();
            if let Some(node) = self.connections.get_mut(index) {
                node.status = ConnectionStatus::Failed(message.clone());
            }
            self.error_dialog = Some(message);
            cx.notify();
            return;
        };

        if let Some(node) = self.connections.get_mut(index) {
            node.status = ConnectionStatus::Connecting;
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let config = ConnectionConfig {
                driver: profile.driver.clone(),
                host: profile.host.clone(),
                port: profile.port,
                username: profile.username.clone(),
                password,
                database: profile.database.clone(),
                options: profile.options.clone(),
                settings: profile.settings.clone(),
            };

            let result = match runtime
                .spawn(async move { driver.connect(&config).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(connection) => {
                        if let Some(node) = view.connections.get_mut(index) {
                            node.status = ConnectionStatus::Connected(Arc::from(connection));
                            node.expanded = true;
                        }
                        view.load_databases(index, cx);
                        // Reopen the last session's tables on this connection.
                        view.restore_session_tables(index, cx);
                    }
                    Err(error) => {
                        let authentication = matches!(&error, Error::Authentication(_));
                        let message = error.to_string();

                        if let Some(node) = view.connections.get_mut(index) {
                            node.status = ConnectionStatus::Failed(message.clone());
                        }

                        if authentication {
                            let weak = cx.weak_entity();
                            let theme = view.theme;
                            let input = make_password_input(theme, &weak, cx);
                            view.password_prompt = Some(PasswordPrompt {
                                index,
                                input,
                                password: String::new(),
                                save_password: true,
                            });
                            view.password_focus_pending = true;
                        } else {
                            // Surface the failure in a dialog rather than dumping the (often long)
                            // driver message under the tree node; the red icon still marks it.
                            view.error_dialog = Some(message);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Disconnect a connection, asking first when it has unsaved work (grids/designers/queries).
    pub(super) fn request_disconnect(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if self.connection_has_unsaved(index, cx) {
            self.unsaved_confirm = Some(PendingAction::Disconnect { index });
            cx.notify();
            return;
        }
        self.disconnect(index, cx);
    }

    /// Open every connection in the tree's multi-selection (a selected group contributes its
    /// connections). Connections that are already open or opening are left alone.
    pub(super) fn connect_tree_selection(&mut self, cx: &mut Context<'_, Self>) {
        for index in self.tree_selection_connection_indexes() {
            let closed = matches!(
                self.connections.get(index).map(|node| &node.status),
                Some(ConnectionStatus::Disconnected) | Some(ConnectionStatus::Failed(_))
            );
            if closed {
                self.connect(index, cx);
            }
        }
    }

    /// Close every open connection in the tree's multi-selection, asking once when any of them has
    /// unsaved work.
    pub(super) fn request_disconnect_tree_selection(&mut self, cx: &mut Context<'_, Self>) {
        let indexes: Vec<usize> = self
            .tree_selection_connection_indexes()
            .into_iter()
            .filter(|index| {
                matches!(
                    self.connections.get(*index).map(|node| &node.status),
                    Some(ConnectionStatus::Connected(_))
                        | Some(ConnectionStatus::Connecting)
                        | Some(ConnectionStatus::Failed(_))
                )
            })
            .collect();
        if indexes.is_empty() {
            return;
        }
        if indexes
            .iter()
            .any(|index| self.connection_has_unsaved(*index, cx))
        {
            self.unsaved_confirm = Some(PendingAction::DisconnectMany { indexes });
            cx.notify();
            return;
        }
        self.disconnect_many(indexes, cx);
    }

    /// Disconnect several connections in one pass.
    pub(super) fn disconnect_many(&mut self, indexes: Vec<usize>, cx: &mut Context<'_, Self>) {
        for index in indexes {
            self.disconnect(index, cx);
        }
    }

    /// Delete the connection tree's multi-selection: the selected groups (with everything still
    /// inside them) and the selected connections.
    pub(super) fn delete_tree_selection(
        &mut self,
        connections: Vec<String>,
        groups: Vec<u64>,
        cx: &mut Context<'_, Self>,
    ) {
        let mut ids: BTreeSet<String> = connections.into_iter().collect();
        // A selected group is removed wholesale, so its connections go with it.
        for group_id in &groups {
            groups::collect_subtree_connection_ids(&self.tree_layout, *group_id, &mut ids);
        }
        // Remove the groups first (their children are spliced out), then delete the connections by
        // profile id so index shifts never matter.
        for group_id in groups {
            self.delete_group(group_id, cx);
        }
        for id in ids {
            if let Some(index) = self
                .connections
                .iter()
                .position(|node| node.profile.id == id)
            {
                self.delete_connection(index, cx);
            }
        }
        self.tree_selection.clear();
        cx.notify();
    }

    pub(super) fn disconnect(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let connection = self.connection_arc(index);

        if let Some(node) = self.connections.get_mut(index) {
            node.status = ConnectionStatus::Disconnected;
            node.databases = Loadable::Idle;
            node.expanded = false;
        }

        // The object pane is scoped to one connection's database; a pane left pointing at this now
        // closed connection is stale, so clear it instead of rendering an empty list.
        if self
            .object_pane
            .as_ref()
            .is_some_and(|pane| pane.read(cx).connection_index == index)
        {
            self.object_pane = None;
        }

        // The Users tab lists one connection's accounts; a closed connection's list is dead data,
        // so forget it and let the next render re-read it for whatever is selected now.
        if self.users_connection == Some(index) {
            self.users_connection = None;
            self.users = Loadable::Idle;
            self.selected_user = None;
            self.info_user = Loadable::Idle;
        }

        if let Some(connection) = connection {
            self.close_connection_grids(&connection, cx);
            self.close_connection_designs(&connection, cx);
            // The Users feature's windows are bound to this connection: close them with it rather
            // than leave a window whose account list and grants can no longer be read or saved.
            self.close_create_user_for_connection(index, cx);
            self.close_privilege_manager_for_connection(&connection, cx);
            let runtime = self.runtime.clone();
            cx.spawn(async move |_this, _cx| {
                let _ = runtime.spawn(async move { connection.close().await }).await;
            })
            .detach();
        }

        self.notify_object_pane(cx);
        cx.notify();
    }

    pub(super) fn load_databases(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(connection) = self.connection_arc(index) else {
            return;
        };

        // Take the loaded list out *before* marking the node as `Loading`. The merge below keeps the
        // open/expanded state (and the already-loaded tables) of the databases that survive the
        // refresh; overwriting the list with `Loading` first would discard it and close every
        // database the user had open.
        let previous = match self.connections.get_mut(index) {
            Some(node) => match std::mem::replace(&mut node.databases, Loadable::Loading) {
                Loadable::Loaded(databases) => databases,
                _ => Vec::new(),
            },
            None => Vec::new(),
        };

        // 刷新 must re-fetch the objects of every database whose tables/routines were already
        // loaded: the merge below keeps `existing.tables`/`existing.routines`, so without this a
        // table dropped outside the app would stay in the tree and in the object list forever (the
        // node would simply look unchanged). Databases that were never opened keep their `Idle`
        // cache and load lazily.
        let reload_objects: Vec<(String, bool, bool)> = previous
            .iter()
            .filter_map(|database| {
                let tables = matches!(database.tables, Loadable::Loaded(_) | Loadable::Failed(_));
                let routines =
                    matches!(database.routines, Loadable::Loaded(_) | Loadable::Failed(_));
                (tables || routines).then(|| (database.name.clone(), tables, routines))
            })
            .collect();

        let runtime = self.runtime.clone();
        let objects_connection = connection.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.list_databases().await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, move |view, cx| {
                let profile_id = view
                    .connections
                    .get(index)
                    .map(|node| node.profile.id.clone())
                    .unwrap_or_default();
                let remembered: Vec<String> = view
                    .remembered_db_expansion
                    .get(&profile_id)
                    .cloned()
                    .unwrap_or_default();
                // The object lists to re-fetch once the merged node is in place (see
                // `reload_objects`): the database's position in the new list, its name, and which
                // of its lists were loaded before.
                let mut reload: Vec<(usize, String, bool, bool)> = Vec::new();
                let refresh_ok = result.is_ok();
                if let Some(node) = view.connections.get_mut(index) {
                    // `previous` was taken before the node went `Loading`, so the databases that
                    // survive the refresh keep their open/expanded state and loaded tables.
                    let mut previous = previous;
                    // A connection may be limited to a subset of its databases; an empty list means
                    // every database is shown.
                    let visible = node.profile.settings.visible_databases.clone();
                    node.databases = match result {
                        Ok(databases) => {
                            let merged: Vec<DatabaseNode> = databases
                                .into_iter()
                                .filter(|database| {
                                    visible.is_empty()
                                        || visible.iter().any(|name| name == &database.name)
                                })
                                .map(|database| {
                                    if let Some(position) = previous
                                        .iter()
                                        .position(|existing| existing.name == database.name)
                                    {
                                        let existing = previous.remove(position);
                                        DatabaseNode {
                                            name: database.name,
                                            tables: existing.tables,
                                            table_statuses: existing.table_statuses,
                                            routines: existing.routines,
                                            opened: existing.opened,
                                            expanded: existing.expanded,
                                            categories: existing.categories,
                                            expanded_schemas: existing.expanded_schemas,
                                            opened_schemas: existing.opened_schemas,
                                            schemas: existing.schemas,
                                            object_cursor: existing.object_cursor,
                                            object_pattern: existing.object_pattern,
                                            namespaces: existing.namespaces,
                                        }
                                    } else {
                                        let expanded = remembered.contains(&database.name);
                                        DatabaseNode {
                                            name: database.name,
                                            tables: Loadable::Idle,
                                            table_statuses: Loadable::Idle,
                                            routines: Loadable::Idle,
                                            opened: false,
                                            expanded,
                                            categories: Default::default(),
                                            expanded_schemas: Default::default(),
                                            opened_schemas: Default::default(),
                                            schemas: None,
                                            object_cursor: None,
                                            object_pattern: None,
                                            namespaces: Loadable::Idle,
                                        }
                                    }
                                })
                                .collect();
                            // Re-fetch the objects of the databases this refresh kept from
                            // `previous`, so 刷新 actually reloads what the tree and the object
                            // list show instead of replaying the stale cache.
                            for (position, database) in merged.iter().enumerate() {
                                if let Some((_, tables, routines)) = reload_objects
                                    .iter()
                                    .find(|(name, _, _)| name == &database.name)
                                {
                                    reload.push((
                                        position,
                                        database.name.clone(),
                                        *tables,
                                        *routines,
                                    ));
                                }
                            }
                            Loadable::Loaded(merged)
                        }
                        Err(error) => Loadable::Failed(error.to_string()),
                    };
                }
                view.completion_generation = view.completion_generation.wrapping_add(1);
                view.remember_databases(index);
                // Reopen the last session's databases on this connection.
                view.restore_session_databases(index, cx);
                // A SQLite file dropped onto the window (or passed on launch) opens its `main`
                // database as soon as the list arrives.
                if view.pending_open_databases.remove(&profile_id) {
                    view.open_first_database(index, cx);
                }
                view.notify_object_pane(cx);
                cx.notify();
                for (database_index, database_name, tables, routines) in reload {
                    if tables {
                        view.load_tables(
                            index,
                            database_index,
                            objects_connection.clone(),
                            database_name.clone(),
                            cx,
                        );
                    }
                    if routines {
                        view.load_routines(
                            index,
                            database_index,
                            objects_connection.clone(),
                            database_name,
                            cx,
                        );
                    }
                }
                view.finish_tree_refresh(index, refresh_ok, cx);
            });
        })
        .detach();
    }

    /// Open a database (double-click on its row, Enter, or the tree's 打开数据库 action): load its
    /// tables/routines/schemas and show its object browser; an already-open database toggles its
    /// expansion, exactly as before. A plain click that only selects the row goes through
    /// [`Self::show_database_objects`] instead, so it never loads or re-expands anything.
    pub(super) fn open_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        self.clear_tree_selection(cx);
        let mut should_load = false;
        let mut should_load_routines = false;
        let mut should_load_schemas = false;
        let mut just_opened = false;
        let supports_routines = self.driver_supports(connection_index, DriverCapability::Routines);
        let supports_schemas = self.driver_supports(connection_index, DriverCapability::Schemas);
        let supports_key_value = self.driver_supports(connection_index, DriverCapability::KeyValue);
        // Do not leave a stale Functions tab selected on an engine without routines.
        if !supports_routines && self.main_tab == MainTab::Functions {
            self.main_tab = MainTab::Tables;
        }

        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            if database.opened {
                database.expanded = !database.expanded;
            } else {
                database.opened = true;
                database.expanded = true;
                just_opened = true;
                if matches!(database.tables, Loadable::Idle | Loadable::Failed(_)) {
                    should_load = true;
                }
                if supports_key_value
                    && matches!(database.namespaces, Loadable::Idle | Loadable::Failed(_))
                {
                    should_load = true;
                }
                if supports_routines
                    && self.main_tab == MainTab::Functions
                    && matches!(database.routines, Loadable::Idle | Loadable::Failed(_))
                {
                    should_load_routines = true;
                }
                if supports_schemas
                    && (database.schemas.is_none()
                        || matches!(database.schemas, Some(Loadable::Idle | Loadable::Failed(_))))
                {
                    should_load_schemas = true;
                }
            }
        }

        if should_load
            && let Some(connection) = self.connection_arc(connection_index)
            && let Some(database_name) = self.database_name(connection_index, database_index)
        {
            self.load_tables(
                connection_index,
                database_index,
                connection,
                database_name,
                cx,
            );
        }

        if should_load_routines
            && let Some(connection) = self.connection_arc(connection_index)
            && let Some(database_name) = self.database_name(connection_index, database_index)
        {
            self.load_routines(
                connection_index,
                database_index,
                connection,
                database_name,
                cx,
            );
        }

        if should_load_schemas
            && let Some(connection) = self.connection_arc(connection_index)
            && let Some(database_name) = self.database_name(connection_index, database_index)
        {
            self.load_schemas(
                connection_index,
                database_index,
                connection,
                database_name,
                cx,
            );
        }

        if just_opened {
            if supports_schemas {
                // A schema-qualified engine shows objects only once a schema is opened: opening
                // the database just reveals its schemas in the tree and clears the content pane.
                self.object_pane = None;
                self.clear_object_search_unless_kept(cx);
                self.active_grid = None;
                self.active_query = None;
                self.active_design = None;
            } else {
                let existing = self.object_pane.as_ref().map(|pane| {
                    let pane = pane.read(cx);
                    (pane.connection_index, pane.database_index, pane.category)
                });
                let category = match existing {
                    Some((pane_connection, pane_database, category))
                        if pane_connection == connection_index
                            && pane_database == database_index =>
                    {
                        category
                    }
                    _ => match self.main_tab {
                        MainTab::Views => Category::Views,
                        MainTab::Functions if supports_routines => Category::Functions,
                        MainTab::Queries => Category::Queries,
                        _ => Category::Tables,
                    },
                };
                self.open_object_pane(connection_index, database_index, None, category, cx);
                self.clear_object_search_unless_kept(cx);
                self.active_grid = None;
                self.active_query = None;
                self.active_design = None;
            }
        }

        self.persist_settings();
        cx.notify();
    }

    /// A plain click on a database row. An already-open database shows its objects in the content
    /// pane, scoped to the active main tab (表/视图/函数); a closed database is only highlighted —
    /// opening stays on the double-click ([`Self::open_database`]).
    pub(super) fn show_database_objects(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        self.clear_tree_selection(cx);
        let opened = matches!(
            self.connections
                .get(connection_index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => databases.get(database_index),
                    _ => None,
                }),
            Some(database) if database.opened
        );
        if opened {
            self.show_objects_for_scope(connection_index, database_index, None, cx);
        }
    }

    /// A plain click on a schema row: an already-open schema shows its objects for the active main
    /// tab; a closed one is only highlighted (the double-click opens it).
    pub(super) fn show_schema_objects(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: String,
        cx: &mut Context<'_, Self>,
    ) {
        self.clear_tree_selection(cx);
        let opened = self
            .connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index),
                _ => None,
            })
            .is_some_and(|database| database.opened_schemas.contains(&schema));
        if opened {
            self.show_objects_for_scope(connection_index, database_index, Some(schema), cx);
        }
    }

    /// Show a database's (or schema's) objects in the content pane, scoped to the active main tab.
    /// Only the object-list tabs (表/视图/函数) do this — 用户/查询/备份 own the pane themselves and
    /// are re-scoped by the tree selection, so they are left alone here. An object pane already
    /// showing the same scope is reused (its scroll and selection survive) and just re-targeted to
    /// the new category; otherwise the pane is rebuilt for the scope.
    fn show_objects_for_scope(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let mut category = match self.main_tab {
            MainTab::Tables => Category::Tables,
            MainTab::Views => Category::Views,
            MainTab::Functions => Category::Functions,
            _ => return,
        };
        // Never leave the pane on a category the engine does not have.
        if category == Category::Functions
            && !self.driver_supports(connection_index, DriverCapability::Routines)
        {
            category = Category::Tables;
        }
        self.show_object_category(connection_index, database_index, schema, category, cx);
        if category == Category::Functions {
            self.ensure_routines_loaded(connection_index, database_index, cx);
        }
        self.active_grid = None;
        self.active_query = None;
        self.active_design = None;
        cx.notify();
    }

    /// Point the content pane at `category` for a scope. A pane already showing that scope is
    /// reused — its scroll position, remembered layout and selection survive — and only the
    /// category changes; any other scope builds a new pane. Rebuilding on every category click
    /// wiped that state (and made the 平铺网格 lay itself out again from an unmeasured viewport),
    /// which is what made the list visibly jump.
    pub(super) fn show_object_category(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: Option<String>,
        category: Category,
        cx: &mut Context<'_, Self>,
    ) {
        let existing = self.object_pane.as_ref().map(|pane| {
            let pane = pane.read(cx);
            (
                pane.connection_index,
                pane.database_index,
                pane.schema.clone(),
                pane.category,
            )
        });
        match existing {
            Some((pane_connection, pane_database, pane_schema, pane_category))
                if pane_connection == connection_index
                    && pane_database == database_index
                    && pane_schema == schema =>
            {
                if pane_category != category
                    && let Some(pane) = self.object_pane.as_ref()
                {
                    pane.update(cx, |pane, cx| {
                        pane.category = category;
                        pane.selected = None;
                        pane.selected_routine = None;
                        // The new category is a different list: start it at its own beginning
                        // instead of keeping the previous list's scroll offset.
                        pane.grid.reset();
                        pane.detail_scroll.reset();
                        cx.notify();
                    });
                    self.rename_edit = None;
                    self.rename_blur = None;
                    self.saved_query_selected = None;
                    self.query_selection.clear();
                    self.objects_selection.clear();
                    self.objects_row_rects.clear();
                }
            }
            _ => {
                self.open_object_pane(connection_index, database_index, schema, category, cx);
                self.clear_object_search_unless_kept(cx);
            }
        }
    }

    /// Create (or replace) the object pane for a database and return the entity handle.
    pub(super) fn open_object_pane(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: Option<String>,
        category: Category,
        cx: &mut Context<'_, Self>,
    ) {
        let app = cx.weak_entity();
        let theme = self.theme;
        // A new pane's scroller has no bounds until its first paint. Seed it with the height the
        // list it replaces was showing, so the 平铺网格 wraps its columns correctly on the first
        // frame instead of laying out one over-tall column and reflowing later.
        let grid_viewport = self
            .object_pane
            .as_ref()
            .map(|pane| pane.read(cx).grid.viewport_height())
            .unwrap_or_default();
        self.rename_edit = None;
        self.rename_blur = None;
        self.saved_query_selected = None;
        self.query_selection.clear();
        self.clear_info_selection();
        self.objects_selection.clear();
        self.objects_row_rects.clear();
        let key_value = self.driver_supports(connection_index, DriverCapability::KeyValue);
        self.object_pane = Some(cx.new(|cx| {
            ObjectPane::new(
                app,
                connection_index,
                database_index,
                schema,
                category,
                key_value,
                theme,
                grid_viewport,
                cx,
            )
        }));
        self.object_search_input.update(cx, |input, cx| {
            input.set_placeholder(
                if key_value {
                    t!("redis.key.search").to_string()
                } else {
                    t!("object.search").to_string()
                },
                cx,
            );
        });
        // Nothing has ever measured a grid in this window yet: one deferred repaint lets the
        // pane's first scroller report its height and the rows settle before the user sees a
        // wrong column split.
        if grid_viewport <= 0.0 {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(32))
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    pub(super) fn toggle_category(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: Option<String>,
        category: Category,
        cx: &mut Context<'_, Self>,
    ) {
        // A category the engine does not have (SQLite functions) is never opened.
        if category == Category::Functions
            && !self.driver_supports(connection_index, DriverCapability::Routines)
        {
            return;
        }
        if category == Category::Backups && !self.driver_supports_backup(connection_index) {
            return;
        }
        self.clear_tree_selection(cx);
        self.clear_info_selection();
        let mut should_load = false;
        let mut should_load_routines = false;
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.categories.toggle(category);
            if matches!(category, Category::Tables | Category::Views)
                && matches!(database.tables, Loadable::Idle | Loadable::Failed(_))
            {
                should_load = true;
            }
            if category == Category::Functions
                && matches!(database.routines, Loadable::Idle | Loadable::Failed(_))
            {
                should_load_routines = true;
            }
        }

        if should_load
            && let Some(connection) = self.connection_arc(connection_index)
            && let Some(name) = self.database_name(connection_index, database_index)
        {
            self.load_tables(connection_index, database_index, connection, name, cx);
        }

        if should_load_routines
            && let Some(connection) = self.connection_arc(connection_index)
            && let Some(name) = self.database_name(connection_index, database_index)
        {
            self.load_routines(connection_index, database_index, connection, name, cx);
        }

        if category == Category::Backups {
            // Selecting the Backups category scopes the Backup main tab to this database and
            // expands its leaves in the tree. There is no object-pane tab for backups: the main
            // tab's own list is the content pane.
            self.active_grid = None;
            self.active_query = None;
            self.active_design = None;
            self.saved_query_selected = None;
            self.query_selection.clear();
            self.main_tab = MainTab::Backups;
            self.refresh_backups(cx);
            cx.notify();
            return;
        }

        self.show_object_category(connection_index, database_index, schema, category, cx);
        self.clear_object_search_unless_kept(cx);
        self.active_grid = None;
        self.active_query = None;
        self.active_design = None;
        match category {
            Category::Tables => self.main_tab = MainTab::Tables,
            Category::Views => self.main_tab = MainTab::Views,
            Category::Functions => self.main_tab = MainTab::Functions,
            Category::Queries => self.main_tab = MainTab::Queries,
            _ => {}
        }
        cx.notify();
    }

    /// Expand or collapse one schema node under a database (SQL Server).
    pub(super) fn toggle_schema(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
            && !database.expanded_schemas.insert(schema.clone())
        {
            database.expanded_schemas.remove(&schema);
        }
        cx.notify();
    }

    /// Handle a double-click on a schema tree row: open the schema (highlight it, expand it and show
    /// its objects) when it is not already showing, otherwise collapse/expand it like a plain node.
    pub(super) fn select_schema(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: String,
        cx: &mut Context<'_, Self>,
    ) {
        self.clear_tree_selection(cx);
        let already_open = self.object_pane.as_ref().is_some_and(|pane| {
            let pane = pane.read(cx);
            pane.connection_index == connection_index
                && pane.database_index == database_index
                && pane.schema.as_deref() == Some(schema.as_str())
        });
        if already_open {
            self.toggle_schema(connection_index, database_index, schema, cx);
        } else {
            self.open_schema(connection_index, database_index, schema, cx);
        }
    }

    /// Open a schema (SQL Server): highlight its tree node, load the database's objects if needed
    /// and show the schema's objects in the content pane.
    pub(super) fn open_schema(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: String,
        cx: &mut Context<'_, Self>,
    ) {
        let mut should_load = false;
        let mut should_load_routines = false;
        let mut should_load_schemas = false;
        let supports_routines = self.driver_supports(connection_index, DriverCapability::Routines);
        let supports_schemas = self.driver_supports(connection_index, DriverCapability::Schemas);

        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.opened = true;
            database.expanded = true;
            database.opened_schemas.insert(schema.clone());
            database.expanded_schemas.insert(schema.clone());
            if matches!(database.tables, Loadable::Idle | Loadable::Failed(_)) {
                should_load = true;
            }
            if supports_routines
                && matches!(database.routines, Loadable::Idle | Loadable::Failed(_))
            {
                should_load_routines = true;
            }
            if supports_schemas
                && (database.schemas.is_none()
                    || matches!(database.schemas, Some(Loadable::Idle | Loadable::Failed(_))))
            {
                should_load_schemas = true;
            }
        }

        if should_load
            && let Some(connection) = self.connection_arc(connection_index)
            && let Some(database_name) = self.database_name(connection_index, database_index)
        {
            self.load_tables(
                connection_index,
                database_index,
                connection,
                database_name,
                cx,
            );
        }
        if should_load_routines
            && let Some(connection) = self.connection_arc(connection_index)
            && let Some(database_name) = self.database_name(connection_index, database_index)
        {
            self.load_routines(
                connection_index,
                database_index,
                connection,
                database_name,
                cx,
            );
        }
        if should_load_schemas
            && let Some(connection) = self.connection_arc(connection_index)
            && let Some(database_name) = self.database_name(connection_index, database_index)
        {
            self.load_schemas(
                connection_index,
                database_index,
                connection,
                database_name,
                cx,
            );
        }

        let category = match self.main_tab {
            MainTab::Views => Category::Views,
            MainTab::Functions if supports_routines => Category::Functions,
            MainTab::Queries => Category::Queries,
            _ => Category::Tables,
        };
        self.show_object_category(connection_index, database_index, Some(schema), category, cx);
        self.active_grid = None;
        self.active_query = None;
        self.active_design = None;
        cx.notify();
    }

    /// Close a schema (SQL Server): un-highlight it, collapse it and clear the content pane when it
    /// was showing this schema.
    pub(super) fn close_schema(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.opened_schemas.remove(&schema);
            database.expanded_schemas.remove(&schema);
        }
        let showing_schema = self.object_pane.as_ref().is_some_and(|pane| {
            let pane = pane.read(cx);
            pane.connection_index == connection_index
                && pane.database_index == database_index
                && pane.schema.as_deref() == Some(schema.as_str())
        });
        if showing_schema {
            self.object_pane = None;
        }
        cx.notify();
    }

    pub(super) fn database_name(
        &self,
        connection_index: usize,
        database_index: usize,
    ) -> Option<String> {
        self.connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => {
                    databases.get(database_index).map(|db| db.name.clone())
                }
                _ => None,
            })
    }

    /// The index of a database by name, for resolving an info-pane table selection.
    pub(super) fn database_index_by_name(
        &self,
        connection_index: usize,
        name: &str,
    ) -> Option<usize> {
        self.connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => {
                    databases.iter().position(|database| database.name == name)
                }
                _ => None,
            })
    }

    /// Select a key-value namespace from the tree: filter the key list and bring it to the front.
    pub(super) fn select_key_namespace(
        &mut self,
        connection_index: usize,
        database_index: usize,
        namespace: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.main_tab = MainTab::Tables;
        let selected = match &namespace {
            Some(name) => format!("keys-{connection_index}-{database_index}-{name}"),
            None => format!("db-{connection_index}-{database_index}"),
        };
        self.tree_pane.update(cx, |tree, cx| {
            tree.selected = Some(selected);
            tree.selected_table = None;
            cx.notify();
        });
        self.set_object_namespace(connection_index, database_index, namespace, cx);
        self.show_database_objects(connection_index, database_index, cx);
    }

    /// Sets (or clears) the object list's namespace filter and reloads the first page.
    pub(super) fn set_object_namespace(
        &mut self,
        connection_index: usize,
        database_index: usize,
        namespace: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let pattern = namespace
            .map(|namespace| format!("{namespace}*"))
            .filter(|pattern| !pattern.is_empty());
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.object_pattern = pattern;
        }
        self.reload_tables(connection_index, database_index, cx);
    }

    pub(super) fn load_tables(
        &mut self,
        connection_index: usize,
        database_index: usize,
        connection: Arc<dyn Connection>,
        database_name: String,
        cx: &mut Context<'_, Self>,
    ) {
        let key_value = self.driver_supports(connection_index, DriverCapability::KeyValue);
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.tables = Loadable::Loading;
            database.table_statuses = Loadable::Loading;
            database.object_cursor = None;
            if key_value && matches!(database.namespaces, Loadable::Idle | Loadable::Failed(_)) {
                database.namespaces = Loadable::Loading;
            }
        }

        // The namespace filter (key-value engines) narrows the listing server-side.
        let pattern =
            self.connections
                .get(connection_index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => databases
                        .get(database_index)
                        .and_then(|database| database.object_pattern.clone()),
                    _ => None,
                });

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            // The key-value namespace list (a full bounded SCAN) runs alongside the first page.
            let namespaces_handle = key_value.then(|| {
                let connection = connection.clone();
                let database = database_name.clone();
                runtime.spawn(async move { connection.list_namespaces(&database).await })
            });

            // The table list is published first: the default 平铺网格 does not show any of the
            // overview columns, and the per-table `COUNT(*)` pass behind `table_statuses` can take
            // seconds on a large database. Waiting for both made the list appear only after the
            // counts had finished.
            let list_connection = connection.clone();
            let list_database = database_name.clone();
            let list_pattern = pattern.clone();
            let tables = match runtime
                .spawn(async move {
                    list_connection
                        .list_objects_page(&list_database, None, list_pattern.as_deref())
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let failed = tables.is_err();
            let _ = this.update(cx, |view, cx| {
                if let Some(node) = view.connections.get_mut(connection_index)
                    && let Loadable::Loaded(databases) = &mut node.databases
                    && let Some(database) = databases.get_mut(database_index)
                {
                    match tables {
                        Ok((tables, cursor)) => {
                            database.tables = Loadable::Loaded(tables);
                            database.object_cursor = cursor;
                        }
                        Err(error) => {
                            database.tables = Loadable::Failed(error.to_string());
                            database.table_statuses = Loadable::Failed(error.to_string());
                        }
                    }
                }
                view.completion_generation = view.completion_generation.wrapping_add(1);
                view.refresh_completion_catalog();
                view.notify_object_pane(cx);
                view.finish_object_refresh(connection_index, database_index, !failed, cx);
                cx.notify();
                // The object list's scroll extents are only known after the
                // frame's paint, so schedule one more frame to reveal the
                // horizontal scrollbar without waiting for a hover/resize.
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(32))
                        .await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            });
            if failed {
                return;
            }

            if let Some(handle) = namespaces_handle {
                let namespaces = match handle.await {
                    Ok(Ok(namespaces)) => Loadable::Loaded(namespaces),
                    Ok(Err(error)) => Loadable::Failed(error.to_string()),
                    Err(error) => Loadable::Failed(error.to_string()),
                };
                let _ = this.update(cx, |view, cx| {
                    if let Some(node) = view.connections.get_mut(connection_index)
                        && let Loadable::Loaded(databases) = &mut node.databases
                        && let Some(database) = databases.get_mut(database_index)
                    {
                        database.namespaces = namespaces;
                    }
                    cx.notify();
                });
            }

            // The overview columns fill in when the extra query returns; the list itself is
            // already on screen and stays interactive meanwhile.
            let statuses = runtime
                .spawn(async move {
                    connection
                        .table_statuses(&database_name)
                        .await
                        .unwrap_or_default()
                })
                .await
                .unwrap_or_default();
            let _ = this.update(cx, |view, cx| {
                if let Some(node) = view.connections.get_mut(connection_index)
                    && let Loadable::Loaded(databases) = &mut node.databases
                    && let Some(database) = databases.get_mut(database_index)
                {
                    database.table_statuses = Loadable::Loaded(statuses);
                }
                view.notify_object_pane(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Load a database's stored routines (functions and procedures) into its node.
    pub(super) fn load_routines(
        &mut self,
        connection_index: usize,
        database_index: usize,
        connection: Arc<dyn Connection>,
        database_name: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.routines = Loadable::Loading;
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.list_routine_infos(&database_name).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                let routines_ok = result.is_ok();
                if let Some(node) = view.connections.get_mut(connection_index)
                    && let Loadable::Loaded(databases) = &mut node.databases
                    && let Some(database) = databases.get_mut(database_index)
                {
                    database.routines = match result {
                        Ok(routines) => Loadable::Loaded(routines),
                        Err(error) => Loadable::Failed(error.to_string()),
                    };
                }
                view.notify_object_pane(cx);
                view.finish_object_refresh(connection_index, database_index, routines_ok, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-fetch a database's routines after one was created or dropped, locating it by the live
    /// connection rather than an index the routine editor does not carry.
    pub(super) fn reload_routines(
        &mut self,
        connection: &Arc<dyn Connection>,
        database: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection_index) = self.connections.iter().position(|node| {
            matches!(
                &node.status,
                ConnectionStatus::Connected(active) if Arc::ptr_eq(active, connection)
            )
        }) else {
            return;
        };
        let database_index =
            self.connections
                .get(connection_index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => databases
                        .iter()
                        .position(|existing| existing.name == database),
                    _ => None,
                });
        if let Some(database_index) = database_index
            && let Some(connection) = self.connection_arc(connection_index)
        {
            self.load_routines(
                connection_index,
                database_index,
                connection,
                database.to_string(),
                cx,
            );
        }
    }

    pub(super) fn select_table(
        &mut self,
        connection_index: usize,
        database: String,
        table: String,
        is_view: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };

        // The info pane follows the selected table.
        if let Some(database_index) = self.database_index_by_name(connection_index, &database) {
            self.set_info_table(connection_index, database_index, table.clone());
        }

        if let Some(index) = self.grids.iter().position(|grid| {
            let grid = grid.read(cx);
            // Only reuse a real table grid; a query result grid also carries the inferred
            // database/table (for editing) but shows no tab, so matching it would open the
            // table with no tab at all.
            grid.state.sql.is_none()
                && grid.state.database == database
                && grid.state.table == table
                && Arc::ptr_eq(&grid.state.connection, &connection)
        }) {
            self.activate_grid(Some(index), cx);
            return;
        }

        let connection_name = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let connection_id = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.id.clone())
            .unwrap_or_default();
        let key_status = if self.driver_supports(connection_index, DriverCapability::KeyValue) {
            self.database_index_by_name(connection_index, &database)
                .and_then(|database_index| {
                    self.connections
                        .get(connection_index)
                        .and_then(|node| match &node.databases {
                            Loadable::Loaded(databases) => databases.get(database_index),
                            _ => None,
                        })
                })
                .and_then(|database| match &database.table_statuses {
                    Loadable::Loaded(statuses) => statuses
                        .iter()
                        .find(|(name, _)| name == &table)
                        .map(|(_, status)| status.clone()),
                    _ => None,
                })
        } else {
            None
        };
        let layout = self.field_layout(&connection_id, &database, &table);
        let key_read_only = key_status
            .as_ref()
            .and_then(|status| status.engine.as_deref())
            .is_some_and(|kind| matches!(kind, "stream" | "json"));
        let id = self.next_grid_id;
        self.next_grid_id += 1;
        // For a key grid whose overview was not loaded yet (e.g. a restored session), fetch it
        // after the grid opens so the key header (type/TTL/memory) still appears.
        let status_connection = (key_status.is_none()
            && self.driver_supports(connection_index, DriverCapability::KeyValue))
        .then(|| connection.clone());
        let status_database = database.clone();
        let status_table = table.clone();

        let state = GridState {
            id,
            connection,
            dialect: self.driver_dialect(connection_index),
            connection_name,
            connection_id,
            database,
            table,
            is_view,
            key_status,
            page_index: 0,
            page_size: self.page_size,
            loading: false,
            error: None,
            columns: Vec::new(),
            all_columns: Vec::new(),
            field_layout: GridFieldLayout {
                order: layout.order,
                hidden: layout.hidden.into_iter().collect(),
            },
            column_order: Vec::new(),
            visible_columns: 0,
            frozen_columns: Vec::new(),
            rows: Arc::new(Vec::new()),
            column_widths: Vec::new(),
            manual_column_widths: false,
            total_rows: None,
            selection: None,
            edits: BTreeMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            sql: None,
            show_toolbar: true,
            show_footer: true,
            editable: !key_read_only,
            sort_rules: Vec::new(),
            sort_open: false,
            sort_draft: Vec::new(),
            sort_selected: None,
            filters: Vec::new(),
            filter_open: false,
            filter_draft: Vec::new(),
            elapsed: None,
        };
        let app = cx.weak_entity();
        let runtime = self.runtime.clone();
        let theme = self.theme;
        let clipboard = self.grid_clipboard.clone();
        let entity = cx.new(|cx| GridView::new(state, app, runtime, theme, clipboard, cx));
        self.grids.push(entity.clone());
        self.active_grid = Some(self.grids.len() - 1);
        entity.update(cx, |grid, cx| grid.load_page(cx));

        if let Some(connection) = status_connection {
            let grid = entity.clone();
            let runtime = self.runtime.clone();
            cx.spawn(async move |_this, cx| {
                let status = runtime
                    .spawn(async move {
                        connection
                            .table_status(&status_database, &status_table)
                            .await
                    })
                    .await;
                if let Ok(Ok(status)) = status {
                    grid.update(cx, |grid, cx| {
                        if grid.state.key_status.is_none() {
                            grid.state.editable = !status
                                .engine
                                .as_deref()
                                .is_some_and(|kind| matches!(kind, "stream" | "json"));
                            grid.state.key_status = Some(status);
                            cx.notify();
                        }
                    });
                }
            })
            .detach();
        }

        cx.notify();
    }

    pub(super) fn activate_grid(&mut self, index: Option<usize>, cx: &mut Context<'_, Self>) {
        self.active_grid = index;
        self.active_query = None;
        self.active_design = None;
        // The grid takes the keyboard focus, so arrows / Tab work right after a tab switch.
        self.grid_focus_pending = index.is_some();
        // The toolbar section follows the tab: Tables for a table, Views for a view.
        let object_tab = index
            .and_then(|index| self.grids.get(index))
            .and_then(|grid| {
                let grid = grid.read(cx);
                if grid.state.sql.is_some() {
                    return None;
                }
                Some((
                    grid.state.is_view,
                    grid.state.connection.clone(),
                    grid.state.database.clone(),
                ))
            });
        if let Some((is_view, connection, database)) = object_tab {
            self.main_tab = if is_view {
                MainTab::Views
            } else {
                MainTab::Tables
            };
            // Bring the connection tree to the tab's database: with tables open from several
            // databases, the highlight must follow the tab on screen rather than stay where the
            // last tree click left it.
            if let Some(connection_index) = self.connection_index_of(&connection) {
                if let Some(node) = self.connections.get_mut(connection_index) {
                    node.expanded = true;
                }
                if let Some(database_index) =
                    self.database_index_by_name(connection_index, &database)
                {
                    self.tree_selection.clear();
                    self.tree_pane.update(cx, |pane, cx| {
                        pane.select_database(connection_index, database_index, cx);
                    });
                }
            }
        }
        cx.notify();
    }

    /// Run a confirmed destructive table operation and refresh the object list afterwards.
    pub(super) fn run_table_operation(
        &mut self,
        connection_index: usize,
        database_index: usize,
        name: String,
        operation: TableOperation,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };

        let runtime = self.runtime.clone();
        let close_connection = connection.clone();
        let close_database = database.clone();
        let close_name = name.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    match operation {
                        TableOperation::Empty => connection.empty_table(&database, &name).await,
                        TableOperation::Truncate => {
                            connection.truncate_table(&database, &name).await
                        }
                    }
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(()) => {
                        // The table is still there but empty: re-fetch the open grids.
                        view.refresh_table_grids(
                            &close_connection,
                            &close_database,
                            &close_name,
                            cx,
                        );
                        view.reload_tables(connection_index, database_index, cx);
                    }
                    Err(error) => view.error_dialog = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Empty or truncate every selected table in one pass. Like the multi-drop path, the first
    /// failure is surfaced but the remaining tables are still attempted.
    pub(super) fn run_table_operations(
        &mut self,
        connection_index: usize,
        database_index: usize,
        names: Vec<String>,
        operation: TableOperation,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };

        let runtime = self.runtime.clone();
        let close_connection = connection.clone();
        let close_database = database.clone();
        let operation_names = names.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    let mut failures = Vec::new();
                    for name in &operation_names {
                        let result = match operation {
                            TableOperation::Empty => connection.empty_table(&database, name).await,
                            TableOperation::Truncate => {
                                connection.truncate_table(&database, name).await
                            }
                        };
                        if let Err(error) = result {
                            failures.push(format!("{name}: {error}"));
                        }
                    }
                    Ok::<Vec<String>, Error>(failures)
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(failures) => {
                        for name in &names {
                            view.refresh_table_grids(&close_connection, &close_database, name, cx);
                        }
                        if !failures.is_empty() {
                            view.error_dialog = Some(failures.join("\n"));
                        }
                        view.reload_tables(connection_index, database_index, cx);
                    }
                    Err(error) => view.error_dialog = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Drop several selected tables in one pass, then refresh the object list. The first failure is
    /// surfaced but the remaining tables are still attempted, so a multi-drop is not all-or-nothing.
    pub(super) fn run_drop_tables(
        &mut self,
        connection_index: usize,
        database_index: usize,
        names: Vec<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };

        let runtime = self.runtime.clone();
        let close_connection = connection.clone();
        let close_database = database.clone();
        let drop_names = names.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    let mut failures = Vec::new();
                    for name in &drop_names {
                        if let Err(error) = connection.drop_table(&database, name).await {
                            failures.push(format!("{name}: {error}"));
                        }
                    }
                    Ok::<Vec<String>, Error>(failures)
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(failures) => {
                        for name in &names {
                            view.close_table_views(&close_connection, &close_database, name, cx);
                        }
                        if !failures.is_empty() {
                            view.error_dialog = Some(failures.join("\n"));
                        }
                        view.reload_tables(connection_index, database_index, cx);
                    }
                    Err(error) => view.error_dialog = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Rename a table (or view) and refresh the object list.
    pub(super) fn rename_table(
        &mut self,
        connection_index: usize,
        database_index: usize,
        old_name: String,
        new_name: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };

        let runtime = self.runtime.clone();
        let close_connection = connection.clone();
        let close_database = database.clone();
        let close_old_name = old_name.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    connection
                        .rename_table(&database, &old_name, &new_name)
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(()) => {
                        // The old table no longer exists under its former name.
                        view.close_table_views(
                            &close_connection,
                            &close_database,
                            &close_old_name,
                            cx,
                        );
                        view.reload_tables(connection_index, database_index, cx);
                    }
                    Err(error) => view.error_dialog = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-fetch a database's table list, e.g. after a table was dropped or renamed.
    pub(super) fn reload_tables(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        self.load_tables(connection_index, database_index, connection, database, cx);
    }

    /// Reload the objects behind the object list's current category (the toolbar refresh action).
    pub(super) fn refresh_object_category(
        &mut self,
        category: Category,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        self.refresh_object_feedback = Some((connection_index, database_index));
        self.refresh_toast(ToastKind::Info, t!("grid.refreshing").to_string(), cx);
        if category == Category::Functions {
            if let (Some(connection), Some(database)) = (
                self.connection_arc(connection_index),
                self.database_name(connection_index, database_index),
            ) {
                self.reload_routines(&connection, &database, cx);
            }
        } else {
            self.reload_tables(connection_index, database_index, cx);
        }
    }

    /// Report an explicit object-list refresh as finished, if it targeted this database.
    pub(super) fn finish_object_refresh(
        &mut self,
        connection_index: usize,
        database_index: usize,
        ok: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if self.refresh_object_feedback == Some((connection_index, database_index)) {
            self.refresh_object_feedback = None;
            let (kind, message) = if ok {
                (ToastKind::Success, t!("grid.refreshed").to_string())
            } else {
                (ToastKind::Error, t!("grid.refresh_failed").to_string())
            };
            self.refresh_toast(kind, message, cx);
        }
    }

    /// Close every grid and designer tab for one table (used when the table is dropped/renamed).
    pub(super) fn close_table_views(
        &mut self,
        connection: &Arc<dyn Connection>,
        database: &str,
        table: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let active_grid_id = self.active_grid_id(cx);
        self.grids.retain(|grid| {
            let grid = grid.read(cx);
            !(grid.state.sql.is_none()
                && grid.state.database == database
                && grid.state.table == table
                && Arc::ptr_eq(&grid.state.connection, connection))
        });
        self.active_grid = active_grid_id.and_then(|id| {
            self.grids
                .iter()
                .position(|grid| grid.read(cx).state.id == id)
        });

        let active_design_id = self
            .active_design
            .and_then(|index| self.designs.get(index))
            .map(|design| design.read(cx).id);
        self.designs.retain(|design| {
            let design = design.read(cx);
            !(design.database == database
                && design.table == table
                && Arc::ptr_eq(&design.connection, connection))
        });
        self.active_design = active_design_id.and_then(|id| {
            self.designs
                .iter()
                .position(|design| design.read(cx).id == id)
        });
    }
}
