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
    pub graphql: GraphqlConfig,
    pub subsonic: SubsonicConfig,
    pub auth: AuthConfig,
    pub sharing: SharingConfig,
    pub mcp: McpConfig,
    pub push: PushConfig,
    pub devices: DevicesConfig,
    pub dsp: DspConfig,
    pub appearance: AppearanceConfig,
}

/// How the apps are drawn: Settings → Appearance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceConfig {
    /// `"koan"`, the site's look throughout (see `docs/design/koan-theme.md`),
    /// or `"system"`, the platform's own look in koan's colours. Read at launch.
    pub theme: String,
    /// In the kōan theme, draw the app's icons beside navigation, tabs and
    /// controls; off, labels alone.
    pub theme_icons: bool,
    /// Take colour from the record playing: the wash behind the window and
    /// the accent. Off, there is no wash and the accent is koan's mint, in
    /// either theme.
    pub record_colours: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: "koan".into(),
            theme_icons: true,
            record_colours: true,
        }
    }
}

/// Share links this koan serves itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SharingConfig {
    /// Where this server is reached from outside, e.g. `https://koan.example.com`.
    /// Share links are built on it; without it a server makes no links, since
    /// it cannot know which of its addresses a stranger can reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
}

/// MCP clients signing in through this server's OAuth.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    /// The only hosts an MCP client may register a redirect to, besides this
    /// machine. Empty allows any HTTPS host: each approval then rests on the
    /// user recognising the host the consent page names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirect_hosts: Vec<String>,
}

/// Push notifications to koan's iOS app, which reach a phone iOS has
/// suspended. Apple accepts them only signed with the key of the team that
/// ships the app, so only a server holding that key can send them; without one
/// a server reaches phones only while they are linked.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PushConfig {
    /// The APNs auth key (`AuthKey_XXXXXXXXXX.p8`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<PathBuf>,
    /// The key itself, PEM, for a deployment that hands secrets over as
    /// environment (`KOAN_PUSH__KEY`) rather than files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The key's ten-character id.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub key_id: String,
    /// The Apple developer team the key belongs to.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub team_id: String,
    /// The app's bundle id, which every push names.
    pub topic: String,
}

impl Default for PushConfig {
    fn default() -> Self {
        Self {
            key_path: None,
            key: None,
            key_id: String::new(),
            team_id: String::new(),
            topic: "cc.blit.koan".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryConfig {
    pub folders: Vec<PathBuf>,
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
    /// Fade out on pause and back in on resume, rather than cutting.
    pub fade_on_pause: bool,
    /// Output audio device name. None = system default.
    /// Persisted by name (not ID) since IDs can change across reboots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_device: Option<String>,
    /// The UPnP renderer last picked as the output, by UDN, and its name for
    /// showing. Gone back to at launch if it is on the network; otherwise the
    /// music plays on `output_device`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer_name: Option<String>,
    /// Look for UPnP renderers on the network. Off, none is found, so none
    /// can be played to.
    pub renderers: bool,
    /// Play silence: the output runs as usual, with every sample zeroed. For
    /// automated runs on a machine someone is using.
    pub muted: bool,
    /// Album art width in terminal columns (default: 24).
    /// Height is always width/2 (square via halfblock rendering).
    pub art_size: u16,
    /// Silence played after the output device changes sample rate, so the
    /// start of the track is not lost while the device relocks its clock.
    /// How long that takes is a property of the device.
    pub rate_switch_lead_in_ms: u32,
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
    /// An OpenSubsonic API key, signing in instead of the password. What
    /// joining with a koan invite stores. config.local.toml, like the password.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    /// The keypair this device proves itself with to the account's other
    /// devices on the local network: base64 of its Ed25519 PKCS#8. Made at
    /// each sign-in with an API key and dropped at sign-out, so the server's
    /// copy of the public key goes with the API key it is kept on. A secret,
    /// kept with the credentials. See `remote::proof`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device_key: String,
    /// Defaults to config_dir()/cache if empty.
    pub cache_dir: Option<PathBuf>,
    /// Parallel download workers for remote tracks (default: 5).
    pub download_workers: usize,
    /// Maximum cache size on disk. Human-readable: "50GB", "500MB", etc.
    /// None or empty = unlimited. Past it, whole albums are evicted, least
    /// recently used first, at startup and as downloads land.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_limit: Option<String>,
    /// Sync the library from the server on startup, and on a timer for a server
    /// that cannot say when it changes; a koan server tells its apps itself.
    ///
    /// Each run walks the library only if the server says it changed since the
    /// last walk, so it is cheap enough to run unattended.
    pub auto_sync: bool,
    /// Minutes between automatic syncs. 0 runs one at startup and no more.
    pub auto_sync_interval_mins: u64,
    /// Keep this device's queue in the account's play queue on the server,
    /// where Subsonic clients save theirs, so a queue can be picked up in
    /// another client or on another kōan device after a restart. Off by
    /// default: between kōan devices the queue already moves live over the
    /// link. Per device.
    pub play_queue: bool,
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
            replaygain: ReplayGainMode::Off,
            target_fps: 60,
            show_fps: false,
            pre_amp_db: 0.0,
            fade_on_pause: true,
            output_device: None,
            renderer: None,
            renderer_name: None,
            renderers: true,
            muted: false,
            art_size: 24,
            rate_switch_lead_in_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualizerConfig {
    pub enabled: bool,
    pub fps: u8,
    /// Visualizer mode by name: "bars" (default), or any name
    /// `VisualizerMode::parse` in koan-tui accepts.
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
            api_key: String::new(),
            device_key: String::new(),
            cache_dir: None,
            download_workers: 5,
            cache_limit: None,
            auto_sync: true,
            auto_sync_interval_mins: 60,
            play_queue: false,
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
    /// A request header an authenticating reverse proxy (Authelia, Authentik,
    /// oauth2-proxy) sets to the signed-in username, e.g. `Remote-User`. The
    /// web UI signs that account in without asking for its password. Honoured
    /// only on connections from `proxy_auth_from`; empty turns it off.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proxy_auth_header: String,
    /// The addresses or CIDR ranges the authenticating proxy connects from.
    /// The header is ignored on a connection from anywhere else.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxy_auth_from: Vec<String>,
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
            proxy_auth_header: String::new(),
            proxy_auth_from: Vec::new(),
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
    /// Shared secret, written by `koan subsonic setup`. Lives in
    /// config.local.toml, which is gitignored and `0600`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// Transcode `stream` for clients that ask for a lower bitrate or another
    /// format. Off, every client gets the original file.
    pub transcode: bool,
    /// The `ffmpeg` transcoding runs, by name on `PATH` or by path. Without
    /// one, originals are served.
    pub ffmpeg: String,
}

impl Default for SubsonicConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: None,
            username: "koan".into(),
            password: String::new(),
            transcode: true,
            ffmpeg: "ffmpeg".into(),
        }
    }
}

/// Credentials for a remote koan server this machine signs in to.
///
/// The other direction from `GraphqlConfig`, which configures the server koan
/// *is*. Written by `koan auth login` and cleared by `koan auth logout`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Server the token below belongs to. One at a time.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub server: String,
    /// Refresh token, exchanged for short-lived access tokens. Revocable at the
    /// server, which is what separates it from a password.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub refresh_token: String,
}

