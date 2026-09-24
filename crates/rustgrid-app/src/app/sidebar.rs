use super::*;

/// The connection status shown by a tree row, reduced to what rendering needs. A failed connection
/// keeps only its state: the error itself is surfaced in a dialog when it happens.
enum TreeStatus {
    Connected,
    Connecting,
    Failed,
    Disconnected,
}

/// A lightweight, owned snapshot of one connection for rendering the tree. `AppView` keeps the
/// real `ConnectionNode`; the snapshot only clones names and flags, never passwords.
struct TreeConnection {
    index: usize,
    name: String,
    driver: String,
    status: TreeStatus,
    expanded: bool,
    databases: Loadable<Vec<TreeDatabase>>,
    /// Saved queries belonging to this connection, across all its databases.
    saved_queries: Vec<TreeSavedQuery>,
}

struct TreeDatabase {
    index: usize,
    name: String,
    opened: bool,
    expanded: bool,
    categories: CategoryExpansion,
    tables: Loadable<Vec<TreeTable>>,
    routines: Loadable<Vec<RoutineInfo>>,
}

struct TreeTable {
    name: String,
    is_view: bool,
}

/// One saved-query leaf shown under a database's Queries category in the connection tree.
struct TreeSavedQuery {
    /// The database folder the query belongs to, matching a `TreeDatabase`'s name.
    database: String,
    /// The query's display name.
    name: String,
    /// The index into `AppView::query_files`, used to open it.
    index: usize,
}

fn snapshot_connections(app: &AppView) -> Vec<TreeConnection> {
    app.connections
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let status = match &node.status {
                ConnectionStatus::Connected(_) => TreeStatus::Connected,
                ConnectionStatus::Connecting => TreeStatus::Connecting,
                ConnectionStatus::Failed(_) => TreeStatus::Failed,
                ConnectionStatus::Disconnected => TreeStatus::Disconnected,
            };
            let databases = match &node.databases {
                Loadable::Idle => Loadable::Idle,
                Loadable::Loading => Loadable::Loading,
                Loadable::Failed(error) => Loadable::Failed(error.clone()),
                Loadable::Loaded(databases) => Loadable::Loaded(
                    databases
                        .iter()
                        .enumerate()
                        .map(|(index, database)| TreeDatabase {
                            index,
                            name: database.name.clone(),
                            opened: database.opened,
                            expanded: database.expanded,
                            categories: database.categories,
                            // Leaf tables are only read when the database is expanded.
                            tables: if database.expanded {
                                match &database.tables {
                                    Loadable::Idle => Loadable::Idle,
                                    Loadable::Loading => Loadable::Loading,
                                    Loadable::Failed(error) => Loadable::Failed(error.clone()),
                                    Loadable::Loaded(tables) => Loadable::Loaded(
                                        tables
                                            .iter()
                                            .map(|table| TreeTable {
                                                name: table.name.clone(),
                                                is_view: matches!(
                                                    table.kind,
                                                    rustgrid_core::ObjectKind::View
                                                ),
                                            })
                                            .collect(),
                                    ),
                                }
                            } else {
                                Loadable::Idle
                            },
                            // Routine leaves are only read when the Functions category is expanded.
                            routines: if database.expanded {
                                match &database.routines {
                                    Loadable::Idle => Loadable::Idle,
                                    Loadable::Loading => Loadable::Loading,
                                    Loadable::Failed(error) => Loadable::Failed(error.clone()),
                                    Loadable::Loaded(routines) => {
                                        Loadable::Loaded(routines.clone())
                                    }
                                }
                            } else {
                                Loadable::Idle
                            },
                        })
                        .collect(),
                ),
            };
            TreeConnection {
                index,
                name: node.profile.name.clone(),
                driver: node.profile.driver.as_str().to_string(),
                status,
                expanded: node.expanded,
                databases,
                saved_queries: app
                    .query_files
                    .iter()
                    .enumerate()
                    .filter(|(_, file)| file.connection_id == node.profile.id)
                    .map(|(index, file)| TreeSavedQuery {
                        database: file.database.clone(),
                        name: file.name.clone(),
                        index,
                    })
                    .collect(),
            }
        })
        .collect()
}

