//! A shelf: favourites, recently played, what is downloaded, or a search.
//! Each is a narrowing of the library's own listings (`koan_core::shelves`),
//! and its page shows the head of each, artists as pills, records as tiles
//! and tracks as rows, with how many there are in all. A section's heading
//! opens the browser for that kind with the shelf as its filter, so the
//! preview and its "see all" read one query and cannot disagree.
//!
//! Everything on the page takes part in one pick, across the kinds, as on an
//! artist's page: ⌘-click starts it, ⇧ extends it in the page's order, and
//! dragging any picked item carries the pick.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use koan_kit::app::*;
use koan_runtime::{ShelfKind, ShelfSummary};

use super::covers::{Art, Covers, Key, Priority, UPLOAD_BYTES_PER_FRAME};
use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::rows::{self, RowPart, RowState, TrackList};
use super::selection::Selection;
use super::tile::{Part, Record as TileRecord, TileHost, TileState, Tiles};
use super::tokens::Paint;
use super::{Dismiss, Lib, SelectAll, actions, chrome};
use crate::components::{
    Collection, CollectionSection, CollectionWidth, EmptyState, PageHeader, Pill, RowKind,
    RowMetrics, SectionHeading, TileGrid,
};
use crate::navigator::{Listing, Shelf};
use crate::theme::Icon;

/// What a shelf is called and what it says with nothing on it.
struct Words {
    title: &'static str,
    icon: Icon,
    empty: &'static str,
    detail: &'static str,
}

fn words(shelf: &ShelfKind) -> Words {
    match shelf {
        ShelfKind::Favourites => Words {
            title: "favourites",
            icon: Icon::Favourite,
            empty: "nothing favourited yet",
            detail: "Hit the heart on a track, a record or an artist.",
        },
        ShelfKind::Recent => Words {
            title: "recently played",
            icon: Icon::RecentlyPlayed,
            empty: "nothing played lately",
            detail: "What you play shows up here, newest first.",
        },
        ShelfKind::Downloaded => Words {
            title: "downloaded",
            icon: Icon::OnDevice,
            empty: "nothing downloaded yet",
            detail: "Records you play or download from your server appear here, with how much of each is on this device.",
        },
        ShelfKind::Search { .. } => Words {
            title: "results",
            icon: Icon::Search,
            empty: "no results",
            detail: "Nothing in the library matches.",
        },
    }
}

/// The shelf a route shows, for the routes that are shelves.
pub fn shelf_of(page: Page) -> Option<ShelfKind> {
    match page {
        Page::Favourites => Some(ShelfKind::Favourites),
        Page::RecentlyPlayed => Some(ShelfKind::Recent),
        Page::OnDevice => Some(ShelfKind::Downloaded),
        _ => None,
    }
}

/// What a shelf's contents depend on, beyond its kind: a summary read at one
/// of these is current until it moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    library: Option<u64>,
    history: Option<u64>,
    favourites: u64,
}

impl Stamp {
    fn of(shelf: &ShelfKind, feed: &super::feed::Feed) -> Self {
        Self {
            library: feed.library_version,
            history: matches!(shelf, ShelfKind::Recent)
                .then_some(feed.history_version)
                .flatten(),
            favourites: if matches!(shelf, ShelfKind::Favourites) {
                feed.favourites_revision
            } else {
                0
            },
        }
    }
}

