use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::{Condvar, Mutex, RwLock};
use tokio::sync::watch;

/// Number of spectrum bars produced by the analyzer.
pub const NUM_BARS: usize = 48;

/// Where the analyser runs until something tells it otherwise. A snapshot is
/// created before any client has said what display it draws on.
const DEFAULT_FPS: u8 = 60;

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
    /// Bumped by every read. The analyser watches it to tell whether anyone is
    /// actually drawing the spectrum — a client can link the engine without
    /// ever opening a visualiser, and an FFT sixty times a second for nobody
    /// is a percent of a core.
    reads: AtomicU64,
    /// Bumped by every published frame, and the thing a subscriber waits on.
    /// Nothing is sent through the channel but the count: a frame is a whole
    /// snapshot behind a lock, so a waiter that misses two publishes wants the
    /// newest one, not the two it slept through.
    published: watch::Sender<u64>,
    /// Where the analyser waits when there is nothing to analyse for, and how
    /// it is woken. Parking rather than looking again on a timer is what makes
    /// an idle koan cost nothing at all: the thread is not scheduled until a
    /// reader arrives or playback starts.
    ///
    /// The flag under the lock is a wake that has already happened. It is what
    /// makes a wake arriving in the moment before the thread parks — the whole
    /// width of the race, and playback starting is exactly when it would land
    /// — a park that returns immediately rather than one nothing will end.
    park: Mutex<bool>,
    unpark: Condvar,
    /// Read on every frame read, so waking a parked analyser costs nothing on
    /// the path that does not need it.
    parked: AtomicBool,
    /// Microseconds between analysis passes.
    ///
    /// It lives beside the frame because the party who knows the right rate is
    /// the party reading frames: bars are drawn on a display, and the rate
    /// worth running at is that display's — 60 on one panel, 120 on another,
    /// and it changes when a window is dragged between them. The analyser
    /// reads it when it next wakes, so setting it costs one store and wakes
    /// nothing.
    interval_us: AtomicU64,
}

impl VizSnapshot {
    /// Create a new snapshot with a zeroed initial frame.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(VizFrame::default()),
            reads: AtomicU64::new(0),
            published: watch::Sender::new(0),
            park: Mutex::new(false),
            unpark: Condvar::new(),
            parked: AtomicBool::new(false),
            interval_us: AtomicU64::new(Self::interval_us(DEFAULT_FPS)),
        })
    }

    /// Read the latest frame. Acquires read lock, clones, releases — <1us.
    pub fn read(&self) -> VizFrame {
        self.touch();
        self.inner.read().clone()
    }

    /// Say that someone wants frames: counted, and enough to wake an analyser
    /// that had parked for want of a reader.
    ///
    /// A subscriber calls this before it waits. Waiting for a frame is wanting
    /// one, and an analyser that stood down because nobody was reading would
    /// otherwise never produce the frame being waited for.
    pub fn touch(&self) {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if self.parked.load(Ordering::Relaxed) {
            self.wake();
        }
    }

    /// Wake the analysis thread if it is parked.
    ///
    /// Called when playback starts: the play head is the one input the
    /// analyser cannot be signalled from, because it is written by the audio
    /// render callback, which may never take a lock.
    pub fn wake(&self) {
        let mut pending = self.park.lock();
        *pending = true;
        self.unpark.notify_all();
    }

    /// Park until something wants a frame again, or until `still_idle` is no
    /// longer true at the moment the lock is held.
    ///
    /// The condition is checked under the same lock `wake` takes, so a reader
    /// arriving between the check and the wait cannot be missed.
    pub fn park_while_idle(&self, still_idle: impl Fn() -> bool) {
        let mut pending = self.park.lock();
        if std::mem::take(&mut pending) || !still_idle() {
            return;
        }
        self.parked.store(true, Ordering::Relaxed);
        while !*pending {
            self.unpark.wait(&mut pending);
        }
        *pending = false;
        self.parked.store(false, Ordering::Relaxed);
    }

    /// A receiver that wakes on each published frame. See `published`.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.published.subscribe()
    }

    /// How many times the frame has been looked at, by either route. Only the
    /// analyser cares — a count that stops moving means nothing is watching,
    /// and there is nothing to analyse for.
    pub fn reads(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }

    /// Run the analyser at `fps` passes a second from its next wake.
    ///
    /// Clamped to something a display could plausibly ask for: the rate is set
    /// from outside koan, and a zero here would be a division and a spin.
    pub fn set_fps(&self, fps: u8) {
        self.interval_us
            .store(Self::interval_us(fps), Ordering::Relaxed);
    }

    /// How long the analyser sleeps between passes.
    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_micros(self.interval_us.load(Ordering::Relaxed))
    }

    /// Passes a second, as last set.
    pub fn fps(&self) -> u8 {
        (1_000_000 / self.interval_us.load(Ordering::Relaxed).max(1)) as u8
    }

    fn interval_us(fps: u8) -> u64 {
        1_000_000 / fps.clamp(1, 240) as u64
    }

    /// Write a new frame. Acquires write lock, swaps, releases — <1us.
    /// MUST only be called after all FFT computation is finished (never hold lock during FFT).
    pub fn write(&self, frame: VizFrame) {
        *self.inner.write() = frame;
        // After the frame is in place, so a woken subscriber reads the frame
        // it was told about rather than the one before it.
        self.published.send_modify(|version| *version += 1);
    }

    /// Reduce the latest frame to three bands.
    ///
    /// Takes the same lock as `read()` but clones nothing — the waveform is the
    /// expensive part of a frame by an order of magnitude, and a caller drawing
    /// three bars would only average it away.
    pub fn levels(&self) -> VizLevels {
        self.touch();
        let frame = self.inner.read();
        VizLevels::of(&frame.spectrum)
    }
}

