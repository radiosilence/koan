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
//!
//! A notification carries a link to its album's cover, which the app's
//! notification service extension fetches and attaches. The extension holds no
//! credentials, so the link authorises itself: an HMAC over one track id and
//! an expiry, keyed by a secret that lives only in this process. It opens that
//! one cover for a few minutes and nothing else.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::get;
use hmac::{Hmac, KeyInit, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use koan_core::config::PushConfig;
use koan_core::db::pool::Pool;
use parking_lot::Mutex;
use serde_json::{Value, json};

/// Apple rejects a token older than an hour and throttles one refreshed more
/// often than every twenty minutes.
const TOKEN_LIFE: Duration = Duration::from_secs(50 * 60);

/// How long a notification's cover link opens its cover. The extension fetches
/// it as the notification arrives; a notification delivered later than this
/// shows without art.
const COVER_LIFE: u64 = 10 * 60;

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
    /// `sharing.public_url`, which cover links are built on. Without it
    /// notifications carry no art.
    public_url: Option<String>,
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
    /// Wake the app because someone chose it in the Control menu: no use an
    /// hour late, unlike `Wake`'s sync.
    WakeNow,
    /// A notification asking to be tapped, for a device a push did not wake.
    /// Tapping it opens the app, which links.
    Summon { title: String, body: String },
    /// Show this, and carry `command` for the app to run when it is tapped.
    Notify {
        title: String,
        body: String,
        command: Value,
        /// A cover link from `Pusher::cover_link`.
        image: Option<String>,
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
            public_url: None,
        })
    }

    /// A link that opens the cover of `track_id`'s album for `COVER_LIFE`.
    pub fn cover_link(&self, track_id: i64) -> Option<String> {
        let base = self.public_url.as_deref()?.trim_end_matches('/');
        let expires = unix_now() + COVER_LIFE;
        let sig = cover_sig(track_id, expires);
        Some(format!("{base}/push/cover/{track_id}/{expires}/{sig}"))
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
        let Headers {
            kind,
            priority,
            expires_in,
            collapse,
        } = headers(push);
        // A Live Activity's pushes go to the app's topic with a suffix.
        let topic = match push {
            Push::Activity(_) => format!("{}.push-type.liveactivity", self.topic),
            _ => self.topic.clone(),
        };
        let expiration = unix_now() + expires_in;
        let request = http
            .post(format!("https://{host}/3/device/{token}"))
            .bearer_auth(bearer)
            .header("apns-topic", topic)
            .header("apns-push-type", kind)
            .header("apns-priority", priority)
            .header("apns-expiration", expiration.to_string());
        let request = match collapse {
            Some(id) => request.header("apns-collapse-id", id),
            None => request,
        };
        let response = request.json(&payload(push)).send();
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

struct Headers {
    kind: &'static str,
    priority: &'static str,
    expires_in: u64,
    /// Pushes with one id replace each other on the way, rather than queue.
    collapse: Option<&'static str>,
}

fn headers(push: &Push) -> Headers {
    let (kind, priority, expires_in, collapse) = match push {
        // Low priority is the only priority a background push may have.
        Push::Wake => ("background", "5", 60 * 60, Some("koan-wake")),
        Push::WakeNow => ("background", "5", 60, Some("koan-wake")),
        // "Play this" an hour late is not what anyone asked for.
        Push::Notify { .. } => ("alert", "10", 10 * 60, None),
        Push::Summon { .. } => ("alert", "10", 2 * 60, Some("koan-summon")),
        // Stale within the minute: the device will have moved on.
        Push::Activity(_) => ("liveactivity", "10", 60, None),
    };
    Headers {
        kind,
        priority,
        expires_in,
        collapse,
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
        Push::Wake | Push::WakeNow => json!({ "aps": { "content-available": 1 } }),
        Push::Summon { title, body } => json!({
            "aps": {
                "alert": { "title": title, "body": body },
                "sound": "default",
                "interruption-level": "time-sensitive",
            },
        }),
        Push::Notify {
            title,
            body,
            command,
            image,
        } => {
            let mut body = json!({
                "aps": {
                    "alert": { "title": title, "body": body },
                    "sound": "default",
                    // Asked for this moment, by the person it is for: through Focus.
                    "interruption-level": "time-sensitive",
                },
                "koan": command,
            });
            if let Some(image) = image {
                // Hands the notification to the service extension, which
                // attaches the cover before it is shown.
                body["aps"]["mutable-content"] = json!(1);
                body["image"] = json!(image);
            }
            body
        }
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
    /// The sleeve, as a small base64 JPEG: a Live Activity cannot fetch
    /// anything, and this is how it gets one while the app is suspended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
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
            art: info
                .state
                .queue
                .iter()
                .find(|e| e.current)
                .and_then(|e| e.track_id.as_deref())
                .and_then(activity_art),
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

/// Apple refuses a Live Activity push over 4 KB, and the text, the times and
/// the envelope take well under a kilobyte of it.
const ART_BUDGET: usize = 2600;

/// The cover of `track` (a uid or a row id) small enough to ride in a Live
/// Activity push, as base64. Worked out once per track: the state it goes in
/// is reported far more often than the track changes.
fn activity_art(track: &str) -> Option<String> {
    use base64::Engine as _;
    static LAST: parking_lot::Mutex<Option<(String, Option<String>)>> =
        parking_lot::Mutex::new(None);
    static COVERS: std::sync::LazyLock<crate::covers::Covers> =
        std::sync::LazyLock::new(crate::covers::Covers::in_config_dir);
    if cfg!(test) {
        return None;
    }
    if let Some((held, art)) = LAST.lock().as_ref()
        && held == track
    {
        return art.clone();
    }
    let art = (|| {
        let db = koan_core::db::pool::shared().get().ok()?;
        let id = koan_core::db::queries::resolve_id(
            &db.conn,
            koan_core::db::queries::UidKind::Track,
            track,
        )
        .ok()??;
        let row = koan_core::db::queries::get_track_row(&db.conn, id).ok()??;
        let cover = COVERS.cover(std::slice::from_ref(&row), crate::covers::SIZES[0])?;
        let img = image::load_from_memory(&cover).ok()?;
        // Smaller and rougher until it fits.
        [(72, 60), (60, 50), (48, 40)]
            .into_iter()
            .find_map(|(side, quality)| {
                let mut out = Vec::new();
                let small = img.thumbnail(side, side).to_rgb8();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
                    .encode_image(&small)
                    .ok()?;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&out);
                (b64.len() <= ART_BUDGET).then_some(b64)
            })
    })();
    *LAST.lock() = Some((track.to_string(), art.clone()));
    art
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
        let pusher = Pusher::from_config(&cfg.push).map(|p| Pusher {
            public_url: cfg.sharing.public_url.filter(|u| !u.trim().is_empty()),
            ..p
        });
        if pusher.is_some() {
            log::info!("push: APNs key {} for {}", cfg.push.key_id, cfg.push.topic);
        }
        pusher
    });
    PUSHER.as_ref()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Minted at start-up and never stored: a restart voids outstanding cover
/// links, which live minutes anyway.
fn cover_mac() -> Hmac<sha2::Sha256> {
    static KEY: std::sync::LazyLock<[u8; 32]> = std::sync::LazyLock::new(|| {
        let mut key = [0; 32];
        getrandom::fill(&mut key).expect("system randomness");
        key
    });
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&*KEY).expect("any key length");
    mac.update(b"koan notification cover\0");
    mac
}

