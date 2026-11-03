//! The kōan theme's components, drawn from its tokens: flat, square, and in
//! Geist Mono. docs/design/koan-theme.md#components describes each; the
//! comments here say only where GPUI asks for something the spec does not.
//!
//! Containers (`Page`, `Toolbar`, `Form`, `Menu`, the table's rows) own the
//! spacing and metrics, and pages declare only content. The design, what
//! each piece owns and the map from the Swift roles to these, is
//! docs/design/koan-kit.md.
//!
//! Every component is a `RenderOnce` element that reads the window's `Look`
//! as it renders. Interactive ones take an id, under which they keep what
//! the pointer is doing and the state of their transitions.

mod animated;
mod answer;
mod availability;
mod badge;
mod banner;
mod bars;
mod button;
pub mod choice;
mod collection;
mod copy;
mod cover;
mod dense;
mod details;
mod drag;
mod empty;
mod fade;
mod field;
mod filter_field;
mod form;
mod heart;
mod inspector;
mod level;
mod link_text;
mod list_keys;
mod lists;
mod menu;
mod metrics;
mod page;
mod panel;
mod picker;
mod pill;
pub(crate) mod placeholder;
mod progress;
mod qr;
mod ring;
mod row_list;
mod rows;
mod scroll;
mod segmented;
mod selection_bar;
pub mod sheet;
mod slide;
mod slider;
mod sparkline;
mod spinner;
mod split;
mod status;
mod stepper;
mod suggestions;
mod table;
mod tabs;
mod tile;
mod toggle;
mod toolbar;
mod transport_control;
mod viewer;
mod window;

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, Bounds, Div, ElementId, Entity, FocusHandle, Global, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, SharedString, StatefulInteractiveElement,
    Styled, Window, canvas, div, px,
};

pub use animated::{Animated, AnimatedQuad, SHADED, SHADER_UNIFORMS, ShaderProgram};
pub use answer::{ANSWER, Answer, Answerer};
pub use availability::{Availability, AvailabilityMark, RingGlide};
pub use badge::{Badge, Chip};
pub use banner::Banner;
pub use bars::{BARS, BarLevels, BarsPlace, FLOOR, PlayingBars, bars_overlay};
pub use button::{Button, ButtonKind};
pub use collection::{Collection, CollectionSection, CollectionWidth};
pub use copy::{CopyButton, CopyText};
pub use cover::{Cover, CoverArt, Mosaic};
pub use dense::{Align, Column, DenseRow, DenseTable};
pub use details::DetailGroup;
pub use drag::{DragChip, DropTarget, INSERTION};
pub use list_keys::{LIST_CONTEXT, ListCursor, Moved};
/// The list keys' actions, by name: `list_actions::Down`.
pub mod list_actions {
    pub use super::list_keys::{
        Activate, Down, ExtendDown, ExtendLeft, ExtendRight, ExtendUp, First, Last, Left, PageDown,
        PageUp, Pick, Right, ShowMenu, Up,
    };
}
pub use empty::EmptyState;
pub use fade::{FadeIn, Reveal};
pub use field::{FieldState, InputEvent, TextField};
pub use filter_field::FilterField;
pub use form::{
    Form, FormRow, FormSection, SectionHeader, form_buttons, form_entry, form_item, form_note,
    form_text,
};
pub use heart::Heart;
pub use inspector::{Inspector, SyncedLines, plain_text};
pub use level::Level;
pub use link_text::{link_action, link_text, name_line};
pub use lists::{
    List, ListAlignment, ListOffset, ListState, OVERDRAW, ScrollStrategy, UniformList,
    UniformListScrollHandle, list, list_scrollbar, offset_down, reveal_around, spoken_list,
    uniform_list, width_probe,
};
pub use menu::{KoanContextMenu, Menu, MenuEntry, MenuSlot};
pub use metrics::{
    BarMetrics, CentreRows, RowKind, RowMetrics, TileColumns, TileGrid, TransportMetrics,
};
pub use page::{Page, PageHeader, PlayButton, SectionHeading, ShuffleButton};
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use panel::TOOLTIPS_DRAWN;
pub use panel::{MenuPanel, MenuRow, MenuSeparator, Tooltip, popover_panel};
pub use picker::Picker;
pub use pill::Pill;
pub use placeholder::{Placeholder, enso_svg};
pub use progress::{ProgressBar, ProgressRow};
pub use qr::QrCode;
pub use ring::ProgressRing;
pub use row_list::{RowList, RowListState, RowOffset};
pub use rows::{ListRow, NavHeading, NavNote, NavRow};
pub use scroll::ScrollArea;
pub use segmented::Segmented;
pub use selection_bar::SelectionBar;
pub use slider::{Glide, Slider};
pub use sparkline::Sparkline;
pub use spinner::Spinner;
pub use split::SplitView;
pub use status::StatusBlock;
pub use stepper::Stepper;
pub use suggestions::{SuggestionHeading, SuggestionPanel, SuggestionRow};
pub use table::{ROW_GROUP, RowNumber, TableRow, table_list};
pub use tabs::{StatusLine, TabStrip};
pub use tile::{TileArt, TileMark, TileSleeve, origin_probe, text_widths};
pub use toggle::{IconToggle, Toggle};
pub use toolbar::{
    Dismiss, TOOLBAR_HEIGHT, Toolbar, ToolbarButton, ToolbarGroup, ToolbarMenu, ToolbarMetrics,
    ToolbarSelection, ToolbarTitle, WINDOW_BUTTONS,
};
pub use transport_control::TransportControl;
pub use viewer::Viewer;
pub use window::SecondaryWindow;

