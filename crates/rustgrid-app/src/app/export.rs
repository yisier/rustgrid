//! The Export Wizard: Navicat's table-data export flow as a separate OS window.
//!
//! The wizard is four pages: choose a format (P1), pick tables and output paths (P2), choose the
//! exported columns (P3), then set options and run (P4 — Navicat's P4 options merged into its P5
//! run/log page). `AppView` owns the state ([`ExportWizard`]); this module renders it, drives the
//! page transitions and runs the export through [`rustgrid_export`] and the
//! [`rustgrid_core::Connection`] trait.
//!
//! Output paths default to the desktop and are chosen with the OS's native file dialogs (via
//! `rfd`), which is why the wizard lives in a real window rather than a `Root` dialog.

use std::path::Path;
use std::rc::Rc;

use rfd::AsyncFileDialog;
use rustgrid_core::{ObjectKind, PageRequest, SortColumn};
use rustgrid_export::{ExportOptions, SqlDialect, TableWriter, default_output_path};

use super::*;

/// Rows fetched per page while exporting. Large enough to keep round trips low, small enough to
/// bound the memory a single page holds.
const EXPORT_PAGE_SIZE: u64 = 5_000;

impl AppView {
    // ----- Opening -----------------------------------------------------------------------------

    /// Open the export wizard for an open database. Every object of the database is listed on P2
    /// with `preselected` (the caller's selection) ticked; the first preselected table is the one
    /// P3 opens on.
    pub(super) fn open_export_wizard(
        &mut self,
        connection_index: usize,
        database_index: usize,
        preselected: &[String],
        cx: &mut Context<'_, Self>,
    ) {
        if self.export_wizard.is_some() {
            return;
        }
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        let tables = self.database_object_names(connection_index, database_index);
        if tables.is_empty() {
            return;
        }
        let output_dir = default_export_dir();
        let format = ExportFormat::Xlsx;
        let preset: std::collections::HashSet<&str> =
            preselected.iter().map(String::as_str).collect();
        let tables: Vec<ExportTablePlan> = tables
            .into_iter()
            .map(|name| {
                let selected = preset.contains(name.as_str());
                // Only the initially selected tables have an output path; the rest are filled in
                // when the user ticks them, so an unticked row never shows a stale directory.
                let path = if selected {
                    default_output_path(&output_dir, &name, format)
                } else {
                    String::new()
                };
                ExportTablePlan {
                    name,
                    selected,
                    path,
                }
            })
            .collect();
        // P3 opens on the first ticked table, or the first row when nothing was preselected.
        let field_table = tables.iter().position(|table| table.selected).unwrap_or(0);

        let weak = cx.weak_entity();
        let dir_input = make_export_dir_input(self.theme, output_dir.clone(), &weak, cx);
        self.export_wizard = Some(ExportWizard {
            connection_index,
            database,
            step: ExportStep::Format,
            format,
            output_dir,
            tables,
            fields: BTreeMap::new(),
            field_table,
            field_combo: None,
            dir_input: Some(dir_input),
            include_header: true,
            continue_on_error: true,
            running: false,
            log: Vec::new(),
            log_scroll: ScrollHandle::new(),
            rows_total: 0,
            rows_done: 0,
            started: None,
            elapsed: None,
            error: None,
        });
        self.open_export_window(cx);
        cx.notify();
    }

