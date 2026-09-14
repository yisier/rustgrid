//! Internal design-system primitives.
//!
//! Everything user-facing that is not a one-off view belongs here so the classic
//! Win32/Navicat look stays consistent and a future UI-library swap only has to touch this
//! module. The rules (square corners, `Theme` colors, `ButtonKind` variants) are described in
//! `AGENTS.md` under "UI conventions".

use gpui::{
    App, BoxShadow, ClickEvent, Div, FontWeight, IntoElement, Point, SharedString, Stateful,
    Window, div, prelude::*, px, rgb, rgba, svg,
};

use crate::theme::Theme;

/// The variant of a push button. See `win_button` / `dialog_button`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ButtonKind {
    Normal,
    Default,
    Selected,
    Disabled,
}

/// The `(background, border, text)` colors for a button of `kind`.
pub(super) fn button_colors(kind: ButtonKind, theme: Theme) -> (u32, u32, u32) {
    match kind {
        ButtonKind::Normal => (theme.button_bg, theme.button_border, theme.text),
        ButtonKind::Default => (theme.button_bg, theme.button_default_border, theme.text),
        ButtonKind::Selected => (
            theme.tree_selected_bg,
            theme.button_default_border,
            theme.tree_selected_text,
        ),
        ButtonKind::Disabled => (theme.button_bg, theme.button_border, theme.text_muted),
    }
}

/// A square push button, the single source of truth for button chrome.
pub(super) fn button(
    id: impl Into<SharedString>,
    label: String,
    kind: ButtonKind,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let (background, border, text) = button_colors(kind, theme);
    let enabled = kind != ButtonKind::Disabled;

    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .px_3()
        .h(px(24.0))
        .text_size(px(12.0))
        .cursor_pointer()
        .bg(rgb(background))
        .text_color(rgb(text))
        .border_1()
        .border_color(rgb(border))
        .when(enabled, move |style| {
            style.hover(move |style| {
                style
                    .bg(rgb(theme.button_hover_bg))
                    .border_color(rgb(theme.button_default_border))
            })
        })
        .on_click(on_click)
        .child(label)
}

/// A dialog push button; `primary` selects the default (accent-bordered) variant.
pub(super) fn dialog_button(
    id: impl Into<SharedString>,
    label: String,
    primary: bool,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let kind = if primary {
        ButtonKind::Default
    } else {
        ButtonKind::Normal
    };
    button(id, label, kind, theme, on_click)
}

/// The square red-hover close button used by every dialog titlebar. The red is a legacy
/// one-off, kept here so it is at least shared.
pub(super) fn dialog_close_button(
    id: &'static str,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(38.0))
        .h_full()
        .flex_none()
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(rgb(theme.text))
        .hover(|style| style.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff)))
        .on_click(on_click)
        .child("✕")
}

/// A borderless icon+label item used by the main and object toolbars (flat, not a push
/// button).
pub(super) fn toolbar_item(
    id: impl Into<SharedString>,
    icon: &'static str,
    label: String,
    enabled: bool,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let color = if enabled {
        theme.text
    } else {
        theme.text_muted
    };

    div()
        .id(id.into())
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .h(px(24.0))
        .rounded_sm()
        .text_size(px(12.0))
        .text_color(rgb(color))
        .when(enabled, move |style| {
            style
                .cursor_pointer()
                .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
        })
        .on_click(on_click)
        .child(
            svg()
                .path(icon)
                .w(px(16.0))
                .h(px(16.0))
                .flex_none()
                .text_color(rgb(color)),
        )
        .child(label)
}

/// A dialog tab: active merges with the page, inactive is a raised button face.
pub(super) fn form_tab(label: String, active: bool, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .px_4()
        .h(px(24.0))
        .text_size(px(12.0))
        .when(active, move |style| {
            style
                .bg(rgb(theme.dialog_bg))
                .border_t_1()
                .border_l_1()
                .border_r_1()
                .border_color(rgb(theme.border))
                .text_color(rgb(theme.text))
                .font_weight(FontWeight::SEMIBOLD)
                .mb(px(-1.0))
        })
        .when(!active, move |style| {
            style
                .bg(rgb(theme.button_bg))
                .border_1()
                .border_color(rgb(theme.border))
                .text_color(rgb(theme.text_muted))
        })
        .child(label)
}

fn separator(theme: Theme, height: f32) -> impl IntoElement {
    div()
        .w(px(1.0))
        .h(px(height))
        .flex_none()
        .mx_1()
        .bg(rgb(theme.border))
}

