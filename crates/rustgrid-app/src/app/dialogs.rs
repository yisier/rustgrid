//! The app's modal dialogs, hosted by gpui-kit's `Root` via `window.open_dialog`.
//!
//! AppView's state (`error_dialog`, `delete_confirm`, `password_prompt`, ...) stays the source of
//! truth; [`AppView::sync_dialog`] reconciles it with the Root dialog stack each render, opening
//! or closing as needed.

use super::objects::object_display;
use super::*;
use gpui_kit::base::SelectableText;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::dialog::{DialogButtonProps, DialogEntrance};

/// The entrance animation every app dialog uses.
///
/// A short in-place fade keeps the modal feeling anchored to where it appears — the previous
/// `set_reduce_motion(true)` in `main.rs` made it appear instantly — while still animating, which
/// is what 0.7.3's configurable `DialogEntrance` is for. Use `DialogEntrance::None` to go back to
/// an instant appearance (or `SlideDown` for the kit's legacy slide).
pub(super) const DIALOG_ENTRANCE: DialogEntrance = DialogEntrance::Fade;

/// One-shot guard for a dialog's initial focus.
///
/// `Root` re-invokes a dialog's builder on every frame, so the "focus the first field once" flag
/// must persist across frames (it lives on `AppView`). Returns `true` the first time and `false`
/// afterwards, so a later control (for example a combo box's search field) keeps the focus the
/// user gives it instead of being overridden each frame.
pub(super) fn take_dialog_focus(done: &mut bool) -> bool {
    if *done {
        false
    } else {
        *done = true;
        true
    }
}

impl AppView {
    /// Reconciles AppView dialog state with the `Root`-owned dialog stack. Called every render.
    pub(super) fn sync_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let desired = if self.db_dialog.is_some() {
            Some(DialogKind::DbDialog)
        } else if self.schema_dialog.is_some() {
            Some(DialogKind::Schema)
        } else if self.create_table_dialog.is_some() {
            Some(DialogKind::CreateTable)
        } else if self.password_prompt.is_some() {
            Some(DialogKind::Password)
        } else if self.save_query_dialog.is_some() {
            Some(DialogKind::SaveQuery)
        } else if self.unsaved_confirm.is_some() {
            Some(DialogKind::Unsaved)
        } else if self.export_choice.is_some() {
            Some(DialogKind::ExportChoice)
        } else if self.new_key_dialog.is_some() {
            Some(DialogKind::NewKey)
        } else if self.key_ttl_dialog.is_some() {
            Some(DialogKind::KeyTtl)
        } else if self.error_dialog.is_some() {
            Some(DialogKind::Error)
        } else if self.delete_confirm.is_some() {
            Some(DialogKind::Confirm)
        } else {
            None
        };

        if desired == self.opened_dialog {
            return;
        }
        self.opened_dialog = desired;
        // A freshly-opened dialog gives its first field the focus once (see the flags' doc).
        self.dialog_focus_done = false;

        // Drop whatever was showing before switching to the new state.
        window.close_all_dialogs(cx);

