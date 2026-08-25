pub mod commands;
pub mod history;
pub mod state;
pub mod undo;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use thiserror::Error;

use crate::audio::{
    analyzer::VizAnalyzer,
    backend::{self, AudioBackend, AudioEngineHandle, BackendError, SampleRateWatch},
    buffer, streaming,
    viz::{VizBuffer, VizSnapshot},
};
use buffer::PlaybackTimeline;
use commands::{CommandChannel, PlayerCommand};
use history::{InFlight, PlayEvent, PlayRecorder};
use state::{LoadState, PlaybackSource, PlaybackState, QueueItemId, SharedPlayerState, TrackInfo};
use undo::{UndoEntry, UndoStack};

/// Ring buffer size in samples. ~1s at 192kHz stereo.
pub(crate) const RING_BUFFER_SIZE: usize = 192_000 * 2;

#[derive(Debug, Error)]
pub enum PlayerError {
    #[error("backend error: {0}")]
    Backend(#[from] BackendError),
    #[error("decode error: {0}")]
    Decode(#[from] buffer::DecodeError),
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
    /// Configured output device name. None = system default.
    output_device_name: Option<String>,
    /// Platform audio backend (CoreAudio on macOS, cpal on Linux).
    backend: Box<dyn AudioBackend>,
    /// Debounce: timestamp of last NextTrack/PrevTrack to suppress key repeat.
    last_skip: std::time::Instant,
    /// Writes plays away from this thread. None when there is no database to
    /// write to, and in tests, which must not touch the real library.
    history: Option<PlayRecorder>,
    /// How much of the current track has been heard so far.
    in_flight: Option<InFlight>,
    /// Playback sessions started — lets tests assert how many engine restarts
    /// an operation costs.
    #[cfg(test)]
    playback_starts: usize,
}

/// Holds the resources for an active playback session.
struct ActivePlayback {
    engine: Box<dyn AudioEngineHandle>,
    decode_handle: buffer::DecodeHandle,
    /// Keeps the device rate subscription alive for as long as this engine is
    /// the one feeding the DAC. Dropped with it.
    _rate_watch: Option<Box<dyn SampleRateWatch>>,
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
        let timeline = PlaybackTimeline::new();
        let cfg = crate::config::Config::load_or_default();
        let viz_analyzer = VizAnalyzer::spawn_with_snapshot(
            Arc::clone(&viz_buffer),
            &cfg.visualizer,
            Arc::clone(&viz_snapshot),
            timeline.samples_played_counter(),
        );

        Self {
            shared_state: SharedPlayerState::new(),
            commands: CommandChannel::new(),
            active_playback: None,
            timeline,
            viz_buffer,
            viz_snapshot,
            _viz_analyzer: viz_analyzer,
            undo_stack: UndoStack::new(),
            batch_buffer: None,
            output_device_name: cfg.playback.output_device.clone(),
            backend: crate::audio::platform_backend(),
            last_skip: std::time::Instant::now(),
            history: None,
            in_flight: None,
            #[cfg(test)]
            playback_starts: 0,
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

    /// Create an audio engine for a stream, switching the output device to the
    /// source rate first so output is bit-perfect.
    ///
    /// The engine is always configured with the source's own rate and channel
    /// count — the format the decode thread writes into the ring buffer. A
    /// device that cannot take the requested rate (MPEG-2/2.5 MP3 rates are
    /// commonly refused) resamples instead of playing at the wrong speed.
    #[allow(clippy::type_complexity)]
    fn create_engine_for(
        &self,
        info: &buffer::StreamInfo,
        consumer: rtrb::Consumer<f32>,
    ) -> Result<(Box<dyn AudioEngineHandle>, Option<Box<dyn SampleRateWatch>>), PlayerError> {
        let device = self.resolve_device()?;
        let device_rate = self.backend.get_device_sample_rate(&device)?;
        let source_rate = info.sample_rate as f64;

        let settled = if (device_rate - source_rate).abs() > 0.1 {
            log::info!(
                "switching device sample rate: {}Hz → {}Hz",
                device_rate,
                source_rate
            );
            match self.backend.set_device_sample_rate(&device, source_rate) {
                Ok(rate) => rate,
                Err(e) => {
                    log::warn!("failed to set device sample rate: {}", e);
                    device_rate
                }
            }
        } else {
            device_rate
        };

        if (settled - source_rate).abs() > 0.1 {
            log::warn!(
                "device stayed at {}Hz (wanted {}Hz) — output is resampled, not bit-perfect",
                settled,
                source_rate
            );
        }

        // The front ends compare this against the source rate to say whether
        // anything had to resample. A log line was the only place it went.
        self.shared_state
            .set_output_sample_rate(settled.round() as u32);

        // koan is not the only client of this device. Subscribe so the front
        // ends learn about a rate someone else moved instead of trusting the
        // reading above until the next track happens to build an engine.
        let watch_state = self.shared_state.clone();
        let watch_name = device.name.clone();
        let rate_watch = self.backend.watch_device_sample_rate(
            &device,
            Box::new(move |rate| {
                log::info!("device sample rate changed externally: {rate}Hz on '{watch_name}'");
                watch_state.set_output_sample_rate(rate.round() as u32);
            }),
        );

        let engine = self.backend.create_engine(
            &device,
            source_rate,
            info.channels as u32,
            consumer,
            self.timeline.samples_played_counter(),
        )?;

        Ok((engine, rate_watch))
    }

    /// Resolve the output device: use configured device name if set,
    /// falling back to system default if not set or if the named device is unavailable.
    fn resolve_device(&self) -> Result<backend::DeviceInfo, PlayerError> {
        if let Some(ref name) = self.output_device_name {
            match self.backend.list_devices() {
                Ok(devices) => {
                    if let Some(dev) = devices.into_iter().find(|d| d.name == *name) {
                        return Ok(dev);
                    }
                    log::warn!(
                        "configured output device '{}' not found, falling back to default",
                        name,
                    );
                }
                Err(e) => {
                    log::warn!("failed to list devices while resolving '{}': {}", name, e);
                }
            }
        }
        Ok(self.backend.default_device()?)
    }

    /// Switch the output device. Persists to config and restarts the engine
    /// on the current track if playing.
    pub fn set_output_device(&mut self, name: String) {
        log::info!("switching output device to: {}", name);
        self.output_device_name = Some(name.clone());

        // Persist to config.toml (not the merged config — avoids leaking secrets).
        if let Err(e) = crate::config::Config::update_base(|cfg| {
            cfg.playback.output_device = Some(name);
        }) {
            log::error!("failed to save output device config: {}", e);
        }

        self.restart_on_current_track();
    }

    /// Clear the configured output device, reverting to system default.
    pub fn clear_output_device(&mut self) {
        log::info!("reverting to system default output device");
        self.output_device_name = None;

        if let Err(e) = crate::config::Config::update_base(|cfg| {
            cfg.playback.output_device = None;
        }) {
            log::error!("failed to save output device config: {}", e);
        }

        self.restart_on_current_track();
    }

    /// If a track is currently playing or paused, restart playback at the
    /// current position (e.g. after switching output devices). Preserves pause state.
    fn restart_on_current_track(&mut self) {
        if let Some(info) = self.shared_state.track_info() {
            let was_paused = self.shared_state.playback_state() == PlaybackState::Paused;
            let position_ms = self.shared_state.position_ms();
            if let Err(e) = self.start_playback(info.id, &info.path, position_ms) {
                log::error!("failed to restart playback on device switch: {}", e);
                return;
            }
            if was_paused {
                self.pause();
            }
        }
    }

    /// Get the current output device name (if configured).
    pub fn output_device_name(&self) -> Option<&str> {
        self.output_device_name.as_deref()
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
                    // The cursor stays here, so TrackReady starts it once the
                    // whole file has landed.
                    log::error!("streaming play failed, waiting for full download: {}", e);
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
    ///
    /// A failure leaves the player cleanly stopped. Displaying a track that no
    /// engine is playing freezes the position and makes the transport lie.
    fn start_playback(
        &mut self,
        id: QueueItemId,
        path: &Path,
        seek_ms: u64,
    ) -> Result<(), PlayerError> {
        #[cfg(test)]
        {
            self.playback_starts += 1;
        }
        let result = self.open_playback(id, path, seek_ms);
        if result.is_err() {
            self.stop_playback_and_clear_state();
        }
        result
    }

    fn open_playback(
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
            bitrate_kbps: info.bitrate_kbps,
            channels: info.channels,
            duration_ms: info.duration_ms,
        }));
        self.shared_state.set_position_ms(seek_ms);
        self.on_track_changed(id);
        log::info!(
            "playing: {} ({:?}) — {} {}Hz/{}ch, {}ms{}",
            path.display(),
            id,
            info.codec,
            info.sample_rate,
            info.channels,
            info.duration_ms,
            if seek_ms > 0 {
                format!(" @{}ms", seek_ms)
            } else {
                String::new()
            }
        );

        let (producer, consumer) = rtrb::RingBuffer::new(RING_BUFFER_SIZE);

        // Reset timeline for new playback session and start decode.
        self.timeline.reset();

        // Gapless lookahead: the decode thread maintains its own cursor
        // (separate from the UI cursor) so it can look ahead through the
        // playlist without affecting what the UI shows as "now playing".
        let advance_state = self.shared_state.clone();
        let decode_cursor = parking_lot::Mutex::new(Some(id));
        let next_track = move || {
            let current = decode_cursor.lock().take()?;
            let next = advance_state.peek_next_ready_after(current);
            if let Some((next_id, _)) = &next {
                let mut guard = decode_cursor.lock();
                *guard = Some(*next_id);
            }
            next
        };

        // Load ReplayGain config for this playback session.
        let cfg = crate::config::Config::load_or_default();
        let rg_mode = cfg.playback.replaygain;
        let pre_amp_db = cfg.playback.pre_amp_db;

        let finish_tx = self.commands.tx.clone();
        let (_stream_info, decode_handle) = buffer::start_decode_file(
            id,
            path,
            producer,
            seek_ms,
            next_track,
            self.timeline.clone(),
            Some(self.viz_buffer.clone()),
            rg_mode,
            pre_amp_db,
            move || {
                finish_tx.send(PlayerCommand::DecodeFinished).ok();
            },
        )?;

        let (engine, rate_watch) = self.create_engine_for(&info, consumer)?;
        engine.start()?;

        self.shared_state.set_playback_state(PlaybackState::Playing);

        self.active_playback = Some(ActivePlayback {
            engine,
            decode_handle,
            _rate_watch: rate_watch,
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
        let result = self.open_streaming_playback(id, path, bytes_written, total);
        if result.is_err() {
            self.stop_playback_and_clear_state();
        }
        result
    }

    fn open_streaming_playback(
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
        // The playlist item's path points to the .part file during download, so the
        // pump opens the correct file. After download completes, the .part is renamed
        // to the final path and the item path is updated — but the pump's open FD
        // remains valid (Unix rename semantics).
        let pump_path = path.to_path_buf();
        let pump_buf = stream_buf.clone();
        let pump_written = bytes_written.clone();
        let pump_state = self.shared_state.clone();
        thread::Builder::new()
            .name("koan-stream-pump".into())
            .spawn(move || {
                use std::fs::File;
                use std::io::Read;
                use std::time::{Duration, Instant};

                /// No new bytes for this long and the download is treated as dead.
                /// `bytes_written` simply stops advancing when one dies, so without
                /// a deadline the pump spins and the decode thread parks forever.
                const STALL_LIMIT: Duration = Duration::from_secs(30);

                let mut file = match File::open(&pump_path) {
                    Ok(f) => f,
                    Err(e) => {
                        log::error!("stream pump: failed to open {}: {}", pump_path.display(), e);
                        pump_buf.fail();
                        return;
                    }
                };
                let mut buf = vec![0u8; 65536];
                let mut offset: u64 = 0;
                let mut last_progress = Instant::now();
                loop {
                    // Nothing left to read the bytes, so nothing left to write them for.
                    if pump_buf.is_abandoned() {
                        return;
                    }
                    match pump_state.item_load_state(id) {
                        Some(LoadState::Failed(e)) => {
                            log::warn!("stream pump: download of {:?} failed: {}", id, e);
                            pump_buf.fail();
                            return;
                        }
                        // The download landed: drain to EOF rather than trusting
                        // `total`, which is 0 for a chunked transfer.
                        Some(LoadState::Ready) => match file.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                pump_buf.push(&buf[..n]);
                                offset += n as u64;
                                continue;
                            }
                            Err(e) => {
                                log::warn!("stream pump read error: {}", e);
                                pump_buf.fail();
                                return;
                            }
                        },
                        _ => {}
                    }

                    let available = pump_written.load(Ordering::Acquire);
                    if offset >= available {
                        if total > 0 && available >= total {
                            break; // Download complete.
                        }
                        if last_progress.elapsed() >= STALL_LIMIT {
                            log::warn!(
                                "stream pump: no data for {}s, abandoning {}",
                                STALL_LIMIT.as_secs(),
                                pump_path.display()
                            );
                            pump_buf.fail();
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    let to_read = ((available - offset) as usize).min(buf.len());
                    match file.read(&mut buf[..to_read]) {
                        Ok(0) => {
                            // File data may lag behind bytes_written (OS buffer flush timing).
                            // Only treat as true EOF if we've pumped all expected data.
                            if total > 0 && offset >= total {
                                break;
                            }
                            let latest = pump_written.load(Ordering::Acquire);
                            if total > 0 && latest >= total && offset >= latest {
                                break;
                            }
                            // Data not yet visible on disk — back off and retry.
                            thread::sleep(Duration::from_millis(1));
                            continue;
                        }
                        Ok(n) => {
                            pump_buf.push(&buf[..n]);
                            offset += n as u64;
                            last_progress = Instant::now();
                        }
                        Err(e) => {
                            log::warn!("stream pump read error: {}", e);
                            pump_buf.fail();
                            return;
                        }
                    }
                }
                pump_buf.finish();
            })
            .map_err(|e| PlayerError::Decode(buffer::DecodeError::Io(e)))?;

        // Probe via a streaming reader — blocks (via condvar) until enough header data arrives.
        let probe_reader = stream_buf.reader();
        let probe_hint = {
            let mut h = symphonia::core::formats::probe::Hint::new();
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
            bitrate_kbps: info.bitrate_kbps,
            channels: info.channels,
            duration_ms: info.duration_ms,
        }));
        self.shared_state.set_position_ms(0);
        self.on_track_changed(id);
        log::info!(
            "streaming: {} ({:?}) — {} {}Hz/{}ch, {}ms",
            path.display(),
            id,
            info.codec,
            info.sample_rate,
            info.channels,
            info.duration_ms,
        );

        let (producer, consumer) = rtrb::RingBuffer::new(RING_BUFFER_SIZE);

        self.timeline.reset();

        // Gapless lookahead after streaming: next track uses normal file path.
        let advance_state = self.shared_state.clone();
        let decode_cursor = parking_lot::Mutex::new(Some(id));
        let next_track = move || {
            let current = decode_cursor.lock().take()?;
            let next = advance_state.peek_next_ready_after(current);
            if let Some((next_id, _)) = &next {
                let mut guard = decode_cursor.lock();
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
        let mut decode_hint = symphonia::core::formats::probe::Hint::new();
        if !ext.is_empty() {
            decode_hint.with_extension(&ext);
        }
        let first = buffer::SourceEntry {
            id,
            path: path_buf,
            hint: decode_hint,
            make_mss: Box::new(move || {
                Ok(symphonia::core::io::MediaSourceStream::new(
                    Box::new(decode_reader),
                    Default::default(),
                ))
            }),
        };

        // Load ReplayGain config for this streaming session.
        let cfg = crate::config::Config::load_or_default();
        let rg_mode = cfg.playback.replaygain;
        let pre_amp_db = cfg.playback.pre_amp_db;

        let finish_tx = self.commands.tx.clone();
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
            rg_mode,
            pre_amp_db,
            move || {
                finish_tx.send(PlayerCommand::DecodeFinished).ok();
            },
        )?;

        let (engine, rate_watch) = self.create_engine_for(&info, consumer)?;
        engine.start()?;

        self.shared_state.set_playback_state(PlaybackState::Playing);

        self.active_playback = Some(ActivePlayback {
            engine,
            decode_handle,
            _rate_watch: rate_watch,
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
        let mut clamped = if duration > 0 {
            position_ms.min(duration.saturating_sub(500))
        } else {
            position_ms
        };

        // Clamp to downloaded portion if streaming to prevent seeking into
        // data that hasn't arrived yet.
        if let Some(dl_frac) = self.shared_state.current_download_fraction() {
            let max_ms = (dl_frac * duration as f64) as u64;
            if max_ms > 5_000 {
                clamped = clamped.min(max_ms - 5_000);
            }
        }

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
        match self.shared_state.advance_cursor_loadable() {
            Some(id) => self.play(id),
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
            if let Err(e) = playback.engine.stop() {
                log::error!("pause failed: {}", e);
                return;
            }
            self.shared_state.set_playback_state(PlaybackState::Paused);
        }
    }

    /// Resume playback.
    pub fn resume(&mut self) {
        if let Some(ref playback) = self.active_playback {
            if let Err(e) = playback.engine.start() {
                log::error!("resume failed: {}", e);
                return;
            }
            self.shared_state.set_playback_state(PlaybackState::Playing);
        }
    }

    /// Stop playback and clear playlist.
    pub fn stop(&mut self) {
        self.shared_state.clear_playlist();
        self.stop_playback_and_clear_state();
    }

    /// Stop the audio engine and decode thread without touching shared state.
    ///
    /// The engine is stopped synchronously (silence begins immediately), but
    /// the heavy teardown (decode thread join + AudioUnit dispose) is moved to
    /// a background thread so the player command loop never blocks — preventing
    /// UI freezes when CoreAudio or the decode thread is slow to shut down.
    fn stop_engine(&mut self) {
        let Some(playback) = self.active_playback.take() else {
            return;
        };
        let ActivePlayback {
            engine,
            mut decode_handle,
            _rate_watch,
        } = playback;

        // Stop audio output first, then get the decode thread gone *before* the
        // engine is dropped.
        //
        // The old order signalled the decode thread and dropped the engine
        // immediately, joining the thread afterwards on a background thread —
        // so the engine's teardown ran while the decode thread was still alive
        // and still writing into the ring buffer that the render callback
        // reads. Tearing CoreAudio down underneath a live producer is exactly
        // the shape of the end-of-queue crash (#89), and the overlap buys
        // nothing: `stop()` has already silenced the output.
        let _ = engine.stop();
        decode_handle.stop();
        drop(engine);
    }

    /// Full stop: tear down engine + clear all display state.
    fn stop_playback_and_clear_state(&mut self) {
        self.finish_play();
        self.stop_engine();
        self.timeline.reset();
        self.shared_state.set_playback_state(PlaybackState::Stopped);
        self.shared_state.set_position_ms(0);
        self.shared_state.set_track_info(None);
    }

    /// Remove a track from the playlist. If it was the cursor, resume at the
    /// track that followed it.
    ///
    /// `remove_item` clears the cursor, and an unset cursor means "start from the
    /// top" — so the successor is pinned down by parking the cursor on the removed
    /// track's predecessor first. `None` is correct only when it was the first item.
    pub fn remove_from_playlist(&mut self, id: QueueItemId) {
        let was_cursor = self.shared_state.is_cursor(id);
        let resume_after = was_cursor
            .then(|| self.shared_state.item_before(id))
            .flatten();
        self.shared_state.remove_item(id);
        if was_cursor {
            self.shared_state.set_cursor(resume_after);
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

                // Re-probe the complete file for accurate duration + stream info.
                // The initial probe was done on partial streaming data and may have
                // underestimated duration, causing premature seek clamping or wrong
                // progress bar display.
                if let Ok(stream_info) = buffer::probe_file(&path)
                    && let Some(current) = self.shared_state.track_info()
                    && current.id == id
                    && stream_info.duration_ms > current.duration_ms
                {
                    log::info!(
                        "track_ready: duration corrected {}ms → {}ms",
                        current.duration_ms,
                        stream_info.duration_ms
                    );
                    self.shared_state.set_track_info(Some(TrackInfo {
                        duration_ms: stream_info.duration_ms,
                        ..current
                    }));
                }

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
    /// The needle has moved to `id`. Close out the outgoing track and write
    /// the new one to history straight away, so history reads in play order
    /// even for a track that is skipped a moment later.
    ///
    /// A seek restarts playback of the same track, so identity is checked
    /// rather than closing unconditionally — otherwise scrubbing around a
    /// track would enter it into history once per seek.
    fn on_track_changed(&mut self, id: QueueItemId) {
        if self.in_flight.as_ref().is_some_and(|f| f.item == id) {
            return;
        }
        self.finish_play();
        let track_id = self.shared_state.item_db_id(id);
        self.in_flight = Some(InFlight::new(id, track_id));
        if let (Some(track_id), Some(recorder)) = (track_id, self.history.as_ref()) {
            recorder.record(PlayEvent::Started { track_id });
        }
    }

    /// Tell history how long the current track was heard for. Returns what was
    /// reported, which is how the tests see it.
    fn finish_play(&mut self) -> Option<PlayEvent> {
        let flight = self.in_flight.take()?;
        let event = PlayEvent::Finished {
            track_id: flight.track_id()?,
            listened_ms: flight.listened_ms(),
        };
        if let Some(recorder) = self.history.as_ref() {
            recorder.record(event);
        }
        Some(event)
    }

    pub fn update_playback_state(&mut self) {
        if self.active_playback.is_none() {
            return;
        }

        if let Some((id, path, info, position_ms)) = self.timeline.current_playback() {
            self.shared_state.set_position_ms(position_ms);

            // A gapless transition moves the needle without anything on this
            // thread having asked it to, so the play is banked from here.
            self.on_track_changed(id);
            if let Some(f) = self.in_flight.as_mut() {
                f.advance(position_ms);
            }

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
                    bitrate_kbps: info.bitrate_kbps,
                    channels: info.channels,
                    duration_ms: info.duration_ms,
                }));
                self.shared_state.set_cursor(Some(id));
            }
        }
    }

    /// A download the cursor is parked on will never land.
    ///
    /// `play()` leaves the cursor on an item that is not yet Ready and stops,
    /// waiting for `TrackReady`. When the download fails instead, that wait has
    /// no end — so walk on to the next item that can still load, or stop
    /// cleanly if there is none.
    pub fn track_failed(&mut self, id: QueueItemId) {
        if !self.shared_state.is_cursor(id) {
            return;
        }
        // Only a parked cursor is waiting on this. Playing means it is being
        // streamed from the partial file — the pump sees the failure and ends
        // the decode, which advances the queue — and paused is the user's.
        if self.shared_state.playback_state() != PlaybackState::Stopped {
            return;
        }
        log::info!("track {:?} cannot load, moving on", id);
        self.next_track();
    }

    /// Decode thread naturally finished (playlist exhausted or error).
    /// Advance to the next playable track; otherwise stop cleanly.
    ///
    /// A track that has not finished downloading parks the cursor on it, so its
    /// `TrackReady`/`TrackStreamReady` resumes the queue instead of being
    /// discarded as "not the cursor".
    fn on_decode_finished(&mut self) {
        log::info!("decode finished, checking for next track");
        match self.shared_state.advance_cursor_loadable() {
            Some(id) => self.play(id),
            None => {
                log::info!("no more tracks — stopping");
                self.stop_playback_and_clear_state();
            }
        }
    }

    /// Snapshot items with their predecessors for an undo of "these were removed".
    /// In playlist order, so undo re-inserts each item after a predecessor that
    /// is already back in place.
    fn snapshot_for_undo(
        &self,
        ids: &[QueueItemId],
    ) -> Vec<(Box<state::PlaylistItem>, Option<QueueItemId>)> {
        self.shared_state
            .items_before(ids)
            .into_iter()
            .filter_map(|(id, after)| Some((Box::new(self.shared_state.get_item(id)?), after)))
            .collect()
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
            PlayerCommand::NextTrack => {
                // Debounce: suppress key repeat from terminal (150ms window).
                let now = std::time::Instant::now();
                if now.duration_since(self.last_skip).as_millis() >= 150 {
                    self.last_skip = now;
                    self.next_track();
                }
            }
            PlayerCommand::PrevTrack => {
                let now = std::time::Instant::now();
                if now.duration_since(self.last_skip).as_millis() >= 150 {
                    self.last_skip = now;
                    self.prev_track();
                }
            }
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
                // Stop engine + clear display state WITHOUT touching the playlist,
                // then snapshot, then clear. This avoids the race where stop()
                // would clear the playlist before we capture it for undo.
                self.stop_playback_and_clear_state();
                let (items, cursor) = self.shared_state.snapshot_playlist();
                self.shared_state.clear_playlist();
                self.push_undo(UndoEntry::Replaced { items, cursor });
            }
            PlayerCommand::ReplacePlaylist { items, start } => {
                // Same order as ClearPlaylist: stop and clear display state
                // before snapshotting, or the snapshot captures an already
                // emptied playlist and undo restores nothing.
                self.stop_playback_and_clear_state();
                let (old_items, cursor) = self.shared_state.snapshot_playlist();
                self.shared_state.clear_playlist();
                self.push_undo(UndoEntry::Replaced {
                    items: old_items,
                    cursor,
                });

                if items.is_empty() {
                    return;
                }
                let start_id = items.get(start).unwrap_or(&items[0]).id;
                self.shared_state.add_items(items);
                self.play(start_id);
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
                // Snapshot before removing anything, and resolve the resume point
                // once: removing one at a time would restart the engine for every
                // deleted track that the cursor lands on along the way.
                let items_with_pos = self.snapshot_for_undo(&ids);
                let resume_after = match self.shared_state.cursor() {
                    Some(cursor) if ids.contains(&cursor) => {
                        Some(self.shared_state.surviving_item_before(cursor, &ids))
                    }
                    _ => None,
                };

                self.shared_state.remove_items(&ids);

                if let Some(resume_after) = resume_after {
                    self.shared_state.set_cursor(resume_after);
                    self.next_track();
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
            PlayerCommand::DecodeFinished => self.on_decode_finished(),
            PlayerCommand::TrackStreamReady(id) => self.track_stream_ready(id),
            PlayerCommand::TrackFailed(id) => self.track_failed(id),
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
            PlayerCommand::SetOutputDevice(name) => self.set_output_device(name),
            PlayerCommand::ClearOutputDevice => self.clear_output_device(),
        }
    }

    /// Apply an undo/redo entry: mutate the playlist and return the inverse entry.
    fn apply_entry(&mut self, entry: UndoEntry) -> Option<UndoEntry> {
        match entry {
            UndoEntry::Added { ids } => {
                // Undo of "items were added": snapshot them with positions, then remove.
                let items_with_pos = self.snapshot_for_undo(&ids);
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
                let items_with_pos = self.snapshot_for_undo(&ids);
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
        player.history = PlayRecorder::spawn();
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
            db_id: None,
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

    fn pending_item(title: &str) -> PlaylistItem {
        PlaylistItem {
            load_state: LoadState::Pending,
            ..make_item(title)
        }
    }

    /// Build `n` ready items, add them, and return their IDs.
    fn seed(player: &mut Player, n: usize) -> Vec<QueueItemId> {
        let items: Vec<_> = (0..n).map(|i| make_item(&format!("t{i}"))).collect();
        let ids = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));
        ids
    }

    // --- cursor transitions ---

    /// Feed the player a track's worth of playback ticks, as the 50ms poll would.
    fn listen(player: &mut Player, from_ms: u64, to_ms: u64) {
        let mut at = from_ms;
        if let Some(f) = player.in_flight.as_mut() {
            f.advance(at); // the position the needle landed on
        }
        while at < to_ms {
            at = (at + 50).min(to_ms);
            if let Some(f) = player.in_flight.as_mut() {
                f.advance(at);
            }
        }
    }

    fn start(player: &mut Player, track_id: i64) -> QueueItemId {
        let id = QueueItemId::new();
        player.on_track_changed(id);
        // The item is not in a playlist here, so there is no db_id to find.
        player
            .in_flight
            .as_mut()
            .unwrap()
            .track_id_for_test(track_id);
        id
    }

    #[test]
    fn a_gapless_transition_closes_the_outgoing_track_and_opens_the_next() {
        let mut player = Player::new();
        start(&mut player, 11);
        listen(&mut player, 0, 200_000);

        let b = QueueItemId::new();
        player.on_track_changed(b);
        let f = player
            .in_flight
            .as_ref()
            .expect("the next track is counting");
        assert_eq!(f.item, b);
        assert_eq!(f.listened_ms(), 0, "and starts from nothing");
    }

    #[test]
    fn a_track_skipped_seconds_in_is_still_history() {
        let mut player = Player::new();
        start(&mut player, 7);
        listen(&mut player, 0, 2_000);

        let event = player
            .finish_play()
            .expect("putting something on is a thing you did, however briefly");
        assert!(matches!(
            event,
            history::PlayEvent::Finished {
                track_id: 7,
                listened_ms: 2_000
            }
        ));
    }

    #[test]
    fn a_track_is_closed_out_once() {
        let mut player = Player::new();
        start(&mut player, 7);
        listen(&mut player, 0, 200_000);

        assert!(player.finish_play().is_some());
        assert!(player.finish_play().is_none());
    }

    #[test]
    fn seeking_around_a_track_does_not_enter_it_twice() {
        let mut player = Player::new();
        let id = start(&mut player, 7);
        listen(&mut player, 0, 120_000);

        // A seek restarts playback of the same item.
        player.on_track_changed(id);
        assert_eq!(
            player.in_flight.as_ref().unwrap().listened_ms(),
            120_000,
            "the seek kept the count rather than restarting it"
        );
        listen(&mut player, 30_000, 40_000);

        let Some(history::PlayEvent::Finished { listened_ms, .. }) = player.finish_play() else {
            panic!("still one play");
        };
        assert_eq!(listened_ms, 130_000);
        assert!(player.finish_play().is_none());
    }

    #[test]
    fn a_track_that_is_not_in_the_library_is_not_recorded() {
        let mut player = Player::new();
        let id = QueueItemId::new();
        player.on_track_changed(id);
        listen(&mut player, 0, 200_000);
        assert!(player.finish_play().is_none());
    }

    #[test]
    fn stopping_closes_out_what_was_heard() {
        let mut player = Player::new();
        start(&mut player, 7);
        listen(&mut player, 0, 150_000);

        player.stop_playback_and_clear_state();
        assert!(player.in_flight.is_none(), "the stop consumed it");
    }

    #[test]
    fn removing_the_playing_track_resumes_at_its_successor() {
        let mut player = Player::new();
        let ids = seed(&mut player, 5);
        player.shared_state.set_cursor(Some(ids[2]));

        player.process_command(PlayerCommand::RemoveFromPlaylist(ids[2]));

        assert_eq!(
            player.shared_state.cursor(),
            Some(ids[3]),
            "playback must continue at the next track, not restart the queue"
        );
        assert_eq!(player.playback_starts, 1);
    }

    #[test]
    fn removing_the_first_playing_track_resumes_at_the_new_first() {
        let mut player = Player::new();
        let ids = seed(&mut player, 3);
        player.shared_state.set_cursor(Some(ids[0]));

        player.process_command(PlayerCommand::RemoveFromPlaylist(ids[0]));

        assert_eq!(player.shared_state.cursor(), Some(ids[1]));
    }

    #[test]
    fn next_track_parks_on_a_track_that_has_not_downloaded_yet() {
        let mut player = Player::new();
        let playing = make_item("playing");
        let waiting = pending_item("waiting");
        let later = make_item("later");
        let (playing_id, waiting_id) = (playing.id, waiting.id);
        player.process_command(PlayerCommand::AddToPlaylist(vec![playing, waiting, later]));
        player.shared_state.set_cursor(Some(playing_id));

        player.process_command(PlayerCommand::DecodeFinished);

        assert_eq!(
            player.shared_state.cursor(),
            Some(waiting_id),
            "the cursor parks on the track being fetched"
        );
        assert_eq!(
            player.playback_starts, 0,
            "nothing to play until its bytes land"
        );

        // The download completes. Because the cursor is parked here, the
        // TrackReady actually reaches the player and the queue resumes.
        player
            .shared_state
            .update_load_state(waiting_id, LoadState::Ready);
        player.process_command(PlayerCommand::TrackReady(waiting_id));

        assert_eq!(player.playback_starts, 1);
        assert_eq!(player.shared_state.cursor(), Some(waiting_id));
    }

    #[test]
    fn a_download_that_cannot_land_moves_the_cursor_on() {
        let mut player = Player::new();
        let waiting = pending_item("waiting");
        let later = make_item("later");
        let (waiting_id, later_id) = (waiting.id, later.id);
        player.process_command(PlayerCommand::AddToPlaylist(vec![waiting, later]));

        player.process_command(PlayerCommand::Play(waiting_id));
        assert_eq!(player.playback_starts, 0, "nothing to play yet");

        // The download gives up. Ready will never come.
        player
            .shared_state
            .update_load_state(waiting_id, LoadState::Failed("remote unavailable".into()));
        player.process_command(PlayerCommand::TrackFailed(waiting_id));

        assert_eq!(
            player.shared_state.cursor(),
            Some(later_id),
            "the queue moves past a track that can never load"
        );
        assert_eq!(player.playback_starts, 1);
    }

    #[test]
    fn a_queue_that_can_never_load_stops_rather_than_waiting() {
        let mut player = Player::new();
        let first = pending_item("first");
        let second = pending_item("second");
        let (first_id, second_id) = (first.id, second.id);
        player.process_command(PlayerCommand::AddToPlaylist(vec![first, second]));

        player.process_command(PlayerCommand::Play(first_id));
        for id in [first_id, second_id] {
            player
                .shared_state
                .update_load_state(id, LoadState::Failed("remote unavailable".into()));
            player.process_command(PlayerCommand::TrackFailed(id));
        }

        assert_eq!(player.playback_starts, 0);
        assert_eq!(
            player.shared_state.playback_state(),
            PlaybackState::Stopped,
            "a stop the UI can see, not an indefinite wait for TrackReady"
        );
    }

    #[test]
    fn a_failure_elsewhere_in_the_queue_leaves_the_cursor_alone() {
        let mut player = Player::new();
        let waiting = pending_item("waiting");
        let other = pending_item("other");
        let (waiting_id, other_id) = (waiting.id, other.id);
        player.process_command(PlayerCommand::AddToPlaylist(vec![waiting, other]));
        player.process_command(PlayerCommand::Play(waiting_id));

        player
            .shared_state
            .update_load_state(other_id, LoadState::Failed("remote unavailable".into()));
        player.process_command(PlayerCommand::TrackFailed(other_id));

        assert_eq!(
            player.shared_state.cursor(),
            Some(waiting_id),
            "a track still downloading keeps the cursor"
        );
    }

    #[test]
    fn batch_delete_containing_the_cursor_restarts_the_engine_once() {
        let mut player = Player::new();
        let ids = seed(&mut player, 5);
        player.shared_state.set_cursor(Some(ids[2]));

        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![
            ids[1], ids[2], ids[3],
        ]));

        assert_eq!(playlist_titles(&player), vec!["t0", "t4"]);
        assert_eq!(player.shared_state.cursor(), Some(ids[4]));
        assert_eq!(
            player.playback_starts, 1,
            "one resume for the whole selection, not one per deleted track"
        );
    }

    #[test]
    fn batch_delete_below_the_cursor_leaves_playback_alone() {
        let mut player = Player::new();
        let ids = seed(&mut player, 4);
        player.shared_state.set_cursor(Some(ids[0]));

        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![ids[2], ids[3]]));

