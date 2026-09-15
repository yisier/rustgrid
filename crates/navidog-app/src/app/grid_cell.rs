use super::*;

impl GridView {
    pub(super) fn render_cell_editor(
        &self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let Some(editor) = self.cell_editor.as_ref() else {
            return div().into_any_element();
        };
        let width = self
            .state
            .column_widths
            .get(editor.col)
            .copied()
            .unwrap_or(GRID_COLUMN_WIDTH)
            .max(80.0);
        let left: f32 = GRID_GUTTER_WIDTH
            + self
                .state
                .column_widths
                .iter()
                .take(editor.col)
                .sum::<f32>();
        let offset_y = f32::from(self.list_scroll.0.borrow().base_handle.offset().y);
        let top = GRID_ROW_HEIGHT + editor.row as f32 * GRID_ROW_HEIGHT + offset_y;
        let theme = self.theme;

        let chars: Vec<char> = editor.value.chars().collect();
        let len = chars.len();
        let mut selection = editor.selection;
        selection.anchor = selection.anchor.min(len);
        selection.cursor = selection.cursor.min(len);
        let (start, end) = selection.range();
        let focused = self.cell_editor_focused;

        let mut shown = div().flex().flex_row().items_center().whitespace_nowrap();
        if focused && start < end {
            if start > 0 {
                shown = shown.child(chars[..start].iter().copied().collect::<String>());
            }
            shown = shown.child(
                div()
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
                    .child(chars[start..end].iter().copied().collect::<String>()),
            );
            if end < len {
                shown = shown.child(chars[end..].iter().copied().collect::<String>());
            }
        } else if focused {
            if selection.cursor > 0 {
                shown = shown.child(
                    chars[..selection.cursor]
                        .iter()
                        .copied()
                        .collect::<String>(),
                );
            }
            if selection.cursor < len {
                shown = shown.child(
                    chars[selection.cursor..]
                        .iter()
                        .copied()
                        .collect::<String>(),
                );
            }
        } else {
            shown = shown.child(editor.value.clone());
        }

        let caret_x = if focused && start >= end && self.caret() {
            let prefix: String = chars[..selection.cursor].iter().copied().collect();
            let run = window.text_style().to_run(prefix.len());
            let layout = window
                .text_system()
                .layout_line(&prefix, px(12.0), &[run], None);
            Some(f32::from(layout.width))
        } else {
            None
        };
        let mut content = div()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .child(shown);
        if let Some(x) = caret_x {
            content = content.child(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(0.0))
                    .w(px(1.5))
                    .h(px(14.0))
                    .bg(rgb(theme.text)),
            );
        }