use crate::theme::{Look, MARK, Motion};

/// A handler a component calls with the window and the app.
pub type Handler<T> = Arc<dyn Fn(&T, &mut Window, &mut App) + 'static>;

/// What the pointer is doing to a control.
#[derive(Default)]
pub(crate) struct Pointer {
    pub hovered: bool,
    pub pressed: bool,
}

pub(crate) fn pointer(id: &ElementId, window: &mut Window, cx: &mut App) -> Entity<Pointer> {
    window.use_keyed_state(id.clone(), cx, |_, _| Pointer::default())
}

/// Keeps `state` up to date with the pointer over `element`.
pub(crate) fn track<E: InteractiveElement + StatefulInteractiveElement>(
    element: E,
    state: &Entity<Pointer>,
) -> E {
    let set = |state: &Entity<Pointer>, change: fn(&mut Pointer)| {
        let state = state.clone();
        move |cx: &mut App| {
            state.update(cx, |pointer, cx| {
                change(pointer);
                cx.notify();
            })
        }
    };
    let hover = state.clone();
    let down = set(state, |p| p.pressed = true);
    let up = set(state, |p| p.pressed = false);
    let up_out = set(state, |p| p.pressed = false);
    element
        .on_hover(move |hovered, _, cx| {
            let hovered = *hovered;
            hover.update(cx, |pointer, cx| {
                pointer.hovered = hovered;
                cx.notify();
            })
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| down(cx))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| up(cx))
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| up_out(cx))
}

/// A focus handle kept under `id`, so a `RenderOnce` control can take the
/// keyboard and show where it is.
pub(crate) fn focus(id: &ElementId, window: &mut Window, cx: &mut App) -> FocusHandle {
    window
        .use_keyed_state(child(id, "focus"), cx, |_, cx| {
            cx.focus_handle().tab_stop(true)
        })
        .read(cx)
        .clone()
}

pub(crate) fn child(id: &ElementId, name: &'static str) -> ElementId {
    ElementId::NamedChild(Arc::new(id.clone()), SharedString::new_static(name))
}

/// A state moving between off (0) and on (1) over a motion token, from
/// wherever it stood. Instant with Reduce Motion.
struct Fade {
    on: bool,
    from: f32,
    started: Option<Instant>,
    duration: Duration,
}

impl Fade {
    fn at(&self, now: Instant) -> f32 {
        let target = if self.on { 1.0 } else { 0.0 };
        let Some(started) = self.started else {
            return target;
        };
        let t = now.duration_since(started).as_secs_f32() / self.duration.as_secs_f32();
        if t >= 1.0 {
            return target;
        }
        self.from + (target - self.from) * Motion::quick_out(t)
    }
}