        assert_eq!(player.shared_state.cursor(), Some(ids[0]));
        assert_eq!(player.playback_starts, 0);
    }

    #[test]
    fn undo_of_a_batch_delete_restores_the_original_order() {
        // The TUI collects a selection from a HashSet, so the IDs arrive in
        // arbitrary order — scrambled here so a snapshot that trusts that order
        // re-inserts C before B and lands it at the end of the playlist.
        let mut player = Player::new();
        let items = vec![
            make_item("A"),
            make_item("B"),
            make_item("C"),
            make_item("D"),
        ];
        let (b_id, c_id) = (items[1].id, items[2].id);
        player.process_command(PlayerCommand::AddToPlaylist(items));

        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![c_id, b_id]));
        assert_eq!(playlist_titles(&player), vec!["A", "D"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C", "D"]);
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
        let version_before = player.shared_state.playlist_version();
        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![b_id, c_id]));
        assert_eq!(playlist_titles(&player), vec!["A", "D"]);
        // One bump for the whole batch. Bumping per item is what made clearing
        // a large queue crawl, and every bump wakes every client watching.
        assert_eq!(
            player.shared_state.playlist_version(),
            version_before + 1,
            "batch removal must bump the playlist version exactly once"
        );

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

    /// Regression test for GitHub #89: AudioEngine must be dropped synchronously
    /// in stop_engine() before the caller changes sample rates. If the engine is
    /// dropped on a background thread, CoreAudio's internal buffer list can be
    /// freed while AudioUnitUninitialize is still tearing it down → crash.
    #[test]
    fn stop_engine_drops_engine_synchronously() {
        use std::sync::atomic::AtomicBool;

        struct MockEngine {
            dropped: Arc<AtomicBool>,
        }
        impl AudioEngineHandle for MockEngine {
            fn start(&self) -> Result<(), BackendError> {
                Ok(())
            }
            fn stop(&self) -> Result<(), BackendError> {
                Ok(())
            }
            fn is_running(&self) -> bool {
                false
            }
        }
        impl Drop for MockEngine {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));

        // Build a minimal decode handle that won't block.
        let stop_flag = Arc::new(AtomicBool::new(false));
        let decode_handle = buffer::DecodeHandle::new_for_test(stop_flag);

        let mut player = Player::new();
        player.active_playback = Some(ActivePlayback {
            engine: Box::new(MockEngine {
                dropped: dropped.clone(),
            }),
            decode_handle,
            _rate_watch: None,
        });

        player.stop_engine();

        // The engine must already be dropped when stop_engine returns.
        // If this fails, the engine was moved to a background thread — the
        // exact race condition that causes the #89 crash.
        assert!(
            dropped.load(Ordering::SeqCst),
            "AudioEngine must be dropped synchronously in stop_engine (GitHub #89)"
        );
    }

    // --- Engine format matches the decoded PCM ---

    /// Backend pinned to one sample rate that refuses every switch, recording
    /// the format the engine is asked for.
    struct StuckBackend {
        rate: f64,
        asked: Arc<std::sync::Mutex<Option<(f64, u32)>>>,
    }

    struct NullEngine;
    impl AudioEngineHandle for NullEngine {
        fn start(&self) -> Result<(), BackendError> {
            Ok(())
        }
        fn stop(&self) -> Result<(), BackendError> {
            Ok(())
        }
        fn is_running(&self) -> bool {
            false
        }
    }

    impl AudioBackend for StuckBackend {
        fn list_devices(&self) -> Result<Vec<backend::DeviceInfo>, BackendError> {
            Ok(vec![self.default_device()?])
        }
        fn default_device(&self) -> Result<backend::DeviceInfo, BackendError> {
            Ok(backend::DeviceInfo {
                name: "Stuck DAC".into(),
                sample_rates: vec![self.rate],
                platform_id: 0,
            })
        }
        fn supported_sample_rates(
            &self,
            _device: &backend::DeviceInfo,
        ) -> Result<Vec<f64>, BackendError> {
            Ok(vec![self.rate])
        }
        fn get_device_sample_rate(
            &self,
            _device: &backend::DeviceInfo,
        ) -> Result<f64, BackendError> {
            Ok(self.rate)
        }
        fn set_device_sample_rate(
            &self,
            _device: &backend::DeviceInfo,
            rate: f64,
        ) -> Result<f64, BackendError> {
            Err(BackendError::UnsupportedSampleRate(rate))
        }
        fn create_engine(
            &self,
            _device: &backend::DeviceInfo,
            sample_rate: f64,
            channels: u32,
            _consumer: rtrb::Consumer<f32>,
            _samples_played: Arc<AtomicU64>,
        ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
            *self.asked.lock().unwrap() = Some((sample_rate, channels));
            Ok(Box::new(NullEngine))
        }
    }

    fn engine_format_for(source_rate: u32, channels: u16, device_rate: f64) -> (f64, u32) {
        let asked = Arc::new(std::sync::Mutex::new(None));
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: device_rate,
            asked: asked.clone(),
        });

        let info = buffer::StreamInfo {
            codec: "MP3".into(),
            sample_rate: source_rate,
            channels,
            bit_depth: Some(16),
            bitrate_kbps: None,
            duration_ms: 1000,
        };
        let (_producer, consumer) = rtrb::RingBuffer::new(16);
        player
            .create_engine_for(&info, consumer)
            .expect("engine creation should succeed");
        let asked = *asked.lock().unwrap();
        asked.expect("engine was never created")
    }

    #[test]
    fn engine_uses_source_rate_when_device_refuses_switch() {
        // MPEG-2 MP3 rates are routinely rejected by output devices. The engine
        // must still be told the rate the PCM actually is.
        assert_eq!(engine_format_for(22050, 2, 48000.0), (22050.0, 2));
        assert_eq!(engine_format_for(32000, 2, 44100.0), (32000.0, 2));
    }

    #[test]
    fn engine_uses_source_channel_count() {
        assert_eq!(engine_format_for(44100, 1, 44100.0), (44100.0, 1));
    }

    /// The rate the device settled at, as the front ends read it.
    fn settled_rate_for(source_rate: u32, device_rate: f64) -> Option<u32> {
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: device_rate,
            asked: Arc::new(std::sync::Mutex::new(None)),
        });
        let state = player.shared_state.clone();

        let info = buffer::StreamInfo {
            codec: "MP3".into(),
            sample_rate: source_rate,
            channels: 2,
            bit_depth: Some(16),
            bitrate_kbps: None,
            duration_ms: 1000,
        };
        let (_producer, consumer) = rtrb::RingBuffer::new(16);
        player
            .create_engine_for(&info, consumer)
            .expect("engine creation should succeed");
        state.output_sample_rate()
    }

    #[test]
    fn settled_device_rate_reaches_the_shared_state() {
        // A device that refuses the switch is being fed resampled audio, and
        // that is the case the front ends have to be able to see. Before this
        // the comparison happened once, in a log line.
        assert_eq!(settled_rate_for(22050, 48000.0), Some(48000));
        // No switch needed, so nothing resampled: the two rates agree.
        assert_eq!(settled_rate_for(44100, 44100.0), Some(44100));
    }

    /// Backend that hands its rate-change callback back to the test.
    struct WatchedBackend {
        inner: StuckBackend,
        #[allow(clippy::type_complexity)]
        captured: Arc<std::sync::Mutex<Option<Box<dyn Fn(f64) + Send + Sync>>>>,
    }

    struct NullWatch;
    impl backend::SampleRateWatch for NullWatch {}

    impl AudioBackend for WatchedBackend {
        fn list_devices(&self) -> Result<Vec<backend::DeviceInfo>, BackendError> {
            self.inner.list_devices()
        }
        fn default_device(&self) -> Result<backend::DeviceInfo, BackendError> {
            self.inner.default_device()
        }
        fn supported_sample_rates(
            &self,
            device: &backend::DeviceInfo,
        ) -> Result<Vec<f64>, BackendError> {
            self.inner.supported_sample_rates(device)
        }
        fn get_device_sample_rate(
            &self,
            device: &backend::DeviceInfo,
        ) -> Result<f64, BackendError> {
            self.inner.get_device_sample_rate(device)
        }
        fn set_device_sample_rate(
            &self,
            device: &backend::DeviceInfo,
            rate: f64,
        ) -> Result<f64, BackendError> {
            self.inner.set_device_sample_rate(device, rate)
        }
        fn watch_device_sample_rate(
            &self,
            _device: &backend::DeviceInfo,
            on_change: Box<dyn Fn(f64) + Send + Sync>,
        ) -> Option<Box<dyn backend::SampleRateWatch>> {
            *self.captured.lock().unwrap() = Some(on_change);
            Some(Box::new(NullWatch))
        }
        fn create_engine(
            &self,
            device: &backend::DeviceInfo,
            sample_rate: f64,
            channels: u32,
            consumer: rtrb::Consumer<f32>,
            samples_played: Arc<AtomicU64>,
        ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
            self.inner
                .create_engine(device, sample_rate, channels, consumer, samples_played)
        }
    }

    #[test]
    fn external_rate_change_reaches_the_shared_state() {
        // The device is shared. Another client moving the rate mid-track used
        // to leave the front ends asserting bit-perfection while the HAL
        // resampled underneath them.
        let captured = Arc::new(std::sync::Mutex::new(None));
        let mut player = Player::new();
        player.backend = Box::new(WatchedBackend {
            inner: StuckBackend {
                rate: 44100.0,
                asked: Arc::new(std::sync::Mutex::new(None)),
            },
            captured: captured.clone(),
        });
        let state = player.shared_state.clone();

        let info = buffer::StreamInfo {
            codec: "FLAC".into(),
            sample_rate: 44100,
            channels: 2,
            bit_depth: Some(16),
            bitrate_kbps: None,
            duration_ms: 1000,
        };
        let (_producer, consumer) = rtrb::RingBuffer::new(16);
        player
            .create_engine_for(&info, consumer)
            .expect("engine creation should succeed");
        assert_eq!(state.output_sample_rate(), Some(44100));

        let on_change = captured.lock().unwrap().take().expect("watch registered");
        on_change(48000.0);
        assert_eq!(state.output_sample_rate(), Some(48000));
    }
}
