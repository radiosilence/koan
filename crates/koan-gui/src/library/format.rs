//! How figures read on the pages, as the Mac app writes them.

use koan_runtime::Track;

/// "3:07", "1:02:34", "--:--" for an unknown length.
pub fn duration(ms: Option<i64>) -> String {
    ms.filter(|ms| *ms > 0)
        .map_or_else(|| "--:--".into(), |ms| crate::time::clock(ms as u64))
}

/// What a track is, as its row shows it: "FLAC 16/44.1", or "MP3 320 kbps"
/// for a lossy file, whose encoder setting says more than its rate does.
pub fn quality(t: &Track) -> Option<String> {
    let codec = t.codec.as_ref()?.to_uppercase();
    Some(match (t.bit_depth, t.sample_rate, t.bitrate) {
        (Some(depth), Some(rate), _) => format!("{codec} {depth}/{}", khz(rate)),
        (_, _, Some(kbps)) if kbps > 0 && LOSSY.contains(&codec.as_str()) => {
            format!("{codec} {kbps} kbps")
        }
        (_, Some(rate), _) => format!("{codec} {} kHz", khz(rate)),
        _ => codec,
    })
}

/// Codecs whose encoder setting, not the rate, says what a file without a
/// bit depth is. Named rather than inferred from a missing bit depth, which a
/// server can leave out for FLAC too.
const LOSSY: [&str; 12] = [
    "MP3", "AAC", "M4A", "VORBIS", "OPUS", "OGG", "OGA", "WMA", "MPC", "MP2", "AC3", "SPX",
];

/// 44100 → "44.1", 96000 → "96".
fn khz(hz: i32) -> String {
    let khz = hz as f64 / 1000.0;
    if khz.fract() == 0.0 {
        format!("{khz:.0}")
    } else {
        format!("{khz:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn durations_read_as_the_mac_writes_them() {
        assert_eq!(duration(Some(187_000)), "3:07");
        assert_eq!(duration(Some(3_754_000)), "1:02:34");
        assert_eq!(duration(None), "--:--");
        assert_eq!(duration(Some(0)), "--:--");
    }

    #[test]
    fn rates_drop_a_trailing_zero() {
        assert_eq!(khz(44_100), "44.1");
        assert_eq!(khz(96_000), "96");
    }
}
