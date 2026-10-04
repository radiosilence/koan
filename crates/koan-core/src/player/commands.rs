use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender, bounded};

use super::state::{PlaylistItem, QueueItemId};

/// Commands from the UI layer to the audio engine.
#[derive(Debug)]
pub enum PlayerCommand {
    /// Set the cursor and start playback.
    Play(QueueItemId),
    /// Set the cursor and load `id` at `position_ms`, playing or paused.
    ///
    /// What a restored session does. Play, Seek and Pause in turn would start
    /// the track from the top and let a moment of it out before the pause.
    Cue {
        id: QueueItemId,
        position_ms: u64,
        play: bool,
    },
    Pause,
    /// Pause, and answer with the playhead once the output has gone silent:
    /// after the fade, where one runs. What a hand-off resumes from.
    PauseAndReport(Sender<u64>),
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
    /// Replace the playlist and open the track at `start`, as one operation:
    /// from `position_ms`, playing or paused.
    ///
    /// Doing this as ClearPlaylist + AddToPlaylist + Play sends three commands
    /// down a bounded channel, and the player acts on each as it arrives: the
    /// first track starts, then the cursor jumps, so clicking track nine of an
    /// album shows track one playing first. It is three undo entries for one
    /// user action. And between the clear and the add the playlist is empty,
    /// which tells the download queue that nothing is wanted.
    ///
    /// `start` past the end starts at the beginning. It opens at
    /// `position_ms`, playing or paused, as `Cue` does: a hand-off picks up
    /// where the source stopped without the top of the track being heard.
    ReplacePlaylist {
        items: Vec<PlaylistItem>,
        start: usize,
        position_ms: u64,
        play: bool,
    },
    /// Download complete — check if cursor is waiting on this item.
    TrackReady(QueueItemId),
    /// Enough data buffered for streaming playback — check if cursor is waiting.
    TrackStreamReady(QueueItemId),
    /// A partial file has been probed off-thread and can be started.
    ///
    /// Probing reads as much of the container as it takes to describe itself,
    /// which for Ogg means its last page — the whole remaining download. That
    /// cannot happen on this loop, so it happens on its own thread and arrives
    /// here as a command like anything else. Stale by the time it lands is the
    /// normal case, and simply ignored.
    StreamProbed {
        id: QueueItemId,
        info: Box<crate::audio::buffer::StreamInfo>,
        /// What the probe had to settle for. Decoding has to be opened the same
        /// way — given a length, a container that went looking for its tail
        /// once will do it again, on the decode thread, where the cost is
        /// silence instead of a busy player.
        mode: crate::audio::streaming::ProbeMode,
    },
    /// Download failed — a cursor parked on this item must stop waiting.
    ///
    /// Without it the player sits on a `Pending` item forever: `Ready` is the
    /// only thing it listens for, and a track that cannot be fetched never
    /// becomes Ready. That is the offline-library stall.
    TrackFailed(QueueItemId),
    /// Fetch these tracks into the cache, with no queue entry to play them.
    CacheTracks(Vec<i64>),
    /// Decode thread exhausted the playlist — auto-advance or stop. Carries
    /// the session it came from, so one sent just before a play or seek is
    /// recognised as stale.
    DecodeFinished(u64),
    /// The decoder queued the next track, so when the playhead reaches it is
    /// now known.
    TrackQueued,
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
    /// The DSP profiles, or the device they key on, changed: load them again
    /// and carry on where playback is.
    ReloadDsp,
    /// Build the output again on the same device and carry on from where the
    /// current track is, paused if it was. For an output the system stopped
    /// underneath us: an iOS interruption (a call, Siri) or a reset of its
    /// media services leaves the old unit unable to start again.
    RestartOutput,
    /// Play to this renderer from now on, carrying on from where the current
    /// track is; `None` brings the music back to this device's own output.
    /// The session is opened by the caller, off this thread: see
    /// `upnp::connect`.
    UseRenderer(Option<Box<crate::upnp::Connection>>),
    /// Set the volume of the renderer being played to, 0–100.
    SetRendererVolume(u8),
    /// What the renderer was heard to do, during the session numbered
    /// `session`. Dropped once that session is over, like `DecodeFinished`.
    Renderer {
        session: u64,
        event: crate::upnp::session::Event,
    },
}

/// Bounded command channel.
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
