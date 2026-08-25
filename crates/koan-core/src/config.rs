use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Once};
use std::time::SystemTime;

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
    pub subsonic: SubsonicConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryConfig {
    pub folders: Vec<PathBuf>,
    /// Run acoustic analysis as part of every scan rather than only on
    /// `koan scan --analyze`. It roughly doubles a scan, so it is off.
    pub analyze_on_scan: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackConfig {
    pub replaygain: ReplayGainMode,
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
    /// Sync the library from the server on startup and on a timer.
    ///
    /// Incremental — it asks the server what changed rather than walking
    /// everything, so it is cheap enough to run unattended. A full sync stays a
    /// deliberate action.
    pub auto_sync: bool,
    /// Minutes between automatic syncs. 0 runs one at startup and no more.
    pub auto_sync_interval_mins: u64,
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
            analyze_on_scan: false,
        }
    }
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            replaygain: ReplayGainMode::Off,
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
    /// Visualizer mode: "bars" (default), "oscilloscope", "radial", "particles", "lissajous".
    pub mode: String,
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
    /// Reactivity multiplier (0.0..2.0, default 1.0).
    /// Scales all beat/spectrum-driven animation coefficients.
    /// 0.0 = static, 1.0 = normal, 2.0 = hypersensitive.
    pub reactivity: f32,
    /// Bass shake: camera jitter + scale pulse on bass hits.
    /// Applies to braille-rendered modes (oscilloscope, radial, wireframe, starfield, etc.).
    pub bass_shake: bool,
    /// Matrix overlay: replace all rendered characters with random matrix glyphs in green.
    /// Applies to any visualizer mode as a post-processing pass.
    pub matrix_overlay: bool,
    /// Beat-reactive background color on braille modes (starfield, wormhole, etc.).
    pub reactive_bg: bool,
}

impl Default for VisualizerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fps: 60,
            mode: "bars".into(),
            scale: "bark".into(),
            amplitude_scale: "aweight".into(),
            bar_decay_ms: 50,
            peak_decay_ms: 180,
            palette: "spectrum".into(),
            reactivity: 1.0,
            bass_shake: true,
            matrix_overlay: false,
            reactive_bg: false,
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
            auto_sync: true,
            auto_sync_interval_mins: 60,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OrganizeConfig {
    /// Named pattern preselected when an organize sheet or modal opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Named patterns — keys are names, values are format strings.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub patterns: HashMap<String, String>,
    /// Move cover art, cue sheets and logs alongside the music they belong to.
    /// On by default: a folder's artwork is part of the release, and leaving it
    /// behind turns one album into two half-albums.
    #[serde(default = "default_true")]
    pub move_ancillary: bool,
}

impl Default for OrganizeConfig {
    fn default() -> Self {
        Self {
            default: None,
            patterns: HashMap::new(),
            move_ancillary: true,
        }
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
    /// Require authentication for API access (default: true).
    /// When false, all requests are treated as admin. When true, JWT auth is enforced.
    pub auth_enabled: bool,
    /// Access token TTL (default: "15m"). Supports: "15m", "1h", "3600s".
    pub access_token_ttl: String,
    /// Refresh token TTL (default: "30d"). Supports: "30d", "7d", "720h".
    pub refresh_token_ttl: String,
    /// Allowed CORS origins. Empty = no cross-origin browser access at all.
    /// Example: ["https://music.example.com"]
    pub cors_origins: Vec<String>,
    /// Extra `Host:` values the server will answer to, beyond `localhost` and
    /// bare IP literals. Requests carrying any other Host are refused, which is
    /// what stops a DNS-rebinding page from reaching the API as same-origin.
    pub allowed_hosts: Vec<String>,
    /// Mark the session cookie `Secure`. Only set this when clients reach koan
    /// over HTTPS — browsers silently discard `Secure` cookies sent over plain
    /// `http://` to anything but localhost.
    pub cookie_secure: bool,
    /// Expose the `organize*` mutations, which physically move files on disk.
    pub allow_organize: bool,
}

fn default_true() -> bool {
    true
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
            auth_enabled: true,
            access_token_ttl: "15m".into(),
            refresh_token_ttl: "30d".into(),
            cors_origins: Vec::new(),
            allowed_hosts: Vec::new(),
            cookie_secure: false,
            allow_organize: false,
        }
    }
}

