//! What moves the playing bars (`PlayingBars`): one feed of bands for the
//! whole app, read only while a bar is on screen in a window that shows.
//! With none, the source's stream is dropped, so whatever produces the
//! bands can park. A hidden window keeps its rows, and with them its slots,
//! so the windows' roots say when they hide and show (`bars_visibility`).
//! Nothing is polled: the source yields a frame and the follow, on the
//! background executor, wakes on it.
//!
//! Where the renderer draws shaders, each slot is a shader quad, painted
//! once by its row and replayed with the row's cached view. The follow
//! writes the bands straight into the slots' levels (`BarLevels`), and each
//! change wakes the window to present its last frame again: a moving bar
//! costs a small encode, never a frame, and still bars cost nothing.
//!
//! Elsewhere the bars are quads. A row draws an empty slot, which records
//! where it was laid out and under which clip, and each window's root paints
//! the bars over its content (`paint_bars`), so a frame of the bars redraws
//! the root alone, whose content is a cached view and is replayed.
//!
//! The app says where the bands come from (`install_bars`): a stream with
//! an item for every frame of its analyser, the heights, 0 to 1, low band
//! to high, where they moved and `None` where they did not. Every frame
//! is a moment to stop at once nothing shows, and the gap a glide runs
//! over is the gap between frames.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt as _;
use futures::stream::BoxStream;
use gpui::{App, AppContext, Entity, Global, Hsla, IntoElement, WindowId};
use parking_lot::Mutex;

use super::{BarLevels, BarsPlace, PlayingBars, Prism, SHADED, bars_overlay};

/// Where the bands come from: asked for a stream when a bar first shows,
/// which is dropped when none does.
pub type BandSource = Arc<dyn Fn(&mut App) -> BoxStream<'static, Option<[f32; 3]>> + Send + Sync>;

#[derive(Default)]
struct Registry {
    /// Where each slot drawn as quads was laid out, and in what colour.
    slots: HashMap<u64, (BarsPlace, Hsla)>,
    /// Each slot drawn as a shader, by its levels, and its window.
    shaders: HashMap<u64, (WindowId, BarLevels)>,
    /// Each window's rainbow prism, while it is drawn there.
    prisms: HashMap<WindowId, Prism>,
    /// Windows that do not show: their slots keep nothing running.
    hidden: HashSet<WindowId>,
    /// Rung as a quad slot goes, so the roots paint again without it: a slot
    /// goes at the end of the first frame its row is not drawn in, which
    /// that frame has already painted it into. Names the slot's window.
    gone: Option<futures::channel::mpsc::UnboundedSender<WindowId>>,
    next_id: u64,
    /// How high each bar stands now.
    heights: [f32; 3],
    stamp: Option<Instant>,
    following: bool,
}

/// What a window's root observes: notified when that window's bars move,
/// where they are quads.
pub struct BarsFeed;

#[derive(Clone)]
struct Bars {
    registry: Arc<Mutex<Registry>>,
    /// Each window's feed, so the bars repaint only the windows they are in.
    feeds: HashMap<WindowId, Entity<BarsFeed>>,
    source: BandSource,
}

impl Global for Bars {}

/// Sets up the bars' feed, with the bands from `source`.
pub fn install_bars(source: BandSource, cx: &mut App) {
    let (gone, mut slots_gone) = futures::channel::mpsc::unbounded();
    let registry: Arc<Mutex<Registry>> = Arc::default();
    registry.lock().gone = Some(gone);
    cx.spawn(async move |cx| {
        while let Some(window) = slots_gone.next().await {
            cx.update(|cx| repaint(&[window], cx));
        }
    })
    .detach();
    // A closed window's feed and its hidden mark go with it.
    let closing = registry.clone();
    cx.on_window_closed(move |cx, window| {
        closing.lock().hidden.remove(&window);
        if cx.has_global::<Bars>() {
            cx.global_mut::<Bars>().feeds.remove(&window);
        }
    })
    .detach();
    cx.set_global(Bars {
        registry,
        feeds: HashMap::new(),
        source,
    });
}

/// The entity `window`'s root observes to paint its bars again; none where
/// the feed was not installed, as in an offscreen render.
pub fn bars_feed(window: WindowId, cx: &mut App) -> Option<Entity<BarsFeed>> {
    let kept = cx.try_global::<Bars>()?.feeds.get(&window).cloned();
    if kept.is_some() {
        return kept;
    }
    let made = cx.new(|_| BarsFeed);
    cx.global_mut::<Bars>().feeds.insert(window, made.clone());
    Some(made)
}

/// `window` was hidden or shown: a hidden window's bars keep nothing
/// running, and one showing again starts them, since it replays its rows
/// rather than drawing them.
pub fn bars_visibility(window: WindowId, shows: bool, cx: &mut App) {
    let Some(bars) = cx.try_global::<Bars>().cloned() else {
        return;
    };
    let live = {
        let mut registry = bars.registry.lock();
        if shows {
            registry.hidden.remove(&window);
        } else {
            registry.hidden.insert(window);
        }
        registry.live()
    };
    if live {
        follow(&bars, cx);
    }
}

