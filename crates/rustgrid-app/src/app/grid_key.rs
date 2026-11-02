//! The dedicated key page for collection keys (`hash` / `set`).
//!
//! A hash or set is a flat bag of field/value (or member) pairs. The SQL grid's WHERE/sort/field
//! chrome and its record toolbar do not apply to it, and the prototype presents it as a key page:
//! a key header bar with the type, TTL, memory and the collection actions, a numbered
//! Field/Value (or Member) table, and a compact pager. The table itself reuses the grid's rows,
//! cell selection and in-place editor, so editing behaves exactly like every other grid.

use super::*;

impl GridView {
    /// The Redis type shown on the dedicated key page, when this grid is one.
    pub(super) fn key_page_kind(&self) -> Option<&str> {
        self.state
            .key_status
            .as_ref()
            .and_then(|status| status.engine.as_deref())
            .filter(|kind| {
                matches!(
                    *kind,
                    "string" | "json" | "hash" | "set" | "list" | "zset" | "stream"
                )
            })
    }

    pub(super) fn is_key_page(&self) -> bool {
        self.key_page_kind().is_some()
    }

    /// The width of the row-selection gutter. A key page has no `#` column, so its cells start at
    /// the pane's left edge; SQL tables keep the 16px gutter.
    pub(super) fn gutter_width(&self) -> f32 {
        if self.is_key_page() {
            0.0
        } else {
            GRID_GUTTER_WIDTH
        }
    }

    /// A string/ReJSON key uses the page's text editor instead of the field/member table.
    pub(super) fn is_string_key_page(&self) -> bool {
        matches!(self.key_page_kind(), Some("string" | "json"))
    }

    /// Whether the key's contents can be edited. Streams are append-only and ReJSON keys are
    /// edited through their module's own commands, so both are read-only here.
    pub(super) fn key_page_editable(&self) -> bool {
        matches!(
            self.key_page_kind(),
            Some("string" | "hash" | "set" | "list" | "zset")
        )
    }

    /// Give the key page's Field/Member and Value columns the width of the pane instead of the
    /// content-fitted SQL grid widths: the prototype's key table fills the page, with the value
    /// column taking the remaining room. Runs on every key-page render, so a window resize keeps
    /// the table stretched; the widths are mirrored into the grid state because hit-testing and
    /// the shared cell editor read them from there.
    pub(super) fn layout_key_page_columns(&mut self) {
        let pane = f32::from(self.grid_pane_bounds.borrow().size.width);
        let available = if pane > 240.0 { pane } else { 700.0 };
        let kind = self.key_page_kind().unwrap_or_default();
        self.state.column_widths = match (kind, self.state.columns.len()) {
            ("set", 1) => vec![available.max(200.0)],
            ("list", 2) => vec![90.0, (available - 90.0).max(220.0)],
            ("zset", 2) => {
                let member = (available * 0.55).clamp(220.0, 480.0);
                vec![member, (available - member).max(120.0)]
            }
            ("hash", 2) => {
                let field = (available * 0.35).clamp(180.0, 340.0);
                vec![field, (available - field).max(220.0)]
            }
            ("stream", 3) => {
                let id = (available * 0.30).clamp(160.0, 300.0);
                let field = (available * 0.22).clamp(120.0, 240.0);
                vec![id, field, (available - id - field).max(220.0)]
            }
            _ => return,
        };
    }

    /// The localized header of one key-page column (the driver calls them `field`, `value` and
    /// `member`; the page shows the prototype's Field / Value / Member labels).
    pub(super) fn key_column_label(&self, name: &str) -> String {
        match name {
            "field" => t!("redis.key.field").to_string(),
            "value" => t!("redis.key.value").to_string(),
            "member" => t!("redis.key.member").to_string(),
            "index" => t!("redis.key.index").to_string(),
            "score" => t!("redis.key.score").to_string(),
            "id" => t!("redis.key.id").to_string(),
            other => other.to_string(),
        }
    }

