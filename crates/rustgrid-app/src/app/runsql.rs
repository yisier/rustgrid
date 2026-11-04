//! *Run SQL File*: right-click a database and run an external `.sql` script against it.
//!
//! The script is decoded with the chosen encoding, split into statements by the driver's own
//! scanner ([`Connection::split_statements`], so PL/SQL blocks, dollar-quoted bodies and T-SQL
//! batches survive) and executed one statement at a time. A failing statement is either reported
//! and skipped (遇到错误时继续) or aborts the run. Progress and the log mirror Navicat's window.

use super::backup::{format_count, format_hms, log_summary_row, now_timestamp, render_log_lines};
use super::file_dialog::open_dialog;
use super::*;

impl AppView {
    /// The database context menu's 运行 SQL 文件 entry: open the run window on this database.
    pub(super) fn open_run_sql_file(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if self.connection_arc(connection_index).is_none() {
            self.error_dialog = Some(t!("sqlfile.not_connected").to_string());
            cx.notify();
            return;
        }
        // A schema-qualified engine needs a default schema for the dialog's 模式 row; use the
        // database's first schema when it has been loaded.
        let schema = if self.driver_supports(connection_index, DriverCapability::Schemas) {
            self.connections
                .get(connection_index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => databases.get(database_index),
                    _ => None,
                })
                .and_then(|database| match &database.schemas {
                    Some(Loadable::Loaded(schemas)) => schemas.first().cloned(),
                    _ => None,
                })
                .unwrap_or_default()
        } else {
            String::new()
        };

        self.new_backup_dialog = None;
        self.restore_dialog = None;
        self.extract_dialog = None;
        self.dump_dialog = None;

        // The 文件 field and 编码 dropdown are built here (the wrappers defer their inner gpui-kit
        // state to the first render), so the dialog body never creates entities while rendering.
        let theme = self.theme;
        let weak = cx.weak_entity();
        let input_app = weak.clone();
        let file_input = cx.new(move |cx| {
            TextInput::new(theme, String::new(), TextInputOptions::default(), cx).on_change(
                Rc::new(move |text, _window, cx| {
                    let text = text.to_string();
                    let _ = input_app.update(cx, |app, cx| app.run_sql_set_file_path(text, cx));
                }),
            )
        });
        let options: Vec<ComboOption> = RUN_SQL_ENCODINGS
            .iter()
            .map(|encoding| ComboOption::new(encoding.label, encoding.label))
            .collect();
        let current = RUN_SQL_ENCODINGS[0].label.to_string();
        let combo_app = weak.clone();
        let encoding_combo = cx.new(move |cx| {
            ComboBox::new(theme, options, current, 220.0, cx)
                .field_width(220.0)
                .on_select(Rc::new(move |value: &str, _window, cx| {
                    let value = value.to_string();
                    let _ = combo_app.update(cx, |app, cx| app.run_sql_set_encoding(&value, cx));
                }))
        });

