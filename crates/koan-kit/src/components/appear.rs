//! Sheets, questions and the command bar coming and going. As one comes up,
//! the window's veil fades in under it (`Veil`) and its panel rises a step
//! as it fades in (`Rise`), over `Motion::ARRIVE`. One put away fades out
//! where it stood over `Motion::LEAVE` (`Fading`): its owner keeps it for
//! that long and draws it as it stood, inert, taking no input. Both take the
//! ease-in-out curve: on the quick-out a fade this short is mostly gone by
//! its first frame and reads as a cut.
//!
//! Each is a layer the GPU composes (`AnimatedLayer`), so no frame is drawn
//! while it moves. Under Reduce Motion or Low Power Mode, and where the
//! renderer draws no shaders, they come and go at once.
//!
//! An entrance runs once per place: its tracks are kept under the element
//! ids around it, so a sheet replaced in place does not rise again, and one
//! fading out, drawn where it stood, does not replay its entrance.

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::*;

use super::layer::{AnimatedLayer, LAYER, ZOOM, standing};
use super::{Animated, Ease, Repeat, SHADED};
use crate::theme::tokens::Motion;
use crate::theme::{KoanTheme, Look, Space};

fn moves(cx: &App) -> bool {
    SHADED && KoanTheme::motion_allowed(cx)
}

fn under_touch(cx: &App) -> bool {
    cx.try_global::<KoanTheme>()
        .is_some_and(|theme| theme.touch)
}

/// A layer's tracks for an entrance under `id`, set going the first time it
/// is drawn: its opacity from nothing, and its offset from `rise` device
/// pixels below where it stands.
fn entrance(id: &'static str, rise: f32, window: &mut Window, cx: &mut App) -> Animated {
    window
        .use_keyed_state(id, cx, |_, _| {
            let animated = Animated::new();
            standing(&animated);
            let now = Animated::now();
            let segment = |k, from| {
                animated.segment(
                    k,
                    from,
                    if k == 0 { 1.0 } else { 0.0 },
                    now,
                    Motion::ARRIVE,
                    Ease::InOut,
                    Repeat::Once,
                )
            };
            segment(0, 0.0);
            if rise != 0.0 {
                segment(2, rise);
            }
            animated
        })
        .read(cx)
        .clone()
}

/// The dim behind a sheet, covering its parent in `color`, fading in as the
/// sheet comes up.
#[derive(IntoElement)]
pub struct Veil {
    color: Hsla,
    leaving: bool,
}

impl Veil {
    pub fn new(color: Hsla) -> Self {
        Self {
            color,
            leaving: false,
        }
    }

    /// Lifting, over `Motion::SETTLE`, for a panel going back down under
    /// touch.
    pub fn leaving(mut self, leaving: bool) -> Self {
        self.leaving = leaving;
        self
    }
}

impl RenderOnce for Veil {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let veil = div().absolute().inset_0().bg(self.color);
        if !moves(cx) {
            return veil.into_any_element();
        }
        let animated = entrance("veil", 0.0, window, cx);
        if self.leaving && animated.track(0).to != 0.0 {
            animated.to(0, 0.0, Motion::SETTLE, Ease::InOut);
        }
        AnimatedLayer::new(&animated, veil).into_any_element()
    }
}

/// A sheet's panel coming up: fading in as it rises a small step into
/// place. Laid out as the panel is, so it stands where the panel would.
#[derive(IntoElement)]
pub struct Rise {
    child: AnyElement,
    in_place: bool,
    leaving: bool,
}

impl Rise {
    /// On a phone, where the scrim stands the panel on the window's foot,
    /// it comes up from below the window instead, as an iOS sheet does.
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            in_place: false,
            leaving: false,
        }
    }

    /// The small rise on every platform: a panel hung near the top.
    pub fn in_place(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            in_place: true,
            leaving: false,
        }
    }

    /// Under touch, a panel going back down to the foot it came from.
    pub fn leaving(mut self, leaving: bool) -> Self {
        self.leaving = leaving;
        self
    }
}