        match desired {
            Some(DialogKind::DbDialog) => self.open_db_dialog(window, cx),
            Some(DialogKind::Schema) => self.open_schema_dialog(window, cx),
            Some(DialogKind::CreateTable) => self.open_create_table_modal(window, cx),
            Some(DialogKind::Password) => self.open_password_prompt(window, cx),
            Some(DialogKind::SaveQuery) => self.open_save_query_dialog(window, cx),
            Some(DialogKind::Error) => self.open_error_dialog(window, cx),
            Some(DialogKind::Confirm) => self.open_confirm_dialog(window, cx),
            Some(DialogKind::Unsaved) => self.open_unsaved_dialog(window, cx),
            Some(DialogKind::ExportChoice) => self.open_export_choice_dialog(window, cx),
            Some(DialogKind::NewKey) => self.open_new_key_dialog(window, cx),
            Some(DialogKind::KeyTtl) => self.open_key_ttl_dialog(window, cx),
            None => {}
        }
    }

    fn open_error_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let message = app.update(cx, |app, _| app.error_dialog.clone().unwrap_or_default());
            let copy_message = message.clone();
            let theme = app.read(cx).theme;
            let on_ok = app.downgrade();
            let on_close = app.downgrade();
            let footer_ok = app.downgrade();
            dialog
                .entrance(DIALOG_ENTRANCE)
                .title(t!("error.title").to_string())
                .centered()
                .content(move |content, _window, _cx| {
                    content.child(
                        div()
                            .id("error-scroll")
                            .max_h(px(320.0))
                            .overflow_y_scroll()
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
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(12.0))
                                    .cursor_text()
                                    // A SelectableText participates in the dialog's window
                                    // selection: the message can be dragged over and copied, not
                                    // only sent to the clipboard through 复制.
                                    .child(SelectableText::new("error-message", message.clone())),
                            ),
                    )
                })
                .footer(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .w_full()
                        .child(ui::button(
                            "error-copy",
                            t!("error.copy").to_string(),
                            ButtonKind::Normal,
                            theme,
                            move |_event, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    copy_message.clone(),
                                ));
                            },
                        ))
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
        window.open_dialog(cx, move |dialog, _window, cx| {
            let (title, message, confirm_label) = app.update(cx, |app, _| app.confirm_texts());
            let theme = app.read(cx).theme;
            let on_ok = app.downgrade();
            let on_cancel = app.downgrade();
            let on_close = app.downgrade();
            let footer_ok = app.downgrade();
            let footer_cancel = app.downgrade();
            let ok_label = confirm_label.clone();

            dialog
                .entrance(DIALOG_ENTRANCE)
                .title(title)
                .centered()
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

    /// The 导出 scope prompt: a radio list for the table scope, or a multi-select dropdown for
    /// query results, plus 取消 / 确定. It sits in front of the Export Wizard, so the user decides
    /// which rows or result sets to export before a file path is chosen.
    fn open_export_choice_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let theme = app.read(cx).theme;
            let on_ok = app.downgrade();
            let on_cancel = app.downgrade();
            let on_close = app.downgrade();
            let footer_ok = app.downgrade();
            let footer_cancel = app.downgrade();
            let content_app = app.clone();

            dialog
                .entrance(DIALOG_ENTRANCE)
                .title(t!("export.confirm_title").to_string())
                .centered()
                .content(move |content, _window, cx| {
                    let (message, options, selected, multiple, combo) =
                        content_app.update(cx, |app, _cx| {
                            let (message, options, selected, multiple) =
                                match app.export_choice.as_ref() {
                                    Some(choice) => (
                                        choice.message.clone(),
                                        choice.options.clone(),
                                        choice.selected.clone(),
                                        choice.multiple,
                                    ),
                                    None => (String::new(), Vec::new(), Vec::new(), false),
                                };
                            (
                                message,
                                options,
                                selected,
                                multiple,
                                app.export_choice_combo.clone(),
                            )
                        });
                    let all_selected =
                        !selected.is_empty() && selected.iter().all(|selected| *selected);
                    let mut body = div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_size(px(12.0)).child(message));
                    if multiple {
                        // A hundred result tabs must not stretch the dialog: the dropdown keeps
                        // one row of chrome and scrolls its own list.
                        if let Some(combo) = combo {
                            body = body.child(div().h(px(24.0)).w_full().child(combo));
                        }
                        let select_all = content_app.clone();
                        body = body.child(
                            div()
                                .id("export-choice-select-all")
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .h(px(22.0))
                                .cursor_pointer()
                                .on_click(move |_event, _window, cx| {
                                    select_all.update(cx, |app, cx| {
                                        app.export_choice_select_all(!all_selected, cx);
                                    });
                                })
                                .child(ui::checkbox_box(all_selected, theme))
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .child(t!("export.select_all").to_string()),
                                ),
                        );
                    } else {
                        let select = content_app.clone();
                        let mut list = div()
                            .id("export-choice-list")
                            .flex()
                            .flex_col()
                            .gap_1()
                            .max_h(px(300.0))
                            .overflow_y_scroll();
                        for (index, label) in options.iter().enumerate() {
                            let label = label.clone();
                            let select = select.clone();
                            list = list.child(
                                div()
                                    .id(SharedString::from(format!("export-choice-{index}")))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .h(px(22.0))
                                    .cursor_pointer()
                                    .on_click(move |_event, _window, cx| {
                                        select.update(cx, |app, cx| {
                                            app.export_choice_select(index, cx);
                                        });
                                    })
                                    .child(ui::radio_box(selected.get(index) == Some(&true), theme))
                                    .child(div().text_size(px(12.0)).child(label)),
                            );
                        }
                        body = body.child(list);
                    }
                    content.child(body)
                })
                .footer({
                    let can_ok = app
                        .read(cx)
                        .export_choice
                        .as_ref()
                        .is_some_and(|choice| choice.selected.iter().any(|selected| *selected));
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .w_full()
                        .child(ui::button(
                            "export-choice-cancel",
                            t!("form.cancel").to_string(),
                            ButtonKind::Normal,
                            theme,
                            move |_event, _window, cx| {
                                let _ = footer_cancel
                                    .update(cx, |app, cx| app.cancel_export_choice(cx));
                            },
                        ))
                        .child(ui::button(
                            "export-choice-ok",
                            t!("form.ok").to_string(),
                            if can_ok {
                                ButtonKind::Default
                            } else {
                                ButtonKind::Disabled
                            },
                            theme,
                            move |_event, _window, cx| {
                                let _ =
                                    footer_ok.update(cx, |app, cx| app.confirm_export_choice(cx));
                            },
                        ))
                })
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("form.ok").to_string())
                        .cancel_text(t!("form.cancel").to_string())
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let _ = on_ok.update(cx, |app, cx| app.confirm_export_choice(cx));
                            true
                        })
                        .on_cancel(move |_, _, cx| {
                            let _ = on_cancel.update(cx, |app, cx| app.cancel_export_choice(cx));
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.cancel_export_choice(cx));
                })
        });
    }

    /// Opens the "discard unsaved changes?" confirmation.
    fn open_unsaved_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let theme = app.read(cx).theme;
            let on_ok = app.downgrade();
            let on_cancel = app.downgrade();
            let on_close = app.downgrade();
            let footer_ok = app.downgrade();
            let footer_cancel = app.downgrade();
            dialog
                .entrance(DIALOG_ENTRANCE)
                .title(t!("unsaved.title").to_string())
                .centered()
                .content(|content, _window, _cx| content.child(t!("unsaved.message").to_string()))
                .footer(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .w_full()
                        .child(ui::button(
                            "unsaved-cancel",
                            t!("form.cancel").to_string(),
                            ButtonKind::Normal,
                            theme,
                            move |_event, _window, cx| {
                                let _ = footer_cancel.update(cx, |app, cx| app.cancel_unsaved(cx));
                            },
                        ))
                        .child(ui::button(
                            "unsaved-ok",
                            t!("unsaved.discard").to_string(),
                            ButtonKind::Danger,
                            theme,
                            move |_event, window, cx| {
                                let _ =
                                    footer_ok.update(cx, |app, cx| app.confirm_unsaved(window, cx));
                            },
                        )),
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("unsaved.discard").to_string())
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text(t!("form.cancel").to_string())
                        .show_cancel(true)
                        .on_ok(move |_, window, cx| {
                            let _ = on_ok.update(cx, |app, cx| app.confirm_unsaved(window, cx));
                            true
                        })
                        .on_cancel(move |_, _, cx| {
                            let _ = on_cancel.update(cx, |app, cx| app.cancel_unsaved(cx));
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.cancel_unsaved(cx));
                })
        });
    }

    /// Cancel the pending "discard unsaved changes?" confirmation.
    pub(super) fn cancel_unsaved(&mut self, cx: &mut Context<'_, Self>) {
        self.unsaved_confirm = None;
        cx.notify();
    }

    /// The user confirmed discarding unsaved changes: run the deferred action.
    pub(super) fn confirm_unsaved(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(action) = self.unsaved_confirm.take() else {
            return;
        };
        self.run_pending_action(action, window, cx);
        cx.notify();
    }

    /// Whether any open tab has unsaved work (query/design edits or grid pending rows).
    pub(super) fn has_unsaved_changes(&self, cx: &App) -> bool {
        self.queries.iter().any(|tab| tab.is_dirty())
            || self.designs.iter().any(|design| design.read(cx).dirty)
            || self.grids.iter().any(|grid| {
                let grid = grid.read(cx);
                !grid.state.edits.is_empty() || !grid.inserts.is_empty()
            })
    }

    /// Whether one connection has unsaved work (its grids/designers or bound query tabs).
    pub(super) fn connection_has_unsaved(&self, index: usize, cx: &App) -> bool {
        let Some(connection) = self.connection_arc(index) else {
            return false;
        };
        self.grids.iter().any(|grid| {
            let grid = grid.read(cx);
            Arc::ptr_eq(&grid.state.connection, &connection)
                && (!grid.state.edits.is_empty() || !grid.inserts.is_empty())
        }) || self.designs.iter().any(|design| {
            let design = design.read(cx);
            Arc::ptr_eq(&design.connection, &connection) && design.dirty
        }) || self
            .queries
            .iter()
            .any(|tab| tab.connection_index == Some(index) && tab.is_dirty())
    }

    /// The main window's close guard, shared by the OS close path (`WM_CLOSE` / the macOS close
    /// button) and the Linux title-bar button. Returns whether the window may close; unsaved work
    /// raises the confirmation and keeps the window open.
    pub(crate) fn should_close_main_window(&mut self, cx: &mut Context<'_, Self>) -> bool {
        if self.has_unsaved_changes(cx) {
            self.unsaved_confirm = Some(PendingAction::Quit);
            cx.notify();
            return false;
        }
        true
    }

    /// Ask before closing the main window when there is unsaved work.
    pub(super) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.should_close_main_window(cx) {
            window.remove_window();
        }
    }

    /// Run the action a pending confirmation was guarding.
    fn run_pending_action(
        &mut self,
        action: PendingAction,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match action {
            PendingAction::Quit => window.remove_window(),
            PendingAction::Disconnect { index } => self.disconnect(index, cx),
            PendingAction::DisconnectMany { indexes } => self.disconnect_many(indexes, cx),
            PendingAction::CloseQuery { index } => self.close_query_now(index, cx),
            PendingAction::CloseDesign { index } => self.close_design_now(index, cx),
            PendingAction::CloseAllTabs => self.close_all_tabs_now(cx),
            PendingAction::CloseOtherTabs { keep } => self.close_other_tabs_now(keep, cx),
            PendingAction::GridDiscard { grid_id, nav } => {
                let target = self
                    .grids
                    .iter()
                    .find(|grid| grid.read(cx).state.id == grid_id)
                    .cloned();
                if let Some(grid) = target {
                    grid.update(cx, |grid, cx| grid.run_grid_nav(nav, cx));
                }
            }
        }
    }

    /// Hand queued notifications to `Root`. Called every render, which owns the window.
    pub(super) fn flush_toasts(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.pending_toasts.is_empty() {
            return;
        }
        for (kind, message) in self.pending_toasts.drain(..) {
            let notification = match kind {
                ToastKind::Success => {
                    gpui_kit::component::notification::Notification::success(message)
                }
                ToastKind::Info => gpui_kit::component::notification::Notification::info(message),
                ToastKind::Error => gpui_kit::component::notification::Notification::error(message),
            };
            window.push_notification(notification, cx);
        }
    }

    /// Queue a transient notification and schedule a frame to deliver it.
    pub(super) fn toast(
        &mut self,
        kind: ToastKind,
        message: impl Into<String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.pending_toasts.push((kind, message.into()));
        cx.notify();
    }

    /// Record a failed persistence, so it is not silently dropped.
    pub(super) fn warn_persist(
        &mut self,
        what: &str,
        error: impl std::fmt::Display,
        cx: &mut Context<'_, Self>,
    ) {
        let message = t!(
            "toast.persist_failed",
            what = what,
            error = error.to_string()
        )
        .to_string();
        self.toast(ToastKind::Error, message, cx);
    }

    fn open_password_prompt(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
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
            let content_app = app.clone();

            dialog
                .entrance(DIALOG_ENTRANCE)
                .title(title)
                .centered()
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
                    let focus_now = content_app
                        .update(cx, |app, _cx| take_dialog_focus(&mut app.dialog_focus_done));
                    if focus_now {
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

    /// Opens the "save query" dialog as a `Root`-managed modal. The dialog body reads its state
    /// back from `AppView` each frame, so the name field and location picker stay live.
    fn open_save_query_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let (title, footer) = app.update(cx, |app, cx| {
                (
                    t!("query.save_title").to_string(),
                    app.save_query_dialog_footer(cx).into_any_element(),
                )
            });
            let on_close = app.downgrade();
            let ok_app = app.downgrade();
            let cancel_app = app.downgrade();
            let content_app = app.clone();
            dialog
                .entrance(DIALOG_ENTRANCE)
                .title(title)
                .w(px(460.0))
                .centered()
                .content(move |content, window, cx| {
                    let body = content_app.update(cx, |app, cx| {
                        app.save_query_dialog_body(cx).into_any_element()
                    });
                    if content_app
                        .update(cx, |app, _cx| take_dialog_focus(&mut app.dialog_focus_done))
                        && let Some(input) = content_app.read(cx).query_name_input.clone()
                    {
                        input.update(cx, |input, cx| input.focus_state(window, cx));
                    }
                    content.child(body)
                })
                .footer(footer)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("form.ok").to_string())
                        .cancel_text(t!("form.cancel").to_string())
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let _ = ok_app.update(cx, |app, cx| app.submit_save_query(cx));
                            false
                        })
                        .on_cancel(move |_, _, cx| {
                            let _ = cancel_app.update(cx, |app, cx| app.cancel_save_query(cx));
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.cancel_save_query(cx));
                })
        });
    }

    fn save_query_dialog_body(&self, _cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.save_query_dialog.as_ref() else {
            return div().into_any_element();
        };

        let mut name_row = div().flex().flex_col().gap_1().child(
            div()
                .text_size(px(12.0))
                .child(t!("query.enter_name").to_string()),
        );
        if let Some(input) = self.query_name_input.as_ref() {
            name_row = name_row.child(div().w_full().h(px(26.0)).child(input.clone()));
        }

        let mut connection = div().flex().flex_col().gap_1().child(
            div()
                .text_size(px(12.0))
                .child(t!("info.connection").to_string()),
        );
        if let Some(combo) = self.save_connection_combo.as_ref() {
            connection = connection.child(div().w_full().child(combo.clone()));
        }
        let mut schema = div().flex().flex_col().gap_1().child(
            div()
                .text_size(px(12.0))
                .child(t!("query.schema").to_string()),
        );
        if let Some(combo) = self.save_database_combo.as_ref() {
            schema = schema.child(div().w_full().child(combo.clone()));
        }

        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(name_row)
            .child(connection)
            .child(schema);
        if let Some(error) = dialog.error.as_ref() {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.danger))
                    .child(error.clone()),
            );
        }
        body.into_any_element()
    }

    fn save_query_dialog_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .w_full()
            .gap_2()
            .h(px(46.0))
            .child(self.dialog_button(
                "save-query-cancel",
                t!("form.cancel").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.cancel_save_query(cx)),
            ))
            .child(self.dialog_button(
                "save-query-ok",
                t!("form.ok").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| this.submit_save_query(cx)),
            ))
    }

    /// Opens the "new table" name prompt for a designer. `initial` pre-fills the field (e.g. after
    /// a failed create so the retry keeps the name).
    pub(super) fn open_create_table_dialog(
        &mut self,
        design_id: u64,
        initial: String,
        cx: &mut Context<'_, Self>,
    ) {
        let weak = cx.weak_entity();
        let input = make_create_table_input(self.theme, initial.clone(), &weak, cx);
        self.create_table_input = Some(input);
        self.create_table_dialog = Some(CreateTableDialog {
            design_id,
            name: initial,
            error: None,
        });
        cx.notify();
    }

    /// Set up the Redis 新建键 modal and build its fields. `group` pre-fills the name field when
    /// the dialog was opened from a namespace folder.
    pub(super) fn start_new_key_dialog(
        &mut self,
        connection_index: usize,
        database_index: usize,
        group: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let theme = self.theme;
        let weak = cx.weak_entity();
        let name = group.unwrap_or_default();
        self.new_key_dialog = Some(NewKeyDialog {
            connection_index,
            database_index,
            name: name.clone(),
            kind: "string".to_string(),
            ttl: String::new(),
            value: String::new(),
            field: String::new(),
            score: String::new(),
            error: None,
            saving: false,
        });

        let name_weak = weak.clone();
        let submit_weak = weak.clone();
        let cancel_weak = weak.clone();
        self.new_key_name_input = Some(cx.new(|cx| {
            TextInput::new(theme, name, TextInputOptions::default(), cx)
                .on_change(Rc::new(move |text, _window, cx| {
                    let _ = name_weak.update(cx, |app, cx| {
                        if let Some(dialog) = app.new_key_dialog.as_mut() {
                            dialog.name = text.to_string();
                            dialog.error = None;
                        }
                        cx.notify();
                    });
                }))
                .on_submit(Rc::new(move |_window, cx| {
                    let _ = submit_weak.update(cx, |app, cx| app.submit_new_key(cx));
                }))
                .on_cancel(Rc::new(move |_window, cx| {
                    let _ = cancel_weak.update(cx, |app, cx| app.cancel_new_key(cx));
                }))
        }));

        let ttl_weak = weak.clone();
        self.new_key_ttl_input = Some(cx.new(|cx| {
            TextInput::new(
                theme,
                "",
                TextInputOptions {
                    placeholder: "-1".into(),
                    accepts: Some(Rc::new(|ch: char| ch.is_ascii_digit() || ch == '-')),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = ttl_weak.update(cx, |app, cx| {
                    if let Some(dialog) = app.new_key_dialog.as_mut() {
                        dialog.ttl = text.to_string();
                        dialog.error = None;
                    }
                    cx.notify();
                });
            }))
        }));

        let kinds = ["string", "hash", "list", "set", "zset", "stream"];
        let options: Vec<ComboOption> =
            kinds.iter().map(|kind| ComboOption::plain(*kind)).collect();
        let type_weak = weak.clone();
        self.new_key_type_combo = Some(cx.new(|cx| {
            ComboBox::new(theme, options, "string".to_string(), 180.0, cx)
                .field_width(180.0)
                .on_select(Rc::new(move |value, _window, cx| {
                    let _ = type_weak.update(cx, |app, cx| {
                        if let Some(dialog) = app.new_key_dialog.as_mut() {
                            dialog.kind = value.to_string();
                            dialog.error = None;
                        }
                        cx.notify();
                    });
                }))
        }));

        let value_weak = weak.clone();
        let value_submit = weak.clone();
        let value_cancel = weak.clone();
        self.new_key_value_input = Some(cx.new(|cx| {
            TextInput::new(theme, "", TextInputOptions::default(), cx)
                .on_change(Rc::new(move |text, _window, cx| {
                    let _ = value_weak.update(cx, |app, cx| {
                        if let Some(dialog) = app.new_key_dialog.as_mut() {
                            dialog.value = text.to_string();
                            dialog.error = None;
                        }
                        cx.notify();
                    });
                }))
                .on_submit(Rc::new(move |_window, cx| {
                    let _ = value_submit.update(cx, |app, cx| app.submit_new_key(cx));
                }))
                .on_cancel(Rc::new(move |_window, cx| {
                    let _ = value_cancel.update(cx, |app, cx| app.cancel_new_key(cx));
                }))
        }));

        let field_weak = weak.clone();
        self.new_key_field_input = Some(cx.new(|cx| {
            TextInput::new(theme, "", TextInputOptions::default(), cx).on_change(Rc::new(
                move |text, _window, cx| {
                    let _ = field_weak.update(cx, |app, cx| {
                        if let Some(dialog) = app.new_key_dialog.as_mut() {
                            dialog.field = text.to_string();
                            dialog.error = None;
                        }
                        cx.notify();
                    });
                },
            ))
        }));

        let score_weak = weak.clone();
        let score_submit = weak.clone();
        self.new_key_score_input = Some(cx.new(|cx| {
            TextInput::new(
                theme,
                "",
                TextInputOptions {
                    placeholder: "1".into(),
                    accepts: Some(Rc::new(|ch: char| {
                        ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.')
                    })),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = score_weak.update(cx, |app, cx| {
                    if let Some(dialog) = app.new_key_dialog.as_mut() {
                        dialog.score = text.to_string();
                        dialog.error = None;
                    }
                    cx.notify();
                });
            }))
            .on_submit(Rc::new(move |_window, cx| {
                let _ = score_submit.update(cx, |app, cx| app.submit_new_key(cx));
            }))
        }));
        cx.notify();
    }

    fn open_new_key_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let (title, footer) = app.update(cx, |app, cx| {
                (
                    t!("redis.key.new").to_string(),
                    app.new_key_dialog_footer(cx).into_any_element(),
                )
            });
            let content_app = app.clone();
            let ok_app = app.downgrade();
            let cancel_app = app.downgrade();
            let on_close = app.downgrade();
            dialog
                .entrance(DIALOG_ENTRANCE)
                .title(title)
                .w(px(460.0))
                .centered()
                .content(move |content, window, cx| {
                    let body = content_app
                        .update(cx, |app, cx| app.new_key_dialog_body(cx).into_any_element());
                    if content_app
                        .update(cx, |app, _cx| take_dialog_focus(&mut app.dialog_focus_done))
                        && let Some(input) = content_app.read(cx).new_key_name_input.clone()
                    {
                        input.update(cx, |input, cx| input.focus_state(window, cx));
                    }
                    content.child(body)
                })
                .footer(footer)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("redis.key.dialog.create").to_string())
                        .cancel_text(t!("form.cancel").to_string())
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let _ = ok_app.update(cx, |app, cx| app.submit_new_key(cx));
                            false
                        })
                        .on_cancel(move |_, _, cx| {
                            let _ = cancel_app.update(cx, |app, cx| app.cancel_new_key(cx));
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.cancel_new_key(cx));
                })
        });
    }

    fn new_key_dialog_body(&self, _cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.new_key_dialog.as_ref() else {
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
        let mut body = div().flex().flex_col().gap_3().p_4();
        body = body.child(field(
            t!("redis.key.dialog.name").to_string(),
            self.new_key_name_input
                .clone()
                .map(|input| div().h(px(26.0)).child(input).into_any_element())
                .unwrap_or_else(|| div().into_any_element()),
        ));
        body = body.child(
            div()
                .flex()
                .flex_row()
                .gap_3()
                .child(
                    div().flex_1().min_w(px(0.0)).child(field(
                        t!("redis.key.dialog.type").to_string(),
                        self.new_key_type_combo
                            .clone()
                            .map(|combo| combo.into_any_element())
                            .unwrap_or_else(|| div().into_any_element()),
                    )),
                )
                .child(
                    div().flex_1().min_w(px(0.0)).child(field(
                        t!("redis.key.dialog.ttl").to_string(),
                        self.new_key_ttl_input
                            .clone()
                            .map(|input| div().h(px(26.0)).child(input).into_any_element())
                            .unwrap_or_else(|| div().into_any_element()),
                    )),
                ),
        );
        // Which initial-entry fields a type takes: a hash/stream entry is a field plus a value, a
        // sorted set's entry is a member plus a score, a set gets one member and every other type
        // one value. All are plain single-line fields (Redis values are arbitrary text, so nothing
        // is pre-formatted).
        let text_input = |input: Option<Entity<TextInput>>| {
            input
                .map(|input| div().h(px(26.0)).child(input).into_any_element())
                .unwrap_or_else(|| div().into_any_element())
        };
        let pair =
            |left_label: String, left: AnyElement, right_label: String, right: AnyElement| {
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(div().flex_1().min_w(px(0.0)).child(field(left_label, left)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(field(right_label, right)),
                    )
                    .into_any_element()
            };
        body = body.child(match dialog.kind.as_str() {
            "hash" | "stream" => pair(
                t!("redis.key.field").to_string(),
                text_input(self.new_key_field_input.clone()),
                t!("redis.key.value").to_string(),
                text_input(self.new_key_value_input.clone()),
            ),
            "zset" => pair(
                t!("redis.key.member").to_string(),
                text_input(self.new_key_field_input.clone()),
                t!("redis.key.score").to_string(),
                text_input(self.new_key_score_input.clone()),
            ),
            "set" => field(
                t!("redis.key.member").to_string(),
                text_input(self.new_key_value_input.clone()),
            ),
            _ => field(
                t!("redis.key.dialog.value").to_string(),
                text_input(self.new_key_value_input.clone()),
            ),
        });
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

    fn new_key_dialog_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .w_full()
            .gap_2()
            .h(px(46.0))
            .child(self.dialog_button(
                "new-key-cancel",
                t!("form.cancel").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.cancel_new_key(cx)),
            ))
            .child(self.dialog_button(
                "new-key-create",
                t!("redis.key.dialog.create").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| this.submit_new_key(cx)),
            ))
    }

    pub(super) fn cancel_new_key(&mut self, cx: &mut Context<'_, Self>) {
        self.new_key_dialog = None;
        self.new_key_name_input = None;
        self.new_key_ttl_input = None;
        self.new_key_type_combo = None;
        self.new_key_value_input = None;
        self.new_key_field_input = None;
        self.new_key_score_input = None;
        cx.notify();
    }

    pub(super) fn submit_new_key(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.new_key_dialog.as_ref() else {
            return;
        };
        if dialog.saving {
            return;
        }
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            return;
        };
        let Some(database) = self.database_name(dialog.connection_index, dialog.database_index)
        else {
            return;
        };
        let name = dialog.name.trim().to_string();
        if name.is_empty() {
            self.new_key_dialog.as_mut().unwrap().error =
                Some(t!("redis.key.name_required").to_string());
            cx.notify();
            return;
        }
        let ttl = match parse_redis_ttl(&dialog.ttl) {
            Ok(ttl) => ttl,
            Err(error) => {
                self.new_key_dialog.as_mut().unwrap().error = Some(error);
                cx.notify();
                return;
            }
        };
        let kind = dialog.kind.clone();
        let value = dialog.value.clone();
        let field = dialog.field.trim().to_string();
        let score = dialog.score.trim().to_string();
        let values = match kind.as_str() {
            "string" => vec![("value".to_string(), Some(value))],
            "hash" => vec![
                ("field".to_string(), Some(non_empty(field, "field"))),
                ("value".to_string(), Some(value)),
            ],
            "list" | "set" => vec![("value".to_string(), Some(value))],
            "zset" => vec![
                ("member".to_string(), Some(non_empty(field, "item"))),
                ("score".to_string(), Some(non_empty(score, "1"))),
            ],
            "stream" => vec![
                ("field".to_string(), Some(non_empty(field, "field"))),
                ("value".to_string(), Some(value)),
            ],
            _ => Vec::new(),
        };
        let spec = rustgrid_core::ObjectCreate { kind, ttl, values };
        if let Some(dialog) = self.new_key_dialog.as_mut() {
            dialog.saving = true;
            dialog.error = None;
        }
        cx.notify();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.create_object(&database, &name, &spec).await })
                .await;
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.new_key_dialog.as_mut() else {
                    return;
                };
                dialog.saving = false;
                match result {
                    Ok(Ok(())) => {
                        let connection_index = dialog.connection_index;
                        let database_index = dialog.database_index;
                        app.new_key_dialog = None;
                        app.new_key_name_input = None;
                        app.new_key_ttl_input = None;
                        app.new_key_type_combo = None;
                        app.new_key_value_input = None;
                        app.new_key_field_input = None;
                        app.new_key_score_input = None;
                        app.reload_tables(connection_index, database_index, cx);
                        app.toast(ToastKind::Success, t!("redis.key.created").to_string(), cx);
                    }
                    Ok(Err(error)) => dialog.error = Some(error.to_string()),
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Set up the Redis 设置 TTL modal for `key`.
    pub(super) fn start_key_ttl_dialog(
        &mut self,
        connection_index: usize,
        database_index: usize,
        key: String,
        cx: &mut Context<'_, Self>,
    ) {
        let theme = self.theme;
        let current = self
            .connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => databases.get(database_index),
                _ => None,
            })
            .and_then(|database| match &database.table_statuses {
                Loadable::Loaded(statuses) => statuses
                    .iter()
                    .find(|(name, _)| name == &key)
                    .and_then(|(_, status)| status.ttl),
                _ => None,
            })
            .unwrap_or(-1);
        let initial = current.to_string();
        self.key_ttl_dialog = Some(KeyTtlDialog {
            connection_index,
            database_index,
            key,
            ttl: initial.clone(),
            error: None,
            saving: false,
        });
        let weak = cx.weak_entity();
        let change = weak.clone();
        let submit = weak.clone();
        let cancel = weak.clone();
        self.key_ttl_input = Some(cx.new(|cx| {
            TextInput::new(
                theme,
                initial,
                TextInputOptions {
                    accepts: Some(Rc::new(|ch: char| ch.is_ascii_digit() || ch == '-')),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = change.update(cx, |app, cx| {
                    if let Some(dialog) = app.key_ttl_dialog.as_mut() {
                        dialog.ttl = text.to_string();
                        dialog.error = None;
                    }
                    cx.notify();
                });
            }))
            .on_submit(Rc::new(move |_window, cx| {
                let _ = submit.update(cx, |app, cx| app.submit_key_ttl(cx));
            }))
            .on_cancel(Rc::new(move |_window, cx| {
                let _ = cancel.update(cx, |app, cx| app.cancel_key_ttl(cx));
            }))
        }));
        cx.notify();
    }

    fn open_key_ttl_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let (title, footer) = app.update(cx, |app, cx| {
                (
                    t!("redis.key.dialog.ttl_title").to_string(),
                    app.key_ttl_dialog_footer(cx).into_any_element(),
                )
            });
            let content_app = app.clone();
            let ok_app = app.downgrade();
            let cancel_app = app.downgrade();
            let on_close = app.downgrade();
            dialog
                .entrance(DIALOG_ENTRANCE)
                .title(title)
                .w(px(340.0))
                .centered()
                .content(move |content, window, cx| {
                    let body = content_app
                        .update(cx, |app, cx| app.key_ttl_dialog_body(cx).into_any_element());
                    if content_app
                        .update(cx, |app, _cx| take_dialog_focus(&mut app.dialog_focus_done))
                        && let Some(input) = content_app.read(cx).key_ttl_input.clone()
                    {
                        input.update(cx, |input, cx| input.focus_state(window, cx));
                    }
                    content.child(body)
                })
                .footer(footer)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("redis.key.dialog.apply").to_string())
                        .cancel_text(t!("form.cancel").to_string())
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let _ = ok_app.update(cx, |app, cx| app.submit_key_ttl(cx));
                            false
                        })
                        .on_cancel(move |_, _, cx| {
                            let _ = cancel_app.update(cx, |app, cx| app.cancel_key_ttl(cx));
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.cancel_key_ttl(cx));
                })
        });
    }

    fn key_ttl_dialog_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.key_ttl_dialog.as_ref() else {
            return div().into_any_element();
        };
        let mut body = div().flex().flex_col().gap_3().p_4();
        body = body.child(
            div()
                .text_size(px(12.0))
                .font_family("Consolas")
                .text_color(rgb(theme.text_muted))
                .child(dialog.key.clone()),
        );
        if let Some(input) = self.key_ttl_input.as_ref() {
            body = body.child(div().w_full().h(px(26.0)).child(input.clone()));
        }
        let quick = |label: String, value: &'static str| {
            let weak = cx.weak_entity();
            div()
                .id(SharedString::from(format!("key-ttl-quick-{value}")))
                .px_2()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(11.5))
                .bg(rgb(theme.button_bg))
                .border_1()
                .border_color(rgb(theme.button_border))
                .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                .on_click(move |_event, _window, cx| {
                    let _ = weak.update(cx, |app, cx| {
                        if let Some(dialog) = app.key_ttl_dialog.as_mut() {
                            dialog.ttl = value.to_string();
                        }
                        if let Some(input) = app.key_ttl_input.clone() {
                            input.update(cx, |input, cx| input.set_text(value, cx));
                        }
                        cx.notify();
                    });
                })
                .child(label)
        };
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(quick(t!("redis.key.dialog.forever").to_string(), "-1"))
                .child(quick(t!("redis.key.dialog.minute").to_string(), "60"))
                .child(quick(t!("redis.key.dialog.hour").to_string(), "3600")),
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

    fn key_ttl_dialog_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .w_full()
            .gap_2()
            .h(px(46.0))
            .child(self.dialog_button(
                "key-ttl-cancel",
                t!("form.cancel").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.cancel_key_ttl(cx)),
            ))
            .child(self.dialog_button(
                "key-ttl-apply",
                t!("redis.key.dialog.apply").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| this.submit_key_ttl(cx)),
            ))
    }

    pub(super) fn cancel_key_ttl(&mut self, cx: &mut Context<'_, Self>) {
        self.key_ttl_dialog = None;
        self.key_ttl_input = None;
        cx.notify();
    }

    pub(super) fn submit_key_ttl(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.key_ttl_dialog.as_ref() else {
            return;
        };
        if dialog.saving {
            return;
        }
        let Some(connection) = self.connection_arc(dialog.connection_index) else {
            return;
        };
        let Some(database) = self.database_name(dialog.connection_index, dialog.database_index)
        else {
            return;
        };
        let ttl = match parse_redis_ttl(&dialog.ttl) {
            Ok(ttl) => ttl,
            Err(error) => {
                self.key_ttl_dialog.as_mut().unwrap().error = Some(error);
                cx.notify();
                return;
            }
        };
        let key = dialog.key.clone();
        let connection_index = dialog.connection_index;
        let database_index = dialog.database_index;
        let status_database = database.clone();
        if let Some(dialog) = self.key_ttl_dialog.as_mut() {
            dialog.saving = true;
            dialog.error = None;
        }
        cx.notify();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move { connection.set_object_ttl(&database, &key, ttl).await })
                .await;
            let _ = this.update(cx, |app, cx| {
                let Some(dialog) = app.key_ttl_dialog.as_mut() else {
                    return;
                };
                dialog.saving = false;
                match result {
                    Ok(Ok(())) => {
                        let key = dialog.key.clone();
                        app.key_ttl_dialog = None;
                        app.key_ttl_input = None;
                        app.reload_tables(connection_index, database_index, cx);
                        // Keep an open key page's header in step with the new expiry.
                        for grid in &app.grids {
                            let matches = {
                                let grid = grid.read(cx);
                                grid.state.database == status_database && grid.state.table == key
                            };
                            if matches {
                                grid.update(cx, |grid, cx| {
                                    if let Some(status) = grid.state.key_status.as_mut() {
                                        status.ttl = Some(ttl.unwrap_or(-1));
                                    }
                                    cx.notify();
                                });
                            }
                        }
                        app.toast(
                            ToastKind::Success,
                            t!("redis.key.ttl_updated").to_string(),
                            cx,
                        );
                    }
                    Ok(Err(error)) => dialog.error = Some(error.to_string()),
                    Err(error) => dialog.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_create_table_modal(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let (title, footer) = app.update(cx, |app, cx| {
                (
                    t!("object.new_table").to_string(),
                    app.create_table_dialog_footer(cx).into_any_element(),
                )
            });
            let on_close = app.downgrade();
            let ok_app = app.downgrade();
            let cancel_app = app.downgrade();
            let content_app = app.clone();
            dialog
                .entrance(DIALOG_ENTRANCE)
                .title(title)
                .w(px(420.0))
                .centered()
                .content(move |content, window, cx| {
                    let body = content_app.update(cx, |app, cx| {
                        app.create_table_dialog_body(cx).into_any_element()
                    });
                    if content_app
                        .update(cx, |app, _cx| take_dialog_focus(&mut app.dialog_focus_done))
                        && let Some(input) = content_app.read(cx).create_table_input.clone()
                    {
                        input.update(cx, |input, cx| input.focus_state(window, cx));
                    }
                    content.child(body)
                })
                .footer(footer)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t!("form.ok").to_string())
                        .cancel_text(t!("form.cancel").to_string())
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let _ = ok_app.update(cx, |app, cx| app.submit_create_table(cx));
                            false
                        })
                        .on_cancel(move |_, _, cx| {
                            let _ = cancel_app.update(cx, |app, cx| app.cancel_create_table(cx));
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.cancel_create_table(cx));
                })
        });
    }

    fn create_table_dialog_body(&self, _cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(dialog) = self.create_table_dialog.as_ref() else {
            return div().into_any_element();
        };
        let mut body = div().flex().flex_col().gap_2().p_4().child(
            div()
                .text_size(px(12.0))
                .child(t!("design.enter_table_name").to_string()),
        );
        if let Some(input) = self.create_table_input.as_ref() {
            body = body.child(div().w_full().h(px(26.0)).child(input.clone()));
        }
        if let Some(error) = dialog.error.as_ref() {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.danger))
                    .child(error.clone()),
            );
        }
        body.into_any_element()
    }

    fn create_table_dialog_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .w_full()
            .gap_2()
            .h(px(46.0))
            .child(self.dialog_button(
                "create-table-cancel",
                t!("form.cancel").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.cancel_create_table(cx)),
            ))
            .child(self.dialog_button(
                "create-table-ok",
                t!("form.ok").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| this.submit_create_table(cx)),
            ))
    }

    pub(super) fn cancel_create_table(&mut self, cx: &mut Context<'_, Self>) {
        self.create_table_dialog = None;
        self.create_table_input = None;
        cx.notify();
    }

    /// Validate the entered name and hand it to the requesting designer to actually create it.
    pub(super) fn submit_create_table(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.create_table_dialog.take() else {
            return;
        };
        let name = dialog.name.trim().to_string();
        if !is_valid_identifier(&name) {
            self.create_table_dialog = Some(CreateTableDialog {
                design_id: dialog.design_id,
                name,
                error: Some(t!("database.invalid_name").to_string()),
            });
            cx.notify();
            return;
        }

        self.create_table_input = None;
        if let Some(design) = self
            .designs
            .iter()
            .find(|design| design.read(cx).id == dialog.design_id)
            .cloned()
        {
            design.update(cx, |design, cx| design.save_new(name, cx));
        }
        cx.notify();
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
        // The list shows `dbo.users` as `users`, so the editor must start from that same bare
        // name; the internal qualified identity stays in `old_name` to find the row and the table.
        let initial = object_display(&old_name);
        let input_initial = initial.clone();
        let input = cx.new(move |cx| {
            TextInput::new(
                theme,
                input_initial,
                TextInputOptions {
                    bare: true,
                    text_size: Some(12.0),
                    // 20px, matching the object row so the framed editor never outgrows it.
                    size: Some(gpui_kit::component::Size::XSmall),
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
            new_name: initial,
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
        // The editor edits the bare name (the schema is implied by the selected schema node), so
        // the comparison and the driver call both use that form; a pasted `schema.name` is reduced
        // to its bare part too, exactly as the list displays it.
        let new_name = object_display(edit.new_name.trim());
        if new_name.is_empty() || new_name == object_display(&edit.old_name) {
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
            Some(DeleteConfirm::Group { id }) => {
                let name = self
                    .group(*id)
                    .map(|group| group.name.clone())
                    .unwrap_or_default();
                (
                    t!("group.delete_title").to_string(),
                    t!("group.delete_confirm", name = name).to_string(),
                    t!("group.delete_button").to_string(),
                )
            }
            Some(DeleteConfirm::TreeSelection {
                connections,
                groups,
            }) => {
                // A group is removed with everything inside it, so the count includes its
                // connections on top of the ones directly selected.
                let mut count = connections.len() + groups.len();
                for id in groups {
                    let mut ids = BTreeSet::new();
                    groups::collect_subtree_connection_ids(&self.tree_layout, *id, &mut ids);
                    count += ids.len();
                }
                (
                    t!("connection.delete_title").to_string(),
                    t!("connection.delete_multi_confirm", count = count).to_string(),
                    t!("connection.delete_multi_button", count = count).to_string(),
                )
            }
            Some(DeleteConfirm::Table {
                name, operation, ..
            }) => {
                let (title_key, message_key, button_key) = match operation {
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
            Some(DeleteConfirm::Tables {
                names,
                connection_index,
                ..
            }) => {
                let count = names.len();
                if self.driver_supports(*connection_index, DriverCapability::KeyValue) {
                    (
                        t!("redis.key.delete_title").to_string(),
                        t!("redis.key.delete_confirm", count = count).to_string(),
                        t!("redis.key.delete_button", count = count).to_string(),
                    )
                } else {
                    (
                        t!("object.delete_tables_title").to_string(),
                        t!("object.drop_tables_confirm", count = count).to_string(),
                        t!("object.drop_tables_button", count = count).to_string(),
                    )
                }
            }
            Some(DeleteConfirm::TablesOperation {
                names, operation, ..
            }) => {
                let count = names.len();
                let (title, message, button) = match operation {
                    TableOperation::Empty => (
                        "object.empty_table",
                        "object.empty_tables_confirm",
                        "object.empty_tables_button",
                    ),
                    TableOperation::Truncate => (
                        "object.truncate_table",
                        "object.truncate_tables_confirm",
                        "object.truncate_tables_button",
                    ),
                };
                (
                    t!(title).to_string(),
                    t!(message, count = count).to_string(),
                    t!(button, count = count).to_string(),
                )
            }
            Some(DeleteConfirm::SavedQuery { index }) => {
                let name = self
                    .query_files
                    .get(*index)
                    .map(|file| file.name.clone())
                    .unwrap_or_default();
                (
                    t!("query.delete_title").to_string(),
                    t!("query.delete_confirm", name = name).to_string(),
                    t!("query.delete_button").to_string(),
                )
            }
            Some(DeleteConfirm::SavedQueries { indices }) => {
                let count = indices.len();
                (
                    t!("query.delete_title").to_string(),
                    t!("query.delete_multi_confirm", count = count).to_string(),
                    t!("query.delete_multi_button", count = count).to_string(),
                )
            }
            Some(DeleteConfirm::BackupEntries { files, configs }) => {
                let count = files.len() + configs.len();
                (
                    t!("backup.delete_title").to_string(),
                    t!("backup.delete_multi_confirm", count = count).to_string(),
                    t!("backup.delete_multi_button", count = count).to_string(),
                )
            }
            Some(DeleteConfirm::BackupFile { index }) => {
                let name = self
                    .backup_files
                    .get(*index)
                    .map(|file| file.name.clone())
                    .unwrap_or_default();
                (
                    t!("backup.delete_title").to_string(),
                    t!("backup.delete_file_confirm", name = name).to_string(),
                    t!("backup.delete_button").to_string(),
                )
            }
            Some(DeleteConfirm::BackupConfig { index }) => {
                let name = self
                    .backup_configs
                    .get(*index)
                    .map(|config| config.name.clone())
                    .unwrap_or_default();
                (
                    t!("backup.delete_title").to_string(),
                    t!("backup.delete_config_confirm", name = name).to_string(),
                    t!("backup.delete_button").to_string(),
                )
            }
            Some(DeleteConfirm::TransferConfigs { indices }) => {
                if indices.len() == 1 {
                    let name = self
                        .transfer_configs
                        .get(indices[0])
                        .map(|config| config.name.clone())
                        .unwrap_or_default();
                    (
                        t!("sync.config.delete_title").to_string(),
                        t!("sync.config.delete_confirm", name = name).to_string(),
                        t!("sync.config.delete_button").to_string(),
                    )
                } else {
                    let count = indices.len();
                    (
                        t!("sync.config.delete_title").to_string(),
                        t!("sync.config.delete_multi_confirm", count = count).to_string(),
                        t!("sync.config.delete_button").to_string(),
                    )
                }
            }
            Some(DeleteConfirm::DesignRows { kind, .. }) => {
                let (message_key, button_key) = match kind {
                    DesignDeleteKind::Fields => {
                        ("design.confirm_delete_field", "design.delete_field")
                    }
                    DesignDeleteKind::Indexes => {
                        ("design.confirm_delete_index", "design.delete_index")
                    }
                    DesignDeleteKind::ForeignKeys => (
                        "design.confirm_delete_foreign_key",
                        "design.delete_foreign_key",
                    ),
                    DesignDeleteKind::UniqueKeys => (
                        "design.confirm_delete_unique_key",
                        "design.delete_unique_key",
                    ),
                    DesignDeleteKind::Checks => {
                        ("design.confirm_delete_check", "design.delete_check")
                    }
                    DesignDeleteKind::Excludes => {
                        ("design.confirm_delete_exclude", "design.delete_exclude")
                    }
                    DesignDeleteKind::Rules => ("design.confirm_delete_rule", "design.delete_rule"),
                    DesignDeleteKind::Triggers => {
                        ("design.confirm_delete_trigger", "design.trigger.delete")
                    }
                };
                (
                    t!("design.delete_title").to_string(),
                    t!(message_key).to_string(),
                    t!(button_key).to_string(),
                )
            }
            Some(DeleteConfirm::User { label, role, .. }) => (
                t!("user.delete_title").to_string(),
                if *role {
                    t!("user.delete_role_confirm", name = label.clone()).to_string()
                } else {
                    t!("user.delete_confirm", name = label.clone()).to_string()
                },
                if *role {
                    t!("user.delete_role_button").to_string()
                } else {
                    t!("user.delete_button").to_string()
                },
            ),
            Some(DeleteConfirm::Routine { label, .. }) => (
                t!("routine.delete_title").to_string(),
                t!("routine.delete_confirm", name = label.clone()).to_string(),
                t!("routine.delete_button").to_string(),
            ),
            Some(DeleteConfirm::View { label, .. }) => (
                t!("view.delete_title").to_string(),
                t!("view.delete_confirm", name = label.clone()).to_string(),
                t!("view.delete_button").to_string(),
            ),
            Some(DeleteConfirm::Schema { schema, .. }) => (
                t!("database.schema_delete_title").to_string(),
                t!("database.schema_delete_confirm", name = schema.clone()).to_string(),
                t!("database.schema_delete_button").to_string(),
            ),
            None => (String::new(), String::new(), t!("form.ok").to_string()),
        }
    }
}

/// Parse a Redis TTL field. Blank, `-1` and non-positive values mean "no expiry"; a positive
/// value is seconds; anything else is an error.
fn parse_redis_ttl(text: &str) -> Result<Option<i64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<i64>() {
        Ok(ttl) if ttl > 0 => Ok(Some(ttl)),
        Ok(_) => Ok(None),
        Err(_) => Err(t!("redis.key.ttl_invalid").to_string()),
    }
}

/// A Redis key's entry field/member, falling back to `fallback` when the field was left blank
/// (a hash field or stream field cannot be empty, and a sorted set member needs a name).
fn non_empty(value: String, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

#[cfg(test)]
mod focus_tests {
    use super::take_dialog_focus;

    #[test]
    fn dialog_initial_focus_is_taken_once() {
        let mut done = false;
        assert!(
            take_dialog_focus(&mut done),
            "first frame focuses the field"
        );
        assert!(
            !take_dialog_focus(&mut done),
            "later frames leave focus alone"
        );
        assert!(!take_dialog_focus(&mut done));
        // A newly-opened dialog resets the flag, so its field is focused again.
        done = false;
        assert!(take_dialog_focus(&mut done));
    }
}