    /// The key page's header bar: type badge, key name, TTL, memory, and the collection actions.
    pub(super) fn render_key_page_bar(
        &self,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let Some(status) = self.state.key_status.as_ref() else {
            return div().into_any_element();
        };
        let kind = status.engine.clone().unwrap_or_default();
        let add_label = match kind.as_str() {
            "hash" => Some(t!("redis.key.add_field").to_string()),
            "set" => Some(t!("redis.key.add_member").to_string()),
            "list" => Some(t!("redis.key.add_element").to_string()),
            "zset" => Some(t!("redis.key.add_member").to_string()),
            _ => None,
        };
        let ttl = match status.ttl {
            Some(-1) => t!("redis.key.persist").to_string(),
            Some(ttl) => format!("{}: {ttl}s", t!("redis.key.ttl_label")),
            None => String::new(),
        };
        let memory = status.data_length.map(human_size).unwrap_or_default();
        let editable = self.key_page_editable() && self.state.editable;
        let collection = add_label.is_some();
        let delete_enabled =
            editable && collection && self.state.selection.is_some() && !self.state.rows.is_empty();
        // The editor is the source of truth for a string key: a keystroke reflected there but not
        // yet in the mirrored `key_string_dirty` must still enable 保存修改.
        let string_dirty = self.is_string_key_page()
            && self.key_string_editor.as_ref().is_some_and(|editor| {
                let live = editor.read(cx).live_text(cx);
                self.key_string_loaded.as_deref() != Some(live.as_str())
            });
        let save_enabled = editable
            && (string_dirty
                || self.key_string_dirty
                || self.cell_editor.is_some()
                || !self.state.edits.is_empty()
                || !self.inserts.is_empty());

        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(38.0))
            .flex_none()
            .px_3()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .text_size(px(12.0))
            .child(super::objects::key_type_badge(&kind, theme))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme.text))
                    .child(self.state.table.clone()),
            );
        if !kind.is_empty() {
            bar = bar.child(
                div()
                    .text_color(rgb(theme.text_muted))
                    .child(kind.to_uppercase()),
            );
        }
        if !ttl.is_empty() {
            bar = bar.child(div().text_color(rgb(theme.text_muted)).child(ttl));
        }
        if !memory.is_empty() {
            bar = bar.child(
                div()
                    .text_color(rgb(theme.text_muted))
                    .child(format!("{}: {memory}", t!("redis.key.memory"))),
            );
        }
        if !editable {
            bar = bar.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("redis.key.read_only").to_string()),
            );
        }
        bar = bar.child(div().flex_1());
        if let Some(add_label) = add_label {
            bar = bar
                .child(ui::toolbar_item(
                    "key-page-add",
                    "icons/add_field.svg",
                    add_label,
                    editable,
                    theme,
                    cx.listener(|this, _event, window, cx| this.add_insert_row(window, cx)),
                ))
                .child(ui::toolbar_item(
                    "key-page-delete",
                    "icons/delete_field.svg",
                    t!("redis.key.delete_selected").to_string(),
                    delete_enabled,
                    theme,
                    cx.listener(|this, _event, _window, cx| this.open_delete_confirm(cx)),
                ));
        }
        if editable {
            bar = bar.child(ui::popup_button(
                "key-page-save",
                t!("redis.key.save_changes").to_string(),
                save_enabled,
                theme,
                cx.listener(|this, _event, _window, cx| {
                    this.save_current(cx);
                }),
            ));
        }
        bar = bar
            .child(ui::toolbar_item(
                "key-page-refresh",
                "icons/refresh.svg",
                t!("common.refresh").to_string(),
                true,
                theme,
                cx.listener(|this, _event, _window, cx| this.refresh(cx)),
            ))
            .when(self.state.loading || self.key_string_saving, |bar| {
                bar.child(self.render_loading_spinner())
            });
        bar.into_any_element()
    }

    /// A compact pager under the key table (the full record toolbar belongs to SQL tables and is
    /// not drawn on a key page).
    pub(super) fn render_key_page_footer(
        &self,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let total = self
            .state
            .total_rows
            .unwrap_or(self.state.rows.len() as u64);
        let page = self.state.page_index + 1;
        let last = self.state.last_page().map(|last| last + 1);
        let at_first = self.state.page_index == 0;
        let at_last = match self.state.last_page() {
            Some(last) => self.state.page_index >= last,
            None => !self.state.has_next(),
        };
        let pager_button = |id: &'static str, icon: &'static str, enabled: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(px(20.0))
                .h(px(18.0))
                .flex_none()
                .rounded_sm()
                .when(enabled, |button| {
                    button
                        .cursor_pointer()
                        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                })
                .opacity(if enabled { 1.0 } else { 0.35 })
                .child(
                    svg()
                        .path(icon)
                        .w(px(10.0))
                        .h(px(10.0))
                        .flex_none()
                        .text_color(rgb(theme.text_muted)),
                )
        };

        let mut prev = pager_button("key-page-prev", "icons/prev.svg", !at_first);
        if !at_first {
            prev = prev.on_click(cx.listener(|this, _event, _window, cx| this.prev_page(cx)));
        }
        let mut next = pager_button("key-page-next", "icons/next.svg", !at_last);
        if !at_last {
            next = next.on_click(cx.listener(|this, _event, _window, cx| this.next_page(cx)));
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(24.0))
            .flex_none()
            .px_3()
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .text_size(px(11.5))
            .text_color(rgb(theme.text_muted))
            .child(format!("{total} {}", t!("redis.key.entries")))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(prev)
                    .child(match last {
                        Some(last) => format!("{page} / {last}"),
                        None => page.to_string(),
                    })
                    .child(next),
            )
            .into_any_element()
    }

    /// The dedicated page for a `string` / ReJSON key: the prototype's RAW / JSON switch above a
    /// large editor. RAW is editable for a string key; JSON is a read-only pretty-printed view
    /// (invalid JSON falls back to the raw text with a warning). ReJSON keys stay read-only.
    pub(super) fn render_string_key_page(&mut self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let editable = self.key_page_editable() && self.state.editable;
        self.sync_key_string_editor(editable, cx);
        let raw = self.key_string_text.clone();
        let pretty = serde_json::from_str::<serde_json::Value>(&relax_json(&raw))
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok());
        let json_mode = self.key_string_format == KeyStringFormat::Json && pretty.is_some();

        let mut page = div()
            .flex()
            .flex_col()
            .size_full()
            // Ctrl/⌘+S must reach the key page even while the RAW editor owns focus.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let keystroke = &event.keystroke;
                if (keystroke.modifiers.control || keystroke.modifiers.platform)
                    && keystroke.key.eq_ignore_ascii_case("s")
                    && this.key_page_kind() == Some("string")
                {
                    cx.stop_propagation();
                    this.save_string_key(cx);
                }
            }));
        page = page.child(self.render_key_page_bar(theme, cx));

        let format_button = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .px_3()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .text_size(px(12.0))
                .when(active, move |button| {
                    button
                        .bg(rgb(theme.input_bg))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme.text))
                })
                .when(!active, move |button| {
                    button
                        .text_color(rgb(theme.text_muted))
                        .hover(move |style| style.text_color(rgb(theme.text)))
                })
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.key_string_format = if label == "RAW" {
                        KeyStringFormat::Raw
                    } else {
                        KeyStringFormat::Json
                    };
                    cx.notify();
                }))
                .child(label.to_string())
        };
        let toggle = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(34.0))
            .flex_none()
            .px_3()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("redis.key.format").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_0p5()
                    .p_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.toolbar_bg))
                    .child(format_button(
                        "key-string-raw",
                        "RAW",
                        self.key_string_format == KeyStringFormat::Raw,
                    ))
                    .child(format_button(
                        "key-string-json",
                        "JSON",
                        self.key_string_format == KeyStringFormat::Json,
                    )),
            );
        page = page.child(toggle);

        let mut body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .px_3()
            .pb_3();
        if self.key_string_format == KeyStringFormat::Json && pretty.is_none() {
            body = body.child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(theme.warning))
                    .pb_1()
                    .child(t!("redis.key.json_invalid").to_string()),
            );
        }
        let editor = if json_mode {
            let pretty = pretty.unwrap_or_default();
            if self
                .key_json_editor
                .as_ref()
                .is_none_or(|editor| editor.read(cx).text() != pretty.as_str())
            {
                if self.key_json_editor.is_none() {
                    let options = ui::SqlEditorOptions {
                        // The JSON grammar is compiled into gpui-kit (tree-sitter-json ships with
                        // the SQL grammar), so the read-only view gets real syntax colours
                        // instead of a plain text run.
                        language: "json".into(),
                        font_size: 13.0,
                        line_number: false,
                        readonly: true,
                        ..Default::default()
                    };
                    self.key_json_editor =
                        Some(cx.new(|cx| ui::SqlEditor::new(cx).options(options)));
                }
                if let Some(editor) = self.key_json_editor.as_ref() {
                    editor.update(cx, |editor, cx| editor.set_text(pretty.clone(), cx));
                }
            }
            self.key_json_editor.clone()
        } else {
            self.ensure_key_string_editor(editable, cx);
            self.key_string_editor.clone()
        };
        if let Some(editor) = editor {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .rounded_md()
                    .overflow_hidden()
                    .child(editor),
            );
        }
        if let Some(error) = self.key_string_error.clone() {
            body = body.child(
                div()
                    .flex_none()
                    .px_2()
                    .py_1()
                    .text_size(px(11.5))
                    .text_color(rgb(theme.danger))
                    .child(error),
            );
        }
        page.child(body).into_any_element()
    }

    /// The raw value of the string key currently loaded in the grid's one row.
    fn key_string_value(&self) -> String {
        self.state
            .rows
            .first()
            .and_then(|row| row.first())
            .map(|cell| cell.as_edit_string())
            .unwrap_or_default()
    }

    /// Build the RAW editor on first use. Its change callback mirrors the text into the grid so
    /// the page's 保存修改 and dirty state work without reading the editor entity back.
    fn ensure_key_string_editor(
        &mut self,
        editable: bool,
        cx: &mut Context<'_, Self>,
    ) -> Entity<ui::SqlEditor> {
        if let Some(editor) = self.key_string_editor.as_ref() {
            return editor.clone();
        }
        let weak = self.self_weak.clone();
        let options = ui::SqlEditorOptions {
            language: "text".into(),
            font_size: 13.0,
            line_number: false,
            readonly: !editable,
            ..Default::default()
        };
        let editor = cx.new(|cx| {
            ui::SqlEditor::new(cx)
                .options(options)
                .on_change(Rc::new(move |text, cx| {
                    let _ = weak.update(cx, |grid, cx| {
                        grid.key_string_text = text.to_string();
                        grid.key_string_dirty = true;
                        grid.key_string_error = None;
                        cx.notify();
                    });
                }))
        });
        self.key_string_editor = Some(editor.clone());
        editor
    }

    /// Keep the RAW editor in step with the fetched value: a clean editor follows a refresh, while
    /// an editor with unsaved edits is left alone.
    fn sync_key_string_editor(&mut self, editable: bool, cx: &mut Context<'_, Self>) {
        let raw = self.key_string_value();
        if !self.key_string_dirty && self.key_string_loaded.as_deref() != Some(raw.as_str()) {
            let editor = self.ensure_key_string_editor(editable, cx);
            editor.update(cx, |editor, cx| editor.set_text(raw.clone(), cx));
            self.key_string_text = raw.clone();
            self.key_string_loaded = Some(raw);
        } else if self.key_string_editor.is_none() {
            let _ = self.ensure_key_string_editor(editable, cx);
        }
    }

    /// Save whatever the key page currently shows: the RAW text for a string key, or the
    /// field/member edits for a collection.
    pub(super) fn save_current(&mut self, cx: &mut Context<'_, Self>) {
        if self.is_string_key_page() {
            self.save_string_key(cx);
        } else {
            self.save_grid(cx);
        }
    }

    /// Save the string page's RAW text through the driver's string write path, then reload the key.
    pub(super) fn save_string_key(&mut self, cx: &mut Context<'_, Self>) {
        if self.key_page_kind() != Some("string") || !self.state.editable || self.key_string_saving
        {
            return;
        }
        // Read the editor's live text rather than the mirror: a click on 保存修改 can land before
        // the last change callback reaches the page, and that made the button look inert.
        let text = self
            .key_string_editor
            .as_ref()
            .map(|editor| editor.read(cx).live_text(cx))
            .unwrap_or_else(|| self.key_string_text.clone());
        self.key_string_text = text.clone();
        let connection = self.state.connection.clone();
        let database = self.state.database.clone();
        let key = self.state.table.clone();
        self.key_string_saving = true;
        self.key_string_error = None;
        cx.notify();
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn(async move {
                    connection
                        .update_rows(
                            &database,
                            &key,
                            &[RowUpdate {
                                keys: Vec::new(),
                                set: vec![("value".to_string(), Some(text))],
                            }],
                        )
                        .await
                })
                .await;
            let _ = this.update(cx, |grid, cx| {
                grid.key_string_saving = false;
                match result {
                    Ok(Ok(())) => {
                        grid.key_string_dirty = false;
                        grid.key_string_loaded = None;
                        grid.show_toast(ToastKind::Success, t!("redis.key.saved").to_string(), cx);
                        grid.reload_after_write(cx);
                    }
                    Ok(Err(error)) => grid.key_string_error = Some(error.to_string()),
                    Err(error) => grid.key_string_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// Make a Redis string parseable as JSON when it contains Java long literals.
///
/// Java serializers often emit `"dictCode": 103L` / `"dictSort": 2L`; strict JSON rejects the
/// trailing `L`, which made the JSON view fall back to RAW even though every JSON formatter the
/// user compared against accepted it. Strip an `L`/`l` that directly follows a digit **outside a
/// string literal**; quoted text such as `"code": "103L"` is left untouched.
fn relax_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escape = false;
    let mut previous: Option<char> = None;
    for ch in text.chars() {
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            out.push(ch);
            previous = Some(ch);
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            previous = Some(ch);
            continue;
        }
        if (ch == 'L' || ch == 'l') && previous.is_some_and(|last| last.is_ascii_digit()) {
            continue;
        }
        out.push(ch);
        previous = Some(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::relax_json;

    #[test]
    fn relaxes_java_long_suffixes_outside_string_literals() {
        assert_eq!(
            relax_json(r#"{"dictCode":103L,"dictSort":2l}"#),
            r#"{"dictCode":103,"dictSort":2}"#
        );
        assert_eq!(
            relax_json(r#"{"code":"103L","literal":"a\\\"bL"}"#),
            r#"{"code":"103L","literal":"a\\\"bL"}"#
        );
    }
}
