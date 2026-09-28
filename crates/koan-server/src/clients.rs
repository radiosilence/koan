//! koan clients linked to this server, and the way to reach them.
//!
//! A client that syncs from this server holds a WebSocket open at
//! `/rest/koanLink` (see `koan_core::remote::link`). Each is registered here
//! under the account it signed in as, so GraphQL (and MCP through it) can hand
//! one a list of tracks to play: build a playlist on the server, hear it on a
//! phone.

use std::sync::LazyLock;

use koan_core::remote::link::{LinkCommand, LinkState};
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;

/// A linked client, as listed.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub username: String,
    /// Unix seconds.
    pub connected_at: i64,
    /// What the client last said it was doing.
    pub state: LinkState,
    /// Unix seconds; when it was last seen playing, if ever since linking.
    pub last_played_at: Option<i64>,
    /// When `state` was reported, in Unix milliseconds.
    pub state_at: i64,
    /// Whether the client has reported its state at all. An app older than
    /// the reports never does, and its `state` then says nothing about it.
    pub reports: bool,
}

impl ClientInfo {
    /// Where the playhead is now, from where it was reported to be.
    pub fn position_ms(&self) -> u64 {
        let pos = self.state.position_ms;
        if !self.state.playing {
            return pos;
        }
        let run = (chrono::Utc::now().timestamp_millis() - self.state_at).max(0) as u64;
        let pos = pos + run;
        if self.state.duration_ms > 0 {
            pos.min(self.state.duration_ms)
        } else {
            pos
        }
    }
}

struct Entry {
    info: ClientInfo,
    /// The client's own id for itself, so a reconnect replaces its entry.
    device: String,
    tx: UnboundedSender<LinkCommand>,
}

/// "When this album is in the library, queue it on my device": a request made
/// before the album exists, fulfilled by the scan that finds it.
#[derive(Debug, Clone)]
pub struct Order {
    pub id: String,
    /// Whose devices it may go to.
    pub username: Option<String>,
    /// A client id or name; `None` for whichever `send` would pick then.
    pub client: Option<String>,
    pub artist: String,
    pub album: String,
    /// Insert after the current track rather than at the end.
    pub play_next: bool,
    /// Unix seconds.
    pub created_at: i64,
}

/// An order nobody's scan has fulfilled in this long is dropped.
const ORDER_TTL: i64 = 24 * 60 * 60;

#[derive(Default)]
pub struct Registry {
    entries: Mutex<Vec<Entry>>,
    orders: Mutex<Vec<Order>>,
}

/// One registry per process: the WebSocket route and the GraphQL schema are
/// built in different places and both need it.
pub fn registry() -> &'static Registry {
    static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::default);
    &REGISTRY
}

