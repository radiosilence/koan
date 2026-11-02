use std::sync::Arc;
use std::time::Instant;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Bounds, DispatchPhase, ElementId, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, RenderOnce,
    SharedString, Styled, Window, canvas, div, px,
};

use super::{Animated, SHADED, ShaderProgram};

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
    /// The desktop's density: a 20-point rail, as the Mac's seek bar has.
    compact: bool,
    on_change: Option<Handler<f32>>,
    on_commit: Option<Handler<()>>,
    glide: Option<Glide>,
}

/// A value that moves on its own between renders, as a seek bar's playhead
/// does: the share of the track at a moment and how fast it moves from it.
/// Where the renderer draws shaders, the fill and the thumb are a shader quad
/// reckoning the value per present from these, so the slider glides with no
/// frame drawn and no timer; elsewhere, and while dragged, the slider draws
/// `value` as given. The owner writes it from any thread, keeps it across
/// renders, and stops it (`set_moving(false)`) when the value rests or
/// nothing shows it.
#[derive(Clone, Default)]
pub struct Glide(
    Animated,
    Arc<parking_lot::Mutex<Option<(f32, f32, Instant)>>>,
);

/// How long a small correction takes to ease out, and the largest one eased
/// rather than jumped, in seconds of play: a little over what a late report
/// leaves between the reckoning and the report. A skip or a seek is longer
/// than this however long the track.
const SETTLE: f32 = 0.25;
const DRIFT: f32 = 0.3;

impl Glide {
    pub fn new() -> Self {
        Self::default()
    }

    /// `value` at `since`, moving `rate` of the track a second.
    /// A new anchor that disagrees a little with where the slider had
    /// reckoned it, while playing on at the same rate, is eased into over
    /// `SETTLE` rather than jumped to: the fill moves continuously and
    /// corrects without a step. A seek, a pause or any larger difference
    /// moves at once.
    pub fn set(&self, value: f32, rate: f32, since: Instant) {
        let now = Instant::now();
        let at = |(value, rate, since): (f32, f32, Instant)| {
            let t = if now >= since {
                now.duration_since(since).as_secs_f32()
            } else {
                -since.duration_since(now).as_secs_f32()
            };
            (value + rate * t).clamp(0.0, 1.0)
        };
        let next = (value, rate, since);
        let previous = self.1.lock().replace(next);
        let offset = match previous {
            Some(previous) if previous != next && previous.1 == rate && rate > 0.0 => {
                let offset = at(previous) - at(next);
                if offset.abs() <= rate * DRIFT { offset } else { 0.0 }
            }
            _ => 0.0,
        };
        let start = if now >= since {
            now.duration_since(since).as_secs_f32()
        } else {
            -since.duration_since(now).as_secs_f32()
        };
        self.0.set_all(0, &[value, rate]);
        if previous != Some(next) {
            self.0.set_all(5, &[offset, start, SETTLE]);
        }
        self.0.set_origin(since);
    }

    /// Whether the value is moving: presented every refresh while it is.
    pub fn set_moving(&self, moving: bool) {
        self.0.run(moving);
    }

    /// The fill, thumb and their colours, in device pixels.
    fn style(&self, look: &Style) {
        self.0.set_all(2, &[look.fill, look.thumb, look.inset]);
        for (i, color) in [look.fill_color, look.thumb_color].into_iter().enumerate() {
            let rgba = color.to_rgb();
            self.0.set_all(8 + i * 4, &[rgba.r, rgba.g, rgba.b, rgba.a]);
        }
    }
}

struct Style {
    fill: f32,
    /// The thumb's side, or 0 while it is hidden.
    thumb: f32,
    /// How far the quad reaches past the rail either side, for the thumb.
    inset: f32,
    fill_color: Hsla,
    thumb_color: Hsla,
}

