//! An artist's page: the photograph and biography from their Wikipedia
//! article, play as the one prominent action, their records under a heading
//! per kind of release, then the records they feature on. An artist with no
//! records of their own shows the tracks they appear on instead.
//!
//! A list of variable-height items (the header, headings, rows of tiles,
//! track rows), so only what is on screen is built, however many records an
//! artist features on. Every item's height but the header's is fixed, which
//! is what lets the page rank sleeves by their distance from the screen.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{Album, AlbumSort, Artist, ArtistInfo, BrowseFilter, ReleaseSection, Track};

use super::covers::{Art, Covers, Key, Priority, Sleeve, UPLOAD_BYTES_PER_FRAME};
use super::drag::{Playable, PlayableDrag};
use super::nav::{Anchor, Anchored, Heading, Page};
use super::rows::{self, RowPart, RowState, TrackList};
use super::selection::Selection;
use super::tile::{self, Grid, Part, Record as TileRecord, TileHost, TileState, Tiles};
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome};
use crate::theme::Space;

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
}

pub struct ArtistPage {
    lib: Lib,
    artist_id: i64,
    record: Option<Rc<Record>>,
    info: Option<ArtistInfo>,
    /// The biography shown whole rather than its opening.
    expanded: bool,
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
                this.lib.covers.update(cx, |c, _| c.release(id));
            }),
        ];
        let restore = lib.nav.read(cx).anchor(Page::Artist(artist_id));
        let mut this = Self {
            lib,
            artist_id,
            record: None,
            info: None,
            expanded: false,
            loaded_version: None,
            request: 0,
            _load: None,
            _enrich: None,
            width: Rc::new(Cell::new(None)),
            items: Rc::new(Vec::new()),
            grid: None,
            list: ListState::new(0, ListAlignment::Top, px(400.0)),
            restore,
            selection: Selection::default(),
            pick: None,
            records: std::collections::HashMap::new(),
            tiles: Tiles::default(),
            menu: None,
            starting: HashSet::new(),
            focus: cx.focus_handle(),
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
            let artist = engine.clone().artist(id).await.ok().flatten();
            let albums = engine
                .clone()
                .albums(Some(id), AlbumSort::Year, 0, None, BrowseFilter::default())
                .await
                .unwrap_or_default();
            let features = engine.clone().artist_features(id).await.unwrap_or_default();
            let info = engine.clone().artist_info(id).await.ok().flatten();
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
                if fetched.is_some() {
                    this.info = fetched;
                    this.list.remeasure_items(0..1);
                    cx.notify();
                }
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
        let sections = sections(&record.albums);
        let titled = sections.len() > 1;
        for (section, start, len) in sections {
            if titled {
                items.push(Item::Heading(
                    koan_ffi::release_section_title(section)
                        .to_lowercase()
                        .into(),
                    Heading::Section(section),
                ));
            }
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
        items
    }

    /// A heading's line, with the space above it and below.
    fn heading_height(p: &Paint) -> Pixels {
        p.look.line_height(Type::Fine) + p.space(Space::L) + p.space(Space::S)
    }

    fn height(&self, p: &Paint, item: &Item, grid: Grid) -> f32 {
        f32::from(match item {
            Item::Header => self
                .list
                .bounds_for_item(0)
                .map_or(px(HEADER_GUESS), |b| b.size.height),
            Item::Heading(..) | Item::FeaturesHeading => Self::heading_height(p),
            Item::Tiles { .. } => grid.row,
            Item::Track(_) => rows::art_height(p),
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
                offset_in_item: px(0.0),
            },
        }
    }

    /// How far down the page is scrolled, in points.
    fn offset(&self, p: &Paint) -> Pixels {
        let Some(grid) = self.grid else {
            return px(0.0);
        };
        let top = self.list.logical_scroll_top();
        let above: f32 = self.items[..top.item_ix.min(self.items.len())]
            .iter()
            .map(|i| self.height(p, i, grid))
            .sum();
        px(above) + top.offset_in_item
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
                        Key::new(Art::Artist(self.artist_id), grid.side, scale),
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
                            Key::new(Art::Album(album), px(rows::SLEEVE), scale),
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
            Item::Heading(title, _) => Self::heading(p, title).into_any_element(),
            Item::FeaturesHeading => {
                let items: Vec<Playable> = record
                    .features
                    .iter()
                    .map(|a| Playable::Album(a.id))
                    .collect();
                let (lib, items2, lib2) = (self.lib.clone(), items.clone(), self.lib.clone());
                div()
                    .flex()
                    .items_end()
                    .gap(p.space(Space::L))
                    .child(Self::heading(p, "features on"))
                    .child(div().pb(p.space(Space::S)).child(
                        chrome::text_button("features-play", "play").on_click(move |_, _, cx| {
                            actions::play(&lib, &items, false, cx).detach()
                        }),
                    ))
                    .child(div().pb(p.space(Space::S)).child(
                        chrome::text_button("features-shuffle", "shuffle").on_click(
                            move |_, _, cx| actions::play(&lib2, &items2, true, cx).detach(),
                        ),
                    ))
                    .into_any_element()
            }
            Item::Tiles { features, range } => {
                let albums = if features {
                    &record.features
                } else {
                    &record.albums
                };
                let Some(albums) = albums.get(range) else {
                    return div().into_any_element();
                };
                self.tiles(albums, features, grid, scale, cx)
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
                    _ => PlayableDrag::new(&[item], track.title.clone()),
                };
                let sleeve = track.album_id.map(|album| {
                    let key = Key::new(Art::Album(album), px(rows::SLEEVE), scale);
                    self.lib.covers.update(cx, |c, _| c.sleeve(key))
                });
                let feed = self.lib.feed.read(cx);
                let state = RowState {
                    lead: (index + 1).to_string().into(),
                    lead_width: rows::lead_width(p, record.appearances.len(), window),
                    current: feed.playing_track == Some(track.id),
                    playing: feed.playing,
                    favourite: feed.is_favourite(item),
                    selected,
                    gathered: true,
                    transfer: feed.transfers.get(&track.id).copied(),
                    drag,
                    height: rows::art_height(p),
                };
                rows::track_row(p, track, index, state, sleeve, Some(self.artist_id), cx)
            }
        }
    }

    fn tiles(
        &mut self,
        albums: &[Album],
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
        let height = grid.row - px(tile::LINE_GAP);
        let mut tiles = Vec::with_capacity(albums.len());
        for album in albums {
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
                show_artist,
                side: grid.side,
                sleeve,
                drag,
                pick,
                after,
            };
            tiles.push(self.tiles.tile(&record, state, height, cx));
        }
        div().flex().gap(grid.gap).h(grid.row).children(tiles)
    }

    fn heading(p: &Paint, title: impl Into<SharedString>) -> Div {
        div()
            .h(Self::heading_height(p))
            .flex()
            .items_end()
            .pb(p.space(Space::S))
            .typeset(p, Type::Fine, p.ink)
            .child(title.into())
    }

    fn header(
        &mut self,
        p: &Paint,
        record: &Record,
        grid: Grid,
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
        let mut photo = div().size(grid.side).flex_none().bg(p.surface);
        if self.info.as_ref().is_some_and(|i| i.has_image) {
            let key = Key::new(Art::Artist(id), grid.side, scale);
            match self.lib.covers.update(cx, |c, _| c.sleeve(key)) {
                Sleeve::Ready(image) => {
                    photo = photo.child(img(ImageSource::Render(image)).size_full())
                }
                Sleeve::Colour(colour) => photo = photo.bg(colour),
                Sleeve::Waiting | Sleeve::None => {}
            }
        }
        let favourite = self.lib.feed.read(cx).is_favourite(item);
        let (lib, lib2) = (self.lib.clone(), self.lib.clone());
        let mut text = div()
            .flex()
            .flex_col()
            .gap(p.space(Space::S))
            .min_w_0()
            .flex_1()
            .child(
                div()
                    .typeset(p, Type::Title, p.strong)
                    .line_clamp(2)
                    .child(name),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(p.space(Space::L))
                    .child(
                        // The page's one prominent action.
                        crate::components::Button::new(
                            "artist-play",
                            crate::components::ButtonKind::Prominent,
                        )
                        .label("play")
                        .on_click(move |_, _, cx| actions::play(&lib, &[item], false, cx).detach()),
                    )
                    .child(
                        chrome::text_button("artist-shuffle", "shuffle").on_click(
                            move |_, _, cx| actions::play(&lib2, &[item], true, cx).detach(),
                        ),
                    )
                    .child(
                        div()
                            .id("artist-page-heart")
                            .cursor_pointer()
                            .typeset(p, Type::Meta, if favourite { p.accent } else { p.muted })
                            .child(super::tile::heart_glyph(p, favourite))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.lib
                                    .feed
                                    .update(cx, |f, cx| f.toggle_favourite(item, cx))
                            })),
                    ),
            );
        if let Some(info) = &self.info
            && let Some(bio) = &info.bio
        {
            let long = bio.len() > BIO_CLAMP;
            let mut body = div()
                .typeset(p, Type::Control, p.ink)
                .child(bio.replace('\n', "\n\n"));
            if long && !self.expanded {
                body = body.line_clamp(4);
            }
            text = text.child(body).child(
                div()
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
                                    this.list.remeasure_items(0..1);
                                    cx.notify();
                                })),
                        )
                    })
                    .when_some(info.bio_url.clone(), |d, url| {
                        d.child(
                            div()
                                .id("bio-source")
                                .cursor_pointer()
                                .underline()
                                .child("wikipedia")
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    })
                    .when_some(info.image_credit.clone(), |d, credit| {
                        d.child(format!("photo: {credit}"))
                    }),
            );
        }
        div()
            .flex()
            .items_start()
            .gap(grid.gap)
            .pt(p.space(Space::L))
            .pb(p.space(Space::L))
            .child(photo)
            .child(text)
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
            RowPart::Artist(id) => self
                .lib
                .nav
                .update(cx, |n, cx| n.open(Page::Artist(id), cx)),
            RowPart::Album(id) => self.lib.nav.update(cx, |n, cx| n.open(Page::Album(id), cx)),
            RowPart::Heart => self
                .lib
                .feed
                .update(cx, |f, cx| f.toggle_favourite(Playable::Track(track), cx)),
        }
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
        let measure = canvas(
            move |bounds, _, cx| {
                if width.get() != Some(bounds.size.width) {
                    width.set(Some(bounds.size.width));
                    cx.notify(entity);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        // Cut again when the record or the columns change; the scroll goes
        // back to where it was left once there is content under it.
        if let (Some(width), Some(record)) = (self.width.get(), self.record.clone()) {
            let grid = Grid::for_width(width, &p.look.scale);
            if self.grid != Some(grid) {
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
        let body = list(self.list.clone(), move |ix, window, cx| {
            view.update(cx, |this, cx| this.item(ix, window, cx))
                .unwrap_or_else(|_| div().into_any_element())
        })
        .size_full()
        .px(px(tile::MARGIN));

        div()
            .key_context("Library")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::dismiss))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .typeset(p, Type::Body, p.ink)
            .child(measure)
            .child(actions::list_menu(
                div().id("artist-page-list").flex_1().min_h_0().child(body),
                cx.weak_entity(),
            ))
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
        let grid = Grid::for_width(px(560.0), &Scale::of(100));
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
            })
            .collect();
        assert_eq!(
            shape,
            [
                "header",
                "[albums]",
                "0..3",
                "3..4",
                "[live]",
                "4..5",
                "[features]",
                "0..1"
            ]
        );
    }
}
