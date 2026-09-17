//! The app's modal dialogs, hosted by gpui-kit's `Root` via `window.open_dialog`.
//!
//! AppView's state (`error_dialog`, `delete_confirm`, `password_prompt`, ...) stays the source of
//! truth; [`AppView::sync_dialog`] reconciles it with the Root dialog stack each render, opening
//! or closing as needed.

use std::cell::Cell;

use super::*;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::dialog::DialogButtonProps;

/// gpui-kit anchors a dialog at 10% of the window height, which leaves short dialogs stranded near
/// the top. Compute a top margin that centres a dialog of roughly `dialog_height` instead — the
/// exact height is only known after layout, so callers pass an estimate.
fn centered_margin_top(window: &Window, dialog_height: f32) -> Pixels {
    let viewport = window.viewport_size().height;
    ((viewport - px(dialog_height)) * 0.5).max(px(24.0))
}

impl AppView {
    /// Reconciles AppView dialog state with the `Root`-owned dialog stack. Called every render.
    pub(super) fn sync_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let desired = if self.form.is_some() {
            Some(DialogKind::ConnectionForm)
        } else if self.db_dialog.is_some() {
            Some(DialogKind::DbDialog)
        } else if self.password_prompt.is_some() {
            Some(DialogKind::Password)
        } else if self.error_dialog.is_some() {
            Some(DialogKind::Error)
        } else if self.delete_confirm.is_some() {
            Some(DialogKind::Confirm)
        } else if self.options_open {
            Some(DialogKind::Options)
        } else {
            None
        };

        if desired == self.opened_dialog {
            return;
        }
        self.opened_dialog = desired;

        // Drop whatever was showing before switching to the new state.
        window.close_all_dialogs(cx);

