//! ⇧⌘K: the terminal UI's track, record and artist pickers as one sheet.
//!
//! Two things carry over on purpose. Several things can be picked before
//! committing, and committing is three verbs: add, add and play, or replace
//! the queue. A palette that only ever plays one track is a worse tool for
//! building a queue, which is what the app is for.
//!
//! Tracks are found through the library's full-text search and records and
//! artists through fuzzy matching, each keystroke answered off the main
//! thread and an answer to an older query dropped. The keys stay in the
//! field: ↑ and ↓ move the highlight, Return adds, ⌘Return adds and plays,
//! ⇧⌘Return replaces the queue (asking first when that takes anything off),
//! Escape closes.

use std::sync::Arc;
use std::time::Duration;

use koan_ffi::{KoanEngine, Placement, SearchKind};
use koan_kit::app::*;
// gpui-component's text input: koan-kit's FilterField draws it, the sheet owns its state.
use koan_kit::raw::component::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
// the list's scroll state, which the arrows move to the highlighted row.
use koan_kit::raw::ScrollHandle;

use super::Kind;
use crate::components::sheet as ask;
use crate::components::{
    Button, ButtonKind, EmptyState, Segmented, SuggestionHeading, SuggestionRow,
};
use crate::library::chrome;
use crate::library::drag::{Playable, sources};
use crate::library::format;
use crate::paint::Paint;
use crate::theme::Icon;

const SETTLE: Duration = Duration::from_millis(90);
const TRACKS: u32 = 60;
const OTHERS: u32 = 40;
const KINDS: [Kind; 3] = [Kind::Track, Kind::Album, Kind::Artist];

/// The sheet asks to be closed: Escape, a click outside it, or a commit.
pub struct Dismissed;

#[derive(Clone, Debug, PartialEq)]
struct Row {
    id: i64,
    kind: Kind,
    title: SharedString,
    detail: Option<SharedString>,
    length: Option<SharedString>,
}

impl Row {
    fn playable(&self) -> Playable {
        match self.kind {
            Kind::Track => Playable::Track(self.id),
            Kind::Album => Playable::Album(self.id),
            Kind::Artist => Playable::Artist(self.id),
        }
    }

