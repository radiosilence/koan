//! A koan client's standing connection to the koan server it syncs from.
//!
//! The server can then act on the client: play a list of tracks on a phone, or
//! pause it. Subsonic has no way for a server to reach a client, so this is a
//! koan extension: a WebSocket at `/rest/koanLink`, authenticated like every
//! other `/rest` call, carrying [`LinkCommand`]s as JSON text frames from the
//! server. Track ids are the server's, which a synced client holds as each
//! track's `remote_id`.

use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use tungstenite::stream::MaybeTlsStream;

use crate::config::{self, Config};
use crate::helpers::{subsonic_auth, subsonic_client};
use crate::remote::client::SubsonicAuth;

/// What a server asks a linked client to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LinkCommand {
    /// Replace the queue with these tracks and play from `start_at`.
    #[serde(rename_all = "camelCase")]
    Play {
        track_ids: Vec<String>,
        #[serde(default)]
        start_at: u32,
    },
    /// Append these tracks to the queue.
    #[serde(rename_all = "camelCase")]
    Enqueue {
        track_ids: Vec<String>,
    },
    /// Insert these tracks after the current one.
    #[serde(rename_all = "camelCase")]
    PlayNext {
        track_ids: Vec<String>,
    },
    /// Take every queue entry for these tracks out of the queue.
    #[serde(rename_all = "camelCase")]
    Remove {
        track_ids: Vec<String>,
    },
    Clear,
    Radio {
        enabled: bool,
    },
    /// Pull what the server has changed: library, favourites and playlists.
    /// `full` walks every track rather than what changed.
    Sync {
        #[serde(default)]
        full: bool,
    },
    /// Delete the downloaded copies of these tracks, so the next play fetches
    /// them again: for a copy that was cached while the server's was bad.
    #[serde(rename_all = "camelCase")]
    Evict {
        track_ids: Vec<String>,
    },
    /// Play this track: from where it sits in the queue, or slotted in after
    /// the current one when the queue does not hold it.
    #[serde(rename_all = "camelCase")]
    JumpTo {
        track_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Seek {
        position_ms: u64,
    },
    Pause,
    Resume,
    Next,
    Previous,
}

impl LinkCommand {
    /// Every track id the command carries, for translating between the
    /// server's row ids and the uids it publishes.
    pub fn track_ids_mut(&mut self) -> Vec<&mut String> {
        match self {
            Self::Play { track_ids, .. }
            | Self::Enqueue { track_ids }
            | Self::PlayNext { track_ids }
            | Self::Remove { track_ids }
            | Self::Evict { track_ids } => track_ids.iter_mut().collect(),
            Self::JumpTo { track_id } => vec![track_id],
            Self::Clear
            | Self::Radio { .. }
            | Self::Sync { .. }
            | Self::Seek { .. }
            | Self::Pause
            | Self::Resume
            | Self::Next
            | Self::Previous => Vec::new(),
        }
    }
}

/// What a linked client tells the server about itself, as it changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkState {
    pub playing: bool,
    pub title: Option<String>,
    pub artist: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    /// Into the current track, as of when this was sent.
    #[serde(default)]
    pub position_ms: u64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub radio: bool,
    /// The queue, or the part of it around the current track when it is long.
    #[serde(default)]
    pub queue: Vec<LinkQueueEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkQueueEntry {
    /// The server's id for the track; `None` for a file only this device has.
    pub track_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub current: bool,
}

impl LinkState {
    /// Whether `self` says something `sent`, reported `elapsed` ago, did not.
    /// A playhead moving at one second per second is not news; a seek, a
    /// pause, another track or an edited queue is.
    pub fn differs(&self, sent: &LinkState, elapsed: Duration) -> bool {
        let strip = |s: &LinkState| LinkState {
            position_ms: 0,
            ..s.clone()
        };
        if strip(self) != strip(sent) {
            return true;
        }
        let expected = if sent.playing {
            sent.position_ms + elapsed.as_millis() as u64
        } else {
            sent.position_ms
        };
        self.position_ms.abs_diff(expected) > 3000
    }
}

/// A message from a client, up the same socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LinkReport {
    State(LinkState),
    /// Where Apple's push service reaches this device, so the server can wake
    /// it once iOS has suspended it and the socket is gone. `sandbox` for a
    /// development build, whose tokens only the sandbox gateway accepts.
    Push {
        token: String,
        sandbox: bool,
    },
}

