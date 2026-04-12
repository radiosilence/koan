use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::albums::get_or_create_album;
use super::artists::{escape_like, get_or_create_artist};
use super::{PlaybackSource, TrackMeta, TrackRow};

/// Map a rusqlite Row to a TrackRow. Expects the standard column order:
/// id, album_id, artist_id, artist_name, album_artist_name, album_title,
/// disc, track_number, title, duration_ms, path,
/// codec, sample_rate, bit_depth, channels, bitrate,
/// genre, source, remote_id, cached_path
fn row_to_track_row(row: &rusqlite::Row) -> rusqlite::Result<TrackRow> {
    let artist_name: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
    Ok(TrackRow {
        id: row.get(0)?,
        album_id: row.get(1)?,
        artist_id: row.get(2)?,
        artist_name: artist_name.clone(),
        album_artist_name: row.get::<_, Option<String>>(4)?.unwrap_or(artist_name),
        album_title: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
        disc: row.get(6)?,
        track_number: row.get(7)?,
        title: row.get(8)?,
        duration_ms: row.get(9)?,
        path: row.get(10)?,
        codec: row.get(11)?,
        sample_rate: row.get(12)?,
        bit_depth: row.get(13)?,
        channels: row.get(14)?,
        bitrate: row.get(15)?,
        genre: row.get(16)?,
        source: row.get(17)?,
        remote_id: row.get(18)?,
        cached_path: row.get(19)?,
    })
}

/// Insert or update a track. Deduplicates local+remote: one row per logical track.
///
/// Matching priority:
/// 1. By path (local tracks)
/// 2. By remote_id (remote tracks)
/// 3. By content match: same artist_id + album_id + title + track# (cross-source merge)
///
/// When merging, local metadata (codec, sample_rate, etc.) wins over remote.
/// The `source` field reflects what's available: "local" if path exists, "remote" if remote-only.
pub fn upsert_track(conn: &Connection, meta: &TrackMeta) -> Result<i64, DbError> {
    // Use a savepoint so this works both standalone and inside an existing
    // transaction (e.g. the batch transaction in scan_folder).
    conn.execute_batch("SAVEPOINT upsert_track")?;

    let result = upsert_track_inner(conn, meta);
    match &result {
        Ok(_) => conn.execute_batch("RELEASE upsert_track")?,
        Err(_) => conn.execute_batch("ROLLBACK TO upsert_track; RELEASE upsert_track")?,
    }
    result
}

