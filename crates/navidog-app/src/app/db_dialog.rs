use super::*;

impl AppView {
    pub(super) fn render_db_dialog(
        &self,
        dialog: &DbDialog,
        _window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;

        // Destructive actions use the shared confirmation modal rather than the form frame.
        if let DbDialog::Delete { name, error, .. } = dialog {
            let mut message = t!("database.delete_message", name = name).to_string();
            if let Some(error) = error {
                message = format!("{message} ({error})");
            }
            let drag = self.confirm_drag(cx);
            return ui::confirm_dialog(
                ui::ConfirmDialog {
                    id: "database-delete",
                    title: t!("database.delete").to_string(),
                    message,
                    confirm_label: t!("database.delete").to_string(),
                    cancel_label: t!("form.cancel").to_string(),
                },
                drag,
                theme,
                Rc::new(cx.listener(|this, _event, _window, cx| this.db_submit(cx))),
                Rc::new(cx.listener(|this, _event, _window, cx| {
                    this.db_dialog = None;
                    this.db_name_input = None;
                    cx.notify();
                })),
            );
        }

        let (title, content, allow_ok): (String, AnyElement, bool) = match dialog {
            DbDialog::New { error, .. } => {
                let mut name_row = div().flex().flex_row().items_center().gap_2().child(
                    div()
                        .w(px(150.0))
                        .flex_none()
                        .text_size(px(12.0))
                        .child(format!("{}:", t!("database.name"))),
                );
                if let Some(input) = self.db_name_input.as_ref() {
                    name_row = name_row.child(div().w(px(300.0)).h(px(24.0)).child(input.clone()));
                }
                let body = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(name_row)
                    .child(db_error(error, theme));
                (
                    t!("database.new").to_string(),
                    div().p_4().child(body).into_any_element(),
                    true,
                )
            }
            DbDialog::Edit {
                name,
                charset,
                collation,
                charsets,
                collations,
                tab,
                loading,
                error,
                ..
            } => {
                let tabs = div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .gap_0p5()
                    .px_2()
                    .pt_2()
                    .bg(rgb(theme.dialog_face))
                    .child(self.db_tab_button(DbTab::General, cx))
                    .child(self.db_tab_button(DbTab::Sql, cx));

                let page: AnyElement = match tab {
                    DbTab::General => {
                        let mut form = div().flex().flex_col().gap_3().p_4().flex_1().child(
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
                                .child(div().text_size(px(12.0)).child(name.clone())),
                        );
                        if *loading {
                            form = form.child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("common.loading").to_string()),
                            );
                        } else {
                            let collation_options: Vec<String> = if charset.is_empty() {
                                collations.clone()
                            } else {
                                let prefix = format!("{charset}_");
                                collations
                                    .iter()
                                    .filter(|candidate| candidate.starts_with(&prefix))
                                    .cloned()
                                    .collect()
                            };
                            form = form.child(self.db_combo(
                                format!("{}:", t!("database.charset")),
                                charset,
                                charsets,
                                DbCombo::Charset,
                                cx,
                            ));
                            form = form.child(self.db_combo(
                                format!("{}:", t!("database.collation")),
                                collation,
                                &collation_options,
                                DbCombo::Collation,
                                cx,
                            ));
                        }
                        form = form.child(db_error(error, theme));
                        form.into_any_element()
                    }
                    DbTab::Sql => {
                        let mut page = div().flex().flex_col().gap_2().p_4().flex_1();
                        match self.db_alter_preview() {
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
                                                    window.focus(&this.db_sql_focus);
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
                                        .child(text),
                                );
                            }
                        }
                        page.into_any_element()
                    }
                };

                (
                    t!("database.edit").to_string(),
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
                        .into_any_element(),
                    !*loading,
                )
            }
            DbDialog::Delete { name, error, .. } => {
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
                    .child(db_error(error, theme));
                (
                    t!("database.delete").to_string(),
                    div().p_4().child(body).into_any_element(),
                    true,
                )
            }
        };

        ui::overlay(theme)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.db_sql_selecting {
                    this.db_sql_cursor = this.db_sql_index_for_position(event.position);
                    cx.notify();
                } else if this.form_dragging {
                    let dx = event.position.x - this.form_drag_origin.x;
                    let dy = event.position.y - this.form_drag_origin.y;
                    this.form_offset = Point {
                        x: this.form_drag_base.x + dx,
                        y: this.form_drag_base.y + dy,
                    };
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.form_dragging || this.db_sql_selecting {
                        this.form_dragging = false;
                        this.db_sql_selecting = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                ui::dialog_frame(theme, theme.dialog_face)
                    .left(self.form_offset.x)
                    .top(self.form_offset.y)
                    .w(px(520.0))
                    .child(
                        ui::dialog_header(theme)
                            .pl_3()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                    this.form_dragging = true;
                                    this.form_drag_origin = event.position;
                                    this.form_drag_base = this.form_offset;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        svg()
                                            .path("icons/database.svg")
                                            .w(px(14.0))
                                            .h(px(14.0))
                                            .flex_none()
                                            .text_color(rgb(theme.text)),
                                    )
                                    .child(div().text_size(px(12.5)).child(title)),
                            )
                            .child(self.dialog_close_button(
                                "db-close",
                                cx.listener(|this, _event, _window, cx| {
                                    this.db_dialog = None;
                                    this.db_name_input = None;
                                    this.db_combo = None;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(content)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .h(px(46.0))
                            .px_3()
                            .border_t_1()
                            .border_color(rgb(theme.border))
                            .child(self.dialog_button(
                                "db-cancel",
                                t!("form.cancel").to_string(),
                                false,
                                cx.listener(|this, _event, _window, cx| {
                                    this.db_dialog = None;
                                    this.db_name_input = None;
                                    cx.notify();
                                }),
                            ))
                            .child(self.dialog_button(
                                "db-ok",
                                t!("form.ok").to_string(),
                                true,
                                cx.listener(move |this, _event, _window, cx| {
                                    if allow_ok {
                                        this.db_submit(cx);
                                    }
                                }),
                            )),
                    ),
            )
            .into_any_element()
    }
}
