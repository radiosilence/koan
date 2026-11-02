//! The heart on anything that can be favourited, the Mac's `FavouriteButton`:
//! the outline off, filled in the accent on. Its own click answers it: six
//! small dots leave a heart that fills, and one ring closes in on a heart
//! that empties. The owner says what a click does; where the heart shows
//! (always, or only under the pointer) is its styling, set as on a `div`.

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Answer, Answerer, Handler};
use crate::theme::{Icon, KoanIcon, Look, Tone, TypeRole};

#[derive(IntoElement)]
pub struct Heart {
    base: Stateful<Div>,
    id: ElementId,
    on: bool,
    role: TypeRole,
    rest: Tone,
    on_click: Option<Handler<ClickEvent>>,
}

impl Heart {
    pub fn new(id: impl Into<ElementId>, on: bool) -> Self {
        let id = id.into();
        Self {
            base: div().id(id.clone()),
            id,
            on,
            role: TypeRole::Meta,
            rest: Tone::Muted,
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
        let glyph = if self.on {
            KoanIcon::new(Icon::Favourited)
                .role(self.role)
                .color(accent)
        } else {
            KoanIcon::new(Icon::Favourite)
                .role(self.role)
                .color(look.color(self.rest))
        };
        let on = self.on;
        let answers = Answerer::of(&self.id, window, cx);
        self.base
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(
                div()
                    .relative()
                    .child(glyph)
                    .children(answers.layer(window, cx)),
            )
            .when_some(self.on_click, move |d, handler| {
                d.on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    let answer = if on { Answer::Close } else { Answer::Burst };
                    answers.start(answer, accent, cx);
                    handler(event, window, cx)
                })
            })
    }
}
