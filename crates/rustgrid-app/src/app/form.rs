use super::*;

impl AppView {
    /// Write one field's edited text into the open form. Called from the input's change
    /// callback, so it must not read the input entity back (it is borrowed during the update).
    pub(super) fn set_form_field(
        &mut self,
        field: FormField,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(form) = self.form.as_mut() {
            form.set_value(field, text.to_string());
        }
        // Editing a flagged field clears its "required" marker.
        self.form_errors.remove(&field);
        cx.notify();
    }

    /// The focus handle of the next (`shift == false`) or previous form field.
    pub(super) fn form_neighbor(
        &mut self,
        field: FormField,
        shift: bool,
        cx: &mut Context<'_, Self>,
    ) -> Option<FocusHandle> {
        let inputs = self.form_inputs.as_ref()?;
        let position = FORM_FIELDS.iter().position(|item| *item == field)?;
        let target = if shift {
            FORM_FIELDS[(position + FORM_FIELDS.len() - 1) % FORM_FIELDS.len()]
        } else {
            FORM_FIELDS[(position + 1) % FORM_FIELDS.len()]
        };
        Some(inputs.get(target).read(cx).focus_handle())
    }

    /// Validate the required fields before 测试连接 / 保存并连接. Returns `true` when the form is
    /// invalid: the empty fields are flagged inline and the page with the error is shown so the
    /// markers are visible. The current page is validated first — it stays put when it is the one
    /// holding the error, and only jumps to the first page with one otherwise.
    fn validate_form(&mut self, cx: &mut Context<'_, Self>) -> bool {
        let missing = self
            .form
            .as_ref()
            .map(|form| form.missing_required())
            .unwrap_or_default();
        let tunnel_missing = self
            .form
            .as_ref()
            .map(missing_tunnel_fields)
            .unwrap_or_default();
        if missing.is_empty() && tunnel_missing.is_empty() {
            self.form_errors.clear();
            self.form_tunnel_errors.clear();
            return false;
        }
        self.test_status = TestStatus::Idle;
        let general_has_error = !missing.is_empty();
        let tunnel_has_error = !tunnel_missing.is_empty();
        self.form_errors = missing.into_iter().collect();
        self.form_tunnel_errors = tunnel_missing;
        // Validate the page the user is looking at first: stay on it when it is the one with the
        // error, and only jump to the first page that has one (常规 before 隧道) when the current
        // page is fine.
        let current = self.form.as_ref().map(|form| form.tab).unwrap_or_default();
        let target = match current {
            FormTab::General if general_has_error => FormTab::General,
            FormTab::Tunnel if tunnel_has_error => FormTab::Tunnel,
            _ if general_has_error => FormTab::General,
            _ if tunnel_has_error => FormTab::Tunnel,
            _ => current,
        };
        if let Some(form) = self.form.as_mut() {
            form.tab = target;
        }
        cx.notify();
        true
    }

