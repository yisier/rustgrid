#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
extern crate rust_i18n;

rust_i18n::i18n!("locales", fallback = "en");

mod app;
mod assets;
#[cfg(target_os = "macos")]
mod dock_icon;
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
use std::time::Duration;

use gpui::{AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use gpui_kit::component::TitleBar;
use rustgrid_core::{BuiltinDriverSource, DriverRegistry};

use crate::app::AppView;
use crate::assets::Assets;
use crate::runtime::Runtime;

fn main() {
    // Dropping a SQLite database onto RustGrid.exe launches the executable with the file as its
    // argument; the running copy opens it when one exists, so a second process never starts.
    let startup_paths: Vec<std::path::PathBuf> = std::env::args_os()
        .skip(1)
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_file())
        .collect();
    // Double-clicking the executable must not open a second copy: the first instance owns the
    // single-instance lock and every later launch exits immediately. The guard is held for the
    // whole of `main`, so the lock lives as long as the app runs.
    let _instance = match single_instance::acquire() {
        Some(guard) => guard,
        None => {
            single_instance::forward_to_running(&startup_paths);
            return;
        }
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
    install_crash_log(config.root());
    let runtime = Arc::new(Runtime::new());

    let settings = app::load_startup_settings(&config);
    rust_i18n::set_locale(settings.language.locale());

    application().with_assets(Assets).run(move |cx| {
        #[cfg(target_os = "macos")]
        dock_icon::install();
        gpui_kit::init(cx);
        // Register the SQL editing rules (bracket pairs, indentation) for the query editor's
        // gpui-kit `Editor`. The grammar itself (tree-sitter `sql`) is linked in through the
        // `tree-sitter-sql` feature and needs no runtime registration.
        install_sql_language(cx);
        // Dialogs enter with a short in-place fade (`dialogs::DIALOG_ENTRANCE`), so motion must stay
        // on — this replaces the old global `set_reduce_motion(true)` kill switch, which made every
        // entrance instant. A user who wants no motion can restore that call here.
        //
        // `TooltipDefaults` is gpui-kit's application-wide tooltip timing; it covers the kit's
        // tooltips (buttons, clipboards, ...). Tooltips built on gpui's own `InteractiveElement::
        // tooltip` keep gpui's 500 ms default unless a caller overrides them per element with
        // `tooltip_show_delay`.
        gpui_kit::base::TooltipDefaults::new()
            .with_show_delay(Duration::from_millis(400))
            .install(cx);
        let bounds = Bounds::centered(None, size(px(1100.0), px(720.0)), cx);
        let registry = registry.clone();
        let config = config.clone();
        let runtime = runtime.clone();
        let startup_paths = startup_paths.clone();

        // `open_window` wraps the content in gpui-kit's `Root`, which now hosts dialogs,
        // sheets, notifications and tooltips automatically.
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(t!("app.title").to_string().into()),
                    ..TitleBar::title_bar_options()
                }),
                app_owns_titlebar_drag: true,
                ..Default::default()
            },
            cx,
            move |window, cx| {
                #[cfg(target_os = "windows")]
                win_resize::install(window);
                let view = cx
                    .new(|cx| AppView::new(registry.clone(), config.clone(), runtime.clone(), cx));
                // A database file the app was launched with (dragged onto the executable, or
                // opened through the shell's "Open with") becomes a connection like a dropped one.
                if !startup_paths.is_empty() {
                    view.update(cx, |app, cx| {
                        for path in startup_paths {
                            app.open_sqlite_path(path, cx);
                        }
                    });
                }
                let weak = view.downgrade();
                // The kit title bar's close button posts `WM_CLOSE` (and macOS' native close goes
                // through the same hook), so the unsaved-work confirmation has to live here rather
                // than on a custom button. It covers Alt+F4 as well.
                window.on_window_should_close(cx, move |_window, cx| {
                    let mut allow = true;
                    let _ = weak.update(cx, |app, cx| {
                        allow = app.should_close_main_window(cx);
                    });
                    allow
                });
                view
            },
        )
        .expect("failed to open the main window");
    });
}

/// The GPUI application this build launches.
///
/// On macOS the accessibility (AccessKit) adapter is deliberately left out. GPUI installs
/// `accesskit_macos::SubclassingAdapter` on every window: it swaps the window content view's
/// Objective-C class at runtime and swaps it back when the window is torn down. That class restore
/// corrupts the KVO state AppKit's Touch Bar bookkeeping (`_NSTouchBarFinderObservation`, key path
/// `nextResponder`) holds on the view, so closing *any* window on a Mac with a Touch Bar aborts the
/// process with an uncaught `NSRangeException` raised from `-[_NSTouchBarFinderObservation
/// invalidate]` during a display-cycle flush. That is what made 保存并连接 crash: saving closes the
/// connection window. Upstream knows about it (zed-industries/zed#64819, longbridge/gpui-kit#3192)
/// and the fix — replacing the subclassing adapter, zed-industries/zed#65186 — is not merged yet,
/// so no released `gpui-fast` carries it. Skipping `a11y_init` altogether is the only in-process way
/// to avoid it; the cost is that VoiceOver cannot read the app. Remove this as soon as `gpui-fast`
/// ships the fix.
#[cfg(target_os = "macos")]
fn application() -> gpui::Application {
    gpui::Application::new_inaccessible(gpui_kit::platform::current_platform(false))
}

