//! The two reusable list layouts a page can present — the 详细列表 (sortable detail table) and the
//! 平铺网格 (Navicat-style column grid) — plus the segmented switch that selects between them.
//!
//! Pages pick a layout and remember the choice per page (see `AppView::view_mode`); the chrome
//! lives here so every page shows the same 详细列表 / 平铺网格 look. Feature code composes the
//! helpers below ([`DetailList`], [`detail_content_width`], [`DetailColumns`],
//! [`detail_column_width`], [`detail_header_row`], [`detail_header_cell`],
//! [`detail_header_column`], [`detail_row`], [`ColumnGrid`], [`grid_columns`], [`grid_column`],
//! [`grid_item`], [`leading_icon_badge`], [`tag_chip`]) with its own rows.
//!
//! A 详细列表's columns are fitted to their content (header label and the widest visible cell) and
//! can be dragged wider or narrower from the handle on each header cell's right edge; the page owns
//! the [`DetailColumns`] state and hands its resolved widths to both the header and the rows.
//!
//! The 平铺网格 is column-major, matching the object list: items fill a column top-to-bottom and
//! wrap to the next column to the right, so the grid scrolls horizontally. [`ColumnGrid`] owns the
//! horizontal scroll and the drag state; pages chunk their items into [`grid_column`]s using
//! [`ColumnGrid::rows_per_column`].

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ClickEvent, CursorStyle, Div, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollHandle, SharedString,
    Stateful, Window, canvas, div, prelude::*, px, rgb, rgba, svg,
};

use super::{hscrollbar_track, scrollbar_fractions, scrollbar_thumb, vscrollbar_track};
use crate::theme::Theme;
use gpui_kit::component::{Icon, IconName};

/// The height of one item row of a 平铺网格 column. This is the single shared row metric: every
/// page's 平铺网格 uses it so the layouts look identical.
pub(crate) const GRID_ROW_HEIGHT: f32 = 20.0;
/// The default and minimum width of one item of a 平铺网格 column. Lists whose names are longer
/// widen their tiles (up to [`GRID_ITEM_MAX_WIDTH`]) so the names are shown in full.
pub(crate) const GRID_ITEM_WIDTH: f32 = 220.0;
/// The widest a 平铺网格 item grows to fit its longest name.
pub(crate) const GRID_ITEM_MAX_WIDTH: f32 = 620.0;
/// The bottom margin a column leaves before it decides it has no room for another row.
pub(crate) const GRID_BOTTOM_MARGIN: f32 = 16.0;

/// The height of a 详细列表's sticky header strip, shared by [`detail_header_row`] and the
/// vertical scrollbar, which starts below it.
pub(crate) const DETAIL_HEADER_HEIGHT: f32 = 32.0;
/// The height of one 详细列表 row. Every page uses it through [`detail_row`], so all detail lists
/// have the same (deliberately compact) density.
pub(crate) const DETAIL_ROW_HEIGHT: f32 = 28.0;
/// The side of the leading icon badge in a 详细列表 row.
pub(crate) const DETAIL_BADGE_SIZE: f32 = 18.0;
/// The thickness of the app-drawn scrollbars, matching [`hscrollbar_track`] / [`vscrollbar_track`].
pub(crate) const DETAIL_SCROLLBAR_THICKNESS: f32 = 14.0;

/// The callback type of [`view_mode_toggle`], shared by every caller.
pub(crate) type ViewModeHandler = Rc<dyn Fn(ViewMode, &ClickEvent, &mut Window, &mut App)>;

/// Which of the two shared list layouts a page presents.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ViewMode {
    /// 详细列表: a sortable table with named columns.
    Detail,
    /// 平铺网格: a Navicat-style column-major grid. The default for every page.
    #[default]
    Grid,
}

impl ViewMode {
    /// Every mode, in the order the switch presents them (平铺网格 first, then 详细列表).
    pub const ALL: [ViewMode; 2] = [ViewMode::Grid, ViewMode::Detail];

    /// The stable id persisted in settings.
    pub fn id(self) -> &'static str {
        match self {
            ViewMode::Detail => "detail",
            ViewMode::Grid => "grid",
        }
    }

    /// Parse a persisted id, defaulting to 平铺网格.
    pub fn from_id(value: &str) -> Self {
        match value {
            "detail" => ViewMode::Detail,
            _ => ViewMode::Grid,
        }
    }

    /// The i18n key for the switch's label.
    pub fn label_key(self) -> &'static str {
        match self {
            ViewMode::Detail => "view.detail",
            ViewMode::Grid => "view.grid",
        }
    }

    /// The switch's icon asset.
    pub fn icon_path(self) -> &'static str {
        match self {
            ViewMode::Detail => "icons/view-detail.svg",
            ViewMode::Grid => "icons/view-grid.svg",
        }
    }
}

/// The segmented 详细列表 / 平铺网格 switch shown at the top of a list page. `on_select` receives
/// the clicked mode; wrap it in an `Rc` so both segments share it.
pub(crate) fn view_mode_toggle(
    theme: Theme,
    current: ViewMode,
    on_select: ViewModeHandler,
) -> impl IntoElement {
    let mut group = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_0p5()
        .p_0p5()
        .flex_none()
        .rounded(px(7.0))
        .bg(rgb(theme.button_bg))
        .border_1()
        .border_color(rgb(theme.border));
    for mode in ViewMode::ALL {
        let active = mode == current;
        let callback = on_select.clone();
        let icon_color = if active { theme.text } else { theme.text_muted };
        let label = t!(mode.label_key()).to_string();
        group = group.child(
            div()
                .id(SharedString::from(format!("view-mode-{}", mode.id())))
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap_1()
                .w(px(26.0))
                .h(px(22.0))
                .flex_none()
                .rounded(px(5.0))
                .cursor_pointer()
                .text_size(px(12.0))
                .when(active, move |style| {
                    style
                        .bg(rgb(theme.dialog_bg))
                        .text_color(rgb(theme.text))
                        .font_weight(FontWeight::MEDIUM)
                        .border_1()
                        .border_color(rgb(theme.border))
                })
                .when(!active, move |style| {
                    style
                        .text_color(rgb(theme.text_muted))
                        .hover(move |style| style.text_color(rgb(theme.text)))
                })
                .tooltip({
                    let label = label.clone();
                    move |_, cx| cx.new(|_| ModeTooltip(label.clone())).into()
                })
                .on_click(move |event, window, cx| callback(mode, event, window, cx))
                .child(
                    svg()
                        .path(mode.icon_path())
                        .w(px(14.0))
                        .h(px(14.0))
                        .flex_none()
                        .text_color(rgb(icon_color)),
                ),
        );
    }
    group
}

