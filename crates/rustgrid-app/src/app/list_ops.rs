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
            MarqueeTarget::Transfers => &self.transfers_selection,
            MarqueeTarget::Queries => &self.query_selection,
            MarqueeTarget::Objects => &self.objects_selection,
        }
    }

    /// The selection of one marquee-capable list, mutably.
    pub(super) fn marquee_selection_mut(&mut self, target: MarqueeTarget) -> &mut ListSelection {
        match target {
            MarqueeTarget::Users => &mut self.users_selection,
            MarqueeTarget::Backups => &mut self.backups_selection,
            MarqueeTarget::Transfers => &mut self.transfers_selection,
            MarqueeTarget::Queries => &mut self.query_selection,
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
            MarqueeTarget::Transfers => &self.transfers_row_rects,
            MarqueeTarget::Queries => &self.query_row_rects,
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
            MarqueeTarget::Transfers => &mut self.transfers_row_rects,
            MarqueeTarget::Queries => &mut self.query_row_rects,
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
            // Push the cleared state to any single-selection mirror the page keeps (the object
            // list's lives on its child pane; the query list's is `saved_query_selected`).
            self.on_selection_changed(target, cx);
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

    /// Select the query/backup list entry a tree leaf stands for.
    ///
    /// Mouse clicks on a leaf already call `hit_backup` / `hit_query`; the keyboard moves the
    /// tree's cursor without touching the list, so the tree painted both its cursor and the
    /// list's previous selection. Keep them in step from either input.
    pub(super) fn sync_list_selection_for_tree_leaf(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(rest) = id.strip_prefix("bak-") {
            let mut parts = rest.splitn(4, '-');
            let (Some(_connection), Some(_database), Some(kind), Some(index)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return;
            };
            let Some(index) = index.parse::<usize>().ok() else {
                return;
            };
            let key = match kind {
                "f" => self
                    .backup_files
                    .get(index)
                    .map(|file| backup::backup_file_key(&file.name)),
                "c" => self
                    .backup_configs
                    .get(index)
                    .map(|config| backup::backup_config_key(&config.name)),
                _ => None,
            };
            if let Some(key) = key {
                self.hit_backup(&key, 1, Modifiers::default(), cx);
            }
            return;
        }
        let Some(rest) = id.strip_prefix("qry-") else {
            return;
        };
        let mut parts = rest.splitn(3, '-');
        let (_connection, _database, index) = (parts.next(), parts.next(), parts.next());
        let Some(index) = index.and_then(|value| value.parse::<usize>().ok()) else {
            return;
        };
        if let Some(key) = self.query_files.get(index).map(query::query_file_key) {
            self.hit_query(&key, 1, Modifiers::default(), cx);
        }
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
        if target == MarqueeTarget::Queries {
            self.sync_query_selected();
            self.sync_query_tree_cursor(cx);
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

/// Whether a connection-tree row key takes part in the tree's multi-selection. Only connection and
/// group rows are selectable; databases and objects stay single-select so the content pane's
/// scoping is unaffected.
pub(super) fn is_tree_selectable(key: &str) -> bool {
    key.starts_with("conn-") || key.starts_with("group-")
}

impl AppView {
    /// The connection tree's multi-selection (connections and groups).
    pub(super) fn tree_selection(&self) -> &ListSelection {
        &self.tree_selection
    }

    /// Whether more than one connection/group row is selected, i.e. the tree's multi-selection
    /// menu should replace the per-item one.
    pub(super) fn tree_multi_selected(&self) -> bool {
        self.tree_selection.items().len() > 1
    }

    /// Apply a click on a selectable tree row (connection/group) under the click's modifier mode.
    /// `visible` is the tree's current order, so Shift extends from the anchor.
    pub(super) fn tree_row_click(
        &mut self,
        key: &str,
        visible: &[String],
        mode: SelectMode,
        shift: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if shift {
            self.tree_selection.extend_to(visible, key);
        } else {
            self.tree_selection.hit(visible, key, mode);
        }
        cx.notify();
    }

    /// Ctrl+A over the tree: select every visible connection/group row.
    pub(super) fn tree_select_all(&mut self, visible: &[String], cx: &mut Context<'_, Self>) {
        self.tree_selection.select_all(visible);
        cx.notify();
    }

    /// Clear the connection/group multi-selection when the user moves the tree selection onto a
    /// non-selectable row (a database, schema, category or object), so only the row just clicked
    /// stays highlighted.
    pub(super) fn clear_tree_selection(&mut self, cx: &mut Context<'_, Self>) {
        if !self.tree_selection.items().is_empty() {
            self.tree_selection.clear();
            cx.notify();
        }
    }

    /// The connection-tree multi-selection's connection profile ids, with each selected group
    /// expanded to every connection under it (used by 打开连接 / 关闭连接).
    pub(super) fn tree_selection_connection_ids(&self) -> Vec<String> {
        let mut ids = BTreeSet::new();
        for key in self.tree_selection.items() {
            if let Some(rest) = key.strip_prefix("conn-") {
                if let Ok(index) = rest.parse::<usize>()
                    && let Some(node) = self.connections.get(index)
                {
                    ids.insert(node.profile.id.clone());
                }
            } else if let Some(rest) = key.strip_prefix("group-")
                && let Ok(group_id) = rest.parse::<u64>()
            {
                groups::collect_subtree_connection_ids(&self.tree_layout, group_id, &mut ids);
            }
        }
        ids.into_iter().collect()
    }

    /// The connection-tree multi-selection's connection indices, in profile order, with each
    /// selected group expanded to its connections.
    pub(super) fn tree_selection_connection_indexes(&self) -> Vec<usize> {
        let ids: BTreeSet<String> = self.tree_selection_connection_ids().into_iter().collect();
        self.connections
            .iter()
            .enumerate()
            .filter(|(_, node)| ids.contains(&node.profile.id))
            .map(|(index, _)| index)
            .collect()
    }

    /// The selected group ids (for 删除连接, which removes groups and their connections together).
    pub(super) fn tree_selection_group_ids(&self) -> Vec<u64> {
        self.tree_selection
            .items()
            .iter()
            .filter_map(|key| key.strip_prefix("group-"))
            .filter_map(|rest| rest.parse::<u64>().ok())
            .collect()
    }

    /// The selected connection profile ids (不展开分组).
    pub(super) fn tree_selection_connection_profile_ids(&self) -> Vec<String> {
        self.tree_selection
            .items()
            .iter()
            .filter_map(|key| key.strip_prefix("conn-"))
            .filter_map(|rest| rest.parse::<usize>().ok())
            .filter_map(|index| {
                self.connections
                    .get(index)
                    .map(|node| node.profile.id.clone())
            })
            .collect()
    }

    /// The tree multi-selection as drag payloads (connections by profile id, groups by id).
    pub(super) fn tree_selection_payloads(&self) -> Vec<TreeDragPayload> {
        self.tree_selection
            .items()
            .iter()
            .filter_map(|key| {
                if let Some(rest) = key.strip_prefix("conn-") {
                    rest.parse::<usize>()
                        .ok()
                        .and_then(|index| self.connections.get(index))
                        .map(|node| TreeDragPayload::Connection(node.profile.id.clone()))
                } else {
                    key.strip_prefix("group-")
                        .and_then(|rest| rest.parse::<u64>().ok())
                        .map(TreeDragPayload::Group)
                }
            })
            .collect()
    }

    /// The payloads a press on `payload` should drag: the pressed row alone, or the whole
    /// multi-selection when the pressed row is part of one.
    pub(super) fn tree_drag_payloads_for(&self, payload: &TreeDragPayload) -> Vec<TreeDragPayload> {
        if self.tree_selection.items().len() > 1 {
            let key = match payload {
                TreeDragPayload::Connection(id) => self
                    .connections
                    .iter()
                    .position(|node| &node.profile.id == id)
                    .map(|index| format!("conn-{index}")),
                TreeDragPayload::Group(id) => Some(format!("group-{id}")),
            };
            if let Some(key) = key
                && self.tree_selection.contains(&key)
            {
                return self.tree_selection_payloads();
            }
        }
        vec![payload.clone()]
    }

    /// Begin a rubber-band drag over the connection tree from `position`.
    pub(super) fn begin_tree_marquee(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &mut Context<'_, Self>,
    ) {
        let mode = selection_mode(modifiers);
        let base = self.tree_selection.items();
        if mode == SelectMode::Replace {
            self.tree_selection.clear();
        }
        self.tree_marquee = Some(MarqueeDrag::new(position, mode, base));
        cx.notify();
    }

    /// Extend the tree's rubber band to `event`'s position and repaint the selection.
    pub(super) fn drag_tree_marquee(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            if self.tree_marquee.take().is_some() {
                cx.notify();
            }
            return;
        }
        let Some(drag) = self.tree_marquee.as_mut() else {
            return;
        };
        drag.current = event.position;
        let (origin, current) = drag.rect();
        let (left, top) = (origin.x.min(current.x), origin.y.min(current.y));
        let (right, bottom) = (origin.x.max(current.x), origin.y.max(current.y));
        let mode = drag.mode;
        let base = drag.base.clone();

        let mut hits: Vec<(String, Bounds<Pixels>)> = self
            .tree_row_rects
            .iter()
            .filter(|(key, _)| is_tree_selectable(key))
            .filter(|(_, rect)| rects_intersect(left, top, right, bottom, **rect))
            .map(|(key, rect)| (key.clone(), *rect))
            .collect();
        hits.sort_by(|a, b| {
            (a.1.top(), a.1.left())
                .partial_cmp(&(b.1.top(), b.1.left()))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let hit_keys: Vec<String> = hits.into_iter().map(|(key, _)| key).collect();
        self.tree_selection.marquee(&base, &hit_keys, mode);
        cx.notify();
    }

    /// Finish the tree's rubber-band drag.
    pub(super) fn end_tree_marquee(&mut self, cx: &mut Context<'_, Self>) {
        if self.tree_marquee.take().is_some() {
            cx.notify();
        }
    }

    /// Whether a press position lands on a connection/group row, which starts a drag-reorder
    /// rather than a rubber-band selection.
    pub(super) fn tree_press_on_reorder_row(&self, position: Point<Pixels>) -> bool {
        self.tree_row_rects
            .iter()
            .any(|(key, rect)| is_tree_selectable(key) && rect.contains(&position))
    }

    /// Note the connection tree's window-space bounds (from the tree pane's paint), so the rubber
    /// band is clamped to the pane.
    pub(super) fn note_tree_bounds(&mut self, rect: Bounds<Pixels>) {
        self.tree_bounds = Some(rect);
    }

    /// The rubber-band rectangle to draw over the connection tree, in window coordinates.
    pub(super) fn tree_marquee_rect(
        &self,
    ) -> Option<(Point<Pixels>, Point<Pixels>, Bounds<Pixels>)> {
        let drag = self.tree_marquee.as_ref()?;
        let (origin, current) = drag.rect();
        // Prefer the tree pane's own bounds; fall back to the union of the selectable rows.
        let bounds = self.tree_bounds.or_else(|| {
            let mut bounds: Option<Bounds<Pixels>> = None;
            for (key, rect) in &self.tree_row_rects {
                if !is_tree_selectable(key) {
                    continue;
                }
                bounds = Some(match bounds {
                    None => *rect,
                    Some(bounds) => Bounds {
                        origin: Point::new(
                            bounds.left().min(rect.left()),
                            bounds.top().min(rect.top()),
                        ),
                        size: size(
                            bounds.right().max(rect.right()) - bounds.left().min(rect.left()),
                            bounds.bottom().max(rect.bottom()) - bounds.top().min(rect.top()),
                        ),
                    },
                });
            }
            bounds
        })?;
        Some((origin, current, bounds))
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

/// Wrap a list row/tile so it publishes its window-space rectangle for the marquee. Every
/// marquee-capable list (Users, Backup, Queries, Tables/Views/Functions) uses this same wrapper.
pub(super) fn row_with_rect(
    app: WeakEntity<AppView>,
    row: impl IntoElement + 'static,
    target: MarqueeTarget,
    key: String,
) -> AnyElement {
    div()
        .on_children_prepainted(move |bounds, _window, cx| {
            let Some(rect) = bounds.first().copied() else {
                return;
            };
            let _ = app.update(cx, |app, _| app.note_row_rect(target, key.clone(), rect));
        })
        .child(row)
        .into_any_element()
}
