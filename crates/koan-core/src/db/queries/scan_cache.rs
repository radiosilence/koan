use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension as _, params};

use crate::db::connection::DbError;

/// Update the scan cache entry for a file. A file that changed since its
/// entry was written may carry other art, so its track and album are named
/// in `art_evictions` for the covers cached by id; a file read again
/// unchanged (a forced re-read) names nothing.
pub fn update_scan_cache(
    conn: &Connection,
    path: &str,
    mtime: i64,
    size: i64,
    track_id: i64,
) -> Result<(), DbError> {
    let changed = conn
        .query_row(
            "SELECT mtime, size FROM scan_cache WHERE path = ?1",
            [path],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?
        .is_some_and(|was| was != (mtime, size));
    if changed {
        conn.execute(
            "INSERT INTO art_evictions (kind, id)
             SELECT 'track', ?1
             UNION ALL
             SELECT 'album', album_id FROM tracks WHERE id = ?1 AND album_id IS NOT NULL",
            [track_id],
        )?;
    }
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

/// What `art_evictions` names since a caller's mark.
#[derive(Debug, Default, PartialEq)]
pub struct ArtEvictionsSince {
    /// The mark to pass back next time.
    pub seq: i64,
    /// `(kind, id)` in the order named: `album`, `artist` or `track`.
    pub named: Vec<(String, i64)>,
    /// The mark is not this table's: entries after it were pruned, or the
    /// database is newer than it. Whatever the caller keeps by id may belong
    /// to other records, so all of it goes.
    pub purge: bool,
}

/// What `art_evictions` names after `after`, a mark this returned before (0
/// for none). The table keeps its latest entries only, and a database made
/// afresh numbers them from the start again: a mark either has outlived is
/// answered with `purge`, since the ids it covered may have been given to
/// other records meanwhile.
pub fn art_evictions_since(conn: &Connection, after: i64) -> rusqlite::Result<ArtEvictionsSince> {
    // The highest ever handed out, which pruning every entry does not lower.
    let high: i64 = conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'art_evictions'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let low: Option<i64> =
        conn.query_row("SELECT MIN(seq) FROM art_evictions", [], |r| r.get(0))?;
    let lost = match low {
        Some(low) => after < low - 1,
        None => after < high,
    };
    let purge = after > 0 && (after > high || lost);
    let named: Vec<(i64, String, i64)> = conn
        .prepare_cached("SELECT seq, kind, id FROM art_evictions WHERE seq > ?1 ORDER BY seq")?
        .query_map([if purge { high } else { after }], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ArtEvictionsSince {
        seq: if purge {
            high
        } else {
            named.last().map_or(after, |r| r.0)
        },
        named: named.into_iter().map(|(_, kind, id)| (kind, id)).collect(),
        purge,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_the_table_has_outlived_purges() {
        let db = test_db();
        let name = |n: i64| {
            for id in 0..n {
                db.conn
                    .execute(
                        "INSERT INTO art_evictions (kind, id) VALUES ('album', ?1)",
                        [id],
                    )
                    .unwrap();
            }
        };
        name(5);
        let all = art_evictions_since(&db.conn, 0).unwrap();
        assert_eq!((all.seq, all.named.len(), all.purge), (5, 5, false));
        let some = art_evictions_since(&db.conn, 3).unwrap();
        assert_eq!((some.seq, some.named.len(), some.purge), (5, 2, false));
        // Pruned past a mark: what it would have heard is gone.
        db.conn
            .execute("DELETE FROM art_evictions WHERE seq <= 4", [])
            .unwrap();
        assert!(art_evictions_since(&db.conn, 2).unwrap().purge);
        assert!(!art_evictions_since(&db.conn, 4).unwrap().purge);
        // Everything pruned: only a mark at the top heard it all.
        db.conn.execute("DELETE FROM art_evictions", []).unwrap();
        assert!(!art_evictions_since(&db.conn, 5).unwrap().purge);
        let behind = art_evictions_since(&db.conn, 3).unwrap();
        assert_eq!((behind.seq, behind.purge), (5, true));
        // A mark past anything this database handed out: another database's.
        let newer = art_evictions_since(&db.conn, 90).unwrap();
        assert_eq!((newer.seq, newer.named.len(), newer.purge), (5, 0, true));
    }
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

    /// A file stored again after it changed names its track and album for
    /// the cover caches; the first entry, and one unchanged, name nothing.
    #[test]
    fn a_changed_file_evicts_its_art() {
        let db = test_db();
        let id = upsert_track(&db.conn, &sample_meta("T", "A", "Al")).unwrap();
        let album: i64 = db
            .conn
            .query_row("SELECT album_id FROM tracks WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        let named = || -> Vec<(String, i64)> {
            let mut stmt = db
                .conn
                .prepare("SELECT kind, id FROM art_evictions ORDER BY seq")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        update_scan_cache(&db.conn, "/m/Al/T.flac", 1, 10, id).unwrap();
        update_scan_cache(&db.conn, "/m/Al/T.flac", 1, 10, id).unwrap();
        assert!(named().is_empty(), "{:?}", named());
        update_scan_cache(&db.conn, "/m/Al/T.flac", 2, 10, id).unwrap();
        assert_eq!(named(), [("track".into(), id), ("album".into(), album)]);
    }
}
