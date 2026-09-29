//! Apple's push service, for reaching a koan iOS app that iOS has suspended.
//!
//! A linked app is reached over its WebSocket. iOS suspends an app in the
//! background that is not playing, and the socket dies with it; a push is the
//! supported way back. Two kinds: a background push that wakes the app for
//! half a minute to link and take what waits for it in the outbox, and a
//! notification for playback, which iOS will not let a suspended app start on
//! its own: the person taps it, the app opens and plays.
//!
//! Token auth: a JWT signed with the team's `.p8` key, reused for under an
//! hour as Apple asks. HTTP/2, which is all the gateway speaks.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use koan_core::config::PushConfig;
use parking_lot::Mutex;
use serde_json::{Value, json};

/// Apple rejects a token older than an hour and throttles one refreshed more
/// often than every twenty minutes.
const TOKEN_LIFE: Duration = Duration::from_secs(50 * 60);

pub struct Pusher {
    key: EncodingKey,
    key_id: String,
    team_id: String,
    topic: String,
    bearer: Mutex<Option<(String, SystemTime)>>,
    /// Built on first send, which is always on a thread of its own: a blocking
    /// client runs a runtime, and building one inside the server's async
    /// runtime panics.
    http: std::sync::OnceLock<reqwest::blocking::Client>,
}

/// What became of a push.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    /// The token is no longer valid for this app: the app was deleted, or the
    /// token belongs to the other gateway. Forget it.
    Gone,
    Failed(String),
}

/// What a push asks of the device.
pub enum Push {
    /// Wake the app to link; whatever waits in the outbox follows.
    Wake,
    /// Show this, and carry `command` for the app to run when it is tapped.
    Notify {
        title: String,
        body: String,
        command: Value,
    },
    /// Update a Live Activity that shows another device.
    Activity(ActivityState),
}

impl Pusher {
    /// The pusher `cfg` describes, if it names a key that can be read.
    pub fn from_config(cfg: &PushConfig) -> Option<Self> {
        if cfg.key_id.is_empty() || cfg.team_id.is_empty() {
            return None;
        }
        let pem = match (&cfg.key, &cfg.key_path) {
            (Some(pem), _) if !pem.trim().is_empty() => pem.clone(),
            (_, Some(path)) => read_key(path)?,
            _ => return None,
        };
        let key = match EncodingKey::from_ec_pem(pem.as_bytes()) {
            Ok(key) => key,
            Err(e) => {
                log::warn!("push: the APNs key is not an EC private key: {e}");
                return None;
            }
        };
        Some(Self {
            key,
            key_id: cfg.key_id.clone(),
            team_id: cfg.team_id.clone(),
            topic: cfg.topic.clone(),
            bearer: Mutex::new(None),
            http: std::sync::OnceLock::new(),
        })
    }

    fn bearer(&self) -> Result<String, String> {
        let mut cached = self.bearer.lock();
        if let Some((token, at)) = cached.as_ref()
            && at.elapsed().is_ok_and(|age| age < TOKEN_LIFE)
        {
            return Ok(token.clone());
        }
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        let iat = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
        let token = jsonwebtoken::encode(
            &header,
            &json!({ "iss": self.team_id, "iat": iat }),
            &self.key,
        )
        .map_err(|e| e.to_string())?;
        *cached = Some((token.clone(), SystemTime::now()));
        Ok(token)
    }

    /// Send `push` to the device holding `token`. Blocks for the round trip,
    /// so never call it from async code.
    pub fn send(&self, token: &str, sandbox: bool, push: &Push) -> Outcome {
        let http = self.http.get_or_init(|| {
            reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default()
        });
        let bearer = match self.bearer() {
            Ok(b) => b,
            Err(e) => return Outcome::Failed(format!("signing: {e}")),
        };
        let host = if sandbox {
            "api.sandbox.push.apple.com"
        } else {
            "api.push.apple.com"
        };
        let (kind, priority, expires_in) = match push {
            // Low priority is the only priority a background push may have.
            Push::Wake => ("background", "5", 60 * 60),
            // "Play this" an hour late is not what anyone asked for.
            Push::Notify { .. } => ("alert", "10", 10 * 60),
            // Stale within the minute: the device will have moved on.
            Push::Activity(_) => ("liveactivity", "10", 60),
        };
        // A Live Activity's pushes go to the app's topic with a suffix.
        let topic = match push {
            Push::Activity(_) => format!("{}.push-type.liveactivity", self.topic),
            _ => self.topic.clone(),
        };
        let expiration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() + expires_in)
            .unwrap_or_default();
        let response = http
            .post(format!("https://{host}/3/device/{token}"))
            .bearer_auth(bearer)
            .header("apns-topic", topic)
            .header("apns-push-type", kind)
            .header("apns-priority", priority)
            .header("apns-expiration", expiration.to_string())
            .json(&payload(push))
            .send();
        match response {
            Ok(r) if r.status().is_success() => Outcome::Sent,
            Ok(r) => {
                let status = r.status();
                let reason = r
                    .json::<Value>()
                    .ok()
                    .and_then(|v| v["reason"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                if status == reqwest::StatusCode::GONE
                    || matches!(
                        reason.as_str(),
                        "BadDeviceToken" | "Unregistered" | "DeviceTokenNotForTopic"
                    )
                {
                    Outcome::Gone
                } else {
                    Outcome::Failed(format!("{status} {reason}"))
                }
            }
            Err(e) => Outcome::Failed(e.to_string()),
        }
    }
}

/// The JSON a push carries. The command rides under `koan`, beside Apple's
/// `aps`, so the app can act on it without asking the server first.
pub fn payload(push: &Push) -> Value {
    match push {
        Push::Activity(state) => json!({
            "aps": {
                "timestamp": state.at as u64,
                "event": "update",
                "content-state": state,
                // Past this the lock screen dims it: nothing has been heard.
                "stale-date": state.at as u64 + ACTIVITY_STALE_SECS,
            },
        }),
        Push::Wake => json!({ "aps": { "content-available": 1 } }),
        Push::Notify {
            title,
            body,
            command,
        } => json!({
            "aps": {
                "alert": { "title": title, "body": body },
                "sound": "default",
                // Asked for this moment, by the person it is for: through Focus.
                "interruption-level": "time-sensitive",
            },
            "koan": command,
        }),
    }
}

/// What a Live Activity shows of the device it follows. The field names are
/// the app's `RemoteActivity.ContentState`, which decodes it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityState {
    pub device: String,
    pub linked: bool,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// When `position_ms` was true, in Unix seconds.
    pub at: f64,
}