    /// The table-kind object names of an open database, in catalog order.
    pub(super) fn database_table_names(
        &self,
        connection_index: usize,
        database_index: usize,
    ) -> Vec<String> {
        self.connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index),
                _ => None,
            })
            .and_then(|database| match &database.tables {
                Loadable::Loaded(tables) => Some(tables),
                _ => None,
            })
            .map(|tables| {
                tables
                    .iter()
                    .filter(|table| matches!(table.kind, ObjectKind::Table))
                    .map(|table| table.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every exportable object name of an open database (tables and views), in catalog order.
    pub(super) fn database_object_names(
        &self,
        connection_index: usize,
        database_index: usize,
    ) -> Vec<String> {
        self.connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index),
                _ => None,
            })
            .and_then(|database| match &database.tables {
                Loadable::Loaded(tables) => Some(tables),
                _ => None,
            })
            .map(|tables| tables.iter().map(|table| table.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Open the OS window hosting the export wizard (mirrors the Backup/Restore window).
    fn open_export_window(&mut self, cx: &mut Context<'_, Self>) {
        if self.export_window.is_some() {
            return;
        }
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        let title = t!("export.title").to_string();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(660.0), px(560.0)), cx);
            let view_weak = weak.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.clone().into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    #[cfg(target_os = "windows")]
                    crate::win_resize::install(window);
                    // `open_window` does not raise what it opens, so the wizard can otherwise
                    // appear behind the main window.
                    window.activate_window();
                    let view = cx.new(|cx| ExportWindow::new(view_weak.clone(), &app_entity, cx));
                    cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
                },
            );
            match opened {
                Ok(handle) => app.update(cx, |app, cx| {
                    app.export_window = Some(handle);
                    cx.notify();
                }),
                Err(error) => app.update(cx, |app, cx| {
                    app.error_dialog = Some(error.to_string());
                    cx.notify();
                }),
            }
        });
    }

    /// Close the Export window (called from its footer, where the window is at hand).
    pub(super) fn export_close(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.export_wizard = None;
        self.export_window = None;
        window.remove_window();
        cx.notify();
    }

    // ----- Navigation --------------------------------------------------------------------------

    fn export_set_format(&mut self, format: ExportFormat, cx: &mut Context<'_, Self>) {
        let Some(wizard) = self.export_wizard.as_mut() else {
            return;
        };
        if wizard.format == format {
            return;
        }
        wizard.format = format;
        // Re-derive every default path so the extension follows the chosen format.
        let output_dir = wizard.output_dir.clone();
        for table in wizard.tables.iter_mut() {
            table.path = default_output_path(&output_dir, &table.name, format);
        }
        cx.notify();
    }

    pub(super) fn export_next(&mut self, cx: &mut Context<'_, Self>) {
        let Some(step) = self.export_wizard.as_ref().map(|wizard| wizard.step) else {
            return;
        };
        if self
            .export_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.running)
        {
            return;
        }
        match step {
            ExportStep::Format => {
                self.set_export_step(ExportStep::Tables, cx);
            }
            ExportStep::Tables => {
                let (any, missing_path) = self
                    .export_wizard
                    .as_ref()
                    .map(|wizard| {
                        (
                            wizard.tables.iter().any(|table| table.selected),
                            wizard
                                .tables
                                .iter()
                                .any(|table| table.selected && table.path.trim().is_empty()),
                        )
                    })
                    .unwrap_or((false, false));
                if !any {
                    self.set_export_error(t!("export.no_tables").to_string(), cx);
                    return;
                }
                if missing_path {
                    self.set_export_error(t!("export.no_path").to_string(), cx);
                    return;
                }
                self.set_export_error(String::new(), cx);
                self.set_export_step(ExportStep::Fields, cx);
                self.export_enter_fields(cx);
            }
            ExportStep::Fields => {
                if let Some(error) = self.export_validate_fields() {
                    self.set_export_error(error, cx);
                    return;
                }
                self.set_export_error(String::new(), cx);
                // Re-entering the run page is a fresh run: drop the previous result so the footer
                // offers Start again instead of a stale Close.
                if let Some(wizard) = self.export_wizard.as_mut() {
                    wizard.elapsed = None;
                    wizard.rows_total = 0;
                    wizard.rows_done = 0;
                    wizard.log.clear();
                }
                self.set_export_step(ExportStep::Options, cx);
            }
            ExportStep::Options => {}
        }
    }

    pub(super) fn export_back(&mut self, cx: &mut Context<'_, Self>) {
        let Some(step) = self.export_wizard.as_ref().map(|wizard| wizard.step) else {
            return;
        };
        if self
            .export_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.running)
        {
            return;
        }
        let previous = match step {
            ExportStep::Format => return,
            ExportStep::Tables => ExportStep::Format,
            ExportStep::Fields => ExportStep::Tables,
            ExportStep::Options => ExportStep::Fields,
        };
        self.set_export_error(String::new(), cx);
        self.set_export_step(previous, cx);
    }

    fn set_export_step(&mut self, step: ExportStep, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.export_wizard.as_mut() {
            wizard.step = step;
        }
        cx.notify();
    }

    fn set_export_error(&mut self, error: String, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.export_wizard.as_mut() {
            wizard.error = if error.is_empty() { None } else { Some(error) };
        }
        cx.notify();
    }

    fn export_validate_fields(&self) -> Option<String> {
        let wizard = self.export_wizard.as_ref()?;
        for table in wizard.tables.iter().filter(|table| table.selected) {
            let Some(fields) = wizard.fields.get(&table.name) else {
                return Some(t!("export.fields_loading").to_string());
            };
            if fields.loading {
                return Some(t!("export.fields_loading").to_string());
            }
            if let Some(error) = &fields.error {
                return Some(error.clone());
            }
            if fields.selected_columns().is_empty() {
                return Some(t!("export.no_fields").to_string());
            }
        }
        None
    }

    // ----- P2: tables and paths ----------------------------------------------------------------

    pub(super) fn export_toggle_table(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let format = self
            .export_wizard
            .as_ref()
            .map(|wizard| wizard.format)
            .unwrap_or(ExportFormat::Xlsx);
        let output_dir = self
            .export_wizard
            .as_ref()
            .map(|wizard| wizard.output_dir.clone())
            .unwrap_or_default();
        if let Some(wizard) = self.export_wizard.as_mut()
            && let Some(table) = wizard.tables.get_mut(index)
        {
            table.selected = !table.selected;
            if table.selected {
                table.path = default_output_path(&output_dir, &table.name, format);
            } else {
                table.path.clear();
            }
        }
        cx.notify();
    }

    pub(super) fn export_set_all_tables(&mut self, selected: bool, cx: &mut Context<'_, Self>) {
        let Some(wizard) = self.export_wizard.as_mut() else {
            return;
        };
        let format = wizard.format;
        let output_dir = wizard.output_dir.clone();
        for table in wizard.tables.iter_mut() {
            table.selected = selected;
            if selected {
                if table.path.trim().is_empty() {
                    table.path = default_output_path(&output_dir, &table.name, format);
                }
            } else {
                table.path.clear();
            }
        }
        cx.notify();
    }

    fn export_set_output_dir(&mut self, directory: &str, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.export_wizard.as_mut() {
            wizard.output_dir = directory.to_string();
            let format = wizard.format;
            for table in wizard.tables.iter_mut().filter(|table| table.selected) {
                table.path = default_output_path(directory, &table.name, format);
            }
        }
        cx.notify();
    }

    /// Open the native folder picker. The dialog runs on rfd's own thread (via the async API) and
    /// is awaited from a gpui task: the synchronous API would run a nested Windows message loop
    /// inside gpui's event handling and crash.
    pub(super) fn export_browse_dir(&mut self, cx: &mut Context<'_, Self>) {
        let Some((start, input)) = self
            .export_wizard
            .as_ref()
            .map(|wizard| (wizard.output_dir.clone(), wizard.dir_input.clone()))
        else {
            return;
        };
        let title = t!("export.directory").to_string();
        cx.spawn(async move |this, cx| {
            let mut dialog = AsyncFileDialog::new().set_title(title);
            if !start.is_empty() {
                dialog = dialog.set_directory(&start);
            }
            let Some(handle) = dialog.pick_folder().await else {
                return;
            };
            let directory = handle.path().to_string_lossy().into_owned();
            let _ = this.update(cx, move |app, cx| {
                app.export_set_output_dir(&directory, cx);
                if let Some(input) = input {
                    input.update(cx, |input, cx| input.set_text(directory.clone(), cx));
                }
            });
        })
        .detach();
    }

    /// Open the native "save file" picker for one table's output path (see [`Self::export_browse_dir`]).
    pub(super) fn export_browse_file(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some((path, format)) = self.export_wizard.as_ref().and_then(|wizard| {
            wizard
                .tables
                .get(index)
                .map(|table| (table.path.clone(), wizard.format))
        }) else {
            return;
        };
        let title = t!("export.export_to").to_string();
        let filter_name = t!(format.label_key()).to_string();
        let extension = format.extension();
        cx.spawn(async move |this, cx| {
            let mut dialog = AsyncFileDialog::new()
                .set_title(title)
                .add_filter(filter_name, &[extension]);
            if let Some(parent) = Path::new(&path).parent()
                && !parent.as_os_str().is_empty()
            {
                dialog = dialog.set_directory(parent);
            }
            if let Some(name) = Path::new(&path).file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy().to_string());
            }
            let Some(handle) = dialog.save_file().await else {
                return;
            };
            let file = handle.path().to_string_lossy().into_owned();
            let _ = this.update(cx, move |app, cx| {
                if let Some(wizard) = app.export_wizard.as_mut()
                    && let Some(table) = wizard.tables.get_mut(index)
                {
                    table.path = file;
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ----- P3: fields --------------------------------------------------------------------------

    /// Move to the fields page: normalise the active table, rebuild the source-table dropdown and
    /// load any missing column lists.
    fn export_enter_fields(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.export_wizard.as_mut() {
            let first = wizard
                .tables
                .iter()
                .position(|table| table.selected)
                .unwrap_or(0);
            if !wizard
                .tables
                .get(wizard.field_table)
                .is_some_and(|table| table.selected)
            {
                wizard.field_table = first;
            }
        }
        self.export_rebuild_field_combo(cx);
        self.export_load_fields(cx);
    }

    /// (Re)build the P3 source-table dropdown from the currently selected tables. It is dropped
    /// when only one table is selected, in which case P3 shows the name as plain text.
    fn export_rebuild_field_combo(&mut self, cx: &mut Context<'_, Self>) {
        let theme = self.theme;
        let weak = cx.weak_entity();
        let Some(wizard) = self.export_wizard.as_mut() else {
            return;
        };
        let selected: Vec<(usize, String)> = wizard
            .tables
            .iter()
            .enumerate()
            .filter(|(_, table)| table.selected)
            .map(|(index, table)| (index, table.name.clone()))
            .collect();
        if selected.len() <= 1 {
            wizard.field_combo = None;
            return;
        }
        let current = wizard
            .tables
            .get(wizard.field_table)
            .map(|table| table.name.clone())
            .unwrap_or_default();
        let options: Vec<ComboOption> = selected
            .iter()
            .map(|(_, name)| ComboOption::plain(name.clone()))
            .collect();
        let combo = cx.new(move |cx| {
            ComboBox::new(theme, options, current, 240.0, cx)
                .field_width(240.0)
                .on_select(Rc::new(move |value: &str, _window, cx| {
                    let name = value.to_string();
                    let _ = weak.update(cx, |app, cx| app.export_set_field_table(&name, cx));
                }))
        });
        wizard.field_combo = Some(combo);
    }

    fn export_set_field_table(&mut self, name: &str, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.export_wizard.as_mut()
            && let Some(index) = wizard.tables.iter().position(|table| table.name == name)
        {
            wizard.field_table = index;
        }
        cx.notify();
    }

    /// Load the column lists of every selected table that has not been loaded yet.
    fn export_load_fields(&mut self, cx: &mut Context<'_, Self>) {
        let Some(wizard) = self.export_wizard.as_ref() else {
            return;
        };
        let connection_index = wizard.connection_index;
        let database = wizard.database.clone();
        let Some(connection) = self.connection_arc(connection_index) else {
            if let Some(wizard) = self.export_wizard.as_mut() {
                wizard.error = Some(t!("export.not_connected").to_string());
            }
            cx.notify();
            return;
        };
        let pending: Vec<String> = wizard
            .tables
            .iter()
            .filter(|table| table.selected)
            .map(|table| table.name.clone())
            .filter(|name| {
                let fields = wizard.fields.get(name);
                fields.is_none()
                    || fields.is_some_and(|fields| {
                        !fields.loading && fields.error.is_some() && fields.columns.is_empty()
                    })
            })
            .collect();
        if pending.is_empty() {
            return;
        }
        for name in &pending {
            if let Some(wizard) = self.export_wizard.as_mut() {
                wizard.fields.insert(
                    name.clone(),
                    ExportFields {
                        columns: Vec::new(),
                        selected: Vec::new(),
                        primary_key: Vec::new(),
                        loading: true,
                        error: None,
                    },
                );
            }
        }
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            for name in pending {
                let connection = connection.clone();
                let database = database.clone();
                let table = name.clone();
                let result = runtime
                    .spawn(async move { connection.columns(&database, &table).await })
                    .await;
                let _ = this.update(cx, |app, cx| {
                    if let Some(wizard) = app.export_wizard.as_mut() {
                        let fields = match result {
                            Ok(Ok(columns)) => ExportFields::all(columns),
                            Ok(Err(error)) => ExportFields {
                                columns: Vec::new(),
                                selected: Vec::new(),
                                primary_key: Vec::new(),
                                loading: false,
                                error: Some(error.to_string()),
                            },
                            Err(error) => ExportFields {
                                columns: Vec::new(),
                                selected: Vec::new(),
                                primary_key: Vec::new(),
                                loading: false,
                                error: Some(error.to_string()),
                            },
                        };
                        wizard.fields.insert(name, fields);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn export_toggle_field(
        &mut self,
        table: &str,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(wizard) = self.export_wizard.as_mut()
            && let Some(fields) = wizard.fields.get_mut(table)
            && let Some(selected) = fields.selected.get_mut(index)
        {
            *selected = !*selected;
        }
        cx.notify();
    }

    /// Set every field of the P3 active table.
    pub(super) fn export_set_all_fields(&mut self, selected: bool, cx: &mut Context<'_, Self>) {
        let Some(name) = self.export_active_table_name() else {
            return;
        };
        if let Some(wizard) = self.export_wizard.as_mut()
            && let Some(fields) = wizard.fields.get_mut(&name)
        {
            for value in fields.selected.iter_mut() {
                *value = selected;
            }
        }
        cx.notify();
    }

    /// The "All Fields" check box: select all when not every field is selected, else clear all.
    pub(super) fn export_toggle_all_fields(&mut self, cx: &mut Context<'_, Self>) {
        let Some(name) = self.export_active_table_name() else {
            return;
        };
        if let Some(wizard) = self.export_wizard.as_mut()
            && let Some(fields) = wizard.fields.get_mut(&name)
        {
            let target = !fields.all_selected();
            for value in fields.selected.iter_mut() {
                *value = target;
            }
        }
        cx.notify();
    }

    fn export_active_table_name(&self) -> Option<String> {
        let wizard = self.export_wizard.as_ref()?;
        wizard
            .tables
            .get(wizard.field_table)
            .map(|table| table.name.clone())
    }

    // ----- P4: options and run -----------------------------------------------------------------

    pub(super) fn export_toggle_include_header(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.export_wizard.as_mut() {
            wizard.include_header = !wizard.include_header;
        }
        cx.notify();
    }

    pub(super) fn export_toggle_continue_on_error(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.export_wizard.as_mut() {
            wizard.continue_on_error = !wizard.continue_on_error;
        }
        cx.notify();
    }

    pub(super) fn export_start(&mut self, cx: &mut Context<'_, Self>) {
        let Some(wizard) = self.export_wizard.as_ref() else {
            return;
        };
        if wizard.running {
            return;
        }
        if let Some(error) = self.export_validate_fields() {
            self.set_export_error(error, cx);
            return;
        }
        // Snapshot everything the run needs so no `AppView` borrow is held across the awaits.
        let plans: Vec<(String, String, Vec<String>, Vec<SortColumn>)> = wizard
            .tables
            .iter()
            .filter(|table| table.selected)
            .map(|table| {
                let fields = wizard.fields.get(&table.name);
                let columns = fields
                    .map(ExportFields::selected_columns)
                    .unwrap_or_default();
                let order_by = fields
                    .map(|fields| {
                        fields
                            .primary_key
                            .iter()
                            .map(|column| SortColumn {
                                column: column.clone(),
                                descending: false,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (table.name.clone(), table.path.clone(), columns, order_by)
            })
            .collect();
        if plans.is_empty() {
            self.set_export_error(t!("export.no_tables").to_string(), cx);
            return;
        }
        if plans.iter().any(|(_, path, _, _)| path.trim().is_empty()) {
            self.set_export_error(t!("export.no_path").to_string(), cx);
            return;
        }
        let connection_index = wizard.connection_index;
        let database = wizard.database.clone();
        let format = wizard.format;
        let options = ExportOptions {
            include_header: wizard.include_header,
        };
        let continue_on_error = wizard.continue_on_error;
        let Some(connection) = self.connection_arc(connection_index) else {
            self.set_export_error(t!("export.not_connected").to_string(), cx);
            return;
        };

        if let Some(wizard) = self.export_wizard.as_mut() {
            wizard.running = true;
            wizard.error = None;
            wizard.log.clear();
            wizard.rows_total = 0;
            wizard.rows_done = 0;
            wizard.elapsed = None;
            wizard.started = Some(std::time::Instant::now());
            wizard
                .log
                .push(t!("export.log.started", time = now_timestamp()).to_string());
            wizard.log.push(
                t!(
                    "export.log.start",
                    count = plans.len(),
                    database = database.clone()
                )
                .to_string(),
            );
            wizard.log_scroll.scroll_to_bottom();
        }
        cx.notify();

        let runtime = self.runtime.clone();
        let dialect = SqlDialect::MySql;
        cx.spawn(async move |this, cx| {
            let mut exported = 0usize;
            let mut failed = 0usize;
            for (name, path, columns, order_by) in plans {
                let created = TableWriter::create(
                    Path::new(&path),
                    &name,
                    &columns,
                    format,
                    options,
                    dialect,
                );
                let mut writer = match created {
                    Ok(writer) => writer,
                    Err(error) => {
                        let message = t!(
                            "export.log.table_failed",
                            name = name.clone(),
                            error = error.to_string()
                        )
                        .to_string();
                        let _ = this.update(cx, |app, cx| app.export_log(message, cx));
                        failed += 1;
                        if !continue_on_error {
                            break;
                        }
                        continue;
                    }
                };

                let mut page_index = 0u64;
                let mut table_rows = 0usize;
                let mut projection: Option<Vec<usize>> = None;
                let mut identity = false;
                let mut error: Option<String> = None;
                loop {
                    let connection = connection.clone();
                    let database = database.clone();
                    let table = name.clone();
                    let request = PageRequest::new(page_index, EXPORT_PAGE_SIZE)
                        .with_order_by(order_by.clone());
                    let result = runtime
                        .spawn(
                            async move { connection.fetch_page(&database, &table, request).await },
                        )
                        .await;
                    let page = match result {
                        Ok(Ok(page)) => page,
                        Ok(Err(err)) => {
                            error = Some(err.to_string());
                            break;
                        }
                        Err(err) => {
                            error = Some(err.to_string());
                            break;
                        }
                    };
                    let fetched = page.rows.len();
                    if page_index == 0
                        && let Some(total) = page.total_rows
                    {
                        let total = total as usize;
                        let _ = this.update(cx, |app, cx| {
                            if let Some(wizard) = app.export_wizard.as_mut() {
                                wizard.rows_total += total;
                            }
                            cx.notify();
                        });
                    }
                    if projection.is_none() {
                        let mut indices = Vec::with_capacity(columns.len());
                        for column in &columns {
                            match page.columns.iter().position(|info| &info.name == column) {
                                Some(index) => indices.push(index),
                                None => {
                                    error = Some(format!("column `{column}` is missing"));
                                    break;
                                }
                            }
                        }
                        if error.is_some() {
                            break;
                        }
                        identity = indices.len() == page.columns.len()
                            && indices
                                .iter()
                                .enumerate()
                                .all(|(position, index)| position == *index);
                        projection = Some(indices);
                    }
                    let rows = match (&projection, identity) {
                        (Some(indices), false) => page
                            .rows
                            .into_iter()
                            .map(|row| {
                                indices
                                    .iter()
                                    .filter_map(|index| row.get(*index).cloned())
                                    .collect()
                            })
                            .collect(),
                        _ => page.rows,
                    };
                    if let Err(write_error) = writer.write_rows(&rows) {
                        error = Some(write_error.to_string());
                        break;
                    }
                    table_rows += fetched;
                    let _ = this.update(cx, |app, cx| {
                        if let Some(wizard) = app.export_wizard.as_mut() {
                            wizard.rows_done += fetched;
                            wizard.log_scroll.scroll_to_bottom();
                        }
                        cx.notify();
                    });
                    if (fetched as u64) < EXPORT_PAGE_SIZE {
                        break;
                    }
                    page_index += 1;
                }

                let finish = match error {
                    Some(error) => Err(error),
                    None => writer.finish().map_err(|error| error.to_string()),
                };
                match finish {
                    Ok(()) => {
                        exported += 1;
                        let message =
                            t!("export.log.table", name = name.clone(), rows = table_rows)
                                .to_string();
                        let _ = this.update(cx, |app, cx| app.export_log(message, cx));
                    }
                    Err(error) => {
                        failed += 1;
                        let message = t!(
                            "export.log.table_failed",
                            name = name.clone(),
                            error = error
                        )
                        .to_string();
                        let _ = this.update(cx, |app, cx| app.export_log(message, cx));
                        if !continue_on_error {
                            break;
                        }
                    }
                }
            }
            let _ = this.update(cx, |app, cx| app.export_finish(exported, failed, cx));
        })
        .detach();
    }

    fn export_log(&mut self, message: String, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.export_wizard.as_mut() {
            wizard.log.push(message);
            wizard.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn export_finish(&mut self, exported: usize, failed: usize, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.export_wizard.as_mut() {
            wizard.running = false;
            if let Some(started) = wizard.started.take() {
                let elapsed = started.elapsed();
                wizard.elapsed = Some(elapsed);
                wizard
                    .log
                    .push(t!("export.log.elapsed", time = format_hms(elapsed)).to_string());
            }
            wizard
                .log
                .push(t!("export.log.done", count = exported, failed = failed).to_string());
            wizard.rows_total = wizard.rows_total.max(wizard.rows_done);
            wizard.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    // ----- Window contents ---------------------------------------------------------------------

    /// The contents of the Export Wizard window: titlebar, page heading, page body and footer.
    pub(super) fn export_window_contents(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(wizard) = self.export_wizard.as_ref() else {
            return div().into_any_element();
        };
        let (page, total) = wizard.step.index();
        let heading_key = match wizard.step {
            ExportStep::Format => "export.step.format",
            ExportStep::Tables => "export.step.tables",
            ExportStep::Fields => "export.step.fields",
            ExportStep::Options => "export.step.options",
        };
        let heading = div()
            .px_3()
            .py_3()
            .flex_none()
            .text_size(px(13.0))
            .text_color(rgb(theme.grid_selection_bg))
            .child(format!("{} ({page}/{total})", t!(heading_key)));
        let body = match wizard.step {
            ExportStep::Format => self.render_export_format_page(cx),
            ExportStep::Tables => self.render_export_tables_page(cx),
            ExportStep::Fields => self.render_export_fields_page(cx),
            ExportStep::Options => self.render_export_options_page(cx),
        };
        let error = div()
            .px_3()
            .pb_1()
            .flex_none()
            .text_size(px(12.0))
            .text_color(rgb(theme.danger))
            .child(wizard.error.clone().unwrap_or_default());
        let content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(body)
            .child(error);
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.dialog_face))
            .text_color(rgb(theme.text))
            .child(child_window_titlebar(t!("export.title").to_string(), theme))
            .child(heading)
            .child(content)
            .child(self.render_export_footer(cx).into_any_element())
            .into_any_element()
    }

    fn render_export_format_page(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(wizard) = self.export_wizard.as_ref() else {
            return div().into_any_element();
        };
        let mut list = div().flex().flex_col().gap_1().px_3().child(
            div()
                .text_size(px(12.0))
                .child(t!("export.format_label").to_string()),
        );
        for format in ExportFormat::ALL {
            let selected = wizard.format == format;
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "export-format-{}",
                        format.extension()
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(22.0))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.export_set_format(format, cx)
                    }))
                    .child(ui::radio_box(selected, theme))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child(t!(format.label_key()).to_string()),
                    ),
            );
        }
        list.into_any_element()
    }

    fn render_export_tables_page(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(wizard) = self.export_wizard.as_ref() else {
            return div().into_any_element();
        };

        let directory_row = {
            let input = wizard
                .dir_input
                .clone()
                .map(|input| {
                    div()
                        .w(px(360.0))
                        .h(px(24.0))
                        .child(input)
                        .into_any_element()
                })
                .unwrap_or_else(|| div().into_any_element());
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .pb_2()
                .child(
                    div()
                        .w(px(80.0))
                        .flex_none()
                        .text_size(px(12.0))
                        .child(t!("export.directory").to_string()),
                )
                .child(input)
                .child(self.dialog_button(
                    "export-browse-dir",
                    t!("export.browse").to_string(),
                    false,
                    cx.listener(|this, _event, _window, cx| this.export_browse_dir(cx)),
                ))
        };

        let mut list = div()
            .id("export-table-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(
                        div()
                            .w(px(220.0))
                            .flex_none()
                            .child(t!("export.table").to_string()),
                    )
                    .child(div().flex_1().child(t!("export.export_to").to_string())),
            );
        for (index, table) in wizard.tables.iter().enumerate() {
            let name = table.name.clone();
            let selected = table.selected;
            // An unticked row has no output path, so a directory change never leaves a stale path
            // behind on a table that is not being exported.
            let path = if selected {
                table.path.clone()
            } else {
                String::new()
            };
            list = list.child(
                div()
                    .id(SharedString::from(format!("export-table-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .h(px(24.0))
                    .cursor_pointer()
                    .when(selected, move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.export_toggle_table(index, cx);
                    }))
                    .child(checkbox_box(selected, theme))
                    .child(tree_icon("icons/tables.svg", theme.icon_table))
                    .child(
                        div()
                            .w(px(180.0))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.0))
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text_muted))
                            .child(path),
                    )
                    .child(export_small_button(
                        format!("export-file-{index}"),
                        selected,
                        theme,
                        cx.listener(move |this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.export_browse_file(index, cx);
                        }),
                    )),
            );
        }

        let buttons = div()
            .flex()
            .flex_row()
            .gap_2()
            .px_3()
            .py_2()
            .child(self.dialog_button(
                "export-select-all-tables",
                t!("export.select_all").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.export_set_all_tables(true, cx)),
            ))
            .child(self.dialog_button(
                "export-select-none-tables",
                t!("export.select_none").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.export_set_all_tables(false, cx)),
            ));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(directory_row)
            .child(list)
            .child(buttons)
            .into_any_element()
    }

    fn render_export_fields_page(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(wizard) = self.export_wizard.as_ref() else {
            return div().into_any_element();
        };
        let Some(table) = wizard.tables.get(wizard.field_table) else {
            return div().into_any_element();
        };
        let table_name = table.name.clone();
        let selected_count = wizard.tables.iter().filter(|table| table.selected).count();
        let source = if selected_count > 1 {
            wizard
                .field_combo
                .clone()
                .map(|combo| div().w(px(240.0)).child(combo).into_any_element())
                .unwrap_or_else(|| div().into_any_element())
        } else {
            div()
                .flex()
                .items_center()
                .h(px(24.0))
                .text_size(px(12.0))
                .child(table_name.clone())
                .into_any_element()
        };

        let fields = wizard.fields.get(&table_name);
        let mut list = div()
            .id("export-field-list")
            .flex()
            .flex_col()
            .gap_0p5()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px_3();
        match fields {
            None => {}
            Some(fields) if fields.loading => {
                list = list.child(
                    div()
                        .py_2()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("common.loading").to_string()),
                );
            }
            Some(fields) if fields.error.is_some() => {
                list = list.child(
                    div()
                        .py_2()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.danger))
                        .child(fields.error.clone().unwrap_or_default()),
                );
            }
            Some(fields) => {
                for (index, name) in fields.columns.iter().enumerate() {
                    let selected = fields.selected.get(index).copied().unwrap_or(false);
                    let field_name = name.clone();
                    let field_table = table_name.clone();
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("export-field-{index}")))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .h(px(20.0))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.export_toggle_field(&field_table, index, cx);
                            }))
                            .child(checkbox_box(selected, theme))
                            .child(div().text_size(px(12.0)).child(field_name)),
                    );
                }
            }
        }

        let all_selected = fields.is_some_and(ExportFields::all_selected);
        let buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .child(self.dialog_button(
                "export-fields-all",
                t!("export.select_all").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.export_set_all_fields(true, cx)),
            ))
            .child(self.dialog_button(
                "export-fields-none",
                t!("export.select_none").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.export_set_all_fields(false, cx)),
            ))
            .child(
                div()
                    .id("export-fields-toggle-all")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(
                        cx.listener(|this, _event, _window, cx| this.export_toggle_all_fields(cx)),
                    )
                    .child(checkbox_box(all_selected, theme))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child(t!("export.all_fields").to_string()),
                    ),
            );

        let source_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .pb_2()
            .child(
                div()
                    .w(px(80.0))
                    .flex_none()
                    .text_size(px(12.0))
                    .child(t!("export.source_table").to_string()),
            )
            .child(source);

        let fields_label = div()
            .px_3()
            .pb_1()
            .text_size(px(12.0))
            .child(t!("export.available_fields").to_string());

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(source_row)
            .child(fields_label)
            .child(list)
            .child(buttons)
            .into_any_element()
    }

    fn render_export_options_page(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(wizard) = self.export_wizard.as_ref() else {
            return div().into_any_element();
        };
        let options = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .pb_2()
            .child(export_check_row(
                "export-opt-header",
                t!("export.include_header").to_string(),
                wizard.include_header,
                theme,
                cx.listener(|this, _event, _window, cx| this.export_toggle_include_header(cx)),
            ))
            .child(export_check_row(
                "export-opt-continue",
                t!("export.continue_on_error").to_string(),
                wizard.continue_on_error,
                theme,
                cx.listener(|this, _event, _window, cx| this.export_toggle_continue_on_error(cx)),
            ));

        let source = wizard
            .tables
            .iter()
            .filter(|table| table.selected)
            .map(|table| table.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let elapsed = if wizard.running {
            wizard
                .started
                .map(|started| started.elapsed())
                .or(wizard.elapsed)
        } else {
            wizard.elapsed
        };
        let summary = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .px_3()
            .pb_2()
            .child(export_summary_row(
                t!("export.source").to_string(),
                source,
                theme,
            ))
            .child(export_summary_row(
                t!("export.total").to_string(),
                wizard.rows_total.to_string(),
                theme,
            ))
            .child(export_summary_row(
                t!("export.processed").to_string(),
                wizard.rows_done.to_string(),
                theme,
            ))
            .child(export_summary_row(
                t!("export.time").to_string(),
                elapsed.map(format_hms).unwrap_or_else(|| "--".to_string()),
                theme,
            ));

        let lines = if wizard.log.is_empty() {
            vec![
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child("--")
                    .into_any_element(),
            ]
        } else {
            wizard
                .log
                .iter()
                .map(|line| {
                    div()
                        .text_size(px(12.0))
                        .child(line.clone())
                        .into_any_element()
                })
                .collect()
        };
        let log = div()
            .id("export-log")
            .flex()
            .flex_col()
            .gap_0p5()
            .flex_1()
            .min_h(px(0.0))
            .mx_3()
            .my_2()
            .p_2()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.dialog_bg))
            .overflow_y_scroll()
            .track_scroll(&wizard.log_scroll)
            .children(lines);

        let total = wizard.rows_total.max(wizard.rows_done);
        let progress = if total == 0 {
            0.0
        } else {
            (wizard.rows_done as f32 / total as f32).clamp(0.0, 1.0)
        };
        let bar = div()
            .h(px(12.0))
            .mx_3()
            .mb_3()
            .flex_none()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.scroll_track))
            .child(div().h_full().w(gpui::relative(progress)).bg(rgb(0x22b14c)));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(options)
            .child(summary)
            .child(log)
            .child(bar)
            .into_any_element()
    }

    fn render_export_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let step = self
            .export_wizard
            .as_ref()
            .map(|wizard| wizard.step)
            .unwrap_or(ExportStep::Format);
        let running = self
            .export_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.running);
        // Once a run has finished the primary button becomes Close, so the footer is not left
        // offering Start again on completed data.
        let finished = !running
            && self
                .export_wizard
                .as_ref()
                .is_some_and(|wizard| wizard.elapsed.is_some());
        let mut right = div().flex().flex_row().items_center().gap_2().flex_none();
        if !finished {
            right = right.child(self.export_footer_button(
                "export-cancel",
                t!("export.cancel").to_string(),
                false,
                !running,
                cx.listener(|this, _event, window, cx| this.export_close(window, cx)),
            ));
        }
        if step != ExportStep::Format {
            right = right.child(self.export_footer_button(
                "export-back",
                t!("export.prev").to_string(),
                false,
                !running,
                cx.listener(|this, _event, _window, cx| this.export_back(cx)),
            ));
        }
        if step == ExportStep::Options {
            if finished {
                right = right.child(self.export_footer_button(
                    "export-close-done",
                    t!("export.close").to_string(),
                    true,
                    true,
                    cx.listener(|this, _event, window, cx| this.export_close(window, cx)),
                ));
            } else {
                right = right.child(self.export_footer_button(
                    "export-start",
                    t!("export.start").to_string(),
                    true,
                    !running,
                    cx.listener(|this, _event, _window, cx| this.export_start(cx)),
                ));
            }
        } else {
            right = right.child(self.export_footer_button(
                "export-next",
                t!("export.next").to_string(),
                true,
                !running,
                cx.listener(|this, _event, _window, cx| this.export_next(cx)),
            ));
        }
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .w_full()
            .h(px(46.0))
            .px_3()
            .child(right)
            .text_color(rgb(theme.text))
    }

    fn export_footer_button(
        &self,
        id: &'static str,
        label: String,
        primary: bool,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        if enabled {
            ui::dialog_button(id, label, primary, self.theme, on_click).into_any_element()
        } else {
            ui::button(id, label, ButtonKind::Disabled, self.theme, on_click).into_any_element()
        }
    }

    /// A footer push button shared with the Import Wizard.
    pub(super) fn wizard_footer_button(
        &self,
        id: &'static str,
        label: String,
        primary: bool,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        self.export_footer_button(id, label, primary, enabled, on_click)
    }
}

