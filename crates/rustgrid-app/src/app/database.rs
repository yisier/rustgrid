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
            database.routines = Loadable::Idle;
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
        self.db_dialog = Some(DbDialog::Edit(DatabaseForm {
            connection_index,
            database_index: None,
            name: String::new(),
            original_charset: String::new(),
            original_collation: String::new(),
            charset: String::new(),
            collation: String::new(),
            charsets: Vec::new(),
            collations: Vec::new(),
            tab: DbTab::General,
            loading: true,
            error: None,
        }));
        self.ensure_db_combos(cx);
        window.focus(&focus, cx);
        self.load_db_form(connection_index, None, cx);
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
        self.db_dialog = Some(DbDialog::Edit(DatabaseForm {
            connection_index,
            database_index: Some(database_index),
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
        }));
        self.ensure_db_combos(cx);
        self.load_db_form(connection_index, Some(name), cx);
        cx.notify();
    }

    /// Load the charset/collation catalogue (and, when `defaults_for` is set, the current defaults
    /// of that existing database) into the already-open database dialog. Shared by the new- and
    /// edit-database paths.
    fn load_db_form(
        &mut self,
        connection_index: usize,
        defaults_for: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            if let Some(DbDialog::Edit(form)) = self.db_dialog.as_mut() {
                form.loading = false;
            }
            cx.notify();
            return;
        };
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let defaults = match defaults_for {
                Some(name) => {
                    let connection = connection.clone();
                    match runtime
                        .spawn(async move { connection.database_defaults(&name).await })
                        .await
                    {
                        Ok(inner) => inner.map(Some),
                        Err(error) => Err(Error::other(error)),
                    }
                }
                None => Ok(None),
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
                if let Some(DbDialog::Edit(form)) = view.db_dialog.as_mut() {
                    form.loading = false;
                    match defaults {
                        Ok(Some((charset, collation))) => {
                            form.charset = charset.clone();
                            form.collation = collation.clone();
                            form.original_charset = charset;
                            form.original_collation = collation;
                        }
                        Ok(None) => {}
                        Err(error) => form.error = Some(error.to_string()),
                    }
                    match charsets {
                        Ok(values) => form.charsets = values,
                        Err(error) => form.error = Some(error.to_string()),
                    }
                    match collations {
                        Ok(values) => form.collations = values,
                        Err(error) => form.error = Some(error.to_string()),
                    }
                }
                cx.notify();
            });
        })
        .detach();
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
            DbDialog::Edit(mut form) => {
                if form.database_index.is_none() {
                    let name = form.name.trim().to_string();
                    if !is_valid_identifier(&name) {
                        form.name = name;
                        form.error = Some(t!("database.invalid_name").to_string());
                        self.db_dialog = Some(DbDialog::Edit(form));
                        cx.notify();
                        return;
                    }

                    let charset = form.charset.trim().to_string();
                    let collation = form.collation.trim().to_string();
                    let charset_for_call = (!charset.is_empty()).then_some(charset);
                    let collation_for_call = (!collation.is_empty()).then_some(collation);

                    let Some(connection) = self.connection_arc(form.connection_index) else {
                        return;
                    };
                    let runtime = self.runtime.clone();
                    cx.spawn(async move |this, cx| {
                        let result = match runtime
                            .spawn(async move {
                                connection
                                    .create_database(
                                        &name,
                                        charset_for_call.as_deref(),
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
                            match result {
                                Ok(()) => {
                                    view.db_name_input = None;
                                    view.load_databases(form.connection_index, cx);
                                }
                                Err(error) => {
                                    form.loading = false;
                                    form.error = Some(error.to_string());
                                    view.db_dialog = Some(DbDialog::Edit(form));
                                }
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                } else {
                    let charset = form.charset.trim().to_string();
                    let collation = form.collation.trim().to_string();
                    let valid = (charset.is_empty() || is_valid_identifier(&charset))
                        && (collation.is_empty() || is_valid_identifier(&collation));
                    if !valid {
                        form.error = Some(t!("database.invalid_name").to_string());
                        self.db_dialog = Some(DbDialog::Edit(form));
                        cx.notify();
                        return;
                    }

                    let charset_changed = charset != form.original_charset;
                    let collation_changed = collation != form.original_collation;
                    if !charset_changed && !collation_changed {
                        cx.notify();
                        return;
                    }

                    let Some(connection) = self.connection_arc(form.connection_index) else {
                        return;
                    };
                    let runtime = self.runtime.clone();
                    let charset_for_call = if charset_changed && !charset.is_empty() {
                        Some(charset)
                    } else {
                        None
                    };
                    let collation_for_call = if collation_changed && !collation.is_empty() {
                        Some(collation)
                    } else {
                        None
                    };
                    let edit_name = form.name.clone();
                    cx.spawn(async move |this, cx| {
                        let result = match runtime
                            .spawn(async move {
                                connection
                                    .alter_database_defaults(
                                        &edit_name,
                                        charset_for_call.as_deref(),
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
                                form.loading = false;
                                form.error = Some(error.to_string());
                                view.db_dialog = Some(DbDialog::Edit(form));
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                }
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
                ComboBox::new(theme, Vec::new(), String::new(), 300.0, cx)
                    .field_width(300.0)
                    .on_select(Rc::new(move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.db_charset_selected(value, cx));
                    }))
            });
            self.db_charset_combo = Some(combo);
        }
        if self.db_collation_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 300.0, cx)
                    .field_width(300.0)
                    .on_select(Rc::new(move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.db_collation_selected(value, cx));
                    }))
            });
            self.db_collation_combo = Some(combo);
        }
    }

    /// Push the dialog's charsets/collations into the two combo entities. Called every frame (the
    /// setters are idempotent) so the list always reflects the loaded schema.
    pub(super) fn sync_db_combos(&mut self, cx: &mut Context<'_, Self>) {
        let Some(DbDialog::Edit(form)) = self.db_dialog.as_ref() else {
            return;
        };
        if form.loading {
            return;
        }
        let charset = form.charset.clone();
        let collation = form.collation.clone();
        let charset_options: Vec<ComboOption> = form
            .charsets
            .iter()
            .cloned()
            .map(ComboOption::plain)
            .collect();
        let prefix = format!("{charset}_");
        let collation_options: Vec<ComboOption> = if charset.is_empty() {
            form.collations
                .iter()
                .cloned()
                .map(ComboOption::plain)
                .collect()
        } else {
            form.collations
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
        if let Some(DbDialog::Edit(form)) = self.db_dialog.as_mut() {
            form.charset = value.to_string();
            let prefix = format!("{value}_");
            if let Some(first) = form
                .collations
                .iter()
                .find(|candidate| candidate.starts_with(&prefix))
                .cloned()
            {
                form.collation = first;
            } else {
                form.collation.clear();
            }
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    pub(super) fn db_collation_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(DbDialog::Edit(form)) = self.db_dialog.as_mut() {
            form.collation = value.to_string();
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    pub(super) fn db_tab_button(&self, tab: DbTab, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let active = matches!(
            self.db_dialog,
            Some(DbDialog::Edit(ref form)) if form.tab == tab
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
        if let Some(DbDialog::Edit(form)) = self.db_dialog.as_mut() {
            form.tab = tab;
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    /// The SQL the dialog would run: `CREATE DATABASE ...` while creating, `ALTER DATABASE ...`
    /// while editing. `None` when there is nothing to preview.
    pub(super) fn db_sql_preview(&self) -> Option<String> {
        let Some(DbDialog::Edit(form)) = self.db_dialog.as_ref() else {
            return None;
        };
        if form.loading {
            return None;
        }
        let connection = self.connection_arc(form.connection_index)?;

        if form.database_index.is_none() {
            let name = form.name.trim();
            if name.is_empty() {
                return None;
            }
            let charset = form.charset.trim();
            let collation = form.collation.trim();
            let charset = (!charset.is_empty()).then_some(charset);
            let collation = (!collation.is_empty()).then_some(collation);
            return Some(connection.create_database_sql(name, charset, collation));
        }

        let charset_changed = form.charset != form.original_charset;
        let collation_changed = form.collation != form.original_collation;
        if !charset_changed && !collation_changed {
            return None;
        }

        let charset = if charset_changed && !form.charset.is_empty() {
            Some(form.charset.as_str())
        } else {
            None
        };
        let collation = if collation_changed && !form.collation.is_empty() {
            Some(form.collation.as_str())
        } else {
            None
        };
        Some(connection.alter_database_sql(&form.name, charset, collation))
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
