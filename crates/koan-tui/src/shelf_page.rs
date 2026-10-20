//! The shelves and search: Favourites, Recently played, Downloaded and a
//! search's results. Each is the first few artists, albums and tracks that
//! answer one question (`koan_core::shelves`), laid out as the apps' shelf
//! pages and search results are: artists as pills, albums as tiles or rows,
//! tracks as rows. A section's heading opens that kind's browser set up as
//! the shelf, its "See all", whose listing the preview is the head of.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use koan_core::db::queries::{self, CreditsOf};
use koan_core::shelves::{self, Shelf, Summary};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use super::actions_menu::{Playable, playable};
use super::app::PickerAction;
use super::browse::{
    Action, Favourites, SeeAll, Source, Thumbs, Tile, View, album_tracks, album_year,
    artist_tracks, credited_track, tile_grid, track_playable,
};
use super::cover_art::Gfx;
use super::rows::{self, Columns, Emphasis, Row, Target};
use super::theme::Theme;
use super::transport::format_time;

/// How long the search box must rest before a search runs.
const DEBOUNCE: Duration = Duration::from_millis(80);
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// Columns the scrollbar takes: two drawn and one of gap.
const BAR_W: u16 = 3;
/// The right-hand column of an album or track row: a duration or a codec.
const TAIL_W: u16 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfKind {
    Favourites,
    Recent,
    Downloaded,
    Search,
}

impl ShelfKind {
    fn shelf(self, query: &str) -> Shelf<'_> {
        match self {
            ShelfKind::Favourites => Shelf::Favourites,
            ShelfKind::Recent => Shelf::Recent,
            ShelfKind::Downloaded => Shelf::Downloaded,
            ShelfKind::Search => Shelf::Search(query),
        }
    }

    fn title(self) -> &'static str {
        match self {
            ShelfKind::Favourites => "favourites",
            ShelfKind::Recent => "recently played",
            ShelfKind::Downloaded => "downloaded",
            ShelfKind::Search => "search",
        }
    }

    fn empty(self) -> &'static str {
        match self {
            ShelfKind::Favourites => "nothing favourited yet: f hearts what the cursor is on",
            ShelfKind::Recent => "nothing played in the last 30 days",
            ShelfKind::Downloaded => "nothing downloaded to this device yet",
            ShelfKind::Search => "no artists, albums or tracks match",
        }
    }
}

/// The kinds a shelf shows, each under its heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Artists,
    Albums,
    Tracks,
}

impl Part {
    fn source(self) -> Source {
        match self {
            Part::Artists => Source::Artists,
            Part::Albums => Source::Albums,
            Part::Tracks => Source::Tracks,
        }
    }
}

