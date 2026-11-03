//! The motion playground: a value moved by one keyframe track (#1859), as a
//! component would move it. Pick what it moves, the easing (a Bézier with
//! handles to drag, a spring with its stiffness and damping), the length
//! and the repeat; play it on a sample at the display's rate, scrub it, see
//! its curve, and copy the call that makes it.

use gpui_kit::prelude::*;
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use super::StoryContent;
use crate::components::{
    Animated, AnimatedQuad, Button, ButtonKind, CopyText, Ease, Repeat, SectionHeader, Segmented,
    ShaderProgram, Slider, Stepper,
};
use crate::theme::KoanStyled;
use crate::theme::{HAIRLINE, Look, Space, Tone, TypeRole};

const PROPERTIES: [&str; 4] = ["position", "opacity", "scale", "colour"];
const EASES: [&str; 5] = ["linear", "in-out", "quick-out", "bézier", "spring"];
const REPEATS: [&str; 3] = ["once", "loop", "alternate"];

/// How far a plotted curve may stray past its ends: a spring or a Bézier
/// overshoots.
const PLOT_LOW: f32 = -0.25;
const PLOT_HIGH: f32 = 1.25;
/// Samples along a plotted curve.
const SAMPLES: usize = 96;

/// The playground's settings, and the track it plays.
pub struct State {
    property: usize,
    ease: usize,
    bezier: [f32; 4],
    stiffness: i64,
    damping: i64,
    length_ms: i64,
    repeat: usize,
    scrub: f32,
    preview: Animated,
    plot: Rc<Cell<Option<Bounds<Pixels>>>>,
    dragging: Option<usize>,
}

impl State {
    pub fn new() -> Self {
        let preview = Animated::new();
        preview.hold(0, 0.0);
        Self {
            property: 0,
            ease: 3,
            bezier: [0.42, 0.0, 0.58, 1.0],
            stiffness: 170,
            damping: 18,
            length_ms: 700,
            repeat: 2,
            scrub: 0.0,
            preview,
            plot: Rc::default(),
            dragging: None,
        }
    }

    fn ease(&self) -> Ease {
        match self.ease {
            0 => Ease::Linear,
            1 => Ease::InOut,
            2 => Ease::QuickOut,
            3 => {
                let [x1, y1, x2, y2] = self.bezier;
                Ease::Bezier(x1, y1, x2, y2)
            }
            _ => Ease::Spring {
                stiffness: self.stiffness as f32,
                damping: self.damping as f32,
            },
        }
    }

    fn repeat(&self) -> Repeat {
        match self.repeat {
            1 => Repeat::Loop,
            2 => Repeat::Alternate,
            _ => Repeat::Once,
        }
    }

    fn length(&self) -> Duration {
        Duration::from_millis(self.length_ms.max(1) as u64)
    }

    /// Plays the track from its start.
    fn play(&self) {
        self.preview.segment(
            0,
            0.0,
            1.0,
            Animated::now(),
            self.length(),
            self.ease(),
            self.repeat(),
        );
    }

    /// Holds the sample where the track stands `share` of the way through.
    fn scrub_to(&mut self, share: f32) {
        self.scrub = share;
        let now = Animated::now();
        let length = self.curve_length();
        self.preview.segment(
            0,
            0.0,
            1.0,
            now - length.mul_f32(share),
            length,
            self.ease(),
            Repeat::Once,
        );
        let value = self.preview.at(0, now);
        self.preview.hold(0, value);
    }

    /// How long one pass takes: a spring's own settling time, else the
    /// length chosen.
    fn curve_length(&self) -> Duration {
        let probe = Animated::new();
        probe.segment(
            0,
            0.0,
            1.0,
            Animated::now(),
            self.length(),
            self.ease(),
            Repeat::Once,
        );
        Duration::from_secs_f32(probe.track(0).length.max(0.001))
    }

    /// The curve, one pass, as values at even steps of time.
    fn curve(&self) -> Vec<f32> {
        let probe = Animated::new();
        let start = Animated::now();
        let length = self.curve_length();
        probe.segment(0, 0.0, 1.0, start, length, self.ease(), Repeat::Once);
        (0..=SAMPLES)
            .map(|i| probe.at(0, start + length.mul_f32(i as f32 / SAMPLES as f32)))
            .collect()
    }

