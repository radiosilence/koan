//! What is drawn over a window's pages: the system's status bar, notch,
//! home indicator and software keyboard on a phone (zero on the desktop),
//! and the bars a shell draws over its pages, frosted while content runs
//! under them.
//!
//! Grounds and the wash run under all of it; content is held clear. An
//! inset change redraws the window, so an element reading these in its
//! render follows rotation and the keyboard.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Global, IntoElement, ParentElement, Pixels, RenderOnce, Styled, Window, WindowId, canvas,
    div, px,
};

use super::{Animated, AnimatedQuad, CursorSpot, Ease, SHADED, ScrollSource, ShaderProgram};
use crate::theme::Look;
use crate::theme::tokens::Motion;

/// Which of the system's insets content keeps clear of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    /// The status bar, the notch and the home indicator.
    System,
    /// Those and the keyboard while it is up: for what a field sits in.
    Keyboard,
}

/// The insets `clear` names, per edge.
pub fn system_insets(clear: Clear, window: &Window) -> gpui::Edges<Pixels> {
    let insets = window.insets();
    match clear {
        Clear::System => insets.safe_area,
        Clear::Keyboard => insets.effective(),
    }
}

pub trait KoanInsets: Styled + Sized {
    /// Padding that holds the element's children clear of what the system
    /// draws over the window, while the element's own ground fills it.
    fn clear_of_system(self, clear: Clear, window: &Window) -> Self {
        let edges = system_insets(clear, window);
        self.pt(edges.top)
            .pr(edges.right)
            .pb(edges.bottom)
            .pl(edges.left)
    }
}

impl<E: Styled> KoanInsets for E {}

/// What a shell draws over its pages, which run the full height of the
/// stage under it: on a phone the status bar, the bar and any notices under
/// it at the top, and the mini player, the tabs and the home indicator at
/// the foot; on the desktop the page column's toolbar and the transport.
/// Each window keeps its own, measured by its shell (`bar_probe`,
/// `foot_probe`); a window whose shell draws nothing over its pages
/// (Settings, the EQ) has none, and so zero.
#[derive(Clone, Copy, Default, PartialEq)]
struct PageInsets {
    top: Pixels,
    bottom: Pixels,
}

/// A window's insets and its scroll edges: whether the page's content runs
/// under the bar and under the foot, and the tracks its frost fades by.
struct Edges {
    insets: PageInsets,
    /// The page shown, hashed (`page_shown`).
    page: Option<u64>,
    under: (bool, bool),
    /// The last report this frame, applied once it is drawn: of two pages
    /// on screen (one swiped away over the other) the one in front, drawn
    /// last, is what the frost follows.
    reported: Option<(bool, bool)>,
    bar: Animated,
    foot: Animated,
}

impl Default for Edges {
    fn default() -> Self {
        let edges = Self {
            insets: PageInsets::default(),
            page: None,
            under: (false, false),
            reported: None,
            bar: Animated::new(),
            foot: Animated::new(),
        };
        edges.bar.hold(0, 0.0);
        edges.foot.hold(0, 0.0);
        edges
    }
}

#[derive(Default)]
struct WindowEdges {
    by_window: HashMap<WindowId, Edges>,
    /// Windows closing are heard, once for the app.
    observing: bool,
}

impl Global for WindowEdges {}

fn page_insets(window: &Window, cx: &App) -> PageInsets {
    cx.try_global::<WindowEdges>()
        .and_then(|edges| edges.by_window.get(&window.window_handle().window_id()))
        .map(|edges| edges.insets)
        .unwrap_or_default()
}

fn edges_mut<'a>(window: &Window, cx: &'a mut App) -> &'a mut Edges {
    if !cx.default_global::<WindowEdges>().observing {
        cx.default_global::<WindowEdges>().observing = true;
        cx.on_window_closed(|cx, closed| {
            cx.default_global::<WindowEdges>().by_window.remove(&closed);
        })
        .detach();
    }
    cx.default_global::<WindowEdges>()
        .by_window
        .entry(window.window_handle().window_id())
        .or_default()
}

/// The room a page leaves at its top for the bar drawn over it (zero where
/// nothing is): the first row of a page that scrolls under the bar starts
/// this far down inside its scroll, and a page that does not scroll is held
/// this far down.
pub fn bar_inset(window: &Window, cx: &App) -> Pixels {
    page_insets(window, cx).top
}

