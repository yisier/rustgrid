use super::*;

impl GridView {
    /// How many leading visible columns are frozen. The frozen fields are pinned to the front of
    /// the display order, so this is also where the fixed block ends.
    pub(super) fn frozen_split(&self) -> usize {
        self.state.frozen_count()
    }

    /// The total width of the frozen columns (the leading `frozen_split` columns).
    pub(super) fn frozen_width(&self) -> f32 {
        let split = self.frozen_split();
        self.state
            .column_widths
            .iter()
            .take(split)
            .sum::<f32>()
            .max(0.0)
    }

    /// The width the non-frozen columns pan inside: the table viewport minus the frozen columns
    /// and the row gutter. Without a frozen field this is the old whole-table viewport minus the
    /// gutter, which keeps the existing horizontal scroll maths unchanged.
    pub(super) fn grid_scroll_viewport(&self) -> f32 {
        let table = f32::from(self.hscroll.bounds().size.width);
        (table - self.gutter_width() - self.frozen_width()).max(0.0)
    }

    /// How far the non-frozen columns can pan; zero when they already fit.
    pub(super) fn grid_scroll_max(&self) -> f32 {
        let split = self.frozen_split();
        let total: f32 = self
            .state
            .column_widths
            .iter()
            .take(self.state.visible_columns)
            .skip(split)
            .sum();
        (total - self.grid_scroll_viewport()).max(0.0)
    }

    /// The current pan of the non-frozen columns, clamped to the current maximum.
    pub(super) fn grid_column_scroll(&self) -> f32 {
        let scroll = if self.frozen_split() > 0 {
            self.column_scroll
        } else {
            -f32::from(self.hscroll.offset().x)
        };
        scroll.clamp(0.0, self.grid_scroll_max())
    }

    /// Move the non-frozen columns to `scroll`. A frozen grid keeps the offset in its own field
    /// (the tracked handle must not translate the frozen subtree); otherwise it drives `hscroll`
    /// exactly as before.
    pub(super) fn set_grid_column_scroll(&mut self, scroll: f32) {
        let scroll = scroll.clamp(0.0, self.grid_scroll_max());
        if self.frozen_split() > 0 {
            self.column_scroll = scroll;
        } else {
            let y = self.hscroll.offset().y;
            self.hscroll.set_offset(Point::new(px(-scroll), y));
        }
    }

