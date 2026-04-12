use crate::db::connection::Database;
use crate::db::queries::{self, TrackMeta};
use crate::remote::client::{SubsonicAlbum, SubsonicAlbumFull, SubsonicClient};

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
/// If `full` is false and a `last_sync` timestamp exists for this server,
/// performs an incremental sync using `getAlbumList2(type=newest)`, stopping
/// when all albums on a page predate the last sync. Otherwise does a full sync
/// using `alphabeticalByName`.
///
/// Pipeline: paginate album list -> fetch album details in parallel (rayon) ->
/// batch-write each page in a single transaction.
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

    // Determine sync mode: incremental (newest-first, stop at last_sync) or full.
    let last_sync = if full {
        None
    } else {
        get_last_sync(db, server_url)?
    };

    let (list_type, is_incremental) = match last_sync {
        Some(_) => ("newest", true),
        None => ("alphabeticalByName", false),
    };

    if is_incremental {
        log::info!("incremental sync (newest since last sync)");
    } else {
        log::info!("full sync (alphabetical)");
    }

    let sync_start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut offset = 0u32;
    let page_size = 500u32;

    loop {
        let album_summaries = client.get_album_list(list_type, page_size, offset)?;
        if album_summaries.is_empty() {
            break;
        }

        let page_count = album_summaries.len();

        // For incremental sync, check if we've passed the last_sync boundary.
        // If the oldest album on this page was created before last_sync, we
        // still process this page but stop after it.
        let should_stop_after = if let Some(last_ts) = last_sync {
            album_summaries
                .iter()
                .filter_map(|a| a.created.as_deref())
                .filter_map(parse_iso8601_to_unix)
                .min()
                .is_some_and(|oldest| oldest < last_ts)
        } else {
            false
        };

        // Parallel fetch: get full album details (with tracks) concurrently.
        let fetched: Vec<(SubsonicAlbum, SubsonicAlbumFull)> = album_summaries
            .into_par_iter()
            .filter_map(|summary| match client.get_album(&summary.id) {
                Ok(full) => Some((summary, full)),
                Err(e) => {
                    log::warn!("failed to fetch album {}: {}", summary.id, e);
                    None
                }
            })
            .collect();

        // Batch write in a single transaction.
        db.conn
            .execute_batch("BEGIN")
            .map_err(crate::db::connection::DbError::from)?;

        for (_, album) in &fetched {
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

        offset += page_count as u32;
        log::info!(
            "synced {} albums ({} tracks) so far...",
            result.albums_synced,
            result.tracks_synced
        );

        if should_stop_after {
            log::info!("incremental sync: reached albums older than last sync, stopping");
            break;
        }
    }

    // Record successful sync timestamp.
    update_last_sync(db, server_url, username, sync_start)?;

    log::info!(
        "sync complete: {} artists, {} albums, {} tracks",
        result.artists_synced,
        result.albums_synced,
        result.tracks_synced,
    );

    Ok(result)
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
