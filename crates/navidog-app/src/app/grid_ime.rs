use std::ops::Range;

use super::*;

/// Converts a character index into a byte offset (clamped to the string length).
fn char_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

/// Converts a byte offset into a character index (clamped to the string length).
fn byte_to_char(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].chars().count()
}

impl GridView {
    /// The grid text field currently focused, if any.
    pub(super) fn active_text_field(&self, window: &Window) -> Option<GridTextField> {
        if self.cell_editor.is_some() && self.cell_editor_focus.is_focused(window) {
            return Some(GridTextField::CellEditor);
        }
        if self.page_input_focus.is_focused(window) {
            return Some(GridTextField::PageInput);
        }
        if self.page_size_focus.is_focused(window) {
            return Some(GridTextField::PageSize);
        }
        if self.sort_combo_focus.is_focused(window) {
            return Some(GridTextField::SortSearch);
        }
        for index in 0..self.filter_value_focus.len() {
            if self.filter_value_focus[index].is_focused(window) {
                return Some(GridTextField::Filter(index, 0));
            }
            if self.filter_value2_focus[index].is_focused(window) {
                return Some(GridTextField::Filter(index, 1));
            }
        }
        None
    }

    /// A full-size paint probe that hands the platform input handler to this grid so `WM_CHAR`
    /// and IME composition reach [`GridView::replace_text_in_range`]. Place it inside the field's
    /// relative container.
    pub(super) fn ime_probe(&self, focus: &FocusHandle) -> impl IntoElement {
        let focus = focus.clone();
        let weak = self.self_weak.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                if let Some(grid) = weak.upgrade() {
                    window.handle_input(&focus, ElementInputHandler::new(bounds, grid), cx);
                }
            },
        )
        .absolute()
        .inset_0()
    }

    fn field_text(&self, field: GridTextField) -> String {
        match field {
            GridTextField::CellEditor => self
                .cell_editor
                .as_ref()
                .map(|editor| editor.value.clone())
                .unwrap_or_default(),
            GridTextField::PageInput => self.page_input.clone(),
            GridTextField::PageSize => self.page_size_input.clone(),
            GridTextField::SortSearch => self.sort_combo_filter.clone(),
            GridTextField::Filter(index, slot) => self
                .state
                .filter_draft
                .get(index)
                .map(|condition| {
                    if slot == 0 {
                        condition.value.clone()
                    } else {
                        condition.value2.clone()
                    }
                })
                .unwrap_or_default(),
        }
    }

    fn set_field_text(&mut self, field: GridTextField, value: String) {
        match field {
            GridTextField::CellEditor => {
                if let Some(editor) = self.cell_editor.as_mut() {
                    editor.value = value;
                }
            }
            GridTextField::PageInput => self.page_input = value,
            GridTextField::PageSize => self.page_size_input = value,
            GridTextField::SortSearch => self.sort_combo_filter = value,
            GridTextField::Filter(index, slot) => {
                if let Some(condition) = self.state.filter_draft.get_mut(index) {
                    if slot == 0 {
                        condition.value = value;
                    } else {
                        condition.value2 = value;
                    }
                }
            }
        }
    }

    /// The byte selection `(start, end)` of `field`. The append-only fields always have their
    /// caret at the end.
    fn field_selection(&self, field: GridTextField) -> (usize, usize) {
        if field == GridTextField::CellEditor
            && let Some(editor) = self.cell_editor.as_ref()
        {
            let (start, end) = editor.selection.range();
            return (
                char_to_byte(&editor.value, start),
                char_to_byte(&editor.value, end),
            );
        }
        let len = self.field_text(field).len();
        (len, len)
    }

    fn set_field_caret(&mut self, field: GridTextField, byte: usize) {
        if field == GridTextField::CellEditor
            && let Some(editor) = self.cell_editor.as_mut()
        {
            let index = byte_to_char(&editor.value, byte);
            editor.selection = FieldSelection {
                anchor: index,
                cursor: index,
            };
        }
    }

    fn field_is_numeric(field: GridTextField) -> bool {
        matches!(field, GridTextField::PageInput | GridTextField::PageSize)
    }

    fn field_changed(&mut self, field: GridTextField, cx: &mut Context<'_, Self>) {
        match field {
            GridTextField::SortSearch => self.sort_combo_highlight = 0,
            GridTextField::Filter(index, slot) => self.filter_active = Some((index, slot)),
            _ => {}
        }
        self.caret_visible = true;
        cx.notify();
    }

    fn ime_replace(
        &mut self,
        field: GridTextField,
        range: Option<(usize, usize)>,
        new_text: &str,
        mark: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let current = self.field_text(field);
        let (mut start, mut end) = range.unwrap_or_else(|| self.field_selection(field));
        start = start.min(current.len());
        end = end.min(current.len()).max(start);
        if !current.is_char_boundary(start) || !current.is_char_boundary(end) {
            return;
        }

        let mut insert = new_text.to_string();
        if Self::field_is_numeric(field) {
            insert.retain(|character| character.is_ascii_digit());
        }

        if field == GridTextField::CellEditor
            && !mark
            && let Some(editor) = self.cell_editor.as_mut()
        {
            editor.history.push(editor.value.clone());
        }

        let mut next = String::with_capacity(current.len() + insert.len());
        next.push_str(&current[..start]);
        next.push_str(&insert);
        next.push_str(&current[end..]);
        self.set_field_text(field, next);

        let caret = start + insert.len();
        self.set_field_caret(field, caret);
        if mark && !insert.is_empty() {
            self.ime_marked = Some(start..caret);
        } else {
            self.ime_marked = None;
        }
        self.ime_field = Some(field);
        self.field_changed(field, cx);
    }
}

impl EntityInputHandler for GridView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let field = self.active_text_field(window)?;
        let text = self.field_text(field);
        let start = offset_from_utf16(&text, range_utf16.start);
        let end = offset_from_utf16(&text, range_utf16.end);
        actual_range.replace(offset_to_utf16(&text, start)..offset_to_utf16(&text, end));
        Some(text[start..end].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let field = self.active_text_field(window)?;
        let text = self.field_text(field);
        let (start, end) = self.field_selection(field);
        Some(UTF16Selection {
            range: offset_to_utf16(&text, start)..offset_to_utf16(&text, end),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let marked = self.ime_marked.clone()?;
        let field = self.ime_field?;
        if self.active_text_field(window) != Some(field) {
            return None;
        }
        let text = self.field_text(field);
        Some(offset_to_utf16(&text, marked.start)..offset_to_utf16(&text, marked.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.ime_marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.active_text_field(window).or(self.ime_field) else {
            return;
        };
        let text = self.field_text(field);
        let range = range_utf16.map(|range| {
            (
                offset_from_utf16(&text, range.start),
                offset_from_utf16(&text, range.end),
            )
        });
        self.ime_replace(field, range, new_text, false, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.active_text_field(window).or(self.ime_field) else {
            return;
        };
        let text = self.field_text(field);
        let range = range_utf16
            .map(|range| {
                (
                    offset_from_utf16(&text, range.start),
                    offset_from_utf16(&text, range.end),
                )
            })
            .or_else(|| {
                self.ime_marked
                    .clone()
                    .map(|marked| (marked.start, marked.end))
            });
        self.ime_replace(field, range, new_text, !new_text.is_empty(), cx);
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(
            bounds.origin,
            gpui::size(px(1.0), bounds.size.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
