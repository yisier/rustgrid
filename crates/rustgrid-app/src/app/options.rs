//! The Options window (设置): a separate OS window, opened from the titlebar's settings button.
//!
//! It hosts the 常规 page (the language selector, 启动时恢复 and 保留对象搜索), the 编辑器 page,
//! which configures the SQL editor (a live preview, font family, font size, the Ctrl + wheel zoom,
//! and the line-number / word-wrap toggles), the 快捷键 page (the query editor's 运行 / 运行已选
//! keystrokes), the 数据 page (grid display) and 关于. Every edit is staged and only applied on 确定,
//! so 取消 discards them — and 确定 pushes the 编辑器 settings into the editors that are already
//! open instead of leaving them for the next tab. Like the Export/Import/User windows, `AppView`
//! owns the state and this module renders it; `AppView::sync_dialog` is not involved.

use super::*;
use gpui_kit::component::switch::Switch;

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

/// The font sizes offered by the editor size dropdown, in px. Also the ladder the query editor's
/// Ctrl + wheel zoom steps along (see `AppView::zoom_editor_font`).
pub(super) const EDITOR_FONT_SIZES: &[u32] = &[10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24];

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

/// A clickable settings row: a label on the left, a switch on the right. The row owns the click
/// (the switch is an inert indicator), so the whole line toggles and the control only reflects the
/// state.
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
        .child(
            Switch::new(SharedString::from(format!("{id}-switch")))
                .checked(checked)
                .with_size(gpui_kit::component::Size::Small)
                .tab_stop(false),
        )
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
        self.options_editor_ctrl_wheel_zoom = self.editor_ctrl_wheel_zoom;
        self.options_grid_show_comments = self.grid_show_comments;
        self.options_grid_show_types = self.grid_show_types;
        self.options_shortcuts = self.shortcuts.clone();
        self.options_preserve_object_search = self.preserve_object_search;
        self.options_restore_session = self.restore_session;
        self.options_stop_on_error = self.stop_on_error;
        self.set_options_max_statements(self.max_statements_per_run.to_string(), cx);
        self.recording_shortcut = None;
        self.ensure_shortcut_search(cx);
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

    /// Build the 快捷键 page's search box on the first open; its text filters the command list by
    /// label, command id or bound keystroke.
    fn ensure_shortcut_search(&mut self, cx: &mut Context<'_, Self>) {
        self.options_shortcut_filter.clear();
        if let Some(search) = self.options_shortcut_search.clone() {
            search.update(cx, |input, cx| input.set_text("", cx));
            return;
        }
        let theme = self.theme;
        let weak = cx.weak_entity();
        let search = cx.new(move |cx| {
            TextInput::new(
                theme,
                "",
                TextInputOptions {
                    placeholder: t!("keymap.search").to_string().into(),
                    icon: Some("icons/search.svg"),
                    clearable: true,
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let _ = weak.update(cx, |app, cx| {
                    app.options_shortcut_filter = text.to_string();
                    cx.notify();
                });
            }))
        });
        self.options_shortcut_search = Some(search);
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
        self.editor_ctrl_wheel_zoom = self.options_editor_ctrl_wheel_zoom;
        self.grid_show_comments = self.options_grid_show_comments;
        self.grid_show_types = self.options_grid_show_types;
        self.shortcuts = self.options_shortcuts.clone();
        self.preserve_object_search = self.options_preserve_object_search;
        self.restore_session = self.options_restore_session;
        self.stop_on_error = self.options_stop_on_error;
        // The 编辑器 settings reach the editors that are already open, not only the next one: the
        // font is rendered from the wrapper's options every frame, and 显示行号 / 自动换行 are
        // pushed into each editor's state here.
        self.apply_editor_display_settings(window, cx);
        // An empty or invalid entry keeps the current limit.
        if let Ok(limit) = self.options_max_statements.trim().parse::<u32>() {
            self.max_statements_per_run = limit;
        }
        self.recording_shortcut = None;
        // Re-bind every command: the previously bound keystrokes are shadowed by a `NoAction`
        // so a changed or cleared shortcut stops firing.
        self.bound_keys = keymap::bind_keymap(cx, &self.shortcuts, &self.bound_keys);
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
                            // The content pane sits on the dialog's own background while the nav
                            // keeps the tinted face, so the two sides read as separate surfaces.
                            .bg(rgb(theme.dialog_bg))
                            // The shortcuts page scrolls its own list under a fixed search box.
                            .when(self.options_section != OptionsSection::Shortcuts, |pane| {
                                pane.overflow_y_scroll()
                            })
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
            .children(self.options_copy_menu_element(cx))
            .into_any_element()
    }

    /// A right-click handler for a setting's row: offers to copy its name, or its path for sharing
    /// ("Options › General › …").
    fn copy_label_menu(
        &self,
        label: String,
        cx: &mut Context<'_, Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
            cx.stop_propagation();
            this.options_copy_menu = Some((event.position, label.clone()));
            cx.notify();
        })
    }

    /// The setting's right-click menu, when open.
    fn options_copy_menu_element(&self, cx: &mut Context<'_, Self>) -> Option<AnyElement> {
        let (position, label) = self.options_copy_menu.clone()?;
        let path = format!(
            "{} › {} › {label}",
            t!("options.title"),
            options_section_label(self.options_section)
        );
        let copy = |text: String| {
            cx.listener(move |this, _event, _window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                this.options_copy_menu = None;
                cx.notify();
            })
        };
        let items = div()
            .flex()
            .flex_col()
            .w(px(190.0))
            .p_0p5()
            .child(self.context_item(
                "options-copy-name",
                t!("options.copy_name").to_string(),
                copy(label),
            ))
            .child(self.context_item(
                "options-copy-path",
                t!("options.copy_path").to_string(),
                copy(path),
            ));
        Some(
            ui::popup_panel(self.theme)
                .id("options-copy-menu")
                .left(position.x)
                .top(position.y)
                .on_mouse_down_out(cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                    this.options_copy_menu = None;
                    cx.notify();
                }))
                .child(items)
                .into_any_element(),
        )
    }

    /// Open the Options window on `section` and point out the setting `highlight` (an element id)
    /// for a few seconds.
    pub(super) fn open_options_at(
        &mut self,
        section: OptionsSection,
        highlight: &'static str,
        cx: &mut Context<'_, Self>,
    ) {
        self.open_options(cx);
        self.options_section = section;
        self.options_highlight = Some(highlight);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(4))
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.options_highlight == Some(highlight) {
                    app.options_highlight = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The left navigation: 常规, 编辑器 and 关于.
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
            .child(
                self.options_category_row(
                    "options-shortcuts",
                    t!("options.shortcuts").to_string(),
                    self.options_section == OptionsSection::Shortcuts,
                    true,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_section = OptionsSection::Shortcuts;
                    this.recording_shortcut = None;
                    cx.notify();
                })),
            )
            .child(
                self.options_category_row(
                    "options-data",
                    t!("options.data").to_string(),
                    self.options_section == OptionsSection::Data,
                    true,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_section = OptionsSection::Data;
                    this.recording_shortcut = None;
                    cx.notify();
                })),
            )
            .child(
                self.options_category_row(
                    "options-about",
                    t!("options.about").to_string(),
                    self.options_section == OptionsSection::About,
                    true,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_section = OptionsSection::About;
                    cx.notify();
                })),
            )
    }

    fn options_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        match self.options_section {
            OptionsSection::General => self.options_general_body(cx).into_any_element(),
            OptionsSection::Editor => self.options_editor_body(cx).into_any_element(),
            OptionsSection::Shortcuts => self.options_shortcuts_body(cx).into_any_element(),
            OptionsSection::Data => self.options_data_body(cx).into_any_element(),
            OptionsSection::About => self.options_about_body(cx).into_any_element(),
        }
    }

    /// The 快捷键 page: every keymap command, grouped and filtered by the search box. Clicking a
    /// command's field starts recording and the next keystroke replaces its shortcuts; the row's
    /// reset button restores the built-in ones, its clear button unbinds it, and 默认 resets all.
    fn options_shortcuts_body(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let filter = self.options_shortcut_filter.trim().to_lowercase();
        let matches = |command: &keymap::Command| {
            filter.is_empty()
                || t!(command.label_key).to_lowercase().contains(&filter)
                || command.id.contains(&filter)
                || keymap::keys_for(command, &self.options_shortcuts)
                    .iter()
                    .any(|key| shortcut_label(key).to_lowercase().contains(&filter))
        };

        let mut list = div()
            .id("options-shortcut-list")
            .flex()
            .flex_col()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .pr_2()
            .overflow_y_scroll()
            .child(
                div()
                    .pb_1()
                    .text_size(px(11.5))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("options.shortcuts.hint").to_string()),
            );
        let mut any = false;
        for group in keymap::CommandGroup::ALL {
            let rows: Vec<AnyElement> = keymap::COMMANDS
                .iter()
                .enumerate()
                .filter(|(_, command)| command.group == group && matches(command))
                .map(|(index, command)| self.shortcut_row(index, command, theme, cx))
                .collect();
            if rows.is_empty() {
                continue;
            }
            any = true;
            list = list
                .child(
                    div()
                        .pt_2()
                        .pb_1()
                        .text_size(px(11.5))
                        .text_color(rgb(theme.text_muted))
                        .child(t!(group.label_key()).to_string()),
                )
                .children(rows);
        }
        if !any {
            list = list.child(
                div()
                    .py_2()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("keymap.no_match").to_string()),
            );
        }

        // Only the search box stays put; the hint and the commands scroll together beneath it.
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .h_full()
            .min_h(px(0.0))
            .children(
                self.options_shortcut_search
                    .clone()
                    .map(|search| div().flex_none().w(px(260.0)).child(search)),
            )
            .child(list)
    }

    /// One 快捷键 row: the command's label (and any conflict), the field that captures its
    /// keystroke, and its reset / clear buttons.
    fn shortcut_row(
        &self,
        index: usize,
        command: &'static keymap::Command,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let id = command.id;
        let recording = self.recording_shortcut == Some(id);
        let focus = self.options_shortcut_focus[index].clone();
        let capture_focus = focus.clone();
        let keys = keymap::keys_for(command, &self.options_shortcuts);
        let overridden = self.options_shortcuts.contains_key(id);
        let conflicts = keymap::conflicts(command, &self.options_shortcuts);

        let field_text = if recording {
            t!("options.shortcuts.press").to_string()
        } else if keys.is_empty() {
            t!("keymap.unbound").to_string()
        } else {
            keys.iter()
                .map(|key| shortcut_label(key))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let mut label = div().flex().flex_col().flex_1().min_w(px(0.0)).child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(theme.text))
                .truncate()
                .child(t!(command.label_key).to_string()),
        );
        if !conflicts.is_empty() {
            let names = conflicts
                .iter()
                .map(|other| t!(other.label_key).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            label = label.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.danger))
                    .truncate()
                    .child(t!("keymap.conflict", names = names).to_string()),
            );
        }

        let icon_button = |element_id: String, glyph: &'static str, visible: bool| {
            div()
                .id(SharedString::from(element_id))
                .flex()
                .items_center()
                .justify_center()
                .w(px(22.0))
                .h(px(22.0))
                .flex_none()
                .rounded(px(4.0))
                .text_size(px(12.0))
                .text_color(rgb(theme.text_muted))
                .when(visible, |button| {
                    button
                        .cursor_pointer()
                        .hover(move |style| {
                            style
                                .bg(rgb(theme.button_hover_bg))
                                .text_color(rgb(theme.text))
                        })
                        .child(glyph)
                })
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .min_h(px(30.0))
            .child(label)
            .child(
                div()
                    .id(SharedString::from(format!("options-shortcut-{id}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(180.0))
                    .h(px(24.0))
                    .px_1()
                    .flex_none()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(if recording { theme.brand } else { theme.border }))
                    .bg(rgb(theme.input_bg))
                    .text_size(px(12.0))
                    .text_color(rgb(if recording || keys.is_empty() {
                        theme.text_muted
                    } else {
                        theme.text
                    }))
                    .truncate()
                    .cursor_pointer()
                    .hover(move |style| style.border_color(rgb(theme.brand)))
                    .track_focus(&focus)
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.recording_shortcut = Some(id);
                        window.focus(&capture_focus, cx);
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _window, cx| {
                        if this.recording_shortcut != Some(id) {
                            return;
                        }
                        // Escape cancels the capture, and must not reach the window's own
                        // "Escape closes the Options window" handler.
                        if event.keystroke.key == "escape" {
                            this.recording_shortcut = None;
                        } else if let Some(shortcut) = shortcut_from_keystroke(&event.keystroke) {
                            keymap::set_keys(command, vec![shortcut], &mut this.options_shortcuts);
                            this.recording_shortcut = None;
                        }
                        // A bare modifier (or an unusable key) keeps the capture open; either way
                        // the keystroke is ours while the row is recording.
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(field_text),
            )
            .child(
                icon_button(format!("options-shortcut-reset-{id}"), "↺", overridden).on_click(
                    cx.listener(move |this, _event, _window, cx| {
                        this.options_shortcuts.remove(id);
                        this.recording_shortcut = None;
                        cx.notify();
                    }),
                ),
            )
            .child(
                icon_button(
                    format!("options-shortcut-clear-{id}"),
                    "✕",
                    !keys.is_empty(),
                )
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    keymap::set_keys(command, Vec::new(), &mut this.options_shortcuts);
                    this.recording_shortcut = None;
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    /// The 数据 page: how the grid presents a table's data.
    fn options_data_body(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(
                div()
                    .text_size(px(12.5))
                    .child(t!("options.data").to_string()),
            )
            .child(
                toggle_row(
                    theme,
                    "options-grid-show-comments",
                    t!("options.data.show_comments").to_string(),
                    self.options_grid_show_comments,
                )
                .on_mouse_down(
                    MouseButton::Right,
                    self.copy_label_menu(t!("options.data.show_comments").to_string(), cx),
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_grid_show_comments = !this.options_grid_show_comments;
                    cx.notify();
                })),
            )
            .child(
                toggle_row(
                    theme,
                    "options-grid-show-types",
                    t!("options.data.show_types").to_string(),
                    self.options_grid_show_types,
                )
                .on_mouse_down(
                    MouseButton::Right,
                    self.copy_label_menu(t!("options.data.show_types").to_string(), cx),
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_grid_show_types = !this.options_grid_show_types;
                    cx.notify();
                })),
            )
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
            .child(
                toggle_row(
                    self.theme,
                    "options-restore-session",
                    t!("options.general.restore_session").to_string(),
                    self.options_restore_session,
                )
                .on_mouse_down(
                    MouseButton::Right,
                    self.copy_label_menu(t!("options.general.restore_session").to_string(), cx),
                )
                .tooltip(ui::text_tooltip(
                    t!("options.general.restore_session_hint").to_string(),
                ))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_restore_session = !this.options_restore_session;
                    cx.notify();
                })),
            )
            .child(
                toggle_row(
                    self.theme,
                    "options-preserve-object-search",
                    t!("options.general.preserve_object_search").to_string(),
                    self.options_preserve_object_search,
                )
                .on_mouse_down(
                    MouseButton::Right,
                    self.copy_label_menu(
                        t!("options.general.preserve_object_search").to_string(),
                        cx,
                    ),
                )
                .tooltip(ui::text_tooltip(
                    t!("options.general.preserve_object_search_hint").to_string(),
                ))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_preserve_object_search = !this.options_preserve_object_search;
                    cx.notify();
                })),
            )
            .child(
                div()
                    .id(OPTIONS_MAX_STATEMENTS)
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .p_1()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgba(0))
                    // Pointed out (from the query's statement-limit notice): an accent frame.
                    .when(
                        self.options_highlight == Some(OPTIONS_MAX_STATEMENTS),
                        |row| {
                            row.border_color(rgb(self.theme.primary))
                                .bg(rgba(self.theme.primary << 8 | 0x1f))
                        },
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        self.copy_label_menu(t!("options.general.max_statements").to_string(), cx),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child(t!("options.general.max_statements").to_string()),
                    )
                    .children(
                        self.options_max_statements_input
                            .clone()
                            .map(|input| div().w(px(80.0)).flex_none().child(input)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(11.5))
                            .text_color(rgb(self.theme.text_muted))
                            .child(t!("options.general.max_statements_hint").to_string()),
                    ),
            )
            .child(
                toggle_row(
                    self.theme,
                    "options-stop-on-error",
                    t!("options.general.stop_on_error").to_string(),
                    self.options_stop_on_error,
                )
                .on_mouse_down(
                    MouseButton::Right,
                    self.copy_label_menu(t!("options.general.stop_on_error").to_string(), cx),
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_stop_on_error = !this.options_stop_on_error;
                    cx.notify();
                })),
            )
    }

    /// Stage the statement limit and show it in its field (built on first use).
    fn set_options_max_statements(&mut self, value: String, cx: &mut Context<'_, Self>) {
        self.options_max_statements = value.clone();
        if let Some(input) = self.options_max_statements_input.clone() {
            input.update(cx, |input, cx| input.set_text(value, cx));
            return;
        }
        let theme = self.theme;
        let weak = cx.weak_entity();
        let input = cx.new(move |cx| {
            TextInput::new(theme, value, TextInputOptions::default(), cx).on_change(Rc::new(
                move |text, _window, cx| {
                    // Keep only the digits, so the field always holds a valid count.
                    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
                    let _ = weak.update(cx, |app, cx| {
                        app.options_max_statements = digits;
                        cx.notify();
                    });
                },
            ))
        });
        self.options_max_statements_input = Some(input);
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
                .on_mouse_down(
                    MouseButton::Right,
                    self.copy_label_menu(t!("options.editor.line_numbers").to_string(), cx),
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
                .on_mouse_down(
                    MouseButton::Right,
                    self.copy_label_menu(t!("options.editor.word_wrap").to_string(), cx),
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_editor_word_wrap = !this.options_editor_word_wrap;
                    cx.notify();
                })),
            )
            .child(
                toggle_row(
                    theme,
                    "options-editor-ctrl-wheel-zoom",
                    t!("options.editor.ctrl_wheel_zoom").to_string(),
                    self.options_editor_ctrl_wheel_zoom,
                )
                .on_mouse_down(
                    MouseButton::Right,
                    self.copy_label_menu(t!("options.editor.ctrl_wheel_zoom").to_string(), cx),
                )
                .tooltip(ui::text_tooltip(
                    t!("options.editor.ctrl_wheel_zoom_hint").to_string(),
                ))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.options_editor_ctrl_wheel_zoom = !this.options_editor_ctrl_wheel_zoom;
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
                    this.options_editor_ctrl_wheel_zoom = true;
                    this.options_grid_show_comments = false;
                    this.options_grid_show_types = false;
                    this.options_shortcuts.clear();
                    this.options_preserve_object_search = true;
                    this.options_restore_session = true;
                    this.options_stop_on_error = false;
                    this.set_options_max_statements(
                        rustgrid_config::DEFAULT_MAX_STATEMENTS.to_string(),
                        cx,
                    );
                    this.recording_shortcut = None;
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

/// The keystroke string for a captured key, or `None` when it cannot stand as a shortcut on its
/// own: a bare modifier, or a character the user would only ever type (with or without Shift).
/// Function and navigation keys are accepted alone; everything else needs Ctrl/Alt/⌘.
fn shortcut_from_keystroke(keystroke: &Keystroke) -> Option<String> {
    let key = keystroke.key.as_str();
    if matches!(key, "control" | "shift" | "alt" | "platform" | "function") {
        return None;
    }
    let modifiers = keystroke.modifiers;
    if !(modifiers.control || modifiers.alt || modifiers.platform)
        && !is_function_key(key)
        && !is_navigation_key(key)
    {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    if modifiers.control {
        parts.push("ctrl");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if modifiers.shift {
        parts.push("shift");
    }
    if modifiers.platform {
        parts.push("cmd");
    }
    parts.push(key);
    Some(parts.join("-"))
}

/// Whether `key` moves rather than types, so it may be a shortcut without Ctrl/Alt/⌘ (F5-style
/// navigation: Page Down, Home, Tab, ...). Enter, Backspace and Space stay typing keys.
fn is_navigation_key(key: &str) -> bool {
    matches!(
        key,
        "up" | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "tab"
            | "insert"
            | "delete"
    )
}

/// Whether `key` is one of the F1..F24 function keys.
fn is_function_key(key: &str) -> bool {
    key.strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=24).contains(&number))
}

/// The element id of the statement-limit setting, so other views can point it out.
pub(super) const OPTIONS_MAX_STATEMENTS: &str = "options-max-statements";

/// A section's name in the Options navigation.
fn options_section_label(section: OptionsSection) -> String {
    match section {
        OptionsSection::General => t!("options.general"),
        OptionsSection::Editor => t!("options.editor"),
        OptionsSection::Shortcuts => t!("options.shortcuts"),
        OptionsSection::Data => t!("options.data"),
        OptionsSection::About => t!("options.about"),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(keystroke: &str) -> Option<String> {
        shortcut_from_keystroke(&Keystroke::parse(keystroke).expect("keystroke"))
    }

    #[test]
    fn recording_accepts_navigation_and_function_keys_alone() {
        assert_eq!(record("pagedown").as_deref(), Some("pagedown"));
        assert_eq!(record("shift-home").as_deref(), Some("shift-home"));
        assert_eq!(record("f5").as_deref(), Some("f5"));
        assert_eq!(record("ctrl-q").as_deref(), Some("ctrl-q"));
    }

    #[test]
    fn recording_rejects_plain_typing() {
        assert_eq!(record("a"), None);
        assert_eq!(record("shift-a"), None);
        assert_eq!(record("enter"), None);
        assert_eq!(record("space"), None);
    }
}
