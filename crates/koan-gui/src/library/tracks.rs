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
use std::time::Duration;

use koan_ffi::{BrowseFilter, Track, TrackBrowseSort};
use koan_kit::app::*;
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::{point, px};
// virtualised lists and their scroll state, which GPUI owns and koan-kit's rows sit in.
use koan_kit::raw::{UniformListScrollHandle, uniform_list};

use super::covers::{Art, Key, Priority};
use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::rows::{self, RowPart, RowState, TrackList};
use super::selection::Selection;
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome};

const PAGE: u32 = 1000;
const TYPING: Duration = Duration::from_millis(80);

#[derive(Clone, PartialEq)]
struct Query {
    search: Option<String>,
    filter: BrowseFilter,
    sort: TrackBrowseSort,
}

pub struct TrackBrowser {
    lib: Lib,
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
                let tracks: HashSet<i64> = gone.tracks.iter().copied().collect();
                this.selection
                    .retain(|p| !matches!(p, Playable::Track(id) if tracks.contains(&id)));
                cx.notify();
            }),
            cx.on_release(move |this, cx| {
                let offset = -this.scroll.0.borrow().base_handle.offset().y;
                this.lib
                    .nav
                    .update(cx, |nav, _| nav.remember_offset(Page::Tracks, offset));
                this.lib.covers.update(cx, |c, cx| c.release(id, cx));
            }),
        ];
        let rewinds = lib.nav.read(cx).rewinds(Page::Tracks, cx);
        let restore = lib.nav.read(cx).offset(Page::Tracks);
        let search =
            cx.new(|cx| chrome::SearchField::new(lib.browse.clone(), "filter tracks", window, cx));
        let filters = cx.new(|cx| chrome::FilterPanel::new(lib.clone(), false, window, cx));
        let mut this = Self {
            lib,
            tracks: Vec::new(),
            total: 0,
            loaded: None,
            loading: None,
            stale: false,
            request: 0,
            _load: None,
            scroll: UniformListScrollHandle::new(),
            visible: 0..0,
            selection: Selection::default(),
            pick: None,
            menu: None,
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
        let typed = self
            .loading
            .as_ref()
            .or(self.loaded.as_ref())
            .is_some_and(|(q, v)| {
                *v == version
                    && q.search != wanted.0.search
                    && q.filter == wanted.0.filter
                    && q.sort == wanted.0.sort
            });
        self.request += 1;
        let request = self.request;
        self.loading = Some(wanted.clone());
        let engine = self.lib.engine.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            if typed {
                cx.background_executor().timer(TYPING).await;
            }
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
                                    this.tracks = pending.take().unwrap_or_default();
                                    this.total = page.total;
                                    this.loaded = Some(wanted.clone());
                                    if let Some(y) = this.restore.take() {
                                        this.scroll
                                            .0
                                            .borrow()
                                            .base_handle
                                            .set_offset(point(px(0.0), -y));
                                    }
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
                    Key::new(Art::Album(album), px(rows::SLEEVE), scale),
                    priority,
                ))
            })
            .collect();
        let owner = cx.entity_id();
        self.lib
            .covers
            .update(cx, |covers, _| covers.request(owner, ranked));
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
            let key = Key::new(Art::Album(album), px(rows::SLEEVE), scale);
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
            kind: crate::components::RowKind::Art,
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

impl TrackBrowser {
    /// The browser's controls, in the window's toolbar.
    pub fn controls(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        crate::components::ToolbarGroup::new()
            .child(self.search.clone())
            .spacer()
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
        let rewinds = self.lib.nav.read(cx).rewinds(Page::Tracks, cx);
        if rewinds != self.rewinds {
            self.rewinds = rewinds;
            self.scroll
                .0
                .borrow()
                .base_handle
                .set_offset(Point::default());
        }
        let narrowed = self.lib.browse.read(cx).is_narrowed();
        let header = chrome::page_header(
            p,
            "tracks",
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
        let body = if self.total == 0 && self.loaded.is_some() {
            chrome::empty_state(
                crate::theme::Icon::Track,
                if narrowed {
                    "nothing matches"
                } else {
                    "no tracks yet"
                },
                if narrowed {
                    "try a different filter."
                } else {
                    "add a music folder in settings, or sign in to a server."
                },
            )
            .into_any_element()
        } else {
            // Rows past what has arrived are drawn empty, so the scroll bar
            // is the listing's length from the first page.
            uniform_list(
                "tracks",
                self.total as usize,
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
            .typeset(p, Type::Body, p.ink)
            .child(header)
            .child(actions::list_menu(
                div()
                    .id("track-list")
                    .flex_1()
                    .min_h_0()
                    .pt(px(10.0))
                    .relative()
                    .child(body)
                    // GPUI's scrollbar over a virtualised list, which koan-kit's ScrollArea cannot wrap.
                    .child(koan_kit::raw::base::Scrollbar::vertical(&self.scroll)),
                cx.weak_entity(),
                window,
                cx,
            ))
    }
}