/// A thin 16px separator between toolbar items.
pub(super) fn toolbar_separator(theme: Theme) -> impl IntoElement {
    separator(theme, 16.0)
}

/// A taller 36px separator between main toolbar groups.
pub(super) fn main_separator(theme: Theme) -> impl IntoElement {
    separator(theme, 36.0)
}

/// A 14px check box glyph: filled primary when checked, empty input face otherwise.
pub(super) fn checkbox_box(checked: bool, theme: Theme) -> impl IntoElement {
    div()
        .w(px(14.0))
        .h(px(14.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(if checked {
            theme.primary
        } else {
            theme.input_bg
        }))
        .text_color(rgb(0xffffff))
        .child(if checked { "✓" } else { "" }.to_string())
}

/// The soft drop shadow behind every floating dialog.
pub(super) fn dialog_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: rgba(0x00000040).into(),
        offset: Point {
            x: px(0.0),
            y: px(2.0),
        },
        blur_radius: px(8.0),
        spread_radius: px(0.0),
    }]
}

/// The full-window modal scrim that centers a dialog frame. Dialogs are app-drawn overlays,
/// never native windows (see `AGENTS.md`).
pub(super) fn overlay(theme: Theme) -> Div {
    div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(theme.overlay))
}

/// The standard square floating dialog frame: `face` background, 1px neutral border, and the
/// shared drop shadow. Callers add width/offset and children.
pub(super) fn dialog_frame(theme: Theme, face: u32) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .bg(rgb(face))
        .border_1()
        .border_color(rgb(theme.neutral))
        .shadow(dialog_shadow())
}

/// The 28px titlebar of the compact message dialogs: dialog-face background with a bottom
/// border. Callers add padding, the drag handler and children.
pub(super) fn dialog_titlebar(theme: Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(28.0))
        .flex_none()
        .bg(rgb(theme.dialog_face))
        .border_b_1()
        .border_color(rgb(theme.border))
}

/// The 32px header of the large tabbed dialogs: dialog background, no bottom border, so it
/// merges with the tab strip below. Callers add padding, the drag handler and children.
pub(super) fn dialog_header(theme: Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(32.0))
        .bg(rgb(theme.dialog_bg))
}

/// The neutral input face shared by text fields, combos and dropdowns. Callers add layout,
/// sizing, focus and children.
pub(super) fn text_field(theme: Theme) -> Div {
    div()
        .bg(rgb(theme.input_bg))
        .border_1()
        .border_color(rgb(theme.border))
}

/// Thumb length and its travel range for a scrollbar track of `viewport` px showing
/// `viewport + max` px of content.
pub(super) fn scrollbar_thumb(viewport: f32, max: f32) -> (f32, f32) {
    if viewport <= 0.0 || max <= 0.0 {
        return (24.0, 0.0);
    }
    let content = (viewport + max).max(1.0);
    let thumb = (viewport * viewport / content).clamp(24.0, viewport.max(24.0));
    (thumb, (viewport - thumb).max(0.0))
}

/// A vertical scrollbar track (14px wide) with its thumb already placed at `thumb_y`.
/// gpui 0.2 does not paint scrollbars for `overflow_*`, so every scrollable pane draws its
/// own; callers attach the drag handler.
pub(super) fn vscrollbar_track(
    id: &'static str,
    theme: Theme,
    thumb_y: f32,
    thumb_h: f32,
) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(14.0))
        .h_full()
        .bg(rgb(theme.toolbar_bg))
        .border_l_1()
        .border_color(rgb(theme.border))
        .cursor_pointer()
        .child(
            div()
                .absolute()
                .left(px(1.0))
                .top(px(thumb_y))
                .w(px(12.0))
                .h(px(thumb_h))
                .bg(rgb(theme.button_border)),
        )
}

/// A horizontal scrollbar track (14px tall) with its thumb already placed at `thumb_x`.
pub(super) fn hscrollbar_track(
    id: &'static str,
    theme: Theme,
    thumb_x: f32,
    thumb_w: f32,
) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .flex_none()
        .w_full()
        .h(px(14.0))
        .bg(rgb(theme.toolbar_bg))
        .border_t_1()
        .border_color(rgb(theme.border))
        .cursor_pointer()
        .child(
            div()
                .absolute()
                .left(px(thumb_x))
                .top(px(1.0))
                .w(px(thumb_w))
                .h(px(12.0))
                .bg(rgb(theme.button_border)),
        )
}
