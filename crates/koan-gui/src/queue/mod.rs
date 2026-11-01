//! The queue: the main stage, grouped by record.
//!
//! The view outlives page switches (the shell holds it for the life of the
//! window), so its selection and scroll position survive a visit elsewhere.
//! Rows are laid out again only when the queue is sent whole; a patch (a
//! track change, a download landing) redraws the rows on screen, which read
//! their item as it is now. A patch never changes a row's height or place, so
//! the list keeps its measurements across one.

pub mod rows;

use std::collections::{HashMap, HashSet};

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{CreditPart, EntryStatus, QueueItem, QueueLock};

use crate::components::{Button, ButtonKind, KoanContextMenu, Menu, MenuEntry};
use crate::covers::{self, CoversHandle};
use crate::engine::{command, engine};
use crate::library::drag::{Playable, PlayableDrag};
use crate::library::feed::Feed;
use crate::links::{self, names};
use crate::mirror::{Mirror, Queue};
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::platform::actions::{
    ClearSelection, Delete, EndOfQueue, Redo, SelectAll, TopOfQueue, Undo,
};
use crate::platform::drop;
use crate::theme::{self, Icon, KoanIcon, KoanStyled, Space, TypeRole};
use crate::window_state;
use rows::Row;

// The queue's own keys. Delete, select all, undo and redo are the
// platform's Edit actions, shared with the menu bar.
actions!(
    queue,
    [
        PlaySelected,
        SelectPrevious,
        SelectNext,
        ExtendPrevious,
        ExtendNext,
    ]
);

pub const CONTEXT: &str = "Queue";

pub fn bindings() -> Vec<KeyBinding> {
    let ctx = Some(CONTEXT);
    vec![
        KeyBinding::new("enter", PlaySelected, ctx),
        KeyBinding::new("up", SelectPrevious, ctx),
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("shift-up", ExtendPrevious, ctx),
        KeyBinding::new("shift-down", ExtendNext, ctx),
    ]
}

/// Row heights, as the Mac's `RowMetrics` gives them: a track is a line of
/// text and its padding, a record's heading carries its sleeve and a second
/// line, a track standing alone carries its own sleeve.
const PADDING: Pixels = px(4.);
const TRACK: Pixels = px(34. + 8.);
const HEADING_SLEEVE: Pixels = px(52.);
const HEADING: Pixels = px(52. + 12. + 8.);
const SINGLE_SLEEVE: Pixels = px(40.);
/// The sleeve of the record the queue follows, beside the header's title.
const HEADER_SLEEVE: Pixels = px(34.);
const SINGLE: Pixels = px(40. + 8. + 8.);

/// A row, as the selection and the arrows name it: by queue item rather than
/// by position, so it survives the queue being sent whole. A heading is
/// named by its run's first item.
#[derive(Clone, PartialEq, Eq, Debug)]
struct RowRef {
    item: String,
    heading: bool,
}

/// Rows dragged within the queue. Named by the layout they were dragged from:
/// a drop after the queue was sent whole again is ignored rather than applied
/// to rows that have since moved.
#[derive(Clone)]
struct DraggedRows {
    row: usize,
    whole: u64,
}

struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        div()
            .px(p.space(Space::M))
            .py(p.space(Space::Xs))
            .bg(p.surface)
            .border_1()
            .border_color(p.rule)
            .text_role(TypeRole::Meta, &p.look)
            .text_color(p.ink)
            .child(self.0.clone())
    }
}

pub struct QueueView {
    queue: Entity<Queue>,
    focus: FocusHandle,
    list: ListState,
    /// The `Queue::whole` the list was last reset for.
    whole: u64,
    /// Picked queue items. A heading reads as picked when its whole run is.
    selected: HashSet<String>,
    /// Where a shift-click or shift-arrow extends from, and where the arrows
    /// move from.
    anchor: Option<RowRef>,
    cursor: Option<RowRef>,
    /// The row at the top of the list and its index then, kept as the list
    /// scrolls and as the view draws, so a queue sent whole can be shown from
    /// the same row.
    top: Option<(RowRef, usize)>,
    /// The row a drag in the queue would drop before, for the one insertion
    /// line drawn while it lasts.
    drop_at: Option<usize>,
    /// The row a secondary click landed on, for the list's one context menu.
    menu_row: Option<usize>,
    /// The navigator's rewinds of the queue already answered.
    rewinds: u32,
    /// Where the rows' links lead.
    nav: Entity<Navigator>,
    /// The favourites, once the library has loaded.
    feed: Option<(Entity<Feed>, Subscription)>,
    /// The running transfers' progress, read once a frame and only by a frame
    /// that draws a transferring row.
    readings: Option<HashMap<i64, Option<f64>>>,
    _subscriptions: [Subscription; 5],
}

