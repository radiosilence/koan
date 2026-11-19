//! A control in a bar of glyphs, such as a player's shuffle, previous,
//! next and repeat: the glyph in `body`, or its word where the look hides
//! icons, 44 points wide to hit and as tall as the controls' row, but only
//! the glyph's width in the layout, so the bar sets the gaps between the
//! glyphs while the hit area reaches into them. One that toggles is `ink`
//! underlined in the accent while on and `muted` off. A press is answered
//! with a ring.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;

use super::{Answer, Answerer, Handler, Tooltip, accessible, answers_click, focus, ring, toggled};
use crate::theme::{HIT, Icon, KoanIcon, KoanStyled, Look, TypeRole, label};

/// A control in a bar of glyphs: a glyph, or its word where icons are
/// hidden, with a finger's target.
#[derive(IntoElement)]
pub struct BarControl {
    id: ElementId,
    icon: Icon,
    label: SharedString,
    on: Option<bool>,
    help: SharedString,
    answer: Answer,
    on_click: Option<Handler<ClickEvent>>,
}

impl BarControl {
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
            answer: Answer::Ring,
            on_click: None,
        }
    }

    /// What a press draws, where a ring would not say it: a skip's streak.
    pub fn answer(mut self, answer: Answer) -> Self {
        self.answer = answer;
        self
    }

    /// A control that toggles, and whether it is on.
    pub fn on(mut self, on: bool) -> Self {
        self.on = Some(on);
        self
    }

    /// What a press does.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for BarControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        let (color, underline) = match self.on {
            Some(true) => (p.ink.hsla(), Some(look.accent.hsla())),
            Some(false) => (p.muted.hsla(), None),
            None => (p.ink.hsla(), None),
        };
        // Quieter under the pointer, at once, and back over `LEAVE` once it
        // has gone, as every control's state leaves.
        let pointer = super::pointer(&super::child(&self.id, "pointer"), window, cx);
        let hovered = pointer.read(cx).hovered;
        let color = super::fade(
            super::child(&self.id, "hover"),
            hovered,
            color,
            color.opacity(0.8),
            window,
            cx,
        );
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
        // A ring leaves a glyph; drawn as a word, the control is answered
        // by a line under it, never a ring through its letters.
        let ringing = if look.icons {
            Answer::Ring
        } else {
            Answer::Sweep
        };
        // A glyph that answers through itself (shuffle, repeat) moves from
        // the state it stood in to the new one, whoever changed it: a click,
        // a shortcut, another device.
        let morphing = look.icons && super::answer::morphs(self.icon);
        if morphing {
            let mark = super::answer::glyph_mark(self.icon, self.on);
            let answer = answers
                .seen(cx)
                .and_then(|from| super::answer::glyph_motion(from, mark))
                .unwrap_or(ringing);
            answers.follow(mark, answer, look.accent.hsla(), window, cx);
        } else {
            // Shuffle or repeat turned on by a shortcut or another device
            // rings as a click does.
            match self.on {
                Some(true) => answers.follow(1, ringing, look.accent.hsla(), window, cx),
                Some(false) => answers.note(0, cx),
                None => {}
            }
        }
        let glyph_side = glyph;
        let drawn = answers.glyph_layer(glyph_side, color, None, window, cx);
        let hidden = answers.glyph_live(cx);
        // A stop in the window's Tab order, which Return and Space click.
        let handle = focus(&self.id, window, cx);
        let accent = look.accent.hsla();
        let tip = self.help.clone();
        let on = self.on;
        let answer = match self.answer {
            Answer::Ring => ringing,
            // Drawn as a word, a glyph's own motion is a line under it.
            answer if answer.glyph() && !look.icons => Answer::Sweep,
            answer => answer,
        };
        super::track(
            accessible(
                div().id(self.id.clone()),
                gpui::Role::Button,
                Some(self.help.clone()),
            ),
            &pointer,
        )
        .when_some(on, |d, on| d.aria_toggled(toggled(on)))
        .relative()
        .track_focus(&handle)
        .w(HIT)
        .mx(-reach)
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_role(TypeRole::Control, &look)
        .text_color(color)
        .tooltip(move |window, cx| Tooltip::view(tip.clone(), window, cx))
        .child(
            div()
                .relative()
                .child(div().when(hidden, |d| d.invisible()).child(face))
                .children(drawn)
                // The on-mark outside the layout: shuffle and repeat stand
                // level with previous and next.
                .children(underline.map(|accent| super::toggle::on_mark(accent, &look)))
                // Off the glyph, not the hit area around it.
                .children(answers.layer(window, cx)),
        )
        .when_some(self.on_click, move |d, on_click| {
            #[allow(clippy::arc_with_non_send_sync)] // handlers run on the main thread only
            let press: Handler<ClickEvent> = Arc::new(move |event, window, cx| {
                // A glyph that morphs answers as its state lands, not here.
                if !morphing {
                    answers.start(answer, accent, cx);
                }
                on_click(event, window, cx)
            });
            answers_click(d, press.clone())
                .on_click(move |event, window, cx| press(event, window, cx))
        })
        .children(ring(&handle, &look, window))
    }
}

/// An item in a bar's trailing group (a badge, a menu's glyph) around
/// `base`: centred across a finger's height, in `meta`,
/// reaching half `TransportMetrics::trailing_gap` either side, and a glyph
/// alone further, to `HIT`. A glyph's reach can pass its neighbour's at
/// small gaps: each item blocks the ones drawn before it, so a press in the
/// overlap goes to one item, never two.
pub fn bar_item(base: Stateful<Div>, glyph_only: bool, look: &Look) -> Stateful<Div> {
    let gap = super::TransportMetrics::trailing_gap(look) / 2.;
    let glyph = Pixels::from(super::TransportMetrics::trailing_icon(look));
    let reach = if glyph_only {
        gap.max((HIT - glyph) / 2.)
    } else {
        gap
    };
    base.flex()
        .items_center()
        .justify_center()
        .h(HIT)
        .px(reach)
        .mx(-reach)
        .occlude()
        .cursor_pointer()
        .text_role(TypeRole::Meta, look)
}
