//! Windows-Explorer-style list interaction shared by the Users list, the Backup list and the
//! table/view object list: rubber-band marquee selection over rows plus an in-place F2 rename.
//!
//! Each list publishes its row rectangles (window space) through `on_children_prepainted` and
//! routes its mouse handlers here. [`ListSelection`] holds the selected row keys, so the same state
//! backs both the 详细列表 and 平铺网格 layouts.

use super::*;

impl AppView {
    /// The selection of one marquee-capable list.
    pub(super) fn marquee_selection(&self, target: MarqueeTarget) -> &ListSelection {
        match target {
            MarqueeTarget::Users => &self.users_selection,
            MarqueeTarget::Backups => &self.backups_selection,
            MarqueeTarget::Objects => &self.objects_selection,
        }
    }

    /// The selection of one marquee-capable list, mutably.
    pub(super) fn marquee_selection_mut(&mut self, target: MarqueeTarget) -> &mut ListSelection {
        match target {
            MarqueeTarget::Users => &mut self.users_selection,
            MarqueeTarget::Backups => &mut self.backups_selection,
            MarqueeTarget::Objects => &mut self.objects_selection,
        }
    }

    /// The row rectangles one list published this frame.
    pub(super) fn marquee_rects(
        &self,
        target: MarqueeTarget,
    ) -> &std::collections::HashMap<String, Bounds<Pixels>> {
        match target {
            MarqueeTarget::Users => &self.users_row_rects,
            MarqueeTarget::Backups => &self.backups_row_rects,
            MarqueeTarget::Objects => &self.objects_row_rects,
        }
    }

    /// Note one row's window-space rectangle (called from the row's `on_children_prepainted`).
    pub(super) fn note_row_rect(
        &mut self,
        target: MarqueeTarget,
        key: String,
        rect: Bounds<Pixels>,
    ) {
        self.marquee_rects_mut(target).insert(key, rect);
    }

    /// Clear one list's row rectangles, so a row that disappeared stops matching the marquee.
    pub(super) fn clear_row_rects(&mut self, target: MarqueeTarget) {
        self.marquee_rects_mut(target).clear();
    }

    fn marquee_rects_mut(
        &mut self,
        target: MarqueeTarget,
    ) -> &mut std::collections::HashMap<String, Bounds<Pixels>> {
        match target {
            MarqueeTarget::Users => &mut self.users_row_rects,
            MarqueeTarget::Backups => &mut self.backups_row_rects,
            MarqueeTarget::Objects => &mut self.objects_row_rects,
        }
    }

    /// Begin a rubber-band drag from `position`, capturing the click's modifier mode.
    pub(super) fn begin_marquee(
        &mut self,
        target: MarqueeTarget,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        let mode = selection_mode(modifiers);
        let base = self.marquee_selection(target).items();
        // A fresh (non-additive) drag clears the selection immediately, like Explorer.
        if mode == SelectMode::Replace {
            self.marquee_selection_mut(target).clear();
            // The object list's single-selection mirror lives on its child pane, so push the
            // cleared state to it (the pane draws the toolbar's enabled state).
            if target == MarqueeTarget::Objects {
                self.on_selection_changed(MarqueeTarget::Objects, cx);
            }
        }
        self.marquee = Some((target, MarqueeDrag::new(position, mode, base)));
        cx.notify();
    }

    /// Extend the marquee to `event`'s position and repaint the selection.
    pub(super) fn drag_marquee(
        &mut self,
        target: MarqueeTarget,
        event: &MouseMoveEvent,
        cx: &mut Context<'_, Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) {
            if let Some((dragging, _)) = &self.marquee
                && *dragging == target
            {
                self.marquee = None;
                cx.notify();
            }
            return;
        }
        let Some((dragging, drag)) = self.marquee.as_mut() else {
            return;
        };
        if *dragging != target {
            return;
        }
        drag.current = event.position;
        let (origin, current) = drag.rect();
        let (left, top) = (origin.x.min(current.x), origin.y.min(current.y));
        let (right, bottom) = (origin.x.max(current.x), origin.y.max(current.y));
        let mode = drag.mode;
        let base = drag.base.clone();

