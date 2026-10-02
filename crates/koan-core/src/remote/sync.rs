use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::db::connection::Database;
use crate::db::queries::{self, TrackMeta};
use crate::remote::client::{
    SubsonicAlbum, SubsonicAlbumFull, SubsonicArtist, SubsonicClient, SubsonicError, SubsonicSong,
};

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
    /// Pages of songs that could not be fetched, on the bulk path. Counts
    /// against completeness the same way.
    pub pages_failed: usize,
    /// Tracks the server listed that could not be written. Counts against
    /// completeness the same way: until they are written, an incremental sync
    /// must not move past them.
    pub tracks_failed: usize,
    /// Tracks removed because the server no longer has them.
    pub tracks_removed: usize,
}

impl SyncResult {
    /// Whether the run covered everything it set out to.
    pub fn is_complete(&self) -> bool {
        self.albums_failed == 0 && self.pages_failed == 0 && self.tracks_failed == 0
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

/// Which part of a sync is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPhase {
    /// Paging through the server's album list.
    Albums,
    /// Fetching and writing tracks. The long part.
    Tracks,
    /// Recording artist metadata.
    Artists,
    /// Relinking, recording the watermark, optimising the database.
    Finishing,
}

/// How far a sync has got. `done` and `total` count albums in the `Albums`
/// phase and tracks in the `Tracks` phase; `total` is `None` where the server
/// gives no way to know it in advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncProgress {
    pub phase: SyncPhase,
    pub done: u64,
    pub total: Option<u64>,
}

/// Albums and songs per list request. 500 is the most `getAlbumList2` allows.
const PAGE_SIZE: u32 = 500;

/// Song pages in flight at once during a full sync. The walk is bound by
/// round trips, not bandwidth; four hides most of a mobile link's latency
/// without asking much of the server.
const FETCH_LANES: usize = 4;

/// Attempts at one song page before it counts as failed.
const PAGE_ATTEMPTS: u32 = 3;

/// Changed albums above which an incremental sync pages every song rather
/// than fetching each album: about a hundred requests for the whole library,
/// against one per album.
const PAGED_ABOVE: usize = 200;

/// Pull the Navidrome/Subsonic library into the local DB.
///
/// The album list is always walked in `alphabeticalByName` order: it is the one
/// ordering stable under concurrent server-side inserts, so an offset walk can
/// never skip an album that was added between two pages.
///
/// A first or full sync then pages through every song with an empty `search3`
/// query — about a hundred requests for fifty thousand tracks — and joins them
/// to the album list. Fetching each album on its own costs one round trip per
/// album, which on a phone is minutes. A server that does not answer an empty
/// query gets the per-album walk instead, as does an incremental sync, which
/// only fetches albums created after `last_sync` and so has few to fetch.
///
/// `last_sync` only advances when everything was fetched. A run that lost
/// albums or pages to network errors leaves the timestamp alone so the next
/// sync fetches them again, rather than writing a permanent hole in the library.
///
/// Deduplication happens in `upsert_track`, which merges a server's copy of a
/// track onto the local row for it instead of creating a duplicate.
pub fn sync_library(
    db: &Database,
    client: &SubsonicClient,
    full: bool,
    server_url: &str,
    username: &str,
    progress: &(dyn Fn(SyncProgress) + Sync),
) -> Result<SyncResult, SyncError> {
    let mut result = SyncResult::default();

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

    let albums = list_albums(client, progress)?;

    // An incremental sync fetches albums created since the last one, and any
    // the client holds differently from how the server lists them: a retag
    // on the server keeps an album's `created` and can give it a new id, and
    // neither would otherwise ever be read again. An unparseable `created` is
    // treated as new: re-fetching is cheap, missing is not.
    let held = last_sync.and_then(|_| held_albums(db));
    let wanted: Vec<&SubsonicAlbum> = albums
        .iter()
        .filter(|a| match last_sync {
            None => true,
            Some(ts) => {
                a.created
                    .as_deref()
                    .and_then(parse_iso8601_to_unix)
                    .is_none_or(|created| created >= ts)
                    || held.as_ref().is_some_and(|h| differs(a, h.get(&a.id)))
            }
        })
        .collect();
    let expected: u64 = wanted
        .iter()
        .filter_map(|a| a.song_count)
        .map(|n| n.max(0) as u64)
        .sum();

    let mut song_ids: HashSet<String> = HashSet::new();
    let mut total = (expected > 0).then_some(expected);
    let walked = if (last_sync.is_none() || wanted.len() > PAGED_ABOVE) && !wanted.is_empty() {
        if total.is_none() {
            total = client.song_count().ok().flatten();
        }
        sync_all_songs(
            db,
            client,
            &albums,
            total,
            &mut result,
            &mut song_ids,
            progress,
        )?
    } else {
        false
    };
    if !walked {
        sync_by_album(
            db,
            client,
            &wanted,
            total,
            &mut result,
            &mut song_ids,
            progress,
        )?;
    }

    // Artist rows are created by track upserts, which carry no MusicBrainz id
    // or sort name. Applied last, because the rows do not exist until their
    // tracks have been written.
    let artists = client.get_artists()?;
    result.artists_synced = artists.len();
    progress(SyncProgress {
        phase: SyncPhase::Artists,
        done: 0,
        total: Some(artists.len() as u64),
    });
    write_artists(db, &artists, &mut result);

    progress(SyncProgress {
        phase: SyncPhase::Finishing,
        done: 0,
        total: None,
    });

    // An empty listing is far likelier to be a server fault than an empty
    // library, and would unlink every file. So is one shorter than the count
    // the server gave: a track deleted mid-walk shifts a later page by one, and
    // the track it pushes out of view is not gone.
    let listed_everything = total.is_none_or(|n| song_ids.len() as u64 >= n);
    if full && result.is_complete() && !song_ids.is_empty() && listed_everything {
        match queries::relink_vanished_remote_ids(&db.conn, &song_ids) {
            Ok(0) => {}
            Ok(n) => log::info!("{n} files had ids the server no longer knows; relinked"),
            Err(e) => log::warn!("failed to relink tracks with vanished remote ids: {e}"),
        }
    }

    // What the server deleted goes here too. Every sync lists every album, so
    // an album gone from that list goes on any sync; a single track deleted
    // from an album only shows on a full one, which lists every track. Both
    // are held to the same guards as above: an empty or short listing is a
    // fault, not a deletion.
    let mut live_albums: std::collections::HashSet<String> =
        albums.iter().map(|a| a.id.clone()).collect();
    if result.is_complete() && !live_albums.is_empty() {
        confirm_missing_albums(db, client, &mut live_albums);
    }
    let live_tracks = (full && result.is_complete() && !song_ids.is_empty() && listed_everything)
        .then_some(&song_ids);
    if result.is_complete() && !live_albums.is_empty() {
        match queries::remove_vanished_remote(&db.conn, live_tracks, Some(&live_albums)) {
            Ok(0) => {}
            Ok(n) => {
                result.tracks_removed = n;
                log::info!("{n} tracks the server no longer has; removed");
            }
            Err(e) => log::warn!("failed to remove tracks the server deleted: {e}"),
        }
    }

    if result.is_complete() {
        update_last_sync(db, server_url, username, sync_start)?;
    } else {
        log::warn!(
            "{} album(s) and {} page(s) failed to fetch and {} track(s) failed to write — leaving last_sync unchanged so the next sync retries them",
            result.albums_failed,
            result.pages_failed,
            result.tracks_failed,
        );
    }

    log::info!(
        "sync complete: {} artists, {} albums, {} tracks, {} albums and {} tracks failed",
        result.artists_synced,
        result.albums_synced,
        result.tracks_synced,
        result.albums_failed,
        result.tracks_failed,
    );

    db.optimize();

    Ok(result)
}

