use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{
    App, Bounds, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, px,
};

use super::{Handler, child, focus, ring};
use crate::theme::{HAIRLINE, HIT, KoanStyled, Look, Motion, TypeRole, label};

/// A row of text options, 18 apart, with no track: `muted`, and the chosen
/// one `ink` and underlined in the accent. The underline moves to a new
/// choice over `Motion::NORMAL`.
#[derive(IntoElement)]
pub struct Segmented {
    id: ElementId,
    options: Vec<SharedString>,
    selected: usize,
    on_change: Option<Handler<usize>>,
}

impl Segmented {
    pub fn new<S: AsRef<str>>(
        id: impl Into<ElementId>,
        options: impl IntoIterator<Item = S>,
        selected: usize,
    ) -> Self {
        Self {
            id: id.into(),
            options: options.into_iter().map(label).collect(),
            selected,
            on_change: None,
        }
    }

    pub fn on_change(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Arc::new(handler));
        self
    }
}

/// Where each option's underline lies, from the last frame drawn, and the
/// move under way.
#[derive(Default)]
struct Marker {
    origin: Option<Bounds<Pixels>>,
    spans: Vec<Option<Bounds<Pixels>>>,
    shown: Option<usize>,
    moving: Option<(usize, Instant)>,
}

/// The underline's gap beneath the text and its weight.
const GAP: f32 = 5.0;

impl RenderOnce for Segmented {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let handle = focus(&self.id, window, cx);
        let marker =
            window.use_keyed_state(child(&self.id, "marker"), cx, |_, _| Marker::default());
        let now = Instant::now();
        let reduce = cx.reduce_motion();
        let count = self.options.len();
        let selected = self.selected;

        // A change of choice starts the marker from where it was drawn.
        let slide = marker.update(cx, |m, _| {
            m.spans.resize(count, None);
            if m.shown.is_some_and(|shown| shown != selected) && !reduce {
                m.moving = m.shown.map(|from| (from, now));
            }
            m.shown = Some(selected);
            let (from, started) = m.moving?;
            let t = now.duration_since(started).as_secs_f32() / Motion::NORMAL.as_secs_f32();
            if t >= 1.0 {
                m.moving = None;
                return None;
            }
            let (a, b, origin) = (
                m.spans.get(from)?.as_ref()?,
                m.spans.get(selected)?.as_ref()?,
                m.origin?,
            );
            let k = Motion::quick_out(t);
            let left = f32::from(a.left() - origin.left()) * (1.0 - k)
                + f32::from(b.left() - origin.left()) * k;
            let width = f32::from(a.size.width) * (1.0 - k) + f32::from(b.size.width) * k;
            Some((left, width))
        });
        if slide.is_some() {
            window.request_animation_frame();
        }

        let accent = look.accent.hsla();
        let origin_marker = marker.clone();
        let mut row = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .items_center()
            .gap(look.spaced(18.0))
            .min_h(HIT)
            .track_focus(&handle)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        origin_marker.update(cx, |m, _| m.origin = Some(bounds));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        for (index, option) in self.options.into_iter().enumerate() {
            let chosen = index == selected;
            let span_marker = marker.clone();
            let on_change = self.on_change.clone();
            let underline = chosen && slide.is_none();
            row = row.child(
                div()
                    .id(index)
                    .relative()
                    .pb(px(GAP))
                    .border_b(HAIRLINE)
                    .border_color(if underline {
                        accent
                    } else {
                        accent.opacity(0.0)
                    })
                    .text_role(TypeRole::Control, &look)
                    .text_color(if chosen {
                        look.palette.ink.hsla()
                    } else {
                        look.palette.muted.hsla()
                    })
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when_some_click(on_change, index)
                    .child(option)
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                span_marker.update(cx, |m, _| {
                                    if let Some(span) = m.spans.get_mut(index) {
                                        *span = Some(bounds);
                                    }
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            );
        }
        if let Some((left, width)) = slide {
            row = row.child(
                div()
                    .absolute()
                    .left(px(left))
                    .w(px(width))
                    .h(HAIRLINE)
                    .bg(accent)
                    .with_bottom_of_spans(&marker, selected, cx),
            );
        }
        if let Some(on_change) = self.on_change {
            row = row.on_key_down(move |event, window, cx| {
                let next = match event.keystroke.key.as_str() {
                    "left" if selected > 0 => selected - 1,
                    "right" if selected + 1 < count => selected + 1,
                    _ => return,
                };
                cx.stop_propagation();
                on_change(&next, window, cx);
            });
        }
        row.children(ring(&handle, &look, window))
    }
}

trait Choice {
    fn when_some_click(self, on_change: Option<Handler<usize>>, index: usize) -> Self;
}

impl Choice for gpui_kit::Stateful<gpui_kit::Div> {
    fn when_some_click(self, on_change: Option<Handler<usize>>, index: usize) -> Self {
        match on_change {
            Some(on_change) => self.on_click(move |_, window, cx| on_change(&index, window, cx)),
            None => self,
        }
    }
}

trait SpanBottom {
    fn with_bottom_of_spans(
        self,
        marker: &gpui_kit::Entity<Marker>,
        selected: usize,
        cx: &App,
    ) -> Self;
}

impl SpanBottom for gpui_kit::Div {
    /// The moving underline sits where the options' own underlines do: on
    /// the bottom edge of the option's box, measured from the row's top.
    fn with_bottom_of_spans(
        self,
        marker: &gpui_kit::Entity<Marker>,
        selected: usize,
        cx: &App,
    ) -> Self {
        let m = marker.read(cx);
        match (m.origin, m.spans.get(selected).copied().flatten()) {
            (Some(origin), Some(span)) => self.top(span.bottom() - origin.top() - HAIRLINE),
            _ => self,
        }
    }
}
