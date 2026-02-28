use std::fs::File;
use std::path::{Path, PathBuf};

use lofty::prelude::*;
use lofty::tag::ItemValue;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use thiserror::Error;

use crate::config::ReplayGainMode;

/// Reference loudness for ReplayGain 2 (EBU R128): -18 LUFS.
const RG2_REFERENCE_LUFS: f64 = -18.0;

#[derive(Debug, Error)]
pub enum ReplayGainError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no audio track found")]
    NoTrack,
    #[error("decode error: {0}")]
    Decode(String),
    #[error("tag error: {0}")]
    Tag(String),
    #[error("ebur128 error: {0}")]
    Ebur128(String),
}

/// ReplayGain values extracted from file tags.
#[derive(Debug, Clone, Default)]
pub struct ReplayGainInfo {
    pub track_gain_db: Option<f64>,
    pub track_peak: Option<f64>,
    pub album_gain_db: Option<f64>,
    pub album_peak: Option<f64>,
}

// Tag field names used across Vorbis comments, ID3v2 TXXX, and APE tags.
const TAG_TRACK_GAIN: &str = "REPLAYGAIN_TRACK_GAIN";
const TAG_TRACK_PEAK: &str = "REPLAYGAIN_TRACK_PEAK";
const TAG_ALBUM_GAIN: &str = "REPLAYGAIN_ALBUM_GAIN";
const TAG_ALBUM_PEAK: &str = "REPLAYGAIN_ALBUM_PEAK";

/// Parse a ReplayGain gain string like "+3.21 dB" or "-1.50 dB" into f64.
fn parse_gain(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s.strip_suffix("dB").or(Some(s))?;
    s.trim().parse::<f64>().ok()
}

/// Parse a ReplayGain peak string (plain float) into f64.
fn parse_peak(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

/// Read ReplayGain tags from a file using lofty.
pub fn read_tags(path: &Path) -> Result<ReplayGainInfo, ReplayGainError> {
    let tagged_file =
        lofty::read_from_path(path).map_err(|e| ReplayGainError::Tag(e.to_string()))?;

    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());

    let Some(tag) = tag else {
        return Ok(ReplayGainInfo::default());
    };

    // lofty's get_string handles Vorbis comments, ID3v2 TXXX, and APE items
    // via ItemKey mapping. We also do a manual fallback for common RG keys.
    let track_gain = find_rg_value(tag, TAG_TRACK_GAIN).and_then(|s| parse_gain(&s));
    let track_peak = find_rg_value(tag, TAG_TRACK_PEAK).and_then(|s| parse_peak(&s));
    let album_gain = find_rg_value(tag, TAG_ALBUM_GAIN).and_then(|s| parse_gain(&s));
    let album_peak = find_rg_value(tag, TAG_ALBUM_PEAK).and_then(|s| parse_peak(&s));

    Ok(ReplayGainInfo {
        track_gain_db: track_gain,
        track_peak,
        album_gain_db: album_gain,
        album_peak,
    })
}

/// Search for a ReplayGain tag value by its standard field name.
/// Tries lofty's built-in ItemKey mapping first, then falls back to
/// searching by raw key name for formats that use string-keyed items.
fn find_rg_value(tag: &lofty::tag::Tag, key_name: &str) -> Option<String> {
    // Map RG field names to lofty's ItemKey variants.
    let item_key = match key_name {
        TAG_TRACK_GAIN => Some(ItemKey::ReplayGainTrackGain),
        TAG_TRACK_PEAK => Some(ItemKey::ReplayGainTrackPeak),
        TAG_ALBUM_GAIN => Some(ItemKey::ReplayGainAlbumGain),
        TAG_ALBUM_PEAK => Some(ItemKey::ReplayGainAlbumPeak),
        _ => None,
    };

    if let Some(ik) = item_key
        && let Some(val) = tag.get_string(ik)
    {
        return Some(val.to_string());
    }

    // Fallback: iterate items looking for a text value matching the key name.
    // Covers edge cases where the tag format uses raw string keys.
    for item in tag.items() {
        if let ItemValue::Text(ref text) = *item.value() {
            // Check if any item's key maps to our key_name via its string repr.
            let key_str = format!("{:?}", item.key());
            if key_str.contains(key_name) {
                return Some(text.clone());
            }
        }
    }

    None
}

