//! The Options window (设置): a separate OS window, opened from the titlebar's settings button.
//!
//! It has a single 常规 page holding just the language selector — the theme is chosen from the
//! titlebar dropdown, so it is not repeated here. Like the Export/Import/User windows, `AppView`
//! owns the state and this module renders it; `AppView::sync_dialog` is not involved.

use super::*;

fn language_label(setting: LanguageSetting) -> String {
    match setting {
        LanguageSetting::En => "English".to_string(),
        LanguageSetting::ZhCn => "简体中文".to_string(),
    }
}

/// The two selectable languages as combo rows, keyed by locale code.
fn language_options() -> Vec<ComboOption> {
    [LanguageSetting::ZhCn, LanguageSetting::En]
        .into_iter()
        .map(|setting| ComboOption::new(setting.locale(), language_label(setting)))
        .collect()
}

fn language_from_locale(locale: &str) -> LanguageSetting {
    if locale == LanguageSetting::En.locale() {
        LanguageSetting::En
    } else {
        LanguageSetting::ZhCn
    }
}

/// The root view of the Options OS window. It re-renders whenever `AppView` changes.
pub(super) struct OptionsWindow {
    app: WeakEntity<AppView>,
    _subscription: Subscription,
}

impl OptionsWindow {
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

impl Render for OptionsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        app.update(cx, |app, cx| app.options_window_contents(cx))
    }
}

impl AppView {
    /// Open the Options window, or raise it if it is already open.
    pub(super) fn open_options(&mut self, cx: &mut Context<'_, Self>) {
        if self.options_window.is_some() {
            self.focus_options_window(cx);
            return;
        }
        self.options_language = self.language;
        self.ensure_language_combo(cx);
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        let focus = self.options_focus.clone();
        let title = t!("options.title").to_string();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(560.0), px(420.0)), cx);
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
                    // `open_window` does not raise what it opens, so the window can otherwise
                    // appear behind the main window.
                    window.activate_window();
                    let view = cx.new(|cx| OptionsWindow::new(view_weak.clone(), &app_entity, cx));
                    let root = cx.new(|cx| gpui_kit::component::Root::new(view, window, cx));
                    // Focus the window root so ESC reaches its handler before any control is
                    // focused.
                    window.focus(&focus, cx);
                    root
                },
            );
            match opened {
                Ok(handle) => app.update(cx, |app, cx| {
                    app.options_window = Some(handle);
                    cx.notify();
                }),
                Err(error) => app.update(cx, |app, cx| {
                    app.error_dialog = Some(error.to_string());
                    cx.notify();
                }),
            }
        });
    }

    /// Raise the already-open Options window.
    fn focus_options_window(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(handle) = self.options_window {
            let _ = handle.update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            });
        }
    }

    /// Close the Options window (called from its footer/ESC, where the window is at hand).
    pub(super) fn close_options(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.options_window = None;
        window.remove_window();
        cx.notify();
    }

    fn ensure_language_combo(&mut self, cx: &mut Context<'_, Self>) {
        if self.language_combo.is_some() {
            return;
        }
        let theme = self.theme;
        let weak = cx.weak_entity();
        let combo = cx.new(|cx| {
            ComboBox::new(theme, language_options(), self.language.locale(), 220.0, cx).on_select(
                Rc::new(move |value, _window, cx| {
                    let _ = weak.update(cx, |app, cx| {
                        app.options_language = language_from_locale(value);
                        cx.notify();
                    });
                }),
            )
        });
        self.language_combo = Some(combo);
    }

    pub(super) fn sync_language_combo(&mut self, cx: &mut Context<'_, Self>) {
        let Some(combo) = self.language_combo.clone() else {
            return;
        };
        let selected = self.options_language.locale().to_string();
        combo.update(cx, |combo, cx| {
            combo.set_options(language_options(), cx);
            combo.set_selected(selected, cx);
        });
    }

    fn apply_options(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let language = self.options_language;
        self.set_language(language, cx);
        self.close_options(window, cx);
    }

    fn options_category_row(
        &self,
        id: &'static str,
        label: String,
        active: bool,
        enabled: bool,
    ) -> impl IntoElement {
        let theme = self.theme;
        let text = if enabled {
            theme.text
        } else {
            theme.text_muted
        };
        div()
            .id(id)
            .flex()
            .items_center()
            .h(px(24.0))
            .px_3()
            .flex_none()
            .text_size(px(12.0))
            .text_color(rgb(text))
            .when(active, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .when(enabled && !active, move |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .child(label)
    }

    fn language_combo(&self, _cx: &mut Context<'_, Self>) -> AnyElement {
        match self.language_combo.clone() {
            Some(combo) => combo.into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// The Options window's contents: a titlebar, the 常规 page and the footer.
    pub(super) fn options_window_contents(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.dialog_face))
            .text_color(rgb(theme.text))
            .track_focus(&self.options_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_options(window, cx);
                }
            }))
            .child(export::child_window_titlebar(
                t!("options.title").to_string(),
                theme,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .child(self.options_nav())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .p_4()
                            .child(self.options_body(cx)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w_full()
                    .px_4()
                    .py_4()
                    .bg(rgb(theme.dialog_bg))
                    .border_t_1()
                    .border_color(rgb(theme.border))
                    .child(self.options_footer(cx)),
            )
            .into_any_element()
    }

    /// The left navigation. Only 常规 remains.
    fn options_nav(&self) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .w(px(150.0))
            .flex_none()
            .py_2()
            .bg(rgb(theme.dialog_face))
            .border_r_1()
            .border_color(rgb(theme.border))
            .child(self.options_category_row(
                "options-general",
                t!("options.general").to_string(),
                true,
                true,
            ))
    }

    /// The 常规 page: just the language selector.
    fn options_body(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(12.5))
                    .child(t!("options.general").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w(px(80.0))
                            .flex_none()
                            .text_size(px(12.0))
                            .child(t!("options.language").to_string()),
                    )
                    .child(self.language_combo(cx)),
            )
    }

    /// The footer: reset-to-default on the left, OK/Cancel on the right.
    fn options_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .child(self.dialog_button(
                "options-default",
                t!("options.default").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| {
                    this.options_language = detect_system_language();
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.dialog_button(
                        "options-ok",
                        t!("form.ok").to_string(),
                        true,
                        cx.listener(|this, _event, window, cx| this.apply_options(window, cx)),
                    ))
                    .child(self.dialog_button(
                        "options-cancel",
                        t!("form.cancel").to_string(),
                        false,
                        cx.listener(|this, _event, window, cx| this.close_options(window, cx)),
                    )),
            )
            .text_color(rgb(theme.text))
    }
}
