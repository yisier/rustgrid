//! Internal design-system primitives.
//!
//! Everything user-facing that is not a one-off view belongs here so the classic
//! Win32/Navicat look stays consistent and a future UI-library swap only has to touch this
//! module. The rules (square corners, `Theme` colors, `ButtonKind` variants) are described in
//! `AGENTS.md` under "UI conventions".

use gpui::{
    App, BoxShadow, ClickEvent, CursorStyle, Div, FontWeight, IntoElement, Point, SharedString,
    Stateful, Window, div, prelude::*, px, relative, rgb, rgba, svg,
};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Sizable, Size};

use crate::theme::Theme;

mod calendar;
mod combo;
mod text_input;
mod views;

pub(crate) use calendar::compact_calendar;
pub(crate) use combo::{ComboBox, ComboOption};
pub(crate) use text_input::{TextInput, TextInputOptions};
pub(super) use views::{
    ColumnGrid, DetailColumns, DetailList, DetailScroll, ViewMode, approx_text_width,
    detail_cell_text, detail_column_width, detail_content_width, detail_header_cell,
    detail_header_cell_plain, detail_header_column, detail_header_row, detail_row, grid_column,
    grid_columns, grid_item_sized, grid_item_width, leading_icon_badge, marquee_rect, tag_chip,
    view_mode_toggle,
};

/// The variant of a push button. See `win_button` / `dialog_button`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ButtonKind {
    Normal,
    Default,
    Danger,
    /// A push button that is rendered but cannot be clicked (e.g. Save with nothing selected).
    Disabled,
}

/// A push button, the single source of truth for button chrome. Backed by the gpui-kit
/// (shadcn-style) button so dialog and command buttons match the rest of the UI kit. Rendered at
/// `Size::Small` (24px, the app's compact dialog metric) rather than the kit's 32px medium.
pub(super) fn button(
    id: impl Into<SharedString>,
    label: String,
    kind: ButtonKind,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    sized_button(id, label, kind, Size::Small, theme, on_click)
}

/// A compact dialog button (24px, matching the toolbar items) for the app's floating popups.
pub(super) fn popup_button(
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
    sized_button(id, label, kind, Size::Small, theme, on_click)
}

fn sized_button(
    id: impl Into<SharedString>,
    label: String,
    kind: ButtonKind,
    size: Size,
    _theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let button = match kind {
        ButtonKind::Normal => Button::new(id.into()).outline(),
        ButtonKind::Default => Button::new(id.into()).primary(),
        ButtonKind::Danger => Button::new(id.into()).danger(),
        ButtonKind::Disabled => Button::new(id.into()).outline().disabled(true),
    };
    button
        .compact()
        .with_size(size)
        .label(label)
        .on_click(on_click)
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

/// A flat icon+label item used by the main and object toolbars, backed by the kit's ghost
/// button so it picks up the shadcn hover/disabled states.
///
/// The kit's own button typography (16px at the default size) is larger than the app's chrome, so
/// the content is supplied as explicit children: a 16px icon and a 12px label.
pub(super) fn toolbar_item(
    id: impl Into<SharedString>,
    icon: &'static str,
    label: String,
    enabled: bool,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let color = if enabled {
        theme.text
    } else {
        theme.text_muted
    };
    Button::new(id.into())
        .ghost()
        .compact()
        .small()
        .disabled(!enabled)
        .on_click(on_click)
        .child(
            svg()
                .path(icon)
                .w(px(16.0))
                .h(px(16.0))
                .flex_none()
                .text_color(rgb(color)),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(color))
                .child(label),
        )
}

/// A small square icon button, the shared chrome for the filter/sort builders' row actions.
/// `bordered` picks the raised face (button background + border) used for add buttons; otherwise
/// it is a flat hover-highlighted icon.
#[allow(clippy::too_many_arguments)]
pub(super) fn icon_button(
    id: impl Into<SharedString>,
    icon: &'static str,
    color: u32,
    width: f32,
    height: f32,
    bordered: bool,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let icon_size = width.min(height) * 0.58;
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .when(bordered, move |style| {
            style
                .border_1()
                .border_color(rgb(theme.border))
                .bg(rgb(theme.button_bg))
        })
        .cursor_pointer()
        .hover(move |style| {
            let style = style.bg(rgb(theme.tree_hover_bg));
            if bordered {
                style.border_color(rgb(theme.button_default_border))
            } else {
                style
            }
        })
        .on_click(on_click)
        .child(
            svg()
                .path(icon)
                .w(px(icon_size))
                .h(px(icon_size))
                .flex_none()
                .text_color(rgb(color)),
        )
}

/// The chrome of a `并且/或者` pill rendered by [`segmented_toggle`]. Two presets cover the filter
/// builder's in-group rail pill and the floating between-groups boundary pill.
#[derive(Clone, Copy)]
pub(super) struct TogglePillStyle {
    pub container_pad: f32,
    pub button_pad_x: f32,
    pub button_pad_y: f32,
    pub container_radius: f32,
    pub button_radius: f32,
    pub container_bg: u32,
    pub container_border: u32,
    pub shadow: bool,
    pub text_size: f32,
}

