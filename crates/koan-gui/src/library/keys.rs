//! A library list's keys, kit-wide (`components::list_keys`): ↑ ↓ move and
//! pick, ⇧ extends, ← → move a tile in a grid, Home and End, Page Up and
//! Page Down, Return plays or opens, Ctrl-Space adds or takes away. Each list
//! says how long it is, how a range or one row is picked, how a row is
//! brought into view and what Return does; the rest is the same on every
//! page.

use koan_kit::app::*;

use crate::components::{ListCursor, Moved, list_actions as keys};

/// How many rows Page Up and Page Down move, where a list does not know its
/// screenful: about one at the rows' height in a window of ordinary size.
pub const PAGE: usize = 12;

pub trait KeyedList: Sized + 'static {
    fn cursor(&mut self) -> &mut ListCursor;
    /// How many rows the keys walk.
    fn len(&self) -> usize;
    /// Picks rows `from..=to`, and those alone.
    fn pick(&mut self, from: usize, to: usize, cx: &mut Context<Self>);
    /// Adds row `ix` to the pick, or takes it out.
    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>);
    /// Brings row `ix` into view.
    fn reveal(&mut self, ix: usize, cx: &mut Context<Self>);
    /// Return on row `ix`: play from it, or open it.
    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>);
    /// How many items a row holds: a grid's columns, where ↑ ↓ move a row
    /// and ← → an item. A list is one.
    fn columns(&self) -> usize {
        1
    }
    /// How many rows Page Up and Page Down move.
    fn page(&self) -> usize {
        PAGE
    }
    /// The menu key: the page's context menu will act on item `ix`, and on
    /// nothing when the cursor is not placed.
    fn menu_at(&mut self, _ix: Option<usize>, _cx: &mut Context<Self>) {}
    /// The item `rows` rows below item `at` (above, negative): the same
    /// column of a grid, held at the ends. A page whose rows are not all the
    /// same length (an artist's sections) says where each one starts.
    fn below(&self, at: usize, rows: isize) -> usize {
        at.saturating_add_signed(rows * self.columns().max(1) as isize)
    }
}

fn moved<V: KeyedList>(this: &mut V, m: Option<Moved>, cx: &mut Context<V>) {
    if let Some(m) = m {
        this.pick(m.pick.0, m.pick.1, cx);
        this.reveal(m.to, cx);
        cx.notify();
    }
}

fn step<V: KeyedList>(this: &mut V, by: isize, extend: bool, cx: &mut Context<V>) {
    let len = this.len();
    let m = this.cursor().step(by, len, extend);
    moved(this, m, cx);
}

/// ↑ ↓ and the page keys: `rows` rows of a grid, items of a list. A cursor
/// not yet placed starts at the top going down and the bottom going up.
fn down<V: KeyedList>(this: &mut V, rows: isize, extend: bool, cx: &mut Context<V>) {
    let last = this.len().saturating_sub(1);
    let by = match this.cursor().at {
        Some(at) => {
            let at = at.min(last);
            this.below(at, rows).min(last) as isize - at as isize
        }
        None => rows.signum(),
    };
    step(this, by, extend, cx);
}

/// ← →: an item in a grid; nothing in a list.
fn across<V: KeyedList>(this: &mut V, by: isize, extend: bool, cx: &mut Context<V>) {
    if this.columns() > 1 {
        step(this, by, extend, cx);
    }
}

fn jump<V: KeyedList>(this: &mut V, to: usize, cx: &mut Context<V>) {
    let len = this.len();
    let m = this.cursor().to(to, len, false);
    moved(this, m, cx);
}

/// The list keys on `element`, which holds the list's focus and sets the
/// `List` key context (`LIST_CONTEXT`, beside its own: "Library List").
pub fn keyed<V: KeyedList, E: InteractiveElement>(element: E, cx: &mut Context<V>) -> E {
    element
        .on_action(cx.listener(|this: &mut V, _: &keys::Up, _, cx| down(this, -1, false, cx)))
        .on_action(cx.listener(|this: &mut V, _: &keys::Down, _, cx| down(this, 1, false, cx)))
        .on_action(cx.listener(|this: &mut V, _: &keys::ExtendUp, _, cx| down(this, -1, true, cx)))
        .on_action(cx.listener(|this: &mut V, _: &keys::ExtendDown, _, cx| down(this, 1, true, cx)))
        .on_action(cx.listener(|this: &mut V, _: &keys::Left, _, cx| across(this, -1, false, cx)))
        .on_action(cx.listener(|this: &mut V, _: &keys::Right, _, cx| across(this, 1, false, cx)))
        .on_action(
            cx.listener(|this: &mut V, _: &keys::ExtendLeft, _, cx| across(this, -1, true, cx)),
        )
        .on_action(
            cx.listener(|this: &mut V, _: &keys::ExtendRight, _, cx| across(this, 1, true, cx)),
        )
        .on_action(cx.listener(|this: &mut V, _: &keys::First, _, cx| jump(this, 0, cx)))
        .on_action(cx.listener(|this: &mut V, _: &keys::Last, _, cx| jump(this, usize::MAX, cx)))
        .on_action(cx.listener(|this: &mut V, _: &keys::PageUp, _, cx| {
            let page = this.page() as isize;
            down(this, -page, false, cx)
        }))
        .on_action(cx.listener(|this: &mut V, _: &keys::PageDown, _, cx| {
            let page = this.page() as isize;
            down(this, page, false, cx)
        }))
        .on_action(cx.listener(|this: &mut V, _: &keys::Activate, window, cx| {
            if let Some(at) = this.cursor().at.filter(|at| *at < this.len()) {
                this.activate(at, window, cx);
            }
        }))
        // Before the context menu, on the same element, opens over it.
        .capture_action(cx.listener(|this: &mut V, _: &keys::ShowMenu, _, cx| {
            let at = this.cursor().at.filter(|at| *at < this.len());
            this.menu_at(at, cx);
        }))
        .on_action(cx.listener(|this: &mut V, _: &keys::Pick, _, cx| {
            if let Some(at) = this.cursor().at.filter(|at| *at < this.len()) {
                this.toggle(at, cx);
                cx.notify();
            }
        }))
}
