//! The grid's value editor (F2), as in Navicat: a panel under the grid that shows the selected
//! cell's whole value in a multi-line editor, for reading or editing long text.
//!
//! The panel takes the focus. Ctrl/⌘+Enter saves through the in-place editor's own commit path
//! (so undo, pending inserts and the unsaved marker behave exactly as for an inline edit) and Esc
//! closes it; on a read-only grid it is a viewer.

use super::*;

/// The value editor's height, in px.
const VALUE_EDITOR_HEIGHT: f32 = 180.0;

/// An open value editor.
pub(super) struct ValueEditor {
    row: usize,
    col: usize,
    column: String,
    /// The value it was loaded with, to tell whether the panel's text was changed.
    original: String,
    editor: Entity<ui::SqlEditor>,
    /// Focus the editor on the next frame (it is built on its first render).
    focus_pending: bool,
}

impl GridView {
    /// F2: open the value editor on the cursor cell (closing any in-place editor first).
    pub(super) fn open_value_editor(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        // F2 again closes it.
        if self.value_editor.is_some() {
            self.save_value_editor(window, cx);
            return;
        }
        let Some((row, col)) = self
            .state
            .selection
            .as_ref()
            .map(|selection| selection.active_cursor())
        else {
            return;
        };
        if row >= self.display_row_count() || col >= self.state.visible_columns {
            return;
        }
        if self.cell_editor.is_some() {
            self.commit_editor(cx);
        }
        let (value, column) = self.cell_value_and_column(row, col);
        let options = ui::SqlEditorOptions {
            language: "text".into(),
            line_number: false,
            readonly: !self.state.editable,
            ..Default::default()
        };
        let editor = cx.new(|cx| ui::SqlEditor::new(cx).options(options));
        editor.update(cx, |editor, cx| {
            editor.set_text(value.clone(), cx);
            // Focus it as soon as it exists: the very next key belongs to the value editor.
            editor.focus_when_ready(window, cx);
        });
        self.state.selection = Some(CellSelection::new(row, col));
        self.value_editor = Some(ValueEditor {
            row,
            col,
            column,
            original: value,
            editor,
            focus_pending: false,
        });
        cx.notify();
    }

    /// A cell's current text (a staged edit, else the loaded value) and its column's name.
    fn cell_value_and_column(&self, row: usize, col: usize) -> (String, String) {
        let value = self
            .staged_value(row, col)
            .cloned()
            .flatten()
            .unwrap_or_else(|| match self.row_at(row) {
                Some(GridRow::Data(data)) => self.state.rows[data]
                    .get(self.state.source_col(col))
                    .map(|cell| cell.as_edit_string())
                    .unwrap_or_default(),
                _ => String::new(),
            });
        let column = self
            .state
            .columns
            .get(col)
            .map(|column| column.name.clone())
            .unwrap_or_default();
        (value, column)
    }

    /// Write `text` into `(row, col)` through the in-place editor's commit path (undo, pending
    /// inserts and auto-commit behave as for an inline edit), keeping the current selection.
    fn store_cell_text(
        &mut self,
        (row, col): (usize, usize),
        text: String,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.state.editable {
            return;
        }
        let selection = self.state.selection.take();
        self.state.selection = Some(CellSelection::new(row, col));
        self.begin_edit((row, col), None, window, cx);
        if let Some(cell_editor) = self.cell_editor.as_mut() {
            cell_editor.value = text;
        }
        self.commit_editor(cx);
        self.state.selection = selection;
    }

    /// While the panel is open, follow the grid's cursor: when it moved to another cell, save a
    /// changed value into the old cell first, then show the new cell's value.
    fn follow_cursor(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(cursor) = self
            .state
            .selection
            .as_ref()
            .map(|selection| selection.active_cursor())
        else {
            return;
        };
        let Some(value_editor) = self.value_editor.as_ref() else {
            return;
        };
        let old = (value_editor.row, value_editor.col);
        if cursor == old
            || cursor.0 >= self.display_row_count()
            || cursor.1 >= self.state.visible_columns
        {
            return;
        }
        let text = value_editor.editor.read(cx).text().to_string();
        if text != value_editor.original {
            self.store_cell_text(old, text, window, cx);
        }
        let (value, column) = self.cell_value_and_column(cursor.0, cursor.1);
        if let Some(value_editor) = self.value_editor.as_mut() {
            value_editor.row = cursor.0;
            value_editor.col = cursor.1;
            value_editor.column = column;
            value_editor.original = value.clone();
            value_editor
                .editor
                .update(cx, |editor, cx| editor.set_text(value, cx));
        }
    }

    /// Save the value editor's text into its cell (when the grid is editable) and close it.
    fn save_value_editor(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(value_editor) = self.value_editor.take() else {
            return;
        };
        let text = value_editor.editor.read(cx).text().to_string();
        if text != value_editor.original {
            self.store_cell_text((value_editor.row, value_editor.col), text, window, cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Close the value editor without saving.
    fn close_value_editor(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.value_editor.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// The panel under the grid, when the value editor is open.
    pub(super) fn render_value_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        self.follow_cursor(window, cx);
        let theme = self.theme;
        let editable = self.state.editable;
        let value_editor = self.value_editor.as_mut()?;
        if value_editor.focus_pending
            && value_editor
                .editor
                .update(cx, |editor, cx| editor.focus(window, cx))
        {
            value_editor.focus_pending = false;
        } else if value_editor.focus_pending {
            cx.notify();
        }
        let hint = if editable {
            t!("grid.value_editor.hint_edit").to_string()
        } else {
            t!("grid.value_editor.hint_read").to_string()
        };
        Some(
            div()
                .id("grid-value-editor")
                .flex()
                .flex_col()
                .flex_none()
                .w_full()
                .h(px(VALUE_EDITOR_HEIGHT))
                .border_t_1()
                .border_color(rgb(theme.border))
                .bg(rgb(theme.input_bg))
                // Ctrl/⌘+Enter saves and Esc closes; captured before the editor sees them.
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    let keystroke = &event.keystroke;
                    let secondary = keystroke.modifiers.control || keystroke.modifiers.platform;
                    if keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.close_value_editor(window, cx);
                    } else if (keystroke.key == "f2" && !secondary)
                        || (keystroke.key == "enter" && secondary)
                    {
                        cx.stop_propagation();
                        this.save_value_editor(window, cx);
                    }
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .px_2()
                        .h(px(24.0))
                        .text_size(px(11.5))
                        .text_color(rgb(theme.text_muted))
                        .child(value_editor.column.clone())
                        .child(hint),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(value_editor.editor.clone()),
                )
                .into_any_element(),
        )
    }
}
