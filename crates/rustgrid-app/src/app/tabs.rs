use super::*;

/// A table/design tab row: `(index, id, title, is_view, is_key, environment colour)`.
type ObjectTabRow = (usize, u64, String, bool, bool, Option<ConnectionColor>);

/// A design tab row: `(index, id, title, is_view, environment colour)`.
type DesignTabRow = (usize, u64, String, bool, Option<ConnectionColor>);

/// A query/editor tab row: `(index, id, title, icon, icon colour, environment colour)`.
type QueryTabRow = (
    usize,
    u64,
    String,
    &'static str,
    u32,
    Option<ConnectionColor>,
);

/// The widest a tab grows before its title is ellipsized; the full title is in the tab's tooltip.
const TAB_MAX_WIDTH: f32 = 170.0;

/// The background fill of a tab (`0xRRGGBBAA`). Every tab of a connection with an environment
/// colour is tinted with it (a stronger shade on the active tab), so the whole tab strip reads as
/// that environment (参考 Navicat); the bar itself stays neutral so tabs never blend into it.
fn tab_background(theme: Theme, color: Option<ConnectionColor>, alpha: u32, fallback: u32) -> u32 {
    match color {
        Some(color) => color.tint(theme, alpha),
        None => (fallback << 8) | 0xff,
    }
}

impl AppView {
    pub(super) fn render_content(
        &mut self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let body: AnyElement =
            if let Some(design) = self.active_design.and_then(|index| self.designs.get(index)) {
                design
                    .clone()
                    .cached(cached_style(|d| {
                        d.flex().flex_col().flex_1().min_w(px(0.0)).min_h(px(0.0))
                    }))
                    .into_any_element()
            } else if let Some(index) = self.active_query {
                self.render_query_view(index, window, cx).into_any_element()
            } else if let Some(grid) = self.active_grid.and_then(|index| self.grids.get(index)) {
                grid.clone()
                    .cached(cached_style(|d| {
                        d.flex().flex_col().flex_1().min_w(px(0.0)).min_h(px(0.0))
                    }))
                    .into_any_element()
            } else if self.main_tab == MainTab::Users {
                self.render_users(cx).into_any_element()
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
            } else if self.main_tab == MainTab::Sync {
                self.render_sync(cx)
            } else if matches!(self.main_tab, MainTab::Functions | MainTab::Views)
                && self.object_pane.is_none()
            {
                div().flex_1().into_any_element()
            } else if let Some(pane) = self.object_pane.clone() {
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_object_toolbar(&pane, cx))
                    .child(pane.clone())
                    .into_any_element()
            } else {
                div().into_any_element()
            };

        // The open-tab strip stays visible on every main tab (including Backup), so the tabs a
        // user already has open do not disappear when they switch views.
        let has_tabs = self.object_pane.is_some()
            || !self.grids.is_empty()
            || !self.designs.is_empty()
            || !self.queries.is_empty()
            || self.main_tab == MainTab::Queries
            || self.main_tab == MainTab::Users;
        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .key_context(PAGE_CONTEXT)
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

