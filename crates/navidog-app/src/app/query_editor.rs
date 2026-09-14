use super::*;

impl AppView {
    pub(super) fn render_query_editor(
        &self,
        query: &QueryTab,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let (start, end) = query.selection();
        let line_starts = sql_line_starts(&query.sql);
        let digits = line_starts.len().to_string().len().max(2);
        let gutter_width = digits as f32 * 8.0 + 12.0;
        let text_left = gutter_width + QUERY_EDITOR_PAD;

        let (caret_offset, caret_height) = if self.query_editor_measured
            && self.query_editor_focused
            && start >= end
            && *self.query_editor_text.borrow() == query.sql
        {
            let previous = self.query_editor_layout.borrow();
            let index = query.caret.min(previous.len());
            match previous.position_for_index(index) {
                Some(position) => {
                    let bounds = previous.bounds();
                    (
                        Some((position.x - bounds.origin.x, position.y - bounds.origin.y)),
                        previous.line_height(),
                    )
                }
                None => (None, px(0.0)),
            }
        } else {
            (None, px(0.0))
        };

        let mut line_numbers: Vec<(usize, f32)> = Vec::new();
        if self.query_editor_measured {
            let previous = self.query_editor_layout.borrow();
            let origin_y = f32::from(previous.bounds().origin.y);
            for (line_index, line_start) in line_starts.iter().enumerate() {
                if *line_start > previous.len() {
                    break;
                }
                if let Some(position) = previous.position_for_index(*line_start) {
                    line_numbers.push((line_index + 1, f32::from(position.y) - origin_y));
                }
            }
        }

        let styled = self.styled_sql(&query.sql, (start, end));
        let layout = styled.layout().clone();
        *self.query_editor_layout.borrow_mut() = layout;
        *self.query_editor_text.borrow_mut() = query.sql.clone();

        let mut gutter = div()
            .absolute()
            .top_0()
            .left_0()
            .h_full()
            .w(px(gutter_width))
            .bg(rgb(theme.header_bg))
            .border_r_1()
            .border_color(rgb(theme.border));
        for (number, y) in line_numbers {
            gutter = gutter.child(
                div()
                    .absolute()
                    .top(px(y + QUERY_EDITOR_PAD))
                    .right(px(8.0))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(rgb(theme.text_muted))
                    .child(number.to_string()),
            );
        }

        let mut root = div()
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .bg(rgb(theme.editor_bg))
            .overflow_hidden()
            .track_focus(&self.query_focus)
            .cursor_text()
            .on_key_down(
                cx.listener(|this, event, window, cx| this.query_editor_key(event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.query_editor_mouse_down(event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.query_editor_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if let Some(index) = this.active_query
                        && let Some(tab) = this.queries.get_mut(index)
                    {
                        tab.selecting = false;
                    }
                    cx.notify();
                }),
            )
            .child(gutter)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .pl(px(text_left))
                    .pr(px(QUERY_EDITOR_PAD))
                    .pt(px(QUERY_EDITOR_PAD))
                    .pb(px(QUERY_EDITOR_PAD))
                    .font_family("Consolas")
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .child(styled),
            );

        if let Some((x, y)) = caret_offset
            && self.caret_visible
        {
            root = root.child(
                div()
                    .absolute()
                    .left(px(f32::from(x) + text_left))
                    .top(px(f32::from(y) + QUERY_EDITOR_PAD))
                    .w(px(1.5))
                    .h(caret_height)
                    .bg(rgb(theme.text)),
            );
        }

        if let Some(completion) = self.query_completion.as_ref()
            && let Some((x, y)) = caret_offset
        {
            let mut list = div()
                .id("query-completion")
                .absolute()
                .left(px(f32::from(x) + text_left))
                .top(px(f32::from(y) + QUERY_EDITOR_PAD + 18.0))
                .w(px(220.0))
                .max_h(px(210.0))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .bg(rgb(theme.dialog_bg))
                .border_1()
                .border_color(rgb(theme.border))
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    cx.stop_propagation();
                });
            for (index, candidate) in completion.candidates.iter().enumerate() {
                let selected = index == completion.selected;
                let label = candidate.clone();
                let insert = candidate.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("query-completion-{index}")))
                        .flex()
                        .items_center()
                        .h(px(20.0))
                        .px_2()
                        .flex_none()
                        .text_size(px(12.0))
                        .cursor_pointer()
                        .when(selected, move |style| {
                            style
                                .bg(rgb(theme.tree_selected_bg))
                                .text_color(rgb(theme.tree_selected_text))
                        })
                        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.accept_query_completion(insert.clone(), cx);
                        }))
                        .child(label),
                );
            }
            root = root.child(deferred(list).with_priority(20));
        }

        root
    }

    pub(super) fn render_query_result(
        &self,
        query: &QueryTab,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let _ = cx;
        match &query.result {
            Loadable::Idle => div().into_any_element(),
            Loadable::Loading => div()
                .flex_1()
                .p_2()
                .text_color(rgb(theme.text_muted))
                .child(t!("query.running").to_string())
                .into_any_element(),
            Loadable::Failed(error) => div()
                .flex_1()
                .p_2()
                .text_color(rgb(theme.danger))
                .child(error.clone())
                .into_any_element(),
            Loadable::Loaded(result) => {
                if !result.has_result_set {
                    let message = if result.rows_affected > 0 {
                        t!("query.rows_affected", count = result.rows_affected).to_string()
                    } else {
                        t!("query.executed").to_string()
                    };
                    div().flex_1().p_2().child(message).into_any_element()
                } else if result.columns.is_empty() {
                    div()
                        .flex_1()
                        .p_2()
                        .text_color(rgb(theme.text_muted))
                        .child(t!("query.empty").to_string())
                        .into_any_element()
                } else {
                    self.render_query_grid(result).into_any_element()
                }
            }
        }
    }

    pub(super) fn render_query_grid(&self, result: &QueryResult) -> impl IntoElement {
        let theme = self.theme;
        let widths = compute_column_widths(&result.columns, &result.rows);
        let content_width: f32 = widths.iter().sum::<f32>().max(1.0);

        let mut header = div().flex().flex_row().flex_none().bg(rgb(theme.header_bg));
        for (index, column) in result.columns.iter().enumerate() {
            let width = widths.get(index).copied().unwrap_or(GRID_COLUMN_WIDTH);
            header = header.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(GRID_ROW_HEIGHT))
                    .w(px(width))
                    .flex_none()
                    .px_2()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .font_weight(FontWeight::SEMIBOLD)
                    .border_r_1()
                    .border_color(rgb(theme.border))
                    .child(column.name.clone()),
            );
        }

        let rows = Arc::new(result.rows.clone());
        let widths = Arc::new(widths);
        let list = uniform_list(
            SharedString::from("query-result-rows"),
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
                        let mut row_element = div()
                            .flex()
                            .flex_row()
                            .bg(rgb(base_background))
                            .h(px(GRID_ROW_HEIGHT));
                        for (index, cell) in row.iter().enumerate() {
                            let width = widths.get(index).copied().unwrap_or(GRID_COLUMN_WIDTH);
                            row_element = row_element.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .h(px(GRID_ROW_HEIGHT))
                                    .w(px(width))
                                    .flex_none()
                                    .px_2()
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(cell.as_display()),
                            );
                        }
                        row_element
                    })
                    .collect::<Vec<_>>()
            },
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::FitList)
        .track_scroll(self.query_result_scroll.clone())
        .flex_1()
        .min_h(px(0.0));

        div().flex().flex_row().flex_1().min_h(px(0.0)).child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .overflow_hidden()
                .child(
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .h_full()
                        .w(px(content_width))
                        .child(header)
                        .child(list),
                ),
        )
    }

    pub(super) fn query_editor_index_for_position(&self, position: Point<Pixels>) -> Option<usize> {
        if !self.query_editor_measured {
            return None;
        }
        let index = self.active_query?;
        let text = self.queries.get(index)?.sql.clone();
        if *self.query_editor_text.borrow() != text {
            return None;
        }
        let layout = self.query_editor_layout.borrow();
        Some(match layout.index_for_position(position) {
            Ok(index) | Err(index) => index,
        })
    }

    pub(super) fn query_editor_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(index) = self.active_query else {
            return;
        };
        window.focus(&self.query_focus);
        let position = self.query_editor_index_for_position(event.position);
        if let (Some(position), Some(tab)) = (position, self.queries.get_mut(index)) {
            if event.modifiers.shift {
                tab.caret = position;
            } else {
                tab.anchor = position;
                tab.caret = position;
            }
            tab.selecting = true;
        }
        self.caret_visible = true;
        cx.notify();
    }

    pub(super) fn query_editor_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(index) = self.active_query else {
            return;
        };
        if !self
            .queries
            .get(index)
            .map(|tab| tab.selecting)
            .unwrap_or(false)
        {
            return;
        }
        let position = self.query_editor_index_for_position(event.position);
        if let (Some(position), Some(tab)) = (position, self.queries.get_mut(index)) {
            tab.caret = position;
        }
        cx.notify();
    }

    pub(super) fn query_editor_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(index) = self.active_query else {
            return;
        };
        if self.query_completion.is_some() {
            match event.keystroke.key.as_str() {
                "up" => {
                    self.move_query_completion(-1, cx);
                    return;
                }
                "down" => {
                    self.move_query_completion(1, cx);
                    return;
                }
                "enter" | "tab" => {
                    self.accept_selected_query_completion(cx);
                    return;
                }
                "escape" => {
                    self.query_completion = None;
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }
        let Some(tab) = self.queries.get(index) else {
            return;
        };
        let sql = tab.sql.clone();
        let mut anchor = tab.anchor.min(sql.len());
        let mut caret = tab.caret.min(sql.len());
        if !sql.is_char_boundary(anchor) {
            anchor = previous_boundary(&sql, anchor);
        }
        if !sql.is_char_boundary(caret) {
            caret = previous_boundary(&sql, caret);
        }
        let (start, end) = (anchor.min(caret), anchor.max(caret));

        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.control || keystroke.modifiers.platform;
        let shift = keystroke.modifiers.shift;

        let mut new_sql = sql.clone();
        let new_caret;
        let new_anchor;
        let mut modified = false;
        let mut close_completion = false;

        if command {
            match keystroke.key.as_str() {
                "a" => {
                    new_anchor = 0;
                    new_caret = sql.len();
                }
                "c" => {
                    if start < end {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            sql[start..end].to_string(),
                        ));
                    }
                    return;
                }
                "x" => {
                    if start < end {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            sql[start..end].to_string(),
                        ));
                        new_sql.replace_range(start..end, "");
                        new_caret = start;
                        new_anchor = start;
                        modified = true;
                    } else {
                        return;
                    }
                }
                "v" => {
                    let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                        return;
                    };
                    let pasted = pasted.replace("\r\n", "\n").replace('\r', "\n");
                    if pasted.is_empty() {
                        return;
                    }
                    new_sql.replace_range(start..end, &pasted);
                    new_caret = start + pasted.len();
                    new_anchor = new_caret;
                    modified = true;
                }
                "space" => {
                    self.refresh_query_completion(true);
                    return;
                }
                _ => return,
            }
        } else {
            match keystroke.key.as_str() {
                "left" => {
                    let cursor = if shift {
                        previous_boundary(&sql, caret)
                    } else if start < end {
                        start
                    } else {
                        previous_boundary(&sql, start)
                    };
                    new_caret = cursor;
                    new_anchor = if shift { anchor } else { cursor };
                    close_completion = true;
                }
                "right" => {
                    let cursor = if shift {
                        next_boundary(&sql, caret)
                    } else if start < end {
                        end
                    } else {
                        next_boundary(&sql, start)
                    };
                    new_caret = cursor;
                    new_anchor = if shift { anchor } else { cursor };
                    close_completion = true;
                }
                "up" => {
                    let cursor = move_vertical(&sql, caret, -1);
                    new_caret = cursor;
                    new_anchor = if shift { anchor } else { cursor };
                    close_completion = true;
                }
                "down" => {
                    let cursor = move_vertical(&sql, caret, 1);
                    new_caret = cursor;
                    new_anchor = if shift { anchor } else { cursor };
                    close_completion = true;
                }
                "home" => {
                    let (line_start, _) = line_bounds(&sql, caret);
                    new_caret = line_start;
                    new_anchor = if shift { anchor } else { line_start };
                    close_completion = true;
                }
                "end" => {
                    let (_, line_end) = line_bounds(&sql, caret);
                    new_caret = line_end;
                    new_anchor = if shift { anchor } else { line_end };
                    close_completion = true;
                }
                "backspace" => {
                    if start < end {
                        new_sql.replace_range(start..end, "");
                        new_caret = start;
                        new_anchor = start;
                    } else if start > 0 {
                        let previous = previous_boundary(&sql, start);
                        new_sql.replace_range(previous..start, "");
                        new_caret = previous;
                        new_anchor = previous;
                    } else {
                        return;
                    }
                    modified = true;
                }
                "delete" => {
                    if start < end {
                        new_sql.replace_range(start..end, "");
                        new_caret = start;
                        new_anchor = start;
                    } else if start < sql.len() {
                        let next = next_boundary(&sql, start);
                        new_sql.replace_range(start..next, "");
                        new_caret = start;
                        new_anchor = start;
                    } else {
                        return;
                    }
                    modified = true;
                }
                "enter" => {
                    new_sql.replace_range(start..end, "\n");
                    new_caret = start + 1;
                    new_anchor = new_caret;
                    modified = true;
                }
                "tab" => {
                    new_sql.replace_range(start..end, "    ");
                    new_caret = start + 4;
                    new_anchor = new_caret;
                    modified = true;
                }
                "space" => {
                    new_sql.replace_range(start..end, " ");
                    new_caret = start + 1;
                    new_anchor = new_caret;
                    modified = true;
                }
                _ => {
                    if let Some(insert) = keystroke
                        .key_char
                        .as_ref()
                        .filter(|insert| !insert.is_empty() && !insert.contains('\n'))
                    {
                        new_sql.replace_range(start..end, insert);
                        new_caret = start + insert.len();
                        new_anchor = new_caret;
                        modified = true;
                    } else {
                        return;
                    }
                }
            }
        }

        if let Some(tab) = self.queries.get_mut(index) {
            tab.sql = new_sql;
            tab.caret = new_caret;
            tab.anchor = new_anchor;
        }
        if close_completion {
            self.query_completion = None;
        } else if modified {
            self.refresh_query_completion(false);
        }
        self.caret_visible = true;
        cx.notify();
    }
}
