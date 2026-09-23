use super::*;

impl AppView {
    pub(super) fn render_query_view(
        &self,
        query: &QueryTab,
        _window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        if query.routine.is_some() {
            return self.render_routine_view(query, cx);
        }
        if query.view.is_some() {
            return self.render_view_view(query, cx);
        }
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
            .into_any_element()
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

    /// The saved-query list shown under the Queries main tab. Like the Backup tab, it is scoped to
    /// the selected (opened) database and behaves like a folder of `.sql` files: double-click opens,
    /// right-click (or F2 / Ctrl+C / Ctrl+V) manages the file. The list offers the shared
    /// 详细列表 / 平铺网格 layouts.
    pub(super) fn render_saved_queries(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let container = div()
            .id("saved-query-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.query_list_focus)
            .key_context(QUERY_LIST_CONTEXT)
            .on_action(cx.listener(|this, _: &RenameQueryFile, window, cx| {
                if let Some(index) = this.saved_query_selected {
                    this.begin_rename_query(index, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopyQueryFile, _window, cx| {
                if let Some(index) = this.saved_query_selected {
                    this.copy_query_file(index);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &PasteQueryFile, _window, cx| {
                this.paste_query_file(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    if this.query_rename.is_none() {
                        window.focus(&this.query_list_focus, cx);
                    }
                }),
            )
            // The 平铺 grid draws its own horizontal scrollbar; these keep its drag alive anywhere
            // in the window while the pointer leaves the 14px track.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.query_grid_drag(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.query_grid_end(cx);
                }),
            );

        if self.query_scope(cx).is_none() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .p_3()
                .text_color(rgb(theme.text_muted))
                .child(t!("query.open_database").to_string())
                .into_any_element();
        }

        let visible = self.sorted_visible_query_files(cx, &self.object_search);
        let body = match self.view_mode(VIEW_PAGE_QUERIES) {
            ViewMode::Detail => self.render_query_detail(&visible, theme, cx),
            ViewMode::Grid => self.render_query_tiles(&visible, theme, cx),
        };
        container.child(body).into_any_element()
    }

    /// The Queries 详细列表: a sortable table of the in-scope saved queries.
    fn render_query_detail(
        &self,
        visible: &[usize],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let sort = self.query_sort;
        let header = ui::detail_header_row(theme)
            .child(div().flex_1().min_w(px(0.0)).child(ui::detail_header_cell(
                "query-sort-name",
                t!("common.name").to_string(),
                (sort.column == QuerySortColumn::Name).then_some(sort.descending),
                theme,
                cx.listener(|this, _event, _window, cx| {
                    this.toggle_query_sort(QuerySortColumn::Name, cx)
                }),
            )))
            .child(
                div()
                    .w(px(QUERY_MODIFIED_WIDTH))
                    .flex_none()
                    .child(ui::detail_header_cell(
                        "query-sort-modified",
                        t!("backup.field.modified").to_string(),
                        (sort.column == QuerySortColumn::Modified).then_some(sort.descending),
                        theme,
                        cx.listener(|this, _event, _window, cx| {
                            this.toggle_query_sort(QuerySortColumn::Modified, cx)
                        }),
                    )),
            )
            .child(
                div()
                    .w(px(QUERY_SIZE_WIDTH))
                    .flex_none()
                    .child(ui::detail_header_cell(
                        "query-sort-size",
                        t!("backup.field.size").to_string(),
                        (sort.column == QuerySortColumn::Size).then_some(sort.descending),
                        theme,
                        cx.listener(|this, _event, _window, cx| {
                            this.toggle_query_sort(QuerySortColumn::Size, cx)
                        }),
                    )),
            );

        let mut body = ui::detail_body();
        if visible.is_empty() {
            body = body.child(query_empty_state(theme));
        }
        for &index in visible {
            body = body.child(self.query_detail_row(index, theme, cx));
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .child(ui::detail_card(theme).child(header).child(body))
            .into_any_element()
    }

    /// One 详细列表 row of a saved query.
    fn query_detail_row(
        &self,
        index: usize,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let file = &self.query_files[index];
        let selected = self.saved_query_selected == Some(index);
        let name: AnyElement = match self.query_rename_input(index) {
            Some(input) => div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(22.0))
                .child(input)
                .into_any_element(),
            None => div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .min_w(px(0.0))
                .text_color(rgb(theme.text))
                .child(ui::leading_icon_badge(
                    "icons/queries.svg",
                    theme.icon_queries,
                    24.0,
                ))
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(file.name.clone()),
                )
                .child(ui::tag_chip(QUERY_TAG.to_string(), theme))
                .into_any_element(),
        };
        ui::detail_row(
            SharedString::from(format!("query-file-{index}")),
            selected,
            theme,
        )
        .on_click(cx.listener(move |this, event, _window, cx| {
            this.saved_query_selected = Some(index);
            if matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2) {
                this.open_saved_query(index, cx);
            }
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.saved_query_selected = Some(index);
                this.context_menu = Some(ContextMenu {
                    target: ContextTarget::QueryFile { index },
                    position: event.position,
                });
                cx.notify();
            }),
        )
        .child(name)
        .child(
            div()
                .w(px(QUERY_MODIFIED_WIDTH))
                .flex_none()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(theme.text_muted))
                .child(query_modified_text(file)),
        )
        .child(
            div()
                .w(px(QUERY_SIZE_WIDTH))
                .flex_none()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(theme.text_muted))
                .child(human_size(file.size)),
        )
    }

