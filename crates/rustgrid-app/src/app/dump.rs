//! *Dump SQL File*: right-click a database, schema, table or view to open an object-selection
//! dialog — the same shape as the backup's *Extract SQL* window — and write a portable `.sql`
//! script for the chosen objects' `CREATE` DDL, their table data, or both.
//!
//! The dump reuses the engine-agnostic backup primitives ([`Connection::backup_object_metadata`]
//! and [`Connection::stream_table_rows`]) so it works for every driver without engine-specific
//! code here. The only dialect-aware part is the identifier quoting of the `INSERT` statements,
//! taken from the driver's [`rustgrid_core::DriverDialect`].

use std::io::Write;
use std::sync::Mutex;

use super::backup::{
    ObjectPickerAction, format_hms, log_stats, make_object_picker_search_input, now_timestamp,
    object_picker_toolbar, render_backup_log, render_object_picker, select_visible_objects,
    toggle_visible_object_kind,
};
use super::*;

/// Rows per multi-row `INSERT` statement in a dump.
const DUMP_BATCH_ROWS: usize = 500;

impl AppView {
    /// The database context menu's 转储 SQL 文件 entry: open the object-selection dialog for the
    /// whole database.
    pub(super) fn request_dump(
        &mut self,
        connection_index: usize,
        database_index: usize,
        mode: DumpMode,
        cx: &mut Context<'_, Self>,
    ) {
        self.open_dump_dialog(connection_index, database_index, None, None, mode, cx);
    }

    /// The schema context menu's 转储 SQL 文件 entry: open the dialog scoped to that schema.
    pub(super) fn request_dump_schema(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: String,
        mode: DumpMode,
        cx: &mut Context<'_, Self>,
    ) {
        self.open_dump_dialog(
            connection_index,
            database_index,
            Some(schema),
            None,
            mode,
            cx,
        );
    }

    /// The table/view context menu's 转储 SQL 文件 entry: open the dialog with that object.
    pub(super) fn request_dump_table(
        &mut self,
        connection_index: usize,
        database_index: usize,
        name: String,
        is_view: bool,
        mode: DumpMode,
        cx: &mut Context<'_, Self>,
    ) {
        let kind = if is_view {
            BackupObjectKind::View
        } else {
            BackupObjectKind::Table
        };
        self.open_dump_dialog(
            connection_index,
            database_index,
            None,
            Some((kind, name)),
            mode,
            cx,
        );
    }

