//! A compact presentation for gpui-kit's calendar.
//!
//! gpui-kit's styled `Calendar` facade is sized for a full popup (28px day cells with 12px
//! padding), which is noticeably taller than the app's dense grid chrome. The styled facade does
//! not expose an item hook, but the unstyled `gpui_base::Calendar` it wraps does — so this
//! wrapper drives the base calendar, supplying the same localized labels and item decoration at
//! the app's metrics. All behavior (month/year views, navigation, selection, muted/disabled/
//! today/range flags) still lives in `CalendarState`; if gpui-kit changes its item API, this is
//! the only file to touch.

use chrono::Weekday;

use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement, SharedString, Window, prelude::*, px,
};
use gpui_kit::base::{
    Calendar, CalendarItem, CalendarItemKind, CalendarItemState, CalendarState, StyledExt,
};
use gpui_kit::component::{ActiveTheme, Icon, IconName};

/// Day cell width; seven of them must fit the calendar's content box.
const CELL_W: f32 = 24.0;
/// Day cell height (the kit's Small facade uses 28).
const DAY_H: f32 = 20.0;
/// Weekday header / prev-next cell height.
const WEEKDAY_H: f32 = 16.0;

/// The kit's calendar at the app's dense metrics, Monday-first.
pub(crate) fn compact_calendar(state: &Entity<CalendarState>) -> impl IntoElement {
    Calendar::new(("compact-calendar", state.entity_id()), state)
        .w_full()
        .first_day_of_week(Weekday::Mon)
        .gap_1()
        .label(label)
        .item(render_item)
}

/// Localized labels for the calendar's header and grids. The styled facade owns equivalent
/// translations under the kit's own `Calendar.*` keys; the app cannot reach that backend (its
/// `t!` resolves against the app's locales), so the same labels live in the app's locale files.
fn label(kind: CalendarItemKind, value: i32) -> SharedString {
    match kind {
        CalendarItemKind::Previous => "‹".into(),
        CalendarItemKind::Next => "›".into(),
        CalendarItemKind::Weekday => t!(format!("calendar.week.{value}")).to_string().into(),
        CalendarItemKind::Month | CalendarItemKind::MonthToggle => {
            t!(format!("calendar.month.{value}")).to_string().into()
        }
        _ => value.to_string().into(),
    }
}

/// Item decorator for [`compact_calendar`], mirroring gpui-kit's styled facade but with smaller
/// cells.
fn render_item(
    item: CalendarItem,
    state: CalendarItemState,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme();
    let muted_fg = theme.muted_foreground;
    let foreground = theme.foreground;
    let accent = theme.accent;
    let accent_fg = theme.accent_foreground;
    let primary = theme.primary;
    let primary_fg = theme.primary_foreground;
    let hover_bg = theme.tokens.secondary_hover;
    let radius = theme.radius;

    let kind = state.kind();
    let item = match kind {
        CalendarItemKind::Previous => item
            .clear_children()
            .child(Icon::new(IconName::ChevronLeft).size_3()),
        CalendarItemKind::Next => item
            .clear_children()
            .child(Icon::new(IconName::ChevronRight).size_3()),
        _ => item,
    };

    let item = item
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius / 2.0);
    let item = match kind {
        CalendarItemKind::Weekday => item
            .w(px(CELL_W))
            .h(px(WEEKDAY_H))
            .text_size(px(10.0))
            .text_color(muted_fg),
        CalendarItemKind::Day => item.w(px(CELL_W)).h(px(DAY_H)).text_size(px(11.0)),
        CalendarItemKind::Month | CalendarItemKind::Year => {
            item.w_full().my_0p5().text_size(px(11.0))
        }
        CalendarItemKind::MonthToggle | CalendarItemKind::YearToggle => {
            item.w_auto().px_1().text_size(px(11.0)).font_medium()
        }
        CalendarItemKind::Previous | CalendarItemKind::Next => {
            item.w(px(CELL_W)).h(px(WEEKDAY_H)).text_color(muted_fg)
        }
    };

    item.when(state.is_muted(), |this| this.text_color(muted_fg))
        .when(state.is_disabled(), |this| this.opacity(0.5))
        .when(state.is_in_range(), |this| {
            this.bg(accent).text_color(accent_fg)
        })
        .when(
            !state.is_active() && !state.is_disabled() && kind != CalendarItemKind::Weekday,
            |this| this.hover(|this| this.bg(hover_bg).text_color(foreground)),
        )
        .when(state.is_active(), |this| {
            this.bg(primary).text_color(primary_fg)
        })
        .when(state.is_today() && !state.is_active(), |this| {
            this.bg(accent).text_color(accent_fg)
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The header labels use dynamic `t!` keys; guard against them silently falling back to the
    /// raw key string.
    #[test]
    fn labels_are_localized() {
        rust_i18n::set_locale("zh-CN");
        assert_eq!(label(CalendarItemKind::Weekday, 1).to_string(), "一");
        assert_eq!(label(CalendarItemKind::Month, 3).to_string(), "3月");
        assert_eq!(label(CalendarItemKind::MonthToggle, 3).to_string(), "3月");

        rust_i18n::set_locale("en");
        assert_eq!(label(CalendarItemKind::Weekday, 1).to_string(), "Mo");
        assert_eq!(label(CalendarItemKind::Month, 3).to_string(), "Mar");
    }
}
