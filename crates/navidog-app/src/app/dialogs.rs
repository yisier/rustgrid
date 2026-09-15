use super::*;

impl AppView {
    pub(super) fn render_error_dialog(
        &self,
        message: &str,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let message = message.to_string();

        let titlebar = ui::dialog_titlebar(theme)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.error_dragging = true;
                    this.error_drag_origin = event.position;
                    this.error_drag_base = this.error_offset;
                    cx.notify();
                }),
            )
            .child(div().w(px(60.0)).flex_none())
            .child(self.dialog_close_button(
                "error-close",
                cx.listener(|this, _event, _window, cx| {
                    this.error_dialog = None;
                    this.error_dragging = false;
                    cx.notify();
                }),
            ));

        let body = div()
            .flex()
            .flex_row()
            .items_start()
            .gap_3()
            .px_4()
            .pt_5()
            .pb_6()
            .child(
                div()
                    .w(px(30.0))
                    .h(px(30.0))
                    .flex_none()
                    .rounded_full()
                    .bg(rgb(theme.danger))
                    .text_color(rgb(0xffffff))
                    .text_size(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("✕"),
            )
            .child(div().flex_1().text_size(px(12.5)).child(message));

        let footer = div()
            .flex()
            .flex_row()
            .justify_center()
            .pb_5()
            .child(self.dialog_button(
                "error-ok",
                t!("form.ok").to_string(),
                true,
                cx.listener(|this, _event, _window, cx| {
                    this.error_dialog = None;
                    this.error_dragging = false;
                    cx.notify();
                }),
            ));

        ui::overlay(theme)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.error_dragging {
                    let dx = event.position.x - this.error_drag_origin.x;
                    let dy = event.position.y - this.error_drag_origin.y;
                    this.error_offset = Point {
                        x: this.error_drag_base.x + dx,
                        y: this.error_drag_base.y + dy,
                    };
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    if this.error_dragging {
                        this.error_dragging = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                ui::dialog_frame(theme, theme.dialog_bg)
                    .left(self.error_offset.x)
                    .top(self.error_offset.y)
                    .w(px(600.0))
                    .child(titlebar)
                    .child(body)
                    .child(footer),
            )
    }

    pub(super) fn render_delete_confirm(
        &self,
        confirm: &DeleteConfirm,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let drag = self.confirm_drag(cx);
        match confirm {
            DeleteConfirm::Rows { rows, .. } => {
                let count = rows.len();
                ui::confirm_dialog(
                    ui::ConfirmDialog {
                        id: "grid-delete",
                        title: t!("grid.delete_title").to_string(),
                        message: t!("grid.delete_confirm", count = count).to_string(),
                        confirm_label: t!("grid.delete_button", count = count).to_string(),
                        cancel_label: t!("form.cancel").to_string(),
                    },
                    drag,
                    theme,
                    Rc::new(cx.listener(|this, _event, _window, cx| this.confirm_delete(cx))),
                    Rc::new(cx.listener(|this, _event, _window, cx| this.cancel_delete(cx))),
                )
            }
            DeleteConfirm::Connection { index } => {
                let name = self
                    .connections
                    .get(*index)
                    .map(|node| node.profile.name.clone())
                    .unwrap_or_default();
                ui::confirm_dialog(
                    ui::ConfirmDialog {
                        id: "connection-delete",
                        title: t!("connection.delete_title").to_string(),
                        message: t!("connection.delete_confirm", name = name).to_string(),
                        confirm_label: t!("connection.delete_button").to_string(),
                        cancel_label: t!("form.cancel").to_string(),
                    },
                    drag,
                    theme,
                    Rc::new(cx.listener(|this, _event, _window, cx| this.confirm_delete(cx))),
                    Rc::new(cx.listener(|this, _event, _window, cx| this.cancel_delete(cx))),
                )
            }
        }
    }

    pub(super) fn render_password_prompt(
        &self,
        prompt: &PasswordPrompt,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let name = self
            .connections
            .get(prompt.index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let title = format!("{}: {}", t!("password.title"), name);
        let theme = self.theme;

        ui::overlay(theme).child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .w(px(360.0))
                .p_4()
                .bg(rgb(theme.dialog_bg))
                .child(div().text_size(px(14.0)).child(title))
                .child(div().w_full().h(px(28.0)).child(prompt.input.clone()))
                .child(
                    div()
                        .id("password-save")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            if let Some(prompt) = this.password_prompt.as_mut() {
                                prompt.save_password = !prompt.save_password;
                            }
                            cx.notify();
                        }))
                        .child(checkbox_box(prompt.save_password, theme))
                        .child(t!("form.save_password").to_string()),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .child(self.dialog_button(
                            "password-cancel",
                            t!("form.cancel").to_string(),
                            false,
                            cx.listener(|this, _event, _window, cx| {
                                this.password_prompt = None;
                                cx.notify();
                            }),
                        ))
                        .child(self.dialog_button(
                            "password-ok",
                            t!("form.ok").to_string(),
                            true,
                            cx.listener(|this, _event, _window, cx| {
                                this.submit_password(cx);
                            }),
                        )),
                ),
        )
    }

    pub(super) fn render_dialog(
        &self,
        form: &ConnectionForm,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let title = if self.editing.is_some() {
            t!("form.edit_title")
        } else {
            t!("form.title")
        };
        let name = if form.name.trim().is_empty() {
            t!("app.title").to_string()
        } else {
            form.name.trim().to_string()
        };
        let heading = format!("{name} - {title}");

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
                    .w(px(PANEL_WIDTH))
                    .overflow_hidden()
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
                                            .path("icons/connection.svg")
                                            .w(px(14.0))
                                            .h(px(14.0))
                                            .flex_none()
                                            .text_color(rgb(theme.text)),
                                    )
                                    .child(div().text_size(px(12.5)).child(heading)),
                            )
                            .child(self.dialog_close_button(
                                "form-close",
                                cx.listener(|this, _event, _window, cx| {
                                    this.form = None;
                                    this.form_inputs = None;
                                    this.context_menu = None;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
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
                            .child(form_tab("HTTP".to_string(), false, theme)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .h(px(430.0))
                            .mx_2()
                            .mb_2()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.dialog_bg))
                            .child(self.render_general_tab(form, window, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .h(px(46.0))
                            .px_3()
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
                            ),
                    ),
            )
    }
}
