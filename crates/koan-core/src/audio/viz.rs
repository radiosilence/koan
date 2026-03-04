use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

/// Default buffer size: 4096 samples covers ~93ms at 44.1kHz,
/// enough for a 2048-point FFT window with room to spare.
const DEFAULT_BUFFER_SIZE: usize = 4096;

/// Number of spectrum bars produced by the analyzer.
pub const NUM_BARS: usize = 48;

// ── Analysis output types (used by both analyzer.rs and visualizer.rs) ───────

/// The output of one analysis pass: spectrum bars, peak holds, and VU levels.
/// Written by `VizAnalyzer` on its background thread; read by the TUI thread.
#[derive(Clone)]
pub struct AnalysisOutput {
    /// Spectrum bar heights (0.0..1.0), one per bar.
    pub spectrum: [f32; NUM_BARS],
    /// Peak hold values (slowly decaying maxima), one per bar.
    pub peaks: [f32; NUM_BARS],
    /// RMS VU levels: [left, right], each 0.0..1.0.
    pub vu_levels: [f32; 2],
}

impl Default for AnalysisOutput {
    fn default() -> Self {
        Self {
            spectrum: [0.0; NUM_BARS],
            peaks: [0.0; NUM_BARS],
            vu_levels: [0.0; 2],
        }
    }
}

/// Shared, lock-protected analysis output.
/// The background analysis thread writes here; the TUI reads a clone each frame.
pub type SharedAnalysisOutput = Arc<Mutex<AnalysisOutput>>;

// ── VizFrame / VizSnapshot (high-level UI-facing snapshot API) ────────────────

/// A single frame of analysis output, ready for the UI thread.
///
/// Held inside `VizSnapshot` under an RwLock. The UI thread clones this in
/// <1us (memcpy of 48 floats + 2 floats + Instant) while holding the read lock.
#[derive(Clone)]
pub struct VizFrame {
    /// Spectrum bar heights (0.0..1.0), one per bar.
    pub spectrum: [f32; NUM_BARS],
    /// RMS VU levels: [left, right], each 0.0..1.0.
    pub vu_levels: [f32; 2],
    /// When this frame was computed.
    pub timestamp: std::time::Instant,
}

impl Default for VizFrame {
    fn default() -> Self {
        Self {
            spectrum: [0.0; NUM_BARS],
            vu_levels: [0.0; 2],
            timestamp: std::time::Instant::now(),
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

impl Default for VizSnapshot {
    fn default() -> Self {
        Self {
            inner: RwLock::new(VizFrame::default()),
        }
    }
}

// ── Raw sample snapshot (used internally by VizBuffer and VizAnalyzer) ────────

/// A point-in-time snapshot of VizBuffer contents, bundling raw samples with
/// the metadata needed to interpret them. Produced by `VizBuffer::snapshot_with_meta`.
pub struct RawVizSnapshot {
    /// Interleaved f32 samples, oldest first.
    pub samples: Vec<f32>,
    /// Channel count for de-interleaving.
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

// ── VizBuffer ────────────────────────────────────────────────────────────────

/// Internal sample storage for the visualization buffer.
struct VizSamples {
    /// Circular buffer of interleaved f32 samples.
    buf: Vec<f32>,
    /// Current write position (wraps around).
    write_pos: usize,
    /// Channel count for de-interleaving.
    channels: u16,
    /// Sample rate for frequency calculations.
    sample_rate: u32,
}

/// Shared visualization sample buffer.
///
/// Written by the decode thread, read by the analysis thread at ~60fps.
/// Uses `parking_lot::Mutex` — contention is near-zero because the decode
/// thread holds the lock for <50us per write and the analysis thread reads
/// at 16ms intervals.
pub struct VizBuffer {
    samples: Mutex<VizSamples>,
}

impl VizBuffer {
    /// Create a new visualization buffer with the default size.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            samples: Mutex::new(VizSamples {
                buf: vec![0.0; DEFAULT_BUFFER_SIZE],
                write_pos: 0,
                channels: 2,
                sample_rate: 44100,
            }),
        })
    }

    /// Push interleaved samples into the circular buffer.
    ///
    /// Called by the decode thread after each packet decode.
    /// Updates channel count and sample rate if they differ from the
    /// current values (happens on track boundaries).
    pub fn push_samples(&self, samples: &[f32], channels: u16, sample_rate: u32) {
        let mut inner = self.samples.lock();
        inner.channels = channels;
        inner.sample_rate = sample_rate;

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

    /// Take a snapshot of the current buffer contents, ordered oldest to newest.
    ///
    /// Returns a contiguous `Vec<f32>` with the most recent samples in chronological order.
    pub fn snapshot(&self) -> Vec<f32> {
        let inner = self.samples.lock();
        let buf_len = inner.buf.len();
        let pos = inner.write_pos;
        let mut out = Vec::with_capacity(buf_len);
        // Write position is where the *next* sample goes, so the oldest
        // sample is at write_pos and the newest is at write_pos - 1.
        out.extend_from_slice(&inner.buf[pos..]);
        out.extend_from_slice(&inner.buf[..pos]);
        out
    }

    /// Take a snapshot bundled with metadata (channels, sample_rate).
    ///
    /// Acquires the lock once to copy both samples and metadata atomically,
    /// so the caller never sees mismatched channel/rate values.
    pub fn snapshot_with_meta(&self) -> RawVizSnapshot {
        let inner = self.samples.lock();
        let buf_len = inner.buf.len();
        let pos = inner.write_pos;
        let mut samples = Vec::with_capacity(buf_len);
        samples.extend_from_slice(&inner.buf[pos..]);
        samples.extend_from_slice(&inner.buf[..pos]);
        RawVizSnapshot {
            samples,
            channels: inner.channels,
            sample_rate: inner.sample_rate,
        }
    }

    /// Current channel count.
    pub fn channels(&self) -> u16 {
        self.samples.lock().channels
    }

    /// Current sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.samples.lock().sample_rate
    }
}

