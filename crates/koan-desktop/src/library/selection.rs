//! Things picked out of a page to play or queue together.
//!
//! A mode rather than a click: a click on a tile already plays the record and
//! the title opens it. While selecting, a click ticks instead, ⇧ for a range
//! and ⌘A for everything the page shows; playing or queueing the pick puts
//! the page back to normal.
//!
//! Held in the order things were ticked, not the page's order. A pick can
//! span several filters (narrow, tick, narrow again, tick more), and the page
//! has no order for something it no longer shows. What is taken together, a
//! ⇧ range or ⌘A, is taken in the page's order.

use std::collections::HashSet;

use super::drag::Playable;

#[derive(Default)]
pub struct Selection {
    active: bool,
    picked: Vec<Playable>,
    members: HashSet<Playable>,
    /// Where a ⇧-click range starts: the last item clicked without ⇧.
    anchor: Option<Playable>,
}

impl Selection {
    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn picked(&self) -> &[Playable] {
        &self.picked
    }

    pub fn contains(&self, item: Playable) -> bool {
        self.members.contains(&item)
    }

    pub fn begin(&mut self, with: Option<Playable>) {
        self.active = true;
        if let Some(item) = with {
            self.toggle(item);
        }
    }

    pub fn end(&mut self) {
        *self = Self::default();
    }

    /// A click on an item while selecting: ⇧ extends from the last one
    /// clicked, anything else flips just this one. `page` is what the page
    /// shows, in its order.
    pub fn click(&mut self, item: Playable, shift: bool, page: &[Playable]) {
        match self.anchor {
            Some(anchor) if shift => self.extend(anchor, item, page),
            _ => self.toggle(item),
        }
    }

    /// A click on something that does something else outside the mode (goes
    /// somewhere, plays). Whether the pick took it: a tick while selecting,
    /// or a new selection with ⌘ held.
    pub fn take(&mut self, item: Playable, shift: bool, command: bool, page: &[Playable]) -> bool {
        if self.active {
            self.click(item, shift, page);
            true
        } else if command {
            self.begin(Some(item));
            true
        } else {
            false
        }
    }

    /// Everything on the page, in its order, after whatever was picked under
    /// another filter.
    pub fn select_all(&mut self, page: &[Playable]) {
        self.active = true;
        self.take_together(page);
    }

    /// The page changed under the pick (a library change): ticks on things
    /// the library no longer has go. Ticks made under another filter stay,
    /// since `exists` answers for the library, not the page.
    pub fn retain(&mut self, exists: impl Fn(Playable) -> bool) {
        self.picked.retain(|p| exists(*p));
        self.members.retain(|p| exists(*p));
        if self.anchor.is_some_and(|a| !exists(a)) {
            self.anchor = None;
        }
    }

    /// What a drag of `grabbed` carries: the whole pick, in tick order, when
    /// it is part of it; otherwise itself.
    pub fn dragging(&self, grabbed: Playable) -> Vec<Playable> {
        if self.contains(grabbed) {
            self.picked.clone()
        } else {
            vec![grabbed]
        }
    }

    fn take_together(&mut self, items: &[Playable]) {
        let keys: HashSet<Playable> = items.iter().copied().collect();
        self.picked.retain(|p| !keys.contains(p));
        self.members.retain(|p| !keys.contains(p));
        for item in items {
            if self.members.insert(*item) {
                self.picked.push(*item);
            }
        }
    }

    fn toggle(&mut self, item: Playable) {
        self.anchor = Some(item);
        if self.members.remove(&item) {
            self.picked.retain(|p| *p != item);
        } else {
            self.members.insert(item);
            self.picked.push(item);
        }
    }

    fn extend(&mut self, anchor: Playable, item: Playable, page: &[Playable]) {
        let (Some(a), Some(b)) = (
            page.iter().position(|p| *p == anchor),
            page.iter().position(|p| *p == item),
        ) else {
            return self.toggle(item);
        };
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        self.take_together(&page[lo..=hi]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Playable::Album as A;

    #[test]
    fn ticks_keep_their_order_and_a_range_takes_page_order() {
        let page = [A(1), A(2), A(3), A(4), A(5)];
        let mut s = Selection::default();
        s.begin(Some(A(4)));
        s.click(A(1), false, &page);
        s.click(A(3), true, &page);
        assert_eq!(s.picked(), &[A(4), A(1), A(2), A(3)]);
        s.click(A(4), false, &page);
        assert_eq!(s.picked(), &[A(1), A(2), A(3)]);
    }

    #[test]
    fn select_all_keeps_ticks_from_another_filter() {
        let mut s = Selection::default();
        s.begin(Some(A(9)));
        s.select_all(&[A(1), A(2)]);
        assert_eq!(s.picked(), &[A(9), A(1), A(2)]);
        assert_eq!(s.dragging(A(1)), vec![A(9), A(1), A(2)]);
        assert_eq!(s.dragging(A(7)), vec![A(7)]);
    }

    #[test]
    fn retain_drops_what_the_library_lost() {
        let mut s = Selection::default();
        s.begin(Some(A(1)));
        s.click(A(2), false, &[]);
        s.retain(|p| p != A(2));
        assert_eq!(s.picked(), &[A(1)]);
        assert!(!s.contains(A(2)));
    }
}