/// How far a state has moved toward `on`, kept under `key`. The first frame
/// a control is drawn it stands where it is, with nothing to animate; frames
/// are asked for only while a change is under way.
pub(crate) fn fade(
    key: ElementId,
    on: bool,
    duration: Duration,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let now = Instant::now();
    let reduce = cx.reduce_motion();
    let state = window.use_keyed_state(key, cx, |_, _| Fade {
        on,
        from: 0.0,
        started: None,
        duration,
    });
    let (value, moving) = state.update(cx, |fade, _| {
        if fade.on != on {
            fade.from = fade.at(now);
            fade.on = on;
            fade.started = (!reduce).then_some(now);
        }
        let value = fade.at(now);
        let moving = fade
            .started
            .is_some_and(|started| now.duration_since(started) < fade.duration);
        (value, moving)
    });
    if moving {
        window.request_animation_frame();
    }
    value
}

/// The keyboard's focus ring, 2 points of accent standing 4 outside the
/// control, drawn over it as its last child. The control is `relative`.
pub(crate) fn ring(handle: &FocusHandle, look: &Look, window: &Window) -> Option<Div> {
    let focused = handle.is_focused(window) && window.last_input_was_keyboard();
    focused.then(|| {
        let standoff = px(-4.0);
        div()
            .absolute()
            .top(standoff)
            .left(standoff)
            .right(standoff)
            .bottom(standoff)
            .border(MARK)
            .border_color(look.accent.hsla())
    })
}

/// A keyboard stop kept under `id`, for a control a page draws itself: its
/// element tracks it, and GPUI turns Return and Space on it into a click.
pub fn tab_stop(id: &ElementId, window: &mut Window, cx: &mut App) -> FocusHandle {
    focus(id, window, cx)
}

/// The focus ring for `handle`, as the kit's own controls draw it: shown
/// while it holds the keyboard and the keyboard was used last.
pub fn focus_ring(handle: &FocusHandle, window: &Window, cx: &App) -> Option<Div> {
    ring(handle, &Look::of(window, cx), window)
}

/// The focus ring drawn inside a row, for the row a list's keyboard cursor
/// is on: the list holds the focus, so the row's ring sits within it rather
/// than standing off over its neighbours.
pub(crate) fn cursor_ring(look: &Look) -> Div {
    div()
        .absolute()
        .inset_0()
        .border(MARK)
        .border_color(look.accent.hsla())
        .child(
            canvas(
                |bounds, window, cx| {
                    let id = window.window_handle().window_id();
                    cx.default_global::<CursorAt>().0 = Some((id, bounds));
                },
                |_, _, _, _| {},
            )
            .size_full(),
        )
}

/// Where the keyboard's cursor ring was drawn, and in which window: what the
/// menu key opens a context menu under. The key clears it and lets a frame
/// draw, so only a ring drawn in that frame counts.
#[derive(Default)]
pub(crate) struct CursorAt(pub Option<(gpui_kit::WindowId, Bounds<Pixels>)>);

impl Global for CursorAt {}

/// Two colours mixed, `t` of the way from `a` to `b`.
pub(crate) fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (gpui_kit::Rgba::from(a), gpui_kit::Rgba::from(b));
    let at = |x: f32, y: f32| x + (y - x) * t;
    gpui_kit::Rgba {
        r: at(a.r, b.r),
        g: at(a.g, b.g),
        b: at(a.b, b.b),
        a: at(a.a, b.a),
    }
    .into()
}

/// A fill or an outline faded in and out with a control's state by the GPU:
/// the ramp is handed over when the state changes and eased at each present,
/// running for the motion's length and then stopping by itself. Instant with
/// Reduce Motion. Uniforms: 0 from, 1 to, 2 the start in seconds of shader
/// time, 3 the length in seconds, 4..8 the colour, 8 the outline's width in
/// device pixels (0 for a fill); `prepare` writes the amount (9).
static TRANSITION: ShaderProgram = ShaderProgram {
    name: "koan-transition",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    if (u[8] > 0.0) {
        float2 edge = min(position, size - position);
        if (min(edge.x, edge.y) > u[8]) {
            return float4(0.0);
        }
    }
    return float4(u[4], u[5], u[6], u[7] * u[9]);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 4>, tile: vec4<f32>) -> vec4<f32> {
    if (u[2].x > 0.0) {
        let edge = min(position, size - position);
        if (min(edge.x, edge.y) > u[2].x) {
            return vec4<f32>(0.0);
        }
    }
    return vec4<f32>(u[1].xyz, u[1].w * u[2].y);
}
"#,
    prepare: Some(|u, time| {
        let t = if u[3] > 0.0 {
            ((time - u[2]) / u[3]).clamp(0.0, 1.0)
        } else {
            1.0
        };
        u[9] = u[0] + (u[1] - u[0]) * Motion::quick_out(t);
    }),
};

