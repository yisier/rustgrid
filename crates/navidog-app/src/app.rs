use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

use gpui::{
    AnyElement, App, BoxShadow, ClickEvent, ClipboardItem, Context, Div, FocusHandle, FontWeight,
    HighlightStyle, ImageSource, KeyDownEvent, ListHorizontalSizingBehavior, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Resource, ScrollHandle,
    SharedString, Stateful, StyledText, Subscription, Svg, TextLayout, UniformListScrollHandle,
    Window, WindowControlArea, deferred, div, img, prelude::*, px, rgb, rgba, svg, uniform_list,
};
use navidog_config::{AppSettings, ConfigStore, LanguageSetting, ThemeSetting};
use navidog_core::{Connection, ConnectionConfig, DriverRegistry, Error, PageRequest, RowUpdate};

use crate::form::{ConnectionForm, FORM_FIELDS, FormField};
use crate::runtime::Runtime;
use crate::session::{
    Category, CategoryExpansion, CellSelection, ConnectionNode, ConnectionStatus, DatabaseNode,
    GridState, Loadable, compute_column_widths,
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

struct CellEditor {
    row: usize,
    col: usize,
    cells: Vec<(usize, usize)>,
    value: String,
}

struct DatePicker {
    row: usize,
    col: usize,
    cells: Vec<(usize, usize)>,
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    has_time: bool,
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
const OBJECT_ROW_HEIGHT: f32 = 20.0;
const OBJECT_BOTTOM_MARGIN: f32 = 20.0;
const GRID_ROW_HEIGHT: f32 = 24.0;
const GRID_COLUMN_WIDTH: f32 = 120.0;

pub struct AppView {
    registry: Arc<DriverRegistry>,
    config: Arc<ConfigStore>,
    runtime: Arc<Runtime>,
    connections: Vec<ConnectionNode>,
    grids: Vec<GridState>,
    active_grid: Option<usize>,
    next_grid_id: u64,
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
    grid_hscroll: ScrollHandle,
    grid_list_scroll: UniformListScrollHandle,
    object_scroll: ScrollHandle,
    page_input: String,
    page_input_focus: FocusHandle,
    page_input_focused: bool,
    object_search: String,
    object_search_focus: FocusHandle,
    object_search_focused: bool,
    grid_focus: FocusHandle,
    selecting_cells: bool,
    cell_editor: Option<CellEditor>,
    cell_editor_focus: FocusHandle,
    cell_editor_focused: bool,
    date_picker: Option<DatePicker>,
    window_bounds_subscription: Option<Subscription>,
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
            grids: Vec::new(),
            active_grid: None,
            next_grid_id: 0,
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
            page_size: 1000,
            sidebar_scroll: ScrollHandle::new(),
            grid_hscroll: ScrollHandle::new(),
            grid_list_scroll: UniformListScrollHandle::new(),
            object_scroll: ScrollHandle::new(),
            page_input: "1".to_string(),
            page_input_focus: cx.focus_handle(),
            page_input_focused: false,
            object_search: String::new(),
            object_search_focus: cx.focus_handle(),
            object_search_focused: false,
            grid_focus: cx.focus_handle(),
            selecting_cells: false,
            cell_editor: None,
            cell_editor_focus: cx.focus_handle(),
            cell_editor_focused: false,
            date_picker: None,
            window_bounds_subscription: None,
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
        const SIZES: [u64; 4] = [100, 500, 1000, 5000];

        let Some(id) = self.active_grid_id() else {
            return;
        };
        let mut size = None;
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id) {
            let position = SIZES
                .iter()
                .position(|size| *size == grid.page_size)
                .unwrap_or(2);
            grid.page_size = SIZES[(position + 1) % SIZES.len()];
            grid.page_index = 0;
            size = Some(grid.page_size);
        }
        if let Some(size) = size {
            self.page_size = size;
        }

        self.sync_page_input();
        self.load_page(id, cx);
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
            self.close_connection_grids(&connection);
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
            self.object_search.clear();
            self.active_grid = None;
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
        self.object_search.clear();
        self.active_grid = None;
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
                // The object list's scroll extents are only known after the
                // frame's paint, so schedule one more frame to reveal the
                // horizontal scrollbar without waiting for a hover/resize.
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(32))
                        .await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            });
        })
        .detach();
    }

    fn select_table(
        &mut self,
        connection_index: usize,
        database: String,
        table: String,
        is_view: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };

        if let Some(index) = self.grids.iter().position(|grid| {
            grid.database == database
                && grid.table == table
                && Arc::ptr_eq(&grid.connection, &connection)
        }) {
            self.activate_grid(Some(index), cx);
            return;
        }

        let connection_name = self
            .connections
            .get(connection_index)
            .map(|node| node.profile.name.clone())
            .unwrap_or_default();
        let id = self.next_grid_id;
        self.next_grid_id += 1;

        self.grids.push(GridState {
            id,
            connection,
            connection_name,
            database,
            table,
            is_view,
            page_index: 0,
            page_size: self.page_size,
            loading: false,
            error: None,
            columns: Vec::new(),
            rows: Arc::new(Vec::new()),
            column_widths: Vec::new(),
            total_rows: None,
            selection: None,
            edits: BTreeMap::new(),
        });
        self.active_grid = Some(self.grids.len() - 1);
        self.sync_page_input();

        self.load_page(id, cx);
    }

    fn activate_grid(&mut self, index: Option<usize>, cx: &mut Context<'_, Self>) {
        self.active_grid = index;
        self.selecting_cells = false;
        self.cell_editor = None;
        self.date_picker = None;
        self.sync_page_input();
        cx.notify();
    }

    fn active_grid_id(&self) -> Option<u64> {
        self.active_grid
            .and_then(|index| self.grids.get(index))
            .map(|grid| grid.id)
    }

    fn close_grid(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if index >= self.grids.len() {
            return;
        }
        self.grids.remove(index);
        match self.active_grid {
            Some(active) if active == index => {
                self.active_grid = if self.grids.is_empty() {
                    None
                } else {
                    Some(index.min(self.grids.len() - 1))
                };
            }
            Some(active) if active > index => self.active_grid = Some(active - 1),
            _ => {}
        }
        self.sync_page_input();
        cx.notify();
    }

    fn close_connection_grids(&mut self, connection: &Arc<dyn Connection>) {
        let active_id = self.active_grid_id();
        self.grids
            .retain(|grid| !Arc::ptr_eq(&grid.connection, connection));
        self.active_grid =
            active_id.and_then(|id| self.grids.iter().position(|grid| grid.id == id));
        self.sync_page_input();
    }

    fn load_page(&mut self, id: u64, cx: &mut Context<'_, Self>) {
        let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id) else {
            return;
        };

        grid.loading = true;
        grid.error = None;
        grid.selection = None;
        grid.edits.clear();

        let connection = grid.connection.clone();
        let database = grid.database.clone();
        let table = grid.table.clone();
        let page_index = grid.page_index;
        let page_size = grid.page_size;

        self.selecting_cells = false;
        self.cell_editor = None;
        self.date_picker = None;
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
                if let Some(grid) = view.grids.iter_mut().find(|grid| grid.id == id) {
                    grid.loading = false;
                    match result {
                        Ok(page) => {
                            grid.column_widths = compute_column_widths(&page.columns, &page.rows);
                            grid.columns = page.columns;
                            grid.rows = Arc::new(page.rows);
                            grid.total_rows = page.total_rows;
                            grid.error = None;
                            view.grid_list_scroll = UniformListScrollHandle::new();
                            view.grid_hscroll.set_offset(Point::default());
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
        let Some(id) = self.active_grid_id() else {
            return;
        };
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id)
            && grid.has_next()
        {
            grid.page_index += 1;
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }

    fn prev_page(&mut self, cx: &mut Context<'_, Self>) {
        let Some(id) = self.active_grid_id() else {
            return;
        };
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id) {
            grid.page_index = grid.page_index.saturating_sub(1);
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }

    fn first_page(&mut self, cx: &mut Context<'_, Self>) {
        let Some(id) = self.active_grid_id() else {
            return;
        };
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id) {
            grid.page_index = 0;
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }

    fn last_page(&mut self, cx: &mut Context<'_, Self>) {
        let Some(id) = self.active_grid_id() else {
            return;
        };
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id)
            && let Some(last) = grid.last_page()
        {
            grid.page_index = last;
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }

    fn sync_page_input(&mut self) {
        let page = self
            .active_grid
            .and_then(|index| self.grids.get(index))
            .map(|grid| grid.page_index + 1)
            .unwrap_or(1);
        self.page_input = page.to_string();
    }

    fn commit_page_input(&mut self, cx: &mut Context<'_, Self>) {
        let requested = self.page_input.trim().parse::<u64>().unwrap_or(1).max(1);
        let Some(id) = self.active_grid_id() else {
            return;
        };
        if let Some(grid) = self.grids.iter_mut().find(|grid| grid.id == id) {
            let target = match grid.last_page() {
                Some(last) => (requested - 1).min(last),
                None => requested - 1,
            };
            grid.page_index = target;
        }
        self.sync_page_input();
        self.load_page(id, cx);
    }

    fn page_input_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }

        match keystroke.key.as_str() {
            "backspace" => {
                self.page_input.pop();
            }
            "enter" => {
                self.commit_page_input(cx);
                return;
            }
            "escape" => {
                self.sync_page_input();
            }
            _ => {
                if let Some(text) = keystroke.key_char.as_ref()
                    && text.chars().all(|character| character.is_ascii_digit())
                {
                    self.page_input.push_str(text);
                }
            }
        }
        cx.notify();
    }

    fn object_search_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }

        match keystroke.key.as_str() {
            "backspace" => {
                self.object_search.pop();
            }
            "escape" => {
                self.object_search.clear();
            }
            "enter" => {}
            _ => {
                if let Some(text) = keystroke.key_char.as_ref()
                    && !text.chars().any(char::is_control)
                {
                    self.object_search.push_str(text);
                }
            }
        }
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(id) = self.active_grid_id() {
            self.load_page(id, cx);
        }
    }

    fn grid_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.cell_editor.is_some() {
            self.commit_editor(cx);
        }
        self.date_picker = None;

        let Some((row, col)) = self.cell_at(event.position) else {
            return;
        };
        if event.click_count >= 2 {
            self.begin_edit((row, col), None, window, cx);
            return;
        }
        if let Some(index) = self.active_grid
            && let Some(grid) = self.grids.get_mut(index)
        {
            grid.selection = Some(CellSelection::new(row, col));
        }
        self.selecting_cells = true;
        window.focus(&self.grid_focus);
        cx.notify();
    }

    fn grid_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        if !self.selecting_cells || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some((row, col)) = self.cell_at(event.position) else {
            return;
        };
        if let Some(index) = self.active_grid
            && let Some(grid) = self.grids.get_mut(index)
            && let Some(selection) = grid.selection.as_mut()
        {
            selection.cursor = (row, col);
        }
        cx.notify();
    }

    fn cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        let index = self.active_grid?;
        let grid = self.grids.get(index)?;
        if grid.rows.is_empty() {
            return None;
        }
        let handle = self.grid_list_scroll.0.borrow().base_handle.clone();
        let bounds = handle.bounds();
        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
            return None;
        }
        let local_x = f32::from(position.x) - f32::from(bounds.left());
        let local_y =
            f32::from(position.y) - f32::from(bounds.top()) - f32::from(handle.offset().y);
        if local_x < 0.0 || local_y < 0.0 {
            return None;
        }
        let row = (local_y / GRID_ROW_HEIGHT).floor() as usize;
        if row >= grid.rows.len() {
            return None;
        }
        let mut accumulated = 0.0f32;
        for (col, width) in grid.column_widths.iter().enumerate() {
            if local_x < accumulated + width {
                return Some((row, col));
            }
            accumulated += width;
        }
        None
    }

    fn grid_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.cell_editor.is_some() || self.date_picker.is_some() {
            return;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }
        let Some(index) = self.active_grid else {
            return;
        };
        let Some((row, col)) = self
            .grids
            .get(index)
            .and_then(|grid| grid.selection)
            .map(|s| s.cursor)
        else {
            return;
        };

        match keystroke.key.as_str() {
            "up" | "down" | "left" | "right" => {
                let (row_count, col_count) = {
                    let grid = &self.grids[index];
                    (grid.rows.len(), grid.columns.len())
                };
                if row_count == 0 || col_count == 0 {
                    return;
                }
                let (mut new_row, mut new_col) = (row, col);
                match keystroke.key.as_str() {
                    "up" => new_row = new_row.saturating_sub(1),
                    "down" => new_row = (new_row + 1).min(row_count - 1),
                    "left" => new_col = new_col.saturating_sub(1),
                    "right" => new_col = (new_col + 1).min(col_count - 1),
                    _ => {}
                }
                if let Some(grid) = self.grids.get_mut(index) {
                    grid.selection = Some(CellSelection::new(new_row, new_col));
                }
                cx.notify();
            }
            "enter" => self.begin_edit((row, col), None, window, cx),
            _ => {
                if let Some(text) = keystroke.key_char.as_ref()
                    && let Some(character) = text.chars().next()
                    && !character.is_control()
                {
                    self.begin_edit((row, col), Some(character), window, cx);
                }
            }
        }
    }

    fn begin_edit(
        &mut self,
        cell: (usize, usize),
        initial: Option<char>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(index) = self.active_grid else {
            return;
        };
        let (row, col) = cell;
        let (value, is_temporal, cells) = {
            let Some(grid) = self.grids.get(index) else {
                return;
            };
            if row >= grid.rows.len() || col >= grid.columns.len() {
                return;
            }
            let value = grid
                .edits
                .get(&(row, col))
                .cloned()
                .flatten()
                .unwrap_or_else(|| grid.rows[row][col].as_edit_string());
            let is_temporal = is_temporal_type(&grid.columns[col].data_type);
            let cells = grid
                .selection
                .filter(|selection| selection.contains(row, col))
                .map(|selection| selection.cells())
                .unwrap_or_else(|| vec![(row, col)]);
            (value, is_temporal, cells)
        };

        if is_temporal {
            self.open_date_picker(index, row, col, cells, &value, cx);
            return;
        }

        let value = match initial {
            Some(character) => character.to_string(),
            None => value,
        };
        self.cell_editor = Some(CellEditor {
            row,
            col,
            cells,
            value,
        });
        window.focus(&self.cell_editor_focus);
        cx.notify();
    }

    fn open_date_picker(
        &mut self,
        index: usize,
        row: usize,
        col: usize,
        cells: Vec<(usize, usize)>,
        value: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(grid) = self.grids.get(index) else {
            return;
        };
        let data_type = grid.columns[col].data_type.to_ascii_lowercase();
        let has_time = data_type.contains("datetime") || data_type.contains("timestamp");
        let base = parse_datetime(value).unwrap_or_else(|| chrono::Local::now().naive_local());
        self.date_picker = Some(DatePicker {
            row,
            col,
            cells,
            year: base.year(),
            month: base.month(),
            day: base.day(),
            hour: base.hour(),
            minute: base.minute(),
            second: base.second(),
            has_time,
        });
        self.cell_editor = None;
        cx.notify();
    }

    fn editor_key(&mut self, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            return;
        }
        match keystroke.key.as_str() {
            "backspace" => {
                if let Some(editor) = self.cell_editor.as_mut() {
                    editor.value.pop();
                }
            }
            "enter" => {
                self.commit_editor(cx);
                return;
            }
            "escape" => {
                self.cancel_editor(cx);
                return;
            }
            _ => {
                if let Some(text) = keystroke.key_char.as_ref()
                    && !text.chars().any(char::is_control)
                    && let Some(editor) = self.cell_editor.as_mut()
                {
                    editor.value.push_str(text);
                }
            }
        }
        cx.notify();
    }

    fn commit_editor(&mut self, cx: &mut Context<'_, Self>) {
        let Some(editor) = self.cell_editor.take() else {
            return;
        };
        if let Some(index) = self.active_grid
            && let Some(grid) = self.grids.get_mut(index)
        {
            for (row, col) in editor.cells {
                grid.edits.insert((row, col), Some(editor.value.clone()));
            }
        }
        cx.notify();
    }

    fn cancel_editor(&mut self, cx: &mut Context<'_, Self>) {
        self.cell_editor = None;
        cx.notify();
    }

    fn date_picker_shift_month(&mut self, delta: i32, cx: &mut Context<'_, Self>) {
        if let Some(picker) = self.date_picker.as_mut() {
            let mut month = picker.month as i32 + delta;
            let mut year = picker.year;
            while month < 1 {
                month += 12;
                year -= 1;
            }
            while month > 12 {
                month -= 12;
                year += 1;
            }
            picker.month = month as u32;
            picker.year = year;
            picker.day = picker.day.min(days_in_month(year, picker.month));
        }
        cx.notify();
    }

    fn date_picker_select_day(&mut self, day: u32, cx: &mut Context<'_, Self>) {
        if let Some(picker) = self.date_picker.as_mut() {
            picker.day = day;
        }
        cx.notify();
    }

    fn date_picker_shift_time(&mut self, field: usize, delta: i32, cx: &mut Context<'_, Self>) {
        if let Some(picker) = self.date_picker.as_mut() {
            match field {
                0 => picker.hour = wrap_unit(picker.hour, delta, 24),
                1 => picker.minute = wrap_unit(picker.minute, delta, 60),
                _ => picker.second = wrap_unit(picker.second, delta, 60),
            }
        }
        cx.notify();
    }

    fn date_picker_today(&mut self, cx: &mut Context<'_, Self>) {
        let now = chrono::Local::now().naive_local();
        if let Some(picker) = self.date_picker.as_mut() {
            picker.year = now.year();
            picker.month = now.month();
            picker.day = now.day();
            picker.hour = now.hour();
            picker.minute = now.minute();
            picker.second = now.second();
        }
        cx.notify();
    }

    fn date_picker_ok(&mut self, cx: &mut Context<'_, Self>) {
        let Some(picker) = self.date_picker.take() else {
            return;
        };
        let value = if picker.has_time {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                picker.year, picker.month, picker.day, picker.hour, picker.minute, picker.second
            )
        } else {
            format!("{:04}-{:02}-{:02}", picker.year, picker.month, picker.day)
        };
        if let Some(index) = self.active_grid
            && let Some(grid) = self.grids.get_mut(index)
        {
            for (row, col) in picker.cells {
                grid.edits.insert((row, col), Some(value.clone()));
            }
        }
        cx.notify();
    }

    fn date_picker_cancel(&mut self, cx: &mut Context<'_, Self>) {
        self.date_picker = None;
        cx.notify();
    }

    fn commit_edits(&mut self, cx: &mut Context<'_, Self>) {
        let Some(index) = self.active_grid else {
            return;
        };
        let Some(grid) = self.grids.get(index) else {
            return;
        };
        if grid.edits.is_empty() {
            return;
        }

        let primary_keys: Vec<usize> = grid
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.primary_key)
            .map(|(index, _)| index)
            .collect();
        let key_columns: Vec<usize> = if primary_keys.is_empty() {
            (0..grid.columns.len()).collect()
        } else {
            primary_keys
        };

        let mut by_row: BTreeMap<usize, Vec<(usize, Option<String>)>> = BTreeMap::new();
        for (&(row, col), value) in &grid.edits {
            by_row.entry(row).or_default().push((col, value.clone()));
        }

        let mut updates = Vec::new();
        for (row, cells) in by_row {
            let Some(row_values) = grid.rows.get(row) else {
                continue;
            };
            let set = cells
                .into_iter()
                .map(|(col, value)| (grid.columns[col].name.clone(), value))
                .collect();
            let keys = key_columns
                .iter()
                .filter_map(|&col| {
                    row_values
                        .get(col)
                        .map(|value| (grid.columns[col].name.clone(), value.as_edit_string()))
                })
                .collect();
            updates.push(RowUpdate { set, keys });
        }

        let connection = grid.connection.clone();
        let database = grid.database.clone();
        let table = grid.table.clone();
        let id = grid.id;
        let runtime = self.runtime.clone();

        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.update_rows(&database, &table, &updates).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| match result {
                Ok(()) => view.load_page(id, cx),
                Err(error) => {
                    if let Some(grid) = view.grids.iter_mut().find(|grid| grid.id == id) {
                        grid.error = Some(error.to_string());
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn cancel_edits(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.active_grid
            && let Some(grid) = self.grids.get_mut(index)
        {
            grid.edits.clear();
            grid.selection = None;
        }
        self.cell_editor = None;
        self.date_picker = None;
        cx.notify();
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
                    is_view,
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

        let body: AnyElement =
            if let Some(grid) = self.active_grid.and_then(|index| self.grids.get(index)) {
                self.render_grid(grid, cx).into_any_element()
            } else if let Some(list) = self.object_list.as_ref() {
                let open_enabled = list.selected.is_some();
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_object_toolbar(open_enabled, cx))
                    .child(self.render_object_body(list, cx))
                    .into_any_element()
            } else {
                div().into_any_element()
            };

        let has_tabs = self.object_list.is_some() || !self.grids.is_empty();
        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .bg(rgb(theme.editor_bg));
        if has_tabs {
            content = content.child(self.render_tab_bar(cx));
        }
        content.child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .overflow_hidden()
                .child(body),
        )
    }

    fn render_tab_bar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;

        let mut bar = div()
            .flex()
            .flex_row()
            .items_end()
            .gap_1()
            .px_1()
            .pt_1()
            .h(px(30.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .overflow_hidden();

        bar = bar.child(self.render_object_tab(self.active_grid.is_none(), cx));
        for (index, grid) in self.grids.iter().enumerate() {
            bar =
                bar.child(self.render_table_tab(index, grid, self.active_grid == Some(index), cx));
        }
        bar
    }

    fn render_object_tab(&self, active: bool, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;

        div()
            .id("tab-object")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h_full()
            .px_2()
            .cursor_pointer()
            .text_size(px(12.0))
            .when(active, |style| {
                style
                    .bg(rgb(theme.editor_bg))
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
            })
            .when(!active, |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.activate_grid(None, cx);
            }))
            .child(tree_icon("icons/tables.svg", theme.icon_tables))
            .child(t!("object.header").to_string())
    }

    fn render_table_tab(
        &self,
        index: usize,
        grid: &GridState,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let kind = t!(if grid.is_view {
            "common.view"
        } else {
            "common.table"
        })
        .to_string();
        let title = format!(
            "{} @{} ({}) - {}",
            grid.table, grid.database, grid.connection_name, kind
        );
        let icon = if grid.is_view {
            "icons/views.svg"
        } else {
            "icons/tables.svg"
        };
        let icon_color = if grid.is_view {
            theme.icon_view
        } else {
            theme.icon_table
        };
        let tab_id = SharedString::from(format!("tab-{}", grid.id));
        let close_id = SharedString::from(format!("tab-close-{}", grid.id));

        div()
            .id(tab_id)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h_full()
            .px_2()
            .cursor_pointer()
            .text_size(px(12.0))
            .when(active, |style| {
                style
                    .bg(rgb(theme.editor_bg))
                    .border_t_1()
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(theme.border))
            })
            .when(!active, |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
            })
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                if index < this.grids.len() {
                    this.activate_grid(Some(index), cx);
                }
            }))
            .child(tree_icon(icon, icon_color))
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .max_w(px(240.0))
                    .child(title),
            )
            .child(
                div()
                    .id(close_id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(16.0))
                    .h(px(16.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.close_grid(index, cx);
                    }))
                    .child("✕"),
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
        self.object_search.clear();
        self.active_grid = None;
        cx.notify();
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
            .justify_between()
            .gap_2()
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
                    )),
            )
            .child(self.render_object_search(cx))
    }

    fn render_object_search(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let has_text = !self.object_search.is_empty();
        let mut field = div()
            .id("object-search")
            .track_focus(&self.object_search_focus)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .w(px(220.0))
            .h(px(24.0))
            .px_2()
            .bg(rgb(theme.input_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .cursor_text()
            .on_key_down(cx.listener(|this, event, _window, cx| this.object_search_key(event, cx)))
            .on_click(cx.listener(|this, _event, window, cx| {
                window.focus(&this.object_search_focus);
                cx.notify();
            }))
            .child(
                svg()
                    .path("icons/search.svg")
                    .w(px(13.0))
                    .h(px(13.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );
        let caret = self.object_search_focused && self.caret_visible;
        if has_text {
            field = field.child(div().flex_1().overflow_hidden().whitespace_nowrap().child(
                format!("{}{}", self.object_search, if caret { "|" } else { "" }),
            ));
        } else {
            field = field.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(if caret { "|" } else { "" })
                    .child(
                        div()
                            .text_color(rgb(theme.text_muted))
                            .child(t!("object.search").to_string()),
                    ),
            );
        }
        if has_text {
            field = field.child(
                div()
                    .id("object-search-clear")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_none()
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.button_hover_bg)))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.object_search.clear();
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path("icons/cross.svg")
                            .w(px(10.0))
                            .h(px(10.0))
                            .flex_none()
                            .text_color(rgb(theme.text_muted)),
                    ),
            );
        }
        field
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
                                let rows = self.object_rows_per_column();
                                let query = self.object_search.trim().to_lowercase();
                                let mut columns =
                                    div().flex().flex_row().items_start().gap_1().p_1();
                                let mut column = div().flex().flex_col();
                                let mut count = 0usize;
                                for table in tables.iter().filter(|table| {
                                    matches!(table.kind, navidog_core::ObjectKind::View)
                                        == want_view
                                        && (query.is_empty()
                                            || table.name.to_lowercase().contains(&query))
                                }) {
                                    if count == rows {
                                        columns = columns.child(column);
                                        column = div().flex().flex_col();
                                        count = 0;
                                    }
                                    column = column.child(self.render_object_item(list, table, cx));
                                    count += 1;
                                }
                                if count > 0 {
                                    columns = columns.child(column);
                                }
                                columns.into_any_element()
                            }
                        }
                    }
                },
            };

        let scroller = div()
            .id("object-scroll")
            .flex()
            .flex_col()
            .items_start()
            .flex_1()
            .min_w(px(0.0))
            .overflow_x_scroll()
            .track_scroll(&self.object_scroll)
            .child(body);

        let mut container = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .child(scroller);
        if self.object_scroll.max_offset().width > px(0.0) {
            container = container.child(self.render_object_hscrollbar(cx));
        }
        container
    }

    fn object_rows_per_column(&self) -> usize {
        let viewport = f32::from(self.object_scroll.bounds().size.height);
        if viewport <= 0.0 {
            return 30;
        }
        let rows = ((viewport - OBJECT_BOTTOM_MARGIN) / OBJECT_ROW_HEIGHT).floor() as usize;
        rows.max(1)
    }

    fn render_object_hscrollbar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let bounds = self.object_scroll.bounds();
        let viewport = f32::from(bounds.size.width);
        let max = f32::from(self.object_scroll.max_offset().width);
        let scroll = -f32::from(self.object_scroll.offset().x);
        let content = (viewport + max).max(1.0);
        let thumb_w = (viewport * viewport / content).clamp(24.0, viewport.max(24.0));
        let travel = (viewport - thumb_w).max(0.0);
        let thumb_x = if max > 0.0 {
            (scroll / max) * travel
        } else {
            0.0
        };

        div()
            .id("object-hscrollbar")
            .relative()
            .flex_none()
            .w_full()
            .h(px(14.0))
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.scroll_object_to(event.position.x, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    this.scroll_object_to(event.position.x, cx);
                }
            }))
            .child(
                div()
                    .absolute()
                    .left(px(thumb_x))
                    .top(px(1.0))
                    .w(px(thumb_w))
                    .h(px(12.0))
                    .bg(rgb(theme.button_border)),
            )
    }

    fn scroll_object_to(&self, mouse_x: Pixels, cx: &mut Context<'_, Self>) {
        let bounds = self.object_scroll.bounds();
        let viewport = f32::from(bounds.size.width);
        let max = f32::from(self.object_scroll.max_offset().width);
        if viewport <= 0.0 || max <= 0.0 {
            return;
        }
        let content = viewport + max;
        let thumb_w = (viewport * viewport / content).clamp(24.0, viewport);
        let travel = viewport - thumb_w;
        if travel <= 0.0 {
            return;
        }
        let relative = f32::from(mouse_x) - f32::from(bounds.left());
        let thumb_x = (relative - thumb_w / 2.0).clamp(0.0, travel);
        let scroll = thumb_x / travel * max;
        let y = self.object_scroll.offset().y;
        self.object_scroll.set_offset(Point::new(px(-scroll), y));
        cx.notify();
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
            .h(px(OBJECT_ROW_HEIGHT))
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
        let is_view = list.category == Category::Views;
        self.select_table(connection_index, database, name, is_view, cx);
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
        let widths: Vec<f32> = if grid.column_widths.is_empty() {
            grid.columns.iter().map(|_| GRID_COLUMN_WIDTH).collect()
        } else {
            grid.column_widths.clone()
        };
        let content_width: f32 = widths.iter().sum::<f32>().max(1.0);

        let mut header = div().flex().flex_row().flex_none().bg(rgb(theme.header_bg));
        for (index, column) in grid.columns.iter().enumerate() {
            let width = widths.get(index).copied().unwrap_or(GRID_COLUMN_WIDTH);
            header = header.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(GRID_ROW_HEIGHT))
                    .w(px(width))
                    .flex_none()
                    .px_2()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .font_weight(FontWeight::SEMIBOLD)
                    .border_r_1()
                    .border_color(rgb(theme.border))
                    .child(column.name.clone()),
            );
        }

        let selection = grid.selection;
        let edits = grid.edits.clone();
        let rows = grid.rows.clone();
        let widths = Arc::new(widths);
        let list = uniform_list(
            SharedString::from(format!("grid-rows-{}", grid.id)),
            rows.len(),
            move |range, _window, _cx| {
                range
                    .map(|row_index| {
                        let row = &rows[row_index];
                        let base_background = if row_index % 2 == 1 {
                            theme.row_alt_bg
                        } else {
                            theme.editor_bg
                        };
                        let mut row_element = div()
                            .flex()
                            .flex_row()
                            .bg(rgb(base_background))
                            .h(px(GRID_ROW_HEIGHT));
                        for (index, cell) in row.iter().enumerate() {
                            let width = widths.get(index).copied().unwrap_or(GRID_COLUMN_WIDTH);
                            let selected = selection
                                .is_some_and(|selection| selection.contains(row_index, index));
                            let edited = edits.get(&(row_index, index));
                            let cell_background = if selected {
                                theme.tree_selected_bg
                            } else if edited.is_some() {
                                theme.cell_edit_bg
                            } else {
                                base_background
                            };
                            let display = match edited {
                                Some(Some(value)) => value.clone(),
                                Some(None) => "NULL".to_string(),
                                None => cell.as_display(),
                            };
                            let mut cell_element = div()
                                .flex()
                                .items_center()
                                .h(px(GRID_ROW_HEIGHT))
                                .w(px(width))
                                .flex_none()
                                .px_2()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .bg(rgb(cell_background));
                            if selected {
                                cell_element =
                                    cell_element.text_color(rgb(theme.tree_selected_text));
                            }
                            row_element = row_element.child(cell_element.child(display));
                        }
                        row_element
                    })
                    .collect::<Vec<_>>()
            },
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::FitList)
        .track_scroll(self.grid_list_scroll.clone())
        .flex_1()
        .min_h(px(0.0))
        .track_focus(&self.grid_focus)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.grid_mouse_down(event, window, cx);
            }),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
            this.grid_mouse_move(event, cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                this.selecting_cells = false;
                cx.notify();
            }),
        )
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            this.grid_key(event, window, cx);
        }));

        let table = div()
            .id("grid-hscroll")
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .track_scroll(&self.grid_hscroll)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .h_full()
                    .w(px(content_width))
                    .child(header)
                    .child(list)
                    .child(self.render_cell_editor(cx))
                    .child(self.render_date_picker(cx)),
            );

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.render_grid_toolbar())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .child(table)
                            .child(self.render_grid_hscrollbar(cx)),
                    )
                    .child(self.render_grid_vscrollbar(cx)),
            )
            .child(self.render_grid_controls(grid, cx))
            .child(self.render_grid_status(grid))
    }

    fn render_cell_editor(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(editor) = self.cell_editor.as_ref() else {
            return div().into_any_element();
        };
        let Some(index) = self.active_grid else {
            return div().into_any_element();
        };
        let Some(grid) = self.grids.get(index) else {
            return div().into_any_element();
        };
        let width = grid
            .column_widths
            .get(editor.col)
            .copied()
            .unwrap_or(GRID_COLUMN_WIDTH)
            .max(80.0);
        let left: f32 = grid.column_widths.iter().take(editor.col).sum();
        let offset_y = f32::from(self.grid_list_scroll.0.borrow().base_handle.offset().y);
        let top = GRID_ROW_HEIGHT + editor.row as f32 * GRID_ROW_HEIGHT + offset_y;
        let theme = self.theme;
        let caret = if self.cell_editor_focused && self.caret_visible {
            "|"
        } else {
            ""
        };

        div()
            .id("cell-editor")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(GRID_ROW_HEIGHT))
            .track_focus(&self.cell_editor_focus)
            .flex()
            .items_center()
            .px_2()
            .bg(rgb(theme.input_bg))
            .border_1()
            .border_color(rgb(theme.primary))
            .text_size(px(12.0))
            .on_key_down(cx.listener(|this, event, _window, cx| this.editor_key(event, cx)))
            .child(format!("{}{}", editor.value, caret))
            .into_any_element()
    }

    fn render_date_picker(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let Some(picker) = self.date_picker.as_ref() else {
            return div().into_any_element();
        };
        let Some(index) = self.active_grid else {
            return div().into_any_element();
        };
        let Some(grid) = self.grids.get(index) else {
            return div().into_any_element();
        };
        let theme = self.theme;
        let width = 232.0f32;
        let content_width: f32 = grid.column_widths.iter().sum();
        let cell_left: f32 = grid.column_widths.iter().take(picker.col).sum();
        let left = cell_left.min((content_width - width).max(0.0)).max(0.0);
        let handle = self.grid_list_scroll.0.borrow().base_handle.clone();
        let offset_y = f32::from(handle.offset().y);
        let viewport_h = f32::from(handle.bounds().size.height);
        let cell_top = GRID_ROW_HEIGHT + picker.row as f32 * GRID_ROW_HEIGHT + offset_y;
        let popup_h = if picker.has_time { 296.0 } else { 264.0 };
        let top = if cell_top + GRID_ROW_HEIGHT + popup_h > GRID_ROW_HEIGHT + viewport_h {
            (cell_top - popup_h).max(GRID_ROW_HEIGHT)
        } else {
            cell_top + GRID_ROW_HEIGHT
        };

        let title = t!("grid.year_month", year = picker.year, month = picker.month).to_string();
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("date-prev")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(20.0))
                    .h(px(20.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.date_picker_shift_month(-1, cx);
                    }))
                    .child("‹"),
            )
            .child(div().text_size(px(12.0)).child(title))
            .child(
                div()
                    .id("date-next")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(20.0))
                    .h(px(20.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.date_picker_shift_month(1, cx);
                    }))
                    .child("›"),
            );

        let first_weekday = NaiveDate::from_ymd_opt(picker.year, picker.month, 1)
            .map(|date| date.weekday().num_days_from_monday() as i32)
            .unwrap_or(0);
        let days = days_in_month(picker.year, picker.month) as i32;
        let now = chrono::Local::now().naive_local();

        let mut weekdays = div().flex().flex_row();
        for label in weekday_labels() {
            weekdays = weekdays.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(28.0))
                    .h(px(18.0))
                    .text_size(px(10.0))
                    .text_color(rgb(theme.text_muted))
                    .child(label),
            );
        }

        let mut calendar = div().flex().flex_col().items_center().child(weekdays);
        for week in 0..6 {
            let mut row = div().flex().flex_row();
            for weekday in 0..7 {
                let day_number = week * 7 + weekday - first_weekday;
                if day_number < 0 || day_number >= days {
                    row = row.child(div().w(px(28.0)).h(px(22.0)));
                    continue;
                }
                let day = day_number as u32 + 1;
                let is_selected = day == picker.day;
                let is_today =
                    now.year() == picker.year && now.month() == picker.month && now.day() == day;
                row = row.child(
                    div()
                        .id(SharedString::from(format!("date-day-{day}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(28.0))
                        .h(px(22.0))
                        .text_size(px(11.0))
                        .cursor_pointer()
                        .when(is_selected, |style| {
                            style.bg(rgb(theme.primary)).text_color(rgb(0xffffff))
                        })
                        .when(!is_selected && is_today, |style| {
                            style.border_1().border_color(rgb(theme.primary))
                        })
                        .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.date_picker_select_day(day, cx);
                        }))
                        .child(day.to_string()),
                );
            }
            calendar = calendar.child(row);
        }

        let mut time_row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap_1();
        if picker.has_time {
            time_row = time_row
                .child(self.time_spinner("date-hour", picker.hour, 0, cx))
                .child(":")
                .child(self.time_spinner("date-minute", picker.minute, 1, cx))
                .child(":")
                .child(self.time_spinner("date-second", picker.second, 2, cx));
        }

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("date-today")
                    .text_size(px(11.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.date_picker_today(cx);
                    }))
                    .child(format!(
                        "{}: {}/{}/{}",
                        t!("grid.today"),
                        now.year(),
                        now.month(),
                        now.day()
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.dialog_button(
                        "date-cancel",
                        t!("form.cancel").to_string(),
                        false,
                        cx.listener(|this, _event, _window, cx| this.date_picker_cancel(cx)),
                    ))
                    .child(self.dialog_button(
                        "date-ok",
                        t!("form.ok").to_string(),
                        true,
                        cx.listener(|this, _event, _window, cx| this.date_picker_ok(cx)),
                    )),
            );

        div()
            .id("date-picker")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .bg(rgb(theme.dialog_bg))
            .border_1()
            .border_color(rgb(theme.text_muted))
            .child(header)
            .child(calendar)
            .child(time_row)
            .child(footer)
            .into_any_element()
    }

    fn time_spinner(
        &self,
        id_prefix: &str,
        value: u32,
        field: usize,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .id(SharedString::from(format!("{id_prefix}-up")))
                    .cursor_pointer()
                    .text_size(px(8.0))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.date_picker_shift_time(field, 1, cx);
                    }))
                    .child("▲"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(28.0))
                    .h(px(18.0))
                    .bg(rgb(theme.input_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_size(px(11.0))
                    .child(format!("{value:02}")),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{id_prefix}-down")))
                    .cursor_pointer()
                    .text_size(px(8.0))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.date_picker_shift_time(field, -1, cx);
                    }))
                    .child("▼"),
            )
    }

    fn render_grid_vscrollbar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let handle = self.grid_list_scroll.0.borrow().base_handle.clone();
        let viewport = f32::from(handle.bounds().size.height);
        let max = f32::from(handle.max_offset().height);
        let scroll = -f32::from(handle.offset().y);
        let content = (viewport + max).max(1.0);
        let thumb_h = if viewport > 0.0 {
            (viewport * viewport / content).clamp(24.0, viewport)
        } else {
            24.0
        };
        let travel = (viewport - thumb_h).max(0.0);
        let thumb_y = if max > 0.0 {
            (scroll / max) * travel
        } else {
            0.0
        };

        div()
            .id("grid-vscrollbar")
            .relative()
            .flex_none()
            .w(px(14.0))
            .h_full()
            .bg(rgb(theme.toolbar_bg))
            .border_l_1()
            .border_color(rgb(theme.border))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.scroll_grid_v_to(event.position.y, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    this.scroll_grid_v_to(event.position.y, cx);
                }
            }))
            .child(
                div()
                    .absolute()
                    .left(px(1.0))
                    .top(px(thumb_y))
                    .w(px(12.0))
                    .h(px(thumb_h))
                    .bg(rgb(theme.button_border)),
            )
    }

    fn render_grid_hscrollbar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        let viewport = f32::from(self.grid_hscroll.bounds().size.width);
        let max = f32::from(self.grid_hscroll.max_offset().width);
        let scroll = -f32::from(self.grid_hscroll.offset().x);
        let content = (viewport + max).max(1.0);
        let thumb_w = if viewport > 0.0 {
            (viewport * viewport / content).clamp(24.0, viewport)
        } else {
            24.0
        };
        let travel = (viewport - thumb_w).max(0.0);
        let thumb_x = if max > 0.0 {
            (scroll / max) * travel
        } else {
            0.0
        };

        div()
            .id("grid-hscrollbar")
            .relative()
            .flex_none()
            .w_full()
            .h(px(14.0))
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.scroll_grid_h_to(event.position.x, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    this.scroll_grid_h_to(event.position.x, cx);
                }
            }))
            .child(
                div()
                    .absolute()
                    .left(px(thumb_x))
                    .top(px(1.0))
                    .w(px(thumb_w))
                    .h(px(12.0))
                    .bg(rgb(theme.button_border)),
            )
    }

    fn scroll_grid_v_to(&self, mouse_y: Pixels, cx: &mut Context<'_, Self>) {
        let handle = self.grid_list_scroll.0.borrow().base_handle.clone();
        let bounds = handle.bounds();
        let viewport = f32::from(bounds.size.height);
        let max = f32::from(handle.max_offset().height);
        if viewport <= 0.0 || max <= 0.0 {
            return;
        }
        let content = viewport + max;
        let thumb_h = (viewport * viewport / content).clamp(24.0, viewport);
        let travel = viewport - thumb_h;
        if travel <= 0.0 {
            return;
        }
        let relative = f32::from(mouse_y) - f32::from(bounds.top());
        let thumb_y = (relative - thumb_h / 2.0).clamp(0.0, travel);
        let scroll = thumb_y / travel * max;
        let x = handle.offset().x;
        handle.set_offset(Point::new(x, px(-scroll)));
        cx.notify();
    }

    fn scroll_grid_h_to(&self, mouse_x: Pixels, cx: &mut Context<'_, Self>) {
        let viewport = f32::from(self.grid_hscroll.bounds().size.width);
        let max = f32::from(self.grid_hscroll.max_offset().width);
        if viewport <= 0.0 || max <= 0.0 {
            return;
        }
        let content = viewport + max;
        let thumb_w = (viewport * viewport / content).clamp(24.0, viewport);
        let travel = viewport - thumb_w;
        if travel <= 0.0 {
            return;
        }
        let relative = f32::from(mouse_x) - f32::from(self.grid_hscroll.bounds().left());
        let thumb_x = (relative - thumb_w / 2.0).clamp(0.0, travel);
        let scroll = thumb_x / travel * max;
        let y = self.grid_hscroll.offset().y;
        self.grid_hscroll.set_offset(Point::new(px(-scroll), y));
        cx.notify();
    }

    fn render_grid_toolbar(&self) -> impl IntoElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .h(px(28.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.grid_tool_button(
                "grid-begin",
                "icons/transaction.svg",
                t!("grid.begin_transaction").to_string(),
                theme.text,
                true,
                true,
                |_, _, _| {},
            ))
            .child(self.grid_tool_button(
                "grid-text",
                "icons/text.svg",
                t!("grid.text").to_string(),
                theme.text,
                true,
                true,
                |_, _, _| {},
            ))
            .child(self.grid_tool_button(
                "grid-filter",
                "icons/filter.svg",
                t!("grid.filter").to_string(),
                theme.text,
                true,
                false,
                |_, _, _| {},
            ))
            .child(self.grid_tool_button(
                "grid-sort",
                "icons/sort.svg",
                t!("grid.sort").to_string(),
                theme.text,
                true,
                false,
                |_, _, _| {},
            ))
            .child(self.grid_tool_button(
                "grid-import",
                "icons/import.svg",
                t!("grid.import").to_string(),
                theme.icon_views,
                true,
                false,
                |_, _, _| {},
            ))
            .child(self.grid_tool_button(
                "grid-export",
                "icons/export.svg",
                t!("grid.export").to_string(),
                theme.icon_views,
                true,
                false,
                |_, _, _| {},
            ))
    }

    #[allow(clippy::too_many_arguments)]
    fn grid_tool_button(
        &self,
        id: &'static str,
        icon: &'static str,
        label: String,
        color: u32,
        enabled: bool,
        has_arrow: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let mut item = div()
            .id(id)
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
            .child(label);
        if has_arrow {
            item = item.child(
                svg()
                    .path("icons/chevron-down.svg")
                    .w(px(10.0))
                    .h(px(10.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            );
        }
        item
    }

    fn render_grid_controls(
        &self,
        grid: &GridState,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = self.theme;
        let last = grid.last_page();
        let at_first = grid.page_index == 0;
        let at_last = match last {
            Some(last) => grid.page_index >= last,
            None => !grid.has_next(),
        };
        let active = theme.text;
        let muted = theme.text_muted;

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .h(px(30.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(self.grid_icon_button(
                        "grid-add",
                        "icons/plus.svg",
                        theme.icon_connection,
                        false,
                        |_, _, _| {},
                    ))
                    .child(self.grid_icon_button(
                        "grid-delete",
                        "icons/minus.svg",
                        theme.danger,
                        false,
                        |_, _, _| {},
                    ))
                    .child(self.grid_icon_button(
                        "grid-commit",
                        "icons/check.svg",
                        theme.icon_connection,
                        !grid.edits.is_empty(),
                        cx.listener(|this, _event, _window, cx| this.commit_edits(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-rollback",
                        "icons/cross.svg",
                        theme.danger,
                        !grid.edits.is_empty(),
                        cx.listener(|this, _event, _window, cx| this.cancel_edits(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-refresh",
                        "icons/refresh.svg",
                        active,
                        true,
                        cx.listener(|this, _event, _window, cx| this.refresh(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-stop",
                        "icons/stop.svg",
                        muted,
                        false,
                        |_, _, _| {},
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(self.grid_icon_button(
                        "grid-first",
                        "icons/first.svg",
                        active,
                        !at_first,
                        cx.listener(|this, _event, _window, cx| this.first_page(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-prev",
                        "icons/prev.svg",
                        active,
                        !at_first,
                        cx.listener(|this, _event, _window, cx| this.prev_page(cx)),
                    ))
                    .child(self.render_page_input(cx))
                    .child(self.grid_icon_button(
                        "grid-next",
                        "icons/next.svg",
                        active,
                        !at_last,
                        cx.listener(|this, _event, _window, cx| this.next_page(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-last",
                        "icons/last.svg",
                        active,
                        !at_last,
                        cx.listener(|this, _event, _window, cx| this.last_page(cx)),
                    ))
                    .child(self.grid_icon_button(
                        "grid-gear",
                        "icons/gear.svg",
                        muted,
                        true,
                        cx.listener(|this, _event, _window, cx| this.cycle_page_size(cx)),
                    )),
            )
    }

    fn grid_icon_button(
        &self,
        id: &'static str,
        icon: &'static str,
        color: u32,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let tint = if enabled { color } else { theme.text_muted };
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(24.0))
            .h(px(22.0))
            .rounded_sm()
            .when(enabled, move |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .on_click(on_click)
            .child(
                svg()
                    .path(icon)
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex_none()
                    .text_color(rgb(tint)),
            )
    }

    fn render_page_input(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id("grid-page-input")
            .track_focus(&self.page_input_focus)
            .flex()
            .items_center()
            .justify_center()
            .w(px(52.0))
            .h(px(22.0))
            .bg(rgb(theme.input_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .text_size(px(12.0))
            .cursor_text()
            .on_key_down(cx.listener(|this, event, _window, cx| this.page_input_key(event, cx)))
            .on_click(cx.listener(|this, _event, window, cx| {
                window.focus(&this.page_input_focus);
                cx.notify();
            }))
            .child(format!(
                "{}{}",
                self.page_input,
                if self.page_input_focused && self.caret_visible {
                    "|"
                } else {
                    ""
                }
            ))
    }

    fn render_grid_status(&self, grid: &GridState) -> impl IntoElement {
        let theme = self.theme;
        let total = grid.total_rows.unwrap_or(0);
        let end = grid
            .page_index
            .saturating_mul(grid.page_size)
            .saturating_add(grid.rows.len() as u64);
        let info = t!(
            "grid.page_info",
            end = end,
            total = total,
            page = grid.page_index + 1
        )
        .to_string();
        let message = match grid.selection {
            Some(selection) => {
                let (start_row, end_row) = selection.rows();
                let (start_col, end_col) = selection.cols();
                t!(
                    "grid.selection_info",
                    rows = end_row - start_row + 1,
                    cols = end_col - start_col + 1
                )
                .to_string()
            }
            None => grid.sql(),
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_4()
            .px_2()
            .h(px(24.0))
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_t_1()
            .border_color(rgb(theme.border))
            .text_size(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(message),
            )
            .child(div().flex_none().child(info))
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

        // gpui does not re-run `render` when the window is resized, so the object list would
        // keep its initial rows-per-column. Observe bounds and force a follow-up frame so the
        // count is recomputed from the freshly measured scroll viewport.
        if self.window_bounds_subscription.is_none() {
            self.window_bounds_subscription =
                Some(cx.observe_window_bounds(window, |_this, window, cx| {
                    cx.notify();
                    cx.on_next_frame(window, |_this, _window, cx| cx.notify());
                }));
        }

        if self.password_focus_pending {
            window.focus(&self.password_focus);
            self.password_focus_pending = false;
        }

        self.object_search_focused = self.object_search_focus.is_focused(window);
        self.page_input_focused = self.page_input_focus.is_focused(window);
        self.cell_editor_focused = self.cell_editor_focus.is_focused(window);

        let text_field_active =
            self.object_search_focused || self.page_input_focused || self.cell_editor_focused;
        if (self.form.is_some() || self.db_dialog.is_some() || text_field_active)
            && !self.caret_blink_running
        {
            self.caret_blink_running = true;
            let executor = cx.background_executor().clone();
            cx.spawn(async move |this, cx| {
                loop {
                    executor.timer(Duration::from_millis(530)).await;
                    let keep_going = this.update(cx, |view, cx| {
                        if view.form.is_some()
                            || view.db_dialog.is_some()
                            || view.object_search_focused
                            || view.page_input_focused
                            || view.cell_editor_focused
                        {
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

fn is_temporal_type(data_type: &str) -> bool {
    let data_type = data_type.to_ascii_lowercase();
    data_type == "date" || data_type == "datetime" || data_type == "timestamp"
}

fn parse_datetime(value: &str) -> Option<NaiveDateTime> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    for format in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Some(parsed);
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return date.and_hms_opt(0, 0, 0);
    }
    None
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn wrap_unit(value: u32, delta: i32, modulus: u32) -> u32 {
    (value as i32 + delta).rem_euclid(modulus as i32) as u32
}

fn weekday_labels() -> [&'static str; 7] {
    if rust_i18n::locale().starts_with("zh") {
        ["一", "二", "三", "四", "五", "六", "日"]
    } else {
        ["M", "T", "W", "T", "F", "S", "S"]
    }
}
