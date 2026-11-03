use std::sync::Arc;

use gpui_kit::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::{Handler, accessible, answers_click, fade, focus, ring, toggled};
use crate::theme::{KoanStyled, Look, TypeRole, label};

/// A row of text options, 18 apart, with no track: `muted`, and the chosen
/// one `ink` and underlined in the accent 5 points below. Each option's
/// underline fades with its choice, as the Mac's `KoanSegmentedPicker` does.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    options: Vec<SharedString>,
    selected: usize,
    on_change: Option<Handler<usize>>,
}

impl Segmented {
    pub fn new<S: AsRef<str>>(
        id: impl Into<ElementId>,
        options: impl IntoIterator<Item = S>,
        selected: usize,
    ) -> Self {
        Self {
            id: id.into(),
            options: options.into_iter().map(label).collect(),
            selected,
            on_change: None,
        }
    }

    pub fn on_change(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let handle = focus(&self.id, window, cx);
        let selected = self.selected;
        let count = self.options.len();
        let accent = look.accent.hsla();
        let mut row = accessible(div().id(self.id.clone()), gpui_kit::Role::RadioGroup, None)
            .relative()
            .flex()
            .items_center()
            .gap(px(18.0));
        for (index, option) in self.options.into_iter().enumerate() {
            let key =
                ElementId::NamedChild(Arc::new(self.id.clone()), format!("option-{index}").into());
            let answers = super::Answerer::of(&key, window, cx);
            let chosen = index == selected;
            // The option coming to be chosen draws its mark in from its
            // leading edge, by click or by the arrows alike.
            if chosen {
                answers.follow(1, super::Answer::Sweep, accent, window, cx);
            } else {
                answers.note(0, cx);
            }
            let edge = fade(
                ElementId::NamedChild(Arc::new(key.clone()), "edge".into()),
                chosen,
                accent.opacity(0.0),
                accent,
                window,
                cx,
            );
            let ink = fade(
                key,
                chosen,
                look.palette.muted.hsla(),
                look.palette.ink.hsla(),
                window,
                cx,
            );
            let mut option = accessible(
                div().id(index),
                gpui_kit::Role::RadioButton,
                Some(option.clone()),
            )
            .aria_toggled(toggled(index == selected))
            .aria_position_in_set(index + 1)
            .aria_size_of_set(count)
            .pb(px(5.0))
            .border_b(look.hairline)
            .border_color(edge)
            .text_role(TypeRole::Control, &look)
            .text_color(ink)
            .whitespace_nowrap()
            .relative()
            .child(option)
            .children(answers.layer(window, cx));
            if let Some(on_change) = self.on_change.clone() {
                #[allow(clippy::arc_with_non_send_sync)] // handlers run on the main thread only
                let choose: Handler<gpui_kit::ClickEvent> = {
                    let on_change = on_change.clone();
                    Arc::new(move |_, window, cx| on_change(&index, window, cx))
                };
                option = answers_click(option, choose.clone())
                    .cursor_pointer()
                    .on_click(move |event, window, cx| choose(event, window, cx));
            }
            row = row.child(option);
        }
        if let Some(on_change) = self.on_change {
            row = row
                .track_focus(&handle)
                .on_key_down(move |event, window, cx| {
                    let next = match event.keystroke.key.as_str() {
                        "left" if selected > 0 => selected - 1,
                        "right" if selected + 1 < count => selected + 1,
                        _ => return,
                    };
                    cx.stop_propagation();
                    on_change(&next, window, cx);
                });
        }
        row.children(ring(&handle, &look, window))
    }
}
