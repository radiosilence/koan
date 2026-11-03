//! An artist's page: the photograph from their Wikipedia article beside
//! their name, play as the one prominent action, their records under a
//! heading per kind of release, the records they feature on, then the
//! article's opening under "about", as the Mac's page has it. An artist with
//! no records of their own shows the tracks they appear on instead.
//!
//! The photograph and biography usually arrive after the page: the photo's
//! slot grows to make room, moving the title along, as the photo fades in
//! (`Reveal`); the biography fades in where a spinner stood.
//!
//! A list of variable-height items (the header, headings, rows of tiles,
//! track rows), so only what is on screen is built, however many records an
//! artist features on. Every item's height but the header's is fixed, which
//! is what lets the page rank sleeves by their distance from the screen.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use koan_kit::app::*;
use koan_runtime::{Album, AlbumSort, Artist, ArtistInfo, BrowseFilter, ReleaseSection, Track};
// virtualised lists and their scroll state, which GPUI owns and koan-kit's rows sit in.
use crate::components::{ListAlignment, ListOffset, ListState, list};

use super::covers::{Art, Covers, Key, Priority, Sleeve, UPLOAD_BYTES_PER_FRAME};
use super::drag::{Playable, PlayableDrag};
use super::nav::{Anchor, Anchored, Heading, Page};
use super::rows::{self, RowPart, RowState, TrackList};
use super::selection::Selection;
use super::tile::{Grid, Part, Record as TileRecord, TileHost, TileState, Tiles};
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome};
use crate::components::{Cover, CoverArt, FadeIn, Reveal, RowMetrics, SectionHeading, Spinner};
use crate::theme::{Motion, Space, TypeRole};

/// A biography longer than this opens clamped, with "more" under it.
const BIO_CLAMP: usize = 300;
/// Until the header has been laid out once.
const HEADER_GUESS: f32 = 320.0;

struct Record {
    artist: Option<Artist>,
    albums: Vec<Album>,
    features: Vec<Album>,
    appearances: Vec<Track>,
}

/// One item of the page's list.
#[derive(Clone)]
enum Item {
    Header,
    Heading(SharedString, Heading),
    FeaturesHeading,
    /// A row of tiles: `albums[range]` of the records or the features.
    Tiles {
        features: bool,
        range: std::ops::Range<usize>,
    },
    Track(usize),
    /// The biography, or a spinner while it is looked up: always last.
    About,
}

pub struct ArtistPage {
    lib: Lib,
    artist_id: i64,
    record: Option<Rc<Record>>,
    info: Option<ArtistInfo>,
    /// The biography shown whole rather than its opening.
    expanded: bool,
    /// The photo, and the biography, became known after the page drew: the
    /// photo's slot grows in and the biography fades in, once. Cleared when
    /// the motion is over, so an item scrolled away and back (a list item is
    /// made again) stands still.
    photo_arriving: bool,
    bio_arriving: bool,
    _arrived: Option<Task<()>>,
    /// The biography is being looked up.
    looking_up: bool,
    loaded_version: Option<u64>,
    request: u64,
    _load: Option<Task<()>>,
    _enrich: Option<Task<()>>,
    width: Rc<Cell<Option<Pixels>>>,
    /// The items, and the grid they were cut for.
    items: Rc<Vec<Item>>,
    grid: Option<Grid>,
    list: ListState,
    /// Where to put the scroll back to the next time the page is cut: on
    /// arriving, after a library change, after a change of columns.
    restore: Option<Anchor>,
    selection: Selection,
    pick: Option<(u64, PlayableDrag)>,
    /// Each record's tile strings, made once per load.
    records: std::collections::HashMap<i64, Rc<TileRecord>>,
    tiles: Tiles<ArtistPage>,
    /// What the list's context menu acts on: the row last right-clicked.
    menu: Option<actions::MenuTarget>,
    starting: HashSet<i64>,
    focus: FocusHandle,
    /// The keyboard's place on the page, in `order`: the records, the
    /// features, then the appearances.
    cursor: crate::components::ListCursor,
    /// The page holds the keyboard, as of the last frame.
    keyed: bool,
    _subscriptions: Vec<Subscription>,
}