/// This device's push token, once the OS has issued one. Set by the app; sent
/// up each link as it opens, and again if it changes.
static PUSH_TOKEN: Mutex<Option<(String, bool)>> = Mutex::new(None);

/// A command as a push notification carries it: the same JSON as over the
/// link.
pub fn parse_command(json: &str) -> Result<LinkCommand, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

/// Record the push token the OS issued this app, and link now to send it.
pub fn set_push_token(token: String, sandbox: bool) {
    *PUSH_TOKEN.lock() = Some((token, sandbox));
    nudge();
}

/// How a client describes itself when it links.
#[derive(Debug, Clone)]
pub struct LinkIdentity {
    /// Shown to whoever picks a client to play on: "James's iPhone".
    pub name: String,
    /// `ios`, `macos` or `linux`.
    pub platform: String,
    /// Stable across restarts, so a reconnect replaces its own entry on the
    /// server rather than listing the device twice.
    pub device_id: String,
}

impl LinkIdentity {
    /// This machine, named `name` or else by its hostname.
    pub fn this_device(name: Option<String>) -> Self {
        let (platform, label) = if cfg!(target_os = "ios") {
            ("ios", "iPhone")
        } else if cfg!(target_os = "macos") {
            ("macos", "Mac")
        } else {
            ("linux", "Linux")
        };
        Self {
            name: name
                .filter(|n| !n.trim().is_empty())
                .or_else(hostname)
                .unwrap_or_else(|| label.to_string()),
            platform: platform.to_string(),
            device_id: device_id(&config::config_dir()),
        }
    }
}

/// Keep a link open to the configured server for as long as the process runs,
/// handing each command to `on_command` on the link's own thread, and telling
/// the server what `state` says whenever it changes: which of a person's
/// devices is the one playing is how the server picks where to send music.
///
/// Reads the config before every attempt, so signing in later links without a
/// restart. A server that is not koan is checked once per sign-in and left
/// alone: Navidrome has no such endpoint.
pub fn spawn(
    identity: LinkIdentity,
    on_command: impl Fn(LinkCommand) + Send + 'static,
    state: impl Fn() -> LinkState + Send + 'static,
) {
    std::thread::Builder::new()
        .name("koan-link".into())
        .spawn(move || run(identity, on_command, state))
        .expect("failed to spawn the link thread");
}

const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);
/// A link that has heard nothing for this long pings, so a dead connection is
/// noticed rather than waited on forever.
const IDLE: Duration = Duration::from_secs(45);
/// How often the link looks at the player's state between messages.
const TICK: Duration = Duration::from_secs(3);

fn run(identity: LinkIdentity, on_command: impl Fn(LinkCommand), state: impl Fn() -> LinkState) {
    let mut wait = RETRY_MIN;
    // The credentials last found not to be a koan server.
    let mut not_koan: Option<SubsonicAuth> = None;
    loop {
        let cfg = Config::load().unwrap_or_default();
        let Some(auth) = subsonic_auth(&cfg) else {
            rest(RETRY_MAX);
            continue;
        };
        if not_koan.as_ref() == Some(&auth) {
            rest(RETRY_MAX);
            continue;
        }
        match subsonic_client(&cfg).map(|c| c.server_type()) {
            Some(Ok(Some(kind))) if kind == "koan" => {}
            Some(Ok(_)) => {
                log::info!("link: {} is not a koan server", auth.base_url);
                not_koan = Some(auth);
                continue;
            }
            _ => {
                rest(wait);
                wait = (wait * 2).min(RETRY_MAX);
                continue;
            }
        }

        match connect(&auth, &identity) {
            Ok(socket) => {
                log::info!("link: connected to {}", auth.base_url);
                wait = RETRY_MIN;
                if let Err(e) = serve(socket, &on_command, &state) {
                    log::info!("link: closed: {e}");
                }
            }
            Err(e) => log::warn!("link: {e}"),
        }
        if rest(wait) {
            wait = RETRY_MIN;
            continue;
        }
        wait = (wait * 2).min(RETRY_MAX);
    }
}

static NUDGE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Try to link again now, rather than when the backoff runs out.
///
/// For an app coming back to the foreground: iOS suspends a backgrounded app,
/// its link dies with it, and the retry it was sleeping towards can be a minute
/// away. Does nothing to a link that is up.
pub fn nudge() {
    *NUDGE.0.lock() = true;
    NUDGE.1.notify_all();
}

