use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Playback state — maps to wire values for FFI.
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
    pub path: PathBuf,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub channels: u16,
    pub duration_ms: u64,
}

/// Status of a track in the queue — for UI display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueEntryStatus {
    Queued,
    Playing,
    Downloading,
    Failed,
}

/// Metadata about a track for queue display. Registered before/during download.
#[derive(Debug, Clone)]
pub struct QueueEntryMeta {
    pub title: String,
    pub artist: String,
    pub duration_ms: Option<u64>,
    pub status: QueueEntryStatus,
}

/// A single entry in the UI-visible queue snapshot.
#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub duration_ms: Option<u64>,
    pub status: QueueEntryStatus,
}

/// Shared player state — atomics for lock-free reads from UI thread.
///
/// The engine writes these, the UI/FFI reads them. No mutexes in the hot path.
#[derive(Debug)]
pub struct SharedPlayerState {
    state: AtomicU8,
    position_ms: AtomicU64,
    track_info: parking_lot::RwLock<Option<TrackInfo>>,
    /// Shadow queue — UI-visible snapshot of the play queue with metadata.
    queue_snapshot: parking_lot::RwLock<Vec<QueueEntry>>,
    /// Bumped on every queue snapshot update so UI can skip redundant redraws.
    queue_version: AtomicU64,
    /// Metadata cache keyed by path — set by resolve/download thread, read by queue builder.
    track_metadata: parking_lot::RwLock<HashMap<PathBuf, QueueEntryMeta>>,
}

impl SharedPlayerState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(PlaybackState::Stopped as u8),
            position_ms: AtomicU64::new(0),
            track_info: parking_lot::RwLock::new(None),
            queue_snapshot: parking_lot::RwLock::new(Vec::new()),
            queue_version: AtomicU64::new(0),
            track_metadata: parking_lot::RwLock::new(HashMap::new()),
        })
    }

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

    // --- Queue snapshot ---

    pub fn queue_snapshot(&self) -> Vec<QueueEntry> {
        self.queue_snapshot.read().clone()
    }

    pub fn set_queue_snapshot(&self, entries: Vec<QueueEntry>) {
        *self.queue_snapshot.write() = entries;
        self.queue_version.fetch_add(1, Ordering::Relaxed);
    }

    pub fn queue_version(&self) -> u64 {
        self.queue_version.load(Ordering::Relaxed)
    }

    // --- Track metadata cache (for download status) ---

    pub fn set_track_meta(&self, path: PathBuf, meta: QueueEntryMeta) {
        self.track_metadata.write().insert(path, meta);
    }

    pub fn track_meta(&self, path: &PathBuf) -> Option<QueueEntryMeta> {
        self.track_metadata.read().get(path).cloned()
    }

    pub fn update_track_status(&self, path: &PathBuf, status: QueueEntryStatus) {
        if let Some(meta) = self.track_metadata.write().get_mut(path) {
            meta.status = status;
        }
    }
}
