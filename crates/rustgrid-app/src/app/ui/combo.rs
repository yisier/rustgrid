//! A searchable drop-down (combo box), backed by gpui-kit's shadcn components.
//!
//! A single-select dropdown uses gpui-kit's `Select`, a multi-select one its `Combobox`. The two
//! render the same shadcn trigger and popup (a search field over a row list with a check mark),
//! but only `Select` closes on every item click: `Combobox` skips the close when the clicked row
//! is already selected (it sees no selection change), which used to leave the filter builder's
//! field dropdown hanging open when the current field was clicked again.
//!
//! The host owns one [`ComboBox`] entity per field and drives it through
//! [`ComboBox::set_options`] / [`ComboBox::set_selected`], reacting to a pick through
//! [`ComboBox::on_select`]. The popup, search field, keyboard navigation and overlay all come
//! from the component library.
//!
//! gpui-kit's state types need a `Window` to be constructed, but the app builds its combos
//! while the view is constructed. The state is therefore created on the first render and pending
//! option/selection changes are flushed there.

use std::rc::Rc;

use gpui::{
    App, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*,
    px, rgb, svg,
};
use gpui_kit::component::Icon;
use gpui_kit::component::combobox::{Combobox, ComboboxEvent, ComboboxState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{FocusableExt, IndexPath, Sizable};

use crate::theme::Theme;

/// Called with the chosen option's value after the user picks it.
pub(crate) type ComboSelectCallback = Rc<dyn Fn(&str, &mut Window, &mut App) + 'static>;

/// Called with every selected value after a multi-select dropdown changes.
pub(crate) type ComboSelectManyCallback = Rc<dyn Fn(&[String], &mut Window, &mut App) + 'static>;

/// One selectable row: `value` is the machine value, `label` is what the user sees.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ComboOption {
    pub value: String,
    pub label: String,
    /// An optional leading glyph (an engine mark for a connection, a database for a schema, ...).
    icon: Option<(&'static str, u32)>,
}

impl ComboOption {
    pub(crate) fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            icon: None,
        }
    }

    /// A row whose value and label are the same string.
    pub(crate) fn plain(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            label: value.clone(),
            value,
            icon: None,
        }
    }

    /// Draw `icon` (tinted with `color`) before the label in both the dropdown row and — via
    /// [`ComboBox::set_icon`] — the trigger when the caller wants that.
    pub(crate) fn with_icon(mut self, icon: &'static str, color: u32) -> Self {
        self.icon = Some((icon, color));
        self
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

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let Some((icon, color)) = self.icon else {
            return self.title().into_any_element();
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                svg()
                    .path(icon)
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_none()
                    .text_color(rgb(color)),
            )
            .child(SharedString::from(self.label.clone()))
            .into_any_element()
    }
}

type ComboDelegate = SearchableVec<ComboOption>;
type ComboState = ComboboxState<ComboDelegate>;
type SingleSelectState = SelectState<ComboDelegate>;

/// The gpui-kit state behind a [`ComboBox`]: `Select` for a single value, `Combobox` when the
/// popup keeps several values selected.
#[derive(Clone)]
enum ComboBacking {
    Single(Entity<SingleSelectState>),
    Multiple(Entity<ComboState>),
}

