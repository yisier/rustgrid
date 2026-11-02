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

/// The object pane's database coordinates (even with no selection) plus its view flag.
fn object_scope(pane: &Entity<ObjectPane>, cx: &App) -> (usize, usize, bool) {
    let pane = pane.read(cx);
    (
        pane.connection_index,
        pane.database_index,
        pane.category == Category::Views,
    )
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

/// The remembered-layout page key for an object category.
fn view_page_for_category(category: Category) -> &'static str {
    match category {
        Category::Tables => VIEW_PAGE_TABLES,
        Category::Views => VIEW_PAGE_VIEWS,
        Category::Functions => VIEW_PAGE_FUNCTIONS,
        Category::Queries => VIEW_PAGE_QUERIES,
        Category::Backups => VIEW_PAGE_BACKUPS,
    }
}

/// A padded list message (loading/failed/empty) coloured by `color`.
fn object_message(color: u32, text: String) -> AnyElement {
    div()
        .p_3()
        .text_color(rgb(color))
        .child(text)
        .into_any_element()
}

/// The per-render snapshot the object list draws from: the remembered layout, the loaded
/// tables/routines, the search text, the current multi-selection and the rename editor.
struct ObjectRenderData {
    mode: ViewMode,
    schema: Option<String>,
    /// Whether the engine is schema-qualified (SQL Server/PostgreSQL). When true, objects are only
    /// listed once a schema is selected.
    supports_schemas: bool,
    tables: Option<Loadable<Vec<rustgrid_core::TableInfo>>>,
    table_statuses: Option<Vec<(String, rustgrid_core::TableStatus)>>,
    routines: Option<Loadable<Vec<RoutineInfo>>>,
    search: String,
    selected: std::collections::HashSet<String>,
    rename: Option<RenameRow>,
}

/// The sort value of one Tables/Views 详细列表 cell, for the column at `column` (a column the
/// category does not show cannot be active).
fn table_sort_key(
    table: &rustgrid_core::TableInfo,
    status: Option<&rustgrid_core::TableStatus>,
    category: Category,
    column: usize,
) -> SortKey {
    let yes = t!("common.yes").to_string();
    let no = t!("common.no").to_string();
    match category {
        Category::Views => match column {
            1 => SortKey::text(if table.updatable { &yes } else { &no }),
            _ => SortKey::text(&object_display(&table.name)),
        },
        _ => match column {
            1 => SortKey::number(status.and_then(|s| s.auto_increment).map(|v| v as f64)),
            2 => SortKey::opt_text(status.and_then(|s| s.updated.as_deref())),
            3 => SortKey::number(status.and_then(|s| s.data_length).map(|v| v as f64)),
            4 => SortKey::opt_text(status.and_then(|s| s.engine.as_deref())),
            5 => SortKey::number(status.and_then(|s| s.rows).map(|v| v as f64)),
            6 => SortKey::opt_text(status.and_then(|s| s.comment.as_deref())),
            _ => SortKey::text(&object_display(&table.name)),
        },
    }
}

