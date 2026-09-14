use super::*;

impl AppView {
    pub(super) fn default_query_connection(&self, cx: &Context<'_, Self>) -> Option<usize> {
        if let Some(pane) = self.object_pane.as_ref() {
            let pane = pane.read(cx);
            if matches!(
                self.connections
                    .get(pane.connection_index)
                    .map(|node| &node.status),
                Some(ConnectionStatus::Connected(_))
            ) {
                return Some(pane.connection_index);
            }
        }
        self.connections
            .iter()
            .position(|node| matches!(node.status, ConnectionStatus::Connected(_)))
    }

    pub(super) fn default_query_database(
        &self,
        connection_index: usize,
        cx: &Context<'_, Self>,
    ) -> Option<String> {
        if let Some(pane) = self.object_pane.as_ref() {
            let pane = pane.read(cx);
            if pane.connection_index == connection_index
                && let Some(name) = self.database_name(pane.connection_index, pane.database_index)
            {
                return Some(name);
            }
        }
        self.connections
            .get(connection_index)
            .and_then(|node| node.profile.database.clone())
            .filter(|name| !name.is_empty())
    }

    pub(super) fn open_new_query(&mut self, cx: &mut Context<'_, Self>) {
        let connection_index = self.default_query_connection(cx);
        let database = connection_index.and_then(|index| self.default_query_database(index, cx));

        let id = self.next_query_id;
        self.next_query_id += 1;
        let mut tab = QueryTab::new(id);
        tab.connection_index = connection_index;
        tab.database = database;
        self.queries.push(tab);
        self.active_query = Some(self.queries.len() - 1);
        self.active_grid = None;
        self.query_combo = None;
        self.query_completion = None;
        self.page_size_menu_open = false;
        self.object_search.clear();
        self.notify_object_pane(cx);
        self.query_focus_pending = true;
        self.main_tab = MainTab::Queries;
        cx.notify();
    }

    pub(super) fn activate_query(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.active_query = Some(index);
        self.active_grid = self
            .queries
            .get(index)
            .and_then(|tab| tab.grid_id)
            .and_then(|id| self.grids.iter().position(|grid| grid.id == id));
        self.query_combo = None;
        self.query_completion = None;
        self.selecting_cells = false;
        self.cell_editor = None;
        self.date_picker = None;
        self.query_focus_pending = true;
        self.sync_page_input();
        cx.notify();
    }

    pub(super) fn close_query(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.queries.len() {
            return;
        }
        let active_id = self.active_grid_id();
        let grid_id = self.queries.remove(index).grid_id;
        if let Some(id) = grid_id
            && let Some(position) = self.grids.iter().position(|grid| grid.id == id)
        {
            self.grids.remove(position);
        }
        match self.active_query {
            Some(active) if active == index => {
                self.active_query = if self.queries.is_empty() {
                    None
                } else {
                    Some(index.min(self.queries.len() - 1))
                };
            }
            Some(active) if active > index => self.active_query = Some(active - 1),
            _ => {}
        }
        self.active_grid = self
            .active_query
            .and_then(|active| self.queries.get(active))
            .and_then(|tab| tab.grid_id)
            .and_then(|id| self.grids.iter().position(|grid| grid.id == id))
            .or_else(|| active_id.and_then(|id| self.grids.iter().position(|grid| grid.id == id)));
        self.query_combo = None;
        self.query_completion = None;
        self.sync_page_input();
        cx.notify();
    }

    pub(super) fn close_tab(&mut self, target: TabTarget, cx: &mut Context<'_, Self>) {
        self.tab_menu = None;
        match target {
            TabTarget::Grid(index) => self.close_grid(index, cx),
            TabTarget::Query(index) => self.close_query(index, cx),
        }
    }

