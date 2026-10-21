//! Apple's push service, and what the relay sends it.
//!
//! Everything Apple is sent is written here. A sender chooses only the kind
//! of push and, for a notification, an opaque blob the app's notification
//! service extension decrypts; when that fails, the text below is what the
//! person sees.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::{ApnsConfig, ApnsKey};
use crate::handle::Slot;
use crate::quota::Clock;

/// Apple rejects a provider token older than an hour and throttles one
/// refreshed more often than every twenty minutes.
const TOKEN_LIFE: Duration = Duration::from_secs(50 * 60);
const MIN_REFRESH: Duration = Duration::from_secs(20 * 60);

/// How often a gateway Apple has refused lets one push through, to learn
/// whether that has been fixed.
const PROBE_EVERY: Duration = Duration::from_secs(60);

pub const ALERT_TITLE: &str = "kōan";
pub const ALERT_BODY: &str = "Something is waiting to play. Tap to open.";

/// The longest blob a sender may attach, encoded. Apple's limit is 4 KB for
/// the whole payload; this leaves room for the relay's own part.
pub const MAX_BLOB: usize = 3072;

/// What a sender asks of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// Wake the app to link and take what waits for it.
    Wake,
    /// The same, for something that is no use an hour late.
    WakeNow,
    /// A notification asking to be tapped.
    Summon,
    /// A notification carrying a command the app runs when it is tapped.
    Notify,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Wake, Kind::WakeNow, Kind::Summon, Kind::Notify];

    pub fn is_alert(self) -> bool {
        matches!(self, Kind::Summon | Kind::Notify)
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Wake => "wake",
            Kind::WakeNow => "wake-now",
            Kind::Summon => "summon",
            Kind::Notify => "notify",
        }
    }
}

/// What became of a push.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// Apple says the app is gone from the device (410 Unregistered).
    Unregistered,
    /// Apple says the token is not one of this app's on this gateway. For a
    /// token never proven that means made up; for a proven one, that the
    /// relay's topic or gateway has changed under it.
    BadToken,
    /// Apple is refusing pushes to this device for now.
    Throttled,
    /// The relay no longer holds a key for the handle's gateway.
    Unserved,
    /// Apple, or the way to it, failed. Worth trying again shortly.
    Unavailable(String),
    /// Apple refused this push for a reason the relay does not know. Not
    /// worth trying again soon, and not a reason to stop the gateway.
    Refused(String),
    /// Apple refused the relay itself: a wrong key, team, topic or
    /// environment. The gateway's breaker is open from now.
    Rejected(String),
    /// The gateway's breaker is open, so nothing was sent.
    Held,
}

/// Open while Apple refuses a gateway's credential. Sending on with a
/// credential Apple has refused is what gets a key looked at, so the gateway
/// stops, lets one push through a minute, and closes only on an answer that
/// shows the credential works.
#[derive(Default)]
struct Breaker {
    open: AtomicBool,
    next_probe: AtomicU64,
}

impl Breaker {
    fn allows(&self, now: u64) -> bool {
        if !self.open.load(Ordering::Acquire) {
            return true;
        }
        let next = self.next_probe.load(Ordering::Relaxed);
        now >= next
            && self
                .next_probe
                .compare_exchange(
                    next,
                    now + PROBE_EVERY.as_nanos() as u64,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
    }

    fn trip(&self, now: u64) {
        self.next_probe
            .store(now + PROBE_EVERY.as_nanos() as u64, Ordering::Relaxed);
        self.open.store(true, Ordering::Release);
    }

    fn close(&self) {
        self.open.store(false, Ordering::Release);
    }
}

struct Gateway {
    base: String,
    key: EncodingKey,
    key_id: String,
    team_id: String,
    bearer: RwLock<Option<(Arc<str>, Instant)>>,
    clients: Vec<reqwest::Client>,
    next: AtomicUsize,
    breaker: Breaker,
}

impl Gateway {
    fn new(base: &str, key: &ApnsKey, team_id: &str, connections: usize) -> Result<Self, String> {
        let pem = key.pem()?;
        let encoding = EncodingKey::from_ec_pem(pem.as_bytes())
            .map_err(|e| format!("key {} is not an EC private key: {e}", key.key_id))?;
        // One client per connection: a client multiplexes every request to
        // a host over a single HTTP/2 connection. Idle connections are kept,
        // as Apple asks, rather than reopened for each push.
        let clients = (0..connections)
            .map(|_| {
                reqwest::Client::builder()
                    .http2_prior_knowledge()
                    .http2_keep_alive_interval(Duration::from_secs(30))
                    .http2_keep_alive_timeout(Duration::from_secs(10))
                    .http2_keep_alive_while_idle(true)
                    .pool_idle_timeout(None)
                    .tcp_nodelay(true)
                    .connect_timeout(Duration::from_secs(5))
                    .timeout(Duration::from_secs(10))
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            key: encoding,
            key_id: key.key_id.clone(),
            team_id: team_id.to_string(),
            bearer: RwLock::new(None),
            clients,
            next: AtomicUsize::new(0),
            breaker: Breaker::default(),
        })
    }

