use super::*;

impl AppView {
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

        if let Some(index) = self.editing.take() {
            if let Some(node) = self.connections.get_mut(index) {
                let mut profile = profile;
                profile.id = node.profile.id.clone();
                node.profile = profile;
                node.password = password;
                node.password_saved = password_saved;
            }
            self.form = None;
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

    pub(super) fn form_key(
        &mut self,
        field: FormField,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.form.is_none() {
            return;
        }

        self.form_active_field = field;

        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.control || keystroke.modifiers.platform;
        let shift = keystroke.modifiers.shift;

        let text = self
            .form
            .as_ref()
            .map(|form| form.value(field).to_string())
            .unwrap_or_default();
        let mut chars: Vec<char> = text.chars().collect();
        let len = chars.len();

        let mut selection = self.form_selection;
        selection.anchor = selection.anchor.min(len);
        selection.cursor = selection.cursor.min(len);
        let (start, end) = selection.range();

        if command {
            match keystroke.key.as_str() {
                "a" => {
                    self.set_selection(0, len, cx);
                }
                "c" => {
                    if start < end {
                        let selected: String = chars[start..end].iter().copied().collect();
                        cx.write_to_clipboard(ClipboardItem::new_string(selected));
                    }
                }
                "x" => {
                    if start < end {
                        let selected: String = chars[start..end].iter().copied().collect();
                        cx.write_to_clipboard(ClipboardItem::new_string(selected));
                        chars.drain(start..end);
                        self.set_field_text(field, &chars, start, cx);
                    }
                }
                "v" => {
                    if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        let pasted: Vec<char> = pasted
                            .chars()
                            .filter(|ch| *ch != '\n' && *ch != '\r' && *ch != '\t')
                            .collect();
                        if !pasted.is_empty() {
                            let mut next = Vec::with_capacity(chars.len() + pasted.len());
                            next.extend_from_slice(&chars[..start]);
                            next.extend_from_slice(&pasted);
                            next.extend_from_slice(&chars[end..]);
                            let caret = start + pasted.len();
                            self.set_field_text(field, &next, caret, cx);
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        match keystroke.key.as_str() {
            "enter" => {
                if !matches!(self.test_status, TestStatus::Testing) {
                    self.save_form(cx);
                }
            }
            "tab" => {
                let position = FORM_FIELDS
                    .iter()
                    .position(|item| *item == field)
                    .unwrap_or(0);
                let target = if shift {
                    FORM_FIELDS[(position + FORM_FIELDS.len() - 1) % FORM_FIELDS.len()]
                } else {
                    FORM_FIELDS[(position + 1) % FORM_FIELDS.len()]
                };
                window.focus(self.form_focus.get(target));
                self.form_active_field = target;
                let target_len = self
                    .form
                    .as_ref()
                    .map(|form| form.value(target).chars().count())
                    .unwrap_or(0);
                self.form_selection = FieldSelection {
                    anchor: target_len,
                    cursor: target_len,
                };
                self.caret_visible = true;
                cx.notify();
            }
            "backspace" => {
                if start < end {
                    chars.drain(start..end);
                    self.set_field_text(field, &chars, start, cx);
                } else if start > 0 {
                    chars.remove(start - 1);
                    self.set_field_text(field, &chars, start - 1, cx);
                }
            }
            "delete" => {
                if start < end {
                    chars.drain(start..end);
                    self.set_field_text(field, &chars, start, cx);
                } else if start < len {
                    chars.remove(start);
                    self.set_field_text(field, &chars, start, cx);
                }
            }
            "left" => {
                let cursor = if shift {
                    selection.cursor.saturating_sub(1)
                } else if start < end {
                    start
                } else {
                    start.saturating_sub(1)
                };
                let anchor = if shift { selection.anchor } else { cursor };
                self.set_selection(anchor, cursor, cx);
            }
            "right" => {
                let cursor = if shift {
                    (selection.cursor + 1).min(len)
                } else if start < end {
                    end
                } else {
                    (start + 1).min(len)
                };
                let anchor = if shift { selection.anchor } else { cursor };
                self.set_selection(anchor, cursor, cx);
            }
            "home" => {
                let anchor = if shift { selection.anchor } else { 0 };
                self.set_selection(anchor, 0, cx);
            }
            "end" => {
                let anchor = if shift { selection.anchor } else { len };
                self.set_selection(anchor, len, cx);
            }
            "space" => self.insert_text(field, &chars, start, end, " ", cx),
            _ => {
                if let Some(insert) = keystroke.key_char.as_ref()
                    && !insert.is_empty()
                {
                    self.insert_text(field, &chars, start, end, insert, cx);
                }
            }
        }
    }

    pub(super) fn set_selection(
        &mut self,
        anchor: usize,
        cursor: usize,
        cx: &mut Context<'_, Self>,
    ) {
        self.form_selection = FieldSelection { anchor, cursor };
        self.caret_visible = true;
        cx.notify();
    }

    pub(super) fn insert_text(
        &mut self,
        field: FormField,
        chars: &[char],
        start: usize,
        end: usize,
        insert: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let insert: Vec<char> = insert.chars().collect();
        let mut next = Vec::with_capacity(chars.len() + insert.len());
        next.extend_from_slice(&chars[..start]);
        next.extend_from_slice(&insert);
        next.extend_from_slice(&chars[end..]);
        let caret = start + insert.len();
        self.set_field_text(field, &next, caret, cx);
    }

    pub(super) fn set_field_text(
        &mut self,
        field: FormField,
        chars: &[char],
        caret: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(form) = self.form.as_mut() {
            form.set_value(field, chars.iter().copied().collect());
        }
        self.form_selection = FieldSelection {
            anchor: caret,
            cursor: caret,
        };
        self.caret_visible = true;
        cx.notify();
    }

    pub(super) fn field_index_for_x(&self, text: &str, x: Pixels, window: &Window) -> usize {
        let char_count = text.chars().count();
        if char_count == 0 {
            return 0;
        }

        let viewport_width: f32 = window.viewport_size().width.into();
        let offset_x: f32 = self.form_offset.x.into();
        let panel_left = (viewport_width - PANEL_WIDTH) / 2.0 + offset_x;
        let text_left = panel_left + FIELD_TEXT_LEFT;
        let relative = f32::from(x) - text_left;
        if relative <= 0.0 {
            return 0;
        }

        let run = window.text_style().to_run(text.len());
        let layout = window
            .text_system()
            .layout_line(text, px(12.0), &[run], None);
        let byte = layout.closest_index_for_x(px(relative)).min(text.len());
        text[..byte].chars().count()
    }

    pub(super) fn password_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let mut submit = false;

        if let Some(prompt) = self.password_prompt.as_mut() {
            let keystroke = &event.keystroke;
            if keystroke.modifiers.control || keystroke.modifiers.platform {
                return;
            }

            match keystroke.key.as_str() {
                "backspace" => {
                    prompt.password.pop();
                }
                "space" => prompt.password.push(' '),
                "enter" => submit = true,
                _ => {
                    if let Some(text) = keystroke.key_char.as_ref() {
                        prompt.password.push_str(text);
                    }
                }
            }
        }

        if submit {
            self.submit_password(cx);
        }

        cx.notify();
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
