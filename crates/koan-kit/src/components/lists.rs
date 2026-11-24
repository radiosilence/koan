//! The virtualised lists a long page sits in, and their scrollbar: GPUI's
//! `uniform_list` (rows of one height, laid out a screenful at a time) and
//! `list` (rows that measure themselves), with the handles that keep their
//! place, and the kōan scrollbar over either. A page building rows of
//! `TableRow`s takes `table_list`; one laying out something else in rows,
//! such as a grid of tiles a row at a time, takes these.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Div, ElementId, EntityId, InteractiveElement, IntoElement, ParentElement, Pixels, Point,
    SharedString, Stateful, Styled, canvas, div, px,
};

pub use gpui::{
    List, ListAlignment, ListOffset, ListState, ScrollStrategy, UniformList,
    UniformListScrollHandle, list, uniform_list,
};

/// How far past the screen a `list` lays out rows, so a scroll lands on
/// rows already measured.
pub const OVERDRAW: Pixels = px(400.0);

/// The overlay scrollbar for a virtualised list's handle, over the list's
/// right edge.
pub fn list_scrollbar<H>(handle: &H) -> impl IntoElement
where
    H: super::ScrollSource,
{
    super::Scrollbar::vertical(handle)
}

/// What a virtualised list sits in for a screen reader: a list named for
/// what it holds, its rows (`TableRow::spoken`) its items.
pub fn spoken_list(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Stateful<Div> {
    super::accessible(div().id(id), gpui::Role::List, Some(name.into()))
}

/// Scrolls the row past row `ix` of a `uniform_list`'s `count` on screen, the
/// one before it where `ix` is at the top: a screen reader moving through the
/// list reaches only rows drawn, so the next one is drawn before it gets
/// there. One neighbour only, the one the reader is heading for: a second
/// scroll could undo the first. The caller draws the list again
/// (`TableRow::on_reveal` does).
pub fn reveal_around(handle: &UniformListScrollHandle, ix: usize, count: usize) {
    if count == 0 {
        return;
    }
    let top = uniform_top(handle, count);
    handle.scroll_to_item(neighbour(ix, top, count), ScrollStrategy::Nearest);
}

/// The row at the top of a `uniform_list` of `count` rows as last laid out,
/// from its scroll offset and the height of its content. (The handle's
/// `logical_scroll_top` counts children a uniform list never registers, so
/// it is always 0.)
fn uniform_top(handle: &UniformListScrollHandle, count: usize) -> usize {
    let state = handle.0.borrow();
    let Some(size) = state.last_item_size else {
        return 0;
    };
    let row = f32::from(size.contents.height) / count as f32;
    if row <= 0.0 {
        return 0;
    }
    let scrolled = -f32::from(state.base_handle.offset().y);
    ((scrolled / row).floor().max(0.0) as usize).min(count - 1)
}

/// The neighbour of row `ix` a reader moving through a list of `count` is
/// heading for, with `top` the row at the top of the screen: the one before
/// where `ix` is at or above the top, the one after otherwise.
pub(crate) fn neighbour(ix: usize, top: usize, count: usize) -> usize {
    if ix <= top {
        ix.saturating_sub(1)
    } else {
        (ix + 1).min(count.saturating_sub(1))
    }
}

/// `reveal_around` for a `list` of rows that measure themselves.
pub fn reveal_around_list(state: &ListState, ix: usize) {
    let count = state.item_count();
    if count == 0 {
        return;
    }
    let top = state.logical_scroll_top().item_ix;
    state.scroll_to_reveal_item(neighbour(ix, top, count));
}

/// A scroll offset `y` down from the top, as a list's handle takes it.
pub fn offset_down(y: Pixels) -> Point<Pixels> {
    Point { x: px(0.0), y: -y }
}

/// An element filling its parent that keeps the parent's width in `width`,
/// and redraws `owner` when it changes: a page laying out a grid or a list
/// by the width it is given.
/// How a uniform list of rows `row` high leaves a phone's foot room at its
/// end: the blank rows it adds after its last, and how far it reaches below
/// its parent's bottom, which clips it, so the blank rows come to exactly
/// the foot's height. Room left as padding would not do: a uniform list
/// draws rows only inside its padding, and nothing would pass under the
/// foot. Zero where nothing is drawn over the page's foot.
pub fn rows_under_foot(row: Pixels, window: &gpui::Window, cx: &App) -> (usize, Pixels) {
    let foot = super::foot_inset(window, cx);
    if foot <= Pixels::ZERO || row <= Pixels::ZERO {
        return (0, Pixels::ZERO);
    }
    let rows = (foot / row).ceil() as usize;
    (rows, row * rows as f32 - foot)
}

/// A page's header over a virtual list that scrolls it away with the rows,
/// in the room the list leaves above its first row, which is `height` once
/// measured (`owner` is told). It stands `above` below the top of the
/// list's content, wherever the list's `scroll` has that: the room a bar
/// over the list takes.
pub fn scrolls_away(
    header: impl IntoElement,
    scroll: &UniformListScrollHandle,
    above: Pixels,
    height: Rc<Cell<Option<Pixels>>>,
    owner: EntityId,
) -> impl IntoElement {
    // Within the list's range, as the list lays its rows out: an offset
    // past either end stands at that end.
    let offset = {
        let state = scroll.0.borrow();
        let max = state.base_handle.max_offset().y.max(Pixels::ZERO);
        state.base_handle.offset().y.clamp(-max, Pixels::ZERO)
    };
    let offset = offset + above;
    let probe = canvas(
        move |bounds, _, cx| {
            if height.get() != Some(bounds.size.height) {
                height.set(Some(bounds.size.height));
                cx.notify(owner);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full();
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(offset)
        .child(div().relative().child(probe).child(header))
}

/// An element that reports the width it is laid out at into `width`, and
/// notifies `owner` when it changes: for a list that lays out its rows by
/// its own width.
pub fn width_probe(width: Rc<Cell<Option<Pixels>>>, owner: EntityId) -> impl IntoElement {
    canvas(
        move |bounds, _, cx| {
            if width.get() != Some(bounds.size.width) {
                width.set(Some(bounds.size.width));
                cx.notify(owner);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// A library page as the app draws one: a virtual list whose header scrolls
/// away with its rows, with its scrollbar, in a cached view inside another,
/// the stage the `Elastic` wraps. Pushed past either end by a mouse wheel or
/// pulled by a trackpad, its rows stay drawn and its header stays on the
/// first row's top edge, on every frame.
#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    use core::prelude::v1::test;
    use gpui::{
        AnyView, AppContext as _, Context, Entity, IntoElement, ParentElement, Pixels, Render,
        ScrollDelta, ScrollEvent, ScrollSource, StyleRefinement, Styled, TestAppContext,
        TouchPhase, VisualTestContext, Window, div, point, px,
    };

    use super::{UniformListScrollHandle, scrolls_away, uniform_list};
    use crate::components::{Elastic, Refresh, SHADED, Scrollbar};

    const ROW: f32 = 40.0;
    const ROW_WIDTH: f32 = 300.0;
    const HEADER: f32 = 60.0;
    const HEADER_WIDTH: f32 = 200.0;

    struct List {
        rows: usize,
        scroll: UniformListScrollHandle,
        header_height: Rc<Cell<Option<Pixels>>>,
    }

    impl Render for List {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let lead = self.header_height.get().unwrap_or_default();
            div()
                .relative()
                .size_full()
                .overflow_hidden()
                .child(
                    uniform_list("rows", self.rows, |range, _, _| {
                        range
                            .map(|_| div().w(px(ROW_WIDTH)).h(px(ROW)).bg(gpui::red()))
                            .collect::<Vec<_>>()
                    })
                    .track_scroll(&self.scroll)
                    .pt(lead)
                    .size_full(),
                )
                .child(scrolls_away(
                    div().w(px(HEADER_WIDTH)).h(px(HEADER)).bg(gpui::blue()),
                    &self.scroll,
                    px(0.),
                    self.header_height.clone(),
                    cx.entity_id(),
                ))
                .child(Scrollbar::vertical(&self.scroll))
        }
    }

    struct Stage {
        list: Entity<List>,
    }

    impl Render for Stage {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                self.list
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
        }
    }

    struct Shell {
        elastic: Elastic,
        stage: AnyView,
        busy: bool,
        asked: Rc<Cell<u32>>,
    }

    impl Render for Shell {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let asked = self.asked.clone();
            let refresh = Refresh {
                top: px(0.),
                busy: self.busy,
                color: gpui::black(),
                start: Rc::new(move |_, _| asked.set(asked.get() + 1)),
            };
            self.elastic.refresh(0, Some(refresh), window, cx);
            let page = self.stage.clone();
            div().size_full().child(self.elastic.wrap(page, window, cx))
        }
    }

    /// The page with `rows` rows. Under Reduce Motion, so a release lands
    /// at once and every wait is on the test's clock: what is laid out is
    /// the same, the motion between being the GPU's.
    fn opened(rows: usize, cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
        cx.update(|cx| {
            crate::theme::init(cx);
            cx.set_reduce_motion(true);
        });
        let (shell, cx) = cx.add_window_view(|_, cx| {
            let list = cx.new(|_| List {
                rows,
                scroll: UniformListScrollHandle::new(),
                header_height: Rc::default(),
            });
            let stage = cx.new(|_| Stage { list });
            Shell {
                elastic: Elastic::new(),
                stage: stage.into(),
                busy: false,
                asked: Rc::default(),
            }
        });
        cx.run_until_parked();
        (shell, cx)
    }

    /// The header's and the rows' top edges in the frame drawn last, in
    /// device pixels, those drawn into a layer group included.
    fn painted(cx: &mut VisualTestContext) -> (Vec<f32>, Vec<f32>) {
        cx.update(|window, _| {
            let scale = window.scale_factor();
            let scene = window.rendered_scene();
            let (mut header, mut rows) = (Vec::new(), Vec::new());
            for quads in std::iter::once(&scene.quads)
                .chain(scene.layer_groups.iter().map(|group| &group.quads))
            {
                for quad in quads {
                    let width = quad.bounds.size.width.0 / scale;
                    let top = quad.bounds.origin.y.0;
                    if (width - HEADER_WIDTH).abs() < 0.5 {
                        header.push(top);
                    } else if (width - ROW_WIDTH).abs() < 0.5 {
                        rows.push(top);
                    }
                }
            }
            rows.sort_by(f32::total_cmp);
            (header, rows)
        })
    }

    /// The rows are drawn, and the header sits on the first one.
    fn in_step(when: &str, cx: &mut VisualTestContext) {
        let scale = cx.update(|window, _| window.scale_factor());
        let (header, rows) = painted(cx);
        assert!(!rows.is_empty(), "{when}: no rows drawn");
        assert_eq!(header.len(), 1, "{when}: header drawn {header:?}");
        let gap = rows[0] - (header[0] + HEADER * scale);
        assert!(
            gap.abs() < 0.5,
            "{when}: header at {} over a first row at {}",
            header[0],
            rows[0]
        );
    }

    /// The display's frames on the test's clock, until nothing asks for
    /// another, each checked.
    fn frames_until_still(when: &str, cx: &mut VisualTestContext) {
        const FRAME: Duration = Duration::from_millis(8);
        for _ in 0..1000 {
            let ran = cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.executor().advance_clock(FRAME);
            cx.run_until_parked();
            in_step(when, cx);
            if ran == 0 {
                return;
            }
        }
        panic!("frames were still being asked for");
    }

    fn scroll(delta: ScrollDelta, phase: TouchPhase, cx: &mut VisualTestContext) {
        cx.simulate_event(ScrollEvent {
            position: point(px(100.), px(300.)),
            delta,
            touch_phase: phase,
            source: ScrollSource::Pointer,
            ..Default::default()
        });
        cx.run_until_parked();
    }

    fn wheel(lines: f32, steps: usize, cx: &mut VisualTestContext) {
        for step in 0..steps {
            scroll(ScrollDelta::Lines(point(0., lines)), TouchPhase::Moved, cx);
            in_step(&format!("wheel {lines} step {step}"), cx);
        }
        frames_until_still("after the wheel", cx);
    }

    #[gpui::test]
    fn a_wheel_past_the_top_keeps_the_header_on_the_rows(cx: &mut TestAppContext) {
        let (_, cx) = opened(200, cx);
        in_step("at rest", cx);
        wheel(3., 6, cx);
    }

    /// A list shorter than its page has nowhere to scroll: a wheel down
    /// leaves the header where it was.
    #[gpui::test]
    fn a_wheel_down_a_short_list_keeps_the_header_on_the_rows(cx: &mut TestAppContext) {
        let (_, cx) = opened(3, cx);
        in_step("at rest", cx);
        wheel(-3., 6, cx);
        let (header, _) = painted(cx);
        assert_eq!(header, vec![0.0], "the header stays at the top");
    }

    #[gpui::test]
    fn a_trackpad_pull_keeps_the_rows_and_the_header_together(cx: &mut TestAppContext) {
        let (shell, cx) = opened(200, cx);
        let pixels = |dy: f32| ScrollDelta::Pixels(point(px(0.), px(dy)));
        scroll(pixels(0.), TouchPhase::Started, cx);
        for step in 0..20 {
            scroll(pixels(20.), TouchPhase::Moved, cx);
            in_step(&format!("pull step {step}"), cx);
            // Drawn into the band's layer, as the app's pages are.
            let grouped = cx.update(|window, _| !window.rendered_scene().layer_groups.is_empty());
            assert!(grouped || !SHADED, "pull step {step}: no layer");
        }
        scroll(pixels(0.), TouchPhase::Ended, cx);
        // The momentum after the fingers lift, still past the top.
        for step in 0..10 {
            scroll(pixels(8.), TouchPhase::Moved, cx);
            in_step(&format!("momentum step {step}"), cx);
        }
        assert_eq!(shell.read_with(cx, |shell, _| shell.asked.get()), 1);
        shell.update(cx, |shell, cx| {
            shell.busy = true;
            cx.notify();
        });
        frames_until_still("held open", cx);
        shell.update(cx, |shell, cx| {
            shell.busy = false;
            cx.notify();
        });
        frames_until_still("refreshed", cx);
    }
}
