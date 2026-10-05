//! The Options window (设置): a separate OS window, opened from the titlebar's settings button.
//!
//! It hosts the 常规 page (the language selector — the app theme is still chosen from the titlebar
//! dropdown) and the 编辑器 page, which configures the SQL editor: a live preview, font family,
//! font size, syntax theme and the line-number / word-wrap toggles. Editor edits are staged and
//! only applied on 确定, so 取消 discards them. Like the Export/Import/User windows, `AppView`
//! owns the state and this module renders it; `AppView::sync_dialog` is not involved.

use super::*;

/// The sentinel combo value for "use the built-in default font".
const DEFAULT_FONT_VALUE: &str = "__default_font__";

/// The monospace families offered by the editor font dropdown. A family that is not installed
/// falls back to gpui's default, so the list can stay static across platforms.
const EDITOR_FONTS: &[&str] = &[
    "Consolas",
    "Cascadia Code",
    "Cascadia Mono",
    "JetBrains Mono",
    "Fira Code",
    "Source Code Pro",
    "IBM Plex Mono",
    "Victor Mono",
    "Menlo",
    "Monaco",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Courier New",
];

/// The font sizes offered by the editor size dropdown, in px.
const EDITOR_FONT_SIZES: &[u32] = &[10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24];

/// The SQL sample shown in the editor page's live preview.
const EDITOR_PREVIEW_SQL: &str = "SELECT u.id, u.name\nFROM users u\nORDER BY u.id LIMIT 5;";

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

/// The editor font families, the built-in default first.
fn editor_font_options() -> Vec<ComboOption> {
    let mut options = vec![ComboOption::new(
        DEFAULT_FONT_VALUE,
        t!("options.editor.font_default").to_string(),
    )];
    options.extend(
        EDITOR_FONTS
            .iter()
            .map(|family| ComboOption::new(*family, *family)),
    );
    options
}

fn font_option_value(family: &str) -> String {
    if family.is_empty() {
        DEFAULT_FONT_VALUE.to_string()
    } else {
        family.to_string()
    }
}

fn font_from_value(value: &str) -> String {
    if value == DEFAULT_FONT_VALUE {
        String::new()
    } else {
        value.to_string()
    }
}

fn editor_size_options() -> Vec<ComboOption> {
    EDITOR_FONT_SIZES
        .iter()
        .map(|size| ComboOption::new(size.to_string(), format!("{size}px")))
        .collect()
}

fn resolve_editor_font(family: &str) -> String {
    if family.is_empty() {
        super::default_editor_font().to_string()
    } else {
        family.to_string()
    }
}

/// A small section heading inside the editor page.
fn section_label(theme: Theme, label: String) -> impl IntoElement {
    div()
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(theme.text))
        .child(label)
}

/// A label above a control.
fn field_label(theme: Theme, label: String, control: AnyElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .child(label),
        )
        .child(control)
}