fn cover_sig(track_id: i64, expires: u64) -> String {
    use base64::Engine;
    let mut mac = cover_mac();
    mac.update(format!("{track_id}.{expires}").as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn cover_sig_valid(track_id: i64, expires: u64, sig: &str) -> bool {
    use base64::Engine;
    let Ok(sig) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(sig) else {
        return false;
    };
    let mut mac = cover_mac();
    mac.update(format!("{track_id}.{expires}").as_bytes());
    expires >= unix_now() && mac.verify_slice(&sig).is_ok()
}

#[derive(Clone)]
struct CoverState {
    pool: std::sync::Arc<Pool>,
    covers: std::sync::Arc<crate::covers::Covers>,
}

/// `/push/cover/{track}/{expires}/{sig}`: public, since the extension that
/// fetches it has no login. Anything but a valid, unexpired link is a 404.
pub fn router(
    pool: std::sync::Arc<Pool>,
    covers: std::sync::Arc<crate::covers::Covers>,
) -> axum::Router {
    axum::Router::new()
        .route("/push/cover/{track}/{expires}/{sig}", get(cover))
        .with_state(CoverState { pool, covers })
}

async fn cover(
    State(s): State<CoverState>,
    UrlPath((track, expires, sig)): UrlPath<(i64, u64, String)>,
) -> Response {
    if !cover_sig_valid(track, expires, &sig) {
        return crate::share::not_found();
    }
    let art = crate::share::blocking(move || {
        let db = s.pool.get().ok()?;
        let row = koan_core::db::queries::get_track_row(&db.conn, track).ok()??;
        s.covers
            .cover(std::slice::from_ref(&row), crate::covers::LARGE)
    })
    .await;
    crate::share::jpeg(art, false)
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

    /// Apple delivers a background push only at priority 5, and holds one
    /// collapse id's pushes as one rather than queueing every retry.
    #[test]
    fn wakes_are_background_low_priority_and_collapse() {
        for push in [Push::Wake, Push::WakeNow] {
            let h = headers(&push);
            assert_eq!(
                (h.kind, h.priority, h.collapse),
                ("background", "5", Some("koan-wake"))
            );
            assert_eq!(payload(&push)["aps"]["content-available"], 1);
        }
        assert!(
            headers(&Push::WakeNow).expires_in <= 60,
            "a late wake is no wake"
        );
        let summon = Push::Summon {
            title: "Mac wants to play here".into(),
            body: "Tap to let it".into(),
        };
        let h = headers(&summon);
        assert_eq!(
            (h.kind, h.priority, h.collapse),
            ("alert", "10", Some("koan-summon"))
        );
        assert_eq!(
            payload(&summon)["aps"]["alert"]["title"],
            "Mac wants to play here"
        );
        assert!(
            payload(&summon).get("koan").is_none(),
            "tapping it only opens the app"
        );
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
            image: None,
        });
        assert_eq!(body["koan"], command);
        assert_eq!(body["aps"]["alert"]["body"], "Golden Standard");
        assert_eq!(body["aps"]["interruption-level"], "time-sensitive");
        assert!(body["aps"].get("mutable-content").is_none());
        assert_eq!(payload(&Push::Wake)["aps"]["content-available"], 1);

        let body = payload(&Push::Notify {
            title: "Play on this iPhone".into(),
            body: "Golden Standard".into(),
            command,
            image: Some("https://koan.example/push/cover/1/2/x".into()),
        });
        assert_eq!(body["aps"]["mutable-content"], 1);
        assert_eq!(body["image"], "https://koan.example/push/cover/1/2/x");
    }

    #[test]
    fn cover_links_open_one_cover_until_they_expire() {
        let later = unix_now() + 60;
        let sig = cover_sig(7, later);
        assert!(cover_sig_valid(7, later, &sig));
        assert!(!cover_sig_valid(8, later, &sig));
        assert!(!cover_sig_valid(7, later + 1, &sig));
        assert!(!cover_sig_valid(7, later, "not-a-signature"));
        let past = unix_now() - 1;
        assert!(!cover_sig_valid(7, past, &cover_sig(7, past)));
    }
}
