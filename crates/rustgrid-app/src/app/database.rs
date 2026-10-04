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
            database.schemas = None;
        }
        self.completion_generation = self.completion_generation.wrapping_add(1);
        self.refresh_completion_catalog();
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
        let spec = self.database_editor_spec(connection_index);
        let recovery_model = if spec.recovery_model {
            "SIMPLE".to_string()
        } else {
            String::new()
        };
        let compatibility_level = self
            .database_compatibility_levels(connection_index)
            .first()
            .map(|level| level.to_string())
            .unwrap_or_default();
        self.db_dialog = Some(DbDialog::Edit(Box::new(DatabaseForm {
            connection_index,
            database_index: None,
            name: String::new(),
            original_charset: String::new(),
            original_collation: String::new(),
            original_owner: String::new(),
            original_recovery_model: String::new(),
            original_compatibility_level: String::new(),
            charset: String::new(),
            collation: String::new(),
            owner: String::new(),
            recovery_model,
            compatibility_level,
            charsets: Vec::new(),
            collations: Vec::new(),
            owners: Vec::new(),
            spec,
            tab: DbTab::General,
            loading: true,
            error: None,
        })));
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
        let spec = self.database_editor_spec(connection_index);
        self.db_dialog = Some(DbDialog::Edit(Box::new(DatabaseForm {
            connection_index,
            database_index: Some(database_index),
            name: name.clone(),
            original_charset: String::new(),
            original_collation: String::new(),
            original_owner: String::new(),
            original_recovery_model: String::new(),
            original_compatibility_level: String::new(),
            charset: String::new(),
            collation: String::new(),
            owner: String::new(),
            recovery_model: String::new(),
            compatibility_level: String::new(),
            charsets: Vec::new(),
            collations: Vec::new(),
            owners: Vec::new(),
            spec,
            tab: DbTab::General,
            loading: true,
            error: None,
        })));
        self.ensure_db_combos(cx);
        self.load_db_form(connection_index, Some(name), cx);
        cx.notify();
    }

    /// Load the option catalogues (and, when `options_for` is set, the current options of that
    /// existing database) into the already-open database dialog. Shared by the new- and
    /// edit-database paths.
    fn load_db_form(
        &mut self,
        connection_index: usize,
        options_for: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            if let Some(DbDialog::Edit(form)) = self.db_dialog.as_mut() {
                form.loading = false;
            }
            cx.notify();
            return;
        };
        let spec = self.database_editor_spec(connection_index);
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let options = match options_for {
                Some(name) => {
                    let connection = connection.clone();
                    match runtime
                        .spawn(async move { connection.database_options(&name).await })
                        .await
                    {
                        Ok(inner) => inner.map(Some),
                        Err(error) => Err(Error::other(error)),
                    }
                }
                None => Ok(None),
            };
            // Only fetch the catalogues the engine's dialog actually shows.
            let charsets = if spec.charset {
                match runtime
                    .spawn({
                        let connection = connection.clone();
                        async move { connection.character_sets().await }
                    })
                    .await
                {
                    Ok(inner) => inner,
                    Err(error) => Err(Error::other(error)),
                }
            } else {
                Ok(Vec::new())
            };
            let collations = if spec.collation {
                match runtime
                    .spawn({
                        let connection = connection.clone();
                        async move { connection.collations().await }
                    })
                    .await
                {
                    Ok(inner) => inner,
                    Err(error) => Err(Error::other(error)),
                }
            } else {
                Ok(Vec::new())
            };
            let owners = if spec.owner {
                match runtime
                    .spawn(async move { connection.database_owners().await })
                    .await
                {
                    Ok(inner) => inner,
                    Err(error) => Err(Error::other(error)),
                }
            } else {
                Ok(Vec::new())
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(DbDialog::Edit(form)) = view.db_dialog.as_mut() {
                    form.loading = false;
                    match options {
                        Ok(Some(options)) => {
                            form.charset = options.charset.clone();
                            form.collation = options.collation.clone();
                            form.owner = options.owner.clone();
                            form.recovery_model = options.recovery_model.clone();
                            form.compatibility_level = options.compatibility_level.clone();
                            form.original_charset = options.charset;
                            form.original_collation = options.collation;
                            form.original_owner = options.owner;
                            form.original_recovery_model = options.recovery_model;
                            form.original_compatibility_level = options.compatibility_level;
                            // Creating a new database: default the owner to the current login.
                            if form.database_index.is_none() && form.owner.is_empty() {
                                form.owner = form.owners.first().cloned().unwrap_or_default();
                            }
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
                    match owners {
                        Ok(values) => {
                            form.owners = values;
                            if form.database_index.is_none() && form.owner.is_empty() {
                                form.owner = form.owners.first().cloned().unwrap_or_default();
                            }
                        }
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

                    let options = form.options();
                    let Some(connection) = self.connection_arc(form.connection_index) else {
                        return;
                    };
                    let runtime = self.runtime.clone();
                    cx.spawn(async move |this, cx| {
                        let result = match runtime
                            .spawn(async move { connection.create_database(&name, &options).await })
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
                    let modified = form.options();
                    let original = form.original_options();
                    if modified == original {
                        cx.notify();
                        return;
                    }

                    let Some(connection) = self.connection_arc(form.connection_index) else {
                        return;
                    };
                    let runtime = self.runtime.clone();
                    let edit_name = form.name.clone();
                    cx.spawn(async move |this, cx| {
                        let result = match runtime
                            .spawn(async move {
                                connection
                                    .alter_database_options(&edit_name, &original, &modified)
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
        let spec = self
            .db_dialog
            .as_ref()
            .and_then(|dialog| match dialog {
                DbDialog::Edit(form) => Some(form.spec.clone()),
                _ => None,
            })
            .unwrap_or_default();
        if spec.owner && self.db_owner_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 300.0, cx)
                    .field_width(300.0)
                    .on_select(Rc::new(move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.db_owner_selected(value, cx));
                    }))
            });
            self.db_owner_combo = Some(combo);
        }
        if spec.recovery_model && self.db_recovery_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 300.0, cx)
                    .field_width(300.0)
                    .on_select(Rc::new(move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.db_recovery_selected(value, cx));
                    }))
            });
            self.db_recovery_combo = Some(combo);
        }
        if spec.compatibility_level && self.db_compat_combo.is_none() {
            let weak = cx.weak_entity();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, Vec::new(), String::new(), 300.0, cx)
                    .field_width(300.0)
                    .on_select(Rc::new(move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| app.db_compat_selected(value, cx));
                    }))
            });
            self.db_compat_combo = Some(combo);
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

    /// Push the SQL Server owner/recovery/compatibility values into their combos.
    pub(super) fn sync_db_option_combos(&mut self, cx: &mut Context<'_, Self>) {
        let Some(DbDialog::Edit(form)) = self.db_dialog.as_ref() else {
            return;
        };
        if form.loading {
            return;
        }
        if form.spec.owner
            && let Some(combo) = self.db_owner_combo.clone()
        {
            let options: Vec<ComboOption> = form
                .owners
                .iter()
                .cloned()
                .map(ComboOption::plain)
                .collect();
            let selected = form.owner.clone();
            combo.update(cx, |combo, cx| {
                combo.set_options(options, cx);
                combo.set_selected(selected, cx);
            });
        }
        if form.spec.recovery_model
            && let Some(combo) = self.db_recovery_combo.clone()
        {
            let options: Vec<ComboOption> = self
                .database_recovery_models(form.connection_index)
                .into_iter()
                .map(|value| ComboOption::plain(value.to_string()))
                .collect();
            let selected = form.recovery_model.clone();
            combo.update(cx, |combo, cx| {
                combo.set_options(options, cx);
                combo.set_selected(selected, cx);
            });
        }
        if form.spec.compatibility_level
            && let Some(combo) = self.db_compat_combo.clone()
        {
            let options: Vec<ComboOption> = self
                .database_compatibility_levels(form.connection_index)
                .into_iter()
                .map(|value| ComboOption::plain(value.to_string()))
                .collect();
            let selected = form.compatibility_level.clone();
            combo.update(cx, |combo, cx| {
                combo.set_options(options, cx);
                combo.set_selected(selected, cx);
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

    pub(super) fn db_owner_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(DbDialog::Edit(form)) = self.db_dialog.as_mut() {
            form.owner = value.to_string();
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    pub(super) fn db_recovery_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(DbDialog::Edit(form)) = self.db_dialog.as_mut() {
            form.recovery_model = value.to_string();
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    pub(super) fn db_compat_selected(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(DbDialog::Edit(form)) = self.db_dialog.as_mut() {
            form.compatibility_level = value.to_string();
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    /// The visible sub-tabs of the database dialog: 常规, any engine-declared informational tabs,
    /// then SQL 预览.
    pub(super) fn db_tabs(&self) -> Vec<DbTab> {
        let mut tabs = vec![DbTab::General];
        if let Some(DbDialog::Edit(form)) = self.db_dialog.as_ref() {
            tabs.extend(form.spec.extra_tabs.iter().copied().map(DbTab::Extra));
        }
        tabs.push(DbTab::Sql);
        tabs
    }

    pub(super) fn db_tab_button(&self, tab: DbTab, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let active = matches!(
            self.db_dialog,
            Some(DbDialog::Edit(ref form)) if form.tab == tab
        );
        let label = match tab {
            DbTab::General => t!("database.tab.general").to_string(),
            DbTab::Extra(kind) => t!(database_editor_tab_key(kind)).to_string(),
            DbTab::Sql => t!("database.tab.sql").to_string(),
        };
        let id = match tab {
            DbTab::General => "db-tab-general",
            DbTab::Extra(DatabaseEditorTab::Filegroups) => "db-tab-filegroups",
            DbTab::Extra(DatabaseEditorTab::Files) => "db-tab-files",
            DbTab::Extra(DatabaseEditorTab::Advanced) => "db-tab-advanced",
            DbTab::Extra(DatabaseEditorTab::Comment) => "db-tab-comment",
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
            let mut sql = connection.create_database_sql(name, &form.options());
            if let Some(owner) = non_empty_owner(&form.owner) {
                if !sql.is_empty() {
                    sql.push('\n');
                }
                sql.push_str(&format!(
                    "ALTER AUTHORIZATION ON DATABASE::[{name}] TO [{owner}];"
                ));
            }
            return Some(sql);
        }

        if form.options() == form.original_options() {
            return None;
        }

        let sql =
            connection.alter_database_sql(&form.name, &form.original_options(), &form.options());
        if sql.is_empty() {
            return None;
        }
        let owner_changed = form.owner != form.original_owner;
        let mut script = sql;
        if owner_changed && let Some(owner) = non_empty_owner(&form.owner) {
            script.push_str(&format!(
                "; ALTER AUTHORIZATION ON DATABASE::{} TO [{}]",
                form.name, owner
            ));
        }
        Some(script)
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

    // ----- Schema management (SQL Server) --------------------------------------------------------

    /// Load a database's list of schemas into its node, so an empty (newly created) schema is
    /// visible in the tree. Only engines that declare schema support do this.
    pub(super) fn load_schemas(
        &mut self,
        connection_index: usize,
        database_index: usize,
        connection: Arc<dyn Connection>,
        database_name: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.schemas = Some(Loadable::Loading);
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.list_schemas(&database_name).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(node) = view.connections.get_mut(connection_index)
                    && let Loadable::Loaded(databases) = &mut node.databases
                    && let Some(database) = databases.get_mut(database_index)
                {
                    database.schemas = Some(match result {
                        Ok(schemas) => Loadable::Loaded(schemas),
                        Err(error) => Loadable::Failed(error.to_string()),
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Open the "New Schema" dialog for a database (SQL Server).
    pub(super) fn open_new_schema(
        &mut self,
        connection_index: usize,
        database_index: usize,
        _window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let weak = cx.weak_entity();
        let input = make_schema_name_input(self.theme, &weak, cx);
        self.schema_name_input = Some(input);
        self.schema_dialog = Some(SchemaDialog {
            connection_index,
            database_index,
            name: String::new(),
            submitting: false,
            error: None,
        });
        // The field's inner gpui-kit state is created on the dialog's first render, so focus is
        // requested there via `schema_focus_pending` rather than here.
        self.schema_focus_pending = true;
        cx.notify();
    }

    pub(super) fn schema_cancel(&mut self, cx: &mut Context<'_, Self>) {
        self.schema_dialog = None;
        self.schema_name_input = None;
        cx.notify();
    }

    pub(super) fn schema_name_changed(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        if let Some(dialog) = self.schema_dialog.as_mut() {
            dialog.name = value.to_string();
            dialog.error = None;
        }
        cx.notify();
    }

    /// Create the schema named in the dialog and reload the database's schema list.
    pub(super) fn schema_submit(&mut self, cx: &mut Context<'_, Self>) {
        let Some(mut dialog) = self.schema_dialog.take() else {
            return;
        };
        let name = dialog.name.trim().to_string();
        if !is_valid_identifier(&name) {
            dialog.submitting = false;
            dialog.error = Some(t!("database.invalid_name").to_string());
            self.schema_dialog = Some(dialog);
            cx.notify();
            return;
        }
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            self.schema_name_input = None;
            cx.notify();
            return;
        };
        let Some(database) = self.database_name(dialog.connection_index, dialog.database_index)
        else {
            self.schema_name_input = None;
            cx.notify();
            return;
        };

        dialog.submitting = true;
        let connection_index = dialog.connection_index;
        let database_index = dialog.database_index;
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn({
                    let connection = connection.clone();
                    let database = database.clone();
                    let name = name.clone();
                    async move { connection.create_schema(&database, &name).await }
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(()) => {
                        view.schema_name_input = None;
                        view.schema_dialog = None;
                        let connection = view.connection_arc(connection_index);
                        let database_name = view.database_name(connection_index, database_index);
                        if let (Some(connection), Some(database_name)) = (connection, database_name)
                        {
                            view.load_schemas(
                                connection_index,
                                database_index,
                                connection,
                                database_name,
                                cx,
                            );
                        }
                    }
                    Err(error) => {
                        dialog.submitting = false;
                        dialog.error = Some(error.to_string());
                        view.schema_dialog = Some(dialog);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Open the shared confirm dialog to drop a schema.
    pub(super) fn confirm_delete_schema(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: String,
        cx: &mut Context<'_, Self>,
    ) {
        self.context_menu = None;
        self.delete_confirm = Some(DeleteConfirm::Schema {
            connection_index,
            database_index,
            schema,
        });
        cx.notify();
    }

    /// Drop a schema and reload the database's schema list.
    pub(super) fn delete_schema(
        &mut self,
        connection_index: usize,
        database_index: usize,
        schema: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn({
                    let connection = connection.clone();
                    let database = database.clone();
                    let schema = schema.clone();
                    async move { connection.drop_schema(&database, &schema).await }
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(()) => {
                        let connection = view.connection_arc(connection_index);
                        let database_name = view.database_name(connection_index, database_index);
                        if let (Some(connection), Some(database_name)) = (connection, database_name)
                        {
                            view.load_schemas(
                                connection_index,
                                database_index,
                                connection,
                                database_name,
                                cx,
                            );
                        }
                    }
                    Err(error) => view.error_dialog = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// The i18n key for an engine-declared database-dialog tab.
pub(super) fn database_editor_tab_key(kind: DatabaseEditorTab) -> &'static str {
    match kind {
        DatabaseEditorTab::Filegroups => "database.tab.filegroups",
        DatabaseEditorTab::Files => "database.tab.files",
        DatabaseEditorTab::Advanced => "database.tab.advanced",
        DatabaseEditorTab::Comment => "database.tab.comment",
    }
}

/// A trimmed owner name, or `None`.
fn non_empty_owner(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}
