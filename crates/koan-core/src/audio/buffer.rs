use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_PCM_F32LE,
    CODEC_ID_PCM_S16LE, CODEC_ID_PCM_S24LE, CODEC_ID_PCM_S32LE, CODEC_ID_VORBIS, CODEC_ID_WAVPACK,
};
use symphonia::core::codecs::audio::{AudioCodecId, AudioCodecParameters, AudioDecoderOptions};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, Track, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Duration, Time, TimeBase};
use thiserror::Error;

use crate::audio::opus::OpusBridge;
use crate::audio::viz::VizBuffer;
use crate::config::ReplayGainMode;
use crate::player::state::QueueItemId;

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("failed to open file: {0}")]
    Io(#[from] std::io::Error),
    #[error("no supported audio track found")]
    NoTrack,
    #[error("unsupported codec")]
    UnsupportedCodec,
    #[error("decode error: {0}")]
    Decode(String),
}

/// Info about the decoded audio stream, extracted before decoding starts.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_depth: Option<u16>,
    /// Bitrate in kbps. Meaningful for lossy codecs, None for lossless.
    pub bitrate_kbps: Option<u32>,
    pub duration_ms: u64,
}

/// Handle to a running decode thread. Drop to stop it.
pub struct DecodeHandle {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl DecodeHandle {
    /// Signal the decode thread to stop without waiting for it to exit.
    pub fn signal_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Create a DecodeHandle with no real thread (for tests only).
    #[cfg(test)]
    pub fn new_for_test(stop: Arc<AtomicBool>) -> Self {
        Self { stop, thread: None }
    }

    /// Signal the decode thread to stop and wait for it.
    pub fn stop(&mut self) {
        self.signal_stop();
        if let Some(handle) = self.thread.take()
            && let Err(payload) = handle.join()
        {
            let msg = payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown");
            log::error!("decode thread panicked: {}", msg);
        }
    }
}

impl Drop for DecodeHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

// --- Playback timeline: the source of truth for "what's playing" ---

/// A track boundary in the playback stream. At `sample_offset` cumulative
/// samples written to the ring buffer, this track starts.
#[derive(Debug, Clone)]
pub struct TrackBoundary {
    pub id: QueueItemId,
    pub path: PathBuf,
    pub info: StreamInfo,
    /// Cumulative interleaved samples written to the ring buffer when this
    /// track's first sample was pushed. For the first track this is 0
    /// (or seek_samples if seeking).
    pub sample_offset: u64,
    /// Samples of this track's audio written to ring buffer so far.
    /// Updated as decode progresses. At EOF, equals total decoded samples.
    pub samples_written: u64,
    /// The seek offset in samples for this track (non-zero only if user seeked).
    pub seek_samples: u64,
}

/// Shared timeline that the decode thread writes and the UI reads.
/// The decode thread appends boundaries; the UI reads them + samples_played
/// to derive current track and position.
pub struct PlaybackTimeline {
    boundaries: parking_lot::RwLock<Vec<TrackBoundary>>,
    /// Total interleaved samples written to the ring buffer across all tracks.
    samples_written: AtomicU64,
    /// Total interleaved samples consumed (played) by the audio engine.
    /// Written by CoreAudio render callback, read by UI.
    pub samples_played: Arc<AtomicU64>,
}

impl PlaybackTimeline {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            boundaries: parking_lot::RwLock::new(Vec::new()),
            samples_written: AtomicU64::new(0),
            samples_played: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Called by decode thread when starting a new track.
    fn push_boundary(&self, boundary: TrackBoundary) {
        self.boundaries.write().push(boundary);
    }

    /// Called by decode thread after pushing samples.
    fn add_written(&self, count: u64) {
        self.samples_written.fetch_add(count, Ordering::Relaxed);
        // Also update the last boundary's samples_written.
        let mut bounds = self.boundaries.write();
        if let Some(last) = bounds.last_mut() {
            last.samples_written += count;
        }
    }

    /// Reset for a new playback session.
    pub fn reset(&self) {
        self.boundaries.write().clear();
        self.samples_written.store(0, Ordering::Relaxed);
        self.samples_played.store(0, Ordering::Relaxed);
    }

    /// Get a clone of the samples_played Arc for the audio engine.
    pub fn samples_played_counter(&self) -> Arc<AtomicU64> {
        self.samples_played.clone()
    }