/// The hover tooltip of one 详细列表 / 平铺网格 icon segment.
struct ModeTooltip(String);

impl Render for ModeTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = gpui_kit::component::Theme::global(cx);
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .text_size(px(12.0))
            .text_color(theme.popover_foreground)
            .child(self.0.clone())
    }
}

/// The natural content width of a 详细列表 whose fixed columns have the given widths: their sum
/// plus the `gap_3` between columns and the `px_3` padding on both sides. Pages hand it to
/// [`DetailList`] so the frame knows when the row is wider than the viewport and must show a
/// horizontal scrollbar.
pub(crate) fn detail_content_width(widths: &[f32]) -> f32 {
    let gaps = widths.len().saturating_sub(1) as f32 * 12.0;
    widths.iter().sum::<f32>() + gaps + 24.0
}

/// The narrowest a 详细列表 column can be dragged to.
pub(crate) const DETAIL_MIN_COLUMN_WIDTH: f32 = 48.0;
/// The widest a 详细列表 column can be dragged to.
pub(crate) const DETAIL_MAX_COLUMN_WIDTH: f32 = 720.0;
/// The horizontal room a 详细列表 column adds around its widest text, so the header's sort badge
/// and the `gap_3` gutter never clip it.
const DETAIL_COLUMN_PADDING: f32 = 20.0;

/// The content-fitted width of one 详细列表 column: `longest` (the widest header label or cell,
/// measured with [`approx_text_width`]) plus padding, clamped between `min` and
/// [`DETAIL_MAX_COLUMN_WIDTH`]. Pages compute it per column, then hand the list to
/// [`DetailColumns::resolve`] so a user-dragged width can override it.
pub(crate) fn detail_column_width(longest: f32, min: f32) -> f32 {
    (longest + DETAIL_COLUMN_PADDING).clamp(min, DETAIL_MAX_COLUMN_WIDTH)
}

/// The live width state of one 详细列表. Pages fit the widths to their content each render and pass
/// them to [`resolve`](Self::resolve); a column the user has dragged keeps its override until the
/// list's column count changes (e.g. the object list switching from Tables to Views). The list's
/// mouse handlers drive the drag through [`begin_resize`](Self::begin_resize) /
/// [`drag_resize`](Self::drag_resize) / [`end_resize`](Self::end_resize).
#[derive(Default)]
pub(crate) struct DetailColumns {
    /// The user's dragged widths, indexed by column; `None` means "use the fitted width".
    overrides: Vec<Option<f32>>,
    resize: Option<DetailResize>,
}

/// The active edge drag of a 详细列表.
#[derive(Clone, Copy)]
struct DetailResize {
    column: usize,
    start_x: f32,
    start_width: f32,
}

impl DetailColumns {
    /// Resolve the widths to render: each fitted width, replaced by the user's override when set.
    /// A change in the column count resets the overrides, since the columns no longer correspond.
    pub(crate) fn resolve(&mut self, fitted: &[f32]) -> Vec<f32> {
        if self.overrides.len() != fitted.len() {
            self.overrides = vec![None; fitted.len()];
            self.resize = None;
        }
        fitted
            .iter()
            .enumerate()
            .map(|(column, width)| self.overrides[column].unwrap_or(*width))
            .collect()
    }

    /// Begin dragging the right edge of `column`, starting from its current `width`.
    pub(crate) fn begin_resize(&mut self, column: usize, mouse_x: Pixels, width: f32) {
        if column >= self.overrides.len() {
            return;
        }
        self.resize = Some(DetailResize {
            column,
            start_x: f32::from(mouse_x),
            start_width: width,
        });
    }

    /// Continue a drag. Returns whether the widths changed, so the caller re-renders.
    pub(crate) fn drag_resize(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(resize) = self.resize else {
            return false;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.resize = None;
            return true;
        }
        let delta = f32::from(event.position.x) - resize.start_x;
        let width =
            (resize.start_width + delta).clamp(DETAIL_MIN_COLUMN_WIDTH, DETAIL_MAX_COLUMN_WIDTH);
        match self.overrides.get_mut(resize.column) {
            Some(slot) if *slot != Some(width) => {
                *slot = Some(width);
                true
            }
            _ => false,
        }
    }

    /// End a drag. Returns whether one was active.
    pub(crate) fn end_resize(&mut self) -> bool {
        self.resize.take().is_some()
    }

    /// Whether `column`'s edge is being dragged, so its handle paints active.
    pub(crate) fn resizing(&self, column: usize) -> bool {
        self.resize.is_some_and(|resize| resize.column == column)
    }
}

/// One 详细列表 header column: a fixed-width, relatively positioned cell with a visible separator
/// and a right-edge drag handle straddling the `gap_3` gutter. The separator is what tells the user
/// where the draggable boundary is. `content` is the header cell; `on_resize_start` begins the drag
/// (the page routes it to [`DetailColumns::begin_resize`]).
pub(crate) fn detail_header_column(
    handle_id: impl Into<SharedString>,
    width: f32,
    active: bool,
    theme: Theme,
    content: impl IntoElement,
    on_resize_start: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let handle = div()
        .id(handle_id.into())
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .right(px(-3.0))
        .w(px(6.0))
        .flex()
        .justify_center()
        .cursor(CursorStyle::ResizeColumn)
        .hover(move |handle| handle.bg(rgba((theme.primary << 8) | 0x26)))
        .when(active, move |handle| {
            handle.bg(rgba((theme.primary << 8) | 0x33))
        })
        .on_mouse_down(MouseButton::Left, on_resize_start)
        .child(div().w(px(1.0)).h_full().bg(rgb(if active {
            theme.primary
        } else {
            theme.button_default_border
        })));
    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .h_full()
        .w(px(width))
        .flex_none()
        .child(content)
        .child(handle)
}

