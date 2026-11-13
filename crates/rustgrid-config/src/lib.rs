mod secrets;
pub mod transfer;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use rustgrid_core::{ConnectionProfile, SavedBackup, SavedQuery, SavedTransfer};
use serde::{Deserialize, Serialize};

const CURRENT_VERSION: u32 = 2;
const PROFILES_FILE: &str = "connections.json";
const SETTINGS_VERSION: u32 = 18;
const SETTINGS_FILE: &str = "settings.json";
/// The settings version that introduced 保留对象搜索 as a deliberate choice. Files written before
/// it kept the old `false` default, so loading turns the switch on once; a file that version or
/// newer carries whatever the user chose.
const PRESERVE_OBJECT_SEARCH_SINCE: u32 = 14;
const QUERIES_VERSION: u32 = 1;
const QUERIES_FILE: &str = "queries.json";
const QUERIES_DIR: &str = "queries";
const BACKUPS_VERSION: u32 = 1;
const BACKUPS_FILE: &str = "backups.json";
const TRANSFERS_VERSION: u32 = 1;
const TRANSFERS_FILE: &str = "transfers.json";
const BACKUPS_DIR: &str = "backups";
const DATABASES_VERSION: u32 = 1;
const DATABASES_FILE: &str = "databases.json";
const SESSION_FILE: &str = "session.json";
const SESSION_VERSION: u32 = 1;

/// What the main window had open when it last saved its session, restored on the next start
/// ("return to where I was"). Tables are restored lazily, once their connection is connected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSession {
    /// Profile ids of the connections that were open.
    #[serde(default)]
    pub connections: Vec<String>,
    /// The query tabs, in tab order.
    #[serde(default)]
    pub queries: Vec<SessionQuery>,
    /// The table / view data tabs, in tab order.
    #[serde(default)]
    pub tables: Vec<SessionTable>,
    /// The tab that was active.
    #[serde(default)]
    pub active: Option<SessionActive>,
    /// The databases that were open in the connection tree, reopened when their connection
    /// connects.
    #[serde(default)]
    pub databases: Vec<SessionDatabase>,
    /// The database whose objects the object list showed (Navicat's current database).
    #[serde(default)]
    pub current_database: Option<SessionDatabase>,
}

/// A database of a [`WorkspaceSession`], by its connection's profile id and its name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionDatabase {
    pub connection_id: String,
    pub database: String,
}

/// One query tab of a [`WorkspaceSession`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionQuery {
    #[serde(default)]
    pub connection_id: Option<String>,
    /// The connection-tree group this tab targets, for a group query (one result page per
    /// connection). `None` for a normal single-connection query.
    #[serde(default)]
    pub group_id: Option<u64>,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    /// The saved query's name and file, when the tab is bound to one.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub saved_path: Option<PathBuf>,
    /// The editor text. `None` for a saved query without unsaved edits, which reloads from its
    /// file; otherwise the text itself ("hot exit": unsaved edits survive a restart).
    #[serde(default)]
    pub sql: Option<String>,
    #[serde(default)]
    pub caret: usize,
}

/// One table / view data tab of a [`WorkspaceSession`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTable {
    pub connection_id: String,
    pub database: String,
    pub table: String,
    #[serde(default)]
    pub is_view: bool,
    #[serde(default)]
    pub page_index: u64,
}

/// The active tab of a [`WorkspaceSession`]: an index into its `queries` or `tables`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "index", rename_all = "snake_case")]
pub enum SessionActive {
    Query(usize),
    Table(usize),
}

#[derive(Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    #[serde(flatten)]
    session: WorkspaceSession,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not determine a configuration directory for this platform")]
    NoConfigDir,

    #[error("could not read or write configuration: {0}")]
    Io(#[from] std::io::Error),

    #[error("could not parse configuration: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("could not encrypt or decrypt stored credentials")]
    Crypto,
}

pub type ConfigResult<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, Clone)]
pub struct ConfigStore {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfilesFile {
    version: u32,
    #[serde(default)]
    profiles: Vec<ConnectionProfile>,
}

impl Default for ProfilesFile {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            profiles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeSetting {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LanguageSetting {
    #[default]
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "ru")]
    Ru,
}

impl LanguageSetting {
    pub fn locale(self) -> &'static str {
        match self {
            LanguageSetting::En => "en",
            LanguageSetting::ZhCn => "zh-CN",
            LanguageSetting::Ru => "ru",
        }
    }
}

/// A table grid's remembered field layout (the 字段筛选 popup): the order its fields are shown in and
/// which of them are hidden. Persisted per connection/database/table, so a table opens with the
/// field set the user last chose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldLayout {
    /// Every field name in display order. A field the saved layout does not know (a column added to
    /// the table afterwards) is appended in the table's own order.
    #[serde(default)]
    pub order: Vec<String>,
    /// The fields currently hidden from the grid.
    #[serde(default)]
    pub hidden: Vec<String>,
}

