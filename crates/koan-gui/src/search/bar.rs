//! The search box centred over the window (koan-kit's `CommandBar`): ⌘K,
//! `/`, ⇧⌘K and the sidebar's field all open it. It searches as it is typed
//! into, through the same `Search` the sidebar and the results page read,
//! and does everything from the keyboard:
//!
//! | Key | Does |
//! |---|---|
//! | ↑ ↓ | light a row, or "All results…" after the last |
//! | Return | with no row lit, or "All results…", every result on the results page; on a lit row, go to it: a record or an artist to its page, a track to its record's, picked |
//! | ⌘Return | play next |
//! | ⌥Return | add to the queue |
//! | ⇧⌘Return | replace the queue, asking first when it holds anything |
//! | Tab | tick the lit row; the verbs then act on everything ticked |
//! | ⇧Return | every result, on the results page |
//! | Escape | clear the ticks, then empty the query, then close |
//!
//! Ticked rows are listed first, whatever the query, so a pick built over
//! several queries stays in view. The answers go deeper while the box is
//! open (`Search::set_deep`), and the keys act on rows only once they answer
//! the query as typed, so a fast Return never acts on the last query's
//! answer.
//!
//! The field and the results are views of their own (`QueryField`,
//! `Found`), drawn cached, as a library page's filter field is: a keystroke
//! draws the field alone, whatever the results hold, and the results are
//! drawn when an answer lands, a sleeve arrives or the light moves.

use std::rc::Rc;

use koan_kit::app::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
use koan_kit::app::*;
use koan_runtime::{Placement, ShelfSummary};

use super::{Kind, Results, Search, step};
use crate::components::{CommandBar, CommandResults, Cover, CoverArt, SuggestionRow};
use crate::library::Lib;
use crate::library::covers::{Art, Key, Priority, UPLOAD_BYTES_PER_FRAME};
use crate::library::drag::Playable;
use crate::library::nav::Page;
use crate::paint::Paint;
use crate::platform::actions::{FocusNext, FocusPrevious, NextRegion, PreviousRegion};
use crate::theme::Icon;

/// The box asks to be closed: Escape on an empty query, a click outside, or
/// a verb that went somewhere.
pub struct Dismissed;

/// One row: a thing found, its strings made once per answer.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    kind: Kind,
    id: i64,
    /// The sleeve the row shows: its record's, or the artist's photo.
    art: Option<Art>,
    title: SharedString,
    detail: Option<SharedString>,
    length: Option<SharedString>,
}

impl Item {
    fn playable(&self) -> Playable {
        match self.kind {
            Kind::Track => Playable::Track(self.id),
            Kind::Album => Playable::Album(self.id),
            Kind::Artist => Playable::Artist(self.id),
        }
    }

    fn same(&self, other: &Item) -> bool {
        self.kind == other.kind && self.id == other.id
    }
}

fn credit(parts: &[koan_runtime::CreditPart]) -> String {
    parts.iter().map(|p| p.text.as_str()).collect()
}

/// How many of each kind the box lists: enough to recognise what was meant.
/// Everything else is behind Return, or "All results…", on the results
/// page; the list scrolls inside the panel when the rows outgrow it.
const TRACKS: usize = 5;
const ALBUMS: usize = 5;
const ARTISTS: usize = 5;

/// The search's answer as rows, in the order they are listed: tracks,
/// records, artists, each kind capped.
fn items(s: &ShelfSummary) -> Vec<Item> {
    let tracks = s.tracks.iter().take(TRACKS).map(|t| Item {
        kind: Kind::Track,
        id: t.id,
        art: t.album_id.map(Art::Album),
        title: t.title.clone().into(),
        detail: Some(format!("{} · {}", credit(&t.artist_credit), t.album_title).into()),
        length: Some(crate::library::format::duration(t.duration_ms).into()),
    });
    let albums = s.albums.iter().take(ALBUMS).map(|a| Item {
        kind: Kind::Album,
        id: a.id,
        art: Some(Art::Album(a.id)),
        title: a.title.clone().into(),
        detail: Some(credit(&a.artist_credit).into()),
        length: None,
    });
    let artists = s.artists.iter().take(ARTISTS).map(|a| Item {
        kind: Kind::Artist,
        id: a.id,
        art: Some(Art::Artist(a.id)),
        title: a.name.clone().into(),
        detail: None,
        length: None,
    });
    tracks.chain(albums).chain(artists).collect()
}