/// A clickable settings row: a label on the left, a check box on the right.
fn toggle_row(theme: Theme, id: &'static str, label: String, checked: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .w_full()
        .h(px(28.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .child(label),
        )
        .child(checkbox_box(checked, theme))
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
        self.options_section = OptionsSection::General;
        self.options_editor_font_family = self.editor_font_family.clone();
        self.options_editor_font_size = self.editor_font_size;
        self.options_editor_line_numbers = self.editor_line_numbers;
        self.options_editor_word_wrap = self.editor_word_wrap;
        self.ensure_language_combo(cx);
        self.ensure_editor_combos(cx);
        let weak = cx.weak_entity();
        let app_entity = cx.entity();
        let focus = self.options_focus.clone();
        let title = t!("options.title").to_string();
        cx.defer(move |cx: &mut App| {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let bounds = Bounds::centered(None, size(px(620.0), px(500.0)), cx);
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

    /// Build the editor page's three dropdowns on the first open, mirroring `ensure_language_combo`.
    fn ensure_editor_combos(&mut self, cx: &mut Context<'_, Self>) {
        let theme = self.theme;
        if self.editor_font_combo.is_none() {
            let weak = cx.weak_entity();
            let selected = font_option_value(&self.options_editor_font_family);
            let combo = cx.new(|cx| {
                ComboBox::new(theme, editor_font_options(), selected, 200.0, cx)
                    .full_width()
                    .on_select(Rc::new(move |value, _window, cx| {
                        let _ = weak.update(cx, |app, cx| {
                            app.options_editor_font_family = font_from_value(value);
                            cx.notify();
                        });
                    }))
            });
            self.editor_font_combo = Some(combo);
        }
        if self.editor_size_combo.is_none() {
            let weak = cx.weak_entity();
            let selected = self.options_editor_font_size.to_string();
            let combo = cx.new(|cx| {
                ComboBox::new(theme, editor_size_options(), selected, 120.0, cx).on_select(Rc::new(
                    move |value, _window, cx| {
                        let size = value.parse().unwrap_or(DEFAULT_EDITOR_FONT_SIZE);
                        let _ = weak.update(cx, |app, cx| {
                            app.options_editor_font_size = size;
                            cx.notify();
                        });
                    },
                ))
            });
            self.editor_size_combo = Some(combo);
        }
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

    /// Keep the editor dropdowns' rows and selection in step with the staged settings.
    pub(super) fn sync_editor_combos(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(combo) = self.editor_font_combo.clone() {
            let selected = font_option_value(&self.options_editor_font_family);
            combo.update(cx, |combo, cx| {
                combo.set_options(editor_font_options(), cx);
                combo.set_selected(selected, cx);
            });
        }
        if let Some(combo) = self.editor_size_combo.clone() {
            let selected = self.options_editor_font_size.to_string();
            combo.update(cx, |combo, cx| {
                combo.set_options(editor_size_options(), cx);
                combo.set_selected(selected, cx);
            });
        }
    }

    fn apply_options(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.editor_font_family = self.options_editor_font_family.clone();
        self.editor_font_size = self.options_editor_font_size;
        self.editor_line_numbers = self.options_editor_line_numbers;
        self.editor_word_wrap = self.options_editor_word_wrap;
        let language = self.options_language;
        self.set_language(language, cx);
        self.persist_settings();
        self.close_options(window, cx);
    }

    fn options_category_row(
        &self,
        id: &'static str,
        label: String,
        active: bool,
        enabled: bool,
    ) -> Stateful<Div> {
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

    fn editor_font_combo(&self) -> AnyElement {
        match self.editor_font_combo.clone() {
            Some(combo) => combo.into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn editor_size_combo(&self) -> AnyElement {
        match self.editor_size_combo.clone() {
            Some(combo) => combo.into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// The Options window's contents: a titlebar, the left nav, the active page and the footer.
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
                    .child(self.options_nav(cx))
                    .child(
                        div()
                            .id("options-scroll")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .h_full()
                            .p_4()
                            .overflow_y_scroll()
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

    /// The left navigation: 常规 and 编辑器.
    fn options_nav(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
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
            .child(
                self.options_category_row(
                    "options-general",
                    t!("options.general").to_string(),
                    self.options_section == OptionsSection::General,
                    true,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_section = OptionsSection::General;
                    cx.notify();
                })),
            )
            .child(
                self.options_category_row(
                    "options-editor",
                    t!("options.editor").to_string(),
                    self.options_section == OptionsSection::Editor,
                    true,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_section = OptionsSection::Editor;
                    cx.notify();
                })),
            )
    }

    fn options_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        match self.options_section {
            OptionsSection::General => self.options_general_body(cx).into_any_element(),
            OptionsSection::Editor => self.options_editor_body(cx).into_any_element(),
        }
    }

    /// The 常规 page: just the language selector.
    fn options_general_body(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
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

    /// The 编辑器 page: a live preview plus the editor settings.
    fn options_editor_body(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .child(section_label(
                theme,
                t!("options.editor.preview").to_string(),
            ))
            .child(self.editor_preview())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_4()
                    .w_full()
                    .child(div().flex_1().min_w(px(0.0)).child(field_label(
                        theme,
                        t!("options.editor.font").to_string(),
                        self.editor_font_combo(),
                    )))
                    .child(div().w(px(130.0)).flex_none().child(field_label(
                        theme,
                        t!("options.editor.size").to_string(),
                        self.editor_size_combo(),
                    ))),
            )
            .child(
                toggle_row(
                    theme,
                    "options-editor-line-numbers",
                    t!("options.editor.line_numbers").to_string(),
                    self.options_editor_line_numbers,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_editor_line_numbers = !this.options_editor_line_numbers;
                    cx.notify();
                })),
            )
            .child(
                toggle_row(
                    theme,
                    "options-editor-word-wrap",
                    t!("options.editor.word_wrap").to_string(),
                    self.options_editor_word_wrap,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_editor_word_wrap = !this.options_editor_word_wrap;
                    cx.notify();
                })),
            )
    }

    /// The live preview: the sample SQL painted with the app theme's SQL colors and the staged
    /// font. (The editor's syntax colors themselves come from gpui-kit's theme.)
    fn editor_preview(&self) -> AnyElement {
        let theme = self.theme;
        let mut highlights = Vec::new();
        for span in sql::highlight(EDITOR_PREVIEW_SQL) {
            let color = match span.token {
                SqlToken::Keyword => theme.sql_keyword,
                SqlToken::String => theme.sql_string,
                SqlToken::Number => theme.sql_number,
                SqlToken::Comment => theme.sql_comment,
                SqlToken::Identifier => theme.text,
            };
            highlights.push((
                span.start..span.end,
                HighlightStyle {
                    color: Some(rgb(color).into()),
                    ..Default::default()
                },
            ));
        }
        let size = self.options_editor_font_size.clamp(8, 48) as f32;
        div()
            .id("options-editor-preview")
            .w_full()
            .h(px(140.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.editor_bg))
            .overflow_hidden()
            .p_3()
            .font_family(resolve_editor_font(&self.options_editor_font_family))
            .text_size(px(size))
            .line_height(px((size * 1.5).max(16.0)))
            .text_color(rgb(theme.text))
            .child(StyledText::new(EDITOR_PREVIEW_SQL.to_string()).with_highlights(highlights))
            .into_any_element()
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
                    this.options_editor_font_family.clear();
                    this.options_editor_font_size = DEFAULT_EDITOR_FONT_SIZE;
                    this.options_editor_line_numbers = true;
                    this.options_editor_word_wrap = true;
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
