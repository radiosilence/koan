use std::collections::HashMap;
use std::path::PathBuf;

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

/// The scan cache entries for the files under `scope`, as path → (mtime,
/// size). Each scope is a directory, whose files' entries are read, or a file,
/// whose own is. A scan of one album directory reads that directory's entries
/// rather than the library's.
pub fn load_scan_cache(
    conn: &Connection,
    scope: &[PathBuf],
) -> Result<HashMap<String, (i64, i64)>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT path, mtime, size FROM scan_cache WHERE path = ?1
         UNION ALL
         SELECT path, mtime, size FROM scan_cache WHERE path >= ?2 AND path < ?3",
    )?;
    let mut map = HashMap::new();
    for dir in crate::index::scanner::minimal_dirs(scope.to_vec()) {
        let (lower, upper) = super::folder_prefix_range(&dir);
        let rows = stmt.query_map(params![dir.to_string_lossy(), lower, upper], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
            ))
        })?;
        for row in rows {
            let (path, data) = row?;
            map.insert(path, data);
        }
    }
    Ok(map)
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
    fn the_cache_is_read_for_the_scope_only() {
        let db = test_db();
        let id = upsert_track(&db.conn, &sample_meta("T", "A", "Al")).unwrap();
        for (path, mtime) in [
            ("/music/Al/T1.flac", 1),
            ("/music/Al/Disc 2/T2.flac", 2),
            ("/music/Al Live/T3.flac", 3),
            ("/music/Other/T4.flac", 4),
            ("/elsewhere/T5.flac", 5),
        ] {
            update_scan_cache(&db.conn, path, mtime, 10, id).unwrap();
        }

        let cache = load_scan_cache(
            &db.conn,
            &[
                PathBuf::from("/music/Al"),
                PathBuf::from("/music/Al/Disc 2"),
                PathBuf::from("/elsewhere/T5.flac"),
            ],
        )
        .unwrap();
        let mut paths: Vec<&str> = cache.keys().map(String::as_str).collect();
        paths.sort();
        assert_eq!(
            paths,
            [
                "/elsewhere/T5.flac",
                "/music/Al/Disc 2/T2.flac",
                "/music/Al/T1.flac"
            ]
        );
        assert_eq!(cache["/music/Al/T1.flac"], (1, 10));
    }
}
