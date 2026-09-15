//! The single-line text input used across the app.
//!
//! It is an `Entity` view so the platform IME can talk to it through
//! [`EntityInputHandler`], which is what makes Chinese (and other composed) input work.
//! Text insertion is driven entirely by the platform input handler (`WM_CHAR` / IME); the
//! key handler only owns navigation and editing commands, so the two never double-insert.
//!
//! Callers size the returned element and may react to edits through
//! [`TextInput::on_change`], [`TextInput::on_submit`], [`TextInput::on_cancel`] and
//! [`TextInput::on_tab`]. The chrome (square border, `input_bg`, accent focus border) keeps the
//! classic Win32/Navicat look; the caret is a painted quad, never a `"|"` glyph.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, InspectorElementId, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, div, fill,
    point, prelude::*, px, relative, rgb, size, svg,
};

use crate::theme::Theme;

/// Called with the current text after every edit.
pub(crate) type TextChangeCallback = Rc<dyn Fn(&str, &mut Window, &mut App) + 'static>;
/// Called when the user presses Enter.
pub(crate) type TextVoidCallback = Rc<dyn Fn(&mut Window, &mut App) + 'static>;
/// Called when the user presses Tab (`true` for Shift+Tab).
pub(crate) type TextTabCallback = Rc<dyn Fn(bool, &mut Window, &mut App) + 'static>;

/// Construction options for [`TextInput`].
#[derive(Default)]
pub(crate) struct TextInputOptions {
    pub masked: bool,
    pub placeholder: SharedString,
    /// Rejects characters the input should not accept (e.g. digits-only fields). `None` accepts
    /// everything, including composed CJK input.
    pub accepts: Option<Rc<dyn Fn(char) -> bool + 'static>>,
    /// An optional leading icon (the search box uses this).
    pub icon: Option<&'static str>,
    /// Shows a clear (`✕`) button while the field has text.
    pub clearable: bool,
}

pub(crate) struct TextInput {
    focus: FocusHandle,
    text: String,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    masked: bool,
    accepts: Option<Rc<dyn Fn(char) -> bool + 'static>>,
    icon: Option<&'static str>,
    clearable: bool,
    theme: Theme,
    focused: bool,
    caret_visible: bool,
    blink_running: bool,
    selecting: bool,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    on_change: Option<TextChangeCallback>,
    on_submit: Option<TextVoidCallback>,
    on_cancel: Option<TextVoidCallback>,
    on_tab: Option<TextTabCallback>,
}

impl TextInput {
    pub(crate) fn new(
        theme: Theme,
        text: impl Into<String>,
        options: TextInputOptions,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = text.into();
        let end = text.len();
        Self {
            focus: cx.focus_handle(),
            text,
            placeholder: options.placeholder,
            selected_range: end..end,
            selection_reversed: false,
            marked_range: None,
            masked: options.masked,
            accepts: options.accepts,
            icon: options.icon,
            clearable: options.clearable,
            theme,
            focused: false,
            caret_visible: true,
            blink_running: false,
            selecting: false,
            last_layout: None,
            last_bounds: None,
            on_change: None,
            on_submit: None,
            on_cancel: None,
            on_tab: None,
        }
    }

    pub(crate) fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        let end = self.text.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub(crate) fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        cx.notify();
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn on_change(mut self, callback: TextChangeCallback) -> Self {
        self.on_change = Some(callback);
        self
    }

    pub(crate) fn on_submit(mut self, callback: TextVoidCallback) -> Self {
        self.on_submit = Some(callback);
        self
    }

    pub(crate) fn on_cancel(mut self, callback: TextVoidCallback) -> Self {
        self.on_cancel = Some(callback);
        self
    }

    pub(crate) fn on_tab(mut self, callback: TextTabCallback) -> Self {
        self.on_tab = Some(callback);
        self
    }

    fn masked_display(&self) -> SharedString {
        if self.masked {
            "*".repeat(self.text.chars().count()).into()
        } else {
            self.text.clone().into()
        }
    }

    /// Maps a byte offset in the real text to a byte offset in the rendered (possibly masked)
    /// text.
    fn display_offset(&self, offset: usize) -> usize {
        let offset = offset.min(self.text.len());
        if self.masked {
            self.text[..offset].chars().count()
        } else {
            offset
        }
    }

