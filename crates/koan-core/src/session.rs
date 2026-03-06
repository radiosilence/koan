use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::player::state::{LoadState, PlaylistItem, QueueItemId, SharedPlayerState};

/// Serializable snapshot of a playlist item (no Arc/Atomic fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionItem {
    id: String,
    path: PathBuf,
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    year: Option<String>,
    codec: Option<String>,
    track_number: Option<i64>,
    disc: Option<i64>,
    duration_ms: Option<u64>,
}

/// Persisted session state — written on exit, restored on next launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    items: Vec<SessionItem>,
    cursor_id: Option<String>,
    position_ms: u64,
    playback_paused: bool,
}

fn session_path() -> PathBuf {
    config::config_dir().join("session.json")
}

impl SessionState {
    /// Snapshot the current player state for persistence.
    pub fn capture(state: &SharedPlayerState) -> Self {
        let (playlist, cursor) = state.snapshot_playlist();
        let position_ms = state.position_ms();
        let playback_paused = matches!(
            state.playback_state(),
            crate::player::state::PlaybackState::Playing
                | crate::player::state::PlaybackState::Paused
        );

        let items: Vec<SessionItem> = playlist
            .into_iter()
            .map(|item| SessionItem {
                id: item.id.0.to_string(),
                path: item.path,
                title: item.title,
                artist: item.artist,
                album_artist: item.album_artist,
                album: item.album,
                year: item.year,
                codec: item.codec,
                track_number: item.track_number,
                disc: item.disc,
                duration_ms: item.duration_ms,
            })
            .collect();

        let cursor_id = cursor.map(|c| c.0.to_string());

        Self {
            items,
            cursor_id,
            position_ms,
            playback_paused,
        }
    }

    /// Restore playlist items and cursor from persisted state.
    /// Filters out items whose files no longer exist on disk.
    /// Returns (playlist_items, cursor_id, position_ms, was_playing).
    pub fn into_playlist(self) -> (Vec<PlaylistItem>, Option<QueueItemId>, u64, bool) {
        let mut cursor_id: Option<QueueItemId> = None;

        let items: Vec<PlaylistItem> = self
            .items
            .into_iter()
            .filter_map(|item| {
                // Skip items whose files have disappeared.
                if !item.path.exists() {
                    return None;
                }

                let uuid = uuid::Uuid::parse_str(&item.id).ok()?;
                let id = QueueItemId(uuid);

                // Track if this was the cursor.
                if self.cursor_id.as_deref() == Some(&item.id) {
                    cursor_id = Some(id);
                }

                Some(PlaylistItem {
                    id,
                    path: item.path,
                    title: item.title,
                    artist: item.artist,
                    album_artist: item.album_artist,
                    album: item.album,
                    year: item.year,
                    codec: item.codec,
                    track_number: item.track_number,
                    disc: item.disc,
                    duration_ms: item.duration_ms,
                    load_state: LoadState::Ready,
                })
            })
            .collect();

        (items, cursor_id, self.position_ms, self.playback_paused)
    }

    /// Save session state to disk.
    pub fn save(&self) -> std::io::Result<()> {
        let path = session_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(self)
            .map_err(std::io::Error::other)?;
        fs::write(&path, json)
    }

    /// Load session state from disk. Returns None if no session file exists.
    pub fn load() -> Option<Self> {
        let path = session_path();
        let contents = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&contents).ok()
    }
}

/// Delete the session file.
pub fn clear() {
    let path = session_path();
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_roundtrip() {
        let state = SessionState {
            items: vec![SessionItem {
                id: uuid::Uuid::now_v7().to_string(),
                path: PathBuf::from("/tmp/test.flac"),
                title: "Test".into(),
                artist: "Artist".into(),
                album_artist: "Artist".into(),
                album: "Album".into(),
                year: Some("2024".into()),
                codec: Some("FLAC".into()),
                track_number: Some(1),
                disc: Some(1),
                duration_ms: Some(200_000),
            }],
            cursor_id: None,
            position_ms: 42_000,
            playback_paused: false,
        };

        let json = serde_json::to_string(&state).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.items.len(), 1);
        assert_eq!(restored.position_ms, 42_000);
        assert_eq!(restored.items[0].title, "Test");
    }

    #[test]
    fn test_into_playlist_filters_missing_files() {
        let state = SessionState {
            items: vec![SessionItem {
                id: uuid::Uuid::now_v7().to_string(),
                path: PathBuf::from("/nonexistent/file.flac"),
                title: "Gone".into(),
                artist: "Artist".into(),
                album_artist: "Artist".into(),
                album: "Album".into(),
                year: None,
                codec: None,
                track_number: None,
                disc: None,
                duration_ms: None,
            }],
            cursor_id: None,
            position_ms: 0,
            playback_paused: false,
        };

        let (items, cursor, _, _) = state.into_playlist();
        assert!(items.is_empty());
        assert!(cursor.is_none());
    }
}