impl ArtistPage {
    pub fn new(lib: Lib, artist_id: i64, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        let subscriptions = vec![
            // Cached under the window, and draws the accent.
            crate::theme::observe_accent(cx),
            cx.observe(&lib.feed, |this, _, cx| {
                this.load(cx);
                cx.notify();
            }),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            cx.subscribe(&lib.covers, |this, _, gone: &super::covers::Evicted, cx| {
                let albums: HashSet<i64> = gone.albums.iter().copied().collect();
                let tracks: HashSet<i64> = gone.tracks.iter().copied().collect();
                this.selection.retain(|p| match p {
                    Playable::Album(id) => !albums.contains(&id),
                    Playable::Track(id) => !tracks.contains(&id),
                    Playable::Artist(_) => true,
                });
                cx.notify();
            }),
            cx.on_release(move |this, cx| {
                let page = Page::Artist(this.artist_id);
                if let Some(anchor) = this.anchor() {
                    this.lib
                        .nav
                        .update(cx, |nav, _| nav.remember_anchor(page, anchor));
                }
                this.lib.covers.update(cx, |c, cx| c.release(id, cx));
            }),
        ];
        let restore = lib.nav.read(cx).anchor(Page::Artist(artist_id));
        let mut this = Self {
            lib,
            artist_id,
            record: None,
            info: None,
            expanded: false,
            photo_arriving: false,
            bio_arriving: false,
            _arrived: None,
            looking_up: true,
            loaded_version: None,
            request: 0,
            _load: None,
            _enrich: None,
            width: Rc::new(Cell::new(None)),
            items: Rc::new(Vec::new()),
            grid: None,
            list: ListState::new(0, ListAlignment::Top, crate::components::OVERDRAW),
            restore,
            selection: Selection::default(),
            pick: None,
            records: std::collections::HashMap::new(),
            tiles: Tiles::default(),
            menu: None,
            starting: HashSet::new(),
            focus: cx.focus_handle().tab_stop(true),
            cursor: Default::default(),
            keyed: false,
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this.enrich(cx);
        this
    }

    /// Everything the page draws, read together and again when the library
    /// moves. The biography comes from the cache here; `enrich` asks further.
    fn load(&mut self, cx: &mut Context<Self>) {
        let version = self.lib.feed.read(cx).library_version;
        if version.is_none() || version == self.loaded_version {
            return;
        }
        self.loaded_version = version;
        self.request += 1;
        let request = self.request;
        let (engine, id) = (self.lib.engine.clone(), self.artist_id);
        self._load = Some(cx.spawn(async move |this, cx| {
            // Read together: each is its own query, and the page waits for all.
            let (artist, albums, features, info) = futures::join!(
                engine.clone().artist(id),
                engine
                    .clone()
                    .albums(Some(id), AlbumSort::Year, 0, None, BrowseFilter::default()),
                engine.clone().artist_features(id),
                engine.clone().artist_info(id),
            );
            let (artist, info) = (artist.ok().flatten(), info.ok().flatten());
            let (albums, features) = (albums.unwrap_or_default(), features.unwrap_or_default());
            // Only when there is nothing else to show: an artist with records
            // has their tracks one click away on each.
            let appearances = if albums.is_empty() {
                engine.tracks(None, Some(id)).await.unwrap_or_default()
            } else {
                Vec::new()
            };
            this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                // Held before the items change under it.
                if this.restore.is_none() {
                    this.restore = this.anchor();
                }
                this.records.clear();
                this.record = Some(Rc::new(Record {
                    artist,
                    albums,
                    features,
                    appearances,
                }));
                this.grid = None;
                if this.info.is_none() {
                    this.info = info;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The biography and photograph, from the cache while it is fresh and
    /// otherwise fetched: seconds on a miss, never waited on.
    fn enrich(&mut self, cx: &mut Context<Self>) {
        let (engine, id) = (self.lib.engine.clone(), self.artist_id);
        self._enrich = Some(cx.spawn(async move |this, cx| {
            let fetched = engine.fetch_artist_info(id).await.ok().flatten();
            this.update(cx, |this, cx| {
                this.looking_up = false;
                if let Some(fetched) = fetched {
                    let (had_photo, had_bio) = this
                        .info
                        .as_ref()
                        .map_or((false, false), |i| (i.has_image, i.bio.is_some()));
                    let drawn = this.record.is_some();
                    this.photo_arriving |= fetched.has_image && !had_photo && drawn;
                    this.bio_arriving |= fetched.bio.is_some() && !had_bio && drawn;
                    if this.photo_arriving || this.bio_arriving {
                        let timer = cx.background_executor().timer(Motion::NORMAL);
                        this._arrived = Some(cx.spawn(async move |this, cx| {
                            timer.await;
                            this.update(cx, |this, _| {
                                this.photo_arriving = false;
                                this.bio_arriving = false;
                            })
                            .ok();
                        }));
                    }
                    this.info = Some(fetched);
                    this.list.remeasure_items(0..1);
                }
                let last = this.items.len();
                this.list.remeasure_items(last.saturating_sub(1)..last);
                cx.notify();
            })
            .ok();
        }));
    }

    /// The items for `record` cut into rows of `grid`.
    fn cut(record: &Record, grid: Grid) -> Vec<Item> {
        let mut items = vec![Item::Header];
        let rows = |items: &mut Vec<Item>, features: bool, from: usize, len: usize| {
            let mut start = from;
            while start < from + len {
                let end = (start + grid.columns).min(from + len);
                items.push(Item::Tiles {
                    features,
                    range: start..end,
                });
                start = end;
            }
        };
        // Headed even when there is one section, so a discography of albums
        // alone still says what it is.
        for (section, start, len) in sections(&record.albums) {
            // In the release's own words; the heading puts them in the
            // theme's case.
            items.push(Item::Heading(
                koan_runtime::release_section_title(section).into(),
                Heading::Section(section),
            ));
            rows(&mut items, false, start, len);
        }
        if !record.features.is_empty() {
            items.push(Item::FeaturesHeading);
            rows(&mut items, true, 0, record.features.len());
        }
        if !record.appearances.is_empty() {
            items.push(Item::Heading("appears on".into(), Heading::Appearances));
            items.extend((0..record.appearances.len()).map(Item::Track));
        }
        items.push(Item::About);
        items
    }

    /// A heading's line, with the space above it and below.
    fn heading_height(p: &Paint) -> Pixels {
        SectionHeading::height(&p.look)
    }

    fn height(&self, p: &Paint, item: &Item, grid: Grid) -> f32 {
        f32::from(match item {
            Item::Header => self
                .list
                .bounds_for_item(0)
                .map_or(Pixels::from(HEADER_GUESS), |b| b.size.height),
            Item::Heading(..) | Item::FeaturesHeading => Self::heading_height(p),
            Item::Tiles { .. } => grid.row,
            Item::Track(_) => rows::art_height(p),
            Item::About => self
                .list
                .bounds_for_item(self.items.len().saturating_sub(1))
                .map_or(Pixels::ZERO, |b| b.size.height),
        })
    }

    /// The item at the top of the screen, by what it shows.
    fn anchor(&self) -> Option<Anchor> {
        let record = self.record.as_ref()?;
        let top = self.list.logical_scroll_top();
        let item = self.items.get(top.item_ix)?;
        let at = match item {
            Item::Header => Anchored::Header,
            Item::Heading(_, heading) => Anchored::Heading(*heading),
            Item::FeaturesHeading => Anchored::Heading(Heading::Features),
            Item::Tiles { features, range } => {
                let albums = if *features {
                    &record.features
                } else {
                    &record.albums
                };
                Anchored::Album {
                    id: albums.get(range.start)?.id,
                    features: *features,
                }
            }
            Item::Track(i) => Anchored::Track(record.appearances.get(*i)?.id),
            Item::About => return None,
        };
        Some(Anchor {
            at,
            offset: top.offset_in_item,
            item_ix: top.item_ix,
        })
    }

    /// The item that shows `anchor` in the items as now cut.
    fn find(&self, record: &Record, anchor: Anchor) -> ListOffset {
        let item_ix = self.items.iter().position(|item| match (item, anchor.at) {
            (Item::Header, Anchored::Header) => true,
            (Item::Heading(_, heading), Anchored::Heading(h)) => *heading == h,
            (Item::FeaturesHeading, Anchored::Heading(Heading::Features)) => true,
            (Item::Tiles { features, range }, Anchored::Album { id, features: f })
                if *features == f =>
            {
                let albums = if *features {
                    &record.features
                } else {
                    &record.albums
                };
                albums
                    .get(range.clone())
                    .is_some_and(|a| a.iter().any(|a| a.id == id))
            }
            (Item::Track(i), Anchored::Track(id)) => {
                record.appearances.get(*i).is_some_and(|t| t.id == id)
            }
            _ => false,
        });
        match item_ix {
            Some(item_ix) => ListOffset {
                item_ix,
                offset_in_item: anchor.offset,
            },
            // Gone from the page (a record the library lost): the place it
            // was, rather than the top.
            None => ListOffset {
                item_ix: anchor.item_ix.min(self.items.len().saturating_sub(1)),
                offset_in_item: Pixels::ZERO,
            },
        }
    }

    /// How far down the page is scrolled, in points.
    fn offset(&self, p: &Paint) -> Pixels {
        let Some(grid) = self.grid else {
            return Pixels::ZERO;
        };
        let top = self.list.logical_scroll_top();
        let above: f32 = self.items[..top.item_ix.min(self.items.len())]
            .iter()
            .map(|i| self.height(p, i, grid))
            .sum();
        Pixels::from(above) + top.offset_in_item
    }

    /// Each item's distance from the screen, ranked as the grid ranks its
    /// rows: on screen, then nearest first, up to two screens away. Until the
    /// header is laid out its height is a guess, but the scroll position and
    /// every item's place are measured with the same guess, so it cancels
    /// for everything below the header.
    fn aim(&mut self, p: &Paint, grid: Grid, scale: f32, cx: &mut Context<Self>) {
        let Some(record) = self.record.clone() else {
            return;
        };
        let viewport = f32::from(self.list.viewport_bounds().size.height).max(1.0);
        let top = f32::from(self.offset(p));
        let bottom = top + viewport;
        let mut y = 0.0;
        let mut ranked = Vec::new();
        for item in self.items.iter() {
            let h = self.height(p, item, grid);
            let distance = (y - bottom).max(top - (y + h)).max(0.0);
            y += h;
            if distance > 2.0 * viewport {
                continue;
            }
            let priority = if distance == 0.0 {
                Priority::Visible
            } else {
                Priority::Ahead(distance)
            };
            match item {
                Item::Header if self.info.as_ref().is_some_and(|i| i.has_image) => {
                    ranked.push((
                        Key::new(
                            Art::Artist(self.artist_id),
                            RowMetrics::artist_photo(&p.look),
                            scale,
                        ),
                        priority,
                    ));
                }
                Item::Tiles { features, range } => {
                    let albums = if *features {
                        &record.features
                    } else {
                        &record.albums
                    };
                    for album in albums.get(range.clone()).unwrap_or_default() {
                        ranked.push((Key::new(Art::Album(album.id), grid.side, scale), priority));
                    }
                }
                Item::Track(i) => {
                    if let Some(album) = record.appearances.get(*i).and_then(|t| t.album_id) {
                        ranked.push((
                            Key::new(Art::Album(album), Pixels::from(rows::SLEEVE), scale),
                            priority,
                        ));
                    }
                }
                _ => {}
            }
        }
        let owner = cx.entity_id();
        self.lib.covers.update(cx, |c, _| c.request(owner, ranked));
    }

    fn pick(&mut self) -> Option<PlayableDrag> {
        if !self.selection.is_active() || self.selection.picked().is_empty() {
            return None;
        }
        let revision = self.selection.revision();
        if self.pick.as_ref().is_none_or(|(r, _)| *r != revision) {
            let picked = self.selection.picked();
            let label = chrome::count(picked.len() as u64, "item", "items");
            self.pick = Some((revision, PlayableDrag::new(picked, label)));
        }
        self.pick.as_ref().map(|(_, drag)| drag.clone())
    }

    /// The page's rows as the keys walk them: each list item holding tiles
    /// or a track, and the part of `order` it shows.
    fn lines(&self) -> Vec<(usize, std::ops::Range<usize>)> {
        let Some(r) = &self.record else {
            return Vec::new();
        };
        let (albums, features) = (r.albums.len(), r.albums.len() + r.features.len());
        self.items
            .iter()
            .enumerate()
            .filter_map(|(ix, item)| match item {
                Item::Tiles {
                    features: false,
                    range,
                } => Some((ix, range.clone())),
                Item::Tiles {
                    features: true,
                    range,
                } => Some((ix, albums + range.start..albums + range.end)),
                Item::Track(t) => Some((ix, features + t..features + t + 1)),
                _ => None,
            })
            .collect()
    }

    fn order(&self) -> Vec<Playable> {
        let Some(r) = &self.record else {
            return Vec::new();
        };
        r.albums
            .iter()
            .chain(&r.features)
            .map(|a| Playable::Album(a.id))
            .chain(r.appearances.iter().map(|t| Playable::Track(t.id)))
            .collect()
    }

    fn item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = &Paint::of(window, cx);
        let (Some(record), Some(grid), Some(item)) =
            (self.record.clone(), self.grid, self.items.get(ix).cloned())
        else {
            return div().into_any_element();
        };
        let scale = window.scale_factor();
        match item {
            Item::Header => self.header(p, &record, grid, scale, cx).into_any_element(),
            Item::About => self.about(p, cx),
            Item::Heading(title, _) => SectionHeading::new(title).into_any_element(),
            Item::FeaturesHeading => {
                let items: Vec<Playable> = record
                    .features
                    .iter()
                    .map(|a| Playable::Album(a.id))
                    .collect();
                let (lib, items2, lib2) = (self.lib.clone(), items.clone(), self.lib.clone());
                SectionHeading::new("features on")
                    .trailing(
                        chrome::text_button("features-play", "play").on_click(move |_, _, cx| {
                            actions::play(&lib, &items, false, cx).detach()
                        }),
                    )
                    .trailing(
                        chrome::text_button("features-shuffle", "shuffle").on_click(
                            move |_, _, cx| actions::play(&lib2, &items2, true, cx).detach(),
                        ),
                    )
                    .into_any_element()
            }
            Item::Tiles { features, range } => {
                let (albums, first) = if features {
                    (&record.features, record.albums.len() + range.start)
                } else {
                    (&record.albums, range.start)
                };
                let Some(albums) = albums.get(range) else {
                    return div().into_any_element();
                };
                self.tiles(albums, first, features, grid, scale, cx)
                    .into_any_element()
            }
            Item::Track(index) => {
                let Some(track) = record.appearances.get(index) else {
                    return div().into_any_element();
                };
                let item = Playable::Track(track.id);
                let selected = self.selection.contains(item);
                let drag = match (selected, self.pick()) {
                    (true, Some(pick)) => pick,
                    _ => {
                        PlayableDrag::new(&[item], track.title.clone()).with_sleeve(track.album_id)
                    }
                };
                let sleeve = track.album_id.map(|album| {
                    let key = Key::new(Art::Album(album), Pixels::from(rows::SLEEVE), scale);
                    self.lib.covers.update(cx, |c, _| c.sleeve(key))
                });
                let feed = self.lib.feed.read(cx);
                let state = RowState {
                    lead: (index + 1).to_string().into(),
                    lead_width: rows::lead_width(p, record.appearances.len(), window),
                    current: feed.playing_track == Some(track.id),
                    favourite: feed.is_favourite(item),
                    selected,
                    gathered: true,
                    transfer: feed.transfers.get(&track.id).copied(),
                    drag,
                    kind: crate::components::RowKind::Art,
                    note: None,
                    cursor: self.keyed
                        && self.cursor.at
                            == Some(record.albums.len() + record.features.len() + index),
                    place: Some((index, record.appearances.len())),
                    reveal: None,
                };
                // A row insets itself by the page's margin, as the body does.
                div()
                    .mx(-RowMetrics::page_margin(&p.look))
                    .child(rows::track_row(
                        p,
                        track,
                        index,
                        state,
                        sleeve,
                        Some(self.artist_id),
                        cx,
                    ))
                    .into_any_element()
            }
        }
    }

    fn tiles(
        &mut self,
        albums: &[Album],
        first: usize,
        show_artist: bool,
        grid: Grid,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let pick = self.pick();
        let revision = self.selection.revision();
        let entity = cx.entity();
        let after: actions::After = Some(Rc::new(move |cx: &mut App| {
            entity.update(cx, |this, cx| {
                this.selection.end();
                cx.notify();
            })
        }));
        let height = grid.tile_height();
        let mut tiles = Vec::with_capacity(albums.len());
        let count = albums.len();
        for (ix, album) in albums.iter().enumerate() {
            let record = self
                .records
                .entry(album.id)
                .or_insert_with(|| Rc::new(TileRecord::of(album)))
                .clone();
            let item = Playable::Album(album.id);
            let selected = self.selection.contains(item);
            let (drag, pick, after) = match (&pick, selected) {
                (Some(pick), true) => (pick.clone(), Some(revision), after.clone()),
                _ => (PlayableDrag::new(&[item], record.title.clone()), None, None),
            };
            let key = Key::new(Art::Album(album.id), grid.side, scale);
            let sleeve = self
                .lib
                .covers
                .update(cx, |c: &mut Covers, _| c.sleeve(key));
            let feed = self.lib.feed.read(cx);
            let state = TileState {
                favourite: feed.is_favourite(item),
                playing: feed.playing_album == Some(album.id),
                arriving: feed.arriving.contains_key(&album.id),
                selecting: self.selection.is_active(),
                selected,
                starting: self.starting.contains(&album.id),
                cursor: self.keyed && self.cursor.at == Some(first + ix),
                show_artist,
                side: grid.side,
                sleeve,
                drag,
                pick,
                after,
                place: (ix, count),
            };
            tiles.push(self.tiles.tile(&record, state, height, cx));
        }
        // Inside the page's margin already: only the grid's share of the
        // spare width is added.
        div()
            .flex()
            .gap(grid.gap)
            .pl(grid.inset - grid.margin)
            .h(grid.row)
            .children(tiles)
    }

    fn header(
        &mut self,
        p: &Paint,
        record: &Record,
        _grid: Grid,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = self.artist_id;
        let item = Playable::Artist(id);
        let name = record
            .artist
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        // No box until the artist is known to have a photo: the header
        // starts at the page's edge. Once known, the box holds its place with
        // the placeholder in it, so the photo itself landing moves nothing;
        // a photo known only after the page drew grows its box in.
        let side = RowMetrics::artist_photo(&p.look);
        let arriving = self.photo_arriving;
        // The box stays only while a photo may yet come: one the covers found
        // there is none of (refused, its address gone) gives the header its
        // room back.
        let no_photo = self
            .lib
            .covers
            .read(cx)
            .is_absent(Art::Artist(self.artist_id));
        let photo = self
            .info
            .as_ref()
            .is_some_and(|i| i.has_image && !no_photo)
            .then(|| {
                let art = self.photo(side, scale, cx).unwrap_or(CoverArt::Missing);
                Reveal::new(
                    ("artist-photo", id as u64),
                    Size {
                        width: side,
                        height: side,
                    },
                    Cover::new(art, side).id(("artist-photo-cover", id as u64)),
                )
                .arriving(arriving)
            });
        // An artist with no records of their own is counted by the tracks
        // they appear on, which the page lists instead.
        let detail = match record.albums.len() as u64 {
            0 => chrome::count(record.appearances.len() as u64, "track", "tracks"),
            albums => chrome::count(albums, "album", "albums"),
        };
        let mut header = super::header::playable_header(&self.lib, item, name, cx).detail(detail);
        if let Some(photo) = photo {
            header = header.art(photo);
        }
        div()
            .w_full()
            .pt(RowMetrics::page_top(&p.look))
            .pb(RowMetrics::header_gap(&p.look))
            .child(header)
    }

    /// The article's opening, under "about" after the records, as the Mac's
    /// page has it; a spinner while it is looked up; nothing when there is
    /// none.
    fn about(&mut self, p: &Paint, cx: &mut Context<Self>) -> AnyElement {
        let id = self.artist_id;
        let Some(bio) = self.info.as_ref().and_then(|i| i.bio.clone()) else {
            if !self.looking_up {
                return div().into_any_element();
            }
            return div()
                .pt(p.space(Space::L))
                .flex()
                .items_center()
                .gap(p.space(Space::S))
                .typeset(p, Type::Control, p.muted)
                .child(Spinner::new(p.look.font_size(TypeRole::Control)))
                .child("looking up biography…")
                .into_any_element();
        };
        let info = self.info.as_ref();
        let long = bio.len() > BIO_CLAMP;
        let mut text = div()
            .max_w(p.look.spaced(680.0))
            .typeset(p, Type::Control, p.muted)
            .child(bio.replace('\n', "\n\n"));
        if long && !self.expanded {
            text = text.line_clamp(4);
        }
        let links = div()
            .flex()
            .gap(p.space(Space::M))
            .typeset(p, Type::Fine, p.muted)
            .when(long && !self.expanded, |d| {
                d.child(
                    div()
                        .id("bio-more")
                        .cursor_pointer()
                        .underline()
                        .child("more")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.expanded = true;
                            let last = this.items.len();
                            this.list.remeasure_items(last.saturating_sub(1)..last);
                            cx.notify();
                        })),
                )
            })
            .when_some(info.and_then(|i| i.bio_url.clone()), |d, url| {
                d.child(
                    div()
                        .id("bio-source")
                        .cursor_pointer()
                        .underline()
                        .child("from wikipedia")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .when_some(info.and_then(|i| i.image_credit.clone()), |d, credit| {
                d.child(format!("photo: {credit}"))
            });
        div()
            .pb(RowMetrics::page_top(&p.look))
            .child(SectionHeading::new("about"))
            .child({
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(p.space(Space::S))
                    .child(text)
                    .child(links);
                if self.bio_arriving {
                    FadeIn::new(("artist-bio", id as u64), body).into_any_element()
                } else {
                    body.into_any_element()
                }
            })
            .into_any_element()
    }

    /// The artist's photo, once it has been found and decoded.
    fn photo(&mut self, side: Pixels, scale: f32, cx: &mut Context<Self>) -> Option<CoverArt> {
        if !self.info.as_ref().is_some_and(|i| i.has_image) {
            return None;
        }
        let key = Key::new(Art::Artist(self.artist_id), side, scale);
        match self.lib.covers.update(cx, |c, _| c.sleeve(key)) {
            Sleeve::Ready(image, _) => Some(CoverArt::Image(image)),
            Sleeve::Colour(colour) => Some(CoverArt::Colour(colour)),
            Sleeve::Waiting => Some(CoverArt::Loading),
            Sleeve::None => None,
        }
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let order = self.order();
        self.selection.select_all(&order);
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selection.is_clear() {
            self.selection.end();
            cx.notify();
        }
    }
}

/// The sections of an artist's records, in page order, as (section, start,
/// length): the engine lists them section by section, so a section is a run.
fn sections(albums: &[Album]) -> Vec<(ReleaseSection, usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=albums.len() {
        if i == albums.len() || albums[i].release_section != albums[start].release_section {
            out.push((albums[start].release_section, start, i - start));
            start = i;
        }
    }
    out
}

impl actions::Menus for ArtistPage {
    fn lib(&self) -> &Lib {
        &self.lib
    }

    fn menu_target(&mut self) -> &mut Option<actions::MenuTarget> {
        &mut self.menu
    }

    fn menu_target_ref(&self) -> &Option<actions::MenuTarget> {
        &self.menu
    }
}

impl TileHost for ArtistPage {
    fn clicked(
        &mut self,
        album: i64,
        part: Part,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let item = Playable::Album(album);
        let order = self.order();
        // The keyboard carries on from the tile clicked.
        if let Some(at) = order.iter().position(|p| *p == item) {
            self.cursor.at = Some(at);
            self.cursor.anchor = Some(at);
        }
        if self
            .selection
            .take(item, modifiers.shift, modifiers.secondary(cx), &order)
        {
            cx.notify();
            return;
        }
        match part {
            Part::Sleeve => {
                self.starting.insert(album);
                cx.notify();
                let started = actions::play(&self.lib, &[item], false, cx);
                self.lib
                    .nav
                    .update(cx, |nav, cx| nav.open(Page::Album(album), cx));
                cx.spawn(async move |this, cx| {
                    started.await;
                    this.update(cx, |this, cx| {
                        this.starting.remove(&album);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            Part::Title => self
                .lib
                .nav
                .update(cx, |nav, cx| nav.open(Page::Album(album), cx)),
            Part::Artist(artist) => self
                .lib
                .nav
                .update(cx, |nav, cx| nav.open(Page::Artist(artist), cx)),
            Part::Heart => self
                .lib
                .feed
                .update(cx, |f, cx| f.toggle_favourite(item, cx)),
        }
    }
}

impl TrackList for ArtistPage {
    fn row_tracks(&self) -> Vec<i64> {
        self.record
            .as_ref()
            .map(|r| r.appearances.iter().map(|t| t.id).collect())
            .unwrap_or_default()
    }

    fn clicked(
        &mut self,
        index: usize,
        track: i64,
        part: RowPart,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        match part {
            RowPart::Row => {
                if let Some(r) = &self.record {
                    let at = r.albums.len() + r.features.len() + index;
                    self.cursor.at = Some(at);
                    self.cursor.anchor = Some(at);
                }
                let ids: Vec<i64> = self
                    .record
                    .as_ref()
                    .map(|r| r.appearances.iter().map(|t| t.id).collect())
                    .unwrap_or_default();
                let item = Playable::Track(track);
                let m = event.modifiers();
                // Outside the mode a plain click marks the row alone: the
                // mode would turn every tile's click into a tick.
                let plays = if !m.shift && !m.secondary(cx) && !self.selection.is_active() {
                    let double = event.click_count() >= 2;
                    if !double {
                        self.selection.mark(item);
                    }
                    double
                } else {
                    let order = || ids.iter().map(|id| Playable::Track(*id)).collect();
                    rows::select(&mut self.selection, item, event, order, cx)
                };
                if plays {
                    actions::play_list(&self.lib, ids, index, cx);
                }
                cx.notify();
            }
            part => part.follow(track, &self.lib, cx),
        }
    }
}

impl super::keys::KeyedList for ArtistPage {
    fn cursor(&mut self) -> &mut crate::components::ListCursor {
        &mut self.cursor
    }

    fn len(&self) -> usize {
        self.record.as_ref().map_or(0, |r| {
            r.albums.len() + r.features.len() + r.appearances.len()
        })
    }

    fn columns(&self) -> usize {
        self.grid.map_or(1, |grid| grid.columns)
    }

    /// Rows of a section's tiles hold the column; a section's last row may
    /// be short, and a track row is one.
    fn below(&self, at: usize, rows: isize) -> usize {
        let lines = self.lines();
        let Some(line) = lines.iter().position(|(_, range)| range.contains(&at)) else {
            return at;
        };
        let column = at - lines[line].1.start;
        let to = line
            .saturating_add_signed(rows)
            .min(lines.len().saturating_sub(1));
        let range = &lines[to].1;
        (range.start + column).min(range.end.saturating_sub(1))
    }

    /// One thing is marked, as a plain click marks a row, without starting
    /// a pick: the tiles still play on a click. A range starts one.
    fn pick(&mut self, from: usize, to: usize, _: &mut Context<Self>) {
        let order = self.order();
        if from == to {
            self.selection.arrow(order[from]);
        } else {
            self.selection.only(order[from]);
            self.selection.click(order[to], true, &order);
        }
    }

    fn toggle(&mut self, ix: usize, _: &mut Context<Self>) {
        let order = self.order();
        self.selection.click(order[ix], false, &order);
    }

    fn reveal(&mut self, ix: usize, _: &mut Context<Self>) {
        if let Some((item, _)) = self.lines().into_iter().find(|(_, r)| r.contains(&ix)) {
            self.list.scroll_to_reveal_item(item);
        }
    }

    /// Return opens a record, as a click on its title does, and plays the
    /// appearances from a track, as a double click does.
    fn activate(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.record.clone() else { return };
        match self.order()[ix] {
            Playable::Album(album) => self
                .lib
                .nav
                .update(cx, |nav, cx| nav.open(Page::Album(album), cx)),
            _ => {
                let ids = r.appearances.iter().map(|t| t.id).collect();
                let from = ix - r.albums.len() - r.features.len();
                actions::play_list(&self.lib, ids, from, cx);
            }
        }
    }

    fn menu_at(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        let entity = cx.weak_entity();
        self.menu = ix.map(|ix| {
            let item = self.order()[ix];
            let album = match (item, &self.record) {
                (Playable::Track(id), Some(r)) => r
                    .appearances
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.album_id),
                _ => None,
            };
            actions::keyed_target(
                &self.selection,
                item,
                album,
                Some(std::rc::Rc::new(move |cx: &mut App| {
                    entity
                        .update(cx, |this, cx| {
                            this.selection.end();
                            cx.notify();
                        })
                        .ok();
                })),
            )
        });
    }
}

impl chrome::Picking for ArtistPage {
    fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }
}

impl Render for ArtistPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let width = self.width.clone();
        let entity = cx.entity_id();
        let measure = crate::components::width_probe(width, entity);

        // Cut again when the record or the columns change; the scroll goes
        // back to where it was left once there is content under it.
        if let (Some(width), Some(record)) = (self.width.get(), self.record.clone()) {
            let grid = Grid::for_width(width, &p.look.scale);
            // Only the room between tiles moved: the rows are cut as they were.
            if self.grid.is_some_and(|g| g.same_rows(&grid)) {
                self.grid = Some(grid);
            } else if self.grid != Some(grid) {
                // A change of columns keeps the place, as a new record does.
                if self.restore.is_none() {
                    self.restore = self.anchor();
                }
                self.grid = Some(grid);
                self.items = Rc::new(Self::cut(&record, grid));
                self.list.reset(self.items.len());
                if let Some(anchor) = self.restore.take() {
                    let top = self.find(&record, anchor);
                    self.list.scroll_to(top);
                }
            }
            self.aim(p, grid, window.scale_factor(), cx);
        }

        // Tiles not drawn last frame go; a few new sleeves into the atlas.
        self.tiles.begin();
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }
        let view = cx.weak_entity();
        // The page's margin is the list's parent's: a list lays its items out
        // across its own bounds and ignores padding of its own.
        let body = div()
            .size_full()
            .px(RowMetrics::page_margin(&p.look))
            .child(
                list(self.list.clone(), move |ix, window, cx| {
                    view.update(cx, |this, cx| this.item(ix, window, cx))
                        .unwrap_or_else(|_| div().into_any_element())
                })
                .size_full(),
            );

        self.keyed = self.focus.is_focused(window);
        let len = super::keys::KeyedList::len(self);
        self.cursor.clamp(len);
        actions::list_menu(
            super::keys::keyed(
                div().key_context("Library List").track_focus(&self.focus),
                cx,
            ),
            cx.weak_entity(),
            window,
            cx,
        )
        .on_action(cx.listener(Self::select_all))
        .on_action(cx.listener(Self::dismiss))
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .typeset(p, Type::Body, p.ink)
        .child(measure)
        .child(
            div()
                .id("artist-page-list")
                .relative()
                .flex_1()
                .min_h_0()
                .child(body)
                // GPUI's scrollbar over a virtualised list, which koan-kit's ScrollArea cannot wrap.
                .child(crate::components::list_scrollbar(&self.list)),
        )
        .when(self.selection.is_active(), |d| {
            d.child(chrome::selection_bar(
                p,
                &self.lib,
                &self.selection,
                cx.entity(),
                cx,
            ))
        })
    }
}

impl Focusable for ArtistPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::tokens::Scale;
    use core::prelude::v1::test;

    fn album(id: i64, section: ReleaseSection) -> Album {
        Album {
            id,
            title: String::new(),
            artist_id: 1,
            artist_name: String::new(),
            artist_credit: Vec::new(),
            year: None,
            codec: None,
            label: None,
            total_discs: None,
            total_tracks: None,
            added_at: None,
            on_device: None,
            release_badge: None,
            release_section: section,
        }
    }

    #[test]
    fn sections_are_runs_of_the_engines_order() {
        let albums = [
            album(1, ReleaseSection::Albums),
            album(2, ReleaseSection::Albums),
            album(3, ReleaseSection::Live),
        ];
        assert_eq!(
            sections(&albums),
            vec![(ReleaseSection::Albums, 0, 2), (ReleaseSection::Live, 2, 1)]
        );
        assert!(sections(&[]).is_empty());
    }

    #[test]
    fn records_are_cut_into_rows_under_their_headings() {
        let record = Record {
            artist: None,
            albums: (0..5)
                .map(|i| {
                    album(
                        i,
                        if i < 4 {
                            ReleaseSection::Albums
                        } else {
                            ReleaseSection::Live
                        },
                    )
                })
                .collect(),
            features: vec![album(9, ReleaseSection::Albums)],
            appearances: Vec::new(),
        };
        let grid = Grid::for_width(Pixels::from(560.0), &Scale::of(100));
        assert_eq!(grid.columns, 3);
        let items = ArtistPage::cut(&record, grid);
        let shape: Vec<String> = items
            .iter()
            .map(|i| match i {
                Item::Header => "header".into(),
                Item::Heading(t, _) => format!("[{t}]"),
                Item::FeaturesHeading => "[features]".into(),
                Item::Tiles { range, .. } => format!("{}..{}", range.start, range.end),
                Item::Track(i) => format!("track {i}"),
                Item::About => "about".into(),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "header",
                "[Albums]",
                "0..3",
                "3..4",
                "[Live]",
                "4..5",
                "[features]",
                "0..1",
                "about"
            ]
        );
        // Albums alone are headed too.
        let alone = Record {
            artist: None,
            albums: (0..2).map(|i| album(i, ReleaseSection::Albums)).collect(),
            features: Vec::new(),
            appearances: Vec::new(),
        };
        let items = ArtistPage::cut(&alone, grid);
        assert!(matches!(&items[1], Item::Heading(t, _) if t.as_ref() == "Albums"));
    }
}