impl TogglePillStyle {
    /// The compact in-group rail pill (`bg-slate-100`, tight padding, no shadow).
    pub(super) fn in_group(theme: Theme) -> Self {
        Self {
            container_pad: 2.0,
            button_pad_x: 7.0,
            button_pad_y: 2.0,
            container_radius: 5.0,
            button_radius: 4.0,
            container_bg: theme.button_bg,
            container_border: theme.button_border,
            shadow: false,
            text_size: 11.0,
        }
    }

    /// The floating between-groups pill (`bg-white`, looser padding, soft shadow).
    pub(super) fn boundary(theme: Theme) -> Self {
        Self {
            container_pad: 2.0,
            button_pad_x: 8.0,
            button_pad_y: 2.0,
            container_radius: 6.0,
            button_radius: 5.0,
            container_bg: theme.dialog_bg,
            container_border: theme.border,
            shadow: true,
            text_size: 11.0,
        }
    }
}

/// A two-option segmented toggle (the filter builder's `并且 | 或者`). The whole widget is a single
/// click target, so `on_click` toggles between the two options.
pub(super) fn segmented_toggle(
    id: impl Into<SharedString>,
    first: String,
    second: String,
    second_active: bool,
    style: TogglePillStyle,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let mut toggle = div()
        .id(id.into())
        .flex()
        .flex_row()
        .items_center()
        .p(px(style.container_pad))
        .flex_none()
        .rounded(px(style.container_radius))
        .border_1()
        .border_color(rgb(style.container_border))
        .bg(rgb(style.container_bg))
        .when(style.shadow, |this| this.shadow(soft_shadow()))
        .cursor_pointer()
        .on_click(on_click);
    for (label, active) in [(first, !second_active), (second, second_active)] {
        toggle = toggle.child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .px(px(style.button_pad_x))
                .py(px(style.button_pad_y))
                .rounded(px(style.button_radius))
                .text_size(px(style.text_size))
                .font_weight(FontWeight::BOLD)
                .when(active, move |style| {
                    style.bg(rgb(theme.primary)).text_color(rgb(0xffffff))
                })
                .when(!active, move |style| {
                    style.text_color(rgb(theme.text_muted))
                })
                .child(label),
        );
    }
    toggle
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

/// A 5px pane resize divider: the shared splitter handle between two side-by-side panes. The
/// pane/window that owns the boundary attaches the mouse-down handler; the enclosing window then
/// routes mouse move/up back to that owner, so the drag survives the pointer leaving the divider.
pub(super) fn pane_resize_divider(id: impl Into<SharedString>, theme: Theme) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .w(px(super::PANE_DIVIDER_WIDTH))
        .h_full()
        .cursor(CursorStyle::ResizeLeftRight)
        .bg(rgb(theme.border))
        .hover(move |style| style.bg(rgb(theme.primary)))
}

/// Like [`pane_resize_divider`] but invisible until hovered, for surfaces (the filter builder)
/// where an always-on divider reads as a scrollbar.
pub(super) fn hover_resize_divider(id: impl Into<SharedString>, theme: Theme) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .w(px(super::PANE_DIVIDER_WIDTH))
        .h_full()
        .cursor(CursorStyle::ResizeLeftRight)
        .hover(move |style| style.bg(rgb(theme.primary)))
}

/// A taller 36px separator between main toolbar groups.
pub(super) fn main_separator(theme: Theme) -> impl IntoElement {
    separator(theme, 36.0)
}

