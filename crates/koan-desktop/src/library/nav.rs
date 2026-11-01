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

pub struct Nav {
    history: Vec<Page>,
    cursor: usize,
    /// How far down each page was scrolled when it was left, put back when
    /// it is shown again.
    offsets: HashMap<Page, Pixels>,
    /// Bumped when a tab is chosen while its page is already showing: the
    /// page goes back to its top.
    rewinds: HashMap<Page, u64>,
}

impl Default for Nav {
    fn default() -> Self {
        Self {
            history: vec![Page::Albums],
            cursor: 0,
            offsets: HashMap::new(),
            rewinds: HashMap::new(),
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

    pub fn rewinds(&self, page: Page) -> u64 {
        self.rewinds.get(&page).copied().unwrap_or(0)
    }
}

