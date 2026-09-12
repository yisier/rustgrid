mod secrets;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use navidog_core::ConnectionProfile;
use serde::{Deserialize, Serialize};

const CURRENT_VERSION: u32 = 1;
const PROFILES_FILE: &str = "connections.json";
const SETTINGS_VERSION: u32 = 2;
const SETTINGS_FILE: &str = "settings.json";

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
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AppSettings {
    pub theme: ThemeSetting,
    pub language: LanguageSetting,
}

impl ConfigStore {
    pub fn new() -> ConfigResult<Self> {
        let dirs = ProjectDirs::from("", "", "navidog").ok_or(ConfigError::NoConfigDir)?;
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
        })
    }

    pub fn save_settings(&self, settings: &AppSettings) -> ConfigResult<()> {
        fs::create_dir_all(&self.root)?;
        let file = SettingsFile {
            version: SETTINGS_VERSION,
            theme: settings.theme,
            language: settings.language,
        };
        let contents = serde_json::to_string_pretty(&file)?;
        fs::write(self.settings_path(), contents)?;
        Ok(())
    }
}

fn migrate(mut file: ProfilesFile) -> ProfilesFile {
    if file.version < CURRENT_VERSION {
        file.version = CURRENT_VERSION;
    }
    file
}