/// Wait `d`, or less if nudged. True when nudged.
fn rest(d: Duration) -> bool {
    let mut nudged = NUDGE.0.lock();
    if !*nudged {
        NUDGE.1.wait_for(&mut nudged, d);
    }
    std::mem::replace(&mut *nudged, false)
}

type Socket = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;

fn connect(auth: &SubsonicAuth, identity: &LinkIdentity) -> Result<Socket, String> {
    let url = link_url(auth, identity)?;
    let (socket, _) = tungstenite::connect(url).map_err(|e| e.to_string())?;
    let stream = match socket.get_ref() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => s.get_ref(),
        _ => return Ok(socket),
    };
    stream
        .set_read_timeout(Some(TICK))
        .map_err(|e| e.to_string())?;
    Ok(socket)
}

/// `/rest/koanLink` with the same credentials every other call carries.
fn link_url(auth: &SubsonicAuth, identity: &LinkIdentity) -> Result<String, String> {
    let base = if let Some(rest) = auth.base_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = auth.base_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        return Err(format!("not an http(s) server: {}", auth.base_url));
    };
    let mut query = auth.query().map_err(|e| e.to_string())?;
    for (k, v) in [
        ("client", identity.name.as_str()),
        ("platform", identity.platform.as_str()),
        ("device", identity.device_id.as_str()),
    ] {
        query.push('&');
        query.push_str(k);
        query.push('=');
        query.push_str(&percent_encode(v));
    }
    Ok(format!("{base}/rest/koanLink?{query}"))
}

