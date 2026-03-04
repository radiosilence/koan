use std::collections::HashMap;
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
    pub organize: OrganizeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryConfig {
    pub folders: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackConfig {
    pub software_volume: bool,
    pub replaygain: ReplayGainMode,
    /// Ticker scroll speed in frames-per-second (default: 8).
    /// The title scrolls one character per frame. Higher = faster scroll.
    pub ticker_fps: u8,
    /// UI render rate in frames-per-second (default: 60).
    /// Controls how often the TUI redraws. 30, 60, or 120 are typical values.
    pub target_fps: u8,
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
    /// Password — stored in config.local.toml (gitignored), not Keychain.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// original | opus-128 | mp3-320
    pub transcode_quality: String,
    /// Defaults to config_dir()/cache if empty.
    pub cache_dir: Option<PathBuf>,
    /// Parallel download workers for remote tracks (default: 5).
    pub download_workers: usize,
}

impl Default for LibraryConfig {
    fn default() -> Self {
        let music_dir = dirs::audio_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join("Music"))
                .unwrap_or_else(|| PathBuf::from("/Music"))
        });
        Self {
            folders: vec![music_dir],
        }
    }
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            software_volume: false,
            replaygain: ReplayGainMode::Album,
            ticker_fps: 8,
            target_fps: 60,
        }
    }
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            username: String::new(),
            password: String::new(),
            transcode_quality: "original".into(),
            cache_dir: None,
            download_workers: 5,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OrganizeConfig {
    /// Default named pattern to use when --pattern is omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Named patterns — keys are names, values are format strings.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub patterns: HashMap<String, String>,
}

impl OrganizeConfig {
    /// Resolve a pattern argument: if it matches a named pattern, return the stored
    /// format string. Otherwise return it as-is (raw format string).
    pub fn resolve_pattern<'a>(&'a self, name_or_raw: &'a str) -> &'a str {
        self.patterns
            .get(name_or_raw)
            .map(|s| s.as_str())
            .unwrap_or(name_or_raw)
    }

    /// Get the default pattern's format string, if configured.
    pub fn default_pattern(&self) -> Option<&str> {
        self.default
            .as_ref()
            .and_then(|name| self.patterns.get(name))
            .map(|s| s.as_str())
    }
}

impl Config {
    /// Load config.toml then deep-merge config.local.toml on top.
    /// Only keys actually present in config.local.toml override — missing keys
    /// keep their values from config.toml (not serde defaults).
    pub fn load() -> Result<Self, ConfigError> {
        let base_path = config_file_path();
        let local_path = config_local_file_path();

        let mut base_val: toml::Value = if base_path.exists() {
            let contents = fs::read_to_string(&base_path)?;
            toml::from_str(&contents)?
        } else {
            toml::Value::Table(toml::map::Map::new())
        };

        if local_path.exists() {
            let local_contents = fs::read_to_string(&local_path)?;
            let local_val: toml::Value = toml::from_str(&local_contents)?;
            deep_merge(&mut base_val, local_val);
        }

        let config: Config = base_val.try_into()?;
        Ok(config)
    }

    /// Load from a specific path.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Write config to the base config.toml.
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = config_file_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        fs::write(&path, contents)?;
        Ok(())
    }

    /// Write config to config.local.toml (for machine-specific / sensitive values).
    pub fn save_local(&self) -> Result<(), ConfigError> {
        let path = config_local_file_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        fs::write(&path, contents)?;
        Ok(())
    }

    /// Resolved cache directory — uses explicit setting or defaults to config_dir/cache.
    pub fn cache_dir(&self) -> PathBuf {
        self.remote
            .cache_dir
            .clone()
            .unwrap_or_else(|| config_dir().join("cache"))
    }
}

/// Recursively merge `overlay` into `base`. Only keys present in `overlay`
/// are touched — everything else in `base` is preserved.
fn deep_merge(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_map), toml::Value::Table(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let entry = base_map
                    .entry(key)
                    .or_insert(toml::Value::Table(toml::map::Map::new()));
                deep_merge(entry, overlay_val);
            }
        }
        (base, overlay) => {
            *base = overlay;
        }
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
        assert_eq!(cfg.playback.replaygain, ReplayGainMode::Album);
        assert!(!cfg.remote.enabled);
        assert_eq!(cfg.remote.transcode_quality, "original");
    }

    #[test]
    fn test_roundtrip_toml() {
        let cfg = Config::default();
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.playback.replaygain, cfg.playback.replaygain);
        assert_eq!(
            deserialized.remote.transcode_quality,
            cfg.remote.transcode_quality
        );
    }

    #[test]
    fn test_load_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[library]
folders = ["/tmp/music"]

