//! The artist browser: every artist in the library, narrowed by the shared
//! filter and search, as the Mac's artist list draws them: a play mark under
//! the pointer, the name, which opens the artist, the heart, and how many
//! records and tracks.

use std::collections::HashSet;
use std::ops::Range;
use std::time::Duration;

use koan_ffi::{Artist, BrowseFilter};
use koan_kit::app::*;

use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::selection::Selection;
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome, rows};
use crate::components::{ROW_GROUP, RowKind, RowNumber, TableRow};
use crate::navigator::{Listing, Shelf};
use crate::theme::Icon;

const TYPING: Duration = Duration::from_millis(80);

#[derive(Clone, PartialEq)]
struct Query {
    search: Option<String>,
    filter: BrowseFilter,
}

/// The browser's columns, in points at 100 %: the heart, the counts.
const HEART: f32 = 20.0;
const COUNT: f32 = 110.0;

/// The browser's listing as it last stood, for the next visit (`Lib::kept`).
struct KeptArtists {
    artists: Vec<Artist>,
    order: Vec<Playable>,
    loaded: (Query, u64),
}

pub struct ArtistBrowser {
    /// A load failed before any landed: the page draws its header and empty
    /// state rather than staying held, so the query can still be changed.
    failed: bool,
    lib: Lib,
    /// A shelf's "see all": the shelf's filter and order in place of the
    /// shared ones, and no controls to change them.
    scope: Option<Shelf>,
    /// The route the browser is on, for its place in the navigator.
    page: Page,
    artists: Vec<Artist>,
    order: Vec<Playable>,
    loaded: Option<(Query, u64)>,
    loading: Option<(Query, u64)>,
    request: u64,
    _load: Option<Task<()>>,
    scroll: crate::components::UniformListScrollHandle,
    selection: Selection,
    pick: Option<(u64, PlayableDrag)>,
    /// What the list's context menu acts on: the row last right-clicked.
    menu: Option<actions::MenuTarget>,
    starting: HashSet<i64>,
    restore: Option<Pixels>,
    rewinds: u64,
    focus: FocusHandle,
    /// The keyboard's place in the list.
    cursor: crate::components::ListCursor,
    /// The list holds the keyboard, as of the last frame.
    keyed: bool,
    search: Entity<chrome::SearchField>,
    filters: Entity<chrome::FilterPanel>,
    _subscriptions: Vec<Subscription>,
}