/// Every row as listed: `ticked` first, then what was found that is not
/// ticked.
fn listed(ticked: &[Item], found: &[Item]) -> Vec<Item> {
    ticked
        .iter()
        .cloned()
        .chain(
            found
                .iter()
                .filter(|f| !ticked.iter().any(|t| t.same(f)))
                .cloned(),
        )
        .collect()
}

/// Where `item` is listed in `rows` now, to keep the light on it.
fn relit(item: Option<&Item>, rows: &[Item]) -> Option<usize> {
    let item = item?;
    rows.iter().position(|r| r.same(item))
}

/// What a key asks of the lit row, or of everything ticked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verb {
    /// Play a track, open a record or an artist.
    Take,
    Next,
    Append,
    /// In place of the queue.
    Replace,
    /// Every result, on their page.
    All,
}

impl Verb {
    fn of(secondary: bool, alt: bool, shift: bool) -> Self {
        match (secondary, alt, shift) {
            (true, _, true) => Verb::Replace,
            (_, _, true) => Verb::All,
            (true, _, _) => Verb::Next,
            (_, true, _) => Verb::Append,
            _ => Verb::Take,
        }
    }
}

pub struct SearchBar {
    input: Entity<InputState>,
    field: Entity<QueryField>,
    found: Entity<Found>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Dismissed> for SearchBar {}

impl Focusable for SearchBar {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl SearchBar {
    pub fn new(lib: Lib, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = lib.search.read(cx).query().to_string();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("search everything");
            state.set_value(query, window, cx);
            state
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let field = cx.new(|cx| QueryField {
            input: input.clone(),
            // Cached, the field draws again only when its own view hears
            // the input change, not the input alone.
            _redraw: cx.observe(&input, |_, _, cx| cx.notify()),
        });
        let found = cx.new(|cx| Found::new(lib, input.clone(), cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        this.found.update(cx, |f, cx| f.typed(&text, cx));
                    }
                    InputEvent::PressEnter { secondary, shift } => {
                        let verb = Verb::of(*secondary, false, *shift);
                        this.found.update(cx, |f, cx| f.act(verb, window, cx));
                    }
                    _ => {}
                },
            ),
            cx.subscribe(&found, |_, _, _: &Dismissed, cx| cx.emit(Dismissed)),
        ];
        Self {
            input,
            field,
            found,
            _subscriptions: subscriptions,
        }
    }
}

/// The query's field, a view of its own so a keystroke draws nothing else.
pub struct QueryField {
    input: Entity<InputState>,
    _redraw: Subscription,
}

impl Render for QueryField {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Laid out on its own, as wide as the room it was given.
        div().size_full().child(CommandBar::field(&self.input))
    }
}

