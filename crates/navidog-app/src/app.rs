use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, BoxShadow, ClickEvent, ClipboardItem, Context, Div, FocusHandle, FontWeight,
    HighlightStyle, ImageSource, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, Resource, ScrollHandle, SharedString, Stateful,
    StyledText, Svg, TextLayout, Window, WindowControlArea, deferred, div, img, prelude::*, px,
    rgb, rgba, svg,
};
use navidog_config::{AppSettings, ConfigStore, LanguageSetting, ThemeSetting};
use navidog_core::{Connection, ConnectionConfig, DriverRegistry, Error, PageRequest};

use crate::form::{ConnectionForm, FORM_FIELDS, FormField};
use crate::runtime::Runtime;
use crate::session::{
    Category, CategoryExpansion, ConnectionNode, ConnectionStatus, DatabaseNode, GridState,
    Loadable,
};
use crate::theme::Theme;

struct FormFocus {
    name: FocusHandle,
    host: FocusHandle,
    port: FocusHandle,
    username: FocusHandle,
    password: FocusHandle,
    database: FocusHandle,
}

impl FormFocus {
    fn get(&self, field: FormField) -> &FocusHandle {
        match field {
            FormField::Name => &self.name,
            FormField::Host => &self.host,
            FormField::Port => &self.port,
            FormField::Username => &self.username,
            FormField::Password => &self.password,
            FormField::Database => &self.database,
        }
    }
}

struct PasswordPrompt {
    index: usize,
    password: String,
    save_password: bool,
}

enum TestStatus {
    Idle,
    Testing,
    Success,
    Failed(String),
}

enum ContextTarget {
    Connection(usize),
    Database {
        connection_index: usize,
        database_index: usize,
    },
}

