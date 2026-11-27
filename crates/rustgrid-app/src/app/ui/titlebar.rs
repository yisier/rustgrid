//! Shared window chrome built on gpui-kit's `TitleBar`.
//!
//! Every app window draws its own title bar (the OS one is suppressed). Using the kit's
//! `TitleBar` keeps the platform behavior in one place: `WindowControlArea`-based window controls
//! on Windows (which is what enables Snap Layouts and the correct maximize/restore state), native
//! traffic lights on macOS, and the window-manager controls on Linux. This module supplies the
//! app's palette, logo and title on top of it.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    App, ClickEvent, ImageSource, IntoElement, MouseButton, Resource, SharedString,
    TitlebarOptions, Window, WindowOptions, div, img, px, rgb,
};
use gpui_kit::component::TitleBar;

use crate::theme::Theme;

/// A title bar's optional close handler (Linux only; Windows and macOS use the native close path).
type CloseHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The options shared by every app-owned window title bar.
///
/// `TitleBar::window_options` sets `app_owns_titlebar_drag`, which macOS needs so AppKit neither
/// drags the window nor delays title-bar clicks while disambiguating double clicks on top of the
/// bar's own handling.
pub(crate) fn window_options(title: impl Into<SharedString>) -> WindowOptions {
    let mut options = TitleBar::window_options();
    options.app_id = Some(crate::app::WINDOW_APP_ID.to_string());
    options.titlebar = Some(TitlebarOptions {
        title: Some(title.into()),
        ..TitleBar::title_bar_options()
    });
    options
}

/// The shared title bar: the app palette replaces the kit's default gradient.
pub(crate) fn title_bar(theme: Theme) -> TitleBar {
    TitleBar::new()
        .bg(rgb(theme.titlebar_bg))
        .border_color(rgb(theme.border))
        .text_color(rgb(theme.text))
}

/// The leading content of a title bar: the app logo and its title.
pub(crate) fn bar_title(text: impl Into<SharedString>, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .h_full()
        .child(
            img(ImageSource::Resource(Resource::Embedded("logo.png".into())))
                .w(px(16.0))
                .h(px(16.0))
                .flex_none(),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(theme.text))
                .child(text.into()),
        )
}

/// The main window's action cluster.
///
/// The kit's bar marks its whole row as the drag region, so the cluster occludes that hitbox;
/// otherwise Windows hit-tests the buttons as `HTCAPTION` and they never receive a click.
pub(crate) fn action_cluster(actions: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .h_full()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(actions)
}

/// A child window's title bar. `on_close` runs on Linux, where the kit's close button is
/// app-handled; Windows and macOS close through `WM_CLOSE` / the native button and use
/// `Window::on_window_should_close` for a guard.
pub(crate) fn window_title_bar(
    text: impl Into<SharedString>,
    theme: Theme,
    on_close: Option<CloseHandler>,
) -> impl IntoElement {
    let bar = title_bar(theme).child(bar_title(text, theme));
    match on_close {
        Some(on_close) => bar.on_close_window(move |event, window, cx| on_close(event, window, cx)),
        None => bar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_options_keep_the_title_and_app_owned_drag() {
        let options = window_options("Test Window");
        assert_eq!(options.app_id.as_deref(), Some(crate::app::WINDOW_APP_ID));
        assert!(options.app_owns_titlebar_drag);
        let titlebar = options.titlebar.expect("every window declares a titlebar");
        assert_eq!(
            titlebar.title.map(|title| title.to_string()),
            Some("Test Window".to_string())
        );
    }
}