        self.run_sql_dialog = Some(RunSqlDialog {
            connection_index,
            database_index,
            schema,
            file_path: None,
            encoding: 0,
            continue_on_error: true,
            tab: BackupDialogTab::Objects,
            running: false,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            log: Vec::new(),
            log_scroll: ScrollHandle::new(),
            total: 0,
            success: 0,
            failed: 0,
            records: 0,
            started: None,
            elapsed: None,
            error: None,
            file_input: Some(file_input),
            encoding_combo: Some(encoding_combo),
        });
        self.open_backup_window(t!("sqlfile.run_title").to_string(), cx);
        cx.notify();
    }

    /// Track the 文件 field's text as the chosen script path.
    pub(super) fn run_sql_set_file_path(&mut self, text: String, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.run_sql_dialog.as_mut() {
            let trimmed = text.trim().to_string();
            dialog.file_path = if trimmed.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(trimmed))
            };
            dialog.error = None;
        }
        cx.notify();
    }

    /// Choose the script file through the system open dialog.
    pub(super) fn run_sql_browse_file(&mut self, cx: &mut Context<'_, Self>) {
        if self.run_sql_dialog.is_none() {
            return;
        }
        let title = t!("sqlfile.choose_file").to_string();
        let default_dir = super::export::default_export_dir();
        cx.spawn(async move |this, cx| {
            let mut dialog = open_dialog(title, "SQL", &["sql", "txt"]);
            if !default_dir.is_empty() {
                dialog = dialog.set_directory(&default_dir);
            }
            let Some(handle) = dialog.pick_file().await else {
                return;
            };
            let path = handle.path().to_path_buf();
            let _ = this.update(cx, |app, cx| {
                let text = path.to_string_lossy().into_owned();
                let input = app
                    .run_sql_dialog
                    .as_mut()
                    .and_then(|dialog| dialog.file_input.clone());
                app.run_sql_set_file_path(text.clone(), cx);
                if let Some(input) = input {
                    input.update(cx, |input, cx| input.set_text(text, cx));
                }
            });
        })
        .detach();
    }

    /// Track the 编码 dropdown's choice.
    pub(super) fn run_sql_set_encoding(&mut self, label: &str, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.run_sql_dialog.as_mut()
            && let Some(index) = RUN_SQL_ENCODINGS
                .iter()
                .position(|encoding| encoding.label == label)
        {
            dialog.encoding = index;
        }
        cx.notify();
    }

    /// Toggle 遇到错误时继续.
    pub(super) fn run_sql_toggle_continue(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.run_sql_dialog.as_mut() {
            dialog.continue_on_error = !dialog.continue_on_error;
        }
        cx.notify();
    }

    /// Switch the Run SQL dialog's tab (常规 / 信息日志).
    pub(super) fn run_sql_set_tab(&mut self, tab: BackupDialogTab, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.run_sql_dialog.as_mut() {
            dialog.tab = tab;
        }
        cx.notify();
    }

    /// Run the chosen script: decode it, split it with the driver's scanner, then execute the
    /// statements one at a time with live progress and a log.
    pub(super) fn run_sql_file(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.run_sql_dialog.as_ref() else {
            return;
        };
        if dialog.running {
            return;
        }
        let connection_index = dialog.connection_index;
        let database_index = dialog.database_index;
        let Some(path) = dialog.file_path.clone() else {
            if let Some(dialog) = self.run_sql_dialog.as_mut() {
                dialog.error = Some(t!("sqlfile.no_file").to_string());
            }
            cx.notify();
            return;
        };
        let encoding = RUN_SQL_ENCODINGS[dialog.encoding].encoding;
        let continue_on_error = dialog.continue_on_error;
        let Some(connection) = self.connection_arc(connection_index) else {
            if let Some(dialog) = self.run_sql_dialog.as_mut() {
                dialog.error = Some(t!("sqlfile.not_connected").to_string());
            }
            cx.notify();
            return;
        };
        let database = self.database_name(connection_index, database_index);

        if let Some(dialog) = self.run_sql_dialog.as_mut() {
            dialog.running = true;
            dialog
                .cancel
                .store(false, std::sync::atomic::Ordering::Relaxed);
            dialog.error = None;
            dialog.tab = BackupDialogTab::Log;
            dialog.log.clear();
            dialog.total = 0;
            dialog.success = 0;
            dialog.failed = 0;
            dialog.records = 0;
            dialog.elapsed = None;
            dialog.started = Some(std::time::Instant::now());
            dialog
                .log
                .push(t!("backup.log.started", time = now_timestamp()).to_string());
            dialog.log_scroll.scroll_to_bottom();
        }

        let runtime = self.runtime.clone();
        let cancel = self
            .run_sql_dialog
            .as_ref()
            .map(|dialog| dialog.cancel.clone())
            .unwrap_or_default();

        cx.spawn(async move |this, cx| {
            let bytes = {
                let read_path = path.clone();
                runtime
                    .spawn_blocking(move || std::fs::read(&read_path))
                    .await
            };
            let bytes = match bytes {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |app, cx| {
                        app.finish_run_sql(Some(error.to_string()), cx)
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update(cx, |app, cx| {
                        app.finish_run_sql(Some(error.to_string()), cx)
                    });
                    return;
                }
            };

            let (text, _, had_errors) = encoding.decode(&bytes);
            let statements = connection.split_statements(&text);
            let total = statements.len();
            let _ = this.update(cx, |app, cx| {
                if let Some(dialog) = app.run_sql_dialog.as_mut() {
                    dialog.total = total;
                    if had_errors {
                        dialog.log.push(t!("sqlfile.decode_lossy").to_string());
                    }
                    dialog
                        .log
                        .push(t!("sqlfile.run_start", count = total).to_string());
                    dialog.log_scroll.scroll_to_bottom();
                }
                cx.notify();
            });

            // Open a pinned session so a session-scoped statement (MySQL's
            // `SET FOREIGN_KEY_CHECKS`, PostgreSQL's `SET`, …) applies to every later statement.
            // Drivers that cannot pin a connection return `None` and we fall back to
            // `execute_query` per statement. The guard releases the session on the runtime (see
            // [`ScriptSessionGuard`]), which is what keeps finishing a run from aborting the app.
            let session = {
                let connection = connection.clone();
                let database = database.clone();
                runtime
                    .spawn(async move { connection.script_session(database.as_deref()).await })
                    .await
            };
            let mut session = ScriptSessionGuard {
                runtime: runtime.clone(),
                session: match session {
                    Ok(Ok(session)) => session,
                    _ => None,
                },
            };

            for (index, statement) in statements.into_iter().enumerate() {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let result = if let Some(active) = session.take() {
                    let sql = statement.clone();
                    match runtime
                        .spawn(async move {
                            let mut active = active;
                            let result = active.execute(&sql).await;
                            (result, active)
                        })
                        .await
                    {
                        Ok((result, returned)) => {
                            session.restore(returned);
                            result
                        }
                        Err(error) => Err(Error::other(error)),
                    }
                } else {
                    let connection = connection.clone();
                    let database = database.clone();
                    let sql = statement.clone();
                    match runtime
                        .spawn(
                            async move { connection.execute_query(database.as_deref(), &sql).await },
                        )
                        .await
                    {
                        Ok(result) => result,
                        Err(error) => Err(Error::other(error)),
                    }
                };
                let (failed, records) = match result {
                    Ok(result) => (None, result.rows_affected as usize),
                    Err(error) => (Some(error.to_string()), 0),
                };
                let stop = failed.is_some() && !continue_on_error;
                let _ = this.update(cx, |app, cx| {
                    app.record_run_sql_result(index, statement, failed, records, cx);
                });
                if stop {
                    break;
                }
            }

            // The session is released by the guard's `Drop` (on the runtime), so every exit path —
            // the loop ending, a stop, or the whole task being dropped — is covered.
            drop(session);

            let cancelled = cancel.load(std::sync::atomic::Ordering::Relaxed);
            let _ = this.update(cx, |app, cx| {
                if cancelled {
                    app.backup_log(t!("backup.log.cancelled").to_string(), cx);
                }
                app.finish_run_sql(None, cx)
            });
        })
        .detach();
    }

    /// Record one statement's outcome for the running script.
    fn record_run_sql_result(
        &mut self,
        index: usize,
        statement: String,
        failed: Option<String>,
        records: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.run_sql_dialog.as_mut() {
            match failed {
                Some(error) => {
                    dialog.failed += 1;
                    let snippet = statement_snippet(&statement);
                    dialog.log.push(
                        t!(
                            "sqlfile.run_statement_failed",
                            index = index + 1,
                            statement = snippet,
                            error = error
                        )
                        .to_string(),
                    );
                }
                None => {
                    dialog.success += 1;
                    dialog.records += records;
                }
            }
            dialog.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn finish_run_sql(&mut self, fatal: Option<String>, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.run_sql_dialog.as_mut() {
            dialog.running = false;
            if let Some(error) = &fatal {
                dialog.error = Some(error.clone());
                dialog.log.push(error.clone());
            }
            if let Some(started) = dialog.started.take() {
                let elapsed = started.elapsed();
                dialog.elapsed = Some(elapsed);
                dialog
                    .log
                    .push(t!("backup.log.elapsed", time = format_hms(elapsed)).to_string());
            }
            dialog.log.push(
                t!(
                    "sqlfile.run_done",
                    success = dialog.success,
                    failed = dialog.failed
                )
                .to_string(),
            );
            dialog.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// Ask the running script to stop after the current statement.
    pub(super) fn run_sql_abort(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.run_sql_dialog.as_mut() {
            dialog
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        cx.notify();
    }

    /// Close the Run SQL window.
    pub(super) fn close_run_sql(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.run_sql_dialog.as_mut() {
            dialog
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.run_sql_dialog = None;
        self.backup_window = None;
        window.remove_window();
        cx.notify();
    }

    /// The body of the open Run SQL dialog (both tabs).
    pub(super) fn run_sql_dialog_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.run_sql_dialog.as_ref() else {
            return div().into_any_element();
        };
        let tab = dialog.tab;
        let connection_name = self
            .connections
            .get(dialog.connection_index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_else(|| "--".to_string());
        let database_name = self
            .database_name(dialog.connection_index, dialog.database_index)
            .unwrap_or_else(|| "--".to_string());
        let schema = dialog.schema.clone();
        let supports_schemas = self
            .connections
            .get(dialog.connection_index)
            .is_some_and(|_| {
                self.driver_supports(dialog.connection_index, DriverCapability::Schemas)
            });
        let file_input = dialog.file_input.clone();
        let encoding_combo = dialog.encoding_combo.clone();
        let continue_on_error = dialog.continue_on_error;
        let total = dialog.total;
        let success = dialog.success;
        let failed = dialog.failed;
        let records = dialog.records;
        let running = dialog.running;
        let started = dialog.started;
        let elapsed = dialog.elapsed;
        let log = dialog.log.clone();
        let log_scroll = dialog.log_scroll.clone();

        // Tabs: 常规 / 信息日志.
        let tabs = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .w_full()
            .px_4()
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.backup_tab_button(
                "runsql-tab-general",
                t!("sqlfile.tab.general").to_string(),
                tab == BackupDialogTab::Objects,
                cx.listener(|this, _event, _window, cx| {
                    this.run_sql_set_tab(BackupDialogTab::Objects, cx)
                }),
            ))
            .child(self.backup_tab_button(
                "runsql-tab-log",
                t!("sqlfile.tab.log").to_string(),
                tab == BackupDialogTab::Log,
                cx.listener(|this, _event, _window, cx| {
                    this.run_sql_set_tab(BackupDialogTab::Log, cx)
                }),
            ));

        let content = match tab {
            BackupDialogTab::Objects => {
                let mut general = div().flex().flex_col().gap_2().px_4().pt_3().w_full();
                general = general
                    .child(readonly_field(
                        t!("sqlfile.server").to_string(),
                        connection_name,
                        theme,
                    ))
                    .child(readonly_field(
                        t!("sqlfile.database").to_string(),
                        database_name,
                        theme,
                    ));
                if supports_schemas {
                    general = general.child(readonly_field(
                        t!("sqlfile.schema").to_string(),
                        if schema.is_empty() {
                            "--".to_string()
                        } else {
                            schema
                        },
                        theme,
                    ));
                }
                general = general
                    .child(file_field(theme, file_input, cx))
                    .child(readonly_combo_field(
                        t!("sqlfile.encoding").to_string(),
                        encoding_combo,
                        theme,
                    ))
                    .child(
                        div()
                            .id("runsql-continue")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .h(px(24.0))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.run_sql_toggle_continue(cx)
                            }))
                            .child(checkbox_box(continue_on_error, theme))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(theme.text))
                                    .child(t!("sqlfile.continue_on_error").to_string()),
                            ),
                    );
                general.into_any_element()
            }
            BackupDialogTab::Log => {
                let processed = success + failed;
                let elapsed_text = if running {
                    started
                        .map(|started| format_hms(started.elapsed()))
                        .unwrap_or_else(|| "N/A".to_string())
                } else {
                    elapsed.map(format_hms).unwrap_or_else(|| "N/A".to_string())
                };
                let progress = if total == 0 {
                    0.0
                } else {
                    (processed as f32 / total as f32).clamp(0.0, 1.0)
                };
                let summary = div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_2()
                    .pt_2()
                    .pb_2()
                    .child(log_summary_row(
                        t!("sqlfile.queries").to_string(),
                        format_count(total),
                        theme,
                    ))
                    .child(log_summary_row(
                        t!("sqlfile.processed").to_string(),
                        format_count(processed),
                        theme,
                    ))
                    .child(log_summary_row(
                        t!("sqlfile.records").to_string(),
                        format_count(records),
                        theme,
                    ))
                    .child(log_summary_row(
                        t!("sqlfile.errors").to_string(),
                        format_count(failed),
                        theme,
                    ))
                    .child(log_summary_row(
                        t!("sqlfile.time").to_string(),
                        elapsed_text,
                        theme,
                    ));
                let log_box = render_log_lines(
                    &log,
                    &log_scroll,
                    &self.log_handle,
                    theme,
                    self.log_select_all,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|app, _event, _window, cx| {
                        if app.log_select_all || app.log_menu.is_some() {
                            app.log_select_all = false;
                            app.log_menu = None;
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|app, event: &MouseDownEvent, _window, cx| {
                        app.log_menu = Some(event.position);
                        app.log_select_all = false;
                        cx.notify();
                    }),
                );
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .mx_4()
                    .mt_3()
                    .mb_3()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.dialog_bg))
                    .overflow_hidden()
                    .child(summary)
                    .child(log_box)
                    .child(
                        div()
                            .h(px(6.0))
                            .mx_3()
                            .mt_3()
                            .mb_3()
                            .flex_none()
                            .rounded(px(9999.0))
                            .bg(rgb(theme.scroll_track))
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(progress))
                                    .rounded(px(9999.0))
                                    .bg(rgb(0x22b14c)),
                            ),
                    )
                    .into_any_element()
            }
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(tabs)
            .child(content)
            .into_any_element()
    }

    /// The footer of the open Run SQL dialog: 开始 / 停止 / 关闭.
    pub(super) fn run_sql_dialog_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(dialog) = self.run_sql_dialog.as_ref() else {
            return div().into_any_element();
        };
        let running = dialog.running;
        let mut footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap_2()
            .w_full()
            .px_4()
            .py_3()
            .flex_none();
        if running {
            footer = footer.child(self.dialog_button(
                "runsql-stop",
                t!("backup.stop").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.run_sql_abort(cx)),
            ));
        } else {
            footer = footer.child(self.dialog_button(
                "runsql-start",
                t!("sqlfile.start").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| this.run_sql_file(cx)),
            ));
        }
        footer = footer.child(self.dialog_button(
            "runsql-close",
            t!("backup.close").to_string(),
            false,
            cx.listener(|this, _event, window, cx| this.close_run_sql(window, cx)),
        ));
        footer.into_any_element()
    }
}

