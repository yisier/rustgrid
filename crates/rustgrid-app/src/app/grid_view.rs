use super::grid::{make_page_input, make_page_size_input};
use super::grid_fields::make_field_filter_search_input;
use super::*;
use gpui::HitboxBehavior;
use gpui_kit::component::{Icon, IconName};

impl GridView {
    /// The key-value detail header above the grid: type badge, key name, type, TTL and memory.
    /// Shown only for a Redis key (a SQL table grid has no `key_status`).
    fn render_key_header(&self, theme: Theme) -> AnyElement {
        let Some(status) = self.state.key_status.as_ref() else {
            return div().into_any_element();
        };
        let kind = status.engine.clone().unwrap_or_default();
        let ttl = match status.ttl {
            Some(-1) => t!("redis.key.persist").to_string(),
            Some(ttl) => ttl.to_string(),
            None => String::new(),
        };
        let memory = status.data_length.map(human_size).unwrap_or_default();
        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .h(px(34.0))
            .flex_none()
            .px_3()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .text_size(px(12.0))
            .child(super::objects::key_type_badge(&kind, theme))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme.text))
                    .child(self.state.table.clone()),
            );
        if !kind.is_empty() {
            header = header.child(
                div()
                    .text_color(rgb(theme.text_muted))
                    .child(kind.to_uppercase()),
            );
        }
        if !ttl.is_empty() {
            header = header.child(
                div()
                    .text_color(rgb(theme.text_muted))
                    .child(format!("{}: {ttl}", t!("redis.key.ttl"))),
            );
        }
        if !memory.is_empty() {
            header = header.child(
                div()
                    .text_color(rgb(theme.text_muted))
                    .child(format!("{}: {memory}", t!("redis.key.memory"))),
            );
        }
        header.into_any_element()
    }

    pub(super) fn new(
        state: GridState,
        app: WeakEntity<AppView>,
        runtime: Arc<Runtime>,
        theme: Theme,
        clipboard: Rc<RefCell<GridClipboard>>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let page_input = (state.page_index + 1).to_string();
        let page_size_input = state.page_size.to_string();
        let weak = cx.weak_entity();
        let field_filter_search_input = make_field_filter_search_input(theme, weak.clone(), cx);
        let page_input_view = make_page_input(theme, page_input.clone(), weak.clone(), cx);
        let page_size_view = make_page_size_input(theme, page_size_input.clone(), weak, cx);
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
            cell_press_reselect: false,
            value_editor: None,
            cell_dragged: false,
            cell_editor: None,
            cell_editor_blur_subscription: None,
            cell_editor_focus_pending: false,
            date_picker: None,
            key_string_format: KeyStringFormat::Raw,
            key_string_editor: None,
            key_json_editor: None,
            key_string_text: String::new(),
            key_string_loaded: None,
            key_string_dirty: false,
            key_string_saving: false,
            key_string_error: None,
            inserts: Vec::new(),
            column_resize: None,
            sort_hover: None,
            sort_field_combos: BTreeMap::new(),
            clipboard,
            refresh_feedback: false,
            filter_value_inputs: BTreeMap::new(),
            filter_field_combos: BTreeMap::new(),
            filter_operator_combos: BTreeMap::new(),
            filter_width: None,
            filter_available_width: 0.0,
            filter_rendered_width: FILTER_CONTENT_WIDTH_DEFAULT,
            filter_resize: None,
            page_input,
            page_input_view,
            page_size_menu_open: false,
            page_size_input,
            page_size_view,
            page_size_focus_pending: false,
            field_filter_open: false,
            field_filter_focus_pending: false,
            field_filter_search: String::new(),
            field_filter_search_input,
            field_filter_draft: GridFieldLayout::default(),
            field_filter_drag: None,
            field_filter_anchor: Rc::new(RefCell::new(Point::default())),
            grid_pane_bounds: Rc::new(RefCell::new(Bounds::default())),
            status_sql_focus: cx.focus_handle(),
            status_sql_layout: TextLayout::default(),
            status_sql_text: String::new(),
            status_sql_anchor: 0,
            status_sql_cursor: 0,
            status_sql_selecting: false,
            empty_grid_height: 0.0,
            self_weak: cx.weak_entity(),
            sum_cache: RefCell::new(None),
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
            self.page_size_view
                .update(cx, |input, cx| input.focus_state(window, cx));
            self.page_size_focus_pending = false;
        }
        if self.field_filter_focus_pending {
            self.field_filter_search_input
                .update(cx, |input, cx| input.focus_state(window, cx));
            self.field_filter_focus_pending = false;
        }
    }

    /// Whether the grid header paints each field's comment under its name: the 数据 option is on
    /// *and* this grid actually has a commented column, so a table without comments keeps the
    /// compact single-line header.
    pub(super) fn show_column_comments(&self, cx: &Context<'_, Self>) -> bool {
        self.app
            .upgrade()
            .is_some_and(|app| app.read(cx).show_column_comments())
            && self
                .state
                .columns
                .iter()
                .any(|column| !column.comment.is_empty())
    }

    /// Whether the grid header paints each field's data type between its name and comment: the
    /// 数据 option is on *and* this grid actually has a typed column (query results may not).
    pub(super) fn show_column_types(&self, cx: &Context<'_, Self>) -> bool {
        self.app
            .upgrade()
            .is_some_and(|app| app.read(cx).show_column_types())
            && self
                .state
                .columns
                .iter()
                .any(|column| !column.data_type.is_empty())
    }

    /// The header's height: one line plus 14px for each of the field's type and comment lines the
    /// 数据 options paint under the name.
    pub(super) fn header_height(&self, cx: &Context<'_, Self>) -> f32 {
        let extra_lines =
            usize::from(self.show_column_types(cx)) + usize::from(self.show_column_comments(cx));
        GRID_ROW_HEIGHT + extra_lines as f32 * GRID_HEADER_LINE_HEIGHT
    }
}

