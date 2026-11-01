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

use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{EntryStatus, QueueItem};

use crate::covers::{self, CoversHandle};
use crate::engine::command;
use crate::mirror::{Mirror, Queue};
use crate::navigator::{Navigator, Route};
use crate::paint::Paint;
use crate::theme::{self, Icon, KoanIcon, KoanStyled, Space, TypeRole};
use rows::Row;

actions!(
    queue,
    [
        PlaySelected,
        RemoveSelected,
        SelectPrevious,
        SelectNext,
        ExtendPrevious,
        ExtendNext,
        SelectAll,
        Undo,
        Redo,
    ]
);

pub const CONTEXT: &str = "Queue";

pub fn bindings() -> Vec<KeyBinding> {
    let ctx = Some(CONTEXT);
    vec![
        KeyBinding::new("enter", PlaySelected, ctx),
        KeyBinding::new("backspace", RemoveSelected, ctx),
        KeyBinding::new("delete", RemoveSelected, ctx),
        KeyBinding::new("up", SelectPrevious, ctx),
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("shift-up", ExtendPrevious, ctx),
        KeyBinding::new("shift-down", ExtendNext, ctx),
        KeyBinding::new(secondary!("a"), SelectAll, ctx),
        KeyBinding::new(secondary!("z"), Undo, ctx),
        KeyBinding::new(secondary!("shift-z"), Redo, ctx),
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
    _subscriptions: [Subscription; 4],
}

impl QueueView {
    pub fn new(nav: &Entity<Navigator>, cx: &mut Context<Self>) -> Self {
        let queue = cx.global::<Mirror>().queue.clone();
        let covers = cx.global::<CoversHandle>().0.clone();
        let subscriptions = [
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
            _subscriptions: subscriptions,
        };
        view.lay_out(cx);
        view
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

    fn remove_selected(&mut self, _: &RemoveSelected, _: &mut Window, cx: &mut Context<Self>) {
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

    fn render_row(&mut self, row: usize, p: &Paint, cx: &mut Context<Self>) -> AnyElement {
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
        let queue = self.queue.read(cx);
        let (height, line, id) = match r {
            Row::Heading { first, len } => {
                let run = &queue.items[first..first + len];
                let line = heading(
                    &run[0],
                    detail(run),
                    sleeve.unwrap_or_else(|| div().into_any_element()),
                    p,
                );
                // Named by what the row is, so hover and drag state stay with
                // it when the queue moves under the pointer.
                let id = SharedString::from(format!("h:{}", queue.base[first].queue_item_id));
                (HEADING, line, id)
            }
            Row::Track { ix, grouped } => {
                let height = if grouped { TRACK } else { SINGLE };
                let line = track(&queue.items[ix], grouped, sleeve, p);
                (
                    height,
                    line,
                    SharedString::from(queue.base[ix].queue_item_id.clone()),
                )
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
            .id(ElementId::Name(id))
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
            .on_drop(cx.listener(move |this, dragged: &DraggedRows, _, cx| {
                let dest = this.drop_at.unwrap_or(row);
                this.drop_rows(dragged, dest, cx);
            }))
            .child(line)
            .into_any_element()
    }
}

fn row_menu(menu: PopupMenu, view: &Entity<QueueView>, cx: &mut App) -> PopupMenu {
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
    menu.item(PopupMenuItem::new("play").on_click(move |_, _, cx| {
        if let Some(id) = first.clone() {
            command(cx, "play", move |e| e.play(id));
        }
    }))
    .item(PopupMenuItem::new(remove_label).on_click(move |_, _, cx| {
        let ids = ids.clone();
        remove.update(cx, |this, cx| this.remove(ids, cx));
    }))
    .separator()
    .item(PopupMenuItem::new("undo").on_click(|_, _, cx| command(cx, "undo", |e| e.undo(None))))
    .item(PopupMenuItem::new("redo").on_click(|_, _, cx| command(cx, "redo", |e| e.redo(None))))
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

fn heading(first: &QueueItem, detail: String, sleeve: AnyElement, p: &Paint) -> Div {
    let artist = if first.album_artist.is_empty() {
        &first.artist
    } else {
        &first.album_artist
    };
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
                .child(line(first.album.clone(), TypeRole::Body, p.strong, p))
                .child(line(artist.clone(), TypeRole::Meta, p.ink, p))
                .child(line(detail, TypeRole::Fine, p.muted, p)),
        )
}

fn track(item: &QueueItem, grouped: bool, sleeve: Option<AnyElement>, p: &Paint) -> Div {
    let playing = item.status == EntryStatus::Playing;
    let title_color = match item.status {
        EntryStatus::Playing => p.accent_text,
        EntryStatus::Played => p.muted,
        EntryStatus::Failed => p.bad,
        EntryStatus::Queued | EntryStatus::Downloading | EntryStatus::PriorityPending => p.ink,
    };
    // A state is a glyph, as the theme draws states; the plain variant keeps
    // a word where a glyph would be.
    let mark = match item.status {
        EntryStatus::Playing => Some((Icon::PlayMark, "playing", p.accent_text)),
        EntryStatus::Downloading => Some((Icon::Downloads, "loading", p.muted)),
        EntryStatus::PriorityPending => Some((Icon::Pending, "next", p.muted)),
        EntryStatus::Failed => Some((Icon::Failure, "failed", p.bad)),
        EntryStatus::Queued | EntryStatus::Played => None,
    };
    let leading = if grouped {
        let number = item.track_number.map(|n| n.to_string()).unwrap_or_default();
        div()
            .w(px(24.))
            .flex_shrink_0()
            .flex()
            .justify_end()
            .text_role(TypeRole::Meta, &p.look)
            .text_color(if playing { p.accent_text } else { p.muted })
            .child(number)
            .into_any_element()
    } else {
        sleeve.unwrap_or_else(|| div().into_any_element())
    };
    // A grouped row names its artists only where they differ from the
    // record's; a row standing alone names its artist and record.
    let secondary = if grouped {
        item.credit_differs.then(|| item.artist.clone())
    } else if item.album.is_empty() {
        Some(item.artist.clone())
    } else {
        Some(format!("{} · {}", item.artist, item.album))
    };
    div()
        .flex()
        .flex_1()
        .min_w(px(0.))
        .items_center()
        .gap(p.space(Space::M))
        .child(
            div()
                .w(px(14.))
                .flex_shrink_0()
                .flex()
                .justify_center()
                .text_role(TypeRole::Fine, &p.look)
                .children(
                    mark.map(|(icon, word, color)| glyph(icon, word, TypeRole::Fine, color, p)),
                ),
        )
        .child(leading)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .child(line(item.title.clone(), TypeRole::Body, title_color, p))
                .children(secondary.map(|s| line(s, TypeRole::Meta, p.muted, p))),
        )
        .when(!item.on_disk && item.on_server, |d| {
            d.child(
                div()
                    .flex_shrink_0()
                    .text_role(TypeRole::Fine, &p.look)
                    .child(glyph(Icon::Cloud, "server", TypeRole::Fine, p.muted, p)),
            )
        })
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

impl Focusable for QueueView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for QueueView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::of(window, cx);
        let top = self.list.logical_scroll_top().item_ix;
        self.keep_top(top, cx);
        let count = self.rows(cx).len();
        let view = cx.entity();
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
            // Below the last row: the end of the queue.
            .on_drop(cx.listener(move |this, dragged: &DraggedRows, _, cx| {
                let dest = this.drop_at.unwrap_or(count);
                this.drop_rows(dragged, dest, cx);
            }))
            .when(count == 0, |d| {
                d.items_center().justify_center().child(
                    div()
                        .text_role(TypeRole::Body, &p.look)
                        .text_color(p.muted)
                        .child("the queue is empty"),
                )
            })
            .when(count > 0, |d| {
                let view = view.clone();
                d.child(
                    list(self.list.clone(), move |row, _, cx| {
                        view.update(cx, |this, cx| this.render_row(row, &p, cx))
                    })
                    .flex_1(),
                )
            })
            .context_menu({
                let view = view.clone();
                move |menu, _, cx| row_menu(menu, &view, cx)
            })
    }
}