/// The room a page leaves at its end for the foot drawn over it (zero where
/// nothing is): the last row of a page that scrolls under the foot ends
/// this far up from the end of its scroll, and what sits at a page's foot
/// in flow is held this far up.
pub fn foot_inset(window: &Window, cx: &App) -> Pixels {
    page_insets(window, cx).bottom
}

/// Sets `bar_inset`. A change draws every view again, cached ones included,
/// since each page reads the inset in its own render.
pub fn set_bar_inset(inset: Pixels, window: &mut Window, cx: &mut App) {
    let insets = page_insets(window, cx);
    set_page_insets(
        PageInsets {
            top: inset,
            ..insets
        },
        window,
        cx,
    );
}

/// Sets `foot_inset`, as `set_bar_inset` does.
pub fn set_foot_inset(inset: Pixels, window: &mut Window, cx: &mut App) {
    let insets = page_insets(window, cx);
    set_page_insets(
        PageInsets {
            bottom: inset,
            ..insets
        },
        window,
        cx,
    );
}

fn set_page_insets(insets: PageInsets, window: &mut Window, cx: &mut App) {
    if page_insets(window, cx) == insets {
        return;
    }
    edges_mut(window, cx).insets = insets;
    // Usually called while the window draws, when a refresh is not taken.
    window.on_next_frame(|window, _| window.refresh());
}