/// What the query found, as rows, and what the keys do to them: drawn when
/// an answer lands, a sleeve arrives, the light moves or a row is ticked.
pub struct Found {
    lib: Lib,
    search: Entity<Search>,
    input: Entity<InputState>,
    /// The answer the found rows were made from, and the rows.
    shown: Option<(Rc<Results>, Rc<Vec<Item>>)>,
    /// What is drawn: the rows of the last answer to land, kept while the
    /// next is read, so typing never empties the list.
    drawn: Rc<Vec<Item>>,
    /// Whether the last answer was to the query as typed, and whether
    /// anything is typed: together, an empty answer is "nothing found".
    answered: bool,
    typed: bool,
    lit: Option<usize>,
    /// Ticked, in the order ticked.
    ticked: Vec<Item>,
    /// The sleeves last asked for.
    aimed: Option<Rc<Vec<Item>>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Dismissed> for Found {}

impl Found {
    fn new(lib: Lib, input: Entity<InputState>, cx: &mut Context<Self>) -> Self {
        let search = lib.search.clone();
        let owner = cx.entity_id();
        let subscriptions = vec![
            // A new answer, to this query or another, keeps the lit row on
            // the thing it lit where that is still listed.
            cx.observe(&search, |this, _, cx| {
                let was = this.lit_item();
                this.landed(cx);
                let rows = listed(&this.ticked, &this.drawn);
                this.lit = relit(was.as_ref(), &rows);
                cx.notify();
            }),
            cx.observe(&lib.covers, |_, _, cx| cx.notify()),
            // Closed: the search goes back to the sidebar's depth, and the
            // sleeves asked for are let go.
            cx.on_release(move |this, cx| {
                this.search.update(cx, |s, cx| s.set_deep(false, cx));
                this.lib.covers.update(cx, |c, cx| c.release(owner, cx));
            }),
        ];
        search.update(cx, |s, cx| s.set_deep(true, cx));
        let mut found = Self {
            lib,
            search,
            input,
            shown: None,
            drawn: Rc::new(Vec::new()),
            answered: false,
            typed: false,
            lit: None,
            ticked: Vec::new(),
            aimed: None,
            _subscriptions: subscriptions,
        };
        found.landed(cx);
        found
    }

    /// The query as typed: asked of the search, which answers off the main
    /// thread; the light goes out, and nothing is drawn until the answer.
    fn typed(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.lit.take().is_some() {
            cx.notify();
        }
        self.search.update(cx, |s, cx| s.set_query(text, cx));
    }

    /// The search answered, or was cleared: what is drawn follows.
    fn landed(&mut self, cx: &App) {
        let query_empty = self.search.read(cx).query().is_empty();
        self.typed = !query_empty;
        self.answered = self.answered(cx);
        if self.answered || query_empty {
            self.drawn = self.found(cx);
        }
    }

    /// What was found for the query as typed, made again only when the
    /// answer changed; nothing until an answer to this very query has come.
    fn found(&mut self, cx: &App) -> Rc<Vec<Item>> {
        let search = self.search.read(cx);
        let Some(results) = search
            .results()
            .filter(|r| !search.query().is_empty() && r.query == search.query())
        else {
            return Rc::new(Vec::new());
        };
        if let Some((shown, rows)) = &self.shown
            && Rc::ptr_eq(shown, results)
        {
            return rows.clone();
        }
        let rows = Rc::new(items(&results.summary));
        self.shown = Some((results.clone(), rows.clone()));
        rows
    }

    fn rows(&mut self, cx: &App) -> Vec<Item> {
        let found = self.found(cx);
        listed(&self.ticked, &found)
    }

    /// The thing lit, from the rows as drawn.
    fn lit_item(&self) -> Option<Item> {
        self.lit
            .and_then(|i| listed(&self.ticked, &self.drawn).get(i).cloned())
    }

    /// Whether the search has answered the query as typed: until it has,
    /// an empty list is not "nothing found".
    fn answered(&self, cx: &App) -> bool {
        let search = self.search.read(cx);
        search.results().is_some_and(|r| r.query == search.query())
    }

    fn arrow(&mut self, down: bool, cx: &mut Context<Self>) {
        let rows = self.rows(cx).len();
        // The rows, then "All results…".
        let count = if rows == 0 { 0 } else { rows + 1 };
        self.lit = step(self.lit, down, count);
        cx.notify();
    }

