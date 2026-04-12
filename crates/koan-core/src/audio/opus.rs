//! Opus decoding bridge — wraps `opus-decoder` to decode packets from
//! Symphonia's Ogg demuxer. Symphonia 0.5 can identify Opus streams but
//! has no codec implementation; this module fills that gap.
//!
//! Opus always decodes to 48 kHz, regardless of the internal sample rate.
//! Channel count is read from the Opus identification header (first packet).

use opus_decoder::OpusDecoder;
use symphonia::core::codecs::CodecParameters;

/// Errors from the Opus decode bridge.
#[derive(Debug, thiserror::Error)]
pub enum OpusError {
    #[error("opus decoder init failed: {0}")]
    Init(String),
    #[error("opus decode error: {0}")]
    Decode(String),
    #[error("invalid opus header")]
    InvalidHeader,
}

/// State for decoding an Opus stream via Symphonia packets.
pub struct OpusBridge {
    decoder: OpusDecoder,
    channels: usize,
    /// Reusable buffer for decoded f32 PCM (interleaved).
    pcm_buf: Vec<f32>,
    /// Samples per channel to skip from the start (Opus pre-skip).
    pre_skip: u32,
    /// Samples per channel already skipped.
    skipped: u32,
    /// Number of Ogg packets seen — first two are header/comment.
    packet_index: u64,
}

/// Opus identification header layout (first 19 bytes minimum):
///   0..8   "OpusHead"
///   8      version (1)
///   9      channel count
///  10..12  pre-skip (little-endian u16)
///  12..16  input sample rate (little-endian u32, informational only)
///  16..18  output gain (little-endian i16)
///  18      channel mapping family
const OPUS_HEAD_MAGIC: &[u8] = b"OpusHead";
const OPUS_HEAD_MIN_LEN: usize = 19;

/// Parse the Opus identification header to extract channel count and pre-skip.
fn parse_opus_head(data: &[u8]) -> Result<(usize, u32), OpusError> {
    if data.len() < OPUS_HEAD_MIN_LEN || &data[..8] != OPUS_HEAD_MAGIC {
        return Err(OpusError::InvalidHeader);
    }
    let channels = data[9] as usize;
    let pre_skip = u16::from_le_bytes([data[10], data[11]]) as u32;
    Ok((channels, pre_skip))
}

impl OpusBridge {
    /// Create a new Opus decoder from Symphonia codec parameters.
    ///
    /// The `extra_data` in `CodecParameters` should contain the Opus
    /// identification header (OpusHead). If not present, falls back to
    /// channel count from codec params.
    pub fn new(params: &CodecParameters) -> Result<Self, OpusError> {
        // Try to get channel count and pre-skip from the OpusHead extra data.
        let (channels, pre_skip) = if let Some(extra) = &params.extra_data {
            parse_opus_head(extra)?
        } else {
            // Fallback: use codec params channel count, assume no pre-skip.
            let ch = params.channels.map(|c| c.count()).unwrap_or(2);
            (ch, 0)
        };

        if channels == 0 || channels > 2 {
            // opus-decoder only supports mono/stereo. Multistream would need
            // OpusMultistreamDecoder, which we don't handle yet.
            return Err(OpusError::Init(format!(
                "unsupported channel count: {channels} (only mono/stereo supported)"
            )));
        }

        let decoder =
            OpusDecoder::new(48000, channels).map_err(|e| OpusError::Init(format!("{e:?}")))?;

        // Max frame size: 120ms at 48kHz = 5760 samples/channel.
        let max_samples = 5760 * channels;
        let pcm_buf = vec![0.0f32; max_samples];

        Ok(Self {
            decoder,
            channels,
            pcm_buf,
            pre_skip,
            skipped: 0,
            packet_index: 0,
        })
    }

    /// Channel count for this stream.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Decode one Symphonia packet. Returns a slice of interleaved f32 PCM
    /// samples, or an empty slice for header/comment packets.
    ///
    /// Handles pre-skip trimming: the first N samples (per Opus spec) are
    /// silently discarded.
    pub fn decode_packet(&mut self, data: &[u8]) -> Result<&[f32], OpusError> {
        let idx = self.packet_index;
        self.packet_index += 1;

        // First packet = OpusHead, second = OpusTags — skip both.
        if idx < 2 {
            return Ok(&[]);
        }

        let frames_per_channel = self
            .decoder
            .decode_float(data, &mut self.pcm_buf, false)
            .map_err(|e| OpusError::Decode(format!("{e:?}")))?;

        let total_samples = frames_per_channel * self.channels;

        // Handle pre-skip: discard the first `pre_skip` samples per channel.
        let start = if self.skipped < self.pre_skip {
            let remaining = (self.pre_skip - self.skipped) as usize;
            let skip_frames = remaining.min(frames_per_channel);
            self.skipped += skip_frames as u32;
            skip_frames * self.channels
        } else {
            0
        };

        Ok(&self.pcm_buf[start..total_samples])
    }

    /// Reset the decoder state (e.g. after a seek).
    pub fn reset(&mut self) {
        self.decoder.reset();
        self.skipped = self.pre_skip; // After seek, pre-skip already applied.
        // Don't reset packet_index — Symphonia handles seek by jumping to
        // audio packets, not re-sending headers.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_opus_head_valid() {
        // Minimal valid OpusHead: stereo, pre-skip=312
        let mut header = vec![0u8; 19];
        header[..8].copy_from_slice(b"OpusHead");
        header[8] = 1; // version
        header[9] = 2; // channels
        header[10] = 0x38; // pre_skip = 312 (0x0138)
        header[11] = 0x01;
        // rest is zeros (sample rate, gain, mapping family)

        let (channels, pre_skip) = parse_opus_head(&header).unwrap();
        assert_eq!(channels, 2);
        assert_eq!(pre_skip, 312);
    }

    #[test]
    fn test_parse_opus_head_mono() {
        let mut header = vec![0u8; 19];
        header[..8].copy_from_slice(b"OpusHead");
        header[8] = 1;
        header[9] = 1; // mono
        header[10] = 0x00;
        header[11] = 0x00;

        let (channels, pre_skip) = parse_opus_head(&header).unwrap();
        assert_eq!(channels, 1);
        assert_eq!(pre_skip, 0);
    }

    #[test]
    fn test_parse_opus_head_invalid_magic() {
        let header = b"NotOpusHead_padding";
        assert!(parse_opus_head(header).is_err());
    }

    #[test]
    fn test_parse_opus_head_too_short() {
        let header = b"OpusHea"; // 7 bytes
        assert!(parse_opus_head(header).is_err());
    }

    #[test]
    fn test_opus_bridge_new_stereo() {
        // Build minimal CodecParameters with OpusHead extra data.
        let mut header = vec![0u8; 19];
        header[..8].copy_from_slice(b"OpusHead");
        header[8] = 1;
        header[9] = 2; // stereo
        header[10] = 0x38;
        header[11] = 0x01; // pre_skip=312

        let mut params = CodecParameters::new();
        params.with_extra_data(header.into_boxed_slice());

        let bridge = OpusBridge::new(&params).unwrap();
        assert_eq!(bridge.channels(), 2);
    }

    #[test]
    fn test_opus_bridge_rejects_multichannel() {
        let mut header = vec![0u8; 19];
        header[..8].copy_from_slice(b"OpusHead");
        header[8] = 1;
        header[9] = 6; // 5.1 surround — not supported
        header[10] = 0x00;
        header[11] = 0x00;

        let mut params = CodecParameters::new();
        params.with_extra_data(header.into_boxed_slice());

        assert!(OpusBridge::new(&params).is_err());
    }
}