struct ContextMenu {
    target: ContextTarget,
    position: Point<Pixels>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DbCombo {
    Charset,
    Collation,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DbTab {
    General,
    Sql,
}

enum DbDialog {
    New {
        connection_index: usize,
        name: String,
        error: Option<String>,
    },
    Edit {
        connection_index: usize,
        database_index: usize,
        name: String,
        original_charset: String,
        original_collation: String,
        charset: String,
        collation: String,
        charsets: Vec<String>,
        collations: Vec<String>,
        tab: DbTab,
        loading: bool,
        error: Option<String>,
    },
    Delete {
        connection_index: usize,
        database_index: usize,
        name: String,
        error: Option<String>,
    },
}

struct ObjectList {
    connection_index: usize,
    database_index: usize,
    category: Category,
    selected: Option<String>,
}

#[derive(Clone, Copy, Default)]
struct FieldSelection {
    anchor: usize,
    cursor: usize,
}

impl FieldSelection {
    fn range(self) -> (usize, usize) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Normal,
    Default,
    Selected,
    Disabled,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MainTab {
    Tables,
    Views,
    Functions,
    Users,
    Queries,
    Backups,
}

const MAIN_TABS: [(MainTab, &str, &str); 6] = [
    (MainTab::Tables, "icons/tables.svg", "main.tables"),
    (MainTab::Views, "icons/views.svg", "main.views"),
    (MainTab::Functions, "icons/functions.svg", "main.functions"),
    (MainTab::Users, "icons/user.svg", "main.users"),
    (MainTab::Queries, "icons/queries.svg", "main.queries"),
    (MainTab::Backups, "icons/backups.svg", "main.backups"),
];

const PANEL_WIDTH: f32 = 620.0;
const FIELD_LABEL_WIDTH: f32 = 96.0;
const FIELD_TEXT_LEFT: f32 = 16.0 + FIELD_LABEL_WIDTH + 8.0 + 8.0;

pub struct AppView {
    registry: Arc<DriverRegistry>,
    config: Arc<ConfigStore>,
    runtime: Arc<Runtime>,
    connections: Vec<ConnectionNode>,
    grid: Option<GridState>,
    form: Option<ConnectionForm>,
    editing: Option<usize>,
    test_status: TestStatus,
    context_menu: Option<ContextMenu>,
    object_list: Option<ObjectList>,
    main_tab: MainTab,
    db_dialog: Option<DbDialog>,
    db_combo: Option<DbCombo>,
    db_focus: FocusHandle,
    db_sql_focus: FocusHandle,
    db_sql_layout: RefCell<TextLayout>,
    db_sql_text: RefCell<String>,
    db_sql_anchor: usize,
    db_sql_cursor: usize,
    db_sql_selecting: bool,
    form_selection: FieldSelection,
    form_selecting: bool,
    form_active_field: FormField,
    form_offset: Point<Pixels>,
    form_dragging: bool,
    form_drag_origin: Point<Pixels>,
    form_drag_base: Point<Pixels>,
    caret_visible: bool,
    caret_blink_running: bool,
    password_prompt: Option<PasswordPrompt>,
    password_focus: FocusHandle,
    password_focus_pending: bool,
    form_focus: FormFocus,
    page_size: u64,
    sidebar_scroll: ScrollHandle,
    grid_scroll: ScrollHandle,
    theme_setting: ThemeSetting,
    theme: Theme,
    language: LanguageSetting,
    selected: Option<String>,
}

impl AppView {
    pub fn new(
        registry: Arc<DriverRegistry>,
        config: Arc<ConfigStore>,
        runtime: Arc<Runtime>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let secrets = config.load_secrets().unwrap_or_default();

        let connections = config
            .load_profiles()
            .unwrap_or_default()
            .into_iter()
            .map(|profile| {
                let password = secrets.get(&profile.id).cloned();
                let password_saved = password.is_some();
                ConnectionNode {
                    profile,
                    password,
                    password_saved,
                    status: ConnectionStatus::Disconnected,
                    databases: Loadable::Idle,
                    expanded: false,
                }
            })
            .collect();

        let settings = config.load_settings().unwrap_or_default();
        let theme_setting = settings.theme;
        let language = settings.language;

        Self {
            registry,
            config,
            runtime,
            connections,
            grid: None,
            form: None,
            editing: None,
            test_status: TestStatus::Idle,
            context_menu: None,
            object_list: None,
            main_tab: MainTab::Tables,
            db_dialog: None,
            db_combo: None,
            db_focus: cx.focus_handle(),
            db_sql_focus: cx.focus_handle(),
            db_sql_layout: RefCell::new(TextLayout::default()),
            db_sql_text: RefCell::new(String::new()),
            db_sql_anchor: 0,
            db_sql_cursor: 0,
            db_sql_selecting: false,
            form_selection: FieldSelection::default(),
            form_selecting: false,
            form_active_field: FormField::Name,
            form_offset: Point::default(),
            form_dragging: false,
            form_drag_origin: Point::default(),
            form_drag_base: Point::default(),
            caret_visible: true,
            caret_blink_running: false,
            password_prompt: None,
            password_focus: cx.focus_handle(),
            password_focus_pending: false,
            form_focus: FormFocus {
                name: cx.focus_handle(),
                host: cx.focus_handle(),
                port: cx.focus_handle(),
                username: cx.focus_handle(),
                password: cx.focus_handle(),
                database: cx.focus_handle(),
            },
            page_size: 100,
            sidebar_scroll: ScrollHandle::new(),
            grid_scroll: ScrollHandle::new(),
            theme_setting,
            theme: Theme::dark(),
            language,
            selected: None,
        }
    }

    fn set_theme(&mut self, setting: ThemeSetting, cx: &mut Context<'_, Self>) {
        if self.theme_setting == setting {
            return;
        }
        self.theme_setting = setting;
        let _ = self.config.save_settings(&AppSettings {
            theme: setting,
            language: self.language,
        });
        cx.notify();
    }

    fn set_language(&mut self, language: LanguageSetting, cx: &mut Context<'_, Self>) {
        if self.language == language {
            return;
        }
        self.language = language;
        rust_i18n::set_locale(language.locale());
        let _ = self.config.save_settings(&AppSettings {
            theme: self.theme_setting,
            language,
        });
        cx.notify();
    }

    fn connection_arc(&self, index: usize) -> Option<Arc<dyn Connection>> {
        match &self.connections.get(index)?.status {
            ConnectionStatus::Connected(connection) => Some(connection.clone()),
            _ => None,
        }
    }

    fn toggle_connection(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let state = match self.connections.get(index).map(|node| &node.status) {
            Some(ConnectionStatus::Connected(_)) => 1,
            Some(ConnectionStatus::Connecting) => 2,
            _ => 0,
        };

        match state {
            1 => self.disconnect(index, cx),
            2 => {}
            _ => self.connect(index, cx),
        }
    }

    fn toggle_expand(&mut self, index: usize) {
        if let Some(node) = self.connections.get_mut(index)
            && matches!(&node.status, ConnectionStatus::Connected(_))
        {
            node.expanded = !node.expanded;
        }
    }

    fn open_new_form(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.editing = None;
        self.form = Some(ConnectionForm::default());
        self.test_status = TestStatus::Idle;
        self.context_menu = None;
        let name_len = self
            .form
            .as_ref()
            .map(|form| form.name.chars().count())
            .unwrap_or(0);
        self.form_selection = FieldSelection {
            anchor: name_len,
            cursor: name_len,
        };
        self.form_selecting = false;
        self.form_active_field = FormField::Name;
        self.form_offset = Point::default();
        self.form_dragging = false;
        self.caret_visible = true;
        window.focus(&self.form_focus.name);
        cx.notify();
    }

    fn open_edit_form(&mut self, index: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        let (profile, password, password_saved) = match self.connections.get(index) {
            Some(node) => (
                node.profile.clone(),
                node.password.clone(),
                node.password_saved,
            ),
            None => return,
        };

        self.editing = Some(index);
        self.form = Some(ConnectionForm::from_profile(
            &profile,
            password,
            password_saved,
        ));
        self.test_status = TestStatus::Idle;
        self.context_menu = None;
        let name_len = self
            .form
            .as_ref()
            .map(|form| form.name.chars().count())
            .unwrap_or(0);
        self.form_selection = FieldSelection {
            anchor: name_len,
            cursor: name_len,
        };
        self.form_selecting = false;
        self.form_active_field = FormField::Name;
        self.form_offset = Point::default();
        self.form_dragging = false;
        self.caret_visible = true;
        window.focus(&self.form_focus.name);
        cx.notify();
    }

    fn delete_connection(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.connections.len() {
            return;
        }

        self.disconnect(index, cx);
        self.connections.remove(index);
        self.grid = None;

        if let Some(editing) = self.editing {
            match editing.cmp(&index) {
                std::cmp::Ordering::Equal => {
                    self.editing = None;
                    self.form = None;
                }
                std::cmp::Ordering::Greater => self.editing = Some(editing - 1),
                std::cmp::Ordering::Less => {}
            }
        }

        let profiles: Vec<_> = self
            .connections
            .iter()
            .map(|node| node.profile.clone())
            .collect();
        let _ = self.config.save_profiles(&profiles);
        self.persist_secrets();

        cx.notify();
    }

    fn persist_secrets(&self) {
        let secrets: BTreeMap<String, String> = self
            .connections
            .iter()
            .filter(|node| node.password_saved)
            .filter_map(|node| {
                node.password
                    .clone()
                    .map(|password| (node.profile.id.clone(), password))
            })
            .collect();
        let _ = self.config.save_secrets(&secrets);
    }

    fn cycle_page_size(&mut self, cx: &mut Context<'_, Self>) {
        const SIZES: [u64; 4] = [50, 100, 200, 500];

        if let Some(grid) = self.grid.as_mut() {
            let position = SIZES
                .iter()
                .position(|size| *size == grid.page_size)
                .unwrap_or(1);
            grid.page_size = SIZES[(position + 1) % SIZES.len()];
            grid.page_index = 0;
        }

        if let Some(size) = self.grid.as_ref().map(|grid| grid.page_size) {
            self.page_size = size;
        }

        self.load_page(cx);
    }

    fn connect(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(profile) = self.connections.get(index).map(|node| node.profile.clone()) else {
            return;
        };
        let password = self
            .connections
            .get(index)
            .and_then(|node| node.password.clone());

        let Some(driver) = self.registry.get(&profile.driver) else {
            if let Some(node) = self.connections.get_mut(index) {
                node.status = ConnectionStatus::Failed(t!("error.driver_missing").to_string());
            }
            cx.notify();
            return;
        };

        if let Some(node) = self.connections.get_mut(index) {
            node.status = ConnectionStatus::Connecting;
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let config = ConnectionConfig {
                driver: profile.driver.clone(),
                host: profile.host.clone(),
                port: profile.port,
                username: profile.username.clone(),
                password,
                database: profile.database.clone(),
                options: profile.options.clone(),
            };

            let result = match runtime
                .spawn(async move { driver.connect(&config).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(connection) => {
                        if let Some(node) = view.connections.get_mut(index) {
                            node.status = ConnectionStatus::Connected(Arc::from(connection));
                            node.expanded = true;
                        }
                        view.load_databases(index, cx);
                    }
                    Err(error) => {
                        let authentication = matches!(&error, Error::Authentication(_));
                        let message = error.to_string();

                        if let Some(node) = view.connections.get_mut(index) {
                            node.status = ConnectionStatus::Failed(message);
                        }

                        if authentication {
                            view.password_prompt = Some(PasswordPrompt {
                                index,
                                password: String::new(),
                                save_password: true,
                            });
                            view.password_focus_pending = true;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn disconnect(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let connection = self.connection_arc(index);

        if let Some(node) = self.connections.get_mut(index) {
            node.status = ConnectionStatus::Disconnected;
            node.databases = Loadable::Idle;
            node.expanded = false;
        }

        if let Some(connection) = connection {
            let runtime = self.runtime.clone();
            cx.spawn(async move |_this, _cx| {
                let _ = runtime.spawn(async move { connection.close().await }).await;
            })
            .detach();
        }

        cx.notify();
    }

    fn load_databases(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(connection) = self.connection_arc(index) else {
            return;
        };

        if let Some(node) = self.connections.get_mut(index) {
            node.databases = Loadable::Loading;
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.list_databases().await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(node) = view.connections.get_mut(index) {
                    node.databases = match result {
                        Ok(databases) => Loadable::Loaded(
                            databases
                                .into_iter()
                                .map(|database| DatabaseNode {
                                    name: database.name,
                                    tables: Loadable::Idle,
                                    opened: false,
                                    expanded: false,
                                    categories: Default::default(),
                                })
                                .collect(),
                        ),
                        Err(error) => Loadable::Failed(error.to_string()),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let mut should_load = false;
        let mut just_opened = false;

        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            if database.opened {
                database.expanded = !database.expanded;
            } else {
                database.opened = true;
                database.expanded = true;
                just_opened = true;
                if matches!(database.tables, Loadable::Idle | Loadable::Failed(_)) {
                    should_load = true;
                }
            }
        }

        if should_load
            && let Some(connection) = self.connection_arc(connection_index)
            && let Some(database_name) = self.database_name(connection_index, database_index)
        {
            self.load_tables(
                connection_index,
                database_index,
                connection,
                database_name,
                cx,
            );
        }

        if just_opened {
            let category = match &self.object_list {
                Some(list)
                    if list.connection_index == connection_index
                        && list.database_index == database_index =>
                {
                    list.category
                }
                _ => match self.main_tab {
                    MainTab::Views => Category::Views,
                    _ => Category::Tables,
                },
            };
            self.object_list = Some(ObjectList {
                connection_index,
                database_index,
                category,
                selected: None,
            });
        }

        cx.notify();
    }

    fn toggle_category(
        &mut self,
        connection_index: usize,
        database_index: usize,
        category: Category,
        cx: &mut Context<'_, Self>,
    ) {
        let mut should_load = false;
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.categories.toggle(category);
            if matches!(category, Category::Tables | Category::Views)
                && matches!(database.tables, Loadable::Idle | Loadable::Failed(_))
            {
                should_load = true;
            }
        }

        if should_load
            && let Some(connection) = self.connection_arc(connection_index)
            && let Some(name) = self.database_name(connection_index, database_index)
        {
            self.load_tables(connection_index, database_index, connection, name, cx);
        }

        self.object_list = Some(ObjectList {
            connection_index,
            database_index,
            category,
            selected: None,
        });
        match category {
            Category::Tables => self.main_tab = MainTab::Tables,
            Category::Views => self.main_tab = MainTab::Views,
            _ => {}
        }
        cx.notify();
    }

    fn database_name(&self, connection_index: usize, database_index: usize) -> Option<String> {
        self.connections
            .get(connection_index)
            .and_then(|node| match &node.databases {
                Loadable::Loaded(databases) => {
                    databases.get(database_index).map(|db| db.name.clone())
                }
                _ => None,
            })
    }

    fn load_tables(
        &mut self,
        connection_index: usize,
        database_index: usize,
        connection: Arc<dyn Connection>,
        database_name: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.tables = Loadable::Loading;
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.list_tables(&database_name).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(node) = view.connections.get_mut(connection_index)
                    && let Loadable::Loaded(databases) = &mut node.databases
                    && let Some(database) = databases.get_mut(database_index)
                {
                    database.tables = match result {
                        Ok(tables) => Loadable::Loaded(tables),
                        Err(error) => Loadable::Failed(error.to_string()),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_table(
        &mut self,
        connection_index: usize,
        database: String,
        table: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };

        self.object_list = None;
        self.grid = Some(GridState {
            connection,
            database,
            table,
            page_index: 0,
            page_size: self.page_size,
            loading: false,
            error: None,
            columns: Vec::new(),
            rows: Vec::new(),
            total_rows: None,
        });

        self.load_page(cx);
    }

    fn load_page(&mut self, cx: &mut Context<'_, Self>) {
        let Some(grid) = self.grid.as_mut() else {
            return;
        };

        grid.loading = true;
        grid.error = None;

        let connection = grid.connection.clone();
        let database = grid.database.clone();
        let table = grid.table.clone();
        let page_index = grid.page_index;
        let page_size = grid.page_size;

        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    connection
                        .fetch_page(&database, &table, PageRequest::new(page_index, page_size))
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(grid) = view.grid.as_mut() {
                    grid.loading = false;
                    match result {
                        Ok(page) => {
                            grid.columns = page.columns;
                            grid.rows = page.rows;
                            grid.total_rows = page.total_rows;
                            grid.error = None;
                        }
                        Err(error) => {
                            grid.error = Some(error.to_string());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn next_page(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(grid) = self.grid.as_mut()
            && grid.has_next()
        {
            grid.page_index += 1;
        }
        self.load_page(cx);
    }

    fn prev_page(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(grid) = self.grid.as_mut() {
            grid.page_index = grid.page_index.saturating_sub(1);
        }
        self.load_page(cx);
    }

    fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        self.load_page(cx);
    }

    fn save_form(&mut self, cx: &mut Context<'_, Self>) {
        let Some(form) = self.form.take() else {
            return;
        };

        let password = if form.password.is_empty() {
            None
        } else {
            Some(form.password.clone())
        };
        let password_saved = form.save_password && password.is_some();

        let index = match self.editing.take() {
            Some(index) => {
                if let Some(node) = self.connections.get_mut(index) {
                    let mut profile = form.to_profile();
                    profile.id = node.profile.id.clone();
                    node.profile = profile;
                    node.password = password;
                    node.password_saved = password_saved;
                }
                self.disconnect(index, cx);
                index
            }
            None => {
                self.connections.push(ConnectionNode {
                    profile: form.to_profile(),
                    password,
                    password_saved,
                    status: ConnectionStatus::Disconnected,
                    databases: Loadable::Idle,
                    expanded: false,
                });
                self.connections.len() - 1
            }
        };

        let profiles: Vec<_> = self
            .connections
            .iter()
            .map(|node| node.profile.clone())
            .collect();
        let _ = self.config.save_profiles(&profiles);
        self.persist_secrets();

        self.connect(index, cx);
        cx.notify();
    }

    fn test_form(&mut self, cx: &mut Context<'_, Self>) {
        let Some(form) = self.form.as_ref() else {
            return;
        };

        let profile = form.to_profile();
        let password = if form.password.is_empty() {
            None
        } else {
            Some(form.password.clone())
        };

        let Some(driver) = self.registry.get(&profile.driver) else {
            self.test_status = TestStatus::Failed(t!("error.driver_missing").to_string());
            cx.notify();
            return;
        };

        self.test_status = TestStatus::Testing;
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let config = ConnectionConfig {
                driver: profile.driver.clone(),
                host: profile.host.clone(),
                port: profile.port,
                username: profile.username.clone(),
                password,
                database: profile.database.clone(),
                options: profile.options.clone(),
            };

            let status = match runtime
                .spawn(async move { driver.connect(&config).await })
                .await
            {
                Ok(Ok(connection)) => {
                    let _ = runtime.spawn(async move { connection.close().await }).await;
                    TestStatus::Success
                }
                Ok(Err(error)) => TestStatus::Failed(error.to_string()),
                Err(error) => TestStatus::Failed(Error::other(error).to_string()),
            };

            let _ = this.update(cx, |view, cx| {
                view.test_status = status;
                cx.notify();
            });
        })
        .detach();
    }

    fn form_key(
        &mut self,
        field: FormField,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.form.is_none() {
            return;
        }

        self.form_active_field = field;

        let keystroke = &event.keystroke;
        let command = keystroke.modifiers.control || keystroke.modifiers.platform;
        let shift = keystroke.modifiers.shift;

        let text = self
            .form
            .as_ref()
            .map(|form| form.value(field).to_string())
            .unwrap_or_default();
        let mut chars: Vec<char> = text.chars().collect();
        let len = chars.len();

        let mut selection = self.form_selection;
        selection.anchor = selection.anchor.min(len);
        selection.cursor = selection.cursor.min(len);
        let (start, end) = selection.range();

        if command {
            match keystroke.key.as_str() {
                "a" => {
                    self.set_selection(0, len, cx);
                }
                "c" => {
                    if start < end {
                        let selected: String = chars[start..end].iter().copied().collect();
                        cx.write_to_clipboard(ClipboardItem::new_string(selected));
                    }
                }
                "x" => {
                    if start < end {
                        let selected: String = chars[start..end].iter().copied().collect();
                        cx.write_to_clipboard(ClipboardItem::new_string(selected));
                        chars.drain(start..end);
                        self.set_field_text(field, &chars, start, cx);
                    }
                }
                "v" => {
                    if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        let pasted: Vec<char> = pasted
                            .chars()
                            .filter(|ch| *ch != '\n' && *ch != '\r' && *ch != '\t')
                            .collect();
                        if !pasted.is_empty() {
                            let mut next = Vec::with_capacity(chars.len() + pasted.len());
                            next.extend_from_slice(&chars[..start]);
                            next.extend_from_slice(&pasted);
                            next.extend_from_slice(&chars[end..]);
                            let caret = start + pasted.len();
                            self.set_field_text(field, &next, caret, cx);
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        match keystroke.key.as_str() {
            "enter" => self.save_form(cx),
            "tab" => {
                let position = FORM_FIELDS
                    .iter()
                    .position(|item| *item == field)
                    .unwrap_or(0);
                let target = if shift {
                    FORM_FIELDS[(position + FORM_FIELDS.len() - 1) % FORM_FIELDS.len()]
                } else {
                    FORM_FIELDS[(position + 1) % FORM_FIELDS.len()]
                };
                window.focus(self.form_focus.get(target));
                self.form_active_field = target;
                let target_len = self
                    .form
                    .as_ref()
                    .map(|form| form.value(target).chars().count())
                    .unwrap_or(0);
                self.form_selection = FieldSelection {
                    anchor: target_len,
                    cursor: target_len,
                };
                self.caret_visible = true;
                cx.notify();
            }
            "backspace" => {
                if start < end {
                    chars.drain(start..end);
                    self.set_field_text(field, &chars, start, cx);
                } else if start > 0 {
                    chars.remove(start - 1);
                    self.set_field_text(field, &chars, start - 1, cx);
                }
            }
            "delete" => {
                if start < end {
                    chars.drain(start..end);
                    self.set_field_text(field, &chars, start, cx);
                } else if start < len {
                    chars.remove(start);
                    self.set_field_text(field, &chars, start, cx);
                }
            }
            "left" => {
                let cursor = if shift {
                    selection.cursor.saturating_sub(1)
                } else if start < end {
                    start
                } else {
                    start.saturating_sub(1)
                };
                let anchor = if shift { selection.anchor } else { cursor };
                self.set_selection(anchor, cursor, cx);
            }
            "right" => {
                let cursor = if shift {
                    (selection.cursor + 1).min(len)
                } else if start < end {
                    end
                } else {
                    (start + 1).min(len)
                };
                let anchor = if shift { selection.anchor } else { cursor };
                self.set_selection(anchor, cursor, cx);
            }
            "home" => {
                let anchor = if shift { selection.anchor } else { 0 };
                self.set_selection(anchor, 0, cx);
            }
            "end" => {
                let anchor = if shift { selection.anchor } else { len };
                self.set_selection(anchor, len, cx);
            }
            "space" => self.insert_text(field, &chars, start, end, " ", cx),
            _ => {
                if let Some(insert) = keystroke.key_char.as_ref()
                    && !insert.is_empty()
                {
                    self.insert_text(field, &chars, start, end, insert, cx);
                }
            }
        }
    }

    fn set_selection(&mut self, anchor: usize, cursor: usize, cx: &mut Context<'_, Self>) {
        self.form_selection = FieldSelection { anchor, cursor };
        self.caret_visible = true;
        cx.notify();
    }

    fn insert_text(
        &mut self,
        field: FormField,
        chars: &[char],
        start: usize,
        end: usize,
        insert: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let insert: Vec<char> = insert.chars().collect();
        let mut next = Vec::with_capacity(chars.len() + insert.len());
        next.extend_from_slice(&chars[..start]);
        next.extend_from_slice(&insert);
        next.extend_from_slice(&chars[end..]);
        let caret = start + insert.len();
        self.set_field_text(field, &next, caret, cx);
    }

    fn set_field_text(
        &mut self,
        field: FormField,
        chars: &[char],
        caret: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(form) = self.form.as_mut() {
            form.set_value(field, chars.iter().copied().collect());
        }
        self.form_selection = FieldSelection {
            anchor: caret,
            cursor: caret,
        };
        self.caret_visible = true;
        cx.notify();
    }

    fn field_index_for_x(&self, text: &str, x: Pixels, window: &Window) -> usize {
        let char_count = text.chars().count();
        if char_count == 0 {
            return 0;
        }

        let viewport_width: f32 = window.viewport_size().width.into();
        let offset_x: f32 = self.form_offset.x.into();
        let panel_left = (viewport_width - PANEL_WIDTH) / 2.0 + offset_x;
        let text_left = panel_left + FIELD_TEXT_LEFT;
        let relative = f32::from(x) - text_left;
        if relative <= 0.0 {
            return 0;
        }

        let run = window.text_style().to_run(text.len());
        let layout = window
            .text_system()
            .layout_line(text, px(12.0), &[run], None);
        let byte = layout.closest_index_for_x(px(relative)).min(text.len());
        text[..byte].chars().count()
    }

    fn password_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let mut submit = false;

        if let Some(prompt) = self.password_prompt.as_mut() {
            let keystroke = &event.keystroke;
            if keystroke.modifiers.control || keystroke.modifiers.platform {
                return;
            }

            match keystroke.key.as_str() {
                "backspace" => {
                    prompt.password.pop();
                }
                "space" => prompt.password.push(' '),
                "enter" => submit = true,
                _ => {
                    if let Some(text) = keystroke.key_char.as_ref() {
                        prompt.password.push_str(text);
                    }
                }
            }
        }

        if submit {
            self.submit_password(cx);
        }

        cx.notify();
    }

    fn submit_password(&mut self, cx: &mut Context<'_, Self>) {
        let Some(prompt) = self.password_prompt.take() else {
            return;
        };

        let index = prompt.index;
        let password = if prompt.password.is_empty() {
            None
        } else {
            Some(prompt.password)
        };
        let password_saved = prompt.save_password && password.is_some();

        if let Some(node) = self.connections.get_mut(index) {
            node.password = password;
            node.password_saved = password_saved;
            node.status = ConnectionStatus::Disconnected;
        }

        self.persist_secrets();
        self.connect(index, cx);
        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let mut list = div().flex().flex_col();

        if self.connections.is_empty() {
            list = list.child(
                div()
                    .p_2()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("sidebar.no_connections").to_string()),
            );
        }

        for (index, node) in self.connections.iter().enumerate() {
            list = list.child(self.render_connection(index, node, cx));
        }

        div()
            .flex()
            .flex_col()
            .w(px(260.0))
            .h_full()
            .flex_none()
            .bg(rgb(theme.sidebar_bg))
            .border_r_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .py_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.sidebar_scroll)
                    .child(list),
            )
            .child(self.render_sidebar_footer(cx))
    }

    fn render_sidebar_footer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;

        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.text_muted))
                    .child(format!(
                        "{}: {}",
                        t!("sidebar.drivers"),
                        self.registry.len()
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .child(self.render_theme_button(
                        ThemeSetting::Light,
                        "theme-light",
                        t!("theme.light").to_string(),
                        cx,
                    ))
                    .child(self.render_theme_button(
                        ThemeSetting::Dark,
                        "theme-dark",
                        t!("theme.dark").to_string(),
                        cx,
                    ))
                    .child(self.render_theme_button(
                        ThemeSetting::System,
                        "theme-system",
                        t!("theme.system").to_string(),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .child(self.render_language_button(
                        LanguageSetting::En,
                        "lang-en",
                        "English".to_string(),
                        cx,
                    ))
                    .child(self.render_language_button(
                        LanguageSetting::ZhCn,
                        "lang-zh",
                        "中文".to_string(),
                        cx,
                    )),
            )
    }

    fn render_language_button(
        &self,
        language: LanguageSetting,
        id: &'static str,
        label: String,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let kind = if self.language == language {
            ButtonKind::Selected
        } else {
            ButtonKind::Normal
        };
        self.win_button(
            id,
            label,
            kind,
            cx.listener(move |this, _event, _window, cx| {
                this.set_language(language, cx);
            }),
        )
        .flex_1()
    }

    fn render_theme_button(
        &self,
        setting: ThemeSetting,
        id: &'static str,
        label: String,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let kind = if self.theme_setting == setting {
            ButtonKind::Selected
        } else {
            ButtonKind::Normal
        };
        self.win_button(
            id,
            label,
            kind,
            cx.listener(move |this, _event, _window, cx| {
                this.set_theme(setting, cx);
            }),
        )
        .flex_1()
    }

    fn render_connection(
        &self,
        index: usize,
        node: &ConnectionNode,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let conn_id = format!("conn-{index}");
        let selected = self.selected.as_deref() == Some(conn_id.as_str());
        let connected = matches!(&node.status, ConnectionStatus::Connected(_));

        let icon_color = match &node.status {
            ConnectionStatus::Connected(_) => theme.icon_connection,
            ConnectionStatus::Connecting => theme.warning,
            ConnectionStatus::Failed(_) => theme.danger,
            ConnectionStatus::Disconnected => theme.neutral,
        };

        let row = div()
            .id(SharedString::from(conn_id.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl_1()
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .overflow_hidden()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, event, _window, cx| {
                this.selected = Some(format!("conn-{index}"));
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                if double_click {
                    let connected = matches!(
                        this.connections.get(index).map(|node| &node.status),
                        Some(ConnectionStatus::Connected(_))
                    );
                    if connected {
                        this.toggle_expand(index);
                    } else {
                        this.connect(index, cx);
                    }
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.selected = Some(format!("conn-{index}"));
                    this.context_menu = Some(ContextMenu {
                        target: ContextTarget::Connection(index),
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            .child(if connected {
                tree_chevron(node.expanded, theme.chevron)
            } else {
                chevron_spacer()
            })
            .child(tree_icon("icons/connection.svg", icon_color))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(node.profile.name.clone()),
            );

        let body = div().flex().flex_col().child(row);

        let mut sub = div().flex().flex_col();

        if let ConnectionStatus::Failed(error) = &node.status {
            sub = sub.child(tree_message(error.clone(), 26.0, theme.danger));
        }

        if node.expanded {
            match &node.databases {
                Loadable::Idle => {}
                Loadable::Loading => {
                    sub = sub.child(tree_message(
                        t!("common.loading").to_string(),
                        26.0,
                        theme.text_muted,
                    ));
                }
                Loadable::Failed(error) => {
                    sub = sub.child(tree_message(error.clone(), 26.0, theme.danger));
                }
                Loadable::Loaded(databases) => {
                    for (database_index, database) in databases.iter().enumerate() {
                        sub = sub.child(self.render_database(index, database_index, database, cx));
                    }
                }
            }
        }

        body.child(sub)
    }

    fn action_button(
        &self,
        id: impl Into<SharedString>,
        label: String,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        self.win_button(id, label, ButtonKind::Normal, on_click)
    }

    fn render_database(
        &self,
        connection_index: usize,
        database_index: usize,
        database: &DatabaseNode,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let db_id = format!("db-{connection_index}-{database_index}");
        let selected = self.selected.as_deref() == Some(db_id.as_str());

        let row = div()
            .id(SharedString::from(db_id.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(18.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, event, _window, cx| {
                this.selected = Some(format!("db-{connection_index}-{database_index}"));
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                if double_click {
                    this.open_database(connection_index, database_index, cx);
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.selected = Some(format!("db-{connection_index}-{database_index}"));
                    this.context_menu = Some(ContextMenu {
                        target: ContextTarget::Database {
                            connection_index,
                            database_index,
                        },
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            .child(if database.opened {
                div()
                    .id(SharedString::from(format!(
                        "db-toggle-{connection_index}-{database_index}"
                    )))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.open_database(connection_index, database_index, cx);
                    }))
                    .child(tree_chevron(database.expanded, theme.chevron))
                    .into_any_element()
            } else {
                chevron_spacer()
            })
            .child(tree_icon(
                "icons/database.svg",
                if database.opened {
                    theme.icon_database_active
                } else {
                    theme.icon_database
                },
            ))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(database.name.clone()),
            );

        let mut sub = div().flex().flex_col();

        if database.expanded {
            match &database.tables {
                Loadable::Idle => {}
                Loadable::Loading => {
                    sub = sub.child(tree_message(
                        t!("common.loading").to_string(),
                        36.0,
                        theme.text_muted,
                    ));
                }
                Loadable::Failed(error) => {
                    sub = sub.child(tree_message(error.clone(), 36.0, theme.danger));
                }
                Loadable::Loaded(_) => {
                    for category in Category::ALL {
                        sub = sub.child(self.render_category(
                            connection_index,
                            database_index,
                            category,
                            database,
                            cx,
                        ));
                    }
                }
            }
        }

        div().flex().flex_col().child(row).child(sub)
    }

    fn render_category(
        &self,
        connection_index: usize,
        database_index: usize,
        category: Category,
        database: &DatabaseNode,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let expanded = database.categories.get(category);
        let cat_id = format!("cat-{connection_index}-{database_index}-{}", category.id());
        let selected = self.selected.as_deref() == Some(cat_id.as_str());
        let label = t!(category.label()).to_string();
        let icon_color = match category {
            Category::Tables => theme.icon_tables,
            Category::Views => theme.icon_views,
            Category::Functions => theme.icon_functions,
            Category::Queries => theme.icon_queries,
            Category::Backups => theme.icon_backups,
        };
        let has_objects = matches!(category, Category::Tables | Category::Views);
        let click_id = cat_id.clone();

        let row = div()
            .id(SharedString::from(cat_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(36.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.selected = Some(click_id.clone());
                this.toggle_category(connection_index, database_index, category, cx);
            }))
            .child(tree_chevron(expanded, theme.chevron))
            .child(tree_icon(category.icon_path(), icon_color))
            .child(div().child(label));

        let mut sub = div().flex().flex_col();

        if expanded {
            if !has_objects {
                sub = sub.child(tree_message(
                    t!("common.empty").to_string(),
                    52.0,
                    theme.text_muted,
                ));
            } else if let Loadable::Loaded(tables) = &database.tables {
                let want_view = category == Category::Views;
                let mut leaf_index = 0usize;
                for table in tables.iter() {
                    if matches!(table.kind, navidog_core::ObjectKind::View) != want_view {
                        continue;
                    }
                    sub = sub.child(self.render_table(
                        connection_index,
                        database_index,
                        leaf_index,
                        &database.name,
                        table,
                        cx,
                    ));
                    leaf_index += 1;
                }
            }
        }

        div().flex().flex_col().child(row).child(sub)
    }

    fn render_table(
        &self,
        connection_index: usize,
        database_index: usize,
        table_index: usize,
        database_name: &str,
        table: &navidog_core::TableInfo,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let is_view = matches!(table.kind, navidog_core::ObjectKind::View);
        let table_id = format!("tbl-{connection_index}-{database_index}-{table_index}");
        let selected = self.selected.as_deref() == Some(table_id.as_str());
        let table_name = table.name.clone();
        let database_name = database_name.to_string();
        let click_id = table_id.clone();

        div()
            .id(SharedString::from(table_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w_full()
            .h(px(22.0))
            .pl(px(54.0))
            .pr_2()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.selected = Some(click_id.clone());
                this.select_table(
                    connection_index,
                    database_name.clone(),
                    table_name.clone(),
                    cx,
                );
            }))
            .child(chevron_spacer())
            .child(tree_icon(
                if is_view {
                    "icons/views.svg"
                } else {
                    "icons/tables.svg"
                },
                if is_view {
                    theme.icon_view
                } else {
                    theme.icon_table
                },
            ))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(table.name.clone()),
            )
    }

    fn render_content(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let open_enabled = self
            .object_list
            .as_ref()
            .map(|list| list.selected.is_some())
            .unwrap_or(false);

        let body: AnyElement = if let Some(list) = self.object_list.as_ref() {
            self.render_object_body(list, cx).into_any_element()
        } else if let Some(grid) = self.grid.as_ref() {
            self.render_grid(grid, cx).into_any_element()
        } else {
            div().into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .bg(rgb(theme.editor_bg))
            .child(self.render_object_header())
            .child(self.render_object_toolbar(open_enabled, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(body),
            )
    }

    fn render_menu_bar(&self) -> impl IntoElement {
        let theme = self.theme;
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(26.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border));

        for (id, key) in [
            ("menu-file", "menu.file"),
            ("menu-tools", "menu.tools"),
            ("menu-view", "menu.view"),
            ("menu-help", "menu.help"),
        ] {
            bar = bar.child(
                div()
                    .id(id)
                    .px_3()
                    .py_0p5()
                    .rounded_sm()
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .child(t!(key).to_string()),
            );
        }

        bar
    }

    fn render_main_toolbar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.main_button(
                "main-connect",
                "icons/connection.svg",
                t!("main.connection").to_string(),
                false,
                true,
                cx.listener(|this, _event, window, cx| this.open_new_form(window, cx)),
            ))
            .child(self.main_button(
                "main-query",
                "icons/queries.svg",
                t!("main.new_query").to_string(),
                false,
                false,
                |_, _, _| {},
            ))
            .child(main_separator(theme));

        for (tab, icon, label_key) in MAIN_TABS {
            bar = bar.child(self.render_main_tab(tab, icon, t!(label_key).to_string(), cx));
        }

        bar
    }

    fn render_main_tab(
        &self,
        tab: MainTab,
        icon: &'static str,
        label: String,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let active = self.main_tab == tab;
        let enabled = matches!(tab, MainTab::Tables | MainTab::Views);
        self.main_button(
            SharedString::from(format!("main-tab-{}", tab as usize)),
            icon,
            label,
            active,
            enabled,
            cx.listener(move |this, _event, _window, cx| {
                if enabled {
                    this.select_main_tab(tab, cx);
                }
            }),
        )
    }

    fn main_button(
        &self,
        id: impl Into<SharedString>,
        icon: &'static str,
        label: String,
        active: bool,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = self.theme;
        let text_color = if active {
            theme.tree_selected_text
        } else if enabled {
            theme.text
        } else {
            theme.text_muted
        };

        div()
            .id(id.into())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .w(px(62.0))
            .h(px(52.0))
            .rounded_sm()
            .cursor_pointer()
            .text_color(rgb(text_color))
            .when(active, move |style| style.bg(rgb(theme.tree_selected_bg)))
            .when(!active && enabled, move |style| {
                style.hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(on_click)
            .child(
                svg()
                    .path(icon)
                    .w(px(22.0))
                    .h(px(22.0))
                    .flex_none()
                    .text_color(rgb(text_color)),
            )
            .child(div().text_size(px(11.0)).child(label))
    }

    fn select_main_tab(&mut self, tab: MainTab, cx: &mut Context<'_, Self>) {
        self.main_tab = tab;
        let category = match tab {
            MainTab::Tables => Category::Tables,
            MainTab::Views => Category::Views,
            _ => return,
        };
        if let Some(list) = self.object_list.as_mut() {
            list.category = category;
            list.selected = None;
        }
        cx.notify();
    }

    fn render_object_header(&self) -> impl IntoElement {
        let theme = self.theme;
        div()
            .px_2()
            .py_1()
            .text_size(px(12.0))
            .bg(rgb(theme.header_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(t!("object.header").to_string())
    }

    fn render_object_toolbar(
        &self,
        open_enabled: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .px_2()
            .py_1()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.toolbar_item(
                "obj-open",
                "icons/tables.svg",
                t!("object.open_table").to_string(),
                open_enabled,
                cx.listener(|this, _event, _window, cx| this.open_selected_object(cx)),
            ))
            .child(toolbar_separator(theme))
            .child(self.toolbar_item(
                "obj-design",
                "icons/design_table.svg",
                t!("object.design_table").to_string(),
                false,
                |_, _, _| {},
            ))
            .child(toolbar_separator(theme))
            .child(self.toolbar_item(
                "obj-new",
                "icons/new_table.svg",
                t!("object.new_table").to_string(),
                false,
                |_, _, _| {},
            ))
            .child(toolbar_separator(theme))
            .child(self.toolbar_item(
                "obj-delete",
                "icons/delete_table.svg",
                t!("object.delete_table").to_string(),
                false,
                |_, _, _| {},
            ))
            .child(toolbar_separator(theme))
            .child(self.toolbar_item(
                "obj-import",
                "icons/import.svg",
                t!("object.import_wizard").to_string(),
                false,
                |_, _, _| {},
            ))
            .child(toolbar_separator(theme))
            .child(self.toolbar_item(
                "obj-export",
                "icons/export.svg",
                t!("object.export_wizard").to_string(),
                false,
                |_, _, _| {},
            ))
    }

    fn render_object_body(
        &self,
        list: &ObjectList,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let connection_index = list.connection_index;
        let database_index = list.database_index;

        let body: AnyElement =
            match self
                .connections
                .get(connection_index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => databases.get(database_index),
                    _ => None,
                }) {
                None => div()
                    .p_3()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("common.empty").to_string())
                    .into_any_element(),
                Some(database) => match &database.tables {
                    Loadable::Idle | Loadable::Loading => div()
                        .p_3()
                        .text_color(rgb(theme.text_muted))
                        .child(t!("common.loading").to_string())
                        .into_any_element(),
                    Loadable::Failed(error) => div()
                        .p_3()
                        .text_color(rgb(theme.danger))
                        .child(error.clone())
                        .into_any_element(),
                    Loadable::Loaded(tables) => {
                        let want_view = match list.category {
                            Category::Tables => Some(false),
                            Category::Views => Some(true),
                            _ => None,
                        };
                        match want_view {
                            None => div()
                                .p_3()
                                .text_color(rgb(theme.text_muted))
                                .child(t!("common.empty").to_string())
                                .into_any_element(),
                            Some(want_view) => {
                                let mut flow = div().flex().flex_row().flex_wrap().gap_0p5().p_2();
                                for table in tables.iter().filter(|table| {
                                    matches!(table.kind, navidog_core::ObjectKind::View)
                                        == want_view
                                }) {
                                    flow = flow.child(self.render_object_item(list, table, cx));
                                }
                                flow.into_any_element()
                            }
                        }
                    }
                },
            };

        div()
            .id("object-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .overflow_scroll()
            .child(body)
    }

    fn render_object_item(
        &self,
        list: &ObjectList,
        table: &navidog_core::TableInfo,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let is_view = matches!(table.kind, navidog_core::ObjectKind::View);
        let selected = list.selected.as_deref() == Some(table.name.as_str());
        let name = table.name.clone();
        let open_name = name.clone();
        let connection_index = list.connection_index;
        let database_index = list.database_index;

        div()
            .id(SharedString::from(format!(
                "obj-{connection_index}-{database_index}-{}",
                table.name
            )))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w(px(220.0))
            .h(px(22.0))
            .px_1()
            .rounded_sm()
            .cursor_pointer()
            .when(selected, move |style| {
                style
                    .bg(rgb(theme.tree_selected_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, event, _window, cx| {
                let double_click =
                    matches!(event, ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2);
                this.select_object(open_name.clone(), cx);
                if double_click {
                    this.open_selected_object(cx);
                }
            }))
            .child(tree_icon(
                if is_view {
                    "icons/views.svg"
                } else {
                    "icons/tables.svg"
                },
                if is_view {
                    theme.icon_view
                } else {
                    theme.icon_table
                },
            ))
            .child(div().overflow_hidden().whitespace_nowrap().child(name))
    }

    fn select_object(&mut self, name: String, cx: &mut Context<'_, Self>) {
        if let Some(list) = self.object_list.as_mut() {
            list.selected = Some(name);
        }
        cx.notify();
    }

    fn open_selected_object(&mut self, cx: &mut Context<'_, Self>) {
        let Some(list) = self.object_list.as_ref() else {
            return;
        };
        let Some(name) = list.selected.clone() else {
            return;
        };
        let connection_index = list.connection_index;
        let database_index = list.database_index;
        let Some(database) = self.database_name(connection_index, database_index) else {
            return;
        };
        self.select_table(connection_index, database, name, cx);
    }

    fn close_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.opened = false;
            database.expanded = false;
            database.tables = Loadable::Idle;
            database.categories = CategoryExpansion::default();
        }
        if let Some(list) = self.object_list.as_ref()
            && list.connection_index == connection_index
            && list.database_index == database_index
        {
            self.object_list = None;
        }
        cx.notify();
    }

    fn open_new_database(
        &mut self,
        connection_index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.db_dialog = Some(DbDialog::New {
            connection_index,
            name: String::new(),
            error: None,
        });
        self.db_combo = None;
        self.form_offset = Point::default();
        window.focus(&self.db_focus);
        cx.notify();
    }

    fn open_edit_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(name) = self.database_name(connection_index, database_index) else {
            return;
        };
        self.db_dialog = Some(DbDialog::Edit {
            connection_index,
            database_index,
            name: name.clone(),
            original_charset: String::new(),
            original_collation: String::new(),
            charset: String::new(),
            collation: String::new(),
            charsets: Vec::new(),
            collations: Vec::new(),
            tab: DbTab::General,
            loading: true,
            error: None,
        });
        self.db_combo = None;
        self.form_offset = Point::default();
        window.focus(&self.db_focus);

        let Some(connection) = self.connection_arc(connection_index) else {
            cx.notify();
            return;
        };
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let defaults = match runtime
                .spawn({
                    let connection = connection.clone();
                    let name = name.clone();
                    async move { connection.database_defaults(&name).await }
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let charsets = match runtime
                .spawn({
                    let connection = connection.clone();
                    async move { connection.character_sets().await }
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let collations = match runtime
                .spawn(async move { connection.collations().await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(DbDialog::Edit {
                    original_charset,
                    original_collation,
                    charset,
                    collation,
                    charsets: charset_options,
                    collations: collation_options,
                    loading,
                    error,
                    ..
                }) = view.db_dialog.as_mut()
                {
                    *loading = false;
                    match defaults {
                        Ok((cs, col)) => {
                            *charset = cs.clone();
                            *collation = col.clone();
                            *original_charset = cs;
                            *original_collation = col;
                        }
                        Err(err) => *error = Some(err.to_string()),
                    }
                    match charsets {
                        Ok(values) => *charset_options = values,
                        Err(err) => *error = Some(err.to_string()),
                    }
                    match collations {
                        Ok(values) => *collation_options = values,
                        Err(err) => *error = Some(err.to_string()),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_delete_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(name) = self.database_name(connection_index, database_index) else {
            return;
        };
        self.db_dialog = Some(DbDialog::Delete {
            connection_index,
            database_index,
            name,
            error: None,
        });
        cx.notify();
    }

    fn db_submit(&mut self, cx: &mut Context<'_, Self>) {
        let Some(dialog) = self.db_dialog.take() else {
            return;
        };

        match dialog {
            DbDialog::New {
                connection_index,
                name,
                ..
            } => {
                let name = name.trim().to_string();
                if !is_valid_identifier(&name) {
                    self.db_dialog = Some(DbDialog::New {
                        connection_index,
                        name,
                        error: Some(t!("database.invalid_name").to_string()),
                    });
                    cx.notify();
                    return;
                }

                let Some(connection) = self.connection_arc(connection_index) else {
                    return;
                };
                let runtime = self.runtime.clone();
                let call_name = name.clone();
                cx.spawn(async move |this, cx| {
                    let result = match runtime
                        .spawn(async move { connection.create_database(&call_name).await })
                        .await
                    {
                        Ok(inner) => inner,
                        Err(error) => Err(Error::other(error)),
                    };

                    let _ = this.update(cx, |view, cx| {
                        match result {
                            Ok(()) => view.load_databases(connection_index, cx),
                            Err(error) => {
                                view.db_dialog = Some(DbDialog::New {
                                    connection_index,
                                    name,
                                    error: Some(error.to_string()),
                                });
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            DbDialog::Edit {
                connection_index,
                database_index,
                name,
                original_charset,
                original_collation,
                charset,
                collation,
                charsets,
                collations,
                tab,
                ..
            } => {
                let charset = charset.trim().to_string();
                let collation = collation.trim().to_string();
                let valid = (charset.is_empty() || is_valid_identifier(&charset))
                    && (collation.is_empty() || is_valid_identifier(&collation));
                if !valid {
                    self.db_dialog = Some(DbDialog::Edit {
                        connection_index,
                        database_index,
                        name,
                        original_charset,
                        original_collation,
                        charset,
                        collation,
                        charsets,
                        collations,
                        tab,
                        loading: false,
                        error: Some(t!("database.invalid_name").to_string()),
                    });
                    cx.notify();
                    return;
                }

                let charset_changed = charset != original_charset;
                let collation_changed = collation != original_collation;
                if !charset_changed && !collation_changed {
                    self.db_dialog = None;
                    cx.notify();
                    return;
                }

                let Some(connection) = self.connection_arc(connection_index) else {
                    return;
                };
                let runtime = self.runtime.clone();
                let for_call = if charset_changed && !charset.is_empty() {
                    Some(charset.clone())
                } else {
                    None
                };
                let collation_for_call = if collation_changed && !collation.is_empty() {
                    Some(collation.clone())
                } else {
                    None
                };
                let edit_name = name.clone();
                cx.spawn(async move |this, cx| {
                    let result = match runtime
                        .spawn(async move {
                            connection
                                .alter_database_defaults(
                                    &edit_name,
                                    for_call.as_deref(),
                                    collation_for_call.as_deref(),
                                )
                                .await
                        })
                        .await
                    {
                        Ok(inner) => inner,
                        Err(error) => Err(Error::other(error)),
                    };

                    let _ = this.update(cx, |view, cx| {
                        if let Err(error) = result {
                            view.db_dialog = Some(DbDialog::Edit {
                                connection_index,
                                database_index,
                                name,
                                original_charset,
                                original_collation,
                                charset,
                                collation,
                                charsets,
                                collations,
                                tab,
                                loading: false,
                                error: Some(error.to_string()),
                            });
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            DbDialog::Delete {
                connection_index,
                database_index,
                name,
                ..
            } => {
                let Some(connection) = self.connection_arc(connection_index) else {
                    return;
                };
                let runtime = self.runtime.clone();
                let drop_name = name.clone();
                cx.spawn(async move |this, cx| {
                    let result = match runtime
                        .spawn(async move { connection.drop_database(&drop_name).await })
                        .await
                    {
                        Ok(inner) => inner,
                        Err(error) => Err(Error::other(error)),
                    };

                    let _ = this.update(cx, |view, cx| {
                        match result {
                            Ok(()) => view.load_databases(connection_index, cx),
                            Err(error) => {
                                view.db_dialog = Some(DbDialog::Delete {
                                    connection_index,
                                    database_index,
                                    name,
                                    error: Some(error.to_string()),
                                });
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    fn db_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }

        match keystroke.key.as_str() {
            "enter" => {
                self.db_submit(cx);
                return;
            }
            "escape" => {
                self.db_dialog = None;
                self.db_combo = None;
                cx.notify();
                return;
            }
            _ => {}
        }

        let Some(dialog) = self.db_dialog.as_mut() else {
            return;
        };
        let target = match dialog {
            DbDialog::New { name, .. } => name,
            _ => return,
        };

        match keystroke.key.as_str() {
            "backspace" => {
                target.pop();
            }
            "space" => target.push(' '),
            _ => {
                if let Some(text) = keystroke.key_char.as_ref() {
                    target.push_str(text);
                }
            }
        }
        cx.notify();
    }

    fn db_field(
        &self,
        label: String,
        value: &str,
        handle: &FocusHandle,
        id: &'static str,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let focused = handle.is_focused(window);
        let shown = if focused && self.caret_visible {
            format!("{value}|")
        } else {
            value.to_string()
        };
        let focus_handle = handle.clone();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(150.0))
                    .flex_none()
                    .text_size(px(12.0))
                    .child(label),
            )
            .child(
                div()
                    .id(id)
                    .track_focus(handle)
                    .cursor_text()
                    .on_key_down(cx.listener(|this, event, _window, cx| this.db_key(event, cx)))
                    .on_click(cx.listener(move |_this, _event, window, _cx| {
                        window.focus(&focus_handle);
                    }))
                    .w(px(300.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .text_size(px(12.0))
                    .bg(rgb(theme.input_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .child(shown),
            )
    }

    fn db_combo(
        &self,
        label: String,
        value: &str,
        options: &[String],
        kind: DbCombo,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let open = self.db_combo == Some(kind);

        let mut list = div()
            .id(SharedString::from(format!("combo-list-{kind:?}")))
            .absolute()
            .top(px(24.0))
            .left_0()
            .w(px(300.0))
            .flex()
            .flex_col()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .h(px((options.len().min(10) as f32) * 22.0 + 4.0))
            .overflow_y_scroll();
        for option in options {
            let selected = option == value;
            let option_label = option.clone();
            let option_value = option.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("combo-{kind:?}-{option}")))
                    .flex()
                    .items_center()
                    .h(px(22.0))
                    .px_2()
                    .flex_none()
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(selected, move |style| {
                        style
                            .bg(rgb(theme.tree_selected_bg))
                            .text_color(rgb(theme.tree_selected_text))
                    })
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.db_select_combo(option_value.clone(), cx);
                    }))
                    .child(option_label),
            );
        }

        let combo_box = div()
            .id(SharedString::from(format!("combo-btn-{kind:?}")))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w(px(300.0))
            .h(px(24.0))
            .px_2()
            .text_size(px(12.0))
            .cursor_pointer()
            .bg(rgb(theme.input_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.db_combo = if this.db_combo == Some(kind) {
                    None
                } else {
                    Some(kind)
                };
                cx.notify();
            }))
            .child(value.to_string())
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(150.0))
                    .flex_none()
                    .text_size(px(12.0))
                    .child(label),
            )
            .child(div().relative().child(combo_box).when(open, move |style| {
                style.child(deferred(list).with_priority(10))
            }))
    }

    fn db_select_combo(&mut self, value: String, cx: &mut Context<'_, Self>) {
        let combo = self.db_combo.take();
        if let Some(DbDialog::Edit {
            charset,
            collation,
            collations,
            ..
        }) = self.db_dialog.as_mut()
        {
            match combo {
                Some(DbCombo::Charset) => {
                    *charset = value.clone();
                    let prefix = format!("{value}_");
                    if let Some(first) = collations
                        .iter()
                        .find(|candidate| candidate.starts_with(&prefix))
                        .cloned()
                    {
                        *collation = first;
                    } else {
                        collation.clear();
                    }
                }
                Some(DbCombo::Collation) => *collation = value,
                None => {}
            }
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    fn db_tab_button(&self, tab: DbTab, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let active = matches!(
            self.db_dialog,
            Some(DbDialog::Edit { tab: current, .. }) if current == tab
        );
        let label = match tab {
            DbTab::General => t!("database.tab.general").to_string(),
            DbTab::Sql => t!("database.tab.sql").to_string(),
        };
        let id = match tab {
            DbTab::General => "db-tab-general",
            DbTab::Sql => "db-tab-sql",
        };

        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .px_4()
            .h(px(24.0))
            .text_size(px(12.0))
            .cursor_pointer()
            .when(active, move |style| {
                style
                    .bg(rgb(theme.dialog_bg))
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text))
                    .font_weight(FontWeight::SEMIBOLD)
                    .mb(px(-1.0))
            })
            .when(!active, move |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.text_color(rgb(theme.text)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| this.db_select_tab(tab, cx)))
            .child(label)
    }

    fn db_select_tab(&mut self, tab: DbTab, cx: &mut Context<'_, Self>) {
        if let Some(DbDialog::Edit { tab: current, .. }) = self.db_dialog.as_mut() {
            *current = tab;
        }
        self.db_sql_anchor = 0;
        self.db_sql_cursor = 0;
        cx.notify();
    }

    fn db_alter_preview(&self) -> Option<String> {
        let Some(DbDialog::Edit {
            connection_index,
            name,
            original_charset,
            original_collation,
            charset,
            collation,
            loading,
            ..
        }) = self.db_dialog.as_ref()
        else {
            return None;
        };
        if *loading {
            return None;
        }

        let charset_changed = charset != original_charset;
        let collation_changed = collation != original_collation;
        if !charset_changed && !collation_changed {
            return None;
        }

        let connection = self.connection_arc(*connection_index)?;
        let charset = if charset_changed && !charset.is_empty() {
            Some(charset.as_str())
        } else {
            None
        };
        let collation = if collation_changed && !collation.is_empty() {
            Some(collation.as_str())
        } else {
            None
        };
        Some(connection.alter_database_sql(name, charset, collation))
    }

    fn db_sql_selection_range(&self) -> (usize, usize) {
        (
            self.db_sql_anchor.min(self.db_sql_cursor),
            self.db_sql_anchor.max(self.db_sql_cursor),
        )
    }

    fn db_sql_index_for_position(&self, position: Point<Pixels>) -> usize {
        let layout = self.db_sql_layout.borrow();
        match layout.index_for_position(position) {
            Ok(index) | Err(index) => index,
        }
    }

    fn db_sql_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let keystroke = &event.keystroke;
        if !(keystroke.modifiers.control || keystroke.modifiers.platform) {
            return;
        }

        match keystroke.key.as_str() {
            "a" => {
                let len = self.db_sql_text.borrow().len();
                self.db_sql_anchor = 0;
                self.db_sql_cursor = len;
                cx.notify();
            }
            "c" => {
                let (start, end) = self.db_sql_selection_range();
                if start < end {
                    let selected = self
                        .db_sql_text
                        .borrow()
                        .get(start..end)
                        .unwrap_or_default()
                        .to_string();
                    cx.write_to_clipboard(ClipboardItem::new_string(selected));
                }
            }
            _ => {}
        }
    }

    fn render_db_dialog(
        &self,
        dialog: &DbDialog,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let (title, content, allow_ok): (String, AnyElement, bool) = match dialog {
            DbDialog::New { name, error, .. } => {
                let body = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.db_field(
                        format!("{}:", t!("database.name")),
                        name,
                        &self.db_focus,
                        "db-name",
                        window,
                        cx,
                    ))
                    .child(db_error(error, theme));
                (
                    t!("database.new").to_string(),
                    div().p_4().child(body).into_any_element(),
                    true,
                )
            }
            DbDialog::Edit {
                name,
                charset,
                collation,
                charsets,
                collations,
                tab,
                loading,
                error,
                ..
            } => {
                let tabs = div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .gap_0p5()
                    .px_2()
                    .pt_2()
                    .bg(rgb(theme.dialog_face))
                    .child(self.db_tab_button(DbTab::General, cx))
                    .child(self.db_tab_button(DbTab::Sql, cx));

                let page: AnyElement = match tab {
                    DbTab::General => {
                        let mut form = div().flex().flex_col().gap_3().p_4().flex_1().child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(150.0))
                                        .flex_none()
                                        .text_size(px(12.0))
                                        .child(format!("{}:", t!("database.name"))),
                                )
                                .child(div().text_size(px(12.0)).child(name.clone())),
                        );
                        if *loading {
                            form = form.child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("common.loading").to_string()),
                            );
                        } else {
                            let collation_options: Vec<String> = if charset.is_empty() {
                                collations.clone()
                            } else {
                                let prefix = format!("{charset}_");
                                collations
                                    .iter()
                                    .filter(|candidate| candidate.starts_with(&prefix))
                                    .cloned()
                                    .collect()
                            };
                            form = form.child(self.db_combo(
                                format!("{}:", t!("database.charset")),
                                charset,
                                charsets,
                                DbCombo::Charset,
                                cx,
                            ));
                            form = form.child(self.db_combo(
                                format!("{}:", t!("database.collation")),
                                collation,
                                &collation_options,
                                DbCombo::Collation,
                                cx,
                            ));
                        }
                        form = form.child(db_error(error, theme));
                        form.into_any_element()
                    }
                    DbTab::Sql => {
                        let mut page = div().flex().flex_col().gap_2().p_4().flex_1();
                        match self.db_alter_preview() {
                            None => {
                                page = page.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(theme.text_muted))
                                        .child(t!("database.no_changes").to_string()),
                                );
                            }
                            Some(sql) => {
                                let (start, end) = self.db_sql_selection_range();
                                let mut text = StyledText::new(sql.clone());
                                if start < end {
                                    text = text.with_highlights(vec![(
                                        start..end,
                                        HighlightStyle {
                                            background_color: Some(
                                                rgb(theme.tree_selected_bg).into(),
                                            ),
                                            ..Default::default()
                                        },
                                    )]);
                                }
                                *self.db_sql_layout.borrow_mut() = text.layout().clone();
                                *self.db_sql_text.borrow_mut() = sql;

                                page = page.child(
                                    div()
                                        .id("db-sql")
                                        .track_focus(&self.db_sql_focus)
                                        .cursor_text()
                                        .w_full()
                                        .flex_1()
                                        .p_2()
                                        .text_size(px(12.0))
                                        .font_family("Consolas")
                                        .bg(rgb(theme.input_bg))
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .overflow_hidden()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                |this, event: &MouseDownEvent, window, cx| {
                                                    window.focus(&this.db_sql_focus);
                                                    let index = this
                                                        .db_sql_index_for_position(event.position);
                                                    this.db_sql_anchor = index;
                                                    this.db_sql_cursor = index;
                                                    this.db_sql_selecting = true;
                                                    cx.notify();
                                                },
                                            ),
                                        )
                                        .on_key_down(cx.listener(|this, event, _window, cx| {
                                            this.db_sql_key(event, cx)
                                        }))
                                        .child(text),
                                );
                            }
                        }
                        page.into_any_element()
                    }
                };

                (
                    t!("database.edit").to_string(),
                    div()
                        .flex()
                        .flex_col()
                        .child(tabs)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .h(px(300.0))
                                .mx_2()
                                .mb_2()
                                .border_1()
                                .border_color(rgb(theme.border))
                                .bg(rgb(theme.dialog_bg))
                                .child(page),
                        )
                        .into_any_element(),
                    !*loading,
                )
            }
            DbDialog::Delete { name, error, .. } => {
                let body = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(12.0)).child(format!(
                        "{}: {}",
                        t!("database.name"),
                        name
                    )))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.danger))
                            .child(t!("database.delete_confirm").to_string()),
                    )
                    .child(db_error(error, theme));
                (
                    t!("database.delete").to_string(),
                    div().p_4().child(body).into_any_element(),
                    true,
                )
            }
        };

        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(theme.overlay))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.db_sql_selecting {
                    this.db_sql_cursor = this.db_sql_index_for_position(event.position);
                    cx.notify();
                } else if this.form_dragging {
                    let dx = event.position.x - this.form_drag_origin.x;
                    let dy = event.position.y - this.form_drag_origin.y;
                    this.form_offset = Point {
                        x: this.form_drag_base.x + dx,
                        y: this.form_drag_base.y + dy,
                    };
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.form_dragging || this.db_sql_selecting {
                        this.form_dragging = false;
                        this.db_sql_selecting = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .relative()
                    .left(self.form_offset.x)
                    .top(self.form_offset.y)
                    .flex()
                    .flex_col()
                    .w(px(520.0))
                    .bg(rgb(theme.dialog_face))
                    .border_1()
                    .border_color(rgb(theme.neutral))
                    .shadow(dialog_shadow())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .h(px(32.0))
                            .pl_3()
                            .bg(rgb(theme.dialog_bg))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                    this.form_dragging = true;
                                    this.form_drag_origin = event.position;
                                    this.form_drag_base = this.form_offset;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        svg()
                                            .path("icons/database.svg")
                                            .w(px(14.0))
                                            .h(px(14.0))
                                            .flex_none()
                                            .text_color(rgb(theme.text)),
                                    )
                                    .child(div().text_size(px(12.5)).child(title)),
                            )
                            .child(self.dialog_close_button(
                                "db-close",
                                cx.listener(|this, _event, _window, cx| {
                                    this.db_dialog = None;
                                    this.db_combo = None;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(content)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .h(px(46.0))
                            .px_3()
                            .border_t_1()
                            .border_color(rgb(theme.border))
                            .child(self.dialog_button(
                                "db-cancel",
                                t!("form.cancel").to_string(),
                                false,
                                cx.listener(|this, _event, _window, cx| {
                                    this.db_dialog = None;
                                    cx.notify();
                                }),
                            ))
                            .child(self.dialog_button(
                                "db-ok",
                                t!("form.ok").to_string(),
                                true,
                                cx.listener(move |this, _event, _window, cx| {
                                    if allow_ok {
                                        this.db_submit(cx);
                                    }
                                }),
                            )),
                    ),
            )
    }

    fn render_grid(&self, grid: &GridState, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let mut header = div().flex().flex_row().bg(rgb(theme.header_bg));
        for column in &grid.columns {
            header = header.child(
                div()
                    .w(px(160.0))
                    .flex_none()
                    .px_2()
                    .py_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .border_r_1()
                    .border_color(rgb(theme.border))
                    .child(column.name.clone()),
            );
        }

        let mut rows = div().flex().flex_col();
        for (row_index, row) in grid.rows.iter().enumerate() {
            let background = if row_index % 2 == 1 {
                rgb(theme.row_alt_bg)
            } else {
                rgb(theme.editor_bg)
            };
            let mut row_element = div().flex().flex_row().bg(background);
            for cell in row {
                row_element = row_element.child(
                    div()
                        .w(px(160.0))
                        .flex_none()
                        .px_2()
                        .py_1()
                        .overflow_hidden()
                        .child(cell.as_display()),
                );
            }
            rows = rows.child(row_element);
        }

        let table = div()
            .id("grid-scroll")
            .flex_1()
            .w_full()
            .overflow_scroll()
            .track_scroll(&self.grid_scroll)
            .child(div().flex().flex_col().child(header).child(rows));

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.render_toolbar(grid, cx))
            .child(table)
    }

    fn render_toolbar(&self, grid: &GridState, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let title = format!("{}.{}", grid.database, grid.table);
        let page_label = format!("{} {}", t!("grid.page"), grid.page_index + 1);
        let total = match grid.total_rows {
            Some(total) => format!("{}: {}", t!("content.rows"), total),
            None => String::new(),
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .px_2()
            .py_1()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(div().text_color(rgb(theme.text_muted)).child(total)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.action_button(
                        "grid-prev",
                        t!("grid.prev").to_string(),
                        cx.listener(|this, _event, _window, cx| this.prev_page(cx)),
                    ))
                    .child(div().child(page_label))
                    .child(self.action_button(
                        "grid-next",
                        t!("grid.next").to_string(),
                        cx.listener(|this, _event, _window, cx| this.next_page(cx)),
                    ))
                    .child(self.action_button(
                        "grid-refresh",
                        t!("grid.refresh").to_string(),
                        cx.listener(|this, _event, _window, cx| this.refresh(cx)),
                    ))
                    .child(self.action_button(
                        "grid-page-size",
                        format!("{}: {}", t!("grid.page_size"), grid.page_size),
                        cx.listener(|this, _event, _window, cx| this.cycle_page_size(cx)),
                    )),
            )
    }

    fn render_password_prompt(
        &self,
        prompt: &PasswordPrompt,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let name = self
            .connections
            .get(prompt.index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let title = format!("{}: {}", t!("password.title"), name);
        let masked = "*".repeat(prompt.password.chars().count());
        let shown = if self.password_focus.is_focused(window) {
            format!("{masked}|")
        } else {
            masked
        };
        let focus_handle = self.password_focus.clone();
        let theme = self.theme;

        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(theme.overlay))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .w(px(360.0))
                    .p_4()
                    .bg(rgb(theme.dialog_bg))
                    .child(div().text_size(px(14.0)).child(title))
                    .child(
                        div()
                            .id("password-field")
                            .track_focus(&self.password_focus)
                            .cursor_text()
                            .on_key_down(cx.listener(|this, event, _window, cx| {
                                this.password_key(event, cx);
                            }))
                            .on_click(cx.listener(move |_this, _event, window, _cx| {
                                window.focus(&focus_handle);
                            }))
                            .h(px(28.0))
                            .flex()
                            .items_center()
                            .px_2()
                            .bg(rgb(theme.input_bg))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .child(shown),
                    )
                    .child(
                        div()
                            .id("password-save")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                if let Some(prompt) = this.password_prompt.as_mut() {
                                    prompt.save_password = !prompt.save_password;
                                }
                                cx.notify();
                            }))
                            .child(checkbox_box(prompt.save_password, theme))
                            .child(t!("form.save_password").to_string()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(self.dialog_button(
                                "password-cancel",
                                t!("form.cancel").to_string(),
                                false,
                                cx.listener(|this, _event, _window, cx| {
                                    this.password_prompt = None;
                                    cx.notify();
                                }),
                            ))
                            .child(self.dialog_button(
                                "password-ok",
                                t!("form.ok").to_string(),
                                true,
                                cx.listener(|this, _event, _window, cx| {
                                    this.submit_password(cx);
                                }),
                            )),
                    ),
            )
    }

    fn render_dialog(
        &self,
        form: &ConnectionForm,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let title = if self.editing.is_some() {
            t!("form.edit_title")
        } else {
            t!("form.title")
        };
        let name = if form.name.trim().is_empty() {
            t!("app.title").to_string()
        } else {
            form.name.trim().to_string()
        };
        let heading = format!("{name} - {title}");

        let mut test = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .min_w(px(0.0))
            .gap_2()
            .child(self.dialog_button(
                "form-test",
                t!("form.test_connection").to_string(),
                false,
                cx.listener(|this, _event, _window, cx| this.test_form(cx)),
            ));

        match &self.test_status {
            TestStatus::Idle => {}
            TestStatus::Testing => {
                test = test.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text_muted))
                        .child(t!("form.testing").to_string()),
                );
            }
            TestStatus::Success => {
                test = test.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.icon_connection))
                        .child(t!("form.test_success").to_string()),
                );
            }
            TestStatus::Failed(_) => {}
        }

        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(theme.overlay))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if this.form_selecting {
                    let text = this
                        .form
                        .as_ref()
                        .map(|form| form.value(this.form_active_field).to_string())
                        .unwrap_or_default();
                    let index = this.field_index_for_x(&text, event.position.x, window);
                    this.form_selection.cursor = index;
                    cx.notify();
                } else if this.db_sql_selecting {
                    this.db_sql_cursor = this.db_sql_index_for_position(event.position);
                    cx.notify();
                } else if this.form_dragging {
                    let dx = event.position.x - this.form_drag_origin.x;
                    let dy = event.position.y - this.form_drag_origin.y;
                    this.form_offset = Point {
                        x: this.form_drag_base.x + dx,
                        y: this.form_drag_base.y + dy,
                    };
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.form_selecting || this.form_dragging || this.db_sql_selecting {
                        this.form_selecting = false;
                        this.form_dragging = false;
                        this.db_sql_selecting = false;
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .relative()
                    .left(self.form_offset.x)
                    .top(self.form_offset.y)
                    .flex()
                    .flex_col()
                    .w(px(PANEL_WIDTH))
                    .bg(rgb(theme.dialog_face))
                    .border_1()
                    .border_color(rgb(theme.neutral))
                    .shadow(dialog_shadow())
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .h(px(32.0))
                            .pl_3()
                            .bg(rgb(theme.dialog_bg))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                    this.form_dragging = true;
                                    this.form_selecting = false;
                                    this.form_drag_origin = event.position;
                                    this.form_drag_base = this.form_offset;
                                    cx.notify();
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        svg()
                                            .path("icons/connection.svg")
                                            .w(px(14.0))
                                            .h(px(14.0))
                                            .flex_none()
                                            .text_color(rgb(theme.text)),
                                    )
                                    .child(div().text_size(px(12.5)).child(heading)),
                            )
                            .child(self.dialog_close_button(
                                "form-close",
                                cx.listener(|this, _event, _window, cx| {
                                    this.form = None;
                                    this.context_menu = None;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .w_full()
                            .gap_0p5()
                            .px_2()
                            .pt_2()
                            .bg(rgb(theme.dialog_face))
                            .child(form_tab(t!("form.tab.general").to_string(), true, theme))
                            .child(form_tab(t!("form.tab.advanced").to_string(), false, theme))
                            .child(form_tab(t!("form.tab.database").to_string(), false, theme))
                            .child(form_tab("SSL".to_string(), false, theme))
                            .child(form_tab("SSH".to_string(), false, theme))
                            .child(form_tab("HTTP".to_string(), false, theme)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .h(px(430.0))
                            .mx_2()
                            .mb_2()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.dialog_bg))
                            .child(self.render_general_tab(form, window, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .h(px(46.0))
                            .px_3()
                            .child(test)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .flex_none()
                                    .gap_2()
                                    .child(self.dialog_button(
                                        "form-save",
                                        t!("form.ok").to_string(),
                                        true,
                                        cx.listener(|this, _event, _window, cx| this.save_form(cx)),
                                    ))
                                    .child(self.dialog_button(
                                        "form-cancel",
                                        t!("form.cancel").to_string(),
                                        false,
                                        cx.listener(|this, _event, _window, cx| {
                                            this.form = None;
                                            this.context_menu = None;
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    ),
            )
    }

    fn render_general_tab(
        &self,
        form: &ConnectionForm,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let mut content = div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .pb_2()
                    .child(self.header_endpoint(
                        "logo.png",
                        t!("form.header_source").to_string(),
                        true,
                        theme,
                    ))
                    .child(
                        div()
                            .w(px(150.0))
                            .border_t_1()
                            .border_color(rgb(theme.border)),
                    )
                    .child(self.header_endpoint(
                        "icons/database.svg",
                        t!("form.header_target").to_string(),
                        false,
                        theme,
                    )),
            )
            .child(self.render_form_field(
                FormField::Name,
                format!("{}:", t!("form.name")),
                form.value(FormField::Name),
                window,
                cx,
            ))
            .child(self.render_form_field(
                FormField::Host,
                format!("{}:", t!("form.host")),
                form.value(FormField::Host),
                window,
                cx,
            ))
            .child(self.render_form_field(
                FormField::Port,
                format!("{}:", t!("form.port")),
                form.value(FormField::Port),
                window,
                cx,
            ))
            .child(self.render_form_field(
                FormField::Username,
                format!("{}:", t!("form.username")),
                form.value(FormField::Username),
                window,
                cx,
            ))
            .child(self.render_form_field(
                FormField::Password,
                format!("{}:", t!("form.password")),
                form.value(FormField::Password),
                window,
                cx,
            ))
            .child(
                div()
                    .id("form-save-password")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .pl(px(120.0))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        if let Some(form) = this.form.as_mut() {
                            form.save_password = !form.save_password;
                        }
                        cx.notify();
                    }))
                    .child(checkbox_box(form.save_password, theme))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child(t!("form.save_password").to_string()),
                    ),
            );

        if let TestStatus::Failed(error) = &self.test_status {
            content =
                content.child(self.render_selectable_text("form-error", error, theme.danger, cx));
        }

        content
    }

    fn render_selectable_text(
        &self,
        id: &'static str,
        text: &str,
        color: u32,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let (start, end) = self.db_sql_selection_range();
        let mut styled = StyledText::new(text.to_string());
        if start < end {
            styled = styled.with_highlights(vec![(
                start..end,
                HighlightStyle {
                    background_color: Some(rgb(theme.tree_selected_bg).into()),
                    ..Default::default()
                },
            )]);
        }
        *self.db_sql_layout.borrow_mut() = styled.layout().clone();
        *self.db_sql_text.borrow_mut() = text.to_string();

        div()
            .id(id)
            .track_focus(&self.db_sql_focus)
            .cursor_text()
            .w_full()
            .overflow_hidden()
            .text_size(px(12.0))
            .text_color(rgb(color))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.db_sql_focus);
                    let index = this.db_sql_index_for_position(event.position);
                    this.db_sql_anchor = index;
                    this.db_sql_cursor = index;
                    this.db_sql_selecting = true;
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, event, _window, cx| this.db_sql_key(event, cx)))
            .child(styled)
    }

    fn header_endpoint(
        &self,
        path: &'static str,
        label: String,
        is_image: bool,
        theme: Theme,
    ) -> AnyElement {
        let icon: AnyElement = if is_image {
            img(ImageSource::Resource(Resource::Embedded(path.into())))
                .w(px(34.0))
                .h(px(34.0))
                .into_any_element()
        } else {
            svg()
                .path(path)
                .w(px(28.0))
                .h(px(28.0))
                .text_color(rgb(theme.text))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(div().h(px(34.0)).flex().items_center().child(icon))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text))
                    .child(label),
            )
            .into_any_element()
    }

    fn dialog_button(
        &self,
        id: &'static str,
        label: String,
        primary: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let kind = if primary {
            ButtonKind::Default
        } else {
            ButtonKind::Normal
        };
        self.win_button(id, label, kind, on_click)
    }

    fn dialog_close_button(
        &self,
        id: &'static str,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(38.0))
            .h_full()
            .flex_none()
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(rgb(self.theme.text))
            .hover(|style| style.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff)))
            .on_click(on_click)
            .child("✕")
    }

    fn win_button(
        &self,
        id: impl Into<SharedString>,
        label: String,
        kind: ButtonKind,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let (background, border, text) = button_colors(kind, theme);
        let enabled = kind != ButtonKind::Disabled;

        div()
            .id(id.into())
            .flex()
            .items_center()
            .justify_center()
            .px_3()
            .h(px(24.0))
            .text_size(px(12.0))
            .cursor_pointer()
            .bg(rgb(background))
            .text_color(rgb(text))
            .border_1()
            .border_color(rgb(border))
            .when(enabled, move |style| {
                style.hover(move |style| {
                    style
                        .bg(rgb(theme.button_hover_bg))
                        .border_color(rgb(theme.button_default_border))
                })
            })
            .on_click(on_click)
            .child(label)
    }

    fn toolbar_item(
        &self,
        id: impl Into<SharedString>,
        icon: &'static str,
        label: String,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let color = if enabled {
            theme.text
        } else {
            theme.text_muted
        };

        div()
            .id(id.into())
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(24.0))
            .rounded_sm()
            .text_size(px(12.0))
            .text_color(rgb(color))
            .when(enabled, move |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(on_click)
            .child(
                svg()
                    .path(icon)
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .text_color(rgb(color)),
            )
            .child(label)
    }

    fn render_form_field(
        &self,
        field: FormField,
        label: String,
        value: &str,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let masked = field == FormField::Password;
        let width = match field {
            FormField::Name | FormField::Host => 360.0,
            FormField::Port => 80.0,
            FormField::Username | FormField::Password | FormField::Database => 300.0,
        };
        let handle = self.form_focus.get(field);
        let focused = handle.is_focused(window);
        let theme = self.theme;

        let chars: Vec<char> = value.chars().collect();
        let display: Vec<char> = if masked {
            vec!['*'; chars.len()]
        } else {
            chars.clone()
        };
        let len = display.len();

        let mut shown = div().flex().flex_row().items_center();
        if focused {
            let mut selection = self.form_selection;
            selection.anchor = selection.anchor.min(len);
            selection.cursor = selection.cursor.min(len);
            let (start, end) = selection.range();
            if start < end {
                if start > 0 {
                    shown = shown.child(display[..start].iter().copied().collect::<String>());
                }
                shown = shown.child(
                    div()
                        .bg(rgb(theme.tree_selected_bg))
                        .text_color(rgb(theme.tree_selected_text))
                        .child(display[start..end].iter().copied().collect::<String>()),
                );
                if end < len {
                    shown = shown.child(display[end..].iter().copied().collect::<String>());
                }
            } else {
                if selection.cursor > 0 {
                    shown = shown.child(
                        display[..selection.cursor]
                            .iter()
                            .copied()
                            .collect::<String>(),
                    );
                }
                if self.caret_visible {
                    shown = shown.child("|");
                }
                if selection.cursor < len {
                    shown = shown.child(
                        display[selection.cursor..]
                            .iter()
                            .copied()
                            .collect::<String>(),
                    );
                }
            }
        } else {
            shown = shown.child(display.iter().copied().collect::<String>());
        }

        let focus_handle = handle.clone();
        let index_text = value.to_string();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(FIELD_LABEL_WIDTH))
                    .flex_none()
                    .text_size(px(12.0))
                    .child(label),
            )
            .child(
                div()
                    .id(SharedString::from(format!("field-{}", field as usize)))
                    .track_focus(handle)
                    .cursor_text()
                    .on_key_down(cx.listener(move |this, event, window, cx| {
                        this.form_key(field, event, window, cx)
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            window.focus(&focus_handle);
                            this.form_active_field = field;
                            let index =
                                this.field_index_for_x(&index_text, event.position.x, window);
                            this.form_selection = FieldSelection {
                                anchor: index,
                                cursor: index,
                            };
                            this.form_selecting = true;
                            this.form_dragging = false;
                            cx.notify();
                        }),
                    )
                    .w(px(width))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .text_size(px(12.0))
                    .bg(rgb(theme.input_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .child(shown),
            )
    }

    fn render_context_menu(
        &self,
        menu: &ContextMenu,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;

        let mut items = div()
            .flex()
            .flex_col()
            .w(px(150.0))
            .py_1()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.border));

        match &menu.target {
            ContextTarget::Connection(index) => {
                let index = *index;
                let connected = self
                    .connections
                    .get(index)
                    .map(|node| matches!(&node.status, ConnectionStatus::Connected(_)))
                    .unwrap_or(false);
                let connect_label = if connected {
                    t!("connection.disconnect").to_string()
                } else {
                    t!("connection.connect").to_string()
                };

                items = items
                    .child(self.context_item(
                        "ctx-connect",
                        connect_label,
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.toggle_connection(index, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "ctx-edit",
                        t!("connection.edit").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.open_edit_form(index, window, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "ctx-delete",
                        t!("connection.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.delete_connection(index, cx);
                        }),
                    ));
            }
            ContextTarget::Database {
                connection_index,
                database_index,
            } => {
                let ci = *connection_index;
                let di = *database_index;
                let opened = self
                    .connections
                    .get(ci)
                    .and_then(|node| match &node.databases {
                        Loadable::Loaded(databases) => databases.get(di),
                        _ => None,
                    })
                    .map(|database| database.opened)
                    .unwrap_or(false);

                if opened {
                    items = items.child(self.context_item(
                        "db-close",
                        t!("database.close").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.close_database(ci, di, cx);
                        }),
                    ));
                } else {
                    items = items.child(self.context_item(
                        "db-open",
                        t!("database.open").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_database(ci, di, cx);
                        }),
                    ));
                }

                items = items
                    .child(self.context_item(
                        "db-edit",
                        t!("database.edit").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.open_edit_database(ci, di, window, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "db-new",
                        t!("database.new").to_string(),
                        cx.listener(move |this, _event, window, cx| {
                            this.context_menu = None;
                            this.open_new_database(ci, window, cx);
                        }),
                    ))
                    .child(self.context_item(
                        "db-delete",
                        t!("database.delete").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.open_delete_database(ci, di, cx);
                        }),
                    ))
                    .child(div().h(px(1.0)).my_1().bg(rgb(theme.border)))
                    .child(self.context_item(
                        "db-refresh",
                        t!("database.refresh").to_string(),
                        cx.listener(move |this, _event, _window, cx| {
                            this.context_menu = None;
                            this.load_databases(ci, cx);
                        }),
                    ));
            }
        }

        div()
            .absolute()
            .left(menu.position.x)
            .top(menu.position.y)
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
                this.context_menu = None;
                cx.notify();
            }))
            .child(items)
    }

    fn context_item(
        &self,
        id: &'static str,
        label: String,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id(id)
            .flex()
            .items_center()
            .w_full()
            .h(px(24.0))
            .px_3()
            .text_size(px(12.5))
            .cursor_pointer()
            .hover(move |style| {
                style
                    .bg(rgb(theme.tree_hover_bg))
                    .text_color(rgb(theme.tree_selected_text))
            })
            .on_click(on_click)
            .child(label)
    }
}

