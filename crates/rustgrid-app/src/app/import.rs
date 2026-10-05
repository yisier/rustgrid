//! The Import Wizard: Navicat's data-import flow as a separate OS window.
//!
//! Four pages: choose a source kind (P1), pick the file and its destination tables (P2 — Navicat's
//! P2+P3 merged: the sheet list carries both the source sheet and the editable destination table),
//! map source fields to destination fields (P3), then run and log (P4). `AppView` owns the state
//! ([`ImportWizard`]); this module renders it, drives the transitions, reads spreadsheets through
//! [`rustgrid_import`] and writes through [`rustgrid_core::Connection`].
//!
//! The source file is chosen with the OS's native file dialog (via `rfd`), which is why the wizard
//! lives in a real window rather than a `Root` dialog.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use chrono::NaiveDateTime;
use dtparse::Parser;
use rfd::AsyncFileDialog;
use rustgrid_core::{ColumnDef, RowInsert, TableSchema};
use rustgrid_import::{SheetData, infer_columns};

use super::export::{
    child_window_titlebar, default_export_dir, export_check_row, export_summary_row, format_hms,
    now_timestamp,
};
use super::*;

/// Rows inserted per `insert_rows` call while importing: bounds memory and gives the log regular
/// progress updates.
const IMPORT_BATCH_ROWS: usize = 500;

impl AppView {
    // ----- Opening -----------------------------------------------------------------------------

    /// Open the import wizard for an open database. When `target_table` is set (the wizard was
    /// launched from a table grid), a single source table defaults to that destination table.
    pub(super) fn open_import_wizard(
        &mut self,
        connection_index: usize,
        database_index: usize,
        target_table: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.import_wizard.is_some() {
            return;
        }
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        let weak = cx.weak_entity();
        let file_input = make_import_file_input(self.theme, String::new(), &weak, cx);
        self.import_wizard = Some(ImportWizard {
            connection_index,
            database_index,
            database,
            step: ImportStep::Format,
            format: ImportFormat::Excel,
            file: String::new(),
            file_input: Some(file_input),
            sheets: Vec::new(),
            target_table,
            existing_tables: Vec::new(),
            fields: BTreeMap::new(),
            mapping_sheet: 0,
            sheet_combo: None,
            mapping_rows: BTreeMap::new(),
            running: false,
            log: Vec::new(),
            log_scroll: ScrollHandle::new(),
            rows_total: 0,
            rows_done: 0,
            imported_tables: 0,
            added: 0,
            updated: 0,
            deleted: 0,
            errors: 0,
            started: None,
            elapsed: None,
            error: None,
        });
        self.open_import_window(cx);
        cx.notify();
    }