/// The persistent scroll state of one 详细列表: the two scroll handles (rows scroll vertically,
/// the header and rows together scroll horizontally), the active scrollbar-thumb drags, and the
/// last measured overflow state of each axis. Pages own one and hand it to [`DetailList`]; because
/// the drag flags and overflow flags live here rather than in the transient [`DetailList`], a thumb
/// drag and the scrollbar visibility survive the per-render rebuild.
#[derive(Default)]
pub(crate) struct DetailScroll {
    /// The horizontal scroll of the header + rows, tracked on the outer viewport.
    hscroll: ScrollHandle,
    /// The vertical scroll of the rows only, tracked on the rows viewport.
    vscroll: ScrollHandle,
    /// The active horizontal-thumb grab offset, if dragging.
    h_grab: Rc<Cell<Option<f32>>>,
    /// The active vertical-thumb grab offset, if dragging.
    v_grab: Rc<Cell<Option<f32>>>,
    /// Whether the last measurement found horizontal overflow. Written by the frame's measurement
    /// canvas (after layout) and read on the next render to decide whether a track is drawn.
    h_overflow: Rc<Cell<bool>>,
    /// Whether the last measurement found vertical overflow.
    v_overflow: Rc<Cell<bool>>,
}

/// The scrolling frame of a 详细列表: a sticky header and the rows, scrolled horizontally together,
/// with the rows additionally scrolling vertically. gpui paints no scrollbars for `overflow_*`, so
/// the frame draws its own tracks as flex siblings (like the table grid) — the vertical one always
/// sits at the visible right edge — and the scroll areas use native `overflow_*_scroll` so a
/// trackpad pans both axes. A track is only drawn when its axis actually overflows; because the
/// scroll extents are known only after layout, a small measurement canvas records the overflow
/// state after each layout and requests one more frame when it changes. The handles, grab flags and
/// overflow flags are owned by the page's [`DetailScroll`], so the scroll position and any drag
/// survive re-renders.
pub(crate) struct DetailList {
    id: &'static str,
    hscroll: ScrollHandle,
    vscroll: ScrollHandle,
    h_grab: Rc<Cell<Option<f32>>>,
    v_grab: Rc<Cell<Option<f32>>>,
    h_overflow: Rc<Cell<bool>>,
    v_overflow: Rc<Cell<bool>>,
    content_width: f32,
    /// The header's height: the vertical scrollbar starts below it.
    header_height: f32,
    header: Div,
    rows: Div,
}

impl DetailList {
    /// Start a list. `content_width` is [`detail_content_width`] of the page's fixed columns; the
    /// header is the [`detail_header_row`] the page built.
    pub(crate) fn new(
        id: &'static str,
        scroll: &DetailScroll,
        content_width: f32,
        header: Div,
    ) -> Self {
        Self {
            id,
            hscroll: scroll.hscroll.clone(),
            vscroll: scroll.vscroll.clone(),
            h_grab: scroll.h_grab.clone(),
            v_grab: scroll.v_grab.clone(),
            h_overflow: scroll.h_overflow.clone(),
            v_overflow: scroll.v_overflow.clone(),
            content_width,
            header_height: DETAIL_HEADER_HEIGHT,
            header,
            rows: div().flex().flex_col().min_w(px(content_width)),
        }
    }

    /// The header height the vertical scrollbar starts below. A list without a header passes
    /// `0.0` so its track spans the whole viewport.
    pub(crate) fn header_height(mut self, height: f32) -> Self {
        self.header_height = height;
        self
    }

    /// Append one row (or the empty-state message) to the list.
    pub(crate) fn child(mut self, child: impl IntoElement) -> Self {
        self.rows = self.rows.child(child);
        self
    }

