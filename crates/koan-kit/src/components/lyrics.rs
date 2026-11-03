//! Lines of timed text following the song: a song's lyrics in their panel.
//!
//! The lines are one layer the GPU composes at each present (`LYRICS`), as
//! a karaoke screen is drawn. When the song reaches a line the column glides
//! up to it, the sung line eases into emphasis and the one before settles
//! back, each on a track the program evaluates at the present's time, so
//! nothing on the CPU runs while they move. Past lines are quieter than the
//! ones to come, and the panel's top and bottom edges fade, so the lines
//! enter and leave softly rather than being cut.
//!
//! Every line is drawn in one size and in `strong`, and the program sets
//! each line's opacity: no line changes its layout as the song moves, so a
//! line that wraps never pushes the others about. Where the renderer draws
//! no shaders, the lines are drawn in their colours and the column steps.

use std::time::Instant;

use gpui_kit::base::Scrollbar;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Animated, Ease, Repeat, SHADED, ShaderProgram};
use crate::theme::{KoanStyled, KoanTheme, Look, Motion, TypeRole};

/// How the lines show. Uniforms: 0 and 1 the sung line's top and bottom, 2
/// and 3 the line sung before it, in device pixels from the layer's top as
/// laid out; 4 a line to come's opacity, 5 a past line's, 6 the sung line's
/// opacity as it starts to ease in, 7 the line before's as it ends, 8 the
/// height of the edge's fade. Tracks: 0 the emphasis moving from the line
/// before to the sung line, 0 to 1; 1 the column's offset down, in device
/// pixels, as it glides to its place.
pub static LYRICS: ShaderProgram = ShaderProgram {
    name: "koan-lyrics",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    float t = clamp(koan_track(u, 0, time), 0.0, 1.0);
    float offset = koan_track(u, 1, time);
    float2 uv = float2(position.x, position.y - offset) / size;
    if (uv.y < 0.0 || uv.y > 1.0) {
        return float4(0.0);
    }
    float4 c = koan_sample(image, tile, uv);
    float3 rgb = c.a > 0.0 ? c.rgb / c.a : float3(0.0);
    float y = position.y - offset;
    float a;
    if (y >= u[0] && y < u[1]) {
        a = mix(u[6], 1.0, t);
    } else if (y >= u[2] && y < u[3]) {
        a = mix(1.0, u[7], t);
    } else if (y < u[0]) {
        a = u[5];
    } else {
        a = u[4];
    }
    float band = max(u[8], 1.0);
    float edge = smoothstep(0.0, band, position.y) * smoothstep(0.0, band, size.y - position.y);
    return float4(rgb, c.a * a * edge * edge);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    let t = clamp(koan_track(u, 0u, time), 0.0, 1.0);
    let offset = koan_track(u, 1u, time);
    let uv = vec2<f32>(position.x, position.y - offset) / size;
    if (uv.y < 0.0 || uv.y > 1.0) {
        return vec4<f32>(0.0);
    }
    let c = koan_sample(tile, uv);
    var rgb = vec3<f32>(0.0);
    if (c.a > 0.0) {
        rgb = c.rgb / c.a;
    }
    let y = position.y - offset;
    var a = u[1].x;
    if (y >= u[0].x && y < u[0].y) {
        a = mix(u[1].z, 1.0, t);
    } else if (y >= u[0].z && y < u[0].w) {
        a = mix(1.0, u[1].w, t);
    } else if (y < u[0].x) {
        a = u[1].y;
    }
    let band = max(u[2].x, 1.0);
    let edge = smoothstep(0.0, band, position.y) * smoothstep(0.0, band, size.y - position.y);
    return vec4<f32>(rgb, c.a * a * edge * edge);
}
"#,
    prepare: None,
};

/// A line to come, and one already sung: quieter than the sung line, the
/// past the quieter of the two.
const UPCOMING: f32 = 0.5;
const PAST: f32 = 0.3;

/// Lines of timed text, the one at `active` emphasised and kept mid-panel as
/// the song moves. A click on a line hands its index to `on_line`.
#[derive(IntoElement)]
pub struct SyncedLines {
    id: ElementId,
    lines: Vec<SharedString>,
    active: Option<usize>,
    since: Option<Instant>,
    on_line: Option<super::Handler<usize>>,
}

