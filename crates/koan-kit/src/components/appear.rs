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

use super::layer::{AnimatedLayer, LAYER, standing};
use super::{Animated, Ease, Repeat, SHADED};
use crate::theme::tokens::Motion;
use crate::theme::{KoanTheme, Look, Space};

fn moves(cx: &App) -> bool {
    SHADED && KoanTheme::motion_allowed(cx)
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
}

impl Veil {
    pub fn new(color: Hsla) -> Self {
        Self { color }
    }
}

impl RenderOnce for Veil {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let veil = div().absolute().inset_0().bg(self.color);
        if !moves(cx) {
            return veil.into_any_element();
        }
        AnimatedLayer::new(&entrance("veil", 0.0, window, cx), veil).into_any_element()
    }
}

/// A sheet's panel coming up: fading in as it rises a small step into
/// place. Laid out as the panel is, so it stands where the panel would.
#[derive(IntoElement)]
pub struct Rise {
    child: AnyElement,
    in_place: bool,
}

impl Rise {
    /// On a phone, where the scrim stands the panel on the window's foot,
    /// it comes up from below the window instead, as an iOS sheet does.
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            in_place: false,
        }
    }

    /// The small rise on every platform: a panel hung near the top.
    pub fn in_place(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            in_place: true,
        }
    }
}

impl RenderOnce for Rise {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !moves(cx) {
            return self.child;
        }
        if Look::of(window, cx).touch && !self.in_place {
            return SlideUp::new(self.child).fits().into_any_element();
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
        if !moves(cx) {
            return self.child;
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
                Motion::SETTLE * 2,
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
        } else {
            div().size_full().child(self.child)
        };
        AnimatedLayer::new(&animated, inner).into_any_element()
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

    /// A fade starting now, or none where things go at once.
    pub fn start(cx: &App) -> Option<Self> {
        if !moves(cx) {
            return None;
        }
        let animated = Animated::new();
        standing(&animated);
        let now = Animated::now();
        animated.segment(0, 1.0, 0.0, now, Self::LENGTH, Ease::InOut, Repeat::Once);
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