    pub(super) fn close_other_tabs(&mut self, target: TabTarget, cx: &mut Context<'_, Self>) {
        self.tab_menu = None;
        match target {
            TabTarget::Grid(keep) => {
                let keep_id = self.grids.get(keep).map(|grid| grid.id);
                while !self.queries.is_empty() {
                    self.close_query(self.queries.len() - 1, cx);
                }
                let mut index = self.grids.len();
                while index > 0 {
                    index -= 1;
                    if Some(self.grids[index].id) != keep_id {
                        self.close_grid(index, cx);
                    }
                }
                if let Some(id) = keep_id
                    && let Some(position) = self.grids.iter().position(|grid| grid.id == id)
                {
                    self.activate_grid(Some(position), cx);
                }
            }
            TabTarget::Query(keep) => {
                let keep_id = self.queries.get(keep).map(|query| query.id);
                while !self.grids.is_empty() {
                    self.close_grid(self.grids.len() - 1, cx);
                }
                let mut index = self.queries.len();
                while index > 0 {
                    index -= 1;
                    if Some(self.queries[index].id) != keep_id {
                        self.close_query(index, cx);
                    }
                }
                if let Some(id) = keep_id
                    && let Some(position) = self.queries.iter().position(|query| query.id == id)
                {
                    self.activate_query(position, cx);
                }
            }
        }
    }

    pub(super) fn close_all_tabs(&mut self, cx: &mut Context<'_, Self>) {
        self.tab_menu = None;
        while !self.queries.is_empty() {
            self.close_query(self.queries.len() - 1, cx);
        }
        while !self.grids.is_empty() {
            self.close_grid(self.grids.len() - 1, cx);
        }
    }

    pub(super) fn query_connection_options(&self) -> Vec<(String, String)> {
        self.connections
            .iter()
            .enumerate()
            .map(|(index, node)| (index.to_string(), node.profile.name.clone()))
            .collect()
    }

