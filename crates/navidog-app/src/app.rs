use std::sync::Arc;

use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, rgb};
use navidog_core::{ConnectionProfile, DriverRegistry};

use crate::runtime::Runtime;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<DriverRegistry>,
    #[allow(dead_code)]
    pub runtime: Arc<Runtime>,
    pub profiles: Arc<Vec<ConnectionProfile>>,
}

pub struct AppView {
    state: AppState,
}

impl AppView {
    pub fn new(state: AppState, _cx: &mut Context<'_, Self>) -> Self {
        Self { state }
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .size_full()
            .bg(rgb(0x1e1e1e))
            .text_color(rgb(0xd4d4d4))
            .child(sidebar(&self.state))
            .child(content())
    }
}

fn sidebar(state: &AppState) -> impl IntoElement {
    let mut items = div().flex().flex_col().gap_1().p_2();

    if state.profiles.is_empty() {
        items = items.child(div().child(t!("sidebar.no_connections").to_string()));
    } else {
        for profile in state.profiles.iter() {
            items = items.child(div().child(profile.name.clone()));
        }
    }

    div()
        .flex()
        .flex_col()
        .w(px(240.0))
        .h_full()
        .flex_none()
        .bg(rgb(0x252526))
        .border_r_1()
        .border_color(rgb(0x3c3c3c))
        .child(
            div()
                .p_2()
                .text_xl()
                .child(t!("sidebar.connections").to_string()),
        )
        .child(div().px_2().child(format!(
            "{}: {}",
            t!("sidebar.drivers"),
            state.registry.len()
        )))
        .child(items)
}

fn content() -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .h_full()
        .overflow_hidden()
        .p_4()
        .child(
            div()
                .text_xl()
                .child(t!("content.no_table_selected").to_string()),
        )
}
