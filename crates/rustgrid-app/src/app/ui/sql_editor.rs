//! `SqlEditor`: the app-facing SQL editor, wrapping gpui-kit's `Editor`/`EditorState`.
//!
//! gpui-kit's editor owns the text, selection, undo/redo, multi-cursor and search; this wrapper
//! keeps feature code off `EditorState` directly. SQL is coloured by RustGrid's own lexer
//! ([`crate::app::sql_lex`]) rather than the tree-sitter grammar, so `#`, `//`, `--x` and an
//! unclosed `/*` look commented out exactly when they will not run. Like `ui::TextInput`
//! / `ui::ComboBox`, the inner state needs a `&mut Window`, so it is built on the first render and
//! pending mutations are flushed there.

use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui::{
    App, AppContext, Context, DispatchPhase, Entity, HitboxBehavior, InteractiveElement,
    IntoElement, ParentElement, Render, ScrollDelta, ScrollEvent, SharedString, Styled,
    Subscription, Window, canvas, div, px,
};
use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{
    Copy, Cut, Editor, EditorState, Enter, InputEvent, Paste, SelectAll, TextDecoration,
    TextDecorationCollection, Undo,
};
use gpui_kit::component::native_menu::NativeMenu;

use crate::app::sql_completion::CompletionScope;
use crate::app::sql_lex::TokenKind;
use rustgrid_core::DriverDialect;

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
            font_size: 14.0,
            line_number: true,
            readonly: false,
        }
    }
}

/// Called with the whole new text whenever the editor's content changes.
pub(crate) type SqlEditorChange = Rc<dyn Fn(&str, &mut App)>;

/// Called when the pointer scrolls over the editor with Ctrl/⌘ held, with the wheel's direction
/// (`> 0` zooms in). The app changes the font-size setting and pushes it back through
/// [`SqlEditor::set_font`]; the returned flag says whether the gesture was consumed (the setting
/// can be off, or the size already at its end), so an unconsumed wheel still scrolls the editor.
pub(crate) type SqlEditorZoom = Rc<dyn Fn(f32, &mut App) -> bool>;

pub(crate) struct SqlEditor {
    text: String,
    options: SqlEditorOptions,
    /// 自动换行: applied to the inner state (whose own default is also on).
    word_wrap: bool,
    on_change: Option<SqlEditorChange>,
    /// Ctrl + wheel zoom, when the app wires it up (Options → 编辑器, on by default).
    on_zoom: Option<SqlEditorZoom>,
    /// The provider, or a factory that builds it with the inner state's context.
    provider: Option<Rc<dyn gpui_kit::component::input::CompletionProvider>>,
    /// The provider's live connection/database/schema scope, so re-scoping does not need to
    /// rebuild the editor.
    scope: Option<Arc<RwLock<CompletionScope>>>,
    state: Option<Entity<EditorState>>,
    /// The SQL colouring (keywords, strings, numbers, comments), from [`crate::app::sql_lex`].
    highlights: Option<TextDecorationCollection>,
    /// MySQL / MariaDB lexing rules (`#` comments, backslash escapes), from the tab's driver.
    mysql: bool,
    /// Caller-provided colouring (the query 信息 log), applied once the inner state exists.
    custom: Option<TextDecorationCollection>,
    pending_custom: Option<Vec<TextDecoration>>,
    /// Take the keyboard focus as soon as the inner editor is built (see `focus_when_ready`).
    focus_on_build: bool,
    /// The theme colours the highlights were painted with, to repaint after a theme change.
    painted_with: Option<[Option<gpui::HighlightStyle>; 5]>,
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
            word_wrap: true,
            on_change: None,
            on_zoom: None,
            provider: None,
            scope: None,
            state: None,
            highlights: None,
            focus_on_build: false,
            custom: None,
            pending_custom: None,
            mysql: false,
            painted_with: None,
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

    /// Handle Ctrl + mouse wheel over the editor (resizing the font) with `callback`.
    pub(crate) fn on_zoom(mut self, callback: SqlEditorZoom) -> Self {
        self.on_zoom = Some(callback);
        self
    }

    /// Change the font without touching the inner state: the render applies the wrapped options
    /// every frame, so this is enough (and needs no window) — the Ctrl + wheel zoom path.
    pub(crate) fn set_font(
        &mut self,
        font_family: SharedString,
        font_size: f32,
        cx: &mut Context<Self>,
    ) {
        self.options.font_family = font_family;
        self.options.font_size = font_size;
        cx.notify();
    }

