//! The playing indicators: three bars moving with the music, wherever a
//! playing row or record is on screen. koan-kit draws them (`PlayingBars`);
//! this is what moves them.
//!
//! One subscription to the analyser for the whole app, read only while a bar
//! is on screen in a window that shows; with none, the stream is dropped and
//! the analyser parks. A hidden window keeps its rows, and with them its
//! slots, so the windows' roots say when they hide and show (`visibility`).
//! Nothing is polled: the analyser publishes a frame and the follow, on the
//! background executor, wakes on it.
//!
//! On macOS each slot is a shader quad, painted once by its row and replayed
//! with the row's cached view. The follow writes the bands straight into the
//! slots' levels (koan-kit's `Animated`), and each change wakes the window to
//! present its last frame again, composing the quads over the layers it kept:
//! a moving bar costs a small encode, never a GPUI frame, and still bars cost
//! nothing.
//!
//! Elsewhere, until the Linux renderer draws shaders, the bars are quads. A
//! row draws an empty slot, which records where it was laid out and under
//! which clip, and each window's root paints the bars over its content
//! (`paint`), so a frame of the bars redraws the root alone, whose content is
//! a cached view and is replayed.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use futures::StreamExt as _;
use koan_kit::app::*;
use koan_kit::components::{
    BarLevels, BarsPlace, PlayingBars, Prism, SHADED, bars_overlay, renderer_mark,
};
use parking_lot::Mutex;

use koan_core::audio::viz::{BarHeights, VizLevels};

use crate::engine::engine;

use crate::theme::Hsla;

#[derive(Default)]
struct Registry {
    /// Where each slot drawn as quads was laid out, and in what colour.
    slots: HashMap<u64, (BarsPlace, Hsla)>,
    /// Each slot drawn as a shader, by its levels, and its window.
    shaders: HashMap<u64, (WindowId, BarLevels)>,
    /// Each window's rainbow prism, while the rainbow is drawn there.
    prisms: HashMap<WindowId, Prism>,
    /// Windows that do not show: their slots keep nothing running.
    hidden: HashSet<WindowId>,
    /// Rung as a quad slot goes, so the roots paint again without it: a slot
    /// goes at the end of the first frame its row is not drawn in, which
    /// that frame has already painted it into. Names the slot's window.
    gone: Option<futures::channel::mpsc::UnboundedSender<WindowId>>,
    next_id: u64,
    /// How high each bar stands, 0 to 1, low band to high: koan-core's
    /// reading of the levels, which the Mac app shares.
    bars: BarHeights,
    stamp: Option<Instant>,
    following: bool,
}

/// What a window's root observes: notified when that window's bars move,
/// where they are quads.
pub struct Feed;

#[derive(Clone)]
struct Levels {
    registry: Arc<Mutex<Registry>>,
    /// Each window's feed, so the bars repaint only the windows they are in.
    feeds: HashMap<WindowId, Entity<Feed>>,
}

impl Global for Levels {}

/// The music plays on a renderer handed the original file, which no bars
/// can follow; with a processed stream, sent from here, they can.
static HANDED_OFF: AtomicBool = AtomicBool::new(false);

/// Follows whether the music is handed off, from the mirror's renderer
/// output and stream format. Every window is drawn again when it changes,
/// which is when an output is chosen.
fn follow_hand_off(cx: &mut App) {
    let Some(mirror) = cx.try_global::<crate::mirror::Mirror>().cloned() else {
        return;
    };
    let update = move |cx: &mut App| {
        let on_renderer = mirror.renderers.read(cx).0.output.is_some();
        let processed = mirror
            .playback
            .read(cx)
            .0
            .format
            .as_ref()
            .is_some_and(|f| f.dsp.is_some());
        let handed = on_renderer && !processed;
        if HANDED_OFF.swap(handed, Ordering::Relaxed) != handed {
            cx.refresh_windows();
        }
    };
    update(cx);
    let (renderers, playback) = {
        let m = cx.global::<crate::mirror::Mirror>();
        (m.renderers.clone(), m.playback.clone())
    };
    let again = update.clone();
    cx.observe(&renderers, move |_, cx| update(cx)).detach();
    cx.observe(&playback, move |_, cx| again(cx)).detach();
}

