use super::*;

impl GridView {
    pub(super) fn grid_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.cell_editor.is_some() {
            // The in-place editor owns the mouse inside its own cell: let gpui-kit's input place
            // the caret and drag-select text instead of driving a cell selection under it.
            let editing_here = self.grid_hit(event.position).is_some_and(|hit| match hit {
                GridHit::Cell(row, col) => self
                    .cell_editor
                    .as_ref()
                    .is_some_and(|editor| editor.row == row && editor.col == col),
                GridHit::Gutter(_) => false,
            });
            if editing_here {
                self.cell_press = None;
                self.selecting_cells = false;
                return;
            }
            self.finish_cell_editor(cx);
        }
        self.date_picker = None;

        let Some(hit) = self.grid_hit(event.position) else {
            self.cell_press = None;
            return;
        };
        let column_count = self.state.columns.len();
        self.cell_dragged = false;
        let ctrl = event.modifiers.control || event.modifiers.platform;
        match hit {
            GridHit::Gutter(row) => {
                self.cell_press = None;
                if self.display_row_count() == 0 {
                    self.selecting_cells = false;
                    cx.notify();
                    return;
                }
                let last = column_count.saturating_sub(1);
                if event.modifiers.shift {
                    if let Some(selection) = self.state.selection.as_mut() {
                        let active = &mut selection.ranges[selection.active];
                        active.cursor = (row, last);
                    } else {
                        self.state.selection = Some(CellSelection::single((row, 0), (row, last)));
                    }
                } else if ctrl {
                    self.add_or_remove_range(row, 0, row, last);
                } else {
                    self.state.selection = Some(CellSelection::single((row, 0), (row, last)));
                }
            }
            GridHit::Cell(row, col) => {
                if event.modifiers.shift {
                    self.cell_press = None;
                    if let Some(selection) = self.state.selection.as_mut() {
                        let active = &mut selection.ranges[selection.active];
                        active.cursor = (row, col);
                    } else {
                        self.state.selection = Some(CellSelection::new(row, col));
                    }
                } else if ctrl {
                    // Excel-like: Ctrl+click toggles the cell in/out of the selection.
                    self.cell_press = None;
                    self.add_or_remove_range(row, col, row, col);
                } else {
                    self.state.selection = Some(CellSelection::new(row, col));
                    self.cell_press = Some((row, col));
                }
            }
        }
        self.selecting_cells = true;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Add a range spanning `(start_row, start_col)..=(end_row, end_col)` to the selection, or
    /// remove the range that already contains that cell (Ctrl+click toggle).
    fn add_or_remove_range(
        &mut self,
        anchor_row: usize,
        anchor_col: usize,
        cursor_row: usize,
        cursor_col: usize,
    ) {
        if self.state.selection.is_none() {
            self.state.selection = Some(CellSelection::single(
                (anchor_row, anchor_col),
                (cursor_row, cursor_col),
            ));
            return;
        }
        let empty = {
            let Some(selection) = self.state.selection.as_mut() else {
                return;
            };
            if let Some(position) = selection
                .ranges
                .iter()
                .position(|range| range.contains(anchor_row, anchor_col))
            {
                selection.ranges.remove(position);
            } else {
                selection.ranges.push(CellRange {
                    anchor: (anchor_row, anchor_col),
                    cursor: (cursor_row, cursor_col),
                });
                selection.active = selection.ranges.len() - 1;
            }
            if !selection.ranges.is_empty() {
                selection.active = selection.active.min(selection.ranges.len() - 1);
            }
            selection.ranges.is_empty()
        };
        if empty {
            self.state.selection = None;
        }
    }

