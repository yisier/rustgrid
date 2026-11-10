//! The grid's 字段筛选 popup: an anchored, non-modal panel under the toolbar item that shows, hides
//! and re-orders the table's fields.
//!
//! Edits land on a draft layout held by [`GridView::field_filter_draft`]; 确定 installs it on the
//! grid, 取消/an Escape/any click outside discards it. Applying projects `GridState::columns` (and
//! their widths) out of the table's natural column order — the rows keep the engine's order and are
//! read through `GridState::source_col` — so it re-projects metadata only: no refetch, no row copy.
//! The result is remembered per connection/database/table, so a table reopens with the field set
//! the user last chose.

use super::*;

/// The popup's search field. Built with the grid (not on the first render) because the grid's
/// `Context` is what the change callback updates.
pub(super) fn make_field_filter_search_input(
    theme: Theme,
    weak: WeakEntity<GridView>,
    cx: &mut Context<'_, GridView>,
) -> Entity<TextInput> {
    let options = TextInputOptions {
        placeholder: t!("grid.field_filter_search").to_string().into(),
        icon: Some("icons/search.svg"),
        clearable: true,
        text_size: Some(12.0),
        size: Some(gpui_kit::component::Size::XSmall),
        ..Default::default()
    };
    let change = weak.clone();
    let cancel = weak;
    cx.new(move |cx| {
        TextInput::new(theme, "", options, cx)
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |grid, cx| {
                    grid.field_filter_search = text.to_string();
                    cx.notify();
                });
            }))
            .on_cancel(Rc::new(move |window, cx| {
                // Escape leaves the search field and closes the popup, handing the keyboard back to
                // the grid.
                let _ = cancel.update(cx, |grid, cx| {
                    grid.close_field_filter(cx);
                    window.focus(&grid.focus, cx);
                });
            }))
    })
}

/// Whether a field's name or comment matches the popup's search text.
fn field_matches(name: &str, comment: &str, needle: &str) -> bool {
    name.to_lowercase().contains(needle) || comment.to_lowercase().contains(needle)
}

impl GridView {
    /// The item's colour: the brand accent once fields are hidden or re-ordered, and the plain
    /// chrome text colour while the grid shows the table's natural field set — so the single icon
    /// is colourless by default and only picks up a colour once a layout is set.
    pub(super) fn field_filter_color(&self) -> u32 {
        if self.state.field_layout_configured() {
            self.theme.brand
        } else {
            self.theme.text
        }
    }

    /// Open or close the 字段筛选 popup.
    pub(super) fn toggle_field_filter(&mut self, cx: &mut Context<'_, Self>) {
        if self.field_filter_open {
            self.close_field_filter(cx);
            return;
        }
        if self.state.sql.is_some() || self.state.all_columns.is_empty() {
            return;
        }
        // The popup works on a draft copy: nothing reaches the grid until 确定. The draft shows the
        // effective display order (frozen fields pinned to the front), which is what the grid and
        // the header currently show.
        self.field_filter_draft = self.state.field_layout.clone();
        self.field_filter_draft.order = self.state.effective_field_order();
        self.field_filter_search.clear();
        let input = self.field_filter_search_input.clone();
        input.update(cx, |input, cx| input.set_text(String::new(), cx));
        self.field_filter_drag = None;
        self.field_filter_open = true;
        self.field_filter_focus_pending = true;
        cx.notify();
    }

    /// Re-translate the popup's search placeholder after a language change. The field is built
    /// once with the grid (like the shared search boxes in `AppView`), so the new text has to be
    /// pushed into it; otherwise reopening the popup after switching language still shows the
    /// previous one.
    pub(super) fn sync_search_placeholder(&mut self, cx: &mut Context<'_, Self>) {
        let input = self.field_filter_search_input.clone();
        input.update(cx, |input, cx| {
            input.set_placeholder(t!("grid.field_filter_search").to_string(), cx);
        });
    }

    /// Dismiss the popup, discarding its draft (the applied layout keeps whatever 确定 last set).
    pub(super) fn close_field_filter(&mut self, cx: &mut Context<'_, Self>) {
        self.field_filter_open = false;
        self.field_filter_drag = None;
        self.field_filter_focus_pending = false;
        cx.notify();
    }

