//! The parts a guided flow is built from: a choice between ways forward
//! (`ChoiceCard`), where the flow has got to (`Steps`), and what is in place
//! on the machine and what is not (`CheckItem`).

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use super::{
    Answer, Answerer, Handler, accessible, answers_click, child, fade, focus, hover_fill, ring,
    toggled, track,
};
use crate::theme::{
    DISABLED_OPACITY, Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label,
};

/// One way forward among several, as a block the width of its column: a
/// glyph, a title and a sentence saying what choosing it means. Chosen, its
/// outline and glyph take the accent and a tick stands at its end, so the
/// choice reads without colour too. A screen reader meets it as one of a
/// group of radio buttons.
#[derive(IntoElement)]
pub struct ChoiceCard {
    id: ElementId,
    icon: Icon,
    title: SharedString,
    detail: Option<SharedString>,
    chosen: bool,
    disabled: bool,
    place: Option<(usize, usize)>,
    on_click: Option<Handler<gpui::ClickEvent>>,
}

impl ChoiceCard {
    /// `title` is an app word, drawn in the theme's case.
    pub fn new(id: impl Into<ElementId>, icon: Icon, title: impl AsRef<str>) -> Self {
        Self {
            id: id.into(),
            icon,
            title: label(title),
            detail: None,
            chosen: false,
            disabled: false,
            place: None,
            on_click: None,
        }
    }

    /// A sentence under the title, which keeps its case.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Its place among the cards it is chosen from, from one, and how many
    /// there are: what a screen reader says as "2 of 3".
    pub fn place(mut self, index: usize, of: usize) -> Self {
        self.place = Some((index, of));
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for ChoiceCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let handle = focus(&self.id, window, cx);
        let answers = Answerer::of(&self.id, window, cx);
        let accent = look.accent.hsla();
        let edge = fade(
            child(&self.id, "edge"),
            self.chosen,
            look.palette.rule.hsla(),
            accent,
            window,
            cx,
        );
        let glyph = fade(
            child(&self.id, "glyph"),
            self.chosen,
            look.palette.muted.hsla(),
            accent,
            window,
            cx,
        );
        let (pointer, lit) = hover_fill(&self.id, &look, window, cx);
        let spoken: SharedString = match &self.detail {
            Some(detail) => format!("{}. {detail}", self.title).into(),
            None => self.title.clone(),
        };
        let mut card = accessible(
            div().id(self.id.clone()),
            gpui::Role::RadioButton,
            Some(spoken),
        )
        .aria_toggled(toggled(self.chosen))
        .aria_disabled(self.disabled)
        .when_some(self.place, |card, (index, of)| {
            card.aria_position_in_set(index).aria_size_of_set(of)
        })
        .relative()
        .w_full()
        .flex()
        .items_start()
        .gap(look.space(Space::M))
        .p(look.space(Space::L))
        .border(look.hairline)
        .border_color(edge)
        .child(lit)
        .child(
            div()
                .relative()
                .flex_none()
                .child(KoanIcon::new(self.icon).role(TypeRole::Title).color(glyph)),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(look.space(Space::Xs))
                .child(
                    div()
                        .text_role(TypeRole::Body, &look)
                        .text_color(look.palette.strong.hsla())
                        .child(self.title),
                )
                .children(self.detail.map(|detail| {
                    div()
                        .text_role(TypeRole::Meta, &look)
                        .text_color(look.palette.muted.hsla())
                        .child(detail)
                })),
        )
        .child(
            div()
                .relative()
                .flex_none()
                .size(px(crate::theme::icons::side(look.points(TypeRole::Body))))
                .when(self.chosen, |mark| {
                    mark.child(
                        KoanIcon::new(Icon::Check)
                            .role(TypeRole::Body)
                            .color(accent),
                    )
                }),
        )
        .children(answers.layer(window, cx));
        if self.disabled {
            card = card.opacity(DISABLED_OPACITY);
        } else if let Some(on_click) = self.on_click {
            #[allow(clippy::arc_with_non_send_sync)] // handlers run on the main thread only
            let press: Handler<gpui::ClickEvent> = Arc::new(move |event, window, cx| {
                answers.start(Answer::Lift, accent, cx);
                on_click(event, window, cx)
            });
            card = answers_click(track(card, &pointer), press.clone())
                .track_focus(&handle)
                .cursor_pointer()
                .on_click(move |event, window, cx| press(event, window, cx));
        }
        card.children(ring(&handle, &look, window))
    }
}

