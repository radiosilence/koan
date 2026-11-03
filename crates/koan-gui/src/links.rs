//! Names that lead to their pages: a record's title to the record, each
//! credited artist to the artist. Underlined under the pointer, as the
//! theme's link button; a name with no page is plain text.

use koan_kit::app::*;
use koan_runtime::CreditPart;

use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::theme::TypeRole;

use crate::theme::Hsla;

/// One name, leading to `route` when it has one. A click on it is not also
/// a click on whatever it sits in, a row say.
pub fn link(
    id: ElementId,
    text: impl Into<SharedString>,
    route: Option<Route>,
    nav: &Entity<Navigator>,
    ink: Hsla,
) -> Stateful<Div> {
    let nav = nav.clone();
    let go = route.map(|route| {
        crate::components::link_action(move |_, _, cx| {
            nav.update(cx, |nav, cx| {
                if nav.go(route) {
                    cx.notify();
                }
            })
        })
    });
    crate::components::link_text(id, text, ink, go)
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
            // Between several names every name is linked: an unlinked part
            // is a join (" & ", ", "), which keeps its width while the names
            // either side give way.
            if parts.len() > 1 && credited.artist_id.is_none() {
                return div()
                    .flex_none()
                    .whitespace_nowrap()
                    .child(credited.text.clone())
                    .into_any_element();
            }
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

/// One line of names on one baseline; a name that does not fit ends in an
/// ellipsis.
pub fn names(children: Vec<AnyElement>, role: TypeRole, color: Hsla, p: &Paint) -> Div {
    crate::components::name_line(children, role, color, &p.look)
}
