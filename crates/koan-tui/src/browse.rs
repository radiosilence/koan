//! The browsing pages: Albums, Favourites and Recent. One page type with a
//! source, because they are the library's own listings with a filter on
//! (`koan_core::shelves`); what each holds and in what order is decided there.
//! Records are shown as tiles of cover art, or as rows.
//!
//! Listings are read off the UI thread, narrowed and ordered in SQL, and
//! arrive whole; a read asked for after another always wins over it.

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
use super::browse_filter::Filters;
use super::cover_art::{Gfx, album_key};
use super::theme::Theme;
use super::thumb_cache;
use super::transport::format_time;

/// The widest a tile gets, in columns. Wider screens get more tiles.
const TILE_MAX_W: u16 = 26;
const TILE_MIN_W: u16 = 14;

/// Decoded covers kept for tiles before the lot is dropped.
const THUMB_LIMIT: usize = 800;

/// How long the grid must be still before its covers are drawn as images.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

/// How many rows beyond the page are loaded ahead of scrolling.
const PREFETCH_ROWS: usize = 3;

/// A second click this soon after the first is a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// Columns the scrollbar takes from the right of a page that overflows: two
/// drawn and one of gap.
const BAR_W: u16 = 3;

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

    fn empty(self) -> &'static str {
        match self {
            Source::Albums => "no albums in the library",
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
    /// A menu of actions for what the cursor is on, with its heading.
    Menu(String, Vec<(String, Action)>),
    /// Open the panel of filters.
    OpenFilters,
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
    Filters,
    Menu,
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
                if self.images.len() >= THUMB_LIMIT
                    && let Ok(wanted) = self.wanted.lock()
                {
                    self.images.retain(|id, _| wanted.contains(id));
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
        self.want(album);
        self.images.get(&album).and_then(Option::as_ref)
    }

    /// Starts loading `album`'s cover without drawing it, for tiles about to
    /// scroll into view.
    pub(super) fn want(&mut self, album: i64) {
        if let Ok(mut w) = self.wanted.lock() {
            w.insert(album);
        }
        if !self.images.contains_key(&album) && self.asked.insert(album) {
            self.tx.send(album).ok();
        }
    }
}

/// A record's cover as a thumbnail, from the disk cache when it has one, else
/// from a file beside or inside one of its tracks or from the server, as the
/// apps do. A library synced from a server has no files to read.
fn load(album: i64) -> Option<DynamicImage> {
    let (row, uid) = {
        let db = koan_core::db::pool::shared().get().ok()?;
        let row = queries::cover_track_for_album(&db.conn, album).ok()??;
        let uid = queries::uids::uids_for(&db.conn, queries::uids::UidKind::Album, [album])
            .ok()
            .and_then(|mut uids| uids.remove(&album));
        (row, uid)
    };
    let local = row.path.as_deref().or(row.cached_path.as_deref());
    let stamp = thumb_cache::stamp(local.map(Path::new));
    if let Some(uid) = &uid
        && let Some(img) = thumb_cache::read(uid, stamp)
    {
        return Some(img);
    }
    // A JPEG keeps no transparency, so the first load looks as every later one.
    let img = DynamicImage::ImageRgb8(fetch(&row, local)?.to_rgb8());
    if let Some(uid) = &uid {
        thumb_cache::write(uid, stamp, &img);
    }
    Some(img)
}

fn fetch(row: &queries::TrackRow, local: Option<&str>) -> Option<DynamicImage> {
    if let Some(bytes) = local.and_then(|p| koan_core::index::folder_art::cover_art(Path::new(p)))
        && let Ok(img) = image::load_from_memory(&bytes)
    {
        return Some(img.thumbnail(256, 256));
    }
    let remote_id = row.remote_id.as_deref()?;
    let cfg = koan_core::config::Config::cached();
    if !cfg.remote.enabled {
        return None;
    }
    let client = koan_core::helpers::subsonic_client(&cfg)?;
    let bytes = client.get_cover_art(remote_id, Some(320)).ok()?;
    Some(image::load_from_memory(&bytes).ok()?.thumbnail(256, 256))
}

/// The tracks of `albums`, record by record in the order given, in one read.
pub(super) fn album_tracks(albums: &[i64]) -> Vec<i64> {
    let Ok(db) = koan_core::db::pool::shared().get() else {
        return Vec::new();
    };
    let mut by_album = queries::tracks_for_albums(&db.conn, albums).unwrap_or_default();
    albums
        .iter()
        .filter_map(|a| by_album.remove(a))
        .flatten()
        .map(|t| t.id)
        .collect()
}

/// Everything a listing is read by.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ask {
    source: Source,
    order: AlbumOrder,
    /// The panel's filters, on the pages they narrow.
    filters: Option<Filters>,
    /// The name filter, matched by the database as the apps' boxes are.
    text: String,
}

/// A listing as the database answered it.
#[derive(Default)]
struct Listing {
    albums: Vec<AlbumRow>,
    /// A shelf's tracks, for its list view.
    tracks: Vec<TrackRow>,
    /// The artist's name, on an artist's page.
    artist: Option<String>,
}

