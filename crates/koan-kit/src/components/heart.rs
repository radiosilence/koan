//! The heart on anything that can be favourited, the Mac's `FavouriteButton`:
//! the outline off, filled in the accent on. Its own click answers it: six
//! small dots leave a heart that fills, and one ring closes in on a heart
//! that empties. A change made elsewhere (a shortcut, another device)
//! answers alike. A heart its owner hides at rest (a row's, shown under the
//! pointer) shows while it answers, so a heart emptied elsewhere is seen to
//! close; one its owner does not draw at all at rest follows its favourite
//! through `answering`. The owner says what a click
//! does; a heart without one is a mark, with no tooltip or pointer, inside
//! a control of its owner's (an album tile). Where the heart shows (always,
//! or only under the pointer) is its styling, set as on a `div`.

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Answer, Answerer, Handler, accessible, answers_click, toggled};
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Tone, TypeRole};

#[derive(IntoElement)]
pub struct Heart {
    base: Stateful<Div>,
    id: ElementId,
    on: bool,
    role: TypeRole,
    rest: Tone,
    help: Option<SharedString>,
    on_click: Option<Handler<ClickEvent>>,
}

/// The heart `id`'s answers, following `on`: a change answers.
fn follow(id: &ElementId, on: bool, window: &mut Window, cx: &mut App) -> Answerer {
    let accent = Look::of(window, cx).accent.hsla();
    let answers = Answerer::of(id, window, cx);
    answers.follow(
        on as u64,
        if on { Answer::Burst } else { Answer::Close },
        accent,
        window,
        cx,
    );
    answers
}

impl Heart {
    /// For a heart its owner does not draw at rest (a tile's, shown under
    /// the pointer or for a favourite): follows `on` while it is not drawn,
    /// and says whether it is answering a change, for which the owner draws
    /// it until the answer has run.
    pub fn answering(
        id: impl Into<ElementId>,
        on: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        follow(&id.into(), on, window, cx).live(cx)
    }

    pub fn new(id: impl Into<ElementId>, on: bool) -> Self {
        let id = id.into();
        Self {
            base: div().id(id.clone()),
            id,
            on,
            role: TypeRole::Meta,
            rest: Tone::Muted,
            help: None,
            on_click: None,
        }
    }

    /// The type the glyph is sized to.
    pub fn role(mut self, role: TypeRole) -> Self {
        self.role = role;
        self
    }

    /// The outline's tone while off: `muted` in a list, `ink` over a sleeve.
    pub fn rest(mut self, tone: Tone) -> Self {
        self.rest = tone;
        self
    }

    /// What its tooltip says, where it says more than "favourite" or
    /// "remove from favourites": the shortcut, on the playing track's heart.
    pub fn help(mut self, help: impl Into<SharedString>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// What a click does: the owner toggles the favourite. The click stops
    /// here, so a heart on a row does not also act on the row.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Arc::new(handler));
        self
    }
}

impl Styled for Heart {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for Heart {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for Heart {}

impl RenderOnce for Heart {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let accent = look.accent.hsla();
        // The accent where it reads as text against the ground, `ink`
        // where it does not: a heart is text-sized.
        let color = if self.on {
            look.text_color(Tone::Accent)
        } else {
            look.color(self.rest)
        };
        let glyph = if look.icons {
            KoanIcon::new(if self.on {
                Icon::Favourited
            } else {
                Icon::Favourite
            })
            .role(self.role)
            .color(color)
            .into_any_element()
        } else {
            // The look hides icons: the heart as a character.
            div()
                .text_role(self.role, &look)
                .text_color(color)
                .child(if self.on { "\u{2665}" } else { "\u{2661}" })
                .into_any_element()
        };
        let on = self.on;
        let answers = follow(&self.id, on, window, cx);
        let live = answers.live(cx);
        let tip = self.help.clone().unwrap_or_else(|| {
            crate::theme::label(if on {
                "remove from favourites"
            } else {
                "favourite"
            })
        });
        // A screen reader hears a toggle named for what it does, whether it
        // is on, and can press it as the pointer does.
        let name = tip.clone();
        accessible(self.base, gpui_kit::Role::Button, Some(name))
            .aria_toggled(toggled(on))
            // Shown while it answers, whatever its owner hides it by.
            .when(live, |d| d.visible())
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .when(self.on_click.is_some(), |d| {
                d.cursor_pointer()
                    .tooltip(move |window, cx| super::Tooltip::view(tip.clone(), window, cx))
            })
            .child(
                div()
                    .relative()
                    .child(glyph)
                    .children(answers.layer(window, cx)),
            )
            .when_some(self.on_click, move |d, handler| {
                let press: Handler<ClickEvent> = Arc::new(move |event, window, cx| {
                    let answer = if on { Answer::Close } else { Answer::Burst };
                    answers.start(answer, accent, cx);
                    handler(event, window, cx)
                });
                answers_click(d, press.clone()).on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    press(event, window, cx)
                })
            })
    }
}
