//! A field's text as drawn: its lines shaped, scrolled to keep the caret in
//! view, the selection behind them, the input method's composition
//! underlined, the placeholder while empty, and the caret, a shader quad
//! whose blink presents the frame again and draws nothing.

use crate::components::ShaderProgram;
use std::ops::Range;

use gpui::{
    App, Bounds, DispatchPhase, ElementId, ElementInputHandler, Entity, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, MouseDownEvent, MouseMoveEvent, Pixels, Point,
    SharedString, Style, TextAlign, TextRun, UnderlineStyle, Window, fill, point, px, size,
};

use super::state::{FieldState, Line};

/// The caret, in its colour (uniforms 4..8) while uniform 0 says it shows.
pub(super) static CARET: ShaderProgram = ShaderProgram {
    name: "koan-caret",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    return float4(u[4], u[5], u[6], u[7] * u[0]);
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(u[1].xyz, u[1].w * u[0].x);
}
"#,
    prepare: None,
};

/// What a field is drawn in, from the look: its text, its placeholder, the
/// caret and its width (the hairline, thicker with more contrast), the
/// selection, a newline's sliver at the end of a selected line, and the
/// spans its owner colours (byte ranges of the text).
#[derive(Clone)]
pub(super) struct Colours {
    pub ink: Hsla,
    pub placeholder: Hsla,
    pub caret: Hsla,
    pub caret_width: Pixels,
    pub selection: Hsla,
    pub newline: Pixels,
    pub spans: Vec<(Range<usize>, Hsla)>,
}

pub(super) struct FieldElement {
    pub state: Entity<FieldState>,
    pub editable: bool,
    pub colours: Colours,
}

pub(super) struct Prepaint {
    lines: Vec<Line>,
    line_height: Pixels,
    scroll: Point<Pixels>,
    selection: Vec<Bounds<Pixels>>,
    caret: Option<Bounds<Pixels>>,
}

