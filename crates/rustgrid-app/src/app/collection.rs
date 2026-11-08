//! The 新建/设计集合 dialog for document-store engines (MongoDB).
//!
//! A collection is not a relational table, so the SQL table designer cannot edit it. This dialog
//! carries the document-store definition the driver exposes: the name (created or renamed) and the
//! collection validator (a query/`$jsonSchema` document), with MongoDB's validation level/action.
//! It is shown only when the connection advertises `DriverCapability::Documents`.

use super::dialogs::{DIALOG_ENTRANCE, take_dialog_focus};
use super::*;
use gpui_kit::component::WindowExt;
use gpui_kit::component::dialog::DialogButtonProps;

impl AppView {
    /// Open the empty 新建集合 dialog for `database`.
    pub(super) fn open_new_collection(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let weak = cx.weak_entity();
        self.collection_dialog = Some(CollectionDialog {
            connection_index,
            database_index,
            original: None,
            name: String::new(),
            validator: String::new(),
            validation_level: "strict".to_string(),
            validation_action: "error".to_string(),
            loading: false,
            loaded: true,
            saving: false,
            error: None,
        });
        self.collection_name_input = Some(make_collection_name_input(self.theme, &weak, cx));
        self.collection_validator_input =
            Some(make_collection_validator_input(self.theme, &weak, cx));
        self.collection_level_combo = Some(make_collection_validation_combo(
            self.theme, &weak, true, cx,
        ));
        self.collection_action_combo = Some(make_collection_validation_combo(
            self.theme, &weak, false, cx,
        ));
        cx.notify();
    }

