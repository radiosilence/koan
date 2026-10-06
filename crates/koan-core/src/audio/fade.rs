use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// How long pause takes to fall silent, and resume to come back.
///
/// Long enough that a pause lands as a fade rather than a cut, short enough
/// that it still reads as the key having been pressed.
const FADE_SECONDS: f64 = 0.15;

/// How long a quick fade takes: the dip a DSP change makes, out of the old
/// processing and into the new. Long enough that neither edge is a step,
/// short enough to pass for the change itself.
pub const QUICK_FADE: std::time::Duration = std::time::Duration::from_millis(15);

/// How long the callback takes to glide to a new sleep gain: the player sets
/// one about this often while a sleep timer fades, so the level moves on
/// without a step.
const SLEEP_GLIDE_SECONDS: f64 = 0.1;

/// What the player asks of a fade. Shared with the render callback, so atomics
/// only.
#[derive(Default)]
pub struct FadeControl {
    /// Where the gain is heading: full when set, silence when not.
    audible: AtomicBool,
    /// Set when the unit is started from stopped, so the first callback begins
    /// the ramp at silence rather than wherever the gain was left.
    from_silence: AtomicBool,
    /// Written by the callback once a fade out has reached silence. The unit
    /// can then be stopped without cutting anything off.
    silent: AtomicBool,
    /// A sleep timer's gain, as `f32` bits: 1.0 but while one fades. Applied
    /// on top of the pause ramp.
    sleep_gain: AtomicU32,
    /// Take `sleep_gain` at once rather than gliding to it.
    sleep_snap: AtomicBool,
    /// `playback.muted`: every sample zeroed after the ramps.
    muted: AtomicBool,
    /// Ramp over `QUICK_FADE` rather than `FADE_SECONDS`. Set by the
    /// quick fades, cleared by the others.
    quick: AtomicBool,
    /// How long one render callback asks for, in microseconds: how late after
    /// a fade begins the callback may first see it.
    period_us: AtomicU32,
}

impl FadeControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            audible: AtomicBool::new(true),
            sleep_gain: AtomicU32::new(1.0f32.to_bits()),
            ..Default::default()
        })
    }

    /// A fade for an output unit, muted when `playback.muted` says so.
    pub fn for_output() -> Arc<Self> {
        let control = Self::new();
        if crate::config::Config::cached().playback.muted {
            log::info!("audio: muted (playback.muted)");
            control.muted.store(true, Ordering::Release);
        }
        control
    }

    pub fn fade_out(&self) {
        self.quick.store(false, Ordering::Release);
        self.audible.store(false, Ordering::Release);
    }

    /// `fade_out` over `QUICK_FADE`.
    pub fn fade_out_quickly(&self) {
        self.quick.store(true, Ordering::Release);
        self.audible.store(false, Ordering::Release);
    }

    /// Ramp back up. `from_silence` when the unit is about to be started from
    /// stopped, where the callback has not been running to bring the gain down.
    pub fn fade_in(&self, from_silence: bool) {
        self.quick.store(false, Ordering::Release);
        self.rise(from_silence);
    }

    /// `fade_in` over `QUICK_FADE`.
    pub fn fade_in_quickly(&self, from_silence: bool) {
        self.quick.store(true, Ordering::Release);
        self.rise(from_silence);
    }

    fn rise(&self, from_silence: bool) {
        self.silent.store(false, Ordering::Release);
        if from_silence {
            self.from_silence.store(true, Ordering::Release);
        }
        self.audible.store(true, Ordering::Release);
    }

    pub fn is_silent(&self) -> bool {
        self.silent.load(Ordering::Acquire)
    }

    /// One render callback's length, as the callback last asked for it.
    /// Zero until the first callback.
    pub fn period(&self) -> std::time::Duration {
        std::time::Duration::from_micros(self.period_us.load(Ordering::Relaxed) as u64)
    }

    /// The sleep timer's gain, glided to over `SLEEP_GLIDE_SECONDS`, or with
    /// `snap` taken at once.
    pub fn set_sleep_gain(&self, gain: f32, snap: bool) {
        if snap {
            self.sleep_snap.store(true, Ordering::Release);
        }
        self.sleep_gain
            .store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Release);
    }
}

