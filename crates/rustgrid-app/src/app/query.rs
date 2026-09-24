use super::backup::reveal_in_file_manager;
use super::*;

impl AppView {
    pub(super) fn default_query_connection(&self, cx: &Context<'_, Self>) -> Option<usize> {
        // A database picked in the connection tree wins, even before its connection is connected.
        if let Some((connection_index, _)) = self.tree_selected_database(cx) {
            return Some(connection_index);
        }
        if let Some(pane) = self.object_pane.as_ref() {
            let pane = pane.read(cx);
            if matches!(
                self.connections
                    .get(pane.connection_index)
                    .map(|node| &node.status),
                Some(ConnectionStatus::Connected(_))
            ) {
                return Some(pane.connection_index);
            }
        }
        self.connections
            .iter()
            .position(|node| matches!(node.status, ConnectionStatus::Connected(_)))
    }

    pub(super) fn default_query_database(
        &self,
        connection_index: usize,
        cx: &Context<'_, Self>,
    ) -> Option<String> {
        // A database picked in the connection tree is the current database, so a new query defaults
        // to it rather than to whatever the object pane last opened.
        if let Some((selected_connection, database_index)) = self.tree_selected_database(cx)
            && selected_connection == connection_index
            && let Some(name) = self.database_name(connection_index, database_index)
        {
            return Some(name);
        }
        if let Some(pane) = self.object_pane.as_ref() {
            let pane = pane.read(cx);
            if pane.connection_index == connection_index
                && let Some(name) = self.database_name(pane.connection_index, pane.database_index)
            {
                return Some(name);
            }
        }
        self.connections
            .get(connection_index)
            .and_then(|node| node.profile.database.clone())
            .filter(|name| !name.is_empty())
    }

