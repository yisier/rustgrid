//! Workspace session restore ("return to where I was"): the open connections, query tabs (with
//! unsaved text, "hot exit") and table tabs survive a restart.
//!
//! The session is saved about a second after anything it records changes (debounced, so typing
//! does not write on every keystroke) and once more when the app quits. Restoring is lazy, like
//! VS Code's editor restore: query tabs come back at once without touching a database; a table
//! tab reopens only when its connection connects (automatically for a connection with a saved
//! password, otherwise when the user connects it), so the start costs no queries.

use std::hash::{Hash, Hasher};
use std::time::Duration;

use rustgrid_config::{
    SessionActive, SessionDatabase, SessionQuery, SessionTable, WorkspaceSession,
};

use super::*;

/// How long the session waits after a change before it is written.
const SESSION_SAVE_DELAY: Duration = Duration::from_millis(1000);

impl AppView {
    /// What is open right now, as a session. Tables still waiting for their connection are kept,
    /// so starting the app without connecting does not drop them from the next session.
    fn session_snapshot(&self, cx: &App) -> WorkspaceSession {
        let connections = self
            .connections
            .iter()
            .filter(|node| matches!(node.status, ConnectionStatus::Connected(_)))
            .map(|node| node.profile.id.clone())
            .collect();
        let queries = self
            .queries
            .iter()
            .map(|tab| {
                // A saved query without unsaved edits reloads from its file.
                let clean_file = tab.saved_path.is_some() && tab.sql == tab.baseline;
                SessionQuery {
                    connection_id: tab
                        .connection_index
                        .and_then(|index| self.connections.get(index))
                        .map(|node| node.profile.id.clone()),
                    database: tab.database.clone(),
                    schema: tab.schema.clone(),
                    name: tab.name.clone(),
                    saved_path: tab.saved_path.clone(),
                    sql: (!clean_file).then(|| tab.sql.clone()),
                    caret: tab.caret,
                }
            })
            .collect();
        let mut tables: Vec<SessionTable> = Vec::new();
        let mut active = self.active_query.map(SessionActive::Query);
        for grid in &self.grids {
            let state = &grid.read(cx).state;
            // Query result grids belong to their query tab and are not restored.
            if state.sql.is_some() {
                continue;
            }
            if self.active_query.is_none()
                && self
                    .active_grid
                    .is_some_and(|index| self.grids.get(index) == Some(grid))
            {
                active = Some(SessionActive::Table(tables.len()));
            }
            tables.push(SessionTable {
                connection_id: state.connection_id.clone(),
                database: state.database.clone(),
                table: state.table.clone(),
                is_view: state.is_view,
                page_index: state.page_index,
            });
        }
        tables.extend(self.pending_session_tables.iter().cloned());
        let mut databases = self.open_session_databases();
        databases.extend(self.pending_session_databases.iter().cloned());
        let current_database = self
            .object_pane
            .as_ref()
            .and_then(|pane| {
                let pane = pane.read(cx);
                let node = self.connections.get(pane.connection_index)?;
                Some(SessionDatabase {
                    connection_id: node.profile.id.clone(),
                    database: self.database_name(pane.connection_index, pane.database_index)?,
                })
            })
            .or_else(|| self.pending_session_current.clone());
        WorkspaceSession {
            connections,
            queries,
            tables,
            active,
            databases,
            current_database,
        }
    }

    /// The databases open in the tree right now.
    fn open_session_databases(&self) -> Vec<SessionDatabase> {
        let mut open = Vec::new();
        for node in &self.connections {
            if let Loadable::Loaded(databases) = &node.databases {
                for database in databases.iter().filter(|database| database.opened) {
                    open.push(SessionDatabase {
                        connection_id: node.profile.id.clone(),
                        database: database.name.clone(),
                    });
                }
            }
        }
        open
    }