fn form_tab(label: String, active: bool, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .px_4()
        .h(px(24.0))
        .text_size(px(12.0))
        .when(active, move |style| {
            style
                .bg(rgb(theme.dialog_bg))
                .border_t_1()
                .border_l_1()
                .border_r_1()
                .border_color(rgb(theme.border))
                .text_color(rgb(theme.text))
                .font_weight(FontWeight::SEMIBOLD)
                .mb(px(-1.0))
        })
        .when(!active, move |style| {
            style
                .bg(rgb(theme.button_bg))
                .border_1()
                .border_color(rgb(theme.border))
                .text_color(rgb(theme.text_muted))
        })
        .child(label)
}

fn toolbar_separator(theme: Theme) -> impl IntoElement {
    div()
        .w(px(1.0))
        .h(px(16.0))
        .flex_none()
        .mx_1()
        .bg(rgb(theme.border))
}

fn main_separator(theme: Theme) -> impl IntoElement {
    div()
        .w(px(1.0))
        .h(px(36.0))
        .flex_none()
        .mx_1()
        .bg(rgb(theme.border))
}

fn db_error(error: &Option<String>, theme: Theme) -> AnyElement {
    match error {
        Some(message) => div()
            .text_size(px(12.0))
            .text_color(rgb(theme.danger))
            .child(message.clone())
            .into_any_element(),
        None => div().into_any_element(),
    }
}

