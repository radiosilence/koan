//! The sleep timer's fade: the last stretch before a timer ends playback,
//! brought down from full level to silence so that it is drifted off to
//! rather than cut.
//!
//! The level falls linearly in decibels, which the ear hears as a steady
//! glide. Locally it is a gain the render callback applies, set here a few
//! times a second and glided between; a renderer is handed the original file,
//! so its own volume is stepped down instead, and put back once it has paused.

use std::time::{Duration, Instant};

use super::state::Sleep;
use super::{Output, Player, Run};

/// The shortest and longest a timer set for a time fades for.
const SHORTEST: Duration = Duration::from_secs(60);
const LONGEST: Duration = Duration::from_secs(300);
/// How long the end of a track or record fades for, or the whole track if it
/// is shorter.
const TRACK_FADE: Duration = Duration::from_secs(60);
/// Where the fade ends, at which point playback pauses: as near silence as
/// makes no difference.
const QUIET_DB: f32 = -60.0;
/// How often the gain is set while it moves. The callback glides between.
const TICK: Duration = Duration::from_millis(100);
/// How often a renderer's volume is stepped: each step is a request to it.
const RENDERER_TICK: Duration = Duration::from_secs(1);
/// How long a cancelled fade takes to come back to full level.
const RESTORE: Duration = Duration::from_secs(1);

/// How long a timer of `minutes` fades for: a tenth of it, between a minute
/// and five.
pub(super) fn fade_length(minutes: u32) -> Duration {
    (Duration::from_secs(u64::from(minutes) * 6)).clamp(SHORTEST, LONGEST)
}

/// The gain `progress` of the way through a fade, from 0 to 1.
pub(super) fn fade_gain(progress: f32) -> f32 {
    db_to_gain(QUIET_DB * progress.clamp(0.0, 1.0))
}

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1e-6).log10()
}

/// A fade under way, or full level being brought back after one.
#[derive(Clone, Copy, Debug)]
pub(super) enum SleepFade {
    Falling {
        gain: f32,
        /// A renderer's volume before the fade, to put back.
        renderer_volume: Option<u8>,
        stepped: Option<Instant>,
        logged: Instant,
    },
    Restoring {
        from: f32,
        since: Instant,
    },
}

impl Player {
    /// When the fade runs: from its start to the moment the timer ends
    /// playback. `None` while nothing plays toward a timer.
    pub(super) fn sleep_window(&self) -> Option<(Instant, Instant)> {
        let set = self.sleep?;
        if self.intent() != Some(Run::Playing) {
            return None;
        }
        if let (Some(at), Some(length)) = (set.at, set.fade) {
            return Some((at.checked_sub(length).unwrap_or(at), at));
        }
        let state = &self.shared_state;
        let duration = state.duration_ms();
        if duration == 0 {
            return None;
        }
        if set.sleep == Sleep::EndOfRecord {
            // Only the record's last track fades.
            let record = |id| state.get_item(id).map(|i| (i.album, i.album_artist));
            let cursor = state.cursor()?;
            let next = state.lookahead_after(cursor).and_then(|l| l.next);
            if next.is_some_and(|next| record(next) == record(cursor)) {
                return None;
            }
        }
        let end =
            Instant::now() + Duration::from_millis(duration.saturating_sub(state.position_ms()));
        let length = TRACK_FADE.min(Duration::from_millis(duration));
        Some((end.checked_sub(length).unwrap_or(end), end))
    }

    /// When the fade next needs the player: at its start, or at its next step.
    pub(super) fn sleep_wake(&self) -> Option<Instant> {
        let now = Instant::now();
        match self.sleep_fade {
            Some(SleepFade::Restoring { .. }) => Some(now + TICK),
            Some(SleepFade::Falling { .. }) => Some(now + self.sleep_tick_period()),
            None => self.sleep_window().map(|(start, _)| start.max(now)),
        }
    }

    fn sleep_tick_period(&self) -> Duration {
        if self.renderer_output() {
            RENDERER_TICK
        } else {
            TICK
        }
    }

    fn renderer_output(&self) -> bool {
        self.session()
            .is_some_and(|s| matches!(s.output, Output::Renderer(_)))
    }

