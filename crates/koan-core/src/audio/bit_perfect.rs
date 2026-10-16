//! Bit-perfect output on Android: which of a USB DAC's mixer formats a
//! stream can pass through untouched, and the app that asks Android for it.
//!
//! Android mixes every app's audio at one rate and applies volume, unless an
//! app sets the USB device's preferred mixer attributes with
//! `MIXER_BEHAVIOR_BIT_PERFECT` (Android 14). The mixer then hands that app's
//! stream to the device unchanged, provided the stream matches the attributes
//! exactly: rate, channels and sample encoding. Those are Java APIs, so the
//! app answers for them through [`OutputHost`]; what to ask for is decided
//! here, where it can be tested.

use std::sync::Arc;

use parking_lot::RwLock;

/// A sample encoding a mixer can run in, and an AAudio stream can be opened
/// with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    I16,
    I24Packed,
    I32,
    Float,
}

impl Encoding {
    /// The integer bits a sample keeps on its way out in this encoding. The
    /// engine's samples are floats, which hold a 24-bit integer exactly and
    /// no more, so a 32-bit integer carries 24 of them.
    pub fn bits(self) -> u16 {
        match self {
            Self::I16 => 16,
            Self::I24Packed | Self::I32 | Self::Float => 24,
        }
    }

    /// Bytes per sample in an AAudio buffer.
    pub fn bytes(self) -> usize {
        match self {
            Self::I16 => 2,
            Self::I24Packed => 3,
            Self::I32 | Self::Float => 4,
        }
    }

    /// `AAUDIO_FORMAT_*`.
    pub fn aaudio(self) -> i32 {
        match self {
            Self::I16 => 1,
            Self::Float => 2,
            Self::I24Packed => 3,
            Self::I32 => 4,
        }
    }
}

/// One bit-perfect configuration a USB output offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MixerFormat {
    /// `AudioDeviceInfo.getId()`, which the stream is opened on.
    pub device_id: i32,
    pub rate: u32,
    pub channels: u16,
    pub encoding: Encoding,
}

/// The app's side of bit-perfect output: Android's `AudioManager`.
///
/// Called on the player thread, before a stream opens.
pub trait OutputHost: Send + Sync {
    /// The rate the system mixer runs at: what a stream not passed through is
    /// resampled to.
    fn mixer_rate(&self) -> u32;
    /// The bit-perfect formats of the USB output in use. Empty when the route
    /// is not USB, the system is older than Android 14, or the person has
    /// turned bit-perfect output off.
    fn offers(&self) -> Vec<MixerFormat>;
    /// Ask Android to pass streams of `format` through untouched; `false` if
    /// it refused.
    fn hold(&self, format: MixerFormat) -> bool;
    /// Give the mixer back to the system.
    fn release(&self);
}

static HOST: RwLock<Option<Arc<dyn OutputHost>>> = RwLock::new(None);

/// Install the app's host. Until one is installed, output is mixed.
pub fn set_host(host: Arc<dyn OutputHost>) {
    *HOST.write() = Some(host);
}

pub(crate) fn host() -> Option<Arc<dyn OutputHost>> {
    HOST.read().clone()
}

/// The format to hold for a stream at `rate` with `channels`, whose samples
/// carry `bit_depth` integer bits (`None` for a lossy source, or one a gain or
/// DSP has processed, which needs 24).
///
/// Only an exact rate and channel count will do: anything else is the mixer's
/// work again. Of the encodings that keep every bit, the widest integer one is
/// preferred: a 16-bit sample zero-padded to 24 or 32 bits reaches the DAC
/// unchanged, and the next track of a gapless run, perhaps of more bits, still
/// fits. A float comes last, leaving the final conversion to the driver.
/// Nothing above 24 bits survives the engine's floats, so a 32-bit source is
/// never bit-perfect.
pub fn choose(
    offers: &[MixerFormat],
    rate: u32,
    channels: u16,
    bit_depth: Option<u16>,
) -> Option<MixerFormat> {
    let needed = bit_depth.unwrap_or(24);
    if needed > 24 {
        return None;
    }
    let rank = |e: Encoding| match e {
        Encoding::I32 => 0,
        Encoding::I24Packed => 1,
        Encoding::I16 => 2,
        Encoding::Float => 3,
    };
    offers
        .iter()
        .filter(|f| f.rate == rate && f.channels == channels && f.encoding.bits() >= needed)
        .min_by_key(|f| rank(f.encoding))
        .copied()
}

