//! Connection-tree groups: the nested layout of groups and connections, plus the actions that
//! create, rename, delete, move and reorder them.
//!
//! The tree's display order and nesting live in `AppView::tree_layout` (persisted in settings);
//! `AppView::connections` still owns the live connection nodes and is indexed everywhere else. A
//! connection is referenced from the layout by its `ConnectionProfile::id`, so reordering the
//! profiles never invalidates the layout.

use std::collections::HashSet;
use std::rc::Rc;

use super::*;

/// Where a dragged tree entry should land.
#[derive(Clone)]
pub(super) enum TreeDrop {
    /// Append at the end of the root level.
    RootEnd,
    /// Append as the last child of this group.
    Into(u64),
    /// Insert directly before this entry, in its current parent.
    Before(TreeDragPayload),
    /// Insert directly after this entry, in its current parent.
    After(TreeDragPayload),
}

/// Remove a connection from the tree layout (used when the connection is deleted).
pub(super) fn remove_connection_entry(layout: &mut Vec<GroupEntry>, connection_id: &str) {
    remove_entry(
        layout,
        &TreeDragPayload::Connection(connection_id.to_string()),
    );
}

/// Reconcile the persisted tree layout with the loaded profiles: drop entries for connections that
/// no longer exist (and de-duplicate a connection mentioned twice), then append every profile the
/// layout does not mention yet at the root, in profile order.
pub(super) fn reconcile_tree(layout: &mut Vec<GroupEntry>, connections: &[ConnectionNode]) {
    let ids: Vec<String> = connections
        .iter()
        .map(|node| node.profile.id.clone())
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    prune_tree(layout, &ids, &mut seen);
    for id in ids {
        if !seen.contains(&id) {
            layout.push(GroupEntry::Connection(id));
        }
    }
}

fn prune_tree(entries: &mut Vec<GroupEntry>, ids: &[String], seen: &mut HashSet<String>) {
    entries.retain_mut(|entry| match entry {
        GroupEntry::Connection(id) => ids.contains(id) && seen.insert(id.clone()),
        GroupEntry::Group(group) => {
            prune_tree(&mut group.children, ids, seen);
            true
        }
    });
}

/// Whether `entry` is the same node the payload names.
fn entry_matches(entry: &GroupEntry, payload: &TreeDragPayload) -> bool {
    match (entry, payload) {
        (GroupEntry::Connection(id), TreeDragPayload::Connection(target)) => id == target,
        (GroupEntry::Group(group), TreeDragPayload::Group(target)) => group.id == *target,
        _ => false,
    }
}

/// Remove one entry (a connection or a whole group) from anywhere in the tree.
fn remove_entry(entries: &mut Vec<GroupEntry>, payload: &TreeDragPayload) -> Option<GroupEntry> {
    if let Some(index) = entries
        .iter()
        .position(|entry| entry_matches(entry, payload))
    {
        return Some(entries.remove(index));
    }
    for entry in entries.iter_mut() {
        if let GroupEntry::Group(group) = entry
            && let Some(found) = remove_entry(&mut group.children, payload)
        {
            return Some(found);
        }
    }
    None
}

/// Insert several `moving` entries directly before/after `target`, in whatever sibling list holds
/// `target`, preserving their order.
fn insert_relative_all(
    entries: &mut Vec<GroupEntry>,
    target: &TreeDragPayload,
    moving: &[GroupEntry],
    after: bool,
) -> bool {
    if let Some(index) = entries
        .iter()
        .position(|entry| entry_matches(entry, target))
    {
        let at = if after { index + 1 } else { index };
        for (offset, entry) in moving.iter().enumerate() {
            entries.insert(at + offset, entry.clone());
        }
        return true;
    }
    for entry in entries.iter_mut() {
        if let GroupEntry::Group(group) = entry
            && insert_relative_all(&mut group.children, target, moving, after)
        {
            return true;
        }
    }
    false
}

