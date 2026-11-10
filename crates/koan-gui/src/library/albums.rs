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

use crate::components::{UniformListScrollHandle, uniform_list};
use koan_kit::app::platform::TouchPhase;
use koan_kit::app::*;
use koan_runtime::{AlbumSort, BrowseFilter};

use super::covers::{Art, Covers, Key, Priority, Sleeve, UPLOAD_BYTES_PER_FRAME};
use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::selection::Selection;
use super::tile::{Grid, Part, Record, TileHost, TileState, Tiles};
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome};
use crate::navigator::{Listing, Shelf};

#[derive(Clone, PartialEq)]
struct Query {
    search: Option<String>,
    filter: BrowseFilter,
    sort: AlbumSort,
    seed: i64,
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

/// The grid's listing as it last stood, for the next visit (`Lib::kept`).
struct KeptAlbums {
    records: Rc<Vec<Rc<Record>>>,
    order: Vec<Playable>,
    loaded: (Query, u64),
}

pub struct AlbumGrid {
    /// A load failed before any landed, and why: the page draws its header
    /// and the failure with a retry rather than staying held, so the query
    /// can still be changed. Cleared by the next listing that lands.
    failed: Option<String>,
    lib: Lib,
    /// A shelf's "see all": the shelf's filter and order in place of the
    /// shared ones, and no controls to change them.
    scope: Option<Shelf>,
    /// The route the grid is on, for its place in the navigator.
    page: Page,
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
    /// The header's height on a phone, where the grid scrolls it away.
    header_height: Rc<Cell<Option<Pixels>>>,
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
    /// The keyboard's place in the grid, by record.
    cursor: crate::components::ListCursor,
    /// Where the cursor was laid out, for the menu key and the scroll.
    spot: crate::components::CursorSpot,
    /// The grid holds the keyboard, as of the last frame.
    keyed: bool,
    search: Entity<chrome::SearchField>,
    filters: Entity<chrome::FilterPanel>,
    /// The list's height as of the last frame, to tell when it moved.
    viewport: f32,
    /// Frames asked for while the list has no height.
    waits: u8,
    _subscriptions: Vec<Subscription>,
}

impl AlbumGrid {
    /// Back to where the last visit left the list, once.
    fn restore_scroll(&mut self) {
        if let Some(y) = self.restore.take() {
            self.scroll
                .0
                .borrow()
                .base_handle
                .set_offset(crate::components::offset_down(y));
        }
    }

    pub fn new(lib: Lib, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::scoped(lib, None, window, cx)
    }

