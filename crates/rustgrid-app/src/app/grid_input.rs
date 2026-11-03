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
        let column_count = self.state.visible_columns;
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
                    // Clicking the cell that is already selected edits it (Navicat).
                    self.cell_press_reselect =
                        self.state.selection.as_ref().is_some_and(|selection| {
                            selection.ranges.len() == 1
                                && selection.active().anchor == (row, col)
                                && selection.active().cursor == (row, col)
                        });
                    self.state.selection = Some(CellSelection::new(row, col));
                    self.cell_press = Some((row, col));
                }
            }
        }
        // A Ctrl+click toggles a range and must not start a drag: the smallest pointer jitter
        // would otherwise extend the active range again and undo the cell just removed.
        self.selecting_cells = !ctrl || event.modifiers.shift;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Add a range spanning `(start_row, start_col)..=(end_row, end_col)` to the selection, or
    /// subtract it from the selection when it already contains that cell (Ctrl+click toggle).
    /// Subtracting a single cell from 全选's one big range splits the range around the cell, so
    /// Ctrl+click clears just that cell instead of dropping the whole selection.
    fn add_or_remove_range(
        &mut self,
        anchor_row: usize,
        anchor_col: usize,
        cursor_row: usize,
        cursor_col: usize,
    ) {
        let Some(selection) = self.state.selection.as_mut() else {
            self.state.selection = Some(CellSelection::single(
                (anchor_row, anchor_col),
                (cursor_row, cursor_col),
            ));
            return;
        };
        if !selection.contains(anchor_row, anchor_col) {
            selection.ranges.push(CellRange {
                anchor: (anchor_row, anchor_col),
                cursor: (cursor_row, cursor_col),
            });
            selection.active = selection.ranges.len() - 1;
            return;
        }
        selection.subtract(CellRange {
            anchor: (anchor_row, anchor_col),
            cursor: (cursor_row, cursor_col),
        });
        if selection.ranges.is_empty() {
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
        let column_count = self.state.visible_columns;
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

    /// Handle one wheel notch: move the selected cell up or down one row and scroll the view just
    /// enough to keep it visible. `delta > 0` is a scroll toward the first row. Returns `true` when
    /// the notch was consumed.
    ///
    /// The wheel moves the *selection* (rather than scrolling the view and dragging the selection
    /// along) so the gesture behaves the same whether or not the table has enough rows to overflow:
    /// the cursor can always be wheeled all the way to the first row (to copy it) or to the last.
    /// With nothing selected the first notch lands on the first row. The view follows only when the
    /// moved row would leave the viewport, so short tables do not scroll at all.
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
        let Some((row, col)) = self
            .state
            .selection
            .as_ref()
            .map(|selection| selection.active_cursor())
        else {
            // Nothing selected yet: the first notch lands on the first row so the wheel can start
            // walking a cursor through the table.
            self.state.selection = Some(CellSelection::new(0, 0));
            self.list_scroll.scroll_to_item(0, ScrollStrategy::Nearest);
            cx.notify();
            return true;
        };
        // One notch moves a single row, regardless of how many "lines" the OS reports for it.
        let steps: isize = if delta > 0.0 { -1 } else { 1 };
        let new_row = (row as isize + steps).clamp(0, rows as isize - 1) as usize;
        if new_row == row {
            // Already on the first or the last row.
            return false;
        }
        self.state.selection = Some(CellSelection::single((new_row, col), (new_row, col)));
        self.list_scroll
            .scroll_to_item(new_row, ScrollStrategy::Nearest);
        cx.notify();
        true
    }

    /// A wheel gesture is an implicit "done": dismiss the date picker (an absolute overlay anchored
    /// to a row would otherwise float over unrelated rows) and close the in-place editor so it
    /// cannot stay anchored to a row that is moving away. The editor's value is only *staged*, not
    /// flushed: scrolling must never hit the database, and a flush would reload the page and drop
    /// the selection the wheel is about to move.
    fn end_transient_editing(&mut self, cx: &mut Context<'_, Self>) {
        self.date_picker = None;
        if self.cell_editor.is_some() {
            self.commit_editor(cx);
        }
    }

    /// Pan the columns by `delta` pixels (`> 0` scrolls toward the first column), for a horizontal
    /// trackpad swipe. A vertical wheel never pans the columns — the horizontal scrollbar (or a
    /// trackpad's horizontal swipe) is the way to reach off-screen columns.
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
        let gutter = self.gutter_width();
        let row = (local_y / GRID_ROW_HEIGHT).floor() as usize;
        if row >= self.display_row_count() {
            return None;
        }
        if local_x < gutter {
            return Some(GridHit::Gutter(row));
        }
        let mut accumulated = gutter;
        for (col, width) in self
            .state
            .column_widths
            .iter()
            .take(self.state.visible_columns)
            .enumerate()
        {
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
                self.save_current(cx);
                cx.stop_propagation();
            } else if self.cell_editor.is_none()
                && self.date_picker.is_none()
                && keystroke.key.eq_ignore_ascii_case("a")
            {
                self.select_all_cells(cx);
                cx.stop_propagation();
            } else if self.cell_editor.is_none()
                && self.date_picker.is_none()
                && keystroke.key.eq_ignore_ascii_case("c")
            {
                self.copy_selection(cx);
                cx.stop_propagation();
            } else if self.cell_editor.is_none()
                && self.date_picker.is_none()
                && keystroke.key.eq_ignore_ascii_case("v")
            {
                self.paste_clipboard(cx);
                cx.stop_propagation();
            } else if self.cell_editor.is_none() && self.date_picker.is_none() {
                if keystroke.key.eq_ignore_ascii_case("z") {
                    if keystroke.modifiers.shift {
                        self.redo_edit(cx);
                    } else {
                        self.undo_edit(cx);
                    }
                } else if keystroke.key.eq_ignore_ascii_case("y") {
                    self.redo_edit(cx);
                }
            }
            return;
        }
        if self.date_picker.is_some() {
            return;
        }
        if self.cell_editor.is_some() {
            // Edit mode (Excel / DataGrip): Left / Right move the caret within the cell and stop at
            // its edges (the kit propagates them there) instead of leaving the cell. Up / Down
            // (also `MoveCellUp` / `MoveCellDown`) save the value and move the selection.
            match keystroke.key.as_str() {
                "left" | "right" => {
                    cx.stop_propagation();
                }
                "up" => {
                    self.cell_editor_move(-1, 0, window, cx);
                    cx.stop_propagation();
                }
                "down" => {
                    self.cell_editor_move(1, 0, window, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
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
                let col_count = self.state.visible_columns;
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
                // Spreadsheet behaviour: the view follows the cursor, and Shift extends the active
                // range from its anchor.
                let anchor = keystroke.modifiers.shift.then(|| {
                    self.state
                        .selection
                        .as_ref()
                        .map_or((row, col), |selection| selection.active().anchor)
                });
                self.grid_select_cell((new_row, new_col), anchor, cx);
                cx.stop_propagation();
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
        let inverse = self.apply_edit_action(action);
        self.state.redo.push(inverse);
        cx.notify();
    }

    pub(super) fn redo_edit(&mut self, cx: &mut Context<'_, Self>) {
        let Some(action) = self.state.redo.pop() else {
            return;
        };
        let inverse = self.apply_edit_action(action);
        self.state.undo.push(inverse);
        cx.notify();
    }

    /// Apply one undo/redo action to the pending edits and return the inverse action, so the
    /// opposite direction can replay it.
    fn apply_edit_action(&mut self, action: EditAction) -> EditAction {
        let mut inverse: EditAction = Vec::with_capacity(action.len());
        for ((row, col), previous) in action {
            let current = self.state.edits.get(&(row, col)).cloned();
            inverse.push(((row, col), current));
            match previous {
                Some(value) => {
                    self.state.edits.insert((row, col), value);
                }
                None => {
                    self.state.edits.remove(&(row, col));
                }
            }
        }
        inverse
    }

    /// Select every displayed cell (Ctrl+A), like Navicat's "select all records".
    pub(super) fn select_all_cells(&mut self, cx: &mut Context<'_, Self>) {
        let rows = self.display_row_count();
        let cols = self.state.visible_columns;
        if rows == 0 || cols == 0 {
            return;
        }
        self.state.selection = Some(CellSelection::single((0, 0), (rows - 1, cols - 1)));
        self.cell_press = None;
        self.selecting_cells = false;
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
            if col >= self.state.visible_columns {
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
                && self
                    .state
                    .rows
                    .get(row)
                    .and_then(|values| values.get(self.state.source_col(col)))
                    .is_some_and(|cell| matches!(cell, CellValue::Null))
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
            self.state.redo.clear();
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
            || col >= self.state.visible_columns
        {
            return;
        }
        let value = self
            .staged_value(row, col)
            .cloned()
            .flatten()
            .unwrap_or_else(|| {
                if row < self.state.rows.len() {
                    self.state.rows[row]
                        .get(self.state.source_col(col))
                        .map(|cell| cell.as_edit_string())
                        .unwrap_or_default()
                } else {
                    String::new()
                }
            });
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
        // A temporal cell's picker opens only from its "…" button (see `toggle_date_picker`), not
        // automatically when the editor appears.
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
        // Auto-commit only when this single-cell edit is the *only* pending change. If other
        // edits/inserts are staged, committing here would flush (and reload away) the batch the
        // user is still assembling, so it waits for the commit button instead.
        if !multi && only_data_rows && self.inserts.is_empty() && self.state.edits.len() == 1 {
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
            if col >= self.state.visible_columns {
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
                None => match self
                    .state
                    .rows
                    .get(row)
                    .and_then(|values| values.get(self.state.source_col(col)))
                {
                    Some(CellValue::Null) | None => None,
                    Some(cell) => Some(cell.as_edit_string()),
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
            self.state.redo.clear();
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

        let column_count = self.state.visible_columns;
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

        // Save and move the selection (no new editor, like Excel); focus returns to the grid so
        // the keys keep working.
        self.grid_select_cell((next_row, next_col), None, cx);
        window.focus(&self.focus, cx);
    }

    /// Arrow in a cell editor: stage the current value and move the editor to the adjacent cell
    /// (`d_row`/`d_col` are a single step). Left/right reach here only when the caret is already at
    /// the text edge, so ordinary caret movement is preserved; up/down always move vertically.
    pub(super) fn cell_editor_move(
        &mut self,
        d_row: isize,
        d_col: isize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(editor) = self.cell_editor.as_ref() else {
            return;
        };
        let (row, col) = (editor.row, editor.col);
        self.commit_editor(cx);

        let column_count = self.state.visible_columns;
        let row_count = self.display_row_count();
        if column_count == 0 || row_count == 0 {
            return;
        }
        let next_row = (row as isize + d_row).clamp(0, row_count as isize - 1) as usize;
        let next_col = (col as isize + d_col).clamp(0, column_count as isize - 1) as usize;
        // Save and move the selection (no new editor); at the grid edge it stays put. Either way
        // focus returns to the grid, so the keys keep working.
        self.grid_select_cell((next_row, next_col), None, cx);
        window.focus(&self.focus, cx);
    }

    pub(super) fn cancel_editor(&mut self, cx: &mut Context<'_, Self>) {
        self.cell_editor = None;
        self.cell_editor_blur_subscription = None;
        self.date_picker = None;
        cx.notify();
    }

    /// Stage a value typed into the info pane's row form. The rules match the in-place cell
    /// editor: an empty field on a `NULL` cell stages nothing, otherwise it becomes an empty
    /// string; a value that matches the page's own value drops the staged edit instead of keeping
    /// a no-op. The edit is keyed by display column, so it shows up in the grid cell too.
    pub(super) fn stage_form_edit(
        &mut self,
        row: usize,
        col: usize,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.state.editable || row >= self.state.rows.len() || col >= self.state.visible_columns
        {
            return;
        }
        let previous = self.state.edits.get(&(row, col)).cloned();
        let original = match self
            .state
            .rows
            .get(row)
            .and_then(|values| values.get(self.state.source_col(col)))
        {
            Some(CellValue::Null) | None => None,
            Some(cell) => Some(cell.as_edit_string()),
        };
        let desired = if text.is_empty() && original.is_none() {
            None
        } else {
            Some(text.to_string())
        };
        if original == desired {
            if previous.is_some() {
                self.state.edits.remove(&(row, col));
                cx.notify();
            }
            return;
        }
        if previous == Some(desired.clone()) {
            return;
        }
        self.push_form_undo(row, col, previous);
        self.state.edits.insert((row, col), desired);
        self.state.redo.clear();
        cx.notify();
    }

    /// Record the first edit of a continuous run on one cell, so one Ctrl+Z restores the value
    /// the run started from instead of stepping through every keystroke.
    fn push_form_undo(&mut self, row: usize, col: usize, previous: Option<Option<String>>) {
        if let Some(last) = self.state.undo.last()
            && last.len() == 1
            && last[0].0 == (row, col)
        {
            return;
        }
        self.state.undo.push(vec![((row, col), previous)]);
        if self.state.undo.len() > 256 {
            self.state.undo.remove(0);
        }
    }

    /// Drop one field's staged value (Escape in the row form), leaving every other pending edit
    /// alone.
    pub(super) fn cancel_form_edit(&mut self, row: usize, col: usize, cx: &mut Context<'_, Self>) {
        if self.state.edits.remove(&(row, col)).is_some() {
            cx.notify();
        }
    }

    /// Enter in the row form: write the pending edits. Nothing pending (the user just opened a
    /// form and pressed Enter) saves nothing and shows no toast.
    pub(super) fn submit_form(&mut self, cx: &mut Context<'_, Self>) {
        if !self.state.edits.is_empty() || !self.inserts.is_empty() {
            self.save_grid(cx);
        }
    }
}

/// A table-grid keyboard jump (keymap commands; the arrow keys stay on `grid_key`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GridJump {
    PageUp,
    PageDown,
    RowStart,
    RowEnd,
    First,
    Last,
    Next,
    Prev,
}

/// Where `jump` takes the cursor at `(row, col)` in a `rows` x `cols` grid, paging by `page` rows.
/// Next / Prev walk the cells row by row (Tab / Shift+Tab, wrapping onto the neighbouring row) and
/// stop at the first / last cell.
fn grid_jump_target(
    (row, col): (usize, usize),
    rows: usize,
    cols: usize,
    jump: GridJump,
    page: usize,
) -> (usize, usize) {
    let (last_row, last_col) = (rows.saturating_sub(1), cols.saturating_sub(1));
    match jump {
        GridJump::PageUp => (row.saturating_sub(page.max(1)), col),
        GridJump::PageDown => ((row + page.max(1)).min(last_row), col),
        GridJump::RowStart => (row, 0),
        GridJump::RowEnd => (row, last_col),
        GridJump::First => (0, 0),
        GridJump::Last => (last_row, last_col),
        GridJump::Next if col < last_col => (row, col + 1),
        GridJump::Next if row < last_row => (row + 1, 0),
        GridJump::Prev if col > 0 => (row, col - 1),
        GridJump::Prev if row > 0 => (row - 1, last_col),
        GridJump::Next | GridJump::Prev => (row, col),
    }
}

impl GridView {
    /// Move the cursor for a keyboard jump (`extend`: Shift, grow the active range instead). Not
    /// while a cell editor or the date picker is open: those own the keys.
    pub(super) fn grid_jump(&mut self, jump: GridJump, extend: bool, cx: &mut Context<'_, Self>) {
        if self.cell_editor.is_some() || self.date_picker.is_some() {
            return;
        }
        let rows = self.display_row_count();
        let cols = self.state.visible_columns;
        if rows == 0 || cols == 0 {
            return;
        }
        let (cursor, anchor) = match self.state.selection.as_ref() {
            Some(selection) => (selection.active_cursor(), selection.active().anchor),
            None => ((0, 0), (0, 0)),
        };
        let target = grid_jump_target(cursor, rows, cols, jump, self.grid_page_rows());
        self.grid_select_cell(target, extend.then_some(anchor), cx);
    }

    /// Select `cell` (extending from `anchor` when given) and scroll it into view.
    pub(super) fn grid_select_cell(
        &mut self,
        (row, col): (usize, usize),
        anchor: Option<(usize, usize)>,
        cx: &mut Context<'_, Self>,
    ) {
        self.state.selection = Some(CellSelection::single(
            anchor.unwrap_or((row, col)),
            (row, col),
        ));
        self.list_scroll
            .scroll_to_item(row, ScrollStrategy::Nearest);
        self.reveal_column(col);
        cx.notify();
    }

    /// How many rows one Page Up / Page Down moves: a screenful less one row of context.
    fn grid_page_rows(&self) -> usize {
        let height = f32::from(self.hscroll.bounds().size.height);
        ((height / GRID_ROW_HEIGHT).floor() as usize)
            .saturating_sub(2)
            .max(1)
    }

    /// Scroll horizontally so column `col` is fully visible.
    fn reveal_column(&self, col: usize) {
        let widths = &self.state.column_widths;
        let gutter = self.gutter_width();
        let left = gutter + widths.iter().take(col).sum::<f32>();
        let right = left + widths.get(col).copied().unwrap_or(0.0);
        let viewport = f32::from(self.hscroll.bounds().size.width);
        if viewport <= 0.0 {
            return;
        }
        let shown = -f32::from(self.hscroll.offset().x);
        let start = if left - gutter < shown {
            left - gutter
        } else if right > shown + viewport {
            right - viewport
        } else {
            return;
        };
        let max = f32::from(self.hscroll.max_offset().x);
        let mut offset = self.hscroll.offset();
        offset.x = px(-start.clamp(0.0, max.max(0.0)));
        self.hscroll.set_offset(offset);
    }
}

#[cfg(test)]
mod jump_tests {
    use super::{GridJump, grid_jump_target};

    #[test]
    fn paging_and_edges() {
        assert_eq!(
            grid_jump_target((5, 2), 100, 4, GridJump::PageDown, 20),
            (25, 2)
        );
        assert_eq!(
            grid_jump_target((95, 2), 100, 4, GridJump::PageDown, 20),
            (99, 2)
        );
        assert_eq!(
            grid_jump_target((5, 2), 100, 4, GridJump::PageUp, 20),
            (0, 2)
        );
        assert_eq!(
            grid_jump_target((5, 2), 100, 4, GridJump::RowStart, 20),
            (5, 0)
        );
        assert_eq!(
            grid_jump_target((5, 2), 100, 4, GridJump::RowEnd, 20),
            (5, 3)
        );
        assert_eq!(
            grid_jump_target((5, 2), 100, 4, GridJump::First, 20),
            (0, 0)
        );
        assert_eq!(
            grid_jump_target((5, 2), 100, 4, GridJump::Last, 20),
            (99, 3)
        );
    }

    #[test]
    fn tab_wraps_across_rows_and_stops_at_the_ends() {
        assert_eq!(grid_jump_target((0, 3), 2, 4, GridJump::Next, 1), (1, 0));
        assert_eq!(grid_jump_target((1, 3), 2, 4, GridJump::Next, 1), (1, 3));
        assert_eq!(grid_jump_target((1, 0), 2, 4, GridJump::Prev, 1), (0, 3));
        assert_eq!(grid_jump_target((0, 0), 2, 4, GridJump::Prev, 1), (0, 0));
    }
}
