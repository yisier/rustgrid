use super::*;

/// The pager's page box: a digits-only [`TextInput`], so caret, mouse selection, clipboard and
/// IME all come from the shared field. Enter jumps to the typed page and Escape restores the
/// current one; the text stays in [`GridView::page_input`], which the navigation buttons rewrite.
pub(super) fn make_page_input(
    theme: Theme,
    text: String,
    weak: WeakEntity<GridView>,
    cx: &mut Context<'_, GridView>,
) -> Entity<TextInput> {
    let options = TextInputOptions {
        accepts: Some(Rc::new(|character: char| character.is_ascii_digit())),
        text_size: Some(12.0),
        bare: true,
        ..Default::default()
    };
    let change = weak.clone();
    let submit = weak.clone();
    let cancel = weak;
    cx.new(move |cx| {
        TextInput::new(theme, text, options, cx)
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |grid, _cx| grid.page_input = text.to_string());
            }))
            .on_submit(Rc::new(move |_window, cx| {
                let _ = submit.update(cx, |grid, cx| grid.commit_page_input(cx));
            }))
            .on_cancel(Rc::new(move |_window, cx| {
                let _ = cancel.update(cx, |grid, cx| grid.sync_page_input(cx));
            }))
    })
}

/// The page-size box of the 限制记录 panel. Same digits-only field as the page box, with Up/Down
/// stepping the value by 100 (the compact stepper Navicat's record limit has).
pub(super) fn make_page_size_input(
    theme: Theme,
    text: String,
    weak: WeakEntity<GridView>,
    cx: &mut Context<'_, GridView>,
) -> Entity<TextInput> {
    let options = TextInputOptions {
        accepts: Some(Rc::new(|character: char| character.is_ascii_digit())),
        text_size: Some(12.0),
        bare: true,
        ..Default::default()
    };
    let change = weak.clone();
    let submit = weak.clone();
    let cancel = weak.clone();
    let arrow = weak;
    cx.new(move |cx| {
        TextInput::new(theme, text, options, cx)
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |grid, _cx| grid.page_size_input = text.to_string());
            }))
            .on_submit(Rc::new(move |_window, cx| {
                let _ = submit.update(cx, |grid, cx| grid.page_size_apply(cx));
            }))
            .on_cancel(Rc::new(move |_window, cx| {
                let _ = cancel.update(cx, |grid, cx| {
                    grid.set_page_size_text(grid.state.page_size.to_string(), cx);
                });
            }))
            .on_arrow(Rc::new(move |down, _window, cx| {
                let _ = arrow.update(cx, |grid, cx| grid.step_page_size(down, cx));
            }))
    })
}

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
        self.settle_after_tab_closed(cx);
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

    /// Close every table/key page opened from one database. Query result grids (`sql` is set)
    /// belong to their query tab, not to the database browser, so they stay open.
    pub(super) fn close_database_grids(
        &mut self,
        connection: &Arc<dyn Connection>,
        database: &str,
        cx: &App,
    ) {
        let active_id = self.active_grid_id(cx);
        self.grids.retain(|grid| {
            let grid = grid.read(cx);
            !(grid.state.sql.is_none()
                && grid.state.database == database
                && Arc::ptr_eq(&grid.state.connection, connection))
        });
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
            DeleteConfirm::Group { id } => self.delete_group(id, cx),
            DeleteConfirm::TreeSelection {
                connections,
                groups,
            } => self.delete_tree_selection(connections, groups, cx),
            DeleteConfirm::Table {
                connection_index,
                database_index,
                name,
                operation,
            } => self.run_table_operation(connection_index, database_index, name, operation, cx),
            DeleteConfirm::Tables {
                connection_index,
                database_index,
                names,
            } => self.run_drop_tables(connection_index, database_index, names, cx),
            DeleteConfirm::TablesOperation {
                connection_index,
                database_index,
                names,
                operation,
            } => self.run_table_operations(connection_index, database_index, names, operation, cx),
            DeleteConfirm::SavedQuery { index } => self.delete_saved_query(index, cx),
            DeleteConfirm::SavedQueries { indices } => self.delete_saved_queries(indices, cx),
            DeleteConfirm::BackupFile { index } => self.delete_backup_file(index, cx),
            DeleteConfirm::BackupConfig { index } => self.delete_backup_config(index, cx),
            DeleteConfirm::TransferConfigs { indices } => self.delete_transfer_configs(indices, cx),
            DeleteConfirm::BackupEntries { files, configs } => {
                self.delete_backups(files, configs, cx)
            }
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
            DeleteConfirm::Schema {
                connection_index,
                database_index,
                schema,
            } => self.delete_schema(connection_index, database_index, schema, cx),
        }
        cx.notify();
    }
}

impl GridView {
    /// The number of rows the grid paints: the loaded page plus any pending insert rows.
    pub(super) fn display_row_count(&self) -> usize {
        self.state.rows.len() + self.inserts.len()
    }

    /// The displayed row at `row`: a page row or a pending insert.
    pub(super) fn row_at(&self, row: usize) -> Option<GridRow> {
        row_origin(&self.inserts, self.state.rows.len(), row)
    }

    /// The staged value of a cell, whether it is an existing row's pending edit or a pending
    /// insert row's value. `Some(None)` means an explicit `NULL`; `None` means no staged value.
    pub(super) fn staged_value(&self, row: usize, col: usize) -> Option<&Option<String>> {
        match self.row_at(row)? {
            GridRow::Data(data) => self.state.edits.get(&(data, col)),
            GridRow::Insert(index) => self.inserts.get(index)?.values.get(&col),
        }
    }