impl QueueView {
    pub fn new(nav: &Entity<Navigator>, cx: &mut Context<Self>) -> Self {
        let queue = cx.global::<Mirror>().queue.clone();
        let covers = cx.global::<CoversHandle>().0.clone();
        let lock = cx.global::<Mirror>().lock.clone();
        let subscriptions = [
            // The header names what the queue follows.
            cx.observe(&lock, |_, _, cx| cx.notify()),
            cx.observe(&queue, |this, queue, cx| {
                if queue.read(cx).whole != this.whole {
                    this.lay_out(cx);
                }
                cx.notify();
            }),
            cx.observe(&covers, |_, _, cx| cx.notify()),
            // The playing row is in the accent, and this view is cached.
            theme::observe_accent(cx),
            // The queue's sidebar row chosen while the queue shows: its top.
            cx.observe(nav, |this, nav, cx| {
                let rewinds = nav.read(cx).rewinds(Route::Queue);
                if rewinds != this.rewinds {
                    this.rewinds = rewinds;
                    this.list.scroll_to(ListOffset::default());
                    // Held now, not at the next paint: a queue sent whole in
                    // between must keep the top, not the row left behind.
                    this.top = None;
                    cx.notify();
                }
            }),
        ];
        // The list calls this as it scrolls, with its own state held, so the
        // handler reads the queue and never the list.
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        let weak = cx.weak_entity();
        list.set_scroll_handler(move |event: &ListScrollEvent, _, cx| {
            weak.update(cx, |this, cx| this.keep_top(event.visible_range.start, cx))
                .ok();
        });
        let mut view = Self {
            queue,
            focus: cx.focus_handle(),
            list,
            whole: 0,
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            top: None,
            rewinds: nav.read(cx).rewinds(Route::Queue),
            drop_at: None,
            menu_row: None,
            nav: nav.clone(),
            feed: None,
            readings: None,
            _subscriptions: subscriptions,
        };
        // The queue can arrive before the window's saved layout is read.
        let grouped = window_state::GROUPED.load(std::sync::atomic::Ordering::Relaxed);
        if view.queue.read(cx).grouped != grouped {
            view.queue.update(cx, |queue, _| queue.regroup(grouped));
        }
        view.lay_out(cx);
        view
    }

    /// "Group by album": the rows under record headings, or flat.
    fn set_grouped(&mut self, grouped: bool, window: &mut Window, cx: &mut Context<Self>) {
        window_state::GROUPED.store(grouped, std::sync::atomic::Ordering::Relaxed);
        window_state::save(window.window_bounds(), cx);
        self.queue.update(cx, |queue, cx| {
            queue.regroup(grouped);
            cx.notify();
        });
    }

    /// The rows' hearts read and toggle the library's favourites.
    pub fn set_feed(&mut self, feed: Entity<Feed>, cx: &mut Context<Self>) {
        let observed = cx.observe(&feed, |_, _, cx| cx.notify());
        self.feed = Some((feed, observed));
        cx.notify();
    }

    /// A transfer's progress, from this frame's reading of every transfer.
    fn progress(&mut self, track: i64, cx: &App) -> Option<f64> {
        self.readings
            .get_or_insert_with(|| {
                engine(cx)
                    .transfer_readings()
                    .into_iter()
                    .map(|figure| (figure.track_id, figure.progress))
                    .collect()
            })
            .get(&track)
            .copied()
            .flatten()
    }

    /// The queue was sent whole: its rows were laid out off the main thread
    /// (`Queue::prepare`); what is left here is keeping the pick and the list.
    fn lay_out(&mut self, cx: &mut Context<Self>) {
        let queue = self.queue.read(cx);
        self.whole = queue.whole;
        self.selected.retain(|id| queue.at.contains_key(id));
        let top = self.list.logical_scroll_top();
        // A hint for the rows not yet measured, so the scrollbar is sized for
        // the whole queue from the first frame.
        self.list.reset_with_uniform_height(queue.rows.len(), TRACK);
        let kept = self.top.take();
        let landing = match kept.and_then(|(r, at)| Some((self.row_of(&r, cx)?, at))) {
            Some((row, at)) => ListOffset {
                item_ix: row,
                // The offset into the row belongs to it only if it is still
                // the row the list was at.
                offset_in_item: if at == top.item_ix {
                    top.offset_in_item
                } else {
                    px(0.)
                },
            },
            // The row is gone: stay at the same place in the list.
            None => ListOffset {
                item_ix: top.item_ix,
                offset_in_item: px(0.),
            },
        };
        self.list.scroll_to(landing);
        if self
            .anchor
            .as_ref()
            .is_some_and(|r| self.row_of(r, cx).is_none())
        {
            self.anchor = None;
        }
        if self
            .cursor
            .as_ref()
            .is_some_and(|r| self.row_of(r, cx).is_none())
        {
            self.cursor = None;
        }
    }

    fn keep_top(&mut self, row: usize, cx: &App) {
        if row < self.rows(cx).len() {
            #[cfg(feature = "profiler")]
            crate::measure::TOP.store(row, std::sync::atomic::Ordering::Relaxed);
            self.top = Some((self.row_ref(row, cx), row));
        }
    }