        div()
            .id("cell-editor")
            .absolute()
            .occlude()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(GRID_ROW_HEIGHT))
            .track_focus(&self.cell_editor_focus)
            .cursor_text()
            .flex()
            .items_center()
            .px_2()
            .bg(rgb(theme.input_bg))
            .border_1()
            .border_color(rgb(theme.primary))
            .text_size(px(12.0))
            .on_key_down(cx.listener(|this, event, _window, cx| this.editor_key(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.cell_editor_focus);
                    let Some(value) = this.cell_editor.as_ref().map(|editor| editor.value.clone())
                    else {
                        return;
                    };
                    let index = this.cell_editor_index_for_x(&value, event.position.x, window);
                    if let Some(editor) = this.cell_editor.as_mut() {
                        if event.modifiers.shift {
                            editor.selection.cursor = index;
                        } else {
                            editor.selection = FieldSelection {
                                anchor: index,
                                cursor: index,
                            };
                            editor.selecting = true;
                        }
                    }
                    this.caret_visible = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let selecting = this
                    .cell_editor
                    .as_ref()
                    .is_some_and(|editor| editor.selecting);
                if !selecting {
                    return;
                }
                let Some(value) = this.cell_editor.as_ref().map(|editor| editor.value.clone())
                else {
                    return;
                };
                let index = this.cell_editor_index_for_x(&value, event.position.x, window);
                if let Some(editor) = this.cell_editor.as_mut() {
                    editor.selection.cursor = index;
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if let Some(editor) = this.cell_editor.as_mut() {
                        editor.selecting = false;
                    }
                    cx.notify();
                }),
            )
            .child(content)
            .child(self.ime_probe(&self.cell_editor_focus))
            .into_any_element()
    }

    pub(super) fn render_date_picker(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(picker) = self.date_picker.as_ref() else {
            return div().into_any_element();
        };
        let theme = self.theme;
        let width = 232.0f32;
        let content_width: f32 = GRID_GUTTER_WIDTH + self.state.column_widths.iter().sum::<f32>();
        let cell_left: f32 = GRID_GUTTER_WIDTH
            + self
                .state
                .column_widths
                .iter()
                .take(picker.col)
                .sum::<f32>();
        let left = cell_left.min((content_width - width).max(0.0)).max(0.0);
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let offset_y = f32::from(handle.offset().y);
        let viewport_h = f32::from(handle.bounds().size.height);
        let cell_top = GRID_ROW_HEIGHT + picker.row as f32 * GRID_ROW_HEIGHT + offset_y;
        let popup_h = if picker.has_time { 296.0 } else { 264.0 };
        let top = if cell_top + GRID_ROW_HEIGHT + popup_h > GRID_ROW_HEIGHT + viewport_h {
            (cell_top - popup_h).max(GRID_ROW_HEIGHT)
        } else {
            cell_top + GRID_ROW_HEIGHT
        };

        let title = t!("grid.year_month", year = picker.year, month = picker.month).to_string();
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("date-prev")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(20.0))
                    .h(px(20.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.date_picker_shift_month(-1, cx);
                    }))
                    .child("‹"),
            )
            .child(div().text_size(px(12.0)).child(title))
            .child(
                div()
                    .id("date-next")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(20.0))
                    .h(px(20.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.date_picker_shift_month(1, cx);
                    }))
                    .child("›"),
            );

        let first_weekday = NaiveDate::from_ymd_opt(picker.year, picker.month, 1)
            .map(|date| date.weekday().num_days_from_monday() as i32)
            .unwrap_or(0);
        let days = days_in_month(picker.year, picker.month) as i32;
        let now = chrono::Local::now().naive_local();

        let mut weekdays = div().flex().flex_row();
        for label in weekday_labels() {
            weekdays = weekdays.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(28.0))
                    .h(px(18.0))
                    .text_size(px(10.0))
                    .text_color(rgb(theme.text_muted))
                    .child(label),
            );
        }

        let mut calendar = div().flex().flex_col().items_center().child(weekdays);
        for week in 0..6 {
            let mut row = div().flex().flex_row();
            for weekday in 0..7 {
                let day_number = week * 7 + weekday - first_weekday;
                if day_number < 0 || day_number >= days {
                    row = row.child(div().w(px(28.0)).h(px(22.0)));
                    continue;
                }
                let day = day_number as u32 + 1;
                let is_selected = day == picker.day;
                let is_today =
                    now.year() == picker.year && now.month() == picker.month && now.day() == day;
                row = row.child(
                    div()
                        .id(SharedString::from(format!("date-day-{day}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(28.0))
                        .h(px(22.0))
                        .text_size(px(11.0))
                        .cursor_pointer()
                        .when(is_selected, |style| {
                            style.bg(rgb(theme.primary)).text_color(rgb(0xffffff))
                        })
                        .when(!is_selected && is_today, |style| {
                            style.border_1().border_color(rgb(theme.primary))
                        })
                        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.date_picker_select_day(day, cx);
                        }))
                        .child(day.to_string()),
                );
            }
            calendar = calendar.child(row);
        }

        let mut time_row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_1();
        if picker.has_time {
            time_row = time_row
                .child(self.time_spinner("date-hour", picker.hour, 0, cx))
                .child(":")
                .child(self.time_spinner("date-minute", picker.minute, 1, cx))
                .child(":")
                .child(self.time_spinner("date-second", picker.second, 2, cx));
        }

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("date-today")
                    .text_size(px(11.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.date_picker_today(cx);
                    }))
                    .child(format!(
                        "{}: {}/{}/{}",
                        t!("grid.today"),
                        now.year(),
                        now.month(),
                        now.day()
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(ui::dialog_button(
                        "date-cancel",
                        t!("form.cancel").to_string(),
                        false,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.date_picker_cancel(cx)),
                    ))
                    .child(ui::dialog_button(
                        "date-ok",
                        t!("form.ok").to_string(),
                        true,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.date_picker_ok(cx)),
                    )),
            );

        div()
            .id("date-picker")
            .absolute()
            .occlude()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.text_muted))
            .child(header)
            .child(calendar)
            .child(time_row)
            .child(footer)
            .into_any_element()
    }

    pub(super) fn time_spinner(
        &self,
        id_prefix: &str,
        value: u32,
        field: usize,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .id(SharedString::from(format!("{id_prefix}-up")))
                    .cursor_pointer()
                    .text_size(px(8.0))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.date_picker_shift_time(field, 1, cx);
                    }))
                    .child("▲"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(28.0))
                    .h(px(18.0))
                    .bg(rgb(theme.input_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_size(px(11.0))
                    .child(format!("{value:02}")),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{id_prefix}-down")))
                    .cursor_pointer()
                    .text_size(px(8.0))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.date_picker_shift_time(field, -1, cx);
                    }))
                    .child("▼"),
            )
    }
}