/// The small type badge before a key name in the key list (`S`/`H`/`L`/`E`/`Z`/`X`, like the
/// HTML prototype's type icons).
pub(super) fn key_type_badge(kind: &str, theme: Theme) -> AnyElement {
    let lower = kind.to_ascii_lowercase();
    let (letter, color) = match lower.as_str() {
        "string" => ("S", theme.brand),
        "hash" => ("H", theme.icon_database),
        "list" => ("L", theme.icon_functions),
        "set" => ("E", theme.icon_view),
        "zset" => ("Z", theme.danger),
        "stream" => ("X", theme.icon_tables),
        "json" => ("J", theme.brand),
        _ => {
            return div().w(px(16.0)).flex_none().into_any_element();
        }
    };
    div()
        .w(px(16.0))
        .h(px(16.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(rgb(color))
        .text_size(px(9.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(color))
        .child(letter)
        .into_any_element()
}

/// The 分组 of one key: the prefix through its first `:`, the same split the tree's namespace
/// folders use (`sys_dict:cache` belongs to `sys_dict:`). A key without a colon has no group.
fn key_group(name: &str) -> String {
    match name.find(':') {
        Some(index) => name[..=index].to_string(),
        None => String::new(),
    }
}

/// The sort value of one key-value key-list cell: 分组 / 键名 / 类型 / TTL / 内存 / 值.
fn key_sort_key(
    table: &rustgrid_core::TableInfo,
    status: Option<&rustgrid_core::TableStatus>,
    column: usize,
) -> SortKey {
    match column {
        0 => SortKey::text(&key_group(&table.name)),
        2 => SortKey::opt_text(status.and_then(|s| s.engine.as_deref())),
        3 => SortKey::number(status.and_then(|s| s.ttl).map(|v| v as f64)),
        4 => SortKey::number(status.and_then(|s| s.data_length).map(|v| v as f64)),
        5 => SortKey::text(&key_preview(status)),
        _ => SortKey::text(&object_display(&table.name)),
    }
}

/// The 值 column's one-line preview of one key: a string key's leading characters, or a
/// collection's entry count (a collection has no single value to show). The cell truncates what
/// does not fit and offers the rest on hover.
fn key_preview(status: Option<&rustgrid_core::TableStatus>) -> String {
    let Some(status) = status else {
        return String::new();
    };
    if let Some(preview) = status.preview.as_deref() {
        return single_line(preview);
    }
    match status.rows {
        Some(rows) => format!("{rows} {}", t!("redis.key.entries")),
        None => String::new(),
    }
}

/// The sort value of one Functions 详细列表 cell: 名 / 修改日期 / 函数类型 / 决定性 / 注释.
fn routine_sort_key(routine: &RoutineInfo, column: usize) -> SortKey {
    let yes = t!("common.yes").to_string();
    let no = t!("common.no").to_string();
    match column {
        1 => SortKey::opt_text(routine.modified.as_deref()),
        2 => SortKey::text(routine.kind.sql_name()),
        3 => SortKey::text(if routine.deterministic { &yes } else { &no }),
        4 => SortKey::text(&single_line(&routine.comment)),
        _ => SortKey::text(&object_display(&routine.name)),
    }
}

/// The schema prefix of a schema-qualified object name (`dbo` in `dbo.users`).
fn schema_of(name: &str) -> Option<&str> {
    name.split_once('.')
        .filter(|(schema, object)| !schema.is_empty() && !object.is_empty())
        .map(|(schema, _)| schema)
}

/// The bare object name shown in lists (`wes_station` for `dbo.wes_station`); the schema is
/// already implied by the selected schema node. Shared with the in-place rename editor, which must
/// prefill the bare name rather than the internal `schema.object` identity.
pub(super) fn object_display(name: &str) -> String {
    name.split_once('.')
        .filter(|(schema, object)| !schema.is_empty() && !object.is_empty())
        .map(|(_, object)| object.to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Wrap an object row/tile so it publishes its window-space rectangle for the marquee.
fn object_row_with_rect(
    app: WeakEntity<AppView>,
    row: impl IntoElement + 'static,
    key: String,
) -> AnyElement {
    super::list_ops::row_with_rect(app, row, MarqueeTarget::Objects, key)
}

/// One object-list navigation key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectStep {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
}

/// Where `step` moves the selection in a list of `count` objects laid out column-major with
/// `per_column` items per column (`usize::MAX` for the single-column 详细列表), paging by `page`
/// items. With nothing selected any key starts at the first object. `None` means "stay".
fn object_nav_target(
    current: Option<usize>,
    count: usize,
    step: ObjectStep,
    per_column: usize,
    page: usize,
) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let last = count - 1;
    let Some(index) = current else {
        return Some(0);
    };
    let target = match step {
        ObjectStep::Up => index.checked_sub(1)?,
        ObjectStep::Down => (index + 1).min(last),
        // Left / Right move one column in the 平铺网格; the 详细列表 has a single column.
        ObjectStep::Left if per_column != usize::MAX => index.checked_sub(per_column)?,
        ObjectStep::Right if per_column != usize::MAX => {
            // Into the next column's same row, or its last item when that column is shorter.
            if index / per_column == last / per_column {
                return None;
            }
            (index + per_column).min(last)
        }
        ObjectStep::Left | ObjectStep::Right => return None,
        ObjectStep::PageUp => index.saturating_sub(page.max(1)),
        ObjectStep::PageDown => (index + page.max(1)).min(last),
        ObjectStep::Home => 0,
        ObjectStep::End => last,
    };
    (target != index).then_some(target)
}

impl AppView {
    /// The Redis key-list toolbar, laid out like the prototype's: a filled 新建键, an outlined
    /// 删除, plain 设置 TTL / 刷新 actions, and the active namespace chip.
    fn render_key_object_toolbar(
        &self,
        pane: &Entity<ObjectPane>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let (connection_index, database_index, category, selected) = {
            let pane = pane.read(cx);
            (
                pane.connection_index,
                pane.database_index,
                pane.category,
                pane.selected.clone(),
            )
        };
        let selected_key = selected.or_else(|| self.objects_selection.items().first().cloned());
        let namespace = self
            .connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index),
                _ => None,
            })
            .and_then(|database| database.object_pattern.clone());
        // A key created while a namespace is active starts inside it (`login_tokens:*` becomes
        // `login_tokens:` in the name field).
        let new_key_group = namespace
            .as_deref()
            .map(|pattern| pattern.trim_end_matches('*').to_string());
        let pane_for_delete = pane.clone();

        let mut left = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .child(self.dialog_button(
                "obj-new-key",
                t!("redis.key.new").to_string(),
                true,
                cx.listener(move |this, _event, _window, cx| {
                    this.start_new_key_dialog(
                        connection_index,
                        database_index,
                        new_key_group.clone(),
                        cx,
                    );
                }),
            ))
            .child(self.dialog_button(
                "obj-delete",
                t!("connection.delete").to_string(),
                false,
                cx.listener(move |this, _event, _window, cx| {
                    let (connection_index, database_index, is_view) =
                        object_scope(&pane_for_delete, cx);
                    if is_view {
                        return;
                    }
                    let mut names = this.objects_selection.items();
                    if names.is_empty() {
                        let Some((_, _, name, _)) = object_selection(&pane_for_delete, cx) else {
                            return;
                        };
                        names.push(name);
                    }
                    this.delete_confirm = Some(DeleteConfirm::Tables {
                        connection_index,
                        database_index,
                        names,
                    });
                    cx.notify();
                }),
            ));
        let ttl_connection = connection_index;
        let ttl_database = database_index;
        let ttl_key = selected_key.clone();
        left = left.child(ui::plain_button(
            "obj-key-ttl",
            t!("redis.tools.key_apply_ttl").to_string(),
            ttl_key.is_some(),
            theme,
            cx.listener(move |this, _event, _window, cx| {
                let Some(key) = ttl_key.clone() else {
                    return;
                };
                this.start_key_ttl_dialog(ttl_connection, ttl_database, key, cx);
            }),
        ));
        left = left.child(toolbar_separator(theme)).child(ui::plain_button(
            "obj-refresh",
            t!("common.refresh").to_string(),
            true,
            theme,
            cx.listener(move |this, _event, _window, cx| {
                this.reload_tables(connection_index, database_index, cx);
            }),
        ));
        if let Some(namespace) = namespace {
            left = left.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .ml_2()
                    .pl_2()
                    .pr_1()
                    .py_0p5()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.toolbar_bg))
                    .text_size(px(11.5))
                    .text_color(rgb(theme.text_muted))
                    .child(format!("{}: {namespace}*", t!("redis.key.directory")))
                    .child(
                        div()
                            .id("obj-key-namespace-clear")
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.select_key_namespace(
                                    connection_index,
                                    database_index,
                                    None,
                                    cx,
                                );
                            }))
                            .child("✕"),
                    ),
            );
        }

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
            .child(left)
            .child(self.render_object_view_controls(category, cx))
    }

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
        if category == Category::Views {
            return self.render_view_object_toolbar(pane, cx);
        }
        let designer_enabled = self.driver_supports_table_designer(connection_index);
        let key_value = self.driver_supports(connection_index, DriverCapability::KeyValue);
        if key_value {
            return self.render_key_object_toolbar(pane, theme, cx);
        }
        let open_label = if key_value {
            t!("redis.key.open").to_string()
        } else {
            t!("object.open_table").to_string()
        };
        let delete_label = if key_value {
            t!("redis.key.delete").to_string()
        } else {
            t!("object.delete_table").to_string()
        };
        let design_enabled = category == Category::Tables && open_enabled && designer_enabled;
        let new_enabled = category == Category::Tables && designer_enabled;
        // Deleting works off the multi-selection: enabled when at least one row is selected, even
        // if more than one (`pane.selected` is the single-selection mirror and is `None` for a
        // multi-row selection).
        let delete_enabled =
            category == Category::Tables && (open_enabled || !self.objects_selection.is_empty());
        // Export works off the multi-selection: enabled when at least one row is selected, even if
        // more than one (`pane.selected` is the single-selection mirror and is `None` for a
        // multi-row selection).
        let export_enabled =
            category == Category::Tables && (open_enabled || !self.objects_selection.is_empty());
        let import_enabled = category == Category::Tables
            && self
                .database_name(connection_index, database_index)
                .is_some();
        let pane_for_open = pane.clone();
        let pane_for_design = pane.clone();
        let pane_for_new = pane.clone();
        let pane_for_export = pane.clone();
        let pane_for_import = pane.clone();
        let pane_for_delete = pane.clone();
        let pane_for_refresh = pane.clone();
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
                        open_label.clone(),
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
                    .when(!key_value, |bar| {
                        bar.child(toolbar_separator(theme))
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
                                    this.open_design_table(
                                        connection_index,
                                        database,
                                        name,
                                        is_view,
                                        cx,
                                    );
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
                    })
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-delete",
                        "icons/delete_table.svg",
                        delete_label.clone(),
                        delete_enabled,
                        cx.listener(move |this, _event, _window, cx| {
                            let (connection_index, database_index, is_view) =
                                object_scope(&pane_for_delete, cx);
                            // Views are dropped from the Views toolbar; here only tables.
                            if is_view {
                                return;
                            }
                            let mut names = this.objects_selection.items();
                            if names.is_empty() {
                                let Some((_, _, name, _)) = object_selection(&pane_for_delete, cx)
                                else {
                                    return;
                                };
                                names.push(name);
                            }
                            this.delete_confirm = Some(DeleteConfirm::Tables {
                                connection_index,
                                database_index,
                                names,
                            });
                            cx.notify();
                        }),
                    ))
                    .when(!key_value, |bar| {
                        bar.child(toolbar_separator(theme)).child(self.toolbar_item(
                            "obj-import",
                            "icons/import.svg",
                            t!("object.import").to_string(),
                            import_enabled,
                            cx.listener(move |this, _event, _window, cx| {
                                let (connection_index, database_index) = {
                                    let pane = pane_for_import.read(cx);
                                    (pane.connection_index, pane.database_index)
                                };
                                this.open_import_wizard(connection_index, database_index, None, cx);
                            }),
                        ))
                    })
                    .when(!key_value, |bar| {
                        bar.child(toolbar_separator(theme)).child(self.toolbar_item(
                            "obj-export",
                            "icons/export.svg",
                            t!("object.export").to_string(),
                            export_enabled,
                            cx.listener(move |this, _event, _window, cx| {
                                let (connection_index, database_index, is_view) =
                                    object_scope(&pane_for_export, cx);
                                if is_view {
                                    return;
                                }
                                let names = this.objects_selection.items();
                                let names = if names.is_empty() {
                                    let Some((_, _, name, _)) =
                                        object_selection(&pane_for_export, cx)
                                    else {
                                        return;
                                    };
                                    vec![name]
                                } else {
                                    names
                                };
                                this.open_export_wizard(
                                    connection_index,
                                    database_index,
                                    &names,
                                    cx,
                                );
                            }),
                        ))
                    })
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "obj-refresh",
                        "icons/refresh.svg",
                        t!("common.refresh").to_string(),
                        true,
                        cx.listener(move |this, _event, _window, cx| {
                            let (connection_index, database_index) = {
                                let pane = pane_for_refresh.read(cx);
                                (pane.connection_index, pane.database_index)
                            };
                            this.reload_tables(connection_index, database_index, cx);
                        }),
                    )),
            )
            .child(self.render_object_view_controls(category, cx))
    }

    /// The object toolbar shown for the `Queries` category, including when no database is open.
    pub(super) fn render_query_object_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let delete_enabled = !self.selected_query_indices(cx).is_empty();
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
                            this.confirm_delete_selected_queries(cx);
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
            .child(self.render_object_view_controls(Category::Queries, cx))
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
        let pane_for_refresh = pane.clone();

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
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "routine-refresh",
                        "icons/refresh.svg",
                        t!("common.refresh").to_string(),
                        true,
                        cx.listener(move |this, _event, _window, cx| {
                            let (connection_index, database_index) = {
                                let pane = pane_for_refresh.read(cx);
                                (pane.connection_index, pane.database_index)
                            };
                            this.refresh_object_category(
                                Category::Functions,
                                connection_index,
                                database_index,
                                cx,
                            );
                        }),
                    )),
            )
            .child(self.render_object_view_controls(Category::Functions, cx))
    }

    /// The toolbar for the Views object list (Navicat's 打开视图 / 设计视图 / 新建视图 /
    /// 删除视图 / 导出向导).
    fn render_view_object_toolbar(
        &self,
        pane: &Entity<ObjectPane>,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let theme = self.theme;
        let has_selection = {
            let pane = pane.read(cx);
            pane.selected.is_some()
        };
        // Export works off the multi-selection, so it stays enabled for more than one selected row.
        let export_enabled = has_selection || !self.objects_selection.is_empty();
        let has_database = {
            let pane = pane.read(cx);
            self.database_name(pane.connection_index, pane.database_index)
                .is_some()
        };
        let pane_for_open = pane.clone();
        let pane_for_design = pane.clone();
        let pane_for_new = pane.clone();
        let pane_for_delete = pane.clone();
        let pane_for_export = pane.clone();
        let pane_for_refresh = pane.clone();

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
                        "view-open",
                        "icons/views.svg",
                        t!("view.open").to_string(),
                        has_selection,
                        cx.listener(move |this, _event, _window, cx| {
                            let Some((connection_index, database_index, name, _)) =
                                object_selection(&pane_for_open, cx)
                            else {
                                return;
                            };
                            let Some(database) =
                                this.database_name(connection_index, database_index)
                            else {
                                return;
                            };
                            this.select_table(connection_index, database, name, true, cx);
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "view-design",
                        "icons/design_table.svg",
                        t!("view.design").to_string(),
                        has_selection,
                        cx.listener(move |this, _event, _window, cx| {
                            let Some((connection_index, database_index, name, _)) =
                                object_selection(&pane_for_design, cx)
                            else {
                                return;
                            };
                            let Some(database) =
                                this.database_name(connection_index, database_index)
                            else {
                                return;
                            };
                            this.open_view(connection_index, database, name, cx);
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "view-new",
                        "icons/new_table.svg",
                        t!("view.new").to_string(),
                        has_database,
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
                            this.open_new_view(connection_index, database, cx);
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "view-delete",
                        "icons/delete_table.svg",
                        t!("view.delete").to_string(),
                        has_selection,
                        cx.listener(move |this, _event, _window, cx| {
                            let Some((connection_index, database_index, name, _)) =
                                object_selection(&pane_for_delete, cx)
                            else {
                                return;
                            };
                            this.confirm_delete_view(connection_index, database_index, name, cx);
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "view-export",
                        "icons/export.svg",
                        t!("object.export").to_string(),
                        export_enabled,
                        cx.listener(move |this, _event, _window, cx| {
                            let (connection_index, database_index, _) =
                                object_scope(&pane_for_export, cx);
                            let names = this.objects_selection.items();
                            let names = if names.is_empty() {
                                let Some((_, _, name, _)) = object_selection(&pane_for_export, cx)
                                else {
                                    return;
                                };
                                vec![name]
                            } else {
                                names
                            };
                            this.open_export_wizard(connection_index, database_index, &names, cx);
                        }),
                    ))
                    .child(toolbar_separator(theme))
                    .child(self.toolbar_item(
                        "view-refresh",
                        "icons/refresh.svg",
                        t!("common.refresh").to_string(),
                        true,
                        cx.listener(move |this, _event, _window, cx| {
                            let (connection_index, database_index) = {
                                let pane = pane_for_refresh.read(cx);
                                (pane.connection_index, pane.database_index)
                            };
                            this.refresh_object_category(
                                Category::Views,
                                connection_index,
                                database_index,
                                cx,
                            );
                        }),
                    )),
            )
            .child(self.render_object_view_controls(Category::Views, cx))
    }

    /// The 详细列表 / 平铺网格 switch plus the search box, shared by the object toolbars. The
    /// switch sits to the left of, and adjacent to, the search box.
    pub(super) fn render_object_view_controls(
        &self,
        category: Category,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let theme = self.theme;
        let page = view_page_for_category(category);
        let weak = cx.weak_entity();
        let on_select = Rc::new(
            move |mode: ViewMode, _event: &ClickEvent, _window: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |app, cx| app.set_view_mode(page, mode, cx));
            },
        );
        let view_mode = self.view_mode(page);
        // The key list has one layout (the detailed key table), so its 详细列表 / 平铺网格 toggle
        // is not shown at all.
        let key_list = category == Category::Tables
            && self
                .object_pane
                .as_ref()
                .is_some_and(|pane| pane.read(cx).key_value);
        let mut controls = div().flex().flex_row().items_center().gap_2();
        if !key_list {
            controls = controls.child(ui::view_mode_toggle(theme, view_mode, on_select));
        }
        controls.child(self.render_object_search(cx))
    }

    pub(super) fn render_object_search(&self, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .w(px(220.0))
            .h(px(24.0))
            .child(self.object_search_input.clone())
    }

    /// The list page whose search the shared box currently edits: the saved-query list on the 查询
    /// main tab, otherwise the object list's category (表 / 视图 / 函数). `None` when the shown page
    /// keeps its own search box (用户, 备份) or the content pane shows something else entirely.
    fn object_search_page(&self, cx: &App) -> Option<&'static str> {
        if self.main_tab == MainTab::Queries {
            return Some(VIEW_PAGE_QUERIES);
        }
        let category = self
            .object_pane
            .as_ref()
            .map(|pane| pane.read(cx).category)?;
        match category {
            Category::Tables | Category::Views | Category::Functions => {
                Some(view_page_for_category(category))
            }
            _ => None,
        }
    }

    /// One list's search text (`VIEW_PAGE_*`), empty when nothing was typed there.
    pub(super) fn object_search_text(&self, page: &str) -> String {
        self.object_searches.get(page).cloned().unwrap_or_default()
    }

    /// Store the search box's text under the page it edits. The box is one field shared by several
    /// lists, so the text is filed per page: 表's search never filters 查询, and so on.
    pub(super) fn set_object_search(&mut self, text: String, cx: &mut Context<'_, Self>) {
        if let Some(page) = self.object_search_page(cx) {
            if text.is_empty() {
                self.object_searches.remove(page);
            } else {
                self.object_searches.insert(page.to_string(), text);
            }
        }
        self.notify_object_pane(cx);
        cx.notify();
    }

    /// Keep the search box showing the active list's own text: switching 表 ↔ 视图 ↔ 函数 ↔ 查询
    /// swaps the field's content instead of letting one list's search filter another.
    pub(super) fn sync_object_search(&mut self, cx: &mut Context<'_, Self>) {
        let page = self.object_search_page(cx);
        if self.search_box_page == page {
            return;
        }
        self.search_box_page = page;
        let Some(page) = page else {
            return;
        };
        let text = self.object_search_text(page);
        self.object_search_input
            .update(cx, |input, cx| input.set_text(text, cx));
    }

    /// Clear the shown list's search and list every object again.
    pub(super) fn clear_object_search(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(page) = self.object_search_page(cx) {
            self.object_searches.remove(page);
        }
        self.object_search_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.notify_object_pane(cx);
        cx.notify();
    }

    /// Clear every list's search unless 保留对象搜索 is on: the searches are then kept while the
    /// shown tab, database, schema or connection changes. With the setting off a switch forgets
    /// them all, so coming back shows every object again.
    pub(super) fn clear_object_search_unless_kept(&mut self, cx: &mut Context<'_, Self>) {
        if self.preserve_object_search || self.object_searches.is_empty() {
            return;
        }
        self.object_searches.clear();
        self.object_search_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.notify_object_pane(cx);
        cx.notify();
    }

    /// Clear one main tab's own search. Every click on a main tab runs this, so re-entering 表
    /// after filtering shows the whole list again — Navicat's behaviour — no matter what
    /// 保留对象搜索 says. That setting now only decides whether a database/connection switch keeps
    /// the searches; it no longer makes a tab's box impossible to clear.
    pub(super) fn clear_main_tab_search(&mut self, tab: MainTab, cx: &mut Context<'_, Self>) {
        let page = match tab {
            MainTab::Tables => Some(VIEW_PAGE_TABLES),
            MainTab::Views => Some(VIEW_PAGE_VIEWS),
            MainTab::Functions => Some(VIEW_PAGE_FUNCTIONS),
            MainTab::Queries => Some(VIEW_PAGE_QUERIES),
            _ => None,
        };
        if let Some(page) = page {
            let had = self.object_searches.remove(page).is_some();
            if self.search_box_page == Some(page) {
                self.object_search_input
                    .update(cx, |input, cx| input.set_text("", cx));
            }
            if had {
                self.notify_object_pane(cx);
                cx.notify();
            }
        }
        if tab == MainTab::Users && !self.user_search.is_empty() {
            self.user_search.clear();
            self.user_search_input
                .update(cx, |input, cx| input.set_text("", cx));
            cx.notify();
        }
        if tab == MainTab::Backups && !self.backup_search.is_empty() {
            self.backup_search.clear();
            self.backup_search_input
                .update(cx, |input, cx| input.set_text("", cx));
            cx.notify();
        }
    }

    /// The content of the object status shown in the window's bottom status bar: item count on the
    /// left, connection and database on the right (Navicat layout).
    pub(super) fn render_object_status(
        &self,
        pane: &Entity<ObjectPane>,
        cx: &Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let (connection_index, database_index, schema, category) = {
            let pane = pane.read(cx);
            (
                pane.connection_index,
                pane.database_index,
                pane.schema.clone(),
                pane.category,
            )
        };
        let connection_name = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let database_name = self
            .database_name(connection_index, database_index)
            .unwrap_or_default();
        let count = self.object_count(
            connection_index,
            database_index,
            schema.as_deref(),
            category,
        );
        let noun = if category == Category::Tables
            && self.driver_supports(connection_index, DriverCapability::KeyValue)
        {
            t!("redis.key.noun").to_string()
        } else {
            t!(category.label()).to_string()
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px_2()
            .text_size(px(12.0))
            .child(
                div()
                    .text_color(rgb(theme.text))
                    .child(format!("{count} {noun}")),
            )
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
        schema: Option<&str>,
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
        let in_schema = |name: &str| schema.is_none_or(|scope| schema_of(name) == Some(scope));
        if category == Category::Functions {
            return match &database.routines {
                Loadable::Loaded(routines) => routines
                    .iter()
                    .filter(|routine| in_schema(&routine.name))
                    .count(),
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
            .filter(|table| {
                matches!(table.kind, rustgrid_core::ObjectKind::View) == want_view
                    && in_schema(&table.name)
            })
            .count()
    }
}

impl ObjectPane {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        app: WeakEntity<AppView>,
        connection_index: usize,
        database_index: usize,
        schema: Option<String>,
        category: Category,
        key_value: bool,
        theme: Theme,
        grid_viewport: f32,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self {
            app,
            connection_index,
            database_index,
            schema,
            category,
            key_value,
            selected: None,
            selected_routine: None,
            grid: ColumnGrid::with_viewport(grid_viewport),
            detail_scroll: DetailScroll::default(),
            table_columns: Rc::new(RefCell::new(DetailColumns::default())),
            routine_columns: Rc::new(RefCell::new(DetailColumns::default())),
            table_sort: DetailSort::default(),
            routine_sort: DetailSort::default(),
            visible_keys: Vec::new(),
            focus: cx.focus_handle(),
            theme,
            mode: ViewMode::default(),
            typeahead: String::new(),
            typeahead_at: None,
            nav_generation: 0,
        }
    }

    /// The Tables/Views 详细列表's active sort, ignoring a column the current category does not
    /// show (the pane stays while the main tab switches Tables ↔ Views).
    fn effective_table_sort(&self) -> DetailSort {
        let columns = if self.category == Category::Views {
            2
        } else if self.key_value {
            6
        } else {
            7
        };
        if self.table_sort.column < columns {
            self.table_sort
        } else {
            DetailSort::default()
        }
    }

    /// Ask `AppView` to re-render, e.g. so the toolbar's Open button reflects the selection.
    fn notify_app(&self, cx: &mut Context<'_, Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |_, cx| cx.notify());
        }
    }

    /// Render the list body from a snapshot of the database's tables or routines.
    fn render_body(&mut self, data: ObjectRenderData, cx: &mut Context<'_, Self>) -> AnyElement {
        let ObjectRenderData {
            mode,
            schema,
            supports_schemas,
            tables,
            table_statuses,
            routines,
            search,
            selected,
            rename,
        } = data;
        let theme = self.theme;
        // A schema-qualified engine lists nothing until a schema is selected: showing every
        // schema's objects would look like the schema was never opened.
        if supports_schemas && schema.is_none() && self.category != Category::Queries {
            self.visible_keys.clear();
            return object_message(theme.text_muted, t!("object.select_schema").to_string());
        }
        let query = search.trim().to_lowercase();
        self.visible_keys.clear();
        if self.category == Category::Functions {
            let Some(routines) = routines else {
                return div().into_any_element();
            };
            return match routines {
                Loadable::Idle => div().into_any_element(),
                Loadable::Loading => {
                    object_message(theme.text_muted, t!("common.loading").to_string())
                }
                Loadable::Failed(error) => object_message(theme.danger, error),
                Loadable::Loaded(routines) => {
                    let items: Vec<&RoutineInfo> = routines
                        .iter()
                        .filter(|routine| {
                            schema
                                .as_deref()
                                .is_none_or(|scope| schema_of(&routine.name) == Some(scope))
                                && (query.is_empty()
                                    || routine.name.to_lowercase().contains(&query))
                        })
                        .collect();
                    let mut items = items;
                    // 名 / 修改日期 / 函数类型 / 决定性 / 注释, by the header the user clicked.
                    let sort = self.routine_sort;
                    sort.sort_by(&mut items, |routine| routine_sort_key(routine, sort.column));
                    self.visible_keys
                        .extend(items.iter().map(|routine| routine.name.clone()));
                    match mode {
                        ViewMode::Detail => {
                            self.render_routine_detail(&items, &selected, theme, cx)
                        }
                        ViewMode::Grid => self.render_routine_grid(&items, &selected, theme, cx),
                    }
                }
            };
        }
        let Some(tables) = tables else {
            return div().into_any_element();
        };
        match tables {
            Loadable::Idle => div().into_any_element(),
            Loadable::Loading => match mode {
                // Lay the empty 平铺网格 out while the tables load: its scroller then reports the
                // real viewport height before the first row arrives, so the loaded list wraps its
                // columns correctly on its very first frame instead of reflowing afterwards.
                ViewMode::Grid => {
                    let placeholder = ui::grid_columns().child(ui::grid_column().child(
                        object_message(theme.text_muted, t!("common.loading").to_string()),
                    ));
                    self.render_grid(placeholder, theme, cx)
                }
                ViewMode::Detail => {
                    object_message(theme.text_muted, t!("common.loading").to_string())
                }
            },
            Loadable::Failed(error) => object_message(theme.danger, error),
            Loadable::Loaded(tables) => {
                let want_view = match self.category {
                    Category::Tables => Some(false),
                    Category::Views => Some(true),
                    _ => None,
                };
                let Some(want_view) = want_view else {
                    return div().into_any_element();
                };
                let items: Vec<&rustgrid_core::TableInfo> = tables
                    .iter()
                    .filter(|table| {
                        matches!(table.kind, rustgrid_core::ObjectKind::View) == want_view
                            && schema
                                .as_deref()
                                .is_none_or(|scope| schema_of(&table.name) == Some(scope))
                            && (query.is_empty() || table.name.to_lowercase().contains(&query))
                    })
                    .collect();
                let mut items = items;
                // The clicked header decides the order (name order by default, Navicat's default),
                // not the driver's catalog order.
                let category = self.category;
                let key_value = self.key_value;
                let sort = self.effective_table_sort();
                sort.sort_by(&mut items, |table| {
                    let status = table_statuses.as_ref().and_then(|list| {
                        list.iter()
                            .find(|(name, _)| name == &table.name)
                            .map(|(_, status)| status)
                    });
                    if key_value && category == Category::Tables {
                        key_sort_key(table, status, sort.column)
                    } else {
                        table_sort_key(table, status, category, sort.column)
                    }
                });
                self.visible_keys
                    .extend(items.iter().map(|table| table.name.clone()));
                if self.key_value && self.category == Category::Tables {
                    return match mode {
                        ViewMode::Detail => self.render_key_detail(
                            &items,
                            table_statuses.as_deref(),
                            &selected,
                            rename.as_ref(),
                            theme,
                            cx,
                        ),
                        ViewMode::Grid => {
                            self.render_table_grid(&items, &selected, rename.as_ref(), theme, cx)
                        }
                    };
                }
                match mode {
                    ViewMode::Detail => self.render_table_detail(
                        &items,
                        table_statuses.as_deref(),
                        &selected,
                        rename.as_ref(),
                        theme,
                        cx,
                    ),
                    ViewMode::Grid => {
                        self.render_table_grid(&items, &selected, rename.as_ref(), theme, cx)
                    }
                }
            }
        }
    }

    /// The content-fitted widths of the Tables/Views 详细列表 columns, matching the header order of
    /// [`render_table_detail`](Self::render_table_detail).
    fn table_detail_widths(
        &self,
        tables: &[&rustgrid_core::TableInfo],
        statuses: Option<&[(String, rustgrid_core::TableStatus)]>,
    ) -> Vec<f32> {
        let yes = t!("common.yes").to_string();
        let no = t!("common.no").to_string();
        let mut longest = vec![ui::approx_text_width(&t!("common.name")) + 30.0];
        match self.category {
            Category::Views => {
                longest.push(ui::approx_text_width(&t!("view.field.updatable")));
            }
            Category::Tables => longest.extend([
                ui::approx_text_width(&t!("table.col.auto_increment")),
                ui::approx_text_width(&t!("table.col.modified")),
                ui::approx_text_width(&t!("table.col.data_length")),
                ui::approx_text_width(&t!("table.col.engine")),
                ui::approx_text_width(&t!("table.col.rows")),
                ui::approx_text_width(&t!("table.col.comment")),
            ]),
            _ => {}
        }
        for table in tables {
            longest[0] = longest[0].max(ui::approx_text_width(&object_display(&table.name)) + 30.0);
            let status = statuses.and_then(|list| {
                list.iter()
                    .find(|(name, _)| name == &table.name)
                    .map(|(_, status)| status)
            });
            match self.category {
                Category::Views => {
                    longest[1] = longest[1].max(ui::approx_text_width(if table.updatable {
                        &yes
                    } else {
                        &no
                    }));
                }
                Category::Tables => {
                    if let Some(status) = status {
                        longest[1] = longest[1].max(ui::approx_text_width(
                            &status
                                .auto_increment
                                .map(|value| value.to_string())
                                .unwrap_or_default(),
                        ));
                        longest[2] = longest[2].max(ui::approx_text_width(
                            &status.updated.clone().unwrap_or_default(),
                        ));
                        longest[3] = longest[3].max(ui::approx_text_width(
                            &status.data_length.map(human_size).unwrap_or_default(),
                        ));
                        longest[4] = longest[4].max(ui::approx_text_width(
                            &status.engine.clone().unwrap_or_default(),
                        ));
                        longest[5] = longest[5].max(ui::approx_text_width(
                            &status
                                .rows
                                .map(|value| value.to_string())
                                .unwrap_or_default(),
                        ));
                        longest[6] = longest[6].max(ui::approx_text_width(&single_line(
                            &status.comment.clone().unwrap_or_default(),
                        )));
                    }
                }
                _ => {}
            }
        }
        let mins: &[f32] = match self.category {
            Category::Views => &[220.0, 100.0],
            Category::Tables => &[220.0, 110.0, 140.0, 100.0, 90.0, 80.0, 160.0],
            _ => &[220.0],
        };
        longest
            .iter()
            .zip(mins)
            .map(|(longest, min)| ui::detail_column_width(*longest, *min))
            .collect()
    }

    /// The Tables/Views 详细列表. Tables show Navicat's overview columns (auto-increment, modified,
    /// data length, engine, rows, comment); views show 名 + 可以更新.
    fn render_table_detail(
        &self,
        tables: &[&rustgrid_core::TableInfo],
        statuses: Option<&[(String, rustgrid_core::TableStatus)]>,
        selected: &std::collections::HashSet<String>,
        rename: Option<&RenameRow>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let labels: Vec<String> = match self.category {
            Category::Views => vec![
                t!("common.name").to_string(),
                t!("view.field.updatable").to_string(),
            ],
            Category::Tables => vec![
                t!("common.name").to_string(),
                t!("table.col.auto_increment").to_string(),
                t!("table.col.modified").to_string(),
                t!("table.col.data_length").to_string(),
                t!("table.col.engine").to_string(),
                t!("table.col.rows").to_string(),
                t!("table.col.comment").to_string(),
            ],
            _ => vec![t!("common.name").to_string()],
        };
        let fitted = self.table_detail_widths(tables, statuses);
        let widths = self.table_columns.borrow_mut().resolve(&fitted);
        let sort = self.effective_table_sort();
        let mut header = ui::detail_header_row(theme);
        for (index, label) in labels.into_iter().enumerate() {
            let width = widths[index];
            let cell = ui::detail_header_cell(
                SharedString::from(format!("object-sort-{index}")),
                label,
                sort.indicator(index),
                theme,
                cx.listener(move |this, _event, _window, cx| {
                    this.table_sort.toggle(index);
                    cx.notify();
                }),
            );
            header = header.child(ui::detail_header_column(
                SharedString::from(format!("object-resize-{index}")),
                width,
                self.table_columns.borrow().resizing(index),
                theme,
                cell,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.table_columns
                        .borrow_mut()
                        .begin_resize(index, event.position.x, width);
                    cx.notify();
                }),
            ));
        }

        let mut list = ui::DetailList::new(
            "object-detail",
            &self.detail_scroll,
            ui::detail_content_width(&widths),
            header,
        );
        for table in tables {
            let key = table.name.clone();
            let status = statuses.and_then(|list| {
                list.iter()
                    .find(|(name, _)| name == &table.name)
                    .map(|(_, status)| status)
            });
            let row = self.table_row(
                table,
                status,
                selected.contains(&key),
                rename,
                &widths,
                theme,
                cx,
            );
            list = list.child(object_row_with_rect(self.app.clone(), row, key));
        }
        list.render(theme)
    }

    /// The key-value list: 键名 / 类型 / TTL / 内存 / 值 (the HTML prototype's key table).
    fn render_key_detail(
        &self,
        tables: &[&rustgrid_core::TableInfo],
        statuses: Option<&[(String, rustgrid_core::TableStatus)]>,
        selected: &std::collections::HashSet<String>,
        rename: Option<&RenameRow>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let labels = vec![
            t!("redis.key.group").to_string(),
            t!("redis.key.name").to_string(),
            t!("redis.key.type").to_string(),
            t!("redis.key.ttl").to_string(),
            t!("redis.key.memory").to_string(),
            t!("redis.key.value").to_string(),
        ];
        let mut longest = [
            ui::approx_text_width(&labels[0]),
            ui::approx_text_width(&labels[1]) + 30.0,
            ui::approx_text_width(&labels[2]),
            ui::approx_text_width(&labels[3]),
            ui::approx_text_width(&labels[4]),
            ui::approx_text_width(&labels[5]),
        ];
        for table in tables {
            let status = statuses.and_then(|list| {
                list.iter()
                    .find(|(name, _)| name == &table.name)
                    .map(|(_, status)| status)
            });
            longest[0] = longest[0].max(ui::approx_text_width(&key_group(&table.name)));
            longest[1] = longest[1].max(ui::approx_text_width(&object_display(&table.name)) + 30.0);
            longest[2] = longest[2].max(ui::approx_text_width(
                status
                    .and_then(|status| status.engine.as_deref())
                    .unwrap_or_default(),
            ));
            longest[3] = longest[3].max(ui::approx_text_width(
                &status
                    .and_then(|status| status.ttl)
                    .map(|ttl| ttl.to_string())
                    .unwrap_or_default(),
            ));
            longest[4] = longest[4].max(ui::approx_text_width(
                &status
                    .and_then(|status| status.data_length)
                    .map(human_size)
                    .unwrap_or_default(),
            ));
            longest[5] = longest[5].max(ui::approx_text_width(&key_preview(status)));
        }
        let mins: [f32; 6] = [120.0, 220.0, 90.0, 80.0, 100.0, 160.0];
        let fitted: Vec<f32> = longest
            .iter()
            .zip(mins)
            .map(|(longest, min)| ui::detail_column_width(*longest, min))
            .collect();
        let widths = self.table_columns.borrow_mut().resolve(&fitted);
        let sort = self.effective_table_sort();
        let mut header = ui::detail_header_row(theme);
        for (index, label) in labels.into_iter().enumerate() {
            let width = widths[index];
            let cell = ui::detail_header_cell(
                SharedString::from(format!("key-sort-{index}")),
                label,
                sort.indicator(index),
                theme,
                cx.listener(move |this, _event, _window, cx| {
                    this.table_sort.toggle(index);
                    cx.notify();
                }),
            );
            header = header.child(ui::detail_header_column(
                SharedString::from(format!("key-resize-{index}")),
                width,
                self.table_columns.borrow().resizing(index),
                theme,
                cell,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.table_columns
                        .borrow_mut()
                        .begin_resize(index, event.position.x, width);
                    cx.notify();
                }),
            ));
        }

        let mut list = ui::DetailList::new(
            "key-detail",
            &self.detail_scroll,
            ui::detail_content_width(&widths),
            header,
        );
        for table in tables {
            let key = table.name.clone();
            let status = statuses.and_then(|list| {
                list.iter()
                    .find(|(name, _)| name == &table.name)
                    .map(|(_, status)| status)
            });
            let row = self.key_row(
                table,
                status,
                selected.contains(&key),
                rename,
                &widths,
                theme,
                cx,
            );
            list = list.child(object_row_with_rect(self.app.clone(), row, key));
        }
        list.render(theme)
    }

    /// One key row of the key list, with its type badge, TTL, memory and value preview.
    #[allow(clippy::too_many_arguments)]
    fn key_row(
        &self,
        table: &rustgrid_core::TableInfo,
        status: Option<&rustgrid_core::TableStatus>,
        selected: bool,
        rename: Option<&RenameRow>,
        widths: &[f32],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        let name = table.name.clone();
        let label_name = object_display(&name);
        let hit_key = name.clone();
        let open_name = name.clone();
        let menu_name = name.clone();
        let menu_app = self.app.clone();
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
                .h(px(22.0))
                .child(ui::rename_field(theme, row.input.clone()))
                .into_any_element(),
            None => div()
                .min_w(px(0.0))
                .child(ui::detail_cell_text(
                    SharedString::from(format!(
                        "key-name-{connection_index}-{database_index}-{name}"
                    )),
                    label_name,
                ))
                .into_any_element(),
        };
        let kind = status
            .and_then(|status| status.engine.clone())
            .unwrap_or_default();
        let badge = key_type_badge(&kind, theme);
        let name_cell = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w(px(widths[1]))
            .flex_none()
            .overflow_hidden()
            .child(badge)
            .child(label);
        let kind_text = status
            .and_then(|status| status.engine.as_deref())
            .unwrap_or_default()
            .to_string();
        let ttl = status.and_then(|status| status.ttl);
        let ttl_text = ttl.map(|ttl| ttl.to_string()).unwrap_or_default();
        let ttl_color = if ttl.is_some_and(|ttl| ttl >= 0) {
            theme.warning
        } else {
            theme.text_muted
        };
        let memory = status
            .and_then(|status| status.data_length)
            .map(human_size)
            .unwrap_or_default();
        let cell = |column: usize, width: f32, text: String, color: u32| {
            div()
                .w(px(width))
                .flex_none()
                .text_color(rgb(color))
                .child(ui::detail_cell_text(
                    SharedString::from(format!(
                        "key-cell-{connection_index}-{database_index}-{name}-{column}"
                    )),
                    text,
                ))
        };

        ui::detail_row(
            SharedString::from(format!("key-{connection_index}-{database_index}-{name}")),
            selected,
            theme,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            if editing_here {
                return;
            }
            let opens = this.click_opens(event.click_count(), event.modifiers(), cx);
            this.commit_pending_rename(cx);
            window.focus(&this.focus, cx);
            this.hit_object(&hit_key, event.modifiers(), cx);
            if opens {
                this.open_object_named(open_name.clone(), cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                if editing_here {
                    return;
                }
                cx.stop_propagation();
                window.focus(&this.focus, cx);
                this.ensure_object_selected(&menu_name, cx);
                let _ = menu_app.update(cx, |app, cx| {
                    app.context_menu = Some(ContextMenu {
                        target: ContextTarget::Table {
                            connection_index,
                            database_index,
                            name: menu_name.clone(),
                            is_view: false,
                            pane: RowPane::Objects,
                        },
                        position: event.position,
                    });
                    cx.notify();
                });
            }),
        )
        .child(cell(0, widths[0], key_group(&name), theme.text_muted))
        .child(name_cell)
        .child(cell(2, widths[2], kind_text, theme.text_muted))
        .child(cell(3, widths[3], ttl_text, ttl_color))
        .child(cell(4, widths[4], memory, theme.text_muted))
        .child(cell(5, widths[5], key_preview(status), theme.text_muted))
    }

    /// One table/view row of the 详细列表.
    #[allow(clippy::too_many_arguments)]
    fn table_row(
        &self,
        table: &rustgrid_core::TableInfo,
        status: Option<&rustgrid_core::TableStatus>,
        selected: bool,
        rename: Option<&RenameRow>,
        widths: &[f32],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let is_view = matches!(table.kind, rustgrid_core::ObjectKind::View);
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        let name = table.name.clone();
        let label_name = object_display(&name);
        let hit_key = name.clone();
        let open_name = name.clone();
        let menu_name = name.clone();
        let cell_key = name.clone();
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
                .h(px(22.0))
                .child(ui::rename_field(theme, row.input.clone()))
                .into_any_element(),
            None => div()
                .min_w(px(0.0))
                .child(ui::detail_cell_text(
                    SharedString::from(format!(
                        "obj-name-{connection_index}-{database_index}-{name}"
                    )),
                    label_name,
                ))
                .into_any_element(),
        };
        let name_cell = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w(px(widths[0]))
            .flex_none()
            .overflow_hidden()
            .child(ui::leading_icon_badge(
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
                ui::DETAIL_BADGE_SIZE,
            ))
            .child(label);
        ui::detail_row(
            SharedString::from(format!("obj-{connection_index}-{database_index}-{name}")),
            selected,
            theme,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            if editing_here {
                return;
            }
            let opens = this.click_opens(event.click_count(), event.modifiers(), cx);
            this.commit_pending_rename(cx);
            window.focus(&this.focus, cx);
            this.hit_object(&hit_key, event.modifiers(), cx);
            if opens {
                this.open_object_named(open_name.clone(), cx);
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
                this.ensure_object_selected(&menu_name, cx);
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
                // The blank-area menu on the ObjectPane container is a fallback; a row's own
                // menu must win.
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .child(name_cell)
        .when(self.category == Category::Views, |row| {
            row.child(
                div()
                    .w(px(widths[1]))
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(rgb(theme.text_muted))
                    .child(if table.updatable {
                        t!("common.yes").to_string()
                    } else {
                        t!("common.no").to_string()
                    }),
            )
        })
        .when(self.category == Category::Tables, |row| {
            let (auto_increment, modified, data_length, engine, rows, comment) = match status {
                Some(status) => (
                    status
                        .auto_increment
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                    status.updated.clone().unwrap_or_default(),
                    status.data_length.map(human_size).unwrap_or_default(),
                    status.engine.clone().unwrap_or_default(),
                    status
                        .rows
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                    single_line(&status.comment.clone().unwrap_or_default()),
                ),
                None => (
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                ),
            };
            let cell = |column: usize, width: f32, text: String| {
                div()
                    .w(px(width))
                    .flex_none()
                    .text_color(rgb(theme.text_muted))
                    .child(ui::detail_cell_text(
                        SharedString::from(format!(
                            "obj-cell-{connection_index}-{database_index}-{cell_key}-{column}"
                        )),
                        text,
                    ))
            };
            row.child(cell(1, widths[1], auto_increment))
                .child(cell(2, widths[2], modified))
                .child(cell(3, widths[3], data_length))
                .child(cell(4, widths[4], engine))
                .child(cell(5, widths[5], rows))
                .child(cell(6, widths[6], comment))
        })
    }

    /// The Tables/Views 平铺网格: the column-major icon+name tiles.
    fn render_table_grid(
        &self,
        tables: &[&rustgrid_core::TableInfo],
        selected: &std::collections::HashSet<String>,
        rename: Option<&RenameRow>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let width = ui::grid_item_width(
            tables
                .iter()
                .map(|table| ui::approx_text_width(&object_display(&table.name)))
                .fold(0.0, f32::max),
        );
        let rows = self.grid.rows_per_column();
        let mut columns = ui::grid_columns();
        let mut column = ui::grid_column();
        let mut count = 0usize;
        for table in tables {
            if count == rows {
                columns = columns.child(column);
                column = ui::grid_column();
                count = 0;
            }
            let key = table.name.clone();
            let tile = self.table_tile(table, selected.contains(&key), rename, theme, width, cx);
            column = column.child(object_row_with_rect(self.app.clone(), tile, key));
            count += 1;
        }
        if count > 0 {
            columns = columns.child(column);
        }
        self.render_grid(columns, theme, cx)
    }

    /// One table/view tile of the 平铺网格.
    #[allow(clippy::too_many_arguments)]
    fn table_tile(
        &self,
        table: &rustgrid_core::TableInfo,
        selected: bool,
        rename: Option<&RenameRow>,
        theme: Theme,
        width: f32,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let is_view = matches!(table.kind, rustgrid_core::ObjectKind::View);
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        let name = table.name.clone();
        let label_name = object_display(&name);
        let hit_key = name.clone();
        let open_name = name.clone();
        let menu_name = name.clone();
        let menu_app = self.app.clone();
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
                .h(px(20.0))
                .child(ui::rename_field(theme, row.input.clone()))
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(theme.text))
                .child(label_name)
                .into_any_element(),
        };
        ui::grid_item_sized(
            SharedString::from(format!(
                "obj-tile-{connection_index}-{database_index}-{name}"
            )),
            selected,
            theme,
            width,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            if editing_here {
                return;
            }
            let opens = this.click_opens(event.click_count(), event.modifiers(), cx);
            this.commit_pending_rename(cx);
            window.focus(&this.focus, cx);
            this.hit_object(&hit_key, event.modifiers(), cx);
            if opens {
                this.open_object_named(open_name.clone(), cx);
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
                this.ensure_object_selected(&menu_name, cx);
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
                // The blank-area menu on the ObjectPane container is a fallback; a row's own
                // menu must win.
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .child(ui::leading_icon_badge(
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
            16.0,
        ))
        .child(label)
    }

    /// The content-fitted widths of the Functions 详细列表 columns (名 / 修改日期 / 函数类型 /
    /// 决定性 / 注释).
    fn routine_detail_widths(&self, routines: &[&RoutineInfo]) -> Vec<f32> {
        let yes = t!("common.yes").to_string();
        let no = t!("common.no").to_string();
        let mut longest = [
            ui::approx_text_width(&t!("common.name")) + 30.0,
            ui::approx_text_width(&t!("routine.col.modified")),
            ui::approx_text_width(&t!("routine.col.kind")),
            ui::approx_text_width(&t!("routine.col.deterministic")),
            ui::approx_text_width(&t!("routine.field.comment")),
        ];
        for routine in routines {
            longest[0] =
                longest[0].max(ui::approx_text_width(&object_display(&routine.name)) + 30.0);
            longest[1] = longest[1].max(ui::approx_text_width(
                &routine.modified.clone().unwrap_or_default(),
            ));
            longest[2] = longest[2].max(ui::approx_text_width(routine.kind.sql_name()));
            longest[3] = longest[3].max(ui::approx_text_width(if routine.deterministic {
                &yes
            } else {
                &no
            }));
            longest[4] = longest[4].max(ui::approx_text_width(&single_line(&routine.comment)));
        }
        vec![
            ui::detail_column_width(longest[0], 220.0),
            ui::detail_column_width(longest[1], 140.0),
            ui::detail_column_width(longest[2], 100.0),
            ui::detail_column_width(longest[3], 90.0),
            ui::detail_column_width(longest[4], 160.0),
        ]
    }

    /// The Functions 详细列表: an icon plus the routine name.
    fn render_routine_detail(
        &self,
        routines: &[&RoutineInfo],
        selected: &std::collections::HashSet<String>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let fitted = self.routine_detail_widths(routines);
        let widths = self.routine_columns.borrow_mut().resolve(&fitted);
        let mut header = ui::detail_header_row(theme);
        for (index, label) in [
            t!("common.name").to_string(),
            t!("routine.col.modified").to_string(),
            t!("routine.col.kind").to_string(),
            t!("routine.col.deterministic").to_string(),
            t!("routine.field.comment").to_string(),
        ]
        .into_iter()
        .enumerate()
        {
            let width = widths[index];
            let cell = ui::detail_header_cell(
                SharedString::from(format!("routine-sort-{index}")),
                label,
                self.routine_sort.indicator(index),
                theme,
                cx.listener(move |this, _event, _window, cx| {
                    this.routine_sort.toggle(index);
                    cx.notify();
                }),
            );
            header = header.child(ui::detail_header_column(
                SharedString::from(format!("routine-resize-{index}")),
                width,
                self.routine_columns.borrow().resizing(index),
                theme,
                cell,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.routine_columns
                        .borrow_mut()
                        .begin_resize(index, event.position.x, width);
                    cx.notify();
                }),
            ));
        }
        let mut list = ui::DetailList::new(
            "routine-detail",
            &self.detail_scroll,
            ui::detail_content_width(&widths),
            header,
        );
        for routine in routines {
            let key = routine.name.clone();
            let row = self.routine_row(routine, selected.contains(&key), &widths, theme, cx);
            list = list.child(object_row_with_rect(self.app.clone(), row, key));
        }
        list.render(theme)
    }

    /// One routine row of the Functions 详细列表.
    fn routine_row(
        &self,
        routine: &RoutineInfo,
        selected: bool,
        widths: &[f32],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        let name = routine.name.clone();
        let label_name = object_display(&name);
        let hit_key = name.clone();
        let open_name = name.clone();
        let menu_name = name.clone();
        let kind = routine.kind;
        let menu_app = self.app.clone();
        ui::detail_row(
            SharedString::from(format!(
                "obj-routine-{connection_index}-{database_index}-{name}"
            )),
            selected,
            theme,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            let opens = this.click_opens(event.click_count(), event.modifiers(), cx);
            window.focus(&this.focus, cx);
            this.hit_object(&hit_key, event.modifiers(), cx);
            if opens {
                this.open_routine_named(open_name.clone(), kind, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.ensure_object_selected(&menu_name, cx);
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
                // The blank-area menu on the ObjectPane container is a fallback; a row's own
                // menu must win.
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .w(px(widths[0]))
                .flex_none()
                .overflow_hidden()
                .child(ui::leading_icon_badge(
                    routine_icon(kind),
                    routine_icon_color(kind, theme),
                    ui::DETAIL_BADGE_SIZE,
                ))
                .child(ui::detail_cell_text(
                    SharedString::from(format!(
                        "routine-cell-{connection_index}-{database_index}-{name}-0"
                    )),
                    label_name,
                )),
        )
        .child(
            div()
                .w(px(widths[1]))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(ui::detail_cell_text(
                    SharedString::from(format!(
                        "routine-cell-{connection_index}-{database_index}-{name}-1"
                    )),
                    routine.modified.clone().unwrap_or_default(),
                )),
        )
        .child(
            div()
                .w(px(widths[2]))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(ui::detail_cell_text(
                    SharedString::from(format!(
                        "routine-cell-{connection_index}-{database_index}-{name}-2"
                    )),
                    kind.sql_name(),
                )),
        )
        .child(
            div()
                .w(px(widths[3]))
                .flex_none()
                .whitespace_nowrap()
                .text_color(rgb(theme.text_muted))
                .child(if routine.deterministic {
                    t!("common.yes").to_string()
                } else {
                    t!("common.no").to_string()
                }),
        )
        .child(
            div()
                .w(px(widths[4]))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(ui::detail_cell_text(
                    SharedString::from(format!(
                        "routine-cell-{connection_index}-{database_index}-{name}-4"
                    )),
                    single_line(&routine.comment),
                )),
        )
    }

    /// The Functions 平铺网格: the column-major routine tiles.
    fn render_routine_grid(
        &self,
        routines: &[&RoutineInfo],
        selected: &std::collections::HashSet<String>,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let width = ui::grid_item_width(
            routines
                .iter()
                .map(|routine| ui::approx_text_width(&object_display(&routine.name)))
                .fold(0.0, f32::max),
        );
        let rows = self.grid.rows_per_column();
        let mut columns = ui::grid_columns();
        let mut column = ui::grid_column();
        let mut count = 0usize;
        for routine in routines {
            if count == rows {
                columns = columns.child(column);
                column = ui::grid_column();
                count = 0;
            }
            let key = routine.name.clone();
            let tile = self.routine_tile(routine, selected.contains(&key), theme, width, cx);
            column = column.child(object_row_with_rect(self.app.clone(), tile, key));
            count += 1;
        }
        if count > 0 {
            columns = columns.child(column);
        }
        self.render_grid(columns, theme, cx)
    }

    /// One routine tile of the Functions 平铺网格.
    fn routine_tile(
        &self,
        routine: &RoutineInfo,
        selected: bool,
        theme: Theme,
        width: f32,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        let name = routine.name.clone();
        let label_name = object_display(&name);
        let hit_key = name.clone();
        let open_name = name.clone();
        let menu_name = name.clone();
        let kind = routine.kind;
        let menu_app = self.app.clone();
        ui::grid_item_sized(
            SharedString::from(format!(
                "obj-routine-tile-{connection_index}-{database_index}-{name}"
            )),
            selected,
            theme,
            width,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            let opens = this.click_opens(event.click_count(), event.modifiers(), cx);
            window.focus(&this.focus, cx);
            this.hit_object(&hit_key, event.modifiers(), cx);
            if opens {
                this.open_routine_named(open_name.clone(), kind, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.ensure_object_selected(&menu_name, cx);
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
                // The blank-area menu on the ObjectPane container is a fallback; a row's own
                // menu must win.
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .child(ui::leading_icon_badge(
            routine_icon(kind),
            routine_icon_color(kind, theme),
            16.0,
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(theme.text))
                .child(label_name),
        )
    }

    /// Wrap a 平铺网格's columns in the shared horizontal scroller plus its scrollbar.
    fn render_grid(&self, columns: Div, theme: Theme, cx: &mut Context<'_, Self>) -> AnyElement {
        let mut grid = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden();
        grid = grid.child(self.grid.scroller("object-grid-scroll", columns));
        if self.grid.overflows() {
            grid = grid.child(
                self.grid
                    .scrollbar("object-grid-hscrollbar", theme)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            // The scrollbar must not also start a marquee.
                            cx.stop_propagation();
                            if this.grid.begin(event.position.x) {
                                cx.notify();
                            }
                        }),
                    ),
            );
        }
        grid.into_any_element()
    }

    /// Commit an open in-place rename (e.g. before the click moves selection elsewhere).
    fn commit_pending_rename(&self, cx: &mut Context<'_, Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        app.update(cx, |app, cx| app.submit_rename(cx));
    }

    /// Keys no command claims: typing a name jumps to the first object starting with it. The
    /// navigation keys, Enter, F2 and Ctrl+A are keymap commands (the `Object*` actions).
    fn object_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || self.renaming(cx) {
            return;
        }
        let Some(character) = keystroke
            .key_char
            .as_ref()
            .and_then(|text| text.chars().next())
            .filter(|character| !character.is_control())
        else {
            return;
        };
        // The list only reads the key to move its highlight; claiming it would stop the platform
        // from delivering the character to the in-place rename editor drawn inside this pane.
        let now = std::time::Instant::now();
        if self
            .typeahead_at
            .is_none_or(|at| now.duration_since(at) > super::sidebar::TYPEAHEAD_RESET)
        {
            self.typeahead.clear();
        }
        self.typeahead_at = Some(now);
        self.typeahead.push(character);
        let labels: Vec<String> = self
            .visible_keys
            .iter()
            .map(|key| object_display(key))
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        if let Some(target) =
            super::sidebar::typeahead_target(&labels, self.object_index(), &self.typeahead)
        {
            self.object_select(target, cx);
        }
    }

    /// Whether this pane's in-place rename editor is open (typing then belongs to it).
    fn renaming(&self, cx: &App) -> bool {
        self.app
            .upgrade()
            .is_some_and(|app| app.read(cx).rename_row(RowPane::Objects).is_some())
    }

    /// The index of the selected object in the last render's order.
    fn object_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.visible_keys.iter().position(|key| key == selected)
    }

    /// Select object `index` from the keyboard (replacing the multi-selection) and scroll it into
    /// view, from the row bounds the last frame recorded.
    fn object_select(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(key) = self.visible_keys.get(index).cloned() else {
            return;
        };
        // Move the highlight now (only this list redraws); the info pane and toolbar follow once
        // the keys settle, so holding an arrow key does not load every object it passes.
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, _| app.objects_selection.select_one(key.clone()));
        }
        self.selected = Some(key.clone());
        self.nav_generation += 1;
        let generation = self.nav_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NAV_SETTLE).await;
            let _ = this.update(cx, |pane, cx| {
                if pane.nav_generation == generation {
                    pane.sync_single_from_app(cx);
                    pane.notify_app(cx);
                }
            });
        })
        .detach();
        let bounds = self
            .app
            .upgrade()
            .and_then(|app| app.read(cx).objects_row_rects.get(&key).copied());
        if let Some(bounds) = bounds {
            match self.mode {
                ViewMode::Detail => self.detail_scroll.reveal(bounds),
                ViewMode::Grid => self.grid.reveal(bounds),
            }
        }
        cx.notify();
    }

    /// Move the selection for one navigation key, following the current layout.
    fn object_step(&mut self, step: ObjectStep, cx: &mut Context<'_, Self>) {
        let count = self.visible_keys.len();
        if count == 0 {
            return;
        }
        // The size of one item, from the last frame, for paging by a screenful.
        let item = self.visible_keys.first().and_then(|key| {
            self.app
                .upgrade()
                .and_then(|app| app.read(cx).objects_row_rects.get(key).copied())
        });
        let (per_column, page) = match self.mode {
            ViewMode::Detail => {
                let row_height = item.map_or(24.0, |bounds| f32::from(bounds.size.height));
                let rows = (self.detail_scroll.viewport_height() / row_height.max(1.0)) as usize;
                (usize::MAX, rows.saturating_sub(1).max(1))
            }
            ViewMode::Grid => {
                let per_column = self.grid.rows_per_column().max(1);
                let width = item.map_or(160.0, |bounds| f32::from(bounds.size.width));
                let columns = (self.grid.viewport_width() / width.max(1.0)) as usize;
                (per_column, per_column * columns.max(1))
            }
        };
        if let Some(target) = object_nav_target(self.object_index(), count, step, per_column, page)
        {
            self.object_select(target, cx);
        }
    }

    /// Ctrl/Cmd+A selects every visible object.
    fn object_select_all(&mut self, cx: &mut Context<'_, Self>) {
        let visible = self.visible_keys.clone();
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, _| app.objects_selection.select_all(&visible));
        }
        self.sync_single_from_app(cx);
        self.notify_app(cx);
        cx.notify();
    }

    /// F2 starts editing the selected object's name in place.
    fn object_rename(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(name) = self.selected.clone() {
            self.begin_rename(name, window, cx);
        }
    }

    /// Enter opens the selected object as a double-click does.
    fn object_open(&mut self, cx: &mut Context<'_, Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        match (self.category, self.selected_routine) {
            (Category::Functions, Some(kind)) => self.open_routine_named(name, kind, cx),
            (Category::Functions, None) => {}
            _ => self.open_object_named(name, cx),
        }
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

    /// Apply one row click to the object list's multi-selection (plain click replaces, Ctrl
    /// toggles, Shift extends from the anchor).
    fn hit_object(&mut self, key: &str, modifiers: Modifiers, cx: &mut Context<'_, Self>) {
        let visible = self.visible_keys.clone();
        let mode = selection_mode(modifiers);
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, _| {
                app.hit_object_selection(&visible, key, mode, modifiers.shift);
            });
        }
        self.sync_single_from_app(cx);
        self.notify_app(cx);
        cx.notify();
    }

    /// Replace the object selection with one row (used by the right-click handler).
    fn set_object_selection_one(&mut self, key: &str, cx: &mut Context<'_, Self>) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, _| {
                app.objects_selection.select_one(key.to_string());
            });
        }
        self.sync_single_from_app(cx);
        self.notify_app(cx);
        cx.notify();
    }

    /// Keep the right-clicked row selected: a click on a row that is already part of the
    /// multi-selection keeps it, otherwise the selection is replaced by that row.
    fn ensure_object_selected(&mut self, key: &str, cx: &mut Context<'_, Self>) {
        let contains = self
            .app
            .upgrade()
            .is_some_and(|app| app.read(cx).objects_selection.contains(key));
        if !contains {
            self.set_object_selection_one(key, cx);
        }
    }

    /// Sync the pane's single-selection mirror (`selected` / `selected_routine`) and the info pane
    /// with `AppView`'s multi-selection.
    pub(super) fn sync_single_from_app(&mut self, cx: &mut Context<'_, Self>) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let (single, kind) = {
            let app_ref = app.read(cx);
            let single = app_ref.objects_selection.single().map(str::to_string);
            let kind = if self.category == Category::Functions {
                single.as_deref().and_then(|name| {
                    let database = app_ref.connections.get(self.connection_index).and_then(
                        |node| match &node.databases {
                            Loadable::Loaded(databases) => databases.get(self.database_index),
                            _ => None,
                        },
                    )?;
                    match &database.routines {
                        Loadable::Loaded(routines) => routines
                            .iter()
                            .find(|routine| routine.name == name)
                            .map(|routine| routine.kind),
                        _ => None,
                    }
                })
            } else {
                None
            };
            (single, kind)
        };
        let changed = self.selected != single || self.selected_routine != kind;
        self.selected = single.clone();
        self.selected_routine = kind;
        if changed {
            let connection_index = self.connection_index;
            let database_index = self.database_index;
            app.update(cx, |app, _| match (single, kind) {
                (Some(name), Some(kind)) => {
                    app.set_info_routine(connection_index, database_index, name, kind);
                }
                (Some(name), None) => {
                    app.set_info_table(connection_index, database_index, name);
                }
                (None, _) => app.clear_info_selection(),
            });
        }
        cx.notify();
    }

    /// Whether this click opens the object: double-click normally, a single click with 单击模式
    /// enabled in Options → General.
    fn click_opens(
        &self,
        click_count: usize,
        modifiers: Modifiers,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        self.app
            .upgrade()
            .is_some_and(|app| app.read(cx).click_opens(click_count, modifiers))
    }

    /// Open a table or view (used by the double-click handler).
    fn open_object_named(&mut self, name: String, cx: &mut Context<'_, Self>) {
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

    /// Open a stored routine (used by the double-click handler).
    fn open_routine_named(&mut self, name: String, kind: RoutineKind, cx: &mut Context<'_, Self>) {
        let connection_index = self.connection_index;
        let database_index = self.database_index;
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, cx| {
                let Some(database) = app.database_name(connection_index, database_index) else {
                    return;
                };
                app.open_routine_by_name(connection_index, database, name, kind, cx);
            });
        }
    }
}