impl Registry {
    /// Add a client, replacing any earlier connection from the same device
    /// and account. Returns its id.
    pub fn register(
        &self,
        username: &str,
        name: &str,
        platform: &str,
        device: &str,
        tx: UnboundedSender<LinkCommand>,
    ) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        let mut entries = self.entries.lock();
        entries.retain(|e| !(e.device == device && e.info.username == username));
        entries.push(Entry {
            info: ClientInfo {
                id: id.clone(),
                name: name.to_string(),
                platform: platform.to_string(),
                username: username.to_string(),
                connected_at: chrono::Utc::now().timestamp(),
                state: LinkState::default(),
                last_played_at: None,
                state_at: chrono::Utc::now().timestamp_millis(),
                reports: false,
            },
            device: device.to_string(),
            tx,
        });
        id
    }

    /// Record what a client says it is doing.
    pub fn report(&self, id: &str, state: LinkState) {
        let mut entries = self.entries.lock();
        if let Some(e) = entries.iter_mut().find(|e| e.info.id == id) {
            if state.playing || e.info.state.playing {
                e.info.last_played_at = Some(chrono::Utc::now().timestamp());
            }
            e.info.state = state;
            e.info.state_at = chrono::Utc::now().timestamp_millis();
            e.info.reports = true;
        }
    }

    pub fn unregister(&self, id: &str) {
        self.entries.lock().retain(|e| e.info.id != id);
    }

    /// Clients `username` may command, newest first; every client for `None`.
    pub fn list(&self, username: Option<&str>) -> Vec<ClientInfo> {
        let mut out: Vec<ClientInfo> = self
            .entries
            .lock()
            .iter()
            .filter(|e| username.is_none_or(|u| e.info.username == u))
            .map(|e| e.info.clone())
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.connected_at));
        out
    }

    /// Send to `id` (an id or a name), or with none to the client the
    /// command most likely means: the one playing, else the one that played
    /// within `RECENT`, else the only one linked. `Err` names the choices when
    /// there is no telling, so whoever asked can ask the person.
    pub fn send(
        &self,
        username: Option<&str>,
        id: Option<&str>,
        cmd: LinkCommand,
    ) -> Result<ClientInfo, String> {
        let clients = self.list(username);
        let target = match id {
            Some(id) => clients
                .iter()
                .find(|c| c.id == id || c.name.eq_ignore_ascii_case(id))
                .ok_or_else(|| format!("no linked client {id}; see `clients`"))?,
            None => pick(&clients, chrono::Utc::now().timestamp())?,
        };
        let entries = self.entries.lock();
        let entry = entries
            .iter()
            .find(|e| e.info.id == target.id)
            .ok_or("that client has just gone")?;
        entry
            .tx
            .send(cmd)
            .map_err(|_| "that client has just gone".to_string())?;
        Ok(target.clone())
    }
}

impl Registry {
    pub fn add_order(&self, order: Order) {
        self.orders.lock().push(order);
    }

    pub fn orders(&self, username: Option<&str>) -> Vec<Order> {
        self.orders
            .lock()
            .iter()
            .filter(|o| username.is_none() || o.username.as_deref() == username)
            .cloned()
            .collect()
    }

    pub fn cancel_order(&self, username: Option<&str>, id: &str) -> bool {
        let mut orders = self.orders.lock();
        let before = orders.len();
        orders
            .retain(|o| !(o.id == id && (username.is_none() || o.username.as_deref() == username)));
        orders.len() != before
    }

    /// Send every order whose album the library now holds, and drop it.
    /// `find` answers an order with the album's track ids, in order.
    pub fn fulfil_orders(&self, find: impl Fn(&Order) -> Option<Vec<i64>>) {
        let now = chrono::Utc::now().timestamp();
        let pending: Vec<Order> = {
            let mut orders = self.orders.lock();
            orders.retain(|o| now - o.created_at < ORDER_TTL);
            orders.clone()
        };
        for order in pending {
            let Some(ids) = find(&order).filter(|ids| !ids.is_empty()) else {
                continue;
            };
            let track_ids = ids.iter().map(i64::to_string).collect();
            let cmd = if order.play_next {
                LinkCommand::PlayNext { track_ids }
            } else {
                LinkCommand::Enqueue { track_ids }
            };
            match self.send(order.username.as_deref(), order.client.as_deref(), cmd) {
                Ok(c) => {
                    log::info!(
                        "link: {} — {} arrived; queued on {}",
                        order.artist,
                        order.album,
                        c.name
                    );
                    self.orders.lock().retain(|o| o.id != order.id);
                }
                // No device to send to yet: kept, and tried after the next scan.
                Err(e) => log::info!("link: {} — {} arrived but {e}", order.artist, order.album),
            }
        }
    }
}

/// Fulfil standing orders against the library at `db_path`.
pub fn fulfil_from(db_path: &std::path::Path) {
    let registry = registry();
    if registry.orders.lock().is_empty() {
        return;
    }
    let Ok(db) = koan_core::db::connection::Database::open(db_path) else {
        return;
    };
    registry.fulfil_orders(|order| album_tracks(&db.conn, &order.artist, &order.album));
}

