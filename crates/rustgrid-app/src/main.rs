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
mod single_instance;
mod sql;
mod theme;
#[cfg(target_os = "windows")]
mod win_resize;

use std::sync::Arc;

use gpui::{AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use rustgrid_core::{BuiltinDriverSource, DriverRegistry};

use crate::app::AppView;
use crate::assets::Assets;
use crate::runtime::Runtime;

fn main() {
    // Double-clicking the executable must not open a second copy: the first instance owns the
    // single-instance lock and every later launch exits immediately. The guard is held for the
    // whole of `main`, so the lock lives as long as the app runs.
    let _instance = match single_instance::acquire() {
        Some(guard) => guard,
        None => return,
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut registry = DriverRegistry::new();
    let drivers = BuiltinDriverSource::new()
        .with(Arc::new(rustgrid_mysql::MysqlDriver::new()))
        .with(Arc::new(rustgrid_mysql::MariaDbDriver::new()))
        .with(Arc::new(rustgrid_sqlite::SqliteDriver::new()))
        .with(Arc::new(rustgrid_sqlserver::SqlServerDriver::new()));
    #[cfg(feature = "driver-odbc")]
    let drivers = drivers.with(Arc::new(rustgrid_odbc::OdbcDriver::new()));
    registry.register_source(&drivers);
    let registry = Arc::new(registry);

    let config = Arc::new(
        rustgrid_config::ConfigStore::new().expect("failed to resolve the configuration directory"),
    );
    let runtime = Arc::new(Runtime::new());

    let settings = app::load_startup_settings(&config);
    rust_i18n::set_locale(settings.language.locale());

    gpui_kit::application().with_assets(Assets).run(move |cx| {
        gpui_kit::init(cx);
        // Register the SQL editing rules (bracket pairs, indentation) for the query editor's
        // gpui-kit `Editor`. The grammar itself (tree-sitter `sql`) is linked in through the
        // `tree-sitter-sql` feature and needs no runtime registration.
        install_sql_language(cx);
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

/// Register the SQL editing rules for gpui-kit's `Editor`. `set_language_config` replaces the
/// language's bracket pairs and indentation rules for the whole application; the tree-sitter
/// grammar is linked in by the `tree-sitter-sql` feature, so no parser setup is needed here.
fn install_sql_language(cx: &mut gpui::App) {
    use gpui_kit::component::input::language_config::LanguageConfig;
    use gpui_kit::component::input::{AutoClosingPair, BracketPair, set_language_config};

    let rules = LanguageConfig::default()
        .brackets([
            BracketPair::new("(", ")"),
            BracketPair::new("{", "}"),
            BracketPair::new("[", "]"),
        ])
        .auto_closing_pairs([
            AutoClosingPair::new("(", ")"),
            AutoClosingPair::new("{", "}"),
            AutoClosingPair::new("[", "]"),
            AutoClosingPair::new("'", "'"),
            AutoClosingPair::new("`", "`"),
        ]);
    set_language_config("sql", rules, cx);
}
