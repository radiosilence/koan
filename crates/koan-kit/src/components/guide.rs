//! The parts a guided flow is built from: a choice between ways forward
//! (`ChoiceCards`), where the flow has got to (`Steps`), and what is in place
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
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole, label};

/// One card of a `ChoiceCards`.
struct Card {
    icon: Icon,
    title: SharedString,
    detail: Option<SharedString>,
}

/// Ways forward to choose one of, each a block the width of the column: a
/// glyph, a title and a sentence saying what choosing it means. The one
/// chosen takes the accent in its outline and glyph, with a tick at its end
/// so the choice reads without colour too. Like `Segmented`, the group is
/// one stop for the keyboard, where the arrow keys move the choice, and a
/// radio group to a screen reader; a click on a card chooses it.
#[derive(IntoElement)]
pub struct ChoiceCards {
    id: ElementId,
    name: Option<SharedString>,
    cards: Vec<Card>,
    chosen: Option<usize>,
    on_change: Option<Handler<usize>>,
}

impl ChoiceCards {
    /// `chosen` is the card chosen, if any is yet.
    pub fn new(id: impl Into<ElementId>, chosen: Option<usize>) -> Self {
        Self {
            id: id.into(),
            name: None,
            cards: Vec::new(),
            chosen,
            on_change: None,
        }
    }

    /// What the group is called to a screen reader: the question it answers.
    pub fn named(mut self, name: impl AsRef<str>) -> Self {
        self.name = Some(label(name));
        self
    }

