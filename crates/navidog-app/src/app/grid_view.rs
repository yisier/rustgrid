use super::*;

impl GridView {
    pub(super) fn new(
        state: GridState,
        app: WeakEntity<AppView>,
        runtime: Arc<Runtime>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let page_input = (state.page_index + 1).to_string();
        let page_size_input = state.page_size.to_string();
        Self {
            state,
            app,
            runtime,
            theme,
            hscroll: ScrollHandle::new(),
            hscroll_grab: None,
            list_scroll: UniformListScrollHandle::new(),
            vscroll_grab: None,
            focus: cx.focus_handle(),
            selecting_cells: false,
            cell_press: None,
            cell_dragged: false,
            cell_editor: None,
            cell_editor_blur_subscription: None,
            cell_editor_focus_pending: false,
            date_picker: None,
            column_resize: None,
            sort_hover: None,
            sort_search: None,
            sort_combo_filter: String::new(),
            sort_combo_highlight: 0,
            filter_value_focus: Vec::new(),
            filter_value2_focus: Vec::new(),
            filter_active: None,
            filter_field_combos: BTreeMap::new(),
            filter_operator_combos: BTreeMap::new(),
            page_input,
            page_input_focus: cx.focus_handle(),
            page_input_focused: false,
            page_size_menu_open: false,
            page_size_input,
            page_size_focus: cx.focus_handle(),
            page_size_focus_pending: false,
            page_size_focused: false,
            caret_visible: true,
            caret_blink_running: false,
            self_weak: cx.weak_entity(),
            ime_field: None,
            ime_marked: None,
        }
    }

    /// Read the theme from `AppView`, keep focus flags and the caret blink in sync, and apply
    /// any pending focus requests. Called at the top of `render`.
    fn prepare_frame(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(app) = self.app.upgrade() {
            let theme = app.read(cx).theme;
            if theme != self.theme {
                self.theme = theme;
                if let Some(input) = self.cell_editor.as_ref().map(|editor| editor.input.clone()) {
                    input.update(cx, |input, cx| input.set_theme(theme, cx));
                }
                let combos: Vec<Entity<ComboBox>> = self
                    .filter_field_combos
                    .values()
                    .chain(self.filter_operator_combos.values())
                    .cloned()
                    .collect();
                for combo in combos {
                    combo.update(cx, |combo, cx| combo.set_theme(theme, cx));
                }
                if let Some(input) = self.sort_search.clone() {
                    input.update(cx, |input, cx| input.set_theme(theme, cx));
                }
            }
        }
        if self.cell_editor_focus_pending {
            if let Some(input) = self.cell_editor.as_ref().map(|editor| editor.input.clone()) {
                let focus = input.read(cx).focus_handle();
                window.focus(&focus);
            }
            self.cell_editor_focus_pending = false;
        }
        if self.page_size_focus_pending {
            window.focus(&self.page_size_focus);
            self.page_size_focus_pending = false;
        }
        self.page_input_focused = self.page_input_focus.is_focused(window);
        self.page_size_focused = self.page_size_focus.is_focused(window);
        self.update_blink(cx);
    }

    fn has_focused_input(&self) -> bool {
        self.page_input_focused || self.page_size_focused
    }

