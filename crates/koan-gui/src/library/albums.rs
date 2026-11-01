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

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{AlbumSort, BrowseFilter};

use super::covers::{Art, Covers, Key, Priority, Sleeve, UPLOAD_BYTES_PER_FRAME};
use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::selection::Selection;
use super::tile::{self, Grid, Part, Record, TileHost, TileState, Tiles};
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome};
use crate::theme::Space;

/// How long typing in the filter field waits for the next keystroke before
/// asking again: each answer is the whole listing, and asking per keystroke
/// queues listings nobody will see.
const TYPING: Duration = Duration::from_millis(80);

#[derive(Clone, PartialEq)]
struct Query {
    search: Option<String>,
    filter: BrowseFilter,
    sort: AlbumSort,
    seed: i64,
}

impl Query {
    /// Only the search differs: a keystroke.
    fn typed(&self, other: &Query) -> bool {
        self.search != other.search
            && self.filter == other.filter
            && self.sort == other.sort
            && self.seed == other.seed
    }
}

/// What the cover ranking depends on, coarsely, within one listing: the rows
/// on screen, the pace in steps of 200 pt/s, the row a flick lands on and the
/// draw size. Unchanged, the ranking would be too; a new listing clears it.
#[derive(PartialEq)]
struct Aimed {
    visible: Range<usize>,
    pace: i32,
    landing: Option<i32>,
    px: u32,
}

/// Where the scroll is, how fast it is going, and, in a flick's momentum,
/// how quickly it is slowing.
#[derive(Default)]
struct Heading {
    /// Distance scrolled from the top of the content, the list's top margin
    /// included.
    offset: f32,
    at: Option<Instant>,
    /// Points per second, positive downwards.
    velocity: f32,
    /// The velocity falls as e^(-decay·t), so a flick comes to rest
    /// `velocity / decay` points on. Only known where the platform reports a
    /// gesture's end before its momentum (macOS); never set elsewhere, and
    /// nothing depends on it being set.
    decay: Option<f32>,
    /// The fingers lifted and the scroll goes on: momentum.
    lifted: bool,
    coasting: bool,
    visible: Range<usize>,
    viewport: f32,
}

pub struct AlbumGrid {
    lib: Lib,
    /// The listing, as its tiles draw it.
    records: Rc<Vec<Rc<Record>>>,
    tiles: Tiles<AlbumGrid>,
    /// What the cover cache was last asked for, so it is asked again only
    /// when the rows on screen or the scroll's pace change.
    aimed: Option<Aimed>,
    /// The page in its order, for ⇧ ranges and ⌘A.
    order: Vec<Playable>,
    loaded: Option<(Query, u64)>,
    loading: Option<(Query, u64)>,
    request: u64,
    _load: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    /// The list's width as last laid out; nothing is drawn in it until known.
    width: Rc<Cell<Option<Pixels>>>,
    /// The window's scale factor as of the last frame, for the device size
    /// sleeves are decoded at.
    scale: Cell<f32>,
    grid: Option<Grid>,
    heading: Rc<RefCell<Heading>>,
    stopped: Option<Task<()>>,
    selection: Selection,
    /// The pick's drag payload, and the selection revision it was built for.
    pick: Option<(u64, PlayableDrag)>,
    /// What the list's context menu acts on: the row last right-clicked.
    menu: Option<actions::MenuTarget>,
    starting: HashSet<i64>,
    /// Where to put the scroll back to once the records are in.
    restore: Option<Pixels>,
    rewinds: u64,
    focus: FocusHandle,
    search: Entity<chrome::SearchField>,
    filters: Entity<chrome::FilterPanel>,
    /// The list's height as of the last frame, to tell when it moved.
    viewport: f32,
    /// Frames asked for while the list has no height.
    waits: u8,
    _subscriptions: Vec<Subscription>,
}