pub(crate) struct ComboBox {
    theme: Theme,
    width: f32,
    /// When set, the trigger field is pinned to this width instead of sizing to its content
    /// (the dropdown menu always uses `width`).
    field_width: Option<f32>,
    /// Stretches the trigger field to fill its parent instead of sizing to its content.
    full_width: bool,
    /// Optional trigger height / text size / corner radius overrides (the filter builder renders
    /// its selects taller than the app's compact default).
    height: Option<f32>,
    text_size: Option<f32>,
    radius: Option<f32>,
    enabled: bool,
    placeholder: SharedString,
    icon: Option<(&'static str, u32)>,
    /// Whether the dropdown keeps the popup open and toggles items (used by the query-result
    /// export picker) instead of replacing the selection.
    multiple: bool,
    selected: String,
    selected_values: Vec<String>,
    options: Vec<ComboOption>,
    on_select: Option<ComboSelectCallback>,
    on_select_many: Option<ComboSelectManyCallback>,
    /// The gpui-kit state, created on the first render.
    state: Option<ComboBacking>,
    subscriptions: Vec<Subscription>,
    pending_options: bool,
    pending_selected: bool,
    /// The value the user last picked from the popup, cleared when the host adopts it through
    /// [`Self::set_selected`]. See [`Self::picked_value`].
    picked: Option<String>,
    /// The values the user last picked in multi-select mode; cleared by
    /// [`Self::set_selected_values`].
    picked_values: Vec<String>,
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
            full_width: false,
            height: None,
            text_size: None,
            radius: None,
            enabled: true,
            placeholder: SharedString::default(),
            icon: None,
            multiple: false,
            selected: selected.into(),
            selected_values: Vec::new(),
            options,
            on_select: None,
            on_select_many: None,
            state: None,
            subscriptions: Vec::new(),
            pending_options: false,
            pending_selected: false,
            picked: None,
            picked_values: Vec::new(),
        }
    }

    /// Makes the dropdown a multi-select: clicking an item toggles it and the popup stays open.
    pub(crate) fn multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    pub(crate) fn on_select(mut self, callback: ComboSelectCallback) -> Self {
        self.on_select = Some(callback);
        self
    }

    pub(crate) fn on_select_many(mut self, callback: ComboSelectManyCallback) -> Self {
        self.on_select_many = Some(callback);
        self
    }

    /// Pins the trigger field to a fixed width, so an empty selection does not collapse it.
    pub(crate) fn field_width(mut self, width: f32) -> Self {
        self.field_width = Some(width);
        self
    }

    /// Stretches the trigger field to fill its parent (used by dialogs, so a combo matches the
    /// width of the text fields above it).
    pub(crate) fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    /// Overrides the trigger height (the filter builder renders its selects at 40px).
    pub(crate) fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }

    /// Overrides the trigger's font size, independent of the height.
    pub(crate) fn text_size(mut self, size: f32) -> Self {
        self.text_size = Some(size);
        self
    }

    /// Overrides the trigger's corner radius.
    pub(crate) fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(radius);
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
        // The host has adopted a value of its own (normally the pick the callback just delivered),
        // so no pick is left waiting to be applied.
        self.picked = None;
        self.pending_selected = true;
        cx.notify();
    }

    /// Sets the multi-select values. The kit applies them on the next render.
    pub(crate) fn set_selected_values(&mut self, values: Vec<String>, cx: &mut Context<Self>) {
        if self.selected_values == values {
            return;
        }
        self.selected_values = values;
        self.picked_values.clear();
        self.pending_selected = true;
        cx.notify();
    }

    /// The value the control is showing: a pick the host has not written back yet, else the
    /// kit's committed selection.
    ///
    /// A pick reaches the host's `on_select` immediately, so this normally just mirrors the model.
    /// A host that reads its model in the same gesture as another control (the grid filter's 应用)
    /// consults it anyway, which guarantees the applied condition matches what the trigger shows
    /// even if the model update was missed.
    pub(crate) fn active_value(&self, cx: &App) -> Option<String> {
        if let Some(picked) = &self.picked {
            return Some(picked.clone());
        }
        let selected = match self.state.as_ref() {
            Some(ComboBacking::Single(state)) => state.read(cx).selected_value().cloned(),
            Some(ComboBacking::Multiple(state)) => state.read(cx).selected_value(),
            None => None,
        };
        selected.or_else(|| (!self.selected.is_empty()).then(|| self.selected.clone()))
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
        // Deliver the pick immediately rather than on the next render. A host that reads its own
        // model in the meantime (the filter builder's 应用 is the one that bit users: the trigger
        // showed the new field while the applied condition still held the old one) then always sees
        // the value the user just chose.
        let state = if self.multiple {
            let items = SearchableVec::new(self.options.clone());
            let state = cx.new(|cx| {
                ComboboxState::new(items, vec![], window, cx)
                    .multiple(true)
                    .searchable(true)
            });
            self.subscriptions.push(cx.subscribe_in(
                &state,
                window,
                |this, _state, event: &ComboboxEvent<ComboDelegate>, window, cx| {
                    if let ComboboxEvent::Change(values) = event {
                        this.picked_values = values.clone();
                        if let Some(callback) = this.on_select_many.clone() {
                            callback(values, window, cx);
                        }
                    }
                },
            ));
            ComboBacking::Multiple(state)
        } else {
            // A single-select popup is `Select`, not `Combobox`: it closes on *every* item click,
            // including the row that is already selected. `ComboboxState` compares the selection
            // before/after and skips the close when nothing changed, which left the filter
            // builder's field dropdown open when the current field was picked again.
            let selected_index = self
                .options
                .iter()
                .position(|option| option.value == self.selected)
                .map(IndexPath::new);
            let items = SearchableVec::new(self.options.clone());
            let state =
                cx.new(|cx| SelectState::new(items, selected_index, window, cx).searchable(true));
            self.subscriptions.push(cx.subscribe_in(
                &state,
                window,
                |this, _state, event: &SelectEvent<ComboDelegate>, window, cx| {
                    if let SelectEvent::Confirm(Some(value)) = event {
                        this.picked = Some(value.clone());
                        if let Some(callback) = this.on_select.clone() {
                            callback(value, window, cx);
                        }
                    }
                },
            ));
            ComboBacking::Single(state)
        };
        self.state = Some(state);
        self.pending_options = true;
        self.pending_selected = true;
    }

    /// Applies the queued option/selection mutations that need a `Window`.
    fn flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.state.clone() else {
            return;
        };

        if self.pending_options {
            self.pending_options = false;
            match &state {
                ComboBacking::Single(state) => {
                    let items = SearchableVec::new(self.options.clone());
                    state.update(cx, |state, cx| state.set_items(items, window, cx));
                }
                ComboBacking::Multiple(state) => {
                    let items = SearchableVec::new(self.options.clone());
                    state.update(cx, |state, cx| state.set_items(items, window, cx));
                }
            }
        }
        if self.pending_selected {
            self.pending_selected = false;
            let selected = self.selected.clone();
            let selected_values = self.selected_values.clone();
            match &state {
                ComboBacking::Single(state) => {
                    state.update(cx, |state, cx| {
                        state.set_selected_value(&selected, window, cx);
                    });
                }
                ComboBacking::Multiple(state) => {
                    state.update(cx, |state, cx| {
                        state.set_selected_values(&selected_values, window, cx);
                    });
                }
            }
        }
    }
}