    /// Tab: the lit row in or out of the ticks.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        let Some(item) = self.lit.and_then(|i| rows.get(i)).cloned() else {
            return;
        };
        match self.ticked.iter().position(|t| t.same(&item)) {
            Some(i) => {
                self.ticked.remove(i);
            }
            None => self.ticked.push(item.clone()),
        }
        // Ticked rows lead the list, so the row moves: the light goes with it.
        let rows = self.rows(cx);
        self.lit = relit(Some(&item), &rows);
        cx.notify();
    }

    fn act(&mut self, verb: Verb, window: &mut Window, cx: &mut Context<Self>) {
        if verb == Verb::All {
            self.search.update(cx, |s, cx| s.submit(cx));
            cx.emit(Dismissed);
            return;
        }
        let rows = self.rows(cx);
        let picking = !self.ticked.is_empty();
        // Return with no row lit and nothing ticked is the query itself:
        // every result, as the Mac's search field submits.
        let all = self.lit == Some(rows.len());
        if all || (verb == Verb::Take && !picking && self.lit.is_none()) {
            if !self.input.read(cx).value().trim().is_empty() {
                self.search.update(cx, |s, cx| s.submit(cx));
                cx.emit(Dismissed);
            }
            return;
        }
        let targets: Vec<Item> = if !picking {
            self.lit
                .and_then(|i| rows.get(i))
                .cloned()
                .into_iter()
                .collect()
        } else {
            self.ticked.clone()
        };
        let Some(first) = targets.first().cloned() else {
            return;
        };
        let items: Vec<Playable> = targets.iter().map(Item::playable).collect();
        let lib = self.lib.clone();
        let queue = move |placement: Placement, cx: &mut App| {
            crate::library::actions::queue(&lib, &items, placement, cx).detach()
        };
        match verb {
            // Choosing goes to the thing and plays nothing: a record or an
            // artist to its page, a track to its record's with the track
            // picked.
            Verb::Take if !picking => {
                let (page, pick) = match first.kind {
                    Kind::Album => (Page::Album(first.id), None),
                    Kind::Artist => (Page::Artist(first.id), None),
                    Kind::Track => match first.art {
                        Some(Art::Album(album)) => (Page::Album(album), Some((album, first.id))),
                        _ => return,
                    },
                };
                // The pick is announced whether or not the page moves: a
                // record already showing takes it at once.
                self.lib.nav.update(cx, |nav, cx| {
                    nav.pick = pick;
                    nav.open(page, cx);
                    cx.notify();
                });
            }
            // Return chooses; a pick is played only by a verb that says so.
            Verb::Take => return,
            Verb::Next => queue(Placement::Next, cx),
            Verb::Append => queue(Placement::Append, cx),
            Verb::Replace => {
                let replace = move |cx: &mut App| {
                    queue(
                        Placement::Replace {
                            start_at: None,
                            shuffled: false,
                        },
                        cx,
                    )
                };
                let queued = cx
                    .global::<crate::mirror::Mirror>()
                    .queue
                    .read(cx)
                    .items
                    .len();
                if queued > 0 {
                    // Asked over the window once the box is gone and the
                    // shell has given the keyboard back, so the question
                    // returns it there.
                    cx.emit(Dismissed);
                    window.defer(cx, move |window, cx| {
                        crate::components::sheet::confirm(
                            crate::components::sheet::Confirm {
                                title: "replace the queue".into(),
                                verb: "replace".into(),
                                destructive: true,
                                lines: vec![
                                format!(
                                    "{} come off the queue, and what you picked takes its place.",
                                    crate::library::chrome::count(queued as u64, "track", "tracks")
                                )
                                .into(),
                            ],
                                note: Some(
                                    "Add to queue keeps the queue and puts these after it.".into(),
                                ),
                            },
                            window,
                            cx,
                            replace,
                        );
                    });
                    return;
                }
                replace(cx);
            }
            Verb::All => {}
        }
        cx.emit(Dismissed);
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ticked.is_empty() {
            self.ticked.clear();
            cx.notify();
        } else if self.input.read(cx).value().is_empty() {
            cx.emit(Dismissed);
        } else {
            self.input.update(cx, |input, cx| input.clean(window, cx));
        }
    }

    /// Asks for every sleeve of a new answer, all at once: the box holds a
    /// hundred rows at most.
    fn aim(
        &mut self,
        found: &Rc<Vec<Item>>,
        side: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.aimed.as_ref().is_some_and(|a| Rc::ptr_eq(a, found)) {
            return;
        }
        self.aimed = Some(found.clone());
        let scale = window.scale_factor();
        let ranked: Vec<(Key, Priority)> = found
            .iter()
            .filter_map(|r| r.art)
            .map(|art| (Key::new(art, side, scale), Priority::Visible))
            .collect();
        let owner = cx.entity_id();
        self.lib
            .covers
            .update(cx, |c, cx| c.request(owner, ranked, cx));
    }
}

