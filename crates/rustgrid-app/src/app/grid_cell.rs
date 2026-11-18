use super::*;

impl GridView {
    /// The open ENUM list of the cell being edited, anchored under (or above) the cell like the
    /// date picker. A press keeps focus in the cell editor so picking does not end the edit.
    pub(super) fn render_enum_menu(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some((row, col, highlight)) = self.enum_menu else {
            return div().into_any_element();
        };
        let members = self.column_members(col).to_vec();
        if members.is_empty() {
            return div().into_any_element();
        }
        let theme = self.theme;
        let header_height = self.header_height(cx);
        let width = self
            .state
            .column_widths
            .get(col)
            .copied()
            .unwrap_or(120.0)
            .max(120.0);
        let cell_left: f32 =
            GRID_GUTTER_WIDTH + self.state.column_widths.iter().take(col).sum::<f32>();
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let offset_y = f32::from(handle.offset().y);
        let viewport_h = f32::from(handle.bounds().size.height);
        let cell_top = header_height + row as f32 * GRID_ROW_HEIGHT + offset_y;
        let popup_h = (members.len() as f32 * GRID_ROW_HEIGHT + 4.0).min(240.0);
        let top = if cell_top + GRID_ROW_HEIGHT + popup_h > header_height + viewport_h {
            (cell_top - popup_h).max(header_height)
        } else {
            cell_top + GRID_ROW_HEIGHT
        };
        let items = members.into_iter().enumerate().map(|(index, member)| {
            let active = index == highlight;
            div()
                .id(SharedString::from(format!("enum-member-{index}")))
                .h(px(GRID_ROW_HEIGHT))
                .px(px(6.0))
                .flex()
                .items_center()
                .whitespace_nowrap()
                .cursor_pointer()
                .when(active, |style| {
                    style
                        .bg(rgb(theme.grid_selection_bg))
                        .text_color(rgb(theme.grid_selection_text))
                })
                .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.enum_menu_pick(index, cx);
                    this.commit_editor(cx);
                }))
                .child(if member.is_empty() {
                    "\u{2007}".to_string()
                } else {
                    member
                })
        });
        ui::popup_panel(theme)
            .id("enum-menu")
            .left(px(cell_left.max(0.0)))
            .top(px(top))
            .w(px(width))
            .max_h(px(240.0))
            .overflow_y_scroll()
            .py(px(2.0))
            .capture_any_mouse_down(|_event, window, _cx| window.prevent_default())
            .children(items)
            .into_any_element()
    }

    pub(super) fn render_date_picker(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(picker) = self.date_picker.as_ref() else {
            return div().into_any_element();
        };
        let theme = self.theme;
        // The header is taller when the 数据 option paints the fields' comments, so the picker must
        // clear it instead of assuming one row.
        let header_height = self.header_height(cx);
        let width = 192.0f32;
        let content_width: f32 = GRID_GUTTER_WIDTH
            + self
                .state
                .column_widths
                .iter()
                .take(self.state.visible_columns)
                .sum::<f32>();
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
        let cell_top = header_height + picker.row as f32 * GRID_ROW_HEIGHT + offset_y;
        let popup_h = if picker.has_time { 248.0 } else { 210.0 };
        let top = if cell_top + GRID_ROW_HEIGHT + popup_h > header_height + viewport_h {
            (cell_top - popup_h).max(header_height)
        } else {
            cell_top + GRID_ROW_HEIGHT
        };

        // The date grid, its day/month/year views and all navigation come from gpui-kit's
        // calendar (through the compact `ui` wrapper); the time-of-day row is gpui-kit's
        // `TimeField`. Only the footer is app-drawn.
        //
        // The mouse-down capture keeps focus where it is (the cell editor or the time field)
        // when a calendar cell is pressed: gpui focuses any `track_focus` element on mouse-down,
        // which would otherwise blur the editor and auto-commit.
        let calendar = div()
            .capture_any_mouse_down(|_event, window, _cx| window.prevent_default())
            .child(ui::compact_calendar(&picker.calendar));

        let now = chrono::Local::now().naive_local();

        let time_row: Option<AnyElement> = picker.time_field.as_ref().map(|field| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .pb_1()
                .child(TimeField::new(field).small())
                .into_any_element()
        });

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
                        cx.listener(|this, _event, window, cx| this.date_picker_ok(window, cx)),
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
            // Focus can sit in the `TimeField`, whose keys never reach the grid list, so Escape
            // is handled here (it bubbles from the focused time segment through this panel).
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key == "escape" {
                    this.date_picker_cancel(cx);
                    cx.stop_propagation();
                }
            }))
            .child(calendar)
            .children(time_row)
            .child(footer)
            .into_any_element()
    }
}