    fn rows<'a>(&self, cx: &'a App) -> &'a [Row] {
        &self.queue.read(cx).rows
    }

    fn row_of(&self, r: &RowRef, cx: &App) -> Option<usize> {
        let queue = self.queue.read(cx);
        let track = *queue.track_rows.get(&r.item)?;
        if !r.heading {
            return Some(track);
        }
        let above = track.checked_sub(1)?;
        matches!(queue.rows[above], Row::Heading { first, .. } if queue.rows[track] == (Row::Track { ix: first, grouped: true }))
            .then_some(above)
    }

    fn row_ref(&self, row: usize, cx: &App) -> RowRef {
        let queue = self.queue.read(cx);
        let base = &queue.base;
        match queue.rows[row] {
            Row::Heading { first, .. } => RowRef {
                item: base[first].queue_item_id.clone(),
                heading: true,
            },
            Row::Track { ix, .. } => RowRef {
                item: base[ix].queue_item_id.clone(),
                heading: false,
            },
        }
    }

    fn row_items<'a>(&self, row: usize, cx: &'a App) -> impl Iterator<Item = &'a String> {
        let queue = self.queue.read(cx);
        queue.rows[row]
            .items()
            .map(|ix| &queue.base[ix].queue_item_id)
    }

    fn is_selected(&self, row: usize, cx: &App) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        self.row_items(row, cx).all(|id| self.selected.contains(id))
    }

    /// The picked queue items, in queue order. Ordered by their indexes
    /// rather than by walking the queue, so it costs the pick's size.
    fn selected_ids(&self, cx: &App) -> Vec<String> {
        let at = &self.queue.read(cx).at;
        let mut picked: Vec<(usize, &String)> = self
            .selected
            .iter()
            .filter_map(|id| Some((*at.get(id)?, id)))
            .collect();
        picked.sort_unstable_by_key(|(ix, _)| *ix);
        picked.into_iter().map(|(_, id)| id.clone()).collect()
    }

    /// The queue items a row action applies to: the selection when the row
    /// is in it, the row alone otherwise.
    fn acting_on(&self, row: usize, cx: &App) -> Vec<String> {
        if self.is_selected(row, cx) {
            self.selected_ids(cx)
        } else {
            self.row_items(row, cx).cloned().collect()
        }
    }

    fn select_only(&mut self, row: usize, cx: &App) {
        self.selected = self.row_items(row, cx).cloned().collect();
        let r = self.row_ref(row, cx);
        self.anchor = Some(r.clone());
        self.cursor = Some(r);
    }

    fn select_range(&mut self, to: usize, cx: &App) {
        let from = self
            .anchor
            .as_ref()
            .and_then(|r| self.row_of(r, cx))
            .unwrap_or(to);
        self.selected = (from.min(to)..=from.max(to))
            .flat_map(|row| self.row_items(row, cx).cloned())
            .collect();
        if self.anchor.is_none() {
            self.anchor = Some(self.row_ref(to, cx));
        }
        self.cursor = Some(self.row_ref(to, cx));
    }

    fn toggle(&mut self, row: usize, cx: &App) {
        let ids: Vec<String> = self.row_items(row, cx).cloned().collect();
        if ids.iter().all(|id| self.selected.contains(id)) {
            for id in &ids {
                self.selected.remove(id);
            }
        } else {
            self.selected.extend(ids);
        }
        let r = self.row_ref(row, cx);
        self.anchor = Some(r.clone());
        self.cursor = Some(r);
    }

    fn click(&mut self, row: usize, event: &ClickEvent, cx: &mut Context<Self>) {
        let modifiers = event.modifiers();
        if event.click_count() >= 2 {
            self.play_row(row, cx);
        } else if modifiers.shift {
            self.select_range(row, cx);
        } else if modifiers.secondary(cx) {
            self.toggle(row, cx);
        } else {
            self.select_only(row, cx);
        }
        cx.notify();
    }

    fn play_row(&self, row: usize, cx: &App) {
        let Some(id) = self.row_items(row, cx).next().cloned() else {
            return;
        };
        command(cx, "play", move |e| e.play(id));
    }

    fn remove(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        for id in &ids {
            self.selected.remove(id);
        }
        command(cx, "remove from queue", move |e| e.remove_from_queue(ids));
        cx.notify();
    }

    fn play_selected(&mut self, _: &PlaySelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.selected_ids(cx).into_iter().next() {
            command(cx, "play", move |e| e.play(id));
        }
    }

    fn remove_selected(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected_ids(cx);
        self.remove(ids, cx);
    }

    fn step(&mut self, by: isize, extend: bool, cx: &mut Context<Self>) {
        let Some(last) = self.rows(cx).len().checked_sub(1) else {
            return;
        };
        let to = match self.cursor.as_ref().and_then(|r| self.row_of(r, cx)) {
            Some(row) => row.saturating_add_signed(by).min(last),
            None if by < 0 => last,
            None => 0,
        };
        if extend {
            self.select_range(to, cx);
        } else {
            self.select_only(to, cx);
        }
        self.list.scroll_to_reveal_item(to);
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, false, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, false, cx);
    }

    fn extend_previous(&mut self, _: &ExtendPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, true, cx);
    }

    fn extend_next(&mut self, _: &ExtendNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, true, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.queue.read(cx).at.keys().cloned().collect();
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        command(cx, "undo", |e| e.undo(None));
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        command(cx, "redo", |e| e.redo(None));
    }

    /// Moves what was dragged to before row `dest`; `dest` past the last row
    /// is the end of the queue. The layout the drag began in must still be
    /// the one on screen: the engine puts items whose target it cannot find
    /// at the end of the queue, so a stale target would move them there.
    fn drop_rows(&mut self, dragged: &DraggedRows, dest: usize, cx: &mut Context<Self>) {
        self.drop_at = None;
        cx.notify();
        let queue = self.queue.read(cx);
        if dragged.whole != queue.whole || dragged.row >= queue.rows.len() {
            return;
        }
        let moving = self.acting_on(dragged.row, cx);
        let queue = self.queue.read(cx);
        let Some((target, after)) = rows::drop_target(&queue.rows, dest, &moving, &queue.base)
        else {
            return;
        };
        command(cx, "move in queue", move |e| {
            e.move_in_queue(moving, target, after)
        });
    }

    /// Queues what was dragged from the library before row `dest`, where
    /// `rows::place` says.
    fn drop_library(&mut self, dragged: &PlayableDrag, dest: usize, cx: &mut Context<Self>) {
        use koan_ffi::Placement;
        self.drop_at = None;
        cx.notify();
        let queue = self.queue.read(cx);
        let id = |ix: usize| queue.base.get(ix).map(|item| item.queue_item_id.clone());
        let sets = dragged.sets.to_vec();
        let place = rows::place(&queue.rows, dest);
        if place == rows::Place::End {
            command(cx, "queue from the library", move |e| {
                e.queue(sets, Placement::Append)
            });
        } else if let rows::Place::After(ix) = place {
            let Some(after) = id(ix) else {
                return;
            };
            command(cx, "queue from the library", move |e| {
                e.queue(
                    sets,
                    Placement::After {
                        queue_item_id: after,
                    },
                )
            });
        } else {
            // Nothing to place after: appended, then moved to the top.
            let Some(first) = id(0) else {
                return;
            };
            command(cx, "queue from the library", move |e| async move {
                let added = e.clone().queue(sets, Placement::Append).await?;
                if added.is_empty() {
                    return Ok(());
                }
                e.move_in_queue(added, first, false).await
            });
        }
    }

    fn drag_label(&self, row: usize, cx: &App) -> SharedString {
        let queue = self.queue.read(cx);
        let count = if self.is_selected(row, cx) {
            self.selected.len()
        } else {
            queue.rows[row].items().len()
        };
        if count == 1 {
            queue.base[queue.rows[row].items().start]
                .title
                .clone()
                .into()
        } else {
            format!("{count} tracks").into()
        }
    }

    fn render_row(
        &mut self,
        row: usize,
        p: &Paint,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = self.rows(cx).len();
        if row >= count {
            return div().into_any_element();
        }
        let selected = self.is_selected(row, cx);
        let r = self.rows(cx)[row];
        // The sleeve first: asking for one can start a fetch, which needs the
        // app, and the rows below only borrow the queue.
        let album = match r {
            Row::Heading { first, .. } => Some(self.queue.read(cx).items[first].album_id),
            Row::Track { ix, grouped: false } => Some(self.queue.read(cx).items[ix].album_id),
            Row::Track { grouped: true, .. } => None,
        };
        let sleeve = album.map(|album| {
            let side = if matches!(r, Row::Heading { .. }) {
                HEADING_SLEEVE
            } else {
                SINGLE_SLEEVE
            };
            covers::draw(covers::image(album, side, cx), side, p.surface)
        });
        // What a track row reads beyond its item: its heart, and how far its
        // transfer has come. A transferring row asks for the next frame, so
        // its ring moves at the display's rate while it is on screen and
        // nothing is drawn for it otherwise.
        let mut favourite = None;
        let mut progress = None;
        if let Row::Track { ix, .. } = r {
            let item = &self.queue.read(cx).items[ix];
            let (track, status) = (item.track_id, item.status);
            if let (Some(track), Some((feed, _))) = (track, &self.feed) {
                favourite = Some(feed.read(cx).is_favourite(Playable::Track(track)));
            }
            if status == EntryStatus::Downloading {
                progress = track.and_then(|track| self.progress(track, cx));
                window.request_animation_frame();
            }
        }
        let feed = self.feed.as_ref().map(|(feed, _)| feed.clone());
        let queue = self.queue.read(cx);
        let (height, line, id) = match r {
            Row::Heading { first, len } => {
                let run = &queue.items[first..first + len];
                // Named by what the row is, so hover and drag state stay with
                // it when the queue moves under the pointer.
                let id = SharedString::from(format!("h:{}", queue.base[first].queue_item_id));
                let look = Look {
                    p,
                    nav: &self.nav,
                    row: &id,
                };
                let line = heading(
                    &run[0],
                    detail(run),
                    sleeve.unwrap_or_else(|| div().into_any_element()),
                    &look,
                );
                (HEADING, line, id)
            }
            Row::Track { ix, grouped } => {
                let height = if grouped { TRACK } else { SINGLE };
                let id = SharedString::from(queue.base[ix].queue_item_id.clone());
                let look = Look {
                    p,
                    nav: &self.nav,
                    row: &id,
                };
                let item = &queue.items[ix];
                let numbering = Numbering {
                    by_disc: item
                        .album_id
                        .is_some_and(|album| queue.disc_records.contains(&album)),
                    wide: !queue.disc_records.is_empty(),
                };
                let line = track(
                    item,
                    grouped,
                    sleeve,
                    numbering,
                    Extras {
                        favourite,
                        feed,
                        progress,
                    },
                    &look,
                );
                (height, line, id)
            }
        };
        let dragged = DraggedRows {
            row,
            whole: queue.whole,
        };
        // The insertion line, while a drag in the queue is over this row: above
        // it, or below the last row for the end of the queue.
        let dragging = cx.has_active_drag();
        let line_above = dragging && self.drop_at == Some(row);
        let line_below = dragging && row + 1 == count && self.drop_at == Some(count);
        let view = cx.entity();
        div()
            .id(ElementId::Name(id.clone()))
            .group(id)
            .h(height)
            .w_full()
            .flex()
            .items_center()
            .px(p.space(Space::L))
            .py(PADDING)
            .gap(p.space(Space::M))
            .when(selected, |d| d.bg(p.surface))
            .when(!selected, |d| d.hover(|s| s.bg(p.hover.opacity(0.3))))
            .when(line_above, |d| d.border_t_1().border_color(p.accent))
            .when(line_below, |d| d.border_b_1().border_color(p.accent))
            .on_drag_move::<DraggedRows>(cx.listener(
                move |this, event: &DragMoveEvent<DraggedRows>, _, cx| {
                    if !event.bounds.contains(&event.event.position) {
                        return;
                    }
                    let lower = event.event.position.y > event.bounds.center().y;
                    let at = rows::insertion_point(row, lower);
                    if this.drop_at != Some(at) {
                        this.drop_at = Some(at);
                        cx.notify();
                    }
                },
            ))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, _| this.menu_row = Some(row)),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.click(row, event, cx);
            }))
            .on_drag(dragged, move |dragged, _, _, cx| {
                let label = view.update(cx, |this, cx| {
                    // A new drag starts from no insertion point.
                    this.drop_at = None;
                    this.drag_label(dragged.row, cx)
                });
                cx.new(|_| DragPreview(label))
            })
            .on_drag_move::<PlayableDrag>(cx.listener(
                move |this, event: &DragMoveEvent<PlayableDrag>, _, cx| {
                    if !event.bounds.contains(&event.event.position) {
                        return;
                    }
                    let lower = event.event.position.y > event.bounds.center().y;
                    let at = rows::insertion_point(row, lower);
                    if this.drop_at != Some(at) {
                        this.drop_at = Some(at);
                        cx.notify();
                    }
                },
            ))
            .on_drop(cx.listener(move |this, dragged: &DraggedRows, _, cx| {
                let dest = this.drop_at.unwrap_or(row);
                this.drop_rows(dragged, dest, cx);
            }))
            .on_drop(cx.listener(move |this, dragged: &PlayableDrag, _, cx| {
                let dest = this.drop_at.unwrap_or(row);
                this.drop_library(dragged, dest, cx);
            }))
            .child(line)
            .into_any_element()
    }
}