[playback]
replaygain = "track"
"#,
        )
        .unwrap();

        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.library.folders, vec![PathBuf::from("/tmp/music")]);
        assert_eq!(cfg.playback.replaygain, ReplayGainMode::Track);
        assert!(!cfg.remote.enabled);
    }

    #[test]
    fn test_partial_toml_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.toml");
        fs::write(&path, "[playback]\nsoftware_volume = true\n").unwrap();

        let cfg = Config::load_from(&path).unwrap();
        assert!(cfg.playback.software_volume);
    }

    #[test]
    fn test_deep_merge_local_overrides_base() {
        // Test deep_merge directly on TOML values (no temp files needed).
        let base_toml = r#"
[library]
folders = ["/base/music"]

[remote]
url = "https://base.example.com"
"#;
        let local_toml = r#"
[library]
folders = ["/local/music"]

[remote]
enabled = true
url = "https://local.example.com"
username = "admin"
"#;

        let mut base_val: toml::Value = toml::from_str(base_toml).unwrap();
        let local_val: toml::Value = toml::from_str(local_toml).unwrap();
        deep_merge(&mut base_val, local_val);

        let cfg: Config = base_val.try_into().unwrap();
        assert_eq!(cfg.library.folders, vec![PathBuf::from("/local/music")]);
        assert!(cfg.remote.enabled);
        assert_eq!(cfg.remote.url, "https://local.example.com");
        assert_eq!(cfg.remote.username, "admin");
    }

    #[test]
    fn test_deep_merge_missing_keys_preserved() {
        let base_toml = r#"
[remote]
url = "https://keep.me"
username = "keepuser"
"#;
        // Local only sets password — url and username should survive.
        let local_toml = r#"
[remote]
password = "secret"
"#;

        let mut base_val: toml::Value = toml::from_str(base_toml).unwrap();
        let local_val: toml::Value = toml::from_str(local_toml).unwrap();
        deep_merge(&mut base_val, local_val);

        let cfg: Config = base_val.try_into().unwrap();
        assert_eq!(cfg.remote.url, "https://keep.me");
        assert_eq!(cfg.remote.username, "keepuser");
        assert_eq!(cfg.remote.password, "secret");
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

    #[test]
    fn test_organize_config_defaults() {
        let cfg = Config::default();
        assert!(cfg.organize.default.is_none());
        assert!(cfg.organize.patterns.is_empty());
    }

    #[test]
    fn test_organize_config_from_toml() {
        let dir = tmp_dir();
        let path = dir.join("organize.toml");
        fs::write(
            &path,
            r#"
[organize]
default = "standard"

[organize.patterns]
standard = "%album artist%/(%date%) %album%/%tracknumber%. %title%"
va-aware = "%album artist%/$if($stricmp(%album artist%,Various Artists),,%album%)"
"#,
        )
        .unwrap();

        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.organize.default.as_deref(), Some("standard"));
        assert_eq!(cfg.organize.patterns.len(), 2);
        assert!(cfg.organize.patterns.contains_key("standard"));
        assert!(cfg.organize.patterns.contains_key("va-aware"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_organize_resolve_named_pattern() {
        let mut cfg = OrganizeConfig::default();
        cfg.patterns
            .insert("standard".into(), "%artist%/%title%".into());

        assert_eq!(cfg.resolve_pattern("standard"), "%artist%/%title%");
        // Unknown name falls through as raw pattern
        assert_eq!(cfg.resolve_pattern("%raw%pattern%"), "%raw%pattern%");
    }

    #[test]
    fn test_organize_default_pattern() {
        let mut cfg = OrganizeConfig {
            default: Some("standard".into()),
            ..OrganizeConfig::default()
        };
        cfg.patterns
            .insert("standard".into(), "%artist%/%title%".into());

        assert_eq!(cfg.default_pattern(), Some("%artist%/%title%"));
    }

    #[test]
    fn test_organize_default_pattern_missing_name() {
        let cfg = OrganizeConfig {
            default: Some("nonexistent".into()),
            ..OrganizeConfig::default()
        };
        // Name doesn't match any pattern → None
        assert_eq!(cfg.default_pattern(), None);
    }

    #[test]
    fn test_deep_merge_organize_patterns() {
        let base_toml = r#"
[organize]
default = "standard"

[organize.patterns]
standard = "base-pattern"
"#;
        let local_toml = r#"
[organize]
default = "custom"

[organize.patterns]
custom = "local-pattern"
"#;

        let mut base_val: toml::Value = toml::from_str(base_toml).unwrap();
        let local_val: toml::Value = toml::from_str(local_toml).unwrap();
        deep_merge(&mut base_val, local_val);

        let cfg: Config = base_val.try_into().unwrap();
        // Local default wins
        assert_eq!(cfg.organize.default.as_deref(), Some("custom"));
        // Both patterns present (deep merge into [organize.patterns] table)
        assert_eq!(cfg.organize.patterns.len(), 2);
        assert_eq!(cfg.organize.patterns["standard"], "base-pattern");
        assert_eq!(cfg.organize.patterns["custom"], "local-pattern");
    }

    #[test]
    fn test_organize_config_roundtrip() {
        let mut cfg = Config::default();
        cfg.organize.default = Some("standard".into());
        cfg.organize
            .patterns
            .insert("standard".into(), "%artist%/%title%".into());

        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.organize.default.as_deref(), Some("standard"));
        assert_eq!(
            deserialized.organize.patterns["standard"],
            "%artist%/%title%"
        );
    }
}
