//! Favourites shared with the remote server: pushing a change, and reconciling the two sides.

use std::sync::Arc;

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::remote::client::{SubsonicClient, SubsonicError};

use super::*;

/// Push a favourite to the remote server, if this track came from one.
///
/// Fire and forget on its own thread: starring is a courtesy to the server, and
/// a slow or unreachable one should not hold up the click that caused it. The
/// local favourite is already written by the time this runs.
///
/// Silently does nothing for a track with no `remote_id` — including a local
/// file whose copy on the server failed to merge with it (#221), which is the
/// one case where the silence is wrong.
///
/// Shared by the TUI, the server and the app.
pub fn sync_favourite_to_remote(db: &Database, track_id: i64, star: bool) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let Ok(Some(remote_id)) = queries::track_remote_id(&db.conn, track_id) else {
        log::warn!("not syncing favourite: track {track_id} has no remote id");
        return;
    };
    push_favourite(db, &cfg, FavouriteKind::Track, remote_id, star);
}

/// Send a favourite changed here to the server now, and keep it in the
/// outbox until a sync has seen the server agree. The push is answered in a
/// moment; a sync already reading the server's stars is not, and would
/// otherwise put back what was just taken off.
fn push_favourite(db: &Database, cfg: &Config, kind: FavouriteKind, remote_id: String, star: bool) {
    if let Err(e) = queries::queue_favourite_change(&db.conn, kind.as_str(), &remote_id, star) {
        log::warn!("could not record a favourite change: {e}");
    }
    let Some(client) = subsonic_client(cfg) else {
        log::warn!("not syncing favourite: no usable server credentials");
        return;
    };
    std::thread::Builder::new()
        .name("koan-fav-sync".into())
        .spawn(
            move || match send_favourite(&client, kind, &remote_id, star) {
                Ok(()) => log::info!("synced favourite to remote: {kind:?} {remote_id} = {star}"),
                Err(e) => log::warn!("failed to sync favourite to remote: {e}"),
            },
        )
        .ok();
}

fn send_favourite(
    client: &SubsonicClient,
    kind: FavouriteKind,
    remote_id: &str,
    star: bool,
) -> Result<(), crate::remote::client::SubsonicError> {
    match (kind, star) {
        (FavouriteKind::Track, true) => client.star(remote_id),
        (FavouriteKind::Track, false) => client.unstar(remote_id),
        (FavouriteKind::Album, true) => client.star_album(remote_id),
        (FavouriteKind::Album, false) => client.unstar_album(remote_id),
        (FavouriteKind::Artist, true) => client.star_artist(remote_id),
        (FavouriteKind::Artist, false) => client.unstar_artist(remote_id),
    }
}

/// What a favourites reconciliation did.
#[derive(Debug, Default, Clone, Copy)]
pub struct FavouriteSync {
    pub pushed: usize,
    pub imported: usize,
}