    /// The database selected in the connection tree (`db-…` or the Queries category `cat-…-q`), as
    /// `(connection_index, database_index)`. `None` when any other row is selected.
    fn tree_selected_database(&self, cx: &App) -> Option<(usize, usize)> {
        let selected = self.tree_pane.read(cx).selected.clone()?;
        if let Some(rest) = selected.strip_prefix("db-") {
            let mut parts = rest.splitn(2, '-');
            return Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?));
        }
        if let Some(rest) = selected.strip_prefix("cat-") {
            let mut parts = rest.splitn(3, '-');
            let pair = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
            return (parts.next()? == "q").then_some(pair);
        }
        None
    }

    pub(super) fn open_new_query(&mut self, cx: &mut Context<'_, Self>) {
        let connection_index = self.default_query_connection(cx);
        let database = connection_index.and_then(|index| self.default_query_database(index, cx));
        self.open_query_with(connection_index, database, cx);
    }

    /// Open a query tab bound to a specific connection, defaulting to its current database.
    pub(super) fn open_new_query_for_connection(
        &mut self,
        connection_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let database = self.default_query_database(connection_index, cx);
        self.open_query_with(Some(connection_index), database, cx);
    }

    /// Open a query tab for a specific database in the tree, selecting it up front.
    pub(super) fn open_new_query_for_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let database = self.database_name(connection_index, database_index);
        self.open_query_with(Some(connection_index), database, cx);
    }

    fn open_query_with(
        &mut self,
        connection_index: Option<usize>,
        database: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let id = self.next_query_id;
        self.next_query_id += 1;
        let mut tab = QueryTab::new(id);
        tab.connection_index = connection_index;
        tab.database = database;
        self.queries.push(tab);
        self.active_query = Some(self.queries.len() - 1);
        self.active_grid = None;
        self.active_design = None;
        self.query_completion = None;
        self.clear_object_search(cx);
        self.query_focus_pending = true;
        self.main_tab = MainTab::Queries;
        cx.notify();
    }

    pub(super) fn activate_query(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.active_query = Some(index);
        self.active_design = None;
        self.active_grid = self
            .queries
            .get(index)
            .and_then(|tab| tab.grid_id)
            .and_then(|id| {
                self.grids
                    .iter()
                    .position(|grid| grid.read(cx).state.id == id)
            });
        self.query_completion = None;
        self.query_focus_pending = true;
        cx.notify();
    }

    pub(super) fn close_query(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.queries.len() {
            return;
        }
        let active_id = self.active_grid_id(cx);
        let removed = self.queries.remove(index);
        let mut grid_ids: Vec<u64> = removed.result_grids.iter().flatten().copied().collect();
        if let Some(id) = removed.grid_id
            && !grid_ids.contains(&id)
        {
            grid_ids.push(id);
        }
        if let Some(id) = removed.view.as_ref().and_then(|view| view.explain_grid_id) {
            grid_ids.push(id);
        }
        for id in grid_ids {
            if let Some(position) = self
                .grids
                .iter()
                .position(|grid| grid.read(cx).state.id == id)
            {
                self.grids.remove(position);
            }
        }
        match self.active_query {
            Some(active) if active == index => {
                self.active_query = if self.queries.is_empty() {
                    None
                } else {
                    Some(index.min(self.queries.len() - 1))
                };
            }
            Some(active) if active > index => self.active_query = Some(active - 1),
            _ => {}
        }
        self.active_grid = self
            .active_query
            .and_then(|active| self.queries.get(active))
            .and_then(|tab| tab.grid_id)
            .and_then(|id| {
                self.grids
                    .iter()
                    .position(|grid| grid.read(cx).state.id == id)
            })
            .or_else(|| {
                active_id.and_then(|id| {
                    self.grids
                        .iter()
                        .position(|grid| grid.read(cx).state.id == id)
                })
            });
        self.query_completion = None;
        cx.notify();
    }

    pub(super) fn close_tab(&mut self, target: TabTarget, cx: &mut Context<'_, Self>) {
        self.tab_menu = None;
        match target {
            TabTarget::Grid(index) => self.close_grid(index, cx),
            TabTarget::Query(index) => self.close_query(index, cx),
            TabTarget::Design(index) => self.close_design(index, cx),
        }
    }

    pub(super) fn close_other_tabs(&mut self, target: TabTarget, cx: &mut Context<'_, Self>) {
        self.tab_menu = None;
        match target {
            TabTarget::Grid(keep) => {
                let keep_id = self.grids.get(keep).map(|grid| grid.read(cx).state.id);
                while !self.queries.is_empty() {
                    self.close_query(self.queries.len() - 1, cx);
                }
                while !self.designs.is_empty() {
                    self.close_design(self.designs.len() - 1, cx);
                }
                let mut index = self.grids.len();
                while index > 0 {
                    index -= 1;
                    if Some(self.grids[index].read(cx).state.id) != keep_id {
                        self.close_grid(index, cx);
                    }
                }
                if let Some(id) = keep_id
                    && let Some(position) = self
                        .grids
                        .iter()
                        .position(|grid| grid.read(cx).state.id == id)
                {
                    self.activate_grid(Some(position), cx);
                }
            }
            TabTarget::Query(keep) => {
                let keep_id = self.queries.get(keep).map(|query| query.id);
                while !self.grids.is_empty() {
                    self.close_grid(self.grids.len() - 1, cx);
                }
                while !self.designs.is_empty() {
                    self.close_design(self.designs.len() - 1, cx);
                }
                let mut index = self.queries.len();
                while index > 0 {
                    index -= 1;
                    if Some(self.queries[index].id) != keep_id {
                        self.close_query(index, cx);
                    }
                }
                if let Some(id) = keep_id
                    && let Some(position) = self.queries.iter().position(|query| query.id == id)
                {
                    self.activate_query(position, cx);
                }
            }
            TabTarget::Design(keep) => {
                let keep_id = self.designs.get(keep).map(|design| design.read(cx).id);
                while !self.queries.is_empty() {
                    self.close_query(self.queries.len() - 1, cx);
                }
                while !self.grids.is_empty() {
                    self.close_grid(self.grids.len() - 1, cx);
                }
                let mut index = self.designs.len();
                while index > 0 {
                    index -= 1;
                    if Some(self.designs[index].read(cx).id) != keep_id {
                        self.close_design(index, cx);
                    }
                }
                if let Some(id) = keep_id
                    && let Some(position) = self
                        .designs
                        .iter()
                        .position(|design| design.read(cx).id == id)
                {
                    self.activate_design(Some(position), cx);
                }
            }
        }
    }

    pub(super) fn close_all_tabs(&mut self, cx: &mut Context<'_, Self>) {
        self.tab_menu = None;
        while !self.queries.is_empty() {
            self.close_query(self.queries.len() - 1, cx);
        }
        while !self.grids.is_empty() {
            self.close_grid(self.grids.len() - 1, cx);
        }
        while !self.designs.is_empty() {
            self.close_design(self.designs.len() - 1, cx);
        }
    }

    pub(super) fn query_connection_options(&self) -> Vec<(String, String)> {
        self.connections
            .iter()
            .enumerate()
            .map(|(index, node)| (index.to_string(), node.profile.name.clone()))
            .collect()
    }

    pub(super) fn query_database_options(&self, connection_index: usize) -> Vec<(String, String)> {
        match self
            .connections
            .get(connection_index)
            .map(|node| &node.databases)
        {
            Some(Loadable::Loaded(databases)) => databases
                .iter()
                .map(|database| (database.name.clone(), database.name.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn ensure_query_combos(&mut self, cx: &mut Context<'_, Self>) {
        let theme = self.theme;
        if self.query_connection_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 240.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.query_connection_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/connection.svg", theme.icon_connection, cx);
            });
            self.query_connection_combo = Some(combo);
        }
        if self.query_database_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 240.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.query_database_selected(value, cx));
                    },
                ))
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/database.svg", theme.icon_database, cx);
            });
            self.query_database_combo = Some(combo);
        }
    }

    pub(super) fn sync_query_combos(&mut self, cx: &mut Context<'_, Self>) {
        let Some(active) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get(active) else {
            return;
        };
        let has_connection = tab.connection_index.is_some();
        let connection_selected = tab
            .connection_index
            .map(|index| index.to_string())
            .unwrap_or_default();
        let database_selected = tab.database.clone().unwrap_or_default();
        let connection_options: Vec<ComboOption> = self
            .query_connection_options()
            .into_iter()
            .map(|(value, label)| ComboOption::new(value, label))
            .collect();
        let database_options: Vec<ComboOption> = tab
            .connection_index
            .map(|index| self.query_database_options(index))
            .unwrap_or_default()
            .into_iter()
            .map(|(value, label)| ComboOption::new(value, label))
            .collect();
        let not_connected = t!("query.not_connected").to_string();
        let database_placeholder = t!("database.name").to_string();
        if let Some(combo) = self.query_connection_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(connection_options, cx);
                combo.set_placeholder(not_connected, cx);
                combo.set_selected(connection_selected, cx);
            });
        }
        if let Some(combo) = self.query_database_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(database_options, cx);
                combo.set_placeholder(database_placeholder, cx);
                combo.set_enabled(has_connection, cx);
                combo.set_selected(database_selected, cx);
            });
        }
    }

    pub(super) fn query_connection_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        self.query_completion = None;
        let Some(index) = self.active_query else {
            return;
        };
        let connection_index = value.parse::<usize>().ok();
        let database = connection_index.and_then(|index| self.default_query_database(index, cx));
        if let Some(tab) = self.queries.get_mut(index) {
            tab.connection_index = connection_index;
            tab.database = database;
        }
        if let Some(connection_index) = connection_index
            && !matches!(
                self.connections
                    .get(connection_index)
                    .map(|node| &node.status),
                Some(ConnectionStatus::Connected(_))
            )
        {
            self.connect(connection_index, cx);
        }
        cx.notify();
    }

    pub(super) fn query_database_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        self.query_completion = None;
        if let Some(index) = self.active_query
            && let Some(tab) = self.queries.get_mut(index)
        {
            tab.database = Some(value.to_string());
        }
        cx.notify();
    }

    pub(super) fn run_query(&mut self, selected_only: bool, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            return;
        };

        let (selection_start, selection_end) = tab.selection();
        let source = if selected_only && selection_start < selection_end {
            &tab.sql[selection_start..selection_end]
        } else {
            tab.sql.as_str()
        };
        let original = source.trim().to_string();
        if original.is_empty() {
            return;
        }
        let executable = original.clone();
        let database = tab.database.clone();
        let connection_name = tab
            .connection_index
            .and_then(|connection_index| self.connections.get(connection_index))
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let Some(connection) = tab.connection_index.and_then(|i| self.connection_arc(i)) else {
            self.clear_query_results(index, Some(t!("query.not_connected").to_string()), cx);
            cx.notify();
            return;
        };

        if let Some(tab) = self.queries.get_mut(index) {
            tab.running = true;
            tab.result_error = None;
            tab.last_sql = executable.clone();
            tab.last_elapsed = None;
        }
        self.query_completion = None;
        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let query_connection = connection.clone();
            let query_sql = executable.clone();
            let query_database = database.clone();
            let started = std::time::Instant::now();
            let result = match runtime
                .spawn(async move {
                    query_connection
                        .execute_query_many(query_database.as_deref(), &query_sql)
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let elapsed = started.elapsed();

            // Resolve each result set's editability from its own statement, so a multi-statement
            // script still gets an editable grid per single-table `SELECT`.
            let plans = match result {
                Ok(results) => {
                    let mut plans = Vec::with_capacity(results.len());
                    for mut result in results {
                        let mut editable = false;
                        let mut grid_database = database.clone().unwrap_or_default();
                        let mut grid_table = String::new();
                        if result.has_result_set
                            && let Some((schema, table)) =
                                sql::infer_single_table(&result.statement)
                        {
                            let target_database =
                                schema.or_else(|| database.clone()).unwrap_or_default();
                            let column_connection = connection.clone();
                            let column_database = target_database.clone();
                            let column_table = table.clone();
                            let columns = match runtime
                                .spawn(async move {
                                    column_connection
                                        .columns(&column_database, &column_table)
                                        .await
                                })
                                .await
                            {
                                Ok(inner) => inner.ok(),
                                Err(_) => None,
                            };
                            if let Some(columns) = columns {
                                for column in result.columns.iter_mut() {
                                    if let Some(found) = columns
                                        .iter()
                                        .find(|candidate| candidate.name == column.name)
                                    {
                                        column.primary_key = found.primary_key;
                                        column.data_type = found.data_type.clone();
                                    }
                                }
                            }
                            editable = true;
                            grid_database = target_database;
                            grid_table = table;
                        }
                        plans.push(QueryResultPlan {
                            result,
                            editable,
                            database: grid_database,
                            table: grid_table,
                        });
                    }
                    Ok(plans)
                }
                Err(error) => Err(error),
            };

            let _ = this.update(cx, |view, cx| {
                view.apply_query_results(index, connection_name, executable, elapsed, plans, cx);
            });
        })
        .detach();
    }

    /// Drop a tab's result grids and reset its result state, recording `error` for the 信息 tab.
    fn clear_query_results(
        &mut self,
        index: usize,
        error: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let ids: Vec<u64> = self
            .queries
            .get(index)
            .map(|tab| tab.result_grids.iter().flatten().copied().collect())
            .unwrap_or_default();
        for id in ids {
            if let Some(position) = self
                .grids
                .iter()
                .position(|grid| grid.read(cx).state.id == id)
            {
                self.grids.remove(position);
            }
        }
        if let Some(tab) = self.queries.get_mut(index) {
            tab.running = false;
            tab.results.clear();
            tab.result_grids.clear();
            tab.active_result = 0;
            tab.grid_id = None;
            tab.result_error = error;
        }
        self.active_grid = None;
        cx.notify();
    }

    /// Switch the bottom result panel to `tab` (`0` is 信息, `1..=n` the `n`-th result set).
    pub(super) fn select_query_result(&mut self, tab: usize, cx: &mut Context<'_, Self>) {
        let Some(query) = self.active_query else {
            return;
        };
        let Some(state) = self.queries.get_mut(query) else {
            return;
        };
        let grid_count = state
            .result_grids
            .iter()
            .filter(|grid| grid.is_some())
            .count();
        if tab > grid_count {
            return;
        }
        state.active_result = tab;
        let grid_id = if tab == 0 {
            None
        } else {
            state
                .result_grids
                .iter()
                .filter_map(|grid| *grid)
                .nth(tab - 1)
        };
        state.grid_id = grid_id;
        self.active_grid = grid_id.and_then(|id| {
            self.grids
                .iter()
                .position(|grid| grid.read(cx).state.id == id)
        });
        cx.notify();
    }

    /// Install the result sets of a query run: one grid per result set, plus the per-entry state
    /// that drives the bottom result tabs and the 信息 panel.
    pub(super) fn apply_query_results(
        &mut self,
        index: usize,
        connection_name: String,
        sql: String,
        elapsed: std::time::Duration,
        result: Result<Vec<QueryResultPlan>, Error>,
        cx: &mut Context<'_, Self>,
    ) {
        let active_id = self.active_grid_id(cx);

        // Drop the previous run's grids for this tab.
        let old_ids: Vec<u64> = self
            .queries
            .get(index)
            .map(|tab| tab.result_grids.iter().flatten().copied().collect())
            .unwrap_or_default();
        for id in old_ids {
            if let Some(position) = self
                .grids
                .iter()
                .position(|grid| grid.read(cx).state.id == id)
            {
                self.grids.remove(position);
            }
        }

        match result {
            Ok(plans) => {
                let Some(connection) = self
                    .queries
                    .get(index)
                    .and_then(|tab| tab.connection_index)
                    .and_then(|i| self.connection_arc(i))
                else {
                    return;
                };
                let mut result_grids: Vec<Option<u64>> = Vec::with_capacity(plans.len());
                let mut results: Vec<QueryResultSummary> = Vec::with_capacity(plans.len());
                for plan in plans {
                    let QueryResultPlan {
                        result,
                        editable,
                        database,
                        table,
                    } = plan;
                    let summary = QueryResultSummary {
                        has_result_set: result.has_result_set,
                        row_count: result.rows.len(),
                        rows_affected: result.rows_affected,
                    };
                    if result.has_result_set {
                        let id = self.next_grid_id;
                        self.next_grid_id += 1;
                        let total = result.rows.len() as u64;
                        let column_widths = compute_column_widths(&result.columns, &result.rows);
                        let state = GridState {
                            id,
                            connection: connection.clone(),
                            connection_name: connection_name.clone(),
                            database,
                            table,
                            is_view: false,
                            page_index: 0,
                            page_size: total.max(1),
                            loading: false,
                            error: None,
                            columns: result.columns,
                            rows: Arc::new(result.rows),
                            column_widths,
                            manual_column_widths: false,
                            total_rows: Some(total),
                            selection: None,
                            edits: BTreeMap::new(),
                            undo: Vec::new(),
                            sql: Some(result.statement.clone()),
                            show_toolbar: false,
                            show_footer: true,
                            editable,
                            sort_rules: Vec::new(),
                            sort_open: false,
                            sort_draft: Vec::new(),
                            sort_selected: None,
                            filters: Vec::new(),
                            filter_open: false,
                            filter_draft: Vec::new(),
                            elapsed: Some(elapsed),
                        };
                        let app = cx.weak_entity();
                        let runtime = self.runtime.clone();
                        let theme = self.theme;
                        let entity = cx.new(|cx| GridView::new(state, app, runtime, theme, cx));
                        self.grids.push(entity);
                        result_grids.push(Some(id));
                    } else {
                        result_grids.push(None);
                    }
                    results.push(summary);
                }
                // The 信息 tab is 0; the first result set (if any) is selected by default.
                let has_grid = result_grids.iter().any(|grid| grid.is_some());
                let active_result = if has_grid { 1 } else { 0 };
                let active_grid_id = result_grids.iter().find_map(|grid| *grid);
                if let Some(tab) = self.queries.get_mut(index) {
                    tab.running = false;
                    tab.result_error = None;
                    tab.results = results;
                    tab.result_grids = result_grids;
                    tab.active_result = active_result;
                    tab.grid_id = active_grid_id;
                    tab.last_sql = sql;
                    tab.last_elapsed = Some(elapsed);
                }
                if self.active_query == Some(index) {
                    self.active_grid = active_grid_id.and_then(|id| {
                        self.grids
                            .iter()
                            .position(|grid| grid.read(cx).state.id == id)
                    });
                } else {
                    self.active_grid = active_id.and_then(|id| {
                        self.grids
                            .iter()
                            .position(|grid| grid.read(cx).state.id == id)
                    });
                }
            }
            Err(error) => {
                if let Some(tab) = self.queries.get_mut(index) {
                    tab.running = false;
                    tab.results.clear();
                    tab.result_grids.clear();
                    tab.active_result = 0;
                    tab.grid_id = None;
                    tab.result_error = Some(error.to_string());
                    tab.last_sql = sql;
                    tab.last_elapsed = Some(elapsed);
                }
                self.active_grid = active_id.and_then(|id| {
                    self.grids
                        .iter()
                        .position(|grid| grid.read(cx).state.id == id)
                });
            }
        }

        cx.notify();
    }

    pub(super) fn stop_query(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        if let Some(tab) = self.queries.get_mut(index) {
            tab.running = false;
        }
        cx.notify();
    }

    pub(super) fn format_query(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get_mut(index) else {
            return;
        };
        tab.sql = sql::format(&tab.sql);
        tab.caret = tab.sql.len();
        tab.anchor = tab.caret;
        self.query_completion = None;
        self.caret_visible = true;
        cx.notify();
    }

    pub(super) fn query_completion_items(
        &self,
        connection_index: Option<usize>,
    ) -> Rc<Vec<(String, String)>> {
        {
            let cache = self.query_completion_cache.borrow();
            if let Some(cache) = cache.as_ref()
                && cache.connection_index == connection_index
                && cache.generation == self.completion_generation
            {
                return cache.items.clone();
            }
        }

        let mut items: Vec<(String, String)> = sql::keywords()
            .iter()
            .map(|keyword| (keyword.to_lowercase(), (*keyword).to_string()))
            .collect();

        if let Some(index) = connection_index
            && let Some(node) = self.connections.get(index)
            && let Loadable::Loaded(databases) = &node.databases
        {
            for database in databases {
                if let Loadable::Loaded(tables) = &database.tables {
                    items.extend(
                        tables
                            .iter()
                            .map(|table| (table.name.to_lowercase(), table.name.clone())),
                    );
                }
            }
        }

        items.sort_by(|a, b| a.0.cmp(&b.0));
        items.dedup_by(|a, b| a.0 == b.0);
        let items = Rc::new(items);
        *self.query_completion_cache.borrow_mut() = Some(CompletionCache {
            connection_index,
            generation: self.completion_generation,
            items: items.clone(),
        });
        items
    }

    pub(super) fn refresh_query_completion(&mut self, force: bool) {
        let Some(index) = self.active_query else {
            self.query_completion = None;
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            self.query_completion = None;
            return;
        };

        let caret = tab.caret.min(tab.sql.len());
        let mut start = caret;
        while start > 0 {
            let previous = previous_boundary(&tab.sql, start);
            let character = tab.sql[previous..start].chars().next().unwrap_or(' ');
            if character.is_alphanumeric() || character == '_' || character == '$' {
                start = previous;
            } else {
                break;
            }
        }
        let prefix = tab.sql[start..caret].to_lowercase();
        let connection_index = tab.connection_index;

        if prefix.is_empty() && !force {
            self.query_completion = None;
            return;
        }

        let candidates: Vec<String> = self
            .query_completion_items(connection_index)
            .iter()
            .filter(|(lower, _)| lower.starts_with(&prefix))
            .take(64)
            .map(|(_, original)| original.clone())
            .collect();

        if candidates.is_empty() {
            self.query_completion = None;
            return;
        }
        self.query_completion = Some(Completion {
            candidates,
            selected: 0,
            start,
            end: caret,
        });
    }

    pub(super) fn move_query_completion(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        if let Some(completion) = self.query_completion.as_mut()
            && !completion.candidates.is_empty()
        {
            let length = completion.candidates.len() as isize;
            completion.selected =
                ((completion.selected as isize + delta).rem_euclid(length)) as usize;
            cx.notify();
        }
    }

    pub(super) fn accept_query_completion(
        &mut self,
        candidate: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(completion) = self.query_completion.take() else {
            return;
        };
        let Some(index) = self.active_query else {
            return;
        };
        if let Some(tab) = self.queries.get_mut(index) {
            let end = completion.end.min(tab.sql.len());
            let start = completion.start.min(end);
            tab.sql.replace_range(start..end, &candidate);
            let caret = start + candidate.len();
            tab.caret = caret;
            tab.anchor = caret;
        }
        self.caret_visible = true;
        cx.notify();
    }

    pub(super) fn accept_selected_query_completion(&mut self, cx: &mut Context<'_, Self>) {
        let candidate = self
            .query_completion
            .as_ref()
            .and_then(|completion| completion.candidates.get(completion.selected))
            .cloned();
        if let Some(candidate) = candidate {
            self.accept_query_completion(candidate, cx);
        }
    }

    pub(super) fn styled_sql(&self, text: &str, selection: (usize, usize)) -> StyledText {
        let theme = self.theme;
        let spans = self.sql_spans(text);
        let (selection_start, selection_end) = selection;

        let mut boundaries = Vec::with_capacity(spans.len() * 2 + 4);
        boundaries.push(0);
        boundaries.push(text.len());
        for span in spans.iter() {
            boundaries.push(span.start);
            boundaries.push(span.end);
        }
        if selection_start < selection_end {
            boundaries.push(selection_start);
            boundaries.push(selection_end);
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        let mut highlights = Vec::new();
        // `spans` and `boundaries` are both sorted and non-overlapping, so a single advancing
        // cursor finds the token for every boundary window in linear time.
        let mut span_index = 0usize;
        for window in boundaries.windows(2) {
            let (start, end) = (window[0], window[1]);
            if start >= end {
                continue;
            }
            while span_index < spans.len() && spans[span_index].end <= start {
                span_index += 1;
            }
            let token = spans
                .get(span_index)
                .filter(|span| span.start <= start && end <= span.end)
                .map(|span| span.token);
            let selected =
                selection_start < selection_end && selection_start <= start && end <= selection_end;
            let color = token.map(|token| rgb(sql_token_color(theme, token)).into());
            let background_color = if selected {
                Some(rgb(theme.tree_selected_bg).into())
            } else {
                None
            };
            if color.is_none() && background_color.is_none() {
                continue;
            }
            highlights.push((
                start..end,
                HighlightStyle {
                    color,
                    background_color,
                    ..Default::default()
                },
            ));
        }

        StyledText::new(text.to_string()).with_highlights(highlights)
    }

    /// The tokenizer spans for `text`, reusing the previous result when the text is unchanged.
    fn sql_spans(&self, text: &str) -> Rc<Vec<SqlSpan>> {
        let mut cache = self.sql_highlight_cache.borrow_mut();
        if let Some((cached, spans)) = cache.as_ref()
            && cached == text
        {
            return spans.clone();
        }
        let spans = Rc::new(sql::highlight(text));
        *cache = Some((text.to_string(), spans.clone()));
        spans
    }

    // ------------------------------------------------------------------------------------------
    // Saved queries: named SQL documents listed under the Queries main tab.
    // ------------------------------------------------------------------------------------------

    /// Open the "save query" dialog for the active editor tab, pre-filled with its current name
    /// and save location.
    pub(super) fn begin_save_query(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            return;
        };
        let name = tab.name.clone().unwrap_or_default();
        let connection_index = tab
            .connection_index
            .or_else(|| self.default_query_connection(cx));
        let database = tab
            .database
            .clone()
            .or_else(|| connection_index.and_then(|i| self.default_query_database(i, cx)))
            .unwrap_or_default();

        let theme = self.theme;
        let weak = cx.weak_entity();
        let change = weak.clone();
        let submit = weak.clone();
        let cancel = weak.clone();
        let initial_name = name.clone();
        let input = cx.new(move |cx| {
            TextInput::new(theme, initial_name.clone(), TextInputOptions::default(), cx)
                .on_change(Rc::new(move |text, _window, cx| {
                    let _ = change.update(cx, |app, cx| {
                        if let Some(dialog) = app.save_query_dialog.as_mut() {
                            dialog.name = text.to_string();
                            dialog.error = None;
                        }
                        cx.notify();
                    });
                }))
                .on_submit(Rc::new(move |_window, cx| {
                    let _ = submit.update(cx, |app, cx| app.submit_save_query(cx));
                }))
                .on_cancel(Rc::new(move |_window, cx| {
                    let _ = cancel.update(cx, |app, cx| app.cancel_save_query(cx));
                }))
        });
        input.update(cx, |input, cx| input.focus_state(window, cx));
        self.query_name_input = Some(input);

        self.ensure_save_dialog_combos(theme, cx);

        self.save_query_dialog = Some(SaveQueryDialog {
            tab_index: index,
            name,
            connection_index,
            database,
            error: None,
        });
        self.save_query_focus_pending = true;
        cx.notify();
    }

    /// Create the save dialog's connection and database pickers once.
    fn ensure_save_dialog_combos(&mut self, theme: Theme, cx: &mut Context<'_, Self>) {
        if self.save_connection_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(
                    theme,
                    Vec::new(),
                    String::new(),
                    SAVE_DIALOG_COMBO_WIDTH,
                    cx,
                )
                .on_select(Rc::new(move |value, _window, cx| {
                    let _ = weak.update(cx, |app, cx| app.save_connection_selected(value, cx));
                }))
                .full_width()
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/connection.svg", theme.icon_connection, cx);
            });
            self.save_connection_combo = Some(combo);
        }
        if self.save_database_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(
                    theme,
                    Vec::new(),
                    String::new(),
                    SAVE_DIALOG_COMBO_WIDTH,
                    cx,
                )
                .on_select(Rc::new(move |value, _window, cx| {
                    let _ = weak.update(cx, |app, cx| app.save_database_selected(value, cx));
                }))
                .full_width()
            });
            combo.update(cx, |combo, cx| {
                combo.set_icon("icons/database.svg", theme.icon_database, cx);
            });
            self.save_database_combo = Some(combo);
        }
    }

    /// The connection picker's rows: every configured connection profile.
    fn save_connection_options(&self) -> Vec<ComboOption> {
        self.connections
            .iter()
            .enumerate()
            .map(|(index, node)| ComboOption::new(index.to_string(), node.profile.name.clone()))
            .collect()
    }

    /// The database picker's rows: the connection's live databases, falling back to the last list
    /// remembered for it so a database can be picked without opening the connection.
    fn save_database_options(&self, connection_index: usize) -> Vec<ComboOption> {
        let Some(node) = self.connections.get(connection_index) else {
            return Vec::new();
        };
        let names: Vec<String> = match &node.databases {
            Loadable::Loaded(databases) if !databases.is_empty() => databases
                .iter()
                .map(|database| database.name.clone())
                .collect(),
            _ => self
                .database_cache
                .get(&node.profile.id)
                .cloned()
                .unwrap_or_default(),
        };
        names.into_iter().map(ComboOption::plain).collect()
    }

    /// Remember a connection's database names (called when they load) so the save dialog can offer
    /// them without opening the connection.
    pub(super) fn remember_databases(&mut self, connection_index: usize) {
        let Some(node) = self.connections.get(connection_index) else {
            return;
        };
        let Loadable::Loaded(databases) = &node.databases else {
            return;
        };
        let names: Vec<String> = databases
            .iter()
            .map(|database| database.name.clone())
            .collect();
        let id = node.profile.id.clone();
        if self.database_cache.get(&id) == Some(&names) {
            return;
        }
        self.database_cache.insert(id, names);
        let _ = self.config.save_database_cache(&self.database_cache);
    }

    pub(super) fn sync_save_dialog_combos(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.save_query_dialog.as_ref() else {
            return;
        };
        let connection_index = dialog.connection_index;
        let database = dialog.database.clone();
        let connection_options = self.save_connection_options();
        let connection_selected = connection_index
            .map(|index| index.to_string())
            .unwrap_or_default();
        let database_options = connection_index
            .map(|index| self.save_database_options(index))
            .unwrap_or_default();
        let connection_placeholder = t!("info.connection").to_string();
        let database_placeholder = t!("query.schema").to_string();
        if let Some(combo) = self.save_connection_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(connection_options, cx);
                combo.set_placeholder(connection_placeholder, cx);
                combo.set_selected(connection_selected, cx);
            });
        }
        if let Some(combo) = self.save_database_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(database_options, cx);
                combo.set_placeholder(database_placeholder, cx);
                combo.set_enabled(connection_index.is_some(), cx);
                combo.set_selected(database, cx);
            });
        }
    }

    /// The user picked a connection: keep the database when it still exists, else reset it to the
    /// connection's configured database when that is among the choices.
    pub(super) fn save_connection_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        let Ok(connection_index) = value.parse::<usize>() else {
            return;
        };
        let options = self.save_database_options(connection_index);
        let default = self
            .connections
            .get(connection_index)
            .and_then(|node| node.profile.database.clone())
            .filter(|database| options.iter().any(|option| &option.value == database));
        if let Some(dialog) = self.save_query_dialog.as_mut() {
            let keep = options.iter().any(|option| option.value == dialog.database);
            dialog.connection_index = Some(connection_index);
            if !keep {
                dialog.database = default.unwrap_or_default();
            }
            dialog.error = None;
        }
        cx.notify();
    }

    pub(super) fn save_database_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.save_query_dialog.as_mut() {
            dialog.database = value.to_string();
            dialog.error = None;
        }
        cx.notify();
    }

    pub(super) fn cancel_save_query(&mut self, cx: &mut Context<'_, Self>) {
        self.save_query_dialog = None;
        self.query_name_input = None;
        cx.notify();
    }

    pub(super) fn submit_save_query(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.save_query_dialog.as_ref() else {
            return;
        };
        let name = dialog.name.trim().to_string();
        let tab_index = dialog.tab_index;
        let connection_index = dialog.connection_index;
        let database = dialog.database.clone();

        if name.is_empty() {
            self.save_query_error(t!("query.name_required").to_string(), cx);
            return;
        }
        let Some(connection_id) = connection_index
            .and_then(|index| self.connections.get(index))
            .map(|node| node.profile.id.clone())
        else {
            self.save_query_error(t!("query.no_location").to_string(), cx);
            return;
        };
        // A saved query always belongs to a database, so the location is not complete without one.
        if database.is_empty() {
            self.save_query_error(t!("query.no_database").to_string(), cx);
            return;
        }
        let Some(sql) = self.queries.get(tab_index).map(|tab| tab.sql.clone()) else {
            return;
        };

        let query = SavedQuery {
            name: name.clone(),
            sql,
            connection_id,
            database: database.clone(),
        };
        let saved_path = match self.config.save_query_file(&query) {
            Ok(path) => path,
            Err(error) => {
                self.save_query_error(error.to_string(), cx);
                return;
            }
        };
        self.refresh_query_files(cx);
        if let Some(index) = self
            .query_files
            .iter()
            .position(|file| file.path == saved_path)
        {
            self.select_query_one(index);
        }

        if let Some(tab) = self.queries.get_mut(tab_index) {
            tab.name = Some(name);
            // Keep the editor bound to the location it was filed under, so a later save does not
            // duplicate it under the tab's previous connection.
            tab.connection_index = connection_index;
            tab.database = (!database.is_empty()).then(|| database.clone());
        }
        if let Some(connection_index) = connection_index
            && !matches!(
                self.connections
                    .get(connection_index)
                    .map(|node| &node.status),
                Some(ConnectionStatus::Connected(_))
            )
        {
            self.connect(connection_index, cx);
        }
        self.save_query_dialog = None;
        self.query_name_input = None;
        self.main_tab = MainTab::Queries;
        cx.notify();
    }

    fn save_query_error(&mut self, message: String, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.save_query_dialog.as_mut() {
            dialog.error = Some(message);
        }
        cx.notify();
    }

    /// The display name of the connection a saved query belongs to (falls back to the stored id
    /// when the connection no longer exists).
    pub(super) fn query_connection_name(&self, connection_id: &str) -> String {
        self.connections
            .iter()
            .find(|node| node.profile.id == connection_id)
            .map(|node| node.profile.name.clone())
            .unwrap_or_else(|| connection_id.to_string())
    }

    /// Rescan the query directory tree for `.sql` files.
    pub(super) fn refresh_query_files(&mut self, cx: &mut Context<'_, Self>) {
        self.query_files = scan_query_files(&self.config);
        self.prune_query_selection();
        self.clamp_query_selection();
        cx.notify();
    }

    /// Drop selected keys whose file no longer exists (e.g. after a rescan).
    fn prune_query_selection(&mut self) {
        let keys: std::collections::HashSet<String> =
            self.query_files.iter().map(query_file_key).collect();
        self.query_selection.retain(|key| keys.contains(key));
    }

    fn clamp_query_selection(&mut self) {
        if let Some(index) = self.saved_query_selected
            && index >= self.query_files.len()
        {
            self.saved_query_selected = None;
            self.query_selection.clear();
        }
    }

    /// The visible query entries' selection keys, in the list's current (sorted) order.
    fn query_visible_keys(&self, cx: &App) -> Vec<String> {
        self.sorted_visible_query_files(cx, &self.object_search)
            .into_iter()
            .filter_map(|index| self.query_files.get(index))
            .map(query_file_key)
            .collect()
    }

    /// Keep the single-selection mirror (`saved_query_selected`) in step with the multi-selection.
    pub(super) fn sync_query_selected(&mut self) {
        self.saved_query_selected = self.query_selection.single().and_then(|key| {
            self.query_files
                .iter()
                .position(|file| query_file_key(file) == key)
        });
    }

    /// Replace the query selection with one file (used by right-click and programmatic selects).
    pub(super) fn select_query_one(&mut self, index: usize) {
        if let Some(file) = self.query_files.get(index) {
            self.query_selection.select_one(query_file_key(file));
        }
        self.saved_query_selected = Some(index);
    }

    /// Apply one query row hit (click) under the click's modifier mode.
    pub(super) fn hit_query(
        &mut self,
        key: &str,
        _click_count: usize,
        modifiers: Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        if self.query_rename.is_some() {
            return;
        }
        let visible = self.query_visible_keys(cx);
        if modifiers.shift {
            self.query_selection.extend_to(&visible, key);
        } else {
            self.query_selection
                .hit(&visible, key, selection_mode(modifiers));
        }
        self.sync_query_selected();
        cx.notify();
    }

    /// The database the Queries tab is scoped to, taken from the connection tree's selection.
    /// Like backups, queries only resolve when the connection is open and the database is opened.
    pub(super) fn query_scope(&self, cx: &App) -> Option<(String, String)> {
        let selected = self.tree_pane.read(cx).selected.clone()?;
        let (connection_index, database_index): (usize, usize) =
            if let Some(rest) = selected.strip_prefix("db-") {
                let mut parts = rest.splitn(2, '-');
                Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
            } else if let Some(rest) = selected.strip_prefix("cat-") {
                let mut parts = rest.splitn(3, '-');
                let pair = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
                // Only the Queries category scopes the Queries tab.
                (parts.next()? == "q").then_some(pair)
            } else {
                None
            }?;
        let node = self.connections.get(connection_index)?;
        if !matches!(node.status, ConnectionStatus::Connected(_)) {
            return None;
        }
        let Loadable::Loaded(databases) = &node.databases else {
            return None;
        };
        let database = databases.get(database_index)?;
        if !database.opened {
            return None;
        }
        Some((node.profile.id.clone(), database.name.clone()))
    }

    /// Indices into `query_files` that belong to the current scope, in list order.
    pub(super) fn visible_query_files(&self, cx: &App) -> Vec<usize> {
        let Some((connection_id, database)) = self.query_scope(cx) else {
            return Vec::new();
        };
        self.query_files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.connection_id == connection_id && file.database == database)
            .map(|(index, _)| index)
            .collect()
    }

    /// The in-scope query files filtered by `search`, ordered by the Queries 详细列表's current
    /// sort. Returns indices into `query_files`.
    pub(super) fn sorted_visible_query_files(&self, cx: &App, search: &str) -> Vec<usize> {
        let needle = search.trim().to_lowercase();
        let mut indices: Vec<usize> = self
            .visible_query_files(cx)
            .into_iter()
            .filter(|index| {
                needle.is_empty()
                    || self.query_files[*index]
                        .name
                        .to_lowercase()
                        .contains(&needle)
            })
            .collect();
        let sort = self.query_sort;
        indices.sort_by(|&a, &b| {
            let left = &self.query_files[a];
            let right = &self.query_files[b];
            let ordering = match sort.column {
                QuerySortColumn::Name => left.name.to_lowercase().cmp(&right.name.to_lowercase()),
                QuerySortColumn::Modified => left.modified.cmp(&right.modified),
                QuerySortColumn::Size => left.size.cmp(&right.size),
            };
            let ordering = if sort.descending {
                ordering.reverse()
            } else {
                ordering
            };
            ordering.then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        indices
    }

    /// Sort the Queries 详细列表 by `column`, flipping the direction when it is already active.
    pub(super) fn toggle_query_sort(
        &mut self,
        column: QuerySortColumn,
        cx: &mut Context<'_, Self>,
    ) {
        if self.query_sort.column == column {
            self.query_sort.descending = !self.query_sort.descending;
        } else {
            self.query_sort.column = column;
            self.query_sort.descending = false;
        }
        cx.notify();
    }

    /// Whether the highlighted query file is inside the current scope (so Delete is enabled).
    pub(super) fn query_selected_in_scope(&self, cx: &App) -> bool {
        let Some(file) = self
            .saved_query_selected
            .and_then(|index| self.query_files.get(index))
        else {
            return false;
        };
        match self.query_scope(cx) {
            Some((connection_id, database)) => {
                file.connection_id == connection_id && file.database == database
            }
            None => false,
        }
    }

    /// Open a saved query file in a new editor tab (or activate the tab that already shows it).
    pub(super) fn open_saved_query(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(file) = self.query_files.get(index) else {
            return;
        };
        let connection_id = file.connection_id.clone();
        let database = file.database.clone();
        let name = file.name.clone();
        let Ok(sql) = std::fs::read_to_string(&file.path) else {
            self.error_dialog = Some(t!("query.read_failed", name = name).to_string());
            cx.notify();
            return;
        };
        let connection_index = self
            .connections
            .iter()
            .position(|node| node.profile.id == connection_id);
        let database_option = (!database.is_empty()).then_some(database);

        let existing = self.queries.iter().position(|tab| {
            tab.name.as_deref() == Some(name.as_str())
                && tab.connection_index == connection_index
                && tab.database.as_deref() == database_option.as_deref()
        });
        if let Some(existing) = existing {
            self.activate_query(existing, cx);
            return;
        }

        self.open_query_with(connection_index, database_option, cx);
        if let Some(active) = self.active_query
            && let Some(tab) = self.queries.get_mut(active)
        {
            tab.sql = sql;
            tab.name = Some(name);
            tab.caret = tab.sql.len();
            tab.anchor = tab.caret;
        }
        if let Some(connection_index) = connection_index
            && !matches!(
                self.connections
                    .get(connection_index)
                    .map(|node| &node.status),
                Some(ConnectionStatus::Connected(_))
            )
        {
            self.connect(connection_index, cx);
        }
        cx.notify();
    }

    /// Ask for confirmation before deleting a query file.
    pub(super) fn confirm_delete_saved_query(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.query_files.len() {
            return;
        }
        self.delete_confirm = Some(DeleteConfirm::SavedQuery { index });
        cx.notify();
    }

    pub(super) fn delete_saved_query(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(file) = self.query_files.get(index) else {
            return;
        };
        let query = SavedQuery {
            name: file.name.clone(),
            sql: String::new(),
            connection_id: file.connection_id.clone(),
            database: file.database.clone(),
        };
        if let Err(error) = self.config.delete_query_file(&query) {
            self.error_dialog = Some(error.to_string());
        }
        self.saved_query_selected = None;
        self.query_selection.clear();
        self.refresh_query_files(cx);
    }

    /// Copy a query file into the app's query clipboard, so Ctrl+V can drop it into any database.
    pub(super) fn copy_query_file(&mut self, index: usize) {
        let Some(file) = self.query_files.get(index) else {
            return;
        };
        self.query_clipboard = Some(QueryClipboard {
            name: file.name.clone(),
            path: file.path.clone(),
        });
    }

    /// Paste the copied query file into the current scope's database folder.
    pub(super) fn paste_query_file(&mut self, cx: &mut Context<'_, Self>) {
        let Some(clipboard) = self.query_clipboard.clone() else {
            return;
        };
        let Some((connection_id, database)) = self.query_scope(cx) else {
            return;
        };
        let suffix = t!("backup.copy_suffix").to_string();
        let mut name = clipboard.name.clone();
        let mut dest = self
            .config
            .query_file_path(&connection_id, &database, &name);
        if dest.exists() {
            let copy = format!("{name} - {suffix}");
            name = copy.clone();
            dest = self
                .config
                .query_file_path(&connection_id, &database, &name);
            let mut number = 2u64;
            while dest.exists() {
                name = format!("{copy} ({number})");
                dest = self
                    .config
                    .query_file_path(&connection_id, &database, &name);
                number += 1;
            }
        }
        match std::fs::copy(&clipboard.path, &dest) {
            Ok(_) => {
                self.refresh_query_files(cx);
                if let Some(index) = self.query_files.iter().position(|file| file.path == dest) {
                    self.select_query_one(index);
                }
            }
            Err(error) => self.error_dialog = Some(error.to_string()),
        }
        cx.notify();
    }

    /// Start the in-place "rename query" editor on the selected file's row.
    pub(super) fn begin_rename_query(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.query_rename.is_some() {
            return;
        }
        let Some(file) = self.query_files.get(index) else {
            return;
        };
        let old_name = file.name.clone();
        self.select_query_one(index);
        let theme = self.theme;
        let weak = cx.weak_entity();
        let change = weak.clone();
        let submit = weak.clone();
        let cancel = weak.clone();
        let initial = old_name.clone();
        let input = cx.new(move |cx| {
            TextInput::new(
                theme,
                initial,
                TextInputOptions {
                    bare: true,
                    text_size: Some(12.0),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| {
                    if let Some(edit) = app.query_rename.as_mut() {
                        edit.new_name = text.to_string();
                    }
                    cx.notify();
                });
            }))
            .on_submit(Rc::new(move |window, cx| {
                let _ = submit.update(cx, |app, cx| {
                    let focus = app.query_list_focus.clone();
                    app.submit_query_rename(cx);
                    window.focus(&focus, cx);
                });
            }))
            .on_cancel(Rc::new(move |window, cx| {
                let _ = cancel.update(cx, |app, cx| {
                    let focus = app.query_list_focus.clone();
                    app.query_rename = None;
                    app.query_rename_blur = None;
                    window.focus(&focus, cx);
                    cx.notify();
                });
            }))
        });
        let focus = input.read(cx).focus_handle();
        self.query_rename = Some(QueryRenameEdit {
            index,
            old_name: old_name.clone(),
            new_name: old_name,
            input,
        });
        self.query_rename_blur = Some(cx.on_blur(&focus, window, |app, _window, cx| {
            if app.query_rename.is_some() {
                app.submit_query_rename(cx);
            }
        }));
        self.query_rename_focus_pending = true;
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Commit the in-place rename; a taken name is rejected with an error dialog.
    pub(super) fn submit_query_rename(&mut self, cx: &mut Context<'_, Self>) {
        let Some(edit) = self.query_rename.take() else {
            return;
        };
        self.query_rename_blur = None;
        let new_name = edit.new_name.trim().to_string();
        if new_name.is_empty() || new_name == edit.old_name {
            cx.notify();
            return;
        }
        let Some(file) = self.query_files.get(edit.index) else {
            cx.notify();
            return;
        };
        let old_path = file.path.clone();
        let connection_id = file.connection_id.clone();
        let database = file.database.clone();
        let new_path = self
            .config
            .query_file_path(&connection_id, &database, &new_name);
        if new_path.exists() {
            self.error_dialog = Some(t!("query.rename_exists", name = new_name).to_string());
            cx.notify();
            return;
        }
        match std::fs::rename(&old_path, &new_path) {
            Ok(()) => {
                self.refresh_query_files(cx);
                if let Some(index) = self
                    .query_files
                    .iter()
                    .position(|file| file.path == new_path)
                {
                    self.select_query_one(index);
                }
            }
            Err(error) => self.error_dialog = Some(error.to_string()),
        }
        cx.notify();
    }

    /// Reveal a query file in the OS file manager.
    pub(super) fn reveal_query(&mut self, index: usize) {
        if let Some(file) = self.query_files.get(index) {
            reveal_in_file_manager(&file.path);
        }
    }

    /// Reveal the current scope's saved-query folder in the OS file manager.
    pub(super) fn reveal_query_folder(&mut self, cx: &mut Context<'_, Self>) {
        let Some((connection_id, database)) = self.query_scope(cx) else {
            return;
        };
        let dir = self
            .config
            .query_file_path(&connection_id, &database, "")
            .parent()
            .map(|parent| parent.to_path_buf())
            .unwrap_or_else(|| self.config.queries_dir());
        let _ = std::fs::create_dir_all(&dir);
        reveal_in_file_manager(&dir);
        cx.notify();
    }

    /// Open the object-info pane for a query file.
    pub(super) fn open_query_info(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.select_query_one(index);
        self.set_info_open(true, cx);
        cx.notify();
    }

    /// Refresh the query file list from disk (also called from the row context menu).
    pub(super) fn refresh_selected_query(&mut self, cx: &mut Context<'_, Self>) {
        self.refresh_query_files(cx);
    }
}

