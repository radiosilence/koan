use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{
    CODEC_TYPE_AAC, CODEC_TYPE_ALAC, CODEC_TYPE_FLAC, CODEC_TYPE_MP3, CODEC_TYPE_OPUS,
    CODEC_TYPE_PCM_F32LE, CODEC_TYPE_PCM_S16LE, CODEC_TYPE_PCM_S24LE, CODEC_TYPE_PCM_S32LE,
    CODEC_TYPE_VORBIS, CODEC_TYPE_WAVPACK, CodecType, DecoderOptions,
};
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use thiserror::Error;

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
    pub bit_depth: u16,
    pub duration_ms: u64,
}

/// Handle to a running decode thread. Drop to stop it.
pub struct DecodeHandle {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl DecodeHandle {
    /// Signal the decode thread to stop and wait for it.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
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
    /// Channel count — needed to convert samples → frames for position calc.
    channels: AtomicU64,
    /// Sample rate — needed for position calc.
    sample_rate: AtomicU64,
}

impl PlaybackTimeline {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            boundaries: parking_lot::RwLock::new(Vec::new()),
            samples_written: AtomicU64::new(0),
            samples_played: Arc::new(AtomicU64::new(0)),
            channels: AtomicU64::new(2),
            sample_rate: AtomicU64::new(44100),
        })
    }

    /// Called by decode thread when starting a new track.
    fn push_boundary(&self, boundary: TrackBoundary) {
        self.channels
            .store(boundary.info.channels as u64, Ordering::Relaxed);
        self.sample_rate
            .store(boundary.info.sample_rate as u64, Ordering::Relaxed);
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
    pub fn current_playback(&self) -> Option<(QueueItemId, PathBuf, StreamInfo, u64)> {
        let played = self.samples_played.load(Ordering::Relaxed);
        let bounds = self.boundaries.read();

        if bounds.is_empty() {
            return None;
        }

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
    pub make_mss: Box<dyn FnOnce() -> MediaSourceStream + Send>,
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
                let file = File::open(&path_clone).expect("failed to open audio file");
                MediaSourceStream::new(Box::new(file), Default::default())
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Probe API
// ---------------------------------------------------------------------------

/// Probe a `MediaSourceStream` (with hint) and return stream info without decoding.
pub fn probe_source(mss: MediaSourceStream, hint: &Hint) -> Result<StreamInfo, DecodeError> {
    probe_mss(mss, hint)
}

/// Probe a file and return stream info without decoding.
pub fn probe_file(path: &Path) -> Result<StreamInfo, DecodeError> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    probe_mss(mss, &hint)
}

/// Internal: probe a `MediaSourceStream` with a hint.
fn probe_mss(mss: MediaSourceStream, hint: &Hint) -> Result<StreamInfo, DecodeError> {
    let probed = symphonia::default::get_probe()
        .format(
            hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| DecodeError::Decode(e.to_string()))?;

    let reader = probed.format;
    let track = reader.default_track().ok_or(DecodeError::NoTrack)?;
    let codec_params = &track.codec_params;
    let sample_rate = codec_params.sample_rate.unwrap_or(44100);
    let channels = codec_params.channels.map(|c| c.count() as u16).unwrap_or(2);
    let bit_depth = codec_params.bits_per_sample.unwrap_or(16) as u16;
    let duration_ms = track
        .codec_params
        .n_frames
        .map(|frames| frames * 1000 / sample_rate as u64)
        .unwrap_or(0);
    let codec = codec_name(codec_params.codec);

    Ok(StreamInfo {
        codec,
        sample_rate,
        channels,
        bit_depth,
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
pub fn start_decode<N>(
    first: SourceEntry,
    producer: rtrb::Producer<f32>,
    seek_ms: u64,
    next_track: N,
    timeline: Arc<PlaybackTimeline>,
) -> Result<(StreamInfo, DecodeHandle), DecodeError>
where
    N: Fn() -> Option<SourceEntry> + Send + 'static,
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
            );
        })
        .map_err(DecodeError::Io)?;

    // Return a placeholder StreamInfo — the real info is pushed to the timeline
    // by the decode thread immediately after probing the source.
    let placeholder = StreamInfo {
        codec: String::from("?"),
        sample_rate: 44100,
        channels: 2,
        bit_depth: 16,
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
pub fn start_decode_file<N>(
    initial_id: QueueItemId,
    path: &Path,
    producer: rtrb::Producer<f32>,
    seek_ms: u64,
    next_track: N,
    timeline: Arc<PlaybackTimeline>,
) -> Result<(StreamInfo, DecodeHandle), DecodeError>
where
    N: Fn() -> Option<(QueueItemId, PathBuf)> + Send + 'static,
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
    )?;
    Ok((info, handle))
}

// ---------------------------------------------------------------------------
// Internal decode loop
// ---------------------------------------------------------------------------

