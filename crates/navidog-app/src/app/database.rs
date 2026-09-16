use super::*;

impl AppView {
    pub(super) fn close_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.opened = false;
            database.expanded = false;
            database.tables = Loadable::Idle;
            database.categories = CategoryExpansion::default();
        }
        self.completion_generation = self.completion_generation.wrapping_add(1);
        if let Some(pane) = self.object_pane.as_ref() {
            let matches = {
                let pane = pane.read(cx);
                pane.connection_index == connection_index && pane.database_index == database_index
            };
            if matches {
                self.object_pane = None;
            }
        }
        cx.notify();
    }

    pub(super) fn open_new_database(
        &mut self,
        connection_index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let weak = cx.weak_entity();
        let input = make_db_name_input(self.theme, &weak, cx);
        let focus = input.read(cx).focus_handle();
        self.db_name_input = Some(input);
        self.db_dialog = Some(DbDialog::New {
            connection_index,
            name: String::new(),
            error: None,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn open_edit_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(name) = self.database_name(connection_index, database_index) else {
            return;
        };
        self.db_dialog = Some(DbDialog::Edit {
            connection_index,
            database_index,
            name: name.clone(),
            original_charset: String::new(),
            original_collation: String::new(),
            charset: String::new(),
            collation: String::new(),
            charsets: Vec::new(),
            collations: Vec::new(),
            tab: DbTab::General,
            loading: true,
            error: None,
        });
        self.ensure_db_combos(cx);

        let Some(connection) = self.connection_arc(connection_index) else {
            cx.notify();
            return;
        };
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let defaults = match runtime
                .spawn({
                    let connection = connection.clone();
                    let name = name.clone();
                    async move { connection.database_defaults(&name).await }
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let charsets = match runtime
                .spawn({
                    let connection = connection.clone();
                    async move { connection.character_sets().await }
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let collations = match runtime
                .spawn(async move { connection.collations().await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(DbDialog::Edit {
                    original_charset,
                    original_collation,
                    charset,
                    collation,
                    charsets: charset_options,
                    collations: collation_options,
                    loading,
                    error,
                    ..
                }) = view.db_dialog.as_mut()
                {
                    *loading = false;
                    match defaults {
                        Ok((cs, col)) => {
                            *charset = cs.clone();
                            *collation = col.clone();
                            *original_charset = cs;
                            *original_collation = col;
                        }
                        Err(err) => *error = Some(err.to_string()),
                    }
                    match charsets {
                        Ok(values) => *charset_options = values,
                        Err(err) => *error = Some(err.to_string()),
                    }
                    match collations {
                        Ok(values) => *collation_options = values,
                        Err(err) => *error = Some(err.to_string()),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn open_delete_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(name) = self.database_name(connection_index, database_index) else {
            return;
        };
        self.db_dialog = Some(DbDialog::Delete {
            connection_index,
            database_index,
            name,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn db_submit(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.db_dialog.take() else {
            return;
        };

        match dialog {
            DbDialog::New {
                connection_index,
                name,
                ..
            } => {
                let name = name.trim().to_string();
                if !is_valid_identifier(&name) {
                    self.db_dialog = Some(DbDialog::New {
                        connection_index,
                        name,
                        error: Some(t!("database.invalid_name").to_string()),
                    });
                    cx.notify();
                    return;
                }

                let Some(connection) = self.connection_arc(connection_index) else {
                    return;
                };
                let runtime = self.runtime.clone();
                let call_name = name.clone();
                cx.spawn(async move |this, cx| {
                    let result = match runtime
                        .spawn(async move { connection.create_database(&call_name).await })
                        .await
                    {
                        Ok(inner) => inner,
                        Err(error) => Err(Error::other(error)),
                    };

                    let _ = this.update(cx, |view, cx| {
                        match result {
                            Ok(()) => view.load_databases(connection_index, cx),
                            Err(error) => {
                                view.db_dialog = Some(DbDialog::New {
                                    connection_index,
                                    name,
                                    error: Some(error.to_string()),
                                });
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            DbDialog::Edit {
                connection_index,
                database_index,
                name,
                original_charset,
                original_collation,
                charset,
                collation,
                charsets,
                collations,
                tab,
                ..
            } => {
                let charset = charset.trim().to_string();
                let collation = collation.trim().to_string();
                let valid = (charset.is_empty() || is_valid_identifier(&charset))
                    && (collation.is_empty() || is_valid_identifier(&collation));
                if !valid {
                    self.db_dialog = Some(DbDialog::Edit {
                        connection_index,
                        database_index,
                        name,
                        original_charset,
                        original_collation,
                        charset,
                        collation,
                        charsets,
                        collations,
                        tab,
                        loading: false,
                        error: Some(t!("database.invalid_name").to_string()),
                    });
                    cx.notify();
                    return;
                }

                let charset_changed = charset != original_charset;
                let collation_changed = collation != original_collation;
                if !charset_changed && !collation_changed {
                    self.db_dialog = None;
                    cx.notify();
                    return;
                }

                let Some(connection) = self.connection_arc(connection_index) else {
                    return;
                };
                let runtime = self.runtime.clone();
                let for_call = if charset_changed && !charset.is_empty() {
                    Some(charset.clone())
                } else {
                    None
                };
                let collation_for_call = if collation_changed && !collation.is_empty() {
                    Some(collation.clone())
                } else {
                    None
                };
                let edit_name = name.clone();
                cx.spawn(async move |this, cx| {
                    let result = match runtime
                        .spawn(async move {
                            connection
                                .alter_database_defaults(
                                    &edit_name,
                                    for_call.as_deref(),
                                    collation_for_call.as_deref(),
                                )
                                .await
                        })
                        .await
                    {
                        Ok(inner) => inner,
                        Err(error) => Err(Error::other(error)),
                    };

                    let _ = this.update(cx, |view, cx| {
                        if let Err(error) = result {
                            view.db_dialog = Some(DbDialog::Edit {
                                connection_index,
                                database_index,
                                name,
                                original_charset,
                                original_collation,
                                charset,
                                collation,
                                charsets,
                                collations,
                                tab,
                                loading: false,
                                error: Some(error.to_string()),
                            });
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            DbDialog::Delete {
                connection_index,
                database_index,
                name,
                ..
            } => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    return;
                };
                let runtime = self.runtime.clone();
                let drop_name = name.clone();
                cx.spawn(async move |this, cx| {
                    let result = match runtime
                        .spawn(async move { connection.drop_database(&drop_name).await })
                        .await
                    {
                        Ok(inner) => inner,
                        Err(error) => Err(Error::other(error)),
                    };

                    let _ = this.update(cx, |view, cx| {
                        match result {
                            Ok(()) => view.load_databases(connection_index, cx),
                            Err(error) => {
                                view.db_dialog = Some(DbDialog::Delete {
                                    connection_index,
                                    database_index,
                                    name,
                                    error: Some(error.to_string()),
                                });
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    /// Create (once) the two editable drop-downs of the edit-database dialog. Their options and
    /// value are pushed in by [`AppView::sync_db_combos`]; selection flows back through the
    /// callbacks.
    pub(super) fn ensure_db_combos(&mut self, cx: &mut Context<'_, Self>) {
        let theme = self.theme;
        if self.db_charset_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 300.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.db_charset_selected(value, cx));
                    },
                ))
            });
            self.db_charset_combo = Some(combo);
        }
        if self.db_collation_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 300.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.db_collation_selected(value, cx));
                    },
                ))
            });
            self.db_collation_combo = Some(combo);
        }
    }

    /// Push the dialog's charsets/collations into the two combo entities. Called every frame (the
    /// setters are idempotent) so the list always reflects the loaded schema.
    pub(super) fn sync_db_combos(&mut self, cx: &mut Context<'_, Self>) {
        let Some(DbDialog::Edit {
            charset,
            collation,
            charsets,
            collations,
            loading,
            ..
        }) = self.db_dialog.as_ref()
        else {
            return;
        };
        if *loading {
            return;
        }
        let charset = charset.clone();
        let collation = collation.clone();
        let charset_options: Vec<ComboOption> =
            charsets.iter().cloned().map(ComboOption::plain).collect();
        let prefix = format!("{charset}_");
        let collation_options: Vec<ComboOption> = if charset.is_empty() {
            collations.iter().cloned().map(ComboOption::plain).collect()
        } else {
            collations
                .iter()
                .filter(|candidate| candidate.starts_with(&prefix))
                .cloned()
                .map(ComboOption::plain)
                .collect()
        };
        if let Some(combo) = self.db_charset_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(charset_options, cx);
                combo.set_selected(charset, cx);
            });
        }
        if let Some(combo) = self.db_collation_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_options(collation_options, cx);
                combo.set_selected(collation, cx);
            });
        }
    }

    pub(super) fn db_charset_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(DbDialog::Edit {
            charset,
            collation,
            collations,
            ..
        }) = self.db_dialog.as_mut()
        {
            *charset = value.to_string();
            let prefix = format!("{value}_");
            if let Some(first) = collations
                .iter()
                .find(|candidate| candidate.starts_with(&prefix))
                .cloned()
            {
                *collation = first;
            } else {
                collation.clear();
            }
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    pub(super) fn db_collation_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(DbDialog::Edit { collation, .. }) = self.db_dialog.as_mut() {
            *collation = value.to_string();
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    pub(super) fn db_tab_button(&self, tab: DbTab, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let active = matches!(
            self.db_dialog,
            Some(DbDialog::Edit { tab: current, .. }) if current == tab
        );
        let label = match tab {
            DbTab::General => t!("database.tab.general").to_string(),
            DbTab::Sql => t!("database.tab.sql").to_string(),
        };
        let id = match tab {
            DbTab::General => "db-tab-general",
            DbTab::Sql => "db-tab-sql",
        };

        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .px_4()
            .h(px(24.0))
            .text_size(px(12.0))
            .cursor_pointer()
            .when(active, move |style| {
                style
                    .bg(rgb(theme.dialog_bg))
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text))
                    .font_weight(FontWeight::SEMIBOLD)
                    .mb(px(-1.0))
            })
            .when(!active, move |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.text_color(rgb(theme.text)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| this.db_select_tab(tab, cx)))
            .child(label)
    }

    pub(super) fn db_select_tab(&mut self, tab: DbTab, cx: &mut Context<'_, Self>) {
        if let Some(DbDialog::Edit { tab: current, .. }) = self.db_dialog.as_mut() {
            *current = tab;
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    pub(super) fn db_alter_preview(&self) -> Option<String> {
        let Some(DbDialog::Edit {
            connection_index,
            name,
            original_charset,
            original_collation,
            charset,
            collation,
            loading,
            ..
        }) = self.db_dialog.as_ref()
        else {
            return None;
        };
        if *loading {
            return None;
        }

        let charset_changed = charset != original_charset;
        let collation_changed = collation != original_collation;
        if !charset_changed && !collation_changed {
            return None;
        }

        let connection = self.connection_arc(*connection_index)?;
        let charset = if charset_changed && !charset.is_empty() {
            Some(charset.as_str())
        } else {
            None
        };
        let collation = if collation_changed && !collation.is_empty() {
            Some(collation.as_str())
        } else {
            None
        };
        Some(connection.alter_database_sql(name, charset, collation))
    }

    pub(super) fn db_sql_selection_range(&self) -> (usize, usize) {
        (
            self.db_sql_anchor.min(self.db_sql_cursor),
            self.db_sql_anchor.max(self.db_sql_cursor),
        )
    }

    pub(super) fn db_sql_index_for_position(&self, position: Point<Pixels>) -> usize {
        let layout = self.db_sql_layout.borrow();
        match layout.index_for_position(position) {
            Ok(index) | Err(index) => index,
        }
    }

    pub(super) fn db_sql_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let keystroke = &event.keystroke;
        if !(keystroke.modifiers.control || keystroke.modifiers.platform) {
            return;
        }

        match keystroke.key.as_str() {
            "a" => {
                let len = self.db_sql_text.borrow().len();
                self.db_sql_anchor = 0;
                self.db_sql_cursor = len;
                cx.notify();
            }
            "c" => {
                let (start, end) = self.db_sql_selection_range();
                if start < end {
                    let selected = self
                        .db_sql_text
                        .borrow()
                        .get(start..end)
                        .unwrap_or_default()
                        .to_string();
                    cx.write_to_clipboard(ClipboardItem::new_string(selected));
                }
            }
            _ => {}
        }
    }
}
