use rusqlite::{Connection, params};

use crate::db::connection::DbError;

use super::ArtistRow;

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

/// Find artists by name (case-insensitive substring match).
pub fn find_artists(conn: &Connection, query: &str) -> Result<Vec<ArtistRow>, DbError> {
    let pattern = format!("%{}%", query);
    let mut stmt = conn.prepare(
        "SELECT DISTINCT a.id, a.name, a.sort_name, a.remote_id
         FROM artists a
         INNER JOIN albums al ON al.artist_id = a.id
         WHERE a.name LIKE ?1 COLLATE NOCASE
         ORDER BY COALESCE(a.sort_name, a.name)",
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

/// All artists, sorted by name.
/// Return artists that own at least one album (album artists only).
/// Track-only artists (e.g. featured artists) are excluded from the
/// top-level library view — they appear inline in the queue display.
pub fn all_artists(conn: &Connection) -> Result<Vec<ArtistRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT a.id, a.name, a.sort_name, a.remote_id
         FROM artists a
         INNER JOIN albums al ON al.artist_id = a.id
         ORDER BY COALESCE(a.sort_name, a.name)",
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

    #[test]
    fn test_artist_create_and_dedup() {
        let db = test_db();
        let id1 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        let id2 = get_or_create_artist(&db.conn, "Aphex Twin", None).unwrap();
        assert_eq!(id1, id2);

        let id3 = get_or_create_artist(&db.conn, "Squarepusher", None).unwrap();
        assert_ne!(id1, id3);
    }
}
