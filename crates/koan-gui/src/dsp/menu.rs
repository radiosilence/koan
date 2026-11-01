//! A choice made from a kōan menu panel: a control showing what is chosen,
//! which opens the choices under it. Used for the device, the preset, a
//! band's type, a target, and each EQ's options.

use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, anchored, deferred, div,
};

use crate::components::{MenuPanel, MenuRow};
use crate::theme::{HAIRLINE, Icon, KoanIcon, KoanStyled, Look, Space, Tone, TypeRole};

/// One entry of a menu.
pub enum Item {
    /// A choice: its label (data, which keeps its case), whether it is in
    /// force, and what choosing it does.
    Choice {
        label: SharedString,
        checked: Option<bool>,
        destructive: bool,
        run: Rc<dyn Fn(&mut Window, &mut App)>,
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
    let toggle = {
        let open = open.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
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
                .h(HAIRLINE)
                .bg(look.palette.rule.hsla())
                .into_any_element(),
        });
        deferred(
            anchored().snap_to_window().child(
                div()
                    .id(gpui_kit::ElementId::NamedChild(
                        std::sync::Arc::new(id.clone()),
                        "panel".into(),
                    ))
                    .occlude()
                    .max_h(gpui_kit::px(420.0))
                    .overflow_y_scroll()
                    .on_mouse_down_out(move |_, _, cx| {
                        close.update(cx, |o, cx| {
                            *o = false;
                            cx.notify();
                        })
                    })
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
