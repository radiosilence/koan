use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use uuid::Uuid;

/// Stable identity for a queue entry. UUIDv7 — time-ordered, unique across duplicates.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueueItemId(pub Uuid);

impl QueueItemId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for QueueItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for QueueItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Short form for logs: first 8 hex chars.
        write!(f, "QId({})", &self.0.to_string()[..8])
    }
}

/// Playback state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlaybackState {
    Stopped = 0,
    Playing = 1,
    Paused = 2,
}

impl PlaybackState {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Playing,
            2 => Self::Paused,
            _ => Self::Stopped,
        }
    }
}

/// Audio format info for the currently playing track.
#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub id: QueueItemId,
    pub path: PathBuf,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub duration_ms: u64,
}

// --- Playlist data model (single source of truth) ---

/// Load state of a playlist item — tracks download lifecycle.
#[derive(Debug, Clone)]
pub enum LoadState {
    Pending,
    Downloading { downloaded: u64, total: u64 },
    Ready,
    Failed(String),
}

/// A single item in the playlist. Replaces QueueEntry + QueueEntryMeta + pending entries
/// as the canonical data. Created once when tracks are added to the playlist.
#[derive(Debug, Clone)]
pub struct PlaylistItem {
    pub id: QueueItemId,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub year: Option<String>,
    pub codec: Option<String>,
    pub track_number: Option<i64>,
    pub disc: Option<i64>,
    pub duration_ms: Option<u64>,
    pub load_state: LoadState,
}

/// The playlist — one flat array, one cursor. Everything else derived.
#[derive(Debug, Clone, Default)]
pub struct Playlist {
    pub items: Vec<PlaylistItem>,
    pub cursor: Option<QueueItemId>,
}

// --- UI view types (kept for TUI compat) ---

/// Status of a track in the queue — for UI display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueEntryStatus {
    Queued,
    Playing,
    Played,
    Downloading,
    /// User double-clicked — this track is priority, will play when ready.
    PriorityPending,
    Failed,
}

/// A single entry in the UI-visible queue snapshot.
#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub id: QueueItemId,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub year: Option<String>,
    pub codec: Option<String>,
    pub track_number: Option<i64>,
    pub disc: Option<i64>,
    pub duration_ms: Option<u64>,
    pub status: QueueEntryStatus,
    pub download_progress: Option<(u64, u64)>,
}

/// Pre-built visible queue — single atomic snapshot for the UI.
#[derive(Debug, Clone, Default)]
pub struct VisibleQueueSnapshot {
    pub entries: Vec<QueueEntry>,
    pub finished_count: usize,
    pub has_playing: bool,
    pub queue_count: usize,
}

/// Shared player state — atomics for lock-free reads from UI thread.
///
/// The engine writes these, the UI reads them. No mutexes in the hot path.
#[derive(Debug)]
pub struct SharedPlayerState {
    state: AtomicU8,
    position_ms: AtomicU64,
    track_info: parking_lot::RwLock<Option<TrackInfo>>,

    /// THE playlist + cursor — one lock, one truth.
    playlist: parking_lot::RwLock<Playlist>,

    /// Bumped on every playlist mutation so UI can skip redundant redraws.
    playlist_version: AtomicU64,

    /// Playback generation — incremented each start_playback so stale decode
    /// thread callbacks can detect they're outdated and skip state mutations.
    playback_generation: AtomicU64,

    /// Set by external signals (e.g. souvlaki Quit event) to request clean shutdown.
    quit_requested: AtomicBool,
}

