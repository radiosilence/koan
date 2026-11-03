//! Animation the GPU draws: an element whose pixels come from a fragment
//! program, moved by keyframes the app writes from any thread, with no GPUI
//! frame drawn while it moves.
//!
//! A page declares one with `AnimatedQuad::new(&PROGRAM, &animated)` and
//! keeps the `Animated` handle, writing into it what moves the drawing when
//! the data changes: the levels of the playing bars, a download's progress,
//! the anchor of a seek bar's playhead. A plain value is a uniform (`set`); a
//! moving one is a track (`to`, `segment`, `hold`): from, to, start, length
//! and curve, which the program reads at present time (`koan_track`), so the
//! page and the views around it are not drawn again while it moves.
//!
//! What the kit takes care of:
//! - **The frame loop.** A change wakes the window to present; a track
//!   presents at the display's rate while it moves and once more as it
//!   lands, then the window parks. `run` keeps the clock going for a drawing
//!   that moves with `time` itself (the prism).
//! - **Parking.** A window with nothing changing or moving parks, as an idle
//!   window does. A hidden or occluded window is not woken, and a quad off
//!   screen is not in the scene, so it costs nothing.
//! - **Reduce Motion.** A `decorative` quad's tracks stand at their ends
//!   while the person has asked for less motion; its values still show as
//!   they change.
//! - **Long uptime.** Track times are seconds from an origin that moves to
//!   now every hour, so f32 keeps its precision.
//!
//! Programs are MSL and WGSL source of one `shade` function (GPUI's
//! `ShaderProgram`), given the pixel's position and the quad's size in
//! device pixels, `time`, the uniforms and tracks, and an optional image,
//! and returning the pixel's straight-alpha colour. Where the renderer draws
//! no shaders (`SHADED` is false), a page draws its fallback itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::*;
use parking_lot::Mutex;

pub use gpui_kit::{
    SHADER_PLAIN, SHADER_TRACKS, SHADER_UNIFORMS, ShaderProgram, Track, TrackCurve,
    TrackRepeat as Repeat,
};

/// How a track moves between its ends (#1859).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease {
    Linear,
    /// Core Animation's `easeInEaseOut`, `cubic-bezier(0.42, 0, 0.58, 1)`.
    InOut,
    /// The theme's quick-out, `cubic-bezier(0.2, 0.9, 0.3, 1)`.
    QuickOut,
    /// A CSS cubic Bézier, `cubic-bezier(x1, y1, x2, y2)`.
    Bezier(f32, f32, f32, f32),
    /// A damped spring of unit mass; a retarget carries its velocity on, and
    /// its length is how long it takes to settle.
    Spring {
        stiffness: f32,
        damping: f32,
    },
}

use crate::theme::KoanTheme;

/// Whether this platform's renderer draws shader quads: Metal, and wgpu
/// where it has storage buffers, as every desktop Linux driver does.
pub const SHADED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// What moves an `AnimatedQuad`: its uniforms, written from any thread, and
/// how it runs. Clones share it.
#[derive(Clone, Default)]
pub struct Animated(Arc<Inner>);

#[derive(Default)]
struct Inner {
    uniforms: Arc<ShaderUniforms>,
    /// The window the quad was last painted in, to wake when a value
    /// changes.
    waker: Mutex<Option<Waker>>,
    running: AtomicBool,
    /// Decorative and the person asked for less motion: tracks hold their
    /// ends.
    still: AtomicBool,
}

/// An `Animated` held without keeping it alive: a clock that follows
/// another's running for as long as its quad is drawn.
#[derive(Clone)]
pub struct WeakAnimated(std::sync::Weak<Inner>);

impl WeakAnimated {
    pub fn upgrade(&self) -> Option<Animated> {
        self.0.upgrade().map(Animated)
    }

    /// Whether it is the handle `animated` was taken from.
    pub fn is(&self, animated: &Animated) -> bool {
        std::sync::Weak::ptr_eq(&self.0, &Arc::downgrade(&animated.0))
    }
}

impl Animated {
    pub fn downgrade(&self) -> WeakAnimated {
        WeakAnimated(Arc::downgrade(&self.0))
    }

    pub fn new() -> Self {
        let animated = Self::default();
        animated.0.uniforms.set_origin(Instant::now());
        animated
    }

