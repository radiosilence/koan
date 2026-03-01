pub mod commands;
pub mod state;

use std::path::Path;
use std::sync::Arc;
use std::thread;

use thiserror::Error;

use crate::audio::{buffer, device, engine};
use buffer::PlaybackTimeline;
use commands::{CommandChannel, PlayerCommand};
use state::{LoadState, PlaybackState, QueueItemId, SharedPlayerState, TrackInfo};

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
    active_playback: Option<ActivePlayback>,
    timeline: Arc<PlaybackTimeline>,
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
            active_playback: None,
            timeline: PlaybackTimeline::new(),
        }
    }

    /// Get a clone of the shared state for UI/FFI reads.
    pub fn shared_state(&self) -> Arc<SharedPlayerState> {
        self.shared_state.clone()
    }

    /// Get the playback timeline for UI reads.
    pub fn timeline(&self) -> Arc<PlaybackTimeline> {
        self.timeline.clone()
    }

    /// Get a command sender for the UI/FFI layer.
    pub fn command_sender(&self) -> crossbeam_channel::Sender<PlayerCommand> {
        self.commands.tx.clone()
    }

    /// Play a specific item in the playlist by ID.
    /// Sets cursor, starts playback if Ready, otherwise waits for TrackReady.
    pub fn play(&mut self, id: QueueItemId) {
        self.shared_state.set_cursor(Some(id));

        match self.shared_state.item_path_if_ready(id) {
            Some(path) => {
                if let Err(e) = self.start_playback(id, &path, 0) {
                    log::error!("play failed: {}", e);
                }
            }
            None => {
                // Item not ready — stop current playback, wait for TrackReady.
                self.stop_engine();
                self.shared_state.set_playback_state(PlaybackState::Stopped);
                log::info!("play: item {:?} not ready, waiting for TrackReady", id);
            }
        }
    }

    /// Internal: start playback of a file.
    fn start_playback(
        &mut self,
        id: QueueItemId,
        path: &Path,
        seek_ms: u64,
    ) -> Result<(), PlayerError> {
        self.stop_engine();

        let info = buffer::probe_file(path)?;

        // Set track_info immediately so the UI never sees a "no playing track" state.
        self.shared_state.set_track_info(Some(TrackInfo {
            id,
            path: path.to_path_buf(),
            codec: info.codec.clone(),
            sample_rate: info.sample_rate,
            bit_depth: info.bit_depth,
            channels: info.channels,
            duration_ms: info.duration_ms,
        }));
        self.shared_state.set_position_ms(0);
        log::info!(
            "playing: {} ({:?}) — {} {}Hz/{}bit/{}ch, {}ms{}",
            path.display(),
            id,
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

        let _generation = self.shared_state.bump_generation();

        // Reset timeline for new playback session and start decode.
        self.timeline.reset();

        // Gapless lookahead: the decode thread maintains its own cursor
        // (separate from the UI cursor) so it can look ahead through the
        // playlist without affecting what the UI shows as "now playing".
        let advance_state = self.shared_state.clone();
        let decode_cursor = std::sync::Mutex::new(Some(id));
        let next_track = move || {
            let current = decode_cursor.lock().unwrap().take()?;
            let next = advance_state.peek_next_ready_after(current);
            if let Some((next_id, _)) = &next {
                *decode_cursor.lock().unwrap() = Some(*next_id);
            }
            next
        };

        let (_stream_info, decode_handle) = buffer::start_decode(
            id,
            path,
            producer,
            seek_ms,
            next_track,
            self.timeline.clone(),
        )?;

        // Create and start audio engine with the timeline's sample counter.
        let actual_rate = device::get_device_sample_rate(device_id).unwrap_or(source_rate);
        let engine = engine::AudioEngine::new(
            device_id,
            actual_rate,
            info.channels as u32,
            consumer,
            self.timeline.samples_played_counter(),
        )?;
        engine.start()?;

        self.shared_state.set_playback_state(PlaybackState::Playing);

        self.active_playback = Some(ActivePlayback {
            engine,
            decode_handle,
        });

        Ok(())
    }

    /// Seek within the current track. Clamps to just before the end to avoid
    /// accidentally skipping. Preserves pause state.
    pub fn seek(&mut self, position_ms: u64) {
        let info = match self.shared_state.track_info() {
            Some(info) => info,
            None => return,
        };
        let id = info.id;
        let path = info.path.clone();
        let duration = info.duration_ms;

        // Clamp to just before the end so we don't skip to the next track.
        let clamped = if duration > 0 {
            position_ms.min(duration.saturating_sub(500))
        } else {
            position_ms
        };

        let was_paused = self.shared_state.playback_state() == PlaybackState::Paused;

        if let Err(e) = self.start_playback(id, &path, clamped) {
            log::error!("seek failed: {}", e);
            return;
        }

        if was_paused {
            self.pause();
        }
    }

    /// Skip to next track in playlist.
    pub fn next_track(&mut self) {
        // advance_cursor finds the next Ready item after cursor.
        match self.shared_state.advance_cursor() {
            Some((id, path)) => {
                if let Err(e) = self.start_playback(id, &path, 0) {
                    log::error!("next track failed: {}", e);
                }
            }
            None => {
                log::info!("no more tracks in playlist");
                self.stop_playback_and_clear_state();
            }
        }
    }

    /// Go back to previous track.
    pub fn prev_track(&mut self) {
        match self.shared_state.retreat_cursor() {
            Some((id, path)) => {
                if matches!(path.try_exists(), Ok(true)) {
                    if let Err(e) = self.start_playback(id, &path, 0) {
                        log::error!("prev track failed: {}", e);
                    }
                } else {
                    log::warn!("prev track path doesn't exist: {}", path.display());
                }
            }
            None => {
                // No previous track — restart current from the beginning.
                if let Some(info) = self.shared_state.track_info()
                    && let Err(e) = self.start_playback(info.id, &info.path, 0)
                {
                    log::error!("restart failed: {}", e);
                }
            }
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

    /// Stop playback and clear playlist.
    pub fn stop(&mut self) {
        self.shared_state.clear_playlist();
        self.stop_playback_and_clear_state();
    }

    /// Stop the audio engine and decode thread without touching shared state.
    fn stop_engine(&mut self) {
        if let Some(mut playback) = self.active_playback.take() {
            let _ = playback.engine.stop();
            playback.decode_handle.stop();
        }
    }

    /// Full stop: tear down engine + clear all display state.
    fn stop_playback_and_clear_state(&mut self) {
        self.stop_engine();
        self.timeline.reset();
        self.shared_state.set_playback_state(PlaybackState::Stopped);
        self.shared_state.set_position_ms(0);
        self.shared_state.set_track_info(None);
    }

    /// Remove a track from the playlist. If it was the cursor, skip to next.
    pub fn remove_from_playlist(&mut self, id: QueueItemId) {
        let was_cursor = self.shared_state.is_cursor(id);
        self.shared_state.remove_item(id);
        if was_cursor {
            self.next_track();
        }
    }

    /// A download finished — if cursor is waiting on this item, start playback.
    pub fn track_ready(&mut self, id: QueueItemId) {
        // Mark as Ready (resolve thread already did this, but be safe).
        self.shared_state.update_load_state(id, LoadState::Ready);

        if !self.shared_state.is_cursor(id) {
            return;
        }

        // Cursor is on this item. If nothing is playing, start playback.
        let is_playing = self.shared_state.playback_state() == PlaybackState::Playing;
        if !is_playing && let Some(path) = self.shared_state.item_path_if_ready(id) {
            log::info!("track_ready: starting playback for {:?}", id);
            if let Err(e) = self.start_playback(id, &path, 0) {
                log::error!("track_ready playback failed: {}", e);
            }
        }
    }

    /// Poll the timeline and update shared state with current track/position.
    /// Called from the command loop on each tick.
    pub fn update_playback_state(&self) {
        if self.active_playback.is_none() {
            return;
        }

        if let Some((id, path, info, position_ms)) = self.timeline.current_playback() {
            self.shared_state.set_position_ms(position_ms);

            // Update track_info + cursor if the timeline shows a different track
            // (gapless transition happened).
            let current_id = self.shared_state.track_info().map(|t| t.id);
            if current_id != Some(id) {
                log::info!("timeline: now playing {:?}", id);
                self.shared_state.set_track_info(Some(TrackInfo {
                    id,
                    path,
                    codec: info.codec,
                    sample_rate: info.sample_rate,
                    bit_depth: info.bit_depth,
                    channels: info.channels,
                    duration_ms: info.duration_ms,
                }));
                self.shared_state.set_cursor(Some(id));
            }
        }
    }

    /// Process a single command.
    pub fn process_command(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::Play(id) => self.play(id),
            PlayerCommand::Pause => self.pause(),
            PlayerCommand::Resume => self.resume(),
            PlayerCommand::Stop => self.stop(),
            PlayerCommand::Seek(pos) => self.seek(pos),
            PlayerCommand::NextTrack => self.next_track(),
            PlayerCommand::PrevTrack => self.prev_track(),
            PlayerCommand::AddToPlaylist(items) => {
                self.shared_state.add_items(items);
            }
            PlayerCommand::RemoveFromPlaylist(id) => self.remove_from_playlist(id),
            PlayerCommand::MoveInPlaylist { id, target, after } => {
                self.shared_state.move_item(id, target, after);
            }
            PlayerCommand::MoveItemsInPlaylist { ids, target, after } => {
                self.shared_state.move_items(&ids, target, after);
            }
            PlayerCommand::TrackReady(id) => self.track_ready(id),
        }
    }

    /// Run the command loop. Blocks until the sender is dropped.
    pub fn run(&mut self) {
        use std::time::Duration;

        let rx = self.commands.rx.clone();
        loop {
            // Poll with timeout so we update position even without commands.
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(cmd) => self.process_command(cmd),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
            self.update_playback_state();
        }
        self.stop();
    }

    /// Spawn the player on a background thread, returning the shared state,
    /// timeline, and command sender.
    pub fn spawn() -> (
        Arc<SharedPlayerState>,
        Arc<PlaybackTimeline>,
        crossbeam_channel::Sender<PlayerCommand>,
    ) {
        let mut player = Self::new();
        let state = player.shared_state();
        let timeline = player.timeline();
        let tx = player.command_sender();

        thread::Builder::new()
            .name("koan-player".into())
            .spawn(move || player.run())
            .expect("failed to spawn player thread");

        (state, timeline, tx)
    }
}