pub struct ShelfPage {
    lib: Lib,
    shelf: ShelfKind,
    /// The search's results page: the summary is the sidebar search's
    /// answer, read as it is typed, rather than read here.
    follows_search: bool,
    /// The search's answer the page shows.
    adopted: Option<Rc<crate::search::Results>>,
    summary: Option<Rc<ShelfSummary>>,
    loaded: Option<Stamp>,
    /// The read in flight and what it was asked at; a newer one replaces it,
    /// and a read landing for an older stamp is dropped.
    loading: Option<Stamp>,
    _load: Option<Task<()>>,
    width: CollectionWidth,
    selection: Selection,
    pick: Option<(u64, PlayableDrag)>,
    records: HashMap<i64, Rc<TileRecord>>,
    tiles: Tiles<ShelfPage>,
    /// What the page's context menu acts on: the item last right-clicked.
    menu: Option<actions::MenuTarget>,
    starting: HashSet<i64>,
    /// The sleeves last asked of the cache, so a frame that wants the same
    /// ones asks nothing.
    aimed: Vec<Key>,
    focus: FocusHandle,
    /// The keyboard's place on the page, in `order`: the artists, the
    /// records, then the tracks.
    cursor: crate::components::ListCursor,
    /// The page holds the keyboard, as of the last frame.
    keyed: bool,
    /// What the keys last moved to, for the collection to scroll to.
    revealed: Option<usize>,
    /// The tile grid as of the last frame, for the keys.
    grid: Option<TileGrid>,
    _subscriptions: Vec<Subscription>,
}

impl ShelfPage {
    pub fn new(lib: Lib, shelf: ShelfKind, _: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(lib, shelf, false, cx)
    }