        match desired {
            Some(DialogKind::ConnectionForm) => self.open_connection_form_dialog(window, cx),
            Some(DialogKind::DbDialog) => self.open_db_dialog(window, cx),
            Some(DialogKind::Password) => self.open_password_prompt(window, cx),
            Some(DialogKind::Error) => self.open_error_dialog(window, cx),
            Some(DialogKind::Confirm) => self.open_confirm_dialog(window, cx),
            Some(DialogKind::Options) => self.open_options_dialog(window, cx),
            None => {}
        }
    }

    fn open_connection_form_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let (heading, footer) = app.update(cx, |app, cx| {
                (app.form_heading(), app.form_footer(cx).into_any_element())
            });
            let on_close = app.downgrade();
            let content_app = app.clone();
            dialog
                .title(heading)
                .w(px(PANEL_WIDTH))
                .content(move |content, window, cx| {
                    let body = content_app
                        .update(cx, |app, cx| app.form_body(window, cx).into_any_element());
                    content.child(body)
                })
                .footer(footer)
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| {
                        app.form = None;
                        app.form_inputs = None;
                        app.context_menu = None;
                        cx.notify();
                    });
                })
        });
    }

    /// The connection form's titlebar text: `Name - New Connection` / `Name - Edit Connection`.
    fn form_heading(&self) -> String {
        let title = if self.editing.is_some() {
            t!("form.edit_title")
        } else {
            t!("form.title")
        };
        let name = self
            .form
            .as_ref()
            .map(|form| form.name.trim().to_string())
            .unwrap_or_default();
        let name = if name.is_empty() {
            t!("app.title").to_string()
        } else {
            name
        };
        format!("{name} - {title}")
    }

    /// The connection form body: the tab strip and the general page.
    fn form_body(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;

        let tabs = div()
            .flex()
            .flex_row()
            .w_full()
            .gap_0p5()
            .px_2()
            .pt_2()
            .bg(rgb(theme.dialog_face))
            .child(form_tab(t!("form.tab.general").to_string(), true, theme))
            .child(form_tab(t!("form.tab.advanced").to_string(), false, theme))
            .child(form_tab(t!("form.tab.database").to_string(), false, theme))
            .child(form_tab("SSL".to_string(), false, theme))
            .child(form_tab("SSH".to_string(), false, theme))
            .child(form_tab("HTTP".to_string(), false, theme));

        let general = div()
            .flex()
            .flex_col()
            .h(px(430.0))
            .mx_2()
            .mb_2()
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.dialog_bg))
            .child(match self.form.as_ref() {
                Some(form) => self.render_general_tab(form, window, cx).into_any_element(),
                None => div().into_any_element(),
            });

        div().flex().flex_col().w_full().child(tabs).child(general)
    }

    /// The connection form footer: test-connection and status on the left, OK/Cancel on the right.
    fn form_footer(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;

        let mut test = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .min_w(px(0.0))
            .gap_2()
            .child(self.dialog_button(
                "form-test",
                t!("form.test_connection").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.test_form(cx)),
            ));

        match &self.test_status {
            TestStatus::Idle => {}
            TestStatus::Testing => {
                test = test.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("form.testing").to_string()),
                );
            }
            TestStatus::Success => {
                test = test.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.icon_connection))
                        .child(t!("form.test_success").to_string()),
                );
            }
            TestStatus::Failed(_) => {}
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .h(px(46.0))
            .child(test)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_none()
                    .gap_2()
                    .child(self.dialog_button(
                        "form-save",
                        t!("form.ok").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| {
                            if !matches!(this.test_status, TestStatus::Testing) {
                                this.save_form(cx);
                            }
                        }),
                    ))
                    .child(self.dialog_button(
                        "form-cancel",
                        t!("form.cancel").to_string(),
                        false,
                        cx.listener(|this, _event, _window, cx| {
                            this.form = None;
                            this.context_menu = None;
                            cx.notify();
                        }),
                    )),
            )
    }

    fn open_error_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let message = app.update(cx, |app, _| app.error_dialog.clone().unwrap_or_default());
            let theme = app.read(cx).theme;
            let on_ok = app.downgrade();
            let on_close = app.downgrade();
            let footer_ok = app.downgrade();
            dialog
                .title(t!("error.title").to_string())
                .margin_top(centered_margin_top(window, 170.0))
                .content(move |content, _window, _cx| {
                    content.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap_3()
                            .child(
                                svg()
                                    .path("icons/circle-x.svg")
                                    .w(px(18.0))
                                    .h(px(18.0))
                                    .flex_none()
                                    .text_color(rgb(theme.danger)),
                            )
                            .child(div().flex_1().min_w(px(0.0)).child(message.clone())),
                    )
                })
                .footer(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .w_full()
                        .child(ui::button(
                            "error-ok",
                            t!("form.ok").to_string(),
                            ButtonKind::Default,
                            theme,
                            move |_event, _window, cx| {
                                let _ = footer_ok.update(cx, |app, cx| {
                                    app.error_dialog = None;
                                    cx.notify();
                                });
                            },
                        )),
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("form.ok").to_string())
                        .show_cancel(false)
                        .on_ok(move |_, _, cx| {
                            let _ = on_ok.update(cx, |app, cx| {
                                app.error_dialog = None;
                                cx.notify();
                            });
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| {
                        app.error_dialog = None;
                        cx.notify();
                    });
                })
        });
    }

    fn open_confirm_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (title, message, confirm_label) = app.update(cx, |app, _| app.confirm_texts());
            let theme = app.read(cx).theme;
            let on_ok = app.downgrade();
            let on_cancel = app.downgrade();
            let on_close = app.downgrade();
            let footer_ok = app.downgrade();
            let footer_cancel = app.downgrade();
            let ok_label = confirm_label.clone();

            dialog
                .title(title)
                .margin_top(centered_margin_top(window, 180.0))
                .content(move |content, _window, _cx| content.child(message.clone()))
                .footer(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .w_full()
                        .child(ui::button(
                            "confirm-cancel",
                            t!("form.cancel").to_string(),
                            ButtonKind::Normal,
                            theme,
                            move |_event, _window, cx| {
                                let _ = footer_cancel.update(cx, |app, cx| app.cancel_delete(cx));
                            },
                        ))
                        .child(ui::button(
                            "confirm-ok",
                            ok_label,
                            ButtonKind::Danger,
                            theme,
                            move |_event, _window, cx| {
                                let _ = footer_ok.update(cx, |app, cx| app.confirm_delete(cx));
                            },
                        )),
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(confirm_label)
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("form.cancel").to_string())
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let _ = on_ok.update(cx, |app, cx| app.confirm_delete(cx));
                            true
                        })
                        .on_cancel(move |_, _, cx| {
                            let _ = on_cancel.update(cx, |app, cx| app.cancel_delete(cx));
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.cancel_delete(cx));
                })
        });
    }

    fn open_password_prompt(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let Some((title, input)) = app.update(cx, |app, _| {
                app.password_prompt.as_ref().map(|prompt| {
                    let name = app
                        .connections
                        .get(prompt.index)
                        .map(|node| node.profile.name.clone())
                        .unwrap_or_default();
                    (
                        format!("{}: {}", t!("password.title"), name),
                        prompt.input.clone(),
                    )
                })
            }) else {
                return dialog.title(String::new());
            };
            let theme = app.read(cx).theme;
            let toggle = app.downgrade();
            let on_ok = app.downgrade();
            let on_cancel = app.downgrade();
            let on_close = app.downgrade();
            let footer_ok = app.downgrade();
            let footer_cancel = app.downgrade();
            let focused = Rc::new(Cell::new(false));
            let content_app = app.clone();

            dialog
                .title(title)
                .margin_top(centered_margin_top(window, 240.0))
                .footer(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .w_full()
                        .child(ui::button(
                            "password-cancel",
                            t!("form.cancel").to_string(),
                            ButtonKind::Normal,
                            theme,
                            move |_event, _window, cx| {
                                let _ = footer_cancel.update(cx, |app, cx| {
                                    app.password_prompt = None;
                                    cx.notify();
                                });
                            },
                        ))
                        .child(ui::button(
                            "password-ok",
                            t!("form.ok").to_string(),
                            ButtonKind::Default,
                            theme,
                            move |_event, _window, cx| {
                                let _ = footer_ok.update(cx, |app, cx| app.submit_password(cx));
                            },
                        )),
                )
                .content(move |content, window, cx| {
                    let (save_password, theme) = content_app.update(cx, |app, _| {
                        let save = app
                            .password_prompt
                            .as_ref()
                            .map(|prompt| prompt.save_password)
                            .unwrap_or(false);
                        (save, app.theme)
                    });
                    if !focused.get() {
                        focused.set(true);
                        let handle = input.read(cx).focus_handle();
                        window.focus(&handle, cx);
                    }
                    let toggle = toggle.clone();
                    content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().h(px(28.0)).child(input.clone()))
                            .child(
                                div()
                                    .id("password-save")
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .on_click(move |_event, _window, cx| {
                                        let _ = toggle.update(cx, |app, cx| {
                                            if let Some(prompt) = app.password_prompt.as_mut() {
                                                prompt.save_password = !prompt.save_password;
                                            }
                                            cx.notify();
                                        });
                                    })
                                    .child(checkbox_box(save_password, theme))
                                    .child(t!("form.save_password").to_string()),
                            ),
                    )
                })
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("form.ok").to_string())
                        .cancel_text(t!("form.cancel").to_string())
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let _ = on_ok.update(cx, |app, cx| app.submit_password(cx));
                            true
                        })
                        .on_cancel(move |_, _, cx| {
                            let _ = on_cancel.update(cx, |app, cx| {
                                app.password_prompt = None;
                                cx.notify();
                            });
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| {
                        app.password_prompt = None;
                        cx.notify();
                    });
                })
        });
    }

    /// Start the in-place "rename table" editor for one row, pre-filled with its current name and
    /// focused. The row itself is drawn by the pane named in `pane`, which the caller notifies.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn begin_rename_table(
        &mut self,
        pane: RowPane,
        connection_index: usize,
        database_index: usize,
        old_name: String,
        _is_view: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.rename_edit.is_some()
            || !self.table_exists(connection_index, database_index, &old_name)
        {
            return;
        }
        let theme = self.theme;
        let weak = cx.weak_entity();
        let change = weak.clone();
        let submit = weak.clone();
        let cancel = weak.clone();
        let initial = old_name.clone();
        let input = cx.new(move |cx| {
            TextInput::new(
                theme,
                initial,
                TextInputOptions {
                    bare: true,
                    text_size: Some(12.0),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| {
                    if let Some(edit) = app.rename_edit.as_mut() {
                        edit.new_name = text.to_string();
                    }
                    cx.notify();
                });
            }))
            .on_submit(Rc::new(move |window, cx| {
                let _ = submit.update(cx, |app, cx| {
                    let owner = app.rename_owner_focus(cx);
                    app.submit_rename(cx);
                    if let Some(owner) = owner {
                        window.focus(&owner, cx);
                    }
                });
            }))
            .on_cancel(Rc::new(move |window, cx| {
                let _ = cancel.update(cx, |app, cx| {
                    let owner = app.rename_owner_focus(cx);
                    app.rename_edit = None;
                    app.rename_blur = None;
                    if let Some(owner) = owner {
                        window.focus(&owner, cx);
                    }
                    cx.notify();
                });
            }))
        });
        let focus = input.read(cx).focus_handle();
        self.rename_edit = Some(RenameEdit {
            pane,
            connection_index,
            database_index,
            old_name: old_name.clone(),
            new_name: old_name,
            input,
        });
        // Clicking away is a commit, like the grid's in-place cell editor.
        self.rename_blur = Some(cx.on_blur(&focus, window, |app, _window, cx| {
            if app.rename_edit.is_some() {
                app.submit_rename(cx);
            }
        }));
        self.rename_focus_pending = true;
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Whether the loaded object list still holds a table with this name — F2 on a stale
    /// selection (after a rename/drop) must not open an editor for a row that is gone.
    fn table_exists(&self, connection_index: usize, database_index: usize, name: &str) -> bool {
        self.connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index),
                _ => None,
            })
            .is_some_and(|database| {
                matches!(&database.tables, Loadable::Loaded(tables)
                    if tables.iter().any(|table| table.name == name))
            })
    }

    /// Commit the in-place rename: reject an empty or unchanged name, otherwise run the rename.
    pub(super) fn submit_rename(&mut self, cx: &mut Context<'_, Self>) {
        let Some(edit) = self.rename_edit.take() else {
            return;
        };
        self.rename_blur = None;
        self.notify_rename_pane(edit.pane, cx);
        let new_name = edit.new_name.trim().to_string();
        if new_name.is_empty() || new_name == edit.old_name {
            cx.notify();
            return;
        }
        self.rename_table(
            edit.connection_index,
            edit.database_index,
            edit.old_name,
            new_name,
            cx,
        );
        cx.notify();
    }

    /// The title/message/confirm label for the pending destructive action.
    fn confirm_texts(&self) -> (String, String, String) {
        match self.delete_confirm.as_ref() {
            Some(DeleteConfirm::Rows { rows, .. }) => {
                let count = rows.len();
                (
                    t!("grid.delete_title").to_string(),
                    t!("grid.delete_confirm", count = count).to_string(),
                    t!("grid.delete_button", count = count).to_string(),
                )
            }
            Some(DeleteConfirm::Connection { index }) => {
                let name = self
                    .connections
                    .get(*index)
                    .map(|node| node.profile.name.clone())
                    .unwrap_or_default();
                (
                    t!("connection.delete_title").to_string(),
                    t!("connection.delete_confirm", name = name).to_string(),
                    t!("connection.delete_button").to_string(),
                )
            }
            Some(DeleteConfirm::Table {
                name, operation, ..
            }) => {
                let (title_key, message_key, button_key) = match operation {
                    TableOperation::Drop => (
                        "object.delete_table",
                        "object.drop_confirm",
                        "object.drop_button",
                    ),
                    TableOperation::Empty => (
                        "object.empty_table",
                        "object.empty_confirm",
                        "object.empty_button",
                    ),
                    TableOperation::Truncate => (
                        "object.truncate_table",
                        "object.truncate_confirm",
                        "object.truncate_button",
                    ),
                };
                (
                    t!(title_key).to_string(),
                    t!(message_key, name = name.clone()).to_string(),
                    t!(button_key).to_string(),
                )
            }
            None => (String::new(), String::new(), t!("form.ok").to_string()),
        }
    }
}