impl AlbumGrid {
    pub fn new(lib: Lib, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        let subscriptions = vec![
            // Cached under the window, and draws the accent.
            crate::theme::observe_accent(cx),
            cx.observe(&lib.browse, |this, _, cx| this.reload(cx)),
            cx.observe(&lib.nav, |_, _, cx| cx.notify()),
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
            cx.on_release(move |this, cx| {
                let offset = px(this.heading.borrow().offset);
                this.lib
                    .nav
                    .update(cx, |nav, _| nav.remember_offset(Page::Albums, offset));
                this.lib.covers.update(cx, |c, _| c.release(id));
            }),
        ];
        let rewinds = lib.nav.read(cx).rewinds(Page::Albums);
        let restore = lib.nav.read(cx).offset(Page::Albums);
        let search = cx.new(|cx| chrome::SearchField::new(lib.browse.clone(), window, cx));
        let filters = cx.new(|cx| chrome::FilterPanel::new(lib.clone(), true, window, cx));
        let mut this = Self {
            lib,
            records: Rc::new(Vec::new()),
            tiles: Tiles::default(),
            aimed: None,
            order: Vec::new(),
            loaded: None,
            loading: None,
            request: 0,
            _load: None,
            scroll: UniformListScrollHandle::new(),
            width: Rc::new(Cell::new(None)),
            scale: Cell::new(window.scale_factor()),
            grid: None,
            heading: Rc::default(),
            stopped: None,
            selection: Selection::default(),
            pick: None,
            menu: None,
            starting: HashSet::new(),
            restore,
            rewinds,
            focus: cx.focus_handle(),
            search,
            filters,
            viewport: 0.0,
            waits: 0,
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
    /// listing lands, so the scroll and the pick are not lost meanwhile. A
    /// keystroke waits a moment for the next one.
    fn reload(&mut self, cx: &mut Context<Self>) {
        // The engine's first report comes within a frame or two; loading
        // before it would load twice.
        let Some(version) = self.lib.feed.read(cx).library_version else {
            return;
        };
        let wanted = (self.query(cx), version);
        if self.loaded.as_ref() == Some(&wanted) || self.loading.as_ref() == Some(&wanted) {
            return;
        }
        let typed = self
            .loading
            .as_ref()
            .or(self.loaded.as_ref())
            .is_some_and(|(q, v)| *v == version && wanted.0.typed(q));
        self.request += 1;
        let request = self.request;
        self.loading = Some(wanted.clone());
        let engine = self.lib.engine.clone();
        let q = wanted.0.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            if typed {
                cx.background_executor().timer(TYPING).await;
            }
            let albums = engine
                .albums(None, q.sort, q.seed, q.search, q.filter)
                .await;
            // Strings made once per listing, off the main thread.
            let albums = match albums {
                Ok(albums) => Ok(cx
                    .background_executor()
                    .spawn(async move { albums.iter().map(Record::of).collect::<Vec<_>>() })
                    .await),
                Err(e) => Err(e),
            };
            this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                this.loading = None;
                match albums {
                    Ok(records) => {
                        this.order = records.iter().map(|r| Playable::Album(r.id)).collect();
                        this.records = Rc::new(records.into_iter().map(Rc::new).collect());
                        this.aimed = None;
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

    /// Note where the scroll is from the list's layout, every painted frame,
    /// and aim the cache from it. Never notifies.
    fn aim(&mut self, visible: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.scroll.0.borrow();
        let offset = -f32::from(state.base_handle.offset().y);
        let viewport = f32::from(state.base_handle.bounds().size.height);
        drop(state);
        self.scale.set(window.scale_factor());
        let now = Instant::now();
        let moved = {
            let mut h = self.heading.borrow_mut();
            h.visible = visible;
            h.viewport = viewport;
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
                    this.send_aim(cx);
                })
                .ok();
            }));
        }
        self.send_aim(cx);
    }

