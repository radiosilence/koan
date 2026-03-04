use rusqlite::{Connection, params};

use crate::db::connection::DbError;

/// Retrieve cached lyrics for a track. Returns (content, is_synced) if found.
pub fn get_cached_lyrics(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<(String, bool)>, DbError> {
    let result = conn.query_row(
        "SELECT content, synced FROM lyrics_cache WHERE track_id = ?1",
        params![track_id],
        |row| {
            let content: String = row.get(0)?;
            let synced: bool = row.get::<_, i32>(1)? != 0;
            Ok((content, synced))
        },
    );

    match result {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Cache lyrics for a track. Replaces any existing cached lyrics.
pub fn cache_lyrics(
    conn: &Connection,
    track_id: i64,
    source: &str,
    synced: bool,
    content: &str,
) -> Result<(), DbError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

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

        let (content, synced) = get_cached_lyrics(&db.conn, track_id).unwrap().unwrap();
        assert_eq!(content, "Hello world\nSecond line");
        assert!(!synced);

        // Overwrite with synced lyrics.
        cache_lyrics(
            &db.conn,
            track_id,
            "lrclib",
            true,
            "[00:12.00]Hello world\n[00:17.20]Second line",
        )
        .unwrap();

        let (content, synced) = get_cached_lyrics(&db.conn, track_id).unwrap().unwrap();
        assert!(content.contains("[00:12.00]"));
        assert!(synced);
    }
}
