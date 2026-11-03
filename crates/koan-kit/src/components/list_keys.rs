//! The keys every list answers to, and where its keyboard cursor is: one
//! behaviour for every list in the app, so the queue, a record's tracks,
//! the history and a playlist move the same way.
//!
//! A list is one stop in the window's Tab order; inside it, ↑ and ↓ move the
//! cursor and pick the row under it, with ⇧ extending the pick, Home and End
//! go to either end, ← → move a tile in a grid (where ↑ ↓ move a row of
//! it), Page Up and Page Down move a screenful, Return plays or
//! opens the row, Ctrl-Space adds it to the pick or takes it out (Space
//! stays play and pause), and the menu key or ⇧F10 opens its context menu. The list
//! sets the `List` key context and handles the actions; `ListCursor` works
//! out where each key leaves the cursor and what it picks. The app binds the
//! keys, from the table its shortcuts sheet lists.

use gpui::actions;

actions!(
    list,
    [
        /// The row above.
        Up,
        /// The row below.
        Down,
        /// The row above, the pick extended to it.
        ExtendUp,
        /// The row below, the pick extended to it.
        ExtendDown,
        /// In a grid, the tile before.
        Left,
        /// In a grid, the tile after.
        Right,
        /// In a grid, the tile before, the pick extended to it.
        ExtendLeft,
        /// In a grid, the tile after, the pick extended to it.
        ExtendRight,
        /// The first row.
        First,
        /// The last row.
        Last,
        /// A screenful up.
        PageUp,
        /// A screenful down.
        PageDown,
        /// Play or open the row under the cursor.
        Activate,
        /// Add the row under the cursor to the pick, or take it out.
        Pick,
        /// The context menu, for the row under the cursor: the menu key, or
        /// ⇧F10.
        ShowMenu,
    ]
);

/// The key context a list sets on the element that holds its focus.
pub const LIST_CONTEXT: &str = "List";

/// Where a list's keyboard cursor is, and where a ⇧ pick extends from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListCursor {
    pub at: Option<usize>,
    pub anchor: Option<usize>,
}

/// What a key did: the row the cursor is on now, where the pick extends
/// from (the anchor, or the row itself), and the rows to pick, in order,
/// inclusive: the one row, or from the anchor to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moved {
    pub to: usize,
    pub from: usize,
    pub pick: (usize, usize),
}

impl ListCursor {
    /// Moves `by` rows through a list of `len`, held at its ends. A cursor
    /// not yet placed starts at the first row going down and the last going
    /// up. `extend` keeps the anchor and picks from it; otherwise the anchor
    /// moves with the cursor and the row alone is picked.
    pub fn step(&mut self, by: isize, len: usize, extend: bool) -> Option<Moved> {
        let last = len.checked_sub(1)?;
        let to = match self.at {
            Some(at) => at.min(last).saturating_add_signed(by).min(last),
            None if by < 0 => last,
            None => 0,
        };
        Some(self.land(to, extend))
    }

    /// To row `to` of a list of `len`: Home, End, or a row chosen otherwise.
    pub fn to(&mut self, to: usize, len: usize, extend: bool) -> Option<Moved> {
        let last = len.checked_sub(1)?;
        Some(self.land(to.min(last), extend))
    }

    fn land(&mut self, to: usize, extend: bool) -> Moved {
        let anchor = match (extend, self.anchor) {
            (true, Some(anchor)) => anchor,
            _ => to,
        };
        self.at = Some(to);
        self.anchor = Some(anchor);
        Moved {
            to,
            from: anchor,
            pick: (anchor.min(to), anchor.max(to)),
        }
    }

    /// A click put the cursor on `at`: the keys carry on from there, and a
    /// ⇧ pick extends from it.
    pub fn place(&mut self, at: usize) {
        self.at = Some(at);
        self.anchor = Some(at);
    }

