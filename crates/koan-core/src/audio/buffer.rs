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

        // Find which track the playback head is in.
        // Walk boundaries to find the last one whose offset <= played.
        let mut current = &bounds[0];
        for b in bounds.iter() {
            if b.sample_offset <= played {
                current = b;
            } else {
                break;
            }
        }

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

/// Probe a file and return stream info without decoding.
pub fn probe_file(path: &Path) -> Result<StreamInfo, DecodeError> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
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

/// Start decoding a file into the ring buffer.
///
/// `initial_id` — the QueueItemId of the first track.
/// `seek_ms` — if > 0, seek to this position before decoding the first track.
/// `next_track` — closure that returns the next (id, path) for gapless playback.
///                Called on EOF. Returns None when the playlist is exhausted.
pub fn start_decode<N>(
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
    let path = path.to_path_buf();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let thread = thread::Builder::new()
        .name("koan-decode".into())
        .spawn(move || {
            decode_queue_loop(
                initial_id,
                &path,
                producer,
                &stop_clone,
                seek_ms,
                &next_track,
                &timeline,
            );
        })
        .map_err(DecodeError::Io)?;

    Ok((
        info,
        DecodeHandle {
            stop,
            thread: Some(thread),
        },
    ))
}

/// Gapless decode loop: decodes the first file, then calls next_track on EOF.
///
/// The key insight: the producer (ring buffer write end) stays alive across track
/// boundaries. The AudioUnit keeps draining the consumer. Zero gap.
fn decode_queue_loop<N>(
    initial_id: QueueItemId,
    first_path: &Path,
    mut producer: rtrb::Producer<f32>,
    stop: &AtomicBool,
    initial_seek_ms: u64,
    next_track: &N,
    timeline: &PlaybackTimeline,
) where
    N: Fn() -> Option<(QueueItemId, PathBuf)>,
{
    // Decode the first track.
    if let Err(e) = decode_single(
        initial_id,
        first_path,
        &mut producer,
        stop,
        initial_seek_ms,
        timeline,
    ) {
        if !stop.load(Ordering::Relaxed) {
            log::error!("decode error on {}: {}", first_path.display(), e);
        }
        return;
    }

    // Gapless: keep going through the playlist.
    while !stop.load(Ordering::Relaxed) {
        let Some((next_id, next_path)) = (next_track)() else {
            log::info!("playlist exhausted, decode thread finishing");
            break;
        };

        log::info!("gapless transition → {}", next_path.display());

        if let Err(e) = decode_single(next_id, &next_path, &mut producer, stop, 0, timeline) {
            if !stop.load(Ordering::Relaxed) {
                log::error!("decode error on {}: {}", next_path.display(), e);
            }
            break;
        }
    }
}

/// Decode a single file into the producer. Returns Ok(()) on clean EOF.
fn decode_single(
    queue_item_id: QueueItemId,
    path: &Path,
    producer: &mut rtrb::Producer<f32>,
    stop: &AtomicBool,
    seek_ms: u64,
    timeline: &PlaybackTimeline,
) -> Result<(), DecodeError> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let format_opts = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &MetadataOptions::default())
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