    fn same(&self, other: &Row) -> bool {
        self.kind == other.kind && self.id == other.id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Commit {
    Add,
    AddAndPlay,
    Replace,
}

impl Commit {
    fn placement(self) -> Placement {
        match self {
            Commit::Add => Placement::Append,
            // Plays the first thing just added rather than the queue's head.
            Commit::AddAndPlay => Placement::AppendAndPlay,
            Commit::Replace => Placement::Replace {
                start_at: None,
                shuffled: false,
            },
        }
    }

    /// What Return means with these modifiers.
    fn of(secondary: bool, shift: bool) -> Self {
        match (secondary, shift) {
            (true, true) => Commit::Replace,
            (true, false) => Commit::AddAndPlay,
            _ => Commit::Add,
        }
    }
}

pub struct PickerSheet {
    engine: Arc<KoanEngine>,
    input: Entity<InputState>,
    kind: Kind,
    results: Vec<Row>,
    /// Picked, in the order they were picked; listed first.
    picked: Vec<Row>,
    /// The row the arrows are on, in the order rows are listed.
    highlighted: Option<usize>,
    generation: u64,
    _search: Option<Task<()>>,
    /// The engine has the command and the sheet is waiting to close.
    resolving: bool,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Dismissed> for PickerSheet {}

impl Focusable for PickerSheet {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl PickerSheet {
    pub fn new(engine: Arc<KoanEngine>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(prompt(Kind::Track)));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscriptions =
            vec![
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => this.schedule(cx),
                        InputEvent::PressEnter { secondary, shift } => {
                            this.commit(Commit::of(*secondary, *shift), window, cx)
                        }
                        _ => {}
                    }
                }),
            ];
        Self {
            engine,
            input,
            kind: Kind::Track,
            results: Vec::new(),
            picked: Vec::new(),
            highlighted: None,
            generation: 0,
            _search: None,
            resolving: false,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Every row as listed: the picks, then what the query found that is not
    /// picked.
    fn shown(&self) -> Vec<&Row> {
        self.picked
            .iter()
            .chain(
                self.results
                    .iter()
                    .filter(|r| !self.picked.iter().any(|p| p.same(r))),
            )
            .collect()
    }

    fn set_kind(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        if kind == self.kind {
            return;
        }
        self.kind = kind;
        self.input.update(cx, |input, cx| {
            input.set_placeholder(prompt(kind), window, cx)
        });
        self.schedule(cx);
    }

    fn schedule(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.highlighted = None;
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            self._search = None;
            self.results.clear();
            cx.notify();
            return;
        }
        let (engine, kind, generation) = (self.engine.clone(), self.kind, self.generation);
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let found = match kind {
                Kind::Track => engine.search(text, TRACKS).await.map(|tracks| {
                    tracks
                        .into_iter()
                        .map(|t| Row {
                            id: t.id,
                            kind,
                            detail: Some(
                                format!(
                                    "{} · {}",
                                    t.artist_credit
                                        .iter()
                                        .map(|c| c.text.as_str())
                                        .collect::<String>(),
                                    t.album_title
                                )
                                .into(),
                            ),
                            length: Some(format::duration(t.duration_ms).into()),
                            title: t.title.into(),
                        })
                        .collect::<Vec<_>>()
                }),
                Kind::Album | Kind::Artist => {
                    let ffi = if kind == Kind::Album {
                        SearchKind::Album
                    } else {
                        SearchKind::Artist
                    };
                    engine.fuzzy_search(text, ffi, OTHERS).await.map(|found| {
                        found
                            .into_iter()
                            .map(|m| Row {
                                id: m.id,
                                kind,
                                title: m.name.into(),
                                detail: None,
                                length: None,
                            })
                            .collect()
                    })
                }
            };
            let found = match found {
                Ok(found) => found,
                Err(e) => {
                    log::warn!("picker: {e}");
                    return;
                }
            };
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.results = found;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn toggle(&mut self, row: Row, cx: &mut Context<Self>) {
        match self.picked.iter().position(|p| p.same(&row)) {
            Some(i) => {
                self.picked.remove(i);
            }
            None => self.picked.push(row),
        }
        cx.notify();
    }

    fn arrow(&mut self, down: bool, cx: &mut Context<Self>) {
        let count = self.shown().len();
        if count == 0 {
            return;
        }
        self.highlighted = Some(match (self.highlighted, down) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(count - 1),
            (Some(i), false) => i.saturating_sub(1),
        });
        if let Some(i) = self.highlighted {
            self.scroll.scroll_to_item(self.child_index(i));
        }
        cx.notify();
    }

    /// Where row `i` sits among the list's children, headings included.
    fn child_index(&self, i: usize) -> usize {
        if self.picked.is_empty() {
            i
        } else if i < self.picked.len() {
            i + 1
        } else {
            i + 2
        }
    }

    /// Albums and artists stand for their tracks: the engine expands them as
    /// it queues. Nothing picked means the highlighted row. Replacing a
    /// queue with anything in it says how much goes first.
    fn commit(&mut self, commit: Commit, window: &mut Window, cx: &mut Context<Self>) {
        if self.resolving {
            return;
        }
        let rows: Vec<Playable> = if self.picked.is_empty() {
            self.highlighted
                .and_then(|i| self.shown().get(i).map(|r| r.playable()))
                .into_iter()
                .collect()
        } else {
            self.picked.iter().map(Row::playable).collect()
        };
        if rows.is_empty() {
            return;
        }
        let queued = cx
            .global::<crate::mirror::Mirror>()
            .queue
            .read(cx)
            .items
            .len();
        if commit == Commit::Replace && queued > 0 {
            let view = cx.weak_entity();
            ask::confirm(
                ask::Confirm {
                    title: "replace the queue".into(),
                    verb: "replace".into(),
                    destructive: true,
                    lines: vec![
                        format!(
                            "{} come off the queue, and what you picked takes its place.",
                            chrome::count(queued as u64, "track", "tracks")
                        )
                        .into(),
                    ],
                    note: Some("Add keeps the queue and puts these after it.".into()),
                },
                window,
                cx,
                move |cx| {
                    view.update(cx, |this, cx| this.send(rows.clone(), commit, cx))
                        .ok();
                },
            );
            return;
        }
        self.send(rows, commit, cx);
    }

    fn send(&mut self, rows: Vec<Playable>, commit: Commit, cx: &mut Context<Self>) {
        self.resolving = true;
        cx.notify();
        let (engine, sets) = (self.engine.clone(), sources(&rows));
        cx.spawn(async move |this, cx| {
            if let Err(e) = engine.queue(sets, commit.placement()).await {
                log::warn!("picker: {e}");
            }
            this.update(cx, |_, cx| cx.emit(Dismissed)).ok();
        })
        .detach();
    }

    fn row(&self, ix: usize, row: &Row, picked: bool, cx: &mut Context<Self>) -> SuggestionRow {
        let mut line = SuggestionRow::new(("picker-row", ix), row.title.clone())
            .icon(match row.kind {
                Kind::Track => Icon::Track,
                Kind::Album => Icon::Album,
                Kind::Artist => Icon::Artist,
            })
            .picked(picked)
            .lit(self.highlighted == Some(ix));
        if let Some(detail) = &row.detail {
            line = line.detail(detail.clone());
        }
        if let Some(length) = &row.length {
            line = line.figure(length.clone());
        }
        let (toggled, played) = (row.clone(), row.clone());
        line.on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
            this.highlighted = Some(ix);
            if e.click_count() >= 2 {
                this.picked = vec![played.clone()];
                this.commit(Commit::AddAndPlay, window, cx);
            } else {
                this.toggle(toggled.clone(), cx);
            }
        }))
    }
}

