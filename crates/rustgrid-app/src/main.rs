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

    let registry = Arc::new(builtin_registry());

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

/// The driver registry this build ships with. `main` and the tests both use it, so the tests cover
/// exactly the drivers a user gets.
fn builtin_registry() -> DriverRegistry {
    let mut registry = DriverRegistry::new();
    let drivers = BuiltinDriverSource::new()
        .with(Arc::new(rustgrid_mysql::MysqlDriver::new()))
        .with(Arc::new(rustgrid_mysql::MariaDbDriver::new()))
        .with(Arc::new(rustgrid_sqlite::SqliteDriver::new()))
        .with(Arc::new(rustgrid_sqlserver::SqlServerDriver::new()))
        .with(Arc::new(rustgrid_postgresql::PostgresDriver::new()))
        .with(Arc::new(rustgrid_oracle::OracleDriver::new()));
    #[cfg(feature = "driver-odbc")]
    let drivers = drivers.with(Arc::new(rustgrid_odbc::OdbcDriver::new()));
    registry.register_source(&drivers);
    registry
}

#[cfg(test)]
mod tests {
    use super::builtin_registry;
    use rustgrid_core::{ConnectionFieldKind, ConnectionHomePage, ConnectionPage};

    /// Every built-in driver's connection-form spec must be internally consistent: each declared
    /// option sits on a page the engine actually shows, keys are unique, and every `visible_when`
    /// control is another declared option whose choice matches. This is exactly the class of bug
    /// that would leave a field silently unreachable.
    #[test]
    fn builtin_connection_forms_are_consistent() {
        let registry = builtin_registry();
        // Guard against the test silently running on an empty/partial registry.
        assert!(
            registry.len() >= 6,
            "expected the built-in drivers to be registered, got {}",
            registry.len()
        );
        for driver in registry.drivers_sorted() {
            let descriptor = driver.descriptor();
            let spec = &descriptor.connection_form;
            let label = format!("{} ({})", descriptor.display_name, descriptor.id);

            let mut pages = vec![ConnectionPage::General];
            pages.extend(spec.tabs.iter().copied());
            let mut unique = pages.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(pages.len(), unique.len(), "{label}: duplicate page");
            assert!(
                !spec.tabs.contains(&ConnectionPage::General),
                "{label}: General must be the first page only"
            );

            // The 常规 page's shape must agree with the driver's own file flag.
            assert_eq!(
                driver.is_file_based(),
                spec.home == ConnectionHomePage::File,
                "{label}: home page and is_file_based disagree"
            );

            let mut keys = Vec::new();
            for option in &spec.options {
                assert!(!option.key.is_empty(), "{label}: empty option key");
                assert!(
                    !keys.contains(&option.key),
                    "{label}: duplicate option key {}",
                    option.key
                );
                keys.push(option.key);
                assert!(
                    !option.label_key.is_empty(),
                    "{label}: option {} has no label key",
                    option.key
                );
                assert!(
                    pages.contains(&option.page),
                    "{label}: option {} is on an undeclared page",
                    option.key
                );
                if let ConnectionFieldKind::Select(choices) = &option.kind {
                    assert!(
                        !choices.is_empty(),
                        "{label}: select {} has no choices",
                        option.key
                    );
                }
            }

            // A `visible_when` control must be another declared option whose value matches one of
            // its choices, so the condition is actually reachable.
            for option in &spec.options {
                let Some((control, value)) = option.visible_when else {
                    continue;
                };
                let Some(control_field) = spec.options.iter().find(|field| field.key == control)
                else {
                    panic!(
                        "{label}: {} is gated on undeclared option {control}",
                        option.key
                    );
                };
                if let ConnectionFieldKind::Select(choices) = &control_field.kind {
                    assert!(
                        choices.iter().any(|choice| choice.value == value),
                        "{label}: {} is gated on {control}={value}, not one of its choices",
                        option.key
                    );
                }
            }

            // A standard field is renamed at most once.
            let mut renamed = Vec::new();
            for entry in &spec.labels {
                assert!(!entry.label_key.is_empty(), "{label}: empty label key");
                assert!(
                    !renamed.contains(&entry.field),
                    "{label}: standard field renamed twice"
                );
                renamed.push(entry.field);
            }

            // The engines that actually declare engine-specific fields must be covered here, so
            // the assertions above are never vacuous.
            if matches!(descriptor.id.as_str(), "oracle" | "sqlserver") {
                assert!(
                    !spec.options.is_empty(),
                    "{label}: expected engine-specific options"
                );
            }
        }
    }
}
