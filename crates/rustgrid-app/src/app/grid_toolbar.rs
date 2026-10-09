use super::grid::filter_node;
use super::*;

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
    pub(super) fn render_grid_toolbar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
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
                self.state.sql.is_none(),
                self.state.filter_open,
                false,
                cx.listener(|this, _event, _window, cx| this.toggle_filter_panel(cx)),
            ))
            .child(self.grid_tool_button(
                "grid-sort",
                "icons/sort.svg",
                t!("grid.sort").to_string(),
                theme.text,
                self.state.sql.is_none(),
                self.state.sort_open,
                false,
                cx.listener(|this, _event, _window, cx| this.toggle_sort_panel(cx)),
            ))
            .child(self.grid_tool_button(
                "grid-import",
                "icons/import.svg",
                t!("grid.import").to_string(),
                theme.icon_views,
                self.state.sql.is_none(),
                false,
                false,
                cx.listener(|this, _event, _window, cx| this.open_grid_import(cx)),
            ))
            .child(self.grid_tool_button(
                "grid-export",
                "icons/export.svg",
                t!("grid.export").to_string(),
                theme.icon_views,
                self.state.sql.is_none(),
                false,
                false,
                cx.listener(|this, _event, _window, cx| this.open_grid_export(cx)),
            ))
    }

    /// Open the Import Wizard scoped to this grid's database, defaulting a single source table to
    /// the table the grid is showing. Table grids only (SQL result grids have no import target).
    fn open_grid_import(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
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

    /// Open the Export Wizard with this grid's table pre-selected. Table grids only.
    fn open_grid_export(&mut self, cx: &mut Context<'_, Self>) {
        if self.state.sql.is_some() {
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
            app.open_export_wizard(
                connection_index,
                database_index,
                std::slice::from_ref(&table),
                cx,
            );
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
            .child(ui::icon_button(
                SharedString::from(format!("sort-remove-{index}")),
                "icons/cross.svg",
                theme.danger,
                18.0,
                18.0,
                false,
                theme,
                cx.listener(move |this, _event, _window, cx| {
                    cx.stop_propagation();
                    this.sort_remove_rule(index, cx)
                }),
            ))
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
            .gap_2()
            .flex_none()
            .min_h(px(46.0))
            .max_h(px(300.0))
            .px_4()
            .py_2()
            .overflow_y_scroll();

        let content: AnyElement = if self.state.filter_draft.is_empty() {
            div()
                .id("filter-add-hint")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .h(px(36.0))
                .w_full()
                .flex_none()
                .rounded(px(12.0))
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
            .px_4()
            .h(px(30.0))
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
        let toggle = self.filter_conjunction_toggle(
            SharedString::from(format!("filter-boundary-toggle-{key}")),
            &toggle_path,
            toggle_conjunction,
            ui::TogglePillStyle::boundary(theme),
            toggle_enabled,
            cx,
        );
        // The trailing connector (no next group yet) keeps its pill out of the way until hovered,
        // so the panel does not end on a dangling `并且/或者`.
        let toggle = if toggle_enabled {
            toggle.into_any_element()
        } else {
            toggle
                .opacity(0.0)
                .group_hover(hover_group.clone(), |style| style.opacity(1.0))
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
            .child(self.filter_conjunction_toggle(
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
            .gap_3()
            .w_full();
        match rail {
            FilterRail::None => {}
            FilterRail::Spacer => {
                row = row.child(div().w(px(FILTER_RAIL_WIDTH)).flex_none());
            }
            FilterRail::Toggle(conjunction) => {
                row = row.child(
                    div()
                        .w(px(FILTER_RAIL_WIDTH))
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_end()
                        .child(self.filter_conjunction_toggle(
                            SharedString::from(format!("filter-conjunction-{key}")),
                            &path,
                            conjunction,
                            ui::TogglePillStyle::in_group(theme),
                            true,
                            cx,
                        )),
                );
            }
        }
        row = row
            .child(field)
            .child(operator)
            .child(self.render_filter_value(&path, 0, cx));
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
            .gap_2()
            .w_full()
            .p_3()
            .rounded(px(12.0))
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .shadow(ui::soft_shadow())
            .child(body)
            .into_any_element()
    }

    /// A `并且/或者` segmented toggle. `id` must be unique within the panel (the same node can be
    /// targeted by both its own rail row and a group's boundary footer). `enabled` is false for the
    /// last group's boundary, where there is no next group to join yet.
    fn filter_conjunction_toggle(
        &self,
        id: SharedString,
        path: &[usize],
        conjunction: FilterConjunction,
        style: ui::TogglePillStyle,
        enabled: bool,
        cx: &mut Context<'_, Self>,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let path = path.to_vec();
        let and = t!(FilterConjunction::And.label_key()).to_string();
        let or = t!(FilterConjunction::Or.label_key()).to_string();
        let second_active = conjunction == FilterConjunction::Or;
        let mut toggle = if enabled {
            ui::segmented_toggle(
                id,
                and,
                or,
                second_active,
                style,
                theme,
                cx.listener(move |this, _event, _window, cx| {
                    this.filter_toggle_conjunction(path.clone(), cx);
                }),
            )
        } else {
            ui::segmented_toggle(id, and, or, second_active, style, theme, |_, _, _| {})
        };
        if !enabled {
            toggle = toggle.opacity(0.45);
        }
        toggle
    }

    fn render_filter_value(
        &self,
        path: &[usize],
        slot: u8,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let key = filter_path_key(path);
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
        let focused = enabled && self.filter_active.as_ref() == Some(&(path.clone(), slot));
        let handles = if slot == 0 {
            &self.filter_value_focus
        } else {
            &self.filter_value2_focus
        };
        let handle = handles
            .iter()
            .find(|(candidate, _)| *candidate == path)
            .map(|(_, handle)| handle.clone());

        let mut field = ui::text_field(theme)
            .id(SharedString::from(format!("filter-value-{key}-{slot}")))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .min_w(px(FILTER_VALUE_WIDTH))
            .h(px(FILTER_CONTROL_HEIGHT))
            .px_2()
            .text_size(px(12.0))
            .rounded(px(FILTER_CONTROL_RADIUS))
            .overflow_hidden();
        if enabled {
            if let Some(handle) = handle {
                let key_target = path.clone();
                let click_target = path.clone();
                field = field
                    .track_focus(&handle)
                    .cursor_text()
                    .on_key_down(cx.listener(move |this, event, _window, cx| {
                        this.filter_value_key(key_target.clone(), slot, event, cx);
                    }))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.filter_focus_value(click_target.clone(), slot, window, cx);
                    }))
                    .child(self.ime_probe(&handle));
            }
        } else {
            field = field
                .bg(rgb(theme.dialog_face))
                .text_color(rgb(theme.text_muted));
        }
        let placeholder = enabled && value.is_empty() && !focused;
        let content = if placeholder {
            div()
                .text_color(rgb(theme.text_null))
                .child(t!("filter.value_placeholder").to_string())
        } else {
            div().child(value)
        };
        field.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .child(content)
                .when(focused, move |style| {
                    style.child(div().w(px(1.5)).h(px(13.0)).flex_none().bg(rgb(theme.text)))
                }),
        )
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
                            && !self.state.loading
                            && !self.state.columns.is_empty(),
                        cx.listener(|this, _event, window, cx| this.add_insert_row(window, cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-delete",
                        "icons/minus.svg",
                        theme.danger,
                        self.state.editable
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

        if self.state.sql.is_none() {
            controls = controls.child(
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
                    .child(self.render_page_input(cx))
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
                    )),
            );
        }
        controls
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

    pub(super) fn render_page_input(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        ui::text_field(theme)
            .id("grid-page-input")
            .track_focus(&self.page_input_focus)
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
            .on_key_down(cx.listener(|this, event, _window, cx| this.page_input_key(event, cx)))
            .on_click(cx.listener(|this, _event, window, cx| {
                window.focus(&this.page_input_focus, cx);
                cx.notify();
            }))
            .child(self.page_input.clone())
            .when(self.page_input_focused && self.caret(), move |input| {
                input.child(ui::text_caret(theme))
            })
            .child(self.ime_probe(&self.page_input_focus))
    }

    /// The collapsible "Limit Records [n] records per page" bar, revealed by the gear button.
    pub(super) fn render_record_limit_panel(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let focused = self.page_size_focused;
        let page_size = self.state.page_size;
        let limit_records = self.limit_records(cx);
        let value = if focused {
            self.page_size_input.clone()
        } else {
            page_size.to_string()
        };

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
                    .track_focus(&self.page_size_focus)
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
                    .on_key_down(
                        cx.listener(|this, event, _window, cx| this.page_size_key(event, cx)),
                    )
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.page_size_input = page_size.to_string();
                        window.focus(&this.page_size_focus, cx);
                        cx.notify();
                    }))
                    .child(
                        div().flex().flex_row().items_center().child(value).child(
                            div()
                                .w(px(1.5))
                                .h(px(13.0))
                                .flex_none()
                                .when(focused && self.caret(), move |caret| {
                                    caret.bg(rgb(theme.text))
                                }),
                        ),
                    )
                    .child(self.ime_probe(&self.page_size_focus)),
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
        match self.state.rows.get(row)?.get(col)? {
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
        self.state.columns.len().hash(&mut hasher);
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

    pub(super) fn render_grid_status(&self) -> impl IntoElement {
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
        let timing = self.state.elapsed.map(|elapsed| {
            t!(
                "grid.query_time",
                seconds = format!("{:.3}", elapsed.as_secs_f64())
            )
            .to_string()
        });
        let message = match self.state.selection.as_ref() {
            Some(selection) => {
                let (start_row, end_row) = selection.rows();
                let (start_col, end_col) = selection.cols();
                if selection.ranges.len() > 1 {
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
                }
            }
            None => flatten_status_sql(&self.state.sql()),
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
                    .when_some(timing, |row, timing| {
                        row.child(div().text_color(rgb(theme.text_muted)).child(timing))
                    })
                    .child(info),
            )
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
