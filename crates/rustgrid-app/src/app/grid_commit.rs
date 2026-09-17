use super::*;

impl GridView {
    pub(super) fn date_picker_shift_month(&mut self, delta: i32, cx: &mut Context<'_, Self>) {
        if let Some(picker) = self.date_picker.as_mut() {
            let mut month = picker.month as i32 + delta;
            let mut year = picker.year;
            while month < 1 {
                month += 12;
                year -= 1;
            }
            while month > 12 {
                month -= 12;
                year += 1;
            }
            picker.month = month as u32;
            picker.year = year;
            picker.day = picker.day.min(days_in_month(year, picker.month));
        }
        cx.notify();
    }

    pub(super) fn date_picker_select_day(&mut self, day: u32, cx: &mut Context<'_, Self>) {
        if let Some(picker) = self.date_picker.as_mut() {
            picker.day = day;
        }
        self.sync_date_picker_to_editor(cx);
        cx.notify();
    }

    pub(super) fn date_picker_shift_time(
        &mut self,
        field: usize,
        delta: i32,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(picker) = self.date_picker.as_mut() {
            match field {
                0 => picker.hour = wrap_unit(picker.hour, delta, 24),
                1 => picker.minute = wrap_unit(picker.minute, delta, 60),
                _ => picker.second = wrap_unit(picker.second, delta, 60),
            }
        }
        self.sync_date_picker_to_editor(cx);
        cx.notify();
    }

    pub(super) fn date_picker_today(&mut self, cx: &mut Context<'_, Self>) {
        let now = chrono::Local::now().naive_local();
        if let Some(picker) = self.date_picker.as_mut() {
            picker.year = now.year();
            picker.month = now.month();
            picker.day = now.day();
            picker.hour = now.hour();
            picker.minute = now.minute();
            picker.second = now.second();
        }
        self.sync_date_picker_to_editor(cx);
        cx.notify();
    }

    pub(super) fn date_picker_ok(&mut self, cx: &mut Context<'_, Self>) {
        if self.date_picker.take().is_none() {
            return;
        }
        self.sync_date_picker_to_editor(cx);
        self.finish_cell_editor(cx);
    }

    pub(super) fn date_picker_cancel(&mut self, cx: &mut Context<'_, Self>) {
        self.date_picker = None;
        self.cancel_editor(cx);
    }

    pub(super) fn commit_edits(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.edits.is_empty() {
            return;
        }

        let primary_keys: Vec<usize> = self
            .state
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.primary_key)
            .map(|(index, _)| index)
            .collect();
        let key_columns: Vec<usize> = if primary_keys.is_empty() {
            (0..self.state.columns.len()).collect()
        } else {
            primary_keys
        };

        let mut by_row: BTreeMap<usize, Vec<(usize, Option<String>)>> = BTreeMap::new();
        for (&(row, col), value) in &self.state.edits {
            by_row.entry(row).or_default().push((col, value.clone()));
        }

        let mut updates = Vec::new();
        for (row, cells) in by_row {
            let Some(row_values) = self.state.rows.get(row) else {
                continue;
            };
            let set = cells
                .into_iter()
                .map(|(col, value)| (self.state.columns[col].name.clone(), value))
                .collect();
            let keys = key_columns
                .iter()
                .filter_map(|&col| {
                    row_values
                        .get(col)
                        .map(|value| (self.state.columns[col].name.clone(), value.as_edit_string()))
                })
                .collect();
            updates.push(RowUpdate { set, keys });
        }

        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let runtime = self.runtime.clone();

        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.update_rows(&database, &table, &updates).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |grid, cx| match result {
                Ok(()) => grid.load_page(cx),
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

    pub(super) fn cancel_edits(&mut self, cx: &mut Context<'_, Self>) {
        self.state.edits.clear();
        self.state.selection = None;
        self.cell_editor = None;
        self.cell_editor_blur_subscription = None;
        self.date_picker = None;
        cx.notify();
    }

    pub(super) fn open_delete_confirm(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
            return;
        }
        let Some(selection) = self.state.selection else {
            return;
        };
        let (start, end) = selection.rows();
        let rows: Vec<usize> = (start..=end)
            .filter(|row| *row < self.state.rows.len())
            .collect();
        if rows.is_empty() {
            return;
        }
        let grid_id = self.state.id;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.delete_confirm = Some(DeleteConfirm::Rows { grid_id, rows });
                cx.notify();
            });
        }
    }

    /// Execute the confirmed row deletion for this grid, then reload the page.
    pub(super) fn delete_rows(&mut self, rows: Vec<usize>, cx: &mut Context<'_, Self>) {
        let primary_keys: Vec<usize> = self
            .state
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.primary_key)
            .map(|(index, _)| index)
            .collect();
        let key_columns: Vec<usize> = if primary_keys.is_empty() {
            (0..self.state.columns.len()).collect()
        } else {
            primary_keys
        };

        let mut keys = Vec::new();
        for row in &rows {
            let Some(values) = self.state.rows.get(*row) else {
                continue;
            };
            let row_keys: Vec<(String, String)> = key_columns
                .iter()
                .filter_map(|&col| {
                    values
                        .get(col)
                        .map(|value| (self.state.columns[col].name.clone(), value.as_edit_string()))
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
                Ok(()) => grid.load_page(cx),
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
    fn show_error(&self, message: String, cx: &mut Context<'_, Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.error_dialog = Some(message);
                cx.notify();
            });
        }
    }
}
