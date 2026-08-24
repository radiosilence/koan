use rusqlite::Connection;

use crate::db::connection::DbError;

use super::LibraryStats;

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
    // A downloaded track keeps `source = 'remote'` — where it came from does
    // not change because a copy landed on disk. What changes is `cached_path`,
    // which is what `set_cached_path` writes and `clear_download_cache` nulls.
    // 'cached' is a source the CHECK constraint allows and nothing writes.
    let cached_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE cached_path IS NOT NULL",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::{sample_meta, upsert_track};

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
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

    /// Downloading does not change where a track came from, so counting
    /// `source = 'cached'` counted nothing — a library with a full cache
    /// reported "0 of 47,944 cached" and always would have.
    #[test]
    fn a_downloaded_track_counts_as_cached_and_stays_remote() {
        let db = test_db();
        let mut meta = sample_meta("Song", "Artist", "Album");
        meta.source = "remote".into();
        meta.path = None;
        meta.remote_id = Some("r1".into());
        let id = upsert_track(&db.conn, &meta).unwrap();

        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.remote_tracks, 1);
        assert_eq!(stats.cached_tracks, 0, "nothing downloaded yet");

        let file = std::env::temp_dir().join("koan-stats-cached.mp3");
        std::fs::write(&file, b"not really audio").unwrap();
        super::super::set_cached_path(&db.conn, id, &file.to_string_lossy()).unwrap();

        let stats = library_stats(&db.conn).unwrap();
        assert_eq!(stats.cached_tracks, 1);
        assert_eq!(
            stats.remote_tracks, 1,
            "a cached copy does not change where the track came from"
        );

        super::super::clear_cached_paths(&db.conn).unwrap();
        assert_eq!(library_stats(&db.conn).unwrap().cached_tracks, 0);
        let _ = std::fs::remove_file(&file);
    }
}
