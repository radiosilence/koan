use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::{
    Answer, Answerer, Handler, accessible, answers_click, child, fade, focus, ring, toggled,
};
use crate::theme::{
    DISABLED_OPACITY, HAIRLINE, HIT, Icon, KoanIcon, KoanStyled, Look, Motion, Space, TypeRole,
    label,
};

/// A square box at the row's trailing edge, the label leading: a `muted`
/// outline off, filled with the accent and ticked in `bg` on.
#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    label: SharedString,
    checked: bool,
    disabled: bool,
    on_change: Option<Handler<bool>>,
}

impl Toggle {
    pub fn new(id: impl Into<ElementId>, text: impl AsRef<str>, checked: bool) -> Self {
        Self {
            id: id.into(),
            label: label(text),
            checked,
            disabled: false,
            on_change: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }
}

/// The box's side.
const BOX: f32 = 14.0;

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let handle = focus(&self.id, window, cx);
        let answers = Answerer::of(&self.id, window, cx);
        let on = fade(
            child(&self.id, "on"),
            self.checked,
            Motion::FAST,
            window,
            cx,
        );
        let accent = look.accent.hsla();
        let mut row = switch(div().id(self.id.clone()), self.label.clone(), self.checked)
            .aria_disabled(self.disabled)
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .gap(look.space(Space::M))
            // The Mac toggle's hit height; the whole row takes the click.
            .min_h(look.spaced(24.0))
            .child(
                div()
                    .text_role(TypeRole::Body, &look)
                    .text_color(look.palette.ink.hsla())
                    .child(self.label),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .size(px(BOX))
                    .border(HAIRLINE)
                    .border_color(super::mix(look.palette.muted.hsla(), accent, on))
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(accent.opacity(on))
                            .when(self.checked, |cell| {
                                cell.child(
                                    KoanIcon::new(Icon::Check)
                                        .points(BOX * 0.64)
                                        .color(look.palette.bg.hsla()),
                                )
                            }),
                    )
                    .children(answers.layer(window, cx)),
            );
        if self.disabled {
            row = row.opacity(DISABLED_OPACITY);
        } else if let Some(on_change) = self.on_change {
            let checked = self.checked;
            let flip: Handler<gpui_kit::ClickEvent> = {
                let on_change = on_change.clone();
                let answers = answers.clone();
                Arc::new(move |_, window, cx| {
                    if !checked {
                        answers.start(Answer::Ring, accent, cx);
                    }
                    on_change(&!checked, window, cx)
                })
            };
            row = answers_click(row, flip)
                .track_focus(&handle)
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    if !checked {
                        answers.start(Answer::Ring, accent, cx);
                    }
                    on_change(&!checked, window, cx)
                });
        }
        row.children(ring(&handle, &look, window))
    }
}

/// One glyph that switches a single thing on and off: `ink` underlined in
/// the accent on, `muted` off. The glyph never changes with the state.
#[derive(IntoElement)]
pub struct IconToggle {
    id: ElementId,
    icon: Icon,
    label: SharedString,
    checked: bool,
    disabled: bool,
    on_change: Option<Handler<bool>>,
}

impl IconToggle {
    /// `text` names the toggle for its tooltip.
    pub fn new(id: impl Into<ElementId>, icon: Icon, text: impl AsRef<str>, checked: bool) -> Self {
        Self {
            id: id.into(),
            icon,
            label: label(text),
            checked,
            disabled: false,
            on_change: None,
        }
    }

    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }

    /// Dimmed and deaf, for a toggle with nothing to act on.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for IconToggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let handle = focus(&self.id, window, cx);
        let answers = Answerer::of(&self.id, window, cx);
        let on = fade(
            child(&self.id, "on"),
            self.checked,
            Motion::FAST,
            window,
            cx,
        );
        let color = super::mix(look.palette.muted.hsla(), look.palette.ink.hsla(), on);
        let label = self.label.clone();
        let mut toggle = switch(div().id(self.id.clone()), self.label.clone(), self.checked)
            .aria_disabled(self.disabled)
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .min_w(HIT)
            .min_h(HIT)
            .tooltip(move |window, cx| super::Tooltip::view(label.clone(), window, cx))
            // The glyph centred as every toolbar button's is; the on-mark
            // drawn under it, outside the layout, so it lifts nothing.
            .child(
                div()
                    .relative()
                    .child(
                        KoanIcon::new(self.icon)
                            .role(TypeRole::Control)
                            .color(color),
                    )
                    .child(on_mark(look.accent.hsla().opacity(on)))
                    .children(answers.layer(window, cx)),
            );
        if self.disabled {
            toggle = toggle.opacity(DISABLED_OPACITY);
        } else if let Some(on_change) = self.on_change {
            let (checked, accent) = (self.checked, look.accent.hsla());
            let flip: Handler<gpui_kit::ClickEvent> = {
                let on_change = on_change.clone();
                let answers = answers.clone();
                Arc::new(move |_, window, cx| {
                    if !checked {
                        answers.start(Answer::Ring, accent, cx);
                    }
                    on_change(&!checked, window, cx)
                })
            };
            toggle = answers_click(toggle, flip)
                .track_focus(&handle)
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    if !checked {
                        answers.start(Answer::Ring, accent, cx);
                    }
                    on_change(&!checked, window, cx)
                });
        }
        toggle.children(ring(&handle, &look, window))
    }
}

/// A toggle as a screen reader meets it: a switch, named, on or off.
fn switch(
    element: gpui_kit::Stateful<gpui_kit::Div>,
    name: SharedString,
    checked: bool,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    accessible(element, gpui_kit::Role::Switch, Some(name)).aria_toggled(toggled(checked))
}

/// The accent hairline a toggle shows while on, 5 points under its glyph
/// and as wide as it: positioned outside the layout, so a toggle's glyph sits
/// where every other glyph in its row does.
pub(crate) fn on_mark(color: gpui_kit::Hsla) -> gpui_kit::Div {
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(-5.0) - HAIRLINE)
        .h(HAIRLINE)
        .bg(color)
}
