//! The search box centred over the window (koan-kit's `CommandBar`): ⌘K,
//! `/`, ⇧⌘K and the sidebar's field all open it. It searches as it is typed
//! into, through the same `Search` the sidebar and the results page read,
//! and does everything from the keyboard:
//!
//! | Key | Does |
//! |---|---|
//! | ↑ ↓ | light a row |
//! | Return | play a track (after the queue's end), open a record or an artist |
//! | ⌘Return | play next |
//! | ⌥Return | add to the queue |
//! | ⇧⌘Return | replace the queue, asking first when it holds anything |
//! | Tab | tick the lit row; the verbs then act on everything ticked |
//! | ⇧Return | every result, on the results page |
//! | Escape | clear the ticks, then empty the query, then close |
//!
//! Ticked rows are listed first, whatever the query, so a pick built over
//! several queries stays in view. The answers go deeper while the box is
//! open (`Search::set_deep`), and rows are shown only once they answer the
//! query as typed, so a fast Return never acts on the last query's answer.
//! It replaces the ⇧⌘K picker.

use std::rc::Rc;

use koan_ffi::{Placement, ShelfSummary};
use koan_kit::app::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
use koan_kit::app::*;

use super::{Kind, Results, Search, step};
use crate::components::{CommandBar, Cover, CoverArt, RowMetrics, SuggestionRow};
use crate::library::Lib;
use crate::library::covers::{Art, Key, Priority, UPLOAD_BYTES_PER_FRAME};
use crate::library::drag::Playable;
use crate::library::nav::Page;
use crate::paint::Paint;
use crate::platform::actions::FocusNext;
use crate::theme::Icon;

/// The box asks to be closed: Escape on an empty query, a click outside, or
/// a verb that went somewhere.
pub struct Dismissed;

/// One row: a thing found, its strings made once per answer.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    kind: Kind,
    id: i64,
    /// The record whose sleeve the row shows.
    album: Option<i64>,
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

fn credit(parts: &[koan_ffi::CreditPart]) -> String {
    parts.iter().map(|p| p.text.as_str()).collect()
}

/// The search's answer as rows, in the order they are listed: tracks,
/// records, artists.
fn items(s: &ShelfSummary) -> Vec<Item> {
    let tracks = s.tracks.iter().map(|t| Item {
        kind: Kind::Track,
        id: t.id,
        album: t.album_id,
        title: t.title.clone().into(),
        detail: Some(format!("{} · {}", credit(&t.artist_credit), t.album_title).into()),
        length: Some(crate::library::format::duration(t.duration_ms).into()),
    });
    let albums = s.albums.iter().map(|a| Item {
        kind: Kind::Album,
        id: a.id,
        album: Some(a.id),
        title: a.title.clone().into(),
        detail: Some(credit(&a.artist_credit).into()),
        length: None,
    });
    let artists = s.artists.iter().map(|a| Item {
        kind: Kind::Artist,
        id: a.id,
        album: None,
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
    lib: Lib,
    search: Entity<Search>,
    input: Entity<InputState>,
    /// The answer the found rows were made from, and the rows.
    shown: Option<(Rc<Results>, Rc<Vec<Item>>)>,
    lit: Option<usize>,
    /// Ticked, in the order ticked.
    ticked: Vec<Item>,
    /// The sleeves last asked for.
    aimed: Option<Rc<Vec<Item>>>,
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
        let search = lib.search.clone();
        let owner = cx.entity_id();
        let query = search.read(cx).query().to_string();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("search everything");
            state.set_value(query, window, cx);
            state
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.lit = None;
                        let text = input.read(cx).value().to_string();
                        this.search.update(cx, |s, cx| s.set_query(&text, cx));
                    }
                    InputEvent::PressEnter { secondary, shift } => {
                        this.act(Verb::of(*secondary, false, *shift), window, cx);
                    }
                    _ => {}
                },
            ),
            cx.observe(&search, |this, _, cx| {
                this.lit = None;
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
        Self {
            lib,
            search,
            input,
            shown: None,
            lit: None,
            ticked: Vec::new(),
            aimed: None,
            _subscriptions: subscriptions,
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

    fn arrow(&mut self, down: bool, cx: &mut Context<Self>) {
        let count = self.rows(cx).len();
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
            None => self.ticked.push(item),
        }
        cx.notify();
    }

    fn act(&mut self, verb: Verb, window: &mut Window, cx: &mut Context<Self>) {
        if verb == Verb::All {
            self.search.update(cx, |s, cx| s.submit(cx));
            cx.emit(Dismissed);
            return;
        }
        let rows = self.rows(cx);
        let targets: Vec<Item> = if self.ticked.is_empty() {
            self.lit
                .or((!rows.is_empty()).then_some(0))
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
            // One record or artist goes to its page; tracks, or a pick,
            // play after the queue's end.
            Verb::Take if targets.len() == 1 && first.kind != Kind::Track => {
                let page = match first.kind {
                    Kind::Album => Page::Album(first.id),
                    _ => Page::Artist(first.id),
                };
                self.lib.nav.update(cx, |nav, cx| nav.open(page, cx));
            }
            Verb::Take => queue(Placement::AppendAndPlay, cx),
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
            .filter_map(|r| r.album)
            .map(|album| (Key::new(Art::Album(album), side, scale), Priority::Visible))
            .collect();
        let owner = cx.entity_id();
        self.lib.covers.update(cx, |c, _| c.request(owner, ranked));
    }
}

impl Render for SearchBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let side = RowMetrics::sleeve(&p.look);
        let found = self.found(cx);
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
            let mut row = SuggestionRow::new(("search-bar-row", ix), item.title.clone());
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
                row = match item.album {
                    Some(album) => {
                        let key = Key::new(Art::Album(album), side, scale);
                        let sleeve = self.lib.covers.update(cx, |c, _| c.sleeve(key));
                        row.art(Cover::new(CoverArt::from(sleeve), side))
                    }
                    None => row.icon(Icon::Artist),
                };
            }
            let row = row
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered && this.lit != Some(ix) {
                        this.lit = Some(ix);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
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
        let mut bar = CommandBar::new("search-bar", &self.input).lit(self.lit);
        for (kind, list) in sections {
            bar = bar.section(kind.map_or("ticked", Kind::heading), list);
        }
        if !self.input.read(cx).value().trim().is_empty() {
            bar = bar.empty("nothing found");
        }
        let bar = bar
            .key(
                "↩",
                if picking {
                    "play these"
                } else {
                    "play or open"
                },
            )
            .key("⌘↩", "play next")
            .key("⌥↩", "add to queue")
            .key("⇧⌘↩", "replace the queue")
            .key("tab", "tick")
            .key("⇧↩", "all results")
            .key(
                "esc",
                if picking {
                    format!("clear {ticks} ticked")
                } else {
                    "close".to_owned()
                },
            )
            .on_dismiss(cx.listener(|_, _, _, cx| cx.emit(Dismissed)));
        div()
            .absolute()
            .inset_0()
            .key_context("SearchBar")
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.arrow(true, cx)))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.arrow(false, cx)))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| this.escape(window, cx)))
            .capture_action(cx.listener(|this, _: &FocusNext, _, cx| this.tick(cx)))
            // The field binds no ⌥Return, so it is taken here.
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                if k.key == "enter" && k.modifiers.alt {
                    cx.stop_propagation();
                    let verb = Verb::of(k.modifiers.secondary(cx), true, k.modifiers.shift);
                    this.act(verb, window, cx);
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
            album: None,
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
        assert_eq!(Verb::of(false, false, true), Verb::All);
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