    /// Apply the 编辑器 display settings to an already-built editor: the font (per frame) plus the
    /// line-number and word-wrap switches, which live on the inner state and need a window.
    pub(crate) fn set_display_options(
        &mut self,
        font_family: SharedString,
        font_size: f32,
        line_number: bool,
        word_wrap: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.options.line_number = line_number;
        self.word_wrap = word_wrap;
        self.set_font(font_family, font_size, cx);
        if let Some(state) = self.state.clone() {
            state.update(cx, |state, cx| {
                state.set_line_number(line_number, window, cx);
                state.set_soft_wrap(word_wrap, window, cx);
            });
        }
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_scope(
        &mut self,
        connection_index: Option<usize>,
        database: Option<String>,
        schema: Option<String>,
        supports_schemas: bool,
        dialect: DriverDialect,
        key_value: bool,
        documents: bool,
        cx: &mut Context<Self>,
    ) {
        let mysql = dialect == DriverDialect::Mysql;
        let language = if key_value {
            "redis"
        } else if documents {
            "json"
        } else {
            "sql"
        };
        let language_changed = self.options.language.as_ref() != language;
        if language_changed {
            self.options.language = language.into();
        }
        if mysql != self.mysql || language_changed {
            self.mysql = mysql;
            self.refresh_highlights(cx);
        }
        if let Some(scope) = &self.scope
            && let Ok(mut scope) = scope.write()
        {
            scope.connection_index = connection_index;
            scope.database = database;
            scope.schema = schema;
            scope.supports_schemas = supports_schemas;
            scope.dialect = dialect;
            scope.key_value = key_value;
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The editor's text right now (the selection offsets index into exactly this).
    pub(crate) fn live_text(&self, cx: &App) -> String {
        match self.state.as_ref() {
            Some(state) => state.read(cx).value().to_string(),
            None => self.text.clone(),
        }
    }

    /// The active selection as byte offsets (`start == end` when nothing is selected).
    pub(crate) fn selected_range(&self, cx: &App) -> (usize, usize) {
        let Some(state) = self.state.as_ref() else {
            return (0, 0);
        };
        let range = state.read(cx).selected_range();
        (range.start, range.end)
    }

    /// Apply a line edit (comment toggle, duplicate, move, swap) computed from the current text and
    /// selection, as one undoable replacement, then restore the edit's selection. `edit` returning
    /// `None` (e.g. moving the first line up) leaves the document alone.
    pub(crate) fn edit_lines(
        &mut self,
        edit: impl FnOnce(&str, Range<usize>) -> Option<crate::app::line_edit::LineEdit>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let (text, selection) = {
            let state = state.read(cx);
            (state.value().to_string(), state.selected_range())
        };
        let Some(edit) = edit(&text, selection) else {
            return;
        };
        state.update(cx, |state, cx| {
            state.set_selected_range(edit.range.clone(), cx);
            state.replace(edit.text.clone(), window, cx);
            state.set_selected_range(edit.selection.clone(), cx);
            // Keep typing where you were (the caret stays visible).
            state.focus(window, cx);
        });
    }

    /// Colour ranges of the text (replacing the previous ones), e.g. the 信息 log's errors.
    pub(crate) fn set_decorations(
        &mut self,
        decorations: Vec<(Range<usize>, gpui::HighlightStyle)>,
        cx: &mut Context<Self>,
    ) {
        self.pending_custom = Some(
            decorations
                .into_iter()
                .map(|(range, style)| TextDecoration::new(range, style))
                .collect(),
        );
        cx.notify();
    }

    /// Focus the editor now, or the moment its inner state is built (on its first render), so
    /// keys typed right after opening it never reach the view behind it.
    pub(crate) fn focus_when_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = self.state.clone() {
            state.update(cx, |state, cx| state.focus(window, cx));
        } else {
            self.focus_on_build = true;
            cx.notify();
        }
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) -> bool {
        let Some(state) = self.state.clone() else {
            return false;
        };
        state.update(cx, |state, cx| state.focus(window, cx));
        true
    }

    /// Put a cursor on every occurrence of the selected text (or of the word at the caret when
    /// nothing is selected), leaving the first occurrence as the active selection.
    ///
    /// This is the programmatic multi-cursor gpui-kit 0.7.3 added: `set_selected_range` replaces
    /// the selection set and `add_selection` appends each further occurrence. Bound to Ctrl+Shift+L.
    pub(crate) fn select_all_occurrences(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let (haystack, needle) = {
            let state = state.read(cx);
            let text = state.value().to_string();
            let range = state.selected_range();
            let needle = if range.start == range.end {
                // Nothing selected: use the identifier under the caret. `selected_range` collapses
                // to the caret offset when empty, so `range.end` is the cursor position.
                word_bounds(&text, range.end)
                    .map(|(start, end)| text[start..end].to_string())
                    .unwrap_or_default()
            } else {
                text.get(range).unwrap_or_default().to_string()
            };
            (text, needle)
        };
        if needle.is_empty() {
            return;
        }
        let ranges = find_all_occurrences(&haystack, &needle);
        if ranges.len() < 2 {
            return;
        }
        state.update(cx, |state, cx| {
            state.set_selected_range(ranges[0].clone(), cx);
            for range in ranges.iter().skip(1) {
                state.add_selection(range.clone(), cx);
            }
        });
    }

    /// Confirm the highlighted completion item — exactly what Enter does — so the editor can bind
    /// Tab to it as well. Returns whether the editor consumed the keystroke: `false` means neither
    /// the completion nor the code-action menu was open, so the caller must let it fall through to
    /// the editor's own Tab binding (indent).
    pub(crate) fn accept_completion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.state.clone() else {
            return false;
        };
        state.update(cx, |state, cx| {
            state.route_overlay_action(
                Box::new(Enter {
                    secondary: false,
                    shift: false,
                }),
                window,
                cx,
            )
        })
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
            let word_wrap = self.word_wrap;
            let state = cx.new(|cx| {
                let language = match options.language.as_ref() {
                    "sql" => plain_sql_language(),
                    // Redis has no tree-sitter grammar in the kit; its colours come from the
                    // app's own scanner below, so keep the parser out of the way.
                    "redis" => "text".into(),
                    _ => options.language.clone(),
                };
                let mut state = EditorState::new(window, cx)
                    .language(language)
                    .line_number(options.line_number)
                    // The fold gutter is only useful beside line numbers; with them off it just
                    // leaves a blank strip before the text (the Redis RAW/JSON views).
                    .folding(options.line_number);
                // The kit's own default is on; the setting is applied explicitly so a wrap choice
                // made in the Options window is what the editor actually uses.
                state.set_soft_wrap(word_wrap, window, cx);
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
            if matches!(options.language.as_ref(), "sql" | "redis") {
                self.highlights = Some(state.update(cx, |state, cx| {
                    state.create_decorations_collection(Vec::new(), cx)
                }));
            }
            if std::mem::take(&mut self.focus_on_build) {
                state.update(cx, |state, cx| state.focus(window, cx));
            }
            self.state = Some(state);
            self.refresh_highlights(cx);
        }
        if let Some(pending) = self.pending_text.take() {
            if let Some(state) = self.state.clone()
                && state.read(cx).value().as_ref() != pending
            {
                self.suppress_change = true;
                state.update(cx, |state, cx| state.set_value(pending, window, cx));
            }
            self.pending_replace = false;
            self.refresh_highlights(cx);
        }
        if let (Some(state), Some(decorations)) = (self.state.clone(), self.pending_custom.take()) {
            match self.custom.as_ref() {
                Some(collection) => collection.set(decorations, cx),
                None => {
                    self.custom = Some(state.update(cx, |state, cx| {
                        state.create_decorations_collection(decorations, cx)
                    }));
                }
            }
        }
        // A theme switch changes the colours: repaint.
        if self.highlights.is_some() && self.painted_with != Some(highlight_palette(cx)) {
            self.refresh_highlights(cx);
        }
    }

    /// Recolour the text (after an edit, a dialect change or a theme switch).
    fn refresh_highlights(&mut self, cx: &mut Context<Self>) {
        let (Some(state), Some(collection)) = (self.state.as_ref(), self.highlights.as_ref())
        else {
            return;
        };
        let palette = highlight_palette(cx);
        self.painted_with = Some(palette);
        let [keyword, function, string, number, comment] = palette;
        let text = state.read(cx).value().to_string();
        let decorations = if text.len() > MAX_HIGHLIGHT_BYTES {
            Vec::new()
        } else if self.options.language.as_ref() == "redis" {
            crate::app::redis_lex::tokens(&text)
                .into_iter()
                .filter_map(|(range, kind)| {
                    let style = match kind {
                        TokenKind::Keyword => keyword,
                        TokenKind::Function => function,
                        TokenKind::String => string,
                        TokenKind::Number => number,
                        TokenKind::Comment => comment,
                    }?;
                    Some(TextDecoration::new(range, style))
                })
                .collect()
        } else {
            crate::app::sql_lex::tokens(&text, self.mysql)
                .into_iter()
                .filter_map(|(range, kind)| {
                    let style = match kind {
                        TokenKind::Keyword => keyword,
                        TokenKind::Function => function,
                        TokenKind::String => string,
                        TokenKind::Number => number,
                        TokenKind::Comment => comment,
                    }?;
                    Some(TextDecoration::new(range, style))
                })
                .collect()
        };
        collection.set(decorations, cx);
    }

    fn on_inner_change(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let text = state.read(cx).value().to_string();
        self.text = text.clone();
        self.refresh_highlights(cx);
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
            .font_family(family.clone())
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
        let mut frame = div()
            .id("sql-editor-frame")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .overflow_hidden()
            .child(editor);
        if let Some(zoom) = self.on_zoom.clone() {
            // Ctrl/⌘ + wheel resizes the font. A plain wheel must still scroll the editor, and
            // scrollable containers (the editor's own viewport) handle the wheel in the *bubble*
            // phase — so the gesture is claimed here, in the capture phase, over a hitbox that
            // covers the editor, exactly like the table grid's wheel handling.
            frame = frame.child(
                canvas(
                    |bounds, window, _cx| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_bounds, hitbox, window, _cx| {
                        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture
                                || !hitbox.should_handle_scroll(window)
                            {
                                return;
                            }
                            if !(event.modifiers.control || event.modifiers.platform) {
                                return;
                            }
                            let dy = match event.delta {
                                ScrollDelta::Lines(delta) => delta.y,
                                ScrollDelta::Pixels(delta) => f32::from(delta.y),
                            };
                            if dy == 0.0 {
                                return;
                            }
                            // Scroll up (positive line delta) zooms in, matching the platform
                            // convention; the previous sign made Ctrl+wheel feel reversed.
                            if zoom(if dy > 0.0 { 1.0 } else { -1.0 }, cx) {
                                cx.stop_propagation();
                            }
                        });
                    },
                )
                .absolute()
                .inset_0(),
            );
        }
        frame.test_support().into_any_element()
    }
}