impl Render for ObjectPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        // Snapshot the rubber band from the rectangles published last frame, then clear them so
        // rows that disappeared stop matching the marquee.
        let marquee = self
            .app
            .upgrade()
            .and_then(|app| app.read(cx).marquee_rect_for(MarqueeTarget::Objects));
        if let Some(app) = self.app.upgrade() {
            app.update(cx, |app, _| app.clear_row_rects(MarqueeTarget::Objects));
        }

        let (search, tables, table_statuses, routines, rename, mode, selected, supports_schemas) = {
            let Some(app) = self.app.upgrade() else {
                return div().into_any_element();
            };
            let app = app.read(cx);
            // The pane is a cached child: AppView pushes the theme on a change, but reading it here
            // as well means a pane that is repainted for any other reason can never keep painting
            // with a stale palette (e.g. the washed-out list right after a light/dark switch).
            self.theme = app.theme;
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
            let table_statuses = database.and_then(|database| match &database.table_statuses {
                Loadable::Loaded(items) => Some(items.clone()),
                _ => None,
            });
            let routines = database.map(|database| match &database.routines {
                Loadable::Idle => Loadable::Idle,
                Loadable::Loading => Loadable::Loading,
                Loadable::Failed(error) => Loadable::Failed(error.clone()),
                Loadable::Loaded(items) => Loadable::Loaded(items.clone()),
            });
            // A key list is always the detailed key table (键名 / 类型 / TTL / 内存); the SQL
            // tile grid is not one of its layouts.
            let mode = if self.key_value && self.category == Category::Tables {
                ViewMode::Detail
            } else {
                app.view_mode_or(view_page_for_category(self.category), ViewMode::default())
            };
            let selected: std::collections::HashSet<String> =
                app.objects_selection.items().into_iter().collect();
            let supports_schemas =
                app.driver_supports(self.connection_index, DriverCapability::Schemas);
            (
                // The pane filters by its own category's search, not by whatever another list's
                // search box holds.
                app.object_search_text(view_page_for_category(self.category)),
                tables,
                table_statuses,
                routines,
                app.rename_row(RowPane::Objects),
                mode,
                selected,
                supports_schemas,
            )
        };

        self.mode = mode;
        let body = self.render_body(
            ObjectRenderData {
                mode,
                schema: self.schema.clone(),
                supports_schemas,
                tables,
                table_statuses,
                routines,
                search,
                selected,
                rename,
            },
            cx,
        );

        let mut container = div()
            .id("object-list")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .key_context(OBJECT_LIST_CONTEXT)
            .track_focus(&self.focus)
            // Keyboard navigation: keymap commands, rebindable from Options → Shortcuts.
            .on_action(
                cx.listener(|this, _: &ObjectUp, _window, cx| this.object_step(ObjectStep::Up, cx)),
            )
            .on_action(cx.listener(|this, _: &ObjectDown, _window, cx| {
                this.object_step(ObjectStep::Down, cx)
            }))
            .on_action(cx.listener(|this, _: &ObjectLeft, _window, cx| {
                this.object_step(ObjectStep::Left, cx)
            }))
            .on_action(cx.listener(|this, _: &ObjectRight, _window, cx| {
                this.object_step(ObjectStep::Right, cx)
            }))
            .on_action(cx.listener(|this, _: &ObjectPageUp, _window, cx| {
                this.object_step(ObjectStep::PageUp, cx)
            }))
            .on_action(cx.listener(|this, _: &ObjectPageDown, _window, cx| {
                this.object_step(ObjectStep::PageDown, cx)
            }))
            .on_action(cx.listener(|this, _: &ObjectHome, _window, cx| {
                this.object_step(ObjectStep::Home, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ObjectEnd, _window, cx| {
                    this.object_step(ObjectStep::End, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ObjectOpen, _window, cx| this.object_open(cx)))
            .on_action(
                cx.listener(|this, _: &ObjectRename, window, cx| this.object_rename(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ObjectSelectAll, _window, cx| this.object_select_all(cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.object_key(event, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    // Clicking outside the in-place rename commits it (the editor swallows clicks
                    // on itself, so this only sees the row/background), then the list takes focus
                    // so F2 works.
                    this.commit_pending_rename(cx);
                    window.focus(&this.focus, cx);
                    if let Some(app) = this.app.upgrade() {
                        app.update(cx, |app, cx| {
                            app.begin_marquee(
                                MarqueeTarget::Objects,
                                event.position,
                                event.modifiers,
                                cx,
                            );
                        });
                    }
                }),
            )
            // Right-clicking the blank area of the object list offers the same category menu as
            // the connection tree's category node: new table / import / export / refresh for
            // 表, or refresh for the other categories.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.commit_pending_rename(cx);
                    window.focus(&this.focus, cx);
                    let target = match this.category {
                        Category::Tables => ContextTarget::TableCategory {
                            connection_index: this.connection_index,
                            database_index: this.database_index,
                            schema: this.schema.clone(),
                        },
                        category => ContextTarget::ObjectCategory {
                            connection_index: this.connection_index,
                            database_index: this.database_index,
                            category,
                        },
                    };
                    if let Some(app) = this.app.upgrade() {
                        app.update(cx, |app, cx| {
                            app.context_menu = Some(ContextMenu {
                                target,
                                position: event.position,
                            });
                            cx.notify();
                        });
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let resized = match this.category {
                    Category::Functions => this.routine_columns.borrow_mut().drag_resize(event),
                    _ => this.table_columns.borrow_mut().drag_resize(event),
                };
                if resized {
                    cx.notify();
                }
                if this.grid.drag(event) {
                    cx.notify();
                }
                if let Some(app) = this.app.upgrade() {
                    app.update(cx, |app, cx| {
                        app.drag_marquee(MarqueeTarget::Objects, event, cx);
                    });
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    let table = this.table_columns.borrow_mut().end_resize();
                    let routine = this.routine_columns.borrow_mut().end_resize();
                    if table || routine {
                        cx.notify();
                    }
                    if this.grid.end() {
                        cx.notify();
                    }
                    if let Some(app) = this.app.upgrade() {
                        app.update(cx, |app, cx| app.end_marquee(cx));
                    }
                }),
            )
            .child(body);
        if let Some(rect) = marquee {
            container = container.child(rect);
        }
        container.into_any_element()
    }
}

#[cfg(test)]
mod nav_tests {
    use super::{ObjectStep, key_group, object_display, object_nav_target};

    const DETAIL: usize = usize::MAX;

    #[test]
    fn a_key_belongs_to_the_namespace_before_its_first_colon() {
        assert_eq!(key_group("sys_dict:sys_yes_no"), "sys_dict:");
        assert_eq!(key_group("login_tokens:abc:def"), "login_tokens:");
        assert_eq!(key_group("standing_key"), "");
    }

    #[test]
    fn a_rename_edits_the_bare_name_of_a_schema_qualified_object() {
        // The in-place editor and the list must agree: no internal `public.` prefix leaks in.
        assert_eq!(
            object_display("public.qrtz_cron_triggers"),
            "qrtz_cron_triggers"
        );
        assert_eq!(object_display("dbo.users"), "users");
        assert_eq!(object_display("users"), "users");
    }

    #[test]
    fn detail_list_moves_by_row_and_page_and_ignores_left_right() {
        assert_eq!(
            object_nav_target(None, 10, ObjectStep::Down, DETAIL, 4),
            Some(0)
        );
        assert_eq!(
            object_nav_target(Some(3), 10, ObjectStep::Down, DETAIL, 4),
            Some(4)
        );
        assert_eq!(
            object_nav_target(Some(9), 10, ObjectStep::Down, DETAIL, 4),
            None
        );
        assert_eq!(
            object_nav_target(Some(0), 10, ObjectStep::Up, DETAIL, 4),
            None
        );
        assert_eq!(
            object_nav_target(Some(3), 10, ObjectStep::PageDown, DETAIL, 4),
            Some(7)
        );
        assert_eq!(
            object_nav_target(Some(8), 10, ObjectStep::PageDown, DETAIL, 4),
            Some(9)
        );
        assert_eq!(
            object_nav_target(Some(2), 10, ObjectStep::PageUp, DETAIL, 4),
            Some(0)
        );
        assert_eq!(
            object_nav_target(Some(5), 10, ObjectStep::Left, DETAIL, 4),
            None
        );
        assert_eq!(
            object_nav_target(Some(5), 10, ObjectStep::Home, DETAIL, 4),
            Some(0)
        );
        assert_eq!(
            object_nav_target(Some(5), 10, ObjectStep::End, DETAIL, 4),
            Some(9)
        );
    }

    #[test]
    fn grid_left_right_move_one_column() {
        // 10 items, 4 per column: columns [0..4) [4..8) [8..10).
        assert_eq!(
            object_nav_target(Some(1), 10, ObjectStep::Right, 4, 8),
            Some(5)
        );
        assert_eq!(
            object_nav_target(Some(5), 10, ObjectStep::Right, 4, 8),
            Some(9)
        );
        assert_eq!(
            object_nav_target(Some(7), 10, ObjectStep::Right, 4, 8),
            Some(9)
        );
        assert_eq!(
            object_nav_target(Some(9), 10, ObjectStep::Right, 4, 8),
            None
        );
        assert_eq!(
            object_nav_target(Some(5), 10, ObjectStep::Left, 4, 8),
            Some(1)
        );
        assert_eq!(object_nav_target(Some(2), 10, ObjectStep::Left, 4, 8), None);
        // Up / Down run through the columns in order.
        assert_eq!(
            object_nav_target(Some(3), 10, ObjectStep::Down, 4, 8),
            Some(4)
        );
    }

    #[test]
    fn an_empty_list_goes_nowhere() {
        assert_eq!(
            object_nav_target(None, 0, ObjectStep::Down, DETAIL, 4),
            None
        );
    }
}
