use std::path::Path;

use lofty::prelude::*;
use lofty::tag::ItemValue;
use thiserror::Error;

use crate::config::ReplayGainMode;

#[derive(Debug, Error)]
pub enum ReplayGainError {
    #[error("tag error: {0}")]
    Tag(String),
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
///
/// Taggers are inconsistent about the suffix's case, and a value that fails to
/// parse silently disables ReplayGain for the file.
fn parse_gain(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = match s.get(s.len().saturating_sub(2)..) {
        Some(suffix) if suffix.eq_ignore_ascii_case("db") => &s[..s.len() - 2],
        _ => s,
    };
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
///
/// Nothing downstream of here clamps — the samples go to the ring buffer and
/// out to the DAC — so this is the last chance to keep a badly tagged file
/// (missing, zero, negative or absurd peak; unbounded pre-amp) from being sent
/// out of range.
pub fn apply_gain(samples: &mut [f32], gain_db: f64, peak: Option<f64>, pre_amp_db: f64) {
    let linear_gain = 10f64.powf((gain_db + pre_amp_db) / 20.0);

    // Only a finite, positive peak says anything about the file's headroom.
    let limited_gain = match peak {
        Some(peak) if peak.is_finite() && peak > 0.0 => linear_gain.min(1.0 / peak),
        _ => linear_gain,
    };
    // A non-finite gain would put NaN through the clamp untouched.
    let gain_f32 = if limited_gain.is_finite() {
        limited_gain as f32
    } else {
        1.0
    };

    for sample in samples.iter_mut() {
        *sample = (*sample * gain_f32).clamp(-1.0, 1.0);
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
    fn test_parse_gain_suffix_is_case_insensitive() {
        assert_eq!(parse_gain("+3.21 db"), Some(3.21));
        assert_eq!(parse_gain("-1.50 DB"), Some(-1.50));
        assert_eq!(parse_gain("2.00Db"), Some(2.0));
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
    fn test_apply_gain_without_peak_never_clips() {
        // A quiet recording tagged +20 dB with no peak tag: ~10x gain, which
        // would previously have gone to the DAC unbounded.
        let mut samples = vec![0.9f32, -0.9];
        apply_gain(&mut samples, 20.0, None, 0.0);
        assert!(samples.iter().all(|s| (-1.0..=1.0).contains(s)));
        assert_eq!(samples, vec![1.0, -1.0]);
    }

    #[test]
    fn test_apply_gain_negative_peak_does_not_invert_phase() {
        // A malformed peak tag must not turn 1/peak into a negative gain.
        let mut samples = vec![0.5f32, -0.5];
        apply_gain(&mut samples, 0.0, Some(-0.8), 0.0);
        assert!(samples[0] > 0.0 && samples[1] < 0.0);
    }

    #[test]
    fn test_apply_gain_ignores_useless_peaks() {
        // Zero and NaN peaks say nothing about headroom; the clamp still holds.
        for peak in [Some(0.0), Some(f64::NAN), Some(f64::INFINITY)] {
            let mut samples = vec![0.9f32];
            apply_gain(&mut samples, 12.0, peak, 0.0);
            assert_eq!(samples[0], 1.0, "peak {:?} let the output run away", peak);
        }
    }

    #[test]
    fn test_apply_gain_survives_absurd_preamp() {
        let mut samples = vec![0.5f32, -0.5];
        apply_gain(&mut samples, 0.0, None, 1000.0);
        assert!(
            samples
                .iter()
                .all(|s| s.is_finite() && (-1.0..=1.0).contains(s))
        );
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
}
