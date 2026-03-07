use std::path::PathBuf;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Serializable representation of a queue item for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedQueueItem {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub year: Option<String>,
    pub codec: Option<String>,
    pub track_number: Option<i64>,
    pub disc: Option<i64>,
    pub duration_ms: Option<u64>,
}

/// Full persisted playback state.
#[derive(Debug, Clone)]
pub struct PersistedPlaybackState {
    pub items: Vec<PersistedQueueItem>,
    pub cursor_path: Option<String>,
    pub position_ms: u64,
}

/// Save the current playback state to the database.
/// Uses UPSERT on the single-row playback_state table (id=1).
pub fn save_playback_state(
    conn: &Connection,
    items: &[PersistedQueueItem],
    cursor_path: Option<&str>,
    position_ms: u64,
) -> rusqlite::Result<()> {
    let json = serde_json::to_string(items).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO playback_state (id, queue_json, cursor_id, position_ms, updated_at)
         VALUES (1, ?1, ?2, ?3, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET
           queue_json = ?1, cursor_id = ?2, position_ms = ?3, updated_at = datetime('now')",
        rusqlite::params![json, cursor_path, position_ms as i64],
    )?;
    Ok(())
}

/// Load persisted playback state. Returns None if no state has been saved.
pub fn load_playback_state(conn: &Connection) -> rusqlite::Result<Option<PersistedPlaybackState>> {
    let result = conn.query_row(
        "SELECT queue_json, cursor_id, position_ms FROM playback_state WHERE id = 1",
        [],
        |row| {
            let json: String = row.get(0)?;
            let cursor_path: Option<String> = row.get(1)?;
            let position_ms: i64 = row.get(2)?;
            Ok((json, cursor_path, position_ms as u64))
        },
    );

    match result {
        Ok((json, cursor_path, position_ms)) => {
            let items: Vec<PersistedQueueItem> = serde_json::from_str(&json).unwrap_or_default();
            if items.is_empty() {
                return Ok(None);
            }
            Ok(Some(PersistedPlaybackState {
                items,
                cursor_path,
                position_ms,
            }))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Clear persisted playback state.
pub fn clear_playback_state(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM playback_state", [])?;
    Ok(())
}

impl PersistedQueueItem {
    /// Create from a playlist item's fields.
    pub fn from_playlist_item(item: &crate::player::state::PlaylistItem) -> Self {
        Self {
            path: item.path.to_string_lossy().into_owned(),
            title: item.title.clone(),
            artist: item.artist.clone(),
            album_artist: item.album_artist.clone(),
            album: item.album.clone(),
            year: item.year.clone(),
            codec: item.codec.clone(),
            track_number: item.track_number,
            disc: item.disc,
            duration_ms: item.duration_ms,
        }
    }

    /// Convert back to a PlaylistItem with fresh ID and Pending load state.
    pub fn to_playlist_item(&self) -> crate::player::state::PlaylistItem {
        crate::player::state::PlaylistItem {
            id: crate::player::state::QueueItemId::new(),
            path: PathBuf::from(&self.path),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album_artist: self.album_artist.clone(),
            album: self.album.clone(),
            year: self.year.clone(),
            codec: self.codec.clone(),
            track_number: self.track_number,
            disc: self.disc,
            duration_ms: self.duration_ms,
            load_state: crate::player::state::LoadState::Ready,
        }
    }
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

    #[test]
    fn test_save_and_load_playback_state() {
        let conn = test_conn();
        let items = vec![
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
            },
        ];

        save_playback_state(&conn, &items, Some("/music/track1.flac"), 42_000).unwrap();

        let loaded = load_playback_state(&conn).unwrap().unwrap();
        assert_eq!(loaded.items.len(), 2);
        assert_eq!(loaded.items[0].title, "Track 1");
        assert_eq!(loaded.items[1].path, "/music/track2.flac");
        assert_eq!(loaded.cursor_path.as_deref(), Some("/music/track1.flac"));
        assert_eq!(loaded.position_ms, 42_000);
    }

    #[test]
    fn test_load_returns_none_when_empty() {
        let conn = test_conn();
        let loaded = load_playback_state(&conn).unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_clear_playback_state() {
        let conn = test_conn();
        let items = vec![PersistedQueueItem {
            path: "/music/track.flac".into(),
            title: "Track".into(),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: "Album".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
        }];
        save_playback_state(&conn, &items, None, 0).unwrap();
        assert!(load_playback_state(&conn).unwrap().is_some());

        clear_playback_state(&conn).unwrap();
        assert!(load_playback_state(&conn).unwrap().is_none());
    }

    #[test]
    fn test_save_overwrites_previous_state() {
        let conn = test_conn();
        let items1 = vec![PersistedQueueItem {
            path: "/music/old.flac".into(),
            title: "Old".into(),
            artist: "A".into(),
            album_artist: "A".into(),
            album: "B".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
        }];
        save_playback_state(&conn, &items1, None, 100).unwrap();

        let items2 = vec![PersistedQueueItem {
            path: "/music/new.flac".into(),
            title: "New".into(),
            artist: "X".into(),
            album_artist: "X".into(),
            album: "Y".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
        }];
        save_playback_state(&conn, &items2, Some("/music/new.flac"), 999).unwrap();

        let loaded = load_playback_state(&conn).unwrap().unwrap();
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].title, "New");
        assert_eq!(loaded.position_ms, 999);
    }
}
