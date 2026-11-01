//! A shelf: the artists, records and tracks that answer one question (what
//! was favourited, what was played lately, what is on this device). Artists
//! as names, records as tiles, tracks as a working list below them.
//!
//! The first few of each, as `koan_core::shelves` cuts them, with how many
//! there are in all. A section's heading opens that kind's browser filtered
//! to the shelf: the listing the preview is the head of.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{Album, Artist, BrowseFilter, ShelfKind, ShelfSummary, Track};

use super::count;
use crate::library::covers::{Art, Key, Priority, UPLOAD_BYTES_PER_FRAME};
use crate::library::drag::{Playable, PlayableDrag};
use crate::library::nav::Page;
use crate::library::rows::{self, RowPart, RowState, TrackList};
use crate::library::selection::Selection;
use crate::library::tile::{self, Grid, Part, Record as TileRecord, TileHost, TileState, Tiles};
use crate::library::tokens::{Paint, Type, Typeset};
use crate::library::{Dismiss, Lib, SelectAll, actions, chrome};
use crate::mirror::Mirror;
use crate::theme::{Icon, KoanIcon, Space};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shelf {
    Favourites,
    Recent,
    OnDevice,
}

impl Shelf {
    fn kind(self) -> ShelfKind {
        match self {
            Shelf::Favourites => ShelfKind::Favourites,
            Shelf::Recent => ShelfKind::Recent,
            Shelf::OnDevice => ShelfKind::Downloaded,
        }
    }

    fn page(self) -> Page {
        match self {
            Shelf::Favourites => Page::Favourites,
            Shelf::Recent => Page::RecentlyPlayed,
            Shelf::OnDevice => Page::OnDevice,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Shelf::Favourites => "favourites",
            Shelf::Recent => "recently played",
            Shelf::OnDevice => "downloaded",
        }
    }

    /// What an empty shelf says.
    fn empty(self) -> (Icon, &'static str, &'static str) {
        match self {
            Shelf::Favourites => (
                Icon::Favourite,
                "nothing favourited yet",
                "hit the heart on a track, a record or an artist.",
            ),
            Shelf::Recent => (
                Icon::RecentlyPlayed,
                "nothing played lately",
                "what you play shows up here, newest first.",
            ),
            Shelf::OnDevice => (
                Icon::OnDevice,
                "nothing downloaded yet",
                "records you play or download from your server appear here, with how much of each is on this device.",
            ),
        }
    }

