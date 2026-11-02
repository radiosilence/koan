//! A transport control (shuffle, previous, next, repeat): the glyph in
//! `body`, or its word where the look hides icons, 44 points to hit but only
//! the glyph's width in the layout, so the controls sit as far apart as the
//! Mac's while the hit area reaches into the gaps. One that toggles is `ink`
//! underlined in the accent while on and `muted` off. A press is answered
//! with a ring.

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Answer, Answerer, Handler, Tooltip};
use crate::theme::{HIT, Icon, KoanIcon, KoanStyled, Look, TypeRole, label};

#[derive(IntoElement)]
pub struct TransportControl {
    id: ElementId,
    icon: Icon,
    label: SharedString,
    on: Option<bool>,
    help: SharedString,
    on_click: Option<Handler<ClickEvent>>,
}

impl TransportControl {
    /// `text` is its word where the look hides icons; `help` what its
    /// tooltip says, its shortcut included.
    pub fn new(
        id: impl Into<ElementId>,
        icon: Icon,
        text: impl AsRef<str>,
        help: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            icon,
            label: label(text),
            on: None,
            help: help.into(),
            on_click: None,
        }
    }

    /// A control that toggles, and whether it is on.
    pub fn on(mut self, on: bool) -> Self {
        self.on = Some(on);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for TransportControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        let (color, underline) = match self.on {
            Some(true) => (p.ink.hsla(), Some(look.accent.hsla())),
            Some(false) => (p.muted.hsla(), None),
            None => (p.ink.hsla(), None),
        };
        let glyph = px(crate::theme::icons::side(look.points(TypeRole::Body)));
        let reach = (HIT - glyph) / 2.;
        let face = if look.icons {
            KoanIcon::new(self.icon)
                .role(TypeRole::Body)
                .color(color)
                .into_any_element()
        } else {
            div().child(self.label.clone()).into_any_element()
        };
        let answers = Answerer::of(&self.id, window, cx);
        let accent = look.accent.hsla();
        let tip = self.help.clone();
        div()
            .id(self.id.clone())
            .relative()
            .w(HIT)
            .mx(-reach)
            .h(HIT)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_role(TypeRole::Control, &look)
            .text_color(color)
            .hover(|s| s.opacity(0.8))
            .tooltip(move |window, cx| Tooltip::view(tip.clone(), window, cx))
            .child(
                div()
                    .relative()
                    .when_some(underline, |d, accent| d.border_b_1().border_color(accent))
                    .child(face)
                    // Off the glyph, not the hit area around it.
                    .children(answers.layer(window, cx)),
            )
            .when_some(self.on_click, move |d, on_click| {
                d.on_click(move |event, window, cx| {
                    answers.start(Answer::Ring, accent, cx);
                    on_click(event, window, cx)
                })
            })
    }
}