impl SyncedLines {
    pub fn new(id: impl Into<ElementId>, lines: Vec<SharedString>, active: Option<usize>) -> Self {
        Self {
            id: id.into(),
            lines,
            active,
            since: None,
            on_line: None,
        }
    }

    /// When the song reached the active line, by the playhead: its motion
    /// is timed from there rather than from when the line was drawn.
    pub fn since(mut self, at: Option<Instant>) -> Self {
        self.since = at;
        self
    }

    pub fn on_line(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_line = Some(std::sync::Arc::new(handler));
        self
    }
}

/// What the lines keep between frames: the scroll, the motion, and which
/// line was sung before.
struct State {
    scroll: ScrollHandle,
    animated: Animated,
    active: Option<usize>,
    before: Option<usize>,
    /// The sung line still to be brought to the middle once it has been
    /// laid out, gliding from the offset given, or placed at once.
    placing: Option<Option<Pixels>>,
    /// Drawn before: the first draw shows its line still.
    drawn: bool,
}

impl RenderOnce for SyncedLines {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let p = look.palette;
        let pad = look.spaced(18.0);
        let band = look.spaced(48.0);
        let scale = window.scale_factor();
        let state = window.use_keyed_state(super::child(&self.id, "lines"), cx, |_, _| {
            let animated = Animated::new();
            animated.hold(0, 1.0);
            animated.hold(1, 0.0);
            State {
                scroll: ScrollHandle::new(),
                animated,
                active: None,
                before: None,
                placing: None,
                drawn: false,
            }
        });
        let active = self.active;
        // An item for each line, after the room above the first.
        let item = |line: usize| line + 1;
        let moving = KoanTheme::motion_allowed(cx);
        let (scroll, animated, before, waiting) = state.update(cx, |s, _| {
            if s.active != active {
                s.before = s.active;
                s.active = active;
                let animates = s.drawn && moving;
                s.placing = Some(animates.then(|| s.scroll.offset().y));
                if animates {
                    let at = self
                        .since
                        .filter(|since| since.elapsed() < Motion::SETTLE)
                        .unwrap_or_else(Animated::now);
                    s.animated.segment(
                        0,
                        0.0,
                        1.0,
                        at,
                        Motion::SETTLE,
                        Ease::QuickOut,
                        Repeat::Once,
                    );
                } else {
                    s.animated.hold(0, 1.0);
                }
            }
            s.drawn = true;
            // Brought to the middle of the view, gliding there from where it
            // stood when the song moved on by a little; a jump (a seek, a new
            // song) and the first draw land at once.
            let mut waiting = false;
            if let (Some(from), Some(line)) = (s.placing, active) {
                match s.scroll.bounds_for_item(item(line)) {
                    Some(bounds) => {
                        s.placing = None;
                        let view = s.scroll.bounds();
                        let to = (view.center().y - bounds.center().y)
                            .clamp(-s.scroll.max_offset().y, Pixels::ZERO);
                        s.scroll.set_offset(Point {
                            x: s.scroll.offset().x,
                            y: to,
                        });
                        let travel = from.map(|from| from - to);
                        match travel.filter(|t| *t > Pixels::ZERO && *t < band * 0.5) {
                            Some(travel) => s.animated.segment(
                                1,
                                f32::from(travel) * scale,
                                0.0,
                                Animated::now(),
                                Motion::SETTLE,
                                Ease::InOut,
                                Repeat::Once,
                            ),
                            None => s.animated.hold(1, 0.0),
                        }
                    }
                    None => waiting = true,
                }
            }
            (s.scroll.clone(), s.animated.clone(), s.before, waiting)
        });
        let forward = match (before, active) {
            (Some(b), Some(a)) => b < a,
            _ => true,
        };
        // Where the sung line and the one before stand in the layer, as the
        // column was laid out last.
        let rect = |line: Option<usize>| {
            line.and_then(|l| scroll.bounds_for_item(item(l)))
                .map(|b| {
                    let top = b.top() + scroll.offset().y - scroll.bounds().top();
                    [
                        f32::from(top) * scale,
                        f32::from(top + b.size.height) * scale,
                    ]
                })
                .unwrap_or([-1.0, -1.0])
        };
        let [n0, n1] = rect(active);
        let [o0, o1] = rect(before);
        animated.set_all(
            0,
            &[
                n0,
                n1,
                o0,
                o1,
                UPCOMING,
                PAST,
                if forward { UPCOMING } else { PAST },
                if forward { PAST } else { UPCOMING },
                f32::from(band) * scale,
            ],
        );
        if waiting || (n0 < 0.0 && active.is_some()) {
            // Not laid out yet: placed once it has been.
            window.request_animation_frame();
        }
        // Room above the first line and below the last, so either can sit
        // mid-panel as the song reaches it.
        let air = || div().h(look.spaced(60.0) + band).into_any_element();
        let mut items = Vec::with_capacity(self.lines.len() + 2);
        items.push(air());
        for (ix, line) in self.lines.into_iter().enumerate() {
            let on_line = self.on_line.clone();
            // Drawn whole for the program to set its opacity; without
            // shaders, in the colour its place gives it.
            let color = if SHADED {
                p.strong.hsla()
            } else if active == Some(ix) {
                p.strong.hsla()
            } else {
                p.muted.hsla()
            };
            items.push(
                div()
                    .id(ix)
                    .map(|d| {
                        #[cfg(test)]
                        let d = d.debug_selector(move || format!("synced-line-{ix}"));
                        d
                    })
                    .w_full()
                    .px(pad)
                    .py(look.spaced(5.5))
                    .cursor_pointer()
                    .text_role(TypeRole::TitleSmall, &look)
                    .text_color(color)
                    .when_some(on_line, |d, f| {
                        d.on_click(move |_, window, cx| f(&ix, window, cx))
                    })
                    // An empty line keeps its height: a pause in the song.
                    .child(if line.is_empty() { " ".into() } else { line })
                    .into_any_element(),
            );
        }
        items.push(air());
        let column = div()
            .id(self.id)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .flex()
            .flex_col()
            .children(items);
        div()
            .relative()
            .size_full()
            .map(|d| {
                if SHADED {
                    d.child(LinesLayer {
                        child: column.into_any_element(),
                        animated,
                    })
                } else {
                    d.child(column)
                }
            })
            .child(Scrollbar::vertical(&scroll))
    }
}

