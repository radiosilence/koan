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
        let length = settle_length(cx);
        self.0.to(0, to, length, Ease::QuickOut);
        length
    }

    /// A page coming in from the trailing edge over the one left; how long
    /// that takes.
    pub fn enter(&self, cx: &App) -> Duration {
        self.0.hold(0, 1.0);
        self.settle(0.0, cx)
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
    use super::*;

    #[gpui::test]
    fn a_push_follows_then_settles_where_it_was_sent(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let push = Push::new();
            push.follow(0.4);
            assert!((push.at() - 0.4).abs() < 1e-4);
            let length = push.settle(1.0, cx);
            // Settling starts from where the finger left it.
            assert!(push.at() < 0.6, "{}", push.at());
            let landed = Animated::now() + length + Duration::from_millis(1);
            assert!((push.0.at(0, landed) - 1.0).abs() < 1e-4);
        });
    }

    #[gpui::test]
    fn a_pushed_page_enters_from_the_trailing_edge(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let push = Push::new();
            let length = push.enter(cx);
            assert!(push.at() > 0.9, "{}", push.at());
            let landed = Animated::now() + length + Duration::from_millis(1);
            assert!(push.0.at(0, landed).abs() < 1e-4);
        });
    }
}
