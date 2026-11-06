//! What the system draws over a window: a phone's status bar, its notch and
//! home indicator, and the software keyboard. Zero on the desktop.
//!
//! Grounds and the wash run under all of it; content is held clear. An
//! inset change redraws the window, so an element reading these in its
//! render follows rotation and the keyboard.

use gpui::{App, Edges, Global, Pixels, Styled, Window, px};

use crate::theme::Look;

/// Which of the system's insets content keeps clear of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    /// The status bar, the notch and the home indicator.
    System,
    /// Those and the keyboard while it is up: for what a field sits in.
    Keyboard,
}

/// The insets `clear` names, per edge.
pub fn system_insets(clear: Clear, window: &Window) -> Edges<Pixels> {
    let insets = window.insets();
    match clear {
        Clear::System => insets.safe_area,
        Clear::Keyboard => insets.effective(),
    }
}

pub trait KoanInsets: Styled + Sized {
    /// Padding that holds the element's children clear of what the system
    /// draws over the window, while the element's own ground fills it.
    fn clear_of_system(self, clear: Clear, window: &Window) -> Self {
        let edges = system_insets(clear, window);
        self.pt(edges.top)
            .pr(edges.right)
            .pb(edges.bottom)
            .pl(edges.left)
    }
}

impl<E: Styled> KoanInsets for E {}

/// How far a phone's bar reaches down over the page from the window's top:
/// the status bar and the bar itself, which the page scrolls under. Set by
/// the phone's shell; absent, and so zero, on the desktop.
#[derive(Clone, Copy, Default, PartialEq)]
struct BarInset(Pixels);

impl Global for BarInset {}

/// The room a page leaves at its top for the bar drawn over it (zero on the
/// desktop): the first row of a page that scrolls under the bar starts this
/// far down inside its scroll, and a page that does not scroll is held this
/// far down.
pub fn bar_inset(cx: &App) -> Pixels {
    cx.try_global::<BarInset>().map_or(px(0.), |inset| inset.0)
}

/// Sets `bar_inset`. A change draws every view again, cached ones included,
/// since each page reads the inset in its own render.
pub fn set_bar_inset(inset: Pixels, window: &mut Window, cx: &mut App) {
    if bar_inset(cx) == inset {
        return;
    }
    cx.set_global(BarInset(inset));
    window.on_next_frame(|window, _| window.refresh());
}

/// How much a frosted ground blurs what lies under it.
const FROST_BLUR: Pixels = px(24.);
/// How much of the ground's colour a frosted ground lays over the blur.
const FROST_TINT: f32 = 0.72;

pub trait KoanFrosted: Styled + Sized {
    /// The ground of a bar the page scrolls under, as the system's bars
    /// are: what lies under it blurred, tinted with the page's ground. With
    /// less transparency asked for, the ground opaque and nothing blurred.
    /// Where the renderer cannot blur, the tint alone.
    fn frosted(self, look: &Look) -> Self {
        let ground = look.palette.bg.hsla();
        if look.opaque {
            self.bg(ground)
        } else {
            self.backdrop_blur(FROST_BLUR)
                .bg(ground.opacity(FROST_TINT))
        }
    }
}

impl<E: Styled> KoanFrosted for E {}
