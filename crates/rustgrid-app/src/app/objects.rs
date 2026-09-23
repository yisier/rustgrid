use super::*;

/// The object pane's current selection plus its database coordinates and whether it is a view.
fn object_selection(pane: &Entity<ObjectPane>, cx: &App) -> Option<(usize, usize, String, bool)> {
    let pane = pane.read(cx);
    pane.selected.clone().map(|name| {
        (
            pane.connection_index,
            pane.database_index,
            name,
            pane.category == Category::Views,
        )
    })
}

/// The Functions pane's selection plus its database coordinates and routine kind.
fn routine_selection(
    pane: &Entity<ObjectPane>,
    cx: &App,
) -> Option<(usize, usize, String, RoutineKind)> {
    let pane = pane.read(cx);
    let name = pane.selected.clone()?;
    let kind = pane.selected_routine?;
    Some((pane.connection_index, pane.database_index, name, kind))
}

impl AppView {
    pub(super) fn render_object_toolbar(
        &self,
        pane: &Entity<ObjectPane>,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let theme = self.theme;
        let (category, open_enabled, connection_index, database_index) = {
            let pane = pane.read(cx);
            (
                pane.category,
                pane.selected.is_some(),
                pane.connection_index,
                pane.database_index,
            )
        };
        if category == Category::Queries {
            return self.render_query_object_toolbar(cx);
        }
        if category == Category::Functions {
            return self.render_routine_object_toolbar(pane, cx);
        }
        let design_enabled = category == Category::Tables && open_enabled;
        let new_enabled = category == Category::Tables;
        let export_enabled = category == Category::Tables && open_enabled;
        let import_enabled = category == Category::Tables
            && self
                .database_name(connection_index, database_index)
                .is_some();
        let pane_for_open = pane.clone();
        let pane_for_design = pane.clone();
        let pane_for_new = pane.clone();
        let pane_for_export = pane.clone();
        let pane_for_import = pane.clone();
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
                        cx.listener(move |this, _event, _window, cx| {
                            let Some((connection_index, database_index, name, is_view)) =
                                object_selection(&pane_for_open, cx)
                            else {
                                return;
                            };
                            let Some(database) =
                                this.database_name(connection_index, database_index)
                            else {
                                return;
                            };
                            this.select_table(connection_index, database, name, is_view, cx);
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-design",
                        "icons/design_table.svg",
                        t!("object.design_table").to_string(),
                        design_enabled,
                        cx.listener(move |this, _event, _window, cx| {
                            let Some((connection_index, database_index, name, is_view)) =
                                object_selection(&pane_for_design, cx)
                            else {
                                return;
                            };
                            let Some(database) =
                                this.database_name(connection_index, database_index)
                            else {
                                return;
                            };
                            this.open_design_table(connection_index, database, name, is_view, cx);
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-new",
                        "icons/new_table.svg",
                        t!("object.new_table").to_string(),
                        new_enabled,
                        cx.listener(move |this, _event, _window, cx| {
                            let (connection_index, database_index) = {
                                let pane = pane_for_new.read(cx);
                                (pane.connection_index, pane.database_index)
                            };
                            let Some(database) =
                                this.database_name(connection_index, database_index)
                            else {
                                return;
                            };
                            this.open_new_table(connection_index, database, cx);
                        }),
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
                        import_enabled,
                        cx.listener(move |this, _event, _window, cx| {
                            let (connection_index, database_index) = {
                                let pane = pane_for_import.read(cx);
                                (pane.connection_index, pane.database_index)
                            };
                            this.open_import_wizard(connection_index, database_index, cx);
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-export",
                        "icons/export.svg",
                        t!("object.export_wizard").to_string(),
                        export_enabled,
                        cx.listener(move |this, _event, _window, cx| {
                            let Some((connection_index, database_index, name, is_view)) =
                                object_selection(&pane_for_export, cx)
                            else {
                                return;
                            };
                            if is_view {
                                return;
                            }
                            this.open_export_wizard(connection_index, database_index, &name, cx);
                        }),
                    )),
            )
            .child(self.render_object_search(cx))
    }

    /// The object toolbar shown for the `Queries` category, including when no database is open.
    pub(super) fn render_query_object_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let delete_enabled = self.query_selected_in_scope(cx);
        // The 详细列表 / 平铺网格 switch sits between the actions and the search box; the shared
        // `ui` control remembers the choice per page.
        let weak = cx.weak_entity();
        let on_select = Rc::new(
            move |mode: ViewMode, _event: &ClickEvent, _window: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |app, cx| app.set_view_mode(VIEW_PAGE_QUERIES, mode, cx));
            },
        );
        let view_mode = self.view_mode(VIEW_PAGE_QUERIES);
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
                        "icons/new_query.svg",
                        t!("main.new_query").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| this.open_new_query(cx)),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-delete-query",
                        "icons/delete_table.svg",
                        t!("connection.delete").to_string(),
                        delete_enabled,
                        cx.listener(|this, _event, _window, cx| {
                            if let Some(index) = this.saved_query_selected {
                                this.confirm_delete_saved_query(index, cx);
                            }
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-refresh-query",
                        "icons/refresh.svg",
                        t!("common.refresh").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| this.refresh_query_files(cx)),
                    )),
            )
            .child(ui::view_mode_toggle(theme, view_mode, on_select))
            .child(self.render_object_search(cx))
    }

    /// The toolbar for the Functions object list (Navicat's 设计函数 / 新建函数 / 删除函数 /
    /// 运行函数).
    fn render_routine_object_toolbar(
        &self,
        pane: &Entity<ObjectPane>,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let theme = self.theme;
        let has_selection = {
            let pane = pane.read(cx);
            pane.selected.is_some() && pane.selected_routine.is_some()
        };
        let has_database = {
            let pane = pane.read(cx);
            self.database_name(pane.connection_index, pane.database_index)
                .is_some()
        };
        let pane_for_design = pane.clone();
        let pane_for_new = pane.clone();
        let pane_for_delete = pane.clone();
        let pane_for_run = pane.clone();

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
                        "routine-design",
                        "icons/design_table.svg",
                        t!("routine.design").to_string(),
                        has_selection,
                        cx.listener(move |this, _event, _window, cx| {
                            let Some((connection_index, database_index, name, kind)) =
                                routine_selection(&pane_for_design, cx)
                            else {
                                return;
                            };
                            let Some(database) =
                                this.database_name(connection_index, database_index)
                            else {
                                return;
                            };
                            this.open_routine_by_name(connection_index, database, name, kind, cx);
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "routine-new",
                        "icons/new_table.svg",
                        t!("routine.new").to_string(),
                        has_database,
                        cx.listener(move |this, event: &ClickEvent, _window, cx| {
                            let (connection_index, database_index) = {
                                let pane = pane_for_new.read(cx);
                                (pane.connection_index, pane.database_index)
                            };
                            let position = match event {
                                ClickEvent::Mouse(mouse) => mouse.down.position,
                                _ => return,
                            };
                            this.context_menu = Some(ContextMenu {
                                target: ContextTarget::NewRoutine {
                                    connection_index,
                                    database_index,
                                },
                                position,
                            });
                            cx.notify();
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "routine-delete",
                        "icons/delete_table.svg",
                        t!("routine.delete").to_string(),
                        has_selection,
                        cx.listener(move |this, _event, _window, cx| {
                            let Some((connection_index, database_index, name, kind)) =
                                routine_selection(&pane_for_delete, cx)
                            else {
                                return;
                            };
                            this.confirm_delete_routine(
                                connection_index,
                                database_index,
                                name,
                                kind,
                                cx,
                            );
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "routine-run",
                        "icons/run.svg",
                        t!("routine.run").to_string(),
                        has_selection,
                        cx.listener(move |this, _event, _window, cx| {
                            let Some((connection_index, database_index, name, kind)) =
                                routine_selection(&pane_for_run, cx)
                            else {
                                return;
                            };
                            let Some(database) =
                                this.database_name(connection_index, database_index)
                            else {
                                return;
                            };
                            this.run_routine_by_name(connection_index, database, kind, name, cx);
                        }),
                    )),
            )
            .child(self.render_object_search(cx))
    }

    pub(super) fn render_object_search(&self, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .w(px(220.0))
            .h(px(24.0))
            .child(self.object_search_input.clone())
    }

    /// Clear the object search box and refresh the filtered list.
    pub(super) fn clear_object_search(&mut self, cx: &mut Context<'_, Self>) {
        self.object_search.clear();
        self.object_search_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.notify_object_pane(cx);
        cx.notify();
    }

    /// The content of the object status shown in the window's bottom status bar: item count on the
    /// left, connection and database on the right (Navicat layout).
    pub(super) fn render_object_status(
        &self,
        pane: &Entity<ObjectPane>,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let (connection_index, database_index, category) = {
            let pane = pane.read(cx);
            (pane.connection_index, pane.database_index, pane.category)
        };
        let connection_name = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let database_name = self
            .database_name(connection_index, database_index)
            .unwrap_or_default();
        let count = self.object_count(connection_index, database_index, category);

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px_2()
            .text_size(px(12.0))
            .child(div().text_color(rgb(theme.text)).child(format!(
                "{} {}",
                count,
                t!(category.label())
            )))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .text_color(rgb(theme.text))
                            .child(tree_icon("icons/connection.svg", theme.icon_connection))
                            .child(connection_name),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .text_color(rgb(theme.text))
                            .child(tree_icon("icons/database.svg", theme.icon_database))
                            .child(database_name),
                    ),
            )
    }

    fn object_count(
        &self,
        connection_index: usize,
        database_index: usize,
        category: Category,
    ) -> usize {
        let Some(node) = self.connections.get(connection_index) else {
            return 0;
        };
        let Loadable::Loaded(databases) = &node.databases else {
            return 0;
        };
        let Some(database) = databases.get(database_index) else {
            return 0;
        };
        if category == Category::Functions {
            return match &database.routines {
                Loadable::Loaded(routines) => routines.len(),
                _ => 0,
            };
        }
        let Loadable::Loaded(tables) = &database.tables else {
            return 0;
        };
        let want_view = match category {
            Category::Tables => false,
            Category::Views => true,
            _ => return 0,
        };
        tables
            .iter()
            .filter(|table| matches!(table.kind, rustgrid_core::ObjectKind::View) == want_view)
            .count()
    }
}