/// Everywhere else this is gpui-kit's own entry point: accessibility stays enabled.
#[cfg(not(target_os = "macos"))]
fn application() -> gpui::Application {
    gpui_kit::application()
}

/// Append a panic report to `crash.log` under the config directory before the default hook prints
/// it. A release build is a Windows GUI subsystem binary with no console and macOS shows the crash
/// only in Console, so a user report of a "闪退" otherwise carries no location. The log names the
/// panic's file/line and, when the build kept symbols, a backtrace.
fn install_crash_log(root: &std::path::Path) {
    use std::io::Write as _;

    let path = root.join("crash.log");
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown location".to_string());
        let report = format!(
            "[{}] RustGrid {} panic at {location}\n{info}\n{}\n\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            env!("CARGO_PKG_VERSION"),
            std::backtrace::Backtrace::force_capture()
        );
        if let Some(directory) = path.parent() {
            let _ = std::fs::create_dir_all(directory);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = file.write_all(report.as_bytes());
        }
        default_hook(info);
    }));
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
        .with(Arc::new(rustgrid_oracle::OracleDriver::new()))
        .with(Arc::new(rustgrid_redis::RedisDriver::new()))
        .with(Arc::new(rustgrid_mongodb::MongoDriver::new()));
    #[cfg(feature = "driver-odbc")]
    let drivers = drivers.with(Arc::new(rustgrid_odbc::OdbcDriver::new()));
    registry.register_source(&drivers);
    registry
}

#[cfg(test)]
mod tests {
    use super::builtin_registry;
    use rustgrid_core::{AdvancedFields, ConnectionFieldKind, ConnectionHomePage, ConnectionPage};

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

            // A standard field is renamed at most once, and any `visible_when` control must be a
            // declared option whose choices actually include the allowed values.
            let mut renamed = Vec::new();
            for entry in &spec.labels {
                assert!(!entry.label_key.is_empty(), "{label}: empty label key");
                assert!(
                    !renamed.contains(&entry.field),
                    "{label}: standard field renamed twice"
                );
                renamed.push(entry.field);
                if let Some((control, allowed)) = entry.visible_when {
                    assert!(!allowed.is_empty(), "{label}: empty visible_when list");
                    let Some(control_field) =
                        spec.options.iter().find(|field| field.key == control)
                    else {
                        panic!("{label}: standard field gated on undeclared option {control}");
                    };
                    if let ConnectionFieldKind::Select(choices) = &control_field.kind {
                        for value in allowed {
                            assert!(
                                choices.iter().any(|choice| choice.value == *value),
                                "{label}: standard field gated on {control}={value}, not one of \
                                 its choices"
                            );
                        }
                    }
                }
            }

            // 高级 must never be an empty page: it needs either a supported standard field or an
            // engine option placed on it (this is why ODBC declares no 高级 tab at all).
            if pages.contains(&ConnectionPage::Advanced) {
                let has_standard = spec.advanced != AdvancedFields::NONE;
                let has_option = spec
                    .options
                    .iter()
                    .any(|option| option.page == ConnectionPage::Advanced);
                assert!(
                    has_standard || has_option,
                    "{label}: 高级 page has nothing to show"
                );
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

    /// MongoDB is a document store: the registration must advertise that capability and keep the
    /// SQL-only surfaces (designer, `.rgbak` backup) off.
    #[test]
    fn mongodb_is_registered_as_a_document_store() {
        use rustgrid_core::{ConnectionProfile, DriverCapability, DriverId};

        let registry = builtin_registry();
        let driver = registry
            .get(&DriverId::new("mongodb"))
            .expect("MongoDB driver is registered");
        let descriptor = driver.descriptor();
        assert_eq!(descriptor.default_port, 27017);
        assert!(
            descriptor.capabilities.has(DriverCapability::Documents),
            "MongoDB must advertise the document-store capability"
        );
        for absent in [
            DriverCapability::Users,
            DriverCapability::Routines,
            DriverCapability::Schemas,
        ] {
            assert!(
                !descriptor.capabilities.has(absent),
                "MongoDB must not advertise {absent:?}"
            );
        }
        let profile = ConnectionProfile {
            id: "test".into(),
            name: "test".into(),
            driver: DriverId::new("mongodb"),
            host: String::new(),
            port: 27017,
            username: String::new(),
            database: None,
            options: Default::default(),
            settings: Default::default(),
        };
        assert!(!driver.supports_table_designer(&profile));
        assert!(!driver.supports_backup(&profile));
    }
}
