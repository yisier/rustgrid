use super::*;

fn language_label(setting: LanguageSetting) -> String {
    match setting {
        LanguageSetting::En => "English".to_string(),
        LanguageSetting::ZhCn => "简体中文".to_string(),
    }
}

impl AppView {
    pub(super) fn open_options(&mut self, cx: &mut Context<'_, Self>) {
        self.options_open = true;
        self.options_theme = self.theme_setting;
        self.options_language = self.language;
        self.options_language_open = false;
        cx.notify();
    }

    pub(super) fn close_options(&mut self, cx: &mut Context<'_, Self>) {
        self.options_open = false;
        self.options_language_open = false;
        cx.notify();
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

    fn language_combo(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let current = self.options_language;
        let open = self.options_language_open;

        let mut list = div()
            .id("options-lang-list")
            .absolute()
            .top(px(24.0))
            .left_0()
            .w(px(220.0))
            .flex()
            .flex_col()
            .py_0p5()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border));
        for setting in [LanguageSetting::ZhCn, LanguageSetting::En] {
            let selected = setting == current;
            let label = language_label(setting);
            let option_id = SharedString::from(format!("options-lang-{label}"));
            list = list.child(
                div()
                    .id(option_id)
                    .flex()
                    .items_center()
                    .h(px(22.0))
                    .px_2()
                    .flex_none()
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(selected, move |style| {
                        style
                            .bg(rgb(theme.tree_selected_bg))
                            .text_color(rgb(theme.tree_selected_text))
                    })
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.options_language = setting;
                        this.options_language_open = false;
                        cx.notify();
                    }))
                    .child(label),
            );
        }

        let combo_box = ui::text_field(theme)
            .id("options-lang-combo")
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w(px(220.0))
            .h(px(24.0))
            .px_2()
            .text_size(px(12.0))
            .cursor_pointer()
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.options_language_open = !this.options_language_open;
                cx.notify();
            }))
            .child(language_label(current))
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );

        div().relative().child(combo_box).when(open, move |style| {
            style.child(deferred(list).with_priority(10))
        })
    }

    pub(super) fn render_options_dialog(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;

        let header = ui::dialog_header(theme)
            .px_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        svg()
                            .path("icons/gear.svg")
                            .w(px(14.0))
                            .h(px(14.0))
                            .flex_none()
                            .text_color(rgb(theme.text)),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .child(t!("options.title").to_string()),
                    ),
            )
            .child(self.dialog_close_button(
                "options-close",
                cx.listener(|this, _event, _window, cx| this.close_options(cx)),
            ));

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

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(48.0))
            .px_3()
            .border_t_1()
            .border_color(rgb(theme.border))
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
            );

        ui::overlay(theme).child(
            ui::dialog_frame(theme, theme.dialog_bg)
                .w(px(640.0))
                .child(header)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .h(px(400.0))
                        .child(nav)
                        .child(content),
                )
                .child(footer),
        )
    }
}
