//! The toolbar's filter field, the sidebar's search field's look: no box,
//! the glyph and a lowercase prompt in `muted`, on the page's ground. With
//! text in it, a clear mark at its trailing edge (the Mac's), `muted` and
//! `ink` under the pointer, empties it and keeps the keyboard there; Escape
//! does the same, and with the field empty goes on to whatever Escape means
//! around it. The caller owns the `InputState` and listens to it: emptying
//! the field is a change like any other.

use gpui::prelude::*;
use gpui::*;
use gpui_kit::component::input::{Escape, InputState};

use super::TextField;
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, Tone, TypeRole};

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
                        .flex_shrink(1.)
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

/// A filter field's look standing for a search that happens elsewhere: the
/// glyph and what is searched for, or `prompt` in `muted` while nothing is,
/// the whole row a button. The sidebar's search field, which opens the
/// search box over the window and shows its query.
#[derive(IntoElement)]
pub struct FilterButton {
    id: ElementId,
    text: SharedString,
    prompt: SharedString,
    on_click: Option<super::Handler<ClickEvent>>,
}

impl FilterButton {
    /// `prompt` in the app's words ("search"); `text` is data (a query) and
    /// keeps its case.
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>, prompt: &str) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            prompt: crate::theme::label(prompt),
            on_click: None,
        }
    }

    pub fn on_click(mut self, run: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(std::sync::Arc::new(run));
        self
    }
}

impl RenderOnce for FilterButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let empty = self.text.is_empty();
        let shown = if empty {
            self.prompt.clone()
        } else {
            self.text.clone()
        };
        let row = super::accessible(div().id(self.id), gpui::Role::Button, Some(shown.clone()));
        let row = match self.on_click {
            Some(run) => super::answers_click(row, run.clone())
                .cursor_pointer()
                .on_click(move |e, window, cx| run(e, window, cx)),
            None => row,
        };
        row.flex()
            .items_center()
            .w_full()
            .gap(super::ToolbarMetrics::FIELD_GLYPH_GAP * look.scale.size)
            .child(
                KoanIcon::new(Icon::Search)
                    .role(TypeRole::Control)
                    .tone(Tone::Muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_role(TypeRole::Control, &look)
                    .text_color(if empty {
                        look.palette.muted.hsla()
                    } else {
                        look.palette.ink.hsla()
                    })
                    .child(shown),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Modifiers, TestAppContext, VisualTestContext, point};

    use super::*;
    use crate::components::{Toolbar, ToolbarButton, ToolbarGroup};
    use core::prelude::v1::test;

    struct Bar {
        input: Entity<InputState>,
        width: Pixels,
    }

    impl Render for Bar {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // As the shell builds it: bare space in the middle, the page's
            // group trailing, the field leading the group.
            div().w(self.width).child(
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
                            .child(
                                div()
                                    .debug_selector(|| "sort".into())
                                    .child(ToolbarButton::new("sort", Icon::Sort, "sort")),
                            ),
                    ),
            )
        }
    }

    /// The field takes its width in a toolbar, and a click on it focuses it.
    #[gpui::test]
    fn the_filter_field_in_a_toolbar_takes_its_width_and_a_click(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::app::platform::init(cx);
            crate::theme::init(cx);
        });
        let (bar, cx) = cx.add_window_view(|window, cx| Bar {
            input: cx.new(|cx| InputState::new(window, cx).placeholder("filter albums")),
            width: px(900.),
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

    /// In a narrow toolbar the field gives up its width, down to its
    /// minimum, before the buttons beside it give up any.
    #[gpui::test]
    fn the_filter_field_shrinks_before_the_toolbar_buttons(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::app::platform::init(cx);
            crate::theme::init(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| Bar {
            input: cx.new(|cx| InputState::new(window, cx).placeholder("filter albums")),
            width: px(220.),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let field = cx.debug_bounds("filter").expect("field drawn");
        let sort = cx.debug_bounds("sort").expect("button drawn");
        assert!(
            field.size.width < super::super::ToolbarMetrics::FIELD,
            "the field kept its whole measure: {field:?}"
        );
        assert!(
            field.size.width >= super::super::ToolbarMetrics::FIELD_MIN,
            "the field went under its minimum: {field:?}"
        );
        assert!(
            sort.size.width >= crate::theme::TOOLBAR_BUTTON,
            "the button gave up width: {sort:?}"
        );
        assert!(sort.right() <= px(220.), "the button is cut off: {sort:?}");
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
    #[gpui::test]
    fn the_clear_mark_and_escape_empty_the_field(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::app::platform::init(cx);
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