impl IntoElement for FieldElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl gpui::Element for FieldElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let rows = self.state.read(cx).rows;
        // The row's width, from nothing: the text scrolls, so a field never
        // holds a row open.
        let mut style = Style {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: px(0.0).into(),
            ..Style::default()
        };
        style.min_size.width = px(0.0).into();
        style.size.height = (window.line_height() * rows as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let state = self.state.read(cx);
        let caret_uniforms = state.caret.clone();
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let empty = state.text.is_empty() && state.marked.is_none();
        let drawn: String = if empty {
            state.placeholder.to_string()
        } else {
            state.drawn()
        };
        let colour = if empty {
            self.colours.placeholder
        } else {
            self.colours.ink
        };
        let marked = state
            .marked
            .as_ref()
            .map(|m| state.to_drawn(m.start)..state.to_drawn(m.end));
        // The owner's colours, where the text is drawn as itself.
        let spans: &[(Range<usize>, Hsla)] = if empty || state.masked {
            &[]
        } else {
            &self.colours.spans
        };

        // Each line shaped on its own, the composition underlined where it
        // falls.
        let mut lines = Vec::new();
        let mut start = 0;
        for text in drawn.split('\n') {
            let range = start..start + text.len();
            let runs = runs(
                &range,
                colour,
                spans,
                &style,
                marked.as_ref(),
                self.colours.caret_width,
            );
            let shaped = window.text_system().shape_line(
                SharedString::from(text.to_string()),
                font_size,
                &runs,
                None,
            );
            lines.push(Line { start, shaped });
            start = range.end + 1;
        }

        // Where the caret is, and the scroll that keeps it in view: across
        // for a single line, down for several.
        let x_of = |offset: usize| -> (usize, Pixels) {
            let row = lines.iter().rposition(|l| l.start <= offset).unwrap_or(0);
            let line = &lines[row];
            (row, line.shaped.x_for_index(offset - line.start))
        };
        let cursor = if empty {
            0
        } else {
            state.to_drawn(state.cursor())
        };
        let (row, caret_x) = x_of(cursor);
        let mut scroll = state.scroll;
        let width = bounds.size.width;
        let widest = lines
            .iter()
            .map(|l| l.shaped.width())
            .fold(px(0.0), Pixels::max);
        if caret_x - scroll.x > width - self.colours.caret_width {
            scroll.x = caret_x - width + self.colours.caret_width;
        }
        if caret_x < scroll.x {
            scroll.x = caret_x;
        }
        scroll.x = scroll.x.clamp(
            px(0.0),
            (widest + self.colours.caret_width - width).max(px(0.0)),
        );
        let height = bounds.size.height;
        let caret_top = line_height * row as f32;
        if caret_top + line_height - scroll.y > height {
            scroll.y = caret_top + line_height - height;
        }
        if caret_top < scroll.y {
            scroll.y = caret_top;
        }
        let content = line_height * lines.len() as f32;
        scroll.y = scroll.y.clamp(px(0.0), (content - height).max(px(0.0)));

        let origin = bounds.origin - scroll;
        let selection = if empty || state.selected.is_empty() {
            Vec::new()
        } else {
            let (from, to) = (
                state.to_drawn(state.selected.start),
                state.to_drawn(state.selected.end),
            );
            lines
                .iter()
                .enumerate()
                .filter_map(|(i, line)| {
                    let end = line.start + line.shaped.text.len();
                    let (a, b) = (from.max(line.start), to.min(end));
                    // A line wholly inside a selection that runs on past it
                    // shows its newline as a sliver.
                    let past = to > end;
                    (a < b || (past && from <= end)).then(|| {
                        let left = line.shaped.x_for_index(a - line.start);
                        let right = line.shaped.x_for_index(b.max(a) - line.start)
                            + if past { self.colours.newline } else { px(0.0) };
                        Bounds::from_corners(
                            origin + point(left, line_height * i as f32),
                            origin + point(right, line_height * (i + 1) as f32),
                        )
                    })
                })
                .collect()
        };
        let caret = (self.editable && state.selected.is_empty()).then(|| {
            Bounds::new(
                origin + point(caret_x, caret_top),
                size(self.colours.caret_width, line_height),
            )
        });
        caret_uniforms.attach(false, window, cx);
        Prepaint {
            lines,
            line_height,
            scroll,
            selection,
            caret,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus, caret) = {
            let state = self.state.read(cx);
            (state.focus.clone(), state.caret.clone())
        };
        // The field is drawn as part of a view, which a cached view's frame
        // replays until the view itself is told it changed: the state tells
        // the view it was last drawn in (`FieldState::drawn_in`).
        let view = window.current_view();
        if self.state.read(cx).drawn_in != Some(view) {
            self.state
                .update(cx, |state, _| state.drawn_in = Some(view));
        }
        if self.editable {
            window.handle_input(
                &focus,
                ElementInputHandler::new(bounds, self.state.clone()),
                cx,
            );
        }
        // A drag from the field follows the pointer past its edges and the
        // window's, extending the selection and scrolling to it: heard
        // window-wide while it lasts, as the field's own hitbox ends at its
        // edge.
        if self.state.read(cx).drag.is_some() {
            let state = self.state.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    state.update(cx, |state, cx| state.mouse_move(event, window, cx));
                }
            });
        }
        if focus.is_focused(window) {
            super::drawn_focused(&focus, bounds);
        }
        // Under touch, a tap anywhere but the field puts the keyboard away:
        // the tap goes where it landed first (a button presses, another
        // field takes the keyboard), and the field lets go only if it still
        // has it.
        if focus.is_focused(window) && window.last_input_was_touch() {
            let focus = focus.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || bounds.contains(&event.position) {
                    return;
                }
                let focus = focus.clone();
                window.defer(cx, move |window, cx| {
                    if focus.is_focused(window) {
                        window.blur(cx);
                        window.dismiss_virtual_keyboard();
                    }
                });
            });
        }
        let origin = bounds.origin - prepaint.scroll;
        let line_height = prepaint.line_height;
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for selection in &prepaint.selection {
                window.paint_quad(fill(*selection, self.colours.selection));
            }
            for (i, line) in prepaint.lines.iter().enumerate() {
                line.shaped
                    .paint(
                        origin + point(px(0.0), line_height * i as f32),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .ok();
            }
            if focus.is_focused(window)
                && let Some(at) = prepaint.caret
            {
                caret.set_color(4, self.colours.caret);
                caret.paint(&CARET, None, at, window);
            }
        });
        let lines = std::mem::take(&mut prepaint.lines);
        let scroll = prepaint.scroll;
        self.state.update(cx, |state, _| {
            state.lines = lines;
            state.bounds = Some(bounds);
            state.line_height = line_height;
            state.scroll = scroll;
        });
    }
}

/// A line's runs: in `colour`, or a span's colour where the owner gave one,
/// with the part of `marked` (an input method's composition, in drawn
/// offsets) that falls in the line underlined.
fn runs(
    line: &Range<usize>,
    colour: Hsla,
    spans: &[(Range<usize>, Hsla)],
    style: &gpui::TextStyle,
    marked: Option<&Range<usize>>,
    underline: Pixels,
) -> Vec<TextRun> {
    // Where anything changes within the line, in the line's offsets.
    let mut cuts = vec![0, line.len()];
    let clip = |at: usize| at.clamp(line.start, line.end) - line.start;
    for (range, _) in spans {
        cuts.extend([clip(range.start), clip(range.end)]);
    }
    if let Some(marked) = marked {
        cuts.extend([clip(marked.start), clip(marked.end)]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .filter(|w| w[1] > w[0])
        .map(|w| {
            let at = line.start + w[0];
            let colour = spans
                .iter()
                .rev()
                .find(|(range, _)| range.start <= at && at < range.end)
                .map_or(colour, |(_, c)| *c);
            let composing = marked.is_some_and(|m| m.start <= at && at < m.end);
            TextRun {
                len: w[1] - w[0],
                font: style.font(),
                color: colour,
                background_color: None,
                underline: composing.then_some(UnderlineStyle {
                    color: Some(colour),
                    thickness: underline,
                    wavy: false,
                }),
                strikethrough: None,
            }
        })
        .collect()
}
