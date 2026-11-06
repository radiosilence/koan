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
mod program;
mod slot;
mod track;
pub(crate) use animated::curve;
mod answer;
mod appear;
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
pub mod field;
mod filter_field;
pub mod focus_trap;
mod form;
mod guide;
mod heart;
mod insets;
mod inspector;
mod layer;
mod level;
mod link_text;
mod list_keys;
mod lists;
mod lyrics;
mod menu;
mod metrics;
mod page;
mod panel;
mod picker;
mod pill;
pub(crate) mod placeholder;
mod popover;
mod prism;
mod progress;
mod qr;
mod ring;
mod root;
mod row_list;
mod rows;
mod scroll;
mod scrollbar;
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
mod touch;
mod transport_control;
mod viewer;
mod window;

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, Div, ElementId, Entity, FocusHandle, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Rgba, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Task, Window, canvas, div, px,
};

pub use animated::{
    Animated, AnimatedQuad, Ease, Repeat, SHADED, SHADER_PLAIN, SHADER_TRACKS, SHADER_UNIFORMS,
    ShaderProgram, TRACK_FLOATS, Track, WeakAnimated,
};
pub use answer::{ANSWER, Answer, Answerer, GLYPH, Morph};
pub use appear::{Fading, Rise, SlideUp, Veil};
pub use arrive::{Arrival, arriving};
pub use availability::{Availability, AvailabilityMark, RingGlide};
pub use badge::{Badge, Chip};
pub use banner::Banner;
pub use bars::{BARS, BarLevels, BarsPlace, FLOOR, PlayingBars, bars_overlay, renderer_mark};
pub use button::{Button, ButtonKind};
pub use collection::{Collection, CollectionSection, CollectionWidth};
pub use command_bar::{CommandBar, CommandResults, CommandSection};
pub use copy::{CopyButton, CopyText};
pub use cover::{Cover, CoverArt, Mosaic};
pub use dense::{Align, Column, DenseRow, DenseTable};
pub use details::DetailGroup;
pub use drag::{DragChip, DropTarget, INSERTION};
pub use list_keys::{LIST_CONTEXT, ListCursor, Moved};
pub use slot::{ColorSlot, install as install_color_slots, present_now};
pub use track::{TrackCurve, spring_length, track_at};
/// The list keys' actions, by name: `list_actions::Down`.
pub mod list_actions {
    pub use super::list_keys::{
        Activate, Down, ExtendDown, ExtendLeft, ExtendRight, ExtendUp, First, Last, Left, PageDown,
        PageUp, Pick, Right, ShowMenu, Up,
    };
}
pub use empty::EmptyState;
pub use fade::{FadeIn, Reveal};
pub use field::{AreaState, FieldState, InputEvent, Labelled, TextArea, TextField, labelled};
pub use filter_field::{FilterButton, FilterField};
pub use form::{
    Form, FormRow, FormSection, SectionHeader, form_buttons, form_entry, form_field, form_item,
    form_note, form_text,
};
pub use guide::{Check, CheckItem, ChoiceCards, Steps};
pub use heart::Heart;
pub use insets::{
    Clear, KoanFrosted, KoanInsets, bar_inset, bar_probe, foot_inset, foot_probe, set_bar_inset,
    set_foot_inset, system_insets,
};
pub use inspector::{Inspector, plain_text};
pub use layer::{AnimatedLayer, LAYER};
pub use level::Level;
pub use link_text::{link_action, link_text, name_line};
pub use lists::{
    List, ListAlignment, ListOffset, ListState, OVERDRAW, ScrollStrategy, UniformList,
    UniformListScrollHandle, list, list_scrollbar, offset_down, reveal_around, reveal_around_list,
    rows_under_foot, scrolls_away, spoken_list, uniform_list, width_probe,
};
pub use lyrics::{LYRICS, SyncedLines};
pub use menu::{KoanContextMenu, Menu, MenuEntry, MenuSlot, on_menu_focus_lost};
pub use metrics::{
    BarMetrics, CentreRows, RowKind, RowMetrics, TileColumns, TileGrid, TransportMetrics,
};
pub use page::{
    Page, PageHeader, PlayButton, SectionHeading, ShuffleButton, head as page_head, title_text,
};
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use panel::TOOLTIPS_DRAWN;
pub use panel::{MenuPanel, MenuRow, MenuSeparator, Tooltip, anchored_panel, popover_panel};
pub use picker::Picker;
pub use pill::Pill;
pub use placeholder::{Placeholder, enso_svg};
pub use prism::{Burst, FlashGate, Prism, PrismFill, Prisms, Sheen};
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
pub use toggle::{CheckBox, IconToggle, Toggle};
pub use toolbar::{
    Dismiss, TOOLBAR_HEIGHT, TextTrigger, Toolbar, ToolbarButton, ToolbarGroup, ToolbarMenu,
    ToolbarMetrics, ToolbarSelection, ToolbarTitle, WINDOW_BUTTONS,
};
pub use touch::{KoanLongPress, KoanTouchDrag};
pub use transport_control::TransportControl;
pub use viewer::Viewer;
pub use window::{
    MainWindow, SecondaryWindow, close_button, draws_decorations, host, moves_window,
    window_buttons,
};

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
        .on_hover(move |hovered, window, cx| {
            let hovered = *hovered;
            // A touch hovers only what it holds down, and a tap's press
            // arrives with its release: held is pressed.
            let touch = window.last_input_was_touch();
            hover.update(cx, |pointer, cx| {
                pointer.hovered = hovered;
                if touch {
                    pointer.pressed = hovered;
                }
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

/// A control's name for a screen reader where its owner gave none: its id
/// read as words ("wash-tint" as "wash tint"), so no slider or choice is met
/// unnamed. An id that is not a name gives none.
pub(crate) fn spoken_id(id: &ElementId) -> Option<SharedString> {
    match id {
        ElementId::Name(name) | ElementId::NamedInteger(name, _) => {
            Some(name.replace(['-', '_'], " ").into())
        }
        _ => None,
    }
}

pub(crate) fn child(id: &ElementId, name: &'static str) -> ElementId {
    ElementId::NamedChild(Arc::new(id.clone()), SharedString::new_static(name))
}

/// A colour moving between its off and on colours on the GPU, through a
/// colour slot (#1859): the control draws once with the slot's colour as a
/// leave fade starts, and the presents that follow move it with no frame
/// drawn. The slot is held only for the fade: as it lands the control is
/// drawn once more in its plain colour, and that draw lets the slot go, so
/// the table's slots are held only by fades still running.
struct ColorFade {
    on: bool,
    /// The slot fading and when it lands, while a leave fade runs.
    fading: Option<(ColorSlot, Instant)>,
    /// Wakes the control as the fade lands.
    _landing: Option<Task<()>>,
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
    let state = window.use_keyed_state(key, cx, |_, _| ColorFade {
        on,
        fading: None,
        _landing: None,
    });
    state.update(cx, |fade, cx| {
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
                            curve: curve(Ease::InOut),
                            repeat: Repeat::Once,
                        },
                    );
                    (slot, now + Motion::LEAVE)
                });
            // One wake at the landing, which draws the plain colour and lets
            // the slot go.
            fade._landing = fade.fading.as_ref().map(|(_, lands)| {
                // Counted to the landing on the present clock, which runs
                // ahead of the wall clock: woken then, the draw sees it landed.
                let wait = lands.saturating_duration_since(Instant::now());
                cx.spawn(async move |fade, cx| {
                    cx.background_executor().timer(wait).await;
                    fade.update(cx, |_, cx| cx.notify()).ok();
                })
            });
        } else if fade.fading.as_ref().is_some_and(|(_, lands)| now >= *lands) {
            fade.fading = None;
            fade._landing = None;
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
/// frame while it eases, the page beside the panel with it. Instant with
/// Reduce Motion.
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

/// Where a list's or a grid's keyboard cursor was last laid out, owned by
/// the page that has the cursor and handed to what draws it (`.spot`). The
/// cursor's ring writes its bounds here as it is laid out, so the page's
/// scroll area can bring it into view (`ScrollArea::follow`) and the menu
/// key can open under it, both from layout rather than from a global.
#[derive(Clone, Default)]
pub struct CursorSpot(std::rc::Rc<Spot>);

#[derive(Default)]
struct Spot {
    bounds: std::cell::Cell<Option<Bounds<Pixels>>>,
    /// Bumped each time the keys move the cursor: what a scroll area
    /// follows, so moving to the same item again still scrolls to it.
    moves: std::cell::Cell<u64>,
}

impl CursorSpot {
    /// The keys moved the cursor: where it was is no longer where it is,
    /// and the next layout brings it into view.
    pub fn moved(&self) {
        self.0.bounds.set(None);
        self.0.moves.set(self.0.moves.get() + 1);
    }

    /// Where the cursor's ring was laid out since it last moved.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.0.bounds.get()
    }

    /// Where the cursor was laid out is no longer trusted, its moves left
    /// alone: the next layout says again.
    pub(crate) fn forget_bounds(&self) {
        self.0.bounds.set(None);
    }

    pub(crate) fn moves(&self) -> u64 {
        self.0.moves.get()
    }

    fn record(&self, bounds: Bounds<Pixels>) {
        self.0.bounds.set(Some(bounds));
    }
}

/// The focus ring drawn inside a row, for the row a list's keyboard cursor
/// is on: the list holds the focus, so the row's ring sits within it rather
/// than standing off over its neighbours. Laid out, it tells `spot` where.
pub(crate) fn cursor_ring(look: &Look, spot: Option<&CursorSpot>) -> Div {
    let ring = div()
        .absolute()
        .inset_0()
        .border(MARK)
        .border_color(look.accent.hsla());
    match spot.cloned() {
        Some(spot) => {
            ring.child(canvas(move |bounds, _, _| spot.record(bounds), |_, _, _, _| {}).size_full())
        }
        None => ring,
    }
}

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
                animated.to(0, 0.0, Motion::LEAVE, Ease::InOut);
            }
        }
        animated.clone()
    })
}

