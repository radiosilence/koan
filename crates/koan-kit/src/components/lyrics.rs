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
//! The sung line is drawn in `title small`, the rest in `control`, as the
//! Mac's panel draws them, all in `strong`; the program sets each line's
//! opacity. Where the renderer draws no shaders, the lines are drawn in
//! their colours and the column steps.

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
/// height of the edge's fade, 9 the line before's opacity as it starts to
/// ease out (1, or where a change still running had brought it). Tracks: 0 the emphasis moving from the line
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
        a = mix(u[9], u[7], t);
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
        a = mix(u[2].y, u[1].w, t);
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
    song: Option<u64>,
    on_line: Option<super::Handler<usize>>,
}

impl SyncedLines {
    pub fn new(id: impl Into<ElementId>, lines: Vec<SharedString>, active: Option<usize>) -> Self {
        Self {
            id: id.into(),
            lines,
            active,
            since: None,
            song: None,
            on_line: None,
        }
    }

    /// Which song these are the lines of (the track): another starts
    /// afresh, lyrics the same as the last song's included. Told by the
    /// lines' text otherwise.
    pub fn song(mut self, song: impl std::hash::Hash) -> Self {
        use std::hash::Hasher;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        song.hash(&mut hasher);
        self.song = Some(hasher.finish());
        self
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
    /// Frames waited for the sung line to be laid out, so the wait ends.
    waited: u8,
    /// Which song's lines these are: another song starts afresh.
    song: u64,
    /// The column's layout is the last song's until it is laid out again:
    /// where a line stands is not asked of it until then.
    stale: bool,
    /// When the sung line's motion started: its emphasis and its glide
    /// keep time together however long placing it waited.
    at: Instant,
    /// The sung line's opacity as its emphasis started, and the line
    /// before's: a change inside one still running starts from where that
    /// one had brought them, rather than flashing.
    from_sung: f32,
    from_before: f32,
}

/// How many frames to wait for the sung line's first layout before placing
/// it is given up.
const WAIT: u8 = 4;

/// A song's lines, told apart from another's.
fn song_of(lines: &[SharedString]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    lines.hash(&mut hasher);
    hasher.finish()
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
                waited: 0,
                song: 0,
                stale: false,
                at: Instant::now(),
                from_sung: UPCOMING,
                from_before: 1.0,
            }
        });
        let count = self.lines.len();
        let active = self.active.filter(|line| *line < count);
        let song = self.song.unwrap_or_else(|| song_of(&self.lines));
        // An item for each line, after the room above the first.
        let item = |line: usize| line + 1;
        let moving = KoanTheme::motion_allowed(cx);
        let (scroll, animated, before, waiting, (from_sung, from_before)) =
            state.update(cx, |s, _| {
                // Another song lands at once, from the top of its own lines.
                if s.song != song {
                    s.song = song;
                    s.stale = true;
                    s.drawn = false;
                    s.active = None;
                    s.before = None;
                    s.placing = None;
                    s.animated.hold(0, 1.0);
                    s.animated.hold(1, 0.0);
                }
                if s.active != active {
                    s.waited = 0;
                    let now = Animated::now();
                    // A change inside one still running: the line it was
                    // bringing in leaves from the opacity it had reached, and the
                    // column glides on from where it is drawn.
                    let t = s.animated.at(0, now).clamp(0.0, 1.0);
                    s.from_before = s.from_sung + (1.0 - s.from_sung) * t;
                    let forward = match (s.active, active) {
                        (Some(b), Some(a)) => b < a,
                        (Some(_), None) => false,
                        (None, _) => true,
                    };
                    s.from_sung = if forward { UPCOMING } else { PAST };
                    s.before = s.active;
                    s.active = active;
                    let animates = s.drawn && moving;
                    let drawn_at = s.scroll.offset().y + px(s.animated.at(1, now) / scale);
                    s.placing = Some(animates.then_some(drawn_at));
                    if animates {
                        let at = self
                            .since
                            .filter(|since| since.elapsed() < Motion::SETTLE)
                            .unwrap_or(now);
                        s.at = at;
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
                        s.from_before = 1.0;
                    }
                }
                s.drawn = true;
                // Brought to the middle of the view, gliding there from where it
                // stood when the song moved on by a little; a jump (a seek, a new
                // song) and the first draw land at once.
                let mut waiting = false;
                if s.stale && s.placing.is_some() {
                    // Laid out for this song first: the bounds held are the
                    // last one's.
                    s.stale = false;
                    waiting = true;
                } else if let (Some(from), Some(line)) = (s.placing, active) {
                    s.stale = false;
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
                            // By a little is the next line or so: less than half
                            // the panel.
                            let travel = from.map(|from| from - to);
                            let little = view.size.height * 0.5;
                            match travel.filter(|t| *t > Pixels::ZERO && *t < little) {
                                // Timed with the emphasis, from when the song
                                // reached the line.
                                Some(travel) => s.animated.segment(
                                    1,
                                    f32::from(travel) * scale,
                                    0.0,
                                    s.at,
                                    Motion::SETTLE,
                                    Ease::InOut,
                                    Repeat::Once,
                                ),
                                None => s.animated.hold(1, 0.0),
                            }
                        }
                        // Not laid out yet: placed once it has been, for a few
                        // frames, and then left to the scroll as it stands.
                        None if s.waited < WAIT => {
                            s.waited += 1;
                            waiting = true;
                        }
                        None => s.placing = None,
                    }
                }
                (
                    s.scroll.clone(),
                    s.animated.clone(),
                    s.before,
                    waiting,
                    (s.from_sung, s.from_before),
                )
            });
        if waiting {
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
            let color = if SHADED || active == Some(ix) {
                p.strong.hsla()
            } else {
                p.muted.hsla()
            };
            // A click finds its line by where the lines are drawn: mid-glide
            // the column stands off its layout by the glide's offset.
            let (glide, lines_at) = (animated.clone(), scroll.clone());
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
                    .text_role(
                        if active == Some(ix) {
                            TypeRole::TitleSmall
                        } else {
                            TypeRole::Control
                        },
                        &look,
                    )
                    .text_color(color)
                    .when_some(on_line, |d, f| {
                        d.on_click(move |e, window, cx| {
                            let offset = px(glide.at(1, Animated::now()) / window.scale_factor());
                            let line = (offset != Pixels::ZERO)
                                .then(|| e.position().y - offset)
                                .and_then(|y| {
                                    (0..count).find(|&l| {
                                        lines_at.bounds_for_item(item(l)).is_some_and(|b| {
                                            let top = b.top() + lines_at.offset().y;
                                            y >= top && y < top + b.size.height
                                        })
                                    })
                                })
                                .unwrap_or(ix);
                            f(&line, window, cx)
                        })
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
                    let sung = active.map(item);
                    let before = before.filter(|line| *line < count).map(item);
                    let band = f32::from(band);
                    let placed = animated.clone();
                    let lines = scroll.clone();
                    // Not decorative: which line is sung is information,
                    // so its emphasis shows under Reduce Motion too, landing
                    // at once as its owner sets it.
                    d.child(
                        super::AnimatedLayer::new(&animated, column)
                            .program(&LYRICS)
                            .informative()
                            .placed(move |bounds, scale| {
                                place(
                                    &placed,
                                    &lines,
                                    bounds,
                                    scale,
                                    sung,
                                    before,
                                    from_sung,
                                    from_before,
                                    band,
                                )
                            }),
                    )
                } else {
                    d.child(column)
                }
            })
            .child(Scrollbar::vertical(&scroll))
    }
}

