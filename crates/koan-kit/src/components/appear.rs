//! Sheets, questions and the command bar coming and going. As one comes up,
//! the window's veil fades in under it (`Veil`) and its panel rises a step
//! as it fades in (`Rise`), over `Motion::ARRIVE`. One put away fades out
//! where it stood over `Motion::LEAVE` (`Fading`): its owner keeps it for
//! that long and draws it as it stood, taking no input. Both take the
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
}

impl Rise {
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
        }
    }
}

impl RenderOnce for Rise {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
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

    /// `child` fading out, composed until its owner lets it go, and covered
    /// so that nothing in it takes the pointer: a question's verb pressed
    /// again as it fades does nothing.
    pub fn draw(&self, child: impl IntoElement) -> AnyElement {
        div()
            .absolute()
            .inset_0()
            .child(AnimatedLayer::new(&self.animated, child).program(&LAYER))
            .child(div().absolute().inset_0().occlude())
            .into_any_element()
    }
}