    pub(super) fn query_database_options(&self, connection_index: usize) -> Vec<(String, String)> {
        match self
            .connections
            .get(connection_index)
            .map(|node| &node.databases)
        {
            Some(Loadable::Loaded(databases)) => databases
                .iter()
                .map(|database| (database.name.clone(), database.name.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn query_select_combo(
        &mut self,
        kind: QueryCombo,
        value: String,
        cx: &mut Context<'_, Self>,
    ) {
        self.query_combo = None;
        self.query_completion = None;
        let Some(index) = self.active_query else {
            return;
        };
        match kind {
            QueryCombo::Connection => {
                let connection_index = value.parse::<usize>().ok();
                let database =
                    connection_index.and_then(|index| self.default_query_database(index, cx));
                if let Some(tab) = self.queries.get_mut(index) {
                    tab.connection_index = connection_index;
                    tab.database = database;
                }
                if let Some(connection_index) = connection_index
                    && !matches!(
                        self.connections
                            .get(connection_index)
                            .map(|node| &node.status),
                        Some(ConnectionStatus::Connected(_))
                    )
                {
                    self.connect(connection_index, cx);
                }
            }
            QueryCombo::Database => {
                if let Some(tab) = self.queries.get_mut(index) {
                    tab.database = Some(value);
                }
            }
        }
        cx.notify();
    }

    pub(super) fn run_query(
        &mut self,
        explain: bool,
        selected_only: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            return;
        };

        let (selection_start, selection_end) = tab.selection();
        let source = if selected_only && selection_start < selection_end {
            &tab.sql[selection_start..selection_end]
        } else {
            tab.sql.as_str()
        };
        let original = source.trim().to_string();
        if original.is_empty() {
            return;
        }
        let executable = if explain {
            format!("EXPLAIN {}", original.trim_end_matches(';').trim())
        } else {
            original.clone()
        };
        let database = tab.database.clone();
        let connection_name = tab
            .connection_index
            .and_then(|connection_index| self.connections.get(connection_index))
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let inferred = if explain {
            None
        } else {
            sql::infer_single_table(&original)
        };
        let Some(connection) = tab.connection_index.and_then(|i| self.connection_arc(i)) else {
            let old = self.queries.get(index).and_then(|tab| tab.grid_id);
            if let Some(old) = old
                && let Some(position) = self.grids.iter().position(|grid| grid.id == old)
            {
                self.grids.remove(position);
            }
            if let Some(tab) = self.queries.get_mut(index) {
                tab.running = false;
                tab.grid_id = None;
                tab.result = Loadable::Failed(t!("query.not_connected").to_string());
            }
            self.active_grid = None;
            cx.notify();
            return;
        };

        if let Some(tab) = self.queries.get_mut(index) {
            tab.running = true;
            tab.result = Loadable::Loading;
        }
        self.query_completion = None;
        self.query_result_scroll = UniformListScrollHandle::new();
        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let query_connection = connection.clone();
            let query_sql = executable.clone();
            let query_database = database.clone();
            let started = std::time::Instant::now();
            let result = match runtime
                .spawn(async move {
                    query_connection
                        .execute_query(query_database.as_deref(), &query_sql)
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let elapsed = started.elapsed();

            let (result, grid_database, grid_table, editable) = match result {
                Ok(mut query_result) => {
                    let mut editable = false;
                    if query_result.has_result_set
                        && let Some((schema, table)) = inferred.clone()
                    {
                        let target_database =
                            schema.or_else(|| database.clone()).unwrap_or_default();
                        let column_connection = connection.clone();
                        let column_database = target_database.clone();
                        let columns = match runtime
                            .spawn(async move {
                                column_connection.columns(&column_database, &table).await
                            })
                            .await
                        {
                            Ok(inner) => inner.ok(),
                            Err(_) => None,
                        };
                        if let Some(columns) = columns {
                            for column in query_result.columns.iter_mut() {
                                if let Some(found) = columns
                                    .iter()
                                    .find(|candidate| candidate.name == column.name)
                                {
                                    column.primary_key = found.primary_key;
                                    column.data_type = found.data_type.clone();
                                }
                            }
                        }
                        editable = true;
                    }
                    let grid_database = inferred
                        .as_ref()
                        .and_then(|(schema, _)| schema.clone())
                        .or_else(|| database.clone())
                        .unwrap_or_default();
                    let grid_table = inferred
                        .as_ref()
                        .map(|(_, table)| table.clone())
                        .unwrap_or_default();
                    (Ok(query_result), grid_database, grid_table, editable)
                }
                Err(error) => (Err(error), String::new(), String::new(), false),
            };

            let _ = this.update(cx, |view, cx| {
                view.apply_query_result(
                    index,
                    connection_name,
                    grid_database,
                    grid_table,
                    executable,
                    editable,
                    elapsed,
                    result,
                    cx,
                );
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_query_result(
        &mut self,
        index: usize,
        connection_name: String,
        database: String,
        table: String,
        sql: String,
        editable: bool,
        elapsed: std::time::Duration,
        result: Result<QueryResult, Error>,
        cx: &mut Context<'_, Self>,
    ) {
        let active_id = self.active_grid_id();

        if let Some(old) = self.queries.get(index).and_then(|tab| tab.grid_id)
            && let Some(position) = self.grids.iter().position(|grid| grid.id == old)
        {
            self.grids.remove(position);
        }

        match result {
            Ok(query_result) if query_result.has_result_set => {
                let Some(tab) = self.queries.get(index) else {
                    return;
                };
                let Some(connection) = tab.connection_index.and_then(|i| self.connection_arc(i))
                else {
                    return;
                };
                let columns = query_result.columns;
                let rows = query_result.rows;
                let total = rows.len() as u64;
                let column_widths = compute_column_widths(&columns, &rows);
                let id = self.next_grid_id;
                self.next_grid_id += 1;
                self.grids.push(GridState {
                    id,
                    connection,
                    connection_name,
                    database,
                    table,
                    is_view: false,
                    page_index: 0,
                    page_size: total.max(1),
                    loading: false,
                    error: None,
                    columns,
                    rows: Arc::new(rows),
                    column_widths,
                    total_rows: Some(total),
                    selection: None,
                    edits: BTreeMap::new(),
                    undo: Vec::new(),
                    sql: Some(sql),
                    show_toolbar: false,
                    editable,
                    sort_rules: Vec::new(),
                    sort_open: false,
                    sort_draft: Vec::new(),
                    sort_combo: None,
                    sort_selected: None,
                    elapsed: Some(elapsed),
                });
                let grid_index = self.grids.len() - 1;
                if let Some(tab) = self.queries.get_mut(index) {
                    tab.running = false;
                    tab.grid_id = Some(id);
                    tab.result = Loadable::Idle;
                }
                if self.active_query == Some(index) {
                    self.active_grid = Some(grid_index);
                } else {
                    self.active_grid =
                        active_id.and_then(|id| self.grids.iter().position(|grid| grid.id == id));
                }
            }
            Ok(query_result) => {
                if let Some(tab) = self.queries.get_mut(index) {
                    tab.running = false;
                    tab.grid_id = None;
                    tab.result = Loadable::Loaded(query_result);
                }
                self.active_grid =
                    active_id.and_then(|id| self.grids.iter().position(|grid| grid.id == id));
            }
            Err(error) => {
                if let Some(tab) = self.queries.get_mut(index) {
                    tab.running = false;
                    tab.grid_id = None;
                    tab.result = Loadable::Failed(error.to_string());
                }
                self.active_grid =
                    active_id.and_then(|id| self.grids.iter().position(|grid| grid.id == id));
            }
        }

        self.sync_page_input();
        self.query_result_scroll = UniformListScrollHandle::new();
        self.grid_hscroll.set_offset(Point::default());
        cx.notify();
    }

    pub(super) fn stop_query(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        if let Some(tab) = self.queries.get_mut(index) {
            tab.running = false;
        }
        cx.notify();
    }

    pub(super) fn format_query(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_query else {
            return;
        };
        let Some(tab) = self.queries.get_mut(index) else {
            return;
        };
        tab.sql = sql::format(&tab.sql);
        tab.caret = tab.sql.len();
        tab.anchor = tab.caret;
        self.query_completion = None;
        self.caret_visible = true;
        cx.notify();
    }

    pub(super) fn query_completion_items(
        &self,
        connection_index: Option<usize>,
    ) -> Rc<Vec<(String, String)>> {
        {
            let cache = self.query_completion_cache.borrow();
            if let Some(cache) = cache.as_ref()
                && cache.connection_index == connection_index
                && cache.generation == self.completion_generation
            {
                return cache.items.clone();
            }
        }

        let mut items: Vec<(String, String)> = sql::keywords()
            .iter()
            .map(|keyword| (keyword.to_lowercase(), (*keyword).to_string()))
            .collect();

        if let Some(index) = connection_index
            && let Some(node) = self.connections.get(index)
            && let Loadable::Loaded(databases) = &node.databases
        {
            for database in databases {
                if let Loadable::Loaded(tables) = &database.tables {
                    items.extend(
                        tables
                            .iter()
                            .map(|table| (table.name.to_lowercase(), table.name.clone())),
                    );
                }
            }
        }

        items.sort_by(|a, b| a.0.cmp(&b.0));
        items.dedup_by(|a, b| a.0 == b.0);
        let items = Rc::new(items);
        *self.query_completion_cache.borrow_mut() = Some(CompletionCache {
            connection_index,
            generation: self.completion_generation,
            items: items.clone(),
        });
        items
    }

    pub(super) fn refresh_query_completion(&mut self, force: bool) {
        let Some(index) = self.active_query else {
            self.query_completion = None;
            return;
        };
        let Some(tab) = self.queries.get(index) else {
            self.query_completion = None;
            return;
        };

        let caret = tab.caret.min(tab.sql.len());
        let mut start = caret;
        while start > 0 {
            let previous = previous_boundary(&tab.sql, start);
            let character = tab.sql[previous..start].chars().next().unwrap_or(' ');
            if character.is_alphanumeric() || character == '_' || character == '$' {
                start = previous;
            } else {
                break;
            }
        }
        let prefix = tab.sql[start..caret].to_lowercase();
        let connection_index = tab.connection_index;

        if prefix.is_empty() && !force {
            self.query_completion = None;
            return;
        }

        let candidates: Vec<String> = self
            .query_completion_items(connection_index)
            .iter()
            .filter(|(lower, _)| lower.starts_with(&prefix))
            .take(64)
            .map(|(_, original)| original.clone())
            .collect();

        if candidates.is_empty() {
            self.query_completion = None;
            return;
        }
        self.query_completion = Some(Completion {
            candidates,
            selected: 0,
            start,
            end: caret,
        });
    }

    pub(super) fn move_query_completion(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        if let Some(completion) = self.query_completion.as_mut()
            && !completion.candidates.is_empty()
        {
            let length = completion.candidates.len() as isize;
            completion.selected =
                ((completion.selected as isize + delta).rem_euclid(length)) as usize;
            cx.notify();
        }
    }

    pub(super) fn accept_query_completion(
        &mut self,
        candidate: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(completion) = self.query_completion.take() else {
            return;
        };
        let Some(index) = self.active_query else {
            return;
        };
        if let Some(tab) = self.queries.get_mut(index) {
            let end = completion.end.min(tab.sql.len());
            let start = completion.start.min(end);
            tab.sql.replace_range(start..end, &candidate);
            let caret = start + candidate.len();
            tab.caret = caret;
            tab.anchor = caret;
        }
        self.caret_visible = true;
        cx.notify();
    }

    pub(super) fn accept_selected_query_completion(&mut self, cx: &mut Context<'_, Self>) {
        let candidate = self
            .query_completion
            .as_ref()
            .and_then(|completion| completion.candidates.get(completion.selected))
            .cloned();
        if let Some(candidate) = candidate {
            self.accept_query_completion(candidate, cx);
        }
    }

    pub(super) fn styled_sql(&self, text: &str, selection: (usize, usize)) -> StyledText {
        let theme = self.theme;
        let spans = self.sql_spans(text);
        let (selection_start, selection_end) = selection;

        let mut boundaries = Vec::with_capacity(spans.len() * 2 + 4);
        boundaries.push(0);
        boundaries.push(text.len());
        for span in spans.iter() {
            boundaries.push(span.start);
            boundaries.push(span.end);
        }
        if selection_start < selection_end {
            boundaries.push(selection_start);
            boundaries.push(selection_end);
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        let mut highlights = Vec::new();
        // `spans` and `boundaries` are both sorted and non-overlapping, so a single advancing
        // cursor finds the token for every boundary window in linear time.
        let mut span_index = 0usize;
        for window in boundaries.windows(2) {
            let (start, end) = (window[0], window[1]);
            if start >= end {
                continue;
            }
            while span_index < spans.len() && spans[span_index].end <= start {
                span_index += 1;
            }
            let token = spans
                .get(span_index)
                .filter(|span| span.start <= start && end <= span.end)
                .map(|span| span.token);
            let selected =
                selection_start < selection_end && selection_start <= start && end <= selection_end;
            let color = token.map(|token| rgb(sql_token_color(theme, token)).into());
            let background_color = if selected {
                Some(rgb(theme.tree_selected_bg).into())
            } else {
                None
            };
            if color.is_none() && background_color.is_none() {
                continue;
            }
            highlights.push((
                start..end,
                HighlightStyle {
                    color,
                    background_color,
                    ..Default::default()
                },
            ));
        }

        StyledText::new(text.to_string()).with_highlights(highlights)
    }

    /// The tokenizer spans for `text`, reusing the previous result when the text is unchanged.
    fn sql_spans(&self, text: &str) -> Rc<Vec<SqlSpan>> {
        let mut cache = self.sql_highlight_cache.borrow_mut();
        if let Some((cached, spans)) = cache.as_ref()
            && cached == text
        {
            return spans.clone();
        }
        let spans = Rc::new(sql::highlight(text));
        *cache = Some((text.to_string(), spans.clone()));
        spans
    }
}
