//! Where the TUI is: one page, the linear history of pages visited, and a
//! cursor into it. Browser-like, as the apps' `Navigator` is: any page from
//! any page, back and forward between them.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Queue,
    NowPlaying,
    Albums,
    Artists,
    /// Every track in the library, as a list.
    Tracks,
    Favourites,
    Playlists,
    Recent,
    /// What is on this device: local files and finished downloads.
    Downloaded,
    History,
    /// Transfers from the server, running and waiting.
    Downloads,
    Settings,
    Search,
    /// One playlist, by its id.
    Playlist(i64),
    /// One record, by its id: reached from a tile, a link, or the queue.
    Album(i64),
    /// One artist's records, by id.
    Artist(i64),
    /// What the output plays: its correction, EQs and preset.
    Eq,
}

impl Page {
    /// Every page that is a place of its own, in the sidebar's order.
    pub const ALL: [Page; 13] = [
        Page::Queue,
        Page::NowPlaying,
        Page::Albums,
        Page::Artists,
        Page::Tracks,
        Page::Favourites,
        Page::Recent,
        Page::Downloaded,
        Page::History,
        Page::Downloads,
        Page::Playlists,
        Page::Search,
        Page::Settings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Queue => "queue",
            Page::NowPlaying => "now playing",
            Page::Albums => "albums",
            Page::Artists => "artists",
            Page::Tracks => "tracks",
            Page::Favourites => "favourites",
            Page::Playlists => "playlists",
            Page::Recent => "recently played",
            Page::Downloaded => "downloaded",
            Page::History => "history",
            Page::Downloads => "downloads",
            Page::Settings => "settings",
            Page::Search => "search",
            Page::Playlist(_) => "playlist",
            Page::Album(_) => "album",
            Page::Artist(_) => "artist",
            Page::Eq => "eq",
        }
    }

    /// The page in the sidebar or the tab bar this one sits under.
    pub fn tab(self) -> Page {
        match self {
            Page::Album(_) => Page::Albums,
            Page::Artist(_) => Page::Artists,
            Page::Playlist(_) => Page::Playlists,
            Page::Eq => Page::Settings,
            other => other,
        }
    }

    /// The label a narrow tab bar has room for.
    pub fn short(self) -> &'static str {
        match self {
            Page::Queue => "queue",
            Page::NowPlaying => "playing",
            Page::Albums => "albums",
            Page::Artists => "artists",
            Page::Tracks => "tracks",
            Page::Favourites => "faves",
            Page::Playlists => "lists",
            Page::Recent => "recent",
            Page::Downloaded => "on device",
            Page::History => "history",
            Page::Downloads => "transfers",
            Page::Settings => "settings",
            Page::Search => "search",
            Page::Playlist(_) => "playlist",
            Page::Album(_) => "album",
            Page::Artist(_) => "artist",
            Page::Eq => "eq",
        }
    }

    /// The digit that opens it, for the pages that have one.
    pub fn key(self) -> Option<char> {
        Some(match self {
            Page::Queue => '1',
            Page::NowPlaying => '2',
            Page::Albums => '3',
            Page::Artists => '4',
            Page::Favourites => '5',
            Page::Playlists => '6',
            Page::Recent => '7',
            Page::History => '8',
            Page::Settings => '9',
            Page::Search => '0',
            Page::Tracks
            | Page::Downloaded
            | Page::Downloads
            | Page::Album(_)
            | Page::Artist(_)
            | Page::Playlist(_)
            | Page::Eq => return None,
        })
    }

    pub fn from_key(c: char) -> Option<Page> {
        Self::ALL.into_iter().find(|p| p.key() == Some(c))
    }
}

/// The widest history kept. Nobody goes back further than this.
const HISTORY: usize = 64;

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

    /// Opens `page`. The pages ahead of the cursor are dropped, as a browser
    /// drops them; opening the page already open does nothing.
    pub fn go(&mut self, page: Page) {
        if self.current() == page {
            return;
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(page);
        if self.history.len() > HISTORY {
            self.history.remove(0);
        }
        self.cursor = self.history.len() - 1;
    }

    pub fn can_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_forward(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }

    pub fn back(&mut self) -> bool {
        let can = self.can_back();
        if can {
            self.cursor -= 1;
        }
        can
    }

    pub fn forward(&mut self) -> bool {
        let can = self.can_forward();
        if can {
            self.cursor += 1;
        }
        can
    }

    /// The page after this one in the sidebar's order, wrapping.
    pub fn next_page(&self) -> Page {
        let i = Page::ALL
            .iter()
            .position(|p| *p == self.current().tab())
            .unwrap_or(0);
        Page::ALL[(i + 1) % Page::ALL.len()]
    }

    pub fn prev_page(&self) -> Page {
        let i = Page::ALL
            .iter()
            .position(|p| *p == self.current().tab())
            .unwrap_or(0);
        Page::ALL[(i + Page::ALL.len() - 1) % Page::ALL.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_forward_walk_the_history() {
        let mut nav = Navigator::default();
        nav.go(Page::Albums);
        nav.go(Page::Artists);
        assert!(nav.back());
        assert_eq!(nav.current(), Page::Albums);
        assert!(nav.forward());
        assert_eq!(nav.current(), Page::Artists);
        assert!(!nav.forward());
    }

    #[test]
    fn going_somewhere_new_drops_what_was_ahead() {
        let mut nav = Navigator::default();
        nav.go(Page::Albums);
        nav.go(Page::Artists);
        nav.back();
        nav.go(Page::Recent);
        assert!(!nav.can_forward());
        nav.back();
        assert_eq!(nav.current(), Page::Albums);
    }

    #[test]
    fn opening_the_open_page_adds_nothing() {
        let mut nav = Navigator::default();
        nav.go(Page::Queue);
        assert!(!nav.can_back());
    }

    #[test]
    fn the_pages_wrap() {
        let mut nav = Navigator::default();
        nav.go(Page::Settings);
        assert_eq!(nav.next_page(), Page::Queue);
        nav.go(Page::Queue);
        assert_eq!(nav.prev_page(), Page::Settings);
    }

    #[test]
    fn a_record_sits_under_albums_and_an_artist_under_artists() {
        assert_eq!(Page::Album(7).tab(), Page::Albums);
        assert_eq!(Page::Artist(7).tab(), Page::Artists);
        assert_eq!(Page::Queue.tab(), Page::Queue);
        let mut nav = Navigator::default();
        nav.go(Page::Album(1));
        nav.go(Page::Album(2));
        assert!(nav.can_back());
        assert_eq!(nav.next_page(), Page::Artists);
    }

    #[test]
    fn every_page_has_its_key() {
        for page in Page::ALL {
            if let Some(c) = page.key() {
                assert_eq!(Page::from_key(c), Some(page));
            }
        }
        // Ten digits, ten pages.
        assert_eq!(Page::ALL.iter().filter(|p| p.key().is_some()).count(), 10);
    }
}