pub fn install(cx: &mut App) {
    follow_hand_off(cx);
    let (gone, mut slots_gone) = futures::channel::mpsc::unbounded();
    let registry: Arc<Mutex<Registry>> = Arc::default();
    registry.lock().gone = Some(gone);
    cx.spawn(async move |cx| {
        while let Some(window) = slots_gone.next().await {
            cx.update(|cx| repaint(&[window], cx));
        }
    })
    .detach();
    cx.set_global(Levels {
        registry,
        feeds: HashMap::new(),
    });
}

/// The entity `window`'s root observes to paint its bars again; none where
/// the app was not installed, as in an offscreen render.
pub fn feed(window: WindowId, cx: &mut App) -> Option<Entity<Feed>> {
    cx.try_global::<Levels>()?;
    let made = cx.new(|_| Feed);
    let levels = cx.global_mut::<Levels>();
    Some(levels.feeds.entry(window).or_insert(made).clone())
}

/// `window` was hidden or shown: a hidden window's bars keep nothing
/// running, and one showing again starts them, since it replays its rows
/// rather than drawing them.
pub fn visibility(window: WindowId, shows: bool, cx: &mut App) {
    let Some(levels) = cx.try_global::<Levels>().cloned() else {
        return;
    };
    let live = {
        let mut registry = levels.registry.lock();
        if shows {
            registry.hidden.remove(&window);
        } else {
            registry.hidden.insert(window);
        }
        registry.live()
    };
    if live {
        follow(&levels, cx);
    }
}

/// `window`'s rainbow prism, fed the bands while it shows, or none to stop.
pub fn prism(window: WindowId, prism: Option<Prism>, cx: &mut App) {
    let Some(levels) = cx.try_global::<Levels>().cloned() else {
        return;
    };
    let live = {
        let mut registry = levels.registry.lock();
        match prism {
            Some(prism) => {
                prism.hold_bands(registry.bars.heights());
                registry.prisms.insert(window, prism);
            }
            None => {
                registry.prisms.remove(&window);
            }
        }
        registry.live()
    };
    if live {
        follow(&levels, cx);
    }
}

/// Paints the bars of `windows` again, where they are quads.
fn repaint(windows: &[WindowId], cx: &mut App) {
    let feeds: Vec<Entity<Feed>> = cx
        .try_global::<Levels>()
        .map(|levels| {
            windows
                .iter()
                .filter_map(|w| levels.feeds.get(w).cloned())
                .collect()
        })
        .unwrap_or_default();
    for feed in feeds {
        feed.update(cx, |_, cx| cx.notify());
    }
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
        let painted = registry
            .slots
            .remove(&self.id)
            .map(|(place, _)| place.window.window_id());
        registry.shaders.remove(&self.id);
        if let (Some(window), Some(gone)) = (painted, &registry.gone) {
            gone.unbounded_send(window).ok();
        }
    }
}