fn prompt(kind: Kind) -> &'static str {
    match kind {
        Kind::Track => "search tracks",
        Kind::Album => "search albums",
        Kind::Artist => "search artists",
    }
}

impl Render for PickerSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let look = &p.look;
        let shown: Vec<Row> = self.shown().into_iter().cloned().collect();
        let picks = self.picked.len();
        let mut list: Vec<AnyElement> = Vec::with_capacity(shown.len() + 2);
        for (ix, row) in shown.iter().enumerate() {
            if picks > 0 && ix == 0 {
                list.push(SuggestionHeading::new("selected").into_any_element());
            }
            if picks > 0 && ix == picks {
                list.push(SuggestionHeading::new("results").into_any_element());
            }
            list.push(self.row(ix, row, ix < picks, cx).into_any_element());
        }
        let query_empty = self.input.read(cx).value().trim().is_empty();
        let body = if query_empty && picks == 0 {
            div()
                .flex_1()
                .child(EmptyState::new(Icon::Search, "search your library"))
                .into_any_element()
        } else {
            div()
                .id("picker-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .children(list)
                .into_any_element()
        };
        let idle = self.resolving || (picks == 0 && self.highlighted.is_none());
        let verb = |id: &'static str, kind: ButtonKind, label: &'static str, commit: Commit| {
            Button::new(id, kind)
                .label(label)
                .disabled(idle)
                .on_click(cx.listener(move |this, _, window, cx| this.commit(commit, window, cx)))
                .into_any_element()
        };
        let status: Vec<AnyElement> = if picks == 0 {
            vec!["click to select · double-click to play now".into_any_element()]
        } else {
            vec![
                format!("{picks} selected").into_any_element(),
                chrome::text_button("picker-clear", "clear")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.picked.clear();
                        cx.notify();
                    }))
                    .into_any_element(),
            ]
        };
        let actions = vec![
            verb(
                "picker-replace",
                ButtonKind::Standard,
                "replace queue",
                Commit::Replace,
            ),
            verb("picker-add", ButtonKind::Standard, "add", Commit::Add),
            verb(
                "picker-play",
                ButtonKind::Prominent,
                "add & play",
                Commit::AddAndPlay,
            ),
        ];
        let kind = KINDS.iter().position(|k| *k == self.kind).unwrap_or(0);
        ask::scrim("picker", look)
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.arrow(true, cx)))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.arrow(false, cx)))
            .capture_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(Dismissed)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(Dismissed)),
            )
            .child(
                ask::list_panel(look)
                    .id("picker-sheet")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(ask::search_head(&self.input, look))
                    .child(ask::band(
                        Segmented::new("picker-kind", ["tracks", "albums", "artists"], kind)
                            .on_change(cx.listener(|this, ix: &usize, window, cx| {
                                this.set_kind(KINDS[*ix], window, cx)
                            })),
                        look,
                    ))
                    .child(body)
                    .child(ask::foot(status, actions, look)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn return_means_add_and_its_modifiers_play_or_replace() {
        assert_eq!(Commit::of(false, false), Commit::Add);
        assert_eq!(Commit::of(false, true), Commit::Add);
        assert_eq!(Commit::of(true, false), Commit::AddAndPlay);
        assert_eq!(Commit::of(true, true), Commit::Replace);
    }
}