fn read(ask: &Ask) -> Listing {
    let Ok(db) = koan_core::db::pool::shared().get() else {
        return Listing::default();
    };
    let now = shelves::now();
    let user = queries::LOCAL_USER;
    let offline = koan_core::remote::offline::active();
    let text = Some(ask.text.trim()).filter(|t| !t.is_empty());
    let mut query = match (ask.source.shelf(), ask.source) {
        (Some(shelf), _) => shelf.albums(user, now),
        (None, Source::Artist(id)) => AlbumQuery {
            artist_id: Some(id),
            order: AlbumOrder::Date,
            ..Default::default()
        },
        (None, _) => AlbumQuery {
            order: ask.order,
            ..Default::default()
        },
    };
    if let Some(filters) = &ask.filters {
        query.filter = filters.album_filter();
    }
    query.filter.on_device |= offline;
    query.search = text;
    let tracks = ask.source.shelf().map(|shelf| {
        let mut listing = shelf.tracks(user, now);
        listing.filter.on_device |= offline;
        listing.filter.search = text.map(str::to_owned);
        listing.page(&db.conn, u32::MAX, 0).unwrap_or_default()
    });
    let artist = match ask.source {
        Source::Artist(id) => queries::get_artist(&db.conn, id)
            .ok()
            .flatten()
            .map(|a| a.name),
        _ => None,
    };
    Listing {
        albums: queries::list_albums(&db.conn, &query).unwrap_or_default(),
        tracks: tracks.unwrap_or_default(),
        artist,
    }
}

/// Asks for listings and the answers, each tagged with its generation.
type Reader = (Sender<(u64, Ask)>, Receiver<(u64, Listing)>);