    /// Open the dump window on its 对象选择 page, mirroring the backup's Extract SQL dialog: the
    /// objects are listed from the live server (or the one table/view the menu named), the user
    /// ticks what to write, and the output file is chosen when the dump starts.
    fn open_dump_dialog(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: Option<String>,
        object: Option<(BackupObjectKind, String)>,
        mode: DumpMode,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        let Some(connection) = self.connection_arc(connection_index) else {
            self.error_dialog = Some(t!("sqlfile.not_connected").to_string());
            cx.notify();
            return;
        };

        // The backup window hosts one operation at a time, so replace whatever it was showing.
        self.new_backup_dialog = None;
        self.restore_dialog = None;
        self.extract_dialog = None;
        self.run_sql_dialog = None;
        let weak = cx.weak_entity();
        self.dump_dialog = Some(DumpSqlDialog {
            connection_index,
            database_index,
            schema: schema.clone(),
            object: object.clone(),
            mode,
            objects: Vec::new(),
            object_search: String::new(),
            object_search_input: Some(make_object_picker_search_input(
                self.theme,
                &weak,
                ObjectPickerAction::Dump,
                cx,
            )),
            loading: true,
            tab: BackupDialogTab::Objects,
            output_path: None,
            running: false,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            log: Vec::new(),
            log_scroll: ScrollHandle::new(),
            total: 0,
            success: 0,
            failed: 0,
            rows_total: 0,
            rows_done: 0,
            started: None,
            elapsed: None,
            error: None,
        });
        // The title names the database (and schema) the dump is scoped to.
        let target = schema.clone().unwrap_or_else(|| database.clone());
        self.open_backup_window(format!("{} - {target}", t!("sqlfile.dump_title")), cx);

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let objects = match object {
                Some(target) => Ok(vec![target]),
                None => {
                    let schema = schema.clone();
                    match runtime
                        .spawn(async move {
                            let mut objects: Vec<(BackupObjectKind, String)> = Vec::new();
                            for table in connection.list_tables(&database).await? {
                                let kind = match table.kind {
                                    rustgrid_core::ObjectKind::View => BackupObjectKind::View,
                                    rustgrid_core::ObjectKind::Table => BackupObjectKind::Table,
                                };
                                objects.push((kind, table.name));
                            }
                            if let Ok(routines) = connection.list_routines(&database).await {
                                for name in routines {
                                    objects.push((BackupObjectKind::Function, name));
                                }
                            }
                            if let Ok(events) = connection.list_events(&database).await {
                                for name in events {
                                    objects.push((BackupObjectKind::Event, name));
                                }
                            }
                            // A schema menu's dump keeps only that schema's objects; their names
                            // are schema-qualified (PostgreSQL, SQL Server, Oracle).
                            if let Some(schema) = schema {
                                let prefix = format!("{schema}.");
                                objects.retain(|(_, name)| name.starts_with(&prefix));
                            }
                            // Tables before views before routines, so restoring the script in
                            // order satisfies the dependencies.
                            objects.sort_by_key(|(kind, _)| match kind {
                                BackupObjectKind::Table => 0,
                                BackupObjectKind::View => 1,
                                BackupObjectKind::Function => 2,
                                BackupObjectKind::Event => 3,
                            });
                            Ok::<_, Error>(objects)
                        })
                        .await
                    {
                        Ok(result) => result,
                        Err(error) => Err(Error::other(error)),
                    }
                }
            };
            let _ = this.update(cx, |app, cx| {
                if let Some(dialog) = app.dump_dialog.as_mut() {
                    dialog.loading = false;
                    match objects {
                        Ok(objects) => {
                            dialog.objects = objects
                                .into_iter()
                                .map(|(kind, name)| BackupObjectEntry {
                                    kind,
                                    name,
                                    selected: true,
                                })
                                .collect();
                        }
                        Err(error) => dialog.error = Some(error.to_string()),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Pick the dump's destination `.sql` file through the system save dialog, then start the run.
    fn choose_dump_output(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.dump_dialog.as_ref() else {
            return;
        };
        let Some(database) = self.database_name(dialog.connection_index, dialog.database_index)
        else {
            return;
        };
        let default_name = match (&dialog.object, &dialog.schema) {
            (Some((_, name)), _) => format!("{name}.sql"),
            (None, Some(schema)) => format!("{schema}.sql"),
            (None, None) => format!("{database}.sql"),
        };
        let title = t!("sqlfile.dump_title").to_string();
        let default_dir = super::export::default_export_dir();

        cx.spawn(async move |this, cx| {
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_title(title)
                .add_filter("SQL", &["sql"])
                .set_file_name(default_name);
            if !default_dir.is_empty() {
                dialog = dialog.set_directory(&default_dir);
            }
            let Some(handle) = dialog.save_file().await else {
                return;
            };
            let path = handle.path().to_path_buf();
            let _ = this.update(cx, |app, cx| {
                if let Some(dialog) = app.dump_dialog.as_mut() {
                    dialog.output_path = Some(path);
                }
                app.run_dump(cx);
            });
        })
        .detach();
    }

    /// Run the dump: choose the output file when it has not been picked yet, then write every
    /// selected object's DDL and/or data while updating the log and progress live.
    pub(super) fn run_dump(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.dump_dialog.as_ref() else {
            return;
        };
        if dialog.running || dialog.loading {
            return;
        }
        let selected: Vec<(BackupObjectKind, String)> = dialog
            .objects
            .iter()
            .filter(|entry| entry.selected)
            .map(|entry| (entry.kind, entry.name.clone()))
            .collect();
        if selected.is_empty() {
            if let Some(dialog) = self.dump_dialog.as_mut() {
                dialog.error = Some(t!("backup.no_objects").to_string());
            }
            cx.notify();
            return;
        }
        if dialog.output_path.is_none() {
            self.choose_dump_output(cx);
            return;
        }
        let connection_index = dialog.connection_index;
        let database_index = dialog.database_index;
        let schema = dialog.schema.clone();
        let object = dialog.object.clone();
        let mode = dialog.mode;
        let Some(path) = dialog.output_path.clone() else {
            return;
        };
        let Some(connection) = self.connection_arc(connection_index) else {
            if let Some(dialog) = self.dump_dialog.as_mut() {
                dialog.error = Some(t!("sqlfile.not_connected").to_string());
            }
            cx.notify();
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        let connection_name = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let dialect = self.driver_dialect(connection_index);
        // Schema-qualified engines (SQL Server, PostgreSQL, Oracle) name objects `schema.table`,
        // so each dotted segment must be quoted on its own.
        let schema_qualified = self.driver_supports(connection_index, DriverCapability::Schemas);

        if let Some(dialog) = self.dump_dialog.as_mut() {
            dialog.running = true;
            dialog.tab = BackupDialogTab::Log;
            dialog
                .cancel
                .store(false, std::sync::atomic::Ordering::Relaxed);
            dialog.error = None;
            dialog.log.clear();
            dialog.total = 0;
            dialog.success = 0;
            dialog.failed = 0;
            dialog.rows_total = 0;
            dialog.rows_done = 0;
            dialog.elapsed = None;
            dialog.started = Some(std::time::Instant::now());
            dialog
                .log
                .push(t!("backup.log.started", time = now_timestamp()).to_string());
            dialog.log_scroll.scroll_to_bottom();
        }

        let mut file = match std::fs::File::create(&path) {
            Ok(file) => file,
            Err(error) => {
                if let Some(dialog) = self.dump_dialog.as_mut() {
                    dialog.error = Some(error.to_string());
                }
                cx.notify();
                return;
            }
        };
        let mut header = format!(
            "-- RustGrid SQL dump\n-- Server: {connection_name}\n-- Database: {database}\n"
        );
        if let Some(schema) = &schema {
            header.push_str(&format!("-- Schema: {schema}\n"));
        }
        if let Some((_, name)) = &object {
            header.push_str(&format!("-- Object: {name}\n"));
        }
        header.push_str(&format!("-- Date: {}\n\n", now_timestamp()));
        // The script can load table data in any order and replace existing objects, so
        // foreign-key enforcement is switched off for its duration and back on at the end.
        if let Some(sql) = dialect.session_foreign_keys_sql(false) {
            header.push_str(sql);
            header.push_str("\n\n");
        }
        if let Err(error) = file.write_all(header.as_bytes()) {
            if let Some(dialog) = self.dump_dialog.as_mut() {
                dialog.error = Some(error.to_string());
            }
            cx.notify();
            return;
        }
        let file = Arc::new(Mutex::new(file));

        let runtime = self.runtime.clone();
        let cancel = self
            .dump_dialog
            .as_ref()
            .map(|dialog| dialog.cancel.clone())
            .unwrap_or_default();

        cx.spawn(async move |this, cx| {
            // A data-only dump has nothing to write for views/routines/events.
            let objects: Vec<(BackupObjectKind, String)> = if mode.includes_structure() {
                selected
            } else {
                selected
                    .into_iter()
                    .filter(|(kind, _)| *kind == BackupObjectKind::Table)
                    .collect()
            };

            let count = objects.len();
            let _ = this.update(cx, |app, cx| {
                if let Some(dialog) = app.dump_dialog.as_mut() {
                    dialog.total = count;
                    dialog
                        .log
                        .push(t!("sqlfile.dump_start", count = count).to_string());
                    dialog.log_scroll.scroll_to_bottom();
                }
                cx.notify();
            });

            let mut fatal: Option<String> = None;

            // Fetch every object's metadata before writing: a table's foreign keys reference other
            // tables, and a fresh import must create and load referenced tables first.
            let mut loaded: Vec<(BackupObjectKind, String, Option<rustgrid_core::ObjectDump>)> =
                Vec::with_capacity(count);
            for (kind, name) in objects {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let fetched = {
                    let connection = connection.clone();
                    let database = database.clone();
                    let call_name = name.clone();
                    runtime
                        .spawn(async move {
                            connection
                                .backup_object_metadata(&database, kind, &call_name)
                                .await
                        })
                        .await
                };
                match fetched {
                    Ok(Ok(meta)) => loaded.push((kind, name, Some(meta))),
                    Ok(Err(error)) => {
                        loaded.push((kind, name.clone(), None));
                        let message = t!(
                            "sqlfile.dump_object_failed",
                            name = name,
                            error = error.to_string()
                        )
                        .to_string();
                        let _ = this
                            .update(cx, |app, cx| app.record_dump_result(false, message, 0, cx));
                    }
                    Err(error) => {
                        loaded.push((kind, name.clone(), None));
                        let message = t!(
                            "sqlfile.dump_object_failed",
                            name = name,
                            error = error.to_string()
                        )
                        .to_string();
                        let _ = this
                            .update(cx, |app, cx| app.record_dump_result(false, message, 0, cx));
                    }
                }
            }

            // Parent tables first (foreign keys resolve), then every other object in its original
            // order: views after the tables they select from, routines after both.
            let table_positions: Vec<usize> = loaded
                .iter()
                .enumerate()
                .filter(|(_, (kind, _, meta))| *kind == BackupObjectKind::Table && meta.is_some())
                .map(|(index, _)| index)
                .collect();
            let tables: Vec<(String, Vec<String>)> = table_positions
                .iter()
                .map(|&index| {
                    (
                        loaded[index].1.clone(),
                        loaded[index]
                            .2
                            .as_ref()
                            .map(|meta| meta.references.clone())
                            .unwrap_or_default(),
                    )
                })
                .collect();
            let mut order: Vec<usize> = rustgrid_core::reference_order(&tables)
                .into_iter()
                .map(|position| table_positions[position])
                .collect();
            let mut placed = vec![false; loaded.len()];
            for &index in &order {
                placed[index] = true;
            }
            for index in 0..loaded.len() {
                if !placed[index] {
                    order.push(index);
                }
            }

            let mut items: Vec<Option<(BackupObjectKind, String, rustgrid_core::ObjectDump)>> =
                loaded
                    .into_iter()
                    .map(|(kind, name, meta)| meta.map(|meta| (kind, name, meta)))
                    .collect();
            // Tables whose foreign-key enforcement this script switched off, re-enabled at the end.
            let mut fk_tables: Vec<String> = Vec::new();
            for index in order {
                let Some((kind, name, meta)) = items[index].take() else {
                    continue;
                };
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }

                // Structure: the object's DDL plus any trigger statements.
                if mode.includes_structure()
                    && let Err(error) = write_structure(&file, &meta)
                {
                    fatal = Some(error.to_string());
                    break;
                }

                // Per-table engines (PostgreSQL, SQL Server) switch foreign-key checks off right
                // after the table exists, so its rows can load before the referenced tables'.
                if mode.includes_data() && kind == BackupObjectKind::Table {
                    let quoted = quote_object_name(dialect, schema_qualified, &name);
                    if let Some(sql) = dialect.table_foreign_keys_sql(&quoted, false) {
                        if let Err(error) = write_sql(&file, &sql) {
                            fatal = Some(error.to_string());
                            break;
                        }
                        fk_tables.push(quoted);
                    }
                }

                // Data: stream the table's rows into batched INSERT statements.
                let mut rows = 0u64;
                let mut row_error: Option<String> = None;
                if mode.includes_data() && kind == BackupObjectKind::Table {
                    let connection = connection.clone();
                    let database = database.clone();
                    let call_name = name.clone();
                    let columns = insert_columns(dialect, &meta.fields);
                    let header = match columns {
                        Some(columns) => format!(
                            "INSERT INTO {} ({columns}) VALUES\n",
                            quote_object_name(dialect, schema_qualified, &name)
                        ),
                        None => String::new(),
                    };
                    let sink = Arc::clone(&file);
                    let result = runtime
                        .spawn(async move {
                            if header.is_empty() {
                                return Ok(0u64);
                            }
                            let mut writer = InsertWriter::new(header);
                            let mut on_row = |tuple: &str| writer.write_row(&sink, tuple);
                            let count = connection
                                .stream_table_rows(&database, &call_name, &mut on_row)
                                .await;
                            let finished = writer.finish(&sink);
                            match count {
                                Ok(count) => {
                                    finished?;
                                    Ok(count)
                                }
                                Err(error) => Err(error),
                            }
                        })
                        .await;
                    match result {
                        Ok(Ok(count)) => rows = count,
                        Ok(Err(error)) => row_error = Some(error.to_string()),
                        Err(error) => row_error = Some(error.to_string()),
                    }
                }

                let _ = this.update(cx, |app, cx| match &row_error {
                    Some(error) => {
                        let message = t!(
                            "sqlfile.dump_object_failed",
                            name = name.clone(),
                            error = error.clone()
                        )
                        .to_string();
                        app.record_dump_result(false, message, 0, cx);
                    }
                    None => {
                        let message =
                            t!("sqlfile.dump_object", name = name.clone(), rows = rows).to_string();
                        app.record_dump_result(true, message, rows as usize, cx);
                    }
                });
            }

            // Restore foreign-key enforcement now that every row is in.
            let mut footer = String::new();
            for table in &fk_tables {
                if let Some(sql) = dialect.table_foreign_keys_sql(table, true) {
                    footer.push_str(&sql);
                    footer.push('\n');
                }
            }
            if let Some(sql) = dialect.session_foreign_keys_sql(true) {
                footer.push('\n');
                footer.push_str(sql);
                footer.push('\n');
            }
            let _ = file
                .lock()
                .map(|mut file| file.write_all(footer.as_bytes()));
            let _ = file.lock().map(|file| file.sync_all());
            let cancelled = cancel.load(std::sync::atomic::Ordering::Relaxed);
            let _ = this.update(cx, |app, cx| {
                if cancelled {
                    app.backup_log(t!("backup.log.cancelled").to_string(), cx);
                }
                app.finish_dump(fatal, cx)
            });
        })
        .detach();
    }

    /// Record one object's outcome for the running dump and keep its log scrolled.
    fn record_dump_result(
        &mut self,
        ok: bool,
        message: String,
        rows: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(dialog) = self.dump_dialog.as_mut() {
            if ok {
                dialog.success += 1;
                dialog.rows_done += rows;
            } else {
                dialog.failed += 1;
            }
            dialog.log.push(message);
            dialog.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn finish_dump(&mut self, fatal: Option<String>, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.dump_dialog.as_mut() {
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
                    "backup.log.summary",
                    total = dialog.total,
                    success = dialog.success,
                    failed = dialog.failed
                )
                .to_string(),
            );
            dialog.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// Ask the running dump to stop after the current object.
    pub(super) fn dump_abort(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.dump_dialog.as_mut() {
            dialog
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        cx.notify();
    }

    /// Close the dump window.
    pub(super) fn close_dump(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.dump_dialog.as_mut() {
            dialog
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.dump_dialog = None;
        self.backup_window = None;
        window.remove_window();
        cx.notify();
    }

    /// The window body of the open dump dialog: the destination line, the 对象选择 / 信息日志 tabs
    /// and the selected page — the same shape as the backup's Extract SQL dialog.
    pub(super) fn dump_dialog_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.dump_dialog.as_ref() else {
            return div().into_any_element();
        };
        let active = dialog.tab;
        let connection_name = self
            .connections
            .get(dialog.connection_index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_else(|| "--".to_string());
        let database_name = self
            .database_name(dialog.connection_index, dialog.database_index)
            .unwrap_or_else(|| "--".to_string());
        let mode = match dialog.mode {
            DumpMode::StructureAndData => t!("database.dump_structure_and_data").to_string(),
            DumpMode::Structure => t!("database.dump_structure").to_string(),
            DumpMode::Data => t!("database.dump_data").to_string(),
        };
        let schema = dialog.schema.clone();
        let output = dialog
            .output_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| t!("backup.extract_no_output").to_string());

        // The target line names the exact database (and schema) being dumped, so the popup is
        // unambiguous about which library its object list belongs to.
        let mut target = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .w_full()
            .px_4()
            .pb_2();
        target = target
            .child(dump_scope_field(
                t!("sqlfile.server").to_string(),
                connection_name,
                theme,
            ))
            .child(dump_scope_field(
                t!("sqlfile.database").to_string(),
                database_name,
                theme,
            ));
        if let Some(schema) = &schema {
            target = target.child(dump_scope_field(
                t!("sqlfile.schema").to_string(),
                schema.clone(),
                theme,
            ));
        }
        target = target.child(dump_scope_field(
            t!("sqlfile.mode").to_string(),
            mode,
            theme,
        ));

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .px_4()
            .pt_3()
            .pb_2()
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("backup.dump_to").to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(12.5))
                    .text_color(rgb(theme.text))
                    .child(output),
            );
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
                "dump-tab-objects",
                t!("backup.tab.objects").to_string(),
                active == BackupDialogTab::Objects,
                cx.listener(|this, _event, _window, cx| {
                    this.set_dump_tab(BackupDialogTab::Objects, cx)
                }),
            ))
            .child(self.backup_tab_button(
                "dump-tab-log",
                t!("backup.tab.log").to_string(),
                active == BackupDialogTab::Log,
                cx.listener(|this, _event, _window, cx| {
                    this.set_dump_tab(BackupDialogTab::Log, cx)
                }),
            ));

        let content = match active {
            BackupDialogTab::Objects => self.render_dump_picker_body(cx),
            BackupDialogTab::Log => {
                let scroll = self
                    .dump_dialog
                    .as_ref()
                    .map(|dialog| dialog.log_scroll.clone())
                    .unwrap_or_default();
                let stats = self
                    .dump_dialog
                    .as_ref()
                    .map(|dialog| {
                        log_stats(
                            dialog.total,
                            dialog.success,
                            dialog.failed,
                            dialog.rows_total,
                            dialog.rows_done,
                            dialog.running,
                            dialog.started,
                            dialog.elapsed,
                        )
                    })
                    .unwrap_or_else(|| log_stats(0, 0, 0, 0, 0, false, None, None));
                render_backup_log(
                    self.dump_dialog
                        .as_ref()
                        .map(|dialog| dialog.log.as_slice())
                        .unwrap_or(&[]),
                    &scroll,
                    &self.log_handle,
                    &stats,
                    theme,
                    self.log_select_all,
                    cx,
                )
            }
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(header)
            .child(target)
            .child(tabs)
            .child(content)
            .into_any_element()
    }

    /// The 对象选择 page of the dump dialog (the shared backup object picker).
    fn render_dump_picker_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.dump_dialog.as_ref() else {
            return div().into_any_element();
        };
        let search = dialog.object_search.clone();
        let summary = object_picker_toolbar(
            &dialog.objects,
            &search,
            dialog.object_search_input.clone(),
            theme,
            "dump-select-all",
            "dump-select-none",
            cx.listener(|this, _event, _window, cx| this.dump_set_all(true, cx)),
            cx.listener(|this, _event, _window, cx| this.dump_set_all(false, cx)),
        );

        let picker = render_object_picker(
            &dialog.objects,
            dialog.loading,
            &self.backup_objects_scroll,
            theme,
            ObjectPickerAction::Dump,
            &search,
            cx,
        );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(summary)
            .child(picker)
            .into_any_element()
    }

    /// The footer of the open dump dialog: 转储 / 停止 / 关闭.
    pub(super) fn dump_dialog_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(dialog) = self.dump_dialog.as_ref() else {
            return div().into_any_element();
        };
        let running = dialog.running;
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .w_full()
            .h(px(46.0))
            .px_4()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.dialog_button(
                        "dump-run",
                        t!("backup.dump_button").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| {
                            if !this
                                .dump_dialog
                                .as_ref()
                                .map(|dialog| dialog.running)
                                .unwrap_or(false)
                            {
                                this.run_dump(cx);
                            }
                        }),
                    ))
                    .when(running, |row| {
                        row.child(self.dialog_button(
                            "dump-stop",
                            t!("backup.stop").to_string(),
                            false,
                            cx.listener(|this, _event, _window, cx| this.dump_abort(cx)),
                        ))
                    })
                    .child(self.dialog_button(
                        "dump-close",
                        t!("backup.close").to_string(),
                        false,
                        cx.listener(|this, _event, window, cx| this.close_dump(window, cx)),
                    )),
            )
            .into_any_element()
    }

