//! The album browser: every record in the library as a grid of sleeves,
//! narrowed and ordered by the database.
//!
//! A uniform list of rows, so only the rows on screen are laid out. As it
//! scrolls it tells the artwork cache what is on screen, where a flick will
//! come to rest, and which rows it is heading for, ported from the Mac grid's
//! `aim`: the sleeves arrive before their tiles do.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use koan_ffi::{Album, AlbumSort, BrowseFilter};

use super::covers::{Art, Covers, Key, Priority};
use super::drag::Playable;
use super::nav::Page;
use super::selection::Selection;
use super::tile::{self, Grid, Part, TileHost, TileState};
use super::tokens::{self, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome};

#[derive(Clone, PartialEq)]
struct Query {
    search: Option<String>,
    filter: BrowseFilter,
    sort: AlbumSort,
    seed: i64,
}

/// Where the scroll is, how fast it is going, and, in a flick's momentum,
/// how quickly it is slowing.
#[derive(Default)]
struct Heading {
    offset: f32,
    at: Option<Instant>,
    /// Points per second, positive downwards.
    velocity: f32,
    /// The velocity falls as e^(-decay·t), so a flick comes to rest
    /// `velocity / decay` points on. Only known where the platform reports a
    /// gesture's end before its momentum (macOS); never set elsewhere.
    decay: Option<f32>,
    /// The fingers lifted and the scroll goes on: momentum.
    lifted: bool,
    coasting: bool,
    visible: Range<usize>,
}

pub struct AlbumGrid {
    lib: Lib,
    albums: Rc<Vec<Album>>,
    /// The page in its order, for ⇧ ranges and ⌘A.
    order: Vec<Playable>,
    loaded: Option<(Query, Option<u64>)>,
    loading: Option<(Query, Option<u64>)>,
    request: u64,
    _load: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    width: Rc<Cell<Pixels>>,
    /// The window's scale factor as of the last frame, for the device size
    /// sleeves are decoded at.
    scale: Cell<f32>,
    grid: Grid,
    heading: Rc<RefCell<Heading>>,
    stopped: Option<Task<()>>,
    selection: Selection,
    starting: HashSet<i64>,
    /// Where to put the scroll back to once the records are in.
    restore: Option<Pixels>,
    rewinds: u64,
    focus: FocusHandle,
    search: Entity<chrome::SearchField>,
    filters: Entity<chrome::FilterPanel>,
    _subscriptions: Vec<Subscription>,
}