/// The fill and thumb as a fragment program. Uniforms: 0 the value at the
/// origin, 1 its rate a second, 2 the fill's height, 3 the thumb's side (0
/// hidden), 4 the inset, in device pixels; 5 a correction still easing out,
/// 6 when it began and 7 how long it takes, in seconds of shader time; 8..12
/// the fill's colour, 12..16 the thumb's.
static GLIDE: ShaderProgram = ShaderProgram {
    name: "koan-slider-glide",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float width = size.x - 2.0 * u[4];
    float x = position.x - u[4];
    float easing = u[7] > 0.0 ? 1.0 - smoothstep(0.0, 1.0, (time - u[6]) / u[7]) : 0.0;
    float end = clamp(u[0] + u[1] * time + u[5] * easing, 0.0, 1.0) * width;
    float dy = abs(position.y - size.y * 0.5);
    if (u[3] > 0.0 && abs(x - end) <= u[3] * 0.5 && dy <= u[3] * 0.5) {
        return float4(u[12], u[13], u[14], u[15]);
    }
    if (x < 0.0 || dy > u[2] * 0.5) {
        return float4(0.0);
    }
    float cover = clamp(end - x + 0.5, 0.0, 1.0);
    return float4(u[8], u[9], u[10], u[11] * cover);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>) -> vec4<f32> {
    let width = size.x - 2.0 * u[1].x;
    let x = position.x - u[1].x;
    var easing = 0.0;
    if (u[1].w > 0.0) {
        easing = 1.0 - smoothstep(0.0, 1.0, (time - u[1].z) / u[1].w);
    }
    let end = clamp(u[0].x + u[0].y * time + u[1].y * easing, 0.0, 1.0) * width;
    let dy = abs(position.y - size.y * 0.5);
    if (u[0].w > 0.0 && abs(x - end) <= u[0].w * 0.5 && dy <= u[0].w * 0.5) {
        return u[3];
    }
    if (x < 0.0 || dy > u[0].z * 0.5) {
        return vec4<f32>(0.0);
    }
    let cover = clamp(end - x + 0.5, 0.0, 1.0);
    return vec4<f32>(u[2].xyz, u[2].w * cover);
}
"#,
    prepare: None,
};

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
            compact: false,
            on_change: None,
            on_commit: None,
            glide: None,
        }
    }

    /// The value moves on its own from `value` as `glide` says; see `Glide`.
    pub fn glide(mut self, glide: &Glide) -> Self {
        self.glide = Some(glide.clone());
        self
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

    /// At the desktop's density, 20 points tall rather than the touch
    /// target's 44: a transport's seek bar under its controls.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(&f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }

    /// Called once a change is finished: a drag let go, or an arrow key's
    /// step. For a value too costly to apply at every step of a drag.
    pub fn on_commit(mut self, handler: impl Fn(&(), &mut Window, &mut App) + 'static) -> Self {
        self.on_commit = Some(Arc::new(handler));
        self
    }
}

