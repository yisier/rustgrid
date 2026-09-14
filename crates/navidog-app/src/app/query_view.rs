use super::*;

impl AppView {
    pub(super) fn render_query_view(
        &self,
        query: &QueryTab,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .px_1()
            .py_0p5()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.query_tool_button(
                "query-save",
                "icons/save.svg",
                t!("query.save").to_string(),
                theme.text,
                theme.text,
                false,
                |_, _, _| {},
            ))
            .child(toolbar_separator(theme))
            .child(self.query_tool_button(
                "query-builder",
                "icons/query_builder.svg",
                t!("query.builder").to_string(),
                theme.text,
                theme.text,
                false,
                |_, _, _| {},
            ))
            .child(self.query_tool_button(
                "query-format",
                "icons/format_sql.svg",
                t!("query.format").to_string(),
                theme.text,
                theme.text,
                true,
                cx.listener(|this, _event, _window, cx| this.format_query(cx)),
            ))
            .child(self.query_tool_button(
                "query-snippets",
                "icons/snippets.svg",
                t!("query.snippets").to_string(),
                theme.text,
                theme.text,
                false,
                |_, _, _| {},
            ));

        let connection_label = query
            .connection_index
            .and_then(|index| self.connections.get(index))
            .map(|node| node.profile.name.clone())
            .unwrap_or_else(|| t!("query.not_connected").to_string());
        let database_label = query
            .database
            .clone()
            .unwrap_or_else(|| t!("database.name").to_string());
        let connection_options = self.query_connection_options();
        let database_options = query
            .connection_index
            .map(|index| self.query_database_options(index))
            .unwrap_or_default();
        let has_connection = query
            .connection_index
            .and_then(|index| self.connection_arc(index))
            .is_some();
        let run_enabled = has_connection && !query.running;

        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py_0p5()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.query_combo(
                QueryCombo::Connection,
                "icons/connection.svg",
                theme.icon_connection,
                &connection_label,
                &connection_options,
                true,
                cx,
            ))
            .child(self.query_combo(
                QueryCombo::Database,
                "icons/database.svg",
                theme.icon_database,
                &database_label,
                &database_options,
                query.connection_index.is_some(),
                cx,
            ))
            .child(div().w(px(10.0)).flex_none())
            .child(self.query_tool_button(
                "query-run",
                "icons/run.svg",
                t!("query.run").to_string(),
                theme.text,
                theme.icon_connection,
                run_enabled,
                cx.listener(|this, _event, _window, cx| this.run_query(false, false, cx)),
            ))
            .child(self.query_tool_button(
                "query-run-selected",
                "icons/run.svg",
                t!("query.run_selected").to_string(),
                theme.text,
                theme.icon_connection,
                run_enabled,
                cx.listener(|this, _event, _window, cx| this.run_query(false, true, cx)),
            ))
            .child(self.query_tool_button(
                "query-stop",
                "icons/stop.svg",
                t!("query.stop").to_string(),
                theme.text_muted,
                theme.danger,
                query.running,
                cx.listener(|this, _event, _window, cx| this.stop_query(cx)),
            ))
            .child(self.query_tool_button(
                "query-explain",
                "icons/explain.svg",
                t!("query.explain").to_string(),
                theme.text,
                theme.text,
                run_enabled,
                cx.listener(|this, _event, _window, cx| this.run_query(true, false, cx)),
            ));

        let editor = self.render_query_editor(query, cx).into_any_element();
        let result_grid = query
            .grid_id
            .and_then(|id| self.grids.iter().find(|grid| grid.id == id));
        let has_result_panel = result_grid.is_some() || !matches!(query.result, Loadable::Idle);
        let body: AnyElement = if !has_result_panel {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(editor)
                .into_any_element()
        } else {
            let result_body: AnyElement = match result_grid {
                Some(grid) => self.render_grid(grid, window, cx).into_any_element(),
                None => self.render_query_result(query, cx),
            };
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(editor),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .border_t_1()
                        .border_color(rgb(theme.border))
                        .bg(rgb(theme.editor_bg))
                        .child(result_body),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(toolbar)
            .child(controls)
            .child(body)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn query_tool_button(
        &self,
        id: &'static str,
        icon: &'static str,
        label: String,
        color: u32,
        icon_color: u32,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme;
        div()
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
                    .text_color(rgb(icon_color)),
            )
            .child(label)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn query_combo(
        &self,
        kind: QueryCombo,
        icon: &'static str,
        icon_color: u32,
        selected: &str,
        options: &[(String, String)],
        enabled: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let open = self.query_combo == Some(kind);

        let mut list = div()
            .id(SharedString::from(format!("query-combo-list-{kind:?}")))
            .absolute()
            .top(px(25.0))
            .left_0()
            .w(px(240.0))
            .flex()
            .flex_col()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .h(px((options.len().min(10) as f32) * 22.0 + 4.0))
            .overflow_y_scroll();
        for (value, label) in options {
            let is_selected = value == selected;
            let value = value.clone();
            let label = label.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("query-combo-{kind:?}-{value}")))
                    .flex()
                    .items_center()
                    .h(px(22.0))
                    .px_2()
                    .flex_none()
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(is_selected, move |style| {
                        style
                            .bg(rgb(theme.tree_selected_bg))
                            .text_color(rgb(theme.tree_selected_text))
                    })
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.query_select_combo(kind, value.clone(), cx);
                    }))
                    .child(label),
            );
        }

        let text_color = if enabled {
            theme.text
        } else {
            theme.text_muted
        };
        let combo = ui::text_field(theme)
            .id(SharedString::from(format!("query-combo-btn-{kind:?}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w(px(240.0))
            .h(px(24.0))
            .px_2()
            .text_size(px(12.0))
            .text_color(rgb(text_color))
            .when(enabled, move |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.border_color(rgb(theme.button_default_border)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if !enabled {
                    return;
                }
                this.query_combo = if this.query_combo == Some(kind) {
                    None
                } else {
                    Some(kind)
                };
                cx.notify();
            }))
            .child(
                svg()
                    .path(icon)
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_none()
                    .text_color(rgb(icon_color)),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(selected.to_string()),
            )
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );

        div().relative().child(combo).when(open, move |style| {
            style.child(deferred(list).with_priority(10))
        })
    }
}
