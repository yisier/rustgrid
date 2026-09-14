use super::*;

impl AppView {
    pub(super) fn active_grid_id(&self) -> Option<u64> {
        self.active_grid
            .and_then(|index| self.grids.get(index))
            .map(|grid| grid.id)
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
        self.sync_page_input();
        cx.notify();
    }

    pub(super) fn close_connection_grids(&mut self, connection: &Arc<dyn Connection>) {
        let active_id = self.active_grid_id();
        self.grids
            .retain(|grid| !Arc::ptr_eq(&grid.connection, connection));
        self.active_grid =
            active_id.and_then(|id| self.grids.iter().position(|grid| grid.id == id));
        self.sync_page_input();
    }

    pub(super) fn load_page(&mut self, id: u64, cx: &mut Context<'_, Self>) {
        if self
            .grids
            .iter()
            .find(|grid| grid.id == id)
            .is_some_and(|grid| grid.sql.is_some())
        {
            self.reload_query_grid(id, cx);
            return;
        }

        let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id) else {
            return;
        };

        grid.loading = true;
        grid.error = None;
        grid.selection = None;
        grid.edits.clear();

        let connection = grid.connection.clone();
        let database = grid.database.clone();
        let table = grid.table.clone();
        let page_index = grid.page_index;
        let page_size = grid.page_size;
        let order_by = grid.sort_columns();

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
                            PageRequest::new(page_index, page_size).with_order_by(order_by),
                        )
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(grid) = view.grids.iter_mut().find(|grid| grid.id == id) {
                    grid.loading = false;
                    match result {
                        Ok(page) => {
                            grid.column_widths = compute_column_widths(&page.columns, &page.rows);
                            grid.columns = page.columns;
                            grid.rows = Arc::new(page.rows);
                            grid.total_rows = page.total_rows;
                            grid.error = None;
                            view.grid_list_scroll = UniformListScrollHandle::new();
                            view.grid_hscroll.set_offset(Point::default());
                        }
                        Err(error) => {
                            grid.error = Some(error.to_string());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn reload_query_grid(&mut self, id: u64, cx: &mut Context<'_, Self>) {
        let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id) else {
            return;
        };
        let Some(sql) = grid.sql.clone() else {
            return;
        };
        let connection = grid.connection.clone();
        let database = grid.database.clone();
        grid.loading = true;
        grid.error = None;
        grid.selection = None;
        grid.edits.clear();
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

            let _ = this.update(cx, |view, cx| {
                if let Some(grid) = view.grids.iter_mut().find(|grid| grid.id == id) {
                    grid.loading = false;
                    match result {
                        Ok(query_result) => {
                            let total = query_result.rows.len() as u64;
                            grid.column_widths =
                                compute_column_widths(&query_result.columns, &query_result.rows);
                            grid.columns = query_result.columns;
                            grid.rows = Arc::new(query_result.rows);
                            grid.total_rows = Some(total);
                            grid.page_index = 0;
                            grid.page_size = total.max(1);
                            grid.error = None;
                            view.query_result_scroll = UniformListScrollHandle::new();
                            view.grid_hscroll.set_offset(Point::default());
                        }
                        Err(error) => grid.error = Some(error.to_string()),
                    }
                }
                view.sync_page_input();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn next_page(&mut self, cx: &mut Context<'_, Self>) {
        let Some(id) = self.active_grid_id() else {
            return;
        };
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id)
            && grid.has_next()
        {
            grid.page_index += 1;
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }

    pub(super) fn prev_page(&mut self, cx: &mut Context<'_, Self>) {
        let Some(id) = self.active_grid_id() else {
            return;
        };
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id) {
            grid.page_index = grid.page_index.saturating_sub(1);
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }

    pub(super) fn first_page(&mut self, cx: &mut Context<'_, Self>) {
        let Some(id) = self.active_grid_id() else {
            return;
        };
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id) {
            grid.page_index = 0;
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }

    pub(super) fn last_page(&mut self, cx: &mut Context<'_, Self>) {
        let Some(id) = self.active_grid_id() else {
            return;
        };
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id)
            && let Some(last) = grid.last_page()
        {
            grid.page_index = last;
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }

    pub(super) fn sync_page_input(&mut self) {
        let page = self
            .active_grid
            .and_then(|index| self.grids.get(index))
            .map(|grid| grid.page_index + 1)
            .unwrap_or(1);
        self.page_input = page.to_string();
    }

    pub(super) fn commit_page_input(&mut self, cx: &mut Context<'_, Self>) {
        let requested = self.page_input.trim().parse::<u64>().unwrap_or(1).max(1);
        let Some(id) = self.active_grid_id() else {
            return;
        };
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id) {
            let target = match grid.last_page() {
                Some(last) => (requested - 1).min(last),
                None => requested - 1,
            };
            grid.page_index = target;
        }
        self.sync_page_input();
        self.load_page(id, cx);
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
                if let Some(text) = keystroke.key_char.as_ref()
                    && text.chars().all(|character| character.is_ascii_digit())
                {
                    self.page_input.push_str(text);
                }
            }
        }
        cx.notify();
    }

    pub(super) fn object_search_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }

        match keystroke.key.as_str() {
            "backspace" => {
                self.object_search.pop();
            }
            "escape" => {
                self.object_search.clear();
            }
            "enter" => {}
            _ => {
                if let Some(text) = keystroke.key_char.as_ref()
                    && !text.chars().any(char::is_control)
                {
                    self.object_search.push_str(text);
                }
            }
        }
        self.notify_object_pane(cx);
        cx.notify();
    }

    pub(super) fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(id) = self.active_grid_id() {
            self.load_page(id, cx);
        }
    }

    pub(super) fn toggle_sort_panel(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_grid else {
            return;
        };
        let Some(grid) = self.grids.get_mut(index) else {
            return;
        };
        if grid.sql.is_some() {
            return;
        }
        if grid.sort_open {
            grid.sort_open = false;
            grid.sort_combo = None;
            grid.sort_selected = None;
            grid.sort_draft.clear();
        } else {
            grid.sort_open = true;
            grid.sort_draft = grid.sort_rules.clone();
            grid.sort_selected = (!grid.sort_draft.is_empty()).then_some(0);
            grid.sort_combo = None;
        }
        cx.notify();
    }

    /// Track which header cell the pointer is over so only that column reveals its sort badge.
    pub(super) fn set_sort_hover(
        &mut self,
        grid_id: u64,
        index: usize,
        hovered: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let next = if hovered {
            Some((grid_id, index))
        } else if self.sort_hover == Some((grid_id, index)) {
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
        let Some(index) = self.active_grid else {
            return;
        };
        let Some(grid) = self.grids.get_mut(index) else {
            return;
        };
        let column = grid
            .columns
            .iter()
            .map(|column| column.name.clone())
            .find(|name| grid.sort_draft.iter().all(|rule| &rule.column != name))
            .or_else(|| grid.columns.first().map(|column| column.name.clone()));
        let Some(column) = column else {
            return;
        };
        let position = grid.sort_draft.len();
        grid.sort_draft.push(SortRule::new(column));
        grid.sort_selected = Some(position);
        grid.sort_combo = None;
        cx.notify();
    }

    pub(super) fn sort_select_rule(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(grid) = self
            .active_grid
            .and_then(|active| self.grids.get_mut(active))
        {
            grid.sort_selected = Some(index);
            grid.sort_combo = None;
        }
        cx.notify();
    }

    pub(super) fn sort_toggle_enabled(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(grid) = self
            .active_grid
            .and_then(|active| self.grids.get_mut(active))
            && let Some(rule) = grid.sort_draft.get_mut(index)
        {
            rule.enabled = !rule.enabled;
        }
        cx.notify();
    }

    pub(super) fn sort_toggle_direction(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(grid) = self
            .active_grid
            .and_then(|active| self.grids.get_mut(active))
            && let Some(rule) = grid.sort_draft.get_mut(index)
        {
            rule.descending = !rule.descending;
        }
        cx.notify();
    }

    pub(super) fn sort_open_combo(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(grid) = self
            .active_grid
            .and_then(|active| self.grids.get_mut(active))
        else {
            return;
        };
        let current = grid
            .sort_draft
            .get(index)
            .map(|rule| rule.column.clone())
            .unwrap_or_default();
        grid.sort_selected = Some(index);
        if grid
            .sort_combo
            .as_ref()
            .is_some_and(|(open, _)| *open == index)
        {
            grid.sort_combo = None;
        } else {
            let highlight = grid
                .columns
                .iter()
                .position(|column| column.name == current)
                .unwrap_or(0);
            grid.sort_combo = Some((index, current));
            self.sort_combo_filter.clear();
            self.sort_combo_highlight = highlight;
            self.sort_combo_focus_pending = true;
        }
        cx.notify();
    }

    /// The columns visible in the open popup after applying the type-ahead filter.
    pub(super) fn sort_combo_matches(&self, grid: &GridState) -> Vec<String> {
        let filter = self.sort_combo_filter.to_lowercase();
        grid.columns
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
        if let Some(grid) = self
            .active_grid
            .and_then(|active| self.grids.get_mut(active))
        {
            if let Some(rule) = grid.sort_draft.get_mut(index) {
                rule.column = column;
            }
            grid.sort_combo = None;
        }
        self.sort_combo_filter.clear();
        cx.notify();
    }

    pub(super) fn sort_combo_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_grid else {
            return;
        };
        let Some(grid) = self.grids.get(index) else {
            return;
        };
        if grid.sort_combo.is_none() {
            return;
        }
        let matches = self.sort_combo_matches(grid);
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
                if let Some(text) = event.keystroke.key_char.as_ref()
                    && !text.chars().any(char::is_control)
                {
                    self.sort_combo_filter.push_str(text);
                    self.sort_combo_highlight = 0;
                }
            }
        }
        self.sort_combo_highlight = self.sort_combo_highlight.min(count.saturating_sub(1));
        cx.notify();
    }

    pub(super) fn sort_confirm_combo(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_grid else {
            return;
        };
        let Some(grid) = self.grids.get(index) else {
            return;
        };
        let matches = self.sort_combo_matches(grid);
        if let Some(column) = matches.get(self.sort_combo_highlight).cloned() {
            self.sort_choose_column(index, column, cx);
            return;
        }
        self.sort_cancel_combo(cx);
    }

    pub(super) fn sort_cancel_combo(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(grid) = self
            .active_grid
            .and_then(|active| self.grids.get_mut(active))
        {
            grid.sort_combo = None;
        }
        self.sort_combo_filter.clear();
        cx.notify();
    }

    pub(super) fn sort_remove_rule(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(grid) = self
            .active_grid
            .and_then(|active| self.grids.get_mut(active))
        {
            if index < grid.sort_draft.len() {
                grid.sort_draft.remove(index);
            }
            grid.sort_combo = None;
            let len = grid.sort_draft.len();
            grid.sort_selected = match grid.sort_selected {
                Some(selected) if selected >= len => len.checked_sub(1),
                Some(selected) if selected > index => Some(selected - 1),
                other => other,
            };
        }
        cx.notify();
    }

    pub(super) fn sort_move_rule(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        if let Some(grid) = self
            .active_grid
            .and_then(|active| self.grids.get_mut(active))
        {
            let len = grid.sort_draft.len();
            if len > 1
                && let Some(selected) = grid.sort_selected
            {
                let target = (selected as isize + delta).clamp(0, len as isize - 1) as usize;
                if target != selected {
                    grid.sort_draft.swap(selected, target);
                    grid.sort_selected = Some(target);
                }
            }
        }
        cx.notify();
    }

    pub(super) fn sort_apply(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_grid else {
            return;
        };
        let Some(id) = self.grids.get(index).map(|grid| grid.id) else {
            return;
        };
        if let Some(grid) = self.grids.get_mut(index) {
            grid.sort_rules = grid.sort_draft.clone();
            grid.sort_combo = None;
            grid.page_index = 0;
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }

    /// The grid header's sort badge. Header sorting is single-column: clicking a column toggles
    /// its direction and drops every other criterion (the panel keeps just this one).
    pub(super) fn toggle_column_sort(&mut self, column: String, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_grid else {
            return;
        };
        let Some(id) = self.grids.get(index).map(|grid| grid.id) else {
            return;
        };
        {
            let Some(grid) = self.grids.get_mut(index) else {
                return;
            };
            if grid.sql.is_some() {
                return;
            }
            let descending = grid
                .sort_rules
                .iter()
                .find(|rule| rule.column == column)
                .map(|rule| !rule.descending)
                .unwrap_or(false);
            let mut rule = SortRule::new(column);
            rule.descending = descending;
            grid.sort_rules = vec![rule];
            if grid.sort_open {
                grid.sort_draft = grid.sort_rules.clone();
                grid.sort_selected = Some(0);
            }
            grid.page_index = 0;
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }
}
