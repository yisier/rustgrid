//! The `Views` main tab's designer: opening/creating views, loading their definitions, saving
//! them, previewing/explaining them and dropping them. A view designer is a [`QueryTab`] whose
//! `view` is set, so it reuses the SQL editor and the tab strip.

use super::*;

impl AppView {
    /// Open (or focus) the designer for an existing view, loading its definition.
    pub(super) fn open_view(
        &mut self,
        connection_index: usize,
        database: String,
        name: String,
        cx: &mut Context<'_, Self>,
    ) {
        if self.connection_arc(connection_index).is_none() {
            return;
        }
        if let Some(index) = self.queries.iter().position(|tab| {
            tab.connection_index == Some(connection_index)
                && tab.database.as_deref() == Some(database.as_str())
                && tab
                    .view
                    .as_ref()
                    .is_some_and(|view| view.original_name.as_deref() == Some(name.as_str()))
        }) {
            self.activate_query(index, cx);
            return;
        }

        let id = self.next_query_id;
        self.next_query_id += 1;
        let mut tab = QueryTab::new(id);
        tab.name = Some(name.clone());
        tab.connection_index = Some(connection_index);
        tab.database = Some(database);
        let mut state = ViewTabState::new(name.clone());
        state.original_name = Some(name);
        tab.view = Some(state);
        self.queries.push(tab);
        let index = self.queries.len() - 1;
        self.active_query = Some(index);
        self.active_design = None;
        self.active_grid = None;
        self.query_focus_pending = true;
        self.refresh_view_details(index, cx);
        cx.notify();
    }

    /// Open a blank designer for a new view. The view is not created until the user saves.
    pub(super) fn open_new_view(
        &mut self,
        connection_index: usize,
        database: String,
        cx: &mut Context<'_, Self>,
    ) {
        if self.connection_arc(connection_index).is_none() {
            return;
        }
        let name = self.unique_view_name(connection_index, &database);
        let template = view_template(&name);
        let id = self.next_query_id;
        self.next_query_id += 1;
        let mut tab = QueryTab::new(id);
        tab.name = Some(name.clone());
        tab.connection_index = Some(connection_index);
        tab.database = Some(database);
        tab.baseline = template.clone();
        tab.sql = template;
        tab.view = Some(ViewTabState::new(name));
        self.queries.push(tab);
        let index = self.queries.len() - 1;
        self.active_query = Some(index);
        self.active_design = None;
        self.active_grid = None;
        self.query_focus_pending = true;
        cx.notify();
    }