/// A stateless shadcn check box: 14px, 3px radius, primary fill when checked. It is drawn
/// rather than using the kit's `Checkbox` because these appear inside rows that own the click
/// and must keep their exact size (and a stateful element would need a unique id per row).
pub(super) fn checkbox_box(checked: bool, theme: Theme) -> impl IntoElement {
    let foreground = if theme.is_dark() {
        theme.window_bg
    } else {
        0xffffff
    };
    div()
        .w(px(14.0))
        .h(px(14.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .border_1()
        .border_color(rgb(if checked { theme.primary } else { theme.border }))
        .bg(rgb(if checked {
            theme.primary
        } else {
            theme.input_bg
        }))
        .text_color(rgb(foreground))
        .text_size(px(10.0))
        .child(if checked { "✓" } else { "" }.to_string())
}

/// A stateless shadcn radio dot, sized to match [`checkbox_box`]. Like the check box it is drawn
/// rather than using the kit's stateful control, so it can sit inside a row that owns the click.
pub(super) fn radio_box(selected: bool, theme: Theme) -> impl IntoElement {
    let foreground = if theme.is_dark() {
        theme.window_bg
    } else {
        0xffffff
    };
    div()
        .w(px(14.0))
        .h(px(14.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(rgb(if selected {
            theme.primary
        } else {
            theme.border
        }))
        .bg(rgb(if selected {
            theme.primary
        } else {
            theme.input_bg
        }))
        .child(
            div()
                .w(px(6.0))
                .h(px(6.0))
                .rounded_full()
                .when(selected, move |style| style.bg(rgb(foreground))),
        )
}

/// The shared chrome for an app-drawn floating surface (popup menus, dropdown lists, pickers):
/// absolutely positioned with the dialog face, a border and the soft shadow.
///
/// It also sets `.occlude()`, which is the important part: gpui dispatches `ScrollWheelEvent` to
/// *every* scrollable hitbox under the cursor (each `overflow_*` container handles it without
/// stopping propagation), so a popup list that overlays a scrollable pane would scroll both.
/// `.occlude()` makes every hitbox behind the popup report `should_handle_scroll() == false`
/// (see `HitboxBehavior::BlockMouse`), leaving only the popup's own list scrollable.
///
/// Every app-drawn floating surface must be built on this helper so that behaviour stays global —
/// do not hand-roll `div().absolute()` popups, and do not drop the occlusion.
pub(super) fn popup_panel(theme: Theme) -> Div {
    div()
        .absolute()
        .occlude()
        .flex()
        .flex_col()
        .bg(rgb(theme.dialog_bg))
        .border_1()
        .border_color(rgb(theme.border))
        .shadow(dialog_shadow())
}

/// The soft drop shadow behind every floating surface (dialogs and popup menus).
pub(super) fn dialog_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: rgba(0x00000026).into(),
        offset: Point {
            x: px(0.0),
            y: px(6.0),
        },
        blur_radius: px(18.0),
        spread_radius: px(0.0),
        inset: false,
    }]
}

/// The very soft shadow used by the filter builder's cards and floating pills, matching the
/// reference design's `shadow-[0_2px_8px_-2px_rgba(15,23,42,0.06)]`.
pub(super) fn soft_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: rgba(0x0f172a0f).into(),
        offset: Point {
            x: px(0.0),
            y: px(2.0),
        },
        blur_radius: px(8.0),
        spread_radius: px(-2.0),
        inset: false,
    }]
}

/// The neutral input face shared by text fields, combos and dropdowns. Callers add layout,/// sizing, focus and children.
pub(super) fn text_field(theme: Theme) -> Div {
    div()
        .bg(rgb(theme.input_bg))
        .border_1()
        .border_color(rgb(theme.border))
}

/// A zero-layout caret anchor: a 0px box with an absolutely positioned 1px bar, so toggling it
/// never pushes the text around (unlike a `"|"` glyph). Used by the simple append-only inputs;
/// the full [`TextInput`](text_input::TextInput) paints its own caret.
pub(super) fn text_caret(theme: Theme) -> impl IntoElement {
    div().relative().flex_none().w(px(0.0)).h_full().child(
        div()
            .absolute()
            .left_0()
            .top(px(2.0))
            .bottom(px(2.0))
            .w(px(1.0))
            .bg(rgb(theme.text)),
    )
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

/// The thumb's `(top, length)` as fractions of the track, for `relative()` placement. Because the
/// thumb is sized and positioned relative to the track it is drawn in, it can never overflow the
/// track even when the viewport used for the math and the track's own size disagree.
pub(super) fn scrollbar_fractions(viewport: f32, max: f32, scroll: f32) -> (f32, f32) {
    if viewport <= 0.0 || max <= 0.0 {
        return (0.0, 1.0);
    }
    let content = (viewport + max).max(1.0);
    let min_len = (24.0 / viewport).min(1.0);
    let length = (viewport / content).clamp(min_len, 1.0);
    let travel = 1.0 - length;
    let top = if max > 0.0 {
        (scroll / max).clamp(0.0, 1.0) * travel
    } else {
        0.0
    };
    (top, length)
}

/// A vertical scrollbar track (14px wide) with its thumb placed by fraction of the track:
/// `thumb_top`/`thumb_len` are in `[0, 1]`, so the thumb is always inside the track. gpui 0.2
/// does not paint scrollbars for `overflow_*`, so every scrollable pane draws its own; callers
/// attach the drag handler.
pub(super) fn vscrollbar_track(
    id: impl Into<SharedString>,
    theme: Theme,
    thumb_top: f32,
    thumb_len: f32,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .relative()
        .flex_none()
        .w(px(14.0))
        .h_full()
        .overflow_hidden()
        .bg(rgb(theme.scroll_track))
        .border_l_1()
        .border_color(rgb(theme.border))
        .cursor_pointer()
        .child(
            div()
                .absolute()
                .left(px(1.0))
                .top(relative(thumb_top))
                .w(px(12.0))
                .h(relative(thumb_len))
                .bg(rgb(theme.scroll_thumb)),
        )
}

/// A horizontal scrollbar track (14px tall) with its thumb placed by fraction of the track.
pub(super) fn hscrollbar_track(
    id: impl Into<SharedString>,
    theme: Theme,
    thumb_left: f32,
    thumb_len: f32,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .relative()
        .flex_none()
        .w_full()
        .h(px(14.0))
        .overflow_hidden()
        .bg(rgb(theme.scroll_track))
        .border_t_1()
        .border_color(rgb(theme.border))
        .cursor_pointer()
        .child(
            div()
                .absolute()
                .left(relative(thumb_left))
                .top(px(1.0))
                .w(relative(thumb_len))
                .h(px(12.0))
                .bg(rgb(theme.scroll_thumb)),
        )
}