    /// Maps a byte offset in the rendered text back to a byte offset in the real text.
    fn text_offset(&self, display: usize) -> usize {
        if self.masked {
            self.text
                .char_indices()
                .nth(display)
                .map(|(index, _)| index)
                .unwrap_or(self.text.len())
        } else {
            let mut offset = display.min(self.text.len());
            while offset > 0 && !self.text.is_char_boundary(offset) {
                offset -= 1;
            }
            offset
        }
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        if offset == 0 {
            return 0;
        }
        let mut index = offset.min(self.text.len()) - 1;
        while index > 0 && !self.text.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    fn next_boundary(&self, offset: usize) -> usize {
        if offset >= self.text.len() {
            return self.text.len();
        }
        let mut index = offset + 1;
        while index < self.text.len() && !self.text.is_char_boundary(index) {
            index += 1;
        }
        index
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.caret_visible = true;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.caret_visible = true;
        cx.notify();
    }

    fn selected_text(&self) -> String {
        let (start, end) = self.selection_range();
        self.text[start..end].to_string()
    }

    fn selection_range(&self) -> (usize, usize) {
        (self.selected_range.start, self.selected_range.end)
    }

    fn insert(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text: String = match &self.accepts {
            Some(accepts) => text
                .chars()
                .filter(|character| accepts(*character))
                .collect(),
            None => text.to_string(),
        };
        if text.is_empty() {
            return;
        }
        let range = self
            .marked_range
            .clone()
            .unwrap_or_else(|| self.selected_range.clone());
        self.text = format!(
            "{}{}{}",
            &self.text[..range.start],
            text,
            &self.text[range.end..]
        );
        let caret = range.start + text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        self.after_edit(window, cx);
    }

    fn delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        let range = self.selected_range.clone();
        self.text = format!("{}{}", &self.text[..range.start], &self.text[range.end..]);
        self.selected_range = range.start..range.start;
        self.selection_reversed = false;
        self.marked_range = None;
        self.after_edit(window, cx);
    }

