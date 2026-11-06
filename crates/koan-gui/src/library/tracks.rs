//! The track browser: every track in the library, narrowed and ordered by the
//! database, read a page at a time and drawn a row at a time.
//!
//! Pages arrive one after another off the main thread and are appended as
//! they land, as the Mac's do. A listing asked for again (a new query, a
//! library change) is read into a fresh list and swapped in once it covers
//! the rows on screen, so the old one stays up meanwhile and nothing is
//! drawn from two listings at once.

use std::collections::HashSet;
use std::ops::Range;

use koan_kit::app::*;
use koan_runtime::{BrowseFilter, Track, TrackBrowseSort};
// virtualised lists and their scroll state, which GPUI owns and koan-kit's rows sit in.
use crate::components::{UniformListScrollHandle, uniform_list};

use super::covers::{Art, Key, Priority};
use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::rows::{self, Reveal, RowPart, RowState, TrackList};
use super::selection::Selection;
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome};
use crate::navigator::{Listing, Shelf};

const PAGE: u32 = 1000;

#[derive(Clone, PartialEq)]
struct Query {
    search: Option<String>,
    filter: BrowseFilter,
    sort: TrackBrowseSort,
}

/// The browser's listing as it last stood, whole, for the next visit
/// (`Lib::kept`).
struct KeptTracks {
    tracks: Vec<Track>,
    total: u64,
    loaded: (Query, u64),
}

pub struct TrackBrowser {
    /// A load failed before any landed, and why: the page draws its header
    /// and the failure with a retry rather than staying held, so the query
    /// can still be changed. Cleared by the next listing that lands.
    failed: Option<String>,
    lib: Lib,
    /// A shelf's "see all": the shelf's filter and order in place of the
    /// shared ones, and no controls to change them.
    scope: Option<Shelf>,
    /// The route the browser is on, for its place in the navigator.
    page: Page,
    tracks: Vec<Track>,
    total: u64,
    loaded: Option<(Query, u64)>,
    /// What the read in flight answers to, until its last page lands.
    loading: Option<(Query, u64)>,
    /// The library moved while a read was in flight: read again when it ends.
    stale: bool,
    request: u64,
    _load: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    /// The header's height on a phone, where the list scrolls it away.
    header_height: std::rc::Rc<std::cell::Cell<Option<Pixels>>>,
    /// The keyboard's place in the list.
    cursor: crate::components::ListCursor,
    /// Where the cursor was laid out, for the menu key and the scroll.
    spot: crate::components::CursorSpot,
    /// The list holds the keyboard, as of the last frame.
    keyed: bool,
    /// The rows on screen as of the last frame.
    visible: Range<usize>,
    selection: Selection,
    pick: Option<(u64, PlayableDrag)>,
    /// What the list's context menu acts on: the row last right-clicked.
    menu: Option<actions::MenuTarget>,
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

impl TrackBrowser {
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

