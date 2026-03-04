pub mod commands;
pub mod state;
pub mod undo;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use thiserror::Error;

use crate::audio::{
    analyzer::VizAnalyzer,
    buffer, device, engine, streaming,
    viz::{VizBuffer, VizSnapshot},
};
use buffer::PlaybackTimeline;
use commands::{CommandChannel, PlayerCommand};
use state::{LoadState, PlaybackSource, PlaybackState, QueueItemId, SharedPlayerState, TrackInfo};
use undo::{UndoEntry, UndoStack};

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
    viz_buffer: Arc<VizBuffer>,
    viz_snapshot: Arc<VizSnapshot>,
    /// Background FFT analysis thread. Held for its lifetime; dropped on Player drop.
    _viz_analyzer: VizAnalyzer,
    undo_stack: UndoStack,
    /// When Some, undo entries are collected into this buffer instead of pushed
    /// directly onto the undo stack. Flushed on EndUndoBatch.
    batch_buffer: Option<Vec<UndoEntry>>,
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
        let viz_buffer = VizBuffer::new();
        let viz_snapshot = VizSnapshot::new();
        let cfg = crate::config::Config::load().unwrap_or_default();
        let viz_analyzer = VizAnalyzer::spawn_with_snapshot(
            Arc::clone(&viz_buffer),
            &cfg.visualizer,
            Arc::clone(&viz_snapshot),
        );

        Self {
            shared_state: SharedPlayerState::new(),
            commands: CommandChannel::new(),
            active_playback: None,
            timeline: PlaybackTimeline::new(),
            viz_buffer,
            viz_snapshot,
            _viz_analyzer: viz_analyzer,
            undo_stack: UndoStack::new(),
            batch_buffer: None,
        }
    }

    /// Get a clone of the shared state for UI reads.
    pub fn shared_state(&self) -> Arc<SharedPlayerState> {
        self.shared_state.clone()
    }

    /// Get the playback timeline for UI reads.
    pub fn timeline(&self) -> Arc<PlaybackTimeline> {
        self.timeline.clone()
    }

    /// Get the visualization buffer for the TUI.
    pub fn viz_buffer(&self) -> Arc<VizBuffer> {
        self.viz_buffer.clone()
    }

    /// Get the shared analysis snapshot for the TUI.
    /// The analysis thread writes here; the UI thread reads a clone each frame.
    pub fn viz_snapshot(&self) -> Arc<VizSnapshot> {
        self.viz_snapshot.clone()
    }

    /// Access undo stack (for tests and UI state queries).
    pub fn undo_stack(&self) -> &UndoStack {
        &self.undo_stack
    }

    /// Get a command sender for the UI layer.
    pub fn command_sender(&self) -> crossbeam_channel::Sender<PlayerCommand> {
        self.commands.tx.clone()
    }

    /// Play a specific item in the playlist by ID.
    /// Sets cursor, starts playback if Ready or streaming-ready, otherwise waits for TrackReady.
    pub fn play(&mut self, id: QueueItemId) {
        self.shared_state.set_cursor(Some(id));

        match self.shared_state.item_playback_source(id) {
            Some(PlaybackSource::Ready(path)) => {
                if let Err(e) = self.start_playback(id, &path, 0) {
                    log::error!("play failed: {}", e);
                }
            }
            Some(PlaybackSource::Streaming {
                path,
                bytes_written,
                total,
            }) => {
                if let Err(e) = self.start_streaming_playback(id, &path, bytes_written, total) {
                    log::error!("streaming play failed, waiting for full download: {}", e);
                    // Fall back to waiting for TrackReady.
                    self.stop_engine();
                    self.shared_state.set_playback_state(PlaybackState::Stopped);
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

        // Set track_info + position immediately so the UI never sees a gap.
        // For seeks, this keeps the bar at the target position instead of
        // flashing to 0 while the new timeline spins up.
        self.shared_state.set_track_info(Some(TrackInfo {
            id,
            path: path.to_path_buf(),
            codec: info.codec.clone(),
            sample_rate: info.sample_rate,
            bit_depth: info.bit_depth,
            channels: info.channels,
            duration_ms: info.duration_ms,
        }));
        self.shared_state.set_position_ms(seek_ms);
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
            let current = decode_cursor.lock().ok()?.take()?;
            let next = advance_state.peek_next_ready_after(current);
            if let Some((next_id, _)) = &next
                && let Ok(mut guard) = decode_cursor.lock()
            {
                *guard = Some(*next_id);
            }
            next
        };

        let (_stream_info, decode_handle) = buffer::start_decode_file(
            id,
            path,
            producer,
            seek_ms,
            next_track,
            self.timeline.clone(),
            Some(self.viz_buffer.clone()),
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

    /// Internal: start streaming playback from a partially-downloaded file.
    ///
    /// Creates a StreamBuffer and a pump thread that reads from the on-disk partial
    /// file as bytes become available (tracked via `bytes_written`). The decode thread
    /// reads from a StreamingSource backed by that buffer, blocking briefly when it
    /// catches up to the write head.
    fn start_streaming_playback(
        &mut self,
        id: QueueItemId,
        path: &Path,
        bytes_written: Arc<AtomicU64>,
        total: u64,
    ) -> Result<(), PlayerError> {
        self.stop_engine();

        // Create a StreamBuffer with known total length.
        let stream_buf = streaming::StreamBuffer::new(if total > 0 { Some(total) } else { None });

        // Spawn a pump thread: reads bytes from the on-disk partial file as they
        // become available (per bytes_written) and pushes them into StreamBuffer.
        // This bridges the disk-based download with StreamingSource's in-memory design.
        let pump_path = path.to_path_buf();
        let pump_buf = stream_buf.clone();
        let pump_written = bytes_written.clone();
        thread::Builder::new()
            .name("koan-stream-pump".into())
            .spawn(move || {
                use std::fs::File;
                use std::io::Read;
                let mut file = match File::open(&pump_path) {
                    Ok(f) => f,
                    Err(e) => {
                        log::error!("stream pump: failed to open {}: {}", pump_path.display(), e);
                        pump_buf.finish();
                        return;
                    }
                };
                let mut buf = vec![0u8; 65536];
                let mut offset: u64 = 0;
                loop {
                    let available = pump_written.load(Ordering::Relaxed);
                    if offset >= available {
                        if total > 0 && available >= total {
                            break; // Download complete.
                        }
                        thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                    let to_read = ((available - offset) as usize).min(buf.len());
                    match file.read(&mut buf[..to_read]) {
                        Ok(0) => break,
                        Ok(n) => {
                            pump_buf.push(&buf[..n]);
                            offset += n as u64;
                        }
                        Err(e) => {
                            log::warn!("stream pump read error: {}", e);
                            break;
                        }
                    }
                }
                pump_buf.finish();
            })
            .map_err(|e| PlayerError::Decode(buffer::DecodeError::Io(e)))?;

        // Probe via a streaming reader — blocks (via condvar) until enough header data arrives.
        let probe_reader = stream_buf.reader();
        let probe_hint = {
            let mut h = symphonia::core::probe::Hint::new();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                h.with_extension(ext);
            }
            h
        };
        let probe_mss =
            symphonia::core::io::MediaSourceStream::new(Box::new(probe_reader), Default::default());
        let info = buffer::probe_source(probe_mss, &probe_hint)?;

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
            "streaming: {} ({:?}) — {} {}Hz/{}bit/{}ch, {}ms",
            path.display(),
            id,
            info.codec,
            info.sample_rate,
            info.bit_depth,
            info.channels,
            info.duration_ms,
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
        self.timeline.reset();

        // Gapless lookahead after streaming: next track uses normal file path.
        let advance_state = self.shared_state.clone();
        let decode_cursor = std::sync::Mutex::new(Some(id));
        let next_track = move || {
            let current = decode_cursor.lock().ok()?.take()?;
            let next = advance_state.peek_next_ready_after(current);
            if let Some((next_id, _)) = &next
                && let Ok(mut guard) = decode_cursor.lock()
            {
                *guard = Some(*next_id);
            }
            next
        };

        // Decode using a fresh StreamingSource reader — reads from the StreamBuffer
        // that the pump thread feeds. The decode thread blocks when it catches up to
        // the write head, resuming as more data arrives.
        // Build a SourceEntry using a fresh StreamingSource reader for the decode thread.
        let decode_reader = stream_buf.reader();
        let path_buf = path.to_path_buf();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        let mut decode_hint = symphonia::core::probe::Hint::new();
        if !ext.is_empty() {
            decode_hint.with_extension(&ext);
        }
        let first = buffer::SourceEntry {
            id,
            path: path_buf,
            hint: decode_hint,
            make_mss: Box::new(move || {
                symphonia::core::io::MediaSourceStream::new(
                    Box::new(decode_reader),
                    Default::default(),
                )
            }),
        };

        let (_stream_info, decode_handle) = buffer::start_decode(
            first,
            producer,
            0,
            move || {
                let (next_id, next_path) = next_track()?;
                Some(buffer::SourceEntry::from_file(next_id, next_path))
            },
            self.timeline.clone(),
            Some(self.viz_buffer.clone()),
        )?;

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
    /// If already streaming this item, trigger progressive metadata enhancement.
    pub fn track_ready(&mut self, id: QueueItemId) {
        // Mark as Ready (download thread already did this, but be safe).
        self.shared_state.update_load_state(id, LoadState::Ready);

        if !self.shared_state.is_cursor(id) {
            return;
        }

        let is_playing = self.shared_state.playback_state() == PlaybackState::Playing;
        let current_track_id = self.shared_state.track_info().map(|t| t.id);

        if is_playing && current_track_id == Some(id) {
            // Already streaming this track — download just finished.
            // Trigger progressive enhancement: re-read full lofty metadata and update state.
            log::info!(
                "track_ready: download complete while streaming {:?}, refreshing metadata",
                id
            );
            self.refresh_track_metadata(id);
            return;
        }

        // Cursor is on this item but not yet playing — start playback now.
        if !is_playing && let Some(path) = self.shared_state.item_path_if_ready(id) {
            log::info!("track_ready: starting playback for {:?}", id);
            if let Err(e) = self.start_playback(id, &path, 0) {
                log::error!("track_ready playback failed: {}", e);
            }
        }
    }

    /// Called when enough data has been buffered for streaming playback.
    /// If the cursor is waiting on this track and nothing is playing, start streaming.
    pub fn track_stream_ready(&mut self, id: QueueItemId) {
        if !self.shared_state.is_cursor(id) {
            return;
        }

        let is_playing = self.shared_state.playback_state() == PlaybackState::Playing;
        if is_playing {
            return; // Already playing something — don't interrupt.
        }

        match self.shared_state.item_playback_source(id) {
            Some(PlaybackSource::Streaming {
                path,
                bytes_written,
                total,
            }) => {
                log::info!(
                    "track_stream_ready: starting streaming playback for {:?}",
                    id
                );
                if let Err(e) = self.start_streaming_playback(id, &path, bytes_written, total) {
                    log::error!("track_stream_ready streaming failed: {}", e);
                }
            }
            Some(PlaybackSource::Ready(path)) => {
                // Download finished between threshold and now — just play normally.
                log::info!(
                    "track_stream_ready: track already ready, starting normal playback for {:?}",
                    id
                );
                if let Err(e) = self.start_playback(id, &path, 0) {
                    log::error!("track_stream_ready playback failed: {}", e);
                }
            }
            None => {} // Not enough data yet — wait.
        }
    }

    /// Re-read full lofty metadata for a track after its download completes.
    /// Updates the playlist item's tags and track_info with complete metadata.
    /// Called from track_ready() when a streaming track finishes downloading.
    fn refresh_track_metadata(&mut self, id: QueueItemId) {
        use crate::index::metadata;

        let path = match self.shared_state.item_path_if_ready(id) {
            Some(p) => p,
            None => return,
        };

        match metadata::read_metadata(&path) {
            Ok(meta) => {
                // Update playlist item with full lofty tags (title, artist, album, duration).
                self.shared_state.update_item_metadata(
                    id,
                    meta.title,
                    meta.artist,
                    meta.album_artist.unwrap_or_default(),
                    meta.album,
                    meta.duration_ms.map(|d| d as u64),
                );
                // Signal UI to re-read cover art and update souvlaki media controls.
                self.shared_state.signal_metadata_refresh();
                log::info!("track_ready: metadata refreshed for {:?}", id);
            }
            Err(e) => {
                log::warn!("track_ready: metadata refresh failed for {:?}: {}", id, e);
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

    /// Route an undo entry to the batch buffer (if batching) or the undo stack.
    fn push_undo(&mut self, entry: UndoEntry) {
        if let Some(ref mut batch) = self.batch_buffer {
            batch.push(entry);
        } else {
            self.undo_stack.push(entry);
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
                let ids: Vec<QueueItemId> = items.iter().map(|i| i.id).collect();
                self.shared_state.add_items(items);
                self.push_undo(UndoEntry::Added { ids });
            }
            PlayerCommand::UpdatePaths(updates) => {
                self.shared_state.update_paths(&updates);
                if let Some(info) = self.shared_state.track_info()
                    && let Some((_, new_path)) = updates.iter().find(|(id, _)| *id == info.id)
                {
                    self.shared_state.set_track_info(Some(TrackInfo {
                        path: new_path.clone(),
                        ..info
                    }));
                }
            }
            PlayerCommand::InsertInPlaylist { items, after } => {
                let ids: Vec<QueueItemId> = items.iter().map(|i| i.id).collect();
                self.shared_state.insert_items_after(items, after);
                self.push_undo(UndoEntry::Inserted { ids });
            }
            PlayerCommand::ClearPlaylist => {
                let (items, cursor) = self.shared_state.snapshot_playlist();
                self.stop();
                self.shared_state.clear_playlist();
                self.push_undo(UndoEntry::Replaced { items, cursor });
            }
            PlayerCommand::RemoveFromPlaylist(id) => {
                let item = self.shared_state.get_item(id);
                let after = self.shared_state.item_before(id);
                self.remove_from_playlist(id);
                if let Some(item) = item {
                    self.push_undo(UndoEntry::Removed {
                        items: vec![(Box::new(item), after)],
                    });
                }
            }
            PlayerCommand::RemoveFromPlaylistBatch(ids) => {
                // Snapshot all items with their predecessors before removing any.
                let items_with_pos: Vec<_> = ids
                    .iter()
                    .filter_map(|&id| {
                        let item = self.shared_state.get_item(id)?;
                        let after = self.shared_state.item_before(id);
                        Some((Box::new(item), after))
                    })
                    .collect();
                for &id in &ids {
                    self.remove_from_playlist(id);
                }
                if !items_with_pos.is_empty() {
                    self.push_undo(UndoEntry::Removed {
                        items: items_with_pos,
                    });
                }
            }
            PlayerCommand::MoveInPlaylist { id, target, after } => {
                let was_after = self.shared_state.item_before(id);
                self.shared_state.move_item(id, target, after);
                self.push_undo(UndoEntry::Moved { id, was_after });
            }
            PlayerCommand::MoveItemsInPlaylist { ids, target, after } => {
                let entries = self.shared_state.items_before(&ids);
                self.shared_state.move_items(&ids, target, after);
                self.push_undo(UndoEntry::MovedBatch { entries });
            }
            PlayerCommand::TrackReady(id) => self.track_ready(id),
            PlayerCommand::TrackStreamReady(id) => self.track_stream_ready(id),
            PlayerCommand::Undo => self.execute_undo(),
            PlayerCommand::Redo => self.execute_redo(),
            PlayerCommand::BeginUndoBatch => {
                self.batch_buffer = Some(Vec::new());
            }
            PlayerCommand::EndUndoBatch => {
                if let Some(entries) = self.batch_buffer.take() {
                    if entries.len() == 1 {
                        // Single entry — push directly, no wrapping.
                        self.undo_stack.push(entries.into_iter().next().unwrap());
                    } else if !entries.is_empty() {
                        self.undo_stack.push(UndoEntry::Batch(entries));
                    }
                }
            }
        }
    }

    /// Apply an undo/redo entry: mutate the playlist and return the inverse entry.
    fn apply_entry(&mut self, entry: UndoEntry) -> Option<UndoEntry> {
        match entry {
            UndoEntry::Added { ids } => {
                // Undo of "items were added": snapshot them with positions, then remove.
                let items_with_pos: Vec<_> = ids
                    .iter()
                    .filter_map(|&id| {
                        let item = self.shared_state.get_item(id)?;
                        let after = self.shared_state.item_before(id);
                        Some((Box::new(item), after))
                    })
                    .collect();
                self.shared_state.remove_items(&ids);
                Some(UndoEntry::Removed {
                    items: items_with_pos,
                })
            }
            UndoEntry::Removed { items } => {
                // Undo of "items were removed": re-insert each at its position.
                let mut ids = Vec::with_capacity(items.len());
                for (item, after) in items {
                    ids.push(item.id);
                    self.shared_state.insert_item_at(*item, after);
                }
                Some(UndoEntry::Added { ids })
            }
            UndoEntry::Inserted { ids } => {
                // Same as Added — snapshot positions, remove items.
                let items_with_pos: Vec<_> = ids
                    .iter()
                    .filter_map(|&id| {
                        let item = self.shared_state.get_item(id)?;
                        let after = self.shared_state.item_before(id);
                        Some((Box::new(item), after))
                    })
                    .collect();
                self.shared_state.remove_items(&ids);
                Some(UndoEntry::Removed {
                    items: items_with_pos,
                })
            }
            UndoEntry::Moved { id, was_after } => {
                let current_after = self.shared_state.item_before(id);
                self.shared_state.move_item_to(id, was_after);
                Some(UndoEntry::Moved {
                    id,
                    was_after: current_after,
                })
            }
            UndoEntry::MovedBatch { entries } => {
                let ids: Vec<QueueItemId> = entries.iter().map(|(id, _)| *id).collect();
                let current_positions = self.shared_state.items_before(&ids);
                self.shared_state.move_items_to(&entries);
                Some(UndoEntry::MovedBatch {
                    entries: current_positions,
                })
            }
            UndoEntry::Replaced { items, cursor } => {
                let (current_items, current_cursor) = self.shared_state.snapshot_playlist();
                self.shared_state.restore_playlist(items, cursor);
                Some(UndoEntry::Replaced {
                    items: current_items,
                    cursor: current_cursor,
                })
            }
            UndoEntry::Batch(entries) => {
                // Apply entries in reverse order, collect inverses.
                let mut inverses = Vec::with_capacity(entries.len());
                for entry in entries.into_iter().rev() {
                    if let Some(inverse) = self.apply_entry(entry) {
                        inverses.push(inverse);
                    }
                }
                inverses.reverse();
                Some(UndoEntry::Batch(inverses))
            }
        }
    }

    /// Execute an undo operation, pushing the inverse onto the redo stack.
    fn execute_undo(&mut self) {
        let Some(entry) = self.undo_stack.pop_undo() else {
            return;
        };
        if let Some(inverse) = self.apply_entry(entry) {
            self.undo_stack.push_redo(inverse);
        }
    }

    /// Execute a redo operation, pushing the inverse onto the undo stack.
    fn execute_redo(&mut self) {
        let Some(entry) = self.undo_stack.pop_redo() else {
            return;
        };
        if let Some(inverse) = self.apply_entry(entry) {
            self.undo_stack.push_undo_keep_redo(inverse);
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
    /// timeline, visualization snapshot, and command sender.
    pub fn spawn() -> (
        Arc<SharedPlayerState>,
        Arc<PlaybackTimeline>,
        Arc<VizSnapshot>,
        crossbeam_channel::Sender<PlayerCommand>,
    ) {
        let mut player = Self::new();
        let state = player.shared_state();
        let timeline = player.timeline();
        let viz_snapshot = player.viz_snapshot();
        let tx = player.command_sender();

        thread::Builder::new()
            .name("koan-player".into())
            .spawn(move || player.run())
            .expect("failed to spawn player thread");

        (state, timeline, viz_snapshot, tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use state::PlaylistItem;
    use std::path::PathBuf;

    fn make_item(title: &str) -> PlaylistItem {
        PlaylistItem {
            id: QueueItemId::new(),
            path: PathBuf::from(format!("/music/{title}.flac")),
            title: title.to_string(),
            artist: String::new(),
            album_artist: String::new(),
            album: String::new(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            load_state: LoadState::Ready,
        }
    }

    fn playlist_ids(player: &Player) -> Vec<QueueItemId> {
        let (items, _) = player.shared_state.snapshot_playlist();
        items.iter().map(|i| i.id).collect()
    }

    fn playlist_titles(player: &Player) -> Vec<String> {
        let (items, _) = player.shared_state.snapshot_playlist();
        items.iter().map(|i| i.title.clone()).collect()
    }

    // --- AddToPlaylist undo/redo ---

    #[test]
    fn undo_add_removes_items() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B")];
        let ids: Vec<_> = items.iter().map(|i| i.id).collect();

        player.process_command(PlayerCommand::AddToPlaylist(items));
        assert_eq!(playlist_ids(&player), ids);
        assert!(player.undo_stack().can_undo());

        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());
        assert!(player.undo_stack().can_redo());
    }

    #[test]
    fn redo_add_restores_items() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B")];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_titles(&player), vec!["A", "B"]);
    }

    // --- RemoveFromPlaylist undo/redo ---

    #[test]
    fn undo_remove_restores_item_at_position() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let b_id = items[1].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylist(b_id));
        assert_eq!(playlist_titles(&player), vec!["A", "C"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);
    }

    #[test]
    fn undo_remove_first_item() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B")];
        let a_id = items[0].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylist(a_id));
        assert_eq!(playlist_titles(&player), vec!["B"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B"]);
    }

    #[test]
    fn undo_batch_remove_restores_all() {
        let mut player = Player::new();
        let items = vec![
            make_item("A"),
            make_item("B"),
            make_item("C"),
            make_item("D"),
        ];
        let b_id = items[1].id;
        let c_id = items[2].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![b_id, c_id]));
        assert_eq!(playlist_titles(&player), vec!["A", "D"]);

        // Single undo restores both
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C", "D"]);
    }

    #[test]
    fn redo_batch_remove() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let a_id = items[0].id;
        let b_id = items[1].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![a_id, b_id]));
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_titles(&player), vec!["C"]);
    }

    #[test]
    fn redo_remove() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let b_id = items[1].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylist(b_id));
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_titles(&player), vec!["A", "C"]);
    }

    // --- InsertInPlaylist undo/redo ---

    #[test]
    fn undo_insert_removes_inserted_items() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("C")];
        let a_id = items[0].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));

        let inserted = vec![make_item("B")];
        player.process_command(PlayerCommand::InsertInPlaylist {
            items: inserted,
            after: a_id,
        });
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "C"]);
    }

    // --- MoveInPlaylist undo/redo ---

    #[test]
    fn undo_move_restores_position() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let a_id = items[0].id;
        let c_id = items[2].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));

        // Move A after C: [B, C, A]
        player.process_command(PlayerCommand::MoveInPlaylist {
            id: a_id,
            target: c_id,
            after: true,
        });
        assert_eq!(playlist_titles(&player), vec!["B", "C", "A"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);
    }

    #[test]
    fn redo_move() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let a_id = items[0].id;
        let c_id = items[2].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::MoveInPlaylist {
            id: a_id,
            target: c_id,
            after: true,
        });
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_titles(&player), vec!["B", "C", "A"]);
    }

    // --- MoveItemsInPlaylist (batch) undo/redo ---

    #[test]
    fn undo_batch_move() {
        let mut player = Player::new();
        let items = vec![
            make_item("A"),
            make_item("B"),
            make_item("C"),
            make_item("D"),
        ];
        let a_id = items[0].id;
        let b_id = items[1].id;
        let d_id = items[3].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));

        // Move A,B after D: [C, D, A, B]
        player.process_command(PlayerCommand::MoveItemsInPlaylist {
            ids: vec![a_id, b_id],
            target: d_id,
            after: true,
        });
        assert_eq!(playlist_titles(&player), vec!["C", "D", "A", "B"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C", "D"]);
    }

    // --- ClearPlaylist undo/redo ---

    #[test]
    fn undo_clear_restores_playlist() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::ClearPlaylist);
        assert!(playlist_ids(&player).is_empty());

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);
    }

    #[test]
    fn redo_clear() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B")];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::ClearPlaylist);
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B"]);

        player.process_command(PlayerCommand::Redo);
        assert!(playlist_ids(&player).is_empty());
    }

    // --- Multi-step undo/redo ---

    #[test]
    fn multiple_undos_in_sequence() {
        let mut player = Player::new();

        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("A")]));
        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("B")]));
        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("C")]));
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A"]);

        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());
    }

    #[test]
    fn undo_redo_undo_cycle() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B")];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_titles(&player), vec!["A", "B"]);

        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());
    }

    #[test]
    fn new_action_clears_redo_stack() {
        let mut player = Player::new();
        let items = vec![make_item("A")];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::Undo);
        assert!(player.undo_stack().can_redo());

        // New action should clear redo
        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("B")]));
        assert!(!player.undo_stack().can_redo());
    }

    #[test]
    fn undo_on_empty_stack_is_noop() {
        let mut player = Player::new();
        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());
    }

    #[test]
    fn redo_on_empty_stack_is_noop() {
        let mut player = Player::new();
        player.process_command(PlayerCommand::Redo);
        assert!(playlist_ids(&player).is_empty());
    }

    // --- Non-undoable commands don't push entries ---

    #[test]
    fn playback_commands_not_undoable() {
        let mut player = Player::new();
        player.process_command(PlayerCommand::Pause);
        player.process_command(PlayerCommand::Resume);
        player.process_command(PlayerCommand::NextTrack);
        player.process_command(PlayerCommand::PrevTrack);
        assert!(!player.undo_stack().can_undo());
    }

    #[test]
    fn update_paths_not_undoable() {
        let mut player = Player::new();
        let items = vec![make_item("A")];
        let id = items[0].id;
        player.process_command(PlayerCommand::AddToPlaylist(items));

        let undo_count = player.undo_stack().undo_len();
        player.process_command(PlayerCommand::UpdatePaths(vec![(
            id,
            PathBuf::from("/new/path.flac"),
        )]));
        assert_eq!(player.undo_stack().undo_len(), undo_count);
    }

    // --- Complex scenarios ---

    #[test]
    fn add_remove_undo_undo_produces_original() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let b_id = items[1].id;
        let original_titles = vec!["A", "B", "C"];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylist(b_id));
        assert_eq!(playlist_titles(&player), vec!["A", "C"]);

        // Undo remove → back to A, B, C
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), original_titles);

        // Undo add → empty
        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());
    }

    #[test]
    fn interleaved_adds_and_moves_undo() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let a_id = items[0].id;
        let c_id = items[2].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));

        // Move A after C: [B, C, A]
        player.process_command(PlayerCommand::MoveInPlaylist {
            id: a_id,
            target: c_id,
            after: true,
        });
        assert_eq!(playlist_titles(&player), vec!["B", "C", "A"]);

        // Add D: [B, C, A, D]
        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("D")]));
        assert_eq!(playlist_titles(&player), vec!["B", "C", "A", "D"]);

        // Undo add D: [B, C, A]
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["B", "C", "A"]);

        // Undo move: [A, B, C]
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);
    }
}
