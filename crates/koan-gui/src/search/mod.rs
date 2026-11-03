//! Search across the library: one query and three kinds of result. The
//! search box over the window (`bar`) types it and lists the answers, the
//! sidebar's field shows the query and opens the box, and the results page
//! shows the search shelf (`koan_core::shelves`), whose headings open the
//! browsers with the query as their search.
//!
//! Every keystroke is answered off the main thread: the engine reads on its
//! own pool, and an answer to an older query is dropped by generation when
//! it lands. Typing settles briefly first, which only spares the engine a
//! round of queries per keystroke from a fast typist.

pub mod bar;
pub mod field;

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use koan_kit::app::*;
use koan_runtime::{KoanEngine, ShelfKind, ShelfSummary};

use crate::navigator::{Listing, Navigator, Route, Shelf};

const SETTLE: Duration = Duration::from_millis(90);

/// How deep the search box's answers go while it is open: enough to build
/// a queue from.
const DEEP_ARTISTS: u32 = 12;
const DEEP_ALBUMS: u32 = 40;
const DEEP_TRACKS: u32 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
    Artist,
}

impl Kind {
    pub fn heading(self) -> &'static str {
        match self {
            Kind::Track => "tracks",
            Kind::Album => "albums",
            Kind::Artist => "artists",
        }
    }
}

/// What a query found, and the query it answers.
pub struct Results {
    pub query: String,
    pub summary: ShelfSummary,
    /// Read as deep as the search box lists, while it is open: the results
    /// page takes only its previews' worth.
    pub deep: bool,
}

pub struct Search {
    engine: Arc<KoanEngine>,
    navigator: Entity<Navigator>,
    /// What was typed, trimmed.
    query: String,
    results: Option<Rc<Results>>,
    /// Moves with every query, so an answer to an older one is dropped on
    /// landing: replacing the task cancels the wait, not the engine's read.
    generation: u64,
    /// The library's version the results were read at: a change to the
    /// library asks the query again.
    library: Option<u64>,
    /// The search box over the window is open: its answers go deeper than
    /// the results page's previews, as the box is picked from.
    deep: bool,
    _task: Option<Task<()>>,
}

impl Search {
    pub fn new(engine: Arc<KoanEngine>, navigator: Entity<Navigator>) -> Self {
        Self {
            engine,
            navigator,
            query: String::new(),
            results: None,
            generation: 0,
            library: None,
            deep: false,
            _task: None,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn results(&self) -> Option<&Rc<Results>> {
        self.results.as_ref()
    }

    /// The query changed. An empty one forgets the results page,
    /// which lands on the page searched from when it was showing.
    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text == self.query {
            return;
        }
        self.query = text.to_string();
        if text.is_empty() {
            self.clear(cx);
            return;
        }
        self.ask(SETTLE, cx);
    }

    /// Whether answers go as deep as the search box lists; a change asks the
    /// query again.
    pub fn set_deep(&mut self, deep: bool, cx: &mut Context<Self>) {
        if self.deep != deep {
            self.deep = deep;
            if !self.query.is_empty() {
                self.ask(Duration::ZERO, cx);
            }
        }
    }

    /// The library moved under the results: the same query, read again.
    pub fn library_changed(&mut self, version: Option<u64>, cx: &mut Context<Self>) {
        let last = std::mem::replace(&mut self.library, version);
        if last.is_some() && last != version && !self.query.is_empty() {
            self.ask(Duration::ZERO, cx);
        }
    }

    fn ask(&mut self, settle: Duration, cx: &mut Context<Self>) {
        self.generation += 1;
        let (engine, generation, query, deep) = (
            self.engine.clone(),
            self.generation,
            self.query.clone(),
            self.deep,
        );
        self._task = Some(cx.spawn(async move |this, cx| {
            if !settle.is_zero() {
                cx.background_executor().timer(settle).await;
            }
            let shelf = ShelfKind::Search {
                query: query.clone(),
            };
            let read = if deep {
                engine
                    .shelf_summary_sized(shelf, DEEP_ARTISTS, DEEP_ALBUMS, DEEP_TRACKS)
                    .await
            } else {
                engine.shelf_summary(shelf).await
            };
            let summary = match read {
                Ok(summary) => summary,
                Err(e) => {
                    log::warn!("search: {e}");
                    return;
                }
            };
            let results = Rc::new(Results {
                query,
                summary,
                deep,
            });
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.results = Some(results);
                cx.notify();
            })
            .ok();
        }));
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self._task = None;
        self.results = None;
        // The search's own pages go with it: its "see all" listings read the
        // query, which is gone.
        self.navigator.update(cx, |nav, cx| {
            nav.forget(Route::Search);
            for listing in [Listing::Albums, Listing::Artists, Listing::Tracks] {
                nav.forget(Route::Shelved(Shelf::Search, listing));
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Every result, on their page.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.query.is_empty() {
            return;
        }
        self.navigator.update(cx, |nav, cx| {
            if nav.go(Route::Search) {
                cx.notify();
            }
        });
    }
}

/// The row lit after an arrow key: down from none lights the first,
/// up from the first lights none, and both stop at the ends.
pub fn step(lit: Option<usize>, down: bool, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match (lit, down) {
        (None, true) => Some(0),
        (None, false) => None,
        (Some(i), true) => Some((i + 1).min(count - 1)),
        (Some(0), false) => None,
        (Some(i), false) => Some(i - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn arrows_light_within_the_list_and_up_from_the_top_lights_none() {
        assert_eq!(step(None, true, 3), Some(0));
        assert_eq!(step(Some(2), true, 3), Some(2));
        assert_eq!(step(Some(1), false, 3), Some(0));
        assert_eq!(step(Some(0), false, 3), None);
        assert_eq!(step(None, false, 3), None);
        assert_eq!(step(None, true, 0), None);
    }
}
