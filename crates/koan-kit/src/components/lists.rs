//! The virtualised lists a long page sits in, and their scrollbar: GPUI's
//! `uniform_list` (rows of one height, laid out a screenful at a time) and
//! `list` (rows that measure themselves), with the handles that keep their
//! place, and the kōan scrollbar over either. A page building rows of
//! `TableRow`s takes `table_list`; one laying out something else in rows,
//! such as a grid of tiles a row at a time, takes these.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    Div, ElementId, EntityId, InteractiveElement, IntoElement, Pixels, Point, SharedString,
    Stateful, Styled, canvas, div, px,
};

pub use gpui_kit::{
    List, ListAlignment, ListOffset, ListState, ScrollStrategy, UniformList,
    UniformListScrollHandle, list, uniform_list,
};

/// How far past the screen a `list` lays out rows, so a scroll lands on
/// rows already measured.
pub const OVERDRAW: Pixels = px(400.0);

/// The overlay scrollbar for a virtualised list's handle, over the list's
/// right edge.
pub fn list_scrollbar<H>(handle: &H) -> impl IntoElement
where
    H: gpui_kit::base::ScrollbarHandle + Clone,
{
    gpui_kit::base::Scrollbar::vertical(handle)
}

/// What a virtualised list sits in for a screen reader: a list named for
/// what it holds, its rows (`TableRow::spoken`) its items.
pub fn spoken_list(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Stateful<Div> {
    super::accessible(div().id(id), gpui_kit::Role::List, Some(name.into()))
}

/// Scrolls row `ix` of a `uniform_list` to the middle, with rows either side
/// of it drawn: what a screen reader moving down the list needs, since only
/// rows drawn are in the accessibility tree.
pub fn reveal_around(handle: &UniformListScrollHandle, ix: usize) {
    handle.scroll_to_item(ix, ScrollStrategy::Center);
}

/// A scroll offset `y` down from the top, as a list's handle takes it.
pub fn offset_down(y: Pixels) -> Point<Pixels> {
    Point { x: px(0.0), y: -y }
}

/// An element filling its parent that keeps the parent's width in `width`,
/// and redraws `owner` when it changes: a page laying out a grid or a list
/// by the width it is given.
pub fn width_probe(width: Rc<Cell<Option<Pixels>>>, owner: EntityId) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            if width.get() != Some(bounds.size.width) {
                width.set(Some(bounds.size.width));
                cx.notify(owner);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}
