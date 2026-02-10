use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender, bounded};

/// Commands from the UI/FFI layer to the audio engine.
#[derive(Debug)]
pub enum PlayerCommand {
    Play(PathBuf),
    /// Queue tracks for gapless playback. Replaces the current queue.
    PlayQueue(Vec<PathBuf>),
    /// Append a track to the end of the queue without interrupting playback.
    Enqueue(PathBuf),
    Pause,
    Resume,
    Stop,
    Seek(u64), // position in ms
    NextTrack, // skip to next in queue
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
