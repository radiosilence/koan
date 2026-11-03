//! A choice made from a kōan menu panel: a control showing what is chosen,
//! which opens the choices under it, with headings and rules between them
//! and destructive actions among them. The open state is kept by the window
//! under the control's id, so the view holding it needs no field. Escape, a
//! press outside or a choice closes it.

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{MenuPanel, MenuRow};
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole};

/// What choosing an entry does.
pub type Run = Rc<dyn Fn(&mut Window, &mut App)>;

/// One entry of a menu.
pub enum Item {
    /// A choice: its label (data, which keeps its case), whether it is in
    /// force, and what choosing it does.
    Choice {
        label: SharedString,
        checked: Option<bool>,
        destructive: bool,
        run: Run,
    },
    /// A heading over the choices after it.
    Heading(SharedString),
    Rule,
}

impl Item {
    pub fn choice(
        label: impl Into<SharedString>,
        checked: bool,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Item::Choice {
            label: label.into(),
            checked: Some(checked),
            destructive: false,
            run: Rc::new(run),
        }
    }

    pub fn action(
        label: impl Into<SharedString>,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Item::Choice {
            label: label.into(),
            checked: None,
            destructive: false,
            run: Rc::new(run),
        }
    }

    pub fn destructive(
        label: impl Into<SharedString>,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Item::Choice {
            label: label.into(),
            checked: None,
            destructive: true,
            run: Rc::new(run),
        }
    }
}

/// How the control looks: a field showing the choice, or a quiet word.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Field,
    Word,
}

/// A control showing `shown` that opens `items` under it. The open state is
/// kept under `id` by the window, so the menu needs no field in its view.
pub fn menu(
    id: impl Into<ElementId>,
    shown: impl Into<SharedString>,
    trigger: Trigger,
    items: Vec<Item>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = id.into();
    let look = Look::of(window, cx);
    let open = window.use_keyed_state(id.clone(), cx, |_, _| false);
    let is_open = *open.read(cx);
    // A press on the trigger while the menu is open is the trigger's to
    // close: the panel's press-outside leaves it alone.
    let over_trigger = window.use_keyed_state(
        ElementId::NamedChild(std::sync::Arc::new(id.clone()), "over".into()),
        cx,
        |_, _| false,
    );
    let toggle = {
        let open = open.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            open.update(cx, |o, cx| {
                *o = !*o;
                cx.notify();
            })
        }
    };
    let control = div()
        .id(id.clone())
        .flex()
        .items_center()
        .gap(look.space(Space::S))
        .cursor_pointer()
        .text_role(TypeRole::Control, &look)
        .map(|d| match trigger {
            Trigger::Field => d
                .px(look.space(Space::M))
                .py(look.space(Space::Xs))
                .bg(look.palette.surface.hsla())
                .text_color(look.palette.ink.hsla()),
            Trigger::Word => d
                .text_color(look.text_color(Tone::Muted))
                .hover(|d| d.text_color(look.palette.ink.hsla())),
        })
        .on_hover({
            let over = over_trigger.clone();
            move |hovered, _, cx| over.update(cx, |o, _| *o = *hovered)
        })
        .on_click(toggle)
        .child(div().whitespace_nowrap().child(shown.into()))
        .when(trigger == Trigger::Field, |d| {
            d.child(
                KoanIcon::new(Icon::Choose)
                    .role(TypeRole::Control)
                    .tone(Tone::Muted),
            )
        });
    let panel = is_open.then(|| {
        let close = open.clone();
        let rows = items.into_iter().enumerate().map(|(i, item)| match item {
            Item::Choice {
                label,
                checked,
                destructive,
                run,
            } => {
                let open = open.clone();
                MenuRow::data(("item", i), label)
                    .map_checked(checked)
                    .destructive(destructive)
                    .on_click(move |_, window, cx| {
                        open.update(cx, |o, cx| {
                            *o = false;
                            cx.notify();
                        });
                        run(window, cx);
                    })
                    .into_any_element()
            }
            Item::Heading(text) => div()
                .px(look.space(Space::M))
                .pt(look.space(Space::S))
                .text_role(TypeRole::Fine, &look)
                .text_color(look.text_color(Tone::Muted))
                .child(text)
                .into_any_element(),
            Item::Rule => div()
                .my(look.space(Space::Xs))
                .h(look.hairline)
                .bg(look.palette.rule.hsla())
                .into_any_element(),
        });
        deferred(
            anchored().snap_to_window().child(
                div()
                    .id(ElementId::NamedChild(
                        std::sync::Arc::new(id.clone()),
                        "panel".into(),
                    ))
                    .occlude()
                    .max_h(look.spaced(420.0))
                    .overflow_y_scroll()
                    .on_mouse_down_out({
                        let close = close.clone();
                        move |_, _, cx| {
                            if *over_trigger.read(cx) {
                                return;
                            }
                            close.update(cx, |o, cx| {
                                *o = false;
                                cx.notify();
                            })
                        }
                    })
                    // Escape closes it, heard while it is drawn: first, on the
                    // way in, and no further, so the sheet it sits in or the
                    // field beside it does not take the same key.
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |_, _, window, _| {
                                let close = close.clone();
                                window.on_key_event(move |e: &KeyDownEvent, phase, _, cx| {
                                    if phase == DispatchPhase::Capture
                                        && e.keystroke.key == "escape"
                                    {
                                        cx.stop_propagation();
                                        close.update(cx, |o, cx| {
                                            *o = false;
                                            cx.notify();
                                        });
                                    }
                                });
                            },
                        )
                        .size_0(),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(MenuPanel::new().children(rows)),
            ),
        )
        .with_priority(1)
    });
    div()
        .relative()
        .child(control)
        .children(panel)
        .into_any_element()
}

trait MapChecked {
    fn map_checked(self, checked: Option<bool>) -> Self;
}

impl MapChecked for MenuRow {
    fn map_checked(self, checked: Option<bool>) -> Self {
        match checked {
            Some(c) => self.checked(c),
            None => self,
        }
    }
}