/// The group with this id, anywhere in the tree.
pub(super) fn find_group(entries: &[GroupEntry], id: u64) -> Option<&ConnectionGroup> {
    for entry in entries {
        if let GroupEntry::Group(group) = entry {
            if group.id == id {
                return Some(group);
            }
            if let Some(found) = find_group(&group.children, id) {
                return Some(found);
            }
        }
    }
    None
}

fn find_group_mut(entries: &mut [GroupEntry], id: u64) -> Option<&mut ConnectionGroup> {
    for entry in entries.iter_mut() {
        if let GroupEntry::Group(group) = entry {
            if group.id == id {
                return Some(group);
            }
            if let Some(found) = find_group_mut(&mut group.children, id) {
                return Some(found);
            }
        }
    }
    None
}

/// Every group in the tree, in display order, as `(id, depth)`.
pub(super) fn collect_groups(entries: &[GroupEntry], depth: usize, out: &mut Vec<(u64, usize)>) {
    for entry in entries {
        if let GroupEntry::Group(group) = entry {
            out.push((group.id, depth));
            collect_groups(&group.children, depth + 1, out);
        }
    }
}

/// Collapse every group (recursively), so a launch that does not restore the previous session
/// shows the tree closed instead of reopening last run's groups.
pub(super) fn collapse_all(entries: &mut [GroupEntry]) {
    for entry in entries {
        if let GroupEntry::Group(group) = entry {
            group.expanded = false;
            collapse_all(&mut group.children);
        }
    }
}

/// The next free group id (one past the largest in use).
pub(super) fn next_group_id(entries: &[GroupEntry]) -> u64 {
    let mut ids = Vec::new();
    collect_group_ids(entries, &mut ids);
    ids.into_iter().max().map_or(1, |max| max + 1)
}

fn collect_group_ids(entries: &[GroupEntry], out: &mut Vec<u64>) {
    for entry in entries {
        if let GroupEntry::Group(group) = entry {
            out.push(group.id);
            collect_group_ids(&group.children, out);
        }
    }
}

/// Collect the profile ids of every connection inside the group `id` (recursively). No-op when the
/// group does not exist.
pub(super) fn collect_subtree_connection_ids(
    entries: &[GroupEntry],
    id: u64,
    out: &mut BTreeSet<String>,
) {
    if let Some(group) = find_group(entries, id) {
        collect_connection_ids(&group.children, out);
    }
}

fn collect_connection_ids(entries: &[GroupEntry], out: &mut BTreeSet<String>) {
    for entry in entries {
        match entry {
            GroupEntry::Connection(id) => {
                out.insert(id.clone());
            }
            GroupEntry::Group(group) => collect_connection_ids(&group.children, out),
        }
    }
}

/// The group that directly holds `connection_id`, if any.
pub(super) fn group_of_connection(entries: &[GroupEntry], connection_id: &str) -> Option<u64> {
    for entry in entries {
        if let GroupEntry::Group(group) = entry {
            if group
                .children
                .iter()
                .any(|child| matches!(child, GroupEntry::Connection(id) if id == connection_id))
            {
                return Some(group.id);
            }
            if let Some(found) = group_of_connection(&group.children, connection_id) {
                return Some(found);
            }
        }
    }
    None
}

impl AppView {
    /// The group with this id, for rendering.
    pub(super) fn group(&self, id: u64) -> Option<&ConnectionGroup> {
        find_group(&self.tree_layout, id)
    }

    /// Every group in the tree, in display order, as `(id, depth)`.
    pub(super) fn groups_flat(&self) -> Vec<(u64, usize)> {
        let mut out = Vec::new();
        collect_groups(&self.tree_layout, 0, &mut out);
        out
    }

    /// The group a connection currently sits in, if any.
    pub(super) fn connection_group(&self, index: usize) -> Option<u64> {
        let id = self.connections.get(index)?.profile.id.clone();
        group_of_connection(&self.tree_layout, &id)
    }

