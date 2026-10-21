//! Apple's push service, and what the relay sends it.
//!
//! Everything a phone shows is written here. A sender chooses only the kind
//! of push and, for a notification, an opaque blob the app's notification
//! service extension decrypts; when that fails, the text below is what the
//! person sees.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::config::{ApnsConfig, ApnsKey};
use crate::handle::Device;

/// Apple rejects a provider token older than an hour and throttles one
/// refreshed more often than every twenty minutes.
const TOKEN_LIFE: Duration = Duration::from_secs(50 * 60);

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
    pub fn is_alert(self) -> bool {
        matches!(self, Kind::Summon | Kind::Notify)
    }
}

/// What became of a push at Apple.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// The token no longer reaches the app. Its handles are worthless.
    Gone,
    /// Apple is refusing pushes to this device for now.
    Throttled,
    /// Apple, or the way to it, failed. Worth trying again later.
    Unavailable(String),
    /// Apple refused the relay itself: a wrong topic, key or environment.
    /// Nothing a sender does will change it, and retrying soon is no use.
    Rejected(String),
}

struct Gateway {
    base: String,
    key: EncodingKey,
    key_id: String,
    team_id: String,
    bearer: RwLock<Option<(Arc<str>, Instant)>>,
    clients: Vec<reqwest::Client>,
    next: AtomicUsize,
}

