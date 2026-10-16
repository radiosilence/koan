//! The browsing pages: Albums, Favourites and Recent. One page type with a
//! source, because they are the library's own listings with a filter on
//! (`koan_core::shelves`); what each holds and in what order is decided there.
//! Records are shown as tiles of cover art, or as rows.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use image::DynamicImage;
use koan_core::db::queries::{self, AlbumOrder, AlbumQuery, AlbumRow, TrackRow};
use koan_core::shelves::{self, Shelf};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::app::PickerAction;
use super::artist_header::{ArtistHeader, HeaderHit};
use super::cover_art::{Gfx, album_key};
use super::theme::Theme;
use super::transport::format_time;

/// The widest a tile gets, in columns. Wider screens get more tiles.
const TILE_MAX_W: u16 = 26;
const TILE_MIN_W: u16 = 14;

/// Decoded covers kept for tiles before the lot is dropped.
const THUMB_LIMIT: usize = 240;

/// A second click this soon after the first is a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Albums,
    Favourites,
    Recent,
    /// One artist's records.
    Artist(i64),
}

impl Source {
    fn shelf(self) -> Option<Shelf<'static>> {
        match self {
            Source::Albums | Source::Artist(_) => None,
            Source::Favourites => Some(Shelf::Favourites),
            Source::Recent => Some(Shelf::Recent),
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Source::Albums => "albums",
            Source::Favourites => "nothing favourited yet",
            Source::Recent => "nothing played in the last 30 days",
            Source::Artist(_) => "no records by this artist",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Tiles,
    List,
}

const ORDERS: [(AlbumOrder, &str); 4] = [
    (AlbumOrder::ArtistThenDate, "artist"),
    (AlbumOrder::RecentlyAdded, "added"),
    (AlbumOrder::Title, "title"),
    (AlbumOrder::YearDesc, "year"),
];

/// What a click or key asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Play(Vec<i64>, PickerAction),
    Favourite(i64),
    OpenAlbum(i64),
    OpenArtist(i64),
    /// Replace the queue with these, shuffled.
    Shuffle(Vec<i64>),
    /// A link out of the terminal.
    OpenUrl(String),
    ToggleAlbum(i64),
    ToggleArtist(i64),
    /// Show what the library holds of a track.
    Info(i64),
    /// Choose a playlist to put these tracks in.
    Choose(Vec<i64>),
    /// Fetch these tracks to this device, or let the copies go.
    Download(Vec<i64>),
    RemoveDownload(Vec<i64>),
}

/// Where the pointer is on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Item(usize),
    /// A record's title under its tile: a link to the record.
    Title(usize),
    /// A record's artist under its tile: a link to the artist.
    Artist(usize),
    Sort,
    View,
    Filter,
    Header(HeaderHit),
}

/// How many covers are fetched at once. Each of a remote library's is an HTTP
/// round trip, so one at a time would fill a screen of tiles over many seconds.
const THUMB_WORKERS: usize = 6;

enum Loaded {
    Image(Option<DynamicImage>),
    /// Scrolled away before its turn came; asked for again if it comes back.
    Skipped,
}

/// Covers, loaded off the UI thread. Asking for one that is not there starts
/// the load and answers nothing; the page asks again next frame. Only covers
/// the last frame wanted are loaded, so scrolling a long library does not
/// queue a fetch for every tile it flew past.
pub struct Thumbs {
    images: HashMap<i64, Option<DynamicImage>>,
    asked: HashSet<i64>,
    wanted: Arc<Mutex<HashSet<i64>>>,
    tx: Sender<i64>,
    rx: Receiver<(i64, Loaded)>,
}

impl Default for Thumbs {
    fn default() -> Self {
        Self::new()
    }
}

impl Thumbs {
    pub fn new() -> Self {
        let (tx, req) = crossbeam_channel::unbounded::<i64>();
        let (done, rx) = crossbeam_channel::unbounded();
        let wanted: Arc<Mutex<HashSet<i64>>> = Arc::default();
        for n in 0..THUMB_WORKERS {
            let (req, done, wanted) = (req.clone(), done.clone(), wanted.clone());
            std::thread::Builder::new()
                .name(format!("koan-thumbs-{n}"))
                .spawn(move || {
                    while let Ok(album) = req.recv() {
                        let still = wanted.lock().is_ok_and(|w| w.contains(&album));
                        let out = if still {
                            Loaded::Image(load(album))
                        } else {
                            Loaded::Skipped
                        };
                        if done.send((album, out)).is_err() {
                            break;
                        }
                    }
                })
                .ok();
        }
        Self {
            images: HashMap::new(),
            asked: HashSet::new(),
            wanted,
            tx,
            rx,
        }
    }

    /// Takes in what the loaders have finished. Returns whether anything came.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((id, out)) = self.rx.try_recv() {
            self.asked.remove(&id);
            if let Loaded::Image(image) = out {
                if self.images.len() >= THUMB_LIMIT {
                    self.images.clear();
                }
                self.images.insert(id, image);
                any = true;
            }
        }
        any
    }

    /// Starts a frame: what was wanted before no longer is, until it is asked
    /// for again.
    pub(super) fn begin_frame(&self) {
        if let Ok(mut w) = self.wanted.lock() {
            w.clear();
        }
    }

    /// The cover for `album`, loading it if it is not here. `None` is either
    /// not yet or no cover.
    pub(super) fn get(&mut self, album: i64) -> Option<&DynamicImage> {
        if let Ok(mut w) = self.wanted.lock() {
            w.insert(album);
        }
        if !self.images.contains_key(&album) && self.asked.insert(album) {
            self.tx.send(album).ok();
        }
        self.images.get(&album).and_then(Option::as_ref)
    }
}