    /// The browser narrowed to `scope`'s shelf, or the library's own.
    pub fn scoped(
        lib: Lib,
        scope: Option<Shelf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let page = scope.map_or(Page::Tracks, |shelf| Page::Shelved(shelf, Listing::Tracks));
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
                let tracks: HashSet<i64> = gone.tracks.iter().copied().collect();
                this.selection
                    .retain(|p| !matches!(p, Playable::Track(id) if tracks.contains(&id)));
                cx.notify();
            }),
            cx.on_release(move |this, cx| {
                // Only a whole listing: one still being read is read afresh.
                if this.scope.is_none()
                    && this.tracks.len() as u64 == this.total
                    && let Some(loaded) = this.loaded.take()
                {
                    this.lib.kept.put(KeptTracks {
                        tracks: std::mem::take(&mut this.tracks),
                        total: this.total,
                        loaded,
                    });
                }
                let offset = -this.scroll.0.borrow().base_handle.offset().y;
                this.lib
                    .nav
                    .update(cx, |nav, _| nav.remember_offset(page, offset));
                this.lib.covers.update(cx, |c, cx| c.release(id, cx));
            }),
        ];
        let rewinds = lib.nav.read(cx).rewinds(page, cx);
        let restore = lib.nav.read(cx).offset(page);
        let search =
            cx.new(|cx| chrome::SearchField::new(lib.browse.clone(), "filter tracks", window, cx));
        let filters = cx.new(|cx| chrome::FilterPanel::new(lib.clone(), false, window, cx));
        let (tracks, total, loaded) = match scope
            .is_none()
            .then(|| lib.kept.take::<KeptTracks>())
            .flatten()
        {
            Some(k) => (k.tracks, k.total, Some(k.loaded)),
            None => (Vec::new(), 0, None),
        };
        let mut this = Self {
            lib,
            scope,
            page,
            tracks,
            total,
            loaded,
            failed: None,
            loading: None,
            stale: false,
            request: 0,
            _load: None,
            scroll: UniformListScrollHandle::new(),
            header_height: Default::default(),
            cursor: Default::default(),
            spot: Default::default(),
            keyed: false,
            visible: 0..0,
            selection: Selection::default(),
            pick: None,
            menu: None,
            restore,
            rewinds,
            focus: cx.focus_handle().tab_stop(true),
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
                this.tracks = Default::default();
                this.cursor = Default::default();
                this.total = 0;
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
        if let Some(shelf) = self.scope {
            return Query {
                search: super::scope::search(shelf, &self.lib, cx),
                filter: super::scope::filter(shelf),
                sort: super::scope::track_sort(shelf),
            };
        }
        let browse = self.lib.browse.read(cx);
        Query {
            search: browse.search(),
            filter: browse.filter.clone(),
            sort: browse.effective_track_sort(),
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(version) = self.lib.feed.read(cx).library_version else {
            return;
        };
        let wanted = (self.query(cx), version);
        if self.loaded.as_ref() == Some(&wanted) || self.loading.as_ref() == Some(&wanted) {
            return;
        }
        // A read is not restarted for the library alone: a scan moves it with
        // every batch, and a read far down the list restarted each time would
        // never land. It is read again once, when the one in flight ends.
        if self.loading.as_ref().is_some_and(|(q, _)| *q == wanted.0) {
            self.stale = true;
            return;
        }
        self.stale = false;
        self.request += 1;
        let request = self.request;
        self.loading = Some(wanted.clone());
        let engine = self.lib.engine.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            let q = wanted.0.clone();
            // Read until the listing covers the rows on screen, then swapped
            // in whole; after that each page is appended as it lands.
            let mut pending: Option<Vec<Track>> = Some(Vec::new());
            let mut read = 0u32;
            loop {
                let page = engine
                    .clone()
                    .track_listing(q.sort, q.search.clone(), q.filter.clone(), PAGE, read)
                    .await;
                let page = match page {
                    Ok(page) => page,
                    Err(e) => {
                        log::warn!("tracks: {e}");
                        let swapped = pending.is_none();
                        this.update(cx, |this, cx| {
                            if this.request != request {
                                return;
                            }
                            this.loading = None;
                            this.failed = Some(crate::settings::model::describe(&e));
                            cx.notify();
                            // Swapped in but not whole: not the listing, so
                            // the next change reads it again.
                            if swapped {
                                this.loaded = None;
                            }
                            if std::mem::take(&mut this.stale) {
                                this.reload(cx);
                            }
                        })
                        .ok();
                        return;
                    }
                };
                let arrived = page.tracks.len() as u32;
                read += arrived;
                let done = arrived == 0 || u64::from(read) >= page.total;
                let carry_on = this
                    .update(cx, |this, cx| {
                        if this.request != request {
                            return false;
                        }
                        match pending.as_mut() {
                            Some(list) => {
                                list.extend(page.tracks);
                                let covers =
                                    list.len() >= this.visible.end.min(page.total as usize);
                                if covers || done {
                                    let tracks = pending.take().unwrap_or_default();
                                    let ids = |t: &[Track]| -> Vec<i64> {
                                        t.iter().map(|t| t.id).collect()
                                    };
                                    this.cursor.relocate(&ids(&this.tracks), &ids(&tracks));
                                    this.tracks = tracks;
                                    this.total = page.total;
                                    this.loaded = Some(wanted.clone());
                                    this.failed = None;
                                    this.restore_scroll();
                                    cx.notify();
                                }
                            }
                            None => {
                                this.tracks.extend(page.tracks);
                                this.total = page.total;
                                cx.notify();
                            }
                        }
                        if done {
                            this.loading = None;
                            if std::mem::take(&mut this.stale) {
                                this.reload(cx);
                            }
                        }
                        true
                    })
                    .unwrap_or(false);
                if !carry_on || done {
                    return;
                }
            }
        }));
    }

    fn pick(&mut self) -> Option<PlayableDrag> {
        if self.selection.picked().is_empty() {
            return None;
        }
        let revision = self.selection.revision();
        if self.pick.as_ref().is_none_or(|(r, _)| *r != revision) {
            let picked = self.selection.picked();
            let label = chrome::count(picked.len() as u64, "track", "tracks");
            self.pick = Some((revision, PlayableDrag::new(picked, label)));
        }
        self.pick.as_ref().map(|(_, drag)| drag.clone())
    }

    /// The covers on screen first, then a screen's worth either side.
    fn aim(&mut self, visible: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = visible.clone();
        let scale = window.scale_factor();
        let reach = visible.len().max(10);
        let from = visible.start.saturating_sub(reach);
        let to = (visible.end + reach).min(self.tracks.len());
        let ranked: Vec<(Key, Priority)> = (from..to)
            .filter_map(|i| {
                let album = self.tracks.get(i)?.album_id?;
                let priority = if visible.contains(&i) {
                    Priority::Visible
                } else {
                    Priority::Ahead(i.abs_diff(visible.start).min(i.abs_diff(visible.end)) as f32)
                };
                Some((
                    Key::new(Art::Album(album), Pixels::from(rows::SLEEVE), scale),
                    priority,
                ))
            })
            .collect();
        let owner = cx.entity_id();
        self.lib
            .covers
            .update(cx, |covers, cx| covers.request(owner, ranked, cx));
    }

    fn row(
        &mut self,
        p: &Paint,
        index: usize,
        scale: f32,
        lead_width: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((id, album, title)) = self
            .tracks
            .get(index)
            .map(|t| (t.id, t.album_id, t.title.clone()))
        else {
            return div().h(rows::art_height(p)).into_any_element();
        };
        let item = Playable::Track(id);
        let selected = self.selection.contains(item);
        let drag = match (selected, self.pick()) {
            (true, Some(pick)) => pick,
            _ => PlayableDrag::new(&[item], title).with_sleeve(album),
        };
        let sleeve = album.map(|album| {
            let key = Key::new(Art::Album(album), Pixels::from(rows::SLEEVE), scale);
            self.lib.covers.update(cx, |c, _| c.sleeve(key))
        });
        let feed = self.lib.feed.read(cx);
        let state = RowState {
            lead: (index + 1).to_string().into(),
            lead_width,
            current: feed.playing_track == Some(id),
            favourite: feed.is_favourite(item),
            selected,
            gathered: true,
            transfer: feed.transfers.get(&id).copied(),
            drag,
            note: None,
            cursor: (self.keyed && self.cursor.at == Some(index)).then(|| self.spot.clone()),
            kind: crate::components::RowKind::Art,
            place: Some((index, self.total as usize)),
            reveal: p
                .a11y
                .then(|| Reveal::Uniform(self.scroll.clone(), index, self.total as usize)),
        };
        rows::track_row(p, &self.tracks[index], index, state, sleeve, None, cx)
    }

    fn order(&self) -> Vec<Playable> {
        self.tracks.iter().map(|t| Playable::Track(t.id)).collect()
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

impl actions::Menus for TrackBrowser {
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

impl TrackList for TrackBrowser {
    fn row_tracks(&self) -> Vec<i64> {
        self.tracks.iter().map(|t| t.id).collect()
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
                // The keyboard carries on from the row clicked.
                self.cursor.click(index, event.modifiers().shift);
                let tracks = &self.tracks;
                let order = || tracks.iter().map(|t| Playable::Track(t.id)).collect();
                if rows::select(
                    &mut self.selection,
                    Playable::Track(track),
                    event,
                    order,
                    cx,
                ) {
                    let ids = self.tracks.iter().map(|t| t.id).collect();
                    actions::play_list(&self.lib, ids, index, cx);
                }
                cx.notify();
            }
            part => part.follow(track, &self.lib, cx),
        }
    }
}

