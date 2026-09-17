use super::*;

/// The window's top-level view: [`AppView`] plus the gpui-kit `Root` overlay layers.
///
/// gpui-kit's `Root::render` does not mount its own dialog/sheet layers, so unless a view renders
/// them, `window.open_dialog` (and friends) silently do nothing. They are mounted here, by a
/// separate view, rather than inside `AppView`: a dialog builder re-enters `AppView` through
/// `Entity::update` to rebuild its body, which would double-borrow the entity during `AppView`'s
/// own render.
pub struct AppShell {
    view: Entity<AppView>,
}

impl AppShell {
    pub fn new(view: Entity<AppView>) -> Self {
        Self { view }
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sheet_layer = gpui_kit::component::Root::render_sheet_layer(window, cx);
        let dialog_layer = gpui_kit::component::Root::render_dialog_layer(window, cx);
        div()
            .size_full()
            .child(self.view.clone())
            .children(sheet_layer)
            .children(dialog_layer)
    }
}
