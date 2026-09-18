use super::*;

impl GridView {
    pub(super) fn render_date_picker(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(picker) = self.date_picker.as_ref() else {
            return div().into_any_element();
        };
        let theme = self.theme;
        let width = 192.0f32;
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
        let popup_h = if picker.has_time { 248.0 } else { 210.0 };
        let top = if cell_top + GRID_ROW_HEIGHT + popup_h > GRID_ROW_HEIGHT + viewport_h {
            (cell_top - popup_h).max(GRID_ROW_HEIGHT)
        } else {
            cell_top + GRID_ROW_HEIGHT
        };

        // The date grid, its day/month/year views and all navigation come from gpui-kit's
        // calendar (through the compact `ui` wrapper); only the time-of-day row and the footer
        // are app-drawn.
        let calendar = ui::compact_calendar(&picker.calendar);

        let now = chrono::Local::now().naive_local();

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
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.date_picker_today(window, cx);
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
                    .child(ui::popup_button(
                        "date-cancel",
                        t!("form.cancel").to_string(),
                        false,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.date_picker_cancel(cx)),
                    ))
                    .child(ui::popup_button(
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
            // gpui moves focus to any element with a tracked focus handle on mouse-down. The
            // kit's calendar root (and its OK/Cancel buttons) are focusable, so without this
            // the in-place editor would blur and auto-commit on the first click. Suppressing
            // the default keeps focus in the editor until OK/Cancel/click-away.
            .capture_any_mouse_down(|_event, window, _cx| window.prevent_default())
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