/// One group in the connection tree. Groups nest: a group's `children` may hold both nested groups
/// and connections (referenced by their `ConnectionProfile::id`). Group names are not required to be
/// unique, so the numeric `id` is what identifies a group.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnectionGroup {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub expanded: bool,
    #[serde(default)]
    pub children: Vec<GroupEntry>,
}

/// One ordered entry of the connection tree: either a nested group or a connection (by profile id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum GroupEntry {
    Group(ConnectionGroup),
    /// A connection, identified by its `ConnectionProfile::id`.
    Connection(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SettingsFile {
    version: u32,
    #[serde(default)]
    theme: ThemeSetting,
    #[serde(default)]
    language: LanguageSetting,
    /// Whether the right-hand object-info pane is shown. Remembered across launches; it is
    /// `false` until the user chooses to reveal it.
    #[serde(default)]
    show_info_pane: bool,
    /// The remembered list layout per page, keyed by a stable page id (e.g. `"queries"`).
    #[serde(default)]
    view_modes: BTreeMap<String, String>,
    /// The SQL editor font family; empty means the built-in default.
    #[serde(default)]
    editor_font_family: String,
    /// The SQL editor font size in px.
    #[serde(default = "default_editor_font_size")]
    editor_font_size: u32,
    /// The navigation panel's (connection tree) font family; empty means the system default.
    #[serde(default)]
    nav_font_family: String,
    /// The navigation panel's font size in px.
    #[serde(default = "default_nav_font_size", alias = "tree_font_size")]
    nav_font_size: f32,
    /// The main panel's (content pane) font family; empty means the system default.
    #[serde(default)]
    main_font_family: String,
    /// The main panel's font size in px.
    #[serde(default = "default_nav_font_size")]
    main_font_size: f32,
    /// The table grid's font family; empty means the system default.
    #[serde(default)]
    grid_font_family: String,
    /// The table grid's font size in px.
    #[serde(default = "default_grid_font_size")]
    grid_font_size: f32,
    /// Ctrl/⌘ + mouse wheel over the navigation panel, main panel or table grid resizes its font.
    #[serde(default = "default_true")]
    ui_ctrl_wheel_zoom: bool,
    #[serde(default = "default_true")]
    editor_line_numbers: bool,
    #[serde(default = "default_true")]
    editor_word_wrap: bool,
    /// Ctrl + mouse wheel resizes the SQL editor's font (on by default).
    #[serde(default = "default_true")]
    editor_ctrl_wheel_zoom: bool,
    /// Connection-profile ids whose tree node is expanded.
    #[serde(default)]
    expanded_connections: Vec<String>,
    /// Expanded database names per connection-profile id, for restoring the tree.
    #[serde(default)]
    expanded_databases: BTreeMap<String, Vec<String>>,
    /// The connection tree's layout: nested groups (and the connections they hold), in display
    /// order. Connections are stored by profile id so the layout survives reordering the profiles.
    #[serde(default)]
    groups: Vec<GroupEntry>,
    /// The per-connection environment colour (Navicat-style), keyed by profile id so it survives
    /// reordering/renaming. The value is a colour id (`"red"`, `"green"` or `"blue"`).
    #[serde(default)]
    connection_colors: BTreeMap<String, String>,
    /// The remembered field layout (字段筛选) per table grid, keyed by a
    /// `"<connection id>\u{1f}<database>\u{1f}<table>"` string.
    #[serde(default)]
    grid_field_layouts: BTreeMap<String, FieldLayout>,
    /// Whether the grid header shows each column's comment under its name.
    #[serde(default)]
    grid_show_comments: bool,
    /// Whether the grid header shows each column's data type between its name and comment.
    #[serde(default)]
    grid_show_types: bool,
    /// Hide the transfer table rows whose comparison found no difference (on by default).
    #[serde(default = "default_true")]
    sync_hide_same: bool,
    /// The user's keymap overrides: command id -> keystrokes (gpui keystroke strings). Only
    /// commands the user changed are stored; an empty list unbinds the command.
    #[serde(default)]
    shortcuts: BTreeMap<String, Vec<String>>,
    /// Keep each list's own search text (表 / 视图 / 函数 / 查询) when switching tabs or
    /// databases (off: switching clears them). On by default; a file written before version 14
    /// turned it on as well, since its old default (`false`) was never chosen deliberately.
    #[serde(default = "default_true")]
    preserve_object_search: bool,
    /// Restore the previous session (query tabs, connections, open databases and table tabs) on
    /// the next start.
    #[serde(default = "default_true")]
    restore_session: bool,
    /// Open tree/list objects with a single click (off: double-click, the default).
    #[serde(default)]
    single_click_open: bool,
    /// The most statements one query run may execute (0: no limit).
    #[serde(default = "default_max_statements")]
    max_statements_per_run: u32,
    /// Stop a script at its first failing statement (off: run the rest and report each).
    #[serde(default)]
    stop_on_error: bool,
    /// Version 11's 运行 keystroke. Read so an upgrade keeps it (folded into `shortcuts`), never
    /// written.
    #[serde(default, skip_serializing)]
    run_shortcut: Option<String>,
    /// Version 11's 运行已选 keystroke; see `run_shortcut`.
    #[serde(default, skip_serializing)]
    run_selected_shortcut: Option<String>,
}

/// The default SQL editor font size, in px (VS Code's default `editor.fontSize`).
pub const DEFAULT_EDITOR_FONT_SIZE: u32 = 14;

/// The default font size of the navigation and main panels, in px (the app's own UI text size).
pub const DEFAULT_NAV_FONT_SIZE: f32 = 12.5;

/// The default font size of the table grid, in px.
pub const DEFAULT_GRID_FONT_SIZE: f32 = 12.5;

fn default_nav_font_size() -> f32 {
    DEFAULT_NAV_FONT_SIZE
}

fn default_grid_font_size() -> f32 {
    DEFAULT_GRID_FONT_SIZE
}

/// The keymap command ids the version-11 `run_shortcut` / `run_selected_shortcut` settings map
/// onto, and their built-in keystrokes then (Navicat's Ctrl+R / Ctrl+Shift+R).
const LEGACY_RUN_COMMAND: &str = "query.run";
const LEGACY_RUN_DEFAULT: &str = "secondary-r";
const LEGACY_RUN_SELECTED_COMMAND: &str = "query.run_selected";
const LEGACY_RUN_SELECTED_DEFAULT: &str = "secondary-shift-r";
/// Version 11 also bound Ctrl/⌘+Enter to 运行已选, unconditionally. It is now one of that
/// command's keystrokes, so a migrated custom keystroke keeps it alongside.
const LEGACY_RUN_SELECTED_ENTER: &str = "secondary-enter";

fn default_editor_font_size() -> u32 {
    DEFAULT_EDITOR_FONT_SIZE
}

fn default_true() -> bool {
    true
}

/// Fold version 11's two run keystrokes into the keymap overrides. A keystroke still at its old
/// default is not an override, and an existing override for the command wins.
fn fold_legacy_shortcuts(
    mut shortcuts: BTreeMap<String, Vec<String>>,
    run: Option<String>,
    run_selected: Option<String>,
) -> BTreeMap<String, Vec<String>> {
    for (command, value, default, also) in [
        (LEGACY_RUN_COMMAND, run, LEGACY_RUN_DEFAULT, None),
        (
            LEGACY_RUN_SELECTED_COMMAND,
            run_selected,
            LEGACY_RUN_SELECTED_DEFAULT,
            Some(LEGACY_RUN_SELECTED_ENTER),
        ),
    ] {
        if let Some(value) = value.filter(|value| value != default) {
            shortcuts.entry(command.to_string()).or_insert_with(|| {
                let also = also.filter(|also| *also != value).map(str::to_string);
                let mut keys = vec![value];
                keys.extend(also);
                keys
            });
        }
    }
    shortcuts
}

/// The default cap on statements per query run: a whole dump pasted into the editor by mistake
/// stops before it runs (Navicat caps it too). Options → General changes it; 0 removes it.
pub const DEFAULT_MAX_STATEMENTS: u32 = 20;

fn default_max_statements() -> u32 {
    DEFAULT_MAX_STATEMENTS
}

#[derive(Debug, Clone)]
pub struct AppSettings {
    pub theme: ThemeSetting,
    pub language: LanguageSetting,
    pub show_info_pane: bool,
    pub view_modes: BTreeMap<String, String>,
    /// The SQL editor font family; empty means the built-in default.
    pub editor_font_family: String,
    /// The SQL editor font size in px.
    pub editor_font_size: u32,
    /// The navigation panel's font family; empty means the system default.
    pub nav_font_family: String,
    /// The navigation panel's font size in px.
    pub nav_font_size: f32,
    /// The main panel's font family; empty means the system default.
    pub main_font_family: String,
    /// The main panel's font size in px.
    pub main_font_size: f32,
    /// The table grid's font family; empty means the system default.
    pub grid_font_family: String,
    /// The table grid's font size in px.
    pub grid_font_size: f32,
    /// Ctrl/⌘ + mouse wheel over a panel resizes its font.
    pub ui_ctrl_wheel_zoom: bool,
    pub editor_line_numbers: bool,
    pub editor_word_wrap: bool,
    /// Whether Ctrl + mouse wheel resizes the SQL editor's font in place.
    pub editor_ctrl_wheel_zoom: bool,
    /// Connection-profile ids whose tree node is expanded.
    pub expanded_connections: Vec<String>,
    /// Expanded database names per connection-profile id.
    pub expanded_databases: BTreeMap<String, Vec<String>>,
    /// The connection tree's layout (nested groups and their connections).
    pub groups: Vec<GroupEntry>,
    /// The per-connection environment colour, keyed by connection-profile id. The value is a
    /// colour id (`"red"`, `"green"` or `"blue"`); a missing entry means no colour.
    pub connection_colors: BTreeMap<String, String>,
    /// The remembered field layout (字段筛选) per table grid, keyed by connection/database/table.
    pub grid_field_layouts: BTreeMap<String, FieldLayout>,
    /// Whether the grid header shows each column's comment under its name.
    pub grid_show_comments: bool,
    /// Whether the grid header shows each column's data type between its name and comment.
    pub grid_show_types: bool,
    /// Hide the transfer table rows whose comparison found no difference.
    pub sync_hide_same: bool,
    /// The keymap overrides edited on the Options window's 快捷键 page: command id -> gpui
    /// keystroke strings (`"secondary-r"`). Commands not listed keep their built-in keystrokes;
    /// an empty list unbinds the command.
    pub shortcuts: BTreeMap<String, Vec<String>>,
    /// Keep each list's own search text (表 / 视图 / 函数 / 查询) when switching tabs or databases
    /// (off: switching clears them, so coming back shows every object).
    pub preserve_object_search: bool,
    /// Restore the previous session (open connections, query tabs, open databases and table tabs)
    /// on the next start.
    pub restore_session: bool,
    /// Open connections, databases, tables and the other tree/list objects with a single click
    /// (off: double-click, the default).
    pub single_click_open: bool,
    /// The most statements one query run may execute, against accidents (0: no limit).
    pub max_statements_per_run: u32,
    /// Stop a script at its first failing statement (off: run the rest and report each).
    pub stop_on_error: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeSetting::default(),
            language: LanguageSetting::default(),
            show_info_pane: false,
            view_modes: BTreeMap::new(),
            editor_font_family: String::new(),
            editor_font_size: DEFAULT_EDITOR_FONT_SIZE,
            nav_font_family: String::new(),
            nav_font_size: DEFAULT_NAV_FONT_SIZE,
            main_font_family: String::new(),
            main_font_size: DEFAULT_NAV_FONT_SIZE,
            grid_font_family: String::new(),
            grid_font_size: DEFAULT_GRID_FONT_SIZE,
            ui_ctrl_wheel_zoom: true,
            editor_line_numbers: true,
            editor_word_wrap: true,
            editor_ctrl_wheel_zoom: true,
            expanded_connections: Vec::new(),
            expanded_databases: BTreeMap::new(),
            groups: Vec::new(),
            connection_colors: BTreeMap::new(),
            grid_field_layouts: BTreeMap::new(),
            grid_show_comments: false,
            grid_show_types: false,
            sync_hide_same: true,
            shortcuts: BTreeMap::new(),
            preserve_object_search: true,
            restore_session: true,
            single_click_open: false,
            max_statements_per_run: DEFAULT_MAX_STATEMENTS,
            stop_on_error: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueriesFile {
    version: u32,
    #[serde(default)]
    queries: Vec<SavedQuery>,
}

impl Default for QueriesFile {
    fn default() -> Self {
        Self {
            version: QUERIES_VERSION,
            queries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupsFile {
    version: u32,
    #[serde(default)]
    backups: Vec<SavedBackup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TransfersFile {
    version: u32,
    #[serde(default)]
    transfers: Vec<SavedTransfer>,
}

/// The last database list seen for each connection (keyed by `ConnectionProfile::id`), so the save
/// dialog can offer databases without opening the connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DatabasesFile {
    version: u32,
    #[serde(default)]
    databases: BTreeMap<String, Vec<String>>,
}

impl Default for BackupsFile {
    fn default() -> Self {
        Self {
            version: BACKUPS_VERSION,
            backups: Vec::new(),
        }
    }
}

impl ConfigStore {
    pub fn new() -> ConfigResult<Self> {
        let dirs = ProjectDirs::from("", "", "rustgrid").ok_or(ConfigError::NoConfigDir)?;
        Ok(Self {
            root: dirs.config_dir().to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn profiles_path(&self) -> PathBuf {
        self.root.join(PROFILES_FILE)
    }

    pub fn settings_path(&self) -> PathBuf {
        self.root.join(SETTINGS_FILE)
    }

    pub fn session_path(&self) -> PathBuf {
        self.root.join(SESSION_FILE)
    }

    /// The last saved workspace session; empty when there is none or it cannot be read (a damaged
    /// session must never stop the app from starting).
    pub fn load_session(&self) -> WorkspaceSession {
        fs::read_to_string(self.session_path())
            .ok()
            .and_then(|contents| serde_json::from_str::<SessionFile>(&contents).ok())
            .map(|file| file.session)
            .unwrap_or_default()
    }

    /// Save the workspace session atomically (write a temporary file, then rename it over the old
    /// one), so a crash mid-write never leaves a half-written session.
    pub fn save_session(&self, session: &WorkspaceSession) -> ConfigResult<()> {
        fs::create_dir_all(&self.root)?;
        let file = SessionFile {
            version: SESSION_VERSION,
            session: session.clone(),
        };
        let contents = serde_json::to_string_pretty(&file)?;
        let temporary = self.root.join(format!("{SESSION_FILE}.tmp"));
        fs::write(&temporary, contents)?;
        fs::rename(&temporary, self.session_path())?;
        Ok(())
    }

    pub fn queries_path(&self) -> PathBuf {
        self.root.join(QUERIES_FILE)
    }

    /// The root directory of saved queries. Each query is a `.sql` file under
    /// `<root>/queries/<connection_id>/<database>/<name>.sql`, or under an extra
    /// `<schema>/` segment when the query was saved while a schema was selected. A group query is
    /// filed under `<root>/queries/_groups/<group_id>/<name>.sql`.
    pub fn queries_dir(&self) -> PathBuf {
        self.root.join(QUERIES_DIR)
    }

    /// The file a saved query is written to. A `schema` files the query one level deeper. A group
    /// query lives under `_groups/<group_id>/` instead of a connection/database folder.
    pub fn query_file_path(
        &self,
        connection_id: &str,
        database: &str,
        schema: Option<&str>,
        name: &str,
    ) -> PathBuf {
        let mut path = self
            .queries_dir()
            .join(sanitize_component(connection_id))
            .join(sanitize_component(database));
        if let Some(schema) = schema.filter(|schema| !schema.is_empty()) {
            path = path.join(sanitize_component(schema));
        }
        path.join(format!("{}.sql", sanitize_component(name)))
    }

    /// The folder holding group queries, one subfolder per connection-tree group.
    pub fn group_queries_dir(&self) -> PathBuf {
        self.queries_dir().join("_groups")
    }

    /// The file a saved group query is written to: `queries/_groups/<group_id>/<name>.sql`.
    pub fn group_query_file_path(&self, group_id: u64, name: &str) -> PathBuf {
        self.group_queries_dir()
            .join(group_id.to_string())
            .join(format!("{}.sql", sanitize_component(name)))
    }

    /// Write (or overwrite) one saved query's `.sql` file.
    pub fn save_query_file(&self, query: &SavedQuery) -> ConfigResult<PathBuf> {
        let path = match query.group_id {
            Some(group_id) => self.group_query_file_path(group_id, &query.name),
            None => self.query_file_path(
                &query.connection_id,
                &query.database,
                query.schema.as_deref(),
                &query.name,
            ),
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, &query.sql)?;
        Ok(path)
    }

    /// Remove one saved query's `.sql` file.
    pub fn delete_query_file(&self, query: &SavedQuery) -> ConfigResult<()> {
        let path = match query.group_id {
            Some(group_id) => self.group_query_file_path(group_id, &query.name),
            None => self.query_file_path(
                &query.connection_id,
                &query.database,
                query.schema.as_deref(),
                &query.name,
            ),
        };
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Move a set of legacy `queries.json` entries into the per-file tree (once). The old file is
    /// renamed to `queries.json.bak` so the migration does not run again.
    pub fn migrate_legacy_queries(&self) -> ConfigResult<()> {
        let path = self.queries_path();
        if !path.exists() {
            return Ok(());
        }
        if let Ok(contents) = fs::read_to_string(&path)
            && let Ok(file) = serde_json::from_str::<QueriesFile>(&contents)
        {
            for query in &file.queries {
                let _ = self.save_query_file(query);
            }
        }
        let _ = fs::rename(&path, self.root.join(format!("{QUERIES_FILE}.bak")));
        Ok(())
    }

    /// The root directory of this app's NB3 backups. Files live under
    /// `<root>/backups/<connection_id>/<database>/<name>.nb3`.
    pub fn backups_dir(&self) -> PathBuf {
        self.root.join(BACKUPS_DIR)
    }

    pub fn load_profiles(&self) -> ConfigResult<Vec<ConnectionProfile>> {
        let path = self.profiles_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let contents = fs::read_to_string(path)?;
        let file: ProfilesFile = serde_json::from_str(&contents)?;
        Ok(migrate(file).profiles)
    }

    pub fn save_profiles(&self, profiles: &[ConnectionProfile]) -> ConfigResult<()> {
        fs::create_dir_all(&self.root)?;
        let file = ProfilesFile {
            version: CURRENT_VERSION,
            profiles: profiles.to_vec(),
        };
        let contents = serde_json::to_string_pretty(&file)?;
        fs::write(self.profiles_path(), contents)?;
        Ok(())
    }

    pub fn load_secrets(&self) -> ConfigResult<BTreeMap<String, String>> {
        secrets::load(&self.root)
    }

    pub fn save_secrets(&self, secrets: &BTreeMap<String, String>) -> ConfigResult<()> {
        secrets::save(&self.root, secrets)
    }

    pub fn load_settings(&self) -> ConfigResult<AppSettings> {
        let path = self.settings_path();
        if !path.exists() {
            return Ok(AppSettings::default());
        }
        let contents = fs::read_to_string(path)?;
        let file: SettingsFile = serde_json::from_str(&contents)?;
        Ok(AppSettings {
            theme: file.theme,
            language: file.language,
            show_info_pane: file.show_info_pane,
            view_modes: file.view_modes,
            editor_font_family: file.editor_font_family,
            editor_font_size: file.editor_font_size,
            nav_font_family: file.nav_font_family,
            nav_font_size: file.nav_font_size,
            main_font_family: file.main_font_family,
            main_font_size: file.main_font_size,
            grid_font_family: file.grid_font_family,
            grid_font_size: file.grid_font_size,
            ui_ctrl_wheel_zoom: file.ui_ctrl_wheel_zoom,
            editor_line_numbers: file.editor_line_numbers,
            editor_word_wrap: file.editor_word_wrap,
            editor_ctrl_wheel_zoom: file.editor_ctrl_wheel_zoom,
            expanded_connections: file.expanded_connections,
            expanded_databases: file.expanded_databases,
            groups: file.groups,
            connection_colors: file.connection_colors,
            grid_field_layouts: file.grid_field_layouts,
            grid_show_comments: file.grid_show_comments,
            grid_show_types: file.grid_show_types,
            sync_hide_same: file.sync_hide_same,
            shortcuts: fold_legacy_shortcuts(
                file.shortcuts,
                file.run_shortcut,
                file.run_selected_shortcut,
            ),
            preserve_object_search: file.preserve_object_search
                || file.version < PRESERVE_OBJECT_SEARCH_SINCE,
            restore_session: file.restore_session,
            single_click_open: file.single_click_open,
            max_statements_per_run: file.max_statements_per_run,
            stop_on_error: file.stop_on_error,
        })
    }

    pub fn save_settings(&self, settings: &AppSettings) -> ConfigResult<()> {
        fs::create_dir_all(&self.root)?;
        let file = SettingsFile {
            version: SETTINGS_VERSION,
            theme: settings.theme,
            language: settings.language,
            show_info_pane: settings.show_info_pane,
            view_modes: settings.view_modes.clone(),
            editor_font_family: settings.editor_font_family.clone(),
            editor_font_size: settings.editor_font_size,
            nav_font_family: settings.nav_font_family.clone(),
            nav_font_size: settings.nav_font_size,
            main_font_family: settings.main_font_family.clone(),
            main_font_size: settings.main_font_size,
            grid_font_family: settings.grid_font_family.clone(),
            grid_font_size: settings.grid_font_size,
            ui_ctrl_wheel_zoom: settings.ui_ctrl_wheel_zoom,
            editor_line_numbers: settings.editor_line_numbers,
            editor_word_wrap: settings.editor_word_wrap,
            editor_ctrl_wheel_zoom: settings.editor_ctrl_wheel_zoom,
            expanded_connections: settings.expanded_connections.clone(),
            expanded_databases: settings.expanded_databases.clone(),
            groups: settings.groups.clone(),
            connection_colors: settings.connection_colors.clone(),
            grid_field_layouts: settings.grid_field_layouts.clone(),
            grid_show_comments: settings.grid_show_comments,
            grid_show_types: settings.grid_show_types,
            sync_hide_same: settings.sync_hide_same,
            shortcuts: settings.shortcuts.clone(),
            preserve_object_search: settings.preserve_object_search,
            restore_session: settings.restore_session,
            single_click_open: settings.single_click_open,
            max_statements_per_run: settings.max_statements_per_run,
            stop_on_error: settings.stop_on_error,
            run_shortcut: None,
            run_selected_shortcut: None,
        };
        let contents = serde_json::to_string_pretty(&file)?;
        fs::write(self.settings_path(), contents)?;
        Ok(())
    }

    pub fn load_queries(&self) -> ConfigResult<Vec<SavedQuery>> {
        let path = self.queries_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let contents = fs::read_to_string(path)?;
        let file: QueriesFile = serde_json::from_str(&contents)?;
        Ok(file.queries)
    }

    pub fn save_queries(&self, queries: &[SavedQuery]) -> ConfigResult<()> {
        fs::create_dir_all(&self.root)?;
        let file = QueriesFile {
            version: QUERIES_VERSION,
            queries: queries.to_vec(),
        };
        let contents = serde_json::to_string_pretty(&file)?;
        fs::write(self.queries_path(), contents)?;
        Ok(())
    }

    pub fn load_backups(&self) -> ConfigResult<Vec<SavedBackup>> {
        let path = self.root.join(BACKUPS_FILE);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let contents = fs::read_to_string(path)?;
        let file: BackupsFile = serde_json::from_str(&contents)?;
        Ok(file.backups)
    }

    pub fn save_backups(&self, backups: &[SavedBackup]) -> ConfigResult<()> {
        fs::create_dir_all(&self.root)?;
        let file = BackupsFile {
            version: BACKUPS_VERSION,
            backups: backups.to_vec(),
        };
        let contents = serde_json::to_string_pretty(&file)?;
        fs::write(self.root.join(BACKUPS_FILE), contents)?;
        Ok(())
    }

    /// The saved 传输 configurations (`transfers.json`), independent of any open connection.
    pub fn load_transfers(&self) -> ConfigResult<Vec<SavedTransfer>> {
        let path = self.root.join(TRANSFERS_FILE);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let contents = fs::read_to_string(path)?;
        let file: TransfersFile = serde_json::from_str(&contents)?;
        Ok(file.transfers)
    }

    pub fn save_transfers(&self, transfers: &[SavedTransfer]) -> ConfigResult<()> {
        fs::create_dir_all(&self.root)?;
        let file = TransfersFile {
            version: TRANSFERS_VERSION,
            transfers: transfers.to_vec(),
        };
        let contents = serde_json::to_string_pretty(&file)?;
        fs::write(self.root.join(TRANSFERS_FILE), contents)?;
        Ok(())
    }

    /// The remembered database names per connection profile id.
    pub fn load_database_cache(&self) -> ConfigResult<BTreeMap<String, Vec<String>>> {
        let path = self.root.join(DATABASES_FILE);
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let contents = fs::read_to_string(path)?;
        let file: DatabasesFile = serde_json::from_str(&contents)?;
        Ok(file.databases)
    }

    pub fn save_database_cache(
        &self,
        databases: &BTreeMap<String, Vec<String>>,
    ) -> ConfigResult<()> {
        fs::create_dir_all(&self.root)?;
        let file = DatabasesFile {
            version: DATABASES_VERSION,
            databases: databases.clone(),
        };
        let contents = serde_json::to_string_pretty(&file)?;
        fs::write(self.root.join(DATABASES_FILE), contents)?;
        Ok(())
    }
}

/// Make a filesystem-safe path component from a connection id or database name.
fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => character,
        })
        .collect()
}

fn migrate(mut file: ProfilesFile) -> ProfilesFile {
    if file.version < CURRENT_VERSION {
        file.version = CURRENT_VERSION;
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn a_workspace_session_round_trips_and_tolerates_missing_fields() {
        let session = WorkspaceSession {
            connections: vec!["conn-1".to_string()],
            queries: vec![SessionQuery {
                connection_id: Some("conn-1".to_string()),
                sql: Some("SELECT 1;".to_string()),
                caret: 9,
                ..Default::default()
            }],
            tables: vec![SessionTable {
                connection_id: "conn-1".to_string(),
                database: "shop".to_string(),
                table: "orders".to_string(),
                is_view: false,
                page_index: 2,
            }],
            active: Some(SessionActive::Table(0)),
            databases: vec![SessionDatabase {
                connection_id: "conn-1".to_string(),
                database: "shop".to_string(),
            }],
            current_database: Some(SessionDatabase {
                connection_id: "conn-1".to_string(),
                database: "shop".to_string(),
            }),
        };
        let json = serde_json::to_string(&SessionFile {
            version: SESSION_VERSION,
            session: session.clone(),
        })
        .expect("serialize");
        let back: SessionFile = serde_json::from_str(&json).expect("parse");
        assert_eq!(back.session, session);

        let minimal: SessionFile = serde_json::from_str(r#"{ "version": 1 }"#).expect("minimal");
        assert_eq!(minimal.session, WorkspaceSession::default());
    }

    #[test]
    fn legacy_run_shortcuts_at_their_defaults_add_no_override() {
        let folded = fold_legacy_shortcuts(
            BTreeMap::new(),
            Some(LEGACY_RUN_DEFAULT.to_string()),
            Some(LEGACY_RUN_SELECTED_DEFAULT.to_string()),
        );
        assert!(folded.is_empty());
    }

    #[test]
    fn changed_legacy_run_shortcuts_become_overrides() {
        let folded = fold_legacy_shortcuts(
            BTreeMap::new(),
            Some("f9".to_string()),
            Some("f8".to_string()),
        );
        assert_eq!(folded.get(LEGACY_RUN_COMMAND), Some(&keys(&["f9"])));
        // Version 11 always bound Ctrl/⌘+Enter to 运行已选 as well; the override keeps it.
        assert_eq!(
            folded.get(LEGACY_RUN_SELECTED_COMMAND),
            Some(&keys(&["f8", "secondary-enter"]))
        );
    }

    #[test]
    fn a_legacy_run_selected_of_ctrl_enter_is_not_listed_twice() {
        let folded =
            fold_legacy_shortcuts(BTreeMap::new(), None, Some("secondary-enter".to_string()));
        assert_eq!(
            folded.get(LEGACY_RUN_SELECTED_COMMAND),
            Some(&keys(&["secondary-enter"]))
        );
    }

    #[test]
    fn an_existing_override_wins_over_a_legacy_value() {
        let mut shortcuts = BTreeMap::new();
        shortcuts.insert(LEGACY_RUN_COMMAND.to_string(), Vec::new());
        let folded = fold_legacy_shortcuts(shortcuts, Some("f9".to_string()), None);
        assert_eq!(folded.get(LEGACY_RUN_COMMAND), Some(&Vec::new()));
    }

    #[test]
    fn version_11_settings_load_and_save_without_the_legacy_fields() {
        let v11 = r#"{ "version": 11, "run_shortcut": "f9", "run_selected_shortcut": "secondary-shift-r" }"#;
        let file: SettingsFile = serde_json::from_str(v11).expect("parse v11");
        let shortcuts = fold_legacy_shortcuts(
            file.shortcuts,
            file.run_shortcut,
            file.run_selected_shortcut,
        );
        assert_eq!(shortcuts.len(), 1);
        assert_eq!(shortcuts.get(LEGACY_RUN_COMMAND), Some(&keys(&["f9"])));

        let saved = SettingsFile {
            version: SETTINGS_VERSION,
            shortcuts,
            run_shortcut: Some("ignored".to_string()),
            run_selected_shortcut: None,
            ..serde_json::from_str(r#"{ "version": 12 }"#).expect("parse v12")
        };
        let json = serde_json::to_string(&saved).expect("serialize");
        assert!(!json.contains("run_shortcut"));
        assert!(json.contains(r#""query.run":["f9"]"#));
    }

    #[test]
    fn preserve_object_search_turns_on_once_and_restore_session_defaults_on() {
        // A file written before version 14 kept the old default (`false`); upgrading turns the
        // setting on, because "保留对象搜索" is on by default now.
        let old = r#"{ "version": 13, "preserve_object_search": false }"#;
        let file: SettingsFile = serde_json::from_str(old).expect("parse v13");
        assert!(file.preserve_object_search || file.version < PRESERVE_OBJECT_SEARCH_SINCE);
        assert!(file.restore_session, "启动时恢复 defaults to on");
        assert!(
            file.editor_ctrl_wheel_zoom,
            "Ctrl + wheel zoom defaults to on"
        );

        // A file written once the switch became a deliberate choice keeps the user's decision.
        let chosen = format!(
            r#"{{ "version": {PRESERVE_OBJECT_SEARCH_SINCE}, "preserve_object_search": false }}"#
        );
        let file: SettingsFile = serde_json::from_str(&chosen).expect("parse v14");
        assert!(!(file.preserve_object_search || file.version < PRESERVE_OBJECT_SEARCH_SINCE));
    }

    #[test]
    fn sync_hide_same_defaults_on_and_keeps_a_saved_choice() {
        let older: SettingsFile = serde_json::from_str(r#"{ "version": 14 }"#).expect("parse v14");
        assert!(older.sync_hide_same, "隐藏一致 defaults to on");

        let chosen: SettingsFile =
            serde_json::from_str(r#"{ "version": 15, "sync_hide_same": false }"#)
                .expect("parse v15");
        assert!(!chosen.sync_hide_same);
    }

    #[test]
    fn single_click_open_defaults_off_and_keeps_a_saved_choice() {
        // 单击模式 is off by default: objects open on double-click.
        let older: SettingsFile = serde_json::from_str(r#"{ "version": 15 }"#).expect("parse v15");
        assert!(!older.single_click_open);

        let chosen: SettingsFile =
            serde_json::from_str(r#"{ "version": 16, "single_click_open": true }"#)
                .expect("parse v16");
        assert!(chosen.single_click_open);
    }
}