fn row_menu(menu: Menu, view: &Entity<QueueView>, cx: &mut App) -> Menu {
    let this = view.read(cx);
    let Some(row) = this.menu_row.filter(|&row| row < this.rows(cx).len()) else {
        return menu;
    };
    let ids = this.acting_on(row, cx);
    let heading = matches!(this.rows(cx)[row], Row::Heading { .. })
        && ids.len() == this.rows(cx)[row].items().len();
    let remove_label = match (heading, ids.len()) {
        (true, _) => "remove record".to_string(),
        (false, 1) => "remove".to_string(),
        (false, n) => format!("remove {n} tracks"),
    };
    // Named when the menu opens, as the rows it acts on are: the queue can be
    // sent whole while the menu is open.
    let first = this.row_items(row, cx).next().cloned();
    let remove = view.clone();
    menu.item(MenuEntry::new("play").on_click(move |_, _, cx| {
        if let Some(id) = first.clone() {
            command(cx, "play", move |e| e.play(id));
        }
    }))
    .item(MenuEntry::new(remove_label).on_click(move |_, _, cx| {
        let ids = ids.clone();
        remove.update(cx, |this, cx| this.remove(ids, cx));
    }))
    .separator()
    .item(MenuEntry::new("undo").on_click(|_, _, cx| command(cx, "undo", |e| e.undo(None))))
    .item(MenuEntry::new("redo").on_click(|_, _, cx| command(cx, "redo", |e| e.redo(None))))
}

