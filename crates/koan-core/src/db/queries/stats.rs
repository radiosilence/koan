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
}
