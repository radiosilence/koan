use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, anchored, deferred, div,
};

use super::{Handler, MenuPanel, MenuRow, accessible, answers_click, child, focus, ring};
use crate::theme::{DISABLED_OPACITY, Icon, KoanIcon, KoanStyled, Look, Space, TypeRole, label};

/// A pop-up picker: the chosen option in `control`, `ink`, with a chevron, in
/// the theme's bordered button, as the Mac's `KoanPicker` is; it opens the
/// theme's panel with the options as ticked rows.
/// Options are app words, drawn lowercase, unless `keeps_case` says they are
/// data (a device's name).
#[derive(IntoElement)]
pub struct Picker {
    id: ElementId,
    options: Vec<SharedString>,
    selected: Option<usize>,
    name: Option<SharedString>,
    disabled: bool,
    on_change: Option<Handler<usize>>,
}

impl Picker {
    pub fn new<S: AsRef<str>>(
        id: impl Into<ElementId>,
        options: impl IntoIterator<Item = S>,
        selected: Option<usize>,
    ) -> Self {
        Self {
            id: id.into(),
            options: options.into_iter().map(label).collect(),
            selected,
            name: None,
            disabled: false,
            on_change: None,
        }
    }

    /// Options that are data rather than the app's words, which keep their
    /// case.
    pub fn keeps_case<S: Into<SharedString>>(
        id: impl Into<ElementId>,
        options: impl IntoIterator<Item = S>,
        selected: Option<usize>,
    ) -> Self {
        Self {
            id: id.into(),
            options: options.into_iter().map(Into::into).collect(),
            selected,
            name: None,
            disabled: false,
            on_change: None,
        }
    }

    /// What the choice is of, for a screen reader: the form row's title.
    pub fn named(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }
}

impl RenderOnce for Picker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let open = window.use_keyed_state(child(&self.id, "open"), cx, |_, _| false);
        let is_open = *open.read(cx) && !self.disabled;
        let handle = focus(&self.id, window, cx);
        // Nothing to choose from, or nothing chosen: said in `muted`, so the
        // control is never an empty box.
        let chosen = self
            .selected
            .and_then(|index| self.options.get(index).cloned());
        let color = if chosen.is_some() {
            look.palette.ink.hsla()
        } else {
            look.palette.muted.hsla()
        };
        let shown = chosen.unwrap_or_else(|| {
            label(if self.options.is_empty() {
                "none"
            } else {
                "choose"
            })
        });

        let toggle = open.clone();
        let (pointer, lit) = super::hover_fill(&self.id, &look, window, cx);
        // A combo box to a screen reader: what is chosen is its value, and
        // a click opens it as the pointer's does.
        let mut picker = accessible(
            div().id(self.id.clone()),
            gpui::Role::ComboBox,
            self.name.clone(),
        )
        .aria_disabled(self.disabled)
        .aria_value(shown.clone())
        .aria_expanded(is_open)
        .relative()
        .flex()
        // On a phone it gives way to the row's edge, cutting what is chosen
        // short inside its box rather than running past the margin.
        .map(|d| {
            if look.touch {
                d.flex_shrink(1.).min_w_0()
            } else {
                d.flex_none()
            }
        })
        .items_center()
        .gap(look.space(Space::Xs))
        // The theme's bordered button, as a form's dropdown is.
        .py(look.spaced(6.) + look.hairline)
        .px(look.spaced(12.) + look.hairline)
        .text_role(TypeRole::Control, &look)
        .text_color(color)
        // The fill and the outline are siblings sized alike, so they are one
        // box whatever the content measures.
        .when(!self.disabled, |d| d.child(lit))
        .child(
            div()
                .absolute()
                .inset_0()
                .border(look.hairline)
                .border_color(look.palette.muted.hsla()),
        )
        .child(div().min_w_0().truncate().child(shown))
        .child(
            KoanIcon::new(Icon::Choose)
                .role(TypeRole::Control)
                .color(color),
        );
        if self.disabled {
            picker = picker.opacity(DISABLED_OPACITY);
        } else {
            let opens: Handler<gpui::ClickEvent> = {
                let toggle = open.clone();
                std::sync::Arc::new(move |_, _, cx| {
                    toggle.update(cx, |open, cx| {
                        *open = true;
                        cx.notify();
                    })
                })
            };
            picker = super::track(answers_click(picker, opens), &pointer)
                .cursor_pointer()
                .track_focus(&handle)
                // Opens only: a click on the picker while the panel is up
                // lands outside the panel, which closes it.
                .on_click(move |_, _, cx| {
                    toggle.update(cx, |open, cx| {
                        *open = true;
                        cx.notify();
                    })
                });
        }

        let on_change = self.on_change;
        let selected = self.selected;
        let menu = is_open.then(|| {
            let close = open.clone();
            let panel = MenuPanel::new().children(self.options.into_iter().enumerate().map(
                |(index, option)| {
                    let on_change = on_change.clone();
                    let open = open.clone();
                    MenuRow::new(index, "")
                        .checked(selected == Some(index))
                        .label_data(option)
                        .on_click(move |_, window, cx| {
                            open.update(cx, |open, cx| {
                                *open = false;
                                cx.notify();
                            });
                            if selected != Some(index)
                                && let Some(on_change) = &on_change
                            {
                                on_change(&index, window, cx);
                            }
                        })
                },
            ));
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(look.space(Space::S))
                    .child(
                        div()
                            .id(child(&self.id, "menu"))
                            .occlude()
                            .on_mouse_down_out(move |_, _, cx| {
                                close.update(cx, |open, cx| {
                                    *open = false;
                                    cx.notify();
                                })
                            })
                            .child(panel),
                    ),
            )
            .with_priority(1)
        });

        picker
            .when_some(menu, |picker, menu| picker.child(menu))
            .children(ring(&handle, &look, window))
    }
}
