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

    pub(super) fn save_form(&mut self, cx: &mut Context<'_, Self>) {
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
            self.form = None;
            self.form_inputs = None;
            self.test_status = TestStatus::Idle;

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

                    view.form = None;
                    view.form_inputs = None;
                    view.test_status = TestStatus::Idle;
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
