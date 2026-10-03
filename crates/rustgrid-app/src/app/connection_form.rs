//! The New/Edit Connection window (新建连接 / 编辑连接).
//!
//! A separate OS window styled like the reference design: a slim header with a status dot, a
//! minimal tab bar with an underline, one-column pages with labels above large inputs, and a
//! footer with 重置 / 测试连接 / 保存并连接.

use std::rc::Rc;

use rfd::AsyncFileDialog;

use super::*;

/// A text field outside the six general fields.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FormExtra {
    TlsCa,
    TlsCert,
    TlsKey,
    ConnectTimeout,
    QueryTimeout,
    KeepaliveInterval,
    InitSql,
}

impl FormExtra {
    fn key(self) -> &'static str {
        match self {
            FormExtra::TlsCa => "form-tls-ca",
            FormExtra::TlsCert => "form-tls-cert",
            FormExtra::TlsKey => "form-tls-key",
            FormExtra::ConnectTimeout => "form-connect-timeout",
            FormExtra::QueryTimeout => "form-query-timeout",
            FormExtra::KeepaliveInterval => "form-keepalive-interval",
            FormExtra::InitSql => "form-init-sql",
        }
    }
}

/// One editable field of a tunnel layer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum TunnelField {
    Host,
    Port,
    Username,
    Password,
    KeyPath,
}

/// The text inputs of one tunnel layer, parallel to `form.settings.tunnel`.
pub(super) struct TunnelInputs {
    pub(super) host: Entity<TextInput>,
    pub(super) port: Entity<TextInput>,
    pub(super) username: Entity<TextInput>,
    pub(super) password: Entity<TextInput>,
    pub(super) key_path: Entity<TextInput>,
}

/// The root view of the connection OS window. It re-renders whenever `AppView` changes.
pub(super) struct ConnectionWindow {
    app: WeakEntity<AppView>,
    _subscription: Subscription,
}

impl ConnectionWindow {
    pub(super) fn new(
        app: WeakEntity<AppView>,
        app_entity: &Entity<AppView>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let subscription = cx.observe(app_entity, |_, _, cx| cx.notify());
        Self {
            app,
            _subscription: subscription,
        }
    }
}

impl Render for ConnectionWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        app.update(cx, |app, cx| app.connection_window_contents(cx))
    }
}

impl AppView {
    // ----- Window lifecycle --------------------------------------------------------------------

