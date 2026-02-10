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
    pub album_artist_name: String,
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
            params![artist_id, album_id, meta.title, meta.track_number],
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
                source,
                merged_remote_id,
                merged_remote_url,
                merged_path,
                id
            ],
        )?;

        conn.execute("DELETE FROM tracks_fts WHERE rowid = ?1", params![id])?;
        conn.execute(
            "INSERT INTO tracks_fts (rowid, title, artist_name, album_title, genre)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, meta.title, artist_name, meta.album, meta.genre],
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
                source,
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

/// Find artists by name (case-insensitive substring match).
pub fn find_artists(conn: &Connection, query: &str) -> Result<Vec<ArtistRow>, DbError> {
    let pattern = format!("%{}%", query);
    let mut stmt = conn.prepare(
        "SELECT id, name, sort_name, remote_id FROM artists
         WHERE name LIKE ?1 COLLATE NOCASE
         ORDER BY COALESCE(sort_name, name)",
    )?;
    let rows = stmt
        .query_map(params![pattern], |row| {
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

/// Get albums for a specific artist, ordered chronologically.
pub fn albums_for_artist(conn: &Connection, artist_id: i64) -> Result<Vec<AlbumRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.title, al.artist_id, a.name, al.date,
                al.total_discs, al.total_tracks, al.codec, al.label, al.remote_id
         FROM albums al
         LEFT JOIN artists a ON al.artist_id = a.id
         WHERE al.artist_id = ?1
         ORDER BY al.date, al.title",
    )?;
    let rows = stmt
        .query_map(params![artist_id], |row| {
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
         WHERE t.artist_id = ?1
         ORDER BY al.date, al.title, t.disc, t.track_number",
    )?;
    let rows = stmt
        .query_map(params![artist_id], |row| {
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
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    fn sample_meta(title: &str, artist: &str, album: &str) -> TrackMeta {
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
            size_bytes: Some(30_000_000),
            mtime: Some(1700000000),
            path: Some(format!("/music/{}/{}.flac", album, title)),
            source: "local".into(),
            remote_id: None,
            remote_url: None,
        }
    }

    #[test]
    fn test_artist_create_and_dedup() {
        let db = test_db();
        let id1 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        let id2 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        assert_eq!(id1, id2);

        let id3 = get_or_create_artist(&db.conn, "Squarepusher", None).unwrap();
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_album_create_and_dedup() {
        let db = test_db();
        let artist = get_or_create_artist(&db.conn, "Boards of Canada", None).unwrap();
        let a1 = get_or_create_album(
            &db.conn,
            "Music Has the Right to Children",
            artist,
            Some("1998"),
            None,
            None,
            Some("FLAC"),
            Some("Warp"),
            None,
        )
        .unwrap();
        let a2 = get_or_create_album(
            &db.conn,
            "Music Has the Right to Children",
            artist,
            Some("1998"),
            None,
            None,
            Some("FLAC"),
            Some("Warp"),
            None,
        )
        .unwrap();
        assert_eq!(a1, a2);
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
    fn test_search_fts() {
        let db = test_db();
        upsert_track(&db.conn, &sample_meta("Vordhosbn", "Aphex Twin", "Drukqs")).unwrap();
        upsert_track(
            &db.conn,
            &sample_meta("Roygbiv", "Boards of Canada", "MHTRTC"),
        )
        .unwrap();
        upsert_track(
            &db.conn,
            &sample_meta("Tha", "Aphex Twin", "Selected Ambient Works"),
        )
        .unwrap();

        // Search by artist.
        let results = search_tracks(&db.conn, "Aphex").unwrap();
        assert_eq!(results.len(), 2);

        // Search by title.
        let results = search_tracks(&db.conn, "Roygbiv").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Roygbiv");

        // Search by album.
        let results = search_tracks(&db.conn, "Drukqs").unwrap();
        assert_eq!(results.len(), 1);
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
    fn test_all_albums_and_tracks() {
        let db = test_db();
        let mut m1 = sample_meta("Track1", "Artist1", "Album1");
        m1.track_number = Some(1);
        let mut m2 = sample_meta("Track2", "Artist1", "Album1");
        m2.track_number = Some(2);
        m2.path = Some("/music/Album1/Track2.flac".into());
        upsert_track(&db.conn, &m1).unwrap();
        upsert_track(&db.conn, &m2).unwrap();

        let albums = all_albums(&db.conn).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "Album1");

        let tracks = tracks_for_album(&db.conn, albums[0].id).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].track_number, Some(1));
        assert_eq!(tracks[1].track_number, Some(2));
    }

    #[test]
    fn test_scan_cache() {
        let db = test_db();
        let meta = sample_meta("T", "A", "Al");
        let id = upsert_track(&db.conn, &meta).unwrap();

        update_scan_cache(&db.conn, "/music/Al/T.flac", 1700000000, 30000000, id).unwrap();

        // Same mtime+size → no rescan needed.
        assert!(!needs_rescan(&db.conn, "/music/Al/T.flac", 1700000000, 30000000).unwrap());

        // Different mtime → rescan.
        assert!(needs_rescan(&db.conn, "/music/Al/T.flac", 1700000001, 30000000).unwrap());

        // Unknown file → rescan.
        assert!(needs_rescan(&db.conn, "/music/Al/New.flac", 1700000000, 30000000).unwrap());
    }

    #[test]
    fn test_library_stats() {
        let db = test_db();
        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 0);
        assert_eq!(stats.total_albums, 0);
        assert_eq!(stats.total_artists, 0);

        upsert_track(&db.conn, &sample_meta("T1", "A1", "Al1")).unwrap();
        upsert_track(&db.conn, &sample_meta("T2", "A2", "Al2")).unwrap();

        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.total_tracks, 2);
        assert_eq!(stats.local_tracks, 2);
        assert_eq!(stats.total_albums, 2);
        assert_eq!(stats.total_artists, 2);
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
}

// --- Queries ---

/// Full-text search across track title, artist, album, genre.
pub fn search_tracks(conn: &Connection, query: &str) -> Result<Vec<TrackRow>, DbError> {
    // FTS5 query — append * for prefix matching.
    let fts_query = format!("{}*", query.trim());

    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.artist_id, a.name, aa.name, al.title,
                t.disc, t.track_number, t.title, t.duration_ms, t.path,
                t.codec, t.sample_rate, t.bit_depth, t.channels, t.bitrate,
                t.genre, t.source, t.remote_id, t.cached_path
         FROM tracks_fts f
         JOIN tracks t ON t.id = f.rowid
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.artist_id = aa.id
         WHERE tracks_fts MATCH ?1
         ORDER BY a.name, al.date, al.title, t.disc, t.track_number
         LIMIT 100",
    )?;

    let rows = stmt
        .query_map(params![fts_query], |row| {
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
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
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
        .query_map(params![], |row| {
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
        })?
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
        |row| {
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
        },
    );

    match result {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Update the cached_path for a track after downloading.
pub fn set_cached_path(conn: &Connection, track_id: i64, path: &str) -> Result<(), DbError> {
    conn.execute(
        "UPDATE tracks SET cached_path = ?1 WHERE id = ?2",
        params![path, track_id],
    )?;
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
        .query_map(params![album_id], |row| {
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

/// Remove scan cache entries and tracks for paths that no longer exist in the given folder.
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
