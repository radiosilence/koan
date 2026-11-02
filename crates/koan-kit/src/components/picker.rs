use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    Anchor, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, anchored, deferred, div,
};

use super::{Handler, MenuPanel, MenuRow, accessible, answers_click, child, focus, ring};
use crate::theme::{
    DISABLED_OPACITY, HAIRLINE, Icon, KoanIcon, KoanStyled, Look, Space, TypeRole, label,
};

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
            disabled: false,
            on_change: None,
        }
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
        let color = look.palette.ink.hsla();
        let shown = self
            .selected
            .and_then(|index| self.options.get(index).cloned())
            .unwrap_or_default();

        let toggle = open.clone();
        // A combo box to a screen reader: what is chosen is its value, and
        // a click opens it as the pointer's does.
        let mut picker = accessible(div().id(self.id.clone()), gpui_kit::Role::ComboBox, None)
            .aria_value(shown.clone())
            .aria_expanded(is_open)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(look.space(Space::Xs))
            // The theme's bordered button, as a form's dropdown is.
            .py(look.spaced(6.))
            .px(look.spaced(12.))
            .border(HAIRLINE)
            .border_color(look.palette.muted.hsla())
            .text_role(TypeRole::Control, &look)
            .text_color(color)
            .child(div().whitespace_nowrap().child(shown))
            .child(
                KoanIcon::new(Icon::Choose)
                    .role(TypeRole::Control)
                    .color(color),
            );
        if self.disabled {
            picker = picker.opacity(DISABLED_OPACITY);
        } else {
            let (surface, pressed) = (look.palette.surface.hsla(), look.palette.hover.hsla());
            let opens: Handler<gpui_kit::ClickEvent> = {
                let toggle = open.clone();
                std::sync::Arc::new(move |_, _, cx| {
                    toggle.update(cx, |open, cx| {
                        *open = true;
                        cx.notify();
                    })
                })
            };
            picker = answers_click(picker, opens)
                .cursor_pointer()
                .hover(move |s| s.bg(surface))
                .active(move |s| s.bg(pressed))
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