    /// Persist the tree layout together with the rest of the settings.
    fn persist_tree(&self) {
        self.persist_settings();
    }

    /// Create a new empty group at the root, named 新分组, and start renaming it in place.
    pub(super) fn add_group(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let id = next_group_id(&self.tree_layout);
        self.tree_layout.push(GroupEntry::Group(ConnectionGroup {
            id,
            name: t!("group.default_name").to_string(),
            expanded: true,
            children: Vec::new(),
        }));
        self.persist_tree();
        self.begin_group_rename(id, window, cx);
        cx.notify();
    }

    /// Toggle a group's expanded state.
    pub(super) fn toggle_group(&mut self, id: u64, cx: &mut Context<'_, Self>) {
        if let Some(group) = find_group_mut(&mut self.tree_layout, id) {
            group.expanded = !group.expanded;
            self.persist_tree();
            cx.notify();
        }
    }

    /// Open the in-place rename editor for a group.
    pub(super) fn begin_group_rename(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(group) = find_group(&self.tree_layout, id) else {
            return;
        };
        let initial = group.name.clone();
        let initial_for_input = initial.clone();
        let theme = self.theme;
        let weak = cx.weak_entity();
        let change = weak.clone();
        let submit = weak.clone();
        let cancel = weak.clone();
        let blur = weak.clone();
        let input = cx.new(move |cx| {
            TextInput::new(
                theme,
                initial_for_input,
                TextInputOptions {
                    bare: true,
                    text_size: Some(12.0),
                    size: Some(gpui_kit::component::Size::XSmall),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| {
                    if let Some(edit) = app.group_rename.as_mut() {
                        edit.name = text.to_string();
                    }
                    cx.notify();
                });
            }))
            .on_submit(Rc::new(move |window, cx| {
                let _ = submit.update(cx, |app, cx| {
                    app.submit_group_rename(cx);
                    window.focus(&app.tree_pane.read(cx).focus_handle(), cx);
                });
            }))
            .on_cancel(Rc::new(move |window, cx| {
                let _ = cancel.update(cx, |app, cx| {
                    app.group_rename = None;
                    window.focus(&app.tree_pane.read(cx).focus_handle(), cx);
                    cx.notify();
                });
            }))
            // Losing focus commits the edit (click-away saves); ESC already cleared it above, so
            // this is a no-op then.
            .on_blur(Rc::new(move |window, cx| {
                let _ = blur.update(cx, |app, cx| {
                    if app.group_rename.is_some() {
                        app.submit_group_rename(cx);
                        window.focus(&app.tree_pane.read(cx).focus_handle(), cx);
                    }
                });
            }))
        });
        let input_entity = input.clone();
        self.group_rename = Some(GroupRenameEdit {
            id,
            name: initial,
            input,
        });
        self.group_rename_focus_pending = true;
        // Focus the inner gpui-kit input directly (not the wrapper handle): only the inner state
        // emits `InputEvent::Blur`, which drives the commit-on-click-away.
        input_entity.update(cx, |input, cx| input.focus_state(window, cx));
        cx.notify();
    }

    /// Commit an open group rename (blank names are ignored so a group keeps a usable label).
    pub(super) fn submit_group_rename(&mut self, cx: &mut Context<'_, Self>) {
        let Some(edit) = self.group_rename.take() else {
            return;
        };
        let name = edit.name.trim().to_string();
        if !name.is_empty()
            && let Some(group) = find_group_mut(&mut self.tree_layout, edit.id)
        {
            group.name = name;
            self.persist_tree();
        }
        cx.notify();
    }

    /// Remove a group, keeping its children: they are spliced into the group's parent at the
    /// group's position, so connections are never deleted with the group.
    pub(super) fn delete_group(&mut self, id: u64, cx: &mut Context<'_, Self>) {
        remove_group(&mut self.tree_layout, id);
        if self.group_rename.as_ref().is_some_and(|edit| edit.id == id) {
            self.group_rename = None;
        }
        // A deleted group cannot stay selected.
        let key = format!("group-{id}");
        self.tree_selection
            .retain(|selected| selected != key.as_str());
        self.persist_tree();
        cx.notify();
    }

    /// Reload every connected connection's databases (the tree's 刷新).
    pub(super) fn refresh_tree(&mut self, cx: &mut Context<'_, Self>) {
        let indexes: Vec<usize> = self
            .connections
            .iter()
            .enumerate()
            .filter(|(_, node)| matches!(node.status, ConnectionStatus::Connected(_)))
            .map(|(index, _)| index)
            .collect();
        if indexes.is_empty() {
            self.toast(ToastKind::Success, t!("grid.refreshed").to_string(), cx);
            return;
        }
        // Remember which nodes the click asked for, so the toast that confirms the refresh waits
        // until the last one has actually landed.
        self.refresh_tree_feedback = indexes.clone();
        self.refresh_tree_failed = false;
        self.toast(ToastKind::Info, t!("grid.refreshing").to_string(), cx);
        for index in indexes {
            self.load_databases(index, cx);
        }
        cx.notify();
    }

    /// Report one connection of an explicit tree refresh as finished.
    pub(super) fn finish_tree_refresh(
        &mut self,
        index: usize,
        ok: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(position) = self
            .refresh_tree_feedback
            .iter()
            .position(|pending| *pending == index)
        else {
            return;
        };
        self.refresh_tree_feedback.remove(position);
        self.refresh_tree_failed |= !ok;
        if self.refresh_tree_feedback.is_empty() {
            let (kind, message) = if self.refresh_tree_failed {
                (ToastKind::Error, t!("grid.refresh_failed").to_string())
            } else {
                (ToastKind::Success, t!("grid.refreshed").to_string())
            };
            self.refresh_tree_failed = false;
            self.toast(kind, message, cx);
        }
    }

    /// Re-fetch one database row's connection list with refresh feedback (the tree's 刷新).
    pub(super) fn refresh_databases(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if self.refresh_tree_feedback.is_empty() {
            self.refresh_tree_failed = false;
        }
        if !self.refresh_tree_feedback.contains(&index) {
            self.refresh_tree_feedback.push(index);
        }
        self.toast(ToastKind::Info, t!("grid.refreshing").to_string(), cx);
        self.load_databases(index, cx);
    }

    /// Open the engine picker for a New Connection that will be filed under `id`.
    pub(super) fn new_connection_in_group(
        &mut self,
        id: u64,
        position: Point<Pixels>,
        cx: &mut Context<'_, Self>,
    ) {
        self.context_menu = None;
        self.move_submenu = None;
        self.connect_menu_group = Some(id);
        *self.connect_menu_anchor.borrow_mut() = position;
        self.connect_menu_open = true;
        cx.notify();
    }

    /// File a freshly created connection into the tree: under `group` when given, otherwise at the
    /// end of the root level.
    pub(super) fn add_connection_to_tree(&mut self, profile_id: String, group: Option<u64>) {
        let entry = GroupEntry::Connection(profile_id);
        match group.and_then(|id| find_group_mut(&mut self.tree_layout, id)) {
            Some(group) => {
                group.children.push(entry);
                group.expanded = true;
            }
            None => self.tree_layout.push(entry),
        }
    }

    /// Move a connection into `group_id` (appended as that group's last child).
    pub(super) fn move_connection_to_group(
        &mut self,
        index: usize,
        group_id: u64,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(id) = self
            .connections
            .get(index)
            .map(|node| node.profile.id.clone())
        else {
            return;
        };
        let payload = TreeDragPayload::Connection(id);
        if let Some(entry) = remove_entry(&mut self.tree_layout, &payload) {
            if let Some(group) = find_group_mut(&mut self.tree_layout, group_id) {
                group.children.push(entry);
                group.expanded = true;
            }
            self.persist_tree();
        }
        cx.notify();
    }

    /// Take a connection out of its group, placing it at the end of the root level. Connections
    /// that are not in a group are left untouched.
    pub(super) fn exclude_connection_from_group(
        &mut self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(id) = self
            .connections
            .get(index)
            .map(|node| node.profile.id.clone())
        else {
            return;
        };
        if group_of_connection(&self.tree_layout, &id).is_none() {
            return;
        }
        if let Some(entry) = remove_entry(&mut self.tree_layout, &TreeDragPayload::Connection(id)) {
            self.tree_layout.push(entry);
            self.persist_tree();
        }
        cx.notify();
    }

    /// Apply a drag-and-drop move over the tree. `payloads` is one row or the whole multi-selection;
    /// every dragged entry lands at the same spot, keeping their relative order.
    pub(super) fn drop_tree_entries(
        &mut self,
        payloads: Vec<TreeDragPayload>,
        drop: TreeDrop,
        cx: &mut Context<'_, Self>,
    ) {
        if payloads.is_empty() {
            return;
        }

        // Which groups are being dragged, and every connection carried inside them.
        let selected_groups: Vec<u64> = payloads
            .iter()
            .filter_map(|payload| match payload {
                TreeDragPayload::Group(id) => Some(*id),
                TreeDragPayload::Connection(_) => None,
            })
            .collect();
        let mut inside_selected_group: BTreeSet<String> = BTreeSet::new();
        for group_id in &selected_groups {
            collect_subtree_connection_ids(
                &self.tree_layout,
                *group_id,
                &mut inside_selected_group,
            );
        }

        // Refuse to drop a group into itself or one of its own descendants, or onto an entry the
        // dragged groups already carry (it would vanish with them).
        match &drop {
            TreeDrop::Into(target) => {
                if selected_groups
                    .iter()
                    .any(|id| id == target || group_in_subtree(&self.tree_layout, *id, *target))
                {
                    return;
                }
            }
            TreeDrop::Before(target) | TreeDrop::After(target) => {
                if payloads.iter().any(|payload| payload == target) {
                    return;
                }
                let carried = match target {
                    TreeDragPayload::Group(target_id) => selected_groups.iter().any(|id| {
                        id == target_id || group_in_subtree(&self.tree_layout, *id, *target_id)
                    }),
                    TreeDragPayload::Connection(id) => inside_selected_group.contains(id),
                };
                if carried {
                    return;
                }
            }
            TreeDrop::RootEnd => {}
        }

        // Drop the payloads that are already carried by another dragged group, so a group and a
        // connection inside it are not detached and re-inserted separately.
        let mut roots: Vec<TreeDragPayload> = Vec::new();
        for payload in &payloads {
            let redundant = match payload {
                TreeDragPayload::Group(id) => selected_groups
                    .iter()
                    .any(|other| other != id && group_in_subtree(&self.tree_layout, *other, *id)),
                TreeDragPayload::Connection(id) => inside_selected_group.contains(id),
            };
            if !redundant {
                roots.push(payload.clone());
            }
        }

        let mut removed: Vec<GroupEntry> = Vec::new();
        for payload in &roots {
            if let Some(entry) = remove_entry(&mut self.tree_layout, payload) {
                removed.push(entry);
            }
        }
        if removed.is_empty() {
            return;
        }

        match drop {
            TreeDrop::RootEnd => self.tree_layout.extend(removed),
            TreeDrop::Into(id) => {
                if let Some(group) = find_group_mut(&mut self.tree_layout, id) {
                    group.children.extend(removed);
                    group.expanded = true;
                } else {
                    self.tree_layout.extend(removed);
                }
            }
            TreeDrop::Before(target) => {
                if !insert_relative_all(&mut self.tree_layout, &target, &removed, false) {
                    self.tree_layout.extend(removed);
                }
            }
            TreeDrop::After(target) => {
                if !insert_relative_all(&mut self.tree_layout, &target, &removed, true) {
                    self.tree_layout.extend(removed);
                }
            }
        }
        self.persist_tree();
        cx.notify();
    }

    // ----- Drag and drop -------------------------------------------------------------------

    /// Note one draggable row's window-space rectangle (from its `on_children_prepainted`).
    pub(super) fn note_tree_row_rect(&mut self, key: String, rect: Bounds<Pixels>) {
        self.tree_row_rects.insert(key, rect);
    }

    /// Drop the row rectangles from the previous frame before the tree republishes them.
    pub(super) fn clear_tree_row_rects(&mut self) {
        self.tree_row_rects.clear();
    }

    /// Record a left-press on a draggable row, so it can start a drag once the pointer moves. When
    /// the pressed row is part of the tree's multi-selection, the whole selection is dragged.
    pub(super) fn tree_press_start(&mut self, payload: TreeDragPayload, position: Point<Pixels>) {
        let payloads = self.tree_drag_payloads_for(&payload);
        self.tree_press = Some((payloads, position));
    }

    /// Update the in-progress drag as the pointer moves.
    pub(super) fn tree_drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.tree_press = None;
            if self.tree_drag.take().is_some() {
                cx.notify();
            }
            return;
        }
        if self.tree_drag.is_none() {
            if let Some((payloads, start)) = self.tree_press.clone() {
                let dx = f32::from(event.position.x) - f32::from(start.x);
                let dy = f32::from(event.position.y) - f32::from(start.y);
                if dx.hypot(dy) > 4.0 {
                    self.tree_drag = Some(TreeDrag {
                        payloads,
                        position: event.position,
                    });
                    cx.notify();
                }
            }
        } else if let Some(drag) = self.tree_drag.as_mut() {
            drag.position = event.position;
            // The drop indicator is drawn by `AppView`, so a whole-app frame redraws it in step
            // with the pointer (the cached tree pane is not notified for every move).
            cx.notify();
        }
    }

    /// Finish a drag on mouse-up, applying the drop if one was in progress.
    pub(super) fn tree_drag_end(&mut self, cx: &mut Context<'_, Self>) {
        self.tree_press = None;
        let Some(drag) = self.tree_drag.take() else {
            return;
        };
        if let Some(drop) = self.tree_drop_at(drag.position) {
            self.drop_tree_entries(drag.payloads, drop, cx);
        }
        cx.notify();
    }

    /// The drop a pointer position maps to over the tree.
    fn tree_drop_at(&self, position: Point<Pixels>) -> Option<TreeDrop> {
        let Some((key, rect)) = self.tree_row_hit(position) else {
            return Some(TreeDrop::RootEnd);
        };
        if let Some(rest) = key.strip_prefix("group-")
            && let Ok(id) = rest.parse::<u64>()
        {
            let third = rect.size.height / 3.0;
            if position.y < rect.top() + third {
                return Some(TreeDrop::Before(TreeDragPayload::Group(id)));
            }
            if position.y > rect.bottom() - third {
                return Some(TreeDrop::After(TreeDragPayload::Group(id)));
            }
            return Some(TreeDrop::Into(id));
        }
        if let Some(rest) = key.strip_prefix("conn-")
            && let Ok(index) = rest.parse::<usize>()
            && let Some(id) = self
                .connections
                .get(index)
                .map(|node| node.profile.id.clone())
        {
            return Some(if position.y < rect.center().y {
                TreeDrop::Before(TreeDragPayload::Connection(id))
            } else {
                TreeDrop::After(TreeDragPayload::Connection(id))
            });
        }
        Some(TreeDrop::RootEnd)
    }

    fn tree_row_hit(&self, position: Point<Pixels>) -> Option<(String, Bounds<Pixels>)> {
        self.tree_row_rects
            .iter()
            .find(|(_, rect)| rect.contains(&position))
            .map(|(key, rect)| (key.clone(), *rect))
    }

    /// The insertion indicator (a line or a group highlight) for the active drag, if any.
    pub(super) fn tree_drop_indicator(&self) -> Option<TreeDropHint> {
        let drag = self.tree_drag.as_ref()?;
        let position = drag.position;
        let Some((key, rect)) = self.tree_row_hit(position) else {
            // Not over a draggable row: the drop is "at the end of the root level", so draw the
            // insertion line at the pointer (clamped below the last row) so it tracks the cursor.
            let bottom = self
                .tree_row_rects
                .values()
                .map(|rect| rect.bottom())
                .max()?;
            let left = self.tree_row_rects.values().map(|rect| rect.left()).min()?;
            let right = self
                .tree_row_rects
                .values()
                .map(|rect| rect.right())
                .max()?;
            let y = position.y.max(bottom + px(1.0));
            return Some(TreeDropHint::Line(Bounds::new(
                Point::new(left, y),
                size(right - left, px(2.0)),
            )));
        };
        if key.starts_with("group-") {
            let third = rect.size.height / 3.0;
            if position.y >= rect.top() + third && position.y <= rect.bottom() - third {
                return Some(TreeDropHint::Highlight(rect));
            }
        }
        let y = if position.y < rect.center().y {
            rect.top() - px(1.0)
        } else {
            rect.bottom() - px(1.0)
        };
        Some(TreeDropHint::Line(Bounds::new(
            Point::new(rect.left(), y),
            size(rect.size.width, px(2.0)),
        )))
    }
}