impl ObjectPane {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        connection_index: usize,
        database_index: usize,
        category: Category,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self {
            app,
            connection_index,
            database_index,
            category,
            selected: None,
            selected_routine: None,
            scroll: ScrollHandle::new(),
            hscroll_grab: None,
            focus: cx.focus_handle(),
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

    /// Render the list body from an owned snapshot of the database's tables or routines.
    fn render_body(
        &self,
        tables: Option<Loadable<Vec<rustgrid_core::TableInfo>>>,
        routines: Option<Loadable<Vec<RoutineInfo>>>,
        search: &str,
        rename: Option<RenameRow>,
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
        if self.category == Category::Functions {
            let Some(routines) = routines else {
                return empty();
            };
            return match routines {
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
                Loadable::Loaded(routines) => {
                    let rows = self.object_rows_per_column();
                    let query = search.trim().to_lowercase();
                    let mut columns = div().flex().flex_row().items_start().gap_1().p_1();
                    let mut column = div().flex().flex_col();
                    let mut count = 0usize;
                    for (index, routine) in routines.iter().enumerate() {
                        if !query.is_empty() && !routine.name.to_lowercase().contains(&query) {
                            continue;
                        }
                        if count == rows {
                            columns = columns.child(column);
                            column = div().flex().flex_col();
                            count = 0;
                        }
                        column = column.child(self.render_routine_item(index, routine, cx));
                        count += 1;
                    }
                    if count > 0 {
                        columns = columns.child(column);
                    }
                    columns.into_any_element()
                }
            };
        }
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
                    matches!(table.kind, rustgrid_core::ObjectKind::View) == want_view
                        && (query.is_empty() || table.name.to_lowercase().contains(&query))
                }) {
                    if count == rows {
                        columns = columns.child(column);
                        column = div().flex().flex_col();
                        count = 0;
                    }
                    column = column.child(self.render_item(table, rename.as_ref(), cx));
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
        let max = f32::from(self.scroll.max_offset().x);
        let scroll = -f32::from(self.scroll.offset().x);
        let (thumb_left, thumb_len) = if max > 0.0 {
            scrollbar_fractions(viewport, max, scroll)
        } else {
            (0.0, 0.0)
        };

        ui::hscrollbar_track("object-hscrollbar", theme, thumb_left, thumb_len).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                this.hscroll_begin(event.position.x, cx);
            }),
        )
    }

    fn hscroll_begin(&mut self, mouse_x: Pixels, cx: &mut Context<'_, Self>) {
        let bounds = self.scroll.bounds();
        let viewport = f32::from(bounds.size.width);
        let max = f32::from(self.scroll.max_offset().x);
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
        let max = f32::from(self.scroll.max_offset().x);
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
        table: &rustgrid_core::TableInfo,
        rename: Option<&RenameRow>,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let is_view = matches!(table.kind, rustgrid_core::ObjectKind::View);
        let selected = self.selected.as_deref() == Some(table.name.as_str());
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        let name = table.name.clone();
        let open_name = name.clone();
        let menu_name = name.clone();
        let menu_app = self.app.clone();
        // The row being renamed draws the in-place editor instead of its label. Its own label is
        // skipped so a bare (transparent) editor never ghosts the old name behind the caret.
        let rename = rename.filter(|row| {
            row.connection_index == connection_index
                && row.database_index == database_index
                && row.old_name == table.name
        });
        let editing_here = rename.is_some();
        let label: AnyElement = match rename {
            Some(row) => div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(OBJECT_ROW_HEIGHT - 2.0))
                .child(row.input.clone())
                .into_any_element(),
            None => div()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(name)
                .into_any_element(),
        };

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
            .on_click(cx.listener(move |this, event, window, cx| {
                if editing_here {
                    return;
                }
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                this.commit_pending_rename(cx);
                window.focus(&this.focus, cx);
                this.select_object(open_name.clone(), cx);
                if double_click {
                    this.open_selected_object(cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if editing_here {
                        return;
                    }
                    this.commit_pending_rename(cx);
                    window.focus(&this.focus, cx);
                    this.selected = Some(menu_name.clone());
                    let _ = menu_app.update(cx, |app, cx| {
                        app.context_menu = Some(ContextMenu {
                            target: ContextTarget::Table {
                                connection_index,
                                database_index,
                                name: menu_name.clone(),
                                is_view,
                                pane: RowPane::Objects,
                            },
                            position: event.position,
                        });
                        cx.notify();
                    });
                    cx.notify();
                }),
            )
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
            .child(label)
    }

    /// One routine row of the Functions object list.
    fn render_routine_item(
        &self,
        index: usize,
        routine: &RoutineInfo,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let selected = self.selected.as_deref() == Some(routine.name.as_str())
            && self.selected_routine == Some(routine.kind);
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        let name = routine.name.clone();
        let kind = routine.kind;
        let open_name = name.clone();
        let menu_name = name.clone();
        let menu_app = self.app.clone();

        div()
            .id(SharedString::from(format!(
                "obj-routine-{connection_index}-{database_index}-{index}"
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
            .on_click(cx.listener(move |this, event, window, cx| {
                window.focus(&this.focus, cx);
                this.select_routine(open_name.clone(), kind, cx);
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                if double_click && let Some(app) = this.app.upgrade() {
                    app.update(cx, |app, cx| {
                        let Some(database) = app.database_name(connection_index, database_index)
                        else {
                            return;
                        };
                        app.open_routine_by_name(
                            connection_index,
                            database,
                            open_name.clone(),
                            kind,
                            cx,
                        );
                    });
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.selected = Some(menu_name.clone());
                    this.selected_routine = Some(kind);
                    let _ = menu_app.update(cx, |app, cx| {
                        app.context_menu = Some(ContextMenu {
                            target: ContextTarget::Routine {
                                connection_index,
                                database_index,
                                name: menu_name.clone(),
                                kind,
                            },
                            position: event.position,
                        });
                        cx.notify();
                    });
                    cx.notify();
                }),
            )
            .child(tree_icon(
                routine_icon(kind),
                routine_icon_color(kind, theme),
            ))
            .child(div().overflow_hidden().whitespace_nowrap().child(name))
    }

    /// Select one routine in the Functions list.
    fn select_routine(&mut self, name: String, kind: RoutineKind, cx: &mut Context<'_, Self>) {
        self.selected = Some(name.clone());
        self.selected_routine = Some(kind);
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, _| {
                app.set_info_routine(connection_index, database_index, name.clone(), kind);
            });
        }
        self.notify_app(cx);
        cx.notify();
    }

    /// Commit an open in-place rename (e.g. before the click moves selection elsewhere).
    fn commit_pending_rename(&self, cx: &mut Context<'_, Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        app.update(cx, |app, cx| app.submit_rename(cx));
    }

    /// F2 starts editing the selected object's name in place.
    fn object_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control
            || keystroke.modifiers.platform
            || keystroke.modifiers.alt
            || !keystroke.key.eq_ignore_ascii_case("f2")
        {
            return;
        }
        let Some(name) = self.selected.clone() else {
            return;
        };
        cx.stop_propagation();
        self.begin_rename(name, window, cx);
    }

    /// Ask `AppView` to open the in-place rename editor for `name` in this pane's row.
    fn begin_rename(&mut self, name: String, window: &mut Window, cx: &mut Context<'_, Self>) {
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        let is_view = self.category == Category::Views;
        let Some(app) = self.app.upgrade() else {
            return;
        };
        app.update(cx, |app, cx| {
            app.begin_rename_table(
                RowPane::Objects,
                connection_index,
                database_index,
                name,
                is_view,
                window,
                cx,
            );
            app.notify_rename_pane(RowPane::Objects, cx);
        });
        cx.notify();
    }

    pub(super) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn select_object(&mut self, name: String, cx: &mut Context<'_, Self>) {
        self.selected = Some(name.clone());
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, _| {
                app.set_info_table(connection_index, database_index, name);
            });
        }
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
        let (search, tables, routines, rename) = {
            let Some(app) = self.app.upgrade() else {
                return div().into_any_element();
            };
            let app = app.read(cx);
            let database = app
                .connections
                .get(self.connection_index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => databases.get(self.database_index),
                    _ => None,
                });
            let tables = database.map(|database| match &database.tables {
                Loadable::Idle => Loadable::Idle,
                Loadable::Loading => Loadable::Loading,
                Loadable::Failed(error) => Loadable::Failed(error.clone()),
                Loadable::Loaded(items) => Loadable::Loaded(items.clone()),
            });
            let routines = database.map(|database| match &database.routines {
                Loadable::Idle => Loadable::Idle,
                Loadable::Loading => Loadable::Loading,
                Loadable::Failed(error) => Loadable::Failed(error.clone()),
                Loadable::Loaded(items) => Loadable::Loaded(items.clone()),
            });
            (
                app.object_search.clone(),
                tables,
                routines,
                app.rename_row(RowPane::Objects),
            )
        };

        let body = self.render_body(tables, routines, &search, rename, cx);

        let scroller = div()
            .id("object-scroll")
            .flex()
            .flex_col()
            .items_start()
            .flex_1()
            .min_w(px(0.0))
            .overflow_x_scroll()
            .track_scroll(&self.scroll)
            .child(body);

        let mut container = div()
            .id("object-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.object_key(event, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    // Clicking the list background takes focus so F2 works. While the rename
                    // editor is open the input owns the keyboard; its blur subscription commits.
                    if !this
                        .app
                        .upgrade()
                        .is_some_and(|app| app.read(cx).rename_edit.is_some())
                    {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
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
            .child(scroller);
        if self.scroll.max_offset().x > px(0.0) {
            container = container.child(self.render_hscrollbar(cx));
        }
        container.into_any_element()
    }
}
