use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
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
    #[error("config error: {0}")]
    Figment(#[from] Box<figment::Error>),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub library: LibraryConfig,
    pub playback: PlaybackConfig,
    pub remote: RemoteConfig,
    pub organize: OrganizeConfig,
    #[serde(alias = "visualiser")]
    pub visualizer: VisualizerConfig,
    pub radio: RadioConfig,
    pub graphql: GraphqlConfig,
    pub discovery: DiscoveryConfig,
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
    /// Show an FPS counter overlay in the top-right corner.
    pub show_fps: bool,
    /// ReplayGain pre-amplification in dB. Applied on top of track/album gain.
    /// Positive values boost, negative values attenuate. Default: 0.0.
    pub pre_amp_db: f64,
    /// Output audio device name. None = system default.
    /// Persisted by name (not ID) since IDs can change across reboots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_device: Option<String>,
    /// Album art width in terminal columns (default: 24).
    /// Height is always width/2 (square via halfblock rendering).
    pub art_size: u16,
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
    /// Password — stored in config.local.toml (gitignored), not config.toml.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// original | opus-128 | mp3-320
    pub transcode_quality: String,
    /// Defaults to config_dir()/cache if empty.
    pub cache_dir: Option<PathBuf>,
    /// Parallel download workers for remote tracks (default: 5).
    pub download_workers: usize,
    /// Maximum cache size on disk. Human-readable: "50GB", "500MB", etc.
    /// None or empty = unlimited. LRU eviction runs on startup when exceeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_limit: Option<String>,
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
            replaygain: ReplayGainMode::Off,
            ticker_fps: 8,
            target_fps: 60,
            show_fps: false,
            pre_amp_db: 0.0,
            output_device: None,
            art_size: 24,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualizerConfig {
    pub enabled: bool,
    pub fps: u8,
    /// Frequency scale: "bark" (default), "mel", "log", "linear".
    pub scale: String,
    /// Amplitude scale: "aweight" (default, A-weighted), "perceptual" (A-weighted + gamma), "sqrt", "linear".
    pub amplitude_scale: String,
    /// Bar decay half-life in milliseconds (how fast bars drop).
    pub bar_decay_ms: u32,
    /// Peak decay half-life in milliseconds (how long peaks linger).
    pub peak_decay_ms: u32,
    /// Color palette: "spectrum" (default), "mono", "fire", "neon".
    /// Controls the frequency-mapped color gradient on spectrum bars.
    pub palette: String,
}

impl Default for VisualizerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fps: 60,
            scale: "bark".into(),
            amplitude_scale: "aweight".into(),
            bar_decay_ms: 50,
            peak_decay_ms: 180,
            palette: "spectrum".into(),
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
            cache_limit: None,
        }
    }
}

/// Parse a human-readable size string like "50GB", "500 MB", "1.5TB" into bytes.
/// Supports B, KB, MB, GB, TB (case-insensitive). Returns None for invalid input.
pub fn parse_size_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Split into numeric part and suffix.
    let mut num_end = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || c == '.' {
            num_end = i + c.len_utf8();
        } else if !c.is_whitespace() {
            break;
        }
    }

    let num_str = s[..num_end].trim();
    let suffix = s[num_end..].trim().to_ascii_uppercase();

    let value: f64 = num_str.parse().ok()?;
    let multiplier: u64 = match suffix.as_str() {
        "" | "B" => 1,
        "KB" | "K" => 1024,
        "MB" | "M" => 1024 * 1024,
        "GB" | "G" => 1024 * 1024 * 1024,
        "TB" | "T" => 1024 * 1024 * 1024 * 1024,
        _ => return None,
    };

    Some((value * multiplier as f64) as u64)
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

/// GraphQL API server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphqlConfig {
    /// Enable the GraphQL API server alongside the TUI (default: true).
    /// Set to false for TUI-only mode (equivalent to --no-api).
    pub enabled: bool,
    /// GraphQL API port (default: 4000).
    pub port: u16,
    /// Bind address for the API server (default: 127.0.0.1).
    /// Use "0.0.0.0" to listen on all interfaces (NOT RECOMMENDED without auth).
    #[serde(default = "default_bind")]
    pub bind: std::net::IpAddr,
    /// Enable GraphiQL web IDE at GET /graphql.
    pub playground: bool,
    /// Enable Subsonic REST API on this port. Omit or null to disable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subsonic_port: Option<u16>,
}

