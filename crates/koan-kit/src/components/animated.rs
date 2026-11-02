//! Animation the GPU draws: an element whose pixels come from a fragment
//! program, moved by values the app writes from any thread or by the clock,
//! with no GPUI frame drawn while it moves.
//!
//! A page declares one with `AnimatedQuad::new(&PROGRAM, &animated)` and
//! keeps the `Animated` handle, writing into it what moves the drawing: the
//! levels of the playing bars, a download's progress, the anchor of a seek
//! bar's playhead. Each frame is presented with the values as they are then;
//! the page and the views around it are not drawn again.
//!
//! What the kit takes care of:
//! - **The frame loop.** Driven by values, a change wakes the window to
//!   present once (`Animated::set`); nothing runs between changes. Driven by
//!   the clock (`Animated::run`), the window presents every refresh while it
//!   runs, and `time` in the program counts from the origin set. A
//!   transition runs for its length and stops by itself (`run_for`).
//! - **Parking.** A window with nothing changing or running parks, as an
//!   idle window does. A hidden or occluded window is not woken, and a quad
//!   off screen is not in the scene, so it costs nothing.
//! - **Reduce Motion.** A `decorative` quad's clock stops while the person
//!   has asked for less motion; its values still show as they change.
//!
//! Programs are MSL and WGSL source of one `shade` function (GPUI's
//! `ShaderProgram`), given the pixel's position and the quad's size in
//! device pixels, `time`, sixteen uniforms and an optional image, and
//! returning the pixel's straight-alpha colour. A `prepare` function may work
//! out per present, on the CPU, what every pixel would otherwise compute
//! alike. Where the renderer draws no shaders (`SHADED` is false), a page
//! draws its fallback itself.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::*;
use parking_lot::Mutex;

pub use gpui_kit::{SHADER_UNIFORMS, ShaderProgram};

use crate::theme::KoanTheme;

/// Whether this platform's renderer draws shader quads.
pub const SHADED: bool = cfg!(target_os = "macos");

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
}

impl Animated {
    pub fn new() -> Self {
        Self::default()
    }

    /// Uniform `index`; a change is presented at the next refresh.
    pub fn set(&self, index: usize, value: f32) {
        if self.0.uniforms.set(index, value) {
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
        let still = decorative && !motion_allowed(cx);
        let running = self.is_running() && !still;
        self.0.uniforms.set_continuous(running);
        self.0.uniforms.set_live(running);
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
                break;
            }
        }
        cx.update(|cx| {
            cx.default_global::<Wakers>().0.remove(&id);
        });
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
