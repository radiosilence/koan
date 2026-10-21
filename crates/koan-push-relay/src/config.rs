//! The relay's configuration: a TOML file named by `KOAN_RELAY_CONFIG`, under
//! `KOAN_RELAY__*` environment variables (`__` after the prefix and between
//! levels), which is how a deployment hands over the secrets. The double
//! underscore keeps variables Kubernetes injects for a Service named
//! `koan-relay` (`KOAN_RELAY_SERVICE_HOST`) out of the configuration.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;

/// No `Debug`: it holds secrets.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Where servers and apps reach the relay.
    pub listen: SocketAddr,
    /// Prometheus metrics and a health check, for the cluster only. Off when
    /// absent.
    pub metrics_listen: Option<SocketAddr>,
    /// The header a trusted proxy in front of the relay puts the caller's
    /// address in (`x-real-ip`, `x-forwarded-for`). Absent, the connection's
    /// own address is the caller's. Set it only behind a proxy that writes
    /// the header: anyone can send it. Behind Traefik, `x-real-ip`, which it
    /// sets to the caller alone.
    pub client_ip_header: Option<String>,
    /// Which address in that header, counting back from the end, is the
    /// caller: 1 for the last, which is the caller in a header that holds
    /// only the caller, or one proxy appending what it saw to
    /// `x-forwarded-for`; 2 behind two proxies that each append.
    pub client_ip_hops: usize,
    /// The revocation table's SQLite file.
    pub store: PathBuf,
    /// The app's bundle id, which every push names.
    pub topic: String,
    pub apns: ApnsConfig,
    pub handles: HandlesConfig,
    /// Keys the hash that stands for a device token in quotas and the
    /// revocation table, base64, 32 bytes. Never rotated: revocations are
    /// found by it.
    pub token_key: String,
    pub limits: Limits,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 8080)),
            metrics_listen: None,
            client_ip_header: None,
            client_ip_hops: 1,
            store: PathBuf::from("relay.db"),
            topic: "cc.blit.koan".into(),
            apns: ApnsConfig::default(),
            handles: HandlesConfig::default(),
            token_key: String::new(),
            limits: Limits::default(),
        }
    }
}

/// No `Debug`: it holds secrets.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ApnsConfig {
    /// The developer team the keys belong to.
    pub team_id: String,
    /// A topic-specific key for the production gateway, which release and
    /// TestFlight builds use.
    pub production: Option<ApnsKey>,
    /// A topic-specific key for the sandbox gateway, which development builds
    /// use. Without it, sandbox tokens are refused at registration.
    pub sandbox: Option<ApnsKey>,
    /// HTTP/2 connections kept to each gateway. Each carries as many streams
    /// as Apple allows at once, a thousand or so.
    pub connections: usize,
    /// Pushes waiting on Apple at once, across both gateways. Beyond this a
    /// push is refused with 503 rather than queued.
    pub max_in_flight: usize,
    /// The gateways' addresses, replaced only by tests.
    pub production_url: String,
    pub sandbox_url: String,
}

impl Default for ApnsConfig {
    fn default() -> Self {
        Self {
            team_id: String::new(),
            production: None,
            sandbox: None,
            connections: 4,
            max_in_flight: 4096,
            production_url: "https://api.push.apple.com".into(),
            sandbox_url: "https://api.sandbox.push.apple.com".into(),
        }
    }
}

/// No `Debug`: it holds the key.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApnsKey {
    /// The key's ten-character id.
    pub key_id: String,
    /// The `.p8` key, PEM. Or `key_path`.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub key_path: Option<PathBuf>,
}

impl ApnsKey {
    pub fn pem(&self) -> Result<String, String> {
        match (&self.key, &self.key_path) {
            (Some(pem), _) if !pem.trim().is_empty() => Ok(pem.clone()),
            (_, Some(path)) => std::fs::read_to_string(path)
                .map_err(|e| format!("reading {}: {e}", path.display())),
            _ => Err(format!("key {} has neither key nor key_path", self.key_id)),
        }
    }
}

/// No `Debug`: it holds secrets.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HandlesConfig {
    /// Sealing keys by id (`"1"` to `"255"`), base64, 32 bytes each. Every
    /// key here opens handles; only `current` seals new ones. Retire a key
    /// once `lifetime_days` have passed since it stopped being current.
    pub keys: BTreeMap<String, String>,
    pub current: u8,
    /// How long a handle opens, at most 365. The app renews at half this.
    pub lifetime_days: u64,
}

