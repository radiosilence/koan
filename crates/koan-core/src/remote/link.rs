//! A koan client's standing connection to the koan server it syncs from.
//!
//! The server can then act on the client: play a list of tracks on a phone, or
//! pause it. Subsonic has no way for a server to reach a client, so this is a
//! koan extension: a WebSocket at `/rest/koanLink`, authenticated like every
//! other `/rest` call, carrying [`LinkCommand`]s as JSON text frames from the
//! server. Track ids are the server's, which a synced client holds as each
//! track's `remote_id`.

use std::collections::HashMap;
use std::net::TcpStream;
use std::os::fd::RawFd;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use tungstenite::stream::MaybeTlsStream;

use crate::config::{self, Config};
use crate::helpers::{subsonic_auth, subsonic_client};
use crate::remote::client::SubsonicAuth;
use crate::remote::profile;
use crate::remote::wire::{self, Waker};

/// What a server asks a linked client to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LinkCommand {
    /// Replace the queue with these tracks and play from `start_at`,
    /// `position_ms` into it.
    #[serde(rename_all = "camelCase")]
    Play {
        track_ids: Vec<String>,
        #[serde(default)]
        start_at: u32,
        #[serde(default, skip_serializing_if = "is_zero")]
        position_ms: u64,
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
    /// Play this queue entry, by the id the device reported it under. Unlike
    /// `JumpTo` it names one entry of a track queued twice, and reaches a file
    /// only that device has.
    PlayItem {
        id: String,
    },
    RemoveItems {
        ids: Vec<String>,
    },
    /// Move these queue entries before or after `target`, in the order given.
    MoveItems {
        ids: Vec<String>,
        target: String,
        after: bool,
    },
    /// Insert these tracks after the queue entry `after`.
    #[serde(rename_all = "camelCase")]
    Insert {
        track_ids: Vec<String>,
        after: String,
    },
    Undo,
    Redo,
    /// Send this device's queue and playhead to the device `to`, as a `play`,
    /// and pause here. The device holding the queue does it, so taking music
    /// from another device and sending it there are one command.
    HandOff {
        to: String,
    },
    /// The account's other devices, as they are now. News rather than a
    /// command: sent whenever one of them changes, to links that asked for it.
    Devices {
        devices: Vec<LinkDevice>,
    },
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl LinkCommand {
    /// Whether a device on the same network, which may belong to anyone, may
    /// send this. Playback and the queue; nothing that touches the library or
    /// the files on disk.
    pub fn allowed_nearby(&self) -> bool {
        !matches!(
            self,
            Self::Sync { .. } | Self::Evict { .. } | Self::Devices { .. }
        )
    }

    /// The server's ids for the tracks this command names, if any.
    pub fn track_ids(&self) -> &[String] {
        match self {
            Self::Play { track_ids, .. }
            | Self::Enqueue { track_ids }
            | Self::PlayNext { track_ids }
            | Self::Remove { track_ids }
            | Self::Evict { track_ids }
            | Self::Insert { track_ids, .. } => track_ids,
            Self::JumpTo { track_id } => std::slice::from_ref(track_id),
            _ => &[],
        }
    }
}

/// Where a command came from, which decides what it may cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    /// The signed-in server, or this person's own devices through it.
    Account,
    /// A device on the same network, which may belong to anyone.
    Nearby,
}

/// Another device on the same account, as the server sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkDevice {
    /// The device's own id: stable across its reconnects.
    pub id: String,
    pub name: String,
    pub platform: String,
    /// Linked now. A device that is not is one iOS has suspended: a command
    /// wakes it, and music reaches it as a notification to tap.
    pub linked: bool,
    /// What it last reported, with the playhead placed as of sending. `None`
    /// until it has reported at all.
    pub state: Option<LinkState>,
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
            | Self::Evict { track_ids }
            | Self::Insert { track_ids, .. } => track_ids.iter_mut().collect(),
            Self::JumpTo { track_id } => vec![track_id],
            Self::PlayItem { .. }
            | Self::RemoveItems { .. }
            | Self::MoveItems { .. }
            | Self::Undo
            | Self::Redo
            | Self::HandOff { .. }
            | Self::Devices { .. }
            | Self::Clear
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
    /// The queue entry's own id on that device, for `playItem` and the rest.
    #[serde(default)]
    pub id: Option<String>,
    /// The server's id for the track; `None` for a file only this device has.
    pub track_id: Option<String>,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub duration_ms: u64,
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
    /// Send `command` to the device `to` on the same account.
    Command {
        to: String,
        command: LinkCommand,
    },
    /// Where to push updates to a Live Activity showing the device `device`;
    /// `None` for both when the activity has ended.
    Activity {
        token: Option<String>,
        device: Option<String>,
        #[serde(default)]
        sandbox: bool,
    },
    /// Who is at the other end of a connection made on the local network.
    Hello(LinkHello),
}