    /// Open 设计集合 for an existing collection and load its current definition.
    pub(super) fn open_design_collection(
        &mut self,
        connection_index: usize,
        database_index: usize,
        collection: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        let weak = cx.weak_entity();
        self.collection_dialog = Some(CollectionDialog {
            connection_index,
            database_index,
            original: Some(collection.clone()),
            name: collection.clone(),
            validator: String::new(),
            validation_level: "strict".to_string(),
            validation_action: "error".to_string(),
            loading: true,
            loaded: false,
            saving: false,
            error: None,
        });
        self.collection_name_input = Some(make_collection_name_input(self.theme, &weak, cx));
        self.collection_validator_input =
            Some(make_collection_validator_input(self.theme, &weak, cx));
        self.collection_level_combo = Some(make_collection_validation_combo(
            self.theme, &weak, true, cx,
        ));
        self.collection_action_combo = Some(make_collection_validation_combo(
            self.theme, &weak, false, cx,
        ));

        let runtime = self.runtime.clone();
        let load_name = collection.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    connection
                        .collection_definition(&database, &load_name)
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.collection_dialog.as_mut() else {
                    return;
                };
                if dialog.original.as_deref() != Some(collection.as_str()) {
                    return;
                }
                dialog.loading = false;
                match result {
                    Ok(Some(definition)) => {
                        dialog.name = definition.name;
                        dialog.validator = definition.validator;
                        if !definition.validation_level.is_empty() {
                            dialog.validation_level = definition.validation_level;
                        }
                        if !definition.validation_action.is_empty() {
                            dialog.validation_action = definition.validation_action;
                        }
                        dialog.loaded = true;
                        app.sync_collection_dialog_inputs(cx);
                    }
                    Ok(None) => {
                        dialog.error = Some(t!("collection.not_found").to_string());
                    }
                    Err(error) => {
                        dialog.error = Some(match error {
                            Error::Query(text) => text,
                            other => other.to_string(),
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Push the dialog state into its input entities (after an async definition load).
    fn sync_collection_dialog_inputs(&self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.collection_dialog.as_ref() else {
            return;
        };
        if let Some(input) = self.collection_name_input.clone() {
            input.update(cx, |input, cx| input.set_text(dialog.name.clone(), cx));
        }
        if let Some(input) = self.collection_validator_input.clone() {
            input.update(cx, |input, cx| input.set_text(dialog.validator.clone(), cx));
        }
        if let Some(combo) = self.collection_level_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_selected(dialog.validation_level.clone(), cx)
            });
        }
        if let Some(combo) = self.collection_action_combo.clone() {
            combo.update(cx, |combo, cx| {
                combo.set_selected(dialog.validation_action.clone(), cx)
            });
        }
    }

    /// `Root` dialog builder for the 新建/设计集合 modal.
    pub(super) fn open_collection_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let (title, ok_text) = app.update(cx, |app, _cx| {
                let editing = app
                    .collection_dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.original.is_some());
                (
                    t!(if editing {
                        "collection.dialog.design_title"
                    } else {
                        "collection.dialog.new_title"
                    })
                    .to_string(),
                    t!(if editing {
                        "common.save"
                    } else {
                        "collection.dialog.create"
                    })
                    .to_string(),
                )
            });
            let content_app = app.clone();
            let theme = app.read(cx).theme;
            let footer_cancel = app.downgrade();
            let footer_ok = app.downgrade();
            let ok_app = app.downgrade();
            let cancel_app = app.downgrade();
            let close_app = app.downgrade();
            dialog
                .entrance(DIALOG_ENTRANCE)
                .title(title)
                .w(px(520.0))
                .centered()
                .content(move |content, window, cx| {
                    let body = content_app.update(cx, |app, cx| {
                        app.collection_dialog_body(cx).into_any_element()
                    });
                    if content_app
                        .update(cx, |app, _cx| take_dialog_focus(&mut app.dialog_focus_done))
                        && let Some(input) = content_app.read(cx).collection_name_input.clone()
                    {
                        input.update(cx, |input, cx| input.focus_state(window, cx));
                    }
                    content.child(body)
                })
                // A plain `Dialog` renders no buttons from `button_props`; the actions live in this
                // footer (the props still install the Enter/Escape handlers).
                .footer(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .w_full()
                        .child(ui::button(
                            "collection-cancel",
                            t!("form.cancel").to_string(),
                            ButtonKind::Normal,
                            theme,
                            move |_event, _window, cx| {
                                let _ =
                                    footer_cancel.update(cx, |app, cx| app.cancel_collection(cx));
                            },
                        ))
                        .child(ui::button(
                            "collection-ok",
                            ok_text.clone(),
                            ButtonKind::Default,
                            theme,
                            move |_event, _window, cx| {
                                let _ = footer_ok.update(cx, |app, cx| app.submit_collection(cx));
                            },
                        )),
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(ok_text)
                        .cancel_text(t!("form.cancel").to_string())
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let _ = ok_app.update(cx, |app, cx| app.submit_collection(cx));
                            false
                        })
                        .on_cancel(move |_, _, cx| {
                            let _ = cancel_app.update(cx, |app, cx| app.cancel_collection(cx));
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = close_app.update(cx, |app, cx| app.cancel_collection(cx));
                })
        });
    }

    fn collection_dialog_body(&self, _cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.collection_dialog.as_ref() else {
            return div().into_any_element();
        };
        let field = |label: String, control: AnyElement| -> AnyElement {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(theme.text_muted))
                        .child(label),
                )
                .child(control)
                .into_any_element()
        };
        let input = |entity: Option<Entity<TextInput>>| {
            entity
                .map(|input| div().h(px(26.0)).child(input).into_any_element())
                .unwrap_or_else(|| div().into_any_element())
        };

        let mut body = div().flex().flex_col().gap_3().p_4();
        body = body.child(field(
            t!("common.name").to_string(),
            input(self.collection_name_input.clone()),
        ));
        if dialog.loading || dialog.saving {
            body = body.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("common.loading").to_string()),
            );
        }
        body = body.child(field(
            t!("collection.validator").to_string(),
            input(self.collection_validator_input.clone()),
        ));
        body = body.child(
            div()
                .flex()
                .flex_row()
                .gap_3()
                .child(
                    div().flex_1().min_w(px(0.0)).child(field(
                        t!("collection.validation_level").to_string(),
                        self.collection_level_combo
                            .clone()
                            .map(|combo| combo.into_any_element())
                            .unwrap_or_else(|| div().into_any_element()),
                    )),
                )
                .child(
                    div().flex_1().min_w(px(0.0)).child(field(
                        t!("collection.validation_action").to_string(),
                        self.collection_action_combo
                            .clone()
                            .map(|combo| combo.into_any_element())
                            .unwrap_or_else(|| div().into_any_element()),
                    )),
                ),
        );
        body = body.child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(theme.text_muted))
                .child(t!("collection.validator_hint").to_string()),
        );
        if let Some(error) = dialog.error.as_ref() {
            body = body.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(theme.danger))
                    .child(error.clone()),
            );
        }
        body.into_any_element()
    }

    pub(super) fn cancel_collection(&mut self, cx: &mut Context<'_, Self>) {
        self.collection_dialog = None;
        self.collection_name_input = None;
        self.collection_validator_input = None;
        self.collection_level_combo = None;
        self.collection_action_combo = None;
        cx.notify();
    }

    pub(super) fn submit_collection(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.collection_dialog.as_mut() else {
            return;
        };
        if dialog.saving || dialog.loading {
            return;
        }
        if dialog.original.is_some() && !dialog.loaded {
            dialog.error = Some(t!("collection.not_loaded").to_string());
            cx.notify();
            return;
        }
        let name = dialog.name.trim().to_string();
        if name.is_empty() {
            dialog.error = Some(t!("collection.name_required").to_string());
            cx.notify();
            return;
        }
        let definition = CollectionDefinition {
            name,
            validator: dialog.validator.trim().to_string(),
            validation_level: dialog.validation_level.clone(),
            validation_action: dialog.validation_action.clone(),
        };
        let original = dialog.original.clone();
        let connection_index = dialog.connection_index;
        let database_index = dialog.database_index;
        let (Some(connection), Some(database)) = (
            self.connection_arc(connection_index),
            self.database_name(connection_index, database_index),
        ) else {
            return;
        };
        if let Some(dialog) = self.collection_dialog.as_mut() {
            dialog.saving = true;
            dialog.error = None;
        }
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    match original {
                        Some(original) => {
                            connection
                                .alter_collection(&database, &original, &definition)
                                .await
                        }
                        None => connection.create_collection(&database, &definition).await,
                    }
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let _ = this.update(cx, |app, cx| match result {
                Ok(()) => {
                    app.collection_dialog = None;
                    app.collection_name_input = None;
                    app.collection_validator_input = None;
                    app.collection_level_combo = None;
                    app.collection_action_combo = None;
                    if let (Some(connection), Some(database)) = (
                        app.connection_arc(connection_index),
                        app.database_name(connection_index, database_index),
                    ) {
                        app.load_tables(connection_index, database_index, connection, database, cx);
                    }
                    cx.notify();
                }
                Err(error) => {
                    if let Some(dialog) = app.collection_dialog.as_mut() {
                        dialog.saving = false;
                        dialog.error = Some(match error {
                            Error::Query(text) => text,
                            other => other.to_string(),
                        });
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
}

fn make_collection_name_input(
    theme: Theme,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    let submit = app.clone();
    let cancel = app.clone();
    cx.new(move |cx| {
        TextInput::new(theme, "", TextInputOptions::default(), cx)
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| {
                    if let Some(dialog) = app.collection_dialog.as_mut() {
                        dialog.name = text.to_string();
                        dialog.error = None;
                    }
                    cx.notify();
                });
            }))
            .on_submit(Rc::new(move |_window, cx| {
                let _ = submit.update(cx, |app, cx| app.submit_collection(cx));
            }))
            .on_cancel(Rc::new(move |_window, cx| {
                let _ = cancel.update(cx, |app, cx| app.cancel_collection(cx));
            }))
    })
}

