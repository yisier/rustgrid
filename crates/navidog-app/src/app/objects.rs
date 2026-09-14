use super::*;

impl AppView {
    pub(super) fn render_object_toolbar(
        &self,
        pane: &Entity<ObjectPane>,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let theme = self.theme;
        let (category, open_enabled) = {
            let pane = pane.read(cx);
            (pane.category, pane.selected.is_some())
        };
        if category == Category::Queries {
            return self.render_query_object_toolbar(cx);
        }
        let pane_for_open = pane.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(self.toolbar_item(
                        "obj-open",
                        "icons/tables.svg",
                        t!("object.open_table").to_string(),
                        open_enabled,
                        cx.listener(move |_this, _event, _window, cx| {
                            pane_for_open.update(cx, |pane, cx| {
                                pane.open_selected_object(cx);
                            });
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-design",
                        "icons/design_table.svg",
                        t!("object.design_table").to_string(),
                        false,
                        |_, _, _| {},
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-new",
                        "icons/new_table.svg",
                        t!("object.new_table").to_string(),
                        false,
                        |_, _, _| {},
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-delete",
                        "icons/delete_table.svg",
                        t!("object.delete_table").to_string(),
                        false,
                        |_, _, _| {},
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-import",
                        "icons/import.svg",
                        t!("object.import_wizard").to_string(),
                        false,
                        |_, _, _| {},
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-export",
                        "icons/export.svg",
                        t!("object.export_wizard").to_string(),
                        false,
                        |_, _, _| {},
                    )),
            )
            .child(self.render_object_search(cx))
    }

    /// The object toolbar shown for the `Queries` category, including when no database is open.
    pub(super) fn render_query_object_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .px_2()
            .py_1()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(self.toolbar_item(
                        "obj-new-query",
                        "icons/queries.svg",
                        t!("main.new_query").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| this.open_new_query(cx)),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-delete-query",
                        "icons/delete_table.svg",
                        t!("connection.delete").to_string(),
                        false,
                        |_, _, _| {},
                    )),
            )
            .child(self.render_object_search(cx))
    }

    pub(super) fn render_object_search(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let has_text = !self.object_search.is_empty();
        let mut field = ui::text_field(theme)
            .id("object-search")
            .track_focus(&self.object_search_focus)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w(px(220.0))
            .h(px(24.0))
            .px_2()
            .cursor_text()
            .on_key_down(cx.listener(|this, event, _window, cx| this.object_search_key(event, cx)))
            .on_click(cx.listener(|this, _event, window, cx| {
                window.focus(&this.object_search_focus);
                cx.notify();
            }))
            .child(
                svg()
                    .path("icons/search.svg")
                    .w(px(13.0))
                    .h(px(13.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );
        let caret = self.object_search_focused && self.caret_visible;
        if has_text {
            field = field.child(div().flex_1().overflow_hidden().whitespace_nowrap().child(
                format!("{}{}", self.object_search, if caret { "|" } else { "" }),
            ));
        } else {
            field = field.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(if caret { "|" } else { "" })
                    .child(if self.object_search_focused {
                        div()
                    } else {
                        div()
                            .text_color(rgb(theme.text_muted))
                            .child(t!("object.search").to_string())
                    }),
            );
        }
        if has_text {
            field = field.child(
                div()
                    .id("object-search-clear")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_none()
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.object_search.clear();
                        this.notify_object_pane(cx);
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path("icons/cross.svg")
                            .w(px(10.0))
                            .h(px(10.0))
                            .flex_none()
                            .text_color(rgb(theme.text_muted)),
                    ),
            );
        }
        field
    }
}

