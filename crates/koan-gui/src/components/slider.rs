use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Bounds, DispatchPhase, ElementId, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, RenderOnce,
    SharedString, Styled, Window, canvas, div, px,
};

use super::{Handler, child, focus, pointer, ring, track};
use crate::theme::{DISABLED_OPACITY, HAIRLINE, HIT, KoanStyled, Look, Space, TypeRole};

/// A 1-point `rule` track with a 3-point accent fill up to the value, and a
/// square 8 × 8 `ink` thumb shown only on hover, focus or drag. The values
/// at either end sit in `fine`, `muted`.
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    value: f32,
    low: Option<SharedString>,
    high: Option<SharedString>,
    step: f32,
    disabled: bool,
    on_change: Option<Handler<f32>>,
}

impl Slider {
    /// `value` is a fraction of the track, 0 to 1.
    pub fn new(id: impl Into<ElementId>, value: f32) -> Self {
        Self {
            id: id.into(),
            value: value.clamp(0.0, 1.0),
            low: None,
            high: None,
            step: 0.05,
            disabled: false,
            on_change: None,
        }
    }

    /// What sits at either end, such as the time played and the time left.
    pub fn ends(mut self, low: impl Into<SharedString>, high: impl Into<SharedString>) -> Self {
        self.low = Some(low.into());
        self.high = Some(high.into());
        self
    }

    /// How far an arrow key moves it.
    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }
}

const THUMB: f32 = 8.0;
const FILL: f32 = 3.0;