    /// Open the OS window hosting the connection form (mirrors the Options/Export windows).
    pub(super) fn open_connection_window(
        &mut self,
        name_focus: FocusHandle,
        cx: &mut Context<'_, Self>,
    ) {
        // Only one connection window exists at a time. If it is already open (possibly behind the
        // main window), raise it and focus the alias field instead of silently doing nothing.
        if let Some(handle) = self.connection_window {
            let _ = handle.update(cx, |_, window, cx| {
                window.activate_window();
                window.focus(&name_focus, cx);
                window.refresh();
            });
            return;
        }
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        let title = self.form_heading();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(620.0), px(560.0)), cx);
            let view_weak = weak.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.clone().into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    #[cfg(target_os = "windows")]
                    crate::win_resize::install(window);
                    window.activate_window();
                    let view =
                        cx.new(|cx| ConnectionWindow::new(view_weak.clone(), &app_entity, cx));
                    let root = cx.new(|cx| gpui_kit::component::Root::new(view, window, cx));
                    window.focus(&name_focus, cx);
                    root
                },
            );
            match opened {
                Ok(handle) => app.update(cx, |app, cx| {
                    app.connection_window = Some(handle);
                    cx.notify();
                }),
                Err(error) => app.update(cx, |app, cx| {
                    app.error_dialog = Some(error.to_string());
                    cx.notify();
                }),
            }
        });
    }

    /// Drop the form state (the window, if any, is handled by the caller).
    pub(super) fn clear_form_state(&mut self) {
        self.form = None;
        self.form_initial = None;
        self.form_inputs = None;
        self.form_errors.clear();
        self.form_tunnel_errors.clear();
        self.form_extra_inputs.clear();
        self.form_tunnel_inputs.clear();
        self.form_select_open = false;
        self.form_tunnel_select_open = false;
        self.editing = None;
        self.test_status = TestStatus::Idle;
    }

    /// Close the connection window (from a footer button or a successful save) and drop its state.
    ///
    /// The window is removed on the next effect flush instead of synchronously: removing a window
    /// fires gpui's window-closed observers, and the app's observer re-enters `AppView`. On the
    /// save path this runs from inside an `AppView` update, so a synchronous removal would
    /// double-lease the entity and panic; deferring lets the current update cycle unwind first.
    pub(super) fn close_connection_window(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(handle) = self.connection_window.take() {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        self.clear_form_state();
        cx.notify();
    }

    /// Close the window from its own key handler (ESC).
    fn cancel_connection_form(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.connection_window = None;
        self.clear_form_state();
        window.remove_window();
        cx.notify();
    }

    /// The window's titlebar text: `Name - New Connection` / `Name - Edit Connection`.
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

    /// The whole window: header, tab bar, the page and the footer.
    pub(super) fn connection_window_contents(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;

        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.dialog_bg))
            .text_color(rgb(theme.text))
            .track_focus(&self.form_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.cancel_connection_form(window, cx);
                }
            }))
            .child(self.form_header())
            .child(self.form_tab_strip(cx))
            .child(
                div()
                    .id("form-page")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_y_scroll()
                    .px_5()
                    .py_3()
                    .child(self.form_page(cx)),
            )
            .child(
                div()
                    .flex_none()
                    .w_full()
                    .border_t_1()
                    .border_color(rgb(theme.border))
                    .child(self.form_footer(cx)),
            );

        if self.form_select_open {
            root = root.child(self.render_select_menu(cx));
        }
        if self.form_tunnel_select_open {
            root = root.child(self.render_tunnel_auth_menu(cx));
        }

        root.into_any_element()
    }

    /// The slim header: a status dot, the title, and the window controls.
    fn form_header(&self) -> impl IntoElement {
        let theme = self.theme;
        let title = if self.editing.is_some() {
            t!("form.window.edit")
        } else {
            t!("form.window.new")
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(48.0))
            .flex_none()
            .px_4()
            .bg(rgb(theme.dialog_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .id("form-titlebar-drag")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .w(px(9.0))
                            .h(px(9.0))
                            .flex_none()
                            .rounded(px(5.0))
                            .bg(rgb(0x22c55e)),
                    )
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    ),
            )
            .child(titlebar_button(
                "form-titlebar-min",
                "—",
                theme,
                |window, _cx| {
                    window.minimize_window();
                },
            ))
            .child(titlebar_button(
                "form-titlebar-max",
                "□",
                theme,
                |window, _cx| {
                    toggle_maximize(window);
                },
            ))
            .child(titlebar_button(
                "form-titlebar-close",
                "✕",
                theme,
                |window, _cx| {
                    window.remove_window();
                },
            ))
    }

    /// The tab bar with a black underline on the active tab.
    fn form_tab_strip(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let current = self.form.as_ref().map(|form| form.tab);
        // A file-based engine has no TLS or tunnel, so those two pages are hidden.
        let file_based = self
            .form
            .as_ref()
            .map(|form| form.file_based)
            .unwrap_or(false);
        let relay = self.relay_label();
        let mut strip = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .flex_none()
            .px_6()
            .bg(rgb(theme.dialog_bg))
            .border_b_1()
            .border_color(rgb(theme.border));

        for tab in FormTab::ALL {
            if file_based && matches!(tab, FormTab::Tls | FormTab::Tunnel) {
                continue;
            }
            let active = current == Some(tab);
            let relay = relay.clone();
            strip = strip.child(
                div()
                    .id(SharedString::from(format!("form-tab-{}", tab as usize)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(40.0))
                    .px_3()
                    .cursor_pointer()
                    .border_b(px(2.0))
                    .border_color(if active {
                        rgb(theme.text)
                    } else {
                        rgba(0x00000000)
                    })
                    .text_color(rgb(if active { theme.text } else { theme.text_muted }))
                    .font_weight(if active {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .when(!active, move |style| {
                        style.hover(move |style| style.text_color(rgb(theme.text)))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_form_tab(tab, cx);
                    }))
                    .child(
                        div()
                            .text_size(px(13.5))
                            .child(t!(tab.label_key()).to_string()),
                    )
                    .when(tab == FormTab::Tunnel, move |row| {
                        row.child(small_pill(relay.clone(), theme))
                    }),
            );
        }

        strip
    }

    /// The current relay mode's short label, for the 隧道/代理 tab badge.
    fn relay_label(&self) -> String {
        match self
            .form
            .as_ref()
            .and_then(|form| form.settings.tunnel.first())
            .map(|layer| layer.kind)
        {
            None => t!("form.tunnel.direct").to_string(),
            Some(kind) => t!(kind.label_key()).to_string(),
        }
    }

    fn set_form_tab(&mut self, tab: FormTab, cx: &mut Context<'_, Self>) {
        if let Some(form) = self.form.as_mut() {
            form.tab = tab;
        }
        self.form_select_open = false;
        self.form_tunnel_select_open = false;
        cx.notify();
    }

    /// The active page's body.
    fn form_page(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(form) = self.form.as_ref() else {
            return div().into_any_element();
        };
        match form.tab {
            FormTab::General => self.render_general_page(form, cx).into_any_element(),
            FormTab::Tls => self.render_tls_page(form, cx).into_any_element(),
            FormTab::Tunnel => self.render_tunnel_page(form, cx).into_any_element(),
            FormTab::Advanced => self.render_advanced_page(form, cx).into_any_element(),
        }
    }

    // ----- 常规 --------------------------------------------------------------------------------

    fn render_general_page(&self, form: &ConnectionForm, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let driver_name = self
            .registry
            .get(&form.driver)
            .map(|driver| driver.display_name())
            .unwrap_or_else(|| form.driver.to_string());
        // A read-only box showing the engine, used on both the network and file layouts.
        let engine_box = |name: String| -> AnyElement {
            div()
                .flex()
                .flex_row()
                .items_center()
                .w_full()
                .h(px(32.0))
                .px_3()
                .rounded(px(8.0))
                .border_1()
                .border_color(rgb(theme.border))
                .bg(rgb(theme.input_bg))
                .text_size(px(13.0))
                .text_color(rgb(theme.text_muted))
                .child(name)
                .into_any_element()
        };
        let input = |field: FormField| -> AnyElement {
            match self
                .form_inputs
                .as_ref()
                .map(|inputs| inputs.get(field).clone())
            {
                Some(input) => div().w_full().child(input).into_any_element(),
                None => div().into_any_element(),
            }
        };
        // A required field flagged by the last 测试连接 / 保存并连接 shows 此项为必填 in place of its
        // normal hint, matching the reference design.
        let error_hint = |field: FormField| -> Option<AnyElement> {
            self.form_errors.contains(&field).then(|| {
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.danger))
                    .child(t!("form.required").to_string())
                    .into_any_element()
            })
        };

        // A file-based engine (SQLite) has no host/port/user/password: it edits one database file.
        if form.file_based {
            return div()
                .flex()
                .flex_col()
                .gap_4()
                .child(form_field(
                    t!("form.engine").to_string(),
                    false,
                    None,
                    engine_box(driver_name),
                    theme,
                ))
                .child(form_field(
                    t!("form.alias").to_string(),
                    true,
                    error_hint(FormField::Name),
                    input(FormField::Name),
                    theme,
                ))
                .child(form_field(
                    t!("form.database_file").to_string(),
                    true,
                    error_hint(FormField::Database).or_else(|| {
                        Some(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(theme.text_muted))
                                .child(t!("form.database_file_hint").to_string())
                                .into_any_element(),
                        )
                    }),
                    database_file_row(theme, input(FormField::Database), cx),
                    theme,
                ))
                .child(match &self.test_status {
                    TestStatus::Failed(error) => self
                        .render_selectable_text("form-error", error, theme.danger, cx)
                        .into_any_element(),
                    _ => div().into_any_element(),
                })
                .into_any_element();
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(form_field(
                t!("form.engine").to_string(),
                false,
                None,
                engine_box(driver_name),
                theme,
            ))
            .child(form_field(
                t!("form.alias").to_string(),
                true,
                error_hint(FormField::Name),
                input(FormField::Name),
                theme,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(
                        div().flex_1().min_w(px(0.0)).child(form_field(
                            t!("form.network").to_string(),
                            true,
                            error_hint(FormField::Host),
                            match self
                                .form_inputs
                                .as_ref()
                                .map(|inputs| inputs.get(FormField::Host).clone())
                            {
                                Some(input) => input.into_any_element(),
                                None => div().into_any_element(),
                            },
                            theme,
                        )),
                    )
                    .child(
                        div().w(px(150.0)).flex_none().child(form_field(
                            t!("form.port").to_string(),
                            true,
                            error_hint(FormField::Port),
                            match self
                                .form_inputs
                                .as_ref()
                                .map(|inputs| inputs.get(FormField::Port).clone())
                            {
                                Some(input) => input.into_any_element(),
                                None => div().into_any_element(),
                            },
                            theme,
                        )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_4()
                    .child(form_field(
                        t!("form.username").to_string(),
                        true,
                        error_hint(FormField::Username),
                        input(FormField::Username),
                        theme,
                    ))
                    .child(form_field(
                        t!("form.password").to_string(),
                        false,
                        error_hint(FormField::Password),
                        input(FormField::Password),
                        theme,
                    )),
            )
            .child(form_field(
                t!("form.database_optional").to_string(),
                false,
                error_hint(FormField::Database),
                input(FormField::Database),
                theme,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("form-save-password")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
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
                                    .text_size(px(12.5))
                                    .child(t!("form.remember_password").to_string()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text_muted))
                            .child(
                                svg()
                                    .path("icons/lock.svg")
                                    .w(px(14.0))
                                    .h(px(14.0))
                                    .text_color(rgb(theme.text_muted)),
                            )
                            .child(t!("form.keyring_hint").to_string()),
                    ),
            )
            .child(match &self.test_status {
                TestStatus::Failed(error) => self
                    .render_selectable_text("form-error", error, theme.danger, cx)
                    .into_any_element(),
                _ => div().into_any_element(),
            })
            .into_any_element()
    }

    // ----- TLS / SSL ---------------------------------------------------------------------------

    fn render_tls_page(
        &self,
        form: &ConnectionForm,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let extra = |field: FormExtra| -> AnyElement {
            match self.form_extra_inputs.get(&field).cloned() {
                Some(input) => div().w_full().child(input).into_any_element(),
                None => div().into_any_element(),
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(form_field(
                t!("form.tls.mode").to_string(),
                false,
                None,
                self.form_select(cx),
                theme,
            ))
            .child(form_field(
                t!("form.tls.ca").to_string(),
                false,
                None,
                file_row(FormExtra::TlsCa, theme, extra(FormExtra::TlsCa), cx),
                theme,
            ))
            .child(form_field(
                t!("form.tls.client_cert").to_string(),
                false,
                None,
                file_row(FormExtra::TlsCert, theme, extra(FormExtra::TlsCert), cx),
                theme,
            ))
            .child(form_field(
                t!("form.tls.client_key").to_string(),
                false,
                None,
                file_row(FormExtra::TlsKey, theme, extra(FormExtra::TlsKey), cx),
                theme,
            ))
            .child(
                div()
                    .id("form-tls-cleartext")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        if let Some(form) = this.form.as_mut() {
                            form.settings.cleartext_password = !form.settings.cleartext_password;
                        }
                        cx.notify();
                    }))
                    .child(checkbox_box(form.settings.cleartext_password, theme))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(t!("form.tls.cleartext").to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("form.tls.cleartext_sub").to_string()),
                            ),
                    ),
            )
    }

    /// The custom select that shows the TLS mode and opens its option menu.
    fn form_select(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mode = self
            .form
            .as_ref()
            .map(|form| form.settings.tls.mode)
            .unwrap_or_default();
        let anchor = self.form_select_anchor.clone();

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px_4()
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg))
            .h(px(32.0))
            .on_children_prepainted(move |bounds, _window, _cx| {
                if let (Some(first), Some(last)) = (bounds.first(), bounds.last()) {
                    *anchor.borrow_mut() = Point::new(first.left(), last.bottom());
                }
            })
            .id("form-tls-select")
            .cursor_pointer()
            .hover(move |style| style.border_color(rgb(theme.button_default_border)))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.form_select_open = !this.form_select_open;
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(13.0))
                    .child(t!(mode.label_key()).to_string()),
            )
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .w(px(15.0))
                    .h(px(15.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            )
            .into_any_element()
    }

    /// The TLS mode option menu.
    fn render_select_menu(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let anchor = *self.form_select_anchor.borrow();
        let current = self
            .form
            .as_ref()
            .map(|form| form.settings.tls.mode)
            .unwrap_or_default();

        let mut items = div().flex().flex_col().w(px(300.0)).p_1();
        for mode in TlsMode::all() {
            let selected = current == mode;
            items = items.child(
                div()
                    .id(SharedString::from(format!("form-tls-{}", mode.label_key())))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(32.0))
                    .px_3()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_tls_mode(mode, cx);
                    }))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .child(t!(mode.label_key()).to_string()),
                    )
                    .when(selected, |row| {
                        row.child(
                            svg()
                                .path("icons/check.svg")
                                .w(px(14.0))
                                .h(px(14.0))
                                .text_color(rgb(theme.brand)),
                        )
                    }),
            );
        }

        ui::popup_panel(theme)
            .left(anchor.x)
            .top(anchor.y + px(4.0))
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.form_select_open = false;
                cx.notify();
            }))
            .child(items)
    }

    fn set_tls_mode(&mut self, mode: TlsMode, cx: &mut Context<'_, Self>) {
        if let Some(form) = self.form.as_mut() {
            form.settings.tls.mode = mode;
        }
        self.form_select_open = false;
        cx.notify();
    }

    /// Open the native file picker for one of the TLS path fields.
    fn browse_form_file(&mut self, field: FormExtra, cx: &mut Context<'_, Self>) {
        let title = match field {
            FormExtra::TlsCa => t!("form.tls.ca").to_string(),
            FormExtra::TlsCert => t!("form.tls.client_cert").to_string(),
            FormExtra::TlsKey => t!("form.tls.client_key").to_string(),
            _ => return,
        };
        cx.spawn(async move |this, cx| {
            let Some(handle) = AsyncFileDialog::new().set_title(title).pick_file().await else {
                return;
            };
            let path = handle.path().to_string_lossy().into_owned();
            let _ = this.update(cx, move |app, cx| {
                app.set_form_extra(field, &path, cx);
                if let Some(input) = app.form_extra_inputs.get(&field).cloned() {
                    input.update(cx, |input, cx| input.set_text(path.clone(), cx));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Open the native file picker for a file-based engine's database (the SQLite file).
    fn browse_database_file(&mut self, cx: &mut Context<'_, Self>) {
        let title = t!("form.database_file").to_string();
        cx.spawn(async move |this, cx| {
            let Some(handle) = AsyncFileDialog::new()
                .set_title(title)
                .add_filter("SQLite", &["db", "sqlite", "sqlite3"])
                .pick_file()
                .await
            else {
                return;
            };
            let path = handle.path().to_string_lossy().into_owned();
            let _ = this.update(cx, move |app, cx| {
                app.set_form_field(FormField::Database, &path, cx);
                if let Some(input) = app
                    .form_inputs
                    .as_ref()
                    .map(|inputs| inputs.get(FormField::Database).clone())
                {
                    input.update(cx, |input, cx| input.set_text(path.clone(), cx));
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ----- 隧道 / 代理 -------------------------------------------------------------------------

    fn render_tunnel_page(
        &self,
        form: &ConnectionForm,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let kind = form.settings.tunnel.first().map(|layer| layer.kind);

        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(hop_chain(form, self.relay_label(), theme))
            .child(form_field(
                t!("form.tunnel.relay_mode").to_string(),
                false,
                None,
                self.relay_segmented(kind, cx),
                theme,
            ));

        match kind {
            None => {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .w_full()
                        .py_8()
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(rgb(theme.border))
                        .child(
                            svg()
                                .path("icons/zap.svg")
                                .w(px(20.0))
                                .h(px(20.0))
                                .text_color(rgb(theme.icon_connection)),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .child(t!("form.tunnel.direct_title").to_string()),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(theme.text_muted))
                                .child(t!("form.tunnel.direct_hint").to_string()),
                        ),
                );
            }
            Some(_) => {
                body = body.child(self.render_tunnel_layer(0, cx));
            }
        }

        body
    }

    /// The relay-mode segmented control.
    fn relay_segmented(
        &self,
        current: Option<TunnelKind>,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let options: [(Option<TunnelKind>, &'static str); 4] = [
            (None, "form.tunnel.direct"),
            (Some(TunnelKind::Ssh), "form.tunnel.ssh"),
            (Some(TunnelKind::Http), "form.tunnel.http"),
            (Some(TunnelKind::Socks5), "form.tunnel.socks5"),
        ];

        let mut track = div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .rounded(px(9.0))
            .bg(rgb(theme.tree_hover_bg))
            .p_1();

        for (kind, key) in options {
            let active = current == kind;
            track = track.child(
                div()
                    .id(SharedString::from(format!("form-relay-{}", key)))
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .h(px(32.0))
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .text_color(rgb(if active { theme.text } else { theme.text_muted }))
                    .font_weight(if active {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .when(active, move |style| {
                        style
                            .bg(rgb(theme.dialog_bg))
                            .border_1()
                            .border_color(rgb(theme.border))
                    })
                    .when(!active, move |style| {
                        style.hover(move |style| style.text_color(rgb(theme.text)))
                    })
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_relay_kind(kind, cx);
                    }))
                    .child(t!(key).to_string()),
            );
        }

        track
    }

    fn set_relay_kind(&mut self, kind: Option<TunnelKind>, cx: &mut Context<'_, Self>) {
        if let Some(form) = self.form.as_mut() {
            match kind {
                None => form.settings.tunnel.clear(),
                Some(kind) => {
                    if form.settings.tunnel.first().map(|layer| layer.kind) != Some(kind) {
                        form.settings.tunnel = vec![TunnelLayer::new(kind)];
                    }
                }
            }
        }
        self.rebuild_tunnel_inputs(cx);
        self.form_tunnel_select_open = false;
        cx.notify();
    }

    fn render_tunnel_layer(&self, index: usize, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let layer = self
            .form
            .as_ref()
            .and_then(|form| form.settings.tunnel.get(index));
        let kind = layer.map(|layer| layer.kind).unwrap_or(TunnelKind::Ssh);
        let auth = layer.map(|layer| layer.auth).unwrap_or_default();
        let inputs = self.form_tunnel_inputs.get(index);
        let error_hint = |field: TunnelField| -> Option<AnyElement> {
            self.form_tunnel_errors.contains(&field).then(|| {
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.danger))
                    .child(t!("form.required").to_string())
                    .into_any_element()
            })
        };

        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .rounded(px(10.0))
            .border_1()
            .border_color(rgb(theme.border));

        if let Some(inputs) = inputs {
            let host_label = match kind {
                TunnelKind::Ssh => t!("form.tunnel.bastion_host"),
                TunnelKind::Http => t!("form.tunnel.http_host"),
                TunnelKind::Socks5 => t!("form.tunnel.socks5_host"),
            };
            let user_label = if kind == TunnelKind::Ssh {
                t!("form.tunnel.ssh_user")
            } else {
                t!("form.tunnel.proxy_user")
            };
            let password_label = if kind == TunnelKind::Ssh {
                t!("form.tunnel.auth.password")
            } else {
                t!("form.tunnel.proxy_password")
            };

            // Row 1: host + port, sharing one row for a compact layout.
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(div().flex_1().min_w(px(0.0)).child(form_field(
                        host_label.to_string(),
                        true,
                        error_hint(TunnelField::Host),
                        div().w_full().child(inputs.host.clone()),
                        theme,
                    )))
                    .child(div().w(px(150.0)).flex_none().child(form_field(
                        t!("form.port").to_string(),
                        true,
                        error_hint(TunnelField::Port),
                        div().w_full().child(inputs.port.clone()),
                        theme,
                    ))),
            );

            // Row 2: username + auth method (SSH) or username + password (proxy kinds).
            let username_field = form_field(
                user_label.to_string(),
                false,
                None,
                div().w_full().child(inputs.username.clone()),
                theme,
            );
            let second_control = if kind == TunnelKind::Ssh {
                form_field(
                    t!("form.tunnel.auth_method").to_string(),
                    false,
                    None,
                    self.tunnel_auth_select(cx),
                    theme,
                )
                .into_any_element()
            } else {
                form_field(
                    password_label.to_string(),
                    false,
                    None,
                    div().w_full().child(inputs.password.clone()),
                    theme,
                )
                .into_any_element()
            };
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(username_field)
                            .into_any_element(),
                    )
                    .child(div().flex_1().min_w(px(0.0)).child(second_control)),
            );

            // Row 3 (SSH only): password or private key path, depending on the auth method.
            if kind == TunnelKind::Ssh {
                if auth == TunnelAuth::KeyFile {
                    let key_index = index;
                    body = body.child(form_field(
                        t!("form.tunnel.key_path").to_string(),
                        true,
                        error_hint(TunnelField::KeyPath),
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(div().flex_1().min_w(px(0.0)).child(inputs.key_path.clone()))
                            .child(
                                div()
                                    .id("form-tunnel-key-browse")
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_center()
                                    .gap_2()
                                    .h(px(32.0))
                                    .px_4()
                                    .flex_none()
                                    .rounded(px(8.0))
                                    .border_1()
                                    .border_color(rgb(theme.border))
                                    .bg(rgb(theme.dialog_bg))
                                    .cursor_pointer()
                                    .text_size(px(12.5))
                                    .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.browse_tunnel_file(key_index, cx);
                                    }))
                                    .child(
                                        svg()
                                            .path("icons/folder.svg")
                                            .w(px(14.0))
                                            .h(px(14.0))
                                            .text_color(rgb(theme.text_muted)),
                                    )
                                    .child(t!("form.browse").to_string()),
                            ),
                        theme,
                    ));
                } else {
                    body = body.child(form_field(
                        password_label.to_string(),
                        false,
                        None,
                        div().w_full().child(inputs.password.clone()),
                        theme,
                    ));
                }
            }
        }

        body
    }

    /// The tunnel page's SSH auth-method select (the trigger; the menu is drawn separately).
    fn tunnel_auth_select(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let auth = self
            .form
            .as_ref()
            .and_then(|form| form.settings.tunnel.first())
            .map(|layer| layer.auth)
            .unwrap_or_default();
        let anchor = self.form_tunnel_select_anchor.clone();

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .px_4()
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.input_bg))
            .h(px(32.0))
            .on_children_prepainted(move |bounds, _window, _cx| {
                if let (Some(first), Some(last)) = (bounds.first(), bounds.last()) {
                    *anchor.borrow_mut() = Point::new(first.left(), last.bottom());
                }
            })
            .id("form-tunnel-auth-select")
            .cursor_pointer()
            .hover(move |style| style.border_color(rgb(theme.button_default_border)))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.form_tunnel_select_open = !this.form_tunnel_select_open;
                cx.notify();
            }))
            .child(
                div()
                    .text_size(px(13.0))
                    .child(t!(auth.label_key()).to_string()),
            )
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .w(px(15.0))
                    .h(px(15.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            )
            .into_any_element()
    }

    /// The SSH auth-method option menu.
    fn render_tunnel_auth_menu(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let anchor = *self.form_tunnel_select_anchor.borrow();
        let current = self
            .form
            .as_ref()
            .and_then(|form| form.settings.tunnel.first())
            .map(|layer| layer.auth)
            .unwrap_or_default();

        let mut items = div().flex().flex_col().w(px(260.0)).p_1();
        for auth in TunnelAuth::all() {
            let selected = current == auth;
            items = items.child(
                div()
                    .id(SharedString::from(format!(
                        "form-tunnel-auth-{}",
                        auth.label_key()
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(32.0))
                    .px_3()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.set_tunnel_auth(auth, cx);
                    }))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .child(t!(auth.label_key()).to_string()),
                    )
                    .when(selected, |row| {
                        row.child(
                            svg()
                                .path("icons/check.svg")
                                .w(px(14.0))
                                .h(px(14.0))
                                .text_color(rgb(theme.brand)),
                        )
                    }),
            );
        }

        ui::popup_panel(theme)
            .left(anchor.x)
            .top(anchor.y + px(4.0))
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.form_tunnel_select_open = false;
                cx.notify();
            }))
            .child(items)
    }

    fn set_tunnel_auth(&mut self, auth: TunnelAuth, cx: &mut Context<'_, Self>) {
        if let Some(form) = self.form.as_mut()
            && let Some(layer) = form.settings.tunnel.first_mut()
        {
            layer.auth = auth;
        }
        self.form_tunnel_select_open = false;
        cx.notify();
    }

    /// Open the native file picker for a tunnel layer's SSH private key.
    fn browse_tunnel_file(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let title = t!("form.tunnel.key_path").to_string();
        cx.spawn(async move |this, cx| {
            let Some(handle) = AsyncFileDialog::new().set_title(title).pick_file().await else {
                return;
            };
            let path = handle.path().to_string_lossy().into_owned();
            let _ = this.update(cx, move |app, cx| {
                app.set_tunnel_field(index, TunnelField::KeyPath, &path, cx);
                if let Some(input) = app
                    .form_tunnel_inputs
                    .get(index)
                    .map(|inputs| inputs.key_path.clone())
                {
                    input.update(cx, |input, cx| input.set_text(path.clone(), cx));
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ----- 高级 --------------------------------------------------------------------------------

    fn render_advanced_page(
        &self,
        form: &ConnectionForm,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let settings = &form.settings;
        let extra = |field: FormExtra| -> AnyElement {
            match self.form_extra_inputs.get(&field).cloned() {
                Some(input) => div()
                    .w(px(90.0))
                    .h(px(32.0))
                    .child(input)
                    .into_any_element(),
                None => div().into_any_element(),
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("form.advanced.section_timeout").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(advanced_card(
                        t!("form.advanced.connect_timeout").to_string(),
                        t!("form.advanced.connect_hint").to_string(),
                        seconds_input(extra(FormExtra::ConnectTimeout), theme),
                        theme,
                    ))
                    .child(advanced_card(
                        t!("form.advanced.query_timeout").to_string(),
                        t!("form.advanced.query_hint").to_string(),
                        seconds_input(extra(FormExtra::QueryTimeout), theme),
                        theme,
                    )),
            )
            .child(advanced_card(
                t!("form.advanced.keepalive").to_string(),
                t!("form.advanced.keepalive_hint").to_string(),
                seconds_input(extra(FormExtra::KeepaliveInterval), theme),
                theme,
            ))
            .child(
                div()
                    .id("form-read-only")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        if let Some(form) = this.form.as_mut() {
                            form.settings.read_only = !form.settings.read_only;
                        }
                        cx.notify();
                    }))
                    .child(checkbox_box(settings.read_only, theme))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(t!("form.advanced.read_only").to_string()),
                                    )
                                    .child(small_pill(
                                        t!("form.advanced.guard").to_string(),
                                        theme,
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("form.advanced.read_only_hint").to_string()),
                            ),
                    ),
            )
            .child(form_field(
                t!("form.advanced.init_sql").to_string(),
                false,
                Some(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("form.advanced.init_sql_hint").to_string())
                        .into_any_element(),
                ),
                div()
                    .w_full()
                    .h(px(32.0))
                    .font_family("Consolas")
                    .child(self.extra_input(FormExtra::InitSql)),
                theme,
            ))
    }

    fn extra_input(&self, field: FormExtra) -> AnyElement {
        match self.form_extra_inputs.get(&field).cloned() {
            Some(input) => input.into_any_element(),
            None => div().into_any_element(),
        }
    }

    // ----- Footer ------------------------------------------------------------------------------

    fn form_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let editing = self.editing.is_some();
        let busy = matches!(self.test_status, TestStatus::Testing);

        let mut status = div().flex().flex_row().items_center().gap_2().flex_none();
        match &self.test_status {
            TestStatus::Idle => {}
            TestStatus::Testing => {
                status = status.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("form.testing").to_string()),
                );
            }
            TestStatus::Success => {
                status = status.child(
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
            .h(px(48.0))
            .px_6()
            .child(ghost_button(
                "form-reset",
                t!("form.reset").to_string(),
                theme,
                cx.listener(|this, _event, _window, cx| this.reset_form(cx)),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(status)
                    .child(outline_icon_button(
                        "form-test",
                        "icons/activity.svg",
                        t!("form.test_connection").to_string(),
                        theme,
                        cx.listener(|this, _event, _window, cx| {
                            if !matches!(this.test_status, TestStatus::Testing) {
                                this.test_form(cx);
                            }
                        }),
                    ))
                    .child(primary_icon_button(
                        "form-save",
                        "icons/play.svg",
                        if editing {
                            t!("form.save").to_string()
                        } else {
                            t!("form.save_connect").to_string()
                        },
                        theme,
                        cx.listener(move |this, _event, _window, cx| {
                            if !busy {
                                this.save_form(cx);
                            }
                        }),
                    )),
            )
    }

    /// Restore the form to the values it had when the window opened.
    fn reset_form(&mut self, cx: &mut Context<'_, Self>) {
        let Some(initial) = self.form_initial.clone() else {
            return;
        };
        self.form = Some(initial);
        self.test_status = TestStatus::Idle;
        self.form_errors.clear();
        self.form_tunnel_errors.clear();
        self.form_select_open = false;
        self.form_tunnel_select_open = false;
        self.rebuild_form_extra_inputs(cx);
        self.rebuild_tunnel_inputs(cx);

        let values: Vec<(FormField, String)> = self
            .form
            .as_ref()
            .map(|form| {
                FORM_FIELDS
                    .iter()
                    .map(|field| (*field, form.field_value(*field).to_string()))
                    .collect()
            })
            .unwrap_or_default();
        for (field, value) in values {
            if let Some(input) = self
                .form_inputs
                .as_ref()
                .map(|inputs| inputs.get(field).clone())
            {
                input.update(cx, |input, cx| input.set_text(value, cx));
            }
        }
        cx.notify();
    }

    // ----- Inputs ------------------------------------------------------------------------------

    /// Build the TLS / advanced text inputs (once per form open).
    pub(super) fn rebuild_form_extra_inputs(&mut self, cx: &mut Context<'_, Self>) {
        self.form_extra_inputs.clear();
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let theme = self.theme;
        let weak = cx.weak_entity();

        let tls = form.settings.tls.clone();
        let connect_timeout = form
            .settings
            .connect_timeout
            .map(|value| value.to_string())
            .unwrap_or_default();
        let query_timeout = form
            .settings
            .query_timeout
            .map(|value| value.to_string())
            .unwrap_or_default();
        let keepalive = form
            .settings
            .keepalive
            .map(|value| value.to_string())
            .unwrap_or_default();
        let init_sql = form.settings.init_sql.clone();

        let entries = [
            (FormExtra::TlsCa, tls.ca, "", "form.tls.ca_placeholder"),
            (
                FormExtra::TlsCert,
                tls.cert,
                "",
                "form.tls.cert_placeholder",
            ),
            (FormExtra::TlsKey, tls.key, "", "form.tls.key_placeholder"),
            (FormExtra::ConnectTimeout, connect_timeout, "", ""),
            (FormExtra::QueryTimeout, query_timeout, "", ""),
            (FormExtra::KeepaliveInterval, keepalive, "", ""),
            (FormExtra::InitSql, init_sql, "", ""),
        ];

        for (field, value, _icon, placeholder_key) in entries {
            let placeholder = if placeholder_key.is_empty() {
                String::new()
            } else {
                t!(placeholder_key).to_string()
            };
            let input = make_extra_input(theme, value, placeholder, field, &weak, cx);
            self.form_extra_inputs.insert(field, input);
        }
    }

    /// Build the tunnel layer inputs to match the current chain.
    pub(super) fn rebuild_tunnel_inputs(&mut self, cx: &mut Context<'_, Self>) {
        self.form_tunnel_inputs.clear();
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let theme = self.theme;
        let weak = cx.weak_entity();
        let layers: Vec<_> = form.settings.tunnel.clone();
        for (index, layer) in layers.into_iter().enumerate() {
            let (host_placeholder, user_placeholder, password_placeholder) =
                if layer.kind == TunnelKind::Ssh {
                    (
                        String::new(),
                        String::new(),
                        t!("form.tunnel.password_placeholder").to_string(),
                    )
                } else {
                    (
                        t!("form.tunnel.proxy_host_placeholder").to_string(),
                        t!("form.tunnel.anonymous_hint").to_string(),
                        t!("form.tunnel.anonymous_hint").to_string(),
                    )
                };
            let host = make_tunnel_input(
                theme,
                layer.host,
                host_placeholder,
                index,
                TunnelField::Host,
                &weak,
                cx,
            );
            let port = make_tunnel_input(
                theme,
                layer.port.to_string(),
                String::new(),
                index,
                TunnelField::Port,
                &weak,
                cx,
            );
            let username = make_tunnel_input(
                theme,
                layer.username,
                user_placeholder,
                index,
                TunnelField::Username,
                &weak,
                cx,
            );
            let password = make_tunnel_input(
                theme,
                layer.password,
                password_placeholder,
                index,
                TunnelField::Password,
                &weak,
                cx,
            );
            let key_path = make_tunnel_input(
                theme,
                layer.key_path,
                String::new(),
                index,
                TunnelField::KeyPath,
                &weak,
                cx,
            );
            self.form_tunnel_inputs.push(TunnelInputs {
                host,
                port,
                username,
                password,
                key_path,
            });
        }
    }

    pub(super) fn set_form_extra(
        &mut self,
        field: FormExtra,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let value = text.trim().to_string();
        match field {
            FormExtra::TlsCa => form.settings.tls.ca = value,
            FormExtra::TlsCert => form.settings.tls.cert = value,
            FormExtra::TlsKey => form.settings.tls.key = value,
            FormExtra::ConnectTimeout => {
                form.settings.connect_timeout = value.parse().ok().filter(|v| *v > 0)
            }
            FormExtra::QueryTimeout => {
                form.settings.query_timeout = value.parse().ok().filter(|v| *v > 0)
            }
            FormExtra::KeepaliveInterval => {
                form.settings.keepalive = value.parse().ok().filter(|v| *v > 0)
            }
            FormExtra::InitSql => form.settings.init_sql = text.to_string(),
        }
        cx.notify();
    }

    pub(super) fn set_tunnel_field(
        &mut self,
        index: usize,
        field: TunnelField,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let Some(layer) = form.settings.tunnel.get_mut(index) else {
            return;
        };
        match field {
            TunnelField::Host => layer.host = text.to_string(),
            TunnelField::Port => {
                // An empty or unparseable field clears the port (0), so validation can flag it
                // instead of silently keeping the previous value.
                layer.port = text.trim().parse().unwrap_or(0);
            }
            TunnelField::Username => layer.username = text.to_string(),
            TunnelField::Password => layer.password = text.to_string(),
            TunnelField::KeyPath => layer.key_path = text.to_string(),
        }
        // Editing a flagged tunnel field clears its "required" marker.
        self.form_tunnel_errors.remove(&field);
        cx.notify();
    }
}

// ----- Shared chrome ---------------------------------------------------------------------------

/// A labelled field: label (with an optional `*` and trailing hint) above the control.
fn form_field(
    label: String,
    required: bool,
    hint: Option<AnyElement>,
    control: impl IntoElement,
    theme: Theme,
) -> impl IntoElement {
    let mut label_row = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                )
                .when(required, move |row| {
                    row.child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(theme.danger))
                            .child("*"),
                    )
                }),
        );
    if let Some(hint) = hint {
        label_row = label_row.child(hint);
    }

    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .child(label_row)
        .child(control)
}