/// How a device introduces itself to one that connected to it over the local
/// network, before anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkHello {
    pub id: String,
    pub name: String,
    pub platform: String,
    /// Which library it plays from, as `library_fingerprint` gives it; `None`
    /// when it is signed in to none. Two devices with the same one share track
    /// ids, so music can be handed between them.
    pub library: Option<String>,
}

/// The server this client plays from, as something two devices can compare
/// without either saying its address to the network.
pub fn library_fingerprint(cfg: &Config) -> Option<String> {
    let auth = subsonic_auth(cfg)?;
    let url = auth.base_url.trim_end_matches('/').to_ascii_lowercase();
    Some(format!("{:x}", md5::compute(url.as_bytes())))
}

/// This device's push token, once the OS has issued one. Set by the app; sent
/// up each link as it opens, and again if it changes.
static PUSH_TOKEN: Mutex<Option<(String, bool)>> = Mutex::new(None);

/// A Live Activity on this device showing another: its push token, the device
/// it shows, and whether the token is the sandbox's. Sent up each link as it
/// opens, and again when it changes; `None` once the activity has ended.
type ActivityToken = (String, String, bool);
static ACTIVITY: Mutex<Option<Option<ActivityToken>>> = Mutex::new(None);

/// Record where the server should push updates to this device's Live
/// Activity, or that there is none now.
pub fn set_activity(activity: Option<ActivityToken>) {
    *ACTIVITY.lock() = Some(activity);
    if let Some(up) = LINK.lock().as_ref() {
        up.waker.wake();
    }
}

/// A command as a push notification carries it: the same JSON as over the
/// link.
pub fn parse_command(json: &str) -> Result<LinkCommand, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

/// Record the push token the OS issued this app, and link now to send it.
pub fn set_push_token(token: String, sandbox: bool) {
    *PUSH_TOKEN.lock() = Some((token, sandbox));
    nudge();
    if let Some(up) = LINK.lock().as_ref() {
        up.waker.wake();
    }
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

/// What this device offers whatever controls it: who it is, what it is doing,
/// and what to do with a command. The link to the server and the connections
/// made on the local network all serve the same one.
#[derive(Clone)]
pub struct Local {
    pub identity: LinkIdentity,
    pub state: Arc<dyn Fn() -> LinkState + Send + Sync>,
    pub on_command: Arc<dyn Fn(LinkCommand, CommandSource) + Send + Sync>,
}

/// Keep a link open to the configured server for as long as the process runs,
/// handing each command to `on_command` on the link's own thread, and telling
/// the server what `state` says whenever it changes: which of a person's
/// devices is the one playing is how the server picks where to send music.
///
/// Reads the config before every attempt, so signing in later links without a
/// restart. Links only to a server whose profile says it can: Navidrome has no
/// such endpoint.
pub fn spawn(local: Local) {
    std::thread::Builder::new()
        .name("koan-link".into())
        .spawn(move || run(local))
        .expect("failed to spawn the link thread");
}

const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);