    fn update_blink(&mut self, cx: &mut Context<'_, Self>) {
        if !self.has_focused_input() || self.caret_blink_running {
            return;
        }
        self.caret_blink_running = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(Duration::from_millis(530)).await;
                let keep_going = this.update(cx, |grid, cx| {
                    if grid.has_focused_input() {
                        grid.caret_visible = !grid.caret_visible;
                        cx.notify();
                        true
                    } else {
                        grid.caret_blink_running = false;
                        grid.caret_visible = true;
                        false
                    }
                });
                if !matches!(keep_going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn caret(&self) -> bool {
        self.caret_visible
    }
}

impl Render for GridView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.prepare_frame(window, cx);
        let theme = self.theme;
        let widths: Vec<f32> = if self.state.column_widths.is_empty() {
            self.state
                .columns
                .iter()
                .map(|_| GRID_COLUMN_WIDTH)
                .collect()
        } else {
            self.state.column_widths.clone()
        };
        let content_width: f32 = (GRID_GUTTER_WIDTH + widths.iter().sum::<f32>()).max(1.0);

        let mut header = div()
            .flex()
            .flex_row()
            .flex_none()
            .bg(rgb(theme.header_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .w(px(GRID_GUTTER_WIDTH))
                    .h(px(GRID_ROW_HEIGHT))
                    .flex_none()
                    .border_r_1()
                    .border_color(rgb(theme.grid_line)),
            );
        for (index, column) in self.state.columns.iter().enumerate() {
            let width = widths.get(index).copied().unwrap_or(GRID_COLUMN_WIDTH);
            let sort_descending = self
                .state
                .sort_rules
                .iter()
                .find(|rule| rule.enabled && rule.column == column.name)
                .map(|rule| rule.descending);
            let selected_column = self.state.selection.is_some_and(|selection| {
                let (start, end) = selection.cols();
                index >= start && index <= end
            });
            let hovered = self.sort_hover == Some(index);
            let revealed = hovered || selected_column;
            let sort_column = column.name.clone();
            let show_badge = revealed || sort_descending.is_some();
            let (sort_icon, sort_color) = match sort_descending {
                Some(true) => ("icons/arrow-down.svg", theme.primary),
                Some(false) => ("icons/arrow-up.svg", theme.primary),
                None => ("icons/sort-none.svg", theme.text_muted),
            };
            let mut slot = div()
                .id(SharedString::from(format!(
                    "grid-sort-badge-{}-{}",
                    self.state.id, index
                )))
                .flex()
                .items_center()
                .justify_center()
                .w(px(13.0))
                .h(px(13.0))
                .ml_1()
                .flex_none();
            if show_badge {
                slot = slot
                    .cursor_pointer()
                    .rounded_sm()
                    .hover(move |style| style.bg(rgb(theme.tree_selected_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.toggle_column_sort(sort_column.clone(), cx);
                    }))
                    .child(
                        svg()
                            .path(sort_icon)
                            .w(px(8.0))
                            .h(px(8.0))
                            .flex_none()
                            .text_color(rgb(sort_color)),
                    );
            }
            let cell = div()
                .id(SharedString::from(format!(
                    "grid-head-{}-{}",
                    self.state.id, index
                )))
                .flex()
                .items_center()
                .h(px(GRID_ROW_HEIGHT))
                .w(px(width))
                .flex_none()
                .pr(px(4.0))
                .whitespace_nowrap()
                .overflow_hidden()
                .cursor_pointer()
                .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                .border_r_1()
                .border_color(rgb(theme.grid_line))
                .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                    this.set_sort_hover(index, *hovered, cx);
                }))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    let rows = this.state.rows.len();
                    if rows > 0 && index < this.state.columns.len() {
                        this.state.selection = Some(CellSelection {
                            anchor: (rows - 1, index),
                            cursor: (0, index),
                        });
                    }
                    this.selecting_cells = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .child(column.name.clone()),
                )
                .child(slot)
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "grid-resize-{}-{}",
                            self.state.id, index
                        )))
                        .absolute()
                        .right(px(0.0))
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .w(px(5.0))
                        .cursor(CursorStyle::ResizeColumn)
                        .when(
                            self.column_resize.is_some_and(|resize| resize.col == index),
                            |handle| handle.bg(rgb(theme.primary)),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                                this.begin_column_resize(index, event.position.x, cx);
                            }),
                        ),
                );
            header = header.child(cell);
        }

        let selection = self.state.selection;
        let edits = self.state.edits.clone();
        let rows = self.state.rows.clone();
        let column_count = widths.len();
        let editing_temporal = self
            .date_picker
            .as_ref()
            .map(|picker| (picker.row, picker.col));
        let preview = self
            .cell_editor
            .as_ref()
            .filter(|editor| editor.cells.len() > 1)
            .map(|editor| (editor.cells.clone(), editor.value.clone()));
        let widths = Arc::new(widths);
        let list = uniform_list(
            SharedString::from(format!("grid-rows-{}", self.state.id)),
            rows.len(),
            move |range, _window, _cx| {
                range
                    .map(|row_index| {
                        let row = &rows[row_index];
                        let base_background = if row_index % 2 == 1 {
                            theme.row_alt_bg
                        } else {
                            theme.editor_bg
                        };
                        let row_selected = selection.is_some_and(|selection| {
                            let (start_row, end_row) = selection.rows();
                            let (start_col, end_col) = selection.cols();
                            row_index >= start_row
                                && row_index <= end_row
                                && start_col == 0
                                && end_col + 1 == column_count
                        });
                        let current_row =
                            selection.is_some_and(|selection| selection.cursor.0 == row_index);
                        let mut row_element = div()
                            .flex()
                            .flex_row()
                            .bg(rgb(base_background))
                            .h(px(GRID_ROW_HEIGHT))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .h(px(GRID_ROW_HEIGHT))
                                    .w(px(GRID_GUTTER_WIDTH))
                                    .flex_none()
                                    .bg(rgb(if row_selected {
                                        theme.tree_selected_bg
                                    } else {
                                        base_background
                                    }))
                                    .border_r_1()
                                    .border_color(rgb(theme.border))
                                    .when(current_row, move |gutter| {
                                        gutter.child(
                                            svg()
                                                .path("icons/row_marker.svg")
                                                .w(px(7.0))
                                                .h(px(7.0))
                                                .flex_none()
                                                .text_color(rgb(if row_selected {
                                                    theme.tree_selected_text
                                                } else {
                                                    theme.primary
                                                })),
                                        )
                                    }),
                            );
                        for (index, cell) in row.iter().enumerate() {
                            let width = widths.get(index).copied().unwrap_or(GRID_COLUMN_WIDTH);
                            let selected = selection
                                .is_some_and(|selection| selection.contains(row_index, index));
                            let edited = edits.get(&(row_index, index));
                            let previewed = preview
                                .as_ref()
                                .filter(|(cells, _)| cells.contains(&(row_index, index)))
                                .map(|(_, value)| value.clone());
                            let cell_background = if selected {
                                theme.tree_selected_bg
                            } else if edited.is_some() || previewed.is_some() {
                                theme.cell_edit_bg
                            } else {
                                base_background
                            };
                            let is_null = match (edited, &previewed) {
                                (Some(Some(_)), _) | (_, Some(_)) => false,
                                (Some(None), _) => true,
                                (None, None) => matches!(cell, CellValue::Null),
                            };
                            let display = if let Some(value) = previewed {
                                value
                            } else {
                                match edited {
                                    Some(Some(value)) => value.clone(),
                                    Some(None) => "(Null)".to_string(),
                                    None => {
                                        if is_null {
                                            "(Null)".to_string()
                                        } else {
                                            cell.as_display()
                                        }
                                    }
                                }
                            };
                            let mut cell_element = div()
                                .flex()
                                .items_center()
                                .h(px(GRID_ROW_HEIGHT))
                                .w(px(width))
                                .flex_none()
                                .pr(px(4.0))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .border_r_1()
                                .border_b_1()
                                .border_color(rgb(theme.grid_line))
                                .bg(rgb(cell_background));
                            if editing_temporal == Some((row_index, index)) {
                                cell_element = cell_element.relative().pr(px(22.0)).child(
                                    div()
                                        .absolute()
                                        .right(px(1.0))
                                        .top(px(1.0))
                                        .bottom(px(1.0))
                                        .w(px(18.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .border_1()
                                        .border_color(rgb(theme.button_border))
                                        .bg(rgb(theme.button_bg))
                                        .text_size(px(11.0))
                                        .text_color(rgb(theme.text))
                                        .child("…"),
                                );
                            }
                            if selected {
                                cell_element =
                                    cell_element.text_color(rgb(theme.tree_selected_text));
                            } else if is_null {
                                cell_element = cell_element
                                    .text_color(rgb(theme.text_null))
                                    .font_weight(FontWeight::THIN);
                            }
                            row_element = row_element.child(cell_element.child(display));
                        }
                        row_element
                    })
                    .collect::<Vec<_>>()
            },
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::FitList)
        .track_scroll(self.list_scroll.clone())
        .flex_1()
        .min_h(px(0.0))
        .track_focus(&self.focus)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.grid_mouse_down(event, window, cx);
            }),
        )
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            this.grid_key(event, window, cx);
        }));

        let table = div()
            .id("grid-hscroll")
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .track_scroll(&self.hscroll)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .h_full()
                    .w(px(content_width))
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        this.grid_mouse_move(event, cx);
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _event, window, cx| {
                            this.selecting_cells = false;
                            let press = this.cell_press.take();
                            let dragged = this.cell_dragged;
                            this.cell_dragged = false;
                            if let Some((row, col)) = press
                                && !dragged
                            {
                                this.begin_edit((row, col), None, window, cx);
                            }
                            cx.notify();
                        }),
                    )
                    .child(header)
                    .child(list)
                    .child(self.render_cell_editor(cx))
                    .child(self.render_date_picker(cx)),
            );

        let mut root = div().relative().flex().flex_col().size_full();
        if self.state.show_toolbar {
            root = root.child(self.render_grid_toolbar(cx));
            if self.state.filter_open {
                root = root.child(self.render_filter_panel(cx));
            }
            if self.state.sort_open {
                root = root.child(self.render_sort_panel(cx));
            }
        }
        let wheel_weak = cx.weak_entity();
        root = root.child(
            div()
                .relative()
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.0))
                                .min_h(px(0.0))
                                .child(table)
                                .child(self.render_grid_hscrollbar(cx)),
                        )
                        .child(self.render_grid_vscrollbar(cx)),
                )
                .child({
                    let weak = wheel_weak.clone();
                    canvas(
                        |_, _, _| {},
                        move |bounds, _state, window, _cx| {
                            window.on_mouse_event(
                                move |event: &ScrollWheelEvent, phase, _window, cx| {
                                    if phase != DispatchPhase::Capture
                                        || !bounds.contains(&event.position)
                                    {
                                        return;
                                    }
                                    let delta = match event.delta {
                                        ScrollDelta::Lines(delta) => delta.y,
                                        ScrollDelta::Pixels(delta) => f32::from(delta.y),
                                    };
                                    if delta == 0.0 {
                                        return;
                                    }
                                    let position = event.position;
                                    let _ = weak.update(cx, |this, cx| {
                                        if this.scroll_grid_selection(position, delta, cx) {
                                            cx.stop_propagation();
                                        }
                                    });
                                },
                            );
                        },
                    )
                    .absolute()
                    .inset_0()
                }),
        );

        if self.state.show_toolbar && self.page_size_menu_open {
            root = root.child(self.render_record_limit_panel(cx));
        }

        root = root
            .child(self.render_grid_controls(cx))
            .child(self.render_grid_status());

        if self.state.show_toolbar
            && self.state.sort_open
            && let Some((rule, _)) = self.state.sort_combo.as_ref()
        {
            root = root.child(self.render_sort_combo_popup(*rule, cx));
        }
        root.on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
            this.grid_hscroll_drag(event, cx);
            this.grid_vscroll_drag(event, cx);
            this.grid_column_drag(event, cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                let h = this.hscroll_grab.take().is_some();
                let v = this.vscroll_grab.take().is_some();
                let c = this.column_resize.take().is_some();
                if h || v || c {
                    cx.notify();
                }
            }),
        )
    }
}