    pub(super) fn save_form(&mut self, cx: &mut Context<'_, Self>) {
        if self.validate_form(cx) {
            return;
        }
        let (profile, password, password_saved) = {
            let Some(form) = self.form.as_ref() else {
                return;
            };
            let password = if form.password.is_empty() {
                None
            } else {
                Some(form.password.clone())
            };
            let password_saved = form.save_password && password.is_some();
            (form.to_profile(), password, password_saved)
        };

        let duplicate =
            self.connections.iter().enumerate().any(|(index, node)| {
                Some(index) != self.editing && node.profile.name == profile.name
            });
        if duplicate {
            self.test_status = TestStatus::Failed(t!("form.duplicate_name").to_string());
            cx.notify();
            return;
        }

        if let Some(index) = self.editing.take() {
            if let Some(node) = self.connections.get_mut(index) {
                let mut profile = profile;
                profile.id = node.profile.id.clone();
                node.profile = profile;
                node.password = password;
                node.password_saved = password_saved;
            }
            self.close_connection_window(cx);

            let profiles: Vec<_> = self
                .connections
                .iter()
                .map(|node| node.profile.clone())
                .collect();
            let _ = self.config.save_profiles(&profiles);
            self.persist_secrets();

            self.disconnect(index, cx);
            self.connect(index, cx);
            cx.notify();
            return;
        }

        // A new connection is only added once it actually connects; a failed
        // attempt keeps the dialog open with the error instead of adding an
        // unusable entry to the sidebar.
        let Some(driver) = self.registry.get(&profile.driver) else {
            self.test_status = TestStatus::Failed(t!("error.driver_missing").to_string());
            cx.notify();
            return;
        };

        self.test_status = TestStatus::Testing;
        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let config = ConnectionConfig {
                driver: profile.driver.clone(),
                host: profile.host.clone(),
                port: profile.port,
                username: profile.username.clone(),
                password: password.clone(),
                database: profile.database.clone(),
                options: profile.options.clone(),
                settings: profile.settings.clone(),
            };

            let result = match runtime
                .spawn(async move { driver.connect(&config).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| match result {
                Ok(connection) => {
                    view.connections.push(ConnectionNode {
                        profile,
                        password,
                        password_saved,
                        status: ConnectionStatus::Connected(Arc::from(connection)),
                        databases: Loadable::Idle,
                        expanded: true,
                    });
                    let index = view.connections.len() - 1;

                    let profiles: Vec<_> = view
                        .connections
                        .iter()
                        .map(|node| node.profile.clone())
                        .collect();
                    let _ = view.config.save_profiles(&profiles);
                    view.persist_secrets();

                    view.close_connection_window(cx);
                    view.load_databases(index, cx);
                }
                Err(error) => {
                    view.test_status = TestStatus::Failed(error.to_string());
                }
            });
        })
        .detach();
    }

    pub(super) fn test_form(&mut self, cx: &mut Context<'_, Self>) {
        if self.validate_form(cx) {
            return;
        }
        let Some(form) = self.form.as_ref() else {
            return;
        };

        let profile = form.to_profile();
        let password = if form.password.is_empty() {
            None
        } else {
            Some(form.password.clone())
        };

        let Some(driver) = self.registry.get(&profile.driver) else {
            self.test_status = TestStatus::Failed(t!("error.driver_missing").to_string());
            cx.notify();
            return;
        };

        self.test_status = TestStatus::Testing;
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let config = ConnectionConfig {
                driver: profile.driver.clone(),
                host: profile.host.clone(),
                port: profile.port,
                username: profile.username.clone(),
                password,
                database: profile.database.clone(),
                options: profile.options.clone(),
                settings: profile.settings.clone(),
            };

            let status = match runtime
                .spawn(async move { driver.connect(&config).await })
                .await
            {
                Ok(Ok(connection)) => {
                    let _ = runtime.spawn(async move { connection.close().await }).await;
                    TestStatus::Success
                }
                Ok(Err(error)) => TestStatus::Failed(error.to_string()),
                Err(error) => TestStatus::Failed(Error::other(error).to_string()),
            };

            let _ = this.update(cx, |view, cx| {
                view.test_status = status;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn submit_password(&mut self, cx: &mut Context<'_, Self>) {
        let Some(prompt) = self.password_prompt.take() else {
            return;
        };

        let index = prompt.index;
        let password = if prompt.password.is_empty() {
            None
        } else {
            Some(prompt.password)
        };
        let password_saved = prompt.save_password && password.is_some();

        if let Some(node) = self.connections.get_mut(index) {
            node.password = password;
            node.password_saved = password_saved;
            node.status = ConnectionStatus::Disconnected;
        }

        self.persist_secrets();
        self.connect(index, cx);
        cx.notify();
    }
}

/// The required fields of the active tunnel layer that are empty. The window edits a single layer
/// (the first), so only that one is validated.
fn missing_tunnel_fields(form: &ConnectionForm) -> BTreeSet<connection_form::TunnelField> {
    let mut missing = BTreeSet::new();
    let Some(layer) = form.settings.tunnel.first() else {
        return missing;
    };
    if layer.host.trim().is_empty() {
        missing.insert(connection_form::TunnelField::Host);
    }
    if layer.port == 0 {
        missing.insert(connection_form::TunnelField::Port);
    }
    if layer.kind == TunnelKind::Ssh
        && layer.auth == TunnelAuth::KeyFile
        && layer.key_path.trim().is_empty()
    {
        missing.insert(connection_form::TunnelField::KeyPath);
    }
    missing
}
