use super::*;

impl GridView {
    pub(super) fn render_grid_vscrollbar(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let viewport = f32::from(handle.bounds().size.height);
        let max = (self.state.rows.len() as f32 * GRID_ROW_HEIGHT - viewport).max(0.0);
        if max <= 0.0 {
            return div().into_any_element();
        }
        let scroll = -f32::from(handle.offset().y);
        let (thumb_top, thumb_len) = scrollbar_fractions(viewport, max, scroll);

        div()
            .flex_none()
            .h_full()
            .pt(px(GRID_ROW_HEIGHT))
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
        let viewport = f32::from(self.hscroll.bounds().size.width);
        let max = f32::from(self.hscroll.max_offset().width);
        if max <= 0.0 {
            // Reserve the same height as a real scrollbar so the vertical track's viewport stays
            // constant whether or not the grid can scroll horizontally.
            return div()
                .flex_none()
                .h(px(GRID_SCROLLBAR_THICKNESS))
                .into_any_element();
        }
        let scroll = -f32::from(self.hscroll.offset().x);
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
        let viewport = f32::from(bounds.size.width);
        let max = f32::from(self.hscroll.max_offset().width);
        let (thumb_w, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let scroll = -f32::from(self.hscroll.offset().x);
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

    pub(super) fn grid_hscroll_set(&self, relative: f32, grab: f32, cx: &mut Context<'_, Self>) {
        let viewport = f32::from(self.hscroll.bounds().size.width);
        let max = f32::from(self.hscroll.max_offset().width);
        let (_, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let thumb_x = (relative - grab).clamp(0.0, travel);
        let scroll = thumb_x / travel * max;
        let y = self.hscroll.offset().y;
        self.hscroll.set_offset(Point::new(px(-scroll), y));
        cx.notify();
    }

    pub(super) fn grid_vscroll_begin(&mut self, mouse_y: Pixels, cx: &mut Context<'_, Self>) {
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let bounds = handle.bounds();
        let viewport = f32::from(bounds.size.height);
        let max = (self.state.rows.len() as f32 * GRID_ROW_HEIGHT - viewport).max(0.0);
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

    pub(super) fn grid_vscroll_set(&self, relative: f32, grab: f32, cx: &mut Context<'_, Self>) {
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let viewport = f32::from(handle.bounds().size.height);
        let max = (self.state.rows.len() as f32 * GRID_ROW_HEIGHT - viewport).max(0.0);
        let (_, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let thumb_y = (relative - grab).clamp(0.0, travel);
        let scroll = thumb_y / travel * max;
        let x = handle.offset().x;
        handle.set_offset(Point::new(x, px(-scroll)));
        cx.notify();
    }
}
