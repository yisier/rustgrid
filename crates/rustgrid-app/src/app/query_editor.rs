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
        let key_value = connection_index
            .is_some_and(|index| self.driver_supports(index, DriverCapability::KeyValue));
        let documents = connection_index
            .is_some_and(|index| self.driver_supports(index, DriverCapability::Documents));
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
                    key_value,
                    documents,
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
                key_value,
            );
        let loader_app = cx.weak_entity();
        let source = source.with_column_loader(Rc::new(move |cx, connection, database, table| {
            let _ = loader_app.update(cx, |app, cx| {
                app.ensure_query_columns(cx, connection, &database, &table);
            });
        }));
        let scope_handle = source.scope_handle();
        let provider: Rc<dyn gpui_kit::component::input::CompletionProvider> =
            Rc::new(sql_completion::SqlCompletionProvider::new(source));
        let options = ui::SqlEditorOptions {
            language: if documents {
                "json"
            } else if key_value {
                "redis"
            } else {
                "sql"
            }
            .into(),
            font_family: self.editor_font().into(),
            font_size: self.editor_font_size(),
            line_number: self.editor_line_numbers,
            readonly: false,
        };
        // A document command is JSON, not SQL: attaching the SQL completion provider there would
        // offer keywords that cannot appear in the document.
        let completion = (!documents).then_some((provider, scope_handle));
        let weak = cx.weak_entity();
        let zoom_weak = weak.clone();
        let editor = cx.new(|cx| {
            let editor = ui::SqlEditor::new(cx).options(options);
            let editor = match completion {
                Some((provider, scope)) => editor.provider(provider).scope(scope),
                None => editor,
            };
            editor
                .on_change(Rc::new(move |text, cx| {
                    let _ = weak.update(cx, |app, cx| app.on_query_editor_changed(index, text, cx));
                }))
                // Ctrl + wheel resizes the editor font (Options → 编辑器).
                .on_zoom(Rc::new(move |step, cx| {
                    zoom_weak
                        .update(cx, |app, cx| app.zoom_editor_font(step, cx))
                        .unwrap_or(false)
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
            let key_value = self.driver_supports(connection_index, DriverCapability::KeyValue);
            let documents = self.driver_supports(connection_index, DriverCapability::Documents);
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
                key_value,
            };
            // Every statement's tables, not only the last one's: the caret may be in any. Redis
            // commands and MongoDB JSON documents are not SQL, so there are no tables to warm.
            if !key_value && !documents {
                let referenced = sql::referenced_tables(text);
                for table in referenced {
                    if let Some((database, table)) = sql_completion::catalog_target(&scope, &table)
                    {
                        self.ensure_query_columns(cx, connection_index, &database, &table);
                    }
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
            .on_action(cx.listener(move |this, _: &RunQuery, _window, cx| {
                // 运行 (Navicat's Ctrl+R): the selection when there is one, else the whole editor.
                this.activate_query(index, cx);
                this.run_query(RunScope::SelectionOrAll, cx);
            }))
            .on_action(cx.listener(move |this, _: &RunSelectedQuery, _window, cx| {
                // Bring the tab that owns this editor to the front, then run its selection.
                this.activate_query(index, cx);
                this.run_query(RunScope::SelectionOrStatement, cx);
            }))
            .on_action(cx.listener(move |this, _: &AcceptCompletion, window, cx| {
                // Tab confirms the completion popup exactly like Enter. The kit has no Tab
                // handling of its own, so when no popup is open the keystroke is left to
                // propagate and the editor's `IndentInline` binding runs instead.
                let editor = this.query_editors.get(index).cloned().flatten();
                if let Some(editor) = editor
                    && editor.update(cx, |editor, cx| editor.accept_completion(window, cx))
                {
                    cx.stop_propagation();
                }
            }))
            .on_action(
                cx.listener(move |this, _: &SelectAllOccurrences, _window, cx| {
                    // Ctrl+Shift+L: put a cursor on every occurrence of the current word/selection.
                    this.activate_query(index, cx);
                    if let Some(Some(editor)) = this.query_editors.get(index) {
                        let editor = editor.clone();
                        editor.update(cx, |editor, cx| editor.select_all_occurrences(cx));
                    }
                }),
            )
            // Line commands (keymap): each is a pure `line_edit` function applied as one undoable
            // replacement.
            .on_action(cx.listener(move |this, _: &ToggleComment, window, cx| {
                let hash = this.query_uses_hash_comments(index);
                this.edit_query_lines(index, window, cx, move |text, selection| {
                    Some(line_edit::toggle_comment(text, selection, hash))
                });
            }))
            .on_action(cx.listener(move |this, _: &DuplicateLines, window, cx| {
                this.edit_query_lines(index, window, cx, |text, selection| {
                    Some(line_edit::duplicate(text, selection))
                });
            }))
            .on_action(cx.listener(move |this, _: &SwapLineUp, window, cx| {
                this.edit_query_lines(index, window, cx, line_edit::swap_with_previous);
            }))
            .on_action(cx.listener(move |this, _: &MoveLinesUp, window, cx| {
                this.edit_query_lines(index, window, cx, |text, selection| {
                    line_edit::move_lines(text, selection, true)
                });
            }))
            .on_action(cx.listener(move |this, _: &MoveLinesDown, window, cx| {
                this.edit_query_lines(index, window, cx, |text, selection| {
                    line_edit::move_lines(text, selection, false)
                });
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
        self.render_sql_preview_marked(key, sql, Vec::new(), cx)
    }

    /// A read-only SQL preview with extra highlights (the transfer window's DDL diff panes). The
    /// wrapper implies selection and Ctrl+C, which hand-drawn text rows never got.
    pub(super) fn render_sql_preview_marked(
        &mut self,
        key: &str,
        sql: &str,
        decorations: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)>,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        if let Some(editor) = self.preview_editors.get(key).cloned() {
            let text_changed = editor.read(cx).text() != sql;
            let decorations_changed = self.preview_decorations.get(key) != Some(&decorations);
            if text_changed || decorations_changed {
                let sql = sql.to_string();
                let applied = decorations.clone();
                editor.update(cx, |editor, cx| {
                    if text_changed {
                        editor.set_text(sql, cx);
                    }
                    editor.set_decorations(applied, cx);
                });
                self.preview_decorations
                    .insert(key.to_string(), decorations);
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
        let weak = cx.weak_entity();
        let editor = cx.new(|cx| {
            ui::SqlEditor::new(cx)
                .options(options)
                .on_zoom(Rc::new(move |step, cx| {
                    weak.update(cx, |app, cx| app.zoom_editor_font(step, cx))
                        .unwrap_or(false)
                }))
        });
        let applied = decorations.clone();
        editor.update(cx, |editor, cx| {
            editor.set_text(sql, cx);
            editor.set_decorations(applied, cx);
        });
        self.preview_decorations
            .insert(key.to_string(), decorations);
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
    pub(super) fn render_query_info(
        &mut self,
        query_index: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let Some(query) = self.queries.get(query_index) else {
            return div().into_any_element();
        };
        let id = query.id;
        let skipped = query.skipped_statements;
        let ran = query.results.len();
        let mysql = self.query_uses_hash_comments(query_index);
        let (text, spans) = query_info_text(query, mysql);
        let theme = self.theme;
        let decorations = info_decorations(&spans, theme, cx);
        let editor = self
            .query_info_editors
            .entry(id)
            .or_insert_with(|| {
                cx.new(|cx| {
                    ui::SqlEditor::new(cx).options(ui::SqlEditorOptions {
                        language: "text".into(),
                        // The result/信息 log is status text, not the query being edited: it
                        // stays at 12px when the editor font is zoomed or changed in Options.
                        font_size: 12.0,
                        line_number: false,
                        readonly: true,
                        ..Default::default()
                    })
                })
            })
            .clone();
        if editor.read(cx).text() != text {
            editor.update(cx, |editor, cx| {
                editor.set_text(text, cx);
                editor.set_decorations(decorations, cx);
            });
        }
        // The statement limit's notice: what was skipped, and a link to the setting.
        let notice = (skipped > 0).then(|| {
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(theme.border))
                .bg(rgba(theme.warning << 8 | 0x22))
                .text_size(px(12.0))
                .child(t!("query.limit_notice", ran = ran, skipped = skipped).to_string())
                .child(
                    div()
                        .id("query-limit-link")
                        .text_color(rgb(theme.primary))
                        .underline()
                        .cursor_pointer()
                        .child(t!("query.limit_link").to_string())
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.open_options_at(
                                OptionsSection::General,
                                super::options::OPTIONS_MAX_STATEMENTS,
                                cx,
                            );
                        })),
                )
        });
        div()
            .id("query-info")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .children(notice)
            .child(editor)
            .into_any_element()
    }
}

/// What a span of the 信息 log is, for colouring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum InfoSpan {
    /// SQL text, coloured like the editor.
    Sql(sql_lex::TokenKind),
    /// Where in the statement the server's error points (`near '…'`).
    ErrorLocation,
    /// An error line.
    Error,
    /// The quoted part of an error line the server points at.
    ErrorFocus,
    /// Outcome and time lines.
    Muted,
}

/// Turn the log's spans into editor decorations in the theme's colours.
fn info_decorations(
    spans: &[(std::ops::Range<usize>, InfoSpan)],
    theme: Theme,
    cx: &App,
) -> Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> {
    let highlight = &gpui_kit::component::Theme::global(cx).highlight_theme;
    let danger: gpui::Hsla = rgb(theme.danger).into();
    let muted: gpui::Hsla = rgb(theme.text_muted).into();
    spans
        .iter()
        .filter_map(|(range, span)| {
            let style = match span {
                InfoSpan::Sql(kind) => highlight.style(match kind {
                    sql_lex::TokenKind::Keyword => "keyword",
                    sql_lex::TokenKind::Function => "function",
                    sql_lex::TokenKind::String => "string",
                    sql_lex::TokenKind::Number => "number",
                    sql_lex::TokenKind::Comment => "comment",
                })?,
                InfoSpan::ErrorLocation => gpui::HighlightStyle {
                    underline: Some(gpui::UnderlineStyle {
                        color: Some(danger),
                        thickness: px(1.5),
                        wavy: true,
                    }),
                    background_color: Some(danger.opacity(0.15)),
                    ..Default::default()
                },
                InfoSpan::Error => gpui::HighlightStyle {
                    color: Some(danger),
                    ..Default::default()
                },
                InfoSpan::ErrorFocus => gpui::HighlightStyle {
                    color: Some(danger),
                    font_weight: Some(FontWeight::BOLD),
                    background_color: Some(danger.opacity(0.15)),
                    ..Default::default()
                },
                InfoSpan::Muted => gpui::HighlightStyle {
                    color: Some(muted),
                    ..Default::default()
                },
            };
            Some((range.clone(), style))
        })
        .collect()
}

/// The part of a server error that points into the statement: MySQL's `near '…' at line N` and
/// PostgreSQL's `at or near "…"`. Returns the byte range of the quoted text inside `error`.
fn error_focus(error: &str) -> Option<std::ops::Range<usize>> {
    for (open, close) in [("near '", '\''), ("near \"", '"')] {
        if let Some(at) = error.find(open) {
            let start = at + open.len();
            // MySQL quotes the rest of the statement, which may itself contain quotes: the
            // closing quote is the one before ` at line`, else the last one.
            let end = error[start..]
                .find(&format!("{close} at line"))
                .map(|offset| start + offset)
                .or_else(|| error.rfind(close).filter(|&end| end >= start))?;
            return Some(start..end);
        }
    }
    None
}

/// The 信息 tab's text, as Navicat's Message tab writes it: each statement, then right under it
/// its outcome (rows, affected rows or the error, in red) and its time; plus the spans to colour.
fn query_info_text(
    query: &QueryTab,
    mysql: bool,
) -> (String, Vec<(std::ops::Range<usize>, InfoSpan)>) {
    let mut out = String::new();
    let mut spans = Vec::new();
    let push_line = |out: &mut String, spans: &mut Vec<_>, line: &str, span: InfoSpan| {
        let start = out.len();
        out.push_str(line);
        spans.push((start..out.len(), span));
        out.push('\n');
    };
    if let Some(error) = &query.result_error {
        push_line(&mut out, &mut spans, error, InfoSpan::Error);
        if let Some(elapsed) = query.last_elapsed {
            let line = format!("> {}: {}", t!("query.elapsed"), format_elapsed(elapsed));
            push_line(&mut out, &mut spans, &line, InfoSpan::Muted);
        }
        return (out, spans);
    }
    for result in &query.results {
        let statement = result.statement.trim();
        let statement_start = out.len();
        out.push_str(statement);
        for (range, kind) in sql_lex::tokens(statement, mysql) {
            spans.push((
                statement_start + range.start..statement_start + range.end,
                InfoSpan::Sql(kind),
            ));
        }
        out.push('\n');
        match &result.error {
            Some(error) => {
                let line = format!("> {error}");
                let line_start = out.len();
                push_line(&mut out, &mut spans, &line, InfoSpan::Error);
                if let Some(focus) = error_focus(&line) {
                    let quoted = &line[focus.clone()];
                    spans.push((
                        line_start + focus.start..line_start + focus.end,
                        InfoSpan::ErrorFocus,
                    ));
                    // Point at the same place in the statement (MySQL quotes the statement from
                    // there on; an empty quote means its end).
                    let location = if quoted.is_empty() {
                        statement.len().saturating_sub(1)..statement.len()
                    } else {
                        let first_line = quoted.lines().next().unwrap_or(quoted);
                        match statement.find(first_line) {
                            Some(at) => at..at + first_line.len(),
                            None => 0..0,
                        }
                    };
                    if !location.is_empty() {
                        spans.push((
                            statement_start + location.start..statement_start + location.end,
                            InfoSpan::ErrorLocation,
                        ));
                    }
                }
            }
            None => {
                let outcome = if result.has_result_set {
                    t!("query.result_rows", count = result.row_count).to_string()
                } else if result.rows_affected > 0 {
                    t!("query.rows_affected", count = result.rows_affected).to_string()
                } else {
                    t!("query.executed").to_string()
                };
                push_line(
                    &mut out,
                    &mut spans,
                    &format!("> {outcome}"),
                    InfoSpan::Muted,
                );
            }
        }
        let time = format!(
            "> {}: {}",
            t!("query.elapsed"),
            format_elapsed(result.elapsed)
        );
        push_line(&mut out, &mut spans, &time, InfoSpan::Muted);
        out.push('\n');
    }
    if query.results.len() > 1
        && let Some(elapsed) = query.last_elapsed
    {
        let line = format!("{}: {}", t!("query.total_elapsed"), format_elapsed(elapsed));
        push_line(&mut out, &mut spans, &line, InfoSpan::Muted);
    }
    (out, spans)
}

impl AppView {
    /// Apply a line edit to query tab `index`'s editor (see [`line_edit`]).
    fn edit_query_lines(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        edit: impl FnOnce(&str, std::ops::Range<usize>) -> Option<line_edit::LineEdit>,
    ) {
        if let Some(Some(editor)) = self.query_editors.get(index) {
            let editor = editor.clone();
            editor.update(cx, |editor, cx| editor.edit_lines(edit, window, cx));
        }
    }

    /// Whether query tab `index` runs against MySQL / MariaDB, where `#` also starts a comment
    /// (so the comment toggle removes it).
    pub(super) fn query_uses_hash_comments(&self, index: usize) -> bool {
        self.queries
            .get(index)
            .and_then(|tab| tab.connection_index)
            .is_some_and(|connection| {
                self.driver_dialect(connection) == rustgrid_core::DriverDialect::Mysql
            })
    }

    /// The toolbar's run timer: live while the query runs, then how long the last run took.
    pub(super) fn query_timer(&mut self, index: usize, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(tab) = self.queries.get(index) else {
            return div().into_any_element();
        };
        let (elapsed, running) = match (tab.run_started, tab.last_elapsed) {
            (Some(started), _) if tab.running => (Some(started.elapsed()), true),
            (_, last) => (last, false),
        };
        if running && !self.query_timer_scheduled {
            // Tick about ten times a second while the query runs.
            self.query_timer_scheduled = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let _ = this.update(cx, |view, cx| {
                    view.query_timer_scheduled = false;
                    cx.notify();
                });
            })
            .detach();
        }
        div()
            .flex_none()
            .px_2()
            .text_size(px(11.5))
            .text_color(rgb(if running {
                theme.text
            } else {
                theme.text_muted
            }))
            .children(elapsed.map(format_elapsed))
            .into_any_element()
    }
}

/// A run time as the query toolbar and the 信息 tab show it: milliseconds under a minute
/// (`835 ms`, `12450 ms`), then minutes and seconds (`2 min 05 s`).
pub(super) fn format_elapsed(elapsed: std::time::Duration) -> String {
    let millis = elapsed.as_millis();
    if millis < 60_000 {
        format!("{millis} ms")
    } else {
        let seconds = elapsed.as_secs();
        format!("{} min {:02} s", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::format_elapsed;
    use std::time::Duration;

    #[test]
    fn the_error_points_at_the_quoted_part() {
        let error = "1064 (42000): You have an error in your SQL syntax; check the manual that \
                     corresponds to your MySQL server version for the right syntax to use near \
                     'FORM t' at line 1";
        let focus = super::error_focus(error).expect("focus");
        assert_eq!(&error[focus], "FORM t");
        let error = "syntax error at or near \"FORM\"";
        assert_eq!(&error[super::error_focus(error).expect("focus")], "FORM");
        assert!(super::error_focus("Table 'x' doesn't exist").is_none());
    }

    #[test]
    fn run_times_read_in_milliseconds_then_minutes() {
        assert_eq!(format_elapsed(Duration::from_micros(835_400)), "835 ms");
        assert_eq!(format_elapsed(Duration::from_millis(12_450)), "12450 ms");
        assert_eq!(format_elapsed(Duration::from_secs(125)), "2 min 05 s");
    }
}
