use super::*;

/// The filter node at `path`, where each element indexes into the previous group's children.
pub(super) fn filter_node<'a>(nodes: &'a [FilterNode], path: &[usize]) -> Option<&'a FilterNode> {
    let (index, parents) = path.split_last()?;
    if parents.is_empty() {
        return nodes.get(*index);
    }
    match filter_node(nodes, parents)? {
        FilterNode::Group(group) => group.children.get(*index),
        FilterNode::Condition(_) => None,
    }
}

pub(super) fn filter_node_mut<'a>(
    nodes: &'a mut [FilterNode],
    path: &[usize],
) -> Option<&'a mut FilterNode> {
    let (index, parents) = path.split_last()?;
    if parents.is_empty() {
        return nodes.get_mut(*index);
    }
    match filter_node_mut(nodes, parents)? {
        FilterNode::Group(group) => group.children.get_mut(*index),
        FilterNode::Condition(_) => None,
    }
}

/// The children list addressed by `path` (the root list when `path` is empty), if it is a group.
fn filter_children_mut<'a>(
    nodes: &'a mut Vec<FilterNode>,
    path: &[usize],
) -> Option<&'a mut Vec<FilterNode>> {
    if path.is_empty() {
        return Some(nodes);
    }
    match filter_node_mut(nodes, path)? {
        FilterNode::Group(group) => Some(&mut group.children),
        FilterNode::Condition(_) => None,
    }
}

/// Every condition node's path, depth-first (used to key the per-field value focus handles).
fn collect_condition_paths(
    nodes: &[FilterNode],
    prefix: &mut Vec<usize>,
    out: &mut Vec<Vec<usize>>,
) {
    for (index, node) in nodes.iter().enumerate() {
        prefix.push(index);
        match node {
            FilterNode::Condition(_) => out.push(prefix.clone()),
            FilterNode::Group(group) => collect_condition_paths(&group.children, prefix, out),
        }
        prefix.pop();
    }
}

fn collect_condition_columns<'a>(nodes: &'a [FilterNode], out: &mut Vec<&'a str>) {
    for node in nodes {
        match node {
            FilterNode::Condition(condition) => out.push(condition.column.as_str()),
            FilterNode::Group(group) => collect_condition_columns(&group.children, out),
        }
    }
}

/// Drop groups that hold no conditions, so removing the last condition of a group also removes the
/// group and its boundary row.
fn prune_filter_groups(nodes: &mut Vec<FilterNode>) {
    for node in nodes.iter_mut() {
        if let FilterNode::Group(group) = node {
            prune_filter_groups(&mut group.children);
        }
    }
    nodes.retain(|node| !matches!(node, FilterNode::Group(group) if group.children.is_empty()));
}

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
            DeleteConfirm::Table {
                connection_index,
                database_index,
                name,
                operation,
            } => self.run_table_operation(connection_index, database_index, name, operation, cx),
            DeleteConfirm::SavedQuery { index } => self.delete_saved_query(index, cx),
            DeleteConfirm::BackupFile { index } => self.delete_backup_file(index, cx),
            DeleteConfirm::BackupConfig { index } => self.delete_backup_config(index, cx),
            DeleteConfirm::DesignRows { design_id, kind } => {
                let target = self
                    .designs
                    .iter()
                    .find(|design| design.read(cx).id == design_id)
                    .cloned();
                if let Some(design) = target {
                    design.update(cx, |design, cx| design.delete_selected(kind, cx));
                }
            }
            DeleteConfirm::User {
                connection_index,
                user,
                host,
                ..
            } => self.delete_user(connection_index, user, host, cx),
            DeleteConfirm::Routine {
                connection_index,
                database_index,
                name,
                kind,
                ..
            } => self.delete_routine(connection_index, database_index, name, kind, cx),
            DeleteConfirm::View {
                connection_index,
                database_index,
                name,
                ..
            } => self.delete_view(connection_index, database_index, name, cx),
        }
        cx.notify();
    }
}

impl GridView {
    /// The number of rows the grid paints: the loaded page plus any pending insert rows.
    pub(super) fn display_row_count(&self) -> usize {
        self.state.rows.len() + self.inserts.len()
    }

    /// The staged value of a cell, whether it is an existing row's pending edit or a pending
    /// insert row's value. `Some(None)` means an explicit `NULL`; `None` means no staged value.
    pub(super) fn staged_value(&self, row: usize, col: usize) -> Option<&Option<String>> {
        let data_rows = self.state.rows.len();
        if row < data_rows {
            self.state.edits.get(&(row, col))
        } else {
            self.inserts.get(row - data_rows)?.get(&col)
        }
    }