impl Gateway {
    fn new(base: &str, key: &ApnsKey, team_id: &str, connections: usize) -> Result<Self, String> {
        let pem = key.pem()?;
        let encoding = EncodingKey::from_ec_pem(pem.as_bytes())
            .map_err(|e| format!("key {} is not an EC private key: {e}", key.key_id))?;
        // One client per connection: a client multiplexes every request to
        // a host over a single HTTP/2 connection.
        let clients = (0..connections.max(1))
            .map(|_| {
                reqwest::Client::builder()
                    .http2_prior_knowledge()
                    .http2_keep_alive_interval(Duration::from_secs(30))
                    .http2_keep_alive_timeout(Duration::from_secs(10))
                    .http2_keep_alive_while_idle(true)
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

    /// Drop the provider token `used`, which Apple refused, unless another
    /// push has already replaced it: every push in flight with it is refused
    /// together, and Apple throttles a token refreshed too often.
    fn forget_bearer(&self, used: &Arc<str>) {
        let mut cached = self.bearer.write();
        if cached
            .as_ref()
            .is_some_and(|(token, _)| Arc::ptr_eq(token, used))
        {
            *cached = None;
        }
    }

    fn client(&self) -> &reqwest::Client {
        &self.clients[self.next.fetch_add(1, Ordering::Relaxed) % self.clients.len()]
    }
}

pub struct Apns {
    topic: String,
    production: Option<Gateway>,
    sandbox: Option<Gateway>,
}

impl Apns {
    pub fn new(cfg: &ApnsConfig, topic: &str) -> Result<Self, String> {
        if cfg.team_id.is_empty() {
            return Err("apns.team_id is not set".into());
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
        Ok(Self {
            topic: topic.to_string(),
            production,
            sandbox,
        })
    }

    /// Whether the relay can reach devices on this gateway.
    pub fn serves(&self, sandbox: bool) -> bool {
        self.gateway(sandbox).is_some()
    }

    fn gateway(&self, sandbox: bool) -> Option<&Gateway> {
        if sandbox {
            self.sandbox.as_ref()
        } else {
            self.production.as_ref()
        }
    }

    pub async fn send(&self, device: &Device, kind: Kind, blob: Option<&str>) -> Delivery {
        let Some(gateway) = self.gateway(device.sandbox) else {
            // A handle minted while the relay had this gateway's key, which
            // it no longer has: the handle will never work again.
            return Delivery::Gone;
        };
        let bearer = match gateway.bearer() {
            Ok(b) => b,
            Err(e) => return Delivery::Unavailable(format!("signing: {e}")),
        };
        let h = headers(kind);
        let mut request = gateway
            .client()
            .post(format!(
                "{}/3/device/{}",
                gateway.base,
                hex::encode(&device.token)
            ))
            .header("authorization", format!("bearer {bearer}"))
            .header("apns-topic", &self.topic)
            .header("apns-push-type", h.push_type)
            .header("apns-priority", h.priority)
            .header("apns-expiration", (unix_now() + h.expires_in).to_string());
        if let Some(id) = h.collapse {
            request = request.header("apns-collapse-id", id);
        }
        let response = match request.json(&payload(kind, blob)).send().await {
            Ok(r) => r,
            // The error's URL holds the device token, which goes no further.
            Err(e) => return Delivery::Unavailable(e.without_url().to_string()),
        };
        let status = response.status();
        if status.is_success() {
            return Delivery::Sent;
        }
        let reason = response
            .json::<Value>()
            .await
            .ok()
            .and_then(|v| v["reason"].as_str().map(str::to_owned))
            .unwrap_or_default();
        match (status.as_u16(), reason.as_str()) {
            (410, _) | (400, "BadDeviceToken" | "DeviceTokenNotForTopic") => Delivery::Gone,
            // Only this reason is about the device; Apple's other 429s
            // are about the relay.
            (429, "TooManyRequests") => Delivery::Throttled,
            (403, "ExpiredProviderToken" | "InvalidProviderToken") => {
                gateway.forget_bearer(&bearer);
                Delivery::Unavailable(format!("{status} {reason}"))
            }
            (429, _) | (500.., _) => Delivery::Unavailable(format!("{status} {reason}")),
            _ => Delivery::Rejected(format!("{status} {reason}")),
        }
    }
}

struct Headers {
    push_type: &'static str,
    priority: &'static str,
    expires_in: u64,
    /// Pushes with one id replace each other on the way rather than queue.
    collapse: Option<&'static str>,
}

fn headers(kind: Kind) -> Headers {
    let (push_type, priority, expires_in, collapse) = match kind {
        // Low priority is the only priority a background push may have.
        Kind::Wake => ("background", "5", 60 * 60, Some("koan-wake")),
        // Its own collapse id: replacing a queued `wake` with a push that
        // expires in a minute would lose the wake.
        Kind::WakeNow => ("background", "5", 60, Some("koan-wake-now")),
        Kind::Summon => ("alert", "10", 2 * 60, Some("koan-summon")),
        // "Play this" an hour late is not what anyone asked for.
        Kind::Notify => ("alert", "10", 10 * 60, None),
    };
    Headers {
        push_type,
        priority,
        expires_in,
        collapse,
    }
}

/// The JSON Apple is sent. Nothing in it but the blob comes from the sender.
pub fn payload(kind: Kind, blob: Option<&str>) -> Value {
    if !kind.is_alert() {
        return json!({ "aps": { "content-available": 1 } });
    }
    let mut body = json!({
        "aps": {
            "alert": { "title": ALERT_TITLE, "body": ALERT_BODY },
            "sound": "default",
            "interruption-level": "time-sensitive",
        },
    });
    if let Some(blob) = blob {
        // Hands the notification to the service extension, which replaces
        // the text above only if the blob decrypts.
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

    #[test]
    fn a_wake_carries_nothing() {
        assert_eq!(
            payload(Kind::Wake, None),
            json!({ "aps": { "content-available": 1 } })
        );
        assert_eq!(payload(Kind::WakeNow, None), payload(Kind::Wake, None));
    }

    #[test]
    fn a_notification_shows_the_relays_text_and_carries_the_blob() {
        let body = payload(Kind::Notify, Some("ciphertext"));
        assert_eq!(body["aps"]["alert"]["title"], ALERT_TITLE);
        assert_eq!(body["aps"]["alert"]["body"], ALERT_BODY);
        assert_eq!(body["aps"]["mutable-content"], 1);
        assert_eq!(body["koan-e2e"], "ciphertext");
        assert_eq!(body.as_object().unwrap().len(), 2);
    }

    #[test]
    fn a_notification_without_a_blob_is_not_handed_to_the_extension() {
        let body = payload(Kind::Summon, None);
        assert!(body["aps"].get("mutable-content").is_none());
        assert!(body.get("koan-e2e").is_none());
    }

    #[test]
    fn the_largest_payload_fits_apples_limit() {
        let blob = "A".repeat(MAX_BLOB);
        let bytes = serde_json::to_vec(&payload(Kind::Notify, Some(&blob))).unwrap();
        assert!(bytes.len() <= 4096, "{} bytes", bytes.len());
    }

    #[test]
    fn only_background_pushes_are_low_priority() {
        for kind in [Kind::Wake, Kind::WakeNow] {
            assert_eq!(headers(kind).push_type, "background");
            assert_eq!(headers(kind).priority, "5");
        }
        for kind in [Kind::Summon, Kind::Notify] {
            assert_eq!(headers(kind).push_type, "alert");
            assert_eq!(headers(kind).priority, "10");
        }
    }
}