/// Subsonic-compatible REST API.
///
/// Credentials are deliberately separate from `[remote]`: the Subsonic protocol
/// authenticates with `md5(password + salt)` over whatever transport the client
/// picked, so the secret has to be recoverable and is exposed to anyone who can
/// capture a request. Reusing the upstream Navidrome password would hand out
/// that account too.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubsonicConfig {
    /// Serve `/rest/*`. Off unless explicitly enabled.
    ///
    /// The routes are mounted on the GraphQL port. `port` adds a second
    /// listener for clients that expect Subsonic on one of its own.
    pub enabled: bool,
    /// Serve `/rest/*` on a dedicated port as well as the GraphQL one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Username Subsonic clients authenticate as.
    pub username: String,
    /// Shared secret. Prefer the OS keychain (`koan subsonic setup`); this field
    /// is the fallback for machines without one, and lives in config.local.toml.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
}

impl Default for SubsonicConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: None,
            username: "koan".into(),
            password: String::new(),
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
            history_window: 200,
            seed_window: 5,
            discovery_weight: 0.3,
        }
    }
}

/// Which of the two files a setting is written to when koan changes it itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// `config.toml` — taste, meaningful on any machine, safe in a dotfiles repo.
    Shared,
    /// `config.local.toml` — belongs to this machine and nowhere else.
    Machine,
}

/// Where the setting at a dotted path belongs.
///
/// `config.toml` is meant to be committed to a dotfiles repo, so three kinds of
/// setting have no business in it: secrets, anything naming this machine's
/// hardware, paths or account, and UI state that a keypress flips — the last
/// kind would rewrite the shared file every session and land on the next
/// machine as someone else's window size.
///
/// Anything not named here is taste, and taste travels.
pub fn layer_of(path: &str) -> Layer {
    match path {
        // Secrets.
        "remote.password"
        | "subsonic.password"
        // This machine's paths, disk and account.
        | "library.folders"
        | "remote.enabled"
        | "remote.url"
        | "remote.username"
        | "remote.cache_dir"
        | "remote.cache_limit"
        // This machine's hardware.
        | "playback.output_device"
        // Which machine serves Subsonic, and as whom. Enabling a REST API is a
        // decision about one host, and the secret guarding it is per-machine.
        | "subsonic.enabled"
        | "subsonic.port"
        | "subsonic.username"
        // Volatile: UI state behind a keybind or a mouse drag.
        | "playback.art_size"
        | "visualizer.enabled"
        | "visualizer.mode"
        | "visualizer.matrix_overlay"
        | "visualizer.bass_shake" => Layer::Machine,
        _ => Layer::Shared,
    }
}

/// Mtimes of the two files `figment()` layers. Keyed on these so a config
/// edited by hand is picked up without koan being told about it; `KOAN_*` env
/// vars are not tracked, since they are fixed for the life of the process.
type ConfigStamp = (Option<SystemTime>, Option<SystemTime>);

type CachedConfig = Option<(ConfigStamp, Arc<Config>)>;

static CONFIG_CACHE: LazyLock<parking_lot::RwLock<CachedConfig>> =
    LazyLock::new(|| parking_lot::RwLock::new(None));

fn config_stamp() -> ConfigStamp {
    stamp_of(&config_file_path(), &config_local_file_path())
}