fn serve(
    mut socket: Socket,
    on_command: &impl Fn(LinkCommand),
    state: &impl Fn() -> LinkState,
) -> Result<(), String> {
    let mut heard = Instant::now();
    let mut pinged = false;
    let mut sent: Option<(LinkState, Instant)> = None;
    let mut sent_push: Option<(String, bool)> = None;
    loop {
        let push = PUSH_TOKEN.lock().clone();
        if push.is_some() && push != sent_push {
            let (token, sandbox) = push.clone().unwrap_or_default();
            let text = serde_json::to_string(&LinkReport::Push { token, sandbox })
                .map_err(|e| e.to_string())?;
            socket
                .send(tungstenite::Message::Text(text.into()))
                .map_err(|e| e.to_string())?;
            sent_push = push;
        }
        let now = state();
        if sent
            .as_ref()
            .is_none_or(|(s, at)| now.differs(s, at.elapsed()))
        {
            let text = serde_json::to_string(&LinkReport::State(now.clone()))
                .map_err(|e| e.to_string())?;
            socket
                .send(tungstenite::Message::Text(text.into()))
                .map_err(|e| e.to_string())?;
            sent = Some((now, Instant::now()));
        }
        match socket.read() {
            Ok(tungstenite::Message::Text(text)) => {
                (heard, pinged) = (Instant::now(), false);
                match serde_json::from_str::<LinkCommand>(&text) {
                    Ok(cmd) => on_command(cmd),
                    Err(e) => log::warn!("link: not a command ({e}): {text}"),
                }
            }
            Ok(tungstenite::Message::Close(_)) => return Err("closed by the server".into()),
            Ok(_) => (heard, pinged) = (Instant::now(), false),
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if heard.elapsed() < IDLE {
                    continue;
                }
                if pinged {
                    return Err("no answer to a ping".into());
                }
                socket
                    .send(tungstenite::Message::Ping(Vec::new().into()))
                    .map_err(|e| e.to_string())?;
                (heard, pinged) = (Instant::now(), true);
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// This library's tracks for the server's ids, in the order given, and
/// whether a sync ran to find them.
///
/// A koan server names a track by its uid, which this library adopted when it
/// synced the track; another server by the id it issued. A server can name a
/// track added since the last sync; if any are missing, an incremental sync
/// runs first, and whatever is still missing after it is left out.
pub fn resolve_tracks(
    db: &crate::db::connection::Database,
    remote_ids: &[String],
) -> (Vec<i64>, bool) {
    let lookup = |db: &crate::db::connection::Database| {
        let mut stmt = db
            .conn
            .prepare_cached(
                "SELECT id FROM tracks WHERE uid = ?1
                 UNION ALL SELECT id FROM tracks WHERE remote_id = ?1 LIMIT 1",
            )
            .ok();
        remote_ids
            .iter()
            .map(|rid| {
                stmt.as_mut()
                    .and_then(|s| s.query_row([rid], |r| r.get::<_, i64>(0)).ok())
            })
            .collect::<Vec<_>>()
    };
    let found = lookup(db);
    if found.iter().all(Option::is_some) {
        return (found.into_iter().flatten().collect(), false);
    }
    sync(db, false);
    (lookup(db).into_iter().flatten().collect(), true)
}

/// A sync from the configured server, as the app runs its own: the library,
/// then favourites and playlists.
pub fn sync(db: &crate::db::connection::Database, full: bool) {
    let cfg = Config::load().unwrap_or_default();
    if let Some(client) = subsonic_client(&cfg)
        && let Err(e) = crate::helpers::sync_remote(
            db,
            &client,
            full,
            &cfg.remote.url,
            &cfg.remote.username,
            &|_| {},
        )
    {
        log::warn!("link: sync failed: {e}");
    }
}

/// A random id kept in the config directory.
fn device_id(dir: &Path) -> String {
    let path = dir.join("device-id");
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim();
        if !id.is_empty() {
            return id.to_string();
        }
    }
    let id = uuid::Uuid::now_v7().to_string();
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&path, &id);
    id
}

fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its whole length, and gethostname
    // writes at most that many bytes.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    if !ok {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]);
    let name = name.trim_end_matches(".local").trim();
    (!name.is_empty() && name != "localhost").then(|| name.to_string())
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_token_is_a_tagged_report() {
        let report = LinkReport::Push {
            token: "ab12".into(),
            sandbox: true,
        };
        let text = serde_json::to_string(&report).unwrap();
        assert_eq!(text, r#"{"type":"push","token":"ab12","sandbox":true}"#);
        assert_eq!(serde_json::from_str::<LinkReport>(&text).unwrap(), report);
    }

    #[test]
    fn commands_are_tagged_json() {
        let play = LinkCommand::Play {
            track_ids: vec!["12".into(), "34".into()],
            start_at: 1,
        };
        let json = serde_json::to_string(&play).unwrap();
        assert_eq!(
            json,
            r#"{"type":"play","trackIds":["12","34"],"startAt":1}"#
        );
        assert_eq!(serde_json::from_str::<LinkCommand>(&json).unwrap(), play);
        assert_eq!(
            serde_json::from_str::<LinkCommand>(r#"{"type":"pause"}"#).unwrap(),
            LinkCommand::Pause
        );
        let report = LinkReport::State(LinkState {
            playing: true,
            title: Some("Portions for Foxes".into()),
            ..Default::default()
        });
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.starts_with(r#"{"type":"state","playing":true,"title":"Portions for Foxes""#));
        assert_eq!(serde_json::from_str::<LinkReport>(&json).unwrap(), report);
    }

    #[test]
    fn a_playhead_moving_on_time_is_not_news() {
        let sent = LinkState {
            playing: true,
            position_ms: 10_000,
            ..Default::default()
        };
        let later = |pos| LinkState {
            position_ms: pos,
            ..sent.clone()
        };
        let five = Duration::from_secs(5);
        assert!(!later(15_000).differs(&sent, five));
        assert!(later(60_000).differs(&sent, five), "a seek");
        let paused = LinkState {
            playing: false,
            ..later(15_000)
        };
        assert!(paused.differs(&sent, five));
    }

    #[test]
    fn the_url_follows_the_scheme_and_names_the_device() {
        let identity = LinkIdentity {
            name: "J's iPhone".into(),
            platform: "ios".into(),
            device_id: "abc".into(),
        };
        let url = link_url(
            &SubsonicAuth::new("https://music.example.com", "j", "pw"),
            &identity,
        )
        .unwrap();
        assert!(url.starts_with("wss://music.example.com/rest/koanLink?"));
        assert!(url.contains("client=J%27s%20iPhone"));
        assert!(url.contains("device=abc"));
        assert!(
            link_url(&SubsonicAuth::new("http://h:4000", "j", "pw"), &identity)
                .unwrap()
                .starts_with("ws://h:4000/")
        );
    }

    #[test]
    fn the_device_id_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let first = device_id(dir.path());
        assert_eq!(device_id(dir.path()), first);
    }
}
