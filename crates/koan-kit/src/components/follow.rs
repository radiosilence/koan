//! Motion a finger drives, on the composition layer: what it moves is drawn
//! once into a layer, each gesture event writes the layer's offset as a held
//! track, and the release hands the GPU the rest of the way as a segment. No
//! frame is drawn while the finger moves or the motion settles; the owner
//! draws again only when the layout changes, as the layer is mounted or let
//! go.
//!
//! - `Follow`: a layer offset by a finger, settling to a target (Now
//!   Playing pulled down, a queue row swiped).
//! - `SwipeAction`: a row slid toward the leading edge over an action that
//!   fills the room it leaves, both moved by one `Follow`.
//! - `Elastic`: a phone page's ends giving under a pan and bouncing a fling.
//! - `Push`: a navigation stack's page sliding over the one under it, which
//!   shows a third of its way behind and dimmed, by one progress track (a
//!   page pushed, swiped back, or sent back by the bar's button).
//!
//! Under Reduce Motion a finger is still followed, and a release lands at
//! once: the settle returns no wait.

use std::time::Duration;

use gpui::prelude::*;
use gpui::*;

use super::layer::{AnimatedLayer, LAYER, standing};
use super::{Animated, Ease, SHADED, ShaderProgram};
use crate::theme::{KoanTheme, Motion};

/// Whether a settle moves, rather than landing at once.
fn moves(cx: &App) -> bool {
    SHADED
        && match cx.try_global::<KoanTheme>() {
            Some(_) => KoanTheme::motion_allowed(cx),
            None => !cx.reduce_motion(),
        }
}

/// How long a settle started now takes: `Motion::SETTLE`, or nothing where
/// it lands at once. The owner lets the layer go after it.
pub fn settle_length(cx: &App) -> Duration {
    if moves(cx) {
        Motion::SETTLE
    } else {
        Duration::ZERO
    }
}

/// A layer offset by a finger: tracks 1 and 2 of a standing `LAYER`, in
/// device pixels. Clones share it.
#[derive(Clone)]
pub struct Follow(Animated);

impl Default for Follow {
    fn default() -> Self {
        Self::new()
    }
}

impl Follow {
    pub fn new() -> Self {
        let animated = Animated::new();
        standing(&animated);
        Self(animated)
    }

    /// The offset standing at `by`, as a gesture event reports it.
    pub fn follow(&self, by: Point<Pixels>, window: &Window) {
        let scale = window.scale_factor();
        self.0.hold(1, f32::from(by.x) * scale);
        self.0.hold(2, f32::from(by.y) * scale);
    }

    /// The offset easing from where it stands to `to`, on the quick-out over
    /// `Motion::SETTLE`; how long that takes.
    pub fn settle(&self, to: Point<Pixels>, window: &Window, cx: &App) -> Duration {
        let scale = window.scale_factor();
        let length = settle_length(cx);
        self.0
            .to(1, f32::from(to.x) * scale, length, Ease::QuickOut);
        self.0
            .to(2, f32::from(to.y) * scale, length, Ease::QuickOut);
        length
    }

    /// Where the offset stands now, in device pixels.
    pub fn at(&self) -> Point<f32> {
        let now = Animated::now();
        point(self.0.at(1, now), self.0.at(2, now))
    }

    /// `child` drawn as the layer the offset moves; composed at rest too,
    /// since a finger holds it still away from its place. Mounted only while
    /// a finger or a settle moves it.
    pub fn layer(&self, child: impl IntoElement) -> AnimatedLayer {
        AnimatedLayer::new(&self.0, child).program(&LAYER)
    }

    pub fn animated(&self) -> &Animated {
        &self.0
    }
}

/// How much of the finger's travel past a list's end shows, UIKit's constant.
const RUBBER: f32 = 0.55;
/// A bounce off a list's end: how hard it is drawn back, critically damped.
const BOUNCE_STIFFNESS: f32 = 140.0;
/// A pan let go past a list's end: firmer than a bounce, it has less to do.
const RETURN_STIFFNESS: f32 = 190.0;

