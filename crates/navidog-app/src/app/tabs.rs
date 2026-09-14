use super::*;

impl AppView {
    pub(super) fn render_content(
        &self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let body: AnyElement =
            if let Some(query) = self.active_query.and_then(|index| self.queries.get(index)) {
                self.render_query_view(query, window, cx).into_any_element()
            } else if let Some(grid) = self.active_grid.and_then(|index| self.grids.get(index)) {
                self.render_grid(grid, window, cx).into_any_element()
            } else if let Some(pane) = self.object_pane.clone() {
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_object_toolbar(&pane, cx))
                    .child(pane)
                    .into_any_element()
            } else if self.main_tab == MainTab::Queries {
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_query_object_toolbar(cx))
                    .child(div().flex_1())
                    .into_any_element()
            } else {
                div().into_any_element()
            };

        let has_tabs = self.object_pane.is_some()
            || !self.grids.is_empty()
            || !self.queries.is_empty()
            || self.main_tab == MainTab::Queries;
        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .bg(rgb(theme.editor_bg));
        if has_tabs {
            content = content.child(self.tab_bar.clone());
        }
        content.child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .overflow_hidden()
                .child(body),
        )
    }
}

impl TabBar {
    pub(super) fn new(app: WeakEntity<AppView>) -> Self {
        Self {
            app,
            scroll: ScrollHandle::new(),
        }
    }

    fn theme(&self, cx: &Context<'_, Self>) -> Theme {
        self.app
            .upgrade()
            .map(|app| app.read(cx).theme)
            .unwrap_or_else(Theme::dark)
    }

    fn scroll_tabs(&mut self, delta: f32, cx: &mut Context<'_, Self>) {
        let max = f32::from(self.scroll.max_offset().width);
        if max <= 0.0 {
            return;
        }
        let current = -f32::from(self.scroll.offset().x);
        let next = (current + delta).clamp(0.0, max);
        let y = self.scroll.offset().y;
        self.scroll.set_offset(Point::new(px(-next), y));
        cx.notify();
    }

