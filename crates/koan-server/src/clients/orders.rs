//! Standing orders: "when this album is in the library, queue it on my
//! device", or add it to a playlist, fulfilled by the scan that finds it.

use super::*;

/// "When this album is in the library, queue it on my device": a request made
/// before the album exists, fulfilled by the scan that finds it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Add to this playlist rather than a device's queue.
    #[serde(default)]
    pub playlist: Option<i64>,
    /// Only these tracks of the album (title substrings, in this order);
    /// empty for all of it.
    #[serde(default)]
    pub titles: Vec<String>,
    /// Unix seconds.
    pub created_at: i64,
}

/// An order nobody's scan has fulfilled in this long is dropped.
pub(super) const ORDER_TTL: i64 = 24 * 60 * 60;

impl Registry {
    pub fn add_order(&self, order: Order) {
        outbox::save_order(&order);
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
        let gone = orders.len() != before;
        if gone {
            outbox::drop_order(id);
        }
        gone
    }

    pub(super) fn done(&self, id: &str) {
        self.orders.lock().retain(|o| o.id != id);
        outbox::drop_order(id);
    }

    /// Send every order whose album the library now holds, and drop it.
    /// `find` answers an order with the album's track uids, in order.
    pub fn fulfil_orders(&self, find: impl Fn(&Order) -> Option<Vec<String>>) {
        let now = chrono::Utc::now().timestamp();
        let pending: Vec<Order> = {
            let mut orders = self.orders.lock();
            for o in orders.iter().filter(|o| now - o.created_at >= ORDER_TTL) {
                outbox::drop_order(&o.id);
            }
            orders.retain(|o| now - o.created_at < ORDER_TTL);
            orders.clone()
        };
        for order in pending.into_iter().filter(|o| o.playlist.is_none()) {
            let Some(ids) = find(&order).filter(|ids| !ids.is_empty()) else {
                continue;
            };
            let track_ids = ids;
            let cmd = if order.play_next {
                LinkCommand::PlayNext { track_ids }
            } else {
                LinkCommand::Enqueue {
                    track_ids,
                    play: false,
                }
            };
            match self.send(order.username.as_deref(), order.client.as_deref(), cmd) {
                Ok(c) => {
                    log::info!(
                        "link: {} — {} arrived; queued on {}",
                        order.artist,
                        order.album,
                        c.name
                    );
                    self.done(&order.id);
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
    let Ok(db) = koan_core::db::connection::Database::open_existing(db_path) else {
        return;
    };
    // Playlist orders are the server's own to carry out: add the tracks, and
    // the playlist reaches every device like any other edit.
    let for_playlists: Vec<Order> = registry
        .orders
        .lock()
        .iter()
        .filter(|o| o.playlist.is_some())
        .cloned()
        .collect();
    let mut edited = false;
    for order in for_playlists {
        let Some(playlist) = order.playlist else {
            continue;
        };
        let Some(ids) = order_tracks(&db.conn, &order) else {
            continue;
        };
        match koan_core::db::queries::add_tracks(&db.conn, playlist, &ids) {
            Ok(_) => {
                // Tests would push to whatever server this machine signs in to.
                if !cfg!(test) {
                    koan_core::playlists::push_to_remote(playlist);
                }
                log::info!(
                    "link: {} — {} arrived; added {} tracks to playlist {playlist}",
                    order.artist,
                    order.album,
                    ids.len()
                );
                registry.done(&order.id);
                edited = true;
            }
            Err(e) => log::warn!("link: could not add to playlist {playlist}: {e}"),
        }
    }
    if edited {
        changed();
    }
    registry.fulfil_orders(|order| {
        let rows = order_tracks(&db.conn, order)?;
        queries::uids_in_order(&db.conn, UidKind::Track, &rows).ok()
    });
}

/// The tracks an order asks for, once its album is in the library: all of
/// it, or the named ones in the order named. `None` until they are there. The
/// album is the newest whose artist and title contain the order's.
pub(super) fn order_tracks(conn: &rusqlite::Connection, order: &Order) -> Option<Vec<i64>> {
    let album = queries::newest_album_matching(conn, &order.artist, &order.album).ok()??;
    let tracks = queries::tracks_for_album(conn, album).ok()?;
    if order.titles.is_empty() {
        return Some(tracks.iter().map(|t| t.id).collect());
    }
    let picked: Vec<i64> = order
        .titles
        .iter()
        .filter_map(|want| {
            let want = want.to_lowercase();
            tracks
                .iter()
                .find(|t| t.title.to_lowercase().contains(&want))
                .map(|t| t.id)
        })
        .collect();
    (!picked.is_empty()).then_some(picked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playlist_order_adds_the_named_tracks_once_they_arrive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("koan.db");
        let db = koan_core::db::connection::Database::open(&path).unwrap();
        let playlist = koan_core::db::queries::create_playlist(
            &db.conn,
            koan_core::db::queries::LOCAL_USER,
            "cyberpunk",
            None,
        )
        .unwrap();
        let order = Order {
            id: "o1".into(),
            username: None,
            client: None,
            artist: "Perturbator".into(),
            album: "Dangerous Days".into(),
            play_next: false,
            playlist: Some(playlist),
            titles: vec!["Future Club".into()],
            created_at: chrono::Utc::now().timestamp(),
        };
        registry().add_order(order);

        // Not in the library yet: nothing happens, and the order waits.
        fulfil_from(&path);
        assert!(registry().orders(None).iter().any(|o| o.id == "o1"));

        db.conn
            .execute_batch(
                "INSERT INTO artists (id, name) VALUES (1, 'Perturbator');
                 INSERT INTO albums (id, title, artist_id) VALUES (1, 'Dangerous Days', 1);
                 INSERT INTO tracks (id, title, album_id, artist_id, track_number, path) VALUES
                   (1, 'Welcome Back', 1, 1, 1, '/1.flac'), (2, 'Future Club', 1, 1, 2, '/2.flac');",
            )
            .unwrap();
        fulfil_from(&path);
        let held: Vec<i64> = db
            .conn
            .prepare("SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1")
            .unwrap()
            .query_map([playlist], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(held, [2]);
        assert!(!registry().orders(None).iter().any(|o| o.id == "o1"));
    }
}