    /// Derive current track info and position from the playback head.
    /// Called by the UI on every tick.
    /// Returns (id, path, stream_info, position_ms).
    ///
    /// Acquires the boundaries read lock BEFORE reading `samples_played` so
    /// channels/sample_rate/boundaries are all from a consistent snapshot.
    /// Without this ordering, a track transition could update the atomics
    /// after we read `samples_played` but before we read the boundary list.
    pub fn current_playback(&self) -> Option<(QueueItemId, PathBuf, StreamInfo, u64)> {
        // Lock first — ensures we see boundaries consistent with the atomic read.
        let bounds = self.boundaries.read();

        if bounds.is_empty() {
            return None;
        }

        // Read samples_played while holding the lock. This guarantees we
        // never observe a stale boundary list with a newer samples_played
        // (or vice versa).
        let played = self.samples_played.load(Ordering::Acquire);

        // Find which track the playback head is in via binary search.
        // partition_point returns first index where offset > played;
        // the track we want is one before that.
        let idx = bounds.partition_point(|b| b.sample_offset <= played);
        let current = if idx > 0 {
            &bounds[idx - 1]
        } else {
            return None;
        };

        let ch = current.info.channels as u64;
        let rate = current.info.sample_rate as u64;
        if ch == 0 || rate == 0 {
            return None;
        }

        // Position within this track: (played - track_start) converted to ms.
        // Add seek offset since that's where playback started within the track.
        let track_samples = played.saturating_sub(current.sample_offset);
        let position_ms =
            (track_samples / ch) * 1000 / rate + (current.seek_samples / ch) * 1000 / rate;

        Some((
            current.id,
            current.path.clone(),
            current.info.clone(),
            position_ms,
        ))
    }
}

// ---------------------------------------------------------------------------
// Source abstraction
// ---------------------------------------------------------------------------

/// A source entry for the generic decode queue.
///
/// Each entry provides an ID, a display path (for logging/timeline),
/// a format hint, and a factory that constructs a fresh `MediaSourceStream`.
pub struct SourceEntry {
    pub id: QueueItemId,
    /// Path used for logging and `TrackBoundary`. Need not be a real FS path.
    pub path: PathBuf,
    /// Format hint for Symphonia (e.g. file extension).
    pub hint: Hint,
    /// Factory that creates the `MediaSourceStream`. Called exactly once per track.
    pub make_mss: Box<dyn FnOnce() -> std::io::Result<MediaSourceStream<'static>> + Send>,
}

impl SourceEntry {
    /// Convenience: build a `SourceEntry` from a local file path.
    pub fn from_file(id: QueueItemId, path: PathBuf) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        let path_clone = path.clone();
        let mut hint = Hint::new();
        if !ext.is_empty() {
            hint.with_extension(&ext);
        }
        Self {
            id,
            path,
            hint,
            make_mss: Box::new(move || {
                let file = File::open(&path_clone)?;
                Ok(MediaSourceStream::new(Box::new(file), Default::default()))
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Probe API
// ---------------------------------------------------------------------------

/// Probe a `MediaSourceStream` (with hint) and return stream info without decoding.
pub fn probe_source(mss: MediaSourceStream<'_>, hint: &Hint) -> Result<StreamInfo, DecodeError> {
    probe_mss(mss, hint)
}

/// Probe a file and return stream info without decoding.
pub fn probe_file(path: &Path) -> Result<StreamInfo, DecodeError> {
    let file_size = std::fs::metadata(path).ok().map(|m| m.len());
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut info = probe_mss(mss, &hint)?;
    // For Opus (and other lossy codecs where symphonia couldn't give us a
    // bitrate), estimate from file size / duration when both are available.
    if info.bitrate_kbps.is_none()
        && info.bit_depth.is_none()
        && let Some(size) = file_size
        && info.duration_ms > 0
    {
        info.bitrate_kbps = Some((size * 8 / info.duration_ms) as u32);
    }
    Ok(info)
}

/// Internal: probe a `MediaSourceStream` with a hint.
fn probe_mss(mss: MediaSourceStream<'_>, hint: &Hint) -> Result<StreamInfo, DecodeError> {
    let reader = symphonia::default::get_probe()
        .probe(
            hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| DecodeError::Decode(e.to_string()))?;

    let track = reader
        .default_track(TrackType::Audio)
        .ok_or(DecodeError::NoTrack)?;
    let codec_params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or(DecodeError::NoTrack)?;
    let is_opus = codec_params.codec == CODEC_ID_OPUS;
    // Opus always decodes to 48 kHz regardless of the input sample rate.
    let sample_rate = if is_opus {
        48000
    } else {
        codec_params.sample_rate.unwrap_or(44100)
    };
    let channels = codec_params
        .channels
        .as_ref()
        .map(|c| c.count() as u16)
        .unwrap_or(2);
    let bit_depth = if is_opus {
        None
    } else {
        Some(codec_params.bits_per_sample.unwrap_or(16) as u16)
    };
    let duration_ms = track_duration_ms(&*reader, track, sample_rate);
    let codec = codec_name(codec_params.codec);

    // Symphonia doesn't expose bitrate directly. For lossy codecs we can
    // estimate from bits_per_coded_sample when the demuxer provides it.
    // Opus estimation from file size is handled in probe_file() where we
    // have the path; here we only have a MediaSourceStream.
    let bitrate_kbps = estimate_bitrate_from_codec_params(codec_params);

    Ok(StreamInfo {
        codec,
        sample_rate,
        channels,
        bit_depth,
        bitrate_kbps,
        duration_ms,
    })
}

// ---------------------------------------------------------------------------
// Generic decode API (SourceEntry-based)
// ---------------------------------------------------------------------------

/// Start decoding from a `SourceEntry` into the ring buffer.
///
/// `first`      — the first track's source entry.
/// `seek_ms`    — if > 0, seek to this position before decoding the first track.
/// `next_track` — closure returning the next `SourceEntry` for gapless playback.
///                Called on EOF. Returns None when the playlist is exhausted.
#[allow(clippy::too_many_arguments)]
pub fn start_decode<N, F>(
    first: SourceEntry,
    producer: rtrb::Producer<f32>,
    seek_ms: u64,
    next_track: N,
    timeline: Arc<PlaybackTimeline>,
    viz_buffer: Option<Arc<VizBuffer>>,
    rg_mode: ReplayGainMode,
    pre_amp_db: f64,
    on_finished: F,
) -> Result<(StreamInfo, DecodeHandle), DecodeError>
where
    N: Fn() -> Option<SourceEntry> + Send + 'static,
    F: FnOnce() + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let thread = thread::Builder::new()
        .name("koan-decode".into())
        .spawn(move || {
            decode_queue_loop(
                first,
                producer,
                &stop_clone,
                seek_ms,
                &next_track,
                &timeline,
                viz_buffer.as_deref(),
                rg_mode,
                pre_amp_db,
            );
            // Notify the player that the decode loop finished (playlist
            // exhausted or error). Only fire if we weren't explicitly stopped
            // (i.e. this is a natural end, not a seek/skip teardown).
            if !stop_clone.load(Ordering::Relaxed) {
                on_finished();
            }
        })
        .map_err(DecodeError::Io)?;

    // Return a placeholder StreamInfo — the real info is pushed to the timeline
    // by the decode thread immediately after probing the source.
    let placeholder = StreamInfo {
        codec: String::from("?"),
        sample_rate: 44100,
        channels: 2,
        bit_depth: Some(16),
        bitrate_kbps: None,
        duration_ms: 0,
    };

    Ok((
        placeholder,
        DecodeHandle {
            stop,
            thread: Some(thread),
        },
    ))
}

// ---------------------------------------------------------------------------
// File-based convenience wrapper
// ---------------------------------------------------------------------------

/// Start decoding a file into the ring buffer (convenience wrapper).
///
/// `initial_id` — the QueueItemId of the first track.
/// `seek_ms` — if > 0, seek to this position before decoding the first track.
/// `next_track` — closure returning the next (id, path) for gapless playback.
#[allow(clippy::too_many_arguments)]
pub fn start_decode_file<N, F>(
    initial_id: QueueItemId,
    path: &Path,
    producer: rtrb::Producer<f32>,
    seek_ms: u64,
    next_track: N,
    timeline: Arc<PlaybackTimeline>,
    viz_buffer: Option<Arc<VizBuffer>>,
    rg_mode: ReplayGainMode,
    pre_amp_db: f64,
    on_finished: F,
) -> Result<(StreamInfo, DecodeHandle), DecodeError>
where
    N: Fn() -> Option<(QueueItemId, PathBuf)> + Send + 'static,
    F: FnOnce() + Send + 'static,
{
    let info = probe_file(path)?;
    let first = SourceEntry::from_file(initial_id, path.to_path_buf());
    let (_, handle) = start_decode(
        first,
        producer,
        seek_ms,
        move || {
            let (id, p) = next_track()?;
            Some(SourceEntry::from_file(id, p))
        },
        timeline,
        viz_buffer,
        rg_mode,
        pre_amp_db,
        on_finished,
    )?;
    Ok((info, handle))
}

// ---------------------------------------------------------------------------
// Internal decode loop
// ---------------------------------------------------------------------------

/// Gapless decode loop: decode first entry, then call next_track on EOF.
///
/// Every track in a session shares one ring buffer, and therefore the audio
/// engine configured for it. A track whose PCM format differs from the first
/// ends the session rather than being written at the wrong format; the player
/// restarts it on a correctly configured engine.
#[allow(clippy::too_many_arguments)]
fn decode_queue_loop<N>(
    first: SourceEntry,
    mut producer: rtrb::Producer<f32>,
    stop: &AtomicBool,
    initial_seek_ms: u64,
    next_track: &N,
    timeline: &PlaybackTimeline,
    viz_buffer: Option<&VizBuffer>,
    rg_mode: ReplayGainMode,
    pre_amp_db: f64,
) where
    N: Fn() -> Option<SourceEntry>,
{
    'session: {
        let path = first.path.clone();
        let hint = first.hint.clone();
        let mss = match (first.make_mss)() {
            Ok(mss) => mss,
            Err(e) => {
                if !stop.load(Ordering::Relaxed) {
                    log::error!("failed to open {}: {}", path.display(), e);
                }
                break 'session;
            }
        };

        let format = match decode_single(
            first.id,
            &path,
            &hint,
            mss,
            &mut producer,
            stop,
            initial_seek_ms,
            timeline,
            viz_buffer,
            rg_mode,
            pre_amp_db,
            None,
        ) {
            Ok(Decoded::Complete(format)) => format,
            Ok(Decoded::FormatMismatch) => break 'session,
            Err(e) => {
                if !stop.load(Ordering::Relaxed) {
                    log::error!("decode error on {}: {}", path.display(), e);
                }
                break 'session;
            }
        };

        while !stop.load(Ordering::Relaxed) {
            let Some(entry) = (next_track)() else {
                log::info!("playlist exhausted, decode thread finishing");
                break;
            };

            log::info!("gapless transition → {}", entry.path.display());
            let next_path = entry.path.clone();
            let next_hint = entry.hint.clone();
            let next_mss = match (entry.make_mss)() {
                Ok(mss) => mss,
                Err(e) => {
                    if !stop.load(Ordering::Relaxed) {
                        log::error!("failed to open {}: {}", next_path.display(), e);
                    }
                    break;
                }
            };

            match decode_single(
                entry.id,
                &next_path,
                &next_hint,
                next_mss,
                &mut producer,
                stop,
                0,
                timeline,
                viz_buffer,
                rg_mode,
                pre_amp_db,
                Some(format),
            ) {
                Ok(Decoded::Complete(_)) => {}
                Ok(Decoded::FormatMismatch) => break,
                Err(e) => {
                    if !stop.load(Ordering::Relaxed) {
                        log::error!("decode error on {}: {}", next_path.display(), e);
                    }
                    break;
                }
            }
        }
    }

    wait_for_drain(&producer, stop);
}

/// Block until the audio engine has consumed everything in the ring buffer.
///
/// A session ends only once its audio has been heard, so the player can tear
/// the engine down without clipping the tail of the last track decoded.
/// Returns early if playback is torn down underneath us.
fn wait_for_drain(producer: &rtrb::Producer<f32>, stop: &AtomicBool) {
    let capacity = producer.buffer().capacity();
    while !stop.load(Ordering::Relaxed) && !producer.is_abandoned() {
        if producer.slots() >= capacity {
            return;
        }
        thread::sleep(std::time::Duration::from_millis(2));
    }
}

// ---------------------------------------------------------------------------
// Core decode single track
// ---------------------------------------------------------------------------

/// The PCM format of a decoded stream: sample rate in Hz and channel count.
/// The audio engine is configured from this, so the ring buffer may only ever
/// hold samples of one such format at a time.
type PcmFormat = (u32, u16);

/// Outcome of decoding one source.
enum Decoded {
    /// Decoded to EOF, in the given format.
    Complete(PcmFormat),
    /// The source's format differs from the stream already in the ring buffer.
    /// Nothing further was written — the engine must be reconfigured first.
    FormatMismatch,
}

/// Decode a single source into the producer. Returns on clean EOF.
///
/// `expected` — the format already in the ring buffer, if any. A source that
/// does not match it is rejected without writing samples or pushing a boundary.
#[allow(clippy::too_many_arguments)]
fn decode_single(
    queue_item_id: QueueItemId,
    path: &Path,
    hint: &Hint,
    mss: MediaSourceStream<'_>,
    producer: &mut rtrb::Producer<f32>,
    stop: &AtomicBool,
    seek_ms: u64,
    timeline: &PlaybackTimeline,
    viz_buffer: Option<&VizBuffer>,
    rg_mode: ReplayGainMode,
    pre_amp_db: f64,
    expected: Option<PcmFormat>,
) -> Result<Decoded, DecodeError> {
    let mut reader = symphonia::default::get_probe()
        .probe(
            hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| DecodeError::Decode(e.to_string()))?;

    let track = reader
        .default_track(TrackType::Audio)
        .ok_or(DecodeError::NoTrack)?;
    let track_id = track.id;
    let codec_params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or(DecodeError::NoTrack)?;
    let is_opus_codec = codec_params.codec == CODEC_ID_OPUS;

    // Opus always decodes to 48 kHz regardless of the internal rate.
    let sample_rate = if is_opus_codec {
        48000
    } else {
        codec_params.sample_rate.unwrap_or(44100)
    };
    let channels = codec_params
        .channels
        .as_ref()
        .map(|c| c.count() as u16)
        .unwrap_or(2);

    let duration_ms = track_duration_ms(&*reader, track, sample_rate);

    // Try codec_params first; fall back to file-size estimation for Opus/lossy.
    let mut bitrate_kbps = estimate_bitrate_from_codec_params(codec_params);
    if bitrate_kbps.is_none()
        && is_opus_codec
        && let Ok(meta) = std::fs::metadata(path)
        && duration_ms > 0
    {
        bitrate_kbps = Some((meta.len() * 8 / duration_ms) as u32);
    }

    let info = StreamInfo {
        codec: codec_name(codec_params.codec),
        sample_rate,
        channels,
        bit_depth: if is_opus_codec {
            None
        } else {
            Some(codec_params.bits_per_sample.unwrap_or(16) as u16)
        },
        bitrate_kbps,
        duration_ms,
    };

    if let Some(expected) = expected
        && expected != (sample_rate, channels)
    {
        log::info!(
            "format change at {}: {}Hz/{}ch → {}Hz/{}ch, restarting audio engine",
            path.display(),
            expected.0,
            expected.1,
            sample_rate,
            channels
        );
        return Ok(Decoded::FormatMismatch);
    }

    let seek_samples = seek_ms * sample_rate as u64 * channels as u64 / 1000;

    // Record this track's boundary in the timeline.
    let write_offset = timeline.samples_written.load(Ordering::Relaxed);
    timeline.push_boundary(TrackBoundary {
        id: queue_item_id,
        path: path.to_path_buf(),
        info,
        sample_offset: write_offset,
        samples_written: 0,
        seek_samples,
    });

    // Build either a Symphonia decoder or our Opus bridge.
    let mut symphonia_decoder = if is_opus_codec {
        None
    } else {
        Some(
            symphonia::default::get_codecs()
                .make_audio_decoder(codec_params, &AudioDecoderOptions::default())
                .map_err(|_| DecodeError::UnsupportedCodec)?,
        )
    };
    let mut opus_bridge = if is_opus_codec {
        Some(OpusBridge::new(codec_params).map_err(|e| DecodeError::Decode(e.to_string()))?)
    } else {
        None
    };

    // Seek if requested (only for the first track usually).
    if seek_ms > 0 {
        reader
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time: Time::from_millis_u64(seek_ms),
                    track_id: Some(track_id),
                },
            )
            .map_err(|e| DecodeError::Decode(format!("seek failed: {}", e)))?;
        if let Some(ref mut dec) = symphonia_decoder {
            dec.reset();
        }
        if let Some(ref mut opus) = opus_bridge {
            opus.reset();
        }
    }