/// A `label   value` read-only row of the 常规 page.
fn readonly_field(label: String, value: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .child(
            div()
                .flex_none()
                .w(px(60.0))
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(12.5))
                .text_color(rgb(theme.text))
                .child(value),
        )
}

/// The 文件 row: a text field plus a `…` browse button.
fn file_field(
    theme: Theme,
    input: Option<Entity<TextInput>>,
    cx: &mut Context<'_, AppView>,
) -> impl IntoElement {
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .child(
            div()
                .flex_none()
                .w(px(60.0))
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child(t!("sqlfile.file").to_string()),
        );
    if let Some(input) = input {
        row = row.child(div().flex_1().min_w(px(0.0)).h(px(24.0)).child(input));
    } else {
        row = row.child(div().flex_1());
    }
    row.child(
        div()
            .id("runsql-browse")
            .flex()
            .items_center()
            .justify_center()
            .w(px(28.0))
            .h(px(24.0))
            .flex_none()
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(theme.border))
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
            .on_click(cx.listener(|this, _event, _window, cx| this.run_sql_browse_file(cx)))
            .child("…"),
    )
}

/// A `label   [dropdown]` row of the 常规 page.
fn readonly_combo_field(
    label: String,
    combo: Option<Entity<ComboBox>>,
    theme: Theme,
) -> impl IntoElement {
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .child(
            div()
                .flex_none()
                .w(px(60.0))
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child(label),
        );
    if let Some(combo) = combo {
        row = row.child(combo);
    }
    row
}