/// A large text input with a trailing 浏览 button.
fn file_row(
    field: FormExtra,
    theme: Theme,
    input: AnyElement,
    cx: &mut Context<'_, AppView>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .child(div().flex_1().min_w(px(0.0)).child(input))
        .child(
            div()
                .id(SharedString::from(format!("{}-browse", field.key())))
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap_2()
                .h(px(32.0))
                .px_4()
                .flex_none()
                .rounded(px(8.0))
                .border_1()
                .border_color(rgb(theme.border))
                .bg(rgb(theme.dialog_bg))
                .cursor_pointer()
                .text_size(px(12.5))
                .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.browse_form_file(field, cx);
                }))
                .child(
                    svg()
                        .path("icons/folder.svg")
                        .w(px(14.0))
                        .h(px(14.0))
                        .text_color(rgb(theme.text_muted)),
                )
                .child(t!("form.browse").to_string()),
        )
}

/// A text input with a trailing 浏览 button for a file-based engine's database file.
fn database_file_row(
    theme: Theme,
    input: AnyElement,
    cx: &mut Context<'_, AppView>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .child(div().flex_1().min_w(px(0.0)).child(input))
        .child(
            div()
                .id("form-database-browse")
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap_2()
                .h(px(32.0))
                .px_4()
                .flex_none()
                .rounded(px(8.0))
                .border_1()
                .border_color(rgb(theme.border))
                .bg(rgb(theme.dialog_bg))
                .cursor_pointer()
                .text_size(px(12.5))
                .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.browse_database_file(cx);
                }))
                .child(
                    svg()
                        .path("icons/folder.svg")
                        .w(px(14.0))
                        .h(px(14.0))
                        .text_color(rgb(theme.text_muted)),
                )
                .child(t!("form.browse").to_string()),
        )
}