/// Reads listings on a thread of the page's own, the latest ask of a burst
/// only, so typing a filter queues no reads for the letters already passed.
fn spawn_reader() -> Reader {
    let (tx, req) = crossbeam_channel::unbounded::<(u64, Ask)>();
    let (done, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("koan-browse".into())
        .spawn(move || {
            while let Ok(mut next) = req.recv() {
                while let Ok(newer) = req.try_recv() {
                    next = newer;
                }
                let (generation, ask) = next;
                if done.send((generation, read(&ask))).is_err() {
                    return;
                }
            }
        })
        .ok();
    (tx, rx)
}

/// What the last render laid out, for the keys and the mouse to agree with it.
#[derive(Default, Clone, Copy)]
struct Geom {
    /// Below the heading and an artist's header: the records themselves,
    /// left of the scrollbar.
    body: Rect,
    cols: usize,
    tile_w: u16,
    tile_h: u16,
    rows: usize,
    sort: Rect,
    view: Rect,
    filter: Rect,
    filters: Rect,
    menu: Rect,
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
    cursor: usize,
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
    /// What the filters panel has narrowed the records by.
    filters: Filters,
    /// The last read asked for and the last one shown; they differ while a
    /// read is under way, and a listing from any but the last is dropped.
    asked: u64,
    shown: u64,
    /// Whether the listing is to be read again before it is next drawn.
    stale: bool,
    /// Whether the next listing to arrive starts at the top, as it does when
    /// the name filter changes; otherwise the cursor stays on its item.
    to_top: bool,
    tx: Sender<(u64, Ask)>,
    rx: Receiver<(u64, Listing)>,
    geom: Geom,
    last_click: Option<(usize, Instant)>,
    /// The scroll the last frame drew, and when it last changed: covers are
    /// drawn as images only once the grid has stopped.
    drawn_scroll: usize,
    moved: Option<Instant>,
}

impl Browse {
    pub fn new(source: Source) -> Self {
        let (tx, rx) = spawn_reader();
        Self {
            source,
            view: View::Tiles,
            title: String::new(),
            albums: Vec::new(),
            tracks: Vec::new(),
            cursor: 0,
            scroll: 0,
            follow: true,
            fav_albums: HashSet::new(),
            fav_artists: HashSet::new(),
            header: None,
            order: 0,
            filter: String::new(),
            filter_active: false,
            filters: Filters::default(),
            asked: 0,
            shown: 0,
            stale: true,
            to_top: false,
            tx,
            rx,
            geom: Geom::default(),
            last_click: None,
            drawn_scroll: 0,
            moved: None,
        }
    }

    /// Whether the list view holds tracks (a shelf's) rather than records.
    fn tracks_in_list(&self) -> bool {
        self.view == View::List && self.source.shelf().is_some()
    }

    /// Whether the filters panel narrows this page: the records of the
    /// library, not one artist's and not a shelf's tracks.
    pub fn filterable(&self) -> bool {
        !matches!(self.source, Source::Artist(_)) && !self.tracks_in_list()
    }

    pub fn set_filters(&mut self, filters: &Filters) {
        if self.filters != *filters {
            self.filters = filters.clone();
            self.stale = true;
        }
    }

    /// Asks for the listing again, keeping the cursor on what it is on.
    /// Cheap enough to do whenever the page opens.
    pub fn reload(&mut self) {
        self.stale = false;
        if let Source::Artist(id) = self.source
            && self.header.is_none()
        {
            self.header = Some(ArtistHeader::new(id));
        }
        self.asked += 1;
        let ask = Ask {
            source: self.source,
            order: ORDERS[self.order].0,
            filters: (!matches!(self.source, Source::Artist(_))).then(|| self.filters.clone()),
            text: self.filter.clone(),
        };
        self.tx.send((self.asked, ask)).ok();
        self.reload_favourites();
    }

    pub fn ensure_loaded(&mut self) {
        if self.stale {
            self.reload();
        }
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
        if let (Some(header), Source::Artist(id)) = (self.header.as_mut(), self.source) {
            header.set_favourite(self.fav_artists.contains(&id));
        }
    }

    /// Takes in a listing that has arrived and what the artist lookup has
    /// found; true when the page changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = self.header.as_mut().is_some_and(ArtistHeader::poll);
        while let Ok((generation, listing)) = self.rx.try_recv() {
            changed |= self.show(generation, listing);
        }
        changed
    }

    /// Shows listing `generation`, unless a newer one was asked for.
    fn show(&mut self, generation: u64, listing: Listing) -> bool {
        if generation != self.asked {
            return false;
        }
        let on = self.cursor_id();
        self.shown = generation;
        self.albums = listing.albums;
        self.tracks = listing.tracks;
        if let Some(name) = listing.artist {
            self.title = name;
        }
        if let Some(header) = self.header.as_mut() {
            header.set_artist(&self.title, self.albums.len());
        }
        self.last_click = None;
        let found = on.and_then(|id| self.position(id));
        if std::mem::take(&mut self.to_top) {
            self.cursor = 0;
            self.scroll = 0;
            self.follow = true;
        } else if let Some(at) = found {
            self.cursor = at;
            self.follow = true;
        } else {
            self.cursor = self.cursor.min(self.len().saturating_sub(1));
        }
        true
    }

    /// The id of the record or track under the cursor.
    fn cursor_id(&self) -> Option<i64> {
        if self.tracks_in_list() {
            self.tracks.get(self.cursor).map(|t| t.id)
        } else {
            self.albums.get(self.cursor).map(|a| a.id)
        }
    }

    fn position(&self, id: i64) -> Option<usize> {
        if self.tracks_in_list() {
            self.tracks.iter().position(|t| t.id == id)
        } else {
            self.albums.iter().position(|a| a.id == id)
        }
    }

    /// Whether a read is under way and nothing has been shown yet.
    fn loading(&self) -> bool {
        self.shown == 0 && (self.asked > 0 || self.stale)
    }

    /// Changes the name filter, and reads the listing it narrows to.
    fn set_text(&mut self, text: String) {
        if text != self.filter {
            self.filter = text;
            self.to_top = true;
            self.reload();
        }
    }

    pub fn len(&self) -> usize {
        if self.tracks_in_list() {
            self.tracks.len()
        } else {
            self.albums.len()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Switches between tiles and rows. The cursor stays on its record, or
    /// on a shelf, where rows are tracks, goes to the first track of it.
    pub fn toggle_view(&mut self) {
        let album = self.album_at(self.cursor);
        self.view = match self.view {
            View::Tiles => View::List,
            View::List => View::Tiles,
        };
        let at = if self.tracks_in_list() {
            self.tracks.iter().position(|t| t.album_id == album)
        } else {
            album.and_then(|id| self.albums.iter().position(|a| a.id == id))
        };
        self.scroll = 0;
        self.move_to(at.unwrap_or(0));
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
        self.cursor = idx.min(self.len().saturating_sub(1));
        self.follow = true;
        self.keep_cursor_visible();
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.follow = false;
        let total_rows = self.len().div_ceil(self.cols());
        let max = total_rows.saturating_sub(self.page_rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    /// The ids a play of the item at `idx` would queue.
    fn ids_at(&self, idx: usize) -> Vec<i64> {
        if self.tracks_in_list() {
            return self.tracks.get(idx).map(|t| vec![t.id]).unwrap_or_default();
        }
        self.albums
            .get(idx)
            .map(|a| album_tracks(&[a.id]))
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
        if self.tracks_in_list() {
            self.tracks.get(idx)?.album_id
        } else {
            Some(self.albums.get(idx)?.id)
        }
    }

    /// The artist the item at `idx` is by.
    fn artist_at(&self, idx: usize) -> Option<i64> {
        if self.tracks_in_list() {
            self.tracks.get(idx)?.artist_id
        } else {
            Some(self.albums.get(idx)?.artist_id)
        }
    }

    /// The track under the cursor when the page lists tracks, for hearting.
    pub fn track_at_cursor(&self) -> Option<i64> {
        if !self.tracks_in_list() {
            return None;
        }
        self.tracks.get(self.cursor).map(|t| t.id)
    }

    /// What can be done with the record or track at `idx`, as a menu.
    fn menu(&self, idx: usize) -> Action {
        use PickerAction::{Append, AppendAndPlay, PlayNext, ReplaceQueue};
        let ids = self.ids_at(idx);
        if ids.is_empty() {
            return Action::None;
        }
        let play = |label: &str, how| (label.to_string(), Action::Play(ids.clone(), how));
        let mut items = Vec::new();
        if self.tracks_in_list() {
            let t = &self.tracks[idx];
            items.extend([
                play("Play", AppendAndPlay),
                play("Add to the queue", Append),
                play("Play next", PlayNext),
                play("Replace the queue", ReplaceQueue),
                (
                    "Add to a playlist\u{2026}".into(),
                    Action::Choose(ids.clone()),
                ),
                ("Favourite or unfavourite".into(), Action::Favourite(t.id)),
            ]);
            if let Some(a) = t.album_id {
                items.push(("Go to the record".into(), Action::OpenAlbum(a)));
            }
            if let Some(a) = t.artist_id {
                items.push(("Go to the artist".into(), Action::OpenArtist(a)));
            }
            items.push(("Track info".into(), Action::Info(t.id)));
            items.push(("Download".into(), Action::Download(ids.clone())));
            items.push((
                "Remove the download".into(),
                Action::RemoveDownload(ids.clone()),
            ));
            return Action::Menu(t.title.clone(), items);
        }
        let a = &self.albums[idx];
        let fav = |on: bool, noun: &str| {
            format!(
                "{} the {noun}",
                if on { "Unfavourite" } else { "Favourite" }
            )
        };
        items.extend([
            ("Open the record".into(), Action::OpenAlbum(a.id)),
            play("Play", AppendAndPlay),
            play("Add to the queue", Append),
            play("Play next", PlayNext),
            play("Replace the queue", ReplaceQueue),
            (
                "Add to a playlist\u{2026}".into(),
                Action::Choose(ids.clone()),
            ),
            (
                fav(self.fav_albums.contains(&a.id), "record"),
                Action::ToggleAlbum(a.id),
            ),
            (
                fav(self.fav_artists.contains(&a.artist_id), "artist"),
                Action::ToggleArtist(a.artist_id),
            ),
            ("Go to the artist".into(), Action::OpenArtist(a.artist_id)),
            ("Download".into(), Action::Download(ids.clone())),
            (
                "Remove the download".into(),
                Action::RemoveDownload(ids.clone()),
            ),
        ]);
        Action::Menu(a.title.clone(), items)
    }

    /// `None` when the key is not the page's, so the app can try it as a
    /// transport key.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Action> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.filter_active {
            match key.code {
                KeyCode::Esc => self.clear_filter(),
                KeyCode::Enter | KeyCode::Up | KeyCode::Down => self.filter_active = false,
                KeyCode::Backspace if self.filter.is_empty() => self.filter_active = false,
                KeyCode::Backspace => {
                    let mut text = self.filter.clone();
                    text.pop();
                    self.set_text(text);
                }
                KeyCode::Char(c) if !ctrl => self.set_text(format!("{}{c}", self.filter)),
                _ => {}
            }
            return Some(Action::None);
        }
        let cols = self.cols();
        let page = self.page_rows() * cols;
        let tiles = self.view == View::Tiles;
        let ids = |b: &Self, make: fn(Vec<i64>) -> Action| {
            let ids = b.ids_at(b.cursor);
            if ids.is_empty() {
                Action::None
            } else {
                make(ids)
            }
        };
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
            KeyCode::Char('f') if ctrl => {
                return Some(if self.filterable() {
                    Action::OpenFilters
                } else {
                    Action::None
                });
            }
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
            KeyCode::Char('m') => return Some(self.menu(self.cursor)),
            KeyCode::Char('w') => return Some(ids(self, Action::Choose)),
            KeyCode::Char('d') => return Some(ids(self, Action::Download)),
            KeyCode::Char('u') => return Some(ids(self, Action::RemoveDownload)),
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
        self.filter_active = false;
        self.set_text(String::new());
    }

    /// Where a tile's title starts, after the heart a favourite wears, and
    /// how many cells it may take.
    fn title_span(&self, album: &AlbumRow, label: usize) -> (usize, usize) {
        if self.fav_albums.contains(&album.id) && label > 3 {
            (3, label - 2)
        } else {
            (1, label)
        }
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
        if inside(g.filters) {
            return Some(Hit::Filters);
        }
        if inside(g.menu) {
            return Some(Hit::Menu);
        }
        if let Some(h) = self.header.as_ref().and_then(|h| h.hit(col, row)) {
            return Some(Hit::Header(h));
        }
        let body = g.body;
        if body.height == 0
            || col < body.x
            || col >= body.x + body.width
            || row < body.y
            || row >= body.y + body.height
        {
            return None;
        }
        match self.view {
            View::Tiles => {
                let tile_w = g.tile_w.max(1);
                let tile_h = g.tile_h.max(1);
                let margin = (body.width.saturating_sub(g.tile_w * g.cols as u16)) / 2;
                if col < body.x + margin {
                    return None;
                }
                let tile_col = (col - body.x - margin) / tile_w;
                if tile_col as usize >= g.cols {
                    return None;
                }
                let idx =
                    (self.scroll + ((row - body.y) / tile_h) as usize) * g.cols + tile_col as usize;
                let album = self.albums.get(idx)?;
                // The label lines under the art are links, as far as their text
                // goes; the rest of the tile selects.
                let dy = (row - body.y) % tile_h;
                let dx = ((col - body.x - margin) % tile_w) as usize;
                let art_h = g.tile_h.saturating_sub(3);
                let label = g.tile_w.saturating_sub(2) as usize;
                let on = |from: usize, text: &str, width: usize| {
                    dx >= from && dx < from + fit(text, width).chars().count()
                };
                let (from, width) = self.title_span(album, label);
                if dy == art_h && on(from, &album.title, width) {
                    return Some(Hit::Title(idx));
                }
                if dy == art_h + 1 && on(1, &album.artist_name, label) {
                    return Some(Hit::Artist(idx));
                }
                Some(Hit::Item(idx))
            }
            View::List => {
                let idx = self.scroll + (row - body.y) as usize;
                (idx < self.len()).then_some(Hit::Item(idx))
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
            Hit::Filters => Action::OpenFilters,
            Hit::Menu => self.menu(self.cursor),
            Hit::Header(h) => match h {
                HeaderHit::Play | HeaderHit::Shuffle => {
                    let records: Vec<i64> = self.albums.iter().map(|a| a.id).collect();
                    let ids = album_tracks(&records);
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
        gfx.begin_frame();
        self.geom = Geom::default();
        self.render_header(area, buf, theme);
        let mut body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        if body.height == 0 || body.width < 4 {
            return;
        }
        if let Some(header) = self.header.as_mut() {
            let h = header.height(body.width, body.height, gfx);
            if h > 0 {
                let header_hover = match hover {
                    Some(Hit::Header(h)) => Some(h),
                    _ => None,
                };
                header.render(
                    Rect::new(body.x, body.y, body.width, h),
                    buf,
                    theme,
                    gfx,
                    header_hover,
                );
                body = Rect::new(body.x, body.y + h, body.width, body.height - h);
            }
        }
        self.geom.body = body;
        if self.is_empty() {
            if !self.loading() {
                let text = if self.filter.is_empty() {
                    self.source.empty()
                } else {
                    "no match"
                };
                buf.set_line(
                    body.x + 1,
                    body.y,
                    &Line::from(Span::styled(text, theme.hint_desc)),
                    body.width.saturating_sub(1),
                );
            }
            return;
        }
        match self.view {
            View::Tiles => self.render_tiles(body, buf, theme, gfx, thumbs, hover),
            View::List => self.render_list(body, buf, theme, favourites, hover),
        }
    }

    fn render_header(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let count = format!(" {} ", self.len());
        let mut x = area.x;
        let end = area.x + area.width;
        let put = |buf: &mut Buffer, x: &mut u16, text: &str, style: Style| -> Rect {
            let w = (text.chars().count() as u16).min(end.saturating_sub(*x));
            let at = Rect::new(*x, area.y, w, 1);
            buf.set_stringn(*x, area.y, text, w as usize, style);
            *x += w;
            at
        };
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
        if !self.filter_active && self.filter.is_empty() {
            controls.push((" / filter ".to_string(), Hit::Filter));
        }
        if self.filterable() {
            controls.push((
                match self.filters.count() {
                    0 => " filters ".to_string(),
                    n => format!(" filters ({n}) "),
                },
                Hit::Filters,
            ));
        }
        if !self.is_empty() {
            controls.push((" \u{22ef} actions ".to_string(), Hit::Menu));
        }
        if self.source == Source::Albums {
            controls.push((format!(" sort: {} ", ORDERS[self.order].1), Hit::Sort));
        }
        controls.push((
            match (self.view, self.tracks_in_list()) {
                (View::Tiles, _) => " view: tiles ",
                (View::List, false) => " view: rows ",
                (View::List, true) => " view: tracks ",
            }
            .to_string(),
            Hit::View,
        ));
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
                Hit::Filters => self.geom.filters = at,
                Hit::Menu => self.geom.menu = at,
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
        // Square art, as tall as the terminal's cell makes it. Two lines of
        // label under it, and a gap. Rows that fit whole are what paging and
        // the cursor keep to; the one after is drawn as far as it reaches.
        let grid = |width: u16| {
            let cols = ((width / TILE_MIN_W) as usize).max(1);
            let tile_w = (width / cols as u16).min(TILE_MAX_W);
            let tile_h = gfx.rows_for(tile_w.saturating_sub(2).max(1)) + 3;
            let rows = ((body.height / tile_h) as usize).max(1);
            (cols, tile_w, tile_h, rows)
        };
        let mut layout = grid(body.width);
        let overflows =
            |(cols, _, _, rows): (usize, u16, u16, usize)| self.albums.len().div_ceil(cols) > rows;
        let bar = overflows(layout) && body.width >= TILE_MIN_W + BAR_W;
        let grid_area = if bar {
            layout = grid(body.width - BAR_W);
            Rect {
                width: body.width - BAR_W,
                ..body
            }
        } else {
            body
        };
        let (cols, tile_w, tile_h, rows) = layout;
        let art_w = tile_w.saturating_sub(2).max(1);
        let art_h = tile_h - 3;
        self.geom.body = grid_area;
        self.geom.cols = cols;
        self.geom.tile_w = tile_w;
        self.geom.tile_h = tile_h;
        self.geom.rows = rows;
        // Left over width is split into a margin so the grid sits centred.
        let margin = (grid_area.width - tile_w * cols as u16) / 2;
        if self.follow {
            self.keep_cursor_visible();
        }
        let total_rows = self.albums.len().div_ceil(cols);
        self.scroll = self.scroll.min(total_rows.saturating_sub(rows));
        let bottom = body.y + body.height;
        if self.scroll != self.drawn_scroll {
            self.drawn_scroll = self.scroll;
            self.moved = Some(Instant::now());
        }
        let moving = self.moved.is_some_and(|at| at.elapsed() < SETTLE);
        let label = tile_w.saturating_sub(2) as usize;

        for r in 0..=rows {
            for c in 0..cols {
                let idx = (self.scroll + r) * cols + c;
                let Some(album) = self.albums.get(idx) else {
                    break;
                };
                let x = grid_area.x + margin + c as u16 * tile_w;
                let y = body.y + r as u16 * tile_h;
                if y >= bottom {
                    continue;
                }
                let selected = idx == self.cursor;
                let art = Rect::new(x + 1, y, art_w, art_h);
                let art_visible = (bottom - y).min(art_h);
                if let Some(img) = thumbs.get(album.id) {
                    gfx.draw_clipped(album_key(album.id), img, art, art_visible, moving, buf);
                } else {
                    placeholder(art, art_visible, buf, theme, &album.title);
                }
                if selected {
                    for dy in 0..art_visible {
                        buf[(x, y + dy)].set_char('\u{258c}').set_style(theme.bar);
                    }
                }
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
                    let (from, width) = self.title_span(album, label);
                    if from > 1 {
                        buf.set_string(x + 1, title_y, "\u{2665}", theme.favourite);
                    }
                    buf.set_stringn(
                        x + from as u16,
                        title_y,
                        fit(&album.title, width),
                        width,
                        style,
                    );
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
                    let year = year(album);
                    if !year.is_empty() && used + 3 + year.chars().count() <= label {
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
        // Covers a few rows either side load before they scroll into view.
        let ahead =
            (self.scroll + rows + 1) * cols..(self.scroll + rows + 1 + PREFETCH_ROWS) * cols;
        let behind = self.scroll.saturating_sub(PREFETCH_ROWS) * cols..self.scroll * cols;
        for idx in ahead.chain(behind) {
            if let Some(album) = self.albums.get(idx) {
                thumbs.want(album.id);
            }
        }
        if bar {
            self.scrollbar(body, buf, theme, total_rows, rows);
        }
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
        let total = self.len();
        let bar = total > rows && body.width >= 6 + BAR_W;
        let text = Rect {
            width: body.width - if bar { BAR_W } else { 0 },
            ..body
        };
        self.geom.body = text;
        self.geom.cols = 1;
        self.geom.tile_w = text.width;
        self.geom.tile_h = 1;
        self.geom.rows = rows;
        if self.follow {
            self.keep_cursor_visible();
        }
        self.scroll = self.scroll.min(total.saturating_sub(rows));
        let tracks = self.tracks_in_list();
        let w = text.width as usize;
        for r in 0..rows {
            let idx = self.scroll + r;
            if idx >= total {
                break;
            }
            let y = body.y + r as u16;
            let selected = idx == self.cursor;
            let hovered = hover == Some(Hit::Item(idx));
            let (name, detail, tail, fav) = if tracks {
                let t = &self.tracks[idx];
                (
                    t.title.as_str(),
                    format!("{} \u{b7} {}", t.artist_name, t.album_title),
                    t.duration_ms
                        .map(|d| format_time(d as u64))
                        .unwrap_or_default(),
                    favourites.contains(&t.id),
                )
            } else {
                let a = &self.albums[idx];
                let tail = [year(a), a.codec.clone().unwrap_or_default()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("  ");
                (
                    a.title.as_str(),
                    a.artist_name.clone(),
                    tail,
                    self.fav_albums.contains(&a.id),
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
            let tail_w = tail.chars().count();
            if fav && w > 2 {
                buf.set_string(
                    body.x + 1,
                    y,
                    "\u{2665}",
                    if selected {
                        theme.library_cursor
                    } else {
                        theme.favourite
                    },
                );
            }
            let name_w = (w.saturating_sub(tail_w + 4) * 3 / 5)
                .max(8)
                .min(w.saturating_sub(4));
            buf.set_stringn(body.x + 3, y, name, name_w, name_style);
            let detail_x = body.x + 3 + name_w as u16 + 2;
            let room = (text.x + text.width).saturating_sub(detail_x + tail_w as u16 + 2) as usize;
            if room > 0 {
                buf.set_stringn(detail_x, y, &detail, room, rest_style);
            }
            if tail_w > 0 && tail_w + 1 < w {
                buf.set_stringn(
                    text.x + text.width - tail_w as u16 - 1,
                    y,
                    &tail,
                    tail_w,
                    rest_style,
                );
            }
        }
        if bar {
            self.scrollbar(body, buf, theme, total, rows);
        }
    }

    /// Draws the scrollbar down the right of `body`, for `total` rows of which
    /// `visible` show.
    fn scrollbar(
        &mut self,
        body: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        total: usize,
        visible: usize,
    ) {
        let x = body.x + body.width - 1;
        let h = body.height as usize;
        // Two columns drawn and three to hit: a thumb one cell wide is hard
        // to land on with a pointer.
        self.geom.bar = Rect::new(x - 2, body.y, BAR_W, body.height);
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

fn year(album: &AlbumRow) -> String {
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
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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

    fn track(id: i64, album: i64) -> TrackRow {
        TrackRow {
            id,
            album_id: Some(album),
            artist_id: Some(album),
            artist_name: "artist".into(),
            album_artist_name: "artist".into(),
            album_title: format!("record {album}"),
            disc: None,
            track_number: Some(1),
            title: format!("song {id}"),
            duration_ms: Some(200_000),
            path: None,
            codec: Some("FLAC".into()),
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            genre: None,
            source: "local".into(),
            remote_id: None,
            cached_path: None,
        }
    }

    fn albums(ids: impl IntoIterator<Item = i64>) -> Listing {
        Listing {
            albums: ids
                .into_iter()
                .map(|i| album(i, &format!("record {i}"), "artist"))
                .collect(),
            ..Default::default()
        }
    }

    /// A page as if the reader had answered with `n` records.
    fn browse(n: usize) -> Browse {
        koan_core::config::isolate_config_for_tests();
        let mut b = Browse::new(Source::Albums);
        b.stale = false;
        b.asked = 1;
        b.show(1, albums(0..n as i64));
        b
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn draw(b: &mut Browse, w: u16, h: u16) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        b.render(
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

    #[test]
    fn tiles_fill_the_width_and_keys_move_by_row() {
        let mut b = browse(40);
        draw(&mut b, 80, 30);
        assert!(b.geom.cols >= 3);
        let cols = b.geom.cols;
        b.handle_key(key(KeyCode::Down));
        assert_eq!(b.cursor, cols);
        b.handle_key(key(KeyCode::Right));
        assert_eq!(b.cursor, cols + 1);
    }

    #[test]
    fn the_cursor_is_kept_on_screen() {
        let mut b = browse(200);
        draw(&mut b, 60, 20);
        b.handle_key(key(KeyCode::End));
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
    fn a_favourites_title_link_starts_after_its_heart() {
        let mut b = browse(3);
        b.fav_albums.insert(0);
        draw(&mut b, 80, 30);
        let art_h = b.geom.tile_h - 3;
        let (x, y) = first_tile(&b, 1, art_h);
        assert_eq!(b.hit(x, y), Some(Hit::Item(0)), "the heart is not the link");
        let (x, y) = first_tile(&b, 3, art_h);
        assert_eq!(b.hit(x, y), Some(Hit::Title(0)));
        // "record 0" is 8 cells from column 3: the last is 10, 11 is past it.
        let (x, y) = first_tile(&b, 11, art_h);
        assert_eq!(b.hit(x, y), Some(Hit::Item(0)));
    }

    #[test]
    fn the_menu_lists_what_can_be_done_with_a_record() {
        let mut b = browse(3);
        draw(&mut b, 80, 24);
        // Without a library to read the tracks of, the menu has nothing to
        // play, and says nothing rather than offering what would do nothing.
        assert_eq!(b.menu(0), Action::None);
        let Some(Hit::Menu) = b.hit(b.geom.menu.x, b.geom.menu.y) else {
            panic!("the actions control is a hit");
        };
    }

    #[test]
    fn ctrl_f_opens_the_filters_only_where_they_apply() {
        let ctrl_f = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
        let mut b = browse(3);
        assert_eq!(b.handle_key(ctrl_f), Some(Action::OpenFilters));
        b.source = Source::Favourites;
        b.view = View::List;
        assert_eq!(b.handle_key(ctrl_f), Some(Action::None));
        b.source = Source::Artist(1);
        b.view = View::Tiles;
        assert_eq!(b.handle_key(ctrl_f), Some(Action::None));
    }

    #[test]
    fn f_favourites_the_record_and_h_its_artist() {
        let mut b = browse(3);
        draw(&mut b, 80, 24);
        b.move_to(1);
        assert_eq!(
            b.handle_key(key(KeyCode::Char('f'))),
            Some(Action::ToggleAlbum(1))
        );
        assert_eq!(
            b.handle_key(key(KeyCode::Char('H'))),
            Some(Action::ToggleArtist(1))
        );
    }

    #[test]
    fn a_favourite_record_wears_a_heart() {
        let mut b = browse(3);
        b.fav_albums.insert(0);
        let buf = draw(&mut b, 80, 24);
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
    fn the_scrollbar_draws_beside_the_tiles_and_rows_not_over_them() {
        for view in [View::Tiles, View::List] {
            let mut b = browse(400);
            b.view = view;
            draw(&mut b, 80, 30);
            let (body, bar) = (b.geom.body, b.geom.bar);
            assert!(bar.width > 0, "{view:?} overflows");
            assert!(body.x + body.width <= bar.x, "{view:?}: {body:?} {bar:?}");
        }
    }

    #[test]
    fn a_row_that_does_not_fit_is_drawn_as_far_as_it_reaches() {
        let mut b = browse(40);
        // Tall enough for a whole row and a bit more.
        let buf = draw(&mut b, 80, 17);
        let theme = Theme::default();
        // The second row starts below the first and is cut by the bottom edge:
        // its placeholder is drawn on the rows that exist.
        let second_row_y = 1 + b.geom.tile_h;
        assert!(second_row_y + 1 < 17, "a second row begins on screen");
        let drawn =
            (second_row_y..17).any(|y| (0..80).any(|x| buf[(x, y)].bg == theme.tokens.surface));
        assert!(drawn);
    }

    #[test]
    fn long_titles_end_in_an_ellipsis() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("a longer title", 8), "a longe\u{2026}");
        assert_eq!(fit("anything", 0), "\u{2026}");
    }

    #[test]
    fn a_listing_keeps_the_cursor_on_its_record() {
        let mut b = browse(10);
        b.move_to(4);
        b.asked = 2;
        // Two records ahead of it are gone, and one is new at the end.
        b.show(2, albums([0, 1, 4, 5, 6, 7, 8, 9, 10]));
        assert_eq!(b.cursor, 2);
        assert_eq!(b.albums[b.cursor].id, 4);
        // When it is gone too, the cursor stays in range.
        b.move_to(8);
        b.asked = 3;
        b.show(3, albums([0, 1]));
        assert_eq!(b.cursor, 1);
        b.asked = 4;
        b.show(4, Listing::default());
        assert_eq!(b.cursor, 0);
        draw(&mut b, 80, 24);
    }

    #[test]
    fn only_the_latest_listing_is_shown() {
        let mut b = browse(3);
        b.asked = 5;
        assert!(!b.show(4, albums(0..40)), "a stale listing is dropped");
        assert_eq!(b.len(), 3);
        assert!(b.show(5, albums(0..40)));
        assert_eq!(b.len(), 40);
    }

    #[test]
    fn typing_a_filter_asks_for_the_listing_and_starts_at_the_top() {
        let mut b = browse(40);
        draw(&mut b, 80, 24);
        b.move_to(30);
        b.handle_key(key(KeyCode::Char('/')));
        let asked = b.asked;
        b.handle_key(key(KeyCode::Char('x')));
        assert_eq!(b.filter, "x");
        assert_eq!(b.asked, asked + 1, "each change asks again");
        b.show(b.asked, albums(25..35));
        assert_eq!(b.cursor, 0, "a new filter starts at the top");
        b.handle_key(key(KeyCode::Backspace));
        b.handle_key(key(KeyCode::Backspace));
        assert!(!b.filter_active, "backspace on an empty filter leaves it");
        let asked = b.asked;
        b.handle_key(key(KeyCode::Esc));
        assert_eq!(b.asked, asked, "nothing changed, nothing asked");
    }

    #[test]
    fn an_artists_rows_are_records() {
        koan_core::config::isolate_config_for_tests();
        let mut b = Browse::new(Source::Artist(7));
        b.stale = false;
        b.asked = 1;
        b.show(1, albums(0..3));
        b.handle_key(key(KeyCode::Char('v')));
        assert_eq!(b.view, View::List);
        assert_eq!(b.len(), 3);
        assert!(!b.tracks_in_list());
    }

    #[test]
    fn switching_view_keeps_the_record() {
        let mut b = browse(10);
        b.source = Source::Favourites;
        b.tracks = (0..10)
            .flat_map(|a| [track(a * 2, a), track(a * 2 + 1, a)])
            .collect();
        draw(&mut b, 80, 24);
        b.move_to(3);
        b.toggle_view();
        assert_eq!(b.tracks[b.cursor].album_id, Some(3));
        b.move_to(b.cursor + 1);
        b.toggle_view();
        assert_eq!(b.cursor, 3);
    }

    #[test]
    fn it_draws_at_any_size() {
        let mut b = browse(30);
        for view in [View::Tiles, View::List] {
            b.view = view;
            for w in [1u16, 2, 5, 13, 14, 16, 17, 20, 60, 200] {
                for h in [1u16, 2, 3, 5, 12, 40] {
                    draw(&mut b, w, h);
                }
            }
        }
    }
}