/// The render callback's half of a fade: the gain itself, which only the
/// callback touches.
///
/// At full level samples pass through untouched, so outside a fade the output
/// stays bit-perfect.
pub struct Fader {
    control: Arc<FadeControl>,
    /// Frames into the ramp, from 0 (silent) to `len` (full). Counted in whole
    /// frames so a fade out ends on an exact frame.
    pos: usize,
    len: usize,
    /// How far a quick fade moves `pos` each frame.
    quick_step: usize,
    sample_rate: f64,
    /// The sleep gain where the callback has it, the target it was last
    /// given, and how far it moves each frame on the way there.
    sleep: f32,
    sleep_target: u32,
    sleep_step: f32,
    glide: f32,
}

impl Fader {
    pub fn new(control: Arc<FadeControl>, sample_rate: f64) -> Self {
        let len = ((sample_rate * FADE_SECONDS) as usize).max(1);
        let quick = ((sample_rate * QUICK_FADE.as_secs_f64()) as usize).max(1);
        Self {
            control,
            pos: len,
            len,
            quick_step: len.div_ceil(quick),
            sample_rate,
            sleep: 1.0,
            sleep_target: 1.0f32.to_bits(),
            sleep_step: 0.0,
            glide: ((sample_rate * SLEEP_GLIDE_SECONDS) as f32).max(1.0),
        }
    }

    /// How many of `wanted` interleaved samples to take from the ring.
    ///
    /// A fade out stops reading exactly where it reaches silence, so the play
    /// head rests on the last sample heard rather than on whatever was
    /// consumed and thrown away.
    pub fn readable(&mut self, wanted: usize, channels: usize) -> usize {
        let frames = wanted / channels.max(1);
        if frames > 0 {
            let us = (frames as f64 * 1e6 / self.sample_rate) as u32;
            self.control.period_us.store(us, Ordering::Relaxed);
        }
        if self.control.from_silence.swap(false, Ordering::AcqRel) {
            self.pos = 0;
        }
        if self.control.audible.load(Ordering::Acquire) {
            return wanted;
        }
        if self.pos == 0 {
            self.control.silent.store(true, Ordering::Release);
        }
        wanted.min(self.pos.div_ceil(self.step()) * channels.max(1))
    }

    /// How far `pos` moves each frame of a ramp.
    fn step(&self) -> usize {
        if self.control.quick.load(Ordering::Relaxed) {
            self.quick_step
        } else {
            1
        }
    }

    /// A callback that plays silence in place of the ring. A fade out has
    /// nothing audible to ramp down, so it is done at once; left pending, it
    /// would ramp through the start of the track when the silence ended.
    pub fn hold(&mut self) {
        if !self.control.audible.load(Ordering::Acquire) {
            self.pos = 0;
            self.control.silent.store(true, Ordering::Release);
        }
    }

    /// Take up a sleep gain the player has set since the last callback.
    fn follow_sleep(&mut self) {
        let bits = self.control.sleep_gain.load(Ordering::Acquire);
        let snap = self.control.sleep_snap.swap(false, Ordering::AcqRel);
        if bits == self.sleep_target && !snap {
            return;
        }
        self.sleep_target = bits;
        let target = f32::from_bits(bits);
        if snap {
            self.sleep = target;
            self.sleep_step = 0.0;
        } else {
            self.sleep_step = (target - self.sleep) / self.glide;
        }
    }

    /// Apply the ramps to interleaved samples just read from the ring.
    pub fn apply(&mut self, samples: &mut [f32], channels: usize) {
        self.ramp(samples, channels);
        if self.control.muted.load(Ordering::Relaxed) {
            samples.fill(0.0);
        }
    }

