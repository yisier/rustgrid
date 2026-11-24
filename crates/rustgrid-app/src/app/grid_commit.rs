use super::grid::row_origin;
use super::*;
use crate::session::display_table_path;
use rustgrid_core::{RowChange, RowChangeKind, render_change_sql};

impl GridView {
    pub(super) fn date_picker_today(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let now = chrono::Local::now().naive_local();
        let Some(picker) = self.date_picker.as_ref() else {
            return;
        };
        let calendar = picker.calendar.clone();
        let time_field = picker.time_field.clone();
        if let Some(today) = NaiveDate::from_ymd_opt(now.year(), now.month(), now.day()) {
            calendar.update(cx, |calendar, cx| {
                calendar.set_date(picker_date(today), window, cx)
            });
        }
        if let Some(field) = time_field
            && let Some(time) = NaiveTime::from_hms_opt(now.hour(), now.minute(), now.second())
        {
            field.update(cx, |field, cx| {
                field.set_time(picker_time(time), window, cx)
            });
        }
        cx.notify();
    }

    /// Close the picker after accepting its value. The value is written into the in-place editor
    /// and focus returns there, so the date/time cell behaves like the other cell editors: the
    /// change is committed only when the editor loses focus, Enter is pressed, or the bottom
    /// commit button is clicked — never merely by confirming the picker.
    pub(super) fn date_picker_ok(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.date_picker.is_none() {
            return;
        }
        self.sync_date_picker_to_editor(cx);
        self.date_picker = None;
        if let Some(editor) = self.cell_editor.as_ref() {
            let focus = editor.input.read(cx).focus_handle();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    pub(super) fn date_picker_cancel(&mut self, cx: &mut Context<'_, Self>) {
        self.date_picker = None;
        self.cancel_editor(cx);
    }

    /// Save: stage the in-place editor (if any), then write every pending insert and edit.
    pub(super) fn save_grid(&mut self, cx: &mut Context<'_, Self>) {
        // A save already in flight still owns the staged edits and inserts: the page has not
        // reloaded yet, so writing them again would repeat its inserts (a double-click on Save
        // inserted the same new record twice).
        if self.saving {
            return;
        }
        if self.cell_editor.is_some() {
            self.commit_editor(cx);
        }
        let inserts = self.collect_inserts();
        let updates = self.collect_updates();
        self.flush_changes(inserts, updates, true, cx);
    }

    /// Auto-commit the pending edits to existing rows (single-cell edits), without inserts.
    /// This is the save that happens when a cell editor loses focus, so it confirms itself with
    /// the same "Changes saved" toast as the explicit Save / Ctrl+S path.
    pub(super) fn commit_edits(&mut self, cx: &mut Context<'_, Self>) {
        let updates = self.collect_updates();
        self.flush_changes(Vec::new(), updates, true, cx);
    }

    /// Build the pending insert rows as engine-agnostic [`RowInsert`]s.
    fn collect_inserts(&self) -> Vec<RowInsert> {
        self.inserts
            .iter()
            .map(|row| RowInsert {
                values: row
                    .values
                    .iter()
                    .map(|(&col, value)| (self.state.columns[col].name.clone(), value.clone()))
                    .collect(),
            })
            .collect()
    }

    /// The display columns that identify a row for an `UPDATE`/`DELETE`: the primary key, or
    /// every column when the table has none.
    fn key_columns(&self) -> Vec<usize> {
        let primary_keys: Vec<usize> = self
            .state
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.primary_key)
            .map(|(index, _)| index)
            .collect();
        if primary_keys.is_empty() {
            (0..self.state.columns.len()).collect()
        } else {
            primary_keys
        }
    }

    /// Build one page row's pending edits as an engine-agnostic [`RowUpdate`].
    fn update_for_row(&self, row: usize) -> Option<RowUpdate> {
        let row_values = self.state.rows.get(row)?;
        let key_columns = self.key_columns();
        let set: Vec<(String, Option<String>)> = self
            .state
            .edits
            .iter()
            .filter(|((edited_row, _), _)| *edited_row == row)
            .filter_map(|(&(_, col), value)| {
                self.state
                    .columns
                    .get(col)
                    .map(|column| (column.name.clone(), value.clone()))
            })
            .collect();
        if set.is_empty() {
            return None;
        }
        let keys = key_columns
            .iter()
            .filter_map(|&col| {
                Some((
                    self.state.columns.get(col)?.name.clone(),
                    row_values
                        .get(self.state.source_col(col))
                        .and_then(|value| value.as_edit_value()),
                ))
            })
            .collect();
        Some(RowUpdate { set, keys })
    }

    /// The `UPDATE` the pending edits will run, for the grid's status line. The active record is
    /// previewed; when it has no edits of its own, the first edited row stands in so a bulk change
    /// still shows its statement instead of the page's `SELECT`.
    pub(super) fn pending_update_sql(&self) -> Option<String> {
        if self.state.edits.is_empty() || self.state.table.is_empty() {
            return None;
        }
        let active = self
            .state
            .selection
            .as_ref()
            .and_then(|selection| self.row_at(selection.active_cursor().0));
        let row = match active {
            Some(GridRow::Data(row)) if self.state.edits.keys().any(|&(r, _)| r == row) => row,
            _ => self
                .state
                .edits
                .keys()
                .map(|(row, _)| row)
                .next()
                .copied()?,
        };
        let update = self.update_for_row(row)?;
        let table = display_table_path(self.state.dialect, &self.state.database, &self.state.table);
        let change = RowChange {
            kind: RowChangeKind::Update,
            keys: update.keys,
            values: update.set,
            changes: Vec::new(),
        };
        Some(render_change_sql(&table, &change, self.state.dialect))
    }

    /// Build the pending existing-row edits as engine-agnostic [`RowUpdate`]s.
    fn collect_updates(&self) -> Vec<RowUpdate> {
        let rows: BTreeSet<usize> = self.state.edits.keys().map(|&(row, _)| row).collect();
        rows.into_iter()
            .filter_map(|row| self.update_for_row(row))
            .collect()
    }

    /// Write the pending inserts and updates in one transaction-ish pass, then reload the page.
    /// `notify_saved` is set by both the explicit Save path (button / Ctrl+S) and the auto-commit
    /// of a single cell edit, so every write that reaches the database confirms itself.
    fn flush_changes(
        &mut self,
        inserts: Vec<RowInsert>,
        updates: Vec<RowUpdate>,
        notify_saved: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if self.saving {
            return;
        }
        if inserts.is_empty() && updates.is_empty() {
            if notify_saved {
                self.show_toast(ToastKind::Info, t!("grid.no_changes").to_string(), cx);
            }
            return;
        }

        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let runtime = self.runtime.clone();
        self.saving = true;

        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    if !inserts.is_empty() {
                        connection.insert_rows(&database, &table, &inserts).await?;
                    }
                    if !updates.is_empty() {
                        connection.update_rows(&database, &table, &updates).await?;
                    }
                    Ok::<(), Error>(())
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |grid, cx| {
                grid.saving = false;
                match result {
                    Ok(()) => {
                        if notify_saved {
                            grid.show_toast(ToastKind::Success, t!("grid.saved").to_string(), cx);
                        }
                        grid.reload_after_write(cx);
                    }
                    Err(error) => {
                        grid.open_row_after_save = None;
                        let message = match error {
                            Error::Query(text) => text,
                            other => other.to_string(),
                        };
                        grid.state.error = Some(message.clone());
                        grid.show_error(message, cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn cancel_edits(&mut self, cx: &mut Context<'_, Self>) {
        self.state.edits.clear();
        self.inserts.clear();
        self.state.undo.clear();
        self.state.redo.clear();
        self.state.selection = None;
        self.cell_editor = None;
        self.cell_editor_blur_subscription = None;
        self.date_picker = None;
        self.enum_menu = None;
        self.open_row_after_save = None;
        self.fill_drag = None;
        cx.notify();
    }

    pub(super) fn open_delete_confirm(&mut self, cx: &mut Context<'_, Self>) {
        let Some(rows) = self.take_delete_rows(cx) else {
            return;
        };
        let grid_id = self.state.id;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.delete_confirm = Some(DeleteConfirm::Rows { grid_id, rows });
                cx.notify();
            });
        }
    }

    /// Stage the delete for the current selection: drop the selected pending insert rows and
    /// return the selected database rows, or `None` when nothing is left to delete. Kept apart
    /// from [`Self::open_delete_confirm`] so a context-menu handler, which already runs inside
    /// `AppView::update`, can build the confirmation without re-entering `AppView` — that
    /// re-entrant update aborted the app when 删除记录 was clicked.
    pub(super) fn take_delete_rows(&mut self, cx: &mut Context<'_, Self>) -> Option<Vec<usize>> {
        // Deletion needs a real target table, which only editable grids (a table page, or a query
        // grid with an inferred single table) have.
        if !self.state.editable {
            return None;
        }
        let selected_rows = self.state.selection.as_ref()?.row_indices();
        let data_rows = self.state.rows.len();
        let mut rows = Vec::new();
        let mut inserts = Vec::new();
        for row in selected_rows {
            match row_origin(&self.inserts, data_rows, row) {
                // Pending insert rows are not in the database yet: discard them outright.
                Some(GridRow::Insert(index)) => inserts.push(index),
                Some(GridRow::Data(row)) => rows.push(row),
                None => {}
            }
        }
        // Highest insert index first, so the remaining indices stay valid.
        inserts.sort_unstable();
        inserts.dedup();
        let mut removed = false;
        for index in inserts.into_iter().rev() {
            if index < self.inserts.len() {
                self.inserts.remove(index);
                removed = true;
            }
        }
        if rows.is_empty() {
            if removed {
                cx.notify();
            }
            return None;
        }
        Some(rows)
    }

    /// Surface a transient notification through the app. Safe from a grid update: `AppView` is a
    /// different entity, and the notification reaches the window on the next render.
    pub(super) fn show_toast(&self, kind: ToastKind, message: String, cx: &mut Context<'_, Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| app.toast(kind, message, cx));
        }
    }

    /// Like [`Self::show_toast`], but in the refresh slot: a new refresh replaces the previous
    /// one instead of stacking a second box.
    pub(super) fn show_refresh_toast(
        &self,
        kind: ToastKind,
        message: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| app.refresh_toast(kind, message, cx));
        }
    }