    /// Open the OS window hosting the import wizard (mirrors the Export Wizard window).
    fn open_import_window(&mut self, cx: &mut Context<'_, Self>) {
        if self.import_window.is_some() {
            return;
        }
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        let title = t!("import.title").to_string();
        let focus = self.import_focus.clone();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(720.0), px(600.0)), cx);
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
                    let view = cx.new(|cx| ImportWindow::new(view_weak.clone(), &app_entity, cx));
                    let root = cx.new(|cx| gpui_kit::component::Root::new(view, window, cx));
                    // Focus the window root so ESC closes it before any control is focused.
                    window.focus(&focus, cx);
                    root
                },
            );
            match opened {
                Ok(handle) => app.update(cx, |app, cx| {
                    app.import_window = Some(handle);
                    cx.notify();
                }),
                Err(error) => app.update(cx, |app, cx| {
                    app.error_dialog = Some(error.to_string());
                    cx.notify();
                }),
            }
        });
    }

    /// Close the Import window (called from its footer, where the window is at hand).
    pub(super) fn import_close(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.import_wizard = None;
        self.import_window = None;
        window.remove_window();
        cx.notify();
    }

    // ----- Navigation --------------------------------------------------------------------------

    fn import_set_format(&mut self, format: ImportFormat, cx: &mut Context<'_, Self>) {
        let unchanged = self
            .import_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.format == format);
        if unchanged {
            return;
        }
        let Some(input) = self
            .import_wizard
            .as_ref()
            .map(|wizard| wizard.file_input.clone())
        else {
            return;
        };
        // The reader changed, so the previously picked file (and anything read from it) no longer
        // applies; clear it and let the user pick a file of the new kind.
        if let Some(wizard) = self.import_wizard.as_mut() {
            wizard.format = format;
            wizard.file.clear();
            wizard.sheets.clear();
            wizard.fields.clear();
            wizard.mapping_rows.clear();
            wizard.sheet_combo = None;
            wizard.error = None;
        }
        if let Some(input) = input {
            input.update(cx, |input, cx| input.set_text(String::new(), cx));
        }
        cx.notify();
    }

    pub(super) fn import_next(&mut self, cx: &mut Context<'_, Self>) {
        let Some(step) = self.import_wizard.as_ref().map(|wizard| wizard.step) else {
            return;
        };
        if self
            .import_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.running)
        {
            return;
        }
        match step {
            ImportStep::Format => self.set_import_step(ImportStep::Source, cx),
            ImportStep::Source => {
                // A path typed rather than browsed has not been read yet; read it now and stay on
                // the page until the sheets are listed.
                if self
                    .import_wizard
                    .as_ref()
                    .is_some_and(|wizard| wizard.sheets.is_empty())
                {
                    if self
                        .import_wizard
                        .as_ref()
                        .is_some_and(|wizard| !wizard.file.trim().is_empty())
                    {
                        self.import_load_file(cx);
                    }
                    return;
                }
                if let Some(error) = self.import_validate_source() {
                    self.set_import_error(error, cx);
                    return;
                }
                self.set_import_error(String::new(), cx);
                self.set_import_step(ImportStep::Mapping, cx);
                self.import_rebuild_mapping(cx);
                self.import_load_fields(cx);
            }
            ImportStep::Mapping => {
                if let Some(error) = self.import_validate_for_run() {
                    self.set_import_error(error, cx);
                    return;
                }
                self.set_import_error(String::new(), cx);
                // Re-entering the run page is a fresh run: drop the previous result so the footer
                // offers Start again instead of a stale Close.
                if let Some(wizard) = self.import_wizard.as_mut() {
                    wizard.elapsed = None;
                    wizard.imported_tables = 0;
                    wizard.rows_total = 0;
                    wizard.rows_done = 0;
                    wizard.added = 0;
                    wizard.updated = 0;
                    wizard.deleted = 0;
                    wizard.errors = 0;
                    wizard.log.clear();
                }
                self.set_import_step(ImportStep::Run, cx);
            }
            ImportStep::Run => {}
        }
    }

    pub(super) fn import_back(&mut self, cx: &mut Context<'_, Self>) {
        let Some(step) = self.import_wizard.as_ref().map(|wizard| wizard.step) else {
            return;
        };
        if self
            .import_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.running)
        {
            return;
        }
        let previous = match step {
            ImportStep::Format => return,
            ImportStep::Source => ImportStep::Format,
            ImportStep::Mapping => ImportStep::Source,
            ImportStep::Run => ImportStep::Mapping,
        };
        self.set_import_error(String::new(), cx);
        self.set_import_step(previous, cx);
    }

    fn set_import_step(&mut self, step: ImportStep, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.import_wizard.as_mut() {
            wizard.step = step;
        }
        cx.notify();
    }

    fn set_import_error(&mut self, error: String, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.import_wizard.as_mut() {
            wizard.error = if error.is_empty() { None } else { Some(error) };
        }
        cx.notify();
    }

    fn import_validate_source(&self) -> Option<String> {
        let wizard = self.import_wizard.as_ref()?;
        if wizard.file.trim().is_empty() {
            return Some(t!("import.no_file").to_string());
        }
        let selected: Vec<&ImportSheetPlan> = wizard
            .sheets
            .iter()
            .filter(|sheet| sheet.selected)
            .collect();
        if selected.is_empty() {
            return Some(t!("import.no_sheets").to_string());
        }
        if selected.iter().any(|sheet| sheet.target.trim().is_empty()) {
            return Some(t!("import.no_target").to_string());
        }
        None
    }

    fn import_validate_for_run(&self) -> Option<String> {
        let wizard = self.import_wizard.as_ref()?;
        if wizard.file.trim().is_empty() {
            return Some(t!("import.no_file").to_string());
        }
        let selected: Vec<&ImportSheetPlan> = wizard
            .sheets
            .iter()
            .filter(|sheet| sheet.selected)
            .collect();
        if selected.is_empty() {
            return Some(t!("import.no_sheets").to_string());
        }
        if selected.iter().any(|sheet| sheet.target.trim().is_empty()) {
            return Some(t!("import.no_target").to_string());
        }
        for sheet in selected {
            let Some(fields) = wizard.fields.get(&sheet.name) else {
                return Some(t!("import.fields_loading").to_string());
            };
            if fields.loading {
                return Some(t!("import.fields_loading").to_string());
            }
            if let Some(error) = &fields.error {
                return Some(error.clone());
            }
            if fields.mapping_columns().is_empty() {
                return Some(t!("import.no_mapping").to_string());
            }
        }
        None
    }

    // ----- P2: file and destination tables -----------------------------------------------------

    /// Open the native file picker, then read the picked workbook's sheet names.
    pub(super) fn import_browse_file(&mut self, cx: &mut Context<'_, Self>) {
        let Some((start, input, format)) = self.import_wizard.as_ref().map(|wizard| {
            (
                wizard.file.clone(),
                wizard.file_input.clone(),
                wizard.format,
            )
        }) else {
            return;
        };
        let title = t!("import.import_from").to_string();
        cx.spawn(async move |this, cx| {
            let start_dir = if start.is_empty() {
                default_export_dir()
            } else {
                Path::new(&start)
                    .parent()
                    .map(|parent| parent.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            let (filter_name, extensions): (&str, &[&str]) = match format {
                ImportFormat::Excel => ("Excel", &["xlsx", "xls", "xlsm", "xlsb", "ods"]),
                ImportFormat::Csv => ("CSV", &["csv"]),
                ImportFormat::Text => ("Text", &["txt"]),
            };
            let mut dialog = AsyncFileDialog::new()
                .set_title(title)
                .add_filter(filter_name, extensions);
            if !start_dir.is_empty() {
                dialog = dialog.set_directory(&start_dir);
            }
            let Some(handle) = dialog.pick_file().await else {
                return;
            };
            let file = handle.path().to_string_lossy().into_owned();
            let _ = this.update(cx, move |app, cx| {
                if let Some(wizard) = app.import_wizard.as_mut() {
                    wizard.file = file.clone();
                }
                if let Some(input) = input {
                    input.update(cx, |input, cx| input.set_text(file.clone(), cx));
                }
                app.import_load_file(cx);
            });
        })
        .detach();
    }

    /// Read the chosen file's table names and build one plan per table. A worksheet (or a delimited
    /// file) whose name matches an existing table targets it; otherwise the name becomes a new
    /// table and "create table" is ticked.
    pub(super) fn import_load_file(&mut self, cx: &mut Context<'_, Self>) {
        let Some(wizard) = self.import_wizard.as_ref() else {
            return;
        };
        let path = wizard.file.trim().to_string();
        if path.is_empty() {
            self.set_import_error(t!("import.no_file").to_string(), cx);
            return;
        }
        let kind = wizard.format.source_kind();
        let connection_index = wizard.connection_index;
        let database_index = wizard.database_index;
        let existing = self.database_table_names(connection_index, database_index);
        if let Some(wizard) = self.import_wizard.as_mut() {
            wizard.sheets.clear();
            wizard.fields.clear();
            wizard.mapping_rows.clear();
            wizard.sheet_combo = None;
            wizard.existing_tables = existing;
            wizard.error = None;
        }
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let job = path.clone();
            let result = runtime
                .spawn_blocking(move || kind.tables(Path::new(&job)))
                .await;
            let loaded = match result {
                Ok(Ok(names)) => names,
                Ok(Err(error)) => {
                    let message = t!("import.read_failed", error = error.to_string()).to_string();
                    let _ = this.update(cx, move |app, cx| app.set_import_error(message, cx));
                    return;
                }
                Err(error) => {
                    let message = t!("import.read_failed", error = error.to_string()).to_string();
                    let _ = this.update(cx, move |app, cx| app.set_import_error(message, cx));
                    return;
                }
            };
            let _ = this.update(cx, move |app, cx| app.import_sheets_loaded(loaded, cx));
        })
        .detach();
    }

    fn import_sheets_loaded(&mut self, names: Vec<String>, cx: &mut Context<'_, Self>) {
        if names.is_empty() {
            self.set_import_error(t!("import.no_sheets").to_string(), cx);
            return;
        }
        let theme = self.theme;
        let weak = cx.weak_entity();
        let (existing, target_table) = self
            .import_wizard
            .as_ref()
            .map(|wizard| (wizard.existing_tables.clone(), wizard.target_table.clone()))
            .unwrap_or_default();
        let mut sheets = Vec::with_capacity(names.len());
        let single_source = names.len() == 1;
        for name in names {
            let (target, create) = match find_existing(&existing, &name) {
                Some(table) => (table.to_string(), false),
                // A wizard opened from a table grid imports into that table when the file holds a
                // single source table; a multi-table source keeps its own names.
                None if single_source => match target_table.as_deref() {
                    Some(table) => (table.to_string(), false),
                    None => (name.clone(), true),
                },
                None => (name.clone(), true),
            };
            let input = make_import_target_input(theme, target.clone(), sheets.len(), &weak, cx);
            sheets.push(ImportSheetPlan {
                name,
                selected: true,
                target,
                create,
                target_input: Some(input),
            });
        }
        if let Some(wizard) = self.import_wizard.as_mut() {
            wizard.sheets = sheets;
            wizard.mapping_sheet = 0;
            wizard.sheet_combo = None;
            wizard.mapping_rows.clear();
            wizard.fields.clear();
        }
        cx.notify();
    }

    fn import_set_target(&mut self, index: usize, value: &str, cx: &mut Context<'_, Self>) {
        let existing = self
            .import_wizard
            .as_ref()
            .map(|wizard| wizard.existing_tables.clone())
            .unwrap_or_default();
        let mut invalidated = None;
        if let Some(wizard) = self.import_wizard.as_mut()
            && let Some(sheet) = wizard.sheets.get_mut(index)
        {
            if sheet.target == value {
                return;
            }
            sheet.target = value.to_string();
            sheet.create = find_existing(&existing, value).is_none();
            invalidated = Some(sheet.name.clone());
        }
        if let Some(name) = invalidated
            && let Some(wizard) = self.import_wizard.as_mut()
        {
            wizard.fields.remove(&name);
        }
        self.import_rebuild_mapping(cx);
        cx.notify();
    }

    fn import_toggle_sheet(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.import_wizard.as_mut()
            && let Some(sheet) = wizard.sheets.get_mut(index)
        {
            sheet.selected = !sheet.selected;
        }
        self.import_rebuild_mapping(cx);
        cx.notify();
    }

    fn import_toggle_create(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.import_wizard.as_mut()
            && let Some(sheet) = wizard.sheets.get_mut(index)
        {
            sheet.create = !sheet.create;
        }
        cx.notify();
    }

    fn import_set_all_sheets(&mut self, selected: bool, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.import_wizard.as_mut() {
            for sheet in wizard.sheets.iter_mut() {
                sheet.selected = selected;
            }
        }
        self.import_rebuild_mapping(cx);
        cx.notify();
    }

    // ----- P3: field mapping -------------------------------------------------------------------

    /// (Re)build the mapping page for the active sheet. It is dropped when the page is not showing,
    /// so changing the selection elsewhere never builds combos that are not displayed.
    fn import_rebuild_mapping(&mut self, cx: &mut Context<'_, Self>) {
        let theme = self.theme;
        let weak = cx.weak_entity();
        if self
            .import_wizard
            .as_ref()
            .is_none_or(|wizard| wizard.step != ImportStep::Mapping)
        {
            if let Some(wizard) = self.import_wizard.as_mut() {
                wizard.mapping_rows.clear();
                wizard.sheet_combo = None;
            }
            return;
        }
        let Some(wizard) = self.import_wizard.as_mut() else {
            return;
        };
        let selected: Vec<(usize, String)> = wizard
            .sheets
            .iter()
            .enumerate()
            .filter(|(_, sheet)| sheet.selected)
            .map(|(index, sheet)| (index, sheet.name.clone()))
            .collect();
        if selected.is_empty() {
            wizard.sheet_combo = None;
            wizard.mapping_rows.clear();
            return;
        }
        if !wizard
            .sheets
            .get(wizard.mapping_sheet)
            .is_some_and(|sheet| sheet.selected)
        {
            wizard.mapping_sheet = selected[0].0;
        }
        if selected.len() <= 1 {
            wizard.sheet_combo = None;
        } else {
            let current = wizard.sheets[wizard.mapping_sheet].name.clone();
            let options: Vec<ComboOption> = selected
                .iter()
                .map(|(_, name)| ComboOption::plain(name.clone()))
                .collect();
            let combo_weak = weak.clone();
            let combo = cx.new(move |cx| {
                ComboBox::new(theme, options, current, 240.0, cx)
                    .field_width(240.0)
                    .on_select(Rc::new(move |value: &str, _window, cx| {
                        let name = value.to_string();
                        let _ = combo_weak
                            .update(cx, |app, cx| app.import_set_mapping_sheet(&name, cx));
                    }))
            });
            wizard.sheet_combo = Some(combo);
        }

        let mut rows = BTreeMap::new();
        for (_, sheet_name) in &selected {
            let Some(fields) = wizard.fields.get(sheet_name) else {
                continue;
            };
            if fields.loading || fields.error.is_some() {
                continue;
            }
            let source = fields.source.clone();
            let target_columns = fields.target_columns.clone();
            let mapped = fields.mapped.clone();
            let mut sheet_rows = Vec::with_capacity(source.len());
            for (index, source_name) in source.into_iter().enumerate() {
                let selected_value = mapped.get(index).cloned().unwrap_or_default();
                let options = mapping_options(&target_columns);
                let row_weak = weak.clone();
                let sheet_for_row = sheet_name.clone();
                let combo = cx.new(move |cx| {
                    ComboBox::new(theme, options, selected_value, 200.0, cx)
                        .field_width(200.0)
                        .on_select(Rc::new(move |value: &str, _window, cx| {
                            let value = value.to_string();
                            let sheet = sheet_for_row.clone();
                            let _ = row_weak.update(cx, |app, cx| {
                                app.import_set_mapping(&sheet, index, &value, cx)
                            });
                        }))
                });
                sheet_rows.push(ImportMappingRow {
                    source: source_name,
                    combo,
                });
            }
            rows.insert(sheet_name.clone(), sheet_rows);
        }
        wizard.mapping_rows = rows;
    }

    fn import_set_mapping_sheet(&mut self, name: &str, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.import_wizard.as_mut()
            && let Some(index) = wizard.sheets.iter().position(|sheet| sheet.name == name)
        {
            wizard.mapping_sheet = index;
        }
        // The rows for every selected sheet are built up front, so switching sheets only changes
        // which set is drawn; no entities are built from inside a combo callback.
        cx.notify();
    }

    fn import_set_mapping(
        &mut self,
        sheet: &str,
        index: usize,
        value: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(wizard) = self.import_wizard.as_mut()
            && let Some(fields) = wizard.fields.get_mut(sheet)
            && let Some(mapped) = fields.mapped.get_mut(index)
        {
            *mapped = value.to_string();
        }
        cx.notify();
    }

    /// Read the sheet data (and, for an existing table, its columns) for every selected sheet that
    /// has not loaded yet.
    fn import_load_fields(&mut self, cx: &mut Context<'_, Self>) {
        let (connection_index, database, file, kind, pending, pending_names) = {
            let Some(wizard) = self.import_wizard.as_ref() else {
                return;
            };
            let kind = wizard.format.source_kind();
            let pending: Vec<(String, String, bool)> = wizard
                .sheets
                .iter()
                .filter(|sheet| sheet.selected)
                .filter(|sheet| {
                    wizard
                        .fields
                        .get(&sheet.name)
                        .is_none_or(|fields| !fields.loading && fields.error.is_some())
                })
                .map(|sheet| {
                    (
                        sheet.name.clone(),
                        sheet.target.trim().to_string(),
                        sheet.create,
                    )
                })
                .collect();
            let names = pending
                .iter()
                .map(|(sheet, _, _)| sheet.clone())
                .collect::<Vec<_>>();
            (
                wizard.connection_index,
                wizard.database.clone(),
                wizard.file.clone(),
                kind,
                pending,
                names,
            )
        };
        let Some(connection) = self.connection_arc(connection_index) else {
            self.set_import_error(t!("import.not_connected").to_string(), cx);
            return;
        };
        if pending.is_empty() {
            return;
        }
        for name in &pending_names {
            if let Some(wizard) = self.import_wizard.as_mut() {
                wizard
                    .fields
                    .insert(name.clone(), ImportSheetFields::loading());
            }
        }
        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            for (sheet, target, create) in pending {
                let read_path = file.clone();
                let read_sheet = sheet.clone();
                let read = runtime
                    .spawn_blocking(move || kind.read(Path::new(&read_path), &read_sheet))
                    .await;
                let data = match read {
                    Ok(Ok(data)) => data,
                    Ok(Err(error)) => {
                        let message = error.to_string();
                        let _ = this.update(cx, move |app, cx| {
                            app.import_field_error(sheet.clone(), message, cx)
                        });
                        continue;
                    }
                    Err(error) => {
                        let message = error.to_string();
                        let _ = this.update(cx, move |app, cx| {
                            app.import_field_error(sheet.clone(), message, cx)
                        });
                        continue;
                    }
                };
                let columns = if create {
                    Ok(None)
                } else {
                    let connection = connection.clone();
                    let database = database.clone();
                    let table = target.clone();
                    match runtime
                        .spawn(async move { connection.columns(&database, &table).await })
                        .await
                    {
                        Ok(inner) => inner.map(Some).map_err(|error| error.to_string()),
                        Err(error) => Err(error.to_string()),
                    }
                };
                let _ = this.update(cx, move |app, cx| {
                    app.import_fields_loaded(sheet, data, columns, cx)
                });
            }
        })
        .detach();
    }

    fn import_field_error(&mut self, sheet: String, error: String, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.import_wizard.as_mut() {
            wizard.fields.insert(
                sheet,
                ImportSheetFields {
                    loading: false,
                    error: Some(error),
                    ..ImportSheetFields::loading()
                },
            );
        }
        self.import_rebuild_mapping(cx);
        cx.notify();
    }

    fn import_fields_loaded(
        &mut self,
        sheet: String,
        data: SheetData,
        columns: Result<Option<Vec<rustgrid_core::ColumnInfo>>, String>,
        cx: &mut Context<'_, Self>,
    ) {
        let fields = match columns {
            Ok(Some(columns)) => {
                let primary_key = columns
                    .iter()
                    .filter(|column| column.primary_key)
                    .map(|column| column.name.clone())
                    .collect();
                let target_columns: Vec<String> =
                    columns.iter().map(|column| column.name.clone()).collect();
                let target_types: Vec<String> = columns
                    .iter()
                    .map(|column| column.data_type.clone())
                    .collect();
                let mapped = auto_match(&data.headers, &target_columns);
                ImportSheetFields {
                    source: data.headers,
                    rows: data.rows,
                    target_columns,
                    target_types,
                    mapped,
                    primary_key,
                    loading: false,
                    error: None,
                }
            }
            Ok(None) => {
                let inferred = infer_columns(&data.headers, &data.rows);
                let primary_key = primary_key_from(&data.headers);
                let source = data.headers.clone();
                let mapped = data.headers.clone();
                let target_types: Vec<String> = inferred
                    .into_iter()
                    .map(|column| column.data_type)
                    .collect();
                ImportSheetFields {
                    source,
                    rows: data.rows,
                    target_columns: data.headers,
                    target_types,
                    mapped,
                    primary_key,
                    loading: false,
                    error: None,
                }
            }
            Err(error) => ImportSheetFields {
                loading: false,
                error: Some(error),
                ..ImportSheetFields::loading()
            },
        };
        if let Some(wizard) = self.import_wizard.as_mut() {
            wizard.fields.insert(sheet, fields);
        }
        self.import_rebuild_mapping(cx);
        cx.notify();
    }

    // ----- P4: run -----------------------------------------------------------------------------

    pub(super) fn import_start(&mut self, cx: &mut Context<'_, Self>) {
        if self
            .import_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.running)
        {
            return;
        }
        if let Some(error) = self.import_validate_for_run() {
            self.set_import_error(error, cx);
            return;
        }
        let (connection_index, database, file, plans, total_rows) = {
            let Some(wizard) = self.import_wizard.as_ref() else {
                return;
            };
            let plans: Vec<ImportRunSheet> = wizard
                .sheets
                .iter()
                .filter(|sheet| sheet.selected)
                .filter_map(|sheet| {
                    let fields = wizard.fields.get(&sheet.name)?;
                    Some(ImportRunSheet {
                        sheet: sheet.name.clone(),
                        target: sheet.target.trim().to_string(),
                        create: sheet.create,
                        source: fields.source.clone(),
                        rows: fields.rows.clone(),
                        columns: fields.mapping_columns(),
                        primary_key: fields.primary_key.clone(),
                    })
                })
                .collect();
            let total_rows: usize = plans.iter().map(|plan| plan.rows.len()).sum();
            (
                wizard.connection_index,
                wizard.database.clone(),
                wizard.file.clone(),
                plans,
                total_rows,
            )
        };
        let Some(connection) = self.connection_arc(connection_index) else {
            self.set_import_error(t!("import.not_connected").to_string(), cx);
            return;
        };
        if plans.is_empty() {
            self.set_import_error(t!("import.no_sheets").to_string(), cx);
            return;
        }

        if let Some(wizard) = self.import_wizard.as_mut() {
            wizard.running = true;
            wizard.error = None;
            wizard.log.clear();
            wizard.rows_total = total_rows;
            wizard.rows_done = 0;
            wizard.imported_tables = 0;
            wizard.added = 0;
            wizard.updated = 0;
            wizard.deleted = 0;
            wizard.errors = 0;
            wizard.elapsed = None;
            wizard.started = Some(std::time::Instant::now());
            wizard
                .log
                .push(t!("import.log.started", time = now_timestamp()).to_string());
            wizard
                .log
                .push(t!("import.log.start", count = plans.len(), file = file).to_string());
            wizard.log_scroll.scroll_to_bottom();
        }
        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let mut imported_tables = 0usize;
            let mut added = 0usize;
            let mut errors = 0usize;

            for plan in plans {
                if plan.columns.is_empty() {
                    continue;
                }
                if plan.create {
                    let schema = TableSchema {
                        columns: create_columns(&plan),
                        ..Default::default()
                    };
                    let connection = connection.clone();
                    let database = database.clone();
                    let target = plan.target.clone();
                    let result = runtime
                        .spawn(async move {
                            connection
                                .save_table_schema(&database, &target, None, &schema)
                                .await
                        })
                        .await;
                    let error = match result {
                        Ok(Ok(())) => None,
                        Ok(Err(error)) => Some(error.to_string()),
                        Err(error) => Some(error.to_string()),
                    };
                    if let Some(error) = error {
                        errors += 1;
                        let message = t!(
                            "import.log.sheet_failed",
                            name = plan.sheet.clone(),
                            error = error
                        )
                        .to_string();
                        let _ = this.update(cx, move |app, cx| app.import_log(message, cx));
                        continue;
                    }
                }

                let mut sheet_rows = 0usize;
                let mut failed = false;
                for chunk in plan.rows.chunks(IMPORT_BATCH_ROWS) {
                    let inserts: Vec<RowInsert> =
                        chunk.iter().map(|row| plan.insert_for(row)).collect();
                    if inserts.is_empty() {
                        continue;
                    }
                    let count = inserts.len();
                    let connection = connection.clone();
                    let database = database.clone();
                    let target = plan.target.clone();
                    let result =
                        runtime
                            .spawn(async move {
                                connection.insert_rows(&database, &target, &inserts).await
                            })
                            .await;
                    let error = match result {
                        Ok(Ok(())) => None,
                        Ok(Err(error)) => Some(error.to_string()),
                        Err(error) => Some(error.to_string()),
                    };
                    match error {
                        None => {
                            sheet_rows += count;
                            added += count;
                            let _ = this.update(cx, move |app, cx| {
                                if let Some(wizard) = app.import_wizard.as_mut() {
                                    wizard.rows_done += count;
                                    wizard.added = added;
                                    wizard.log_scroll.scroll_to_bottom();
                                }
                                cx.notify();
                            });
                        }
                        Some(error) => {
                            errors += 1;
                            failed = true;
                            let message = t!(
                                "import.log.sheet_failed",
                                name = plan.sheet.clone(),
                                error = error
                            )
                            .to_string();
                            let _ = this.update(cx, move |app, cx| app.import_log(message, cx));
                            break;
                        }
                    }
                }
                if !failed {
                    imported_tables += 1;
                }
                let message = t!(
                    "import.log.sheet",
                    name = plan.sheet.clone(),
                    rows = sheet_rows
                )
                .to_string();
                let _ = this.update(cx, move |app, cx| app.import_log(message, cx));
            }

            let _ = this.update(cx, move |app, cx| {
                app.import_finish(imported_tables, errors, cx)
            });
        })
        .detach();
    }

    fn import_log(&mut self, message: String, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.import_wizard.as_mut() {
            wizard.log.push(message);
            wizard.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn import_finish(&mut self, imported_tables: usize, errors: usize, cx: &mut Context<'_, Self>) {
        if let Some(wizard) = self.import_wizard.as_mut() {
            wizard.running = false;
            wizard.imported_tables = imported_tables;
            wizard.errors = errors;
            if let Some(started) = wizard.started.take() {
                let elapsed = started.elapsed();
                wizard.elapsed = Some(elapsed);
                wizard
                    .log
                    .push(t!("import.log.elapsed", time = format_hms(elapsed)).to_string());
            }
            wizard.rows_total = wizard.rows_total.max(wizard.rows_done);
            wizard
                .log
                .push(t!("import.log.done", count = imported_tables, errors = errors).to_string());
            wizard.log_scroll.scroll_to_bottom();
        }
        // Re-fetch the database's tables so a newly created table shows up in the object tree.
        let target = self
            .import_wizard
            .as_ref()
            .map(|wizard| (wizard.connection_index, wizard.database.clone()));
        if let Some((connection_index, database)) = target
            && let Some(connection) = self.connection_arc(connection_index)
        {
            self.reload_database_tables(&connection, &database, cx);
        }
        cx.notify();
    }

    // ----- Window contents ---------------------------------------------------------------------

    /// The contents of the Import Wizard window: titlebar, page heading, page body and footer.
    pub(super) fn import_window_contents(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(wizard) = self.import_wizard.as_ref() else {
            return div().into_any_element();
        };
        let (page, total) = wizard.step.index();
        let heading_key = match wizard.step {
            ImportStep::Format => "import.step.format",
            ImportStep::Source => "import.step.source",
            ImportStep::Mapping => "import.step.mapping",
            ImportStep::Run => "import.step.run",
        };
        let heading = div()
            .px_3()
            .py_3()
            .flex_none()
            .text_size(px(13.0))
            .text_color(rgb(theme.grid_selection_bg))
            .child(format!("{} ({page}/{total})", t!(heading_key)));
        let body = match wizard.step {
            ImportStep::Format => self.render_import_format_page(cx),
            ImportStep::Source => self.render_import_source_page(cx),
            ImportStep::Mapping => self.render_import_mapping_page(cx),
            ImportStep::Run => self.render_import_run_page(cx),
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
            .track_focus(&self.import_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.import_close(window, cx);
                }
            }))
            .child(child_window_titlebar(t!("import.title").to_string(), theme))
            .child(heading)
            .child(content)
            .child(self.render_import_footer(cx).into_any_element())
            .into_any_element()
    }

    fn render_import_format_page(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(wizard) = self.import_wizard.as_ref() else {
            return div().into_any_element();
        };
        let mut list = div().flex().flex_col().gap_1().px_3().child(
            div()
                .text_size(px(12.0))
                .child(t!("import.type_label").to_string()),
        );
        for format in ImportFormat::ALL {
            let selected = wizard.format == format;
            let row = div()
                .id(SharedString::from(format!(
                    "import-format-{}",
                    format.label_key()
                )))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(22.0))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.import_set_format(format, cx);
                }))
                .child(ui::radio_box(selected, theme))
                .child(
                    div()
                        .text_size(px(12.0))
                        .child(t!(format.label_key()).to_string()),
                );
            list = list.child(row);
        }
        list.into_any_element()
    }

    fn render_import_source_page(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(wizard) = self.import_wizard.as_ref() else {
            return div().into_any_element();
        };

        let file_input = wizard
            .file_input
            .clone()
            .map(|input| {
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(24.0))
                    .child(input)
                    .into_any_element()
            })
            .unwrap_or_else(|| div().flex_1().into_any_element());
        let file_row = div()
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
                    .child(t!("import.import_from").to_string()),
            )
            .child(file_input)
            .child(self.dialog_button(
                "import-browse-file",
                t!("import.browse").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.import_browse_file(cx)),
            ));

        let mut list = div()
            .id("import-sheet-list")
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
                            .w(px(214.0))
                            .flex_none()
                            .child(t!("import.source_table").to_string()),
                    )
                    .child(
                        div()
                            .w(px(220.0))
                            .flex_none()
                            .child(t!("import.target_table").to_string()),
                    )
                    .child(div().flex_1().child(t!("import.create_table").to_string())),
            );
        for (index, sheet) in wizard.sheets.iter().enumerate() {
            let selected = sheet.selected;
            let create = sheet.create;
            let target_input = sheet.target_input.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("import-sheet-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .h(px(28.0))
                    .when(selected, move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .child(
                        div()
                            .id(SharedString::from(format!("import-sheet-check-{index}")))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.import_toggle_sheet(index, cx);
                            }))
                            .child(checkbox_box(selected, theme)),
                    )
                    .child(tree_icon("icons/tables.svg", theme.icon_table))
                    .child(
                        div()
                            .w(px(180.0))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.0))
                            .child(sheet.name.clone()),
                    )
                    .child(
                        target_input
                            .map(|input| {
                                div()
                                    .w(px(220.0))
                                    .flex_none()
                                    .h(px(24.0))
                                    .child(input)
                                    .into_any_element()
                            })
                            .unwrap_or_else(|| div().into_any_element()),
                    )
                    .child(div().flex_1().child(export_check_row(
                        format!("import-sheet-create-{index}"),
                        t!("import.create_table").to_string(),
                        create,
                        theme,
                        cx.listener(move |this, _event, _window, cx| {
                            this.import_toggle_create(index, cx);
                        }),
                    ))),
            );
        }

        let buttons = div()
            .flex()
            .flex_row()
            .gap_2()
            .px_3()
            .py_2()
            .child(self.dialog_button(
                "import-select-all",
                t!("import.select_all").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.import_set_all_sheets(true, cx)),
            ))
            .child(self.dialog_button(
                "import-select-none",
                t!("import.deselect_all").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.import_set_all_sheets(false, cx)),
            ));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(file_row)
            .child(list)
            .child(buttons)
            .into_any_element()
    }

    fn render_import_mapping_page(&self, _cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(wizard) = self.import_wizard.as_ref() else {
            return div().into_any_element();
        };
        let active = wizard.sheets.get(wizard.mapping_sheet);
        let sheet_name = active.map(|sheet| sheet.name.clone()).unwrap_or_default();
        let target_name = active.map(|sheet| sheet.target.clone()).unwrap_or_default();
        let selected_count = wizard.sheets.iter().filter(|sheet| sheet.selected).count();

        let source = if selected_count > 1 {
            wizard
                .sheet_combo
                .clone()
                .map(|combo| div().w(px(240.0)).child(combo).into_any_element())
                .unwrap_or_else(|| div().into_any_element())
        } else {
            div()
                .flex()
                .items_center()
                .h(px(24.0))
                .text_size(px(12.0))
                .child(sheet_name.clone())
                .into_any_element()
        };

        let source_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .pb_1()
            .child(
                div()
                    .w(px(80.0))
                    .flex_none()
                    .text_size(px(12.0))
                    .child(t!("import.source_table").to_string()),
            )
            .child(source);
        let target_row = div()
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
                    .child(t!("import.target_table").to_string()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(24.0))
                    .text_size(px(12.0))
                    .child(target_name),
            );

        let fields = wizard.fields.get(&sheet_name);
        let rows = wizard.mapping_rows.get(&sheet_name);
        let primary_key = fields
            .map(|fields| fields.primary_key.clone())
            .unwrap_or_default();
        let mapped = fields
            .map(|fields| fields.mapped.clone())
            .unwrap_or_default();

        let mut list = div()
            .id("import-mapping-list")
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
                            .child(t!("import.source_field").to_string()),
                    )
                    .child(
                        div()
                            .w(px(200.0))
                            .flex_none()
                            .child(t!("import.target_field").to_string()),
                    )
                    .child(div().flex_1().child(t!("import.primary_key").to_string())),
            );
        match fields {
            None => {}
            Some(fields) if fields.loading => {
                list = list.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("common.loading").to_string()),
                );
            }
            Some(fields) if fields.error.is_some() => {
                list = list.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.danger))
                        .child(fields.error.clone().unwrap_or_default()),
                );
            }
            Some(_) => {
                for (index, row) in rows.into_iter().flatten().enumerate() {
                    let is_key = mapped
                        .get(index)
                        .is_some_and(|target| primary_key.iter().any(|key| key == target));
                    let key = if is_key {
                        svg()
                            .path("icons/primary_key.svg")
                            .w(px(14.0))
                            .h(px(14.0))
                            .flex_none()
                            .text_color(rgb(theme.icon_table))
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    };
                    list = list.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .h(px(24.0))
                            .child(
                                div()
                                    .w(px(220.0))
                                    .flex_none()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(12.0))
                                    .child(row.source.clone()),
                            )
                            .child(div().w(px(200.0)).flex_none().child(row.combo.clone()))
                            .child(key),
                    );
                }
            }
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .child(source_row)
            .child(target_row)
            .child(list)
            .into_any_element()
    }

    fn render_import_run_page(&self, _cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(wizard) = self.import_wizard.as_ref() else {
            return div().into_any_element();
        };
        let source = wizard
            .sheets
            .iter()
            .filter(|sheet| sheet.selected)
            .map(|sheet| sheet.name.clone())
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
                t!("import.source").to_string(),
                source,
                theme,
            ))
            .child(export_summary_row(
                t!("import.tables").to_string(),
                wizard.imported_tables.to_string(),
                theme,
            ))
            .child(export_summary_row(
                t!("import.processed").to_string(),
                wizard.rows_done.to_string(),
                theme,
            ))
            .child(export_summary_row(
                t!("import.errors").to_string(),
                wizard.errors.to_string(),
                theme,
            ))
            .child(export_summary_row(
                t!("import.added").to_string(),
                wizard.added.to_string(),
                theme,
            ))
            .child(export_summary_row(
                t!("import.updated").to_string(),
                wizard.updated.to_string(),
                theme,
            ))
            .child(export_summary_row(
                t!("import.deleted").to_string(),
                wizard.deleted.to_string(),
                theme,
            ))
            .child(export_summary_row(
                t!("import.time").to_string(),
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
            .id("import-log")
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
            .child(summary)
            .child(log)
            .child(bar)
            .into_any_element()
    }

    fn render_import_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let step = self
            .import_wizard
            .as_ref()
            .map(|wizard| wizard.step)
            .unwrap_or(ImportStep::Format);
        let running = self
            .import_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.running);
        // Once a run has finished the primary button becomes Close, so the footer is not left
        // offering Start again on completed data.
        let finished = !running
            && self
                .import_wizard
                .as_ref()
                .is_some_and(|wizard| wizard.elapsed.is_some());
        let mut right = div().flex().flex_row().items_center().gap_2().flex_none();
        if !finished {
            right = right.child(self.wizard_footer_button(
                "import-cancel",
                t!("import.cancel").to_string(),
                false,
                !running,
                cx.listener(|this, _event, window, cx| this.import_close(window, cx)),
            ));
        }
        if step != ImportStep::Format {
            right = right.child(self.wizard_footer_button(
                "import-back",
                t!("import.prev").to_string(),
                false,
                !running,
                cx.listener(|this, _event, _window, cx| this.import_back(cx)),
            ));
        }
        if step == ImportStep::Run {
            if finished {
                right = right.child(self.wizard_footer_button(
                    "import-close-done",
                    t!("import.close").to_string(),
                    true,
                    true,
                    cx.listener(|this, _event, window, cx| this.import_close(window, cx)),
                ));
            } else {
                right = right.child(self.wizard_footer_button(
                    "import-start",
                    t!("import.start").to_string(),
                    true,
                    !running,
                    cx.listener(|this, _event, _window, cx| this.import_start(cx)),
                ));
            }
        } else {
            right = right.child(self.wizard_footer_button(
                "import-next",
                t!("import.next").to_string(),
                true,
                !running,
                cx.listener(|this, _event, _window, cx| this.import_next(cx)),
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
}

/// The root view of the Import Wizard OS window. It re-renders whenever `AppView` changes so the
/// window stays in sync while pages change and the import runs.
pub(super) struct ImportWindow {
    app: WeakEntity<AppView>,
    _subscription: Subscription,
}

impl ImportWindow {
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

impl Render for ImportWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        app.update(cx, |app, cx| app.import_window_contents(cx))
    }
}

// ----- Free helpers ---------------------------------------------------------------------------

/// One selected sheet's snapshot taken when the import starts, so no `AppView` borrow is held
/// across the awaits.
struct ImportRunSheet {
    sheet: String,
    target: String,
    create: bool,
    source: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    /// `(source column index, destination column, destination type)`.
    columns: Vec<(usize, String, String)>,
    primary_key: Vec<String>,
}

impl ImportRunSheet {
    /// Build the `INSERT` row for one data row: every mapped source column, with an empty cell
    /// becoming an explicit SQL `NULL` and date/time values normalised for the destination column.
    fn insert_for(&self, row: &[Option<String>]) -> RowInsert {
        let values = self
            .columns
            .iter()
            .filter_map(|(index, target, data_type)| {
                row.get(*index).cloned().map(|value| {
                    let value = value.map(|value| normalize_value(&value, data_type));
                    (target.clone(), value)
                })
            })
            .collect();
        RowInsert { values }
    }
}

/// Build the `CREATE TABLE` columns for a sheet the wizard creates: inferred from the data, keeping
/// only the columns that are actually imported, with the inferred primary key flagged.
fn create_columns(plan: &ImportRunSheet) -> Vec<ColumnDef> {
    let mut columns = infer_columns(&plan.source, &plan.rows);
    columns.retain(|column| {
        plan.columns
            .iter()
            .any(|(_, target, _)| target == &column.name)
    });
    for column in &mut columns {
        if plan.primary_key.iter().any(|key| key == &column.name) {
            column.primary_key = true;
            // MySQL requires every PRIMARY KEY part to be NOT NULL (error 1171).
            column.nullable = false;
        }
    }
    columns
}

/// Normalise a text value for a destination column so MySQL accepts it: date/time columns get the
/// canonical `YYYY-MM-DD[ HH:MM:SS]` form (text sources use many conventions, e.g. `26/6/2025`),
/// everything else is left untouched. An unparseable value is passed through so the server reports
/// it rather than the wizard silently dropping data.
fn normalize_value(value: &str, data_type: &str) -> String {
    let base = data_type
        .split('(')
        .next()
        .unwrap_or(data_type)
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "date" => parse_date_value(value)
            .map(|parsed| parsed.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| value.to_string()),
        "datetime" | "timestamp" => parse_date_value(value)
            .map(|parsed| parsed.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| value.to_string()),
        "time" => parse_date_value(value)
            .map(|parsed| parsed.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| value.to_string()),
        _ => value.to_string(),
    }
}

