use super::*;
use gpui::ElementId;
use gpui_kit::base::SelectableText;

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

    pub(super) fn render_context_menu(
        &self,
        menu: &ContextMenu,
        _window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let mut items = div().flex().flex_col().w(px(170.0)).p_0p5();

        match &menu.target {
            ContextTarget::Connection(index) => {
                let index = *index;
                let status = self.connections.get(index).map(|node| &node.status);
                let connected = matches!(status, Some(ConnectionStatus::Connected(_)));
                let connecting = matches!(status, Some(ConnectionStatus::Connecting));
                // A failed or connecting connection has no live session, but it does hold state (its
                // error message), so it can still be closed to reset it. Only a connected one is
                // "open"; an untouched one is neither.
                let closeable = matches!(
                    status,
                    Some(ConnectionStatus::Connected(_))
                        | Some(ConnectionStatus::Connecting)
                        | Some(ConnectionStatus::Failed(_))
                );

                // Show only the action that applies: an open (or opening) connection offers just
                // 关闭连接, an untouched one just 打开连接, and a failed one both (retry, or reset).
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
                    .child(self.color_submenu_item(index, cx))
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.manage_group_submenu_item(index, cx));

                // Creating a database and refreshing the list both need a live connection, but stay
                // visible (greyed out) when disconnected. Engines without database management
                // (SQLite) do not show the create item at all.
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

                items = items.child(div().h(px(1.0)).my_1().bg(rgb(theme.border)));
                items =
                    if connected {
                        items.child(self.context_item(
                            "ctx-refresh",
                            t!("connection.refresh").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.load_databases(index, cx);
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
                let menu_position = menu.position;
                items = items
                    .child(self.context_item(
                        "group-new-connection",
                        t!("group.new_connection").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.new_connection_in_group(id, menu_position, cx);
                        }),
                    ))
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                        Some(ConnectionStatus::Connected(_))
                            | Some(ConnectionStatus::Connecting)
                            | Some(ConnectionStatus::Failed(_))
                    )
                });
                let any_closed = indexes.iter().any(|index| {
                    matches!(
                        self.connections.get(*index).map(|node| &node.status),
                        Some(ConnectionStatus::Disconnected) | Some(ConnectionStatus::Failed(_))
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
                    .into_any_element()
                } else {
                    self.context_item_disabled(
                        "tree-sel-connect",
                        t!("connection.connect").to_string(),
                    )
                    .into_any_element()
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
                    .into_any_element()
                } else {
                    self.context_item_disabled(
                        "tree-sel-disconnect",
                        t!("connection.disconnect").to_string(),
                    )
                    .into_any_element()
                });
                items = items.child(self.import_connections_submenu_item(cx));
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
                    .into_any_element()
                } else {
                    self.context_item_disabled(
                        "tree-sel-delete",
                        t!("connection.delete_connection").to_string(),
                    )
                    .into_any_element()
                });
                items = items
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.import_connections_submenu_item(cx))
                    .child(self.context_item(
                        "tree-export-connections",
                        t!("connection.export_connection").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.import_submenu = false;
                            this.open_export_connections_window(cx);
                        }),
                    ))
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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

                if self.driver_supports(ci, DriverCapability::DatabaseManagement) {
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
                        ))
                        .child(self.context_item(
                            "db-delete",
                            t!("database.delete").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.open_delete_database(ci, di, cx);
                            }),
                        ));
                }
                items = items
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                                this.load_databases(ci, cx);
                            }),
                        ));
                } else {
                    items = items
                        .child(self.context_item(
                            "db-run-sql",
                            t!("database.run_sql_file").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.dump_submenu = None;
                                this.open_run_sql_file(ci, di, cx);
                            }),
                        ))
                        .child(self.dump_sql_submenu_item(ci, di, cx))
                        .child(self.context_item(
                            "db-refresh",
                            t!("database.refresh").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.load_databases(ci, cx);
                            }),
                        ));
                }
                if self.driver_supports(ci, DriverCapability::Schemas) {
                    items = items
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                let key_value = self.driver_supports(ci, DriverCapability::KeyValue);

                items = items.child(
                    self.context_item(
                        "table-open",
                        t!(if is_view {
                            "view.open"
                        } else if key_value {
                            "redis.key.open"
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
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                        .child(self.context_item(
                            "view-drop",
                            t!("view.delete").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.confirm_delete_view(ci, di, view_drop_name.clone(), cx);
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
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                        .child(self.context_item(
                            "key-ttl",
                            t!("redis.tools.key_apply_ttl").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.start_key_ttl_dialog(ci, di, ttl_name.clone(), cx);
                            }),
                        ))
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                        .child(self.context_item(
                            "table-drop",
                            t!("object.delete_table").to_string(),
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
                        ))
                        .child(self.context_item(
                            "table-empty",
                            t!("object.empty_table").to_string(),
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
                        ))
                        .child(self.context_item(
                            "table-truncate",
                            t!("object.truncate_table").to_string(),
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
                        ))
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                        .child(self.context_item(
                            "table-rename",
                            t!("object.rename_table").to_string(),
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
                if self.driver_supports_table_designer(ci) {
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
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)));
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "tablecat-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.reload_tables(ci, di, cx);
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
                            Category::Queries => this.refresh_query_files(cx),
                            Category::Backups => this.refresh_backups(cx),
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "routine-delete",
                        t!("routine.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.confirm_delete_routine(ci, di, drop_name.clone(), kind, cx);
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "backup-ctx-extract",
                        t!("backup.extract_sql").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_extract_sql(index, cx);
                        }),
                    ))
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "backup-ctx-reveal",
                        t!("backup.reveal").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.reveal_backup(index);
                            cx.notify();
                        }),
                    ))
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "queryfile-ctx-reveal",
                        t!("backup.reveal").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.reveal_query(index);
                            cx.notify();
                        }),
                    ))
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                    ));
            }
            ContextTarget::TransferList => {
                items = items.child(self.context_item(
                    "transferlist-ctx-new",
                    t!("sync.new").to_string(),
                    cx.listener(|this, _event, _window, cx| {
                        this.context_menu = None;
                        this.open_new_transfer(cx);
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "backuplist-ctx-reveal",
                        t!("backup.reveal").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.reveal_backup_folder(cx);
                        }),
                    ))
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "backuplist-ctx-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_selected_backup(cx);
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "querylist-ctx-reveal",
                        t!("backup.reveal").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.reveal_query_folder(cx);
                        }),
                    ))
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "querylist-ctx-refresh",
                        t!("connection.refresh").to_string(),
                        cx.listener(|this, _event, _window, cx| {
                            this.context_menu = None;
                            this.refresh_selected_query(cx);
                        }),
                    ));
            }
            ContextTarget::Grid { grid_id } => {
                let grid_id = *grid_id;
                let grid = self
                    .grids
                    .iter()
                    .find(|grid| grid.read(cx).state.id == grid_id)
                    .cloned();
                if let Some(grid) = grid {
                    let (has_selection, editable, deletable, key_value) = {
                        let g = grid.read(cx);
                        (
                            g.state.selection.is_some(),
                            g.state.editable,
                            g.state.editable
                                && g.state.selection.is_some()
                                && !g.state.rows.is_empty(),
                            g.state.key_status.is_some(),
                        )
                    };
                    let for_delete = grid.clone();
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
                        .into_any_element()
                    } else {
                        self.context_item_disabled("grid-ctx-delete", delete_label)
                            .into_any_element()
                    });
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
                        .into_any_element()
                    } else {
                        self.context_item_disabled("grid-ctx-copy", t!("grid.copy").to_string())
                            .into_any_element()
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
                            .into_any_element()
                        } else {
                            self.context_item_disabled(
                                "grid-ctx-copy-insert",
                                t!("grid.copy_insert").to_string(),
                            )
                            .into_any_element()
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
                        .into_any_element()
                    } else {
                        self.context_item_disabled("grid-ctx-paste", t!("grid.paste").to_string())
                            .into_any_element()
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
                            .into_any_element()
                        } else {
                            self.context_item_disabled(
                                "grid-ctx-null",
                                t!("grid.set_null").to_string(),
                            )
                            .into_any_element()
                        });
                    }
                    items = items
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
        }

        // The panel places itself from the pointer through `anchored`, which measures the menu
        // after layout: a menu opened near the bottom edge switches its anchor corner and grows
        // upward from the click instead of being clamped by a guessed height; the old fixed
        // estimate pushed a short menu far above the row it belonged to.
        anchored()
            .position(menu.position)
            .anchor(Anchor::TopLeft)
            .child(
                ui::popup_panel(theme)
                    .relative()
                    .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                        // The 转储 SQL 文件, 管理分组, 颜色 and 导入连接 flyouts overflow past this
                        // panel, so a click on one also reads as "outside" here. Keep the menu
                        // alive when the click is on a flyout (their zones are only populated
                        // while they are shown).
                        let on_flyout = [
                            &this.dump_zone,
                            &this.move_zone,
                            &this.color_zone,
                            &this.import_zone,
                        ]
                        .iter()
                        .any(|zone| zone.borrow().contains(&event.position));
                        if on_flyout {
                            return;
                        }
                        this.context_menu = None;
                        this.dump_submenu = None;
                        this.move_submenu = None;
                        this.color_submenu = None;
                        this.import_submenu = false;
                        cx.notify();
                    }))
                    .child(items),
            )
    }

    pub(super) fn render_tab_menu(
        &self,
        menu: &TabMenu,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let target = menu.target;
        let items = div()
            .flex()
            .flex_col()
            .w(px(200.0))
            .p_0p5()
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
            ))
            .child(self.context_item_shortcut(
                "tab-close-all",
                t!("tab.close_all").to_string(),
                self.command_shortcut_label("general.close_all_tabs", cx.platform_info().os),
                cx.listener(|this, _event, _window, cx| this.close_all_tabs(cx)),
            ));

        anchored()
            .position(menu.position)
            .anchor(Anchor::TopLeft)
            .child(
                ui::popup_panel(theme)
                    .relative()
                    .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                        this.tab_menu = None;
                        cx.notify();
                    }))
                    .child(items),
            )
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

    pub(super) fn context_item(
        &self,
        id: impl Into<ElementId>,
        label: String,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        self.context_item_shortcut(id, label, None, on_click)
    }

    /// [`AppView::context_item`] with the command's configured shortcut on the right, as the tab
    /// menu shows it; `None` (an unbound command) leaves the row as a plain item.
    pub(super) fn context_item_shortcut(
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

    /// A context-menu row that stays visible but is greyed out and non-interactive, used for
    /// actions that need a live connection the menu was opened on.
    pub(super) fn context_item_disabled(
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

    /// The database context menu's 转储 SQL 文件 row: a submenu trigger that reveals a right-hand
    /// flyout with the three dump modes (结构和数据 / 仅结构 / 仅数据). The flyout is a child of
    /// this row, positioned just past the menu's right edge, so it reads like Navicat's submenu.
    fn dump_sql_submenu_item(
        &self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let open = self.dump_submenu == Some((connection_index, database_index));
        let zone = self.dump_zone.clone();
        let bounds_slot = self.dump_zone.clone();
        let mut base = div()
            .relative()
            .flex()
            .items_center()
            .w_full()
            .h(px(26.0))
            .px_3()
            .gap_1()
            .text_size(px(12.5));
        if open {
            // The flyout panel is this row's last child; its rectangle is what an outside click
            // must be tested against (the flyout overflows the menu panel).
            base = base.on_children_prepainted(move |bounds, _window, _cx| {
                bounds_slot.borrow_mut().flyout = bounds.last().copied();
            });
        } else {
            // Drop the stale flyout rectangle so a later outside click closes the menu normally.
            bounds_slot.borrow_mut().flyout = None;
        }
        let row = base
            .id("db-dump-sql")
            .cursor_pointer()
            .hover(move |style| {
                style
                    .bg(rgb(theme.tree_hover_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                // Open the flyout when the row is hovered (Navicat's submenu behaviour), and close
                // it again once the pointer leaves the row and the flyout beside it.
                if zone.borrow().stays_open(*hovered, window) {
                    if this.dump_submenu != Some((connection_index, database_index)) {
                        this.dump_submenu = Some((connection_index, database_index));
                        cx.notify();
                    }
                } else if this.dump_submenu == Some((connection_index, database_index)) {
                    this.dump_submenu = None;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.dump_submenu = Some((connection_index, database_index));
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(t!("database.dump_sql").to_string()),
            )
            .child(
                svg()
                    .path("icons/chevron-right.svg")
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );
        let row = if open {
            row.child(dump_sql_flyout(
                connection_index,
                database_index,
                theme,
                self.dump_zone.clone(),
                cx,
            ))
        } else {
            row
        };
        self.submenu_row(row, self.dump_zone.clone())
    }

    /// The connection context menu's 管理分组 row: a submenu trigger that reveals a right-hand
    /// flyout listing 移动到 (every group, nested by indentation) and 从分组内排除.
    fn manage_group_submenu_item(
        &self,
        index: usize,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let open = self.move_submenu == Some(index);
        let zone = self.move_zone.clone();
        let bounds_slot = self.move_zone.clone();
        let mut base = div()
            .relative()
            .flex()
            .items_center()
            .w_full()
            .h(px(26.0))
            .px_3()
            .gap_1()
            .text_size(px(12.5));
        if open {
            base = base.on_children_prepainted(move |bounds, _window, _cx| {
                bounds_slot.borrow_mut().flyout = bounds.last().copied();
            });
        } else {
            bounds_slot.borrow_mut().flyout = None;
        }
        let row = base
            .id("ctx-manage-group")
            .cursor_pointer()
            .hover(move |style| {
                style
                    .bg(rgb(theme.tree_hover_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                // Open on hover, and close again once the pointer leaves the row and its flyout.
                if zone.borrow().stays_open(*hovered, window) {
                    if this.move_submenu != Some(index) {
                        this.dump_submenu = None;
                        this.color_submenu = None;
                        this.move_submenu = Some(index);
                        cx.notify();
                    }
                } else if this.move_submenu == Some(index) {
                    this.move_submenu = None;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.dump_submenu = None;
                this.color_submenu = None;
                this.move_submenu = Some(index);
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(t!("group.manage").to_string()),
            )
            .child(
                svg()
                    .path("icons/chevron-right.svg")
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );
        let row = if open {
            row.child(self.manage_group_flyout(index, theme, cx))
        } else {
            row
        };
        self.submenu_row(row, self.move_zone.clone())
    }

    /// The 管理分组 flyout: a 移动到 header, the nested group list, and 从分组内排除 (enabled only
    /// while the connection is in a group).
    fn manage_group_flyout(
        &self,
        index: usize,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let zone = self.move_zone.clone();
        let mut flyout = ui::popup_panel(theme)
            // Abut the trigger row so the pointer never crosses a gap on its way to the flyout.
            .left(relative(1.0))
            .top(px(0.0))
            .w(px(190.0))
            .p_0p5()
            .id("ctx-manage-group-flyout")
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if !zone.borrow().stays_open(*hovered, window) && this.move_submenu == Some(index) {
                    this.move_submenu = None;
                    cx.notify();
                }
            }));
        flyout = flyout.child(
            div()
                .px_3()
                .py_1()
                .text_size(px(11.0))
                .text_color(rgb(theme.text_muted))
                .child(t!("group.move_to").to_string()),
        );
        let groups = self.groups_flat();
        if groups.is_empty() {
            flyout = flyout.child(
                div()
                    .px_3()
                    .h(px(24.0))
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("group.none").to_string()),
            );
        }
        for (id, depth) in groups {
            let name = self
                .group(id)
                .map(|group| group.name.clone())
                .unwrap_or_default();
            flyout = flyout.child(
                div()
                    .id(SharedString::from(format!("move-group-{id}")))
                    .flex()
                    .items_center()
                    .w_full()
                    .h(px(26.0))
                    .pr_2()
                    .pl(px(12.0 + depth as f32 * 12.0))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .hover(move |style| {
                        style
                            .bg(rgb(theme.tree_hover_bg))
                            .text_color(rgb(theme.tree_selected_text))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.context_menu = None;
                        this.move_submenu = None;
                        this.move_connection_to_group(index, id, cx);
                    }))
                    .child(name),
            );
        }
        flyout = flyout.child(div().h(px(1.0)).my_1().bg(rgb(theme.border)));
        if self.connection_group(index).is_some() {
            flyout = flyout.child(
                div()
                    .id("ctx-exclude-group")
                    .flex()
                    .items_center()
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
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.context_menu = None;
                        this.move_submenu = None;
                        this.exclude_connection_from_group(index, cx);
                    }))
                    .child(t!("group.exclude").to_string()),
            );
        } else {
            flyout = flyout.child(
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .h(px(26.0))
                    .px_3()
                    .text_size(px(12.5))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("group.exclude").to_string()),
            );
        }
        flyout.into_any_element()
    }

    /// The connection context menu's 颜色 row: a submenu trigger opening the red/green/blue
    /// environment picker. Mirrors [`AppView::manage_group_submenu_item`].
    fn color_submenu_item(&self, index: usize, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let open = self.color_submenu == Some(index);
        let zone = self.color_zone.clone();
        let bounds_slot = self.color_zone.clone();
        let mut base = div()
            .relative()
            .flex()
            .items_center()
            .w_full()
            .h(px(26.0))
            .px_3()
            .gap_1()
            .text_size(px(12.5));
        if open {
            // The flyout panel is this row's last child; its rectangle is what an outside click
            // must be tested against (the flyout overflows the menu panel).
            base = base.on_children_prepainted(move |bounds, _window, _cx| {
                bounds_slot.borrow_mut().flyout = bounds.last().copied();
            });
        } else {
            bounds_slot.borrow_mut().flyout = None;
        }
        let row = base
            .id("ctx-connection-color")
            .cursor_pointer()
            .hover(move |style| {
                style
                    .bg(rgb(theme.tree_hover_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                // Open on hover (Navicat's behaviour), and close again once the pointer leaves the
                // row and the flyout beside it.
                if zone.borrow().stays_open(*hovered, window) {
                    if this.color_submenu != Some(index) {
                        this.dump_submenu = None;
                        this.move_submenu = None;
                        this.color_submenu = Some(index);
                        cx.notify();
                    }
                } else if this.color_submenu == Some(index) {
                    this.color_submenu = None;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.dump_submenu = None;
                this.move_submenu = None;
                this.color_submenu = Some(index);
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(t!("connection.color").to_string()),
            )
            .child(
                svg()
                    .path("icons/chevron-right.svg")
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );
        let row = if open {
            row.child(self.color_flyout(index, theme, cx))
        } else {
            row
        };
        self.submenu_row(row, self.color_zone.clone())
    }

    /// The 颜色 flyout: one row per colour, with a coloured dot on the left and a check on the
    /// active one. Clicking the active colour clears it.
    fn color_flyout(&self, index: usize, theme: Theme, cx: &mut Context<'_, Self>) -> AnyElement {
        let current = self.connection_color(index);
        let zone = self.color_zone.clone();
        let mut flyout = ui::popup_panel(theme)
            // Abut the trigger row (100% of the row's padding box) so the pointer never crosses a
            // dead gap between the row and the flyout, which would close the submenu on the way in.
            .left(relative(1.0))
            .top(px(0.0))
            .w(px(150.0))
            .p_0p5()
            // Leaving the flyout closes the submenu unless the pointer moved onto its trigger row
            // (which re-opens it); together with the row's own handler this keeps the flyout up
            // exactly while the pointer is inside the row-and-flyout zone.
            .id("ctx-connection-color-flyout")
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if !zone.borrow().stays_open(*hovered, window) && this.color_submenu == Some(index)
                {
                    this.color_submenu = None;
                    cx.notify();
                }
            }));
        for color in ConnectionColor::ALL {
            let selected = current == Some(color);
            flyout = flyout.child(
                div()
                    .id(SharedString::from(format!("conn-color-{}", color.id())))
                    .flex()
                    .items_center()
                    .gap_2()
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
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.context_menu = None;
                        this.color_submenu = None;
                        let next = if this.connection_color(index) == Some(color) {
                            None
                        } else {
                            Some(color)
                        };
                        this.set_connection_color(index, next, cx);
                    }))
                    .child(
                        div()
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_full()
                            .flex_none()
                            .bg(rgb(color.accent(theme))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(t!(color.label_key()).to_string()),
                    )
                    .child(
                        svg()
                            .path("icons/check.svg")
                            .w(px(12.0))
                            .h(px(12.0))
                            .flex_none()
                            .text_color(rgb(color.accent(theme)))
                            .opacity(if selected { 1.0 } else { 0.0 }),
                    ),
            );
        }
        flyout.into_any_element()
    }
}