    /// A click put the cursor on `at`. A ⇧-click extends the pick from the
    /// anchor, which it keeps, so ⇧ and the arrows go on from the same
    /// origin; any other click is the new origin.
    pub fn click(&mut self, at: usize, extend: bool) {
        if !extend || self.anchor.is_none() {
            self.anchor = Some(at);
        }
        self.at = Some(at);
    }

    /// The rows changed under the cursor (a reload, new rows above it): the
    /// cursor and the anchor stay on the rows they were on, found by key
    /// from `old` in `new`; one whose row is gone keeps its place, held
    /// within the new length.
    pub fn relocate<K: PartialEq>(&mut self, old: &[K], new: &[K]) {
        let find = |at: Option<usize>| {
            at.map(|a| {
                old.get(a)
                    .and_then(|key| new.iter().position(|n| n == key))
                    .unwrap_or(a)
            })
        };
        self.at = find(self.at);
        self.anchor = find(self.anchor);
        self.clamp(new.len());
    }

    /// A list changed under the cursor: held within its new length, or
    /// cleared when it is empty.
    pub fn clamp(&mut self, len: usize) {
        match len.checked_sub(1) {
            None => *self = Self::default(),
            Some(last) => {
                self.at = self.at.map(|a| a.min(last));
                self.anchor = self.anchor.map(|a| a.min(last));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shift_click_keeps_the_origin() {
        let mut c = ListCursor::default();
        c.click(5, false);
        c.click(8, true);
        assert_eq!((c.at, c.anchor), (Some(8), Some(5)));
        let m = c.step(-5, 10, true).unwrap();
        assert_eq!(
            (m.from, m.to, m.pick),
            (5, 3, (3, 5)),
            "back past the origin"
        );
        c.click(2, false);
        assert_eq!((c.at, c.anchor), (Some(2), Some(2)));
        let mut fresh = ListCursor::default();
        fresh.click(4, true);
        assert_eq!(fresh.anchor, Some(4), "nothing to extend from");
    }

    #[test]
    fn the_cursor_follows_its_row_through_a_reload() {
        let mut c = ListCursor {
            at: Some(1),
            anchor: Some(0),
        };
        // Two new rows above, and the anchor's row gone.
        c.relocate(&[10, 11, 12], &[20, 21, 11, 12]);
        assert_eq!((c.at, c.anchor), (Some(2), Some(0)));
        // The cursor's row gone and the list shorter: held at its end.
        c.relocate(&[20, 21, 11, 12], &[20]);
        assert_eq!((c.at, c.anchor), (Some(0), Some(0)));
        c.relocate(&[20], &[] as &[i32]);
        assert_eq!(c, ListCursor::default(), "an empty list has no cursor");
    }

    #[test]
    fn the_cursor_steps_and_extends_within_the_list() {
        let mut c = ListCursor::default();
        assert_eq!(c.step(1, 0, false), None, "an empty list has nowhere");
        assert_eq!(c.step(1, 5, false).unwrap().to, 0, "down starts at the top");
        assert_eq!(
            c.step(1, 5, false).unwrap(),
            Moved {
                to: 1,
                from: 1,
                pick: (1, 1)
            }
        );
        assert_eq!(
            c.step(2, 5, true).unwrap(),
            Moved {
                to: 3,
                from: 1,
                pick: (1, 3)
            }
        );
        assert_eq!(
            c.step(-3, 5, true).unwrap(),
            Moved {
                to: 0,
                from: 1,
                pick: (0, 1)
            }
        );
        assert_eq!(c.step(-1, 5, false).unwrap().to, 0, "held at the top");
        assert_eq!(c.to(99, 5, false).unwrap().to, 4, "End is the last row");
        let mut fresh = ListCursor::default();
        assert_eq!(
            fresh.step(-1, 5, false).unwrap().to,
            4,
            "up starts at the bottom"
        );
        fresh.clamp(2);
        assert_eq!(fresh.at, Some(1));
        fresh.clamp(0);
        assert_eq!(fresh, ListCursor::default());
    }
}
