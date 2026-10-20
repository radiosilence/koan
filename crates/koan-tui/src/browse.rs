//! The library's browsers: Albums, Artists and Tracks, and an artist's page,
//! which is their albums under a header. One page type with a source, because
//! they are the library's own listings, narrowed by the browse filters and the
//! name filter and ordered by the sort, as the apps' browsers are
//! (`koan_core::browse`). Albums are shown as tiles of cover art, or as rows.
//!
//! Listings are read off the UI thread, narrowed and ordered in SQL, and
//! arrive whole; a read asked for after another always wins over it.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use image::DynamicImage;
use koan_core::browse::{self as core_browse, AlbumSort, BrowseFilter, TrackSort};
use koan_core::db::queries::{
    self, AlbumOrder, AlbumQuery, AlbumRow, ArtistRow, CreditsOf, TrackRow,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::actions_menu::{Playable, credited, playable};
use super::app::PickerAction;
use super::artist_header::{ArtistHeader, HeaderHit};
use super::cover_art::{Gfx, album_key};
use super::rows::{self, Columns, Emphasis, Row, Target};
use super::theme::Theme;
use super::thumb_cache;
use super::transport::format_time;

/// The widest a tile gets, in columns. Wider screens get more tiles.
const TILE_MAX_W: u16 = 26;
const TILE_MIN_W: u16 = 14;

/// Decoded covers kept for tiles before the lot is dropped.
const THUMB_LIMIT: usize = 800;

/// How long the grid must be still before its covers are drawn as images.
const SETTLE: Duration = Duration::from_millis(150);

/// How many rows beyond the page are loaded ahead of scrolling.
const PREFETCH_ROWS: usize = 3;

/// A second click this soon after the first is a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// Columns the scrollbar takes from the right of a page that overflows: two
/// drawn and one of gap.
const BAR_W: u16 = 3;

/// Tracks are read this many at a time, more as the cursor nears the end of
/// what is here: a library has tens of thousands.
const TRACK_PAGE: u32 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Albums,
    Artists,
    Tracks,
    /// One artist's albums, or the tracks they appear on when they have none.
    Artist(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Tiles,
    List,
}

/// What the page lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Albums,
    Artists,
    Tracks,
}

fn album_sort_label(sort: AlbumSort) -> &'static str {
    match sort {
        AlbumSort::RecentlyAdded => "recently added",
        AlbumSort::Title => "title",
        AlbumSort::Artist => "artist",
        AlbumSort::Year => "year, newest",
        AlbumSort::YearAsc => "year, oldest",
        AlbumSort::Random => "random",
        AlbumSort::LastPlayed => "last played",
        AlbumSort::Downloaded => "most downloaded",
        AlbumSort::BestMatch => "best match",
    }
}

fn track_sort_label(sort: TrackSort) -> &'static str {
    match sort {
        TrackSort::Artist => "artist",
        TrackSort::Title => "title",
        TrackSort::Album => "album",
        TrackSort::Duration => "duration",
        TrackSort::LastPlayed => "last played",
        TrackSort::BestMatch => "best match",
    }
}

/// The sort after `now` in `offered`, round to the first.
fn next_sort<T: PartialEq + Copy>(now: T, offered: &[T]) -> T {
    let at = offered.iter().position(|s| *s == now);
    offered[at.map_or(0, |i| (i + 1) % offered.len())]
}

/// What a click or key asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Play(Vec<i64>, PickerAction),
    Favourite(i64),
    OpenAlbum(i64),
    OpenArtist(i64),
    /// The artist a track's or album's credit names; one of them, asked
    /// for, when it names several.
    OpenCredited(CreditsOf, i64),
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
    /// Open the panel of filters, for the browser on screen.
    OpenFilters,
    /// Fetch these tracks to this device, or let the copies go.
    Download(Vec<i64>),
    RemoveDownload(Vec<i64>),
    /// Open a browser set up as a shelf is: its "See all".
    SeeAll(SeeAll),
    /// Forget the plays History has picked out, or the one under its cursor.
    ForgetPlays,
}

/// A browser to open, and what to open it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeeAll {
    pub source: Source,
    pub filter: BrowseFilter,
    pub album_sort: AlbumSort,
    pub track_sort: TrackSort,
    pub text: String,
}