    // Read ReplayGain tags and select the active gain for this track.
    let rg_gain = if rg_mode != ReplayGainMode::Off {
        match crate::audio::replaygain::read_tags(path) {
            Ok(rg_info) => {
                let selected = crate::audio::replaygain::select_gain(&rg_info, rg_mode);
                if let Some((gain_db, _)) = selected {
                    log::info!(
                        "replaygain: applying {:.2} dB ({:?}) to {}",
                        gain_db,
                        rg_mode,
                        path.display()
                    );
                }
                selected
            }
            Err(e) => {
                log::debug!("replaygain: no tags for {}: {}", path.display(), e);
                None
            }
        }
    } else {
        None
    };
    let mut rg_scratch: Vec<f32> = Vec::new();

    let mut sample_buf: Vec<f32> = Vec::new();

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(Decoded::Complete((sample_rate, channels)));
        }

        let packet = match reader.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(Decoded::Complete((sample_rate, channels))),
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };

        if packet.track_id != track_id {
            continue;
        }

        // Decode the packet — either via Opus bridge or Symphonia codec.
        let samples: &[f32] = if let Some(ref mut opus) = opus_bridge {
            match opus.decode_packet(&packet.data) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("opus decode error (skipping packet): {}", e);
                    continue;
                }
            }
        } else {
            let decoder = symphonia_decoder.as_mut().unwrap();
            let decoded = match decoder.decode(&packet) {
                Ok(d) => d,
                Err(symphonia::core::errors::Error::DecodeError(e)) => {
                    log::warn!("decode error (skipping packet): {}", e);
                    continue;
                }
                Err(e) => return Err(DecodeError::Decode(e.to_string())),
            };

            let spec = decoded.spec();
            let (decoded_rate, decoded_channels) = (spec.rate(), spec.channels().count() as u16);
            // The engine is configured from the probed format. PCM that
            // disagrees with it would play at the wrong speed, so end the
            // session instead and let the player reconfigure.
            if (decoded_rate, decoded_channels) != (sample_rate, channels) {
                log::warn!(
                    "{}: decoded {}Hz/{}ch but stream declares {}Hz/{}ch, restarting audio engine",
                    path.display(),
                    decoded_rate,
                    decoded_channels,
                    sample_rate,
                    channels
                );
                return Ok(Decoded::FormatMismatch);
            }
            decoded.copy_to_vec_interleaved(&mut sample_buf);
            &sample_buf[..]
        };

        if samples.is_empty() {
            continue;
        }

        // Apply ReplayGain if active. Uses a reusable scratch buffer to avoid
        // allocating per packet. Zero overhead when RG is off.
        let samples = if let Some((gain_db, peak)) = rg_gain {
            rg_scratch.clear();
            rg_scratch.extend_from_slice(samples);
            crate::audio::replaygain::apply_gain(&mut rg_scratch, gain_db, peak, pre_amp_db);
            &rg_scratch[..]
        } else {
            samples
        };

        // Push samples into ring buffer, blocking if full.
        // VizBuffer is updated incrementally inside this loop so it receives
        // samples at the real-time audio consumption rate (paced by the audio
        // callback draining the rtrb consumer), not in packet-sized bursts.
        // Without this, FLAC packets (~93ms each at 44.1kHz) would update the
        // viz buffer only ~11 times/sec, making waveform modes visibly choppy.
        let mut offset = 0;
        while offset < samples.len() {
            if stop.load(Ordering::Relaxed) {
                return Ok(Decoded::Complete((sample_rate, channels)));
            }

            let slots = producer.slots();
            if slots == 0 {
                thread::sleep(std::time::Duration::from_micros(500));
                continue;
            }

            let chunk_size = slots.min(samples.len() - offset);
            if let Ok(mut chunk) = producer.write_chunk_uninit(chunk_size) {
                let to_write = &samples[offset..offset + chunk_size];
                let (first, second) = chunk.as_mut_slices();
                let first_len = first.len().min(to_write.len());
                for (slot, &val) in first.iter_mut().zip(&to_write[..first_len]) {
                    slot.write(val);
                }
                if first_len < to_write.len() {
                    for (slot, &val) in second.iter_mut().zip(&to_write[first_len..]) {
                        slot.write(val);
                    }
                }
                // SAFETY: All slots in the chunk have been initialized by the
                // two loops above — first.len() + second.len() == chunk_size,
                // and every slot is written via MaybeUninit::write().
                unsafe { chunk.commit_all() };

                // Feed viz buffer at the same rate as rtrb consumption.
                if let Some(viz) = viz_buffer {
                    viz.push_samples(to_write, channels, sample_rate);
                }

                offset += chunk_size;
            }
        }

        timeline.add_written(samples.len() as u64);
    }
}