    /// The grid narrowed to `scope`'s shelf, or the library's own.
    pub fn scoped(
        lib: Lib,
        scope: Option<Shelf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let page = scope.map_or(Page::Albums, |shelf| Page::Shelved(shelf, Listing::Albums));
        let id = cx.entity_id();
        let subscriptions = vec![
            // Cached under the window, and draws the accent.
            crate::theme::observe_accent(cx),
            cx.observe(&lib.browse, |this, _, cx| this.reload(cx)),
            // A search's own listing follows its query.
            cx.observe(&lib.search, |this, _, cx| this.reload(cx)),
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
                if let Some(loaded) = this.loaded.take().filter(|_| this.scope.is_none()) {
                    this.lib.kept.put(KeptAlbums {
                        records: this.records.clone(),
                        order: std::mem::take(&mut this.order),
                        loaded,
                    });
                }
                let offset = Pixels::from(this.heading.borrow().offset);
                this.lib
                    .nav
                    .update(cx, |nav, _| nav.remember_offset(page, offset));
                this.lib.covers.update(cx, |c, cx| c.release(id, cx));
            }),
        ];
        let rewinds = lib.nav.read(cx).rewinds(page, cx);
        let restore = lib.nav.read(cx).offset(page);
        let search =
            cx.new(|cx| chrome::SearchField::new(lib.browse.clone(), "filter albums", window, cx));
        let filters = cx.new(|cx| chrome::FilterPanel::new(lib.clone(), true, window, cx));
        let kept = if scope.is_none() {
            lib.kept.take::<KeptAlbums>()
        } else {
            None
        };
        let (records, order, loaded) = match kept {
            Some(k) => (k.records, k.order, Some(k.loaded)),
            None => (Rc::new(Vec::new()), Vec::new(), None),
        };
        let mut this = Self {
            lib,
            scope,
            page,
            records,
            tiles: Tiles::default(),
            aimed: None,
            order,
            loaded,
            failed: None,
            loading: None,
            request: 0,
            _load: None,
            scroll: UniformListScrollHandle::new(),
            width: Rc::new(Cell::new(None)),
            header_height: Rc::new(Cell::new(None)),
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
            focus: cx.focus_handle().tab_stop(true),
            cursor: Default::default(),
            spot: Default::default(),
            keyed: false,
            search,
            filters,
            viewport: 0.0,
            waits: 0,
            _subscriptions: subscriptions,
        };
        // A kept listing draws at once, where the last visit left it. One
        // from another query, or from before the library moved, is not drawn:
        // the page waits for its own, as a first visit does, rather than
        // drawing old rows and landing the kept place on other records. The
        // place is kept for a listing of the same query.
        let version = this.lib.feed.read(cx).library_version;
        let query = this.query(cx);
        match this
            .loaded
            .as_ref()
            .map(|loaded| chrome::kept(loaded, &query, version))
        {
            Some(chrome::Kept::Draw) => this.restore_scroll(),
            Some(kept) => {
                if kept == chrome::Kept::Forget {
                    this.restore = None;
                }
                this.records = Rc::new(Vec::new());
                this.order.clear();
                this.loaded = None;
            }
            None => {}
        }
        this.reload(cx);
        this
    }

    /// The failure's "try again": the same listing asked for afresh.
    fn retry(&mut self, cx: &mut Context<Self>) {
        self.failed = None;
        self.loading = None;
        self.reload(cx);
        cx.notify();
    }

    fn query(&self, cx: &App) -> Query {
        let browse = self.lib.browse.read(cx);
        if let Some(shelf) = self.scope {
            return Query {
                search: super::scope::search(shelf, &self.lib, cx),
                filter: super::scope::filter(shelf),
                sort: super::scope::album_sort(shelf),
                seed: browse.seed,
            };
        }
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
    /// keystroke asks at once; the answer to one typed over is dropped by
    /// `request` when it lands.
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
        self.request += 1;
        let request = self.request;
        self.loading = Some(wanted.clone());
        let engine = self.lib.engine.clone();
        let q = wanted.0.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
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
                        this.failed = None;
                        this.restore_scroll();
                    }
                    Err(e) => {
                        log::warn!("albums: {e}");
                        this.failed = Some(crate::settings::model::describe(&e));
                    }
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
            .update(cx, |covers, cx| covers.request(owner, ranked, cx));
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
        let height = grid.tile_height();
        let mut tiles = Vec::with_capacity(end.saturating_sub(start));
        let count = records.len();
        for (ix, record) in records
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
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
                    Sleeve::Ready(image, _) => Some(image.id),
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
                cursor: (self.keyed && self.cursor.at == Some(start + ix))
                    .then(|| self.spot.clone()),
                show_artist: true,
                side: grid.side,
                sleeve,
                drag,
                pick,
                after,
                place: (start + ix, count),
            };
            tiles.push(self.tiles.tile(record, state, height, cx));
        }
        div()
            .flex()
            .gap(grid.gap)
            .px(grid.inset)
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
        // Only the room between tiles moved: every row is where it was.
        if let Some(old) = self.grid
            && !old.same_rows(&grid)
        {
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
                .set_offset(crate::components::offset_down(Pixels::from(y)));
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

    /// How many records the grid lists.
    pub fn listed(&self) -> usize {
        self.records.len()
    }

    /// How many tiles are on screen.
    pub fn on_screen(&self) -> usize {
        let Some(grid) = self.grid else { return 0 };
        let visible = self.heading.borrow().visible.clone();
        visible
            .map(|row| ((row + 1) * grid.columns).min(self.records.len()) - row * grid.columns)
            .sum()
    }

    /// Jumps to the top, or to the end.
    pub fn jump(&mut self, to_end: bool, cx: &mut Context<Self>) {
        let handle = self.scroll.0.borrow().base_handle.clone();
        let y = if to_end {
            handle.max_offset().y
        } else {
            Pixels::ZERO
        };
        handle.set_offset(crate::components::offset_down(y));
        cx.notify();
    }

    /// Scrolls on by `distance`, back to the top past the end.
    pub fn scroll_by(&mut self, distance: Pixels, cx: &mut Context<Self>) {
        let handle = self.scroll.0.borrow().base_handle.clone();
        let max = handle.max_offset().y;
        let mut y = -handle.offset().y + distance;
        if y > max {
            y = Pixels::ZERO;
        }
        handle.set_offset(crate::components::offset_down(y));
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

    fn cursor_spot(&self) -> Option<crate::components::CursorSpot> {
        Some(self.spot.clone())
    }
}

impl TileHost for AlbumGrid {
    fn revealed(&mut self, album: i64, cx: &mut Context<Self>) {
        if let (Some(grid), Some(ix)) = (
            self.grid,
            self.records.iter().position(|record| record.id == album),
        ) {
            let columns = grid.columns.max(1);
            crate::components::reveal_around(
                &self.scroll,
                ix / columns,
                self.records.len().div_ceil(columns),
            );
            // A scroll handle asks for no frame: drawn again so the next row
            // of tiles reaches the accessibility tree on an idle window.
            cx.notify();
        }
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
        // The keyboard carries on from the tile clicked.
        if let Some(index) = self.records.iter().position(|r| r.id == album) {
            self.cursor.click(index, modifiers.shift);
        }
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

impl AlbumGrid {
    /// Whether records are being picked: what the toolbar's select shows.
    pub fn picking(&self) -> bool {
        self.selection.is_active()
    }

    /// The grid's controls, in the window's toolbar.
    pub fn controls(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        chrome::browser_controls(&self.search, window, cx)
            .child(chrome::filter_button(&self.filters, &self.lib, cx))
            .child(chrome::album_sort_button(&self.lib, cx))
            .children(chrome::reshuffle_button(&self.lib, cx))
            .spacer()
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
                cx,
            ))
            .into_any_element()
    }
}

impl Render for AlbumGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        self.scale.set(window.scale_factor());
        let rewinds = self.lib.nav.read(cx).rewinds(self.page, cx);
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
        // A shelf's "see all" has no filter of its own: it says what the
        // shelf holds, not what the library browser's filter left.
        let (empty_title, empty_detail) =
            super::scope::empty(self.scope, narrowed, "no albums yet");
        let header = chrome::page_header(
            p,
            self.scope.map_or("albums", |shelf| {
                super::scope::title(shelf, Listing::Albums)
            }),
            chrome::count(count as u64, "album", "albums"),
            div(),
        );

        let width = self.width.clone();
        let entity = cx.entity_id();
        let measure = crate::components::width_probe(width, entity);

        // Until its first listing lands the page draws nothing, then header,
        // total and tiles together: nothing pops in after the rest.
        let held = self.loaded.is_none() && self.failed.is_none();
        // The grid scrolls its header away with the tiles, as the iOS app's
        // does, and passes under the toolbar: held above them it would keep
        // a fifth of a phone's screen, and cut the tiles off at its edge.
        let away = !held;
        // The header starts below the bar, which the list scrolls under.
        let above = crate::components::bar_inset(window, cx);
        let lead = if away {
            self.header_height.get().unwrap_or_default() + above
        } else {
            Pixels::ZERO
        };
        let scrolled = self.scroll.0.borrow().base_handle.offset().y;
        // A phone's bar names the page: its list leads with the filter field
        // alone.
        let away_header = away.then(|| {
            if p.look.compact {
                chrome::with_filter(&self.search, p)
            } else {
                header.into_any_element()
            }
        });
        let body = match self.grid {
            _ if held => div().into_any_element(),
            _ if self.loaded.is_none() => chrome::failed_state(
                crate::theme::Icon::Album,
                self.failed.as_deref().unwrap_or_default(),
                cx.listener(|this, _, _, cx| this.retry(cx)),
            )
            .into_any_element(),
            _ if count == 0 => {
                chrome::empty_state(crate::theme::Icon::Album, empty_title, empty_detail)
                    .into_any_element()
            }
            Some(grid) => uniform_list(
                "albums",
                // Blank rows after the last pass under a phone's foot.
                grid.rows(count) + crate::components::rows_under_foot(grid.row, window, cx).0,
                // Builds rows only: uniform_list calls it to measure a row as
                // well as to lay out the visible ones, so anything done once
                // a frame happens in `render`.
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    range.map(|row| this.row(row, grid, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .pt(grid.margin + lead)
            .pb(grid.margin)
            .size_full()
            .map(|list| {
                let below = crate::components::rows_under_foot(grid.row, window, cx).1;
                crate::components::spoken_list("albums-list", "albums")
                    // Reaching below the page so its blank rows come to the
                    // foot's height (`rows_under_foot`).
                    .map(|d| {
                        if below > Pixels::ZERO {
                            d.absolute().top_0().left_0().right_0().bottom(-below)
                        } else {
                            d.size_full()
                        }
                    })
                    .child(list)
            })
            .into_any_element(),
            None => div().into_any_element(),
        };

        self.keyed = self.focus.is_focused(window);
        self.cursor.clamp(count);
        actions::list_menu(
            super::keys::keyed(
                div().key_context("Library List").track_focus(&self.focus),
                cx,
            ),
            cx.weak_entity(),
            actions::Menus::cursor_spot(self),
            window,
            cx,
        )
        .on_action(cx.listener(Self::select_all))
        .on_action(cx.listener(Self::dismiss))
        .size_full()
        .flex()
        .flex_col()
        .typeset(p, Type::Body, p.ink)
        .child(
            div()
                .id("album-grid")
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .on_scroll(cx.listener(|this, e: &ScrollEvent, _, _| this.scrolled(e)))
                .child(measure)
                .child(body)
                .children(away_header.map(|header| {
                    crate::components::scrolls_away(
                        header,
                        scrolled + above,
                        self.header_height.clone(),
                        entity,
                    )
                }))
                .child(crate::components::keep_clear(&self.spot, &self.scroll))
                // GPUI's scrollbar over a virtualised list, which koan-kit's ScrollArea cannot wrap.
                .child(
                    crate::components::Scrollbar::vertical(&self.scroll)
                        .when(away, crate::components::Scrollbar::under_bar),
                ),
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

impl super::keys::KeyedList for AlbumGrid {
    fn cursor(&mut self) -> &mut crate::components::ListCursor {
        &mut self.cursor
    }

    fn spot(&self) -> Option<&crate::components::CursorSpot> {
        Some(&self.spot)
    }

    fn len(&self) -> usize {
        self.records.len()
    }

    fn columns(&self) -> usize {
        self.grid.map_or(1, |grid| grid.columns)
    }

    fn page(&self) -> usize {
        self.heading.borrow().visible.len().max(1)
    }

    /// One record is marked, as a plain click on a list row marks it,
    /// without starting a pick: the tiles still play on a click. A range
    /// starts one.
    fn pick(&mut self, from: usize, to: usize, _: &mut Context<Self>) {
        super::keys::mark_range(&mut self.selection, &self.order, from, to);
    }

    fn toggle(&mut self, ix: usize, _: &mut Context<Self>) {
        let item = Playable::Album(self.records[ix].id);
        self.selection.click(item, false, &self.order);
    }

    fn reveal(&mut self, ix: usize, _: &mut Context<Self>) {
        if let Some(grid) = self.grid {
            self.scroll.scroll_to_item(
                ix / grid.columns.max(1),
                crate::components::ScrollStrategy::Nearest,
            );
        }
    }

    /// Return opens the record, as a click on its title does.
    fn activate(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        let album = self.records[ix].id;
        self.lib
            .nav
            .update(cx, |nav, cx| nav.open(Page::Album(album), cx));
    }

    fn menu_at(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        let entity = cx.weak_entity();
        self.menu = ix.map(|ix| {
            actions::keyed_target(
                &self.selection,
                Playable::Album(self.records[ix].id),
                None,
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

impl Focusable for AlbumGrid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::tokens::Scale;
    use core::prelude::v1::test;

    fn grid() -> Grid {
        Grid::for_width(Pixels::from(800.0), &Scale::of(100))
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
        let Some(Priority::Ahead(d)) = priority(&ranked, 7) else {
            panic!("row 7 is ranked ahead");
        };
        assert!((d - 2.0 * row).abs() < 0.01);
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
        // 1200 points (0.3 s at 4000 pt/s) and four rows past the screen's
        // foot, against two rows behind its top.
        let ahead = |r: usize| {
            ranked
                .iter()
                .any(|(k, p)| k.art == Art::Album(r as i64) && matches!(p, Priority::Ahead(_)))
        };
        let reach = ((13.0 * row + 1200.0 + 4.0 * row) / row) as usize;
        assert!(ahead(reach) && !ahead(reach + 1));
        assert!(ahead(8) && !ahead(7));
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
