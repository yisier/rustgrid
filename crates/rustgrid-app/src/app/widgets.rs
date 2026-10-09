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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)));

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
            ContextTarget::Database {
                connection_index,
                database_index,
            } => {
                let ci = *connection_index;
                let di = *database_index;
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
                    ))
                    .child(self.context_item(
                        "db-refresh",
                        t!("database.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.load_databases(ci, cx);
                        }),
                    ));
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

                items = items.child(
                    self.context_item(
                        "table-open",
                        t!(if is_view {
                            "view.open"
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
                } else {
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

                if !is_view {
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
                                this.delete_confirm = Some(DeleteConfirm::Table {
                                    connection_index: ci,
                                    database_index: di,
                                    name: drop_name.clone(),
                                    operation: TableOperation::Drop,
                                });
                                cx.notify();
                            }),
                        ))
                        .child(self.context_item(
                            "table-empty",
                            t!("object.empty_table").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.delete_confirm = Some(DeleteConfirm::Table {
                                    connection_index: ci,
                                    database_index: di,
                                    name: empty_name.clone(),
                                    operation: TableOperation::Empty,
                                });
                                cx.notify();
                            }),
                        ))
                        .child(self.context_item(
                            "table-truncate",
                            t!("object.truncate_table").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                this.delete_confirm = Some(DeleteConfirm::Table {
                                    connection_index: ci,
                                    database_index: di,
                                    name: truncate_name.clone(),
                                    operation: TableOperation::Truncate,
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
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
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
                let grid = self
                    .grids
                    .iter()
                    .find(|grid| grid.read(cx).state.id == *grid_id)
                    .cloned();
                if let Some(grid) = grid {
                    let (has_selection, editable, deletable) = {
                        let g = grid.read(cx);
                        (
                            g.state.selection.is_some(),
                            g.state.editable,
                            g.state.editable
                                && g.state.selection.is_some()
                                && !g.state.rows.is_empty(),
                        )
                    };
                    let for_delete = grid.clone();
                    let for_copy = grid.clone();
                    let for_insert = grid.clone();
                    let for_paste = grid.clone();
                    let for_refresh = grid.clone();
                    let for_insert_row = grid.clone();
                    let for_null = grid.clone();
                    items = items.child(if deletable {
                        self.context_item(
                            "grid-ctx-delete",
                            t!("grid.delete_rows").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                for_delete.update(cx, |grid, cx| grid.open_delete_confirm(cx));
                                cx.notify();
                            }),
                        )
                        .into_any_element()
                    } else {
                        self.context_item_disabled(
                            "grid-ctx-delete",
                            t!("grid.delete_rows").to_string(),
                        )
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
                    items = items.child(if has_selection {
                        self.context_item(
                            "grid-ctx-copy-insert",
                            t!("grid.copy_insert").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                for_insert.update(cx, |grid, cx| grid.copy_selection_as_insert(cx));
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
                    items = items.child(if editable {
                        self.context_item(
                            "grid-ctx-insert-row",
                            t!("grid.insert_row").to_string(),
                            cx.listener(move |this, _event, window, cx| {
                                this.context_menu = None;
                                for_insert_row
                                    .update(cx, |grid, cx| grid.add_insert_row(window, cx));
                                cx.notify();
                            }),
                        )
                        .into_any_element()
                    } else {
                        self.context_item_disabled(
                            "grid-ctx-insert-row",
                            t!("grid.insert_row").to_string(),
                        )
                        .into_any_element()
                    });
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
                        self.context_item_disabled("grid-ctx-null", t!("grid.set_null").to_string())
                            .into_any_element()
                    });
                    items = items
                        .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                        .child(self.context_item(
                            "grid-ctx-refresh",
                            t!("grid.refresh").to_string(),
                            cx.listener(move |this, _event, _window, cx| {
                                this.context_menu = None;
                                for_refresh.update(cx, |grid, cx| grid.refresh(cx));
                                cx.notify();
                            }),
                        ));
                }
            }
        }

        ui::popup_panel(theme)
            .left(menu.position.x)
            .top(menu.position.y)
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.context_menu = None;
                cx.notify();
            }))
            .child(items)
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
            .w(px(170.0))
            .p_0p5()
            .child(self.context_item(
                "tab-close",
                t!("tab.close").to_string(),
                cx.listener(move |this, _event, _window, cx| this.close_tab(target, cx)),
            ))
            .child(self.context_item(
                "tab-close-others",
                t!("tab.close_other").to_string(),
                cx.listener(move |this, _event, _window, cx| this.close_other_tabs(target, cx)),
            ))
            .child(self.context_item(
                "tab-close-all",
                t!("tab.close_all").to_string(),
                cx.listener(|this, _event, _window, cx| this.close_all_tabs(cx)),
            ));

        ui::popup_panel(theme)
            .left(menu.position.x)
            .top(menu.position.y)
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.tab_menu = None;
                cx.notify();
            }))
            .child(items)
    }

    pub(super) fn context_item(
        &self,
        id: impl Into<ElementId>,
        label: String,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = self.theme;
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
            .on_click(on_click)
            .child(label)
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
}