    /// 确定: install the popup's draft layout on the grid, remember it per table and close.
    ///
    /// The draft lists the frozen fields first (the order the grid shows). [`GridState::field_layout`]
    /// must keep the unfrozen order, so the frozen names are taken out and put back at the positions
    /// they held before they were frozen: that is what makes 取消冻结 restore the original field
    /// order. A non-frozen field's new position is kept exactly where the user dropped it.
    pub(super) fn field_filter_apply(&mut self, cx: &mut Context<'_, Self>) {
        let layout = self.state.field_layout.with_display_order(
            &self.field_filter_draft.order,
            self.field_filter_draft.hidden.clone(),
            &self.state.frozen_columns,
            &self.state.all_columns,
        );
        self.state.field_layout = layout;
        self.state.reproject_fields();
        self.field_filter_draft = self.state.field_layout.clone();
        self.field_filter_draft.order = self.state.effective_field_order();
        self.persist_field_layout(cx);
        self.close_field_filter(cx);
    }

    /// How many of the fields the draft shows.
    fn field_filter_visible_count(&self) -> usize {
        self.field_filter_draft
            .order
            .iter()
            .filter(|name| !self.field_filter_draft.hidden.contains(*name))
            .count()
    }

    /// Hide or show one field in the draft. The last visible field can never be hidden.
    pub(super) fn field_filter_toggle(&mut self, name: String, cx: &mut Context<'_, Self>) {
        let hide = !self.field_filter_draft.hidden.contains(&name);
        if hide && self.field_filter_visible_count() <= 1 {
            return;
        }
        self.field_filter_draft.set_hidden(&name, hide);
        cx.notify();
    }

    /// 重置: put the draft back the way the engine returns the fields — the table's own order with
    /// every field shown — while keeping the frozen fields pinned to the front, exactly as the grid
    /// displays them.
    pub(super) fn field_filter_reset(&mut self, cx: &mut Context<'_, Self>) {
        let mut order: Vec<String> = self
            .state
            .frozen_columns
            .iter()
            .filter(|name| {
                self.state
                    .all_columns
                    .iter()
                    .any(|column| &column.name == *name)
            })
            .cloned()
            .collect();
        for column in &self.state.all_columns {
            if !order.contains(&column.name) {
                order.push(column.name.clone());
            }
        }
        self.field_filter_draft.order = order;
        self.field_filter_draft.hidden.clear();
        cx.notify();
    }

    /// 全选: show every field in the draft (keeping the current order).
    pub(super) fn field_filter_show_all(&mut self, cx: &mut Context<'_, Self>) {
        if self.field_filter_draft.hidden.is_empty() {
            return;
        }
        self.field_filter_draft.hidden.clear();
        cx.notify();
    }

    /// 反选: flip every field's visibility in the draft, never hiding the last visible one.
    pub(super) fn field_filter_invert(&mut self, cx: &mut Context<'_, Self>) {
        self.field_filter_draft.invert_visibility();
        cx.notify();
    }

    /// The frozen fields the draft still shows, in the order they are pinned at the top.
    fn field_filter_frozen_count(&self) -> usize {
        self.field_filter_draft
            .order
            .iter()
            .filter(|name| {
                !self.field_filter_draft.hidden.contains(*name)
                    && self
                        .state
                        .frozen_columns
                        .iter()
                        .any(|frozen| frozen == *name)
            })
            .count()
    }

    /// Start dragging a field row's handle. Frozen fields are pinned to the front, so their rows
    /// cannot be dragged; the lock beside their name says why.
    pub(super) fn field_filter_press(
        &mut self,
        index: usize,
        position_y: f32,
        cx: &mut Context<'_, Self>,
    ) {
        let frozen = self
            .field_filter_draft
            .order
            .get(index)
            .is_some_and(|name| self.state.is_frozen_column(name));
        if frozen {
            return;
        }
        self.field_filter_drag = Some(FieldFilterDrag {
            from: index,
            current: index,
            start_y: position_y,
        });
        cx.notify();
    }

