use super::*;
use gpui::HitboxBehavior;

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
            inserts: Vec::new(),
            column_resize: None,
            sort_hover: None,
            sort_field_combos: BTreeMap::new(),
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
                    .chain(self.sort_field_combos.values())
                    .cloned()
                    .collect();
                for combo in combos {
                    combo.update(cx, |combo, cx| combo.set_theme(theme, cx));
                }
            }
        }
        if self.cell_editor_focus_pending {
            if let Some(input) = self.cell_editor.as_ref().map(|editor| editor.input.clone()) {
                let focus = input.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
            self.cell_editor_focus_pending = false;
        }
        if self.page_size_focus_pending {
            window.focus(&self.page_size_focus, cx);
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
        let inserts = self.inserts.clone();
        let data_rows = rows.len();
        let column_count = widths.len();
        // A plain press is about to open the in-place editor on release, so paint that cell as an
        // input right away instead of flashing the blue selection fill. A drag switches to a cell
        // selection (`cell_dragged`), which then paints normally.
        let pressed = self.cell_press;
        let dragged = self.cell_dragged;
        // Used to route the Tab/Shift+Tab cell actions from inside the (weak-context) list
        // closure back into `GridView`.
        let action_weak = self.self_weak.clone();
        let editing_temporal = self
            .date_picker
            .as_ref()
            .map(|picker| (picker.row, picker.col));
        // The cell being edited, with its live input entity. The editor is rendered *inside* that
        // cell (below) rather than as an overlay, so it scrolls with the rows and can never be left
        // floating at a stale offset; the cell's own text is replaced by it.
        let editing_cell = self
            .cell_editor
            .as_ref()
            .map(|editor| (editor.row, editor.col, editor.input.clone()));
        let preview = self
            .cell_editor
            .as_ref()
            .filter(|editor| editor.cells.len() > 1)
            .map(|editor| (editor.cells.clone(), editor.value.clone()));
        let widths = Arc::new(widths);
        let list = uniform_list(
            SharedString::from(format!("grid-rows-{}", self.state.id)),
            data_rows + inserts.len(),
            move |range, _window, _cx| {
                range
                    .map(|row_index| {
                        // Rows past the loaded page are pending inserts (gutter `*`, unset cells).
                        let insert =
                            (row_index >= data_rows).then(|| &inserts[row_index - data_rows]);
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
                                        theme.grid_selection_bg
                                    } else {
                                        base_background
                                    }))
                                    .border_r_1()
                                    .border_color(rgb(theme.border))
                                    .when(insert.is_none() && current_row, move |gutter| {
                                        gutter.child(
                                            svg()
                                                .path("icons/row_marker.svg")
                                                .w(px(7.0))
                                                .h(px(7.0))
                                                .flex_none()
                                                .text_color(rgb(if row_selected {
                                                    theme.grid_selection_text
                                                } else {
                                                    theme.primary
                                                })),
                                        )
                                    })
                                    .when(insert.is_some(), move |gutter| {
                                        gutter.child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(if row_selected {
                                                    theme.grid_selection_text
                                                } else {
                                                    theme.primary
                                                }))
                                                .child("*"),
                                        )
                                    }),
                            );
                        for index in 0..column_count {
                            let width = widths.get(index).copied().unwrap_or(GRID_COLUMN_WIDTH);
                            let selected = selection
                                .is_some_and(|selection| selection.contains(row_index, index));
                            // A pending insert row's staged value (`None` = explicit NULL).
                            let staged = insert.and_then(|row| row.get(&index));
                            let edited = if insert.is_some() {
                                None
                            } else {
                                edits.get(&(row_index, index))
                            };
                            let previewed = preview
                                .as_ref()
                                .filter(|(cells, _)| cells.contains(&(row_index, index)))
                                .map(|(_, value)| value.clone());
                            // The cell that owns the in-place editor reads as a plain input: no
                            // selection fill behind the transparent editor.
                            let editing_here = matches!(
                                &editing_cell,
                                Some((row, col, _)) if (*row, *col) == (row_index, index)
                            );
                            let pressing_here = !dragged && pressed == Some((row_index, index));
                            let cell_background = if editing_here || pressing_here {
                                theme.input_bg
                            } else if selected {
                                theme.grid_selection_bg
                            } else if edited.is_some() || staged.is_some() || previewed.is_some() {
                                theme.cell_edit_bg
                            } else {
                                base_background
                            };
                            let is_null = if insert.is_some() {
                                !matches!(staged, Some(Some(_)))
                            } else {
                                let cell = rows[row_index].get(index);
                                match (edited, &previewed) {
                                    (Some(Some(_)), _) | (_, Some(_)) => false,
                                    (Some(None), _) => true,
                                    (None, None) => matches!(cell, Some(CellValue::Null)),
                                }
                            };
                            let display = if insert.is_some() {
                                match staged {
                                    Some(Some(value)) => value.clone(),
                                    _ => "(Null)".to_string(),
                                }
                            } else if let Some(value) = previewed {
                                value
                            } else {
                                match edited {
                                    Some(Some(value)) => value.clone(),
                                    Some(None) => "(Null)".to_string(),
                                    None => {
                                        if is_null {
                                            "(Null)".to_string()
                                        } else {
                                            rows[row_index][index].as_display()
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
                            if editing_here || pressing_here {
                                // The in-place editor reads as a normal field, not as a null cell.
                                cell_element = cell_element.text_color(rgb(theme.text));
                            } else if selected {
                                cell_element =
                                    cell_element.text_color(rgb(theme.grid_selection_text));
                            } else if is_null {
                                cell_element = cell_element
                                    .text_color(rgb(theme.text_null))
                                    .font_weight(FontWeight::THIN);
                            }
                            if editing_here {
                                let next_weak = action_weak.clone();
                                let prev_weak = action_weak.clone();
                                cell_element = cell_element
                                    .key_context(GRID_CELL_CONTEXT)
                                    .on_action(move |_: &NextCell, window, cx| {
                                        let _ = next_weak.update(cx, |grid, cx| {
                                            grid.cell_editor_tab(false, window, cx);
                                        });
                                    })
                                    .on_action(move |_: &PrevCell, window, cx| {
                                        let _ = prev_weak.update(cx, |grid, cx| {
                                            grid.cell_editor_tab(true, window, cx);
                                        });
                                    });
                            }
                            match &editing_cell {
                                Some((row, col, input)) if (*row, *col) == (row_index, index) => {
                                    cell_element = cell_element.child(input.clone());
                                }
                                _ => {
                                    cell_element = cell_element.child(display);
                                }
                            }
                            row_element = row_element.child(cell_element);
                        }
                        row_element
                    })
                    .collect::<Vec<_>>()
            },
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::FitList)
        .track_scroll(&self.list_scroll)
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
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_w(px(0.0))
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
                    // The wheel handler must ignore events aimed at an overlay above the grid —
                    // the combobox popup covers the window with `.occlude()`. A plain rectangle
                    // test on the grid bounds cannot see that, so insert a hitbox and use gpui's
                    // occlusion-aware `should_handle_scroll` instead.
                    canvas(
                        |bounds, window, _cx| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                        move |_bounds, hitbox, window, _cx| {
                            window.on_mouse_event(
                                move |event: &ScrollWheelEvent, phase, window, cx| {
                                    if phase != DispatchPhase::Capture
                                        || !hitbox.should_handle_scroll(window)
                                    {
                                        return;
                                    }
                                    let delta = match event.delta {
                                        ScrollDelta::Lines(delta) => delta.y * GRID_ROW_HEIGHT,
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
