//! What an element paints, drawn once into a texture of its own and composed
//! by the GPU at each present (#1859): faded, moved or revealed by tracks,
//! with no frame drawn while it moves. What `FadeIn` and `Reveal` are made of.
//!
//! The content is laid out and painted as it would be unwrapped, so it takes
//! the pointer where it stands; only how it shows moves.

use gpui::*;

use super::{Animated, ShaderProgram};

/// The layer's composition. Tracks: 0 the opacity, 1 and 2 the offset in
/// device pixels, 3 how much of it shows, from the top left, 0 to 1.
pub static LAYER: ShaderProgram = ShaderProgram {
    name: "koan-layer",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float shown = koan_track(u, 3, time);
    if (position.x > size.x * shown || position.y > size.y * shown) {
        return float4(0.0);
    }
    float2 offset = float2(koan_track(u, 1, time), koan_track(u, 2, time));
    float2 uv = (position - offset) / size;
    if (any(uv < 0.0) || any(uv > 1.0)) {
        return float4(0.0);
    }
    // The layer was drawn over clear: its colours are premultiplied.
    float4 c = koan_sample(image, tile, uv);
    float3 rgb = c.a > 0.0 ? c.rgb / c.a : float3(0.0);
    return float4(rgb, c.a * koan_track(u, 0, time));
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let shown = koan_track(u, 3u, time);
    if (position.x > size.x * shown || position.y > size.y * shown) {
        return vec4<f32>(0.0);
    }
    let offset = vec2<f32>(koan_track(u, 1u, time), koan_track(u, 2u, time));
    let uv = (position - offset) / size;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    let c = koan_sample(tile, uv);
    var rgb = vec3<f32>(0.0);
    if (c.a > 0.0) {
        rgb = c.rgb / c.a;
    }
    return vec4<f32>(rgb, c.a * koan_track(u, 0u, time));
}
"#,
    prepare: None,
};

/// A layer scaled about a point and moved: tracks 0 the opacity, 1 the
/// scale and 2 and 3 the offset, the point in device pixels from the layer's
/// origin in uniforms 0 and 1. The layer is drawn at its full size and
/// scaled down, so nothing is enlarged.
pub static ZOOM: ShaderProgram = ShaderProgram {
    name: "koan-zoom",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float scale = max(koan_track(u, 1, time), 0.001);
    float2 origin = float2(u[0], u[1]);
    float2 offset = float2(koan_track(u, 2, time), koan_track(u, 3, time));
    float2 uv = ((position - offset - origin) / scale + origin) / size;
    if (any(uv < 0.0) || any(uv > 1.0)) {
        return float4(0.0);
    }
    float4 c = koan_sample(image, tile, uv);
    float3 rgb = c.a > 0.0 ? c.rgb / c.a : float3(0.0);
    return float4(rgb, c.a * koan_track(u, 0, time));
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let scale = max(koan_track(u, 1u, time), 0.001);
    let origin = u[0].xy;
    let offset = vec2<f32>(koan_track(u, 2u, time), koan_track(u, 3u, time));
    let uv = ((position - offset - origin) / scale + origin) / size;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    let c = koan_sample(tile, uv);
    var rgb = vec3<f32>(0.0);
    if (c.a > 0.0) {
        rgb = c.rgb / c.a;
    }
    return vec4<f32>(rgb, c.a * koan_track(u, 0u, time));
}
"#,
    prepare: None,
};

/// What writes a layer's placement uniforms: its bounds and the window's scale.
type Placed = Box<dyn Fn(Bounds<Pixels>, f32)>;

/// `child` drawn as a layer whose opacity, offset and reveal `animated`'s
/// tracks move (`LAYER`). Decorative: under Reduce Motion it stands at its
/// tracks' ends.
pub struct AnimatedLayer {
    child: AnyElement,
    animated: Animated,
    program: &'static ShaderProgram,
    /// Composed by the program at rest too, not only while it moves.
    always: bool,
    /// What it shows is information, so its tracks move under Reduce Motion.
    informative: bool,
    /// Writes the uniforms that depend on where it is painted, given its
    /// bounds and the window's scale, each time it paints.
    placed: Option<Placed>,
    /// Room composed below its bounds, for content offset down into it.
    below: Pixels,
}

impl AnimatedLayer {
    /// A layer standing whole until `animated`'s tracks say otherwise.
    pub fn new(animated: &Animated, child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            animated: animated.clone(),
            program: &LAYER,
            always: false,
            informative: false,
            placed: None,
            below: Pixels::ZERO,
        }
    }

    /// Composed over `below` more than its bounds at the foot, so content
    /// its offset moves down is drawn there rather than cut off.
    pub fn room_below(mut self, below: Pixels) -> Self {
        self.below = below;
        self
    }

    /// Composed by `program` rather than `LAYER`, at rest as well as in
    /// motion: a layer whose look is its program's (the lyrics' opacities
    /// and edges).
    pub fn program(mut self, program: &'static ShaderProgram) -> Self {
        self.program = program;
        self.always = true;
        self
    }

    /// Composed by `program` rather than `LAYER` while it moves, and painted
    /// plainly at rest.
    pub fn moving_program(mut self, program: &'static ShaderProgram) -> Self {
        self.program = program;
        self
    }

    /// Its motion says something (which line is sung): under Reduce Motion
    /// its tracks still move, as their owner sets them.
    pub fn informative(mut self) -> Self {
        self.informative = true;
        self
    }

    /// `placed` writes the uniforms that follow where the layer is painted,
    /// with its bounds and the window's scale, each paint: a scroll that
    /// moves what is inside it.
    pub fn placed(mut self, placed: impl Fn(Bounds<Pixels>, f32) + 'static) -> Self {
        self.placed = Some(Box::new(placed));
        self
    }
}

impl IntoElement for AnimatedLayer {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for AnimatedLayer {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.animated.attach(!self.informative, window, cx);
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // A layer only while it moves: settled, it is painted as it stands,
        // with no texture of its own.
        if !self.always && !self.animated.moving() {
            self.child.paint(window, cx);
            return;
        }
        if let Some(placed) = &self.placed {
            placed(bounds, window.scale_factor());
        }
        let mut bounds = bounds;
        bounds.size.height += self.below;
        let child = &mut self.child;
        self.animated
            .paint_layer(self.program, bounds, window, cx, |window, cx| {
                child.paint(window, cx)
            });
        // Its view drawn again once it settles, which paints it with no
        // group: a cached view would otherwise go on replaying the group,
        // and its texture, until something else drew it (#1878). A layer
        // composed at rest too keeps its group, so nothing is owed it: asked
        // at rest, the wait is nothing and the view would draw again at
        // once, every frame.
        if !self.always
            && let Some(wait) = self.animated.settled_in()
        {
            let view = window.current_view();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(wait).await;
                cx.update(|cx| cx.notify(view));
            })
            .detach();
        }
    }
}

/// A layer's tracks standing whole: fully shown, in place.
pub fn standing(animated: &Animated) {
    animated.hold(0, 1.0);
    animated.hold(1, 0.0);
    animated.hold(2, 0.0);
    animated.hold(3, 1.0);
}
