//! A control answering what was done to it: a small mark drawn by the GPU
//! around or under the control for a moment, then gone. The kit's controls
//! call it from their own handlers (a button pressed, a toggle turned, a
//! heart filled, something copied), so every page that uses them answers
//! alike with no code of its own.
//!
//! Subtle by rule: one shader quad over the control and a little past its
//! edge, in the accent at low opacity, never over the control's text, for
//! no longer than `Motion::SETTLE`. Its progress is one track the GPU moves,
//! and the quad leaves the scene when it ends, so a control at rest, or off
//! screen, costs nothing. Under Reduce Motion nothing is drawn.

use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Animated, AnimatedQuad, Ease, Repeat, SHADED, child};
use crate::theme::{KoanTheme, Motion};

/// What an answer draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// One ring leaving the control's outline: a button pressed, a toggle
    /// turned on.
    Ring,
    /// Six dots leaving the control and fading: a heart filled.
    Burst,
    /// One ring closing in on the control: a heart emptied.
    Close,
    /// A line drawn under the control from its leading edge: something
    /// copied, something arrived at a destination.
    Sweep,
    /// The control's ground lifting to the accent and settling back: a
    /// bordered button pressed, rows landing.
    Lift,
    /// A short streak leaving the glyph the way it points: a skip, forward
    /// (`true`) or back.
    Streak(bool),
}

impl Answer {
    fn code(self) -> f32 {
        match self {
            Answer::Ring => 0.0,
            Answer::Burst => 1.0,
            Answer::Close => 2.0,
            Answer::Sweep => 3.0,
            Answer::Lift => 4.0,
            Answer::Streak(_) => 5.0,
        }
    }

    /// Which way it travels, for a streak: 1 forward, -1 back.
    fn heading(self) -> f32 {
        match self {
            Answer::Streak(false) => -1.0,
            _ => 1.0,
        }
    }

    /// How long it runs: a ring, out or in, is a press's echo and as quick
    /// as a sleeve arriving; what travels (dots, a line drawn the width of
    /// a row) or lifts a ground takes the longest token.
    fn duration(self) -> Duration {
        match self {
            Answer::Ring | Answer::Close | Answer::Streak(_) => Motion::ARRIVE,
            Answer::Burst | Answer::Sweep | Answer::Lift => Motion::SETTLE,
        }
    }

    /// How what it draws travels: out from the control decisively and
    /// settling (quick-out); a ring closing in eases both ways, as a
    /// gathering rather than a throw.
    fn ease(self) -> Ease {
        match self {
            Answer::Close => Ease::InOut,
            _ => Ease::QuickOut,
        }
    }

    /// How far past the control's edge the answer reaches, in points.
    fn spill(self) -> f32 {
        match self {
            Answer::Ring | Answer::Close => 6.0,
            Answer::Burst | Answer::Streak(_) => 9.0,
            Answer::Sweep | Answer::Lift => 0.0,
        }
    }
}

