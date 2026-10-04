//! `SqlEditor`: the app-facing SQL editor, wrapping gpui-kit's `Editor`/`EditorState`.
//!
//! gpui-kit's editor owns the text, selection, undo/redo, multi-cursor, search and tree-sitter
//! highlighting; this wrapper keeps feature code off `EditorState` directly. Like `ui::TextInput`
//! / `ui::ComboBox`, the inner state needs a `&mut Window`, so it is built on the first render and
//! pending mutations are flushed there.

use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Window, div, px,
};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{
    Copy, Cut, Editor, EditorState, InputEvent, Paste, Redo, SelectAll, Undo,
};
use gpui_kit::component::native_menu::NativeMenu;

use crate::app::sql_completion::CompletionScope;

/// Options applied when the editor is first built.
#[derive(Clone)]
pub(crate) struct SqlEditorOptions {
    pub language: SharedString,
    pub font_family: SharedString,
    pub font_size: f32,
    pub line_number: bool,
    /// Render a non-editable preview (used by the routine/view SQL 预览 tabs).
    pub readonly: bool,
}

impl Default for SqlEditorOptions {
    fn default() -> Self {
        Self {
            language: "sql".into(),
            font_family: "Consolas".into(),
            font_size: 13.0,
            line_number: true,
            readonly: false,
        }
    }
}

/// Called with the whole new text whenever the editor's content changes.
pub(crate) type SqlEditorChange = Rc<dyn Fn(&str, &mut App)>;

pub(crate) struct SqlEditor {
    text: String,
    options: SqlEditorOptions,
    on_change: Option<SqlEditorChange>,
    /// The provider, or a factory that builds it with the inner state's context.
    provider: Option<Rc<dyn gpui_kit::component::input::CompletionProvider>>,
    /// The provider's live connection/database/schema scope, so re-scoping does not need to
    /// rebuild the editor.
    scope: Option<Arc<RwLock<CompletionScope>>>,
    state: Option<Entity<EditorState>>,
    subscriptions: Vec<Subscription>,
    /// A `set_text` requested before the state existed (or that still needs applying).
    pending_text: Option<String>,
    /// Whether the queued text is a full replace (rather than just a mirror).
    pending_replace: bool,
    /// Set while the wrapper pushes text into the editor, so the resulting change does not echo
    /// back to the app and loop.
    suppress_change: bool,
}

impl SqlEditor {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let _ = cx;
        Self {
            text: String::new(),
            options: SqlEditorOptions::default(),
            on_change: None,
            provider: None,
            scope: None,
            state: None,
            subscriptions: Vec::new(),
            pending_text: None,
            pending_replace: false,
            suppress_change: false,
        }
    }

    pub(crate) fn options(mut self, options: SqlEditorOptions) -> Self {
        self.options = options;
        self
    }

    pub(crate) fn on_change(mut self, callback: SqlEditorChange) -> Self {
        self.on_change = Some(callback);
        self
    }

    pub(crate) fn provider(
        mut self,
        provider: Rc<dyn gpui_kit::component::input::CompletionProvider>,
    ) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Share the completion scope with the provider, so [`SqlEditor::set_scope`] can re-scope it.
    pub(crate) fn scope(mut self, scope: Arc<RwLock<CompletionScope>>) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Re-scope the completion provider after the tab's connection/database/schema changes.
    pub(crate) fn set_scope(
        &mut self,
        connection_index: Option<usize>,
        database: Option<String>,
        schema: Option<String>,
        supports_schemas: bool,
        driver: Option<String>,
        _cx: &mut Context<Self>,
    ) {
        if let Some(scope) = &self.scope
            && let Ok(mut scope) = scope.write()
        {
            scope.connection_index = connection_index;
            scope.database = database;
            scope.schema = schema;
            scope.supports_schemas = supports_schemas;
            scope.driver = driver;
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The active selection as byte offsets (`start == end` when nothing is selected).
    pub(crate) fn selected_range(&self, cx: &App) -> (usize, usize) {
        let Some(state) = self.state.as_ref() else {
            return (0, 0);
        };
        let range = state.read(cx).selected_range();
        (range.start, range.end)
    }

    /// Replace the whole document. Before the first render (and whenever no window is at hand) the
    /// text is queued and applied on the next `flush`.
    pub(crate) fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        self.text = text.clone();
        self.pending_text = Some(text);
        self.pending_replace = true;
        cx.notify();
    }

    fn flush(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_none() {
            let options = self.options.clone();
            let provider = self.provider.clone();
            let state = cx.new(|cx| {
                let mut state = EditorState::new(window, cx)
                    .language(options.language.clone())
                    .line_number(options.line_number);
                if let Some(provider) = provider {
                    state.lsp_mut().completion_provider = Some(provider);
                }
                state
            });
            self.subscriptions.push(cx.subscribe(
                &state,
                |editor: &mut Self, _state, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        editor.on_inner_change(cx);
                    }
                },
            ));
            self.state = Some(state);
        }
        if let Some(pending) = self.pending_text.take() {
            if let Some(state) = self.state.clone()
                && state.read(cx).value().as_ref() != pending
            {
                self.suppress_change = true;
                state.update(cx, |state, cx| state.set_value(pending, window, cx));
            }
            self.pending_replace = false;
        }
    }

    fn on_inner_change(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let text = state.read(cx).value().to_string();
        self.text = text.clone();
        if self.suppress_change {
            self.suppress_change = false;
            return;
        }
        if let Some(callback) = self.on_change.clone() {
            callback(&text, cx);
        }
        cx.notify();
    }
}