impl ArtistBrowser {
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
        let page = scope.map_or(Page::Artists, |shelf| {
            Page::Shelved(shelf, Listing::Artists)
        });
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
            cx.subscribe(&lib.covers, |this, _, gone: &super::covers::Evicted, cx| {
                let artists: HashSet<i64> = gone.artists.iter().copied().collect();
                this.selection
                    .retain(|p| !matches!(p, Playable::Artist(id) if artists.contains(&id)));
                cx.notify();
            }),
            cx.on_release(move |this, cx| {
                if let Some(loaded) = this.loaded.take().filter(|_| this.scope.is_none()) {
                    this.lib.kept.put(KeptArtists {
                        artists: std::mem::take(&mut this.artists),
                        order: std::mem::take(&mut this.order),
                        loaded,
                    });
                }
                let offset = -this.scroll.0.borrow().base_handle.offset().y;
                this.lib
                    .nav
                    .update(cx, |nav, _| nav.remember_offset(page, offset));
            }),
        ];
        let rewinds = lib.nav.read(cx).rewinds(page, cx);
        let restore = lib.nav.read(cx).offset(page);
        let search =
            cx.new(|cx| chrome::SearchField::new(lib.browse.clone(), "filter artists", window, cx));
        let filters = cx.new(|cx| chrome::FilterPanel::new(lib.clone(), true, window, cx));
        let (artists, order, loaded) = match scope
            .is_none()
            .then(|| lib.kept.take::<KeptArtists>())
            .flatten()
        {
            Some(k) => (k.artists, k.order, Some(k.loaded)),
            None => (Vec::new(), Vec::new(), None),
        };
        let mut this = Self {
            lib,
            scope,
            page,
            artists,
            order,
            loaded,
            failed: false,
            loading: None,
            request: 0,
            _load: None,
            scroll: crate::components::UniformListScrollHandle::new(),
            selection: Selection::default(),
            pick: None,
            menu: None,
            starting: HashSet::new(),
            restore,
            rewinds,
            focus: cx.focus_handle().tab_stop(true),
            cursor: Default::default(),
            keyed: false,
            search,
            filters,
            _subscriptions: subscriptions,
        };
        // A kept listing draws at once, where the last visit left it.
        if this.loaded.is_some() {
            this.restore_scroll();
        }
        // A listing kept from a visit under another query (a shelf's "see
        // all" changed the filter) is not drawn: the page waits for its own,
        // as a first visit does, rather than drawing the old rows and
        // swapping them.
        if this
            .loaded
            .as_ref()
            .is_some_and(|(q, _)| *q != this.query(cx))
        {
            this.artists = Default::default();
            this.order.clear();
            this.loaded = None;
        }
        this.reload(cx);
        this
    }

    fn query(&self, cx: &App) -> Query {
        if let Some(shelf) = self.scope {
            return Query {
                search: super::scope::search(shelf, &self.lib, cx),
                filter: super::scope::filter(shelf),
            };
        }
        let browse = self.lib.browse.read(cx);
        Query {
            search: browse.search(),
            filter: browse.filter.clone(),
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
        let typed = self
            .loading
            .as_ref()
            .or(self.loaded.as_ref())
            .is_some_and(|(q, v)| *v == version && q.filter == wanted.0.filter);
        self.request += 1;
        let request = self.request;
        self.loading = Some(wanted.clone());
        let engine = self.lib.engine.clone();
        self._load = Some(cx.spawn(async move |this, cx| {
            if typed {
                cx.background_executor().timer(TYPING).await;
            }
            let q = wanted.0.clone();
            let artists = engine.artists(q.search, q.filter).await;
            this.update(cx, |this, cx| {
                if this.request != request {
                    return;
                }
                this.loading = None;
                match artists {
                    Ok(artists) => {
                        this.order = artists.iter().map(|a| Playable::Artist(a.id)).collect();
                        this.artists = artists;
                        this.loaded = Some(wanted);
                        this.restore_scroll();
                    }
                    Err(e) => {
                        log::warn!("artists: {e}");
                        this.failed = true;
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn pick(&mut self) -> Option<PlayableDrag> {
        if self.selection.picked().is_empty() {
            return None;
        }
        let revision = self.selection.revision();
        if self.pick.as_ref().is_none_or(|(r, _)| *r != revision) {
            let picked = self.selection.picked();
            let label = chrome::count(picked.len() as u64, "artist", "artists");
            self.pick = Some((revision, PlayableDrag::new(picked, label)));
        }
        self.pick.as_ref().map(|(_, drag)| drag.clone())
    }

    fn play(&mut self, artist: i64, cx: &mut Context<Self>) {
        self.starting.insert(artist);
        cx.notify();
        let started = actions::play(&self.lib, &[Playable::Artist(artist)], false, cx);
        cx.spawn(async move |this, cx| {
            started.await;
            this.update(cx, |this, cx| {
                this.starting.remove(&artist);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn row(&mut self, p: &Paint, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(artist) = self.artists.get(index).cloned() else {
            return div().h(rows::height(p)).into_any_element();
        };
        let id = artist.id;
        let item = Playable::Artist(id);
        let selected = self.selection.contains(item);
        let p = &p.picked(selected);
        let drag = match (selected, self.pick()) {
            (true, Some(pick)) => pick,
            _ => PlayableDrag::new(&[item], artist.name.clone()),
        };
        let favourite = self.lib.feed.read(cx).is_favourite(item);
        let starting = self.starting.contains(&id);
        let target = actions::MenuTarget {
            items: drag.items.clone(),
            album: None,
            after: None,
        };
        let view = cx.weak_entity();
        let heart = crate::components::Heart::new(("artist-heart", id as u64), favourite)
            .when(!favourite, |d| {
                d.invisible().group_hover(ROW_GROUP, |s| s.visible())
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.lib
                    .feed
                    .update(cx, |f, cx| f.toggle_favourite(item, cx));
            }));
        let name = div()
            .id(("artist-name", id as u64))
            .cursor_pointer()
            .hover(|s| s.underline())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.lib
                    .nav
                    .update(cx, |n, cx| n.open(Page::Artist(id), cx));
            }))
            .child(artist.name.clone());
        let play = cx.weak_entity();
        let count = self.artists.len();
        let scroll = self.scroll.clone();
        TableRow::new(("artist", id as u64), RowKind::Line)
            .when(p.a11y, |row| {
                row.spoken(crate::components::spoken([
                    artist.name.as_str(),
                    chrome::count(artist.album_count.max(0) as u64, "album", "albums").as_str(),
                    chrome::count(artist.track_count.max(0) as u64, "track", "tracks").as_str(),
                    if favourite { "favourite" } else { "" },
                ]))
                .place(index, count)
                .on_reveal(move |_, _| crate::components::reveal_around(&scroll, index, count))
            })
            .selected(selected)
            .cursor(self.keyed && self.cursor.at == Some(index))
            // Exactly the glyph's width, so it starts on the header's edge.
            .lead_exact(
                crate::components::RowMetrics::lead_glyph(&p.look),
                if starting {
                    crate::components::Spinner::new(p.look.font_size(Type::Meta)).into_any_element()
                } else {
                    RowNumber::glyph(("artist-play", id as u64), Icon::Artist)
                        .picked(selected)
                        .on_play(move |_, cx| {
                            play.update(cx, |this, cx| this.play(id, cx)).ok();
                        })
                        .into_any_element()
                },
            )
            .title(name)
            .figure(HEART, heart)
            .figure(
                COUNT,
                chrome::count(artist.album_count.max(0) as u64, "album", "albums"),
            )
            .figure(
                COUNT,
                chrome::count(artist.track_count.max(0) as u64, "track", "tracks"),
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                // The keyboard carries on from the row clicked.
                this.cursor.at = Some(index);
                this.cursor.anchor = Some(index);
                let order = &this.order;
                if rows::select(&mut this.selection, item, e, || order.clone(), cx) {
                    this.lib
                        .nav
                        .update(cx, |n, cx| n.open(Page::Artist(id), cx));
                }
                cx.notify();
            }))
            .on_drag(drag, |drag: &PlayableDrag, at, _, cx| drag.preview(at, cx))
            .on_mouse_down(MouseButton::Right, actions::mark(view, target))
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
}

impl actions::Menus for ArtistBrowser {
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

impl ArtistBrowser {
    /// The browser's controls, in the window's toolbar.
    pub fn controls(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        crate::components::ToolbarGroup::new()
            .child(self.search.clone())
            .spacer()
            .child(chrome::filter_button(&self.filters, &self.lib, cx))
            .into_any_element()
    }
}

impl Render for ArtistBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
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
        let header = chrome::page_header(
            p,
            self.scope.map_or("artists", |shelf| {
                super::scope::title(shelf, Listing::Artists)
            }),
            chrome::count(self.artists.len() as u64, "artist", "artists"),
            div(),
        );
        // Until its first listing lands the page draws nothing, then header,
        // total and rows together.
        let held = self.loaded.is_none() && !self.failed;
        let body = if held {
            div().into_any_element()
        } else if self.artists.is_empty() {
            chrome::empty_state(
                crate::theme::Icon::Artist,
                if narrowed {
                    "nothing matches"
                } else {
                    "no artists yet"
                },
                if narrowed {
                    "try a different filter."
                } else {
                    "add a music folder in settings, or sign in to a server."
                },
            )
            .into_any_element()
        } else {
            crate::components::uniform_list(
                "artists",
                self.artists.len(),
                cx.processor(|this, range: Range<usize>, window, cx| {
                    let p = Paint::of(window, cx);
                    range.map(|i| this.row(&p, i, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .map(|list| {
                crate::components::spoken_list("artists-list", "artists")
                    .size_full()
                    .child(list)
            })
            .into_any_element()
        };
        self.keyed = self.focus.is_focused(window);
        self.cursor.clamp(self.artists.len());
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
        .size_full()
        .flex()
        .flex_col()
        .typeset(p, Type::Body, p.ink)
        .when(!held, |d| d.child(header))
        .child(
            div()
                .id("artist-list")
                .flex_1()
                .min_h_0()
                .pt(p.look.spaced(10.0))
                .relative()
                .child(body)
                .child(crate::components::list_scrollbar(&self.scroll)),
        )
    }
}

impl super::keys::KeyedList for ArtistBrowser {
    fn cursor(&mut self) -> &mut crate::components::ListCursor {
        &mut self.cursor
    }

    fn len(&self) -> usize {
        self.artists.len()
    }

    fn pick(&mut self, from: usize, to: usize, _: &mut Context<Self>) {
        self.selection.only(self.order[from]);
        if to > from {
            self.selection.click(self.order[to], true, &self.order);
        }
    }

    fn toggle(&mut self, ix: usize, _: &mut Context<Self>) {
        self.selection.click(self.order[ix], false, &self.order);
    }

    fn reveal(&mut self, ix: usize, _: &mut Context<Self>) {
        self.scroll
            .scroll_to_item(ix, crate::components::ScrollStrategy::Nearest);
    }

    /// Return opens the artist, as a double click does.
    fn activate(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        let id = self.artists[ix].id;
        self.lib
            .nav
            .update(cx, |n, cx| n.open(Page::Artist(id), cx));
    }

    fn menu_at(&mut self, ix: Option<usize>, _: &mut Context<Self>) {
        self.menu = ix.map(|ix| actions::keyed_target(&self.selection, self.order[ix], None, None));
    }
}

impl Focusable for ArtistBrowser {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