    /// Bring the level to where the fade has got to, or back toward full.
    /// Called on each wake.
    pub(super) fn sleep_tick(&mut self) {
        let now = Instant::now();
        if let Some(SleepFade::Restoring { from, since }) = self.sleep_fade {
            let t = (now - since).as_secs_f32() / RESTORE.as_secs_f32();
            if t >= 1.0 {
                self.set_sleep_gain(1.0, false);
                self.sleep_fade = None;
            } else {
                self.set_sleep_gain(db_to_gain(gain_to_db(from) * (1.0 - t)), false);
                return;
            }
        }
        let window = self.sleep_window().filter(|(start, _)| now >= *start);
        match (window, self.sleep_fade) {
            (Some((start, end)), fade) => {
                let length = (end - start).as_secs_f32().max(0.001);
                let gain = fade_gain((now - start).as_secs_f32() / length);
                let (renderer_volume, stepped, logged) = match fade {
                    Some(SleepFade::Falling {
                        renderer_volume,
                        stepped,
                        logged,
                        ..
                    }) => (renderer_volume, stepped, logged),
                    _ => {
                        log::info!("sleep timer: fading over {:.0} s", length);
                        let volume = self
                            .renderer_output()
                            .then(|| self.shared_state.renderer().and_then(|o| o.volume))
                            .flatten();
                        (volume, None, now)
                    }
                };
                let stepped = match renderer_volume {
                    Some(volume) => {
                        // The renderer's scale is its own; stepped evenly by
                        // how far the fade has got.
                        let due = stepped.is_none_or(|at| now - at >= RENDERER_TICK);
                        if due {
                            let progress = ((now - start).as_secs_f32() / length).clamp(0.0, 1.0);
                            let step = (f32::from(volume) * (1.0 - progress)).round() as u8;
                            self.set_renderer_volume(step);
                            Some(now)
                        } else {
                            stepped
                        }
                    }
                    None => {
                        self.set_sleep_gain(gain, false);
                        stepped
                    }
                };
                let logged = if now - logged >= Duration::from_secs(5) {
                    log::info!("sleep timer: fading, {:.1} dB", gain_to_db(gain));
                    now
                } else {
                    logged
                };
                self.sleep_fade = Some(SleepFade::Falling {
                    gain,
                    renderer_volume,
                    stepped,
                    logged,
                });
            }
            // Out of the window again, a skip or a seek back: full level.
            (None, Some(SleepFade::Falling { .. })) => self.restore_sleep_fade(),
            (None, _) => {}
        }
    }

    /// Bring a fade back up to full level over `RESTORE`: the timer was
    /// cancelled or changed, or playback moved out of its window.
    pub(super) fn restore_sleep_fade(&mut self) {
        if let Some(SleepFade::Falling {
            gain,
            renderer_volume,
            ..
        }) = self.sleep_fade
        {
            log::info!("sleep timer: fade called off, back to full level");
            match renderer_volume {
                Some(volume) => {
                    self.set_renderer_volume(volume);
                    self.sleep_fade = None;
                }
                None => {
                    self.sleep_fade = Some(SleepFade::Restoring {
                        from: gain,
                        since: Instant::now(),
                    })
                }
            }
        }
    }

    /// Playback has paused at the end of a fade, or by hand during one: full
    /// level for whatever plays next. Locally it is taken at once, which is
    /// unheard with the output stopped; a renderer's volume is put back.
    pub(super) fn end_sleep_fade(&mut self) {
        match self.sleep_fade.take() {
            Some(SleepFade::Falling {
                renderer_volume: Some(volume),
                ..
            }) => self.set_renderer_volume(volume),
            Some(_) => self.set_sleep_gain(1.0, true),
            None => {}
        }
    }

    pub(super) fn set_sleep_gain(&self, gain: f32, snap: bool) {
        if let Some(engine) = self.session().and_then(|s| s.engine()) {
            engine.set_sleep_gain(gain, snap);
        }
    }

    /// Paused by hand during a fade: the listener is awake. The timer is
    /// off, and resuming plays at full level. Locally that is taken on
    /// resuming, since doing it now would come up through the pause's own
    /// short fade.
    pub(super) fn paused_in_sleep_fade(&mut self) {
        if !self.sleep_fading() {
            return;
        }
        log::info!("sleep timer: paused during the fade, so cancelled");
        self.sleep = None;
        match self.sleep_fade.take() {
            Some(SleepFade::Falling {
                renderer_volume: Some(volume),
                ..
            }) => self.set_renderer_volume(volume),
            _ => self.sleep_snap_on_resume = true,
        }
    }

    pub(super) fn sleep_fading(&self) -> bool {
        matches!(self.sleep_fade, Some(SleepFade::Falling { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fade_is_a_tenth_of_the_timer_between_a_minute_and_five() {
        let s = |secs| Duration::from_secs(secs);
        assert_eq!(fade_length(1), s(60), "never under a minute");
        assert_eq!(fade_length(10), s(60));
        assert_eq!(fade_length(15), s(90));
        assert_eq!(fade_length(30), s(180));
        assert_eq!(fade_length(50), s(300));
        assert_eq!(fade_length(60), s(300), "never over five");
        assert_eq!(fade_length(600), s(300));
    }

    /// Linear in decibels: each equal stretch of the fade takes the same
    /// number of decibels off, so it is heard as a steady glide.
    #[test]
    fn the_level_falls_evenly_in_decibels_to_silence() {
        assert_eq!(fade_gain(0.0), 1.0);
        assert!((gain_to_db(fade_gain(0.5)) + 30.0).abs() < 0.01);
        assert!((gain_to_db(fade_gain(1.0)) + 60.0).abs() < 0.01);
        let steps: Vec<f32> = (0..=10)
            .map(|i| gain_to_db(fade_gain(i as f32 / 10.0)))
            .collect();
        assert!(steps.windows(2).all(|w| (w[0] - w[1] - 6.0).abs() < 0.01));
    }
}
