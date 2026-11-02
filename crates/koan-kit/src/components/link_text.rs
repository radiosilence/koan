//! Names that lead somewhere: a record's title, a credited artist. Plain
//! text until the pointer is on one that leads somewhere, then underlined in
//! `ink`. A line of them shares one baseline and, too long for its room,
//! ends each name in an ellipsis rather than cutting it mid-glyph.

use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Handler;
use crate::theme::{KoanStyled, Look, TypeRole};

/// One name. With `on_click` it leads somewhere: the pointer underlines it
/// in `ink`, and a click on it is not also a click on what it sits in.
pub fn link_text(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    ink: Hsla,
    on_click: Option<Handler<ClickEvent>>,
) -> Stateful<Div> {
    // Shrinks with the others on its line, to an ellipsis.
    let name = div().id(id.into()).min_w_0().truncate().child(text.into());
    let Some(run) = on_click else {
        return name;
    };
    name.cursor_pointer()
        .hover(move |s| s.underline().text_color(ink))
        .on_click(move |e, window, cx| {
            cx.stop_propagation();
            run(e, window, cx)
        })
}

/// What a name leads to when clicked.
pub fn link_action(
    run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Handler<ClickEvent> {
    Arc::new(run)
}

/// One line of names on one baseline in `role` and `color`, as wide as its
/// container lets it be; a name that does not fit ends in an ellipsis.
pub fn name_line(children: Vec<AnyElement>, role: TypeRole, color: Hsla, look: &Look) -> Div {
    div()
        .flex()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_role(role, look)
        .text_color(color)
        .children(children)
}