/// A Live Activity nobody has updated in this long shows as stale.
const ACTIVITY_STALE_SECS: u64 = 8 * 60 * 60;

impl ActivityState {
    pub fn of(info: &crate::clients::ClientInfo) -> Self {
        Self {
            device: info.name.clone(),
            linked: true,
            title: info.state.title.clone(),
            artist: info.state.artist.clone(),
            album: info.state.album.clone(),
            playing: info.state.playing,
            position_ms: info.position_ms(),
            duration_ms: info.state.duration_ms,
            at: chrono::Utc::now().timestamp_millis() as f64 / 1000.0,
        }
    }

    /// Whether the activity would show something different from `sent`. A
    /// playhead moving on time is drawn by the activity itself.
    pub fn differs(&self, sent: &Self) -> bool {
        let strip = |s: &Self| Self {
            position_ms: 0,
            at: 0.0,
            ..s.clone()
        };
        if strip(self) != strip(sent) {
            return true;
        }
        let expected = if sent.playing {
            sent.position_ms + ((self.at - sent.at).max(0.0) * 1000.0) as u64
        } else {
            sent.position_ms
        };
        self.position_ms.abs_diff(expected) > 3000
    }
}

fn read_key(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(pem) => Some(pem),
        Err(e) => {
            log::warn!("push: cannot read the APNs key at {}: {e}", path.display());
            None
        }
    }
}

/// The server's pusher, from the config it started with; `None` when no key
/// is configured, and pushes are simply not sent.
pub fn pusher() -> Option<&'static Pusher> {
    static PUSHER: std::sync::LazyLock<Option<Pusher>> = std::sync::LazyLock::new(|| {
        let cfg = koan_core::config::Config::load().unwrap_or_default();
        let pusher = Pusher::from_config(&cfg.push);
        if pusher.is_some() {
            log::info!("push: APNs key {} for {}", cfg.push.key_id, cfg.push.topic);
        }
        pusher
    });
    PUSHER.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway P-256 key, generated for these tests and used nowhere else.
    const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgMavrDWJ3FFXFskYn
rsmSSRycut7lJn10pzM1NivXehuhRANCAARKklUe/y7J3SZEK36mnyt5ejhmbNKT
AtQSJr6Wg9OtOkzZdoOhdRVcNFW8q9peFQ+S7qIcWNbXlhi+cAlpf0ce
-----END PRIVATE KEY-----";

    fn config() -> PushConfig {
        PushConfig {
            key: Some(TEST_KEY.into()),
            key_id: "ABC123DEFG".into(),
            team_id: "TEAM123456".into(),
            ..PushConfig::default()
        }
    }

    #[test]
    fn no_key_no_pusher() {
        assert!(Pusher::from_config(&PushConfig::default()).is_none());
    }

    #[test]
    fn the_token_is_es256_with_the_key_id_and_reused() {
        let pusher = Pusher::from_config(&config()).expect("pusher");
        let token = pusher.bearer().unwrap();
        let header = jsonwebtoken::decode_header(&token).unwrap();
        assert_eq!(header.alg, Algorithm::ES256);
        assert_eq!(header.kid.as_deref(), Some("ABC123DEFG"));
        assert_eq!(pusher.bearer().unwrap(), token);
    }

    #[test]
    fn a_notification_carries_its_command() {
        let command = json!({ "type": "play", "trackIds": ["1"], "startAt": 0 });
        let body = payload(&Push::Notify {
            title: "Play on this iPhone".into(),
            body: "Golden Standard".into(),
            command: command.clone(),
        });
        assert_eq!(body["koan"], command);
        assert_eq!(body["aps"]["alert"]["body"], "Golden Standard");
        assert_eq!(body["aps"]["interruption-level"], "time-sensitive");
        assert_eq!(payload(&Push::Wake)["aps"]["content-available"], 1);
    }
}