    /// What happens once a frame, before the rows are built: the rows on
    /// screen from the scroll's offset (rows are all one height), the cover
    /// cache aimed from them, a few new sleeves taken into the atlas (another
    /// frame asked for while more wait), and the frame's tiles begun.
    fn frame(&mut self, grid: Grid, count: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (offset, viewport) = {
            let state = self.scroll.0.borrow();
            (
                -f32::from(state.base_handle.offset().y),
                f32::from(state.base_handle.bounds().size.height),
            )
        };
        // The list's bounds come from its last layout: none yet on the first
        // frame it is drawn, and the old ones just after a resize. Another
        // frame follows to aim from the real ones; a list at no height at all
        // (a collapsed pane) gets two tries and then waits for a resize. An
        // empty listing draws no list, so it has no bounds to wait for.
        if count > 0 {
            if viewport != self.viewport {
                self.viewport = viewport;
                self.waits = 0;
                window.request_animation_frame();
            } else if viewport <= 0.0 && self.waits < 2 {
                self.waits += 1;
                window.request_animation_frame();
            }
        }
        let row = f32::from(grid.row).max(1.0);
        let top = (offset - f32::from(grid.margin)).max(0.0);
        let rows = grid.rows(count);
        let last = (((top + viewport) / row).ceil() as usize).min(rows);
        let first = ((top / row).floor() as usize).min(last);
        self.aim(first..last, window, cx);
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }
        self.tiles.begin();
    }

    fn send_aim(&mut self, cx: &mut Context<Self>) {
        let Some(grid) = self.grid else { return };
        let scale = self.scale.get();
        let records = self.records.clone();
        let shape = {
            let h = self.heading.borrow();
            let row = f32::from(grid.row).max(1.0);
            Aimed {
                visible: h.visible.clone(),
                pace: (h.velocity / 200.0) as i32,
                landing: h.decay.map(|d| ((h.offset + h.velocity / d) / row) as i32),
                px: Key::new(Art::Album(0), grid.side, scale).px,
            }
        };
        if self.aimed.as_ref() == Some(&shape) {
            return;
        }
        self.aimed = Some(shape);
        let ranked = rank(&self.heading.borrow(), &grid, records.len(), |row| {
            let start = row * grid.columns;
            let end = (start + grid.columns).min(records.len());
            records
                .get(start..end)
                .unwrap_or_default()
                .iter()
                .map(|r| Key::new(Art::Album(r.id), grid.side, scale))
                .collect()
        });
        let owner = cx.entity_id();
        self.lib
            .covers
            .update(cx, |covers, _| covers.request(owner, ranked));
    }

    /// The pick's drag payload, built once per change to the pick.
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

    fn row(&mut self, row: usize, grid: Grid, cx: &mut Context<Self>) -> AnyElement {
        let start = row * grid.columns;
        let end = (start + grid.columns).min(self.records.len());
        let records = self.records.clone();
        let scale = self.scale.get();
        let pick = self.pick();
        let revision = self.selection.revision();
        let entity = cx.entity();
        let after: actions::After = Some(std::rc::Rc::new(move |cx: &mut App| {
            entity.update(cx, |this, cx| {
                this.selection.end();
                cx.notify();
            })
        }));
        let height = grid.row - px(tile::LINE_GAP);
        let mut tiles = Vec::with_capacity(end.saturating_sub(start));
        for record in records.get(start..end).unwrap_or_default() {
            let item = Playable::Album(record.id);
            let selected = self.selection.contains(item);
            let (drag, pick, after) = match (&pick, selected) {
                (Some(pick), true) => (pick.clone(), Some(revision), after.clone()),
                _ => (PlayableDrag::new(&[item], record.title.clone()), None, None),
            };
            let key = Key::new(Art::Album(record.id), grid.side, scale);
            let sleeve = self
                .lib
                .covers
                .update(cx, |c: &mut Covers, _| c.sleeve(key));
            // Only rows on screen: uniform_list also builds a row to measure.
            if let Some(bench) = self
                .lib
                .bench
                .as_ref()
                .filter(|_| self.heading.borrow().visible.contains(&row))
            {
                let image = match &sleeve {
                    Sleeve::Ready(image) => Some(image.id),
                    _ => None,
                };
                let standin = image.is_some() && !self.lib.covers.read(cx).has(key);
                bench
                    .borrow_mut()
                    .tile(record.id, &sleeve, standin, image, Instant::now());
            }
            let feed = self.lib.feed.read(cx);
            let state = TileState {
                favourite: feed.is_favourite(item),
                playing: feed.playing_album == Some(record.id),
                arriving: feed.arriving.contains_key(&record.id),
                selecting: self.selection.is_active(),
                selected,
                starting: self.starting.contains(&record.id),
                show_artist: true,
                side: grid.side,
                sleeve,
                drag,
                pick,
                after,
            };
            tiles.push(self.tiles.tile(record, state, height, cx));
        }
        div()
            .flex()
            .gap(grid.gap)
            .px(grid.margin)
            .h(grid.row)
            .children(tiles)
            .into_any_element()
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection.select_all(&self.order);
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selection.is_clear() {
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

    /// A new column count keeps the record at the top of the screen at the
    /// top, rather than the pixel offset, which would land in another band
    /// of records.
    fn relayout(&mut self, grid: Grid) {
        if let Some(old) = self.grid {
            let top = (self.heading.borrow().offset - f32::from(old.margin)).max(0.0);
            let first = (top / f32::from(old.row)) as usize * old.columns;
            let row = first / grid.columns;
            let y = if row == 0 {
                0.0
            } else {
                f32::from(grid.margin) + row as f32 * f32::from(grid.row)
            };
            self.scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.0), -px(y)));
        }
        self.grid = Some(grid);
    }
}

