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
            return;
        };
        if event.click_count >= 2 {
            if let GridHit::Cell(row, col) = hit {
                self.begin_edit((row, col), None, window, cx);
            }
            return;
        }
        let column_count = self.state.columns.len();
        match hit {
            GridHit::Gutter(row) => {
                if !self.state.rows.is_empty() {
                    self.state.selection = Some(CellSelection {
                        anchor: (row, 0),
                        cursor: (row, column_count.saturating_sub(1)),
                    });
                }
            }
            GridHit::Cell(row, col) => {
                if event.modifiers.shift {
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
        position: Point<Pixels>,
        delta: f32,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(selection) = self.state.selection else {
            return false;
        };
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let bounds = handle.bounds();
        if !bounds.contains(&position) {
            return false;
        }
        let rows = self.state.rows.len();
        if rows == 0 {
            return false;
        }

        let step: isize = if delta > 0.0 { -1 } else { 1 };
        let max = f32::from(handle.max_offset().height);
        let scroll = -f32::from(handle.offset().y);
        let viewport_h = f32::from(bounds.size.height);
        let visible = (viewport_h / GRID_ROW_HEIGHT).floor().max(1.0) as usize;
        let first = (scroll / GRID_ROW_HEIGHT).round() as isize;
        let max_first = ((max / GRID_ROW_HEIGHT).round() as isize).max(0);
        let new_scroll =
            ((first + step).clamp(0, max_first) as f32 * GRID_ROW_HEIGHT).clamp(0.0, max);
        let new_first = (new_scroll / GRID_ROW_HEIGHT).round() as usize;
        let new_last = (new_first + visible.saturating_sub(1)).min(rows - 1);

        let (row, col) = selection.cursor;
        let moved = (row as isize + step).clamp(0, rows as isize - 1) as usize;
        let new_row = moved.clamp(new_first, new_last);

        if new_scroll != scroll {
            let x = handle.offset().x;
            handle.set_offset(Point::new(x, px(-new_scroll)));
        }
        self.state.selection = Some(CellSelection {
            anchor: (new_row, col),
            cursor: (new_row, col),
        });

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
        let caret = value.chars().count();
        self.cell_editor = Some(CellEditor {
            row,
            col,
            cells,
            value: value.clone(),
            selection: FieldSelection {
                anchor: caret,
                cursor: caret,
            },
            selecting: false,
            history: Vec::new(),
        });
        self.cell_editor_focus_pending = true;
        window.focus(&self.cell_editor_focus);
        if is_temporal {
            self.open_date_picker(row, col, &value, cx);
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
    pub(super) fn sync_date_picker_to_editor(&mut self) {
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
            editor.history.push(editor.value.clone());
            editor.value = value;
            let caret = editor.value.chars().count();
            editor.selection = FieldSelection {
                anchor: caret,
                cursor: caret,
            };
        }
        self.caret_visible = true;
    }

    pub(super) fn editor_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let keystroke = event.keystroke.clone();
        let command = keystroke.modifiers.control || keystroke.modifiers.platform;
        let shift = keystroke.modifiers.shift;

        let Some(editor) = self.cell_editor.as_ref() else {
            return;
        };
        let text = editor.value.clone();
        let mut chars: Vec<char> = text.chars().collect();
        let len = chars.len();
        let mut selection = editor.selection;
        selection.anchor = selection.anchor.min(len);
        selection.cursor = selection.cursor.min(len);
        let (start, end) = selection.range();

        let mut new_value: Option<Vec<char>> = None;
        let mut new_cursor = selection.cursor;
        let mut new_anchor = selection.anchor;
        let mut undo = false;
        let mut commit = false;
        let mut cancel = false;

        if command {
            match keystroke.key.as_str() {
                "a" => {
                    new_anchor = 0;
                    new_cursor = len;
                }
                "c" => {
                    if start < end {
                        let selected: String = chars[start..end].iter().copied().collect();
                        cx.write_to_clipboard(ClipboardItem::new_string(selected));
                    }
                    return;
                }
                "x" => {
                    if start < end {
                        let selected: String = chars[start..end].iter().copied().collect();
                        cx.write_to_clipboard(ClipboardItem::new_string(selected));
                        chars.drain(start..end);
                        new_cursor = start;
                        new_anchor = start;
                        new_value = Some(chars);
                    } else {
                        return;
                    }
                }
                "v" => {
                    if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        let pasted: Vec<char> = pasted
                            .chars()
                            .filter(|character| *character != '\n' && *character != '\r')
                            .collect();
                        if !pasted.is_empty() {
                            let mut next: Vec<char> = chars[..start].to_vec();
                            next.extend_from_slice(&pasted);
                            next.extend_from_slice(&chars[end..]);
                            new_cursor = start + pasted.len();
                            new_anchor = new_cursor;
                            new_value = Some(next);
                        } else {
                            return;
                        }
                    } else {
                        return;
                    }
                }
                "z" => undo = true,
                _ => return,
            }
        } else {
            match keystroke.key.as_str() {
                "left" => {
                    let cursor = if shift {
                        selection.cursor.saturating_sub(1)
                    } else if start < end {
                        start
                    } else {
                        selection.cursor.saturating_sub(1)
                    };
                    new_cursor = cursor;
                    new_anchor = if shift { selection.anchor } else { cursor };
                }
                "right" => {
                    let cursor = if shift {
                        (selection.cursor + 1).min(len)
                    } else if start < end {
                        end
                    } else {
                        (selection.cursor + 1).min(len)
                    };
                    new_cursor = cursor;
                    new_anchor = if shift { selection.anchor } else { cursor };
                }
                "home" => {
                    new_cursor = 0;
                    new_anchor = if shift { selection.anchor } else { 0 };
                }
                "end" => {
                    new_cursor = len;
                    new_anchor = if shift { selection.anchor } else { len };
                }
                "backspace" => {
                    if start < end {
                        chars.drain(start..end);
                        new_cursor = start;
                        new_anchor = start;
                        new_value = Some(chars);
                    } else if start > 0 {
                        chars.remove(start - 1);
                        new_cursor = start - 1;
                        new_anchor = start - 1;
                        new_value = Some(chars);
                    } else {
                        return;
                    }
                }
                "delete" => {
                    if start < end {
                        chars.drain(start..end);
                        new_cursor = start;
                        new_anchor = start;
                        new_value = Some(chars);
                    } else if start < len {
                        chars.remove(start);
                        new_cursor = start;
                        new_anchor = start;
                        new_value = Some(chars);
                    } else {
                        return;
                    }
                }
                "enter" => commit = true,
                "escape" => cancel = true,
                _ => {
                    // Text characters (including IME composition) are delivered to
                    // `GridView::replace_text_in_range` via the platform input handler.
                    return;
                }
            }
        }

        if commit {
            self.finish_cell_editor(cx);
            return;
        }
        if cancel {
            self.cancel_editor(cx);
            return;
        }

        let Some(editor) = self.cell_editor.as_mut() else {
            return;
        };
        if undo {
            if let Some(previous) = editor.history.pop() {
                editor.value = previous;
            }
            let len = editor.value.chars().count();
            editor.selection = FieldSelection {
                anchor: len,
                cursor: len,
            };
        } else {
            if let Some(value) = new_value {
                editor.history.push(editor.value.clone());
                editor.value = value.into_iter().collect();
            }
            editor.selection = FieldSelection {
                anchor: new_anchor,
                cursor: new_cursor,
            };
        }
        self.caret_visible = true;
        cx.notify();
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

    pub(super) fn cell_editor_index_for_x(&self, value: &str, x: Pixels, window: &Window) -> usize {
        let char_count = value.chars().count();
        let col = self
            .cell_editor
            .as_ref()
            .map(|editor| editor.col)
            .unwrap_or(0);
        let content_left = self.list_scroll.0.borrow().base_handle.bounds().left();
        let cell_left: f32 =
            GRID_GUTTER_WIDTH + self.state.column_widths.iter().take(col).sum::<f32>();
        let text_left = f32::from(content_left) + cell_left + 8.0;
        let relative = f32::from(x) - text_left;
        if char_count == 0 || relative <= 0.0 {
            return 0;
        }
        let run = window.text_style().to_run(value.len());
        let layout = window
            .text_system()
            .layout_line(value, px(12.0), &[run], None);
        let byte = layout.closest_index_for_x(px(relative)).min(value.len());
        value[..byte].chars().count()
    }

    pub(super) fn cell_editor_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(value) = self.cell_editor.as_ref().map(|editor| editor.value.clone()) else {
            return;
        };
        let index = self.cell_editor_index_for_x(&value, event.position.x, window);
        if let Some(editor) = self.cell_editor.as_mut() {
            editor.selection.cursor = index;
        }
        cx.notify();
    }

    pub(super) fn commit_editor(&mut self, cx: &mut Context<'_, Self>) {
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
        cx.notify();
    }
}
