use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use uuid::Uuid;

use crate::remote::downloads::{ByteFeed, DownloadStore};

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
        // The tail: a v7's leading hex is its timestamp, shared by a whole batch.
        let hex = self.0.simple().to_string();
        write!(f, "QId({})", &hex[hex.len() - 8..])
    }
}

/// A step the decoder took through the queue from `after`, to `next`: the
/// first item after it that has not failed, or the end of the queue. The
/// decoder queues `next` as `chosen` when it is Ready, and stops at it when it
/// is still arriving.
#[derive(Debug, Clone)]
pub struct Lookahead {
    pub after: QueueItemId,
    pub next: Option<QueueItemId>,
    pub chosen: Option<(QueueItemId, PathBuf)>,
}

/// The first item after `after` that has not failed: `Some(None)` at the end
/// of the queue, `None` when `after` is not in it.
fn first_unfailed_after(
    items: &[PlaylistItem],
    after: QueueItemId,
) -> Option<Option<&PlaylistItem>> {
    let at = items.iter().position(|item| item.id == after)?;
    Some(
        items[at + 1..]
            .iter()
            .find(|item| !matches!(item.state, ItemState::Failed(_))),
    )
}

/// Set in `SharedPlayerState::state` beside the playback state while the
/// player waits for a track.
const WAITING: u8 = 0x80;

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
#[derive(Debug, Clone, PartialEq)]
pub struct TrackInfo {
    pub id: QueueItemId,
    pub path: PathBuf,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: Option<u16>,
    pub bitrate_kbps: Option<u32>,
    pub channels: u16,
    pub duration_ms: u64,
}

// --- Playlist data model ---

/// Minimum bytes written before streaming playback can begin.
pub const STREAM_THRESHOLD: u64 = 256 * 1024;

/// Held back from the seekable extent of a downloading track.
///
/// Bytes are converted to time at the average bitrate, so on VBR the estimate
/// wanders either side of the truth; landing short of the write head costs a
/// couple of seconds of reach and landing past it costs a stall.
pub const SEEK_SAFETY_MS: u64 = 2_000;

/// What a playlist item can say about itself.
///
/// Only what is true of the item regardless of any transfer: whether the bytes
/// at its path can be played. Whether one is *arriving* is the download store's
/// business, and asking the item would mean two accounts of one fact that have
/// to be kept in step. Read [`LoadState`] for the two
/// together.
///
/// Once a transfer ends, this is written before anything is woken: a reader
/// waiting on the transfer learns how it ended from here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ItemState {
    /// Nothing has resolved this yet.
    #[default]
    Pending,
    /// The file at `path` is there and playable.
    Ready,
    /// It cannot be made playable, and this is why. Not only download
    /// failures: a track with no local file and no remote copy fails here
    /// without a transfer ever being attempted.
    Failed(String),
}

/// An item's state, and any transfer against it, as one answer.
///
/// Derived rather than stored. `Downloading` carries the store's own figures —
/// the very same counter the downloader writes — so there is nothing to copy
/// and nothing that can drift.
#[derive(Debug, Clone)]
pub enum LoadState {
    Pending,
    Downloading {
        /// Where the bytes are going: the in-progress `.part` file, not the
        /// destination it is renamed to at the end.
        path: PathBuf,
        /// Total bytes expected, or 0 when the server sent no Content-Length.
        total: u64,
        /// How many bytes have landed. The download thread writes it per chunk
        /// without taking any lock the player holds.
        bytes_written: Arc<ByteFeed>,
    },
    Ready,
    Failed(String),
}

impl LoadState {
    /// An item's state, with whatever the download store says about it.
    ///
    /// The item's own state stands once it is anything but `Pending`: a
    /// transfer's end is written to every entry waiting on it before the
    /// transfer itself is marked settled. While it is pending, a listed
    /// transfer for its track is what is happening to it — whichever entry
    /// that transfer was started for.
    pub fn of(item: &PlaylistItem, downloads: &DownloadStore) -> Self {
        match &item.state {
            ItemState::Ready => Self::Ready,
            ItemState::Failed(reason) => Self::Failed(reason.clone()),
            ItemState::Pending => match item.db_id.and_then(|id| downloads.live(id)) {
                Some(live) => Self::Downloading {
                    path: live.source,
                    total: live.total,
                    bytes_written: live.written,
                },
                None => Self::Pending,
            },
        }
    }
}

/// Resolved playback source for a playlist item.
pub enum PlaybackSource {
    /// File fully downloaded — play from path.
    Ready(PathBuf),
    /// File being downloaded — enough data buffered to start streaming.
    Streaming {
        path: PathBuf,
        bytes_written: Arc<crate::remote::downloads::ByteFeed>,
        total: u64,
    },
}

/// A single item in the playlist. Created once when tracks are added to the playlist.
#[derive(Debug, Clone)]
pub struct PlaylistItem {
    pub id: QueueItemId,
    /// Database track ID — set for tracks loaded from DB, used for downloads.
    pub db_id: Option<i64>,
    /// The playlist entry this came from, when it came from a playlist.
    ///
    /// A playlist may hold the same track twice, and two copies are two queue
    /// items. Without this a playlist row can only ask "is my *track* playing?"
    /// and both copies answer yes. The entry id is the one thing that tells
    /// them apart, so the queue carries it.
    pub playlist_entry_id: Option<i64>,
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
    /// What the item can say about itself. Ask [`SharedPlayerState::load_state`]
    /// for this together with any transfer against it.
    pub state: ItemState,
}

/// The playlist — one flat array, one cursor. Everything else derived.
#[derive(Debug, Clone, Default)]
pub struct Playlist {
    pub items: Vec<PlaylistItem>,
    pub cursor: Option<QueueItemId>,
}

// --- UI view types ---

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

impl QueueEntryStatus {
    /// The item under the cursor. Every front end maps it through here, so
    /// the transport and the queue row agree on what it is doing.
    ///
    /// `PriorityPending` is waiting its turn with no bytes moving; once its
    /// transfer starts it is `Downloading`, with progress to draw.
    pub fn at_cursor(state: &ItemState, transferring: bool) -> Self {
        match state {
            ItemState::Ready => Self::Playing,
            ItemState::Failed(_) => Self::Failed,
            ItemState::Pending if transferring => Self::Downloading,
            ItemState::Pending => Self::PriorityPending,
        }
    }
}

/// A single entry in the UI-visible queue snapshot.
#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub id: QueueItemId,
    /// Database track ID — set for tracks loaded from DB, used for downloads.
    pub db_id: Option<i64>,
    /// The playlist row this came from — see `PlaylistItem::playlist_entry_id`.
    pub playlist_entry_id: Option<i64>,
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
    /// Why this entry cannot play, when `status` is `Failed`.
    pub error: Option<String>,
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
    /// The playback state, with `WAITING` set while the player waits for a
    /// track it was asked for. One atomic, so no reader sees one updated
    /// without the other.
    state: AtomicU8,
    /// Where a session starts, and where a stopped one stands. While one is
    /// running the playhead is read off the timeline instead.
    position_ms: AtomicU64,
    timeline: std::sync::OnceLock<Arc<crate::audio::buffer::PlaybackTimeline>>,
    track_info: parking_lot::RwLock<Option<TrackInfo>>,

    /// The playlist and its cursor, under one lock.
    playlist: parking_lot::RwLock<Playlist>,

    /// Bumped on every playlist mutation so UI can skip redundant redraws.
    playlist_version: AtomicU64,

    /// Bumped only when what a saved session holds changes: the items and
    /// their metadata, not the cursor or load states. What decides whether
    /// the saved queue has to be written again.
    content_version: AtomicU64,

    /// Bumped when the set of items still waiting for a file may have
    /// changed: items added or removed, or one put back to `Pending`. What the
    /// download queue follows; a download landing or the cursor moving does
    /// not move it.
    pending_version: AtomicU64,

    /// Every transfer this player's items are fetched by.
    downloads: Arc<DownloadStore>,

    /// Set by external signals (e.g. souvlaki Quit event) to request clean shutdown.
    quit_requested: AtomicBool,

    /// Set when metadata has been refreshed (e.g. download completed while streaming).
    /// The UI loop checks this to force a souvlaki/cover-art update without a track change.
    metadata_refresh_pending: AtomicBool,

    /// The rate the output device settled at for the current track, or 0 when
    /// nothing has played yet. Compared against the source rate, it is the one
    /// thing koan can say for certain about the path to the DAC: whether it
    /// handed the device the samples as they are, or something had to resample
    /// to reach it. Everything past that — other clients, the volume stage — is
    /// the system's, and not ours to claim.
    output_sample_rate: AtomicU64,

    /// What DSP is doing to the current session's audio. `None` is the
    /// bit-perfect path.
    dsp: parking_lot::RwLock<Option<crate::audio::dsp::DspStatus>>,
    /// The playhead of a renderer this koan is playing to, which keeps its
    /// own clock: where it was last heard to be, and since when it has been
    /// running from there. Read in place of the timeline while set.
    renderer_clock: parking_lot::Mutex<Option<RendererClock>>,

    /// The UPnP renderer playing in place of the local output, if one is.
    renderer: parking_lot::RwLock<Option<crate::upnp::Output>>,
}

