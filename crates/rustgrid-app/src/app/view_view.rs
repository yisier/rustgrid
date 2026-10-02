//! Rendering for the view designer: the 保存/预览/解释/视图创建工具/美化SQL toolbar, the
//! 定义/高级/SQL 预览 sub-tabs, and their bodies. The 定义 tab reuses the SQL editor.

use super::*;

impl AppView {
    /// The view designer's full view, shown instead of the ordinary query view when the active
    /// tab carries view state.
    pub(super) fn render_view_view(
        &mut self,
        query_index: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        // Snapshot the fields needed, so the `queries` borrow ends before `render_query_editor`
        // (which needs `&mut self`).
        let (saving, original_name, explain_running, active_tab, explain_open, has_connection) = {
            let Some(query) = self.queries.get(query_index) else {
                return div().into_any_element();
            };
            let Some(view) = query.view.as_ref() else {
                return div().into_any_element();
            };
            (
                view.saving,
                view.original_name.is_some(),
                view.explain_running,
                view.tab,
                view.explain_open,
                query
                    .connection_index
                    .and_then(|index| self.connection_arc(index))
                    .is_some(),
            )
        };
        let can_preview = has_connection && original_name && !saving;

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
                "view-save",
                "icons/save.svg",
                t!("view.save").to_string(),
                theme.text,
                theme.text,
                has_connection && !saving,
                cx.listener(|this, _event, _window, cx| this.save_view(cx)),
            ))
            .child(self.query_tool_button(
                "view-preview",
                "icons/views.svg",
                t!("view.preview").to_string(),
                theme.text,
                theme.icon_view,
                can_preview,
                cx.listener(|this, _event, _window, cx| this.preview_view(cx)),
            ))
            .child(self.query_tool_button(
                "view-explain",
                "icons/explain.svg",
                t!("view.explain").to_string(),
                theme.text,
                theme.text,
                has_connection && !saving && !explain_running,
                cx.listener(|this, _event, _window, cx| this.explain_view(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(self.query_tool_button(
                "view-builder",
                "icons/query_builder.svg",
                t!("view.builder").to_string(),
                theme.text_muted,
                theme.text_muted,
                false,
                |_, _, _| {},
            ))
            .child(toolbar_separator(theme))
            .child(self.query_tool_button(
                "view-format",
                "icons/format_sql.svg",
                t!("view.format").to_string(),
                theme.text,
                theme.text,
                true,
                cx.listener(|this, _event, _window, cx| this.format_query(cx)),
            ));

        let mut tabs = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .px_2()
            .h(px(26.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border));
        for tab in ViewTab::ALL {
            tabs = tabs.child(self.view_sub_tab(tab, active_tab == tab, cx));
        }

        let body: AnyElement = match active_tab {
            ViewTab::Definition => self.render_query_editor(query_index, cx),
            ViewTab::Advanced => {
                let advanced = self
                    .queries
                    .get(query_index)
                    .and_then(|query| query.view.as_ref())
                    .cloned();
                match advanced {
                    Some(view) => self.render_view_advanced(&view).into_any_element(),
                    None => div().into_any_element(),
                }
            }
            ViewTab::Sql => self.render_view_preview(query_index, cx),
        };

        let mut root = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .child(toolbar)
            .child(tabs)
            .child(body);
        if explain_open {
            root = root.child(self.render_view_explain_panel(query_index, cx));
        }
        root.into_any_element()
    }

    /// The designer's bottom 信息/解释 panel: the executed `EXPLAIN` statement and its status on
    /// the 信息 tab, the plan table on the 解释 tab.
    fn render_view_explain_panel(
        &self,
        query_index: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(query) = self.queries.get(query_index) else {
            return div().into_any_element();
        };
        let Some(view) = query.view.as_ref() else {
            return div().into_any_element();
        };
        let mut tabs = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .px_2()
            .h(px(24.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_b_1()
            .border_color(rgb(theme.border));
        for tab in [ViewExplainTab::Info, ViewExplainTab::Result] {
            tabs = tabs.child(self.view_explain_tab_button(tab, view.explain_tab == tab, cx));
        }

        let body: AnyElement = match view.explain_tab {
            ViewExplainTab::Info => self.render_view_explain_info(view),
            ViewExplainTab::Result => self.render_view_explain_result(query, view, cx),
        };

        div()
            .flex()
            .flex_col()
            .flex_none()
            .h(px(260.0))
            .min_h(px(0.0))
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(tabs)
            .child(body)
            .into_any_element()
    }

    /// One 信息/解释 tab button of the explain panel.
    fn view_explain_tab_button(
        &self,
        tab: ViewExplainTab,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id(tab.id())
            .flex()
            .items_center()
            .justify_center()
            .px_3()
            .h(px(20.0))
            .rounded_sm()
            .text_size(px(12.0))
            .cursor_pointer()
            .when(active, move |style| {
                style
                    .bg(rgb(theme.brand_muted))
                    .text_color(rgb(theme.brand))
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!active, move |style| {
                style
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(
                cx.listener(move |this, _event, _window, cx| this.select_view_explain_tab(tab, cx)),
            )
            .child(t!(tab.label_key()).to_string())
    }

    /// The 信息 tab: the executed `EXPLAIN` statement plus its status, like Navicat's log pane.
    fn render_view_explain_info(&self, view: &ViewTabState) -> AnyElement {
        let theme = self.theme;
        let mut log = div()
            .flex()
            .flex_col()
            .p_2()
            .font_family("Consolas")
            .text_size(px(12.0))
            .line_height(px(16.0))
            .text_color(rgb(theme.text))
            .child(view.explain_sql.clone());
        if view.explain_running {
            log = log.child(
                div()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("view.explain.running").to_string()),
            );
        } else {
            if let Some(error) = view.explain_error.as_ref() {
                log = log.child(
                    div()
                        .text_color(rgb(theme.danger))
                        .child(format!("> {error}")),
                );
            } else {
                log = log.child(div().child(format!("> {}", t!("view.explain.ok"))));
            }
            if let Some(elapsed) = view.explain_elapsed {
                log = log.child(
                    div().child(
                        t!(
                            "view.explain.time",
                            seconds = format!("{:.3}", elapsed.as_secs_f64())
                        )
                        .to_string(),
                    ),
                );
            }
        }
        div()
            .id("view-explain-info")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .bg(rgb(theme.editor_bg))
            .child(log)
            .into_any_element()
    }

    /// The 解释 tab: the plan table, reusing the ordinary result grid.
    fn render_view_explain_result(
        &self,
        _query: &QueryTab,
        view: &ViewTabState,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        if view.explain_running {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .bg(rgb(theme.editor_bg))
                .text_color(rgb(theme.text_muted))
                .child(t!("common.loading").to_string())
                .into_any_element();
        }
        if let Some(error) = view.explain_error.as_ref() {
            return div()
                .p_3()
                .bg(rgb(theme.editor_bg))
                .text_color(rgb(theme.danger))
                .child(error.clone())
                .into_any_element();
        }
        let grid = view.explain_grid_id.and_then(|id| {
            self.grids
                .iter()
                .find(|grid| grid.read(cx).state.id == id)
                .cloned()
        });
        match grid {
            Some(grid) => grid.into_any_element(),
            None => div()
                .p_3()
                .bg(rgb(theme.editor_bg))
                .text_color(rgb(theme.text_muted))
                .child(t!("common.empty").to_string())
                .into_any_element(),
        }
    }

    /// One 定义/高级/SQL 预览 sub-tab button.
    fn view_sub_tab(
        &self,
        tab: ViewTab,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id(tab.id())
            .flex()
            .items_center()
            .justify_center()
            .px_3()
            .h(px(22.0))
            .rounded_sm()
            .text_size(px(12.0))
            .cursor_pointer()
            .when(active, move |style| {
                style
                    .bg(rgb(theme.brand_muted))
                    .text_color(rgb(theme.brand))
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!active, move |style| {
                style
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| this.select_view_tab(tab, cx)))
            .child(t!(tab.label_key()).to_string())
    }

    /// The 高级 tab: the view's creation settings and metadata.
    fn render_view_advanced(&self, view: &ViewTabState) -> impl IntoElement {
        let theme = self.theme;
        let mut rows = div().flex().flex_col().gap_1().p_3().text_size(px(12.0));

        let Some(details) = view.details.as_ref() else {
            return rows.child(
                div()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("view.not_saved").to_string()),
            );
        };
        let info = &details.info;
        let row = |label: &str, value: String| {
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .w(px(120.0))
                        .flex_none()
                        .text_color(rgb(theme.text_muted))
                        .child(label.to_string()),
                )
                .child(div().flex_1().min_w(px(0.0)).child(value))
        };
        rows = rows
            .child(row(&t!("view.field.name"), info.name.clone()))
            .child(row(&t!("view.field.definer"), info.definer.clone()))
            .child(row(&t!("view.field.security"), info.security_type.clone()))
            .child(row(&t!("view.field.algorithm"), info.algorithm.clone()))
            .child(row(
                &t!("view.field.check_option"),
                info.check_option.clone(),
            ))
            .child(row(
                &t!("view.field.updatable"),
                if info.updatable {
                    t!("common.yes").to_string()
                } else {
                    t!("common.no").to_string()
                },
            ))
            .child(row(
                &t!("view.field.created"),
                info.created.clone().unwrap_or_default(),
            ))
            .child(row(
                &t!("view.field.modified"),
                info.modified.clone().unwrap_or_default(),
            ))
            .child(row(
                &t!("view.field.charset"),
                details.character_set_client.clone(),
            ))
            .child(row(
                &t!("view.field.collation"),
                details.collation_connection.clone(),
            ));
        rows
    }

    /// The SQL 预览 tab: the script the Save button runs, read-only.
    fn render_view_preview(
        &mut self,
        query_index: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let Some(sql) = self.view_preview_sql() else {
            return div().into_any_element();
        };
        let key = format!("view-{query_index}");
        self.render_sql_preview(&key, &sql, cx)
    }
}
