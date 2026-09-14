use super::*;

impl AppView {
    pub(super) fn render_general_tab(
        &self,
        form: &ConnectionForm,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let mut content = div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .pb_2()
                    .child(self.header_endpoint(
                        "logo.png",
                        t!("form.header_source").to_string(),
                        true,
                        theme,
                    ))
                    .child(
                        div()
                            .w(px(150.0))
                            .border_t_1()
                            .border_color(rgb(theme.border)),
                    )
                    .child(self.header_endpoint(
                        "icons/database.svg",
                        t!("form.header_target").to_string(),
                        false,
                        theme,
                    )),
            )
            .child(self.render_form_field(
                FormField::Name,
                format!("{}:", t!("form.name")),
                form.value(FormField::Name),
                window,
                cx,
            ))
            .child(self.render_form_field(
                FormField::Host,
                format!("{}:", t!("form.host")),
                form.value(FormField::Host),
                window,
                cx,
            ))
            .child(self.render_form_field(
                FormField::Port,
                format!("{}:", t!("form.port")),
                form.value(FormField::Port),
                window,
                cx,
            ))
            .child(self.render_form_field(
                FormField::Username,
                format!("{}:", t!("form.username")),
                form.value(FormField::Username),
                window,
                cx,
            ))
            .child(self.render_form_field(
                FormField::Password,
                format!("{}:", t!("form.password")),
                form.value(FormField::Password),
                window,
                cx,
            ))
            .child(
                div()
                    .id("form-save-password")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .pl(px(120.0))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        if let Some(form) = this.form.as_mut() {
                            form.save_password = !form.save_password;
                        }
                        cx.notify();
                    }))
                    .child(checkbox_box(form.save_password, theme))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child(t!("form.save_password").to_string()),
                    ),
            );

        if let TestStatus::Failed(error) = &self.test_status {
            content =
                content.child(self.render_selectable_text("form-error", error, theme.danger, cx));
        }

        content
    }

    pub(super) fn render_selectable_text(
        &self,
        id: &'static str,
        text: &str,
        color: u32,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let (start, end) = self.db_sql_selection_range();
        let mut styled = StyledText::new(text.to_string());
        if start < end {
            styled = styled.with_highlights(vec![(
                start..end,
                HighlightStyle {
                    background_color: Some(rgb(theme.tree_selected_bg).into()),
                    ..Default::default()
                },
            )]);
        }
        *self.db_sql_layout.borrow_mut() = styled.layout().clone();
        *self.db_sql_text.borrow_mut() = text.to_string();

        div()
            .id(id)
            .track_focus(&self.db_sql_focus)
            .cursor_text()
            .w_full()
            .overflow_hidden()
            .text_size(px(12.0))
            .text_color(rgb(color))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.db_sql_focus);
                    let index = this.db_sql_index_for_position(event.position);
                    this.db_sql_anchor = index;
                    this.db_sql_cursor = index;
                    this.db_sql_selecting = true;
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, event, _window, cx| this.db_sql_key(event, cx)))
            .child(styled)
    }

    pub(super) fn header_endpoint(
        &self,
        path: &'static str,
        label: String,
        is_image: bool,
        theme: Theme,
    ) -> AnyElement {
        let icon: AnyElement = if is_image {
            img(ImageSource::Resource(Resource::Embedded(path.into())))
                .w(px(34.0))
                .h(px(34.0))
                .into_any_element()
        } else {
            svg()
                .path(path)
                .w(px(28.0))
                .h(px(28.0))
                .text_color(rgb(theme.text))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(div().h(px(34.0)).flex().items_center().child(icon))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text))
                    .child(label),
            )
            .into_any_element()
    }

    pub(super) fn dialog_button(
        &self,
        id: &'static str,
        label: String,
        primary: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        ui::dialog_button(id, label, primary, self.theme, on_click)
    }

    pub(super) fn dialog_close_button(
        &self,
        id: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        ui::dialog_close_button(id, self.theme, on_click)
    }

    pub(super) fn win_button(
        &self,
        id: impl Into<SharedString>,
        label: String,
        kind: ButtonKind,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        ui::button(id, label, kind, self.theme, on_click)
    }

    pub(super) fn toolbar_item(
        &self,
        id: impl Into<SharedString>,
        icon: &'static str,
        label: String,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        ui::toolbar_item(id, icon, label, enabled, self.theme, on_click)
    }

    pub(super) fn render_form_field(
        &self,
        field: FormField,
        label: String,
        value: &str,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let masked = field == FormField::Password;
        let width = match field {
            FormField::Name | FormField::Host => 360.0,
            FormField::Port => 80.0,
            FormField::Username | FormField::Password | FormField::Database => 300.0,
        };
        let handle = self.form_focus.get(field);
        let focused = handle.is_focused(window);
        let theme = self.theme;

        let chars: Vec<char> = value.chars().collect();
        let display: Vec<char> = if masked {
            vec!['*'; chars.len()]
        } else {
            chars.clone()
        };
        let len = display.len();

        let mut shown = div().flex().flex_row().items_center();
        if focused {
            let mut selection = self.form_selection;
            selection.anchor = selection.anchor.min(len);
            selection.cursor = selection.cursor.min(len);
            let (start, end) = selection.range();
            if start < end {
                if start > 0 {
                    shown = shown.child(display[..start].iter().copied().collect::<String>());
                }
                shown = shown.child(
                    div()
                        .bg(rgb(theme.tree_selected_bg))
                        .text_color(rgb(theme.tree_selected_text))
                        .child(display[start..end].iter().copied().collect::<String>()),
                );
                if end < len {
                    shown = shown.child(display[end..].iter().copied().collect::<String>());
                }
            } else {
                if selection.cursor > 0 {
                    shown = shown.child(
                        display[..selection.cursor]
                            .iter()
                            .copied()
                            .collect::<String>(),
                    );
                }
                if self.caret_visible {
                    shown = shown.child("|");
                }
                if selection.cursor < len {
                    shown = shown.child(
                        display[selection.cursor..]
                            .iter()
                            .copied()
                            .collect::<String>(),
                    );
                }
            }
        } else {
            shown = shown.child(display.iter().copied().collect::<String>());
        }

        let focus_handle = handle.clone();
        let index_text = value.to_string();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(FIELD_LABEL_WIDTH))
                    .flex_none()
                    .text_size(px(12.0))
                    .child(label),
            )
            .child(
                div()
                    .id(SharedString::from(format!("field-{}", field as usize)))
                    .track_focus(handle)
                    .cursor_text()
                    .on_key_down(cx.listener(move |this, event, window, cx| {
                        this.form_key(field, event, window, cx)
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            window.focus(&focus_handle);
                            this.form_active_field = field;
                            let index =
                                this.field_index_for_x(&index_text, event.position.x, window);
                            this.form_selection = FieldSelection {
                                anchor: index,
                                cursor: index,
                            };
                            this.form_selecting = true;
                            this.form_dragging = false;
                            cx.notify();
                        }),
                    )
                    .w(px(width))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .text_size(px(12.0))
                    .bg(rgb(theme.input_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .child(shown),
            )
    }

    pub(super) fn render_context_menu(
        &self,
        menu: &ContextMenu,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let mut items = div()
            .flex()
            .flex_col()
            .w(px(150.0))
            .py_1()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border));

        match &menu.target {
            ContextTarget::Connection(index) => {
                let index = *index;
                let connected = self
                    .connections
                    .get(index)
                    .map(|node| matches!(&node.status, ConnectionStatus::Connected(_)))
                    .unwrap_or(false);
                let connect_label = if connected {
                    t!("connection.disconnect").to_string()
                } else {
                    t!("connection.connect").to_string()
                };

                items = items
                    .child(self.context_item(
                        "ctx-connect",
                        connect_label,
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.toggle_connection(index, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "ctx-edit",
                        t!("connection.edit").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.open_edit_form(index, window, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "ctx-delete",
                        t!("connection.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.delete_connection(index, cx);
                        }),
                    ));
            }
            ContextTarget::Database {
                connection_index,
                database_index,
            } => {
                let ci = *connection_index;
                let di = *database_index;
                let opened = self
                    .connections
                    .get(ci)
                    .and_then(|node| match &node.databases {
                        Loadable::Loaded(databases) => databases.get(di),
                        _ => None,
                    })
                    .map(|database| database.opened)
                    .unwrap_or(false);

                if opened {
                    items = items.child(self.context_item(
                        "db-close",
                        t!("database.close").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.close_database(ci, di, cx);
                        }),
                    ));
                } else {
                    items = items.child(self.context_item(
                        "db-open",
                        t!("database.open").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_database(ci, di, cx);
                        }),
                    ));
                }

                items = items
                    .child(self.context_item(
                        "db-edit",
                        t!("database.edit").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.open_edit_database(ci, di, window, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "db-new",
                        t!("database.new").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.open_new_database(ci, window, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "db-delete",
                        t!("database.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_delete_database(ci, di, cx);
                        }),
                    ))
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "db-refresh",
                        t!("database.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.load_databases(ci, cx);
                        }),
                    ));
            }
        }

        div()
            .absolute()
            .left(menu.position.x)
            .top(menu.position.y)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.context_menu = None;
                cx.notify();
            }))
            .child(items)
    }

    pub(super) fn render_tab_menu(
        &self,
        menu: &TabMenu,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let target = menu.target;
        let items = div()
            .flex()
            .flex_col()
            .w(px(160.0))
            .py_1()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .child(self.context_item(
                "tab-close",
                t!("tab.close").to_string(),
                cx.listener(move |this, _event, _window, cx| this.close_tab(target, cx)),
            ))
            .child(self.context_item(
                "tab-close-others",
                t!("tab.close_other").to_string(),
                cx.listener(move |this, _event, _window, cx| this.close_other_tabs(target, cx)),
            ))
            .child(self.context_item(
                "tab-close-all",
                t!("tab.close_all").to_string(),
                cx.listener(|this, _event, _window, cx| this.close_all_tabs(cx)),
            ));

        div()
            .absolute()
            .left(menu.position.x)
            .top(menu.position.y)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.tab_menu = None;
                cx.notify();
            }))
            .child(items)
    }

    pub(super) fn context_item(
        &self,
        id: &'static str,
        label: String,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id(id)
            .flex()
            .items_center()
            .w_full()
            .h(px(24.0))
            .px_3()
            .text_size(px(12.5))
            .cursor_pointer()
            .hover(move |style| {
                style
                    .bg(rgb(theme.tree_hover_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .on_click(on_click)
            .child(label)
    }
}
