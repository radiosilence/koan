//! What a field edits: its text, its selection and what an input method is
//! composing, the history its edits leave, and the caret's blink. The page
//! owns it (`cx.new(|cx| FieldState::new(window, cx))`) and listens to its
//! `InputEvent`s; `TextField` and `TextArea` draw it.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::{
    App, Bounds, ClipboardItem, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ShapedLine,
    SharedString, Subscription, Task, UTF16Selection, Window, point, px,
};
use unicode_segmentation::UnicodeSegmentation as _;

use super::actions::*;
use crate::components::Animated;

/// What a field says to its owner.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    /// The text changed by the person's hand (typing, a paste, an undo, an
    /// input method). `set_value` changes it without saying so.
    Change,
    /// Return pressed: with Command (Control off the Mac) as `secondary`,
    /// with Shift as `shift`. A multi-line field takes a plain Return as a
    /// new line and says only the others.
    PressEnter { secondary: bool, shift: bool },
    /// The field took the keyboard.
    Focus,
    /// The field let the keyboard go.
    Blur,
}

/// How a drag that began with one, two or three clicks extends the
/// selection: by characters, words or lines.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Unit {
    Char,
    Word,
    Line,
}

/// One step of the history: the text and selection before an edit.
#[derive(Clone, Debug)]
struct Snapshot {
    text: String,
    selected: Range<usize>,
    reversed: bool,
}

/// An edit's kind, so consecutive typing undoes as one step, as on the Mac.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Type,
    Delete,
    Other,
    /// An input method committing what it composed: the composition's
    /// first keystroke already took the step.
    Commit,
}

/// How long typing pauses before the next keystroke starts a new undo step.
const COALESCE: Duration = Duration::from_millis(1000);
/// The caret's blink: shown, then hidden, this long each.
const BLINK: Duration = Duration::from_millis(500);
/// How long after an edit or a move the caret stays shown before it blinks.
const BLINK_REST: Duration = Duration::from_millis(500);
/// How many steps the history keeps.
const HISTORY: usize = 200;

/// One laid-out line, as the field last painted it: where it starts in the
/// text, as the text is drawn (bullets for a masked field), and its shape.
#[derive(Clone)]
pub(super) struct Line {
    /// The line's first byte in the drawn text.
    pub start: usize,
    pub shaped: ShapedLine,
}

/// What `TextField` and `TextArea` edit.
pub struct FieldState {
    pub(super) focus: FocusHandle,
    pub(super) text: String,
    pub(super) placeholder: SharedString,
    pub(super) masked: bool,
    /// Lines shown, for a multi-line field; 1 for a single line.
    pub(super) rows: usize,
    pub(super) selected: Range<usize>,
    /// The selection's moving end is its start.
    pub(super) reversed: bool,
    /// What an input method is composing, underlined until it is committed.
    pub(super) marked: Option<Range<usize>>,
    /// As last painted: the lines, where they were drawn, a line's height,
    /// and how far the text is scrolled within the field.
    pub(super) lines: Vec<Line>,
    pub(super) bounds: Option<Bounds<Pixels>>,
    pub(super) line_height: Pixels,
    pub(super) scroll: Point<Pixels>,
    /// A drag under way, by what unit, and the selection it started from.
    drag: Option<(Unit, Range<usize>)>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<(Kind, Instant)>,
    /// The caret's shader uniforms: 0 whether it shows, 4..8 its colour.
    pub(super) caret: Animated,
    blink: Task<()>,
    _focus: [Subscription; 2],
}

impl EventEmitter<InputEvent> for FieldState {}