    /// Build the frame. The header and the vertical scroller share one horizontally-scrolled
    /// container of at least `content_width`, so the header stays aligned with its rows while the
    /// pane is too narrow and the columns are left-packed when it is wide.
    pub(crate) fn render(self, theme: Theme) -> AnyElement {
        let Self {
            id,
            hscroll,
            vscroll,
            h_grab,
            v_grab,
            h_overflow,
            v_overflow,
            content_width,
            header_height,
            header,
            rows,
        } = self;

        // Read the overflow state measured after the previous layout, and refresh it once the
        // current layout has run (see the measurement canvas below).
        let show_h = h_overflow.get();
        let show_v = v_overflow.get();
        let measure_h = hscroll.clone();
        let measure_v = vscroll.clone();
        let h_flag = h_overflow.clone();
        let v_flag = v_overflow.clone();
        let measure = canvas(
            |_bounds, _window: &mut Window, _cx: &mut App| {},
            move |_bounds, _state, window: &mut Window, _cx: &mut App| {
                // Painted after the scroll areas (it is the frame's last child), so the tracked
                // handles already carry this frame's measured extents.
                let h = measure_h.max_offset().x > px(0.0);
                let v = measure_v.max_offset().y > px(0.0);
                if h_flag.get() != h {
                    h_flag.set(h);
                    window.request_animation_frame();
                }
                if v_flag.get() != v {
                    v_flag.set(v);
                    window.request_animation_frame();
                }
            },
        );

        let h_begin = {
            let hscroll = hscroll.clone();
            let h_grab = h_grab.clone();
            move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                begin_h_thumb_drag(&hscroll, &h_grab, event.position.x);
                window.refresh();
            }
        };
        let v_begin = {
            let vscroll = vscroll.clone();
            let v_grab = v_grab.clone();
            move |event: &MouseDownEvent, window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                begin_v_thumb_drag(&vscroll, &v_grab, event.position.y);
                window.refresh();
            }
        };
        let move_scroll = hscroll.clone();
        let move_vscroll = vscroll.clone();
        let move_h_grab = h_grab.clone();
        let move_v_grab = v_grab.clone();
        let on_move = move |event: &MouseMoveEvent, window: &mut Window, _cx: &mut App| {
            let mut changed = drag_h_thumb(&move_scroll, &move_h_grab, event.position.x);
            changed |= drag_v_thumb(&move_vscroll, &move_v_grab, event.position.y);
            if changed {
                window.refresh();
            }
        };
        let up_h_grab = h_grab.clone();
        let up_v_grab = v_grab.clone();
        let on_up = move |_event: &MouseUpEvent, window: &mut Window, _cx: &mut App| {
            if up_h_grab.take().is_some() | up_v_grab.take().is_some() {
                window.refresh();
            }
        };

        let h_track: AnyElement = if show_h {
            hscrollbar_element(id, &hscroll, theme, h_begin)
        } else {
            // Reserve the same height as a real scrollbar so the vertical track's viewport stays
            // constant whether or not the columns overflow, like the table grid.
            div()
                .flex_none()
                .h(px(DETAIL_SCROLLBAR_THICKNESS))
                .into_any_element()
        };
        let v_track: AnyElement = if show_v {
            vscrollbar_element(id, &vscroll, theme, header_height, v_begin)
        } else {
            div().into_any_element()
        };

        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .bg(rgb(theme.editor_bg))
            .on_mouse_move(on_move)
            .on_mouse_up(MouseButton::Left, on_up)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h(px(0.0))
                            .min_w(px(0.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("{id}-hscroll")))
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .min_w(px(0.0))
                                    .overflow_x_scroll()
                                    .restrict_scroll_to_axis()
                                    .track_scroll(&hscroll)
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .h_full()
                                            .w_full()
                                            .min_w(px(content_width))
                                            .child(header)
                                            .child(
                                                div()
                                                    .id(SharedString::from(format!("{id}-vscroll")))
                                                    .flex()
                                                    .flex_col()
                                                    .flex_1()
                                                    .min_h(px(0.0))
                                                    .min_w(px(content_width))
                                                    .overflow_y_scroll()
                                                    .restrict_scroll_to_axis()
                                                    .track_scroll(&vscroll)
                                                    .child(rows),
                                            ),
                                    ),
                            )
                            .child(h_track),
                    )
                    .child(v_track),
            )
            .child(measure.absolute().inset_0())
            .into_any_element()
    }
}

/// The horizontal scrollbar of a 详细列表, drawn only when the columns overflow. `on_mouse_down`
/// begins the thumb drag.
fn hscrollbar_element(
    id: &'static str,
    hscroll: &ScrollHandle,
    theme: Theme,
    on_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let viewport = f32::from(hscroll.bounds().size.width);
    let max = f32::from(hscroll.max_offset().x);
    let scroll = -f32::from(hscroll.offset().x);
    let (thumb_left, thumb_len) = scrollbar_fractions(viewport, max, scroll);
    hscrollbar_track(
        SharedString::from(format!("{id}-hscrollbar")),
        theme,
        thumb_left,
        thumb_len,
    )
    .on_mouse_down(MouseButton::Left, on_mouse_down)
    .into_any_element()
}

/// The vertical scrollbar of a 详细列表, spanning the rows below the header and above the
/// horizontal bar, drawn only when the rows overflow. `on_mouse_down` begins the thumb drag.
fn vscrollbar_element(
    id: &'static str,
    vscroll: &ScrollHandle,
    theme: Theme,
    header_height: f32,
    on_mouse_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let viewport = f32::from(vscroll.bounds().size.height);
    let max = f32::from(vscroll.max_offset().y);
    let scroll = -f32::from(vscroll.offset().y);
    let (thumb_top, thumb_len) = scrollbar_fractions(viewport, max, scroll);
    div()
        .flex_none()
        .h_full()
        .pt(px(header_height))
        .pb(px(DETAIL_SCROLLBAR_THICKNESS))
        .child(
            vscrollbar_track(
                SharedString::from(format!("{id}-vscrollbar")),
                theme,
                thumb_top,
                thumb_len,
            )
            .on_mouse_down(MouseButton::Left, on_mouse_down),
        )
        .into_any_element()
}

/// Capture the horizontal thumb at `mouse_x` and jump the view there.
fn begin_h_thumb_drag(scroll: &ScrollHandle, grab: &Cell<Option<f32>>, mouse_x: Pixels) {
    let bounds = scroll.bounds();
    let viewport = f32::from(bounds.size.width);
    let max = f32::from(scroll.max_offset().x);
    let (thumb_len, travel) = scrollbar_thumb(viewport, max);
    if travel <= 0.0 {
        return;
    }
    let offset = -f32::from(scroll.offset().x);
    let thumb_start = (offset / max) * travel;
    let relative = f32::from(mouse_x) - f32::from(bounds.left());
    let grabbed = if relative >= thumb_start && relative <= thumb_start + thumb_len {
        relative - thumb_start
    } else {
        thumb_len / 2.0
    };
    grab.set(Some(grabbed));
    set_h_thumb(scroll, relative, grabbed);
}

/// Continue a horizontal thumb drag. Returns whether a drag was active.
fn drag_h_thumb(scroll: &ScrollHandle, grab: &Cell<Option<f32>>, mouse_x: Pixels) -> bool {
    let Some(grabbed) = grab.get() else {
        return false;
    };
    let relative = f32::from(mouse_x) - f32::from(scroll.bounds().left());
    set_h_thumb(scroll, relative, grabbed);
    true
}

/// Move the horizontal view so the grabbed point of the thumb sits under `relative`.
fn set_h_thumb(scroll: &ScrollHandle, relative: f32, grabbed: f32) {
    let viewport = f32::from(scroll.bounds().size.width);
    let max = f32::from(scroll.max_offset().x);
    let (_, travel) = scrollbar_thumb(viewport, max);
    if travel <= 0.0 {
        return;
    }
    let thumb_x = (relative - grabbed).clamp(0.0, travel);
    let offset = thumb_x / travel * max;
    let y = scroll.offset().y;
    scroll.set_offset(Point::new(px(-offset), y));
}

