use super::*;

impl AppView {
    pub(super) fn active_grid_id(&self, cx: &App) -> Option<u64> {
        self.active_grid
            .and_then(|index| self.grids.get(index))
            .map(|grid| grid.read(cx).state.id)
    }

    pub(super) fn close_grid(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.grids.len() {
            return;
        }
        self.grids.remove(index);
        match self.active_grid {
            Some(active) if active == index => {
                self.active_grid = if self.grids.is_empty() {
                    None
                } else {
                    Some(index.min(self.grids.len() - 1))
                };
            }
            Some(active) if active > index => self.active_grid = Some(active - 1),
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn close_connection_grids(&mut self, connection: &Arc<dyn Connection>, cx: &App) {
        let active_id = self.active_grid_id(cx);
        self.grids
            .retain(|grid| !Arc::ptr_eq(&grid.read(cx).state.connection, connection));
        self.active_grid = active_id.and_then(|id| {
            self.grids
                .iter()
                .position(|grid| grid.read(cx).state.id == id)
        });
    }

    pub(super) fn cancel_delete(&mut self, cx: &mut Context<'_, Self>) {
        self.delete_confirm = None;
        cx.notify();
    }

    pub(super) fn confirm_delete(&mut self, cx: &mut Context<'_, Self>) {
        let Some(confirm) = self.delete_confirm.take() else {
            return;
        };
        match confirm {
            DeleteConfirm::Rows { grid_id, rows } => {
                let target = self
                    .grids
                    .iter()
                    .find(|grid| grid.read(cx).state.id == grid_id)
                    .cloned();
                if let Some(grid) = target {
                    grid.update(cx, |grid, cx| grid.delete_rows(rows, cx));
                }
            }
            DeleteConfirm::Connection { index } => self.delete_connection(index, cx),
        }
        cx.notify();
    }
}

impl GridView {
    pub(super) fn load_page(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
            self.reload_query(cx);
            return;
        }

        self.state.loading = true;
        self.state.error = None;
        self.state.selection = None;
        self.state.edits.clear();

        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let page_index = self.state.page_index;
        let page_size = self.state.page_size;
        let order_by = self.state.sort_columns();
        let filter = self.state.filter_conditions();

        self.selecting_cells = false;
        self.cell_editor = None;
        self.date_picker = None;
        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    connection
                        .fetch_page(
                            &database,
                            &table,
                            PageRequest::new(page_index, page_size)
                                .with_order_by(order_by)
                                .with_filter(filter),
                        )
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |grid, cx| {
                grid.state.loading = false;
                match result {
                    Ok(page) => {
                        grid.state.column_widths = compute_column_widths(&page.columns, &page.rows);
                        grid.state.columns = page.columns;
                        grid.state.rows = Arc::new(page.rows);
                        grid.state.total_rows = page.total_rows;
                        grid.state.error = None;
                        grid.list_scroll = UniformListScrollHandle::new();
                        grid.hscroll.set_offset(Point::default());
                    }
                    Err(error) => {
                        grid.state.error = Some(error.to_string());
                    }
                }
                grid.sync_page_input();
                cx.notify();
            });
        })
        .detach();
    }

    fn reload_query(&mut self, cx: &mut Context<'_, Self>) {
        let Some(sql) = self.state.sql.clone() else {
            return;
        };
        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        self.state.loading = true;
        self.state.error = None;
        self.state.selection = None;
        self.state.edits.clear();
        self.cell_editor = None;
        self.date_picker = None;
        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let query_database = (!database.is_empty()).then(|| database.clone());
            let result = match runtime
                .spawn(async move {
                    connection
                        .execute_query(query_database.as_deref(), &sql)
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |grid, cx| {
                grid.state.loading = false;
                match result {
                    Ok(query_result) => {
                        let total = query_result.rows.len() as u64;
                        grid.state.column_widths =
                            compute_column_widths(&query_result.columns, &query_result.rows);
                        grid.state.columns = query_result.columns;
                        grid.state.rows = Arc::new(query_result.rows);
                        grid.state.total_rows = Some(total);
                        grid.state.page_index = 0;
                        grid.state.page_size = total.max(1);
                        grid.state.error = None;
                        grid.list_scroll = UniformListScrollHandle::new();
                        grid.hscroll.set_offset(Point::default());
                    }
                    Err(error) => grid.state.error = Some(error.to_string()),
                }
                grid.sync_page_input();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn next_page(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.has_next() {
            self.state.page_index += 1;
        }
        self.sync_page_input();
        self.load_page(cx);
    }

    pub(super) fn prev_page(&mut self, cx: &mut Context<'_, Self>) {
        self.state.page_index = self.state.page_index.saturating_sub(1);
        self.sync_page_input();
        self.load_page(cx);
    }

    pub(super) fn first_page(&mut self, cx: &mut Context<'_, Self>) {
        self.state.page_index = 0;
        self.sync_page_input();
        self.load_page(cx);
    }

    pub(super) fn last_page(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(last) = self.state.last_page() {
            self.state.page_index = last;
        }
        self.sync_page_input();
        self.load_page(cx);
    }

    pub(super) fn sync_page_input(&mut self) {
        self.page_input = (self.state.page_index + 1).to_string();
    }

    pub(super) fn commit_page_input(&mut self, cx: &mut Context<'_, Self>) {
        let requested = self.page_input.trim().parse::<u64>().unwrap_or(1).max(1);
        let target = match self.state.last_page() {
            Some(last) => (requested - 1).min(last),
            None => requested - 1,
        };
        self.state.page_index = target;
        self.sync_page_input();
        self.load_page(cx);
    }

    pub(super) fn page_input_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }

        match keystroke.key.as_str() {
            "backspace" => {
                self.page_input.pop();
            }
            "enter" => {
                self.commit_page_input(cx);
                return;
            }
            "escape" => {
                self.sync_page_input();
            }
            _ => {
                // Digits (and IME) arrive through the grid's input handler, which filters to
                // ASCII digits for the page field.
            }
        }
        cx.notify();
    }

    pub(super) fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        self.load_page(cx);
    }

    pub(super) fn toggle_sort_panel(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
            return;
        }
        if self.state.sort_open {
            self.state.sort_open = false;
            self.state.sort_combo = None;
            self.state.sort_selected = None;
            self.state.sort_draft.clear();
        } else {
            self.state.sort_open = true;
            self.state.sort_draft = self.state.sort_rules.clone();
            self.state.sort_selected = (!self.state.sort_draft.is_empty()).then_some(0);
            self.state.sort_combo = None;
        }
        cx.notify();
    }

    /// Track which header cell the pointer is over so only that column reveals its sort badge.
    pub(super) fn set_sort_hover(
        &mut self,
        index: usize,
        hovered: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let next = if hovered {
            Some(index)
        } else if self.sort_hover == Some(index) {
            None
        } else {
            self.sort_hover
        };
        if next != self.sort_hover {
            self.sort_hover = next;
            cx.notify();
        }
    }

    pub(super) fn sort_add_rule(&mut self, cx: &mut Context<'_, Self>) {
        let column = self
            .state
            .columns
            .iter()
            .map(|column| column.name.clone())
            .find(|name| {
                self.state
                    .sort_draft
                    .iter()
                    .all(|rule| &rule.column != name)
            })
            .or_else(|| self.state.columns.first().map(|column| column.name.clone()));
        let Some(column) = column else {
            return;
        };
        let position = self.state.sort_draft.len();
        self.state.sort_draft.push(SortRule::new(column));
        self.state.sort_selected = Some(position);
        self.state.sort_combo = None;
        cx.notify();
    }

    pub(super) fn sort_select_rule(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.state.sort_selected = Some(index);
        self.state.sort_combo = None;
        cx.notify();
    }

    pub(super) fn sort_toggle_enabled(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(rule) = self.state.sort_draft.get_mut(index) {
            rule.enabled = !rule.enabled;
        }
        cx.notify();
    }

    pub(super) fn sort_toggle_direction(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(rule) = self.state.sort_draft.get_mut(index) {
            rule.descending = !rule.descending;
        }
        cx.notify();
    }

    pub(super) fn sort_open_combo(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let current = self
            .state
            .sort_draft
            .get(index)
            .map(|rule| rule.column.clone())
            .unwrap_or_default();
        self.state.sort_selected = Some(index);
        if self
            .state
            .sort_combo
            .as_ref()
            .is_some_and(|(open, _)| *open == index)
        {
            self.state.sort_combo = None;
        } else {
            let highlight = self
                .state
                .columns
                .iter()
                .position(|column| column.name == current)
                .unwrap_or(0);
            self.state.sort_combo = Some((index, current));
            self.sort_combo_filter.clear();
            self.sort_combo_highlight = highlight;
            self.sort_combo_focus_pending = true;
        }
        cx.notify();
    }

    /// The columns visible in the open popup after applying the type-ahead filter.
    pub(super) fn sort_combo_matches(&self) -> Vec<String> {
        let filter = self.sort_combo_filter.to_lowercase();
        self.state
            .columns
            .iter()
            .filter(|column| filter.is_empty() || column.name.to_lowercase().contains(&filter))
            .map(|column| column.name.clone())
            .collect()
    }

    /// Commit a column choice made by clicking or pressing Enter; the popup closes immediately.
    pub(super) fn sort_choose_column(
        &mut self,
        index: usize,
        column: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(rule) = self.state.sort_draft.get_mut(index) {
            rule.column = column;
        }
        self.state.sort_combo = None;
        self.sort_combo_filter.clear();
        cx.notify();
    }

    pub(super) fn sort_combo_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        if self.state.sort_combo.is_none() {
            return;
        }
        let matches = self.sort_combo_matches();
        let count = matches.len();

        match event.keystroke.key.as_str() {
            "up" => {
                if count > 0 {
                    self.sort_combo_highlight = self.sort_combo_highlight.saturating_sub(1);
                }
            }
            "down" => {
                if count > 0 {
                    self.sort_combo_highlight = (self.sort_combo_highlight + 1) % count;
                }
            }
            "enter" => {
                if let Some(column) = matches.get(self.sort_combo_highlight).cloned() {
                    let index = self.state.sort_combo.as_ref().map(|(i, _)| *i).unwrap_or(0);
                    self.sort_choose_column(index, column, cx);
                    return;
                }
            }
            "escape" => {
                self.sort_cancel_combo(cx);
                return;
            }
            "backspace" => {
                self.sort_combo_filter.pop();
                self.sort_combo_highlight = 0;
            }
            _ => {
                // Text characters (including IME) arrive through the grid's input handler.
            }
        }
        self.sort_combo_highlight = self.sort_combo_highlight.min(count.saturating_sub(1));
        cx.notify();
    }

    pub(super) fn sort_confirm_combo(&mut self, cx: &mut Context<'_, Self>) {
        let matches = self.sort_combo_matches();
        if let Some(column) = matches.get(self.sort_combo_highlight).cloned() {
            let index = self.state.sort_combo.as_ref().map(|(i, _)| *i).unwrap_or(0);
            self.sort_choose_column(index, column, cx);
            return;
        }
        self.sort_cancel_combo(cx);
    }

    pub(super) fn sort_cancel_combo(&mut self, cx: &mut Context<'_, Self>) {
        self.state.sort_combo = None;
        self.sort_combo_filter.clear();
        cx.notify();
    }

    pub(super) fn sort_remove_rule(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index < self.state.sort_draft.len() {
            self.state.sort_draft.remove(index);
        }
        self.state.sort_combo = None;
        let len = self.state.sort_draft.len();
        self.state.sort_selected = match self.state.sort_selected {
            Some(selected) if selected >= len => len.checked_sub(1),
            Some(selected) if selected > index => Some(selected - 1),
            other => other,
        };
        cx.notify();
    }

    pub(super) fn sort_move_rule(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        let len = self.state.sort_draft.len();
        if len > 1
            && let Some(selected) = self.state.sort_selected
        {
            let target = (selected as isize + delta).clamp(0, len as isize - 1) as usize;
            if target != selected {
                self.state.sort_draft.swap(selected, target);
                self.state.sort_selected = Some(target);
            }
        }
        cx.notify();
    }

    pub(super) fn sort_apply(&mut self, cx: &mut Context<'_, Self>) {
        self.state.sort_rules = self.state.sort_draft.clone();
        self.state.sort_combo = None;
        self.state.page_index = 0;
        self.sync_page_input();
        self.load_page(cx);
    }

    /// The grid header's sort badge. Header sorting is single-column: clicking a column toggles
    /// its direction and drops every other criterion (the panel keeps just this one).
    pub(super) fn toggle_column_sort(&mut self, column: String, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
            return;
        }
        let descending = self
            .state
            .sort_rules
            .iter()
            .find(|rule| rule.column == column)
            .map(|rule| !rule.descending)
            .unwrap_or(false);
        let mut rule = SortRule::new(column);
        rule.descending = descending;
        self.state.sort_rules = vec![rule];
        if self.state.sort_open {
            self.state.sort_draft = self.state.sort_rules.clone();
            self.state.sort_selected = Some(0);
        }
        self.state.page_index = 0;
        self.sync_page_input();
        self.load_page(cx);
    }

    pub(super) fn toggle_filter_panel(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.filter_open {
            self.state.filter_open = false;
            self.state.filter_combo = None;
            self.filter_active = None;
            self.state.filter_draft.clear();
            self.filter_value_focus.clear();
            self.filter_value2_focus.clear();
        } else {
            if self.state.sql.is_some() {
                return;
            }
            self.state.filter_open = true;
            self.state.filter_combo = None;
            self.filter_active = None;
            self.state.filter_draft = self.state.filters.clone();
            if self.state.filter_draft.is_empty()
                && let Some(column) = self.state.columns.first().map(|column| column.name.clone())
            {
                self.state.filter_draft.push(FilterCondition::new(column));
            }
            self.rebuild_filter_focus(cx);
        }
        cx.notify();
    }

    fn rebuild_filter_focus(&mut self, cx: &mut Context<'_, Self>) {
        self.filter_value_focus = self
            .state
            .filter_draft
            .iter()
            .map(|_| cx.focus_handle())
            .collect();
        self.filter_value2_focus = self
            .state
            .filter_draft
            .iter()
            .map(|_| cx.focus_handle())
            .collect();
    }

    fn next_filter_column(&self) -> String {
        self.state
            .columns
            .iter()
            .map(|column| column.name.clone())
            .find(|name| {
                self.state
                    .filter_draft
                    .iter()
                    .all(|condition| &condition.column != name)
            })
            .or_else(|| self.state.columns.first().map(|column| column.name.clone()))
            .unwrap_or_default()
    }

    pub(super) fn filter_add_rule(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let column = self.next_filter_column();
        if column.is_empty() {
            return;
        }
        let position = (index + 1).min(self.state.filter_draft.len());
        self.state
            .filter_draft
            .insert(position, FilterCondition::new(column));
        self.filter_value_focus.insert(position, cx.focus_handle());
        self.filter_value2_focus.insert(position, cx.focus_handle());
        self.state.filter_combo = None;
        cx.notify();
    }

    pub(super) fn filter_remove_rule(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index < self.state.filter_draft.len() {
            self.state.filter_draft.remove(index);
        }
        if index < self.filter_value_focus.len() {
            self.filter_value_focus.remove(index);
            self.filter_value2_focus.remove(index);
        }
        if matches!(self.filter_active, Some((row, _)) if row == index) {
            self.filter_active = None;
        } else if let Some((row, slot)) = self.filter_active
            && row > index
        {
            self.filter_active = Some((row - 1, slot));
        }
        self.state.filter_combo = None;
        cx.notify();
    }

    pub(super) fn filter_toggle_enabled(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(condition) = self.state.filter_draft.get_mut(index) {
            condition.enabled = !condition.enabled;
        }
        cx.notify();
    }

    pub(super) fn filter_toggle_conjunction(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(condition) = self.state.filter_draft.get_mut(index) {
            condition.conjunction = condition.conjunction.toggled();
        }
        cx.notify();
    }

    pub(super) fn filter_open_combo(
        &mut self,
        index: usize,
        kind: FilterCombo,
        cx: &mut Context<'_, Self>,
    ) {
        self.state.filter_combo = if self.state.filter_combo == Some((index, kind)) {
            None
        } else {
            Some((index, kind))
        };
        self.filter_active = None;
        cx.notify();
    }

    pub(super) fn filter_choose_field(
        &mut self,
        index: usize,
        column: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(condition) = self.state.filter_draft.get_mut(index) {
            condition.column = column;
        }
        self.state.filter_combo = None;
        cx.notify();
    }

    pub(super) fn filter_choose_operator(
        &mut self,
        index: usize,
        operator: FilterOperator,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(condition) = self.state.filter_draft.get_mut(index) {
            condition.operator = operator;
        }
        self.state.filter_combo = None;
        cx.notify();
    }

    pub(super) fn filter_focus_value(
        &mut self,
        index: usize,
        slot: u8,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.state.filter_combo = None;
        self.filter_active = Some((index, slot));
        let handle = if slot == 0 {
            self.filter_value_focus.get(index).cloned()
        } else {
            self.filter_value2_focus.get(index).cloned()
        };
        if let Some(handle) = handle {
            window.focus(&handle);
        }
        cx.notify();
    }

    pub(super) fn filter_value_key(
        &mut self,
        index: usize,
        slot: u8,
        event: &KeyDownEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }
        let mut apply = false;
        let mut unfocus = false;
        if let Some(condition) = self.state.filter_draft.get_mut(index) {
            let buffer = if slot == 0 {
                &mut condition.value
            } else {
                &mut condition.value2
            };
            match keystroke.key.as_str() {
                "backspace" => {
                    buffer.pop();
                }
                "escape" => unfocus = true,
                "enter" => apply = true,
                _ => {
                    // Text characters (including IME) arrive through the grid's input handler.
                }
            }
        }
        if unfocus {
            self.filter_active = None;
        }
        if apply {
            self.filter_apply(cx);
            return;
        }
        cx.notify();
    }

    pub(super) fn filter_apply(&mut self, cx: &mut Context<'_, Self>) {
        self.state.filters = self.state.filter_draft.clone();
        self.state.filter_combo = None;
        self.filter_active = None;
        self.state.page_index = 0;
        self.sync_page_input();
        self.load_page(cx);
    }

    pub(super) fn toggle_page_size_menu(&mut self, cx: &mut Context<'_, Self>) {
        if self.page_size_menu_open {
            self.page_size_menu_open = false;
            cx.notify();
            return;
        }
        if self.state.sql.is_some() {
            return;
        }
        self.page_size_input = self.state.page_size.to_string();
        self.page_size_menu_open = true;
        self.page_size_focus_pending = true;
        cx.notify();
    }

    pub(super) fn toggle_limit_records(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.limit_records = !app.limit_records;
                cx.notify();
            });
        }
    }

    pub(super) fn page_size_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }
        match keystroke.key.as_str() {
            "backspace" => {
                self.page_size_input.pop();
            }
            "enter" => {
                self.page_size_apply(cx);
                return;
            }
            "escape" => {
                self.page_size_input = self.state.page_size.to_string();
            }
            "up" | "down" => {
                let current = self.page_size_input.parse::<u64>().unwrap_or(1000);
                let next = if keystroke.key == "up" {
                    current.saturating_add(100)
                } else {
                    current.saturating_sub(100).max(1)
                };
                self.page_size_input = next.to_string();
            }
            _ => {
                // Digits (and IME) arrive through the grid's input handler, which filters to
                // ASCII digits for the page-size field.
            }
        }
        cx.notify();
    }

    pub(super) fn page_size_apply(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
            return;
        }
        let requested = self.page_size_input.trim().parse::<u64>().unwrap_or(1000);
        let size = if self.limit_records(cx) {
            requested.clamp(1, NO_LIMIT_PAGE_SIZE)
        } else {
            NO_LIMIT_PAGE_SIZE
        };
        self.state.page_size = size;
        self.state.page_index = 0;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                app.page_size = size;
                cx.notify();
            });
        }
        self.page_size_menu_open = false;
        self.sync_page_input();
        self.load_page(cx);
    }
}
