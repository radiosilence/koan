//! What an element paints, drawn once into a texture of its own and composed
//! by the GPU at each present (#1859): faded, moved or revealed by tracks,
//! with no frame drawn while it moves. What `FadeIn` and `Reveal` are made of.
//!
//! The content is laid out and painted as it would be unwrapped, so it takes
//! the pointer where it stands; only how it shows moves.

use gpui_kit::*;

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

/// `child` drawn as a layer whose opacity, offset and reveal `animated`'s
/// tracks move (`LAYER`). Decorative: under Reduce Motion it stands at its
/// tracks' ends.
pub struct AnimatedLayer {
    child: AnyElement,
    animated: Animated,
}

impl AnimatedLayer {
    /// A layer standing whole until `animated`'s tracks say otherwise.
    pub fn new(animated: &Animated, child: impl IntoElement) -> Self {
        Self {
            child: child.into_any_element(),
            animated: animated.clone(),
        }
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
        self.animated.attach(true, window, cx);
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
        let child = &mut self.child;
        self.animated
            .paint_layer(&LAYER, bounds, window, cx, |window, cx| {
                child.paint(window, cx)
            });
    }
}

/// A layer's tracks standing whole: fully shown, in place.
pub fn standing(animated: &Animated) {
    animated.hold(0, 1.0);
    animated.hold(1, 0.0);
    animated.hold(2, 0.0);
    animated.hold(3, 1.0);
}