/// Above this size the text is left uncoloured (lexing runs on every edit).
const MAX_HIGHLIGHT_BYTES: usize = 4 << 20;

/// The editor language SQL editors use: the SQL grammar (for bracket and string awareness) with
/// its highlight queries removed, since [`crate::app::sql_lex`] colours the text. Registered once.
fn plain_sql_language() -> SharedString {
    const NAME: &str = "rustgrid-sql";
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        let registry = gpui_kit::component::highlighter::LanguageRegistry::singleton();
        if let Some(mut config) = registry.language("sql") {
            config.name = NAME.into();
            config.highlights = SharedString::default();
            config.injections = SharedString::default();
            config.locals = SharedString::default();
            config.injection_languages = Vec::new();
            registry.register(NAME, &config);
        }
    });
    NAME.into()
}

/// The theme's keyword, function, string, number and comment styles.
fn highlight_palette(cx: &App) -> [Option<gpui::HighlightStyle>; 5] {
    let theme = &gpui_kit::component::Theme::global(cx).highlight_theme;
    ["keyword", "function", "string", "number", "comment"].map(|name| theme.style(name))
}

/// The SQL editor's right-click menu: the app's 运行已选择的 first, then the standard edit items.
/// 运行 and 重做 are deliberately left out — the full run is the toolbar's/`Ctrl+Enter`'s job and
/// redo is available from the keyboard (its menu entry only cluttered the short menu).
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
    .separator()
    .menu(t!("query.cut").to_string(), Box::new(Cut))
    .menu(t!("query.copy").to_string(), Box::new(Copy))
    .menu(t!("query.paste").to_string(), Box::new(Paste))
    .separator()
    .menu(t!("query.select_all").to_string(), Box::new(SelectAll))
}