/// Parse a date/time string with `dtparse` (a port of Python's `dateutil`), day-first so
/// `26/6/2025` reads as 26 June. Timezones are ignored; an unrecognised string yields `None`.
fn parse_date_value(value: &str) -> Option<NaiveDateTime> {
    Parser::default()
        .parse(
            value.trim(),
            Some(true),
            None,
            false,
            false,
            None,
            true,
            &HashMap::new(),
        )
        .ok()
        .map(|(naive, _offset, _tokens)| naive)
}

/// Build the P2 destination-table field. Edits rewrite the plan's target and re-evaluate whether the
/// table must be created.
fn make_import_target_input(
    theme: Theme,
    initial: String,
    index: usize,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    cx.new(move |cx| {
        TextInput::new(theme, initial, TextInputOptions::default(), cx).on_change(Rc::new(
            move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| app.import_set_target(index, text, cx));
            },
        ))
    })
}

/// Build the P2 source-file field. Edits write straight into the wizard; submitting the field reads
/// the sheets.
fn make_import_file_input(
    theme: Theme,
    initial: String,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    let submit = app.clone();
    cx.new(move |cx| {
        TextInput::new(theme, initial, TextInputOptions::default(), cx)
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| {
                    if let Some(wizard) = app.import_wizard.as_mut() {
                        wizard.file = text.to_string();
                    }
                    cx.notify();
                });
            }))
            .on_submit(Rc::new(move |_window, cx| {
                let _ = submit.update(cx, |app, cx| app.import_load_file(cx));
            }))
    })
}

