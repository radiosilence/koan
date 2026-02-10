use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use super::connection::DbError;

// --- Row types ---

#[derive(Debug, Clone)]
pub struct ArtistRow {
    pub id: i64,
    pub name: String,
    pub sort_name: Option<String>,
    pub remote_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AlbumRow {
    pub id: i64,
    pub title: String,
    pub artist_id: i64,
    pub artist_name: String,
    pub date: Option<String>,
    pub total_discs: Option<i32>,
    pub total_tracks: Option<i32>,
    pub codec: Option<String>,
    pub label: Option<String>,
    pub remote_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TrackRow {
    pub id: i64,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub artist_name: String,
    pub album_title: String,
    pub disc: Option<i32>,
    pub track_number: Option<i32>,
    pub title: String,
    pub duration_ms: Option<i64>,
    pub path: Option<String>,
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub bitrate: Option<i32>,
    pub genre: Option<String>,
    pub source: String,
    pub remote_id: Option<String>,
    pub cached_path: Option<String>,
}

/// Where to get audio data for playback. Local always wins.
#[derive(Debug, Clone)]
pub enum PlaybackSource {
    Local(PathBuf),
    Cached(PathBuf),
    Remote(String),
}

#[derive(Debug, Clone, Default)]
pub struct LibraryStats {
    pub total_tracks: i64,
    pub local_tracks: i64,
    pub remote_tracks: i64,
    pub cached_tracks: i64,
    pub total_albums: i64,
    pub total_artists: i64,
}

// --- Mutations ---

/// Get or create an artist by name. Returns the artist ID.
pub fn get_or_create_artist(
    conn: &Connection,
    name: &str,
    remote_id: Option<&str>,
) -> Result<i64, DbError> {
    // Try to find existing.
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM artists WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        // Update remote_id if we have one and the existing doesn't.
        if let Some(rid) = remote_id {
            conn.execute(
                "UPDATE artists SET remote_id = ?1 WHERE id = ?2 AND remote_id IS NULL",
                params![rid, id],
            )?;
        }
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO artists (name, remote_id) VALUES (?1, ?2)",
        params![name, remote_id],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Get or create an album by title + artist. Returns the album ID.
#[allow(clippy::too_many_arguments)]
pub fn get_or_create_album(
    conn: &Connection,
    title: &str,
    artist_id: i64,
    date: Option<&str>,
    total_discs: Option<i32>,
    total_tracks: Option<i32>,
    codec: Option<&str>,
    label: Option<&str>,
    remote_id: Option<&str>,
) -> Result<i64, DbError> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM albums WHERE title = ?1 AND artist_id = ?2",
            params![title, artist_id],
            |row| row.get(0),
        )
        .ok();

    if let Some(id) = existing {
        if let Some(rid) = remote_id {
            conn.execute(
                "UPDATE albums SET remote_id = ?1 WHERE id = ?2 AND remote_id IS NULL",
                params![rid, id],
            )?;
        }
        return Ok(id);
    }

    conn.execute(
        "INSERT INTO albums (title, artist_id, date, total_discs, total_tracks, codec, label, remote_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![title, artist_id, date, total_discs, total_tracks, codec, label, remote_id],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Metadata for inserting/updating a track.
#[derive(Debug, Clone)]
pub struct TrackMeta {
    pub title: String,
    pub artist: String,
    pub album_artist: Option<String>,
    pub album: String,
    pub date: Option<String>,
    pub disc: Option<i32>,
    pub track_number: Option<i32>,
    pub genre: Option<String>,
    pub label: Option<String>,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub bitrate: Option<i32>,
    pub size_bytes: Option<i64>,
    pub mtime: Option<i64>,
    pub path: Option<String>,
    pub source: String,
    pub remote_id: Option<String>,
    pub remote_url: Option<String>,
}

/// Insert or update a track. Auto-creates artist/album.
pub fn upsert_track(conn: &Connection, meta: &TrackMeta) -> Result<i64, DbError> {
    let artist_name = meta.album_artist.as_deref().unwrap_or(&meta.artist);
    let artist_id = get_or_create_artist(conn, artist_name, None)?;
    let album_id = get_or_create_album(
        conn,
        &meta.album,
        artist_id,
        meta.date.as_deref(),
        None,
        None,
        meta.codec.as_deref(),
        meta.label.as_deref(),
        None,
    )?;

    // Use path as the unique key for local tracks, remote_id for remote.
    let track_id: Option<i64> = if let Some(ref path) = meta.path {
        conn.query_row(
            "SELECT id FROM tracks WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )
        .ok()
    } else if let Some(ref rid) = meta.remote_id {
        conn.query_row(
            "SELECT id FROM tracks WHERE remote_id = ?1",
            params![rid],
            |row| row.get(0),
        )
        .ok()
    } else {
        None
    };

    if let Some(id) = track_id {
        conn.execute(
            "UPDATE tracks SET album_id=?1, artist_id=?2, disc=?3, track_number=?4,
             title=?5, duration_ms=?6, codec=?7, sample_rate=?8, bit_depth=?9,
             channels=?10, bitrate=?11, size_bytes=?12, mtime=?13, genre=?14,
             source=?15, remote_id=?16, remote_url=?17, path=?18
             WHERE id=?19",
            params![
                album_id,
                artist_id,
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
                meta.source,
                meta.remote_id,
                meta.remote_url,
                meta.path,
                id
            ],
        )?;

        // Update FTS.
        conn.execute("DELETE FROM tracks_fts WHERE rowid = ?1", params![id])?;
        conn.execute(
            "INSERT INTO tracks_fts (rowid, title, artist_name, album_title, genre)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, meta.title, artist_name, meta.album, meta.genre],
        )?;

        Ok(id)
    } else {
        conn.execute(
            "INSERT INTO tracks (album_id, artist_id, disc, track_number, title,
             duration_ms, path, codec, sample_rate, bit_depth, channels, bitrate,
             size_bytes, mtime, genre, source, remote_id, remote_url)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
            params![
                album_id,
                artist_id,
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
                meta.source,
                meta.remote_id,
                meta.remote_url
            ],
        )?;

        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO tracks_fts (rowid, title, artist_name, album_title, genre)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, meta.title, artist_name, meta.album, meta.genre],
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

// --- Queries ---

/// Full-text search across track title, artist, album, genre.
pub fn search_tracks(conn: &Connection, query: &str) -> Result<Vec<TrackRow>, DbError> {
    // FTS5 query — append * for prefix matching.
    let fts_query = format!("{}*", query.trim());

    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks_fts f
         JOIN tracks t ON t.id = f.rowid
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE tracks_fts MATCH ?1
         ORDER BY rank
         LIMIT 100",
    )?;

    let rows = stmt
        .query_map(params![fts_query], |row| {
            Ok(TrackRow {
                id: row.get(0)?,
                album_id: row.get(1)?,
                artist_id: row.get(2)?,
                artist_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                album_title: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                disc: row.get(5)?,
                track_number: row.get(6)?,
                title: row.get(7)?,
                duration_ms: row.get(8)?,
                path: row.get(9)?,
                codec: row.get(10)?,
                sample_rate: row.get(11)?,
                bit_depth: row.get(12)?,
                channels: row.get(13)?,
                bitrate: row.get(14)?,
                genre: row.get(15)?,
                source: row.get(16)?,
                remote_id: row.get(17)?,
                cached_path: row.get(18)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
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

/// Get all albums with their artist name, sorted.
pub fn all_albums(conn: &Connection) -> Result<Vec<AlbumRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         ORDER BY a.name, al.date, al.title",
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok(AlbumRow {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_id: row.get(2)?,
                artist_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                date: row.get(4)?,
                total_discs: row.get(5)?,
                total_tracks: row.get(6)?,
                codec: row.get(7)?,
                label: row.get(8)?,
                remote_id: row.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Get tracks for a specific album, ordered by disc/track number.
pub fn tracks_for_album(conn: &Connection, album_id: i64) -> Result<Vec<TrackRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.album_id = ?1
         ORDER BY t.disc, t.track_number",
    )?;

    let rows = stmt
        .query_map(params![album_id], |row| {
            Ok(TrackRow {
                id: row.get(0)?,
                album_id: row.get(1)?,
                artist_id: row.get(2)?,
                artist_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                album_title: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                disc: row.get(5)?,
                track_number: row.get(6)?,
                title: row.get(7)?,
                duration_ms: row.get(8)?,
                path: row.get(9)?,
                codec: row.get(10)?,
                sample_rate: row.get(11)?,
                bit_depth: row.get(12)?,
                channels: row.get(13)?,
                bitrate: row.get(14)?,
                genre: row.get(15)?,
                source: row.get(16)?,
                remote_id: row.get(17)?,
                cached_path: row.get(18)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// All artists, sorted by name.
pub fn all_artists(conn: &Connection) -> Result<Vec<ArtistRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, sort_name, remote_id FROM artists ORDER BY COALESCE(sort_name, name)",
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok(ArtistRow {
                id: row.get(0)?,
                name: row.get(1)?,
                sort_name: row.get(2)?,
                remote_id: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Library statistics broken down by source.
pub fn library_stats(conn: &Connection) -> Result<LibraryStats, DbError> {
    let total_tracks: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
    let local_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE source = 'local'",
        [],
        |r| r.get(0),
    )?;
    let remote_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE source = 'remote'",
        [],
        |r| r.get(0),
    )?;
    let cached_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE source = 'cached'",
        [],
        |r| r.get(0),
    )?;
    let total_albums: i64 = conn.query_row("SELECT COUNT(*) FROM albums", [], |r| r.get(0))?;
    let total_artists: i64 = conn.query_row("SELECT COUNT(*) FROM artists", [], |r| r.get(0))?;

    Ok(LibraryStats {
        total_tracks,
        local_tracks,
        remote_tracks,
        cached_tracks,
        total_albums,
        total_artists,
    })
}

/// Update the scan cache entry for a file.
pub fn update_scan_cache(
    conn: &Connection,
    path: &str,
    mtime: i64,
    size: i64,
    track_id: i64,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT OR REPLACE INTO scan_cache (path, mtime, size, track_id) VALUES (?1, ?2, ?3, ?4)",
        params![path, mtime, size, track_id],
    )?;
    Ok(())
}

/// Check if a file needs re-scanning (mtime or size changed).
pub fn needs_rescan(conn: &Connection, path: &str, mtime: i64, size: i64) -> Result<bool, DbError> {
    let cached = conn.query_row(
        "SELECT mtime, size FROM scan_cache WHERE path = ?1",
        params![path],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    );

    match cached {
        Ok((cached_mtime, cached_size)) => Ok(mtime != cached_mtime || size != cached_size),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(true),
        Err(e) => Err(e.into()),
    }
}

/// Remove scan cache entries for paths that no longer exist in the given folder.
pub fn remove_stale_tracks(conn: &Connection, folder: &Path) -> Result<usize, DbError> {
    let folder_str = folder.to_string_lossy();
    let prefix = format!("{}%", folder_str);

    // Find tracks in this folder that no longer exist on disk.
    let mut stmt = conn.prepare(
        "SELECT t.id, t.path FROM tracks t
         WHERE t.path LIKE ?1 AND t.source = 'local'",
    )?;

    let stale: Vec<(i64, String)> = stmt
        .query_map(params![prefix], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .filter(|(_, path)| !Path::new(path).exists())
        .collect();

    let count = stale.len();
    for (id, path) in &stale {
        conn.execute("DELETE FROM tracks_fts WHERE rowid = ?1", params![id])?;
        conn.execute("DELETE FROM scan_cache WHERE path = ?1", params![path])?;
        conn.execute("DELETE FROM tracks WHERE id = ?1", params![id])?;
    }

    Ok(count)
}