    fn scroll_button(
        &self,
        id: &'static str,
        left: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme(cx);
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(18.0))
            .h_full()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .text_color(rgb(theme.text_muted))
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.scroll_tabs(if left { -180.0 } else { 180.0 }, cx);
            }))
            .child(
                svg()
                    .path(if left {
                        "icons/tab-prev.svg"
                    } else {
                        "icons/tab-next.svg"
                    })
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            )
    }

    fn object_tab(
        &self,
        theme: Theme,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let app = self.app.clone();
        div()
            .id("tab-object")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h_full()
            .px_2()
            .flex_none()
            .cursor_pointer()
            .text_size(px(12.0))
            .when(active, |style| {
                style
                    .bg(rgb(theme.editor_bg))
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
            })
            .when(!active, |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                let _ = app.update(cx, |app, cx| app.activate_grid(None, cx));
            }))
            .child(tree_icon("icons/tables.svg", theme.icon_tables))
            .child(t!("object.header").to_string())
    }

    fn query_tab(
        &self,
        theme: Theme,
        index: usize,
        id: u64,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let title = format!("{} - {}", t!("query.untitled"), t!("common.query"));
        let tab_id = SharedString::from(format!("query-tab-{id}"));
        let close_id = SharedString::from(format!("query-tab-close-{id}"));
        let activate = self.app.clone();
        let close = self.app.clone();
        let menu = self.app.clone();
        let middle = self.app.clone();

        div()
            .id(tab_id)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h_full()
            .px_2()
            .flex_none()
            .cursor_pointer()
            .text_size(px(12.0))
            .when(active, |style| {
                style
                    .bg(rgb(theme.editor_bg))
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
            })
            .when(!active, |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                let _ = activate.update(cx, |app, cx| {
                    if index < app.queries.len() {
                        app.activate_query(index, cx);
                    }
                });
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_this, event: &MouseDownEvent, _window, cx| {
                    let _ = menu.update(cx, |app, cx| {
                        app.context_menu = None;
                        app.tab_menu = Some(TabMenu {
                            target: TabTarget::Query(index),
                            position: event.position,
                        });
                        cx.notify();
                    });
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |_this, _event, _window, cx| {
                    let _ = middle.update(cx, |app, cx| app.close_query(index, cx));
                }),
            )
            .child(tree_icon("icons/queries.svg", theme.icon_queries))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .max_w(px(110.0))
                    .child(title),
            )
            .child(
                div()
                    .id(close_id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(16.0))
                    .h(px(16.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                    .on_click(cx.listener(move |_this, _event, _window, cx| {
                        cx.stop_propagation();
                        let _ = close.update(cx, |app, cx| app.close_query(index, cx));
                    }))
                    .child("✕"),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn table_tab(
        &self,
        theme: Theme,
        index: usize,
        id: u64,
        title: String,
        is_view: bool,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let icon = if is_view {
            "icons/views.svg"
        } else {
            "icons/tables.svg"
        };
        let icon_color = if is_view {
            theme.icon_view
        } else {
            theme.icon_table
        };
        let tab_id = SharedString::from(format!("tab-{id}"));
        let close_id = SharedString::from(format!("tab-close-{id}"));
        let activate = self.app.clone();
        let close = self.app.clone();
        let menu = self.app.clone();
        let middle = self.app.clone();

        div()
            .id(tab_id)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h_full()
            .px_2()
            .flex_none()
            .cursor_pointer()
            .text_size(px(12.0))
            .when(active, |style| {
                style
                    .bg(rgb(theme.editor_bg))
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
            })
            .when(!active, |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                let _ = activate.update(cx, |app, cx| {
                    if index < app.grids.len() {
                        app.activate_grid(Some(index), cx);
                    }
                });
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_this, event: &MouseDownEvent, _window, cx| {
                    let _ = menu.update(cx, |app, cx| {
                        app.context_menu = None;
                        app.tab_menu = Some(TabMenu {
                            target: TabTarget::Grid(index),
                            position: event.position,
                        });
                        cx.notify();
                    });
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |_this, _event, _window, cx| {
                    let _ = middle.update(cx, |app, cx| app.close_grid(index, cx));
                }),
            )
            .child(tree_icon(icon, icon_color))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .max_w(px(110.0))
                    .child(title),
            )
            .child(
                div()
                    .id(close_id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(16.0))
                    .h(px(16.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                    .on_click(cx.listener(move |_this, _event, _window, cx| {
                        cx.stop_propagation();
                        let _ = close.update(cx, |app, cx| app.close_grid(index, cx));
                    }))
                    .child("✕"),
            )
    }
}

impl Render for TabBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (theme, active_grid, active_query, grid_tabs, query_tabs) = {
            let Some(app) = self.app.upgrade() else {
                return div().into_any_element();
            };
            let app = app.read(cx);
            let grid_tabs: Vec<(usize, u64, String, bool)> = app
                .grids
                .iter()
                .enumerate()
                .filter(|(_, grid)| grid.sql.is_none())
                .map(|(index, grid)| {
                    let kind = t!(if grid.is_view {
                        "common.view"
                    } else {
                        "common.table"
                    })
                    .to_string();
                    let title = format!(
                        "{} @{} ({}) - {}",
                        grid.table, grid.database, grid.connection_name, kind
                    );
                    (index, grid.id, title, grid.is_view)
                })
                .collect();
            let query_tabs: Vec<(usize, u64)> = app
                .queries
                .iter()
                .enumerate()
                .map(|(index, query)| (index, query.id))
                .collect();
            (
                app.theme,
                app.active_grid,
                app.active_query,
                grid_tabs,
                query_tabs,
            )
        };

        let mut strip = div()
            .id("tab-strip")
            .flex()
            .flex_row()
            .items_end()
            .gap_1()
            .pt_1()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .overflow_x_scroll()
            .track_scroll(&self.scroll);

        strip = strip.child(self.object_tab(
            theme,
            active_grid.is_none() && active_query.is_none(),
            cx,
        ));
        for (index, id, title, is_view) in grid_tabs {
            strip = strip.child(self.table_tab(
                theme,
                index,
                id,
                title,
                is_view,
                active_grid == Some(index),
                cx,
            ));
        }
        for (index, id) in query_tabs {
            strip = strip.child(self.query_tab(theme, index, id, active_query == Some(index), cx));
        }

        div()
            .flex()
            .flex_row()
            .items_end()
            .h(px(30.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .child(self.scroll_button("tab-scroll-left", true, cx))
            .child(strip)
            .child(self.scroll_button("tab-scroll-right", false, cx))
            .into_any_element()
    }
}