/// The newest album whose artist and title contain these, as track ids in
/// disc and track order.
pub fn album_tracks(conn: &rusqlite::Connection, artist: &str, album: &str) -> Option<Vec<i64>> {
    let like = |s: &str| format!("%{}%", s.replace(['%', '_'], ""));
    let album_id: i64 = conn
        .query_row(
            "SELECT al.id FROM albums al JOIN artists a ON a.id = al.artist_id
              WHERE a.name LIKE ?1 COLLATE NOCASE AND al.title LIKE ?2 COLLATE NOCASE
              ORDER BY al.id DESC LIMIT 1",
            [like(artist), like(album)],
            |r| r.get(0),
        )
        .ok()?;
    let mut stmt = conn
        .prepare("SELECT id FROM tracks WHERE album_id = ?1 ORDER BY disc, track_number, id")
        .ok()?;
    let ids = stmt
        .query_map([album_id], |r| r.get(0))
        .ok()?
        .filter_map(Result::ok)
        .collect();
    Some(ids)
}

/// How long ago a client can have stopped playing and still be the obvious
/// one to send music to.
const RECENT: i64 = 6 * 60 * 60;

fn pick(clients: &[ClientInfo], now: i64) -> Result<&ClientInfo, String> {
    if let Some(c) = clients.iter().find(|c| c.state.playing) {
        return Ok(c);
    }
    if let Some(c) = clients
        .iter()
        .filter(|c| c.last_played_at.is_some_and(|t| now - t < RECENT))
        .max_by_key(|c| c.last_played_at)
    {
        return Ok(c);
    }
    match clients {
        [] => Err("no koan app is linked to this server; open koan on the device".into()),
        [only] => Ok(only),
        several => Err(format!(
            "several koan apps are linked and none has played recently: {}. Ask which, then pass `client`",
            several
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reconnect_replaces_the_device_and_commands_reach_it() {
        let reg = Registry::default();
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let (tx3, _rx3) = tokio::sync::mpsc::unbounded_channel();
        reg.register("j", "phone", "ios", "dev-1", tx1);
        let id = reg.register("j", "phone", "ios", "dev-1", tx2);
        reg.register("someone", "laptop", "macos", "dev-2", tx3);

        assert_eq!(reg.list(Some("j")).len(), 1);
        assert_eq!(reg.list(None).len(), 2);

        let sent = reg.send(Some("j"), None, LinkCommand::Pause).unwrap();
        assert_eq!(sent.id, id);
        assert_eq!(rx2.try_recv().unwrap(), LinkCommand::Pause);

        // Another account's device is not this account's to command.
        assert!(
            reg.send(Some("j"), Some("laptop"), LinkCommand::Pause)
                .is_err()
        );

        reg.unregister(&id);
        assert!(reg.send(Some("j"), None, LinkCommand::Pause).is_err());
    }

    #[test]
    fn the_device_playing_is_the_one_meant() {
        let reg = Registry::default();
        let (tx1, _rx1) = tokio::sync::mpsc::unbounded_channel();
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
        let mac = reg.register("j", "mac", "macos", "dev-1", tx1);
        let phone = reg.register("j", "phone", "ios", "dev-2", tx2);

        // Two idle devices: no telling, so the caller is told to ask.
        let err = reg.send(Some("j"), None, LinkCommand::Pause).unwrap_err();
        assert!(err.contains("mac") && err.contains("phone"), "{err}");

        reg.report(
            &phone,
            LinkState {
                playing: true,
                ..Default::default()
            },
        );
        assert_eq!(
            reg.send(Some("j"), None, LinkCommand::Pause).unwrap().id,
            phone
        );
        assert_eq!(rx2.try_recv().unwrap(), LinkCommand::Pause);

        // Stopped a moment ago: still the one meant, over the Mac.
        reg.report(&phone, LinkState::default());
        assert_eq!(
            reg.send(Some("j"), None, LinkCommand::Pause).unwrap().id,
            phone
        );
        let _ = mac;
    }
}