/// Controlling this device from others on the local network, and finding
/// them. Devices on the same account reach each other through the server
/// whatever this says.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DevicesConfig {
    /// Take part in the local network at all: listen, announce, look for other
    /// devices and dial them. Off, this device reaches others only through the
    /// server. For a shared network, and for test runs that relaunch an app
    /// over and over, which would otherwise announce it to every device in
    /// the house each time.
    pub nearby: bool,
    /// Listen on the local network and announce this device there, so any
    /// koan app on it can see what is playing and control it.
    pub discoverable: bool,
    /// The port listened on. Fixed so that an address typed into another
    /// device (over Tailscale, where nothing is announced) keeps working.
    pub port: u16,
    /// Devices to connect to by address, `host:port`: for networks that do
    /// not carry Bonjour, such as a tailnet.
    pub addresses: Vec<String>,
    /// What a kōan on the local network may have this device do, whoever is
    /// signed in there.
    pub nearby_control: NearbyControl,
    /// Keep the Mac app running in the menu bar once its window is closed, so
    /// other devices can still see and control this one. Quitting it then
    /// means this Mac is out of reach until it is opened again.
    pub keep_running: bool,
}

/// What devices on the local network may do with this one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NearbyControl {
    /// Play, the queue, the output, the preset and the volume, and moving the
    /// music here or away: what a household network wants.
    #[default]
    Full,
    /// Play and the queue only, and a hand-off that stays on the network: for
    /// a network shared with strangers.
    Playback,
}

impl Default for DevicesConfig {
    fn default() -> Self {
        Self {
            nearby: true,
            discoverable: true,
            port: DEVICES_PORT,
            addresses: Vec::new(),
            nearby_control: NearbyControl::Full,
            keep_running: false,
        }
    }
}

/// "koan" on a phone keypad.
pub const DEVICES_PORT: u16 = 5626;

/// Equalisation and convolution for the listening setup: headphones, speakers,
/// a room. Each profile names the output devices it is for, so plugging in the
/// headphones selects their correction. A device no profile names plays
/// bit-perfect, untouched.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DspConfig {
    /// Off bypasses every profile without forgetting any of them.
    pub enabled: bool,
    pub profiles: Vec<DspProfile>,
    /// Output devices whose AutoEQ suggestion was turned down. See
    /// `audio::dsp::autoeq::suggest`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub autoeq_dismissed: Vec<String>,
}

impl Default for DspConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            profiles: Vec::new(),
            autoeq_dismissed: Vec::new(),
        }
    }
}

