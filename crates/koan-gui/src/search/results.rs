//! Everything the query found, in sections: artists as pills, records as the
//! library's tiles, tracks as its rows. A result goes where it lives in the
//! library: an artist to their page, a record's title to its page, a track to
//! its record. A sleeve plays its record, asking first when that would take a
//! queue off. Each heading opens its browser over the whole of what the query
//! finds, the section being the head of that listing.
//!
//! One pick across every kind, as the album browser's: ⌘-click or ⇧-click
//! starts it, ⌘A takes everything shown. It survives a new query, so a pick
//! can gather from several searches, and ends when the page is left. What is
//! dragged out is the pick, or the one thing grabbed.
//!
//! The shelf's preview is a few dozen items at most, so the page lays them
//! all out and nothing is virtualised.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::Placement;

use super::replace;
use super::{Results, Search};
use crate::components::EmptyState;
use crate::library::covers::{Art, Covers, Key, Priority, UPLOAD_BYTES_PER_FRAME};
use crate::library::drag::{Playable, PlayableDrag};
use crate::library::nav::Page;
use crate::library::rows::{self, RowPart, RowState, TrackList};
use crate::library::selection::Selection;
use crate::library::tile::{self, Grid, Part, Record as TileRecord, TileHost, TileState, Tiles};
use crate::library::tokens::{Paint, Type, Typeset};
use crate::library::{Dismiss, Lib, SelectAll, actions, chrome};
use crate::theme::{Icon, KoanIcon, Space, Tone};