/// Every key worth working on and how much it matters: the rows on screen,
/// then the screenful where a flick will come to rest, then the rows in the
/// direction of travel as far as the scroll reaches in the next few hundred
/// milliseconds, and a few past them, with rows behind; at rest, three rows
/// either way. Ported from the Mac grid's `aim`. Rows are laid out below the
/// grid's top margin.
fn rank(
    heading: &Heading,
    grid: &Grid,
    items: usize,
    keys_of_row: impl Fn(usize) -> Vec<Key>,
) -> Vec<(Key, Priority)> {
    let rows = grid.rows(items);
    let row = f32::from(grid.row);
    let viewport = heading.viewport;
    if rows == 0 || row <= 0.0 || viewport <= 0.0 {
        return Vec::new();
    }
    let top = heading.offset - f32::from(grid.margin);
    let bottom = top + viewport;
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

    let row_of = |y: f32| ((y.max(0.0) / row).floor() as usize).min(rows - 1);
    let distance = |r: usize, top: f32, bottom: f32| {
        let (rt, rb) = (r as f32 * row, (r + 1) as f32 * row);
        (rt - bottom).max(top - rb).max(0.0)
    };
    // Last frame's rows, which a listing that has shrunk since may no
    // longer have.
    let visible = heading.visible.start.min(rows)..heading.visible.end.min(rows);
    let mut ranked: Vec<(Key, Priority)> = Vec::new();
    let mut push = |r: usize, p: Priority| {
        for key in keys_of_row(r) {
            ranked.push((key, p));
        }
    };
    for r in row_of(from)..=row_of(to) {
        if !visible.contains(&r) {
            push(r, Priority::Ahead(distance(r, top, bottom)));
        }
    }
    if let Some(y) = rest {
        for r in row_of(y - row)..=row_of(y + viewport + row) {
            push(r, Priority::Landing(distance(r, y, y + viewport)));
        }
    }
    for r in visible {
        push(r, Priority::Visible);
    }
    ranked
}

/// What the album grid's benchmark drives it with.
impl AlbumGrid {
    /// The listing is in, and every sleeve on screen is drawn or known to
    /// have none.
    pub fn ready_on_screen(&self, cx: &App) -> bool {
        let Some(grid) = self.grid else { return false };
        if self.loaded.is_none() {
            return false;
        }
        let visible = self.heading.borrow().visible.clone();
        if visible.is_empty() {
            return false;
        }
        let covers = self.lib.covers.read(cx);
        let scale = self.scale.get();
        visible
            .flat_map(|row| row * grid.columns..((row + 1) * grid.columns).min(self.records.len()))
            .all(|i| {
                let key = Key::new(Art::Album(self.records[i].id), grid.side, scale);
                covers.has(key) || covers.is_absent(key.art)
            })
    }

    /// Jumps to the top, or to the end.
    pub fn jump(&mut self, to_end: bool, cx: &mut Context<Self>) {
        let handle = self.scroll.0.borrow().base_handle.clone();
        let y = if to_end {
            handle.max_offset().y
        } else {
            px(0.0)
        };
        handle.set_offset(point(px(0.0), -y));
        cx.notify();
    }

    /// Scrolls on by `distance`, back to the top past the end.
    pub fn scroll_by(&mut self, distance: Pixels, cx: &mut Context<Self>) {
        let handle = self.scroll.0.borrow().base_handle.clone();
        let max = handle.max_offset().y;
        let mut y = -handle.offset().y + distance;
        if y > max {
            y = px(0.0);
        }
        handle.set_offset(point(px(0.0), -y));
        cx.notify();
    }
}

impl chrome::Picking for AlbumGrid {
    fn selection_mut(&mut self) -> &mut Selection {
        &mut self.selection
    }
}