impl DspConfig {
    /// The profile for the output device called `device`, if DSP is on and
    /// one names it.
    pub fn profile_for(&self, device: &str) -> Option<&DspProfile> {
        if !self.enabled {
            return None;
        }
        self.profiles
            .iter()
            .find(|p| p.devices.iter().any(|d| d == device))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DspProfile {
    pub name: String,
    /// Output devices, by name, that play through this profile.
    pub devices: Vec<String>,
    /// Gain before the filters. Unset, it is derived from the filters' peak
    /// gain, so that no boost can push a sample past full scale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preamp_db: Option<f64>,
    /// Run in order, at the output rate, ahead of the impulse responses.
    pub filters: Vec<DspFilter>,
    /// Impulse responses as WAV files, one per sample rate the correction was
    /// exported at; a relative path is read from beside the config. Each
    /// file's own rate is the rate it applies to.
    pub impulses: Vec<PathBuf>,
    /// The files it was imported from, by name, for showing where it came
    /// from. Nothing reads them again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source: Vec<String>,
    /// For a correction installed from AutoEQ: the target it was made for,
    /// and another to move it to. See `audio::dsp::targets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<DspTarget>,
    /// Other profiles played first, in order: a headphone's correction and
    /// then taste on top of it, each switched on or off. A profile with
    /// layers is a stack. See `audio::dsp::Setup::load`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<DspLayer>,
    /// Whether it is the account's, kept on every device signed in to its
    /// kōan server, or this device's alone. Unset, it follows from what the
    /// profile is: see `audio::dsp::profiles::scope`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<DspScope>,
    /// What every device calls it, made when it is first shared. Its name may
    /// change; this does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    /// The device that first shared it, by name: what tells two different
    /// profiles of one name apart when they meet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// The bounds a profile must keep to be played: what any real correction
/// asks, and nothing that would deafen a listener, starve the decode thread
/// or exhaust memory. `DspProfile::sanitize` brings every profile within
/// them, wherever it came from. The one place they are set.
pub mod dsp_bounds {
    use std::ops::RangeInclusive;
    pub const FREQ_HZ: RangeInclusive<f64> = 1.0..=48_000.0;
    pub const GAIN_DB: RangeInclusive<f64> = -30.0..=30.0;
    pub const Q: RangeInclusive<f64> = 0.01..=100.0;
    pub const DELAY_MS: RangeInclusive<f64> = 0.0..=2_000.0;
    /// Two seconds at 384 kHz.
    pub const DELAY_SAMPLES: RangeInclusive<f64> = 0.0..=768_000.0;
    /// A mix's linear gain: ±30 dB.
    pub const MIX_GAIN: f64 = 31.63;
    pub const CHANNELS: u16 = 64;
    /// Parametric bands applying to any one channel.
    pub const BANDS_PER_CHANNEL: usize = 64;
    pub const FILTERS: usize = 256;
    pub const GRAPHIC_POINTS: usize = 2_048;
    pub const LAYERS: usize = 32;
    pub const NAME: usize = 128;

    // What a whole chain, layers and target step included, may add up to:
    // each value within bounds can still sum past what a device can hold
    // or keep up with.

    /// Delay on any one channel, every delay summed, in ms. A delay in
    /// samples counts at 44.1 kHz, the lowest rate it could mean the most
    /// time at.
    pub const CHAIN_DELAY_MS: f64 = 2_000.0;
    /// Graphic curves on any one channel: each is a minimum-phase FIR of up
    /// to a second of taps at the output rate. Two is a target step and a
    /// curve of the user's own.
    pub const CHAIN_GRAPHICS: usize = 2;
    /// Mixes in a chain.
    pub const CHAIN_MIXES: usize = 8;
    /// Taps of an impulse response, per route: the longest a shipped room
    /// correction is known to need, Harman's 780 pack at 192 kHz, which is
    /// over five seconds at 48 kHz, longer than any room's decay.
    pub const IMPULSE_TAPS: usize = 262_145;
}

/// Bring a whole chain within the `dsp_bounds` budgets, saying what was
/// changed: delays past the total on a channel shortened or dropped,
/// graphic curves and mixes past their counts dropped.
pub fn budget(filters: &mut Vec<DspFilter>) -> Vec<String> {
    use dsp_bounds as b;
    let mut notes = Vec::new();
    let mut note = |n: String| {
        if !notes.contains(&n) {
            notes.push(n);
        }
    };
    let channels = b::CHANNELS as usize;
    let mut delay = vec![0.0f64; channels];
    let mut graphics = vec![0usize; channels];
    let mut mixes = 0;
    let on = |cs: &[u16], c: usize| cs.is_empty() || cs.contains(&(c as u16));
    filters.retain_mut(|f| match f {
        DspFilter::Delay(d) => {
            let ms = d.ms + d.samples / 44.1;
            let room = (0..channels)
                .filter(|c| on(&d.channels, *c))
                .map(|c| b::CHAIN_DELAY_MS - delay[c])
                .fold(b::CHAIN_DELAY_MS, f64::min)
                .max(0.0);
            if ms > room {
                note(format!(
                    "delays shortened to {} ms in all",
                    b::CHAIN_DELAY_MS
                ));
                if room <= 0.0 {
                    return false;
                }
                // Shortened in proportion, keeping how it was given.
                let scale = room / ms;
                d.ms *= scale;
                d.samples = (d.samples * scale).floor();
            }
            let ms = d.ms + d.samples / 44.1;
            for (c, total) in delay.iter_mut().enumerate() {
                if on(&d.channels, c) {
                    *total += ms;
                }
            }
            true
        }
        DspFilter::Graphic(g) => {
            let full =
                (0..channels).any(|c| on(&g.channels, c) && graphics[c] == b::CHAIN_GRAPHICS);
            if full {
                note(format!(
                    "graphic EQs past {} a channel dropped",
                    b::CHAIN_GRAPHICS
                ));
                return false;
            }
            for (c, n) in graphics.iter_mut().enumerate() {
                if on(&g.channels, c) {
                    *n += 1;
                }
            }
            true
        }
        DspFilter::Mix(_) => {
            mixes += 1;
            if mixes > b::CHAIN_MIXES {
                note(format!("mixes past {} dropped", b::CHAIN_MIXES));
                return false;
            }
            true
        }
        DspFilter::Band(_) => true,
    });
    notes
}

impl DspProfile {
    /// Bring the profile within `dsp_bounds`, saying what was changed: values
    /// past a bound are clamped to it, ones that are not finite are dropped
    /// with what holds them, and filters, bands, layers and points past a
    /// cap are cut off at it. Every profile is played as this leaves it,
    /// wherever it came from, so one that would deafen a listener, starve
    /// the decode thread or exhaust memory still plays, adjusted, and its
    /// page says how.
    pub fn sanitize(&mut self) -> Vec<String> {
        use dsp_bounds as b;
        let mut notes: Vec<String> = Vec::new();
        let mut note = |n: String| {
            if !notes.contains(&n) {
                notes.push(n);
            }
        };
        let mut clamp = |what: &str, v: &mut f64, r: &std::ops::RangeInclusive<f64>, unit: &str| {
            let c = v.clamp(*r.start(), *r.end());
            if c != *v {
                note(format!(
                    "{what} clamped to {}{unit}",
                    if *v > *r.end() { r.end() } else { r.start() }
                ));
                *v = c;
            }
        };
        let finite = |v: f64| v.is_finite();
        let mut dropped = Vec::new();

        let cleaned: String = self
            .name
            .chars()
            .filter(|c| !c.is_control())
            .take(b::NAME)
            .collect();
        let cleaned = if cleaned.trim().is_empty() {
            "Profile".to_owned()
        } else {
            cleaned
        };
        if cleaned != self.name {
            dropped.push("name shortened".to_owned());
            self.name = cleaned;
        }
        if let Some(db) = self.preamp_db.as_mut() {
            if finite(*db) {
                clamp("preamp", db, &b::GAIN_DB, " dB");
            } else {
                self.preamp_db = None;
                dropped.push("preamp dropped".to_owned());
            }
        }
        let bad_channel = |cs: &[u16]| cs.iter().any(|c| *c >= b::CHANNELS);
        let mut per_channel = [0usize; b::CHANNELS as usize];
        let before = self.filters.len();
        let mut kept = Vec::with_capacity(before.min(b::FILTERS));
        for mut filter in std::mem::take(&mut self.filters) {
            if kept.len() == b::FILTERS {
                dropped.push(format!("filters past {} dropped", b::FILTERS));
                break;
            }
            let keep = match &mut filter {
                DspFilter::Band(f) => {
                    if ![f.freq, f.gain_db, f.q].into_iter().all(finite) || bad_channel(&f.channels)
                    {
                        false
                    } else {
                        let on =
                            |c: usize| f.channels.is_empty() || f.channels.contains(&(c as u16));
                        if (0..b::CHANNELS as usize)
                            .any(|c| on(c) && per_channel[c] == b::BANDS_PER_CHANNEL)
                        {
                            dropped.push(format!(
                                "bands past {} a channel dropped",
                                b::BANDS_PER_CHANNEL
                            ));
                            continue;
                        } else {
                            for (c, n) in per_channel.iter_mut().enumerate() {
                                if on(c) {
                                    *n += 1;
                                }
                            }
                            clamp("a band's frequency", &mut f.freq, &b::FREQ_HZ, " Hz");
                            clamp("a band's gain", &mut f.gain_db, &b::GAIN_DB, " dB");
                            clamp("a band's Q", &mut f.q, &b::Q, "");
                            true
                        }
                    }
                }
                DspFilter::Delay(d) => {
                    if !finite(d.ms) || !finite(d.samples) || bad_channel(&d.channels) {
                        false
                    } else {
                        clamp("delay", &mut d.ms, &b::DELAY_MS, " ms");
                        clamp("delay", &mut d.samples, &b::DELAY_SAMPLES, " samples");
                        true
                    }
                }
                DspFilter::Mix(m) => {
                    m.outputs.truncate(b::CHANNELS as usize);
                    for o in &mut m.outputs {
                        o.retain(|(c, g)| *c < b::CHANNELS && finite(*g));
                        o.truncate(b::CHANNELS as usize);
                        for (_, g) in o.iter_mut() {
                            clamp("a mix's gain", g, &(-b::MIX_GAIN..=b::MIX_GAIN), "");
                        }
                    }
                    true
                }
                DspFilter::Graphic(g) => {
                    if bad_channel(&g.channels) {
                        false
                    } else {
                        g.points.retain(|(hz, db)| finite(*hz) && finite(*db));
                        if g.points.len() > b::GRAPHIC_POINTS {
                            g.points.truncate(b::GRAPHIC_POINTS);
                            dropped.push(format!(
                                "graphic EQ points past {} dropped",
                                b::GRAPHIC_POINTS
                            ));
                        }
                        for (hz, db) in &mut g.points {
                            clamp(
                                "a graphic EQ's frequency",
                                hz,
                                &(0.0..=*b::FREQ_HZ.end()),
                                " Hz",
                            );
                            clamp("a graphic EQ's gain", db, &b::GAIN_DB, " dB");
                        }
                        true
                    }
                }
            };
            if keep {
                kept.push(filter);
            } else {
                dropped.push("a filter that could not play dropped".to_owned());
            }
        }
        self.filters = kept;
        for n in budget(&mut self.filters) {
            dropped.push(n);
        }
        if self.layers.len() > b::LAYERS {
            self.layers.truncate(b::LAYERS);
            dropped.push(format!("layers past {} dropped", b::LAYERS));
        }
        self.layers.retain(|l| l.profile.chars().count() <= b::NAME);
        if self
            .origin
            .as_ref()
            .is_some_and(|o| o.chars().count() > b::NAME)
        {
            self.origin = None;
        }
        if let Some(t) = &mut self.target {
            let long = |s: &str| s.chars().count() > b::NAME;
            if long(&t.made_for) {
                self.target = None;
                dropped.push("target dropped".to_owned());
            } else if t.chosen.as_deref().is_some_and(long) {
                t.chosen = None;
                dropped.push("chosen target dropped".to_owned());
            }
        }
        for d in dropped {
            note(d);
        }
        notes
    }

    /// As played: `sanitize`d, leaving this one as it is.
    pub fn sanitized(&self) -> Self {
        let mut p = self.clone();
        p.sanitize();
        p
    }
}

/// Where a profile is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DspScope {
    /// The account's: synced through its kōan server to its other devices.
    Everywhere,
    /// This device's: never uploaded.
    Device,
}

/// One profile played as part of another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DspLayer {
    pub profile: String,
    #[serde(default = "layer_on")]
    pub on: bool,
}

fn layer_on() -> bool {
    true
}

/// The target a correction was made for, and the one chosen in its place.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DspTarget {
    /// One of the targets koan ships, by id.
    pub made_for: String,
    /// A target koan ships, or one added (`added:<name>`). Unset, or the same
    /// as `made_for`, the correction plays as it was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<String>,
}

