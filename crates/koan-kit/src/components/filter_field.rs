//! The toolbar's filter field, the sidebar's search field's look: no box,
//! the glyph and a lowercase prompt in `muted`, on the page's ground. With
//! text in it, a clear mark at its trailing edge (the Mac's), `muted` and
//! `ink` under the pointer, empties it and keeps the keyboard there; Escape
//! does the same, and with the field empty goes on to whatever Escape means
//! around it. The caller owns the `InputState` and listens to it: emptying
//! the field is a change like any other. What was cleared fades out
//! leftward for a moment, a layer the GPU moves, so the clear is seen.

use crate::components::field::{Escape, FieldState as InputState};
use gpui::prelude::*;
use gpui::*;

use super::{Animated, AnimatedLayer, Ease, Repeat, TextField};
use crate::theme::{Icon, KoanIcon, KoanStyled, KoanTheme, Look, Motion, Tone, TypeRole};

/// The text a clear took, fading out, and the timer that lets it go.
#[derive(Default)]
struct Cleared {
    animated: Animated,
    ghost: Option<SharedString>,
    _end: Option<Task<()>>,
}

/// Empties the field, leaving what it held to fade out leftward over
/// `Motion::SETTLE`: nothing under Reduce Motion.
fn clear(
    input: &Entity<InputState>,
    cleared: &Entity<Cleared>,
    view: EntityId,
    window: &mut Window,
    cx: &mut App,
) {
    let text: SharedString = input.read(cx).value().to_string().into();
    input.update(cx, |input, cx| input.clean(window, cx));
    if text.is_empty() || !KoanTheme::motion_allowed(cx) {
        return;
    }
    let travel = -12.0 * window.scale_factor();
    cleared.update(cx, |c, cx| {
        let at = Animated::now();
        super::layer::standing(&c.animated);
        c.animated.segment(
            0,
            1.0,
            0.0,
            at,
            Motion::SETTLE,
            Ease::QuickOut,
            Repeat::Once,
        );
        c.animated.segment(
            1,
            0.0,
            travel,
            at,
            Motion::SETTLE,
            Ease::QuickOut,
            Repeat::Once,
        );
        c.ghost = Some(text);
        c._end = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Motion::SETTLE).await;
            this.update(cx, |c, _| c.ghost = None).ok();
            cx.update(|cx| cx.notify(view));
        }));
    });
    cx.notify(view);
}

#[derive(IntoElement)]
pub struct FilterField {
    input: Entity<InputState>,
    fill: bool,
    label: SharedString,
}

impl FilterField {
    pub fn new(input: &Entity<InputState>) -> Self {
        Self {
            input: input.clone(),
            fill: false,
            label: crate::theme::label("filter"),
        }
    }

    /// What a screen reader calls the field, in the app's words: "filter"
    /// unless the page says what it filters.
    pub fn label(mut self, label: &str) -> Self {
        self.label = crate::theme::label(label);
        self
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
        let (escape, clearing, focus) =
            (self.input.clone(), self.input.clone(), self.input.clone());
        let cleared = window.use_keyed_state(
            ElementId::from(("filter-cleared", self.input.entity_id().as_u64())),
            cx,
            |_, _| Cleared::default(),
        );
        let view = window.current_view();
        let ghost = cleared.read(cx).ghost.clone();
        let fading = cleared.read(cx).animated.clone();
        let (on_escape, on_mark) = (cleared.clone(), cleared.clone());
        let hover = look.palette.ink.hsla();
        // Under touch a toolbar's field, focused, opens out to the toolbar's
        // width over the buttons beside it, easing from where it stood, as
        // an iOS search bar does; it eases back once it lets the keyboard go.
        // Where it stood is kept while it stands there.
        let rest = window.use_keyed_state(
            ElementId::from(("filter-rest", self.input.entity_id().as_u64())),
            cx,
            |_, _| None::<Bounds<Pixels>>,
        );
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let opens = look.compact && !self.fill;
        let open = if opens {
            super::ease(
                ElementId::from(("filter-open", self.input.entity_id().as_u64())),
                focused,
                Motion::SETTLE,
                window,
                cx,
            )
        } else {
            0.0
        };
        let stood = rest.read(cx).filter(|_| open > 0.0);
        // The whole control keeps the keyboard under touch: its clear mark
        // and glyph are the field's, not somewhere else to tap.
        let keeps = self.input.clone();
        let standing = (open <= 0.0).then(|| rest.clone());
        let measure = canvas(
            move |bounds, _, cx| {
                if keeps.read(cx).keeps != Some(bounds) {
                    keeps.update(cx, |state, _| state.keeps = Some(bounds));
                }
                if let Some(rest) = &standing
                    && *rest.read(cx) != Some(bounds)
                {
                    rest.update(cx, |rest, _| *rest = Some(bounds));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let field = div()
            .id("filter-field")
            .map(|d| {
                #[cfg(test)]
                let d = d.debug_selector(|| "filter".into());
                d
            })
            .relative()
            .child(measure)
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
                    d.map(|d| {
                        // Under touch the field has a row to itself and takes
                        // what the buttons beside it leave.
                        if look.compact {
                            d.flex_1()
                        } else {
                            d.w(super::ToolbarMetrics::FIELD * look.scale.size)
                        }
                    })
                    .min_h(super::ToolbarMetrics::FIELD_HIT.max(look.target()))
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
                    clear(&escape, &on_escape, view, window, cx);
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
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .child(TextField::new(&self.input).label(self.label).bare())
                    .children(ghost.map(|ghost| {
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .child(AnimatedLayer::new(
                                &fading,
                                div()
                                    .whitespace_nowrap()
                                    .text_role(TypeRole::Control, &look)
                                    .text_color(look.palette.ink.hsla())
                                    .child(ghost),
                            ))
                    })),
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
                            clear(&clearing, &on_mark, view, window, cx);
                            clearing.update(cx, |input, cx| input.focus(window, cx));
                        }),
                )
            });
        let Some(stood) = stood else {
            return field.into_any_element();
        };
        // Opening: where it stood holds its place in the row, and the field
        // is drawn over the toolbar from there toward the window's margins,
        // on the ground so the buttons under it do not show through.
        let margin =
            super::system_insets(super::Clear::System, window).left + super::ToolbarMetrics::INSET;
        let wide = window.viewport_size().width - margin * 2.;
        let left = stood.left() + (margin - stood.left()) * open;
        let width = stood.size.width + (wide - stood.size.width) * open;
        div()
            .flex_shrink(1.)
            .min_w(super::ToolbarMetrics::FIELD_MIN * look.scale.size)
            .w(stood.size.width)
            .h(stood.size.height)
            .child(
                deferred(
                    anchored().position(point(left, stood.top())).child(
                        field
                            .w(width)
                            .min_w(width)
                            .h(stood.size.height)
                            .px(super::ToolbarMetrics::INSET * open)
                            .bg(look.palette.bg.hsla()),
                    ),
                )
                .with_priority(1),
            )
            .into_any_element()
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
                            .child(FilterField::new(&self.input))
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
                move |_, e: &crate::components::field::InputEvent, _| {
                    if matches!(e, crate::components::field::InputEvent::Change) {
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
