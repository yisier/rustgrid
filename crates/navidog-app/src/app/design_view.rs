use super::design::*;
use super::*;

#[derive(Clone, Copy)]
enum DetailToggle {
    AutoIncrement,
    Unsigned,
    Zerofill,
}

impl TableDesignView {
    fn toggle_detail(&mut self, toggle: DetailToggle, cx: &mut Context<'_, Self>) {
        match toggle {
            DetailToggle::AutoIncrement => self.toggle_auto_increment(cx),
            DetailToggle::Unsigned => self.toggle_unsigned(cx),
            DetailToggle::Zerofill => self.toggle_zerofill(cx),
        }
    }
}

impl Render for TableDesignView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        if let Some(app) = self.app.upgrade() {
            self.theme = app.read(cx).theme;
        }
        self.render_design(cx)
    }
}

impl TableDesignView {
    fn render_design(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(rgb(theme.editor_bg))
            .text_size(px(12.0))
            .text_color(rgb(theme.text))
            .child(self.render_toolbar(cx))
            .child(self.render_subtabs(cx));

        root = root.child(self.render_body(cx));

        let root_anchor = self.root_anchor.clone();
        root = root.on_children_prepainted(move |bounds, _window, _cx| {
            if let Some(first) = bounds.first() {
                *root_anchor.borrow_mut() = Point::new(first.left(), first.top());
            }
        });

        if self.type_combo.is_some() {
            root = root.child(self.render_type_combo(cx));
        }

        root.on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
            this.hscroll_drag(event, cx);
            this.vscroll_drag(event, cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                let h = this.hscroll_grab.take().is_some();
                let v = this.vscroll_grab.take().is_some();
                if h || v {
                    cx.notify();
                }
            }),
        )
        .into_any_element()
    }

    fn render_toolbar(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let fields = self.tab == DesignTab::Fields && !self.loading;
        let selected = self.selected_field.is_some();
        let can_up = self.selected_field.map(|index| index > 0).unwrap_or(false);
        let can_down = self
            .selected_field
            .map(|index| index + 1 < self.schema.columns.len())
            .unwrap_or(false);
        let save_enabled = !self.saving && !self.loading;

        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(28.0))
            .flex_none()
            .px_1()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(ui::toolbar_item(
                "design-save",
                "icons/save.svg",
                t!("design.save").to_string(),
                save_enabled,
                theme,
                cx.listener(|this, _event, _window, cx| this.save(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(ui::toolbar_item(
                "design-add-field",
                "icons/add_field.svg",
                t!("design.add_field").to_string(),
                fields,
                theme,
                cx.listener(|this, _event, _window, cx| this.add_field(cx)),
            ))
            .child(ui::toolbar_item(
                "design-insert-field",
                "icons/insert_field.svg",
                t!("design.insert_field").to_string(),
                fields && selected,
                theme,
                cx.listener(|this, _event, _window, cx| this.insert_field(cx)),
            ))
            .child(ui::toolbar_item(
                "design-delete-field",
                "icons/delete_field.svg",
                t!("design.delete_field").to_string(),
                fields && selected,
                theme,
                cx.listener(|this, _event, _window, cx| this.delete_field(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(ui::toolbar_item(
                "design-primary-key",
                "icons/primary_key.svg",
                t!("design.primary_key").to_string(),
                fields && selected,
                theme,
                cx.listener(|this, _event, _window, cx| this.toggle_primary_key(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(ui::toolbar_item(
                "design-move-up",
                "icons/arrow-up.svg",
                t!("design.move_up").to_string(),
                fields && can_up,
                theme,
                cx.listener(|this, _event, _window, cx| this.move_field(-1, cx)),
            ))
            .child(ui::toolbar_item(
                "design-move-down",
                "icons/arrow-down.svg",
                t!("design.move_down").to_string(),
                fields && can_down,
                theme,
                cx.listener(|this, _event, _window, cx| this.move_field(1, cx)),
            ))
    }

    fn render_subtabs(&self, cx: &mut Context<'_, Self>) -> Div {
        let theme = self.theme;
        let mut bar = div()
            .flex()
            .flex_row()
            .items_end()
            .gap_1()
            .h(px(26.0))
            .flex_none()
            .px_2()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border));
        for tab in DesignTab::ALL {
            let active = self.tab == tab;
            bar = bar.child(
                div()
                    .id(tab.id())
                    .flex()
                    .items_center()
                    .justify_center()
                    .px_4()
                    .h(px(24.0))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(active, move |style| {
                        style
                            .bg(rgb(theme.editor_bg))
                            .border_t_1()
                            .border_l_1()
                            .border_r_1()
                            .border_color(rgb(theme.border))
                            .text_color(rgb(theme.text))
                            .font_weight(FontWeight::SEMIBOLD)
                            .mb(px(-1.0))
                    })
                    .when(!active, move |style| {
                        style
                            .bg(rgb(theme.button_bg))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .text_color(rgb(theme.text_muted))
                            .hover(move |style| style.text_color(rgb(theme.text)))
                    })
                    .on_click(
                        cx.listener(move |this, _event, _window, cx| this.select_tab(tab, cx)),
                    )
                    .child(t!(tab.label_key()).to_string()),
            );
        }
        bar
    }

    fn render_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let content: AnyElement = if let Some(error) = self.error.as_ref() {
            div()
                .p_3()
                .text_color(rgb(theme.danger))
                .child(error.clone())
                .into_any_element()
        } else if self.loading && self.original.is_none() {
            div()
                .p_3()
                .text_color(rgb(theme.text_muted))
                .child(t!("design.loading").to_string())
                .into_any_element()
        } else {
            match self.tab {
                DesignTab::Fields => self.render_fields(cx),
                DesignTab::Indexes => self.render_indexes(),
                DesignTab::ForeignKeys => self.render_foreign_keys(),
                DesignTab::Triggers => self.render_triggers(),
                DesignTab::Options => self.render_options(),
                DesignTab::Comment => self.render_comment(),
                DesignTab::Sql => self.render_sql(),
            }
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_hidden()
            .child(content)
            .into_any_element()
    }

    fn render_fields(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let widths = [
            FIELD_NAME_WIDTH,
            FIELD_TYPE_WIDTH,
            FIELD_LENGTH_WIDTH,
            FIELD_DECIMALS_WIDTH,
            FIELD_NOTNULL_WIDTH,
            FIELD_VIRTUAL_WIDTH,
            FIELD_KEY_WIDTH,
            FIELD_COMMENT_WIDTH,
        ];
        let content_width: f32 = DESIGN_GUTTER_WIDTH + widths.iter().sum::<f32>();

        let columns = self.schema.columns.clone();
        let selected = self.selected_field;
        let edit = self
            .edit
            .as_ref()
            .map(|editor| (editor.row, editor.column, editor.input.clone()));
        let weak = self.self_weak.clone();
        let combo_anchor = self.combo_anchor.clone();

        let list = uniform_list(
            SharedString::from(format!("design-fields-{}", self.id)),
            columns.len(),
            move |range, _window, _cx| {
                range
                    .map(|row| {
                        let column = &columns[row];
                        let row_selected = selected == Some(row);
                        let row_background = if row_selected {
                            theme.tree_selected_bg
                        } else if row % 2 == 1 {
                            theme.row_alt_bg
                        } else {
                            theme.editor_bg
                        };

                        let editor_for = |target: FieldColumn| {
                            edit.as_ref()
                                .filter(|(row_index, column_kind, _)| {
                                    *row_index == row && *column_kind == target
                                })
                                .map(|(_, _, input)| input.clone())
                        };

                        let mut row_element = div()
                            .flex()
                            .flex_row()
                            .h(px(DESIGN_ROW_HEIGHT))
                            .bg(rgb(row_background))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .h(px(DESIGN_ROW_HEIGHT))
                                    .w(px(DESIGN_GUTTER_WIDTH))
                                    .flex_none()
                                    .border_r_1()
                                    .border_color(rgb(theme.grid_line))
                                    .when(row_selected, |gutter| {
                                        gutter.child(
                                            svg()
                                                .path("icons/row_marker.svg")
                                                .w(px(7.0))
                                                .h(px(7.0))
                                                .flex_none()
                                                .text_color(rgb(theme.tree_selected_text)),
                                        )
                                    }),
                            )
                            .child(design_edit_cell(
                                SharedString::from(format!("design-name-{row}")),
                                FIELD_NAME_WIDTH,
                                column.name.clone(),
                                theme,
                                row_selected,
                                &weak,
                                row,
                                FieldColumn::Name,
                                editor_for(FieldColumn::Name),
                            ))
                            .child(design_type_cell(
                                SharedString::from(format!("design-type-{row}")),
                                FIELD_TYPE_WIDTH,
                                column.data_type.clone(),
                                theme,
                                row_selected,
                                &weak,
                                row,
                                combo_anchor.clone(),
                            ))
                            .child(design_edit_cell(
                                SharedString::from(format!("design-length-{row}")),
                                FIELD_LENGTH_WIDTH,
                                column.length.clone(),
                                theme,
                                row_selected,
                                &weak,
                                row,
                                FieldColumn::Length,
                                editor_for(FieldColumn::Length),
                            ))
                            .child(design_edit_cell(
                                SharedString::from(format!("design-decimals-{row}")),
                                FIELD_DECIMALS_WIDTH,
                                column.decimals.clone(),
                                theme,
                                row_selected,
                                &weak,
                                row,
                                FieldColumn::Decimals,
                                editor_for(FieldColumn::Decimals),
                            ))
                            .child(design_check_cell(
                                SharedString::from(format!("design-notnull-{row}")),
                                FIELD_NOTNULL_WIDTH,
                                !column.nullable,
                                theme,
                                Some((weak.clone(), row)),
                            ))
                            .child(design_check_cell(
                                SharedString::from(format!("design-virtual-{row}")),
                                FIELD_VIRTUAL_WIDTH,
                                !column.generated.trim().is_empty(),
                                theme,
                                None,
                            ))
                            .child(design_key_cell(
                                SharedString::from(format!("design-key-{row}")),
                                FIELD_KEY_WIDTH,
                                column,
                                theme,
                            ))
                            .child(design_edit_cell(
                                SharedString::from(format!("design-comment-{row}")),
                                FIELD_COMMENT_WIDTH,
                                column.comment.clone(),
                                theme,
                                row_selected,
                                &weak,
                                row,
                                FieldColumn::Comment,
                                editor_for(FieldColumn::Comment),
                            ));

                        if row_selected {
                            row_element = row_element
                                .child(div().flex_1())
                                .text_color(rgb(theme.tree_selected_text));
                        }
                        row_element
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(self.list_scroll.clone())
        .flex_1()
        .min_h(px(0.0));

        let header = self.render_field_header(theme, &widths);

        let table = div()
            .id("design-hscroll")
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .track_scroll(&self.hscroll)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .h_full()
                    .w(px(content_width))
                    .child(header)
                    .child(list),
            );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .child(table)
                            .child(self.render_hscrollbar(cx)),
                    )
                    .child(self.render_vscrollbar(cx)),
            )
            .child(self.render_detail())
            .into_any_element()
    }

    fn render_field_header(&self, theme: Theme, widths: &[f32; 8]) -> Div {
        let labels = [
            "design.col.name",
            "design.col.type",
            "design.col.length",
            "design.col.decimals",
            "design.col.not_null",
            "design.col.virtual",
            "design.col.key",
            "design.col.comment",
        ];
        let mut header = div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(DESIGN_HEADER_HEIGHT))
            .w_full()
            .bg(rgb(theme.header_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .w(px(DESIGN_GUTTER_WIDTH))
                    .h(px(DESIGN_HEADER_HEIGHT))
                    .flex_none()
                    .border_r_1()
                    .border_color(rgb(theme.grid_line)),
            );
        for (index, key) in labels.iter().enumerate() {
            header = header.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(DESIGN_HEADER_HEIGHT))
                    .w(px(widths[index]))
                    .flex_none()
                    .px_2()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(rgb(theme.grid_line))
                    .child(t!(*key).to_string()),
            );
        }
        header
    }

    fn render_detail(&self) -> Div {
        let theme = self.theme;
        let enabled = self.selected_field.is_some();
        let column = self
            .selected_field
            .and_then(|index| self.schema.columns.get(index));
        let auto_increment = column.map(|column| column.auto_increment).unwrap_or(false);
        let unsigned = column.map(|column| column.unsigned).unwrap_or(false);
        let zerofill = column.map(|column| column.zerofill).unwrap_or(false);

        let mut panel = div()
            .flex()
            .flex_col()
            .flex_none()
            .h(px(DESIGN_DETAIL_HEIGHT))
            .w_full()
            .px_2()
            .py_2()
            .gap_2()
            .bg(rgb(theme.dialog_face))
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(230.0))
                            .flex_none()
                            .text_size(px(12.0))
                            .child(t!("design.default").to_string()),
                    )
                    .child(
                        div()
                            .w(px(360.0))
                            .h(px(22.0))
                            .when(!enabled, |style| style.opacity(0.5))
                            .child(self.default_input.clone()),
                    ),
            );

        for (id, key, checked, toggle) in [
            (
                "design-detail-auto-increment",
                "design.auto_increment",
                auto_increment,
                DetailToggle::AutoIncrement,
            ),
            (
                "design-detail-unsigned",
                "design.unsigned",
                unsigned,
                DetailToggle::Unsigned,
            ),
            (
                "design-detail-zerofill",
                "design.fill_zero",
                zerofill,
                DetailToggle::Zerofill,
            ),
        ] {
            let weak = self.self_weak.clone();
            panel = panel.child(
                div()
                    .id(id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .when(enabled, |style| style.cursor_pointer())
                    .child(checkbox_box(checked, theme))
                    .child(div().text_size(px(12.0)).child(t!(key).to_string()))
                    .when(enabled, |style| {
                        style.on_click(move |_event, _window, cx| {
                            let _ = weak.update(cx, |view, cx| view.toggle_detail(toggle, cx));
                        })
                    }),
            );
        }

        panel
    }

    fn render_indexes(&self) -> AnyElement {
        let theme = self.theme;
        let rows = self
            .schema
            .indexes
            .iter()
            .map(|index| {
                vec![
                    index.name.clone(),
                    index.columns.join(", "),
                    index.index_type.clone(),
                    if index.primary {
                        "PRIMARY".to_string()
                    } else if index.unique {
                        "YES".to_string()
                    } else {
                        String::new()
                    },
                    index.comment.clone(),
                ]
            })
            .collect::<Vec<_>>();
        simple_table(
            theme,
            "index",
            &[
                ("design.index.name", 180.0),
                ("design.index.fields", 220.0),
                ("design.index.type", 100.0),
                ("design.index.unique", 90.0),
                ("design.index.comment", 220.0),
            ],
            rows,
        )
    }

    fn render_foreign_keys(&self) -> AnyElement {
        let theme = self.theme;
        let rows = self
            .schema
            .foreign_keys
            .iter()
            .map(|foreign_key| {
                vec![
                    foreign_key.name.clone(),
                    foreign_key.columns.join(", "),
                    foreign_key.referenced_table.clone(),
                    foreign_key.referenced_columns.join(", "),
                    foreign_key.on_delete.clone(),
                    foreign_key.on_update.clone(),
                ]
            })
            .collect::<Vec<_>>();
        simple_table(
            theme,
            "fk",
            &[
                ("design.fk.name", 180.0),
                ("design.fk.columns", 170.0),
                ("design.fk.ref_table", 160.0),
                ("design.fk.ref_columns", 170.0),
                ("design.fk.delete", 100.0),
                ("design.fk.update", 100.0),
            ],
            rows,
        )
    }

    fn render_triggers(&self) -> AnyElement {
        let theme = self.theme;
        let rows = self
            .schema
            .triggers
            .iter()
            .map(|trigger| {
                vec![
                    trigger.name.clone(),
                    trigger.timing.clone(),
                    trigger.event.clone(),
                    trigger.statement.replace('\n', " "),
                ]
            })
            .collect::<Vec<_>>();
        simple_table(
            theme,
            "trigger",
            &[
                ("design.trigger.name", 200.0),
                ("design.trigger.timing", 100.0),
                ("design.trigger.event", 100.0),
                ("design.trigger.statement", 380.0),
            ],
            rows,
        )
    }

    fn render_options(&self) -> AnyElement {
        let theme = self.theme;
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(option_row(
                t!("design.options.engine"),
                self.engine_input.clone(),
                theme,
            ))
            .child(option_row(
                t!("design.options.charset"),
                self.charset_input.clone(),
                theme,
            ))
            .child(option_row(
                t!("design.options.collation"),
                self.collation_input.clone(),
                theme,
            ))
            .child(option_row(
                t!("design.options.auto_increment"),
                self.auto_increment_input.clone(),
                theme,
            ))
            .child(option_row(
                t!("design.comment_label"),
                self.comment_input.clone(),
                theme,
            ));
        panel = panel.child(div().flex_1());
        panel.into_any_element()
    }

    fn render_comment(&self) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(option_row(
                t!("design.comment_label"),
                self.comment_input.clone(),
                theme,
            ))
            .child(div().flex_1())
            .into_any_element()
    }

    fn render_sql(&self) -> AnyElement {
        let theme = self.theme;
        let sql = self.preview_sql();
        let unchanged = sql.trim().is_empty();
        let text = if unchanged {
            t!("design.no_changes").to_string()
        } else {
            sql
        };
        let color = if unchanged {
            theme.text_muted
        } else {
            theme.text
        };
        div()
            .id("design-sql")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .bg(rgb(theme.editor_bg))
            .p_3()
            .font_family("Consolas")
            .text_size(px(12.0))
            .text_color(rgb(color))
            .child(div().whitespace_nowrap().child(text))
            .into_any_element()
    }

    fn render_type_combo(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(row) = self.type_combo else {
            return div().into_any_element();
        };
        let origin = *self.root_anchor.borrow();
        let anchor = self
            .combo_anchor
            .borrow()
            .get(&row)
            .copied()
            .unwrap_or_default();
        let anchor = Point::new(anchor.x - origin.x, anchor.y - origin.y);
        let current = self
            .schema
            .columns
            .get(row)
            .map(|column| column.data_type.clone())
            .unwrap_or_default();
        let weak = self.self_weak.clone();

        let mut options = div()
            .id("design-type-options")
            .max_h(px(300.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for data_type in self.filtered_types() {
            let selected = data_type == current;
            let weak = weak.clone();
            options = options.child(
                div()
                    .id(SharedString::from(format!(
                        "design-type-option-{data_type}"
                    )))
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
                    .on_click(move |_event, _window, cx| {
                        let _ = weak.update(cx, |view, cx| view.select_type(row, data_type, cx));
                    })
                    .child(data_type.to_string()),
            );
        }

        let mut popup = div()
            .id("design-type-list")
            .absolute()
            .left(anchor.x)
            .top(anchor.y)
            .w(px(FIELD_TYPE_WIDTH))
            .flex()
            .flex_col()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .shadow(dialog_shadow())
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.type_combo = None;
                cx.notify();
            }));
        if let Some(search) = self.type_search.clone() {
            popup = popup.child(
                div()
                    .flex_none()
                    .p_1()
                    .border_b_1()
                    .border_color(rgb(theme.grid_line))
                    .child(div().h(px(20.0)).child(search)),
            );
        }
        popup = popup.child(options);
        deferred(popup).with_priority(100).into_any_element()
    }

    fn render_hscrollbar(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let viewport = f32::from(self.hscroll.bounds().size.width);
        let max = f32::from(self.hscroll.max_offset().width);
        if max <= 0.0 {
            return div().flex_none().h(px(14.0)).into_any_element();
        }
        let scroll = -f32::from(self.hscroll.offset().x);
        let (thumb_left, thumb_len) = scrollbar_fractions(viewport, max, scroll);
        ui::hscrollbar_track("design-hscrollbar", theme, thumb_left, thumb_len)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.hscroll_begin(event.position.x, cx);
                }),
            )
            .into_any_element()
    }

    fn render_vscrollbar(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let viewport = f32::from(handle.bounds().size.height);
        let max = f32::from(handle.max_offset().height);
        if max <= 0.0 {
            return div().into_any_element();
        }
        let scroll = -f32::from(handle.offset().y);
        let (thumb_top, thumb_len) = scrollbar_fractions(viewport, max, scroll);
        div()
            .flex_none()
            .h_full()
            .pt(px(DESIGN_HEADER_HEIGHT))
            .pb(px(14.0))
            .child(
                ui::vscrollbar_track("design-vscrollbar", theme, thumb_top, thumb_len)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.vscroll_begin(event.position.y, cx);
                        }),
                    ),
            )
            .into_any_element()
    }

    fn hscroll_begin(&mut self, mouse_x: Pixels, cx: &mut Context<'_, Self>) {
        let bounds = self.hscroll.bounds();
        let viewport = f32::from(bounds.size.width);
        let max = f32::from(self.hscroll.max_offset().width);
        let (thumb_w, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let scroll = -f32::from(self.hscroll.offset().x);
        let thumb_x = (scroll / max) * travel;
        let relative = f32::from(mouse_x) - f32::from(bounds.left());
        let grab = if relative >= thumb_x && relative <= thumb_x + thumb_w {
            relative - thumb_x
        } else {
            thumb_w / 2.0
        };
        self.hscroll_grab = Some(grab);
        self.hscroll_set(relative, grab, cx);
    }

    fn hscroll_set(&self, relative: f32, grab: f32, cx: &mut Context<'_, Self>) {
        let viewport = f32::from(self.hscroll.bounds().size.width);
        let max = f32::from(self.hscroll.max_offset().width);
        let (_, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let thumb_x = (relative - grab).clamp(0.0, travel);
        let scroll = thumb_x / travel * max;
        let y = self.hscroll.offset().y;
        self.hscroll.set_offset(Point::new(px(-scroll), y));
        cx.notify();
    }

    fn vscroll_begin(&mut self, mouse_y: Pixels, cx: &mut Context<'_, Self>) {
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let bounds = handle.bounds();
        let viewport = f32::from(bounds.size.height);
        let max = f32::from(handle.max_offset().height);
        let (thumb_h, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let scroll = -f32::from(handle.offset().y);
        let thumb_y = (scroll / max) * travel;
        let relative = f32::from(mouse_y) - f32::from(bounds.top());
        let grab = if relative >= thumb_y && relative <= thumb_y + thumb_h {
            relative - thumb_y
        } else {
            thumb_h / 2.0
        };
        self.vscroll_grab = Some(grab);
        self.vscroll_set(relative, grab, cx);
    }

    fn vscroll_set(&self, relative: f32, grab: f32, cx: &mut Context<'_, Self>) {
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let viewport = f32::from(handle.bounds().size.height);
        let max = f32::from(handle.max_offset().height);
        let (_, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let thumb_y = (relative - grab).clamp(0.0, travel);
        let scroll = thumb_y / travel * max;
        let x = handle.offset().x;
        handle.set_offset(Point::new(x, px(-scroll)));
        cx.notify();
    }

    pub(super) fn hscroll_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some(grab) = self.hscroll_grab else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.hscroll_grab = None;
            cx.notify();
            return;
        }
        let relative = f32::from(event.position.x) - f32::from(self.hscroll.bounds().left());
        self.hscroll_set(relative, grab, cx);
    }

    pub(super) fn vscroll_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some(grab) = self.vscroll_grab else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.vscroll_grab = None;
            cx.notify();
            return;
        }
        let handle = self.list_scroll.0.borrow().base_handle.clone();
        let relative = f32::from(event.position.y) - f32::from(handle.bounds().top());
        self.vscroll_set(relative, grab, cx);
    }
}