fn run(local: Local) {
    let mut wait = RETRY_MIN;
    loop {
        crate::quiet::wait_until_awake();
        let cfg = Config::load().unwrap_or_default();
        let Some(auth) = subsonic_auth(&cfg) else {
            rest(RETRY_MAX);
            continue;
        };
        match profile::for_auth(&auth) {
            Some(p) if p.links() => {}
            // Not a server that links; asked again when the sign-in changes.
            Some(_) => {
                rest(RETRY_MAX);
                continue;
            }
            None => {
                rest(wait);
                wait = (wait * 2).min(RETRY_MAX);
                continue;
            }
        }

        match connect(&auth, &local.identity) {
            Ok((mut socket, fd)) => {
                log::info!("link: connected to {}", auth.base_url);
                // The server is answering: downloads waiting out an outage
                // against it need not wait for their backoff to find out.
                if let Some(client) = crate::helpers::subsonic_client(&cfg) {
                    client.outage().retry_now();
                }
                wait = RETRY_MIN;
                if let Err(e) = serve(&mut socket, fd, &local) {
                    log::info!("link: closed: {e}");
                }
                *LINK.lock() = None;
                crate::remote::devices::set_linked(false);
            }
            Err(e) => {
                log::warn!("link: {e}");
                // Asked again before the next attempt: the server may have
                // been replaced by one that does not link. Not after a link
                // that simply dropped, which is every time iOS suspends the
                // app: re-asking then would put two round trips in front of
                // every reconnect.
                profile::forget();
            }
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

/// Close the link, if it is up: the app has nothing to keep it open for.
pub fn hang_up() {
    if let Some(up) = LINK.lock().as_ref() {
        up.waker.wake();
    }
}

/// The link while it is up: what waits to go up it, and how to wake it.
struct Up {
    waker: Arc<Waker>,
    outbox: Vec<LinkReport>,
}

static LINK: Mutex<Option<Up>> = Mutex::new(None);

/// Send `report` up the link. False when the link is down.
pub fn report(report: LinkReport) -> bool {
    let mut link = LINK.lock();
    let Some(up) = link.as_mut() else {
        return false;
    };
    up.outbox.push(report);
    up.waker.wake();
    true
}

type Socket = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;

fn connect(auth: &SubsonicAuth, identity: &LinkIdentity) -> Result<(Socket, RawFd), String> {
    let url = link_url(auth, identity)?;
    let (socket, _) = tungstenite::connect(url).map_err(|e| e.to_string())?;
    let fd = wire::prepare(socket.get_ref())?;
    Ok((socket, fd))
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
        // Send this link the account's other devices. A server that predates
        // them ignores it.
        ("devices", "1"),
    ] {
        query.push('&');
        query.push_str(k);
        query.push('=');
        query.push_str(&percent_encode(v));
    }
    Ok(format!("{base}/rest/koanLink?{query}"))
}

fn serve(socket: &mut Socket, fd: RawFd, local: &Local) -> Result<(), String> {
    let waker = Waker::new().map_err(|e| e.to_string())?;
    wire::wake_on_engine_change(&waker);
    *LINK.lock() = Some(Up {
        waker: waker.clone(),
        outbox: Vec::new(),
    });
    crate::remote::devices::set_linked(true);
    let mut session = LinkSession {
        local,
        sent: None,
        sent_push: None,
        sent_activity: None,
    };
    wire::drive(socket, fd, &waker, &mut session)
}

struct LinkSession<'a> {
    local: &'a Local,
    sent: Option<(LinkState, Instant)>,
    sent_push: Option<(String, bool)>,
    sent_activity: Option<Option<ActivityToken>>,
}

impl wire::Session for LinkSession<'_> {
    fn outgoing(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        let push = PUSH_TOKEN.lock().clone();
        if let Some((token, sandbox)) = push.clone()
            && push != self.sent_push
        {
            out.push(LinkReport::Push { token, sandbox });
            self.sent_push = push;
        }
        let activity = ACTIVITY.lock().clone();
        if activity.is_some() && activity != self.sent_activity {
            let (token, device, sandbox) = match activity.clone().flatten() {
                Some((t, d, s)) => (Some(t), Some(d), s),
                None => (None, None, false),
            };
            out.push(LinkReport::Activity {
                token,
                device,
                sandbox,
            });
            self.sent_activity = activity;
        }
        if let Some(up) = LINK.lock().as_mut() {
            out.append(&mut up.outbox);
        }
        let now = (self.local.state)();
        if self
            .sent
            .as_ref()
            .is_none_or(|(s, at)| now.differs(s, at.elapsed()))
        {
            out.push(LinkReport::State(now.clone()));
            self.sent = Some((now, Instant::now()));
        }
        out.iter()
            .filter_map(|r| serde_json::to_string(r).ok())
            .collect()
    }

    fn incoming(&mut self, text: &str) {
        match serde_json::from_str::<LinkCommand>(text) {
            Ok(LinkCommand::Devices { devices }) => {
                crate::remote::devices::set_account(devices);
            }
            Ok(cmd) => (self.local.on_command)(cmd, CommandSource::Account),
            Err(e) => log::warn!("link: not a command ({e}): {text}"),
        }
    }

    fn done(&self) -> bool {
        !crate::quiet::awake()
    }
}