/// Capture the vertical thumb at `mouse_y` and jump the view there.
fn begin_v_thumb_drag(scroll: &ScrollHandle, grab: &Cell<Option<f32>>, mouse_y: Pixels) {
    let bounds = scroll.bounds();
    let viewport = f32::from(bounds.size.height);
    let max = f32::from(scroll.max_offset().y);
    let (thumb_len, travel) = scrollbar_thumb(viewport, max);
    if travel <= 0.0 {
        return;
    }
    let offset = -f32::from(scroll.offset().y);
    let thumb_start = (offset / max) * travel;
    let relative = f32::from(mouse_y) - f32::from(bounds.top());
    let grabbed = if relative >= thumb_start && relative <= thumb_start + thumb_len {
        relative - thumb_start
    } else {
        thumb_len / 2.0
    };
    grab.set(Some(grabbed));
    set_v_thumb(scroll, relative, grabbed);
}

/// Continue a vertical thumb drag. Returns whether a drag was active.
fn drag_v_thumb(scroll: &ScrollHandle, grab: &Cell<Option<f32>>, mouse_y: Pixels) -> bool {
    let Some(grabbed) = grab.get() else {
        return false;
    };
    let relative = f32::from(mouse_y) - f32::from(scroll.bounds().top());
    set_v_thumb(scroll, relative, grabbed);
    true
}

/// Move the vertical view so the grabbed point of the thumb sits under `relative`.
fn set_v_thumb(scroll: &ScrollHandle, relative: f32, grabbed: f32) {
    let viewport = f32::from(scroll.bounds().size.height);
    let max = f32::from(scroll.max_offset().y);
    let (_, travel) = scrollbar_thumb(viewport, max);
    if travel <= 0.0 {
        return;
    }
    let thumb_y = (relative - grabbed).clamp(0.0, travel);
    let offset = thumb_y / travel * max;
    let x = scroll.offset().x;
    scroll.set_offset(Point::new(x, px(-offset)));
}

/// The sticky header strip of a 详细列表. It shares [`detail_row`]'s `gap_3` so the header cells
/// line up exactly with the data cells.
pub(crate) fn detail_header_row(theme: Theme) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .w_full()
        .h(px(DETAIL_HEADER_HEIGHT))
        .px_3()
        .flex_none()
        .bg(rgb(theme.header_bg))
        .border_b_1()
        .border_color(rgb(theme.border))
}

/// One clickable header cell of a 详细列表. `sort` is `Some(descending)` when this column is the
/// active sort, drawing the up/down arrow.
pub(crate) fn detail_header_cell(
    id: impl Into<SharedString>,
    label: String,
    sort: Option<bool>,
    theme: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (icon, color, opacity) = match sort {
        Some(true) => (IconName::SortDescending, theme.primary, 1.0),
        Some(false) => (IconName::SortAscending, theme.primary, 1.0),
        None => (IconName::ChevronsUpDown, theme.text_muted, 0.5),
    };
    div()
        .id(id.into())
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .h_full()
        .cursor_pointer()
        .whitespace_nowrap()
        .text_size(px(12.0))
        .text_color(rgb(theme.text_muted))
        .hover(move |style| style.text_color(rgb(theme.text)))
        .on_click(on_click)
        .child(label)
        .child(
            Icon::new(icon)
                .w(px(11.0))
                .h(px(11.0))
                .flex_none()
                .text_color(rgb(color))
                .opacity(opacity),
        )
}

/// One 详细列表 cell's comparable value, for a header-click sort. Pages map their columns onto
/// these (a missing value, text compared case-insensitively, or a number). The ordering is total,
/// so the key can be handed to `sort_by_cached_key`.
#[derive(Clone)]
pub(crate) enum SortKey {
    /// No value (e.g. a table whose status did not load). Ascending, these come first.
    Empty,
    Text(String),
    /// A number carrying a value that may be absent; `None` sorts as [`SortKey::Empty`].
    Number(Option<f64>),
}

impl SortKey {
    /// A text key (case-insensitive; an empty string counts as missing).
    pub(crate) fn text(value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() {
            Self::Empty
        } else {
            Self::Text(value.to_lowercase())
        }
    }

    /// A text key from an optional value.
    pub(crate) fn opt_text(value: Option<&str>) -> Self {
        value.map(Self::text).unwrap_or(Self::Empty)
    }

    /// A number key from an optional value.
    pub(crate) fn number(value: Option<f64>) -> Self {
        match value {
            Some(value) => Self::Number(Some(value)),
            None => Self::Empty,
        }
    }
}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (Self::Empty, Self::Empty) => Ordering::Equal,
            (Self::Empty, _) => Ordering::Less,
            (_, Self::Empty) => Ordering::Greater,
            (Self::Number(left), Self::Number(right)) => left
                .unwrap_or(f64::NEG_INFINITY)
                .total_cmp(&right.unwrap_or(f64::NEG_INFINITY)),
            (Self::Text(left), Self::Text(right)) => left.cmp(right),
            // Mixed kinds never come from one column; order them deterministically anyway.
            (Self::Text(_), Self::Number(_)) => Ordering::Less,
            (Self::Number(_), Self::Text(_)) => Ordering::Greater,
        }
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for SortKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for SortKey {}

/// The active sort of one 详细列表: which column, and whether it runs descending. A page keeps one
/// per list, clicks a header through [`toggle`](Self::toggle), passes
/// [`indicator`](Self::indicator) to [`detail_header_cell`] and orders its rows with
/// [`sort_by`](Self::sort_by). The first column (usually the name) is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DetailSort {
    pub(crate) column: usize,
    pub(crate) descending: bool,
}

impl DetailSort {
    /// A click on `column`'s header: the same column flips the direction, another one starts
    /// ascending there.
    pub(crate) fn toggle(&mut self, column: usize) {
        if self.column == column {
            self.descending = !self.descending;
        } else {
            self.column = column;
            self.descending = false;
        }
    }