    /// Track times are seconds from the uniforms' origin, in f32: an hour on,
    /// the origin moves to now and every track with it, so a koan open for
    /// days still reckons its motion to the frame.
    fn rebase(&self) {
        let now = Instant::now();
        let shift = self.0.uniforms.time_at(now);
        if shift < 3600.0 {
            return;
        }
        self.0.uniforms.set_origin(now);
        for k in 0..SHADER_TRACKS {
            let mut track = self.0.uniforms.track(k);
            track.start -= shift;
            self.0.uniforms.set_track(k, anchored(track, 0.0));
        }
    }

    /// Uniform `index`; a change is presented at the next refresh.
    pub fn set(&self, index: usize, value: f32) {
        if self.0.uniforms.set(index, value) {
            self.wake();
        }
    }

    /// The clock tracks are reckoned in: when a frame made now reaches the
    /// screen. What a retarget samples a moving value at.
    pub fn now() -> Instant {
        gpui_kit::present_now()
    }

    /// Where track `k` stands at `at`, as the program draws it.
    pub fn at(&self, k: usize, at: Instant) -> f32 {
        self.0.uniforms.track(k).at(self.0.uniforms.time_at(at))
    }

    /// Whether a track is moving now.
    pub fn moving(&self) -> bool {
        self.0.uniforms.tracks_moving(Self::now())
    }

    /// How long until every track stands still: `None` while one repeats.
    pub fn settled_in(&self) -> Option<Duration> {
        let t = self.0.uniforms.time_at(Self::now());
        let mut last = 0.0_f32;
        for k in 0..SHADER_TRACKS {
            last = last.max(self.0.uniforms.track(k).end()? - t);
        }
        Some(Duration::from_secs_f32(last.max(0.0)))
    }

    /// Track `k` as it was last written.
    pub fn track(&self, k: usize) -> Track {
        self.0.uniforms.track(k)
    }

    /// Track `k` standing at `value`.
    pub fn hold(&self, k: usize, value: f32) {
        self.rebase();
        self.put(k, Track::hold(value));
    }

    /// Track `k` moving from where it stands now to `to` over `length`: a
    /// retarget mid-flight starts from the value, and with a spring the
    /// velocity, it had reached.
    pub fn to(&self, k: usize, to: f32, length: Duration, ease: Ease) {
        // Before any time is read, so the new track is in the new base.
        self.rebase();
        let now = Self::now();
        let current = self.0.uniforms.track(k);
        let t = self.0.uniforms.time_at(now);
        let from = current.at(t);
        let (curve, length) = match ease {
            Ease::Spring { stiffness, damping } => {
                let dt = 1.0 / 240.0;
                let velocity = (current.at(t + dt) - from) / dt;
                (
                    TrackCurve::Spring {
                        stiffness,
                        damping,
                        velocity,
                    },
                    gpui_kit::spring_length(from - to, velocity, stiffness, damping),
                )
            }
            ease => (curve(ease), length.as_secs_f32()),
        };
        self.put(
            k,
            Track {
                from,
                to,
                start: t,
                length,
                curve,
                repeat: Repeat::Once,
            },
        );
    }

    /// Track `k` as a whole segment, for a value whose start is known: an
    /// analyser frame at its own time, a playhead from its anchor, a drift
    /// that breathes (`Repeat::Alternate`).
    #[allow(clippy::too_many_arguments)]
    pub fn segment(
        &self,
        k: usize,
        from: f32,
        to: f32,
        start: Instant,
        length: Duration,
        ease: Ease,
        repeat: Repeat,
    ) {
        self.rebase();
        let (curve, length) = match ease {
            Ease::Spring { stiffness, damping } => (
                TrackCurve::Spring {
                    stiffness,
                    damping,
                    velocity: 0.0,
                },
                gpui_kit::spring_length(from - to, 0.0, stiffness, damping),
            ),
            ease => (curve(ease), length.as_secs_f32()),
        };
        // A repeat starts at its latest whole cycle, worked out on instants
        // so a start long ago keeps its phase exactly.
        let period = match repeat {
            Repeat::Once => 0.0,
            Repeat::Loop => length as f64,
            Repeat::Alternate => 2.0 * length as f64,
        };
        let now = Instant::now();
        let start = match now.checked_duration_since(start) {
            Some(ago) if period > 0.0 => {
                let cycles = (ago.as_secs_f64() / period).floor();
                start + Duration::from_secs_f64(cycles * period)
            }
            _ => start,
        };
        self.put(
            k,
            Track {
                from,
                to,
                start: self.0.uniforms.time_at(start),
                length,
                curve,
                repeat,
            },
        );
    }

