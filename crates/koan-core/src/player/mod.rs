pub mod commands;
pub mod queue;
pub mod state;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use thiserror::Error;

use crate::audio::{buffer, device, engine};
use buffer::DecodeCallbacks;
use commands::{CommandChannel, PlayerCommand};
use queue::TrackQueue;
use state::{PlaybackState, SharedPlayerState, TrackInfo};

/// Ring buffer size in samples. ~1s at 192kHz stereo.
const RING_BUFFER_SIZE: usize = 192_000 * 2;

#[derive(Debug, Error)]
pub enum PlayerError {
    #[error("device error: {0}")]
    Device(#[from] device::DeviceError),
    #[error("decode error: {0}")]
    Decode(#[from] buffer::DecodeError),
    #[error("engine error: {0}")]
    Engine(#[from] engine::EngineError),
}

/// The player controller. Owns the audio pipeline and processes commands.
pub struct Player {
    shared_state: Arc<SharedPlayerState>,
    commands: CommandChannel,
    queue: Arc<TrackQueue>,
    /// Tracks we've already played — for going back with PrevTrack.
    history: VecDeque<PathBuf>,
    active_playback: Option<ActivePlayback>,
}

/// Holds the resources for an active playback session.
struct ActivePlayback {
    engine: engine::AudioEngine,
    decode_handle: buffer::DecodeHandle,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Self {
        Self {
            shared_state: SharedPlayerState::new(),
            commands: CommandChannel::new(),
            queue: Arc::new(TrackQueue::new()),
            history: VecDeque::new(),
            active_playback: None,
        }
    }

    /// Get a clone of the shared state for UI/FFI reads.
    pub fn shared_state(&self) -> Arc<SharedPlayerState> {
        self.shared_state.clone()
    }

    /// Get a command sender for the UI/FFI layer.
    pub fn command_sender(&self) -> crossbeam_channel::Sender<PlayerCommand> {
        self.commands.tx.clone()
    }

    /// Start playing a queue of files with gapless transitions.
    /// The first file starts immediately; the rest are queued.
    pub fn play_queue(&mut self, paths: &[impl AsRef<Path>]) -> Result<(), PlayerError> {
        if paths.is_empty() {
            return Ok(());
        }

        // Set up the queue with tracks 2..N.
        self.queue.clear();
        for path in paths.iter().skip(1) {
            self.queue.push_back(path.as_ref().to_path_buf());
        }

        // Start playing the first track — the decode thread will handle the rest.
        self.start_playback(paths[0].as_ref(), 0)?;
        Ok(())
    }

    /// Play a single file, clearing the queue.
    pub fn play(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.queue.clear();
        self.start_playback(path, 0)
    }

    /// Play a file, optionally seeking to a position.
    pub fn play_at(&mut self, path: &Path, seek_ms: u64) -> Result<(), PlayerError> {
        self.queue.clear();
        self.start_playback(path, seek_ms)
    }

    /// Internal: start playback of a file, keeping current queue intact.
    fn start_playback(&mut self, path: &Path, seek_ms: u64) -> Result<(), PlayerError> {
        self.stop_playback_no_clear();

        let info = buffer::probe_file(path)?;
        log::info!(
            "playing: {} — {} {}Hz/{}bit/{}ch, {}ms{}",
            path.display(),
            info.codec,
            info.sample_rate,
            info.bit_depth,
            info.channels,
            info.duration_ms,
            if seek_ms > 0 {
                format!(" @{}ms", seek_ms)
            } else {
                String::new()
            }
        );

        let device_id = device::default_output_device()?;
        let device_rate = device::get_device_sample_rate(device_id)?;
        let source_rate = info.sample_rate as f64;

        if (device_rate - source_rate).abs() > 0.1 {
            log::info!(
                "switching device sample rate: {}Hz → {}Hz",
                device_rate,
                source_rate
            );
            if let Err(e) = device::set_device_sample_rate(device_id, source_rate) {
                log::warn!(
                    "failed to set sample rate (continuing at device rate): {}",
                    e
                );
            }
        }

        let (producer, consumer) = rtrb::RingBuffer::new(RING_BUFFER_SIZE);

        // Callbacks for the decode thread.
        let pos_state = self.shared_state.clone();
        let track_state = self.shared_state.clone();

        let callbacks = DecodeCallbacks {
            on_position: move |pos_ms| {
                pos_state.set_position_ms(pos_ms);
            },
            on_track_change: move |path, stream_info| {
                log::info!("now playing: {}", path.display());
                track_state.set_track_info(Some(TrackInfo {
                    path,
                    codec: stream_info.codec,
                    sample_rate: stream_info.sample_rate,
                    bit_depth: stream_info.bit_depth,
                    channels: stream_info.channels,
                    duration_ms: stream_info.duration_ms,
                }));
                track_state.set_position_ms(0);
            },
        };

        let (_stream_info, decode_handle) =
            buffer::start_decode(path, producer, seek_ms, self.queue.clone(), callbacks)?;

        // Create and start audio engine.
        let actual_rate = device::get_device_sample_rate(device_id).unwrap_or(source_rate);
        let engine =
            engine::AudioEngine::new(device_id, actual_rate, info.channels as u32, consumer)?;
        engine.start()?;

        self.shared_state.set_playback_state(PlaybackState::Playing);

        self.active_playback = Some(ActivePlayback {
            engine,
            decode_handle,
        });

        Ok(())
    }

    /// Seek within the current track. If past the end, skip to next track.
    pub fn seek(&mut self, position_ms: u64) {
        let info = match self.shared_state.track_info() {
            Some(info) => info,
            None => return,
        };
        let path = info.path.clone();
        let duration = info.duration_ms;

        // Past the end → next track.
        if duration > 0 && position_ms >= duration {
            self.next_track();
            return;
        }

        // Don't clear the queue on seek — just rebuild decode for this track.
        if let Err(e) = self.start_playback(&path, position_ms) {
            log::error!("seek failed: {}", e);
        }
    }

    /// Skip to next track in queue. Pushes current track onto history.
    pub fn next_track(&mut self) {
        // Save current track to history so we can go back.
        if let Some(info) = self.shared_state.track_info() {
            self.history.push_back(info.path);
        }

        let next = self.queue.pop_front();
        match next {
            Some(path) => {
                if let Err(e) = self.start_playback(&path, 0) {
                    log::error!("next track failed: {}", e);
                }
            }
            None => {
                log::info!("no more tracks in queue");
                self.stop_playback_no_clear();
            }
        }
    }

    /// Go back to previous track. Pushes current track back onto queue front.
    pub fn prev_track(&mut self) {
        let prev = match self.history.pop_back() {
            Some(p) => p,
            None => {
                // No history — restart current track from the beginning.
                if let Some(info) = self.shared_state.track_info()
                    && let Err(e) = self.start_playback(&info.path, 0)
                {
                    log::error!("restart failed: {}", e);
                }
                return;
            }
        };

        // Push current track back to front of queue.
        if let Some(info) = self.shared_state.track_info() {
            self.queue.push_front(info.path);
        }

        if let Err(e) = self.start_playback(&prev, 0) {
            log::error!("prev track failed: {}", e);
        }
    }

    /// Pause playback.
    pub fn pause(&mut self) {
        if let Some(ref playback) = self.active_playback {
            let _ = playback.engine.stop();
            self.shared_state.set_playback_state(PlaybackState::Paused);
        }
    }

    /// Resume playback.
    pub fn resume(&mut self) {
        if let Some(ref playback) = self.active_playback {
            let _ = playback.engine.start();
            self.shared_state.set_playback_state(PlaybackState::Playing);
        }
    }

    /// Stop playback and clear queue.
    pub fn stop(&mut self) {
        self.queue.clear();
        self.stop_playback_no_clear();
    }

    /// Stop playback without clearing the queue.
    fn stop_playback_no_clear(&mut self) {
        if let Some(mut playback) = self.active_playback.take() {
            let _ = playback.engine.stop();
            playback.decode_handle.stop();
        }
        self.shared_state.set_playback_state(PlaybackState::Stopped);
        self.shared_state.set_position_ms(0);
        self.shared_state.set_track_info(None);
    }

    /// Append a track to the queue without interrupting playback.
    pub fn enqueue(&mut self, path: &Path) {
        self.queue.push_back(path.to_path_buf());
    }

    /// Process a single command.
    pub fn process_command(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::Play(path) => {
                if let Err(e) = self.play(&path) {
                    log::error!("play failed: {}", e);
                }
            }
            PlayerCommand::PlayQueue(paths) => {
                if let Err(e) = self.play_queue(&paths) {
                    log::error!("play queue failed: {}", e);
                }
            }
            PlayerCommand::Enqueue(path) => self.enqueue(&path),
            PlayerCommand::Pause => self.pause(),
            PlayerCommand::Resume => self.resume(),
            PlayerCommand::Stop => self.stop(),
            PlayerCommand::Seek(pos) => self.seek(pos),
            PlayerCommand::NextTrack => self.next_track(),
            PlayerCommand::PrevTrack => self.prev_track(),
        }
    }

    /// Run the command loop. Blocks until the sender is dropped.
    pub fn run(&mut self) {
        let rx = self.commands.rx.clone();
        while let Ok(cmd) = rx.recv() {
            self.process_command(cmd);
        }
        self.stop();
    }

    /// Spawn the player on a background thread, returning the shared state and command sender.
    pub fn spawn() -> (
        Arc<SharedPlayerState>,
        crossbeam_channel::Sender<PlayerCommand>,
    ) {
        let mut player = Self::new();
        let state = player.shared_state();
        let tx = player.command_sender();

        thread::Builder::new()
            .name("koan-player".into())
            .spawn(move || player.run())
            .expect("failed to spawn player thread");

        (state, tx)
    }
}