impl Render for ComboBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.ensure_state(window, cx);
        self.flush(window, cx);
        let state = self.state.clone().expect("combo state");

        let mut container = div();
        if self.full_width {
            container = container.w_full();
        } else if let Some(width) = self.field_width {
            container = container.w(px(width));
        }

        // gpui-kit defaults the popup's search field to an untranslated "Search...". Every
        // dropdown in the app gets the current language's placeholder instead.
        let search_placeholder = SharedString::from(t!("common.search_placeholder").to_string());
        let control = match &state {
            ComboBacking::Single(state) => {
                let mut select = Select::new(state)
                    .placeholder(self.placeholder.clone())
                    .search_placeholder(search_placeholder.clone())
                    .menu_width(px(self.width))
                    .cleanable(false)
                    .disabled(!self.enabled)
                    .focus_ring(false)
                    .xsmall();
                if let Some(height) = self.height {
                    select = select.h(px(height));
                }
                if let Some(size) = self.text_size {
                    select = select.text_size(px(size));
                }
                if let Some(radius) = self.radius {
                    select = select.rounded(px(radius));
                }
                if let Some((icon, color)) = self.icon {
                    select = select.icon(Icon::default().path(icon).text_color(rgb(color)));
                }
                select.into_any_element()
            }
            ComboBacking::Multiple(state) => {
                let mut combo = Combobox::new(state)
                    .placeholder(self.placeholder.clone())
                    .search_placeholder(search_placeholder.clone())
                    .menu_width(px(self.width))
                    .cleanable(false)
                    .disabled(!self.enabled)
                    .focus_ring(false)
                    .xsmall();
                if let Some(height) = self.height {
                    combo = combo.h(px(height));
                }
                if let Some(size) = self.text_size {
                    combo = combo.text_size(px(size));
                }
                if let Some(radius) = self.radius {
                    combo = combo.rounded(px(radius));
                }
                if let Some((icon, color)) = self.icon {
                    combo = combo.icon(Icon::default().path(icon).text_color(rgb(color)));
                }
                combo.into_any_element()
            }
        };
        container.child(control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, Keystroke, Point, TestAppContext, WindowBounds, WindowOptions, size};
    use gpui_kit::TestSupportExt as _;
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::test::TestWindowExt as _;

    struct ComboHarness {
        combo: Entity<ComboBox>,
        show: bool,
    }

    impl Render for ComboHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let mut root = div().size_full().p_4();
            if self.show {
                root = root.child(
                    div()
                        .id("combo-host")
                        .test_support()
                        .w(px(300.0))
                        .h(px(24.0))
                        .child(self.combo.clone()),
                );
            }
            root
        }
    }

    fn options() -> Vec<ComboOption> {
        vec![
            ComboOption::plain("macce"),
            ComboOption::plain("sjis"),
            ComboOption::plain("utf8mb4"),
        ]
    }

    fn open(
        cx: &mut TestAppContext,
        selected: &str,
    ) -> (
        gpui::AnyWindowHandle,
        Entity<ComboHarness>,
        Rc<std::cell::RefCell<Option<String>>>,
    ) {
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            let picked = Rc::new(std::cell::RefCell::new(None::<String>));
            let sink = picked.clone();
            let combo = cx.new(|cx| {
                ComboBox::new(Theme::light(), options(), selected, 300.0, cx)
                    .field_width(300.0)
                    .on_select(Rc::new(move |value, _window, _cx| {
                        *sink.borrow_mut() = Some(value.to_string());
                    }))
            });
            let harness = cx.new(|_| ComboHarness { combo, show: true });
            let (handle, _) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(800.0), px(600.0)),
                    })),
                    ..Default::default()
                },
                cx,
                |_, _| harness.clone(),
            )
            .expect("open test window");
            (handle, harness, picked)
        })
    }

    /// Type `text` into the open popup's search field (when non-empty) and confirm the first
    /// match with Enter.
    fn search_and_confirm(
        handle: gpui::AnyWindowHandle,
        cx: &mut TestAppContext,
        text: &str,
        picked: &Rc<std::cell::RefCell<Option<String>>>,
    ) -> Option<String> {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("combo-host", cx);
            window.render_frame(cx);
            if !text.is_empty() {
                window.input(text, cx);
                window.render_frame(cx);
            }
        })
        .unwrap();
        // The list debounces the search for 100ms; let it settle so the filtered rows are built.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        picked.borrow().clone()
    }

    #[gpui_kit::test]
    fn combo_search_receives_input_in_a_plain_window(cx: &mut TestAppContext) {
        let (handle, _harness, picked) = open(cx, "utf8mb4");
        assert_eq!(
            search_and_confirm(handle, cx, "mac", &picked).as_deref(),
            Some("macce"),
            "typing `mac` should narrow the list to the first matching row"
        );
    }

    /// Clicking the row that is already selected must still confirm (and close the popup). With
    /// `ComboboxState` this emitted no event at all (it only reacts to a selection *change*), so the
    /// filter builder's field dropdown stayed open when the current field was picked again.
    #[gpui_kit::test]
    fn combo_confirms_the_already_selected_row(cx: &mut TestAppContext) {
        let (handle, _harness, picked) = open(cx, "macce");
        assert_eq!(
            search_and_confirm(handle, cx, "", &picked).as_deref(),
            Some("macce")
        );
    }

    #[gpui_kit::test]
    fn combo_search_receives_input_inside_a_dialog(cx: &mut TestAppContext) {
        let (handle, harness, picked) = open(cx, "utf8mb4");
        let combo = cx.read(|cx| harness.read(cx).combo.clone());
        // The harness must not render the combo itself, or it would appear twice (behind the
        // dialog and inside it).
        harness.update(cx, |harness, cx| {
            harness.show = false;
            cx.notify();
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.open_dialog(cx, move |dialog, _window, _cx| {
                let combo = combo.clone();
                dialog.title("Test").content(move |content, _window, _cx| {
                    content.child(
                        div()
                            .id("combo-host")
                            .test_support()
                            .w(px(300.0))
                            .h(px(24.0))
                            .child(combo.clone()),
                    )
                })
            });
            window.render_frame(cx);
        })
        .unwrap();

        assert_eq!(
            search_and_confirm(handle, cx, "mac", &picked).as_deref(),
            Some("macce")
        );
    }

    /// The New/Edit Database dialog keeps a name field focused once and then lets the user move to
    /// the combo box. Its builder runs every frame, so the one-shot focus must survive across
    /// frames — otherwise the name field is re-focused each frame and the combo's search box can
    /// never hold the focus (this is what `AppView::dialog_focus_done` guards).
    #[gpui_kit::test]
    fn combo_search_survives_a_dialog_that_focuses_its_field_once(cx: &mut TestAppContext) {
        use crate::app::dialogs::take_dialog_focus;
        use crate::app::ui::{TextInput, TextInputOptions};
        use std::cell::RefCell;

        let (handle, harness, picked) = open(cx, "utf8mb4");
        let combo = cx.read(|cx| harness.read(cx).combo.clone());
        harness.update(cx, |harness, cx| {
            harness.show = false;
            cx.notify();
        });

        let name = cx.new(|cx| {
            TextInput::new(
                Theme::light(),
                String::new(),
                TextInputOptions::default(),
                cx,
            )
        });
        let focused = Rc::new(RefCell::new(false));
        let name_dialog = name.clone();
        let combo_dialog = combo.clone();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.open_dialog(cx, move |dialog, _window, _cx| {
                let name = name_dialog.clone();
                let combo = combo_dialog.clone();
                let focused = focused.clone();
                dialog.title("Test").content(move |content, window, cx| {
                    if take_dialog_focus(&mut focused.borrow_mut()) {
                        name.update(cx, |input, cx| input.focus_state(window, cx));
                    }
                    content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .id("name-host")
                                    .test_support()
                                    .w(px(300.0))
                                    .h(px(24.0))
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .id("combo-host")
                                    .test_support()
                                    .w(px(300.0))
                                    .h(px(24.0))
                                    .child(combo.clone()),
                            ),
                    )
                })
            });
            window.render_frame(cx);
        })
        .unwrap();

        assert_eq!(
            search_and_confirm(handle, cx, "mac", &picked).as_deref(),
            Some("macce")
        );
    }
}
