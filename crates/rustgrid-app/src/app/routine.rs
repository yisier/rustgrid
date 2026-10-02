//! The `Functions` main tab and the stored-routine editor: opening/creating routines, loading
//! their definitions, saving them, running them and dropping them. A routine editor is a
//! [`QueryTab`] whose `routine` is set, so it reuses the SQL editor and the tab strip.

use super::*;

impl AppView {
    /// Open (or focus) the editor for an existing routine, loading its definition.
    pub(super) fn open_routine(
        &mut self,
        connection_index: usize,
        database: String,
        info: RoutineInfo,
        cx: &mut Context<'_, Self>,
    ) {
        if self.connection_arc(connection_index).is_none() {
            return;
        }
        if let Some(index) = self.queries.iter().position(|tab| {
            tab.connection_index == Some(connection_index)
                && tab.database.as_deref() == Some(database.as_str())
                && tab.routine.as_ref().is_some_and(|routine| {
                    routine.original_name.as_deref() == Some(info.name.as_str())
                        && routine.kind == info.kind
                })
        }) {
            self.activate_query(index, cx);
            return;
        }

        let id = self.next_query_id;
        self.next_query_id += 1;
        let mut tab = QueryTab::new(id);
        tab.name = Some(info.name.clone());
        tab.connection_index = Some(connection_index);
        tab.database = Some(database);
        let mut state = RoutineTabState::new(info.kind, info.name.clone());
        state.original_name = Some(info.name.clone());
        state.original_kind = Some(info.kind);
        tab.routine = Some(state);
        self.queries.push(tab);
        let index = self.queries.len() - 1;
        self.active_query = Some(index);
        self.active_design = None;
        self.active_grid = None;
        self.query_focus_pending = true;
        self.refresh_routine_details(index, cx);
        cx.notify();
    }

