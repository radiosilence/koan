//! Favourites: the one way one is written, and reconciling them with the
//! server they came from.

use std::collections::HashSet;

use rusqlite::OptionalExtension;

use crate::config::Config;
use crate::db::connection::Database;
use crate::db::queries;
use crate::remote::client::{SubsonicClient, SubsonicError};

use super::*;

/// Make an artist, record or track a favourite of `user`'s (`star:
/// Some(true)`), stop it being one (`Some(false)`), or toggle it (`None`).
/// Returns the new state, or `None` when there is no such row.
///
/// Every front end writes favourites through this. The upstream server has
/// one account, the local user's, so only their favourites go to it: the
/// change is queued in the outbox in the transaction that writes the
/// favourite, so a sync importing the server's stars sees both or neither,
/// and is then sent on a thread of its own. A slow or unreachable server never
/// holds up the click; the outbox delivers what the send does not, and a
/// change the send delivered is marked so that no sync sends it again over a
/// newer change made elsewhere.
pub fn set_favourite(
    db: &Database,
    user: i64,
    kind: FavouriteKind,
    id: i64,
    star: Option<bool>,
) -> rusqlite::Result<Option<bool>> {
    let Some((now, queued)) = write_favourite(db, user, kind, id, star)? else {
        return Ok(None);
    };
    if let Some(change) = queued {
        send_soon(kind, change);
    }
    Ok(Some(now))
}