    fn ramp(&mut self, samples: &mut [f32], channels: usize) {
        self.follow_sleep();
        let rising = self.control.audible.load(Ordering::Acquire);
        let pausing = !((rising && self.pos == self.len) || (!rising && self.pos == 0));
        let sleeping = self.sleep != 1.0 || self.sleep_step != 0.0;
        if !pausing && !sleeping {
            return;
        }
        let target = f32::from_bits(self.sleep_target);
        let step = self.step();
        for frame in samples.chunks_mut(channels.max(1)) {
            if pausing {
                self.pos = if rising {
                    (self.pos + step).min(self.len)
                } else {
                    self.pos.saturating_sub(step)
                };
            }
            if self.sleep_step != 0.0 {
                self.sleep += self.sleep_step;
                if (self.sleep_step > 0.0 && self.sleep >= target)
                    || (self.sleep_step < 0.0 && self.sleep <= target)
                {
                    self.sleep = target;
                    self.sleep_step = 0.0;
                }
            }
            // Squared, so the ear hears an even fade rather than one that
            // hangs loud and then drops away.
            let level = self.pos as f32 / self.len as f32;
            let gain = level * level * self.sleep;
            for s in frame {
                *s *= gain;
            }
        }
        if !rising && self.pos == 0 {
            self.control.silent.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 1000.0; // 150 frames per fade

    fn render(fader: &mut Fader, frames: usize) -> Vec<f32> {
        let wanted = fader.readable(frames * 2, 2);
        let mut out = vec![1.0f32; wanted];
        fader.apply(&mut out, 2);
        out
    }

    #[test]
    fn a_fade_out_over_silence_is_silent_at_once() {
        let control = FadeControl::new();
        let mut fader = Fader::new(control.clone(), RATE);
        fader.hold();
        assert!(!control.is_silent(), "holding at full level fades nothing");

        control.fade_out();
        fader.hold();
        assert!(control.is_silent());
        assert_eq!(
            fader.readable(64, 2),
            0,
            "nothing of the track plays after the pause"
        );
    }

    #[test]
    fn full_level_passes_samples_through() {
        let control = FadeControl::new();
        let mut fader = Fader::new(control, RATE);
        let out = render(&mut fader, 64);
        assert_eq!(out.len(), 128);
        assert!(out.iter().all(|s| *s == 1.0));
    }

    #[test]
    fn muted_reads_on_and_plays_silence() {
        let control = FadeControl::new();
        control.muted.store(true, Ordering::Release);
        let mut fader = Fader::new(control, RATE);
        let out = render(&mut fader, 64);
        assert_eq!(out.len(), 128, "the track moves on as if heard");
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn fade_out_stops_reading_at_silence() {
        let control = FadeControl::new();
        let mut fader = Fader::new(control.clone(), RATE);
        control.fade_out();

        let first = render(&mut fader, 100);
        assert_eq!(first.len(), 200);
        assert!(first.windows(2).all(|w| w[1] <= w[0]), "gain only falls");
        assert!(!control.is_silent());

        let second = render(&mut fader, 100);
        assert_eq!(second.len(), 100, "only the rest of the ramp is read");
        assert_eq!(*second.last().unwrap(), 0.0);
        assert!(control.is_silent());

        assert!(
            render(&mut fader, 100).is_empty(),
            "nothing read once silent"
        );
    }

    #[test]
    fn fade_in_from_stopped_starts_at_silence() {
        let control = FadeControl::new();
        let mut fader = Fader::new(control.clone(), RATE);
        control.fade_in(true);

        let out = render(&mut fader, 200);
        assert!(out[0] < 0.01);
        assert!(out.windows(2).all(|w| w[1] >= w[0]), "gain only rises");
        assert_eq!(*out.last().unwrap(), 1.0);
    }

    /// A sleep gain is glided to, not stepped; full level is left alone,
    /// which keeps the output bit-perfect outside a sleep timer's fade.
    #[test]
    fn a_sleep_gain_glides_and_full_level_is_untouched() {
        let control = FadeControl::new();
        let mut fader = Fader::new(control.clone(), RATE);
        assert!(render(&mut fader, 10).iter().all(|s| *s == 1.0));

        control.set_sleep_gain(0.5, false);
        let glide = render(&mut fader, 100); // the whole glide, 0.1 s
        assert!(glide.windows(2).all(|w| w[1] <= w[0]), "only falls");
        assert!(glide[0] < 1.0 && glide[0] > 0.99, "no step: {}", glide[0]);
        assert!((glide.last().unwrap() - 0.5).abs() < 1e-4);
        assert!(
            render(&mut fader, 10)
                .iter()
                .all(|s| (*s - 0.5).abs() < 1e-6)
        );

        control.set_sleep_gain(1.0, true);
        assert!(
            render(&mut fader, 10).iter().all(|s| *s == 1.0),
            "snapped back"
        );
    }

    #[test]
    fn resume_mid_fade_turns_around() {
        let control = FadeControl::new();
        let mut fader = Fader::new(control.clone(), RATE);
        control.fade_out();
        let down = render(&mut fader, 50);
        control.fade_in(false);
        let up = render(&mut fader, 10);
        assert!(up[0] > *down.last().unwrap(), "rises from where it was");
        assert!(!control.is_silent());
    }

    /// A DSP change: the old processing fades out quickly, stops reading at
    /// silence, and a new session fades in from silence. Stitched together,
    /// at a rate where the fades are real lengths, no sample jumps from the
    /// one before by more than a full-scale 1 kHz sine moves on its own, and
    /// the whole dip lasts twice `QUICK_FADE`.
    #[test]
    fn a_quick_fade_out_and_in_has_no_step() {
        let rate = 48000.0;
        let sine = |phase: f64, n: usize| {
            (0..n)
                .map(|i| (phase + std::f64::consts::TAU * 1000.0 * i as f64 / rate).sin() as f32)
                .collect::<Vec<f32>>()
        };
        // One render callback: as much of `source` from `at` as the fader
        // reads, ramped.
        let callback = |fader: &mut Fader, source: &[f32], at: &mut usize, frames: usize| {
            let take = fader.readable(frames, 1);
            let mut out = source[*at..*at + take].to_vec();
            *at += take;
            fader.apply(&mut out, 1);
            out
        };

        let old = FadeControl::new();
        let mut fader = Fader::new(old.clone(), rate);
        let playing = sine(0.0, 48000);
        let mut at = 0;
        // 492 frames: a quarter cycle past a whole one, so the fade starts at
        // the peak, where a cut would be a step of 1.
        let mut heard = callback(&mut fader, &playing, &mut at, 492);
        old.fade_out_quickly();
        let mut tail = Vec::new();
        for _ in 0..20 {
            tail.extend(callback(&mut fader, &playing, &mut at, 512));
            if old.is_silent() {
                break;
            }
        }
        assert!(old.is_silent());
        let quick = (rate * QUICK_FADE.as_secs_f64()) as usize;
        assert!(tail.len() <= quick + 1, "faded in {} frames", tail.len());
        heard.extend(tail);

        let new = FadeControl::new();
        let mut fader = Fader::new(new.clone(), rate);
        new.fade_in_quickly(true);
        let next = sine(2.9, 2048);
        heard.extend(callback(&mut fader, &next, &mut 0, 2048));

        // The ramp's own slope adds a little where the sine is near its peak.
        let natural = (std::f64::consts::TAU * 1000.0 / rate) as f32 * 1.05;
        let worst = heard
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(
            worst <= natural,
            "a step of {worst}, more than the sine's own {natural}"
        );

        // Quick fades are for DSP changes only: a pause after one takes the
        // ordinary length.
        new.fade_out();
        let pause = callback(&mut fader, &playing, &mut 0, 48000);
        assert_eq!(pause.len(), (rate * FADE_SECONDS) as usize);
    }

    /// The callback's length, which the player waits on beyond a quick fade,
    /// as the buffers a Mac and PipeWire use by default ask for it.
    #[test]
    fn the_callback_reports_its_period() {
        for (frames, rate) in [(512usize, 44100.0), (1024, 48000.0)] {
            let control = FadeControl::new();
            let mut fader = Fader::new(control.clone(), rate);
            assert_eq!(control.period(), std::time::Duration::ZERO);
            fader.readable(frames * 2, 2);
            let want = frames as f64 / rate;
            assert!(
                (control.period().as_secs_f64() - want).abs() < 2e-6,
                "{frames} frames"
            );
        }
    }
}