    fn build(lib: Lib, shelf: ShelfKind, follows_search: bool, cx: &mut Context<Self>) -> Self {
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
                this.lib.covers.update(cx, |c, cx| c.release(id, cx));
            }),
        ];
        let mut this = Self {
            lib,
            shelf,
            follows_search,
            adopted: None,
            summary: None,
            loaded: None,
            loading: None,
            _load: None,
            width: CollectionWidth::default(),
            selection: Selection::default(),
            pick: None,
            records: HashMap::new(),
            tiles: Tiles::default(),
            menu: None,
            starting: HashSet::new(),
            aimed: Vec::new(),
            focus: cx.focus_handle().tab_stop(true),
            cursor: Default::default(),
            keyed: false,
            revealed: None,
            grid: None,
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    /// The search's results: whatever the sidebar's search last answered.
    pub fn results(lib: Lib, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = lib.search.clone();
        let query = search.read(cx).query().to_string();
        let mut this = Self::build(lib, ShelfKind::Search { query }, true, cx);
        this._subscriptions
            .push(cx.observe(&search, |this, search, cx| {
                this.adopt(&search, cx);
                cx.notify();
            }));
        this.adopt(&search, cx);
        this
    }

    fn adopt(&mut self, search: &Entity<crate::search::Search>, cx: &mut Context<Self>) {
        let Some(results) = search.read(cx).results().cloned() else {
            return;
        };
        if self
            .adopted
            .as_ref()
            .is_some_and(|a| Rc::ptr_eq(a, &results))
        {
            return;
        }
        self.shelf = ShelfKind::Search {
            query: results.query.clone(),
        };
        // The search box's deeper answer, while it is open, cut to what this
        // page previews: it shows the same whichever asked last.
        let mut summary = results.summary.clone();
        if results.deep {
            use koan_core::shelves::{PREVIEW_ALBUMS, PREVIEW_ARTISTS, PREVIEW_TRACKS};
            summary.artists.truncate(PREVIEW_ARTISTS as usize);
            summary.albums.truncate(PREVIEW_ALBUMS as usize);
            summary.tracks.truncate(PREVIEW_TRACKS as usize);
        }
        self.records = summary
            .albums
            .iter()
            .map(|a| (a.id, Rc::new(TileRecord::of(a))))
            .collect();
        self.summary = Some(Rc::new(summary));
        self.adopted = Some(results);
    }

    /// Reads the shelf again when what it depends on has moved since the
    /// last read, or the one in flight.
    fn load(&mut self, cx: &mut Context<Self>) {
        if self.follows_search {
            return;
        }
        let feed = self.lib.feed.read(cx);
        if feed.library_version.is_none() {
            return;
        }
        let stamp = Stamp::of(&self.shelf, feed);
        if Some(stamp) == self.loading || (self.loading.is_none() && Some(stamp) == self.loaded) {
            return;
        }
        self.loading = Some(stamp);
        let (engine, shelf) = (self.lib.engine.clone(), self.shelf.clone());
        self._load = Some(cx.spawn(async move |this, cx| {
            let read = engine.shelf_summary(shelf).await;
            this.update(cx, |this, cx| {
                if this.loading != Some(stamp) {
                    return;
                }
                this.loading = None;
                match read {
                    Ok(summary) => {
                        this.loaded = Some(stamp);
                        this.records = summary
                            .albums
                            .iter()
                            .map(|a| (a.id, Rc::new(TileRecord::of(a))))
                            .collect();
                        this.summary = Some(Rc::new(summary));
                        cx.notify();
                    }
                    Err(e) => log::warn!("shelf {:?}: {e}", this.shelf),
                }
            })
            .ok();
        }));
    }

    /// Everything on the page, in its order: what ⇧ and ⌘A range over.
    fn order(&self) -> Vec<Playable> {
        let Some(s) = &self.summary else {
            return Vec::new();
        };
        s.artists
            .iter()
            .map(|a| Playable::Artist(a.id))
            .chain(s.albums.iter().map(|a| Playable::Album(a.id)))
            .chain(s.tracks.iter().map(|t| Playable::Track(t.id)))
            .collect()
    }

    /// Whether the keyboard's cursor is on item `at` of `order`.
    fn on(&self, at: usize) -> bool {
        self.keyed && self.cursor.at == Some(at)
    }

    /// The page's lines as the keys walk them, as parts of `order`: the
    /// artists' pills as one, each row of tiles, each track.
    fn lines(&self) -> Vec<std::ops::Range<usize>> {
        let (Some(s), Some(grid)) = (&self.summary, self.grid) else {
            return Vec::new();
        };
        let (artists, albums) = (s.artists.len(), s.artists.len() + s.albums.len());
        let columns = grid.columns.max(1);
        let mut lines = Vec::new();
        if artists > 0 {
            lines.push(0..artists);
        }
        let mut start = artists;
        while start < albums {
            lines.push(start..(start + columns).min(albums));
            start += columns;
        }
        lines.extend((albums..albums + s.tracks.len()).map(|t| t..t + 1));
        lines
    }

    /// Where the collection should scroll for the keys: the section, and
    /// the item within it.
    fn reveal_in(&self, section: usize) -> Option<usize> {
        let s = self.summary.as_ref()?;
        let at = self.revealed?;
        let bounds = [
            0,
            s.artists.len(),
            s.artists.len() + s.albums.len(),
            usize::MAX,
        ];
        (bounds[section]..bounds[section + 1])
            .contains(&at)
            .then(|| at - bounds[section])
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

    /// What a drag of `item` carries: the pick when `item` is in it.
    fn drag(&mut self, item: Playable, label: SharedString, album: Option<i64>) -> PlayableDrag {
        match (self.selection.contains(item), self.pick()) {
            (true, Some(pick)) => pick,
            _ => PlayableDrag::new(&[item], label).with_sleeve(album),
        }
    }

    /// Opens `page`'s browser narrowed to this shelf, from its top, sorted
    /// as the shelf is.
    fn see_all(&mut self, listing: Listing, cx: &mut Context<Self>) {
        let shelf = match &self.shelf {
            ShelfKind::Favourites => Shelf::Favourites,
            ShelfKind::Recent => Shelf::Recent,
            ShelfKind::Downloaded => Shelf::Downloaded,
            ShelfKind::Search { .. } => Shelf::Search,
        };
        let page = Page::Shelved(shelf, listing);
        self.lib.nav.update(cx, |nav, cx| {
            nav.forget_place(page);
            nav.open(page, cx);
        });
    }

    /// Asks the cache for every sleeve the page shows, once per change of
    /// what that is. A shelf shows a few of each, so all of it is visible.
    fn aim(&mut self, p: &Paint, grid: TileGrid, scale: f32, cx: &mut Context<Self>) {
        let Some(s) = &self.summary else { return };
        let keys: Vec<Key> = s
            .albums
            .iter()
            .map(|a| Key::new(Art::Album(a.id), grid.side, scale))
            .chain(s.tracks.iter().filter_map(|t| {
                t.album_id
                    .map(|a| Key::new(Art::Album(a), RowMetrics::sleeve(&p.look), scale))
            }))
            .collect();
        if keys == self.aimed {
            return;
        }
        self.aimed = keys.clone();
        let owner = cx.entity_id();
        self.lib.covers.update(cx, |c, _| {
            c.request(owner, keys.into_iter().map(|k| (k, Priority::Visible)))
        });
    }

    fn heading(
        &self,
        title: &'static str,
        total: u64,
        listing: Listing,
        cx: &mut Context<Self>,
    ) -> SectionHeading {
        SectionHeading::new(title)
            .count(chrome::grouped(total))
            .on_open(
                SharedString::from(format!("see-all-{title}")),
                cx.listener(move |this, _, _, cx| this.see_all(listing, cx)),
            )
    }

    fn pills(&mut self, s: &ShelfSummary, cx: &mut Context<Self>) -> Vec<Pill> {
        let view = cx.weak_entity();
        let selecting = self.selection.is_active();
        s.artists
            .iter()
            .enumerate()
            .map(|(index, artist)| {
                let item = Playable::Artist(artist.id);
                let drag = self.drag(item, artist.name.clone().into(), None);
                let target = actions::MenuTarget {
                    items: drag.items.clone(),
                    album: None,
                    after: None,
                };
                let id = artist.id;
                let mut pill = Pill::new(
                    ("shelf-artist", id as u64),
                    Icon::Artist,
                    artist.name.clone(),
                );
                if selecting {
                    pill = pill.ticking(self.selection.contains(item));
                }
                pill = pill.cursor(self.on(index));
                pill.on_click(super::listen(
                    view.clone(),
                    move |this: &mut Self, e: &ClickEvent, window, cx| {
                        this.pill_clicked(id, e, window, cx)
                    },
                ))
                .on_drag(drag, |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx))
                .on_mouse_down(MouseButton::Right, actions::mark(view.clone(), target))
            })
            .collect()
    }

    fn pill_clicked(
        &mut self,
        id: i64,
        e: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let item = Playable::Artist(id);
        let m = e.modifiers();
        let order = self.order();
        self.place(item, &order);
        if !self.selection.take(item, m.shift, m.secondary(cx), &order) {
            self.lib
                .nav
                .update(cx, |nav, cx| nav.open(Page::Artist(id), cx));
        }
        cx.notify();
    }

    fn tiles(
        &mut self,
        s: &ShelfSummary,
        grid: TileGrid,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let pick = self.pick();
        let revision = self.selection.revision();
        let entity = cx.entity();
        let after: actions::After = Some(Rc::new(move |cx: &mut App| {
            entity.update(cx, |this, cx| {
                this.selection.end();
                cx.notify();
            })
        }));
        let mut tiles = Vec::with_capacity(s.albums.len());
        // A tile's place among the tiles drawn: a record not read yet is
        // neither drawn nor counted.
        let count = s
            .albums
            .iter()
            .filter(|album| self.records.contains_key(&album.id))
            .count();
        let mut shown = 0;
        for (ix, album) in s.albums.iter().enumerate() {
            let Some(record) = self.records.get(&album.id).cloned() else {
                continue;
            };
            let place = (shown, count);
            shown += 1;
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
                cursor: self.on(s.artists.len() + ix),
                show_artist: true,
                side: grid.side,
                sleeve,
                drag,
                pick,
                after,
                place,
            };
            tiles.push(self.tiles.tile(&record, state, grid.tile_height(), cx));
        }
        tiles
    }

    fn rows(
        &mut self,
        p: &Paint,
        s: &ShelfSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let scale = window.scale_factor();
        let lead_width = rows::lead_width(p, s.tracks.len(), window);
        let mut out = Vec::with_capacity(s.tracks.len());
        for (index, track) in s.tracks.iter().enumerate() {
            let item = Playable::Track(track.id);
            let drag = self.drag(item, track.title.clone().into(), track.album_id);
            let sleeve = track.album_id.map(|album| {
                let key = Key::new(Art::Album(album), RowMetrics::sleeve(&p.look), scale);
                self.lib.covers.update(cx, |c, _| c.sleeve(key))
            });
            let feed = self.lib.feed.read(cx);
            let state = RowState {
                lead: (index + 1).to_string().into(),
                lead_width,
                current: feed.playing_track == Some(track.id),
                favourite: feed.is_favourite(item),
                selected: self.selection.contains(item),
                gathered: true,
                transfer: feed.transfers.get(&track.id).copied(),
                drag,
                note: None,
                cursor: self.on(s.artists.len() + s.albums.len() + index),
                kind: RowKind::Art,
                place: Some((index, s.tracks.len())),
                reveal: None,
            };
            out.push(rows::track_row(p, track, index, state, sleeve, None, cx));
        }
        out
    }

    /// Plays a record in place of the queue and opens its page, the sleeve
    /// marked as starting until the engine has the command.
    fn play_record(&mut self, album: i64, cx: &mut Context<Self>) {
        self.starting.insert(album);
        cx.notify();
        let started = actions::play(&self.lib, &[Playable::Album(album)], false, cx);
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

/// The page's detail line: only the kinds there are, the whole shelf's
/// counts rather than the preview's.
fn counts(s: &ShelfSummary) -> String {
    [
        (s.artist_total, "artist", "artists"),
        (s.album_total, "album", "albums"),
        (s.track_total, "track", "tracks"),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, one, many)| chrome::count(n, one, many))
    .collect::<Vec<_>>()
    .join(" · ")
}