/// The write [`set_favourite`] makes, and the change it queued for the
/// server, if it queued one.
fn write_favourite(
    db: &Database,
    user: i64,
    kind: FavouriteKind,
    id: i64,
    star: Option<bool>,
) -> rusqlite::Result<Option<(bool, Option<queries::FavouriteChange>)>> {
    let conn = &db.conn;
    queries::atomically(conn, || {
        let table = match kind {
            FavouriteKind::Track => "tracks",
            FavouriteKind::Album => "albums",
            FavouriteKind::Artist => "artists",
        };
        let exists = conn
            .query_row(
                &format!("SELECT 1 FROM {table} WHERE id = ?1"),
                [id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Ok(None);
        }
        let now = match (kind, star) {
            (FavouriteKind::Track, None) => queries::toggle_favourite(conn, user, id)?,
            (FavouriteKind::Album, None) => queries::toggle_favourite_album(conn, user, id)?,
            (FavouriteKind::Artist, None) => queries::toggle_favourite_artist(conn, user, id)?,
            (FavouriteKind::Track, Some(true)) => {
                queries::add_favourite(conn, user, id)?;
                true
            }
            (FavouriteKind::Track, Some(false)) => {
                queries::remove_favourite(conn, user, id)?;
                false
            }
            (FavouriteKind::Album, Some(on)) => {
                queries::set_favourite_album(conn, user, id, on)?;
                on
            }
            (FavouriteKind::Artist, Some(on)) => {
                queries::set_favourite_artist(conn, user, id, on)?;
                on
            }
        };
        if !queries::auth::is_local_user(conn, user)? {
            return Ok(Some((now, None)));
        }
        let remote_id = match kind {
            FavouriteKind::Track => queries::track_remote_id(conn, id)?,
            FavouriteKind::Album => queries::album_remote_id(conn, id)?,
            FavouriteKind::Artist => queries::artist_remote_id(conn, id)?,
        };
        let queued = remote_id
            .map(|remote_id| queries::queue_favourite_change(conn, kind.as_str(), &remote_id, now))
            .transpose()?;
        Ok(Some((now, queued)))
    })
}

/// Send a change already in the outbox now, rather than at the next sync.
fn send_soon(kind: FavouriteKind, change: queries::FavouriteChange) {
    let cfg = Config::cached();
    if !cfg.remote.enabled {
        return;
    }
    let Some(client) = subsonic_client(&cfg) else {
        log::warn!("not syncing favourite: no usable server credentials");
        return;
    };
    std::thread::Builder::new()
        .name("koan-fav-sync".into())
        .spawn(move || match crate::db::pool::shared().get() {
            Ok(db) => deliver(&db, &client, kind, &change),
            Err(e) => log::warn!("favourite not sent; sent with the next sync: {e}"),
        })
        .ok();
}

/// Send one change from the outbox, and mark it sent if the server took it.
fn deliver(
    db: &Database,
    client: &SubsonicClient,
    kind: FavouriteKind,
    change: &queries::FavouriteChange,
) {
    match send_favourite(client, kind, &change.remote_id, change.star) {
        Ok(()) => {
            log::info!(
                "synced favourite to remote: {kind:?} {} = {}",
                change.remote_id,
                change.star
            );
            if let Err(e) = queries::mark_favourite_change_sent(&db.conn, change) {
                log::warn!("could not mark a sent favourite change: {e}");
            }
        }
        Err(e) => log::warn!("failed to sync favourite to remote: {e}"),
    }
}

fn send_favourite(
    client: &SubsonicClient,
    kind: FavouriteKind,
    remote_id: &str,
    star: bool,
) -> Result<(), SubsonicError> {
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
    pub removed: usize,
}

/// An item as the outbox and the server's recorded stars name it.
type Key = (String, String);

/// Reconcile favourites with the server, both directions.
///
/// Sends the favourites changed here first, then reads what the server has
/// starred and compares it with what it had starred at the last sync:
///
/// - a favourite here the server stopped starring was unstarred elsewhere,
///   and goes;
/// - one the server has never starred has not reached it yet (made before its
///   item was on the server, or before this device signed in), and is sent;
/// - whatever the server stars that is not a favourite here is imported.
///
/// A change made here that the server has not taken, or made while this ran,
/// stays in the outbox, and the server's stars neither add nor remove its
/// item: they may have been read before the change reached the server.
///
/// Covers albums and artists as well as tracks — `getStarred2` returns all
/// three from one request, and reading only songs would leave a starred album
/// invisible to koan.
pub fn reconcile_favourites(
    db: &Database,
    client: &SubsonicClient,
) -> rusqlite::Result<FavouriteSync> {
    let mut out = FavouriteSync::default();
    let mut before = queries::server_stars(&db.conn)?;

    for change in queries::favourite_changes(&db.conn)? {
        let kind = match change.kind.as_str() {
            "album" => FavouriteKind::Album,
            "artist" => FavouriteKind::Artist,
            _ => FavouriteKind::Track,
        };
        // What the server held once it took the change, which is what the
        // stars read below are compared with: a delivered star missing from
        // them was unstarred elsewhere.
        let delivered = |before: &mut HashSet<Key>| {
            let k = (change.kind.clone(), change.remote_id.clone());
            if change.star {
                before.insert(k);
            } else {
                before.remove(&k);
            }
        };
        // Sent when it was made, before the stars below are read.
        if change.sent {
            delivered(&mut before);
            queries::forget_favourite_change(&db.conn, &change)?;
            continue;
        }
        let sent = send_favourite(client, kind, &change.remote_id, change.star);
        match &sent {
            Ok(()) => {
                out.pushed += 1;
                delivered(&mut before);
            }
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
        queries::forget_favourite_change(&db.conn, &change)?;
    }

    let starred = match client.get_starred_all() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("could not fetch starred items from the server: {e}");
            return Ok(out);
        }
    };
    let songs: Vec<String> = starred.song.into_iter().map(|s| s.id).collect();
    let albums: Vec<String> = starred.album.into_iter().map(|a| a.id).collect();
    let artists: Vec<String> = starred.artist.into_iter().map(|a| a.id).collect();
    let key = |kind: FavouriteKind, id: &str| (kind.as_str().to_owned(), id.to_owned());
    let on_server: HashSet<Key> = [
        (FavouriteKind::Track, &songs),
        (FavouriteKind::Album, &albums),
        (FavouriteKind::Artist, &artists),
    ]
    .into_iter()
    .flat_map(|(kind, ids)| ids.iter().map(move |id| key(kind, id)))
    .collect();
    let pending = pending_changes(db)?;

    let mut sent: Vec<Key> = Vec::new();
    for (kind, _, remote_id) in local_favourites(db)? {
        let k = key(kind, &remote_id);
        if on_server.contains(&k) || before.contains(&k) || pending.contains(&k) {
            continue;
        }
        match send_favourite(client, kind, &remote_id, true) {
            Ok(()) => {
                out.pushed += 1;
                sent.push(k);
            }
            Err(e) => log::warn!("favourite not sent; tried again at the next sync: {e}"),
        }
    }

    // The outbox is read again inside the transaction: a change recorded
    // since the read above is newer than the server's stars, and one recorded
    // during the writes waits for them rather than being written over.
    let (imported, removed) = queries::atomically(&db.conn, || -> rusqlite::Result<_> {
        let pending = pending_changes(db)?;
        let user = queries::LOCAL_USER;
        let mut removed = 0;
        for (kind, row, remote_id) in local_favourites(db)? {
            let k = key(kind, &remote_id);
            if on_server.contains(&k) || pending.contains(&k) || !before.contains(&k) {
                continue;
            }
            match kind {
                FavouriteKind::Track => queries::remove_favourite(&db.conn, user, row)?,
                FavouriteKind::Album => queries::set_favourite_album(&db.conn, user, row, false)?,
                FavouriteKind::Artist => queries::set_favourite_artist(&db.conn, user, row, false)?,
            }
            removed += 1;
        }
        let settled = |kind: FavouriteKind, ids: &[String]| -> Vec<String> {
            ids.iter()
                .filter(|id| !pending.contains(&key(kind, id)))
                .cloned()
                .collect()
        };
        let imported = queries::import_remote_favourites(
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
        )?;
        queries::set_server_stars(
            &db.conn,
            on_server
                .iter()
                .chain(&sent)
                .map(|(kind, id)| (kind.as_str(), id.as_str())),
        )?;
        Ok((imported, removed))
    })?;
    out.imported = imported;
    out.removed = removed;
    Ok(out)
}