fn line(content: impl Into<SharedString>, role: TypeRole, color: Hsla, p: &Paint) -> Div {
    div()
        .min_w(px(0.))
        .truncate()
        .text_role(role, &p.look)
        .text_color(color)
        .child(content.into())
}

/// A record's run as its heading sums it up: the year, how many tracks, how
/// long, and the codec where the run has one.
fn detail(run: &[QueueItem]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(year) = run[0].year.as_ref().filter(|y| !y.is_empty()) {
        parts.push(year.clone());
    }
    parts.push(match run.len() {
        1 => "1 track".into(),
        n => format!("{n} tracks"),
    });
    let total: u64 = run.iter().filter_map(|item| item.duration_ms).sum();
    if total > 0 {
        parts.push(crate::time::clock(total));
    }
    let mut codecs = run.iter().filter_map(|item| item.codec.as_deref());
    if let Some(codec) = codecs.next()
        && codecs.all(|c| c == codec)
    {
        parts.push(codec.to_uppercase());
    }
    parts.join(" · ")
}

/// The glyph with icons on, its word without.
fn glyph(icon: Icon, word: &'static str, role: TypeRole, color: Hsla, p: &Paint) -> AnyElement {
    if p.look.icons {
        KoanIcon::new(icon)
            .role(role)
            .color(color)
            .into_any_element()
    } else {
        div().text_color(color).child(word).into_any_element()
    }
}

/// What every part of one row draws with: the look, where links lead, and
/// the row's own id, which names its parts.
struct Look<'a> {
    p: &'a Paint,
    nav: &'a Entity<Navigator>,
    row: &'a SharedString,
}

impl Look<'_> {
    fn id(&self, part: &str, ix: usize) -> ElementId {
        ElementId::Name(format!("{}:{part}:{ix}", self.row).into())
    }
}