impl Default for HandlesConfig {
    fn default() -> Self {
        Self {
            keys: BTreeMap::new(),
            current: 1,
            lifetime_days: 60,
        }
    }
}

/// What one caller may ask of the relay. Counted per relay process: with
/// several replicas, each allows this much.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    /// Notifications sent through one handle an hour. The app gives each
    /// server a handle of its own, so this is one server's allowance for one
    /// device, and a server that spends it spends no other server's. Kept
    /// low: a notification is something a person asked for, a few times an
    /// hour at most.
    pub handle_alerts_per_hour: u32,
    /// Background wakes through one handle an hour.
    pub handle_wakes_per_hour: u32,
    /// Notifications one device can be sent an hour, from every server
    /// together.
    pub device_alerts_per_hour: u32,
    /// Background wakes one device can be sent an hour, the proofs of its
    /// registrations included. iOS throttles these itself, to two or three an
    /// hour; this bounds what reaches Apple.
    pub device_wakes_per_hour: u32,
    /// Revocations one device can make an hour. Counted apart from its
    /// wakes, so a server spending those cannot stop the device revoking.
    pub device_revocations_per_hour: u32,
    /// Pushes from one address a minute. A server with many listeners sends
    /// from one address, so this is generous.
    pub ip_pushes_per_minute: u32,
    /// Registrations from one address a minute. Each sends Apple a push to
    /// prove the token. Phones on a mobile network share an address, but a
    /// phone registers once per server every month or so.
    pub ip_registrations_per_minute: u32,
    /// Revocations from one address a minute.
    pub ip_revocations_per_minute: u32,
    /// Pushes sent to Apple a second, from everyone.
    pub global_pushes_per_second: u32,
    /// Proof pushes, for registrations and revocations, sent to Apple a
    /// second from everyone. Each may be for a token Apple has never issued.
    pub proofs_per_second: u32,
    /// Tokens Apple refuses as unknown an hour, from proofs, from one
    /// address (IPv6 by /48), before that address may prove no more until
    /// the count falls back.
    pub ip_bad_tokens_per_hour: u32,
    /// The same count across everyone, past which no registration is proved
    /// until it falls back: a backstop for made-up tokens from many
    /// addresses, at the price of registrations waiting while it lasts.
    /// Revocation asks nothing of Apple, so it is never held.
    pub bad_tokens_per_hour: u32,
    /// Callers each limiter keeps a count for. Past this, a caller it has no
    /// count for is refused until the idle ones are swept.
    pub max_tracked: usize,
    /// Devices the revocation table holds. Past this, revocations are refused.
    pub max_revocations: usize,
    /// Requests being handled at once, beyond which new ones are shed.
    pub max_requests: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            handle_alerts_per_hour: 6,
            handle_wakes_per_hour: 12,
            device_alerts_per_hour: 20,
            device_wakes_per_hour: 30,
            device_revocations_per_hour: 10,
            ip_pushes_per_minute: 6000,
            ip_registrations_per_minute: 120,
            ip_revocations_per_minute: 60,
            global_pushes_per_second: 20_000,
            proofs_per_second: 20,
            ip_bad_tokens_per_hour: 10,
            bad_tokens_per_hour: 360,
            max_tracked: 1_000_000,
            max_revocations: 1_000_000,
            max_requests: 50_000,
        }
    }
}

impl Config {
    /// The configuration file named by `KOAN_RELAY_CONFIG`, if any, under the
    /// environment.
    pub fn load() -> Result<Self, String> {
        let mut figment = Figment::new();
        if let Ok(path) = std::env::var("KOAN_RELAY_CONFIG") {
            figment = figment.merge(Toml::file_exact(path));
        }
        figment
            .merge(Env::prefixed("KOAN_RELAY__").split("__"))
            .extract()
            .map_err(|e| e.to_string())
    }
}

/// A base64 secret of exactly 32 bytes.
pub fn secret(name: &str, value: &str) -> Result<[u8; 32], String> {
    let bytes = STANDARD
        .decode(value.trim())
        .map_err(|_| format!("{name} is not base64"))?;
    bytes
        .try_into()
        .map_err(|_| format!("{name} must be 32 bytes"))
}
