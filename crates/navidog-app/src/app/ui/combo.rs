//! An editable, searchable drop-down (combo box) in the classic Win32/Navicat style.
//!
//! The field above the list is a real [`TextInput`], so it can be focused, edited and its text
//! selected/overwritten with the mouse or the keyboard (drag, Shift+arrows, Ctrl+A). Every
//! keystroke filters the options with a case-insensitive fuzzy match; Enter picks the highlighted
//! option, Escape closes and restores the committed value. The host owns one [`ComboBox`] entity
//! per field and drives it through [`ComboBox::set_options`] / [`ComboBox::set_selected`] plus the
//! [`ComboBox::on_select`] callback.

use std::rc::Rc;

use gpui::{
    App, Context, Entity, IntoElement, KeyDownEvent, MouseButton, MouseMoveEvent, Pixels, Render,
    ScrollHandle, SharedString, Subscription, WeakEntity, Window, deferred, div, prelude::*, px,
    rgb, svg,
};

use super::{
    TextInput, TextInputOptions, dialog_shadow, scrollbar_fractions, scrollbar_thumb,
    vscrollbar_track,
};
use crate::theme::Theme;

/// Called with the chosen option's value after the user picks it.
pub(crate) type ComboSelectCallback = Rc<dyn Fn(&str, &mut Window, &mut App) + 'static>;

/// One selectable row: `value` is the machine value, `label` is what the user sees.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ComboOption {
    pub value: String,
    pub label: String,
}

impl ComboOption {
    pub(crate) fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }

    /// A row whose value and label are the same string.
    pub(crate) fn plain(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            label: value.clone(),
            value,
        }
    }
}

pub(crate) struct ComboBox {
    theme: Theme,
    id: u64,
    input: Entity<TextInput>,
    blur_subscription: Option<Subscription>,
    options: Vec<ComboOption>,
    selected: String,
    query: String,
    open: bool,
    highlight: Option<usize>,
    width: f32,
    height: f32,
    enabled: bool,
    icon: Option<(&'static str, u32)>,
    on_select: Option<ComboSelectCallback>,
    scroll: ScrollHandle,
    scroll_grab: Option<f32>,
    /// Height of the visible options viewport for the current popup, in pixels.
    scroll_viewport: f32,
    /// Maximum scroll offset for the current popup, derived from the row count (gpui's cached
    /// `max_offset` is a frame behind, which made the thumb change length when the list opened).
    scroll_max: f32,
    /// Text to push into the input on the next render. Applying it eagerly from a callback that
    /// runs inside the input's own handler (Enter/Escape) would re-enter its borrow.
    pending_text: Option<String>,
}

impl ComboBox {
    pub(crate) fn new(
        theme: Theme,
        options: Vec<ComboOption>,
        selected: impl Into<String>,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected = selected.into();
        let label = label_of(&options, &selected);
        let id = cx.entity_id().as_u64();
        let weak: WeakEntity<Self> = cx.weak_entity();
        let change = weak.clone();
        let submit = weak.clone();
        let cancel = weak;
        let input = cx.new(move |cx| {
            TextInput::new(theme, label, TextInputOptions::default(), cx)
                .on_change(Rc::new(move |text, _window, cx| {
                    let _ = change.update(cx, |combo, cx| combo.query_changed(text, cx));
                }))
                .on_submit(Rc::new(move |window, cx| {
                    let _ = submit.update(cx, |combo, cx| combo.commit(window, cx));
                }))
                .on_cancel(Rc::new(move |_window, cx| {
                    let _ = cancel.update(cx, |combo, cx| combo.cancel(cx));
                }))
        });

        Self {
            theme,
            id,
            input,
            blur_subscription: None,
            options,
            selected,
            query: String::new(),
            open: false,
            highlight: None,
            width,
            height: 24.0,
            enabled: true,
            icon: None,
            on_select: None,
            scroll: ScrollHandle::new(),
            scroll_grab: None,
            scroll_viewport: 0.0,
            scroll_max: 0.0,
            pending_text: None,
        }
    }

    pub(crate) fn on_select(mut self, callback: ComboSelectCallback) -> Self {
        self.on_select = Some(callback);
        self
    }

    /// Replaces the list of rows. Keeps the current filter/highlight valid.
    pub(crate) fn set_options(&mut self, options: Vec<ComboOption>, cx: &mut Context<Self>) {
        if self.options == options {
            return;
        }
        self.options = options;
        self.highlight = self.highlight.filter(|index| *index < self.options.len());
        if self.open {
            self.highlight = self.matches().first().copied();
        }
        cx.notify();
    }

    /// Sets the committed value and shows its label (unless the user is mid-edit).
    pub(crate) fn set_selected(&mut self, value: impl Into<String>, cx: &mut Context<Self>) {
        let value = value.into();
        if self.selected == value {
            return;
        }
        self.selected = value;
        if !self.open {
            self.restore_text();
        }
        cx.notify();
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            self.open = false;
        }
        cx.notify();
    }

