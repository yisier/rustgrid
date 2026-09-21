use super::*;

impl AppView {
    pub(super) fn render_menu_bar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let anchor = self.menu_popup_anchor.clone();
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(26.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border));

        for (id, key) in [
            ("menu-file", "menu.file"),
            ("menu-view", "menu.view"),
            ("menu-tools", "menu.tools"),
            ("menu-help", "menu.help"),
        ] {
            let mut item = div()
                .id(id)
                .px_3()
                .py_0p5()
                .rounded_sm()
                .text_size(px(12.0))
                .cursor_pointer()
                .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                .child(t!(key).to_string());
            if id == "menu-tools" {
                item = item
                    .when(self.tools_menu_open, move |style| {
                        style.bg(rgb(theme.tree_selected_bg))
                    })
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.tools_menu_open = !this.tools_menu_open;
                        cx.notify();
                    }));
            }
            bar = bar.child(item);
        }

        // Remember where the Tools item sits so its dropdown can be anchored under it. The menu
        // bar's direct children are the four items in order (File, View, Tools, Help).
        bar.on_children_prepainted(move |bounds, _window, _cx| {
            if let Some(bounds) = bounds.get(2) {
                *anchor.borrow_mut() = Point::new(bounds.left(), bounds.bottom());
            }
        })
    }

    pub(super) fn render_tools_menu(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let anchor = *self.menu_popup_anchor.borrow();
        let items = div()
            .flex()
            .flex_col()
            .w(px(180.0))
            .py_1()
            .child(self.context_item(
                "tools-options",
                t!("menu.options").to_string(),
                cx.listener(|this, _event, _window, cx| {
                    this.tools_menu_open = false;
                    this.open_options(cx);
                }),
            ));

        ui::popup_panel(theme)
            .left(anchor.x)
            .top(anchor.y)
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.tools_menu_open = false;
                cx.notify();
            }))
            .child(items)
    }

    pub(super) fn render_main_toolbar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
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
            .child(self.main_button(
                "main-connect",
                "icons/connection.svg",
                t!("main.connection").to_string(),
                false,
                true,
                cx.listener(|this, _event, window, cx| this.open_new_form(window, cx)),
            ))
            .child(self.main_button(
                "main-query",
                "icons/queries.svg",
                t!("main.new_query").to_string(),
                false,
                true,
                cx.listener(|this, _event, _window, cx| this.open_new_query(cx)),
            ))
            .child(main_separator(theme));

        for (tab, icon, label_key) in MAIN_TABS {
            bar = bar.child(self.render_main_tab(tab, icon, t!(label_key).to_string(), cx));
        }

        bar
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
            MainTab::Tables | MainTab::Views | MainTab::Users | MainTab::Queries | MainTab::Backups
        );
        self.main_button(
            SharedString::from(format!("main-tab-{}", tab as usize)),
            icon,
            label,
            active,
            enabled,
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
        active: bool,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = self.theme;
        let text_color = if active {
            theme.tree_selected_text
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
            .w(px(62.0))
            .h(px(52.0))
            .rounded_sm()
            .cursor_pointer()
            .text_color(rgb(text_color))
            .when(active, move |style| style.bg(rgb(theme.tree_selected_bg)))
            .when(!active && enabled, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(on_click)
            .child(
                svg()
                    .path(icon)
                    .w(px(22.0))
                    .h(px(22.0))
                    .flex_none()
                    .text_color(rgb(text_color)),
            )
            .child(div().text_size(px(11.0)).child(label))
    }

    pub(super) fn select_main_tab(&mut self, tab: MainTab, cx: &mut Context<'_, Self>) {
        self.main_tab = tab;
        if tab == MainTab::Users {
            self.refresh_users(cx);
            self.privilege_manager = None;
            self.active_grid = None;
            self.active_query = None;
            self.active_design = None;
            self.saved_query_selected = None;
            // Navicat shows the account details pane by default on the Users tab.
            self.info_open = true;
            cx.notify();
            return;
        }
        if tab == MainTab::Backups {
            self.refresh_backups(cx);
            self.active_grid = None;
            self.active_query = None;
            self.active_design = None;
            self.saved_query_selected = None;
            cx.notify();
            return;
        }
        let category = match tab {
            MainTab::Tables => Category::Tables,
            MainTab::Views => Category::Views,
            MainTab::Queries => Category::Queries,
            _ => return,
        };
        if let Some(pane) = self.object_pane.as_ref() {
            pane.update(cx, |pane, cx| {
                pane.category = category;
                pane.selected = None;
                cx.notify();
            });
        }
        self.clear_object_search(cx);
        self.active_grid = None;
        self.active_query = None;
        self.active_design = None;
        self.saved_query_selected = None;
        if tab == MainTab::Queries {
            self.refresh_query_files(cx);
        }
        cx.notify();
    }
}
