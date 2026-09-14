use super::*;

impl AppView {
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
        self.sync_date_picker_to_editor();
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
        self.sync_date_picker_to_editor();
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
        self.sync_date_picker_to_editor();
        cx.notify();
    }

    pub(super) fn date_picker_ok(&mut self, cx: &mut Context<'_, Self>) {
        if self.date_picker.take().is_none() {
            return;
        }
        self.sync_date_picker_to_editor();
        self.finish_cell_editor(cx);
    }

    pub(super) fn date_picker_cancel(&mut self, cx: &mut Context<'_, Self>) {
        self.date_picker = None;
        self.cancel_editor(cx);
    }

    pub(super) fn commit_edits(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_grid else {
            return;
        };
        let Some(grid) = self.grids.get(index) else {
            return;
        };
        if grid.edits.is_empty() {
            return;
        }

        let primary_keys: Vec<usize> = grid
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.primary_key)
            .map(|(index, _)| index)
            .collect();
        let key_columns: Vec<usize> = if primary_keys.is_empty() {
            (0..grid.columns.len()).collect()
        } else {
            primary_keys
        };

        let mut by_row: BTreeMap<usize, Vec<(usize, Option<String>)>> = BTreeMap::new();
        for (&(row, col), value) in &grid.edits {
            by_row.entry(row).or_default().push((col, value.clone()));
        }

        let mut updates = Vec::new();
        for (row, cells) in by_row {
            let Some(row_values) = grid.rows.get(row) else {
                continue;
            };
            let set = cells
                .into_iter()
                .map(|(col, value)| (grid.columns[col].name.clone(), value))
                .collect();
            let keys = key_columns
                .iter()
                .filter_map(|&col| {
                    row_values
                        .get(col)
                        .map(|value| (grid.columns[col].name.clone(), value.as_edit_string()))
                })
                .collect();
            updates.push(RowUpdate { set, keys });
        }

        let connection = grid.connection.clone();
        let database = grid.database.clone();
        let table = grid.table.clone();
        let id = grid.id;
        let runtime = self.runtime.clone();

        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.update_rows(&database, &table, &updates).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| match result {
                Ok(()) => view.load_page(id, cx),
                Err(error) => {
                    let message = match error {
                        Error::Query(text) => text,
                        other => other.to_string(),
                    };
                    if let Some(grid) = view.grids.iter_mut().find(|grid| grid.id == id) {
                        grid.error = Some(message.clone());
                    }
                    view.error_dialog = Some(message);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn cancel_edits(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.active_grid
            && let Some(grid) = self.grids.get_mut(index)
        {
            grid.edits.clear();
            grid.selection = None;
        }
        self.cell_editor = None;
        self.date_picker = None;
        cx.notify();
    }

    pub(super) fn open_delete_confirm(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_grid else {
            return;
        };
        let Some(grid) = self.grids.get(index) else {
            return;
        };
        if grid.sql.is_some() {
            return;
        }
        let Some(selection) = grid.selection else {
            return;
        };
        let (start, end) = selection.rows();
        let rows: Vec<usize> = (start..=end).filter(|row| *row < grid.rows.len()).collect();
        if rows.is_empty() {
            return;
        }
        self.delete_confirm = Some(DeleteConfirm {
            grid_id: grid.id,
            rows,
        });
        cx.notify();
    }

    pub(super) fn cancel_delete(&mut self, cx: &mut Context<'_, Self>) {
        self.delete_confirm = None;
        cx.notify();
    }

    pub(super) fn confirm_delete(&mut self, cx: &mut Context<'_, Self>) {
        let Some(confirm) = self.delete_confirm.take() else {
            return;
        };
        let Some(grid) = self.grids.iter().find(|grid| grid.id == confirm.grid_id) else {
            return;
        };

        let primary_keys: Vec<usize> = grid
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.primary_key)
            .map(|(index, _)| index)
            .collect();
        let key_columns: Vec<usize> = if primary_keys.is_empty() {
            (0..grid.columns.len()).collect()
        } else {
            primary_keys
        };

        let mut keys = Vec::new();
        for row in &confirm.rows {
            let Some(values) = grid.rows.get(*row) else {
                continue;
            };
            let row_keys: Vec<(String, String)> = key_columns
                .iter()
                .filter_map(|&col| {
                    values
                        .get(col)
                        .map(|value| (grid.columns[col].name.clone(), value.as_edit_string()))
                })
                .collect();
            if !row_keys.is_empty() {
                keys.push(row_keys);
            }
        }
        if keys.is_empty() {
            return;
        }

        let connection = grid.connection.clone();
        let database = grid.database.clone();
        let table = grid.table.clone();
        let id = grid.id;
        let runtime = self.runtime.clone();

        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.delete_rows(&database, &table, &keys).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| match result {
                Ok(()) => view.load_page(id, cx),
                Err(error) => {
                    let message = match error {
                        Error::Query(text) => text,
                        other => other.to_string(),
                    };
                    if let Some(grid) = view.grids.iter_mut().find(|grid| grid.id == id) {
                        grid.error = Some(message.clone());
                    }
                    view.error_dialog = Some(message);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
