//! Names that lead to their pages: a record's title to the record, each
//! credited artist to the artist. Underlined under the pointer, as the
//! theme's link button; a name with no page is plain text.

use crate::theme::Hsla;
use koan_ffi::CreditPart;
use koan_kit::app::*;

use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::theme::{KoanStyled, TypeRole};

/// One name, leading to `route` when it has one. A click on it is not also
/// a click on whatever it sits in, a row say.
pub fn link(
    id: ElementId,
    text: impl Into<SharedString>,
    route: Option<Route>,
    nav: &Entity<Navigator>,
    ink: Hsla,
) -> Stateful<Div> {
    let name = div().id(id).flex_shrink_0().child(text.into());
    let Some(route) = route else {
        return name;
    };
    let nav = nav.clone();
    name.cursor_pointer()
        .hover(move |s| s.underline().text_color(ink))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            nav.update(cx, |nav, cx| {
                if nav.go(route) {
                    cx.notify();
                }
            });
        })
}

/// A credit, each credited artist its own link; the joins between them, and
/// a credit naming no linked artist, are plain. `fallback` is the credit as
/// text, for a row with no parts.
pub fn credit(
    parts: &[CreditPart],
    fallback: &str,
    id: impl Fn(usize) -> ElementId,
    nav: &Entity<Navigator>,
    ink: Hsla,
) -> Vec<AnyElement> {
    if parts.is_empty() {
        return vec![link(id(0), fallback.to_string(), None, nav, ink).into_any_element()];
    }
    parts
        .iter()
        .enumerate()
        .map(|(ix, credited)| {
            link(
                id(ix),
                credited.text.clone(),
                credited.artist_id.map(Route::Artist),
                nav,
                ink,
            )
            .into_any_element()
        })
        .collect()
}

/// One line of names on one baseline, clipped at its container's edge.
pub fn names(children: Vec<AnyElement>, role: TypeRole, color: Hsla, p: &Paint) -> Div {
    div()
        .flex()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_role(role, &p.look)
        .text_color(color)
        .children(children)
}
