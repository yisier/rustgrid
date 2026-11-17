use super::*;
use gpui::HitboxBehavior;
use std::cell::Cell;

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

/// The widest a tab grows; the full title is in the tab's tooltip.
const TAB_MAX_WIDTH: f32 = 170.0;

/// The narrowest a tab shrinks to as more tabs open. The strip lays its tabs out as flexbox items
/// (so it resizes them itself when a pane changes the strip's width), and past this minimum the
/// tabs stop shrinking and the strip scrolls instead — keeping room for the icon, a few title
/// characters and the close button.
const TAB_MIN_WIDTH: f32 = 96.0;

/// How far the pointer must travel before a press on a tab becomes a drag (so a plain click still
/// activates the tab).
const TAB_DRAG_THRESHOLD: f32 = 4.0;

/// How close to the strip's edge a dragged tab auto-scrolls the strip, and by how much per move.
const TAB_DRAG_EDGE: f32 = 28.0;
const TAB_DRAG_SCROLL_STEP: f32 = 14.0;

/// The ◀ / ▶ pager's width.
const TAB_SCROLL_ARROW_WIDTH: f32 = 18.0;

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
        let main_font = self.main_font();
        let main_font_size = self.main_font_size();
        let ui_zoom = self.ui_ctrl_wheel_zoom;
        // Whether the active page is a list (where Ctrl + wheel zooms the main panel). Grids and
        // editors own their own zoom gesture, so they are left out.
        let mut main_zoom = false;

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
                main_zoom = true;
                self.render_users(cx).into_any_element()
            } else if self.main_tab == MainTab::Queries {
                main_zoom = true;
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_query_object_toolbar(cx))
                    .child(self.render_saved_queries(cx))
                    .into_any_element()
            } else if self.main_tab == MainTab::Backups {
                main_zoom = true;
                self.render_backups(cx).into_any_element()
            } else if self.main_tab == MainTab::Sync {
                main_zoom = true;
                self.render_sync(cx)
            } else if matches!(self.main_tab, MainTab::Functions | MainTab::Views)
                && self.object_pane.is_none()
            {
                main_zoom = true;
                div().flex_1().into_any_element()
            } else if let Some(pane) = self.object_pane.clone() {
                main_zoom = true;
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
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .key_context(PAGE_CONTEXT)
            .bg(rgb(theme.editor_bg))
            .text_size(px(main_font_size));
        if let Some(font) = main_font {
            content = content.font_family(font);
        }
        if has_tabs {
            // Not cached: the bar is cheap to rebuild, and it must re-evaluate its width-derived
            // state (the pagers and the tabs' flex sizing) whenever a pane changes the content's
            // width, which a cached child would miss.
            content = content.child(self.tab_bar.clone());
        }
        let mut body_frame = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .overflow_hidden();
        if main_zoom && ui_zoom {
            let app = cx.weak_entity();
            body_frame = body_frame.child(
                canvas(
                    |bounds, window, _cx| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_bounds, hitbox, window, _cx| {
                        let app = app.clone();
                        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture
                                || !hitbox.should_handle_scroll(window)
                            {
                                return;
                            }
                            if !(event.modifiers.control || event.modifiers.platform) {
                                return;
                            }
                            let dy = match event.delta {
                                ScrollDelta::Lines(delta) => delta.y,
                                ScrollDelta::Pixels(delta) => f32::from(delta.y),
                            };
                            if dy == 0.0 {
                                return;
                            }
                            let app = app.clone();
                            let consumed = app
                                .update(cx, |app, cx| {
                                    app.zoom_main_font(if dy > 0.0 { 1.0 } else { -1.0 }, cx)
                                })
                                .unwrap_or(false);
                            if consumed {
                                cx.stop_propagation();
                            }
                        });
                    },
                )
                .absolute()
                .inset_0(),
            );
        }
        content.child(body_frame.child(body))
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
            overflow: Rc::new(Cell::new(false)),
            last_active_tab: None,
        }
    }

    fn theme(&self, cx: &Context<'_, Self>) -> Theme {
        self.app
            .upgrade()
            .map(|app| app.read(cx).theme)
            .unwrap_or_else(Theme::dark)
    }

    /// The center x of the tab at display position `index` in the last layout, if it was drawn.
    /// The strip's tracked scroll handle records one window-space rectangle per child, scroll
    /// offset included, so this is where the drag's midpoint test works.
    fn tab_center(&self, index: usize) -> Option<f32> {
        self.scroll
            .bounds_for_item(index)
            .map(|bounds| f32::from(bounds.origin.x) + f32::from(bounds.size.width) / 2.0)
    }

    fn scroll_tabs(&mut self, delta: f32, cx: &mut Context<'_, Self>) -> bool {
        let max = f32::from(self.scroll.max_offset().x);
        if max <= 0.0 {
            return false;
        }
        let current = -f32::from(self.scroll.offset().x);
        let next = (current + delta).clamp(0.0, max);
        if (next - current).abs() < 0.5 {
            return false;
        }
        let y = self.scroll.offset().y;
        self.scroll.set_offset(Point::new(px(-next), y));
        cx.notify();
        true
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
            .w(px(TAB_SCROLL_ARROW_WIDTH))
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
        dragging: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let tab_id = SharedString::from(format!("query-tab-{id}"));
        let close_id = SharedString::from(format!("query-tab-close-{id}"));
        let activate = self.app.clone();
        let close = self.app.clone();
        let menu = self.app.clone();
        let middle = self.app.clone();
        let press = self.app.clone();
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
            .flex_1()
            .min_w(px(TAB_MIN_WIDTH))
            .max_w(px(TAB_MAX_WIDTH))
            .overflow_hidden()
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
            .when(dragging, |style| {
                style.bg(hover_bg).border_color(rgb(theme.brand))
            })
            .hover(move |style| style.bg(hover_bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, event: &MouseDownEvent, _window, cx| {
                    let _ = press.update(cx, |app, _cx| {
                        app.tab_press_start(TabTarget::Query(index), event.position);
                    });
                }),
            )
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
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .when(active, |style| style.font_weight(FontWeight::SEMIBOLD))
                    .child(title),
            )
            .child(
                div()
                    .id(close_id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
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
        dragging: bool,
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
        let press = self.app.clone();
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
            .flex_1()
            .min_w(px(TAB_MIN_WIDTH))
            .max_w(px(TAB_MAX_WIDTH))
            .overflow_hidden()
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
            .when(dragging, |style| {
                style.bg(hover_bg).border_color(rgb(theme.brand))
            })
            .hover(move |style| style.bg(hover_bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, event: &MouseDownEvent, _window, cx| {
                    let _ = press.update(cx, |app, _cx| {
                        app.tab_press_start(TabTarget::Grid(index), event.position);
                    });
                }),
            )
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
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .when(active, |style| style.font_weight(FontWeight::SEMIBOLD))
                    .child(title),
            )
            .child(
                div()
                    .id(close_id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
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
        dragging: bool,
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
        let press = self.app.clone();
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
            .flex_1()
            .min_w(px(TAB_MIN_WIDTH))
            .max_w(px(TAB_MAX_WIDTH))
            .overflow_hidden()
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
            .when(dragging, |style| {
                style.bg(hover_bg).border_color(rgb(theme.brand))
            })
            .hover(move |style| style.bg(hover_bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, event: &MouseDownEvent, _window, cx| {
                    let _ = press.update(cx, |app, _cx| {
                        app.tab_press_start(TabTarget::Design(index), event.position);
                    });
                }),
            )
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
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .when(active, |style| style.font_weight(FontWeight::SEMIBOLD))
                    .child(title),
            )
            .child(
                div()
                    .id(close_id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
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
            active_target,
            tab_drag,
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
                    let kind = if grid.state.documents {
                        t!(if grid.state.is_view {
                            "common.view"
                        } else {
                            "common.collection"
                        })
                    } else {
                        t!(if grid.state.is_view {
                            "common.view"
                        } else {
                            "common.table"
                        })
                    }
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
                    } else if let Some(batch) = &query.batch {
                        let group_name = groups::find_group(&app.tree_layout, batch.group_id)
                            .map(|group| group.name.clone())
                            .unwrap_or_else(|| batch.group_name.clone());
                        let title = match &query.name {
                            Some(name) => format!("{name} - {}", t!("common.query")),
                            None => format!(
                                "{} · {} - {}",
                                t!("query.batch.title"),
                                group_name,
                                t!("common.query")
                            ),
                        };
                        let icon = if query.name.is_some() {
                            "icons/queries.svg"
                        } else {
                            "icons/folder.svg"
                        };
                        (icon, app.theme.icon_queries, title)
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
                        .or_else(|| {
                            query
                                .batch
                                .as_ref()
                                .and_then(|batch| batch.targets.first())
                                .and_then(|target| target.connection_index)
                        })
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
                app.current_tab(cx),
                app.tab_drag,
            )
        };

        // The tabs are flex items (auto width up to `TAB_MAX_WIDTH`, shrinking to
        // `TAB_MIN_WIDTH`), so the layout itself resizes them whenever a pane changes the strip's
        // width. The ◀ / ▶ pagers are drawn from the previous frame's overflow measurement.
        let needs_scroll = self.overflow.get();

        // Draw the tabs in their display order (the same order Ctrl+Tab walks): the opening order,
        // then whatever the user dragged them into.
        let order = self
            .app
            .upgrade()
            .map(|app| app.read(cx).ordered_tabs(cx))
            .unwrap_or_default();
        let is_dragging = |target: TabTarget| tab_drag.is_some_and(|drag| drag.source == target);
        let mut tabs: Vec<(TabTarget, AnyElement)> = Vec::new();
        for (index, id, title, is_view, is_key, color) in grid_tabs {
            let target = TabTarget::Grid(index);
            tabs.push((
                target,
                self.table_tab(
                    theme,
                    index,
                    id,
                    title,
                    is_view,
                    is_key,
                    color,
                    active_grid == Some(index),
                    is_dragging(target),
                    cx,
                )
                .into_any_element(),
            ));
        }
        for (index, id, title, is_view, color) in design_tabs {
            let target = TabTarget::Design(index);
            tabs.push((
                target,
                self.design_tab(
                    theme,
                    index,
                    id,
                    title,
                    is_view,
                    color,
                    active_design == Some(index),
                    is_dragging(target),
                    cx,
                )
                .into_any_element(),
            ));
        }
        for (index, id, title, icon, icon_color, color) in query_tabs {
            let target = TabTarget::Query(index);
            tabs.push((
                target,
                self.query_tab(
                    theme,
                    index,
                    id,
                    title,
                    icon,
                    icon_color,
                    active_query == Some(index),
                    color,
                    is_dragging(target),
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
        // The tabs are the strip's own children: that makes `scroll_to_item` reveal the active
        // tab, and the strip's tracked scroll handle records their rectangles for the drag reorder
        // (`ScrollHandle::bounds_for_item`).
        let strip = div()
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
            .track_scroll(&self.scroll)
            .children(tabs.into_iter().map(|(_, tab)| tab));

        // Whether the tabs overflow is known only after layout, so a paint-only canvas over the
        // wrapper (which the strip fills) compares the strip's scroll range with the last frame's
        // state and asks for one more frame when it changed. The pagers are drawn from that
        // previous measurement.
        let overflow = self.overflow.clone();
        let scroll = self.scroll.clone();
        let weak = cx.entity().downgrade();
        let measure = canvas(
            |_bounds, _window: &mut Window, _cx: &mut App| {},
            move |_bounds, _state, window: &mut Window, cx: &mut App| {
                let overflows = scroll.max_offset().x > px(0.0);
                if overflow.get() != overflows {
                    overflow.set(overflows);
                    window.request_animation_frame();
                    let _ = weak.update(cx, |_, cx| cx.notify());
                }
            },
        )
        .absolute()
        .inset_0();

        // Reveal the active tab when it changes or a new tab opens at the far end (the strip may
        // be scrolled away from it after the user paged it).
        if self.last_active_tab != active_target {
            self.last_active_tab = active_target;
            if let Some(target) = active_target
                && let Some(position) = order.iter().position(|tab| *tab == target)
            {
                self.scroll.scroll_to_item(position);
            }
        }

        // The object tab is pinned; the arrows only appear (and wrap the scrollable tab strip)
        // once even the minimum-width tabs no longer fit.
        //
        // The bar itself stays neutral; the environment colour lives on the active tab, so tabs
        // never blend into the bar.
        let mut bar = div()
            .flex()
            .flex_row()
            .items_end()
            .gap_1()
            .h(px(30.0))
            .w_full()
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
        // `ScrollableMask` maps a plain mouse wheel's vertical lines onto the strip's horizontal
        // scroll, as gpui gives an `overflow_x_scroll` container only a horizontal delta. The
        // wrapper keeps the mask over the viewport rather than sliding with the content.
        bar = bar.child(
            div()
                .relative()
                .flex()
                .flex_row()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .child(strip)
                .child(gpui_kit::base::ScrollableMask::vertical_wheel(&self.scroll))
                .child(measure),
        );
        if needs_scroll {
            bar = bar.child(self.scroll_button("tab-scroll-right", false, cx));
        }
        bar.into_any_element()
    }
}

impl AppView {
    /// Note a left-press on a tab. It only becomes a drag once the pointer travels far enough
    /// ([`TAB_DRAG_THRESHOLD`]), so a plain click still activates the tab.
    pub(super) fn tab_press_start(&mut self, target: TabTarget, position: Point<Pixels>) {
        self.tab_press = Some((target, position));
    }

    /// Update the in-progress tab drag: reorder the strip as the pointer crosses a neighbour's
    /// midpoint and auto-scroll it near the edges. Only the tab bar repaints, and only when the
    /// order or the strip's offset actually changes, so a drag does not re-render the page under
    /// the pointer.
    pub(super) fn tab_drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            let was_dragging = self.tab_drag.take().is_some();
            self.tab_press = None;
            if was_dragging {
                self.notify_tab_bar(cx);
            }
            return;
        }
        let Some((source, start)) = self.tab_press else {
            return;
        };
        if self.tab_drag.is_none() {
            let dx = f32::from(event.position.x) - f32::from(start.x);
            let dy = f32::from(event.position.y) - f32::from(start.y);
            if dx.hypot(dy) <= TAB_DRAG_THRESHOLD {
                return;
            }
            self.tab_drag = Some(TabDrag { source });
            self.notify_tab_bar(cx);
        }
        self.tab_drag_step(f32::from(event.position.x), cx);
        self.tab_drag_autoscroll(event.position, cx);
    }

    /// Finish a tab drag on mouse-up. The order is already applied (the strip reordered live), so
    /// the drop only clears the drag state.
    pub(super) fn tab_drag_end(&mut self, cx: &mut Context<'_, Self>) {
        self.tab_press = None;
        if self.tab_drag.take().is_some() {
            self.notify_tab_bar(cx);
        }
    }

    pub(super) fn notify_tab_bar(&self, cx: &mut Context<'_, Self>) {
        self.tab_bar.update(cx, |_, cx| cx.notify());
    }

    /// Move the dragged tab one slot when the pointer crosses the midpoint between it and its
    /// neighbour. The tabs are all one width, so each swap shifts it exactly one slot; the midpoint
    /// rule gives the half-tab hysteresis that keeps a drag from flickering between two slots.
    fn tab_drag_step(&mut self, pointer_x: f32, cx: &mut Context<'_, Self>) {
        let Some(drag) = self.tab_drag else {
            return;
        };
        let order = self.ordered_tabs(cx);
        // The centers come from the last layout and line up with display positions. If any tab was
        // not drawn yet (the first frame after opening one), wait for the next move.
        let centers: Option<Vec<f32>> = {
            let bar = self.tab_bar.read(cx);
            (0..order.len())
                .map(|index| bar.tab_center(index))
                .collect()
        };
        let Some(centers) = centers else {
            return;
        };
        let Some(mut position) = order.iter().position(|target| *target == drag.source) else {
            return;
        };
        let mut moved = false;
        loop {
            let target =
                if position > 0 && pointer_x < (centers[position] + centers[position - 1]) / 2.0 {
                    position - 1
                } else if position + 1 < centers.len()
                    && pointer_x > (centers[position] + centers[position + 1]) / 2.0
                {
                    position + 1
                } else {
                    break;
                };
            // `ordered_tabs` reconciled `tab_sequence` just above, so display positions and the
            // sequence line up one to one.
            let mut sequence = self.tab_sequence.borrow_mut();
            if target < sequence.len() && position < sequence.len() {
                sequence.swap(position, target);
            }
            drop(sequence);
            position = target;
            moved = true;
        }
        if moved {
            self.notify_tab_bar(cx);
        }
    }

    /// Pan the strip while a dragged tab sits near either edge, so it can be dragged past the
    /// visible tabs.
    fn tab_drag_autoscroll(&mut self, position: Point<Pixels>, cx: &mut Context<'_, Self>) {
        let (left, right) = {
            let bar = self.tab_bar.read(cx);
            let bounds = bar.scroll.bounds();
            (f32::from(bounds.left()), f32::from(bounds.right()))
        };
        let x = f32::from(position.x);
        let delta = if x < left + TAB_DRAG_EDGE {
            -TAB_DRAG_SCROLL_STEP
        } else if x > right - TAB_DRAG_EDGE {
            TAB_DRAG_SCROLL_STEP
        } else {
            return;
        };
        self.tab_bar
            .update(cx, |bar, cx| bar.scroll_tabs(delta, cx));
    }

    /// Close every tab to the right of `target` (the tab menu's 关闭右侧).
    pub(super) fn close_tabs_to_right(&mut self, target: TabTarget, cx: &mut Context<'_, Self>) {
        self.tab_menu = None;
        if self.has_unsaved_changes(cx) {
            self.unsaved_confirm = Some(PendingAction::CloseRightTabs { keep: target });
            cx.notify();
            return;
        }
        self.close_tabs_to_right_now(target, cx);
    }

    pub(super) fn close_tabs_to_right_now(
        &mut self,
        target: TabTarget,
        cx: &mut Context<'_, Self>,
    ) {
        let order = self.ordered_tabs(cx);
        let Some(position) = order.iter().position(|tab| *tab == target) else {
            return;
        };
        let active_was_right = self
            .current_tab(cx)
            .and_then(|active| order.iter().position(|tab| *tab == active))
            .is_some_and(|active| active > position);
        // Closing shifts the arrays, so collect stable ids first and resolve each one against the
        // current arrays as it closes.
        let closing = &order[position + 1..];
        let keep = match target {
            TabTarget::Grid(index) => self
                .grids
                .get(index)
                .map(|grid| TabKey::Grid(grid.read(cx).state.id)),
            TabTarget::Query(index) => self.queries.get(index).map(|query| TabKey::Query(query.id)),
            TabTarget::Design(index) => self
                .designs
                .get(index)
                .map(|design| TabKey::Design(design.read(cx).id)),
        };
        let grid_ids: Vec<u64> = closing
            .iter()
            .filter_map(|tab| match tab {
                TabTarget::Grid(index) => self.grids.get(*index).map(|grid| grid.read(cx).state.id),
                _ => None,
            })
            .collect();
        let query_ids: Vec<u64> = closing
            .iter()
            .filter_map(|tab| match tab {
                TabTarget::Query(index) => self.queries.get(*index).map(|query| query.id),
                _ => None,
            })
            .collect();
        let design_ids: Vec<u64> = closing
            .iter()
            .filter_map(|tab| match tab {
                TabTarget::Design(index) => {
                    self.designs.get(*index).map(|design| design.read(cx).id)
                }
                _ => None,
            })
            .collect();
        for id in query_ids {
            if let Some(index) = self.queries.iter().position(|query| query.id == id) {
                self.close_query_now(index, cx);
            }
        }
        for id in design_ids {
            if let Some(index) = self
                .designs
                .iter()
                .position(|design| design.read(cx).id == id)
            {
                self.close_design_now(index, cx);
            }
        }
        for id in grid_ids {
            if let Some(index) = self
                .grids
                .iter()
                .position(|grid| grid.read(cx).state.id == id)
            {
                self.close_grid(index, cx);
            }
        }
        // The tab the user right-clicked stays where it is; if the active tab was one of the ones
        // just closed, show the clicked one instead of an unrelated neighbour. `target`'s index may
        // have shifted while closing, so the tab is resolved by its stable id again.
        if active_was_right || self.current_tab(cx).is_none() {
            let keep = keep.and_then(|key| match key {
                TabKey::Grid(id) => self
                    .grids
                    .iter()
                    .position(|grid| grid.read(cx).state.id == id)
                    .map(TabTarget::Grid),
                TabKey::Query(id) => self
                    .queries
                    .iter()
                    .position(|query| query.id == id)
                    .map(TabTarget::Query),
                TabKey::Design(id) => self
                    .designs
                    .iter()
                    .position(|design| design.read(cx).id == id)
                    .map(TabTarget::Design),
            });
            self.show_tab(keep, cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, Point, TestAppContext, WindowBounds, WindowOptions, size};
    use gpui_kit::TestSupportExt as _;
    use gpui_kit::test::TestWindowExt as _;

    /// The tab strip's layout contract: equal-width tabs that fill the strip, cap at
    /// [`TAB_MAX_WIDTH`] and shrink together down to [`TAB_MIN_WIDTH`], after which the strip
    /// scrolls (the ◀ / ▶ pagers' condition). Widths come from the flexbox, so widening the strip
    /// (a pane closing, the sidebar narrowing) grows the tabs back without recomputing anything in
    /// Rust.
    struct StripHarness {
        scroll: ScrollHandle,
        count: usize,
        width: f32,
    }

    fn tab_id(index: usize) -> SharedString {
        SharedString::from(format!("test-tab-{index}"))
    }

    impl Render for StripHarness {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<'_, Self>,
        ) -> impl IntoElement {
            let tabs = (0..self.count).map(|index| {
                div()
                    .id(tab_id(index))
                    .test_support()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h_full()
                    .px_2()
                    .flex_1()
                    .min_w(px(TAB_MIN_WIDTH))
                    .max_w(px(TAB_MAX_WIDTH))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .child("a long tab title that is ellipsized"),
                    )
                    .into_any_element()
            });
            div().w(px(self.width)).h(px(30.0)).child(
                div()
                    .id("test-tab-strip")
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap_1()
                    .pt_1()
                    .h_full()
                    .w_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll)
                    .children(tabs),
            )
        }
    }

    fn tab_widths(
        handle: gpui::AnyWindowHandle,
        count: usize,
        cx: &mut TestAppContext,
    ) -> Vec<f32> {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            (0..count)
                .map(|index| f32::from(window.find(tab_id(index)).bounds().size.width))
                .collect()
        })
        .expect("window is alive")
    }

    #[gpui_kit::test]
    fn tabs_fill_the_strip_then_shrink_and_overflow(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let scroll = ScrollHandle::new();
        let (handle, harness) = cx.update(|cx| {
            let harness = cx.new(|_| StripHarness {
                scroll: scroll.clone(),
                count: 5,
                width: 800.0,
            });
            let (handle, _) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(1000.0), px(200.0)),
                    })),
                    ..Default::default()
                },
                cx,
                |_, _| harness.clone(),
            )
            .expect("open test window");
            (handle, harness)
        });

        // Five tabs share an 800px strip: (800 - 4 gaps) / 5 = 156.8 each, below the 170 cap.
        let widths = tab_widths(handle, 5, cx);
        for width in &widths {
            assert!(
                (*width - 156.8).abs() < 1.5,
                "tabs should fill the strip evenly, measured {widths:?}"
            );
        }
        assert_eq!(scroll.max_offset().x, px(0.0), "five tabs fit");

        // Fewer tabs cap at TAB_MAX_WIDTH instead of stretching further.
        harness.update(cx, |harness, cx| {
            harness.count = 2;
            cx.notify();
        });
        for width in tab_widths(handle, 2, cx) {
            assert!(
                (width - TAB_MAX_WIDTH).abs() < 1.0,
                "tabs should stop at the maximum width, measured {width}"
            );
        }

        // Ten tabs at 800px would need 76px each: they stop at the minimum and the strip scrolls.
        harness.update(cx, |harness, cx| {
            harness.count = 10;
            cx.notify();
        });
        let widths = tab_widths(handle, 10, cx);
        for width in &widths {
            assert!(
                (width - TAB_MIN_WIDTH).abs() < 1.0,
                "tabs should stop at the minimum width, measured {widths:?}"
            );
        }
        window_pump(handle, cx);
        assert!(
            scroll.max_offset().x > px(0.0),
            "a strip of ten minimum-width tabs must overflow (the pagers' condition)"
        );

        // Widening the strip (a pane closes) grows them straight back, without any Rust-side
        // width bookkeeping: the bug this guards is tabs staying tiny beside blank space.
        harness.update(cx, |harness, cx| {
            harness.width = 1600.0;
            cx.notify();
        });
        let widths = tab_widths(handle, 10, cx);
        for width in &widths {
            assert!(
                (width - 156.4).abs() < 1.5,
                "tabs should grow back when the strip widens, measured {widths:?}"
            );
        }
        window_pump(handle, cx);
        assert_eq!(scroll.max_offset().x, px(0.0), "ten tabs fit 1600px");
    }

    fn window_pump(handle: gpui::AnyWindowHandle, cx: &mut TestAppContext) {
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("window is alive");
    }
}
