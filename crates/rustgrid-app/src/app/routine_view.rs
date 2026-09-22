//! Rendering for the stored-routine editor: the 保存/运行/停止/查找/自动换行 toolbar, the
//! 定义/信息/SQL 预览 sub-tabs, and their bodies. The 定义 tab reuses the SQL editor.

use super::*;

impl AppView {
    /// The routine editor's full view, shown instead of the ordinary query view when the active
    /// tab carries routine state.
    pub(super) fn render_routine_view(
        &self,
        query: &QueryTab,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(routine) = query.routine.as_ref() else {
            return div().into_any_element();
        };
        let has_connection = query
            .connection_index
            .and_then(|index| self.connection_arc(index))
            .is_some();
        let running = query.running;
        let saving = routine.saving;

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
                "routine-save",
                "icons/save.svg",
                t!("routine.save").to_string(),
                theme.text,
                theme.text,
                has_connection && !saving,
                cx.listener(|this, _event, _window, cx| this.save_routine(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(self.query_tool_button(
                "routine-run",
                "icons/run.svg",
                t!("routine.run").to_string(),
                theme.text,
                theme.icon_connection,
                has_connection && !running,
                cx.listener(|this, _event, _window, cx| this.run_routine(cx)),
            ))
            .child(self.query_tool_button(
                "routine-stop",
                "icons/stop.svg",
                t!("query.stop").to_string(),
                theme.text_muted,
                theme.danger,
                running,
                cx.listener(|this, _event, _window, cx| this.stop_query(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(self.query_tool_button(
                "routine-find",
                "icons/search.svg",
                t!("routine.find").to_string(),
                theme.text,
                theme.text,
                true,
                cx.listener(|this, _event, _window, cx| this.toggle_routine_find(cx)),
            ))
            .child(self.query_tool_button(
                "routine-wrap",
                "icons/text.svg",
                t!("routine.word_wrap").to_string(),
                if routine.word_wrap {
                    theme.brand
                } else {
                    theme.text
                },
                if routine.word_wrap {
                    theme.brand
                } else {
                    theme.text
                },
                true,
                cx.listener(|this, _event, _window, cx| this.toggle_routine_word_wrap(cx)),
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
        for tab in RoutineTab::ALL {
            tabs = tabs.child(self.routine_sub_tab(tab, routine.tab == tab, cx));
        }

        let find_bar: AnyElement = if routine.find_open {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .flex_none()
                .bg(rgb(theme.toolbar_bg))
                .border_b_1()
                .border_color(rgb(theme.border))
                .child(
                    div()
                        .w(px(240.0))
                        .h(px(24.0))
                        .child(self.routine_find_input.clone()),
                )
                .child(self.query_tool_button(
                    "routine-find-next",
                    "icons/next.svg",
                    t!("routine.find_next").to_string(),
                    theme.text,
                    theme.text,
                    true,
                    cx.listener(|this, _event, _window, cx| this.routine_find_next(cx)),
                ))
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let body: AnyElement = match routine.tab {
            RoutineTab::Definition => self.render_query_editor(query, cx).into_any_element(),
            RoutineTab::Info => self.render_routine_info(routine).into_any_element(),
            RoutineTab::Sql => self.render_routine_preview(cx),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .child(toolbar)
            .child(tabs)
            .child(find_bar)
            .child(body)
            .into_any_element()
    }

    /// One 定义/信息/SQL 预览 sub-tab button.
    fn routine_sub_tab(
        &self,
        tab: RoutineTab,
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
            .on_click(
                cx.listener(move |this, _event, _window, cx| this.select_routine_tab(tab, cx)),
            )
            .child(t!(tab.label_key()).to_string())
    }

    /// The 信息 tab: the routine's metadata and creation settings.
    fn render_routine_info(&self, routine: &RoutineTabState) -> impl IntoElement {
        let theme = self.theme;
        let mut rows = div().flex().flex_col().gap_1().p_3().text_size(px(12.0));

        let Some(details) = routine.details.as_ref() else {
            return rows.child(
                div()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("routine.not_saved").to_string()),
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
            .child(row(&t!("routine.field.name"), info.name.clone()))
            .child(row(
                &t!("routine.field.kind"),
                t!(info.kind.label_key()).to_string(),
            ));
        if !info.return_type.is_empty() {
            rows = rows.child(row(&t!("routine.field.returns"), info.return_type.clone()));
        }
        rows = rows
            .child(row(&t!("routine.field.definer"), info.definer.clone()))
            .child(row(
                &t!("routine.field.created"),
                info.created.clone().unwrap_or_default(),
            ))
            .child(row(
                &t!("routine.field.modified"),
                info.modified.clone().unwrap_or_default(),
            ))
            .child(row(
                &t!("routine.field.security"),
                info.security_type.clone(),
            ))
            .child(row(
                &t!("routine.field.data_access"),
                info.data_access.clone(),
            ))
            .child(row(
                &t!("routine.field.deterministic"),
                if info.deterministic {
                    t!("common.yes").to_string()
                } else {
                    t!("common.no").to_string()
                },
            ))
            .child(row(&t!("routine.field.comment"), info.comment.clone()))
            .child(row(&t!("routine.field.sql_mode"), details.sql_mode.clone()))
            .child(row(
                &t!("routine.field.charset"),
                details.character_set_client.clone(),
            ))
            .child(row(
                &t!("routine.field.collation"),
                details.collation_connection.clone(),
            ));
        rows
    }

    /// The SQL 预览 tab: the script the Save button runs, read-only.
    fn render_routine_preview(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(sql) = self.routine_preview_sql() else {
            return div().into_any_element();
        };
        let styled = self.styled_sql(&sql, (0, 0));
        let _ = cx;
        div()
            .id("routine-preview-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .bg(rgb(theme.editor_bg))
            .child(
                div()
                    .p_2()
                    .font_family("Consolas")
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .child(styled),
            )
            .into_any_element()
    }
}