/// A file that does not exist stamps as `None`, so creating one is a change.
fn stamp_of(base: &Path, local: &Path) -> ConfigStamp {
    let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    (mtime(base), mtime(local))
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
        let cfg: Self = Self::figment()
            .extract()
            .map_err(|e| ConfigError::Figment(Box::new(e)))?;

        // Security: refuse to start if config files containing secrets are
        // tracked by git.
        check_secrets_in_git();

        Ok(cfg)
    }

    /// Load config, logging and falling back to defaults on error.
    ///
    /// Served from the cache, so what this costs is a clone of the struct
    /// rather than two file reads and a figment merge.
    pub fn load_or_default() -> Self {
        (*Self::cached()).clone()
    }

    /// The merged config, reloaded only when it changed on disk.
    ///
    /// `load()` re-reads both TOML files and re-runs the whole figment merge,
    /// and koan reaches it from paths that run per frame — `library_folders()`
    /// is read from a SwiftUI list body. Callers that want to avoid even the
    /// clone `load_or_default()` does can hold this `Arc`.
    pub fn cached() -> Arc<Config> {
        let stamp = config_stamp();
        if let Some((seen, cfg)) = CONFIG_CACHE.read().as_ref()
            && *seen == stamp
        {
            return cfg.clone();
        }

        let cfg = Arc::new(Self::load().unwrap_or_else(|e| {
            log::warn!("failed to load config, using defaults: {}", e);
            Self::default()
        }));
        *CONFIG_CACHE.write() = Some((stamp, cfg.clone()));
        cfg
    }

    /// Drop the cached config. koan's own writes invalidate explicitly rather
    /// than relying on the mtime, which can land in the same filesystem tick as
    /// the read before it.
    pub fn invalidate_cache() {
        *CONFIG_CACHE.write() = None;
    }

    /// Load from a specific TOML file (no env var overlay).
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    /// The two files merged, without the env layer.
    ///
    /// `persist` diffs against this rather than against `config.toml` alone, so
    /// a mutation setting a value the user already has writes nothing at all.
    /// `KOAN_*` is left out because there is no file to write it back to.
    fn from_files() -> Result<Self, ConfigError> {
        Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(config_file_path()))
            .merge(Toml::file(config_local_file_path()))
            .extract()
            .map_err(|e| ConfigError::Figment(Box::new(e)))
    }

    /// Apply a mutation and write each changed setting to the file that owns it.
    ///
    /// Only what the closure actually changed is written, so comments, layout
    /// and every untouched key survive — including the commented-out defaults
    /// `koan config init` leaves as a reference. `layer_of` decides the file.
    ///
    /// A shared write also clears any copy of that key from
    /// `config.local.toml`: the local layer wins, so leaving one there would
    /// make the write silently do nothing. A machine write clears the key from
    /// `config.toml` for the same reason in reverse, which drains settings that
    /// older versions of koan wrongly wrote to the shared file.
    pub fn persist<F>(mutate: F) -> Result<(), ConfigError>
    where
        F: FnOnce(&mut Config),
    {
        let before = Self::from_files()?;
        let mut after = before.clone();
        mutate(&mut after);

        let mut changes = Vec::new();
        diff_into(
            "",
            &toml::Value::try_from(&before)?,
            &toml::Value::try_from(&after)?,
            &mut changes,
        );
        if changes.is_empty() {
            return Ok(());
        }

        let base_path = config_file_path();
        let local_path = config_local_file_path();
        let mut base = read_document(&base_path)?;
        let mut local = read_document(&local_path)?;

        for (path, value) in &changes {
            let (target, other) = match layer_of(path) {
                Layer::Shared => (&mut base, &mut local),
                Layer::Machine => (&mut local, &mut base),
            };
            match value {
                Some(v) => doc_set(target, path, v),
                None => doc_remove(target, path),
            }
            doc_remove(other, path);
        }

        write_document(&base_path, &base, false)?;
        write_document(&local_path, &local, true)?;
        Self::invalidate_cache();
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

/// Collect the leaf settings that differ between two serialized configs, as
/// `(dotted path, new value)`. `None` means the key is gone and should be
/// removed rather than written — which is how an emptied password or a cleared
/// output device reaches the file as an absent key rather than a blank one.
fn diff_into(
    prefix: &str,
    before: &toml::Value,
    after: &toml::Value,
    out: &mut Vec<(String, Option<toml::Value>)>,
) {
    let (b, a) = match (before.as_table(), after.as_table()) {
        (Some(b), Some(a)) => (b, a),
        _ => {
            if before != after {
                out.push((prefix.to_string(), Some(after.clone())));
            }
            return;
        }
    };

    let empty = toml::Value::Table(toml::map::Map::new());
    for key in b
        .keys()
        .chain(a.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match (b.get(key), a.get(key)) {
            (Some(bv), Some(av)) => diff_into(&path, bv, av, out),
            // Newly present: recurse into tables so a new pattern writes one
            // key rather than replacing the whole table.
            (None, Some(av)) => diff_into(&path, &empty, av, out),
            (Some(_), None) => out.push((path, None)),
            (None, None) => unreachable!("key came from one of the two tables"),
        }
    }
}

fn read_document(path: &Path) -> Result<toml_edit::DocumentMut, ConfigError> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Ok(toml_edit::DocumentMut::new());
    };
    contents
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| ConfigError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
}