fn make_collection_validator_input(
    theme: Theme,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    cx.new(move |cx| {
        TextInput::new(
            theme,
            "",
            TextInputOptions {
                placeholder: t!("collection.validator_placeholder").to_string().into(),
                ..TextInputOptions::default()
            },
            cx,
        )
        .on_change(Rc::new(move |text, _window, cx| {
            let _ = change.update(cx, |app, cx| {
                if let Some(dialog) = app.collection_dialog.as_mut() {
                    dialog.validator = text.to_string();
                    dialog.error = None;
                }
                cx.notify();
            });
        }))
    })
}

/// The validation-level combo (`level = true`) or the validation-action combo.
fn make_collection_validation_combo(
    theme: Theme,
    app: &WeakEntity<AppView>,
    level: bool,
    cx: &mut Context<'_, AppView>,
) -> Entity<ComboBox> {
    let choices: Vec<(&str, &str)> = if level {
        vec![
            ("off", "collection.validation.level.off"),
            ("strict", "collection.validation.level.strict"),
            ("moderate", "collection.validation.level.moderate"),
        ]
    } else {
        vec![
            ("warn", "collection.validation.action.warn"),
            ("error", "collection.validation.action.error"),
        ]
    };
    let options: Vec<ComboOption> = choices
        .iter()
        .map(|(value, label)| ComboOption::new(*value, t!(*label).to_string()))
        .collect();
    let selected = choices[1].0.to_string();
    let weak = app.clone();
    cx.new(move |cx| {
        ComboBox::new(theme, options, selected, 200.0, cx).on_select(Rc::new(
            move |value, _window, cx| {
                let _ = weak.update(cx, |app, cx| {
                    if let Some(dialog) = app.collection_dialog.as_mut() {
                        if level {
                            dialog.validation_level = value.to_string();
                        } else {
                            dialog.validation_action = value.to_string();
                        }
                        dialog.error = None;
                    }
                    cx.notify();
                });
            },
        ))
    })
}
