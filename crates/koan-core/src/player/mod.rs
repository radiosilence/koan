pub mod commands;
pub mod queue;
pub mod state;

use std::path::Path;
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

        self.queue.clear();
        self.shared_state.clear_finished();
        for path in paths.iter().skip(1) {
            self.queue.push_back(path.as_ref().to_path_buf());
        }

        self.start_playback(paths[0].as_ref(), 0)?;
        self.rebuild_queue_snapshot();
        Ok(())
    }

    /// Play a single file, clearing the queue.
    pub fn play(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.queue.clear();
        self.shared_state.clear_finished();
        self.start_playback(path, 0)?;
        self.rebuild_queue_snapshot();
        Ok(())
    }

    /// Play a file, optionally seeking to a position.
    pub fn play_at(&mut self, path: &Path, seek_ms: u64) -> Result<(), PlayerError> {
        self.queue.clear();
        self.shared_state.clear_finished();
        self.start_playback(path, seek_ms)?;
        self.rebuild_queue_snapshot();
        Ok(())
    }

    /// Internal: start playback of a file, keeping current queue intact.
    fn start_playback(&mut self, path: &Path, seek_ms: u64) -> Result<(), PlayerError> {
        // Stop the running engine/decode thread but DON'T clear track_info.
        // This prevents a visible gap where track_info is None (causing UI flicker).
        self.stop_engine();

        let info = buffer::probe_file(path)?;

        // Set track_info immediately so the UI never sees a "no playing track" state.
        self.shared_state.set_track_info(Some(TrackInfo {
            path: path.to_path_buf(),
            codec: info.codec.clone(),
            sample_rate: info.sample_rate,
            bit_depth: info.bit_depth,
            channels: info.channels,
            duration_ms: info.duration_ms,
        }));
        self.shared_state.set_position_ms(0);
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

        // Bump generation so stale callbacks from the previous decode thread
        // know they're outdated and skip state mutations.
        let generation = self.shared_state.bump_generation();

        // Callbacks for the decode thread.
        let pos_state = self.shared_state.clone();
        let pos_gen = generation;
        let track_state = self.shared_state.clone();
        let cb_queue = self.queue.clone();

        let callbacks = DecodeCallbacks {
            on_position: move |pos_ms| {
                // Only update position if this callback is from the current generation.
                if pos_state.generation() == pos_gen {
                    pos_state.set_position_ms(pos_ms);
                }
            },
            on_track_change: move |path, stream_info| {
                // Stale callback from a previous start_playback — another track is
                // already playing. Ignore to avoid corrupting state.
                if track_state.generation() != generation {
                    log::info!(
                        "ignoring stale on_track_change for {} (generation {} != {})",
                        path.display(),
                        generation,
                        track_state.generation()
                    );
                    return;
                }
                // Push previous track to finished — but only if it differs from the
                // incoming track. start_playback pre-sets track_info to the new track,
                // so for player-initiated transitions old == new (skip push).
                // For gapless auto-advance old != new (push the finished track).
                if let Some(old_info) = track_state.track_info()
                    && old_info.path != path
                {
                    track_state.push_finished(old_info.path);
                }
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
                // Rebuild queue snapshot from actual queue state (decode thread
                // pops tracks, so the old snapshot goes stale on gapless advance).
                let paths = cb_queue.snapshot();
                track_state.rebuild_queue_snapshot_from_paths(paths);
                track_state.rebuild_visible_queue();
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

    /// Skip to next track in queue. Pushes current track to finished.
    pub fn next_track(&mut self) {
        if let Some(info) = self.shared_state.track_info() {
            self.shared_state.push_finished(info.path);
        }

        // Rebuild snapshot BEFORE pop so the UI sees the current track in
        // finished while the next track is still in the queue snapshot.
        self.rebuild_queue_snapshot();

        let next = self.queue.pop_front();
        match next {
            Some(path) => {
                if let Err(e) = self.start_playback(&path, 0) {
                    log::error!("next track failed: {}", e);
                }
            }
            None => {
                log::info!("no more tracks in queue");
                self.stop_playback_and_clear_state();
            }
        }
        // Rebuild again to reflect the popped track now playing.
        self.rebuild_queue_snapshot();
    }

    /// Go back to previous track. Pops from finished, pushes current back to queue.
    pub fn prev_track(&mut self) {
        let prev = match self.shared_state.pop_finished() {
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
        // Single rebuild AFTER all mutations — no intermediate snapshot that
        // shows a shorter list (which caused the 1-frame jank).
        self.rebuild_queue_snapshot();
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
        self.shared_state.clear_finished();
        self.stop_playback_and_clear_state();
        self.rebuild_queue_snapshot();
    }

    /// Stop the audio engine and decode thread without touching shared state.
    /// Used by start_playback to tear down the old pipeline before starting a new one.
    fn stop_engine(&mut self) {
        if let Some(mut playback) = self.active_playback.take() {
            let _ = playback.engine.stop();
            playback.decode_handle.stop();
        }
    }

    /// Full stop: tear down engine + clear all display state.
    fn stop_playback_and_clear_state(&mut self) {
        self.stop_engine();
        self.shared_state.set_playback_state(PlaybackState::Stopped);
        self.shared_state.set_position_ms(0);
        self.shared_state.set_track_info(None);
    }

    /// Interrupt: play this track immediately, pushing current to finished.
    /// Does NOT clear the queue or history — just cuts in.
    pub fn play_interrupt(&mut self, path: &Path) {
        log::info!("play_interrupt: starting {}", path.display());
        if let Some(info) = self.shared_state.track_info() {
            log::info!("play_interrupt: pushing current track to finished: {}", info.path.display());
            self.shared_state.push_finished(info.path);
        }
        if let Err(e) = self.start_playback(path, 0) {
            log::error!("play_interrupt failed: {}", e);
        }
        self.rebuild_queue_snapshot();
    }

    /// Append a track to the queue without interrupting playback.
    pub fn enqueue(&mut self, path: &Path) {
        self.queue.push_back(path.to_path_buf());
        self.rebuild_queue_snapshot();
    }

    /// Remove a track from the queue by index.
    pub fn remove_from_queue(&mut self, index: usize) {
        self.queue.remove(index);
        self.rebuild_queue_snapshot();
    }

    /// Move a track within the queue.
    pub fn move_in_queue(&mut self, from: usize, to: usize) {
        self.queue.move_track(from, to);
        self.rebuild_queue_snapshot();
    }

    /// Skip to a specific position in the queue.
    /// Pushes current track and all skipped tracks to finished atomically.
    pub fn skip_to(&mut self, index: usize) {
        // Collect all paths to push to finished BEFORE mutating queue state.
        let mut to_finish = Vec::with_capacity(index + 1);
        if let Some(info) = self.shared_state.track_info() {
            to_finish.push(info.path);
        }
        for _ in 0..index {
            if let Some(path) = self.queue.pop_front() {
                to_finish.push(path);
            }
        }

        // Commit finished paths and rebuild snapshot BEFORE popping the target.
        // The target is still in the queue snapshot at this point, so the UI
        // sees: finished tracks moved to history, target still queued. No gap.
        self.shared_state.extend_finished(to_finish);
        self.rebuild_queue_snapshot();

        // Now pop target and start playback.
        match self.queue.pop_front() {
            Some(path) => {
                if let Err(e) = self.start_playback(&path, 0) {
                    log::error!("skip_to failed: {}", e);
                }
            }
            None => {
                log::info!("skip_to: index out of range");
                self.stop_playback_and_clear_state();
            }
        }

        // Rebuild again: target is now playing (track_info set), removed from queue.
        self.rebuild_queue_snapshot();
    }

    /// Skip back to a finished track by index.
    /// Drains finished from that index, pushes current + remaining back to queue.
    pub fn skip_back(&mut self, finished_index: usize) {
        let drained = self.shared_state.drain_finished_from(finished_index);
        if drained.is_empty() {
            return;
        }

        let target = drained[0].clone();

        // Push current playing back to front of queue.
        if let Some(info) = self.shared_state.track_info() {
            self.queue.push_front(info.path);
        }

        // Push remaining drained tracks (after target) to front in reverse to maintain order.
        for path in drained[1..].iter().rev() {
            self.queue.push_front(path.clone());
        }

        if let Err(e) = self.start_playback(&target, 0) {
            log::error!("skip_back failed: {}", e);
        }
        // Single rebuild AFTER all mutations — no intermediate snapshot.
        self.rebuild_queue_snapshot();
    }

    /// Rebuild the shadow queue snapshot + visible queue for the UI.
    fn rebuild_queue_snapshot(&self) {
        let paths = self.queue.snapshot();
        self.shared_state.rebuild_queue_snapshot_from_paths(paths);
        self.shared_state.rebuild_visible_queue();
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
            PlayerCommand::PlayInterrupt(path) => self.play_interrupt(&path),
            PlayerCommand::Pause => self.pause(),
            PlayerCommand::Resume => self.resume(),
            PlayerCommand::Stop => self.stop(),
            PlayerCommand::Seek(pos) => self.seek(pos),
            PlayerCommand::NextTrack => self.next_track(),
            PlayerCommand::PrevTrack => self.prev_track(),
            PlayerCommand::RemoveFromQueue(index) => self.remove_from_queue(index),
            PlayerCommand::MoveInQueue { from, to } => self.move_in_queue(from, to),
            PlayerCommand::SkipTo(index) => self.skip_to(index),
            PlayerCommand::SkipBack(index) => self.skip_back(index),
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