/// Apply ReplayGain to an f32 sample buffer in-place.
///
/// `gain_db`: the gain to apply in decibels.
/// `peak`: optional peak value for clipping prevention.
/// `pre_amp_db`: additional pre-amplification in dB (from user config).
pub fn apply_gain(samples: &mut [f32], gain_db: f64, peak: Option<f64>, pre_amp_db: f64) {
    let total_gain_db = gain_db + pre_amp_db;
    let linear_gain = 10f64.powf(total_gain_db / 20.0);

    // If we know the peak, limit gain so it won't clip.
    let limited_gain = if let Some(peak) = peak {
        let max_gain = 1.0 / peak;
        linear_gain.min(max_gain)
    } else {
        linear_gain
    };

    let gain_f32 = limited_gain as f32;
    for sample in samples.iter_mut() {
        *sample *= gain_f32;
    }
}

/// Select the appropriate gain value based on ReplayGain mode.
/// Returns `(gain_db, peak)` if applicable, or `None` for off/unknown.
pub fn select_gain(info: &ReplayGainInfo, mode: ReplayGainMode) -> Option<(f64, Option<f64>)> {
    match mode {
        ReplayGainMode::Track => info.track_gain_db.map(|g| (g, info.track_peak)),
        ReplayGainMode::Album => info
            .album_gain_db
            .or(info.track_gain_db)
            .map(|g| (g, info.album_peak.or(info.track_peak))),
        ReplayGainMode::Off => None,
    }
}

/// Scan a single track and compute its ReplayGain values (track gain + peak).
pub fn scan_track(path: &Path) -> Result<ReplayGainInfo, ReplayGainError> {
    let (sample_rate, channels, all_samples) = decode_to_samples(path)?;

    let mut ebu = ebur128::EbuR128::new(channels as u32, sample_rate, ebur128::Mode::all())
        .map_err(|e| ReplayGainError::Ebur128(e.to_string()))?;

    // Feed interleaved f32 samples.
    ebu.add_frames_f32(&all_samples)
        .map_err(|e| ReplayGainError::Ebur128(e.to_string()))?;

    let loudness = ebu
        .loudness_global()
        .map_err(|e| ReplayGainError::Ebur128(e.to_string()))?;
    let gain_db = RG2_REFERENCE_LUFS - loudness;

    // Peak across all channels.
    let mut peak = 0.0f64;
    for ch in 0..channels as u32 {
        let ch_peak = ebu
            .sample_peak(ch)
            .map_err(|e| ReplayGainError::Ebur128(e.to_string()))?;
        if ch_peak > peak {
            peak = ch_peak;
        }
    }

    Ok(ReplayGainInfo {
        track_gain_db: Some(gain_db),
        track_peak: Some(peak),
        album_gain_db: None,
        album_peak: None,
    })
}

/// Scan multiple tracks as an album. Returns per-track info with album gain/peak filled in.
pub fn scan_album(paths: &[PathBuf]) -> Result<Vec<ReplayGainInfo>, ReplayGainError> {
    if paths.is_empty() {
        return Ok(vec![]);
    }

    // First pass: scan each track individually and collect decoded data for album pass.
    let mut track_infos = Vec::with_capacity(paths.len());
    let mut album_ebu: Option<ebur128::EbuR128> = None;

    for path in paths {
        let (sample_rate, channels, all_samples) = decode_to_samples(path)?;

        // Per-track analysis.
        let mut track_ebu =
            ebur128::EbuR128::new(channels as u32, sample_rate, ebur128::Mode::all())
                .map_err(|e| ReplayGainError::Ebur128(e.to_string()))?;
        track_ebu
            .add_frames_f32(&all_samples)
            .map_err(|e| ReplayGainError::Ebur128(e.to_string()))?;

        let loudness = track_ebu
            .loudness_global()
            .map_err(|e| ReplayGainError::Ebur128(e.to_string()))?;
        let gain_db = RG2_REFERENCE_LUFS - loudness;

        let mut peak = 0.0f64;
        for ch in 0..channels as u32 {
            let ch_peak = track_ebu
                .sample_peak(ch)
                .map_err(|e| ReplayGainError::Ebur128(e.to_string()))?;
            if ch_peak > peak {
                peak = ch_peak;
            }
        }

        track_infos.push(ReplayGainInfo {
            track_gain_db: Some(gain_db),
            track_peak: Some(peak),
            album_gain_db: None,
            album_peak: None,
        });

        // Album-level: feed same samples into a shared EbuR128 instance.
        let ebu = album_ebu.get_or_insert_with(|| {
            // Unwrap safe: if track decoding succeeded, this will too.
            ebur128::EbuR128::new(channels as u32, sample_rate, ebur128::Mode::all()).unwrap()
        });
        ebu.add_frames_f32(&all_samples)
            .map_err(|e| ReplayGainError::Ebur128(e.to_string()))?;
    }

    // Album-level loudness and peak.
    if let Some(ref ebu) = album_ebu {
        let album_loudness = ebu
            .loudness_global()
            .map_err(|e| ReplayGainError::Ebur128(e.to_string()))?;
        let album_gain = RG2_REFERENCE_LUFS - album_loudness;

        // Album peak = max of all track peaks.
        let album_peak = track_infos
            .iter()
            .filter_map(|i| i.track_peak)
            .fold(0.0f64, f64::max);

        for info in &mut track_infos {
            info.album_gain_db = Some(album_gain);
            info.album_peak = Some(album_peak);
        }
    }

    Ok(track_infos)
}

