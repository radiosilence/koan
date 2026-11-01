use std::sync::Arc;

use gpui_kit::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::{Handler, child, fade, focus, ring, track};
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
        let on = fade(
            child(&self.id, "on"),
            self.checked,
            Motion::FAST,
            window,
            cx,
        );
        let accent = look.accent.hsla();
        let mut row = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .gap(look.space(Space::M))
            .min_h(HIT)
            .track_focus(&handle)
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
                            .when_checked(self.checked, &look),
                    ),
            );
        if self.disabled {
            row = row.opacity(DISABLED_OPACITY);
        } else if let Some(on_change) = self.on_change {
            let checked = self.checked;
            let on_key = on_change.clone();
            row = row
                .cursor_pointer()
                .on_click(move |_, window, cx| on_change(&!checked, window, cx))
                .on_key_down(move |event, window, cx| {
                    if event.keystroke.key == "space" {
                        cx.stop_propagation();
                        on_key(&!checked, window, cx);
                    }
                });
        }
        row.children(ring(&handle, &look, window))
    }
}

trait Tick {
    fn when_checked(self, checked: bool, look: &Look) -> Self;
}

impl Tick for gpui_kit::Div {
    fn when_checked(self, checked: bool, look: &Look) -> Self {
        if checked {
            self.child(
                KoanIcon::new(Icon::Check)
                    .points(8.0)
                    .color(look.palette.bg.hsla()),
            )
        } else {
            self
        }
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
            on_change: None,
        }
    }

    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for IconToggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let handle = focus(&self.id, window, cx);
        let state = super::pointer(&self.id, window, cx);
        let on = fade(
            child(&self.id, "on"),
            self.checked,
            Motion::FAST,
            window,
            cx,
        );
        let color = super::mix(look.palette.muted.hsla(), look.palette.ink.hsla(), on);
        let label = self.label.clone();
        let mut toggle = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .min_w(HIT)
            .min_h(HIT)
            .track_focus(&handle)
            .tooltip(move |window, cx| super::Tooltip::view(label.clone(), window, cx))
            .child(
                div()
                    .pb(px(5.0))
                    .border_b(HAIRLINE)
                    .border_color(look.accent.hsla().opacity(on))
                    .child(
                        KoanIcon::new(self.icon)
                            .role(TypeRole::Control)
                            .color(color),
                    ),
            );
        toggle = track(toggle, &state);
        if let Some(on_change) = self.on_change {
            let checked = self.checked;
            let on_key = on_change.clone();
            toggle = toggle
                .cursor_pointer()
                .on_click(move |_, window, cx| on_change(&!checked, window, cx))
                .on_key_down(move |event, window, cx| {
                    if event.keystroke.key == "space" {
                        cx.stop_propagation();
                        on_key(&!checked, window, cx);
                    }
                });
        }
        toggle.children(ring(&handle, &look, window))
    }
}
