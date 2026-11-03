//! A thing named in a collection as a chip: an artist among records and
//! tracks, the Mac's `ArtistPill`. A glyph, or a selection tick while
//! picking, and the name on one line, square on `rule`. A long name (a
//! classical credit naming the soloist, orchestra and conductor) stops at
//! 260 points and truncates rather than laying a paragraph on its side.
//!
//! Listeners attach to it as to a `div` (click, drag, menus); what a click
//! does is the owner's.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::{Icon, KoanIcon, KoanStyled, Look, TypeRole};

#[derive(IntoElement)]
pub struct Pill {
    base: Stateful<Div>,
    icon: Icon,
    text: SharedString,
    /// `Some` while picking: whether this one is picked.
    tick: Option<bool>,
    cursor: bool,
    spot: Option<super::CursorSpot>,
}

impl Pill {
    /// The longest a pill grows before its name truncates, at 100 %.
    const MAX_WIDTH: f32 = 260.0;

    /// A pill names a thing, so its text keeps its case.
    pub fn new(id: impl Into<ElementId>, icon: Icon, text: impl Into<SharedString>) -> Self {
        Self {
            base: div().id(id),
            icon,
            text: text.into(),
            tick: None,
            cursor: false,
            spot: None,
        }
    }

    /// While the page is picking: the tick in the glyph's place, filled in
    /// the accent when `picked`.
    pub fn ticking(mut self, picked: bool) -> Self {
        self.tick = Some(picked);
        self
    }

    /// Under the page's keyboard cursor while the page holds the keyboard:
    /// the focus ring, once the keyboard was used last.
    pub fn cursor(mut self, cursor: bool) -> Self {
        self.cursor = cursor;
        self
    }

    /// Where the cursor's ring tells its page it was laid out, while this
    /// is under the cursor: what the page's scroll area follows and the menu
    /// key opens under.
    pub fn spot(mut self, spot: &super::CursorSpot) -> Self {
        self.spot = Some(spot.clone());
        self
    }

    /// The gap between pills in a flow, both ways.
    pub fn gap(look: &Look) -> Pixels {
        look.spaced(8.0)
    }
}

impl InteractiveElement for Pill {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for Pill {}

impl RenderOnce for Pill {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        let glyph = match self.tick {
            Some(true) => KoanIcon::new(Icon::Picked).color(look.accent.hsla()),
            Some(false) => KoanIcon::new(Icon::Unpicked).color(p.muted.hsla()),
            None => KoanIcon::new(self.icon).color(p.muted.hsla()),
        }
        .role(TypeRole::Fine);
        let ring = (self.cursor && window.last_input_was_keyboard())
            .then(|| super::cursor_ring(&look, self.spot.as_ref()));
        self.base
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(look.spaced(5.0))
            .max_w(look.spaced(Self::MAX_WIDTH))
            .px(look.spaced(11.0))
            .py(look.spaced(6.0))
            .bg(p.rule.hsla())
            .cursor_pointer()
            .child(glyph)
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_role(TypeRole::Control, &look)
                    .text_color(p.ink.hsla())
                    .child(self.text),
            )
            .children(ring)
    }
}