fn default_bind() -> std::net::IpAddr {
    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
}

impl Default for GraphqlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 4000,
            bind: default_bind(),
            playground: false,
            subsonic_port: None,
        }
    }
}

/// Radio / infinite play mode configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RadioConfig {
    /// Number of tracks to keep queued ahead of the cursor.
    pub lookahead: usize,
    /// Number of tracks to add each time the queue runs low.
    pub batch_size: usize,
    /// Use Subsonic getSimilarSongs2 when a remote server is configured.
    pub use_subsonic: bool,
    /// Don't repeat any of the last N tracks (play history exclusion window).
    pub history_window: usize,
    /// Number of recently played tracks to use as seed (drifting seed window).
    pub seed_window: usize,
    /// Discovery weight: 0.0 = only familiar tracks, 1.0 = maximise discovery.
    /// Controls the recency bonus — higher values boost never-played/long-forgotten tracks.
    pub discovery_weight: f64,
}

impl Default for RadioConfig {
    fn default() -> Self {
        Self {
            lookahead: 5,
            batch_size: 5,
            use_subsonic: true,
            history_window: 200,
            seed_window: 5,
            discovery_weight: 0.3,
        }
    }
}

/// Acoustic analysis / discovery configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoveryConfig {
    /// Run acoustic analysis automatically after library scan (default: false).
    pub analysis_on_scan: bool,
    /// Weight for acoustic similarity signal in radio mode scoring (0.0..1.0).
    pub acoustic_weight: f64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            analysis_on_scan: false,
            acoustic_weight: 0.5,
        }
    }
}