/// One step of a profile's processing. Bands on different channels commute;
/// a mix does not, which is why the steps are an ordered list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DspFilter {
    Delay(Delay),
    Mix(Mix),
    Graphic(GraphicEq),
    #[serde(untagged)]
    Band(EqFilter),
}

impl DspFilter {
    /// The channels it applies to, from 0. Empty is every channel; a mix
    /// names its own.
    pub fn channels(&self) -> &[u16] {
        match self {
            Self::Band(f) => &f.channels,
            Self::Delay(d) => &d.channels,
            Self::Graphic(g) => &g.channels,
            Self::Mix(_) => &[],
        }
    }

    pub fn channels_mut(&mut self) -> Option<&mut Vec<u16>> {
        match self {
            Self::Band(f) => Some(&mut f.channels),
            Self::Delay(d) => Some(&mut d.channels),
            Self::Graphic(g) => Some(&mut g.channels),
            Self::Mix(_) => None,
        }
    }
}

impl From<EqFilter> for DspFilter {
    fn from(f: EqFilter) -> Self {
        Self::Band(f)
    }
}

/// A fixed delay: `ms` and `samples` added together, at the output rate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Delay {
    #[serde(skip_serializing_if = "is_zero")]
    pub ms: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub samples: f64,
    /// Keep the fraction of a sample, through an allpass, rather than round
    /// to the nearest whole one.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub subsample: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<u16>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Channels made from others. `outputs[o]` is what channel `o` becomes, as
