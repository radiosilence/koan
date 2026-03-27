use rusqlite::Connection;

use super::playback_state::PersistedQueueItem;

/// A named queue snapshot.
#[derive(Debug, Clone)]
pub struct QueueSnapshot {
    pub id: i64,
    pub name: String,
    pub items: Vec<PersistedQueueItem>,
    pub cursor_path: Option<String>,
    pub position_ms: u64,
    pub created_at: String,
}

/// Summary for listing (without deserializing queue_json).
#[derive(Debug, Clone)]
pub struct QueueSnapshotSummary {
    pub id: i64,
    pub name: String,
    pub track_count: usize,
    pub cursor_path: Option<String>,
    pub position_ms: u64,
    pub created_at: String,
}

/// Save the current queue as a named snapshot. Overwrites if name exists.
pub fn save_snapshot(
    conn: &Connection,
    name: &str,
    items: &[PersistedQueueItem],
    cursor_path: Option<&str>,
    position_ms: u64,
) -> rusqlite::Result<i64> {
    let json = serde_json::to_string(items).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO queue_snapshots (name, queue_json, cursor_path, position_ms, created_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))
         ON CONFLICT(name) DO UPDATE SET
           queue_json = ?2, cursor_path = ?3, position_ms = ?4, created_at = datetime('now')",
        rusqlite::params![name, json, cursor_path, position_ms as i64],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Load a named snapshot. Returns None if not found.
pub fn load_snapshot(conn: &Connection, name: &str) -> rusqlite::Result<Option<QueueSnapshot>> {
    let result = conn.query_row(
        "SELECT id, name, queue_json, cursor_path, position_ms, created_at
         FROM queue_snapshots WHERE name = ?1",
        [name],
        |row| {
            let id: i64 = row.get(0)?;
            let name: String = row.get(1)?;
            let json: String = row.get(2)?;
            let cursor_path: Option<String> = row.get(3)?;
            let position_ms: i64 = row.get(4)?;
            let created_at: String = row.get(5)?;
            Ok((id, name, json, cursor_path, position_ms as u64, created_at))
        },
    );

    match result {
        Ok((id, name, json, cursor_path, position_ms, created_at)) => {
            let items: Vec<PersistedQueueItem> = serde_json::from_str(&json).unwrap_or_default();
            Ok(Some(QueueSnapshot {
                id,
                name,
                items,
                cursor_path,
                position_ms,
                created_at,
            }))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// List all snapshots (summary only — doesn't deserialize queue_json).
pub fn list_snapshots(conn: &Connection) -> rusqlite::Result<Vec<QueueSnapshotSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, queue_json, cursor_path, position_ms, created_at
         FROM queue_snapshots ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        let id: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        let json: String = row.get(2)?;
        let cursor_path: Option<String> = row.get(3)?;
        let position_ms: i64 = row.get(4)?;
        let created_at: String = row.get(5)?;
        let track_count: usize = serde_json::from_str::<Vec<serde_json::Value>>(&json)
            .map(|v| v.len())
            .unwrap_or(0);
        Ok(QueueSnapshotSummary {
            id,
            name,
            track_count,
            cursor_path,
            position_ms: position_ms as u64,
            created_at,
        })
    })?;
    rows.collect()
}

/// Delete a named snapshot. Returns true if a row was deleted.
pub fn delete_snapshot(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    let deleted = conn.execute("DELETE FROM queue_snapshots WHERE name = ?1", [name])?;
    Ok(deleted > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "on").unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    fn sample_items() -> Vec<PersistedQueueItem> {
        vec![
            PersistedQueueItem {
                path: "/music/track1.flac".into(),
                title: "Track 1".into(),
                artist: "Artist".into(),
                album_artist: "Artist".into(),
                album: "Album".into(),
                year: Some("2024".into()),
                codec: Some("FLAC".into()),
                track_number: Some(1),
                disc: Some(1),
                duration_ms: Some(240_000),
                db_id: None,
            },
            PersistedQueueItem {
                path: "/music/track2.flac".into(),
                title: "Track 2".into(),
                artist: "Artist".into(),
                album_artist: "Artist".into(),
                album: "Album".into(),
                year: Some("2024".into()),
                codec: Some("FLAC".into()),
                track_number: Some(2),
                disc: Some(1),
                duration_ms: Some(180_000),
                db_id: None,
            },
        ]
    }

    #[test]
    fn save_and_load_snapshot() {
        let conn = test_conn();
        let items = sample_items();

        save_snapshot(&conn, "techno", &items, Some("/music/track1.flac"), 42_000).unwrap();

        let snap = load_snapshot(&conn, "techno").unwrap().unwrap();
        assert_eq!(snap.name, "techno");
        assert_eq!(snap.items.len(), 2);
        assert_eq!(snap.items[0].title, "Track 1");
        assert_eq!(snap.cursor_path.as_deref(), Some("/music/track1.flac"));
        assert_eq!(snap.position_ms, 42_000);
    }

    #[test]
    fn save_overwrites_existing() {
        let conn = test_conn();
        let items = sample_items();

        save_snapshot(&conn, "mix", &items, None, 0).unwrap();
        save_snapshot(&conn, "mix", &items[..1], Some("/music/track1.flac"), 999).unwrap();

        let snap = load_snapshot(&conn, "mix").unwrap().unwrap();
        assert_eq!(snap.items.len(), 1);
        assert_eq!(snap.position_ms, 999);
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let conn = test_conn();
        assert!(load_snapshot(&conn, "nope").unwrap().is_none());
    }

    #[test]
    fn list_snapshots_returns_all() {
        let conn = test_conn();
        let items = sample_items();

        save_snapshot(&conn, "first", &items, None, 0).unwrap();
        save_snapshot(&conn, "second", &items[..1], None, 100).unwrap();

        let list = list_snapshots(&conn).unwrap();
        assert_eq!(list.len(), 2);
        let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"first"));
        assert!(names.contains(&"second"));
        let second = list.iter().find(|s| s.name == "second").unwrap();
        assert_eq!(second.track_count, 1);
    }

    #[test]
    fn delete_snapshot_works() {
        let conn = test_conn();
        save_snapshot(&conn, "doomed", &sample_items(), None, 0).unwrap();

        assert!(delete_snapshot(&conn, "doomed").unwrap());
        assert!(!delete_snapshot(&conn, "doomed").unwrap()); // already gone
        assert!(load_snapshot(&conn, "doomed").unwrap().is_none());
    }
}
