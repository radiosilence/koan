//! The kōan scrollbar, which every scrolling surface lays over its right
//! edge: thin and square on no track, `muted` at rest and `ink` under the
//! pointer or held, taking no width from what scrolls. It shows at once
//! while the content scrolls or the pointer is over its edge, and after a
//! rest fades out quickly (`Motion::LEAVE`), on the GPU: a shader quad whose
//! opacity is a track, so the fade draws no frame. Drag the thumb to scroll,
//! or press the edge to bring the thumb there.
//!
//! A screen reader scrolls the content itself, which GPUI reports as a
//! scroll area; the bar is how the content's place is drawn, and is left
//! out of the accessibility tree.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::*;

use super::{Animated, Ease, Repeat, SHADED, ShaderProgram};
use crate::theme::Look;
use crate::theme::tokens::Motion;

/// What a scrollbar reads and moves: a scrolling div's `ScrollHandle`, a
/// `uniform_list`'s handle or a `list`'s state. Offsets are GPUI's, zero at
/// the top and negative down the content.
pub trait ScrollSource: Clone + 'static {
    fn offset(&self) -> Point<Pixels>;
    fn set_offset(&self, offset: Point<Pixels>);
    /// How far the content scrolls: zero when it fits.
    fn max_offset(&self) -> Pixels;
    fn start_drag(&self) {}
    fn end_drag(&self) {}
}

impl ScrollSource for ScrollHandle {
    fn offset(&self) -> Point<Pixels> {
        ScrollHandle::offset(self)
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        ScrollHandle::set_offset(self, offset)
    }

    fn max_offset(&self) -> Pixels {
        ScrollHandle::max_offset(self).y
    }
}

impl ScrollSource for UniformListScrollHandle {
    fn offset(&self) -> Point<Pixels> {
        self.0.borrow().base_handle.offset()
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.0.borrow_mut().base_handle.set_offset(offset)
    }

    fn max_offset(&self) -> Pixels {
        self.0.borrow().base_handle.max_offset().y
    }
}

impl ScrollSource for ListState {
    fn offset(&self) -> Point<Pixels> {
        self.scroll_px_offset_for_scrollbar()
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.set_offset_from_scrollbar(offset)
    }

    fn max_offset(&self) -> Pixels {
        self.max_offset_for_scrollbar().y
    }

    fn start_drag(&self) {
        self.scrollbar_drag_started()
    }

    fn end_drag(&self) {
        self.scrollbar_drag_ended()
    }
}

/// The thumb in its colour (uniforms 4..8), at the opacity of track 0.
static THUMB: ShaderProgram = ShaderProgram {
    name: "koan-scrollbar",
    msl: r#"
float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) {
    return float4(u[4], u[5], u[6], u[7] * clamp(koan_track(u, 0, time), 0.0, 1.0));
}
"#,
    wgsl: r#"
fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(u[1].xyz, u[1].w * clamp(koan_track(u, 0u, time), 0.0, 1.0));
}
"#,
    prepare: None,
};

/// The thumb's width at rest, and under the pointer or held.
const REST: Pixels = px(3.0);
const WIDE: Pixels = px(5.0);
/// Between the thumb and the edges it runs along.
const INSET: Pixels = px(3.0);
/// The edge that answers the pointer: the wide thumb and its insets.
const EDGE: Pixels = px(11.0);
/// The edge a finger takes the thumb by while it shows, as iOS lets a
/// scroll indicator be held to scrub a long list.
const FINGER_EDGE: Pixels = px(44.0);
const MIN_LENGTH: Pixels = px(32.0);
/// How long the bar stays after the content stops or the pointer leaves.
const LINGER: Duration = Duration::from_millis(800);

/// The vertical scrollbar over its parent, which is `relative` and the
/// size of what scrolls.
pub struct Scrollbar<S: ScrollSource> {
    source: S,
    under_bar: bool,
    under_foot: bool,
}

impl<S: ScrollSource> Scrollbar<S> {
    pub fn vertical(source: &S) -> Self {
        Self {
            source: source.clone(),
            under_bar: false,
            under_foot: false,
        }
    }

    /// Over content that scrolls under a phone's bar and its foot: the
    /// track runs between them (`bar_inset`, `foot_inset`).
    pub fn under_bar(mut self) -> Self {
        self.under_bar = true;
        self.under_foot = true;
        self
    }

