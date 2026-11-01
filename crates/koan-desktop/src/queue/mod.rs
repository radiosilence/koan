//! The queue: the main stage, grouped by record.
//!
//! The view outlives page switches (the shell holds it for the life of the
//! window), so its selection and scroll position survive a visit elsewhere.
//! Rows are laid out again only when the queue is sent whole; a patch (a
//! track change, a download landing) redraws the rows on screen, which read
//! their item as it is now. A patch never changes a row's height or place, so
//! the list keeps its measurements across one.

mod rows;

use std::collections::{HashMap, HashSet};

use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::{EntryStatus, QueueItem};

use crate::covers::{CoversHandle, sleeve};
use crate::engine::command;
use crate::mirror::{Mirror, Queue};
use crate::theme::{self, Palette, space, text};
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
        KeyBinding::new("secondary-a", SelectAll, ctx),
        KeyBinding::new("secondary-z", Undo, ctx),
        KeyBinding::new("secondary-shift-z", Redo, ctx),
    ]
}

/// Row heights, as the Mac's `RowMetrics` gives them: a track is a line of
/// text and its padding, a record's heading carries its sleeve and a second
/// line, a track standing alone carries its own sleeve.
const PADDING: Pixels = px(4.);
const TRACK: Pixels = px(34. + 8.);
const HEADING_SLEEVE: Pixels = px(32.);
const HEADING: Pixels = px(32. + 12. + 8.);
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
    label: SharedString,
}

struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        div()
            .px(space::M)
            .py(space::XS)
            .bg(p.surface)
            .border_1()
            .border_color(p.rule)
            .text_size(text::META)
            .text_color(p.ink)
            .child(self.0.clone())
    }
}

pub struct QueueView {
    queue: Entity<Queue>,
    focus: FocusHandle,
    list: ListState,
    rows: Vec<Row>,
    /// The track row of each queue item, for finding a `RowRef` again.
    track_rows: HashMap<String, usize>,
    /// The `Queue::whole` the rows were laid out from.
    whole: u64,
    /// Picked queue items. A heading reads as picked when its whole run is.
    selected: HashSet<String>,
    /// Where a shift-click or shift-arrow extends from, and where the arrows
    /// move from.
    anchor: Option<RowRef>,
    cursor: Option<RowRef>,
    _subscriptions: [Subscription; 2],
}

