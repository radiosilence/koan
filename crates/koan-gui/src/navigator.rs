//! Where the app is: one page, the pages visited, and a cursor into them.
//!
//! koan navigates like a browser, any page from any page, as the Mac app's
//! `Navigator.swift` does: going somewhere new from the middle of the history
//! drops what was ahead of the cursor.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Queue,
    Albums,
    Artists,
    Tracks,
    Favourites,
    RecentlyPlayed,
    History,
    Downloads,
    Playlist(i64),
}

pub struct Navigator {
    history: Vec<Page>,
    cursor: usize,
}

impl Default for Navigator {
    fn default() -> Self {
        Self {
            history: vec![Page::Queue],
            cursor: 0,
        }
    }
}

impl Navigator {
    pub fn current(&self) -> Page {
        self.history[self.cursor]
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }

    /// Whether the page changed.
    pub fn go(&mut self, page: Page) -> bool {
        if page == self.current() {
            return false;
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(page);
        self.cursor = self.history.len() - 1;
        true
    }

    pub fn back(&mut self) -> bool {
        let moved = self.can_go_back();
        if moved {
            self.cursor -= 1;
        }
        moved
    }

    pub fn forward(&mut self) -> bool {
        let moved = self.can_go_forward();
        if moved {
            self.cursor += 1;
        }
        moved
    }

    /// Drops a page that no longer exists (a deleted playlist) from the
    /// history, keeping the cursor on the page it was on where that survives.
    pub fn forget(&mut self, page: Page) {
        if !self.history.contains(&page) {
            return;
        }
        let surviving = self.history[..self.cursor]
            .iter()
            .filter(|p| **p != page)
            .count();
        self.history.retain(|p| *p != page);
        if self.history.is_empty() {
            self.history.push(Page::Queue);
        }
        self.cursor = surviving.min(self.history.len() - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUEUE: Page = Page::Queue;
    const ALBUMS: Page = Page::Albums;
    const TRACKS: Page = Page::Tracks;

    #[test]
    fn going_somewhere_new_drops_what_was_ahead() {
        let mut nav = Navigator::default();
        nav.go(ALBUMS);
        nav.go(TRACKS);
        assert!(nav.back());
        nav.go(Page::Artists);
        assert_eq!(nav.current(), Page::Artists);
        assert!(!nav.can_go_forward());
        assert!(nav.back());
        assert_eq!(nav.current(), ALBUMS);
    }

    #[test]
    fn the_same_page_is_not_a_visit() {
        let mut nav = Navigator::default();
        assert!(!nav.go(QUEUE));
        assert!(!nav.can_go_back());
    }

    #[test]
    fn forgetting_a_page_keeps_the_cursor_where_it_was() {
        let mut nav = Navigator::default();
        let playlist = Page::Playlist(7);
        nav.go(playlist);
        nav.go(ALBUMS);
        nav.go(playlist);
        nav.go(TRACKS);
        nav.forget(playlist);
        assert_eq!(nav.current(), TRACKS);
        assert!(nav.back());
        assert_eq!(nav.current(), ALBUMS);
        assert!(nav.back());
        assert_eq!(nav.current(), QUEUE);
    }

    #[test]
    fn forgetting_the_page_on_screen_lands_on_its_neighbour() {
        let mut nav = Navigator::default();
        let playlist = Page::Playlist(7);
        nav.go(ALBUMS);
        nav.go(playlist);
        nav.forget(playlist);
        assert_eq!(nav.current(), ALBUMS);
    }
}
