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
        if self.index_combo.is_some() {
            root = root.child(self.render_index_combo(cx));
        }
        if self.index_fields_open {
            root = root.child(self.render_index_fields_popup(cx));
        }
        if self.fk_combo.is_some() {
            root = root.child(self.render_fk_combo(cx));
        }
        if self.fk_fields_open {
            root = root.child(self.render_fk_fields_popup(cx));
        }
        if self.fk_ref_open {
            root = root.child(self.render_fk_ref_popup(cx));
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
        let indexes = self.tab == DesignTab::Indexes && !self.loading;
        let foreign_keys = self.tab == DesignTab::ForeignKeys && !self.loading;
        let selected = self.selected_field.is_some();
        let can_up = self.selected_field.map(|index| index > 0).unwrap_or(false);
        let can_down = self
            .selected_field
            .map(|index| index + 1 < self.schema.columns.len())
            .unwrap_or(false);
        let can_delete_index = self
            .selected_index
            .and_then(|row| self.schema.indexes.get(row))
            .map(|index| !index.primary)
            .unwrap_or(false);
        let save_enabled = !self.saving && !self.loading;

        let mut bar = div()
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
            .child(toolbar_separator(theme));

        if fields {
            bar = bar
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
                    cx.listener(|this, _event, _window, cx| {
                        this.request_delete(DesignDeleteKind::Fields, cx)
                    }),
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
                ));
        } else if indexes {
            bar = bar
                .child(ui::toolbar_item(
                    "design-add-index",
                    "icons/add_field.svg",
                    t!("design.add_index").to_string(),
                    indexes,
                    theme,
                    cx.listener(|this, _event, _window, cx| this.add_index(cx)),
                ))
                .child(ui::toolbar_item(
                    "design-delete-index",
                    "icons/delete_field.svg",
                    t!("design.delete_index").to_string(),
                    indexes && can_delete_index,
                    theme,
                    cx.listener(|this, _event, _window, cx| {
                        this.request_delete(DesignDeleteKind::Indexes, cx)
                    }),
                ));
        } else if foreign_keys {
            let can_delete_fk = self.selected_fk.is_some();
            bar = bar
                .child(ui::toolbar_item(
                    "design-add-fk",
                    "icons/add_field.svg",
                    t!("design.add_foreign_key").to_string(),
                    foreign_keys,
                    theme,
                    cx.listener(|this, _event, _window, cx| this.add_foreign_key(cx)),
                ))
                .child(ui::toolbar_item(
                    "design-delete-fk",
                    "icons/delete_field.svg",
                    t!("design.delete_foreign_key").to_string(),
                    foreign_keys && can_delete_fk,
                    theme,
                    cx.listener(|this, _event, _window, cx| {
                        this.request_delete(DesignDeleteKind::ForeignKeys, cx)
                    }),
                ));
        }

        bar
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
        let content: AnyElement = if self.loading && self.original.is_none() {
            div()
                .p_3()
                .text_color(rgb(self.theme.text_muted))
                .child(t!("design.loading").to_string())
                .into_any_element()
        } else {
            match self.tab {
                DesignTab::Fields => self.render_fields(cx),
                DesignTab::Indexes => self.render_indexes(cx),
                DesignTab::ForeignKeys => self.render_foreign_keys(cx),
                DesignTab::Triggers => self.render_triggers(),
                DesignTab::Options => self.render_options(cx),
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
        let field_rows = self.field_rows.clone();
        let edit = self
            .edit
            .as_ref()
            .map(|editor| (editor.row, editor.column, editor.input.clone()));
        let weak = self.self_weak.clone();
        let combo_anchor = self.combo_anchor.clone();

        let list = uniform_list(
            SharedString::from(format!("design-fields-{}", self.id)),
            columns.len().max(1),
            move |range, _window, _cx| {
                range
                    .map(|row| {
                        if row >= columns.len() {
                            return placeholder_row(PlaceholderKind::Field, &widths, theme, &weak);
                        }
                        let column = &columns[row];
                        let row_selected = field_rows.contains(row);
                        let row_active = selected == Some(row);
                        let row_background = if row % 2 == 1 {
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
                        let gutter_weak = weak.clone();

                        div()
                            .flex()
                            .flex_row()
                            .h(px(DESIGN_ROW_HEIGHT))
                            .bg(rgb(row_background))
                            .child(
                                div()
                                    .id(SharedString::from(format!("design-gutter-{row}")))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .h(px(DESIGN_ROW_HEIGHT))
                                    .w(px(DESIGN_GUTTER_WIDTH))
                                    .flex_none()
                                    .border_r_1()
                                    .border_color(rgb(theme.grid_line))
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        move |event: &MouseDownEvent, _window, cx| {
                                            let _ = gutter_weak.update(cx, |view, cx| {
                                                view.select_field_row(row, &event.modifiers, cx)
                                            });
                                        },
                                    )
                                    .when(row_active, |gutter| {
                                        gutter.child(
                                            svg()
                                                .path("icons/row_marker.svg")
                                                .w(px(7.0))
                                                .h(px(7.0))
                                                .flex_none()
                                                .text_color(rgb(theme.primary)),
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
                                false,
                                &weak,
                                row,
                                combo_anchor.clone(),
                            ))
                            .child(design_edit_cell(
                                SharedString::from(format!("design-length-{row}")),
                                FIELD_LENGTH_WIDTH,
                                column.length.clone(),
                                theme,
                                false,
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
                                false,
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
                                column.primary_key,
                                theme,
                                Some((weak.clone(), row)),
                            ))
                            .child(design_edit_cell(
                                SharedString::from(format!("design-comment-{row}")),
                                FIELD_COMMENT_WIDTH,
                                column.comment.clone(),
                                theme,
                                false,
                                &weak,
                                row,
                                FieldColumn::Comment,
                                editor_for(FieldColumn::Comment),
                            ))
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.list_scroll)
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

    fn render_indexes(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let widths = [
            INDEX_NAME_WIDTH,
            INDEX_FIELDS_WIDTH,
            INDEX_KIND_WIDTH,
            INDEX_METHOD_WIDTH,
            INDEX_COMMENT_WIDTH,
        ];
        let content_width: f32 = DESIGN_GUTTER_WIDTH + widths.iter().sum::<f32>();
        let indexes = self.schema.indexes.clone();
        let selected = self.selected_index;
        let index_rows = self.index_rows.clone();
        let edit = self
            .index_edit
            .as_ref()
            .map(|editor| (editor.row, editor.column, editor.input.clone()));
        let weak = self.self_weak.clone();
        let combo_anchor = self.index_combo_anchor.clone();
        let fields_anchor = self.index_fields_anchor.clone();

        let list = uniform_list(
            SharedString::from(format!("design-indexes-{}", self.id)),
            indexes.len().max(1),
            move |range, _window, _cx| {
                range
                    .map(|row| {
                        if row >= indexes.len() {
                            return placeholder_row(PlaceholderKind::Index, &widths, theme, &weak);
                        }
                        let index = &indexes[row];
                        let row_selected = index_rows.contains(row);
                        let row_active = selected == Some(row);
                        let row_background = if row % 2 == 1 {
                            theme.row_alt_bg
                        } else {
                            theme.editor_bg
                        };
                        let editor_for = |target: IndexColumn| {
                            edit.as_ref()
                                .filter(|(row_index, column_kind, _)| {
                                    *row_index == row && *column_kind == target
                                })
                                .map(|(_, _, input)| input.clone())
                        };
                        let kind_label = if index.primary {
                            t!("design.index.kind.primary").to_string()
                        } else {
                            t!(IndexKind::of(index).label_key()).to_string()
                        };
                        let method = index_method(index);
                        let gutter_weak = weak.clone();

                        div()
                            .flex()
                            .flex_row()
                            .h(px(DESIGN_ROW_HEIGHT))
                            .bg(rgb(row_background))
                            .child(
                                div()
                                    .id(SharedString::from(format!("index-gutter-{row}")))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .h(px(DESIGN_ROW_HEIGHT))
                                    .w(px(DESIGN_GUTTER_WIDTH))
                                    .flex_none()
                                    .border_r_1()
                                    .border_color(rgb(theme.grid_line))
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        move |event: &MouseDownEvent, _window, cx| {
                                            let _ = gutter_weak.update(cx, |view, cx| {
                                                view.select_index_row(row, &event.modifiers, cx)
                                            });
                                        },
                                    )
                                    .when(row_active, |gutter| {
                                        gutter.child(
                                            svg()
                                                .path("icons/row_marker.svg")
                                                .w(px(7.0))
                                                .h(px(7.0))
                                                .flex_none()
                                                .text_color(rgb(theme.primary)),
                                        )
                                    }),
                            )
                            .child(index_edit_cell(
                                SharedString::from(format!("index-name-{row}")),
                                INDEX_NAME_WIDTH,
                                index.name.clone(),
                                theme,
                                row_selected,
                                &weak,
                                row,
                                IndexColumn::Name,
                                editor_for(IndexColumn::Name),
                            ))
                            .child(index_fields_cell(
                                SharedString::from(format!("index-fields-{row}")),
                                INDEX_FIELDS_WIDTH,
                                index.columns.join(", "),
                                theme,
                                false,
                                &weak,
                                row,
                                fields_anchor.clone(),
                            ))
                            .child(index_choice_cell(
                                SharedString::from(format!("index-kind-{row}")),
                                INDEX_KIND_WIDTH,
                                kind_label,
                                !index.primary,
                                theme,
                                false,
                                &weak,
                                row,
                                IndexComboField::Kind,
                                combo_anchor.clone(),
                            ))
                            .child(index_choice_cell(
                                SharedString::from(format!("index-method-{row}")),
                                INDEX_METHOD_WIDTH,
                                method.unwrap_or_default().to_string(),
                                !index.primary && method.is_some(),
                                theme,
                                false,
                                &weak,
                                row,
                                IndexComboField::Method,
                                combo_anchor.clone(),
                            ))
                            .child(index_edit_cell(
                                SharedString::from(format!("index-comment-{row}")),
                                INDEX_COMMENT_WIDTH,
                                index.comment.clone(),
                                theme,
                                false,
                                &weak,
                                row,
                                IndexColumn::Comment,
                                editor_for(IndexColumn::Comment),
                            ))
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h(px(0.0));

        let header = self.render_index_header(theme, &widths);

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
            .into_any_element()
    }

    fn render_index_header(&self, theme: Theme, widths: &[f32; 5]) -> Div {
        let labels = [
            "design.index.name",
            "design.index.fields",
            "design.index.type",
            "design.index.method",
            "design.index.comment",
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

    fn render_foreign_keys(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let widths = [
            FK_NAME_WIDTH,
            FK_COLUMNS_WIDTH,
            FK_SCHEMA_WIDTH,
            FK_REF_TABLE_WIDTH,
            FK_REF_COLUMNS_WIDTH,
            FK_DELETE_WIDTH,
            FK_UPDATE_WIDTH,
        ];
        let content_width: f32 = DESIGN_GUTTER_WIDTH + widths.iter().sum::<f32>();
        let foreign_keys = self.schema.foreign_keys.clone();
        let database = self.database.clone();
        let selected = self.selected_fk;
        let fk_rows = self.fk_rows.clone();
        let edit = self
            .fk_edit
            .as_ref()
            .map(|editor| (editor.row, editor.column, editor.input.clone()));
        let weak = self.self_weak.clone();
        let combo_anchor = self.fk_combo_anchor.clone();
        let fields_anchor = self.fk_fields_anchor.clone();
        let ref_anchor = self.fk_ref_anchor.clone();

        let list = uniform_list(
            SharedString::from(format!("design-fks-{}", self.id)),
            foreign_keys.len().max(1),
            move |range, _window, _cx| {
                range
                    .map(|row| {
                        if row >= foreign_keys.len() {
                            return placeholder_row(
                                PlaceholderKind::ForeignKey,
                                &widths,
                                theme,
                                &weak,
                            );
                        }
                        let foreign_key = &foreign_keys[row];
                        let row_selected = fk_rows.contains(row);
                        let row_active = selected == Some(row);
                        let row_background = if row % 2 == 1 {
                            theme.row_alt_bg
                        } else {
                            theme.editor_bg
                        };
                        let editor_for = |target: FkColumn| {
                            edit.as_ref()
                                .filter(|(row_index, column_kind, _)| {
                                    *row_index == row && *column_kind == target
                                })
                                .map(|(_, _, input)| input.clone())
                        };
                        let gutter_weak = weak.clone();

                        div()
                            .flex()
                            .flex_row()
                            .h(px(DESIGN_ROW_HEIGHT))
                            .bg(rgb(row_background))
                            .child(
                                div()
                                    .id(SharedString::from(format!("fk-gutter-{row}")))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .h(px(DESIGN_ROW_HEIGHT))
                                    .w(px(DESIGN_GUTTER_WIDTH))
                                    .flex_none()
                                    .border_r_1()
                                    .border_color(rgb(theme.grid_line))
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        move |event: &MouseDownEvent, _window, cx| {
                                            let _ = gutter_weak.update(cx, |view, cx| {
                                                view.select_fk_row(row, &event.modifiers, cx)
                                            });
                                        },
                                    )
                                    .when(row_active, |gutter| {
                                        gutter.child(
                                            svg()
                                                .path("icons/row_marker.svg")
                                                .w(px(7.0))
                                                .h(px(7.0))
                                                .flex_none()
                                                .text_color(rgb(theme.primary)),
                                        )
                                    }),
                            )
                            .child(fk_edit_cell(
                                SharedString::from(format!("fk-name-{row}")),
                                FK_NAME_WIDTH,
                                foreign_key.name.clone(),
                                theme,
                                row_selected,
                                &weak,
                                row,
                                FkColumn::Name,
                                editor_for(FkColumn::Name),
                            ))
                            .child(fk_picker_cell(
                                SharedString::from(format!("fk-columns-{row}")),
                                FK_COLUMNS_WIDTH,
                                foreign_key.columns.join(", "),
                                theme,
                                false,
                                &weak,
                                row,
                                FkPicker::Columns,
                                fields_anchor.clone(),
                            ))
                            .child(
                                design_cell(
                                    SharedString::from(format!("fk-schema-{row}")),
                                    FK_SCHEMA_WIDTH,
                                    theme,
                                    false,
                                )
                                .px_2()
                                .child(database.clone()),
                            )
                            .child(fk_edit_cell(
                                SharedString::from(format!("fk-ref-table-{row}")),
                                FK_REF_TABLE_WIDTH,
                                foreign_key.referenced_table.clone(),
                                theme,
                                false,
                                &weak,
                                row,
                                FkColumn::ReferencedTable,
                                editor_for(FkColumn::ReferencedTable),
                            ))
                            .child(fk_picker_cell(
                                SharedString::from(format!("fk-ref-columns-{row}")),
                                FK_REF_COLUMNS_WIDTH,
                                foreign_key.referenced_columns.join(", "),
                                theme,
                                false,
                                &weak,
                                row,
                                FkPicker::Referenced,
                                ref_anchor.clone(),
                            ))
                            .child(fk_choice_cell(
                                SharedString::from(format!("fk-delete-{row}")),
                                FK_DELETE_WIDTH,
                                rule_label(&foreign_key.on_delete),
                                theme,
                                false,
                                &weak,
                                row,
                                FkComboField::OnDelete,
                                combo_anchor.clone(),
                            ))
                            .child(fk_choice_cell(
                                SharedString::from(format!("fk-update-{row}")),
                                FK_UPDATE_WIDTH,
                                rule_label(&foreign_key.on_update),
                                theme,
                                false,
                                &weak,
                                row,
                                FkComboField::OnUpdate,
                                combo_anchor.clone(),
                            ))
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h(px(0.0));

        let header = self.render_fk_header(theme, &widths);

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
            .into_any_element()
    }

    fn render_fk_header(&self, theme: Theme, widths: &[f32; 7]) -> Div {
        let labels = [
            "design.fk.name",
            "design.fk.columns",
            "design.fk.schema",
            "design.fk.ref_table",
            "design.fk.ref_columns",
            "design.fk.delete",
            "design.fk.update",
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

    fn render_options(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        self.sync_option_combos(cx);
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(option_combo_row(
                t!("design.options.engine"),
                Some(self.engine_combo.clone()),
                theme,
            ))
            .child(option_combo_row(
                t!("design.options.charset"),
                Some(self.charset_combo.clone()),
                theme,
            ))
            .child(option_combo_row(
                t!("design.options.collation"),
                Some(self.collation_combo.clone()),
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
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .h(px(20.0))
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
                    .child(data_type.to_string())
                    .child(
                        svg()
                            .path("icons/check.svg")
                            .w(px(12.0))
                            .h(px(12.0))
                            .flex_none()
                            .text_color(rgb(theme.tree_selected_text))
                            .opacity(if selected { 1.0 } else { 0.0 }),
                    ),
            );
        }

        let mut popup = ui::popup_panel(theme)
            .id("design-type-list")
            .left(anchor.x)
            .top(anchor.y)
            .w(px(FIELD_TYPE_WIDTH))
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

    /// The dropdown popup for an index row's kind/method cells.
    fn render_index_combo(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some((row, field)) = self.index_combo else {
            return div().into_any_element();
        };
        let Some(index) = self.schema.indexes.get(row) else {
            return div().into_any_element();
        };
        let origin = *self.root_anchor.borrow();
        let anchor = self
            .index_combo_anchor
            .borrow()
            .get(&(row, field))
            .copied()
            .unwrap_or_default();
        let anchor = Point::new(anchor.x - origin.x, anchor.y - origin.y);
        let weak = self.self_weak.clone();

        let mut options = div()
            .id("design-index-options")
            .max_h(px(300.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        let width = match field {
            IndexComboField::Kind => {
                let current = IndexKind::of(index);
                for kind in IndexKind::ALL {
                    options = options.child(index_choice_option(
                        format!("index-kind-option-{}", kind.value()),
                        t!(kind.label_key()).to_string(),
                        kind == current,
                        row,
                        field,
                        kind.value(),
                        &weak,
                        theme,
                    ));
                }
                INDEX_KIND_WIDTH
            }
            IndexComboField::Method => {
                let current = index_method(index);
                for method in INDEX_METHODS {
                    options = options.child(index_choice_option(
                        format!("index-method-option-{method}"),
                        method.to_string(),
                        current == Some(method),
                        row,
                        field,
                        method,
                        &weak,
                        theme,
                    ));
                }
                INDEX_METHOD_WIDTH
            }
        };

        let popup = ui::popup_panel(theme)
            .id("design-index-list")
            .left(anchor.x)
            .top(anchor.y)
            .w(px(width))
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.index_combo = None;
                cx.notify();
            }));
        deferred(popup.child(options))
            .with_priority(100)
            .into_any_element()
    }

    /// The "choose fields" popup for the selected index.
    fn render_index_fields_popup(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some(row) = self.selected_index else {
            return div().into_any_element();
        };
        let Some(index) = self.schema.indexes.get(row) else {
            return div().into_any_element();
        };
        let origin = *self.root_anchor.borrow();
        let anchor = *self.index_fields_anchor.borrow();
        let anchor = Point::new(anchor.x - origin.x, anchor.y - origin.y);
        let current = index.columns.clone();
        let columns = self.schema.columns.clone();
        let weak = self.self_weak.clone();

        let mut list = div()
            .id("design-index-fields-list")
            .max_h(px(260.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for column in &columns {
            let name = column.name.clone();
            let option_name = name.clone();
            let checked = current.iter().any(|existing| existing == &name);
            let weak = weak.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("index-field-option-{name}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(20.0))
                    .px_2()
                    .flex_none()
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(move |_event, _window, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            view.toggle_index_column(row, &option_name, cx)
                        });
                    })
                    .child(checkbox_box(checked, theme))
                    .child(name),
            );
        }

        let popup = ui::popup_panel(theme)
            .id("design-index-fields")
            .left(anchor.x)
            .top(anchor.y)
            .w(px(220.0))
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.index_fields_open = false;
                cx.notify();
            }));
        deferred(popup.child(list))
            .with_priority(100)
            .into_any_element()
    }

    /// The dropdown popup for a foreign-key row's on-delete/on-update cells.
    fn render_fk_combo(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let Some((row, field)) = self.fk_combo else {
            return div().into_any_element();
        };
        if self.schema.foreign_keys.get(row).is_none() {
            return div().into_any_element();
        }
        let origin = *self.root_anchor.borrow();
        let anchor = self
            .fk_combo_anchor
            .borrow()
            .get(&(row, field))
            .copied()
            .unwrap_or_default();
        let anchor = Point::new(anchor.x - origin.x, anchor.y - origin.y);
        let weak = self.self_weak.clone();
        let current = match field {
            FkComboField::OnDelete => self.schema.foreign_keys[row].on_delete.as_str(),
            FkComboField::OnUpdate => self.schema.foreign_keys[row].on_update.as_str(),
        };

        let mut options = div()
            .id("design-fk-options")
            .max_h(px(300.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for rule in FK_RULES {
            options = options.child(fk_choice_option(
                format!("fk-rule-option-{}", rule.replace(' ', "_")),
                rule_label(rule),
                rule == current,
                row,
                field,
                rule,
                &weak,
                theme,
            ));
        }

        let popup = ui::popup_panel(theme)
            .id("design-fk-list")
            .left(anchor.x)
            .top(anchor.y)
            .w(px(FK_DELETE_WIDTH.max(FK_UPDATE_WIDTH)))
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.fk_combo = None;
                cx.notify();
            }));
        deferred(popup.child(options))
            .with_priority(100)
            .into_any_element()
    }

    fn render_fk_fields_popup(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let columns: Vec<String> = self
            .schema
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect();
        self.render_fk_picker_popup(columns, true, cx)
    }

    fn render_fk_ref_popup(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let columns = self.fk_ref_columns.clone();
        self.render_fk_picker_popup(columns, false, cx)
    }

    /// Shared renderer for the foreign key's child/referenced column pickers. `child` selects which
    /// list the toggles write to.
    fn render_fk_picker_popup(
        &self,
        columns: Vec<String>,
        child: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(row) = self.selected_fk else {
            return div().into_any_element();
        };
        let Some(foreign_key) = self.schema.foreign_keys.get(row) else {
            return div().into_any_element();
        };
        let selected: Vec<String> = if child {
            foreign_key.columns.clone()
        } else {
            foreign_key.referenced_columns.clone()
        };
        let origin = *self.root_anchor.borrow();
        let anchor = if child {
            *self.fk_fields_anchor.borrow()
        } else {
            *self.fk_ref_anchor.borrow()
        };
        let anchor = Point::new(anchor.x - origin.x, anchor.y - origin.y);
        let weak = self.self_weak.clone();

        let mut list = div()
            .id("design-fk-fields-list")
            .max_h(px(260.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if columns.is_empty() {
            list = list.child(
                div()
                    .p_2()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("design.fk.no_columns").to_string()),
            );
        }
        for column in &columns {
            let name = column.clone();
            let option_name = name.clone();
            let checked = selected.iter().any(|existing| existing == &name);
            let weak = weak.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("fk-field-option-{name}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(20.0))
                    .px_2()
                    .flex_none()
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(move |_event, _window, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            if child {
                                view.toggle_fk_column(row, &option_name, cx)
                            } else {
                                view.toggle_fk_ref_column(row, &option_name, cx)
                            }
                        });
                    })
                    .child(checkbox_box(checked, theme))
                    .child(name),
            );
        }

        let popup = ui::popup_panel(theme)
            .id("design-fk-fields")
            .left(anchor.x)
            .top(anchor.y)
            .w(px(220.0))
            .on_mouse_down_out(cx.listener(move |this, _event, _window, cx| {
                if child {
                    this.fk_fields_open = false;
                } else {
                    this.fk_ref_open = false;
                }
                cx.notify();
            }));
        deferred(popup.child(list))
            .with_priority(100)
            .into_any_element()
    }

    fn render_hscrollbar(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let viewport = f32::from(self.hscroll.bounds().size.width);
        let max = f32::from(self.hscroll.max_offset().x);
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
        let max = f32::from(handle.max_offset().y);
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
        let max = f32::from(self.hscroll.max_offset().x);
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
        let max = f32::from(self.hscroll.max_offset().x);
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
        let max = f32::from(handle.max_offset().y);
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
        let max = f32::from(handle.max_offset().y);
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

/// The grid whose blank placeholder row is being rendered. Clicking it creates the first entry.
#[derive(Clone, Copy)]
enum PlaceholderKind {
    Field,
    Index,
    ForeignKey,
}

/// A blank editable row shown below the header when a designer grid is empty. Clicking any cell
/// creates the first entry and starts editing its name, so an empty grid is never a dead end.
fn placeholder_row(
    kind: PlaceholderKind,
    widths: &[f32],
    theme: Theme,
    weak: &WeakEntity<TableDesignView>,
) -> Div {
    let prefix = match kind {
        PlaceholderKind::Field => "design-field",
        PlaceholderKind::Index => "design-index",
        PlaceholderKind::ForeignKey => "design-fk",
    };
    let mut row = div()
        .flex()
        .flex_row()
        .h(px(DESIGN_ROW_HEIGHT))
        .bg(rgb(theme.editor_bg))
        .child(
            div()
                .w(px(DESIGN_GUTTER_WIDTH))
                .h(px(DESIGN_ROW_HEIGHT))
                .flex_none()
                .border_r_1()
                .border_color(rgb(theme.grid_line)),
        );
    for (index, width) in widths.iter().enumerate() {
        let weak = weak.clone();
        row = row.child(
            design_cell(
                SharedString::from(format!("{prefix}-empty-{index}")),
                *width,
                theme,
                false,
            )
            .cursor_pointer()
            .on_click(move |_event, window, cx| {
                let _ = weak.update(cx, |view, vcx| match kind {
                    PlaceholderKind::Field => view.begin_empty_field(window, vcx),
                    PlaceholderKind::Index => view.begin_empty_index(window, vcx),
                    PlaceholderKind::ForeignKey => view.begin_empty_foreign_key(window, vcx),
                });
            }),
        );
    }
    row
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
            style.text_color(rgb(theme.grid_selection_text))
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
    let cell = design_cell(id, width, theme, selected)
        .when(selected, |cell| cell.bg(rgb(theme.grid_selection_bg)));
    if let Some(editor) = editor {
        return cell
            .bg(rgb(theme.input_bg))
            // The selected-cell text color (white) must not leak into the editor, or its text is
            // invisible on the light input background.
            .text_color(rgb(theme.text))
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
fn index_edit_cell(
    id: SharedString,
    width: f32,
    text: String,
    theme: Theme,
    selected: bool,
    weak: &WeakEntity<TableDesignView>,
    row: usize,
    column: IndexColumn,
    editor: Option<Entity<TextInput>>,
) -> AnyElement {
    let cell = design_cell(id, width, theme, selected)
        .when(selected, |cell| cell.bg(rgb(theme.grid_selection_bg)));
    if let Some(editor) = editor {
        return cell
            .bg(rgb(theme.input_bg))
            // The selected-cell text color (white) must not leak into the editor, or its text is
            // invisible on the light input background.
            .text_color(rgb(theme.text))
            .child(div().w(px(width)).h(px(DESIGN_ROW_HEIGHT)).child(editor))
            .into_any_element();
    }
    let weak = weak.clone();
    cell.pr(px(4.0))
        .cursor_text()
        .on_click(move |_event, window, cx| {
            let _ = weak.update(cx, |view, cx| {
                view.begin_index_edit(row, column, window, cx)
            });
        })
        .child(text)
        .into_any_element()
}

/// The index grid's "Fields" cell: the joined column names plus a "..." button that opens the
/// field picker.
#[allow(clippy::too_many_arguments)]
fn index_fields_cell(
    id: SharedString,
    width: f32,
    text: String,
    theme: Theme,
    selected: bool,
    weak: &WeakEntity<TableDesignView>,
    row: usize,
    anchor: Rc<RefCell<Point<Pixels>>>,
) -> AnyElement {
    let weak = weak.clone();
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
            style.text_color(rgb(theme.grid_selection_text))
        })
        .child(div().flex_1().min_w(px(0.0)).overflow_hidden().child(text))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(20.0))
                .h(px(18.0))
                .flex_none()
                .mr_1()
                .border_1()
                .border_color(rgb(theme.border))
                .rounded(px(3.0))
                .bg(rgb(theme.button_bg))
                .cursor_pointer()
                .on_children_prepainted(move |bounds, _window, _cx| {
                    if let Some(first) = bounds.first() {
                        *anchor.borrow_mut() = Point::new(first.left(), first.bottom());
                    }
                })
                .id(SharedString::from(format!("index-fields-btn-{row}")))
                .on_click(move |_event, window, cx| {
                    let _ = weak.update(cx, |view, cx| view.open_index_fields(row, window, cx));
                })
                .child(div().text_size(px(12.0)).child("...")),
        )
        .into_any_element()
}

/// A dropdown-style cell of the index grid (kind or method).
#[allow(clippy::too_many_arguments)]
fn index_choice_cell(
    id: SharedString,
    width: f32,
    text: String,
    enabled: bool,
    theme: Theme,
    selected: bool,
    weak: &WeakEntity<TableDesignView>,
    row: usize,
    field: IndexComboField,
    anchor: IndexComboAnchors,
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
            style.text_color(rgb(theme.grid_selection_text))
        })
        .pr(px(4.0))
        .when(enabled, |style| style.cursor_pointer())
        .on_children_prepainted(move |bounds, _window, _cx| {
            if let Some(first) = bounds.first() {
                anchor
                    .borrow_mut()
                    .insert((row, field), Point::new(first.left(), first.bottom()));
            }
        })
        .id(id)
        .on_click(move |_event, window, cx| {
            if enabled {
                let _ = weak.update(cx, |view, cx| view.open_index_combo(row, field, window, cx));
            }
        })
        .child(div().flex_1().min_w(px(0.0)).overflow_hidden().child(text))
        .child(
            svg()
                .path("icons/chevron-down.svg")
                .w(px(10.0))
                .h(px(10.0))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .opacity(if enabled { 1.0 } else { 0.0 }),
        )
        .into_any_element()
}

/// One row of an index kind/method dropdown popup.
#[allow(clippy::too_many_arguments)]
fn index_choice_option(
    id: String,
    label: String,
    selected: bool,
    row: usize,
    field: IndexComboField,
    value: &'static str,
    weak: &WeakEntity<TableDesignView>,
    theme: Theme,
) -> AnyElement {
    let weak = weak.clone();
    div()
        .id(SharedString::from(id))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_1()
        .h(px(20.0))
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
            let _ = weak.update(cx, |view, cx| {
                view.select_index_combo(row, field, value, cx)
            });
        })
        .child(label)
        .child(
            svg()
                .path("icons/check.svg")
                .w(px(12.0))
                .h(px(12.0))
                .flex_none()
                .text_color(rgb(theme.tree_selected_text))
                .opacity(if selected { 1.0 } else { 0.0 }),
        )
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
            style.text_color(rgb(theme.grid_selection_text))
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
        .px_2()
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
    primary_key: bool,
    theme: Theme,
    toggle: Option<(WeakEntity<TableDesignView>, usize)>,
) -> AnyElement {
    let cell = design_cell(id, width, theme, false)
        .px_2()
        .child(checkbox_box(primary_key, theme));
    let Some((weak, row)) = toggle else {
        return cell.into_any_element();
    };
    cell.cursor_pointer()
        .on_click(move |_event, _window, cx| {
            let _ = weak.update(cx, |view, cx| view.toggle_field_primary_key(row, cx));
        })
        .into_any_element()
}

fn rule_label(rule: &str) -> String {
    if rule.trim().is_empty() {
        t!("design.fk.rule_default").to_string()
    } else {
        rule.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
fn fk_edit_cell(
    id: SharedString,
    width: f32,
    text: String,
    theme: Theme,
    selected: bool,
    weak: &WeakEntity<TableDesignView>,
    row: usize,
    column: FkColumn,
    editor: Option<Entity<TextInput>>,
) -> AnyElement {
    let cell = design_cell(id, width, theme, selected)
        .when(selected, |cell| cell.bg(rgb(theme.grid_selection_bg)));
    if let Some(editor) = editor {
        return cell
            .bg(rgb(theme.input_bg))
            // The selected-cell text color (white) must not leak into the editor, or its text is
            // invisible on the light input background.
            .text_color(rgb(theme.text))
            .child(div().w(px(width)).h(px(DESIGN_ROW_HEIGHT)).child(editor))
            .into_any_element();
    }
    let weak = weak.clone();
    cell.pr(px(4.0))
        .cursor_text()
        .on_click(move |_event, window, cx| {
            let _ = weak.update(cx, |view, cx| view.begin_fk_edit(row, column, window, cx));
        })
        .child(text)
        .into_any_element()
}

/// A foreign-key cell showing a column list with a "..." button that opens the matching picker.
#[allow(clippy::too_many_arguments)]
fn fk_picker_cell(
    id: SharedString,
    width: f32,
    text: String,
    theme: Theme,
    selected: bool,
    weak: &WeakEntity<TableDesignView>,
    row: usize,
    picker: FkPicker,
    anchor: Rc<RefCell<Point<Pixels>>>,
) -> AnyElement {
    let weak = weak.clone();
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
            style.text_color(rgb(theme.grid_selection_text))
        })
        .child(div().flex_1().min_w(px(0.0)).overflow_hidden().child(text))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(20.0))
                .h(px(18.0))
                .flex_none()
                .mr_1()
                .border_1()
                .border_color(rgb(theme.border))
                .rounded(px(3.0))
                .bg(rgb(theme.button_bg))
                .cursor_pointer()
                .on_children_prepainted(move |bounds, _window, _cx| {
                    if let Some(first) = bounds.first() {
                        *anchor.borrow_mut() = Point::new(first.left(), first.bottom());
                    }
                })
                .id(SharedString::from(format!("fk-picker-btn-{row}")))
                .on_click(move |_event, window, cx| {
                    let _ = weak.update(cx, |view, cx| match picker {
                        FkPicker::Columns => view.open_fk_fields(row, window, cx),
                        FkPicker::Referenced => view.open_fk_ref(row, window, cx),
                    });
                })
                .child(div().text_size(px(12.0)).child("...")),
        )
        .into_any_element()
}

/// A dropdown-style cell of the foreign-key grid (on delete / on update).
#[allow(clippy::too_many_arguments)]
fn fk_choice_cell(
    id: SharedString,
    width: f32,
    text: String,
    theme: Theme,
    selected: bool,
    weak: &WeakEntity<TableDesignView>,
    row: usize,
    field: FkComboField,
    anchor: FkComboAnchors,
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
            style.text_color(rgb(theme.grid_selection_text))
        })
        .pr(px(4.0))
        .cursor_pointer()
        .on_children_prepainted(move |bounds, _window, _cx| {
            if let Some(first) = bounds.first() {
                anchor
                    .borrow_mut()
                    .insert((row, field), Point::new(first.left(), first.bottom()));
            }
        })
        .id(id)
        .on_click(move |_event, window, cx| {
            let _ = weak.update(cx, |view, cx| view.open_fk_combo(row, field, window, cx));
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

/// One row of a foreign-key rule dropdown popup.
#[allow(clippy::too_many_arguments)]
fn fk_choice_option(
    id: String,
    label: String,
    selected: bool,
    row: usize,
    field: FkComboField,
    value: &'static str,
    weak: &WeakEntity<TableDesignView>,
    theme: Theme,
) -> AnyElement {
    let weak = weak.clone();
    div()
        .id(SharedString::from(id))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_1()
        .h(px(20.0))
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
            let _ = weak.update(cx, |view, cx| view.select_fk_combo(row, field, value, cx));
        })
        .child(label)
        .child(
            svg()
                .path("icons/check.svg")
                .w(px(12.0))
                .h(px(12.0))
                .flex_none()
                .text_color(rgb(theme.tree_selected_text))
                .opacity(if selected { 1.0 } else { 0.0 }),
        )
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

/// A labelled drop-down row (`label: [combo]`) for the Options tab.
fn option_combo_row(
    label: impl Into<String>,
    combo: Option<Entity<ComboBox>>,
    theme: Theme,
) -> Div {
    let label = label.into();
    let mut row = div().flex().flex_row().items_center().gap_2().child(
        div()
            .w(px(150.0))
            .flex_none()
            .text_size(px(12.0))
            .child(label),
    );
    if let Some(combo) = combo {
        row = row.child(combo);
    }
    row.text_color(rgb(theme.text))
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