    /// Execute the confirmed row deletion for this grid, then reload the page.
    pub(super) fn delete_rows(&mut self, rows: Vec<usize>, cx: &mut Context<'_, Self>) {
        let key_columns = self.key_columns();
        let mut keys = Vec::new();
        for row in &rows {
            let Some(values) = self.state.rows.get(*row) else {
                continue;
            };
            let row_keys: Vec<(String, Option<String>)> = key_columns
                .iter()
                .filter_map(|&col| {
                    values
                        .get(self.state.source_col(col))
                        .map(|value| (self.state.columns[col].name.clone(), value.as_edit_value()))
                })
                .collect();
            if !row_keys.is_empty() {
                keys.push(row_keys);
            }
        }
        if keys.is_empty() {
            return;
        }

        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let runtime = self.runtime.clone();

        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.delete_rows(&database, &table, &keys).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |grid, cx| match result {
                Ok(()) => grid.reload_after_write(cx),
                Err(error) => {
                    let message = match error {
                        Error::Query(text) => text,
                        other => other.to_string(),
                    };
                    grid.state.error = Some(message.clone());
                    grid.show_error(message, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Surface a query error in the app-level error dialog.
    pub(super) fn show_error(&self, message: String, cx: &mut Context<'_, Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.error_dialog = Some(message);
                cx.notify();
            });
        }
    }
}