impl Render for GridView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.prepare_frame(window, cx);
        let theme = self.theme;
        let key_page = self.is_key_page();
        if key_page && self.is_string_key_page() {
            return self.render_string_key_page(cx);
        }
        if key_page {
            self.layout_key_page_columns();
        }
        let header_height = if key_page {
            GRID_ROW_HEIGHT
        } else {
            self.header_height(cx)
        };
        let show_types = !key_page && self.show_column_types(cx);
        let show_comments = !key_page && self.show_column_comments(cx);
        // The hidden fields are kept at the tail of `columns`/`column_widths`, so only the leading
        // `visible_columns` entries are drawn.
        let all_widths: Vec<f32> = if self.state.column_widths.len() == self.state.columns.len() {
            self.state.column_widths.clone()
        } else {
            self.state
                .columns
                .iter()
                .map(|_| GRID_COLUMN_WIDTH)
                .collect()
        };
        let visible_count = self.state.visible_columns.min(all_widths.len());
        let widths: Vec<f32> = all_widths[..visible_count].to_vec();
        let gutter_width = self.gutter_width();
        let content_width: f32 = (gutter_width + widths.iter().sum::<f32>()).max(1.0);

        let mut header = div()
            .flex()
            .flex_row()
            .flex_none()
            .bg(rgb(theme.header_bg))
            .border_b_1()
            .border_color(rgb(theme.border));
        if gutter_width > 0.0 {
            header = header.child(
                div()
                    .w(px(gutter_width))
                    .h(px(header_height))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .border_r_1()
                    .border_color(rgb(theme.grid_line)),
            );
        }
        for (index, column) in self.state.columns.iter().enumerate().take(visible_count) {
            let width = widths.get(index).copied().unwrap_or(GRID_COLUMN_WIDTH);
            let sort_descending = self
                .state
                .sort_rules
                .iter()
                .find(|rule| rule.enabled && rule.column == column.name)
                .map(|rule| rule.descending);
            let selected_column = self.state.selection.as_ref().is_some_and(|selection| {
                let (start, end) = selection.cols();
                index >= start && index <= end
            });
            let hovered = self.sort_hover == Some(index);
            let revealed = hovered || selected_column;
            let sort_column = column.name.clone();
            // A key page has no sort: field/member order is the server's own, so no sort badge.
            let show_badge =
                !key_page && self.state.sql.is_none() && (revealed || sort_descending.is_some());
            let (sort_icon, sort_color, sort_opacity) = match sort_descending {
                Some(true) => (IconName::SortDescending, theme.primary, 1.0),
                Some(false) => (IconName::SortAscending, theme.primary, 1.0),
                None => (IconName::ChevronsUpDown, theme.text_muted, 0.5),
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
                        Icon::new(sort_icon)
                            .w(px(12.0))
                            .h(px(12.0))
                            .flex_none()
                            .text_color(rgb(sort_color))
                            .opacity(sort_opacity),
                    );
            }
            let cell = div()
                .id(SharedString::from(format!(
                    "grid-head-{}-{}",
                    self.state.id, index
                )))
                .flex()
                .items_center()
                .h(px(header_height))
                .w(px(width))
                .flex_none()
                .px(px(6.0))
                .whitespace_nowrap()
                .overflow_hidden()
                .when(!key_page, |head| {
                    head.cursor_pointer()
                        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                        .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                            this.set_sort_hover(index, *hovered, cx);
                        }))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            let rows = this.state.rows.len();
                            if rows > 0 && index < this.state.visible_columns {
                                this.state.selection =
                                    Some(CellSelection::single((rows - 1, index), (0, index)));
                            }
                            this.selecting_cells = false;
                            cx.notify();
                        }))
                })
                .border_r_1()
                .border_color(rgb(theme.grid_line))
                .child(if key_page {
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(theme.text_muted))
                        .child(self.key_column_label(&column.name))
                        .into_any_element()
                } else {
                    header_cell_text(
                        SharedString::from(format!("grid-head-text-{}-{}", self.state.id, index)),
                        column,
                        width,
                        show_types,
                        show_comments,
                        theme,
                    )
                })
                .when(!key_page, |head| head.child(slot))
                .when(!key_page, |head| {
                    head.child(
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
                    )
                });
            header = header.child(cell);
        }

        let selection = self.state.selection.clone();
        let edits = self.state.edits.clone();
        let rows = self.state.rows.clone();
        // Display column -> the row's own (engine-order) cell index.
        let column_order = Arc::new(self.state.column_order.clone());
        let inserts = self.inserts.clone();
        let data_rows = rows.len();
        let column_count = widths.len();
        // Whether the grid has nothing to paint, captured before `inserts` moves into the list
        // closure below (the empty body is drawn instead of an empty list).
        let empty_page = data_rows + inserts.len() == 0;
        // A plain press is about to open the in-place editor on release, so paint that cell as an
        // input right away instead of flashing the blue selection fill. A drag switches to a cell
        // selection (`cell_dragged`), which then paints normally.
        let pressed = self.cell_press;
        let dragged = self.cell_dragged;
        // Used to route the Tab/Shift+Tab cell actions from inside the (weak-context) list
        // closure back into `GridView`.
        let action_weak = self.self_weak.clone();
        // The cell being edited when its column is temporal. Such a cell carries a "…" button
        // that toggles the date/time popup, whether or not the popup is currently open.
        let editing_temporal = self.cell_editor.as_ref().and_then(|editor| {
            self.state
                .columns
                .get(editor.col)
                .is_some_and(|column| is_temporal_type(&column.data_type))
                .then_some((editor.row, editor.col))
        });
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
        // The empty-table filler needs the widths after the list closure below has taken the Arc.
        let filler_widths = Arc::clone(&widths);
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
                        let row_selected = selection.as_ref().is_some_and(|selection| {
                            let (start_row, end_row) = selection.rows();
                            let (start_col, end_col) = selection.cols();
                            row_index >= start_row
                                && row_index <= end_row
                                && start_col == 0
                                && end_col + 1 == column_count
                        });
                        let current_row = selection
                            .as_ref()
                            .is_some_and(|selection| selection.active_cursor().0 == row_index);
                        let mut row_element = div()
                            .flex()
                            .flex_row()
                            .bg(rgb(base_background))
                            .h(px(GRID_ROW_HEIGHT))
                            .when(!key_page, |row| {
                                row.child(
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
                                )
                            });
                        for index in 0..column_count {
                            let width = widths.get(index).copied().unwrap_or(GRID_COLUMN_WIDTH);
                            let selected = selection
                                .as_ref()
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
                                let cell = source_cell(&rows, &column_order, row_index, index);
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
                                            source_cell(&rows, &column_order, row_index, index)
                                                .map(|cell| cell.as_display())
                                                .unwrap_or_default()
                                        }
                                    }
                                }
                            };
                            // A cell is exactly one row tall and truncates horizontally, so a value
                            // that carries embedded line breaks (long PEM/base64 text, `TEXT`
                            // columns) must be flattened first — otherwise its extra lines spill
                            // over the rows below and the grid reads as garbled. The raw value is
                            // kept for the editor and for copy/paste; only the display is collapsed.
                            // Avoid the extra allocation when the value has no line break.
                            let display = if display.contains(['\r', '\n', '\t']) {
                                single_line(&display)
                            } else {
                                display
                            };
                            let mut cell_element = div()
                                .flex()
                                .items_center()
                                .h(px(GRID_ROW_HEIGHT))
                                .w(px(width))
                                .flex_none()
                                // Horizontal padding keeps a value off its neighbour's divider (and
                                // off the previous column's text), so bordered cells read as
                                // separate values instead of one run of characters.
                                .px(px(6.0))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .border_r_1()
                                .border_b_1()
                                .border_color(rgb(theme.grid_line))
                                .bg(rgb(cell_background));
                            if editing_temporal == Some((row_index, index)) {
                                let toggle_weak = action_weak.clone();
                                cell_element = cell_element.relative().pr(px(22.0)).child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "date-toggle-{row_index}-{index}"
                                        )))
                                        .absolute()
                                        .right(px(1.0))
                                        .top(px(1.0))
                                        .bottom(px(1.0))
                                        .w(px(18.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                                        .border_1()
                                        .border_color(rgb(theme.button_border))
                                        .bg(rgb(theme.button_bg))
                                        .text_size(px(11.0))
                                        .text_color(rgb(theme.text))
                                        .on_click(move |_event, window, cx| {
                                            let _ = toggle_weak.update(cx, |grid, cx| {
                                                grid.toggle_date_picker(
                                                    row_index, index, window, cx,
                                                );
                                            });
                                        })
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
                                let up_weak = action_weak.clone();
                                let down_weak = action_weak.clone();
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
                                    })
                                    .on_action(move |_: &MoveCellUp, window, cx| {
                                        let _ = up_weak.update(cx, |grid, cx| {
                                            grid.cell_editor_move(-1, 0, window, cx);
                                        });
                                    })
                                    .on_action(move |_: &MoveCellDown, window, cx| {
                                        let _ = down_weak.update(cx, |grid, cx| {
                                            grid.cell_editor_move(1, 0, window, cx);
                                        });
                                    });
                            }
                            match &editing_cell {
                                Some((row, col, input)) if (*row, *col) == (row_index, index) => {
                                    cell_element = cell_element.child(input.clone());
                                }
                                _ => {
                                    // Constrain the value to the cell's padded content box: gpui's
                                    // overflow clip is the border box, so an unconstrained child
                                    // would still run into the right padding and touch the divider.
                                    cell_element = cell_element.child(
                                        div()
                                            .w_full()
                                            .min_w(px(0.0))
                                            .overflow_hidden()
                                            .child(display),
                                    );
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
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.grid_right_mouse_down(event, window, cx);
            }),
        )
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            this.grid_key(event, window, cx);
        }))
        // Keyboard commands (keymap): F5 refresh and the spreadsheet jumps.
        .key_context(GRID_CONTEXT)
        .on_action(cx.listener(|this, _: &GridRefresh, _window, cx| this.refresh(cx)))
        .on_action(
            cx.listener(|this, _: &GridEditValue, window, cx| this.open_value_editor(window, cx)),
        )
        .on_action(cx.listener(|this, _: &GridPageUp, _window, cx| {
            this.grid_jump(super::grid_input::GridJump::PageUp, false, cx)
        }))
        .on_action(cx.listener(|this, _: &GridPageDown, _window, cx| {
            this.grid_jump(super::grid_input::GridJump::PageDown, false, cx)
        }))
        .on_action(cx.listener(|this, _: &GridSelectPageUp, _window, cx| {
            this.grid_jump(super::grid_input::GridJump::PageUp, true, cx)
        }))
        .on_action(cx.listener(|this, _: &GridSelectPageDown, _window, cx| {
            this.grid_jump(super::grid_input::GridJump::PageDown, true, cx)
        }))
        .on_action(cx.listener(|this, _: &GridRowStart, _window, cx| {
            this.grid_jump(super::grid_input::GridJump::RowStart, false, cx)
        }))
        .on_action(cx.listener(|this, _: &GridRowEnd, _window, cx| {
            this.grid_jump(super::grid_input::GridJump::RowEnd, false, cx)
        }))
        .on_action(cx.listener(|this, _: &GridFirstCell, _window, cx| {
            this.grid_jump(super::grid_input::GridJump::First, false, cx)
        }))
        .on_action(cx.listener(|this, _: &GridLastCell, _window, cx| {
            this.grid_jump(super::grid_input::GridJump::Last, false, cx)
        }))
        .on_action(cx.listener(|this, _: &GridNextCell, _window, cx| {
            this.grid_jump(super::grid_input::GridJump::Next, false, cx)
        }))
        .on_action(cx.listener(|this, _: &GridPrevCell, _window, cx| {
            this.grid_jump(super::grid_input::GridJump::Prev, false, cx)
        }));

        // With no rows to paint, keep the table visible: draw the empty body (gutter + one bordered
        // cell per column, alternating row bands) tall enough to fill the area, instead of leaving a
        // blank pane under the header. It is decorative — no hit-testing — so nothing can be
        // selected; clicking it just focuses the grid.
        let list_body: AnyElement =
            if empty_page && !self.state.loading && !self.state.columns.is_empty() {
                let rows_to_fill =
                    ((self.empty_grid_height / GRID_ROW_HEIGHT).ceil() as usize).max(1) + 1;
                let measure = cx.weak_entity();
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .track_focus(&self.focus)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, window, cx| {
                            window.focus(&this.focus, cx);
                        }),
                    )
                    // The body area's height, measured from this flex-sized child (its rows are
                    // clipped by it), so the next frame draws exactly enough rows to fill it.
                    .on_children_prepainted(move |bounds, _window, cx| {
                        let Some(rect) = bounds.first().copied() else {
                            return;
                        };
                        let height = f32::from(rect.size.height);
                        let _ = measure.update(cx, |grid, cx| {
                            if (grid.empty_grid_height - height).abs() > 0.5 {
                                grid.empty_grid_height = height;
                                cx.notify();
                            }
                        });
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .w_full()
                            .overflow_hidden()
                            .child(empty_grid_rows(
                                &filler_widths,
                                rows_to_fill,
                                gutter_width,
                                theme,
                            )),
                    )
                    .into_any_element()
            } else {
                list.into_any_element()
            };

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
                        cx.listener(|this, event: &MouseUpEvent, window, cx| {
                            this.selecting_cells = false;
                            let press = this.cell_press.take();
                            let dragged = this.cell_dragged;
                            this.cell_dragged = false;
                            // A click selects; a second click on the selected cell, a double-click,
                            // Enter or typing edits it (Navicat). F2 opens the value editor.
                            let reselect = std::mem::take(&mut this.cell_press_reselect);
                            if let Some((row, col)) = press
                                && !dragged
                                && (event.click_count >= 2 || reselect)
                            {
                                this.begin_edit((row, col), None, window, cx);
                            }
                            cx.notify();
                        }),
                    )
                    .child(header)
                    .child(list_body)
                    .child(self.render_date_picker(cx)),
            );

        let mut root = div().relative().flex().flex_col().size_full();
        if key_page {
            root = root.child(self.render_key_page_bar(theme, cx));
        } else if self.state.key_status.is_some() {
            root = root.child(self.render_key_header(theme));
        }
        if !key_page && self.state.show_toolbar {
            root = root.child(self.render_grid_toolbar(cx));
            if self.state.filter_open {
                root = root.child(self.render_filter_panel(cx));
            }
            if self.state.sort_open {
                root = root.child(self.render_sort_panel(cx));
            }
        }
        // The 字段筛选 popup floats over the body (a panel, not a child window), anchored under its
        // toolbar item; the backdrop behind it dismisses the popup on a click anywhere else.
        if !key_page && self.state.show_toolbar && self.field_filter_open {
            root = root.child(self.render_field_filter_backdrop(cx));
            root = root.child(self.render_field_filter_popup(cx));
        }
        // The pane's own window-space bounds, so the popup can be positioned inside it. The root's
        // children are the toolbar plus the body/controls, so the pane rectangle is their *union* —
        // the toolbar alone would report the pane's width but only its own 28px height, which used
        // to pin the popup's field list to its minimum height.
        let pane_bounds = self.grid_pane_bounds.clone();
        root = root.on_children_prepainted(move |bounds, _window, _cx| {
            let union = bounds.iter().copied().reduce(|a, b| a.union(&b));
            if let Some(union) = union {
                *pane_bounds.borrow_mut() = union;
            }
        });
        // Surface a failed fetch above the table. A running one is a spinner inside the toolbar
        // (or the status bar when there is none) — never a row of its own, which would push the
        // rows down and back up on every refresh.
        if let Some(error) = self.state.error.clone() {
            root = root.child(
                div()
                    .id("grid-error")
                    .flex_none()
                    .px_3()
                    .py_1()
                    .bg(rgb(theme.danger))
                    .text_color(rgb(0xFFFFFF))
                    .text_size(px(12.0))
                    .child(error),
            );
        }
        let wheel_weak = cx.weak_entity();
        let wheel_focus = self.focus.clone();
        // A grid with no bar to hang the spinner on (the view designer's preview) floats it over
        // the header's right end instead — still without taking a row from the table.
        let floating_loading: Option<AnyElement> =
            (self.state.loading && !self.state.show_toolbar && !self.state.show_footer)
                .then(|| self.render_loading_spinner().into_any_element());
        root = root.child(
            div()
                .relative()
                .flex()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .children(
                    floating_loading.map(|spinner| {
                        div().absolute().top(px(2.0)).right(px(14.0)).child(spinner)
                    }),
                )
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
                    let grid_focus = wheel_focus.clone();
                    // The wheel handler must ignore events aimed at an overlay above the grid —
                    // the combobox popup covers the window with `.occlude()`. A plain rectangle
                    // test on the grid bounds cannot see that, so insert a hitbox and use gpui's
                    // occlusion-aware `should_handle_scroll` instead.
                    canvas(
                        |bounds, window, _cx| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                        move |_bounds, hitbox, window, _cx| {
                            let grid_focus = grid_focus.clone();
                            window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                                if phase != DispatchPhase::Capture
                                    || !hitbox.should_handle_scroll(window)
                                {
                                    return;
                                }
                                let (dx, dy) = match event.delta {
                                    ScrollDelta::Lines(delta) => {
                                        (delta.x * GRID_ROW_HEIGHT, delta.y * GRID_ROW_HEIGHT)
                                    }
                                    ScrollDelta::Pixels(delta) => {
                                        (f32::from(delta.x), f32::from(delta.y))
                                    }
                                };
                                if dx == 0.0 && dy == 0.0 {
                                    return;
                                }
                                let position = event.position;
                                let consumed = weak
                                    .update(cx, |this, cx| {
                                        // A horizontal trackpad swipe pans the columns; the
                                        // vertical wheel keeps driving vertical scrolling. A
                                        // horizontal gesture is claimed even when the columns
                                        // cannot pan any further: the row list scrolls on one
                                        // axis only and gpui maps a delta with no room on its
                                        // axis onto the other, so letting such an event bubble
                                        // made a swipe past the last column roll the rows
                                        // instead of doing nothing. A vertical delta is left to
                                        // bubble as before, so the wheel still reaches the list
                                        // when the selection cannot move.
                                        let horizontal = dx != 0.0;
                                        if horizontal {
                                            this.scroll_grid_columns(dx, cx);
                                        }
                                        let vertical = dy != 0.0
                                            && this.scroll_grid_selection(position, dy, cx);
                                        let consumed = horizontal || vertical;
                                        if consumed {
                                            cx.stop_propagation();
                                        }
                                        consumed
                                    })
                                    .unwrap_or(false);
                                if consumed {
                                    // Scrolling is an implicit commit of an in-place cell
                                    // editor, and dropping that editor also drops the keyboard
                                    // focus it held — after which Ctrl+C, the arrow keys and
                                    // Delete would silently do nothing. Hand focus back to the
                                    // grid so its shortcuts keep working.
                                    window.focus(&grid_focus, cx);
                                }
                            });
                        },
                    )
                    .absolute()
                    .inset_0()
                }),
        );

        if self.state.show_toolbar && self.page_size_menu_open {
            root = root.child(self.render_record_limit_panel(window, cx));
        }

        // F2's value editor sits under the rows, above the footer (as in Navicat).
        if let Some(panel) = self.render_value_editor(window, cx) {
            root = root.child(panel);
        }

        if key_page {
            root = root.child(self.render_key_page_footer(theme, cx));
        } else if self.state.show_footer {
            root = root
                .child(self.render_grid_controls(cx))
                .child(self.render_grid_status(cx));
        }

        root.on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
            this.grid_hscroll_drag(event, cx);
            this.grid_vscroll_drag(event, cx);
            this.grid_column_drag(event, cx);
            this.filter_resize_drag(event, cx);
            this.field_filter_drag_move(event, cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                let h = this.hscroll_grab.take().is_some();
                let v = this.vscroll_grab.take().is_some();
                let c = this.column_resize.take().is_some();
                let f = this.filter_resize.take().is_some();
                if this.field_filter_drag.is_some() {
                    this.field_filter_drag = None;
                    cx.notify();
                }
                if h || v || c || f {
                    cx.notify();
                }
            }),
        )
        .into_any_element()
    }
}

