use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// How long pause takes to fall silent, and resume to come back.
///
/// Long enough that a pause lands as a fade rather than a cut, short enough
/// that it still reads as the key having been pressed.
const FADE_SECONDS: f64 = 0.15;

/// How long a sleep timer takes to fall silent: long enough to drift off to
/// rather than be woken by.
const SLOW_FADE_SECONDS: f64 = 6.0;

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
    /// The fade out under way is a sleep timer's: `SLOW_FADE_SECONDS` long.
    slow: AtomicBool,
}

impl FadeControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            audible: AtomicBool::new(true),
            ..Default::default()
        })
    }

    pub fn fade_out(&self) {
        self.audible.store(false, Ordering::Release);
    }

    /// A fade out over seconds rather than a moment, from wherever the gain
    /// is.
    pub fn fade_out_slowly(&self) {
        self.slow.store(true, Ordering::Release);
        self.audible.store(false, Ordering::Release);
    }

    /// Ramp back up. `from_silence` when the unit is about to be started from
    /// stopped, where the callback has not been running to bring the gain down.
    pub fn fade_in(&self, from_silence: bool) {
        self.silent.store(false, Ordering::Release);
        self.slow.store(false, Ordering::Release);
        if from_silence {
            self.from_silence.store(true, Ordering::Release);
        }
        self.audible.store(true, Ordering::Release);
    }

    pub fn is_silent(&self) -> bool {
        self.silent.load(Ordering::Acquire)
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
    short: usize,
    slow: usize,
}

impl Fader {
    pub fn new(control: Arc<FadeControl>, sample_rate: f64) -> Self {
        let len = ((sample_rate * FADE_SECONDS) as usize).max(1);
        Self {
            control,
            pos: len,
            len,
            short: len,
            slow: ((sample_rate * SLOW_FADE_SECONDS) as usize).max(1),
        }
    }

    /// Take up the ramp's length the control asks for, at the same level.
    fn follow_length(&mut self) {
        let len = if self.control.slow.load(Ordering::Acquire) {
            self.slow
        } else {
            self.short
        };
        if len != self.len {
            self.pos = self.pos * len / self.len;
            self.len = len;
        }
    }

    /// How many of `wanted` interleaved samples to take from the ring.
    ///
    /// A fade out stops reading exactly where it reaches silence, so the play
    /// head rests on the last sample heard rather than on whatever was
    /// consumed and thrown away.
    pub fn readable(&mut self, wanted: usize, channels: usize) -> usize {
        self.follow_length();
        if self.control.from_silence.swap(false, Ordering::AcqRel) {
            self.pos = 0;
        }
        if self.control.audible.load(Ordering::Acquire) {
            return wanted;
        }
        if self.pos == 0 {
            self.control.silent.store(true, Ordering::Release);
        }
        wanted.min(self.pos * channels.max(1))
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

    /// Apply the ramp to interleaved samples just read from the ring.
    pub fn apply(&mut self, samples: &mut [f32], channels: usize) {
        self.follow_length();
        let rising = self.control.audible.load(Ordering::Acquire);
        if (rising && self.pos == self.len) || (!rising && self.pos == 0) {
            return;
        }
        for frame in samples.chunks_mut(channels.max(1)) {
            self.pos = if rising {
                (self.pos + 1).min(self.len)
            } else {
                self.pos.saturating_sub(1)
            };
            // Squared, so the ear hears an even fade rather than one that
            // hangs loud and then drops away.
            let level = self.pos as f32 / self.len as f32;
            let gain = level * level;
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

    /// A sleep timer's fade takes seconds, and a resume after it comes back
    /// in a moment, as from a pause.
    #[test]
    fn a_slow_fade_takes_seconds_and_the_resume_does_not() {
        let control = FadeControl::new();
        let mut fader = Fader::new(control.clone(), RATE);
        control.fade_out_slowly();
        let down = render(&mut fader, 3000);
        assert_eq!(down.len(), 6000, "half way, still reading");
        assert!(down.windows(2).all(|w| w[1] <= w[0]));
        assert!(*down.last().unwrap() > 0.0 && !control.is_silent());
        render(&mut fader, 3000);
        assert!(control.is_silent());

        control.fade_in(false);
        let up = render(&mut fader, 150);
        assert_eq!(*up.last().unwrap(), 1.0, "back in the short ramp");
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
}