fn damped(stiffness: f32) -> f32 {
    2.0 * stiffness.sqrt()
}

/// How far a page shows pulled for a finger `raw` past its end, in a
/// viewport `dimension` tall: it gives less the further it goes.
fn rubber(raw: f32, dimension: f32) -> f32 {
    raw.signum() * (1.0 - 1.0 / (raw.abs() * RUBBER / dimension + 1.0)) * dimension
}

/// The finger travel that shows a page pulled `shown`: `rubber` undone.
fn unrubber(shown: f32, dimension: f32) -> f32 {
    let shown = shown.abs().min(dimension * 0.99) * shown.signum();
    shown.signum() * dimension * (1.0 / (1.0 - shown.abs() / dimension) - 1.0) / RUBBER
}

#[derive(Default)]
struct ElasticState {
    /// Finger travel past the end, in points; zero while none.
    pulled: f32,
    finger: bool,
    /// The layer is mounted: pulled, or settling back.
    layered: bool,
    /// When the settle under way lands; a fling's remaining steps are not
    /// bounced off the end again before it has.
    settling: Option<std::time::Instant>,
    /// The last scroll step's time, and the speed it moved at in points per
    /// second: a fling's, as it reaches the end.
    last: Option<std::time::Instant>,
    speed: f32,
    /// What the finger has moved the page by since it last stood at its top.
    travelled: f32,
}

/// A page whose ends give, as a UIKit scroll view's: a vertical pan the page
/// has no more of to show pulls the page off its place, less the further it
/// goes, and a fling that reaches an end bounces. The whole page is the
/// layer the GPU moves, so a list of any kind has it without knowing, and
/// nothing is drawn while it moves. One per stage; the page goes in through
/// `wrap`.
#[derive(Clone)]
pub struct Elastic {
    follow: Follow,
    state: std::rc::Rc<std::cell::RefCell<ElasticState>>,
}

impl Default for Elastic {
    fn default() -> Self {
        Self::new()
    }
}

impl Elastic {
    pub fn new() -> Self {
        Self {
            follow: Follow::new(),
            state: Default::default(),
        }
    }

