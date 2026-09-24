use super::*;

/// Width of one titlebar icon button (theme / GitHub / settings).
const TITLEBAR_ICON_BUTTON: f32 = 34.0;
/// Width of the theme dropdown panel, used to right-align it under its button.
const THEME_MENU_WIDTH: f32 = 168.0;
/// The project's public repository, opened by the titlebar GitHub button.
const GITHUB_URL: &str = "https://github.com/yisier/rustgrid";
/// The connection types Navicat's New Connection menu lists. Only the engines present in the
/// driver registry are selectable (today that is just MySQL); the rest are shown greyed out.
const CONNECTION_TYPES: [(&str, &str); 7] = [
    ("mysql", "MySQL..."),
    ("postgresql", "PostgreSQL..."),
    ("oracle", "Oracle..."),
    ("sqlite", "SQLite..."),
    ("sqlserver", "SQL Server..."),
    ("mariadb", "MariaDB..."),
    ("mongodb", "MongoDB..."),
];

/// The visual state of one main-toolbar button: `active` fills it with the brand tint, `enabled`
/// greys it out, and `caret` appends the dropdown chevron.
#[derive(Clone, Copy)]
pub(super) struct MainButtonState {
    pub(super) active: bool,
    pub(super) enabled: bool,
    pub(super) caret: bool,
}

