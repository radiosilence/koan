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
    /// Defaults to config_dir()/cache if empty.
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
    /// Load config.toml then overlay config.local.toml on top.
    /// Local overrides win — use it for machine-specific paths, credentials, etc.
    pub fn load() -> Result<Self, ConfigError> {
        let base_path = config_file_path();
        let local_path = config_local_file_path();

        let mut config = if base_path.exists() {
            let contents = fs::read_to_string(&base_path)?;
            toml::from_str(&contents)?
        } else {
            Self::default()
        };

        if local_path.exists() {
            let local_contents = fs::read_to_string(&local_path)?;
            let local: Config = toml::from_str(&local_contents)?;
            config.merge(local);
        }

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

    /// Merge another config on top — non-default/non-empty values from `other` win.
    fn merge(&mut self, other: Config) {
        if !other.library.folders.is_empty() {
            self.library.folders = other.library.folders;
        }
        self.library.watch = other.library.watch;
        self.playback = other.playback;
        if other.remote.enabled {
            self.remote.enabled = true;
        }
        if !other.remote.url.is_empty() {
            self.remote.url = other.remote.url;
        }
        if !other.remote.username.is_empty() {
            self.remote.username = other.remote.username;
        }
        if !other.remote.transcode_quality.is_empty() {
            self.remote.transcode_quality = other.remote.transcode_quality;
        }
        if other.remote.cache_dir.is_some() {
            self.remote.cache_dir = other.remote.cache_dir;
        }
    }

    /// Resolved cache directory — uses explicit setting or defaults to config_dir/cache.
    pub fn cache_dir(&self) -> PathBuf {
        self.remote
            .cache_dir
            .clone()
            .unwrap_or_else(|| config_dir().join("cache"))
    }
}

/// `~/.config/koan/`
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("koan")
}

/// Path to the base config TOML file (committable to dotfiles).
pub fn config_file_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Path to the local override config (gitignored, machine-specific).
pub fn config_local_file_path() -> PathBuf {
    config_dir().join("config.local.toml")
}

/// Path to the database file.
pub fn db_path() -> PathBuf {
    config_dir().join("koan.db")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("koan-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_defaults() {
        let cfg = Config::default();
        assert!(cfg.library.watch);
        assert!(!cfg.playback.exclusive_mode);
        assert_eq!(cfg.playback.replaygain, ReplayGainMode::Album);
        assert!(!cfg.remote.enabled);
        assert_eq!(cfg.remote.transcode_quality, "original");
    }

    #[test]
    fn test_roundtrip_toml() {
        let cfg = Config::default();
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.library.watch, cfg.library.watch);
        assert_eq!(deserialized.playback.replaygain, cfg.playback.replaygain);
        assert_eq!(
            deserialized.remote.transcode_quality,
            cfg.remote.transcode_quality
        );
    }

    #[test]
    fn test_load_from_file() {
        let dir = tmp_dir();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            r#"
[library]
folders = ["/tmp/music"]
watch = false

[playback]
replaygain = "track"
"#,
        )
        .unwrap();

        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.library.folders, vec![PathBuf::from("/tmp/music")]);
        assert!(!cfg.library.watch);
        assert_eq!(cfg.playback.replaygain, ReplayGainMode::Track);
        // Remote should be default since not in file.
        assert!(!cfg.remote.enabled);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_partial_toml_uses_defaults() {
        let dir = tmp_dir();
        let path = dir.join("partial.toml");
        fs::write(&path, "[playback]\nexclusive_mode = true\n").unwrap();

        let cfg = Config::load_from(&path).unwrap();
        assert!(cfg.playback.exclusive_mode);
        // Library should get defaults.
        assert!(cfg.library.watch);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_merge_local_overrides_base() {
        let mut base = Config::default();
        base.library.folders = vec![PathBuf::from("/base/music")];
        base.remote.url = "https://base.example.com".into();

        let mut local = Config::default();
        local.library.folders = vec![PathBuf::from("/local/music")];
        local.remote.enabled = true;
        local.remote.url = "https://local.example.com".into();
        local.remote.username = "admin".into();

        base.merge(local);

        assert_eq!(base.library.folders, vec![PathBuf::from("/local/music")]);
        assert!(base.remote.enabled);
        assert_eq!(base.remote.url, "https://local.example.com");
        assert_eq!(base.remote.username, "admin");
    }

    #[test]
    fn test_merge_empty_fields_dont_clobber() {
        let mut base = Config::default();
        base.remote.url = "https://keep.me".into();
        base.remote.username = "keepuser".into();

        let local = Config::default(); // empty remote fields
        base.merge(local);

        // Empty strings shouldn't overwrite.
        assert_eq!(base.remote.url, "https://keep.me");
        assert_eq!(base.remote.username, "keepuser");
    }

    #[test]
    fn test_cache_dir_default() {
        let cfg = Config::default();
        assert!(cfg.cache_dir().ends_with("cache"));
    }

    #[test]
    fn test_cache_dir_explicit() {
        let mut cfg = Config::default();
        cfg.remote.cache_dir = Some(PathBuf::from("/custom/cache"));
        assert_eq!(cfg.cache_dir(), PathBuf::from("/custom/cache"));
    }
}