/// The root view of the Export Wizard OS window. It re-renders whenever `AppView` changes so the
/// window stays in sync while pages change and the export runs.
pub(super) struct ExportWindow {
    app: WeakEntity<AppView>,
    _subscription: Subscription,
}

impl ExportWindow {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        app_entity: &Entity<AppView>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let subscription = cx.observe(app_entity, |_, _, cx| cx.notify());
        Self {
            app,
            _subscription: subscription,
        }
    }
}

impl Render for ExportWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        app.update(cx, |app, cx| app.export_window_contents(cx))
    }
}

// ----- Free helpers ---------------------------------------------------------------------------

/// The directory new exports default into: the desktop, else the home directory.
pub(super) fn default_export_dir() -> String {
    if let Some(dirs) = directories::UserDirs::new() {
        if let Some(desktop) = dirs.desktop_dir() {
            return desktop.to_string_lossy().into_owned();
        }
        return dirs.home_dir().to_string_lossy().into_owned();
    }
    std::env::current_dir()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Build the P2 output-directory field. Edits rewrite `wizard.output_dir` and re-derive the
/// selected tables' default paths.
fn make_export_dir_input(
    theme: Theme,
    initial: String,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    cx.new(move |cx| {
        TextInput::new(theme, initial, TextInputOptions::default(), cx).on_change(Rc::new(
            move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| app.export_set_output_dir(text, cx));
            },
        ))
    })
}

