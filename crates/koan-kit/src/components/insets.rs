//! What the system draws over a window: a phone's status bar, its notch and
//! home indicator, and the software keyboard. Zero on the desktop.
//!
//! Grounds and the wash run under all of it; content is held clear. An
//! inset change redraws the window, so an element reading these in its
//! render follows rotation and the keyboard.

use gpui::{Edges, Pixels, Styled, Window};

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
