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
    /// Download complete — check if cursor is waiting on this item.
    TrackReady(QueueItemId),
    /// Enough data buffered for streaming playback — check if cursor is waiting.
    TrackStreamReady(QueueItemId),
    /// Undo the last reversible playlist operation.
    Undo,
    /// Redo the last undone operation.
    Redo,
    /// Begin collecting undo entries into a single batch (e.g. drag operations).
    BeginUndoBatch,
    /// End the batch — collapse collected entries into one undo step.
    EndUndoBatch,
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
