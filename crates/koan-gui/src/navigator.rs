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
    /// The search's results, for the query the sidebar's field holds.
    Search,
    Albums,
    Artists,
    Tracks,
    Favourites,
    RecentlyPlayed,
    /// What can play on this device: the downloaded shelf.
    OnDevice,
    History,
    Downloads,
    Playlist(i64),
    Album(i64),
    Artist(i64),
    /// A shelf's "see all": the album, artist or track browser with the
    /// shelf as its own filter, leaving the library browser's as it was.
    Shelved(Shelf, Listing),
    /// The phone's Library tab: the library's sections.
    Library,
    /// The phone's list of playlists, a section of its Library tab.
    Playlists,
    /// The phone's Search tab: the field, and the results under it.
    SearchTab,
    /// The phone's Settings tab: its sections.
    Settings,
    /// A section of Settings, as a phone's screen.
    SettingsSection(crate::settings::Tab),
}

/// A shelf, as a route names it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Shelf {
    Favourites,
    Recent,
    Downloaded,
    /// The sidebar's search, for its query as it stands.
    Search,
}

/// Which of the library's listings a shelf's "see all" opens.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Listing {
    Albums,
    Artists,
    Tracks,
}

impl Route {
    /// A route by name, for tools that open the app somewhere: `albums`,
    /// `favourites`, `recent`, `downloaded`, `album:<id>`, `artist:<id>`,
    /// `playlist:<id>`.
    pub fn named(name: &str) -> Option<Route> {
        let id = |s: &str| s.parse::<i64>().ok();
        match name.split_once(':') {
            Some(("album", n)) => id(n).map(Route::Album),
            Some(("artist", n)) => id(n).map(Route::Artist),
            Some(("playlist", n)) => id(n).map(Route::Playlist),
            _ => match name {
                "queue" => Some(Route::Queue),
                "albums" => Some(Route::Albums),
                "artists" => Some(Route::Artists),
                "tracks" => Some(Route::Tracks),
                "favourites" => Some(Route::Favourites),
                "recent" => Some(Route::RecentlyPlayed),
                "downloaded" => Some(Route::OnDevice),
                _ => None,
            },
        }
    }
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
    /// On a phone, the tab showing, and every other tab's history and cursor
    /// as it was left, each tab a stack of its own as on iOS. The desktop has
    /// one tab.
    tab: usize,
    stashed: HashMap<usize, (Vec<Route>, usize)>,
}

impl Default for Navigator {
    fn default() -> Self {
        Self {
            history: vec![Route::Queue],
            cursor: 0,
            rewinds: HashMap::new(),
            tab: 0,
            stashed: HashMap::new(),
        }
    }
}

impl Navigator {
    pub fn current(&self) -> Route {
        self.history[self.cursor]
    }

