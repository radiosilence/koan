use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender, bounded};

use super::state::{PlaylistItem, QueueItemId};

/// Commands from the UI layer to the audio engine.
#[derive(Debug)]
pub enum PlayerCommand {
    /// Set cursor + start playback. Replaces Play/SkipTo/SkipBack/PlayInterrupt.
    Play(QueueItemId),
    Pause,
    Resume,
    Stop,
    Seek(u64), // position in ms
    NextTrack,
    PrevTrack,
    AddToPlaylist(Vec<PlaylistItem>),
    RemoveFromPlaylist(QueueItemId),
    /// Batch remove: delete multiple items as a single undoable operation.
    RemoveFromPlaylistBatch(Vec<QueueItemId>),
    MoveInPlaylist {
        id: QueueItemId,
        target: QueueItemId,
        after: bool,
    },
    /// Batch move: extract `ids` and reinsert them at `target` position.
    MoveItemsInPlaylist {
        ids: Vec<QueueItemId>,
        target: QueueItemId,
        after: bool,
    },
    /// Put the queue in exactly this order, keeping every item.
    ///
    /// A queue locked to a playlist follows it, and following a reorder means
    /// moving the items that are already there — rebuilding them would issue
    /// new ids and throw away what has played, what is mid-download and where
    /// the cursor is. Ids not named keep their relative order at the end.
    ReorderPlaylist(Vec<QueueItemId>),
    /// Update file paths for playlist items after an organize operation.
    /// On Unix, rename() doesn't invalidate open FDs so playback continues.
    UpdatePaths(Vec<(QueueItemId, PathBuf)>),
    /// Insert items after a specific queue item (for drag/drop at cursor position).
    InsertInPlaylist {
        items: Vec<PlaylistItem>,
        after: QueueItemId,
    },
    /// Clear the entire playlist (stop + remove all items).
    ClearPlaylist,
    /// Replace the playlist and start playing at `start`, as one operation.
    ///
    /// Doing this as ClearPlaylist + AddToPlaylist + Play sends three commands
    /// down a bounded channel, and the player acts on each as it arrives: the
    /// first track starts, then the cursor jumps. Clicking track nine of an
    /// album visibly flashed track one as playing first. It is also three undo
    /// entries for one user action.
    ///
    /// `start` past the end starts at the beginning.
    ReplacePlaylist {
        items: Vec<PlaylistItem>,
        start: usize,
    },
    /// Download complete — check if cursor is waiting on this item.
    TrackReady(QueueItemId),
    /// Enough data buffered for streaming playback — check if cursor is waiting.
    TrackStreamReady(QueueItemId),
    /// Download failed — a cursor parked on this item must stop waiting.
    ///
    /// Without it the player sits on a `Pending` item forever: `Ready` is the
    /// only thing it listens for, and a track that cannot be fetched never
    /// becomes Ready. That is the offline-library stall.
    TrackFailed(QueueItemId),
    /// Decode thread exhausted the playlist — auto-advance or stop.
    DecodeFinished,
    /// Undo the last reversible playlist operation.
    Undo,
    /// Redo the last undone operation.
    Redo,
    /// Begin collecting undo entries into a single batch (e.g. drag operations).
    BeginUndoBatch,
    /// End the batch — collapse collected entries into one undo step.
    EndUndoBatch,
    /// Switch output audio device by name. Restarts engine on current track.
    SetOutputDevice(String),
    /// Clear the configured output device, reverting to system default.
    ClearOutputDevice,
}

/// Bounded SPSC command channel.
///
/// Small capacity — we don't want commands queuing up. If the engine is busy,
/// the UI should know about it, not silently buffer 50 seeks.
pub struct CommandChannel {
    pub tx: Sender<PlayerCommand>,
    pub rx: Receiver<PlayerCommand>,
}

impl Default for CommandChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandChannel {
    pub fn new() -> Self {
        let (tx, rx) = bounded(16);
        Self { tx, rx }
    }
}
