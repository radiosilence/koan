use std::sync::Arc;

use parking_lot::Mutex;

/// Default buffer size: 4096 samples covers ~93ms at 44.1kHz,
/// enough for a 2048-point FFT window with room to spare.
const DEFAULT_BUFFER_SIZE: usize = 4096;

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
/// Written by the decode thread, read by the TUI at ~20fps.
/// Uses `parking_lot::Mutex` — contention is near-zero because the decode
/// thread holds the lock for <50us per write and the TUI reads at 50ms
/// intervals.
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
    /// Called by the TUI thread at ~20fps. Returns a contiguous `Vec<f32>`
    /// with the most recent samples in chronological order.
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
}