    fn put(&self, k: usize, track: Track) {
        let track = if self.0.still.load(Ordering::Relaxed) {
            Track::hold(match track.repeat {
                Repeat::Once => track.to,
                _ => track.from,
            })
        } else {
            track
        };
        self.0.uniforms.set_track(k, track);
        self.wake();
    }

    /// `color` into the four uniforms from `index`, as red, green, blue and
    /// alpha. A colour naming a colour slot (the accent) is read from the
    /// slot at each present, so it moves in the program with no frame drawn.
    pub fn set_color(&self, index: usize, color: Hsla) {
        if self.0.uniforms.set_color(index, color) {
            self.wake();
        }
    }

    /// Uniforms from `start` on.
    pub fn set_all(&self, start: usize, values: &[f32]) {
        let mut changed = false;
        for (i, value) in values.iter().enumerate() {
            changed |= self.0.uniforms.set(start + i, *value);
        }
        if changed {
            self.wake();
        }
    }

    /// Uniform `index` as it stands.
    pub fn get(&self, index: usize) -> f32 {
        self.0.uniforms.get(index)
    }

    /// The moment `time` in the program counts from.
    pub fn set_origin(&self, origin: Instant) {
        if self.0.uniforms.set_origin(origin) {
            self.wake();
        }
    }

    /// Whether the clock runs: presented every refresh while it does, for a
    /// drawing that moves with `time`.
    pub fn run(&self, running: bool) {
        self.0.uniforms.set_live_until(None);
        if self.0.running.swap(running, Ordering::Relaxed) != running {
            self.0.uniforms.set_continuous(running);
            self.0.uniforms.set_live(running);
            self.wake();
        }
    }

    /// Runs the clock for `length` from now and then stops by itself, with
    /// nothing to stop it: a transition.
    pub fn run_for(&self, length: Duration) {
        self.0
            .uniforms
            .set_live_until(Some(Instant::now() + length));
        self.0.running.store(true, Ordering::Relaxed);
        self.0.uniforms.set_continuous(true);
        self.0.uniforms.set_live(true);
        self.wake();
    }

    pub fn is_running(&self) -> bool {
        self.0.running.load(Ordering::Relaxed)
    }

    fn wake(&self) {
        if let Some(waker) = &*self.0.waker.lock() {
            waker.wake();
        }
    }

    /// For an element drawing the quad itself rather than through
    /// `AnimatedQuad`: call as it is laid out (in prepaint), with whether its
    /// motion is decorative, then `paint` it.
    pub fn attach(&self, decorative: bool, window: &mut Window, cx: &mut App) {
        *self.0.waker.lock() = Some(waker(window, cx));
        // A track not written for hours keeps its precision.
        self.rebase();
        let still = decorative && !motion_allowed(cx);
        self.0.still.store(still, Ordering::Relaxed);
        // Written before it was known: what moves stands at its end, or a
        // repeat at its start.
        if still {
            for k in 0..SHADER_TRACKS {
                let track = self.0.uniforms.track(k);
                if track.length > 0.0 {
                    self.0.uniforms.set_track(
                        k,
                        Track::hold(match track.repeat {
                            Repeat::Once => track.to,
                            _ => track.from,
                        }),
                    );
                }
            }
        }
        let running = self.is_running() && !still;
        self.0.uniforms.set_continuous(running);
        self.0.uniforms.set_live(running);
    }

    /// Paints what `paint` paints into a texture of its own, composed over
    /// `bounds` by `program` sampling it (#1859).
    pub fn paint_layer(
        &self,
        program: &'static ShaderProgram,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
        paint: impl FnOnce(&mut Window, &mut App),
    ) {
        window.paint_layer_group(bounds, program, self.0.uniforms.clone(), cx, paint);
    }