    /// Over content that scrolls under a phone's foot alone: the track ends
    /// above it (`foot_inset`).
    pub fn under_foot(mut self) -> Self {
        self.under_foot = true;
        self
    }
}

#[derive(Default)]
struct Bar {
    /// The offset the last frame drew, to tell a scroll.
    drawn: Option<Pixels>,
    hovered: bool,
    /// Held: where on the thumb it was taken, from its top.
    grab: Option<Pixels>,
    /// Showing where shaders do not draw the fade.
    shown: bool,
    fade: Animated,
    linger: Option<Task<()>>,
}

impl Bar {
    fn show(&mut self) {
        self.fade.hold(0, 1.0);
        self.shown = true;
        self.linger = None;
    }

    /// Fades out after the linger, unless the pointer is over it or holds it.
    fn rest(this: &Rc<RefCell<Self>>, view: EntityId, cx: &mut App) {
        let mut bar = this.borrow_mut();
        if bar.hovered || bar.grab.is_some() {
            return;
        }
        let weak = Rc::downgrade(this);
        bar.linger = Some(cx.spawn(async move |cx| {
            cx.background_executor().timer(LINGER).await;
            let Some(bar) = weak.upgrade() else { return };
            let mut bar = bar.borrow_mut();
            let now = Animated::now();
            let from = bar.fade.at(0, now);
            bar.fade
                .segment(0, from, 0.0, now, Motion::LEAVE, Ease::InOut, Repeat::Once);
            bar.shown = false;
            drop(bar);
            if !SHADED {
                cx.update(|cx| cx.notify(view));
            }
        }));
    }
}

pub struct Prepaint {
    hitbox: Option<Hitbox>,
    thumb: Option<Bounds<Pixels>>,
    track: Bounds<Pixels>,
    bar: Rc<RefCell<Bar>>,
}