/// The ASCII identifier-ish run around `cursor` (letters, digits and `_`), as a byte range, or
/// `None` when the caret is not on or immediately after such a character. Used to pick the word for
/// [`SqlEditor::select_all_occurrences`] when nothing is selected.
fn word_bounds(text: &str, cursor: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let cursor = cursor.min(bytes.len());
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    // Accept the character under the caret, or the one just before it (the common "caret sits at
    // the end of the word just typed" case).
    let anchor = if cursor < bytes.len() && is_word(bytes[cursor]) {
        cursor
    } else if cursor > 0 && is_word(bytes[cursor - 1]) {
        cursor - 1
    } else {
        return None;
    };
    let mut start = anchor;
    while start > 0 && is_word(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = anchor + 1;
    while end < bytes.len() && is_word(bytes[end]) {
        end += 1;
    }
    Some((start, end))
}

/// Every non-overlapping occurrence of `needle` in `haystack`, in document order.
fn find_all_occurrences(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    if needle.is_empty() {
        return ranges;
    }
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        ranges.push(start..end);
        from = end;
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_json_view_uses_the_compiled_json_grammar() {
        struct Resolver;
        impl gpui_kit::base::input::HighlightStyleResolver for Resolver {
            fn style(&self, _: &str) -> Option<gpui::HighlightStyle> {
                Some(gpui::HighlightStyle::default())
            }
        }

        let mut highlighter = gpui_kit::component::highlighter::SyntaxHighlighter::new("json");
        assert_eq!(highlighter.language().as_ref(), "json");
        let text = ropey::Rope::from_str("{\"key\": 1}");
        highlighter.update(None, &text, Some(std::time::Duration::from_secs(2)));
        assert!(
            !highlighter.styles(&(0..text.len()), &Resolver).is_empty(),
            "the key page's JSON view should have syntax colours"
        );
    }

    #[test]
    fn find_all_occurrences_is_non_overlapping_and_ordered() {
        assert_eq!(find_all_occurrences("a_a_a", "a"), vec![0..1, 2..3, 4..5]);
        assert_eq!(find_all_occurrences("aaaa", "aa"), vec![0..2, 2..4]);
        assert!(find_all_occurrences("abc", "").is_empty());
    }

    #[test]
    fn word_bounds_reads_the_identifier_under_or_before_the_caret() {
        let text = "SELECT user_id FROM t";
        assert_eq!(word_bounds(text, 10), Some((7, 14)));
        assert_eq!(word_bounds(text, 3), Some((0, 6)));
        // Caret in the run of spaces between the keywords selects nothing.
        assert_eq!(word_bounds("SELECT  user_id", 7), None);
        // End of a word includes the character before the caret.
        assert_eq!(word_bounds(text, 6), Some((0, 6)));
    }

    use gpui::{
        Bounds, Modifiers, MouseButton, Point, TestAppContext, VisualTestContext, WindowBounds,
        WindowOptions, point, size,
    };
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

    /// The app's read-only SQL 预览 panes (the table designer's, the user window's, the privilege
    /// manager's) are this editor: a mouse drag must select the script and Ctrl+C must copy it. A
    /// plain text run (`SelectableText`) selects but never copies, because the window's Ctrl+C is
    /// dispatched along the focused element's path and nothing inside the run holds focus.
    struct PreviewHarness {
        editor: Entity<SqlEditor>,
    }

    impl Render for PreviewHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().p_2().child(self.editor.clone())
        }
    }

    #[gpui_kit::test]
    fn readonly_preview_selects_and_copies(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let sql = "CREATE TABLE `db`.`t` (\n  id bigint NOT NULL\n);";
        let (handle, harness) = cx.update(|cx| {
            let harness = cx.new(|cx| PreviewHarness {
                editor: cx.new(|cx| {
                    SqlEditor::new(cx).options(SqlEditorOptions {
                        language: "sql".into(),
                        font_family: "Consolas".into(),
                        font_size: 12.0,
                        line_number: false,
                        readonly: true,
                    })
                }),
            });
            harness.update(cx, |harness, cx| {
                harness
                    .editor
                    .update(cx, |editor, cx| editor.set_text(sql, cx));
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

        let mut cx = VisualTestContext::from_window(handle, cx);
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_mouse_down(
            point(px(10.), px(24.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(260.), px(24.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(px(260.), px(24.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        let selected = cx.update(|window, cx| {
            let _ = window.draw(cx);
            let editor = harness.read(cx).editor.clone();
            let editor = editor.read(cx);
            let (start, end) = editor.selected_range(cx);
            editor.text()[start..end].to_string()
        });
        assert!(
            selected.contains("CREATE TABLE"),
            "a drag over the preview should select the script, selected {selected:?}"
        );
        cx.simulate_keystrokes("ctrl-c");
        let copied = cx
            .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .unwrap_or_default();
        assert_eq!(copied, selected, "Ctrl+C should copy the selected script");
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