/// Write a document, skipping files that would be created empty. `secret` marks
/// the file 0o600 — it is the one that holds passwords.
fn write_document(
    path: &Path,
    doc: &toml_edit::DocumentMut,
    secret: bool,
) -> Result<(), ConfigError> {
    let contents = doc.to_string();
    if contents.trim().is_empty() && !path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = secret;
    Ok(())
}

fn implicit_table() -> toml_edit::Item {
    let mut table = toml_edit::Table::new();
    // Implicit: the header prints only if the table ends up holding something,
    // so writing a nested key never leaves a bare `[organize]` behind.
    table.set_implicit(true);
    toml_edit::Item::Table(table)
}

fn doc_set(doc: &mut toml_edit::DocumentMut, path: &str, value: &toml::Value) {
    let segments: Vec<&str> = path.split('.').collect();
    let (last, parents) = segments.split_last().expect("a diffed path is never empty");

    let mut table = doc.as_table_mut();
    for segment in parents {
        let item = table.entry(segment).or_insert_with(implicit_table);
        // A scalar sitting where a section belongs is malformed either way;
        // the setting koan is writing wins.
        if !item.is_table() {
            *item = implicit_table();
        }
        table = item.as_table_mut().expect("just ensured it is a table");
    }
    // Comments attach to the key, and `insert` replaces the key. Overwrite the
    // value in place where one already exists so the line keeps its notes.
    match table.get_mut(last) {
        Some(existing) => *existing = toml_edit::value(to_edit_value(value)),
        None => {
            table.insert(last, toml_edit::value(to_edit_value(value)));
        }
    }
}

fn doc_remove(doc: &mut toml_edit::DocumentMut, path: &str) {
    let segments: Vec<&str> = path.split('.').collect();
    let (last, parents) = segments.split_last().expect("a diffed path is never empty");

    let mut table = doc.as_table_mut();
    for segment in parents {
        match table.get_mut(segment).and_then(|i| i.as_table_mut()) {
            Some(child) => table = child,
            None => return,
        }
    }
    // An emptied table keeps its header: it still carries the commented-out
    // defaults `koan config init` wrote, and those are the reference.
    table.remove(last);
}

fn to_edit_value(value: &toml::Value) -> toml_edit::Value {
    match value {
        toml::Value::String(s) => s.as_str().into(),
        toml::Value::Integer(i) => (*i).into(),
        toml::Value::Float(f) => (*f).into(),
        toml::Value::Boolean(b) => (*b).into(),
        toml::Value::Datetime(d) => d.to_string().into(),
        toml::Value::Array(items) => items
            .iter()
            .map(to_edit_value)
            .collect::<toml_edit::Array>()
            .into(),
        toml::Value::Table(t) => {
            let mut inline = toml_edit::InlineTable::new();
            for (k, v) in t {
                inline.insert(k, to_edit_value(v));
            }
            inline.into()
        }
    }
}

/// Where koan keeps its configuration, library database and cache.
///
/// `~/.config/koan/` unless pointed elsewhere. `KOAN_CONFIG_DIR` is the
/// user-facing way to do that — one machine, more than one library — and
/// `set_config_dir` is the in-process one, which is what tests need: without
/// it they read whatever configuration belongs to whoever ran them, right down
/// to that person's server and their keychain.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = CONFIG_DIR.read().clone() {
        return dir;
    }
    if let Some(dir) = std::env::var_os("KOAN_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("koan")
}

/// Point koan's configuration at `dir` for the life of the process.
///
/// Takes precedence over `KOAN_CONFIG_DIR`, and drops the cached config, which
/// was keyed on the mtimes of files in a directory that is no longer the one
/// being read. Set it before anything spawns: background threads resolve the
/// directory when they run, not when they are created.
pub fn set_config_dir(dir: impl Into<PathBuf>) {
    *CONFIG_DIR.write() = Some(dir.into());
    Config::invalidate_cache();
}