impl Render for SqlEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.flush(window, cx);
        let font_size = self.options.font_size;
        let family = self.options.font_family.clone();
        let readonly = self.options.readonly;
        let Some(state) = self.state.clone() else {
            return div().into_any_element();
        };
        let editor = Editor::new(&state)
            .font_family(family)
            .text_size(px(font_size))
            .bordered(false)
            .readonly(readonly)
            .context_menu(|menu: NativeMenu, _window, _cx| sql_editor_context_menu(menu))
            // gpui-kit forces multi-line editors to `height: auto` (its `Input::render` applies
            // `h_auto()` after `size_full()`), so an unstyled surface grows with its text and
            // spills over whatever sits below (the query page's result panel), while a measured
            // pin goes stale as soon as the surrounding split changes. The frame is a `flex_col`
            // whose own height comes from the enclosing flex chain, so laying the editor out with
            // basis 0 + grow (and an overridable minimum) gives it exactly the free space: long
            // documents scroll inside the editor instead of overflowing the frame.
            .flex_1()
            .min_h(px(0.0));
        div()
            .id("sql-editor-frame")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(editor)
            .test_support()
            .into_any_element()
    }
}

/// The SQL editor's right-click menu: the app's 运行已选择的 first, then the standard edit items.
///
/// `Editor::context_menu` replaces the built-in menu, so the standard items are rebuilt here from
/// the same input actions the built-in menu uses.
fn sql_editor_context_menu(menu: NativeMenu) -> NativeMenu {
    menu.menu(
        t!("query.run_selected").to_string(),
        Box::new(crate::app::RunSelectedQuery),
    )
    .separator()
    .menu(t!("query.undo").to_string(), Box::new(Undo))
    .menu(t!("query.redo").to_string(), Box::new(Redo))
    .separator()
    .menu(t!("query.cut").to_string(), Box::new(Cut))
    .menu(t!("query.copy").to_string(), Box::new(Copy))
    .menu(t!("query.paste").to_string(), Box::new(Paste))
    .separator()
    .menu(t!("query.select_all").to_string(), Box::new(SelectAll))
}

#[cfg(test)]
mod tests {
    use super::*;

    use gpui::{Bounds, Point, TestAppContext, WindowBounds, WindowOptions, size};
    use gpui_kit::test::TestWindowExt as _;

    /// The query page's shape: a fixed toolbar strip, the editor filling the rest, and — once a
    /// query has run — a fixed-height result panel at the bottom.
    struct QueryPageHarness {
        editor: Entity<SqlEditor>,
        result_panel: bool,
    }

    impl Render for QueryPageHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let mut page = div()
                .flex()
                .flex_col()
                .size_full()
                .overflow_hidden()
                .child(div().h(px(52.0)).flex_none());
            page = page.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(self.editor.clone()),
            );
            if self.result_panel {
                page = page.child(div().h(px(280.0)).flex_none());
            }
            page
        }
    }

    /// The rendered height of the editor's frame (the element that must not overflow its slot).
    fn editor_frame_height(handle: gpui::AnyWindowHandle, cx: &mut TestAppContext) -> Option<f32> {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            Some(f32::from(
                window.find("sql-editor-frame").bounds().size.height,
            ))
        })
        .unwrap()
    }

    #[gpui_kit::test]
    fn editor_fills_its_slot_and_shrinks_when_the_result_panel_opens(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (handle, harness) = cx.update(|cx| {
            let harness = cx.new(|cx| QueryPageHarness {
                editor: cx.new(SqlEditor::new),
                result_panel: false,
            });
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
            (handle, harness)
        });

        // A long document, so a content-sized surface would overflow the slot many times over.
        let sql = "SELECT *\nFROM some_table\n".repeat(40);
        harness.update(cx, |harness, cx| {
            harness
                .editor
                .update(cx, |editor, cx| editor.set_text(sql, cx));
        });

        // Without the result panel the editor owns everything below the toolbar strip.
        let full = editor_frame_height(handle, cx).expect("editor frame measured");
        assert!(
            (full - 548.0).abs() < 2.0,
            "editor should fill the pane below the 52px strip, measured {full}"
        );

        // Opening the result panel must shrink the editor to the remaining space — the old
        // measured-pin kept the stale full height and the editor spilled over the panel.
        harness.update(cx, |harness, _| harness.result_panel = true);
        let split = editor_frame_height(handle, cx).expect("editor frame measured");
        assert!(
            (split - 268.0).abs() < 2.0,
            "editor should shrink above the 280px result panel, measured {split}"
        );
    }
}
