//! A searchable drop-down (combo box), backed by gpui-kit's shadcn [`Combobox`].
//!
//! The host owns one [`ComboBox`] entity per field and drives it through
//! [`ComboBox::set_options`] / [`ComboBox::set_selected`], reacting to a pick through
//! [`ComboBox::on_select`]. The popup, search field, keyboard navigation and overlay all come
//! from the component library.
//!
//! gpui-kit's `ComboboxState` needs a `Window` to be constructed, but the app builds its combos
//! while the view is constructed. The state is therefore created on the first render and pending
//! option/selection changes are flushed there.

use std::rc::Rc;

use gpui::{
    App, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*,
    px, rgb,
};
use gpui_kit::component::Icon;
use gpui_kit::component::combobox::{Combobox, ComboboxEvent, ComboboxState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::{FocusableExt, Sizable};

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

impl SearchableListItem for ComboOption {
    type Value = String;

    fn title(&self) -> SharedString {
        SharedString::from(self.label.clone())
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

type ComboDelegate = SearchableVec<ComboOption>;
type ComboState = ComboboxState<ComboDelegate>;

pub(crate) struct ComboBox {
    theme: Theme,
    width: f32,
    /// When set, the trigger field is pinned to this width instead of sizing to its content
    /// (the dropdown menu always uses `width`).
    field_width: Option<f32>,
    enabled: bool,
    placeholder: SharedString,
    icon: Option<(&'static str, u32)>,
    selected: String,
    options: Vec<ComboOption>,
    on_select: Option<ComboSelectCallback>,
    /// The gpui-kit state, created on the first render.
    state: Option<Entity<ComboState>>,
    subscriptions: Vec<Subscription>,
    pending_options: bool,
    pending_selected: bool,
    /// A pick reported by the kit, waiting for the next render to reach the callback.
    pending_select: Option<String>,
}

impl ComboBox {
    pub(crate) fn new(
        theme: Theme,
        options: Vec<ComboOption>,
        selected: impl Into<String>,
        width: f32,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            theme,
            width,
            field_width: None,
            enabled: true,
            placeholder: SharedString::default(),
            icon: None,
            selected: selected.into(),
            options,
            on_select: None,
            state: None,
            subscriptions: Vec::new(),
            pending_options: false,
            pending_selected: false,
            pending_select: None,
        }
    }

    pub(crate) fn on_select(mut self, callback: ComboSelectCallback) -> Self {
        self.on_select = Some(callback);
        self
    }

    /// Pins the trigger field to a fixed width, so an empty selection does not collapse it.
    pub(crate) fn field_width(mut self, width: f32) -> Self {
        self.field_width = Some(width);
        self
    }

    /// Replaces the list of rows and re-applies the current selection.
    pub(crate) fn set_options(&mut self, options: Vec<ComboOption>, cx: &mut Context<Self>) {
        if self.options == options {
            return;
        }
        self.options = options;
        self.pending_options = true;
        self.pending_selected = true;
        cx.notify();
    }

    /// Sets the committed value.
    pub(crate) fn set_selected(&mut self, value: impl Into<String>, cx: &mut Context<Self>) {
        let value = value.into();
        if self.selected == value {
            return;
        }
        self.selected = value;
        self.pending_selected = true;
        cx.notify();
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        cx.notify();
    }

    pub(crate) fn set_icon(&mut self, icon: &'static str, color: u32, cx: &mut Context<Self>) {
        if self.icon == Some((icon, color)) {
            return;
        }
        self.icon = Some((icon, color));
        cx.notify();
    }

    pub(crate) fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if self.placeholder == placeholder {
            return;
        }
        self.placeholder = placeholder;
        cx.notify();
    }

    pub(crate) fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        cx.notify();
    }

    /// Creates the inner gpui-kit state on the first render and wires its events.
    fn ensure_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_some() {
            return;
        }
        let items = SearchableVec::new(self.options.clone());
        let state = cx.new(|cx| {
            ComboboxState::new(items, vec![], window, cx)
                .multiple(false)
                .searchable(true)
        });
        self.subscriptions.push(cx.subscribe(
            &state,
            |this, _state, event: &ComboboxEvent<ComboDelegate>, cx| {
                if let ComboboxEvent::Change(values) = event
                    && let Some(value) = values.first()
                {
                    this.pending_select = Some(value.clone());
                    cx.notify();
                }
            },
        ));
        self.state = Some(state);
        self.pending_options = true;
        self.pending_selected = true;
    }

    /// Applies queued mutations and dispatches queued callbacks, which need a `Window`.
    fn flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.state.clone() else {
            return;
        };

        if self.pending_options {
            self.pending_options = false;
            let items = SearchableVec::new(self.options.clone());
            state.update(cx, |state, cx| state.set_items(items, window, cx));
        }
        if self.pending_selected {
            self.pending_selected = false;
            let selected = self.selected.clone();
            state.update(cx, |state, cx| {
                state.set_selected_values(std::slice::from_ref(&selected), window, cx);
            });
        }

        if let Some(value) = self.pending_select.take()
            && let Some(callback) = self.on_select.clone()
        {
            callback(&value, window, cx);
        }
    }
}

impl Render for ComboBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_state(window, cx);
        self.flush(window, cx);
        let state = self.state.clone().expect("combo state");

        let mut combo = Combobox::new(&state)
            .placeholder(self.placeholder.clone())
            .menu_width(px(self.width))
            .cleanable(false)
            .disabled(!self.enabled)
            .focus_ring(false)
            .xsmall();
        if let Some((icon, color)) = self.icon {
            combo = combo.icon(Icon::default().path(icon).text_color(rgb(color)));
        }

        let mut container = div();
        if let Some(width) = self.field_width {
            container = container.w(px(width));
        }
        container.child(combo)
    }
}