    pub(super) fn render_grid_vscrollbar(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let header_height = self.header_height(cx);
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let viewport = f32::from(handle.bounds().size.height);
        let max = (self.display_row_count() as f32 * GRID_ROW_HEIGHT - viewport).max(0.0);
        if max <= 0.0 {
            return div().into_any_element();
        }
        let scroll = -f32::from(handle.offset().y);
        let (thumb_top, thumb_len) = scrollbar_fractions(viewport, max, scroll);

        div()
            .flex_none()
            .h_full()
            .pt(px(header_height))
            .pb(px(GRID_SCROLLBAR_THICKNESS))
            .child(
                ui::vscrollbar_track("grid-vscrollbar", theme, thumb_top, thumb_len).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                        this.grid_vscroll_begin(event.position.y, cx);
                    }),
                ),
            )
            .into_any_element()
    }

    pub(super) fn render_grid_hscrollbar(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let viewport = self.grid_scroll_viewport();
        let max = self.grid_scroll_max();
        if max <= 0.0 {
            // Reserve the same height as a real scrollbar so the vertical track's viewport stays
            // constant whether or not the grid can scroll horizontally.
            return div()
                .flex_none()
                .h(px(GRID_SCROLLBAR_THICKNESS))
                .into_any_element();
        }
        let scroll = self.grid_column_scroll();
        let (thumb_left, thumb_len) = scrollbar_fractions(viewport, max, scroll);

        ui::hscrollbar_track("grid-hscrollbar", theme, thumb_left, thumb_len)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.grid_hscroll_begin(event.position.x, cx);
                }),
            )
            .into_any_element()
    }

    pub(super) fn grid_hscroll_begin(&mut self, mouse_x: Pixels, cx: &mut Context<'_, Self>) {
        let bounds = self.hscroll.bounds();
        let viewport = self.grid_scroll_viewport();
        let max = self.grid_scroll_max();
        let (thumb_w, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let scroll = self.grid_column_scroll();
        let thumb_x = (scroll / max) * travel;
        let relative = f32::from(mouse_x) - f32::from(bounds.left());
        let grab = if relative >= thumb_x && relative <= thumb_x + thumb_w {
            relative - thumb_x
        } else {
            thumb_w / 2.0
        };
        self.hscroll_grab = Some(grab);
        self.grid_hscroll_set(relative, grab, cx);
    }

    pub(super) fn grid_hscroll_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some(grab) = self.hscroll_grab else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.hscroll_grab = None;
            cx.notify();
            return;
        }
        let relative = f32::from(event.position.x) - f32::from(self.hscroll.bounds().left());
        self.grid_hscroll_set(relative, grab, cx);
    }

    pub(super) fn grid_hscroll_set(
        &mut self,
        relative: f32,
        grab: f32,
        cx: &mut Context<'_, Self>,
    ) {
        let viewport = self.grid_scroll_viewport();
        let max = self.grid_scroll_max();
        let (_, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let thumb_x = (relative - grab).clamp(0.0, travel);
        let scroll = thumb_x / travel * max;
        self.set_grid_column_scroll(scroll);
        cx.notify();
    }

    pub(super) fn grid_vscroll_begin(&mut self, mouse_y: Pixels, cx: &mut Context<'_, Self>) {
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let bounds = handle.bounds();
        let viewport = f32::from(bounds.size.height);
        let max = (self.display_row_count() as f32 * GRID_ROW_HEIGHT - viewport).max(0.0);
        let (thumb_h, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let scroll = -f32::from(handle.offset().y);
        let thumb_y = (scroll / max) * travel;
        let relative = f32::from(mouse_y) - f32::from(bounds.top());
        let grab = if relative >= thumb_y && relative <= thumb_y + thumb_h {
            relative - thumb_y
        } else {
            thumb_h / 2.0
        };
        self.vscroll_grab = Some(grab);
        self.grid_vscroll_set(relative, grab, cx);
    }

    pub(super) fn grid_vscroll_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some(grab) = self.vscroll_grab else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.vscroll_grab = None;
            cx.notify();
            return;
        }
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let relative = f32::from(event.position.y) - f32::from(handle.bounds().top());
        self.grid_vscroll_set(relative, grab, cx);
    }

    pub(super) fn grid_vscroll_set(
        &mut self,
        relative: f32,
        grab: f32,
        cx: &mut Context<'_, Self>,
    ) {
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let viewport = f32::from(handle.bounds().size.height);
        let rows = self.display_row_count();
        let max = (rows as f32 * GRID_ROW_HEIGHT - viewport).max(0.0);
        let (_, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let thumb_y = (relative - grab).clamp(0.0, travel);
        let scroll = thumb_y / travel * max;
        let old_scroll = -f32::from(handle.offset().y);
        let x = handle.offset().x;
        handle.set_offset(Point::new(x, px(-scroll)));
        // Drag the selected cell along with the viewport. Without this the selection is left
        // off-screen, and the very next wheel notch (which moves the selection and then scrolls it
        // back into view) snapped the whole grid back to the selected row.
        self.follow_scrollbar(scroll, old_scroll, viewport, rows);
        cx.notify();
    }

    /// Move the selected cell by the same number of rows the viewport just scrolled, keeping its
    /// screen position and clamping it inside the viewport, so dragging the scrollbar carries the
    /// selection with it (at most it lands at the top/bottom of the visible rows).
    fn follow_scrollbar(&mut self, scroll: f32, old_scroll: f32, viewport: f32, rows: usize) {
        if rows == 0 || viewport <= 0.0 {
            return;
        }
        let Some((row, col)) = self
            .state
            .selection
            .as_ref()
            .map(|selection| selection.active_cursor())
        else {
            return;
        };
        let old_first = (old_scroll / GRID_ROW_HEIGHT).floor() as isize;
        let first = (scroll / GRID_ROW_HEIGHT).floor() as isize;
        let visible = (viewport / GRID_ROW_HEIGHT).ceil() as isize;
        let last = (first + visible).clamp(1, rows as isize);
        // The row the cursor should be on if it keeps its screen position, clamped so it stays
        // visible.
        let target = (row as isize) + (first - old_first);
        let new_row = target.clamp(first, (last - 1).max(first)) as usize;
        if new_row != row {
            self.state.selection = Some(CellSelection::single((new_row, col), (new_row, col)));
        }
    }
}