/// How a track row numbers itself: by disc and track on a record with a
/// second disc, and wider wherever any record in the queue does.
#[derive(Clone, Copy)]
struct Numbering {
    by_disc: bool,
    wide: bool,
}

/// A track row's heart and transfer.
struct Extras {
    /// `None` for a track the library does not hold, or before it loads.
    favourite: Option<bool>,
    feed: Option<Entity<Feed>>,
    /// How far the transfer has come, when the server said how big.
    progress: Option<f64>,
}

fn link(
    id: ElementId,
    text: impl Into<SharedString>,
    route: Option<Route>,
    look: &Look,
) -> Stateful<Div> {
    links::link(id, text, route, look.nav, look.p.ink)
}

fn credit(parts: &[CreditPart], fallback: &str, part: &str, look: &Look) -> Vec<AnyElement> {
    links::credit(
        parts,
        fallback,
        |ix| look.id(part, ix),
        look.nav,
        look.p.ink,
    )
}

fn heading(first: &QueueItem, detail: String, sleeve: AnyElement, look: &Look) -> Div {
    let p = look.p;
    let (parts, artist) = if first.album_artist.is_empty() {
        (&first.artist_credit, &first.artist)
    } else {
        (&first.album_artist_credit, &first.album_artist)
    };
    let album = link(
        look.id("album", 0),
        first.album.clone(),
        first.album_id.map(Route::Album),
        look,
    )
    .into_any_element();
    div()
        .flex()
        .flex_1()
        .min_w(px(0.))
        .items_center()
        .gap(p.space(Space::M))
        .child(sleeve)
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.))
                .child(names(vec![album], TypeRole::Body, p.strong, p))
                .child(names(
                    credit(parts, artist, "artist", look),
                    TypeRole::Meta,
                    p.ink,
                    p,
                ))
                .child(line(detail, TypeRole::Fine, p.muted, p)),
        )
}

fn track(
    item: &QueueItem,
    grouped: bool,
    sleeve: Option<AnyElement>,
    numbering: Numbering,
    extras: Extras,
    look: &Look,
) -> Div {
    let p = look.p;
    let playing = item.status == EntryStatus::Playing;
    let title_color = match item.status {
        EntryStatus::Playing => p.accent_text,
        EntryStatus::Played => p.muted,
        EntryStatus::Failed => p.bad,
        EntryStatus::Queued | EntryStatus::Downloading | EntryStatus::PriorityPending => p.ink,
    };
    let leading = if grouped {
        div()
            .w(if numbering.wide { px(36.) } else { px(24.) })
            .flex_shrink_0()
            .flex()
            .justify_end()
            .text_role(TypeRole::Meta, &p.look)
            .text_color(if playing { p.accent_text } else { p.muted })
            .child(rows::place(item, numbering.by_disc))
            .into_any_element()
    } else {
        sleeve.unwrap_or_else(|| div().into_any_element())
    };
    // A grouped row names its artists only where they differ from the
    // record's; a row standing alone names its artist and record.
    let secondary = if grouped {
        item.credit_differs
            .then(|| credit(&item.artist_credit, &item.artist, "artist", look))
    } else {
        let mut parts = credit(&item.artist_credit, &item.artist, "artist", look);
        if !item.album.is_empty() {
            parts.push(
                div()
                    .flex_shrink_0()
                    .child("\u{a0}·\u{a0}")
                    .into_any_element(),
            );
            parts.push(
                link(
                    look.id("album", 0),
                    item.album.clone(),
                    item.album_id.map(Route::Album),
                    look,
                )
                .into_any_element(),
            );
        }
        Some(parts)
    };
    div()
        .flex()
        .flex_1()
        .min_w(px(0.))
        .items_center()
        .gap(p.space(Space::M))
        .child(
            div()
                // A glyph's width, or a word's in the plain look.
                .w(if p.look.icons { px(14.) } else { px(56.) })
                .flex_shrink_0()
                .flex()
                .justify_center()
                .text_role(TypeRole::Fine, &p.look)
                .when(playing, |d| {
                    d.child(glyph(
                        Icon::PlayMark,
                        "playing",
                        TypeRole::Fine,
                        p.accent_text,
                        p,
                    ))
                }),
        )
        .child(leading)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .child(line(item.title.clone(), TypeRole::Body, title_color, p))
                .children(secondary.map(|parts| names(parts, TypeRole::Meta, p.muted, p))),
        )
        .child(availability(item, extras.progress, p))
        .child(heart(item, &extras, look))
        .children(item.codec.clone().map(|codec| {
            div()
                .w(px(48.))
                .flex_shrink_0()
                .text_role(TypeRole::Fine, &p.look)
                .text_color(p.muted)
                .child(codec.to_uppercase())
        }))
        .child(
            div()
                .w(px(44.))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_role(TypeRole::Meta, &p.look)
                .text_color(p.muted)
                .child(item.duration_ms.map(crate::time::clock).unwrap_or_default()),
        )
}

