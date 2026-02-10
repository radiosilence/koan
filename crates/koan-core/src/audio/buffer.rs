use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

use crate::player::queue::TrackQueue;

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

/// Callbacks fired by the decode thread to inform the player of state changes.
pub struct DecodeCallbacks<F, G>
where
    F: Fn(u64) + Send + 'static,
    G: Fn(PathBuf, StreamInfo) + Send + 'static,
{
    /// Called periodically with current decode position in ms.
    pub on_position: F,
    /// Called when a new track starts playing (gapless transition or initial).
    /// Receives the path and stream info of the new track.
    pub on_track_change: G,
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

/// Start decoding a file (with optional queue for gapless) into the ring buffer.
///
/// `seek_ms` — if > 0, seek to this position before decoding the first track.
/// `queue` — remaining tracks for gapless playback. The decode thread pops from
///           this when the current track hits EOF.
pub fn start_decode<F, G>(
    path: &Path,
    producer: rtrb::Producer<f32>,
    seek_ms: u64,
    queue: Arc<TrackQueue>,
    callbacks: DecodeCallbacks<F, G>,
) -> Result<(StreamInfo, DecodeHandle), DecodeError>
where
    F: Fn(u64) + Send + 'static,
    G: Fn(PathBuf, StreamInfo) + Send + 'static,
{
    let info = probe_file(path)?;
    let path = path.to_path_buf();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    let thread = thread::Builder::new()
        .name("koan-decode".into())
        .spawn(move || {
            decode_queue_loop(&path, producer, &stop_clone, seek_ms, &queue, &callbacks);
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

/// Gapless decode loop: decodes the first file, then pops from the queue on EOF.
///
/// The key insight: the producer (ring buffer write end) stays alive across track
/// boundaries. The AudioUnit keeps draining the consumer. Zero gap.
fn decode_queue_loop<F, G>(
    first_path: &Path,
    mut producer: rtrb::Producer<f32>,
    stop: &AtomicBool,
    initial_seek_ms: u64,
    queue: &TrackQueue,
    callbacks: &DecodeCallbacks<F, G>,
) where
    F: Fn(u64) + Send,
    G: Fn(PathBuf, StreamInfo) + Send,
{
    // Decode the first track.
    let result = decode_single(first_path, &mut producer, stop, initial_seek_ms, callbacks);

    if let Err(e) = &result {
        if !stop.load(Ordering::Relaxed) {
            log::error!("decode error on {}: {}", first_path.display(), e);
        }
        return;
    }

    // Gapless: keep going through the queue.
    while !stop.load(Ordering::Relaxed) {
        let next = queue.pop_front();
        let Some(next_path) = next else {
            // Queue empty — we're done.
            log::info!("queue empty, decode thread finishing");
            break;
        };

        log::info!("gapless transition → {}", next_path.display());

        let result = decode_single(&next_path, &mut producer, stop, 0, callbacks);
        if let Err(e) = &result {
            if !stop.load(Ordering::Relaxed) {
                log::error!("decode error on {}: {}", next_path.display(), e);
            }
            break;
        }
    }
}

/// Decode a single file into the producer. Returns Ok(()) on clean EOF.
fn decode_single<F, G>(
    path: &Path,
    producer: &mut rtrb::Producer<f32>,
    stop: &AtomicBool,
    seek_ms: u64,
    callbacks: &DecodeCallbacks<F, G>,
) -> Result<(), DecodeError>
where
    F: Fn(u64) + Send,
    G: Fn(PathBuf, StreamInfo) + Send,
{
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

    // Notify player of track change.
    (callbacks.on_track_change)(path.to_path_buf(), info);

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
    let mut samples_decoded: u64 = seek_ms * sample_rate as u64 * channels as u64 / 1000;

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }

        let packet = match reader.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                // EOF — this track is done. Return cleanly for gapless to pick up next.
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

        samples_decoded += samples.len() as u64;
        let ch = spec.channels.count() as u64;
        if ch > 0 {
            let position_ms = (samples_decoded / ch) * 1000 / sample_rate as u64;
            (callbacks.on_position)(position_ms);
        }
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