impl RenderOnce for Rise {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if Look::of(window, cx).touch && !self.in_place {
            return SlideUp::new(self.child)
                .fits()
                .leaving(self.leaving)
                .into_any_element();
        }
        if !moves(cx) {
            return self.child;
        }
        let step = Look::of(window, cx).space(Space::S);
        let device = f32::from(step) * window.scale_factor();
        // Composed over a step more than the panel, so its foot is not cut
        // off while it stands lower.
        AnimatedLayer::new(&entrance("rise", device, window, cx), self.child)
            .room_below(step)
            .into_any_element()
    }
}

/// A screen coming up from the foot of the window, as a phone's full-screen
/// player does from its mini player: the whole height, easing out, on the
/// GPU. Opaque throughout; Reduce Motion shows it at once.
#[derive(IntoElement)]
pub struct SlideUp {
    child: AnyElement,
    leaving: bool,
    fits: bool,
}

impl SlideUp {
    /// How long it takes to come up.
    pub const RISE: Duration = Duration::from_millis(Motion::SETTLE.as_millis() as u64 * 2);

    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            leaving: false,
            fits: false,
        }
    }

    /// Going back down, over `Motion::SETTLE`; the owner lets it go after.
    /// A sheet's panel rather than a screen: as tall as its content, up to
    /// the room above the keyboard and the system's insets.
    pub fn fits(mut self) -> Self {
        self.fits = true;
        self
    }

    pub fn leaving(mut self, leaving: bool) -> Self {
        self.leaving = leaving;
        self
    }
}

impl RenderOnce for SlideUp {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let moving = moves(cx);
        // A sheet's panel takes no more than the room above the keyboard and
        // the system's insets, a step in, shrinking its body to scroll; Now
        // Playing takes the whole screen.
        let inner = if self.fits {
            let room = super::sheet::room(&Look::of(window, cx), window);
            div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .max_h(room)
                .child(self.child)
                .into_any_element()
        } else if moving {
            div().size_full().child(self.child).into_any_element()
        } else {
            self.child
        };
        if !moving {
            return inner;
        }
        let height = f32::from(window.viewport_size().height) * window.scale_factor();
        let state = window.use_keyed_state("slide-up", cx, |_, _| {
            let animated = Animated::new();
            standing(&animated);
            animated.segment(
                2,
                height,
                0.0,
                Animated::now(),
                Self::RISE,
                Ease::QuickOut,
                Repeat::Once,
            );
            (animated, false)
        });
        let (animated, left) = state.read(cx).clone();
        if self.leaving && !left {
            animated.to(2, height, Motion::SETTLE, Ease::InOut);
            state.update(cx, |state, _| state.1 = true);
        }
        AnimatedLayer::new(&animated, inner).into_any_element()
    }
}

/// A picture flying from one place to another across the window, scaled and
/// moved on the GPU: `from` and `to` are window bounds, `child` drawn at
/// `to`, the whole window its layer. `id` names the flight; `length` and
/// `ease` are the owner's, to match what moves with it.
#[derive(IntoElement)]
pub struct Flying {
    id: ElementId,
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    child: AnyElement,
    length: Duration,
}

impl Flying {
    pub fn new(
        id: impl Into<ElementId>,
        from: Bounds<Pixels>,
        to: Bounds<Pixels>,
        length: Duration,
        child: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            from,
            to,
            child: child.into_any_element(),
            length,
        }
    }
}

impl RenderOnce for Flying {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let scale = window.scale_factor();
        let (from, to, length) = (self.from, self.to, self.length);
        let animated = window
            .use_keyed_state(self.id, cx, |_, _| {
                let animated = Animated::new();
                let now = Animated::now();
                let width = |b: Bounds<Pixels>| f32::from(b.size.width).max(1.0);
                let dx = f32::from(from.center().x - to.center().x) * scale;
                let dy = f32::from(from.center().y - to.center().y) * scale;
                animated.hold(0, 1.0);
                for (k, start, end) in [
                    (1, width(from) / width(to), 1.0),
                    (2, dx, 0.0),
                    (3, dy, 0.0),
                ] {
                    animated.segment(k, start, end, now, length, Ease::QuickOut, Repeat::Once);
                }
                animated
            })
            .read(cx)
            .clone();
        let origin = animated.clone();
        div().absolute().inset_0().child(
            AnimatedLayer::new(
                &animated,
                div().absolute().inset_0().child(
                    div()
                        .absolute()
                        .left(to.origin.x)
                        .top(to.origin.y)
                        .w(to.size.width)
                        .h(to.size.height)
                        .child(self.child),
                ),
            )
            .program(&ZOOM)
            .placed(move |bounds, scale| {
                let local = to.center() - bounds.origin;
                origin.set(0, f32::from(local.x) * scale);
                origin.set(1, f32::from(local.y) * scale);
            }),
        )
    }
}