/// The playing bars in `color`. Inside an element with an id, which keys the
/// slot's state.
pub fn bars(color: Hsla) -> AnyElement {
    // Played on a renderer that is handed the file itself, nothing reaches
    // the analyser: the slot shows where it plays instead.
    if HANDED_OFF.load(Ordering::Relaxed) {
        return renderer_mark(color).into_any_element();
    }
    let prism = crate::rainbow::Painted::now();
    PlayingBars::new(color, move |place, window, cx| {
        let levels = cx.try_global::<Levels>().cloned()?;
        let state = window.use_keyed_state("playing-bars", cx, |_, _| {
            let mut registry = levels.registry.lock();
            registry.next_id += 1;
            let bars = BarLevels::new();
            bars.hold(registry.bars.heights());
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
                registry
                    .shaders
                    .insert(id, (place.window.window_id(), bars.clone()));
            } else {
                registry.slots.insert(id, (place, color));
            }
        }
        follow(&levels, cx);
        Some(bars)
    })
    .prism(prism)
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
        let bands = registry.bars.heights();
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
    let stream = engine(cx).viz_keyframes();
    let registry = levels.registry.clone();
    let (wake, mut woken) = futures::channel::mpsc::unbounded::<Vec<WindowId>>();
    cx.background_spawn(async move {
        // Whether to stop is decided and recorded under one lock, so a
        // window showing in between starts a new follow rather than
        // finding this one still marked as running. Dropping the stream
        // lets the analyser park.
        loop {
            let Some(heard) = stream.next().await else {
                registry.lock().following = false;
                break;
            };
            let mut registry = registry.lock();
            if !registry.live() {
                registry.following = false;
                break;
            }
            // Shaders are presented by their own windows as the bands are
            // written; quads need a frame of their windows' roots.
            if registry.take(VizLevels {
                low: heard.low,
                mid: heard.mid,
                high: heard.high,
            }) {
                let windows: Vec<WindowId> = registry
                    .slots
                    .values()
                    .map(|(place, _)| place.window.window_id())
                    .filter(|w| !registry.hidden.contains(w))
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect();
                if !windows.is_empty() {
                    wake.unbounded_send(windows).ok();
                }
            }
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Some(windows) = woken.next().await {
            cx.update(|cx| repaint(&windows, cx));
        }
    })
    .detach();
}

impl Registry {
    /// Whether a bar shows in a window that shows.
    fn live(&self) -> bool {
        let shows = |w: &WindowId| !self.hidden.contains(w);
        self.slots
            .values()
            .any(|(place, _)| shows(&place.window.window_id()))
            || self.shaders.values().any(|(window, _)| shows(window))
            || self.prisms.keys().any(shows)
    }

    /// A frame from the analyser. Whether the bars moved; the shaders' bands
    /// are written as it is taken.
    fn take(&mut self, heard: VizLevels) -> bool {
        let now = Instant::now();
        // Clamped: a machine that slept owes the bars nothing.
        let elapsed = self
            .stamp
            .map_or(0.0, |then| now.duration_since(then).as_secs_f32().min(0.25));
        self.stamp = Some(now);
        let Some(next) = self.bars.take(heard, now) else {
            return false;
        };
        // Glided to over the gap between the last two frames, so it is
        // reached about as the next arrives; the first frame lands.
        let over = std::time::Duration::from_secs_f32(elapsed);
        for (_, bars) in self.shaders.values() {
            bars.glide(next, over);
        }
        for prism in self.prisms.values() {
            prism.hear(next, over);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{BarLevels, Registry, VizLevels, WindowId};

    fn heard(low: f32, mid: f32, high: f32) -> VizLevels {
        VizLevels { low, mid, high }
    }

    #[test]
    fn a_hidden_window_keeps_no_bars_running() {
        let mut registry = Registry::default();
        registry
            .shaders
            .insert(1, (WindowId::from(1), BarLevels::new()));
        assert!(registry.live());
        registry.hidden.insert(WindowId::from(1));
        assert!(!registry.live());
        registry
            .shaders
            .insert(2, (WindowId::from(2), BarLevels::new()));
        assert!(registry.live(), "a bar in another window that shows");
    }

    #[test]
    fn shaders_take_the_bands() {
        let mut registry = Registry::default();
        let levels = BarLevels::new();
        registry
            .shaders
            .insert(1, (WindowId::from(1), levels.clone()));
        assert!(registry.take(heard(0.5, 0.2, 0.1)));
        assert_eq!(levels.target(0), 1.0);
        registry.take(heard(0.25, 0.2, 0.1));
        assert!((levels.target(0) - 0.5).abs() < 0.01);
    }
}