    /// F5 / the Shortcuts command: refresh whatever the page is showing. A focused grid keeps its
    /// own `GridRefresh` binding; this path covers the list pages and a grid whose focus sits
    /// elsewhere.
    pub(super) fn refresh_current_page(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.active_grid
            && let Some(grid) = self.grids.get(index).cloned()
        {
            grid.update(cx, |grid, cx| grid.refresh(cx));
            return;
        }
        if self.active_query.is_some() || self.active_design.is_some() {
            return;
        }
        match self.main_tab {
            MainTab::Users => self.refresh_users_feedback(cx),
            MainTab::Queries => self.refresh_query_files_feedback(cx),
            MainTab::Backups => self.refresh_selected_backup(cx),
            MainTab::Sync => self.refresh_transfer_configs(cx),
            MainTab::Tables | MainTab::Views | MainTab::Functions => {
                let Some(pane) = self.object_pane.as_ref() else {
                    return;
                };
                let (connection_index, database_index, category) = {
                    let pane = pane.read(cx);
                    (pane.connection_index, pane.database_index, pane.category)
                };
                self.refresh_object_category(category, connection_index, database_index, cx);
            }
        }
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
        color: Option<ConnectionColor>,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let app = self.app.clone();
        // The pinned Object tab keeps the connection colour while it is the active tab (e.g. on
        // 用户/查询/备份, where it is the only tab), and turns neutral once a file tab takes over.
        let active_bg = match (color, active) {
            (Some(color), true) => rgba(color.tint(theme, 0x85)),
            _ => rgba((theme.editor_bg << 8) | 0xff),
        };
        let hover_bg = if active {
            active_bg
        } else {
            rgba((theme.tree_hover_bg << 8) | 0xff)
        };
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
                    .bg(active_bg)
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
            .hover(move |style| style.bg(hover_bg))
            .on_click(cx.listener(move |_this, _event, _window, cx| {
                let _ = app.update(cx, |app, cx| app.activate_grid(None, cx));
            }))
            .child(tree_icon("icons/tables.svg", theme.icon_tables))
            .child(
                div()
                    .when(active, |style| style.font_weight(FontWeight::SEMIBOLD))
                    .child(t!("object.header").to_string()),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn query_tab(
        &self,
        theme: Theme,
        index: usize,
        id: u64,
        title: String,
        icon: &'static str,
        icon_color: u32,
        active: bool,
        color: Option<ConnectionColor>,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let tab_id = SharedString::from(format!("query-tab-{id}"));
        let close_id = SharedString::from(format!("query-tab-close-{id}"));
        let activate = self.app.clone();
        let close = self.app.clone();
        let menu = self.app.clone();
        let middle = self.app.clone();
        let active_bg = rgba(tab_background(theme, color, 0x85, theme.editor_bg));
        let inactive_bg = rgba(tab_background(theme, color, 0x24, theme.button_bg));
        let hover_bg = if active {
            active_bg
        } else {
            rgba(tab_background(theme, color, 0x4d, theme.tree_hover_bg))
        };

        div()
            .id(tab_id)
            .tooltip(ui::text_tooltip(title.clone()))
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
                    .bg(active_bg)
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
            })
            .when(!active, |style| {
                style
                    .bg(inactive_bg)
                    .border_1()
                    .border_color(rgb(theme.border))
            })
            .hover(move |style| style.bg(hover_bg))
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
            .child(tree_icon(icon, icon_color))
            .child(
                div()
                    .truncate()
                    .max_w(px(TAB_MAX_WIDTH))
                    .when(active, |style| style.font_weight(FontWeight::SEMIBOLD))
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
        is_key: bool,
        color: Option<ConnectionColor>,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let (icon, icon_color) = if is_key {
            ("icons/primary_key.svg", theme.brand)
        } else if is_view {
            ("icons/views.svg", theme.icon_view)
        } else {
            ("icons/tables.svg", theme.icon_table)
        };
        let tab_id = SharedString::from(format!("tab-{id}"));
        let close_id = SharedString::from(format!("tab-close-{id}"));
        let activate = self.app.clone();
        let close = self.app.clone();
        let menu = self.app.clone();
        let middle = self.app.clone();
        let active_bg = rgba(tab_background(theme, color, 0x85, theme.editor_bg));
        let inactive_bg = rgba(tab_background(theme, color, 0x24, theme.button_bg));
        let hover_bg = if active {
            active_bg
        } else {
            rgba(tab_background(theme, color, 0x4d, theme.tree_hover_bg))
        };

        div()
            .id(tab_id)
            .tooltip(ui::text_tooltip(title.clone()))
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
                    .bg(active_bg)
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
            })
            .when(!active, |style| {
                style
                    .bg(inactive_bg)
                    .border_1()
                    .border_color(rgb(theme.border))
            })
            .hover(move |style| style.bg(hover_bg))
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
                    .truncate()
                    .max_w(px(TAB_MAX_WIDTH))
                    .when(active, |style| style.font_weight(FontWeight::SEMIBOLD))
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
        color: Option<ConnectionColor>,
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
        let active_bg = rgba(tab_background(theme, color, 0x85, theme.editor_bg));
        let inactive_bg = rgba(tab_background(theme, color, 0x24, theme.button_bg));
        let hover_bg = if active {
            active_bg
        } else {
            rgba(tab_background(theme, color, 0x4d, theme.tree_hover_bg))
        };