/// An album as this client holds it: what a listing of the server's albums
/// can be checked against.
struct HeldAlbum {
    title: String,
    artist: String,
    tracks: i64,
    seconds: i64,
}

/// Every album this client holds from the server, by the server's id. `None`
/// if it cannot be read, which leaves an incremental sync to go by `created`
/// alone rather than fetching everything.
fn held_albums(db: &Database) -> Option<HashMap<String, HeldAlbum>> {
    let read = || -> rusqlite::Result<HashMap<String, HeldAlbum>> {
        let mut stmt = db.conn.prepare(
            "SELECT al.remote_id, al.title, COALESCE(ar.name, ''),
                    COUNT(t.id), COALESCE(SUM(t.duration_ms), 0) / 1000
             FROM albums al
             LEFT JOIN artists ar ON ar.id = al.artist_id
             LEFT JOIN tracks t ON t.album_id = al.id AND t.remote_id IS NOT NULL
             WHERE al.remote_id IS NOT NULL
             GROUP BY al.id",
        )?;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                HeldAlbum {
                    title: r.get(1)?,
                    artist: r.get(2)?,
                    tracks: r.get(3)?,
                    seconds: r.get(4)?,
                },
            ))
        })?
        .collect()
    };
    read()
        .inspect_err(|e| log::warn!("could not read held albums; syncing by date alone: {e}"))
        .ok()
}

/// Whether the server lists an album differently from how it is held: not
/// held at all, renamed, credited to someone else, or with other tracks.
fn differs(listed: &SubsonicAlbum, held: Option<&HeldAlbum>) -> bool {
    let Some(held) = held else { return true };
    listed.name != held.title
        || listed.artist.as_deref().is_some_and(|a| a != held.artist)
        || listed
            .song_count
            .is_some_and(|n| i64::from(n) != held.tracks)
        || listed
            .duration
            .is_some_and(|d| (d - held.seconds).abs() > 2)
}

/// Ask the server about each album this library holds that the listing left
/// out, keeping any it still answers for. The listing is an offset walk: an
/// album deleted from an earlier page mid-walk shifts the next page by one, and
/// the album pushed off the boundary is missing from the list without being
/// gone. Only "not found" confirms a deletion; any other failure keeps it.
fn confirm_missing_albums(db: &Database, client: &SubsonicClient, live: &mut HashSet<String>) {
    let held: Vec<String> = db
        .conn
        .prepare(
            "SELECT DISTINCT al.remote_id FROM albums al JOIN tracks t ON t.album_id = al.id
              WHERE al.remote_id IS NOT NULL AND t.path IS NULL AND t.remote_id IS NOT NULL",
        )
        .and_then(|mut stmt| {
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()
        })
        .unwrap_or_default();
    let missing: Vec<String> = held.into_iter().filter(|id| !live.contains(id)).collect();
    for id in missing {
        match client.get_album(&id) {
            Err(SubsonicError::Api { code: 70, .. }) => {}
            Ok(_) => {
                log::info!("album {id} was missing from the listing but still exists");
                live.insert(id);
            }
            Err(e) => {
                log::warn!("could not confirm album {id} was deleted ({e}); keeping it");
                live.insert(id);
            }
        }
    }
}

/// Every album on the server, once each.
fn list_albums(
    client: &SubsonicClient,
    progress: &(dyn Fn(SyncProgress) + Sync),
) -> Result<Vec<SubsonicAlbum>, SyncError> {
    let mut albums = Vec::new();
    // Guards against an album appearing on two pages when the server-side list
    // shifts under the offset walk.
    let mut seen: HashSet<String> = HashSet::new();
    let mut offset = 0u32;
    loop {
        let page = client.get_album_list("alphabeticalByName", PAGE_SIZE, offset)?;
        let count = page.len() as u32;
        offset += count;
        albums.extend(page.into_iter().filter(|a| seen.insert(a.id.clone())));
        progress(SyncProgress {
            phase: SyncPhase::Albums,
            done: albums.len() as u64,
            total: None,
        });
        if count < PAGE_SIZE {
            return Ok(albums);
        }
    }
}

