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

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{KoanEngine, Placement, SearchKind};

use super::{Kind, replace};
use crate::components::{Button, ButtonKind, EmptyState, Segmented, TextField};
use crate::library::chrome;
use crate::library::drag::{Playable, sources};
use crate::library::format;
use crate::library::tokens::{Paint, Type, Typeset};
use crate::theme::{Icon, KoanIcon, Space, Tone};

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
    subtitle: Option<SharedString>,
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
    /// Replace Queue was asked for with a queue to lose.
    confirming: bool,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Dismissed> for PickerSheet {}

impl PickerSheet {
    pub fn new(engine: Arc<KoanEngine>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(prompt(Kind::Track)));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscriptions =
            vec![
                cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => this.schedule(cx),
                    InputEvent::PressEnter { secondary, shift } => {
                        this.commit(Commit::of(*secondary, *shift), cx)
                    }
                    _ => {}
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
            confirming: false,
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
                            subtitle: Some(
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
                                subtitle: None,
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
        cx.stop_propagation();
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

    fn escape(&mut self, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.confirming {
            self.confirming = false;
            cx.notify();
        } else {
            cx.emit(Dismissed);
        }
    }

    /// Albums and artists stand for their tracks: the engine expands them as
    /// it queues. Nothing picked means the highlighted row.
    fn commit(&mut self, commit: Commit, cx: &mut Context<Self>) {
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
        if commit == Commit::Replace && !self.confirming && replace::queued(cx) > 0 {
            self.confirming = true;
            cx.notify();
            return;
        }
        self.confirming = false;
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

    fn row(
        &self,
        p: &Paint,
        ix: usize,
        row: &Row,
        picked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let icon = if picked {
            KoanIcon::new(Icon::Picked).role(Type::Body).color(p.accent)
        } else {
            KoanIcon::new(match row.kind {
                Kind::Track => Icon::Track,
                Kind::Album => Icon::Album,
                Kind::Artist => Icon::Artist,
            })
            .role(Type::Body)
            .tone(Tone::Muted)
        };
        let (toggled, played) = (row.clone(), row.clone());
        let hover = p.row_hover();
        div()
            .id(("picker-row", ix))
            .flex()
            .items_center()
            .gap(p.space(Space::M))
            .px(p.space(Space::L))
            .py(p.space(Space::Xs))
            .cursor_pointer()
            .when(self.highlighted == Some(ix), |d| d.bg(p.surface))
            .when(self.highlighted != Some(ix), |d| {
                d.hover(move |s| s.bg(hover))
            })
            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                this.highlighted = Some(ix);
                if e.click_count() >= 2 {
                    this.picked = vec![played.clone()];
                    this.commit(Commit::AddAndPlay, cx);
                } else {
                    this.toggle(toggled.clone(), cx);
                }
            }))
            .child(div().w(p.sized(18.0)).flex_none().child(icon))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .typeset(p, Type::Body, p.ink)
                            .child(row.title.clone()),
                    )
                    .children(
                        row.subtitle
                            .clone()
                            .map(|s| div().truncate().typeset(p, Type::Fine, p.muted).child(s)),
                    ),
            )
            .children(
                row.length
                    .clone()
                    .map(|l| div().flex_none().typeset(p, Type::Meta, p.muted).child(l)),
            )
            .into_any_element()
    }

    fn heading(p: &Paint, title: &'static str) -> AnyElement {
        div()
            .px(p.space(Space::L))
            .pt(p.space(Space::M))
            .pb(p.space(Space::Xs))
            .typeset(p, Type::Fine, p.ink)
            .child(title)
            .into_any_element()
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
        let shown: Vec<Row> = self.shown().into_iter().cloned().collect();
        let picks = self.picked.len();
        let mut list: Vec<AnyElement> = Vec::with_capacity(shown.len() + 2);
        for (ix, row) in shown.iter().enumerate() {
            if picks > 0 && ix == 0 {
                list.push(Self::heading(p, "selected"));
            }
            if picks > 0 && ix == picks {
                list.push(Self::heading(p, "results"));
            }
            list.push(self.row(p, ix, row, ix < picks, cx));
        }
        let query_empty = self.input.read(cx).value().trim().is_empty();
        let body = if query_empty && picks == 0 {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
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
                .on_click(cx.listener(move |this, _, _, cx| this.commit(commit, cx)))
        };
        let foot = div()
            .flex()
            .items_center()
            .gap(p.space(Space::S))
            .px(p.space(Space::L))
            .py(p.space(Space::S))
            .border_t_1()
            .border_color(p.rule)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(p.space(Space::S))
                    .typeset(p, Type::Fine, p.muted)
                    .when(picks == 0, |d| {
                        d.child("click to select · double-click to play now")
                    })
                    .when(picks > 0, |d| {
                        d.child(format!("{picks} selected")).child(
                            chrome::text_button("picker-clear", "clear").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.picked.clear();
                                    cx.notify();
                                },
                            )),
                        )
                    }),
            )
            .child(verb(
                "picker-replace",
                ButtonKind::Standard,
                "replace queue",
                Commit::Replace,
            ))
            .child(verb("picker-add", ButtonKind::Standard, "add", Commit::Add))
            .child(verb(
                "picker-play",
                ButtonKind::Prominent,
                "add & play",
                Commit::AddAndPlay,
            ));
        let kind = KINDS.iter().position(|k| *k == self.kind).unwrap_or(0);
        let asking = self.confirming.then(|| {
            let view = cx.entity();
            let (replace_view, add, cancel) = (view.clone(), view.clone(), view);
            replace::ask(
                p,
                replace::queued(cx),
                "replace",
                Rc::new(move |_, cx| {
                    replace_view.update(cx, |this, cx| this.commit(Commit::Replace, cx))
                }),
                Rc::new(move |_, cx| add.update(cx, |this, cx| this.commit(Commit::Add, cx))),
                Rc::new(move |_, cx| {
                    cancel.update(cx, |this, cx| {
                        this.confirming = false;
                        cx.notify();
                    })
                }),
            )
        });
        div()
            .id("picker")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.dim())
            .occlude()
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.arrow(true, cx)))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.arrow(false, cx)))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| this.escape(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(Dismissed)),
            )
            .child(
                div()
                    .id("picker-sheet")
                    .relative()
                    .w(px(680.))
                    .h(relative(0.8))
                    .max_h(px(640.))
                    .flex()
                    .flex_col()
                    .bg(p.bg)
                    .border_1()
                    .border_color(p.rule)
                    .typeset(p, Type::Body, p.ink)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(p.space(Space::S))
                            .px(p.space(Space::L))
                            .py(p.space(Space::M))
                            .border_b_1()
                            .border_color(p.rule)
                            .child(
                                KoanIcon::new(Icon::Search)
                                    .role(Type::Body)
                                    .tone(Tone::Muted),
                            )
                            .child(div().flex_1().child(TextField::new(&self.input).bare())),
                    )
                    .child(
                        div()
                            .px(p.space(Space::L))
                            .py(p.space(Space::S))
                            .border_b_1()
                            .border_color(p.rule)
                            .child(
                                Segmented::new(
                                    "picker-kind",
                                    ["tracks", "albums", "artists"],
                                    kind,
                                )
                                .on_change(cx.listener(
                                    |this, ix: &usize, window, cx| {
                                        this.set_kind(KINDS[*ix], window, cx)
                                    },
                                )),
                            ),
                    )
                    .child(body)
                    .child(foot)
                    .children(asking),
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
