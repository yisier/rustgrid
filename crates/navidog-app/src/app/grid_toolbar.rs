use super::grid::filter_node;
use super::*;

/// The per-row actions of the filter builder.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterAction {
    /// Remove the node.
    Remove,
    /// Add a condition child (inside a group) or sibling.
    AddCondition,
    /// Add a nested group child (inside a group) or sibling.
    AddGroup,
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
                "grid-begin",
                "icons/transaction.svg",
                t!("grid.begin_transaction").to_string(),
                theme.text,
                true,
                false,
                true,
                |_, _, _| {},
            ))
            .child(self.grid_tool_button(
                "grid-text",
                "icons/text.svg",
                t!("grid.text").to_string(),
                theme.text,
                true,
                false,
                true,
                |_, _, _| {},
            ))
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
                true,
                self.state.sort_open,
                false,
                cx.listener(|this, _event, _window, cx| this.toggle_sort_panel(cx)),
            ))
            .child(self.grid_tool_button(
                "grid-import",
                "icons/import.svg",
                t!("grid.import").to_string(),
                theme.icon_views,
                true,
                false,
                false,
                |_, _, _| {},
            ))
            .child(self.grid_tool_button(
                "grid-export",
                "icons/export.svg",
                t!("grid.export").to_string(),
                theme.icon_views,
                true,
                false,
                false,
                |_, _, _| {},
            ))
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
            .on_click(on_click)
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
            .child(self.sort_arrow_button("sort-move-up", "icons/arrow-up.svg", 1, cx))
            .child(self.sort_arrow_button("sort-move-down", "icons/arrow-down.svg", -1, cx))
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
            .flex_none()
            .min_h(px(46.0))
            .max_h(px(224.0))
            .overflow_y_scroll();

        if self.state.filter_draft.is_empty() {
            list = list.child(
                div()
                    .id("filter-add-hint")
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
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.filter_add_condition(Vec::new(), cx);
                    }))
                    .child(sort_plus_badge(theme))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!("grid.filter_hint").to_string()),
                    ),
            );
        } else {
            let draft = self.state.filter_draft.clone();
            for row in self.filter_rows(&draft, &mut Vec::new(), 0, cx) {
                list = list.child(row);
            }
        }

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_1()
            .h(px(26.0))
            .flex_none()
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(self.filter_apply_button(cx));

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

    /// Flatten the filter tree into rows, depth-first. A group renders its condition rows and then
    /// its boundary row (`并且/或者` + add/remove group).
    fn filter_rows(
        &mut self,
        nodes: &[FilterNode],
        prefix: &mut Vec<usize>,
        depth: usize,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        for (index, node) in nodes.iter().enumerate() {
            prefix.push(index);
            match node {
                FilterNode::Condition(_) => {
                    rows.push(self.render_filter_condition_row(prefix, node, depth, index == 0, cx))
                }
                FilterNode::Group(group) => {
                    rows.extend(self.filter_rows(&group.children, prefix, depth + 1, cx));
                    let next = nodes.get(index + 1).map(|_| {
                        let mut path = prefix.clone();
                        if let Some(last) = path.last_mut() {
                            *last = index + 1;
                        }
                        path
                    });
                    rows.push(self.render_filter_group_control(prefix, next, depth, cx));
                }
            }
            prefix.pop();
        }
        rows
    }

    /// One condition row: `[并且/或者] field op value ... [−][+]`. The first condition of a group
    /// has no leading conjunction, so it gets a spacer instead of the toggle.
    fn render_filter_condition_row(
        &mut self,
        path: &[usize],
        node: &FilterNode,
        depth: usize,
        first: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let path: Vec<usize> = path.to_vec();
        let key = filter_path_key(&path);
        let FilterNode::Condition(condition) = node else {
            return div().into_any_element();
        };
        let mut row = div()
            .id(SharedString::from(format!("filter-row-{key}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .h(px(26.0))
            .flex_none()
            .when(depth > 0, |row| row.pl(px(depth as f32 * 16.0 + 4.0)));

        row = row.child(if first {
            div()
                .w(px(FILTER_TOGGLE_WIDTH))
                .flex_none()
                .into_any_element()
        } else {
            self.filter_conjunction_toggle(&path, condition.conjunction, cx)
                .into_any_element()
        });

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

        row = row
            .child(field)
            .child(operator)
            .child(self.render_filter_value(&path, 0, cx));
        if condition.operator.needs_second_value() {
            row = row
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("filter.between").to_string()),
                )
                .child(self.render_filter_value(&path, 1, cx));
        }
        row = row
            .child(self.filter_row_action(&path, FilterAction::Remove, cx))
            .child(self.filter_row_action(&path, FilterAction::AddCondition, cx));

        row.into_any_element()
    }

    /// A group's boundary row, rendered after its conditions. The toggle sets the conjunction of
    /// the next group; `+` inserts a new group after this one and `−` removes this group. The row
    /// starts at the field column so the controls sit under the operator column.
    fn render_filter_group_control(
        &mut self,
        group_path: &[usize],
        next_path: Option<Vec<usize>>,
        depth: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let group_path: Vec<usize> = group_path.to_vec();
        let key = filter_path_key(&group_path);
        let mut controls = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_1()
            .w(px(FILTER_OPERATOR_WIDTH))
            .flex_none();
        match next_path {
            Some(next_path) => {
                let conjunction = filter_node(&self.state.filter_draft, &next_path)
                    .map(FilterNode::conjunction)
                    .unwrap_or_default();
                controls =
                    controls.child(self.filter_conjunction_toggle(&next_path, conjunction, cx));
            }
            None => controls = controls.child(div().w(px(FILTER_TOGGLE_WIDTH)).flex_none()),
        }
        controls = controls
            .child(self.filter_row_action(&group_path, FilterAction::AddGroup, cx))
            .child(self.filter_row_action(&group_path, FilterAction::Remove, cx));

        div()
            .id(SharedString::from(format!("filter-group-{key}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .pl(px((depth + 1) as f32 * 16.0 + 4.0))
            .h(px(24.0))
            .flex_none()
            .child(
                div()
                    .w(px(FILTER_TOGGLE_WIDTH))
                    .flex_none()
                    .into_any_element(),
            )
            .child(div().w(px(FILTER_FIELD_WIDTH)).flex_none())
            .child(controls)
            .child(div().flex_1())
            .into_any_element()
    }

    fn filter_conjunction_toggle(
        &self,
        path: &[usize],
        conjunction: FilterConjunction,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let path = path.to_vec();
        ui::segmented_toggle(
            SharedString::from(format!("filter-conjunction-{}", filter_path_key(&path))),
            t!(FilterConjunction::And.label_key()).to_string(),
            t!(FilterConjunction::Or.label_key()).to_string(),
            conjunction == FilterConjunction::Or,
            FILTER_TOGGLE_WIDTH,
            theme,
            cx.listener(move |this, _event, _window, cx| {
                this.filter_toggle_conjunction(path.clone(), cx);
            }),
        )
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
            .w(px(FILTER_VALUE_WIDTH))
            .h(px(20.0))
            .flex_none()
            .pl(px(2.0))
            .pr(px(2.0))
            .text_size(px(12.0))
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
        field.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .child(value)
                .when(focused, move |style| {
                    style.child(div().w(px(1.5)).h(px(13.0)).flex_none().bg(rgb(theme.text)))
                }),
        )
    }

    fn filter_row_action(
        &self,
        path: &[usize],
        action: FilterAction,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let path = path.to_vec();
        let key = filter_path_key(&path);
        let (icon, color, id) = match action {
            FilterAction::AddCondition => (
                "icons/plus.svg",
                theme.icon_backups,
                format!("filter-add-condition-{key}"),
            ),
            FilterAction::AddGroup => (
                "icons/plus.svg",
                theme.icon_backups,
                format!("filter-add-group-{key}"),
            ),
            FilterAction::Remove => (
                "icons/minus.svg",
                theme.danger,
                format!("filter-remove-{key}"),
            ),
        };
        ui::icon_button(
            SharedString::from(id),
            icon,
            color,
            18.0,
            18.0,
            false,
            theme,
            cx.listener(move |this, _event, _window, cx| match action {
                FilterAction::AddCondition => this.filter_add_condition(path.clone(), cx),
                FilterAction::AddGroup => this.filter_add_group(path.clone(), cx),
                FilterAction::Remove => this.filter_remove_node(path.clone(), cx),
            }),
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
                        false,
                        |_, _, _| {},
                    ))
                    .child(self.grid_icon_button(
                        "grid-delete",
                        "icons/minus.svg",
                        theme.danger,
                        self.state.sql.is_none()
                            && self.state.selection.is_some()
                            && !self.state.rows.is_empty(),
                        cx.listener(|this, _event, _window, cx| this.open_delete_confirm(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-commit",
                        "icons/check.svg",
                        theme.icon_connection,
                        !self.state.edits.is_empty(),
                        cx.listener(|this, _event, _window, cx| this.commit_edits(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-rollback",
                        "icons/cross.svg",
                        theme.danger,
                        !self.state.edits.is_empty(),
                        cx.listener(|this, _event, _window, cx| this.cancel_edits(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-refresh",
                        "icons/refresh.svg",
                        active,
                        true,
                        cx.listener(|this, _event, _window, cx| this.refresh(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-stop",
                        "icons/stop.svg",
                        muted,
                        false,
                        |_, _, _| {},
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
            .on_click(on_click)
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

    pub(super) fn render_grid_status(&self) -> impl IntoElement {
        let theme = self.theme;
        let total = self.state.total_rows.unwrap_or(0);
        let end = self
            .state
            .page_index
            .saturating_mul(self.state.page_size)
            .saturating_add(self.state.rows.len() as u64);
        let info = t!(
            "grid.page_info",
            end = end,
            total = total,
            page = self.state.page_index + 1
        )
        .to_string();
        let timing = self.state.elapsed.map(|elapsed| {
            t!(
                "grid.query_time",
                seconds = format!("{:.3}", elapsed.as_secs_f64())
            )
            .to_string()
        });
        let message = match self.state.selection {
            Some(selection) => {
                let (start_row, end_row) = selection.rows();
                let (start_col, end_col) = selection.cols();
                t!(
                    "grid.selection_info",
                    rows = end_row - start_row + 1,
                    cols = end_col - start_col + 1
                )
                .to_string()
            }
            None => self.state.sql(),
        };

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
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(message),
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