    /// A card: `title` is an app word, drawn in the theme's case; `detail`
    /// a sentence, which keeps its case.
    pub fn card(
        mut self,
        icon: Icon,
        title: impl AsRef<str>,
        detail: Option<impl Into<SharedString>>,
    ) -> Self {
        self.cards.push(Card {
            icon,
            title: label(title),
            detail: detail.map(Into::into),
        });
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for ChoiceCards {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let handle = focus(&self.id, window, cx);
        let accent = look.accent.hsla();
        let count = self.cards.len();
        let chosen = self.chosen;
        let mut group = accessible(
            div().id(self.id.clone()),
            gpui::Role::RadioGroup,
            self.name.clone(),
        )
        .relative()
        .w_full()
        .flex()
        .flex_col()
        .gap(look.space(Space::M));
        for (index, card) in self.cards.into_iter().enumerate() {
            let id = ElementId::NamedChild(Arc::new(self.id.clone()), format!("{index}").into());
            let on = chosen == Some(index);
            let answers = Answerer::of(&id, window, cx);
            let edge = fade(
                child(&id, "edge"),
                on,
                look.palette.rule.hsla(),
                accent,
                window,
                cx,
            );
            let glyph = fade(
                child(&id, "glyph"),
                on,
                look.palette.muted.hsla(),
                accent,
                window,
                cx,
            );
            let (pointer, lit) = hover_fill(&id, &look, window, cx);
            let spoken: SharedString = match &card.detail {
                Some(detail) => format!("{}. {detail}", card.title).into(),
                None => card.title.clone(),
            };
            let mut option =
                accessible(div().id(id.clone()), gpui::Role::RadioButton, Some(spoken))
                    .aria_toggled(toggled(on))
                    .aria_position_in_set(index + 1)
                    .aria_size_of_set(count)
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
                            .child(KoanIcon::new(card.icon).role(TypeRole::Title).color(glyph)),
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
                                    .child(card.title),
                            )
                            .children(card.detail.map(|detail| {
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
                            .when(on, |mark| {
                                mark.child(
                                    KoanIcon::new(Icon::Check)
                                        .role(TypeRole::Body)
                                        .color(accent),
                                )
                            }),
                    )
                    .children(answers.layer(window, cx));
            if let Some(on_change) = self.on_change.clone() {
                #[allow(clippy::arc_with_non_send_sync)] // handlers run on the main thread only
                let choose: Handler<gpui::ClickEvent> = {
                    let handle = handle.clone();
                    Arc::new(move |_, window, cx| {
                        answers.start(Answer::Lift, accent, cx);
                        // The group holds the keyboard after a click, as a
                        // segmented control does, so the arrows go on from it.
                        window.focus(&handle, cx);
                        on_change(&index, window, cx)
                    })
                };
                option = answers_click(track(option, &pointer), choose.clone())
                    .cursor_pointer()
                    .on_click(move |event, window, cx| choose(event, window, cx));
            }
            group = group.child(option);
        }
        if let Some(on_change) = self.on_change {
            group = group
                .track_focus(&handle)
                .on_key_down(move |event, window, cx| {
                    let next = match (event.keystroke.key.as_str(), chosen) {
                        ("up" | "left", Some(at)) if at > 0 => at - 1,
                        ("down" | "right", Some(at)) if at + 1 < count => at + 1,
                        ("up" | "left" | "down" | "right", None) if count > 0 => 0,
                        _ => return,
                    };
                    cx.stop_propagation();
                    on_change(&next, window, cx);
                });
        }
        group.children(ring(&handle, &look, window))
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::accesskit::Action;
    use gpui::{IntoElement, TestAppContext, VisualTestContext};

    use super::*;
    use crate::components::testing::{act, click, node, press, redraw, stage, tree};

    fn music(cx: &mut TestAppContext, chosen: Rc<Cell<Option<usize>>>) -> &mut VisualTestContext {
        let (_, cx) = stage(cx, move |_, _| {
            let chosen = chosen.clone();
            vec![(
                "music",
                ChoiceCards::new("music", chosen.get())
                    .named("where is your music")
                    .card(Icon::Laptop, "On this computer", Some("Files here."))
                    .card(Icon::Server, "On a server", None::<&str>)
                    .on_change(move |index, _, _| chosen.set(Some(*index)))
                    .into_any_element(),
            )]
        });
        cx
    }

    #[gpui::test]
    fn a_radio_group_of_cards_one_toggled(cx: &mut TestAppContext) {
        let cx = music(cx, Rc::new(Cell::new(Some(1))));
        assert!(tree(cx).to_string().contains("RadioGroup"));
        for (name, at, on) in [
            ("on this computer. Files here.", 1, "False"),
            ("on a server", 2, "True"),
        ] {
            let (_, card) = node(cx, name);
            assert_eq!(card["role"], "RadioButton");
            assert_eq!(card["toggled"], on, "{name}");
            assert_eq!(card["position_in_set"], at, "{name}");
            assert_eq!(card["size_of_set"], 2, "{name}");
        }
    }

    #[gpui::test]
    fn a_click_chooses_and_the_arrows_move_on(cx: &mut TestAppContext) {
        let chosen = Rc::new(Cell::new(None));
        let cx = music(cx, chosen.clone());
        act(cx, "on a server", Action::Click);
        assert_eq!(chosen.get(), Some(1));
        redraw(cx);
        press(cx, "up");
        assert_eq!(chosen.get(), Some(0));
        redraw(cx);
        press(cx, "up");
        assert_eq!(chosen.get(), Some(0), "the first card stays chosen");
        click(cx, "music");
        assert!(chosen.get().is_some());
    }

    #[gpui::test]
    fn a_check_says_its_state(cx: &mut TestAppContext) {
        let (_, cx) = stage(cx, |_, _| {
            vec![
                (
                    "installed",
                    CheckItem::new("installed", Check::Done, "Tailscale is installed")
                        .into_any_element(),
                ),
                (
                    "running",
                    CheckItem::new("running", Check::ToDo, "Tailscale is running")
                        .into_any_element(),
                ),
                (
                    "looking",
                    CheckItem::new("looking", Check::Looking, "Funnel is allowed")
                        .into_any_element(),
                ),
            ]
        });
        node(cx, "Tailscale is installed, done");
        node(cx, "Tailscale is running, not yet");
        node(cx, "Funnel is allowed, checking");
    }

    #[gpui::test]
    fn steps_say_where_the_flow_is(cx: &mut TestAppContext) {
        let (_, cx) = stage(cx, |_, _| {
            vec![(
                "steps",
                Steps::new("steps", ["welcome", "your music", "playback"], 1).into_any_element(),
            )]
        });
        node(cx, "step 2 of 3: your music");
    }
}
