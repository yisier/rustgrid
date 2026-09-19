use super::*;

impl AppView {
    pub(super) fn render_content(
        &self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let body: AnyElement = if let Some(design) =
            self.active_design.and_then(|index| self.designs.get(index))
        {
            design
                .clone()
                .cached(cached_style(|d| {
                    d.flex().flex_col().flex_1().min_w(px(0.0)).min_h(px(0.0))
                }))
                .into_any_element()
        } else if let Some(query) = self.active_query.and_then(|index| self.queries.get(index)) {
            self.render_query_view(query, window, cx).into_any_element()
        } else if let Some(grid) = self.active_grid.and_then(|index| self.grids.get(index)) {
            grid.clone()
                .cached(cached_style(|d| {
                    d.flex().flex_col().flex_1().min_w(px(0.0)).min_h(px(0.0))
                }))
                .into_any_element()
        } else if self.main_tab == MainTab::Queries {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .overflow_hidden()
                .child(self.render_query_object_toolbar(cx))
                .child(self.render_saved_queries(cx))
                .into_any_element()
        } else if self.main_tab == MainTab::Backups {
            self.render_backups(cx).into_any_element()
        } else if let Some(pane) = self.object_pane.clone() {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .overflow_hidden()
                .child(self.render_object_toolbar(&pane, cx))
                .child(pane.clone())
                .child(self.render_object_status(&pane, cx))
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let has_tabs = self.main_tab != MainTab::Backups
            && (self.object_pane.is_some()
                || !self.grids.is_empty()
                || !self.designs.is_empty()
                || !self.queries.is_empty()
                || self.main_tab == MainTab::Queries);
        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .bg(rgb(theme.editor_bg));
        if has_tabs {
            content = content.child(self.tab_bar.clone().cached(cached_style(|d| {
                d.flex().flex_row().h(px(30.0)).flex_none().w_full()
            })));
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
        let max = f32::from(self.scroll.max_offset().x);
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
        name: Option<String>,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let title = match name {
            Some(name) => format!("{name} - {}", t!("common.query")),
            None => format!("{} - {}", t!("query.untitled"), t!("common.query")),
        };
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

    #[allow(clippy::too_many_arguments)]
    fn design_tab(
        &self,
        theme: Theme,
        index: usize,
        id: u64,
        title: String,
        is_view: bool,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let icon_color = if is_view {
            theme.icon_view
        } else {
            theme.icon_table
        };
        let tab_id = SharedString::from(format!("design-tab-{id}"));
        let close_id = SharedString::from(format!("design-tab-close-{id}"));
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
                    if index < app.designs.len() {
                        app.activate_design(Some(index), cx);
                    }
                });
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_this, event: &MouseDownEvent, _window, cx| {
                    let _ = menu.update(cx, |app, cx| {
                        app.context_menu = None;
                        app.tab_menu = Some(TabMenu {
                            target: TabTarget::Design(index),
                            position: event.position,
                        });
                        cx.notify();
                    });
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |_this, _event, _window, cx| {
                    let _ = middle.update(cx, |app, cx| app.close_design(index, cx));
                }),
            )
            .child(tree_icon("icons/design_table.svg", icon_color))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .max_w(px(160.0))
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
                        let _ = close.update(cx, |app, cx| app.close_design(index, cx));
                    }))
                    .child("✕"),
            )
    }
}

impl Render for TabBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (theme, active_grid, active_query, active_design, grid_tabs, query_tabs, design_tabs) = {
            let Some(app) = self.app.upgrade() else {
                return div().into_any_element();
            };
            let app = app.read(cx);
            let grid_tabs: Vec<(usize, u64, String, bool)> = app
                .grids
                .iter()
                .enumerate()
                .filter_map(|(index, entity)| {
                    let grid = entity.read(cx);
                    if grid.state.sql.is_some() {
                        return None;
                    }
                    let kind = t!(if grid.state.is_view {
                        "common.view"
                    } else {
                        "common.table"
                    })
                    .to_string();
                    let title = format!(
                        "{} @{} ({}) - {}",
                        grid.state.table, grid.state.database, grid.state.connection_name, kind
                    );
                    Some((index, grid.state.id, title, grid.state.is_view))
                })
                .collect();
            let query_tabs: Vec<(usize, u64, Option<String>)> = app
                .queries
                .iter()
                .enumerate()
                .map(|(index, query)| (index, query.id, query.name.clone()))
                .collect();
            let design_tabs: Vec<(usize, u64, String, bool)> = app
                .designs
                .iter()
                .enumerate()
                .map(|(index, entity)| {
                    let design = entity.read(cx);
                    let kind = t!(if design.is_view {
                        "common.view"
                    } else {
                        "common.table"
                    })
                    .to_string();
                    let name = if design.is_new && design.table.is_empty() {
                        t!("object.new_table").to_string()
                    } else {
                        design.table.clone()
                    };
                    let title = if design.dirty {
                        format!(
                            "{} @{} ({}) - {} *",
                            name, design.database, design.connection_name, kind
                        )
                    } else {
                        format!(
                            "{} @{} ({}) - {}",
                            name, design.database, design.connection_name, kind
                        )
                    };
                    (index, design.id, title, design.is_view)
                })
                .collect();
            (
                app.theme,
                app.active_grid,
                app.active_query,
                app.active_design,
                grid_tabs,
                query_tabs,
                design_tabs,
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
        for (index, id, title, is_view) in design_tabs {
            strip = strip.child(self.design_tab(
                theme,
                index,
                id,
                title,
                is_view,
                active_design == Some(index),
                cx,
            ));
        }
        for (index, id, name) in query_tabs {
            strip = strip.child(self.query_tab(
                theme,
                index,
                id,
                name,
                active_query == Some(index),
                cx,
            ));
        }

        // The object tab is pinned; the arrows only appear (and wrap the scrollable tab strip)
        // when the open tabs no longer fit.
        let needs_scroll = self.scroll.max_offset().x > px(0.0);
        let mut bar = div()
            .flex()
            .flex_row()
            .items_end()
            .gap_1()
            .h(px(30.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .child(self.object_tab(
                theme,
                active_grid.is_none() && active_query.is_none() && active_design.is_none(),
                cx,
            ));
        if needs_scroll {
            bar = bar.child(self.scroll_button("tab-scroll-left", true, cx));
        }
        bar = bar.child(strip);
        if needs_scroll {
            bar = bar.child(self.scroll_button("tab-scroll-right", false, cx));
        }
        bar.into_any_element()
    }
}
