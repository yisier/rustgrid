use super::*;

impl AppView {
    pub(super) fn render_query_view(
        &self,
        query: &QueryTab,
        _window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .px_1()
            .py_0p5()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.query_tool_button(
                "query-save",
                "icons/save.svg",
                t!("query.save").to_string(),
                theme.text,
                theme.text,
                true,
                cx.listener(|this, _event, window, cx| this.begin_save_query(window, cx)),
            ))
            .child(toolbar_separator(theme))
            .child(self.query_tool_button(
                "query-format",
                "icons/format_sql.svg",
                t!("query.format").to_string(),
                theme.text,
                theme.text,
                true,
                cx.listener(|this, _event, _window, cx| this.format_query(cx)),
            ));

        let has_connection = query
            .connection_index
            .and_then(|index| self.connection_arc(index))
            .is_some();
        let run_enabled = has_connection && !query.running;
        // One run button: it runs the selection when there is one, otherwise the whole editor.
        let has_selection = query.caret != query.anchor;
        let run_label = if has_selection {
            t!("query.run_selected").to_string()
        } else {
            t!("query.run").to_string()
        };

        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py_0p5()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.query_connection_combo_element())
            .child(self.query_database_combo_element())
            .child(div().w(px(10.0)).flex_none())
            .child(self.query_tool_button(
                "query-run",
                "icons/run.svg",
                run_label,
                theme.text,
                theme.icon_connection,
                run_enabled,
                cx.listener(move |this, _event, _window, cx| this.run_query(has_selection, cx)),
            ))
            .child(self.query_tool_button(
                "query-stop",
                "icons/stop.svg",
                t!("query.stop").to_string(),
                theme.text_muted,
                theme.danger,
                query.running,
                cx.listener(|this, _event, _window, cx| this.stop_query(cx)),
            ));

        let editor = self.render_query_editor(query, cx).into_any_element();
        let result_grid = query.grid_id.and_then(|id| {
            self.grids
                .iter()
                .find(|grid| grid.read(cx).state.id == id)
                .cloned()
        });
        let has_result_panel = result_grid.is_some() || !matches!(query.result, Loadable::Idle);
        let body: AnyElement = if !has_result_panel {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(editor)
                .into_any_element()
        } else {
            let result_body: AnyElement = match result_grid {
                Some(grid) => grid.into_any_element(),
                None => self.render_query_result(query, cx),
            };
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(editor),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .border_t_1()
                        .border_color(rgb(theme.border))
                        .bg(rgb(theme.editor_bg))
                        .child(result_body),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(toolbar)
            .child(controls)
            .child(body)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn query_tool_button(
        &self,
        id: &'static str,
        icon: &'static str,
        label: String,
        color: u32,
        icon_color: u32,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme;
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(24.0))
            .rounded_sm()
            .text_size(px(12.0))
            .text_color(rgb(color))
            .when(enabled, move |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(on_click)
            .child(
                svg()
                    .path(icon)
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .text_color(rgb(icon_color)),
            )
            .child(label)
    }

    fn query_connection_combo_element(&self) -> AnyElement {
        match self.query_connection_combo.as_ref() {
            Some(combo) => combo.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn query_database_combo_element(&self) -> AnyElement {
        match self.query_database_combo.as_ref() {
            Some(combo) => combo.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// One sortable header cell of the saved-query list.
    fn saved_query_header_cell(
        &self,
        column: SavedQueryColumn,
        label_key: &'static str,
        width: f32,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let (id, active) = match column {
            SavedQueryColumn::Name => (
                "saved-query-sort-name",
                matches!(self.saved_query_sort, Some((SavedQueryColumn::Name, _))),
            ),
            SavedQueryColumn::Connection => (
                "saved-query-sort-connection",
                matches!(
                    self.saved_query_sort,
                    Some((SavedQueryColumn::Connection, _))
                ),
            ),
            SavedQueryColumn::Database => (
                "saved-query-sort-database",
                matches!(self.saved_query_sort, Some((SavedQueryColumn::Database, _))),
            ),
        };
        let descending =
            matches!(self.saved_query_sort, Some((current, true)) if current == column);
        let color = if active { theme.text } else { theme.text_muted };
        let mut cell = div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w(px(width))
            .flex_none()
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(color))
            .hover(move |style| style.text_color(rgb(theme.text)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.toggle_saved_query_sort(column, cx);
            }))
            .child(t!(label_key).to_string());
        if active {
            cell = cell.child(
                svg()
                    .path(if descending {
                        "icons/arrow-down.svg"
                    } else {
                        "icons/arrow-up.svg"
                    })
                    .w(px(10.0))
                    .h(px(10.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );
        }
        cell.into_any_element()
    }

    /// The saved-query list shown under the Queries main tab: a sortable header plus one row per
    /// saved query showing its name, connection and database. Scoped by the connection tree's
    /// selection (a database selected -> that database only; a connection selected -> all of its
    /// databases; otherwise every saved query).
    pub(super) fn render_saved_queries(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let query = self.object_search.trim().to_lowercase();
        let filter = self.saved_query_filter(cx);

        let mut visible: Vec<usize> = self
            .saved_queries
            .iter()
            .enumerate()
            .filter(|(_, saved)| self.saved_query_matches(saved, &filter))
            .filter(|(_, saved)| {
                query.is_empty()
                    || saved.name.to_lowercase().contains(&query)
                    || self
                        .saved_query_connection_name(saved)
                        .to_lowercase()
                        .contains(&query)
                    || saved.database.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect();

        if let Some((column, descending)) = self.saved_query_sort {
            visible.sort_by(|a, b| {
                let left = &self.saved_queries[*a];
                let right = &self.saved_queries[*b];
                let ordering = match column {
                    SavedQueryColumn::Name => {
                        left.name.to_lowercase().cmp(&right.name.to_lowercase())
                    }
                    SavedQueryColumn::Connection => self
                        .saved_query_connection_name(left)
                        .to_lowercase()
                        .cmp(&self.saved_query_connection_name(right).to_lowercase()),
                    SavedQueryColumn::Database => left
                        .database
                        .to_lowercase()
                        .cmp(&right.database.to_lowercase()),
                };
                if descending {
                    ordering.reverse()
                } else {
                    ordering
                }
            });
        }

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(24.0))
            .px_1()
            .flex_none()
            .child(div().w(px(16.0)).flex_none())
            .child(self.saved_query_header_cell(
                SavedQueryColumn::Name,
                "query.column.name",
                SAVED_QUERY_NAME_WIDTH,
                cx,
            ))
            .child(self.saved_query_header_cell(
                SavedQueryColumn::Connection,
                "query.column.connection",
                SAVED_QUERY_CONNECTION_WIDTH,
                cx,
            ))
            .child(self.saved_query_header_cell(
                SavedQueryColumn::Database,
                "query.column.database",
                SAVED_QUERY_DATABASE_WIDTH,
                cx,
            ));

        let mut list = div()
            .id("saved-query-list")
            .flex()
            .flex_col()
            .items_start()
            .flex_1()
            .min_h(px(0.0))
            .px_1()
            .pb_1()
            .overflow_y_scroll();
        let has_rows = !visible.is_empty();
        for index in visible {
            let saved = &self.saved_queries[index];
            let selected = self.saved_query_selected == Some(index);
            let name = saved.name.clone();
            let connection = self.saved_query_connection_name(saved);
            let database = saved.database.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("saved-query-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(22.0))
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(selected, move |style| {
                        style
                            .bg(rgb(theme.tree_selected_bg))
                            .text_color(rgb(theme.tree_selected_text))
                    })
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, event, _window, cx| {
                        let double_click =
                            matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                        this.saved_query_selected = Some(index);
                        if double_click {
                            this.open_saved_query(index, cx);
                        } else {
                            cx.notify();
                        }
                    }))
                    .child(tree_icon("icons/queries.svg", theme.icon_queries))
                    .child(
                        div()
                            .w(px(SAVED_QUERY_NAME_WIDTH))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(name),
                    )
                    .child(
                        div()
                            .w(px(SAVED_QUERY_CONNECTION_WIDTH))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(rgb(theme.text_muted))
                            .child(connection),
                    )
                    .child(
                        div()
                            .w(px(SAVED_QUERY_DATABASE_WIDTH))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(rgb(theme.text_muted))
                            .child(database),
                    ),
            );
        }

        let body: AnyElement = if has_rows {
            list.into_any_element()
        } else {
            div()
                .p_3()
                .text_color(rgb(theme.text_muted))
                .child(t!("common.empty").to_string())
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(header)
            .child(body)
            .into_any_element()
    }
}