/// Where the pointer is on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Item(usize),
    /// A name that leads to its page.
    Link(Target),
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

    /// Forgets which albums had no cover, so they are looked for again: a
    /// scan or a sync may have brought one.
    pub fn retry_missing(&mut self) {
        self.images.retain(|_, image| image.is_some());
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

/// An album's cover as a thumbnail, from the disk cache when it has one, else
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

/// A track as the apps' menu takes it.
pub(super) fn track_playable(t: &TrackRow, favourite: bool) -> Playable {
    Playable::Track {
        id: t.id,
        favourite,
        on_disk: t.path.is_some() || t.cached_path.is_some(),
        album: t.album_id,
        artists: credited(CreditsOf::Tracks, t.id, &t.artist_name, t.artist_id),
    }
}

/// The tracks of `albums`, album by album in the order given, in one read.
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

/// An artist's albums as their page lists them, narrowed offline to what can
/// play here.
fn artist_album_query(id: i64) -> AlbumQuery<'static> {
    AlbumQuery {
        artist_id: Some(id),
        order: AlbumOrder::Date,
        filter: queries::AlbumFilter {
            on_device: koan_core::remote::offline::active(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Every track of an artist's albums, as play and shuffle on their page and in
/// search take them; what they appear on when they have no albums of their own.
pub(super) fn artist_tracks(id: i64) -> Vec<i64> {
    let Ok(db) = koan_core::db::pool::shared().get() else {
        return Vec::new();
    };
    let albums: Vec<i64> = queries::list_albums(&db.conn, &artist_album_query(id))
        .unwrap_or_default()
        .iter()
        .map(|a| a.id)
        .collect();
    if albums.is_empty() {
        return queries::tracks_for_artist(&db.conn, id)
            .unwrap_or_default()
            .iter()
            .map(|t| t.id)
            .collect();
    }
    drop(db);
    album_tracks(&albums)
}

/// Everything a listing is read by.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ask {
    source: Source,
    filter: BrowseFilter,
    album_sort: AlbumSort,
    track_sort: TrackSort,
    seed: i64,
    /// The name filter, matched by the database as the apps' boxes are.
    text: String,
    /// How many tracks to read.
    limit: u32,
}

/// A listing as the database answered it.
#[derive(Default)]
struct Listing {
    favourites: Favourites,
    albums: Vec<AlbumRow>,
    artists: Vec<ArtistRow>,
    tracks: Vec<TrackRow>,
    /// How many tracks pass, of which `tracks` are the first.
    track_total: u64,
    /// The artist's name, on an artist's page.
    artist: Option<String>,
}

/// What is a favourite, read with each listing so the hearts drawn match it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Favourites {
    pub albums: HashSet<i64>,
    pub artists: HashSet<i64>,
    pub tracks: HashSet<i64>,
}

impl Favourites {
    pub(crate) fn read(db: &koan_core::db::connection::Database) -> Self {
        let (conn, user) = (&db.conn, queries::LOCAL_USER);
        Self {
            albums: queries::favourite_album_id_set(conn, user).unwrap_or_default(),
            artists: queries::favourite_artist_id_set(conn, user).unwrap_or_default(),
            tracks: queries::load_favourites(conn, user).unwrap_or_default(),
        }
    }
}

fn read(ask: &Ask) -> Listing {
    let Ok(db) = koan_core::db::pool::shared().get() else {
        return Listing::default();
    };
    let text = Some(ask.text.as_str());
    let mut listing = Listing {
        favourites: Favourites::read(&db),
        ..Listing::default()
    };
    match ask.source {
        Source::Albums => {
            let query = core_browse::albums(&ask.filter, text, ask.album_sort, ask.seed);
            listing.albums = queries::list_albums(&db.conn, &query).unwrap_or_default();
        }
        Source::Artists => {
            let query = core_browse::artists(&ask.filter, text);
            listing.artists = queries::list_artists(&db.conn, &query).unwrap_or_default();
        }
        Source::Tracks => {
            let tracks = core_browse::tracks(&ask.filter, text, ask.track_sort);
            listing.track_total = tracks.count(&db.conn).unwrap_or_default();
            listing.tracks = tracks.page(&db.conn, ask.limit, 0).unwrap_or_default();
        }
        Source::Artist(id) => {
            let query = AlbumQuery {
                search: core_browse::trimmed(text),
                ..artist_album_query(id)
            };
            listing.albums = queries::list_albums(&db.conn, &query).unwrap_or_default();
            if listing.albums.is_empty() && ask.text.trim().is_empty() {
                listing.tracks = queries::tracks_for_artist(&db.conn, id).unwrap_or_default();
                listing.track_total = listing.tracks.len() as u64;
            }
            listing.artist = queries::get_artist(&db.conn, id)
                .ok()
                .flatten()
                .map(|a| a.name);
        }
    }
    listing
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
#[derive(Default, Clone)]
struct Geom {
    /// Below the toolbar and an artist's header: the items themselves, left
    /// of the scrollbar.
    body: Rect,
    cols: usize,
    tile_w: u16,
    tile_h: u16,
    rows: usize,
    links: Vec<(Rect, Target)>,
    /// The scrollbar's column, when the page overflows.
    bar: Rect,
    /// Rows in all; the scroll runs between it and the page.
    total_rows: usize,
}

pub struct Browse {
    pub source: Source,
    pub view: View,
    /// The heading, for a page about one artist.
    title: String,
    albums: Vec<AlbumRow>,
    artists: Vec<ArtistRow>,
    tracks: Vec<TrackRow>,
    track_total: u64,
    cursor: usize,
    scroll: usize,
    /// Whether the next frame scrolls to the cursor; the wheel and the
    /// scrollbar turn it off so they are not undone.
    follow: bool,
    /// The albums and artists that are favourites, for their hearts, and
    /// the tracks, for the menu's favourite item.
    fav_albums: HashSet<i64>,
    fav_artists: HashSet<i64>,
    fav_tracks: HashSet<i64>,
    header: Option<ArtistHeader>,
    pub filter: String,
    pub filter_active: bool,
    /// What the filters panel has narrowed the listing by.
    filters: BrowseFilter,
    album_sort: AlbumSort,
    track_sort: TrackSort,
    /// Which shuffle Random means: held, so narrowing a shuffle does not deal
    /// it again.
    seed: i64,
    /// How many tracks are asked for.
    limit: u32,
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

fn new_seed() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

impl Browse {
    pub fn new(source: Source) -> Self {
        let (tx, rx) = spawn_reader();
        Self {
            source,
            view: View::Tiles,
            title: String::new(),
            albums: Vec::new(),
            artists: Vec::new(),
            tracks: Vec::new(),
            track_total: 0,
            cursor: 0,
            scroll: 0,
            follow: true,
            fav_albums: HashSet::new(),
            fav_artists: HashSet::new(),
            fav_tracks: HashSet::new(),
            header: None,
            filter: String::new(),
            filter_active: false,
            filters: BrowseFilter::default(),
            album_sort: AlbumSort::default(),
            track_sort: TrackSort::default(),
            seed: new_seed(),
            limit: TRACK_PAGE,
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

    fn kind(&self) -> Kind {
        match self.source {
            Source::Albums => Kind::Albums,
            Source::Artists => Kind::Artists,
            Source::Tracks => Kind::Tracks,
            Source::Artist(_) if self.albums.is_empty() && !self.tracks.is_empty() => Kind::Tracks,
            Source::Artist(_) => Kind::Albums,
        }
    }

    /// Whether albums are drawn as tiles.
    fn tiles(&self) -> bool {
        self.view == View::Tiles && self.kind() == Kind::Albums
    }

    /// Whether the filters panel narrows this page: the browsers, not an
    /// artist's page.
    pub fn filterable(&self) -> bool {
        !matches!(self.source, Source::Artist(_))
    }

    /// Whether the panel offers the lossless filter: a track's codec says it
    /// already, and the track browser narrows by codec instead.
    pub fn offers_lossless(&self) -> bool {
        self.source != Source::Tracks
    }

    pub fn set_filters(&mut self, filters: &BrowseFilter) {
        if self.filterable() && self.filters != *filters {
            self.filters = filters.clone();
            self.limit = TRACK_PAGE;
            self.stale = true;
        }
    }

    /// Sets the page up as a shelf's "See all" asks: its filters, sorts and
    /// name filter, read afresh from the top.
    pub fn adopt(&mut self, see: &SeeAll) {
        self.filters = see.filter.clone();
        self.album_sort = see.album_sort;
        self.track_sort = see.track_sort;
        self.filter = see.text.clone();
        self.filter_active = false;
        self.limit = TRACK_PAGE;
        self.to_top = true;
        self.stale = true;
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
            filter: self.filters.clone(),
            album_sort: self.album_sort,
            track_sort: self.track_sort,
            seed: self.seed,
            text: self.filter.clone(),
            limit: self.limit,
        };
        self.tx.send((self.asked, ask)).ok();
    }

    pub fn ensure_loaded(&mut self) {
        if self.stale {
            self.reload();
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
        let kind = self.kind();
        self.shown = generation;
        self.albums = listing.albums;
        self.artists = listing.artists;
        self.tracks = listing.tracks;
        self.track_total = listing.track_total;
        self.fav_albums = listing.favourites.albums;
        self.fav_artists = listing.favourites.artists;
        self.fav_tracks = listing.favourites.tracks;
        if let (Some(header), Source::Artist(id)) = (self.header.as_mut(), self.source) {
            header.set_favourite(self.fav_artists.contains(&id));
        }
        if let Some(name) = listing.artist {
            self.title = name;
        }
        if let Some(header) = self.header.as_mut() {
            let holds = if self.albums.is_empty() && !self.tracks.is_empty() {
                let on: HashSet<_> = self.tracks.iter().filter_map(|t| t.album_id).collect();
                format!(
                    "appears on {}",
                    super::text::count(on.len() as u64, "album")
                )
            } else {
                super::text::count(self.albums.len() as u64, "album")
            };
            header.set_artist(&self.title, holds);
        }
        self.last_click = None;
        let found = on
            .filter(|_| kind == self.kind())
            .and_then(|id| self.position(id));
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

    /// The id of the album, artist or track under the cursor.
    fn cursor_id(&self) -> Option<i64> {
        match self.kind() {
            Kind::Albums => self.albums.get(self.cursor).map(|a| a.id),
            Kind::Artists => self.artists.get(self.cursor).map(|a| a.id),
            Kind::Tracks => self.tracks.get(self.cursor).map(|t| t.id),
        }
    }

    fn position(&self, id: i64) -> Option<usize> {
        match self.kind() {
            Kind::Albums => self.albums.iter().position(|a| a.id == id),
            Kind::Artists => self.artists.iter().position(|a| a.id == id),
            Kind::Tracks => self.tracks.iter().position(|t| t.id == id),
        }
    }

    /// Whether nothing has been shown yet: the first read is under way.
    fn loading(&self) -> bool {
        self.shown == 0
    }

    /// Changes the name filter, and reads the listing it narrows to.
    fn set_text(&mut self, text: String) {
        if text != self.filter {
            self.filter = text;
            self.to_top = true;
            self.limit = TRACK_PAGE;
            self.reload();
        }
    }

    pub fn len(&self) -> usize {
        match self.kind() {
            Kind::Albums => self.albums.len(),
            Kind::Artists => self.artists.len(),
            Kind::Tracks => self.tracks.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many the listing holds in all: tracks arrive a page at a time.
    fn total(&self) -> u64 {
        match self.kind() {
            Kind::Tracks => self.track_total.max(self.tracks.len() as u64),
            _ => self.len() as u64,
        }
    }

    /// Switches albums between tiles and rows, the cursor on its album.
    pub fn toggle_view(&mut self) {
        if self.kind() != Kind::Albums {
            return;
        }
        self.view = match self.view {
            View::Tiles => View::List,
            View::List => View::Tiles,
        };
        self.scroll = 0;
        self.move_to(self.cursor);
    }

    /// The next sort the page offers.
    pub fn cycle_sort(&mut self) {
        let searching = !self.filter.trim().is_empty();
        match self.source {
            Source::Albums => {
                self.album_sort = next_sort(
                    self.album_sort,
                    &AlbumSort::offered(&self.filters, searching),
                );
            }
            Source::Tracks => {
                self.track_sort = next_sort(
                    self.track_sort,
                    &TrackSort::offered(&self.filters, searching),
                );
                self.limit = TRACK_PAGE;
            }
            _ => return,
        }
        self.reload();
    }

    /// Deals the random order again.
    pub fn reshuffle(&mut self) {
        if self.source == Source::Albums && self.album_sort == AlbumSort::Random {
            self.seed = new_seed();
            self.to_top = true;
            self.reload();
        }
    }

    fn sort_label(&self) -> Option<&'static str> {
        match self.source {
            Source::Albums => Some(album_sort_label(self.album_sort)),
            Source::Tracks => Some(track_sort_label(self.track_sort)),
            _ => None,
        }
    }

    fn cols(&self) -> usize {
        if self.tiles() {
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
        self.read_ahead();
    }

    /// Asks for more tracks when the view nears the end of what is here.
    fn read_ahead(&mut self) {
        let n = self.tracks.len();
        let near = self.cursor.max(self.scroll + self.page_rows()) + self.page_rows() * 2;
        if self.source == Source::Tracks
            && (n as u64) < self.track_total
            && near >= n
            && self.limit as usize <= n
        {
            self.limit = self.limit.saturating_add(TRACK_PAGE);
            self.reload();
        }
    }

    /// A notch of the wheel: a row of tiles, or three of a list.
    pub fn wheel(&mut self, down: bool) {
        let rows = if self.tiles() { 1 } else { 3 };
        self.scroll_by(if down { rows } else { -rows });
    }

    pub fn scroll_by(&mut self, rows: isize) {
        self.follow = false;
        let total_rows = self.len().div_ceil(self.cols());
        let max = total_rows.saturating_sub(self.page_rows());
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
        self.read_ahead();
    }

    /// The ids a play of the item at `idx` would queue.
    fn ids_at(&self, idx: usize) -> Vec<i64> {
        match self.kind() {
            Kind::Albums => self
                .albums
                .get(idx)
                .map(|a| album_tracks(&[a.id]))
                .unwrap_or_default(),
            Kind::Artists => self
                .artists
                .get(idx)
                .map(|a| artist_tracks(a.id))
                .unwrap_or_default(),
            Kind::Tracks => self.tracks.get(idx).map(|t| vec![t.id]).unwrap_or_default(),
        }
    }

    /// What Enter and a second click do: a track plays the list from there,
    /// as the apps' track lists do; anything else opens.
    fn primary(&self, idx: usize) -> Action {
        match self.kind() {
            Kind::Tracks if idx < self.tracks.len() => Action::Play(
                self.tracks.iter().map(|t| t.id).collect(),
                PickerAction::ReplaceFrom(idx),
            ),
            Kind::Albums => self.album_at(idx).map_or(Action::None, Action::OpenAlbum),
            Kind::Artists => self.artist_at(idx).map_or(Action::None, Action::OpenArtist),
            Kind::Tracks => Action::None,
        }
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

    /// The album the item at `idx` is, or is on.
    fn album_at(&self, idx: usize) -> Option<i64> {
        match self.kind() {
            Kind::Albums => Some(self.albums.get(idx)?.id),
            Kind::Tracks => self.tracks.get(idx)?.album_id,
            Kind::Artists => None,
        }
    }

    /// The artist the item at `idx` is, or is by.
    fn artist_at(&self, idx: usize) -> Option<i64> {
        match self.kind() {
            Kind::Albums => Some(self.albums.get(idx)?.artist_id),
            Kind::Artists => Some(self.artists.get(idx)?.id),
            Kind::Tracks => self.tracks.get(idx)?.artist_id,
        }
    }

    /// What `A` opens: the artist at `idx`, or the artists its credit names.
    fn open_artist_at(&self, idx: usize) -> Action {
        let credited =
            |of, id: Option<i64>| id.map_or(Action::None, |id| Action::OpenCredited(of, id));
        match self.kind() {
            Kind::Albums => credited(CreditsOf::Albums, self.album_at(idx)),
            Kind::Artists => self.artist_at(idx).map_or(Action::None, Action::OpenArtist),
            Kind::Tracks => credited(CreditsOf::Tracks, self.tracks.get(idx).map(|t| t.id)),
        }
    }

    /// The track under the cursor when the page lists tracks, for hearting.
    pub fn track_at_cursor(&self) -> Option<i64> {
        if self.kind() != Kind::Tracks {
            return None;
        }
        self.tracks.get(self.cursor).map(|t| t.id)
    }

    /// What `f` favourites: the item under the cursor.
    fn favourite_at(&self, idx: usize) -> Action {
        match self.kind() {
            Kind::Albums => self.album_at(idx).map_or(Action::None, Action::ToggleAlbum),
            Kind::Artists => self
                .artist_at(idx)
                .map_or(Action::None, Action::ToggleArtist),
            Kind::Tracks => self
                .tracks
                .get(idx)
                .map_or(Action::None, |t| Action::Favourite(t.id)),
        }
    }

    /// What can be done with the item at `idx`, as a menu.
    fn menu(&self, idx: usize) -> Action {
        let ids = self.ids_at(idx);
        if ids.is_empty() {
            return Action::None;
        }
        let (title, thing) = match self.kind() {
            Kind::Tracks => {
                let t = &self.tracks[idx];
                let favourite = self.fav_tracks.contains(&t.id);
                (t.title.clone(), track_playable(t, favourite))
            }
            Kind::Albums => {
                let a = &self.albums[idx];
                let thing = Playable::Album {
                    id: a.id,
                    tracks: ids,
                    favourite: self.fav_albums.contains(&a.id),
                    artists: credited(CreditsOf::Albums, a.id, &a.artist_name, Some(a.artist_id)),
                };
                (a.title.clone(), thing)
            }
            Kind::Artists => {
                let a = &self.artists[idx];
                let thing = Playable::Artist {
                    id: a.id,
                    tracks: ids,
                    favourite: self.fav_artists.contains(&a.id),
                };
                (a.name.clone(), thing)
            }
        };
        Action::Menu(title, playable(&thing))
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
                KeyCode::Char('u') if ctrl => self.set_text(String::new()),
                KeyCode::Char(c) if !ctrl => self.set_text(format!("{}{c}", self.filter)),
                _ => {}
            }
            return Some(Action::None);
        }
        let cols = self.cols();
        let page = self.page_rows() * cols;
        let tiles = self.tiles();
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
            KeyCode::Enter => return Some(self.primary(self.cursor)),
            KeyCode::Char('O') => {
                return Some(
                    self.album_at(self.cursor)
                        .map_or(Action::None, Action::OpenAlbum),
                );
            }
            KeyCode::Char('A') => {
                return Some(self.open_artist_at(self.cursor));
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
            KeyCode::Char('f') => return Some(self.favourite_at(self.cursor)),
            KeyCode::Char('m') => return Some(self.menu(self.cursor)),
            KeyCode::Char('w') => return Some(ids(self, Action::Choose)),
            KeyCode::Char('d') => return Some(ids(self, Action::Download)),
            KeyCode::Char('u') => return Some(ids(self, Action::RemoveDownload)),
            KeyCode::Char('i') if self.kind() == Kind::Tracks => {
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
            KeyCode::Char('o') => self.cycle_sort(),
            KeyCode::Char('S') if self.album_sort == AlbumSort::Random => self.reshuffle(),
            KeyCode::Esc if !self.filter.is_empty() => self.clear_filter(),
            _ => return None,
        }
        Some(Action::None)
    }

    fn clear_filter(&mut self) {
        self.filter_active = false;
        self.set_text(String::new());
    }

    /// What the pointer is over.
    pub fn hit(&self, col: u16, row: u16) -> Option<Hit> {
        let g = &self.geom;
        let inside = |r: &Rect| {
            r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
        };
        if let Some(h) = self.header.as_ref().and_then(|h| h.hit(col, row)) {
            return Some(Hit::Header(h));
        }
        if let Some((_, t)) = g.links.iter().find(|(r, _)| inside(r)) {
            return Some(Hit::Link(*t));
        }
        let body = g.body;
        if !inside(&body) {
            return None;
        }
        if !self.tiles() {
            let idx = self.scroll + (row - body.y) as usize;
            return (idx < self.len()).then_some(Hit::Item(idx));
        }
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
        let idx = (self.scroll + ((row - body.y) / tile_h) as usize) * g.cols + tile_col as usize;
        (idx < self.albums.len()).then_some(Hit::Item(idx))
    }

    /// A left click. The first selects; the second, soon after on the same
    /// item, opens or plays it.
    pub fn click(&mut self, hit: Hit) -> Action {
        match hit {
            Hit::Header(h) => {
                let Source::Artist(id) = self.source else {
                    return Action::None;
                };
                let tracks = || {
                    let ids = artist_tracks(id);
                    (!ids.is_empty()).then_some(ids)
                };
                match h {
                    HeaderHit::Play => tracks().map_or(Action::None, |ids| {
                        Action::Play(ids, PickerAction::ReplaceQueue)
                    }),
                    HeaderHit::Shuffle => tracks().map_or(Action::None, Action::Shuffle),
                    HeaderHit::Add => {
                        tracks().map_or(Action::None, |ids| Action::Play(ids, PickerAction::Append))
                    }
                    HeaderHit::Next => tracks().map_or(Action::None, |ids| {
                        Action::Play(ids, PickerAction::PlayNext)
                    }),
                    HeaderHit::Heart => Action::ToggleArtist(id),
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
                }
            }
            Hit::Link(Target::Album(id)) => Action::OpenAlbum(id),
            Hit::Link(Target::Artist(id)) => Action::OpenArtist(id),
            Hit::Item(idx) => {
                let double = self
                    .last_click
                    .is_some_and(|(i, at)| i == idx && at.elapsed() < DOUBLE_CLICK);
                self.move_to(idx);
                if double {
                    self.last_click = None;
                    self.primary(idx)
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
        self.geom = Geom::default();
        if area.width < 4 || area.height == 0 {
            return;
        }
        self.ensure_loaded();
        thumbs.begin_frame();
        gfx.begin_frame();
        let mut body = super::shell::body(area);
        if body.height == 0 {
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
            if !self.loading() && body.height > 0 {
                rows::put(
                    buf,
                    body.x + 1,
                    body.y,
                    body.width.saturating_sub(2),
                    self.empty_text(),
                    theme.hint_desc,
                );
            }
            return;
        }
        let hover_link = match hover {
            Some(Hit::Link(t)) => Some(t),
            _ => None,
        };
        let hover_item = match hover {
            Some(Hit::Item(i)) => Some(i),
            _ => None,
        };
        if self.tiles() {
            self.render_tiles(body, buf, theme, gfx, thumbs, hover_link);
        } else {
            self.render_list(body, buf, theme, favourites, hover_item, hover_link);
        }
    }

    fn empty_text(&self) -> &'static str {
        let narrowed = !self.filter.trim().is_empty() || self.filters.active_count() > 0;
        match self.source {
            _ if narrowed => "nothing matches",
            Source::Albums => "no albums yet: add a folder in Settings, or sign in to a server",
            Source::Artists => "no artists yet",
            Source::Tracks => "no tracks yet",
            Source::Artist(_) => "no albums by this artist",
        }
    }

    /// The page's title and count on the left; its filter, sort, view and
    /// actions on the right, as the keys that do them.
    pub fn toolbar(&self) -> crate::shell::Toolbar {
        use crate::shell::{ToolItem, Toolbar};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        let title = match self.source {
            Source::Albums => "albums",
            Source::Artists => "artists",
            Source::Tracks => "tracks",
            Source::Artist(_) => self.title.as_str(),
        };
        let noun = match self.kind() {
            Kind::Albums => "album",
            Kind::Artists => "artist",
            Kind::Tracks => "track",
        };
        let mut bar = Toolbar::new(title);
        // An artist's header says what the page holds.
        if !self.loading() && !matches!(self.source, Source::Artist(_)) {
            bar = bar.count(super::text::count(self.total(), noun));
        }
        let filter = if self.filter_active || !self.filter.is_empty() {
            let cursor = if self.filter_active { "\u{2588}" } else { "" };
            format!("/{}{cursor}", self.filter)
        } else {
            "/ filter".into()
        };
        bar = bar.item(
            ToolItem::new(filter, key('/'))
                .on(!self.filter.is_empty())
                .priority(5),
        );
        if self.filterable() {
            let n = self.filters.active_count();
            let label = match n {
                0 => "filters".to_string(),
                n => format!("filters ({n})"),
            };
            bar = bar.item(
                ToolItem::new(
                    label,
                    KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
                )
                .on(n > 0)
                .priority(3),
            );
        }
        if let Some(sort) = self.sort_label() {
            bar = bar.item(ToolItem::new(format!("sort: {sort}"), key('o')).priority(4));
            if self.source == Source::Albums && self.album_sort == AlbumSort::Random {
                bar = bar.item(ToolItem::new("\u{2684} shuffle again", key('S')).priority(2));
            }
        }
        if self.kind() == Kind::Albums {
            let view = match self.view {
                View::Tiles => "view: grid",
                View::List => "view: list",
            };
            bar = bar.item(ToolItem::new(view, key('v')).priority(2));
        }
        if !self.is_empty() {
            bar = bar.item(ToolItem::new("\u{22ef} actions", key('m')).priority(1));
        }
        bar
    }

    fn render_tiles(
        &mut self,
        body: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        gfx: &mut Gfx,
        thumbs: &mut Thumbs,
        hover: Option<Target>,
    ) {
        // Square art, as tall as the terminal's cell makes it. Two lines of
        // label under it, and a gap. Rows that fit whole are what paging and
        // the cursor keep to; the one after is drawn as far as it reaches.
        let grid = |width: u16| tile_grid(width, body.height, gfx);
        let mut layout = grid(body.width);
        let n = self.albums.len();
        let overflows = |(cols, _, _, rows): (usize, u16, u16, usize)| n.div_ceil(cols) > rows;
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
        let total_rows = n.div_ceil(cols);
        self.scroll = self.scroll.min(total_rows.saturating_sub(rows));
        let bottom = body.y + body.height;
        if self.scroll != self.drawn_scroll {
            self.drawn_scroll = self.scroll;
            self.moved = Some(Instant::now());
        }
        let moving = self.moved.is_some_and(|at| at.elapsed() < SETTLE);
        // An artist's own page does not name them under every tile.
        let name_artist = !matches!(self.source, Source::Artist(_));
        let mut links = Vec::new();

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
                Tile {
                    album,
                    favourite: self.fav_albums.contains(&album.id),
                    selected: idx == self.cursor,
                    name_artist,
                }
                .draw(
                    buf,
                    theme,
                    gfx,
                    thumbs,
                    Rect::new(x, y, tile_w, art_h + 3),
                    bottom,
                    moving,
                    hover,
                    &mut links,
                );
            }
        }
        self.geom.links = links;
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
        hover_item: Option<usize>,
        hover: Option<Target>,
    ) {
        let rows_on = body.height as usize;
        let n = self.len();
        let bar = n > rows_on && body.width >= 6 + BAR_W;
        let text = Rect {
            width: body.width - if bar { BAR_W } else { 0 },
            ..body
        };
        self.geom.body = text;
        self.geom.cols = 1;
        self.geom.rows = rows_on;
        if self.follow {
            self.keep_cursor_visible();
        }
        self.scroll = self.scroll.min(n.saturating_sub(rows_on));
        let kind = self.kind();
        let cols = match kind {
            Kind::Tracks => {
                let lead = (self.total().max(1).ilog10() + 1) as u16;
                Columns::new(text, lead, 7, true, true)
            }
            Kind::Albums => Columns::new(text, 0, ALBUM_TAIL, true, false),
            Kind::Artists => Columns::new(text, 0, ARTIST_TAIL, false, false),
        };
        let mut links = Vec::new();
        for r in 0..rows_on {
            let idx = self.scroll + r;
            if idx >= n {
                break;
            }
            let row = Rect::new(text.x, body.y + r as u16, text.width, 1);
            let emphasis = Emphasis::of(idx == self.cursor, hover_item == Some(idx));
            match kind {
                Kind::Tracks => {
                    let t = &self.tracks[idx];
                    let lead = (idx + 1).to_string();
                    let time = t
                        .duration_ms
                        .map(|d| format_time(d as u64))
                        .unwrap_or_default();
                    Row {
                        favourite: favourites.contains(&t.id),
                        lead: &lead,
                        name: &t.title,
                        artist: Some((&t.artist_name, t.artist_id)),
                        detail: Some((&t.album_title, t.album_id.map(Target::Album))),
                        tail: &time,
                    }
                    .draw(buf, &cols, row, theme, emphasis, hover, &mut links);
                }
                Kind::Albums => {
                    let a = &self.albums[idx];
                    Row {
                        favourite: self.fav_albums.contains(&a.id),
                        lead: "",
                        name: &a.title,
                        artist: Some((&a.artist_name, Some(a.artist_id))),
                        detail: None,
                        tail: "",
                    }
                    .draw(buf, &cols, row, theme, emphasis, hover, &mut links);
                    // The year, then the codec at the end.
                    if cols.tail.w == ALBUM_TAIL {
                        let quiet = emphasis.quiet(theme);
                        let end = cols.tail.x + cols.tail.w;
                        rows::put(buf, cols.tail.x, row.y, 4, &year(a), quiet);
                        let codec = a.codec.as_deref().unwrap_or_default();
                        rows::put_right(buf, end, row.y, 5, codec, quiet);
                    }
                }
                Kind::Artists => {
                    let a = &self.artists[idx];
                    Row {
                        favourite: self.fav_artists.contains(&a.id),
                        lead: "",
                        name: &a.name,
                        artist: None,
                        detail: None,
                        tail: "",
                    }
                    .draw(buf, &cols, row, theme, emphasis, hover, &mut links);
                    if cols.tail.w == ARTIST_TAIL {
                        let quiet = emphasis.quiet(theme);
                        let end = cols.tail.x + cols.tail.w;
                        let albums = super::text::count(a.album_count.max(0) as u64, "album");
                        let tracks = super::text::count(a.track_count.max(0) as u64, "track");
                        rows::put_right(buf, end - 12, row.y, 10, &albums, quiet);
                        rows::put_right(buf, end, row.y, 11, &tracks, quiet);
                    }
                }
            }
        }
        self.geom.links = links;
        if bar {
            self.scrollbar(body, buf, theme, n, rows_on);
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
        self.read_ahead();
    }
}

/// The tiles' sizes across `width` cells and down `height`: how many go
/// across, how wide and tall each is, and how many rows fit whole. As the
/// apps' grids: the fewest across that keep each within the largest tile, as
/// long as each stays at least the smallest.
pub(super) fn tile_grid(width: u16, height: u16, gfx: &Gfx) -> (usize, u16, u16, usize) {
    let cols = width.div_ceil(TILE_MAX_W).min(width / TILE_MIN_W).max(1) as usize;
    let tile_w = (width / cols as u16).min(TILE_MAX_W);
    let tile_h = gfx.rows_for(tile_w.saturating_sub(2).max(1)) + 3;
    let rows = ((height / tile_h) as usize).max(1);
    (cols, tile_w, tile_h, rows)
}

/// One album as a tile: its cover, and under it its title and, when
/// `name_artist`, its artist (else its year). Both names are links.
pub(super) struct Tile<'a> {
    pub album: &'a AlbumRow,
    pub favourite: bool,
    pub selected: bool,
    pub name_artist: bool,
}

impl Tile<'_> {
    /// Draws the tile at `at`, cut at `bottom`; covers are drawn as images
    /// only while not `moving`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw(
        &self,
        buf: &mut Buffer,
        theme: &Theme,
        gfx: &mut Gfx,
        thumbs: &mut Thumbs,
        at: Rect,
        bottom: u16,
        moving: bool,
        hover: Option<Target>,
        links: &mut Vec<(Rect, Target)>,
    ) {
        let (x, y) = (at.x, at.y);
        let album = self.album;
        let art_w = at.width.saturating_sub(2).max(1);
        let art_h = at.height.saturating_sub(3);
        let art = Rect::new(x + 1, y, art_w, art_h);
        let art_visible = bottom.saturating_sub(y).min(art_h);
        if let Some(img) = thumbs.get(album.id) {
            gfx.draw_clipped(album_key(album.id), img, art, art_visible, moving, buf);
        } else {
            placeholder(art, art_visible, buf, theme, &album.title);
        }
        if self.selected {
            for dy in 0..art_visible {
                buf[(x, y + dy)].set_char('\u{258c}').set_style(theme.bar);
            }
        }
        let label = at.width.saturating_sub(2);
        let title_y = y + art_h;
        if title_y < bottom {
            let style = if self.selected {
                theme.track_cursor
            } else {
                theme.track_normal
            };
            let target = Target::Album(album.id);
            // A favourite wears a heart before its title.
            let (from, width) = if self.favourite && label > 3 {
                buf.set_string(x + 1, title_y, "\u{2665}", theme.favourite);
                (3, label - 2)
            } else {
                (1, label)
            };
            let drawn = rows::put(
                buf,
                x + from,
                title_y,
                width,
                &album.title,
                rows::link(style, hover == Some(target)),
            );
            links.push((drawn, target));
        }
        if title_y + 1 < bottom {
            let quiet = if self.selected {
                theme.track_playing
            } else {
                theme.hint_desc
            };
            let year = year(album);
            if self.name_artist {
                // The artist is a link; the year after it is not.
                let target = Target::Artist(album.artist_id);
                let drawn = rows::put(
                    buf,
                    x + 1,
                    title_y + 1,
                    label,
                    &album.artist_name,
                    rows::link(quiet, hover == Some(target)),
                );
                links.push((drawn, target));
                let after = drawn.x + drawn.width;
                if !year.is_empty() && after + 7 <= x + 1 + label {
                    buf.set_string(
                        after,
                        title_y + 1,
                        format!(" \u{b7} {year}"),
                        theme.hint_desc,
                    );
                }
            } else {
                rows::put(buf, x + 1, title_y + 1, label, &year, theme.hint_desc);
            }
        }
    }
}

/// The first of `rows` shown in a list scrolled to `scroll`, moved only as
/// far as brings `cursor` into view, and kept for the next frame and the
/// pointer.
pub(super) fn scroll_to(scroll: &Cell<usize>, cursor: usize, rows: usize) -> usize {
    let rows = rows.max(1);
    let mut top = scroll.get();
    if cursor < top {
        top = cursor;
    } else if cursor >= top + rows {
        top = cursor + 1 - rows;
    }
    scroll.set(top);
    top
}

/// An album row's tail: the year, a gap, the codec.
const ALBUM_TAIL: u16 = 11;

/// An artist row's tail: "12 albums", a gap, "340 tracks".
const ARTIST_TAIL: u16 = 23;

fn year(album: &AlbumRow) -> String {
    album_year(album.date.as_deref()).unwrap_or_default()
}

/// Leading year of a free-form album date tag. Tags are arbitrary text: this
/// counts characters, not bytes, and yields nothing when there aren't four.
pub fn album_year(date: Option<&str>) -> Option<String> {
    let year: String = date?.chars().take(4).collect();
    (year.chars().count() == 4).then_some(year)
}

/// A tile with no cover (yet): the album's first letter on the raised
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
pub(super) mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    pub fn album(id: i64, title: &str, artist: &str) -> AlbumRow {
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

    pub fn track(id: i64, album: i64) -> TrackRow {
        TrackRow {
            id,
            album_id: Some(album),
            artist_id: Some(album),
            artist_name: "artist".into(),
            album_artist_name: "artist".into(),
            album_title: format!("album {album}"),
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
                .map(|i| album(i, &format!("album {i}"), "artist"))
                .collect(),
            ..Default::default()
        }
    }

    /// A page of `source` as if the reader had answered with `listing`.
    fn page(source: Source, listing: Listing) -> Browse {
        koan_core::config::isolate_config_for_tests();
        let mut b = Browse::new(source);
        b.stale = false;
        b.asked = 1;
        b.show(1, listing);
        b
    }

    fn browse(n: usize) -> Browse {
        page(Source::Albums, albums(0..n as i64))
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
    fn a_tracks_menu_is_the_apps_track_menu() {
        let listing = Listing {
            tracks: (0..3).map(|i| track(i, 7)).collect(),
            track_total: 3,
            ..Default::default()
        };
        let mut b = page(Source::Tracks, listing);
        b.fav_tracks.insert(1);
        let Action::Menu(title, items) = b.menu(1) else {
            panic!("a menu");
        };
        assert_eq!(title, "song 1");
        assert_eq!(items, playable(&track_playable(&b.tracks[1], true)));
    }

    #[test]
    fn an_album_without_a_cover_is_looked_at_again_after_a_sync() {
        let mut t = Thumbs::new();
        t.images.insert(1, None);
        t.images.insert(2, Some(DynamicImage::new_rgb8(1, 1)));
        t.retry_missing();
        assert!(
            !t.images.contains_key(&1),
            "asked for again when next drawn"
        );
        assert!(t.images.contains_key(&2), "a cover that was found is kept");
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
    fn a_click_selects_and_the_second_opens_the_album() {
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
        assert_eq!(b.hit(x, y), Some(Hit::Link(Target::Album(0))));
        assert_eq!(b.click(Hit::Link(Target::Album(0))), Action::OpenAlbum(0));
        let (x, y) = first_tile(&b, 2, art_h + 1);
        assert_eq!(b.hit(x, y), Some(Hit::Link(Target::Artist(0))));
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
        assert_eq!(b.hit(x, y), Some(Hit::Link(Target::Album(0))));
        // "album 0" is 7 cells from column 3: the last is 9, 10 is past it.
        let (x, y) = first_tile(&b, 10, art_h);
        assert_eq!(b.hit(x, y), Some(Hit::Item(0)));
    }

    #[test]
    fn the_toolbar_offers_each_control_as_its_key() {
        let mut b = browse(3);
        draw(&mut b, 100, 24);
        let keys: Vec<_> = b.toolbar().items.iter().map(|i| i.key.code).collect();
        for c in ['/', 'f', 'o', 'v', 'm'] {
            assert!(keys.contains(&KeyCode::Char(c)), "{c}");
        }
    }

    #[test]
    fn random_offers_a_reshuffle_and_s_deals_again() {
        let mut b = browse(3);
        while b.album_sort != AlbumSort::Random {
            b.cycle_sort();
        }
        draw(&mut b, 120, 24);
        assert!(
            b.toolbar()
                .items
                .iter()
                .any(|i| i.key.code == KeyCode::Char('S'))
        );
        let (seed, asked) = (b.seed, b.asked);
        b.handle_key(key(KeyCode::Char('S')));
        assert_ne!(b.seed, seed);
        assert_eq!(b.asked, asked + 1);
    }

    #[test]
    fn ctrl_f_opens_the_filters_on_the_browsers_only() {
        let ctrl_f = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
        let mut b = browse(3);
        assert_eq!(b.handle_key(ctrl_f), Some(Action::OpenFilters));
        let mut b = page(Source::Artist(1), albums(0..3));
        assert_eq!(b.handle_key(ctrl_f), Some(Action::None));
    }

    #[test]
    fn f_favourites_the_item_and_h_its_artist() {
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
    fn a_favourite_album_wears_a_heart() {
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
        draw(&mut b, 80, 60);
        // Tall enough for a whole row and three lines of the next.
        let h = b.geom.body.y + b.geom.tile_h + 3;
        let buf = draw(&mut b, 80, h);
        let theme = Theme::default();
        let second_row_y = b.geom.body.y + b.geom.tile_h;
        assert_eq!(b.geom.rows, 1);
        let drawn =
            (second_row_y..h).any(|y| (0..80).any(|x| buf[(x, y)].bg == theme.tokens.surface));
        assert!(drawn);
    }

    #[test]
    fn a_listing_keeps_the_cursor_on_its_album() {
        let mut b = browse(10);
        b.move_to(4);
        b.asked = 2;
        // Two albums ahead of it are gone, and one is new at the end.
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
    fn an_artist_with_no_albums_lists_what_they_appear_on() {
        let listing = Listing {
            tracks: (0..3).map(|i| track(i, 9)).collect(),
            track_total: 3,
            ..Default::default()
        };
        let mut b = page(Source::Artist(7), listing);
        assert_eq!(b.kind(), Kind::Tracks);
        assert_eq!(b.len(), 3);
        b.move_to(1);
        assert_eq!(
            b.handle_key(key(KeyCode::Enter)),
            Some(Action::Play(vec![0, 1, 2], PickerAction::ReplaceFrom(1)))
        );
    }

    #[test]
    fn enter_on_a_track_plays_the_list_from_it() {
        let listing = Listing {
            tracks: (0..5).map(|i| track(i, i)).collect(),
            track_total: 5,
            ..Default::default()
        };
        let mut b = page(Source::Tracks, listing);
        draw(&mut b, 100, 20);
        b.move_to(3);
        assert_eq!(
            b.handle_key(key(KeyCode::Enter)),
            Some(Action::Play(
                vec![0, 1, 2, 3, 4],
                PickerAction::ReplaceFrom(3)
            ))
        );
        // Each row's artist and album are links.
        assert!(b.geom.links.iter().any(|(_, t)| *t == Target::Artist(0)));
        assert!(b.geom.links.iter().any(|(_, t)| *t == Target::Album(0)));
    }

    #[test]
    fn tracks_are_read_a_page_at_a_time() {
        let listing = Listing {
            tracks: (0..TRACK_PAGE as i64).map(|i| track(i, 1)).collect(),
            track_total: 5000,
            ..Default::default()
        };
        let mut b = page(Source::Tracks, listing);
        draw(&mut b, 100, 20);
        let asked = b.asked;
        b.move_to(100);
        assert_eq!(b.asked, asked, "far from the end, nothing more is asked");
        b.move_to(TRACK_PAGE as usize - 5);
        assert_eq!(b.asked, asked + 1);
        assert_eq!(b.limit, TRACK_PAGE * 2);
    }

    #[test]
    fn the_columns_of_a_track_list_line_up() {
        let listing = Listing {
            tracks: vec![
                TrackRow {
                    title: "日本語のタイトルがとても長い曲".into(),
                    artist_name: "アーティスト".into(),
                    ..track(1, 1)
                },
                TrackRow {
                    title: "short".into(),
                    artist_name: "Someone With A Very Long Name Indeed".into(),
                    ..track(2, 2)
                },
            ],
            track_total: 2,
            ..Default::default()
        };
        let mut b = page(Source::Tracks, listing);
        draw(&mut b, 120, 20);
        let artists: Vec<u16> = b
            .geom
            .links
            .iter()
            .filter(|(_, t)| matches!(t, Target::Artist(_)))
            .map(|(r, _)| r.x)
            .collect();
        assert_eq!(artists.len(), 2);
        assert_eq!(artists[0], artists[1], "artists start in one column");
    }

    #[test]
    fn it_draws_at_any_size() {
        for source in [Source::Albums, Source::Tracks, Source::Artists] {
            let listing = Listing {
                albums: albums(0..30).albums,
                tracks: (0..30).map(|i| track(i, i)).collect(),
                artists: (0..30)
                    .map(|i| ArtistRow {
                        id: i,
                        name: format!("artist {i}"),
                        sort_name: None,
                        remote_id: None,
                        album_count: 2,
                        track_count: 20,
                    })
                    .collect(),
                track_total: 30,
                ..Default::default()
            };
            let mut b = page(source, listing);
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
}
