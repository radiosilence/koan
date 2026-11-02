//! The playing indicators: three bars moving with the music, wherever a
//! playing row or record is on screen. koan-kit draws them (`PlayingBars`);
//! this is what moves them.
//!
//! One subscription to the analyser for the whole app, read only while a bar
//! is on screen; with none, the stream is dropped and the analyser parks.
//! Nothing is polled: the analyser publishes a frame and the follow, on the
//! background executor, wakes on it.
//!
//! On macOS each slot is a shader quad, painted once by its row and replayed
//! with the row's cached view. The follow writes the bands straight into the
//! slots' levels and marks them live, and while they are the window presents
//! its last frame again whenever they move, drawing only the shader quads
//! over the frame it kept: a moving bar costs a small encode, never a GPUI
//! frame. A quarter of a second after the bands last moved they stop being
//! live and the window goes back to sleep; when they move again one frame is
//! drawn to notice.
//!
//! Elsewhere, until the Linux renderer draws shaders, the bars are quads. A
//! row draws an empty slot, which records where it was laid out and under
//! which clip, and each window's root paints the bars over its content
//! (`paint`), so a frame of the bars redraws the root alone, whose content is
//! a cached view and is replayed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::FutureExt as _;
use koan_kit::app::*;
use koan_kit::components::{BarLevels, BarsPlace, PlayingBars, SHADED, bars_overlay};
// the colour the caller's palette gives the bars.
use koan_kit::raw::Hsla;
use parking_lot::Mutex;

use crate::engine::engine;

/// A band below this is room tone: it never sets a ceiling, and silence is
/// measured against it, so a quiet passage stays flat.
const QUIETEST: f32 = 0.12;
/// Seconds a band takes to forget half of a loud passage.
const FORGET: f32 = 4.0;
/// The analyser's rate while bars are on screen.
const FPS: u8 = 60;
/// How long after the levels last moved the bars stop being presented.
const STILL: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Registry {
    /// Where each slot drawn as quads was laid out, and in what colour.
    slots: HashMap<u64, (BarsPlace, Hsla)>,
    /// Each slot drawn as a shader, by its levels.
    shaders: HashMap<u64, BarLevels>,
    next_id: u64,
    /// How high each bar stands, 0 to 1, low band to high: each band
    /// against its own recent range, so a record mastered quiet moves as
    /// much as a loud one.
    bands: [f32; 3],
    ceiling: [f32; 3],
    stamp: Option<Instant>,
    following: bool,
    /// The shaders are being presented every refresh.
    live: bool,
}

/// What every window's root observes: notified when the bars move (quads)
/// or start moving (shaders).
pub struct Feed;

#[derive(Clone)]
struct Levels {
    registry: Arc<Mutex<Registry>>,
    feed: Entity<Feed>,
}

impl Global for Levels {}

pub fn install(cx: &mut App) {
    let feed = cx.new(|_| Feed);
    cx.set_global(Levels {
        registry: Arc::default(),
        feed,
    });
}

/// The entity a window's root observes to paint the bars again; none where
/// the app was not installed, as in an offscreen render.
pub fn feed(cx: &App) -> Option<Entity<Feed>> {
    cx.try_global::<Levels>().map(|levels| levels.feed.clone())
}

/// A slot's life: created when its row is first drawn, dropped with the
/// row's element state.
struct SlotState {
    id: u64,
    registry: Arc<Mutex<Registry>>,
    levels: BarLevels,
}

impl Drop for SlotState {
    fn drop(&mut self) {
        // The follow sees an empty registry at its next frame and ends.
        let mut registry = self.registry.lock();
        registry.slots.remove(&self.id);
        registry.shaders.remove(&self.id);
    }
}

/// The playing bars in `color`. Inside an element with an id, which keys the
/// slot's state.
pub fn bars(color: Hsla) -> impl IntoElement {
    PlayingBars::new(color, move |place, window, cx| {
        let levels = cx.try_global::<Levels>().cloned()?;
        let state = window.use_keyed_state("playing-bars", cx, |_, _| {
            let mut registry = levels.registry.lock();
            registry.next_id += 1;
            let bars = BarLevels::new();
            bars.set(registry.bands);
            bars.set_live(registry.live);
            SlotState {
                id: registry.next_id,
                registry: levels.registry.clone(),
                levels: bars,
            }
        });
        let (id, bars) = {
            let state = state.read(cx);
            (state.id, state.levels.clone())
        };
        {
            let mut registry = levels.registry.lock();
            if SHADED {
                registry.shaders.insert(id, bars.clone());
            } else {
                registry.slots.insert(id, (place, color));
            }
        }
        follow(&levels, cx);
        Some(bars)
    })
}