    fn backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            let boundary = self.previous_boundary(cursor);
            self.selected_range = boundary..cursor;
        }
        self.delete_selection(window, cx);
    }

    fn forward_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            let boundary = self.next_boundary(cursor);
            self.selected_range = cursor..boundary;
        }
        self.delete_selection(window, cx);
    }

    fn after_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.caret_visible = true;
        if let Some(callback) = self.on_change.clone() {
            let text = self.text.clone();
            callback(&text, window, cx);
        }
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.control || keystroke.modifiers.platform;
        let shift = keystroke.modifiers.shift;

        if command {
            match keystroke.key.as_str() {
                "a" => {
                    self.selected_range = 0..self.text.len();
                    self.selection_reversed = false;
                    self.caret_visible = true;
                    cx.notify();
                }
                "c" => self.copy(cx),
                "x" => self.cut(window, cx),
                "v" => self.paste(window, cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }

        match keystroke.key.as_str() {
            "backspace" => self.backspace(window, cx),
            "delete" => self.forward_delete(window, cx),
            "left" => {
                if shift {
                    self.select_to(self.previous_boundary(self.cursor_offset()), cx);
                } else if self.selected_range.is_empty() {
                    let target = self.previous_boundary(self.cursor_offset());
                    self.move_to(target, cx);
                } else {
                    self.move_to(self.selected_range.start, cx);
                }
            }
            "right" => {
                if shift {
                    self.select_to(self.next_boundary(self.cursor_offset()), cx);
                } else if self.selected_range.is_empty() {
                    let target = self.next_boundary(self.cursor_offset());
                    self.move_to(target, cx);
                } else {
                    self.move_to(self.selected_range.end, cx);
                }
            }
            "home" => self.move_to(0, cx),
            "end" => self.move_to(self.text.len(), cx),
            "enter" => {
                if let Some(callback) = self.on_submit.clone() {
                    callback(window, cx);
                }
            }
            "escape" => {
                if let Some(callback) = self.on_cancel.clone() {
                    callback(window, cx);
                }
            }
            "tab" => {
                if let Some(callback) = self.on_tab.clone() {
                    callback(shift, window, cx);
                }
            }
            _ => {
                // Text characters are inserted by the platform input handler (WM_CHAR / IME),
                // so let the event propagate.
                return;
            }
        }
        cx.stop_propagation();
    }

    fn copy(&self, cx: &mut Context<Self>) {
        let selected = self.selected_text();
        if !selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(selected));
        }
    }

    fn cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected_text();
        if !selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(selected));
            self.delete_selection(window, cx);
        }
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = text.replace(['\n', '\r'], " ");
            self.insert(&text, window, cx);
        }
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.text.is_empty() {
            return;
        }
        self.text.clear();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.after_edit(window, cx);
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return self.text.len();
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.text.len();
        }
        self.text_offset(line.closest_index_for_x(position.x - bounds.left()))
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus);
        self.selecting = true;
        let index = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx);
        }
    }

    fn on_mouse_up(&mut self, _event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let index = self.index_for_mouse_position(event.position);
        self.select_to(index, cx);
    }

    fn ensure_blink(&mut self, cx: &mut Context<Self>) {
        if !self.focused || self.blink_running {
            return;
        }
        self.blink_running = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(std::time::Duration::from_millis(530)).await;
                let keep_going = this.update(cx, |input, cx| {
                    if input.focused {
                        input.caret_visible = !input.caret_visible;
                        cx.notify();
                        true
                    } else {
                        input.blink_running = false;
                        input.caret_visible = true;
                        false
                    }
                });
                if !matches!(keep_going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for character in self.text.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += character.len_utf16();
            utf8 += character.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for character in self.text.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += character.len_utf8();
            utf16 += character.len_utf16();
        }
        utf16
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.selected_range = range;
        self.selection_reversed = false;
        self.insert(new_text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        let text: String = match &self.accepts {
            Some(accepts) => new_text
                .chars()
                .filter(|character| accepts(*character))
                .collect(),
            None => new_text.to_string(),
        };
        self.text = format!(
            "{}{}{}",
            &self.text[..range.start],
            text,
            &self.text[range.end..]
        );
        if text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + text.len());
        }
        let caret = range.start + text.len();
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|selected| {
                let from = self.offset_from_utf16(selected.start);
                let to = self.offset_from_utf16(selected.end);
                range.start + from..range.start + to
            })
            .unwrap_or(caret..caret);
        self.selection_reversed = false;
        self.caret_visible = true;
        if let Some(callback) = self.on_change.clone() {
            let text = self.text.clone();
            callback(&text, window, cx);
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start = self.display_offset(range.start);
        let end = self.display_offset(range.end);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(start), bounds.top()),
            point(bounds.left() + line.x_for_index(end), bounds.bottom()),
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

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.focused = self.focus.is_focused(window);
        self.ensure_blink(cx);
        let theme = self.theme;

        let mut field = div()
            .size_full()
            .relative()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .text_size(px(12.0))
            .line_height(px(16.0))
            .bg(rgb(theme.input_bg))
            .border_1()
            .border_color(rgb(if self.focused {
                theme.button_default_border
            } else {
                theme.border
            }))
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move));

        if let Some(icon) = self.icon {
            field = field.child(
                svg()
                    .path(icon)
                    .w(px(13.0))
                    .h(px(13.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );
        }

        field = field.child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .flex()
                .items_center()
                .child(TextElement { input: cx.entity() }),
        );

        if self.clearable && !self.text.is_empty() {
            field = field.child(
                div()
                    .id("text-input-clear")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_none()
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, window, cx| {
                            cx.stop_propagation();
                            this.clear(window, cx);
                        }),
                    )
                    .child(
                        svg()
                            .path("icons/cross.svg")
                            .w(px(10.0))
                            .h(px(10.0))
                            .flex_none()
                            .text_color(rgb(theme.text_muted)),
                    ),
            );
        }

        field
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let display = input.masked_display();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if display.is_empty() {
            (input.placeholder.clone(), style.color.opacity(0.4))
        } else {
            (display, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            let marked_start = input.display_offset(marked_range.start);
            let marked_end = input.display_offset(marked_range.end);
            vec![
                TextRun {
                    len: marked_start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_end.saturating_sub(marked_start),
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len().saturating_sub(marked_end),
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let selection_start = input.display_offset(selected_range.start);
        let selection_end = input.display_offset(selected_range.end);
        let cursor = input.display_offset(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + line.x_for_index(cursor), bounds.top()),
                        size(px(1.5), bounds.bottom() - bounds.top()),
                    ),
                    rgb(input.theme.text),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selection_start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selection_end),
                            bounds.bottom(),
                        ),
                    ),
                    rgb(input.theme.tree_selected_bg),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let Some(line) = prepaint.line.take() else {
            return;
        };
        line.paint(bounds.origin, window.line_height(), window, cx)
            .ok();

        if focus_handle.is_focused(window)
            && self.input.read(cx).caret_visible
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}