    /// Open a routine by name, e.g. from the connection tree, when only its identity is known.
    pub(super) fn open_routine_by_name(
        &mut self,
        connection_index: usize,
        database: String,
        name: String,
        kind: RoutineKind,
        cx: &mut Context<'_, Self>,
    ) {
        let info = self
            .connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases
                    .iter()
                    .find(|existing| existing.name == database)
                    .and_then(|database| match &database.routines {
                        Loadable::Loaded(routines) => routines
                            .iter()
                            .find(|routine| routine.name == name && routine.kind == kind)
                            .cloned(),
                        _ => None,
                    }),
                _ => None,
            })
            .unwrap_or_else(|| RoutineInfo {
                name: name.clone(),
                kind,
                ..RoutineInfo::default()
            });
        self.open_routine(connection_index, database, info, cx);
    }

    /// Open a blank editor for a new routine. The routine is not created until the user saves.
    pub(super) fn open_new_routine(
        &mut self,
        connection_index: usize,
        database: String,
        kind: RoutineKind,
        cx: &mut Context<'_, Self>,
    ) {
        if self.connection_arc(connection_index).is_none() {
            return;
        }
        let name = self.unique_routine_name(connection_index, &database, kind);
        let template = routine_template(kind, &name);
        let id = self.next_query_id;
        self.next_query_id += 1;
        let mut tab = QueryTab::new(id);
        tab.name = Some(name.clone());
        tab.connection_index = Some(connection_index);
        tab.database = Some(database);
        tab.sql = template;
        tab.routine = Some(RoutineTabState::new(kind, name));
        self.queries.push(tab);
        let index = self.queries.len() - 1;
        self.active_query = Some(index);
        self.active_design = None;
        self.active_grid = None;
        self.query_focus_pending = true;
        cx.notify();
    }

    /// A routine name not already taken in `database`, e.g. `new_function`, `new_function_1`.
    fn unique_routine_name(
        &self,
        connection_index: usize,
        database: &str,
        kind: RoutineKind,
    ) -> String {
        let base = match kind {
            RoutineKind::Function => "new_function",
            RoutineKind::Procedure => "new_procedure",
        };
        let existing: Vec<String> = self
            .connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => {
                    databases.iter().find(|existing| existing.name == database)
                }
                _ => None,
            })
            .and_then(|database| match &database.routines {
                Loadable::Loaded(routines) => Some(
                    routines
                        .iter()
                        .map(|routine| routine.name.clone())
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default();
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

    /// (Re)load one routine tab's definition and metadata.
    pub(super) fn refresh_routine_details(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(tab) = self.queries.get(index) else {
            return;
        };
        let Some(routine) = tab.routine.as_ref() else {
            return;
        };
        let Some(name) = routine.original_name.clone() else {
            return;
        };
        let kind = routine.original_kind.unwrap_or(routine.kind);
        let Some(connection) = tab.connection_index.and_then(|i| self.connection_arc(i)) else {
            return;
        };
        let database = tab.database.clone().unwrap_or_default();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.routine_details(&database, kind, &name).await })
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
                            tab.caret = 0;
                            tab.anchor = 0;
                            if let Some(routine) = tab.routine.as_mut() {
                                routine.name = details.info.name.clone();
                                routine.kind = details.info.kind;
                                routine.details = Some(details);
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

    /// Ensure a database's routines are loaded, e.g. when the Functions tab becomes active.
    pub(super) fn ensure_routines_loaded(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let needs = self
            .connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index).map(|database| {
                    matches!(database.routines, Loadable::Idle | Loadable::Failed(_))
                }),
                _ => None,
            })
            .unwrap_or(false);
        if !needs {
            return;
        }
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let Some(name) = self.database_name(connection_index, database_index) else {
            return;
        };
        self.load_routines(connection_index, database_index, connection, name, cx);
    }

    /// Switch the active routine editor's sub-tab.
    pub(super) fn select_routine_tab(&mut self, tab: RoutineTab, cx: &mut Context<'_, Self>) {
        if let Some(routine) = self
            .active_query
            .and_then(|index| self.queries.get_mut(index))
            .and_then(|tab| tab.routine.as_mut())
        {
            routine.tab = tab;
        }
        cx.notify();
    }

    /// Toggle the definition editor's word wrap.
    pub(super) fn toggle_routine_word_wrap(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(routine) = self
            .active_query
            .and_then(|index| self.queries.get_mut(index))
            .and_then(|tab| tab.routine.as_mut())
        {
            routine.word_wrap = !routine.word_wrap;
        }
        cx.notify();
    }

    /// Show or hide the routine editor's find bar.
    pub(super) fn toggle_routine_find(&mut self, cx: &mut Context<'_, Self>) {
        let mut query = String::new();
        let mut opening = false;
        if let Some(routine) = self
            .active_query
            .and_then(|index| self.queries.get_mut(index))
            .and_then(|tab| tab.routine.as_mut())
        {
            routine.find_open = !routine.find_open;
            opening = routine.find_open;
            query = routine.find_query.clone();
        }
        if opening {
            let input = self.routine_find_input.clone();
            input.update(cx, |input, cx| input.set_text(query, cx));
        }
        cx.notify();
    }

    /// Move the caret to the next occurrence of the active routine tab's find text.
    pub(super) fn routine_find_next(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            return;
        };
        let Some(routine) = tab.routine.as_ref() else {
            return;
        };
        let needle = routine.find_query.clone();
        if needle.is_empty() {
            return;
        }
        let haystack = tab.sql.to_lowercase();
        let needle_lower = needle.to_lowercase();
        let start = tab.caret.min(tab.sql.len());
        let found = haystack[start..]
            .find(&needle_lower)
            .map(|offset| start + offset)
            .or_else(|| haystack.find(&needle_lower));
        let Some(found) = found else {
            return;
        };
        if let Some(tab) = self.queries.get_mut(index) {
            tab.anchor = found;
            tab.caret = found + needle.len();
            tab.selecting = false;
        }
        self.caret_visible = true;
        cx.notify();
    }

    /// The SQL the Save button will run, for the routine editor's SQL 预览 tab.
    pub(super) fn routine_preview_sql(&self) -> Option<String> {
        let index = self.active_query?;
        let tab = self.queries.get(index)?;
        let routine = tab.routine.as_ref()?;
        let connection = tab.connection_index.and_then(|i| self.connection_arc(i))?;
        let database = tab.database.clone().unwrap_or_default();
        let (kind, name) =
            sql::routine_identity(&tab.sql).unwrap_or_else(|| (routine.kind, routine.name.clone()));
        let edit = RoutineEdit {
            kind,
            name,
            definition: tab.sql.clone(),
        };
        let original = routine.original_name.clone().zip(routine.original_kind);
        let original_ref = original.as_ref().map(|(name, kind)| (name.as_str(), *kind));
        Some(connection.routine_sql(&database, original_ref, &edit))
    }

    /// Save the active routine editor: drop the original (when it exists) and run the editor's
    /// `CREATE` statement.
    pub(super) fn save_routine(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            return;
        };
        let Some(routine) = tab.routine.as_ref() else {
            return;
        };
        if routine.saving {
            return;
        }
        let Some(connection) = tab.connection_index.and_then(|i| self.connection_arc(i)) else {
            return;
        };
        let database = tab.database.clone().unwrap_or_default();
        let definition = tab.sql.clone();
        let (kind, name) = sql::routine_identity(&definition)
            .unwrap_or_else(|| (routine.kind, routine.name.clone()));
        let original = routine.original_name.clone().zip(routine.original_kind);
        let edit = RoutineEdit {
            kind,
            name: name.clone(),
            definition,
        };
        if let Some(routine) = self
            .queries
            .get_mut(index)
            .and_then(|tab| tab.routine.as_mut())
        {
            routine.saving = true;
        }
        let runtime = self.runtime.clone();
        let list_connection = connection.clone();
        let list_database = database.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    let original_ref = original.as_ref().map(|(name, kind)| (name.as_str(), *kind));
                    connection
                        .save_routine(&database, original_ref, &edit)
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
                        if let Some(tab) = app.queries.get_mut(index) {
                            tab.name = Some(name.clone());
                            if let Some(routine) = tab.routine.as_mut() {
                                routine.name = name.clone();
                                routine.kind = kind;
                                routine.original_name = Some(name.clone());
                                routine.original_kind = Some(kind);
                                routine.saving = false;
                            }
                        }
                        app.reload_routines(&list_connection, &list_database, cx);
                        app.refresh_routine_details(index, cx);
                    }
                    Err(error) => {
                        if let Some(routine) = app
                            .queries
                            .get_mut(index)
                            .and_then(|tab| tab.routine.as_mut())
                        {
                            routine.saving = false;
                        }
                        app.error_dialog = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Run the active routine. A parameterless routine is called straight away; one with
    /// parameters opens a query tab pre-filled with the call so the arguments can be supplied.
    pub(super) fn run_routine(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            return;
        };
        let Some(routine) = tab.routine.as_ref() else {
            return;
        };
        let Some(connection_index) = tab.connection_index else {
            return;
        };
        let database = tab.database.clone().unwrap_or_default();
        let definition = tab.sql.clone();
        let (kind, name) = sql::routine_identity(&definition)
            .unwrap_or_else(|| (routine.kind, routine.name.clone()));
        let parameters = sql::routine_parameters(&definition);
        self.run_routine_call(connection_index, database, kind, name, &parameters, cx);
    }

    /// Run a routine identified only by name/kind (the Functions toolbar's 运行函数).
    pub(super) fn run_routine_by_name(
        &mut self,
        connection_index: usize,
        database: String,
        kind: RoutineKind,
        name: String,
        cx: &mut Context<'_, Self>,
    ) {
        self.run_routine_call(connection_index, database, kind, name, &[], cx);
    }

    /// Open a query tab for `CALL`/`SELECT` and run it when the routine takes no arguments.
    fn run_routine_call(
        &mut self,
        connection_index: usize,
        database: String,
        kind: RoutineKind,
        name: String,
        parameters: &[String],
        cx: &mut Context<'_, Self>,
    ) {
        let call = routine_call_sql(&database, kind, &name, parameters);
        let id = self.next_query_id;
        self.next_query_id += 1;
        let mut tab = QueryTab::new(id);
        tab.connection_index = Some(connection_index);
        tab.database = Some(database);
        tab.sql = call;
        tab.caret = 0;
        tab.anchor = 0;
        self.queries.push(tab);
        let index = self.queries.len() - 1;
        self.activate_query(index, cx);
        if parameters.is_empty() {
            self.run_query(false, cx);
        }
    }

    /// Ask for confirmation before dropping a routine.
    pub(super) fn confirm_delete_routine(
        &mut self,
        connection_index: usize,
        database_index: usize,
        name: String,
        kind: RoutineKind,
        cx: &mut Context<'_, Self>,
    ) {
        self.delete_confirm = Some(DeleteConfirm::Routine {
            connection_index,
            database_index,
            label: name.clone(),
            name,
            kind,
        });
        cx.notify();
    }

    /// Drop a routine and refresh the list and any open editor tab for it.
    pub(super) fn delete_routine(
        &mut self,
        connection_index: usize,
        database_index: usize,
        name: String,
        kind: RoutineKind,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        let runtime = self.runtime.clone();
        let list_connection = connection.clone();
        let list_database = database.clone();
        let tab_name = name.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.drop_routine(&database, kind, &name).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let _ = this.update(cx, |app, cx| match result {
                Ok(()) => {
                    // Close the editor tab for the dropped routine, if any.
                    if let Some(index) = app.queries.iter().position(|tab| {
                        tab.routine.as_ref().is_some_and(|routine| {
                            routine.original_name.as_deref() == Some(tab_name.as_str())
                                && routine.kind == kind
                        })
                    }) {
                        app.close_query(index, cx);
                    }
                    // Drop the routine info selection when it pointed at the routine just removed.
                    if app.info_routine_selected.as_ref().is_some_and(
                        |(_, selected_database, selected_name, selected_kind)| {
                            *selected_database == database_index
                                && selected_name == &tab_name
                                && *selected_kind == kind
                        },
                    ) {
                        app.clear_info_selection();
                    }
                    app.reload_routines(&list_connection, &list_database, cx);
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

/// The default body template for a brand-new routine.
pub(super) fn routine_template(kind: RoutineKind, name: &str) -> String {
    match kind {
        RoutineKind::Function => format!(
            "CREATE FUNCTION {}(p1 int)\nRETURNS int\nBEGIN\n    RETURN 0;\nEND",
            quote_identifier(name)
        ),
        RoutineKind::Procedure => format!(
            "CREATE PROCEDURE {}()\nBEGIN\n    #Routine body goes here...\nEND",
            quote_identifier(name)
        ),
    }
}

/// The statement that runs a routine: `SELECT` for a function, `CALL` for a procedure. Parameters
/// are emitted as comments the user fills in.
fn routine_call_sql(
    database: &str,
    kind: RoutineKind,
    name: &str,
    parameters: &[String],
) -> String {
    let qualified = format!("{}.{}", quote_identifier(database), quote_identifier(name));
    let arguments = parameters
        .iter()
        .map(|parameter| format!("/* {parameter} */"))
        .collect::<Vec<_>>()
        .join(", ");
    if kind.is_procedure() {
        format!("CALL {qualified}({arguments});")
    } else {
        format!("SELECT {qualified}({arguments});")
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}
