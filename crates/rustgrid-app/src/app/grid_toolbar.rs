use super::grid::filter_node;
use super::*;
use gpui_kit::component::spinner::Spinner;

/// The per-row actions of the filter builder.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterAction {
    /// Remove the node (a condition row or a group).
    Remove,
    /// Add a condition inside the group that owns the row (the condition row's `+`).
    AddCondition,
    /// Add the next sibling group after the group whose boundary row owns the button.
    AddGroup,
}

/// The left rail cell of a filter row: the `并且/或者` toggle that joins the row to its previous
/// sibling, a plain indent for the first row of a group, or nothing at the root.
#[derive(Clone, Copy)]
enum FilterRail {
    None,
    Spacer,
    Toggle(FilterConjunction),
}

/// A stable string form of a filter node path, for element ids.
fn filter_path_key(path: &[usize]) -> String {
    path.iter()
        .map(|index| index.to_string())
        .collect::<Vec<_>>()
        .join("-")
}

impl GridView {
    /// The grid's running-fetch indicator: one spinner, dropped into a bar the grid already
    /// draws — the toolbar when it has one, the status bar otherwise. It is never a row of its
    /// own, so a fetch starting or ending shifts nothing under the pointer.
    pub(super) fn render_loading_spinner(&self) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id("grid-loading")
            .flex_none()
            .flex()
            .items_center()
            .tooltip(ui::text_tooltip(t!("common.loading").to_string()))
            .child(Spinner::new().color(rgb(theme.text_muted).into()))
    }

    pub(super) fn render_grid_toolbar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        // A key's detail grid is a content editor (hash fields, list items, ...), not a table:
        // WHERE/sort/field layout do not apply.
        let key_value = self.state.key_status.is_some();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .h(px(28.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.grid_tool_button(
                "grid-filter",
                "icons/filter.svg",
                t!("grid.filter").to_string(),
                theme.text,
                !key_value && self.state.sql.is_none(),
                self.state.filter_open,
                false,
                cx.listener(|this, _event, _window, cx| this.toggle_filter_panel(cx)),
            ))
            .child(self.grid_tool_button(
                "grid-sort",
                "icons/sort.svg",
                t!("grid.sort").to_string(),
                theme.text,
                !key_value && self.state.sql.is_none(),
                self.state.sort_open,
                false,
                cx.listener(|this, _event, _window, cx| this.toggle_sort_panel(cx)),
            ))
            .child(self.grid_tool_button(
                "grid-import",
                "icons/import.svg",
                t!("grid.import").to_string(),
                theme.icon_views,
                !key_value && self.state.sql.is_none(),
                false,
                false,
                cx.listener(|this, _event, _window, cx| this.open_grid_import(cx)),
            ))
            .child(self.grid_tool_button(
                "grid-export",
                "icons/export.svg",
                t!("grid.export").to_string(),
                theme.icon_views,
                self.state.sql.is_none() && !self.state.all_columns.is_empty(),
                false,
                false,
                cx.listener(|this, _event, _window, cx| this.open_grid_export(cx)),
            ))
            .child({
                // The 字段筛选 item toggles the popup; while the popup is open its click-away
                // backdrop covers this button too, so a second press dismisses rather than
                // re-opening. The wrapper only reports the button's position, which anchors the
                // popup directly under it. One icon serves both states: it is colourless while the
                // grid shows the table's natural field set and gains the brand colour once a field
                // is hidden or re-ordered (i.e. once 确定 applied a layout).
                let anchor = self.field_filter_anchor.clone();
                let configured = self.state.field_layout_configured();
                div()
                    .flex()
                    .flex_none()
                    .on_children_prepainted(move |bounds, _window, _cx| {
                        if let Some(first) = bounds.first() {
                            *anchor.borrow_mut() = Point::new(first.left(), first.top());
                        }
                    })
                    .child(self.grid_tool_button(
                        "grid-field-filter",
                        "icons/field_filter.svg",
                        t!("grid.fields").to_string(),
                        self.field_filter_color(),
                        !key_value
                            && self.state.sql.is_none()
                            && !self.state.all_columns.is_empty(),
                        configured && self.field_filter_open,
                        false,
                        cx.listener(|this, _event, _window, cx| this.toggle_field_filter(cx)),
                    ))
            })
            // The spinner sits at the toolbar's far end while a fetch runs; the toolbar has a
            // fixed 28px height, so the table below never moves for it.
            .when(self.state.loading, |bar| {
                bar.child(div().flex_1())
                    .child(self.render_loading_spinner())
            })
    }

    /// Open the Import Wizard scoped to this grid's database, defaulting a single source table to
    /// the table the grid is showing. Table grids only (SQL result grids have no import target).
    fn open_grid_import(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() || self.state.key_status.is_some() {
            return;
        }
        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let Some(app) = self.app.upgrade() else {
            return;
        };
        app.update(cx, |app, cx| {
            let Some((connection_index, database_index)) = app.table_scope(&connection, &database)
            else {
                return;
            };
            app.open_import_wizard(connection_index, database_index, Some(table.clone()), cx);
        });
    }

    /// Open the 导出 confirmation with this grid's table pre-selected, offering the whole table or
    /// the rows currently on screen (which already carry the applied filter and sort). Table grids
    /// only.
    fn open_grid_export(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() || self.state.all_columns.is_empty() {
            return;
        }
        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let table = self.state.table.clone();
        let page = ExportSnapshot {
            name: table.clone(),
            columns: self.state.all_columns.clone(),
            rows: self.state.rows.clone(),
        };
        let Some(app) = self.app.upgrade() else {
            return;
        };
        app.update(cx, move |app, cx| {
            let Some((connection_index, database_index)) = app.table_scope(&connection, &database)
            else {
                return;
            };
            app.open_table_export_choice(connection_index, database_index, table, page, cx);
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn grid_tool_button(
        &self,
        id: &'static str,
        icon: &'static str,
        label: String,
        color: u32,
        enabled: bool,
        selected: bool,
        has_arrow: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let mut item = div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(24.0))
            .rounded_sm()
            .text_size(px(12.0))
            .text_color(rgb(color))
            .when(selected, move |style| style.bg(rgb(theme.tree_selected_bg)))
            .when(enabled, move |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .when(enabled, |this| this.on_click(on_click))
            .child(
                svg()
                    .path(icon)
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .text_color(rgb(color)),
            )
            .child(label);
        if has_arrow {
            item = item.child(
                svg()
                    .path("icons/chevron-down.svg")
                    .w(px(10.0))
                    .h(px(10.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );
        }
        item
    }

    pub(super) fn render_sort_panel(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut list = div()
            .id("sort-rule-list")
            .flex()
            .flex_col()
            .flex_none()
            .min_h(px(48.0))
            .max_h(px(150.0))
            .overflow_y_scroll();

        if self.state.sort_draft.is_empty() {
            list = list.child(
                div()
                    .id("sort-add-hint")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .h(px(24.0))
                    .w_full()
                    .flex_none()
                    .bg(rgb(theme.tree_hover_bg))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_selected_bg)))
                    .on_click(cx.listener(|this, _event, _window, cx| this.sort_add_rule(cx)))
                    .child(sort_plus_badge(theme))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!("grid.sort_hint").to_string()),
                    ),
            );
        } else {
            let draft = self.state.sort_draft.clone();
            for (index, rule) in draft.iter().enumerate() {
                list = list.child(self.render_sort_rule(index, rule, cx));
            }
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_1()
                    .h(px(22.0))
                    .flex_none()
                    .child(ui::icon_button(
                        "sort-add-row",
                        "icons/plus.svg",
                        theme.text,
                        16.0,
                        16.0,
                        true,
                        theme,
                        cx.listener(|this, _event, _window, cx| this.sort_add_rule(cx)),
                    )),
            );
        }

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .h(px(26.0))
            .flex_none()
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(self.sort_arrow_button("sort-move-up", "icons/arrow-up.svg", -1, cx))
            .child(self.sort_arrow_button("sort-move-down", "icons/arrow-down.svg", 1, cx))
            .child(div().w(px(14.0)).flex_none())
            .child(self.sort_apply_button(cx));

        div()
            .flex()
            .flex_col()
            .flex_none()
            .bg(rgb(theme.editor_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(list)
            .child(footer)
            .into_any_element()
    }

    pub(super) fn render_sort_rule(
        &mut self,
        index: usize,
        rule: &SortRule,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let selected = self.state.sort_selected == Some(index);
        let rule = rule.clone();
        let combo = self.sort_field_combo(index, cx);
        combo.update(cx, |combo, cx| combo.set_selected(rule.column.clone(), cx));
        div()
            .id(SharedString::from(format!("sort-rule-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .h(px(24.0))
            .flex_none()
            .when(selected, move |style| style.bg(rgb(theme.tree_hover_bg)))
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, _event, _window, cx| this.sort_select_rule(index, cx)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("sort-enabled-{index}")))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.sort_toggle_enabled(index, cx)
                    }))
                    .child(checkbox_box(rule.enabled, theme)),
            )
            .child(combo)
            .child(
                div()
                    .id(SharedString::from(format!("sort-direction-{index}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(20.0))
                    .w(px(56.0))
                    .flex_none()
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.sort_toggle_direction(index, cx)
                    }))
                    .child(if rule.descending {
                        t!("grid.sort_desc").to_string()
                    } else {
                        t!("grid.sort_asc").to_string()
                    }),
            )
            // The same flat outlined `−` as the filter builder's remove action: the two panels sit
            // side by side, so a bare `✕` here read as a different control.
            .child(
                div()
                    .id(SharedString::from(format!("sort-remove-{index}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(FILTER_ROW_ACTION_SIZE))
                    .h(px(FILTER_ROW_ACTION_SIZE))
                    .flex_none()
                    .rounded(px(FILTER_ROW_ACTION_RADIUS))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.dialog_bg))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.sort_remove_rule(index, cx)
                    }))
                    .child(
                        svg()
                            .path("icons/minus.svg")
                            .w(px(FILTER_ROW_ACTION_SIZE * 0.5))
                            .h(px(FILTER_ROW_ACTION_SIZE * 0.5))
                            .flex_none()
                            .text_color(rgb(theme.text_null)),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn sort_arrow_button(
        &self,
        id: &'static str,
        icon: &'static str,
        delta: isize,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        ui::icon_button(
            id,
            icon,
            theme.text,
            22.0,
            20.0,
            false,
            theme,
            cx.listener(move |this, _event, _window, cx| this.sort_move_rule(delta, cx)),
        )
    }

    pub(super) fn sort_apply_button(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id("sort-apply")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(20.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(theme.text))
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(|this, _event, _window, cx| this.sort_apply(cx)))
            .child(
                svg()
                    .path("icons/check.svg")
                    .w(px(13.0))
                    .h(px(13.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            )
            .child(t!("grid.apply").to_string())
    }

    pub(super) fn render_filter_panel(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut list = div()
            .id("filter-condition-list")
            .flex()
            .flex_col()
            .gap_1()
            .flex_none()
            .min_h(px(40.0))
            .max_h(px(260.0))
            .px_3()
            .py_1()
            .overflow_y_scroll();

        let content: AnyElement = if self.state.filter_draft.is_empty() {
            div()
                .id("filter-add-hint")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(30.0))
                .w_full()
                .flex_none()
                .rounded(px(8.0))
                .bg(rgb(theme.dialog_bg))
                .border_1()
                .border_color(rgb(theme.border))
                .cursor_pointer()
                .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.filter_add_condition(Vec::new(), cx);
                }))
                .child(sort_plus_badge(theme))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("grid.filter_hint").to_string()),
                )
                .into_any_element()
        } else {
            let draft = self.state.filter_draft.clone();
            self.render_filter_children(&draft, &mut Vec::new(), false, cx)
        };

        // The condition block fills the pane by default (responsive) and freezes to a dragged width
        // once the divider on its right edge is used. The pane width is measured from the full-width
        // row each frame, so the block tracks window resizes.
        let auto_width = if self.filter_available_width > 1.0 {
            self.filter_available_width.min(FILTER_CONTENT_WIDTH_MAX)
        } else {
            FILTER_CONTENT_WIDTH_MAX
        };
        let width = self.filter_width.unwrap_or(auto_width);

        let measure = cx.weak_entity();
        let block =
            div()
                .flex_none()
                .w(px(width))
                .on_children_prepainted(move |bounds, _window, cx| {
                    let Some(rect) = bounds.first().copied() else {
                        return;
                    };
                    let rendered = f32::from(rect.size.width);
                    let _ = measure.update(cx, |grid, _cx| grid.filter_rendered_width = rendered);
                });

        let avail = cx.weak_entity();
        let row = div()
            .flex()
            .flex_row()
            .items_stretch()
            .w_full()
            .flex_none()
            .child(block.child(content))
            .child(
                ui::hover_resize_divider("filter-content-divider", theme)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.filter_resize =
                                Some((f32::from(event.position.x), this.filter_rendered_width));
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(|this, event: &ClickEvent, _window, cx| {
                        if matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2)
                        {
                            this.filter_width = None;
                            cx.notify();
                        }
                    })),
            );
        list = list.child(
            div()
                .on_children_prepainted(move |bounds, _window, cx| {
                    let Some(rect) = bounds.first().copied() else {
                        return;
                    };
                    let width = f32::from(rect.size.width);
                    let _ = avail.update(cx, |grid, cx| {
                        if (grid.filter_available_width - width).abs() > 0.5 {
                            grid.filter_available_width = width;
                            cx.notify();
                        }
                    });
                })
                .child(row),
        );

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(26.0))
            .flex_none()
            .child(self.filter_apply_button(cx));

        div()
            .flex()
            .flex_col()
            .flex_none()
            .bg(rgb(theme.header_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(list)
            .child(footer)
            .into_any_element()
    }

    /// Drag the divider on the filter block's right edge to resize it.
    pub(super) fn filter_resize_drag(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let Some((start_x, start_width)) = self.filter_resize else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.filter_resize = None;
            cx.notify();
            return;
        }
        let delta = f32::from(event.position.x) - start_x;
        self.filter_width =
            Some((start_width + delta).clamp(FILTER_CONTENT_WIDTH_MIN, FILTER_CONTENT_WIDTH_MAX));
        cx.notify();
    }

    /// Render one level of filter nodes as a column.
    ///
    /// Inside a group (`in_group`) each condition row after the first carries its `并且/或者` toggle
    /// in a left gutter, so no separate full-width row is spent on it. A group's outgoing boundary
    /// (the toggle joining the *next* group plus the next-group actions) is rendered as its own
    /// centred row *between* the group cards, not inside either card.
    fn render_filter_children(
        &mut self,
        nodes: &[FilterNode],
        prefix: &mut Vec<usize>,
        in_group: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let mut column = div().flex().flex_col().gap_1().w_full();
        for (index, node) in nodes.iter().enumerate() {
            prefix.push(index);
            if index > 0 && !in_group && !matches!(nodes[index - 1], FilterNode::Group(_)) {
                // Only root conditions need a standalone boundary row of their own.
                column = column.child(self.render_filter_conjunction_row(
                    prefix,
                    node.conjunction(),
                    cx,
                ));
            }
            match node {
                FilterNode::Condition(_) => {
                    let rail = if in_group {
                        if index == 0 {
                            FilterRail::Spacer
                        } else {
                            FilterRail::Toggle(node.conjunction())
                        }
                    } else {
                        FilterRail::None
                    };
                    column = column.child(self.render_filter_condition_row(prefix, node, rail, cx));
                }
                FilterNode::Group(group) => {
                    column =
                        column.child(self.render_filter_group_box(prefix, &group.children, cx));
                    let next = nodes.get(index + 1).map(|next| {
                        let mut next_path = prefix.clone();
                        if let Some(last) = next_path.last_mut() {
                            *last = index + 1;
                        }
                        (next_path, next.conjunction())
                    });
                    column = column.child(self.render_filter_boundary_row(prefix, next, cx));
                }
            }
            prefix.pop();
        }
        column.into_any_element()
    }

    /// The between-groups connector: a full-width hairline with the floating `并且/或者` pill centred
    /// on it, and the `−`/`+` next-group actions revealed on hover to the pill's right. The actions
    /// sit in a fixed slot balanced on the left so the pill never shifts.
    fn render_filter_boundary_row(
        &self,
        path: &[usize],
        next: Option<(Vec<usize>, FilterConjunction)>,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let path = path.to_vec();
        let key = filter_path_key(&path);
        let (toggle_path, toggle_conjunction, toggle_enabled) = match &next {
            Some((next_path, conjunction)) => (next_path.clone(), *conjunction, true),
            None => (path.clone(), FilterConjunction::And, false),
        };
        let hover_group = SharedString::from(format!("filter-boundary-hover-{key}"));
        let control = self.filter_conjunction_control(
            SharedString::from(format!("filter-boundary-toggle-{key}")),
            &toggle_path,
            toggle_conjunction,
            ui::TogglePillStyle::boundary(theme),
            toggle_enabled,
            cx,
        );
        // The trailing connector (no next group yet) keeps its control out of the way until hovered,
        // so the panel does not end on a dangling `并且/或者`.
        let toggle: AnyElement = if toggle_enabled {
            control
        } else {
            div()
                .flex()
                .items_center()
                .opacity(0.0)
                .group_hover(hover_group.clone(), |style| style.opacity(1.0))
                .child(control)
                .into_any_element()
        };
        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .flex_none()
            .opacity(0.0)
            .group_hover(hover_group.clone(), |style| style.opacity(1.0))
            .child(self.filter_row_action(
                &path,
                FilterAction::Remove,
                FILTER_BOUNDARY_ACTION_SIZE,
                FILTER_BOUNDARY_ACTION_RADIUS,
                cx,
            ))
            .child(self.filter_row_action(
                &path,
                FilterAction::AddGroup,
                FILTER_BOUNDARY_ACTION_SIZE,
                FILTER_BOUNDARY_ACTION_RADIUS,
                cx,
            ));

        div()
            .id(SharedString::from(format!("filter-boundary-{key}")))
            .group(hover_group)
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .w_full()
            .h(px(FILTER_BOUNDARY_BAR_HEIGHT))
            .flex_none()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px((FILTER_BOUNDARY_BAR_HEIGHT - 1.0) / 2.0))
                    .h(px(1.0))
                    .bg(rgb(theme.border)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(FILTER_BOUNDARY_ACTION_SLOT)).flex_none())
                    .child(toggle)
                    .child(
                        div()
                            .w(px(FILTER_BOUNDARY_ACTION_SLOT))
                            .flex_none()
                            .child(actions),
                    ),
            )
            .into_any_element()
    }

    /// The centred `并且/或者` row shown between two siblings outside a group.
    fn render_filter_conjunction_row(
        &self,
        path: &[usize],
        conjunction: FilterConjunction,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .w_full()
            .py_1()
            .flex_none()
            .child(self.filter_conjunction_control(
                SharedString::from(format!("filter-conjunction-{}", filter_path_key(path))),
                path,
                conjunction,
                ui::TogglePillStyle::in_group(theme),
                true,
                cx,
            ))
            .into_any_element()
    }

    /// One condition row: `[rail] field op value ... [−][+]`, where the rail holds the `并且/或者`
    /// toggle joining this row to the previous one inside a group.
    fn render_filter_condition_row(
        &mut self,
        path: &[usize],
        node: &FilterNode,
        rail: FilterRail,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let path: Vec<usize> = path.to_vec();
        let key = filter_path_key(&path);
        let FilterNode::Condition(condition) = node else {
            return div().into_any_element();
        };

        let field_options: Vec<ComboOption> = self
            .state
            .columns
            .iter()
            .map(|column| ComboOption::plain(column.name.clone()))
            .collect();
        let field_selected = condition.column.clone();
        let field = self.filter_field_combo(&path, cx);
        field.update(cx, |combo, cx| {
            combo.set_options(field_options, cx);
            combo.set_selected(field_selected, cx);
        });

        let operator_index = FilterOperator::ALL
            .iter()
            .position(|operator| *operator == condition.operator)
            .unwrap_or(0);
        let operator = self.filter_operator_combo(&path, cx);
        operator.update(cx, |combo, cx| {
            combo.set_selected(operator_index.to_string(), cx);
        });

        let mut row = div()
            .id(SharedString::from(format!("filter-row-{key}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full();
        match rail {
            FilterRail::None => {}
            FilterRail::Spacer | FilterRail::Toggle(_) => {
                // Every in-group condition row leads with its enable checkbox in a fixed left-hand
                // column, so the boxes line up down the panel (and sit at the card's left edge); the
                // `并且/或者` pill, when the row has one, follows it in the same rail.
                let mut rail_cell = div()
                    .w(px(FILTER_RAIL_WIDTH))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(self.filter_enable_checkbox(
                        SharedString::from(format!("filter-enabled-{key}")),
                        &path,
                        node.enabled(),
                        cx,
                    ));
                if let FilterRail::Toggle(conjunction) = rail {
                    rail_cell = rail_cell.child(self.filter_conjunction_pill(
                        SharedString::from(format!("filter-conjunction-{key}")),
                        &path,
                        conjunction,
                        ui::TogglePillStyle::in_group(theme),
                        cx,
                    ));
                }
                row = row.child(rail_cell);
            }
        }
        row = row.child(field).child(operator);
        // An operator that consumes no value (`是 null`, `不是空的`, ...) gets no value field at
        // all: a disabled-looking box still read as an editable input, so hide the slot instead of
        // leaving a placeholder behind. `Between` keeps its two fields.
        if condition.operator.needs_value() {
            row = row.child(self.render_filter_value(&path, 0, cx));
        }
        if condition.operator.needs_second_value() {
            row = row
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("filter.between").to_string()),
                )
                .child(self.render_filter_value(&path, 1, cx));
        }
        row = row.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .flex_none()
                .child(self.filter_row_action(
                    &path,
                    FilterAction::Remove,
                    FILTER_ROW_ACTION_SIZE,
                    FILTER_ROW_ACTION_RADIUS,
                    cx,
                ))
                .child(self.filter_row_action(
                    &path,
                    FilterAction::AddCondition,
                    FILTER_ROW_ACTION_SIZE,
                    FILTER_ROW_ACTION_RADIUS,
                    cx,
                )),
        );

        row.into_any_element()
    }

    /// A group node: a bordered card around its children. Its outgoing boundary (`并且/或者` plus
    /// the next-group actions) is drawn as a separate row by [`Self::render_filter_children`].
    fn render_filter_group_box(
        &mut self,
        path: &[usize],
        children: &[FilterNode],
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let path: Vec<usize> = path.to_vec();
        let key = filter_path_key(&path);
        let body = self.render_filter_children(children, &mut path.clone(), true, cx);

        div()
            .id(SharedString::from(format!("filter-group-{key}")))
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .p_2()
            .rounded(px(10.0))
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .shadow(ui::soft_shadow())
            .child(body)
            .into_any_element()
    }

    /// The `并且/或者` control of a filter node: a small enable checkbox followed by a single-label
    /// pill. The pill shows the node's current conjunction and flips it on click, so one slot
    /// replaces the old two-segment `并且 | 或者` switch; the checkbox turns the node (and, for a
    /// group, its whole subtree) on or off. `id` must be unique within the panel (the same node can
    /// be targeted by both its own rail row and a group's boundary footer). `enabled` is false for
    /// the trailing group boundary, where there is no next node to join yet.
    fn filter_conjunction_control(
        &self,
        id: SharedString,
        path: &[usize],
        conjunction: FilterConjunction,
        style: ui::TogglePillStyle,
        enabled: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let node_enabled = filter_node(&self.state.filter_draft, path)
            .map(FilterNode::enabled)
            .unwrap_or(true);
        let path = path.to_vec();
        let mut control = div().flex().flex_row().items_center().gap_1().flex_none();
        if enabled {
            control = control.child(self.filter_enable_checkbox(
                SharedString::from(format!("{id}-enabled")),
                &path,
                node_enabled,
                cx,
            ));
        } else {
            // Keep the slot (the boundary bar must not shift) but leave it inert and invisible.
            control = control.child(
                div()
                    .flex()
                    .items_center()
                    .flex_none()
                    .opacity(0.0)
                    .child(ui::checkbox_box(node_enabled, theme)),
            );
        }
        let mut pill = if enabled {
            self.filter_conjunction_pill(id, &path, conjunction, style, cx)
        } else {
            ui::toggle_pill(
                id,
                t!(conjunction.label_key()).to_string(),
                style,
                theme,
                |_, _, _| {},
            )
        };
        if !enabled {
            pill = pill.opacity(0.45);
        }
        control.child(pill).into_any_element()
    }

    /// The single-label `并且/或者` pill by itself (no enable checkbox). The in-group rail pairs it
    /// with a checkbox in its own fixed column; the boundary row uses
    /// [`Self::filter_conjunction_control`], which carries both.
    fn filter_conjunction_pill(
        &self,
        id: SharedString,
        path: &[usize],
        conjunction: FilterConjunction,
        style: ui::TogglePillStyle,
        cx: &mut Context<'_, Self>,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let path = path.to_vec();
        let label = t!(conjunction.label_key()).to_string();
        ui::toggle_pill(
            id,
            label,
            style,
            theme,
            cx.listener(move |this, _event, _window, cx| {
                this.filter_toggle_conjunction(path.clone(), cx);
            }),
        )
    }

    /// The small enable checkbox of a filter node. It sits beside the `并且/或者` pill, or on its own
    /// for a group's first condition (which has no pill); toggling it enables/disables that node.
    fn filter_enable_checkbox(
        &self,
        id: SharedString,
        path: &[usize],
        checked: bool,
        cx: &mut Context<'_, Self>,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let path = path.to_vec();
        div()
            .id(id)
            .flex()
            .items_center()
            .flex_none()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, _window, cx| {
                cx.stop_propagation();
                this.filter_toggle_enabled(path.clone(), cx);
            }))
            .child(ui::checkbox_box(checked, theme))
    }

    /// One value field of a condition row. An enabled slot hosts a real [`TextInput`] (caret,
    /// mouse selection, clipboard and IME come from gpui-kit); a slot the operator does not use
    /// shows an inert placeholder instead. A value on a temporal column (`date`/`datetime`/
    /// `timestamp`) also carries a "…" button that opens the date/time picker, like the grid's
    /// in-place cell editor.
    fn render_filter_value(
        &mut self,
        path: &[usize],
        slot: u8,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let path = path.to_vec();
        let condition = filter_node(&self.state.filter_draft, &path);
        let operator = condition
            .map(|node| match node {
                FilterNode::Condition(condition) => condition.operator,
                FilterNode::Group(_) => FilterOperator::Equal,
            })
            .unwrap_or(FilterOperator::Equal);
        let enabled = if slot == 0 {
            operator.needs_value()
        } else {
            operator.needs_second_value()
        };
        let value = match condition {
            Some(FilterNode::Condition(condition)) => {
                if slot == 0 {
                    condition.value.clone()
                } else {
                    condition.value2.clone()
                }
            }
            _ => String::new(),
        };
        let column_type = match condition {
            Some(FilterNode::Condition(condition)) => self
                .state
                .columns
                .iter()
                .find(|column| column.name == condition.column)
                .map(|column| column.data_type.clone())
                .unwrap_or_default(),
            _ => String::new(),
        };
        let temporal = enabled && is_temporal_type(&column_type);
        let has_time = {
            let column_type = column_type.to_ascii_lowercase();
            column_type.contains("datetime") || column_type.contains("timestamp")
        };
        let picker_open = self
            .filter_date_picker
            .as_ref()
            .is_some_and(|picker| picker.path == path && picker.slot == slot);

        let mut field = ui::text_field(theme)
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .min_w(px(FILTER_VALUE_WIDTH))
            .max_w(px(FILTER_VALUE_MAX_WIDTH))
            .h(px(FILTER_CONTROL_HEIGHT))
            .px_2()
            .rounded(px(FILTER_CONTROL_RADIUS))
            .overflow_hidden();
        if !enabled {
            return field
                .bg(rgb(theme.dialog_face))
                .text_size(px(12.0))
                .child(
                    div()
                        .text_color(rgb(theme.text_null))
                        .child(t!("filter.value_placeholder").to_string()),
                )
                .into_any_element();
        }
        if temporal {
            // Record the field's inner rectangle every frame — well before the "…" button can be
            // pressed — so the popup is anchored under it on the very frame it opens, and follows
            // the field when the filter panel scrolls.
            let anchors = self.filter_date_anchors.clone();
            let anchor_key = (path.clone(), slot);
            field = field.on_children_prepainted(move |bounds, _window, _cx| {
                if let Some(rect) = bounds.first() {
                    anchors.borrow_mut().insert(anchor_key.clone(), *rect);
                }
            });
        }
        let picker_value = value.clone();
        let input = self.filter_value_input(&path, slot, value, cx);
        field = field.child(div().flex_1().min_w(px(0.0)).h_full().child(input));
        if temporal {
            let button_path = path.clone();
            field = field.child(
                div()
                    .id(SharedString::from(format!(
                        "filter-date-{}-{slot}",
                        filter_path_key(&path)
                    )))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(px(18.0))
                    .h_full()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(rgb(if picker_open {
                        theme.primary
                    } else {
                        theme.text_muted
                    }))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.filter_toggle_date_picker(
                            button_path.clone(),
                            slot,
                            picker_value.clone(),
                            has_time,
                            window,
                            cx,
                        );
                    }))
                    .child("…"),
            );
        }
        field.into_any_element()
    }

    /// The click-away backdrop under the filter builder's date/time picker. A press outside the
    /// calendar dismisses the popup (and is not forwarded to the control underneath, so a second
    /// click reaches it).
    pub(super) fn render_filter_date_backdrop(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        deferred(
            div()
                .id("filter-date-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.filter_date_picker_dismiss(cx);
                    }),
                ),
        )
        .with_priority(FILTER_DATE_BACKDROP_PRIORITY)
        .into_any_element()
    }

    /// The filter builder's date/time picker, a small popup under the value field that opened it.
    /// The calendar and time row come from gpui-kit (through `ui::compact_calendar`); the footer
    /// is app-drawn, exactly like the grid's in-place picker.
    pub(super) fn render_filter_date_picker(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(picker) = self.filter_date_picker.as_ref() else {
            return div().into_any_element();
        };
        let pane = *self.grid_pane_bounds.borrow();
        let anchor = self
            .filter_date_anchors
            .borrow()
            .get(&(picker.path.clone(), picker.slot))
            .copied()
            .unwrap_or_default();
        let width = 192.0f32;
        let popup_h = if picker.has_time { 248.0 } else { 210.0 };
        // The anchor is the field's inner content box; subtract its 1px border and 8px padding so
        // the popup lines up with the field's outer edge.
        let left = (f32::from(anchor.left()) - 9.0 - f32::from(pane.left()))
            .clamp(0.0, (f32::from(pane.size.width) - width - 8.0).max(0.0));
        let anchor_top = f32::from(anchor.top()) - f32::from(pane.top());
        let below = anchor_top + f32::from(anchor.size.height) + 2.0;
        let top = if below + popup_h > f32::from(pane.size.height) {
            (anchor_top - popup_h - 2.0).max(0.0)
        } else {
            below
        };

        // A mouse-down on a calendar cell must not steal focus from the value field (or the time
        // field); the time segments stay focusable so they can be keyboard-edited.
        let calendar = div()
            .capture_any_mouse_down(|_event, window, _cx| window.prevent_default())
            .child(ui::compact_calendar(&picker.calendar));
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
        let now = chrono::Local::now().naive_local();

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("filter-date-today")
                    .text_size(px(11.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.filter_date_picker_today(window, cx);
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
                        "filter-date-cancel",
                        t!("form.cancel").to_string(),
                        false,
                        theme,
                        cx.listener(|this, _event, window, cx| {
                            this.filter_date_picker_cancel(window, cx);
                        }),
                    ))
                    .child(ui::popup_button(
                        "filter-date-ok",
                        t!("form.ok").to_string(),
                        true,
                        theme,
                        cx.listener(|this, _event, window, cx| {
                            this.filter_date_picker_ok(window, cx);
                        }),
                    )),
            );

        let panel = ui::popup_panel(theme)
            .id("filter-date-picker")
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .p_2()
            .gap_1()
            .border_color(rgb(theme.text_muted))
            // The `TimeField`'s keys never reach the grid, so Escape is handled on the panel.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.filter_date_picker_cancel(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(calendar)
            .children(time_row)
            .child(footer);
        deferred(panel)
            .with_priority(FILTER_DATE_POPUP_PRIORITY)
            .into_any_element()
    }

    /// One `−` / `+` action of the filter builder, drawn in the design's flat outlined style.
    #[allow(clippy::too_many_arguments)]
    fn filter_row_action(
        &self,
        path: &[usize],
        action: FilterAction,
        size: f32,
        radius: f32,
        cx: &mut Context<'_, Self>,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let path = path.to_vec();
        let key = filter_path_key(&path);
        let (icon, color, id) = match action {
            FilterAction::AddCondition => (
                "icons/plus.svg",
                theme.text_muted,
                format!("filter-add-condition-{key}"),
            ),
            FilterAction::AddGroup => (
                "icons/plus.svg",
                theme.text_muted,
                format!("filter-add-group-{key}"),
            ),
            FilterAction::Remove => (
                "icons/minus.svg",
                theme.text_null,
                format!("filter-remove-{key}"),
            ),
        };
        div()
            .id(SharedString::from(id))
            .flex()
            .items_center()
            .justify_center()
            .w(px(size))
            .h(px(size))
            .flex_none()
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.dialog_bg))
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, _event, _window, cx| match action {
                FilterAction::AddCondition => this.filter_add_condition(path.clone(), cx),
                FilterAction::AddGroup => this.filter_add_group(path.clone(), cx),
                FilterAction::Remove => this.filter_remove_node(path.clone(), cx),
            }))
            .child(
                svg()
                    .path(icon)
                    .w(px(size * 0.5))
                    .h(px(size * 0.5))
                    .flex_none()
                    .text_color(rgb(color)),
            )
    }

    fn filter_apply_button(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id("filter-apply")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(20.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(theme.text))
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(|this, _event, _window, cx| this.filter_apply(cx)))
            .child(
                svg()
                    .path("icons/check.svg")
                    .w(px(13.0))
                    .h(px(13.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            )
            .child(t!("grid.apply").to_string())
    }

    pub(super) fn render_grid_controls(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        // A string/JSON key is one value: there is no row to add or delete.
        let scalar_key = self
            .state
            .key_status
            .as_ref()
            .is_some_and(|status| matches!(status.engine.as_deref(), Some("string" | "json")));
        let last = self.state.last_page();
        let at_first = self.state.page_index == 0;
        let at_last = match last {
            Some(last) => self.state.page_index >= last,
            None => !self.state.has_next(),
        };
        let active = theme.text;
        let muted = theme.text_muted;

        let mut controls = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .h(px(30.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(self.grid_icon_button(
                        "grid-add",
                        "icons/plus.svg",
                        theme.icon_connection,
                        self.state.editable
                            && !scalar_key
                            && !self.state.loading
                            && self.state.visible_columns > 0,
                        cx.listener(|this, _event, window, cx| this.add_insert_row(window, cx)),
                    ))
                    .child(
                        self.grid_icon_button(
                            "grid-duplicate",
                            "icons/copy.svg",
                            theme.icon_connection,
                            self.state.editable
                                && !scalar_key
                                && !self.state.loading
                                && self.state.visible_columns > 0
                                && self.state.selection.is_some()
                                && !self.state.rows.is_empty(),
                            cx.listener(|this, _event, window, cx| this.duplicate_row(window, cx)),
                        )
                        .tooltip(ui::text_tooltip(t!("grid.duplicate_row").to_string())),
                    )
                    .child(self.grid_icon_button(
                        "grid-delete",
                        "icons/minus.svg",
                        theme.danger,
                        self.state.editable
                            && !scalar_key
                            && self.state.selection.is_some()
                            && !self.state.rows.is_empty(),
                        cx.listener(|this, _event, _window, cx| this.open_delete_confirm(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-commit",
                        "icons/check.svg",
                        theme.icon_connection,
                        self.cell_editor.is_some()
                            || !self.state.edits.is_empty()
                            || !self.inserts.is_empty(),
                        cx.listener(|this, _event, _window, cx| this.save_grid(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-rollback",
                        "icons/cross.svg",
                        theme.danger,
                        self.cell_editor.is_some()
                            || !self.state.edits.is_empty()
                            || !self.inserts.is_empty(),
                        cx.listener(|this, _event, _window, cx| this.cancel_edits(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-refresh",
                        "icons/refresh.svg",
                        active,
                        true,
                        cx.listener(|this, _event, _window, cx| this.refresh(cx)),
                    )),
            );

        let pager = self.state.sql.is_none().then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(self.grid_icon_button(
                    "grid-first",
                    "icons/first.svg",
                    active,
                    !at_first,
                    cx.listener(|this, _event, _window, cx| this.first_page(cx)),
                ))
                .child(self.grid_icon_button(
                    "grid-prev",
                    "icons/prev.svg",
                    active,
                    !at_first,
                    cx.listener(|this, _event, _window, cx| this.prev_page(cx)),
                ))
                .child(self.render_page_input())
                .child(self.grid_icon_button(
                    "grid-next",
                    "icons/next.svg",
                    active,
                    !at_last,
                    cx.listener(|this, _event, _window, cx| this.next_page(cx)),
                ))
                .child(self.grid_icon_button(
                    "grid-last",
                    "icons/last.svg",
                    active,
                    !at_last,
                    cx.listener(|this, _event, _window, cx| this.last_page(cx)),
                ))
                .child(self.grid_icon_button(
                    "grid-gear",
                    "icons/gear.svg",
                    if self.page_size_menu_open {
                        theme.primary
                    } else {
                        muted
                    },
                    true,
                    cx.listener(|this, _event, _window, cx| this.toggle_page_size_menu(cx)),
                ))
                .child(self.render_form_toggle(cx))
        });
        // Navicat's grid shows the last fetch's duration next to the pager; the 耗时 label sits
        // just left of it so the eye finds the time and the page controls together.
        let timing = self.state.elapsed.map(|elapsed| {
            t!(
                "grid.query_time",
                seconds = format!("{:.3}", elapsed.as_secs_f64())
            )
            .to_string()
        });
        if timing.is_some() || pager.is_some() {
            let mut right = div().flex().flex_row().items_center().gap_2();
            if let Some(timing) = timing {
                // A very small font step down, so it reads as a status annotation, not a control.
                right = right.child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(rgb(muted))
                        .whitespace_nowrap()
                        .child(timing),
                );
            }
            if let Some(pager) = pager {
                right = right.child(pager);
            }
            controls = controls.child(right);
        }
        controls
    }

    /// The footer's 编辑表单 toggle, right of the page-size gear. It only lights up (and is only
    /// clickable) while a data row is selected, and opens the right-hand pane's row form.
    fn render_form_toggle(&self, cx: &mut Context<'_, Self>) -> Stateful<Div> {
        let theme = self.theme;
        let has_row = self.state.selection.as_ref().is_some_and(|selection| {
            selection
                .row_indices()
                .into_iter()
                .any(|row| matches!(self.row_at(row), Some(GridRow::Data(_))))
        });
        let open = self.app.upgrade().is_some_and(|app| app.read(cx).form_open);
        let button = self
            .grid_icon_button(
                "grid-form",
                "icons/form.svg",
                if open { theme.primary } else { theme.text },
                has_row,
                cx.listener(|this, _event, _window, cx| {
                    if let Some(app) = this.app.upgrade() {
                        app.update(cx, |app, cx| app.toggle_form(cx));
                    }
                    // The button's lit state reads `AppView::form_open`; repaint this grid. Do it here
                    // rather than through `AppView::notify_grids`, which would update this very grid
                    // re-entrantly and abort.
                    cx.notify();
                }),
            )
            .tooltip(ui::text_tooltip(t!("grid.form_view").to_string()));
        // With no record selected the icon is disabled; fade it well below the page-size gear's
        // muted tint so "not available yet" is obvious at a glance.
        if has_row { button } else { button.opacity(0.4) }
    }

    pub(super) fn grid_icon_button(
        &self,
        id: &'static str,
        icon: &'static str,
        color: u32,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let tint = if enabled { color } else { theme.text_muted };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(24.0))
            .h(px(22.0))
            .rounded_sm()
            .when(enabled, move |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .when(enabled, |this| this.on_click(on_click))
            .child(
                svg()
                    .path(icon)
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_none()
                    .text_color(rgb(tint)),
            )
    }

    pub(super) fn render_page_input(&self) -> impl IntoElement {
        let theme = self.theme;
        ui::text_field(theme)
            .id("grid-page-input")
            .relative()
            .flex()
            .items_center()
            .justify_start()
            .w(px(52.0))
            .h(px(22.0))
            .pl(px(2.0))
            .pr(px(2.0))
            .text_size(px(12.0))
            .cursor_text()
            .child(self.page_input_view.clone())
    }

    /// The collapsible "Limit Records [n] records per page" bar, revealed by the gear button.
    pub(super) fn render_record_limit_panel(
        &self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let focused = self.page_size_view.read(cx).is_focused(window, cx);
        let limit_records = self.limit_records(cx);

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .h(px(28.0))
            .flex_none()
            .w_full()
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .id("page-limit-toggle")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(
                        cx.listener(|this, _event, _window, cx| this.toggle_limit_records(cx)),
                    )
                    .tooltip(ui::text_tooltip(
                        t!("grid.limit_records_hint", max = NO_LIMIT_PAGE_SIZE).to_string(),
                    ))
                    .child(checkbox_box(limit_records, theme))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child(t!("grid.limit_records").to_string()),
                    ),
            )
            .child(
                div()
                    .id("page-size-input")
                    .relative()
                    .flex()
                    .items_center()
                    .justify_start()
                    .w(px(64.0))
                    .h(px(20.0))
                    .pl(px(2.0))
                    .pr(px(2.0))
                    .bg(rgb(if limit_records {
                        theme.input_bg
                    } else {
                        theme.dialog_face
                    }))
                    .border_1()
                    .border_color(rgb(if focused { theme.primary } else { theme.border }))
                    .text_size(px(12.0))
                    .cursor_text()
                    .child(self.page_size_view.clone()),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .child(t!("grid.records_per_page").to_string()),
            )
            .into_any_element()
    }

    /// The numeric value of a cell as the user currently sees it: a staged edit or pending
    /// insert wins over the loaded row, so the sum tracks in-flight typing. `None` for `NULL`,
    /// text, binary and boolean cells.
    fn numeric_cell(&self, row: usize, col: usize) -> Option<NumericCell> {
        if let Some(staged) = self.staged_value(row, col) {
            return staged.as_deref().and_then(parse_numeric_text);
        }
        let GridRow::Data(data) = self.row_at(row)? else {
            return None;
        };
        match self.state.rows.get(data)?.get(self.state.source_col(col))? {
            CellValue::Int(value) => Some(NumericCell::Int(i128::from(*value))),
            CellValue::Uint(value) => Some(NumericCell::Int(i128::from(*value))),
            CellValue::Float(value) if value.is_finite() => Some(NumericCell::Float(*value)),
            _ => None,
        }
    }

    /// The Excel-style 求和 of the current selection: the sum of every numeric cell it covers.
    /// `None` unless the selection spans more than one cell and holds at least one number, so a
    /// single click keeps the status bar's plain selection text. Both table pages and ad-hoc
    /// query results go through here, so the two read the same.
    ///
    /// Called from `render`, so it stays linear in the selected cell count: a page may hold up to
    /// a million rows and a whole-column selection must not turn into a quadratic scan.
    fn selection_sum(&self) -> Option<String> {
        let selection = self.state.selection.as_ref()?;
        let cell_count: usize = selection
            .ranges
            .iter()
            .map(|range| {
                let (start_row, end_row) = range.rows();
                let (start_col, end_col) = range.cols();
                (end_row - start_row + 1) * (end_col - start_col + 1)
            })
            .sum();
        if cell_count < 2 {
            return None;
        }

        // The sum is O(selected cells); a whole-column selection over a large page would otherwise
        // be recomputed on every frame. Cache it keyed by a cheap signature of the inputs.
        let signature = self.selection_sum_signature(selection);
        if let Some((cached_signature, cached)) = self.sum_cache.borrow().as_ref()
            && *cached_signature == signature
        {
            return cached.clone();
        }
        let result = self.compute_selection_sum(selection);
        *self.sum_cache.borrow_mut() = Some((signature, result.clone()));
        result
    }

    /// A cheap hash of everything `selection_sum` depends on: the selected ranges, the loaded
    /// rows, and the pending edits/inserts.
    fn selection_sum_signature(&self, selection: &CellSelection) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        selection.ranges.len().hash(&mut hasher);
        for range in &selection.ranges {
            range.anchor.hash(&mut hasher);
            range.cursor.hash(&mut hasher);
        }
        (Arc::as_ptr(&self.state.rows) as usize).hash(&mut hasher);
        self.state.visible_columns.hash(&mut hasher);
        self.state.edits.hash(&mut hasher);
        self.inserts.hash(&mut hasher);
        hasher.finish()
    }

    fn compute_selection_sum(&self, selection: &CellSelection) -> Option<String> {
        let mut total = NumericSum::default();
        match selection.ranges.as_slice() {
            // One rectangle cannot repeat a cell, so it needs no de-duplication.
            [range] => {
                let (start_row, end_row) = range.rows();
                let (start_col, end_col) = range.cols();
                for row in start_row..=end_row {
                    for col in start_col..=end_col {
                        total.add(self.numeric_cell(row, col));
                    }
                }
            }
            // Ctrl-dragged ranges may overlap; a set keeps a shared cell from counting twice.
            ranges => {
                let mut seen = std::collections::HashSet::new();
                for range in ranges {
                    let (start_row, end_row) = range.rows();
                    let (start_col, end_col) = range.cols();
                    for row in start_row..=end_row {
                        for col in start_col..=end_col {
                            if seen.insert((row, col)) {
                                total.add(self.numeric_cell(row, col));
                            }
                        }
                    }
                }
            }
        }

        total.render()
    }

    pub(super) fn render_grid_status(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let end = self
            .state
            .page_index
            .saturating_mul(self.state.page_size)
            .saturating_add(self.state.rows.len() as u64);
        // Some drivers do not report a total row count; then the "N total" half is omitted rather
        // than shown as a misleading 0.
        let info = match self.state.total_rows {
            Some(total) => t!(
                "grid.page_info",
                end = end,
                total = total,
                page = self.state.page_index + 1
            )
            .to_string(),
            None => t!(
                "grid.page_info_unknown",
                end = end,
                page = self.state.page_index + 1
            )
            .to_string(),
        };
        let message: AnyElement = match self.state.selection.as_ref() {
            Some(selection) => {
                let (start_row, end_row) = selection.rows();
                let (start_col, end_col) = selection.cols();
                let text = if selection.ranges.len() > 1 {
                    t!(
                        "grid.selection_info",
                        rows = selection.row_indices().len(),
                        cols = selection.col_indices().len()
                    )
                    .to_string()
                } else {
                    t!(
                        "grid.selection_info",
                        rows = end_row - start_row + 1,
                        cols = end_col - start_col + 1
                    )
                    .to_string()
                };
                div().child(text).into_any_element()
            }
            // The SQL the grid ran: a hand-selectable line (drag, double-click to select all,
            // Ctrl+C) so the user can copy the exact statement from the status bar.
            None => self.render_status_sql(flatten_status_sql(&self.state.sql()), cx),
        };
        // Excel-style aggregate: a multi-cell selection that holds numbers also reports their
        // sum, right after the selection text.
        let sum = self.selection_sum();

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_4()
            .px_2()
            .h(px(24.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .text_size(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(message)
                    .when_some(sum, |row, sum| {
                        row.child(
                            div()
                                .flex_none()
                                .text_color(rgb(theme.danger))
                                .child(t!("grid.selection_sum", sum = sum).to_string()),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_4()
                    .flex_none()
                    // Grids without a toolbar (SQL results) report the fetch here instead.
                    .when(self.state.loading && !self.state.show_toolbar, |right| {
                        right.child(self.render_loading_spinner())
                    })
                    .child(info),
            )
    }

    /// The status bar's SQL line, hand-selectable: drag to select, double-click to select the whole
    /// statement, Ctrl+A / Ctrl+C as usual. The layout is cached so a pointer position maps back to
    /// a byte offset in the flattened text.
    fn render_status_sql(&mut self, sql: String, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        // A different statement (a reload) invalidates any previous selection.
        if self.status_sql_text != sql {
            self.status_sql_text = sql.clone();
            self.status_sql_anchor = 0;
            self.status_sql_cursor = 0;
            self.status_sql_selecting = false;
        }
        let (start, end) = self.status_sql_range();
        let mut styled = StyledText::new(sql);
        if start < end {
            styled = styled.with_highlights(vec![(
                start..end,
                HighlightStyle {
                    background_color: Some(rgb(theme.grid_selection_bg).into()),
                    ..Default::default()
                },
            )]);
        }
        self.status_sql_layout = styled.layout().clone();
        div()
            .id("grid-status-sql")
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .track_focus(&self.status_sql_focus)
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.status_sql_focus, cx);
                    if event.click_count >= 2 {
                        this.status_sql_anchor = 0;
                        this.status_sql_cursor = this.status_sql_text.len();
                        this.status_sql_selecting = false;
                    } else {
                        let index = this.status_sql_index_for_position(event.position);
                        this.status_sql_anchor = index;
                        this.status_sql_cursor = index;
                        this.status_sql_selecting = true;
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if !this.status_sql_selecting {
                    return;
                }
                let index = this.status_sql_index_for_position(event.position);
                if index != this.status_sql_cursor {
                    this.status_sql_cursor = index;
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.status_sql_selecting {
                        this.status_sql_selecting = false;
                        cx.notify();
                    }
                }),
            )
            .key_context(GRID_STATUS_CONTEXT)
            .on_action(cx.listener(|this, _: &CopyStatusSql, _window, cx| this.status_sql_copy(cx)))
            .on_action(cx.listener(|this, _: &SelectAllStatusSql, _window, cx| {
                this.status_sql_select_all(cx);
            }))
            .child(styled)
            .into_any_element()
    }

    fn status_sql_index_for_position(&self, position: Point<Pixels>) -> usize {
        match self.status_sql_layout.index_for_position(position) {
            Ok(index) | Err(index) => index,
        }
    }

    /// The selected byte range of the status SQL, clamped to the text and snapped to character
    /// boundaries (a stale selection from a longer previous statement must not panic the layout).
    fn status_sql_range(&self) -> (usize, usize) {
        let len = self.status_sql_text.len();
        let mut start = self.status_sql_anchor.min(self.status_sql_cursor).min(len);
        let mut end = self.status_sql_anchor.max(self.status_sql_cursor).min(len);
        while start > 0 && !self.status_sql_text.is_char_boundary(start) {
            start -= 1;
        }
        while end > 0 && !self.status_sql_text.is_char_boundary(end) {
            end -= 1;
        }
        (start, end)
    }

    fn status_sql_copy(&mut self, cx: &mut Context<'_, Self>) {
        let (start, end) = self.status_sql_range();
        if start < end {
            let selected = self.status_sql_text[start..end].to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(selected));
        }
    }

    fn status_sql_select_all(&mut self, cx: &mut Context<'_, Self>) {
        self.status_sql_anchor = 0;
        self.status_sql_cursor = self.status_sql_text.len();
        cx.notify();
    }

    pub(super) fn limit_records(&self, cx: &Context<'_, Self>) -> bool {
        self.app
            .upgrade()
            .map(|app| app.read(cx).limit_records)
            .unwrap_or(true)
    }
}

/// Collapse a statement into the single-line preview the 24px status bar can show.
///
/// A query grid's `sql` keeps its author's newlines and indentation. gpui shapes every `\n` as a
/// line break even under `whitespace_nowrap`, so the raw text would lay out as a block far taller
/// than the bar and spill over the result rows above it.
pub(super) fn flatten_status_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One numeric value lifted out of a cell for the status bar's aggregate. Integers keep their
/// exact value (a `BIGINT` column can exceed what `f64` represents), floats fall back to `f64`.
enum NumericCell {
    Int(i128),
    Float(f64),
}

/// Running total of the selected numeric cells. Integers are summed exactly and only folded into
/// `f64` at the end, so a column of `BIGINT`s stays exact as long as it holds no decimal.
#[derive(Default)]
struct NumericSum {
    int: i128,
    float: f64,
    has_float: bool,
    count: usize,
}

impl NumericSum {
    fn add(&mut self, value: Option<NumericCell>) {
        match value {
            Some(NumericCell::Int(value)) => {
                self.int += value;
                self.count += 1;
            }
            Some(NumericCell::Float(value)) => {
                self.has_float = true;
                self.float += value;
                self.count += 1;
            }
            None => {}
        }
    }

    /// The formatted sum, or `None` when the selection held no numbers at all (text, `NULL` and
    /// binary cells only) — the status bar then shows just the selection text.
    fn render(&self) -> Option<String> {
        if self.count == 0 {
            return None;
        }
        Some(if self.has_float {
            format_numeric_sum(self.int as f64 + self.float)
        } else {
            self.int.to_string()
        })
    }
}

/// Read a staged cell string (a pending edit or a pending insert row) as a number. `"(Null)"`,
/// blank text and non-numeric text all fail to parse, which is the "not a number" answer we
/// want; non-finite spellings (`NaN`, `inf`) are rejected too so they cannot poison the sum.
fn parse_numeric_text(text: &str) -> Option<NumericCell> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(value) = text.parse::<i128>() {
        return Some(NumericCell::Int(value));
    }
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(NumericCell::Float)
}

/// Render a summed `f64` for the status bar: no trailing `.0`, no float noise (`0.1 + 0.2`
/// prints as `0.3`).
fn format_numeric_sum(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    if value == value.trunc() && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    let mut text = format!("{value:.6}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{
        NumericCell, NumericSum, flatten_status_sql, format_numeric_sum, parse_numeric_text,
    };

    #[test]
    fn status_sql_flattens_newlines_and_indentation() {
        let sql = "SELECT DISTINCT\n  loc.positive_aisle_no\nFROM\n  wes_location loc\nLEFT JOIN wes_capacity capacity ON capacity.id = loc.capacity_id\n";
        assert_eq!(
            flatten_status_sql(sql),
            "SELECT DISTINCT loc.positive_aisle_no FROM wes_location loc \
             LEFT JOIN wes_capacity capacity ON capacity.id = loc.capacity_id"
        );
        assert_eq!(flatten_status_sql("  "), "");
        assert_eq!(flatten_status_sql(""), "");
    }

    #[test]
    fn numeric_text_accepts_numbers_only() {
        assert!(matches!(
            parse_numeric_text(" 55239 "),
            Some(NumericCell::Int(55239))
        ));
        assert!(matches!(
            parse_numeric_text("-7"),
            Some(NumericCell::Int(-7))
        ));
        assert!(matches!(
            parse_numeric_text("12.5"),
            Some(NumericCell::Float(value)) if value == 12.5
        ));
        // A staged NULL, blank input and plain text are all "not a number".
        assert!(parse_numeric_text("(Null)").is_none());
        assert!(parse_numeric_text("").is_none());
        assert!(parse_numeric_text("abc").is_none());
        // Non-finite spellings must not slip into the sum.
        assert!(parse_numeric_text("NaN").is_none());
        assert!(parse_numeric_text("inf").is_none());
    }

    #[test]
    fn numeric_sum_drops_trailing_zeroes() {
        assert_eq!(format_numeric_sum(55239.0), "55239");
        assert_eq!(format_numeric_sum(-1400.0), "-1400");
        assert_eq!(format_numeric_sum(12.5), "12.5");
        // Float noise from summing decimals is trimmed to something readable.
        assert_eq!(format_numeric_sum(0.1 + 0.2), "0.3");
    }

    #[test]
    fn numeric_sum_keeps_integers_exact_and_skips_non_numbers() {
        let mut sum = NumericSum::default();
        // A BIGINT beyond `f64`'s exact range still sums exactly.
        sum.add(Some(NumericCell::Int(9_007_199_254_740_993)));
        sum.add(Some(NumericCell::Int(1)));
        sum.add(None); // NULL / text / binary cells contribute nothing.
        assert_eq!(sum.render().as_deref(), Some("9007199254740994"));

        // One float switches the total to float arithmetic.
        let mut mixed = NumericSum::default();
        mixed.add(Some(NumericCell::Int(3)));
        mixed.add(Some(NumericCell::Float(0.5)));
        assert_eq!(mixed.render().as_deref(), Some("3.5"));
    }

    #[test]
    fn numeric_sum_of_nothing_is_none() {
        let sum = NumericSum::default();
        assert!(sum.render().is_none());
    }
}