impl actions::Menus for AlbumGrid {
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

impl TileHost for AlbumGrid {
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
            .take(item, modifiers.shift, modifiers.secondary(cx), &self.order)
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
        let p = &Paint::of(window, cx);
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
        if let Some(width) = self.width.get() {
            let grid = Grid::for_width(width, &p.look.scale);
            if Some(grid) != self.grid {
                self.relayout(grid);
            }
        }
        let count = self.records.len();
        if let Some(grid) = self.grid {
            self.frame(grid, count, window, cx);
        }
        let narrowed = self.lib.browse.read(cx).is_narrowed();
        let header = chrome::page_header(
            p,
            "albums",
            chrome::count(count as u64, "album", "albums"),
            div()
                .flex()
                .items_center()
                .gap(p.space(Space::L))
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
                if width.get() != Some(bounds.size.width) {
                    width.set(Some(bounds.size.width));
                    cx.notify(entity);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let body = match self.grid {
            _ if count == 0 && self.loaded.is_some() => chrome::empty_state(
                crate::theme::Icon::Album,
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
            .into_any_element(),
            Some(grid) => uniform_list(
                "albums",
                grid.rows(count),
                // Builds rows only: uniform_list calls it to measure a row as
                // well as to lay out the visible ones, so anything done once
                // a frame happens in `render`.
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    range.map(|row| this.row(row, grid, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .pt(grid.margin)
            .pb(grid.margin)
            .size_full()
            .into_any_element(),
            None => div().into_any_element(),
        };

        div()
            .key_context("Library")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::dismiss))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .typeset(p, Type::Body, p.ink)
            .child(header)
            .child(actions::list_menu(
                div()
                    .id("album-grid")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .on_scroll(cx.listener(|this, e: &ScrollEvent, _, _| this.scrolled(e)))
                    .child(measure)
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui_kit::*` brings GPUI's own `test` attribute in with it.
    use crate::theme::tokens::Scale;
    use core::prelude::v1::test;

    fn grid() -> Grid {
        Grid::for_width(px(800.0), &Scale::of(100))
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

    fn at(g: &Grid, row: f32) -> f32 {
        row * f32::from(g.row) + f32::from(g.margin)
    }

    #[test]
    fn at_rest_three_rows_either_way_and_the_screen_first() {
        let g = grid();
        let row = f32::from(g.row);
        let heading = Heading {
            offset: at(&g, 10.0),
            visible: 10..13,
            viewport: 3.0 * row,
            ..Heading::default()
        };
        let ranked = rank(&heading, &g, 1000 * g.columns, keys);
        assert_eq!(priority(&ranked, 11), Some(Priority::Visible));
        assert_eq!(priority(&ranked, 7), Some(Priority::Ahead(2.0 * row)));
        assert!(matches!(priority(&ranked, 15), Some(Priority::Ahead(_))));
        assert_eq!(priority(&ranked, 5), None);
        assert_eq!(priority(&ranked, 17), None);
    }

    #[test]
    fn a_fast_scroll_reaches_further_ahead_than_behind_and_a_flick_ranks_its_landing() {
        let g = grid();
        let row = f32::from(g.row);
        let heading = Heading {
            offset: at(&g, 10.0),
            velocity: 4000.0,
            decay: Some(2.0),
            visible: 10..13,
            viewport: 3.0 * row,
            ..Heading::default()
        };
        let ranked = rank(&heading, &g, 1000 * g.columns, keys);
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
    fn rows_a_shrunk_listing_no_longer_has_are_not_ranked() {
        let g = grid();
        let row = f32::from(g.row);
        let heading = Heading {
            offset: at(&g, 40.0),
            visible: 40..43,
            viewport: 3.0 * row,
            ..Heading::default()
        };
        let items = 5 * g.columns;
        let ranked = rank(&heading, &g, items, keys);
        assert!(
            ranked
                .iter()
                .all(|(k, _)| matches!(k.art, Art::Album(r) if (r as usize) < 5))
        );
    }

    #[test]
    fn an_empty_grid_ranks_nothing() {
        let heading = Heading {
            viewport: 600.0,
            ..Heading::default()
        };
        assert!(rank(&heading, &grid(), 0, keys).is_empty());
    }
}