    /// The browsers' filter that is this shelf.
    fn filter(self) -> BrowseFilter {
        BrowseFilter {
            favourites: self == Shelf::Favourites,
            recent: self == Shelf::Recent,
            downloaded: self == Shelf::OnDevice,
            ..BrowseFilter::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Artists,
    Albums,
    Tracks,
}

#[derive(Clone, Debug, PartialEq)]
enum Item {
    Heading(Section),
    Artists,
    Tiles(std::ops::Range<usize>),
    Track(usize),
}

fn cut(summary: &ShelfSummary, columns: usize) -> Vec<Item> {
    let mut items = Vec::new();
    if !summary.artists.is_empty() {
        items.push(Item::Heading(Section::Artists));
        items.push(Item::Artists);
    }
    if !summary.albums.is_empty() {
        items.push(Item::Heading(Section::Albums));
        let mut start = 0;
        while start < summary.albums.len() {
            let end = (start + columns.max(1)).min(summary.albums.len());
            items.push(Item::Tiles(start..end));
            start = end;
        }
    }
    if !summary.tracks.is_empty() {
        items.push(Item::Heading(Section::Tracks));
        items.extend((0..summary.tracks.len()).map(Item::Track));
    }
    items
}

pub struct ShelfPage {
    lib: Lib,
    shelf: Shelf,
    summary: Option<Rc<ShelfSummary>>,
    loaded: Option<(u64, u64, usize)>,
    request: u64,
    _load: Option<Task<()>>,
    width: Rc<Cell<Option<Pixels>>>,
    grid: Option<Grid>,
    items: Rc<Vec<Item>>,
    list: ListState,
    selection: Selection,
    menu: Option<actions::MenuTarget>,
    records: HashMap<i64, Rc<TileRecord>>,
    tiles: Tiles<ShelfPage>,
    history: Entity<crate::mirror::Slice<u64>>,
    rewinds: u64,
    /// Where the page was left, put back once it is first cut.
    restore: Option<crate::library::nav::Anchor>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ShelfPage {
    pub fn new(lib: Lib, shelf: Shelf, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let owner = cx.entity_id();
        let history = cx.global::<Mirror>().history.clone();
        let subscriptions = vec![
            crate::theme::observe_accent(cx),
            cx.observe(&lib.feed, |this, _, cx| {
                this.load(cx);
                cx.notify();
            }),
            cx.observe(&history, |this, _, cx| this.load(cx)),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            cx.observe(&lib.nav, |this, nav, cx| {
                let rewinds = nav.read(cx).rewinds(this.shelf.page(), cx);
                if rewinds != this.rewinds {
                    this.rewinds = rewinds;
                    this.list.scroll_to(ListOffset::default());
                    cx.notify();
                }
            }),
            cx.on_release(move |this, cx| {
                let top = this.list.logical_scroll_top();
                let page = this.shelf.page();
                this.lib.nav.update(cx, |nav, _| {
                    nav.remember_anchor(
                        page,
                        crate::library::nav::Anchor {
                            at: crate::library::nav::Anchored::Header,
                            offset: top.offset_in_item,
                            item_ix: top.item_ix,
                        },
                    )
                });
                this.lib.covers.update(cx, |c, _| c.release(owner));
            }),
        ];
        let rewinds = lib.nav.read(cx).rewinds(shelf.page(), cx);
        let mut this = Self {
            lib,
            shelf,
            summary: None,
            loaded: None,
            request: 0,
            _load: None,
            width: Rc::new(Cell::new(None)),
            grid: None,
            items: Rc::default(),
            list: ListState::new(0, ListAlignment::Top, px(400.0)),
            selection: Selection::default(),
            menu: None,
            records: HashMap::new(),
            tiles: Tiles::default(),
            history,
            rewinds,
            restore: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    /// Reads the shelf again when what it answers to moved: the library,
    /// the plays (recently played) or the favourites.
    fn load(&mut self, cx: &mut Context<Self>) {
        let feed = self.lib.feed.read(cx);
        let Some(library) = feed.library_version else {
            return;
        };
        let wanted = (
            library,
            if self.shelf == Shelf::Recent {
                self.history.read(cx).0
            } else {
                0
            },
            if self.shelf == Shelf::Favourites {
                feed.favourite_count()
            } else {
                0
            },
        );
        if self.loaded == Some(wanted) {
            return;
        }
        self.loaded = Some(wanted);
        self.request += 1;
        let request = self.request;
        let (engine, kind) = (self.lib.engine.clone(), self.shelf.kind());
        self._load = Some(cx.spawn(async move |this, cx| {
            let summary = match engine.shelf_summary(kind).await {
                Ok(summary) => summary,
                Err(e) => {
                    log::warn!("shelf: {e}");
                    return;
                }
            };
            this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                let first = this.summary.is_none();
                this.records.clear();
                this.summary = Some(Rc::new(summary));
                // Cut again at the next frame, which knows the width.
                this.grid = None;
                if first {
                    let page = this.shelf.page();
                    if let Some(anchor) = this.lib.nav.read(cx).anchor(page) {
                        this.restore = Some(anchor);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_all(&self, section: Section, cx: &mut App) {
        let filter = self.shelf.filter();
        self.lib
            .browse
            .update(cx, |b, cx| b.update(cx, |b| b.filter = filter));
        let page = match section {
            Section::Artists => Page::Artists,
            Section::Albums => Page::Albums,
            Section::Tracks => Page::Tracks,
        };
        self.lib.nav.update(cx, |nav, cx| nav.open(page, cx));
    }

    fn heading(&self, p: &Paint, section: Section, cx: &mut Context<Self>) -> AnyElement {
        let Some(summary) = &self.summary else {
            return div().into_any_element();
        };
        let (title, total) = match section {
            Section::Artists => ("artists", summary.artist_total),
            Section::Albums => ("albums", summary.album_total),
            Section::Tracks => ("tracks", summary.track_total),
        };
        let id = match section {
            Section::Artists => "see-artists",
            Section::Albums => "see-albums",
            Section::Tracks => "see-tracks",
        };
        div()
            .pt(p.space(Space::L))
            .pb(p.space(Space::S))
            .child(
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(p.space(Space::S))
                    .cursor_pointer()
                    .typeset(p, Type::Fine, p.ink)
                    .hover(|s| s.underline())
                    .child(title)
                    .child(
                        div()
                            .typeset(p, Type::Fine, p.muted)
                            .child(total.to_string()),
                    )
                    .child(
                        KoanIcon::new(Icon::Disclosure)
                            .role(Type::Fine)
                            .color(p.muted),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.open_all(section, cx))),
            )
            .into_any_element()
    }

    fn artists(&self, p: &Paint, cx: &mut Context<Self>) -> AnyElement {
        let Some(summary) = self.summary.clone() else {
            return div().into_any_element();
        };
        let ink = p.ink;
        div()
            .flex()
            .flex_wrap()
            .gap(p.space(Space::S))
            .pb(p.space(Space::S))
            .children(summary.artists.iter().map(|a: &Artist| {
                let (id, item) = (a.id, Playable::Artist(a.id));
                let target = actions::MenuTarget {
                    items: [item].into(),
                    album: None,
                    after: None,
                };
                div()
                    .id(("shelf-artist", id as u64))
                    .px(p.space(Space::M))
                    .py(p.space(Space::Xs))
                    .border_1()
                    .border_color(p.rule)
                    .cursor_pointer()
                    .typeset(p, Type::Meta, p.muted)
                    .hover(move |s| s.text_color(ink))
                    .child(a.name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.lib
                            .nav
                            .update(cx, |nav, cx| nav.open(Page::Artist(id), cx))
                    }))
                    .on_drag(
                        PlayableDrag::new(&[item], a.name.clone()),
                        |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx),
                    )
                    .on_mouse_down(MouseButton::Right, actions::mark(cx.weak_entity(), target))
            }))
            .into_any_element()
    }

    fn tiles(
        &mut self,
        albums: &[Album],
        grid: Grid,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let height = grid.row - px(tile::LINE_GAP);
        let mut tiles = Vec::with_capacity(albums.len());
        for album in albums {
            let record = self
                .records
                .entry(album.id)
                .or_insert_with(|| Rc::new(TileRecord::of(album)))
                .clone();
            let item = Playable::Album(album.id);
            let key = Key::new(Art::Album(album.id), grid.side, scale);
            let sleeve = self.lib.covers.update(cx, |c, _| c.sleeve(key));
            let feed = self.lib.feed.read(cx);
            let state = TileState {
                favourite: feed.is_favourite(item),
                playing: feed.playing_album == Some(album.id),
                arriving: feed.arriving.contains_key(&album.id),
                selecting: false,
                selected: false,
                starting: false,
                show_artist: true,
                side: grid.side,
                sleeve,
                drag: PlayableDrag::new(&[item], record.title.clone()),
                pick: None,
                after: None,
            };
            tiles.push(self.tiles.tile(&record, state, height, cx));
        }
        div()
            .flex()
            .gap(grid.gap)
            .h(grid.row)
            .children(tiles)
            .into_any_element()
    }

    fn track(
        &mut self,
        p: &Paint,
        index: usize,
        track: &Track,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item = Playable::Track(track.id);
        let selected = self.selection.contains(item);
        let drag = if selected && !self.selection.picked().is_empty() {
            let picked = self.selection.picked();
            PlayableDrag::new(picked, count(picked.len(), "track", "tracks"))
        } else {
            PlayableDrag::new(&[item], track.title.clone())
        };
        let sleeve = track.album_id.map(|album| {
            let key = Key::new(Art::Album(album), px(rows::SLEEVE), window.scale_factor());
            self.lib.covers.update(cx, |c, _| c.sleeve(key))
        });
        let total = self.summary.as_ref().map_or(0, |s| s.tracks.len());
        let feed = self.lib.feed.read(cx);
        let state = RowState {
            lead: (index + 1).to_string().into(),
            lead_width: rows::lead_width(p, total, window),
            current: feed.playing_track == Some(track.id),
            playing: feed.playing,
            favourite: feed.is_favourite(item),
            selected,
            gathered: true,
            transfer: feed.transfers.get(&track.id).copied(),
            drag,
            height: rows::art_height(p),
        };
        rows::track_row(p, track, index, state, sleeve, None, cx)
    }

    fn item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = &Paint::of(window, cx);
        let (Some(summary), Some(grid), Some(item)) =
            (self.summary.clone(), self.grid, self.items.get(ix).cloned())
        else {
            return div().into_any_element();
        };
        match item {
            Item::Heading(section) => self.heading(p, section, cx),
            Item::Artists => self.artists(p, cx),
            Item::Tiles(range) => match summary.albums.get(range) {
                Some(albums) => self.tiles(albums, grid, window.scale_factor(), cx),
                None => div().into_any_element(),
            },
            Item::Track(i) => match summary.tracks.get(i) {
                // Rows run to the page's edges, as in the browsers.
                Some(track) => div()
                    .mx(-px(tile::MARGIN))
                    .child(self.track(p, i, track, window, cx))
                    .into_any_element(),
                None => div().into_any_element(),
            },
        }
    }

    fn aim(&mut self, p: &Paint, grid: Grid, scale: f32, cx: &mut Context<Self>) {
        let Some(summary) = self.summary.clone() else {
            return;
        };
        // A shelf is a preview, a few screens at most: everything on it is
        // ranked, what is on screen first.
        let visible = self.list.logical_scroll_top().item_ix;
        let mut ranked = Vec::new();
        for (ix, item) in self.items.iter().enumerate() {
            let priority = if ix >= visible && ix < visible + 12 {
                Priority::Visible
            } else {
                Priority::Ahead(ix.abs_diff(visible) as f32)
            };
            match item {
                Item::Tiles(range) => {
                    for album in summary.albums.get(range.clone()).unwrap_or_default() {
                        ranked.push((Key::new(Art::Album(album.id), grid.side, scale), priority));
                    }
                }
                Item::Track(i) => {
                    if let Some(album) = summary.tracks.get(*i).and_then(|t| t.album_id) {
                        ranked.push((
                            Key::new(Art::Album(album), px(rows::SLEEVE), scale),
                            priority,
                        ));
                    }
                }
                _ => {}
            }
        }
        let _ = p;
        let owner = cx.entity_id();
        self.lib.covers.update(cx, |c, _| c.request(owner, ranked));
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let order: Vec<Playable> = self
            .summary
            .as_ref()
            .map(|s| s.tracks.iter().map(|t| Playable::Track(t.id)).collect())
            .unwrap_or_default();
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
        _: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let item = Playable::Album(album);
        match part {
            Part::Sleeve => {
                actions::play(&self.lib, &[item], false, cx).detach();
                self.lib
                    .nav
                    .update(cx, |nav, cx| nav.open(Page::Album(album), cx));
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
                    .summary
                    .as_ref()
                    .map(|s| s.tracks.iter().map(|t| t.id).collect())
                    .unwrap_or_default();
                let order = || ids.iter().map(|id| Playable::Track(*id)).collect();
                if rows::select(
                    &mut self.selection,
                    Playable::Track(track),
                    event,
                    order,
                    cx,
                ) {
                    actions::play_list(&self.lib, ids.clone(), index, cx);
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

impl Render for ShelfPage {
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

        if let (Some(width), Some(summary)) = (self.width.get(), self.summary.clone()) {
            let grid = Grid::for_width(width, &p.look.scale);
            if self.grid != Some(grid) {
                let top = self.restore.take().map_or_else(
                    || self.list.logical_scroll_top(),
                    |a| ListOffset {
                        item_ix: a.item_ix,
                        offset_in_item: a.offset,
                    },
                );
                self.grid = Some(grid);
                self.items = Rc::new(cut(&summary, grid.columns));
                self.list.reset(self.items.len());
                self.list.scroll_to(ListOffset {
                    item_ix: top.item_ix.min(self.items.len().saturating_sub(1)),
                    offset_in_item: top.offset_in_item,
                });
            }
            self.aim(p, grid, window.scale_factor(), cx);
        }
        self.tiles.begin();
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }

        let detail = self
            .summary
            .as_ref()
            .map(|s| {
                let mut parts = Vec::new();
                if s.artist_total > 0 {
                    parts.push(count(s.artist_total as usize, "artist", "artists"));
                }
                if s.album_total > 0 {
                    parts.push(count(s.album_total as usize, "album", "albums"));
                }
                if s.track_total > 0 {
                    parts.push(count(s.track_total as usize, "track", "tracks"));
                }
                parts.join(" · ")
            })
            .unwrap_or_default();
        let header = chrome::page_header(p, self.shelf.title(), detail, div());
        let empty = self
            .summary
            .as_ref()
            .is_some_and(|s| s.artists.is_empty() && s.albums.is_empty() && s.tracks.is_empty());
        let body = if empty {
            let (icon, title, detail) = self.shelf.empty();
            chrome::empty_state(icon, title, detail).into_any_element()
        } else {
            let view = cx.weak_entity();
            list(self.list.clone(), move |ix, window, cx| {
                view.update(cx, |this, cx| this.item(ix, window, cx))
                    .unwrap_or_else(|_| div().into_any_element())
            })
            .size_full()
            .px(px(tile::MARGIN))
            .into_any_element()
        };
        div()
            .key_context("Library")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::dismiss))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .typeset(p, Type::Body, p.ink)
            .child(measure)
            .child(header)
            .child(actions::list_menu(
                div()
                    .id("shelf-list")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(body)
                    .child(gpui_kit::base::Scrollbar::vertical(&self.list)),
                cx.weak_entity(),
                window,
                cx,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn a_shelf_is_its_filter() {
        assert!(Shelf::Favourites.filter().favourites);
        assert!(Shelf::Recent.filter().recent);
        assert!(Shelf::OnDevice.filter().downloaded);
        assert!(!Shelf::OnDevice.filter().favourites);
    }
}
