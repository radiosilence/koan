//! The sidebar's search field, and its suggestions under it. Taking the
//! keyboard, it opens the search box over the window (`search::bar`) on the
//! same query. The field keeps
//! the keyboard while the suggestions follow what is typed: the arrows light
//! one, Return takes the lit one or, with none lit, opens every result, and
//! Escape empties the field (koan-kit's `FilterField`). ⌘K and `/` reach it
//! through the shell.

use std::rc::Rc;

use koan_kit::app::input::{InputEvent, InputState, MoveDown, MoveUp};
use koan_kit::app::*;

use super::{Cleared, Kind, Results, Search, Suggestion, step};
use crate::components::{
    Cover, CoverArt, FilterField, RowMetrics, SuggestionHeading, SuggestionPanel, SuggestionRow,
};
use crate::library::covers::{Art, Covers, Key, Priority, UPLOAD_BYTES_PER_FRAME};
use crate::paint::Paint;
use crate::theme::{Icon, Space};

pub struct SearchField {
    search: Entity<Search>,
    input: Entity<InputState>,
    covers: Entity<Covers>,
    /// The suggestion the arrows or the pointer lit.
    lit: Option<usize>,
    focused: bool,
    /// The pointer is over the panel: a click there takes the keyboard from
    /// the field, and the panel stays for the click to land.
    pointing: bool,
    /// The answer whose sleeves were last asked for.
    aimed: Option<Rc<Results>>,
    _subscriptions: Vec<Subscription>,
}

impl SearchField {
    pub fn new(
        search: Entity<Search>,
        covers: Entity<Covers>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("search"));
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => {
                            this.lit = None;
                            let text = input.read(cx).value().to_string();
                            this.search.update(cx, |s, cx| s.set_query(&text, cx));
                        }
                        InputEvent::PressEnter { .. } => this.enter(cx),
                        // The sidebar's field is a way into the search box over
                        // the window, which takes the keyboard with the query.
                        InputEvent::Focus => {
                            this.focused = true;
                            window.dispatch_action(
                                crate::platform::actions::Search.boxed_clone(),
                                cx,
                            );
                        }
                        InputEvent::Blur => this.focused = false,
                    }
                    cx.notify();
                },
            ),
            cx.subscribe_in(&search, window, |this, _, _: &Cleared, window, cx| {
                this.input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                this.lit = None;
                this.pointing = false;
                cx.notify();
            }),
            // A new answer redraws the panel, and the lit line starts again.
            cx.observe(&search, |this, _, cx| {
                this.lit = None;
                cx.notify();
            }),
            // A sleeve landing redraws the field only while the panel shows.
            cx.observe(&covers, |this, _, cx| {
                if this.panel(cx).is_some() {
                    cx.notify();
                }
            }),
        ];
        Self {
            search,
            input,
            covers,
            lit: None,
            focused: false,
            pointing: false,
            aimed: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// The answer the panel shows, while it shows.
    fn panel(&self, cx: &App) -> Option<Rc<Results>> {
        if !(self.focused || self.pointing) {
            return None;
        }
        let search = self.search.read(cx);
        let results = search.results()?;
        (!search.query().is_empty() && !results.suggestions.is_empty()).then(|| results.clone())
    }

    fn enter(&mut self, cx: &mut Context<Self>) {
        let lit = self
            .lit
            .zip(self.panel(cx))
            .and_then(|(i, r)| r.suggestions.get(i).cloned());
        match lit {
            Some(suggestion) => self.take(&suggestion, cx),
            None => self.search.update(cx, |s, cx| s.submit(cx)),
        }
    }

    fn take(&mut self, suggestion: &Suggestion, cx: &mut Context<Self>) {
        self.pointing = false;
        self.search.update(cx, |s, cx| s.take(suggestion, cx));
    }

    fn arrow(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(results) = self.panel(cx) else {
            cx.propagate();
            return;
        };
        self.lit = step(self.lit, down, results.suggestions.len());
        cx.notify();
    }

    /// Asks for the sleeves of a new answer, all at once: the panel holds a
    /// dozen lines at most.
    fn aim(&mut self, results: &Rc<Results>, p: &Paint, window: &Window, cx: &mut Context<Self>) {
        if self.aimed.as_ref().is_some_and(|a| Rc::ptr_eq(a, results)) {
            return;
        }
        self.aimed = Some(results.clone());
        let (side, scale) = (RowMetrics::sleeve(&p.look), window.scale_factor());
        let ranked: Vec<(Key, Priority)> = results
            .suggestions
            .iter()
            .filter_map(|s| s.album)
            .map(|album| (Key::new(Art::Album(album), side, scale), Priority::Visible))
            .collect();
        let owner = cx.entity_id();
        self.covers.update(cx, |c, _| c.request(owner, ranked));
    }

    fn line(
        &mut self,
        p: &Paint,
        ix: usize,
        suggestion: &Suggestion,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> SuggestionRow {
        let mut row = SuggestionRow::new(("suggestion", ix), suggestion.title.clone())
            .lit(self.lit == Some(ix));
        if let Some(detail) = &suggestion.detail {
            row = row.detail(detail.clone());
        }
        row = match (suggestion.kind, suggestion.album) {
            (Kind::Artist, _) | (_, None) => row.icon(Icon::Artist),
            (_, Some(album)) => {
                let side = RowMetrics::sleeve(&p.look);
                let key = Key::new(Art::Album(album), side, window.scale_factor());
                let sleeve = self.covers.update(cx, |c, _| c.sleeve(key));
                row.art(Cover::new(CoverArt::from(sleeve), side))
            }
        };
        let taken = suggestion.clone();
        row.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered && this.lit != Some(ix) {
                this.lit = Some(ix);
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |this, _, _, cx| this.take(&taken, cx)))
    }
}

impl Render for SearchField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let panel = self.panel(cx).map(|results| {
            self.aim(&results, p, window, cx);
            if self.covers.update(cx, |c, cx| {
                c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
            }) {
                window.request_animation_frame();
            }
            let mut panel = SuggestionPanel::new("suggestions").on_hover(cx.listener(
                |this, hovered: &bool, _, cx| {
                    this.pointing = *hovered;
                    if !hovered {
                        this.lit = None;
                    }
                    cx.notify();
                },
            ));
            for (ix, suggestion) in results.suggestions.iter().enumerate() {
                if ix == 0 || results.suggestions[ix - 1].kind != suggestion.kind {
                    panel = panel.child(SuggestionHeading::new(suggestion.kind.heading()));
                }
                panel = panel.child(self.line(p, ix, suggestion, window, cx));
            }
            panel
        });
        div()
            .relative()
            .w_full()
            .px(p.space(Space::L))
            .py(p.space(Space::S))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.arrow(true, cx)))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.arrow(false, cx)))
            .child(FilterField::new(&self.input).fill())
            .children(panel)
    }
}