/// Write ReplayGain tags to a file using lofty.
pub fn write_tags(path: &Path, info: &ReplayGainInfo) -> Result<(), ReplayGainError> {
    let mut tagged_file =
        lofty::read_from_path(path).map_err(|e| ReplayGainError::Tag(e.to_string()))?;

    // Avoid double mutable borrow by checking primary first.
    let has_primary = tagged_file.primary_tag().is_some();
    let tag = if has_primary {
        tagged_file.primary_tag_mut().unwrap()
    } else {
        tagged_file
            .first_tag_mut()
            .ok_or_else(|| ReplayGainError::Tag("no tag container found".into()))?
    };

    if let Some(gain) = info.track_gain_db {
        tag.insert_text(ItemKey::ReplayGainTrackGain, format_gain(gain));
    }
    if let Some(peak) = info.track_peak {
        tag.insert_text(ItemKey::ReplayGainTrackPeak, format_peak(peak));
    }
    if let Some(gain) = info.album_gain_db {
        tag.insert_text(ItemKey::ReplayGainAlbumGain, format_gain(gain));
    }
    if let Some(peak) = info.album_peak {
        tag.insert_text(ItemKey::ReplayGainAlbumPeak, format_peak(peak));
    }

    tag.save_to_path(path, lofty::config::WriteOptions::default())
        .map_err(|e| ReplayGainError::Tag(e.to_string()))?;

    Ok(())
}

/// Format gain as "+X.XX dB" / "-X.XX dB".
fn format_gain(db: f64) -> String {
    format!("{:+.2} dB", db)
}

/// Format peak as "X.XXXXXX".
fn format_peak(peak: f64) -> String {
    format!("{:.6}", peak)
}