/// The width of the save dialog's connection and database pickers.
const SAVE_DIALOG_COMBO_WIDTH: f32 = 380.0;

/// Scan the config dir's `queries/` tree for `.sql` files, sorted by name.
pub(super) fn scan_query_files(config: &ConfigStore) -> Vec<QueryFileInfo> {
    let root = config.queries_dir();
    let mut files = Vec::new();
    if let Ok(connections) = std::fs::read_dir(&root) {
        for connection in connections.flatten() {
            if !connection
                .file_type()
                .map(|kind| kind.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let connection_id = connection.file_name().to_string_lossy().into_owned();
            let Ok(databases) = std::fs::read_dir(connection.path()) else {
                continue;
            };
            for database in databases.flatten() {
                if !database
                    .file_type()
                    .map(|kind| kind.is_dir())
                    .unwrap_or(false)
                {
                    continue;
                }
                let database_name = database.file_name().to_string_lossy().into_owned();
                let Ok(entries) = std::fs::read_dir(database.path()) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("sql") {
                        continue;
                    }
                    let metadata = entry.metadata().ok();
                    files.push(QueryFileInfo {
                        name: path
                            .file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        connection_id: connection_id.clone(),
                        database: database_name.clone(),
                        size: metadata.as_ref().map(|meta| meta.len()).unwrap_or(0),
                        created: metadata.as_ref().and_then(|meta| meta.created().ok()),
                        modified: metadata.and_then(|meta| meta.modified().ok()),
                        path,
                    });
                }
            }
        }
    }
    files.sort_by_key(|file| file.name.to_lowercase());
    files
}

/// The selection key of a saved-query row: its file path, unique across scopes.
pub(super) fn query_file_key(file: &QueryFileInfo) -> String {
    format!("q:{}", file.path.display())
}
