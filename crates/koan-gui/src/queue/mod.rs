//! The queue: the main stage, grouped by record.
//!
//! The view outlives page switches (the shell holds it for the life of the
//! window), so its selection and scroll position survive a visit elsewhere.
//! Rows are laid out again only when the queue is sent whole; a patch (a
//! track change, a download landing) redraws the rows on screen, which read
//! their item as it is now. A patch never changes a row's height or place, so
//! the list keeps its measurements across one.

pub mod rows;

use std::collections::HashSet;

use koan_ffi::{CreditPart, EntryStatus, QueueItem, QueueLock};
use koan_kit::app::*;
// the colour a row part is drawn in, chosen from the theme by the caller.
use koan_kit::raw::Hsla;

use crate::components::{
    Availability, AvailabilityMark, Cover, DragChip, EmptyState, INSERTION, IconToggle,
    KoanContextMenu, Menu, MenuEntry, MenuPanel, MenuRow, Page, PageHeader, RowKind, RowList,
    RowListState, RowMetrics, RowOffset, TableRow, ToolbarButton, ToolbarGroup, ToolbarMenu,
    ToolbarSelection,
};
use crate::covers::{self, CoversHandle};
use crate::engine::{command, engine};
use crate::library::drag::{Playable, PlayableDrag, PlaylistDrag};
use crate::library::feed::Feed;
use crate::links::{self, names};
use crate::mirror::{Mirror, Queue};
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::platform::actions::{
    ClearSelection, Delete, EndOfQueue, Redo, SelectAll, TopOfQueue, Undo,
};
use crate::platform::drop;
use crate::theme::{self, Icon, KoanIcon, KoanStyled, TypeRole};
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

/// The queue's columns, in points at 100 %, the Mac's: the playing mark (a
/// word's room in the plain look), the place on the record (wider when a
/// record in the queue numbers by disc), then the figures trailing.
const MARK: f32 = 16.0;
const MARK_WORD: f32 = 56.0;
const PLACE: f32 = 24.0;
const PLACE_WIDE: f32 = 36.0;
const AVAILABILITY: f32 = 14.0;
const AVAILABILITY_WORD: f32 = 48.0;
const HEART: f32 = 16.0;
const CODEC: f32 = 48.0;
const LENGTH: f32 = 44.0;

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

/// What a drag of queue rows shows: the first row's sleeve and how many.
struct DragPreview(SharedString, Option<i64>);

impl Render for DragPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let side = RowMetrics::nav_art(&p.look);
        let chip = DragChip::new(self.0.clone());
        match self.1 {
            Some(album) => chip.sleeve(covers::art(Some(album), side, cx)),
            None => chip,
        }
    }
}

pub struct QueueView {
    queue: Entity<Queue>,
    focus: FocusHandle,
    list: RowListState,
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
    /// Keeping the playing track in view as it changes, the Mac's: on from
    /// the crosshair, off by a scroll of the person's own or with nothing
    /// playing. For the session only.
    following: bool,
    /// The queue item last brought into view while following, and its row.
    followed: Option<String>,
    followed_row: Option<usize>,
    /// The rest of the queue can be shuffled in place: there is a rest, and
    /// shuffle play is off (it keeps an order of its own beside the queue).
    shufflable: bool,
    _subscriptions: [Subscription; 6],
}

impl QueueView {
    pub fn new(nav: &Entity<Navigator>, cx: &mut Context<Self>) -> Self {
        let queue = cx.global::<Mirror>().queue.clone();
        let covers = cx.global::<CoversHandle>().0.clone();
        let lock = cx.global::<Mirror>().lock.clone();
        let playback = cx.global::<Mirror>().playback.clone();
        let subscriptions = [
            cx.observe(&playback, |this, playback, cx| {
                let now = &playback.read(cx).0;
                let shufflable = now.queue_shufflable && !now.shuffle;
                if shufflable != this.shufflable {
                    this.shufflable = shufflable;
                    cx.notify();
                }
            }),
            // What the queue follows names the header.
            cx.observe(&lock, |_, _, cx| cx.notify()),
            cx.observe(&queue, |this, queue, cx| {
                if queue.read(cx).whole != this.whole {
                    this.lay_out(cx);
                }
                this.follow(cx);
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
                    this.list.scroll_to_top();
                    // Held now, not at the next paint: a queue sent whole in
                    // between must keep the top, not the row left behind.
                    this.top = None;
                    cx.notify();
                }
            }),
        ];
        // The list calls this as it scrolls, with its own state held, so the
        // handler reads the queue and never the list.
        let list = RowListState::new();
        let weak = cx.weak_entity();
        list.on_scroll(move |start, _, cx| {
            weak.update(cx, |this, cx| this.keep_top(start, cx)).ok();
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
            following: false,
            followed: None,
            followed_row: None,
            shufflable: {
                let now = &cx.global::<Mirror>().playback.read(cx).0;
                now.queue_shufflable && !now.shuffle
            },
            _subscriptions: subscriptions,
        };
        // The queue can arrive before the window's saved layout is read.
        let grouped = window_state::GROUPED.load(std::sync::atomic::Ordering::Relaxed);
        if view.queue.read(cx).grouped != grouped {
            crate::mirror::regroup(&view.queue, grouped, cx);
        }
        view.lay_out(cx);
        view
    }