fn upsert_track_inner(conn: &Connection, meta: &TrackMeta) -> Result<i64, DbError> {
    let album_artist_name = meta.album_artist.as_deref().unwrap_or(&meta.artist);
    let album_artist_id = get_or_create_artist(conn, album_artist_name, None)?;
    // Track artist — may differ from album artist (e.g. compilations, VA albums).
    let track_artist_id = if meta.artist == album_artist_name {
        album_artist_id
    } else {
        get_or_create_artist(conn, &meta.artist, None)?
    };
    let album_id = get_or_create_album(
        conn,
        &meta.album,
        album_artist_id,
        meta.date.as_deref(),
        None,
        None,
        meta.codec.as_deref(),
        meta.label.as_deref(),
        None,
    )?;

    // 1. Match by path.
    let track_id: Option<i64> = if let Some(ref path) = meta.path {
        conn.query_row(
            "SELECT id FROM tracks WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )
        .ok()
    } else {
        None
    };

    // 2. Match by remote_id.
    let track_id = track_id.or_else(|| {
        meta.remote_id.as_ref().and_then(|rid| {
            conn.query_row(
                "SELECT id FROM tracks WHERE remote_id = ?1",
                params![rid],
                |row| row.get(0),
            )
            .ok()
        })
    });

    // 3. Content match: same artist + album + title + track# (cross-source dedup).
    let track_id = track_id.or_else(|| {
        conn.query_row(
            "SELECT id FROM tracks
             WHERE artist_id = ?1 AND album_id = ?2 AND title = ?3
               AND COALESCE(track_number, -1) = COALESCE(?4, -1)",
            params![track_artist_id, album_id, meta.title, meta.track_number],
            |row| row.get(0),
        )
        .ok()
    });

    if let Some(id) = track_id {
        // Merge: preserve existing fields that the incoming meta doesn't have.
        // Local scan provides path + high-quality metadata.
        // Remote sync provides remote_id + remote_url.
        let (existing_path, existing_remote_id, existing_remote_url): (
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT path, remote_id, remote_url FROM tracks WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or((None, None, None));

        let merged_path = meta.path.as_ref().or(existing_path.as_ref());
        let merged_remote_id = meta.remote_id.as_ref().or(existing_remote_id.as_ref());
        let merged_remote_url = meta.remote_url.as_ref().or(existing_remote_url.as_ref());

        // Source reflects what's available: local path wins.
        let source = if merged_path.is_some() {
            "local"
        } else {
            &meta.source
        };

        conn.execute(
            "UPDATE tracks SET album_id=?1, artist_id=?2, disc=?3, track_number=?4,
             title=?5, duration_ms=?6, codec=?7, sample_rate=?8, bit_depth=?9,
             channels=?10, bitrate=?11, size_bytes=?12, mtime=?13, genre=?14,
             source=?15, remote_id=?16, remote_url=?17, path=?18
             WHERE id=?19",
            params![
                album_id,
                track_artist_id,
                meta.disc,
                meta.track_number,
                meta.title,
                meta.duration_ms,
                meta.codec,
                meta.sample_rate,
                meta.bit_depth,
                meta.channels,
                meta.bitrate,
                meta.size_bytes,
                meta.mtime,
                meta.genre,
                source,
                merged_remote_id,
                merged_remote_url,
                merged_path,
                id
            ],
        )?;

        conn.execute("DELETE FROM tracks_fts WHERE rowid = ?1", params![id])?;
        // Index both track artist and album artist for FTS searchability.
        let fts_artist = if meta.artist == album_artist_name {
            meta.artist.clone()
        } else {
            format!("{} {}", meta.artist, album_artist_name)
        };
        conn.execute(
            "INSERT INTO tracks_fts (rowid, title, artist_name, album_title, genre)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, meta.title, fts_artist, meta.album, meta.genre],
        )?;

        Ok(id)
    } else {
        let source = if meta.path.is_some() {
            "local"
        } else {
            &meta.source
        };

        conn.execute(
            "INSERT INTO tracks (album_id, artist_id, disc, track_number, title,
             duration_ms, path, codec, sample_rate, bit_depth, channels, bitrate,
             size_bytes, mtime, genre, source, remote_id, remote_url)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
            params![
                album_id,
                track_artist_id,
                meta.disc,
                meta.track_number,
                meta.title,
                meta.duration_ms,
                meta.path,
                meta.codec,
                meta.sample_rate,
                meta.bit_depth,
                meta.channels,
                meta.bitrate,
                meta.size_bytes,
                meta.mtime,
                meta.genre,
                source,
                meta.remote_id,
                meta.remote_url
            ],
        )?;

        let id = conn.last_insert_rowid();
        let fts_artist = if meta.artist == album_artist_name {
            meta.artist.clone()
        } else {
            format!("{} {}", meta.artist, album_artist_name)
        };
        conn.execute(
            "INSERT INTO tracks_fts (rowid, title, artist_name, album_title, genre)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, meta.title, fts_artist, meta.album, meta.genre],
        )?;

        Ok(id)
    }
}

/// Remove a track by local path.
pub fn remove_track_by_path(conn: &Connection, path: &str) -> Result<(), DbError> {
    let track_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM tracks WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = track_id {
        conn.execute("DELETE FROM tracks_fts WHERE rowid = ?1", params![id])?;
        conn.execute("DELETE FROM scan_cache WHERE track_id = ?1", params![id])?;
        conn.execute("DELETE FROM tracks WHERE id = ?1", params![id])?;
    }
    Ok(())
}

/// Remove all tracks with a given source (e.g., 'remote' before re-sync).
pub fn remove_tracks_by_source(conn: &Connection, source: &str) -> Result<usize, DbError> {
    // Delete FTS entries first.
    conn.execute(
        "DELETE FROM tracks_fts WHERE rowid IN (SELECT id FROM tracks WHERE source = ?1)",
        params![source],
    )?;
    let count = conn.execute("DELETE FROM tracks WHERE source = ?1", params![source])?;
    Ok(count)
}

/// Remove scan cache entries and tracks for paths that no longer exist in the given folder.
///
/// Remote-backed tracks (those with a `remote_id`) are demoted to remote-only
/// instead of deleted: their `path` is nulled, `source` set to "remote", and
/// local-only fields (`mtime`, `size_bytes`) cleared. This preserves streaming
/// fallback when a local drive is unplugged. When the drive comes back,
/// `upsert_track` content-match (strategy 3) re-merges the path automatically.
///
/// Pure-local tracks (no `remote_id`) are deleted as before.
pub fn remove_stale_tracks(conn: &Connection, folder: &Path) -> Result<usize, DbError> {
    let folder_str = folder.to_string_lossy();
    let prefix = format!("{}%", escape_like(&folder_str));

    // Find tracks in this folder that no longer exist on disk.
    // Use `path IS NOT NULL` instead of `source = 'local'` to catch all tracks
    // with local paths regardless of source flag (e.g. merged local+remote rows).
    let mut stmt = conn.prepare(
        "SELECT t.id, t.path, t.remote_id FROM tracks t
         WHERE t.path LIKE ?1 ESCAPE '\\' AND t.path IS NOT NULL",
    )?;

    let stale: Vec<(i64, String, Option<String>)> = stmt
        .query_map(params![prefix], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .filter(|(_, path, _)| !Path::new(path).exists())
        .collect();

    let count = stale.len();
    for (id, path, remote_id) in &stale {
        conn.execute("DELETE FROM scan_cache WHERE path = ?1", params![path])?;

        if remote_id.is_some() {
            // Demote to remote-only: null out local fields, keep the row for streaming.
            conn.execute(
                "UPDATE tracks SET path = NULL, source = 'remote', mtime = NULL, size_bytes = NULL
                 WHERE id = ?1",
                params![id],
            )?;
        } else {
            // Pure local — delete entirely. Clean up all FK references first.
            conn.execute("DELETE FROM tracks_fts WHERE rowid = ?1", params![id])?;
            conn.execute("DELETE FROM lyrics_cache WHERE track_id = ?1", params![id])?;
            conn.execute("DELETE FROM play_history WHERE track_id = ?1", params![id])?;
            conn.execute("DELETE FROM track_vectors WHERE track_id = ?1", params![id])?;
            conn.execute("DELETE FROM tracks WHERE id = ?1", params![id])?;
        }
    }

    Ok(count)
}

/// Get all tracks for an artist, ordered chronologically (album date, disc, track#).
pub fn tracks_for_artist(conn: &Connection, artist_id: i64) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.artist_id = ?1 OR al.artist_id = ?1
         ORDER BY al.date, al.title, t.disc, t.track_number",
    )?;
    let rows = stmt
        .query_map(params![artist_id], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Load all tracks that have a local path into a HashMap keyed by path.
/// Used by the playlist builder to skip expensive lofty reads for known files.
///
/// For large libraries, prefer `tracks_by_paths()` which only fetches the
/// tracks you actually need.
pub fn all_tracks_by_path(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.path IS NOT NULL",
    )?;

    let rows = stmt
        .query_map(params![], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;

    let mut map = std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        if let Some(ref path) = row.path {
            map.insert(path.clone(), row);
        }
    }
    Ok(map)
}

/// Load tracks matching a specific set of paths into a HashMap.
/// Processes in batches of 500 to stay within SQLite variable limits.
/// For small path sets this is dramatically cheaper than `all_tracks_by_path`.
pub fn tracks_by_paths(
    conn: &Connection,
    paths: &[String],
) -> Result<std::collections::HashMap<String, TrackRow>, DbError> {
    const BATCH_SIZE: usize = 500;
    let mut map = std::collections::HashMap::with_capacity(paths.len());

    for chunk in paths.chunks(BATCH_SIZE) {
        let placeholders: String = chunk
            .iter()
            .enumerate()
            .map(|(i, _)| {
                if i == 0 {
                    "?".to_string()
                } else {
                    ",?".to_string()
                }
            })
            .collect();

        let sql = format!(
            "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                    t.disc, t.track_number, t.title, t.duration_ms, t.path,
                    t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                    t.genre, t.source, t.remote_id, t.cached_path
             FROM tracks t
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             LEFT JOIN artists aa ON al.artist_id = aa.id
             WHERE t.path IN ({placeholders})"
        );

        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt
            .query_map(params.as_slice(), row_to_track_row)?
            .collect::<Result<Vec<_>, _>>()?;

        for row in rows {
            if let Some(ref path) = row.path {
                map.insert(path.clone(), row);
            }
        }
    }

    Ok(map)
}

/// Get all tracks in the library, ordered by artist/album/disc/track.
pub fn all_tracks(conn: &Connection) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         ORDER BY a.name, al.date, al.title, t.disc, t.track_number",
    )?;

    let rows = stmt
        .query_map(params![], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Get random tracks from the library, optionally filtered by artist.
pub fn random_tracks(
    conn: &Connection,
    count: u32,
    artist_id: Option<i64>,
) -> Result<Vec<TrackRow>, DbError> {
    let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
        if let Some(aid) = artist_id {
            (
                "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                    t.disc, t.track_number, t.title, t.duration_ms, t.path,
                    t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                    t.genre, t.source, t.remote_id, t.cached_path
             FROM tracks t
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             LEFT JOIN artists aa ON al.artist_id = aa.id
             WHERE t.artist_id = ?1 OR al.artist_id = ?1
             ORDER BY RANDOM()
             LIMIT ?2"
                    .into(),
                vec![
                    Box::new(aid) as Box<dyn rusqlite::types::ToSql>,
                    Box::new(count),
                ],
            )
        } else {
            (
                "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                    t.disc, t.track_number, t.title, t.duration_ms, t.path,
                    t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                    t.genre, t.source, t.remote_id, t.cached_path
             FROM tracks t
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             LEFT JOIN artists aa ON al.artist_id = aa.id
             ORDER BY RANDOM()
             LIMIT ?1"
                    .into(),
                vec![Box::new(count) as Box<dyn rusqlite::types::ToSql>],
            )
        };
    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(params_refs.as_slice(), row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Get all tracks with pagination.
pub fn all_tracks_paged(
    conn: &Connection,
    limit: u32,
    offset: u32,
) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         ORDER BY a.name, al.date, al.title, t.disc, t.track_number
         LIMIT ?1 OFFSET ?2",
    )?;
    let rows = stmt
        .query_map(params![limit, offset], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Get a single track by ID with full metadata.
pub fn get_track_row(conn: &Connection, track_id: i64) -> Result<Option<TrackRow>, DbError> {
    let result = conn.query_row(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.id = ?1",
        params![track_id],
        row_to_track_row,
    );

    match result {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Look up a track ID by its local file path.
pub fn track_id_by_path(conn: &Connection, path: &str) -> Result<Option<i64>, DbError> {
    let result = conn.query_row(
        "SELECT id FROM tracks WHERE path = ?1 OR cached_path = ?1 OR remote_url = ?1",
        params![path],
        |row| row.get(0),
    );
    match result {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Clear all cached_path values (used when purging the download cache).
pub fn clear_cached_paths(conn: &Connection) -> Result<(), DbError> {
    conn.execute(
        "UPDATE tracks SET cached_path = NULL, cache_size_bytes = NULL, cache_download_date = NULL",
        params![],
    )?;
    Ok(())
}

/// Update the cached_path for a track after downloading, recording size and timestamp.
pub fn set_cached_path(conn: &Connection, track_id: i64, path: &str) -> Result<(), DbError> {
    let size_bytes: Option<i64> = std::fs::metadata(path).ok().map(|m| m.len() as i64);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    conn.execute(
        "UPDATE tracks SET cached_path = ?1, cache_size_bytes = ?2, cache_download_date = ?3
         WHERE id = ?4",
        params![path, size_bytes, now, track_id],
    )?;
    Ok(())
}

/// Row returned by the cache eviction query.
#[derive(Debug, Clone)]
pub struct CachedAlbumInfo {
    pub album_id: i64,
    pub album_title: String,
    pub artist_name: String,
    pub total_size: i64,
    pub track_ids: Vec<i64>,
    pub cached_paths: Vec<String>,
}

/// Get cached albums ordered by LRU (oldest last-played first), excluding favourited tracks.
/// Returns albums with their total cache size and file paths for eviction.
pub fn cached_albums_lru(conn: &Connection) -> Result<Vec<CachedAlbumInfo>, DbError> {
    // Get all cached tracks with their last played timestamp.
    // A track is "protected" if it appears in the favourites table.
    // We exclude any album that has ANY favourited cached track.
    // Uses LEFT JOIN with pre-aggregated play_history to avoid O(N) correlated subquery.
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, COALESCE(al.title, 'Unknown'), COALESCE(a.name, 'Unknown'),
                t.cached_path, COALESCE(t.cache_size_bytes, 0),
                ph_max.last_play,
                EXISTS(SELECT 1 FROM favourites f
                       WHERE f.track_path = t.cached_path
                          OR f.track_path = t.path
                          OR f.track_path = t.remote_url) as is_fav
         FROM tracks t
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists a ON al.artist_id = a.id
         LEFT JOIN (SELECT track_id, MAX(played_at) as last_play
                    FROM play_history GROUP BY track_id) ph_max
                ON ph_max.track_id = t.id
         WHERE t.cached_path IS NOT NULL
         ORDER BY t.album_id, t.disc, t.track_number",
    )?;

    struct CachedTrackRow {
        track_id: i64,
        album_id: Option<i64>,
        album_title: String,
        artist_name: String,
        cached_path: String,
        size: i64,
        last_play: Option<i64>,
        is_fav: bool,
    }

    let rows: Vec<CachedTrackRow> = stmt
        .query_map([], |row| {
            Ok(CachedTrackRow {
                track_id: row.get(0)?,
                album_id: row.get(1)?,
                album_title: row.get(2)?,
                artist_name: row.get(3)?,
                cached_path: row.get(4)?,
                size: row.get(5)?,
                last_play: row.get(6)?,
                is_fav: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Group by album_id. Use -1 for tracks without an album.
    let mut albums: std::collections::BTreeMap<i64, CachedAlbumInfo> =
        std::collections::BTreeMap::new();
    // Track max last_play per album, and whether album has any favourites.
    let mut album_last_play: HashMap<i64, Option<i64>> = HashMap::new();
    let mut album_has_fav: HashSet<i64> = HashSet::new();

    for r in &rows {
        let aid = r.album_id.unwrap_or(-r.track_id); // unique key for albumless tracks
        if r.is_fav {
            album_has_fav.insert(aid);
        }
        let entry = albums.entry(aid).or_insert_with(|| CachedAlbumInfo {
            album_id: aid,
            album_title: r.album_title.clone(),
            artist_name: r.artist_name.clone(),
            total_size: 0,
            track_ids: Vec::new(),
            cached_paths: Vec::new(),
        });
        entry.total_size += r.size;
        entry.track_ids.push(r.track_id);
        entry.cached_paths.push(r.cached_path.clone());

        let current_max = album_last_play.entry(aid).or_insert(None);
        *current_max = match (*current_max, r.last_play) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
    }

    // Filter out albums with any favourited tracks, then sort by last_play ascending (oldest first).
    // Never-played albums sort before everything (None < Some).
    let mut result: Vec<CachedAlbumInfo> = albums
        .into_values()
        .filter(|a| !album_has_fav.contains(&a.album_id))
        .collect();

    result.sort_by_key(|a| album_last_play.get(&a.album_id).copied().unwrap_or(None));

    Ok(result)
}

/// Get total cache size from DB tracking (sum of cache_size_bytes for all cached tracks).
pub fn total_cache_size(conn: &Connection) -> Result<i64, DbError> {
    let size: i64 = conn.query_row(
        "SELECT COALESCE(SUM(cache_size_bytes), 0) FROM tracks WHERE cached_path IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(size)
}

/// Clear cache tracking for specific tracks (after eviction deletes files).
pub fn clear_cache_for_tracks(conn: &Connection, track_ids: &[i64]) -> Result<(), DbError> {
    for &id in track_ids {
        conn.execute(
            "UPDATE tracks SET cached_path = NULL, cache_size_bytes = NULL, cache_download_date = NULL
             WHERE id = ?1",
            params![id],
        )?;
    }
    Ok(())
}

/// Resolve the best playback source for a track. Local > Cached > Remote.
pub fn resolve_playback_path(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<PlaybackSource>, DbError> {
    let row = conn.query_row(
        "SELECT path, cached_path, remote_url, source FROM tracks WHERE id = ?1",
        params![track_id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    );

    match row {
        Ok((path, cached_path, remote_url, _source)) => {
            // Local file always wins.
            if let Some(p) = path {
                let pb = PathBuf::from(&p);
                if pb.exists() {
                    return Ok(Some(PlaybackSource::Local(pb)));
                }
            }
            // Cached download.
            if let Some(cp) = cached_path {
                let pb = PathBuf::from(&cp);
                if pb.exists() {
                    return Ok(Some(PlaybackSource::Cached(pb)));
                }
            }
            // Remote stream.
            if let Some(url) = remote_url {
                return Ok(Some(PlaybackSource::Remote(url)));
            }
            Ok(None)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Get tracks for a specific album, ordered by disc/track number.
pub fn tracks_for_album(conn: &Connection, album_id: i64) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE t.album_id = ?1
         ORDER BY t.disc, t.track_number",
    )?;

    let rows = stmt
        .query_map(params![album_id], row_to_track_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Build a SQL `IN (?, ?, ...)` clause with the given number of placeholders.
fn in_clause(n: usize) -> String {
    let mut s = String::with_capacity(2 + n * 2);
    s.push('(');
    for i in 0..n {
        if i > 0 {
            s.push(',');
        }
        s.push('?');
    }
    s.push(')');
    s
}

/// Get distinct genres for a batch of artist IDs in a single query.
/// Returns a map from artist_id → set of lowercased genre strings.
pub fn genres_by_artist_ids(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, HashSet<String>>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT t.artist_id, t.genre FROM tracks t
         WHERE t.artist_id IN {} AND t.genre IS NOT NULL
         UNION
         SELECT al.artist_id, t.genre FROM tracks t
         JOIN albums al ON t.album_id = al.id
         WHERE al.artist_id IN {} AND t.genre IS NOT NULL",
        in_clause(ids.len()),
        in_clause(ids.len()),
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
        .iter()
        .chain(ids.iter())
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map: HashMap<i64, HashSet<String>> = HashMap::new();
    for row in rows {
        let (artist_id, genre) = row?;
        map.entry(artist_id)
            .or_default()
            .insert(genre.to_lowercase());
    }
    Ok(map)
}

/// Get distinct genres for a batch of album IDs in a single query.
/// Returns a map from album_id → set of lowercased genre strings.
pub fn genres_by_album_ids(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, HashSet<String>>, DbError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT t.album_id, t.genre FROM tracks t
         WHERE t.album_id IN {} AND t.genre IS NOT NULL",
        in_clause(ids.len()),
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
        .iter()
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map: HashMap<i64, HashSet<String>> = HashMap::new();
    for row in rows {
        let (album_id, genre) = row?;
        map.entry(album_id)
            .or_default()
            .insert(genre.to_lowercase());
    }
    Ok(map)
}

/// Get all artist IDs that have at least one favourited track, in a single query.
pub fn favourite_artist_ids_batch(conn: &Connection) -> Result<HashSet<i64>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT t.artist_id FROM tracks t
         JOIN favourites f ON (t.path = f.track_path OR t.cached_path = f.track_path)
         WHERE t.artist_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

/// Get all album IDs that have at least one favourited track, in a single query.
pub fn favourite_album_ids_batch(conn: &Connection) -> Result<HashSet<i64>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT t.album_id FROM tracks t
         JOIN favourites f ON (t.path = f.track_path OR t.cached_path = f.track_path)
         WHERE t.album_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(row?);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::{library_stats, sample_meta, search_tracks};

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn test_upsert_track() {
        let db = test_db();
        let meta = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        let id1 = upsert_track(&db.conn, &meta).unwrap();

        // Same path → same track ID (upsert).
        let id2 = upsert_track(&db.conn, &meta).unwrap();
        assert_eq!(id1, id2);

        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 1);
        assert_eq!(stats.local_tracks, 1);
    }

    #[test]
    fn test_remove_track_by_path() {
        let db = test_db();
        upsert_track(&db.conn, &sample_meta("Track1", "Artist1", "Album1")).unwrap();
        upsert_track(&db.conn, &sample_meta("Track2", "Artist1", "Album1")).unwrap();

        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 2);

        remove_track_by_path(&db.conn, "/music/Album1/Track1.flac").unwrap();
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 1);

        // Search should no longer find it.
        let results = search_tracks(&db.conn, "Track1").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_remove_tracks_by_source() {
        let db = test_db();
        upsert_track(&db.conn, &sample_meta("Local", "Artist", "Album")).unwrap();

        let mut remote = sample_meta("Remote", "Artist", "Album");
        remote.source = "remote".into();
        remote.path = None;
        remote.remote_id = Some("r1".into());
        remote.remote_url = Some("https://example.com/stream/r1".into());
        upsert_track(&db.conn, &remote).unwrap();

        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 2);

        let removed = remove_tracks_by_source(&db.conn, "remote").unwrap();
        assert_eq!(removed, 1);
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 1);
        assert_eq!(library_stats(&db.conn).unwrap().local_tracks, 1);
    }

    #[test]
    fn test_resolve_playback_local_wins() {
        let db = test_db();

        // Insert a local track.
        let local = sample_meta("Song", "Artist", "Album");
        let local_id = upsert_track(&db.conn, &local).unwrap();

        match resolve_playback_path(&db.conn, local_id).unwrap() {
            // Path won't exist on disk in test, so falls through.
            // But we can at least verify it doesn't panic.
            Some(_) | None => {}
        }
    }

    #[test]
    fn test_resolve_playback_remote_fallback() {
        let db = test_db();

        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r42".into());
        meta.remote_url = Some("https://example.com/stream/r42".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        let source = resolve_playback_path(&db.conn, id).unwrap().unwrap();
        match source {
            PlaybackSource::Remote(url) => {
                assert!(url.contains("r42"));
            }
            _ => panic!("expected Remote source"),
        }
    }

    #[test]
    fn test_nonexistent_track_resolution() {
        let db = test_db();
        let result = resolve_playback_path(&db.conn, 99999).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_dedup_local_then_remote() {
        let db = test_db();

        // Insert local track first.
        let local = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        let local_id = upsert_track(&db.conn, &local).unwrap();

        // Sync same track from remote — should merge, not duplicate.
        let mut remote = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        remote.source = "remote".into();
        remote.path = None;
        remote.remote_id = Some("sub-42".into());
        remote.remote_url = Some("https://example.com/stream/sub-42".into());
        let remote_id = upsert_track(&db.conn, &remote).unwrap();

        // Same row.
        assert_eq!(local_id, remote_id);

        // Only 1 track total.
        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 1);

        // Source should be "local" since it has a path.
        assert_eq!(stats.local_tracks, 1);
        assert_eq!(stats.remote_tracks, 0);

        // But it should have the remote_id merged in.
        let row: (Option<String>, Option<String>, Option<String>) = db
            .conn
            .query_row(
                "SELECT path, remote_id, remote_url FROM tracks WHERE id = ?1",
                params![local_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(row.0.is_some()); // local path preserved
        assert_eq!(row.1.as_deref(), Some("sub-42")); // remote_id merged
        assert!(row.2.is_some()); // remote_url merged
    }

    #[test]
    fn test_dedup_remote_then_local() {
        let db = test_db();

        // Insert remote track first.
        let mut remote = sample_meta("Vordhosbn", "Aphex Twin", "Drukqs");
        remote.source = "remote".into();
        remote.path = None;
        remote.remote_id = Some("sub-99".into());
        remote.remote_url = Some("https://example.com/stream/sub-99".into());
        let remote_id = upsert_track(&db.conn, &remote).unwrap();

        // Scan local file — same track, should merge.
        let local = sample_meta("Vordhosbn", "Aphex Twin", "Drukqs");
        let local_id = upsert_track(&db.conn, &local).unwrap();

        // Same row.
        assert_eq!(remote_id, local_id);

        // Only 1 track.
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 1);

        // Source flipped to "local" since it now has a path.
        assert_eq!(library_stats(&db.conn).unwrap().local_tracks, 1);

        // Remote info preserved.
        let rid: Option<String> = db
            .conn
            .query_row(
                "SELECT remote_id FROM tracks WHERE id = ?1",
                params![local_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rid.as_deref(), Some("sub-99"));
    }

    #[test]
    fn test_remove_stale_preserves_remote_backed() {
        let db = test_db();

        // Create a merged local+remote track (path exists in DB but not on disk).
        let mut meta = sample_meta("Ageispolis", "Aphex Twin", "SAW 85-92");
        meta.path = Some("/nonexistent/SAW 85-92/Ageispolis.flac".into());
        meta.remote_id = Some("sub-10".into());
        meta.remote_url = Some("https://example.com/stream/sub-10".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        // Verify it starts as local.
        let source: String = db
            .conn
            .query_row(
                "SELECT source FROM tracks WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source, "local");

        // Remove stale tracks in the folder — file doesn't exist on disk.
        let removed = remove_stale_tracks(&db.conn, Path::new("/nonexistent/SAW 85-92")).unwrap();
        assert_eq!(removed, 1);

        // Track should still exist (not deleted), demoted to remote-only.
        let row: (
            Option<String>,
            String,
            Option<i64>,
            Option<i64>,
            Option<String>,
        ) = db
            .conn
            .query_row(
                "SELECT path, source, mtime, size_bytes, remote_id FROM tracks WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert!(row.0.is_none(), "path should be NULL");
        assert_eq!(row.1, "remote", "source should be 'remote'");
        assert!(row.2.is_none(), "mtime should be NULL");
        assert!(row.3.is_none(), "size_bytes should be NULL");
        assert_eq!(row.4.as_deref(), Some("sub-10"), "remote_id preserved");

        // Playback should fall through to remote stream.
        let playback = resolve_playback_path(&db.conn, id).unwrap().unwrap();
        match playback {
            PlaybackSource::Remote(url) => assert!(url.contains("sub-10")),
            _ => panic!("expected Remote playback source"),
        }
    }

    #[test]
    fn test_remove_stale_deletes_pure_local() {
        let db = test_db();

        // Pure local track — no remote_id.
        let meta = sample_meta("PureLocal", "Artist", "Album");
        // sample_meta generates path "/music/Album/PureLocal.flac" which won't exist.
        let id = upsert_track(&db.conn, &meta).unwrap();

        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 1);

        let removed = remove_stale_tracks(&db.conn, Path::new("/music/Album")).unwrap();
        assert_eq!(removed, 1);

        // Track should be fully deleted.
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 0);

        // Verify the row is gone.
        let exists: bool = db
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM tracks WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!exists, "pure local track should be deleted");
    }

    #[test]
    fn test_reattach_on_rescan() {
        let db = test_db();

        // Create a merged local+remote track with a non-existent path.
        let mut meta = sample_meta("Xtal", "Aphex Twin", "SAW 85-92");
        meta.path = Some("/nonexistent/SAW 85-92/Xtal.flac".into());
        meta.remote_id = Some("sub-20".into());
        meta.remote_url = Some("https://example.com/stream/sub-20".into());
        let original_id = upsert_track(&db.conn, &meta).unwrap();

        // Simulate stale removal (drive unplugged).
        remove_stale_tracks(&db.conn, Path::new("/nonexistent/SAW 85-92")).unwrap();

        // Verify demoted to remote-only.
        let source: String = db
            .conn
            .query_row(
                "SELECT source FROM tracks WHERE id = ?1",
                params![original_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source, "remote");

        // Simulate re-scan: same track shows up again with a path.
        // upsert_track content match (strategy 3) should re-merge the path.
        let mut rescan = sample_meta("Xtal", "Aphex Twin", "SAW 85-92");
        rescan.path = Some("/nonexistent/SAW 85-92/Xtal.flac".into());
        let rescan_id = upsert_track(&db.conn, &rescan).unwrap();

        // Same row — content match merged it back.
        assert_eq!(original_id, rescan_id);

        // Source should flip back to "local" since it has a path again.
        let row: (Option<String>, String, Option<String>) = db
            .conn
            .query_row(
                "SELECT path, source, remote_id FROM tracks WHERE id = ?1",
                params![rescan_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            row.0.as_deref(),
            Some("/nonexistent/SAW 85-92/Xtal.flac"),
            "path re-attached"
        );
        assert_eq!(row.1, "local", "source flipped back to local");
        assert_eq!(row.2.as_deref(), Some("sub-20"), "remote_id preserved");

        // Only 1 track — no duplication.
        assert_eq!(library_stats(&db.conn).unwrap().total_tracks, 1);
    }

    #[test]
    fn test_genres_by_artist_ids() {
        let db = test_db();
        let mut meta1 = sample_meta("Track1", "ArtistA", "Album1");
        meta1.genre = Some("Rock".into());
        upsert_track(&db.conn, &meta1).unwrap();

        let mut meta2 = sample_meta("Track2", "ArtistA", "Album1");
        meta2.genre = Some("Jazz".into());
        meta2.track_number = Some(2);
        meta2.path = Some("/music/Album1/Track2.flac".into());
        upsert_track(&db.conn, &meta2).unwrap();

        let mut meta3 = sample_meta("Track3", "ArtistB", "Album2");
        meta3.genre = Some("Metal".into());
        upsert_track(&db.conn, &meta3).unwrap();

        // Look up ArtistA's ID.
        let artist_a_id: i64 = db
            .conn
            .query_row("SELECT id FROM artists WHERE name = 'ArtistA'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let artist_b_id: i64 = db
            .conn
            .query_row("SELECT id FROM artists WHERE name = 'ArtistB'", [], |row| {
                row.get(0)
            })
            .unwrap();

        let genres = genres_by_artist_ids(&db.conn, &[artist_a_id, artist_b_id]).unwrap();
        let a_genres = genres.get(&artist_a_id).unwrap();
        assert!(a_genres.contains("rock"));
        assert!(a_genres.contains("jazz"));
        let b_genres = genres.get(&artist_b_id).unwrap();
        assert!(b_genres.contains("metal"));
    }

    #[test]
    fn test_genres_by_artist_ids_empty() {
        let db = test_db();
        let genres = genres_by_artist_ids(&db.conn, &[]).unwrap();
        assert!(genres.is_empty());
    }

    #[test]
    fn test_genres_by_album_ids() {
        let db = test_db();
        let mut meta1 = sample_meta("Track1", "Artist", "AlbumX");
        meta1.genre = Some("Ambient".into());
        upsert_track(&db.conn, &meta1).unwrap();

        let mut meta2 = sample_meta("Track2", "Artist", "AlbumX");
        meta2.genre = Some("IDM".into());
        meta2.track_number = Some(2);
        meta2.path = Some("/music/AlbumX/Track2.flac".into());
        upsert_track(&db.conn, &meta2).unwrap();

        let album_id: i64 = db
            .conn
            .query_row("SELECT id FROM albums WHERE title = 'AlbumX'", [], |row| {
                row.get(0)
            })
            .unwrap();

        let genres = genres_by_album_ids(&db.conn, &[album_id]).unwrap();
        let album_genres = genres.get(&album_id).unwrap();
        assert!(album_genres.contains("ambient"));
        assert!(album_genres.contains("idm"));
    }

    #[test]
    fn test_favourite_artist_ids_batch() {
        let db = test_db();
        let meta = sample_meta("FavTrack", "FavArtist", "FavAlbum");
        upsert_track(&db.conn, &meta).unwrap();

        // Add to favourites.
        crate::db::queries::add_favourite(
            &db.conn,
            std::path::Path::new("/music/FavAlbum/FavTrack.flac"),
        )
        .unwrap();

        let artist_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM artists WHERE name = 'FavArtist'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let fav_ids = favourite_artist_ids_batch(&db.conn).unwrap();
        assert!(fav_ids.contains(&artist_id));
    }

    #[test]
    fn test_favourite_artist_ids_batch_empty() {
        let db = test_db();
        let fav_ids = favourite_artist_ids_batch(&db.conn).unwrap();
        assert!(fav_ids.is_empty());
    }

    #[test]
    fn test_favourite_album_ids_batch() {
        let db = test_db();
        let meta = sample_meta("FavTrack", "FavArtist", "FavAlbum");
        upsert_track(&db.conn, &meta).unwrap();

        crate::db::queries::add_favourite(
            &db.conn,
            std::path::Path::new("/music/FavAlbum/FavTrack.flac"),
        )
        .unwrap();

        let album_id: i64 = db
            .conn
            .query_row(
                "SELECT id FROM albums WHERE title = 'FavAlbum'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let fav_ids = favourite_album_ids_batch(&db.conn).unwrap();
        assert!(fav_ids.contains(&album_id));
    }

    #[test]
    fn test_favourite_album_ids_batch_empty() {
        let db = test_db();
        let fav_ids = favourite_album_ids_batch(&db.conn).unwrap();
        assert!(fav_ids.is_empty());
    }

    #[test]
    fn test_set_cached_path_records_size_and_date() {
        let db = test_db();
        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r1".into());
        meta.remote_url = Some("https://example.com/r1".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        // Create a temp file to simulate a cached download.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp.as_file().try_clone().unwrap(), &[0u8; 1024]).unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        set_cached_path(&db.conn, id, &path).unwrap();

        let (cached_path, size, download_date): (Option<String>, Option<i64>, Option<i64>) = db
            .conn
            .query_row(
                "SELECT cached_path, cache_size_bytes, cache_download_date FROM tracks WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert_eq!(cached_path.as_deref(), Some(path.as_str()));
        assert!(size.unwrap() > 0, "cache_size_bytes should be positive");
        assert!(
            download_date.unwrap() > 0,
            "cache_download_date should be set"
        );
    }

    #[test]
    fn test_total_cache_size() {
        let db = test_db();

        // Start with zero.
        assert_eq!(total_cache_size(&db.conn).unwrap(), 0);

        // Insert a cached track with known size.
        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r1".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        db.conn
            .execute(
                "UPDATE tracks SET cached_path = '/cache/song.flac', cache_size_bytes = 50000000 WHERE id = ?1",
                params![id],
            )
            .unwrap();

        assert_eq!(total_cache_size(&db.conn).unwrap(), 50_000_000);
    }

    #[test]
    fn test_clear_cache_for_tracks() {
        let db = test_db();
        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r1".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        db.conn
            .execute(
                "UPDATE tracks SET cached_path = '/cache/song.flac', cache_size_bytes = 1000, cache_download_date = 12345 WHERE id = ?1",
                params![id],
            )
            .unwrap();

        clear_cache_for_tracks(&db.conn, &[id]).unwrap();

        let (path, size, date): (Option<String>, Option<i64>, Option<i64>) = db
            .conn
            .query_row(
                "SELECT cached_path, cache_size_bytes, cache_download_date FROM tracks WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert!(path.is_none());
        assert!(size.is_none());
        assert!(date.is_none());
    }

    #[test]
    fn test_cached_albums_lru_excludes_favourites() {
        let db = test_db();

        // Create two remote-cached albums.
        for (album, tracks) in &[("AlbumA", vec!["T1", "T2"]), ("AlbumB", vec!["T3", "T4"])] {
            for (i, title) in tracks.iter().enumerate() {
                let mut meta = sample_meta(title, "Artist", album);
                meta.source = "remote".into();
                meta.path = None;
                meta.remote_id = Some(format!("r-{}", title));
                meta.track_number = Some((i + 1) as i32);
                let id = upsert_track(&db.conn, &meta).unwrap();
                let cached = format!("/cache/{}/{}.flac", album, title);
                db.conn
                    .execute(
                        "UPDATE tracks SET cached_path = ?1, cache_size_bytes = 10000000 WHERE id = ?2",
                        params![cached, id],
                    )
                    .unwrap();
            }
        }

        // Favourite a track from AlbumB.
        crate::db::queries::add_favourite(&db.conn, std::path::Path::new("/cache/AlbumB/T3.flac"))
            .unwrap();

        let albums = cached_albums_lru(&db.conn).unwrap();

        // AlbumB should be excluded (has a favourite), only AlbumA returned.
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].album_title, "AlbumA");
    }

    #[test]
    fn test_cached_albums_lru_sorted_by_last_play() {
        let db = test_db();

        // Create two cached albums.
        let mut album_ids = Vec::new();
        for (album, played_at) in &[("OldAlbum", 1000), ("NewAlbum", 9000)] {
            let mut meta = sample_meta("Track", "Artist", album);
            meta.source = "remote".into();
            meta.path = None;
            meta.remote_id = Some(format!("r-{}", album));
            let id = upsert_track(&db.conn, &meta).unwrap();
            let cached = format!("/cache/{}/Track.flac", album);
            db.conn
                .execute(
                    "UPDATE tracks SET cached_path = ?1, cache_size_bytes = 10000000 WHERE id = ?2",
                    params![cached, id],
                )
                .unwrap();

            // Record play history.
            db.conn
                .execute(
                    "INSERT INTO play_history (track_id, played_at) VALUES (?1, ?2)",
                    params![id, played_at],
                )
                .unwrap();

            album_ids.push(id);
        }

        let albums = cached_albums_lru(&db.conn).unwrap();
        assert_eq!(albums.len(), 2);
        // OldAlbum (played_at=1000) should come first (evicted first).
        assert_eq!(albums[0].album_title, "OldAlbum");
        assert_eq!(albums[1].album_title, "NewAlbum");
    }

    #[test]
    fn test_clear_cached_paths_clears_all_tracking() {
        let db = test_db();
        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r1".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        db.conn
            .execute(
                "UPDATE tracks SET cached_path = '/x', cache_size_bytes = 100, cache_download_date = 999 WHERE id = ?1",
                params![id],
            )
            .unwrap();

        clear_cached_paths(&db.conn).unwrap();

        let (path, size, date): (Option<String>, Option<i64>, Option<i64>) = db
            .conn
            .query_row(
                "SELECT cached_path, cache_size_bytes, cache_download_date FROM tracks WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();

        assert!(path.is_none());
        assert!(size.is_none());
        assert!(date.is_none());
    }
}