        div()
            .id(tab_id)
            .tooltip(ui::text_tooltip(title.clone()))
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
                    .bg(active_bg)
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
            })
            .when(!active, |style| {
                style
                    .bg(inactive_bg)
                    .border_1()
                    .border_color(rgb(theme.border))
            })
            .hover(move |style| style.bg(hover_bg))
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
                    .truncate()
                    .max_w(px(TAB_MAX_WIDTH))
                    .when(active, |style| style.font_weight(FontWeight::SEMIBOLD))
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
        let (
            theme,
            active_grid,
            active_query,
            active_design,
            grid_tabs,
            query_tabs,
            design_tabs,
            object_color,
        ) = {
            let Some(app) = self.app.upgrade() else {
                return div().into_any_element();
            };
            let app = app.read(cx);
            let grid_tabs: Vec<ObjectTabRow> = app
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
                    let is_key = grid.state.key_status.is_some();
                    let title = if is_key {
                        // A Redis key opens as its own page; the tab is the key name (the full
                        // `key @database (connection)` title would only ellipsize at 170px).
                        grid.state.table.clone()
                    } else {
                        format!(
                            "{} @{} ({}) - {}",
                            grid.state.table, grid.state.database, grid.state.connection_name, kind
                        )
                    };
                    let color = app
                        .connection_index_of(&grid.state.connection)
                        .and_then(|connection| app.connection_color(connection));
                    Some((
                        index,
                        grid.state.id,
                        title,
                        grid.state.is_view,
                        is_key,
                        color,
                    ))
                })
                .collect();
            let query_tabs: Vec<QueryTabRow> = app
                .queries
                .iter()
                .enumerate()
                .map(|(index, query)| {
                    let connection_name = query
                        .connection_index
                        .and_then(|connection| app.connections.get(connection))
                        .map(|node| node.profile.name.clone())
                        .unwrap_or_default();
                    let (icon, icon_color, title) = if let Some(view) = &query.view {
                        (
                            "icons/views.svg",
                            app.theme.icon_views,
                            format!(
                                "{} @{} ({}) - {}",
                                view.name,
                                query.database.clone().unwrap_or_default(),
                                connection_name,
                                t!("common.view")
                            ),
                        )
                    } else if let Some(routine) = &query.routine {
                        (
                            "icons/functions.svg",
                            app.theme.icon_functions,
                            format!(
                                "{} @{} ({}) - {}",
                                routine.name,
                                query.database.clone().unwrap_or_default(),
                                connection_name,
                                t!(routine.kind.label_key())
                            ),
                        )
                    } else {
                        let title = match &query.name {
                            Some(name) => format!("{name} - {}", t!("common.query")),
                            None => {
                                format!("{} - {}", t!("query.untitled"), t!("common.query"))
                            }
                        };
                        ("icons/queries.svg", app.theme.icon_queries, title)
                    };
                    let color = query
                        .connection_index
                        .and_then(|connection| app.connection_color(connection));
                    (index, query.id, title, icon, icon_color, color)
                })
                .collect();
            let design_tabs: Vec<DesignTabRow> = app
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
                    let color = app
                        .connection_index_of(&design.connection)
                        .and_then(|connection| app.connection_color(connection));
                    (index, design.id, title, design.is_view, color)
                })
                .collect();
            // The pinned Object tab keeps the colour of the connection whose object list it shows,
            // so the environment stays visible on every main tab (表/视图/函数/用户/查询/备份).
            let object_color = app
                .object_pane
                .as_ref()
                .map(|pane| pane.read(cx).connection_index)
                .and_then(|connection| app.connection_color(connection));
            (
                app.theme,
                app.active_grid,
                app.active_query,
                app.active_design,
                grid_tabs,
                query_tabs,
                design_tabs,
                object_color,
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

        // Draw the tabs in their opening order (the same order Ctrl+Tab walks).
        let order = self
            .app
            .upgrade()
            .map(|app| app.read(cx).ordered_tabs(cx))
            .unwrap_or_default();
        let mut tabs: Vec<(TabTarget, AnyElement)> = Vec::new();
        for (index, id, title, is_view, is_key, color) in grid_tabs {
            tabs.push((
                TabTarget::Grid(index),
                self.table_tab(
                    theme,
                    index,
                    id,
                    title,
                    is_view,
                    is_key,
                    color,
                    active_grid == Some(index),
                    cx,
                )
                .into_any_element(),
            ));
        }
        for (index, id, title, is_view, color) in design_tabs {
            tabs.push((
                TabTarget::Design(index),
                self.design_tab(
                    theme,
                    index,
                    id,
                    title,
                    is_view,
                    color,
                    active_design == Some(index),
                    cx,
                )
                .into_any_element(),
            ));
        }
        for (index, id, title, icon, icon_color, color) in query_tabs {
            tabs.push((
                TabTarget::Query(index),
                self.query_tab(
                    theme,
                    index,
                    id,
                    title,
                    icon,
                    icon_color,
                    active_query == Some(index),
                    color,
                    cx,
                )
                .into_any_element(),
            ));
        }
        tabs.sort_by_key(|(target, _)| {
            order
                .iter()
                .position(|ordered| ordered == target)
                .unwrap_or(usize::MAX)
        });
        strip = strip.children(tabs.into_iter().map(|(_, tab)| tab));

        // The object tab is pinned; the arrows only appear (and wrap the scrollable tab strip)
        // when the open tabs no longer fit.
        let needs_scroll = self.scroll.max_offset().x > px(0.0);
        // The bar itself stays neutral; the environment colour lives on the active tab, so tabs
        // never blend into the bar.
        let mut bar = div()
            .flex()
            .flex_row()
            .items_end()
            .gap_1()
            .h(px(30.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg));
        bar = bar.child(self.object_tab(
            theme,
            active_grid.is_none() && active_query.is_none() && active_design.is_none(),
            object_color,
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
