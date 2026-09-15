use super::*;

impl GridView {
    pub(super) fn grid_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.cell_editor.is_some() {
            self.finish_cell_editor(cx);
        }
        self.date_picker = None;

        let Some(hit) = self.grid_hit(event.position) else {
            self.cell_press = None;
            return;
        };
        let column_count = self.state.columns.len();
        self.cell_dragged = false;
        match hit {
            GridHit::Gutter(row) => {
                self.cell_press = None;
                if !self.state.rows.is_empty() {
                    self.state.selection = Some(CellSelection {
                        anchor: (row, 0),
                        cursor: (row, column_count.saturating_sub(1)),
                    });
                }
            }
            GridHit::Cell(row, col) => {
                if event.modifiers.shift {
                    self.cell_press = None;
                    if let Some(selection) = self.state.selection {
                        self.state.selection = Some(CellSelection {
                            anchor: selection.anchor,
                            cursor: (row, col),
                        });
                    } else {
                        self.state.selection = Some(CellSelection::new(row, col));
                    }
                } else {
                    self.state.selection = Some(CellSelection::new(row, col));
                    self.cell_press = Some((row, col));
                }
            }
        }
        self.selecting_cells = true;
        window.focus(&self.focus);
        cx.notify();
    }

    pub(super) fn grid_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        if !self.selecting_cells || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(hit) = self.grid_hit(event.position) else {
            return;
        };
        if let GridHit::Cell(row, col) = hit
            && self.cell_press != Some((row, col))
        {
            self.cell_dragged = true;
        }
        if let Some(selection) = self.state.selection {
            let (start_row, _) = selection.rows();
            self.state.selection = Some(match hit {
                GridHit::Gutter(row) => CellSelection {
                    anchor: (start_row, 0),
                    cursor: (row, self.state.columns.len().saturating_sub(1)),
                },
                GridHit::Cell(row, col) => CellSelection {
                    anchor: selection.anchor,
                    cursor: (row, col),
                },
            });
        }
        cx.notify();
    }

    /// Scroll the grid one row in response to a wheel notch, moving the selected row with the
    /// viewport. Returns `true` when the event was consumed.
    ///
    /// The viewport moves directly instead of the view snapping to the selection, so wheeling
    /// continues from wherever the scrollbar left the view. A selection that is currently
    /// off-screen moves to the edge row that scrolls into view rather than dragging the view
    /// back to it.
    pub(super) fn scroll_grid_selection(
        &mut self,
        _position: Point<Pixels>,
        delta: f32,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let bounds = handle.bounds();
        let rows = self.state.rows.len();
        if rows == 0 {
            return false;
        }

        let step: isize = if delta > 0.0 { -1 } else { 1 };
        let viewport_h = f32::from(bounds.size.height);
        if viewport_h <= 0.0 {
            return false;
        }
        // Derive the maximum scroll from the known row count instead of gpui's cached
        // `max_offset` (which can be a frame behind right after a scrollbar drag) so wheeling
        // never becomes stuck.
        let max = (rows as f32 * GRID_ROW_HEIGHT - viewport_h).max(0.0);
        if max <= 0.0 {
            return false;
        }
        let scroll = -f32::from(handle.offset().y);
        let visible = (viewport_h / GRID_ROW_HEIGHT).floor().max(1.0) as usize;
        let first = (scroll / GRID_ROW_HEIGHT).round() as isize;
        let max_first = ((max / GRID_ROW_HEIGHT).round() as isize).max(0);
        let new_scroll =
            ((first + step).clamp(0, max_first) as f32 * GRID_ROW_HEIGHT).clamp(0.0, max);
        if (new_scroll - scroll).abs() < 0.5 {
            // Already at the top/bottom.
            return false;
        }

        let new_first = (new_scroll / GRID_ROW_HEIGHT).round() as usize;
        let new_last = (new_first + visible.saturating_sub(1)).min(rows - 1);
        let x = handle.offset().x;
        handle.set_offset(Point::new(x, px(-new_scroll)));

        // Keep the cursor row inside the viewport, moving it to the edge if it scrolled away.
        if let Some(selection) = self.state.selection {
            let (row, col) = selection.cursor;
            let moved = (row as isize + step).clamp(0, rows as isize - 1) as usize;
            let new_row = moved.clamp(new_first, new_last);
            self.state.selection = Some(CellSelection {
                anchor: (new_row, col),
                cursor: (new_row, col),
            });
        }

        cx.notify();
        true
    }

    pub(super) fn grid_hit(&self, position: Point<Pixels>) -> Option<GridHit> {
        if self.state.rows.is_empty() {
            return None;
        }
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let bounds = handle.bounds();
        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
            return None;
        }
        let local_x = f32::from(position.x) - f32::from(bounds.left());
        let local_y =
            f32::from(position.y) - f32::from(bounds.top()) - f32::from(handle.offset().y);
        if local_x < 0.0 || local_y < 0.0 {
            return None;
        }
        let row = (local_y / GRID_ROW_HEIGHT).floor() as usize;
        if row >= self.state.rows.len() {
            return None;
        }
        if local_x < GRID_GUTTER_WIDTH {
            return Some(GridHit::Gutter(row));
        }
        let mut accumulated = GRID_GUTTER_WIDTH;
        for (col, width) in self.state.column_widths.iter().enumerate() {
            if local_x < accumulated + width {
                return Some(GridHit::Cell(row, col));
            }
            accumulated += width;
        }
        None
    }

    pub(super) fn grid_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.cell_editor.is_some() || self.date_picker.is_some() {
            return;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            if keystroke.key.eq_ignore_ascii_case("z") {
                self.undo_edit(cx);
            }
            return;
        }
        let Some((row, col)) = self.state.selection.map(|s| s.cursor) else {
            return;
        };

        match keystroke.key.as_str() {
            "up" | "down" | "left" | "right" => {
                let row_count = self.state.rows.len();
                let col_count = self.state.columns.len();
                if row_count == 0 || col_count == 0 {
                    return;
                }
                let (mut new_row, mut new_col) = (row, col);
                match keystroke.key.as_str() {
                    "up" => new_row = new_row.saturating_sub(1),
                    "down" => new_row = (new_row + 1).min(row_count - 1),
                    "left" => new_col = new_col.saturating_sub(1),
                    "right" => new_col = (new_col + 1).min(col_count - 1),
                    _ => {}
                }
                self.state.selection = Some(CellSelection::new(new_row, new_col));
                cx.stop_propagation();
                cx.notify();
            }
            "delete" => {
                self.set_selection_null(cx);
                cx.stop_propagation();
            }
            "enter" => {
                self.begin_edit((row, col), None, window, cx);
                cx.stop_propagation();
            }
            _ => {
                if let Some(text) = keystroke.key_char.as_ref()
                    && let Some(character) = text.chars().next()
                    && !character.is_control()
                {
                    self.begin_edit((row, col), Some(character), window, cx);
                    // Consume the keystroke so the platform does not also emit a `WM_CHAR`
                    // for the character that seeded the edit.
                    cx.stop_propagation();
                }
            }
        }
    }

    pub(super) fn undo_edit(&mut self, cx: &mut Context<'_, Self>) {
        let Some(action) = self.state.undo.pop() else {
            return;
        };
        for ((row, col), previous) in action {
            match previous {
                Some(value) => {
                    self.state.edits.insert((row, col), value);
                }
                None => {
                    self.state.edits.remove(&(row, col));
                }
            }
        }
        cx.notify();
    }

    pub(super) fn set_selection_null(&mut self, cx: &mut Context<'_, Self>) {
        let cells = self
            .state
            .selection
            .map(|selection| selection.cells())
            .unwrap_or_default();
        if cells.is_empty() {
            return;
        }
        let mut action = Vec::new();
        for (row, col) in cells {
            if row >= self.state.rows.len() || col >= self.state.columns.len() {
                continue;
            }
            if !self.state.edits.contains_key(&(row, col))
                && matches!(self.state.rows[row][col], CellValue::Null)
            {
                continue;
            }
            let previous = self.state.edits.get(&(row, col)).cloned();
            if previous == Some(None) {
                continue;
            }
            action.push(((row, col), previous));
            self.state.edits.insert((row, col), None);
        }
        if !action.is_empty() {
            self.state.undo.push(action);
            if self.state.undo.len() > 256 {
                self.state.undo.remove(0);
            }
        }
        cx.notify();
    }

    pub(super) fn begin_edit(
        &mut self,
        cell: (usize, usize),
        initial: Option<char>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let (row, col) = cell;
        if !self.state.editable || row >= self.state.rows.len() || col >= self.state.columns.len() {
            return;
        }
        let value = self
            .state
            .edits
            .get(&(row, col))
            .cloned()
            .flatten()
            .unwrap_or_else(|| self.state.rows[row][col].as_edit_string());
        let is_temporal = is_temporal_type(&self.state.columns[col].data_type);
        let cells = self
            .state
            .selection
            .filter(|selection| selection.contains(row, col))
            .map(|selection| selection.cells())
            .unwrap_or_else(|| vec![(row, col)]);

        let value = match initial {
            Some(character) => character.to_string(),
            None => value,
        };

        let theme = self.theme;
        let weak = self.self_weak.clone();
        let initial_text = value.clone();
        let input = cx.new(move |cx| {
            TextInput::new(theme, initial_text, TextInputOptions::default(), cx)
                .on_change(Rc::new({
                    let weak = weak.clone();
                    move |text, _window, cx| {
                        let _ = weak.update(cx, |grid, cx| grid.cell_editor_changed(text, cx));
                    }
                }))
                .on_submit(Rc::new({
                    let weak = weak.clone();
                    move |_window, cx| {
                        let _ = weak.update(cx, |grid, cx| grid.finish_cell_editor(cx));
                    }
                }))
                .on_cancel(Rc::new({
                    let weak = weak.clone();
                    move |_window, cx| {
                        let _ = weak.update(cx, |grid, cx| grid.cancel_editor(cx));
                    }
                }))
        });
        let focus = input.read(cx).focus_handle();
        input.update(cx, |input, cx| input.set_padding_left(0.0, cx));
        self.cell_editor_blur_subscription =
            Some(cx.on_blur(&focus, window, |this, _window, cx| {
                if this.cell_editor.is_some() {
                    this.finish_cell_editor(cx);
                }
            }));
        self.cell_editor = Some(CellEditor {
            row,
            col,
            cells,
            value: value.clone(),
            input,
        });
        self.cell_editor_focus_pending = true;
        window.focus(&focus);
        if is_temporal {
            self.open_date_picker(row, col, &value, cx);
        }
        cx.notify();
    }

    /// Mirror the editor's text so the multi-cell preview and the eventual commit see it.
    fn cell_editor_changed(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        if let Some(editor) = self.cell_editor.as_mut() {
            editor.value = text.to_string();
        }
        cx.notify();
    }

    pub(super) fn open_date_picker(
        &mut self,
        row: usize,
        col: usize,
        value: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let data_type = self.state.columns[col].data_type.to_ascii_lowercase();
        let has_time = data_type.contains("datetime") || data_type.contains("timestamp");
        let base = parse_datetime(value).unwrap_or_else(|| chrono::Local::now().naive_local());
        self.date_picker = Some(DatePicker {
            row,
            col,
            year: base.year(),
            month: base.month(),
            day: base.day(),
            hour: base.hour(),
            minute: base.minute(),
            second: base.second(),
            has_time,
        });
        cx.notify();
    }

    /// Rewrite the cell editor's text from the current date picker state.
    pub(super) fn sync_date_picker_to_editor(&mut self, cx: &mut Context<'_, Self>) {
        let Some(picker) = self.date_picker.as_ref() else {
            return;
        };
        let value = if picker.has_time {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                picker.year, picker.month, picker.day, picker.hour, picker.minute, picker.second
            )
        } else {
            format!("{:04}-{:02}-{:02}", picker.year, picker.month, picker.day)
        };
        if let Some(editor) = self.cell_editor.as_mut() {
            editor.value = value.clone();
            let input = editor.input.clone();
            input.update(cx, |input, cx| input.set_text(value, cx));
        }
        self.caret_visible = true;
    }

    /// Stage the current cell edit, then auto-commit single-cell edits to the database.
    /// Batch edits stay pending until the user presses the commit button.
    pub(super) fn finish_cell_editor(&mut self, cx: &mut Context<'_, Self>) {
        let multi = self
            .cell_editor
            .as_ref()
            .is_some_and(|editor| editor.cells.len() > 1);
        if self.cell_editor.is_none() {
            return;
        }
        self.commit_editor(cx);
        if !multi {
            self.commit_edits(cx);
        }
    }

    pub(super) fn commit_editor(&mut self, cx: &mut Context<'_, Self>) {
        self.cell_editor_blur_subscription = None;
        let Some(editor) = self.cell_editor.take() else {
            return;
        };
        let mut action = Vec::new();
        for (row, col) in editor.cells {
            if row >= self.state.rows.len() || col >= self.state.columns.len() {
                continue;
            }
            let previous = self.state.edits.get(&(row, col)).cloned();
            let original = match &previous {
                Some(value) => value.clone(),
                None => match &self.state.rows[row][col] {
                    CellValue::Null => None,
                    cell => Some(cell.as_edit_string()),
                },
            };
            let desired = if editor.value.is_empty() && original.is_none() {
                None
            } else {
                Some(editor.value.clone())
            };
            if original == desired {
                continue;
            }
            action.push(((row, col), previous));
            self.state.edits.insert((row, col), desired);
        }
        if !action.is_empty() {
            self.state.undo.push(action);
            if self.state.undo.len() > 256 {
                self.state.undo.remove(0);
            }
        }
        cx.notify();
    }

    pub(super) fn cancel_editor(&mut self, cx: &mut Context<'_, Self>) {
        self.cell_editor = None;
        self.cell_editor_blur_subscription = None;
        self.date_picker = None;
        cx.notify();
    }
}