/// The answers as one program. Uniforms: 0 the answer, 1 which way a streak
/// goes, 2..6 the colour, 6
/// the spill and 7 a point, in device pixels; track 0 the answer's time
/// from 0 to 1, which its fade follows, and track 1 its travel, on the
/// answer's own curve.
pub static ANSWER: ShaderProgram = ShaderProgram {
    name: "koan-answer",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float t = clamp(koan_track(u, 0, time), 0.0, 1.0);
    if (t >= 1.0) { return float4(0.0); }
    float e = clamp(koan_track(u, 1, time), 0.0, 1.0);
    float spill = u[6];
    float pt = u[7];
    float2 inner = size - 2.0 * spill;
    float2 c = size * 0.5;
    float half_side = 0.5 * min(inner.x, inner.y);
    float d = length(position - c);
    float a = 0.0;
    int kind = int(u[0] + 0.5);
    if (kind == 0) {
        float r = half_side + spill * 0.9 * e;
        a = 0.35 * (1.0 - t) * clamp(0.75 * pt - abs(d - r) + 0.5, 0.0, 1.0);
    } else if (kind == 1) {
        float r = mix(half_side * 0.9, half_side + spill * 0.9, e);
        float dot = 1.2 * pt * (1.0 - 0.5 * t);
        for (int i = 0; i < 6; i++) {
            float ang = 0.5236 + float(i) * 1.0472;
            float2 p = c + r * float2(cos(ang), sin(ang));
            a = max(a, clamp(dot - length(position - p) + 0.5, 0.0, 1.0));
        }
        a *= 0.6 * (1.0 - t);
    } else if (kind == 2) {
        float r = mix(half_side + spill * 0.9, half_side * 0.8, e);
        a = 0.3 * (1.0 - t) * clamp(0.75 * pt - abs(d - r) + 0.5, 0.0, 1.0);
    } else if (kind == 3) {
        float line = size.y - pt;
        bool under = position.y >= line && position.x <= size.x * e;
        a = under ? 0.6 * (1.0 - t * t) : 0.0;
    } else if (kind == 5) {
        float x = (position.x - c.x) * u[1];
        float half_w = 0.5 * inner.x;
        float head = half_w + spill * e;
        float tail = mix(half_w, head, t);
        float across = clamp(0.75 * pt - abs(position.y - c.y) + 0.5, 0.0, 1.0);
        a = (x >= tail && x <= head) ? 0.45 * (1.0 - t) * across : 0.0;
    } else {
        a = 0.12 * (1.0 - e);
    }
    return float4(u[2], u[3], u[4], u[5] * a);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let t = clamp(koan_track(u, 0u, time), 0.0, 1.0);
    if (t >= 1.0) { return vec4<f32>(0.0); }
    let e = clamp(koan_track(u, 1u, time), 0.0, 1.0);
    let spill = u[1].z;
    let pt = u[1].w;
    let inner = size - 2.0 * spill;
    let c = size * 0.5;
    let half_side = 0.5 * min(inner.x, inner.y);
    let d = length(position - c);
    var a = 0.0;
    let kind = i32(u[0].x + 0.5);
    if (kind == 0) {
        let r = half_side + spill * 0.9 * e;
        a = 0.35 * (1.0 - t) * clamp(0.75 * pt - abs(d - r) + 0.5, 0.0, 1.0);
    } else if (kind == 1) {
        let r = mix(half_side * 0.9, half_side + spill * 0.9, e);
        let dot_r = 1.2 * pt * (1.0 - 0.5 * t);
        for (var i = 0; i < 6; i++) {
            let ang = 0.5236 + f32(i) * 1.0472;
            let p = c + r * vec2<f32>(cos(ang), sin(ang));
            a = max(a, clamp(dot_r - length(position - p) + 0.5, 0.0, 1.0));
        }
        a = a * 0.6 * (1.0 - t);
    } else if (kind == 2) {
        let r = mix(half_side + spill * 0.9, half_side * 0.8, e);
        a = 0.3 * (1.0 - t) * clamp(0.75 * pt - abs(d - r) + 0.5, 0.0, 1.0);
    } else if (kind == 3) {
        let line = size.y - pt;
        if (position.y >= line && position.x <= size.x * e) {
            a = 0.6 * (1.0 - t * t);
        }
    } else if (kind == 5) {
        let x = (position.x - c.x) * u[0].y;
        let half_w = 0.5 * inner.x;
        let head = half_w + spill * e;
        let tail = mix(half_w, head, t);
        let across = clamp(0.75 * pt - abs(position.y - c.y) + 0.5, 0.0, 1.0);
        if (x >= tail && x <= head) {
            a = 0.45 * (1.0 - t) * across;
        }
    } else {
        a = 0.12 * (1.0 - e);
    }
    let rgba = vec4<f32>(u[0].z, u[0].w, u[1].x, u[1].y);
    return vec4<f32>(rgba.xyz, rgba.w * a);
}
"#,
    prepare: None,
};

/// A control's answer while one is drawn.
#[derive(Default)]
struct State {
    animated: Animated,
    live: Option<Answer>,
    /// When the control's own handler last started an answer: the change it
    /// answered is not answered again as the control draws it, once.
    own: Option<Instant>,
    /// The mark `follow` was last given.
    seen: Option<u64>,
    _end: Option<Task<()>>,
}

/// A control's answers, taken as the control is drawn and moved into its
/// handlers: keyed state found again from a handler would not be the
/// control's own.
#[derive(Clone)]
pub struct Answerer(Entity<State>);

impl Answerer {
    /// The answers of the control `id`.
    pub fn of(id: &ElementId, window: &mut Window, cx: &mut App) -> Self {
        Self(window.use_keyed_state(child(id, "answer"), cx, |_, _| State::default()))
    }

    /// Starts `answer` in `color`, from the control's own handler. Nothing
    /// happens under Reduce Motion or where the renderer draws no shaders.
    pub fn start(&self, answer: Answer, color: Hsla, cx: &mut App) {
        self.0.update(cx, |s, _| s.own = Some(Instant::now()));
        self.run(answer, color, cx);
    }

