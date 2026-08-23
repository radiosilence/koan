use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

/// Number of spectrum bars produced by the analyzer.
pub const NUM_BARS: usize = 48;

/// Number of waveform frames carried in each VizFrame for oscilloscope/lissajous modes.
/// 2048 frames (~46ms at 44.1kHz) matches the FFT window size — enough for smooth waveform display.
pub const WAVEFORM_SAMPLES: usize = 2048;

/// Delay-line capacity in interleaved samples.
///
/// The decode thread writes here at the moment it writes into the ring buffer,
/// which is up to a full ring ahead of what the DAC is playing. To show what is
/// being *heard*, the buffer must reach back one whole ring plus the longest
/// window the analyzer asks for.
const DELAY_LINE_SIZE: usize = crate::player::RING_BUFFER_SIZE + WAVEFORM_SAMPLES * 2;

// ── VizFrame / VizSnapshot (high-level UI-facing snapshot API) ────────────────

/// A single frame of analysis output, ready for the UI thread.
///
/// Held inside `VizSnapshot` under an RwLock. The UI thread clones this in
/// <1us (memcpy of 48 floats + 2 floats + 1 float + waveform + Instant) while holding the read lock.
#[derive(Clone)]
pub struct VizFrame {
    /// Spectrum bar heights (0.0..1.0), one per bar. Already smoothed by the analyzer.
    pub spectrum: [f32; NUM_BARS],
    /// Peak hold values (slowly decaying maxima), one per bar. Managed by the analyzer.
    pub peaks: [f32; NUM_BARS],
    /// RMS VU levels: [left, right], each 0.0..1.0.
    pub vu_levels: [f32; 2],
    /// Beat energy (0.0..1.0). Spikes on transients in the low bands,
    /// decays quickly. Used by the TUI for beat-reactive color shifts.
    pub beat_energy: f32,
    /// When this frame was computed.
    pub timestamp: std::time::Instant,
    /// Raw waveform samples for oscilloscope/lissajous rendering.
    /// Interleaved stereo (L, R, L, R...) — `WAVEFORM_SAMPLES` frames = `WAVEFORM_SAMPLES * 2` values.
    /// Empty when no audio is playing.
    pub waveform: Vec<f32>,
}

impl Default for VizFrame {
    fn default() -> Self {
        Self {
            spectrum: [0.0; NUM_BARS],
            peaks: [0.0; NUM_BARS],
            vu_levels: [0.0; 2],
            beat_energy: 0.0,
            timestamp: std::time::Instant::now(),
            waveform: Vec::new(),
        }
    }
}

/// Thread-safe snapshot of the latest analysis frame.
///
/// Written by the analysis thread (~60fps), read by the UI thread every frame.
///
/// Lock discipline:
/// - Writer: compute everything in thread-local scratch, then acquire write lock,
///   swap the frame (~200B memcpy), release. Hold time <1us.
/// - Reader (UI): acquire read lock, clone frame, release. Hold time <1us.
///   All decay/smoothing happens on the local clone with no lock held.
pub struct VizSnapshot {
    inner: RwLock<VizFrame>,
}

impl VizSnapshot {
    /// Create a new snapshot with a zeroed initial frame.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(VizFrame::default()),
        })
    }

    /// Read the latest frame. Acquires read lock, clones, releases — <1us.
    pub fn read(&self) -> VizFrame {
        self.inner.read().clone()
    }

    /// Write a new frame. Acquires write lock, swaps, releases — <1us.
    /// MUST only be called after all FFT computation is finished (never hold lock during FFT).
    pub fn write(&self, frame: VizFrame) {
        *self.inner.write() = frame;
    }
}

// ── Raw sample window (used internally by VizBuffer and VizAnalyzer) ──────────

