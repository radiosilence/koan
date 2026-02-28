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
    Played,
    Downloading,
    /// User double-clicked — this track is priority, will play when ready.
    PriorityPending,
    Failed,
}

/// Metadata about a track for queue display. Registered before/during download.
#[derive(Debug, Clone)]
pub struct QueueEntryMeta {
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
}

/// A single entry in the UI-visible queue snapshot.
#[derive(Debug, Clone)]
pub struct QueueEntry {
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
}

/// Pre-built visible queue — single atomic snapshot combining finished, playing,
/// queued, and pending entries. Built by the player/decode thread after every
/// mutation so the UI never reads inconsistent state across multiple locks.
#[derive(Debug, Clone, Default)]
pub struct VisibleQueueSnapshot {
    pub entries: Vec<QueueEntry>,
    pub finished_count: usize,
    pub has_playing: bool,
    pub queue_count: usize,
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
    /// Pending queue — tracks being resolved/downloaded, not yet in the player queue.
    /// The UI appends these after the real queue snapshot.
    pending_queue: parking_lot::RwLock<Vec<QueueEntry>>,
    /// Tracks that have finished playing — for playlist-style display.
    /// Updated by the decode thread (gapless) and player commands (next/skip).
    finished_paths: parking_lot::RwLock<Vec<PathBuf>>,
    /// Pre-built visible queue — the UI reads this single lock instead of 4.
    visible_queue: parking_lot::RwLock<VisibleQueueSnapshot>,
    /// Download progress keyed by cache path — (downloaded_bytes, total_bytes).
    /// Updated by the download thread, read by the queue view for progress display.
    download_progress: parking_lot::RwLock<HashMap<PathBuf, (u64, u64)>>,
    /// Playback generation — incremented each start_playback so stale decode
    /// thread callbacks (from a previous start_playback) can detect they're
    /// outdated and skip state mutations.
    playback_generation: AtomicU64,
    /// Priority play target — when the user double-clicks a pending (downloading)
    /// track, store its path here. The resolve thread checks after each download
    /// and plays it immediately when ready.
    priority_play: parking_lot::RwLock<Option<PathBuf>>,
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
            pending_queue: parking_lot::RwLock::new(Vec::new()),
            finished_paths: parking_lot::RwLock::new(Vec::new()),
            visible_queue: parking_lot::RwLock::new(VisibleQueueSnapshot::default()),
            download_progress: parking_lot::RwLock::new(HashMap::new()),
            playback_generation: AtomicU64::new(0),
            priority_play: parking_lot::RwLock::new(None),
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
        // Also update pending queue entries so rebuild_visible_queue picks it up.
        let mut pending = self.pending_queue.write();
        for entry in pending.iter_mut() {
            if &entry.path == path {
                entry.status = status;
            }
        }
    }

    // --- Pending queue (pre-download) ---

    pub fn set_pending_queue(&self, entries: Vec<QueueEntry>) {
        *self.pending_queue.write() = entries;
        self.rebuild_visible_queue();
    }

    /// Remove a track from the pending queue (e.g. after it's been enqueued in the real queue).
    pub fn remove_pending(&self, path: &PathBuf) {
        let mut pending = self.pending_queue.write();
        pending.retain(|e| &e.path != path);
        drop(pending);
        self.rebuild_visible_queue();
    }

    /// Remove from pending queue by index.
    pub fn remove_pending_at(&self, index: usize) {
        let mut pending = self.pending_queue.write();
        if index < pending.len() {
            pending.remove(index);
        }
        drop(pending);
        self.rebuild_visible_queue();
    }

    /// Reorder within pending queue.
    pub fn move_pending(&self, from: usize, to: usize) {
        let mut pending = self.pending_queue.write();
        if from < pending.len() && to < pending.len() {
            let entry = pending.remove(from);
            pending.insert(to, entry);
        }
        drop(pending);
        self.rebuild_visible_queue();
    }

    pub fn queue_snapshot_len(&self) -> usize {
        self.queue_snapshot.read().len()
    }

    pub fn pending_queue_len(&self) -> usize {
        self.pending_queue.read().len()
    }

    /// Get the combined queue: real queue snapshot + pending entries.
    pub fn full_queue(&self) -> Vec<QueueEntry> {
        let mut combined = self.queue_snapshot.read().clone();
        combined.extend(self.pending_queue.read().clone());
        combined
    }

    // --- Finished paths (played track history) ---

    pub fn push_finished(&self, path: PathBuf) {
        self.finished_paths.write().push(path);
    }

    /// Push multiple paths to finished in a single lock acquisition.
    /// Prevents the UI from seeing partially-updated finished state.
    pub fn extend_finished(&self, paths: impl IntoIterator<Item = PathBuf>) {
        self.finished_paths.write().extend(paths);
    }

    pub fn pop_finished(&self) -> Option<PathBuf> {
        self.finished_paths.write().pop()
    }

    pub fn clear_finished(&self) {
        self.finished_paths.write().clear();
    }

    pub fn finished_paths(&self) -> Vec<PathBuf> {
        self.finished_paths.read().clone()
    }

    pub fn finished_count(&self) -> usize {
        self.finished_paths.read().len()
    }

    /// Drain finished paths from `index` onward, returning the removed paths.
    pub fn drain_finished_from(&self, index: usize) -> Vec<PathBuf> {
        let mut finished = self.finished_paths.write();
        if index >= finished.len() {
            return vec![];
        }
        finished.drain(index..).collect()
    }

    // --- Download progress ---

    pub fn set_download_progress(&self, path: PathBuf, downloaded: u64, total: u64) {
        self.download_progress
            .write()
            .insert(path, (downloaded, total));
    }

    pub fn download_progress(&self, path: &PathBuf) -> Option<(u64, u64)> {
        self.download_progress.read().get(path).copied()
    }

    pub fn clear_download_progress(&self, path: &PathBuf) {
        self.download_progress.write().remove(path);
    }

    pub fn all_download_progress(&self) -> HashMap<PathBuf, (u64, u64)> {
        self.download_progress.read().clone()
    }

    // --- Playback generation (stale callback detection) ---

    /// Increment the generation counter. Returns the new value.
    /// Called by start_playback before creating decode callbacks.
    pub fn bump_generation(&self) -> u64 {
        self.playback_generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn generation(&self) -> u64 {
        self.playback_generation.load(Ordering::Relaxed)
    }

    // --- Priority play (pending track interrupt) ---

    pub fn set_priority_play(&self, path: PathBuf) {
        *self.priority_play.write() = Some(path);
    }

    /// Take the priority play path (returns and clears it).
    pub fn take_priority_play(&self) -> Option<PathBuf> {
        self.priority_play.write().take()
    }

    pub fn priority_play_path(&self) -> Option<PathBuf> {
        self.priority_play.read().clone()
    }

    /// Build queue entries from paths using the metadata cache.
    pub fn rebuild_queue_snapshot_from_paths(&self, paths: Vec<PathBuf>) {
        let meta_cache = self.track_metadata.read();
        let entries: Vec<QueueEntry> = paths
            .into_iter()
            .map(|path| {
                let mut entry = Self::build_entry_from_meta(&path, &meta_cache);
                // Preserve original status from metadata (Queued/Downloading/etc).
                entry.status = meta_cache
                    .get(&path)
                    .map(|m| m.status)
                    .unwrap_or(QueueEntryStatus::Queued);
                entry
            })
            .collect();
        drop(meta_cache);
        self.set_queue_snapshot(entries);
    }

    // --- Visible queue (atomic snapshot for UI) ---

    /// Get the pre-built visible queue snapshot. Single lock read — no TOCTOU.
    pub fn visible_queue(&self) -> VisibleQueueSnapshot {
        self.visible_queue.read().clone()
    }

    /// Build the complete visible queue from all state and write it atomically.
    /// Called from the player/decode thread after all mutations for a command.
    pub fn rebuild_visible_queue(&self) {
        let finished = self.finished_paths.read();
        let track_info = self.track_info.read();
        let queue = self.queue_snapshot.read();
        let pending = self.pending_queue.read();
        let meta_cache = self.track_metadata.read();

        let current_path = track_info.as_ref().map(|i| &i.path);
        let mut entries = Vec::new();

        // Finished entries (filter current track to handle race).
        let mut finished_count = 0;
        for path in finished.iter() {
            if Some(path) == current_path {
                continue;
            }
            let mut entry = Self::build_entry_from_meta(path, &meta_cache);
            entry.status = QueueEntryStatus::Played;
            entries.push(entry);
            finished_count += 1;
        }

        // Playing entry.
        let has_playing = track_info.is_some();
        if let Some(info) = &*track_info {
            let mut entry = Self::build_entry_from_meta(&info.path, &meta_cache);
            entry.status = QueueEntryStatus::Playing;
            // Use duration from TrackInfo if metadata doesn't have it.
            if entry.duration_ms.is_none() {
                entry.duration_ms = Some(info.duration_ms);
            }
            entries.push(entry);
        }

        // Queue entries (skip first occurrence of current track for stale snapshot).
        let mut skipped_current = false;
        let mut queue_count = 0;
        for entry in queue.iter() {
            if !skipped_current
                && let Some(cp) = current_path
                && &entry.path == cp
            {
                skipped_current = true;
                continue;
            }
            entries.push(entry.clone());
            queue_count += 1;
        }

        // Pending entries.
        for entry in pending.iter() {
            entries.push(entry.clone());
        }

        // Release read locks before acquiring write lock.
        drop(finished);
        drop(track_info);
        drop(queue);
        drop(pending);
        drop(meta_cache);

        *self.visible_queue.write() = VisibleQueueSnapshot {
            entries,
            finished_count,
            has_playing,
            queue_count,
        };
        self.queue_version.fetch_add(1, Ordering::Relaxed);
    }

    /// Build a QueueEntry from a path using the metadata cache.
    fn build_entry_from_meta(
        path: &PathBuf,
        meta_cache: &HashMap<PathBuf, QueueEntryMeta>,
    ) -> QueueEntry {
        let meta = meta_cache.get(path);
        QueueEntry {
            title: meta.map(|m| m.title.clone()).unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into()
            }),
            artist: meta.map(|m| m.artist.clone()).unwrap_or_default(),
            album_artist: meta.map(|m| m.album_artist.clone()).unwrap_or_default(),
            album: meta.map(|m| m.album.clone()).unwrap_or_default(),
            year: meta.and_then(|m| m.year.clone()),
            codec: meta.and_then(|m| m.codec.clone()),
            track_number: meta.and_then(|m| m.track_number),
            disc: meta.and_then(|m| m.disc),
            duration_ms: meta.and_then(|m| m.duration_ms),
            status: QueueEntryStatus::Queued,
            path: path.clone(),
        }
    }
}