/// `(input channel, linear gain)` pairs read before any is written; an empty
/// list is silence. Channels past the end of `outputs` pass through.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mix {
    pub outputs: Vec<Vec<(u16, f64)>>,
}

/// A curve of `(Hz, dB)` points, as Equalizer APO's `GraphicEQ:` gives one,
/// run as a minimum-phase FIR designed at the output rate. Between points the
/// gain is interpolated against log frequency; past the ends it holds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphicEq {
    pub points: Vec<(f64, f64)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<u16>,
}

/// One parametric band, as AutoEQ and Equalizer APO describe it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqFilter {
    #[serde(rename = "type")]
    pub kind: EqFilterKind,
    /// Centre or corner frequency in Hz.
    pub freq: f64,
    #[serde(default)]
    pub gain_db: f64,
    #[serde(default = "default_q")]
    pub q: f64,
    /// The channels it applies to, from 0. Empty is every channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<u16>,
}

fn default_q() -> f64 {
    std::f64::consts::FRAC_1_SQRT_2
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqFilterKind {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    Notch,
    BandPass,
    AllPass,
    /// First-order sections, 6 dB per octave. The shelves are at half their
    /// gain at `freq`; `q` is not used.
    LowShelfFirstOrder,
    HighShelfFirstOrder,
    LowPassFirstOrder,
    HighPassFirstOrder,
    AllPassFirstOrder,
    /// Flat gain, `gain_db`: a channel's own preamp.
    Gain,
}

impl EqFilterKind {
    /// As written in the config.
    pub fn name(self) -> &'static str {
        match self {
            Self::Peaking => "peaking",
            Self::LowShelf => "low_shelf",
            Self::HighShelf => "high_shelf",
            Self::LowPass => "low_pass",
            Self::HighPass => "high_pass",
            Self::Notch => "notch",
            Self::BandPass => "band_pass",
            Self::AllPass => "all_pass",
            Self::LowShelfFirstOrder => "low_shelf_first_order",
            Self::HighShelfFirstOrder => "high_shelf_first_order",
            Self::LowPassFirstOrder => "low_pass_first_order",
            Self::HighPassFirstOrder => "high_pass_first_order",
            Self::AllPassFirstOrder => "all_pass_first_order",
            Self::Gain => "gain",
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
        | "remote.api_key"
        | "remote.device_key"
        | "subsonic.password"
        | "auth.refresh_token"
        | "push.key"
        | "push.key_path"
        | "push.key_id"
        | "push.team_id"
        // This machine's paths, disk and account.
        | "library.folders"
        | "remote.enabled"
        | "remote.url"
        | "remote.username"
        | "remote.cache_dir"
        | "remote.cache_limit"
        // Whether this device follows the account's saved play queue.
        | "remote.play_queue"
        // This machine's hardware.
        | "playback.output_device"
        | "playback.renderer"
        | "playback.renderer_name"
        | "playback.renderers"
        | "playback.muted"
        | "playback.rate_switch_lead_in_ms"
        // Which machine serves Subsonic, and as whom. Enabling a REST API is a
        // decision about one host, and the secret guarding it is per-machine.
        | "subsonic.enabled"
        | "subsonic.port"
        | "subsonic.username"
        | "subsonic.transcode"
        | "subsonic.ffmpeg"
        // Whether this machine is open to its network, and where others are.
        | "devices.nearby"
        | "devices.discoverable"
        | "devices.port"
        | "devices.addresses"
        | "devices.nearby_control"
        | "devices.keep_running"
        // Which koan server this machine signs in to.
        | "auth.server"
        // Volatile: UI state behind a keybind or a mouse drag.
        | "playback.art_size"
        | "visualizer.enabled"
        | "visualizer.mode"
        | "visualizer.matrix_overlay"
        | "visualizer.bass_shake" => Layer::Machine,
        // The listening setup: these headphones, this room.
        p if p == "dsp" || p.starts_with("dsp.") => Layer::Machine,
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

    /// Resolved cache directory — the explicit setting, or `default_cache_dir`.
    pub fn cache_dir(&self) -> PathBuf {
        self.remote
            .cache_dir
            .clone()
            .unwrap_or_else(default_cache_dir)
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
    fs::write(path, &contents)?;
    #[cfg(target_os = "tvos")]
    kept::written(path, contents.as_bytes());
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
        Some(existing) => *existing = to_edit_item(value),
        None => {
            table.insert(last, to_edit_item(value));
        }
    }
}

/// A list of tables — DSP profiles — is written as `[[sections]]`, and any list
/// of tables inside one a line per entry, so the file stays editable by hand.
fn to_edit_item(value: &toml::Value) -> toml_edit::Item {
    let toml::Value::Array(items) = value else {
        return toml_edit::value(to_edit_value(value));
    };
    if items.is_empty() || !items.iter().all(toml::Value::is_table) {
        return toml_edit::value(to_edit_value(value));
    }
    let mut sections = toml_edit::ArrayOfTables::new();
    for item in items {
        let mut section = toml_edit::Table::new();
        for (k, v) in item.as_table().expect("checked above") {
            let mut v = to_edit_value(v);
            if let toml_edit::Value::Array(list) = &mut v
                && list.iter().any(toml_edit::Value::is_inline_table)
            {
                for entry in list.iter_mut() {
                    entry.decor_mut().set_prefix("\n    ");
                    if let Some(t) = entry.as_inline_table_mut() {
                        t.sort_values_by(|a, _, b, _| key_rank(a).cmp(&key_rank(b)));
                    }
                }
                list.set_trailing("\n");
                list.set_trailing_comma(true);
            }
            section.insert(k, toml_edit::value(v));
        }
        section.sort_values_by(|a, _, b, _| key_rank(a).cmp(&key_rank(b)));
        sections.push(section);
    }
    toml_edit::Item::ArrayOfTables(sections)
}

/// What an entry is reads first: a profile's name, a band's type.
fn key_rank(key: &toml_edit::Key) -> (u8, &str) {
    let k = key.get();
    (if k == "name" || k == "type" { 0 } else { 1 }, k)
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
/// `~/.config/koan/` (on iOS, `Library/Application Support/koan`) unless
/// pointed elsewhere. `KOAN_CONFIG_DIR` is the
/// user-facing way to do that — one machine, more than one library — and
/// `set_config_dir` is the in-process one, which is what tests need: without
/// it they read whatever configuration belongs to whoever ran them, right down
/// to that person's server and their password.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = CONFIG_DIR.read().clone() {
        return dir;
    }
    if let Some(dir) = std::env::var_os("KOAN_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    platform_config_dir()
}

#[cfg(not(any(target_os = "ios", target_os = "tvos")))]
fn platform_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("koan")
}

/// An iOS app may write inside its container's `Documents`, `Library` and
/// `tmp`, and nowhere else: `~/.config` is refused on a device, though the
/// simulator allows it. Application Support is where an app's own state goes.
#[cfg(target_os = "ios")]
fn platform_config_dir() -> PathBuf {
    ios_library().join("Application Support").join("koan")
}

/// A tvOS app has no persistent storage of its own: `Library/Caches` is the
/// one directory it can write, and the system empties it when space runs short.
/// The files live there, beside the download cache rather than inside it, which
/// is trimmed; `kept` holds a copy that survives a purge.
#[cfg(target_os = "tvos")]
fn platform_config_dir() -> PathBuf {
    let dir = ios_library().join("Caches").join("koan-config");
    kept::restore(&dir);
    dir
}

/// The configuration files of a tvOS app, mirrored into its preferences.
///
/// tvOS keeps an app's preferences (up to 500 KB) when it purges
/// `Library/Caches`, and a purge would otherwise sign the television out and
/// forget its settings. Every write to the platform directory is copied here,
/// and the first look at the directory puts back a file that has gone. The
/// Keychain would also survive, and is deliberately not used for credentials.
/// The library database is not kept: the next sync rebuilds it.
#[cfg(target_os = "tvos")]
mod kept {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Once;

    use core_foundation::base::TCFType;
    use core_foundation::data::{CFData, CFDataRef};
    use core_foundation::string::CFString;
    use core_foundation_sys::base::{CFGetTypeID, CFRelease};
    use core_foundation_sys::data::CFDataGetTypeID;
    use core_foundation_sys::preferences::{
        CFPreferencesAppSynchronize, CFPreferencesCopyAppValue, CFPreferencesSetAppValue,
        kCFPreferencesCurrentApplication,
    };

    const FILES: [&str; 2] = ["config.toml", "config.local.toml"];

    /// Once per process: a file that is missing comes back from the
    /// preferences, and one that is present is copied into them, which is how
    /// a configuration written before this existed starts being kept.
    pub(super) fn restore(dir: &Path) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            for name in FILES {
                let path = dir.join(name);
                match fs::read(&path) {
                    Ok(contents) => save(name, &contents),
                    Err(_) => {
                        let Some(contents) = load(name) else { continue };
                        let _ = fs::create_dir_all(dir);
                        if fs::write(&path, &contents).is_ok() {
                            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
                            log::info!("config: restored {name} after the system cleared it");
                        }
                    }
                }
            }
        });
    }

    /// Mirror a write, if it went to the platform directory rather than one a
    /// test or `KOAN_CONFIG_DIR` chose.
    pub(super) fn written(path: &Path, contents: &[u8]) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return;
        };
        if FILES.contains(&name) && path.parent() == Some(super::platform_config_dir().as_path()) {
            save(name, contents);
        }
    }

    fn save(name: &str, contents: &[u8]) {
        let key = CFString::new(name);
        let data = CFData::from_buffer(contents);
        // SAFETY: both are live CF objects for the length of the call, and the
        // preferences retain what they keep.
        unsafe {
            CFPreferencesSetAppValue(
                key.as_concrete_TypeRef(),
                data.as_CFTypeRef(),
                kCFPreferencesCurrentApplication,
            );
            CFPreferencesAppSynchronize(kCFPreferencesCurrentApplication);
        }
    }

    fn load(name: &str) -> Option<Vec<u8>> {
        let key = CFString::new(name);
        // SAFETY: a Copy function returns an owned reference or null. Anything
        // but data is released here; data is owned by the wrapper.
        unsafe {
            let value = CFPreferencesCopyAppValue(
                key.as_concrete_TypeRef(),
                kCFPreferencesCurrentApplication,
            );
            if value.is_null() {
                return None;
            }
            if CFGetTypeID(value) != CFDataGetTypeID() {
                CFRelease(value);
                return None;
            }
            Some(
                CFData::wrap_under_create_rule(value as CFDataRef)
                    .bytes()
                    .to_vec(),
            )
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "tvos"))]
fn ios_library() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library")
}

