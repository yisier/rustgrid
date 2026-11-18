use super::*;
use gpui::HitboxBehavior;
use gpui::StyleRefinement;
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

/// The widest a tab grows before its title is ellipsized. A tab sizes itself to its title (the
/// titles carry the object, database and connection), so a short tab stays narrow while a long one
/// uses the room it needs; the full title is always in the tab's tooltip.
const TAB_MAX_WIDTH: f32 = 260.0;

/// The narrowest a tab shrinks to as more tabs open. Past this minimum the tabs stop shrinking and
/// the strip scrolls instead — keeping room for the icon, a few title characters and the close
/// button.
const TAB_MIN_WIDTH: f32 = 120.0;

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

/// The highlight on the tab a dragged tab hovers: a brand-coloured edge on the side the dragged
/// tab would land, plus a light fill. Zed's tabs show the drop side the same way. The dragged
/// tab's own slot gets no highlight, so dropping back where it started reads as a no-op.
fn tab_drop_target_style(
    style: StyleRefinement,
    theme: Theme,
    target_index: usize,
    dragged: &DraggedTab,
) -> StyleRefinement {
    if target_index == dragged.source_index {
        return style;
    }
    let style = style
        .bg(rgb(theme.tree_hover_bg))
        .border_color(rgb(theme.brand));
    if target_index < dragged.source_index {
        style.border_l_2()
    } else {
        style.border_r_2()
    }
}

/// The reorder a drop performs: the dragged item leaves its slot, then takes the hovered tab's
/// display index (`usize::MAX` = the strip's end). Removing first is what makes the highlight
/// honest — a target on the left receives the item before it, a target on the right after it,
/// exactly the edge shown while hovering.
fn move_tab_in_order<T>(order: &mut Vec<T>, from: usize, target_index: usize) {
    if order.len() <= 1 || from >= order.len() {
        return;
    }
    let key = order.remove(from);
    let at = target_index.min(order.len());
    order.insert(at, key);
}

/// The display data of a tab, used to draw the floating copy while it is dragged.
#[derive(Clone)]
struct TabVisual {
    title: String,
    icon: &'static str,
    icon_color: u32,
    color: Option<ConnectionColor>,
}

/// What a tab drag carries: the tab's stable identity and the display position it started from,
/// so a hovered tab can show on which side the dragged tab would land.
#[derive(Clone, Copy)]
struct DraggedTab {
    source: TabTarget,
    source_index: usize,
}

/// The floating copy of a dragged tab. The engine builds this view once when the drag starts and
/// paints it at the pointer on every frame, so following the pointer never re-renders the tab
/// strip or the page (Zed's tab drag uses the same mechanism).
struct TabDragPreview {
    theme: Theme,
    visual: TabVisual,
    width: f32,
}

