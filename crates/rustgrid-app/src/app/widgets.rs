use super::*;
use gpui::{ElementId, Focusable as _};
use gpui_kit::base::SelectableText;

/// The ordered rows of a context menu before they are handed to the kit's `PopupMenu`. Keeping a
/// `child`/builder shape lets the target match below read like the old hand-drawn `div` chain.
struct MenuItemList(Vec<PopupMenuItem>);

impl MenuItemList {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn child(mut self, item: PopupMenuItem) -> Self {
        self.0.push(item);
        self
    }
}

impl IntoIterator for MenuItemList {
    type Item = PopupMenuItem;
    type IntoIter = std::vec::IntoIter<PopupMenuItem>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl AppView {
    pub(super) fn render_selectable_text(
        &self,
        id: &'static str,
        text: &str,
        color: u32,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let (start, end) = self.db_sql_selection_range();
        let mut styled = StyledText::new(text.to_string());
        if start < end {
            styled = styled.with_highlights(vec![(
                start..end,
                HighlightStyle {
                    background_color: Some(rgb(theme.tree_selected_bg).into()),
                    ..Default::default()
                },
            )]);
        }
        *self.db_sql_layout.borrow_mut() = styled.layout().clone();
        *self.db_sql_text.borrow_mut() = text.to_string();

        div()
            .id(id)
            .track_focus(&self.db_sql_focus)
            .cursor_text()
            .w_full()
            .overflow_hidden()
            .text_size(px(12.0))
            .text_color(rgb(color))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.db_sql_focus, cx);
                    let index = this.db_sql_index_for_position(event.position);
                    this.db_sql_anchor = index;
                    this.db_sql_cursor = index;
                    this.db_sql_selecting = true;
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, event, _window, cx| this.db_sql_key(event, cx)))
            .child(styled)
    }

    /// A selectable error message, used wherever a driver/validation failure is shown.
    ///
    /// Built on `gpui_base::SelectableText`, the plain-text primitive that participates in the
    /// window-scoped text selection the dialog already installs. Unlike a focus-tracking text
    /// element it adds nothing to the dialog's layout or focus, so neighbouring controls (the
    /// schema dialog's name input) keep their own state.
    pub(super) fn render_db_error(&self, error: &Option<String>) -> AnyElement {
        match error {
            Some(message) => div()
                .w_full()
                .text_size(px(12.0))
                .text_color(rgb(self.theme.danger))
                .child(SelectableText::new("db-error", message.clone()))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }

    pub(super) fn dialog_button(
        &self,
        id: &'static str,
        label: String,
        primary: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        ui::dialog_button(id, label, primary, self.theme, on_click)
    }

    pub(super) fn win_button(
        &self,
        id: impl Into<SharedString>,
        label: String,
        kind: ButtonKind,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        ui::button(id, label, kind, self.theme, on_click)
    }

    pub(super) fn toolbar_item(
        &self,
        id: impl Into<SharedString>,
        icon: &'static str,
        label: String,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        ui::toolbar_item(id, icon, label, enabled, self.theme, on_click)
    }

    /// The context menu's items, in order. Every context menu is a kit `PopupMenu` built when it
    /// opens, so rows, separators and submenus are its items instead of hand-drawn divs.
    fn context_menu_items(
        &self,
        target: &ContextTarget,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> MenuItemList {
        let mut items = MenuItemList::new();

        match target {
            ContextTarget::Connection(index) => {
                let index = *index;
                let status = self.connections.get(index).map(|node| &node.status);
                let connected = matches!(status, Some(ConnectionStatus::Connected(_)));
                let connecting = matches!(status, Some(ConnectionStatus::Connecting));
                // A connecting connection has no live session yet, but it still has an attempt in
                // flight, so it can be closed to cancel it. Only a connected one is "open"; an
                // untouched one is neither.
                let closeable = matches!(
                    status,
                    Some(ConnectionStatus::Connected(_)) | Some(ConnectionStatus::Connecting)
                );

                // Show only the action that applies: an open (or opening) connection offers just
                // 关闭连接, an untouched one just 打开连接.
                if !connected && !connecting {
                    items = items.child(self.context_item(
                        "ctx-connect",
                        t!("connection.connect").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.connect(index, cx);
                        }),
                    ));
                }

                if closeable {
                    items = items.child(self.context_item(
                        "ctx-disconnect",
                        t!("connection.disconnect").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.request_disconnect(index, cx);
                        }),
                    ));
                }

                items = items
                    .child(self.context_item(
                        "ctx-edit",
                        t!("connection.edit").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.open_edit_form(index, window, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "ctx-delete",
                        t!("connection.delete_connection").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.delete_confirm = Some(DeleteConfirm::Connection { index });
                            cx.notify();
                        }),
                    ))
                    .child(self.context_item(
                        "ctx-copy",
                        t!("connection.copy").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.open_copy_form(index, window, cx);
                        }),
                    ))
                    .child(self.color_submenu_item(index, window, cx))
                    .child(PopupMenuItem::separator())
                    .child(self.manage_group_submenu_item(index, window, cx));

                // Creating a database and refreshing the list both need a live connection, but stay
                // visible (greyed out) when disconnected. Engines without database management
                // (SQLite) do not show the create item at all; a document store's dialog asks for
                // the first collection it is created with.
                if self.driver_supports(index, DriverCapability::DatabaseManagement) {
                    items = if connected {
                        items.child(self.context_item(
                            "ctx-new-database",
                            t!("database.new").to_string(),
                            cx.listener(move |this, _event, window, cx| {
                                this.context_menu = None;
                                this.open_new_database(index, window, cx);
                            }),
                        ))
                    } else {
                        items.child(self.context_item_disabled(
                            "ctx-new-database",
                            t!("database.new").to_string(),
                        ))
                    };
                }

                items = items.child(self.context_item(
                    "ctx-new-query",
                    t!("main.new_query").to_string(),
                    cx.listener(move |this, _event, _window, cx| {
                        this.context_menu = None;
                        this.open_new_query_for_connection(index, cx);
                    }),
                ));

                items = items.child(PopupMenuItem::separator());
                items =
                    if connected {
                        items.child(self.context_item(
                            "ctx-refresh",
                            t!("connection.refresh").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.refresh_databases(index, cx);
                            }),
                        ))
                    } else {
                        items.child(self.context_item_disabled(
                            "ctx-refresh",
                            t!("connection.refresh").to_string(),
                        ))
                    };
            }
            ContextTarget::Group { id } => {
                let id = *id;
                let menu_position = position;
                items = items
                    .child(self.context_item(
                        "group-new-query",
                        t!("main.new_query").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_new_query_for_group(id, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "group-new-connection",
                        t!("group.new_connection").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.new_connection_in_group(id, menu_position, cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "group-rename",
                        t!("group.rename").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.begin_group_rename(id, window, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "group-delete",
                        t!("group.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.delete_confirm = Some(DeleteConfirm::Group { id });
                            cx.notify();
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "group-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_tree(cx);
                        }),
                    ));
            }
            ContextTarget::TreeSelection => {
                let indexes = self.tree_selection_connection_indexes();
                let any_open = indexes.iter().any(|index| {
                    matches!(
                        self.connections.get(*index).map(|node| &node.status),
                        Some(ConnectionStatus::Connected(_)) | Some(ConnectionStatus::Connecting)
                    )
                });
                let any_closed = indexes.iter().any(|index| {
                    matches!(
                        self.connections.get(*index).map(|node| &node.status),
                        Some(ConnectionStatus::Disconnected)
                    )
                });
                let deletable = !indexes.is_empty() || !self.tree_selection_group_ids().is_empty();
                items = items.child(if any_closed {
                    self.context_item(
                        "tree-sel-connect",
                        t!("connection.connect").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.connect_tree_selection(cx);
                        }),
                    )
                } else {
                    self.context_item_disabled(
                        "tree-sel-connect",
                        t!("connection.connect").to_string(),
                    )
                });
                items = items.child(if any_open {
                    self.context_item(
                        "tree-sel-disconnect",
                        t!("connection.disconnect").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.request_disconnect_tree_selection(cx);
                        }),
                    )
                } else {
                    self.context_item_disabled(
                        "tree-sel-disconnect",
                        t!("connection.disconnect").to_string(),
                    )
                });
                items = items.child(self.import_connections_menu_item(window, cx));
                items = items.child(if deletable {
                    self.context_item(
                        "tree-sel-delete",
                        t!("connection.delete_connection").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            let connections = this.tree_selection_connection_profile_ids();
                            let groups = this.tree_selection_group_ids();
                            this.delete_confirm = Some(DeleteConfirm::TreeSelection {
                                connections,
                                groups,
                            });
                            cx.notify();
                        }),
                    )
                } else {
                    self.context_item_disabled(
                        "tree-sel-delete",
                        t!("connection.delete_connection").to_string(),
                    )
                });
                items = items
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "tree-sel-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_tree(cx);
                        }),
                    ));
            }
            ContextTarget::TreeBlank => {
                items = items
                    .child(self.context_item(
                        "tree-new-group",
                        t!("group.new").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.add_group(window, cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.import_connections_menu_item(window, cx))
                    .child(self.context_item(
                        "tree-export-connections",
                        t!("connection.export_connection").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.import_submenu = false;
                            this.open_export_connections_window(cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "tree-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_tree(cx);
                        }),
                    ));
            }
            ContextTarget::Database {
                connection_index,
                database_index,
            } => {
                let ci = *connection_index;
                let di = *database_index;
                let key_value = self.driver_supports(ci, DriverCapability::KeyValue);
                let documents = self.driver_supports(ci, DriverCapability::Documents);
                let schema_engine = self.driver_supports(ci, DriverCapability::Schemas);
                let opened = self
                    .connections
                    .get(ci)
                    .and_then(|node| match &node.databases {
                        Loadable::Loaded(databases) => databases.get(di),
                        _ => None,
                    })
                    .map(|database| database.opened)
                    .unwrap_or(false);

                if opened {
                    items = items.child(self.context_item(
                        "db-close",
                        t!("database.close").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.close_database(ci, di, cx);
                        }),
                    ));
                } else {
                    items = items.child(self.context_item(
                        "db-open",
                        t!("database.open").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_database(ci, di, cx);
                        }),
                    ));
                }

                if documents {
                    // A document store's database level creates collections, not tables:
                    // 新建集合 is the action that applies here (the connection-level 新建数据库
                    // asks for the database and its first collection together), and 删除数据库
                    // stays available (MongoDB drops one).
                    items = items.child(self.context_item(
                        "db-new-collection",
                        t!("object.new_collection").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_new_collection(ci, di, cx);
                        }),
                    ));
                }

                if self.driver_supports(ci, DriverCapability::DatabaseManagement) {
                    if !documents {
                        items = items
                            .child(self.context_item(
                                "db-edit",
                                t!("database.edit").to_string(),
                                cx.listener(move |this, _event, window, cx| {
                                    this.context_menu = None;
                                    this.open_edit_database(ci, di, window, cx);
                                }),
                            ))
                            .child(self.context_item(
                                "db-new",
                                t!("database.new").to_string(),
                                cx.listener(move |this, _event, window, cx| {
                                    this.context_menu = None;
                                    this.open_new_database(ci, window, cx);
                                }),
                            ));
                    }
                    items = items.child(self.context_item(
                        "db-delete",
                        t!("database.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_delete_database(ci, di, cx);
                        }),
                    ));
                }
                items = items
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "db-new-query",
                        t!("main.new_query").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_new_query_for_database(ci, di, cx);
                        }),
                    ));
                if key_value {
                    items = items
                        .child(self.context_item(
                            "db-new-key",
                            t!("redis.key.new").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.start_new_key_dialog(ci, di, None, cx);
                            }),
                        ))
                        .child(self.context_item(
                            "db-refresh",
                            t!("database.refresh").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.refresh_databases(ci, cx);
                            }),
                        ));
                } else {
                    if !documents {
                        items = items.child(self.context_item(
                            "db-run-sql",
                            t!("database.run_sql_file").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.open_run_sql_file(ci, di, None, cx);
                            }),
                        ));
                        // A schema-qualified engine dumps one schema at a time, so 转储 SQL 文件
                        // lives on the schema row (the schema menu) instead of the database row.
                        if !schema_engine {
                            items = items.child(
                                self.dump_sql_submenu_item(ci, di, None, None, false, window, cx),
                            );
                        }
                    }
                    items = items.child(self.context_item(
                        "db-refresh",
                        t!("database.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_databases(ci, cx);
                        }),
                    ));
                }
                if self.driver_supports(ci, DriverCapability::Schemas) {
                    items = items
                        .child(PopupMenuItem::separator())
                        .child(self.context_item(
                            "db-new-schema",
                            t!("database.new_schema").to_string(),
                            cx.listener(move |this, _event, window, cx| {
                                this.context_menu = None;
                                this.open_new_schema(ci, di, window, cx);
                            }),
                        ));
                }
            }
            ContextTarget::KeyNamespace {
                connection_index,
                database_index,
                namespace,
            } => {
                let ci = *connection_index;
                let di = *database_index;
                let namespace = namespace.clone();
                items = items
                    .child(self.context_item(
                        "key-namespace-new-key",
                        t!("redis.key.new").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.start_new_key_dialog(ci, di, Some(namespace.clone()), cx);
                        }),
                    ))
                    .child(self.context_item(
                        "key-namespace-refresh",
                        t!("common.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_tree(cx);
                        }),
                    ));
            }
            ContextTarget::Schema {
                connection_index,
                database_index,
                schema,
            } => {
                let ci = *connection_index;
                let di = *database_index;
                let schema = schema.clone();
                let new_query_schema = schema.clone();
                let open_schema_name = schema.clone();
                let close_schema_name = schema.clone();
                // Show only the action that applies: an already-open schema can be closed, a closed
                // one can be opened (mirrors the database row's menu).
                let opened = self
                    .connections
                    .get(ci)
                    .and_then(|node| match &node.databases {
                        Loadable::Loaded(databases) => databases.get(di),
                        _ => None,
                    })
                    .is_some_and(|database| database.opened_schemas.contains(&schema));
                if opened {
                    items = items.child(self.context_item(
                        "schema-close",
                        t!("database.close_schema").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.close_schema(ci, di, close_schema_name.clone(), cx);
                        }),
                    ));
                } else {
                    items = items.child(self.context_item(
                        "schema-open",
                        t!("database.open_schema").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_schema(ci, di, open_schema_name.clone(), cx);
                        }),
                    ));
                }
                items = items
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "schema-new",
                        t!("database.new_schema").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.open_new_schema(ci, di, window, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "schema-new-query",
                        t!("main.new_query").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_new_query_for_schema(ci, di, new_query_schema.clone(), cx);
                        }),
                    ))
                    .child(self.context_item(
                        "schema-run-sql",
                        t!("database.run_sql_file").to_string(),
                        cx.listener({
                            let schema = schema.clone();
                            move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.open_run_sql_file(ci, di, Some(schema.clone()), cx);
                            }
                        }),
                    ))
                    .child(self.dump_sql_submenu_item(
                        ci,
                        di,
                        None,
                        Some(schema.clone()),
                        false,
                        window,
                        cx,
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "schema-delete",
                        t!("database.delete_schema").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.confirm_delete_schema(ci, di, schema.clone(), cx);
                        }),
                    ));
            }
            ContextTarget::Table {
                connection_index,
                database_index,
                name,
                is_view,
                pane,
            } => {
                let ci = *connection_index;
                let di = *database_index;
                let is_view = *is_view;
                let pane = *pane;
                let open_name = name.clone();
                let design_name = name.clone();
                let drop_name = name.clone();
                let empty_name = name.clone();
                let truncate_name = name.clone();
                let rename_name = name.clone();
                let export_name = name.clone();
                let import_target = name.clone();
                let info_name = name.clone();
                let key_value = self.driver_supports(ci, DriverCapability::KeyValue);
                let documents = self.driver_supports(ci, DriverCapability::Documents);

                items = items.child(
                    self.context_item(
                        "table-open",
                        t!(if is_view {
                            "view.open"
                        } else if key_value {
                            "redis.key.open"
                        } else if documents {
                            "object.open_collection"
                        } else {
                            "object.open_table"
                        })
                        .to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            let Some(database) = this.database_name(ci, di) else {
                                return;
                            };
                            this.select_table(ci, database, open_name.clone(), is_view, cx);
                        }),
                    ),
                );

                // A view is designed with the view designer, exported from its data and dropped
                // with `DROP VIEW`; it has no rows of its own, so the table-only operations
                // (Empty/Truncate/Rename/Import) are not offered.
                if is_view {
                    let view_design_name = design_name.clone();
                    let view_export_name = export_name.clone();
                    let view_drop_name = drop_name.clone();
                    items = items
                        .child(self.context_item(
                            "view-design",
                            t!("view.design").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                let Some(database) = this.database_name(ci, di) else {
                                    return;
                                };
                                this.open_view(ci, database, view_design_name.clone(), cx);
                            }),
                        ))
                        .child(PopupMenuItem::separator())
                        .child(self.context_item(
                            "view-export",
                            t!("object.export_wizard").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.open_export_wizard(
                                    ci,
                                    di,
                                    std::slice::from_ref(&view_export_name),
                                    cx,
                                );
                            }),
                        ))
                        .child(PopupMenuItem::separator())
                        .child(self.context_item(
                            "view-drop",
                            t!("view.delete").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.confirm_delete_view(ci, di, view_drop_name.clone(), cx);
                            }),
                        ));
                } else if documents {
                    // A document store designs a collection (name + validator), not a SQL table.
                    items = items.child(self.context_item(
                        "table-design",
                        t!("object.design_collection").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_design_collection(ci, di, design_name.clone(), cx);
                        }),
                    ));
                } else if self.driver_supports_table_designer(ci) {
                    items = items.child(self.context_item(
                        "table-design",
                        t!("object.design_table").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            let Some(database) = this.database_name(ci, di) else {
                                return;
                            };
                            this.open_design_table(ci, database, design_name.clone(), is_view, cx);
                        }),
                    ));
                }

                if !is_view && key_value {
                    // A Redis key has no rows to empty/truncate and no SQL import/export: offer
                    // TTL, delete and rename, matching the key list's own toolbar.
                    let ttl_name = name.clone();
                    items = items
                        .child(PopupMenuItem::separator())
                        .child(self.context_item(
                            "key-ttl",
                            t!("redis.tools.key_apply_ttl").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.start_key_ttl_dialog(ci, di, ttl_name.clone(), cx);
                            }),
                        ))
                        .child(PopupMenuItem::separator())
                        .child(self.context_item(
                            "table-drop",
                            t!("redis.key.delete").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                let selected = this.objects_selection.contains(&drop_name);
                                let mut names = if selected {
                                    this.objects_selection.items()
                                } else {
                                    Vec::new()
                                };
                                if names.is_empty() {
                                    names.push(drop_name.clone());
                                }
                                this.delete_confirm = Some(DeleteConfirm::Tables {
                                    connection_index: ci,
                                    database_index: di,
                                    names,
                                });
                                cx.notify();
                            }),
                        ))
                        .child(self.context_item(
                            "table-rename",
                            t!("redis.key.rename").to_string(),
                            cx.listener(move |this, _event, window, cx| {
                                this.context_menu = None;
                                this.begin_rename_table(
                                    pane,
                                    ci,
                                    di,
                                    rename_name.clone(),
                                    is_view,
                                    window,
                                    cx,
                                );
                                this.notify_rename_pane(pane, cx);
                                cx.notify();
                            }),
                        ));
                } else if !is_view {
                    items = items
                        .child(PopupMenuItem::separator())
                        .child(self.context_item(
                            "table-export",
                            t!("object.export_wizard").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.open_export_wizard(
                                    ci,
                                    di,
                                    std::slice::from_ref(&export_name),
                                    cx,
                                );
                            }),
                        ))
                        .child(self.context_item(
                            "table-import",
                            t!("object.import_wizard").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.open_import_wizard(ci, di, Some(import_target.clone()), cx);
                            }),
                        ))
                        .child(PopupMenuItem::separator())
                        .child(
                            self.context_item(
                                "table-drop",
                                t!(if documents {
                                    "object.delete_collection"
                                } else {
                                    "object.delete_table"
                                })
                                .to_string(),
                                cx.listener(move |this, _event, _window, cx| {
                                    this.context_menu = None;
                                    // Drop every selected table, not just the right-clicked one (when
                                    // the clicked row is part of the multi-selection).
                                    let selected = this.objects_selection.contains(&drop_name);
                                    let mut names = if selected {
                                        this.objects_selection.items()
                                    } else {
                                        Vec::new()
                                    };
                                    if names.is_empty() {
                                        names.push(drop_name.clone());
                                    }
                                    this.delete_confirm = Some(DeleteConfirm::Tables {
                                        connection_index: ci,
                                        database_index: di,
                                        names,
                                    });
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            self.context_item(
                                "table-empty",
                                t!(if documents {
                                    "object.empty_collection"
                                } else {
                                    "object.empty_table"
                                })
                                .to_string(),
                                cx.listener(move |this, _event, _window, cx| {
                                    this.context_menu = None;
                                    let selected = pane == RowPane::Objects
                                        && this.objects_selection.contains(&empty_name);
                                    let mut names = if selected {
                                        this.objects_selection.items()
                                    } else {
                                        Vec::new()
                                    };
                                    if names.is_empty() {
                                        names.push(empty_name.clone());
                                    }
                                    this.delete_confirm = Some(if names.len() == 1 {
                                        DeleteConfirm::Table {
                                            connection_index: ci,
                                            database_index: di,
                                            name: names.remove(0),
                                            operation: TableOperation::Empty,
                                        }
                                    } else {
                                        DeleteConfirm::TablesOperation {
                                            connection_index: ci,
                                            database_index: di,
                                            names,
                                            operation: TableOperation::Empty,
                                        }
                                    });
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            self.context_item(
                                "table-truncate",
                                t!(if documents {
                                    "object.truncate_collection"
                                } else {
                                    "object.truncate_table"
                                })
                                .to_string(),
                                cx.listener(move |this, _event, _window, cx| {
                                    this.context_menu = None;
                                    let selected = pane == RowPane::Objects
                                        && this.objects_selection.contains(&truncate_name);
                                    let mut names = if selected {
                                        this.objects_selection.items()
                                    } else {
                                        Vec::new()
                                    };
                                    if names.is_empty() {
                                        names.push(truncate_name.clone());
                                    }
                                    this.delete_confirm = Some(if names.len() == 1 {
                                        DeleteConfirm::Table {
                                            connection_index: ci,
                                            database_index: di,
                                            name: names.remove(0),
                                            operation: TableOperation::Truncate,
                                        }
                                    } else {
                                        DeleteConfirm::TablesOperation {
                                            connection_index: ci,
                                            database_index: di,
                                            names,
                                            operation: TableOperation::Truncate,
                                        }
                                    });
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(PopupMenuItem::separator())
                        .child(
                            self.context_item(
                                "table-rename",
                                t!(if documents {
                                    "object.rename_collection"
                                } else {
                                    "object.rename_table"
                                })
                                .to_string(),
                                cx.listener(move |this, _event, window, cx| {
                                    this.context_menu = None;
                                    this.begin_rename_table(
                                        pane,
                                        ci,
                                        di,
                                        rename_name.clone(),
                                        is_view,
                                        window,
                                        cx,
                                    );
                                    this.notify_rename_pane(pane, cx);
                                    cx.notify();
                                }),
                            ),
                        );
                }
                // A SQL table/view dumps its own script: the database menu's 转储 SQL 文件 narrowed
                // to the right-clicked object. A document store speaks no SQL and a Redis key has
                // neither a DDL nor a data script, so neither offers it.
                if !key_value && !documents {
                    items = items.child(self.dump_sql_submenu_item(
                        ci,
                        di,
                        Some(name.clone()),
                        None,
                        is_view,
                        window,
                        cx,
                    ));
                }
                items = items
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "table-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            let category = if is_view {
                                Category::Views
                            } else {
                                Category::Tables
                            };
                            this.refresh_object_category(category, ci, di, cx);
                        }),
                    ));
                // 对象信息 opens the right-hand details pane for this table/view/collection. A
                // Redis key has no catalog entry to describe (no status and no DDL), so it is not
                // offered there.
                if !key_value {
                    items = items.child(self.context_item(
                        "table-info",
                        t!("object.object_info").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_table_info(ci, di, info_name.clone(), cx);
                        }),
                    ));
                }
            }
            ContextTarget::TableCategory {
                connection_index,
                database_index,
                schema,
            } => {
                let ci = *connection_index;
                let di = *database_index;
                let _ = schema;
                let new_database = self.database_name(ci, di);
                let documents = self.driver_supports(ci, DriverCapability::Documents);
                if documents {
                    items = items
                        .child(self.context_item(
                            "tablecat-new",
                            t!("object.new_collection").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.open_new_collection(ci, di, cx);
                            }),
                        ))
                        .child(PopupMenuItem::separator());
                } else if self.driver_supports_table_designer(ci) {
                    items = items
                        .child(self.context_item(
                            "tablecat-new",
                            t!("object.new_table").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                let Some(database) = new_database.clone() else {
                                    return;
                                };
                                this.open_new_table(ci, database, cx);
                            }),
                        ))
                        .child(PopupMenuItem::separator());
                }
                items = items
                    .child(self.context_item(
                        "tablecat-import",
                        t!("object.import_wizard").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_import_wizard(ci, di, None, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "tablecat-export",
                        t!("object.export_wizard").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_export_wizard(ci, di, &[], cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "tablecat-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_object_category(Category::Tables, ci, di, cx);
                        }),
                    ));
            }
            ContextTarget::ObjectCategory {
                connection_index,
                database_index,
                category,
            } => {
                let ci = *connection_index;
                let di = *database_index;
                let category = *category;
                items = items.child(self.context_item(
                    "objcat-refresh",
                    t!("connection.refresh").to_string(),
                    cx.listener(move |this, _event, _window, cx| {
                        this.context_menu = None;
                        match category {
                            Category::Queries => this.refresh_query_files_feedback(cx),
                            Category::Backups => this.refresh_selected_backup(cx),
                            other => this.refresh_object_category(other, ci, di, cx),
                        }
                    }),
                ));
            }
            ContextTarget::Routine {
                connection_index,
                database_index,
                name,
                kind,
            } => {
                let ci = *connection_index;
                let di = *database_index;
                let kind = *kind;
                let design_name = name.clone();
                let run_name = name.clone();
                let drop_name = name.clone();
                items = items
                    .child(self.context_item(
                        "routine-design",
                        t!("routine.design").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            let Some(database) = this.database_name(ci, di) else {
                                return;
                            };
                            this.open_routine_by_name(ci, database, design_name.clone(), kind, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "routine-run",
                        t!("routine.run").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            let Some(database) = this.database_name(ci, di) else {
                                return;
                            };
                            this.run_routine_by_name(ci, database, kind, run_name.clone(), cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "routine-delete",
                        t!("routine.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.confirm_delete_routine(ci, di, drop_name.clone(), kind, cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "routine-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_object_category(Category::Functions, ci, di, cx);
                        }),
                    ));
            }
            ContextTarget::NewRoutine {
                connection_index,
                database_index,
            } => {
                let ci = *connection_index;
                let di = *database_index;
                items = items
                    .child(self.context_item(
                        "new-function",
                        t!("routine.new_function").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            let Some(database) = this.database_name(ci, di) else {
                                return;
                            };
                            this.open_new_routine(ci, database, RoutineKind::Function, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "new-procedure",
                        t!("routine.new_procedure").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            let Some(database) = this.database_name(ci, di) else {
                                return;
                            };
                            this.open_new_routine(ci, database, RoutineKind::Procedure, cx);
                        }),
                    ));
            }
            ContextTarget::BackupFile { index } => {
                let index = *index;
                items = items
                    .child(self.context_item(
                        "backup-ctx-restore",
                        t!("backup.restore").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_restore_backup(index, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "backup-ctx-new",
                        t!("backup.new").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_new_backup(cx);
                        }),
                    ))
                    .child(self.context_item(
                        "backup-ctx-delete",
                        t!("backup.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.backup_selected = Some(BackupSelection::File(index));
                            this.confirm_delete_backup(cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "backup-ctx-extract",
                        t!("backup.extract_sql").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_extract_sql(index, cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "backup-ctx-copy",
                        t!("backup.copy").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.copy_backup_file(index);
                            cx.notify();
                        }),
                    ))
                    .child(self.context_item(
                        "backup-ctx-rename",
                        t!("backup.rename").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.begin_backup_rename(BackupSelection::File(index), window, cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "backup-ctx-reveal",
                        t!("backup.reveal").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.reveal_backup(index);
                            cx.notify();
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "backup-ctx-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_selected_backup(cx);
                        }),
                    ))
                    .child(self.context_item(
                        "backup-ctx-info",
                        t!("backup.object_info").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_backup_info(index, cx);
                        }),
                    ));
            }
            ContextTarget::BackupConfig { index } => {
                let index = *index;
                items = items
                    .child(self.context_item(
                        "backup-config-ctx-delete",
                        t!("backup.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.backup_selected = Some(BackupSelection::Config(index));
                            this.confirm_delete_backup(cx);
                        }),
                    ))
                    .child(self.context_item(
                        "backup-config-ctx-rename",
                        t!("backup.rename").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.begin_backup_rename(BackupSelection::Config(index), window, cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "backup-config-ctx-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_selected_backup(cx);
                        }),
                    ))
                    .child(self.context_item(
                        "backup-config-ctx-info",
                        t!("backup.object_info").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.backup_selected = Some(BackupSelection::Config(index));
                            this.set_info_open(true, cx);
                        }),
                    ));
            }
            ContextTarget::SyncList => {
                items = items.child(self.context_item(
                    "synclist-ctx-refresh",
                    t!("connection.refresh").to_string(),
                    cx.listener(|this, _event, _window, cx| {
                        this.context_menu = None;
                        this.sync_recompare_all(cx);
                    }),
                ));
            }
            ContextTarget::QueryFile { index } => {
                let index = *index;
                items = items
                    .child(self.context_item(
                        "queryfile-ctx-open",
                        t!("query.open_query").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_saved_query(index, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "queryfile-ctx-delete",
                        t!("query.delete_query").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.select_query_one(index);
                            this.confirm_delete_saved_query(index, cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "queryfile-ctx-copy",
                        t!("backup.copy").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.copy_query_file(index);
                            cx.notify();
                        }),
                    ))
                    .child(self.context_item(
                        "queryfile-ctx-rename",
                        t!("backup.rename").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.begin_rename_query(index, window, cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "queryfile-ctx-reveal",
                        t!("backup.reveal").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.reveal_query(index);
                            cx.notify();
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "queryfile-ctx-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_selected_query(cx);
                        }),
                    ))
                    .child(self.context_item(
                        "queryfile-ctx-info",
                        t!("backup.object_info").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_query_info(index, cx);
                        }),
                    ));
            }
            ContextTarget::TransferConfig { index } => {
                let index = *index;
                items = items
                    .child(self.context_item(
                        "transfer-ctx-open",
                        t!("backup.open").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_transfer_config(index, cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "transfer-ctx-delete",
                        t!("sync.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            if let Some(key) = this.transfer_key(index) {
                                this.hit_transfer(&key, Modifiers::default(), cx);
                            }
                            this.confirm_delete_transfer(cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "transfer-ctx-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_transfer_configs(cx);
                        }),
                    ));
            }
            ContextTarget::TransferList => {
                items = items
                    .child(self.context_item(
                        "transferlist-ctx-new",
                        t!("sync.new").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_new_transfer(cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "transferlist-ctx-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_transfer_configs(cx);
                        }),
                    ));
            }
            ContextTarget::BackupList => {
                items = items
                    .child(self.context_item(
                        "backuplist-ctx-new",
                        t!("backup.new").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_new_backup(cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "backuplist-ctx-reveal",
                        t!("backup.reveal").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.reveal_backup_folder(cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "backuplist-ctx-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_selected_backup(cx);
                        }),
                    ));
            }
            ContextTarget::UserList => {
                items = items.child(self.context_item(
                    "userlist-ctx-refresh",
                    t!("connection.refresh").to_string(),
                    cx.listener(|this, _event, _window, cx| {
                        this.context_menu = None;
                        this.refresh_users_feedback(cx);
                    }),
                ));
            }
            ContextTarget::QueryList => {
                items = items
                    .child(self.context_item(
                        "querylist-ctx-new",
                        t!("main.new_query").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_new_query(cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "querylist-ctx-reveal",
                        t!("backup.reveal").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.reveal_query_folder(cx);
                        }),
                    ))
                    .child(PopupMenuItem::separator())
                    .child(self.context_item(
                        "querylist-ctx-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_selected_query(cx);
                        }),
                    ));
            }
            ContextTarget::Grid { grid_id, cell } => {
                let grid_id = *grid_id;
                let cell = *cell;
                let grid = self
                    .grids
                    .iter()
                    .find(|grid| grid.read(cx).state.id == grid_id)
                    .cloned();
                if let Some(grid) = grid {
                    let (has_selection, editable, deletable, key_value, can_filter) = {
                        let g = grid.read(cx);
                        let cell_is_data = cell.is_some_and(|(row, _)| {
                            matches!(g.row_at(row), Some(GridRow::Data(_)))
                        });
                        (
                            g.state.selection.is_some(),
                            g.state.editable,
                            g.state.editable
                                && g.state.selection.is_some()
                                && !g.state.rows.is_empty(),
                            g.state.key_status.is_some(),
                            g.state.sql.is_none() && g.state.key_status.is_none() && cell_is_data,
                        )
                    };
                    let for_filter = grid.clone();
                    let for_delete = grid.clone();
                    let for_duplicate = grid.clone();
                    let for_copy = grid.clone();
                    let for_insert = grid.clone();
                    let for_paste = grid.clone();
                    let for_refresh = grid.clone();
                    let for_null = grid.clone();
                    let delete_label = if key_value {
                        t!("redis.key.delete_selected").to_string()
                    } else {
                        t!("grid.delete_rows").to_string()
                    };
                    if can_filter && let Some((row, col)) = cell {
                        items = items
                            .child(self.context_item(
                                "grid-ctx-filter",
                                t!("grid.filter").to_string(),
                                cx.listener(move |this, _event, _window, cx| {
                                    this.context_menu = None;
                                    for_filter
                                        .update(cx, |grid, cx| grid.filter_cell(row, col, cx));
                                    cx.notify();
                                }),
                            ))
                            .child(PopupMenuItem::separator());
                    }
                    items = items.child(if deletable {
                        self.context_item(
                            "grid-ctx-delete",
                            delete_label.clone(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                // The listener already runs inside `AppView::update`: take the
                                // rows from the grid and set the confirmation here. Letting the
                                // grid update `AppView` (as `open_delete_confirm` does) would
                                // re-enter this entity's update and abort the app.
                                let rows =
                                    for_delete.update(cx, |grid, cx| grid.take_delete_rows(cx));
                                if let Some(rows) = rows {
                                    this.delete_confirm =
                                        Some(DeleteConfirm::Rows { grid_id, rows });
                                }
                                cx.notify();
                            }),
                        )
                    } else {
                        self.context_item_disabled("grid-ctx-delete", delete_label)
                    });
                    if !key_value {
                        items = items.child(if deletable {
                            self.context_item(
                                "grid-ctx-duplicate",
                                t!("grid.duplicate_row").to_string(),
                                cx.listener(move |this, _event, window, cx| {
                                    this.context_menu = None;
                                    for_duplicate
                                        .update(cx, |grid, cx| grid.duplicate_row(window, cx));
                                    cx.notify();
                                }),
                            )
                        } else {
                            self.context_item_disabled(
                                "grid-ctx-duplicate",
                                t!("grid.duplicate_row").to_string(),
                            )
                        });
                    }
                    items = items.child(if has_selection {
                        self.context_item(
                            "grid-ctx-copy",
                            t!("grid.copy").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                for_copy.update(cx, |grid, cx| grid.copy_selection(cx));
                                cx.notify();
                            }),
                        )
                    } else {
                        self.context_item_disabled("grid-ctx-copy", t!("grid.copy").to_string())
                    });
                    if !key_value {
                        items = items.child(if has_selection {
                            self.context_item(
                                "grid-ctx-copy-insert",
                                t!("grid.copy_insert").to_string(),
                                cx.listener(move |this, _event, _window, cx| {
                                    this.context_menu = None;
                                    for_insert
                                        .update(cx, |grid, cx| grid.copy_selection_as_insert(cx));
                                    cx.notify();
                                }),
                            )
                        } else {
                            self.context_item_disabled(
                                "grid-ctx-copy-insert",
                                t!("grid.copy_insert").to_string(),
                            )
                        });
                    }
                    items = items.child(if editable {
                        self.context_item(
                            "grid-ctx-paste",
                            t!("grid.paste").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                for_paste.update(cx, |grid, cx| grid.paste_clipboard(cx));
                                cx.notify();
                            }),
                        )
                    } else {
                        self.context_item_disabled("grid-ctx-paste", t!("grid.paste").to_string())
                    });
                    if !key_value {
                        items = items.child(if editable && has_selection {
                            self.context_item(
                                "grid-ctx-null",
                                t!("grid.set_null").to_string(),
                                cx.listener(move |this, _event, _window, cx| {
                                    this.context_menu = None;
                                    for_null.update(cx, |grid, cx| grid.set_selection_null(cx));
                                    cx.notify();
                                }),
                            )
                        } else {
                            self.context_item_disabled(
                                "grid-ctx-null",
                                t!("grid.set_null").to_string(),
                            )
                        });
                    }
                    items = items
                        .child(PopupMenuItem::separator())
                        .child(self.context_item(
                            "grid-ctx-refresh",
                            t!("grid.refresh").to_string(),
                            cx.listener(move |this, _event, window, cx| {
                                this.context_menu = None;
                                // Deferred: `refresh` asks for the unsaved-changes confirmation
                                // through `AppView`, and this listener is already inside an
                                // `AppView::update` (a direct call would re-enter it and panic).
                                let grid = for_refresh.clone();
                                cx.defer_in(window, move |_this, _window, cx| {
                                    grid.update(cx, |grid, cx| grid.refresh(cx));
                                });
                                cx.notify();
                            }),
                        ));
                }
            }
            ContextTarget::GridHeader { grid_id, column } => {
                let grid_id = *grid_id;
                let column = *column;
                let grid = self
                    .grids
                    .iter()
                    .find(|grid| grid.read(cx).state.id == grid_id)
                    .cloned();
                if let Some(grid) = grid {
                    let (valid, on_frozen_column, has_frozen, can_hide, can_filter) = {
                        let g = grid.read(cx);
                        (
                            column < g.state.visible_columns,
                            g.state
                                .columns
                                .get(column)
                                .is_some_and(|column| g.state.is_frozen_column(&column.name)),
                            !g.state.frozen_columns.is_empty(),
                            g.state.sql.is_none() && g.state.visible_columns > 1,
                            g.state.sql.is_none(),
                        )
                    };
                    let for_filter = grid.clone();
                    let for_freeze = grid.clone();
                    let for_unfreeze = grid.clone();
                    let for_unfreeze_all = grid.clone();
                    let for_hide = grid.clone();
                    let for_refresh = grid.clone();
                    // 筛选 opens the WHERE panel with this field already added, the way Navicat's
                    // header menu does, so a filter can be started from the column itself.
                    items = items.child(if valid && can_filter {
                        self.context_item(
                            "grid-head-filter",
                            t!("grid.filter").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                for_filter.update(cx, |grid, cx| grid.filter_column(column, cx));
                                cx.notify();
                            }),
                        )
                    } else {
                        self.context_item_disabled(
                            "grid-head-filter",
                            t!("grid.filter").to_string(),
                        )
                    });
                    items = items.child(PopupMenuItem::separator());
                    // 冻结列 / 取消冻结 are one item: the frozen column itself offers 取消冻结,
                    // every other column offers 冻结列.
                    items = items.child(if on_frozen_column {
                        self.context_item(
                            "grid-head-unfreeze",
                            t!("grid.unfreeze_column").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                for_unfreeze
                                    .update(cx, |grid, cx| grid.unfreeze_columns(column, cx));
                                cx.notify();
                            }),
                        )
                    } else if valid {
                        self.context_item(
                            "grid-head-freeze",
                            t!("grid.freeze_column").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                for_freeze.update(cx, |grid, cx| grid.freeze_column(column, cx));
                                cx.notify();
                            }),
                        )
                    } else {
                        self.context_item_disabled(
                            "grid-head-freeze",
                            t!("grid.freeze_column").to_string(),
                        )
                    });
                    items = items.child(if has_frozen {
                        self.context_item(
                            "grid-head-unfreeze-all",
                            t!("grid.unfreeze_all_columns").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                for_unfreeze_all
                                    .update(cx, |grid, cx| grid.unfreeze_all_columns(cx));
                                cx.notify();
                            }),
                        )
                    } else {
                        self.context_item_disabled(
                            "grid-head-unfreeze-all",
                            t!("grid.unfreeze_all_columns").to_string(),
                        )
                    });
                    items = items.child(if valid && can_hide {
                        self.context_item(
                            "grid-head-hide",
                            t!("grid.hide_column").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                // `hide_column` must not update `AppView` through the grid's weak
                                // handle: this listener already runs inside an `AppView::update`,
                                // and re-entering the same entity aborts the app. Take the grid's
                                // new layout and store it from here instead.
                                let g = for_hide.read(cx);
                                let (connection_id, database, table) = (
                                    g.state.connection_id.clone(),
                                    g.state.database.clone(),
                                    g.state.table.clone(),
                                );
                                let layout = for_hide.update(cx, |grid, cx| {
                                    grid.hide_column(column, cx)
                                        .then(|| grid.field_layout_snapshot())
                                });
                                if let Some(layout) = layout
                                    && !connection_id.is_empty()
                                {
                                    this.set_field_layout(
                                        &connection_id,
                                        &database,
                                        &table,
                                        layout,
                                    );
                                }
                                cx.notify();
                            }),
                        )
                    } else {
                        self.context_item_disabled(
                            "grid-head-hide",
                            t!("grid.hide_column").to_string(),
                        )
                    });
                    items = items
                        .child(PopupMenuItem::separator())
                        .child(self.context_item(
                            "grid-head-refresh",
                            t!("grid.refresh").to_string(),
                            cx.listener(move |this, _event, window, cx| {
                                this.context_menu = None;
                                let grid = for_refresh.clone();
                                cx.defer_in(window, move |_this, _window, cx| {
                                    grid.update(cx, |grid, cx| grid.refresh(cx));
                                });
                                cx.notify();
                            }),
                        ));
                }
            }
        }

        items
    }

    /// Build the kit menu for the open context menu when its target changed (or it was dismissed
    /// and reopened), and remember it until its `DismissEvent`. Menu entities are never rebuilt
    /// per frame: hover, keyboard navigation and submenus keep their state until the kit dismisses
    /// it. Called once per render.
    pub(super) fn sync_context_menu(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(menu) = self.context_menu.clone() else {
            self.context_menu_key = None;
            return;
        };
        if self.context_menu_key.as_ref() == Some(&menu.target) && self.context_menu_view.is_some()
        {
            return;
        }
        let items = self.context_menu_items(&menu.target, menu.position, window, cx);
        let view = PopupMenu::build(window, cx, move |menu, _, _| {
            let mut menu = menu
                .min_w(px(170.0))
                .max_w(px(420.0))
                .max_h(px(560.0))
                .scrollable(true);
            for item in items {
                menu = menu.item(item);
            }
            menu
        });
        view.focus_handle(cx).focus(window, cx);
        let subscription = cx.subscribe(&view, |this, _, _: &DismissEvent, cx| {
            this.context_menu = None;
            this.context_menu_key = None;
            this.context_menu_view = None;
            this.context_menu_sub = None;
            cx.notify();
        });
        self.context_menu_key = Some(menu.target);
        self.context_menu_view = Some(view);
        self.context_menu_sub = Some(subscription);
    }

    /// The tab context menu's items, built as a kit `PopupMenu` when the menu opens.
    fn tab_menu_items(&self, target: TabTarget, cx: &mut Context<'_, Self>) -> MenuItemList {
        // 关闭右侧 is only useful while something sits to the right of the clicked tab.
        let order = self.ordered_tabs(cx);
        let tabs_to_the_right = order
            .iter()
            .position(|tab| *tab == target)
            .map(|position| order.len() - position - 1)
            .unwrap_or(0);
        let mut items = MenuItemList::new()
            .child(self.context_item_shortcut(
                "tab-close",
                t!("tab.close").to_string(),
                self.command_shortcut_label("general.close_tab", cx.platform_info().os),
                cx.listener(move |this, _event, _window, cx| this.close_tab(target, cx)),
            ))
            .child(self.context_item_shortcut(
                "tab-close-others",
                t!("tab.close_other").to_string(),
                self.command_shortcut_label("general.close_other_tabs", cx.platform_info().os),
                cx.listener(move |this, _event, _window, cx| this.close_other_tabs(target, cx)),
            ));
        items = if tabs_to_the_right > 0 {
            items.child(self.context_item(
                "tab-close-right",
                t!("tab.close_right").to_string(),
                cx.listener(move |this, _event, _window, cx| this.close_tabs_to_right(target, cx)),
            ))
        } else {
            items.child(
                self.context_item_disabled("tab-close-right", t!("tab.close_right").to_string()),
            )
        };
        items.child(self.context_item_shortcut(
            "tab-close-all",
            t!("tab.close_all").to_string(),
            self.command_shortcut_label("general.close_all_tabs", cx.platform_info().os),
            cx.listener(|this, _event, _window, cx| this.close_all_tabs(cx)),
        ))
    }

    /// Build the tab context menu when its target changed (or it was dismissed), mirroring
    /// [`AppView::sync_context_menu`].
    pub(super) fn sync_tab_menu(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(target) = self.tab_menu.as_ref().map(|menu| menu.target) else {
            self.tab_menu_key = None;
            return;
        };
        if self.tab_menu_key == Some(target) && self.tab_menu_view.is_some() {
            return;
        }
        // A context menu and a tab menu are mutually exclusive; drop the old menu's entity.
        self.context_menu = None;
        self.context_menu_key = None;
        self.context_menu_view = None;
        self.context_menu_sub = None;
        let items = self.tab_menu_items(target, cx);
        let view = PopupMenu::build(window, cx, move |menu, _, _| {
            let mut menu = menu.min_w(px(200.0)).max_w(px(360.0)).max_h(px(480.0));
            for item in items {
                menu = menu.item(item);
            }
            menu
        });
        view.focus_handle(cx).focus(window, cx);
        let subscription = cx.subscribe(&view, |this, _, _: &DismissEvent, cx| {
            this.tab_menu = None;
            this.tab_menu_key = None;
            this.tab_menu_view = None;
            this.tab_menu_sub = None;
            cx.notify();
        });
        self.tab_menu_key = Some(target);
        self.tab_menu_view = Some(view);
        self.tab_menu_sub = Some(subscription);
    }

    /// Wraps a submenu trigger row so its window-space rectangle can be recorded into the
    /// submenu's [`SubmenuZone`]. `on_children_prepainted` reports an element's *children*, so only
    /// a wrapper whose sole child is the row yields the row's own bounds — which the flyout's hover
    /// handling needs, because the flyout occludes the row's hover state.
    pub(super) fn submenu_row(
        &self,
        row: impl IntoElement,
        zone: Rc<RefCell<SubmenuZone>>,
    ) -> impl IntoElement {
        div()
            .w_full()
            .on_children_prepainted(move |bounds, _window, _cx| {
                if let Some(row) = bounds.first() {
                    zone.borrow_mut().row = Some(*row);
                }
            })
            .child(row)
    }

    /// A row of a hand-drawn dropdown (the New Connection engine menu, log copy menu, ...). The
    /// context menus use [`AppView::context_item`]'s kit menu items instead; these small anchored
    /// panels still build their rows directly.
    pub(super) fn menu_row(
        &self,
        id: impl Into<ElementId>,
        label: String,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        self.menu_row_shortcut(id, label, None, on_click)
    }

    /// [`AppView::menu_row`] with a shortcut label on the right.
    pub(super) fn menu_row_shortcut(
        &self,
        id: impl Into<ElementId>,
        label: String,
        shortcut: Option<String>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .w_full()
            .h(px(26.0))
            .px_3()
            .text_size(px(12.5))
            .cursor_pointer()
            .hover(move |style| {
                style
                    .bg(rgb(theme.tree_hover_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .on_click(on_click)
            .child(div().flex_1().min_w(px(0.0)).truncate().child(label))
            .children(shortcut.map(|shortcut| {
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(shortcut)
            }))
    }

    /// A greyed-out, non-interactive row of a hand-drawn dropdown.
    pub(super) fn menu_row_disabled(
        &self,
        id: impl Into<ElementId>,
        label: String,
    ) -> Stateful<Div> {
        let theme = self.theme;
        div()
            .id(id)
            .flex()
            .items_center()
            .w_full()
            .h(px(26.0))
            .px_3()
            .text_size(px(12.5))
            .text_color(rgb(theme.text_muted))
            .child(label)
    }

    /// One row of a context menu. The `id` is kept in the signature so the many call sites read
    /// the same; the kit menu tracks rows by index and hover, so no element id is needed.
    pub(super) fn context_item(
        &self,
        _id: impl Into<ElementId>,
        label: String,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> PopupMenuItem {
        PopupMenuItem::new(label).on_click(on_click)
    }

    /// [`AppView::context_item`] with the command's configured shortcut on the right, as the tab
    /// menu shows it; `None` (an unbound command) leaves the row as a plain item.
    pub(super) fn context_item_shortcut(
        &self,
        _id: impl Into<ElementId>,
        label: String,
        shortcut: Option<String>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> PopupMenuItem {
        let Some(shortcut) = shortcut else {
            return PopupMenuItem::new(label).on_click(on_click);
        };
        let muted = self.theme.text_muted;
        PopupMenuItem::element(move |_, _| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .w_full()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .child(label.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(rgb(muted))
                        .child(shortcut.clone()),
                )
        })
        .on_click(on_click)
    }

    /// A context-menu row that stays visible but is greyed out and non-interactive, used for
    /// actions that need a live connection the menu was opened on.
    pub(super) fn context_item_disabled(
        &self,
        _id: impl Into<ElementId>,
        label: String,
    ) -> PopupMenuItem {
        PopupMenuItem::new(label).disabled(true)
    }

    /// The 转储 SQL 文件 submenu: the dump modes, each starting a `.sql` dump of the database,
    /// the schema, or the table/view the menu was opened on.
    fn dump_sql_submenu_item(
        &self,
        connection_index: usize,
        database_index: usize,
        table: Option<String>,
        schema: Option<String>,
        is_view: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> PopupMenuItem {
        let labels = if table.is_none() || !is_view {
            vec![
                t!("database.dump_structure_and_data").to_string(),
                t!("database.dump_structure").to_string(),
                t!("database.dump_data").to_string(),
            ]
        } else {
            // A view has no rows of its own, so only its script can be dumped.
            vec![t!("database.dump_structure").to_string()]
        };
        let modes = if table.is_none() || !is_view {
            vec![
                DumpMode::StructureAndData,
                DumpMode::Structure,
                DumpMode::Data,
            ]
        } else {
            vec![DumpMode::Structure]
        };
        let mut items = Vec::new();
        for (label, mode) in labels.into_iter().zip(modes) {
            let target = table.clone();
            let target_schema = schema.clone();
            items.push(PopupMenuItem::new(label).on_click(cx.listener(
                move |this, _event, _window, cx| match &target {
                    Some(name) => this.request_dump_table(
                        connection_index,
                        database_index,
                        name.clone(),
                        is_view,
                        mode,
                        cx,
                    ),
                    None => match &target_schema {
                        Some(schema) => this.request_dump_schema(
                            connection_index,
                            database_index,
                            schema.clone(),
                            mode,
                            cx,
                        ),
                        None => this.request_dump(connection_index, database_index, mode, cx),
                    },
                },
            )));
        }
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let mut menu = menu.min_w(px(170.0));
            for item in items {
                menu = menu.item(item);
            }
            menu
        });
        PopupMenuItem::submenu(t!("database.dump_sql").to_string(), menu)
    }

    /// The connection context menu's 管理分组 submenu: 移动到 (every group, nested by indentation)
    /// and 从分组内排除.
    fn manage_group_submenu_item(
        &self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> PopupMenuItem {
        let groups = self.groups_flat();
        let mut items = vec![PopupMenuItem::label(t!("group.move_to").to_string())];
        if groups.is_empty() {
            items.push(PopupMenuItem::new(t!("group.none").to_string()).disabled(true));
        }
        for (id, depth) in groups {
            let name = self
                .group(id)
                .map(|group| group.name.clone())
                .unwrap_or_default();
            let label = format!("{}{name}", "  ".repeat(depth as usize));
            items.push(PopupMenuItem::new(label).on_click(cx.listener(
                move |this, _event, _window, cx| {
                    this.move_connection_to_group(index, id, cx);
                },
            )));
        }
        items.push(PopupMenuItem::separator());
        let exclude = PopupMenuItem::new(t!("group.exclude").to_string());
        items.push(if self.connection_group(index).is_some() {
            exclude.on_click(cx.listener(move |this, _event, _window, cx| {
                this.exclude_connection_from_group(index, cx);
            }))
        } else {
            exclude.disabled(true)
        });
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let mut menu = menu.min_w(px(190.0));
            for item in items {
                menu = menu.item(item);
            }
            menu
        });
        PopupMenuItem::submenu(t!("group.manage").to_string(), menu)
    }

    /// The connection context menu's 颜色 submenu: the red/yellow/green environment picker, with
    /// the active colour checked. Clicking the active colour clears it.
    fn color_submenu_item(
        &self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> PopupMenuItem {
        let current = self.connection_color(index);
        let theme = self.theme;
        let mut items = Vec::new();
        for color in ConnectionColor::ALL {
            let selected = current == Some(color);
            let accent = color.accent(theme);
            let label = t!(color.label_key()).to_string();
            items.push(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .w(px(10.0))
                                .h(px(10.0))
                                .rounded_full()
                                .flex_none()
                                .bg(rgb(accent)),
                        )
                        .child(div().flex_1().overflow_hidden().child(label.clone()))
                })
                .checked(selected)
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    let next = if this.connection_color(index) == Some(color) {
                        None
                    } else {
                        Some(color)
                    };
                    this.set_connection_color(index, next, cx);
                })),
            );
        }
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let mut menu = menu.min_w(px(150.0));
            for item in items {
                menu = menu.item(item);
            }
            menu
        });
        PopupMenuItem::submenu(t!("connection.color").to_string(), menu)
    }
}