    /// Switch the dump dialog's tab (对象选择 / 信息日志).
    pub(super) fn set_dump_tab(&mut self, tab: BackupDialogTab, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.dump_dialog.as_mut() {
            dialog.tab = tab;
        }
        cx.notify();
    }

    /// Toggle one object in the dump dialog's selection.
    pub(super) fn dump_toggle_object(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.dump_dialog.as_mut()
            && let Some(entry) = dialog.objects.get_mut(index)
        {
            entry.selected = !entry.selected;
        }
        cx.notify();
    }

    /// Toggle every object of one kind in the dump dialog's selection.
    pub(super) fn dump_toggle_kind(&mut self, kind: BackupObjectKind, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.dump_dialog.as_mut() {
            toggle_visible_object_kind(&mut dialog.objects, kind, &dialog.object_search);
        }
        cx.notify();
    }

    pub(super) fn dump_set_all(&mut self, selected: bool, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.dump_dialog.as_mut() {
            select_visible_objects(&mut dialog.objects, selected, &dialog.object_search);
        }
        cx.notify();
    }
}

/// One `label value` pair of the dump window's target line.
fn dump_scope_field(label: String, value: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .min_w(px(0.0))
        .child(
            div()
                .flex_none()
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child(label),
        )
        .child(
            div()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(value),
        )
}