impl Focusable for FieldState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FieldState {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        let focused = cx.on_focus(&focus, window, |this, _, cx| {
            this.blink_from_shown(cx);
            cx.emit(InputEvent::Focus);
        });
        let blurred = cx.on_blur(&focus, window, |this, _, cx| {
            this.blink = Task::ready(());
            this.caret.set(0, 0.0);
            this.drag = None;
            cx.emit(InputEvent::Blur);
            cx.notify();
        });
        Self {
            focus,
            text: String::new(),
            placeholder: SharedString::default(),
            masked: false,
            rows: 1,
            selected: 0..0,
            reversed: false,
            marked: None,
            lines: Vec::new(),
            bounds: None,
            line_height: px(0.0),
            scroll: Point::default(),
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            caret: Animated::new(),
            blink: Task::ready(()),
            _focus: [focused, blurred],
        }
    }

    /// What it shows while empty, in `muted`.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// The text it starts with.
    pub fn default_value(mut self, value: impl Into<SharedString>) -> Self {
        self.text = value.into().to_string();
        self.selected = self.text.len()..self.text.len();
        self
    }

    /// Its text drawn as bullets, never copied or read aloud: a password.
    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    /// `rows` lines tall, taking Return as a new line: a `TextArea`'s.
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows.max(1);
        self
    }

    pub fn value(&self) -> SharedString {
        SharedString::from(self.text.clone())
    }

    /// Sets the text from the owner, the caret at its end. Says no
    /// `Change`, and starts the history afresh: the person's edits are what
    /// undo walks back.
    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
        let value = if self.multi_line() {
            value.to_string()
        } else {
            value.replace('\n', " ")
        };
        if value == self.text {
            return;
        }
        self.text = value;
        self.selected = self.text.len()..self.text.len();
        self.reversed = false;
        self.marked = None;
        self.undo.clear();
        self.redo.clear();
        self.last_edit = None;
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    /// Empties the field as the person's edit (a clear mark, Escape in a
    /// filter): says `Change`, and can be undone.
    pub fn clean(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit(0..self.text.len(), "", Kind::Other, window, cx);
    }

    /// The whole text replaced by a screen reader's request: an edit of
    /// the person's, which says `Change` and can be undone.
    pub(super) fn set_by_assistant(
        &mut self,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(0..self.text.len(), &value, Kind::Other, window, cx);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    pub(super) fn multi_line(&self) -> bool {
        self.rows > 1
    }

    /// Where typing goes: the selection's moving end.
    pub(super) fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    /// The selection's fixed end.
    fn anchor(&self) -> usize {
        if self.reversed {
            self.selected.end
        } else {
            self.selected.start
        }
    }

    // ── Boundaries ──────────────────────────────────────────────────────

    fn previous_grapheme(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .rev()
            .find_map(|(i, _)| (i < offset).then_some(i))
            .unwrap_or(0)
    }

    fn next_grapheme(&self, offset: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .find_map(|(i, _)| (i > offset).then_some(i))
            .unwrap_or(self.text.len())
    }

    /// The start of the word before `offset`, as Option-Left goes: past
    /// spaces and punctuation, then to the word's start. A masked field
    /// has one word, as a secure field does.
    fn previous_word(&self, offset: usize) -> usize {
        if self.masked {
            return self.line_start(offset);
        }
        self.text[..offset]
            .unicode_word_indices()
            .last()
            .map_or_else(|| self.line_start(offset), |(i, _)| i)
            .max(self.line_start(offset))
    }

    /// The end of the word after `offset`, as Option-Right goes.
    fn next_word(&self, offset: usize) -> usize {
        if self.masked {
            return self.line_end(offset);
        }
        self.text[offset..]
            .unicode_word_indices()
            .next()
            .map_or_else(|| self.line_end(offset), |(i, w)| offset + i + w.len())
            .min(self.line_end(offset))
    }

    /// The word around `offset`, or the run of spaces or the mark it is
    /// in: what a double click selects.
    fn word_at(&self, offset: usize) -> Range<usize> {
        if self.masked {
            return self.line_range(offset);
        }
        let line = self.line_range(offset);
        let mut at = line.start;
        for segment in self.text[line.clone()].split_word_bounds() {
            let end = at + segment.len();
            if offset < end || end == line.end {
                return at..end;
            }
            at = end;
        }
        offset..offset
    }

    fn line_start(&self, offset: usize) -> usize {
        self.text[..offset].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.text[offset..]
            .find('\n')
            .map_or(self.text.len(), |i| offset + i)
    }

    fn line_range(&self, offset: usize) -> Range<usize> {
        self.line_start(offset)..self.line_end(offset)
    }

    /// The offset one line up or down from `offset`, at the same distance
    /// across, as the field last drew it; `None` past the first or last.
    fn vertical(&self, offset: usize, down: bool) -> Option<usize> {
        let line = self.line_of(offset)?;
        let target = if down {
            line.checked_add(1).filter(|l| *l < self.lines.len())?
        } else {
            line.checked_sub(1)?
        };
        let x = self.lines[line]
            .shaped
            .x_for_index(self.to_drawn(offset) - self.lines[line].start);
        let shaped = &self.lines[target];
        let drawn = shaped.start + shaped.shaped.closest_index_for_x(x);
        Some(self.from_drawn(drawn))
    }

    /// Which drawn line holds `offset`.
    fn line_of(&self, offset: usize) -> Option<usize> {
        let drawn = self.to_drawn(offset);
        self.lines
            .iter()
            .rposition(|line| line.start <= drawn)
            .or((!self.lines.is_empty()).then_some(0))
    }

    // ── The drawn text ──────────────────────────────────────────────────

    /// The text as drawn: bullets for each character of a masked field.
    pub(super) fn drawn(&self) -> String {
        if self.masked {
            "•".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }

    /// An offset in the text as an offset in the drawn text.
    pub(super) fn to_drawn(&self, offset: usize) -> usize {
        if self.masked {
            self.text[..offset].chars().count() * '•'.len_utf8()
        } else {
            offset
        }
    }

    /// An offset in the drawn text as an offset in the text.
    pub(super) fn from_drawn(&self, drawn: usize) -> usize {
        if self.masked {
            let chars = drawn / '•'.len_utf8();
            self.text
                .char_indices()
                .nth(chars)
                .map_or(self.text.len(), |(i, _)| i)
        } else {
            drawn.min(self.text.len())
        }
    }

    /// The offset under a point in the window, as the field last drew it.
    pub(super) fn offset_at(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), false) = (self.bounds, self.lines.is_empty()) else {
            return 0;
        };
        let local = position - bounds.origin + self.scroll;
        let row = if self.line_height > px(0.0) {
            (local.y / self.line_height).floor().max(0.0) as usize
        } else {
            0
        };
        let line = &self.lines[row.min(self.lines.len() - 1)];
        let drawn = line.start + line.shaped.closest_index_for_x(local.x.max(px(0.0)));
        self.from_drawn(drawn)
    }

    // ── Moving and selecting ────────────────────────────────────────────

    pub(super) fn move_to(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        self.reversed = false;
        self.moved(window, cx);
    }

    pub(super) fn select_to(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = self.anchor();
        self.reversed = offset < anchor;
        self.selected = anchor.min(offset)..anchor.max(offset);
        self.moved(window, cx);
    }

    fn moved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.last_edit = None;
        self.blink_from_shown(cx);
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    pub(super) fn left(&mut self, _: &MoveLeft, window: &mut Window, cx: &mut Context<Self>) {
        let to = if self.selected.is_empty() {
            self.previous_grapheme(self.cursor())
        } else {
            self.selected.start
        };
        self.move_to(to, window, cx);
    }

    pub(super) fn right(&mut self, _: &MoveRight, window: &mut Window, cx: &mut Context<Self>) {
        let to = if self.selected.is_empty() {
            self.next_grapheme(self.cursor())
        } else {
            self.selected.end
        };
        self.move_to(to, window, cx);
    }

    /// A single line leaves Up to its owner (a list it leads takes the
    /// arrows); a multi-line field moves a line, and past the first goes
    /// to its start.
    pub(super) fn up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        if !self.multi_line() {
            cx.propagate();
            return;
        }
        let to = self.vertical(self.cursor(), false).unwrap_or(0);
        self.move_to(to, window, cx);
    }

    pub(super) fn down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        if !self.multi_line() {
            cx.propagate();
            return;
        }
        let to = self
            .vertical(self.cursor(), true)
            .unwrap_or(self.text.len());
        self.move_to(to, window, cx);
    }

    pub(super) fn select_left(
        &mut self,
        _: &SelectLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.previous_grapheme(self.cursor()), window, cx);
    }

    pub(super) fn select_right(
        &mut self,
        _: &SelectRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.next_grapheme(self.cursor()), window, cx);
    }

    pub(super) fn select_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(self.cursor(), false).unwrap_or(0);
        self.select_to(to, window, cx);
    }

    pub(super) fn select_down(
        &mut self,
        _: &SelectDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to = self
            .vertical(self.cursor(), true)
            .unwrap_or(self.text.len());
        self.select_to(to, window, cx);
    }

    pub(super) fn word_left(
        &mut self,
        _: &MoveWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to(self.previous_word(self.cursor()), window, cx);
    }

    pub(super) fn word_right(
        &mut self,
        _: &MoveWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_to(self.next_word(self.cursor()), window, cx);
    }

    pub(super) fn select_word_left(
        &mut self,
        _: &SelectWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.previous_word(self.cursor()), window, cx);
    }

    pub(super) fn select_word_right(
        &mut self,
        _: &SelectWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.next_word(self.cursor()), window, cx);
    }

    pub(super) fn home(&mut self, _: &MoveHome, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_start(self.cursor()), window, cx);
    }

    pub(super) fn end(&mut self, _: &MoveEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_end(self.cursor()), window, cx);
    }

    pub(super) fn select_home(
        &mut self,
        _: &SelectHome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.line_start(self.cursor()), window, cx);
    }

    pub(super) fn select_end(
        &mut self,
        _: &SelectEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.line_end(self.cursor()), window, cx);
    }

    pub(super) fn start(&mut self, _: &MoveToStart, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
    }

    pub(super) fn finish(&mut self, _: &MoveToEnd, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), window, cx);
    }

    pub(super) fn select_to_start(
        &mut self,
        _: &SelectToStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(0, window, cx);
    }

    pub(super) fn select_to_end(
        &mut self,
        _: &SelectToEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.text.len(), window, cx);
    }

    pub(super) fn select_all(
        &mut self,
        _: &SelectAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = 0..self.text.len();
        self.reversed = false;
        self.moved(window, cx);
    }

    // ── Editing ─────────────────────────────────────────────────────────

    /// Replaces `range` with `with`, the caret after it, as one step of
    /// the history (`kind` decides whether it joins the step before), and
    /// says `Change`.
    fn edit(
        &mut self,
        range: Range<usize>,
        with: &str,
        kind: Kind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let with = if self.multi_line() {
            with.to_string()
        } else {
            with.replace(['\n', '\r'], " ")
        };
        if range.is_empty() && with.is_empty() {
            return;
        }
        self.remember(kind);
        self.text.replace_range(range.clone(), &with);
        let at = range.start + with.len();
        self.selected = at..at;
        self.reversed = false;
        self.marked = None;
        self.changed(window, cx);
    }

    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.blink_from_shown(cx);
        window.notify_text_input_changed(&self.focus);
        cx.emit(InputEvent::Change);
        cx.notify();
    }

    /// Keeps the state before an edit, unless the edit continues the last
    /// one (typing, or deleting, without a pause).
    fn remember(&mut self, kind: Kind) {
        let continues = kind != Kind::Other
            && self
                .last_edit
                .is_some_and(|(last, at)| last == kind && at.elapsed() < COALESCE);
        let joins = kind == Kind::Commit || continues;
        if !joins {
            self.undo.push(Snapshot {
                text: self.text.clone(),
                selected: self.selected.clone(),
                reversed: self.reversed,
            });
            if self.undo.len() > HISTORY {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some((kind, Instant::now()));
    }

    fn restore(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        self.text = snapshot.text;
        self.selected = snapshot.selected;
        self.reversed = snapshot.reversed;
        self.marked = None;
        self.last_edit = None;
        self.changed(window, cx);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            selected: self.selected.clone(),
            reversed: self.reversed,
        }
    }

    pub(super) fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(before) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(before, window, cx);
        }
    }

    pub(super) fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(after) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(after, window, cx);
        }
    }

    pub(super) fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected.is_empty() {
            self.previous_grapheme(self.cursor())..self.cursor()
        } else {
            self.selected.clone()
        };
        self.edit(range, "", Kind::Delete, window, cx);
    }

    pub(super) fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected.is_empty() {
            self.cursor()..self.next_grapheme(self.cursor())
        } else {
            self.selected.clone()
        };
        self.edit(range, "", Kind::Delete, window, cx);
    }

    pub(super) fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = if self.selected.is_empty() {
            self.previous_word(self.cursor())..self.cursor()
        } else {
            self.selected.clone()
        };
        self.edit(range, "", Kind::Other, window, cx);
    }

    pub(super) fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = if self.selected.is_empty() {
            self.cursor()..self.next_word(self.cursor())
        } else {
            self.selected.clone()
        };
        self.edit(range, "", Kind::Other, window, cx);
    }

    pub(super) fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = if self.selected.is_empty() {
            self.line_start(self.cursor())..self.cursor()
        } else {
            self.selected.clone()
        };
        self.edit(range, "", Kind::Other, window, cx);
    }

    pub(super) fn delete_to_line_end(
        &mut self,
        _: &DeleteToLineEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = if self.selected.is_empty() {
            self.cursor()..self.line_end(self.cursor())
        } else {
            self.selected.clone()
        };
        self.edit(range, "", Kind::Other, window, cx);
    }

    pub(super) fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        self.returned(false, false, window, cx);
    }

    pub(super) fn shift_enter(
        &mut self,
        _: &ShiftEnter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.returned(false, true, window, cx);
    }

    pub(super) fn secondary_enter(
        &mut self,
        _: &SecondaryEnter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.returned(true, false, window, cx);
    }

    /// Return: a new line in a multi-line field, unless with Command;
    /// otherwise said to the owner.
    fn returned(
        &mut self,
        secondary: bool,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.multi_line() && !secondary {
            self.edit(self.selected.clone(), "\n", Kind::Other, window, cx);
            return;
        }
        cx.emit(InputEvent::PressEnter { secondary, shift });
    }

    pub(super) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        // A masked field's text stays in it.
        if self.masked || self.selected.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.text[self.selected.clone()].to_string(),
        ));
    }

    pub(super) fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.masked || self.selected.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.text[self.selected.clone()].to_string(),
        ));
        self.edit(self.selected.clone(), "", Kind::Other, window, cx);
    }

    pub(super) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.edit(self.selected.clone(), &text, Kind::Other, window, cx);
        }
    }

    pub(super) fn character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    // ── The pointer ─────────────────────────────────────────────────────

    /// A press: one click places the caret (Shift extends to it), two
    /// select the word, three the line; a drag from there extends by the
    /// same unit.
    pub(super) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }
        window.focus(&self.focus, cx);
        let at = self.offset_at(event.position);
        let unit = match event.click_count {
            0 | 1 => Unit::Char,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        match unit {
            Unit::Char if event.modifiers.shift => self.select_to(at, window, cx),
            Unit::Char => self.move_to(at, window, cx),
            Unit::Word => {
                let word = self.word_at(at);
                self.selected = word;
                self.reversed = false;
                self.moved(window, cx);
            }
            Unit::Line => {
                self.selected = self.line_range(at);
                self.reversed = false;
                self.moved(window, cx);
            }
        }
        self.drag = Some((unit, self.selected.clone()));
    }

    pub(super) fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((unit, from)) = self.drag.clone() else {
            return;
        };
        if !event.dragging() {
            self.drag = None;
            return;
        }
        let at = self.offset_at(event.position);
        let reach = match unit {
            Unit::Char => at..at,
            Unit::Word => self.word_at(at),
            Unit::Line => self.line_range(at),
        };
        let (start, end) = (from.start.min(reach.start), from.end.max(reach.end));
        // The moving end follows the pointer: before the start, it is the
        // selection's start.
        self.reversed = reach.start < from.start;
        let selected = start..end;
        if selected != self.selected {
            self.selected = selected;
            self.moved(window, cx);
        }
    }

    pub(super) fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag = None;
    }

    // ── The caret ───────────────────────────────────────────────────────

    /// Shows the caret now and starts its blink after a rest, as on the
    /// Mac: steady while typing or moving, blinking once still. Only a
    /// focused field blinks; a blink presents the last frame again with
    /// the caret's uniform changed, and draws nothing.
    fn blink_from_shown(&mut self, cx: &mut Context<Self>) {
        self.caret.set(0, 1.0);
        self.blink = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BLINK_REST).await;
            let mut shown = true;
            loop {
                shown = !shown;
                let alive = this
                    .update(cx, |this, _| {
                        this.caret.set(0, if shown { 1.0 } else { 0.0 })
                    })
                    .is_ok();
                if !alive {
                    return;
                }
                cx.background_executor().timer(BLINK).await;
            }
        });
    }
}