    /// What `column`'s header cell paints: `Some(descending)` while it is the active sort.
    pub(crate) fn indicator(&self, column: usize) -> Option<bool> {
        (self.column == column).then_some(self.descending)
    }

    /// Order `items` by `key`, which yields the active column's value for an item. The key is
    /// computed once per item (a table can have thousands of rows, and its key is the column's
    /// text), not once per comparison.
    pub(crate) fn sort_by<T>(&self, items: &mut [T], mut key: impl FnMut(&T) -> SortKey) {
        if self.descending {
            items.sort_by_cached_key(|item| std::cmp::Reverse(key(item)));
        } else {
            items.sort_by_cached_key(|item| key(item));
        }
    }
}

/// The base styling of one 详细列表 row. The caller adds its column cells and the click handlers.
pub(crate) fn detail_row(
    id: impl Into<SharedString>,
    selected: bool,
    theme: Theme,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .w_full()
        .h(px(DETAIL_ROW_HEIGHT))
        .px_3()
        .flex_none()
        .border_b_1()
        .border_color(rgb(theme.grid_line))
        .cursor_pointer()
        .when(selected, move |style| style.bg(rgb(theme.brand_muted)))
        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
}

/// The maximum number of characters a 详细列表 cell displays before it is shortened with an
/// ellipsis. The full value is shown in a hover tooltip when it is cut.
pub(crate) const DETAIL_CELL_MAX_CHARS: usize = 50;