    /// Append a blank insert row (the "+" button), select its first cell and reveal it.
    pub(super) fn add_insert_row(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.state.editable || self.state.loading || self.state.columns.is_empty() {
            return;
        }
        // Repeatedly pressing "+" right after an untouched row should just refocus it.
        let last = self.display_row_count().saturating_sub(1);
        let blank = self.display_row_count() > 0
            && matches!(
                self.row_at(last),
                Some(GridRow::Insert(index))
                    if self.inserts[index].after.is_none()
                        && self.inserts[index].values.is_empty()
            );
        if !blank {
            self.inserts.push(PendingInsert::appended());
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

    /// 复制行: insert a copy of the selected record right below it (the DBX clone-row behaviour),
    /// leaving every auto-increment / generated column unset so the engine assigns it, then open
    /// the in-place editor on the new row's first column.
    pub(super) fn duplicate_row(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.state.editable
            || self.state.loading
            || self.state.visible_columns == 0
            || self.state.key_status.is_some()
            || self.state.rows.is_empty()
        {
            return;
        }
        // Stage the live editor first (without flushing it to the database), so the copy sees the
        // value the user just typed.
        if self.cell_editor.is_some() {
            self.commit_editor(cx);
        }
        let Some(source) = self.selected_row() else {
            return;
        };
        let Some(origin) = self.row_at(source) else {
            return;
        };
        let (values, after) = match origin {
            GridRow::Data(data) => (
                duplicate_values(
                    &self.state.columns,
                    &self.state.column_order,
                    &self.state.rows,
                    data,
                    &self.state.edits,
                ),
                Some(data),
            ),
            GridRow::Insert(index) => {
                let source = &self.inserts[index];
                let values = source
                    .values
                    .iter()
                    .filter(|(col, _)| {
                        self.state
                            .columns
                            .get(**col)
                            .is_none_or(|column| !column.auto_increment && !column.generated)
                    })
                    .map(|(col, value)| (*col, value.clone()))
                    .collect();
                (values, source.after)
            }
        };
        self.inserts.push(PendingInsert { values, after });
        let row = display_of_insert(&self.inserts, self.state.rows.len(), self.inserts.len() - 1)
            .unwrap_or_else(|| self.display_row_count() - 1);
        self.state.selection = Some(CellSelection::new(row, 0));
        self.list_scroll
            .scroll_to_item(row, ScrollStrategy::Nearest);
        // The clone opens on its first column, so bring the pane back to it even if the user had
        // scrolled right.
        self.hscroll
            .set_offset(Point::new(px(0.0), self.hscroll.offset().y));
        // The clone lands in edit mode on its first column, ready to type.
        window.focus(&self.focus, cx);
        self.begin_edit((row, 0), None, window, cx);
        cx.notify();
    }

    /// The row the grid asks about: the selection's active cursor, or its first row.
    fn selected_row(&self) -> Option<usize> {
        let selection = self.state.selection.as_ref()?;
        let cursor = selection.active_cursor().0;
        if cursor < self.display_row_count() {
            return Some(cursor);
        }
        selection
            .row_indices()
            .into_iter()
            .find(|row| *row < self.display_row_count())
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
        // A reload in place (Refresh / F5, after a write) keeps the selected cells.
        if reset_scroll {
            self.state.selection = None;
        }
        self.state.edits.clear();
        self.inserts.clear();
        self.state.undo.clear();
        self.state.redo.clear();

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
        let started = std::time::Instant::now();
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
            let elapsed = started.elapsed();

            let _ = this.update(cx, |grid, cx| {
                grid.state.loading = false;
                grid.state.elapsed = Some(elapsed);
                // Consume the explicit-refresh marker now, so a fetch that replaces this one does
                // not leave a stale "refreshed" confirmation behind.
                let refreshed = std::mem::take(&mut grid.refresh_feedback);
                match result {
                    Ok(page) => {
                        // The widths carry over by column name when the user has dragged column
                        // edges, so paging/sorting does not snap them back.
                        let previous_columns = grid.state.columns.clone();
                        let previous_widths = grid.state.column_widths.clone();
                        let manual = grid.state.manual_column_widths;
                        let widths = page_column_widths(
                            &page.columns,
                            &page.rows,
                            &previous_columns,
                            &previous_widths,
                            manual,
                        );
                        grid.state.install_page(page.columns, page.rows, widths);
                        grid.state.total_rows = page.total_rows;
                        // Drop a kept selection that no longer fits the reloaded page.
                        let (rows, cols) = (grid.display_row_count(), grid.state.visible_columns);
                        if grid.state.selection.as_ref().is_some_and(|selection| {
                            selection.rows().1 >= rows || selection.cols().1 >= cols
                        }) {
                            grid.state.selection = None;
                        }
                        // A freshly loaded page opens with its first record selected, so the
                        // status bar and keyboard navigation start on real data instead of an
                        // empty grid. A kept selection (refresh / reload after a write) wins.
                        if grid.state.selection.is_none()
                            && !grid.state.rows.is_empty()
                            && grid.state.visible_columns > 0
                        {
                            grid.state.selection = Some(CellSelection::new(0, 0));
                        }
                        if grid.field_filter_open {
                            // Keep the in-progress draft, but fold in any field the table has
                            // gained since the popup opened so it is still listable.
                            grid.field_filter_draft.order = grid
                                .field_filter_draft
                                .resolved_order(&grid.state.all_columns);
                        }
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
                            grid.inserts.push(PendingInsert::appended());
                            grid.state.selection = Some(CellSelection::new(0, 0));
                        }
                        if reset_scroll {
                            grid.reset_scroll();
                        }
                        if refreshed {
                            grid.show_refresh_toast(
                                ToastKind::Success,
                                t!("grid.refreshed").to_string(),
                                cx,
                            );
                        }
                    }
                    Err(error) => {
                        grid.state.error = Some(error.to_string());
                    }
                }
                grid.sync_page_input(cx);
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

    /// Put the grid's rows back at the top. Used when the row set changes identity (page turn,
    /// sort/filter); `reload_after_write` skips it so a write or refresh does not jump the view.
    /// The horizontal position is deliberately kept: sorting or filtering reuses the same
    /// columns, and snapping them back to the first column made the user lose the width they
    /// had scrolled to. The handle clamps the offset itself when a new column set is narrower.
    fn reset_scroll(&mut self) {
        self.list_scroll = UniformListScrollHandle::new();
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
        self.state.undo.clear();
        self.state.redo.clear();
        self.cell_editor = None;
        self.cell_editor_blur_subscription = None;
        self.date_picker = None;
        cx.notify();

        let runtime = self.runtime.clone();
        let started = std::time::Instant::now();
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
            let elapsed = started.elapsed();

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
                grid.state.elapsed = Some(elapsed);
                let refreshed = std::mem::take(&mut grid.refresh_feedback);
                match result {
                    Ok(mut query_result) => {
                        if let Some(catalog_columns) = catalog_columns {
                            for column in query_result.columns.iter_mut() {
                                if let Some(found) = catalog_columns
                                    .iter()
                                    .find(|candidate| candidate.name == column.name)
                                {
                                    column.primary_key = found.primary_key;
                                    column.auto_increment = found.auto_increment;
                                    column.generated = found.generated;
                                    column.data_type = found.data_type.clone();
                                }
                            }
                        }
                        let total = query_result.rows.len() as u64;
                        // A query grid has no table to key a field layout on, so it always shows
                        // every column in the statement's own order.
                        let widths =
                            compute_column_widths(&query_result.columns, &query_result.rows);
                        grid.state
                            .install_page(query_result.columns, query_result.rows, widths);
                        grid.state.total_rows = Some(total);
                        grid.state.page_index = 0;
                        grid.state.page_size = total.max(1);
                        grid.state.error = None;
                        if reset_scroll {
                            grid.reset_scroll();
                        }
                        if refreshed {
                            grid.show_refresh_toast(
                                ToastKind::Success,
                                t!("grid.refreshed").to_string(),
                                cx,
                            );
                        }
                    }
                    Err(error) => grid.state.error = Some(error.to_string()),
                }
                grid.sync_page_input(cx);
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
        if self.guard_pending(GridNav::Next, cx) {
            return;
        }
        self.next_page_now(cx);
    }

    pub(super) fn next_page_now(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.has_next() {
            self.state.page_index += 1;
        }
        self.sync_page_input(cx);
        self.load_page(cx);
    }

    pub(super) fn prev_page(&mut self, cx: &mut Context<'_, Self>) {
        if self.guard_pending(GridNav::Prev, cx) {
            return;
        }
        self.prev_page_now(cx);
    }

    pub(super) fn prev_page_now(&mut self, cx: &mut Context<'_, Self>) {
        self.state.page_index = self.state.page_index.saturating_sub(1);
        self.sync_page_input(cx);
        self.load_page(cx);
    }

    pub(super) fn first_page(&mut self, cx: &mut Context<'_, Self>) {
        if self.guard_pending(GridNav::First, cx) {
            return;
        }
        self.first_page_now(cx);
    }

    pub(super) fn first_page_now(&mut self, cx: &mut Context<'_, Self>) {
        self.state.page_index = 0;
        self.sync_page_input(cx);
        self.load_page(cx);
    }

    pub(super) fn last_page(&mut self, cx: &mut Context<'_, Self>) {
        if self.guard_pending(GridNav::Last, cx) {
            return;
        }
        self.last_page_now(cx);
    }

    pub(super) fn last_page_now(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(last) = self.state.last_page() {
            self.state.page_index = last;
        }
        self.sync_page_input(cx);
        self.load_page(cx);
    }

    pub(super) fn sync_page_input(&mut self, cx: &mut Context<'_, Self>) {
        self.page_input = (self.state.page_index + 1).to_string();
        let text = self.page_input.clone();
        self.page_input_view
            .update(cx, |input, cx| input.set_text(text, cx));
    }

    /// Replace the page-size box's text and keep its field in step.
    pub(super) fn set_page_size_text(&mut self, text: String, cx: &mut Context<'_, Self>) {
        self.page_size_input = text.clone();
        self.page_size_view
            .update(cx, |input, cx| input.set_text(text, cx));
    }

    /// The text the page box holds right now, including a keystroke the shared field has not
    /// flushed back to [`Self::page_input`] yet (Enter can arrive before the next render).
    fn page_text_now(&self, cx: &App) -> String {
        self.page_input_view
            .read(cx)
            .current_text(cx)
            .unwrap_or_else(|| self.page_input.clone())
    }

    /// The text the page-size box holds right now, for the same reason as [`Self::page_text_now`].
    fn page_size_text_now(&self, cx: &App) -> String {
        self.page_size_view
            .read(cx)
            .current_text(cx)
            .unwrap_or_else(|| self.page_size_input.clone())
    }

    pub(super) fn commit_page_input(&mut self, cx: &mut Context<'_, Self>) {
        self.page_input = self.page_text_now(cx);
        let requested = self.page_input.trim().parse::<u64>().unwrap_or(1).max(1);
        let target = match self.state.last_page() {
            Some(last) => (requested - 1).min(last),
            None => requested - 1,
        };
        if self.guard_pending(GridNav::Page(target), cx) {
            self.sync_page_input(cx);
            return;
        }
        self.state.page_index = target;
        self.sync_page_input(cx);
        self.load_page(cx);
    }

    pub(super) fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        if self.guard_pending(GridNav::Refresh, cx) {
            return;
        }
        self.refresh_now(cx);
    }

    /// Run the fetch of an explicit refresh and remember to confirm it once it lands, so the
    /// toolbar / context-menu / F5 refresh never looks like a dropped click.
    fn refresh_now(&mut self, cx: &mut Context<'_, Self>) {
        self.refresh_feedback = true;
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
        // The popup menu is widened to fit the longest column name; the trigger sizes to content.
        let menu_width = self.field_menu_width();
        let entity = cx.new(move |cx| {
            ComboBox::new(theme, options, selected, menu_width, cx).on_select(Rc::new(
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
        if self.guard_pending(GridNav::Sort, cx) {
            return;
        }
        self.sort_apply_now(cx);
    }

    pub(super) fn sort_apply_now(&mut self, cx: &mut Context<'_, Self>) {
        self.sync_sort_controls(cx);
        self.state.sort_rules = self.state.sort_draft.clone();
        self.state.page_index = 0;
        self.sync_page_input(cx);
        self.load_page(cx);
    }

    /// Fold a column pick the host has not adopted yet back into the sort draft, so 应用 always
    /// matches what the panel shows.
    fn sync_sort_controls(&mut self, cx: &App) {
        let picks: Vec<(usize, String)> = self
            .sort_field_combos
            .iter()
            .filter_map(|(index, combo)| {
                combo.read(cx).active_value(cx).map(|value| (*index, value))
            })
            .collect();
        for (index, column) in picks {
            if let Some(rule) = self.state.sort_draft.get_mut(index) {
                rule.column = column;
            }
        }
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
        if self.guard_pending(
            GridNav::HeaderSort {
                column: rule.column.clone(),
                descending,
            },
            cx,
        ) {
            return;
        }
        self.state.sort_rules = vec![rule];
        if self.state.sort_open {
            self.state.sort_draft = self.state.sort_rules.clone();
            self.state.sort_selected = Some(0);
        }
        self.state.page_index = 0;
        self.sync_page_input(cx);
        self.load_page(cx);
    }

    pub(super) fn toggle_filter_panel(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.filter_open {
            self.state.filter_open = false;
            self.state.filter_draft.clear();
            self.filter_value_inputs.clear();
            self.filter_field_combos.clear();
            self.filter_operator_combos.clear();
            self.filter_date_picker = None;
        } else {
            if self.state.sql.is_some() {
                return;
            }
            self.state.filter_open = true;
            self.filter_value_inputs.clear();
            self.filter_field_combos.clear();
            self.filter_operator_combos.clear();
            self.filter_date_picker = None;
            self.state.filter_draft = self.state.filters.clone();
            if self.state.filter_draft.is_empty() {
                self.filter_draft_start();
            }
        }
        cx.notify();
    }

    /// Open the 筛选 panel and add a condition on `column` (the header's context menu). The
    /// applied filters seed the draft exactly like the toolbar's 筛选 button, then the clicked
    /// field is appended to the last group so the panel opens ready to take a value.
    pub(super) fn filter_column(&mut self, column: usize, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
            return;
        }
        let Some(name) = self
            .state
            .columns
            .get(column)
            .map(|column| column.name.clone())
        else {
            return;
        };
        if !self.state.filter_open {
            self.state.filter_open = true;
            self.state.filter_draft = self.state.filters.clone();
        }
        let condition = FilterNode::Condition(FilterCondition::new(name));
        match self.state.filter_draft.last_mut() {
            Some(FilterNode::Group(group)) => group.children.push(condition),
            _ => self.state.filter_draft.push(condition),
        }
        self.after_filter_change(cx);
    }

    /// Start a filter from a cell's own value (the cell context menu's 筛选): open the WHERE panel
    /// with a condition on that column, using the clicked value (or `IS NULL` / `IS EMPTY` for a
    /// NULL / empty cell), then apply it right away.
    pub(super) fn filter_cell(&mut self, row: usize, col: usize, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() || self.state.key_status.is_some() {
            return;
        }
        let Some(GridRow::Data(data)) = self.row_at(row) else {
            return;
        };
        let Some(name) = self
            .state
            .columns
            .get(col)
            .map(|column| column.name.clone())
        else {
            return;
        };
        let cell = self
            .state
            .rows
            .get(data)
            .and_then(|values| values.get(self.state.source_col(col)));
        let mut condition = FilterCondition::new(name);
        match cell {
            Some(CellValue::Null) | None => condition.operator = FilterOperator::IsNull,
            Some(cell) => {
                let value = cell.as_edit_string();
                if value.is_empty() {
                    condition.operator = FilterOperator::IsEmpty;
                } else {
                    condition.value = value;
                }
            }
        }
        if !self.state.filter_open {
            self.state.filter_open = true;
            self.state.filter_draft = self.state.filters.clone();
        }
        let node = FilterNode::Condition(condition);
        if self.state.filter_draft.is_empty() {
            // Match a manually started filter: the first condition lives in a group card, whose
            // in-group rail is what carries the enable checkbox and the 并且/或者 pill.
            let mut group = FilterGroup::new(FilterConjunction::And);
            group.children.push(node);
            self.state.filter_draft.push(FilterNode::Group(group));
        } else {
            match self.state.filter_draft.last_mut() {
                Some(FilterNode::Group(group)) => group.children.push(node),
                _ => self.state.filter_draft.push(node),
            }
        }
        self.after_filter_change(cx);
        self.filter_apply(cx);
    }

    /// Seed an empty draft with one parenthesized group holding a single condition.
    fn filter_draft_start(&mut self) {
        if let Some(group) = self.new_filter_group() {
            self.state.filter_draft.push(group);
        }
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

    /// Remove one node from the draft and apply the change at once, so a removed condition does not
    /// silently reappear when the panel is reopened.
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
        self.filter_apply(cx);
    }

    /// Reset the popups/editors after a structural change. Groups left without conditions are
    /// dropped (the boundary rows hide them from edits anyway).
    fn after_filter_change(&mut self, cx: &mut Context<'_, Self>) {
        prune_filter_groups(&mut self.state.filter_draft);
        self.filter_value_inputs.clear();
        self.filter_field_combos.clear();
        self.filter_operator_combos.clear();
        self.filter_date_picker = None;
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

    /// Toggle the enable checkbox beside a filter node's `并且/或者` control: a disabled condition
    /// (or group, with its whole subtree) is skipped when the filter is applied.
    pub(super) fn filter_toggle_enabled(&mut self, path: Vec<usize>, cx: &mut Context<'_, Self>) {
        if let Some(node) = filter_node_mut(&mut self.state.filter_draft, &path) {
            let enabled = !node.enabled();
            node.set_enabled(enabled);
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
        let selected = match filter_node(&self.state.filter_draft, &key) {
            Some(FilterNode::Condition(condition)) => condition.column.clone(),
            _ => self
                .state
                .columns
                .first()
                .map(|column| column.name.clone())
                .unwrap_or_default(),
        };
        // The popup menu is widened to fit the longest column name; the trigger stays compact.
        let menu_width = self.field_menu_width();
        let entity = cx.new(move |cx| {
            ComboBox::new(theme, options, selected, menu_width, cx)
                .field_width(FILTER_FIELD_WIDTH)
                .height(FILTER_CONTROL_HEIGHT)
                .text_size(FILTER_CONTROL_TEXT_SIZE)
                .radius(FILTER_CONTROL_RADIUS)
                .on_select(Rc::new(move |value, _window, cx| {
                    let _ = weak.update(cx, |grid, cx| {
                        grid.filter_choose_field(callback_path.clone(), value.to_string(), cx);
                    });
                }))
        });
        self.filter_field_combos.insert(key, entity.clone());
        entity
    }

    /// The dropdown menu width that fits the longest column name, so field names are not
    /// truncated. The trigger keeps its own fixed width; only the popup grows.
    fn field_menu_width(&self) -> f32 {
        let longest = self
            .state
            .columns
            .iter()
            .map(|column| ui::approx_text_width(&column.name))
            .fold(FILTER_FIELD_WIDTH, f32::max);
        (longest + 56.0).clamp(FILTER_FIELD_WIDTH, 420.0)
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
        let selected = match filter_node(&self.state.filter_draft, &key) {
            Some(FilterNode::Condition(condition)) => FilterOperator::ALL
                .iter()
                .position(|operator| *operator == condition.operator)
                .unwrap_or(0)
                .to_string(),
            _ => "0".to_string(),
        };
        let entity = cx.new(move |cx| {
            ComboBox::new(theme, options, selected, FILTER_OPERATOR_WIDTH, cx)
                .field_width(FILTER_OPERATOR_WIDTH)
                .height(FILTER_CONTROL_HEIGHT)
                .text_size(FILTER_CONTROL_TEXT_SIZE)
                .radius(FILTER_CONTROL_RADIUS)
                .on_select(Rc::new(move |value, _window, cx| {
                    let Ok(index) = value.parse::<usize>() else {
                        return;
                    };
                    let Some(operator) = FilterOperator::ALL.get(index).copied() else {
                        return;
                    };
                    let _ = weak.update(cx, |grid, cx| {
                        grid.filter_choose_operator(callback_path.clone(), operator, cx);
                    });
                }))
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

    /// The single-line editor for one filter value slot, created on first use and cached by
    /// `(path, slot)`. A structural change clears the cache, so the paths it is keyed by stay valid.
    /// Using the shared [`TextInput`] means the field gets a caret, mouse selection, clipboard and
    /// IME from gpui-kit instead of the bespoke append-only handling the page boxes still use.
    pub(super) fn filter_value_input(
        &mut self,
        path: &[usize],
        slot: u8,
        value: String,
        cx: &mut Context<'_, Self>,
    ) -> Entity<TextInput> {
        let key = (path.to_vec(), slot);
        if let Some(input) = self.filter_value_inputs.get(&key) {
            return input.clone();
        }
        let theme = self.theme;
        let weak = self.self_weak.clone();
        let change_path = key.0.clone();
        let submit_weak = weak.clone();
        let cancel_weak = weak.clone();
        let input = cx.new(move |cx| {
            TextInput::new(
                theme,
                value,
                TextInputOptions {
                    bare: true,
                    text_size: Some(12.0),
                    placeholder: t!("filter.value_placeholder").to_string().into(),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = weak.update(cx, |grid, cx| {
                    grid.filter_value_changed(change_path.clone(), slot, text, cx);
                });
            }))
            .on_submit(Rc::new(move |_window, cx| {
                let _ = submit_weak.update(cx, |grid, cx| grid.filter_apply(cx));
            }))
            .on_cancel(Rc::new(move |window, cx| {
                // Escape leaves the field (keeping whatever was typed) and returns focus to the
                // grid, matching the old append-only field.
                let _ = cancel_weak.update(cx, |grid, cx| window.focus(&grid.focus, cx));
            }))
        });
        self.filter_value_inputs.insert(key, input.clone());
        input
    }

    /// Mirror a filter value field's text into the draft condition.
    fn filter_value_changed(
        &mut self,
        path: Vec<usize>,
        slot: u8,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(FilterNode::Condition(condition)) =
            filter_node_mut(&mut self.state.filter_draft, &path)
        {
            if slot == 0 {
                condition.value = text.to_string();
            } else {
                condition.value2 = text.to_string();
            }
        }
        cx.notify();
    }

    /// Open the date/time picker of one temporal filter value field, or apply-and-close it when
    /// that field's picker is already open (the value field's "…" toggle).
    pub(super) fn filter_toggle_date_picker(
        &mut self,
        path: Vec<usize>,
        slot: u8,
        value: String,
        has_time: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self
            .filter_date_picker
            .as_ref()
            .is_some_and(|picker| picker.path == path && picker.slot == slot)
        {
            self.filter_date_picker_ok(window, cx);
            return;
        }
        let base = parse_datetime(&value).unwrap_or_else(|| chrono::Local::now().naive_local());
        let date = NaiveDate::from_ymd_opt(base.year(), base.month(), base.day())
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let calendar = cx.new(|cx| {
            let mut state = CalendarState::new(window, cx);
            state.set_date(date, window, cx);
            state
        });
        // `TimePrecision::Second` matches the grid's cell picker and MySQL's rendered
        // `datetime`/`timestamp`; a `date` column gets no time row.
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
        self.filter_date_picker = Some(FilterDatePicker {
            path,
            slot,
            has_time,
            calendar,
            time_field,
        });
        cx.notify();
    }

    /// Write the picker's date/time into the filter value field and close it. Unlike the grid's
    /// cell picker nothing is committed here: the WHERE still runs when 应用 is pressed.
    pub(super) fn filter_date_picker_ok(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(picker) = self.filter_date_picker.take() else {
            return;
        };
        let Some(date) = picker.calendar.read(cx).date().start() else {
            cx.notify();
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
        if let Some(input) = self
            .filter_value_inputs
            .get(&(picker.path.clone(), picker.slot))
            .cloned()
        {
            let text = value.clone();
            input.update(cx, |input, cx| {
                input.set_text(text, cx);
                input.focus_state(window, cx);
            });
        }
        if let Some(FilterNode::Condition(condition)) =
            filter_node_mut(&mut self.state.filter_draft, &picker.path)
        {
            if picker.slot == 0 {
                condition.value = value;
            } else {
                condition.value2 = value;
            }
        }
        cx.notify();
    }

    /// Move the open picker to today (and the current time), leaving it open to confirm.
    pub(super) fn filter_date_picker_today(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let now = chrono::Local::now().naive_local();
        let Some(picker) = self.filter_date_picker.as_ref() else {
            return;
        };
        if let Some(today) = NaiveDate::from_ymd_opt(now.year(), now.month(), now.day()) {
            picker
                .calendar
                .update(cx, |calendar, cx| calendar.set_date(today, window, cx));
        }
        if let Some(field) = picker.time_field.as_ref()
            && let Some(time) = NaiveTime::from_hms_opt(now.hour(), now.minute(), now.second())
        {
            field.update(cx, |field, cx| field.set_time(time, window, cx));
        }
        cx.notify();
    }

    /// Close the picker without touching the value, and return focus to its field.
    pub(super) fn filter_date_picker_cancel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(picker) = self.filter_date_picker.take() else {
            return;
        };
        if let Some(input) = self
            .filter_value_inputs
            .get(&(picker.path, picker.slot))
            .cloned()
        {
            input.update(cx, |input, cx| input.focus_state(window, cx));
        }
        cx.notify();
    }

    /// Dismiss the picker from a click outside it, without moving focus back to the field (the
    /// press was aimed at another control).
    pub(super) fn filter_date_picker_dismiss(&mut self, cx: &mut Context<'_, Self>) {
        if self.filter_date_picker.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn filter_apply(&mut self, cx: &mut Context<'_, Self>) {
        if self.guard_pending(GridNav::Filter, cx) {
            return;
        }
        self.filter_apply_now(cx);
    }

    /// Remove every filter condition and apply the empty filter at once (the panel's 清空).
    pub(super) fn filter_clear(&mut self, cx: &mut Context<'_, Self>) {
        self.state.filter_draft.clear();
        self.after_filter_change(cx);
        self.filter_apply(cx);
    }

    pub(super) fn filter_apply_now(&mut self, cx: &mut Context<'_, Self>) {
        self.sync_filter_controls(cx);
        self.filter_date_picker = None;
        self.state.filters = self.state.filter_draft.clone();
        self.state.page_index = 0;
        self.sync_page_input(cx);
        self.load_page(cx);
    }

    /// Fold every control of the filter panel into the draft before it is applied: a combo pick or
    /// a typed value is normally mirrored at once, but the hosts dispatch their callbacks one
    /// render late, so 应用 must not rely on that having happened already. Without this the panel
    /// could show `RECEIVE_ID` while the applied `WHERE` still held the condition's old `ID`.
    fn sync_filter_controls(&mut self, cx: &App) {
        let fields: Vec<(Vec<usize>, String)> = self
            .filter_field_combos
            .iter()
            .filter_map(|(path, combo)| {
                combo
                    .read(cx)
                    .active_value(cx)
                    .map(|value| (path.clone(), value))
            })
            .collect();
        for (path, column) in fields {
            if let Some(FilterNode::Condition(condition)) =
                filter_node_mut(&mut self.state.filter_draft, &path)
            {
                condition.column = column;
            }
        }

        let operators: Vec<(Vec<usize>, String)> = self
            .filter_operator_combos
            .iter()
            .filter_map(|(path, combo)| {
                combo
                    .read(cx)
                    .active_value(cx)
                    .map(|value| (path.clone(), value))
            })
            .collect();
        for (path, value) in operators {
            let Ok(index) = value.parse::<usize>() else {
                continue;
            };
            let Some(operator) = FilterOperator::ALL.get(index).copied() else {
                continue;
            };
            if let Some(FilterNode::Condition(condition)) =
                filter_node_mut(&mut self.state.filter_draft, &path)
            {
                condition.operator = operator;
            }
        }

        let values: Vec<((Vec<usize>, u8), String)> = self
            .filter_value_inputs
            .iter()
            .filter_map(|(key, input)| {
                input
                    .read(cx)
                    .current_text(cx)
                    .map(|text| (key.clone(), text))
            })
            .collect();
        for ((path, slot), text) in values {
            if let Some(FilterNode::Condition(condition)) =
                filter_node_mut(&mut self.state.filter_draft, &path)
            {
                if slot == 0 {
                    condition.value = text;
                } else {
                    condition.value2 = text;
                }
            }
        }
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
        self.set_page_size_text(self.state.page_size.to_string(), cx);
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

    /// The page-size box's Up/Down step: 100 records up, 100 down, never below one.
    pub(super) fn step_page_size(&mut self, down: bool, cx: &mut Context<'_, Self>) {
        let current = self
            .page_size_text_now(cx)
            .trim()
            .parse::<u64>()
            .unwrap_or(1000);
        let next = if down {
            current.saturating_sub(100).max(1)
        } else {
            current.saturating_add(100)
        };
        self.set_page_size_text(next.to_string(), cx);
    }

    pub(super) fn page_size_apply(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
            return;
        }
        if self.guard_pending(GridNav::PageSize, cx) {
            return;
        }
        self.page_size_apply_now(cx);
    }

    pub(super) fn page_size_apply_now(&mut self, cx: &mut Context<'_, Self>) {
        self.page_size_input = self.page_size_text_now(cx);
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
        self.set_page_size_text(size.to_string(), cx);
        self.sync_page_input(cx);
        self.load_page(cx);
    }

    /// Whether the grid holds edits/inserts that have not been written yet.
    pub(super) fn has_pending_edits(&self) -> bool {
        !self.state.edits.is_empty() || !self.inserts.is_empty()
    }

    /// If there are pending edits, ask the app to confirm before `nav` discards them. Returns
    /// `true` when the action was deferred (and must not run now).
    fn guard_pending(&mut self, nav: GridNav, cx: &mut Context<'_, Self>) -> bool {
        if !self.has_pending_edits() {
            return false;
        }
        let app = self.app.clone();
        let grid_id = self.state.id;
        let _ = app.update(cx, |app, cx| {
            app.unsaved_confirm = Some(PendingAction::GridDiscard { grid_id, nav });
            cx.notify();
        });
        true
    }

    /// Run a deferred grid navigation after the user confirmed discarding pending edits.
    pub(super) fn run_grid_nav(&mut self, nav: GridNav, cx: &mut Context<'_, Self>) {
        match nav {
            GridNav::First => self.first_page_now(cx),
            GridNav::Prev => self.prev_page_now(cx),
            GridNav::Next => self.next_page_now(cx),
            GridNav::Last => self.last_page_now(cx),
            GridNav::Page(target) => {
                self.state.page_index = target;
                self.sync_page_input(cx);
                self.load_page(cx);
            }
            GridNav::Sort => self.sort_apply_now(cx),
            GridNav::HeaderSort { column, descending } => {
                let mut rule = SortRule::new(column);
                rule.descending = descending;
                self.state.sort_rules = vec![rule];
                if self.state.sort_open {
                    self.state.sort_draft = self.state.sort_rules.clone();
                    self.state.sort_selected = Some(0);
                }
                self.state.page_index = 0;
                self.sync_page_input(cx);
                self.load_page(cx);
            }
            GridNav::Filter => self.filter_apply_now(cx),
            GridNav::Refresh => self.refresh_now(cx),
            GridNav::PageSize => self.page_size_apply_now(cx),
        }
    }
}

/// How many pending inserts are anchored inline (cloned under a data row).
fn inline_inserts(inserts: &[PendingInsert]) -> usize {
    inserts
        .iter()
        .filter(|insert| insert.after.is_some())
        .count()
}

/// How many inline inserts render before data row `data`.
fn inserts_before(inserts: &[PendingInsert], data: usize) -> usize {
    inserts
        .iter()
        .filter(|insert| insert.after.is_some_and(|after| after < data))
        .count()
}

/// Map a displayed row position to its owner. Data rows and their cloned rows interleave; the
/// "+" rows (`after: None`) follow the whole page.
pub(super) fn row_origin(
    inserts: &[PendingInsert],
    data_rows: usize,
    display: usize,
) -> Option<GridRow> {
    let inline = inline_inserts(inserts);
    let appended_start = data_rows + inline;
    if display >= appended_start {
        // Appended inserts keep their push order after every data row and clone.
        let mut remaining = display - appended_start;
        for (index, insert) in inserts.iter().enumerate() {
            if insert.after.is_none() {
                if remaining == 0 {
                    return Some(GridRow::Insert(index));
                }
                remaining -= 1;
            }
        }
        return None;
    }
    if data_rows == 0 {
        return None;
    }
    // The first display position of data row d is `d + inserts_before(d)`; it grows strictly, so a
    // binary search finds the row holding `display`. The insert list is tiny, so counting inline
    // rows per probe stays cheap even for a million-row page.
    let mut low = 0usize;
    let mut high = data_rows;
    let mut found = 0usize;
    while low < high {
        let mid = (low + high) / 2;
        if mid + inserts_before(inserts, mid) <= display {
            found = mid;
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    let start = found + inserts_before(inserts, found);
    if display == start {
        return Some(GridRow::Data(found));
    }
    let mut remaining = display - start - 1;
    for (index, insert) in inserts.iter().enumerate() {
        if insert.after == Some(found) {
            if remaining == 0 {
                return Some(GridRow::Insert(index));
            }
            remaining -= 1;
        }
    }
    None
}

/// The display position of a pending insert, or `None` for a stale anchor.
fn display_of_insert(inserts: &[PendingInsert], data_rows: usize, index: usize) -> Option<usize> {
    let insert = inserts.get(index)?;
    match insert.after {
        None => {
            let start = data_rows + inline_inserts(inserts);
            let mut rank = 0;
            for (other, candidate) in inserts.iter().enumerate() {
                if candidate.after.is_none() {
                    if other == index {
                        return Some(start + rank);
                    }
                    rank += 1;
                }
            }
            None
        }
        Some(data) if data < data_rows => {
            let start = data + inserts_before(inserts, data) + 1;
            let mut rank = 0;
            for (other, candidate) in inserts.iter().enumerate() {
                if candidate.after == Some(data) {
                    if other == index {
                        return Some(start + rank);
                    }
                    rank += 1;
                }
            }
            None
        }
        Some(_) => None,
    }
}

/// Build the pending insert row for a clone of `row`: every column's value, except the ones the
/// engine owns (auto-increment and generated columns stay unset, so the engine fills them) and
/// binary cells. The pending insert path carries text only, so writing a BLOB back as its hex
/// preview would corrupt it; leaving it out keeps that failure loud instead of silent. A staged
/// edit wins over the loaded value.
fn duplicate_values(
    columns: &[ColumnInfo],
    column_order: &[usize],
    rows: &[Vec<CellValue>],
    row: usize,
    edits: &BTreeMap<(usize, usize), Option<String>>,
) -> BTreeMap<usize, Option<String>> {
    let mut values = BTreeMap::new();
    let Some(cells) = rows.get(row) else {
        return values;
    };
    for (col, column) in columns.iter().enumerate() {
        if column.auto_increment || column.generated {
            continue;
        }
        if let Some(staged) = edits.get(&(row, col)) {
            values.insert(col, staged.clone());
            continue;
        }
        let Some(cell) = cells.get(column_order.get(col).copied().unwrap_or(col)) else {
            continue;
        };
        if matches!(cell, CellValue::Bytes(_)) {
            continue;
        }
        values.insert(col, cell.as_edit_value());
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, auto_increment: bool, generated: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: "int".to_string(),
            nullable: false,
            primary_key: false,
            auto_increment,
            generated,
            comment: String::new(),
        }
    }

    fn insert(after: Option<usize>) -> PendingInsert {
        PendingInsert {
            values: BTreeMap::new(),
            after,
        }
    }

    #[test]
    fn a_clone_renders_directly_under_its_source_row() {
        let inserts = vec![insert(Some(1))];
        assert_eq!(row_origin(&inserts, 3, 0), Some(GridRow::Data(0)));
        assert_eq!(row_origin(&inserts, 3, 1), Some(GridRow::Data(1)));
        assert_eq!(row_origin(&inserts, 3, 2), Some(GridRow::Insert(0)));
        assert_eq!(row_origin(&inserts, 3, 3), Some(GridRow::Data(2)));
        assert_eq!(row_origin(&inserts, 3, 4), None);
        assert_eq!(display_of_insert(&inserts, 3, 0), Some(2));
    }

    #[test]
    fn clones_of_the_same_row_stack_in_order() {
        let inserts = vec![insert(Some(1)), insert(Some(1)), insert(None)];
        assert_eq!(row_origin(&inserts, 3, 2), Some(GridRow::Insert(0)));
        assert_eq!(row_origin(&inserts, 3, 3), Some(GridRow::Insert(1)));
        assert_eq!(row_origin(&inserts, 3, 4), Some(GridRow::Data(2)));
        assert_eq!(row_origin(&inserts, 3, 5), Some(GridRow::Insert(2)));
        assert_eq!(display_of_insert(&inserts, 3, 1), Some(3));
        assert_eq!(display_of_insert(&inserts, 3, 2), Some(5));
    }

    #[test]
    fn plus_rows_stay_after_the_whole_page() {
        let inserts = vec![insert(None)];
        assert_eq!(row_origin(&inserts, 2, 0), Some(GridRow::Data(0)));
        assert_eq!(row_origin(&inserts, 2, 1), Some(GridRow::Data(1)));
        assert_eq!(row_origin(&inserts, 2, 2), Some(GridRow::Insert(0)));
        assert_eq!(row_origin(&inserts, 2, 3), None);
    }

    #[test]
    fn a_clone_skips_engine_owned_columns_and_binary_values() {
        let columns = vec![
            column("id", true, false),
            column("name", false, false),
            column("total", false, true),
            column("blob", false, false),
            column("note", false, false),
        ];
        let rows = vec![vec![
            CellValue::Int(7),
            CellValue::Text("alice".to_string()),
            CellValue::Int(3),
            CellValue::Bytes(vec![0, 1, 2]),
            CellValue::Null,
        ]];
        let values = duplicate_values(&columns, &[0, 1, 2, 3, 4], &rows, 0, &BTreeMap::new());
        assert_eq!(values.get(&0), None, "auto-increment must stay unset");
        assert_eq!(values.get(&1), Some(&Some("alice".to_string())));
        assert_eq!(values.get(&2), None, "generated columns cannot be written");
        assert_eq!(values.get(&3), None, "binary cells cannot round-trip");
        assert_eq!(values.get(&4), Some(&None), "NULL is copied as NULL");
    }

    #[test]
    fn a_clone_follows_the_field_layout_and_hidden_columns() {
        // Display order: "name" (engine 1), "id" (engine 0), "note" (engine 2).
        let columns = vec![
            column("name", false, false),
            column("id", true, false),
            column("note", false, false),
        ];
        let rows = vec![vec![
            CellValue::Int(7),
            CellValue::Text("alice".to_string()),
            CellValue::Text("hello".to_string()),
        ]];
        let values = duplicate_values(&columns, &[1, 0, 2], &rows, 0, &BTreeMap::new());
        assert_eq!(values.get(&0), Some(&Some("alice".to_string())));
        assert_eq!(values.get(&1), None);
        assert_eq!(values.get(&2), Some(&Some("hello".to_string())));
    }

    #[test]
    fn a_staged_edit_wins_over_the_loaded_value() {
        let columns = vec![column("name", false, false)];
        let rows = vec![vec![CellValue::Text("old".to_string())]];
        let mut edits = BTreeMap::new();
        edits.insert((0, 0), Some("new".to_string()));
        let values = duplicate_values(&columns, &[0], &rows, 0, &edits);
        assert_eq!(values.get(&0), Some(&Some("new".to_string())));
    }
}