    /// Append a blank insert row (the "+" button), select its first cell and reveal it.
    pub(super) fn add_insert_row(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.state.editable || self.state.loading || self.state.columns.is_empty() {
            return;
        }
        // Repeatedly pressing "+" right after an untouched row should just refocus it.
        if !self.inserts.last().is_some_and(|row| row.is_empty()) {
            self.inserts.push(BTreeMap::new());
        }
        if self.cell_editor.is_some() {
            self.finish_cell_editor(cx);
        }
        let row = self.display_row_count() - 1;
        self.state.selection = Some(CellSelection::new(row, 0));
        self.list_scroll.scroll_to_item(row, ScrollStrategy::Bottom);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Load the current page, resetting the scroll position (page navigation, sort, filter).
    pub(super) fn load_page(&mut self, cx: &mut Context<'_, Self>) {
        self.load_page_inner(true, cx);
    }

    /// Reload the current page without moving the view: used after a write and by the Refresh
    /// button, so the user keeps their scroll position.
    pub(super) fn reload_after_write(&mut self, cx: &mut Context<'_, Self>) {
        self.load_page_inner(false, cx);
    }

    fn load_page_inner(&mut self, reset_scroll: bool, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
            self.reload_query(reset_scroll, cx);
            return;
        }

        self.state.loading = true;
        self.state.error = None;
        self.state.selection = None;
        self.state.edits.clear();
        self.inserts.clear();

        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let page_index = self.state.page_index;
        let page_size = self.state.page_size;
        let order_by = self.state.sort_columns();
        let filter = self.state.filter_conditions();

        self.selecting_cells = false;
        self.cell_editor = None;
        self.cell_editor_blur_subscription = None;
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
                        if !grid.state.manual_column_widths
                            || grid.state.column_widths.len() != page.columns.len()
                        {
                            grid.state.column_widths =
                                compute_column_widths(&page.columns, &page.rows);
                        }
                        grid.state.columns = page.columns;
                        grid.state.rows = Arc::new(page.rows);
                        grid.state.total_rows = page.total_rows;
                        grid.state.error = None;
                        // An empty editable table opens with one blank insert row ready to type
                        // into. Skipped for views, later pages and filtered views.
                        if grid.state.editable
                            && !grid.state.is_view
                            && page_index == 0
                            && grid.state.rows.is_empty()
                            && grid.state.filters.is_empty()
                            && grid.inserts.is_empty()
                        {
                            grid.inserts.push(BTreeMap::new());
                            grid.state.selection = Some(CellSelection::new(0, 0));
                        }
                        if reset_scroll {
                            grid.reset_scroll();
                        }
                    }
                    Err(error) => {
                        grid.state.error = Some(error.to_string());
                    }
                }
                grid.sync_page_input();
                cx.notify();
                // The scroll extents are only known after this frame's paint, so schedule one
                // more frame to reveal the scrollbars without waiting for a hover/resize.
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(32))
                        .await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            });
        })
        .detach();
    }

    /// Put the grid back at the top-left. Used when the row set changes identity (page turn,
    /// sort/filter); `reload_after_write` skips it so a write or refresh does not jump the view.
    fn reset_scroll(&mut self) {
        self.list_scroll = UniformListScrollHandle::new();
        self.hscroll.set_offset(Point::default());
    }

    fn reload_query(&mut self, reset_scroll: bool, cx: &mut Context<'_, Self>) {
        let Some(sql) = self.state.sql.clone() else {
            return;
        };
        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let editable = self.state.editable;
        self.state.loading = true;
        self.state.error = None;
        self.state.selection = None;
        self.state.edits.clear();
        self.inserts.clear();
        self.cell_editor = None;
        self.cell_editor_blur_subscription = None;
        self.date_picker = None;
        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let query_database = (!database.is_empty()).then(|| database.clone());
            let query_connection = connection.clone();
            let result = match runtime
                .spawn(async move {
                    query_connection
                        .execute_query(query_database.as_deref(), &sql)
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            // `execute_query` only returns result-set metadata, so re-attach the catalog's
            // primary-key/data-type flags. Without them a later edit would use every column as
            // the row key, binding `NULL` numeric columns as `''` (MySQL 1292).
            let catalog_columns = if editable && !table.is_empty() {
                let column_connection = connection.clone();
                let column_database = database.clone();
                let column_table = table.clone();
                match runtime
                    .spawn(async move {
                        column_connection
                            .columns(&column_database, &column_table)
                            .await
                    })
                    .await
                {
                    Ok(inner) => inner.ok(),
                    Err(_) => None,
                }
            } else {
                None
            };

            let _ = this.update(cx, |grid, cx| {
                grid.state.loading = false;
                match result {
                    Ok(mut query_result) => {
                        if let Some(catalog_columns) = catalog_columns {
                            for column in query_result.columns.iter_mut() {
                                if let Some(found) = catalog_columns
                                    .iter()
                                    .find(|candidate| candidate.name == column.name)
                                {
                                    column.primary_key = found.primary_key;
                                    column.data_type = found.data_type.clone();
                                }
                            }
                        }
                        let total = query_result.rows.len() as u64;
                        if !grid.state.manual_column_widths
                            || grid.state.column_widths.len() != query_result.columns.len()
                        {
                            grid.state.column_widths =
                                compute_column_widths(&query_result.columns, &query_result.rows);
                        }
                        grid.state.columns = query_result.columns;
                        grid.state.rows = Arc::new(query_result.rows);
                        grid.state.total_rows = Some(total);
                        grid.state.page_index = 0;
                        grid.state.page_size = total.max(1);
                        grid.state.error = None;
                        if reset_scroll {
                            grid.reset_scroll();
                        }
                    }
                    Err(error) => grid.state.error = Some(error.to_string()),
                }
                grid.sync_page_input();
                cx.notify();
                // Same as `load_page`: measure the scroll extents on a follow-up frame so the
                // scrollbars show up as soon as the results paint.
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(32))
                        .await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
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
        self.reload_after_write(cx);
    }

    pub(super) fn toggle_sort_panel(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
            return;
        }
        if self.state.sort_open {
            self.state.sort_open = false;
            self.state.sort_selected = None;
            self.state.sort_draft.clear();
            self.sort_field_combos.clear();
        } else {
            self.state.sort_open = true;
            self.state.sort_draft = self.state.sort_rules.clone();
            self.state.sort_selected = (!self.state.sort_draft.is_empty()).then_some(0);
            self.sort_field_combos.clear();
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
        self.sort_field_combos.clear();
        cx.notify();
    }

    pub(super) fn sort_select_rule(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.state.sort_selected = Some(index);
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

    /// The shared drop-down for a sort rule's column, created lazily per rule index.
    pub(super) fn sort_field_combo(
        &mut self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> Entity<ComboBox> {
        if let Some(entity) = self.sort_field_combos.get(&index) {
            return entity.clone();
        }
        let theme = self.theme;
        let weak = self.self_weak.clone();
        let options: Vec<ComboOption> = self
            .state
            .columns
            .iter()
            .map(|column| ComboOption::plain(column.name.clone()))
            .collect();
        let selected = self
            .state
            .sort_draft
            .get(index)
            .map(|rule| rule.column.clone())
            .unwrap_or_default();
        let entity = cx.new(move |cx| {
            ComboBox::new(theme, options, selected, FILTER_FIELD_WIDTH, cx).on_select(Rc::new(
                move |value, _window, cx| {
                    let _ = weak.update(cx, |grid, cx| {
                        grid.sort_choose_column(index, value.to_string(), cx);
                    });
                },
            ))
        });
        self.sort_field_combos.insert(index, entity.clone());
        entity
    }

    /// Commit a column choice made in a sort rule's dropdown.
    pub(super) fn sort_choose_column(
        &mut self,
        index: usize,
        column: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(rule) = self.state.sort_draft.get_mut(index) {
            rule.column = column;
        }
        cx.notify();
    }

    pub(super) fn sort_remove_rule(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index < self.state.sort_draft.len() {
            self.state.sort_draft.remove(index);
        }
        self.sort_field_combos.clear();
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
                self.sort_field_combos.clear();
            }
        }
        cx.notify();
    }

    pub(super) fn sort_apply(&mut self, cx: &mut Context<'_, Self>) {
        self.state.sort_rules = self.state.sort_draft.clone();
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
            self.filter_active = None;
            self.state.filter_draft.clear();
            self.filter_value_focus.clear();
            self.filter_value2_focus.clear();
            self.filter_field_combos.clear();
            self.filter_operator_combos.clear();
        } else {
            if self.state.sql.is_some() {
                return;
            }
            self.state.filter_open = true;
            self.filter_active = None;
            self.filter_field_combos.clear();
            self.filter_operator_combos.clear();
            self.state.filter_draft = self.state.filters.clone();
            if self.state.filter_draft.is_empty() {
                self.filter_draft_start();
            }
            self.rebuild_filter_focus(cx);
        }
        cx.notify();
    }

    /// Seed an empty draft with one parenthesized group holding a single condition.
    fn filter_draft_start(&mut self) {
        if let Some(group) = self.new_filter_group() {
            self.state.filter_draft.push(group);
        }
    }

    /// One value focus handle per condition node, keyed by the node's path in the draft tree.
    fn rebuild_filter_focus(&mut self, cx: &mut Context<'_, Self>) {
        let mut paths = Vec::new();
        collect_condition_paths(&self.state.filter_draft, &mut Vec::new(), &mut paths);
        self.filter_value_focus = paths
            .iter()
            .map(|path| (path.clone(), cx.focus_handle()))
            .collect();
        self.filter_value2_focus = paths
            .into_iter()
            .map(|path| (path, cx.focus_handle()))
            .collect();
    }

    fn next_filter_column(&self) -> String {
        let mut used: Vec<&str> = Vec::new();
        collect_condition_columns(&self.state.filter_draft, &mut used);
        self.state
            .columns
            .iter()
            .map(|column| column.name.clone())
            .find(|name| !used.contains(&name.as_str()))
            .or_else(|| self.state.columns.first().map(|column| column.name.clone()))
            .unwrap_or_default()
    }

    fn new_filter_condition(&self) -> Option<FilterNode> {
        let column = self.next_filter_column();
        (!column.is_empty()).then(|| FilterNode::Condition(FilterCondition::new(column)))
    }

    /// A new group holding a single condition.
    fn new_filter_group(&self) -> Option<FilterNode> {
        let condition = self.new_filter_condition()?;
        let mut group = FilterGroup::new(FilterConjunction::And);
        group.children.push(condition);
        Some(FilterNode::Group(group))
    }

    /// The condition row's "+": insert a sibling condition after the node at `path`. With an empty
    /// path it appends to the last group (starting a group when the draft is empty).
    pub(super) fn filter_add_condition(&mut self, path: Vec<usize>, cx: &mut Context<'_, Self>) {
        if path.is_empty() {
            if self.state.filter_draft.is_empty() {
                self.filter_draft_start();
            } else if let Some(condition) = self.new_filter_condition() {
                match self.state.filter_draft.last_mut() {
                    Some(FilterNode::Group(group)) => group.children.push(condition),
                    _ => self.state.filter_draft.push(condition),
                }
            }
            self.after_filter_change(cx);
            return;
        }

        let Some(condition) = self.new_filter_condition() else {
            return;
        };
        if let Some(FilterNode::Group(group)) = filter_node_mut(&mut self.state.filter_draft, &path)
        {
            group.children.push(condition);
        } else {
            self.filter_insert_after(&path, condition);
        }
        self.after_filter_change(cx);
    }

    /// The group boundary row's "+": insert a new group after the one at `path`.
    pub(super) fn filter_add_group(&mut self, path: Vec<usize>, cx: &mut Context<'_, Self>) {
        let Some(group) = self.new_filter_group() else {
            return;
        };
        if path.is_empty() {
            self.state.filter_draft.push(group);
        } else {
            self.filter_insert_after(&path, group);
        }
        self.after_filter_change(cx);
    }

    /// Insert `node` right after the node at `path`, in the same parent.
    fn filter_insert_after(&mut self, path: &[usize], node: FilterNode) {
        let (parent, position) = match path.split_last() {
            Some((index, parents)) => (parents.to_vec(), index + 1),
            None => (Vec::new(), usize::MAX),
        };
        if let Some(children) = filter_children_mut(&mut self.state.filter_draft, &parent) {
            let position = position.min(children.len());
            children.insert(position, node);
        }
    }

    pub(super) fn filter_remove_node(&mut self, path: Vec<usize>, cx: &mut Context<'_, Self>) {
        let Some((index, parents)) = path.split_last() else {
            return;
        };
        let (index, parents) = (*index, parents.to_vec());
        if let Some(children) = filter_children_mut(&mut self.state.filter_draft, &parents)
            && index < children.len()
        {
            children.remove(index);
        }
        self.after_filter_change(cx);
    }

    /// Reset the popups/active field and refresh focus handles after a structural change. Groups
    /// left without conditions are dropped (the boundary rows hide them from edits anyway).
    fn after_filter_change(&mut self, cx: &mut Context<'_, Self>) {
        prune_filter_groups(&mut self.state.filter_draft);
        self.filter_active = None;
        self.filter_field_combos.clear();
        self.filter_operator_combos.clear();
        self.rebuild_filter_focus(cx);
        cx.notify();
    }

    pub(super) fn filter_toggle_conjunction(
        &mut self,
        path: Vec<usize>,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(node) = filter_node_mut(&mut self.state.filter_draft, &path) {
            let conjunction = node.conjunction().toggled();
            node.set_conjunction(conjunction);
        }
        cx.notify();
    }

    /// The shared drop-down for a condition's column, created lazily per node path.
    pub(super) fn filter_field_combo(
        &mut self,
        path: &[usize],
        cx: &mut Context<'_, Self>,
    ) -> Entity<ComboBox> {
        let key = path.to_vec();
        if let Some(entity) = self.filter_field_combos.get(&key) {
            return entity.clone();
        }
        let theme = self.theme;
        let weak = self.self_weak.clone();
        let callback_path = key.clone();
        let options: Vec<ComboOption> = self
            .state
            .columns
            .iter()
            .map(|column| ComboOption::plain(column.name.clone()))
            .collect();
        let selected = self
            .state
            .columns
            .first()
            .map(|column| column.name.clone())
            .unwrap_or_default();
        let entity = cx.new(move |cx| {
            ComboBox::new(theme, options, selected, FILTER_FIELD_WIDTH, cx).on_select(Rc::new(
                move |value, _window, cx| {
                    let _ = weak.update(cx, |grid, cx| {
                        grid.filter_choose_field(callback_path.clone(), value.to_string(), cx);
                    });
                },
            ))
        });
        self.filter_field_combos.insert(key, entity.clone());
        entity
    }

    /// The shared drop-down for a condition's comparison operator, keyed by node path.
    pub(super) fn filter_operator_combo(
        &mut self,
        path: &[usize],
        cx: &mut Context<'_, Self>,
    ) -> Entity<ComboBox> {
        let key = path.to_vec();
        if let Some(entity) = self.filter_operator_combos.get(&key) {
            return entity.clone();
        }
        let theme = self.theme;
        let weak = self.self_weak.clone();
        let callback_path = key.clone();
        let options: Vec<ComboOption> = FilterOperator::ALL
            .iter()
            .enumerate()
            .map(|(index, operator)| {
                ComboOption::new(index.to_string(), t!(operator.label_key()).to_string())
            })
            .collect();
        let entity = cx.new(move |cx| {
            ComboBox::new(theme, options, "0", FILTER_OPERATOR_WIDTH, cx).on_select(Rc::new(
                move |value, _window, cx| {
                    let Ok(index) = value.parse::<usize>() else {
                        return;
                    };
                    let Some(operator) = FilterOperator::ALL.get(index).copied() else {
                        return;
                    };
                    let _ = weak.update(cx, |grid, cx| {
                        grid.filter_choose_operator(callback_path.clone(), operator, cx);
                    });
                },
            ))
        });
        self.filter_operator_combos.insert(key, entity.clone());
        entity
    }

    pub(super) fn filter_choose_field(
        &mut self,
        path: Vec<usize>,
        column: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(FilterNode::Condition(condition)) =
            filter_node_mut(&mut self.state.filter_draft, &path)
        {
            condition.column = column;
        }
        cx.notify();
    }

    pub(super) fn filter_choose_operator(
        &mut self,
        path: Vec<usize>,
        operator: FilterOperator,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(FilterNode::Condition(condition)) =
            filter_node_mut(&mut self.state.filter_draft, &path)
        {
            condition.operator = operator;
        }
        cx.notify();
    }

    pub(super) fn filter_focus_value(
        &mut self,
        path: Vec<usize>,
        slot: u8,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.filter_active = Some((path.clone(), slot));
        let handles = if slot == 0 {
            &self.filter_value_focus
        } else {
            &self.filter_value2_focus
        };
        let handle = handles
            .iter()
            .find(|(candidate, _)| *candidate == path)
            .map(|(_, handle)| handle.clone());
        if let Some(handle) = handle {
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    pub(super) fn filter_value_key(
        &mut self,
        path: Vec<usize>,
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
        if let Some(FilterNode::Condition(condition)) =
            filter_node_mut(&mut self.state.filter_draft, &path)
        {
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
            // The grid is cached and reads `limit_records` from `AppView`; push the change.
            cx.notify();
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