/// An element filling its parent that keeps `bar_inset` at the parent's
/// height: in what a shell draws over the top of its pages.
pub fn bar_probe() -> impl IntoElement {
    canvas(
        |bounds, window, cx| set_bar_inset(bounds.size.height, window, cx),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// An element filling its parent that tells `told` its bounds as it is laid
/// out: where something stands, for what moves to it.
pub fn bounds_probe(told: impl FnOnce(Bounds<Pixels>, &mut App) + 'static) -> impl IntoElement {
    canvas(move |bounds, _, cx| told(bounds, cx), |_, _, _, _| {})
        .absolute()
        .size_full()
}

/// An element filling its parent that keeps `foot_inset` at the parent's
/// height: in what a shell draws over the foot of its pages.
pub fn foot_probe() -> impl IntoElement {
    canvas(
        |bounds, window, cx| set_foot_inset(bounds.size.height, window, cx),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// An element filling its parent, the part of a page a list scrolls in, that
/// holds the list's keyboard cursor clear of the bar and the foot drawn over
/// the page each time the keys move it: the list brings the row on screen
/// by its own lights, then this nudges it out from under them. After the
/// list among the parent's children, so the row has been laid out.
pub fn keep_clear<S: ScrollSource>(spot: &CursorSpot, source: &S) -> impl IntoElement {
    let (spot, source) = (spot.clone(), source.clone());
    canvas(
        move |area, window, cx| {
            let Some(at) = spot.bounds() else {
                return;
            };
            if !spot.clearing() {
                return;
            }
            let top = area.top() + bar_inset(window, cx);
            let bottom = area.bottom() - foot_inset(window, cx);
            let by = if at.top() < top {
                top - at.top()
            } else if at.bottom() > bottom {
                bottom - at.bottom()
            } else {
                return;
            };
            let offset = source.offset();
            let y = (offset.y + by).clamp(-source.max_offset(), Pixels::ZERO);
            source.set_offset(gpui::point(offset.x, y));
            window.refresh();
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// Tells the window's frost whether the page's content now runs under its
/// bar and under its foot: what a scroll passing under them reports as it
/// is laid out. A change fades the frost in or out on the GPU, with no
/// frame drawn.
pub fn report_scroll_edges(under_bar: bool, under_foot: bool, window: &mut Window, cx: &mut App) {
    let edges = edges_mut(window, cx);
    if edges.reported.replace((under_bar, under_foot)).is_some() {
        return;
    }
    let id = window.window_handle().window_id();
    cx.defer(move |cx| {
        let Some(edges) = cx.default_global::<WindowEdges>().by_window.get_mut(&id) else {
            return;
        };
        let Some(under) = edges.reported.take() else {
            return;
        };
        if edges.under == under {
            return;
        }
        let was = std::mem::replace(&mut edges.under, under);
        let fade = |animated: &Animated, on: bool| {
            animated.to(0, if on { 1.0 } else { 0.0 }, Motion::NORMAL, Ease::InOut)
        };
        if was.0 != under.0 {
            fade(&edges.bar, under.0);
        }
        if was.1 != under.1 {
            fade(&edges.foot, under.1);
        }
        if !SHADED {
            cx.refresh_windows();
        }
    });
}

/// Names the page the window shows, as its shell draws: when it is another
/// page than last time, the scroll edges are cleared, so its own scroll
/// reports them again as it is laid out and a page with none leaves the
/// frost clear.
pub fn page_shown(page: impl Hash, window: &mut Window, cx: &mut App) {
    let mut hasher = DefaultHasher::new();
    page.hash(&mut hasher);
    let page = hasher.finish();
    let edges = edges_mut(window, cx);
    if edges.page == Some(page) {
        return;
    }
    edges.page = Some(page);
    report_scroll_edges(false, false, window, cx);
    // A page drawn from a cached view would otherwise keep its last
    // prepaint, and its scroll would never report its edges again.
    window.refresh();
}

/// How much a frosted ground blurs what lies under it.
const FROST_BLUR: Pixels = px(14.);
/// How much of the ground's colour a frosted ground lays over the blur.
const FROST_TINT: f32 = 0.45;
/// A frost's tint: track 0 how much of it shows; `u[0..4]` its colour.
static FROST: ShaderProgram = ShaderProgram {
    name: "koan-frost",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    return float4(u[0], u[1], u[2], u[3] * koan_track(u, 0, time));
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(u[0].xyz, u[0].w * koan_track(u, 0u, time));
}
"#,
    prepare: None,
};

/// Which of a window's edges a frost grounds.
#[derive(Clone, Copy)]
enum Edge {
    Bar,
    Foot,
}

/// The ground of what a shell draws over its pages, filling its parent,
/// which is `relative`, and drawn before the rest of it: clear while
/// nothing is under it, and frosted as the system's bars are while the
/// page's content runs under it (what lies under blurred, and under a foot
/// tinted with the page's ground; a bar is tinted only where the renderer
/// cannot blur). With less
/// transparency asked for, opaque and unblurred while content is under it.
#[derive(IntoElement)]
pub struct Frost {
    edge: Edge,
}

impl Frost {
    /// Under a bar at the top of the pages.
    pub fn bar() -> Self {
        Self { edge: Edge::Bar }
    }

    /// Under a foot at the end of the pages.
    pub fn foot() -> Self {
        Self { edge: Edge::Foot }
    }
}

impl RenderOnce for Frost {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ground = look.palette.bg.hsla();
        let color = if look.opaque {
            ground
        } else {
            ground.opacity(FROST_TINT)
        };
        let edges = edges_mut(window, cx);
        let (animated, under) = match self.edge {
            Edge::Bar => (edges.bar.clone(), edges.under.0),
            Edge::Foot => (edges.foot.clone(), edges.under.1),
        };
        // The blur, whole from the first pixel to the last.
        let blur = (!look.opaque).then(|| div().absolute().inset_0().backdrop_blur(FROST_BLUR));
        // A bar is blurred alone where the renderer blurs: a tint of the
        // ground over it darkens a bold wash. The foot keeps the tint, which
        // sets the transport apart from the page. The ground stays for less
        // transparency, and as the whole frost where nothing blurs.
        let tint = if SHADED && !look.opaque && matches!(self.edge, Edge::Bar) {
            None
        } else if SHADED {
            animated.set_color(0, color);
            AnimatedQuad::new(&FROST, &animated)
                .decorative()
                .absolute()
                .inset_0()
                .into_any_element()
                .into()
        } else {
            Some(
                div()
                    .absolute()
                    .inset_0()
                    .when(under, |d| d.bg(color))
                    .into_any_element(),
            )
        };
        div().absolute().inset_0().children(blur).children(tint)
    }
}

pub trait KoanFrosted: Styled + Sized {
    /// A frosted ground standing as it is, whatever is under it: what lies
    /// under blurred. With less
    /// transparency asked for, the ground opaque and nothing blurred. Where
    /// the renderer cannot blur, the tint alone. A bar a page scrolls under
    /// takes `Frost`, which stays clear while nothing is.
    fn frosted(self, look: &Look) -> Self {
        let ground = look.palette.bg.hsla();
        if look.opaque {
            self.bg(ground)
        } else if SHADED {
            self.backdrop_blur(FROST_BLUR)
        } else {
            self.backdrop_blur(FROST_BLUR)
                .bg(ground.opacity(FROST_TINT))
        }
    }
}

impl<E: Styled> KoanFrosted for E {}