/// The spectrum reduced to low, mid and high energy.
///
/// A playing indicator wants a few numbers a few dozen times a second, not 48
/// bands, peak holds and a 2048-frame waveform window. Both come off the same
/// analyser; this is the one for callers that poll often and draw little.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VizLevels {
    /// Mean energy across the bottom third of the bars, 0.0..1.0.
    pub low: f32,
    /// Mean energy across the middle third.
    pub mid: f32,
    /// Mean energy across the top third.
    pub high: f32,
}

impl VizLevels {
    /// Split the bars into equal thirds and average each.
    ///
    /// Thirds of the bar range, not of the frequency range: the bars are laid
    /// out on whatever perceptual scale the analyser is configured for, so
    /// splitting them evenly already follows how the ear divides the spectrum.
    fn of(spectrum: &[f32; NUM_BARS]) -> Self {
        let band = NUM_BARS / 3;
        let mean = |bars: &[f32]| bars.iter().sum::<f32>() / bars.len() as f32;
        Self {
            low: mean(&spectrum[..band]),
            mid: mean(&spectrum[band..band * 2]),
            high: mean(&spectrum[band * 2..]),
        }
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
    fn fps_sets_the_analysis_interval_and_clamps() {
        let snap = VizSnapshot::new();
        assert_eq!(snap.fps(), DEFAULT_FPS);

        snap.set_fps(120);
        assert_eq!(snap.fps(), 120);
        assert_eq!(snap.interval(), std::time::Duration::from_micros(8_333));

        // A rate from outside koan: zero would be a division and a spin.
        snap.set_fps(0);
        assert_eq!(snap.fps(), 1);
    }

    #[test]
    fn a_wake_landing_before_the_park_is_not_slept_through() {
        let snap = VizSnapshot::new();
        // Playback starting is a wake with no read behind it, and it can land
        // in the moment between the analyser deciding to park and parking.
        snap.wake();
        // Would block forever if the wake had been missed.
        snap.park_while_idle(|| true);

        // And it is not sticky beyond the one park it was meant for.
        let woken = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&woken);
        let snapshot = Arc::clone(&snap);
        let waiter = std::thread::spawn(move || {
            snapshot.park_while_idle(|| true);
            flag.store(true, Ordering::Relaxed);
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !woken.load(Ordering::Relaxed),
            "parked thread woke on its own"
        );
        snap.wake();
        waiter.join().unwrap();
    }

    #[test]
    fn a_read_wakes_a_parked_analyser() {
        let snap = VizSnapshot::new();
        let snapshot = Arc::clone(&snap);
        let waiter = std::thread::spawn(move || snapshot.park_while_idle(|| true));
        std::thread::sleep(std::time::Duration::from_millis(50));
        // What a subscriber does before it waits for a frame.
        snap.touch();
        waiter.join().unwrap();
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

    #[test]
    fn levels_average_each_third_of_the_bars() {
        let mut spectrum = [0.0f32; NUM_BARS];
        let band = NUM_BARS / 3;
        spectrum[..band].fill(0.6);
        spectrum[band..band * 2].fill(0.3);
        spectrum[band * 2..].fill(0.0);
        // One loud bar in the top third: the mean carries it, diluted.
        spectrum[NUM_BARS - 1] = 1.0;

        let levels = VizLevels::of(&spectrum);
        assert!((levels.low - 0.6).abs() < 0.001);
        assert!((levels.mid - 0.3).abs() < 0.001);
        assert!((levels.high - 1.0 / band as f32).abs() < 0.001);
    }
}