/// Decode a file to interleaved f32 samples using Symphonia.
/// Returns (sample_rate, channels, samples).
fn decode_to_samples(path: &Path) -> Result<(u32, u16, Vec<f32>), ReplayGainError> {
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
        .map_err(|e| ReplayGainError::Decode(e.to_string()))?;

    let mut reader = probed.format;
    let track = reader.default_track().ok_or(ReplayGainError::NoTrack)?;
    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| ReplayGainError::Decode(e.to_string()))?;

    let mut all_samples = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match reader.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(ReplayGainError::Decode(e.to_string())),
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
            Err(e) => return Err(ReplayGainError::Decode(e.to_string())),
        };

        let spec = *decoded.spec();
        let duration = decoded.capacity();
        let sbuf = sample_buf.get_or_insert_with(|| SampleBuffer::new(duration as u64, spec));
        sbuf.copy_interleaved_ref(decoded);
        all_samples.extend_from_slice(sbuf.samples());
    }

    Ok((sample_rate, channels, all_samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gain_with_db_suffix() {
        assert_eq!(parse_gain("+3.21 dB"), Some(3.21));
        assert_eq!(parse_gain("-1.50 dB"), Some(-1.50));
        assert_eq!(parse_gain("0.00 dB"), Some(0.0));
    }

    #[test]
    fn test_parse_gain_without_suffix() {
        assert_eq!(parse_gain("+3.21"), Some(3.21));
        assert_eq!(parse_gain("-1.50"), Some(-1.50));
    }

    #[test]
    fn test_parse_gain_invalid() {
        assert_eq!(parse_gain("not a number"), None);
        assert_eq!(parse_gain(""), None);
    }

    #[test]
    fn test_parse_peak() {
        assert_eq!(parse_peak("1.000000"), Some(1.0));
        assert_eq!(parse_peak("0.987654"), Some(0.987654));
        assert_eq!(parse_peak("nope"), None);
    }

    #[test]
    fn test_db_to_linear_conversion() {
        // 0 dB = gain of 1.0
        let linear = 10f64.powf(0.0 / 20.0);
        assert!((linear - 1.0).abs() < 1e-10);

        // +6 dB ~ 2.0
        let linear = 10f64.powf(6.0 / 20.0);
        assert!((linear - 1.9953).abs() < 0.01);

        // -6 dB ~ 0.5
        let linear = 10f64.powf(-6.0 / 20.0);
        assert!((linear - 0.5012).abs() < 0.01);
    }

    #[test]
    fn test_apply_gain_zero_db_is_identity() {
        let mut samples = vec![0.5f32, -0.3, 0.8, -1.0];
        let original = samples.clone();
        apply_gain(&mut samples, 0.0, None, 0.0);
        for (a, b) in samples.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_apply_gain_positive() {
        // +6 dB roughly doubles amplitude.
        let mut samples = vec![0.25f32, -0.25];
        apply_gain(&mut samples, 6.0, None, 0.0);
        assert!(samples[0] > 0.49 && samples[0] < 0.51);
        assert!(samples[1] < -0.49 && samples[1] > -0.51);
    }

    #[test]
    fn test_apply_gain_negative_reduces_volume() {
        let mut samples = vec![1.0f32, -1.0];
        apply_gain(&mut samples, -6.0, None, 0.0);
        // -6 dB ~ 0.5
        assert!(samples[0] > 0.49 && samples[0] < 0.52);
        assert!(samples[1] < -0.49 && samples[1] > -0.52);
    }

    #[test]
    fn test_apply_gain_peak_limiting() {
        // Peak = 0.9, gain would push above 1.0 → should be limited.
        let mut samples = vec![0.9f32];
        // +6 dB would double to 1.8, but peak=0.9 means max_gain = 1/0.9 ~ 1.111
        apply_gain(&mut samples, 6.0, Some(0.9), 0.0);
        // Should be clamped: 0.9 * (1/0.9) = 1.0
        assert!(samples[0] <= 1.001);
    }

    #[test]
    fn test_apply_gain_with_preamp() {
        let mut samples = vec![0.5f32];
        // 0 dB gain + 6 dB preamp = +6 dB total.
        apply_gain(&mut samples, 0.0, None, 6.0);
        assert!(samples[0] > 0.99 && samples[0] < 1.01);
    }

    #[test]
    fn test_apply_gain_empty_buffer() {
        let mut samples: Vec<f32> = vec![];
        apply_gain(&mut samples, 6.0, Some(0.5), 3.0);
        assert!(samples.is_empty());
    }

    #[test]
    fn test_select_gain_track_mode() {
        let info = ReplayGainInfo {
            track_gain_db: Some(-3.0),
            track_peak: Some(0.95),
            album_gain_db: Some(-5.0),
            album_peak: Some(0.98),
        };
        let (gain, peak) = select_gain(&info, ReplayGainMode::Track).unwrap();
        assert!((gain - (-3.0)).abs() < 1e-10);
        assert_eq!(peak, Some(0.95));
    }

    #[test]
    fn test_select_gain_album_mode() {
        let info = ReplayGainInfo {
            track_gain_db: Some(-3.0),
            track_peak: Some(0.95),
            album_gain_db: Some(-5.0),
            album_peak: Some(0.98),
        };
        let (gain, peak) = select_gain(&info, ReplayGainMode::Album).unwrap();
        assert!((gain - (-5.0)).abs() < 1e-10);
        assert_eq!(peak, Some(0.98));
    }

    #[test]
    fn test_select_gain_album_falls_back_to_track() {
        let info = ReplayGainInfo {
            track_gain_db: Some(-3.0),
            track_peak: Some(0.95),
            album_gain_db: None,
            album_peak: None,
        };
        let (gain, peak) = select_gain(&info, ReplayGainMode::Album).unwrap();
        assert!((gain - (-3.0)).abs() < 1e-10);
        assert_eq!(peak, Some(0.95));
    }

    #[test]
    fn test_select_gain_off() {
        let info = ReplayGainInfo {
            track_gain_db: Some(-3.0),
            track_peak: Some(0.95),
            album_gain_db: Some(-5.0),
            album_peak: Some(0.98),
        };
        assert!(select_gain(&info, ReplayGainMode::Off).is_none());
    }

    #[test]
    fn test_select_gain_no_data() {
        let info = ReplayGainInfo::default();
        assert!(select_gain(&info, ReplayGainMode::Track).is_none());
        assert!(select_gain(&info, ReplayGainMode::Album).is_none());
    }

    #[test]
    fn test_format_gain() {
        assert_eq!(format_gain(3.21), "+3.21 dB");
        assert_eq!(format_gain(-1.5), "-1.50 dB");
        assert_eq!(format_gain(0.0), "+0.00 dB");
    }

    #[test]
    fn test_format_peak() {
        assert_eq!(format_peak(1.0), "1.000000");
        assert_eq!(format_peak(0.987654), "0.987654");
    }
}