    /// `page`, its ends giving.
    pub fn wrap(&self, page: impl IntoElement, window: &mut Window, _: &mut App) -> AnyElement {
        let layered = self.state.borrow().layered;
        let view = window.current_view();
        let page = page.into_any_element();
        let page = if layered {
            self.follow.layer(page).into_any_element()
        } else {
            page
        };
        let this = self.clone();
        div()
            .relative()
            .size_full()
            .child(page)
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |bounds, hitbox, window, _| {
                        let this = this.clone();
                        let dimension = f32::from(bounds.size.height);
                        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                            if event.source == ScrollSource::Touch
                                && hitbox.should_handle_scroll(window)
                            {
                                this.heard(event, phase, dimension, view, window, cx);
                            }
                        });
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .into_any_element()
    }

    /// Whether the page has been scrolled since it last stood at its top: a
    /// rewind has somewhere to come from.
    pub fn scrolled(&self) -> bool {
        self.state.borrow().travelled.abs() > 1.0
    }

    /// Back at its top by a jump: the page comes the last part of the way as
    /// a scroll arriving there would, on the GPU. `over` is how far, in
    /// points. Nothing happens if it had not been scrolled.
    pub fn rewound(&self, over: Pixels, view: EntityId, window: &Window, cx: &mut App) {
        let scrolled = {
            let mut state = self.state.borrow_mut();
            std::mem::take(&mut state.travelled).abs() > 1.0
        };
        if !scrolled || !moves(cx) {
            return;
        }
        self.follow.follow(point(Pixels::ZERO, -over), window);
        let length = self.follow.settle(Point::default(), window, cx);
        self.land(length, view, cx);
    }

    fn land(&self, after: Duration, view: EntityId, cx: &mut App) {
        let at = std::time::Instant::now() + after;
        {
            let mut state = self.state.borrow_mut();
            state.settling = Some(at);
            state.layered = true;
        }
        cx.notify(view);
        let state = self.state.clone();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(after).await;
            let landed = {
                let mut state = state.borrow_mut();
                let landed = state.settling == Some(at) && state.pulled == 0.0 && !state.finger;
                if landed {
                    state.settling = None;
                    state.layered = false;
                }
                landed
            };
            if landed {
                cx.update(|cx| cx.notify(view));
            }
        })
        .detach();
    }

    fn heard(
        &self,
        event: &ScrollEvent,
        phase: DispatchPhase,
        dimension: f32,
        view: EntityId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let dy = f32::from(event.delta.pixel_delta(px(16.)).y);
        if phase == DispatchPhase::Capture {
            let now = std::time::Instant::now();
            let mut state = self.state.borrow_mut();
            if let Some(last) = state.last.replace(now) {
                let dt = now.duration_since(last).as_secs_f32();
                if dt > 0.001 && dy != 0.0 {
                    state.speed = dy / dt;
                }
            }
            match event.touch_phase {
                TouchPhase::Started => {
                    state.finger = true;
                    // A finger landing on a bounce holds it where it stands.
                    if state.settling.take().is_some() {
                        let shown = self.follow.at().y / window.scale_factor();
                        state.pulled = unrubber(shown, dimension);
                        self.follow.follow(point(Pixels::ZERO, px(shown)), window);
                    }
                    return;
                }
                TouchPhase::Moved if state.finger && state.pulled != 0.0 => {
                    let next = state.pulled + dy;
                    // Past the place it stands at rest, the page has its
                    // ordinary pan back.
                    state.pulled = if next * state.pulled <= 0.0 {
                        0.0
                    } else {
                        next
                    };
                    let shown = rubber(state.pulled, dimension);
                    self.follow.follow(point(Pixels::ZERO, px(shown)), window);
                    cx.stop_propagation();
                    return;
                }
                TouchPhase::Ended | TouchPhase::Cancelled if state.finger => {
                    state.finger = false;
                    if state.layered && state.settling.is_none() {
                        state.pulled = 0.0;
                        let length = if moves(cx) {
                            self.follow.animated().throw(
                                2,
                                0.0,
                                0.0,
                                RETURN_STIFFNESS,
                                damped(RETURN_STIFFNESS),
                            )
                        } else {
                            self.follow.follow(Point::default(), window);
                            Duration::ZERO
                        };
                        drop(state);
                        self.land(length, view, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                _ => {
                    if dy != 0.0 {
                        state.travelled += dy;
                    }
                    return;
                }
            }
        }
        // Bubbling: nothing under the finger could take this step.
        let mut state = self.state.borrow_mut();
        state.travelled -= dy;
        match event.touch_phase {
            TouchPhase::Moved if state.finger && dy != 0.0 => {
                state.pulled += dy;
                let shown = rubber(state.pulled, dimension);
                let layered = std::mem::replace(&mut state.layered, true);
                self.follow.follow(point(Pixels::ZERO, px(shown)), window);
                if !layered {
                    drop(state);
                    cx.notify(view);
                }
            }
            TouchPhase::Moved if !state.finger && dy != 0.0 && !state.layered => {
                let settling = state
                    .settling
                    .is_some_and(|at| at > std::time::Instant::now());
                if settling || !moves(cx) {
                    return;
                }
                let velocity = state.speed * window.scale_factor();
                drop(state);
                let length = self.follow.animated().throw(
                    2,
                    0.0,
                    velocity,
                    BOUNCE_STIFFNESS,
                    damped(BOUNCE_STIFFNESS),
                );
                self.land(length, view, cx);
            }
            _ => {}
        }
    }
}

/// What fills the room a row slid toward the leading edge leaves: drawn
/// whole under the row and shown only on the strip the row uncovers, moved
/// so that its middle stays in the middle of that strip. Track 1 is the
/// row's offset in device pixels, zero or less.
pub static SWIPE_ACTION: ShaderProgram = ShaderProgram {
    name: "koan-swipe-action",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float dx = koan_track(u, 1, time);
    float edge = size.x + dx;
    if (position.x < edge) {
        return float4(0.0);
    }
    float2 uv = (position - float2(edge * 0.5, 0.0)) / size;
    if (any(uv < 0.0) || any(uv > 1.0)) {
        return float4(0.0);
    }
    float4 c = koan_sample(image, tile, uv);
    float3 rgb = c.a > 0.0 ? c.rgb / c.a : float3(0.0);
    return float4(rgb, c.a);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let dx = koan_track(u, 1u, time);
    let edge = size.x + dx;
    if (position.x < edge) {
        return vec4<f32>(0.0);
    }
    let uv = (position - vec2<f32>(edge * 0.5, 0.0)) / size;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    let c = koan_sample(tile, uv);
    var rgb = vec3<f32>(0.0);
    if (c.a > 0.0) {
        rgb = c.rgb / c.a;
    }
    return vec4<f32>(rgb, c.a);
}
"#,
    prepare: None,
};

/// A row a finger slides toward the leading edge, over `action`, which is
/// laid out over the row's whole place (absolute, inset to it) and shows on
/// the strip the row uncovers. Both move by `follow`'s horizontal offset.
#[derive(IntoElement)]
pub struct SwipeAction {
    follow: Follow,
    row: AnyElement,
    action: AnyElement,
}

impl SwipeAction {
    pub fn new(follow: &Follow, row: impl IntoElement, action: impl IntoElement) -> Self {
        Self {
            follow: follow.clone(),
            row: row.into_any_element(),
            action: action.into_any_element(),
        }
    }
}

impl RenderOnce for SwipeAction {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .relative()
            .child(AnimatedLayer::new(&self.follow.0, self.action).program(&SWIPE_ACTION))
            .child(self.follow.layer(self.row))
    }
}

