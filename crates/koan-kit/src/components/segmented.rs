use std::sync::Arc;

use gpui_kit::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::{Handler, accessible, answers_click, fade, focus, mix, ring, toggled};
use crate::theme::{KoanStyled, Look, Motion, TypeRole, label};

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
            let chosen = fade(key, index == selected, Motion::FAST, window, cx);
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
            .border_color(accent.opacity(chosen))
            .text_role(TypeRole::Control, &look)
            .text_color(mix(
                look.palette.muted.hsla(),
                look.palette.ink.hsla(),
                chosen,
            ))
            .whitespace_nowrap()
            .relative()
            .child(option)
            .children(answers.layer(window, cx));
            if let Some(on_change) = self.on_change.clone() {
                #[allow(clippy::arc_with_non_send_sync)] // handlers run on the main thread only
                let choose: Handler<gpui_kit::ClickEvent> = {
                    let on_change = on_change.clone();
                    let answers = answers.clone();
                    // The chosen option's mark draws in from its leading edge.
                    Arc::new(move |_, window, cx| {
                        if index != selected {
                            answers.start(super::Answer::Sweep, accent, cx);
                        }
                        on_change(&index, window, cx)
                    })
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::accesskit::Action;
    use gpui_kit::{IntoElement, TestAppContext, VisualTestContext, point};

    use super::*;
    use crate::components::testing::{act, bounds, click_at, node, redraw, stage, tree};

    fn looks(cx: &mut TestAppContext, chosen: Rc<Cell<usize>>) -> &mut VisualTestContext {
        let (_, cx) = stage(cx, move |_, _| {
            let chosen = chosen.clone();
            vec![(
                "look",
                Segmented::new("look", ["kōan", "system", "plain"], chosen.get())
                    .on_change(move |index, _, _| chosen.set(*index))
                    .into_any_element(),
            )]
        });
        cx
    }

    #[gpui_kit::test]
    fn a_radio_group_of_options_one_toggled(cx: &mut TestAppContext) {
        let cx = looks(cx, Rc::new(Cell::new(1)));
        assert!(tree(cx).to_string().contains("RadioGroup"));
        for (name, at, on) in [("kōan", 1, "False"), ("system", 2, "True"), ("plain", 3, "False")] {
            let (_, option) = node(cx, name);
            assert_eq!(option["role"], "RadioButton");
            assert_eq!(option["toggled"], on, "{name}");
            assert_eq!(option["position_in_set"], at, "{name}");
            assert_eq!(option["size_of_set"], 3, "{name}");
        }
    }

    #[gpui_kit::test]
    fn a_click_chooses_an_option(cx: &mut TestAppContext) {
        let chosen = Rc::new(Cell::new(0));
        let cx = looks(cx, chosen.clone());
        let b = bounds(cx, "look");
        click_at(cx, point(b.right() - gpui_kit::px(4.), b.center().y));
        assert_eq!(chosen.get(), 2);
        redraw(cx);
        assert_eq!(node(cx, "plain").1["toggled"], "True");
        act(cx, "kōan", Action::Click);
        assert_eq!(chosen.get(), 0);
    }

    #[gpui_kit::test]
    fn arrows_move_the_choice_and_stop_at_the_ends(cx: &mut TestAppContext) {
        let chosen = Rc::new(Cell::new(0));
        let cx = looks(cx, chosen.clone());
        cx.update(|window, cx| window.focus_next(cx));
        cx.simulate_keystrokes("left");
        assert_eq!(chosen.get(), 0, "moved before the first");
        cx.simulate_keystrokes("right");
        assert_eq!(chosen.get(), 1);
        redraw(cx);
        chosen.set(2);
        redraw(cx);
        cx.simulate_keystrokes("right");
        assert_eq!(chosen.get(), 2, "moved past the last");
    }
}
