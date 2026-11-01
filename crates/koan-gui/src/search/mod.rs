//! Search across the library: one query and three result sets, the shape
//! every streaming client uses. The sidebar's field types it, a panel under
//! the field suggests a few of each kind, and the results page shows them
//! all. The picker (⇧⌘K) is a separate sheet for building a queue.
//!
//! The results are the search shelf's (`koan_core::shelves`): each section is
//! the head of the listing its heading opens, and its total that listing's
//! length. The closest matches come first; a query nothing holds as typed
//! falls back to fuzzy matching, so a typo still finds what was meant.

pub mod field;
pub mod picker;
pub mod replace;
pub mod results;

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use koan_ffi::{KoanEngine, ShelfKind, ShelfSummary};

use crate::navigator::{Navigator, Route};

/// How long typing settles before the query is asked. Answers arrive off the
/// main thread either way; this only spares the engine a round of queries
/// per keystroke from a fast typist.
const SETTLE: Duration = Duration::from_millis(90);

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
    pub subtitle: Option<SharedString>,
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

    pub fn is_empty(&self) -> bool {
        let s = &self.summary;
        s.artists.is_empty() && s.albums.is_empty() && s.tracks.is_empty()
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
        subtitle: Some(format!("{} · {}", credit(&t.artist_credit), t.album_title).into()),
        route: t.album_id.map(Route::Album),
    });
    let albums = s.albums.iter().take(SUGGESTED_ALBUMS).map(|a| Suggestion {
        kind: Kind::Album,
        id: a.id,
        album: Some(a.id),
        title: a.title.clone().into(),
        subtitle: Some(credit(&a.artist_credit).into()),
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
            subtitle: None,
            route: Some(Route::Artist(a.id)),
        });
    tracks.chain(albums).chain(artists).collect()
}

/// The field was emptied by the model, after a suggestion was taken: the
/// field's text follows.
pub struct Cleared;

pub struct Search {
    engine: Arc<KoanEngine>,
    navigator: Entity<Navigator>,
    /// What was typed, trimmed.
    query: String,
    results: Option<Rc<Results>>,
    searching: bool,
    /// Moves with every query, so an answer to an older one is dropped on
    /// landing: replacing the task cancels the wait, not the engine's read.
    generation: u64,
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
            searching: false,
            generation: 0,
            _task: None,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn results(&self) -> Option<&Rc<Results>> {
        self.results.as_ref()
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    /// The field's text changed. An empty one forgets the results page, which
    /// lands on the page searched from when it was showing.
    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.trim();
        if text == self.query {
            return;
        }
        self.query = text.to_string();
        self.generation += 1;
        if text.is_empty() {
            self.clear(cx);
            return;
        }
        self.searching = true;
        let (engine, generation, query) =
            (self.engine.clone(), self.generation, self.query.clone());
        self._task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
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
            // Shaping the suggestions' strings is the only work here, and
            // small; the engine's read ran on its own pool.
            let results = Rc::new(Results::new(query, summary));
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.results = Some(results);
                this.searching = false;
                // Moved once there is something to show: the page searched
                // from is better to look at meanwhile than an empty one.
                this.navigator.update(cx, |nav, cx| {
                    if nav.go(Route::Search) {
                        cx.notify();
                    }
                });
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self._task = None;
        self.results = None;
        self.searching = false;
        self.navigator.update(cx, |nav, cx| {
            nav.forget(Route::Search);
            cx.notify();
        });
        cx.notify();
    }

    /// Return with no suggestion lit: every result.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.query.is_empty() || self.results.is_none() {
            return;
        }
        self.navigator.update(cx, |nav, cx| {
            if nav.go(Route::Search) {
                cx.notify();
            }
        });
    }

    /// Goes where a suggestion lives, then empties the field without taking
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
        self.generation += 1;
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
    use koan_ffi::{Artist, CreditPart, Track};

    fn track(id: i64, album: Option<i64>) -> Track {
        Track {
            id,
            title: format!("t{id}"),
            artist_name: "a".into(),
            artist_credit: vec![CreditPart {
                text: "a".into(),
                artist_id: Some(1),
            }],
            album_artist_name: String::new(),
            album_artist_id: None,
            album_artist_credit: Vec::new(),
            credit_differs: false,
            album_title: "r".into(),
            album_id: album,
            artist_id: Some(1),
            disc: None,
            track_number: None,
            duration_ms: None,
            codec: None,
            sample_rate: None,
            bit_depth: None,
            channels: None,
            bitrate: None,
            genre: None,
            source: "local".into(),
            on_server: false,
            on_disk: true,
            path: None,
            is_favourite: false,
        }
    }

    fn artist(id: i64) -> Artist {
        Artist {
            id,
            name: format!("artist {id}"),
            sort_name: None,
            album_count: 0,
            track_count: 0,
        }
    }

    #[test]
    fn suggestions_take_a_few_of_each_kind_in_order() {
        let summary = ShelfSummary {
            artists: (1..=6).map(artist).collect(),
            artist_total: 6,
            albums: Vec::new(),
            album_total: 0,
            tracks: (1..=8)
                .map(|i| track(i, (i != 2).then_some(40 + i)))
                .collect(),
            track_total: 8,
        };
        let s = suggestions(&summary);
        assert_eq!(s.len(), SUGGESTED_TRACKS + SUGGESTED_ARTISTS);
        assert!(s[..5].iter().all(|s| s.kind == Kind::Track));
        assert_eq!(s[0].route, Some(Route::Album(41)));
        assert_eq!(s[0].subtitle.as_deref(), Some("a · r"));
        // A track on no record goes nowhere.
        assert_eq!(s[1].route, None);
        assert_eq!(s[5].route, Some(Route::Artist(1)));
    }

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