/// Write one object's `CREATE` DDL (and any trigger statements) to the dump file.
fn write_structure(
    file: &Mutex<std::fs::File>,
    meta: &rustgrid_core::ObjectDump,
) -> rustgrid_core::Result<()> {
    let mut output = String::new();
    // Navicat-style: drop a same-named object first, so the dump replaces it on a re-run.
    let drop = meta.drop_ddl.trim().trim_end_matches(';').trim_end();
    if !drop.is_empty() {
        output.push_str(drop);
        output.push_str(";\n");
    }
    let ddl = meta.ddl.trim().trim_end_matches(';').trim_end();
    if !ddl.is_empty() {
        output.push_str(ddl);
        output.push_str(";\n");
    }
    for trigger in &meta.trigger_ddl {
        let trigger = trigger.trim().trim_end_matches(';').trim_end();
        if !trigger.is_empty() {
            output.push_str(trigger);
            output.push_str(";\n");
        }
    }
    output.push('\n');
    let mut file = file.lock().unwrap();
    file.write_all(output.as_bytes()).map_err(Error::other)
}

/// Write one already-terminated SQL statement line to the dump file.
fn write_sql(file: &Mutex<std::fs::File>, sql: &str) -> rustgrid_core::Result<()> {
    let mut file = file.lock().unwrap();
    file.write_all(sql.as_bytes()).map_err(Error::other)?;
    file.write_all(b"\n").map_err(Error::other)
}