/// This library's tracks for the server's ids, in the order given, and
/// whether a sync ran to find them.
///
/// A koan server names a track by its uid, which this library adopted when it
/// synced the track; another server by the id it issued. A server can name a
/// track added since the last sync; if any are missing and `may_sync`, an
/// incremental sync runs first, and whatever is still missing after it is left
/// out. An id a sync already failed to find does not start another for a
/// while: a command naming a track deleted on the server would otherwise sync
/// every time it arrived.
pub fn resolve_tracks(
    db: &crate::db::connection::Database,
    remote_ids: &[String],
    may_sync: bool,
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
    let missing: Vec<&String> = remote_ids
        .iter()
        .zip(&found)
        .filter(|(_, f)| f.is_none())
        .map(|(id, _)| id)
        .collect();
    if missing.is_empty() || !may_sync || missing.iter().all(|id| recently_missed(id)) {
        return (found.into_iter().flatten().collect(), false);
    }
    sync(db, false);
    let found = lookup(db);
    let mut missed = MISSED.lock();
    let now = Instant::now();
    missed.retain(|_, at| now.duration_since(*at) < MISS_TTL);
    for (id, _) in remote_ids.iter().zip(&found).filter(|(_, f)| f.is_none()) {
        missed.insert(id.clone(), now);
    }
    (found.into_iter().flatten().collect(), true)
}

/// Ids a sync looked for and did not find, and when.
static MISSED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);
const MISS_TTL: Duration = Duration::from_secs(300);

fn recently_missed(id: &str) -> bool {
    MISSED
        .lock()
        .get(id)
        .is_some_and(|at| at.elapsed() < MISS_TTL)
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

    // Neither case may reach `sync`: a test has no business reading the
    // machine's config and syncing against the server it names.
    #[test]
    fn unknown_ids_sync_only_when_allowed_and_not_recently_missed() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::connection::Database::open(&dir.path().join("koan.db")).unwrap();

        let (found, synced) = resolve_tracks(&db, &["from-a-stranger".into()], false);
        assert!(found.is_empty());
        assert!(!synced, "a nearby peer's unknown id must not start a sync");

        MISSED
            .lock()
            .insert("deleted-on-server".into(), Instant::now());
        let (_, synced) = resolve_tracks(&db, &["deleted-on-server".into()], true);
        assert!(
            !synced,
            "an id a sync just failed to find does not start another"
        );
    }

    #[test]
    fn commands_name_their_tracks() {
        let play: LinkCommand =
            serde_json::from_str(r#"{"type":"play","trackIds":["a","b"]}"#).unwrap();
        assert_eq!(play.track_ids(), ["a", "b"]);
        let jump: LinkCommand = serde_json::from_str(r#"{"type":"jumpTo","trackId":"c"}"#).unwrap();
        assert_eq!(jump.track_ids(), ["c"]);
        assert!(LinkCommand::Pause.track_ids().is_empty());
    }

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
            position_ms: 0,
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
        assert!(url.contains("devices=1"));
        assert!(
            link_url(&SubsonicAuth::new("http://h:4000", "j", "pw"), &identity)
                .unwrap()
                .starts_with("ws://h:4000/")
        );
    }

    #[test]
    fn a_relayed_command_nests_the_command() {
        let report = LinkReport::Command {
            to: "phone".into(),
            command: LinkCommand::HandOff { to: "mac".into() },
        };
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(
            json,
            r#"{"type":"command","to":"phone","command":{"type":"handOff","to":"mac"}}"#
        );
        assert_eq!(serde_json::from_str::<LinkReport>(&json).unwrap(), report);
    }

    #[test]
    fn an_older_queue_entry_still_reads() {
        let e: LinkQueueEntry =
            serde_json::from_str(r#"{"trackId":"7","title":"t","artist":"a","current":true}"#)
                .unwrap();
        assert_eq!(e.id, None);
        assert_eq!(e.duration_ms, 0);
    }

    #[test]
    fn strangers_cannot_touch_the_library() {
        assert!(LinkCommand::Pause.allowed_nearby());
        assert!(LinkCommand::HandOff { to: "x".into() }.allowed_nearby());
        assert!(!LinkCommand::Sync { full: true }.allowed_nearby());
        assert!(!LinkCommand::Evict { track_ids: vec![] }.allowed_nearby());
    }

    #[test]
    fn the_device_id_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let first = device_id(dir.path());
        assert_eq!(device_id(dir.path()), first);
    }
}