/// The column drawn as one layer under `LYRICS`, always: its opacities and
/// its edges are the program's even at rest.
struct LinesLayer {
    child: AnyElement,
    animated: Animated,
}

impl IntoElement for LinesLayer {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for LinesLayer {
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
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        let child = self.child.request_layout(window, cx);
        (window.request_layout(style, [child], cx), ())
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
        // Not decorative: which line is being sung is information, so its
        // emphasis shows under Reduce Motion too, landing at once.
        self.animated.attach(false, window, cx);
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
            .paint_layer(&LYRICS, bounds, window, cx, |window, cx| {
                child.paint(window, cx)
            });
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use core::prelude::v1::test;

    struct Lines {
        clicked: Rc<Cell<Option<usize>>>,
    }

    impl Render for Lines {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicked = self.clicked.clone();
            div().w(px(280.)).h(px(400.)).child(
                SyncedLines::new(
                    "lines",
                    vec!["one".into(), "".into(), "three".into()],
                    Some(2),
                )
                .on_line(move |ix, _, _| clicked.set(Some(*ix))),
            )
        }
    }

    /// A click on a line hands its index over, an empty line keeping its
    /// place among the others.
    #[gpui_kit::test]
    fn a_click_names_the_line(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let clicked = Rc::new(Cell::new(None));
        let (_, cx) = cx.add_window_view(|_, _| Lines {
            clicked: clicked.clone(),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let at = cx
            .debug_bounds("synced-line-2")
            .expect("the line is drawn")
            .center();
        cx.simulate_click(at, Modifiers::default());
        assert_eq!(clicked.get(), Some(2));
    }
}
