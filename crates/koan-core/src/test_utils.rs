//! Shared test utilities for koan-core integration tests.
//!
//! Behind `#[cfg(test)]` — not compiled into release builds.

use std::io::Write;
use std::path::Path;

/// Generate a minimal valid WAV file: RIFF header + fmt chunk + data chunk.
///
/// Produces silence (all zeros) at the given spec. The result is parseable by
/// both lofty (for metadata/tag reading) and symphonia (for decoding).
///
/// `sample_rate` — e.g. 44100
/// `channels`    — 1 (mono) or 2 (stereo)
/// `duration_secs` — how many seconds of silence
/// `bit_depth`   — 16 (PCM s16le)
pub fn generate_wav(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    duration_secs: f32,
    bit_depth: u16,
) {
    let bytes_per_sample = bit_depth / 8;
    let block_align = channels * bytes_per_sample;
    let byte_rate = sample_rate * block_align as u32;
    let num_samples = (sample_rate as f32 * duration_secs) as u32;
    let data_size = num_samples * block_align as u32;
    // RIFF header (12) + fmt chunk (24) + data chunk header (8) + data
    let file_size = 4 + 24 + 8 + data_size; // size after "RIFF" + 4-byte size field

    let mut file = std::fs::File::create(path).expect("failed to create WAV file");

    // RIFF header
    file.write_all(b"RIFF").unwrap();
    file.write_all(&file_size.to_le_bytes()).unwrap();
    file.write_all(b"WAVE").unwrap();

    // fmt sub-chunk
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap(); // sub-chunk size
    file.write_all(&1u16.to_le_bytes()).unwrap(); // PCM format
    file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    file.write_all(&block_align.to_le_bytes()).unwrap();
    file.write_all(&bit_depth.to_le_bytes()).unwrap();

    // data sub-chunk
    file.write_all(b"data").unwrap();
    file.write_all(&data_size.to_le_bytes()).unwrap();
    // Write silence (zeros) in 4KB chunks to avoid allocating huge buffers.
    let zeros = [0u8; 4096];
    let mut remaining = data_size as usize;
    while remaining > 0 {
        let chunk = remaining.min(zeros.len());
        file.write_all(&zeros[..chunk]).unwrap();
        remaining -= chunk;
    }

    file.flush().unwrap();
}

/// Generate a WAV file with a sine tone for decode verification.
///
/// Same structure as `generate_wav` but fills the data chunk with a sine wave
/// at the given frequency. 16-bit signed PCM, mono.
pub fn generate_wav_tone(path: &Path, sample_rate: u32, frequency_hz: f32, duration_secs: f32) {
    let channels: u16 = 1;
    let bit_depth: u16 = 16;
    let bytes_per_sample = bit_depth / 8;
    let block_align = channels * bytes_per_sample;
    let byte_rate = sample_rate * block_align as u32;
    let num_samples = (sample_rate as f32 * duration_secs) as u32;
    let data_size = num_samples * block_align as u32;
    let file_size = 4 + 24 + 8 + data_size;

    let mut file = std::fs::File::create(path).expect("failed to create WAV file");

    // RIFF header
    file.write_all(b"RIFF").unwrap();
    file.write_all(&file_size.to_le_bytes()).unwrap();
    file.write_all(b"WAVE").unwrap();

    // fmt sub-chunk
    file.write_all(b"fmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
    file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&sample_rate.to_le_bytes()).unwrap();
    file.write_all(&byte_rate.to_le_bytes()).unwrap();
    file.write_all(&block_align.to_le_bytes()).unwrap();
    file.write_all(&bit_depth.to_le_bytes()).unwrap();

    // data sub-chunk
    file.write_all(b"data").unwrap();
    file.write_all(&data_size.to_le_bytes()).unwrap();

    // Write sine wave samples
    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let sample = (2.0 * std::f32::consts::PI * frequency_hz * t).sin();
        let sample_i16 = (sample * i16::MAX as f32) as i16;
        file.write_all(&sample_i16.to_le_bytes()).unwrap();
    }

    file.flush().unwrap();
}
