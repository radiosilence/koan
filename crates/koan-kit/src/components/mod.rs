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
mod arrive;
mod availability;
mod badge;
mod banner;
mod bars;
mod button;
pub mod choice;
mod collection;
mod command_bar;
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
use std::time::Instant;

use gpui_kit::{
    AnyElement, App, Bounds, ColorSlot, Div, ElementId, Entity, FocusHandle, Global, Hsla,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Rgba, SharedString,
    StatefulInteractiveElement, Styled, TrackCurve, Window, canvas, div, px,
};

pub use animated::{
    Animated, AnimatedQuad, Ease, Repeat, SHADED, SHADER_PLAIN, SHADER_TRACKS, SHADER_UNIFORMS,
    ShaderProgram, Track,
};
pub use answer::{ANSWER, Answer, Answerer};
pub use arrive::{Arrival, arriving};
pub use availability::{Availability, AvailabilityMark, RingGlide};
pub use badge::{Badge, Chip};
pub use banner::Banner;
pub use bars::{BARS, BarLevels, BarsPlace, FLOOR, PlayingBars, bars_overlay, renderer_mark};
pub use button::{Button, ButtonKind};
pub use collection::{Collection, CollectionSection, CollectionWidth};
pub use command_bar::{CommandBar, CommandSection};
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
pub use slide::{Slide, SlideState};
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

/// A colour moving between its off and on colours on the GPU, through a
/// colour slot (#1859): the control draws once with the slot's colour as a
/// leave fade starts, and the presents that follow move it with no frame
/// drawn. The slot is held only for the fade, and let go at the control's
/// next draw after it lands; at rest the control draws a plain colour.
struct ColorFade {
    on: bool,
    /// The slot fading and when it lands, while a leave fade runs.
    fading: Option<(ColorSlot, Instant)>,
}

/// The colour a control's text, glyph or edge draws in, `on_color` when `on`
/// and `off_color` otherwise, kept under `key`. Arriving is at once and
/// leaving lingers: a hover lights a control as the pointer reaches it and
/// fades over `Motion::LEAVE` once it has gone. Instant with Reduce Motion,
/// and when every slot is taken.
pub(crate) fn fade(
    key: ElementId,
    on: bool,
    off_color: Hsla,
    on_color: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> Hsla {
    let reduce = cx.reduce_motion();
    let state = window.use_keyed_state(key, cx, |_, _| ColorFade { on, fading: None });
    state.update(cx, |fade, _| {
        let now = Animated::now();
        if fade.on != on {
            fade.on = on;
            fade.fading = (!on && !reduce)
                .then(|| ColorSlot::claim(Rgba::from(on_color)))
                .flatten()
                .map(|slot| {
                    slot.to(
                        Rgba::from(off_color),
                        Track {
                            from: 0.0,
                            to: 1.0,
                            start: 0.0,
                            length: Motion::LEAVE.as_secs_f32(),
                            curve: TrackCurve::Bezier(0.2, 0.9, 0.3, 1.0),
                            repeat: Repeat::Once,
                        },
                    );
                    (slot, now + Motion::LEAVE)
                });
        } else if fade.fading.as_ref().is_some_and(|(_, lands)| now >= *lands) {
            fade.fading = None;
        }
        match &fade.fading {
            Some((slot, _)) => slot.hsla(1.0),
            None if on => on_color,
            None => off_color,
        }
    })
}

/// A panel's openness between 0 and 1, eased toward `on` over `duration`
/// from wherever it stood and kept under `key`: what `Slide` and
/// `SplitView::shown` lay a width out by. Layout moves on the CPU, frame by
/// frame while it eases, until panels slide as animated layers the GPU
/// composes. Instant with Reduce Motion.
pub(crate) fn ease(
    key: ElementId,
    on: bool,
    duration: std::time::Duration,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    struct Ease {
        on: bool,
        from: f32,
        started: Option<Instant>,
    }
    let now = Instant::now();
    let reduce = cx.reduce_motion();
    let at = |e: &Ease| {
        let target = if e.on { 1.0 } else { 0.0 };
        match e.started {
            Some(started) => {
                let t = now.duration_since(started).as_secs_f32() / duration.as_secs_f32();
                if t >= 1.0 {
                    target
                } else {
                    e.from + (target - e.from) * Motion::quick_out(t)
                }
            }
            None => target,
        }
    };
    let state = window.use_keyed_state(key, cx, |_, _| Ease {
        on,
        from: 0.0,
        started: None,
    });
    let (value, moving) = state.update(cx, |e, _| {
        if e.on != on {
            e.from = at(e);
            e.on = on;
            e.started = (!reduce).then_some(now);
        }
        let moving = e.started.is_some_and(|s| now.duration_since(s) < duration);
        (at(e), moving)
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

/// A fill or an outline following a control's state on the GPU: lit at once
/// as the state comes on (the pointer arrives), and faded out over
/// `Motion::LEAVE` as it goes off, by its track. Uniforms: 0..4 the colour,
/// 4 the outline's width in device pixels (0 for a fill); track 0 the amount.
static TRANSITION: ShaderProgram = ShaderProgram {
    name: "koan-transition",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    if (u[4] > 0.0) {
        float2 edge = min(position, size - position);
        if (min(edge.x, edge.y) > u[4]) {
            return float4(0.0);
        }
    }
    return float4(u[0], u[1], u[2], u[3] * koan_track(u, 0, time));
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    if (u[1].x > 0.0) {
        let edge = min(position, size - position);
        if (min(edge.x, edge.y) > u[1].x) {
            return vec4<f32>(0.0);
        }
    }
    return vec4<f32>(u[0].xyz, u[0].w * koan_track(u, 0u, time));
}
"#,
    prepare: None,
};

/// A state's transition between off and on, kept under `key`: its amount
/// as track 0, on at once and off over `Motion::LEAVE`. The first frame a
/// control is drawn it stands where it is.
pub(crate) fn transition(key: ElementId, on: bool, window: &mut Window, cx: &mut App) -> Animated {
    let state = window.use_keyed_state(key, cx, |_, _| {
        let animated = Animated::new();
        animated.hold(0, if on { 1.0 } else { 0.0 });
        (on, animated)
    });
    state.update(cx, |(was, animated), _| {
        if *was != on {
            *was = on;
            if on {
                animated.hold(0, 1.0);
            } else {
                animated.to(0, 0.0, Motion::LEAVE, Ease::QuickOut);
            }
        }
        animated.clone()
    })
}

/// A fill under a control, in `color`, faded with `transition`.
pub(crate) fn fill(color: Hsla, transition: &Animated) -> AnyElement {
    if !SHADED {
        let amount = transition.at(0, Animated::now());
        return div()
            .absolute()
            .size_full()
            .bg(color.opacity(amount))
            .into_any_element();
    }
    let rgba = color.to_rgb();
    transition.set_all(0, &[rgba.r, rgba.g, rgba.b, rgba.a, 0.0]);
    AnimatedQuad::new(&TRANSITION, transition)
        .decorative()
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