/// Page through every song with an empty `search3` query, joined to the album
/// list, one transaction per page.
///
/// Returns `false`, having written nothing, when the server does not list
/// songs that way — older servers answer an empty query with nothing, or with
/// an error. The caller then walks the albums one at a time.
fn sync_all_songs(
    db: &Database,
    client: &SubsonicClient,
    albums: &[SubsonicAlbum],
    total: Option<u64>,
    result: &mut SyncResult,
    song_ids: &mut HashSet<String>,
    progress: &(dyn Fn(SyncProgress) + Sync),
) -> Result<bool, SyncError> {
    let first = match client.all_songs_page(PAGE_SIZE, 0) {
        Ok(songs) if !songs.is_empty() => songs,
        Ok(_) => {
            log::info!("server lists no songs for an empty search; syncing album by album");
            return Ok(false);
        }
        Err(e) => {
            log::info!("server refused an empty search ({e}); syncing album by album");
            return Ok(false);
        }
    };

    let by_id: HashMap<&str, &SubsonicAlbum> = albums.iter().map(|a| (a.id.as_str(), a)).collect();
    let mut albums_seen: HashSet<String> = HashSet::new();
    let mut write = |songs: Vec<SubsonicSong>,
                     result: &mut SyncResult,
                     song_ids: &mut HashSet<String>|
     -> Result<(), SyncError> {
        let batch = group_by_album(songs, &by_id);
        albums_seen.extend(batch.iter().map(|a| a.id.clone()));
        write_albums(db, client, &batch, result, song_ids)?;
        progress(SyncProgress {
            phase: SyncPhase::Tracks,
            done: song_ids.len() as u64,
            total,
        });
        Ok(())
    };

    let short = (first.len() as u32) < PAGE_SIZE;
    write(first, result, song_ids)?;

    if !short {
        // Pages are fetched on a few lanes and written here, in whatever order
        // they arrive — each is a whole transaction on its own, and the
        // database has one writer however many are fetching. The lanes are
        // threads rather than rayon tasks because they spend their time
        // waiting on the network, not the CPU.
        let next = AtomicU32::new(PAGE_SIZE);
        // The offset of the first page that came back short. Nothing past it
        // is worth asking for.
        let end = AtomicU32::new(u32::MAX);
        let (tx, rx) = std::sync::mpsc::sync_channel(FETCH_LANES);
        let failed = std::thread::scope(|scope| -> Result<usize, SyncError> {
            for _ in 0..FETCH_LANES {
                let tx = tx.clone();
                let (next, end) = (&next, &end);
                scope.spawn(move || {
                    loop {
                        let offset = next.fetch_add(PAGE_SIZE, Ordering::Relaxed);
                        if offset >= end.load(Ordering::Relaxed) {
                            return;
                        }
                        let page = fetch_page(client, offset);
                        // A page that failed after its retries ends the walk
                        // too: the server is likely gone, and asking on for
                        // ever would never finish. The run is then incomplete
                        // and the next sync walks it again.
                        if page
                            .as_ref()
                            .map_or(true, |songs| (songs.len() as u32) < PAGE_SIZE)
                        {
                            end.fetch_min(offset, Ordering::Relaxed);
                        }
                        if tx.send((offset, page)).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(tx);

            let mut failed = 0;
            for (offset, page) in rx {
                match page {
                    Ok(songs) => write(songs, result, song_ids)?,
                    Err(e) => {
                        log::warn!("failed to fetch songs from offset {offset}: {e}");
                        failed += 1;
                    }
                }
            }
            Ok(failed)
        })?;
        result.pages_failed += failed;
    }

    result.albums_synced += albums_seen.len();
    Ok(true)
}

/// One page of songs, retried a couple of times: a failed page is a hole in
/// the library until the next full sync, so it is worth a second ask.
fn fetch_page(
    client: &SubsonicClient,
    offset: u32,
) -> Result<Vec<SubsonicSong>, super::client::SubsonicError> {
    let mut attempt = 1;
    loop {
        match client.all_songs_page(PAGE_SIZE, offset) {
            Ok(songs) => return Ok(songs),
            Err(e) if attempt >= PAGE_ATTEMPTS => return Err(e),
            Err(e) => {
                log::debug!("songs from offset {offset}, attempt {attempt}: {e}");
                std::thread::sleep(std::time::Duration::from_millis(250 * attempt as u64));
                attempt += 1;
            }
        }
    }
}

/// A page of songs as the albums they belong to, each carrying the metadata
/// the album list gave for it.
///
/// A song whose album is not in the list — added after the list was read — is
/// written under what the song itself says about its album.
fn group_by_album(
    songs: Vec<SubsonicSong>,
    albums: &HashMap<&str, &SubsonicAlbum>,
) -> Vec<SubsonicAlbumFull> {
    let mut grouped: Vec<SubsonicAlbumFull> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for song in songs {
        let Some(album_id) = song.album_id.clone() else {
            log::warn!("song {} has no album id; skipped", song.id);
            continue;
        };
        let i = *index.entry(album_id.clone()).or_insert_with(|| {
            grouped.push(match albums.get(album_id.as_str()) {
                Some(album) => SubsonicAlbumFull {
                    id: album.id.clone(),
                    name: album.name.clone(),
                    artist: album.artist.clone(),
                    artist_id: album.artist_id.clone(),
                    year: album.year,
                    genre: album.genre.clone(),
                    song_count: album.song_count,
                    created: album.created.clone(),
                    music_brainz_id: album.music_brainz_id.clone(),
                    sort_name: album.sort_name.clone(),
                    record_labels: album.record_labels.clone(),
                    song: Vec::new(),
                },
                None => SubsonicAlbumFull {
                    id: album_id,
                    name: song.album.clone().unwrap_or_default(),
                    artist: song.artist.clone(),
                    artist_id: song.artist_id.clone(),
                    year: song.year,
                    genre: song.genre.clone(),
                    song_count: None,
                    created: None,
                    music_brainz_id: None,
                    sort_name: None,
                    record_labels: Vec::new(),
                    song: Vec::new(),
                },
            });
            grouped.len() - 1
        });
        grouped[i].song.push(song);
    }
    grouped
}

/// Albums to write per transaction on the per-album path. Small enough that
/// progress moves often, large enough that the fetches overlap.
const ALBUM_BATCH: usize = 100;

/// Fetch each album on its own and write them a batch at a time. What an
/// incremental sync does, and what a full one falls back to on a server that
/// cannot list songs in bulk.
fn sync_by_album(
    db: &Database,
    client: &SubsonicClient,
    albums: &[&SubsonicAlbum],
    total: Option<u64>,
    result: &mut SyncResult,
    song_ids: &mut HashSet<String>,
    progress: &(dyn Fn(SyncProgress) + Sync),
) -> Result<(), SyncError> {
    for batch in albums.chunks(ALBUM_BATCH) {
        let failures = AtomicUsize::new(0);
        let fetched: Vec<SubsonicAlbumFull> = batch
            .par_iter()
            .filter_map(|album| match client.get_album(&album.id) {
                Ok(full) => Some(full),
                Err(e) => {
                    log::warn!("failed to fetch album {}: {}", album.id, e);
                    failures.fetch_add(1, Ordering::Relaxed);
                    None
                }
            })
            .collect();
        result.albums_failed += failures.into_inner();
        result.albums_synced += fetched.len();

        write_albums(db, client, &fetched, result, song_ids)?;
        progress(SyncProgress {
            phase: SyncPhase::Tracks,
            done: song_ids.len() as u64,
            total,
        });
        log::info!(
            "synced {} albums ({} tracks) so far...",
            result.albums_synced,
            result.tracks_synced
        );
    }
    Ok(())
}

/// Record what the server knows about each artist.
///
/// Best-effort: a library that synced its tracks fine should not fail because
/// one artist row could not be updated.
fn write_artists(db: &Database, artists: &[SubsonicArtist], result: &mut SyncResult) {
    // Immediate: a transaction that reads before it writes cannot take the
    // write lock later if another connection wrote in between, and fails at
    // once instead of waiting for it.
    if db.conn.execute_batch("BEGIN IMMEDIATE").is_err() {
        return;
    }
    let mut enriched = 0;
    for artist in artists {
        match queries::enrich_remote_artist(
            &db.conn,
            &artist.id,
            artist.music_brainz_id.as_deref(),
            artist.sort_name.as_deref(),
        ) {
            Ok(()) => enriched += 1,
            Err(e) => log::warn!(
                "failed to record artist metadata for {}: {}",
                artist.name,
                e
            ),
        }
    }
    if db.conn.execute_batch("COMMIT").is_err() {
        let _ = db.conn.execute_batch("ROLLBACK");
        return;
    }
    result.artists_synced = enriched;
    log::info!("recorded metadata for {enriched} artists");
}

/// Write one batch of fetched albums in a single transaction.
fn write_albums(
    db: &Database,
    client: &SubsonicClient,
    albums: &[SubsonicAlbumFull],
    result: &mut SyncResult,
    song_ids: &mut HashSet<String>,
) -> Result<(), SyncError> {
    // Immediate, for the reason `write_artists` gives: deferred, a page that
    // met another writer failed every insert in it, logging each.
    db.conn
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(crate::db::connection::DbError::from)?;

    for album in albums {
        let artist_name = album.artist.as_deref().unwrap_or("Unknown Artist");

        for song in &album.song {
            song_ids.insert(song.id.clone());
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
                // OpenSubsonic servers report these; a plain Subsonic one
                // leaves them out and the track keeps no quality figures.
                //
                // Zero means "not applicable", not "zero" — Navidrome reports
                // bitDepth 0 for every lossy file. Storing it would render an
                // MP3 as 0-bit, which is worse than saying nothing.
                sample_rate: positive(song.sampling_rate),
                bit_depth: positive(song.bit_depth),
                channels: positive(song.channel_count),
                bitrate: song.bit_rate,
                size_bytes: None,
                mtime: None,
                path: None,
                source: "remote".to_string(),
                remote_id: Some(song.id.clone()),
                remote_url: Some(client.stream_url_template(&song.id)),
                album_remote_id: Some(album.id.clone()),
                artist_remote_id: album.artist_id.clone(),
                mbid: song.music_brainz_id.clone(),
                album_mbid: album.music_brainz_id.clone(),
                album_added_at: album.created.clone(),
            };

            match queries::upsert_synced_track(&db.conn, &meta, song_ids) {
                Ok(_) => result.tracks_synced += 1,
                Err(e) => {
                    result.tracks_failed += 1;
                    log::warn!("failed to insert remote track {}: {}", song.title, e);
                }
            }
        }

        // The album row is created through its tracks, so it only ever sees
        // what a file's tags say. Track totals, the label and the MusicBrainz
        // id belong to the release, and came back in the same response.
        if let Err(e) = queries::enrich_remote_album(
            &db.conn,
            &album.id,
            album.music_brainz_id.as_deref(),
            album.sort_name.as_deref(),
            album.song_count,
            album
                .record_labels
                .iter()
                .map(|l| l.name.as_str())
                .find(|l| !l.is_empty()),
        ) {
            log::warn!("failed to record album metadata for {}: {}", album.name, e);
        }
    }

    db.conn
        .execute_batch("COMMIT")
        .map_err(crate::db::connection::DbError::from)?;

    Ok(())
}

/// Treat a non-positive figure as absent. None of sample rate, bit depth or
/// channel count has a meaningful zero.
fn positive(value: Option<i32>) -> Option<i32> {
    value.filter(|v| *v > 0)
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
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: Some(1000),
            size_bytes: None,
            mtime: None,
            path: None,
            source: "remote".into(),
            remote_id: Some(remote_id.into()),
            remote_url: Some(format!("https://example.com/stream?id={}", remote_id)),
            album_remote_id: Some(format!("album-of-{remote_id}")),
            artist_remote_id: Some(format!("artist-of-{remote_id}")),
            mbid: Some(format!("mbid-of-{remote_id}")),
            album_mbid: None,
            album_added_at: None,
        }
    }

    /// Navidrome reports bitDepth 0 for every lossy file. Keeping it would
    /// render an MP3 as 0-bit; absent is the honest answer.
    #[test]
    fn a_zero_quality_figure_is_treated_as_absent() {
        assert_eq!(positive(Some(0)), None);
        assert_eq!(positive(Some(16)), Some(16));
        assert_eq!(positive(None), None);
    }

    /// The album row is created through its tracks, so it only ever sees what
    /// a file's tags say. Track totals, the label and the MusicBrainz id are
    /// properties of the release and arrive in the same response.
    #[test]
    fn album_metadata_from_the_server_is_recorded() {
        let (db, _dir) = test_db();

        let meta = remote_track_meta("remote-300", "Anguish", "Sleep", "Volume One");
        queries::upsert_track(&db.conn, &meta).unwrap();

        queries::enrich_remote_album(
            &db.conn,
            "album-of-remote-300",
            Some("mb-album-1"),
            Some("volume one"),
            Some(6),
            Some("Off The Disk"),
        )
        .unwrap();

        let row: (Option<String>, Option<String>, Option<i32>, Option<String>) = db
            .conn
            .query_row(
                "SELECT mbid, sort_name, total_tracks, label FROM albums WHERE title = 'Volume One'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("mb-album-1"));
        assert_eq!(row.1.as_deref(), Some("volume one"));
        assert_eq!(row.2, Some(6));
        assert_eq!(row.3.as_deref(), Some("Off The Disk"));
    }

    /// Enrichment fills blanks. A locally-scanned album whose tags named a
    /// label must keep it, rather than have the server's answer written over.
    #[test]
    fn server_metadata_does_not_overwrite_what_tags_said() {
        let (db, _dir) = test_db();

        let mut meta = remote_track_meta("remote-301", "Dopesmoker", "Sleep", "Dopesmoker");
        meta.label = Some("From The Tags".into());
        queries::upsert_track(&db.conn, &meta).unwrap();

        queries::enrich_remote_album(
            &db.conn,
            "album-of-remote-301",
            None,
            None,
            None,
            Some("From The Server"),
        )
        .unwrap();

        let label: Option<String> = db
            .conn
            .query_row(
                "SELECT label FROM albums WHERE title = 'Dopesmoker'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(label.as_deref(), Some("From The Tags"));
    }

    /// Artists existed only as a side effect of a track upsert, so nothing
    /// ever wrote their MusicBrainz id or sort name.
    #[test]
    fn artist_metadata_from_the_server_is_recorded() {
        let (db, _dir) = test_db();

        let meta = remote_track_meta("remote-302", "Holy Mountain", "Sleep", "Holy Mountain");
        queries::upsert_track(&db.conn, &meta).unwrap();

        queries::enrich_remote_artist(
            &db.conn,
            "artist-of-remote-302",
            Some("mb-artist-1"),
            Some("sleep"),
        )
        .unwrap();

        let row: (Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT mbid, sort_name FROM artists WHERE name = 'Sleep'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("mb-artist-1"));
        assert_eq!(row.1.as_deref(), Some("sleep"));
    }

    /// The recording id travels with the track.
    #[test]
    fn a_synced_track_keeps_its_musicbrainz_id() {
        let (db, _dir) = test_db();

        let meta = remote_track_meta("remote-303", "Aquarian", "Sleep", "Dopesmoker");
        let id = queries::upsert_track(&db.conn, &meta).unwrap();

        let mbid: Option<String> = db
            .conn
            .query_row("SELECT mbid FROM tracks WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(mbid.as_deref(), Some("mbid-of-remote-303"));
    }

    /// A remote track carries the quality figures an OpenSubsonic server
    /// reports. Without them the format badge has nothing to show, which is
    /// what every remote-only track in a synced library used to look like.
    #[test]
    fn a_synced_track_keeps_its_quality_figures() {
        let (db, _dir) = test_db();

        let meta = remote_track_meta("remote-200", "Anguish", "Sleep", "Volume One");
        let id = queries::upsert_track(&db.conn, &meta).unwrap();

        let row = queries::get_track_row(&db.conn, id).unwrap().unwrap();
        assert_eq!(row.sample_rate, Some(44100));
        assert_eq!(row.bit_depth, Some(16));
        assert_eq!(row.channels, Some(2));
    }

    /// The server keys stars, shares and cover art off album and artist ids,
    /// so a sync that only kept the track's left the library unable to refer
    /// to either — every album row came back with a null remote_id.
    #[test]
    fn a_sync_records_the_album_and_artist_ids_too() {
        let (db, _dir) = test_db();

        let meta = remote_track_meta("remote-100", "Enter", "Russian Circles", "Enter");
        queries::upsert_track(&db.conn, &meta).unwrap();

        let album: Option<String> = db
            .conn
            .query_row(
                "SELECT remote_id FROM albums WHERE title = 'Enter'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(album.as_deref(), Some("album-of-remote-100"));

        let artist: Option<String> = db
            .conn
            .query_row(
                "SELECT remote_id FROM artists WHERE name = 'Russian Circles'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(artist.as_deref(), Some("artist-of-remote-100"));
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
        /// Album ids left out of the album list while still existing, as an
        /// offset walk does when a deletion shifts a page.
        unlisted: Mutex<HashSet<String>>,
        /// Prepended to the album list once the first list page has been served,
        /// modelling a server-side insert landing mid-pagination.
        insert_after_first_page: Mutex<Option<(String, String, String)>>,
        list_pages_served: AtomicUsize,
        list_types: Mutex<Vec<String>>,
        album_calls: Mutex<Vec<String>>,
        /// Answer an empty `search3` with every album's song, as an
        /// OpenSubsonic server does. Off, it answers with nothing, as older
        /// servers do.
        lists_songs: bool,
        /// `songOffset`s asked for.
        song_pages: Mutex<Vec<usize>>,
        /// Song pages that fail with a 500, by offset.
        failing_song_pages: Mutex<HashSet<usize>>,
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
                let unlisted = state.unlisted.lock().unwrap();
                let slice: Vec<String> = albums
                    .iter()
                    .filter(|(id, _, _)| !unlisted.contains(id))
                    .skip(offset)
                    .take(size)
                    .map(|(id, name, created)| {
                        format!(
                            r#"{{"id":"{}","name":"{}","artist":"Stub Artist","created":"{}","songCount":1}}"#,
                            id, name, created
                        )
                    })
                    .collect();
                drop(albums);
                drop(unlisted);

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
            "search3" => {
                let offset: usize = params.get("songOffset").and_then(|o| o.parse().ok()).unwrap_or(0);
                let size: usize = params.get("songCount").and_then(|s| s.parse().ok()).unwrap_or(20);
                state.song_pages.lock().unwrap().push(offset);
                if state.failing_song_pages.lock().unwrap().contains(&offset) {
                    return (500, r#"{"error":"boom"}"#.to_string());
                }
                let songs: Vec<String> = if state.lists_songs {
                    state
                        .albums
                        .lock()
                        .unwrap()
                        .iter()
                        .skip(offset)
                        .take(size)
                        .map(|(id, name, _)| {
                            format!(
                                r#"{{"id":"s{id}","title":"Song {id}","albumId":"{id}","album":"{name}","track":1,"suffix":"flac"}}"#
                            )
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                (
                    200,
                    format!(
                        r#"{{"subsonic-response":{{"status":"ok","searchResult3":{{"song":[{}]}}}}}}"#,
                        songs.join(",")
                    ),
                )
            }
            "getAlbum" => {
                let id = params.get("id").copied().unwrap_or("");
                state.album_calls.lock().unwrap().push(id.to_string());
                if state.failing.lock().unwrap().contains(id) {
                    (500, r#"{"error":"boom"}"#.to_string())
                } else if !state.albums.lock().unwrap().iter().any(|(a, _, _)| a == id) {
                    (
                        200,
                        r#"{"subsonic-response":{"status":"failed","error":{"code":70,"message":"not found"}}}"#
                            .to_string(),
                    )
                } else {
                    (
                        200,
                        format!(
                            r#"{{"subsonic-response":{{"status":"ok","album":{{"id":"{id}","name":"{name}","artist":"Stub Artist","song":[{{"id":"s{id}","title":"Song {id}","track":1,"suffix":"flac"}}]}}}}}}"#,
                            name = state
                                .albums
                                .lock()
                                .unwrap()
                                .iter()
                                .find(|(a, _, _)| a == id)
                                .map_or_else(|| format!("Album {id}"), |(_, n, _)| n.clone())
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

        let first = sync_library(&db, &client, false, &server.url(), "u", &|_| {}).unwrap();
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
        let second = sync_library(&db, &client, false, &server.url(), "u", &|_| {}).unwrap();

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

        let result = sync_library(&db, &client, true, &server.url(), "u", &|_| {}).unwrap();
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
    fn a_full_sync_relinks_files_whose_server_id_changed() {
        let (db, _dir) = test_db();
        let file = TrackMeta {
            date: None,
            disc: None,
            path: Some("/music/song.flac".into()),
            source: "local".into(),
            album_remote_id: None,
            artist_remote_id: None,
            mbid: None,
            ..remote_track_meta("s-before-rescan", "Song a0000", "Stub Artist", "Album 0000")
        };
        queries::upsert_track(&db.conn, &file).unwrap();

        let state = Arc::new(StubState {
            albums: Mutex::new(stub_albums(1)),
            ..Default::default()
        });
        let server = StubServer::start(state);
        let client = SubsonicClient::new(&server.url(), "u", "p");
        sync_library(&db, &client, true, &server.url(), "u", &|_| {}).unwrap();

        let rows: Vec<(Option<String>, Option<String>)> = db
            .conn
            .prepare("SELECT path, remote_id FROM tracks WHERE title = 'Song a0000'")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(Some("/music/song.flac".into()), Some("sa0000".into()))],
            "the file takes the server's current id, and the recording is listed once"
        );
    }

    #[test]
    fn a_full_sync_folds_server_rows_whose_id_vanished() {
        let (db, _dir) = test_db();
        let ghost = TrackMeta {
            date: None,
            disc: None,
            album_remote_id: None,
            artist_remote_id: None,
            mbid: None,
            ..remote_track_meta("s-before-rescan", "Song a0000", "Stub Artist", "Album 0000")
        };
        let ghost_id = queries::upsert_track(&db.conn, &ghost).unwrap();
        let ghost_url = ghost.remote_url.clone().unwrap();
        db.conn
            .execute(
                "INSERT INTO favourites (track_path) VALUES (?1)",
                params![ghost_url],
            )
            .unwrap();

        let state = Arc::new(StubState {
            albums: Mutex::new(stub_albums(1)),
            ..Default::default()
        });
        let server = StubServer::start(state);
        let client = SubsonicClient::new(&server.url(), "u", "p");
        sync_library(&db, &client, true, &server.url(), "u", &|_| {}).unwrap();

        let rows: Vec<(i64, String, String)> = db
            .conn
            .prepare("SELECT id, remote_id, remote_url FROM tracks WHERE title = 'Song a0000'")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1, "the dead row takes the live id");
        let (id, remote_id, remote_url) = &rows[0];
        assert_eq!(*id, ghost_id);
        assert_eq!(remote_id, "sa0000");
        let favourites: Vec<String> = db
            .conn
            .prepare("SELECT track_path FROM favourites")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            favourites,
            vec![remote_url.clone()],
            "the favourite follows"
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

        // Everything held as the server lists it, then the watermark put
        // between the two vintages.
        sync_library(&db, &client, true, &server.url(), "u", &|_| {}).unwrap();
        state.album_calls.lock().unwrap().clear();
        let watermark = parse_iso8601_to_unix("2025-01-01T00:00:00Z").unwrap();
        update_last_sync(&db, &server.url(), "u", watermark).unwrap();

        let result = sync_library(&db, &client, false, &server.url(), "u", &|_| {}).unwrap();

        assert_eq!(result.albums_synced, 1, "only the new album needs fetching");
        assert_eq!(
            *state.album_calls.lock().unwrap(),
            vec!["a0002".to_string()]
        );
    }

    /// A retag on the server keeps an album's `created`. Going by the date
    /// alone, the client would hold the old tags until a full sync.
    #[test]
    fn incremental_sync_fetches_an_album_the_server_now_lists_differently() {
        let (db, _dir) = test_db();
        let mut albums = stub_albums(3);
        for a in &mut albums {
            a.2 = "2020-01-01T00:00:00Z".into();
        }
        let state = Arc::new(StubState {
            albums: Mutex::new(albums),
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");
        sync_library(&db, &client, true, &server.url(), "u", &|_| {}).unwrap();
        state.album_calls.lock().unwrap().clear();

        state.albums.lock().unwrap()[1].1 = "Album 0001 (Retitled)".into();
        let result = sync_library(&db, &client, false, &server.url(), "u", &|_| {}).unwrap();

        assert_eq!(
            *state.album_calls.lock().unwrap(),
            vec!["a0001".to_string()]
        );
        assert_eq!(result.albums_synced, 1);
        let title: String = db
            .conn
            .query_row(
                "SELECT title FROM albums WHERE remote_id = 'a0001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "Album 0001 (Retitled)");
    }

    /// An album the client has never held is fetched however old the server
    /// says it is: a retag can move tracks to a new album id with an old date.
    #[test]
    fn incremental_sync_fetches_an_album_it_does_not_hold() {
        let (db, _dir) = test_db();
        let mut albums = stub_albums(2);
        for a in &mut albums {
            a.2 = "2020-01-01T00:00:00Z".into();
        }
        let state = Arc::new(StubState {
            albums: Mutex::new(albums),
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");
        sync_library(&db, &client, true, &server.url(), "u", &|_| {}).unwrap();
        state.album_calls.lock().unwrap().clear();

        state.albums.lock().unwrap().push((
            "a0099".into(),
            "Album 0099".into(),
            "2020-01-01T00:00:00Z".into(),
        ));
        sync_library(&db, &client, false, &server.url(), "u", &|_| {}).unwrap();

        assert_eq!(
            *state.album_calls.lock().unwrap(),
            vec!["a0099".to_string()]
        );
    }

    /// An album missing from the listing is removed only once the server says
    /// it is gone.
    #[test]
    fn an_album_left_out_of_the_listing_is_removed_only_when_gone() {
        let (db, _dir) = test_db();
        let state = Arc::new(StubState {
            albums: Mutex::new(stub_albums(3)),
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");
        sync_library(&db, &client, true, &server.url(), "u", &|_| {}).unwrap();

        state.unlisted.lock().unwrap().insert("a0001".into());
        let shifted = sync_library(&db, &client, false, &server.url(), "u", &|_| {}).unwrap();
        assert_eq!(shifted.tracks_removed, 0, "still there, only unlisted");

        state
            .albums
            .lock()
            .unwrap()
            .retain(|(id, _, _)| id != "a0001");
        let deleted = sync_library(&db, &client, false, &server.url(), "u", &|_| {}).unwrap();
        assert_eq!(deleted.tracks_removed, 1);
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
        let result = sync_library(&db, &client, false, &server.url(), "u", &|_| {}).unwrap();

        assert_eq!(
            result.albums_synced, 1,
            "an album with no usable timestamp must not be assumed old"
        );
    }

    /// A server that lists songs for an empty query is synced in pages of
    /// songs, not one request per album.
    #[test]
    fn a_full_sync_pages_songs_instead_of_fetching_each_album() {
        let (db, _dir) = test_db();
        let state = Arc::new(StubState {
            albums: Mutex::new(stub_albums(1_234)),
            lists_songs: true,
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");

        let seen = Mutex::new(Vec::new());
        let result = sync_library(&db, &client, true, &server.url(), "u", &|p| {
            seen.lock().unwrap().push(p)
        })
        .unwrap();

        assert!(state.album_calls.lock().unwrap().is_empty(), "no getAlbum");
        let mut pages = state.song_pages.lock().unwrap().clone();
        pages.sort();
        pages.dedup();
        assert_eq!(pages[..3], [0, 500, 1000]);
        assert_eq!(result.tracks_synced, 1_234);
        assert_eq!(result.albums_synced, 1_234);
        assert!(result.is_complete());
        assert!(get_last_sync(&db, &server.url()).unwrap().is_some());

        let album: (Option<String>, Option<i32>) = db
            .conn
            .query_row(
                "SELECT al.remote_id, al.total_tracks FROM albums al WHERE al.title = 'Album 0007'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            album,
            (Some("a0007".into()), Some(1)),
            "album metadata comes from the list"
        );

        let seen = seen.into_inner().unwrap();
        let tracks: Vec<&SyncProgress> = seen
            .iter()
            .filter(|p| p.phase == SyncPhase::Tracks)
            .collect();
        assert!(tracks.len() >= 3, "progress at least per page");
        assert!(tracks.iter().all(|p| p.total == Some(1_234)));
        assert_eq!(tracks.last().unwrap().done, 1_234);
        assert_eq!(seen.last().unwrap().phase, SyncPhase::Finishing);
    }

    /// Older servers answer an empty query with nothing. The album walk is
    /// what they get instead.
    #[test]
    fn a_server_that_lists_no_songs_is_synced_album_by_album() {
        let (db, _dir) = test_db();
        let state = Arc::new(StubState {
            albums: Mutex::new(stub_albums(3)),
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");

        let result = sync_library(&db, &client, true, &server.url(), "u", &|_| {}).unwrap();

        assert_eq!(state.song_pages.lock().unwrap().clone(), vec![0]);
        assert_eq!(state.album_calls.lock().unwrap().len(), 3);
        assert_eq!(result.tracks_synced, 3);
        assert!(result.is_complete());
    }

    /// A page lost to the network leaves the run incomplete, so the watermark
    /// stays put and the next sync walks the library again.
    #[test]
    fn a_failed_song_page_does_not_advance_last_sync() {
        let (db, _dir) = test_db();
        let state = Arc::new(StubState {
            albums: Mutex::new(stub_albums(1_200)),
            lists_songs: true,
            failing_song_pages: Mutex::new([500].into_iter().collect()),
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");

        let result = sync_library(&db, &client, true, &server.url(), "u", &|_| {}).unwrap();

        assert_eq!(result.pages_failed, 1);
        assert!(!result.is_complete());
        assert_eq!(get_last_sync(&db, &server.url()).unwrap(), None);
        let retries = state
            .song_pages
            .lock()
            .unwrap()
            .iter()
            .filter(|&&o| o == 500)
            .count();
        assert_eq!(retries as u32, PAGE_ATTEMPTS);
    }

    /// An incremental sync fetches the few new albums one by one rather than
    /// walking every song.
    #[test]
    fn an_incremental_sync_does_not_walk_every_song() {
        let (db, _dir) = test_db();
        let state = Arc::new(StubState {
            albums: Mutex::new(stub_albums(3)),
            lists_songs: true,
            ..Default::default()
        });
        let server = StubServer::start(state.clone());
        let client = SubsonicClient::new(&server.url(), "u", "p");
        update_last_sync(&db, &server.url(), "u", 0).unwrap();

        sync_library(&db, &client, false, &server.url(), "u", &|_| {}).unwrap();

        assert!(state.song_pages.lock().unwrap().is_empty());
        assert_eq!(state.album_calls.lock().unwrap().len(), 3);
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

    /// A koan server once published row ids and now publishes UUIDs; any
    /// server that rescans can renumber. A re-sync keeps the row, and with it
    /// the history and the favourite, rather than adding a second copy.
    #[test]
    fn resyncing_a_track_under_a_new_server_id_keeps_one_row() {
        let (db, _dir) = test_db();
        let before = remote_track_meta("46215", "Archangel", "Burial", "Untrue");
        let row = queries::upsert_synced_track(&db.conn, &before, &HashSet::from(["46215".into()]))
            .unwrap();
        queries::add_favourite(
            &db.conn,
            queries::LOCAL_USER,
            std::path::Path::new(before.remote_url.as_deref().unwrap()),
        )
        .unwrap();

        let uid = "0199a0b2-7c4e-7d3a-9f1b-2c3d4e5f6a7b";
        let after = remote_track_meta(uid, "Archangel", "Burial", "Untrue");
        let again =
            queries::upsert_synced_track(&db.conn, &after, &HashSet::from([uid.into()])).unwrap();

        assert_eq!(again, row, "the same row, not a second copy");
        assert_eq!(queries::library_stats(&db.conn).unwrap().total_tracks, 1);
        let (remote_id, album, artist): (String, String, String) = db
            .conn
            .query_row(
                "SELECT t.remote_id, al.remote_id, ar.remote_id FROM tracks t
                 JOIN albums al ON al.id = t.album_id JOIN artists ar ON ar.id = t.artist_id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(remote_id, uid);
        assert_eq!(album, format!("album-of-{uid}"));
        assert_eq!(artist, format!("artist-of-{uid}"));
        let adopted: String = db
            .conn
            .query_row("SELECT uid FROM tracks WHERE id = ?1", [row], |r| r.get(0))
            .unwrap();
        assert_eq!(adopted, uid, "the row takes the server's uid");
        let favourites = queries::load_favourites(&db.conn, queries::LOCAL_USER).unwrap();
        assert_eq!(
            favourites,
            HashSet::from([std::path::PathBuf::from(after.remote_url.unwrap())]),
            "the favourite follows the new stream address"
        );
    }

    /// Two entries a server lists with the same tags are two tracks, however
    /// alike: the second is not taken for the first under an old id.
    #[test]
    fn identical_entries_in_one_sync_stay_two_rows() {
        let (db, _dir) = test_db();
        let mut seen = HashSet::new();
        for id in ["dup-1", "dup-2"] {
            seen.insert(id.to_string());
            queries::upsert_synced_track(
                &db.conn,
                &remote_track_meta(id, "Archangel", "Burial", "Untrue"),
                &seen,
            )
            .unwrap();
        }
        assert_eq!(queries::library_stats(&db.conn).unwrap().total_tracks, 2);
    }
}
