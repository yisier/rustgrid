#[macro_use]
extern crate rust_i18n;

rust_i18n::i18n!("locales", fallback = "en");

mod app;
mod runtime;

use std::sync::Arc;

use gpui::{AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};
use navidog_core::DriverRegistry;

use crate::app::{AppState, AppView};
use crate::runtime::Runtime;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut registry = DriverRegistry::new();
    registry.register(Arc::new(navidog_mysql::MysqlDriver::new()));

    let profiles = navidog_config::ConfigStore::new()
        .and_then(|store| store.load_profiles())
        .unwrap_or_default();

    let state = AppState {
        registry: Arc::new(registry),
        runtime: Arc::new(Runtime::new()),
        profiles: Arc::new(profiles),
    };

    Application::new().run(move |cx| {
        let bounds = Bounds::centered(None, size(px(1100.0), px(720.0)), cx);
        let state = state.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |_window, cx| cx.new(|cx| AppView::new(state.clone(), cx)),
        )
        .expect("failed to open the main window");
    });
}
