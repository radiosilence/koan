use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("serialize error: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub library: LibraryConfig,
    pub playback: PlaybackConfig,
    pub remote: RemoteConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryConfig {
    pub folders: Vec<PathBuf>,
    pub watch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackConfig {
    pub exclusive_mode: bool,
    pub software_volume: bool,
    pub replaygain: ReplayGainMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplayGainMode {
    Off,
    Track,
    Album,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteConfig {
    pub enabled: bool,
    pub url: String,
    pub username: String,
    /// original | opus-128 | mp3-320
    pub transcode_quality: String,
    /// Defaults to data_dir()/cache if empty.
    pub cache_dir: Option<PathBuf>,
}

impl Default for LibraryConfig {
    fn default() -> Self {
        let music_dir = dirs::audio_dir().unwrap_or_else(|| PathBuf::from("~/Music"));
        Self {
            folders: vec![music_dir],
            watch: true,
        }
    }
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            exclusive_mode: false,
            software_volume: false,
            replaygain: ReplayGainMode::Album,
        }
    }
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            username: String::new(),
            transcode_quality: "original".into(),
            cache_dir: None,
        }
    }
}

impl Config {
    /// Load config from disk, falling back to defaults if missing.
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_file_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(&path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Load from a specific path.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Write config to disk, creating directories if needed.
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = config_file_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        fs::write(&path, contents)?;
        Ok(())
    }

    /// Resolved cache directory — uses explicit setting or defaults to data_dir/cache.
    pub fn cache_dir(&self) -> PathBuf {
        self.remote
            .cache_dir
            .clone()
            .unwrap_or_else(|| data_dir().join("cache"))
    }
}

/// `~/.config/koan/`
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("koan")
}

/// `~/.local/share/koan/`
pub fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("koan")
}

/// Path to the config TOML file.
pub fn config_file_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Path to the database file.
pub fn db_path() -> PathBuf {
    data_dir().join("koan.db")
}
