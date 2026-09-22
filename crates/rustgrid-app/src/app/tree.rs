use super::*;

impl AppView {
    pub(super) fn connection_arc(&self, index: usize) -> Option<Arc<dyn Connection>> {
        match &self.connections.get(index)?.status {
            ConnectionStatus::Connected(connection) => Some(connection.clone()),
            _ => None,
        }
    }

    pub(super) fn toggle_connection(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let state = match self.connections.get(index).map(|node| &node.status) {
            Some(ConnectionStatus::Connected(_)) => 1,
            Some(ConnectionStatus::Connecting) => 2,
            _ => 0,
        };

        match state {
            1 => self.disconnect(index, cx),
            2 => {}
            _ => self.connect(index, cx),
        }
    }

    pub(super) fn toggle_expand(&mut self, index: usize) {
        if let Some(node) = self.connections.get_mut(index)
            && matches!(&node.status, ConnectionStatus::Connected(_))
        {
            node.expanded = !node.expanded;
        }
    }

    pub(super) fn open_new_form(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.editing = None;
        self.show_form(ConnectionForm::default(), window, cx);
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
        self.show_form(
            ConnectionForm::from_profile(&profile, password, password_saved),
            window,
            cx,
        );
    }

    fn show_form(&mut self, form: ConnectionForm, window: &mut Window, cx: &mut Context<'_, Self>) {
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
            ],
        };
        let name_focus = inputs.get(FormField::Name).read(cx).focus_handle();
        self.form_inputs = Some(inputs);
        self.form = Some(form);
        self.test_status = TestStatus::Idle;
        self.context_menu = None;
        window.focus(&name_focus, cx);
        cx.notify();
    }

    pub(super) fn delete_connection(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.connections.len() {
            return;
        }

        self.disconnect(index, cx);
        self.connections.remove(index);

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
        let _ = self.config.save_profiles(&profiles);
        self.persist_secrets();

        cx.notify();
    }

    pub(super) fn persist_secrets(&self) {
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
        let _ = self.config.save_secrets(&secrets);
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
            if let Some(node) = self.connections.get_mut(index) {
                node.status = ConnectionStatus::Failed(t!("error.driver_missing").to_string());
            }
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
                    }
                    Err(error) => {
                        let authentication = matches!(&error, Error::Authentication(_));
                        let message = error.to_string();

                        if let Some(node) = view.connections.get_mut(index) {
                            node.status = ConnectionStatus::Failed(message);
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
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn disconnect(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let connection = self.connection_arc(index);

        if let Some(node) = self.connections.get_mut(index) {
            node.status = ConnectionStatus::Disconnected;
            node.databases = Loadable::Idle;
            node.expanded = false;
        }

        if let Some(connection) = connection {
            self.close_connection_grids(&connection, cx);
            self.close_connection_designs(&connection, cx);
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

        if let Some(node) = self.connections.get_mut(index) {
            node.databases = Loadable::Loading;
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.list_databases().await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(node) = view.connections.get_mut(index) {
                    // Keep databases that were already open/expanded (and their loaded tables)
                    // so refreshing the list does not close the user's databases.
                    let previous = match std::mem::replace(&mut node.databases, Loadable::Idle) {
                        Loadable::Loaded(databases) => databases,
                        _ => Vec::new(),
                    };
                    node.databases = match result {
                        Ok(databases) => {
                            let mut previous = previous;
                            Loadable::Loaded(
                                databases
                                    .into_iter()
                                    .map(|database| {
                                        if let Some(position) = previous
                                            .iter()
                                            .position(|existing| existing.name == database.name)
                                        {
                                            let existing = previous.remove(position);
                                            DatabaseNode {
                                                name: database.name,
                                                tables: existing.tables,
                                                routines: existing.routines,
                                                opened: existing.opened,
                                                expanded: existing.expanded,
                                                categories: existing.categories,
                                            }
                                        } else {
                                            DatabaseNode {
                                                name: database.name,
                                                tables: Loadable::Idle,
                                                routines: Loadable::Idle,
                                                opened: false,
                                                expanded: false,
                                                categories: Default::default(),
                                            }
                                        }
                                    })
                                    .collect(),
                            )
                        }
                        Err(error) => Loadable::Failed(error.to_string()),
                    };
                }
                view.completion_generation = view.completion_generation.wrapping_add(1);
                view.notify_object_pane(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let mut should_load = false;
        let mut should_load_routines = false;
        let mut just_opened = false;

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
                if self.main_tab == MainTab::Functions
                    && matches!(database.routines, Loadable::Idle | Loadable::Failed(_))
                {
                    should_load_routines = true;
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

        if just_opened {
            let existing = self.object_pane.as_ref().map(|pane| {
                let pane = pane.read(cx);
                (pane.connection_index, pane.database_index, pane.category)
            });
            let category = match existing {
                Some((pane_connection, pane_database, category))
                    if pane_connection == connection_index && pane_database == database_index =>
                {
                    category
                }
                _ => match self.main_tab {
                    MainTab::Views => Category::Views,
                    MainTab::Functions => Category::Functions,
                    MainTab::Queries => Category::Queries,
                    _ => Category::Tables,
                },
            };
            self.open_object_pane(connection_index, database_index, category, cx);
            self.clear_object_search(cx);
            self.active_grid = None;
            self.active_query = None;
            self.active_design = None;
        }

        cx.notify();
    }

    /// Create (or replace) the object pane for a database and return the entity handle.
    fn open_object_pane(
        &mut self,
        connection_index: usize,
        database_index: usize,
        category: Category,
        cx: &mut Context<'_, Self>,
    ) {
        let app = cx.weak_entity();
        let theme = self.theme;
        self.rename_edit = None;
        self.rename_blur = None;
        self.saved_query_selected = None;
        self.object_pane =
            Some(cx.new(|cx| {
                ObjectPane::new(app, connection_index, database_index, category, theme, cx)
            }));
    }

    pub(super) fn toggle_category(
        &mut self,
        connection_index: usize,
        database_index: usize,
        category: Category,
        cx: &mut Context<'_, Self>,
    ) {
        self.clear_info_table();
        if category == Category::Backups {
            // Selecting the Backups category scopes the Backup main tab to this database. The
            // backup files themselves live in the middle pane, not as tree leaves.
            self.active_grid = None;
            self.active_query = None;
            self.active_design = None;
            self.saved_query_selected = None;
            self.main_tab = MainTab::Backups;
            self.refresh_backups(cx);
            cx.notify();
            return;
        }
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

        self.open_object_pane(connection_index, database_index, category, cx);
        self.clear_object_search(cx);
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

    pub(super) fn load_tables(
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
            database.tables = Loadable::Loading;
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.list_tables(&database_name).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(node) = view.connections.get_mut(connection_index)
                    && let Loadable::Loaded(databases) = &mut node.databases
                    && let Some(database) = databases.get_mut(database_index)
                {
                    database.tables = match result {
                        Ok(tables) => Loadable::Loaded(tables),
                        Err(error) => Loadable::Failed(error.to_string()),
                    };
                }
                view.completion_generation = view.completion_generation.wrapping_add(1);
                view.notify_object_pane(cx);
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
            grid.state.database == database
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
        let id = self.next_grid_id;
        self.next_grid_id += 1;

        let state = GridState {
            id,
            connection,
            connection_name,
            database,
            table,
            is_view,
            page_index: 0,
            page_size: self.page_size,
            loading: false,
            error: None,
            columns: Vec::new(),
            rows: Arc::new(Vec::new()),
            column_widths: Vec::new(),
            manual_column_widths: false,
            total_rows: None,
            selection: None,
            edits: BTreeMap::new(),
            undo: Vec::new(),
            sql: None,
            show_toolbar: true,
            editable: true,
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
        let entity = cx.new(|cx| GridView::new(state, app, runtime, theme, cx));
        self.grids.push(entity.clone());
        self.active_grid = Some(self.grids.len() - 1);
        entity.update(cx, |grid, cx| grid.load_page(cx));

        cx.notify();
    }

    pub(super) fn activate_grid(&mut self, index: Option<usize>, cx: &mut Context<'_, Self>) {
        self.active_grid = index;
        self.active_query = None;
        self.active_design = None;
        self.query_completion = None;
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
                        TableOperation::Drop => connection.drop_table(&database, &name).await,
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
                        match operation {
                            // The table is gone: close its grids/designer.
                            TableOperation::Drop => {
                                view.close_table_views(
                                    &close_connection,
                                    &close_database,
                                    &close_name,
                                    cx,
                                );
                            }
                            // Still there but empty: re-fetch the open grids.
                            TableOperation::Empty | TableOperation::Truncate => {
                                view.refresh_table_grids(
                                    &close_connection,
                                    &close_database,
                                    &close_name,
                                    cx,
                                );
                            }
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