/// Reconcile favourites with the server, both directions.
///
/// Sends the favourites changed here first, then stars every local favourite
/// the server knows about but has not starred, then imports everything the
/// server has starred. Union rather than mirror: the server records no
/// unstar, so treating it as authoritative would silently delete favourites
/// made here. Reading the server's stars first keeps a sync from re-sending
/// every favourite, one request each.
///
/// A change made here that the server has not taken, or made while this ran,
/// stays in the outbox, and the server's stars are not imported over it: they
/// may have been read before it reached the server.
///
/// Covers albums and artists as well as tracks — `getStarred2` returns all
/// three from one request, and reading only songs would leave a starred album
/// invisible to koan.
pub fn reconcile_favourites(db: &Database, client: &SubsonicClient) -> FavouriteSync {
    let mut out = FavouriteSync::default();

    for change in queries::favourite_changes(&db.conn).unwrap_or_default() {
        let kind = match change.kind.as_str() {
            "album" => FavouriteKind::Album,
            "artist" => FavouriteKind::Artist,
            _ => FavouriteKind::Track,
        };
        let sent = send_favourite(client, kind, &change.remote_id, change.star);
        match &sent {
            Ok(()) => out.pushed += 1,
            // Refused for the account: signing in again changes the answer.
            Err(SubsonicError::Api { code: 40..=44, .. }) => {
                log::warn!("favourite change refused for the account; kept for the next sync");
                continue;
            }
            // Answered and refused, such as an item the server no longer has:
            // asking again gets the same answer.
            Err(e @ SubsonicError::Api { .. }) => {
                log::warn!("favourite change refused by the server; dropped: {e}");
            }
            // Not reached.
            Err(e) => {
                log::warn!("favourite change not sent; kept for the next sync: {e}");
                continue;
            }
        }
        if let Err(e) = queries::forget_favourite_change(&db.conn, &change) {
            log::warn!("could not clear a sent favourite change: {e}");
        }
    }

    let starred = match client.get_starred_all() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("could not fetch starred items from the server: {e}");
            return out;
        }
    };
    let songs: Vec<String> = starred.song.into_iter().map(|s| s.id).collect();
    let albums: Vec<String> = starred.album.into_iter().map(|a| a.id).collect();
    let artists: Vec<String> = starred.artist.into_iter().map(|a| a.id).collect();

    let unstarred = |ids: Vec<String>, starred: &[String]| {
        let starred: std::collections::HashSet<&String> = starred.iter().collect();
        ids.into_iter()
            .filter(|id| !starred.contains(id))
            .collect::<Vec<_>>()
    };
    let tracks = queries::favourites_with_remote_id(&db.conn, queries::LOCAL_USER)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    for remote_id in unstarred(tracks, &songs) {
        if client.star(&remote_id).is_ok() {
            out.pushed += 1;
        }
    }
    let local_albums = queries::favourite_albums_with_remote_id(&db.conn, queries::LOCAL_USER)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    for remote_id in unstarred(local_albums, &albums) {
        if client.star_album(&remote_id).is_ok() {
            out.pushed += 1;
        }
    }
    let local_artists = queries::favourite_artists_with_remote_id(&db.conn, queries::LOCAL_USER)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    for remote_id in unstarred(local_artists, &artists) {
        if client.star_artist(&remote_id).is_ok() {
            out.pushed += 1;
        }
    }

    // Read after the stars were: anything here now is newer than they are.
    // The read and the imports are one transaction, so a change recorded
    // between them waits for the imports rather than being written over.
    let imported = queries::atomically(&db.conn, || -> rusqlite::Result<usize> {
        let changed: std::collections::HashSet<(String, String)> =
            queries::favourite_changes(&db.conn)?
                .into_iter()
                .map(|c| (c.kind, c.remote_id))
                .collect();
        let settled = |kind: FavouriteKind, ids: &[String]| -> Vec<String> {
            ids.iter()
                .filter(|id| !changed.contains(&(kind.as_str().to_owned(), (*id).clone())))
                .cloned()
                .collect()
        };
        let user = queries::LOCAL_USER;
        Ok(queries::import_remote_favourites(
            &db.conn,
            user,
            &settled(FavouriteKind::Track, &songs),
        )? + queries::import_remote_favourite_albums(
            &db.conn,
            user,
            &settled(FavouriteKind::Album, &albums),
        )? + queries::import_remote_favourite_artists(
            &db.conn,
            user,
            &settled(FavouriteKind::Artist, &artists),
        )?)
    });
    match imported {
        Ok(n) => out.imported += n,
        Err(e) => log::warn!("could not import the server's favourites: {e}"),
    }
    out
}

/// What a favourite applies to. Subsonic stars all three, under different
/// parameter names — passing an album id as `id` silently stars nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FavouriteKind {
    Track,
    Album,
    Artist,
}

impl FavouriteKind {
    /// How the outbox names it.
    fn as_str(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "album",
            Self::Artist => "artist",
        }
    }
}

/// Push an album or artist favourite to the server.
///
/// Same shape as [`sync_favourite_to_remote`], but the remote id comes from the
/// album or artist row rather than the track's path.
pub fn sync_collection_favourite_to_remote(
    db: &Database,
    kind: FavouriteKind,
    id: i64,
    star: bool,
) {
    let cfg = Config::load().unwrap_or_default();
    if !cfg.remote.enabled {
        return;
    }
    let remote_id = match kind {
        FavouriteKind::Album => queries::album_remote_id(&db.conn, id),
        FavouriteKind::Artist => queries::artist_remote_id(&db.conn, id),
        FavouriteKind::Track => return,
    };
    let Ok(Some(remote_id)) = remote_id else {
        log::warn!("not syncing favourite: {kind:?} {id} has no remote id");
        return;
    };
    push_favourite(db, &cfg, kind, remote_id, star);
}

#[cfg(test)]
mod favourite_sync_tests {
    use super::*;
    use crate::db::queries::sample_meta;
    use std::sync::Mutex;

    /// How the server answers an unstar.
    #[derive(Clone, Copy)]
    enum Unstar {
        Taken,
        /// The connection closes unanswered, as a server out of reach.
        Unreached,
        /// An error answer, as for an item the server does not have.
        Refused,
    }

    /// A server with song `s1` and album `a1` starred, recording every id it
    /// is asked to star.
    fn serve(stars: Arc<Mutex<Vec<String>>>) -> String {
        serve_with(stars, Unstar::Taken)
    }

