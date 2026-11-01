//! The sidebar's search field, and its suggestions in a square panel under
//! it, laid over the sidebar's rows. The field keeps the keyboard while the
//! suggestions follow what is typed: the arrows light one, Return takes the
//! one lit or, with none lit, opens every result, and Escape empties the
//! field. `/` and ⌘K reach it through the shell.

use std::rc::Rc;

use gpui_kit::component::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Cleared, Kind, Results, Search, Suggestion, step};
use crate::components::TextField;
use crate::library::covers::{Art, Covers, Key, Priority, Sleeve, UPLOAD_BYTES_PER_FRAME};
use crate::library::tokens::{Paint, Type, Typeset};
use crate::theme::{Icon, KoanIcon, Space, Tone};

const SLEEVE: Pixels = px(28.);

pub struct SearchField {
    search: Entity<Search>,
    input: Entity<InputState>,
    /// The suggestion the arrows or the pointer lit.
    lit: Option<usize>,
    focused: bool,
    /// The pointer is over the panel: a click there may take the keyboard
    /// from the field, and the panel stays for the click to land.
    pointing: bool,
    /// The library's artwork, once the library is open.
    covers: Option<Entity<Covers>>,
    /// The answer whose sleeves were last asked for.
    aimed: Option<Rc<Results>>,
    _subscriptions: Vec<Subscription>,
}

impl SearchField {
    pub fn new(search: Entity<Search>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("search"));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                match event {
                    InputEvent::Change => {
                        this.lit = None;
                        let text = input.read(cx).value().to_string();
                        this.search.update(cx, |s, cx| s.set_query(&text, cx));
                    }
                    InputEvent::PressEnter { .. } => this.enter(cx),
                    InputEvent::Focus => this.focused = true,
                    InputEvent::Blur => this.focused = false,
                }
                cx.notify();
            }),
            cx.subscribe_in(&search, window, |this, _, _: &Cleared, window, cx| {
                this.input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                this.lit = None;
                this.pointing = false;
                cx.notify();
            }),
            // A new answer redraws the panel; the lit row starts again.
            cx.observe(&search, |this, _, cx| {
                this.lit = None;
                cx.notify();
            }),
        ];
        Self {
            search,
            input,
            lit: None,
            focused: false,
            pointing: false,
            covers: None,
            aimed: None,
            _subscriptions: subscriptions,
        }
    }

    /// The library's artwork, for the suggestions' sleeves. A sleeve landing
    /// redraws the field only while the panel shows.
    pub fn set_covers(&mut self, covers: Entity<Covers>, cx: &mut Context<Self>) {
        self._subscriptions.push(cx.observe(&covers, |this, _, cx| {
            if this.panel(cx).is_some() {
                cx.notify();
            }
        }));
        self.covers = Some(covers);
        cx.notify();
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
            return;
        };
        self.lit = step(self.lit, down, results.suggestions.len());
        cx.stop_propagation();
        cx.notify();
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.read(cx).value().is_empty() {
            return;
        }
        cx.stop_propagation();
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.lit = None;
        self.search.update(cx, |s, cx| s.set_query("", cx));
        cx.notify();
    }

    /// Asks for the sleeves of a new answer, all of them at once: the panel
    /// holds a dozen lines at most.
    fn aim(&mut self, results: &Rc<Results>, window: &Window, cx: &mut Context<Self>) {
        if self.aimed.as_ref().is_some_and(|a| Rc::ptr_eq(a, results)) {
            return;
        }
        let Some(covers) = &self.covers else { return };
        self.aimed = Some(results.clone());
        let scale = window.scale_factor();
        let ranked: Vec<(Key, Priority)> = results
            .suggestions
            .iter()
            .filter_map(|s| s.album)
            .map(|album| {
                (
                    Key::new(Art::Album(album), SLEEVE, scale),
                    Priority::Visible,
                )
            })
            .collect();
        let owner = cx.entity_id();
        covers.update(cx, |c, _| c.request(owner, ranked));
    }

    fn line(
        &mut self,
        p: &Paint,
        ix: usize,
        suggestion: &Suggestion,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lit = self.lit == Some(ix);
        let scale = window.scale_factor();
        let sleeve = match (suggestion.album, &self.covers) {
            (Some(album), Some(covers)) => {
                let key = Key::new(Art::Album(album), SLEEVE, scale);
                Some(covers.update(cx, |c, _| c.sleeve(key)))
            }
            _ => None,
        };
        let art = match (suggestion.kind, sleeve) {
            (Kind::Artist, _) => div()
                .size(SLEEVE)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    KoanIcon::new(Icon::Artist)
                        .role(Type::Meta)
                        .tone(Tone::Muted),
                ),
            (_, Some(Sleeve::Ready(image))) => div()
                .size(SLEEVE)
                .child(img(ImageSource::Render(image)).size_full()),
            (_, Some(Sleeve::Colour(colour))) => div().size(SLEEVE).bg(colour),
            _ => div().size(SLEEVE).bg(p.surface),
        };
        let taken = suggestion.clone();
        div()
            .id(("suggestion", ix))
            .flex()
            .items_center()
            .gap(p.space(Space::S))
            .px(p.space(Space::S))
            .py(p.space(Space::Xs))
            .cursor_pointer()
            .when(lit, |d| d.bg(p.surface))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.lit != Some(ix) {
                    this.lit = Some(ix);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.take(&taken, cx)))
            .child(art.flex_none())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .typeset(p, Type::Meta, p.ink)
                            .child(suggestion.title.clone()),
                    )
                    .children(suggestion.subtitle.clone().map(|subtitle| {
                        div()
                            .truncate()
                            .typeset(p, Type::Fine, p.muted)
                            .child(subtitle)
                    })),
            )
            .into_any_element()
    }
}