impl SharedPlayerState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(PlaybackState::Stopped as u8),
            position_ms: AtomicU64::new(0),
            track_info: parking_lot::RwLock::new(None),
            playlist: parking_lot::RwLock::new(Playlist::default()),
            playlist_version: AtomicU64::new(0),
            playback_generation: AtomicU64::new(0),
            quit_requested: AtomicBool::new(false),
        })
    }

    // --- Playback state ---

    pub fn playback_state(&self) -> PlaybackState {
        PlaybackState::from_u8(self.state.load(Ordering::Relaxed))
    }

    pub fn set_playback_state(&self, state: PlaybackState) {
        self.state.store(state as u8, Ordering::Relaxed);
    }

    pub fn position_ms(&self) -> u64 {
        self.position_ms.load(Ordering::Relaxed)
    }

    pub fn set_position_ms(&self, pos: u64) {
        self.position_ms.store(pos, Ordering::Relaxed);
    }

    pub fn track_info(&self) -> Option<TrackInfo> {
        self.track_info.read().clone()
    }

    pub fn set_track_info(&self, info: Option<TrackInfo>) {
        *self.track_info.write() = info;
    }

    // --- Playback generation ---

    pub fn bump_generation(&self) -> u64 {
        self.playback_generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn generation(&self) -> u64 {
        self.playback_generation.load(Ordering::Relaxed)
    }

    // --- Quit ---

    pub fn request_quit(&self) {
        self.quit_requested.store(true, Ordering::Relaxed);
    }

    pub fn quit_requested(&self) -> bool {
        self.quit_requested.load(Ordering::Relaxed)
    }

    // --- Playlist version ---

    pub fn playlist_version(&self) -> u64 {
        self.playlist_version.load(Ordering::Relaxed)
    }

    fn bump_version(&self) {
        self.playlist_version.fetch_add(1, Ordering::Relaxed);
    }

    // --- Playlist mutations (called from player thread via commands) ---

    /// Append items to the playlist.
    pub fn add_items(&self, items: Vec<PlaylistItem>) {
        let mut pl = self.playlist.write();
        pl.items.extend(items);
        drop(pl);
        self.bump_version();
    }

    /// Insert items after a specific queue item.
    pub fn insert_items_after(&self, items: Vec<PlaylistItem>, after: QueueItemId) {
        let mut pl = self.playlist.write();
        let insert_at = match pl.items.iter().position(|item| item.id == after) {
            Some(pos) => pos + 1,
            None => pl.items.len(), // fallback: append
        };
        for (i, item) in items.into_iter().enumerate() {
            pl.items.insert(insert_at + i, item);
        }
        drop(pl);
        self.bump_version();
    }

    /// Update file paths for playlist items (after organize moves files).
    pub fn update_paths(&self, updates: &[(QueueItemId, PathBuf)]) {
        let mut pl = self.playlist.write();
        for (id, new_path) in updates {
            if let Some(item) = pl.items.iter_mut().find(|item| item.id == *id) {
                item.path = new_path.clone();
            }
        }
        drop(pl);
        self.bump_version();
    }

    /// Remove an item by ID.
    pub fn remove_item(&self, id: QueueItemId) {
        let mut pl = self.playlist.write();
        pl.items.retain(|item| item.id != id);
        // If cursor was on removed item, clear it (caller handles next_track).
        if pl.cursor == Some(id) {
            pl.cursor = None;
        }
        drop(pl);
        self.bump_version();
    }

    /// Move an item relative to another entry.
    pub fn move_item(&self, id: QueueItemId, target: QueueItemId, after: bool) {
        let mut pl = self.playlist.write();
        let Some(from) = pl.items.iter().position(|item| item.id == id) else {
            return;
        };
        let item = pl.items.remove(from);
        let Some(to) = pl.items.iter().position(|item| item.id == target) else {
            // Target gone — put it back.
            let pos = from.min(pl.items.len());
            pl.items.insert(pos, item);
            return;
        };
        let insert_at = if after { to + 1 } else { to };
        pl.items.insert(insert_at, item);
        drop(pl);
        self.bump_version();
    }

    /// Batch move: extract items by ID, reinsert them at `target` position.
    /// Preserves the relative order of the moved items.
    pub fn move_items(&self, ids: &[QueueItemId], target: QueueItemId, after: bool) {
        use std::collections::HashSet;
        let id_set: HashSet<QueueItemId> = ids.iter().copied().collect();

        let mut pl = self.playlist.write();

        // Partition: extract moved items, keep the rest.
        let mut remaining = Vec::with_capacity(pl.items.len());
        let mut moved = Vec::with_capacity(ids.len());
        for item in pl.items.drain(..) {
            if id_set.contains(&item.id) {
                moved.push(item);
            } else {
                remaining.push(item);
            }
        }

        // Find target in the remaining items.
        let insert_at = match remaining.iter().position(|item| item.id == target) {
            Some(pos) => {
                if after {
                    pos + 1
                } else {
                    pos
                }
            }
            None => remaining.len(),
        };

        // Splice moved items in at the target position.
        for (i, item) in moved.into_iter().enumerate() {
            remaining.insert(insert_at + i, item);
        }

        pl.items = remaining;
        drop(pl);
        self.bump_version();
    }

    /// Set the cursor (what's playing / should play).
    pub fn set_cursor(&self, id: Option<QueueItemId>) {
        let mut pl = self.playlist.write();
        pl.cursor = id;
        drop(pl);
        self.bump_version();
    }

    /// Get the current cursor ID.
    pub fn cursor(&self) -> Option<QueueItemId> {
        self.playlist.read().cursor
    }

    /// Clear the entire playlist + cursor.
    pub fn clear_playlist(&self) {
        let mut pl = self.playlist.write();
        pl.items.clear();
        pl.cursor = None;
        drop(pl);
        self.bump_version();
    }

    // --- Called from decode thread (gapless) ---

    /// Advance cursor to the next Ready item. Returns (id, path) if found.
    /// Moves the cursor. Used for explicit next-track commands.
    pub fn advance_cursor(&self) -> Option<(QueueItemId, PathBuf)> {
        let mut pl = self.playlist.write();
        let cursor_pos = match pl.cursor {
            Some(cid) => pl.items.iter().position(|item| item.id == cid),
            None => None,
        };

        let start = match cursor_pos {
            Some(pos) => pos + 1,
            None => 0,
        };

        // Find next Ready item after cursor.
        for i in start..pl.items.len() {
            if matches!(pl.items[i].load_state, LoadState::Ready) {
                let item = &pl.items[i];
                let result = (item.id, item.path.clone());
                pl.cursor = Some(item.id);
                drop(pl);
                self.bump_version();
                return Some(result);
            }
        }
        None
    }

    /// Peek at the next Ready item after a given item ID WITHOUT moving the cursor.
    /// Used by the decode thread for gapless lookahead — the cursor is moved
    /// later by update_playback_state when playback actually reaches the track.
    pub fn peek_next_ready_after(&self, after_id: QueueItemId) -> Option<(QueueItemId, PathBuf)> {
        let pl = self.playlist.read();
        let pos = pl.items.iter().position(|item| item.id == after_id);

        let start = match pos {
            Some(p) => p + 1,
            None => 0,
        };

        for i in start..pl.items.len() {
            if matches!(pl.items[i].load_state, LoadState::Ready) {
                let item = &pl.items[i];
                return Some((item.id, item.path.clone()));
            }
        }
        None
    }

    /// Retreat cursor to the previous item. Returns (id, path) if found.
    /// For prev_track — goes to the item before cursor regardless of load state.
    pub fn retreat_cursor(&self) -> Option<(QueueItemId, PathBuf)> {
        let mut pl = self.playlist.write();
        let cursor_pos = match pl.cursor {
            Some(cid) => pl.items.iter().position(|item| item.id == cid),
            None => None,
        };

        let prev_pos = cursor_pos.and_then(|p| p.checked_sub(1));

        match prev_pos {
            Some(pos) => {
                let item = &pl.items[pos];
                let result = (item.id, item.path.clone());
                pl.cursor = Some(item.id);
                drop(pl);
                self.bump_version();
                Some(result)
            }
            None => None,
        }
    }

    // --- Called from resolve thread ---

    /// Update the load state of a playlist item. Safe — just a field update under lock.
    pub fn update_load_state(&self, id: QueueItemId, new_state: LoadState) {
        let mut pl = self.playlist.write();
        if let Some(item) = pl.items.iter_mut().find(|item| item.id == id) {
            item.load_state = new_state;
        }
        drop(pl);
        self.bump_version();
    }

    /// Get the path of an item if it's Ready.
    pub fn item_path_if_ready(&self, id: QueueItemId) -> Option<PathBuf> {
        let pl = self.playlist.read();
        pl.items.iter().find(|item| item.id == id).and_then(|item| {
            if matches!(item.load_state, LoadState::Ready) {
                Some(item.path.clone())
            } else {
                None
            }
        })
    }

    /// Check if the cursor is on the given item.
    pub fn is_cursor(&self, id: QueueItemId) -> bool {
        self.playlist.read().cursor == Some(id)
    }

    // --- Snapshot helpers for undo ---

    /// Get the full playlist snapshot (items + cursor) for undo of ClearPlaylist.
    pub fn snapshot_playlist(&self) -> (Vec<PlaylistItem>, Option<QueueItemId>) {
        let pl = self.playlist.read();
        (pl.items.clone(), pl.cursor)
    }

    /// Get an item by ID (for undo of RemoveFromPlaylist).
    pub fn get_item(&self, id: QueueItemId) -> Option<PlaylistItem> {
        let pl = self.playlist.read();
        pl.items.iter().find(|item| item.id == id).cloned()
    }

    /// Get the ID of the item immediately before the given ID (None if first).
    pub fn item_before(&self, id: QueueItemId) -> Option<QueueItemId> {
        let pl = self.playlist.read();
        let pos = pl.items.iter().position(|item| item.id == id)?;
        if pos == 0 {
            None
        } else {
            Some(pl.items[pos - 1].id)
        }
    }

    /// For each ID, get the ID of the item before it (or None if first).
    /// Used to snapshot positions before a batch move for undo.
    pub fn items_before(&self, ids: &[QueueItemId]) -> Vec<(QueueItemId, Option<QueueItemId>)> {
        let pl = self.playlist.read();
        ids.iter()
            .filter_map(|&id| {
                let pos = pl.items.iter().position(|item| item.id == id)?;
                let before = if pos == 0 {
                    None
                } else {
                    Some(pl.items[pos - 1].id)
                };
                Some((id, before))
            })
            .collect()
    }

    /// Restore a full playlist from snapshot (for redo of ClearPlaylist undo).
    pub fn restore_playlist(&self, items: Vec<PlaylistItem>, cursor: Option<QueueItemId>) {
        let mut pl = self.playlist.write();
        pl.items = items;
        pl.cursor = cursor;
        drop(pl);
        self.bump_version();
    }

    /// Remove multiple items by IDs.
    pub fn remove_items(&self, ids: &[QueueItemId]) {
        use std::collections::HashSet;
        let id_set: HashSet<QueueItemId> = ids.iter().copied().collect();
        let mut pl = self.playlist.write();
        pl.items.retain(|item| !id_set.contains(&item.id));
        if let Some(cursor) = pl.cursor
            && id_set.contains(&cursor)
        {
            pl.cursor = None;
        }
        drop(pl);
        self.bump_version();
    }

    /// Insert a single item after a given ID (or at front if None).
    pub fn insert_item_at(&self, item: PlaylistItem, after: Option<QueueItemId>) {
        let mut pl = self.playlist.write();
        let insert_at = match after {
            Some(after_id) => {
                match pl.items.iter().position(|i| i.id == after_id) {
                    Some(pos) => pos + 1,
                    None => pl.items.len(), // fallback
                }
            }
            None => 0,
        };
        pl.items.insert(insert_at, item);
        drop(pl);
        self.bump_version();
    }

    /// Move a single item to after `after` (or to front if None).
    pub fn move_item_to(&self, id: QueueItemId, after: Option<QueueItemId>) {
        let mut pl = self.playlist.write();
        let Some(from) = pl.items.iter().position(|item| item.id == id) else {
            return;
        };
        let item = pl.items.remove(from);
        let insert_at = match after {
            Some(after_id) => match pl.items.iter().position(|i| i.id == after_id) {
                Some(pos) => pos + 1,
                None => pl.items.len(),
            },
            None => 0,
        };
        pl.items.insert(insert_at, item);
        drop(pl);
        self.bump_version();
    }

    /// Batch move: reposition each item to after its given predecessor.
    /// Processes in order so earlier insertions don't corrupt later positions.
    pub fn move_items_to(&self, entries: &[(QueueItemId, Option<QueueItemId>)]) {
        for &(id, after) in entries {
            self.move_item_to(id, after);
        }
    }

    // --- Called from UI thread (read lock) ---

    /// Derive the visible queue from the playlist + cursor. O(n).
    /// Called once per UI tick.
    pub fn derive_visible_queue(&self) -> VisibleQueueSnapshot {
        let pl = self.playlist.read();
        let track_info = self.track_info.read();

        let cursor_pos = match pl.cursor {
            Some(cid) => pl.items.iter().position(|item| item.id == cid),
            None => None,
        };

        let mut entries = Vec::with_capacity(pl.items.len());
        let mut finished_count = 0;
        let mut has_playing = false;
        let mut queue_count = 0;

        for (i, item) in pl.items.iter().enumerate() {
            let (status, dl_progress) = match cursor_pos {
                Some(cp) if i < cp => {
                    finished_count += 1;
                    (QueueEntryStatus::Played, None)
                }
                Some(cp) if i == cp => {
                    has_playing = true;
                    // Cursor item: playing if loaded, priority pending if not.
                    let status = match &item.load_state {
                        LoadState::Ready => QueueEntryStatus::Playing,
                        LoadState::Downloading { .. } => QueueEntryStatus::PriorityPending,
                        LoadState::Pending => QueueEntryStatus::PriorityPending,
                        LoadState::Failed(_) => QueueEntryStatus::Failed,
                    };
                    let dl = match &item.load_state {
                        LoadState::Downloading { downloaded, total } => Some((*downloaded, *total)),
                        _ => None,
                    };
                    (status, dl)
                }
                _ => {
                    // After cursor (or no cursor) — upcoming.
                    queue_count += 1;
                    let (status, dl) = match &item.load_state {
                        LoadState::Ready => (QueueEntryStatus::Queued, None),
                        LoadState::Downloading { downloaded, total } => {
                            (QueueEntryStatus::Downloading, Some((*downloaded, *total)))
                        }
                        LoadState::Pending => (QueueEntryStatus::Downloading, None),
                        LoadState::Failed(_) => (QueueEntryStatus::Failed, None),
                    };
                    (status, dl)
                }
            };

            // Override duration from TrackInfo if we have it and this is playing.
            let duration_ms =
                if has_playing && status == QueueEntryStatus::Playing && item.duration_ms.is_none()
                {
                    track_info.as_ref().map(|ti| ti.duration_ms)
                } else {
                    item.duration_ms
                };

            entries.push(QueueEntry {
                id: item.id,
                path: item.path.clone(),
                title: item.title.clone(),
                artist: item.artist.clone(),
                album_artist: item.album_artist.clone(),
                album: item.album.clone(),
                year: item.year.clone(),
                codec: item.codec.clone(),
                track_number: item.track_number,
                disc: item.disc,
                duration_ms,
                status,
                download_progress: dl_progress,
            });
        }

        VisibleQueueSnapshot {
            entries,
            finished_count,
            has_playing,
            queue_count,
        }
    }
}
