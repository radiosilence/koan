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

/// Load the entire scan cache into a HashMap for batch lookups.
/// Returns path → (mtime, size) for all cached entries.
pub fn load_scan_cache(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, (i64, i64)>, DbError> {
    let mut stmt = conn.prepare("SELECT path, mtime, size FROM scan_cache")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
        ))
    })?;

    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (path, data) = row?;
        map.insert(path, data);
    }
    Ok(map)
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

    #[test]
    fn test_load_scan_cache() {
        let db = test_db();
        let meta = sample_meta("T1", "A", "Al");
        let id1 = upsert_track(&db.conn, &meta).unwrap();
        let meta2 = sample_meta("T2", "A", "Al");
        let id2 = upsert_track(&db.conn, &meta2).unwrap();

        update_scan_cache(&db.conn, "/music/Al/T1.flac", 100, 200, id1).unwrap();
        update_scan_cache(&db.conn, "/music/Al/T2.flac", 300, 400, id2).unwrap();

        let cache = load_scan_cache(&db.conn).unwrap();
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("/music/Al/T1.flac"), Some(&(100, 200)));
        assert_eq!(cache.get("/music/Al/T2.flac"), Some(&(300, 400)));
        assert_eq!(cache.get("/music/Al/T3.flac"), None);
    }
}