/// The visual hint drawn while dragging a tree row.
pub(super) enum TreeDropHint {
    /// A 2px insertion line at this window-space rectangle.
    Line(Bounds<Pixels>),
    /// A highlight over a group the entry would drop into.
    Highlight(Bounds<Pixels>),
}

/// Remove a group from the tree, splicing its children into its parent at its position. Returns the
/// removed (now empty of the group shell) group metadata for callers that care.
fn remove_group(entries: &mut Vec<GroupEntry>, id: u64) -> Option<ConnectionGroup> {
    if let Some(index) = entries
        .iter()
        .position(|entry| matches!(entry, GroupEntry::Group(group) if group.id == id))
        && let GroupEntry::Group(mut group) = entries.remove(index)
    {
        let children = std::mem::take(&mut group.children);
        for (offset, child) in children.into_iter().enumerate() {
            entries.insert(index + offset, child);
        }
        return Some(group);
    }
    for entry in entries.iter_mut() {
        if let GroupEntry::Group(group) = entry
            && let Some(removed) = remove_group(&mut group.children, id)
        {
            return Some(removed);
        }
    }
    None
}

/// Whether group `candidate` is `ancestor` or nested anywhere under it.
fn group_in_subtree(entries: &[GroupEntry], ancestor: u64, candidate: u64) -> bool {
    let Some(group) = find_group(entries, ancestor) else {
        return false;
    };
    subtree_contains(&group.children, candidate)
}