    fn bearer(&self) -> Result<Arc<str>, String> {
        if let Some((token, at)) = self.bearer.read().as_ref()
            && at.elapsed() < TOKEN_LIFE
        {
            return Ok(token.clone());
        }
        let mut cached = self.bearer.write();
        if let Some((token, at)) = cached.as_ref()
            && at.elapsed() < TOKEN_LIFE
        {
            return Ok(token.clone());
        }
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        let token: Arc<str> = jsonwebtoken::encode(
            &header,
            &json!({ "iss": self.team_id, "iat": unix_now() }),
            &self.key,
        )
        .map_err(|e| e.to_string())?
        .into();
        *cached = Some((token.clone(), Instant::now()));
        Ok(token)
    }

    /// Apple calls the provider token `used` expired. Drop it, unless
    /// another push has already replaced it: every push in flight with it is
    /// refused together. One signed under twenty minutes ago is not expired;
    /// the relay's clock is wrong, and signing again would only be throttled,
    /// so that is an operator's to fix: `false`.
    fn expire_bearer(&self, used: &Arc<str>) -> bool {
        let mut cached = self.bearer.write();
        match cached.as_ref() {
            Some((token, at)) if Arc::ptr_eq(token, used) => {
                if at.elapsed() < MIN_REFRESH {
                    return false;
                }
                *cached = None;
                true
            }
            _ => true,
        }
    }

    fn client(&self) -> &reqwest::Client {
        &self.clients[self.next.fetch_add(1, Ordering::Relaxed) % self.clients.len()]
    }
}

/// Who a push is for.
pub struct Target<'a> {
    pub token: &'a [u8],
    pub sandbox: bool,
    /// The server the push came through, for a push through a handle; none
    /// for the relay's own proof of a token.
    pub slot: Option<&'a Slot>,
}

pub struct Apns {
    topic: String,
    production: Option<Gateway>,
    sandbox: Option<Gateway>,
    clock: Clock,
}

impl Apns {
    pub fn new(cfg: &ApnsConfig, topic: &str) -> Result<Self, String> {
        if cfg.team_id.is_empty() {
            return Err("apns.team_id is not set".into());
        }
        if !(1..=64).contains(&cfg.connections) {
            return Err("apns.connections must be 1 to 64".into());
        }
        let gateway = |base: &str, key: &Option<ApnsKey>| {
            key.as_ref()
                .map(|key| Gateway::new(base, key, &cfg.team_id, cfg.connections))
                .transpose()
        };
        let production = gateway(&cfg.production_url, &cfg.production)?;
        let sandbox = gateway(&cfg.sandbox_url, &cfg.sandbox)?;
        if production.is_none() && sandbox.is_none() {
            return Err("no APNs key: set apns.production or apns.sandbox".into());
        }
        // Sign once now, so a key that cannot sign stops the relay starting
        // rather than failing every push.
        for gateway in production.iter().chain(sandbox.iter()) {
            gateway
                .bearer()
                .map_err(|e| format!("signing with key {}: {e}", gateway.key_id))?;
        }
        Ok(Self {
            topic: topic.to_string(),
            production,
            sandbox,
            clock: Clock::new(),
        })
    }

    /// Whether the relay can reach devices on this gateway.
    pub fn serves(&self, sandbox: bool) -> bool {
        self.gateway(sandbox).is_some()
    }

    /// Whether this gateway's breaker is open.
    pub fn held(&self, sandbox: bool) -> bool {
        self.gateway(sandbox)
            .is_some_and(|g| g.breaker.open.load(Ordering::Relaxed))
    }

    /// Open this gateway's breaker: Apple answered in a way that says the
    /// relay, not the device, is wrong.
    pub fn trip(&self, sandbox: bool) {
        if let Some(gateway) = self.gateway(sandbox) {
            gateway.breaker.trip(self.clock.now());
        }
    }

    fn gateway(&self, sandbox: bool) -> Option<&Gateway> {
        if sandbox {
            self.sandbox.as_ref()
        } else {
            self.production.as_ref()
        }
    }