impl AlbumGrid {
    pub fn new(lib: Lib, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        let subscriptions = vec![
            cx.observe(&lib.browse, |this, _, cx| this.reload(cx)),
            cx.observe(&lib.feed, |this, _, cx| {
                this.reload(cx);
                cx.notify();
            }),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            cx.subscribe(&lib.covers, |this, _, gone: &super::covers::Evicted, cx| {
                let albums: HashSet<i64> = gone.albums.iter().copied().collect();
                this.selection
                    .retain(|p| !matches!(p, Playable::Album(id) if albums.contains(&id)));
                cx.notify();
            }),
            cx.on_release(move |this, cx| this.lib.covers.update(cx, |c, _| c.release(id))),
        ];
        let rewinds = lib.nav.read(cx).rewinds(Page::Albums);
        let restore = lib.nav.read(cx).offset(Page::Albums);
        let search = cx.new(|cx| chrome::SearchField::new(lib.browse.clone(), window, cx));
        let filters = cx.new(|cx| chrome::FilterPanel::new(lib.clone(), true, window, cx));
        let mut this = Self {
            lib,
            albums: Rc::new(Vec::new()),
            order: Vec::new(),
            loaded: None,
            loading: None,
            request: 0,
            _load: None,
            scroll: UniformListScrollHandle::new(),
            width: Rc::new(Cell::new(window.viewport_size().width)),
            scale: Cell::new(window.scale_factor()),
            grid: Grid::for_width(window.viewport_size().width),
            heading: Rc::default(),
            stopped: None,
            selection: Selection::default(),
            starting: HashSet::new(),
            restore,
            rewinds,
            focus: cx.focus_handle(),
            search,
            filters,
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    fn query(&self, cx: &App) -> Query {
        let browse = self.lib.browse.read(cx);
        Query {
            search: browse.search(),
            filter: browse.filter.clone(),
            sort: browse.effective_album_sort(),
            seed: browse.seed,
        }
    }

    /// Ask for the listing again when what it answers to has moved: the
    /// query, or the library under it. What is on screen stays until the new
    /// listing lands, so the scroll and the pick are not lost meanwhile.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let version = self.lib.feed.read(cx).library_version;
        let wanted = (self.query(cx), version);
        if self.loaded.as_ref() == Some(&wanted) || self.loading.as_ref() == Some(&wanted) {
            return;
        }
        self.request += 1;
        let request = self.request;
        self.loading = Some(wanted.clone());
        let engine = self.lib.engine.clone();
        let q = wanted.0.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let albums = engine
                .albums(None, q.sort, q.seed, q.search, q.filter)
                .await;
            this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                this.loading = None;
                match albums {
                    Ok(albums) => {
                        this.order = albums.iter().map(|a| Playable::Album(a.id)).collect();
                        this.albums = Rc::new(albums);
                        this.loaded = Some(wanted);
                        if let Some(y) = this.restore.take() {
                            this.scroll
                                .0
                                .borrow()
                                .base_handle
                                .set_offset(point(px(0.0), -y));
                        }
                    }
                    Err(e) => log::warn!("albums: {e}"),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Rank what the cache should work on, from the rows on screen and the
    /// scroll's heading. Called from the list's layout every painted frame,
    /// and once more when the scroll stops; never notifies.
    fn aim(&mut self, visible: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.scroll.0.borrow();
        let offset = -f32::from(state.base_handle.offset().y);
        let viewport = f32::from(state.base_handle.bounds().size.height);
        drop(state);
        let now = Instant::now();
        let moved = {
            let mut h = self.heading.borrow_mut();
            h.visible = visible.clone();
            let moved = h.at.is_none() || (offset - h.offset).abs() > 0.5;
            if moved {
                let elapsed = h.at.map_or(0.0, |at| now.duration_since(at).as_secs_f32());
                if elapsed > 0.0 && elapsed < 0.1 {
                    let instant = (offset - h.offset) / elapsed;
                    let velocity = h.velocity * 0.6 + instant * 0.4;
                    if h.coasting
                        && velocity * h.velocity > 0.0
                        && velocity.abs() < h.velocity.abs()
                    {
                        let decay = (h.velocity.abs() / velocity.abs()).ln() / elapsed;
                        let blended = h.decay.map_or(decay, |d| d * 0.7 + decay * 0.3);
                        h.decay = Some(blended.clamp(1.0, 12.0));
                    } else if !h.coasting {
                        h.decay = None;
                    }
                    h.velocity = velocity;
                } else {
                    h.velocity = 0.0;
                    h.decay = None;
                }
                h.offset = offset;
                h.at = Some(now);
            }
            moved
        };
        if moved {
            self.lib
                .nav
                .update(cx, |nav, _| nav.remember_offset(Page::Albums, px(offset)));
            // The scroll has stopped when nothing has moved it for a moment:
            // the heading goes still and the cache is re-aimed three rows
            // either way.
            self.stopped = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                this.update(cx, |this, cx| {
                    {
                        let mut h = this.heading.borrow_mut();
                        h.velocity = 0.0;
                        h.decay = None;
                    }
                    this.send_aim(viewport, cx);
                })
                .ok();
            }));
        }
        self.scale.set(window.scale_factor());
        self.send_aim(viewport, cx);
    }

    fn send_aim(&mut self, viewport: f32, cx: &mut Context<Self>) {
        let scale = self.scale.get();
        let ranked = rank(
            &self.heading.borrow(),
            &self.grid,
            viewport,
            self.albums.len(),
            |row| {
                let start = row * self.grid.columns;
                self.albums[start..(start + self.grid.columns).min(self.albums.len())]
                    .iter()
                    .map(|a| Key::new(Art::Album(a.id), self.grid.side, scale))
                    .collect()
            },
        );
        let owner = cx.entity_id();
        self.lib
            .covers
            .update(cx, |covers, _| covers.request(owner, ranked));
    }

    fn row(&mut self, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let start = row * self.grid.columns;
        let end = (start + self.grid.columns).min(self.albums.len());
        let albums = self.albums.clone();
        let scale = self.scale.get();
        let feed = self.lib.feed.read(cx);
        let states: Vec<TileState> = albums[start..end]
            .iter()
            .map(|a| TileState {
                favourite: feed.is_favourite(Playable::Album(a.id)),
                playing: feed.playing_album == Some(a.id),
                arriving: feed.arriving.contains_key(&a.id),
                selecting: self.selection.is_active(),
                selected: self.selection.contains(Playable::Album(a.id)),
                starting: self.starting.contains(&a.id),
                show_artist: true,
            })
            .collect();
        let mut tiles = Vec::with_capacity(end - start);
        for (album, state) in albums[start..end].iter().zip(states) {
            let key = Key::new(Art::Album(album.id), self.grid.side, scale);
            let sleeve = self
                .lib
                .covers
                .update(cx, |c: &mut Covers, _| c.sleeve(key));
            tiles.push(tile::tile(album, self.grid.side, sleeve, state, self, cx));
        }
        div()
            .flex()
            .gap(self.grid.gap)
            .px(self.grid.margin)
            .h(self.grid.row)
            .children(tiles)
            .into_any_element()
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection.select_all(&self.order);
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_active() {
            self.selection.end();
            cx.notify();
        }
    }

