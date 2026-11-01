//! Asking before the queue goes. A click on a sleeve among results plays the
//! record, and a queue built up over an evening should not go to a click
//! meant to open it; nor should the picker's Replace Queue take it without
//! saying what goes. With anything queued, the person sees how much would be
//! taken off and chooses: replace it, add instead, or neither.

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::components::{Button, ButtonKind};
use crate::library::chrome;
use crate::library::tokens::{Paint, Type, Typeset};
use crate::mirror::Mirror;
use crate::theme::Space;

pub type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// How many tracks are queued: what replacing takes off.
pub fn queued(cx: &App) -> usize {
    cx.global::<Mirror>().queue.read(cx).items.len()
}

/// The question over the whole of its page, which it dims. `replace` is the
/// prominent verb's label.
pub fn ask(
    p: &Paint,
    queued: usize,
    replace: &'static str,
    on_replace: Handler,
    on_add: Handler,
    on_cancel: Handler,
) -> impl IntoElement {
    let run = |handler: &Handler| {
        let handler = handler.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| handler(window, cx)
    };
    let dismiss = on_cancel.clone();
    deferred(
        div()
            .id("replace-queue")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.dim())
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| dismiss(window, cx))
            .child(
                div()
                    .id("replace-queue-panel")
                    .w(px(360.))
                    .flex()
                    .flex_col()
                    .gap(p.space(Space::M))
                    .p(p.space(Space::L))
                    .bg(p.bg)
                    .border_1()
                    .border_color(p.rule)
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .typeset(p, Type::TitleSmall, p.strong)
                            .child("replace the queue?"),
                    )
                    .child(div().typeset(p, Type::Body, p.ink).child(format!(
                        "this takes the {} queued off.",
                        chrome::count(queued as u64, "track", "tracks")
                    )))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(p.space(Space::S))
                            .child(
                                Button::new("replace-cancel", ButtonKind::Compact)
                                    .label("cancel")
                                    .on_click(run(&on_cancel)),
                            )
                            .child(
                                Button::new("replace-add", ButtonKind::Standard)
                                    .label("add to queue")
                                    .on_click(run(&on_add)),
                            )
                            .child(
                                Button::new("replace-play", ButtonKind::Prominent)
                                    .label(replace)
                                    .on_click(run(&on_replace)),
                            ),
                    ),
            ),
    )
    .with_priority(2)
}