/// Keeps a running script's pinned session alive and releases it **on the Tokio runtime**.
///
/// A session owns a pooled connection (MySQL/MariaDB, SQLite, PostgreSQL), and sqlx hands a
/// connection back to its pool by spawning onto the Tokio runtime: outside one it panics with "this
/// functionality requires a Tokio context". The run's future executes on gpui's executor, so a
/// session dropped there aborted the whole app (silently — a release build has no console) the
/// moment the script finished. Routing every release through this guard covers the run ending, a
/// stop/abort, and the task itself being dropped.
pub(super) struct ScriptSessionGuard {
    pub(super) runtime: Arc<Runtime>,
    pub(super) session: Option<Box<dyn rustgrid_core::ScriptSession>>,
}

impl ScriptSessionGuard {
    /// Take the session out to run one statement on it.
    pub(super) fn take(&mut self) -> Option<Box<dyn rustgrid_core::ScriptSession>> {
        self.session.take()
    }

    /// Put the session back after a statement ran.
    pub(super) fn restore(&mut self, session: Box<dyn rustgrid_core::ScriptSession>) {
        self.session = Some(session);
    }
}

impl Drop for ScriptSessionGuard {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            // Spawn the drop onto the runtime, where the connection is released. Dropping the
            // `JoinHandle` only detaches the task, so it still runs.
            drop(self.runtime.spawn(async move { drop(session) }));
        }
    }
}

/// A bounded one-line preview of a statement for the failure log.
fn statement_snippet(statement: &str) -> String {
    let one_line: String = statement.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > 80 {
        let mut snippet: String = one_line.chars().take(80).collect();
        snippet.push('…');
        snippet
    } else {
        one_line
    }
}