impl<S: ScrollSource> IntoElement for Scrollbar<S> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<S: ScrollSource> Element for Scrollbar<S> {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        Some("koan-scrollbar".into())
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
        let mut style = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        style.inset = Edges::all(px(0.0).into());
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let bar = window.with_element_state(id.expect("an id"), |bar, _| {
            let bar = bar.unwrap_or_else(|| {
                let bar = Rc::new(RefCell::new(Bar::default()));
                bar.borrow().fade.hold(0, 0.0);
                bar
            });
            (bar.clone(), bar)
        });
        let touch = Look::of(window, cx).touch;
        let edge = if touch { FINGER_EDGE } else { EDGE };
        let below = if self.under_bar {
            super::bar_inset(window, cx)
        } else {
            px(0.0)
        };
        let above = if self.under_foot {
            super::foot_inset(window, cx)
        } else {
            px(0.0)
        };
        let track = Bounds::from_corners(
            point(bounds.right() - edge, bounds.top() + below + INSET),
            point(bounds.right(), bounds.bottom() - above - INSET),
        );
        let max = self.source.max_offset();
        let offset = -self.source.offset().y;
        let content = bounds.size.height + max;
        // Content passing under what is drawn over the page frosts it.
        if self.under_bar || self.under_foot {
            super::report_scroll_edges(
                self.under_bar && offset > px(0.5),
                self.under_foot && offset < max - px(0.5),
                window,
                cx,
            );
        }
        if max <= px(0.5) || bounds.size.height <= px(0.0) {
            bar.borrow_mut().drawn = Some(offset);
            return Prepaint {
                hitbox: None,
                thumb: None,
                track,
                bar,
            };
        }
        // A scroll since the last frame shows it, and it rests again once
        // the content stops.
        let view = window.current_view();
        let scrolled = {
            let mut b = bar.borrow_mut();
            let moved = b.drawn.is_some_and(|drawn| drawn != offset);
            b.drawn = Some(offset);
            moved
        };
        if scrolled {
            bar.borrow_mut().show();
            Bar::rest(&bar, view, cx);
        }
        let length = (track.size.height * (bounds.size.height / content))
            .max(MIN_LENGTH)
            .min(track.size.height);
        let room = track.size.height - length;
        let top = track.top() + room * (offset / max).clamp(0.0, 1.0);
        let wide = {
            let b = bar.borrow();
            b.hovered || b.grab.is_some()
        };
        let width = if wide { WIDE } else { REST };
        let thumb = Bounds::new(
            point(bounds.right() - INSET - width, top),
            size(width, length),
        );
        // Under touch the edge is held only while the thumb shows, so taps
        // on the rows beside it reach them at rest.
        let takes = !touch || {
            let b = bar.borrow();
            b.shown || b.grab.is_some()
        };
        Prepaint {
            // The content behind still takes the wheel and the trackpad.
            hitbox: takes
                .then(|| window.insert_hitbox(track, HitboxBehavior::BlockMouseExceptScroll)),
            thumb: Some(thumb),
            track,
            bar,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (Some(hitbox), Some(thumb)) = (prepaint.hitbox.clone(), prepaint.thumb) else {
            return;
        };
        let bar = prepaint.bar.clone();
        let track = prepaint.track;
        let view = window.current_view();
        let look = Look::of(window, cx);
        let colour = {
            let b = bar.borrow();
            if b.grab.is_some() {
                look.palette.ink.hsla()
            } else if b.hovered {
                look.palette.ink.hsla().opacity(0.7)
            } else {
                look.palette.muted.hsla().opacity(0.6)
            }
        };
        window.set_cursor_style(CursorStyle::Arrow, &hitbox);
        if SHADED {
            let fade = bar.borrow().fade.clone();
            fade.set_color(4, colour);
            fade.attach(true, window, cx);
            fade.paint(&THUMB, None, thumb, window);
        } else if bar.borrow().shown {
            window.paint_quad(fill(thumb, colour));
        }

        // The pointer over the edge shows it; leaving lets it rest.
        let over = bar.clone();
        let edge = hitbox.clone();
        window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let hovered = edge.is_hovered(window);
            if over.borrow().hovered == hovered {
                return;
            }
            over.borrow_mut().hovered = hovered;
            if hovered {
                over.borrow_mut().show();
            } else {
                Bar::rest(&over, view, cx);
            }
            cx.notify(view);
        });

        // A finger on the showing thumb's edge drags it rather than
        // scrolling the list, arriving as the pointer's press below.
        if look.touch {
            let held = hitbox.clone();
            window.on_mouse_event(move |event: &TouchDragEvent, phase, window, _| {
                if phase == DispatchPhase::Bubble
                    && event.phase == TouchPhase::Started
                    && !window.default_prevented()
                    && held.is_hovered(window)
                {
                    window.prevent_default();
                    window.carry_touch_drag_as_pointer();
                }
            });
        }

        // A press on the thumb takes it where it was pressed; on the edge
        // beside it, brings the thumb's middle there and takes it.
        let (press, source) = (bar.clone(), self.source.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            cx.stop_propagation();
            let grab = if thumb.contains(&event.position) {
                event.position.y - thumb.top()
            } else {
                thumb.size.height / 2.0
            };
            press.borrow_mut().grab = Some(grab);
            press.borrow_mut().show();
            source.start_drag();
            scroll_to(&source, event.position.y - grab, track, thumb.size.height);
            cx.notify(view);
        });

        // Held, the thumb follows the pointer anywhere in the window. A move
        // without the button, or the release, lets it go: a press and
        // release within one frame, which drew no listener for the move,
        // ends here as well.
        let (held, source) = (bar.clone(), self.source.clone());
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let Some(grab) = held.borrow().grab else {
                return;
            };
            if event.pressed_button != Some(MouseButton::Left) {
                release(&held, &source, view, cx);
                return;
            }
            scroll_to(&source, event.position.y - grab, track, thumb.size.height);
            cx.notify(view);
        });
        let (held, source) = (bar.clone(), self.source.clone());
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                release(&held, &source, view, cx);
            }
        });
    }
}

/// Lets go of the thumb, if held.
fn release<S: ScrollSource>(bar: &Rc<RefCell<Bar>>, source: &S, view: EntityId, cx: &mut App) {
    if bar.borrow_mut().grab.take().is_some() {
        source.end_drag();
        Bar::rest(bar, view, cx);
        cx.notify(view);
    }
}

