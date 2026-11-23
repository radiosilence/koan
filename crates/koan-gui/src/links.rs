//! Names that lead to their pages: a record's title to the record, each
//! credited artist to the artist. Underlined under the pointer, as the
//! theme's link button; a name with no page is plain text.
//!
//! A secondary press on a name notes the thing it names, and the context
//! menu of whatever the name sits in (a list, the transport) opens that
//! thing's menu in place of its own: a record's title gives the record's
//! menu wherever it is drawn.

use koan_kit::app::*;
use koan_runtime::CreditPart;

use crate::library::actions::MenuTarget;
use crate::library::drag::Playable;
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::theme::TypeRole;

use crate::theme::Hsla;

/// One name, leading to `route` when it has one. A click on it is not also
/// a click on whatever it sits in, a row say; a secondary press on it opens
/// the menu of the record or artist it leads to.
pub fn link(
    id: ElementId,
    text: impl Into<SharedString>,
    route: Option<Route>,
    nav: &Entity<Navigator>,
    ink: Hsla,
) -> Stateful<Div> {
    let thing = match route {
        Some(Route::Album(id)) => Some(target(Playable::Album(id), None)),
        Some(Route::Artist(id)) => Some(target(Playable::Artist(id), None)),
        _ => None,
    };
    link_as(id, text, route, thing, nav, ink)
}

/// `link`, whose menu is `thing`'s: a track's title leading to its record
/// gives the track's menu.
pub fn link_as(
    id: ElementId,
    text: impl Into<SharedString>,
    route: Option<Route>,
    thing: Option<MenuTarget>,
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
    let name = crate::components::link_text(id, text, ink, go);
    match thing {
        Some(thing) => name.on_mouse_down(MouseButton::Right, press(thing)),
        None => name,
    }
}

/// The menu target for one thing; `album` the record of a track.
pub fn target(thing: Playable, album: Option<i64>) -> MenuTarget {
    MenuTarget {
        items: std::sync::Arc::from([thing]),
        album,
        after: None,
    }
}

/// What the last secondary press on a name named, until a menu takes it.
#[derive(Default)]
struct Pressed(Option<MenuTarget>);

impl Global for Pressed {}

/// A secondary press on a name: notes `thing` for the menu it opens.
pub fn press(thing: MenuTarget) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    move |_, _, cx| cx.default_global::<Pressed>().0 = Some(thing.clone())
}

/// Forgets the name a press landed on: the window's root, as every
/// secondary press begins, so one no menu took is not taken by the next.
pub fn forget_pressed(cx: &mut App) {
    if cx.has_global::<Pressed>() {
        cx.global_mut::<Pressed>().0 = None;
    }
}

/// The name a secondary press landed on, taken by the menu it opened.
pub fn take_pressed(cx: &mut App) -> Option<MenuTarget> {
    cx.try_global::<Pressed>()?;
    cx.global_mut::<Pressed>().0.take()
}