const THUMB: f32 = 8.0;
/// A compact slider's rail.
const COMPACT: Pixels = px(20.0);
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
        let height = if self.compact { COMPACT } else { HIT };
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
        let glide = self.glide.clone().filter(|_| SHADED);

        // Whether a drag that began on this slider is under way. Set and
        // cleared without a notify: nothing is drawn from it.
        let dragging = window.use_keyed_state(child(&self.id, "dragging"), cx, |_, _| false);
        let measure = track_bounds.clone();
        let rail_id = child(&self.id, "rail");
        let drag_moves = (!self.disabled)
            .then(|| self.on_change.clone())
            .flatten()
            .map(|on_change| (on_change, self.on_commit.clone(), dragging.clone()));
        let mut rail = div()
            .id(rail_id.clone())
            .relative()
            .flex_1()
            .h(height)
            .child(
                canvas(
                    move |bounds, _, cx| measure.update(cx, |b, _| *b = bounds),
                    // The drag follows the pointer past the track and the
                    // window until it is let go, through window-wide mouse
                    // listeners. Not GPUI's drag and drop: while one of
                    // those is active, every mouse move redraws the whole
                    // window past its caches.
                    move |bounds, _, window, _| {
                        let Some((on_change, on_commit, dragging)) = drag_moves else {
                            return;
                        };
                        let (moving, ending, starting) =
                            (dragging.clone(), dragging.clone(), dragging);
                        // A left press starts afresh, before the rail's own
                        // handler sees it: a release the window never heard
                        // (let go outside it, on some platforms) must not
                        // leave the next drag elsewhere moving this slider.
                        let lost = on_commit.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture
                                && event.button == MouseButton::Left
                                && starting.update(cx, |d, _| std::mem::replace(d, false))
                                && let Some(commit) = &lost
                            {
                                // The drag ended where the window did not
                                // hear it: what it reached is committed.
                                commit(&(), window, cx);
                            }
                        });
                        // In the capture phase, which runs in paint order:
                        // what is painted over the slider, such as a
                        // client-side window border, cannot stop it. A move
                        // that leaves the value where it was (past either
                        // end) is not passed on.
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture
                                && event.pressed_button == Some(MouseButton::Left)
                                && *moving.read(cx)
                            {
                                let to = at(bounds, event.position);
                                if to != value {
                                    on_change(&to, window, cx);
                                }
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture
                                && event.button == MouseButton::Left
                                && ending.update(cx, |d, _| std::mem::replace(d, false))
                                && let Some(on_commit) = &on_commit
                            {
                                on_commit(&(), window, cx);
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
                    .top(px((f32::from(height) - f32::from(HAIRLINE)) / 2.0))
                    .h(HAIRLINE)
                    .bg(look.palette.rule.hsla()),
            )
            .when(glide.is_none(), |rail| {
                rail.child(
                    div()
                        .absolute()
                        .left_0()
                        .w(gpui_kit::relative(value))
                        .top(px((f32::from(height) - FILL) / 2.0))
                        .h(px(FILL))
                        .bg(look.accent.hsla()),
                )
            })
            .when(glide.is_none() && thumb, |rail| {
                rail.child(
                    div()
                        .absolute()
                        .left(gpui_kit::relative(value))
                        .ml(px(-THUMB / 2.0))
                        .top(px((f32::from(height) - THUMB) / 2.0))
                        .size(px(THUMB))
                        .bg(look.palette.ink.hsla()),
                )
            })
            .when_some(glide, |rail, glide| {
                let (fill_color, thumb_color) = (look.accent.hsla(), look.palette.ink.hsla());
                rail.child(
                    canvas(
                        move |_, window, cx| {
                            let scale = window.scale_factor();
                            glide.style(&Style {
                                fill: FILL * scale,
                                thumb: if thumb { THUMB * scale } else { 0.0 },
                                inset: THUMB / 2.0 * scale,
                                fill_color,
                                thumb_color,
                            });
                            glide.0.attach(false, window, cx);
                            glide
                        },
                        |bounds, glide, window, _| glide.0.paint(&GLIDE, None, bounds, window),
                    )
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(-THUMB / 2.0))
                    .right(px(-THUMB / 2.0)),
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
            let key_commit = self.on_commit;
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
                    if let Some(commit) = &key_commit {
                        commit(&(), window, cx);
                    }
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
        AnyWindowHandle, AppContext, Context, Entity, Modifiers, MouseDownEvent, PlatformInput,
        Render, StyleRefinement, TestAppContext, point,
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

    fn down(x: f32, y: f32) -> PlatformInput {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x), px(y)),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        })
    }

    fn held(x: f32, y: f32) -> PlatformInput {
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x), px(y)),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        })
    }

    fn up(x: f32, y: f32) -> PlatformInput {
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(x), px(y)),
            modifiers: Modifiers::default(),
            click_count: 1,
        })
    }

    /// Dispatches each event and draws after it, as the window would.
    fn send(cx: &mut TestAppContext, window: AnyWindowHandle, events: Vec<PlatformInput>) {
        cx.update_window(window, |_, window, cx| {
            for event in events {
                window.dispatch_event(event, cx);
                window.draw(cx).clear(cx);
            }
        })
        .unwrap();
    }

    /// A drag moves the slider and redraws its owner alone: the page around
    /// it replays. It ends when the button is let go, or, where the release
    /// never reached the window, at the next press.
    #[gpui_kit::test]
    fn a_drag_redraws_only_the_slider(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let renders = Rc::new(Cell::new(0));
        let counted = renders.clone();
        let made = Rc::new(Cell::new(None));
        let held_owner = made.clone();
        let handle = cx.add_window(move |_, cx| {
            let slider = cx.new(|_| Owner { value: 0.1 });
            held_owner.set(Some(slider.clone()));
            Page {
                owner: slider,
                neighbour: cx.new(|_| Neighbour(counted)),
            }
        });
        let window: AnyWindowHandle = handle.into();
        let owner = made.take().unwrap();
        let value = |cx: &mut TestAppContext| cx.update(|cx| owner.read(cx).value);
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        send(
            cx,
            window,
            vec![
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(40.), px(30.)),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                down(40., 30.),
            ],
        );

        renders.set(0);
        send(
            cx,
            window,
            (1..=20)
                .map(|step| held(40. + step as f32 * 30., 30.))
                .collect(),
        );
        let dragged = value(cx);
        assert!(dragged > 0.3, "the drag moved the slider, to {dragged}");
        assert_eq!(
            renders.get(),
            0,
            "the page beside the slider redrew during the drag"
        );

        // The release never reached the window; the next press is elsewhere,
        // and a drag from it leaves the slider where it was.
        send(cx, window, vec![down(300., 300.), held(100., 320.)]);
        assert_eq!(value(cx), dragged);

        // A drag on the slider, let go: a move with the button held again,
        // from a press the window did not see, leaves it where it was.
        send(
            cx,
            window,
            vec![down(100., 30.), held(200., 30.), up(200., 30.)],
        );
        let released = value(cx);
        assert!(released < dragged, "the second drag moved the slider back");
        send(cx, window, vec![held(500., 30.)]);
        assert_eq!(value(cx), released);
    }
}