    pub(crate) fn set_icon(&mut self, icon: &'static str, color: u32, cx: &mut Context<Self>) {
        self.icon = Some((icon, color));
        self.input
            .update(cx, |input, cx| input.set_icon(Some(icon), Some(color), cx));
        cx.notify();
    }

    pub(crate) fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        self.input
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
    }

    pub(crate) fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        self.input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        cx.notify();
    }

    fn label_for(&self, value: &str) -> String {
        // Prefer a matching row's label; fall back to the raw value so unknown values still show.
        self.options
            .iter()
            .find(|option| option.value == value)
            .map(|option| option.label.clone())
            .unwrap_or_else(|| value.to_string())
    }

    fn restore_text(&mut self) {
        self.pending_text = Some(self.label_for(&self.selected));
    }

    /// Indices into `options` that match the current query, best matches first.
    fn matches(&self) -> Vec<usize> {
        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return (0..self.options.len()).collect();
        }
        let mut substring = Vec::new();
        let mut subsequence = Vec::new();
        for (index, option) in self.options.iter().enumerate() {
            let label = option.label.to_lowercase();
            if label.contains(&query) {
                substring.push(index);
            } else if is_subsequence(&query, &label) {
                subsequence.push(index);
            }
        }
        substring.extend(subsequence);
        substring
    }

    fn query_changed(&mut self, text: &str, cx: &mut Context<Self>) {
        self.query = text.to_string();
        self.open = true;
        self.highlight = self.matches().first().copied();
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled || self.open {
            return;
        }
        self.open = true;
        self.query.clear();
        self.highlight = self
            .options
            .iter()
            .position(|option| option.value == self.selected)
            .or_else(|| self.matches().first().copied());
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        let focus = self.input.read(cx).focus_handle();
        window.focus(&focus);
        self.input.update(cx, |input, cx| input.select_all(cx));
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.query.clear();
        self.highlight = None;
        self.restore_text();
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.close(cx);
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let matches = self.matches();
        if matches.is_empty() {
            return;
        }
        let current = self
            .highlight
            .and_then(|highlight| matches.iter().position(|index| *index == highlight))
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(matches.len() as isize) as usize;
        self.highlight = Some(matches[next]);
        cx.notify();
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choice = self
            .highlight
            .filter(|index| *index < self.options.len())
            .or_else(|| self.matches().first().copied());
        match choice {
            Some(index) => self.choose(index, window, cx),
            None => self.cancel(cx),
        }
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(option) = self.options.get(index).cloned() else {
            return;
        };
        self.selected = option.value.clone();
        self.open = false;
        self.query.clear();
        self.highlight = None;
        self.pending_text = Some(option.label.clone());
        if let Some(callback) = self.on_select.clone() {
            callback(&option.value, window, cx);
        }
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "up" => {
                self.move_highlight(-1, cx);
                cx.stop_propagation();
            }
            "down" => {
                self.move_highlight(1, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// A draggable track+thumb, driven by this combo's scroll handle (gpui 0.2 paints no
    /// scrollbars for `overflow_*`, so the shared `vscrollbar_track` is drawn manually).
    fn render_scrollbar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let max = self.scroll_max;
        let viewport = self.scroll_viewport;
        if max <= 0.0 || viewport <= 0.0 {
            return div().into_any_element();
        }
        let scroll = (-f32::from(self.scroll.offset().y)).clamp(0.0, max);
        let (thumb_top, thumb_len) = scrollbar_fractions(viewport, max, scroll);
        vscrollbar_track(
            SharedString::from(format!("combo-scrollbar-{}", self.id)),
            self.theme,
            thumb_top,
            thumb_len,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &gpui::MouseDownEvent, _window, cx| {
                this.scroll_begin(event.position.y, cx);
            }),
        )
        .into_any_element()
    }

    fn scroll_begin(&mut self, mouse_y: Pixels, cx: &mut Context<Self>) {
        let bounds = self.scroll.bounds();
        let viewport = self.scroll_viewport;
        let max = self.scroll_max;
        let (thumb_h, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let scroll = (-f32::from(self.scroll.offset().y)).clamp(0.0, max);
        let thumb_y = (scroll / max) * travel;
        let relative = f32::from(mouse_y) - f32::from(bounds.top());
        let grab = if relative >= thumb_y && relative <= thumb_y + thumb_h {
            relative - thumb_y
        } else {
            thumb_h / 2.0
        };
        self.scroll_grab = Some(grab);
        self.scroll_set(relative, grab, cx);
    }

    fn scroll_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(grab) = self.scroll_grab else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.scroll_grab = None;
            cx.notify();
            return;
        }
        let relative = f32::from(event.position.y) - f32::from(self.scroll.bounds().top());
        self.scroll_set(relative, grab, cx);
    }

    fn scroll_set(&mut self, relative: f32, grab: f32, cx: &mut Context<Self>) {
        let viewport = self.scroll_viewport;
        let max = self.scroll_max;
        let (_, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let thumb_y = (relative - grab).clamp(0.0, travel);
        let scroll = thumb_y / travel * max;
        self.scroll.set_offset(gpui::point(px(0.0), px(-scroll)));
        cx.notify();
    }

    fn render_popup(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let matches = self.matches();

        let mut options = div()
            .id(SharedString::from(format!("combo-options-{}", self.id)))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col();

        for &index in &matches {
            let option = &self.options[index];
            let selected = option.value == self.selected;
            let highlighted = self.highlight == Some(index);
            options = options.child(
                div()
                    .id(SharedString::from(format!(
                        "combo-option-{}-{index}",
                        self.id
                    )))
                    .flex()
                    .items_center()
                    .h(px(20.0))
                    .px_2()
                    .flex_none()
                    .text_size(px(12.0))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(highlighted, |style| {
                        style
                            .bg(rgb(theme.tree_selected_bg))
                            .text_color(rgb(theme.tree_selected_text))
                    })
                    .when(!highlighted && selected, |style| {
                        style.font_weight(gpui::FontWeight::SEMIBOLD)
                    })
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_mouse_move(cx.listener(move |this, _event, _window, cx| {
                        if this.highlight != Some(index) {
                            this.highlight = Some(index);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.choose(index, window, cx);
                    }))
                    .child(option.label.clone()),
            );
        }

        let row_height = 20.0f32;
        let total_rows = matches.len();
        let visible_rows = total_rows.clamp(1, 12);
        let viewport = visible_rows as f32 * row_height;
        let desired = viewport + 2.0;
        // Size the thumb from the known row geometry, not gpui's cached `max_offset`, so it stays
        // a stable length from the first frame the popup is shown.
        self.scroll_viewport = viewport;
        self.scroll_max = (total_rows as f32 * row_height - viewport).max(0.0);
        let list = div()
            .id(SharedString::from(format!("combo-list-{}", self.id)))
            .absolute()
            .top(px(self.height))
            .left_0()
            .w(px(self.width))
            .h(px(desired))
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.text_muted))
            .shadow(dialog_shadow())
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| this.close(cx)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.scroll_drag(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _window, _cx| this.scroll_grab = None),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(options)
                    .child(self.render_scrollbar(cx)),
            );

        deferred(list).with_priority(100).into_any_element()
    }
}

