use rusqlite::{Connection, params};

use crate::db::connection::DbError;

/// A track's cached lyrics, as they were stored.
#[derive(Debug, Clone)]
pub struct CachedLyrics {
    pub content: String,
    pub synced: bool,
    /// Where the words came from, as `cache_lyrics` was told.
    pub source: String,
    /// When the copy was fetched, in seconds since the epoch.
    pub fetched_at: i64,
}

/// Retrieve cached lyrics for a track.
pub fn get_cached_lyrics(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<CachedLyrics>, DbError> {
    let result = conn.query_row(
        "SELECT content, synced, source, fetched_at FROM lyrics_cache WHERE track_id = ?1",
        params![track_id],
        |row| {
            Ok(CachedLyrics {
                content: row.get(0)?,
                synced: row.get::<_, i32>(1)? != 0,
                source: row.get(2)?,
                fetched_at: row.get(3)?,
            })
        },
    );

    match result {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Restart a cached copy's clock, leaving the copy as it is.
pub fn touch_lyrics(conn: &Connection, track_id: i64) -> Result<(), DbError> {
    conn.execute(
        "UPDATE lyrics_cache SET fetched_at = ?2 WHERE track_id = ?1",
        params![track_id, crate::shelves::now()],
    )?;
    Ok(())
}

/// Cache lyrics for a track. Replaces any existing cached lyrics.
pub fn cache_lyrics(
    conn: &Connection,
    track_id: i64,
    source: &str,
    synced: bool,
    content: &str,
) -> Result<(), DbError> {
    let now = crate::shelves::now();
    conn.execute(
        "INSERT INTO lyrics_cache (track_id, source, synced, content, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(track_id) DO UPDATE SET
             source = excluded.source,
             synced = excluded.synced,
             content = excluded.content,
             fetched_at = excluded.fetched_at",
        params![track_id, source, synced as i32, content, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connection::Database;
    use crate::db::queries::sample_meta;
    use crate::db::queries::tracks::upsert_track;

    fn test_db() -> Database {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        Database { conn }
    }

    #[test]
    fn test_cache_and_retrieve_lyrics() {
        let db = test_db();
        let meta = sample_meta("Windowlicker", "Aphex Twin", "Windowlicker EP");
        let track_id = upsert_track(&db.conn, &meta).unwrap();

        // No cached lyrics initially.
        assert!(get_cached_lyrics(&db.conn, track_id).unwrap().is_none());

        // Cache plain lyrics.
        cache_lyrics(
            &db.conn,
            track_id,
            "lrclib",
            false,
            "Hello world\nSecond line",
        )
        .unwrap();

        let cached = get_cached_lyrics(&db.conn, track_id).unwrap().unwrap();
        assert_eq!(cached.content, "Hello world\nSecond line");
        assert!(!cached.synced);
        assert_eq!(cached.source, "lrclib");

        // Overwrite with synced lyrics.
        cache_lyrics(
            &db.conn,
            track_id,
            "lrclib",
            true,
            "[00:12.00]Hello world\n[00:17.20]Second line",
        )
        .unwrap();

        let cached = get_cached_lyrics(&db.conn, track_id).unwrap().unwrap();
        assert!(cached.content.contains("[00:12.00]"));
        assert!(cached.synced);
    }
}
