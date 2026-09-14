use super::*;

impl AppView {
    pub(super) fn render_menu_bar(&self) -> impl IntoElement {
        let theme = self.theme;
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
            ("menu-tools", "menu.tools"),
            ("menu-view", "menu.view"),
            ("menu-help", "menu.help"),
        ] {
            bar = bar.child(
                div()
                    .id(id)
                    .px_3()
                    .py_0p5()
                    .rounded_sm()
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .child(t!(key).to_string()),
            );
        }

        bar
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
        let enabled = matches!(tab, MainTab::Tables | MainTab::Views | MainTab::Queries);
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
        self.object_search.clear();
        self.notify_object_pane(cx);
        self.active_grid = None;
        self.active_query = None;
        cx.notify();
    }
}
