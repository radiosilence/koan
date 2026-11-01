//! Where the library window is: one page, the pages visited, and where each
//! listing was scrolled to. A stand-in for the app's navigator, with the same
//! shape: any page from any page, back and forward through what was visited.

use std::collections::HashMap;

use gpui_kit::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Page {
    Albums,
    Artists,
    Tracks,
    Album(i64),
    Artist(i64),
}

impl Page {
    /// The library's own pages, which the tabs choose between.
    pub fn is_root(self) -> bool {
        matches!(self, Page::Albums | Page::Artists | Page::Tracks)
    }
}

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
    history: Vec<Page>,
    cursor: usize,
    /// How far down each page was scrolled when it was left, put back when
    /// it is shown again.
    offsets: HashMap<Page, Pixels>,
    /// The same, for pages laid out in items of different heights.
    anchors: HashMap<Page, Anchor>,
    /// Bumped when a tab is chosen while its page is already showing: the
    /// page goes back to its top.
    rewinds: HashMap<Page, u64>,
    /// The track whose details are open over the page.
    pub info: Option<i64>,
}

impl Default for Nav {
    fn default() -> Self {
        Self {
            history: vec![Page::Albums],
            cursor: 0,
            offsets: HashMap::new(),
            anchors: HashMap::new(),
            rewinds: HashMap::new(),
            info: None,
        }
    }
}

impl Nav {
    pub fn current(&self) -> Page {
        self.history[self.cursor]
    }

    pub fn open(&mut self, page: Page, cx: &mut Context<Self>) {
        if page == self.current() {
            if page.is_root() {
                *self.rewinds.entry(page).or_default() += 1;
                self.offsets.remove(&page);
                cx.notify();
            }
            return;
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(page);
        self.cursor += 1;
        cx.notify();
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.can_go_back() {
            self.cursor -= 1;
            cx.notify();
        }
    }

    pub fn forward(&mut self, cx: &mut Context<Self>) {
        if self.can_go_forward() {
            self.cursor += 1;
            cx.notify();
        }
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

    pub fn rewinds(&self, page: Page) -> u64 {
        self.rewinds.get(&page).copied().unwrap_or(0)
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