    /// Draws `answer` in `color`.
    fn run(&self, answer: Answer, color: Hsla, cx: &mut App) {
        if !SHADED || !KoanTheme::motion_allowed(cx) {
            return;
        }
        self.0.update(cx, |s, cx| {
            s.animated.set_all(0, &[answer.code(), answer.heading()]);
            s.animated.set_color(2, color);
            let at = Animated::now();
            for (k, ease) in [(0, Ease::Linear), (1, answer.ease())] {
                s.animated
                    .segment(k, 0.0, 1.0, at, answer.duration(), ease, Repeat::Once);
            }
            s.live = Some(answer);
            s._end = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(answer.duration()).await;
                // The quad leaves the scene once the answer has run.
                this.update(cx, |s, cx| {
                    s.live = None;
                    cx.notify();
                })
                .ok();
            }));
            cx.notify();
        });
    }

    /// Answers a change the control is told of rather than one done to it
    /// (a favourite set from a shortcut, music arriving from a menu), as
    /// the control draws: `mark` names the state, and a draw that finds it
    /// moved starts `answer`. The first draw only notes it, so a control
    /// scrolled in or drawn fresh shows its state still, and a change the
    /// control answered from its own handler a moment ago is not answered
    /// twice.
    pub fn follow(
        &self,
        mark: u64,
        answer: Answer,
        color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.moved(mark, false, answer, color, window, cx);
    }

    /// Notes `mark` without answering: a state the control answers only one
    /// way into (an option becoming the chosen one), passed while it stands
    /// in the other, so the next move in answers.
    pub fn note(&self, mark: u64, cx: &mut App) {
        self.0.update(cx, |s, _| s.seen = Some(mark));
    }

    /// As `follow`, for something whose appearing is the news (a notice):
    /// its first draw answers too.
    pub fn announce(
        &self,
        mark: u64,
        answer: Answer,
        color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.moved(mark, true, answer, color, window, cx);
    }

    fn moved(
        &self,
        mark: u64,
        first: bool,
        answer: Answer,
        color: Hsla,
        window: &mut Window,
        cx: &mut App,
    ) {
        let s = self.0.read(cx);
        let seen = s.seen;
        if seen == Some(mark) {
            return;
        }
        // The change the control's own handler just answered, once; any
        // other change, however soon after, is answered.
        let answered = self.0.update(cx, |s, _| {
            s.seen = Some(mark);
            s.own
                .take()
                .is_some_and(|at| at.elapsed() < answer.duration())
        });
        if (first || seen.is_some()) && !answered {
            self.run(answer, color, cx);
            window.request_animation_frame();
        }
    }

    /// Whether an answer is being drawn.
    pub fn live(&self, cx: &App) -> bool {
        self.0.read(cx).live.is_some()
    }

    /// The mark last followed or noted.
    pub(crate) fn seen(&self, cx: &App) -> Option<u64> {
        self.0.read(cx).seen
    }

    /// The answer being drawn, as a quad over the control and a little past
    /// it, for the control to add as its last child; nothing at rest. The
    /// control is `relative`.
    pub fn layer(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let s = self.0.read(cx);
        let answer = s.live?;
        let look = crate::theme::Look::of(window, cx);
        let spill = look.spaced(answer.spill());
        let scale = window.scale_factor();
        s.animated
            .set_all(6, &[f32::from(spill) * scale, scale * look.scale.space]);
        Some(
            AnimatedQuad::new(&ANSWER, &s.animated)
                .absolute()
                .top(-spill)
                .left(-spill)
                .right(-spill)
                .bottom(-spill)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::{TestAppContext, VisualTestContext, div, red};

    /// Two controls side by side, each following its own mark.
    struct Two {
        marks: [u64; 2],
        answers: Vec<Answerer>,
    }

    impl Render for Two {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.answers = (0..2)
                .map(|i| {
                    let answers = Answerer::of(&ElementId::from(("control", i)), window, cx);
                    answers.follow(self.marks[i], Answer::Sweep, red(), window, cx);
                    answers
                })
                .collect();
            div()
        }
    }

    /// A control's answers are its own: one control's mark moving leaves
    /// its neighbour's state where it was.
    #[gpui_kit::test]
    fn each_control_follows_its_own_mark(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| Two {
            marks: [0, 0],
            answers: Vec::new(),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        view.update(cx, |two, cx| {
            two.marks[0] = 1;
            cx.notify();
        });
        cx.run_until_parked();
        let seen = view.update(cx, |two, cx| {
            two.answers.iter().map(|a| a.seen(cx)).collect::<Vec<_>>()
        });
        assert_eq!(seen, [Some(1), Some(0)]);
    }

    /// A change the control's own handler answered is not answered again as
    /// it draws, once; the next change is answered.
    #[gpui_kit::test]
    fn a_handler_s_answer_holds_off_one_change(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (view, cx) = cx.add_window_view(|_, _| Two {
            marks: [0, 0],
            answers: Vec::new(),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let own = |view: &Entity<Two>, cx: &mut VisualTestContext| {
            view.update(cx, |two, cx| two.answers[0].0.read(cx).own.is_some())
        };
        view.update(cx, |two, cx| two.answers[0].start(Answer::Burst, red(), cx));
        assert!(own(&view, cx), "the handler's answer is noted");
        view.update(cx, |two, cx| {
            two.marks[0] = 1;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(!own(&view, cx), "the change it answered took the note");
        view.update(cx, |two, cx| {
            two.marks[0] = 2;
            cx.notify();
        });
        cx.run_until_parked();
        let seen = view.update(cx, |two, cx| two.answers[0].seen(cx));
        assert_eq!(seen, Some(2));
    }

    #[test]
    fn every_answer_is_within_the_motion_tokens() {
        for answer in [
            Answer::Ring,
            Answer::Burst,
            Answer::Close,
            Answer::Sweep,
            Answer::Lift,
            Answer::Streak(true),
        ] {
            assert!(answer.duration() <= Motion::SETTLE);
        }
    }
}