/// The fill under a control that lights under the pointer, as every
/// control's does: `surface` arriving with the pointer, `hover` while
/// pressed, fading out over `Motion::LEAVE` once it has gone. The caller
/// tracks `pointer` on the control with `track` and adds the fill before its
/// content, so the content draws over it.
pub(crate) fn hover_fill(
    id: &ElementId,
    look: &Look,
    window: &mut Window,
    cx: &mut App,
) -> (Entity<Pointer>, AnyElement) {
    let state = pointer(&child(id, "pointer"), window, cx);
    let (hovered, pressed) = {
        let p = state.read(cx);
        (p.hovered, p.pressed)
    };
    let color = if pressed {
        look.palette.hover.hsla()
    } else {
        look.palette.surface.hsla()
    };
    let lit = transition(child(id, "lit"), hovered || pressed, window, cx);
    (state, fill(color, &lit))
}

/// A control of a page's own whose colour changes under the pointer as the
/// kit's controls' do: to `lit` at once as the pointer arrives, back to
/// `rest` over `Motion::LEAVE` once it has gone. Returns the element, named
/// `id` and following the pointer, and the colour its content draws in now.
pub fn hover_tint(
    id: impl Into<ElementId>,
    rest: Hsla,
    lit: Hsla,
    window: &mut Window,
    cx: &mut App,
) -> (Stateful<Div>, Hsla) {
    let id = id.into();
    let state = pointer(&child(&id, "pointer"), window, cx);
    let hovered = state.read(cx).hovered;
    let color = fade(child(&id, "tint"), hovered, rest, lit, window, cx);
    (track(div().id(id), &state), color)
}