/// Gapless decode loop: decode first entry, then call next_track on EOF.
fn decode_queue_loop<N>(
    first: SourceEntry,
    mut producer: rtrb::Producer<f32>,
    stop: &AtomicBool,
    initial_seek_ms: u64,
    next_track: &N,
    timeline: &PlaybackTimeline,
) where
    N: Fn() -> Option<SourceEntry>,
{
    let path = first.path.clone();
    let hint = first.hint.clone();
    let mss = (first.make_mss)();

    if let Err(e) = decode_single(
        first.id,
        &path,
        &hint,
        mss,
        &mut producer,
        stop,
        initial_seek_ms,
        timeline,
    ) {
        if !stop.load(Ordering::Relaxed) {
            log::error!("decode error on {}: {}", path.display(), e);
        }
        return;
    }

    while !stop.load(Ordering::Relaxed) {
        let Some(entry) = (next_track)() else {
            log::info!("playlist exhausted, decode thread finishing");
            break;
        };

        log::info!("gapless transition → {}", entry.path.display());
        let next_path = entry.path.clone();
        let next_hint = entry.hint.clone();
        let next_mss = (entry.make_mss)();

        if let Err(e) = decode_single(
            entry.id,
            &next_path,
            &next_hint,
            next_mss,
            &mut producer,
            stop,
            0,
            timeline,
        ) {
            if !stop.load(Ordering::Relaxed) {
                log::error!("decode error on {}: {}", next_path.display(), e);
            }
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Core decode single track
// ---------------------------------------------------------------------------

/// Decode a single source into the producer. Returns Ok(()) on clean EOF.
#[allow(clippy::too_many_arguments)]
fn decode_single(
    queue_item_id: QueueItemId,
    path: &Path,
    hint: &Hint,
    mss: MediaSourceStream,
    producer: &mut rtrb::Producer<f32>,
    stop: &AtomicBool,
    seek_ms: u64,
    timeline: &PlaybackTimeline,
) -> Result<(), DecodeError> {
    let format_opts = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };

    let probed = symphonia::default::get_probe()
        .format(hint, mss, &format_opts, &MetadataOptions::default())
        .map_err(|e| DecodeError::Decode(e.to_string()))?;

    let mut reader = probed.format;
    let track = reader.default_track().ok_or(DecodeError::NoTrack)?;
    let track_id = track.id;
    let codec_params = &track.codec_params;
    let sample_rate = codec_params.sample_rate.unwrap_or(44100);
    let channels = codec_params.channels.map(|c| c.count() as u16).unwrap_or(2);

    let info = StreamInfo {
        codec: codec_name(codec_params.codec),
        sample_rate,
        channels,
        bit_depth: codec_params.bits_per_sample.unwrap_or(16) as u16,
        duration_ms: codec_params
            .n_frames
            .map(|f| f * 1000 / sample_rate as u64)
            .unwrap_or(0),
    };

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

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| DecodeError::UnsupportedCodec)?;

    // Seek if requested (only for the first track usually).
    if seek_ms > 0 {
        let secs = seek_ms / 1000;
        let frac = (seek_ms % 1000) as f64 / 1000.0;
        reader
            .seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time: Time::new(secs, frac),
                    track_id: Some(track_id),
                },
            )
            .map_err(|e| DecodeError::Decode(format!("seek failed: {}", e)))?;
        decoder.reset();
    }

    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }

        let packet = match reader.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(());
            }
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(e)) => {
                log::warn!("decode error (skipping packet): {}", e);
                continue;
            }
            Err(e) => return Err(DecodeError::Decode(e.to_string())),
        };

        let spec = *decoded.spec();
        let duration = decoded.capacity();

        let sbuf = sample_buf.get_or_insert_with(|| SampleBuffer::new(duration as u64, spec));

        sbuf.copy_interleaved_ref(decoded);
        let samples = sbuf.samples();

        // Push samples into ring buffer, blocking if full.
        let mut offset = 0;
        while offset < samples.len() {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
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
                unsafe { chunk.commit_all() };
                offset += chunk_size;
            }
        }

        timeline.add_written(samples.len() as u64);
    }
}

pub fn codec_name(codec: CodecType) -> String {
    match codec {
        CODEC_TYPE_FLAC => "FLAC",
        CODEC_TYPE_MP3 => "MP3",
        CODEC_TYPE_AAC => "AAC",
        CODEC_TYPE_VORBIS => "Vorbis",
        CODEC_TYPE_OPUS => "Opus",
        CODEC_TYPE_ALAC => "ALAC",
        CODEC_TYPE_WAVPACK => "WavPack",
        CODEC_TYPE_PCM_S16LE => "PCM/16",
        CODEC_TYPE_PCM_S24LE => "PCM/24",
        CODEC_TYPE_PCM_S32LE => "PCM/32",
        CODEC_TYPE_PCM_F32LE => "PCM/f32",
        other => return format!("Unknown({:?})", other),
    }
    .to_string()
}
