use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::db::connection::Database;
use crate::db::queries::{self, TrackMeta};
use crate::remote::client::{SubsonicAlbumFull, SubsonicClient};

use rayon::prelude::*;
use rusqlite::params;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("subsonic error: {0}")]
    Subsonic(#[from] super::client::SubsonicError),
    #[error("db error: {0}")]
    Db(#[from] crate::db::connection::DbError),
}

#[derive(Debug, Default)]
pub struct SyncResult {
    pub artists_synced: usize,
    pub albums_synced: usize,
    pub tracks_synced: usize,
    /// Albums whose details could not be fetched. Non-zero means `last_sync`
    /// was left where it was so the next sync picks them up again.
    pub albums_failed: usize,
}

impl SyncResult {
    /// Whether the run covered everything it set out to.
    pub fn is_complete(&self) -> bool {
        self.albums_failed == 0
    }
}

/// Get the last sync timestamp for a remote server, if any.
pub fn get_last_sync(
    db: &Database,
    url: &str,
) -> Result<Option<i64>, crate::db::connection::DbError> {
    let result = db.conn.query_row(
        "SELECT last_sync FROM remote_servers WHERE url = ?1",
        params![url],
        |row| row.get::<_, Option<i64>>(0),
    );
    match result {
        Ok(ts) => Ok(ts),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Update (or insert) the last sync timestamp for a remote server.
pub fn update_last_sync(
    db: &Database,
    url: &str,
    username: &str,
    timestamp: i64,
) -> Result<(), crate::db::connection::DbError> {
    db.conn.execute(
        "INSERT INTO remote_servers (url, username, last_sync)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(url) DO UPDATE SET last_sync = ?3",
        params![url, username, timestamp],
    )?;
    Ok(())
}

/// Parse an ISO 8601 / RFC 3339 timestamp string into a unix timestamp (seconds).
/// Returns `None` if the string can't be parsed.
///
/// Handles common Subsonic/Navidrome variants:
/// - Full RFC 3339: `2024-01-15T10:30:00Z`, `2024-01-15T10:30:00+05:30`
/// - Fractional seconds: `2024-01-15T10:30:00.123Z`
/// - Missing timezone (assumed UTC): `2024-01-15T10:30:00`
fn parse_iso8601_to_unix(s: &str) -> Option<i64> {
    use chrono::{DateTime, FixedOffset, NaiveDateTime};

    // Try strict RFC 3339 first (handles Z, offsets, fractional seconds).
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }

    // Subsonic sometimes omits timezone — parse as naive and assume UTC.
    // Try with fractional seconds first, then without.
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(naive.and_utc().timestamp());
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(naive.and_utc().timestamp());
    }

    // Some servers use space instead of T.
    if let Ok(dt) = DateTime::<FixedOffset>::parse_from_str(s, "%Y-%m-%d %H:%M:%S%:z") {
        return Some(dt.timestamp());
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(naive.and_utc().timestamp());
    }

    None
}

/// Pull the Navidrome/Subsonic library into the local DB.
///
/// The album list is always walked in `alphabeticalByName` order: it is the one
/// ordering stable under concurrent server-side inserts, so an offset walk can
/// never skip an album that was added between two pages. An incremental sync
/// walks the same list but only fetches details for albums created after
/// `last_sync` — the list pages are cheap, the per-album fetches are not.
///
/// `last_sync` only advances when every album fetch succeeded. A run that lost
/// albums to network errors leaves the timestamp alone so the next incremental
/// sync re-fetches them, rather than writing a permanent hole in the library.
///
/// Deduplication happens in `upsert_track` — if a local track already exists
/// with the same artist + album + title + track#, the remote_id and remote_url
/// are merged onto the existing row instead of creating a duplicate.
pub fn sync_library(
    db: &Database,
    client: &SubsonicClient,
    full: bool,
    server_url: &str,
    username: &str,
) -> Result<SyncResult, SyncError> {
    let mut result = SyncResult::default();

    let artists = client.get_artists()?;
    result.artists_synced = artists.len();
    log::info!("syncing {} artists from remote", artists.len());

    let last_sync = if full {
        None
    } else {
        get_last_sync(db, server_url)?
    };

    match last_sync {
        Some(ts) => log::info!("incremental sync (albums created after {})", ts),
        None => log::info!("full sync"),
    }

    let sync_start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut offset = 0u32;
    let page_size = 500u32;
    // Guards against an album appearing on two pages when the server-side list
    // shifts under the offset walk.
    let mut seen_ids: HashSet<String> = HashSet::new();

    loop {
        let page = client.get_album_list("alphabeticalByName", page_size, offset)?;
        if page.is_empty() {
            break;
        }
        let page_count = page.len();
        offset += page_count as u32;

        // Skip albums already handled this run, and — when incremental —
        // albums the server says predate the last sync. An unparseable
        // `created` is treated as new: re-fetching is cheap, missing is not.
        let to_fetch: Vec<String> = page
            .into_iter()
            .filter(|a| seen_ids.insert(a.id.clone()))
            .filter(|a| match last_sync {
                None => true,
                Some(ts) => a
                    .created
                    .as_deref()
                    .and_then(parse_iso8601_to_unix)
                    .is_none_or(|created| created >= ts),
            })
            .map(|a| a.id)
            .collect();

        if !to_fetch.is_empty() {
            let failures = AtomicUsize::new(0);
            let fetched: Vec<SubsonicAlbumFull> = to_fetch
                .into_par_iter()
                .filter_map(|id| match client.get_album(&id) {
                    Ok(full) => Some(full),
                    Err(e) => {
                        log::warn!("failed to fetch album {}: {}", id, e);
                        failures.fetch_add(1, Ordering::Relaxed);
                        None
                    }
                })
                .collect();
            result.albums_failed += failures.into_inner();

            write_albums(db, client, &fetched, &mut result)?;
        }

        log::info!(
            "synced {} albums ({} tracks) so far...",
            result.albums_synced,
            result.tracks_synced
        );

        if (page_count as u32) < page_size {
            break;
        }
    }

    if result.is_complete() {
        update_last_sync(db, server_url, username, sync_start)?;
    } else {
        log::warn!(
            "{} album(s) failed to fetch — leaving last_sync unchanged so the next sync retries them",
            result.albums_failed
        );
    }

    log::info!(
        "sync complete: {} artists, {} albums, {} tracks, {} failed",
        result.artists_synced,
        result.albums_synced,
        result.tracks_synced,
        result.albums_failed,
    );

    Ok(result)
}

/// Write one batch of fetched albums in a single transaction.
fn write_albums(
    db: &Database,
    client: &SubsonicClient,
    albums: &[SubsonicAlbumFull],
    result: &mut SyncResult,
) -> Result<(), SyncError> {
    db.conn
        .execute_batch("BEGIN")
        .map_err(crate::db::connection::DbError::from)?;

    for album in albums {
        result.albums_synced += 1;
        let artist_name = album.artist.as_deref().unwrap_or("Unknown Artist");

        for song in &album.song {
            let meta = TrackMeta {
                title: song.title.clone(),
                artist: song
                    .artist
                    .clone()
                    .unwrap_or_else(|| artist_name.to_string()),
                album_artist: album.artist.clone(),
                album: album.name.clone(),
                date: album.year.map(|y| y.to_string()),
                disc: song.disc_number,
                track_number: song.track,
                genre: song.genre.clone().or_else(|| album.genre.clone()),
                label: None,
                duration_ms: song.duration.map(|d| d * 1000),
                codec: song.suffix.clone(),
                sample_rate: None,
                bit_depth: None,
                channels: None,
                bitrate: song.bit_rate,
                size_bytes: None,
                mtime: None,
                path: None,
                source: "remote".to_string(),
                remote_id: Some(song.id.clone()),
                remote_url: Some(client.stream_url_template(&song.id)),
                album_added_at: album.created.clone(),
            };

            match queries::upsert_track(&db.conn, &meta) {
                Ok(_) => result.tracks_synced += 1,
                Err(e) => log::warn!("failed to insert remote track {}: {}", song.title, e),
            }
        }
    }

    db.conn
        .execute_batch("COMMIT")
        .map_err(crate::db::connection::DbError::from)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rfc3339_with_z() {
        // 2024-01-15T10:30:00Z = 1705314600
        assert_eq!(
            parse_iso8601_to_unix("2024-01-15T10:30:00Z"),
            Some(1705314600)
        );
    }

    #[test]
    fn parse_rfc3339_with_offset() {
        // 10:30 IST (+05:30) = 05:00 UTC = 1705294800
        assert_eq!(
            parse_iso8601_to_unix("2024-01-15T10:30:00+05:30"),
            Some(1705294800)
        );
    }

    #[test]
    fn parse_rfc3339_negative_offset() {
        // 10:30 EST (-05:00) = 15:30 UTC = 1705332600
        assert_eq!(
            parse_iso8601_to_unix("2024-01-15T10:30:00-05:00"),
            Some(1705332600)
        );
    }

    #[test]
    fn parse_fractional_seconds_z() {
        assert_eq!(
            parse_iso8601_to_unix("2024-01-15T10:30:00.123Z"),
            Some(1705314600)
        );
    }

    #[test]
    fn parse_fractional_seconds_offset() {
        assert_eq!(
            parse_iso8601_to_unix("2024-01-15T10:30:00.999+00:00"),
            Some(1705314600)
        );
    }

    #[test]
    fn parse_no_timezone_assumes_utc() {
        assert_eq!(
            parse_iso8601_to_unix("2024-01-15T10:30:00"),
            Some(1705314600)
        );
    }

    #[test]
    fn parse_no_timezone_fractional() {
        assert_eq!(
            parse_iso8601_to_unix("2024-01-15T10:30:00.500"),
            Some(1705314600)
        );
    }

    #[test]
    fn parse_space_separator_with_tz() {
        assert_eq!(
            parse_iso8601_to_unix("2024-01-15 10:30:00+00:00"),
            Some(1705314600)
        );
    }

    #[test]
    fn parse_space_separator_no_tz() {
        assert_eq!(
            parse_iso8601_to_unix("2024-01-15 10:30:00"),
            Some(1705314600)
        );
    }

    #[test]
    fn parse_garbage_returns_none() {
        assert_eq!(parse_iso8601_to_unix("not-a-date"), None);
        assert_eq!(parse_iso8601_to_unix(""), None);
        assert_eq!(parse_iso8601_to_unix("2024"), None);
    }

    #[test]
    fn parse_epoch() {
        assert_eq!(parse_iso8601_to_unix("1970-01-01T00:00:00Z"), Some(0));
    }

    // --- Sync → DB integration tests ---

    use crate::db::connection::Database;
    use crate::db::queries;
    use std::sync::{Arc, Mutex};

    fn test_db() -> (Database, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("sync_test.db")).unwrap();
        (db, dir)
    }

    /// Build a TrackMeta matching how sync_library constructs them from SubsonicSong data.
    fn remote_track_meta(remote_id: &str, title: &str, artist: &str, album: &str) -> TrackMeta {
        TrackMeta {
            title: title.into(),
            artist: artist.into(),
            album_artist: Some(artist.into()),
            album: album.into(),
            date: Some("2024".into()),
            disc: Some(1),
            track_number: Some(1),
            genre: Some("Electronic".into()),
            label: None,
            duration_ms: Some(240_000),
            codec: Some("FLAC".into()),
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: Some(1000),
            size_bytes: None,
            mtime: None,
            path: None,
            source: "remote".into(),
            remote_id: Some(remote_id.into()),
            remote_url: Some(format!("https://example.com/stream?id={}", remote_id)),
            album_added_at: None,
        }
    }

    #[test]
    fn sync_upserts_tracks_to_database() {
        let (db, _dir) = test_db();

        let meta = remote_track_meta("remote-001", "Vordhosbn", "Aphex Twin", "Drukqs");
        let track_id = queries::upsert_track(&db.conn, &meta).unwrap();
        assert!(track_id > 0, "upsert should return a valid track ID");

        // Verify the track exists with correct remote_id.
        let row = queries::get_track_row(&db.conn, track_id)
            .unwrap()
            .expect("track should exist in DB");
        assert_eq!(row.title, "Vordhosbn");
        assert_eq!(row.artist_name, "Aphex Twin");
        assert_eq!(row.album_title, "Drukqs");
        assert_eq!(row.remote_id.as_deref(), Some("remote-001"));
        assert_eq!(row.source, "remote");
    }

    // --- sync_library against a stub Subsonic server ---

    /// Minimal Subsonic server: serves getArtists, getAlbumList2 and getAlbum
    /// so `sync_library` can be driven end to end without a real Navidrome.
    struct StubServer {
        addr: std::net::SocketAddr,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
    }

    #[derive(Default)]
    struct StubState {
        /// Album (id, name, created) in the order the server lists them.
        albums: Mutex<Vec<(String, String, String)>>,
        /// Album ids whose getAlbum call fails with a 500.
        failing: Mutex<HashSet<String>>,
        /// Prepended to the album list once the first list page has been served,
        /// modelling a server-side insert landing mid-pagination.
        insert_after_first_page: Mutex<Option<(String, String, String)>>,
        list_pages_served: AtomicUsize,
        list_types: Mutex<Vec<String>>,
        album_calls: Mutex<Vec<String>>,
    }

    impl StubServer {
        fn start(state: Arc<StubState>) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let addr = listener.local_addr().unwrap();
            let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));

            let stop = shutdown.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            // BSD sockets inherit O_NONBLOCK from the listener.
                            let _ = stream.set_nonblocking(false);
                            let state = state.clone();
                            std::thread::spawn(move || handle(stream, state));
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self { addr, shutdown }
        }

        fn url(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    impl Drop for StubServer {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::Relaxed);
        }
    }

    /// Serve requests on one connection until the peer closes it. Keep-alive
    /// matters here: reqwest pools connections, and a server that hangs up after
    /// every response makes parallel fetches fail on reused sockets.
    fn handle(mut stream: std::net::TcpStream, state: Arc<StubState>) {
        use std::io::{BufRead, Write};

        let Ok(peek) = stream.try_clone() else { return };
        let mut reader = std::io::BufReader::new(peek);

        loop {
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                return;
            }
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line == "\r\n" || line == "\n" {
                    break;
                }
                line.clear();
            }

            let target = request_line.split_whitespace().nth(1).unwrap_or("/");
            let (path, query) = target.split_once('?').unwrap_or((target, ""));
            let params: std::collections::HashMap<&str, &str> = query
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .collect();

            let (status, body) = respond(&state, path, &params);

            let write = write!(
                stream,
                "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                status,
                body.len()
            )
            .and_then(|()| stream.write_all(body.as_bytes()))
            .and_then(|()| stream.flush());
            if write.is_err() {
                return;
            }
        }
    }

    fn respond(
        state: &Arc<StubState>,
        path: &str,
        params: &std::collections::HashMap<&str, &str>,
    ) -> (u16, String) {
        match path.rsplit('/').next().unwrap_or("") {
            "getArtists" => (
                200,
                r#"{"subsonic-response":{"status":"ok","artists":{"index":[{"artist":[{"id":"ar1","name":"Stub Artist"}]}]}}}"#.to_string(),
            ),
            "getAlbumList2" => {
                state
                    .list_types
                    .lock()
                    .unwrap()
                    .push(params.get("type").copied().unwrap_or("").to_string());
                let offset: usize = params.get("offset").and_then(|o| o.parse().ok()).unwrap_or(0);
                let size: usize = params.get("size").and_then(|s| s.parse().ok()).unwrap_or(500);

                let albums = state.albums.lock().unwrap();
                let slice: Vec<String> = albums
                    .iter()
                    .skip(offset)
                    .take(size)
                    .map(|(id, name, created)| {
                        format!(
                            r#"{{"id":"{}","name":"{}","artist":"Stub Artist","created":"{}"}}"#,
                            id, name, created
                        )
                    })
                    .collect();
                drop(albums);

                if state.list_pages_served.fetch_add(1, Ordering::SeqCst) == 0
                    && let Some(new_album) = state.insert_after_first_page.lock().unwrap().take()
                {
                    state.albums.lock().unwrap().insert(0, new_album);
                }

                (
                    200,
                    format!(
                        r#"{{"subsonic-response":{{"status":"ok","albumList2":{{"album":[{}]}}}}}}"#,
                        slice.join(",")
                    ),
                )
            }
            "getAlbum" => {
                let id = params.get("id").copied().unwrap_or("");
                state.album_calls.lock().unwrap().push(id.to_string());
                if state.failing.lock().unwrap().contains(id) {
                    (500, r#"{"error":"boom"}"#.to_string())
                } else {
                    (
                        200,
                        format!(
                            r#"{{"subsonic-response":{{"status":"ok","album":{{"id":"{id}","name":"Album {id}","artist":"Stub Artist","song":[{{"id":"s{id}","title":"Song {id}","track":1,"suffix":"flac"}}]}}}}}}"#
                        ),
                    )
                }
            }
            _ => (404, "{}".to_string()),
        }
    }

    fn stub_albums(n: usize) -> Vec<(String, String, String)> {
        (0..n)
            .map(|i| {
                (
                    format!("a{:04}", i),
                    format!("Album {:04}", i),
                    "2024-01-15T10:30:00Z".to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn failed_album_fetch_does_not_advance_last_sync_and_next_sync_retries() {
        let (db, _dir) = test_db();
        let state = Arc::new(StubState {
            albums: Mutex::new(stub_albums(4)),
            failing: Mutex::new(["a0002".to_string()].into_iter().collect()),
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");

        let first = sync_library(&db, &client, false, &server.url(), "u").unwrap();
        assert_eq!(first.albums_failed, 1, "the failing album must be counted");
        assert_eq!(first.albums_synced, 3);
        assert!(!first.is_complete());
        assert_eq!(
            get_last_sync(&db, &server.url()).unwrap(),
            None,
            "an incomplete sync must not advance last_sync"
        );

        // Second run: the album now succeeds and is picked up because the sync
        // still has no watermark to skip past.
        state.failing.lock().unwrap().clear();
        state.album_calls.lock().unwrap().clear();
        let second = sync_library(&db, &client, false, &server.url(), "u").unwrap();

        assert!(
            state
                .album_calls
                .lock()
                .unwrap()
                .contains(&"a0002".to_string()),
            "the previously failed album must be retried"
        );
        assert_eq!(second.albums_failed, 0);
        assert!(second.is_complete());
        assert!(
            get_last_sync(&db, &server.url()).unwrap().is_some(),
            "a clean sync advances last_sync"
        );
    }

    #[test]
    fn album_inserted_mid_pagination_is_not_fetched_twice_or_skipped() {
        // 600 albums forces a second list page; a server-side insert between
        // pages shifts the offset window, which without de-dup replays the
        // page boundary and, with `newest` ordering, can drop albums entirely.
        let (db, _dir) = test_db();
        let state = Arc::new(StubState {
            albums: Mutex::new(stub_albums(600)),
            insert_after_first_page: Mutex::new(Some((
                "aNEW".to_string(),
                "AAA Brand New".to_string(),
                "2024-06-01T00:00:00Z".to_string(),
            ))),
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");

        let result = sync_library(&db, &client, true, &server.url(), "u").unwrap();
        assert_eq!(result.albums_failed, 0);

        let calls = state.album_calls.lock().unwrap().clone();
        let unique: HashSet<&String> = calls.iter().collect();
        assert_eq!(
            calls.len(),
            unique.len(),
            "no album may be fetched twice after the window shifts"
        );

        // Every album present before the shift must still have been fetched.
        for i in 0..600 {
            let id = format!("a{:04}", i);
            assert!(unique.contains(&id), "album {} was skipped", id);
        }

        let types = state.list_types.lock().unwrap().clone();
        assert!(
            types.iter().all(|t| t == "alphabeticalByName"),
            "the paginated walk must use a stable ordering, got {:?}",
            types
        );
    }

    #[test]
    fn incremental_sync_only_fetches_albums_created_after_last_sync() {
        let (db, _dir) = test_db();
        let mut albums = stub_albums(3);
        albums[0].2 = "2020-01-01T00:00:00Z".into();
        albums[1].2 = "2020-01-01T00:00:00Z".into();
        albums[2].2 = "2030-01-01T00:00:00Z".into();

        let state = Arc::new(StubState {
            albums: Mutex::new(albums),
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");

        // Watermark between the two vintages.
        let watermark = parse_iso8601_to_unix("2025-01-01T00:00:00Z").unwrap();
        update_last_sync(&db, &server.url(), "u", watermark).unwrap();

        let result = sync_library(&db, &client, false, &server.url(), "u").unwrap();

        assert_eq!(result.albums_synced, 1, "only the new album needs fetching");
        assert_eq!(
            *state.album_calls.lock().unwrap(),
            vec!["a0002".to_string()]
        );
    }

    #[test]
    fn album_with_unparseable_created_is_always_fetched() {
        let (db, _dir) = test_db();
        let state = Arc::new(StubState {
            albums: Mutex::new(vec![("a0000".into(), "Album".into(), "who knows".into())]),
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");

        update_last_sync(&db, &server.url(), "u", 4_000_000_000).unwrap();
        let result = sync_library(&db, &client, false, &server.url(), "u").unwrap();

        assert_eq!(
            result.albums_synced, 1,
            "an album with no usable timestamp must not be assumed old"
        );
    }

    #[test]
    fn sync_deduplicates_by_remote_id() {
        let (db, _dir) = test_db();

        // First upsert.
        let meta1 = remote_track_meta("remote-dup", "Original Title", "Artist A", "Album X");
        let id1 = queries::upsert_track(&db.conn, &meta1).unwrap();

        // Second upsert with same remote_id but different metadata.
        let meta2 = remote_track_meta("remote-dup", "Updated Title", "Artist A", "Album X");
        let id2 = queries::upsert_track(&db.conn, &meta2).unwrap();

        // Should be the same row (dedup by remote_id).
        assert_eq!(id1, id2, "same remote_id should resolve to same track row");

        // Verify the metadata was updated.
        let row = queries::get_track_row(&db.conn, id2)
            .unwrap()
            .expect("track should exist");
        assert_eq!(row.title, "Updated Title");
        assert_eq!(row.remote_id.as_deref(), Some("remote-dup"));

        // Verify only one track exists.
        let stats = queries::library_stats(&db.conn).unwrap();
        assert_eq!(
            stats.total_tracks, 1,
            "should have exactly 1 track after dedup"
        );
    }
}