        let mut hits: Vec<(String, Bounds<Pixels>)> = self
            .marquee_rects(target)
            .iter()
            .filter(|(_, rect)| rects_intersect(left, top, right, bottom, **rect))
            .map(|(key, rect)| (key.clone(), *rect))
            .collect();
        // Apply in visual order so the anchor-based modes stay deterministic.
        hits.sort_by(|a, b| {
            (a.1.top(), a.1.left())
                .partial_cmp(&(b.1.top(), b.1.left()))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let hit_keys: Vec<String> = hits.into_iter().map(|(key, _)| key).collect();
        self.marquee_selection_mut(target)
            .marquee(&base, &hit_keys, mode);
        self.on_selection_changed(target, cx);
        cx.notify();
    }

    /// Finish a rubber-band drag.
    pub(super) fn end_marquee(&mut self, cx: &mut Context<'_, Self>) {
        if self.marquee.take().is_some() {
            cx.notify();
        }
    }

    /// The rubber-band rectangle to draw over one list, if a drag is active there.
    pub(super) fn marquee_rect_for(&self, target: MarqueeTarget) -> Option<AnyElement> {
        let (dragging, drag) = self.marquee.as_ref()?;
        if *dragging != target {
            return None;
        }
        let (origin, current) = drag.rect();
        let bounds = self
            .marquee_bounds(target)
            .unwrap_or_else(|| Bounds::new(Point::default(), size(px(1.0), px(1.0))));
        ui::marquee_rect(origin, current, bounds, self.theme).map(|rect| rect.into_any_element())
    }

    /// The window-space bounds of one list pane, where the marquee is clamped.
    fn marquee_bounds(&self, target: MarqueeTarget) -> Option<Bounds<Pixels>> {
        let rects = self.marquee_rects(target);
        let mut iter = rects.values();
        let first = iter.next()?;
        let mut bounds = *first;
        for rect in iter {
            bounds = Bounds {
                origin: Point::new(bounds.left().min(rect.left()), bounds.top().min(rect.top())),
                size: size(
                    bounds.right().max(rect.right()) - bounds.left().min(rect.left()),
                    bounds.bottom().max(rect.bottom()) - bounds.top().min(rect.top()),
                ),
            };
        }
        Some(bounds)
    }

    /// Hook for a list whose selection just changed (e.g. to resync a toolbar).
    pub(super) fn on_selection_changed(
        &mut self,
        target: MarqueeTarget,
        cx: &mut Context<'_, Self>,
    ) {
        if target == MarqueeTarget::Users {
            self.selected_user = self
                .users_selection
                .single()
                .and_then(|key| self.user_index_by_key(key));
            self.info_loaded_for = None;
        }
        if target == MarqueeTarget::Objects {
            // The object list's single-selection mirror lives on its child pane, so push the new
            // selection there. Deferred: this can run from inside a pane listener/update.
            if let Some(pane) = self.object_pane.clone() {
                cx.defer(move |cx| {
                    pane.update(cx, |pane, cx| pane.sync_single_from_app(cx));
                });
            }
        }
        cx.notify();
    }

    /// Apply one object-list row hit (click) under the click's modifier mode. `visible` is the
    /// list's current order, so Shift extends from the anchor.
    pub(super) fn hit_object_selection(
        &mut self,
        visible: &[String],
        key: &str,
        mode: SelectMode,
        shift: bool,
    ) {
        if shift {
            self.objects_selection.extend_to(visible, key);
        } else {
            self.objects_selection.hit(visible, key, mode);
        }
    }
}

/// The selection mode a mouse-down's modifiers ask for.
pub(super) fn selection_mode(modifiers: Modifiers) -> SelectMode {
    if modifiers.shift {
        SelectMode::Add
    } else if modifiers.control || modifiers.platform {
        SelectMode::Toggle
    } else {
        SelectMode::Replace
    }
}
