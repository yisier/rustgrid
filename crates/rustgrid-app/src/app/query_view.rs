use super::query::query_file_key;
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
        let has_result_panel = query.running
            || query.result_error.is_some()
            || !query.results.is_empty()
            || result_grid.is_some();
        let body: AnyElement = if !has_result_panel {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(editor)
                .into_any_element()
        } else {
            // `active_result` is 0 for the 信息 tab, 1..=n for the n-th result set.
            let result_body: AnyElement = if query.running {
                div()
                    .flex_1()
                    .p_2()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("query.running").to_string())
                    .into_any_element()
            } else if let Some(error) = &query.result_error {
                div()
                    .flex_1()
                    .p_2()
                    .text_color(rgb(theme.danger))
                    .child(error.clone())
                    .into_any_element()
            } else if query.active_result == 0 {
                self.render_query_info(query)
            } else if let Some(grid) = result_grid {
                grid.into_any_element()
            } else {
                div().into_any_element()
            };
            let mut panel = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .border_t_1()
                .border_color(rgb(theme.border))
                .bg(rgb(theme.editor_bg));
            if !query.results.is_empty() {
                panel = panel.child(self.render_query_result_tabs(query, cx));
            }
            panel = panel.child(result_body);
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
                .child(panel)
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

    /// The bottom result tabs: a fixed 信息 tab then one 结果 tag per result set.
    fn render_query_result_tabs(&self, query: &QueryTab, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(26.0))
            .flex_none()
            .px_2()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.query_result_tab(
                "query-result-info",
                t!("query.info_tab").to_string(),
                0,
                query.active_result == 0,
                cx,
            ));
        let mut result_number = 0usize;
        for (index, _) in query.results.iter().enumerate() {
            if query.result_grids.get(index).copied().flatten().is_none() {
                continue;
            }
            result_number += 1;
            bar = bar.child(self.query_result_tab(
                SharedString::from(format!("query-result-{index}")),
                t!("query.result_tab", n = result_number).to_string(),
                result_number,
                query.active_result == result_number,
                cx,
            ));
        }
        bar.into_any_element()
    }

    /// One bottom result tag. `tab` is the value [`AppView::select_query_result`] selects.
    fn query_result_tab(
        &self,
        id: impl Into<SharedString>,
        label: String,
        tab: usize,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        div()
            .id(id.into())
            .flex()
            .items_center()
            .justify_center()
            .h(px(20.0))
            .px_3()
            .rounded(px(4.0))
            .text_size(px(11.5))
            .cursor_pointer()
            .when(active, move |style| {
                style
                    .bg(rgb(theme.editor_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text))
            })
            .when(!active, move |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.text_color(rgb(theme.text)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select_query_result(tab, cx);
            }))
            .child(label)
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
        let mut container = div()
            .id("saved-query-list")
            .relative()
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
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if this.query_rename.is_some() {
                        return;
                    }
                    window.focus(&this.query_list_focus, cx);
                    this.begin_marquee(MarqueeTarget::Queries, event.position, event.modifiers, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.context_menu = Some(ContextMenu {
                        target: ContextTarget::QueryList,
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            // The 平铺 grid draws its own horizontal scrollbar; these keep its drag alive anywhere
            // in the window while the pointer leaves the 14px track.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.query_detail_columns.borrow_mut().drag_resize(event) {
                    cx.notify();
                }
                this.query_grid_drag(event, cx);
                this.drag_marquee(MarqueeTarget::Queries, event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.query_detail_columns.borrow_mut().end_resize() {
                        cx.notify();
                    }
                    this.query_grid_end(cx);
                    this.end_marquee(cx);
                }),
            );

        if self.query_scope(cx).is_none() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .into_any_element();
        }

        let visible = self.sorted_visible_query_files(cx, &self.object_search);
        let body = match self.view_mode(VIEW_PAGE_QUERIES) {
            ViewMode::Detail => self.render_query_detail(&visible, theme, cx),
            ViewMode::Grid => self.render_query_tiles(&visible, theme, cx),
        };
        container = container.child(body);
        if let Some(rect) = self.marquee_rect_for(MarqueeTarget::Queries) {
            container = container.child(rect);
        }
        container.into_any_element()
    }

    /// The content-fitted widths of the Queries 详细列表 columns (名称 / 修改日期 / 文件大小).
    fn query_detail_widths(&self, visible: &[usize]) -> Vec<f32> {
        let mut longest = [
            ui::approx_text_width(&t!("common.name")) + 64.0,
            ui::approx_text_width(&t!("backup.field.modified")),
            ui::approx_text_width(&t!("backup.field.size")),
        ];
        for index in visible {
            let Some(file) = self.query_files.get(*index) else {
                continue;
            };
            longest[0] = longest[0].max(ui::approx_text_width(&file.name) + 64.0);
            longest[1] = longest[1].max(ui::approx_text_width(&query_modified_text(file)));
            longest[2] = longest[2].max(ui::approx_text_width(&human_size(file.size)));
        }
        vec![
            ui::detail_column_width(longest[0], 220.0),
            ui::detail_column_width(longest[1], 140.0),
            ui::detail_column_width(longest[2], 80.0),
        ]
    }

    /// The Queries 详细列表: a sortable table of the in-scope saved queries.
    fn render_query_detail(
        &self,
        visible: &[usize],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let sort = self.query_sort;
        let fitted = self.query_detail_widths(visible);
        let widths = self.query_detail_columns.borrow_mut().resolve(&fitted);
        let mut header = ui::detail_header_row(theme);
        for (index, (id, label, column)) in [
            (
                "query-sort-name",
                t!("common.name").to_string(),
                QuerySortColumn::Name,
            ),
            (
                "query-sort-modified",
                t!("backup.field.modified").to_string(),
                QuerySortColumn::Modified,
            ),
            (
                "query-sort-size",
                t!("backup.field.size").to_string(),
                QuerySortColumn::Size,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let width = widths[index];
            let cell = ui::detail_header_cell(
                id,
                label,
                (sort.column == column).then_some(sort.descending),
                theme,
                cx.listener(move |this, _event, _window, cx| this.toggle_query_sort(column, cx)),
            );
            header = header.child(ui::detail_header_column(
                SharedString::from(format!("query-resize-{index}")),
                width,
                self.query_detail_columns.borrow().resizing(index),
                theme,
                cell,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.query_detail_columns.borrow_mut().begin_resize(
                        index,
                        event.position.x,
                        width,
                    );
                    cx.notify();
                }),
            ));
        }

        let mut list = ui::DetailList::new(
            "query-detail",
            &self.query_detail_scroll,
            ui::detail_content_width(&widths),
            header,
        );
        for &index in visible {
            list = list.child(self.query_detail_row(index, &widths, theme, cx));
        }
        list.render(theme)
    }

    /// One 详细列表 row of a saved query.
    fn query_detail_row(
        &self,
        index: usize,
        widths: &[f32],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let file = &self.query_files[index];
        let key = query_file_key(file);
        let selected = self.query_selection.contains(&key);
        let name: AnyElement = match self.query_rename_input(index) {
            Some(input) => div()
                .w(px(widths[0]))
                .flex_none()
                .h(px(22.0))
                .child(input)
                .into_any_element(),
            None => div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .w(px(widths[0]))
                .flex_none()
                .overflow_hidden()
                .text_color(rgb(theme.text))
                .child(ui::leading_icon_badge(
                    "icons/queries.svg",
                    theme.icon_queries,
                    22.0,
                ))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("query-cell-{index}-0")),
                    file.name.clone(),
                ))
                .child(ui::tag_chip(QUERY_TAG.to_string(), theme))
                .into_any_element(),
        };
        let click_key = key.clone();
        let menu_key = key.clone();
        let row = ui::detail_row(
            SharedString::from(format!("query-file-{index}")),
            selected,
            theme,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            this.hit_query(&click_key, event.click_count(), event.modifiers(), cx);
            if matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2) {
                this.open_saved_query(index, cx);
            }
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                if !this.query_selection.contains(&menu_key) {
                    this.select_query_one(index);
                }
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
                .w(px(widths[1]))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("query-cell-{index}-1")),
                    query_modified_text(file),
                )),
        )
        .child(
            div()
                .w(px(widths[2]))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("query-cell-{index}-2")),
                    human_size(file.size),
                )),
        );
        list_ops::row_with_rect(cx.weak_entity(), row, MarqueeTarget::Queries, key)
    }

    /// The Queries 平铺网格: the in-scope saved queries in a column-major grid (items fill a column
    /// top-to-bottom, then wrap to the next column), scrolling horizontally like the object list.
    fn render_query_tiles(
        &self,
        visible: &[usize],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let width = ui::grid_item_width(
            visible
                .iter()
                .filter_map(|index| self.query_files.get(*index))
                .map(|file| ui::approx_text_width(&file.name))
                .fold(0.0, f32::max),
        );
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
            column = column.child(self.query_grid_item(index, theme, width, cx));
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
        width: f32,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let file = &self.query_files[index];
        let key = query_file_key(file);
        let selected = self.query_selection.contains(&key);
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
        let click_key = key.clone();
        let menu_key = key.clone();
        let item = ui::grid_item_sized(
            SharedString::from(format!("query-tile-{index}")),
            selected,
            theme,
            width,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            this.hit_query(&click_key, event.click_count(), event.modifiers(), cx);
            if matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2) {
                this.open_saved_query(index, cx);
            }
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                if !this.query_selection.contains(&menu_key) {
                    this.select_query_one(index);
                }
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
            16.0,
        ))
        .child(title);
        list_ops::row_with_rect(cx.weak_entity(), item, MarqueeTarget::Queries, key)
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

/// A saved query's modified timestamp, or `--` when the filesystem did not report one.
fn query_modified_text(file: &QueryFileInfo) -> String {
    file.modified
        .map(format_file_time)
        .unwrap_or_else(|| "--".to_string())
}
