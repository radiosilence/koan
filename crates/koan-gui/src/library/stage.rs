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
    _subscriptions: Vec<Subscription>,
}

impl LibraryStage {
    pub fn new(lib: Lib, _: &mut Window, cx: &mut Context<Self>) -> Self {
        // A page is dropped when the shell shows a page of its own, so a
        // hidden grid neither reloads nor keeps its sleeves ranked on screen;
        // its place comes back from the navigator's offsets.
        let subscriptions = vec![cx.observe(&lib.nav, |this: &mut Self, nav, cx| {
            show_record(nav.read(cx).current(cx), cx);
            if !Self::draws(nav.read(cx).current(cx)) {
                this.page = None;
                this.controls.update(cx, |c, cx| c.clear(cx));
            }
            cx.notify();
        })];
        show_record(lib.nav.read(cx).current(cx), cx);
        Self {
            lib,
            page: None,
            info: None,
            controls: cx.new(|_| super::controls::ControlsSlot::default()),
            _subscriptions: subscriptions,
        }
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
                | Page::Playlist(_)
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
        let view: AnyView = match current {
            Page::Albums => {
                let page = cx.new(|cx| AlbumGrid::new(lib, window, cx));
                let browse = lib_browse.clone();
                slot.update(cx, |s, cx| {
                    s.show_keyed(&page, AlbumGrid::controls, AlbumGrid::picking, &browse, cx)
                });
                page.into()
            }
            Page::Artists => {
                let page = cx.new(|cx| super::artists::ArtistBrowser::new(lib, window, cx));
                slot.update(cx, |s, cx| {
                    s.show(&page, super::artists::ArtistBrowser::controls, cx)
                });
                page.into()
            }
            Page::Album(id) => cx
                .new(|cx| super::album_page::AlbumPage::new(lib, id, window, cx))
                .into(),
            Page::Artist(id) => cx
                .new(|cx| super::artist_page::ArtistPage::new(lib, id, window, cx))
                .into(),
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
                page.into()
            }
            Page::Playlist(id) => {
                let page = cx.new(|cx| super::playlist::PlaylistPage::new(lib, id, window, cx));
                slot.update(cx, |s, cx| {
                    s.show(&page, super::playlist::PlaylistPage::controls, cx)
                });
                page.into()
            }
            Page::Search => cx
                .new(|cx| super::shelf::ShelfPage::results(lib, window, cx))
                .into(),
            page => match super::shelf::shelf_of(page) {
                Some(shelf) => cx
                    .new(|cx| super::shelf::ShelfPage::new(lib, shelf, window, cx))
                    .into(),
                // The shell draws its own pages for every other route.
                None => return None,
            },
        };
        if super::shelf::shelf_of(current).is_some()
            || matches!(current, Page::Album(_) | Page::Artist(_) | Page::Search)
        {
            slot.update(cx, |s, cx| s.clear(cx));
        }
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
            .when_some(self.info.as_ref(), |d, (_, info)| d.child(info.clone()))
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