/// A row a click acts on, lit under the pointer as controls are (`surface`,
/// leaving over `Motion::LEAVE`): its content goes in as children, over the
/// fill.
pub fn lit_row(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> Stateful<Div> {
    let id = id.into();
    let look = Look::of(window, cx);
    let (state, fill) = hover_fill(&id, &look, window, cx);
    track(div().id(id), &state)
        .relative()
        .cursor_pointer()
        .child(fill)
}

/// A fill under a control, in `color`, faded with `transition`. Out, and
/// not fading, it is nothing: a control at rest puts no quad in the scene.
pub(crate) fn fill(color: Hsla, transition: &Animated) -> AnyElement {
    if transition.at(0, Animated::now()) <= 0.0 && !transition.moving() {
        return gpui::Empty.into_any_element();
    }
    if !SHADED {
        let amount = plain_amount(transition);
        return div()
            .absolute()
            .inset_0()
            .bg(color.opacity(amount))
            .into_any_element();
    }
    transition.set_color(0, color);
    transition.set(4, 0.0);
    AnimatedQuad::new(&TRANSITION, transition)
        .decorative()
        .absolute()
        .inset_0()
        .into_any_element()
}

/// How lit a fill is where the renderer draws no shaders: no fade, since
/// nothing draws the frames between, so the state as it is headed.
fn plain_amount(transition: &Animated) -> f32 {
    transition.track(0).to
}

/// A control as a screen reader meets it: its role and its name. The tree
/// is built only while assistive technology is attached, so this costs
/// nothing otherwise. See docs/design/koan-kit.md, "Accessibility".
pub(crate) fn accessible<E: StatefulInteractiveElement>(
    element: E,
    role: gpui::Role,
    name: Option<SharedString>,
) -> E {
    let element = element.role(role);
    match name {
        Some(name) => element.aria_label(name),
        None => element,
    }
}

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
            gpui::Role::Alert
        } else {
            gpui::Role::Status
        },
        Some(text.clone()),
    )
    .aria_value(text)
    .aria_live(if alarm {
        gpui::accesskit::Live::Assertive
    } else {
        gpui::accesskit::Live::Polite
    })
}