    pub async fn send(&self, to: &Target<'_>, kind: Kind, blob: Option<&str>) -> Delivery {
        let Some(gateway) = self.gateway(to.sandbox) else {
            return Delivery::Unserved;
        };
        if !gateway.breaker.allows(self.clock.now()) {
            return Delivery::Held;
        }
        let bearer = match gateway.bearer() {
            Ok(b) => b,
            Err(e) => return Delivery::Unavailable(format!("signing: {e}")),
        };
        let h = headers(kind, to.slot);
        let response = gateway
            .client()
            .post(format!(
                "{}/3/device/{}",
                gateway.base,
                hex::encode(to.token)
            ))
            .header("authorization", format!("bearer {bearer}"))
            .header("apns-topic", &self.topic)
            .header("apns-push-type", h.push_type)
            .header("apns-priority", h.priority)
            .header("apns-expiration", (unix_now() + h.expires_in).to_string())
            .header("apns-collapse-id", h.collapse)
            .json(&payload(kind, to.slot, blob))
            .send()
            .await;
        let response = match response {
            Ok(r) => r,
            // The error's URL holds the device token, which goes no further.
            Err(e) => return Delivery::Unavailable(e.without_url().to_string()),
        };
        let status = response.status();
        let delivery = if status.is_success() {
            Delivery::Sent
        } else {
            let reason = response
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v["reason"].as_str().map(str::to_owned))
                .unwrap_or_default();
            match (status.as_u16(), reason.as_str()) {
                (410, _) => Delivery::Unregistered,
                (400, "BadDeviceToken" | "DeviceTokenNotForTopic") => Delivery::BadToken,
                // Only this reason is about the device; Apple's other 429s
                // are about the relay.
                (429, "TooManyRequests") => Delivery::Throttled,
                // An expired token is re-signed by the next push. A token
                // Apple calls invalid, or refreshed too often, takes an
                // operator: a wrong key or team, or a relay re-signing in a
                // loop.
                (403, "ExpiredProviderToken") => {
                    if gateway.expire_bearer(&bearer) {
                        Delivery::Unavailable(format!("{status} {reason}"))
                    } else {
                        Delivery::Rejected(format!(
                            "{status} {reason} for a token signed under twenty minutes ago: is the clock right?"
                        ))
                    }
                }
                // Answers about the relay's own credential, topic or
                // environment: every push through the gateway would get them.
                (
                    403,
                    "InvalidProviderToken"
                    | "MissingProviderToken"
                    | "Forbidden"
                    | "BadEnvironmentKeyIdInToken"
                    | "UnrelatedKeyIdInToken",
                )
                | (
                    400,
                    "BadTopic"
                    | "TopicDisallowed"
                    | "MissingTopic"
                    | "BadCertificate"
                    | "BadCertificateEnvironment",
                )
                | (429, "TooManyProviderTokenUpdates") => {
                    Delivery::Rejected(format!("{status} {reason}"))
                }
                (429, _) | (500.., _) => Delivery::Unavailable(format!("{status} {reason}")),
                _ => Delivery::Refused(format!("{status} {reason}")),
            }
        };
        match &delivery {
            // Answers only a working credential gets.
            Delivery::Sent | Delivery::Unregistered | Delivery::Throttled => {
                gateway.breaker.close()
            }
            Delivery::Rejected(_) => gateway.breaker.trip(self.clock.now()),
            _ => {}
        }
        delivery
    }
}

struct Headers {
    push_type: &'static str,
    priority: &'static str,
    expires_in: u64,
    /// Pushes with one id replace each other on the way rather than queue.
    collapse: String,
}

fn headers(kind: Kind, slot: Option<&Slot>) -> Headers {
    // A server's notifications replace its own, never another server's.
    let from = |prefix: &str| match slot {
        Some(slot) => format!("{prefix}-{}", hex::encode(slot)),
        None => prefix.to_string(),
    };
    let (push_type, priority, expires_in, collapse) = match kind {
        // Low priority is the only priority a background push may have.
        Kind::Wake => ("background", "5", 60 * 60, "koan-wake".to_string()),
        // Its own collapse id: replacing a queued `wake` with a push that
        // expires in a minute would lose the wake.
        Kind::WakeNow => ("background", "5", 60, "koan-wake-now".to_string()),
        Kind::Summon => ("alert", "10", 2 * 60, from("koan-summon")),
        // "Play this" an hour late is not what anyone asked for, and a newer
        // request replaces an older one rather than stacking.
        Kind::Notify => ("alert", "10", 10 * 60, from("koan-play")),
    };
    Headers {
        push_type,
        priority,
        expires_in,
        collapse,
    }
}

