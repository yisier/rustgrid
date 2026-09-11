#[macro_use]
extern crate rust_i18n;

rust_i18n::i18n!("locales", fallback = "en");

mod app;
mod form;
mod runtime;
mod session;

use std::sync::Arc;

use gpui::{
    AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use navidog_core::DriverRegistry;

use crate::app::AppView;
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
    let registry = Arc::new(registry);

    let config = Arc::new(
        navidog_config::ConfigStore::new().expect("failed to resolve the configuration directory"),
    );
    let runtime = Arc::new(Runtime::new());

    Application::new().run(move |cx| {
        let bounds = Bounds::centered(None, size(px(1100.0), px(720.0)), cx);
        let registry = registry.clone();
        let config = config.clone();
        let runtime = runtime.clone();

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(t!("app.title").to_string().into()),
                    appears_transparent: false,
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |_window, cx| {
                cx.new(|cx| AppView::new(registry.clone(), config.clone(), runtime.clone(), cx))
            },
        )
        .expect("failed to open the main window");
    });
}