/// A record's cover as a thumbnail: from a file beside or inside one of its
/// tracks, else from the server, as the apps do. A library synced from a
/// server has no files to read.
fn load(album: i64) -> Option<DynamicImage> {
    let db = koan_core::db::pool::shared().get().ok()?;
    let row = queries::cover_track_for_album(&db.conn, album).ok()??;
    let local = row.path.as_deref().or(row.cached_path.as_deref());
    if let Some(bytes) = local.and_then(|p| koan_core::index::folder_art::cover_art(Path::new(p)))
        && let Ok(img) = image::load_from_memory(&bytes)
    {
        return Some(img.thumbnail(256, 256));
    }
    let remote_id = row.remote_id?;
    let cfg = koan_core::config::Config::cached();
    if !cfg.remote.enabled {
        return None;
    }
    let client = koan_core::helpers::subsonic_client(&cfg)?;
    let bytes = client.get_cover_art(&remote_id, Some(320)).ok()?;
    Some(image::load_from_memory(&bytes).ok()?.thumbnail(256, 256))
}

/// What the last render laid out, for the keys and the mouse to agree with it.
#[derive(Default, Clone, Copy)]
struct Geom {
    /// Below the heading and an artist's header: the records themselves.
    body: Rect,
    cols: usize,
    tile_w: u16,
    tile_h: u16,
    rows: usize,
    sort: Rect,
    view: Rect,
    filter: Rect,
    /// The scrollbar's column, when the page overflows.
    bar: Rect,
    /// Rows in all, and the fully visible ones; the scroll runs between.
    total_rows: usize,
}

pub struct Browse {
    pub source: Source,
    pub view: View,
    /// The heading, for a page about one artist.
    title: String,
    albums: Vec<AlbumRow>,
    tracks: Vec<TrackRow>,
    /// What is shown, as indices into `albums` or `tracks`: all of them, or
    /// the ones the filter keeps.
    shown: Vec<usize>,
    pub cursor: usize,
    scroll: usize,
    /// Whether the next frame scrolls to the cursor; the wheel and the
    /// scrollbar turn it off so they are not undone.
    follow: bool,
    /// The records and artists that are favourites, for the heart on a tile.
    fav_albums: HashSet<i64>,
    fav_artists: HashSet<i64>,
    header: Option<ArtistHeader>,
    order: usize,
    pub filter: String,
    pub filter_active: bool,
    loaded: bool,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
}

