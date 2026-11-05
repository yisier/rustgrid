//! A compact multi-line text editor for the app's forms.
//!
//! It wraps gpui-kit's code [`Editor`], configured as a plain text area: no line numbers, no
//! folding, soft wrap on. The editor supplies caret, selection, clipboard, IME, undo/redo and
//! syntax highlighting (the row form points it at the `json` grammar for JSON values), while the
//! wrapper keeps the app's callback API (`on_change` / `on_blur` / `on_cancel`, `set_text`,
//! `set_placeholder`, ...) and creates the inner state lazily on the first render, because
//! gpui-kit's state types need a `Window`.

use std::rc::Rc;

use gpui::{App, Context, SharedString, Subscription, Window, div, prelude::*, px};
use gpui_kit::component::input::{Editor, EditorState, InputEvent};

use crate::theme::Theme;

/// Called with the current text after every edit.
pub(crate) type TextAreaChange = Rc<dyn Fn(&str, &mut Window, &mut App) + 'static>;
/// Called when the editor loses focus.
pub(crate) type TextAreaVoid = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

/// Construction options for [`TextArea`].
pub(crate) struct TextAreaOptions {
    /// `true` colours the value with the `json` grammar; `false` leaves it uncoloured.
    pub json: bool,
    /// Show a line-number gutter (JSON code blocks use it; plain long text does not).
    pub line_numbers: bool,
    pub placeholder: SharedString,
    pub text_size: f32,
}

impl Default for TextAreaOptions {
    fn default() -> Self {
        Self {
            json: false,
            line_numbers: false,
            placeholder: SharedString::default(),
            text_size: 12.0,
        }
    }
}

pub(crate) struct TextArea {
    initial_text: String,
    json: bool,
    line_numbers: bool,
    placeholder: SharedString,
    text_size: f32,
    /// The text queued for the next render, where a `Window` is available.
    pending_text: Option<String>,
    pending_placeholder: Option<SharedString>,
    /// A line-number toggle queued for the next render, where the state can take a window.
    pending_line_numbers: Option<bool>,
    /// The gpui-kit editor state, created on the first render.
    state: Option<gpui::Entity<EditorState>>,
    subscriptions: Vec<Subscription>,
    on_change: Option<TextAreaChange>,
    on_blur: Option<TextAreaVoid>,
    on_cancel: Option<TextAreaVoid>,
    /// A change reported by the editor, waiting for the next render to reach the callback.
    pending_change: Option<String>,
    /// A focus loss reported by the editor, waiting for the next render.
    pending_blur: bool,
    /// A focus request made before the inner state existed.
    focus_pending: bool,
    _theme: Theme,
}

impl TextArea {
    pub(crate) fn new(
        theme: Theme,
        text: impl Into<String>,
        options: TextAreaOptions,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            initial_text: text.into(),
            json: options.json,
            line_numbers: options.line_numbers,
            placeholder: options.placeholder,
            text_size: options.text_size,
            pending_text: None,
            pending_placeholder: None,
            pending_line_numbers: None,
            state: None,
            subscriptions: Vec::new(),
            on_change: None,
            on_blur: None,
            on_cancel: None,
            pending_change: None,
            pending_blur: false,
            focus_pending: false,
            _theme: theme,
        }
    }

    pub(crate) fn on_change(mut self, callback: TextAreaChange) -> Self {
        self.on_change = Some(callback);
        self
    }

    pub(crate) fn on_blur(mut self, callback: TextAreaVoid) -> Self {
        self.on_blur = Some(callback);
        self
    }

    pub(crate) fn on_cancel(mut self, callback: TextAreaVoid) -> Self {
        self.on_cancel = Some(callback);
        self
    }

    /// The editor's text: the pending programmatic value before the first render, the live state
    /// afterwards.
    pub(crate) fn current_text(&self, cx: &App) -> Option<String> {
        if let Some(pending) = &self.pending_text {
            return Some(pending.clone());
        }
        Some(match self.state.as_ref() {
            Some(state) => state.read(cx).value().to_string(),
            None => self.initial_text.clone(),
        })
    }

    /// Replace the whole document. Applied on the next render, where the state can take a window.
    pub(crate) fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.pending_text = Some(text.into());
        cx.notify();
    }

    pub(crate) fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.pending_placeholder = Some(placeholder.into());
        cx.notify();
    }

    /// Show or hide the line-number gutter. The JSON code block shows it only while the value is
    /// formatted, where the lines are the point of the view.
    pub(crate) fn set_line_numbers(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.line_numbers == enabled {
            return;
        }
        self.line_numbers = enabled;
        self.pending_line_numbers = Some(enabled);
        cx.notify();
    }

    pub(crate) fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.state.as_ref().is_some_and(|state| {
            state
                .read(cx)
                .presentation()
                .focus_handle()
                .is_focused(window)
        })
    }

    pub(crate) fn focus_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = self.state.clone() {
            state.update(cx, |state, cx| state.focus(window, cx));
        } else {
            self.focus_pending = true;
            cx.notify();
        }
    }

    fn ensure_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_some() {
            return;
        }
        let language: SharedString = if self.json { "json" } else { "text" }.into();
        let text = self
            .pending_text
            .take()
            .unwrap_or_else(|| self.initial_text.clone());
        let placeholder = self.placeholder.clone();
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language(language)
                .line_number(self.line_numbers)
                .folding(false);
            state.set_soft_wrap(true, window, cx);
            state.set_placeholder(placeholder, window, cx);
            state.set_value(text, window, cx);
            state
        });
        self.subscriptions.push(cx.subscribe(
            &state,
            |area: &mut Self, _state, event: &InputEvent, cx| match event {
                InputEvent::Change => {
                    let text = area
                        .state
                        .as_ref()
                        .map(|state| state.read(cx).value().to_string())
                        .unwrap_or_default();
                    area.pending_change = Some(text);
                    cx.notify();
                }
                InputEvent::Blur => {
                    area.pending_blur = true;
                    cx.notify();
                }
                _ => {}
            },
        ));
        if std::mem::take(&mut self.focus_pending) {
            state.update(cx, |state, cx| state.focus(window, cx));
        }
        self.state = Some(state);
    }

    fn flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_state(window, cx);
        let Some(state) = self.state.clone() else {
            return;
        };
        if let Some(text) = self.pending_text.take() {
            if state.read(cx).value().as_ref() != text {
                state.update(cx, |state, cx| state.set_value(text, window, cx));
            }
            cx.notify();
        }
        if let Some(placeholder) = self.pending_placeholder.take() {
            state.update(cx, |state, cx| {
                state.set_placeholder(placeholder, window, cx)
            });
        }
        if let Some(enabled) = self.pending_line_numbers.take() {
            state.update(cx, |state, cx| state.set_line_number(enabled, window, cx));
        }
        if let Some(text) = self.pending_change.take()
            && let Some(callback) = self.on_change.clone()
        {
            callback(&text, window, cx);
        }
        if std::mem::take(&mut self.pending_blur)
            && let Some(callback) = self.on_blur.clone()
        {
            callback(window, cx);
        }
    }

    fn handle_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape"
            && let Some(callback) = self.on_cancel.clone()
        {
            callback(window, cx);
        }
    }
}

impl Render for TextArea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.flush(window, cx);
        let Some(state) = self.state.clone() else {
            return div().into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .text_size(px(self.text_size))
            .on_key_down(cx.listener(Self::handle_key_down))
            .child(
                Editor::new(&state)
                    .appearance(false)
                    .bordered(false)
                    .text_size(px(self.text_size))
                    .flex_1()
                    .min_h(px(0.0)),
            )
            .into_any_element()
    }
}