pub struct ResultsPage {
    lib: Lib,
    search: Entity<Search>,
    scroll: ScrollHandle,
    width: Rc<Cell<Option<Pixels>>>,
    selection: Selection,
    pick: Option<(u64, PlayableDrag)>,
    /// Each record's tile strings, made once per answer.
    records: HashMap<i64, Rc<TileRecord>>,
    /// The answer the records and the sleeve requests were made for.
    made_for: Option<(Rc<Results>, Option<Grid>)>,
    tiles: Tiles<ResultsPage>,
    menu: Option<actions::MenuTarget>,
    /// Records whose sleeve was clicked, until the engine has the command.
    starting: HashSet<i64>,
    /// A record whose sleeve was clicked with a queue to lose, waiting on
    /// the person.
    replacing: Option<i64>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ResultsPage {
    pub fn new(lib: Lib, search: Entity<Search>, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        let subscriptions = vec![
            // Cached under the window, and draws the accent.
            crate::theme::observe_accent(cx),
            cx.observe(&search, |this, _, cx| {
                this.scroll.set_offset(Point::default());
                cx.notify();
            }),
            cx.observe(&lib.feed, |_, _, cx| cx.notify()),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            cx.subscribe(
                &lib.covers,
                |this, _, gone: &crate::library::covers::Evicted, cx| {
                    let albums: HashSet<i64> = gone.albums.iter().copied().collect();
                    let tracks: HashSet<i64> = gone.tracks.iter().copied().collect();
                    this.selection.retain(|p| match p {
                        Playable::Album(id) => !albums.contains(&id),
                        Playable::Track(id) => !tracks.contains(&id),
                        Playable::Artist(_) => true,
                    });
                    cx.notify();
                },
            ),
            // Leaving the page ends the pick and any question it asked.
            cx.observe(&lib.nav, |this, nav, cx| {
                if nav.read(cx).current(cx) != Page::Search
                    && (!this.selection.is_clear() || this.replacing.is_some())
                {
                    this.selection.end();
                    this.replacing = None;
                    cx.notify();
                }
            }),
            cx.on_release(move |this, cx| this.lib.covers.update(cx, |c, _| c.release(id))),
        ];
        Self {
            lib,
            search,
            scroll: ScrollHandle::new(),
            width: Rc::new(Cell::new(None)),
            selection: Selection::default(),
            pick: None,
            records: HashMap::new(),
            made_for: None,
            tiles: Tiles::default(),
            menu: None,
            starting: HashSet::new(),
            replacing: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn results(&self, cx: &App) -> Option<Rc<Results>> {
        self.search.read(cx).results().cloned()
    }

    /// Everything shown, in the page's order: what ⇧ ranges and ⌘A take.
    fn order(&self, cx: &App) -> Vec<Playable> {
        let Some(r) = self.results(cx) else {
            return Vec::new();
        };
        let s = &r.summary;
        s.artists
            .iter()
            .map(|a| Playable::Artist(a.id))
            .chain(s.albums.iter().map(|a| Playable::Album(a.id)))
            .chain(s.tracks.iter().map(|t| Playable::Track(t.id)))
            .collect()
    }

    fn pick(&mut self) -> Option<PlayableDrag> {
        if self.selection.picked().is_empty() {
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

    /// What dragging `item` carries: the pick when it is in it.
    fn drag(&mut self, item: Playable, label: SharedString) -> PlayableDrag {
        match (self.selection.contains(item), self.pick()) {
            (true, Some(pick)) => pick,
            _ => PlayableDrag::new(&[item], label),
        }
    }

    /// A click that a pick takes first: while picking, or with ⌘ or ⇧ held.
    fn picked(&mut self, item: Playable, m: Modifiers, cx: &mut Context<Self>) -> bool {
        let order = self.order(cx);
        let took = self
            .selection
            .take(item, m.shift, m.secondary(cx) || m.shift, &order);
        if took {
            cx.notify();
        }
        took
    }

    fn open(&self, page: Page, cx: &mut Context<Self>) {
        self.lib.nav.update(cx, |nav, cx| nav.open(page, cx));
    }

    /// A section's heading: the browser over the whole of what the query
    /// finds there.
    fn see_all(&self, page: Page, cx: &mut Context<Self>) {
        let query = self.search.read(cx).query().to_string();
        self.lib
            .browse
            .update(cx, |browse, cx| browse.set_search(query, cx));
        self.open(page, cx);
    }

    fn play_album(&mut self, album: i64, cx: &mut Context<Self>) {
        self.replacing = None;
        self.starting.insert(album);
        cx.notify();
        let started = actions::play(&self.lib, &[Playable::Album(album)], false, cx);
        self.open(Page::Album(album), cx);
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

    /// Asks for the sleeves of a new answer or a new grid, all at once.
    fn aim(
        &mut self,
        results: &Rc<Results>,
        grid: Option<Grid>,
        scale: f32,
        cx: &mut Context<Self>,
    ) {
        if self
            .made_for
            .as_ref()
            .is_some_and(|(r, g)| Rc::ptr_eq(r, results) && *g == grid)
        {
            return;
        }
        if self
            .made_for
            .as_ref()
            .is_none_or(|(r, _)| !Rc::ptr_eq(r, results))
        {
            self.records.clear();
        }
        self.made_for = Some((results.clone(), grid));
        let s = &results.summary;
        let mut ranked: Vec<(Key, Priority)> = Vec::new();
        if let Some(grid) = grid {
            ranked.extend(s.albums.iter().map(|a| {
                (
                    Key::new(Art::Album(a.id), grid.side, scale),
                    Priority::Visible,
                )
            }));
        }
        ranked.extend(s.tracks.iter().filter_map(|t| {
            let key = Key::new(Art::Album(t.album_id?), px(rows::SLEEVE), scale);
            Some((key, Priority::Ahead(1.0)))
        }));
        let owner = cx.entity_id();
        self.lib.covers.update(cx, |c, _| c.request(owner, ranked));
    }

    fn heading(
        &self,
        p: &Paint,
        title: &'static str,
        shown: usize,
        total: u64,
        page: Page,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ink = p.ink;
        div()
            .id(title)
            .px(px(tile::MARGIN))
            .flex()
            .items_center()
            .gap(p.space(Space::Xs))
            .pt(p.space(Space::L))
            .pb(p.space(Space::S))
            .cursor_pointer()
            .typeset(p, Type::Fine, p.ink)
            .hover(move |s| s.text_color(ink).underline())
            .child(title)
            .child(
                div()
                    .typeset(p, Type::Fine, p.muted)
                    .child(total.max(shown as u64).to_string()),
            )
            .child(
                KoanIcon::new(Icon::Disclosure)
                    .role(Type::Fine)
                    .tone(Tone::Muted),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.see_all(page, cx)))
    }

    fn pill(&mut self, p: &Paint, id: i64, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let item = Playable::Artist(id);
        let selected = self.selection.contains(item);
        let drag = self.drag(item, name.to_string().into());
        let target = actions::MenuTarget {
            items: drag.items.clone(),
            album: None,
            after: None,
        };
        let (rule, hover) = (p.rule, p.hover);
        div()
            .id(("artist-pill", id as u64))
            .px(p.space(Space::M))
            .py(p.space(Space::Xs))
            .bg(rule)
            .border_1()
            .border_color(if selected { p.accent } else { rule })
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .typeset(p, Type::Meta, p.ink)
            .whitespace_nowrap()
            .child(name.to_string())
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                if !this.picked(item, e.modifiers(), cx) {
                    this.open(Page::Artist(id), cx);
                }
            }))
            .on_drag(drag, |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx))
            .on_mouse_down(MouseButton::Right, actions::mark(cx.weak_entity(), target))
            .into_any_element()
    }

    fn tile_rows(
        &mut self,
        results: &Results,
        grid: Grid,
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
        let height = grid.row - px(tile::LINE_GAP);
        let mut out = Vec::new();
        for chunk in results.summary.albums.chunks(grid.columns) {
            let mut tiles = Vec::with_capacity(chunk.len());
            for album in chunk {
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
                    show_artist: true,
                    side: grid.side,
                    sleeve,
                    drag,
                    pick,
                    after,
                };
                tiles.push(self.tiles.tile(&record, state, height, cx));
            }
            out.push(
                div()
                    .px(grid.margin)
                    .flex()
                    .gap(grid.gap)
                    .h(grid.row)
                    .children(tiles)
                    .into_any_element(),
            );
        }
        out
    }