impl TreePane {
    pub(super) fn new(app: WeakEntity<AppView>, cx: &mut Context<'_, Self>) -> Self {
        Self {
            app,
            scroll: ScrollHandle::new(),
            selected: None,
            selected_table: None,
            rename_row: None,
            focus: cx.focus_handle(),
            theme: Theme::dark(),
        }
    }

    pub(super) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// F2 starts editing the selected table's name in place.
    fn tree_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control
            || keystroke.modifiers.platform
            || keystroke.modifiers.alt
            || !keystroke.key.eq_ignore_ascii_case("f2")
        {
            return;
        }
        let Some((connection_index, database_index, name, is_view)) = self.selected_table.clone()
        else {
            return;
        };
        cx.stop_propagation();
        let Some(app) = self.app.upgrade() else {
            return;
        };
        app.update(cx, |app, cx| {
            app.begin_rename_table(
                RowPane::Tree,
                connection_index,
                database_index,
                name,
                is_view,
                window,
                cx,
            );
            app.notify_rename_pane(RowPane::Tree, cx);
        });
        cx.notify();
    }

    /// Commit an open in-place rename (e.g. before a click moves the selection elsewhere).
    fn commit_pending_rename(&self, cx: &mut Context<'_, Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        app.update(cx, |app, cx| app.submit_rename(cx));
    }

    fn render_connection(
        &self,
        connection: &TreeConnection,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let index = connection.index;
        let conn_id = format!("conn-{index}");
        let selected = self.selected.as_deref() == Some(conn_id.as_str());
        let connected = matches!(connection.status, TreeStatus::Connected);

        let icon_color = match &connection.status {
            TreeStatus::Connected => theme.icon_connection,
            TreeStatus::Connecting => theme.warning,
            TreeStatus::Failed => theme.danger,
            TreeStatus::Disconnected => theme.neutral,
        };
        // A theme-independent badge color so the driver glyph keeps its contrast on both the
        // light and dark backgrounds.
        let badge = match &connection.status {
            TreeStatus::Connected => 0x2e9e5b,
            TreeStatus::Connecting => 0xb58900,
            TreeStatus::Failed => 0xd13438,
            TreeStatus::Disconnected => 0x6b6b6b,
        };

        let click_app = self.app.clone();
        let menu_app = self.app.clone();
        let row = div()
            .id(SharedString::from(conn_id.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl_1()
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .overflow_hidden()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, event, window, cx| {
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(format!("conn-{index}"));
                if let Some(app) = this.app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.clear_info_selection();
                        cx.notify();
                    });
                }
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                if double_click {
                    let _ = click_app.update(cx, |app, cx| {
                        let connected = matches!(
                            app.connections.get(index).map(|node| &node.status),
                            Some(ConnectionStatus::Connected(_))
                        );
                        if connected {
                            app.toggle_expand(index);
                        } else {
                            app.connect(index, cx);
                        }
                    });
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.selected = Some(format!("conn-{index}"));
                    let _ = menu_app.update(cx, |app, cx| {
                        app.context_menu = Some(ContextMenu {
                            target: ContextTarget::Connection(index),
                            position: event.position,
                        });
                        cx.notify();
                    });
                }),
            )
            .child(if connected {
                tree_chevron(connection.expanded, theme.chevron)
            } else {
                chevron_spacer()
            })
            .child(tree_driver_icon(&connection.driver, icon_color, badge))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(connection.name.clone()),
            );

        let body = div().flex().flex_col().w_full().child(row);

        let mut sub = div().flex().flex_col().w_full();

        if connection.expanded {
            match &connection.databases {
                Loadable::Idle => {}
                Loadable::Loading => {
                    sub = sub.child(tree_message(
                        t!("common.loading").to_string(),
                        26.0,
                        theme.text_muted,
                    ));
                }
                Loadable::Failed(error) => {
                    sub = sub.child(tree_message(error.clone(), 26.0, theme.danger));
                }
                Loadable::Loaded(databases) => {
                    for database in databases {
                        sub = sub.child(self.render_database(
                            index,
                            database,
                            &connection.saved_queries,
                            cx,
                        ));
                    }
                }
            }
        }

        body.child(sub)
    }

    fn render_database(
        &self,
        connection_index: usize,
        database: &TreeDatabase,
        saved_queries: &[TreeSavedQuery],
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let database_index = database.index;
        let db_id = format!("db-{connection_index}-{database_index}");
        let selected = self.selected.as_deref() == Some(db_id.as_str());

        let open_app = self.app.clone();
        let chevron_app = self.app.clone();
        let menu_app = self.app.clone();
        let row = div()
            .id(SharedString::from(db_id.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(18.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, event, window, cx| {
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(format!("db-{connection_index}-{database_index}"));
                if let Some(app) = this.app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.clear_info_selection();
                        cx.notify();
                    });
                }
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                if double_click {
                    let _ = open_app.update(cx, |app, cx| {
                        app.open_database(connection_index, database_index, cx);
                    });
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.selected = Some(format!("db-{connection_index}-{database_index}"));
                    let _ = menu_app.update(cx, |app, cx| {
                        app.context_menu = Some(ContextMenu {
                            target: ContextTarget::Database {
                                connection_index,
                                database_index,
                            },
                            position: event.position,
                        });
                        cx.notify();
                    });
                }),
            )
            .child(if database.opened {
                div()
                    .id(SharedString::from(format!(
                        "db-toggle-{connection_index}-{database_index}"
                    )))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(move |_this, _event, _window, cx| {
                        let _ = chevron_app.update(cx, |app, cx| {
                            app.open_database(connection_index, database_index, cx);
                        });
                    }))
                    .child(tree_chevron(database.expanded, theme.chevron))
                    .into_any_element()
            } else {
                chevron_spacer()
            })
            .child(tree_icon(
                "icons/database.svg",
                if database.opened {
                    theme.icon_database_active
                } else {
                    theme.icon_database
                },
            ))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(database.name.clone()),
            );

        let mut sub = div().flex().flex_col().w_full();

        if database.expanded {
            match &database.tables {
                Loadable::Idle => {}
                Loadable::Loading => {
                    sub = sub.child(tree_message(
                        t!("common.loading").to_string(),
                        36.0,
                        theme.text_muted,
                    ));
                }
                Loadable::Failed(error) => {
                    sub = sub.child(tree_message(error.clone(), 36.0, theme.danger));
                }
                Loadable::Loaded(_) => {
                    for category in Category::ALL {
                        sub = sub.child(self.render_category(
                            connection_index,
                            database_index,
                            category,
                            database,
                            saved_queries,
                            cx,
                        ));
                    }
                }
            }
        }

        div().flex().flex_col().w_full().child(row).child(sub)
    }

    fn render_category(
        &self,
        connection_index: usize,
        database_index: usize,
        category: Category,
        database: &TreeDatabase,
        saved_queries: &[TreeSavedQuery],
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let expanded = database.categories.get(category);
        let cat_id = format!("cat-{connection_index}-{database_index}-{}", category.id());
        let selected = self.selected.as_deref() == Some(cat_id.as_str());
        let label = t!(category.label()).to_string();
        let icon_color = match category {
            Category::Tables => theme.icon_tables,
            Category::Views => theme.icon_views,
            Category::Functions => theme.icon_functions,
            Category::Queries => theme.icon_queries,
            Category::Backups => theme.icon_backups,
        };
        let click_id = cat_id.clone();
        let app = self.app.clone();

        let row = div()
            .id(SharedString::from(cat_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(36.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(click_id.clone());
                let _ = app.update(cx, |app, cx| {
                    app.toggle_category(connection_index, database_index, category, cx);
                });
            }))
            .child(tree_chevron(expanded, theme.chevron))
            .child(tree_icon(category.icon_path(), icon_color))
            .child(div().child(label));

        let mut sub = div().flex().flex_col().w_full();

        if expanded {
            match category {
                Category::Tables | Category::Views => {
                    if let Loadable::Loaded(tables) = &database.tables {
                        let want_view = category == Category::Views;
                        let mut leaf_index = 0usize;
                        for table in tables.iter() {
                            if table.is_view != want_view {
                                continue;
                            }
                            sub = sub.child(self.render_table(
                                connection_index,
                                database_index,
                                leaf_index,
                                &database.name,
                                table,
                                cx,
                            ));
                            leaf_index += 1;
                        }
                    }
                }
                Category::Functions => match &database.routines {
                    Loadable::Idle | Loadable::Loading => {
                        sub = sub.child(tree_message(
                            t!("common.loading").to_string(),
                            52.0,
                            theme.text_muted,
                        ));
                    }
                    Loadable::Failed(error) => {
                        sub = sub.child(tree_message(error.clone(), 52.0, theme.danger));
                    }
                    Loadable::Loaded(routines) => {
                        for (index, routine) in routines.iter().enumerate() {
                            sub = sub.child(self.render_routine(
                                connection_index,
                                database_index,
                                index,
                                &database.name,
                                routine,
                                cx,
                            ));
                        }
                    }
                },
                Category::Queries => {
                    for query in saved_queries
                        .iter()
                        .filter(|query| query.database == database.name)
                    {
                        sub = sub.child(self.render_saved_query(
                            connection_index,
                            database_index,
                            query,
                            cx,
                        ));
                    }
                }
                _ => {}
            }
        }

        div().flex().flex_col().w_full().child(row).child(sub)
    }

    /// One stored routine leaf under the connection tree's Functions category.
    fn render_routine(
        &self,
        connection_index: usize,
        database_index: usize,
        routine_index: usize,
        database_name: &str,
        routine: &RoutineInfo,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let routine_id = format!("rtn-{connection_index}-{database_index}-{routine_index}");
        let selected = self.selected.as_deref() == Some(routine_id.as_str());
        let name = routine.name.clone();
        let click_name = name.clone();
        let kind = routine.kind;
        let click_id = routine_id.clone();
        let database_name = database_name.to_string();
        let app = self.app.clone();
        let menu_app = self.app.clone();
        let menu_name = routine.name.clone();

        div()
            .id(SharedString::from(routine_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(54.0))
            .pr_2()
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
                this.selected = Some(click_id.clone());
                this.selected_table = None;
                let _ = app.update(cx, |app, cx| {
                    app.set_info_routine(
                        connection_index,
                        database_index,
                        click_name.clone(),
                        kind,
                    );
                    cx.notify();
                });
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                if double_click {
                    let _ = app.update(cx, |app, cx| {
                        app.open_routine_by_name(
                            connection_index,
                            database_name.clone(),
                            click_name.clone(),
                            kind,
                            cx,
                        );
                    });
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.selected = Some(format!(
                        "rtn-{connection_index}-{database_index}-{routine_index}"
                    ));
                    this.selected_table = None;
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
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(name),
            )
    }

    fn render_table(
        &self,
        connection_index: usize,
        database_index: usize,
        table_index: usize,
        database_name: &str,
        table: &TreeTable,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let is_view = table.is_view;
        let table_id = format!("tbl-{connection_index}-{database_index}-{table_index}");
        let selected = self.selected.as_deref() == Some(table_id.as_str());
        let table_name = table.name.clone();
        let database_name = database_name.to_string();
        let click_id = table_id.clone();
        let app = self.app.clone();
        let menu_app = self.app.clone();
        let menu_name = table.name.clone();
        // The row being renamed draws the in-place editor instead of its label, so a bare
        // (transparent) editor never ghosts the old name behind the caret.
        let rename = self.rename_row.as_ref().filter(|row| {
            row.connection_index == connection_index
                && row.database_index == database_index
                && row.old_name == table.name
        });
        let editing_here = rename.is_some();
        let label: AnyElement = match rename {
            Some(row) => div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(20.0))
                .child(row.input.clone())
                .into_any_element(),
            None => div()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(table.name.clone())
                .into_any_element(),
        };

        div()
            .id(SharedString::from(table_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(54.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if editing_here {
                    return;
                }
                this.commit_pending_rename(cx);
                this.selected_table = Some((
                    connection_index,
                    database_index,
                    table_name.clone(),
                    is_view,
                ));
                window.focus(&this.focus, cx);
                this.selected = Some(click_id.clone());
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                let _ = app.update(cx, |app, cx| {
                    if double_click {
                        app.select_table(
                            connection_index,
                            database_name.clone(),
                            table_name.clone(),
                            is_view,
                            cx,
                        );
                    } else {
                        // A single click only selects the table (and updates the info pane);
                        // opening its data grid takes a double-click, like Navicat.
                        app.set_info_table(connection_index, database_index, table_name.clone());
                    }
                });
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if editing_here {
                        return;
                    }
                    this.commit_pending_rename(cx);
                    window.focus(&this.focus, cx);
                    this.selected = Some(format!(
                        "tbl-{connection_index}-{database_index}-{table_index}"
                    ));
                    this.selected_table =
                        Some((connection_index, database_index, menu_name.clone(), is_view));
                    let _ = menu_app.update(cx, |app, cx| {
                        app.context_menu = Some(ContextMenu {
                            target: ContextTarget::Table {
                                connection_index,
                                database_index,
                                name: menu_name.clone(),
                                is_view,
                                pane: RowPane::Tree,
                            },
                            position: event.position,
                        });
                        cx.notify();
                    });
                    cx.notify();
                }),
            )
            .child(chevron_spacer())
            .child(tree_icon(
                if is_view {
                    "icons/views.svg"
                } else {
                    "icons/tables.svg"
                },
                if is_view {
                    theme.icon_views
                } else {
                    theme.icon_tables
                },
            ))
            .child(label)
    }

    /// One saved-query leaf under the connection tree's Queries category. A single click selects
    /// it (and scopes the Queries main tab to its database); a double-click opens it.
    fn render_saved_query(
        &self,
        connection_index: usize,
        database_index: usize,
        query: &TreeSavedQuery,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let query_id = format!("qry-{connection_index}-{database_index}-{}", query.index);
        let selected = self.selected.as_deref() == Some(query_id.as_str());
        let name = query.name.clone();
        let click_id = query_id.clone();
        let app = self.app.clone();
        let file_index = query.index;

        div()
            .id(SharedString::from(query_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(54.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.commit_pending_rename(cx);
                this.selected_table = None;
                window.focus(&this.focus, cx);
                this.selected = Some(click_id.clone());
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                let _ = app.update(cx, |app, cx| {
                    if double_click {
                        app.open_saved_query(file_index, cx);
                    } else {
                        cx.notify();
                    }
                });
                cx.notify();
            }))
            .child(chevron_spacer())
            .child(tree_icon("icons/queries.svg", theme.icon_queries))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(name),
            )
    }
}

impl Render for TreePane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (theme, connections, rename) = {
            let Some(app) = self.app.upgrade() else {
                return div().into_any_element();
            };
            let app = app.read(cx);
            (
                app.theme,
                snapshot_connections(app),
                app.rename_row(RowPane::Tree),
            )
        };
        self.theme = theme;
        self.rename_row = rename;

        let mut list = div().flex().flex_col().w_full();
        if connections.is_empty() {
            list = list.child(
                div()
                    .w_full()
                    .p_2()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("sidebar.no_connections").to_string()),
            );
        }
        for connection in &connections {
            list = list.child(self.render_connection(connection, cx));
        }

        div()
            .id("sidebar-scroll")
            .flex()
            .flex_col()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .py_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.tree_key(event, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    // Clicking the tree background takes focus so F2 works. While the rename
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
            .child(list)
            .into_any_element()
    }
}