impl Browse {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            view: View::Tiles,
            title: String::new(),
            albums: Vec::new(),
            tracks: Vec::new(),
            shown: Vec::new(),
            cursor: 0,
            scroll: 0,
            follow: true,
            fav_albums: HashSet::new(),
            fav_artists: HashSet::new(),
            header: None,
            order: 0,
            filter: String::new(),
            filter_active: false,
            loaded: false,
            geom: Geom::default(),
            last_click: None,
        }
    }

    /// Whether the list view holds tracks (a shelf's) rather than records.
    fn tracks_in_list(&self) -> bool {
        self.view == View::List && self.source != Source::Albums
    }

    /// Reads the listing again. Cheap enough to do whenever the page opens.
    pub fn reload(&mut self) {
        let Ok(db) = koan_core::db::pool::shared().get() else {
            return;
        };
        let now = shelves::now();
        let user = queries::LOCAL_USER;
        let query = match (self.source.shelf(), self.source) {
            (Some(shelf), _) => shelf.albums(user, now),
            (None, Source::Artist(id)) => AlbumQuery {
                artist_id: Some(id),
                order: AlbumOrder::Date,
                ..Default::default()
            },
            (None, _) => AlbumQuery {
                order: ORDERS[self.order].0,
                ..Default::default()
            },
        };
        self.albums = queries::list_albums(&db.conn, &query).unwrap_or_default();
        if let Source::Artist(id) = self.source {
            if self.header.is_none() {
                self.header = Some(ArtistHeader::new(id));
            }
            self.title = self
                .albums
                .first()
                .map(|a| a.artist_name.clone())
                .unwrap_or_default();
        }
        self.tracks = match self.source.shelf() {
            Some(shelf) => shelf
                .tracks(user, now)
                .page(&db.conn, 5000, 0)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        self.loaded = true;
        self.refilter();
        self.reload_favourites();
    }

    /// Reads which records and artists are favourites.
    pub fn reload_favourites(&mut self) {
        let Ok(db) = koan_core::db::pool::shared().get() else {
            return;
        };
        self.fav_albums =
            queries::favourite_album_id_set(&db.conn, queries::LOCAL_USER).unwrap_or_default();
        self.fav_artists =
            queries::favourite_artist_id_set(&db.conn, queries::LOCAL_USER).unwrap_or_default();
        if let Some(header) = self.header.as_mut() {
            header.set_favourite(match self.source {
                Source::Artist(id) => self.fav_artists.contains(&id),
                _ => false,
            });
        }
    }

    /// Takes what the artist lookup has found; true when the page changed.
    pub fn poll(&mut self) -> bool {
        self.header.as_mut().is_some_and(ArtistHeader::poll)
    }

    /// Every track of every record shown, in the order of the records.
    fn artist_ids(&self) -> Vec<i64> {
        let Ok(db) = koan_core::db::pool::shared().get() else {
            return Vec::new();
        };
        self.albums
            .iter()
            .filter_map(|a| queries::tracks_for_album(&db.conn, a.id).ok())
            .flatten()
            .map(|t| t.id)
            .collect()
    }

    pub fn ensure_loaded(&mut self) {
        if !self.loaded {
            self.reload();
        }
    }

    fn refilter(&mut self) {
        let q = self.filter.to_lowercase();
        let keep = |haystack: &[&str]| {
            q.is_empty() || haystack.iter().any(|h| h.to_lowercase().contains(&q))
        };
        self.shown = if self.tracks_in_list() {
            (0..self.tracks.len())
                .filter(|&i| {
                    let t = &self.tracks[i];
                    keep(&[&t.title, &t.artist_name, &t.album_title])
                })
                .collect()
        } else {
            (0..self.albums.len())
                .filter(|&i| {
                    let a = &self.albums[i];
                    keep(&[&a.title, &a.artist_name])
                })
                .collect()
        };
        self.cursor = self.cursor.min(self.shown.len().saturating_sub(1));
        self.scroll = 0;
        self.follow = true;
        self.keep_cursor_visible();
    }

    pub fn len(&self) -> usize {
        self.shown.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    pub fn toggle_view(&mut self) {
        self.view = match self.view {
            View::Tiles => View::List,
            View::List => View::Tiles,
        };
        self.cursor = 0;
        self.refilter();
    }

    pub fn cycle_order(&mut self) {
        if self.source == Source::Albums {
            self.order = (self.order + 1) % ORDERS.len();
            self.reload();
        }
    }

    fn cols(&self) -> usize {
        if self.view == View::Tiles {
            self.geom.cols.max(1)
        } else {
            1
        }
    }

    fn page_rows(&self) -> usize {
        self.geom.rows.max(1)
    }

    fn keep_cursor_visible(&mut self) {
        let row = self.cursor / self.cols();
        let rows = self.page_rows();
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + rows {
            self.scroll = row + 1 - rows;
        }
    }

    fn move_to(&mut self, idx: usize) {
        self.cursor = idx.min(self.shown.len().saturating_sub(1));
        self.follow = true;
        self.keep_cursor_visible();
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.follow = false;
        let cols = self.cols();
        let total_rows = self.shown.len().div_ceil(cols);
        let max = total_rows.saturating_sub(self.page_rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    /// The ids a play of the item at `idx` would queue.
    fn ids_at(&self, idx: usize) -> Vec<i64> {
        let Some(&i) = self.shown.get(idx) else {
            return Vec::new();
        };
        if self.tracks_in_list() {
            return vec![self.tracks[i].id];
        }
        koan_core::db::pool::shared()
            .get()
            .ok()
            .and_then(|db| queries::tracks_for_album(&db.conn, self.albums[i].id).ok())
            .map(|ts| ts.iter().map(|t| t.id).collect())
            .unwrap_or_default()
    }

    pub fn append_at(&mut self, idx: usize) -> Action {
        self.move_to(idx);
        self.play(idx, PickerAction::Append)
    }

    fn play(&self, idx: usize, how: PickerAction) -> Action {
        let ids = self.ids_at(idx);
        if ids.is_empty() {
            Action::None
        } else {
            Action::Play(ids, how)
        }
    }

    /// The record the item at `idx` is, or is on.
    fn album_at(&self, idx: usize) -> Option<i64> {
        let &i = self.shown.get(idx)?;
        if self.tracks_in_list() {
            self.tracks[i].album_id
        } else {
            Some(self.albums[i].id)
        }
    }

    /// The artist the item at `idx` is by.
    fn artist_at(&self, idx: usize) -> Option<i64> {
        let &i = self.shown.get(idx)?;
        if self.tracks_in_list() {
            self.tracks[i].artist_id
        } else {
            Some(self.albums[i].artist_id)
        }
    }

    /// The track under the cursor when the page lists tracks, for hearting.
    pub fn track_at_cursor(&self) -> Option<i64> {
        if !self.tracks_in_list() {
            return None;
        }
        self.shown.get(self.cursor).map(|&i| self.tracks[i].id)
    }

    /// `None` when the key is not the page's, so the app can try it as a
    /// transport key.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Action> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.filter_active {
            match key.code {
                KeyCode::Esc => self.clear_filter(),
                KeyCode::Enter => self.filter_active = false,
                KeyCode::Backspace => {
                    if self.filter.pop().is_none() {
                        self.filter_active = false;
                    }
                    self.refilter();
                }
                KeyCode::Char(c) if !ctrl => {
                    self.filter.push(c);
                    self.refilter();
                }
                KeyCode::Up | KeyCode::Down => self.filter_active = false,
                _ => {}
            }
            return Some(Action::None);
        }
        let cols = self.cols();
        let page = self.page_rows() * cols;
        let tiles = self.view == View::Tiles;
        match key.code {
            KeyCode::Left | KeyCode::Char('h') if tiles => {
                self.move_to(self.cursor.saturating_sub(1));
            }
            KeyCode::Right | KeyCode::Char('l') if tiles => self.move_to(self.cursor + 1),
            KeyCode::Up | KeyCode::Char('k') => self.move_to(self.cursor.saturating_sub(cols)),
            KeyCode::Down | KeyCode::Char('j') => self.move_to(self.cursor + cols),
            KeyCode::PageUp => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.cursor + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(page)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + page),
            KeyCode::Home | KeyCode::Char('g') => self.move_to(0),
            KeyCode::End | KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Enter if self.tracks_in_list() => {
                return Some(self.play(self.cursor, PickerAction::AppendAndPlay));
            }
            KeyCode::Enter | KeyCode::Char('O') => {
                return Some(
                    self.album_at(self.cursor)
                        .map_or(Action::None, Action::OpenAlbum),
                );
            }
            KeyCode::Char('A') => {
                return Some(
                    self.artist_at(self.cursor)
                        .map_or(Action::None, Action::OpenArtist),
                );
            }
            KeyCode::Char('P') => return Some(self.play(self.cursor, PickerAction::AppendAndPlay)),
            KeyCode::Char('r') if ctrl => {
                return Some(self.play(self.cursor, PickerAction::ReplaceQueue));
            }
            KeyCode::Char('a') => return Some(self.play(self.cursor, PickerAction::Append)),
            KeyCode::Char('N') => return Some(self.play(self.cursor, PickerAction::PlayNext)),
            KeyCode::Char('f') if self.tracks_in_list() => {
                return Some(
                    self.track_at_cursor()
                        .map_or(Action::None, Action::Favourite),
                );
            }
            KeyCode::Char('f') => {
                return Some(
                    self.album_at(self.cursor)
                        .map_or(Action::None, Action::ToggleAlbum),
                );
            }
            KeyCode::Char('w') => {
                let ids = self.ids_at(self.cursor);
                return Some(if ids.is_empty() {
                    Action::None
                } else {
                    Action::Choose(ids)
                });
            }
            KeyCode::Char('d') => {
                let ids = self.ids_at(self.cursor);
                return Some(if ids.is_empty() {
                    Action::None
                } else {
                    Action::Download(ids)
                });
            }
            KeyCode::Char('u') => {
                let ids = self.ids_at(self.cursor);
                return Some(if ids.is_empty() {
                    Action::None
                } else {
                    Action::RemoveDownload(ids)
                });
            }
            KeyCode::Char('i') if self.tracks_in_list() => {
                return Some(self.track_at_cursor().map_or(Action::None, Action::Info));
            }
            KeyCode::Char('H') => {
                let artist = match self.source {
                    Source::Artist(id) => Some(id),
                    _ => self.artist_at(self.cursor),
                };
                return Some(artist.map_or(Action::None, Action::ToggleArtist));
            }
            KeyCode::Char('/') => self.filter_active = true,
            KeyCode::Char('v') => self.toggle_view(),
            KeyCode::Char('o') => self.cycle_order(),
            KeyCode::Esc if !self.filter.is_empty() => self.clear_filter(),
            _ => return None,
        }
        Some(Action::None)
    }

    fn clear_filter(&mut self) {
        self.filter.clear();
        self.filter_active = false;
        self.refilter();
    }

    /// What the pointer is over.
    pub fn hit(&self, col: u16, row: u16) -> Option<Hit> {
        let g = &self.geom;
        let inside = |r: Rect| col >= r.x && col < r.x + r.width && row == r.y;
        if inside(g.sort) {
            return Some(Hit::Sort);
        }
        if inside(g.view) {
            return Some(Hit::View);
        }
        if inside(g.filter) {
            return Some(Hit::Filter);
        }
        if let Some(h) = self.header.as_ref().and_then(|h| h.hit(col, row)) {
            return Some(Hit::Header(h));
        }
        let body_y = g.body.y;
        if g.body.height == 0
            || col < g.body.x
            || col >= g.body.x + g.body.width
            || row < body_y
            || row >= g.body.y + g.body.height
        {
            return None;
        }
        match self.view {
            View::Tiles => {
                let margin = (g.body.width.saturating_sub(g.tile_w * g.cols as u16)) / 2;
                if col < g.body.x + margin {
                    return None;
                }
                let tile_row = (row - body_y) / g.tile_h.max(1);
                let tile_col = (col - g.body.x - margin) / g.tile_w.max(1);
                if tile_col as usize >= g.cols {
                    return None;
                }
                let idx = (self.scroll + tile_row as usize) * g.cols + tile_col as usize;
                let &i = self.shown.get(idx)?;
                let album = &self.albums[i];
                // The label lines under the art are links, as far as their text
                // goes; the rest of the tile selects.
                let dy = (row - body_y) % g.tile_h.max(1);
                let dx = (col - g.body.x - margin) % g.tile_w.max(1);
                let art_h = g.tile_h.saturating_sub(3);
                let label = g.tile_w.saturating_sub(2) as usize;
                let on_text =
                    |text: &str| dx >= 1 && (dx as usize) <= fit(text, label).chars().count();
                if dy == art_h && on_text(&album.title) {
                    return Some(Hit::Title(idx));
                }
                if dy == art_h + 1 && on_text(&album.artist_name) {
                    let end = fit(&album.artist_name, label).chars().count();
                    if (dx as usize) <= end {
                        return Some(Hit::Artist(idx));
                    }
                }
                Some(Hit::Item(idx))
            }
            View::List => {
                let idx = self.scroll + (row - body_y) as usize;
                (idx < self.shown.len()).then_some(Hit::Item(idx))
            }
        }
    }

    /// A left click. The first selects; the second, soon after on the same
    /// item, plays it.
    pub fn click(&mut self, hit: Hit) -> Action {
        match hit {
            Hit::Sort => {
                self.cycle_order();
                Action::None
            }
            Hit::View => {
                self.toggle_view();
                Action::None
            }
            Hit::Filter => {
                self.filter_active = true;
                Action::None
            }
            Hit::Header(h) => match h {
                HeaderHit::Play | HeaderHit::Shuffle => {
                    let ids = self.artist_ids();
                    match (h, ids.is_empty()) {
                        (_, true) => Action::None,
                        (HeaderHit::Play, _) => Action::Play(ids, PickerAction::ReplaceQueue),
                        _ => Action::Shuffle(ids),
                    }
                }
                HeaderHit::Heart => match self.source {
                    Source::Artist(id) => Action::ToggleArtist(id),
                    _ => Action::None,
                },
                HeaderHit::More => {
                    if let Some(header) = self.header.as_mut() {
                        header.toggle_expanded();
                    }
                    Action::None
                }
                HeaderHit::Wikipedia => self
                    .header
                    .as_ref()
                    .and_then(ArtistHeader::url)
                    .map_or(Action::None, Action::OpenUrl),
            },
            Hit::Title(idx) => {
                self.move_to(idx);
                self.album_at(idx).map_or(Action::None, Action::OpenAlbum)
            }
            Hit::Artist(idx) => {
                self.move_to(idx);
                self.artist_at(idx).map_or(Action::None, Action::OpenArtist)
            }
            Hit::Item(idx) => {
                let double = self
                    .last_click
                    .is_some_and(|(i, at)| i == idx && at.elapsed() < DOUBLE_CLICK);
                self.move_to(idx);
                if double {
                    self.last_click = None;
                    if self.tracks_in_list() {
                        self.play(idx, PickerAction::AppendAndPlay)
                    } else {
                        self.album_at(idx).map_or(Action::None, Action::OpenAlbum)
                    }
                } else {
                    self.last_click = Some((idx, Instant::now()));
                    Action::None
                }
            }
        }
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
        hover: Option<Hit>,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.ensure_loaded();
        thumbs.begin_frame();
        self.geom = Geom::default();
        self.render_header(area, buf, theme);
        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        if body.height == 0 {
            return;
        }
        if body.width < 4 {
            return;
        }
        let mut body = body;
        if let Some(mut header) = self.header.take() {
            let h = header.height(body.width, body.height);
            if h > 0 {
                let header_hover = match hover {
                    Some(Hit::Header(h)) => Some(h),
                    _ => None,
                };
                let name = self.title.clone();
                header.render(
                    Rect::new(body.x, body.y, body.width, h),
                    buf,
                    theme,
                    gfx,
                    &name,
                    self.albums.len(),
                    header_hover,
                );
                body = Rect::new(body.x, body.y + h, body.width, body.height - h);
            }
            self.header = Some(header);
        }
        self.geom.body = body;
        if self.shown.is_empty() {
            let msg = if self.filter.is_empty() {
                self.source.noun()
            } else {
                "no match"
            };
            let text = if self.source == Source::Albums && self.filter.is_empty() {
                "no albums in the library"
            } else {
                msg
            };
            buf.set_line(
                body.x + 1,
                body.y,
                &Line::from(Span::styled(text, theme.hint_desc)),
                body.width.saturating_sub(1),
            );
            return;
        }
        match self.view {
            View::Tiles => self.render_tiles(body, buf, theme, gfx, thumbs, hover),
            View::List => self.render_list(body, buf, theme, favourites, hover),
        }
    }

    fn render_header(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let count = format!(" {} ", self.shown.len());
        let mut x = area.x;
        let end = area.x + area.width;
        let put = |buf: &mut Buffer, x: &mut u16, text: &str, style: Style| -> Rect {
            let w = (text.chars().count() as u16).min(end.saturating_sub(*x));
            let at = Rect::new(*x, area.y, w, 1);
            buf.set_stringn(*x, area.y, text, w as usize, style);
            *x += w;
            at
        };
        put(buf, &mut x, "", theme.heading);
        let title = match self.source {
            Source::Albums => "albums",
            Source::Favourites => "favourites",
            Source::Recent => "recent",
            Source::Artist(_) => self.title.as_str(),
        };
        put(buf, &mut x, title, theme.heading);
        put(buf, &mut x, &count, theme.hint_desc);
        if self.filter_active || !self.filter.is_empty() {
            let cursor = if self.filter_active { "\u{2588}" } else { "" };
            let text = format!(" /{}{} ", self.filter, cursor);
            self.geom.filter = put(buf, &mut x, &text, theme.hint_key);
        }
        // The rule fills the rest, with the page's own controls on its right.
        let mut controls = Vec::new();
        if self.source == Source::Albums {
            controls.push((format!(" sort: {} ", ORDERS[self.order].1), Hit::Sort));
        }
        controls.push((
            match (self.view, self.source) {
                (View::Tiles, _) => " view: tiles ".to_string(),
                (View::List, Source::Albums | Source::Artist(_)) => " view: rows ".to_string(),
                (View::List, _) => " view: tracks ".to_string(),
            },
            Hit::View,
        ));
        if !self.filter_active && self.filter.is_empty() {
            controls.insert(0, (" / filter ".to_string(), Hit::Filter));
        }
        let total: u16 = controls.iter().map(|(c, _)| c.chars().count() as u16).sum();
        let rule_end = end.saturating_sub(total).max(x);
        for cx in x..rule_end {
            buf[(cx, area.y)].set_char('\u{2500}').set_style(theme.rule);
        }
        x = rule_end;
        for (text, kind) in controls {
            let at = put(buf, &mut x, &text, theme.hint_desc);
            match kind {
                Hit::Sort => self.geom.sort = at,
                Hit::Filter => self.geom.filter = at,
                _ => self.geom.view = at,
            }
        }
    }

    fn render_tiles(
        &mut self,
        body: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        gfx: &mut Gfx,
        thumbs: &mut Thumbs,
        hover: Option<Hit>,
    ) {
        let cols = ((body.width / TILE_MIN_W) as usize).max(1);
        let tile_w = (body.width / cols as u16).min(TILE_MAX_W);
        // A cell is about twice as tall as wide, so art of `w` columns is
        // square at `w / 2` rows. Two lines of label under it, and a gap.
        let art_w = tile_w.saturating_sub(2).max(1);
        let art_h = (art_w / 2).max(1);
        let tile_h = art_h + 3;
        // Rows that fit whole, which paging and the cursor keep to; the one
        // after is drawn as far as it reaches.
        let rows = ((body.height / tile_h) as usize).max(1);
        self.geom.cols = cols;
        self.geom.tile_w = tile_w;
        self.geom.tile_h = tile_h;
        self.geom.rows = rows;
        // Left over width is split into a margin so the grid sits centred.
        let margin = (body.width - tile_w * cols as u16) / 2;
        if self.follow {
            self.keep_cursor_visible();
        }
        let total_rows = self.shown.len().div_ceil(cols);
        self.scroll = self.scroll.min(total_rows.saturating_sub(rows));
        let bottom = body.y + body.height;

        for r in 0..=rows {
            for c in 0..cols {
                let idx = (self.scroll + r) * cols + c;
                let Some(&i) = self.shown.get(idx) else { break };
                let album = &self.albums[i];
                let x = body.x + margin + c as u16 * tile_w;
                let y = body.y + r as u16 * tile_h;
                if y >= bottom {
                    continue;
                }
                let selected = idx == self.cursor;
                let art = Rect::new(x + 1, y, art_w, art_h);
                let art_visible = (bottom - y).min(art_h);
                if let Some(img) = thumbs.get(album.id) {
                    gfx.draw_clipped(album_key(album.id), img, art, art_visible, buf);
                } else {
                    placeholder(art, art_visible, buf, theme, &album.title);
                }
                if selected {
                    for dy in 0..art_visible {
                        buf[(x, y + dy)].set_char('\u{258c}').set_style(theme.bar);
                    }
                }
                let label = tile_w.saturating_sub(2) as usize;
                let title_y = y + art_h;
                if title_y < bottom {
                    let style = if selected {
                        theme.track_cursor
                    } else {
                        theme.track_normal
                    };
                    let style = if hover == Some(Hit::Title(idx)) {
                        style.add_modifier(Modifier::UNDERLINED)
                    } else {
                        style
                    };
                    if self.fav_albums.contains(&album.id) && label > 3 {
                        buf.set_string(x + 1, title_y, "\u{2665}", theme.favourite);
                        buf.set_stringn(
                            x + 3,
                            title_y,
                            fit(&album.title, label - 2),
                            label - 2,
                            style,
                        );
                    } else {
                        buf.set_stringn(x + 1, title_y, fit(&album.title, label), label, style);
                    }
                }
                if title_y + 1 < bottom {
                    // The artist is a link; the year after it is not.
                    let artist = fit(&album.artist_name, label);
                    let used = artist.chars().count();
                    let style = if selected {
                        theme.track_playing
                    } else {
                        theme.hint_desc
                    };
                    let style = if hover == Some(Hit::Artist(idx)) {
                        style.add_modifier(Modifier::UNDERLINED)
                    } else {
                        style
                    };
                    buf.set_stringn(x + 1, title_y + 1, &artist, label, style);
                    let year = queries_year(album);
                    if !year.is_empty() && used + 3 + year.len() <= label {
                        buf.set_stringn(
                            x + 1 + used as u16,
                            title_y + 1,
                            format!(" \u{b7} {year}"),
                            label - used,
                            theme.hint_desc,
                        );
                    }
                }
            }
        }
        self.scrollbar(body, buf, theme, total_rows, rows);
    }

    fn render_list(
        &mut self,
        body: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        favourites: &HashSet<i64>,
        hover: Option<Hit>,
    ) {
        let rows = body.height as usize;
        self.geom.cols = 1;
        self.geom.tile_w = body.width;
        self.geom.tile_h = 1;
        self.geom.rows = rows;
        if self.follow {
            self.keep_cursor_visible();
        }
        self.scroll = self.scroll.min(self.shown.len().saturating_sub(rows));
        let tracks = self.tracks_in_list();
        for r in 0..rows {
            let idx = self.scroll + r;
            let Some(&i) = self.shown.get(idx) else { break };
            let y = body.y + r as u16;
            let selected = idx == self.cursor;
            let hovered = hover == Some(Hit::Item(idx));
            let (name, detail, tail, fav) = if tracks {
                let t = &self.tracks[i];
                (
                    t.title.clone(),
                    format!("{} \u{b7} {}", t.artist_name, t.album_title),
                    t.duration_ms
                        .map(|d| format_time(d as u64))
                        .unwrap_or_default(),
                    favourites.contains(&t.id),
                )
            } else {
                let a = &self.albums[i];
                let year = queries_year(a);
                (
                    a.title.clone(),
                    a.artist_name.clone(),
                    [year, a.codec.clone().unwrap_or_default()]
                        .iter()
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("  "),
                    false,
                )
            };
            let (name_style, rest_style) = if selected {
                (theme.library_cursor, theme.library_cursor)
            } else if hovered {
                (theme.library_hover, theme.hint_desc)
            } else {
                (theme.track_normal, theme.hint_desc)
            };
            if selected {
                for cx in body.x..body.x + body.width {
                    buf[(cx, y)].set_style(theme.library_cursor);
                }
                buf[(body.x, y)].set_char('\u{258c}').set_style(theme.bar);
            }
            let w = body.width as usize;
            let tail_w = tail.chars().count();
            let gutter = if fav { "\u{2665}" } else { " " };
            buf.set_stringn(
                body.x + 1,
                y,
                gutter,
                1,
                if selected {
                    theme.library_cursor
                } else {
                    theme.favourite
                },
            );
            let name_w = (w.saturating_sub(tail_w + 4) * 3 / 5)
                .max(8)
                .min(w.saturating_sub(4));
            buf.set_stringn(body.x + 3, y, &name, name_w, name_style);
            let detail_x = body.x + 3 + name_w as u16 + 2;
            let room = (body.x + body.width).saturating_sub(detail_x + tail_w as u16 + 2) as usize;
            if room > 0 {
                buf.set_stringn(detail_x, y, &detail, room, rest_style);
            }
            if tail_w > 0 && tail_w + 1 < w {
                buf.set_stringn(
                    body.x + body.width - tail_w as u16 - 1,
                    y,
                    &tail,
                    tail_w,
                    rest_style,
                );
            }
        }
        self.scrollbar(body, buf, theme, self.shown.len(), rows);
    }

    fn scrollbar(
        &mut self,
        body: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        total: usize,
        visible: usize,
    ) {
        if total <= visible || body.height == 0 || body.width < 6 {
            return;
        }
        let x = body.x + body.width - 1;
        let h = body.height as usize;
        // Two columns drawn and three to hit: a thumb one cell wide is hard
        // to land on with a pointer.
        self.geom.bar = Rect::new(x - 2, body.y, 3, body.height);
        self.geom.total_rows = total;
        let thumb = (h * visible / total).max(1);
        let top = (h - thumb) * self.scroll / (total - visible).max(1);
        for row in 0..h {
            let on = row >= top && row < top + thumb;
            for cx in [x - 1, x] {
                buf[(cx, body.y + row as u16)]
                    .set_char(if on { '\u{2588}' } else { ' ' })
                    .set_style(if on {
                        Style::new().fg(theme.scrollbar_thumb)
                    } else {
                        Style::new().bg(theme.scrollbar_bg)
                    });
            }
        }
    }

    /// Whether `col`, `row` is on the scrollbar.
    pub fn on_scrollbar(&self, col: u16, row: u16) -> bool {
        let b = self.geom.bar;
        b.width > 0 && col >= b.x && col < b.x + b.width && row >= b.y && row < b.y + b.height
    }

    /// Scrolls so the thumb is at `row`, for a click or a drag on the bar.
    pub fn scroll_to_bar(&mut self, row: u16) {
        self.follow = false;
        let b = self.geom.bar;
        if b.height == 0 {
            return;
        }
        let max = self.geom.total_rows.saturating_sub(self.page_rows());
        let frac = row.saturating_sub(b.y) as f64 / (b.height.saturating_sub(1)).max(1) as f64;
        self.scroll = ((frac.clamp(0.0, 1.0) * max as f64).round() as usize).min(max);
    }
}