/// The cell a display column reads from. The rows keep the engine's column order (so a field-layout
/// change never has to touch them), so the display column index is mapped back through the layout.
fn source_cell<'a>(
    rows: &'a [Vec<CellValue>],
    column_order: &[usize],
    row: usize,
    col: usize,
) -> Option<&'a CellValue> {
    let source = column_order.get(col).copied().unwrap_or(col);
    rows.get(row)?.get(source)
}

/// One grid header cell's text: the field name, with the field's data type and comment under it
/// when the 数据 options are on (the type sits between the name and the comment). Every line is
/// clipped to the column, and a line that is too long to fit is shown in full in a hover tooltip.
fn header_cell_text(
    id: SharedString,
    column: &rustgrid_core::ColumnInfo,
    width: f32,
    show_types: bool,
    show_comments: bool,
    theme: Theme,
) -> AnyElement {
    let name = column.name.clone();
    let data_type = column.data_type.clone();
    let comment = column.comment.clone();
    // The type and comment lines are only painted when the 数据 options are on, but a field name
    // that does not fit is always worth a tooltip.
    let text_width = (width - 12.0).max(16.0);
    let name_cut = ui::approx_text_width(&name) > text_width;
    let type_shown = show_types && !data_type.is_empty();
    let comment_shown = show_comments && !comment.is_empty();
    let type_cut = type_shown && ui::approx_text_width(&data_type) > text_width;
    let comment_cut = comment_shown && ui::approx_text_width(&comment) > text_width;
    // A clipped name shows the whole header (name, type and comment) in the tooltip; otherwise only
    // the line that does not fit needs one.
    let tooltip = if name_cut {
        let mut lines = vec![name.clone()];
        if type_shown {
            lines.push(data_type.clone());
        }
        if comment_shown {
            lines.push(comment.clone());
        }
        Some(lines.join("\n"))
    } else if type_cut {
        Some(data_type.clone())
    } else if comment_cut {
        Some(comment.clone())
    } else {
        None
    };

    let mut container = div()
        .id(id)
        .flex()
        .flex_1()
        .min_w(px(0.0))
        .overflow_hidden();
    if let Some(tooltip) = tooltip {
        container = container.tooltip(ui::text_tooltip(tooltip));
    }
    let text = if type_shown || comment_shown {
        let mut lines = div()
            .flex()
            .flex_col()
            .justify_center()
            .w_full()
            .min_w(px(0.0))
            .child(div().truncate().child(name));
        if type_shown {
            lines = lines.child(
                div()
                    .truncate()
                    .text_size(px(10.0))
                    .text_color(rgb(theme.text_muted))
                    .child(data_type),
            );
        }
        if comment_shown {
            lines = lines.child(
                div()
                    .truncate()
                    .text_size(px(10.0))
                    .text_color(rgb(theme.text_muted))
                    .child(comment),
            );
        }
        lines.into_any_element()
    } else {
        div()
            .w_full()
            .min_w(px(0.0))
            .truncate()
            .child(name)
            .into_any_element()
    };
    container.child(text).into_any_element()
}