    pub(super) fn grid_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        if !self.selecting_cells || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(hit) = self.grid_hit(event.position) else {
            return;
        };
        let mut changed = false;
        if let GridHit::Cell(row, col) = hit
            && self.cell_press != Some((row, col))
            && !self.cell_dragged
        {
            self.cell_dragged = true;
            changed = true;
        }
        let column_count = self.state.columns.len();
        match hit {
            GridHit::Gutter(row) => {
                let first = self
                    .state
                    .selection
                    .as_ref()
                    .map(|selection| selection.rows().0)
                    .unwrap_or(row);
                let last = column_count.saturating_sub(1);
                let next = CellSelection::single((first, 0), (row, last));
                if self.state.selection.as_ref() != Some(&next) {
                    self.state.selection = Some(next);
                    changed = true;
                }
            }
            GridHit::Cell(row, col) => {
                if let Some(selection) = self.state.selection.as_mut() {
                    let active = &mut selection.ranges[selection.active];
                    if active.cursor != (row, col) {
                        active.cursor = (row, col);
                        changed = true;
                    }
                }
            }
        }
        // A pointer move within the same cell changes nothing on screen; only notify on a real
        // change so a drag does not re-render the whole window for every pixel.
        if changed {
            cx.notify();
        }
    }

    /// Scroll the grid by `delta` pixels (`> 0` scrolls toward the top) in response to the wheel,
    /// keeping the selected row inside the viewport. Returns `true` when the event was consumed.
    ///
    /// Scrolling is pixel-based so trackpads and high-resolution wheels move smoothly instead of
    /// jumping a whole row per event. A selection that is currently off-screen moves to the edge
    /// row that scrolls into view rather than dragging the view back to it.
    pub(super) fn scroll_grid_selection(
        &mut self,
        _position: Point<Pixels>,
        delta: f32,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        self.end_transient_editing(cx);
        let rows = self.display_row_count();
        if rows == 0 || delta == 0.0 {
            return false;
        }
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let viewport_h = f32::from(handle.bounds().size.height);
        if viewport_h <= 0.0 {
            return false;
        }
        // Prefer gpui's measured max, but fall back to the row count for the first frame after a
        // page loads (the handle is replaced and its extents are not measured until paint). This
        // keeps wheeling from getting stuck right after loading.
        let mut max = f32::from(handle.max_offset().y);
        if max <= 0.0 {
            max = (rows as f32 * GRID_ROW_HEIGHT - viewport_h).max(0.0);
        }
        if max <= 0.0 {
            // No vertical overflow (every row is visible): the wheel moves the selected cell
            // up/down instead of scrolling. With no selection, pan horizontally so a short but
            // wide table's off-screen columns are still reachable.
            if let Some(selection) = self.state.selection.as_ref() {
                // One wheel notch moves the selection a single row, regardless of how many
                // "lines" the OS reports for that notch.
                let steps: isize = if delta > 0.0 { -1 } else { 1 };
                let (row, col) = selection.active_cursor();
                let new_row = (row as isize + steps).clamp(0, rows as isize - 1) as usize;
                if new_row == row {
                    return false;
                }
                self.state.selection = Some(CellSelection::single((new_row, col), (new_row, col)));
                cx.notify();
                return true;
            }
            return self.scroll_grid_columns(delta, cx);
        }
        let scroll = -f32::from(handle.offset().y);
        let new_scroll = (scroll - delta).clamp(0.0, max);
        if (new_scroll - scroll).abs() < 0.5 {
            // Already at the top/bottom.
            return false;
        }
        let x = handle.offset().x;
        handle.set_offset(Point::new(x, px(-new_scroll)));

        // Keep the cursor row inside the viewport, moving it to the nearest edge if it scrolled
        // away (rather than dragging the view back to it).
        if let Some(selection) = self.state.selection.as_ref() {
            let (row, col) = selection.active_cursor();
            let first = (new_scroll / GRID_ROW_HEIGHT).floor() as usize;
            let visible = (viewport_h / GRID_ROW_HEIGHT).ceil() as usize;
            let last = (first + visible).min(rows);
            if row < first || row >= last {
                let new_row = row.clamp(first, last.saturating_sub(1));
                self.state.selection = Some(CellSelection::single((new_row, col), (new_row, col)));
            }
        }

        cx.notify();
        true
    }

    /// Scrolling is an implicit "done": dismiss the date picker (an absolute overlay anchored to
    /// a row would otherwise float over unrelated rows) and commit/close the in-place editor so
    /// it cannot stay anchored to a row that is moving away.
    fn end_transient_editing(&mut self, cx: &mut Context<'_, Self>) {
        self.date_picker = None;
        if self.cell_editor.is_some() {
            self.finish_cell_editor(cx);
        }
    }

    /// Pan the columns by `delta` pixels (`> 0` scrolls toward the first column), for a horizontal
    /// trackpad swipe. Also used as the wheel fallback when the grid cannot scroll vertically, so
    /// a short but wide table's off-screen columns are still reachable.
    pub(super) fn scroll_grid_columns(&mut self, delta: f32, cx: &mut Context<'_, Self>) -> bool {
        self.end_transient_editing(cx);
        let max = f32::from(self.hscroll.max_offset().x);
        if max <= 0.0 {
            return false;
        }
        let scroll = -f32::from(self.hscroll.offset().x);
        let new_scroll = (scroll - delta).clamp(0.0, max);
        if (new_scroll - scroll).abs() < 0.5 {
            return false;
        }
        let y = self.hscroll.offset().y;
        self.hscroll.set_offset(Point::new(px(-new_scroll), y));
        cx.notify();
        true
    }

    /// Start dragging the right edge of `col`; the width follows the pointer until mouse-up.
    pub(super) fn begin_column_resize(
        &mut self,
        col: usize,
        mouse_x: Pixels,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(width) = self.state.column_widths.get(col).copied() else {
            return;
        };
        self.column_resize = Some(ColumnResize {
            col,
            start_x: f32::from(mouse_x),
            start_width: width,
        });
        cx.notify();
    }

    pub(super) fn grid_column_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some(resize) = self.column_resize else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.column_resize = None;
            cx.notify();
            return;
        }
        let delta = f32::from(event.position.x) - resize.start_x;
        let width = (resize.start_width + delta).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
        if let Some(slot) = self.state.column_widths.get_mut(resize.col)
            && *slot != width
        {
            *slot = width;
            self.state.manual_column_widths = true;
            cx.notify();
        }
    }

    pub(super) fn grid_hit(&self, position: Point<Pixels>) -> Option<GridHit> {
        if self.display_row_count() == 0 {
            return None;
        }
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let bounds = handle.bounds();
        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
            return None;
        }
        // The list's own bounds are already translated by the parent's horizontal scroll, so the
        // horizontal offset must be applied against the scroll viewport instead — subtracting both
        // would double-count it and land the hit a few columns to the right when scrolled.
        let viewport = self.hscroll.bounds();
        let local_x = if viewport.size.width > px(0.0) {
            f32::from(position.x) - f32::from(viewport.left()) - f32::from(self.hscroll.offset().x)
        } else {
            f32::from(position.x) - f32::from(bounds.left())
        };
        let local_y =
            f32::from(position.y) - f32::from(bounds.top()) - f32::from(handle.offset().y);
        if local_x < 0.0 || local_y < 0.0 {
            return None;
        }
        let row = (local_y / GRID_ROW_HEIGHT).floor() as usize;
        if row >= self.display_row_count() {
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
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            // Save works while the in-place editor holds focus too (the key bubbles up to here).
            if keystroke.key.eq_ignore_ascii_case("s") {
                self.save_grid(cx);
                cx.stop_propagation();
            } else if self.cell_editor.is_none()
                && self.date_picker.is_none()
                && keystroke.key.eq_ignore_ascii_case("z")
            {
                self.undo_edit(cx);
            }
            return;
        }
        if self.cell_editor.is_some() || self.date_picker.is_some() {
            return;
        }
        let Some((row, col)) = self
            .state
            .selection
            .as_ref()
            .map(|selection| selection.active_cursor())
        else {
            return;
        };

        match keystroke.key.as_str() {
            "up" | "down" | "left" | "right" => {
                let row_count = self.display_row_count();
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
            .as_ref()
            .map(|selection| selection.cells())
            .unwrap_or_default();
        if cells.is_empty() {
            return;
        }
        let data_rows = self.state.rows.len();
        let mut action = Vec::new();
        for (row, col) in cells {
            if col >= self.state.columns.len() {
                continue;
            }
            if row >= data_rows {
                // Pending insert rows have no undo stack: clearing just stages an explicit NULL.
                if let Some(insert) = self.inserts.get_mut(row - data_rows)
                    && insert.get(&col) != Some(&None)
                {
                    insert.insert(col, None);
                }
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
        if !self.state.editable
            || row >= self.display_row_count()
            || col >= self.state.columns.len()
        {
            return;
        }
        let value = self
            .staged_value(row, col)
            .cloned()
            .flatten()
            .unwrap_or_else(|| {
                if row < self.state.rows.len() {
                    self.state.rows[row][col].as_edit_string()
                } else {
                    String::new()
                }
            });
        let is_temporal = is_temporal_type(&self.state.columns[col].data_type);
        let cells = self
            .state
            .selection
            .as_ref()
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
            TextInput::new(
                theme,
                initial_text,
                TextInputOptions {
                    bare: true,
                    text_size: Some(12.5),
                    ..Default::default()
                },
                cx,
            )
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
                // While the date/time picker is open focus may sit in its `TimeField`; losing the
                // editor's focus then is not "leaving the edit", so do not auto-commit. The picker
                // commits or cancels explicitly (and clicking another cell commits through
                // `grid_mouse_down`).
                if this.cell_editor.is_some() && this.date_picker.is_none() {
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
        window.focus(&focus, cx);
        if is_temporal {
            self.open_date_picker(row, col, &value, window, cx);
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
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let data_type = self.state.columns[col].data_type.to_ascii_lowercase();
        let has_time = data_type.contains("datetime") || data_type.contains("timestamp");
        let base = parse_datetime(value).unwrap_or_else(|| chrono::Local::now().naive_local());
        let date = NaiveDate::from_ymd_opt(base.year(), base.month(), base.day())
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let calendar = cx.new(|cx| {
            let mut state = CalendarState::new(window, cx);
            state.set_date(date, window, cx);
            state
        });
        // `TimePrecision::Second` matches MySQL's rendered `datetime`/`timestamp`; a `date` column
        // gets no time field at all.
        let time_field = if has_time {
            let time = NaiveTime::from_hms_opt(base.hour(), base.minute(), base.second())
                .unwrap_or_default();
            Some(cx.new(|cx| {
                let mut state = TimeFieldState::new(window, cx).precision(TimePrecision::Second);
                state.set_time(time, window, cx);
                state
            }))
        } else {
            None
        };
        self.date_picker = Some(DatePicker {
            row,
            col,
            has_time,
            calendar,
            time_field,
        });
        cx.notify();
    }

    /// Toggle the in-place date/time picker for the cell being edited (the cell's "…" button):
    /// open it from the editor's current value, or apply-and-close it when already open.
    pub(super) fn toggle_date_picker(
        &mut self,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self
            .date_picker
            .as_ref()
            .is_some_and(|picker| picker.row == row && picker.col == col)
        {
            self.date_picker_ok(window, cx);
            return;
        }
        let Some(value) = self.cell_editor.as_ref().map(|editor| editor.value.clone()) else {
            return;
        };
        self.open_date_picker(row, col, &value, window, cx);
    }

    /// Rewrite the cell editor's text from the current date picker state.
    pub(super) fn sync_date_picker_to_editor(&mut self, cx: &mut Context<'_, Self>) {
        let Some(picker) = self.date_picker.as_ref() else {
            return;
        };
        let Some(date) = picker.calendar.read(cx).date().start() else {
            return;
        };
        let value = if picker.has_time {
            let time = picker
                .time_field
                .as_ref()
                .map(|field| field.read(cx).time())
                .unwrap_or_default();
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                date.year(),
                date.month(),
                date.day(),
                time.hour(),
                time.minute(),
                time.second()
            )
        } else {
            format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
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
        let data_rows = self.state.rows.len();
        let Some(editor) = self.cell_editor.as_ref() else {
            return;
        };
        let multi = editor.cells.len() > 1;
        // A single-cell edit on an existing row auto-commits; a pending insert row waits for Save
        // (the checkmark or Ctrl+S) so a whole new row is inserted in one statement.
        let only_data_rows = editor.row < data_rows;
        self.commit_editor(cx);
        if !multi && only_data_rows {
            self.commit_edits(cx);
        }
    }

    pub(super) fn commit_editor(&mut self, cx: &mut Context<'_, Self>) {
        self.cell_editor_blur_subscription = None;
        // The calendar is an absolute overlay: it must not outlive the editor, or it keeps
        // floating over (and swallowing clicks aimed at) the rows.
        self.date_picker = None;
        let Some(editor) = self.cell_editor.take() else {
            return;
        };
        let data_rows = self.state.rows.len();
        let mut action = Vec::new();
        for (row, col) in editor.cells {
            if col >= self.state.columns.len() {
                continue;
            }
            if row >= data_rows {
                // Stage into the pending insert row instead of the existing-row edit map.
                let Some(insert) = self.inserts.get_mut(row - data_rows) else {
                    continue;
                };
                let previous = insert.get(&col).cloned();
                let staged = if editor.value.is_empty() && previous.is_none() {
                    None
                } else if editor.value.is_empty() {
                    Some(None)
                } else {
                    Some(Some(editor.value.clone()))
                };
                if previous == staged {
                    continue;
                }
                match staged {
                    Some(value) => {
                        insert.insert(col, value);
                    }
                    None => {
                        insert.remove(&col);
                    }
                }
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

    /// Tab in a cell editor: stage the current value and open the editor on the next (or, with
    /// Shift, previous) cell, wrapping to the following/previous row. The value is left pending
    /// rather than auto-committed so a whole row's edits are saved together with the checkmark.
    pub(super) fn cell_editor_tab(
        &mut self,
        backwards: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(editor) = self.cell_editor.as_ref() else {
            return;
        };
        let (row, col) = (editor.row, editor.col);
        self.commit_editor(cx);

        let column_count = self.state.columns.len();
        let row_count = self.display_row_count();
        if column_count == 0 || row_count == 0 {
            return;
        }

        let (next_row, next_col) = if backwards {
            if col > 0 {
                (row, col - 1)
            } else if row > 0 {
                (row - 1, column_count - 1)
            } else {
                (row, col)
            }
        } else if col + 1 < column_count {
            (row, col + 1)
        } else if row + 1 < row_count {
            (row + 1, 0)
        } else {
            (row, col)
        };

        if (next_row, next_col) == (row, col) {
            // Already at the last cell: Tab simply closes the editor.
            cx.notify();
            return;
        }

        self.state.selection = Some(CellSelection::new(next_row, next_col));
        if next_row != row {
            self.list_scroll
                .scroll_to_item(next_row, ScrollStrategy::Nearest);
        }
        self.begin_edit((next_row, next_col), None, window, cx);
        cx.notify();
    }

    pub(super) fn cancel_editor(&mut self, cx: &mut Context<'_, Self>) {
        self.cell_editor = None;
        self.cell_editor_blur_subscription = None;
        self.date_picker = None;
        cx.notify();
    }
}
