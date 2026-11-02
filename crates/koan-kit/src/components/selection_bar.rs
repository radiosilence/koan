//! Along the foot of a page while picking: how many are picked, and what to
//! do with them. One bar for every page, so the verbs read the same wherever
//! something can be picked; each verb ends the mode.

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Button, ButtonKind, Handler, RowMetrics};
use crate::theme::{HAIRLINE, KoanStyled, Look, Space, TypeRole};

/// The count in `meta`, `ink`, leading; the verbs trailing, `Text` buttons
/// or anything drawn as one (a menu's trigger), then "done". On `bg` with a
/// `rule` above, at the page's margins.
#[derive(IntoElement)]
pub struct SelectionBar {
    count: SharedString,
    verbs: Vec<AnyElement>,
    done: Option<Handler<ClickEvent>>,
}

impl SelectionBar {
    /// `count` as the page says it: "3 picked".
    pub fn new(count: impl Into<SharedString>) -> Self {
        Self {
            count: count.into(),
            verbs: Vec::new(),
            done: None,
        }
    }

    /// A verb as a `Text` button in the app's words.
    pub fn verb(
        self,
        id: impl Into<ElementId>,
        text: impl AsRef<str>,
        run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.element(Button::new(id, ButtonKind::Text).label(text).on_click(run))
    }

    /// A verb that is more than a button, such as a menu's trigger.
    pub fn element(mut self, verb: impl IntoElement) -> Self {
        self.verbs.push(verb.into_any_element());
        self
    }

    /// Ends the mode without acting.
    pub fn on_done(mut self, run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.done = Some(std::sync::Arc::new(run));
        self
    }
}

impl RenderOnce for SelectionBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .flex()
            .items_center()
            .justify_between()
            .px(RowMetrics::page_margin(&look))
            .py(look.space(Space::M))
            .border_t(look.hairline)
            .border_color(look.palette.rule.hsla())
            .bg(look.palette.bg.hsla())
            .child(
                div()
                    .text_role(TypeRole::Meta, &look)
                    .text_color(look.palette.ink.hsla())
                    .child(self.count),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(look.space(Space::L))
                    .children(self.verbs)
                    .children(self.done.map(|done| {
                        Button::new("selection-done", ButtonKind::Text)
                            .label("done")
                            .on_click(move |e, window, cx| done(e, window, cx))
                    })),
            )
    }
}
