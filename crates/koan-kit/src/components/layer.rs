//! What an element paints, drawn once into a texture of its own and composed
//! by the GPU at each present (#1859): faded, moved or revealed by tracks,
//! with no frame drawn while it moves. What `FadeIn` and `Reveal` are made of.
//!
//! The content is laid out and painted as it would be unwrapped, so it takes
//! the pointer where it stands; only how it shows moves.

use gpui_kit::*;

use super::{Animated, Ease, Repeat, ShaderProgram};
use crate::theme::{KoanTheme, Motion};

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

/// A column that slides in and out at an edge (a panel, the sidebar): its
/// slot laid out at its full width at once, so what is beside it moves in
/// one step, and its content sliding by the layer's offset track. Gone, the
/// slot is let go once the slide out ends.
pub(crate) struct Sliding {
    /// Whether the slot is laid out: shown, or still sliding out.
    pub laid_out: bool,
    pub animated: Animated,
}

/// The slide for `key`, `travel` device pixels toward the edge the column
/// leaves by (negative for the leading edge).
pub(crate) fn sliding(
    key: ElementId,
    shown: bool,
    travel: f32,
    window: &mut Window,
    cx: &mut App,
) -> Sliding {
    struct State {
        shown: bool,
        laid_out: bool,
        animated: Animated,
        _gone: Option<Task<()>>,
    }
    let state = window.use_keyed_state(key, cx, |_, _| {
        let animated = Animated::new();
        standing(&animated);
        State {
            shown,
            laid_out: shown,
            animated,
            _gone: None,
        }
    });
    let moving = KoanTheme::motion_allowed(cx);
    state.update(cx, |s, cx| {
        if s.shown != shown {
            s.shown = shown;
            let now = Animated::now();
            if !moving {
                s.laid_out = shown;
                s._gone = None;
                s.animated.hold(1, 0.0);
            } else if shown {
                s.laid_out = true;
                s._gone = None;
                s.animated.segment(
                    1,
                    travel,
                    0.0,
                    now,
                    Motion::SETTLE,
                    Ease::QuickOut,
                    Repeat::Once,
                );
            } else {
                s.animated.segment(
                    1,
                    0.0,
                    travel,
                    now,
                    Motion::SETTLE,
                    Ease::QuickOut,
                    Repeat::Once,
                );
                s._gone = Some(cx.spawn(async move |s, cx| {
                    cx.background_executor().timer(Motion::SETTLE).await;
                    s.update(cx, |s, cx| {
                        s.laid_out = false;
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
        Sliding {
            laid_out: s.laid_out,
            animated: s.animated.clone(),
        }
    })
}