impl AppView {
    /// The window titlebar: the draggable app identity on the left and, on the right, the theme
    /// picker, a GitHub link and the settings button, followed by the min/max/close controls.
    ///
    /// The old File/View/Tools/Help menu bar used to sit below this; it was removed and its only
    /// working item (Tools → Options) now lives on the settings button.
    pub(super) fn render_titlebar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let anchor = self.theme_menu_anchor.clone();

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(32.0))
            .flex_none()
            .bg(rgb(theme.titlebar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .id("titlebar-drag")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .h_full()
                    .px_3()
                    .text_size(px(12.5))
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        img(ImageSource::Resource(Resource::Embedded("logo.png".into())))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex_none(),
                    )
                    .child(t!("app.title").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h_full()
                    // The theme button is this row's first child; remember its bounds so the
                    // dropdown can be anchored under its right edge.
                    .on_children_prepainted(move |bounds, _window, _cx| {
                        if let Some(bounds) = bounds.first() {
                            *anchor.borrow_mut() = Point::new(bounds.left(), bounds.bottom());
                        }
                    })
                    .child(self.titlebar_theme_button(theme, cx))
                    .child(titlebar_icon_button(
                        "titlebar-github",
                        "icons/github.svg",
                        theme,
                        |_event, _window, _cx| open_in_browser(GITHUB_URL),
                    ))
                    .child(titlebar_icon_button(
                        "titlebar-settings",
                        "icons/gear.svg",
                        theme,
                        cx.listener(|this, _event, _window, cx| this.open_options(cx)),
                    ))
                    .child(titlebar_button(
                        "titlebar-min",
                        "—",
                        theme,
                        |window, _cx| {
                            window.minimize_window();
                        },
                    ))
                    .child(titlebar_button(
                        "titlebar-max",
                        "□",
                        theme,
                        |window, _cx| {
                            toggle_maximize(window);
                        },
                    ))
                    .child(titlebar_button(
                        "titlebar-close",
                        "✕",
                        theme,
                        |window, _cx| {
                            window.remove_window();
                        },
                    )),
            )
    }

    /// The titlebar theme button; its icon mirrors the current setting (monitor / sun / moon).
    fn titlebar_theme_button(&self, theme: Theme, cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .id("titlebar-theme")
            .flex()
            .items_center()
            .justify_center()
            .w(px(TITLEBAR_ICON_BUTTON))
            .h_full()
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
            .when(self.theme_menu_open, move |style| {
                style.bg(rgb(theme.button_hover_bg))
            })
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.theme_menu_open = !this.theme_menu_open;
                this.connect_menu_open = false;
                cx.notify();
            }))
            .child(
                svg()
                    .path(theme_setting_icon(self.theme_setting))
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            )
    }

    /// The theme dropdown: 跟随系统 / 亮色模式 / 暗黑模式, with a check on the active setting.
    pub(super) fn render_theme_menu(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let anchor = *self.theme_menu_anchor.borrow();
        // Right-align the panel under its button, clamped so it never leaves the window.
        let left = (anchor.x - (px(THEME_MENU_WIDTH) - px(TITLEBAR_ICON_BUTTON))).max(px(4.0));

        let items = div()
            .flex()
            .flex_col()
            .w(px(THEME_MENU_WIDTH))
            .p_0p5()
            .child(self.theme_menu_item(
                "theme-menu-system",
                ThemeSetting::System,
                "icons/monitor.svg",
                t!("theme.follow_system").to_string(),
                cx,
            ))
            .child(self.theme_menu_item(
                "theme-menu-light",
                ThemeSetting::Light,
                "icons/sun.svg",
                t!("theme.light_mode").to_string(),
                cx,
            ))
            .child(self.theme_menu_item(
                "theme-menu-dark",
                ThemeSetting::Dark,
                "icons/moon.svg",
                t!("theme.dark_mode").to_string(),
                cx,
            ));

        ui::popup_panel(theme)
            .left(left)
            .top(anchor.y + px(4.0))
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.theme_menu_open = false;
                cx.notify();
            }))
            .child(items)
    }

    /// One row of the theme dropdown: icon, label, and a check mark that stays invisible when the
    /// row is not the active setting, so rows never shift.
    fn theme_menu_item(
        &self,
        id: &'static str,
        setting: ThemeSetting,
        icon: &'static str,
        label: String,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let selected = self.theme_setting == setting;
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .h(px(28.0))
            .px_2()
            .rounded_sm()
            .text_size(px(12.5))
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.theme_menu_open = false;
                this.set_theme(setting, cx);
                cx.notify();
            }))
            .child(
                svg()
                    .path(icon)
                    .w(px(15.0))
                    .h(px(15.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            )
            .child(div().flex_1().child(label))
            .child(
                svg()
                    .path("icons/check.svg")
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_none()
                    .text_color(rgb(theme.brand))
                    .opacity(if selected { 1.0 } else { 0.0 }),
            )
    }

    pub(super) fn render_main_toolbar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let anchor = self.connect_menu_anchor.clone();
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            // The first child is the New Connection button; remember where it sits so its engine
            // dropdown can be anchored under it.
            .on_children_prepainted(move |bounds, _window, _cx| {
                if let Some(bounds) = bounds.first() {
                    *anchor.borrow_mut() = Point::new(bounds.left(), bounds.bottom());
                }
            })
            .child(self.main_button(
                "main-connect",
                "icons/connection.svg",
                t!("main.new_connection").to_string(),
                MainButtonState {
                    active: self.connect_menu_open,
                    enabled: true,
                    caret: true,
                },
                cx.listener(|this, _event, _window, cx| {
                    this.connect_menu_open = !this.connect_menu_open;
                    this.theme_menu_open = false;
                    cx.notify();
                }),
            ))
            .child(self.main_button(
                "main-query",
                "icons/new_query.svg",
                t!("main.new_query").to_string(),
                MainButtonState {
                    active: false,
                    enabled: true,
                    caret: false,
                },
                cx.listener(|this, _event, _window, cx| this.open_new_query(cx)),
            ))
            .child(main_separator(theme));

        for (tab, icon, label_key) in MAIN_TABS {
            bar = bar.child(self.render_main_tab(tab, icon, t!(label_key).to_string(), cx));
        }

        bar
    }

    /// The New Connection engine dropdown. Only engines present in the driver registry are
    /// selectable (today that is just MySQL); the rest are listed greyed out, like Navicat's menu.
    pub(super) fn render_connect_menu(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let anchor = *self.connect_menu_anchor.borrow();

        let mut items = div().flex().flex_col().w(px(150.0)).p_0p5();
        for (id, label) in CONNECTION_TYPES {
            if self.registry.get(&DriverId::new(id)).is_some() {
                items = items.child(self.context_item(
                    id,
                    label.to_string(),
                    cx.listener(|this, _event, window, cx| {
                        this.connect_menu_open = false;
                        this.open_new_form(window, cx);
                    }),
                ));
            } else {
                items = items.child(self.context_item_disabled(id, label.to_string()));
            }
        }

        ui::popup_panel(theme)
            .left(anchor.x)
            .top(anchor.y + px(2.0))
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.connect_menu_open = false;
                cx.notify();
            }))
            .child(items)
    }

    pub(super) fn render_main_tab(
        &self,
        tab: MainTab,
        icon: &'static str,
        label: String,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let active = self.main_tab == tab;
        let enabled = matches!(
            tab,
            MainTab::Tables
                | MainTab::Views
                | MainTab::Functions
                | MainTab::Users
                | MainTab::Queries
                | MainTab::Backups
        );
        self.main_button(
            SharedString::from(format!("main-tab-{}", tab as usize)),
            icon,
            label,
            MainButtonState {
                active,
                enabled,
                caret: false,
            },
            cx.listener(move |this, _event, _window, cx| {
                if enabled {
                    this.select_main_tab(tab, cx);
                }
            }),
        )
    }

    pub(super) fn main_button(
        &self,
        id: impl Into<SharedString>,
        icon: &'static str,
        label: String,
        state: MainButtonState,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = self.theme;
        let MainButtonState {
            active,
            enabled,
            caret,
        } = state;
        let text_color = if active {
            theme.brand
        } else if enabled {
            theme.text
        } else {
            theme.text_muted
        };

        div()
            .id(id.into())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            // A minimum width keeps the short (mostly 2-character) labels on a common grid, but the
            // button grows past it for longer localized labels (e.g. "New Connection") so the text
            // never wraps onto a second line.
            .min_w(px(62.0))
            .px_2()
            .flex_none()
            .h(px(52.0))
            .rounded_sm()
            .cursor_pointer()
            .text_color(rgb(text_color))
            .when(active, move |style| style.bg(rgb(theme.brand_muted)))
            .when(!active && enabled, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(on_click)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_0p5()
                    .child(
                        svg()
                            .path(icon)
                            .w(px(22.0))
                            .h(px(22.0))
                            .flex_none()
                            .text_color(rgb(text_color)),
                    )
                    .when(caret, move |row| {
                        row.child(
                            svg()
                                .path("icons/chevron-down.svg")
                                .w(px(11.0))
                                .h(px(11.0))
                                .flex_none()
                                .text_color(rgb(text_color)),
                        )
                    }),
            )
            .child(div().text_size(px(11.0)).whitespace_nowrap().child(label))
    }

    /// The `(connection_index, database_index)` implied by the connection tree's current
    /// selection, if it points at a database row or one of its children. Falls back to the open
    /// object pane's coordinates so the tree still follows the main tabs when the selection is a
    /// connection row.
    fn tree_selection_scope(&self, cx: &App) -> Option<(usize, usize)> {
        if let Some(pair) = self
            .tree_pane
            .read(cx)
            .selected
            .as_deref()
            .and_then(parse_scope_id)
        {
            return Some(pair);
        }
        self.object_pane.as_ref().map(|pane| {
            let pane = pane.read(cx);
            (pane.connection_index, pane.database_index)
        })
    }

    /// Move the connection tree's cursor onto `category`'s node under the currently selected
    /// database, so switching main tabs also moves the tree highlight.
    fn select_tree_category(&mut self, category: Category, cx: &mut Context<'_, Self>) {
        let Some((connection_index, database_index)) = self.tree_selection_scope(cx) else {
            return;
        };
        let id = format!("cat-{connection_index}-{database_index}-{}", category.id());
        self.tree_pane.update(cx, |pane, cx| {
            pane.selected = Some(id);
            pane.selected_table = None;
            cx.notify();
        });
    }

    pub(super) fn select_main_tab(&mut self, tab: MainTab, cx: &mut Context<'_, Self>) {
        self.main_tab = tab;
        if tab == MainTab::Users {
            self.refresh_users(cx);
            self.active_grid = None;
            self.active_query = None;
            self.active_design = None;
            self.saved_query_selected = None;
            self.query_selection.clear();
            cx.notify();
            return;
        }
        if tab == MainTab::Backups {
            self.select_tree_category(Category::Backups, cx);
            self.refresh_backups(cx);
            self.active_grid = None;
            self.active_query = None;
            self.active_design = None;
            self.saved_query_selected = None;
            self.query_selection.clear();
            cx.notify();
            return;
        }
        let category = match tab {
            MainTab::Tables => Category::Tables,
            MainTab::Views => Category::Views,
            MainTab::Functions => Category::Functions,
            MainTab::Queries => Category::Queries,
            _ => return,
        };
        if let Some(pane) = self.object_pane.as_ref() {
            pane.update(cx, |pane, cx| {
                pane.category = category;
                pane.selected = None;
                pane.selected_routine = None;
                cx.notify();
            });
        }
        // The previous category's multi-selection keys do not apply to the new one.
        self.objects_selection.clear();
        self.objects_row_rects.clear();
        if category == Category::Functions
            && let Some(pane) = self.object_pane.as_ref()
        {
            let (connection_index, database_index) = {
                let pane = pane.read(cx);
                (pane.connection_index, pane.database_index)
            };
            self.ensure_routines_loaded(connection_index, database_index, cx);
        }
        self.clear_object_search(cx);
        // The object list's highlight is gone, so its routine detail should not linger either.
        self.clear_info_routine();
        self.active_grid = None;
        self.active_query = None;
        self.active_design = None;
        self.saved_query_selected = None;
        self.query_selection.clear();
        self.select_tree_category(category, cx);
        if tab == MainTab::Queries {
            self.refresh_query_files(cx);
        }
        cx.notify();
    }
}