    /// The Queries 平铺网格: the in-scope saved queries in a column-major grid (items fill a column
    /// top-to-bottom, then wrap to the next column), scrolling horizontally like the object list.
    fn render_query_tiles(
        &self,
        visible: &[usize],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        if visible.is_empty() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(query_empty_state(theme))
                .into_any_element();
        }
        let rows = self.query_grid.rows_per_column();
        let mut columns = ui::grid_columns();
        let mut column = ui::grid_column();
        let mut count = 0usize;
        for &index in visible {
            if count == rows {
                columns = columns.child(column);
                column = ui::grid_column();
                count = 0;
            }
            column = column.child(self.query_grid_item(index, theme, cx));
            count += 1;
        }
        if count > 0 {
            columns = columns.child(column);
        }

        let mut grid = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden();
        grid = grid.child(self.query_grid.scroller("query-grid-scroll").child(columns));
        if self.query_grid.overflows() {
            grid = grid.child(
                self.query_grid
                    .scrollbar("query-grid-hscrollbar", theme)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.query_grid_begin(event.position.x, cx);
                        }),
                    ),
            );
        }
        grid.into_any_element()
    }

    /// One item of a 平铺网格 column: the query's icon badge and name.
    fn query_grid_item(
        &self,
        index: usize,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let file = &self.query_files[index];
        let selected = self.saved_query_selected == Some(index);
        let title: AnyElement = match self.query_rename_input(index) {
            Some(input) => div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(20.0))
                .child(input)
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(theme.text))
                .child(file.name.clone())
                .into_any_element(),
        };
        ui::grid_item(
            SharedString::from(format!("query-tile-{index}")),
            selected,
            theme,
        )
        .on_click(cx.listener(move |this, event, _window, cx| {
            this.saved_query_selected = Some(index);
            if matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2) {
                this.open_saved_query(index, cx);
            }
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.saved_query_selected = Some(index);
                this.context_menu = Some(ContextMenu {
                    target: ContextTarget::QueryFile { index },
                    position: event.position,
                });
                cx.notify();
            }),
        )
        .child(ui::leading_icon_badge(
            "icons/queries.svg",
            theme.icon_queries,
            20.0,
        ))
        .child(title)
    }

    /// Start dragging the 平铺 grid's horizontal scrollbar.
    fn query_grid_begin(&mut self, mouse_x: Pixels, cx: &mut Context<'_, Self>) {
        if self.query_grid.begin(mouse_x) {
            cx.notify();
        }
    }

    /// Continue dragging the 平铺 grid's horizontal scrollbar.
    fn query_grid_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        if self.query_grid.drag(event) {
            cx.notify();
        }
    }

    /// Finish dragging the 平铺 grid's horizontal scrollbar.
    fn query_grid_end(&mut self, cx: &mut Context<'_, Self>) {
        if self.query_grid.end() {
            cx.notify();
        }
    }

    /// The in-place rename editor for a query file, when it belongs to this row.
    fn query_rename_input(&self, index: usize) -> Option<Entity<TextInput>> {
        self.query_rename
            .as_ref()
            .filter(|edit| edit.index == index)
            .map(|edit| edit.input.clone())
    }
}

/// The tag chip shown beside a saved query's name.
const QUERY_TAG: &str = "SQL";

/// Column widths of the Queries 详细列表, shared by the header and its rows.
const QUERY_MODIFIED_WIDTH: f32 = 190.0;
const QUERY_SIZE_WIDTH: f32 = 110.0;

/// A saved query's modified timestamp, or `--` when the filesystem did not report one.
fn query_modified_text(file: &QueryFileInfo) -> String {
    file.modified
        .map(format_file_time)
        .unwrap_or_else(|| "--".to_string())
}

/// The muted message shown when a query layout has no rows.
fn query_empty_state(theme: Theme) -> AnyElement {
    div()
        .p_3()
        .text_color(rgb(theme.text_muted))
        .child(t!("common.empty").to_string())
        .into_any_element()
}