/// A window of `VizBuffer` contents, bundling raw samples with the metadata
/// needed to interpret them. Filled by `VizBuffer::snapshot_at`.
#[derive(Default)]
pub struct RawVizSnapshot {
    /// Interleaved f32 samples, oldest first.
    pub samples: Vec<f32>,
    /// Channel count for de-interleaving.
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

// ── VizBuffer ────────────────────────────────────────────────────────────────

/// Internal sample storage for the visualization delay line.
struct VizSamples {
    /// Circular buffer of interleaved f32 samples.
    buf: Vec<f32>,
    /// Current write position (wraps around).
    write_pos: usize,
    /// Cumulative interleaved samples ever pushed. Compared against the audio
    /// engine's played counter to find how far back in the buffer "now" is.
    head_offset: u64,
    /// Channel count for de-interleaving.
    channels: u16,
    /// Sample rate for frequency calculations.
    sample_rate: u32,
}

/// Shared visualization delay line.
///
/// Written by the decode thread, read by the analysis thread at ~60fps.
/// Uses `parking_lot::Mutex` — contention is near-zero because the decode
/// thread holds the lock for <50us per write and the analysis thread reads
/// at 16ms intervals.
///
/// The decode thread runs far ahead of the DAC — a local FLAC decodes 50-100x
/// realtime, so the ring buffer saturates within a second of pressing play and
/// stays that way — which means the newest sample here is not the sample being
/// heard. Reads are therefore keyed on the engine's played counter rather than
/// on the write head: see `snapshot_at`. Feeding the buffer from the render
/// callback would sidestep the delay, but that thread may never lock.
pub struct VizBuffer {
    samples: Mutex<VizSamples>,
}

impl VizBuffer {
    /// Create a new visualization buffer.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            samples: Mutex::new(VizSamples {
                buf: vec![0.0; DELAY_LINE_SIZE],
                write_pos: 0,
                head_offset: 0,
                channels: 2,
                sample_rate: 44100,
            }),
        })
    }

    /// Push interleaved samples into the delay line.
    ///
    /// Called by the decode thread as it writes into the ring buffer.
    /// Updates channel count and sample rate if they differ from the
    /// current values (happens on track boundaries).
    pub fn push_samples(&self, samples: &[f32], channels: u16, sample_rate: u32) {
        let mut inner = self.samples.lock();
        inner.channels = channels;
        inner.sample_rate = sample_rate;
        inner.head_offset += samples.len() as u64;

        let buf_len = inner.buf.len();
        if samples.len() >= buf_len {
            // More samples than buffer size — just copy the tail.
            let start = samples.len() - buf_len;
            inner.buf.copy_from_slice(&samples[start..]);
            inner.write_pos = 0;
        } else {
            let pos = inner.write_pos;
            let first = buf_len - pos;
            if samples.len() <= first {
                inner.buf[pos..pos + samples.len()].copy_from_slice(samples);
                inner.write_pos = (pos + samples.len()) % buf_len;
            } else {
                inner.buf[pos..].copy_from_slice(&samples[..first]);
                let remaining = samples.len() - first;
                inner.buf[..remaining].copy_from_slice(&samples[first..]);
                inner.write_pos = remaining;
            }
        }
    }

    /// Clear the delay line and restart its offset at zero.
    ///
    /// Called at the start of a decode session, when the engine's played
    /// counter also restarts at zero.
    pub fn reset(&self) {
        let mut inner = self.samples.lock();
        inner.buf.fill(0.0);
        inner.write_pos = 0;
        inner.head_offset = 0;
    }

    /// Fill `out` with the `frames` frames ending at `played` cumulative
    /// interleaved samples having left the audio engine.
    ///
    /// `played` is the counter the render callback publishes, so the window
    /// ends on the sample currently reaching the DAC rather than on whatever
    /// the decode thread wrote last. History older than the delay line is gone,
    /// so the lookback is clamped to what the buffer still holds.
    pub fn snapshot_at(&self, played: u64, frames: usize, out: &mut RawVizSnapshot) {
        let inner = self.samples.lock();
        let buf_len = inner.buf.len();

        out.channels = inner.channels;
        out.sample_rate = inner.sample_rate;
        out.samples.clear();

        let wanted = frames
            .saturating_mul(inner.channels.max(1) as usize)
            .min(buf_len);
        if wanted == 0 {
            return;
        }

        // How far the write head has run ahead of the play head, capped so the
        // window itself still fits behind it.
        let delay = inner
            .head_offset
            .saturating_sub(played)
            .min((buf_len - wanted) as u64) as usize;
        let end = (inner.write_pos + buf_len - delay) % buf_len;
        let start = (end + buf_len - wanted) % buf_len;

        out.samples.reserve(wanted);
        if start < end {
            out.samples.extend_from_slice(&inner.buf[start..end]);
        } else {
            out.samples.extend_from_slice(&inner.buf[start..]);
            out.samples.extend_from_slice(&inner.buf[..end]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fill the delay line with a ramp so every sample identifies its own index.
    fn push_ramp(buf: &VizBuffer, count: usize) {
        let samples: Vec<f32> = (0..count).map(|i| i as f32).collect();
        buf.push_samples(&samples, 2, 44100);
    }

    #[test]
    fn snapshot_at_head_returns_newest_samples() {
        let buf = VizBuffer::new();
        push_ramp(&buf, 1000);

        // Everything pushed has also been played — the window ends at the head.
        let mut snap = RawVizSnapshot::default();
        buf.snapshot_at(1000, 100, &mut snap);
        assert_eq!(snap.samples.len(), 200);
        for (i, &val) in snap.samples.iter().enumerate() {
            assert_eq!(val, (800 + i) as f32);
        }
    }

    #[test]
    fn snapshot_at_walks_back_to_the_play_head() {
        let buf = VizBuffer::new();
        push_ramp(&buf, 100_000);

        // The decode thread is 40_000 samples ahead of the DAC, so the window
        // must end on sample 60_000, not on the newest sample written.
        let mut snap = RawVizSnapshot::default();
        buf.snapshot_at(60_000, 512, &mut snap);
        assert_eq!(snap.samples.len(), 1024);
        assert_eq!(*snap.samples.last().unwrap(), 59_999.0);
        assert_eq!(snap.samples[0], (60_000 - 1024) as f32);
    }

    #[test]
    fn snapshot_at_tracks_the_play_head_across_wraps() {
        let buf = VizBuffer::new();
        // Push more than the delay line holds so the write position wraps.
        let total = DELAY_LINE_SIZE + 5_000;
        push_ramp(&buf, total);

        let played = (total - 2_000) as u64;
        let mut snap = RawVizSnapshot::default();
        buf.snapshot_at(played, 256, &mut snap);
        assert_eq!(snap.samples.len(), 512);
        assert_eq!(*snap.samples.last().unwrap(), (played - 1) as f32);
        assert_eq!(snap.samples[0], (played - 512) as f32);
    }

    #[test]
    fn snapshot_at_clamps_lookback_to_buffer_length() {
        let buf = VizBuffer::new();
        push_ramp(&buf, DELAY_LINE_SIZE * 2);

        // A play head further back than the delay line reaches: the oldest
        // retained samples are returned rather than a window that runs past
        // the write head into the future.
        let mut snap = RawVizSnapshot::default();
        buf.snapshot_at(0, 64, &mut snap);
        assert_eq!(snap.samples.len(), 128);
        assert_eq!(snap.samples[0], DELAY_LINE_SIZE as f32);
    }

    #[test]
    fn reset_clears_samples_and_offset() {
        let buf = VizBuffer::new();
        push_ramp(&buf, 10_000);
        buf.reset();

        let mut snap = RawVizSnapshot::default();
        buf.snapshot_at(0, 32, &mut snap);
        assert!(snap.samples.iter().all(|&s| s == 0.0));

        // Offset restarted, so a fresh push is back in phase with played = 0.
        push_ramp(&buf, 500);
        buf.snapshot_at(500, 10, &mut snap);
        assert_eq!(*snap.samples.last().unwrap(), 499.0);
    }

    #[test]
    fn snapshot_at_reports_metadata() {
        let buf = VizBuffer::new();
        buf.push_samples(&[1.0, 2.0], 1, 96000);

        let mut snap = RawVizSnapshot::default();
        buf.snapshot_at(2, 2, &mut snap);
        assert_eq!(snap.channels, 1);
        assert_eq!(snap.sample_rate, 96000);
        assert_eq!(snap.samples, vec![1.0, 2.0]);
    }

    #[test]
    fn viz_snapshot_read_write() {
        let snap = VizSnapshot::new();
        let frame = snap.read();
        assert_eq!(frame.spectrum.len(), NUM_BARS);
        assert_eq!(frame.vu_levels, [0.0, 0.0]);

        let mut new_spectrum = [0.0f32; NUM_BARS];
        new_spectrum[5] = 0.9;
        snap.write(VizFrame {
            spectrum: new_spectrum,
            peaks: [0.0; NUM_BARS],
            vu_levels: [0.5, 0.5],
            beat_energy: 0.0,
            timestamp: std::time::Instant::now(),
            waveform: Vec::new(),
        });

        let frame2 = snap.read();
        assert!((frame2.spectrum[5] - 0.9).abs() < 0.001);
        assert!((frame2.vu_levels[0] - 0.5).abs() < 0.001);
    }
}
