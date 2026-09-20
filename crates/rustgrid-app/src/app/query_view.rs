use super::*;

impl AppView {
    pub(super) fn render_query_view(
        &self,
        query: &QueryTab,
        _window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .px_1()
            .py_0p5()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.query_tool_button(
                "query-save",
                "icons/save.svg",
                t!("query.save").to_string(),
                theme.text,
                theme.text,
                true,
                cx.listener(|this, _event, window, cx| this.begin_save_query(window, cx)),
            ))
            .child(toolbar_separator(theme))
            .child(self.query_tool_button(
                "query-format",
                "icons/format_sql.svg",
                t!("query.format").to_string(),
                theme.text,
                theme.text,
                true,
                cx.listener(|this, _event, _window, cx| this.format_query(cx)),
            ));

        let has_connection = query
            .connection_index
            .and_then(|index| self.connection_arc(index))
            .is_some();
        let run_enabled = has_connection && !query.running;
        // One run button: it runs the selection when there is one, otherwise the whole editor.
        let has_selection = query.caret != query.anchor;
        let run_label = if has_selection {
            t!("query.run_selected").to_string()
        } else {
            t!("query.run").to_string()
        };

        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py_0p5()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.query_connection_combo_element())
            .child(self.query_database_combo_element())
            .child(div().w(px(10.0)).flex_none())
            .child(self.query_tool_button(
                "query-run",
                "icons/run.svg",
                run_label,
                theme.text,
                theme.icon_connection,
                run_enabled,
                cx.listener(move |this, _event, _window, cx| this.run_query(has_selection, cx)),
            ))
            .child(self.query_tool_button(
                "query-stop",
                "icons/stop.svg",
                t!("query.stop").to_string(),
                theme.text_muted,
                theme.danger,
                query.running,
                cx.listener(|this, _event, _window, cx| this.stop_query(cx)),
            ));

        let editor = self.render_query_editor(query, cx).into_any_element();
        let result_grid = query.grid_id.and_then(|id| {
            self.grids
                .iter()
                .find(|grid| grid.read(cx).state.id == id)
                .cloned()
        });
        let has_result_panel = result_grid.is_some() || !matches!(query.result, Loadable::Idle);
        let body: AnyElement = if !has_result_panel {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(editor)
                .into_any_element()
        } else {
            let result_body: AnyElement = match result_grid {
                Some(grid) => grid.into_any_element(),
                None => self.render_query_result(query, cx),
            };
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(editor),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .border_t_1()
                        .border_color(rgb(theme.border))
                        .bg(rgb(theme.editor_bg))
                        .child(result_body),
                )
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(toolbar)
            .child(controls)
            .child(body)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn query_tool_button(
        &self,
        id: &'static str,
        icon: &'static str,
        label: String,
        color: u32,
        icon_color: u32,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme;
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(24.0))
            .rounded_sm()
            .text_size(px(12.0))
            .text_color(rgb(color))
            .when(enabled, move |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(on_click)
            .child(
                svg()
                    .path(icon)
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .text_color(rgb(icon_color)),
            )
            .child(label)
    }

    fn query_connection_combo_element(&self) -> AnyElement {
        match self.query_connection_combo.as_ref() {
            Some(combo) => combo.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn query_database_combo_element(&self) -> AnyElement {
        match self.query_database_combo.as_ref() {
            Some(combo) => combo.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// The saved-query file list shown under the Queries main tab. Like the Backup tab, it is
    /// scoped to the selected (opened) database and behaves like a folder of `.sql` files:
    /// double-click opens, right-click (or F2 / Ctrl+C / Ctrl+V) manages the file.
    pub(super) fn render_saved_queries(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut list = div()
            .id("saved-query-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_focus(&self.query_list_focus)
            .key_context(QUERY_LIST_CONTEXT)
            .on_action(cx.listener(|this, _: &RenameQueryFile, window, cx| {
                if let Some(index) = this.saved_query_selected {
                    this.begin_rename_query(index, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopyQueryFile, _window, cx| {
                if let Some(index) = this.saved_query_selected {
                    this.copy_query_file(index);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &PasteQueryFile, _window, cx| {
                this.paste_query_file(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    if this.query_rename.is_none() {
                        window.focus(&this.query_list_focus, cx);
                    }
                }),
            )
            .py_1();

        if self.query_scope(cx).is_none() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .p_3()
                .text_color(rgb(theme.text_muted))
                .child(t!("query.open_database").to_string())
                .into_any_element();
        }

        let mut has_any = false;
        let query = self.object_search.trim().to_lowercase();
        let visible: Vec<usize> = self
            .visible_query_files(cx)
            .into_iter()
            .filter(|index| {
                query.is_empty()
                    || self.query_files[*index]
                        .name
                        .to_lowercase()
                        .contains(&query)
            })
            .collect();
        for index in visible {
            has_any = true;
            let file = &self.query_files[index];
            let selected = self.saved_query_selected == Some(index);
            let rename = self
                .query_rename
                .as_ref()
                .filter(|edit| edit.index == index)
                .map(|edit| edit.input.clone());
            list = list.child(query_file_row(
                SharedString::from(format!("query-file-{index}")),
                theme,
                file.name.clone(),
                selected,
                rename,
                cx.listener(move |this, event, _window, cx| {
                    this.saved_query_selected = Some(index);
                    if matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2) {
                        this.open_saved_query(index, cx);
                    }
                    cx.notify();
                }),
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.saved_query_selected = Some(index);
                    this.context_menu = Some(ContextMenu {
                        target: ContextTarget::QueryFile { index },
                        position: event.position,
                    });
                    cx.notify();
                }),
            ));
        }

        if !has_any {
            list = list.child(
                div()
                    .p_3()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("common.empty").to_string()),
            );
        }
        list.into_any_element()
    }
}

/// One row of the saved-query file list: an icon and the file name. When `rename` is set the row
/// draws the in-place editor instead of its title.
fn query_file_row(
    id: SharedString,
    theme: Theme,
    title: String,
    selected: bool,
    rename: Option<Entity<TextInput>>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_right_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let title_element: AnyElement = match rename {
        Some(input) => div()
            .flex_1()
            .min_w(px(0.0))
            .h(px(20.0))
            .child(input)
            .into_any_element(),
        None => div()
            .text_size(px(12.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .child(title)
            .into_any_element(),
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .w_full()
        .h(px(24.0))
        .px_2()
        .cursor_pointer()
        .when(selected, move |style| {
            style
                .bg(rgb(theme.tree_selected_bg))
                .text_color(rgb(theme.tree_selected_text))
        })
        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
        .on_click(on_click)
        .on_mouse_down(MouseButton::Right, on_right_click)
        .child(tree_icon("icons/queries.svg", theme.icon_queries))
        .child(div().flex_1().min_w(px(0.0)).child(title_element))
}