    fn track_rows(
        &mut self,
        p: &Paint,
        results: &Results,
        scale: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let tracks = &results.summary.tracks;
        let lead_width = rows::lead_width(p, tracks.len(), window);
        let mut out = Vec::with_capacity(tracks.len());
        for (index, track) in tracks.iter().enumerate() {
            let item = Playable::Track(track.id);
            let selected = self.selection.contains(item);
            let drag = self.drag(item, track.title.clone().into());
            let sleeve = track.album_id.map(|album| {
                let key = Key::new(Art::Album(album), px(rows::SLEEVE), scale);
                self.lib.covers.update(cx, |c, _| c.sleeve(key))
            });
            let feed = self.lib.feed.read(cx);
            let state = RowState {
                lead: (index + 1).to_string().into(),
                lead_width,
                current: feed.playing_track == Some(track.id),
                playing: feed.playing,
                favourite: feed.is_favourite(item),
                selected,
                gathered: true,
                transfer: feed.transfers.get(&track.id).copied(),
                drag,
                height: rows::art_height(p),
            };
            out.push(rows::track_row(p, track, index, state, sleeve, None, cx));
        }
        out
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let order = self.order(cx);
        self.selection.select_all(&order);
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.replacing.take().is_some() {
            cx.notify();
        } else if !self.selection.is_clear() {
            self.selection.end();
            cx.notify();
        }
    }

    fn empty(&self, p: &Paint, cx: &App) -> Div {
        let search = self.search.read(cx);
        let state = if search.query().is_empty() {
            Some(EmptyState::new(Icon::Search, "search your library"))
        } else if search.is_searching() {
            None
        } else {
            Some(
                EmptyState::new(Icon::Search, "nothing found").detail(format!(
                    "no artists, albums or tracks match “{}”.",
                    search.query()
                )),
            )
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.bg)
            .children(state)
    }
}