/// The child window's titlebar (same chrome as the Backup window).
pub(super) fn child_window_titlebar(title: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(px(30.0))
        .flex_none()
        .bg(rgb(theme.titlebar_bg))
        .border_b_1()
        .border_color(rgb(theme.border))
        .child(
            div()
                .id("export-titlebar-drag")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .h_full()
                .px_3()
                .text_size(px(12.5))
                .window_control_area(WindowControlArea::Drag)
                .child(
                    img(ImageSource::Resource(Resource::Embedded("logo.png".into())))
                        .w(px(16.0))
                        .h(px(16.0))
                        .flex_none(),
                )
                .child(title),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h_full()
                .child(export_titlebar_button(
                    "export-titlebar-min",
                    "—",
                    theme,
                    |window, _cx| window.minimize_window(),
                ))
                .child(export_titlebar_button(
                    "export-titlebar-max",
                    "□",
                    theme,
                    |window, _cx| toggle_maximize(window),
                ))
                .child(export_titlebar_button(
                    "export-titlebar-close",
                    "✕",
                    theme,
                    |window, _cx| window.remove_window(),
                )),
        )
}

pub(super) fn export_titlebar_button(
    id: &'static str,
    label: &'static str,
    theme: Theme,
    action: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(40.0))
        .h_full()
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
        .on_click(move |_event, window, cx| action(window, cx))
        .child(label.to_string())
}