    fn scrolled(&mut self, event: &ScrollEvent) {
        let mut h = self.heading.borrow_mut();
        match event.touch_phase {
            TouchPhase::Started => {
                h.lifted = false;
                h.coasting = false;
            }
            TouchPhase::Ended => h.lifted = true,
            _ => h.coasting = h.lifted,
        }
    }
}

/// Every key worth working on and how much it matters: the rows on screen,
/// then the screenful where a flick will come to rest, then the rows in the
/// direction of travel as far as the scroll reaches in the next few hundred
/// milliseconds, and a few past them, with rows behind; at rest, three rows
/// either way. Ported from the Mac grid's `aim`.
fn rank(
    heading: &Heading,
    grid: &Grid,
    viewport: f32,
    items: usize,
    keys_of_row: impl Fn(usize) -> Vec<Key>,
) -> Vec<(Key, Priority)> {
    let rows = grid.rows(items);
    let row = f32::from(grid.row);
    if rows == 0 || row <= 0.0 || viewport <= 0.0 {
        return Vec::new();
    }
    let (top, bottom) = (heading.offset, heading.offset + viewport);
    let speed = heading.velocity.abs();
    let still = speed < 60.0;
    let reach = if still {
        3.0 * row
    } else {
        (speed * 0.3).min(3.0 * viewport) + 4.0 * row
    };
    let behind = if still { 3.0 * row } else { 2.0 * row };
    let (from, to) = if heading.velocity >= 0.0 {
        (top - behind, bottom + reach)
    } else {
        (top - reach, bottom + behind)
    };
    let end = (rows as f32 * row - viewport).max(0.0);
    let rest = heading
        .decay
        .map(|decay| (top + heading.velocity / decay).clamp(0.0, end));

    let row_of = |y: f32| ((y / row).floor().max(0.0) as usize).min(rows - 1);
    let distance = |r: usize, top: f32, bottom: f32| {
        let (rt, rb) = (r as f32 * row, (r + 1) as f32 * row);
        (rt - bottom).max(top - rb).max(0.0)
    };
    let mut ranked: Vec<(Key, Priority)> = Vec::new();
    let mut push = |r: usize, p: Priority, ranked: &mut Vec<(Key, Priority)>| {
        for key in keys_of_row(r) {
            ranked.push((key, p));
        }
    };
    for r in row_of(from.max(0.0))..=row_of(to.max(0.0)) {
        let d = distance(r, top, bottom);
        let p = if d == 0.0 && heading.visible.contains(&r) {
            Priority::Visible
        } else {
            Priority::Ahead(d)
        };
        push(r, p, &mut ranked);
    }
    if let Some(y) = rest {
        for r in row_of((y - row).max(0.0))..=row_of(y + viewport + row) {
            push(
                r,
                Priority::Landing(distance(r, y, y + viewport)),
                &mut ranked,
            );
        }
    }
    for r in heading.visible.clone() {
        push(r, Priority::Visible, &mut ranked);
    }
    ranked
}

impl chrome::Picking for AlbumGrid {
    fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }
}

impl TileHost for AlbumGrid {
    fn lib(&self) -> &Lib {
        &self.lib
    }

    fn selection(&self) -> &Selection {
        &self.selection
    }

    fn starting(&self, album: i64) -> bool {
        self.starting.contains(&album)
    }

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
        if self
            .selection
            .take(item, modifiers.shift, modifiers.secondary(), &self.order)
        {
            cx.notify();
            return;
        }
        match part {
            // The record is where its tracks are, so a click on its sleeve
            // opens it as well as playing it, as the Mac's does.
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
                .update(cx, |feed, cx| feed.toggle_favourite(item, cx)),
        }
    }
}