    fn serve_with(stars: Arc<Mutex<Vec<String>>>, unstar: Unstar) -> String {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 2 {
                    line.clear();
                }
                let target = request.split_whitespace().nth(1).unwrap_or("");
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let body = match path.rsplit('/').next().unwrap() {
                    "getStarred2" => {
                        r#"{"subsonic-response":{"status":"ok","starred2":{"song":[{"id":"s1","title":"One"}],"album":[{"id":"a1","name":"Album"}]}}}"#
                    }
                    "unstar" => match unstar {
                        Unstar::Taken => r#"{"subsonic-response":{"status":"ok"}}"#,
                        Unstar::Unreached => continue,
                        Unstar::Refused => {
                            r#"{"subsonic-response":{"status":"failed","error":{"code":70,"message":"not found"}}}"#
                        }
                    },
                    "star" => {
                        if let Some((_, id)) = query
                            .split('&')
                            .filter_map(|kv| kv.split_once('='))
                            .find(|(k, _)| *k == "id")
                        {
                            stars.lock().unwrap().push(id.to_string());
                        }
                        r#"{"subsonic-response":{"status":"ok"}}"#
                    }
                    _ => r#"{"subsonic-response":{"status":"ok"}}"#,
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    #[test]
    fn only_favourites_the_server_lacks_are_starred() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        for (title, remote_id) in [("One", "s1"), ("Two", "s2")] {
            let mut meta = sample_meta(title, "Artist", "Album");
            meta.path = Some(format!("/music/{title}.flac"));
            meta.remote_id = Some(remote_id.into());
            let id = queries::upsert_track(&db.conn, &meta).unwrap();
            queries::add_favourite(&db.conn, queries::LOCAL_USER, id).unwrap();
        }

        let stars = Arc::new(Mutex::new(Vec::new()));
        let url = serve(stars.clone());
        let sync = reconcile_favourites(&db, &SubsonicClient::new(&url, "u", "pw"));
        assert_eq!(sync.pushed, 1);
        assert_eq!(*stars.lock().unwrap(), ["s2"]);
    }

    /// An album with remote id `a1`, not a favourite here.
    fn album(db: &Database) -> i64 {
        let mut meta = sample_meta("One", "Artist", "Album");
        meta.path = Some("/music/One.flac".into());
        let track = queries::upsert_track(&db.conn, &meta).unwrap();
        let album: i64 = db
            .conn
            .query_row("SELECT album_id FROM tracks WHERE id = ?1", [track], |r| {
                r.get(0)
            })
            .unwrap();
        db.conn
            .execute("UPDATE albums SET remote_id = 'a1' WHERE id = ?1", [album])
            .unwrap();
        album
    }

    /// Unfavourited here while the server still lists it: a sync that read
    /// the server's stars before the unstar reached it must not put it back.
    #[test]
    fn an_unstar_the_server_has_not_taken_is_not_undone() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let album = album(&db);
        queries::queue_favourite_change(&db.conn, "album", "a1", false).unwrap();

        let url = serve_with(Arc::new(Mutex::new(Vec::new())), Unstar::Unreached);
        reconcile_favourites(&db, &SubsonicClient::new(&url, "u", "pw"));
        let favourites = queries::favourite_album_id_set(&db.conn, queries::LOCAL_USER).unwrap();
        assert!(
            !favourites.contains(&album),
            "the server's stale star came back"
        );
        assert_eq!(
            queries::favourite_changes(&db.conn).unwrap().len(),
            1,
            "kept to send again"
        );
    }

    /// The server answered and said no: asking again gets the same answer,
    /// and a change kept for ever would hide the item from every import.
    #[test]
    fn a_change_the_server_refuses_leaves_the_outbox() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        queries::queue_favourite_change(&db.conn, "album", "gone", false).unwrap();

        let url = serve_with(Arc::new(Mutex::new(Vec::new())), Unstar::Refused);
        reconcile_favourites(&db, &SubsonicClient::new(&url, "u", "pw"));
        assert!(queries::favourite_changes(&db.conn).unwrap().is_empty());
    }

    /// A change sent while a newer one to the same item is made is cleared
    /// without taking the newer one with it.
    #[test]
    fn clearing_a_sent_change_keeps_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        queries::queue_favourite_change(&db.conn, "album", "a1", true).unwrap();
        let sent = queries::favourite_changes(&db.conn).unwrap().remove(0);
        queries::queue_favourite_change(&db.conn, "album", "a1", false).unwrap();
        queries::queue_favourite_change(&db.conn, "album", "a1", true).unwrap();
        queries::forget_favourite_change(&db.conn, &sent).unwrap();
        assert_eq!(queries::favourite_changes(&db.conn).unwrap().len(), 1);
    }

    /// The changes name the old server's items, which mean nothing to the next.
    #[test]
    fn forgetting_the_server_forgets_its_changes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        queries::queue_favourite_change(&db.conn, "album", "a1", false).unwrap();
        forget_remote(&db).unwrap();
        assert!(queries::favourite_changes(&db.conn).unwrap().is_empty());
    }

    #[test]
    fn a_change_the_server_takes_leaves_the_outbox() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        queries::queue_favourite_change(&db.conn, "track", "s2", true).unwrap();

        let stars = Arc::new(Mutex::new(Vec::new()));
        let url = serve(stars.clone());
        let sync = reconcile_favourites(&db, &SubsonicClient::new(&url, "u", "pw"));
        assert_eq!(sync.pushed, 1);
        assert_eq!(*stars.lock().unwrap(), ["s2"]);
        assert!(queries::favourite_changes(&db.conn).unwrap().is_empty());
    }

    /// A second change to the same item replaces the first.
    #[test]
    fn only_the_latest_change_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        queries::queue_favourite_change(&db.conn, "album", "a1", false).unwrap();
        queries::queue_favourite_change(&db.conn, "album", "a1", true).unwrap();
        let changes = queries::favourite_changes(&db.conn).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].star);
    }
}
