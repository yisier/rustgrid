use super::*;
use gpui_kit::component::WindowExt;

impl AppView {
    /// Opens the database dialog as a `Root`-managed modal.
    pub(super) fn open_db_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let (title, footer) = app.update(cx, |app, cx| {
                (
                    app.db_dialog_title(),
                    app.db_dialog_footer(cx).into_any_element(),
                )
            });
            let on_close = app.downgrade();
            let content_app = app.clone();
            dialog
                .title(title)
                .w(px(560.0))
                .content(move |content, _window, cx| {
                    let body =
                        content_app.update(cx, |app, cx| app.db_dialog_body(cx).into_any_element());
                    content.child(body)
                })
                .footer(footer)
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.db_cancel(cx));
                })
        });
    }

    /// Opens the SQL Server "New Schema" dialog as a `Root`-managed modal.
    pub(super) fn open_schema_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            // The footer is built once here and is never rebuilt, so anything it reads must not
            // depend on later state. `submitting` is only read per-frame inside `content`.
            let title = app.update(cx, |_, _| t!("database.new_schema").to_string());
            let theme = app.read(cx).theme;
            let on_close = app.downgrade();
            let footer_cancel = app.downgrade();
            let footer_ok = app.downgrade();
            let content_app = app.clone();
            let ok_app = app.downgrade();
            let cancel_app = app.downgrade();
            dialog
                .title(title)
                .margin_top(super::dialogs::centered_margin_top(window, 190.0))
                .content(move |content, _window, cx| {
                    let body = content_app.update(cx, |app, _cx| {
                        let Some(dialog) = app.schema_dialog.as_ref() else {
                            return div().into_any_element();
                        };
                        let field = match app.schema_name_input.as_ref() {
                            Some(input) => div().h(px(24.0)).w_full().child(input.clone()),
                            None => div().w_full(),
                        };
                        let error = dialog.error.clone();
                        let submitting = dialog.submitting;
                        let theme = app.theme;
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(field)
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("database.schema_name_hint").to_string()),
                            )
                            // Kept in `content`, which re-renders every frame, so the busy state
                            // actually reaches the screen (the footer cannot carry it).
                            .when(submitting, |this| {
                                this.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(theme.text_muted))
                                        .child(t!("common.loading").to_string()),
                                )
                            })
                            .child(app.render_db_error(&error))
                            .into_any_element()
                    });
                    content.child(body)
                })
                .footer(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .w_full()
                        .child(ui::button(
                            "schema-cancel",
                            t!("form.cancel").to_string(),
                            ButtonKind::Normal,
                            theme,
                            move |_event, _window, cx| {
                                let _ = footer_cancel.update(cx, |app, cx| app.schema_cancel(cx));
                            },
                        ))
                        .child(ui::button(
                            "schema-ok",
                            t!("form.ok").to_string(),
                            ButtonKind::Default,
                            theme,
                            move |_event, _window, cx| {
                                // Re-entry is guarded inside `schema_submit`, which sees the
                                // live state; this closure cannot (it is built once).
                                let _ = footer_ok.update(cx, |app, cx| app.schema_submit(cx));
                            },
                        )),
                )
                .button_props(
                    gpui_kit::component::dialog::DialogButtonProps::default()
                        .ok_text(t!("form.ok").to_string())
                        .cancel_text(t!("form.cancel").to_string())
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let _ = ok_app.update(cx, |app, cx| app.schema_submit(cx));
                            // Never close from here. Returning `true` would dismiss the dialog on
                            // Enter even when the create failed, leaving `schema_dialog` set with
                            // no dialog on screen (and the error unreadable). Closing is driven
                            // entirely by `schema_dialog` going back to `None`.
                            false
                        })
                        .on_cancel(move |_, _, cx| {
                            let _ = cancel_app.update(cx, |app, cx| app.schema_cancel(cx));
                            true
                        }),
                )
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.schema_cancel(cx));
                })
        });
    }

    pub(super) fn db_cancel(&mut self, cx: &mut Context<'_, Self>) {
        self.db_dialog = None;
        self.db_name_input = None;
        cx.notify();
    }

    fn db_dialog_title(&self) -> String {
        match self.db_dialog {
            Some(DbDialog::Edit(ref form)) if form.database_index.is_none() => {
                t!("database.new").to_string()
            }
            Some(DbDialog::Edit(_)) => t!("database.edit").to_string(),
            Some(DbDialog::Delete { .. }) => t!("database.delete").to_string(),
            None => String::new(),
        }
    }

    fn db_dialog_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let allow_ok = match self.db_dialog.as_ref() {
            Some(DbDialog::Edit(form)) => !form.loading,
            Some(_) => true,
            None => false,
        };
        let ok_label = if matches!(self.db_dialog, Some(DbDialog::Delete { .. })) {
            t!("database.delete").to_string()
        } else {
            t!("form.ok").to_string()
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .w_full()
            .gap_2()
            .h(px(46.0))
            .child(self.dialog_button(
                "db-cancel",
                t!("form.cancel").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.db_cancel(cx)),
            ))
            .child(self.dialog_button(
                "db-ok",
                ok_label,
                true,
                cx.listener(move |this, _event, _window, cx| {
                    if allow_ok {
                        this.db_submit(cx);
                    }
                }),
            ))
    }

    fn db_dialog_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;

        match self.db_dialog.as_ref() {
            Some(DbDialog::Edit(form)) => {
                // Creating: the name is the editable input. Editing: it is fixed and read-only.
                let name_field: AnyElement = if form.database_index.is_none() {
                    match self.db_name_input.as_ref() {
                        Some(input) => div()
                            .w(px(300.0))
                            .h(px(24.0))
                            .child(input.clone())
                            .into_any_element(),
                        None => div().into_any_element(),
                    }
                } else {
                    div()
                        .text_size(px(12.0))
                        .child(form.name.clone())
                        .into_any_element()
                };

                let mut tabs = div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .gap_0p5()
                    .px_2()
                    .pt_2()
                    .bg(rgb(theme.dialog_face));
                for tab in self.db_tabs() {
                    tabs = tabs.child(self.db_tab_button(tab, cx));
                }

                let page: AnyElement = match form.tab {
                    DbTab::General => {
                        let mut body = div().flex().flex_col().gap_3().p_4().flex_1().child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(150.0))
                                        .flex_none()
                                        .text_size(px(12.0))
                                        .child(format!("{}:", t!("database.name"))),
                                )
                                .child(name_field),
                        );
                        if form.loading {
                            body = body.child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("common.loading").to_string()),
                            );
                        } else {
                            if form.spec.owner {
                                body = body.child(db_combo_row(
                                    format!("{}:", t!("database.owner")),
                                    self.db_owner_combo.clone(),
                                ));
                            }
                            if form.spec.charset {
                                body = body.child(db_combo_row(
                                    format!("{}:", t!("database.charset")),
                                    self.db_charset_combo.clone(),
                                ));
                            }
                            if form.spec.collation {
                                body = body.child(db_combo_row(
                                    format!("{}:", t!("database.collation")),
                                    self.db_collation_combo.clone(),
                                ));
                            }
                            if form.spec.recovery_model {
                                body = body.child(db_combo_row(
                                    format!("{}:", t!("database.recovery_model")),
                                    self.db_recovery_combo.clone(),
                                ));
                            }
                            if form.spec.compatibility_level {
                                body = body.child(db_combo_row(
                                    format!("{}:", t!("database.compatibility_level")),
                                    self.db_compat_combo.clone(),
                                ));
                            }
                        }
                        body = body.child(self.render_db_error(&form.error));
                        body.into_any_element()
                    }
                    DbTab::Extra(kind) => {
                        let body = div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_4()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(
                                        t!(super::database::database_editor_tab_key(kind))
                                            .to_string(),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("database.tab.not_editable").to_string()),
                            );
                        body.into_any_element()
                    }
                    DbTab::Sql => {
                        let mut page = div().flex().flex_col().gap_2().p_4().flex_1();
                        match self.db_sql_preview() {
                            None => {
                                page = page.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(theme.text_muted))
                                        .child(t!("database.no_changes").to_string()),
                                );
                            }
                            Some(sql) => {
                                let (start, end) = self.db_sql_selection_range();
                                let mut text = StyledText::new(sql.clone());
                                if start < end {
                                    text = text.with_highlights(vec![(
                                        start..end,
                                        HighlightStyle {
                                            background_color: Some(
                                                rgb(theme.tree_selected_bg).into(),
                                            ),
                                            ..Default::default()
                                        },
                                    )]);
                                }
                                *self.db_sql_layout.borrow_mut() = text.layout().clone();
                                *self.db_sql_text.borrow_mut() = sql;

                                page = page.child(
                                    div()
                                        .id("db-sql")
                                        .track_focus(&self.db_sql_focus)
                                        .cursor_text()
                                        .w_full()
                                        .flex_1()
                                        .p_2()
                                        .text_size(px(12.0))
                                        .font_family("Consolas")
                                        .bg(rgb(theme.input_bg))
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .overflow_hidden()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                |this, event: &MouseDownEvent, window, cx| {
                                                    window.focus(&this.db_sql_focus, cx);
                                                    let index = this
                                                        .db_sql_index_for_position(event.position);
                                                    this.db_sql_anchor = index;
                                                    this.db_sql_cursor = index;
                                                    this.db_sql_selecting = true;
                                                    cx.notify();
                                                },
                                            ),
                                        )
                                        .on_key_down(cx.listener(|this, event, _window, cx| {
                                            this.db_sql_key(event, cx)
                                        }))
                                        .on_mouse_move(cx.listener(
                                            |this, event: &MouseMoveEvent, _window, cx| {
                                                if this.db_sql_selecting {
                                                    this.db_sql_cursor = this
                                                        .db_sql_index_for_position(event.position);
                                                    cx.notify();
                                                }
                                            },
                                        ))
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(
                                                |this, _event: &MouseUpEvent, _window, cx| {
                                                    if this.db_sql_selecting {
                                                        this.db_sql_selecting = false;
                                                        cx.notify();
                                                    }
                                                },
                                            ),
                                        )
                                        .child(text),
                                );
                            }
                        }
                        page.into_any_element()
                    }
                };

                div()
                    .flex()
                    .flex_col()
                    .child(tabs)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .h(px(300.0))
                            .mx_2()
                            .mb_2()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.dialog_bg))
                            .child(page),
                    )
                    .into_any_element()
            }
            Some(DbDialog::Delete {
                name,
                submitting,
                error,
                ..
            }) => {
                let body = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(12.0)).child(format!(
                        "{}: {}",
                        t!("database.name"),
                        name
                    )))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.danger))
                            .child(t!("database.delete_confirm").to_string()),
                    )
                    // The footer is built once, so the busy state can only reach the screen
                    // from here (rebuilt every frame).
                    .when(*submitting, |this| {
                        this.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgb(theme.text_muted))
                                .child(t!("common.loading").to_string()),
                        )
                    })
                    .child(self.render_db_error(error));
                div().p_4().child(body).into_any_element()
            }
            None => div().into_any_element(),
        }
    }
}
/// A labelled editable drop-down row (`label: [combo]`), with the combo entity rendered as-is.
fn db_combo_row(label: String, combo: Option<Entity<ComboBox>>) -> Div {
    let mut row = div().flex().flex_row().items_center().gap_2().child(
        div()
            .w(px(150.0))
            .flex_none()
            .text_size(px(12.0))
            .child(label),
    );
    if let Some(combo) = combo {
        row = row.child(combo);
    }
    row
}