/// Duration of a track in milliseconds.
///
/// The container's stated duration is authoritative because a track's timebase
/// is not always the reciprocal of the sample rate — Matroska ticks in
/// milliseconds, and states its duration at media level rather than per track.
/// Falls back to the playable frame count when no duration is stated at all.
pub(crate) fn track_duration_ms(
    reader: &(impl FormatReader + ?Sized),
    track: &Track,
    sample_rate: u32,
) -> u64 {
    fn to_ms(time_base: Option<TimeBase>, duration: Option<Duration>) -> Option<u64> {
        let time = time_base?.calc_duration(duration?)?;
        Some(time.as_millis().max(0) as u64)
    }

    let media = reader.media_info();
    to_ms(track.time_base, track.duration)
        .or_else(|| to_ms(media.time_base, media.duration))
        .or_else(|| {
            track
                .num_frames
                .map(|frames| frames * 1000 / sample_rate as u64)
        })
        .unwrap_or(0)
}

/// Estimate bitrate (kbps) from Symphonia codec parameters.
///
/// Symphonia doesn't expose a `bit_rate` field. For lossy codecs like MP3/AAC
/// we can derive it from `bits_per_coded_sample` when the demuxer populates it.
/// Returns `None` for lossless codecs or when the info isn't available.
fn estimate_bitrate_from_codec_params(params: &AudioCodecParameters) -> Option<u32> {
    let is_lossy = matches!(
        params.codec,
        CODEC_ID_MP3 | CODEC_ID_AAC | CODEC_ID_VORBIS | CODEC_ID_OPUS
    );
    if !is_lossy {
        return None;
    }

    // bits_per_coded_sample * sample_rate / 1000 gives kbps for CBR streams.
    // Few demuxers fill this in, but it's our best shot without file size.
    let bpcs = params.bits_per_coded_sample?;
    let sr = params.sample_rate?;
    let channels = params
        .channels
        .as_ref()
        .map(|c| c.count() as u32)
        .unwrap_or(2);
    Some(bpcs * sr * channels / 1000)
}

