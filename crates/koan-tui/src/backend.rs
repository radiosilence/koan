//! Player backend abstraction — local (in-process) or remote (GQL).
//!
//! The TUI uses `dyn PlayerBackend` for commands, state reads, and viz data.
//! Hot-path reads (position, playback state, viz) go through SharedPlayerState/VizSnapshot
//! for sub-microsecond access. Commands go through send_command().

use std::sync::Arc;

use crossbeam_channel::Sender;
use koan_core::audio::viz::VizSnapshot;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::SharedPlayerState;

pub trait PlayerBackend: Send + Sync {
    /// Send a command to the player (local channel or GQL mutation).
    fn send_command(&self, cmd: PlayerCommand);

    /// Access the shared player state for hot-path reads (atomics, RwLock).
    fn shared_state(&self) -> &Arc<SharedPlayerState>;

    /// Access the viz snapshot for hot-path reads (spectrum, peaks, VU).
    fn viz_snapshot(&self) -> &Arc<VizSnapshot>;
}

/// Local backend — direct in-process player. Commands go through crossbeam channel.
/// State reads are direct atomic/RwLock access. This is the default mode.
pub struct LocalBackend {
    state: Arc<SharedPlayerState>,
    viz: Arc<VizSnapshot>,
    tx: Sender<PlayerCommand>,
}

impl LocalBackend {
    pub fn new(
        state: Arc<SharedPlayerState>,
        viz: Arc<VizSnapshot>,
        tx: Sender<PlayerCommand>,
    ) -> Self {
        Self { state, viz, tx }
    }

    /// Get a clone of the command sender (needed for DownloadQueue, media keys, etc.)
    pub fn cmd_tx(&self) -> Sender<PlayerCommand> {
        self.tx.clone()
    }
}

impl PlayerBackend for LocalBackend {
    fn send_command(&self, cmd: PlayerCommand) {
        if let Err(e) = self.tx.send(cmd) {
            log::error!("failed to send player command: {}", e);
        }
    }

    fn shared_state(&self) -> &Arc<SharedPlayerState> {
        &self.state
    }

    fn viz_snapshot(&self) -> &Arc<VizSnapshot> {
        &self.viz
    }
}