/// Parse a connection-tree selection id (`db-` / `tbl-` / `cat-` / `qry-`) into its
/// `(connection_index, database_index)` pair.
fn parse_scope_id(id: &str) -> Option<(usize, usize)> {
    let rest = ["db-", "tbl-", "cat-", "qry-"]
        .into_iter()
        .find_map(|prefix| id.strip_prefix(prefix))?;
    let mut parts = rest.splitn(3, '-');
    let connection_index = parts.next()?.parse().ok()?;
    let database_index = parts.next()?.parse().ok()?;
    Some((connection_index, database_index))
}

/// The icon for a theme setting, shared by the titlebar button and the dropdown rows.
fn theme_setting_icon(setting: ThemeSetting) -> &'static str {
    match setting {
        ThemeSetting::System => "icons/monitor.svg",
        ThemeSetting::Light => "icons/sun.svg",
        ThemeSetting::Dark => "icons/moon.svg",
    }
}

/// One titlebar icon button (GitHub, settings). `on_click` is the full handler so a call site can
/// pass either a plain closure or a `cx.listener`.
fn titlebar_icon_button(
    id: &'static str,
    icon: &'static str,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(TITLEBAR_ICON_BUTTON))
        .h_full()
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
        .on_click(on_click)
        .child(
            svg()
                .path(icon)
                .w(px(16.0))
                .h(px(16.0))
                .flex_none()
                .text_color(rgb(theme.text_muted)),
        )
}

/// Open `url` in the OS default browser.
pub(super) fn open_in_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("rundll32")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        let _ = url;
    }
}