/// The content scrolled so the thumb's top is at `top`.
fn scroll_to<S: ScrollSource>(source: &S, top: Pixels, track: Bounds<Pixels>, length: Pixels) {
    let room = track.size.height - length;
    if room <= px(0.0) {
        return;
    }
    let share = ((top - track.top()) / room).clamp(0.0, 1.0);
    let offset = source.offset();
    source.set_offset(point(offset.x, -(source.max_offset() * share)));
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    // `gpui::*` brings GPUI's `test` attribute, which would expand its own
    // `#[test]` into itself.
    use core::prelude::v1::test;

    struct Page {
        scroll: ScrollHandle,
        rows: usize,
        pressed: Rc<Cell<u32>>,
    }

    impl Render for Page {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let pressed = self.pressed.clone();
            div()
                .id("page")
                .relative()
                .size(px(200.0))
                .debug_selector(|| "page".into())
                .child(
                    div()
                        .id("content")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .on_mouse_down(MouseButton::Left, move |_, _, _| {
                            pressed.set(pressed.get() + 1)
                        })
                        .children((0..self.rows).map(|i| div().id(i).h(px(20.0)).w_full())),
                )
                .child(Scrollbar::vertical(&self.scroll))
        }
    }

    fn page(rows: usize, cx: &mut TestAppContext) -> (Entity<Page>, &mut VisualTestContext) {
        cx.update(crate::theme::init);
        let (page, cx) = cx.add_window_view(|_, _| Page {
            scroll: ScrollHandle::new(),
            rows,
            pressed: Rc::new(Cell::new(0)),
        });
        cx.run_until_parked();
        (page, cx)
    }

    fn offset(page: &Entity<Page>, cx: &mut VisualTestContext) -> Pixels {
        page.read_with(cx, |p, _| -p.scroll.offset().y)
    }

    /// A press on the edge brings the thumb there, a drag moves it, and
    /// neither reaches the content beneath; the content scrolls with it.
    #[gpui::test]
    fn the_thumb_is_pressed_and_dragged(cx: &mut TestAppContext) {
        let (page, cx) = page(50, cx);
        let bounds = cx.debug_bounds("page").expect("drawn");
        let edge = bounds.right() - px(4.0);
        let max = page.read_with(cx, |p, _| p.scroll.max_offset().y);
        assert!(max > px(0.0));
        cx.simulate_mouse_down(
            point(edge, bounds.bottom() - px(4.0)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        assert_eq!(offset(&page, cx), max, "pressed at the foot: the end");
        cx.simulate_mouse_move(
            point(edge, bounds.top() - px(40.0)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.run_until_parked();
        assert_eq!(
            offset(&page, cx),
            px(0.0),
            "dragged past the top: the start"
        );
        cx.simulate_mouse_up(
            point(edge, bounds.top()),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.run_until_parked();
        assert_eq!(page.read_with(cx, |p, _| p.pressed.get()), 0);
    }

    /// A press and release within one frame lets go: a move after it, with
    /// no button, does not scroll.
    #[gpui::test]
    fn a_tap_on_the_edge_lets_go(cx: &mut TestAppContext) {
        let (page, cx) = page(50, cx);
        let bounds = cx.debug_bounds("page").expect("drawn");
        let edge = bounds.right() - px(4.0);
        let at = point(edge, bounds.center().y);
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        let tapped = offset(&page, cx);
        cx.simulate_mouse_move(point(edge, bounds.bottom()), None, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            offset(&page, cx),
            tapped,
            "a move after the tap is the pointer's own"
        );
    }

    /// The wheel over the bar's edge scrolls the content behind it.
    #[gpui::test]
    fn the_wheel_over_the_edge_scrolls(cx: &mut TestAppContext) {
        let (page, cx) = page(50, cx);
        let bounds = cx.debug_bounds("page").expect("drawn");
        cx.simulate_event(ScrollEvent {
            position: point(bounds.right() - px(4.0), bounds.center().y),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-60.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        assert!(offset(&page, cx) > px(0.0), "the content scrolled");
    }

    /// Content that fits has no bar: its edge is the content's.
    #[gpui::test]
    fn content_that_fits_has_no_bar(cx: &mut TestAppContext) {
        let (page, cx) = page(3, cx);
        let bounds = cx.debug_bounds("page").expect("drawn");
        let at = point(bounds.right() - px(4.0), bounds.top() + px(10.0));
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        assert_eq!(page.read_with(cx, |p, _| p.pressed.get()), 1);
    }
}