impl actions::Menus for ShelfPage {
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

impl TileHost for ShelfPage {
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
        self.place(item, &order);
        if self
            .selection
            .take(item, modifiers.shift, modifiers.secondary(cx), &order)
        {
            cx.notify();
            return;
        }
        match part {
            // A queue built up over an evening does not go to a click meant
            // to play one record: with anything queued, it asks first.
            Part::Sleeve => {
                let queued = cx
                    .global::<crate::mirror::Mirror>()
                    .queue
                    .read(cx)
                    .items
                    .len();
                if queued == 0 {
                    self.play_record(album, cx);
                    return;
                }
                let view = cx.weak_entity();
                let title = self
                    .records
                    .get(&album)
                    .map(|r| r.title.to_string())
                    .unwrap_or_default();
                crate::components::sheet::confirm(
                    crate::components::sheet::Confirm {
                        title: "replace the queue".into(),
                        verb: "play".into(),
                        destructive: true,
                        lines: vec![
                            format!(
                                "{} come off the queue, and {title} plays in its place.",
                                chrome::count(queued as u64, "track", "tracks")
                            )
                            .into(),
                        ],
                        note: Some("Add to queue, in the menu, keeps the queue.".into()),
                    },
                    window,
                    cx,
                    move |cx| {
                        view.update(cx, |this, cx| this.play_record(album, cx)).ok();
                    },
                );
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

impl TrackList for ShelfPage {
    fn row_tracks(&self) -> Vec<i64> {
        self.summary
            .as_ref()
            .map(|s| s.tracks.iter().map(|t| t.id).collect())
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
                let ids = self.row_tracks();
                let item = Playable::Track(track);
                let order = self.order();
                self.place(item, &order);
                let m = event.modifiers();
                // Outside the mode a plain click marks the row alone: the
                // mode would turn every tile's and pill's click into a tick.
                let plays = if !m.shift && !m.secondary(cx) && !self.selection.is_active() {
                    let double = event.click_count() >= 2;
                    if !double {
                        self.selection.mark(item);
                    }
                    double
                } else {
                    let order = self.order();
                    rows::select(&mut self.selection, item, event, || order, cx)
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

impl ShelfPage {
    /// The keyboard carries on from the item clicked.
    fn place(&mut self, item: Playable, order: &[Playable]) {
        if let Some(at) = order.iter().position(|p| *p == item) {
            self.cursor.at = Some(at);
            self.cursor.anchor = Some(at);
        }
    }
}

impl super::keys::KeyedList for ShelfPage {
    fn cursor(&mut self) -> &mut crate::components::ListCursor {
        &mut self.cursor
    }

    fn len(&self) -> usize {
        self.summary
            .as_ref()
            .map_or(0, |s| s.artists.len() + s.albums.len() + s.tracks.len())
    }

    /// ← → walk the pills and the tiles; the grid's columns say how.
    fn columns(&self) -> usize {
        self.grid.map_or(2, |grid| grid.columns.max(2))
    }

    /// From the pills down to the first record; within the records, the
    /// same column; a track row is one.
    fn below(&self, at: usize, rows: isize) -> usize {
        let lines = self.lines();
        let Some(line) = lines.iter().position(|range| range.contains(&at)) else {
            return at;
        };
        let column = at - lines[line].start;
        let to = line
            .saturating_add_signed(rows)
            .min(lines.len().saturating_sub(1));
        let range = &lines[to];
        (range.start + column).min(range.end.saturating_sub(1))
    }

    /// One thing is marked without starting a pick, so the tiles and pills
    /// still act on a click. A range starts one.
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
        self.revealed = Some(ix);
    }

    /// Return opens an artist or a record, and plays the tracks from a
    /// track, as a double click does.
    fn activate(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        let order = self.order();
        match order[ix] {
            Playable::Artist(id) => self
                .lib
                .nav
                .update(cx, |n, cx| n.open(Page::Artist(id), cx)),
            Playable::Album(id) => self.lib.nav.update(cx, |n, cx| n.open(Page::Album(id), cx)),
            Playable::Track(_) => {
                let skip = order
                    .iter()
                    .take_while(|p| !matches!(p, Playable::Track(_)))
                    .count();
                let ids = self.row_tracks();
                actions::play_list(&self.lib, ids, ix - skip, cx);
            }
        }
    }

    fn menu_at(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        let entity = cx.weak_entity();
        self.menu = ix.map(|ix| {
            let item = self.order()[ix];
            let album = match (item, &self.summary) {
                (Playable::Track(id), Some(s)) => s
                    .tracks
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

impl chrome::Picking for ShelfPage {
    fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }
}

impl Render for ShelfPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let words = words(&self.shelf);
        let scale = window.scale_factor();
        let grid = self.width.grid(&p.look);
        self.grid = grid;
        self.keyed = self.focus.is_focused(window);
        let len = super::keys::KeyedList::len(self);
        self.cursor.clamp(len);

        // Tiles not drawn last frame go; a few new sleeves into the atlas.
        self.tiles.begin();
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }

        let mut header = PageHeader::new(words.title);
        let mut collection = Collection::new("shelf", &self.width)
            .empty(EmptyState::new(words.icon, words.empty).detail(words.detail));
        if let Some(s) = self.summary.clone() {
            header = header.detail(counts(&s));
            let artists = self.heading("artists", s.artist_total, Listing::Artists, cx);
            collection = collection.section(
                CollectionSection::pills(artists, self.pills(&s, cx)).reveal(self.reveal_in(0)),
            );
            if let Some(grid) = grid {
                self.aim(p, grid, scale, cx);
                let albums = self.heading("albums", s.album_total, Listing::Albums, cx);
                let tiles = self.tiles(&s, grid, scale, cx);
                collection = collection.section(
                    CollectionSection::tiles(albums, grid, tiles).reveal(self.reveal_in(1)),
                );
            }
            let tracks = self.heading("tracks", s.track_total, Listing::Tracks, cx);
            let rows = self.rows(p, &s, window, cx);
            collection =
                collection.section(CollectionSection::rows(tracks, rows).reveal(self.reveal_in(2)));
        }
        let collection = collection.header(header);

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
        .child(
            div()
                .id("shelf-page")
                .relative()
                .flex_1()
                .min_h_0()
                .child(collection),
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

impl Focusable for ShelfPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_name_only_the_kinds_there_are() {
        let s = ShelfSummary {
            artists: Vec::new(),
            artist_total: 0,
            albums: Vec::new(),
            album_total: 1,
            tracks: Vec::new(),
            track_total: 1204,
        };
        assert_eq!(counts(&s), "1 album · 1,204 tracks");
    }
}