/// The quoted `INSERT` column list, or `None` when the object exposes no columns.
fn insert_columns(dialect: rustgrid_core::DriverDialect, fields: &[String]) -> Option<String> {
    if fields.is_empty() {
        return None;
    }
    Some(
        fields
            .iter()
            .map(|field| dialect.quote_identifier(field))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// Quote an object name for the `INSERT` statement. Schema-qualified engines report `schema.name`,
/// so each dotted segment is quoted separately (`"public"."users"`).
fn quote_object_name(
    dialect: rustgrid_core::DriverDialect,
    schema_qualified: bool,
    name: &str,
) -> String {
    // A schema-qualified engine reports `schema.name` even when the driver does not expose a
    // separate schema level (the generic ODBC driver), so split the dotted name whenever the
    // dialect itself has schema-qualified objects.
    let split = schema_qualified
        || (name.contains('.')
            && matches!(
                dialect,
                rustgrid_core::DriverDialect::SqlServer
                    | rustgrid_core::DriverDialect::Postgres
                    | rustgrid_core::DriverDialect::Oracle
            ));
    if split {
        name.split('.')
            .map(|part| dialect.quote_identifier(part))
            .collect::<Vec<_>>()
            .join(".")
    } else {
        dialect.quote_identifier(name)
    }
}

/// Streams a table's value tuples into the file as batched multi-row `INSERT` statements.
struct InsertWriter {
    /// The `INSERT INTO <table> (<columns>) VALUES` prefix, repeated before **every** batch (a
    /// bare value list is not valid SQL).
    header: String,
    buffer: Vec<String>,
}

impl InsertWriter {
    fn new(header: String) -> Self {
        Self {
            header,
            buffer: Vec::new(),
        }
    }

    fn write_row(&mut self, file: &Mutex<std::fs::File>, tuple: &str) -> rustgrid_core::Result<()> {
        self.buffer.push(tuple.to_string());
        if self.buffer.len() >= DUMP_BATCH_ROWS {
            self.flush(file)?;
        }
        Ok(())
    }

    /// Flush the tail of the current batch.
    fn finish(&mut self, file: &Mutex<std::fs::File>) -> rustgrid_core::Result<()> {
        self.flush(file)
    }

    fn flush(&mut self, file: &Mutex<std::fs::File>) -> rustgrid_core::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let rows = self.buffer.join(",\n");
        self.buffer.clear();
        // Every batch carries its own `INSERT` header.
        let text = format!("{}{rows}\n;\n", self.header);
        file.lock()
            .unwrap()
            .write_all(text.as_bytes())
            .map_err(Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::{DUMP_BATCH_ROWS, InsertWriter, quote_object_name, write_structure};
    use std::sync::Mutex;

    /// The generic ODBC driver reports `schema.name` even though it has no separate schema level,
    /// so the dump writer must still quote each dotted segment (it used to quote the whole
    /// `[dbo.users]`, which SQL Server cannot resolve).
    #[test]
    fn schema_qualified_names_are_quoted_per_segment() {
        assert_eq!(
            quote_object_name(rustgrid_core::DriverDialect::SqlServer, false, "dbo.users"),
            "[dbo].[users]"
        );
        assert_eq!(
            quote_object_name(rustgrid_core::DriverDialect::Mysql, false, "users"),
            "`users`"
        );
    }

    /// A dump writes each object's DROP ahead of its CREATE, so re-running the script replaces the
    /// existing objects instead of failing (Navicat's export does the same).
    #[test]
    fn structure_drops_before_create() {
        let dir = std::env::temp_dir().join(format!("rustgrid-dump-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("structure.sql");
        let file = Mutex::new(std::fs::File::create(&path).unwrap());
        let meta = rustgrid_core::ObjectDump {
            name: "users".to_string(),
            kind: rustgrid_core::BackupObjectKind::Table,
            ddl: "CREATE TABLE `users` (`id` int)".to_string(),
            drop_ddl: "DROP TABLE IF EXISTS `users`".to_string(),
            references: Vec::new(),
            fields: Vec::new(),
            trigger_ddl: Vec::new(),
            rows: Vec::new(),
        };
        write_structure(&file, &meta).unwrap();
        drop(file);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("DROP TABLE IF EXISTS `users`;\nCREATE TABLE `users`"),
            "drop must precede the create:\n{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every batch of a multi-row `INSERT` must repeat the `INSERT INTO …` header; only the first
    /// one used to get it, so tables larger than one batch dumped invalid SQL for the rest.
    #[test]
    fn insert_batches_repeat_the_header() {
        let dir = std::env::temp_dir().join(format!("rustgrid-dump-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dump.sql");
        let file = Mutex::new(std::fs::File::create(&path).unwrap());
        let header = "INSERT INTO `t` (`a`) VALUES\n".to_string();
        let mut writer = InsertWriter::new(header.clone());
        for index in 0..(DUMP_BATCH_ROWS + 3) {
            writer.write_row(&file, &format!("({index})")).unwrap();
        }
        writer.finish(&file).unwrap();
        drop(file);

        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.matches("INSERT INTO `t` (`a`) VALUES").count(),
            2,
            "each batch needs its own header:\n{text}"
        );
        for statement in text.split(';').filter(|part| !part.trim().is_empty()) {
            assert!(
                statement.trim_start().starts_with("INSERT INTO"),
                "a batch is missing its INSERT header:\n{statement}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