    /// "Group by album": the rows under record headings, or flat.
    fn set_grouped(&mut self, grouped: bool, window: &mut Window, cx: &mut Context<Self>) {
        window_state::GROUPED.store(grouped, std::sync::atomic::Ordering::Relaxed);
        window_state::save(window.window_bounds(), cx);
        crate::mirror::regroup(&self.queue, grouped, cx);
    }

    /// The rows' hearts read and toggle the library's favourites.
    pub fn set_feed(&mut self, feed: Entity<Feed>, cx: &mut Context<Self>) {
        let observed = cx.observe(&feed, |_, _, cx| cx.notify());
        self.feed = Some((feed, observed));
        cx.notify();
    }

    /// The queue was sent whole: its rows were laid out off the main thread
    /// (`Queue::prepare`); what is left here is keeping the pick and the list.
    fn lay_out(&mut self, cx: &mut Context<Self>) {
        let queue = self.queue.read(cx);
        self.whole = queue.whole;
        self.selected.retain(|id| queue.at.contains_key(id));
        let top = self.list.top();
        // A hint for the rows not yet measured, so the scrollbar is sized for
        // the whole queue from the first frame.
        self.list.reset(queue.rows.len(), RowKind::Text);
        let kept = self.top.take();
        let landing = match kept.and_then(|(r, at)| Some((self.row_of(&r, cx)?, at))) {
            Some((row, at)) => RowOffset {
                item_ix: row,
                // The offset into the row belongs to it only if it is still
                // the row the list was at.
                offset_in_item: if at == top.item_ix {
                    top.offset_in_item
                } else {
                    Pixels::ZERO
                },
            },
            // The row is gone: stay at the same place in the list.
            None => RowOffset {
                item_ix: top.item_ix,
                offset_in_item: Pixels::ZERO,
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
        self.list.reveal(to);
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = self.rows(cx).len();
        if row >= count {
            return div().into_any_element();
        }
        let selected = self.is_selected(row, cx);
        // The insertion line keeps the accent over a picked row.
        let insertion = p.accent;
        let p = &p.picked(selected);
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
                RowMetrics::heading_sleeve(&p.look)
            } else {
                RowMetrics::sleeve(&p.look)
            };
            let art = covers::art(album, side, cx);
            Cover::new(art, side)
                .id(("sleeve", row as u64))
                .into_any_element()
        });
        // What a track row reads beyond its item: its heart. A transfer's
        // progress its availability mark reads each frame it is drawn.
        let mut favourite = None;
        let mut availability = None;
        if let Row::Track { ix, .. } = r {
            let item = &self.queue.read(cx).items[ix];
            let (status, track, on_server, on_disk) =
                (item.status, item.track_id, item.on_server, item.on_disk);
            let mut transfer = None;
            if let (Some(track), Some((feed, _))) = (track, &self.feed) {
                let feed = feed.read(cx);
                favourite = Some(feed.is_favourite(Playable::Track(track)));
                transfer = feed.transfers.get(&track).copied();
            }
            // Without the library's feed (an offscreen render), the row's own
            // status is all there is to go on.
            if self.feed.is_none() && status == EntryStatus::Downloading {
                transfer = Some(koan_ffi::TransferState::Running);
            }
            availability =
                crate::transfers::of_queue_row(status, track, on_server, on_disk, transfer);
        }
        let feed = self.feed.as_ref().map(|(feed, _)| feed.clone());
        let queue = self.queue.read(cx);
        let (row_frame, _) = match r {
            Row::Heading { first, len } => {
                let run = &queue.items[first..first + len];
                // Named by what the row is, so hover and drag state stay with
                // it when the queue moves under the pointer.
                let id = SharedString::from(format!("h:{}", queue.base[first].queue_item_id));
                let look = Look {
                    p,
                    nav: &self.nav,
                    row: &id,
                    picked: selected,
                    waiting: true,
                };
                let frame = heading(
                    TableRow::new(ElementId::Name(id.clone()), RowKind::Heading),
                    &run[0],
                    detail(run),
                    sleeve.unwrap_or_else(|| div().into_any_element()),
                    &look,
                );
                (frame, id)
            }
            Row::Track { ix, grouped } => {
                let kind = if grouped { RowKind::Text } else { RowKind::Art };
                let id = SharedString::from(queue.base[ix].queue_item_id.clone());
                let look = Look {
                    p,
                    nav: &self.nav,
                    row: &id,
                    picked: selected,
                    waiting: true,
                };
                let item = &queue.items[ix];
                let numbering = Numbering {
                    by_disc: item
                        .album_id
                        .is_some_and(|album| queue.disc_records.contains(&album)),
                    wide: !queue.disc_records.is_empty(),
                };
                let frame = track(
                    TableRow::new(ElementId::Name(id.clone()), kind),
                    item,
                    grouped,
                    sleeve,
                    numbering,
                    Extras {
                        favourite,
                        feed,
                        availability,
                    },
                    &look,
                );
                (frame, id)
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
        let list = self.list.clone();
        row_frame
            .selected(selected)
            .when(p.a11y, |frame| {
                frame
                    .place(row, count)
                    .on_reveal(move |_, _| list.reveal_around(row))
            })
            .when(line_above, |d| {
                d.border_t(INSERTION).border_color(insertion)
            })
            .when(line_below, |d| {
                d.border_b(INSERTION).border_color(insertion)
            })
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
                let (label, album) = view.update(cx, |this, cx| {
                    // A new drag starts from no insertion point.
                    this.drop_at = None;
                    let queue = this.queue.read(cx);
                    let album = queue
                        .rows
                        .get(dragged.row)
                        .and_then(|row| queue.items.get(row.items().start))
                        .and_then(|item| item.album_id);
                    (this.drag_label(dragged.row, cx), album)
                });
                cx.new(|_| DragPreview(label, album))
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
            .on_drag_move::<PlaylistDrag>(cx.listener(
                move |this, event: &DragMoveEvent<PlaylistDrag>, _, cx| {
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
            .on_drop(cx.listener(move |this, dragged: &PlaylistDrag, _, cx| {
                let dest = this.drop_at.unwrap_or(row);
                this.drop_library(&dragged.as_library(), dest, cx);
            }))
            .into_any_element()
    }
}

fn row_menu(menu: Menu, view: &Entity<QueueView>, cx: &mut App) -> Menu {
    let this = view.read(cx);
    let Some(row) = this.menu_row.filter(|&row| row < this.rows(cx).len()) else {
        return menu;
    };
    let ids = this.acting_on(row, cx);
    let grouped = this.queue.read(cx).grouped;
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
    // The library's tracks under the rows, for the verbs on their files.
    let tracks: Vec<i64> = {
        let queue = this.queue.read(cx);
        ids.iter()
            .filter_map(|id| queue.at.get(id).and_then(|&ix| queue.items[ix].track_id))
            .collect()
    };
    let title = match tracks.len() {
        1 => "1 track".to_owned(),
        n => format!("{n} tracks"),
    };
    let remove = view.clone();
    menu.item(
        MenuEntry::new("play")
            .icon(Icon::Play)
            .on_click(move |_, _, cx| {
                if let Some(id) = first.clone() {
                    command(cx, "play", move |e| e.play(id));
                }
            }),
    )
    .item(
        MenuEntry::new(remove_label)
            .icon(Icon::Remove)
            .on_click(move |_, _, cx| {
                let ids = ids.clone();
                remove.update(cx, |this, cx| this.remove(ids, cx));
            }),
    )
    .separator()
    .item(
        MenuEntry::new("undo")
            .icon(Icon::Undo)
            .on_click(|_, _, cx| command(cx, "undo", |e| e.undo(None))),
    )
    .item(
        MenuEntry::new("redo")
            .icon(Icon::Redo)
            .on_click(|_, _, cx| command(cx, "redo", |e| e.redo(None))),
    )
    .separator()
    .item(MenuEntry::new("organise…").icon(Icon::Organize).on_click({
        let (tracks, title) = (tracks.clone(), title.clone());
        move |_, window, cx| crate::organize::open(title.clone(), tracks.clone(), window, cx)
    }))
    .item(
        MenuEntry::new("download to cache")
            .icon(Icon::Downloads)
            .on_click({
                let tracks = tracks.clone();
                move |_, _, cx| {
                    let tracks = tracks.clone();
                    command(cx, "download to cache", move |e| {
                        e.download_to_cache(tracks)
                    })
                }
            }),
    )
    .item(
        MenuEntry::new("remove downloaded files…")
            .icon(Icon::Clear)
            .on_click(move |_, window, cx| {
                crate::library::actions::ask_remove_downloads(
                    crate::engine::engine(cx),
                    title.clone(),
                    tracks.clone(),
                    window,
                    cx,
                )
            }),
    )
    .separator()
    .item(
        MenuEntry::new("jump to playing")
            .icon(Icon::JumpToPlaying)
            .on_click({
                let view = view.clone();
                move |_, _, cx| view.update(cx, |this, cx| this.jump_to_playing(cx))
            }),
    )
    .item(
        MenuEntry::new("group by album")
            .icon(Icon::Album)
            .checked(grouped)
            .on_click({
                let view = view.clone();
                move |_, window, cx| {
                    view.update(cx, |this, cx| this.set_grouped(!grouped, window, cx))
                }
            }),
    )
}

pub(crate) fn line(
    content: impl Into<SharedString>,
    role: TypeRole,
    color: Hsla,
    p: &Paint,
) -> Div {
    div()
        .min_w_0()
        .truncate()
        .text_role(role, &p.look)
        .text_color(color)
        .child(content.into())
}

/// A record's run as its heading sums it up: the year, how many tracks, how
/// long, and the codec where the run has one.
pub(crate) fn detail(run: &[QueueItem]) -> String {
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
pub(crate) struct Look<'a> {
    pub p: &'a Paint,
    pub nav: &'a Entity<Navigator>,
    pub row: &'a SharedString,
    /// Picked: drawn in `on_selection`, which `p` already is.
    pub picked: bool,
    /// Rows of the queue mark a track waiting to play; a playlist's rows
    /// are not waiting for anything.
    pub waiting: bool,
}

impl Look<'_> {
    fn id(&self, part: &str, ix: usize) -> ElementId {
        ElementId::Name(format!("{}:{part}:{ix}", self.row).into())
    }
}

/// How a track row numbers itself: by disc and track on a record with a
/// second disc, and wider wherever any record in the queue does.
#[derive(Clone, Copy)]
pub(crate) struct Numbering {
    pub by_disc: bool,
    pub wide: bool,
}

/// A track row's heart and transfer.
pub(crate) struct Extras {
    /// `None` for a track the library does not hold, or before it loads.
    pub favourite: Option<bool>,
    pub feed: Option<Entity<Feed>>,
    /// Where its file is, or its transfer's glide.
    pub availability: Option<Availability>,
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

pub(crate) fn heading(
    row: TableRow,
    first: &QueueItem,
    detail: String,
    sleeve: AnyElement,
    look: &Look,
) -> TableRow {
    let p = look.p;
    let (parts, artist) = if first.album_artist.is_empty() {
        (&first.artist_credit, &first.artist)
    } else {
        (&first.album_artist_credit, &first.album_artist)
    };
    let row = if p.a11y {
        row.spoken(crate::components::spoken([
            first.album.as_str(),
            artist.as_str(),
            detail.as_str(),
        ]))
    } else {
        row
    };
    let album = link(
        look.id("album", 0),
        first.album.clone(),
        first.album_id.map(Route::Album),
        look,
    )
    .into_any_element();
    row.lead_exact(RowMetrics::heading_sleeve(&p.look), sleeve)
        .title(names(vec![album], TypeRole::Body, p.strong, p))
        .line(names(
            credit(parts, artist, "artist", look),
            TypeRole::Meta,
            p.muted,
            p,
        ))
        .line(line(detail, TypeRole::Fine, p.muted, p))
}

pub(crate) fn track(
    row: TableRow,
    item: &QueueItem,
    grouped: bool,
    sleeve: Option<AnyElement>,
    numbering: Numbering,
    extras: Extras,
    look: &Look,
) -> TableRow {
    let p = look.p;
    let playing = item.status == EntryStatus::Playing;
    let title_color = match item.status {
        EntryStatus::Playing => p.accent_text,
        EntryStatus::Played => p.muted,
        EntryStatus::Failed => p.bad,
        EntryStatus::Queued | EntryStatus::Downloading | EntryStatus::PriorityPending => p.ink,
    };
    let place = div()
        .text_role(TypeRole::Meta, &p.look)
        .text_color(if playing { p.accent_text } else { p.muted })
        .child(rows::track_place(item, numbering.by_disc));
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
    // What the track is doing, the Mac's status column: the bars while it
    // plays, moving with the music; a tick once played; the pending square
    // while it waits; next in line to download, or failed.
    let quiet = if look.picked { p.on_selection } else { p.rule };
    // What the mark means, said when the pointer rests on it.
    let meaning = match item.status {
        EntryStatus::Playing => Some("playing"),
        EntryStatus::Played => Some("played"),
        EntryStatus::PriorityPending => Some("downloads next"),
        EntryStatus::Failed => Some("couldn't be played"),
        EntryStatus::Queued | EntryStatus::Downloading if look.waiting => {
            Some("waiting for its download")
        }
        EntryStatus::Queued | EntryStatus::Downloading => None,
    };
    let mark = div()
        .id(look.id("status", 0))
        .when_some(meaning, |d, meaning| {
            d.tooltip(move |window, cx| {
                crate::components::Tooltip::view(crate::theme::label(meaning), window, cx)
            })
        })
        .text_role(TypeRole::Fine, &p.look)
        .map(|d| match item.status {
            // Bars in the look that draws glyphs, as the library's rows:
            // moving with what is heard, so they fall with a pause's fade and
            // lie flat while paused. The word otherwise.
            EntryStatus::Playing if p.look.icons => d.child(crate::levels::bars(p.accent_text)),
            EntryStatus::Playing => d.child(glyph(
                Icon::PlayMark,
                "playing",
                TypeRole::Fine,
                p.accent_text,
                p,
            )),
            EntryStatus::Played => {
                d.child(glyph(Icon::Check, "played", TypeRole::Fine, p.muted, p))
            }
            EntryStatus::PriorityPending => d.child(glyph(
                Icon::Downloads,
                "next",
                TypeRole::Fine,
                p.accent_text,
                p,
            )),
            EntryStatus::Failed => d.child(glyph(
                Icon::WarningFilled,
                "failed",
                TypeRole::Fine,
                p.bad,
                p,
            )),
            EntryStatus::Queued | EntryStatus::Downloading => d
                .when(p.look.icons && look.waiting, |d| {
                    d.child(glyph(Icon::Pending, "", TypeRole::Fine, quiet, p))
                }),
        });
    let row = if p.a11y {
        let length = item.duration_ms.map(crate::time::clock).unwrap_or_default();
        row.spoken(crate::components::spoken([
            item.title.as_str(),
            if grouped && !item.credit_differs {
                ""
            } else {
                item.artist.as_str()
            },
            if grouped { "" } else { item.album.as_str() },
            length.as_str(),
            meaning.unwrap_or(""),
            if extras.favourite == Some(true) {
                "favourite"
            } else {
                ""
            },
            extras
                .availability
                .as_ref()
                .and_then(|a| a.help())
                .unwrap_or(""),
        ]))
    } else {
        row
    };
    let row = row.lead(if p.look.icons { MARK } else { MARK_WORD }, mark);
    let row = if grouped {
        row.lead(if numbering.wide { PLACE_WIDE } else { PLACE }, place)
    } else {
        row.lead_exact(
            RowMetrics::sleeve(&p.look),
            sleeve.unwrap_or_else(|| div().into_any_element()),
        )
    };
    let row = row.title(line(item.title.clone(), TypeRole::Body, title_color, p));
    let row = match secondary {
        Some(parts) => row.line(names(parts, TypeRole::Meta, p.muted, p)),
        None => row,
    };
    row.figure(
        if p.look.icons {
            AVAILABILITY
        } else {
            AVAILABILITY_WORD
        },
        AvailabilityMark::new(extras.availability.clone())
            .id(look.id("availability", 0))
            .picked(look.picked)
            .into_any_element(),
    )
    .figure(HEART, heart(item, &extras, look))
    .figure(
        CODEC,
        div()
            .text_role(TypeRole::Fine, &p.look)
            .text_color(p.muted)
            .children(item.codec.as_ref().map(|codec| codec.to_uppercase())),
    )
    .figure(
        LENGTH,
        div()
            .text_role(TypeRole::Meta, &p.look)
            .text_color(p.muted)
            .child(item.duration_ms.map(crate::time::clock).unwrap_or_default()),
    )
}

/// The heart, as the Mac's: shown while the track is a favourite or the row
/// is under the pointer, and toggling it.
fn heart(item: &QueueItem, extras: &Extras, look: &Look) -> AnyElement {
    let p = look.p;
    let slot = div().flex().justify_center();
    let (Some(track), Some(favourite), Some(feed)) =
        (item.track_id, extras.favourite, extras.feed.clone())
    else {
        return slot.into_any_element();
    };
    slot.child(
        div()
            .id(look.id("heart", 0))
            .cursor_pointer()
            .when(!favourite, |d| {
                // The row's own group, which `TableRow` names for every row.
                d.invisible()
                    .group_hover(crate::components::ROW_GROUP, |s| s.visible())
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
    /// Scrolls to the playing row and follows it, as the crosshair does.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        if !self.following {
            self.toggle_follow(cx);
        } else if let Some((_, row)) = self.playing_row(cx) {
            self.list.reveal(row);
            cx.notify();
        }
    }

    /// The cursor's row: the track playing, or the one the player waits
    /// on while it downloads or after it failed, which a row's status names
    /// otherwise.
    fn playing_row(&self, cx: &App) -> Option<(String, usize)> {
        let id = cx
            .global::<Mirror>()
            .playback
            .read(cx)
            .0
            .queue_item_id
            .clone()?;
        let row = *self.queue.read(cx).track_rows.get(&id)?;
        Some((id, row))
    }

    /// The crosshair: jump to the playing track and follow it, or stop.
    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        if self.following {
            self.following = false;
        } else if let Some((id, row)) = self.playing_row(cx) {
            self.list.reveal(row);
            self.following = true;
            self.followed = Some(id);
            self.followed_row = Some(row);
        }
        cx.notify();
    }

    /// While following, a new playing track is brought into view; with
    /// nothing playing there is nothing to follow.
    fn follow(&mut self, cx: &mut Context<Self>) {
        if !self.following {
            return;
        }
        match self.playing_row(cx) {
            None => self.following = false,
            // Revealed when it changes, and when an edit moves it.
            Some((id, row)) => {
                if self.followed.as_ref() != Some(&id) || self.followed_row != Some(row) {
                    self.list.reveal(row);
                    self.followed = Some(id);
                    self.followed_row = Some(row);
                }
            }
        }
    }
}

impl QueueView {
    /// The page's header, the Mac's: what the queue is (a playlist or a
    /// record it follows, or the queue), how many tracks and how long, and
    /// what can be done to it as a whole.
    fn header(&self, cx: &mut Context<Self>) -> PageHeader {
        let queue = self.queue.read(cx);
        let count = queue.items.len() as u64;
        let total: u64 = queue.items.iter().filter_map(|item| item.duration_ms).sum();
        let title = match &cx.global::<Mirror>().lock.read(cx).0 {
            Some(QueueLock::Playlist { playlist }) => format!("playing {}", playlist.name),
            Some(QueueLock::Album { album }) => format!("playing {}", album.title),
            None => "queue".into(),
        };
        let mut detail = crate::library::chrome::count(count, "track", "tracks");
        if total > 0 {
            detail = format!("{detail} · {}", crate::time::clock(total));
        }
        PageHeader::data(title).detail(detail)
    }

    /// The queue's controls in the window's toolbar, the Mac's: while rows
    /// are picked, how many and what to do with them; then jump to playing,
    /// the dice, grouping, undo and redo, and the rest under ⋯.
    pub fn controls(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let queue = self.queue.read(cx);
        let count = queue.items.len() as u64;
        let grouped = queue.grouped;
        let saveable = queue.items.iter().any(|i| i.track_id.is_some());
        let picked = self.selected.len();
        let view = cx.entity();
        let mut controls = ToolbarGroup::new();
        if picked > 0 {
            let (clear, remove) = (view.clone(), view.clone());
            controls = controls
                .child(
                    ToolbarSelection::new(format!("{picked} selected"))
                        .action("queue-deselect", "clear", move |_, _, cx| {
                            clear.update(cx, |this, cx| {
                                this.selected.clear();
                                cx.notify();
                            })
                        })
                        .action("queue-remove", "remove", move |_, _, cx| {
                            remove.update(cx, |this, cx| {
                                let ids = this.selected_ids(cx);
                                this.remove(ids, cx);
                            })
                        }),
                )
                .spacer();
        }
        let (jump, regroup) = (view.clone(), view.clone());
        controls
            .child(
                IconToggle::new(
                    "queue-follow",
                    Icon::JumpToPlaying,
                    if self.following {
                        "following what's playing"
                    } else {
                        "jump to what's playing and follow it"
                    },
                    self.following,
                )
                .disabled(count == 0)
                .on_change(move |_, _, cx| jump.update(cx, |this, cx| this.toggle_follow(cx))),
            )
            .child(
                ToolbarButton::new(
                    "queue-dice",
                    Icon::Reshuffle,
                    "shuffle the rest of the queue",
                )
                // As the Mac refuses it: nothing after the cursor, or shuffle
                // play on, which keeps an order of its own.
                .disabled(!self.shufflable)
                .on_click(|_, _, cx| command(cx, "shuffle queue", |e| e.shuffle_queue())),
            )
            .child(
                IconToggle::new("queue-group", Icon::Album, "group by album", grouped).on_change(
                    move |on, window, cx| {
                        let on = *on;
                        regroup.update(cx, |this, cx| this.set_grouped(on, window, cx))
                    },
                ),
            )
            .child(
                ToolbarButton::new("queue-undo", Icon::Undo, "undo")
                    .on_click(|_, _, cx| command(cx, "undo", |e| e.undo(None))),
            )
            .child(
                ToolbarButton::new("queue-redo", Icon::Redo, "redo")
                    .on_click(|_, _, cx| command(cx, "redo", |e| e.redo(None))),
            )
            .child(more(count, saveable))
            .into_any_element()
    }
}

/// The queue's ⋯: saving it as a playlist, and clearing it. Its tracks are
/// read as "save" is chosen, not each time the toolbar draws.
fn more(count: u64, saveable: bool) -> impl IntoElement {
    ToolbarMenu::new(
        "queue-more",
        ToolbarButton::new("queue-more-trigger", Icon::More, "more"),
        move |dismiss, _, _| {
            let close = dismiss.clone();
            MenuPanel::new()
                .child(
                    MenuRow::new("queue-save", "save as playlist…")
                        .disabled(!saveable)
                        .on_click(move |_, window, cx| {
                            close(window, cx);
                            let ids: Vec<i64> = cx
                                .global::<Mirror>()
                                .queue
                                .read(cx)
                                .items
                                .iter()
                                .filter_map(|i| i.track_id)
                                .collect();
                            save(ids, window, cx);
                        }),
                )
                .child(
                    MenuRow::new("queue-clear", "clear queue…")
                        .disabled(count == 0)
                        .on_click(move |_, window, cx| {
                            dismiss(window, cx);
                            clear(count, window, cx);
                        }),
                )
        },
    )
}

/// Asks for a name, then makes a playlist of the queue's tracks in order.
fn save(track_ids: Vec<i64>, window: &mut Window, cx: &mut App) {
    let n = track_ids.len() as u64;
    crate::components::sheet::name(
        "save the queue as a playlist",
        "save",
        "",
        move |name| {
            vec![
                format!(
                    "A playlist named “{name}” is made of the queue's {}, in order.",
                    crate::library::chrome::count(n, "track", "tracks")
                )
                .into(),
            ]
        },
        window,
        cx,
        move |name, cx| {
            let ids = track_ids.clone();
            command(cx, "save the queue as a playlist", move |e| {
                e.create_playlist(name, ids)
            });
        },
    );
}

/// Asks before emptying the queue, saying how much goes; undo brings it back.
fn clear(count: u64, window: &mut Window, cx: &mut App) {
    crate::components::sheet::confirm(
        crate::components::sheet::Confirm {
            title: "clear the queue".into(),
            verb: "clear".into(),
            destructive: true,
            lines: vec![
                format!(
                    "{} leave the queue, and playback stops.",
                    crate::library::chrome::count(count, "track", "tracks")
                )
                .into(),
            ],
            note: Some("Undo puts them back.".into()),
        },
        window,
        cx,
        |cx| command(cx, "clear queue", |e| e.clear_queue()),
    );
}

impl QueueView {
    /// A drag over the page, wherever the rows' own handlers did not take
    /// it: outside the page the insertion line goes, and below the last row
    /// it moves to the end, however fast the pointer left the row above.
    fn drag_over_page(
        &mut self,
        page: Bounds<Pixels>,
        at: Point<Pixels>,
        count: usize,
        cx: &mut Context<Self>,
    ) {
        let last = count
            .checked_sub(1)
            .and_then(|last| self.list.row_bounds(last))
            .map(|b| b.bottom());
        let Some(wanted) = page_drop(page.contains(&at), last, at.y, count) else {
            return;
        };
        if self.drop_at != wanted {
            self.drop_at = wanted;
            cx.notify();
        }
    }
}

/// Where a drag over the queue's page, outside its rows, puts the insertion
/// point: none outside the page, the end below the last row drawn (at
/// `last`, its bottom edge), and otherwise what the rows said, which is
/// `None` here.
fn page_drop(inside: bool, last: Option<Pixels>, y: Pixels, count: usize) -> Option<Option<usize>> {
    if !inside {
        Some(None)
    } else if last.is_some_and(|bottom| y > bottom) {
        Some(Some(count))
    } else {
        None
    }
}

impl Render for QueueView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let top = self.list.top().item_ix;
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
            // A scroll of the person's own stops following.
            .on_scroll(cx.listener(|this, _: &ScrollEvent, _, cx| {
                if this.following {
                    this.following = false;
                    cx.notify();
                }
            }))
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
            // Moving the view by hand stops following, as a scroll does.
            .on_action(cx.listener(|this, _: &TopOfQueue, _, cx| {
                this.following = false;
                this.list.scroll_to_top();
                this.top = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &EndOfQueue, _, cx| {
                this.following = false;
                if let Some(last) = this.rows(cx).len().checked_sub(1) {
                    this.list.reveal(last);
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
            // A drag that leaves the queue takes its insertion line with it,
            // so a drag released elsewhere leaves none behind; one in the
            // blank space under the last row lands at the end.
            .on_drag_move::<PlayableDrag>(cx.listener(
                move |this, event: &DragMoveEvent<PlayableDrag>, _, cx| {
                    this.drag_over_page(event.bounds, event.event.position, count, cx)
                },
            ))
            .on_drag_move::<DraggedRows>(cx.listener(
                move |this, event: &DragMoveEvent<DraggedRows>, _, cx| {
                    this.drag_over_page(event.bounds, event.event.position, count, cx)
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
            .on_drag_move::<PlaylistDrag>(cx.listener(
                move |this, event: &DragMoveEvent<PlaylistDrag>, _, cx| {
                    this.drag_over_page(event.bounds, event.event.position, count, cx)
                },
            ))
            .on_drop(cx.listener(move |this, dragged: &PlaylistDrag, _, cx| {
                let dest = this.drop_at.unwrap_or(count);
                this.drop_library(&dragged.as_library(), dest, cx);
            }))
            .child(Page::new(self.header(cx)).body(if count == 0 {
                EmptyState::new(Icon::QueueSection, "queue is empty")
                    .detail("Find something to play from albums or artists.")
                    .into_any_element()
            } else {
                let view = view.clone();
                RowList::new(&self.list, move |row, window, cx| {
                    view.update(cx, |this, cx| this.render_row(row, &p, window, cx))
                })
                .named("queue")
                .into_any_element()
            }))
            .koan_context_menu("queue-menu", window, cx, {
                let view = view.clone();
                move |menu, _, cx| row_menu(menu, &view, cx)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Below the last row the drop is at the end, whatever row the pointer
    /// last crossed; outside the page there is none; over the rows, theirs.
    #[test]
    fn the_blank_under_the_rows_is_the_end() {
        let at = |y: f32| Pixels::from(y);
        assert_eq!(page_drop(true, Some(at(200.)), at(260.), 5), Some(Some(5)));
        assert_eq!(page_drop(false, Some(at(200.)), at(260.), 5), Some(None));
        assert_eq!(page_drop(true, Some(at(200.)), at(120.), 5), None);
        // The last row off screen: the list fills the page, nothing is blank.
        assert_eq!(page_drop(true, None, at(260.), 5), None);
    }
}