/// One 详细列表 cell's text: clipped to its column, shortened to [`DETAIL_CELL_MAX_CHARS`]
/// characters with a trailing ellipsis when it is longer, and showing the full value in a hover
/// tooltip in that case. `id` must be unique across the page.
pub(crate) fn detail_cell_text(
    id: impl Into<SharedString>,
    text: impl Into<String>,
) -> Stateful<Div> {
    let full = text.into();
    match shorten_detail_text(&full) {
        Some(short) => div()
            .id(id.into())
            .min_w(px(0.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .child(short)
            .tooltip(move |_, cx| cx.new(|_| DetailTextTooltip(full.clone())).into()),
        None => div()
            .id(id.into())
            .min_w(px(0.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .child(full),
    }
}

/// `Some(shortened)` when `text` is longer than [`DETAIL_CELL_MAX_CHARS`] characters, else `None`.
/// Counted in characters, not bytes, so CJK text is not split mid-character.
fn shorten_detail_text(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(DETAIL_CELL_MAX_CHARS).collect();
    chars.next().is_some().then(|| format!("{head}…"))
}

/// The hover tooltip showing a 详细列表 cell's full value when it was shortened.
struct DetailTextTooltip(String);

impl Render for DetailTextTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = gpui_kit::component::Theme::global(cx);
        div()
            .max_w(px(420.0))
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .text_size(px(12.0))
            .text_color(theme.popover_foreground)
            .child(self.0.clone())
    }
}

/// The row of columns of a 平铺网格. Add one [`grid_column`] per column and let the caller put the
/// whole thing inside a [`ColumnGrid::scroller`].
pub(crate) fn grid_columns() -> Div {
    div().flex().flex_row().items_start().gap_1().p_1()
}

/// One vertical column of a 平铺网格. Items stack with no vertical gap so the shared row height is
/// the only spacing — every page's grid therefore has the same density.
pub(crate) fn grid_column() -> Div {
    div().flex().flex_col()
}

/// The base styling of one item of a 平铺网格 column, at an explicit width. The caller adds the
/// icon and label.
pub(crate) fn grid_item_sized(
    id: impl Into<SharedString>,
    selected: bool,
    theme: Theme,
    width: f32,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .w(px(width))
        .h(px(GRID_ROW_HEIGHT))
        .px_1()
        .flex_none()
        .rounded_sm()
        .cursor_pointer()
        .when(selected, move |style| style.bg(rgb(theme.brand_muted)))
        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
}

/// An approximate rendered width of `text` at the 12.5px UI font. The metrics deliberately
/// over-estimate a little (13px per CJK glyph, 7.5px per ASCII character): an under-estimate made
/// a fitted 平铺网格 tile a few pixels too narrow, clipping the last glyph of a long name.
pub(crate) fn approx_text_width(text: &str) -> f32 {
    text.chars()
        .map(|character| {
            if (character as u32) >= 0x1100 {
                13.0
            } else {
                7.5
            }
        })
        .sum()
}

/// The tile width for a 平铺网格 whose longest name is `longest` pixels wide: the default/minimum
/// plus the leading icon, the gap and the horizontal padding, capped at
/// [`GRID_ITEM_MAX_WIDTH`]. The extra room keeps a name measured by [`approx_text_width`] from
/// ending flush against the tile edge (which clipped its last character).
pub(crate) fn grid_item_width(longest: f32) -> f32 {
    (longest + 38.0).clamp(GRID_ITEM_WIDTH, GRID_ITEM_MAX_WIDTH)
}

/// The horizontal scroll state of a 平铺网格: the column-major list scrolls sideways, so it needs
/// its own offset plus the drag state of the scrollbar thumb (gpui paints no scrollbars for
/// `overflow_x_scroll`, so the grid draws and drives its own).
#[derive(Default)]
pub(crate) struct ColumnGrid {
    scroll: ScrollHandle,
    grab: Option<f32>,
    /// The scroller's last measured height. A freshly built grid has no bounds until its first
    /// paint, so the list that replaces another one seeds this from the previous grid: the first
    /// frame then already wraps its columns at the real height instead of laying out one tall
    /// clipped column and reflowing a frame later.
    viewport: Cell<f32>,
}

/// Scroll `scroll` vertically just enough that `item` (window bounds from the last frame) is fully
/// visible. gpui's offset is negative as the content scrolls down and stays in `-max..0`.
pub(crate) fn reveal_vertically(scroll: &ScrollHandle, item: Bounds<Pixels>) {
    let view = scroll.bounds();
    let mut offset = scroll.offset();
    if item.top() < view.top() {
        offset.y += view.top() - item.top();
    } else if item.bottom() > view.bottom() {
        offset.y -= item.bottom() - view.bottom();
    } else {
        return;
    }
    offset.y = offset.y.min(px(0.0)).max(-scroll.max_offset().y);
    scroll.set_offset(offset);
}

/// Scroll `scroll` horizontally just enough that `item` is fully visible (see
/// [`reveal_vertically`]).
pub(crate) fn reveal_horizontally(scroll: &ScrollHandle, item: Bounds<Pixels>) {
    let view = scroll.bounds();
    let mut offset = scroll.offset();
    if item.left() < view.left() {
        offset.x += view.left() - item.left();
    } else if item.right() > view.right() {
        offset.x -= item.right() - view.right();
    } else {
        return;
    }
    offset.x = offset.x.min(px(0.0)).max(-scroll.max_offset().x);
    scroll.set_offset(offset);
}

impl DetailScroll {
    /// Scroll the rows so `row` is visible after a keyboard move.
    pub(crate) fn reveal(&self, row: Bounds<Pixels>) {
        reveal_vertically(&self.vscroll, row);
    }

    /// The rows viewport's height, for paging by a screenful.
    pub(crate) fn viewport_height(&self) -> f32 {
        f32::from(self.vscroll.bounds().size.height)
    }

    /// Return both axes to the top-left, e.g. when the list switches to another category.
    pub(crate) fn reset(&self) {
        self.vscroll.set_offset(Point::new(px(0.0), px(0.0)));
        self.hscroll.set_offset(Point::new(px(0.0), px(0.0)));
    }
}

impl ColumnGrid {
    /// A grid whose first render already knows the viewport height of the list it replaces.
    pub(crate) fn with_viewport(height: f32) -> Self {
        Self {
            viewport: Cell::new(height),
            ..Self::default()
        }
    }

    /// Scroll the columns so `item` is visible after a keyboard move.
    pub(crate) fn reveal(&self, item: Bounds<Pixels>) {
        reveal_horizontally(&self.scroll, item);
    }

    /// The grid viewport's width, for paging by a screenful of columns.
    pub(crate) fn viewport_width(&self) -> f32 {
        f32::from(self.scroll.bounds().size.width)
    }

    /// The grid viewport's height: the scroller's own bounds once it has been laid out, otherwise
    /// the height remembered from the list it replaced (or the one this grid measured before a
    /// rebuild). Zero only for a grid that has never been painted anywhere.
    pub(crate) fn viewport_height(&self) -> f32 {
        let measured = f32::from(self.scroll.bounds().size.height);
        if measured > 0.0 {
            self.viewport.set(measured);
            measured
        } else {
            self.viewport.get()
        }
    }

    /// How many item rows fit in one column at the current viewport height.
    pub(crate) fn rows_per_column(&self) -> usize {
        self.rows_per_column_with(GRID_ROW_HEIGHT)
    }

    /// How many item rows of `row_height` fit in one column at the current viewport height.
    pub(crate) fn rows_per_column_with(&self, row_height: f32) -> usize {
        let viewport = self.viewport_height();
        if viewport <= 0.0 {
            return 30;
        }
        let rows = ((viewport - GRID_BOTTOM_MARGIN) / row_height).floor() as usize;
        rows.max(1)
    }

    /// Return the list to its first column, e.g. when it switches to another category.
    pub(crate) fn reset(&self) {
        self.scroll.set_offset(Point::new(px(0.0), px(0.0)));
    }

    /// The scroller that holds the [`grid_columns`] body.
    ///
    /// gpui only feeds an `overflow_x_scroll` container a horizontal delta, so a plain mouse wheel
    /// over a column-major list does nothing on its own. gpui-kit's `ScrollableMask` maps the
    /// wheel's vertical lines onto the horizontal axis (trackpad gestures and horizontal wheels
    /// are left to the scroller, and it hands the wheel to the parent at the edge).
    pub(crate) fn scroller(
        &self,
        id: impl Into<SharedString>,
        content: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .relative()
            .flex()
            .flex_col()
            .items_start()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(
                div()
                    .id(id.into())
                    .flex()
                    .flex_col()
                    .items_start()
                    .size_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll)
                    .child(content),
            )
            .child(gpui_kit::base::ScrollableMask::vertical_wheel(&self.scroll))
    }

    /// Capture the scrollbar thumb at `mouse_x` and jump the view there. Returns whether a drag is
    /// now active (so the caller can re-render).
    pub(crate) fn begin(&mut self, mouse_x: Pixels) -> bool {
        let bounds = self.scroll.bounds();
        let viewport = f32::from(bounds.size.width);
        let max = f32::from(self.scroll.max_offset().x);
        let (thumb_len, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return false;
        }
        let scroll = -f32::from(self.scroll.offset().x);
        let thumb_x = (scroll / max) * travel;
        let relative = f32::from(mouse_x) - f32::from(bounds.left());
        let grab = if relative >= thumb_x && relative <= thumb_x + thumb_len {
            relative - thumb_x
        } else {
            thumb_len / 2.0
        };
        self.grab = Some(grab);
        self.set(relative, grab);
        true
    }

    /// Continue a scrollbar drag. Returns whether anything changed.
    pub(crate) fn drag(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(grab) = self.grab else {
            return false;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.grab = None;
            return true;
        }
        let relative = f32::from(event.position.x) - f32::from(self.scroll.bounds().left());
        self.set(relative, grab);
        true
    }

    /// End a scrollbar drag. Returns whether one was active.
    pub(crate) fn end(&mut self) -> bool {
        self.grab.take().is_some()
    }

    /// The horizontal scrollbar track; the caller attaches the mouse-down handler.
    pub(crate) fn scrollbar(&self, id: impl Into<SharedString>, theme: Theme) -> Stateful<Div> {
        let viewport = f32::from(self.scroll.bounds().size.width);
        let max = f32::from(self.scroll.max_offset().x);
        let scroll = -f32::from(self.scroll.offset().x);
        let (thumb_left, thumb_len) = if max > 0.0 {
            scrollbar_fractions(viewport, max, scroll)
        } else {
            (0.0, 0.0)
        };
        hscrollbar_track(id, theme, thumb_left, thumb_len)
    }

    /// Whether the content is wider than the viewport, so the scrollbar should be shown.
    pub(crate) fn overflows(&self) -> bool {
        self.scroll.max_offset().x > px(0.0)
    }

    fn set(&self, relative: f32, grab: f32) {
        let viewport = f32::from(self.scroll.bounds().size.width);
        let max = f32::from(self.scroll.max_offset().x);
        let (_, travel) = scrollbar_thumb(viewport, max);
        if travel <= 0.0 {
            return;
        }
        let thumb_x = (relative - grab).clamp(0.0, travel);
        let scroll = thumb_x / travel * max;
        let y = self.scroll.offset().y;
        self.scroll.set_offset(Point::new(px(-scroll), y));
    }
}

/// A rounded, softly tinted square holding a row or tile's leading icon (Navicat-style grouping).
pub(crate) fn leading_icon_badge(icon: &'static str, color: u32, size: f32) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .rounded(px(size * 0.28))
        .bg(rgba((color << 8) | 0x1f))
        .child(
            svg()
                .path(icon)
                .w(px(size * 0.6))
                .h(px(size * 0.6))
                .flex_none()
                .text_color(rgb(color)),
        )
}

/// Frames a row's in-place rename editor. The editor itself is a bare (transparent) text field so
/// it lines up with the row's text, but on its own that leaves no hint that the row is editable;
/// this draws an input-like box around it (the way Navicat boxes a name being renamed). The caller
/// sizes the surrounding row to the row height, so the box fits it exactly and never bleeds into
/// the neighbouring rows.
pub(crate) fn rename_field(theme: Theme, input: impl IntoElement + 'static) -> impl IntoElement {
    div()
        .id("rename-field")
        .flex()
        .items_center()
        .w_full()
        .h_full()
        .px_1()
        .rounded(px(3.0))
        .bg(rgb(theme.input_bg))
        .border_1()
        .border_color(rgb(theme.brand))
        .overflow_hidden()
        // A mouse-down inside the editor must not reach the list/tree background handler, which
        // commits the rename; only a click *outside* the field should end editing.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().flex_1().min_w(px(0.0)).h_full().child(input))
}

/// A small muted chip, e.g. the `SQL` tag beside a saved query's name.
pub(crate) fn tag_chip(label: String, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .h(px(16.0))
        .px_1()
        .flex_none()
        .rounded(px(4.0))
        .bg(rgb(theme.button_bg))
        .text_color(rgb(theme.text_muted))
        .text_size(px(10.0))
        .child(label)
}

/// The translucent rubber-band rectangle drawn while a list marquee-selects. `origin` and `current`
/// are window-space points; the rectangle is clamped to `bounds` so it never spills over the pane.
pub(crate) fn marquee_rect(
    origin: Point<Pixels>,
    current: Point<Pixels>,
    bounds: Bounds<Pixels>,
    theme: Theme,
) -> Option<impl IntoElement> {
    let left = origin.x.min(current.x).max(bounds.left());
    let top = origin.y.min(current.y).max(bounds.top());
    let right = origin.x.max(current.x).min(bounds.right());
    let bottom = origin.y.max(current.y).min(bounds.bottom());
    let width = right - left;
    let height = bottom - top;
    if width <= px(0.0) || height <= px(0.0) {
        return None;
    }
    Some(
        div()
            .absolute()
            .left(left - bounds.left())
            .top(top - bounds.top())
            .w(width)
            .h(height)
            .rounded(px(2.0))
            .bg(rgba((theme.brand << 8) | 0x22))
            .border_1()
            .border_color(rgb(theme.brand)),
    )
}

#[cfg(test)]
mod tests {
    use super::{ColumnGrid, DetailSort, SortKey};

    #[test]
    fn a_seeded_grid_wraps_its_columns_before_its_own_scroller_is_measured() {
        // 464px leaves room for 22 rows at 20px with the 16px bottom margin.
        let grid = ColumnGrid::with_viewport(464.0);
        assert_eq!(grid.rows_per_column(), 22);
        // A grid that has never been painted anywhere keeps the old fallback.
        assert_eq!(ColumnGrid::default().rows_per_column(), 30);
    }

    #[test]
    fn a_detail_sort_toggles_direction_and_starts_elsewhere_ascending() {
        let mut sort = DetailSort::default();
        assert_eq!(
            sort,
            DetailSort {
                column: 0,
                descending: false
            }
        );
        assert_eq!(sort.indicator(0), Some(false));
        assert_eq!(sort.indicator(1), None);

        sort.toggle(0);
        assert_eq!(sort.indicator(0), Some(true));
        sort.toggle(0);
        assert_eq!(sort.indicator(0), Some(false));

        sort.toggle(2);
        assert_eq!(sort.column, 2);
        assert!(!sort.descending, "a new column starts ascending");
        assert_eq!(sort.indicator(0), None);
    }

    #[test]
    fn sort_keys_compare_text_case_insensitively_and_put_missing_values_first() {
        let mut items = vec!["Beta", "", "alpha", "Gamma"];
        let mut sort = DetailSort::default();
        sort.sort_by(&mut items, |text| SortKey::text(text));
        assert_eq!(items, vec!["", "alpha", "Beta", "Gamma"]);

        sort.toggle(0);
        sort.sort_by(&mut items, |text| SortKey::text(text));
        assert_eq!(items, vec!["Gamma", "Beta", "alpha", ""]);

        // Numbers sort numerically, not lexicographically, with missing values first.
        let mut numbers = vec![10u64, 2, 100];
        sort.toggle(0); // back to ascending
        sort.sort_by(&mut numbers, |value| SortKey::number(Some(*value as f64)));
        assert_eq!(numbers, vec![2, 10, 100]);

        let mut optional = vec![Some(3u64), None, Some(1)];
        sort.sort_by(&mut optional, |value| {
            SortKey::number(value.map(|value| value as f64))
        });
        assert_eq!(optional, vec![None, Some(1), Some(3)]);
    }
}
