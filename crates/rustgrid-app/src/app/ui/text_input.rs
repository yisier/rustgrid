//! The single-line text input used across the app.
//!
//! It is a thin wrapper over gpui-kit's shadcn [`Input`]: the visual chrome, caret, selection,
//! IME composition, undo/redo and clipboard handling all come from the component library. The
//! wrapper keeps the app's original public API (`TextInputOptions`, `on_change`, `on_submit`,
//! `on_cancel`, `on_tab`, `set_text`, ...) so call sites stay unchanged.
//!
//! gpui-kit's `InputState` must be constructed with a `Window`, but the app builds its inputs
//! while the view is constructed (no window yet). The inner state is therefore created lazily on
//! the first render, and any pending reads/writes are flushed there.

use std::rc::Rc;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyDownEvent, SharedString, Subscription, Window,
    div, prelude::*, px, rgb,
};
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputEvent, InputState, Position};
use gpui_kit::component::{FocusableExt, Sizable, Size};

use crate::theme::Theme;

/// Base text size for the app's text inputs. gpui-kit's `Size::Small` typography is larger than
/// the app's chrome (which inherits 12.5px), so every input is pinned to this unless a caller
/// overrides it via [`TextInputOptions::text_size`].
pub(crate) const DEFAULT_TEXT_SIZE: f32 = 12.5;

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
    /// Tint for `icon`; muted when unset.
    pub icon_color: Option<u32>,
    /// Shows a clear (`✕`) button while the field has text.
    pub clearable: bool,
    /// Renders the field frameless: no background or border, and no inner padding. Used by the
    /// in-place cell editors, which must blend into the grid/design cell instead of looking like a
    /// native input box floating over it.
    pub bare: bool,
    /// Overrides [`DEFAULT_TEXT_SIZE`] (e.g. the 12px table-designer cells).
    pub text_size: Option<f32>,
    /// Overrides the kit control size (default `Size::Small`, i.e. 24px). Compact popup fields
    /// pass `Size::XSmall` (20px) to match the dropdowns.
    pub size: Option<Size>,
}

/// A queued mutation that must run on the next render, where a `Window` is available.
enum Pending {
    SetText(String, bool),
    SetPlaceholder(SharedString),
}