    /// Reorder the field being dragged as the pointer crosses row boundaries — in the draft only;
    /// the grid picks the new order up when 确定 is pressed. The list has a fixed row height, so the
    /// target position is where the pointer has travelled from the press.
    pub(super) fn field_filter_drag_move(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(drag) = self.field_filter_drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.field_filter_drag = None;
            cx.notify();
            return;
        }
        let len = self.field_filter_draft.order.len();
        if len == 0 {
            return;
        }
        let steps = ((f32::from(event.position.y) - drag.start_y) / FIELD_FILTER_ROW_HEIGHT).round()
            as isize;
        // The frozen fields are pinned to the top, so a normal field can never be dropped into
        // (or above) their block.
        let floor = self.field_filter_frozen_count();
        if floor >= len {
            return;
        }
        let target = (drag.from as isize + steps).clamp(floor as isize, len as isize - 1) as usize;
        if target == drag.current {
            return;
        }
        let order = self.field_filter_draft.order.clone();
        self.field_filter_draft
            .move_field(&order, drag.current, target);
        self.field_filter_drag = Some(FieldFilterDrag {
            current: target,
            ..drag
        });
        cx.notify();
    }

    /// Finish a drag.
    pub(super) fn field_filter_drag_end(&mut self, cx: &mut Context<'_, Self>) {
        if self.field_filter_drag.take().is_some() {
            cx.notify();
        }
    }

    /// The applied layout in the persisted shape, for callers that store it through `AppView`.
    pub(super) fn field_layout_snapshot(&self) -> FieldLayout {
        FieldLayout {
            order: self.state.field_layout.order.clone(),
            hidden: self.state.field_layout.hidden.iter().cloned().collect(),
        }
    }

    /// Write the applied layout to settings so the next open of this table restores it.
    pub(super) fn persist_field_layout(&mut self, cx: &mut Context<'_, Self>) {
        let connection_id = self.state.connection_id.clone();
        if connection_id.is_empty() {
            return;
        }
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let layout = self.field_layout_snapshot();
        app.update(cx, |app, cx| {
            app.set_field_layout(&connection_id, &database, &table, layout);
            cx.notify();
        });
    }

    /// The full-pane backdrop behind the popup: one click anywhere outside dismisses it.
    pub(super) fn render_field_filter_backdrop(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        deferred(
            div()
                .id("field-filter-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.close_field_filter(cx);
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                    this.field_filter_drag_move(event, cx);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                        this.field_filter_drag_end(cx);
                    }),
                ),
        )
        .with_priority(FIELD_FILTER_BACKDROP_PRIORITY)
        .into_any_element()
    }

    /// The 字段筛选 panel, anchored under its toolbar item.
    pub(super) fn render_field_filter_popup(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let pane = *self.grid_pane_bounds.borrow();
        let anchor = *self.field_filter_anchor.borrow();
        let max_left = (f32::from(pane.size.width) - FIELD_FILTER_POPUP_WIDTH - 8.0).max(0.0);
        let left = (f32::from(anchor.x) - f32::from(pane.left())).clamp(0.0, max_left);
        let top = GRID_TOOLBAR_HEIGHT + 2.0;
        // Keep the whole panel inside the pane — the header, search box and footer take roughly
        // this much — while letting a tall pane show as many fields as fit.
        let list_max_height = (f32::from(pane.size.height) - top - FIELD_FILTER_CHROME_HEIGHT)
            .clamp(140.0, FIELD_FILTER_LIST_MAX_HEIGHT);

        let columns = self.state.all_columns.clone();
        let order = self.field_filter_draft.order.clone();
        let hidden = self.field_filter_draft.hidden.clone();
        let frozen_columns = self.state.frozen_columns.clone();
        let needle = self.field_filter_search.trim().to_lowercase();
        let dragged = self.field_filter_drag.map(|drag| drag.current);
        let weak = self.self_weak.clone();
        let total = order.len();
        let visible_total = self.field_filter_visible_count();

        let mut list = div()
            .id("field-filter-list")
            .flex()
            .flex_col()
            .flex_none()
            // Keep the whole panel inside the pane: the list shrinks when the pane is short, and
            // scrolls instead.
            .max_h(px(list_max_height))
            .overflow_y_scroll();
        let mut rows = 0usize;
        for (index, name) in order.iter().enumerate() {
            let comment = columns
                .iter()
                .find(|column| &column.name == name)
                .map(|column| single_line(&column.comment))
                .unwrap_or_default();
            if !needle.is_empty() && !field_matches(name, &comment, &needle) {
                continue;
            }
            rows += 1;
            let checked = !hidden.contains(name);
            let row_name = name.clone();
            let toggle_name = name.clone();
            let toggle_weak = weak.clone();
            let row = div()
                .id(SharedString::from(format!("field-filter-row-{name}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(FIELD_FILTER_ROW_HEIGHT))
                .px_2()
                .flex_none()
                .when(dragged == Some(index), move |style| {
                    style.bg(rgb(theme.tree_selected_bg))
                })
                .child(
                    // The drag handle is the only place a press starts a reorder, so clicking the
                    // row (or its check box) still toggles the field's visibility.
                    div()
                        .id(SharedString::from(format!("field-filter-handle-{name}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(14.0))
                        .h_full()
                        .flex_none()
                        .cursor_grab()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                this.field_filter_press(index, f32::from(event.position.y), cx);
                            }),
                        )
                        .child(
                            svg()
                                .path("icons/drag_handle.svg")
                                .w(px(14.0))
                                .h(px(14.0))
                                .flex_none()
                                .text_color(rgb(theme.text_muted)),
                        ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("field-filter-toggle-{name}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .flex_1()
                        .min_w(px(0.0))
                        .h_full()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                        .on_click(move |_event, _window, cx| {
                            let _ = toggle_weak.update(cx, |grid, cx| {
                                grid.field_filter_toggle(toggle_name.clone(), cx);
                            });
                        })
                        .child(checkbox_box(checked, theme))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.0))
                                .text_color(rgb(theme.text))
                                .child(row_name.clone()),
                        )
                        .when(frozen_columns.contains(name), |row| {
                            row.child(
                                svg()
                                    .path("icons/lock.svg")
                                    .w(px(11.0))
                                    .h(px(11.0))
                                    .flex_none()
                                    .text_color(rgb(theme.primary)),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(11.0))
                                .text_color(rgb(theme.text_muted))
                                .child(comment.clone()),
                        ),
                );
            list = list.child(row);
        }
        if rows == 0 {
            list = list.child(
                div()
                    .h(px(FIELD_FILTER_ROW_HEIGHT))
                    .px_2()
                    .flex_none()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("grid.field_filter_empty").to_string()),
            );
        }

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .h(px(24.0))
            .flex_none()
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme.text))
                    .child(t!("grid.field_filter").to_string()),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(format!("{visible_total}/{total}")),
            );

        let footer = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .flex_none()
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .child(self.field_filter_action(
                        "field-filter-reset",
                        t!("grid.field_filter_reset").to_string(),
                        cx.listener(|this, _event, _window, cx| this.field_filter_reset(cx)),
                    ))
                    .child(self.field_filter_action(
                        "field-filter-select-all",
                        t!("grid.field_filter_select_all").to_string(),
                        cx.listener(|this, _event, _window, cx| this.field_filter_show_all(cx)),
                    ))
                    .child(self.field_filter_action(
                        "field-filter-invert",
                        t!("grid.field_filter_invert").to_string(),
                        cx.listener(|this, _event, _window, cx| this.field_filter_invert(cx)),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .flex_1()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .child(ui::popup_button(
                        "field-filter-cancel",
                        t!("form.cancel").to_string(),
                        false,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.close_field_filter(cx)),
                    ))
                    .child(ui::popup_button(
                        "field-filter-ok",
                        t!("form.ok").to_string(),
                        true,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.field_filter_apply(cx)),
                    )),
            );

        let panel = ui::popup_panel(theme)
            .id("field-filter-popup")
            .left(px(left))
            .top(px(top))
            .w(px(FIELD_FILTER_POPUP_WIDTH))
            .flex()
            .flex_col()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.field_filter_drag_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.field_filter_drag_end(cx);
                }),
            )
            .child(header)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .flex_none()
                    .child(self.field_filter_search_input.clone()),
            )
            .child(list)
            .child(footer);

        deferred(panel)
            .with_priority(FIELD_FILTER_POPUP_PRIORITY)
            .into_any_element()
    }

    /// One footer action of the 字段筛选 popup.
    fn field_filter_action(
        &self,
        id: &'static str,
        label: String,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .h(px(20.0))
            .px_1()
            .rounded_sm()
            .flex_none()
            .text_size(px(11.0))
            .text_color(rgb(theme.text))
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(on_click)
            .child(label)
    }
}
