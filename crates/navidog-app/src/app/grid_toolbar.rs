use super::*;

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

    pub(super) fn render_sort_panel(&self, cx: &mut Context<'_, Self>) -> AnyElement {
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
            for (index, rule) in self.state.sort_draft.iter().enumerate() {
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
                    .child(
                        div()
                            .id("sort-add-row")
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(16.0))
                            .h(px(16.0))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.button_bg))
                            .cursor_pointer()
                            .hover(move |style| {
                                style
                                    .bg(rgb(theme.tree_hover_bg))
                                    .border_color(rgb(theme.button_default_border))
                            })
                            .on_click(
                                cx.listener(|this, _event, _window, cx| this.sort_add_rule(cx)),
                            )
                            .child(
                                svg()
                                    .path("icons/plus.svg")
                                    .w(px(10.0))
                                    .h(px(10.0))
                                    .flex_none()
                                    .text_color(rgb(theme.text)),
                            ),
                    ),
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
        &self,
        index: usize,
        rule: &SortRule,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let selected = self.state.sort_selected == Some(index);
        let rule = rule.clone();
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
            .child(
                ui::text_field(theme)
                    .id(SharedString::from(format!("sort-field-{index}")))
                    .flex()
                    .items_center()
                    .px_2()
                    .h(px(20.0))
                    .w(px(180.0))
                    .flex_none()
                    .text_size(px(12.0))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(move |style| style.border_color(rgb(theme.button_default_border)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.sort_open_combo(index, cx)
                    }))
                    .child(rule.column.clone()),
            )
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
            .child(
                div()
                    .id(SharedString::from(format!("sort-remove-{index}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(18.0))
                    .h(px(18.0))
                    .flex_none()
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.sort_remove_rule(index, cx)
                    }))
                    .child(
                        svg()
                            .path("icons/cross.svg")
                            .w(px(9.0))
                            .h(px(9.0))
                            .flex_none()
                            .text_color(rgb(theme.danger)),
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
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(22.0))
            .h(px(20.0))
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, _event, _window, cx| this.sort_move_rule(delta, cx)))
            .child(
                svg()
                    .path(icon)
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(theme.text)),
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

    pub(super) fn render_filter_panel(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut list = div()
            .id("filter-condition-list")
            .flex()
            .flex_col()
            .flex_none()
            .min_h(px(46.0))
            .max_h(px(184.0))
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
                    .on_click(cx.listener(|this, _event, _window, cx| this.filter_add_rule(0, cx)))
                    .child(sort_plus_badge(theme))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!("grid.filter_hint").to_string()),
                    ),
            );
        } else {
            for (index, condition) in self.state.filter_draft.iter().enumerate() {
                list = list.child(self.render_filter_row(index, condition, cx));
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

    fn render_filter_row(
        &self,
        index: usize,
        condition: &FilterCondition,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let mut row = div()
            .id(SharedString::from(format!("filter-row-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .h(px(26.0))
            .flex_none();

        if index == 0 {
            row = row.child(div().w(px(64.0)).flex_none());
        } else {
            row = row.child(self.filter_conjunction_toggle(index, cx));
        }

        row = row
            .child(
                div()
                    .id(SharedString::from(format!("filter-enabled-{index}")))
                    .flex()
                    .items_center()
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.filter_toggle_enabled(index, cx);
                    }))
                    .child(checkbox_box(condition.enabled, theme)),
            )
            .child(self.render_filter_field_combo(index, cx))
            .child(self.render_filter_operator_combo(index, cx))
            .child(self.render_filter_value(index, 0, cx));

        if condition.operator.needs_second_value() {
            row = row
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("filter.between").to_string()),
                )
                .child(self.render_filter_value(index, 1, cx));
        }

        row = row
            .child(self.filter_row_action(index, false, cx))
            .child(self.filter_row_action(index, true, cx));

        row.into_any_element()
    }

    fn filter_conjunction_toggle(
        &self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let conjunction = self
            .state
            .filter_draft
            .get(index)
            .map(|condition| condition.conjunction)
            .unwrap_or_default();
        let mut toggle = div()
            .id(SharedString::from(format!("filter-conjunction-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .w(px(64.0))
            .flex_none()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.filter_toggle_conjunction(index, cx);
            }));
        for option in [FilterConjunction::And, FilterConjunction::Or] {
            let active = option == conjunction;
            toggle = toggle.child(
                div()
                    .px_1()
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .text_size(px(11.0))
                    .rounded_sm()
                    .when(active, move |style| {
                        style.bg(rgb(theme.primary)).text_color(rgb(0xffffff))
                    })
                    .when(!active, move |style| {
                        style.text_color(rgb(theme.text_muted))
                    })
                    .child(t!(option.label_key()).to_string()),
            );
        }
        toggle
    }

    fn render_filter_field_combo(
        &self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let open = self.state.filter_combo == Some((index, FilterCombo::Field));
        let value = self
            .state
            .filter_draft
            .get(index)
            .map(|condition| condition.column.clone())
            .unwrap_or_default();

        let button = ui::text_field(theme)
            .id(SharedString::from(format!("filter-field-{index}")))
            .flex()
            .items_center()
            .px_2()
            .h(px(20.0))
            .w(px(170.0))
            .flex_none()
            .text_size(px(12.0))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |style| style.border_color(rgb(theme.button_default_border)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.filter_open_combo(index, FilterCombo::Field, cx);
            }))
            .child(value);

        let mut list = div()
            .id(SharedString::from(format!("filter-field-list-{index}")))
            .absolute()
            .top(px(22.0))
            .left_0()
            .w(px(170.0))
            .max_h(px(240.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border));
        for column in &self.state.columns {
            let name = column.name.clone();
            let click = name.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "filter-field-option-{index}-{name}"
                    )))
                    .flex()
                    .items_center()
                    .h(px(20.0))
                    .px_2()
                    .flex_none()
                    .text_size(px(12.0))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.filter_choose_field(index, click.clone(), cx);
                    }))
                    .child(name),
            );
        }

        div().relative().child(button).when(open, move |style| {
            style.child(deferred(list).with_priority(10))
        })
    }

    fn render_filter_operator_combo(
        &self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let open = self.state.filter_combo == Some((index, FilterCombo::Operator));
        let operator = self
            .state
            .filter_draft
            .get(index)
            .map(|condition| condition.operator)
            .unwrap_or(FilterOperator::Equal);

        let button = ui::text_field(theme)
            .id(SharedString::from(format!("filter-operator-{index}")))
            .flex()
            .items_center()
            .px_2()
            .h(px(20.0))
            .w(px(150.0))
            .flex_none()
            .text_size(px(12.0))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |style| style.border_color(rgb(theme.button_default_border)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.filter_open_combo(index, FilterCombo::Operator, cx);
            }))
            .child(t!(operator.label_key()).to_string());

        let mut list = div()
            .id(SharedString::from(format!("filter-operator-list-{index}")))
            .absolute()
            .top(px(22.0))
            .left_0()
            .w(px(150.0))
            .max_h(px(240.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border));
        for option in FilterOperator::ALL {
            let selected = option == operator;
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "filter-operator-option-{index}-{}",
                        option.label_key()
                    )))
                    .flex()
                    .items_center()
                    .h(px(20.0))
                    .px_2()
                    .flex_none()
                    .text_size(px(12.0))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(selected, move |style| {
                        style
                            .bg(rgb(theme.tree_selected_bg))
                            .text_color(rgb(theme.tree_selected_text))
                    })
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.filter_choose_operator(index, option, cx);
                    }))
                    .child(t!(option.label_key()).to_string()),
            );
        }

        div().relative().child(button).when(open, move |style| {
            style.child(deferred(list).with_priority(10))
        })
    }

    fn render_filter_value(
        &self,
        index: usize,
        slot: u8,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let condition = self.state.filter_draft.get(index);
        let operator = condition
            .map(|condition| condition.operator)
            .unwrap_or(FilterOperator::Equal);
        let enabled = if slot == 0 {
            operator.needs_value()
        } else {
            operator.needs_second_value()
        };
        let value = condition
            .map(|condition| {
                if slot == 0 {
                    condition.value.clone()
                } else {
                    condition.value2.clone()
                }
            })
            .unwrap_or_default();
        let focused = enabled && self.filter_active == Some((index, slot));
        let handle = if slot == 0 {
            self.filter_value_focus.get(index).cloned()
        } else {
            self.filter_value2_focus.get(index).cloned()
        };

        let mut field = ui::text_field(theme)
            .id(SharedString::from(format!("filter-value-{index}-{slot}")))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .w(px(200.0))
            .h(px(20.0))
            .flex_none()
            .px_2()
            .text_size(px(12.0))
            .overflow_hidden();
        if enabled {
            if let Some(handle) = handle {
                field = field
                    .track_focus(&handle)
                    .cursor_text()
                    .on_key_down(cx.listener(move |this, event, _window, cx| {
                        this.filter_value_key(index, slot, event, cx);
                    }))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.filter_focus_value(index, slot, window, cx);
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
        index: usize,
        add: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let (icon, color, id) = if add {
            (
                "icons/plus.svg",
                theme.icon_backups,
                SharedString::from(format!("filter-add-{index}")),
            )
        } else {
            (
                "icons/cross.svg",
                theme.danger,
                SharedString::from(format!("filter-remove-{index}")),
            )
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(18.0))
            .h(px(18.0))
            .flex_none()
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if add {
                    this.filter_add_rule(index, cx);
                } else {
                    this.filter_remove_rule(index, cx);
                }
            }))
            .child(
                svg()
                    .path(icon)
                    .w(px(10.0))
                    .h(px(10.0))
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

    pub(super) fn render_sort_combo_popup(
        &self,
        rule_index: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let matches = self.sort_combo_matches();
        let highlight = self
            .sort_combo_highlight
            .min(matches.len().saturating_sub(1));
        let filter = self.sort_combo_filter.clone();

        let search = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(22.0))
            .flex_none()
            .relative()
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                svg()
                    .path("icons/search.svg")
                    .w(px(11.0))
                    .h(px(11.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            )
            .child({
                let mut text = div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(12.0))
                    .text_color(rgb(if filter.is_empty() {
                        theme.text_muted
                    } else {
                        theme.text
                    }));
                text = text.child(if filter.is_empty() {
                    t!("grid.sort_search").to_string()
                } else {
                    filter.clone()
                });
                if self.sort_combo_focused && self.caret() {
                    text = text.child(ui::text_caret(theme));
                }
                text
            })
            .child(self.ime_probe(&self.sort_combo_focus));

        let mut options = div()
            .id("sort-combo-options")
            .flex()
            .flex_col()
            .max_h(px(220.0))
            .overflow_y_scroll();
        for (position, name) in matches.iter().enumerate() {
            let selected = position == highlight;
            let option_name = name.clone();
            let click_name = name.clone();
            options = options.child(
                div()
                    .id(SharedString::from(format!("sort-option-{option_name}")))
                    .flex()
                    .items_center()
                    .h(px(18.0))
                    .px_2()
                    .flex_none()
                    .text_size(px(12.0))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(selected, move |style| {
                        style
                            .bg(rgb(theme.tree_selected_bg))
                            .text_color(rgb(theme.tree_selected_text))
                    })
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.sort_choose_column(rule_index, click_name.clone(), cx)
                    }))
                    .child(option_name),
            );
        }
        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_2()
            .py_1()
            .child(ui::dialog_button(
                "sort-combo-ok",
                t!("form.ok").to_string(),
                true,
                theme,
                cx.listener(|this, _event, _window, cx| this.sort_confirm_combo(cx)),
            ))
            .child(ui::dialog_button(
                "sort-combo-cancel",
                t!("form.cancel").to_string(),
                false,
                theme,
                cx.listener(|this, _event, _window, cx| this.sort_cancel_combo(cx)),
            ));

        let left = 40.0;
        let top = 52.0 + rule_index as f32 * 24.0;
        div()
            .id("sort-combo-popup")
            .track_focus(&self.sort_combo_focus)
            .on_key_down(cx.listener(|this, event, _window, cx| this.sort_combo_key(event, cx)))
            .absolute()
            .occlude()
            .left(px(left))
            .top(px(top))
            .w(px(180.0))
            .flex()
            .flex_col()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.text_muted))
            .shadow(dialog_shadow())
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.sort_cancel_combo(cx);
            }))
            .child(search)
            .child(options)
            .child(div().h(px(1.0)).flex_none().bg(rgb(theme.border)))
            .child(footer)
            .into_any_element()
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
            .justify_center()
            .w(px(52.0))
            .h(px(22.0))
            .text_size(px(12.0))
            .cursor_text()
            .on_key_down(cx.listener(|this, event, _window, cx| this.page_input_key(event, cx)))
            .on_click(cx.listener(|this, _event, window, cx| {
                window.focus(&this.page_input_focus);
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
                    .justify_center()
                    .w(px(64.0))
                    .h(px(20.0))
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
                        window.focus(&this.page_size_focus);
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
