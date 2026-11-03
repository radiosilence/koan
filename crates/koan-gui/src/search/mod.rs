//! Search across the library: one query and three kinds of result. The
//! sidebar's field types it, a panel under the field suggests a few of each
//! kind, and the results page shows the search shelf (`koan_core::shelves`),
//! whose headings open the browsers with the query as their search. The
//! picker (⇧⌘K) is a separate sheet for building a queue.
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

use koan_ffi::{KoanEngine, ShelfKind, ShelfSummary};
use koan_kit::app::*;

use crate::navigator::{Listing, Navigator, Route, Shelf};

const SETTLE: Duration = Duration::from_millis(90);

/// How deep the search box's answers go, as the ⇧⌘K picker's did: enough
/// to build a queue from.
const DEEP_ARTISTS: u32 = 12;
const DEEP_ALBUMS: u32 = 40;
const DEEP_TRACKS: u32 = 60;

/// What the suggestions panel offers of each kind, as the Mac's does.
const SUGGESTED_TRACKS: usize = 5;
const SUGGESTED_ALBUMS: usize = 4;
const SUGGESTED_ARTISTS: usize = 4;

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

/// One line of the suggestions panel, its strings made once per answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub kind: Kind,
    pub id: i64,
    /// The record whose sleeve the line shows; an artist shows a glyph.
    pub album: Option<i64>,
    pub title: SharedString,
    pub detail: Option<SharedString>,
    /// Where taking it goes: a track lives on its record.
    pub route: Option<Route>,
}

/// What a query found, and the query it answers.
pub struct Results {
    pub query: String,
    pub summary: ShelfSummary,
    pub suggestions: Vec<Suggestion>,
}

impl Results {
    pub fn new(query: String, summary: ShelfSummary) -> Self {
        let suggestions = suggestions(&summary);
        Self {
            query,
            summary,
            suggestions,
        }
    }
}

fn credit(parts: &[koan_ffi::CreditPart]) -> String {
    parts.iter().map(|p| p.text.as_str()).collect()
}

/// Five tracks, four records, four artists: enough to recognise what was
/// meant. Everything else is behind Return.
pub fn suggestions(s: &ShelfSummary) -> Vec<Suggestion> {
    let tracks = s.tracks.iter().take(SUGGESTED_TRACKS).map(|t| Suggestion {
        kind: Kind::Track,
        id: t.id,
        album: t.album_id,
        title: t.title.clone().into(),
        detail: Some(format!("{} · {}", credit(&t.artist_credit), t.album_title).into()),
        route: t.album_id.map(Route::Album),
    });
    let albums = s.albums.iter().take(SUGGESTED_ALBUMS).map(|a| Suggestion {
        kind: Kind::Album,
        id: a.id,
        album: Some(a.id),
        title: a.title.clone().into(),
        detail: Some(credit(&a.artist_credit).into()),
        route: Some(Route::Album(a.id)),
    });
    let artists = s
        .artists
        .iter()
        .take(SUGGESTED_ARTISTS)
        .map(|a| Suggestion {
            kind: Kind::Artist,
            id: a.id,
            album: None,
            title: a.name.clone().into(),
            detail: None,
            route: Some(Route::Artist(a.id)),
        });
    tracks.chain(albums).chain(artists).collect()
}

/// The model emptied the query (a suggestion was taken): the field's text
/// follows.
pub struct Cleared;

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
    /// the sidebar's suggestions and the results page's previews, as the box
    /// is picked from.
    deep: bool,
    _task: Option<Task<()>>,
}

impl EventEmitter<Cleared> for Search {}

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

    /// The field's text changed. An empty one forgets the results page,
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
            let results = Rc::new(Results::new(query, summary));
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

    /// Return with no suggestion lit: every result, on their page.
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

    /// Goes where a suggestion lives, then empties the query without taking
    /// the person back: they have been sent somewhere.
    pub fn take(&mut self, suggestion: &Suggestion, cx: &mut Context<Self>) {
        if let Some(route) = suggestion.route {
            self.navigator.update(cx, |nav, cx| {
                if nav.go(route) {
                    cx.notify();
                }
            });
        }
        self.query.clear();
        self.clear(cx);
        cx.emit(Cleared);
    }
}

/// The suggestion lit after an arrow key: down from none lights the first,
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

    #[test]
    fn suggestions_take_a_few_of_each_kind_in_order() {
        let artist = |id| koan_ffi::Artist {
            id,
            name: format!("artist {id}"),
            sort_name: None,
            album_count: 0,
            track_count: 0,
        };
        let summary = ShelfSummary {
            artists: (1..=6).map(artist).collect(),
            artist_total: 6,
            albums: Vec::new(),
            album_total: 0,
            tracks: Vec::new(),
            track_total: 0,
        };
        let s = suggestions(&summary);
        assert_eq!(s.len(), SUGGESTED_ARTISTS);
        assert!(s.iter().all(|s| s.kind == Kind::Artist));
        assert_eq!(s[0].route, Some(Route::Artist(1)));
    }
}
