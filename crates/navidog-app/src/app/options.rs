use super::*;
use gpui_kit::component::WindowExt;

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

impl AppView {
    pub(super) fn open_options(&mut self, cx: &mut Context<'_, Self>) {
        self.options_open = true;
        self.options_theme = self.theme_setting;
        self.options_language = self.language;
        self.ensure_language_combo(cx);
        cx.notify();
    }

    pub(super) fn close_options(&mut self, cx: &mut Context<'_, Self>) {
        self.options_open = false;
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

    fn apply_options(&mut self, cx: &mut Context<'_, Self>) {
        let theme = self.options_theme;
        let language = self.options_language;
        self.set_theme(theme, cx);
        self.set_language(language, cx);
        self.close_options(cx);
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

    fn theme_option(
        &self,
        id: &'static str,
        setting: ThemeSetting,
        label: String,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let kind = if self.options_theme == setting {
            ButtonKind::Selected
        } else {
            ButtonKind::Normal
        };
        self.win_button(
            id,
            label,
            kind,
            cx.listener(move |this, _event, _window, cx| {
                this.options_theme = setting;
                cx.notify();
            }),
        )
    }

    fn language_combo(&self, _cx: &mut Context<'_, Self>) -> AnyElement {
        match self.language_combo.clone() {
            Some(combo) => combo.into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// Opens the options dialog as a `Root`-managed modal.
    pub(super) fn open_options_dialog(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let app = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let footer = app.update(cx, |app, cx| {
                app.options_dialog_footer(cx).into_any_element()
            });
            let on_close = app.downgrade();
            let content_app = app.clone();
            dialog
                .title(t!("options.title").to_string())
                .w(px(640.0))
                .content(move |content, _window, cx| {
                    let body = content_app
                        .update(cx, |app, cx| app.options_dialog_body(cx).into_any_element());
                    content.child(body)
                })
                .footer(footer)
                .on_close(move |_, _, cx| {
                    let _ = on_close.update(cx, |app, cx| app.close_options(cx));
                })
        });
    }

    /// The options dialog body: the category nav plus the general page.
    fn options_dialog_body(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;

        let nav = div()
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
            .child(self.options_category_row(
                "options-tab",
                t!("options.tab").to_string(),
                false,
                false,
            ))
            .child(self.options_category_row(
                "options-query",
                t!("options.query").to_string(),
                false,
                false,
            ))
            .child(self.options_category_row(
                "options-editor",
                t!("options.editor").to_string(),
                false,
                false,
            ))
            .child(self.options_category_row(
                "options-record",
                t!("options.record").to_string(),
                false,
                false,
            ))
            .child(self.options_category_row(
                "options-file-location",
                t!("options.file_location").to_string(),
                false,
                false,
            ))
            .child(self.options_category_row(
                "options-proxy",
                t!("options.proxy").to_string(),
                false,
                false,
            ))
            .child(self.options_category_row(
                "options-environment",
                t!("options.environment").to_string(),
                false,
                false,
            ))
            .child(self.options_category_row(
                "options-advanced",
                t!("options.advanced").to_string(),
                false,
                false,
            ));

        let content = div()
            .flex()
            .flex_col()
            .flex_1()
            .gap_3()
            .p_4()
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
                            .child(t!("options.theme").to_string()),
                    )
                    .child(self.theme_option(
                        "options-theme-light",
                        ThemeSetting::Light,
                        t!("theme.light").to_string(),
                        cx,
                    ))
                    .child(self.theme_option(
                        "options-theme-dark",
                        ThemeSetting::Dark,
                        t!("theme.dark").to_string(),
                        cx,
                    ))
                    .child(self.theme_option(
                        "options-theme-system",
                        ThemeSetting::System,
                        t!("theme.system").to_string(),
                        cx,
                    )),
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
            .child(div().flex_1());

        div()
            .flex()
            .flex_row()
            .h(px(400.0))
            .child(nav)
            .child(content)
    }

    /// The options dialog footer: reset-to-default on the left, OK/Cancel on the right.
    fn options_dialog_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .h(px(48.0))
            .child(self.dialog_button(
                "options-default",
                t!("options.default").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| {
                    this.options_theme = ThemeSetting::System;
                    this.options_language = LanguageSetting::En;
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
                        cx.listener(|this, _event, _window, cx| this.apply_options(cx)),
                    ))
                    .child(self.dialog_button(
                        "options-cancel",
                        t!("form.cancel").to_string(),
                        false,
                        cx.listener(|this, _event, _window, cx| this.close_options(cx)),
                    )),
            )
            .text_color(rgb(theme.text))
    }
}
