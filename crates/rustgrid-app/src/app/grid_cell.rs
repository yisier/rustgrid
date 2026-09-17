use super::*;

impl GridView {
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

        ui::popup_panel(theme)
            .id("date-picker")
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .p_2()
            .gap_1()
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