/// Normalise a name for matching: lowercase, alphanumerics only. So `Order_ID` matches `order id`.
fn normalize_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

/// The existing table whose normalised name matches `name`, if any. A schema-qualified
/// existing name (`dbo.users`) is also matched by its bare object name.
fn find_existing<'a>(existing: &'a [String], name: &str) -> Option<&'a str> {
    let key = normalize_key(name);
    existing.iter().map(String::as_str).find(|table| {
        normalize_key(table) == key
            || table
                .rsplit('.')
                .next()
                .is_some_and(|bare| normalize_key(bare) == key)
    })
}

/// The new table's primary key: the source column named `id`, if there is one.
fn primary_key_from(headers: &[String]) -> Vec<String> {
    headers
        .iter()
        .filter(|header| normalize_key(header) == "id")
        .cloned()
        .collect()
}

/// Match source columns to destination columns by normalised name, each destination column used at
/// most once. Unmatched source columns map to an empty string (not imported).
fn auto_match(source: &[String], target_columns: &[String]) -> Vec<String> {
    let mut used = vec![false; target_columns.len()];
    let mut mapped = Vec::with_capacity(source.len());
    for name in source {
        let key = normalize_key(name);
        let found = target_columns
            .iter()
            .enumerate()
            .find(|(index, column)| !used[*index] && normalize_key(column) == key)
            .map(|(index, _)| index);
        match found {
            Some(index) => {
                used[index] = true;
                mapped.push(target_columns[index].clone());
            }
            None => mapped.push(String::new()),
        }
    }
    mapped
}