/// A clickable check-box row used by the options page.
pub(super) fn export_check_row(
    id: impl Into<SharedString>,
    label: String,
    checked: bool,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(22.0))
        .cursor_pointer()
        .on_click(on_click)
        .child(checkbox_box(checked, theme))
        .child(div().text_size(px(12.0)).child(label))
}

/// A small bordered "..." button, used to pick one table's output file. `enabled` is false for an
/// unticked table, which has no output path.
pub(super) fn export_small_button(
    id: String,
    enabled: bool,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut button = div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .justify_center()
        .w(px(22.0))
        .h(px(20.0))
        .flex_none()
        .rounded(px(3.0))
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.button_bg))
        .text_size(px(12.0));
    if enabled {
        button = button
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
            .on_click(on_click);
    } else {
        button = button.text_color(rgb(theme.text_muted));
    }
    button.child("…")
}

/// One `label    value` row of the options page summary block.
pub(super) fn export_summary_row(label: String, value: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .child(
            div()
                .w(px(80.0))
                .flex_none()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(value),
        )
}

/// Format an elapsed duration as `HH:MM:SS`.
pub(super) fn format_hms(elapsed: std::time::Duration) -> String {
    let total = elapsed.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total % 3600) / 60,
        total % 60
    )
}

/// The local wall-clock timestamp used by the export log.
pub(super) fn now_timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