fn design_cell(id: SharedString, width: f32, theme: Theme, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(DESIGN_ROW_HEIGHT))
        .w(px(width))
        .flex_none()
        .whitespace_nowrap()
        .overflow_hidden()
        .border_r_1()
        .border_b_1()
        .border_color(rgb(theme.grid_line))
        .when(selected, |style| {
            style.text_color(rgb(theme.tree_selected_text))
        })
}

#[allow(clippy::too_many_arguments)]
fn design_edit_cell(
    id: SharedString,
    width: f32,
    text: String,
    theme: Theme,
    selected: bool,
    weak: &WeakEntity<TableDesignView>,
    row: usize,
    column: FieldColumn,
    editor: Option<Entity<TextInput>>,
) -> AnyElement {
    let cell = design_cell(id, width, theme, selected);
    if let Some(editor) = editor {
        return cell
            .child(div().w(px(width)).h(px(DESIGN_ROW_HEIGHT)).child(editor))
            .into_any_element();
    }
    let weak = weak.clone();
    cell.pr(px(4.0))
        .cursor_text()
        .on_click(move |_event, window, cx| {
            let _ = weak.update(cx, |view, cx| view.begin_edit(row, column, window, cx));
        })
        .child(text)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn design_type_cell(
    id: SharedString,
    width: f32,
    text: String,
    theme: Theme,
    selected: bool,
    weak: &WeakEntity<TableDesignView>,
    row: usize,
    anchor: Rc<RefCell<BTreeMap<usize, Point<Pixels>>>>,
) -> AnyElement {
    let weak = weak.clone();
    div()
        .flex()
        .items_center()
        .h(px(DESIGN_ROW_HEIGHT))
        .w(px(width))
        .flex_none()
        .whitespace_nowrap()
        .overflow_hidden()
        .border_r_1()
        .border_b_1()
        .border_color(rgb(theme.grid_line))
        .when(selected, |style| {
            style.text_color(rgb(theme.tree_selected_text))
        })
        .pr(px(4.0))
        .cursor_pointer()
        .on_children_prepainted(move |bounds, _window, _cx| {
            if let Some(first) = bounds.first() {
                anchor
                    .borrow_mut()
                    .insert(row, Point::new(first.left(), first.bottom()));
            }
        })
        .id(id)
        .on_click(move |_event, window, cx| {
            let _ = weak.update(cx, |view, cx| view.open_type_combo(row, window, cx));
        })
        .child(div().flex_1().min_w(px(0.0)).overflow_hidden().child(text))
        .child(
            svg()
                .path("icons/chevron-down.svg")
                .w(px(10.0))
                .h(px(10.0))
                .flex_none()
                .text_color(rgb(theme.text_muted)),
        )
        .into_any_element()
}

fn design_check_cell(
    id: SharedString,
    width: f32,
    checked: bool,
    theme: Theme,
    toggle: Option<(WeakEntity<TableDesignView>, usize)>,
) -> AnyElement {
    let cell = design_cell(id, width, theme, false)
        .justify_center()
        .child(checkbox_box(checked, theme));
    let Some((weak, row)) = toggle else {
        return cell.into_any_element();
    };
    cell.cursor_pointer()
        .on_click(move |_event, _window, cx| {
            let _ = weak.update(cx, |view, cx| view.toggle_not_null(row, cx));
        })
        .into_any_element()
}

fn design_key_cell(
    id: SharedString,
    width: f32,
    column: &navidog_core::ColumnDef,
    theme: Theme,
) -> AnyElement {
    let cell = design_cell(id, width, theme, false).px_2();
    if !column.primary_key {
        return cell.into_any_element();
    }
    cell.child(
        svg()
            .path("icons/primary_key.svg")
            .w(px(13.0))
            .h(px(13.0))
            .flex_none()
            .text_color(rgb(theme.warning)),
    )
    .child(div().ml_1().child("1"))
    .into_any_element()
}

fn option_row(label: impl Into<String>, input: Entity<TextInput>, theme: Theme) -> Div {
    let label = label.into();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(150.0))
                .flex_none()
                .text_size(px(12.0))
                .child(label),
        )
        .child(div().w(px(300.0)).h(px(22.0)).child(input))
        .text_color(rgb(theme.text))
}