/// Paints this window's bars over everything drawn before it, where they
/// are quads. Called last from a window's root.
pub fn paint(cx: &App) -> impl IntoElement + use<> {
    let registry = cx
        .try_global::<Levels>()
        .map(|levels| levels.registry.clone())
        .unwrap_or_default();
    bars_overlay(move |window| {
        let here = window.window_handle();
        let registry = registry.lock();
        let bands = registry.bands;
        for (place, color) in registry.slots.values().filter(|(p, _)| p.window == here) {
            place.paint(window, bands, *color);
        }
    })
}

/// Starts reading the analyser, if nothing is yet. The levels are read and
/// written off the main thread; the main thread hears only when a frame has
/// to be drawn.
fn follow(levels: &Levels, cx: &mut App) {
    if std::mem::replace(&mut levels.registry.lock().following, true) {
        return;
    }
    let engine = engine(cx);
    engine.set_viz_fps(FPS);
    let stream = engine.viz_stream();
    let registry = levels.registry.clone();
    let (wake, mut woken) = futures::channel::mpsc::unbounded::<()>();
    let executor = cx.background_executor().clone();
    cx.background_spawn(async move {
        loop {
            // Only while presented does stillness need a deadline: a paused
            // player publishes nothing, and nothing then wakes this.
            let live = registry.lock().live;
            let heard = if live {
                futures::select_biased! {
                    heard = stream.next().fuse() => match heard {
                        Some(heard) => Some(heard),
                        None => break,
                    },
                    _ = executor.timer(STILL).fuse() => None,
                }
            } else {
                match stream.next().await {
                    Some(heard) => Some(heard),
                    None => break,
                }
            };
            let mut registry = registry.lock();
            if registry.slots.is_empty() && registry.shaders.is_empty() {
                break;
            }
            let Some(heard) = heard else {
                // Still for a while: the bars are presented no longer.
                if registry.live {
                    registry.set_live(false);
                }
                continue;
            };
            if registry.take([heard.low, heard.mid, heard.high]) && registry.notice() {
                wake.unbounded_send(()).ok();
            }
        }
        // Dropping the stream lets the analyser park.
        let mut registry = registry.lock();
        registry.following = false;
        registry.set_live(false);
    })
    .detach();
    let feed = levels.feed.clone();
    cx.spawn(async move |cx| {
        use futures::StreamExt as _;
        while woken.next().await.is_some() {
            cx.update(|cx| feed.update(cx, |_, cx| cx.notify()));
        }
    })
    .detach();
}

impl Registry {
    /// A frame from the analyser. Whether the bars moved; the shaders' bands
    /// are written as it is taken.
    fn take(&mut self, heard: [f32; 3]) -> bool {
        let now = Instant::now();
        // Clamped: a machine that slept owes the bars nothing.
        let elapsed = self
            .stamp
            .map_or(0.0, |then| now.duration_since(then).as_secs_f32().min(0.25));
        self.stamp = Some(now);
        let hold = 0.5_f32.powf(elapsed / FORGET);
        let mut next = self.bands;
        for band in 0..3 {
            self.ceiling[band] = heard[band].max((self.ceiling[band] * hold).max(QUIETEST));
            next[band] = (heard[band] / self.ceiling[band]).min(1.0);
        }
        let moved = next != self.bands;
        self.bands = next;
        if moved {
            for bars in self.shaders.values() {
                bars.set(next);
            }
        }
        moved
    }

    /// The bars moved: whether a frame must be drawn for the windows to see
    /// it. Quads are drawn by the frame; shaders need one only to start
    /// being presented again.
    fn notice(&mut self) -> bool {
        if !self.slots.is_empty() {
            return true;
        }
        if self.live {
            return false;
        }
        self.set_live(true);
        true
    }

    fn set_live(&mut self, live: bool) {
        self.live = live;
        for bars in self.shaders.values() {
            bars.set_live(live);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BarLevels, QUIETEST, Registry};

    #[test]
    fn a_band_is_measured_against_its_own_recent_loudest() {
        let mut registry = Registry::default();
        registry.take([0.5, 0.2, 0.1]);
        assert_eq!(registry.bands[0], 1.0);
        assert_eq!(registry.bands[1], 1.0);
        // Below room tone: measured against it, never normalised up.
        assert!((registry.bands[2] - 0.1 / QUIETEST).abs() < 1e-6);
        assert!(!registry.take([0.5, 0.2, 0.1]));
        registry.take([0.25, 0.2, 0.1]);
        assert!((registry.bands[0] - 0.5).abs() < 0.01);
    }

    #[test]
    fn shaders_take_the_bands_and_ask_for_one_frame_to_start() {
        let mut registry = Registry::default();
        let uniforms = BarLevels::new();
        registry.shaders.insert(1, uniforms.clone());
        assert!(registry.take([0.5, 0.2, 0.1]));
        assert_eq!(uniforms.band(0), 1.0);
        assert!(registry.notice());
        assert!(uniforms.is_live());
        registry.take([0.25, 0.2, 0.1]);
        // Already presented every refresh: no frame needed.
        assert!(!registry.notice());
        registry.set_live(false);
        assert!(!uniforms.is_live());
    }
}