pub fn codec_name(codec: AudioCodecId) -> String {
    match codec {
        CODEC_ID_FLAC => "FLAC",
        CODEC_ID_MP3 => "MP3",
        CODEC_ID_AAC => "AAC",
        CODEC_ID_VORBIS => "Vorbis",
        CODEC_ID_OPUS => "Opus",
        CODEC_ID_ALAC => "ALAC",
        CODEC_ID_WAVPACK => "WavPack",
        CODEC_ID_PCM_S16LE => "PCM/16",
        CODEC_ID_PCM_S24LE => "PCM/24",
        CODEC_ID_PCM_S32LE => "PCM/32",
        CODEC_ID_PCM_F32LE => "PCM/f32",
        other => return format!("Unknown({:?})", other),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::player::state::QueueItemId;

    fn make_info(sample_rate: u32, channels: u16) -> StreamInfo {
        StreamInfo {
            codec: "FLAC".to_string(),
            sample_rate,
            channels,
            bit_depth: Some(16),
            bitrate_kbps: None,
            duration_ms: 10_000,
        }
    }

    fn make_boundary(
        id: QueueItemId,
        sample_offset: u64,
        seek_samples: u64,
        channels: u16,
        sample_rate: u32,
    ) -> TrackBoundary {
        TrackBoundary {
            id,
            path: PathBuf::from("/music/track.flac"),
            info: make_info(sample_rate, channels),
            sample_offset,
            samples_written: 0,
            seek_samples,
        }
    }

    // --- PlaybackTimeline tests ---

    #[test]
    fn test_timeline_single_track() {
        // Push one boundary at offset 0 with stereo 44100 Hz audio.
        // After simulating 44100 frames (88200 interleaved samples) played,
        // current_playback() should report track index 0 at position 1000 ms.
        let timeline = PlaybackTimeline::new();
        let id = QueueItemId::new();
        // sample_offset=0, seek_samples=0, channels=2, sample_rate=44100
        timeline.push_boundary(make_boundary(id, 0, 0, 2, 44100));
        timeline.add_written(88200); // 1 second of audio

        // Simulate 1 second played: 44100 frames * 2 channels = 88200 interleaved samples
        timeline.samples_played.store(88200, Ordering::Relaxed);

        let result = timeline.current_playback();
        assert!(
            result.is_some(),
            "expected Some for single track with samples played"
        );
        let (result_id, _path, _info, position_ms) = result.unwrap();
        assert_eq!(result_id, id);
        assert_eq!(
            position_ms, 1000,
            "1 second of 44100 Hz stereo should be 1000 ms"
        );
    }

    #[test]
    fn test_timeline_gapless_transition() {
        // Two tracks in gapless sequence. Track 1 ends at sample 88200 (1 sec stereo 44100 Hz).
        // Track 2 begins at sample_offset 88200. When playback head is at 100000 (past the boundary),
        // current_playback() should report track 2.
        let timeline = PlaybackTimeline::new();
        let id1 = QueueItemId::new();
        let id2 = QueueItemId::new();

        // Track 1: starts at offset 0
        timeline.push_boundary(make_boundary(id1, 0, 0, 2, 44100));
        timeline.add_written(88200);

        // Track 2: starts at offset 88200 (immediately after track 1's samples)
        timeline.push_boundary(make_boundary(id2, 88200, 0, 2, 44100));
        timeline.add_written(44100); // half a second of track 2

        // Set playback head past the track 1/2 boundary
        timeline.samples_played.store(90000, Ordering::Relaxed);

        let result = timeline.current_playback();
        assert!(result.is_some());
        let (result_id, _path, _info, position_ms) = result.unwrap();
        assert_eq!(
            result_id, id2,
            "playback head past boundary should report second track"
        );
        // (90000 - 88200) / 2 channels * 1000 / 44100 = 900 / 44100 ≈ 20 ms
        assert_eq!(position_ms, 20, "position within track 2 should be ~20 ms");
    }

    #[test]
    fn test_timeline_zero_samples() {
        // With 0 samples played and a boundary at offset 0, current_playback() should
        // still return the first track at position 0 ms.
        let timeline = PlaybackTimeline::new();
        let id = QueueItemId::new();
        timeline.push_boundary(make_boundary(id, 0, 0, 2, 44100));
        timeline.add_written(1000);
        timeline.samples_played.store(0, Ordering::Relaxed);

        let result = timeline.current_playback();
        assert!(
            result.is_some(),
            "expected Some at 0 samples played with a boundary at offset 0"
        );
        let (result_id, _path, _info, position_ms) = result.unwrap();
        assert_eq!(result_id, id);
        assert_eq!(position_ms, 0);
    }

    #[test]
    fn test_timeline_past_all_boundaries() {
        // When samples_played exceeds all boundaries, the last track should be reported.
        // The binary search finds the last boundary whose sample_offset <= played.
        let timeline = PlaybackTimeline::new();
        let id1 = QueueItemId::new();
        let id2 = QueueItemId::new();

        timeline.push_boundary(make_boundary(id1, 0, 0, 2, 44100));
        timeline.add_written(88200);
        timeline.push_boundary(make_boundary(id2, 88200, 0, 2, 44100));
        timeline.add_written(88200);

        // Simulate playback far past both tracks
        timeline
            .samples_played
            .store(999_999_999, Ordering::Relaxed);

        let result = timeline.current_playback();
        assert!(result.is_some());
        let (result_id, _path, _info, _position_ms) = result.unwrap();
        assert_eq!(
            result_id, id2,
            "samples past all boundaries should report the last track"
        );
    }

    #[test]
    fn test_timeline_seek_offset() {
        // When a seek offset is set, position_ms should include the seek position.
        // seek_samples = 88200 means playback started 1 second into the track.
        // With 0 additional samples played past the boundary, position should be 1000 ms.
        let timeline = PlaybackTimeline::new();
        let id = QueueItemId::new();
        let seek_samples = 88200u64; // 1 second at 44100 Hz stereo
        timeline.push_boundary(make_boundary(id, 0, seek_samples, 2, 44100));
        timeline.add_written(44100); // half a second written so far
        // samples_played at the track boundary (0 frames past the track start)
        timeline.samples_played.store(0, Ordering::Relaxed);

        let result = timeline.current_playback();
        assert!(result.is_some());
        let (_result_id, _path, _info, position_ms) = result.unwrap();
        // track_samples = 0 - 0 = 0; seek contribution = (88200/2)*1000/44100 = 1000 ms
        assert_eq!(
            position_ms, 1000,
            "position should include seek offset of 1000 ms"
        );
    }

    #[test]
    fn test_timeline_reset() {
        // After reset(), current_playback() returns None and all counters are cleared.
        let timeline = PlaybackTimeline::new();
        let id = QueueItemId::new();
        timeline.push_boundary(make_boundary(id, 0, 0, 2, 44100));
        timeline.add_written(88200);
        timeline.samples_played.store(44100, Ordering::Relaxed);

        // Sanity check: playback is live before reset
        assert!(timeline.current_playback().is_some());

        timeline.reset();

        assert!(
            timeline.current_playback().is_none(),
            "after reset, current_playback should return None"
        );
        assert_eq!(
            timeline.samples_played.load(Ordering::Relaxed),
            0,
            "samples_played should be 0 after reset"
        );
        assert_eq!(
            timeline.samples_written.load(Ordering::Relaxed),
            0,
            "samples_written should be 0 after reset"
        );
    }

    // --- Probe and decode integration tests ---

    #[test]
    fn probe_file_extracts_stream_info() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("probe_test.wav");
        crate::test_utils::generate_wav(&wav_path, 44100, 2, 1.0, 16);

        let info = probe_file(&wav_path).expect("probe_file should succeed on a valid WAV");
        assert_eq!(info.sample_rate, 44100, "sample rate mismatch");
        assert_eq!(info.channels, 2, "channel count mismatch");
        assert_eq!(info.bit_depth, Some(16), "bit depth mismatch");
        assert!(
            info.duration_ms > 900 && info.duration_ms < 1100,
            "duration should be ~1000ms, got {}",
            info.duration_ms
        );
        assert!(
            info.codec.contains("PCM"),
            "codec should be PCM variant, got {}",
            info.codec
        );
    }

    #[test]
    fn decode_single_produces_samples() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("tone.wav");
        // 440 Hz sine, mono, 0.1s — enough to verify non-zero decode output.
        crate::test_utils::generate_wav_tone(&wav_path, 44100, 440.0, 0.1);

        // Set up rtrb ring buffer.
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(44100 * 2);

        let timeline = PlaybackTimeline::new();
        let stop = Arc::new(AtomicBool::new(false));

        let id = QueueItemId::new();
        let entry = SourceEntry::from_file(id, wav_path.clone());
        let hint = entry.hint.clone();
        let mss = (entry.make_mss)().expect("should open WAV file");

        let result = decode_single(
            id,
            &wav_path,
            &hint,
            mss,
            &mut producer,
            &stop,
            0,
            &timeline,
            None,
            crate::config::ReplayGainMode::Off,
            0.0,
            None,
        );
        assert!(
            matches!(result, Ok(Decoded::Complete((44100, 1)))),
            "decode_single should complete at the source format"
        );

        // Read samples from the consumer side.
        let available = consumer.slots();
        assert!(available > 0, "expected samples in ring buffer, got 0");

        // Verify at least some samples are non-zero (it's a sine wave, not silence).
        let mut found_nonzero = false;
        while consumer.slots() > 0 {
            if let Ok(chunk) = consumer.read_chunk(consumer.slots().min(1024)) {
                let (first, second) = chunk.as_slices();
                for &s in first.iter().chain(second.iter()) {
                    if s.abs() > 0.001 {
                        found_nonzero = true;
                        break;
                    }
                }
                chunk.commit_all();
            }
            if found_nonzero {
                break;
            }
        }
        assert!(
            found_nonzero,
            "expected non-zero samples from 440Hz sine decode"
        );
    }

    // --- Ring buffer format contract ---

    /// Decode a queue of files through `decode_queue_loop` with a consumer
    /// draining in the background. Returns the boundaries the decode thread
    /// pushed onto the timeline.
    fn run_queue(paths: &[PathBuf]) -> Vec<TrackBoundary> {
        let (producer, mut consumer) = rtrb::RingBuffer::new(1 << 16);
        let timeline = PlaybackTimeline::new();
        let stop = Arc::new(AtomicBool::new(false));

        let drain_stop = Arc::new(AtomicBool::new(false));
        let drain_flag = drain_stop.clone();
        let drainer = std::thread::spawn(move || {
            while !drain_flag.load(Ordering::Relaxed) {
                let n = consumer.slots();
                if n > 0
                    && let Ok(chunk) = consumer.read_chunk(n)
                {
                    chunk.commit_all();
                }
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        });

        let rest: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(paths[1..].to_vec());
        let next_track = move || {
            let mut rest = rest.lock().ok()?;
            if rest.is_empty() {
                return None;
            }
            Some(SourceEntry::from_file(QueueItemId::new(), rest.remove(0)))
        };

        decode_queue_loop(
            SourceEntry::from_file(QueueItemId::new(), paths[0].clone()),
            producer,
            &stop,
            0,
            &next_track,
            &timeline,
            None,
            crate::config::ReplayGainMode::Off,
            0.0,
        );

        drain_stop.store(true, Ordering::Relaxed);
        drainer.join().unwrap();

        timeline.boundaries.read().clone()
    }

    #[test]
    fn gapless_continues_when_format_matches() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        let b = dir.path().join("b.wav");
        crate::test_utils::generate_wav(&a, 44100, 2, 0.1, 16);
        crate::test_utils::generate_wav(&b, 44100, 2, 0.1, 16);

        let bounds = run_queue(&[a, b]);
        assert_eq!(
            bounds.len(),
            2,
            "same-format tracks should decode gaplessly"
        );
    }

    #[test]
    fn gapless_stops_at_sample_rate_change() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        let b = dir.path().join("b.wav");
        crate::test_utils::generate_wav(&a, 44100, 2, 0.1, 16);
        crate::test_utils::generate_wav(&b, 48000, 2, 0.1, 16);

        let bounds = run_queue(&[a, b]);
        assert_eq!(
            bounds.len(),
            1,
            "a 48kHz track must not join a 44.1kHz ring buffer"
        );
        assert_eq!(bounds[0].info.sample_rate, 44100);
    }

    #[test]
    fn gapless_stops_at_channel_change() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        let b = dir.path().join("b.wav");
        crate::test_utils::generate_wav(&a, 44100, 2, 0.1, 16);
        crate::test_utils::generate_wav(&b, 44100, 1, 0.1, 16);

        let bounds = run_queue(&[a, b]);
        assert_eq!(
            bounds.len(),
            1,
            "a mono track must not join a stereo ring buffer"
        );
        assert_eq!(bounds[0].info.channels, 2);
    }

    #[test]
    fn drain_waits_for_the_consumer() {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(64);
        for _ in 0..64 {
            producer.push(0.0).unwrap();
        }
        let stop = Arc::new(AtomicBool::new(false));

        let reader = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let chunk = consumer.read_chunk(64).unwrap();
            chunk.commit_all();
            consumer
        });

        wait_for_drain(&producer, &stop);
        assert_eq!(producer.slots(), 64, "drain must wait for an empty buffer");
        drop(reader.join().unwrap());
    }

    #[test]
    fn drain_returns_when_playback_is_torn_down() {
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(64);
        let stop = Arc::new(AtomicBool::new(true));
        wait_for_drain(&producer, &stop);
        drop(consumer);
    }
}
