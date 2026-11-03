//! The kit's shader programs: a `shade` in MSL and WGSL that reads the kit's
//! tracks through `koan_track`. Each is handed to gpui once, as a program
//! whose source has the tracks' functions before it and whose `prepare`
//! keeps `time` and the tracks' starts on a recent base, then runs the
//! program's own.

use std::collections::HashMap;
use std::sync::LazyLock;

use gpui::SHADER_UNIFORMS;
use parking_lot::Mutex;

use super::track::{SHADER_PLAIN, SHADER_TRACKS, TRACK_FLOATS, TRACKS_MSL, TRACKS_WGSL};
use super::{Repeat as TrackRepeat, Track};

/// A fragment program drawing a quad, written once for each renderer.
///
/// Each source defines `shade`, as gpui's `ShaderProgram` describes, and may
/// call `koan_track(u, k, time)` for where track `k` stands at `time`.
#[derive(Debug)]
pub struct ShaderProgram {
    /// What the compiled program is labelled with in a GPU debugger.
    pub name: &'static str,
    /// `shade` in the Metal Shading Language.
    pub msl: &'static str,
    /// `shade` in WGSL.
    pub wgsl: &'static str,
    /// Run on the CPU once per quad as each frame is presented, given its
    /// uniforms and `time`, to write values every pixel would otherwise work
    /// out alike into the uniforms the program reads.
    pub prepare: Option<fn(&mut [f32; SHADER_UNIFORMS], f32)>,
}

impl ShaderProgram {
    /// This program as gpui draws it, made the first time it is asked for.
    pub fn gpui(&'static self) -> &'static gpui::ShaderProgram {
        static MADE: LazyLock<Mutex<HashMap<usize, &'static gpui::ShaderProgram>>> =
            LazyLock::new(Default::default);
        let key = self as *const Self as usize;
        if let Some(made) = MADE.lock().get(&key) {
            return made;
        }
        let own = self.prepare;
        let prepare: &'static (dyn Fn(&mut [f32; SHADER_UNIFORMS], f64) -> f32 + Send + Sync) =
            Box::leak(Box::new(
                move |u: &mut [f32; SHADER_UNIFORMS], seconds: f64| {
                    let time = on_recent_base(u, seconds);
                    if let Some(own) = own {
                        own(u, time);
                    }
                    time
                },
            ));
        let made: &'static gpui::ShaderProgram = Box::leak(Box::new(gpui::ShaderProgram {
            name: self.name,
            msl: Box::leak(format!("{TRACKS_MSL}\n{}", self.msl).into_boxed_str()),
            wgsl: Box::leak(format!("{TRACKS_WGSL}\n{}", self.wgsl).into_boxed_str()),
            prepare: Some(prepare),
        }));
        *MADE.lock().entry(key).or_insert(made)
    }
}

/// How often `time` starts again from a whole hour.
const BASE: f64 = 3600.0;

/// `time` and every track's start as seconds from the last whole hour of
/// `seconds`, a repeating track's start moved up to its latest whole cycle,
/// all worked out in f64: the program reckons the same motion, to the
/// frame, however long the quad has been drawn. Rewrites the present's copy
/// only, so writers never see a base move under them.
fn on_recent_base(u: &mut [f32; SHADER_UNIFORMS], seconds: f64) -> f32 {
    let base = (seconds / BASE).floor() * BASE;
    if base == 0.0 {
        return seconds as f32;
    }
    for k in 0..SHADER_TRACKS {
        let at = SHADER_PLAIN + k * TRACK_FLOATS;
        let mut track = Track::decode(&u[at..at + TRACK_FLOATS]);
        if track.length <= 0.0 {
            continue;
        }
        let mut start = track.start as f64;
        let period = match track.repeat {
            TrackRepeat::Once => 0.0,
            TrackRepeat::Loop => track.length as f64,
            TrackRepeat::Alternate => 2.0 * track.length as f64,
        };
        if period > 0.0 && start < seconds {
            start += ((seconds - start) / period).floor() * period;
        }
        track.start = (start - base) as f32;
        u[at..at + TRACK_FLOATS].copy_from_slice(&track.encode());
    }
    (seconds - base) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::TrackCurve;

    /// Three days into a quad's life, a looping track is drawn where it
    /// stands, to well within a frame.
    #[test]
    fn days_on_the_motion_is_exact() {
        let mut u = [0.0; SHADER_UNIFORMS];
        let track = Track {
            from: 0.0,
            to: 1.0,
            start: 0.0,
            length: 1.0,
            curve: TrackCurve::Linear,
            repeat: TrackRepeat::Loop,
        };
        u[SHADER_PLAIN..SHADER_PLAIN + TRACK_FLOATS].copy_from_slice(&track.encode());
        for offset in [0.25, 0.5, 0.75] {
            let mut copy = u;
            let time = on_recent_base(&mut copy, 3.0 * 86_400.0 + offset);
            let drawn = Track::decode(&copy[SHADER_PLAIN..SHADER_PLAIN + TRACK_FLOATS]).at(time);
            assert!(
                (drawn - offset as f32).abs() < 1e-3,
                "{offset}s into a cycle drew {drawn}"
            );
        }
    }
}
