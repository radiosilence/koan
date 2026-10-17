//! Where the TUI is: one page, the linear history of pages visited, and a
//! cursor into it. Browser-like, as the apps' `Navigator` is: any page from
//! any page, back and forward between them.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Queue,
    NowPlaying,
    Albums,
    Artists,
    Favourites,
    Playlists,
    Recent,
    History,
    Settings,
    Search,
    /// The library's tracks.
    Tracks,
    /// What can play on this device.
    Downloaded,
    /// One playlist, by its id.
    Playlist(i64),
    /// One record, by its id: reached from a tile, a link, or the queue.
    Album(i64),
    /// One artist's records, by id.
    Artist(i64),
}

impl Page {
    pub const ALL: [Page; 10] = [
        Page::Queue,
        Page::NowPlaying,
        Page::Albums,
        Page::Artists,
        Page::Favourites,
        Page::Playlists,
        Page::Recent,
        Page::History,
        Page::Settings,
        Page::Search,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Queue => "queue",
            Page::NowPlaying => "now playing",
            Page::Albums => "albums",
            Page::Artists => "artists",
            Page::Favourites => "favourites",
            Page::Playlists => "playlists",
            Page::Recent => "recent",
            Page::History => "history",
            Page::Settings => "settings",
            Page::Search => "search",
            Page::Tracks => "tracks",
            Page::Downloaded => "downloaded",
            Page::Playlist(_) => "playlist",
            Page::Album(_) => "album",
            Page::Artist(_) => "artist",
        }
    }

    /// The page in the sidebar or the tab bar this one sits under.
    pub fn tab(self) -> Page {
        match self {
            Page::Album(_) => Page::Albums,
            Page::Artist(_) => Page::Artists,
            Page::Playlist(_) => Page::Playlists,
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
            Page::Favourites => "faves",
            Page::Playlists => "lists",
            Page::Recent => "recent",
            Page::History => "history",
            Page::Settings => "settings",
            Page::Search => "search",
            Page::Tracks => "tracks",
            Page::Downloaded => "offline",
            Page::Playlist(_) => "playlist",
            Page::Album(_) => "album",
            Page::Artist(_) => "artist",
        }
    }

    /// The key that opens it.
    pub fn key(self) -> char {
        match self {
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
            | Page::Album(_)
            | Page::Artist(_)
            | Page::Playlist(_) => ' ',
        }
    }

    pub fn from_key(c: char) -> Option<Page> {
        Self::ALL.into_iter().find(|p| p.key() == c)
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
        nav.go(Page::Search);
        assert_eq!(nav.next_page(), Page::Queue);
        nav.go(Page::Queue);
        assert_eq!(nav.prev_page(), Page::Search);
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
            assert_eq!(Page::from_key(page.key()), Some(page));
        }
    }
}