/// The dropdown options for one mapping row: a "do not import" entry plus every destination column.
fn mapping_options(target_columns: &[String]) -> Vec<ComboOption> {
    let mut options = Vec::with_capacity(target_columns.len() + 1);
    options.push(ComboOption::new("", t!("import.do_not_import").to_string()));
    options.extend(
        target_columns
            .iter()
            .map(|column| ComboOption::plain(column.clone())),
    );
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_key_strips_separators_and_case() {
        assert_eq!(normalize_key("Order_ID"), "orderid");
        assert_eq!(normalize_key("order id"), "orderid");
        assert_eq!(normalize_key("Order-ID"), "orderid");
    }

    #[test]
    fn auto_match_pairs_by_normalised_name() {
        let source = vec![
            "ID".to_string(),
            "Create Time".to_string(),
            "extra".to_string(),
        ];
        let target = vec!["id".to_string(), "create_time".to_string()];
        assert_eq!(auto_match(&source, &target), vec!["id", "create_time", ""]);
    }

    #[test]
    fn auto_match_never_reuses_a_destination_column() {
        let source = vec!["a".to_string(), "a".to_string()];
        let target = vec!["a".to_string()];
        assert_eq!(auto_match(&source, &target), vec!["a", ""]);
    }

    #[test]
    fn primary_key_is_the_id_column() {
        let headers = vec!["ID".to_string(), "name".to_string()];
        assert_eq!(primary_key_from(&headers), vec!["ID"]);
        assert!(primary_key_from(&["name".to_string()]).is_empty());
    }

    #[test]
    fn created_primary_key_is_not_null() {
        let plan = ImportRunSheet {
            sheet: "s".to_string(),
            target: "t".to_string(),
            create: true,
            source: vec!["id".to_string(), "name".to_string()],
            rows: vec![vec![Some("1".to_string()), Some("a".to_string())]],
            columns: vec![
                (0, "id".to_string(), "bigint".to_string()),
                (1, "name".to_string(), "varchar".to_string()),
            ],
            primary_key: vec!["id".to_string()],
        };
        let columns = create_columns(&plan);
        let id = columns.iter().find(|column| column.name == "id").unwrap();
        assert!(id.primary_key);
        assert!(!id.nullable);
        assert_eq!(id.data_type, "bigint");
        let name = columns.iter().find(|column| column.name == "name").unwrap();
        assert!(!name.primary_key);
        assert!(name.nullable);
    }

    #[test]
    fn normalizes_common_date_formats_to_mysql() {
        // Day-first numeric (the user's case) and other common shapes.
        for value in [
            "26/6/2025 14:02:23",
            "26-06-2025 14:02:23",
            "2025-06-26 14:02:23",
            "2025/6/26 14:02:23",
            "2025-06-26T14:02:23Z",
            "26 Jun 2025 14:02:23",
            "Jun 26, 2025 14:02:23",
        ] {
            assert_eq!(
                normalize_value(value, "datetime"),
                "2025-06-26 14:02:23",
                "for {value:?}"
            );
        }
        // Ambiguous numeric dates read day-first locally.
        assert_eq!(normalize_value("01/02/2025", "date"), "2025-02-01");
        assert_eq!(normalize_value("26/6/2025", "date"), "2025-06-26");
        // A datetime value stored in a DATE column drops the time.
        assert_eq!(normalize_value("2025-06-26 14:02:23", "date"), "2025-06-26");
        assert_eq!(normalize_value("14:02:23", "time"), "14:02:23");
        // Non date/time columns and unparseable values are untouched.
        assert_eq!(normalize_value("26/6/2025", "varchar"), "26/6/2025");
        assert_eq!(normalize_value("hello", "datetime"), "hello");
        // A `datetime(6)` precision suffix is understood.
        assert_eq!(
            normalize_value("26/6/2025 14:02:23", "datetime(6)"),
            "2025-06-26 14:02:23"
        );
    }
}
