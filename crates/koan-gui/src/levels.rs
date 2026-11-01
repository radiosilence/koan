//! The playing indicators: three bars moving with the music, wherever a
//! playing row or record is on screen.
//!
//! One subscription to the analyser for the whole app, read only while a bar
//! is on screen; with none, the stream is dropped and the analyser parks.
//! Nothing is polled: the analyser publishes a frame and the follow wakes on
//! it, and a paused player publishes nothing once the bars have fallen.
//!
//! A frame must not redraw the page a bar sits in: GPUI redraws a view and
//! every view above it, so a bar drawn by its row would lay out the whole
//! queue or album grid at the analyser's rate. Instead a row draws an empty
//! slot (`bars`), which records where it was laid out and the clip it was
//! laid out under, and each window's root paints the bars over its content
//! (`paint`). A frame then redraws the root alone, whose content is a cached
//! view and is replayed. A slot lives as long as its row is drawn: its state
//! is the row's element state, dropped by GPUI the first frame the row is not
//! drawn.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use koan_kit::raw::*;

use crate::engine::engine;

/// The bars' size, the Mac's: three 2-point bars 2 points apart, 11 tall.
const BAR: f32 = 2.0;
const GAP: f32 = 2.0;
const BARS: usize = 3;
pub const WIDTH: Pixels = px(BARS as f32 * BAR + (BARS - 1) as f32 * GAP);
pub const HEIGHT: Pixels = px(11.);
/// How far a bar stands with nothing playing through it, so a still bar
/// reads as a mark rather than as nothing.
const FLOOR: f32 = 0.15;
/// A band below this is room tone: it never sets a ceiling, and silence is
/// measured against it, so a quiet passage stays flat.
const QUIETEST: f32 = 0.12;
/// Seconds a band takes to forget half of a loud passage.
const FORGET: f32 = 4.0;
/// The analyser's rate while bars are on screen.
const FPS: u8 = 60;

/// Where a bar was laid out.
#[derive(Clone, Copy)]
struct Slot {
    window: WindowId,
    bounds: Bounds<Pixels>,
    mask: ContentMask<Pixels>,
    color: Hsla,
}

#[derive(Default)]
struct Registry {
    slots: HashMap<u64, Slot>,
    next_id: u64,
    /// How high each bar stands, 0 to 1, low band to high: each band
    /// against its own recent range, so a record mastered quiet moves as
    /// much as a loud one.
    bands: [f32; 3],
    ceiling: [f32; 3],
    stamp: Option<Instant>,
    following: bool,
}

/// What every window's root observes: notified when the bars move.
pub struct Feed;

#[derive(Clone)]
struct Levels {
    registry: Rc<RefCell<Registry>>,
    feed: Entity<Feed>,
}

impl Global for Levels {}

pub fn install(cx: &mut App) {
    let feed = cx.new(|_| Feed);
    cx.set_global(Levels {
        registry: Rc::default(),
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
    registry: Rc<RefCell<Registry>>,
}

impl Drop for SlotState {
    fn drop(&mut self) {
        // The follow sees an empty registry at its next frame and ends.
        self.registry.borrow_mut().slots.remove(&self.id);
    }
}

/// The playing bars in `color`, as a slot the window's root paints them
/// into. Inside an element with an id, which keys the slot's state.
pub fn bars(color: Hsla) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            let Some(levels) = cx.try_global::<Levels>().cloned() else {
                return;
            };
            let state = window.use_keyed_state("playing-bars", cx, |_, _| {
                let mut registry = levels.registry.borrow_mut();
                registry.next_id += 1;
                SlotState {
                    id: registry.next_id,
                    registry: levels.registry.clone(),
                }
            });
            let id = state.read(cx).id;
            let slot = Slot {
                window: window.window_handle().window_id(),
                bounds,
                mask: window.content_mask(),
                color,
            };
            levels.registry.borrow_mut().slots.insert(id, slot);
            follow(&levels, cx);
        },
        |_, _, _, _| {},
    )
    .w(WIDTH)
    .h(HEIGHT)
    .flex_shrink_0()
}

/// Paints this window's bars over everything drawn before it. Called last
/// from a window's root.
pub fn paint(cx: &App) -> impl IntoElement + use<> {
    let registry = cx
        .try_global::<Levels>()
        .map(|levels| levels.registry.clone())
        .unwrap_or_default();
    canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            let here = window.window_handle().window_id();
            let registry = registry.borrow();
            let bands = registry.bands;
            for slot in registry.slots.values().filter(|s| s.window == here) {
                window.with_content_mask(Some(slot.mask), |window| {
                    for (i, band) in bands.iter().enumerate() {
                        let height = slot.bounds.size.height * band.max(FLOOR);
                        let x = slot.bounds.origin.x + px(i as f32 * (BAR + GAP));
                        let y = slot.bounds.origin.y + slot.bounds.size.height - height;
                        window.paint_quad(fill(
                            Bounds::new(point(x, y), size(px(BAR), height)),
                            slot.color,
                        ));
                    }
                });
            }
        },
    )
    .absolute()
    .size_full()
}

/// Starts reading the analyser, if nothing is yet.
fn follow(levels: &Levels, cx: &mut App) {
    if std::mem::replace(&mut levels.registry.borrow_mut().following, true) {
        return;
    }
    let engine = engine(cx);
    engine.set_viz_fps(FPS);
    let stream = engine.viz_stream();
    let registry = levels.registry.clone();
    let feed = levels.feed.clone();
    cx.spawn(async move |cx| {
        while let Some(heard) = stream.next().await {
            let moved = {
                let mut registry = registry.borrow_mut();
                if registry.slots.is_empty() {
                    break;
                }
                registry.take([heard.low, heard.mid, heard.high])
            };
            if moved {
                cx.update(|cx| feed.update(cx, |_, cx| cx.notify()));
            }
        }
        // Dropping the stream lets the analyser park.
        registry.borrow_mut().following = false;
    })
    .detach();
}

impl Registry {
    /// A frame from the analyser. Whether the bars moved.
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
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::{QUIETEST, Registry};

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
}