impl Config {
    /// Build the figment provider chain:
    /// defaults → config.toml → config.local.toml → KOAN_* env vars.
    ///
    /// Env vars use `KOAN_` prefix with `__` as section separator:
    ///   KOAN_REMOTE__PASSWORD, KOAN_GRAPHQL__PORT, KOAN_PLAYBACK__TARGET_FPS, etc.
    fn figment() -> Figment {
        let base_path = config_file_path();
        let local_path = config_local_file_path();

        Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(&base_path))
            .merge(Toml::file(&local_path))
            .merge(Env::prefixed("KOAN_").split("__"))
    }

    /// Load config from all layers: defaults → config.toml → config.local.toml → KOAN_* env vars.
    pub fn load() -> Result<Self, ConfigError> {
        Self::figment()
            .extract()
            .map_err(|e| ConfigError::Figment(Box::new(e)))
    }

    /// Load config, logging and falling back to defaults on error.
    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|e| {
            log::warn!("failed to load config, using defaults: {}", e);
            Self::default()
        })
    }

    /// Load from a specific TOML file (no env var overlay).
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Patch config.toml with a mutation closure. Reads the base file only (not
    /// config.local.toml or env vars), applies the closure, writes back.
    /// This prevents secrets from config.local.toml or env vars leaking into config.toml.
    pub fn update_base<F>(mutate: F) -> Result<(), ConfigError>
    where
        F: FnOnce(&mut Config),
    {
        let path = config_file_path();
        let mut cfg = if path.exists() {
            Config::load_from(&path)?
        } else {
            Config::default()
        };
        mutate(&mut cfg);
        cfg.write_to(&path)?;
        Ok(())
    }

    /// Write this config to a specific path as TOML.
    fn write_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)?;
        fs::write(path, contents)?;
        Ok(())
    }

    /// Patch a single section in config.local.toml, preserving all other content.
    /// Creates the file if it doesn't exist. Sets 0o600 permissions on Unix.
    pub fn patch_local(
        section: &str,
        values: &toml::map::Map<String, toml::Value>,
    ) -> Result<(), ConfigError> {
        let path = config_local_file_path();
        let mut doc: toml::Value = if path.exists() {
            let contents = fs::read_to_string(&path)?;
            toml::from_str(&contents).unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()))
        } else {
            toml::Value::Table(toml::map::Map::new())
        };

        let table = doc.as_table_mut().expect("root is always a table");
        let section_table = table
            .entry(section)
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
            .as_table_mut()
            .ok_or_else(|| {
                ConfigError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("[{}] is not a table", section),
                ))
            })?;

        for (key, value) in values {
            section_table.insert(key.clone(), value.clone());
        }

        let contents = toml::to_string_pretty(&doc)?;
        fs::write(&path, contents)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// Resolved cache directory — uses explicit setting or defaults to config_dir/cache.
    pub fn cache_dir(&self) -> PathBuf {
        self.remote
            .cache_dir
            .clone()
            .unwrap_or_else(|| config_dir().join("cache"))
    }

    /// Parsed cache limit in bytes, or None if unlimited.
    pub fn cache_limit_bytes(&self) -> Option<u64> {
        self.remote
            .cache_limit
            .as_deref()
            .and_then(parse_size_bytes)
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
        assert_eq!(cfg.playback.replaygain, ReplayGainMode::Off);
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
    fn test_figment_layered_loading() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("config.toml");
        let local_path = dir.path().join("config.local.toml");

        fs::write(
            &base_path,
            r#"
[remote]
url = "https://base.example.com"
"#,
        )
        .unwrap();
        fs::write(
            &local_path,
            r#"
[remote]
enabled = true
url = "https://local.example.com"
username = "admin"
password = "secret"
"#,
        )
        .unwrap();

        // Build a figment with explicit paths (can't use load() since it reads from ~/.config).
        let cfg: Config = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(&base_path))
            .merge(Toml::file(&local_path))
            .extract()
            .unwrap();

        assert!(cfg.remote.enabled);
        assert_eq!(cfg.remote.url, "https://local.example.com");
        assert_eq!(cfg.remote.username, "admin");
        assert_eq!(cfg.remote.password, "secret");
    }

    #[test]
    fn test_figment_missing_keys_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("config.toml");
        let local_path = dir.path().join("config.local.toml");

        fs::write(
            &base_path,
            r#"
[remote]
url = "https://keep.me"
username = "keepuser"
"#,
        )
        .unwrap();
        fs::write(
            &local_path,
            r#"
[remote]
password = "secret"
"#,
        )
        .unwrap();

        let cfg: Config = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(&base_path))
            .merge(Toml::file(&local_path))
            .extract()
            .unwrap();

        assert_eq!(cfg.remote.url, "https://keep.me");
        assert_eq!(cfg.remote.username, "keepuser");
        assert_eq!(cfg.remote.password, "secret");
    }

    #[test]
    fn test_env_var_override() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("config.toml");

        fs::write(
            &base_path,
            r#"
[remote]
url = "https://file.example.com"
"#,
        )
        .unwrap();

        // SAFETY: test is single-threaded and vars are cleaned up immediately after.
        unsafe {
            std::env::set_var("KOAN_REMOTE__URL", "https://env.example.com");
            std::env::set_var("KOAN_REMOTE__PASSWORD", "env-secret");
            std::env::set_var("KOAN_GRAPHQL__PORT", "9999");
        }

        let cfg: Config = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(&base_path))
            .merge(Env::prefixed("KOAN_").split("__"))
            .extract()
            .unwrap();

        assert_eq!(cfg.remote.url, "https://env.example.com");
        assert_eq!(cfg.remote.password, "env-secret");
        assert_eq!(cfg.graphql.port, 9999);

        // Clean up env vars.
        unsafe {
            std::env::remove_var("KOAN_REMOTE__URL");
            std::env::remove_var("KOAN_REMOTE__PASSWORD");
            std::env::remove_var("KOAN_GRAPHQL__PORT");
        }
    }

    #[test]
    fn test_update_base_does_not_leak_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("config.toml");

        // Write an initial base config.
        fs::write(
            &base_path,
            r#"
[playback]
target_fps = 60

[remote]
url = "https://base.example.com"
"#,
        )
        .unwrap();

        // Simulate: update_base patches base config only.
        let mut base_cfg = Config::load_from(&base_path).unwrap();
        base_cfg.visualizer.enabled = false;
        base_cfg.write_to(&base_path).unwrap();

        // Verify: no password leaked into config.toml.
        let written = fs::read_to_string(&base_path).unwrap();
        assert!(!written.contains("secret"));
        assert!(!written.contains("password"));

        // Verify the field was saved.
        let reloaded = Config::load_from(&base_path).unwrap();
        assert!(!reloaded.visualizer.enabled);
        assert_eq!(reloaded.remote.url, "https://base.example.com");
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
    fn test_figment_organize_patterns_merge() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("config.toml");
        let local_path = dir.path().join("config.local.toml");

        fs::write(
            &base_path,
            r#"
[organize]
default = "standard"

[organize.patterns]
standard = "base-pattern"
"#,
        )
        .unwrap();
        fs::write(
            &local_path,
            r#"
[organize]
default = "custom"

[organize.patterns]
custom = "local-pattern"
"#,
        )
        .unwrap();

        let cfg: Config = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(&base_path))
            .merge(Toml::file(&local_path))
            .extract()
            .unwrap();

        // Local default wins.
        assert_eq!(cfg.organize.default.as_deref(), Some("custom"));
        // Both patterns present (figment merges maps).
        assert_eq!(cfg.organize.patterns.len(), 2);
        assert_eq!(cfg.organize.patterns["standard"], "base-pattern");
        assert_eq!(cfg.organize.patterns["custom"], "local-pattern");
    }

    #[test]
    fn test_output_device_config_roundtrip() {
        let mut cfg = Config::default();
        cfg.playback.output_device = Some("My DAC".into());

        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(
            deserialized.playback.output_device.as_deref(),
            Some("My DAC")
        );
    }

    #[test]
    fn test_output_device_config_default_is_none() {
        let cfg = Config::default();
        assert!(cfg.playback.output_device.is_none());

        // Roundtrip: None should not appear in serialized output.
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        assert!(!serialized.contains("output_device"));
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert!(deserialized.playback.output_device.is_none());
    }

    #[test]
    fn test_output_device_config_from_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[playback]