impl QueueView {
    pub fn new(cx: &mut Context<Self>) -> Self {
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
        ];
        let mut view = Self {
            queue,
            focus: cx.focus_handle(),
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            rows: Vec::new(),
            track_rows: HashMap::new(),
            whole: 0,
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            _subscriptions: subscriptions,
        };
        view.lay_out(cx);
        view
    }

    fn lay_out(&mut self, cx: &mut Context<Self>) {
        let queue = self.queue.read(cx);
        self.whole = queue.whole;
        self.rows = rows::build(&queue.base);
        self.track_rows = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(row, r)| match r {
                Row::Track { ix, .. } => Some((queue.base[*ix].queue_item_id.clone(), row)),
                Row::Heading { .. } => None,
            })
            .collect();
        self.selected.retain(|id| queue.at.contains_key(id));
        self.list.reset(self.rows.len());
        if self
            .anchor
            .as_ref()
            .is_some_and(|r| self.row_of(r).is_none())
        {
            self.anchor = None;
        }
        if self
            .cursor
            .as_ref()
            .is_some_and(|r| self.row_of(r).is_none())
        {
            self.cursor = None;
        }
    }

    fn row_of(&self, r: &RowRef) -> Option<usize> {
        let track = *self.track_rows.get(&r.item)?;
        if !r.heading {
            return Some(track);
        }
        let above = track.checked_sub(1)?;
        matches!(self.rows[above], Row::Heading { first, .. } if self.rows[track] == (Row::Track { ix: first, grouped: true }))
            .then_some(above)
    }

    fn row_ref(&self, row: usize, cx: &App) -> RowRef {
        let base = &self.queue.read(cx).base;
        match self.rows[row] {
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

    fn row_items<'a>(&self, row: usize, base: &'a [QueueItem]) -> impl Iterator<Item = &'a String> {
        self.rows[row].items().map(|ix| &base[ix].queue_item_id)
    }

    fn is_selected(&self, row: usize, cx: &App) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        let base = &self.queue.read(cx).base;
        self.row_items(row, base)
            .all(|id| self.selected.contains(id))
    }

    /// The picked queue items, in queue order.
    fn selected_ids(&self, cx: &App) -> Vec<String> {
        if self.selected.is_empty() {
            return Vec::new();
        }
        self.queue
            .read(cx)
            .base
            .iter()
            .filter(|item| self.selected.contains(&item.queue_item_id))
            .map(|item| item.queue_item_id.clone())
            .collect()
    }

    /// The queue items a row action applies to: the selection when the row
    /// is in it, the row alone otherwise.
    fn acting_on(&self, row: usize, cx: &App) -> Vec<String> {
        if self.is_selected(row, cx) {
            self.selected_ids(cx)
        } else {
            let base = &self.queue.read(cx).base;
            self.row_items(row, base).cloned().collect()
        }
    }

    fn select_only(&mut self, row: usize, cx: &App) {
        let base = &self.queue.read(cx).base;
        self.selected = self.row_items(row, base).cloned().collect();
        let r = self.row_ref(row, cx);
        self.anchor = Some(r.clone());
        self.cursor = Some(r);
    }

    fn select_range(&mut self, to: usize, cx: &App) {
        let from = self
            .anchor
            .as_ref()
            .and_then(|r| self.row_of(r))
            .unwrap_or(to);
        let base = &self.queue.read(cx).base;
        self.selected = (from.min(to)..=from.max(to))
            .flat_map(|row| self.row_items(row, base).cloned())
            .collect();
        if self.anchor.is_none() {
            self.anchor = Some(self.row_ref(to, cx));
        }
        self.cursor = Some(self.row_ref(to, cx));
    }

    fn toggle(&mut self, row: usize, cx: &App) {
        let base = &self.queue.read(cx).base;
        let ids: Vec<String> = self.row_items(row, base).cloned().collect();
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
        let base = &self.queue.read(cx).base;
        let Some(id) = self.row_items(row, base).next().cloned() else {
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
        let Some(last) = self.rows.len().checked_sub(1) else {
            return;
        };
        let to = match self.cursor.as_ref().and_then(|r| self.row_of(r)) {
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
        if dragged.whole != self.whole || dragged.row >= self.rows.len() {
            return;
        }
        let moving = self.acting_on(dragged.row, cx);
        let base = &self.queue.read(cx).base;
        let Some((target, after)) = rows::drop_target(&self.rows, dest, &moving, base) else {
            return;
        };
        command(cx, "move in queue", move |e| {
            e.move_in_queue(moving, target, after)
        });
    }

    fn drag_label(&self, row: usize, cx: &App) -> SharedString {
        let base = &self.queue.read(cx).base;
        let count = if self.is_selected(row, cx) {
            self.selected.len()
        } else {
            self.rows[row].items().len()
        };
        if count == 1 {
            base[self.rows[row].items().start].title.clone().into()
        } else {
            format!("{count} tracks").into()
        }
    }

    fn render_row(&mut self, row: usize, cx: &mut Context<Self>) -> AnyElement {
        if row >= self.rows.len() {
            return div().into_any_element();
        }
        let p = theme::palette(cx);
        let selected = self.is_selected(row, cx);
        let (height, line) = match self.rows[row] {
            Row::Heading { first, .. } => {
                let item = self.queue.read(cx).items[first].clone();
                (HEADING, heading(&item, &p, cx))
            }
            Row::Track { ix, grouped } => {
                let item = self.queue.read(cx).items[ix].clone();
                let height = if grouped { TRACK } else { SINGLE };
                (height, track(&item, grouped, &p, cx))
            }
        };
        let dragged = DraggedRows {
            row,
            whole: self.whole,
            label: self.drag_label(row, cx),
        };
        let view = cx.entity();
        div()
            .id(("queue-row", row))
            .h(height)
            .w_full()
            .flex()
            .items_center()
            .px(space::L)
            .py(PADDING)
            .gap(space::M)
            .when(selected, |d| d.bg(p.surface))
            .when(!selected, |d| d.hover(|s| s.bg(p.hover.opacity(0.3))))
            .drag_over::<DraggedRows>(move |s, _, _, _| s.border_t_1().border_color(p.accent))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.click(row, event, cx);
            }))
            .on_drag(dragged, |dragged, _, _, cx| {
                cx.new(|_| DragPreview(dragged.label.clone()))
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedRows, _, cx| {
                this.drop_rows(dragged, row, cx);
            }))
            .child(line)
            .context_menu(move |menu, _, cx| row_menu(menu, &view, row, cx))
            .into_any_element()
    }
}

