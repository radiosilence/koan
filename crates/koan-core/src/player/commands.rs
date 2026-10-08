use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, SendTimeoutError, Sender, bounded};

use super::state::{PlayMode, PlaylistItem, QueueItemId, QueueMode, Repeat, SleepTimer};

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
    /// Answer once every command sent before this one has been applied and
    /// published: the queue, the cursor and the playback state all as they
    /// left them. For a caller that must look at what it asked for, not at
    /// what was there before.
    Barrier(Sender<()>),
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
    ///
    /// `mode` says whether the play mode is reset, as a play from a play
    /// button resets it. Shuffled, the track opened is the play order's
    /// first, not `start`.
    ReplacePlaylist {
        items: Vec<PlaylistItem>,
        start: usize,
        position_ms: u64,
        play: bool,
        mode: QueueMode,
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
    /// The iOS audio route changed. Its rate is read again, and the output's
    /// rate asked for on it, then the DSP profiles reloaded for it.
    RouteChanged,
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
    /// The renderer last used, found on the network after launch. Taken as
    /// `UseRenderer` would be, unless playback or the output has moved since
    /// launch, in which case it is dropped: see `upnp::resume`.
    ResumeRenderer(Box<crate::upnp::Connection>),
    /// The renderer used last time did not turn up, or is someone else's:
    /// play here what was held for it.
    ResumeRendererMissed,
    /// The app is quitting. Stop the renderer, which would otherwise play out
    /// its buffer and be left on a URL nothing serves, then answer. Nothing
    /// else changes: the session as saved is what the next launch resumes.
    ReleaseRenderer(crossbeam_channel::Sender<()>),
    /// Set the volume of the renderer being played to, 0–100.
    SetRendererVolume(u8),
    /// Turn shuffle on or off. The queue never moves: shuffle chooses which
    /// of the items yet to play this pass plays next.
    SetShuffle(bool),
    /// What follows a track at its end: the queue's next, the first again
    /// after the last, or the same item.
    SetRepeat(Repeat),
    /// Set the sleep timer, or with `None` cancel it.
    SetSleepTimer(Option<SleepTimer>),
    /// Take the mode a saved session had. Sent before its queue, so a
    /// shuffled one is played in a play order drawn as the queue arrives.
    RestorePlayMode(PlayMode),
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

/// Sends `command` from a thread the player may be about to join.
///
/// A plain send blocks while the channel is full, and the player, which is
/// the channel's only reader, does not read while it joins: the two would wait
/// on each other for good. This waits for room only until `stop` is set, which
/// the player does before joining; a command given up then belonged to a
/// session the player has already ended.
pub fn send_unless_stopped(tx: &Sender<PlayerCommand>, command: PlayerCommand, stop: &AtomicBool) {
    let mut command = command;
    while !stop.load(Ordering::Relaxed) {
        match tx.send_timeout(command, Duration::from_millis(10)) {
            Err(SendTimeoutError::Timeout(unsent)) => command = unsent,
            Ok(()) | Err(SendTimeoutError::Disconnected(_)) => return,
        }
    }
}

impl PlayerCommand {
    /// Whether it asks for something to be heard.
    pub fn asks_to_play(&self) -> bool {
        matches!(
            self,
            Self::Play(_)
                | Self::Cue { play: true, .. }
                | Self::Resume
                | Self::NextTrack
                | Self::PrevTrack
                | Self::ReplacePlaylist { play: true, .. }
        )
    }
}

/// Stop the renderer playing, if one is, before the app quits, waiting at
/// most `timeout` for it to answer. Call it after the session is saved.
pub fn release_renderer(
    player: &crossbeam_channel::Sender<PlayerCommand>,
    timeout: std::time::Duration,
) {
    let (tx, rx) = crossbeam_channel::bounded(1);
    if player.send(PlayerCommand::ReleaseRenderer(tx)).is_ok() {
        let _ = rx.recv_timeout(timeout);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::audio::buffer::{self, PlaybackTimeline, Processing, SourceEntry};

    #[test]
    fn a_full_channel_never_holds_up_stopping_the_decoder() {
        let channel = CommandChannel::new();
        while channel.tx.try_send(PlayerCommand::Stop).is_ok() {}
        let tx = channel.tx.clone();
        let (producer, _consumer) = rtrb::RingBuffer::new(1024);
        // An unreadable file ends the session at once, so the decode thread
        // reaches the full channel straight away.
        let mut decode = buffer::start_decode(
            SourceEntry::from_file(QueueItemId::new(), "/nonexistent/koan.flac".into()),
            producer,
            0,
            || None,
            PlaybackTimeline::new(),
            None,
            Processing::default(),
            move |stop| send_unless_stopped(&tx, PlayerCommand::DecodeFinished(1), stop),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));

        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            decode.stop();
            done_tx.send(()).ok();
        });
        let started = Instant::now();
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("stopping the decoder waited on the full channel");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(channel.rx.len(), 16);
    }

    #[test]
    fn a_natural_end_waits_for_room() {
        let channel = CommandChannel::new();
        while channel.tx.try_send(PlayerCommand::Stop).is_ok() {}
        let stop = Arc::new(AtomicBool::new(false));
        let tx = channel.tx.clone();
        let flag = stop.clone();
        let sender = std::thread::spawn(move || {
            send_unless_stopped(&tx, PlayerCommand::DecodeFinished(7), &flag)
        });
        std::thread::sleep(Duration::from_millis(50));
        for _ in 0..16 {
            channel.rx.recv().unwrap();
        }
        sender.join().unwrap();
        assert!(matches!(
            channel.rx.try_recv(),
            Ok(PlayerCommand::DecodeFinished(7))
        ));
    }
}