fn is_valid_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn dialog_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: rgba(0x00000040).into(),
        offset: Point {
            x: px(0.0),
            y: px(2.0),
        },
        blur_radius: px(8.0),
        spread_radius: px(0.0),
    }]
}

fn button_colors(kind: ButtonKind, theme: Theme) -> (u32, u32, u32) {
    match kind {
        ButtonKind::Normal => (theme.button_bg, theme.button_border, theme.text),
        ButtonKind::Default => (theme.button_bg, theme.button_default_border, theme.text),
        ButtonKind::Selected => (
            theme.tree_selected_bg,
            theme.button_default_border,
            theme.tree_selected_text,
        ),
        ButtonKind::Disabled => (theme.button_bg, theme.button_border, theme.text_muted),
    }
}

fn checkbox_box(checked: bool, theme: Theme) -> impl IntoElement {
    div()
        .w(px(14.0))
        .h(px(14.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(if checked {
            theme.primary
        } else {
            theme.input_bg
        }))
        .text_color(rgb(0xffffff))
        .child(if checked { "✓" } else { "" }.to_string())
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.theme = Theme::resolve(self.theme_setting, window.appearance());
        let theme = self.theme;

        if self.password_focus_pending {
            window.focus(&self.password_focus);
            self.password_focus_pending = false;
        }

        if (self.form.is_some() || self.db_dialog.is_some()) && !self.caret_blink_running {
            self.caret_blink_running = true;
            let executor = cx.background_executor().clone();
            cx.spawn(async move |this, cx| {
                loop {
                    executor.timer(Duration::from_millis(530)).await;
                    let keep_going = this.update(cx, |view, cx| {
                        if view.form.is_some() || view.db_dialog.is_some() {
                            view.caret_visible = !view.caret_visible;
                            cx.notify();
                            true
                        } else {
                            view.caret_blink_running = false;
                            view.caret_visible = true;
                            false
                        }
                    });
                    if !matches!(keep_going, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
        }

        let body = div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .overflow_hidden()
            .child(self.render_sidebar(cx))
            .child(self.render_content(cx));

        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme.window_bg))
            .text_color(rgb(theme.text))
            .text_size(px(12.5))
            .child(render_titlebar(theme))
            .child(self.render_menu_bar())
            .child(self.render_main_toolbar(cx))
            .child(body);

        if let Some(menu) = self.context_menu.as_ref() {
            root = root.child(self.render_context_menu(menu, cx));
        }

        if let Some(form) = self.form.as_ref() {
            root = root.child(self.render_dialog(form, window, cx));
        }

        if let Some(dialog) = self.db_dialog.as_ref() {
            root = root.child(self.render_db_dialog(dialog, window, cx));
        }

        if let Some(prompt) = self.password_prompt.as_ref() {
            root = root.child(self.render_password_prompt(prompt, window, cx));
        }

        root
    }
}

fn tree_chevron(expanded: bool, color: u32) -> AnyElement {
    svg()
        .path(if expanded {
            "icons/chevron-down.svg"
        } else {
            "icons/chevron-right.svg"
        })
        .w(px(14.0))
        .h(px(14.0))
        .flex_none()
        .text_color(rgb(color))
        .into_any_element()
}

fn chevron_spacer() -> AnyElement {
    div().w(px(14.0)).flex_none().into_any_element()
}

fn tree_icon(path: &'static str, color: u32) -> Svg {
    svg()
        .path(path)
        .w(px(16.0))
        .h(px(16.0))
        .flex_none()
        .text_color(rgb(color))
}

fn tree_message(text: String, indent: f32, color: u32) -> impl IntoElement {
    div()
        .pl(px(indent))
        .pr_2()
        .py_0p5()
        .text_color(rgb(color))
        .child(text)
}

fn render_titlebar(theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(px(32.0))
        .flex_none()
        .bg(rgb(theme.titlebar_bg))
        .border_b_1()
        .border_color(rgb(theme.border))
        .child(
            div()
                .id("titlebar-drag")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .h_full()
                .px_3()
                .text_sm()
                .window_control_area(WindowControlArea::Drag)
                .child(
                    img(ImageSource::Resource(Resource::Embedded("logo.png".into())))
                        .w(px(18.0))
                        .h(px(18.0))
                        .flex_none(),
                )
                .child(t!("app.title").to_string()),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h_full()
                .child(titlebar_button(
                    "titlebar-min",
                    "—",
                    theme,
                    |window, _cx| {
                        window.minimize_window();
                    },
                ))
                .child(titlebar_button(
                    "titlebar-max",
                    "□",
                    theme,
                    |window, _cx| {
                        window.zoom_window();
                    },
                ))
                .child(titlebar_button(
                    "titlebar-close",
                    "✕",
                    theme,
                    |window, _cx| {
                        window.remove_window();
                    },
                )),
        )
}

fn titlebar_button(
    id: &'static str,
    label: &str,
    theme: Theme,
    action: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(40.0))
        .h_full()
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
        .on_click(move |_event, window, cx| action(window, cx))
        .child(label.to_string())
}