    /// The call a component makes for this motion.
    fn code(&self) -> String {
        let ease = match self.ease() {
            Ease::Linear => "Ease::Linear".to_string(),
            Ease::InOut => "Ease::InOut".to_string(),
            Ease::QuickOut => "Ease::QuickOut".to_string(),
            Ease::Bezier(x1, y1, x2, y2) => {
                format!("Ease::Bezier({x1:.2}, {y1:.2}, {x2:.2}, {y2:.2})")
            }
            Ease::Spring { stiffness, damping } => {
                format!("Ease::Spring {{ stiffness: {stiffness:.0}, damping: {damping:.0} }}")
            }
        };
        let repeat = ["Repeat::Once", "Repeat::Loop", "Repeat::Alternate"][self.repeat];
        format!(
            "animated.segment(0, 0.0, 1.0, Animated::now(), Duration::from_millis({}), {ease}, {repeat});",
            self.length_ms
        )
    }

    /// Where a press or drag at `at` puts Bézier handle `handle`, in the
    /// plot's terms.
    fn drag_handle(&mut self, handle: usize, at: Point<Pixels>) {
        let Some(bounds) = self.plot.get() else {
            return;
        };
        let x = ((at.x - bounds.origin.x) / bounds.size.width).clamp(0.0, 1.0);
        let up = 1.0 - (at.y - bounds.origin.y) / bounds.size.height;
        let y = PLOT_LOW + up * (PLOT_HIGH - PLOT_LOW);
        self.bezier[handle * 2] = x;
        self.bezier[handle * 2 + 1] = y.clamp(PLOT_LOW, PLOT_HIGH);
    }

    /// The handle nearest `at`, within reach of it.
    fn handle_at(&self, at: Point<Pixels>) -> Option<usize> {
        let bounds = self.plot.get()?;
        (0..2)
            .map(|h| {
                let p = plot_point(bounds, self.bezier[h * 2], self.bezier[h * 2 + 1]);
                (h, f32::from((p.x - at.x).abs() + (p.y - at.y).abs()))
            })
            .filter(|(_, d)| *d < 24.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
    }
}

/// Where `(x, y)` of the curve lands in the plot.
fn plot_point(bounds: Bounds<Pixels>, x: f32, y: f32) -> Point<Pixels> {
    let up = (y - PLOT_LOW) / (PLOT_HIGH - PLOT_LOW);
    point(
        bounds.origin.x + bounds.size.width * x,
        bounds.origin.y + bounds.size.height * (1.0 - up),
    )
}

/// The sample the track moves: a square sliding, fading, growing or
/// changing colour. Uniforms: 0 which, 1 device pixels to a point; track 0
/// the value.
static PREVIEW: ShaderProgram = ShaderProgram {
    name: "koan-playground",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float s = u[1];
    float v = koan_track(u, 0, time);
    int which = int(u[0]);
    float side = 36.0 * s;
    float pad = 12.0 * s;
    float2 c = size * 0.5;
    float scale = 1.0;
    float alpha = 1.0;
    float3 colour = float3(1.0, 0.18, 0.6);
    if (which == 0) {
        c.x = mix(pad + side * 0.5, size.x - pad - side * 0.5, v);
    } else if (which == 1) {
        alpha = clamp(v, 0.0, 1.0);
    } else if (which == 2) {
        scale = max(0.3 + 0.9 * v, 0.0);
    } else {
        colour = mix(float3(1.0, 0.18, 0.6), float3(0.0, 0.78, 1.0), clamp(v, 0.0, 1.0));
    }
    float2 q = abs(position - c) - side * 0.5 * scale;
    float inside = clamp(0.5 - max(q.x, q.y), 0.0, 1.0);
    float lane = (which == 0 && abs(position.y - size.y * 0.5) < 0.5 * s) ? 0.2 : 0.0;
    return float4(colour, max(inside * alpha, lane));
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let s = u[0].y;
    let v = koan_track(u, 0u, time);
    let which = i32(u[0].x);
    let side = 36.0 * s;
    let pad = 12.0 * s;
    var c = size * 0.5;
    var scale = 1.0;
    var alpha = 1.0;
    var colour = vec3<f32>(1.0, 0.18, 0.6);
    if (which == 0) {
        c.x = mix(pad + side * 0.5, size.x - pad - side * 0.5, v);
    } else if (which == 1) {
        alpha = clamp(v, 0.0, 1.0);
    } else if (which == 2) {
        scale = max(0.3 + 0.9 * v, 0.0);
    } else {
        colour = mix(vec3<f32>(1.0, 0.18, 0.6), vec3<f32>(0.0, 0.78, 1.0), clamp(v, 0.0, 1.0));
    }
    let q = abs(position - c) - side * 0.5 * scale;
    let inside = clamp(0.5 - max(q.x, q.y), 0.0, 1.0);
    let lane = select(0.0, 0.2, which == 0 && abs(position.y - size.y * 0.5) < 0.5 * s);
    return vec4<f32>(colour, max(inside * alpha, lane));
}
"#,
    prepare: None,
};