/// Write `samples` into an AAudio buffer of `encoding`. Exact for any sample
/// that came from an integer of at most the encoding's bits: the decoder
/// scales an n-bit integer by 2^(n-1), which this undoes.
pub fn encode(samples: &[f32], encoding: Encoding, out: &mut [u8]) {
    match encoding {
        Encoding::Float => {
            for (s, o) in samples.iter().zip(out.as_chunks_mut::<4>().0) {
                o.copy_from_slice(&s.to_ne_bytes());
            }
        }
        Encoding::I16 => {
            for (s, o) in samples.iter().zip(out.as_chunks_mut::<2>().0) {
                o.copy_from_slice(&(scale(*s, 15) as i16).to_ne_bytes());
            }
        }
        Encoding::I24Packed => {
            for (s, o) in samples.iter().zip(out.as_chunks_mut::<3>().0) {
                let v = scale(*s, 23) as i32;
                o.copy_from_slice(&v.to_le_bytes()[..3]);
            }
        }
        Encoding::I32 => {
            for (s, o) in samples.iter().zip(out.as_chunks_mut::<4>().0) {
                o.copy_from_slice(&(scale(*s, 31) as i32).to_ne_bytes());
            }
        }
    }
}

/// `s` as a signed integer of `bits + 1` bits, clamped to its range.
fn scale(s: f32, bits: u32) -> i64 {
    let full = (1i64 << bits) as f64;
    ((s as f64) * full).round().clamp(-full, full - 1.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(rate: u32, channels: u16, encoding: Encoding) -> MixerFormat {
        MixerFormat {
            device_id: 7,
            rate,
            channels,
            encoding,
        }
    }

    #[test]
    fn nothing_offered_is_the_mixer() {
        assert_eq!(choose(&[], 44100, 2, Some(16)), None);
    }

    #[test]
    fn only_the_source_rate_and_channels_will_do() {
        let offers = [
            offer(48000, 2, Encoding::I24Packed),
            offer(44100, 1, Encoding::I24Packed),
        ];
        assert_eq!(choose(&offers, 44100, 2, Some(16)), None);
    }

    #[test]
    fn the_widest_integer_encoding_so_a_deeper_track_still_fits() {
        let all = [
            offer(44100, 2, Encoding::I16),
            offer(44100, 2, Encoding::Float),
            offer(44100, 2, Encoding::I24Packed),
            offer(44100, 2, Encoding::I32),
        ];
        let chosen = |offers: &[MixerFormat]| choose(offers, 44100, 2, Some(16)).unwrap().encoding;
        assert_eq!(chosen(&all), Encoding::I32);
        assert_eq!(chosen(&all[..3]), Encoding::I24Packed);
        assert_eq!(chosen(&all[..1]), Encoding::I16);
    }

    #[test]
    fn a_32_bit_source_is_never_bit_perfect() {
        let offers = [offer(44100, 2, Encoding::I32)];
        assert_eq!(choose(&offers, 44100, 2, Some(32)), None);
    }

    #[test]
    fn sixteen_bits_cannot_carry_twenty_four() {
        let offers = [offer(96000, 2, Encoding::I16)];
        assert_eq!(choose(&offers, 96000, 2, Some(24)), None);
    }

    #[test]
    fn a_float_mixer_serves_when_no_integer_one_will() {
        let offers = [
            offer(96000, 2, Encoding::I16),
            offer(96000, 2, Encoding::Float),
        ];
        assert_eq!(
            choose(&offers, 96000, 2, Some(24)).unwrap().encoding,
            Encoding::Float
        );
    }

    #[test]
    fn a_source_without_a_bit_depth_needs_twenty_four() {
        let offers = [
            offer(48000, 2, Encoding::I16),
            offer(48000, 2, Encoding::I24Packed),
        ];
        assert_eq!(
            choose(&offers, 48000, 2, None).unwrap().encoding,
            Encoding::I24Packed
        );
        assert_eq!(choose(&offers[..1], 48000, 2, None), None);
    }

    #[test]
    fn encoding_gives_back_the_integers_the_decoder_scaled() {
        for &n in &[-8_388_608i32, -1, 0, 1, 4_660_803, 8_388_607] {
            let s = n as f32 / 8_388_608.0;
            let mut out = [0u8; 3];
            encode(&[s], Encoding::I24Packed, &mut out);
            let back = i32::from_le_bytes([
                out[0],
                out[1],
                out[2],
                if out[2] & 0x80 != 0 { 0xff } else { 0 },
            ]);
            assert_eq!(back, n);

            let mut out = [0u8; 4];
            encode(&[s], Encoding::I32, &mut out);
            assert_eq!(i32::from_ne_bytes(out), n << 8);
        }
        for &n in &[-32768i16, -1, 0, 1, 12345, 32767] {
            let mut out = [0u8; 2];
            encode(&[n as f32 / 32768.0], Encoding::I16, &mut out);
            assert_eq!(i16::from_ne_bytes(out), n);
        }
    }

    #[test]
    fn full_scale_clamps_rather_than_wraps() {
        let mut out = [0u8; 2];
        encode(&[1.0], Encoding::I16, &mut out);
        assert_eq!(i16::from_ne_bytes(out), i16::MAX);
    }
}