/// A row that moved when the rows around it changed: drawn where it now
/// stands, offset by `dy` points back to where it was and easing into place
/// on the GPU over `Motion::SETTLE`. `id` names the move: it runs once. At
/// once under Reduce Motion.
#[derive(IntoElement)]
pub struct Shifted {
    id: ElementId,
    delta: Point<Pixels>,
    child: AnyElement,
}

impl Shifted {
    pub fn new(id: impl Into<ElementId>, delta: Point<Pixels>, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            delta,
            child: child.into_any_element(),
        }
    }
}

impl RenderOnce for Shifted {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !moves(cx) || self.delta == Point::default() {
            return self.child;
        }
        let scale = window.scale_factor();
        let (dx, dy) = (
            f32::from(self.delta.x) * scale,
            f32::from(self.delta.y) * scale,
        );
        let animated = window
            .use_keyed_state(self.id, cx, |_, _| {
                let animated = Animated::new();
                standing(&animated);
                let now = Animated::now();
                let segment = |k, from: f32| {
                    if from != 0.0 {
                        animated.segment(
                            k,
                            from,
                            0.0,
                            now,
                            Motion::SETTLE,
                            Ease::QuickOut,
                            Repeat::Once,
                        );
                    }
                };
                segment(1, dx);
                segment(2, dy);
                animated
            })
            .read(cx)
            .clone();
        // Where it comes from, as far as the window shows: a row from far
        // off is out of sight until it nears.
        let window_size = window.viewport_size();
        let toward = point(
            self.delta.x.clamp(-window_size.width, window_size.width),
            self.delta.y.clamp(-window_size.height, window_size.height),
        );
        AnimatedLayer::new(&animated, self.child)
            .room_toward(toward)
            .into_any_element()
    }
}

/// Something opening out of a point: a menu from the press that held it, a
/// picture from the sleeve tapped. Scaled up on the GPU from `FROM` as it
/// fades in over `Motion::ARRIVE`, on a spring-like quick-out; at once under
/// Reduce Motion. `at` is the point in window coordinates, clamped into what
/// it opens, so a menu pushed in from the window's edge still opens from the
/// side nearest the press.
#[derive(IntoElement)]
pub struct ZoomIn {
    child: AnyElement,
    at: Point<Pixels>,
}

impl ZoomIn {
    const FROM: f32 = 0.82;

    pub fn new(at: Point<Pixels>, child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            at,
        }
    }
}

impl RenderOnce for ZoomIn {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !moves(cx) {
            return self.child;
        }
        let animated = window
            .use_keyed_state("zoom-in", cx, |_, _| {
                let animated = Animated::new();
                let now = Animated::now();
                for (k, from) in [(0, 0.0), (1, Self::FROM)] {
                    animated.segment(
                        k,
                        from,
                        1.0,
                        now,
                        Motion::ARRIVE,
                        Ease::QuickOut,
                        Repeat::Once,
                    );
                }
                animated
            })
            .read(cx)
            .clone();
        let at = self.at;
        let origin = animated.clone();
        AnimatedLayer::new(&animated, self.child)
            .moving_program(&ZOOM)
            .placed(move |bounds, scale| {
                let local = at - bounds.origin;
                let inside = |v: Pixels, max: Pixels| f32::from(v.clamp(px(0.), max)) * scale;
                origin.set(0, inside(local.x, bounds.size.width));
                origin.set(1, inside(local.y, bounds.size.height));
            })
            .into_any_element()
    }
}

