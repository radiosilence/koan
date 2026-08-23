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

/// Generate an MP3 carrying both an ID3v2.3 tag and an ID3v1 trailer, with
/// different titles in each.
///
/// The pair is what matters: ID3v1's fields are a fixed 30 bytes, so a real
/// library is full of files whose v1 title is a truncated copy of the v2 one.
/// Anything reading tags has to prefer v2, and the way that breaks is silent.
///
/// The audio is a run of valid but silent MPEG-1 Layer III frames — enough for
/// Symphonia to probe the file, which is the path this exercises.
pub fn generate_mp3_with_both_tags(path: &Path, id3v2_title: &str, id3v1_title: &str) {
    let mut out: Vec<u8> = Vec::new();

    // -- ID3v2.3 header, then a TIT2 frame holding the full title.
    let mut frames: Vec<u8> = Vec::new();
    frames.extend_from_slice(b"TIT2");
    let body_len = id3v2_title.len() as u32 + 1; // + encoding byte
    frames.extend_from_slice(&body_len.to_be_bytes());
    frames.extend_from_slice(&[0, 0]); // flags
    frames.push(0); // ISO-8859-1
    frames.extend_from_slice(id3v2_title.as_bytes());

    // A track number too: a row without one cannot content-match the same track
    // synced from a remote server, so it matters as much as the title.
    frames.extend_from_slice(b"TRCK");
    frames.extend_from_slice(&2u32.to_be_bytes());
    frames.extend_from_slice(&[0, 0]);
    frames.push(0);
    frames.push(b'7');

    out.extend_from_slice(b"ID3");
    out.extend_from_slice(&[3, 0, 0]); // v2.3, no flags
    let n = frames.len() as u32;
    // Synchsafe: seven bits per byte.
    out.extend_from_slice(&[
        ((n >> 21) & 0x7f) as u8,
        ((n >> 14) & 0x7f) as u8,
        ((n >> 7) & 0x7f) as u8,
        (n & 0x7f) as u8,
    ]);
    out.extend_from_slice(&frames);

    // -- Audio: MPEG-1 Layer III, 128 kbps, 44.1 kHz, stereo. 417 bytes a
    // frame at this rate, header included; a zeroed payload is silence.
    for _ in 0..40 {
        out.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        out.extend_from_slice(&[0u8; 413]);
    }

    // -- ID3v1 trailer, whose 30-byte title field is the truncated one.
    let mut v1 = [0u8; 128];
    v1[..3].copy_from_slice(b"TAG");
    let title = id3v1_title.as_bytes();
    let take = title.len().min(30);
    v1[3..3 + take].copy_from_slice(&title[..take]);
    out.extend_from_slice(&v1);

    std::fs::write(path, out).expect("failed to write MP3 file");
}