/// Where the track's file is, or how its download is going, as the Mac's
/// `AvailabilityMark`: waiting at the front of the downloads, failed, a ring
/// filling as the bytes land (spinning while the size is unknown), or kept
/// on the server, downloaded from it, or a local file.
fn availability(item: &QueueItem, progress: Option<f64>, p: &Paint) -> AnyElement {
    let quiet = p.muted.opacity(0.6);
    let mark = |icon, word, color| glyph(icon, word, TypeRole::Fine, color, p);
    let inner = match item.status {
        EntryStatus::PriorityPending => Some(mark(Icon::Downloads, "next", p.accent_text)),
        EntryStatus::Failed => Some(mark(Icon::WarningFilled, "failed", p.bad)),
        EntryStatus::Downloading => Some(ring(progress, p.muted).into_any_element()),
        _ => match (item.on_server, item.on_disk) {
            (true, true) => Some(mark(Icon::CloudKept, "kept", p.muted)),
            (true, false) => Some(mark(Icon::Cloud, "server", quiet)),
            (false, true) => Some(mark(Icon::OnDevice, "local", p.muted)),
            (false, false) => None,
        },
    };
    div()
        .w(if p.look.icons { px(14.) } else { px(48.) })
        .flex_shrink_0()
        .flex()
        .justify_end()
        .text_role(TypeRole::Fine, &p.look)
        .children(inner)
        .into_any_element()
}

/// A download's ring: an arc of `progress` from twelve o'clock, or, with no
/// size to measure against, a spinning seven-tenths of one. Painted as one
/// path; only a row on screen with a running transfer asks for frames.
fn ring(progress: Option<f64>, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            use std::f32::consts::{FRAC_PI_2, TAU};
            let (start, sweep) = match progress {
                Some(fraction) => (0.0, (fraction as f32).clamp(0.02, 1.0)),
                None => {
                    let ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() % 1000)
                        .unwrap_or_default();
                    (ms as f32 / 1000.0, 0.7)
                }
            };
            let centre = bounds.center();
            let radius = px(5.);
            let steps = ((sweep * 48.0).ceil() as usize).max(2);
            let mut path = PathBuilder::stroke(px(1.5));
            for step in 0..=steps {
                let angle = (start + sweep * step as f32 / steps as f32) * TAU - FRAC_PI_2;
                let at = point(
                    centre.x + radius * angle.cos(),
                    centre.y + radius * angle.sin(),
                );
                if step == 0 {
                    path.move_to(at);
                } else {
                    path.line_to(at);
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(12.))
    .flex_shrink_0()
}

/// The heart, as the Mac's: shown while the track is a favourite or the row
/// is under the pointer, and toggling it.
fn heart(item: &QueueItem, extras: &Extras, look: &Look) -> AnyElement {
    let p = look.p;
    let slot = div().w(px(16.)).flex_shrink_0().flex().justify_center();
    let (Some(track), Some(favourite), Some(feed)) =
        (item.track_id, extras.favourite, extras.feed.clone())
    else {
        return slot.into_any_element();
    };
    let row = look.row.clone();
    slot.child(
        div()
            .id(look.id("heart", 0))
            .cursor_pointer()
            .when(!favourite, |d| {
                d.invisible().group_hover(row, |s| s.visible())
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                feed.update(cx, |feed, cx| {
                    feed.toggle_favourite(Playable::Track(track), cx)
                });
            })
            .child(glyph(
                if favourite {
                    Icon::Favourited
                } else {
                    Icon::Favourite
                },
                if favourite { "♥" } else { "♡" },
                TypeRole::Fine,
                if favourite { p.accent_text } else { p.muted },
                p,
            )),
    )
    .into_any_element()
}