/// The page in front: drawn over the stage, moved toward the trailing edge
/// by track 0, the push's progress (0 in place, 1 gone).
pub static PUSH_FRONT: ShaderProgram = ShaderProgram {
    name: "koan-push-front",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float progress = koan_track(u, 0, time);
    float2 uv = (position - float2(size.x * progress, 0.0)) / size;
    if (any(uv < 0.0) || any(uv > 1.0)) {
        return float4(0.0);
    }
    float4 c = koan_sample(image, tile, uv);
    float3 rgb = c.a > 0.0 ? c.rgb / c.a : float3(0.0);
    return float4(rgb, c.a);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let progress = koan_track(u, 0u, time);
    let uv = (position - vec2<f32>(size.x * progress, 0.0)) / size;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    let c = koan_sample(tile, uv);
    var rgb = vec3<f32>(0.0);
    if (c.a > 0.0) {
        rgb = c.rgb / c.a;
    }
    return vec4<f32>(rgb, c.a);
}
"#,
    prepare: None,
};

/// The page under: shown on the strip the page in front uncovers alone, a
/// third of its way behind, under a veil (uniforms 0 to 3, its colour at
/// rest) that fades as the page in front goes when uniform 4 is set.
pub static PUSH_UNDER: ShaderProgram = ShaderProgram {
    name: "koan-push-under",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float progress = koan_track(u, 0, time);
    if (position.x > size.x * progress) {
        return float4(0.0);
    }
    float2 uv = (position + float2(size.x * 0.3 * (1.0 - progress), 0.0)) / size;
    float4 c = float4(0.0);
    if (all(uv >= 0.0) && all(uv <= 1.0)) {
        c = koan_sample(image, tile, uv);
    }
    float veil = u[3] * (u[4] > 0.5 ? 1.0 - progress : 1.0);
    // The veil over the page, both premultiplied.
    float3 rgb = float3(u[0], u[1], u[2]) * veil + c.rgb * (1.0 - veil);
    float a = veil + c.a * (1.0 - veil);
    return float4(a > 0.0 ? rgb / a : float3(0.0), a);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let progress = koan_track(u, 0u, time);
    if (position.x > size.x * progress) {
        return vec4<f32>(0.0);
    }
    let uv = (position + vec2<f32>(size.x * 0.3 * (1.0 - progress), 0.0)) / size;
    var c = vec4<f32>(0.0);
    if (all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0))) {
        c = koan_sample(tile, uv);
    }
    var veil = u[0].w;
    if (u[1].x > 0.5) {
        veil = veil * (1.0 - progress);
    }
    let rgb = u[0].xyz * veil + c.rgb * (1.0 - veil);
    let a = veil + c.a * (1.0 - veil);
    var straight = vec3<f32>(0.0);
    if (a > 0.0) {
        straight = rgb / a;
    }
    return vec4<f32>(straight, a);
}
"#,
    prepare: None,
};

