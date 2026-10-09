//! The SQL query editor and the result 信息 tab.
//!
//! The editor itself is gpui-kit's `Editor`, owned per query tab through [`ui::SqlEditor`]. This
//! module only manages those entities (create on first use, mirror their text into `QueryTab::sql`,
//! re-scope completion to the tab's connection/database) and renders the 信息 tab. Text editing,
//! undo/redo, multi-cursor, search and syntax highlighting all come from the wrapped editor.

use std::rc::Rc;

use super::*;

impl AppView {
    /// Ensure a [`ui::SqlEditor`] exists for the query at `index`, scoped to its connection and
    /// database, and return it.
    pub(super) fn ensure_query_editor(
        &mut self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> Option<Entity<ui::SqlEditor>> {
        let tab = self.queries.get(index)?;
        let sql = tab.sql.clone();
        let connection_index = tab.connection_index;
        let database = tab.database.clone();
        let schema = tab.schema.clone();
        let supports_schemas = connection_index
            .is_some_and(|index| self.driver_supports(index, DriverCapability::Schemas));
        let dialect = connection_index
            .and_then(|index| self.connections.get(index))
            .and_then(|node| self.registry.get(&node.profile.driver))
            .map(|driver| driver.dialect())
            .unwrap_or_default();

        if self.query_editors.len() <= index {
            self.query_editors.resize_with(index + 1, || None);
        }
        if let Some(editor) = self.query_editors[index].clone() {
            // Keep the completion scope in step with the tab's connection/database/schema.
            editor.update(cx, |editor, cx| {
                editor.set_scope(
                    connection_index,
                    database.clone(),
                    schema.clone(),
                    supports_schemas,
                    dialect,
                    cx,
                )
            });
            return Some(editor);
        }

        let source = sql_completion::CompletionSource::new(self.completion_catalog.clone())
            .with_scope(
                connection_index,
                database.clone(),
                schema.clone(),
                supports_schemas,
                dialect,
            );
        let scope_handle = source.scope_handle();
        let provider: Rc<dyn gpui_kit::component::input::CompletionProvider> =
            Rc::new(sql_completion::SqlCompletionProvider::new(source));
        let options = ui::SqlEditorOptions {
            language: "sql".into(),
            font_family: self.editor_font().into(),
            font_size: self.editor_font_size(),
            line_number: self.editor_line_numbers,
            readonly: false,
        };
        let weak = cx.weak_entity();
        let editor = cx.new(|cx| {
            ui::SqlEditor::new(cx)
                .options(options)
                .provider(provider)
                .scope(scope_handle)
                .on_change(Rc::new(move |text, cx| {
                    let _ = weak.update(cx, |app, cx| app.on_query_editor_changed(index, text, cx));
                }))
        });
        editor.update(cx, |editor, cx| editor.set_text(sql, cx));
        self.query_editors[index] = Some(editor.clone());
        Some(editor)
    }

    /// Mirror an edit from the wrapped editor into the tab, so run/save/format read current text.
    fn on_query_editor_changed(&mut self, index: usize, text: &str, cx: &mut Context<'_, Self>) {
        let connection_index = self.queries.get(index).and_then(|tab| tab.connection_index);
        let database = self.queries.get(index).and_then(|tab| tab.database.clone());
        let schema = self.queries.get(index).and_then(|tab| tab.schema.clone());
        if let Some(tab) = self.queries.get_mut(index) {
            tab.sql = text.to_string();
        }
        // Warm the completion catalog's columns for every table the statement now references, so
        // `alias.`/`table.` completion has types and comments ready.
        if let Some(connection_index) = connection_index {
            let scope = sql_completion::CompletionScope {
                connection_index: Some(connection_index),
                database,
                schema,
                supports_schemas: self.driver_supports(connection_index, DriverCapability::Schemas),
                dialect: self
                    .connections
                    .get(connection_index)
                    .and_then(|node| self.registry.get(&node.profile.driver))
                    .map(|driver| driver.dialect())
                    .unwrap_or_default(),
            };
            let referenced =
                sql::referenced_tables(&sql::current_statement(text, text.len(), scope.dialect));
            for table in referenced {
                if let Some((database, table)) = sql_completion::catalog_target(&scope, &table) {
                    self.ensure_query_columns(cx, connection_index, &database, &table);
                }
            }
        }
        cx.notify();
    }

    /// Drop a closed tab's editor and shift the ones after it down.
    pub(super) fn remove_query_editor(&mut self, index: usize) {
        if index < self.query_editors.len() {
            self.query_editors.remove(index);
        }
    }

    /// Push a tab's current text into its editor (after programmatic changes like 美化SQL or
    /// loading a saved query).
    pub(super) fn sync_query_editor_text(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(text) = self.queries.get(index).map(|tab| tab.sql.clone()) else {
            return;
        };
        if let Some(Some(editor)) = self.query_editors.get(index)
            && editor.read(cx).text() != text
        {
            let editor = editor.clone();
            editor.update(cx, |editor, cx| editor.set_text(text, cx));
        }
    }

    /// The active query tab's editor, rendering it as the editor surface.
    pub(super) fn render_query_editor(
        &mut self,
        query_index: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(editor) = self.ensure_query_editor(query_index, cx) else {
            return div().into_any_element();
        };
        let focus = self.query_focus.clone();
        let index = query_index;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .bg(rgb(theme.editor_bg))
            .track_focus(&focus)
            .key_context(QUERY_EDITOR_CONTEXT)
            .on_action(cx.listener(move |this, _: &SaveQuery, window, cx| {
                // Bring the tab that owns this editor to the front, then save it (in place when it
                // is already bound to a file, otherwise through the name/location dialog).
                this.activate_query(index, cx);
                // A view/routine designer reuses this editor; Ctrl+S must save the definition
                // (save_view/save_routine), not prompt to save a free-form query file.
                let (is_routine, is_view) = this
                    .queries
                    .get(index)
                    .map(|tab| (tab.routine.is_some(), tab.view.is_some()))
                    .unwrap_or((false, false));
                if is_routine {
                    this.save_routine(cx);
                } else if is_view {
                    this.save_view(cx);
                } else {
                    this.begin_save_query(window, cx);
                }
            }))
            .on_action(cx.listener(move |this, _: &RunSelectedQuery, _window, cx| {
                // Bring the tab that owns this editor to the front, then run its selection.
                this.activate_query(index, cx);
                this.run_query(true, cx);
            }))
            .child(editor)
            .into_any_element()
    }

    /// A read-only SQL preview (routine/view 预览 tabs), rendered with the wrapped gpui-kit editor.
    /// The entity is cached by `key` so scroll position and syntax highlighting survive re-renders.
    pub(super) fn render_sql_preview(
        &mut self,
        key: &str,
        sql: &str,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        if let Some(editor) = self.preview_editors.get(key).cloned() {
            if editor.read(cx).text() != sql {
                let sql = sql.to_string();
                editor.update(cx, |editor, cx| editor.set_text(sql, cx));
            }
            return editor.into_any_element();
        }
        let options = ui::SqlEditorOptions {
            language: "sql".into(),
            font_family: self.editor_font().into(),
            font_size: self.editor_font_size(),
            line_number: false,
            readonly: true,
        };
        let sql = sql.to_string();
        let editor = cx.new(|cx| ui::SqlEditor::new(cx).options(options));
        editor.update(cx, |editor, cx| editor.set_text(sql, cx));
        self.preview_editors.insert(key.to_string(), editor.clone());
        editor.into_any_element()
    }

    /// Refresh the shared completion catalog from every loaded connection catalog.
    pub(super) fn refresh_completion_catalog(&self) {
        let mut catalog = sql_completion::CompletionCatalog::default();
        for (connection_index, node) in self.connections.iter().enumerate() {
            if let Loadable::Loaded(databases) = &node.databases {
                for database in databases {
                    if let Loadable::Loaded(tables) = &database.tables {
                        let names: Vec<String> =
                            tables.iter().map(|table| table.name.clone()).collect();
                        catalog
                            .tables
                            .insert((connection_index, database.name.clone()), names);
                    }
                    if let Loadable::Loaded(routines) = &database.routines {
                        catalog.functions.insert(
                            connection_index,
                            routines
                                .iter()
                                .filter(|routine| routine.kind == RoutineKind::Function)
                                .map(|routine| routine.name.clone())
                                .collect(),
                        );
                    }
                }
            }
        }
        // Carry over the on-demand column cache (loaded by the editor's completion).
        if let Ok(mut target) = self.completion_catalog.write() {
            for (key, entry) in self.query_column_cache.borrow().iter() {
                if let ColumnCacheEntry::Loaded(columns) = entry {
                    target.columns.insert(
                        key.clone(),
                        columns
                            .iter()
                            .map(|column| {
                                (
                                    column.name.clone(),
                                    column.data_type.clone(),
                                    column.comment.clone(),
                                )
                            })
                            .collect(),
                    );
                }
            }
            target.tables = catalog.tables;
            target.functions = catalog.functions;
        }
    }

    /// The 信息 tab: the executed script and one status line per statement result.
    pub(super) fn render_query_info(&self, query_index: usize) -> AnyElement {
        let theme = self.theme;
        let Some(query) = self.queries.get(query_index) else {
            return div().into_any_element();
        };
        if let Some(error) = &query.result_error {
            return div()
                .flex_1()
                .p_2()
                .text_color(rgb(theme.danger))
                .child(error.clone())
                .into_any_element();
        }
        let mut body = div().flex().flex_col().gap_2().p_2().w_full();
        for (index, result) in query.results.iter().enumerate() {
            let status = if result.has_result_set {
                t!("query.result_rows", count = result.row_count).to_string()
            } else if result.rows_affected > 0 {
                t!("query.rows_affected", count = result.rows_affected).to_string()
            } else {
                t!("query.executed").to_string()
            };
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(rgb(theme.text_muted))
                            .child(format!("{} {}", t!("query.statement"), index + 1)),
                    )
                    .child(div().text_size(px(11.5)).child(status)),
            );
        }
        if let Some(elapsed) = query.last_elapsed {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(format!(
                        "{}: {:.3}s",
                        t!("query.elapsed"),
                        elapsed.as_secs_f64()
                    )),
            );
        }
        div()
            .id("query-info")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(body)
            .into_any_element()
    }
}