    /// Paints the quad over `bounds`, drawn by `program`, sampling `image`.
    pub fn paint(
        &self,
        program: &'static ShaderProgram,
        image: Option<Arc<RenderImage>>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
    ) {
        let uniforms = self.0.uniforms.clone();
        match image {
            Some(image) => {
                window
                    .paint_shader_image(bounds, program, uniforms, image)
                    .ok();
            }
            None => window.paint_shader(bounds, program, uniforms),
        }
    }
}

/// A repeating track's start moved up to its latest whole cycle at or
/// before `now`, seconds of shader time: the same motion, with times kept
/// small, so f32 reckons them to the frame however long ago it began.
fn anchored(mut track: Track, now: f32) -> Track {
    let period = match track.repeat {
        Repeat::Once => return track,
        Repeat::Loop => track.length,
        Repeat::Alternate => 2.0 * track.length,
    };
    if period > 0.0 && track.start < now {
        track.start += ((now - track.start) / period).floor() * period;
    }
    track
}

pub(crate) fn curve(ease: Ease) -> TrackCurve {
    match ease {
        Ease::Linear => TrackCurve::Linear,
        Ease::InOut => TrackCurve::Bezier(0.42, 0.0, 0.58, 1.0),
        Ease::QuickOut => TrackCurve::Bezier(0.2, 0.9, 0.3, 1.0),
        Ease::Bezier(x1, y1, x2, y2) => TrackCurve::Bezier(x1, y1, x2, y2),
        Ease::Spring { .. } => TrackCurve::Linear,
    }
}

fn motion_allowed(cx: &App) -> bool {
    match cx.try_global::<KoanTheme>() {
        Some(_) => KoanTheme::motion_allowed(cx),
        None => !cx.reduce_motion(),
    }
}

/// Asks one window for a present from any thread: a message to a task on
/// the main thread, at most one in flight.
#[derive(Clone)]
struct Waker {
    send: UnboundedSender<()>,
    pending: Arc<AtomicBool>,
}

impl Waker {
    fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            self.send.unbounded_send(()).ok();
        }
    }
}

/// Each window's waker, made the first time a quad is painted in it.
#[derive(Default)]
struct Wakers(HashMap<WindowId, Waker>);

impl Global for Wakers {}

fn waker(window: &mut Window, cx: &mut App) -> Waker {
    let id = window.window_handle().window_id();
    if let Some(waker) = cx.default_global::<Wakers>().0.get(&id) {
        return waker.clone();
    }
    let (send, mut woken) = unbounded();
    let waker = Waker {
        send,
        pending: Arc::default(),
    };
    let pending = waker.pending.clone();
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        while woken.next().await.is_some() {
            pending.store(false, Ordering::Release);
            if cx
                .update_window(handle, |_, window, _| window.request_present())
                .is_err()
            {
                cx.update(|cx| {
                    cx.default_global::<Wakers>().0.remove(&id);
                });
                return;
            }
        }
        // The sender went with the app's globals, and the app with them.
    })
    .detach();
    cx.default_global::<Wakers>().0.insert(id, waker.clone());
    waker
}

/// An element drawn by `program` from `animated`, sized by its parent or by
/// its own style.
#[derive(IntoElement)]
pub struct AnimatedQuad {
    program: &'static ShaderProgram,
    animated: Animated,
    image: Option<Arc<RenderImage>>,
    decorative: bool,
    style: StyleRefinement,
}

impl AnimatedQuad {
    pub fn new(program: &'static ShaderProgram, animated: &Animated) -> Self {
        Self {
            program,
            animated: animated.clone(),
            image: None,
            decorative: false,
            style: StyleRefinement::default(),
        }
    }

    /// The image the program samples (`koan_sample`).
    pub fn image(mut self, image: Arc<RenderImage>) -> Self {
        self.image = Some(image);
        self
    }

    /// Motion for its own sake, such as a drift: its clock stops while the
    /// person asks for less motion. Progress (a playhead, a download) is not
    /// decorative.
    pub fn decorative(mut self) -> Self {
        self.decorative = true;
        self
    }
}

impl Styled for AnimatedQuad {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for AnimatedQuad {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let Self {
            program,
            animated,
            image,
            decorative,
            style,
        } = self;
        let mut quad = canvas(
            move |_, window, cx| {
                animated.attach(decorative, window, cx);
                animated
            },
            move |bounds, animated, window, _| animated.paint(program, image, bounds, window),
        );
        *quad.style() = style;
        quad
    }
}
