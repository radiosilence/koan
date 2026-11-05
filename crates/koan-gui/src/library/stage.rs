//! The library's part of the shell's stage: the page the navigator is on,
//! and a track's details over it.

use koan_kit::app::*;

use super::Lib;
use super::albums::AlbumGrid;
use super::nav::Page;

pub struct LibraryStage {
    lib: Lib,
    /// The page on screen, built when the navigator arrives at it. Pages are
    /// rebuilt on each visit and put their scroll back from the navigator.
    page: Option<(Page, AnyView)>,
    info: Option<(i64, Entity<super::info::TrackInfo>)>,
    /// The page's controls, for the window's toolbar.
    controls: Entity<super::controls::ControlsSlot>,
    /// Where the page on screen takes the keyboard, for the shell's F6.
    page_focus: Option<FocusHandle>,
    /// The shell moved to a library page: the next page made takes the
    /// keyboard, as the queue does when it is shown.
    focus_on_arrival: bool,
    _subscriptions: Vec<Subscription>,
}

impl LibraryStage {
    /// What the browsers are narrowed and ordered by.
    pub fn browse(&self) -> &Entity<super::browse::Browse> {
        &self.lib.browse
    }

    pub fn new(lib: Lib, _: &mut Window, cx: &mut Context<Self>) -> Self {
        // A page is dropped when the shell shows a page of its own, so a
        // hidden grid neither reloads nor keeps its sleeves ranked on screen;
        // its place comes back from the navigator's offsets.
        let mut subscriptions = vec![cx.observe(&lib.nav, |this: &mut Self, nav, cx| {
            show_record(nav.read(cx).current(cx), cx);
            if !Self::draws(nav.read(cx).current(cx)) {
                this.page = None;
                this.focus_on_arrival = false;
                this.controls.update(cx, |c, cx| c.clear(cx));
            }
            cx.notify();
        })];
        // Gone with its window (closed, the app kept running): no record's
        // page shows, and the windows left follow the playing record.
        subscriptions.push(cx.on_release(|_, cx| {
            if cx
                .try_global::<crate::root::ShownRecord>()
                .is_some_and(|s| s.0.is_some())
            {
                cx.set_global(crate::root::ShownRecord(None));
            }
        }));
        show_record(lib.nav.read(cx).current(cx), cx);
        Self {
            lib,
            page: None,
            info: None,
            controls: cx.new(|_| super::controls::ControlsSlot::default()),
            page_focus: None,
            focus_on_arrival: false,
            _subscriptions: subscriptions,
        }
    }

    /// Where the page on screen takes the keyboard.
    pub fn page_focus(&self) -> Option<FocusHandle> {
        self.page_focus.clone()
    }

