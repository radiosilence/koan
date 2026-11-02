//! The artist browser: every artist in the library, narrowed by the shared
//! filter and search, as the Mac's artist list draws them: a play mark under
//! the pointer, the name, which opens the artist, the heart, and how many
//! records and tracks.

use std::collections::HashSet;
use std::ops::Range;
use std::time::Duration;

use koan_ffi::{Artist, BrowseFilter};
use koan_kit::app::*;
// sizes and colours computed at runtime from measurements, not chosen from the theme.
use koan_kit::raw::{point, px};
// virtualised lists and their scroll state, which GPUI owns and koan-kit's rows sit in.
use koan_kit::raw::{UniformListScrollHandle, uniform_list};

use super::drag::{Playable, PlayableDrag};
use super::nav::Page;
use super::selection::Selection;
use super::tokens::{Paint, Type, Typeset};
use super::{Dismiss, Lib, SelectAll, actions, chrome, rows};
use crate::components::{ROW_GROUP, RowKind, RowNumber, TableRow};
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
    artists: Vec<Artist>,
    order: Vec<Playable>,
    loaded: Option<(Query, u64)>,
    loading: Option<(Query, u64)>,
    request: u64,
    _load: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    selection: Selection,
    pick: Option<(u64, PlayableDrag)>,
    /// What the list's context menu acts on: the row last right-clicked.
    menu: Option<actions::MenuTarget>,
    starting: HashSet<i64>,
    restore: Option<Pixels>,
    rewinds: u64,
    focus: FocusHandle,
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
                .set_offset(point(px(0.0), -y));
        }
    }

    pub fn new(lib: Lib, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            // Cached under the window, and draws the accent.
            crate::theme::observe_accent(cx),
            cx.observe(&lib.browse, |this, _, cx| this.reload(cx)),
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
            cx.on_release(|this, cx| {
                if let Some(loaded) = this.loaded.take() {
                    this.lib.kept.put(KeptArtists {
                        artists: std::mem::take(&mut this.artists),
                        order: std::mem::take(&mut this.order),
                        loaded,
                    });
                }
                let offset = -this.scroll.0.borrow().base_handle.offset().y;
                this.lib
                    .nav
                    .update(cx, |nav, _| nav.remember_offset(Page::Artists, offset));
            }),
        ];
        let rewinds = lib.nav.read(cx).rewinds(Page::Artists, cx);
        let restore = lib.nav.read(cx).offset(Page::Artists);
        let search =
            cx.new(|cx| chrome::SearchField::new(lib.browse.clone(), "filter artists", window, cx));
        let filters = cx.new(|cx| chrome::FilterPanel::new(lib.clone(), true, window, cx));
        let (artists, order, loaded) = match lib.kept.take::<KeptArtists>() {
            Some(k) => (k.artists, k.order, Some(k.loaded)),
            None => (Vec::new(), Vec::new(), None),
        };
        let mut this = Self {
            lib,
            artists,
            order,
            loaded,
            failed: false,
            loading: None,
            request: 0,
            _load: None,
            scroll: UniformListScrollHandle::new(),
            selection: Selection::default(),
            pick: None,
            menu: None,
            starting: HashSet::new(),
            restore,
            rewinds,
            focus: cx.focus_handle(),
            search,
            filters,
            _subscriptions: subscriptions,
        };
        // A kept listing draws at once, where the last visit left it.
        if this.loaded.is_some() {
            this.restore_scroll();
        }
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(version) = self.lib.feed.read(cx).library_version else {
            return;
        };
        let browse = self.lib.browse.read(cx);
        let wanted = (
            Query {
                search: browse.search(),
                filter: browse.filter.clone(),
            },
            version,
        );
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
        let heart = div()
            .id(("artist-heart", id as u64))
            .cursor_pointer()
            .text_color(if favourite { p.accent } else { p.muted })
            .when(!favourite, |d| {
                d.invisible().group_hover(ROW_GROUP, |s| s.visible())
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.lib
                    .feed
                    .update(cx, |f, cx| f.toggle_favourite(item, cx));
            }))
            .child(super::tile::heart_glyph(p, favourite));
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
        TableRow::new(("artist", id as u64), RowKind::Line)
            .selected(selected)
            // Exactly the glyph's width, so it starts on the header's edge.
            .lead_exact(
                crate::components::RowMetrics::lead_glyph(&p.look),
                if starting {
                    crate::components::Spinner::new(p.look.font_size(Type::Meta)).into_any_element()
                } else {
                    RowNumber::glyph(("artist-play", id as u64), Icon::Artist)
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
        let rewinds = self.lib.nav.read(cx).rewinds(Page::Artists, cx);
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
            "artists",
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
            uniform_list(
                "artists",
                self.artists.len(),
                cx.processor(|this, range: Range<usize>, window, cx| {
                    let p = Paint::of(window, cx);
                    range.map(|i| this.row(&p, i, cx)).collect::<Vec<_>>()
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
            .when(!held, |d| d.child(header))
            .child(actions::list_menu(
                div()
                    .id("artist-list")
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