fn subtree_contains(entries: &[GroupEntry], id: u64) -> bool {
    for entry in entries {
        if let GroupEntry::Group(group) = entry
            && (group.id == id || subtree_contains(&group.children, id))
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: u64, name: &str, children: Vec<GroupEntry>) -> GroupEntry {
        GroupEntry::Group(ConnectionGroup {
            id,
            name: name.to_string(),
            expanded: true,
            children,
        })
    }

    fn conn(id: &str) -> GroupEntry {
        GroupEntry::Connection(id.to_string())
    }

    #[test]
    fn collapse_all_closes_every_group_at_every_depth() {
        let mut layout = vec![
            conn("a"),
            group(
                1,
                "outer",
                vec![conn("b"), group(2, "inner", vec![conn("c")])],
            ),
        ];
        collapse_all(&mut layout);
        assert!(!find_group(&layout, 1).unwrap().expanded);
        assert!(!find_group(&layout, 2).unwrap().expanded);
    }

    #[test]
    fn deleting_a_group_keeps_its_children() {
        let mut layout = vec![
            conn("a"),
            group(1, "g", vec![conn("b"), group(2, "n", vec![conn("c")])]),
        ];
        remove_group(&mut layout, 1);
        // The removed group's children are spliced in at its position, so no connection is lost.
        assert_eq!(layout.len(), 3);
        assert!(matches!(&layout[0], GroupEntry::Connection(id) if id == "a"));
        assert!(matches!(&layout[1], GroupEntry::Connection(id) if id == "b"));
        assert!(matches!(&layout[2], GroupEntry::Group(g) if g.id == 2));
    }

    #[test]
    fn a_new_connection_is_filed_under_its_group() {
        let mut layout = vec![group(1, "g", vec![]), conn("a")];
        let entry = GroupEntry::Connection("new".to_string());
        find_group_mut(&mut layout, 1).unwrap().children.push(entry);
        assert_eq!(group_of_connection(&layout, "new"), Some(1));
        assert_eq!(group_of_connection(&layout, "a"), None);
    }

    #[test]
    fn a_group_cannot_be_dropped_into_its_own_descendant() {
        let layout = vec![group(1, "g", vec![group(2, "n", vec![])])];
        // A descendant is rejected; a group is handled as "itself" separately (see drop_tree_entries).
        assert!(group_in_subtree(&layout, 1, 2));
        assert!(!group_in_subtree(&layout, 1, 1));
        assert!(!group_in_subtree(&layout, 2, 1));
    }

    #[test]
    fn next_group_id_is_one_past_the_max() {
        let layout = vec![group(3, "a", vec![group(7, "b", vec![])])];
        assert_eq!(next_group_id(&layout), 8);
        assert_eq!(next_group_id(&[]), 1);
    }

    #[test]
    fn insert_relative_places_a_group_before_its_sibling() {
        let mut layout = vec![group(1, "a", vec![]), group(2, "b", vec![])];
        let moving = [group(3, "c", vec![])];
        assert!(insert_relative_all(
            &mut layout,
            &TreeDragPayload::Group(2),
            &moving,
            false
        ));
        assert!(matches!(&layout[1], GroupEntry::Group(g) if g.id == 3));
        assert!(matches!(&layout[2], GroupEntry::Group(g) if g.id == 2));
    }

    #[test]
    fn insert_relative_all_keeps_the_order_of_several_entries() {
        let mut layout = vec![group(1, "a", vec![])];
        let moving = [conn("x"), conn("y")];
        assert!(insert_relative_all(
            &mut layout,
            &TreeDragPayload::Group(1),
            &moving,
            true
        ));
        assert!(matches!(&layout[1], GroupEntry::Connection(id) if id == "x"));
        assert!(matches!(&layout[2], GroupEntry::Connection(id) if id == "y"));
    }

    #[test]
    fn reconcile_drops_removed_and_appends_new_connections() {
        fn node(id: &str) -> ConnectionNode {
            ConnectionNode {
                profile: rustgrid_core::ConnectionProfile {
                    id: id.to_string(),
                    name: id.to_string(),
                    driver: rustgrid_core::DriverId::new("mysql"),
                    host: String::new(),
                    port: 0,
                    username: String::new(),
                    database: None,
                    options: Default::default(),
                    settings: Default::default(),
                },
                password: None,
                password_saved: false,
                status: ConnectionStatus::Disconnected,
                databases: Loadable::Idle,
                expanded: false,
            }
        }
        // "gone" was removed; "new" was never in the layout; "kept" is nested but preserved.
        let mut layout = vec![
            conn("gone"),
            group(1, "g", vec![conn("kept")]),
            conn("kept"),
        ];
        let connections = [node("kept"), node("new")];
        reconcile_tree(&mut layout, &connections);
        assert_eq!(group_of_connection(&layout, "kept"), Some(1));
        assert!(find_group(&layout, 1).is_some());
        assert!(
            layout
                .iter()
                .any(|e| matches!(e, GroupEntry::Connection(id) if id == "new"))
        );
        assert!(
            !layout
                .iter()
                .any(|e| matches!(e, GroupEntry::Connection(id) if id == "gone"))
        );
    }
}