/// A renderer's playhead: `position_ms`, plus the time since `running` if it
/// is playing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RendererClock {
    pub position_ms: u64,
    pub running: Option<std::time::Instant>,
}

impl RendererClock {
    pub fn now_ms(&self) -> u64 {
        self.position_ms + self.running.map_or(0, |at| at.elapsed().as_millis() as u64)
    }
}

impl SharedPlayerState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(PlaybackState::Stopped as u8),
            position_ms: AtomicU64::new(0),
            timeline: std::sync::OnceLock::new(),
            track_info: parking_lot::RwLock::new(None),
            playlist: parking_lot::RwLock::new(Playlist::default()),
            playlist_version: AtomicU64::new(0),
            content_version: AtomicU64::new(0),
            pending_version: AtomicU64::new(0),
            downloads: DownloadStore::new(),
            quit_requested: AtomicBool::new(false),
            metadata_refresh_pending: AtomicBool::new(false),
            output_sample_rate: AtomicU64::new(0),
            dsp: parking_lot::RwLock::new(None),
            renderer_clock: parking_lot::Mutex::new(None),
            renderer: parking_lot::RwLock::new(None),
        })
    }

    // --- Playback state ---

    fn transport(&self) -> (PlaybackState, bool) {
        let bits = self.state.load(Ordering::Acquire);
        (PlaybackState::from_u8(bits & !WAITING), bits & WAITING != 0)
    }

    pub fn playback_state(&self) -> PlaybackState {
        self.transport().0
    }

    pub fn set_playback_state(&self, state: PlaybackState) {
        self.set_transport(state, false);
    }

    /// Whether the player is waiting for a track it was asked for to arrive.
    /// Stopped while waiting, the track opens playing; paused, it opens paused.
    pub fn is_waiting(&self) -> bool {
        self.transport().1
    }

    /// Playing, or waiting for a track that will open playing: what a
    /// play/pause toggle pauses.
    pub fn wants_to_play(&self) -> bool {
        matches!(
            self.transport(),
            (PlaybackState::Playing, _) | (PlaybackState::Stopped, true)
        )
    }

    /// Nothing loaded and nothing waited for: where adding tracks starts them.
    pub fn is_idle(&self) -> bool {
        self.transport() == (PlaybackState::Stopped, false)
    }

    /// Publish the playback state and the wait together.
    pub fn set_transport(&self, state: PlaybackState, waiting: bool) {
        let bits = state as u8 | if waiting { WAITING } else { 0 };
        if self.state.swap(bits, Ordering::AcqRel) != bits {
            self.changed();
        }
    }

    /// Where the playhead is, read off the samples the output has played, so
    /// it is right whenever it is asked and nothing has to keep it up to date.
    pub fn position_ms(&self) -> u64 {
        let clock = *self.renderer_clock.lock();
        if let Some(clock) = clock {
            let at = clock.now_ms();
            let duration = self.duration_ms();
            return if duration > 0 { at.min(duration) } else { at };
        }
        if self.playback_state() != PlaybackState::Stopped
            && let Some((_, at)) = self.timeline.get().and_then(|t| t.playhead())
        {
            return at;
        }
        self.position_ms.load(Ordering::Acquire)
    }

    /// The timeline the playhead is read from. Set once, by the player.
    pub(crate) fn attach_timeline(&self, timeline: Arc<crate::audio::buffer::PlaybackTimeline>) {
        let _ = self.timeline.set(timeline);
    }

    pub fn set_position_ms(&self, pos: u64) {
        self.position_ms.store(pos, Ordering::Release);
        // Deliberately silent. The playhead advances on its own and is
        // published as an anchor rather than as a reading — a wake per
        // position would be the tick this whole arrangement removes. A seek, a
        // pause and a track change all move something else here as well, and
        // those are exactly the ones a client has to be told about.
    }

    /// Set by the player while a renderer is the output. A change to it is a
    /// change clients are told about: the clock only moves when the renderer
    /// is heard from, or when a command moved it.
    /// Whether the playhead is advancing on its own. Playing, but held, while
    /// a renderer has been told to play and has not yet started: a client
    /// counting on from the command would run ahead of the sound.
    pub fn playhead_moving(&self) -> bool {
        match *self.renderer_clock.lock() {
            Some(clock) => clock.running.is_some(),
            None => self.playback_state() == PlaybackState::Playing,
        }
    }

    pub(crate) fn renderer_clock(&self) -> Option<RendererClock> {
        *self.renderer_clock.lock()
    }

    pub(crate) fn set_renderer_clock(&self, clock: Option<RendererClock>) {
        let mut guard = self.renderer_clock.lock();
        if *guard != clock {
            *guard = clock;
            drop(guard);
            self.changed();
        }
    }

    /// The renderer this koan is playing to, when it is not its own output.
    pub fn renderer(&self) -> Option<crate::upnp::Output> {
        self.renderer.read().clone()
    }

    pub(crate) fn set_renderer(&self, output: Option<crate::upnp::Output>) {
        *self.renderer.write() = output;
        self.changed();
    }

    pub(crate) fn update_renderer(&self, f: impl FnOnce(&mut crate::upnp::Output)) {
        let changed = match self.renderer.write().as_mut() {
            Some(out) => {
                let before = out.clone();
                f(out);
                *out != before
            }
            None => false,
        };
        if changed {
            self.changed();
        }
    }

    pub fn track_info(&self) -> Option<TrackInfo> {
        self.track_info.read().clone()
    }

    pub fn set_track_info(&self, info: Option<TrackInfo>) {
        *self.track_info.write() = info;
        self.changed();
    }

    /// How far into the currently playing track a seek can land.
    ///
    /// A track on disk is seekable end to end. One still downloading is
    /// seekable only as far as its bytes reach: bytes map to time by the
    /// average bitrate, exact for lossless and CBR and drifting on VBR, which
    /// is what `SEEK_SAFETY_MS` covers. Zero when nothing is playing.
    ///
    /// The one value both the clamp in `Player::seek` and the extent front ends
    /// draw on the seek bar come from — a bar that shows a reachable position
    /// the player then refuses is worse than no bar.
    pub fn seekable_ms(&self) -> u64 {
        let Some(info) = self.track_info.read().clone() else {
            return 0;
        };

        // Released before the playlist lock is taken: derive_visible_queue takes
        // these two in the opposite order, so holding both would close a cycle.
        let pl = self.playlist.read();
        let Some(item) = pl.items.iter().find(|item| item.id == info.id) else {
            return info.duration_ms;
        };

        let LoadState::Downloading {
            total,
            bytes_written,
            ..
        } = self.load_state(item)
        else {
            return info.duration_ms;
        };

        // A container that could not describe itself from the bytes downloaded
        // states no duration, and cannot be seeked at all until the rest of it
        // lands — there is no index to seek against and no end to seek within.
        // Ogg is the one that does this; it keeps its duration in its last page.
        if info.duration_ms == 0 {
            return 0;
        }

        let written = bytes_written.load(Ordering::Acquire);
        let reached = if total > 0 && info.duration_ms > 0 {
            ((written as f64 / total as f64) * info.duration_ms as f64) as u64
        } else if let Some(kbps) = info.bitrate_kbps.filter(|k| *k > 0) {
            // No Content-Length. Bytes still say how much audio has arrived,
            // given what the probe measured the bitrate to be: 1 kbps is
            // 1 bit per ms, so bits divided by kbps is milliseconds.
            written.saturating_mul(8) / kbps as u64
        } else {
            // Nothing to derive a position from — forward seeking would be a
            // guess, so allow only what has already been played.
            return self.position_ms();
        };

        reached.saturating_sub(SEEK_SAFETY_MS).min(info.duration_ms)
    }

    /// The duration to show for what is playing.
    ///
    /// The container's own answer wherever it gave one. A partial file that
    /// could not be read far enough to state a duration has none, and the
    /// library's figure stands in — it came from the server, it is right, and
    /// a transport that reads 0:00 for nine hours of music is worse than one
    /// reading a figure the container has not caught up with yet.
    pub fn duration_ms(&self) -> u64 {
        let Some(info) = self.track_info.read().clone() else {
            return 0;
        };
        if info.duration_ms > 0 {
            return info.duration_ms;
        }
        // Released before the playlist lock, as everywhere else here.
        self.playlist
            .read()
            .items
            .iter()
            .find(|item| item.id == info.id)
            .and_then(|item| item.duration_ms)
            .unwrap_or(0)
    }

    /// `seekable_ms`, but `None` when the whole track is reachable — which is
    /// every track that is not mid-download. What a front end draws a boundary
    /// from: no boundary is the normal case and should cost no mark.
    pub fn seek_ceiling_ms(&self) -> Option<u64> {
        let duration = self.duration_ms();
        if duration == 0 {
            return None;
        }
        let seekable = self.seekable_ms();
        (seekable < duration).then_some(seekable)
    }

    /// Download fraction (0.0..1.0) for the currently playing track, if streaming.
    /// Returns `None` for fully-downloaded or non-playing tracks.
    pub fn current_download_fraction(&self) -> Option<f64> {
        // Released before the playlist lock is taken: derive_visible_queue takes
        // these two in the opposite order, so holding both would close a cycle.
        let id = self.track_info.read().as_ref()?.id;
        let pl = self.playlist.read();
        pl.items
            .iter()
            .find(|item| item.id == id)
            .and_then(|item| match self.load_state(item) {
                LoadState::Downloading {
                    bytes_written,
                    total,
                    ..
                } => {
                    let written = bytes_written.load(Ordering::Acquire);
                    (total > 0).then(|| (written as f64 / total as f64).min(1.0))
                }
                _ => None,
            })
    }

    // --- Quit ---

    pub fn request_quit(&self) {
        self.quit_requested.store(true, Ordering::Release);
    }

    pub fn quit_requested(&self) -> bool {
        self.quit_requested.load(Ordering::Acquire)
    }

    // --- Metadata refresh ---

    /// Signal that metadata has been refreshed mid-stream (e.g. download completed).
    /// The UI loop calls `take_metadata_refresh()` to consume this flag and
    /// force a souvlaki/cover-art update without waiting for a track change.
    pub fn signal_metadata_refresh(&self) {
        self.metadata_refresh_pending.store(true, Ordering::Release);
        self.changed();
    }

    /// Returns true and clears the flag if a metadata refresh is pending.
    pub fn take_metadata_refresh(&self) -> bool {
        self.metadata_refresh_pending
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    // --- Output device rate ---

    /// `None` until a track has started and the device rate is known.
    pub fn output_sample_rate(&self) -> Option<u32> {
        match self.output_sample_rate.load(Ordering::Acquire) {
            0 => None,
            rate => Some(rate as u32),
        }
    }

    pub fn set_output_sample_rate(&self, rate: u32) {
        self.output_sample_rate
            .store(u64::from(rate), Ordering::Release);
        self.changed();
    }

    /// Back to "not known yet", for the window where the device is between
    /// rates. A switch takes as long as the hardware needs to reclock — the
    /// better part of a second on USB — and the previous track's rate is not
    /// an answer for this one.
    pub fn clear_output_sample_rate(&self) {
        self.output_sample_rate.store(0, Ordering::Release);
    }

    // --- DSP ---

    pub fn dsp(&self) -> Option<crate::audio::dsp::DspStatus> {
        self.dsp.read().clone()
    }

    pub fn set_dsp(&self, status: Option<crate::audio::dsp::DspStatus>) {
        let mut dsp = self.dsp.write();
        if *dsp != status {
            *dsp = status;
            drop(dsp);
            self.changed();
        }
    }

    // --- Playlist version ---

    pub fn playlist_version(&self) -> u64 {
        self.playlist_version.load(Ordering::Acquire)
    }

    fn bump_version(&self) {
        self.playlist_version.fetch_add(1, Ordering::AcqRel);
        self.changed();
    }

    pub fn content_version(&self) -> u64 {
        self.content_version.load(Ordering::Acquire)
    }

    /// `bump_version`, for a change to what a saved session holds.
    fn bump_content(&self) {
        self.content_version.fetch_add(1, Ordering::AcqRel);
        self.pending_version.fetch_add(1, Ordering::AcqRel);
        self.bump_version();
    }

    /// See the field. Moves with every content change, and when an item goes
    /// back to `Pending`.
    pub fn pending_version(&self) -> u64 {
        self.pending_version.load(Ordering::Acquire)
    }

    /// The transfers this player's items are fetched by.
    pub fn downloads(&self) -> &Arc<DownloadStore> {
        &self.downloads
    }

    fn load_state(&self, item: &PlaylistItem) -> LoadState {
        LoadState::of(item, &self.downloads)
    }

    /// Say that something here moved, without saying what.
    ///
    /// Every version and atomic in this struct stays exactly as it was — they
    /// are what a watcher consults to find out what changed. This is what
    /// spares it looking when nothing did. See `crate::signal`.
    pub fn changed(&self) {
        crate::signal::engine_changed().bump();
    }

    // --- Playlist mutations (called from player thread via commands) ---

    /// Append items to the playlist.
    pub fn add_items(&self, items: Vec<PlaylistItem>) {
        let mut pl = self.playlist.write();
        pl.items.extend(items);
        drop(pl);
        self.bump_content();
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
        self.bump_content();
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
        self.bump_content();
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
        self.bump_content();
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
        self.bump_content();
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
        self.bump_content();
    }

    /// Set the cursor (what's playing / should play).
    pub fn set_cursor(&self, id: Option<QueueItemId>) {
        let mut pl = self.playlist.write();
        pl.cursor = id;
        drop(pl);
        self.bump_version();
    }

    pub fn cursor(&self) -> Option<QueueItemId> {
        self.playlist.read().cursor
    }

    /// The path of the item under the cursor, without copying the playlist.
    pub fn cursor_path(&self) -> Option<PathBuf> {
        let pl = self.playlist.read();
        let cursor = pl.cursor?;
        pl.items
            .iter()
            .find(|item| item.id == cursor)
            .map(|item| item.path.clone())
    }

    /// Clear the entire playlist + cursor.
    /// Swap the whole playlist for `items`, with no cursor, as one change.
    /// Returns what it held, for undo.
    ///
    /// One write and one version bump, not a clear and an add. The download
    /// queue reads the playlist on its own thread whenever it changes, and an
    /// empty playlist between the two would read as nothing wanted: every
    /// transfer for a track in both the old queue and the new one let go,
    /// cancelled, and started again.
    pub fn replace_playlist(
        &self,
        items: Vec<PlaylistItem>,
    ) -> (Vec<PlaylistItem>, Option<QueueItemId>) {
        let mut pl = self.playlist.write();
        let old = std::mem::replace(&mut pl.items, items);
        let cursor = pl.cursor.take();
        drop(pl);
        self.bump_content();
        (old, cursor)
    }

    pub fn clear_playlist(&self) {
        let mut pl = self.playlist.write();
        pl.items.clear();
        pl.cursor = None;
        drop(pl);
        self.bump_content();
    }

    // --- Called from decode thread (gapless) ---

    /// Move the cursor to the next item that can still play — the first item
    /// after the cursor that is not `Failed` — and return its ID.
    ///
    /// An item that is still downloading parks the cursor rather than being
    /// skipped, so playback resumes from it when its data lands. Skipping it
    /// would drop it from the queue for good.
    ///
    /// With no cursor set, starts from the top. A cursor pointing at an item
    /// that is no longer in the playlist yields `None` — restarting from the
    /// top would silently replay the queue.
    pub fn advance_cursor_loadable(&self) -> Option<QueueItemId> {
        let mut pl = self.playlist.write();
        let start = match pl.cursor {
            Some(cid) => pl.items.iter().position(|item| item.id == cid)? + 1,
            None => 0,
        };

        let next = pl
            .items
            .get(start..)?
            .iter()
            .find(|item| !matches!(item.state, ItemState::Failed(_)))
            .map(|item| item.id)?;

        pl.cursor = Some(next);
        drop(pl);
        self.bump_version();
        Some(next)
    }

    /// One step of the decoder's gapless lookahead from `after_id`. Does not
    /// move the cursor; `update_playback_state` does that when the playhead
    /// gets there.
    ///
    /// Failed items are passed over, as `advance_cursor_loadable` passes over
    /// them. A track still arriving is not: the decoder stops at it, the
    /// session drains, and the advance that follows waits for it. Passing over
    /// it would play the track after it and move the cursor beyond it, so it
    /// would never be heard.
    ///
    /// None when `after_id` has been removed: the lookahead then has nothing
    /// to follow, and starting from the top would gaplessly replay the queue.
    pub fn lookahead_after(&self, after_id: QueueItemId) -> Option<Lookahead> {
        let pl = self.playlist.read();
        let next = first_unfailed_after(&pl.items, after_id)?;
        Some(Lookahead {
            after: after_id,
            next: next.map(|item| item.id),
            chosen: next
                .filter(|item| matches!(item.state, ItemState::Ready))
                .map(|item| (item.id, item.path.clone())),
        })
    }

    /// Whether the decoder would still take `step`: the first item after
    /// `after` that has not failed is still `next`. A track it stopped at
    /// landing since is not a change, since the advance at the end of the
    /// session plays it in order; an edit that puts another track first is.
    pub fn still_follows(&self, step: &Lookahead) -> bool {
        let pl = self.playlist.read();
        first_unfailed_after(&pl.items, step.after)
            .is_some_and(|next| next.map(|item| item.id) == step.next)
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

    /// Update the load state of a playlist item.
    pub fn update_item_state(&self, id: QueueItemId, new_state: ItemState) {
        let pending = new_state == ItemState::Pending;
        let mut pl = self.playlist.write();
        if let Some(item) = pl.items.iter_mut().find(|item| item.id == id) {
            item.state = new_state;
        }
        drop(pl);
        if pending {
            self.pending_version.fetch_add(1, Ordering::AcqRel);
        }
        self.bump_version();
    }

    /// An item's own state, without the rest of it.
    pub fn item_state(&self, id: QueueItemId) -> Option<ItemState> {
        let pl = self.playlist.read();
        pl.items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.state.clone())
    }

    /// Take what a finished download's own tags can add.
    ///
    /// Streaming starts on partial Symphonia tags, so an item with nothing
    /// behind it takes the lot once the whole file is there. An item that came
    /// out of the library does not: the record is what the queue was built
    /// from and what every other track on it carries, and a file whose tags
    /// disagree — a server album titled one way, the file inside titled
    /// another — would split its album in two the moment it finished
    /// downloading. The duration is the file's to know either way.
    pub fn update_item_metadata(
        &self,
        id: QueueItemId,
        title: String,
        artist: String,
        album_artist: String,
        album: String,
        duration_ms: Option<u64>,
    ) {
        let mut pl = self.playlist.write();
        if let Some(item) = pl.items.iter_mut().find(|item| item.id == id) {
            if item.db_id.is_none() {
                item.title = title;
                item.artist = artist;
                item.album_artist = album_artist;
                item.album = album;
            }
            if let Some(dur) = duration_ms {
                item.duration_ms = Some(dur);
            }
        }
        drop(pl);
        self.bump_content();
    }

    /// Get the playback source for an item if it's ready to play.
    /// Returns `None` if not enough data is available yet.
    pub fn item_playback_source(&self, id: QueueItemId) -> Option<PlaybackSource> {
        let pl = self.playlist.read();
        pl.items
            .iter()
            .find(|item| item.id == id)
            .and_then(|item| match self.load_state(item) {
                LoadState::Ready => Some(PlaybackSource::Ready(item.path.clone())),
                LoadState::Downloading {
                    path,
                    total,
                    bytes_written,
                } => {
                    let written = bytes_written.load(Ordering::Acquire);
                    (written >= STREAM_THRESHOLD).then_some(PlaybackSource::Streaming {
                        path,
                        bytes_written,
                        total,
                    })
                }
                _ => None,
            })
    }

    /// Put back to `Pending` every queue item whose file has gone, and say
    /// which they were so they can be fetched again.
    ///
    /// The queue holds paths, and clearing downloads deletes the files under
    /// them. An item left claiming `Ready` opens nothing when it is played —
    /// it is not broken, it is a remote track that has to be fetched a second
    /// time. Only items with a database row behind them: one without has
    /// nowhere to be fetched from, and parking the cursor on it would be worse
    /// than letting it fail honestly.
    pub fn reset_items_with_missing_files(&self) -> Vec<(i64, QueueItemId)> {
        let mut pl = self.playlist.write();
        let mut reset = Vec::new();
        for item in pl.items.iter_mut() {
            let Some(db_id) = item.db_id else { continue };
            if !matches!(item.state, ItemState::Ready) {
                continue;
            }
            if item.path.exists() {
                continue;
            }
            item.state = ItemState::Pending;
            reset.push((db_id, item.id));
        }
        drop(pl);
        if !reset.is_empty() {
            self.pending_version.fetch_add(1, Ordering::AcqRel);
            self.bump_version();
        }
        reset
    }

    /// The item's path if it is `Ready`. A caller that can stream wants `item_playback_source`.
    pub fn item_path_if_ready(&self, id: QueueItemId) -> Option<PathBuf> {
        let pl = self.playlist.read();
        pl.items.iter().find(|item| item.id == id).and_then(|item| {
            if matches!(item.state, ItemState::Ready) {
                Some(item.path.clone())
            } else {
                None
            }
        })
    }

    pub fn is_cursor(&self, id: QueueItemId) -> bool {
        self.playlist.read().cursor == Some(id)
    }

    /// Get QueueItemIds of all playlist items sharing the same album as the given item.
    /// Matches on both album name and album artist to avoid false positives
    /// (e.g. two different "Greatest Hits" by different artists).
    pub fn same_album_item_ids(&self, id: QueueItemId) -> Vec<QueueItemId> {
        let pl = self.playlist.read();
        let Some(cursor) = pl.items.iter().find(|item| item.id == id) else {
            return vec![];
        };
        let album = cursor.album.clone();
        let album_artist = cursor.album_artist.clone();
        pl.items
            .iter()
            .filter(|item| {
                item.id != id && item.album == album && item.album_artist == album_artist
            })
            .map(|item| item.id)
            .collect()
    }

    /// Every playlist item still waiting for its file that has a track to
    /// fetch, as `(db_id, QueueItemId)`, in the order the player will reach
    /// it: from the cursor to the end, then from the top. What the download
    /// queue fetches, and in that order — the tracks before the cursor are
    /// the ones least likely to be played next.
    pub fn pending_downloads(&self) -> Vec<(i64, QueueItemId)> {
        let pl = self.playlist.read();
        let from = pl
            .cursor
            .and_then(|c| pl.items.iter().position(|item| item.id == c))
            .unwrap_or(0);
        let (before, after) = pl.items.split_at(from);
        after
            .iter()
            .chain(before)
            .filter(|item| matches!(item.state, ItemState::Pending))
            .filter_map(|item| item.db_id.map(|db_id| (db_id, item.id)))
            .collect()
    }

    /// Every entry with a library track, in the order `pending_downloads`
    /// gives, whatever its state: what the cache has to hold, downloaded or
    /// not, for the player to reach it.
    pub fn playback_order(&self) -> Vec<(i64, QueueItemId)> {
        let pl = self.playlist.read();
        let from = pl
            .cursor
            .and_then(|c| pl.items.iter().position(|item| item.id == c))
            .unwrap_or(0);
        let (before, after) = pl.items.split_at(from);
        after
            .iter()
            .chain(before)
            .filter_map(|item| item.db_id.map(|db_id| (db_id, item.id)))
            .collect()
    }

    /// Get the db_id for a specific playlist item.
    pub fn item_db_id(&self, id: QueueItemId) -> Option<i64> {
        let pl = self.playlist.read();
        pl.items
            .iter()
            .find(|item| item.id == id)
            .and_then(|item| item.db_id)
    }

    /// Get the load state of a specific playlist item.
    pub fn item_load_state(&self, id: QueueItemId) -> Option<LoadState> {
        let pl = self.playlist.read();
        pl.items
            .iter()
            .find(|item| item.id == id)
            .map(|item| self.load_state(item))
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

    /// Put the items in exactly this order.
    ///
    /// Items not named keep their relative order and follow at the end, so a
    /// stale order cannot lose anything. The items themselves are moved, not
    /// rebuilt: their ids, load states and download progress are what the rest
    /// of the player is holding on to.
    pub fn reorder_to(&self, order: &[QueueItemId]) {
        let mut pl = self.playlist.write();
        let mut taken: Vec<Option<PlaylistItem>> = pl.items.drain(..).map(Some).collect();
        let mut sorted = Vec::with_capacity(taken.len());
        for id in order {
            if let Some(slot) = taken
                .iter_mut()
                .find(|i| i.as_ref().is_some_and(|i| i.id == *id))
                && let Some(item) = slot.take()
            {
                sorted.push(item);
            }
        }
        sorted.extend(taken.into_iter().flatten());
        pl.items = sorted;
        drop(pl);
        self.bump_content();
    }

    /// For each ID, the ID of the item before it (or None if first), returned in
    /// playlist order regardless of the order `ids` arrives in.
    ///
    /// Undo replays these left to right, so an item whose recorded predecessor is
    /// also in `ids` must come after it — otherwise the predecessor is missing at
    /// replay time and the item lands at the end of the playlist instead.
    pub fn items_before(&self, ids: &[QueueItemId]) -> Vec<(QueueItemId, Option<QueueItemId>)> {
        use std::collections::HashSet;
        let wanted: HashSet<QueueItemId> = ids.iter().copied().collect();
        let pl = self.playlist.read();
        pl.items
            .iter()
            .enumerate()
            .filter(|(_, item)| wanted.contains(&item.id))
            .map(|(pos, item)| {
                let before = if pos == 0 {
                    None
                } else {
                    Some(pl.items[pos - 1].id)
                };
                (item.id, before)
            })
            .collect()
    }

    /// The nearest item before `id` that is not itself being removed — where
    /// playback resumes from after a batch delete that takes out the cursor.
    /// `None` means resume from the top of what survives.
    pub fn surviving_item_before(
        &self,
        id: QueueItemId,
        removed: &[QueueItemId],
    ) -> Option<QueueItemId> {
        use std::collections::HashSet;
        let removed: HashSet<QueueItemId> = removed.iter().copied().collect();
        let pl = self.playlist.read();
        let pos = pl.items.iter().position(|item| item.id == id)?;
        pl.items[..pos]
            .iter()
            .rev()
            .find(|item| !removed.contains(&item.id))
            .map(|item| item.id)
    }

    /// Restore a full playlist from snapshot (for redo of ClearPlaylist undo).
    pub fn restore_playlist(&self, items: Vec<PlaylistItem>, cursor: Option<QueueItemId>) {
        let mut pl = self.playlist.write();
        pl.items = items;
        pl.cursor = cursor;
        drop(pl);
        self.bump_content();
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
        self.bump_content();
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
        self.bump_content();
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
        self.bump_content();
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
        // Read before the playlist lock — see current_download_fraction.
        let playing_duration_ms = self.track_info.read().as_ref().map(|ti| ti.duration_ms);
        // One pass over the transfers rather than one lookup, and a path
        // cloned, per row.
        let transfers: HashMap<i64, (u64, u64)> = self
            .downloads
            .readings()
            .into_iter()
            .map(|r| (r.track_id, (r.written, r.total)))
            .collect();
        let pl = self.playlist.read();

        let cursor_pos = match pl.cursor {
            Some(cid) => pl.items.iter().position(|item| item.id == cid),
            None => None,
        };

        let mut entries = Vec::with_capacity(pl.items.len());
        let mut finished_count = 0;
        let mut has_playing = false;
        let mut queue_count = 0;

        for (i, item) in pl.items.iter().enumerate() {
            let is_cursor = cursor_pos == Some(i);
            let is_before_cursor = cursor_pos.is_some_and(|cp| i < cp);

            // The byte count is the download thread's own counter, written per
            // chunk without the playlist lock, so a transfer never bumps the
            // playlist version.
            let dl_progress = match item.state {
                ItemState::Pending => item.db_id.and_then(|id| transfers.get(&id).copied()),
                _ => None,
            };
            let transferring = dl_progress.is_some();

            let status = if is_cursor {
                has_playing = true;
                QueueEntryStatus::at_cursor(&item.state, transferring)
            } else if is_before_cursor {
                finished_count += 1;
                match &item.state {
                    ItemState::Ready => QueueEntryStatus::Played,
                    // A spinner only while bytes are moving: a track skipped
                    // past is not being fetched just for being behind the
                    // cursor.
                    ItemState::Pending if transferring => QueueEntryStatus::Downloading,
                    ItemState::Pending => QueueEntryStatus::Queued,
                    ItemState::Failed(_) => QueueEntryStatus::Failed,
                }
            } else {
                queue_count += 1;
                match &item.state {
                    ItemState::Ready => QueueEntryStatus::Queued,
                    ItemState::Pending if transferring => QueueEntryStatus::Downloading,
                    // Waiting its turn, not arriving: a spinner on every one
                    // of these read as the whole album downloading at once.
                    ItemState::Pending => QueueEntryStatus::Queued,
                    ItemState::Failed(_) => QueueEntryStatus::Failed,
                }
            };

            // Override duration from TrackInfo if we have it and this is playing.
            let duration_ms =
                if has_playing && status == QueueEntryStatus::Playing && item.duration_ms.is_none()
                {
                    playing_duration_ms
                } else {
                    item.duration_ms
                };

            entries.push(QueueEntry {
                id: item.id,
                db_id: item.db_id,
                playlist_entry_id: item.playlist_entry_id,
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
                error: match &item.state {
                    ItemState::Failed(reason) => Some(reason.clone()),
                    _ => None,
                },
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- helpers ---

    fn make_item(title: &str, state: ItemState) -> PlaylistItem {
        PlaylistItem {
            playlist_entry_id: None,
            id: QueueItemId::new(),
            db_id: None,
            path: PathBuf::from(format!("/music/{title}.flac")),
            title: title.to_string(),
            artist: "Artist".to_string(),
            album_artist: "Artist".to_string(),
            album: "Album".to_string(),
            year: None,
            codec: Some("FLAC".to_string()),
            track_number: None,
            disc: None,
            duration_ms: Some(200_000),
            state,
        }
    }

    /// An item with a transfer running against it, told to the state's store
    /// the way the downloader tells it. Returns the transfer's byte feed.
    fn downloading_item(
        state: &SharedPlayerState,
        title: &str,
        total: u64,
    ) -> (PlaylistItem, Arc<ByteFeed>) {
        static NEXT_TRACK: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
        let mut item = make_item(title, ItemState::Pending);
        item.db_id = Some(NEXT_TRACK.fetch_add(1, Ordering::Relaxed));
        let feed = start_transfer(state, &item, total);
        (item, feed)
    }

    /// Claim, announce and start the transfer for `item`'s track.
    fn start_transfer(state: &SharedPlayerState, item: &PlaylistItem, total: u64) -> Arc<ByteFeed> {
        let track_id = item.db_id.expect("a transfer is for a library track");
        let store = state.downloads();
        store.claim(track_id, Some(item.id));
        let feed = store.announce(
            track_id,
            item.title.clone(),
            String::new(),
            PathBuf::from(format!("/cache/{}.flac.part", item.title)),
            PathBuf::from(format!("/cache/{}.flac", item.title)),
        );
        store.started(track_id, total);
        feed
    }

    fn ready_item(title: &str) -> PlaylistItem {
        make_item(title, ItemState::Ready)
    }

    fn pending_item(title: &str) -> PlaylistItem {
        make_item(title, ItemState::Pending)
    }

    fn failed_item(title: &str) -> PlaylistItem {
        make_item(title, ItemState::Failed("nope".into()))
    }

    const DURATION_MS: u64 = 32_523_787;

    /// A nine-hour track under the cursor, `downloaded` bytes of `total` in.
    /// `total` of 0 stands for a server that sent no Content-Length.
    fn streaming_state(
        downloaded: u64,
        total: u64,
        bitrate_kbps: Option<u32>,
    ) -> Arc<SharedPlayerState> {
        streaming_state_with_duration(downloaded, total, bitrate_kbps, DURATION_MS)
    }

    /// The same, but saying what the container managed to state about itself.
    /// A partial Ogg states nothing, which is zero here.
    fn streaming_state_with_duration(
        downloaded: u64,
        total: u64,
        bitrate_kbps: Option<u32>,
        container_duration_ms: u64,
    ) -> Arc<SharedPlayerState> {
        let mut item = make_item("train", ItemState::Pending);
        item.db_id = Some(1);
        let id = item.id;
        let path = item.path.clone();

        let state = SharedPlayerState::new();
        start_transfer(&state, &item, total).set(downloaded);
        state.add_items(vec![item]);
        state.set_cursor(Some(id));
        state.set_track_info(Some(TrackInfo {
            id,
            path,
            codec: "Opus".into(),
            sample_rate: 48_000,
            bit_depth: None,
            bitrate_kbps,
            channels: 2,
            duration_ms: container_duration_ms,
        }));
        state
    }

    // --- seekable_ms ---

    #[test]
    fn a_track_on_disk_is_seekable_end_to_end() {
        let item = ready_item("done");
        let id = item.id;
        let path = item.path.clone();
        let state = SharedPlayerState::new();
        state.add_items(vec![item]);
        state.set_cursor(Some(id));
        state.set_track_info(Some(TrackInfo {
            id,
            path,
            codec: "FLAC".into(),
            sample_rate: 44_100,
            bit_depth: Some(16),
            bitrate_kbps: None,
            channels: 2,
            duration_ms: 200_000,
        }));

        assert_eq!(state.seekable_ms(), 200_000);
        // Nothing to draw a boundary for, so front ends are told there isn't one.
        assert_eq!(state.seek_ceiling_ms(), None);
    }

    #[test]
    fn a_downloading_track_is_seekable_as_far_as_its_bytes_reach() {
        // A quarter of a nine-hour file in: a quarter of the way through it,
        // less the margin the byte-to-time estimate is worth.
        let state = streaming_state(100, 400, None);
        assert_eq!(state.seekable_ms(), 32_523_787 / 4 - SEEK_SAFETY_MS);
        assert_eq!(
            state.seek_ceiling_ms(),
            Some(32_523_787 / 4 - SEEK_SAFETY_MS)
        );
    }

    #[test]
    fn a_transfer_without_a_content_length_falls_back_to_bitrate() {
        // No total to take a fraction of. 128 kbps is 128 bits per ms, so a
        // megabyte is 8 388 608 bits and a little over 65 seconds.
        let state = streaming_state(1024 * 1024, 0, Some(128));
        assert_eq!(state.seekable_ms(), 1024 * 1024 * 8 / 128 - SEEK_SAFETY_MS);
    }

    #[test]
    fn nothing_to_estimate_from_allows_no_forward_seek() {
        // Neither a length nor a bitrate: anywhere past the playhead is a
        // guess, and a guess that lands past the write head is a stall.
        let state = streaming_state(1024 * 1024, 0, None);
        state.set_position_ms(12_000);
        assert_eq!(state.seekable_ms(), 12_000);
    }

    #[test]
    fn the_seekable_extent_never_exceeds_the_track() {
        // A download reporting more bytes than it advertised must not offer a
        // seek past the end of the music.
        let state = streaming_state(500, 400, None);
        assert_eq!(state.seekable_ms(), 32_523_787);
    }

    #[test]
    fn nothing_playing_is_seekable_nowhere() {
        assert_eq!(SharedPlayerState::new().seekable_ms(), 0);
        assert_eq!(SharedPlayerState::new().seek_ceiling_ms(), None);
    }

    #[test]
    fn a_container_that_cannot_state_its_duration_cannot_be_seeked() {
        // A partial Ogg keeps its duration in a last page that has not arrived,
        // so it opens and plays but has nothing to seek against. Half the bytes
        // being present does not change that.
        let state = streaming_state_with_duration(200, 400, Some(128), 0);
        assert_eq!(state.seekable_ms(), 0);
    }

    #[test]
    fn the_library_duration_stands_in_for_a_silent_container() {
        // What is shown on the transport, so nine hours of music does not read
        // as 0:00 while it caches.
        let state = streaming_state_with_duration(200, 400, Some(128), 0);
        assert_eq!(state.duration_ms(), 200_000, "the item's own figure");
        // And it is a display figure only — it grants no seeking.
        assert_eq!(state.seekable_ms(), 0);
        assert_eq!(state.seek_ceiling_ms(), Some(0));
    }

    #[test]
    fn the_container_duration_wins_where_there_is_one() {
        let state = streaming_state(200, 400, None);
        assert_eq!(state.duration_ms(), DURATION_MS);
    }

    #[test]
    fn the_download_landing_restores_seeking() {
        // The sequence the whole design turns on: a track that opened without a
        // duration gets one when the finished file is re-read, and is seekable
        // end to end from that moment — no restart, no handover.
        let state = streaming_state_with_duration(400, 400, Some(128), 0);
        assert_eq!(state.seekable_ms(), 0);

        // What the downloader does when the bytes land: settle the transfer,
        // then say the file is playable. In that order — while the store still
        // says a transfer is running, it is.
        let id = state.cursor().expect("cursor");
        let _ =
            crate::remote::downloads::settle(&state, 1, &Ok(PathBuf::from("/cache/train.flac")));
        assert_eq!(state.item_state(id), Some(ItemState::Ready));
        let info = state.track_info().expect("track info");
        state.set_track_info(Some(TrackInfo {
            duration_ms: DURATION_MS,
            ..info
        }));

        assert_eq!(state.seekable_ms(), DURATION_MS);
        assert_eq!(state.seek_ceiling_ms(), None, "no boundary left to draw");
    }

    // --- advance_cursor_loadable ---

    #[test]
    fn advance_parks_on_a_still_downloading_track() {
        // A track that has not arrived yet must hold the cursor, not be skipped:
        // skipping it drops it from the queue for good, and it is the item whose
        // TrackReady has to resume playback.
        let state = SharedPlayerState::new();
        let item0 = ready_item("track-0");
        let item1 = pending_item("track-1");
        let item2 = ready_item("track-2");
        let (id0, id1) = (item0.id, item1.id);

        state.add_items(vec![item0, item1, item2]);

        assert_eq!(state.advance_cursor_loadable(), Some(id0));
        assert_eq!(state.advance_cursor_loadable(), Some(id1));
        assert_eq!(state.cursor(), Some(id1));
    }

    #[test]
    fn advance_skips_failed_items() {
        let state = SharedPlayerState::new();
        let item0 = ready_item("track-0");
        let item1 = failed_item("track-1");
        let item2 = ready_item("track-2");
        let (id0, id2) = (item0.id, item2.id);

        state.add_items(vec![item0, item1, item2]);
        state.set_cursor(Some(id0));

        assert_eq!(state.advance_cursor_loadable(), Some(id2));
    }

    #[test]
    fn advance_stops_at_end_of_playlist() {
        let state = SharedPlayerState::new();
        let item0 = ready_item("track-0");
        let item1 = ready_item("track-1");
        let id1 = item1.id;

        state.add_items(vec![item0, item1]);
        state.set_cursor(Some(id1));

        assert_eq!(state.advance_cursor_loadable(), None);
        assert_eq!(
            state.cursor(),
            Some(id1),
            "cursor unchanged on a failed advance"
        );
    }

    #[test]
    fn advance_with_only_failed_items_returns_none() {
        let state = SharedPlayerState::new();
        state.add_items(vec![failed_item("bad-0"), failed_item("bad-1")]);

        assert_eq!(state.advance_cursor_loadable(), None);
    }

    #[test]
    fn advance_from_a_vanished_cursor_does_not_restart_the_queue() {
        let state = SharedPlayerState::new();
        let item0 = ready_item("track-0");
        let item1 = ready_item("track-1");
        let id0 = item0.id;

        state.add_items(vec![item0, item1]);
        let ghost = QueueItemId::new();
        state.set_cursor(Some(ghost));

        assert_eq!(state.advance_cursor_loadable(), None);
        assert_ne!(state.cursor(), Some(id0));
    }

    // --- lookahead_after ---

    fn chosen_after(state: &SharedPlayerState, id: QueueItemId) -> Option<QueueItemId> {
        state
            .lookahead_after(id)
            .and_then(|step| step.chosen)
            .map(|(id, _)| id)
    }

    #[test]
    fn a_step_stops_at_a_track_still_arriving() {
        let state = SharedPlayerState::new();
        let a = ready_item("a");
        let b = PlaylistItem {
            state: ItemState::Pending,
            ..ready_item("b")
        };
        let c = ready_item("c");
        let (ida, idb) = (a.id, b.id);
        state.add_items(vec![a, b, c]);

        let step = state.lookahead_after(ida).unwrap();
        assert_eq!(step.next, Some(idb));
        assert!(step.chosen.is_none(), "c is not queued over it");

        state.update_item_state(idb, ItemState::Ready);
        assert!(
            state.still_follows(&step),
            "its landing is not an edit: the advance plays it"
        );

        state.add_items(vec![ready_item("d")]);
        assert!(state.still_follows(&step), "nor is adding after it");

        state.insert_items_after(vec![ready_item("next")], ida);
        assert!(!state.still_follows(&step), "a track put before it is");
    }

    #[test]
    fn a_step_passes_over_failed_tracks() {
        let state = SharedPlayerState::new();
        let a = ready_item("a");
        let failed = PlaylistItem {
            state: ItemState::Failed("gone".into()),
            ..ready_item("failed")
        };
        let c = ready_item("c");
        let (ida, idc) = (a.id, c.id);
        state.add_items(vec![a, failed, c]);

        let step = state.lookahead_after(ida).unwrap();
        assert_eq!(step.chosen.as_ref().map(|(id, _)| *id), Some(idc));

        let another = PlaylistItem {
            state: ItemState::Failed("gone".into()),
            ..ready_item("another")
        };
        state.insert_items_after(vec![another], ida);
        assert!(
            state.still_follows(&step),
            "another failed track before it changes nothing"
        );

        let arriving = PlaylistItem {
            state: ItemState::Pending,
            ..ready_item("arriving")
        };
        state.insert_items_after(vec![arriving], ida);
        assert!(!state.still_follows(&step), "a track still arriving does");
    }

    #[test]
    fn a_step_breaks_when_what_it_chose_moves_ahead_of_it() {
        let state = SharedPlayerState::new();
        let (a, c) = (ready_item("a"), ready_item("c"));
        let (ida, idc) = (a.id, c.id);
        state.add_items(vec![a, c]);
        let step = state.lookahead_after(ida).unwrap();
        state.move_item(idc, ida, false);
        assert!(!state.still_follows(&step));
    }

    #[test]
    fn a_step_to_the_end_of_the_queue_breaks_when_a_track_is_added() {
        let state = SharedPlayerState::new();
        let a = ready_item("a");
        let ida = a.id;
        state.add_items(vec![a]);
        let step = state.lookahead_after(ida).unwrap();
        assert!(step.chosen.is_none());
        assert!(state.still_follows(&step));
        state.add_items(vec![ready_item("b")]);
        assert!(!state.still_follows(&step));
    }

    #[test]
    fn peek_after_a_removed_item_returns_none() {
        // The decode thread's lookahead runs seconds ahead of what is audible.
        // Removing the track it is pre-decoding must end the lookahead, not send
        // it back to the top of the queue.
        let state = SharedPlayerState::new();
        let item0 = ready_item("track-0");
        let item1 = ready_item("track-1");
        let item2 = ready_item("track-2");
        let (id0, id1, id2) = (item0.id, item1.id, item2.id);

        state.add_items(vec![item0, item1, item2]);
        assert_eq!(chosen_after(&state, id1), Some(id2));

        state.remove_item(id2);
        assert!(
            state.lookahead_after(id2).is_none(),
            "a vanished reference must not resolve to the head of the queue"
        );
        assert_ne!(chosen_after(&state, id2), Some(id0));
    }

    #[test]
    fn the_lookahead_steps_over_a_track_that_failed() {
        let state = SharedPlayerState::new();
        let playing = ready_item("playing");
        let failed = failed_item("failed");
        let after = ready_item("after");
        let (playing_id, after_id) = (playing.id, after.id);
        state.add_items(vec![playing, failed, after]);

        assert_eq!(chosen_after(&state, playing_id), Some(after_id));
    }

    // --- surviving_item_before ---

    #[test]
    fn surviving_predecessor_skips_items_being_removed() {
        let state = SharedPlayerState::new();
        let items: Vec<_> = (0..4).map(|i| ready_item(&format!("track-{i}"))).collect();
        let ids: Vec<_> = items.iter().map(|i| i.id).collect();
        state.add_items(items);

        // Deleting 1..=3 leaves 0 as the resume point for a cursor on 3.
        assert_eq!(
            state.surviving_item_before(ids[3], &ids[1..4]),
            Some(ids[0])
        );
        // Deleting everything from the top leaves nothing to resume after.
        assert_eq!(state.surviving_item_before(ids[2], &ids), None);
    }

    // --- retreat_cursor ---

    #[test]
    fn test_retreat_cursor_goes_to_previous_item() {
        let state = SharedPlayerState::new();
        let item0 = ready_item("track-0");
        let item1 = ready_item("track-1");
        let id0 = item0.id;
        let id1 = item1.id;

        state.add_items(vec![item0, item1]);
        state.set_cursor(Some(id1));

        let result = state.retreat_cursor();
        assert!(result.is_some(), "expected to retreat to previous item");
        assert_eq!(result.unwrap().0, id0, "should retreat to first item");
        assert_eq!(state.cursor(), Some(id0));
    }

    #[test]
    fn test_retreat_cursor_returns_none_when_at_first_item() {
        let state = SharedPlayerState::new();
        let item0 = ready_item("only-track");
        let id0 = item0.id;

        state.add_items(vec![item0]);
        state.set_cursor(Some(id0));

        let result = state.retreat_cursor();
        assert!(result.is_none(), "cannot retreat before the first item");
        // Cursor stays on the first item.
        assert_eq!(state.cursor(), Some(id0));
    }

    #[test]
    fn test_retreat_cursor_returns_none_when_cursor_is_unset() {
        let state = SharedPlayerState::new();
        state.add_items(vec![ready_item("track-0")]);

        let result = state.retreat_cursor();
        assert!(
            result.is_none(),
            "retreat with no cursor should return None"
        );
    }

    // --- derive_visible_queue ---

    #[test]
    fn test_derive_visible_queue_statuses() {
        // playlist: [played, playing, queued]
        let state = SharedPlayerState::new();
        let item0 = ready_item("played-track");
        let item1 = ready_item("playing-track");
        let item2 = ready_item("queued-track");
        let id1 = item1.id;

        state.add_items(vec![item0, item1, item2]);
        state.set_cursor(Some(id1));

        let snap = state.derive_visible_queue();

        assert_eq!(snap.entries.len(), 3);
        assert_eq!(snap.entries[0].status, QueueEntryStatus::Played);
        assert_eq!(snap.entries[1].status, QueueEntryStatus::Playing);
        assert_eq!(snap.entries[2].status, QueueEntryStatus::Queued);
        assert!(snap.has_playing);
        assert_eq!(snap.finished_count, 1);
        assert_eq!(snap.queue_count, 1);
    }

    #[test]
    fn test_derive_visible_queue_downloading_statuses() {
        // Downloading reads the same at the cursor as after it: bytes are
        // moving, and the row has progress to draw.
        let state = SharedPlayerState::new();
        let (dl_cursor, _) = downloading_item(&state, "downloading-at-cursor", 1_000_000);
        let (dl_queued, _) = downloading_item(&state, "downloading-queued", 500_000);
        let id_cursor = dl_cursor.id;

        state.add_items(vec![dl_cursor, dl_queued]);
        state.set_cursor(Some(id_cursor));

        let snap = state.derive_visible_queue();

        assert_eq!(snap.entries[0].status, QueueEntryStatus::Downloading);
        assert_eq!(snap.entries[1].status, QueueEntryStatus::Downloading);
    }

    #[test]
    fn a_cursor_waiting_its_turn_is_priority_pending() {
        let state = SharedPlayerState::new();
        let item = pending_item("waiting");
        let id = item.id;
        state.add_items(vec![item]);
        state.set_cursor(Some(id));

        let snap = state.derive_visible_queue();
        assert_eq!(snap.entries[0].status, QueueEntryStatus::PriorityPending);
    }

    #[test]
    fn a_second_entry_for_a_track_reads_the_transfer_running_for_it() {
        // One transfer per track: the entry queued second has none of its own,
        // and streams and reports from the first one's.
        let state = SharedPlayerState::new();
        let (first, bytes) = downloading_item(&state, "twice", 1_000_000);
        bytes.set(STREAM_THRESHOLD);
        let mut again = pending_item("twice");
        again.db_id = first.db_id;
        let again_id = again.id;
        state.add_items(vec![first, again]);

        assert!(matches!(
            state.item_load_state(again_id),
            Some(LoadState::Downloading { .. })
        ));
        assert!(matches!(
            state.item_playback_source(again_id),
            Some(PlaybackSource::Streaming { .. })
        ));
    }

    /// The saved queue is rewritten when its contents move, and only then:
    /// a track change or a download landing is a position save.
    #[test]
    fn only_a_change_to_what_is_saved_moves_the_content_version() {
        let state = SharedPlayerState::new();
        let a = make_item("a", ItemState::Pending);
        let b = make_item("b", ItemState::Ready);
        let (a_id, b_id) = (a.id, b.id);

        let start = state.content_version();
        state.add_items(vec![a, b]);
        let added = state.content_version();
        assert_ne!(added, start);

        state.set_cursor(Some(a_id));
        state.update_item_state(a_id, ItemState::Ready);
        state.advance_cursor_loadable();
        state.retreat_cursor();
        assert_eq!(state.content_version(), added);
        assert_eq!(state.cursor_path(), Some(PathBuf::from("/music/a.flac")));

        state.move_item_to(b_id, None);
        assert_ne!(state.content_version(), added);
    }

    #[test]
    fn progress_follows_the_counter_without_touching_the_playlist() {
        // The download thread writes bytes and nothing else. A queue derived
        // afterwards must see them — the version has not moved, and the load
        // state it was given is the one it still holds.
        let state = SharedPlayerState::new();
        let (item, bytes) = downloading_item(&state, "downloading", 1_000);
        state.add_items(vec![item]);

        let version = state.playlist_version();
        bytes.set(250);

        let snap = state.derive_visible_queue();
        assert_eq!(snap.entries[0].download_progress, Some((250, 1_000)));
        assert_eq!(
            state.playlist_version(),
            version,
            "progress must not read as a queue mutation"
        );
        assert_eq!(state.downloads().readings()[0].written, 250);
    }

    #[test]
    fn test_derive_visible_queue_no_cursor_all_queued() {
        let state = SharedPlayerState::new();
        state.add_items(vec![ready_item("a"), ready_item("b"), ready_item("c")]);

        let snap = state.derive_visible_queue();

        assert_eq!(snap.entries.len(), 3);
        for entry in &snap.entries {
            assert_eq!(entry.status, QueueEntryStatus::Queued);
        }
        assert!(!snap.has_playing);
        assert_eq!(snap.finished_count, 0);
        assert_eq!(snap.queue_count, 3);
    }

    // --- same_album_item_ids ---

    fn make_album_item(title: &str, album: &str, album_artist: &str) -> PlaylistItem {
        PlaylistItem {
            playlist_entry_id: None,
            id: QueueItemId::new(),
            db_id: None,
            path: PathBuf::from(format!("/music/{title}.flac")),
            title: title.to_string(),
            artist: "Artist".to_string(),
            album_artist: album_artist.to_string(),
            album: album.to_string(),
            year: None,
            codec: Some("FLAC".to_string()),
            track_number: None,
            disc: None,
            duration_ms: Some(200_000),
            state: ItemState::Ready,
        }
    }

    #[test]
    fn test_same_album_item_ids_returns_album_mates() {
        let state = SharedPlayerState::new();
        let a1 = make_album_item("A1", "Album A", "Artist A");
        let a2 = make_album_item("A2", "Album A", "Artist A");
        let b1 = make_album_item("B1", "Album B", "Artist B");
        let a3 = make_album_item("A3", "Album A", "Artist A");

        let id_a1 = a1.id;
        let id_a2 = a2.id;
        let id_a3 = a3.id;

        state.add_items(vec![a1, a2, b1, a3]);

        let mates = state.same_album_item_ids(id_a1);
        assert_eq!(mates.len(), 2);
        assert!(mates.contains(&id_a2));
        assert!(mates.contains(&id_a3));
    }

    #[test]
    fn test_same_album_item_ids_distinguishes_album_artists() {
        // Two albums named the same but by different artists — should NOT match.
        let state = SharedPlayerState::new();
        let a1 = make_album_item("A1", "Greatest Hits", "Artist A");
        let b1 = make_album_item("B1", "Greatest Hits", "Artist B");

        let id_a1 = a1.id;

        state.add_items(vec![a1, b1]);

        let mates = state.same_album_item_ids(id_a1);
        assert!(mates.is_empty(), "different album_artist should not match");
    }

    #[test]
    fn test_same_album_item_ids_unknown_id_returns_empty() {
        let state = SharedPlayerState::new();
        state.add_items(vec![ready_item("track-0")]);

        let bogus = QueueItemId::new();
        let mates = state.same_album_item_ids(bogus);
        assert!(mates.is_empty());
    }

    // --- update_item_metadata ---

    #[test]
    fn test_update_item_metadata_leaves_library_tags_alone() {
        let state = SharedPlayerState::new();
        let mut item = make_album_item("A1", "Nite Versions (mixed)", "Soulwax");
        item.db_id = Some(29615);
        let id = item.id;
        state.add_items(vec![item]);

        state.update_item_metadata(
            id,
            "[unknown]".into(),
            "Soulwax".into(),
            "Soulwax".into(),
            "Nite Versions".into(),
            Some(54_000),
        );

        let pl = state.playlist.read();
        assert_eq!(pl.items[0].album, "Nite Versions (mixed)");
        assert_eq!(pl.items[0].title, "A1");
        assert_eq!(pl.items[0].duration_ms, Some(54_000));
    }

    #[test]
    fn test_update_item_metadata_fills_in_an_item_with_nothing_behind_it() {
        let state = SharedPlayerState::new();
        let item = make_album_item("A1", "", "");
        let id = item.id;
        state.add_items(vec![item]);

        state.update_item_metadata(
            id,
            "Teachers".into(),
            "Soulwax".into(),
            "Soulwax".into(),
            "Nite Versions".into(),
            Some(148_000),
        );

        let pl = state.playlist.read();
        assert_eq!(pl.items[0].title, "Teachers");
        assert_eq!(pl.items[0].album, "Nite Versions");
        assert_eq!(pl.items[0].duration_ms, Some(148_000));
    }

    // --- move_item_to ---

    #[test]
    fn test_move_item_to_reorders_playlist() {
        // Start: [A, B, C]. Move C to after A → [A, C, B].
        let state = SharedPlayerState::new();
        let item_a = ready_item("A");
        let item_b = ready_item("B");
        let item_c = ready_item("C");
        let id_a = item_a.id;
        let id_b = item_b.id;
        let id_c = item_c.id;

        state.add_items(vec![item_a, item_b, item_c]);
        state.move_item_to(id_c, Some(id_a));

        let (items, _) = state.snapshot_playlist();
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["A", "C", "B"]);
        assert_eq!(items[0].id, id_a);
        assert_eq!(items[1].id, id_c);
        assert_eq!(items[2].id, id_b);
    }

    #[test]
    fn test_move_item_to_front_when_after_is_none() {
        // Start: [A, B, C]. Move C to front (after=None) → [C, A, B].
        let state = SharedPlayerState::new();
        let item_a = ready_item("A");
        let item_b = ready_item("B");
        let item_c = ready_item("C");
        let id_c = item_c.id;

        state.add_items(vec![item_a, item_b, item_c]);
        state.move_item_to(id_c, None);

        let (items, _) = state.snapshot_playlist();
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["C", "A", "B"]);
    }

    // --- move_items (batch) ---

    #[test]
    fn test_move_items_batch_preserves_relative_order() {
        // Start: [A, B, C, D]. Move [A, C] after D → [B, D, A, C].
        let state = SharedPlayerState::new();
        let item_a = ready_item("A");
        let item_b = ready_item("B");
        let item_c = ready_item("C");
        let item_d = ready_item("D");
        let id_a = item_a.id;
        let id_b = item_b.id;
        let id_c = item_c.id;
        let id_d = item_d.id;

        state.add_items(vec![item_a, item_b, item_c, item_d]);
        state.move_items(&[id_a, id_c], id_d, true);

        let (items, _) = state.snapshot_playlist();
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["B", "D", "A", "C"]);
        assert_eq!(items[0].id, id_b);
        assert_eq!(items[1].id, id_d);
        assert_eq!(items[2].id, id_a);
        assert_eq!(items[3].id, id_c);
    }

    // --- pending_downloads ---

    #[test]
    fn test_pending_downloads_collects_pending_with_db_id() {
        let state = SharedPlayerState::new();
        let mut item_a = ready_item("local");
        item_a.db_id = None;

        let mut item_b = pending_item("remote-1");
        item_b.db_id = Some(10);
        let id_b = item_b.id;

        let mut item_c = ready_item("cached");
        item_c.db_id = Some(20);

        let mut item_d = pending_item("remote-2");
        item_d.db_id = Some(30);
        let id_d = item_d.id;

        // Pending without db_id — should NOT appear (no way to download).
        let item_e = pending_item("orphan");

        state.add_items(vec![item_a, item_b, item_c, item_d, item_e]);

        let pending = state.pending_downloads();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0], (10, id_b));
        assert_eq!(pending[1], (30, id_d));

        // From the cursor on, then from the top: playing the fourth, the
        // tracks before it come last.
        state.set_cursor(Some(id_d));
        assert_eq!(state.pending_downloads(), vec![(30, id_d), (10, id_b)]);
    }

    #[test]
    fn test_item_db_id_and_load_state() {
        let state = SharedPlayerState::new();
        let mut item = pending_item("track");
        item.db_id = Some(42);
        let id = item.id;
        state.add_items(vec![item]);

        assert_eq!(state.item_db_id(id), Some(42));
        assert!(matches!(
            state.item_load_state(id),
            Some(LoadState::Pending)
        ));

        state.update_item_state(id, ItemState::Ready);
        assert!(matches!(state.item_load_state(id), Some(LoadState::Ready)));
    }
}