/// The hop-chain visual on the tunnel page.
fn hop_chain(form: &ConnectionForm, relay: String, theme: Theme) -> impl IntoElement {
    let target = format!("{}:{}", form.host.trim(), form.port.trim());
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .w_full()
        .px_3()
        .py_2()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgb(theme.border))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child(
                    svg()
                        .path("icons/monitor.svg")
                        .w(px(14.0))
                        .h(px(14.0))
                        .text_color(rgb(theme.text_muted)),
                )
                .child(t!("form.tunnel.client").to_string()),
        )
        .child(chain_arrow(theme))
        .child(small_pill(relay, theme))
        .child(chain_arrow(theme))
        .child(
            div()
                .flex()
                .items_center()
                .h(px(24.0))
                .px_3()
                .rounded(px(6.0))
                .bg(rgb(theme.primary))
                .text_size(px(11.5))
                .text_color(rgb(theme.dialog_bg))
                .child(target),
        )
}

fn chain_arrow(theme: Theme) -> impl IntoElement {
    div().flex().items_center().child(
        svg()
            .path("icons/arrow-right.svg")
            .w(px(14.0))
            .h(px(14.0))
            .text_color(rgb(theme.text_muted)),
    )
}

/// A small muted pill (tab badge, relay label).
fn small_pill(text: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .h(px(20.0))
        .px_2()
        .rounded(px(5.0))
        .bg(rgb(theme.tree_hover_bg))
        .text_size(px(11.0))
        .text_color(rgb(theme.text_muted))
        .child(text)
}