/// Where a flow of several steps has got to: each step's name in a line,
/// those done ticked in `muted`, the one showing in `strong` over the accent,
/// those to come in `muted`. It shows the way, and is not a control: the
/// flow's own buttons move along it.
#[derive(IntoElement)]
pub struct Steps {
    id: ElementId,
    names: Vec<SharedString>,
    at: usize,
}

impl Steps {
    /// `names` are app words, drawn in the theme's case; `at` is the step
    /// showing, from zero.
    pub fn new<S: AsRef<str>>(
        id: impl Into<ElementId>,
        names: impl IntoIterator<Item = S>,
        at: usize,
    ) -> Self {
        Self {
            id: id.into(),
            names: names.into_iter().map(label).collect(),
            at,
        }
    }
}

impl RenderOnce for Steps {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let count = self.names.len();
        let at = self.at.min(count.saturating_sub(1));
        let spoken = self
            .names
            .get(at)
            .map(|name| format!("step {} of {count}: {name}", at + 1));
        accessible(
            div().id(self.id.clone()),
            gpui::Role::Group,
            spoken.map(Into::into),
        )
        .flex()
        .flex_wrap()
        .items_center()
        .gap(look.space(Space::L))
        .children(self.names.into_iter().enumerate().map(|(index, name)| {
            let done = index < at;
            let here = index == at;
            let tone = if here { Tone::Strong } else { Tone::Muted };
            let color = look.text_color(tone);
            div()
                .flex()
                .items_center()
                .gap(look.space(Space::Xs))
                .pb(px(4.0))
                .border_b(look.hairline)
                .border_color(if here {
                    look.accent.hsla()
                } else {
                    look.accent.hsla().opacity(0.0)
                })
                .text_role(TypeRole::Meta, &look)
                .text_color(color)
                .when(done, |step| {
                    step.child(KoanIcon::new(Icon::Check).role(TypeRole::Meta).color(color))
                })
                .child(name)
        }))
    }
}

/// Whether something a flow needs is in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Done,
    ToDo,
    /// Not known yet: still being looked for.
    Looking,
}

/// One thing a flow needs, as a line of a checklist: a box ticked in the
/// accent when it is in place, empty in `muted` when it is not, a pending
/// glyph while it is being looked for; its title and what it is for; and the
/// verbs that put it in place at the end.
#[derive(IntoElement)]
pub struct CheckItem {
    id: ElementId,
    check: Check,
    title: SharedString,
    detail: Option<SharedString>,
    actions: Vec<AnyElement>,
}

impl CheckItem {
    /// `title` is a sentence, which keeps its case.
    pub fn new(id: impl Into<ElementId>, check: Check, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            check,
            title: title.into(),
            detail: None,
            actions: Vec::new(),
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

/// The box's side.
const BOX: f32 = 14.0;

impl RenderOnce for CheckItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let accent = look.accent.hsla();
        let state = match self.check {
            Check::Done => "done",
            Check::ToDo => "not yet",
            Check::Looking => "checking",
        };
        let spoken: SharedString = format!("{}, {state}", self.title).into();
        let mark = match self.check {
            Check::Looking => div()
                .flex_none()
                .size(px(BOX))
                .flex()
                .items_center()
                .justify_center()
                .child(KoanIcon::new(Icon::Pending).points(BOX).tone(Tone::Muted)),
            check => {
                let done = check == Check::Done;
                div()
                    .flex_none()
                    .size(px(BOX))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border(look.hairline)
                    .border_color(if done {
                        accent
                    } else {
                        look.palette.muted.hsla()
                    })
                    .when(done, |b| {
                        b.bg(accent).child(
                            KoanIcon::new(Icon::Check)
                                .points(BOX * 0.64)
                                .color(look.palette.bg.hsla()),
                        )
                    })
            }
        };
        accessible(div().id(self.id), gpui::Role::Group, Some(spoken))
            .flex()
            .items_center()
            .gap(look.space(Space::M))
            .py(look.space(Space::Xs))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_start()
                    .gap(look.space(Space::M))
                    // The box sits on the title's first line.
                    .child(div().pt(px(3.0 * look.scale.text)).child(mark))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_role(TypeRole::Body, &look)
                                    .text_color(look.palette.ink.hsla())
                                    .child(self.title),
                            )
                            .children(self.detail.map(|detail| {
                                div()
                                    .text_role(TypeRole::Meta, &look)
                                    .text_color(look.palette.muted.hsla())
                                    .child(detail)
                            })),
                    ),
            )
            .children(self.actions)
    }
}