/// The items with a change in the outbox.
fn pending_changes(db: &Database) -> rusqlite::Result<HashSet<Key>> {
    Ok(queries::favourite_changes(&db.conn)?
        .into_iter()
        .map(|c| (c.kind, c.remote_id))
        .collect())
}

/// The local user's favourites the server knows, as (kind, row id, remote id).
fn local_favourites(db: &Database) -> rusqlite::Result<Vec<(FavouriteKind, i64, String)>> {
    let user = queries::LOCAL_USER;
    let tagged = |kind: FavouriteKind, rows: Vec<(i64, String)>| {
        rows.into_iter().map(move |(row, id)| (kind, row, id))
    };
    Ok(tagged(
        FavouriteKind::Track,
        queries::favourites_with_remote_id(&db.conn, user)?,
    )
    .chain(tagged(
        FavouriteKind::Album,
        queries::favourite_albums_with_remote_id(&db.conn, user)?,
    ))
    .chain(tagged(
        FavouriteKind::Artist,
        queries::favourite_artists_with_remote_id(&db.conn, user)?,
    ))
    .collect())
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

#[cfg(test)]
mod favourite_sync_tests {
    use super::*;
    use crate::auth::Role;
    use crate::db::queries::sample_meta;
    use std::sync::Arc;
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
        serve_on(stars, starred(&["s1"]), unstar)
    }

    fn starred(songs: &[&str]) -> Arc<Mutex<Vec<String>>> {
        Arc::new(Mutex::new(songs.iter().map(|s| s.to_string()).collect()))
    }

    /// A server whose starred songs are `songs`, which its star and unstar
    /// change, with album `a1` starred.
    fn serve_on(
        stars: Arc<Mutex<Vec<String>>>,
        songs: Arc<Mutex<Vec<String>>>,
        unstar: Unstar,
    ) -> String {
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
                let id = query
                    .split('&')
                    .filter_map(|kv| kv.split_once('='))
                    .find(|(k, _)| *k == "id")
                    .map(|(_, id)| id.to_string());
                let ok = r#"{"subsonic-response":{"status":"ok"}}"#.to_string();
                let body = match path.rsplit('/').next().unwrap() {
                    "getStarred2" => {
                        let songs = songs
                            .lock()
                            .unwrap()
                            .iter()
                            .map(|id| format!(r#"{{"id":"{id}","title":"{id}"}}"#))
                            .collect::<Vec<_>>()
                            .join(",");
                        format!(
                            r#"{{"subsonic-response":{{"status":"ok","starred2":{{"song":[{songs}],"album":[{{"id":"a1","name":"Album"}}]}}}}}}"#
                        )
                    }
                    "unstar" => match unstar {
                        Unstar::Taken => {
                            if let Some(id) = id {
                                songs.lock().unwrap().retain(|s| *s != id);
                            }
                            ok
                        }
                        Unstar::Unreached => continue,
                        Unstar::Refused => {
                            r#"{"subsonic-response":{"status":"failed","error":{"code":70,"message":"not found"}}}"#.to_string()
                        }
                    },
                    "star" => {
                        if let Some(id) = id {
                            stars.lock().unwrap().push(id.clone());
                            let mut songs = songs.lock().unwrap();
                            if !songs.contains(&id) {
                                songs.push(id);
                            }
                        }
                        ok
                    }
                    _ => ok,
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

    /// A track the server knows as `remote_id`, a favourite here.
    fn favourite_track(db: &Database, title: &str, remote_id: &str) -> i64 {
        let mut meta = sample_meta(title, "Artist", "Album");
        meta.path = Some(format!("/music/{title}.flac"));
        meta.remote_id = Some(remote_id.into());
        let id = queries::upsert_track(&db.conn, &meta).unwrap();
        queries::add_favourite(&db.conn, queries::LOCAL_USER, id).unwrap();
        id
    }

    fn is_favourite(db: &Database, track: i64) -> bool {
        queries::load_favourites(&db.conn, queries::LOCAL_USER)
            .unwrap()
            .contains(&track)
    }

    /// Unstarred on another device after this one saw it starred: the next
    /// sync takes it off here too.
    #[test]
    fn an_unstar_made_elsewhere_reaches_this_device() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let track = favourite_track(&db, "One", "s1");
        let songs = starred(&["s1"]);
        let url = serve_on(
            Arc::new(Mutex::new(Vec::new())),
            songs.clone(),
            Unstar::Taken,
        );
        let client = SubsonicClient::new(&url, "u", "pw");

        reconcile_favourites(&db, &client).unwrap();
        assert!(is_favourite(&db, track));

        songs.lock().unwrap().clear();
        let sync = reconcile_favourites(&db, &client).unwrap();
        assert_eq!(sync.removed, 1);
        assert_eq!(sync.pushed, 0, "starred back on the server");
        assert!(!is_favourite(&db, track));
        assert!(songs.lock().unwrap().is_empty());
    }

    /// Starred here and taken by the server at once, then unstarred on another
    /// device: the next sync here neither sends the star again nor keeps it.
    #[test]
    fn a_delivered_star_is_not_sent_again_over_an_unstar_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let track = favourite_track(&db, "Three", "s3");
        queries::remove_favourite(&db.conn, queries::LOCAL_USER, track).unwrap();
        let songs = starred(&[]);
        let stars = Arc::new(Mutex::new(Vec::new()));
        let url = serve_on(stars.clone(), songs.clone(), Unstar::Taken);
        let client = SubsonicClient::new(&url, "u", "pw");
        reconcile_favourites(&db, &client).unwrap();

        let (now, queued) = write_favourite(
            &db,
            queries::LOCAL_USER,
            FavouriteKind::Track,
            track,
            Some(true),
        )
        .unwrap()
        .unwrap();
        assert!(now);
        deliver(&db, &client, FavouriteKind::Track, &queued.unwrap());
        assert_eq!(*songs.lock().unwrap(), ["s3"]);

        songs.lock().unwrap().clear();
        let sync = reconcile_favourites(&db, &client).unwrap();
        assert_eq!(sync.pushed, 0);
        assert_eq!(*stars.lock().unwrap(), ["s3"], "the star was sent again");
        assert!(songs.lock().unwrap().is_empty());
        assert!(
            !is_favourite(&db, track),
            "the unstar made elsewhere was lost"
        );
        assert!(queries::favourite_changes(&db.conn).unwrap().is_empty());
    }

    /// Unfavourited here while the server was out of reach: the sync that
    /// cannot send it keeps it, and the next one sends it and keeps it off.
    #[test]
    fn an_unstar_made_offline_survives_until_sent() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let track = favourite_track(&db, "One", "s1");
        let songs = starred(&["s1"]);
        let stars = Arc::new(Mutex::new(Vec::new()));
        let reachable = SubsonicClient::new(
            &serve_on(stars.clone(), songs.clone(), Unstar::Taken),
            "u",
            "pw",
        );
        let unreachable = SubsonicClient::new(
            &serve_on(stars.clone(), songs.clone(), Unstar::Unreached),
            "u",
            "pw",
        );
        reconcile_favourites(&db, &reachable).unwrap();

        let written =
            write_favourite(&db, queries::LOCAL_USER, FavouriteKind::Track, track, None).unwrap();
        let (now, queued) = written.unwrap();
        assert!(!now);
        assert_eq!(queued.unwrap().remote_id, "s1");

        reconcile_favourites(&db, &unreachable).unwrap();
        assert!(
            !is_favourite(&db, track),
            "the server's stale star came back"
        );
        assert_eq!(queries::favourite_changes(&db.conn).unwrap().len(), 1);

        reconcile_favourites(&db, &reachable).unwrap();
        assert!(!is_favourite(&db, track));
        assert!(queries::favourite_changes(&db.conn).unwrap().is_empty());
        assert!(
            songs.lock().unwrap().is_empty(),
            "the unstar reached the server"
        );
        assert!(stars.lock().unwrap().is_empty(), "nothing was starred back");
    }

    /// Favourited on another device while this one already has the track.
    #[test]
    fn a_star_made_elsewhere_is_imported() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let track = favourite_track(&db, "Two", "s2");
        queries::remove_favourite(&db.conn, queries::LOCAL_USER, track).unwrap();
        let songs = starred(&[]);
        let url = serve_on(
            Arc::new(Mutex::new(Vec::new())),
            songs.clone(),
            Unstar::Taken,
        );
        let client = SubsonicClient::new(&url, "u", "pw");
        reconcile_favourites(&db, &client).unwrap();

        songs.lock().unwrap().push("s2".into());
        assert_eq!(reconcile_favourites(&db, &client).unwrap().imported, 1);
        assert!(is_favourite(&db, track));
    }

    /// Only the local user's favourites name the server's account.
    #[test]
    fn another_accounts_favourite_is_not_queued() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let track = favourite_track(&db, "One", "s1");
        queries::auth::create_user(&db.conn, "admin", "pw", Role::Admin).unwrap();
        let mate = queries::auth::create_user(&db.conn, "mate", "pw", Role::User).unwrap();
        let written = write_favourite(&db, mate, FavouriteKind::Track, track, Some(true)).unwrap();
        assert_eq!(written, Some((true, None)));
        assert!(queries::favourite_changes(&db.conn).unwrap().is_empty());
    }

    #[test]
    fn a_missing_row_is_not_favourited() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        for kind in [
            FavouriteKind::Track,
            FavouriteKind::Album,
            FavouriteKind::Artist,
        ] {
            assert_eq!(
                write_favourite(&db, queries::LOCAL_USER, kind, 404, Some(true)).unwrap(),
                None
            );
        }
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
        let sync = reconcile_favourites(&db, &SubsonicClient::new(&url, "u", "pw")).unwrap();
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
        reconcile_favourites(&db, &SubsonicClient::new(&url, "u", "pw")).unwrap();
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
        reconcile_favourites(&db, &SubsonicClient::new(&url, "u", "pw")).unwrap();
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
        let sync = reconcile_favourites(&db, &SubsonicClient::new(&url, "u", "pw")).unwrap();
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
