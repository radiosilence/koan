//! What the system draws over a window: a phone's status bar, its notch and
//! home indicator, and the software keyboard. Zero on the desktop.
//!
//! Grounds and the wash run under all of it; content is held clear. An
//! inset change redraws the window, so an element reading these in its
//! render follows rotation and the keyboard.

use gpui::{App, Edges, Global, IntoElement, Pixels, Styled, Window, canvas, px};

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

/// What a phone's shell draws over its pages, which run the full height of
/// the window under it: at the top the status bar, the bar and any notices
/// under it; at the foot the mini player, the tabs and the home indicator.
/// Set by the phone's shell as it measures them; absent, and so zero, on the
/// desktop.
#[derive(Clone, Copy, Default, PartialEq)]
struct PageInsets {
    top: Pixels,
    bottom: Pixels,
}

impl Global for PageInsets {}

fn page_insets(cx: &App) -> PageInsets {
    cx.try_global::<PageInsets>().copied().unwrap_or_default()
}

/// The room a page leaves at its top for the bar drawn over it (zero on the
/// desktop): the first row of a page that scrolls under the bar starts this
/// far down inside its scroll, and a page that does not scroll is held this
/// far down.
pub fn bar_inset(cx: &App) -> Pixels {
    page_insets(cx).top
}

/// The room a page leaves at its end for the foot drawn over it (zero on
/// the desktop): the last row of a page that scrolls under the foot ends
/// this far up from the end of its scroll, and what sits at a page's foot
/// in flow is held this far up.
pub fn foot_inset(cx: &App) -> Pixels {
    page_insets(cx).bottom
}

/// Sets `bar_inset`. A change draws every view again, cached ones included,
/// since each page reads the inset in its own render.
pub fn set_bar_inset(inset: Pixels, window: &mut Window, cx: &mut App) {
    let insets = page_insets(cx);
    set_page_insets(
        PageInsets {
            top: inset,
            ..insets
        },
        window,
        cx,
    );
}

/// Sets `foot_inset`, as `set_bar_inset` does.
pub fn set_foot_inset(inset: Pixels, window: &mut Window, cx: &mut App) {
    let insets = page_insets(cx);
    set_page_insets(
        PageInsets {
            bottom: inset,
            ..insets
        },
        window,
        cx,
    );
}

fn set_page_insets(insets: PageInsets, window: &mut Window, cx: &mut App) {
    if page_insets(cx) == insets {
        return;
    }
    cx.set_global(insets);
    // Usually called while the window draws, when a refresh is not taken.
    window.on_next_frame(|window, _| window.refresh());
}

/// An element filling its parent that keeps `bar_inset` at the parent's
/// height: in what a phone's shell draws over the top of its pages.
pub fn bar_probe() -> impl IntoElement {
    canvas(
        |bounds, window, cx| set_bar_inset(bounds.size.height, window, cx),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// An element filling its parent that keeps `foot_inset` at the parent's
/// height: in what a phone's shell draws over the foot of its pages.
pub fn foot_probe() -> impl IntoElement {
    canvas(
        |bounds, window, cx| set_foot_inset(bounds.size.height, window, cx),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// How much a frosted ground blurs what lies under it.
const FROST_BLUR: Pixels = px(14.);
/// How much of the ground's colour a frosted ground lays over the blur.
const FROST_TINT: f32 = 0.45;

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
