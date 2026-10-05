use rusqlite::{Connection, params};

use super::auth::resolve_user;

// A bookmark is a place to resume a track from: one per account and track,
// with an optional note. Subsonic clients make them for audiobooks, podcasts
// and long mixes. It names a row, and follows it through merges.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub track_id: i64,
    pub position_ms: i64,
    pub comment: Option<String>,
    /// Seconds since the epoch.
    pub created_at: i64,
    pub changed_at: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Save where `user` is in a track, replacing the bookmark already there.
pub fn save_bookmark(
    conn: &Connection,
    user: i64,
    track_id: i64,
    position_ms: i64,
    comment: Option<&str>,
) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO bookmarks (user_id, track_id, position_ms, comment, created_at, changed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT (user_id, track_id) DO UPDATE SET
             position_ms = excluded.position_ms,
             comment = excluded.comment,
             changed_at = excluded.changed_at",
    )?
    .execute(params![
        resolve_user(conn, user)?,
        track_id,
        position_ms.max(0),
        comment,
        now()
    ])?;
    Ok(())
}

/// Remove `user`'s bookmark in a track. Returns whether there was one.
pub fn delete_bookmark(conn: &Connection, user: i64, track_id: i64) -> rusqlite::Result<bool> {
    let removed = conn
        .prepare_cached("DELETE FROM bookmarks WHERE user_id = ?1 AND track_id = ?2")?
        .execute(params![resolve_user(conn, user)?, track_id])?;
    Ok(removed > 0)
}

/// `user`'s bookmarks, most recently changed first.
pub fn bookmarks(conn: &Connection, user: i64) -> rusqlite::Result<Vec<Bookmark>> {
    let mut stmt = conn.prepare_cached(
        "SELECT track_id, position_ms, comment, created_at, changed_at FROM bookmarks
         WHERE user_id = ?1 ORDER BY changed_at DESC, track_id",
    )?;
    let rows = stmt.query_map([resolve_user(conn, user)?], |r| {
        Ok(Bookmark {
            track_id: r.get(0)?,
            position_ms: r.get(1)?,
            comment: r.get(2)?,
            created_at: r.get(3)?,
            changed_at: r.get(4)?,
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::LOCAL_USER;

    /// A library of two tracks, ids 1 and 2.
    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn.execute_batch("INSERT INTO tracks (id, title) VALUES (1, 'Side A'), (2, 'Side B');")
            .unwrap();
        conn
    }

    #[test]
    fn save_replaces_and_delete_removes() {
        let conn = &conn();
        save_bookmark(conn, LOCAL_USER, 1, 60_000, Some("chapter 2")).unwrap();
        save_bookmark(conn, LOCAL_USER, 1, 90_000, None).unwrap();
        let saved = bookmarks(conn, LOCAL_USER).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].position_ms, 90_000);
        assert_eq!(saved[0].comment, None);
        assert!(delete_bookmark(conn, LOCAL_USER, 1).unwrap());
        assert!(!delete_bookmark(conn, LOCAL_USER, 1).unwrap());
        assert!(bookmarks(conn, LOCAL_USER).unwrap().is_empty());
    }

    #[test]
    fn a_bookmark_needs_its_track() {
        let conn = &conn();
        assert!(save_bookmark(conn, LOCAL_USER, 99, 0, None).is_err());
        save_bookmark(conn, LOCAL_USER, 2, 1_000, None).unwrap();
        conn.execute("DELETE FROM tracks WHERE id = 2", []).unwrap();
        assert!(bookmarks(conn, LOCAL_USER).unwrap().is_empty());
    }
}