    /// A cheap fingerprint of what the session records (not the caret, which moves constantly).
    fn session_signature(&self, cx: &App) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for node in &self.connections {
            matches!(node.status, ConnectionStatus::Connected(_)).hash(&mut hasher);
        }
        for tab in &self.queries {
            (
                tab.connection_index,
                &tab.database,
                &tab.schema,
                &tab.name,
                &tab.saved_path,
                &tab.sql,
            )
                .hash(&mut hasher);
        }
        for grid in &self.grids {
            let state = &grid.read(cx).state;
            (
                state.sql.is_some(),
                &state.connection_id,
                &state.database,
                &state.table,
                state.page_index,
            )
                .hash(&mut hasher);
        }
        (self.active_query, self.active_grid).hash(&mut hasher);
        self.pending_session_tables.len().hash(&mut hasher);
        self.open_session_databases().hash(&mut hasher);
        self.object_pane
            .as_ref()
            .map(|pane| {
                let pane = pane.read(cx);
                (pane.connection_index, pane.database_index)
            })
            .hash(&mut hasher);
        hasher.finish()
    }

    /// Called on render: once the session has been restored, schedule a save when what it records
    /// changed. The save runs `SESSION_SAVE_DELAY` later, so a burst of changes writes once.
    pub(super) fn track_session(&mut self, cx: &mut Context<'_, Self>) {
        if !self.session_restored {
            return;
        }
        let signature = self.session_signature(cx);
        if signature == self.session_signature_saved || self.session_save_pending {
            return;
        }
        self.session_save_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SESSION_SAVE_DELAY).await;
            let _ = this.update(cx, |view, cx| {
                view.session_save_pending = false;
                view.save_session_now(cx);
            });
        })
        .detach();
    }

    /// Write the session now (also from the quit hook).
    pub(super) fn save_session_now(&mut self, cx: &mut Context<'_, Self>) {
        if !self.session_restored {
            return;
        }
        let session = self.session_snapshot(cx);
        self.session_signature_saved = self.session_signature(cx);
        // A failed write is not worth a dialog: the next change tries again.
        let _ = self.config.save_session(&session);
    }

    /// Restore the last session at startup: the query tabs at once, the tables once their
    /// connections connect (connections with a saved password reconnect now).
    pub(super) fn restore_session(&mut self, cx: &mut Context<'_, Self>) {
        let session = self.config.load_session();
        for query in &session.queries {
            let connection_index = query.connection_id.as_ref().and_then(|id| {
                self.connections
                    .iter()
                    .position(|node| &node.profile.id == id)
            });
            self.open_query_with(
                connection_index,
                query.database.clone(),
                query.schema.clone(),
                cx,
            );
            let file_text = query
                .saved_path
                .as_ref()
                .and_then(|path| std::fs::read_to_string(path).ok());
            if let Some(active) = self.active_query
                && let Some(tab) = self.queries.get_mut(active)
            {
                tab.name = query.name.clone();
                tab.saved_path = query.saved_path.clone().filter(|_| file_text.is_some());
                tab.baseline = file_text.clone().unwrap_or_default();
                tab.sql = query.sql.clone().or(file_text).unwrap_or_default();
                tab.caret = query.caret.min(tab.sql.len());
                tab.anchor = tab.caret;
            }
        }
        self.pending_session_tables = session.tables;
        self.pending_session_active = session.active;
        self.pending_session_databases = session.databases;
        self.pending_session_current = session.current_database;
        if let Some(SessionActive::Query(index)) = session.active
            && index < self.queries.len()
        {
            self.activate_query(index, cx);
        }
        // Reconnect what was open, but only where no password prompt would interrupt the start.
        let reconnect: Vec<usize> = session
            .connections
            .iter()
            .filter_map(|id| {
                self.connections
                    .iter()
                    .position(|node| &node.profile.id == id && node.password.is_some())
            })
            .collect();
        self.session_restored = true;
        self.session_signature_saved = self.session_signature(cx);
        for index in reconnect {
            self.connect(index, cx);
        }
        cx.notify();
    }

    /// After connection `index` listed its databases: reopen the session's open databases on it
    /// (loading their objects, as a double-click would) and show the current database's objects,
    /// without leaving the tab that is active, so a new query defaults to that database again.
    pub(super) fn restore_session_databases(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(profile_id) = self
            .connections
            .get(index)
            .map(|node| node.profile.id.clone())
        else {
            return;
        };
        let (mine, rest): (Vec<SessionDatabase>, Vec<SessionDatabase>) = self
            .pending_session_databases
            .drain(..)
            .partition(|database| database.connection_id == profile_id);
        self.pending_session_databases = rest;
        let current = self
            .pending_session_current
            .take_if(|current| current.connection_id == profile_id);
        if mine.is_empty() && current.is_none() {
            return;
        }
        let Some(connection) = self.connection_arc(index) else {
            return;
        };
        for wanted in mine.iter().chain(current.iter()) {
            let Some(database_index) = self.database_index_by_name(index, &wanted.database) else {
                continue;
            };
            let mut load = false;
            if let Some(node) = self.connections.get_mut(index)
                && let Loadable::Loaded(databases) = &mut node.databases
                && let Some(database) = databases.get_mut(database_index)
                && !database.opened
            {
                database.opened = true;
                database.expanded = true;
                load = matches!(database.tables, Loadable::Idle | Loadable::Failed(_));
            }
            if load {
                self.load_tables(
                    index,
                    database_index,
                    connection.clone(),
                    wanted.database.clone(),
                    cx,
                );
            }
        }
        if let Some(current) = current
            && let Some(database_index) = self.database_index_by_name(index, &current.database)
            && !self.driver_supports(index, DriverCapability::Schemas)
        {
            self.open_object_pane(index, database_index, None, Category::Tables, cx);
            self.tree_pane.update(cx, |tree, _| {
                tree.selected = Some(format!("db-{index}-{database_index}"));
            });
        }
        cx.notify();
    }

    /// After connection `index` connects: reopen the session's tables on it (lazily, now that
    /// they can load), returning to the tab that was active.
    pub(super) fn restore_session_tables(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(profile_id) = self
            .connections
            .get(index)
            .map(|node| node.profile.id.clone())
        else {
            return;
        };
        let active_table = match self.pending_session_active {
            Some(SessionActive::Table(position)) => {
                self.pending_session_tables.get(position).cloned()
            }
            _ => None,
        };
        let (mine, rest): (Vec<SessionTable>, Vec<SessionTable>) = self
            .pending_session_tables
            .drain(..)
            .partition(|table| table.connection_id == profile_id);
        self.pending_session_tables = rest;
        if mine.is_empty() {
            return;
        }
        let previous_query = self.active_query;
        for table in &mine {
            self.select_table(
                index,
                table.database.clone(),
                table.table.clone(),
                table.is_view,
                cx,
            );
            if table.page_index > 0
                && let Some(grid) = self
                    .active_grid
                    .and_then(|grid| self.grids.get(grid))
                    .cloned()
            {
                let page = table.page_index;
                grid.update(cx, |grid, cx| {
                    grid.state.page_index = page;
                    grid.load_page(cx);
                });
            }
        }
        // Return to the tab that was active: that table, or the query the user was in.
        if let Some(active) = active_table.filter(|table| table.connection_id == profile_id) {
            self.select_table(index, active.database, active.table, active.is_view, cx);
            self.pending_session_active = None;
        } else if let Some(query) = previous_query {
            self.activate_query(query, cx);
        }
        cx.notify();
    }
}