/// An advanced-settings card: a title, a hint and a trailing control.
fn advanced_card(
    title: String,
    hint: String,
    control: impl IntoElement,
    theme: Theme,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_3()
        .flex_1()
        .px_3()
        .py_2()
        .rounded(px(10.0))
        .border_1()
        .border_color(rgb(theme.border))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(theme.text_muted))
                        .child(hint),
                ),
        )
        .child(control)
}

/// A number input followed by a `s` unit.
fn seconds_input(input: AnyElement, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .flex_none()
        .child(input)
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child(t!("form.advanced.seconds").to_string()),
        )
}

/// A plain text footer button (重置).
fn ghost_button(
    id: &'static str,
    label: String,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(28.0))
        .px_3()
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(px(12.5))
        .text_color(rgb(theme.text_muted))
        .hover(move |style| {
            style
                .bg(rgb(theme.tree_hover_bg))
                .text_color(rgb(theme.text))
        })
        .on_click(on_click)
        .child(label)
}

/// An outline footer button with a leading icon (测试连接).
fn outline_icon_button(
    id: &'static str,
    icon_path: &'static str,
    label: String,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(28.0))
        .px_3()
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.dialog_bg))
        .cursor_pointer()
        .text_size(px(12.5))
        .text_color(rgb(theme.text))
        .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
        .on_click(on_click)
        .child(
            svg()
                .path(icon_path)
                .w(px(14.0))
                .h(px(14.0))
                .text_color(rgb(theme.text)),
        )
        .child(label)
}