/// Point configuration at a directory belonging to this process alone.
///
/// Tests call this before anything reads configuration. Without it they read
/// whatever belongs to whoever ran them — that person's library folders, their
/// remote server, and a prompt for their keychain — so the same test does
/// different things on different machines, and passes on CI only because it
/// finds nothing there at all.
///
/// Process-wide rather than per-test on purpose: the threads koan spawns
/// resolve the directory when they run, which is often after the test that
/// started them has finished.
pub fn isolate_config_for_tests() {
    let dir = std::env::temp_dir().join(format!("koan-test-config-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    set_config_dir(dir);
}

static CONFIG_DIR: LazyLock<parking_lot::RwLock<Option<PathBuf>>> =
    LazyLock::new(|| parking_lot::RwLock::new(None));

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

/// Refuse to start when credentials are sitting in version control, which is a
/// security incident rather than a warning anyone would act on.
///
/// Runs once per process. It reads both config files and, when a password is
/// present, forks `git ls-files` — and `load()` is reached from UI paths that
/// run per frame. The name says what it is: a gate on starting, not a check
/// that belongs on every read.
fn check_secrets_in_git() {
    static ONCE: Once = Once::new();
    ONCE.call_once(scan_for_tracked_secrets);
}

fn scan_for_tracked_secrets() {
    let sensitive_fields = ["password"];

    for (label, path) in [
        ("config.toml", config_file_path()),
        ("config.local.toml", config_local_file_path()),
    ] {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };

        // Check if this file contains any sensitive fields with non-empty values.
        let has_secrets = sensitive_fields.iter().any(|field| {
            contents.lines().any(|line| {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix(field) {
                    let rest = rest.trim_start();
                    if let Some(value) = rest.strip_prefix('=') {
                        let value = value.trim().trim_matches('"').trim_matches('\'');
                        return !value.is_empty();
                    }
                }
                false
            })
        });

        if !has_secrets {
            continue;
        }

        // Check if this file is tracked by git.
        if is_tracked_by_git(&path) {
            eprintln!();
            eprintln!("╔══════════════════════════════════════════════════════════════╗");
            eprintln!("║  SECURITY: {label} contains credentials and is tracked by git!  ║");
            eprintln!("╠══════════════════════════════════════════════════════════════╣");
            eprintln!("║                                                              ║");
            eprintln!("║  File: {:<52} ║", path.display());
            eprintln!("║                                                              ║");
            eprintln!("║  Your password is in version control. You should:            ║");
            eprintln!("║  1. Remove the file from git: git rm --cached <file>         ║");
            eprintln!("║  2. Add it to .gitignore                                     ║");
            eprintln!("║  3. Rotate your credentials immediately                      ║");
            eprintln!("║  4. Move secrets to config.local.toml (gitignored)           ║");
            eprintln!("║     or use `koan remote login` for keyring storage           ║");
            eprintln!("║                                                              ║");
            eprintln!("╚══════════════════════════════════════════════════════════════╝");
            eprintln!();
            panic!("Refusing to start: credentials tracked by git in {label}. See above.");
        }
    }
}

/// Check if a file is tracked by git (staged or committed, not just in a repo).
fn is_tracked_by_git(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    // `git ls-files --error-unmatch <file>` exits 0 if tracked, 1 if not.
    std::process::Command::new("git")
        .args(["ls-files", "--error-unmatch"])
        .arg(path)
        .current_dir(parent)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
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
        fs::write(&path, "[playback]\ntarget_fps = 30\n").unwrap();

        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.playback.target_fps, 30);
        assert_eq!(cfg.playback.replaygain, ReplayGainMode::Off);
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

    #[test]
    fn player_uses_config_on_init() {
        // Verify that Config::load_from correctly picks up playback settings
        // that Player::new() would consume. This tests the contract between
        // config and player initialization without requiring audio hardware.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[playback]
replaygain = "track"
output_device = "My Fancy DAC"
pre_amp_db = -3.5
target_fps = 30
art_size = 32

[visualizer]
enabled = false
mode = "oscilloscope"
fps = 30
"#,
        )
        .unwrap();

        let cfg = Config::load_from(&path).unwrap();

        // These are the fields Player::new() reads from config.
        assert_eq!(
            cfg.playback.replaygain,
            ReplayGainMode::Track,
            "replaygain should be 'track'"
        );
        assert_eq!(
            cfg.playback.output_device.as_deref(),
            Some("My Fancy DAC"),
            "output_device should match config"
        );
        assert!(
            (cfg.playback.pre_amp_db - (-3.5)).abs() < f64::EPSILON,
            "pre_amp_db should be -3.5"
        );
        assert_eq!(cfg.playback.target_fps, 30, "target_fps should be 30");
        assert_eq!(cfg.playback.art_size, 32, "art_size should be 32");

        // Visualizer config is also consumed at player init.
        assert!(!cfg.visualizer.enabled, "visualizer should be disabled");
        assert_eq!(cfg.visualizer.mode, "oscilloscope");
        assert_eq!(cfg.visualizer.fps, 30);
    }

    #[test]
    fn a_missing_config_file_stamps_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("config.toml");
        let local = dir.path().join("config.local.toml");

        assert_eq!(stamp_of(&base, &local), (None, None));

        fs::write(&base, "[remote]\nurl = \"https://example.com\"\n").unwrap();
        let (base_stamp, local_stamp) = stamp_of(&base, &local);
        assert!(base_stamp.is_some(), "creating the file must be a change");
        assert!(local_stamp.is_none());
    }

    #[test]
    fn editing_a_config_file_changes_its_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("config.toml");
        let local = dir.path().join("config.local.toml");
        fs::write(&base, "[playback]\ntarget_fps = 60\n").unwrap();

        let before = stamp_of(&base, &local);
        // Coarse-grained filesystems would otherwise stamp both writes alike.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&base, "[playback]\ntarget_fps = 30\n").unwrap();

        assert_ne!(
            before,
            stamp_of(&base, &local),
            "a config edited by hand has to be picked up"
        );
    }

    #[test]
    fn invalidating_forces_a_reload() {
        let first = Config::cached();
        Config::invalidate_cache();
        assert!(
            !Arc::ptr_eq(&first, &Config::cached()),
            "koan's own writes invalidate explicitly; the next read must re-parse"
        );
    }

    // ---- persist: which file a setting lands in -------------------------

    /// `persist` reads and writes process-global paths, so these run one at a
    /// time rather than racing each other through `set_config_dir`.
    static PERSIST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Point config at a fresh directory and hand back (base, local) paths.
    fn persist_sandbox(name: &str) -> (PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("koan-persist-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        set_config_dir(&dir);
        (config_file_path(), config_local_file_path())
    }

    #[test]
    fn persist_keeps_comments_and_untouched_keys() {
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (base, _local) = persist_sandbox("comments");
        fs::write(
            &base,
            "# koan — shareable defaults\n\n[visualizer]\n# fps = 60\npalette = \"fire\"\n",
        )
        .unwrap();

        Config::persist(|cfg| cfg.visualizer.palette = "neon".into()).unwrap();

        let written = fs::read_to_string(&base).unwrap();
        assert!(
            written.contains("# koan — shareable defaults"),
            "the header comment must survive a write: {written}"
        );
        assert!(
            written.contains("# fps = 60"),
            "commented-out defaults are the template's whole point: {written}"
        );
        assert!(written.contains("palette = \"neon\""));
        assert!(
            !written.contains("[graphql]"),
            "an untouched section must not be invented: {written}"
        );
    }

    #[test]
    fn persist_routes_machine_settings_to_the_local_file() {
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (base, local) = persist_sandbox("routing");

        Config::persist(|cfg| {
            cfg.playback.replaygain = ReplayGainMode::Album;
            cfg.playback.output_device = Some("My DAC".into());
            cfg.playback.art_size = 40;
            cfg.visualizer.mode = "starfield".into();
        })
        .unwrap();

        let shared = fs::read_to_string(&base).unwrap();
        let machine = fs::read_to_string(&local).unwrap();

        assert!(shared.contains("replaygain = \"album\""), "{shared}");
        for machine_only in ["output_device", "art_size", "starfield"] {
            assert!(
                !shared.contains(machine_only),
                "{machine_only} is this machine's, not the dotfiles repo's: {shared}"
            );
            assert!(machine.contains(machine_only), "{machine}");
        }
    }

    #[test]
    fn persist_never_writes_the_default_library_folder_into_the_shared_file() {
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (base, _local) = persist_sandbox("folders");

        // Toggling the visualiser used to serialise the whole struct, baking
        // the *default* music directory into the file people commit.
        Config::persist(|cfg| cfg.visualizer.enabled = false).unwrap();

        let shared = fs::read_to_string(&base).unwrap_or_default();
        assert!(
            !shared.contains("folders"),
            "a visualiser toggle must not invent library folders: {shared}"
        );
    }

    #[test]
    fn persist_drains_machine_settings_an_older_koan_left_in_the_shared_file() {
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (base, local) = persist_sandbox("drain");
        fs::write(&base, "[playback]\nart_size = 24\ntarget_fps = 60\n").unwrap();

        Config::persist(|cfg| cfg.playback.art_size = 48).unwrap();

        let shared = fs::read_to_string(&base).unwrap();
        assert!(
            !shared.contains("art_size"),
            "the stale shared copy has to go, or dotfiles keep carrying it: {shared}"
        );
        assert!(shared.contains("target_fps"), "{shared}");
        assert!(
            fs::read_to_string(&local)
                .unwrap()
                .contains("art_size = 48")
        );
    }

    #[test]
    fn persist_clears_the_local_copy_so_a_shared_write_takes_effect() {
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_base, local) = persist_sandbox("shadow");
        fs::write(&local, "[playback]\ntarget_fps = 30\n").unwrap();

        Config::persist(|cfg| cfg.playback.target_fps = 120).unwrap();

        assert_eq!(
            Config::from_files().unwrap().playback.target_fps,
            120,
            "local wins the merge, so a shared write over a local copy would \
             otherwise be silently ignored: {}",
            fs::read_to_string(&local).unwrap()
        );
    }

    #[test]
    fn persist_writes_nothing_when_the_mutation_changes_nothing() {
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (base, _local) = persist_sandbox("noop");
        fs::write(&base, "# untouched\n[playback]\ntarget_fps = 60\n").unwrap();

        Config::persist(|cfg| cfg.playback.target_fps = 60).unwrap();

        assert_eq!(
            fs::read_to_string(&base).unwrap(),
            "# untouched\n[playback]\ntarget_fps = 60\n"
        );
    }

    #[test]
    fn persist_keeps_passwords_out_of_the_shared_file() {
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (base, local) = persist_sandbox("secrets");

        Config::persist(|cfg| {
            cfg.remote.password = "hunter2".into();
            cfg.subsonic.password = "s3cret".into();
            cfg.visualizer.palette = "mono".into();
        })
        .unwrap();

        let shared = fs::read_to_string(&base).unwrap();
        assert!(!shared.contains("hunter2"), "{shared}");
        assert!(!shared.contains("s3cret"), "{shared}");
        assert!(shared.contains("mono"));

        let machine = fs::read_to_string(&local).unwrap();
        assert!(machine.contains("hunter2") && machine.contains("s3cret"));
    }

    #[test]
    fn persist_removes_a_cleared_password_rather_than_blanking_it() {
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_base, local) = persist_sandbox("clear-secret");
        fs::write(
            &local,
            "[remote]\nurl = \"https://a.example\"\npassword = \"old\"\n",
        )
        .unwrap();

        Config::persist(|cfg| cfg.remote.password = String::new()).unwrap();

        let machine = fs::read_to_string(&local).unwrap();
        assert!(
            !machine.contains("password"),
            "an emptied secret should leave no key behind: {machine}"
        );
        assert!(machine.contains("url"), "{machine}");
    }

    #[test]
    fn persist_adds_one_organize_pattern_without_disturbing_the_others() {
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (base, _local) = persist_sandbox("patterns");
        fs::write(
            &base,
            "[organize.patterns]\nflat = \"%artist% - %title%\"\n",
        )
        .unwrap();

        Config::persist(|cfg| {
            cfg.organize
                .patterns
                .insert("standard".into(), "%album artist%/%album%".into());
        })
        .unwrap();

        let cfg = Config::from_files().unwrap();
        assert_eq!(cfg.organize.patterns["flat"], "%artist% - %title%");
        assert_eq!(cfg.organize.patterns["standard"], "%album artist%/%album%");
    }
}