fn at(bounds: Bounds<Pixels>, position: Point<Pixels>) -> f32 {
    let width = f32::from(bounds.size.width);
    if width <= 0.0 {
        return 0.0;
    }
    (f32::from(position.x - bounds.left()) / width).clamp(0.0, 1.0)
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let handle = focus(&self.id, window, cx);
        let state = pointer(&self.id, window, cx);
        let track_bounds = window.use_keyed_state(child(&self.id, "track"), cx, |_, _| {
            Bounds::<Pixels>::default()
        });
        let (hovered, pressed) = {
            let p = state.read(cx);
            (p.hovered, p.pressed)
        };
        let focused = handle.is_focused(window);
        let thumb = !self.disabled && (hovered || pressed || focused);
        let value = self.value;

        // Whether a drag that began on this slider is under way. Set and
        // cleared without a notify: nothing is drawn from it.
        let dragging = window.use_keyed_state(child(&self.id, "dragging"), cx, |_, _| false);
        if self.disabled {
            dragging.update(cx, |d, _| *d = false);
        }
        let measure = track_bounds.clone();
        let rail_id = child(&self.id, "rail");
        let drag_moves = (!self.disabled)
            .then(|| self.on_change.clone())
            .flatten()
            .map(|on_change| (on_change, dragging.clone()));
        let mut rail = div()
            .id(rail_id.clone())
            .relative()
            .flex_1()
            .h(HIT)
            .child(
                canvas(
                    move |bounds, _, cx| measure.update(cx, |b, _| *b = bounds),
                    // The drag follows the pointer past the track and the
                    // window until it is let go, through window-wide mouse
                    // listeners. Not GPUI's drag and drop: while one of
                    // those is active, every mouse move redraws the whole
                    // window past its caches.
                    move |bounds, _, window, _| {
                        let Some((on_change, dragging)) = drag_moves else {
                            return;
                        };
                        let (moving, ending, starting) =
                            (dragging.clone(), dragging.clone(), dragging);
                        // Any press starts afresh, before the rail's own
                        // handler sees it: a release the window never heard
                        // (let go outside it, on some platforms) must not
                        // leave the next drag elsewhere moving this slider.
                        window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                starting.update(cx, |d, _| *d = false);
                            }
                        });
                        // In the capture phase, which nothing painted over
                        // the slider can stop: the drag holds across a
                        // client-side window border, say.
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture
                                && event.pressed_button == Some(MouseButton::Left)
                                && *moving.read(cx)
                            {
                                on_change(&at(bounds, event.position), window, cx);
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture && event.button == MouseButton::Left
                            {
                                ending.update(cx, |d, _| *d = false);
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px((f32::from(HIT) - f32::from(HAIRLINE)) / 2.0))
                    .h(HAIRLINE)
                    .bg(look.palette.rule.hsla()),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .w(gpui_kit::relative(value))
                    .top(px((f32::from(HIT) - FILL) / 2.0))
                    .h(px(FILL))
                    .bg(look.accent.hsla()),
            )
            .when(thumb, |rail| {
                rail.child(
                    div()
                        .absolute()
                        .left(gpui_kit::relative(value))
                        .ml(px(-THUMB / 2.0))
                        .top(px((f32::from(HIT) - THUMB) / 2.0))
                        .size(px(THUMB))
                        .bg(look.palette.ink.hsla()),
                )
            });

        let end = |text: Option<SharedString>| {
            text.map(|text| {
                div()
                    .flex_none()
                    .text_role(TypeRole::Fine, &look)
                    .text_color(look.palette.muted.hsla())
                    .child(text)
            })
        };
        let mut slider = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .items_center()
            .gap(look.space(Space::S));
        if self.disabled {
            slider = slider.opacity(DISABLED_OPACITY);
        } else if let Some(on_change) = self.on_change {
            let (down, key) = (on_change.clone(), on_change);
            let down_bounds = track_bounds.clone();
            let step = self.step;
            rail = rail.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let bounds = *down_bounds.read(cx);
                dragging.update(cx, |d, _| *d = true);
                down(&at(bounds, event.position), window, cx);
            });
            slider = track(slider, &state)
                .track_focus(&handle)
                .cursor_pointer()
                .on_key_down(move |event, window, cx| {
                    let to = match event.keystroke.key.as_str() {
                        "left" | "down" => value - step,
                        "right" | "up" => value + step,
                        _ => return,
                    };
                    cx.stop_propagation();
                    key(&to.clamp(0.0, 1.0), window, cx);
                });
        }
        slider
            .children(end(self.low))
            .child(rail)
            .children(end(self.high))
            .children(ring(&handle, &look, window))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{
        AppContext, Context, Entity, Modifiers, MouseDownEvent, PlatformInput, Render,
        StyleRefinement, TestAppContext, point,
    };

    use super::*;

    struct Owner {
        value: f32,
    }

    impl Render for Owner {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let owner = cx.entity();
            div()
                .w_full()
                .h(px(60.))
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .child(
                            Slider::new("slider", self.value).on_change(move |value, _, cx| {
                                let value = *value;
                                owner.update(cx, |owner, cx| {
                                    owner.value = value;
                                    cx.notify();
                                })
                            }),
                        ),
                )
        }
    }

    struct Neighbour(Rc<Cell<usize>>);

    impl Render for Neighbour {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            div().size_full()
        }
    }

    struct Page {
        owner: Entity<Owner>,
        neighbour: Entity<Neighbour>,
    }

    impl Render for Page {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    self.owner
                        .clone()
                        .cached(StyleRefinement::default().w_full().h(px(60.))),
                )
                .child(
                    div().flex_1().child(
                        self.neighbour
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
                )
        }
    }

    /// A drag moves the slider and redraws its owner alone: the page around
    /// it replays. GPUI's drag and drop redraws the whole window past its
    /// caches at every move, which is what a slider must not use.
    #[gpui_kit::test]
    fn a_drag_redraws_only_the_slider(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let renders = Rc::new(Cell::new(0));
        let counted = renders.clone();
        let owner = Rc::new(Cell::new(None));
        let held = owner.clone();
        let window = cx.add_window(move |_, cx| {
            let slider = cx.new(|_| Owner { value: 0.1 });
            held.set(Some(slider.clone()));
            Page {
                owner: slider,
                neighbour: cx.new(|_| Neighbour(counted)),
            }
        });
        let at = |x: f32| point(px(x), px(30.));
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: at(40.),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
            window.draw(cx).clear(cx);
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: at(40.),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.draw(cx).clear(cx);
        })
        .unwrap();
        renders.set(0);
        for step in 1..=20 {
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at(40. + step as f32 * 30.),
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                window.draw(cx).clear(cx);
            })
            .unwrap();
        }
        let owner = owner.take().unwrap();
        let value = cx.update(|cx| owner.read(cx).value);
        assert!(value > 0.3, "the drag moved the slider, to {value}");
        assert_eq!(
            renders.get(),
            0,
            "the page beside the slider redrew during the drag"
        );

        // The release never reached the window; the next press and drag
        // are elsewhere, and leave the slider where it was.
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(300.), px(300.)),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.draw(cx).clear(cx);
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(40.), px(320.)),
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert_eq!(cx.update(|cx| owner.read(cx).value), value);
    }
}