fn simple_table(
    theme: Theme,
    id: &'static str,
    columns: &[(&'static str, f32)],
    rows: Vec<Vec<String>>,
) -> AnyElement {
    let mut header = div()
        .flex()
        .flex_row()
        .flex_none()
        .h(px(DESIGN_HEADER_HEIGHT))
        .bg(rgb(theme.header_bg))
        .border_b_1()
        .border_color(rgb(theme.border));
    for (key, width) in columns {
        header = header.child(
            div()
                .flex()
                .items_center()
                .h(px(DESIGN_HEADER_HEIGHT))
                .w(px(*width))
                .flex_none()
                .px_2()
                .whitespace_nowrap()
                .overflow_hidden()
                .border_r_1()
                .border_color(rgb(theme.grid_line))
                .child(t!(*key).to_string()),
        );
    }

    let mut body = div()
        .id(id)
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll();
    for (index, row) in rows.iter().enumerate() {
        let mut row_element = div()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(DESIGN_ROW_HEIGHT))
            .bg(rgb(if index % 2 == 1 {
                theme.row_alt_bg
            } else {
                theme.editor_bg
            }));
        for (column_index, value) in row.iter().enumerate() {
            let width = columns
                .get(column_index)
                .map(|(_, width)| *width)
                .unwrap_or(120.0);
            row_element = row_element.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(DESIGN_ROW_HEIGHT))
                    .w(px(width))
                    .flex_none()
                    .px_2()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .border_r_1()
                    .border_b_1()
                    .border_color(rgb(theme.grid_line))
                    .child(value.clone()),
            );
        }
        body = body.child(row_element);
    }

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .child(header)
        .child(body)
        .into_any_element()
}