impl Render for AlbumGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scale.set(window.scale_factor());
        let rewinds = self.lib.nav.read(cx).rewinds(Page::Albums);
        if rewinds != self.rewinds {
            self.rewinds = rewinds;
            self.scroll
                .0
                .borrow()
                .base_handle
                .set_offset(Point::default());
        }
        let grid = Grid::for_width(self.width.get());
        if grid != self.grid {
            self.grid = grid;
        }
        let count = self.albums.len();
        let narrowed = self.lib.browse.read(cx).is_narrowed();
        let header = chrome::page_header(
            "albums",
            chrome::count(count as u64, "album", "albums"),
            div()
                .flex()
                .items_center()
                .gap(tokens::space::s16())
                .child(self.search.clone())
                .child(chrome::filter_button(&self.filters, &self.lib, cx))
                .child(chrome::album_sort_button(&self.lib, cx))
                .child(chrome::select_toggle(
                    self.selection.is_active(),
                    cx.listener(|this, _, _, cx| {
                        if this.selection.is_active() {
                            this.selection.end();
                        } else {
                            this.selection.begin(None);
                        }
                        cx.notify();
                    }),
                )),
        );

        let width = self.width.clone();
        let entity = cx.entity_id();
        let measure = canvas(
            move |bounds, _, cx| {
                if width.get() != bounds.size.width {
                    width.set(bounds.size.width);
                    cx.notify(entity);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let body = if count == 0 && self.loaded.is_some() {
            chrome::empty_state(
                if narrowed {
                    "nothing matches"
                } else {
                    "no albums yet"
                },
                if narrowed {
                    "try a different filter."
                } else {
                    "add a music folder in settings, or sign in to a server."
                },
            )
            .into_any_element()
        } else {
            uniform_list(
                "albums",
                self.grid.rows(count),
                cx.processor(|this, range: Range<usize>, window, cx| {
                    this.aim(range.clone(), window, cx);
                    range.map(|row| this.row(row, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .pt(self.grid.margin)
            .size_full()
            .into_any_element()
        };

        div()
            .key_context("Library")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::dismiss))
            .size_full()
            .flex()
            .flex_col()
            .bg(tokens::BG)
            .role(Type::Body, tokens::INK)
            .child(header)
            .child(
                div()
                    .id("album-grid")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .on_scroll(cx.listener(|this, e: &ScrollEvent, _, _| this.scrolled(e)))
                    .child(measure)
                    .child(body),
            )
            .when(self.selection.is_active(), |d| {
                d.child(chrome::selection_bar(
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

    fn grid() -> Grid {
        Grid::for_width(px(800.0))
    }

    fn keys(row: usize) -> Vec<Key> {
        vec![Key {
            art: Art::Album(row as i64),
            px: 64,
        }]
    }

    fn priority(ranked: &[(Key, Priority)], row: usize) -> Option<Priority> {
        ranked
            .iter()
            .filter(|(k, _)| k.art == Art::Album(row as i64))
            .map(|(_, p)| *p)
            .reduce(|a, b| if b < a { b } else { a })
    }

    #[test]
    fn at_rest_three_rows_either_way_and_the_screen_first() {
        let g = grid();
        let row = f32::from(g.row);
        let heading = Heading {
            offset: 10.0 * row,
            visible: 10..13,
            ..Heading::default()
        };
        let ranked = rank(&heading, &g, 3.0 * row, 1000 * g.columns, keys);
        assert_eq!(priority(&ranked, 11), Some(Priority::Visible));
        assert!(matches!(priority(&ranked, 7), Some(Priority::Ahead(_))));
        assert!(matches!(priority(&ranked, 15), Some(Priority::Ahead(_))));
        assert_eq!(priority(&ranked, 5), None);
        assert_eq!(priority(&ranked, 17), None);
    }

    #[test]
    fn a_fast_scroll_reaches_further_ahead_than_behind_and_a_flick_ranks_its_landing() {
        let g = grid();
        let row = f32::from(g.row);
        let heading = Heading {
            offset: 10.0 * row,
            velocity: 4000.0,
            decay: Some(2.0),
            visible: 10..13,
            ..Heading::default()
        };
        let ranked = rank(&heading, &g, 3.0 * row, 1000 * g.columns, keys);
        assert!(matches!(priority(&ranked, 20), Some(Priority::Ahead(_))));
        assert_eq!(priority(&ranked, 6), None);
        // 2000 points on, where the flick comes to rest.
        let rest = ((10.0 * row + 2000.0) / row) as usize + 1;
        assert!(matches!(
            priority(&ranked, rest),
            Some(Priority::Landing(_))
        ));
    }

    #[test]
    fn an_empty_grid_ranks_nothing() {
        assert!(rank(&Heading::default(), &grid(), 600.0, 0, keys).is_empty());
    }
}
