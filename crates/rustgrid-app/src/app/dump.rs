//! *Dump SQL File*: right-click a database and write a portable `.sql` script for the whole
//! database — its objects' `CREATE` DDL, its table data, or both.
//!
//! The dump reuses the engine-agnostic backup primitives ([`Connection::backup_object_metadata`]
//! and [`Connection::stream_table_rows`]) so it works for every driver without engine-specific
//! code here. The only dialect-aware part is the identifier quoting of the `INSERT` statements,
//! taken from the driver's [`rustgrid_core::DriverDialect`].

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use super::backup::{backup_target_line, format_hms, log_stats, now_timestamp, render_backup_log};
use super::*;

/// Rows per multi-row `INSERT` statement in a dump.
const DUMP_BATCH_ROWS: usize = 500;

impl AppView {
    /// The database context menu's 转储 SQL 文件 entry: pick the destination file through the
    /// system save dialog, then open the dump window and start writing.
    pub(super) fn request_dump(
        &mut self,
        connection_index: usize,
        database_index: usize,
        mode: DumpMode,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        if self.connection_arc(connection_index).is_none() {
            self.error_dialog = Some(t!("sqlfile.not_connected").to_string());
            cx.notify();
            return;
        }
        let default_name = format!("{database}.sql");
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
                app.open_dump_window(connection_index, database_index, mode, path, cx);
            });
        })
        .detach();
    }

    /// Open the dump window with the chosen output file and start the dump.
    fn open_dump_window(
        &mut self,
        connection_index: usize,
        database_index: usize,
        mode: DumpMode,
        output_path: PathBuf,
        cx: &mut Context<'_, Self>,
    ) {
        // The backup window hosts one operation at a time, so replace whatever it was showing.
        self.new_backup_dialog = None;
        self.restore_dialog = None;
        self.extract_dialog = None;
        self.run_sql_dialog = None;
        self.dump_dialog = Some(DumpSqlDialog {
            connection_index,
            database_index,
            mode,
            output_path: Some(output_path),
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
        self.open_backup_window(t!("sqlfile.dump_title").to_string(), cx);
        self.run_dump(cx);
        cx.notify();
    }

    /// Run the dump: enumerate the database's objects, then write each one's DDL and/or data to
    /// the chosen file while updating the log and progress live.
    pub(super) fn run_dump(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.dump_dialog.as_ref() else {
            return;
        };
        if dialog.running {
            return;
        }
        let connection_index = dialog.connection_index;
        let database_index = dialog.database_index;
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
            "-- RustGrid SQL dump\n-- Server: {connection_name}\n-- Database: {database}\n-- Date: {}\n\n",
            now_timestamp()
        );
        // MySQL/MariaDB dumps disable foreign-key checks while the data loads (like mysqldump and
        // Navicat), so tables can be inserted in any order without 1452 errors.
        let foreign_keys = dialect == rustgrid_core::DriverDialect::Mysql;
        if foreign_keys {
            header.push_str("SET FOREIGN_KEY_CHECKS=0;\n\n");
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
            // Enumerate the objects: tables and views plus stored routines and events.
            let listing = {
                let connection = connection.clone();
                let database = database.clone();
                runtime
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
                        // Tables before views before routines: `SHOW CREATE VIEW` references its
                        // base tables, and routines may reference both, so they must already exist.
                        objects.sort_by_key(|(kind, _)| match kind {
                            BackupObjectKind::Table => 0,
                            BackupObjectKind::View => 1,
                            BackupObjectKind::Function => 2,
                            BackupObjectKind::Event => 3,
                        });
                        Ok::<_, Error>(objects)
                    })
                    .await
            };
            let objects = match listing {
                Ok(Ok(objects)) => objects,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |app, cx| app.finish_dump(Some(error.to_string()), cx));
                    return;
                }
                Err(error) => {
                    let _ = this.update(cx, |app, cx| app.finish_dump(Some(error.to_string()), cx));
                    return;
                }
            };

            // A data-only dump has nothing to write for views/routines/events.
            let objects: Vec<(BackupObjectKind, String)> = if mode.includes_structure() {
                objects
            } else {
                objects
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
            for (kind, name) in objects {
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let meta = {
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
                let meta = match meta {
                    Ok(Ok(meta)) => meta,
                    Ok(Err(error)) => {
                        let message = t!(
                            "sqlfile.dump_object_failed",
                            name = name.clone(),
                            error = error.to_string()
                        )
                        .to_string();
                        let _ = this
                            .update(cx, |app, cx| app.record_dump_result(false, message, 0, cx));
                        continue;
                    }
                    Err(error) => {
                        let message = t!(
                            "sqlfile.dump_object_failed",
                            name = name.clone(),
                            error = error.to_string()
                        )
                        .to_string();
                        let _ = this
                            .update(cx, |app, cx| app.record_dump_result(false, message, 0, cx));
                        continue;
                    }
                };

                // Structure: the object's DDL plus any trigger statements.
                if mode.includes_structure()
                    && let Err(error) = write_structure(&file, &meta)
                {
                    fatal = Some(error.to_string());
                    break;
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

            if foreign_keys {
                let _ = file
                    .lock()
                    .map(|mut file| file.write_all(b"\nSET FOREIGN_KEY_CHECKS=1;\n"));
            }
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

    /// Open the dumped `.sql` file with the OS default application (the 打开 button).
    pub(super) fn open_dump_output(&mut self, cx: &mut Context<'_, Self>) {
        let Some(path) = self
            .dump_dialog
            .as_ref()
            .and_then(|dialog| dialog.output_path.clone())
        else {
            return;
        };
        super::backup::open_with_default_app(&path);
        cx.notify();
    }

    /// The window body of the open dump dialog.
    pub(super) fn dump_dialog_body(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.dump_dialog.as_ref() else {
            return div().into_any_element();
        };
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
        let output = dialog
            .output_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stats = log_stats(
            dialog.total,
            dialog.success,
            dialog.failed,
            dialog.rows_total,
            dialog.rows_done,
            dialog.running,
            dialog.started,
            dialog.elapsed,
        );
        let log = dialog.log.clone();
        let scroll = dialog.log_scroll.clone();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(backup_target_line(
                t!("backup.target").to_string(),
                connection_name,
                database_name,
                theme,
            ))
            .child(field_line(t!("sqlfile.mode").to_string(), mode, theme))
            .child(field_line(
                t!("sqlfile.file").to_string(),
                if output.is_empty() {
                    t!("backup.extract_no_output").to_string()
                } else {
                    output
                },
                theme,
            ))
            .child(render_backup_log(
                &log,
                &scroll,
                &self.log_handle,
                &stats,
                theme,
                self.log_select_all,
                cx,
            ))
            .into_any_element()
    }

    /// The footer of the open dump dialog: 打开 / 停止 / 关闭.
    pub(super) fn dump_dialog_footer(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(dialog) = self.dump_dialog.as_ref() else {
            return div().into_any_element();
        };
        let running = dialog.running;
        let has_output = dialog.output_path.is_some();
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
        if has_output && !running {
            footer = footer.child(self.dialog_button(
                "dump-open",
                t!("backup.open").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.open_dump_output(cx)),
            ));
        }
        if running {
            footer = footer.child(self.dialog_button(
                "dump-stop",
                t!("backup.stop").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.dump_abort(cx)),
            ));
        }
        footer = footer.child(self.dialog_button(
            "dump-close",
            t!("backup.close").to_string(),
            false,
            cx.listener(|this, _event, window, cx| this.close_dump(window, cx)),
        ));
        footer.into_any_element()
    }
}

/// A `label   value` line for the dump window's header block.
fn field_line(label: String, value: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .w_full()
        .px_4()
        .pb_1()
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

/// Write one object's `CREATE` DDL (and any trigger statements) to the dump file.
fn write_structure(
    file: &Mutex<std::fs::File>,
    meta: &rustgrid_core::ObjectDump,
) -> rustgrid_core::Result<()> {
    let mut output = String::new();
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
    use super::{DUMP_BATCH_ROWS, InsertWriter, quote_object_name};
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
