#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
extern crate rust_i18n;

rust_i18n::i18n!("locales", fallback = "en");

mod app;
mod assets;
mod form;
mod list_select;
mod runtime;
mod session;
mod sql;
mod theme;
#[cfg(target_os = "windows")]
mod win_resize;

use std::sync::Arc;

use gpui::{AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use rustgrid_core::DriverRegistry;

use crate::app::AppView;
use crate::assets::Assets;
use crate::runtime::Runtime;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut registry = DriverRegistry::new();
    registry.register(Arc::new(rustgrid_mysql::MysqlDriver::new()));
    let registry = Arc::new(registry);

    let config = Arc::new(
        rustgrid_config::ConfigStore::new().expect("failed to resolve the configuration directory"),
    );
    let runtime = Arc::new(Runtime::new());

    let settings = app::load_startup_settings(&config);
    rust_i18n::set_locale(settings.language.locale());

    gpui_kit::application().with_assets(Assets).run(move |cx| {
        gpui_kit::init(cx);
        // Dialogs (and gpui-kit's other animated chrome) should appear in place rather than
        // sliding/fading in; this is the engine's global switch for that.
        cx.set_reduce_motion(true);
        let bounds = Bounds::centered(None, size(px(1100.0), px(720.0)), cx);
        let registry = registry.clone();
        let config = config.clone();
        let runtime = runtime.clone();

        // `open_window` wraps the content in gpui-kit's `Root`, which now hosts dialogs,
        // sheets, notifications and tooltips automatically.
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(t!("app.title").to_string().into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                #[cfg(target_os = "windows")]
                win_resize::install(window);
                cx.new(|cx| AppView::new(registry.clone(), config.clone(), runtime.clone(), cx))
            },
        )
        .expect("failed to open the main window");
    });
}