/// The dark primary footer button (保存并连接).
fn primary_icon_button(
    id: &'static str,
    icon_path: &'static str,
    label: String,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(28.0))
        .px_3()
        .rounded(px(6.0))
        .bg(rgb(theme.primary))
        .cursor_pointer()
        .text_size(px(12.5))
        .text_color(rgb(theme.dialog_bg))
        .hover(move |style| style.opacity(0.9))
        .on_click(on_click)
        .child(
            svg()
                .path(icon_path)
                .w(px(14.0))
                .h(px(14.0))
                .text_color(rgb(theme.dialog_bg)),
        )
        .child(label)
}

fn make_extra_input(
    theme: Theme,
    value: String,
    placeholder: String,
    field: FormExtra,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    cx.new(move |cx| {
        TextInput::new(
            theme,
            value,
            TextInputOptions {
                placeholder: placeholder.into(),
                size: Some(gpui_kit::component::Size::Medium),
                text_size: Some(13.0),
                ..Default::default()
            },
            cx,
        )
        .on_change(Rc::new(move |text, _window, cx| {
            let _ = change.update(cx, |app, cx| app.set_form_extra(field, text, cx));
        }))
    })
}

fn make_tunnel_input(
    theme: Theme,
    value: String,
    placeholder: String,
    index: usize,
    field: TunnelField,
    app: &WeakEntity<AppView>,
    cx: &mut Context<'_, AppView>,
) -> Entity<TextInput> {
    let change = app.clone();
    cx.new(move |cx| {
        TextInput::new(
            theme,
            value,
            TextInputOptions {
                placeholder: placeholder.into(),
                size: Some(gpui_kit::component::Size::Medium),
                text_size: Some(13.0),
                ..Default::default()
            },
            cx,
        )
        .on_change(Rc::new(move |text, _window, cx| {
            let _ = change.update(cx, |app, cx| app.set_tunnel_field(index, field, text, cx));
        }))
    })
}