impl Focusable for QueueView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl QueueView {
    /// The page's header, as the Mac's: what the queue is ("queue", or
    /// "playing" and the playlist or record it still follows, with its
    /// sleeve) and its count and length, then the pick and the queue's
    /// actions.
    fn header(&mut self, p: &Paint, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let lock = cx.global::<Mirror>().lock.read(cx).0.clone();
        let (grouped, tracks, total) = {
            let queue = self.queue.read(cx);
            let total: u64 = queue.items.iter().filter_map(|item| item.duration_ms).sum();
            (queue.grouped, queue.items.len(), total)
        };
        let picked = self.selected.len();
        // A queue that is one record, grouped, has that record's heading as
        // its first row; the header does not say it again.
        let record_once = grouped && matches!(lock, Some(QueueLock::Album { .. }));
        let mut summary = crate::library::chrome::count(tracks as u64, "track", "tracks");
        if total > 0 {
            summary = format!("{summary} · {}", crate::time::clock(total));
        }
        let look_row = SharedString::from("queue-header");
        let look = Look {
            p,
            nav: &self.nav,
            row: &look_row,
        };
        let (sleeve, title): (Option<AnyElement>, Vec<AnyElement>) = match &lock {
            _ if record_once => (None, Vec::new()),
            Some(QueueLock::Album { album }) => (
                Some(covers::sleeve(Some(album.id), HEADER_SLEEVE, p.surface, cx)),
                vec![
                    div()
                        .flex_shrink_0()
                        .child("playing\u{a0}")
                        .into_any_element(),
                    link(
                        look.id("album", 0),
                        album.title.clone(),
                        Some(Route::Album(album.id)),
                        &look,
                    )
                    .into_any_element(),
                ],
            ),
            Some(QueueLock::Playlist { playlist }) => (
                None,
                vec![
                    div()
                        .flex_shrink_0()
                        .child("playing\u{a0}")
                        .into_any_element(),
                    link(look.id("playlist", 0), playlist.name.clone(), None, &look)
                        .into_any_element(),
                ],
            ),
            None => (None, vec![div().child("queue").into_any_element()]),
        };
        let icon = |id: &'static str, icon: Icon, label: &'static str| {
            Button::new(id, ButtonKind::Icon).icon(icon).label(label)
        };
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(p.space(Space::M))
            .px(p.space(Space::L))
            .pt(p.space(Space::L))
            .pb(p.space(Space::M))
            .children(sleeve)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .when(!title.is_empty(), |d| {
                        d.child(names(title, TypeRole::Body, p.strong, p))
                    })
                    .when(!record_once, |d| {
                        d.child(line(summary, TypeRole::Fine, p.muted, p))
                    }),
            )
            .when(picked > 0, |d| {
                d.child(line(
                    format!("{picked} selected"),
                    TypeRole::Meta,
                    p.muted,
                    p,
                ))
                .child(
                    icon("queue-remove", Icon::Remove, "remove selected").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.remove_selected(&Delete, window, cx)
                        }),
                    ),
                )
            })
            .child(
                icon("queue-jump", Icon::JumpToPlaying, "jump to playing")
                    .on_click(cx.listener(|this, _, _, cx| this.jump_to_playing(cx))),
            )
            .child(
                icon(
                    "queue-dice",
                    Icon::Reshuffle,
                    "shuffle the rest of the queue in place",
                )
                .disabled(tracks < 2)
                .on_click(|_, _, cx| command(cx, "shuffle", |e| e.shuffle_queue())),
            )
            .child(
                icon("queue-group", Icon::Album, "group by album")
                    .selected(grouped)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_grouped(!grouped, window, cx)
                    })),
            )
            .child(
                icon("queue-undo", Icon::Undo, "undo")
                    .on_click(|_, _, cx| command(cx, "undo", |e| e.undo(None))),
            )
            .child(
                icon("queue-redo", Icon::Redo, "redo")
                    .on_click(|_, _, cx| command(cx, "redo", |e| e.redo(None))),
            )
            .child(
                // Undoable, with undo beside it.
                icon("queue-clear", Icon::Clear, "clear queue")
                    .disabled(tracks == 0)
                    .on_click(|_, _, cx| command(cx, "clear the queue", |e| e.clear_queue())),
            )
    }

    /// Scrolls to the playing row, if there is one.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let queue = self.queue.read(cx);
        let row = queue
            .items
            .iter()
            .find(|item| item.status == EntryStatus::Playing)
            .and_then(|item| queue.track_rows.get(&item.queue_item_id))
            .copied();
        if let Some(row) = row {
            self.list.scroll_to_reveal_item(row);
            cx.notify();
        }
    }
}

impl Render for QueueView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        self.readings = None;
        let top = self.list.logical_scroll_top().item_ix;
        self.keep_top(top, cx);
        let count = self.rows(cx).len();
        let view = cx.entity();
        if self.drop_at.is_some() && !cx.has_active_drag() {
            self.drop_at = None;
        }
        div()
            .id("queue")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::play_selected))
            .on_action(cx.listener(Self::remove_selected))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::extend_previous))
            .on_action(cx.listener(Self::extend_next))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(|this, _: &ClearSelection, _, cx| {
                this.selected.clear();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TopOfQueue, _, cx| {
                this.list.scroll_to(ListOffset::default());
                this.top = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &EndOfQueue, _, cx| {
                if let Some(last) = this.rows(cx).len().checked_sub(1) {
                    this.list.scroll_to_reveal_item(last);
                    cx.notify();
                }
            }))
            // Files from outside: indexed where they lie and appended.
            .on_drop(cx.listener(|_, paths: &ExternalPaths, _, cx| {
                drop::queue_files(&engine(cx), paths.paths().to_vec(), cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            // One menu for the list, on the row the secondary click landed on:
            // cleared as the press arrives, set by the row it reaches.
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                if event.button == MouseButton::Right {
                    this.menu_row = None;
                }
            }))
            // A library drag that leaves the queue takes its insertion line
            // with it, so a drag released elsewhere leaves none behind.
            .on_drag_move::<PlayableDrag>(cx.listener(
                |this, event: &DragMoveEvent<PlayableDrag>, _, cx| {
                    if this.drop_at.is_some() && !event.bounds.contains(&event.event.position) {
                        this.drop_at = None;
                        cx.notify();
                    }
                },
            ))
            // Below the last row: the end of the queue.
            .on_drop(cx.listener(move |this, dragged: &DraggedRows, _, cx| {
                let dest = this.drop_at.unwrap_or(count);
                this.drop_rows(dragged, dest, cx);
            }))
            .on_drop(cx.listener(move |this, dragged: &PlayableDrag, _, cx| {
                let dest = this.drop_at.unwrap_or(count);
                this.drop_library(dragged, dest, cx);
            }))
            .child(self.header(&p, cx))
            .when(count == 0, |d| {
                d.child(
                    div().flex_1().flex().items_center().justify_center().child(
                        div()
                            .text_role(TypeRole::Body, &p.look)
                            .text_color(p.muted)
                            .child("the queue is empty"),
                    ),
                )
            })
            .when(count > 0, |d| {
                let view = view.clone();
                d.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(
                            list(self.list.clone(), move |row, window, cx| {
                                view.update(cx, |this, cx| this.render_row(row, &p, window, cx))
                            })
                            .size_full(),
                        )
                        .child(gpui_kit::base::Scrollbar::vertical(&self.list)),
                )
            })
            .koan_context_menu("queue-menu", window, cx, {
                let view = view.clone();
                move |menu, _, cx| row_menu(menu, &view, cx)
            })
    }
}
