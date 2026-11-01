//! The library's part of the shell's stage: the page the navigator is on,
//! and a track's details over it.

use koan_kit::raw::prelude::*;
use koan_kit::raw::*;

use super::Lib;
use super::albums::AlbumGrid;
use super::nav::Page;

pub struct LibraryStage {
    lib: Lib,
    /// The page on screen, built when the navigator arrives at it. Pages are
    /// rebuilt on each visit and put their scroll back from the navigator.
    page: Option<(Page, AnyView)>,
    info: Option<(i64, Entity<super::info::TrackInfo>)>,
    _subscriptions: Vec<Subscription>,
}

impl LibraryStage {
    pub fn new(lib: Lib, _: &mut Window, cx: &mut Context<Self>) -> Self {
        // A page is dropped when the shell shows a page of its own, so a
        // hidden grid neither reloads nor keeps its sleeves ranked on screen;
        // its place comes back from the navigator's offsets.
        let subscriptions = vec![cx.observe(&lib.nav, |this: &mut Self, nav, cx| {
            if !Self::draws(nav.read(cx).current(cx)) {
                this.page = None;
            }
            cx.notify();
        })];
        Self {
            lib,
            page: None,
            info: None,
            _subscriptions: subscriptions,
        }
    }

    /// Whether the library draws `route`.
    pub fn draws(route: Page) -> bool {
        matches!(
            route,
            Page::Albums | Page::Artists | Page::Tracks | Page::Album(_) | Page::Artist(_)
        )
    }

    /// The page's controls for the window's toolbar: its filter field and
    /// buttons, drawn in the toolbar's trailing items. None until the pages
    /// provide them.
    pub fn toolbar(&self) -> Option<AnyView> {
        None
    }

    fn page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyView> {
        let current = self.lib.nav.read(cx).current(cx);
        if let Some((page, view)) = &self.page
            && *page == current
        {
            return Some(view.clone());
        }
        let lib = self.lib.clone();
        let view: AnyView = match current {
            Page::Albums => cx.new(|cx| AlbumGrid::new(lib, window, cx)).into(),
            Page::Artists => cx
                .new(|cx| super::artists::ArtistBrowser::new(lib, window, cx))
                .into(),
            Page::Album(id) => cx
                .new(|cx| super::album_page::AlbumPage::new(lib, id, window, cx))
                .into(),
            Page::Artist(id) => cx
                .new(|cx| super::artist_page::ArtistPage::new(lib, id, window, cx))
                .into(),
            Page::Tracks => cx
                .new(|cx| super::tracks::TrackBrowser::new(lib, window, cx))
                .into(),
            // The shell draws its own pages for every other route.
            _ => return None,
        };
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
