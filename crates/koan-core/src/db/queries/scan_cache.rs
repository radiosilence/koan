use rusqlite::{Connection, params};

use crate::db::connection::DbError;

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
}