/// A navigation stack's push: one progress track both pages read, 0 with
/// the page in front in place and 1 with it gone past the trailing edge.
/// Clones share it.
#[derive(Clone)]
pub struct Push(Animated);

impl Default for Push {
    fn default() -> Self {
        Self::new()
    }
}

impl Push {
    pub fn new() -> Self {
        let animated = Animated::new();
        animated.hold(0, 0.0);
        Self(animated)
    }

    /// Whether a push moves at all: where it would land at once, its owner
    /// goes straight to where it ends.
    pub fn moves(cx: &App) -> bool {
        moves(cx)
    }

    /// The progress standing where a finger has it.
    pub fn follow(&self, progress: f32) {
        self.0.hold(0, progress);
    }

    /// The progress easing from where it stands to `to`; how long that
    /// takes.
    pub fn settle(&self, to: f32, cx: &App) -> Duration {
        self.settle_over(to, settle_length(cx))
    }

    fn settle_over(&self, to: f32, length: Duration) -> Duration {
        self.0.to(0, to, length, Ease::QuickOut);
        length
    }

    /// A page coming in from the trailing edge over the one left; how long
    /// that takes.
    pub fn enter(&self, cx: &App) -> Duration {
        self.0.hold(0, 1.0);
        self.settle(0.0, cx)
    }

    #[cfg(test)]
    fn enter_over(&self, length: Duration) -> Duration {
        self.0.hold(0, 1.0);
        self.settle_over(0.0, length)
    }

    /// Where the progress stands now.
    pub fn at(&self) -> f32 {
        self.0.at(0, Animated::now())
    }

    /// The page in front, laid out over the whole stage.
    pub fn front(&self, child: impl IntoElement) -> AnimatedLayer {
        AnimatedLayer::new(&self.0, child).program(&PUSH_FRONT)
    }

    /// The page under, laid out over the whole stage, under `veil` (its
    /// colour with the page in front in place), which fades as that page
    /// goes when `fades`.
    pub fn under(&self, child: impl IntoElement, veil: Hsla, fades: bool) -> AnimatedLayer {
        self.0.set_color(0, veil);
        self.0.set(4, if fades { 1.0 } else { 0.0 });
        AnimatedLayer::new(&self.0, child).program(&PUSH_UNDER)
    }
}

#[cfg(test)]
mod tests {
    use super::{Animated, Duration, Motion, Push};

    #[test]
    fn a_push_follows_then_settles_where_it_was_sent() {
        let push = Push::new();
        push.follow(0.4);
        assert!((push.at() - 0.4).abs() < 1e-4);
        let length = push.settle_over(1.0, Motion::SETTLE);
        // Settling starts from where the finger left it.
        assert!(push.at() < 0.6, "{}", push.at());
        let landed = Animated::now() + length + Duration::from_millis(1);
        assert!((push.0.at(0, landed) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_pushed_page_enters_from_the_trailing_edge() {
        let push = Push::new();
        let length = push.enter_over(Motion::SETTLE);
        assert!(push.at() > 0.9, "{}", push.at());
        let landed = Animated::now() + length + Duration::from_millis(1);
        assert!(push.0.at(0, landed).abs() < 1e-4);
    }

    #[test]
    fn a_release_under_reduce_motion_lands_at_once() {
        let push = Push::new();
        push.follow(0.7);
        push.settle_over(1.0, Duration::ZERO);
        assert!((push.at() - 1.0).abs() < 1e-4);
    }
}