impl ObjectPane {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        connection_index: usize,
        database_index: usize,
        category: Category,
        theme: Theme,
    ) -> Self {
        Self {
            app,
            connection_index,
            database_index,
            category,
            selected: None,
            scroll: ScrollHandle::new(),
            hscroll_grab: None,
            theme,
        }
    }

    /// Ask `AppView` to re-render, e.g. so the toolbar's Open button reflects the selection.
    fn notify_app(&self, cx: &mut Context<'_, Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |_, cx| cx.notify());
        }
    }

    fn object_rows_per_column(&self) -> usize {
        let viewport = f32::from(self.scroll.bounds().size.height);
        if viewport <= 0.0 {
            return 30;
        }
        let rows = ((viewport - OBJECT_BOTTOM_MARGIN) / OBJECT_ROW_HEIGHT).floor() as usize;
        rows.max(1)
    }

    /// Render the list body from an owned snapshot of the database's tables.
    fn render_body(
        &self,
        tables: Option<Loadable<Vec<navidog_core::TableInfo>>>,
        search: &str,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let empty = || {
            div()
                .p_3()
                .text_color(rgb(theme.text_muted))
                .child(t!("common.empty").to_string())
                .into_any_element()
        };
        let Some(tables) = tables else {
            return empty();
        };
        match tables {
            Loadable::Idle | Loadable::Loading => div()
                .p_3()
                .text_color(rgb(theme.text_muted))
                .child(t!("common.loading").to_string())
                .into_any_element(),
            Loadable::Failed(error) => div()
                .p_3()
                .text_color(rgb(theme.danger))
                .child(error)
                .into_any_element(),
            Loadable::Loaded(tables) => {
                let want_view = match self.category {
                    Category::Tables => Some(false),
                    Category::Views => Some(true),
                    _ => None,
                };
                let Some(want_view) = want_view else {
                    return empty();
                };
                let rows = self.object_rows_per_column();
                let query = search.trim().to_lowercase();
                let mut columns = div().flex().flex_row().items_start().gap_1().p_1();
                let mut column = div().flex().flex_col();
                let mut count = 0usize;
                for table in tables.iter().filter(|table| {
                    matches!(table.kind, navidog_core::ObjectKind::View) == want_view
                        && (query.is_empty() || table.name.to_lowercase().contains(&query))
                }) {
                    if count == rows {
                        columns = columns.child(column);
                        column = div().flex().flex_col();
                        count = 0;
                    }
                    column = column.child(self.render_item(table, cx));
                    count += 1;
                }
                if count > 0 {
                    columns = columns.child(column);
                }
                columns.into_any_element()
            }
        }
    }

    fn render_hscrollbar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let bounds = self.scroll.bounds();
        let viewport = f32::from(bounds.size.width);
        let max = f32::from(self.scroll.max_offset().width);
        let scroll = -f32::from(self.scroll.offset().x);
        let (thumb_w, travel) = scrollbar_thumb(viewport, max);
        let thumb_x = if max > 0.0 {
            (scroll / max) * travel
        } else {
            0.0
        };

        ui::hscrollbar_track("object-hscrollbar", theme, thumb_x, thumb_w).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                this.hscroll_begin(event.position.x, cx);
            }),
        )
    }

    fn hscroll_begin(&mut self, mouse_x: Pixels, cx: &mut Context<'_, Self>) {
        let bounds = self.scroll.bounds();
        let viewport = f32::from(bounds.size.width);
        let max = f32::from(self.scroll.max_offset().width);
        let (thumb_w, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let scroll = -f32::from(self.scroll.offset().x);
        let thumb_x = (scroll / max) * travel;
        let relative = f32::from(mouse_x) - f32::from(bounds.left());
        let grab = if relative >= thumb_x && relative <= thumb_x + thumb_w {
            relative - thumb_x
        } else {
            thumb_w / 2.0
        };
        self.hscroll_grab = Some(grab);
        self.hscroll_set(relative, grab, cx);
    }

    fn hscroll_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some(grab) = self.hscroll_grab else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.hscroll_grab = None;
            cx.notify();
            return;
        }
        let relative = f32::from(event.position.x) - f32::from(self.scroll.bounds().left());
        self.hscroll_set(relative, grab, cx);
    }

    fn hscroll_set(&self, relative: f32, grab: f32, cx: &mut Context<'_, Self>) {
        let viewport = f32::from(self.scroll.bounds().size.width);
        let max = f32::from(self.scroll.max_offset().width);
        let (_, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let thumb_x = (relative - grab).clamp(0.0, travel);
        let scroll = thumb_x / travel * max;
        let y = self.scroll.offset().y;
        self.scroll.set_offset(Point::new(px(-scroll), y));
        cx.notify();
    }

    fn render_item(
        &self,
        table: &navidog_core::TableInfo,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let is_view = matches!(table.kind, navidog_core::ObjectKind::View);
        let selected = self.selected.as_deref() == Some(table.name.as_str());
        let name = table.name.clone();
        let open_name = name.clone();
        let connection_index = self.connection_index;
        let database_index = self.database_index;

        div()
            .id(SharedString::from(format!(
                "obj-{connection_index}-{database_index}-{}",
                table.name
            )))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w(px(220.0))
            .h(px(OBJECT_ROW_HEIGHT))
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, event, _window, cx| {
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                this.select_object(open_name.clone(), cx);
                if double_click {
                    this.open_selected_object(cx);
                }
            }))
            .child(tree_icon(
                if is_view {
                    "icons/views.svg"
                } else {
                    "icons/tables.svg"
                },
                if is_view {
                    theme.icon_view
                } else {
                    theme.icon_table
                },
            ))
            .child(div().overflow_hidden().whitespace_nowrap().child(name))
    }

    fn select_object(&mut self, name: String, cx: &mut Context<'_, Self>) {
        self.selected = Some(name);
        self.notify_app(cx);
        cx.notify();
    }

    fn open_selected_object(&mut self, cx: &mut Context<'_, Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        let is_view = self.category == Category::Views;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                let Some(database) = app.database_name(connection_index, database_index) else {
                    return;
                };
                app.select_table(connection_index, database, name, is_view, cx);
            });
        }
    }
}

impl Render for ObjectPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (search, tables) = {
            let Some(app) = self.app.upgrade() else {
                return div().into_any_element();
            };
            let app = app.read(cx);
            let tables = app
                .connections
                .get(self.connection_index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => databases.get(self.database_index),
                    _ => None,
                })
                .map(|database| match &database.tables {
                    Loadable::Idle => Loadable::Idle,
                    Loadable::Loading => Loadable::Loading,
                    Loadable::Failed(error) => Loadable::Failed(error.clone()),
                    Loadable::Loaded(items) => Loadable::Loaded(items.clone()),
                });
            (app.object_search.clone(), tables)
        };

        let body = self.render_body(tables, &search, cx);

        let mut container = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.hscroll_drag(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.hscroll_grab.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(body);
        if self.scroll.max_offset().width > px(0.0) {
            container = container.child(self.render_hscrollbar(cx));
        }
        container.into_any_element()
    }
}
