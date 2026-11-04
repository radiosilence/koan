//! Search across the library: one query and three kinds of result. The
//! search box over the window (`bar`) types it and lists the answers, the
//! sidebar's field shows the query and opens the box, and the results page
//! shows the search shelf (`koan_core::shelves`), whose headings open the
//! browsers with the query as their search.
//!
//! Every keystroke is asked at once and answered off the main thread: the
//! engine reads the three kinds side by side on its own pool, and an answer
//! to an older query is dropped by generation when it lands. Nothing waits
//! for typing to pause, so an answer is drawn as soon as its read ends.

pub mod bar;
pub mod field;

use std::rc::Rc;
use std::sync::Arc;

use koan_kit::app::*;
use koan_runtime::{KoanEngine, ShelfKind, ShelfSummary};

use crate::navigator::{Listing, Navigator, Route, Shelf};

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

/// What a query found, and the query it answers: the results page's
/// previews, of which the search box lists the first few of each kind.
pub struct Results {
    pub query: String,
    pub summary: Rc<ShelfSummary>,
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
        self.ask(cx);
    }

    /// The library moved under the results: the same query, read again.
    pub fn library_changed(&mut self, version: Option<u64>, cx: &mut Context<Self>) {
        let last = std::mem::replace(&mut self.library, version);
        if last.is_some() && last != version && !self.query.is_empty() {
            self.ask(cx);
        }
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let (engine, generation, query) =
            (self.engine.clone(), self.generation, self.query.clone());
        self._task = Some(cx.spawn(async move |this, cx| {
            let shelf = ShelfKind::Search {
                query: query.clone(),
            };
            let summary = match engine.shelf_summary(shelf).await {
                Ok(summary) => summary,
                Err(e) => {
                    log::warn!("search: {e}");
                    return;
                }
            };
            let results = Rc::new(Results {
                query,
                summary: Rc::new(summary),
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