/// The JSON Apple is sent. Nothing in it but the blob comes from the sender;
/// `koan-from` is the slot sealed in the handle the push came through, which
/// the app maps to the server it gave that handle, whatever the blob says.
pub fn payload(kind: Kind, slot: Option<&Slot>, blob: Option<&str>) -> Value {
    if !kind.is_alert() {
        return json!({ "aps": { "content-available": 1 } });
    }
    let mut body = json!({
        "aps": {
            "alert": { "title": ALERT_TITLE, "body": ALERT_BODY },
            "sound": "default",
            // Never time-sensitive: a relayed notification comes from a
            // server the relay cannot vouch for, and does not break through
            // a Focus.
            "interruption-level": "active",
        },
    });
    if let Some(slot) = slot {
        body["koan-from"] = json!(hex::encode(slot));
    }
    if let Some(blob) = blob {
        // Hands the notification to the service extension, which shows its
        // own template in place of the text above only if the blob decrypts.
        body["aps"]["mutable-content"] = json!(1);
        body["koan-e2e"] = json!(blob);
    }
    body
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOT: Slot = [0x5a; 16];

    #[test]
    fn a_wake_carries_nothing() {
        assert_eq!(
            payload(Kind::Wake, Some(&SLOT), None),
            json!({ "aps": { "content-available": 1 } })
        );
        assert_eq!(
            payload(Kind::WakeNow, None, None),
            payload(Kind::Wake, Some(&SLOT), None)
        );
    }

    #[test]
    fn a_notification_shows_the_relays_text_and_says_who_sent_it() {
        let body = payload(Kind::Notify, Some(&SLOT), Some("ciphertext"));
        assert_eq!(body["aps"]["alert"]["title"], ALERT_TITLE);
        assert_eq!(body["aps"]["alert"]["body"], ALERT_BODY);
        assert_eq!(body["aps"]["interruption-level"], "active");
        assert_eq!(body["aps"]["mutable-content"], 1);
        assert_eq!(body["koan-e2e"], "ciphertext");
        assert_eq!(body["koan-from"], hex::encode(SLOT));
        assert_eq!(body.as_object().unwrap().len(), 3);
    }

    #[test]
    fn a_notification_without_a_blob_is_not_handed_to_the_extension() {
        let body = payload(Kind::Summon, Some(&SLOT), None);
        assert!(body["aps"].get("mutable-content").is_none());
        assert!(body.get("koan-e2e").is_none());
    }

    #[test]
    fn the_largest_payload_fits_apples_limit() {
        let blob = "A".repeat(MAX_BLOB);
        let bytes = serde_json::to_vec(&payload(Kind::Notify, Some(&SLOT), Some(&blob))).unwrap();
        assert!(bytes.len() <= 4096, "{} bytes", bytes.len());
    }

    #[test]
    fn only_background_pushes_are_low_priority() {
        for kind in [Kind::Wake, Kind::WakeNow] {
            assert_eq!(headers(kind, None).push_type, "background");
            assert_eq!(headers(kind, None).priority, "5");
        }
        for kind in [Kind::Summon, Kind::Notify] {
            assert_eq!(headers(kind, None).push_type, "alert");
            assert_eq!(headers(kind, None).priority, "10");
        }
    }

    #[test]
    fn one_servers_notifications_never_replace_anothers() {
        let other: Slot = [0x11; 16];
        for kind in [Kind::Summon, Kind::Notify] {
            let a = headers(kind, Some(&SLOT)).collapse;
            assert_ne!(a, headers(kind, Some(&other)).collapse);
            assert!(a.len() <= 64, "Apple's limit on a collapse id");
        }
    }

    #[test]
    fn a_held_breaker_lets_one_probe_through_a_minute_until_closed() {
        let b = Breaker::default();
        assert!(b.allows(0));
        b.trip(0);
        let minute = PROBE_EVERY.as_nanos() as u64;
        assert!(!b.allows(1));
        assert!(b.allows(minute), "the minute's probe");
        assert!(!b.allows(minute), "and only one");
        // No timer closes it: a minute on there is another single probe, and
        // only a working answer lets everything through again.
        assert!(!b.allows(2 * minute - 1));
        assert!(b.allows(2 * minute));
        assert!(!b.allows(2 * minute));
        b.close();
        assert!(b.allows(2 * minute));
    }
}
