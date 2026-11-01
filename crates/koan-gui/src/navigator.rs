//! Where the app is: a route, the routes visited, and a cursor into them.
//!
//! koan navigates like a browser, any page from any page: an album is reached
//! from the queue, the grid, an artist or search, and none of them is its
//! parent, so there is no tree to walk and nothing to be "up". Going
//! somewhere new from the middle of the history drops what was ahead.
//!
//! A route is data, ids and never entities, so a route can arrive from
//! anywhere (a sidebar row, a link from another device, a menu) and pages
//! never know how they were reached. Every rule about moving lives here, with
//! its tests; views observe the navigator and draw where it says.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Route {
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

/// What choosing a route did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chosen {
    /// To another page.
    Moved,
    /// The page already showing, chosen again: back to its top, as a
    /// browser tab's own link reloads it. Nothing is recorded.
    Rewound,
}

pub struct Navigator {
    history: Vec<Route>,
    cursor: usize,
    /// How many times each route has been sent back to its top. A page that
    /// keeps its place watches its own count and goes to its top when it
    /// moves.
    rewinds: HashMap<Route, u32>,
}

impl Default for Navigator {
    fn default() -> Self {
        Self {
            history: vec![Route::Queue],
            cursor: 0,
            rewinds: HashMap::new(),
        }
    }
}

impl Navigator {
    pub fn current(&self) -> Route {
        self.history[self.cursor]
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }

    pub fn rewinds(&self, route: Route) -> u32 {
        self.rewinds.get(&route).copied().unwrap_or(0)
    }

    /// A move from anywhere: a link, a menu, a page's own controls. The
    /// route already showing is not a move.
    pub fn go(&mut self, route: Route) -> bool {
        if route == self.current() {
            return false;
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(route);
        self.cursor = self.history.len() - 1;
        true
    }

    /// A route chosen where it is listed (the sidebar): a move, or for the
    /// page already showing, back to its top.
    pub fn choose(&mut self, route: Route) -> Chosen {
        if self.go(route) {
            Chosen::Moved
        } else {
            *self.rewinds.entry(route).or_default() += 1;
            Chosen::Rewound
        }
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

    /// Drops a route that no longer exists (a deleted playlist): it is not
    /// somewhere Back can return to. What survives behind the cursor keeps
    /// its order, so the cursor stays on the page it was on, or lands on the
    /// one before it when that page was the one forgotten.
    pub fn forget(&mut self, route: Route) {
        if !self.history.contains(&route) {
            return;
        }
        let surviving = self.history[..self.cursor]
            .iter()
            .filter(|r| **r != route)
            .count();
        self.history.retain(|r| *r != route);
        if self.history.is_empty() {
            self.history.push(Route::Queue);
        }
        self.cursor = surviving.min(self.history.len() - 1);
        self.rewinds.remove(&route);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUEUE: Route = Route::Queue;
    const ALBUMS: Route = Route::Albums;
    const TRACKS: Route = Route::Tracks;
    const PLAYLIST: Route = Route::Playlist(7);

    #[test]
    fn going_somewhere_new_drops_what_was_ahead() {
        let mut nav = Navigator::default();
        nav.go(ALBUMS);
        nav.go(TRACKS);
        assert!(nav.back());
        nav.go(Route::Artists);
        assert_eq!(nav.current(), Route::Artists);
        assert!(!nav.can_go_forward());
        assert!(nav.back());
        assert_eq!(nav.current(), ALBUMS);
    }

    #[test]
    fn back_and_forward_walk_the_history_and_stop_at_its_ends() {
        let mut nav = Navigator::default();
        nav.go(ALBUMS);
        nav.go(TRACKS);
        assert!(nav.back());
        assert!(nav.back());
        assert_eq!(nav.current(), QUEUE);
        assert!(!nav.back());
        assert!(nav.forward());
        assert!(nav.forward());
        assert_eq!(nav.current(), TRACKS);
        assert!(!nav.forward());
    }

    #[test]
    fn the_page_showing_is_not_a_visit() {
        let mut nav = Navigator::default();
        assert!(!nav.go(QUEUE));
        assert!(!nav.can_go_back());
    }

    #[test]
    fn choosing_the_page_showing_rewinds_it_and_records_nothing() {
        let mut nav = Navigator::default();
        nav.go(ALBUMS);
        assert_eq!(nav.choose(ALBUMS), Chosen::Rewound);
        assert_eq!(nav.choose(ALBUMS), Chosen::Rewound);
        assert_eq!(nav.rewinds(ALBUMS), 2);
        assert_eq!(nav.rewinds(QUEUE), 0);
        assert!(nav.back());
        assert_eq!(nav.current(), QUEUE);
        assert!(!nav.back());
    }

    #[test]
    fn choosing_another_page_moves_without_rewinding_it() {
        let mut nav = Navigator::default();
        assert_eq!(nav.choose(ALBUMS), Chosen::Moved);
        assert_eq!(nav.choose(QUEUE), Chosen::Moved);
        assert_eq!(nav.rewinds(QUEUE), 0);
    }

    #[test]
    fn forgetting_a_page_keeps_the_cursor_where_it_was() {
        let mut nav = Navigator::default();
        nav.go(PLAYLIST);
        nav.go(ALBUMS);
        nav.go(PLAYLIST);
        nav.go(TRACKS);
        nav.forget(PLAYLIST);
        assert_eq!(nav.current(), TRACKS);
        assert!(nav.back());
        assert_eq!(nav.current(), ALBUMS);
        assert!(nav.back());
        assert_eq!(nav.current(), QUEUE);
    }

    #[test]
    fn forgetting_the_page_on_screen_lands_on_the_one_before() {
        let mut nav = Navigator::default();
        nav.go(ALBUMS);
        nav.go(PLAYLIST);
        nav.forget(PLAYLIST);
        assert_eq!(nav.current(), ALBUMS);
        assert!(!nav.can_go_forward());
    }

    #[test]
    fn forgetting_every_page_leaves_the_queue() {
        let mut nav = Navigator {
            history: vec![PLAYLIST],
            cursor: 0,
            rewinds: HashMap::new(),
        };
        nav.forget(PLAYLIST);
        assert_eq!(nav.current(), QUEUE);
    }
}