/// The decorative body of a table with no rows: the gutter and one bordered cell per column,
/// repeated `rows` times with the same alternating bands as real rows, so an empty page still reads
/// as an empty table rather than a blank pane under the header. The rows carry no hit-testing.
fn empty_grid_rows(
    widths: &[f32],
    rows: usize,
    gutter_width: f32,
    theme: Theme,
) -> impl IntoElement {
    let mut column = div().flex().flex_col().w_full();
    for row in 0..rows {
        let background = if row % 2 == 1 {
            theme.row_alt_bg
        } else {
            theme.editor_bg
        };
        let mut row_element = div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(GRID_ROW_HEIGHT))
            .bg(rgb(background));
        if gutter_width > 0.0 {
            row_element = row_element.child(
                div()
                    .w(px(gutter_width))
                    .h(px(GRID_ROW_HEIGHT))
                    .flex_none()
                    .border_r_1()
                    .border_color(rgb(theme.grid_line)),
            );
        }
        for width in widths {
            row_element = row_element.child(
                div()
                    .w(px(*width))
                    .h(px(GRID_ROW_HEIGHT))
                    .flex_none()
                    .border_r_1()
                    .border_b_1()
                    .border_color(rgb(theme.grid_line)),
            );
        }
        column = column.child(row_element);
    }
    column
}
