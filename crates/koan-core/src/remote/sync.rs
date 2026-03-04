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
fn parse_iso8601_to_unix(s: &str) -> Option<i64> {
    // Try full RFC 3339 first (e.g. "2024-01-15T10:30:00Z" or "2024-01-15T10:30:00+00:00").
    // Then try common Subsonic variants like "2024-01-15T10:30:00.000Z".
    // The `chrono`-free approach: parse manually or use a simpler method.

    // Strip fractional seconds for simpler parsing.
    let normalized = if let Some(dot_pos) = s.find('.') {
        // Find where fractional seconds end (at 'Z', '+', or '-' after the dot).
        let rest = &s[dot_pos + 1..];
        let end = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
        format!("{}{}", &s[..dot_pos], &s[dot_pos + 1 + end..])
    } else {
        s.to_string()
    };

    // Append Z if no timezone info present.
    let with_tz = if !normalized.contains('Z')
        && !normalized.contains('+')
        && !normalized[10..].contains('-')
    {
        format!("{}Z", normalized)
    } else {
        normalized
    };

    // Replace trailing Z with +00:00 for consistent parsing.
    let rfc3339 = with_tz.replace("Z", "+00:00").replace("z", "+00:00");

    // Parse: "2024-01-15T10:30:00+00:00"
    parse_rfc3339_manual(&rfc3339)
}

/// Manual RFC 3339 parser returning unix timestamp.
fn parse_rfc3339_manual(s: &str) -> Option<i64> {
    // Expected format: YYYY-MM-DDTHH:MM:SS+HH:MM or YYYY-MM-DDTHH:MM:SS-HH:MM
    if s.len() < 25 {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: i64 = s[5..7].parse().ok()?;
    let day: i64 = s[8..10].parse().ok()?;
    let hour: i64 = s[11..13].parse().ok()?;
    let min: i64 = s[14..16].parse().ok()?;
    let sec: i64 = s[17..19].parse().ok()?;

    let tz_sign: i64 = if s.as_bytes()[19] == b'-' { -1 } else { 1 };
    let tz_hour: i64 = s[20..22].parse().ok()?;
    let tz_min: i64 = s[23..25].parse().ok()?;

    // Days from year 1970 to the given date (simplified, handles leap years).
    let days = days_from_epoch(year, month, day)?;
    let utc_secs = days * 86400 + hour * 3600 + min * 60 + sec;
    let tz_offset = tz_sign * (tz_hour * 3600 + tz_min * 60);

    Some(utc_secs - tz_offset)
}

/// Calculate days from Unix epoch (1970-01-01) to a given date.
fn days_from_epoch(year: i64, month: i64, day: i64) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days)
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
                    remote_url: Some(client.stream_url(&song.id)),
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