impl actions::Menus for ResultsPage {
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

impl TileHost for ResultsPage {
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
        if self.picked(item, modifiers, cx) {
            return;
        }
        match part {
            Part::Sleeve => {
                if replace::queued(cx) > 0 {
                    self.replacing = Some(album);
                    cx.notify();
                } else {
                    self.play_album(album, cx);
                }
            }
            Part::Title => self.open(Page::Album(album), cx),
            Part::Artist(artist) => self.open(Page::Artist(artist), cx),
            Part::Heart => self
                .lib
                .feed
                .update(cx, |f, cx| f.toggle_favourite(item, cx)),
        }
    }
}

impl TrackList for ResultsPage {
    fn clicked(
        &mut self,
        _: usize,
        track: i64,
        part: RowPart,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        match part {
            // A track lives on its record: that is where it is played from.
            RowPart::Row => {
                if self.picked(Playable::Track(track), event.modifiers(), cx) {
                    return;
                }
                let album = self.results(cx).and_then(|r| {
                    r.summary
                        .tracks
                        .iter()
                        .find(|t| t.id == track)
                        .and_then(|t| t.album_id)
                });
                if let Some(album) = album {
                    self.open(Page::Album(album), cx);
                }
            }
            RowPart::Artist(id) => self.open(Page::Artist(id), cx),
            RowPart::Album(id) => self.open(Page::Album(id), cx),
            RowPart::Heart => self
                .lib
                .feed
                .update(cx, |f, cx| f.toggle_favourite(Playable::Track(track), cx)),
        }
    }
}

impl chrome::Picking for ResultsPage {
    fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }
}

impl Render for ResultsPage {
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

        let results = self.results(cx).filter(|r| !r.is_empty());
        let Some(results) = results else {
            return div()
                .relative()
                .size_full()
                .child(measure)
                .child(self.empty(p, cx));
        };
        let scale = window.scale_factor();
        let grid = self.width.get().map(|w| Grid::for_width(w, &p.look.scale));
        self.aim(&results, grid, scale, cx);
        self.tiles.begin();
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }

        let s = &results.summary;
        let mut body = div().flex().flex_col().pb(p.space(Space::L)).child(
            div()
                .px(px(tile::MARGIN))
                .pt(p.space(Space::L))
                .flex()
                .typeset(p, Type::Title, p.strong)
                .child("results for ")
                .child(format!("“{}”", results.query)),
        );
        if !s.artists.is_empty() {
            let pills: Vec<AnyElement> = s
                .artists
                .iter()
                .map(|a| self.pill(p, a.id, &a.name, cx))
                .collect();
            body = body
                .child(self.heading(
                    p,
                    "artists",
                    s.artists.len(),
                    s.artist_total,
                    Page::Artists,
                    cx,
                ))
                .child(
                    div()
                        .px(px(tile::MARGIN))
                        .flex()
                        .flex_wrap()
                        .gap(p.space(Space::S))
                        .children(pills),
                );
        }
        if !s.albums.is_empty() {
            body = body.child(self.heading(
                p,
                "albums",
                s.albums.len(),
                s.album_total,
                Page::Albums,
                cx,
            ));
            if let Some(grid) = grid {
                body = body.children(self.tile_rows(&results, grid, scale, cx));
            }
        }
        if !s.tracks.is_empty() {
            body = body
                .child(self.heading(p, "tracks", s.tracks.len(), s.track_total, Page::Tracks, cx))
                .children(self.track_rows(p, &results, scale, window, cx));
        }

        let asking = self.replacing.map(|album| {
            let view = cx.entity();
            let (play, add, cancel) = (view.clone(), view.clone(), view);
            replace::ask(
                p,
                replace::queued(cx),
                "play",
                Rc::new(move |_, cx| play.update(cx, |this, cx| this.play_album(album, cx))),
                Rc::new(move |_, cx| {
                    add.update(cx, |this, cx| {
                        this.replacing = None;
                        actions::queue(&this.lib, &[Playable::Album(album)], Placement::Append, cx)
                            .detach();
                        cx.notify();
                    })
                }),
                Rc::new(move |_, cx| {
                    cancel.update(cx, |this, cx| {
                        this.replacing = None;
                        cx.notify();
                    })
                }),
            )
        });

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
            .child(actions::list_menu(
                div()
                    .id("search-results")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
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
            .children(asking)
    }
}