/// The 转储 SQL 文件 flyout: the three dump modes, each starting a whole-database `.sql` dump.
fn dump_sql_flyout(
    connection_index: usize,
    database_index: usize,
    theme: Theme,
    zone: Rc<RefCell<SubmenuZone>>,
    cx: &mut Context<'_, AppView>,
) -> AnyElement {
    let modes = [
        (
            "db-dump-all",
            t!("database.dump_structure_and_data").to_string(),
            DumpMode::StructureAndData,
        ),
        (
            "db-dump-structure",
            t!("database.dump_structure").to_string(),
            DumpMode::Structure,
        ),
        (
            "db-dump-data",
            t!("database.dump_data").to_string(),
            DumpMode::Data,
        ),
    ];
    let mut flyout = ui::popup_panel(theme)
        // Abut the trigger row so the pointer never crosses a gap on its way to the flyout.
        .left(relative(1.0))
        .top(px(0.0))
        .w(px(150.0))
        .p_0p5()
        .id("db-dump-sql-flyout")
        .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
            if !zone.borrow().stays_open(*hovered, window)
                && this.dump_submenu == Some((connection_index, database_index))
            {
                this.dump_submenu = None;
                cx.notify();
            }
        }));
    for (id, label, mode) in modes {
        flyout = flyout.child(
            div()
                .id(id)
                .flex()
                .items_center()
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
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.context_menu = None;
                    this.dump_submenu = None;
                    this.request_dump(connection_index, database_index, mode, cx);
                }))
                .child(label),
        );
    }
    flyout.into_any_element()
}