/// `window`'s prism, fed the bands while it shows, or none to stop.
pub fn bars_prism(window: WindowId, prism: Option<Prism>, cx: &mut App) {
    let Some(bars) = cx.try_global::<Bars>().cloned() else {
        return;
    };
    let live = {
        let mut registry = bars.registry.lock();
        match prism {
            Some(prism) => {
                prism.hold_bands(registry.heights);
                registry.prisms.insert(window, prism);
            }
            None => {
                registry.prisms.remove(&window);
            }
        }
        registry.live()
    };
    if live {
        follow(&bars, cx);
    }
}

/// Paints the bars of `windows` again, where they are quads.
fn repaint(windows: &[WindowId], cx: &mut App) {
    let feeds: Vec<Entity<BarsFeed>> = cx
        .try_global::<Bars>()
        .map(|bars| {
            windows
                .iter()
                .filter_map(|w| bars.feeds.get(w).cloned())
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

/// The playing bars in `color`, moved by the feed. Inside an element with
/// an id, which keys the slot's state.
pub fn live_bars(color: Hsla) -> PlayingBars {
    PlayingBars::new(color, move |place, window, cx| {
        let bars = cx.try_global::<Bars>().cloned()?;
        let state = window.use_keyed_state("playing-bars", cx, |_, _| {
            let mut registry = bars.registry.lock();
            registry.next_id += 1;
            let levels = BarLevels::new();
            levels.hold(registry.heights);
            SlotState {
                id: registry.next_id,
                registry: bars.registry.clone(),
                levels,
            }
        });
        let (id, levels) = {
            let state = state.read(cx);
            (state.id, state.levels.clone())
        };
        {
            let mut registry = bars.registry.lock();
            if SHADED {
                registry
                    .shaders
                    .insert(id, (place.window.window_id(), levels.clone()));
            } else {
                registry.slots.insert(id, (place, color));
            }
        }
        follow(&bars, cx);
        Some(levels)
    })
}

/// Paints this window's bars over everything drawn before it, where they
/// are quads. Drawn last by a window's root.
pub fn paint_bars(cx: &App) -> impl IntoElement + use<> {
    let registry = cx
        .try_global::<Bars>()
        .map(|bars| bars.registry.clone())
        .unwrap_or_default();
    bars_overlay(move |window| {
        let here = window.window_handle();
        let registry = registry.lock();
        let bands = registry.heights;
        for (place, color) in registry.slots.values().filter(|(p, _)| p.window == here) {
            place.paint(window, bands, *color);
        }
    })
}

/// Starts reading the source, if nothing is yet. The bands are read and
/// written off the main thread; the main thread hears only when a frame has
/// to be drawn.
fn follow(bars: &Bars, cx: &mut App) {
    if std::mem::replace(&mut bars.registry.lock().following, true) {
        return;
    }
    let mut stream = (bars.source)(cx);
    let registry = bars.registry.clone();
    let (wake, mut woken) = futures::channel::mpsc::unbounded::<Vec<WindowId>>();
    cx.background_spawn(async move {
        // Whether to stop is decided and recorded under one lock, so a
        // window showing in between starts a new follow rather than
        // finding this one still marked as running. Dropping the stream
        // lets the source park.
        loop {
            let Some(heights) = stream.next().await else {
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
            if !registry.take(heights) {
                continue;
            }
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

    /// A frame from the source. Whether the bars moved; the shaders' bands
    /// are written as it is taken.
    fn take(&mut self, heights: Option<[f32; 3]>) -> bool {
        let now = Instant::now();
        // Clamped: a machine that slept owes the bars nothing.
        let elapsed = self
            .stamp
            .map_or(0.0, |then| now.duration_since(then).as_secs_f32().min(0.25));
        self.stamp = Some(now);
        let Some(heights) = heights else {
            return false;
        };
        self.heights = heights;
        // Glided to over the gap between the last two frames, so it is
        // reached about as the next arrives; the first frame lands.
        let over = std::time::Duration::from_secs_f32(elapsed);
        for (_, levels) in self.shaders.values() {
            levels.glide(heights, over);
        }
        for prism in self.prisms.values() {
            prism.hear(heights, over);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{BarLevels, Registry, WindowId};

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
        assert!(registry.take(Some([1.0, 0.4, 0.2])));
        assert_eq!(levels.target(0), 1.0);
        assert!(!registry.take(None), "a frame that moved nothing");
        assert_eq!(levels.target(0), 1.0);
        registry.take(Some([0.5, 0.4, 0.2]));
        assert!((levels.target(0) - 0.5).abs() < 0.01);
    }
}