/// Something the cursor can be on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Heading(Part),
    Artist(usize),
    Album(usize),
    Track(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfHit {
    Item(usize),
    Link(Target),
    Input,
    Clear,
}

/// Where everything goes for one width: each item's place in the page as if
/// it were drawn whole, from its top.
#[derive(Default)]
struct Layout {
    width: u16,
    /// The width the page had to lay out in: `width`, or that less the
    /// scrollbar's when the page overflows.
    room: u16,
    view: Option<View>,
    generation: u64,
    items: Vec<(Item, Rect)>,
    /// Where lines start, which the scroll stops at, so a tile is never cut
    /// at the top.
    stops: Vec<u16>,
    height: u16,
    cols: Option<Columns>,
}

#[derive(Default, Clone)]
struct Geom {
    list: Rect,
    bar: Rect,
    input: Rect,
    clear: Rect,
    items: Vec<(Rect, usize)>,
    links: Vec<(Rect, Target)>,
}

/// A read: its generation and the query it is for.
type Ask = (u64, String);

pub struct ShelfPage {
    pub kind: ShelfKind,
    pub query: String,
    /// Whether the keys go to the search box rather than the results.
    pub typing: bool,
    summary: Option<Summary>,
    view: View,
    cursor: usize,
    scroll: usize,
    follow: bool,
    layout: Layout,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
    /// What is a favourite, for the hearts and the menu.
    fav_albums: HashSet<i64>,
    fav_artists: HashSet<i64>,
    fav_tracks: HashSet<i64>,
    /// The last read asked for and the one shown; a summary from any but the
    /// last is dropped.
    generation: u64,
    shown: u64,
    tx: Sender<Ask>,
    rx: Receiver<(u64, Summary, Favourites)>,
}

/// Reads summaries off the UI thread, the last ask of a burst only.
fn spawn_reader(kind: ShelfKind) -> (Sender<Ask>, Receiver<(u64, Summary, Favourites)>) {
    let (tx, req) = crossbeam_channel::unbounded::<Ask>();
    let (done, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("koan-shelf".into())
        .spawn(move || {
            while let Ok(mut next) = req.recv() {
                // Typing waits for the box to rest; a shelf asks once.
                if kind == ShelfKind::Search {
                    loop {
                        match req.recv_timeout(DEBOUNCE) {
                            Ok(newer) => next = newer,
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                }
                while let Ok(newer) = req.try_recv() {
                    next = newer;
                }
                let (generation, query) = next;
                let Ok(db) = koan_core::db::pool::shared().get() else {
                    continue;
                };
                let summary = shelves::summary(
                    &db.conn,
                    kind.shelf(&query),
                    queries::LOCAL_USER,
                    shelves::now(),
                    koan_core::remote::offline::active(),
                );
                let favourites = Favourites::read(&db);
                drop(db);
                if let Ok(summary) = summary
                    && done.send((generation, summary, favourites)).is_err()
                {
                    return;
                }
            }
        })
        .ok();
    (tx, rx)
}

/// Every item of a summary in the order the page lays them out.
fn items_of(s: &Summary) -> Vec<Item> {
    let mut items = Vec::new();
    for (part, len, item) in [
        (
            Part::Artists,
            s.artists.preview.len(),
            Item::Artist as fn(usize) -> Item,
        ),
        (Part::Albums, s.albums.preview.len(), Item::Album),
        (Part::Tracks, s.tracks.preview.len(), Item::Track),
    ] {
        if len > 0 {
            items.push(Item::Heading(part));
            items.extend((0..len).map(item));
        }
    }
    items
}

fn inside(r: &Rect, col: u16, row: u16) -> bool {
    r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

impl ShelfPage {
    pub fn new(kind: ShelfKind) -> Self {
        let (tx, rx) = spawn_reader(kind);
        Self {
            kind,
            query: String::new(),
            typing: kind == ShelfKind::Search,
            summary: None,
            view: View::Tiles,
            cursor: 0,
            scroll: 0,
            follow: true,
            layout: Layout::default(),
            geom: Geom::default(),
            last_click: None,
            fav_albums: HashSet::new(),
            fav_artists: HashSet::new(),
            fav_tracks: HashSet::new(),
            generation: 0,
            shown: 0,
            tx,
            rx,
        }
    }

    fn searching(&self) -> bool {
        self.kind == ShelfKind::Search
    }

    /// Opens the page: a shelf reads itself again, a search keeps what was
    /// searched and is ready for typing.
    pub fn open(&mut self) {
        if self.searching() {
            self.typing = true;
        } else {
            self.reload();
        }
    }

    /// Asks for the summary again; an empty search clears at once.
    pub fn reload(&mut self) {
        self.generation += 1;
        let q = self.query.trim().to_string();
        if self.searching() && q.is_empty() {
            self.summary = None;
            self.shown = self.generation;
            self.cursor = 0;
            self.scroll = 0;
        } else {
            self.tx.send((self.generation, q)).ok();
        }
    }

    /// Takes in what the reader has finished. Returns whether anything came.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((generation, summary, favourites)) = self.rx.try_recv() {
            if generation == self.generation {
                self.fav_albums = favourites.albums;
                self.fav_artists = favourites.artists;
                self.fav_tracks = favourites.tracks;
            }
            any |= self.show(generation, summary);
        }
        any
    }

    fn show(&mut self, generation: u64, summary: Summary) -> bool {
        if generation != self.generation {
            return false;
        }
        let on = self.layout.items.get(self.cursor).map(|(item, _)| *item);
        self.shown = generation;
        self.summary = Some(summary);
        self.last_click = None;
        self.layout = Layout::default();
        if self.searching() {
            self.cursor = 0;
            self.scroll = 0;
            self.follow = true;
        } else {
            // A shelf read again keeps its place: the cursor on the same kind
            // of item at the same place, as near as there is one, else on
            // its section's heading.
            let s = self.summary.as_ref().expect("just set");
            let order = items_of(s);
            let at = |item: Item| order.iter().position(|i| *i == item);
            let near = |i: usize, len: usize| i.min(len.saturating_sub(1));
            self.cursor = match on {
                None => 0,
                Some(on) => {
                    let (same, part) = match on {
                        Item::Heading(part) => (on, part),
                        Item::Artist(i) => (
                            Item::Artist(near(i, s.artists.preview.len())),
                            Part::Artists,
                        ),
                        Item::Album(i) => {
                            (Item::Album(near(i, s.albums.preview.len())), Part::Albums)
                        }
                        Item::Track(i) => {
                            (Item::Track(near(i, s.tracks.preview.len())), Part::Tracks)
                        }
                    };
                    at(same)
                        .or_else(|| at(Item::Heading(part)))
                        .unwrap_or(self.cursor.min(order.len().saturating_sub(1)))
                }
            };
        }
        true
    }

    fn count(&self) -> usize {
        self.summary.as_ref().map_or(0, |s| {
            let heads = [
                !s.artists.preview.is_empty(),
                !s.albums.preview.is_empty(),
                !s.tracks.preview.is_empty(),
            ]
            .into_iter()
            .filter(|on| *on)
            .count();
            heads + s.artists.preview.len() + s.albums.preview.len() + s.tracks.preview.len()
        })
    }

    fn item(&self, idx: usize) -> Option<Item> {
        self.layout.items.get(idx).map(|(item, _)| *item)
    }

    /// Lays the page out for `width` cells, unless it already is.
    fn lay_out(&mut self, width: u16, height: u16, gfx: &Gfx) {
        let fresh = self.layout.width == width
            && self.layout.view == Some(self.view)
            && self.layout.generation == self.shown;
        if fresh {
            return;
        }
        let mut layout = Layout {
            width,
            view: Some(self.view),
            generation: self.shown,
            ..Default::default()
        };
        let Some(s) = &self.summary else {
            self.layout = layout;
            return;
        };
        let cols = Columns::new(Rect::new(0, 0, width, 1), 0, TAIL_W, true, true);
        let mut y = 0u16;
        // Whether a blank line goes before the next heading.
        let mut gap = false;
        let heading = |layout: &mut Layout, y: &mut u16, gap: &mut bool, part: Part| {
            if std::mem::replace(gap, true) {
                *y += 1;
            }
            layout
                .items
                .push((Item::Heading(part), Rect::new(0, *y, width, 1)));
            layout.stops.push(*y);
            *y += 1;
        };
        if !s.artists.preview.is_empty() {
            heading(&mut layout, &mut y, &mut gap, Part::Artists);
            let mut x = 1u16;
            layout.stops.push(y);
            for (i, a) in s.artists.preview.iter().enumerate() {
                let w = (a.name.width() as u16 + 2)
                    .min(width.saturating_sub(2))
                    .max(3);
                if x > 1 && x + w > width.saturating_sub(1) {
                    x = 1;
                    y += 1;
                    layout.stops.push(y);
                }
                layout.items.push((Item::Artist(i), Rect::new(x, y, w, 1)));
                x += w + 2;
            }
            y += 1;
        }
        if !s.albums.preview.is_empty() {
            heading(&mut layout, &mut y, &mut gap, Part::Albums);
            match self.view {
                View::Tiles => {
                    let (n, tile_w, tile_h, _) = tile_grid(width, height, gfx);
                    for (i, _) in s.albums.preview.iter().enumerate() {
                        let (r, c) = (i / n, i % n);
                        let ty = y + r as u16 * tile_h;
                        if c == 0 {
                            layout.stops.push(ty);
                        }
                        let rect = Rect::new(c as u16 * tile_w, ty, tile_w, tile_h);
                        layout.items.push((Item::Album(i), rect));
                    }
                    y += s.albums.preview.len().div_ceil(n) as u16 * tile_h;
                    // The tiles end in their own gap.
                    gap = false;
                }
                View::List => {
                    for i in 0..s.albums.preview.len() {
                        layout.stops.push(y);
                        layout
                            .items
                            .push((Item::Album(i), Rect::new(0, y, width, 1)));
                        y += 1;
                    }
                }
            }
        }
        if !s.tracks.preview.is_empty() {
            heading(&mut layout, &mut y, &mut gap, Part::Tracks);
            for i in 0..s.tracks.preview.len() {
                layout.stops.push(y);
                layout
                    .items
                    .push((Item::Track(i), Rect::new(0, y, width, 1)));
                y += 1;
            }
        }
        layout.height = y;
        layout.cols = Some(cols);
        self.layout = layout;
    }

    fn rows(&self) -> u16 {
        self.geom.list.height.max(1)
    }

    /// The scroll's top, as a line of the page.
    fn top(&self) -> u16 {
        self.layout.stops.get(self.scroll).copied().unwrap_or(0)
    }

    /// The furthest stop the scroll goes to: the first from which the rest
    /// of the page fits.
    fn max_scroll(&self) -> usize {
        let h = self.rows();
        let last = self.layout.height.saturating_sub(h);
        self.layout
            .stops
            .iter()
            .position(|&s| s >= last)
            .unwrap_or(self.layout.stops.len().saturating_sub(1))
    }

    fn keep_cursor_visible(&mut self) {
        let Some(&(_, rect)) = self.layout.items.get(self.cursor) else {
            return;
        };
        let h = self.rows();
        // Up to the heading over the first of a section, so it is seen.
        let first = match self.item(self.cursor.saturating_sub(1)) {
            Some(Item::Heading(_)) if self.cursor > 0 => self.layout.items[self.cursor - 1].1.y,
            _ => rect.y,
        };
        if first < self.top() {
            self.scroll = self
                .layout
                .stops
                .iter()
                .rposition(|&s| s <= first)
                .unwrap_or(0);
        }
        while rect.y + rect.height > self.top() + h && self.scroll < self.max_scroll() {
            self.scroll += 1;
        }
    }

    fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.layout.items.len().saturating_sub(1));
        self.follow = true;
        self.keep_cursor_visible();
    }

    /// Up or down a line: to the item on the line before or after the
    /// cursor's nearest across.
    fn move_line(&mut self, down: bool) {
        let Some(&(_, at)) = self.layout.items.get(self.cursor) else {
            return;
        };
        let centre = at.x + at.width / 2;
        let line = |r: &Rect| r.y;
        let next = self
            .layout
            .items
            .iter()
            .map(|(_, r)| line(r))
            .filter(|&y| if down { y > at.y } else { y < at.y })
            .reduce(|a, b| if down { a.min(b) } else { a.max(b) });
        let Some(y) = next else {
            if !down && self.searching() {
                self.typing = true;
            }
            return;
        };
        let best = self
            .layout
            .items
            .iter()
            .enumerate()
            .filter(|(_, (_, r))| r.y == y)
            .min_by_key(|(_, (_, r))| (r.x + r.width / 2).abs_diff(centre))
            .map(|(i, _)| i);
        if let Some(i) = best {
            self.move_to(i);
        }
    }

    pub fn scroll_by(&mut self, lines: isize) {
        self.follow = false;
        self.scroll = self
            .scroll
            .saturating_add_signed(lines)
            .min(self.max_scroll());
    }

    /// The tracks of whatever is at `idx`.
    fn ids_at(&self, idx: usize) -> Vec<i64> {
        let Some(s) = &self.summary else {
            return Vec::new();
        };
        match self.item(idx) {
            Some(Item::Track(i)) => vec![s.tracks.preview[i].id],
            Some(Item::Album(i)) => album_tracks(&[s.albums.preview[i].id]),
            Some(Item::Artist(i)) => artist_tracks(s.artists.preview[i].id),
            Some(Item::Heading(_)) | None => Vec::new(),
        }
    }

    fn play(&self, idx: usize, how: PickerAction) -> Action {
        let ids = self.ids_at(idx);
        if ids.is_empty() {
            Action::None
        } else {
            Action::Play(ids, how)
        }
    }

    fn see_all(&self, part: Part) -> Action {
        let (filter, album_sort, track_sort) =
            koan_core::browse::of_shelf(self.kind.shelf(&self.query));
        Action::SeeAll(SeeAll {
            source: part.source(),
            filter,
            album_sort,
            track_sort,
            text: if self.searching() {
                self.query.trim().to_string()
            } else {
                String::new()
            },
        })
    }

    /// What Enter and a second click do: a heading opens its "See all", an
    /// artist or an album opens, and a track plays; on a shelf, the shelf's
    /// tracks from it.
    fn primary(&self, idx: usize) -> Action {
        let Some(s) = &self.summary else {
            return Action::None;
        };
        match self.item(idx) {
            Some(Item::Heading(part)) => self.see_all(part),
            Some(Item::Artist(i)) => Action::OpenArtist(s.artists.preview[i].id),
            Some(Item::Album(i)) => Action::OpenAlbum(s.albums.preview[i].id),
            Some(Item::Track(i)) if self.searching() => {
                Action::Play(vec![s.tracks.preview[i].id], PickerAction::AppendAndPlay)
            }
            Some(Item::Track(i)) => Action::Play(
                s.tracks.preview.iter().map(|t| t.id).collect(),
                PickerAction::ReplaceFrom(i),
            ),
            None => Action::None,
        }
    }

    fn album_of(&self, idx: usize) -> Option<i64> {
        let s = self.summary.as_ref()?;
        match self.item(idx)? {
            Item::Album(i) => Some(s.albums.preview[i].id),
            Item::Track(i) => s.tracks.preview[i].album_id,
            _ => None,
        }
    }

    fn artist_of(&self, idx: usize) -> Option<i64> {
        let s = self.summary.as_ref()?;
        match self.item(idx)? {
            Item::Artist(i) => Some(s.artists.preview[i].id),
            Item::Album(i) => Some(s.albums.preview[i].artist_id),
            Item::Track(i) => s.tracks.preview[i].artist_id,
            Item::Heading(_) => None,
        }
    }

    /// What `A` opens: the artist at `idx`, or the artists its credit names.
    fn open_artist_at(&self, idx: usize) -> Action {
        let Some(s) = &self.summary else {
            return Action::None;
        };
        match self.item(idx) {
            Some(Item::Artist(i)) => Action::OpenArtist(s.artists.preview[i].id),
            Some(Item::Album(i)) => Action::OpenCredited(CreditsOf::Albums, s.albums.preview[i].id),
            Some(Item::Track(i)) if s.tracks.preview[i].artist_id.is_some() => {
                Action::OpenCredited(CreditsOf::Tracks, s.tracks.preview[i].id)
            }
            _ => Action::None,
        }
    }

    fn favourite_at(&self, idx: usize) -> Action {
        let Some(s) = &self.summary else {
            return Action::None;
        };
        match self.item(idx) {
            Some(Item::Artist(i)) => Action::ToggleArtist(s.artists.preview[i].id),
            Some(Item::Album(i)) => Action::ToggleAlbum(s.albums.preview[i].id),
            Some(Item::Track(i)) => Action::Favourite(s.tracks.preview[i].id),
            _ => Action::None,
        }
    }

    /// What can be done with the item at `idx`.
    fn menu(&self, idx: usize) -> Action {
        let Some(s) = &self.summary else {
            return Action::None;
        };
        let ids = self.ids_at(idx);
        if ids.is_empty() {
            return Action::None;
        }
        let (title, thing) = match self.item(idx) {
            Some(Item::Artist(i)) => {
                let a = &s.artists.preview[i];
                let thing = Playable::Artist {
                    id: a.id,
                    tracks: ids,
                    favourite: self.fav_artists.contains(&a.id),
                };
                (a.name.clone(), thing)
            }
            Some(Item::Album(i)) => {
                let a = &s.albums.preview[i];
                let thing = Playable::Album {
                    id: a.id,
                    tracks: ids,
                    favourite: self.fav_albums.contains(&a.id),
                };
                (a.title.clone(), thing)
            }
            Some(Item::Track(i)) => {
                let t = &s.tracks.preview[i];
                let thing = track_playable(t, self.fav_tracks.contains(&t.id));
                (t.title.clone(), thing)
            }
            _ => return Action::None,
        };
        Action::Menu(title, playable(&thing))
    }

    fn set_query(&mut self, query: String) {
        if query != self.query {
            self.query = query;
            self.reload();
        }
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Action> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.typing {
            match key.code {
                KeyCode::Esc if !self.query.is_empty() => self.set_query(String::new()),
                KeyCode::Esc | KeyCode::BackTab => return None,
                KeyCode::Enter | KeyCode::Down | KeyCode::Tab if self.count() > 0 => {
                    self.typing = false;
                    self.move_to(0);
                }
                KeyCode::Tab => return None,
                KeyCode::Enter | KeyCode::Down => {}
                KeyCode::Backspace => {
                    let mut q = self.query.clone();
                    q.pop();
                    self.set_query(q);
                }
                KeyCode::Char('u') if ctrl => self.set_query(String::new()),
                KeyCode::Char(c) if !ctrl => self.set_query(format!("{}{c}", self.query)),
                _ => {}
            }
            return Some(Action::None);
        }
        let n = self.layout.items.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_line(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_line(true),
            KeyCode::Left | KeyCode::Char('h') => {
                if let (Some(&(_, at)), Some(&(_, before))) = (
                    self.layout.items.get(self.cursor),
                    self.layout.items.get(self.cursor.wrapping_sub(1)),
                ) && before.y == at.y
                {
                    self.move_to(self.cursor - 1);
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if let (Some(&(_, at)), Some(&(_, after))) = (
                    self.layout.items.get(self.cursor),
                    self.layout.items.get(self.cursor + 1),
                ) && after.y == at.y
                {
                    self.move_to(self.cursor + 1);
                }
            }
            KeyCode::PageUp | KeyCode::Char('u') if key.code == KeyCode::PageUp || ctrl => {
                for _ in 0..self.rows() {
                    self.move_line(false);
                }
            }
            KeyCode::PageDown | KeyCode::Char('d') if key.code == KeyCode::PageDown || ctrl => {
                for _ in 0..self.rows() {
                    self.move_line(true);
                }
            }
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(n.saturating_sub(1)),
            KeyCode::Enter => return Some(self.primary(self.cursor)),
            KeyCode::Char('O') => {
                return Some(
                    self.album_of(self.cursor)
                        .map_or(Action::None, Action::OpenAlbum),
                );
            }
            KeyCode::Char('A') => return Some(self.open_artist_at(self.cursor)),
            KeyCode::Char('P') => return Some(self.play(self.cursor, PickerAction::AppendAndPlay)),
            KeyCode::Char('a') => return Some(self.play(self.cursor, PickerAction::Append)),
            KeyCode::Char('N') => return Some(self.play(self.cursor, PickerAction::PlayNext)),
            KeyCode::Char('r') if ctrl => {
                return Some(self.play(self.cursor, PickerAction::ReplaceQueue));
            }
            KeyCode::Char('m') => return Some(self.menu(self.cursor)),
            KeyCode::Char('f') => return Some(self.favourite_at(self.cursor)),
            KeyCode::Char('H') => {
                return Some(
                    self.artist_of(self.cursor)
                        .map_or(Action::None, Action::ToggleArtist),
                );
            }
            KeyCode::Char('i') => {
                return Some(match (self.item(self.cursor), &self.summary) {
                    (Some(Item::Track(i)), Some(s)) => Action::Info(s.tracks.preview[i].id),
                    _ => Action::None,
                });
            }
            KeyCode::Char('w' | 'd' | 'u') => {
                let ids = self.ids_at(self.cursor);
                if ids.is_empty() {
                    return Some(Action::None);
                }
                return Some(match key.code {
                    KeyCode::Char('w') => Action::Choose(ids),
                    KeyCode::Char('d') => Action::Download(ids),
                    _ => Action::RemoveDownload(ids),
                });
            }
            KeyCode::Char('v') => self.toggle_view(),
            KeyCode::Char('/') if self.searching() => self.typing = true,
            KeyCode::Esc if self.searching() && !self.query.is_empty() => {
                self.set_query(String::new());
                self.typing = true;
            }
            _ => return None,
        }
        Some(Action::None)
    }

    /// Switches albums between tiles and rows, the cursor where it was.
    pub fn toggle_view(&mut self) {
        let on = self.item(self.cursor);
        self.view = match self.view {
            View::Tiles => View::List,
            View::List => View::Tiles,
        };
        // The items stay in the same order; only where they go changes.
        self.layout.view = None;
        if on.is_some() {
            self.follow = true;
        }
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<ShelfHit> {
        let g = &self.geom;
        if inside(&g.clear, col, row) {
            return Some(ShelfHit::Clear);
        }
        if inside(&g.input, col, row) {
            return Some(ShelfHit::Input);
        }
        if let Some((_, t)) = g.links.iter().find(|(r, _)| inside(r, col, row)) {
            return Some(ShelfHit::Link(*t));
        }
        g.items
            .iter()
            .find(|(r, _)| inside(r, col, row))
            .map(|(_, i)| ShelfHit::Item(*i))
    }

    pub fn click(&mut self, hit: ShelfHit) -> Action {
        match hit {
            ShelfHit::Input => {
                self.typing = true;
                Action::None
            }
            ShelfHit::Clear => {
                self.set_query(String::new());
                self.typing = true;
                Action::None
            }
            ShelfHit::Link(Target::Album(id)) => Action::OpenAlbum(id),
            ShelfHit::Link(Target::Credited(of, id)) => Action::OpenCredited(of, id),
            ShelfHit::Item(i) => {
                self.typing = false;
                let double = self
                    .last_click
                    .is_some_and(|(n, at)| n == i && at.elapsed() < DOUBLE_CLICK);
                self.move_to(i);
                // A heading leads somewhere at once, as a link does.
                if double || matches!(self.item(i), Some(Item::Heading(_))) {
                    self.last_click = None;
                    self.primary(i)
                } else {
                    self.last_click = Some((i, Instant::now()));
                    Action::None
                }
            }
        }
    }

    pub fn on_scrollbar(&self, col: u16, row: u16) -> bool {
        inside(&self.geom.bar, col, row)
    }

    pub fn scroll_to_bar(&mut self, row: u16) {
        self.follow = false;
        let b = self.geom.bar;
        if b.height == 0 {
            return;
        }
        let max = self.max_scroll();
        let frac = row.saturating_sub(b.y) as f64 / b.height.saturating_sub(1).max(1) as f64;
        self.scroll = ((frac.clamp(0.0, 1.0) * max as f64).round() as usize).min(max);
    }

    /// The whole shelf's counts, not the preview's, of the kinds it has.
    /// The shelf's title, what it holds, and its view and actions.
    pub fn toolbar(&self) -> crate::shell::Toolbar {
        use crate::shell::{ToolItem, Toolbar};
        use crossterm::event::{KeyCode, KeyEvent};
        let mut bar = Toolbar::new(self.kind.title());
        let counts = self.counts();
        if !counts.is_empty() {
            bar = bar.count(counts);
        }
        if self
            .summary
            .as_ref()
            .is_some_and(|s| !s.albums.preview.is_empty())
        {
            let view = match self.view {
                View::Tiles => "view: grid",
                View::List => "view: list",
            };
            bar = bar.item(ToolItem::new(view, KeyEvent::from(KeyCode::Char('v'))).priority(2));
        }
        if self.count() > 0 {
            bar = bar.item(
                ToolItem::new("\u{22ef} actions", KeyEvent::from(KeyCode::Char('m'))).priority(1),
            );
        }
        bar
    }

    fn counts(&self) -> String {
        let Some(s) = &self.summary else {
            return String::new();
        };
        [
            (s.artists.total, "artist"),
            (s.albums.total, "album"),
            (s.tracks.total, "track"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, noun)| super::text::count(n, noun))
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        gfx: &mut Gfx,
        thumbs: &mut Thumbs,
        favourites: &HashSet<i64>,
        hover: Option<ShelfHit>,
    ) {
        self.geom = Geom::default();
        if area.width < 8 || area.height < 2 {
            return;
        }
        thumbs.begin_frame();
        gfx.begin_frame();
        let mut top = super::shell::body(area).y;
        if self.searching() {
            self.render_box(Rect::new(area.x, top, area.width, 1), buf, theme, hover);
            top += 2;
        }
        let list = Rect::new(
            area.x,
            top,
            area.width,
            (area.y + area.height).saturating_sub(top),
        );
        if list.height == 0 {
            return;
        }
        if self.count() == 0 {
            let msg = if self.searching() && self.query.trim().is_empty() {
                "artists, albums and tracks that match appear here"
            } else if self.shown != self.generation {
                ""
            } else {
                self.kind.empty()
            };
            rows::put(
                buf,
                list.x + 1,
                list.y,
                list.width.saturating_sub(2),
                msg,
                theme.hint_desc,
            );
            self.geom.list = list;
            return;
        }
        // Laid out across the whole width first; narrowed for the scrollbar
        // if that overflows. Once a frame has decided, the next with the
        // same shelf and room lays nothing out.
        let fresh = self.layout.room == list.width
            && self.layout.view == Some(self.view)
            && self.layout.generation == self.shown;
        if !fresh {
            self.lay_out(list.width, list.height, gfx);
            if self.layout.height > list.height && list.width > 20 {
                self.lay_out(list.width - BAR_W, list.height, gfx);
            }
            self.layout.room = list.width;
        }
        let bar = self.layout.width < list.width;
        let text = Rect {
            width: self.layout.width,
            ..list
        };
        self.geom.list = text;
        self.cursor = self.cursor.min(self.layout.items.len().saturating_sub(1));
        if self.follow {
            self.keep_cursor_visible();
        }
        self.scroll = self.scroll.min(self.max_scroll());
        self.draw_items(text, buf, theme, gfx, thumbs, favourites, hover);
        if bar {
            self.scrollbar(list, buf, theme);
        }
    }

    fn render_box(&mut self, at: Rect, buf: &mut Buffer, theme: &Theme, hover: Option<ShelfHit>) {
        let end = at.x + at.width;
        buf.set_string(at.x, at.y, "/", theme.hint_key);
        let (text, style) = if self.query.is_empty() && !self.typing {
            ("search the library".to_string(), theme.hint_desc)
        } else {
            let cursor = if self.typing { "\u{2588}" } else { "" };
            (format!("{}{cursor}", self.query), theme.track_normal)
        };
        let clear_w = if self.query.is_empty() { 0 } else { 7 };
        rows::put(
            buf,
            at.x + 2,
            at.y,
            at.width.saturating_sub(3 + clear_w),
            &text,
            style,
        );
        self.geom.input = Rect::new(at.x, at.y, at.width.saturating_sub(clear_w), 1);
        if clear_w > 0 {
            let style = rows::link(theme.hint_desc, hover == Some(ShelfHit::Clear));
            self.geom.clear = rows::put(buf, end - 5, at.y, 5, "clear", style);
        }
        // A rule under the box, as a field's edge.
        let w = at.width.saturating_sub(clear_w);
        for x in at.x..at.x + w {
            if at.y + 1 < buf.area.y + buf.area.height {
                buf[(x, at.y + 1)]
                    .set_char('\u{2500}')
                    .set_style(theme.rule);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_items(
        &mut self,
        list: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        gfx: &mut Gfx,
        thumbs: &mut Thumbs,
        favourites: &HashSet<i64>,
        hover: Option<ShelfHit>,
    ) {
        let Some(s) = &self.summary else {
            return;
        };
        let top = self.top();
        let bottom = list.y + list.height;
        let hover_link = match hover {
            Some(ShelfHit::Link(t)) => Some(t),
            _ => None,
        };
        let hover_item = match hover {
            Some(ShelfHit::Item(i)) => Some(i),
            _ => None,
        };
        let cols = self.layout.cols.unwrap_or_default();
        let shift = |c: Columns| Columns {
            heart: c.heart + list.x,
            lead: rows::Span {
                x: c.lead.x + list.x,
                ..c.lead
            },
            name: rows::Span {
                x: c.name.x + list.x,
                ..c.name
            },
            artist: rows::Span {
                x: c.artist.x + list.x,
                ..c.artist
            },
            detail: rows::Span {
                x: c.detail.x + list.x,
                ..c.detail
            },
            tail: rows::Span {
                x: c.tail.x + list.x,
                ..c.tail
            },
        };
        let cols = shift(cols);
        let moving = false;
        let mut links = Vec::new();
        let mut items = Vec::new();
        for (idx, &(item, rect)) in self.layout.items.iter().enumerate() {
            if rect.y + rect.height <= top || rect.y >= top + list.height {
                continue;
            }
            let y = list.y + (rect.y - top);
            let at = Rect::new(list.x + rect.x, y, rect.width, rect.height);
            let selected = idx == self.cursor && !self.typing;
            let emphasis = Emphasis::of(selected, hover_item == Some(idx));
            let row = Rect::new(list.x, y, list.width, 1);
            match item {
                Item::Heading(part) => {
                    let (name, total) = match part {
                        Part::Artists => ("artists", s.artists.total),
                        Part::Albums => ("albums", s.albums.total),
                        Part::Tracks => ("tracks", s.tracks.total),
                    };
                    if selected {
                        Emphasis::Selected.ground(buf, row, theme);
                    }
                    let clickable = rows::heading(
                        buf,
                        row,
                        theme,
                        name,
                        &total.to_string(),
                        true,
                        hover_item == Some(idx),
                    );
                    items.push((clickable, idx));
                }
                Item::Artist(i) => {
                    let a = &s.artists.preview[i];
                    let style = match emphasis {
                        Emphasis::Selected => theme.library_cursor,
                        Emphasis::Hovered => theme.library_hover,
                        Emphasis::Plain => {
                            Style::new().bg(theme.tokens.surface).fg(theme.tokens.ink)
                        }
                    };
                    for x in at.x..(at.x + at.width).min(list.x + list.width) {
                        buf[(x, y)].set_char(' ').set_style(style);
                    }
                    rows::put(buf, at.x + 1, y, at.width.saturating_sub(2), &a.name, style);
                    items.push((at, idx));
                }
                Item::Album(i) => {
                    let a = &s.albums.preview[i];
                    match self.view {
                        View::Tiles => {
                            Tile {
                                album: a,
                                favourite: self.fav_albums.contains(&a.id),
                                selected,
                                name_artist: true,
                            }
                            .draw(
                                buf, theme, gfx, thumbs, at, bottom, moving, hover_link, &mut links,
                            );
                            items.push((at, idx));
                        }
                        View::List => {
                            let year = album_year(a.date.as_deref()).unwrap_or_default();
                            let codec = a.codec.clone().unwrap_or_default();
                            Row {
                                favourite: self.fav_albums.contains(&a.id),
                                lead: "",
                                name: &a.title,
                                artist: Some((
                                    &a.artist_name,
                                    Some(Target::Credited(CreditsOf::Albums, a.id)),
                                )),
                                detail: Some((&year, None)),
                                tail: &codec,
                            }
                            .draw(buf, &cols, row, theme, emphasis, hover_link, &mut links);
                            items.push((row, idx));
                        }
                    }
                }
                Item::Track(i) => {
                    let t = &s.tracks.preview[i];
                    let time = t
                        .duration_ms
                        .map(|d| format_time(d as u64))
                        .unwrap_or_default();
                    Row {
                        favourite: favourites.contains(&t.id),
                        lead: "",
                        name: &t.title,
                        artist: Some((&t.artist_name, credited_track(t))),
                        detail: Some((&t.album_title, t.album_id.map(Target::Album))),
                        tail: &time,
                    }
                    .draw(buf, &cols, row, theme, emphasis, hover_link, &mut links);
                    items.push((row, idx));
                }
            }
        }
        self.geom.links = links;
        self.geom.items = items;
    }

    fn scrollbar(&mut self, list: Rect, buf: &mut Buffer, theme: &Theme) {
        let x = list.x + list.width - 1;
        let h = list.height as usize;
        let total = self.layout.height as usize;
        let visible = h.min(total);
        self.geom.bar = Rect::new(x - 2, list.y, BAR_W, list.height);
        let thumb = (h * visible / total.max(1)).max(1);
        let max = self.max_scroll().max(1);
        let top = (h - thumb) * self.scroll.min(max) / max;
        for row in 0..h {
            let on = row >= top && row < top + thumb;
            for cx in [x - 1, x] {
                buf[(cx, list.y + row as u16)]
                    .set_char(if on { '\u{2588}' } else { ' ' })
                    .set_style(if on {
                        Style::new().fg(theme.scrollbar_thumb)
                    } else {
                        Style::new().bg(theme.scrollbar_bg)
                    });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::tests::{album, track};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use koan_core::db::queries::ArtistRow;
    use koan_core::shelves::Section;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn artist(id: i64, name: &str) -> ArtistRow {
        ArtistRow {
            id,
            name: name.into(),
            sort_name: None,
            remote_id: None,
            album_count: 1,
            track_count: 10,
        }
    }

    fn summary(artists: usize, albums: usize, tracks: usize) -> Summary {
        Summary {
            artists: Section {
                preview: (0..artists as i64)
                    .map(|i| artist(i, &format!("artist {i}")))
                    .collect(),
                total: artists as u64 + 20,
            },
            albums: Section {
                preview: (0..albums as i64)
                    .map(|i| album(i, &format!("album {i}"), "someone"))
                    .collect(),
                total: albums as u64,
            },
            tracks: Section {
                preview: (0..tracks as i64).map(|i| track(i, i)).collect(),
                total: tracks as u64,
            },
        }
    }

    fn page(kind: ShelfKind, s: Summary) -> ShelfPage {
        koan_core::config::isolate_config_for_tests();
        let mut p = ShelfPage::new(kind);
        p.typing = false;
        p.generation = 1;
        p.show(1, s);
        p
    }

    fn draw(p: &mut ShelfPage, w: u16, h: u16) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        p.render(
            area,
            &mut buf,
            &Theme::default(),
            &mut Gfx::halfblocks(),
            &mut Thumbs::new(),
            &HashSet::new(),
            None,
        );
        buf
    }

    fn line(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn sections_come_in_order_under_their_headings() {
        let mut p = page(ShelfKind::Favourites, summary(3, 2, 4));
        let buf = draw(&mut p, 100, 60);
        let text: Vec<String> = (0..60).map(|y| line(&buf, y)).collect();
        let at = |s: &str| text.iter().position(|l| l.contains(s)).unwrap();
        assert!(at("artists 23") < at("albums 2"));
        assert!(at("albums 2") < at("tracks 4"));
        // The counts are the shell's toolbar's.
        assert_eq!(
            p.toolbar().count.as_deref(),
            Some("23 artists \u{b7} 2 albums \u{b7} 4 tracks")
        );
    }

    #[test]
    fn a_shelf_read_again_keeps_the_cursor_on_its_item() {
        let mut p = page(ShelfKind::Favourites, summary(2, 0, 3));
        draw(&mut p, 100, 40);
        // On the second track, after the artists' heading and pills and the
        // tracks' heading.
        p.cursor = 5;
        assert_eq!(p.item(p.cursor), Some(Item::Track(1)));
        // One artist fewer: the track is a row higher, and the cursor too.
        p.generation = 2;
        p.show(2, summary(1, 0, 3));
        draw(&mut p, 100, 40);
        assert_eq!(p.item(p.cursor), Some(Item::Track(1)));
        // No artists at all: their heading goes as well.
        p.generation = 3;
        p.show(3, summary(0, 0, 3));
        draw(&mut p, 100, 40);
        assert_eq!(p.item(p.cursor), Some(Item::Track(1)));
    }

    #[test]
    fn a_heading_opens_its_see_all() {
        let mut p = page(ShelfKind::Recent, summary(2, 2, 2));
        draw(&mut p, 100, 60);
        let Action::SeeAll(see) = p.primary(0) else {
            panic!("the first item is the artists' heading");
        };
        assert_eq!(see.source, Source::Artists);
        assert!(see.filter.recent);
        let (r, _) = p.geom.items[0];
        assert!(matches!(
            p.click(ShelfHit::Item(p.hit(r.x, r.y).map_or(0, |h| match h {
                ShelfHit::Item(i) => i,
                _ => 99,
            }))),
            Action::SeeAll(_)
        ));
    }

    #[test]
    fn a_search_carries_its_query_into_see_all() {
        let mut p = page(ShelfKind::Search, summary(1, 1, 1));
        p.query = "moss".into();
        draw(&mut p, 100, 60);
        let tracks = p
            .layout
            .items
            .iter()
            .position(|(i, _)| *i == Item::Heading(Part::Tracks))
            .unwrap();
        let Action::SeeAll(see) = p.primary(tracks) else {
            panic!("a heading");
        };
        assert_eq!(see.text, "moss");
        assert_eq!(see.track_sort, koan_core::browse::TrackSort::BestMatch);
    }

    #[test]
    fn the_artist_column_lines_up_across_albums_and_tracks() {
        let mut p = page(ShelfKind::Search, summary(0, 3, 3));
        p.toggle_view();
        draw(&mut p, 120, 40);
        let xs: Vec<u16> = p
            .geom
            .links
            .iter()
            .filter(|(_, t)| matches!(t, Target::Credited(..)))
            .map(|(r, _)| r.x)
            .collect();
        assert_eq!(xs.len(), 6);
        assert!(xs.windows(2).all(|w| w[0] == w[1]), "{xs:?}");
    }

    #[test]
    fn up_and_down_move_by_line_and_left_and_right_within_one() {
        let mut p = page(ShelfKind::Favourites, summary(3, 0, 3));
        draw(&mut p, 100, 40);
        p.move_to(1);
        assert_eq!(p.item(p.cursor), Some(Item::Artist(0)));
        p.handle_key(key(KeyCode::Right));
        assert_eq!(p.item(p.cursor), Some(Item::Artist(1)));
        p.handle_key(key(KeyCode::Down));
        assert_eq!(p.item(p.cursor), Some(Item::Heading(Part::Tracks)));
        p.handle_key(key(KeyCode::Down));
        assert_eq!(p.item(p.cursor), Some(Item::Track(0)));
        p.handle_key(key(KeyCode::Left));
        assert_eq!(p.item(p.cursor), Some(Item::Track(0)), "nothing beside it");
    }

    #[test]
    fn the_scroll_never_cuts_a_tile_at_the_top() {
        let mut p = page(ShelfKind::Favourites, summary(2, 30, 10));
        draw(&mut p, 80, 24);
        p.handle_key(key(KeyCode::End));
        draw(&mut p, 80, 24);
        let top = p.top();
        assert!(p.layout.stops.contains(&top));
        for (item, r) in &p.layout.items {
            if matches!(item, Item::Album(_)) {
                assert!(r.y >= top || r.y + r.height <= top, "{r:?} cut at {top}");
            }
        }
    }

    #[test]
    fn enter_on_a_shelf_track_plays_the_shelf_from_it() {
        let mut p = page(ShelfKind::Favourites, summary(0, 0, 4));
        draw(&mut p, 100, 30);
        p.move_to(3);
        assert_eq!(
            p.handle_key(key(KeyCode::Enter)),
            Some(Action::Play(vec![0, 1, 2, 3], PickerAction::ReplaceFrom(2)))
        );
    }

    #[test]
    fn typing_builds_the_query_and_escape_clears_then_leaves() {
        koan_core::config::isolate_config_for_tests();
        let mut p = ShelfPage::new(ShelfKind::Search);
        for c in "abc".chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(p.query, "abc");
        p.handle_key(key(KeyCode::Backspace));
        assert_eq!(p.query, "ab");
        assert_eq!(p.handle_key(key(KeyCode::Esc)), Some(Action::None));
        assert_eq!(p.query, "");
        assert!(p.handle_key(key(KeyCode::Esc)).is_none(), "now it leaves");
    }

    #[test]
    fn numbers_and_commands_are_text_while_typing() {
        koan_core::config::isolate_config_for_tests();
        let mut p = ShelfPage::new(ShelfKind::Search);
        for c in "1a/".chars() {
            assert!(p.handle_key(key(KeyCode::Char(c))).is_some());
        }
        assert_eq!(p.query, "1a/");
    }

    #[test]
    fn only_the_latest_search_is_shown() {
        let mut p = page(ShelfKind::Search, summary(1, 0, 0));
        p.generation = 3;
        assert!(!p.show(2, summary(5, 0, 0)), "a stale result is dropped");
        assert_eq!(p.count(), 2);
        assert!(p.show(3, summary(4, 0, 0)));
        assert_eq!(p.count(), 5);
    }

    #[test]
    fn it_draws_at_any_size() {
        for kind in [ShelfKind::Favourites, ShelfKind::Search] {
            let mut p = page(kind, summary(12, 8, 10));
            for view in [View::Tiles, View::List] {
                p.view = view;
                for w in [0u16, 5, 8, 9, 20, 50, 80, 200] {
                    for h in [0u16, 2, 3, 5, 12, 40] {
                        draw(&mut p, w, h);
                    }
                }
            }
        }
    }
}