impl EntityInputHandler for FieldState {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.from_utf16_range(&range);
        actual.replace(self.to_utf16_range(&range));
        Some(self.text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.to_utf16_range(&self.selected),
            reversed: self.reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = self.from_utf16_range(&range);
        self.reversed = false;
        self.moved(window, cx);
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.text.encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.to_utf16_range(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        window.notify_text_input_changed(&self.focus);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.from_utf16_range(&range))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        // What the keyboard types joins the typing before it; a word an
        // input method commits is one step.
        let kind = if self.marked.is_some() {
            Kind::Commit
        } else if text.chars().count() == 1 {
            Kind::Type
        } else {
            Kind::Other
        };
        self.edit(range, text, kind, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.from_utf16_range(&range))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        // The first keystroke of a composition is remembered; the rest of
        // it, until committed, is the same step.
        if self.marked.is_none() {
            self.remember(Kind::Other);
        }
        self.text.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        let start16 = self.to_utf16(range.start);
        let len16 = text.encode_utf16().count();
        self.selected = selected
            .map(|s| {
                self.from_utf16_range(&(start16 + s.start.min(len16)..start16 + s.end.min(len16)))
            })
            .unwrap_or(range.start + text.len()..range.start + text.len());
        self.reversed = false;
        self.changed(window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.from_utf16_range(&range);
        let bounds = self.bounds?;
        let line = self.line_of(range.start)?;
        let shaped = &self.lines[line];
        let x = |offset: usize| {
            shaped
                .shaped
                .x_for_index(self.to_drawn(offset).saturating_sub(shaped.start))
        };
        let top = bounds.top() + self.line_height * line as f32 - self.scroll.y;
        let left = bounds.left() - self.scroll.x;
        Some(Bounds::from_corners(
            point(left + x(range.start), top),
            point(left + x(range.end.max(range.start)), top + self.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.bounds?;
        Some(self.to_utf16(self.offset_at(position)))
    }
}

impl FieldState {
    fn to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .encode_utf16()
            .count()
    }

    fn from_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        for (i, c) in self.text.char_indices() {
            if utf16 >= offset {
                return i;
            }
            utf16 += c.len_utf16();
        }
        self.text.len()
    }

    fn to_utf16_range(&self, range: &Range<usize>) -> Range<usize> {
        self.to_utf16(range.start)..self.to_utf16(range.end)
    }

    fn from_utf16_range(&self, range: &Range<usize>) -> Range<usize> {
        self.from_utf16(range.start)..self.from_utf16(range.end)
    }

    /// The selection in characters, for a screen reader, as the text
    /// counts them; the caret's offset when nothing is selected.
    pub(super) fn selection_in_chars(&self) -> (usize, usize) {
        let chars = |offset: usize| self.text[..offset].chars().count();
        (chars(self.anchor()), chars(self.cursor()))
    }
}