/// Where the sung line and the one before stand in the layer, in device
/// pixels from its top, as the column is laid out and scrolled now (a scroll
/// by the person moves them with it), and the opacities they move between.
#[allow(clippy::too_many_arguments)]
fn place(
    animated: &Animated,
    scroll: &ScrollHandle,
    bounds: Bounds<Pixels>,
    scale: f32,
    sung: Option<usize>,
    before: Option<usize>,
    from_sung: f32,
    from_before: f32,
    band: f32,
) {
    let rect = |item: Option<usize>| {
        item.and_then(|i| scroll.bounds_for_item(i))
            .map(|b| {
                let top = b.top() + scroll.offset().y - bounds.top();
                [
                    f32::from(top) * scale,
                    f32::from(top + b.size.height) * scale,
                ]
            })
            // Nowhere: no row of the layer is that line.
            .unwrap_or([-1.0, -1.0])
    };
    let [n0, n1] = rect(sung);
    let [o0, o1] = rect(before);
    // The line before ends as what it now is: past when the song moved on,
    // to come when it went back.
    let before_ends = if from_sung == UPCOMING {
        PAST
    } else {
        UPCOMING
    };
    animated.set_all(
        0,
        &[
            n0,
            n1,
            o0,
            o1,
            UPCOMING,
            PAST,
            from_sung,
            before_ends,
            band * scale,
            from_before,
        ],
    );
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

    /// Only the sung line is drawn large; the others in the control size.
    #[gpui_kit::test]
    fn only_the_sung_line_is_large(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (_, cx) = cx.add_window_view(|_, _| Lines {
            clicked: Rc::new(Cell::new(None)),
        });
        let cx: &mut VisualTestContext = cx;
        cx.run_until_parked();
        let height = |cx: &mut VisualTestContext, line: &'static str| {
            cx.debug_bounds(line).expect("drawn").size.height
        };
        let (sung, other) = (height(cx, "synced-line-2"), height(cx, "synced-line-0"));
        assert!(sung > other, "sung {sung:?}, other {other:?}");
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