/// `text` cut to `width` cells, with an ellipsis where it was cut.
pub(super) fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('\u{2026}');
    cut
}

fn queries_year(album: &AlbumRow) -> String {
    super::library::album_year(album.date.as_deref()).unwrap_or_default()
}

/// A tile with no cover (yet): the record's first letter on the raised
/// ground, so the grid keeps its shape while covers arrive.
pub(super) fn placeholder(area: Rect, visible: u16, buf: &mut Buffer, theme: &Theme, title: &str) {
    let ground = Style::new().bg(theme.tokens.surface);
    for y in area.y..area.y + visible.min(area.height) {
        for x in area.x..area.x + area.width {
            buf[(x, y)].set_char(' ').set_style(ground);
        }
    }
    let letter = title
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "\u{266a}".into());
    if area.height / 2 < visible {
        buf.set_string(
            area.x + area.width / 2,
            area.y + area.height / 2,
            letter,
            ground.fg(theme.tokens.muted).add_modifier(Modifier::BOLD),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: i64, title: &str, artist: &str) -> AlbumRow {
        AlbumRow {
            id,
            title: title.into(),
            artist_id: id,
            artist_name: artist.into(),
            date: Some("1998-04-20".into()),
            total_discs: None,
            total_tracks: None,
            codec: Some("FLAC".into()),
            label: None,
            remote_id: None,
            added_at: None,
            on_device: None,
        }
    }

    fn browse(n: usize) -> Browse {
        koan_core::config::isolate_config_for_tests();
        let mut b = Browse::new(Source::Albums);
        b.albums = (0..n)
            .map(|i| album(i as i64, &format!("record {i}"), "artist"))
            .collect();
        b.loaded = true;
        b.refilter();
        b
    }

    fn draw(b: &mut Browse, w: u16, h: u16) {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        b.render(
            area,
            &mut buf,
            &theme,
            &mut Gfx::halfblocks(),
            &mut Thumbs::new(),
            &HashSet::new(),
            None,
        );
    }

    #[test]
    fn tiles_fill_the_width_and_keys_move_by_row() {
        let mut b = browse(40);
        draw(&mut b, 80, 30);
        assert!(b.geom.cols >= 3);
        let cols = b.geom.cols;
        b.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Down,
        ));
        assert_eq!(b.cursor, cols);
        b.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Right,
        ));
        assert_eq!(b.cursor, cols + 1);
    }

    #[test]
    fn the_cursor_is_kept_on_screen() {
        let mut b = browse(200);
        draw(&mut b, 60, 20);
        b.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::End,
        ));
        draw(&mut b, 60, 20);
        let row = b.cursor / b.geom.cols;
        assert!(row >= b.scroll && row < b.scroll + b.geom.rows);
    }

    /// The cell at `dx`, `dy` inside the first tile.
    fn first_tile(b: &Browse, dx: u16, dy: u16) -> (u16, u16) {
        let g = &b.geom;
        let margin = g.body.width.saturating_sub(g.tile_w * g.cols as u16) / 2;
        (g.body.x + margin + dx, g.body.y + dy)
    }

    #[test]
    fn a_click_selects_and_the_second_opens_the_record() {
        let mut b = browse(10);
        draw(&mut b, 80, 30);
        let (x, y) = first_tile(&b, 2, 1);
        let hit = b.hit(x, y).expect("a tile");
        assert_eq!(hit, Hit::Item(0));
        assert_eq!(b.click(hit), Action::None);
        assert_eq!(b.click(hit), Action::OpenAlbum(0));
        assert!(b.last_click.is_none());
    }

    #[test]
    fn the_title_and_artist_under_a_tile_are_links() {
        let mut b = browse(10);
        draw(&mut b, 80, 30);
        let art_h = b.geom.tile_h - 3;
        let (x, y) = first_tile(&b, 2, art_h);
        assert_eq!(b.hit(x, y), Some(Hit::Title(0)));
        assert_eq!(b.click(Hit::Title(0)), Action::OpenAlbum(0));
        let (x, y) = first_tile(&b, 2, art_h + 1);
        assert_eq!(b.hit(x, y), Some(Hit::Artist(0)));
        assert_eq!(b.click(Hit::Artist(0)), Action::OpenArtist(0));
        // Past the end of the text it is the tile again.
        let (x, y) = first_tile(&b, b.geom.tile_w - 1, art_h);
        assert_eq!(b.hit(x, y), Some(Hit::Item(0)));
    }

    #[test]
    fn f_favourites_the_record_and_h_its_artist() {
        let mut b = browse(3);
        draw(&mut b, 80, 24);
        b.move_to(1);
        let key = |c| crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(c));
        assert_eq!(b.handle_key(key('f')), Some(Action::ToggleAlbum(1)));
        assert_eq!(b.handle_key(key('H')), Some(Action::ToggleArtist(1)));
    }

    #[test]
    fn a_favourite_record_wears_a_heart() {
        let mut b = browse(3);
        b.fav_albums.insert(0);
        draw(&mut b, 80, 24);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        b.render(
            area,
            &mut buf,
            &theme,
            &mut Gfx::halfblocks(),
            &mut Thumbs::new(),
            &HashSet::new(),
            None,
        );
        let shown: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(shown.contains('\u{2665}'));
    }

    #[test]
    fn the_wheel_is_not_undone_by_the_next_frame() {
        let mut b = browse(200);
        draw(&mut b, 80, 24);
        b.scroll_by(2);
        draw(&mut b, 80, 24);
        assert_eq!(b.scroll, 2);
        b.move_to(0);
        draw(&mut b, 80, 24);
        assert_eq!(b.scroll, 0);
    }

    #[test]
    fn the_scrollbar_is_a_handle() {
        let mut b = browse(400);
        draw(&mut b, 80, 30);
        let bar = b.geom.bar;
        assert!(b.on_scrollbar(bar.x, bar.y));
        assert!(!b.on_scrollbar(bar.x - 1, bar.y));
        b.scroll_to_bar(bar.y + bar.height - 1);
        let max = b.geom.total_rows - b.page_rows();
        assert_eq!(b.scroll, max);
        b.scroll_to_bar(bar.y);
        assert_eq!(b.scroll, 0);
    }

    #[test]
    fn a_row_that_does_not_fit_is_drawn_as_far_as_it_reaches() {
        let mut b = browse(40);
        // Tall enough for a whole row and a bit more.
        draw(&mut b, 80, 17);
        let art_h = b.geom.tile_h - 3;
        let area = Rect::new(0, 0, 80, 17);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        b.render(
            area,
            &mut buf,
            &theme,
            &mut Gfx::halfblocks(),
            &mut Thumbs::new(),
            &HashSet::new(),
            None,
        );
        // The second row starts below the first and is cut by the bottom edge:
        // its placeholder is drawn on the rows that exist.
        let second_row_y = 1 + b.geom.tile_h;
        assert!(second_row_y + 1 < 17, "a second row begins on screen");
        let drawn =
            (second_row_y..17).any(|y| (0..80).any(|x| buf[(x, y)].bg == theme.tokens.surface));
        assert!(drawn, "art_h {art_h}");
    }

    #[test]
    fn long_titles_end_in_an_ellipsis() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("a longer title", 8), "a longe\u{2026}");
        assert_eq!(fit("anything", 0), "\u{2026}");
    }

    #[test]
    fn the_filter_narrows_what_is_shown() {
        let mut b = browse(12);
        b.filter = "record 1".into();
        b.refilter();
        assert_eq!(b.len(), 3);
        b.filter = "zzz".into();
        b.refilter();
        assert!(b.is_empty());
    }

    #[test]
    fn it_draws_at_any_size() {
        let mut b = browse(30);
        for view in [View::Tiles, View::List] {
            b.view = view;
            for w in [1u16, 2, 5, 13, 14, 20, 60, 200] {
                for h in [1u16, 2, 3, 5, 12, 40] {
                    draw(&mut b, w, h);
                }
            }
        }
    }
}
