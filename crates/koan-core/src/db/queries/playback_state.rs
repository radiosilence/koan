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
    /// Database track ID — enables re-downloading on session restore.
    /// Absent in pre-v0.18.2 persisted state; serde default covers migration.
    #[serde(default)]
    pub db_id: Option<i64>,
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
            db_id: item.db_id,
        }
    }

    /// Convert back to a PlaylistItem with fresh ID.
    /// Verifies the file path still exists on disk — missing cache files
    /// get `LoadState::Pending` so the player re-triggers their download.
    pub fn to_playlist_item(&self) -> crate::player::state::PlaylistItem {
        let path = PathBuf::from(&self.path);
        let load_state = if path.exists() {
            crate::player::state::LoadState::Ready
        } else {
            log::debug!(
                "session restore: path missing, marking pending: {}",
                self.path,
            );
            crate::player::state::LoadState::Pending
        };
        crate::player::state::PlaylistItem {
            id: crate::player::state::QueueItemId::new(),
            db_id: self.db_id,
            path,
            title: self.title.clone(),
            artist: self.artist.clone(),
            album_artist: self.album_artist.clone(),
            album: self.album.clone(),
            year: self.year.clone(),
            codec: self.codec.clone(),
            track_number: self.track_number,
            disc: self.disc,
            duration_ms: self.duration_ms,
            load_state,
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
            db_id: None,
        }];
        save_playback_state(&conn, &items, None, 0).unwrap();
        assert!(load_playback_state(&conn).unwrap().is_some());

        clear_playback_state(&conn).unwrap();
        assert!(load_playback_state(&conn).unwrap().is_none());
    }

    #[test]
    fn test_to_playlist_item_marks_missing_path_as_pending() {
        let item = PersistedQueueItem {
            path: "/nonexistent/path/track.flac".into(),
            title: "Ghost".into(),
            artist: "Nobody".into(),
            album_artist: "Nobody".into(),
            album: "Void".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            db_id: Some(42),
        };
        let playlist_item = item.to_playlist_item();
        assert!(
            matches!(
                playlist_item.load_state,
                crate::player::state::LoadState::Pending
            ),
            "missing file should be Pending, got {:?}",
            playlist_item.load_state,
        );
        assert_eq!(playlist_item.db_id, Some(42), "db_id should be preserved");
    }

    #[test]
    fn test_to_playlist_item_marks_existing_path_as_ready() {
        // Use cargo's own manifest as a file that definitely exists.
        let manifest = env!("CARGO_MANIFEST_DIR");
        let existing_path = format!("{}/Cargo.toml", manifest);
        let item = PersistedQueueItem {
            path: existing_path,
            title: "Real".into(),
            artist: "Someone".into(),
            album_artist: "Someone".into(),
            album: "Exists".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            db_id: None,
        };
        let playlist_item = item.to_playlist_item();
        assert!(
            matches!(
                playlist_item.load_state,
                crate::player::state::LoadState::Ready
            ),
            "existing file should be Ready, got {:?}",
            playlist_item.load_state,
        );
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
            db_id: None,
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
            db_id: None,
        }];
        save_playback_state(&conn, &items2, Some("/music/new.flac"), 999).unwrap();

        let loaded = load_playback_state(&conn).unwrap().unwrap();
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].title, "New");
        assert_eq!(loaded.position_ms, 999);
    }

    #[test]
    fn test_db_id_round_trip() {
        let conn = test_conn();
        let items = vec![
            PersistedQueueItem {
                path: "/music/local.flac".into(),
                title: "Local".into(),
                artist: "A".into(),
                album_artist: "A".into(),
                album: "B".into(),
                year: None,
                codec: None,
                track_number: None,
                disc: None,
                duration_ms: None,
                db_id: None,
            },
            PersistedQueueItem {
                path: "/cache/remote.flac".into(),
                title: "Remote".into(),
                artist: "C".into(),
                album_artist: "C".into(),
                album: "D".into(),
                year: None,
                codec: None,
                track_number: None,
                disc: None,
                duration_ms: None,
                db_id: Some(99),
            },
        ];
        save_playback_state(&conn, &items, None, 0).unwrap();

        let loaded = load_playback_state(&conn).unwrap().unwrap();
        assert_eq!(
            loaded.items[0].db_id, None,
            "local track should have no db_id"
        );
        assert_eq!(
            loaded.items[1].db_id,
            Some(99),
            "remote track db_id should persist"
        );
    }

    #[test]
    fn test_db_id_migration_from_old_format() {
        // Simulate old-format JSON that lacks the db_id field.
        let conn = test_conn();
        let old_json = r#"[{"path":"/music/track.flac","title":"T","artist":"A","album_artist":"A","album":"B","year":null,"codec":null,"track_number":null,"disc":null,"duration_ms":null}]"#;
        conn.execute(
            "INSERT INTO playback_state (id, queue_json, cursor_id, position_ms, updated_at)
             VALUES (1, ?1, NULL, 0, datetime('now'))",
            rusqlite::params![old_json],
        )
        .unwrap();

        let loaded = load_playback_state(&conn).unwrap().unwrap();
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(
            loaded.items[0].db_id, None,
            "missing field should default to None"
        );
    }

    #[test]
    fn save_and_restore_playback_state_roundtrip() {
        // Full round-trip: build playlist items, save state with cursor and position,
        // clear in-memory, restore from DB, verify everything matches.
        let conn = test_conn();

        let items = vec![
            PersistedQueueItem {
                path: "/music/alpha.flac".into(),
                title: "Alpha".into(),
                artist: "Band".into(),
                album_artist: "Band".into(),
                album: "Debut".into(),
                year: Some("2023".into()),
                codec: Some("FLAC".into()),
                track_number: Some(1),
                disc: Some(1),
                duration_ms: Some(300_000),
                db_id: Some(10),
            },
            PersistedQueueItem {
                path: "/music/beta.flac".into(),
                title: "Beta".into(),
                artist: "Band".into(),
                album_artist: "Band".into(),
                album: "Debut".into(),
                year: Some("2023".into()),
                codec: Some("FLAC".into()),
                track_number: Some(2),
                disc: Some(1),
                duration_ms: Some(250_000),
                db_id: Some(11),
            },
            PersistedQueueItem {
                path: "/music/gamma.flac".into(),
                title: "Gamma".into(),
                artist: "Other".into(),
                album_artist: "Other".into(),
                album: "Solo".into(),
                year: None,
                codec: Some("MP3".into()),
                track_number: Some(1),
                disc: None,
                duration_ms: Some(180_000),
                db_id: None,
            },
        ];

        // Save with cursor on the second track, position 42s in.
        let cursor_path = "/music/beta.flac";
        let position_ms = 42_000u64;
        save_playback_state(&conn, &items, Some(cursor_path), position_ms).unwrap();

        // Simulate "clear in-memory state" by just loading fresh from DB.
        let restored = load_playback_state(&conn)
            .unwrap()
            .expect("should have persisted state");

        // Verify queue integrity.
        assert_eq!(restored.items.len(), 3, "queue length mismatch");

        // Verify item fields survived the round-trip.
        assert_eq!(restored.items[0].title, "Alpha");
        assert_eq!(restored.items[0].path, "/music/alpha.flac");
        assert_eq!(restored.items[0].db_id, Some(10));
        assert_eq!(restored.items[0].track_number, Some(1));

        assert_eq!(restored.items[1].title, "Beta");
        assert_eq!(restored.items[1].artist, "Band");
        assert_eq!(restored.items[1].duration_ms, Some(250_000));
        assert_eq!(restored.items[1].db_id, Some(11));

        assert_eq!(restored.items[2].title, "Gamma");
        assert_eq!(restored.items[2].artist, "Other");
        assert_eq!(restored.items[2].codec.as_deref(), Some("MP3"));
        assert_eq!(restored.items[2].db_id, None);

        // Verify cursor and position.
        assert_eq!(
            restored.cursor_path.as_deref(),
            Some(cursor_path),
            "cursor should point to the second track"
        );
        assert_eq!(
            restored.position_ms, position_ms,
            "position should survive round-trip"
        );

        // Verify to_playlist_item conversion (paths don't exist on disk, so all get Pending).
        let playlist_items: Vec<_> = restored
            .items
            .iter()
            .map(|i| i.to_playlist_item())
            .collect();
        assert_eq!(playlist_items.len(), 3);
        for pi in &playlist_items {
            assert!(
                matches!(pi.load_state, crate::player::state::LoadState::Pending),
                "non-existent paths should be Pending"
            );
        }
        // Each converted item should have a unique QueueItemId.
        let ids: std::collections::HashSet<_> = playlist_items.iter().map(|i| i.id.0).collect();
        assert_eq!(ids.len(), 3, "each item should get a unique QueueItemId");
    }
}