fn row_menu(menu: PopupMenu, view: &Entity<QueueView>, row: usize, cx: &mut App) -> PopupMenu {
    let this = view.read(cx);
    if row >= this.rows.len() {
        return menu;
    }
    let ids = this.acting_on(row, cx);
    let heading =
        matches!(this.rows[row], Row::Heading { .. }) && ids.len() == this.rows[row].items().len();
    let remove_label = match (heading, ids.len()) {
        (true, _) => "remove record".to_string(),
        (false, 1) => "remove".to_string(),
        (false, n) => format!("remove {n} tracks"),
    };
    let play = view.clone();
    let remove = view.clone();
    menu.item(PopupMenuItem::new("play").on_click(move |_, _, cx| {
        play.read(cx).play_row(row, cx);
    }))
    .item(PopupMenuItem::new(remove_label).on_click(move |_, _, cx| {
        let ids = ids.clone();
        remove.update(cx, |this, cx| this.remove(ids, cx));
    }))
    .separator()
    .item(PopupMenuItem::new("undo").on_click(|_, _, cx| command(cx, "undo", |e| e.undo(None))))
    .item(PopupMenuItem::new("redo").on_click(|_, _, cx| command(cx, "redo", |e| e.redo(None))))
}

fn line(content: impl Into<SharedString>, size: Pixels, color: Hsla) -> Div {
    div()
        .min_w(px(0.))
        .truncate()
        .text_size(size)
        .text_color(color)
        .child(content.into())
}

fn heading(first: &QueueItem, p: &Palette, cx: &mut App) -> Div {
    let artist = if first.album_artist.is_empty() {
        &first.artist
    } else {
        &first.album_artist
    };
    let detail = match &first.year {
        Some(year) => format!("{artist} · {year}"),
        None => artist.clone(),
    };
    div()
        .flex()
        .flex_1()
        .min_w(px(0.))
        .items_center()
        .gap(space::M)
        .child(sleeve(first.album_id, HEADING_SLEEVE, cx))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.))
                .child(line(first.album.clone(), text::BODY, p.strong))
                .child(line(detail, text::META, p.muted)),
        )
}

fn track(item: &QueueItem, grouped: bool, p: &Palette, cx: &mut App) -> Div {
    let playing = item.status == EntryStatus::Playing;
    let title_color = match item.status {
        EntryStatus::Playing => p.accent,
        EntryStatus::Played => p.muted,
        EntryStatus::Failed => p.bad,
        EntryStatus::Queued | EntryStatus::Downloading | EntryStatus::PriorityPending => p.ink,
    };
    let (mark, mark_color) = match item.status {
        EntryStatus::Playing => ("▶", p.accent),
        EntryStatus::Downloading => ("↓", p.muted),
        EntryStatus::PriorityPending => ("…", p.muted),
        EntryStatus::Failed => ("!", p.bad),
        EntryStatus::Queued | EntryStatus::Played => ("", p.muted),
    };
    let leading = if grouped {
        let number = item.track_number.map(|n| n.to_string()).unwrap_or_default();
        div()
            .w(px(24.))
            .flex_shrink_0()
            .flex()
            .justify_end()
            .text_size(text::META)
            .text_color(if playing { p.accent } else { p.muted })
            .child(number)
            .into_any_element()
    } else {
        sleeve(item.album_id, SINGLE_SLEEVE, cx)
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
        .gap(space::M)
        .child(
            div()
                .w(px(14.))
                .flex_shrink_0()
                .text_size(text::FINE)
                .text_color(mark_color)
                .child(mark),
        )
        .child(leading)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .child(line(item.title.clone(), text::BODY, title_color))
                .children(secondary.map(|s| line(s, text::META, p.muted))),
        )
        .when(!item.on_disk && item.on_server, |d| {
            d.child(
                div()
                    .flex_shrink_0()
                    .text_size(text::FINE)
                    .text_color(p.muted)
                    .child("☁"),
            )
        })
        .children(item.codec.clone().map(|codec| {
            div()
                .w(px(48.))
                .flex_shrink_0()
                .text_size(text::FINE)
                .text_color(p.muted)
                .child(codec.to_lowercase())
        }))
        .child(
            div()
                .w(px(44.))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_size(text::META)
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let count = self.rows.len();
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
            // Below the last row: the end of the queue.
            .on_drop(cx.listener(move |this, dragged: &DraggedRows, _, cx| {
                this.drop_rows(dragged, count, cx);
            }))
            .when(count == 0, |d| {
                d.items_center().justify_center().child(
                    div()
                        .text_size(text::BODY)
                        .text_color(p.muted)
                        .child("the queue is empty"),
                )
            })
            .when(count > 0, |d| {
                d.child(
                    list(self.list.clone(), move |row, _, cx| {
                        view.update(cx, |this, cx| this.render_row(row, cx))
                    })
                    .flex_1(),
                )
            })
    }
}