impl Render for Found {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let side = SuggestionRow::sleeve(&p.look, true);
        let found = self.drawn.clone();
        self.aim(&found, side, window, cx);
        if self.lib.covers.update(cx, |c, cx| {
            c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
        }) {
            window.request_animation_frame();
        }
        let rows = listed(&self.ticked, &found);
        let ticks = self.ticked.len();
        let picking = ticks > 0;
        let scale = window.scale_factor();
        // The ticked first under a heading of their own, then what was
        // found, by kind.
        let mut sections: Vec<(Option<Kind>, Vec<SuggestionRow>)> = Vec::new();
        for (ix, item) in rows.iter().enumerate() {
            let mut row = SuggestionRow::new(("search-bar-row", ix), item.title.clone()).tall();
            if let Some(detail) = &item.detail {
                row = row.detail(detail.clone());
            }
            if let Some(length) = &item.length {
                row = row.figure(length.clone());
            }
            if picking {
                row = row.picked(ix < ticks).icon(match item.kind {
                    Kind::Track => Icon::Track,
                    Kind::Album => Icon::Album,
                    Kind::Artist => Icon::Artist,
                });
            } else {
                row = match item.art {
                    Some(art) => {
                        let key = Key::new(art, side, scale);
                        let sleeve = self.lib.covers.update(cx, |c, _| c.sleeve(key));
                        let row = row.art(Cover::new(CoverArt::from(sleeve), side));
                        if matches!(art, Art::Artist(_)) {
                            row.round()
                        } else {
                            row
                        }
                    }
                    None => row.icon(Icon::Track),
                };
            }
            let row = row
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    // A row left over from the last answer is not the query's.
                    if *hovered && this.lit != Some(ix) && ix < this.rows(cx).len() {
                        this.lit = Some(ix);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    if ix >= this.rows(cx).len() {
                        return;
                    }
                    this.lit = Some(ix);
                    let m = e.modifiers();
                    if m.secondary(cx) && !m.shift {
                        this.tick(cx);
                    } else {
                        this.act(Verb::Take, window, cx);
                    }
                }));
            let group = (ix >= ticks).then_some(item.kind);
            match sections.last_mut() {
                Some((kind, list)) if *kind == group => list.push(row),
                _ => sections.push((group, vec![row])),
            }
        }
        let mut bar = CommandResults::new("search-bar-results").lit(self.lit);
        for (kind, list) in sections {
            bar = bar.section(kind.map_or("ticked", Kind::heading), list);
        }
        // After the last row: every result, on the results page.
        let all = rows.len();
        bar = bar.last(
            SuggestionRow::new("search-bar-all", "All results…")
                .tall()
                .icon(Icon::Search)
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered && this.lit != Some(all) {
                        this.lit = Some(all);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.lit = Some(all);
                    this.act(Verb::All, window, cx);
                })),
        );
        if self.typed && self.answered {
            bar = bar.empty("nothing found");
        }
        // What the keys do: with nothing lit or ticked, Return is the query
        // and the arrows choose a row; on a row, or a pick, the verbs act on
        // it. With nothing yet found, only Escape does anything.
        let on_all = self.lit == Some(rows.len());
        let chosen = picking || (self.lit.is_some() && !on_all);
        let bar = if rows.is_empty() {
            bar
        } else if !chosen {
            bar.key("↩", "all results").key("↑↓", "choose")
        } else {
            let bar = if picking {
                bar
            } else {
                bar.key("↩", "go to")
            };
            bar.key("⌘↩", "play next")
                .key("⌥↩", "add to queue")
                .key("⇧⌘↩", "replace the queue")
                .key("tab", "tick")
                .key("⇧↩", "all results")
        };
        bar.key(
            "esc",
            if picking {
                format!("clear {ticks} ticked")
            } else {
                "close".to_owned()
            },
        )
    }
}