/// A checked state as AccessKit has it.
pub(crate) fn toggled(on: bool) -> gpui::Toggled {
    if on {
        gpui::Toggled::True
    } else {
        gpui::Toggled::False
    }
}

/// Answers a screen reader's click as a pointer's would, with `handler`.
pub(crate) fn answers_click<E: StatefulInteractiveElement>(
    element: E,
    handler: Handler<gpui::ClickEvent>,
) -> E {
    element.on_a11y_action(gpui::AccessibleAction::Click, move |_, window, cx| {
        handler(&gpui::ClickEvent::default(), window, cx)
    })
}

pub use popover::{Selectable, opens_panel};
pub use root::Root;
pub use scrollbar::{ScrollSource, Scrollbar};

/// The window root's and the panels' keys, bound once by `theme::init`.
pub(crate) fn bind_root(cx: &mut gpui::App) {
    root::bind(cx);
    popover::bind(cx);
}

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use core::prelude::v1::test;

    /// Without shaders nothing draws a fade's frames, so a fill shows the
    /// state it is headed to: lit as the pointer arrives, gone as it leaves.
    #[test]
    fn a_plain_fill_reads_where_its_state_is_headed() {
        let lit = Animated::new();
        lit.hold(0, 1.0);
        assert_eq!(plain_amount(&lit), 1.0, "lit at once");
        lit.to(0, 0.0, Motion::LEAVE, Ease::InOut);
        assert!(lit.at(0, Animated::now()) > 0.5, "the GPU's fade has begun");
        assert_eq!(plain_amount(&lit), 0.0, "gone at once without it");
    }

    /// A leave the eye can follow: most of the highlight is still there a
    /// frame after the pointer goes, and half of it midway (#2054).
    #[test]
    fn a_leave_is_seen_fading() {
        let lit = Animated::new();
        lit.hold(0, 1.0);
        lit.to(0, 0.0, Motion::LEAVE, Ease::InOut);
        let track = lit.track(0);
        let frame = 1.0 / 60.0;
        assert!(track.at(track.start + frame) > 0.9, "one frame in");
        assert!(track.at(track.start + 2.0 * frame) > 0.75, "two frames in");
        let midway = track.at(track.start + track.length / 2.0);
        assert!((midway - 0.5).abs() < 0.05, "midway: {midway}");
    }
}