impl Render for SearchField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &Paint::of(window, cx);
        let panel = self.panel(cx).map(|results| {
            self.aim(&results, window, cx);
            if let Some(covers) = &self.covers
                && covers.update(cx, |c, cx| {
                    c.admit(UPLOAD_BYTES_PER_FRAME, Some(window), cx)
                })
            {
                window.request_animation_frame();
            }
            let mut rows: Vec<AnyElement> = Vec::new();
            for (ix, suggestion) in results.suggestions.iter().enumerate() {
                if ix == 0 || results.suggestions[ix - 1].kind != suggestion.kind {
                    rows.push(
                        div()
                            .px(p.space(Space::S))
                            .pt(p.space(Space::S))
                            .pb(p.space(Space::Xs))
                            .typeset(p, Type::Fine, p.muted)
                            .child(suggestion.kind.heading())
                            .into_any_element(),
                    );
                }
                rows.push(self.line(p, ix, suggestion, window, cx));
            }
            deferred(
                div()
                    .id("suggestions")
                    .absolute()
                    .top(relative(1.))
                    .left(p.space(Space::S))
                    .right(p.space(Space::S))
                    .max_h(px(560.))
                    .overflow_y_scroll()
                    .occlude()
                    .bg(p.bg)
                    .border_1()
                    .border_color(p.rule)
                    .pb(p.space(Space::Xs))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.pointing = *hovered;
                        if !hovered {
                            this.lit = None;
                        }
                        cx.notify();
                    }))
                    .children(rows),
            )
            .with_priority(1)
        });
        div()
            .relative()
            .w_full()
            .px(p.space(Space::S))
            .py(p.space(Space::S))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| this.arrow(true, cx)))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| this.arrow(false, cx)))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| this.escape(window, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(p.space(Space::Xs))
                    .child(
                        KoanIcon::new(Icon::Search)
                            .role(Type::Control)
                            .tone(Tone::Muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(TextField::new(&self.input).bare()),
                    ),
            )
            .children(panel)
    }
}