/// A notice hanging under the bar: it drops out from behind the bar as it
/// arrives and goes back up behind it, over `Motion::SETTLE`, on the GPU.
/// Its owner keeps drawing it `leaving` for `Hang::LEAVE` and lets it go
/// after. At once under Reduce Motion.
#[derive(IntoElement)]
pub struct Hang {
    child: AnyElement,
    leaving: bool,
}

impl Hang {
    pub const LEAVE: Duration = Motion::SETTLE;

    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            leaving: false,
        }
    }

    pub fn leaving(mut self, leaving: bool) -> Self {
        self.leaving = leaving;
        self
    }
}

impl RenderOnce for Hang {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !moves(cx) {
            return self.child;
        }
        let animated = window
            .use_keyed_state("hang", cx, |_, _| {
                let animated = Animated::new();
                standing(&animated);
                // Held out of sight until its height is known, at its first
                // paint.
                animated.hold(0, 0.0);
                animated
            })
            .read(cx)
            .clone();
        let leaving = self.leaving;
        let arrives = animated.clone();
        AnimatedLayer::new(&animated, self.child)
            .placed(move |bounds, scale| {
                let height = f32::from(bounds.size.height) * scale;
                let showing = arrives.track(0).to != 0.0;
                if leaving && showing {
                    arrives.to(0, 0.0, Motion::SETTLE, Ease::InOut);
                    arrives.to(2, -height, Motion::SETTLE, Ease::InOut);
                } else if !leaving && !showing {
                    // From behind the bar the first time; told to stay as it
                    // goes, from wherever it had got to.
                    if arrives.track(2).to == 0.0 {
                        arrives.hold(2, -height);
                    }
                    arrives.to(0, 1.0, Motion::SETTLE, Ease::QuickOut);
                    arrives.to(2, 0.0, Motion::SETTLE, Ease::QuickOut);
                }
            })
            .into_any_element()
    }
}

/// Something put away, fading out where it stood. Its owner keeps what it
/// drew beside this, draws it through `draw` in the place it stood until
/// it has run, `LENGTH` after it started, and lets it go then.
#[derive(Clone)]
pub struct Fading {
    animated: Animated,
    started: Instant,
}

impl Fading {
    pub const LENGTH: Duration = Motion::LEAVE;

    /// How long something put away goes on being drawn: under touch its
    /// panel slides back down over `Motion::SETTLE`.
    pub fn length(cx: &App) -> Duration {
        if under_touch(cx) {
            Motion::SETTLE
        } else {
            Self::LENGTH
        }
    }

    /// A fade starting now, or none where things go at once. Under touch the
    /// layer stands whole and its panel and veil, told they are `leaving`,
    /// go by themselves.
    pub fn start(cx: &App) -> Option<Self> {
        if !moves(cx) {
            return None;
        }
        let animated = Animated::new();
        standing(&animated);
        let now = Animated::now();
        if !under_touch(cx) {
            animated.segment(0, 1.0, 0.0, now, Self::LENGTH, Ease::InOut, Repeat::Once);
        } else {
            animated.segment(0, 1.0, 1.0, now, Motion::SETTLE, Ease::Linear, Repeat::Once);
        }
        Some(Self {
            animated,
            started: Instant::now(),
        })
    }

    /// Whether it began fading by `when`: an owner's timer, set going with a
    /// fade and waking `LENGTH` later, lets go of every fade this is true of,
    /// and leaves a later one to its own timer.
    pub fn began_by(&self, when: Instant) -> bool {
        self.started <= when
    }

    /// `child` fading out, composed until its owner lets it go, and drawn
    /// inert: the pointer passes through it to the window, its scrim
    /// included, and the keyboard never tabs into it, so the window takes
    /// input the moment it is put away.
    pub fn draw(&self, child: impl IntoElement) -> AnyElement {
        inert(
            div()
                .absolute()
                .inset_0()
                .child(AnimatedLayer::new(&self.animated, child).program(&LAYER)),
        )
        .into_any_element()
    }
}