impl Render for TabDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let visual = &self.visual;
        let background = rgba(tab_background(theme, visual.color, 0xa0, theme.editor_bg));
        div()
            .w(px(self.width))
            .h(px(28.0))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .overflow_hidden()
            .bg(background)
            .border_1()
            .border_color(rgb(theme.brand))
            .shadow(ui::dialog_shadow())
            .text_size(px(12.0))
            .child(tree_icon(visual.icon, visual.icon_color))
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(visual.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .w(px(16.0))
                    .h(px(16.0))
                    .text_color(rgb(theme.text_muted))
                    .child("✕"),
            )
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
            // Cached again so a re-render of the page (a grid scroll, the status bar, ...) does
            // not rebuild every tab; `tab_revision` notifies it when its own inputs or the
            // strip's width change, and a tab drop notifies it directly.
            content = content.child(self.tab_bar.clone().cached(cached_style(|d| {
                d.flex().flex_row().h(px(30.0)).flex_none().w_full()
            })));
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

    /// The window-space rectangle of the tab at display position `index` in the last layout, if it
    /// was drawn. The strip's tracked scroll handle records one per child, scroll offset included.
    fn tab_bounds(&self, index: usize) -> Option<Bounds<Pixels>> {
        self.scroll.bounds_for_item(index)
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

    /// Pan the strip while a dragged tab sits near either edge, so it can be carried past the
    /// tabs currently in view.
    fn drag_autoscroll(&mut self, position: Point<Pixels>, cx: &mut Context<'_, Self>) {
        let bounds = self.scroll.bounds();
        let x = f32::from(position.x);
        let delta = if x < f32::from(bounds.left()) + TAB_DRAG_EDGE {
            -TAB_DRAG_SCROLL_STEP
        } else if x > f32::from(bounds.right()) - TAB_DRAG_EDGE {
            TAB_DRAG_SCROLL_STEP
        } else {
            return;
        };
        self.scroll_tabs(delta, cx);
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
        target: TabTarget,
        display_index: usize,
        drag_width: f32,
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
        let drop = self.app.clone();
        let drag_visual = TabVisual {
            title: title.clone(),
            icon,
            icon_color,
            color,
        };
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
            .hover(move |style| style.bg(hover_bg))
            .on_drag(
                DraggedTab {
                    source: target,
                    source_index: display_index,
                },
                move |_dragged, _offset, _window, cx| {
                    cx.new(|_| TabDragPreview {
                        theme,
                        visual: drag_visual.clone(),
                        width: drag_width,
                    })
                },
            )
            .drag_over::<DraggedTab>(move |tab, dragged, _window, _cx| {
                tab_drop_target_style(tab, theme, display_index, dragged)
            })
            .on_drop(move |dragged: &DraggedTab, _window, cx| {
                // Deferred, and a plain closure rather than `cx.listener`: the drop reorders the
                // tabs and notifies the tab bar, and neither may happen while this bar is
                // borrowed by its own event listener.
                let app = drop.clone();
                let source = dragged.source;
                cx.defer(move |cx| {
                    let _ = app.update(cx, |app, cx| {
                        app.drop_tab_on(source, display_index, cx);
                    });
                });
            })
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
        target: TabTarget,
        display_index: usize,
        drag_width: f32,
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
        let drop = self.app.clone();
        let drag_visual = TabVisual {
            title: title.clone(),
            icon,
            icon_color,
            color,
        };
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
            .hover(move |style| style.bg(hover_bg))
            .on_drag(
                DraggedTab {
                    source: target,
                    source_index: display_index,
                },
                move |_dragged, _offset, _window, cx| {
                    cx.new(|_| TabDragPreview {
                        theme,
                        visual: drag_visual.clone(),
                        width: drag_width,
                    })
                },
            )
            .drag_over::<DraggedTab>(move |tab, dragged, _window, _cx| {
                tab_drop_target_style(tab, theme, display_index, dragged)
            })
            .on_drop(move |dragged: &DraggedTab, _window, cx| {
                // Deferred, and a plain closure rather than `cx.listener`: the drop reorders the
                // tabs and notifies the tab bar, and neither may happen while this bar is
                // borrowed by its own event listener.
                let app = drop.clone();
                let source = dragged.source;
                cx.defer(move |cx| {
                    let _ = app.update(cx, |app, cx| {
                        app.drop_tab_on(source, display_index, cx);
                    });
                });
            })
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
        target: TabTarget,
        display_index: usize,
        drag_width: f32,
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
        let drop = self.app.clone();
        let drag_visual = TabVisual {
            title: title.clone(),
            icon: "icons/design_table.svg",
            icon_color,
            color,
        };
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
            .hover(move |style| style.bg(hover_bg))
            .on_drag(
                DraggedTab {
                    source: target,
                    source_index: display_index,
                },
                move |_dragged, _offset, _window, cx| {
                    cx.new(|_| TabDragPreview {
                        theme,
                        visual: drag_visual.clone(),
                        width: drag_width,
                    })
                },
            )
            .drag_over::<DraggedTab>(move |tab, dragged, _window, _cx| {
                tab_drop_target_style(tab, theme, display_index, dragged)
            })
            .on_drop(move |dragged: &DraggedTab, _window, cx| {
                // Deferred, and a plain closure rather than `cx.listener`: the drop reorders the
                // tabs and notifies the tab bar, and neither may happen while this bar is
                // borrowed by its own event listener.
                let app = drop.clone();
                let source = dragged.source;
                cx.defer(move |cx| {
                    let _ = app.update(cx, |app, cx| {
                        app.drop_tab_on(source, display_index, cx);
                    });
                });
            })
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
        let mut tabs: Vec<(TabTarget, AnyElement)> = Vec::new();
        for (index, id, title, is_view, is_key, color) in grid_tabs {
            let target = TabTarget::Grid(index);
            let display_index = order.iter().position(|tab| *tab == target).unwrap_or(0);
            let drag_width = self
                .tab_bounds(display_index)
                .map(|bounds| f32::from(bounds.size.width))
                .unwrap_or(TAB_MAX_WIDTH);
            tabs.push((
                target,
                self.table_tab(
                    theme,
                    target,
                    display_index,
                    drag_width,
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
            let target = TabTarget::Design(index);
            let display_index = order.iter().position(|tab| *tab == target).unwrap_or(0);
            let drag_width = self
                .tab_bounds(display_index)
                .map(|bounds| f32::from(bounds.size.width))
                .unwrap_or(TAB_MAX_WIDTH);
            tabs.push((
                target,
                self.design_tab(
                    theme,
                    target,
                    display_index,
                    drag_width,
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
            let target = TabTarget::Query(index);
            let display_index = order.iter().position(|tab| *tab == target).unwrap_or(0);
            let drag_width = self
                .tab_bounds(display_index)
                .map(|bounds| f32::from(bounds.size.width))
                .unwrap_or(TAB_MAX_WIDTH);
            tabs.push((
                target,
                self.query_tab(
                    theme,
                    target,
                    display_index,
                    drag_width,
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
        // The tabs are the strip's own children: that makes `scroll_to_item` reveal the active
        // tab, and the strip's tracked scroll handle records their rectangles.
        //
        // A trailing filler is the drag's end target (dropping past the last tab appends) and the
        // strip scrolls while a dragged tab nears either edge, so a tab can be carried past the
        // visible ones. The filler keeps a minimum width once the tabs overflow, exactly as Zed's
        // tab-bar drop target does.
        let end_drop = self.app.clone();
        let strip_end = div()
            .id("tab-strip-end")
            .flex_grow_1()
            .min_w(px(24.0))
            .h_full()
            .child("")
            .on_drop(move |dragged: &DraggedTab, _window, cx| {
                // As on the tabs: defer the reorder past this event's dispatch.
                let app = end_drop.clone();
                let source = dragged.source;
                cx.defer(move |cx| {
                    let _ = app.update(cx, |app, cx| {
                        app.drop_tab_on(source, usize::MAX, cx);
                    });
                });
            })
            .drag_over::<DraggedTab>(move |style, _dragged, _window, _cx| {
                style.bg(rgb(theme.tree_hover_bg))
            });
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
            .on_drag_move::<DraggedTab>(cx.listener(
                |this, event: &DragMoveEvent<DraggedTab>, _window, cx| {
                    this.drag_autoscroll(event.event.position, cx);
                },
            ))
            .children(tabs.into_iter().map(|(_, tab)| tab))
            .child(strip_end);

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
    pub(super) fn notify_tab_bar(&self, cx: &mut Context<'_, Self>) {
        self.tab_bar.update(cx, |_, cx| cx.notify());
    }

    /// A tab was dropped onto the tab at display position `target_index` (or the strip's empty
    /// area, passed as `usize::MAX`). The dragged tab takes that tab's place: before it when it
    /// comes from the right, after it when it comes from the left (the side the drop highlight
    /// shows). The order changes once, on drop, exactly as it does in Zed's tabs.
    pub(super) fn drop_tab_on(
        &mut self,
        source: TabTarget,
        target_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let order = self.ordered_tabs(cx);
        let Some(from) = order.iter().position(|tab| *tab == source) else {
            return;
        };
        if order.len() > 1 {
            // `ordered_tabs` reconciled the sequence just above, so it mirrors the display order.
            let mut sequence = self.tab_sequence.borrow_mut();
            move_tab_in_order(&mut sequence, from, target_index);
        }
        self.notify_tab_bar(cx);
        self.show_tab(Some(source), cx);
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

    /// The tab strip's layout contract: a tab sizes itself to its title, stops at
    /// [`TAB_MAX_WIDTH`] and shrinks down to [`TAB_MIN_WIDTH`] when the tabs no longer fit, after
    /// which the strip scrolls (the ◀ / ▶ pagers' condition). Widths come from the flexbox, so
    /// widening the strip (a pane closing, the sidebar narrowing) grows the tabs back without
    /// recomputing anything in Rust.
    struct StripHarness {
        scroll: ScrollHandle,
        count: usize,
        width: f32,
        title: &'static str,
    }

    /// Longer than [`TAB_MAX_WIDTH`], so a tab's natural size reaches the cap.
    const LONG_TITLE: &str =
        "a very long tab title that definitely exceeds the maximum tab width and then some more";

    fn tab_id(index: usize) -> SharedString {
        SharedString::from(format!("test-tab-{index}"))
    }

    impl Render for StripHarness {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<'_, Self>,
        ) -> impl IntoElement {
            let title = self.title;
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
                    .min_w(px(TAB_MIN_WIDTH))
                    .max_w(px(TAB_MAX_WIDTH))
                    .overflow_hidden()
                    .child(div().min_w(px(0.0)).truncate().child(title))
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
                title: LONG_TITLE,
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

        // Five long-title tabs share an 800px strip: (800 - 4 gaps) / 5 = 156.8 each.
        let widths = tab_widths(handle, 5, cx);
        for width in &widths {
            assert!(
                (*width - 156.8).abs() < 1.5,
                "tabs should share the strip when their content does not fit, measured {widths:?}"
            );
        }
        assert_eq!(scroll.max_offset().x, px(0.0), "five tabs fit");

        // Fewer long tabs size to their title and stop at the maximum.
        harness.update(cx, |harness, cx| {
            harness.count = 2;
            cx.notify();
        });
        for width in tab_widths(handle, 2, cx) {
            assert!(
                (width - TAB_MAX_WIDTH).abs() < 1.0,
                "a long title should reach the maximum width, measured {width}"
            );
        }

        // A short title keeps its own (minimum) width instead of stretching to fill.
        harness.update(cx, |harness, cx| {
            harness.count = 3;
            harness.title = "q";
            cx.notify();
        });
        let widths = tab_widths(handle, 3, cx);
        for width in &widths {
            assert!(
                (width - TAB_MIN_WIDTH).abs() < 1.0,
                "a short title should stay at the minimum width, measured {widths:?}"
            );
        }
        assert_eq!(scroll.max_offset().x, px(0.0), "three short tabs fit");

        // Ten long tabs at 800px would need 76px each: they stop at the minimum and scroll.
        harness.update(cx, |harness, cx| {
            harness.count = 10;
            harness.title = LONG_TITLE;
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

    #[test]
    fn tab_drop_target_highlights_the_landing_side() {
        let dragged = DraggedTab {
            source: TabTarget::Grid(0),
            source_index: 2,
        };
        let edge = |value: f32| gpui::AbsoluteLength::Pixels(px(value));

        // Dropping on a tab to the left lands before it: the target's left edge lights up.
        let left =
            tab_drop_target_style(gpui::StyleRefinement::default(), Theme::dark(), 0, &dragged);
        assert_eq!(left.border_widths.left, Some(edge(2.0)));
        assert!(left.border_widths.right.is_none());
        assert!(left.background.is_some());

        // Dropping on a tab to the right lands after it: the right edge lights up.
        let right =
            tab_drop_target_style(gpui::StyleRefinement::default(), Theme::dark(), 4, &dragged);
        assert_eq!(right.border_widths.right, Some(edge(2.0)));
        assert!(right.border_widths.left.is_none());
        assert!(right.background.is_some());

        // Its own slot stays plain: dropping back where it started is a no-op.
        let own =
            tab_drop_target_style(gpui::StyleRefinement::default(), Theme::dark(), 2, &dragged);
        assert!(own.background.is_none());
        assert!(own.border_widths.left.is_none());
        assert!(own.border_widths.right.is_none());
    }

    #[test]
    fn tab_drop_reorders_to_the_highlighted_side() {
        // Dropping A on C puts it after C (C's right edge was highlighted).
        let mut tabs = vec!["A", "B", "C"];
        move_tab_in_order(&mut tabs, 0, 2);
        assert_eq!(tabs, ["B", "C", "A"]);

        // Dropping C on A puts it before A (A's left edge was highlighted).
        let mut tabs = vec!["A", "B", "C"];
        move_tab_in_order(&mut tabs, 2, 0);
        assert_eq!(tabs, ["C", "A", "B"]);

        // Dropping B on C puts it after C, and on A before A.
        let mut tabs = vec!["A", "B", "C"];
        move_tab_in_order(&mut tabs, 1, 2);
        assert_eq!(tabs, ["A", "C", "B"]);
        let mut tabs = vec!["A", "B", "C"];
        move_tab_in_order(&mut tabs, 1, 0);
        assert_eq!(tabs, ["B", "A", "C"]);

        // Dropping on itself, or past the last tab, changes nothing / appends.
        let mut tabs = vec!["A", "B", "C"];
        move_tab_in_order(&mut tabs, 1, 1);
        assert_eq!(tabs, ["A", "B", "C"]);
        move_tab_in_order(&mut tabs, 1, usize::MAX);
        assert_eq!(tabs, ["A", "C", "B"]);
    }

    fn window_pump(handle: gpui::AnyWindowHandle, cx: &mut TestAppContext) {
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .expect("window is alive");
    }
}
