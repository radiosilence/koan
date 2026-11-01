//! What the library pages keep beside the app's navigator: where each listing
//! was scrolled to, and the track whose details are open. Moves go to the
//! navigator, which the pages reach through this.

use std::collections::HashMap;

use koan_kit::raw::*;

use crate::navigator::Navigator;
pub use crate::navigator::Route as Page;

/// A place on a page that survives the page being laid out again: the item
/// at the top of the screen, named by what it shows, and how far into it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub at: Anchored,
    pub offset: Pixels,
    /// Where the item was, for when it is gone: the place nearest it rather
    /// than the top.
    pub item_ix: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchored {
    Header,
    Heading(Heading),
    /// The row of records that starts with this one.
    Album {
        id: i64,
        features: bool,
    },
    Track(i64),
}

/// A heading on a page, by what it heads rather than where it falls, since
/// headings come and go as sections do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heading {
    Section(koan_ffi::ReleaseSection),
    Features,
    Appearances,
}

pub struct Nav {
    navigator: Entity<Navigator>,
    /// How far down each page was scrolled when it was left, put back when
    /// it is shown again.
    offsets: HashMap<Page, Pixels>,
    /// The same, for pages laid out in items of different heights.
    anchors: HashMap<Page, Anchor>,
    /// The track whose details are open over the page.
    pub info: Option<i64>,
    _navigator: Subscription,
}

impl Nav {
    pub fn new(navigator: Entity<Navigator>, cx: &mut Context<Self>) -> Self {
        // Pages observe this, so a move or a rewind reaches them through it.
        let watch = cx.observe(&navigator, |_, _, cx| cx.notify());
        Self {
            navigator,
            offsets: HashMap::new(),
            anchors: HashMap::new(),
            info: None,
            _navigator: watch,
        }
    }

    pub fn current(&self, cx: &App) -> Page {
        self.navigator.read(cx).current()
    }

    pub fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        self.navigator.update(cx, |nav, cx| {
            if nav.go(page) {
                cx.notify();
            }
        });
    }

    pub fn offset(&self, page: Page) -> Option<Pixels> {
        self.offsets.get(&page).copied()
    }

    /// Not a change anyone redraws for, so it does not notify.
    pub fn remember_offset(&mut self, page: Page, offset: Pixels) {
        self.offsets.insert(page, offset);
    }

    pub fn anchor(&self, page: Page) -> Option<Anchor> {
        self.anchors.get(&page).copied()
    }

    pub fn remember_anchor(&mut self, page: Page, anchor: Anchor) {
        self.anchors.insert(page, anchor);
    }

    /// How many times the page has been sent back to its top from the
    /// sidebar; a page goes to its top when this moves.
    pub fn rewinds(&self, page: Page, cx: &App) -> u64 {
        u64::from(self.navigator.read(cx).rewinds(page))
    }

    pub fn show_info(&mut self, track: i64, cx: &mut Context<Self>) {
        self.info = Some(track);
        cx.notify();
    }

    pub fn close_info(&mut self, cx: &mut Context<Self>) {
        if self.info.take().is_some() {
            cx.notify();
        }
    }
}
