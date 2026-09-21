mod secrets;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use rustgrid_core::{ConnectionProfile, SavedBackup, SavedQuery};
use serde::{Deserialize, Serialize};

const CURRENT_VERSION: u32 = 1;
const PROFILES_FILE: &str = "connections.json";
const SETTINGS_VERSION: u32 = 3;
const SETTINGS_FILE: &str = "settings.json";
const QUERIES_VERSION: u32 = 1;
const QUERIES_FILE: &str = "queries.json";
const QUERIES_DIR: &str = "queries";
const BACKUPS_VERSION: u32 = 1;
const BACKUPS_FILE: &str = "backups.json";
const BACKUPS_DIR: &str = "backups";

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
}

impl LanguageSetting {
    pub fn locale(self) -> &'static str {
        match self {
            LanguageSetting::En => "en",
            LanguageSetting::ZhCn => "zh-CN",
        }
    }
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
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AppSettings {
    pub theme: ThemeSetting,
    pub language: LanguageSetting,
    pub show_info_pane: bool,
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

    pub fn queries_path(&self) -> PathBuf {
        self.root.join(QUERIES_FILE)
    }

    /// The root directory of saved queries. Each query is a `.sql` file under
    /// `<root>/queries/<connection_id>/<database>/<name>.sql`, mirroring the backups tree.
    pub fn queries_dir(&self) -> PathBuf {
        self.root.join(QUERIES_DIR)
    }

    /// The file a saved query is written to.
    pub fn query_file_path(&self, connection_id: &str, database: &str, name: &str) -> PathBuf {
        self.queries_dir()
            .join(sanitize_component(connection_id))
            .join(sanitize_component(database))
            .join(format!("{}.sql", sanitize_component(name)))
    }

    /// Write (or overwrite) one saved query's `.sql` file.
    pub fn save_query_file(&self, query: &SavedQuery) -> ConfigResult<PathBuf> {
        let path = self.query_file_path(&query.connection_id, &query.database, &query.name);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, &query.sql)?;
        Ok(path)
    }

    /// Remove one saved query's `.sql` file.
    pub fn delete_query_file(&self, query: &SavedQuery) -> ConfigResult<()> {
        let path = self.query_file_path(&query.connection_id, &query.database, &query.name);
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
        })
    }

    pub fn save_settings(&self, settings: &AppSettings) -> ConfigResult<()> {
        fs::create_dir_all(&self.root)?;
        let file = SettingsFile {
            version: SETTINGS_VERSION,
            theme: settings.theme,
            language: settings.language,
            show_info_pane: settings.show_info_pane,
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
