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
use std::time::Duration;

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
    Pause,
    Resume,
    Next,
    Previous,
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
/// handing each command to `on_command` on the link's own thread.
///
/// Reads the config before every attempt, so signing in later links without a
/// restart. A server that is not koan is checked once per sign-in and left
/// alone: Navidrome has no such endpoint.
pub fn spawn(identity: LinkIdentity, on_command: impl Fn(LinkCommand) + Send + 'static) {
    std::thread::Builder::new()
        .name("koan-link".into())
        .spawn(move || run(identity, on_command))
        .expect("failed to spawn the link thread");
}

const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);
/// A read that sees nothing for this long pings, so a dead connection is
/// noticed rather than waited on forever.
const IDLE: Duration = Duration::from_secs(45);

fn run(identity: LinkIdentity, on_command: impl Fn(LinkCommand)) {
    let mut wait = RETRY_MIN;
    // The credentials last found not to be a koan server.
    let mut not_koan: Option<SubsonicAuth> = None;
    loop {
        let cfg = Config::load().unwrap_or_default();
        let Some(auth) = subsonic_auth(&cfg) else {
            std::thread::sleep(RETRY_MAX);
            continue;
        };
        if not_koan.as_ref() == Some(&auth) {
            std::thread::sleep(RETRY_MAX);
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
                std::thread::sleep(wait);
                wait = (wait * 2).min(RETRY_MAX);
                continue;
            }
        }

        match connect(&auth, &identity) {
            Ok(socket) => {
                log::info!("link: connected to {}", auth.base_url);
                wait = RETRY_MIN;
                if let Err(e) = serve(socket, &on_command) {
                    log::info!("link: closed: {e}");
                }
            }
            Err(e) => log::warn!("link: {e}"),
        }
        std::thread::sleep(wait);
        wait = (wait * 2).min(RETRY_MAX);
    }
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
        .set_read_timeout(Some(IDLE))
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

fn serve(mut socket: Socket, on_command: &impl Fn(LinkCommand)) -> Result<(), String> {
    let mut pinged = false;
    loop {
        match socket.read() {
            Ok(tungstenite::Message::Text(text)) => {
                pinged = false;
                match serde_json::from_str::<LinkCommand>(&text) {
                    Ok(cmd) => on_command(cmd),
                    Err(e) => log::warn!("link: not a command ({e}): {text}"),
                }
            }
            Ok(tungstenite::Message::Close(_)) => return Err("closed by the server".into()),
            Ok(_) => pinged = false,
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if pinged {
                    return Err("no answer to a ping".into());
                }
                socket
                    .send(tungstenite::Message::Ping(Vec::new().into()))
                    .map_err(|e| e.to_string())?;
                pinged = true;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// This library's tracks for the server's ids, in the order given.
///
/// A server can name a track added since the last sync; if any are missing, an
/// incremental sync runs first, and whatever is still missing after it is left
/// out.
pub fn resolve_tracks(db: &crate::db::connection::Database, remote_ids: &[String]) -> Vec<i64> {
    let lookup = |db: &crate::db::connection::Database| {
        let mut stmt = db
            .conn
            .prepare_cached("SELECT id FROM tracks WHERE remote_id = ?1")
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
        return found.into_iter().flatten().collect();
    }
    let cfg = Config::load().unwrap_or_default();
    if let Some(client) = subsonic_client(&cfg)
        && let Err(e) = crate::remote::sync::sync_library(
            db,
            &client,
            false,
            &cfg.remote.url,
            &cfg.remote.username,
            &|_| {},
        )
    {
        log::warn!("link: sync before playing failed: {e}");
    }
    lookup(db).into_iter().flatten().collect()
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
