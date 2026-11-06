//! The virtualised lists a long page sits in, and their scrollbar: GPUI's
//! `uniform_list` (rows of one height, laid out a screenful at a time) and
//! `list` (rows that measure themselves), with the handles that keep their
//! place, and the kōan scrollbar over either. A page building rows of
//! `TableRow`s takes `table_list`; one laying out something else in rows,
//! such as a grid of tiles a row at a time, takes these.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Div, ElementId, EntityId, InteractiveElement, IntoElement, ParentElement, Pixels, Point,
    SharedString, Stateful, Styled, canvas, div, px,
};

pub use gpui::{
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
    H: super::ScrollSource,
{
    super::Scrollbar::vertical(handle)
}

/// What a virtualised list sits in for a screen reader: a list named for
/// what it holds, its rows (`TableRow::spoken`) its items.
pub fn spoken_list(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Stateful<Div> {
    super::accessible(div().id(id), gpui::Role::List, Some(name.into()))
}

/// Scrolls the row past row `ix` of a `uniform_list`'s `count` on screen, the
/// one before it where `ix` is at the top: a screen reader moving through the
/// list reaches only rows drawn, so the next one is drawn before it gets
/// there. One neighbour only, the one the reader is heading for: a second
/// scroll could undo the first. The caller draws the list again
/// (`TableRow::on_reveal` does).
pub fn reveal_around(handle: &UniformListScrollHandle, ix: usize, count: usize) {
    if count == 0 {
        return;
    }
    let top = uniform_top(handle, count);
    handle.scroll_to_item(neighbour(ix, top, count), ScrollStrategy::Nearest);
}

/// The row at the top of a `uniform_list` of `count` rows as last laid out,
/// from its scroll offset and the height of its content. (The handle's
/// `logical_scroll_top` counts children a uniform list never registers, so
/// it is always 0.)
fn uniform_top(handle: &UniformListScrollHandle, count: usize) -> usize {
    let state = handle.0.borrow();
    let Some(size) = state.last_item_size else {
        return 0;
    };
    let row = f32::from(size.contents.height) / count as f32;
    if row <= 0.0 {
        return 0;
    }
    let scrolled = -f32::from(state.base_handle.offset().y);
    ((scrolled / row).floor().max(0.0) as usize).min(count - 1)
}

/// The neighbour of row `ix` a reader moving through a list of `count` is
/// heading for, with `top` the row at the top of the screen: the one before
/// where `ix` is at or above the top, the one after otherwise.
pub(crate) fn neighbour(ix: usize, top: usize, count: usize) -> usize {
    if ix <= top {
        ix.saturating_sub(1)
    } else {
        (ix + 1).min(count.saturating_sub(1))
    }
}

/// `reveal_around` for a `list` of rows that measure themselves.
pub fn reveal_around_list(state: &ListState, ix: usize) {
    let count = state.item_count();
    if count == 0 {
        return;
    }
    let top = state.logical_scroll_top().item_ix;
    state.scroll_to_reveal_item(neighbour(ix, top, count));
}

/// A scroll offset `y` down from the top, as a list's handle takes it.
pub fn offset_down(y: Pixels) -> Point<Pixels> {
    Point { x: px(0.0), y: -y }
}

/// An element filling its parent that keeps the parent's width in `width`,
/// and redraws `owner` when it changes: a page laying out a grid or a list
/// by the width it is given.
/// A page's header over a virtual list that scrolls it away with the rows:
/// drawn at the list's scroll `offset` over the room the list leaves above
/// its first row, which is `height` once measured (`owner` is told).
pub fn scrolls_away(
    header: impl IntoElement,
    offset: Pixels,
    height: Rc<Cell<Option<Pixels>>>,
    owner: EntityId,
) -> impl IntoElement {
    let probe = canvas(
        move |bounds, _, cx| {
            if height.get() != Some(bounds.size.height) {
                height.set(Some(bounds.size.height));
                cx.notify(owner);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full();
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(offset)
        .child(div().relative().child(probe).child(header))
}

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