output_device = "External Speakers"
"#,
        )
        .unwrap();

        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(
            cfg.playback.output_device.as_deref(),
            Some("External Speakers")
        );
    }

    #[test]
    fn test_graphql_bind_defaults_to_localhost() {
        let cfg = GraphqlConfig::default();
        assert_eq!(
            cfg.bind,
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        );
    }

    #[test]
    fn test_graphql_bind_from_toml() {
        let toml_str = r#"
[graphql]
bind = "0.0.0.0"
port = 5000
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            cfg.graphql.bind,
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(cfg.graphql.port, 5000);
    }

    #[test]
    fn test_graphql_bind_omitted_defaults_to_localhost() {
        let toml_str = r#"
[graphql]
port = 4000
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            cfg.graphql.bind,
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        );
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

    #[test]
    fn test_parse_size_bytes() {
        assert_eq!(parse_size_bytes("50GB"), Some(50 * 1024 * 1024 * 1024));
        assert_eq!(parse_size_bytes("500MB"), Some(500 * 1024 * 1024));
        assert_eq!(parse_size_bytes("1TB"), Some(1024 * 1024 * 1024 * 1024));
        assert_eq!(parse_size_bytes("100KB"), Some(100 * 1024));
        assert_eq!(parse_size_bytes("1024B"), Some(1024));
        assert_eq!(parse_size_bytes("1024"), Some(1024));

        // Case insensitive.
        assert_eq!(parse_size_bytes("50gb"), Some(50 * 1024 * 1024 * 1024));
        assert_eq!(parse_size_bytes("50Gb"), Some(50 * 1024 * 1024 * 1024));

        // Short suffixes.
        assert_eq!(parse_size_bytes("50G"), Some(50 * 1024 * 1024 * 1024));
        assert_eq!(parse_size_bytes("500M"), Some(500 * 1024 * 1024));

        // Spaces.
        assert_eq!(parse_size_bytes("50 GB"), Some(50 * 1024 * 1024 * 1024));
        assert_eq!(parse_size_bytes(" 50GB "), Some(50 * 1024 * 1024 * 1024));

        // Decimal.
        assert_eq!(
            parse_size_bytes("1.5GB"),
            Some((1.5 * 1024.0 * 1024.0 * 1024.0) as u64)
        );

        // Invalid.
        assert_eq!(parse_size_bytes(""), None);
        assert_eq!(parse_size_bytes("abc"), None);
        assert_eq!(parse_size_bytes("50XB"), None);
    }

    #[test]
    fn test_cache_limit_config_from_toml() {
        let toml_str = r#"
[remote]
cache_limit = "50GB"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.remote.cache_limit.as_deref(), Some("50GB"));
        assert_eq!(cfg.cache_limit_bytes(), Some(50 * 1024 * 1024 * 1024));
    }

    #[test]
    fn test_cache_limit_none_by_default() {
        let cfg = Config::default();
        assert!(cfg.remote.cache_limit.is_none());
        assert!(cfg.cache_limit_bytes().is_none());
    }

    #[test]
    fn test_cache_limit_not_serialized_when_none() {
        let cfg = Config::default();
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        assert!(!serialized.contains("cache_limit"));
    }
}
