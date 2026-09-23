//! A Windows-Explorer-style multi-selection state shared by the app's flat lists (the Users list,
//! the Backup list and the table/view object lists).
//!
//! The list owner keeps one [`ListSelection`] per pane and drives it from the row's mouse handlers
//! (a plain click starts a new single selection, Ctrl toggles, Shift extends from the last anchor)
//! and from the pane's background handlers (a drag paints a rubber band over the rows it touches).
//! The same state backs both the 详细列表 and 平铺网格 layouts, so a page's selection survives a
//! layout switch.

use std::collections::BTreeSet;

use gpui::{Bounds, Pixels, Point};

/// How a click or rubber band combines with the existing selection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelectMode {
    /// Replace the selection with the hit item(s) (a plain click / a fresh drag).
    Replace,
    /// Add the hit item(s) to the selection (Ctrl / Shift).
    Add,
    /// Remove the hit item(s) from the selection (Ctrl while dragging over selected rows).
    Toggle,
}

/// The multiple-selection state of one flat list. `items` holds the concrete row identities
/// (row indices, table names, ...), so the state is layout independent.
#[derive(Default)]
pub struct ListSelection {
    items: BTreeSet<String>,
    /// The item a Shift-click extends from — the last one clicked without Shift.
    anchor: Option<String>,
}

impl ListSelection {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, item: &str) -> bool {
        self.items.contains(item)
    }

    /// The selected items, in selection order (sorted for stable rendering/actions).
    pub fn items(&self) -> Vec<String> {
        self.items.iter().cloned().collect()
    }

    /// The single selected item, or `None` when nothing (or more than one thing) is selected.
    pub fn single(&self) -> Option<&str> {
        (self.items.len() == 1)
            .then(|| self.items.iter().next())
            .flatten()
            .map(String::as_str)
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.anchor = None;
    }

    /// Select one item, replacing whatever was selected.
    pub fn select_one(&mut self, item: impl Into<String>) {
        let item = item.into();
        self.items.clear();
        self.items.insert(item.clone());
        self.anchor = Some(item);
    }

    /// Extend the selection from the anchor to `item`, over `visible` (the list's current order).
    /// Falls back to selecting `item` alone when there is no anchor.
    pub fn extend_to(&mut self, visible: &[String], item: &str) {
        let Some(anchor) = self.anchor.clone() else {
            self.select_one(item.to_string());
            return;
        };
        let Some(start) = visible.iter().position(|candidate| candidate == &anchor) else {
            self.select_one(item.to_string());
            return;
        };
        let Some(end) = visible.iter().position(|candidate| candidate == item) else {
            self.select_one(item.to_string());
            return;
        };
        let (start, end) = (start.min(end), start.max(end));
        self.items = visible[start..=end].iter().cloned().collect();
    }

    /// Add one item to the selection without moving the anchor.
    pub fn add(&mut self, item: impl Into<String>) {
        self.items.insert(item.into());
    }

    /// Toggle one item in or out of the selection.
    pub fn toggle(&mut self, item: &str) {
        if !self.items.remove(item) {
            self.items.insert(item.to_string());
            self.anchor = Some(item.to_string());
        }
    }

    /// Apply one row hit under `mode`; `visible` is the list's current order (for Shift).
    pub fn hit(&mut self, _visible: &[String], item: &str, mode: SelectMode) {
        match mode {
            SelectMode::Replace => self.select_one(item.to_string()),
            SelectMode::Add => {
                self.add(item.to_string());
                self.anchor = Some(item.to_string());
            }
            SelectMode::Toggle => self.toggle(item),
        }
    }

    /// Apply a marquee that currently covers `hits`, using the mode captured when the drag began.
    pub fn marquee(&mut self, base: &[String], hits: &[String], mode: SelectMode) {
        match mode {
            SelectMode::Replace => {
                self.items = hits.iter().cloned().collect();
            }
            SelectMode::Add => {
                self.items = base.iter().cloned().collect();
                self.items.extend(hits.iter().cloned());
            }
            SelectMode::Toggle => {
                self.items = base.iter().cloned().collect();
                for hit in hits {
                    if !self.items.remove(hit) {
                        self.items.insert(hit.clone());
                    }
                }
            }
        }
    }
}

/// An in-progress rubber-band drag over a list pane.
#[derive(Clone)]
pub struct MarqueeDrag {
    /// Where the drag started, in window space.
    pub origin: Point<Pixels>,
    /// The current pointer position, in window space.
    pub current: Point<Pixels>,
    /// How the marquee combines with the selection captured at drag start.
    pub mode: SelectMode,
    /// The selection at drag start, so dragging back and forth is not cumulative.
    pub base: Vec<String>,
}

impl MarqueeDrag {
    pub fn new(origin: Point<Pixels>, mode: SelectMode, base: Vec<String>) -> Self {
        Self {
            origin,
            current: origin,
            mode,
            base,
        }
    }

    /// The normalized rectangle of the drag, in window space.
    pub fn rect(&self) -> (Point<Pixels>, Point<Pixels>) {
        (self.origin, self.current)
    }
}

/// Whether a window-space rectangle (the marquee) intersects a row's bounds.
pub fn rects_intersect(
    marquee_left: Pixels,
    marquee_top: Pixels,
    marquee_right: Pixels,
    marquee_bottom: Pixels,
    row: Bounds<Pixels>,
) -> bool {
    row.right() >= marquee_left
        && row.left() <= marquee_right
        && row.bottom() >= marquee_top
        && row.top() <= marquee_bottom
}

/// The selection mode a mouse-down's modifiers ask for.
pub fn selection_mode(modifiers: gpui::Modifiers) -> SelectMode {
    if modifiers.shift {
        SelectMode::Add
    } else if modifiers.control || modifiers.platform {
        SelectMode::Toggle
    } else {
        SelectMode::Replace
    }
}