impl Default for VizBuffer {
    fn default() -> Self {
        // Cannot return Arc<Self> from Default, so this creates the inner value.
        // Callers should prefer VizBuffer::new() which returns Arc<VizBuffer>.
        Self {
            samples: Mutex::new(VizSamples {
                buf: vec![0.0; DEFAULT_BUFFER_SIZE],
                write_pos: 0,
                channels: 2,
                sample_rate: 44100,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_snapshot_basic() {
        let buf = VizBuffer::new();
        let samples: Vec<f32> = (0..100).map(|i| i as f32).collect();
        buf.push_samples(&samples, 2, 44100);

        let snap = buf.snapshot();
        assert_eq!(snap.len(), DEFAULT_BUFFER_SIZE);
        // Last 100 samples should be 0..100, preceded by zeros.
        let tail = &snap[DEFAULT_BUFFER_SIZE - 100..];
        for (i, &val) in tail.iter().enumerate() {
            assert_eq!(val, i as f32);
        }
    }

    #[test]
    fn push_wraps_around() {
        let buf = VizBuffer::new();
        // Fill the buffer completely.
        let samples: Vec<f32> = (0..DEFAULT_BUFFER_SIZE as u32).map(|i| i as f32).collect();
        buf.push_samples(&samples, 2, 44100);

        // Push more to wrap.
        let extra: Vec<f32> = (0..10).map(|i| (i + 1000) as f32).collect();
        buf.push_samples(&extra, 2, 44100);

        let snap = buf.snapshot();
        // Newest 10 samples should be 1000..1010.
        let tail = &snap[DEFAULT_BUFFER_SIZE - 10..];
        for (i, &val) in tail.iter().enumerate() {
            assert_eq!(val, (i + 1000) as f32);
        }
    }

    #[test]
    fn push_larger_than_buffer() {
        let buf = VizBuffer::new();
        let big: Vec<f32> = (0..(DEFAULT_BUFFER_SIZE + 500) as u32)
            .map(|i| i as f32)
            .collect();
        buf.push_samples(&big, 2, 48000);

        let snap = buf.snapshot();
        assert_eq!(snap.len(), DEFAULT_BUFFER_SIZE);
        // Should contain the last DEFAULT_BUFFER_SIZE samples.
        for (i, &val) in snap.iter().enumerate() {
            assert_eq!(val, (i + 500) as f32);
        }
        assert_eq!(buf.sample_rate(), 48000);
    }

    #[test]
    fn channels_and_sample_rate() {
        let buf = VizBuffer::new();
        assert_eq!(buf.channels(), 2);
        assert_eq!(buf.sample_rate(), 44100);

        buf.push_samples(&[1.0, 2.0], 1, 96000);
        assert_eq!(buf.channels(), 1);
        assert_eq!(buf.sample_rate(), 96000);
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
            vu_levels: [0.5, 0.5],
            timestamp: std::time::Instant::now(),
        });

        let frame2 = snap.read();
        assert!((frame2.spectrum[5] - 0.9).abs() < 0.001);
        assert!((frame2.vu_levels[0] - 0.5).abs() < 0.001);
    }
}