/// Where downloads are kept when the config names nowhere.
///
/// Beside the config everywhere but iOS. There it is `Library/Caches`, which
/// is not backed up to iCloud — a cache of lossless files would otherwise count
/// against someone's iCloud storage — and which iOS may clear when the device
/// is short of space, which is what a cache is for.
fn default_cache_dir() -> PathBuf {
    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    if CONFIG_DIR.read().is_none() && std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return ios_library().join("Caches").join("koan");
    }
    config_dir().join("cache")
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
/// remote server, and their password — so the same test does
/// different things on different machines, and passes on CI only because it
/// finds nothing there at all.
///
/// Process-wide rather than per-test on purpose: the threads koan spawns
/// resolve the directory when they run, which is often after the test that
/// started them has finished.
///
/// It waits for a test that holds `SWITCHING` (koan-core's `PERSIST_LOCK`)
/// to finish before switching, so that test never has the directory moved
/// out from under it mid-way.
pub fn isolate_config_for_tests() {
    let _one = SWITCHING.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("koan-test-config-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    set_config_dir(dir);
}

/// Held by a test that points config at a directory of its own for its
/// whole run, and by `isolate_config_for_tests` while it switches, so no
/// test's directory changes under it.
#[doc(hidden)]
pub static SWITCHING: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// Above this, `koan.log` is moved aside to `koan.log.1`, replacing the one
/// before, so the log never holds more than twice it.
const LOG_LIMIT: u64 = 16 * 1024 * 1024;
/// How many lines go by between looks at the file's size: the app stays open
/// for days, so checking only when the file is opened is not enough.
const LOG_CHECK_EVERY: u32 = 4096;

/// `koan.log`, kept to a size. Every logger writes through one of these.
#[derive(Default)]
pub struct LogFile {
    file: Option<fs::File>,
    lines: u32,
}

impl LogFile {
    pub fn write(&mut self, line: std::fmt::Arguments) {
        use std::io::Write as _;
        if self.file.is_none() {
            self.file = open_log();
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let _ = writeln!(file, "{line}");
        self.lines = self.lines.wrapping_add(1);
        if self.lines.is_multiple_of(LOG_CHECK_EVERY)
            && file.metadata().is_ok_and(|m| m.len() > LOG_LIMIT)
        {
            self.file = open_log();
        }
    }

    pub fn flush(&mut self) {
        if let Some(file) = self.file.as_mut() {
            let _ = std::io::Write::flush(file);
        }
    }
}

/// Open `koan.log` for appending, creating the configuration directory first
/// and moving an oversized log aside.
///
/// A logger starts before anything else has had reason to create the
/// directory, so on a first launch it would otherwise find nowhere to write.
fn open_log() -> Option<fs::File> {
    let dir = config_dir();
    fs::create_dir_all(&dir).ok()?;
    let path = dir.join("koan.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = fs::rename(&path, dir.join("koan.log.1"));
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// Refuse to start when credentials are sitting in version control, which is a
/// security incident rather than a warning anyone would act on.
///
/// Runs once per process. It reads both config files and, when a password is
/// present, forks `git ls-files` — and `load()` is reached from UI paths that
/// run per frame.
fn check_secrets_in_git() {
    static ONCE: Once = Once::new();
    ONCE.call_once(scan_for_tracked_secrets);
}

fn scan_for_tracked_secrets() {
    let sensitive_fields = ["password", "api_key", "refresh_token"];

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
            eprintln!("║     `koan remote login` writes there for you                 ║");
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
pub(crate) mod tests {
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
    }

    #[test]
    fn test_roundtrip_toml() {
        let cfg = Config::default();
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.playback.replaygain, cfg.playback.replaygain);
        assert_eq!(
            deserialized.remote.download_workers,
            cfg.remote.download_workers
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
    pub(crate) static PERSIST_LOCK: &std::sync::Mutex<()> = &super::SWITCHING;

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
    fn persist_writes_dsp_profiles_to_the_local_file_as_sections() {
        let _guard = PERSIST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (base, local) = persist_sandbox("dsp");

        let profile = DspProfile {
            name: "HD 600".into(),
            devices: vec!["Topping E30".into()],
            preamp_db: None,
            filters: vec![
                DspFilter::Band(EqFilter {
                    kind: EqFilterKind::Peaking,
                    freq: 20.0,
                    gain_db: -1.3,
                    q: 2.0,
                    channels: vec![],
                }),
                DspFilter::Band(EqFilter {
                    kind: EqFilterKind::HighShelfFirstOrder,
                    freq: 10000.0,
                    gain_db: 2.5,
                    q: 0.7,
                    channels: vec![],
                }),
                DspFilter::Delay(Delay {
                    ms: 1.5,
                    channels: vec![1],
                    ..Default::default()
                }),
                DspFilter::Mix(Mix {
                    outputs: vec![vec![(0, 0.5), (1, 0.5)], vec![]],
                }),
                DspFilter::Graphic(GraphicEq {
                    points: vec![(20.0, -1.0), (1000.0, 0.0)],
                    channels: vec![],
                }),
            ],
            impulses: vec![],
            source: vec![],
            target: None,
            layers: vec![],
            scope: None,
            uid: None,
            origin: None,
        };
        Config::persist(|cfg| cfg.dsp.profiles.push(profile.clone())).unwrap();

        assert!(!base.exists() || !fs::read_to_string(&base).unwrap().contains("dsp"));
        let machine = fs::read_to_string(&local).unwrap();
        assert!(machine.contains("[[dsp.profiles]]"), "{machine}");
        assert!(
            machine.contains("\n    { type = \"peaking\""),
            "one band per line: {machine}"
        );
        assert_eq!(Config::from_files().unwrap().dsp.profiles, vec![profile]);
        assert_eq!(
            Config::from_files()
                .unwrap()
                .dsp
                .profile_for("Topping E30")
                .map(|p| p.name.as_str()),
            Some("HD 600")
        );
    }

    /// Every bound holds, whatever a profile says: values clamped, ones
    /// that are not numbers dropped, counts cut off, each change noted.
    #[test]
    fn a_profile_is_played_within_bounds() {
        let band = |freq: f64, gain_db: f64, q: f64| {
            DspFilter::Band(EqFilter {
                kind: EqFilterKind::Peaking,
                freq,
                gain_db,
                q,
                channels: vec![],
            })
        };
        let mut p = DspProfile {
            name: "Loud\u{7}".into(),
            preamp_db: Some(60.0),
            filters: vec![
                DspFilter::Delay(Delay {
                    ms: 1e12,
                    ..Default::default()
                }),
                band(0.5, 200.0, 0.0),
                band(f64::NAN, 0.0, 1.0),
                DspFilter::Band(EqFilter {
                    kind: EqFilterKind::Peaking,
                    freq: 1000.0,
                    gain_db: 0.0,
                    q: 1.0,
                    channels: vec![99],
                }),
                DspFilter::Mix(Mix {
                    outputs: vec![vec![(0, 1e9), (1, f64::INFINITY)]],
                }),
                DspFilter::Graphic(GraphicEq {
                    points: (0..3000).map(|i| (i as f64, 1.0)).collect(),
                    channels: vec![],
                }),
            ],
            ..Default::default()
        };
        p.filters.extend((0..300).map(|_| band(1000.0, 1.0, 1.0)));
        let notes = p.sanitize();
        let has = |n: &str| notes.iter().any(|x| x == n);
        assert!(has("delay clamped to 2000 ms"), "{notes:?}");
        assert!(has("preamp clamped to 30 dB"), "{notes:?}");
        assert!(has("a band's gain clamped to 30 dB"), "{notes:?}");
        assert!(has("a band's frequency clamped to 1 Hz"), "{notes:?}");
        assert!(has("a filter that could not play dropped"), "{notes:?}");
        assert!(has("bands past 64 a channel dropped"), "{notes:?}");
        assert!(has("graphic EQ points past 2048 dropped"), "{notes:?}");
        assert_eq!(p.name, "Loud");
        assert_eq!(p.preamp_db, Some(30.0));
        assert_eq!(
            p.filters[0],
            DspFilter::Delay(Delay {
                ms: 2000.0,
                ..Default::default()
            })
        );
        assert_eq!(p.filters[1], band(1.0, 30.0, 0.01));
        let DspFilter::Mix(m) = &p.filters[2] else {
            panic!("{:?}", p.filters[2])
        };
        assert_eq!(m.outputs, vec![vec![(0, dsp_bounds::MIX_GAIN)]]);
        let bands = p
            .filters
            .iter()
            .filter(|f| matches!(f, DspFilter::Band(_)))
            .count();
        assert_eq!(bands, dsp_bounds::BANDS_PER_CHANNEL);
        assert!(p.clone().sanitize().is_empty(), "within bounds now");
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