    /// The page showing takes the keyboard: at once when it is already
    /// made (back from the shortcuts or the artwork viewer), else as the
    /// stage makes it. Only a page about to be made is waited for, so a wait
    /// never outlives the frame and takes the keyboard from a field later.
    pub fn take_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.lib.nav.read(cx).current(cx);
        match (&self.page, &self.page_focus) {
            (Some((page, _)), Some(focus)) if *page == current => {
                self.focus_on_arrival = false;
                window.focus(focus, cx);
            }
            _ => self.focus_on_arrival = true,
        }
    }

    /// What the library's pages read and act through.
    pub fn lib(&self) -> super::Lib {
        self.lib.clone()
    }

    /// The library's artwork cache.
    pub fn covers(&self) -> Entity<super::covers::Covers> {
        self.lib.covers.clone()
    }

    /// Whether the library draws `route`.
    pub fn draws(route: Page) -> bool {
        matches!(
            route,
            Page::Albums
                | Page::Artists
                | Page::Tracks
                | Page::Album(_)
                | Page::Artist(_)
                | Page::Favourites
                | Page::RecentlyPlayed
                | Page::OnDevice
                | Page::Search
                | Page::History
                | Page::Playlist(_)
                | Page::Shelved(..)
        )
    }

    /// The page's controls for the window's toolbar: its filter field and
    /// buttons, drawn in the toolbar's trailing items. None until the pages
    /// provide them.
    pub fn toolbar(&self) -> Option<AnyView> {
        Some(self.controls.clone().into())
    }

    fn page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyView> {
        let current = self.lib.nav.read(cx).current(cx);
        if let Some((page, view)) = &self.page
            && *page == current
        {
            return Some(view.clone());
        }
        let lib = self.lib.clone();
        let lib_browse = self.lib.browse.clone();
        let slot = self.controls.clone();
        // Each page with where it takes the keyboard.
        fn held<T: Render + Focusable>(page: Entity<T>, cx: &App) -> (AnyView, FocusHandle) {
            let focus = page.focus_handle(cx);
            (page.into(), focus)
        }
        let (view, focus): (AnyView, FocusHandle) = match current {
            Page::Albums => {
                let page = cx.new(|cx| AlbumGrid::new(lib, window, cx));
                let browse = lib_browse.clone();
                slot.update(cx, |s, cx| {
                    s.show_keyed(&page, AlbumGrid::controls, AlbumGrid::picking, &browse, cx)
                });
                held(page, cx)
            }
            Page::Artists => {
                let page = cx.new(|cx| super::artists::ArtistBrowser::new(lib, window, cx));
                slot.update(cx, |s, cx| {
                    s.show(&page, super::artists::ArtistBrowser::controls, cx)
                });
                held(page, cx)
            }
            Page::Album(id) => {
                let page = cx.new(|cx| super::album_page::AlbumPage::new(lib, id, window, cx));
                held(page, cx)
            }
            Page::Artist(id) => {
                let page = cx.new(|cx| super::artist_page::ArtistPage::new(lib, id, window, cx));
                held(page, cx)
            }
            Page::Tracks => {
                let page = cx.new(|cx| super::tracks::TrackBrowser::new(lib, window, cx));
                let browse = lib_browse.clone();
                slot.update(cx, |s, cx| {
                    s.show_keyed(
                        &page,
                        super::tracks::TrackBrowser::controls,
                        super::tracks::TrackBrowser::picking,
                        &browse,
                        cx,
                    )
                });
                held(page, cx)
            }
            Page::Playlist(id) => {
                let page = cx.new(|cx| super::playlist::PlaylistPage::new(lib, id, window, cx));
                slot.update(cx, |s, cx| {
                    s.show(&page, super::playlist::PlaylistPage::controls, cx)
                });
                held(page, cx)
            }
            Page::History => {
                let page = cx.new(|cx| super::history::HistoryPage::new(lib, window, cx));
                slot.update(cx, |s, cx| {
                    s.show(&page, super::history::HistoryPage::controls, cx)
                });
                held(page, cx)
            }
            Page::Search => {
                let page = cx.new(|cx| super::shelf::ShelfPage::results(lib, window, cx));
                held(page, cx)
            }
            // A shelf's "see all": the browser on the shelf's own filter, with
            // no controls to change it.
            Page::Shelved(shelf, listing) => {
                use crate::navigator::Listing;
                match listing {
                    Listing::Albums => held(
                        cx.new(|cx| AlbumGrid::scoped(lib, Some(shelf), window, cx)),
                        cx,
                    ),
                    Listing::Artists => held(
                        cx.new(|cx| {
                            super::artists::ArtistBrowser::scoped(lib, Some(shelf), window, cx)
                        }),
                        cx,
                    ),
                    Listing::Tracks => held(
                        cx.new(|cx| {
                            super::tracks::TrackBrowser::scoped(lib, Some(shelf), window, cx)
                        }),
                        cx,
                    ),
                }
            }
            page => {
                // The shell draws its own pages for every other route.
                let shelf = super::shelf::shelf_of(page)?;
                let page = cx.new(|cx| super::shelf::ShelfPage::new(lib, shelf, window, cx));
                held(page, cx)
            }
        };
        if super::shelf::shelf_of(current).is_some()
            || matches!(
                current,
                Page::Album(_) | Page::Artist(_) | Page::Search | Page::Shelved(..)
            )
        {
            slot.update(cx, |s, cx| s.clear(cx));
        }
        if std::mem::take(&mut self.focus_on_arrival) {
            window.focus(&focus, cx);
        }
        self.page_focus = Some(focus);
        self.page = Some((current, view.clone()));
        Some(view)
    }
}

impl Render for LibraryStage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.page(window, cx);
        let wanted = self.lib.nav.read(cx).info;
        if self.info.as_ref().map(|(id, _)| *id) != wanted {
            let lib = self.lib.clone();
            self.info = wanted.map(|id| {
                (
                    id,
                    cx.new(|cx| super::info::TrackInfo::new(lib, id, window, cx)),
                )
            });
        }
        div()
            .relative()
            .size_full()
            // Each page replays its last frame unless it notified, so the
            // overlay above it never redraws a 5,000-record grid.
            .children(page.map(|page| page.cached(StyleRefinement::default().size_full())))
            // Above the page and the playing bars painted over it.
            .when_some(self.info.as_ref(), |d, (_, info)| {
                d.child(crate::components::sheet::layered(
                    crate::components::sheet::Layer::Panel,
                    info.clone(),
                ))
            })
    }
}

/// The window takes a record's colours while its page is the one showing,
/// and the playing record's otherwise: decided by the route, so it follows
/// back, forward and every other way the page changes.
fn show_record(route: Page, cx: &mut App) {
    let shown = match route {
        Page::Album(id) => Some(id),
        _ => None,
    };
    if cx
        .try_global::<crate::root::ShownRecord>()
        .and_then(|s| s.0)
        != shown
    {
        cx.set_global(crate::root::ShownRecord(shown));
    }
}
