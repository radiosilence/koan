//! The toolbar's filter field, the sidebar's search field's look: no box,
//! the glyph and a lowercase prompt in `muted`, on the page's ground. With
//! text in it, a clear mark at its trailing edge (the Mac's), `muted` and
//! `ink` under the pointer, empties it and keeps the keyboard there; Escape
//! does the same, and with the field empty goes on to whatever Escape means
//! around it. The caller owns the `InputState` and listens to it: emptying
//! the field is a change like any other.

use gpui_kit::component::input::{Escape, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::TextField;
use crate::theme::{Icon, KoanIcon, Look, Tone, TypeRole};

#[derive(IntoElement)]
pub struct FilterField {
    input: Entity<InputState>,
    fill: bool,
}

impl FilterField {
    pub fn new(input: &Entity<InputState>) -> Self {
        Self {
            input: input.clone(),
            fill: false,
        }
    }

    /// As wide as what holds it, as the sidebar's search field is, rather
    /// than the toolbar's measure.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }
}

impl RenderOnce for FilterField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let empty = self.input.read(cx).value().is_empty();
        let (escape, clear, focus) = (self.input.clone(), self.input.clone(), self.input.clone());
        let hover = look.palette.ink.hsla();
        div()
            .id("filter-field")
            .flex()
            .items_center()
            .cursor_text()
            .gap(super::ToolbarMetrics::FIELD_GLYPH_GAP * look.scale.size)
            .map(|d| {
                if self.fill {
                    d.w_full()
                } else {
                    // Its measure, given up before the toolbar's buttons
                    // when the page column is narrow.
                    // In the toolbar, a button's height to click, glyph and
                    // margins included, not the text's line alone.
                    d.w(super::ToolbarMetrics::FIELD * look.scale.size)
                        .min_h(super::ToolbarMetrics::FIELD_HIT)
                        .flex_shrink()
                        .min_w(super::ToolbarMetrics::FIELD_MIN * look.scale.size)
                }
            })
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus.update(cx, |input, cx| input.focus(window, cx));
            })
            .on_action(move |_: &Escape, window, cx| {
                if escape.read(cx).value().is_empty() {
                    cx.propagate();
                } else {
                    escape.update(cx, |input, cx| input.clean(window, cx));
                }
            })
            .child(
                KoanIcon::new(Icon::Search)
                    .role(TypeRole::Control)
                    .tone(Tone::Muted),
            )
            // The rest of the row: an input is as wide as its parent, which a
            // flex row otherwise sizes to the input's content, nothing.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(TextField::new(&self.input).bare()),
            )
            .when(!empty, |d| {
                d.child(
                    div()
                        .id("filter-clear")
                        .map(|d| {
                            #[cfg(test)]
                            let d = d.debug_selector(|| "filter-clear-mark".into());
                            d
                        })
                        .flex_none()
                        .cursor_pointer()
                        .group("filter-clear")
                        .child(
                            KoanIcon::new(Icon::ClearField)
                                .role(TypeRole::Control)
                                .tone(Tone::Muted)
                                .hover_color("filter-clear", hover),
                        )
                        // Down rather than click, so the field never loses
                        // the keyboard to the mark.
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            window.prevent_default();
                            clear.update(cx, |input, cx| {
                                input.clean(window, cx);
                                input.focus(window, cx);
                            });
                        }),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, point};

    use super::*;
    use crate::components::{Toolbar, ToolbarButton, ToolbarGroup};
    use core::prelude::v1::test;

    struct Bar {
        input: Entity<InputState>,
    }

    impl Render for Bar {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // As the shell builds it: bare space in the middle, the page's
            // group trailing, the field leading the group.
            div().w(px(900.)).child(
                Toolbar::new()
                    .leading(ToolbarButton::new("back", Icon::Back, "back"))
                    .principal(div().id("drag").size_full())
                    .trailing(
                        ToolbarGroup::new()
                            .child(
                                div()
                                    .debug_selector(|| "filter".into())
                                    .child(FilterField::new(&self.input)),
                            )
                            .spacer()
                            .child(ToolbarButton::new("sort", Icon::Sort, "sort")),
                    ),
            )
        }
    }

    /// The field takes its width in a toolbar, and a click on it focuses it.
    #[gpui_kit::test]
    fn the_filter_field_in_a_toolbar_takes_its_width_and_a_click(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (bar, cx) = cx.add_window_view(|window, cx| Bar {
            input: cx.new(|cx| InputState::new(window, cx).placeholder("filter albums")),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let field = cx.debug_bounds("filter").expect("field drawn");
        assert!(field.size.width >= px(160.), "{field:?}");
        let input = bar.read_with(cx, |b, _| b.input.clone());
        let focused = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| input.read(cx).focus_handle(cx).is_focused(window))
        };
        assert!(!focused(cx));
        let at = point(field.right() - px(40.), field.center().y);
        cx.simulate_click(at, Modifiers::default());
        cx.run_until_parked();
        assert!(
            focused(cx),
            "a click at {at:?} on {field:?} focused nothing"
        );
    }

    struct Field {
        input: Entity<InputState>,
    }

    impl Render for Field {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(300.))
                .child(FilterField::new(&self.input).fill())
        }
    }

    /// Typing shows the clear mark; a press on it empties the field, tells
    /// the owner, and leaves the keyboard in the field. Escape empties it
    /// too, and with nothing left goes on to what is around it.
    #[gpui_kit::test]
    fn the_clear_mark_and_escape_empty_the_field(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (field, cx) = cx.add_window_view(|window, cx| Field {
            input: cx.new(|cx| InputState::new(window, cx).placeholder("search")),
        });
        let cx: &mut VisualTestContext = cx;
        let input = field.read_with(cx, |f, _| f.input.clone());
        let changes = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = changes.clone();
        cx.update(|_, cx| {
            cx.subscribe(
                &input,
                move |_, e: &gpui_kit::component::input::InputEvent, _| {
                    if matches!(e, gpui_kit::component::input::InputEvent::Change) {
                        counted.set(counted.get() + 1);
                    }
                },
            )
            .detach()
        });
        cx.update(|window, cx| input.update(cx, |i, cx| i.focus(window, cx)));
        cx.simulate_input("abba");
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| input.read(cx).value().to_string()),
            "abba"
        );
        let typed = changes.get();
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| input.read(cx).value().to_string()), "");
        assert!(
            changes.get() > typed,
            "emptying is a change the owner hears"
        );

        cx.simulate_input("x");
        cx.run_until_parked();
        let clear = cx
            .debug_bounds("filter-clear-mark")
            .expect("the mark shows with text");
        cx.simulate_mouse_down(clear.center(), MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| input.read(cx).value().to_string()), "");
        assert!(
            cx.update(|window, cx| input.read(cx).focus_handle(cx).is_focused(window)),
            "the field keeps the keyboard"
        );
        assert!(cx.debug_bounds("filter-clear-mark").is_none());
    }
}