    /// The route a step back would show: what a phone's swipe back draws
    /// under the page as it goes.
    pub fn behind(&self) -> Option<Route> {
        self.cursor.checked_sub(1).map(|ix| self.history[ix])
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

    pub fn tab(&self) -> usize {
        self.tab
    }

    /// Whether the page showing is its tab's first, which a phone's back
    /// gesture cannot leave.
    /// How far into its tab's history the page showing is: 0 at the root.
    pub fn history_ix(&self) -> usize {
        self.cursor
    }

    pub fn at_root(&self) -> bool {
        self.cursor == 0
    }

    /// A phone's tab chosen: its stack as it was left, or `root` alone the
    /// first time. The tab showing, chosen again, goes back to its root, as
    /// a tab bar's own tab does, and its root, chosen at the root, rewinds.
    pub fn switch(&mut self, tab: usize, root: Route) -> Chosen {
        if tab == self.tab {
            if self.cursor == 0 {
                *self.rewinds.entry(self.history[0]).or_default() += 1;
            } else {
                self.history.truncate(1);
                self.cursor = 0;
            }
            return Chosen::Rewound;
        }
        let (history, cursor) = self.stashed.remove(&tab).unwrap_or_else(|| (vec![root], 0));
        let left = (std::mem::replace(&mut self.history, history), self.cursor);
        self.stashed.insert(self.tab, left);
        self.cursor = cursor;
        self.tab = tab;
        Chosen::Moved
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
    /// its order, so the cursor stays on the page it was on, or, when that
    /// page was the one forgotten, lands where Back would have taken it. Two
    /// visits of one page left side by side become one.
    /// Rewind counts are kept: they only ever grow, so a route that comes
    /// back (a row id reused) never repeats a count a page has seen.
    pub fn forget(&mut self, route: Route) {
        for (history, cursor) in self.stashed.values_mut() {
            Self::forget_here(history, cursor, route);
        }
        Self::forget_here(&mut self.history, &mut self.cursor, route);
    }

    fn forget_here(history: &mut Vec<Route>, cursor: &mut usize, route: Route) {
        let mut nav = Navigator {
            history: std::mem::take(history),
            cursor: *cursor,
            ..Navigator::default()
        };
        nav.forget_showing(route);
        *history = nav.history;
        *cursor = nav.cursor;
    }

    fn forget_showing(&mut self, route: Route) {
        if !self.history.contains(&route) {
            return;
        }
        let showing = self.current() == route;
        let surviving = self.history[..self.cursor]
            .iter()
            .filter(|r| **r != route)
            .count();
        self.history.retain(|r| *r != route);
        if self.history.is_empty() {
            self.history.push(Route::Queue);
        }
        let landing = if showing {
            surviving.saturating_sub(1)
        } else {
            surviving
        }
        .min(self.history.len() - 1);
        // The pages either side of a forgotten one can be the same page, and
        // the same page twice in a row is not a step Back can take.
        let mut collapsed: Vec<Route> = Vec::with_capacity(self.history.len());
        let mut cursor = 0;
        for (ix, route) in self.history.iter().enumerate() {
            if collapsed.last() != Some(route) {
                collapsed.push(*route);
            }
            if ix == landing {
                cursor = collapsed.len() - 1;
            }
        }
        self.history = collapsed;
        self.cursor = cursor;
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
    fn forgetting_the_page_on_screen_mid_history_lands_where_back_would() {
        let mut nav = Navigator::default();
        nav.go(ALBUMS);
        nav.go(PLAYLIST);
        nav.go(TRACKS);
        assert!(nav.back());
        nav.forget(PLAYLIST);
        assert_eq!(nav.current(), ALBUMS);
        assert!(nav.forward());
        assert_eq!(nav.current(), TRACKS);
    }

    #[test]
    fn forgetting_a_page_between_two_visits_of_one_page_joins_them() {
        let mut nav = Navigator::default();
        nav.go(ALBUMS);
        nav.go(PLAYLIST);
        nav.go(ALBUMS);
        nav.forget(PLAYLIST);
        assert_eq!(nav.current(), ALBUMS);
        assert!(nav.back());
        assert_eq!(nav.current(), QUEUE);
        assert!(!nav.back());
    }

    #[test]
    fn a_forgotten_route_keeps_its_rewinds() {
        let mut nav = Navigator::default();
        nav.go(PLAYLIST);
        nav.choose(PLAYLIST);
        nav.forget(PLAYLIST);
        nav.go(PLAYLIST);
        nav.choose(PLAYLIST);
        assert_eq!(nav.rewinds(PLAYLIST), 2);
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
            ..Navigator::default()
        };
        nav.forget(PLAYLIST);
        assert_eq!(nav.current(), QUEUE);
    }

    #[test]
    fn each_tab_keeps_its_own_stack() {
        let mut nav = Navigator::default();
        nav.go(ALBUMS);
        assert_eq!(nav.switch(1, Route::Library), Chosen::Moved);
        assert_eq!(nav.current(), Route::Library);
        assert!(!nav.can_go_back());
        nav.go(PLAYLIST);
        nav.switch(0, QUEUE);
        assert_eq!(nav.current(), ALBUMS);
        assert!(nav.back());
        assert_eq!(nav.current(), QUEUE);
        nav.switch(1, Route::Library);
        assert_eq!(nav.current(), PLAYLIST);
    }

    #[test]
    fn the_tab_showing_chosen_again_goes_to_its_root_then_rewinds() {
        let mut nav = Navigator::default();
        nav.go(ALBUMS);
        nav.go(TRACKS);
        assert_eq!(nav.switch(0, QUEUE), Chosen::Rewound);
        assert_eq!(nav.current(), QUEUE);
        assert!(!nav.can_go_forward());
        assert_eq!(nav.rewinds(QUEUE), 0);
        nav.switch(0, QUEUE);
        assert_eq!(nav.rewinds(QUEUE), 1);
    }

    #[test]
    fn forgetting_a_page_reaches_the_tabs_not_showing() {
        let mut nav = Navigator::default();
        nav.go(PLAYLIST);
        nav.switch(1, Route::Library);
        nav.forget(PLAYLIST);
        nav.switch(0, QUEUE);
        assert_eq!(nav.current(), QUEUE);
        assert!(!nav.can_go_forward());
    }
}