pub(crate) struct TextInput {
    theme: Theme,
    masked: bool,
    placeholder: SharedString,
    accepts: Option<Rc<dyn Fn(char) -> bool + 'static>>,
    icon: Option<&'static str>,
    icon_color: Option<u32>,
    clearable: bool,
    bare: bool,
    text_size: Option<f32>,
    size: Option<Size>,
    /// The entity's own focus handle. Callers focus this one; the render pass forwards focus to
    /// the inner gpui-kit state.
    focus: FocusHandle,
    /// The gpui-kit input state, created on the first render.
    state: Option<Entity<InputState>>,
    subscriptions: Vec<Subscription>,
    pending: Vec<Pending>,
    /// Text handed to the inner input at creation time.
    initial_text: String,
    /// A change reported by the inner input, waiting for the next render to reach the callback.
    pending_change: Option<String>,
    /// Whether the next change event was caused by a programmatic `set_text`.
    suppress_change: bool,
    /// An Enter press reported by the inner input, waiting for the next render.
    pending_submit: bool,
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
        Self {
            theme,
            masked: options.masked,
            placeholder: options.placeholder,
            accepts: options.accepts,
            icon: options.icon,
            icon_color: options.icon_color,
            clearable: options.clearable,
            bare: options.bare,
            text_size: options.text_size,
            size: options.size,
            focus: cx.focus_handle(),
            state: None,
            subscriptions: Vec::new(),
            pending: Vec::new(),
            initial_text: text.into(),
            pending_change: None,
            suppress_change: false,
            pending_submit: false,
            on_change: None,
            on_submit: None,
            on_cancel: None,
            on_tab: None,
        }
    }

    /// Replaces the field's text. Applied on the next render, where a `Window` is available.
    pub(crate) fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.pending.push(Pending::SetText(text.into(), true));
        cx.notify();
    }

    /// Replaces the field's placeholder. Applied on the next render, where a `Window` is available.
    pub(crate) fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.pending
            .push(Pending::SetPlaceholder(placeholder.into()));
        cx.notify();
    }

    /// Retained for API compatibility; the kit owns the field padding.
    pub(crate) fn set_padding_left(&mut self, _padding: f32, _cx: &mut Context<Self>) {}

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

    /// Focus the field, forwarding to the inner gpui-kit input when its lazily-created state
    /// already exists. Focusing the wrapper handle beforehand is not enough once the dialog's
    /// focus handling has moved on: the wrapper has no input handler, so keystrokes would be lost.
    pub(crate) fn focus_state(&self, window: &mut Window, cx: &mut App) {
        if let Some(state) = self.state.clone() {
            state.update(cx, |state, cx| state.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
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

    /// Creates the inner gpui-kit state on the first render and wires its events.
    fn ensure_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_some() {
            return;
        }

        let placeholder = self.placeholder.clone();
        let masked = self.masked;
        let initial = self.initial_text.clone();
        let accepts = self.accepts.clone();
        let state = cx.new(|cx| {
            let mut builder = InputState::new(window, cx)
                .placeholder(placeholder)
                .masked(masked)
                .default_value(initial);
            if let Some(accepts) = accepts {
                builder = builder.validate(move |text, _cx| text.chars().all(|c| accepts(c)));
            }
            builder
        });

        self.subscriptions
            .push(
                cx.subscribe(&state, |this, state, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        if this.suppress_change {
                            this.suppress_change = false;
                        } else {
                            this.pending_change = Some(state.read(cx).value().to_string());
                        }
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        this.pending_submit = true;
                        cx.notify();
                    }
                    _ => {}
                }),
            );

        // Forward focus from our own handle to the inner state.
        self.subscriptions
            .push(cx.on_focus(&self.focus, window, |this, window, cx| {
                if let Some(state) = this.state.clone() {
                    state.update(cx, |state, cx| state.focus(window, cx));
                }
            }));

        self.state = Some(state);
        // Callers often focus the wrapper right after creating it, before its lazy inner state
        // exists (in-place cell editors, form fields, prompts). `on_focus` above only fires when
        // focus *changes*, so forward the already-held focus now or the field is never typable.
        // Put the caret at the end (rather than the default start) so the field opens ready to
        // keep typing, like a plain input.
        if self.focus.is_focused(window) {
            let state = self.state.clone().expect("text input state");
            let end = self.initial_text.chars().count() as u32;
            state.update(cx, |state, cx| {
                state.set_cursor_position(Position::new(0, end), window, cx);
            });
        }
    }

    /// Applies queued mutations and dispatches queued callbacks, all of which need a `Window`.
    fn flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.state.clone() else {
            return;
        };

        for action in std::mem::take(&mut self.pending) {
            match action {
                Pending::SetText(text, suppress) => {
                    if suppress {
                        self.suppress_change = true;
                    }
                    state.update(cx, |state, cx| {
                        state.set_value(text.clone(), window, cx);
                    });
                }
                Pending::SetPlaceholder(placeholder) => {
                    state.update(cx, |state, cx| {
                        state.set_placeholder(placeholder.clone(), window, cx);
                    });
                }
            }
        }

        if let Some(text) = self.pending_change.take()
            && let Some(callback) = self.on_change.clone()
        {
            callback(&text, window, cx);
        }

        if self.pending_submit {
            self.pending_submit = false;
            if let Some(callback) = self.on_submit.clone() {
                callback(window, cx);
            }
        }
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => {
                if let Some(callback) = self.on_cancel.clone() {
                    callback(window, cx);
                }
            }
            "tab" => {
                if let Some(callback) = self.on_tab.clone() {
                    callback(event.keystroke.modifiers.shift, window, cx);
                }
            }
            _ => {}
        }
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_state(window, cx);
        self.flush(window, cx);
        let _ = self.theme;
        let state = self.state.clone().expect("text input state");

        let mut input = Input::new(&state)
            .h_full()
            .with_size(self.size.unwrap_or(Size::Small))
            .bordered(!self.bare)
            .appearance(!self.bare)
            .focus_ring(false)
            .cleanable(self.clearable)
            .text_size(px(self.text_size.unwrap_or(DEFAULT_TEXT_SIZE)));
        if self.bare {
            // Blend into the cell: no inset, so the caret and text line up with the cell text.
            input = input.px(px(0.0)).py(px(0.0));
        }
        if let Some(icon) = self.icon {
            input = input.prefix(
                Icon::default()
                    .path(icon)
                    .text_color(rgb(self.icon_color.unwrap_or(self.theme.text_muted))),
            );
        }

        div()
            .size_full()
            .on_key_down(cx.listener(Self::handle_key_down))
            .child(input)
    }
}