impl Render for SearchBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A cached view is as big as its style says, whatever it draws: the
        // field one line of its type, the results the room under the query.
        let line = CommandBar::field_height(window, cx);
        let bar = CommandBar::new(
            "search-bar",
            self.field
                .clone()
                .cached(StyleRefinement::default().w_full().h(line)),
        )
        .results(
            self.found
                .clone()
                .cached(StyleRefinement::default().w_full().flex_1().min_h_0()),
        )
        .on_dismiss(cx.listener(|_, _, _, cx| cx.emit(Dismissed)));
        div()
            .absolute()
            .inset_0()
            .key_context("SearchBar")
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                this.found.update(cx, |f, cx| f.arrow(true, cx))
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.found.update(cx, |f, cx| f.arrow(false, cx))
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                this.found.update(cx, |f, cx| f.escape(window, cx))
            }))
            .capture_action(
                cx.listener(|this, _: &FocusNext, _, cx| this.found.update(cx, |f, cx| f.tick(cx))),
            )
            .capture_action(cx.listener(|this, _: &FocusPrevious, _, cx| {
                this.found.update(cx, |f, cx| f.tick(cx))
            }))
            // The box holds the keyboard while it is up: F6 does not take
            // it to the page behind.
            .capture_action(cx.listener(|_, _: &NextRegion, _, _| {}))
            .capture_action(cx.listener(|_, _: &PreviousRegion, _, _| {}))
            // The field binds neither ⌥Return nor ⇧⌘Return, so they are
            // taken here.
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                let m = &k.modifiers;
                if k.key == "enter" && (m.alt || (m.secondary(cx) && m.shift)) {
                    cx.stop_propagation();
                    let verb = Verb::of(m.secondary(cx), m.alt, m.shift);
                    this.found.update(cx, |f, cx| f.act(verb, window, cx));
                }
            }))
            .child(bar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn item(kind: Kind, id: i64) -> Item {
        Item {
            kind,
            id,
            art: None,
            title: SharedString::default(),
            detail: None,
            length: None,
        }
    }

    #[test]
    fn the_verbs_follow_the_modifiers() {
        assert_eq!(Verb::of(false, false, false), Verb::Take);
        assert_eq!(Verb::of(true, false, false), Verb::Next);
        assert_eq!(Verb::of(false, true, false), Verb::Append);
        assert_eq!(Verb::of(true, false, true), Verb::Replace);
        assert_eq!(Verb::of(true, true, true), Verb::Replace);
        assert_eq!(Verb::of(false, false, true), Verb::All);
    }

    /// The light follows its row when ticking moves it to the top, and when
    /// a new answer lists it elsewhere; it goes out when the row is gone.
    #[test]
    fn the_light_stays_on_its_row() {
        let (a, b, c) = (
            item(Kind::Track, 1),
            item(Kind::Track, 2),
            item(Kind::Album, 3),
        );
        let found = [a.clone(), b.clone(), c.clone()];
        let ticked = [b.clone()];
        assert_eq!(relit(Some(&b), &listed(&ticked, &found)), Some(0));
        assert_eq!(
            relit(Some(&c), &listed(&[], &[c.clone(), a.clone()])),
            Some(0)
        );
        assert_eq!(relit(Some(&c), &listed(&[], &[a, b])), None);
        assert_eq!(relit(None, &found), None);
    }

    /// A pick stays listed whatever the query, first and once.
    #[test]
    fn ticked_rows_lead_and_are_not_listed_twice() {
        let ticked = [item(Kind::Album, 3)];
        let found = [item(Kind::Track, 1), item(Kind::Album, 3)];
        let rows = listed(&ticked, &found);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].same(&ticked[0]));
        assert!(rows[1].same(&found[0]));
    }
}