    /// A view name not already taken in `database`, e.g. `new_view`, `new_view_1`.
    fn unique_view_name(&self, connection_index: usize, database: &str) -> String {
        let existing: Vec<String> = self
            .connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => {
                    databases.iter().find(|existing| existing.name == database)
                }
                _ => None,
            })
            .and_then(|database| match &database.tables {
                Loadable::Loaded(tables) => Some(
                    tables
                        .iter()
                        .filter(|table| matches!(table.kind, rustgrid_core::ObjectKind::View))
                        .map(|table| table.name.clone())
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default();
        let base = "new_view";
        if !existing.iter().any(|name| name == base) {
            return base.to_string();
        }
        let mut counter = 1;
        loop {
            let candidate = format!("{base}_{counter}");
            if !existing.iter().any(|name| name == &candidate) {
                return candidate;
            }
            counter += 1;
        }
    }

    /// (Re)load one view designer's definition and metadata.
    pub(super) fn refresh_view_details(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.queries.get(index) else {
            return;
        };
        let Some(view) = tab.view.as_ref() else {
            return;
        };
        let Some(name) = view.original_name.clone() else {
            return;
        };
        let Some(connection) = tab.connection_index.and_then(|i| self.connection_arc(i)) else {
            return;
        };
        let database = tab.database.clone().unwrap_or_default();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.view_details(&database, &name).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let _ = this.update(cx, |app, cx| {
                match result {
                    Ok(details) => {
                        if let Some(tab) = app.queries.get_mut(index) {
                            tab.sql = details.definition.clone();
                            tab.baseline = tab.sql.clone();
                            tab.caret = 0;
                            tab.anchor = 0;
                            if let Some(view) = tab.view.as_mut() {
                                view.name = details.info.name.clone();
                                view.details = Some(details);
                            }
                        }
                    }
                    Err(error) => app.error_dialog = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Switch the active view designer's sub-tab.
    pub(super) fn select_view_tab(&mut self, tab: ViewTab, cx: &mut Context<'_, Self>) {
        if let Some(view) = self
            .active_query
            .and_then(|index| self.queries.get_mut(index))
            .and_then(|tab| tab.view.as_mut())
        {
            view.tab = tab;
        }
        cx.notify();
    }

    /// The SQL the Save button will run, for the designer's SQL 预览 tab.
    pub(super) fn view_preview_sql(&self) -> Option<String> {
        let index = self.active_query?;
        let tab = self.queries.get(index)?;
        let view = tab.view.as_ref()?;
        let connection = tab.connection_index.and_then(|i| self.connection_arc(i))?;
        let database = tab.database.clone().unwrap_or_default();
        let dialect = tab
            .connection_index
            .map(|index| self.driver_dialect(index))
            .unwrap_or_default();
        let name = sql::view_identity_for(&tab.sql, dialect).unwrap_or_else(|| view.name.clone());
        let edit = ViewEdit {
            name,
            definition: tab.sql.clone(),
        };
        Some(connection.view_sql(&database, view.original_name.as_deref(), &edit))
    }

    /// Save the active view designer: drop the original (when it exists) and run the editor's
    /// `CREATE` statement.
    pub(super) fn save_view(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            return;
        };
        let Some(view) = tab.view.as_ref() else {
            return;
        };
        if view.saving {
            return;
        }
        let Some(connection) = tab.connection_index.and_then(|i| self.connection_arc(i)) else {
            return;
        };
        let database = tab.database.clone().unwrap_or_default();
        let definition = tab.sql.clone();
        let dialect = tab
            .connection_index
            .map(|index| self.driver_dialect(index))
            .unwrap_or_default();
        let (name, original) = (
            sql::view_identity_for(&definition, dialect).unwrap_or_else(|| view.name.clone()),
            view.original_name.clone(),
        );
        let edit = ViewEdit {
            name: name.clone(),
            definition,
        };
        if let Some(view) = self
            .queries
            .get_mut(index)
            .and_then(|tab| tab.view.as_mut())
        {
            view.saving = true;
        }
        let runtime = self.runtime.clone();
        let list_connection = connection.clone();
        let list_database = database.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    connection
                        .save_view(&database, original.as_deref(), &edit)
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let _ = this.update(cx, |app, cx| {
                match result {
                    Ok(()) => {
                        let mut notify: Option<String> = None;
                        if let Some(tab) = app.queries.get_mut(index) {
                            tab.name = Some(name.clone());
                            tab.baseline = tab.sql.clone();
                            if let Some(view) = tab.view.as_mut() {
                                view.name = name.clone();
                                view.original_name = Some(name.clone());
                                view.saving = false;
                            }
                            notify = Some(t!("view.saved", name = name.clone()).to_string());
                        }
                        app.reload_database_tables(&list_connection, &list_database, cx);
                        app.refresh_view_details(index, cx);
                        if let Some(message) = notify {
                            app.toast(ToastKind::Success, message, cx);
                        }
                    }
                    Err(error) => {
                        if let Some(view) =
                            app.queries.get_mut(index).and_then(|tab| tab.view.as_mut())
                        {
                            view.saving = false;
                        }
                        app.error_dialog = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 预览: open the saved view's data in a grid.
    pub(super) fn preview_view(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            return;
        };
        let Some(view) = tab.view.as_ref() else {
            return;
        };
        let (Some(connection_index), Some(database), Some(name)) = (
            tab.connection_index,
            tab.database.clone(),
            view.original_name.clone(),
        ) else {
            return;
        };
        self.select_table(connection_index, database, name, true, cx);
        self.active_query = None;
        self.active_design = None;
        cx.notify();
    }

    /// Switch the bottom explain panel's sub-tab.
    pub(super) fn select_view_explain_tab(
        &mut self,
        tab: ViewExplainTab,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(view) = self
            .active_query
            .and_then(|index| self.queries.get_mut(index))
            .and_then(|tab| tab.view.as_mut())
        {
            view.explain_tab = tab;
        }
        cx.notify();
    }

    /// 解释: run `EXPLAIN` over the view definition's query and show the plan in the designer's
    /// bottom 信息/解释 panel.
    pub(super) fn explain_view(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            return;
        };
        let Some(view) = tab.view.as_ref() else {
            return;
        };
        if view.explain_running {
            return;
        }
        let Some(connection) = tab.connection_index.and_then(|i| self.connection_arc(i)) else {
            return;
        };
        let connection_name = tab
            .connection_index
            .and_then(|i| self.connections.get(i))
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let database = tab.database.clone().unwrap_or_default();
        let dialect = tab
            .connection_index
            .map(|index| self.driver_dialect(index))
            .unwrap_or_default();
        let source = sql::view_select_for(&tab.sql, dialect)
            .unwrap_or_else(|| format!("SELECT * FROM {}", quote_identifier(&view.name)));
        let explain_sql = format!("EXPLAIN\n{source};");

        // Re-running 解释 replaces the previous plan.
        self.remove_view_explain_grid(index, cx);
        if let Some(view) = self
            .queries
            .get_mut(index)
            .and_then(|tab| tab.view.as_mut())
        {
            view.explain_open = true;
            view.explain_tab = ViewExplainTab::Result;
            view.explain_sql = explain_sql.clone();
            view.explain_running = true;
            view.explain_elapsed = None;
            view.explain_error = None;
        }
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let query_connection = connection.clone();
            let query_sql = explain_sql.clone();
            let query_database = Some(database.clone());
            let started = std::time::Instant::now();
            let result = match runtime
                .spawn(async move {
                    query_connection
                        .execute_query(query_database.as_deref(), &query_sql)
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let elapsed = started.elapsed();
            let _ = this.update(cx, |app, cx| {
                app.apply_view_explain_result(
                    index,
                    connection_name,
                    database,
                    explain_sql,
                    elapsed,
                    result,
                    cx,
                );
            });
        })
        .detach();
    }

    /// Store one 解释 run's outcome and build the plan grid.
    #[allow(clippy::too_many_arguments)]
    fn apply_view_explain_result(
        &mut self,
        index: usize,
        connection_name: String,
        database: String,
        sql: String,
        elapsed: std::time::Duration,
        result: Result<QueryResult, Error>,
        cx: &mut Context<'_, Self>,
    ) {
        self.remove_view_explain_grid(index, cx);

        if let Ok(query_result) = &result
            && query_result.has_result_set
        {
            let Some(connection) = self
                .queries
                .get(index)
                .and_then(|tab| tab.connection_index)
                .and_then(|i| self.connection_arc(i))
            else {
                return;
            };
            let columns = query_result.columns.clone();
            let rows = query_result.rows.clone();
            let total = rows.len() as u64;
            let column_widths = compute_column_widths(&columns, &rows);
            let id = self.next_grid_id;
            self.next_grid_id += 1;
            let state = GridState {
                id,
                connection,
                connection_name,
                database,
                table: String::new(),
                is_view: true,
                page_index: 0,
                page_size: total.max(1),
                loading: false,
                error: None,
                columns,
                rows: Arc::new(rows),
                column_widths,
                manual_column_widths: false,
                total_rows: Some(total),
                selection: None,
                edits: BTreeMap::new(),
                undo: Vec::new(),
                redo: Vec::new(),
                sql: Some(sql),
                show_toolbar: false,
                show_footer: false,
                editable: false,
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
            if let Some(view) = self
                .queries
                .get_mut(index)
                .and_then(|tab| tab.view.as_mut())
            {
                view.explain_grid_id = Some(id);
                view.explain_running = false;
                view.explain_elapsed = Some(elapsed);
                view.explain_error = None;
                view.explain_tab = ViewExplainTab::Result;
            }
        } else {
            let error = match result {
                Ok(query_result) => format!(
                    "{} {}",
                    t!("view.explain.no_result"),
                    query_result.rows_affected
                ),
                Err(error) => error.to_string(),
            };
            if let Some(view) = self
                .queries
                .get_mut(index)
                .and_then(|tab| tab.view.as_mut())
            {
                view.explain_running = false;
                view.explain_elapsed = Some(elapsed);
                view.explain_error = Some(error);
            }
        }
        cx.notify();
    }

    /// Drop the explain plan grid of one designer tab, if it has one.
    fn remove_view_explain_grid(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(id) = self
            .queries
            .get(index)
            .and_then(|tab| tab.view.as_ref())
            .and_then(|view| view.explain_grid_id)
        else {
            return;
        };
        if let Some(position) = self
            .grids
            .iter()
            .position(|grid| grid.read(cx).state.id == id)
        {
            self.grids.remove(position);
        }
        if let Some(view) = self
            .queries
            .get_mut(index)
            .and_then(|tab| tab.view.as_mut())
        {
            view.explain_grid_id = None;
        }
    }

    /// Ask for confirmation before dropping a view.
    pub(super) fn confirm_delete_view(
        &mut self,
        connection_index: usize,
        database_index: usize,
        name: String,
        cx: &mut Context<'_, Self>,
    ) {
        self.delete_confirm = Some(DeleteConfirm::View {
            connection_index,
            database_index,
            label: name.clone(),
            name,
        });
        cx.notify();
    }

    /// Drop a view and refresh the list and any open designer/grid for it.
    pub(super) fn delete_view(
        &mut self,
        connection_index: usize,
        database_index: usize,
        name: String,
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
                .spawn(async move { connection.drop_view(&database, &name).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let _ = this.update(cx, |app, cx| match result {
                Ok(()) => {
                    app.close_table_views(&close_connection, &close_database, &close_name, cx);
                    // Close the designer tab for the dropped view, if any.
                    if let Some(index) = app.queries.iter().position(|tab| {
                        tab.view.as_ref().is_some_and(|view| {
                            view.original_name.as_deref() == Some(close_name.as_str())
                        })
                    }) {
                        app.close_query(index, cx);
                    }
                    app.reload_tables(connection_index, database_index, cx);
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
}

/// The default body template for a brand-new view.
pub(super) fn view_template(name: &str) -> String {
    format!(
        "CREATE VIEW {} AS\nSELECT\n    *\nFROM\n    `table_name`;",
        quote_identifier(name)
    )
}

fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}