impl Render for ComboBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        if let Some(text) = self.pending_text.take() {
            self.input.update(cx, |input, cx| input.set_text(text, cx));
        }
        if self.blur_subscription.is_none() {
            let focus = self.input.read(cx).focus_handle();
            self.blur_subscription = Some(cx.on_blur(&focus, window, |this, _window, cx| {
                this.close(cx);
            }));
        }

        let theme = self.theme;
        let enabled = self.enabled;
        let open = self.open;
        let input = self.input.clone();

        let mut root = div()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .w(px(self.width))
            .h(px(self.height))
            .flex_none()
            .text_size(px(12.0));

        root = root.child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, window, cx| this.open(window, cx)),
                )
                .child(input),
        );
        root = root.child(
            div()
                .id(SharedString::from(format!("combo-chevron-{}", self.id)))
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.0))
                .h_full()
                .flex_none()
                .bg(rgb(theme.input_bg))
                .border_t_1()
                .border_r_1()
                .border_b_1()
                .border_color(rgb(theme.border))
                .when(enabled, |style| {
                    style.cursor_pointer().hover(move |style| {
                        style
                            .bg(rgb(theme.button_hover_bg))
                            .border_color(rgb(theme.button_default_border))
                    })
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, window, cx| {
                        if this.open {
                            this.cancel(cx);
                        } else {
                            this.open(window, cx);
                        }
                    }),
                )
                .child(
                    svg()
                        .path("icons/chevron-down.svg")
                        .w(px(11.0))
                        .h(px(11.0))
                        .flex_none()
                        .text_color(rgb(theme.text_muted)),
                ),
        );

        if open {
            root = root.child(self.render_popup(cx));
        }
        root.on_key_down(cx.listener(|this, event, window, cx| this.on_key_down(event, window, cx)))
    }
}

fn label_of(options: &[ComboOption], value: &str) -> String {
    options
        .iter()
        .find(|option| option.value == value)
        .map(|option| option.label.clone())
        .unwrap_or_else(|| value.to_string())
}

/// A case-insensitive subsequence test (`fzf`-style fuzzy match).
fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|needle| chars.any(|haystack| haystack == needle))
}