impl TrackBrowser {
    /// Whether tracks are being picked: what the toolbar's select shows.
    pub fn picking(&self) -> bool {
        self.selection.is_active()
    }

    /// The browser's controls, in the window's toolbar.
    pub fn controls(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        chrome::browser_controls(&self.search)
            .child(chrome::filter_button(&self.filters, &self.lib, cx))
            .child(chrome::track_sort_button(&self.lib, cx))
            .into_any_element()
    }
}

impl Render for TrackBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(super::covers::UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }
        let rewinds = self.lib.nav.read(cx).rewinds(self.page, cx);
        if rewinds != self.rewinds {
            self.rewinds = rewinds;
            self.scroll
                .0
                .borrow()
                .base_handle
                .set_offset(Point::default());
        }
        let narrowed = self.lib.browse.read(cx).is_narrowed();
        // A shelf's "see all" has no filter of its own: it says what the
        // shelf holds, not what the library browser's filter left.
        let (empty_title, empty_detail) =
            super::scope::empty(self.scope, narrowed, "no tracks yet");
        let header = chrome::page_header(
            p,
            self.scope.map_or("tracks", |shelf| {
                super::scope::title(shelf, Listing::Tracks)
            }),
            chrome::count(self.total, "track", "tracks"),
            div(),
        );
        let scale = window.scale_factor();
        let lead_width = rows::lead_width(p, self.total as usize, window);
        // The rows on screen from the scroll's offset, rows being one height.
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
        if self.total > 0 {
            if viewport != self.viewport {
                self.viewport = viewport;
                self.waits = 0;
                window.request_animation_frame();
            } else if viewport <= 0.0 && self.waits < 2 {
                self.waits += 1;
                window.request_animation_frame();
            }
        }
        let row = f32::from(rows::art_height(p)).max(1.0);
        let last = (((offset + viewport) / row).ceil() as usize).min(self.total as usize);
        let first = ((offset / row).floor() as usize).min(last);
        self.aim(first..last, window, cx);
        // Until its first listing lands the page draws nothing, then header,
        // total and rows together.
        let held = self.loaded.is_none() && self.failed.is_none();
        // A phone's list scrolls its header away with the rows, as the iOS
        // app's does.
        let phone = crate::mobile::phone() && !held;
        // The header starts below the phone's bar, which the list scrolls
        // under.
        let above = if phone {
            crate::components::bar_inset(cx) + p.look.spaced(10.0)
        } else {
            Pixels::ZERO
        };
        let lead = if phone {
            self.header_height.get().unwrap_or_default() + above
        } else {
            Pixels::ZERO
        };
        let scrolled = self.scroll.0.borrow().base_handle.offset().y;
        let entity = cx.entity_id();
        let (inline_header, away_header) = if phone {
            (None, Some(chrome::with_filter(&self.search, p)))
        } else {
            ((!held).then_some(header), None)
        };
        let body = if held {
            div().into_any_element()
        } else if self.loaded.is_none() {
            chrome::failed_state(
                crate::theme::Icon::Track,
                self.failed.as_deref().unwrap_or_default(),
                cx.listener(|this, _, _, cx| this.retry(cx)),
            )
            .into_any_element()
        } else if self.total == 0 {
            chrome::empty_state(crate::theme::Icon::Track, empty_title, empty_detail)
                .into_any_element()
        } else {
            let under_foot = crate::components::rows_under_foot(rows::art_height(p), cx);
            // Rows past what has arrived are drawn empty, so the scroll bar
            // is the listing's length from the first page.
            uniform_list(
                "tracks",
                // Blank rows after the last pass under a phone's foot.
                self.total as usize + under_foot.0,
                // Builds rows only: uniform_list also calls it to measure a
                // row, so the cover requests happen once a frame in `render`.
                cx.processor(move |this, range: Range<usize>, window, cx| {
                    let p = Paint::of(window, cx);
                    range
                        .map(|i| this.row(&p, i, scale, lead_width, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .pt(lead)
            .size_full()
            .map(|list| {
                crate::components::spoken_list("tracks-list", "tracks")
                    // Reaching below the page so its blank rows come to the
                    // foot's height (`rows_under_foot`).
                    .map(|d| {
                        if under_foot.1 > Pixels::ZERO {
                            d.absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .bottom(-under_foot.1)
                        } else {
                            d.size_full()
                        }
                    })
                    .child(list)
            })
            .into_any_element()
        };
        self.keyed = self.focus.is_focused(window);
        let page = actions::list_menu(
            super::keys::keyed(
                div().key_context("Library List").track_focus(&self.focus),
                cx,
            ),
            cx.weak_entity(),
            actions::Menus::cursor_spot(self),
            window,
            cx,
        );
        page.on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::dismiss))
            .size_full()
            .flex()
            .flex_col()
            .typeset(p, Type::Body, p.ink)
            .children(inline_header)
            .child(
                div()
                    .id("track-list")
                    .overflow_hidden()
                    .flex_1()
                    .min_h_0()
                    .when(!phone, |d| d.pt(p.look.spaced(10.0)))
                    .relative()
                    .child(body)
                    // GPUI's scrollbar over a virtualised list, which koan-kit's ScrollArea cannot wrap.
                    .child(
                        crate::components::Scrollbar::vertical(&self.scroll)
                            .when(phone, crate::components::Scrollbar::under_bar),
                    )
                    .children(away_header.map(|header| {
                        crate::components::scrolls_away(
                            header,
                            scrolled + above,
                            self.header_height.clone(),
                            entity,
                        )
                    })),
            )
    }
}

impl Focusable for TrackBrowser {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl super::keys::KeyedList for TrackBrowser {
    fn cursor(&mut self) -> &mut crate::components::ListCursor {
        &mut self.cursor
    }

    fn spot(&self) -> Option<&crate::components::CursorSpot> {
        Some(&self.spot)
    }

    fn len(&self) -> usize {
        self.tracks.len()
    }

    fn pick(&mut self, from: usize, to: usize, _: &mut Context<Self>) {
        let order = self.order();
        self.selection.only(order[from]);
        if to != from {
            self.selection.click(order[to], true, &order);
        }
    }

    fn toggle(&mut self, ix: usize, _: &mut Context<Self>) {
        let order = self.order();
        self.selection.click(order[ix], false, &order);
    }

    fn reveal(&mut self, ix: usize, _: &mut Context<Self>) {
        self.scroll
            .scroll_to_item(ix, crate::components::ScrollStrategy::Nearest);
    }

    fn activate(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        let ids = self.tracks.iter().map(|t| t.id).collect();
        actions::play_list(&self.lib, ids, ix, cx);
    }

    fn menu_at(&mut self, ix: Option<usize>, _: &mut Context<Self>) {
        self.menu = ix.map(|ix| {
            let track = &self.tracks[ix];
            actions::keyed_target(
                &self.selection,
                Playable::Track(track.id),
                track.album_id,
                None,
            )
        });
    }
}
