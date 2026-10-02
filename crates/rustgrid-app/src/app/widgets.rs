use super::*;

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
                            this.disconnect(index, cx);
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
                // visible (greyed out) when disconnected.
                items =
                    if connected {
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
                    ))
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
                            this.extract_backup_sql(index, cx);
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
                            this.begin_rename_backup(index, window, cx);
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
        id: &'static str,
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
        id: &'static str,
        label: String,
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
            .text_color(rgb(theme.text_muted))
            .child(label)
    }
}