/// A state's transition between off and on, kept under `key`: what draws it
/// on the GPU. The first frame a control is drawn it stands where it is.
pub(crate) fn transition(
    key: ElementId,
    on: bool,
    duration: Duration,
    window: &mut Window,
    cx: &mut App,
) -> Animated {
    let now = Instant::now();
    let reduce = cx.reduce_motion();
    let state = window.use_keyed_state(key, cx, |_, _| {
        let animated = Animated::new();
        let at = if on { 1.0 } else { 0.0 };
        animated.set_all(0, &[at, at, 0.0, 0.0]);
        (
            Fade {
                on,
                from: at,
                started: None,
                duration,
            },
            now,
            animated,
        )
    });
    state.update(cx, |(fade, origin, animated), _| {
        if fade.on != on {
            fade.from = fade.at(now);
            fade.on = on;
            fade.started = (!reduce).then_some(now);
            let start = now.duration_since(*origin).as_secs_f32();
            let length = if reduce { 0.0 } else { duration.as_secs_f32() };
            animated.set_origin(*origin);
            animated.set_all(0, &[fade.from, if on { 1.0 } else { 0.0 }, start, length]);
            if !reduce {
                animated.run_for(duration);
            }
        }
        animated.clone()
    })
}

/// A fill under a control, in `color`, faded with `transition`.
pub(crate) fn fill(color: Hsla, transition: &Animated) -> AnyElement {
    if !SHADED {
        let amount = transition.get(1);
        return div()
            .absolute()
            .size_full()
            .bg(color.opacity(amount))
            .into_any_element();
    }
    let rgba = color.to_rgb();
    transition.set_all(4, &[rgba.r, rgba.g, rgba.b, rgba.a, 0.0]);
    AnimatedQuad::new(&TRANSITION, transition)
        .absolute()
        .size_full()
        .into_any_element()
}

/// A control as a screen reader meets it: its role and its name. The tree
/// is built only while assistive technology is attached, so this costs
/// nothing otherwise. See docs/design/koan-kit.md, "Accessibility".
pub(crate) fn accessible<E: StatefulInteractiveElement>(
    element: E,
    role: gpui_kit::Role,
    name: Option<SharedString>,
) -> E {
    let element = element.role(role);
    match name {
        Some(name) => element.aria_label(name),
        None => element,
    }
}

/// Answers a screen reader's click as a pointer's would, with `handler`.
/// A row's or a tile's parts as a screen reader says them: the non-empty
/// ones, in order, between commas. What `TableRow::spoken` takes.
pub fn spoken<S: AsRef<str>>(parts: impl IntoIterator<Item = S>) -> SharedString {
    parts
        .into_iter()
        .filter(|part| !part.as_ref().is_empty())
        .map(|part| part.as_ref().to_owned())
        .collect::<Vec<_>>()
        .join(", ")
        .into()
}

/// Something the app says on its own, read out as it appears without
/// moving focus: politely, or at once when something went wrong. The text is
/// both name and value, since VoiceOver announces a live node's value and
/// Orca its name; and the element must be a leaf, since a live region's
/// descendants are live too and would be announced again.
pub(crate) fn announced<E: StatefulInteractiveElement>(
    element: E,
    text: SharedString,
    tone: crate::theme::Tone,
) -> E {
    let alarm = tone == crate::theme::Tone::Bad;
    accessible(
        element,
        if alarm {
            gpui_kit::Role::Alert
        } else {
            gpui_kit::Role::Status
        },
        Some(text.clone()),
    )
    .aria_value(text)
    .aria_live(if alarm {
        gpui_kit::accesskit::Live::Assertive
    } else {
        gpui_kit::accesskit::Live::Polite
    })
}

/// A checked state as AccessKit has it.
pub(crate) fn toggled(on: bool) -> gpui_kit::Toggled {
    if on {
        gpui_kit::Toggled::True
    } else {
        gpui_kit::Toggled::False
    }
}

pub(crate) fn answers_click<E: StatefulInteractiveElement>(
    element: E,
    handler: Handler<gpui_kit::ClickEvent>,
) -> E {
    element.on_a11y_action(gpui_kit::AccessibleAction::Click, move |_, window, cx| {
        handler(&gpui_kit::ClickEvent::default(), window, cx)
    })
}