/// What the components' tests share: a window holding a component, the
/// pointer and the keys reaching it, and the node a screen reader meets.
#[cfg(test)]
pub(crate) mod testing {
    use gpui::accesskit::{Action, ActionRequest, NodeId, TreeId};
    use gpui::prelude::*;
    use gpui::{
        AnyElement, Bounds, Context, Entity, IntoElement, KeyUpEvent, Keystroke, Modifiers,
        ParentElement, Pixels, Point, Render, Styled, TestAppContext, VisualTestContext, Window,
        div, px,
    };
    use serde_json::Value;

    type Build = Box<dyn Fn(&mut Window, &mut Context<Stage>) -> Vec<(&'static str, AnyElement)>>;

    /// A page drawing whatever its builder returns, each part wrapped at its
    /// own size so a test can find it by the selector it gives.
    pub struct Stage {
        build: Build,
    }

    impl Render for Stage {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().w(px(600.)).flex().flex_col().items_start().children(
                (self.build)(window, cx).into_iter().map(|(name, element)| {
                    div().debug_selector(move || name.into()).child(element)
                }),
            )
        }
    }

    /// Opens a window on the parts, with the theme installed and a screen
    /// reader attached, so the accessibility tree is built.
    pub fn stage(
        cx: &mut TestAppContext,
        build: impl Fn(&mut Window, &mut Context<Stage>) -> Vec<(&'static str, AnyElement)> + 'static,
    ) -> (Entity<Stage>, &mut VisualTestContext) {
        cx.update(|cx| {
            crate::theme::init(cx);
        });
        let (stage, cx) = cx.add_window_view(|_, _| Stage {
            build: Box::new(build),
        });
        cx.update(|window, _| window.set_a11y_active_for_test(true));
        cx.run_until_parked();
        (stage, cx)
    }

    /// Redraws and settles, after a test changed what the parts hold.
    pub fn redraw(cx: &mut VisualTestContext) {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    /// Where the part named `name` was drawn.
    pub fn bounds(cx: &mut VisualTestContext, name: &'static str) -> Bounds<Pixels> {
        cx.debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not drawn"))
    }

    /// `key` pressed and released on what has the keyboard, drawn first so
    /// the focused control has registered its keys: a control's Enter and
    /// Space click on the release.
    pub fn press(cx: &mut VisualTestContext, key: &'static str) {
        redraw(cx);
        cx.simulate_keystrokes(key);
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("a key"),
        });
        cx.run_until_parked();
    }

    /// A click at `at`.
    pub fn click_at(cx: &mut VisualTestContext, at: Point<Pixels>) {
        cx.simulate_click(at, Modifiers::default());
        cx.run_until_parked();
    }

    /// A click at the centre of the part named `name`.
    pub fn click(cx: &mut VisualTestContext, name: &'static str) {
        let at = bounds(cx, name).center();
        click_at(cx, at);
    }

    /// The accessibility tree as JSON.
    pub fn tree(cx: &mut VisualTestContext) -> Value {
        let json = cx
            .update(|window, _| window.debug_a11y_tree_json())
            .expect("a tree while a screen reader is attached");
        serde_json::from_str(&json).expect("JSON")
    }

    /// The node a screen reader knows by `label`: its id and its semantics.
    pub fn node(cx: &mut VisualTestContext, label: &str) -> (NodeId, Value) {
        let tree = tree(cx);
        let node = tree["nodes"]
            .as_object()
            .expect("nodes")
            .values()
            .find(|node| node["aria"]["label"] == label)
            .unwrap_or_else(|| panic!("no node named {label} in {tree:#}"))
            .clone();
        let id = node["accesskit_id"]
            .as_str()
            .expect("an id")
            .parse()
            .expect("a number");
        (NodeId(id), node["aria"].clone())
    }

    /// A screen reader asking the node named `label` to do `action`.
    pub fn act(cx: &mut VisualTestContext, label: &str, action: Action) {
        let (target, _) = node(cx, label);
        cx.update(|window, cx| {
            window.dispatch_a11y_action(
                ActionRequest {
                    action,
                    target_tree: TreeId::ROOT,
                    target_node: target,
                    data: None,
                },
                cx,
            )
        });
        cx.run_until_parked();
    }
}