impl StoryContent {
    pub(super) fn playground(
        &mut self,
        look: &Look,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let entity = cx.entity();
        let state = &self.playground;
        state
            .preview
            .set_all(0, &[state.property as f32, window.scale_factor()]);
        let change = |f: fn(&mut State, usize)| {
            let entity = entity.clone();
            move |index: &usize, _: &mut Window, cx: &mut App| {
                let index = *index;
                entity.update(cx, |story, cx| {
                    f(&mut story.playground, index);
                    story.playground.play();
                    cx.notify();
                })
            }
        };
        let step = |f: fn(&mut State, i64)| {
            let entity = entity.clone();
            move |value: &i64, _: &mut Window, cx: &mut App| {
                let value = *value;
                entity.update(cx, |story, cx| {
                    f(&mut story.playground, value);
                    story.playground.play();
                    cx.notify();
                })
            }
        };
        let row = || {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(look.space(Space::M))
        };

        // The curve, plotted as dots; the Bézier's handles and their arms.
        let samples = state.curve();
        let bezier = (state.ease == 3).then_some(state.bezier);
        let plot_bounds = state.plot.clone();
        let (accent, muted, rule) = (
            look.accent.hsla(),
            look.palette.muted.hsla(),
            look.palette.rule.hsla(),
        );
        let dot = look.spaced(2.0);
        let knob = look.spaced(10.0);
        let plot = canvas(
            move |bounds, _, _| plot_bounds.set(Some(bounds)),
            move |bounds, _, window, _| {
                for y in [0.0, 1.0] {
                    let a = plot_point(bounds, 0.0, y);
                    window.paint_quad(fill(Bounds::new(a, size(bounds.size.width, px(1.0))), rule));
                }
                for (i, v) in samples.iter().enumerate() {
                    let p = plot_point(bounds, i as f32 / SAMPLES as f32, *v);
                    window.paint_quad(fill(
                        Bounds::new(point(p.x - dot / 2.0, p.y - dot / 2.0), size(dot, dot)),
                        accent,
                    ));
                }
                if let Some([x1, y1, x2, y2]) = bezier {
                    for (from, (x, y)) in [((0.0, 0.0), (x1, y1)), ((1.0, 1.0), (x2, y2))] {
                        let a = plot_point(bounds, from.0, from.1);
                        let b = plot_point(bounds, x, y);
                        for i in 0..=24 {
                            let t = i as f32 / 24.0;
                            let p = point(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(p.x - px(0.5), p.y - px(0.5)),
                                    size(px(1.0), px(1.0)),
                                ),
                                muted,
                            ));
                        }
                        window.paint_quad(fill(
                            Bounds::new(
                                point(b.x - knob / 2.0, b.y - knob / 2.0),
                                size(knob, knob),
                            ),
                            muted,
                        ));
                    }
                }
            },
        )
        .size_full();
        let (down, moved, up) = (entity.clone(), entity.clone(), entity.clone());
        let plot = div()
            .id("playground-plot")
            .w(look.spaced(420.0))
            .h(look.spaced(220.0))
            .border(HAIRLINE)
            .border_color(look.palette.rule.hsla())
            .child(plot)
            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                down.update(cx, |story, cx| {
                    let state = &mut story.playground;
                    if state.ease == 3 {
                        state.dragging = state.handle_at(event.position);
                        if let Some(handle) = state.dragging {
                            state.drag_handle(handle, event.position);
                            cx.notify();
                        }
                    }
                })
            })
            .on_mouse_move(move |event, _, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                moved.update(cx, |story, cx| {
                    if let Some(handle) = story.playground.dragging {
                        story.playground.drag_handle(handle, event.position);
                        cx.notify();
                    }
                })
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                up.update(cx, |story, cx| {
                    if story.playground.dragging.take().is_some() {
                        story.playground.play();
                        cx.notify();
                    }
                })
            });

        let play = {
            let entity = entity.clone();
            Button::new("playground-play", ButtonKind::Prominent)
                .label("play")
                .on_click(move |_, _, cx| entity.read(cx).playground.play())
        };
        let scrub = {
            let entity = entity.clone();
            Slider::new("playground-scrub", state.scrub).on_change(move |share, _, cx| {
                let share = *share;
                entity.update(cx, |story, cx| {
                    story.playground.scrub_to(share);
                    cx.notify();
                })
            })
        };
        let text = |content: String, role: TypeRole, tone: Tone| {
            div()
                .text_role(role, look)
                .text_color(look.text_color(tone))
                .child(content)
                .into_any_element()
        };
        let mut v = vec![
            SectionHeader::new("What moves").into_any_element(),
            Segmented::new("playground-property", PROPERTIES, state.property)
                .on_change(change(|s, i| s.property = i))
                .into_any_element(),
            SectionHeader::new("The easing").into_any_element(),
            Segmented::new("playground-ease", EASES, state.ease)
                .on_change(change(|s, i| s.ease = i))
                .into_any_element(),
        ];
        if state.ease == 4 {
            v.push(
                row()
                    .child(
                        Stepper::new(
                            "playground-stiffness",
                            "stiffness",
                            state.stiffness,
                            20..=600,
                        )
                        .shown(state.stiffness.to_string())
                        .on_change(step(|s, x| s.stiffness = x)),
                    )
                    .child(
                        Stepper::new("playground-damping", "damping", state.damping, 2..=80)
                            .shown(state.damping.to_string())
                            .on_change(step(|s, x| s.damping = x)),
                    )
                    .into_any_element(),
            );
        }
        if state.ease == 3 {
            let [x1, y1, x2, y2] = state.bezier;
            v.push(text(
                format!("cubic-bezier({x1:.2}, {y1:.2}, {x2:.2}, {y2:.2}): drag the handles"),
                TypeRole::Meta,
                Tone::Muted,
            ));
        }
        v.extend([
            plot.into_any_element(),
            SectionHeader::new("Timing").into_any_element(),
            row()
                .child(
                    Stepper::new("playground-length", "length", state.length_ms, 50..=3000)
                        .shown(format!("{} ms", state.length_ms))
                        .on_change(step(|s, x| s.length_ms = (x / 50) * 50)),
                )
                .child(
                    Segmented::new("playground-repeat", REPEATS, state.repeat)
                        .on_change(change(|s, i| s.repeat = i)),
                )
                .child(play)
                .into_any_element(),
            SectionHeader::new("On a sample, at the display's rate").into_any_element(),
            div()
                .w(look.spaced(420.0))
                .h(look.spaced(72.0))
                .border(HAIRLINE)
                .border_color(look.palette.rule.hsla())
                .child(AnimatedQuad::new(&PREVIEW, &state.preview).size_full())
                .into_any_element(),
            div().w(look.spaced(420.0)).child(scrub).into_any_element(),
            SectionHeader::new("The call").into_any_element(),
            CopyText::new("playground-code", state.code())
                .help("copy the call")
                .into_any_element(),
        ]);
        v
    }
}
