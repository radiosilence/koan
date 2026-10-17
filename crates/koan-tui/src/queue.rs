//! The queue page: what is queued, in album runs or a row per track, with
//! what each track is doing, where its bytes are, and the links to its album
//! and artist, as the apps' queue lays it out.

use std::collections::{HashMap, HashSet};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{QueueEntry, QueueEntryStatus, QueueItemId};

use super::app::{HoverZone, Mode};
use super::rows::{self, Avail, Link, Marker, Status};
use super::theme::Theme;

/// Where the row's `\u{22ef}` button sits: left of the scrollbar.
pub fn menu_button_x(area: Rect) -> u16 {
    area.x + area.width.saturating_sub(3)
}

/// How to move the entries in `moving` so that, together and in queue order,
/// they start at `start`. `None` when that is where they already are.
///
/// Placed against the entries that are not moving, whose order no move of
/// these changes: asking again from a snapshot taken before the last move
/// landed asks for the same thing.
pub fn placement(
    entries: &[QueueEntry],
    moving: &HashSet<QueueItemId>,
    start: usize,
) -> Option<PlayerCommand> {
    let (ids, rest): (Vec<_>, Vec<_>) = entries
        .iter()
        .map(|e| e.id)
        .partition(|id| moving.contains(id));
    let start = start.min(rest.len());
    let at: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| moving.contains(&e.id))
        .map(|(i, _)| i)
        .collect();
    if ids.is_empty() || rest.is_empty() || at.iter().copied().eq(start..start + ids.len()) {
        return None;
    }
    let (target, after) = match start.checked_sub(1) {
        Some(before) => (rest[before], true),
        None => (rest[0], false),
    };
    Some(PlayerCommand::MoveItemsInPlaylist { ids, target, after })
}

/// One line of the queue as drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// An album heading over the run `first..=last`.
    Heading { first: usize, last: usize },
    /// A track under its heading.
    Track(usize),
    /// A track standing alone: ungrouped, a single, or without an album. It
    /// says who it is by and what it is from itself.
    Single(usize),
}

/// The queue's lines: built when the queue changes or is regrouped, and read
/// by the drawing and every hit test until then.
#[derive(Debug, Default, Clone)]
pub struct Layout {
    pub grouped: bool,
    lines: Vec<Line>,
    /// The line each entry is drawn on.
    line_of: Vec<usize>,
    /// The whole queue's length.
    pub total_ms: u64,
}

impl Layout {
    /// Whether it was built from these entries' count; a queue changed under
    /// it is drawn from a layout built on the spot instead.
    fn fits(&self, entries: &[QueueEntry]) -> bool {
        self.line_of.len() == entries.len()
    }
}

impl Layout {
    /// Contiguous runs of one album by one album artist, never sorted or
    /// merged. A run of one that is its own album, a single, is a row of its
    /// own, as is a track without an album.
    pub fn build(entries: &[QueueEntry], grouped: bool) -> Self {
        let mut lines = Vec::with_capacity(entries.len() + entries.len() / 8);
        let mut line_of = Vec::with_capacity(entries.len());
        fn key(e: &QueueEntry) -> (&str, &str) {
            (&e.album_artist, &e.album)
        }
        let mut i = 0;
        while i < entries.len() {
            let e = &entries[i];
            if !grouped || e.album.is_empty() {
                line_of.push(lines.len());
                lines.push(Line::Single(i));
                i += 1;
                continue;
            }
            let last = (i..entries.len())
                .take_while(|&j| key(&entries[j]) == key(e))
                .last()
                .unwrap_or(i);
            if last == i && e.title.trim().to_lowercase() == e.album.trim().to_lowercase() {
                line_of.push(lines.len());
                lines.push(Line::Single(i));
            } else {
                lines.push(Line::Heading { first: i, last });
                for j in i..=last {
                    line_of.push(lines.len());
                    lines.push(Line::Track(j));
                }
            }
            i = last + 1;
        }
        Self {
            grouped,
            lines,
            line_of,
            total_ms: entries.iter().filter_map(|e| e.duration_ms).sum(),
        }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// The line entry `idx` is drawn on.
    pub fn line_of(&self, idx: usize) -> Option<usize> {
        self.line_of.get(idx).copied()
    }
}

/// What the library knows of the queue's tracks: read when the queue
/// changes, never per frame.
#[derive(Debug, Default, Clone)]
pub struct Facts {
    /// (on the server, on this device) for each library track.
    pub sources: HashMap<i64, (bool, bool)>,
    /// What the queue is, when it is exactly a playlist or an album.
    pub lock: Option<String>,
}

impl Facts {
    pub fn read(
        entries: &[QueueEntry],
        state: &koan_core::player::state::SharedPlayerState,
    ) -> Self {
        use koan_core::db::queries;
        let Ok(db) = koan_core::db::pool::shared().get() else {
            return Self::default();
        };
        let ids: Vec<i64> = entries.iter().filter_map(|e| e.db_id).collect();
        let lock = match koan_core::playlists::queue_lock(&db, state) {
            Some(koan_core::playlists::QueueLock::Playlist(id)) => {
                queries::get_playlist(&db.conn, id)
                    .ok()
                    .flatten()
                    .map(|p| p.name)
            }
            Some(koan_core::playlists::QueueLock::Album(id)) => queries::get_album(&db.conn, id)
                .ok()
                .flatten()
                .map(|a| a.title),
            None => None,
        };
        Self {
            sources: queries::sources_for_tracks(&db.conn, &ids).unwrap_or_default(),
            lock,
        }
    }
}

/// The controls in the queue's heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Scroll to what is playing.
    Playing,
    /// Group by album (`true`) or a row per track.
    Grouped(bool),
    Undo,
    Redo,
    /// Save the queue as a playlist.
    Save,
    ClearQueue,
    Deselect,
    RemoveSelected,
}

/// The heading's controls and where each sits, right to left from the edge;
/// those that do not fit are left out from the left.
pub fn controls(
    area: Rect,
    selected: usize,
    grouped: bool,
    left_end: u16,
) -> Vec<(Rect, Control, String)> {
    let mut items: Vec<(String, Control)> = Vec::new();
    if selected > 0 {
        items.push(("remove".into(), Control::RemoveSelected));
        items.push(("deselect".into(), Control::Deselect));
    }
    items.extend([
        ("playing".into(), Control::Playing),
        (
            if grouped { "by album" } else { "by track" }.into(),
            Control::Grouped(!grouped),
        ),
        ("undo".into(), Control::Undo),
        ("redo".into(), Control::Redo),
        ("save".into(), Control::Save),
        ("clear".into(), Control::ClearQueue),
    ]);
    let mut x = area.x + area.width;
    let mut out = Vec::new();
    for (label, control) in items.into_iter().rev() {
        let w = label.width() as u16;
        if x < left_end + w + 2 {
            break;
        }
        x -= w;
        out.push((Rect::new(x, area.y, w, 1), control, label));
        x -= 2;
    }
    out
}

pub struct QueueView<'a> {
    entries: &'a [QueueEntry],
    layout: &'a Layout,
    facts: Option<&'a Facts>,
    mode: &'a Mode,
    cursor: usize,
    scroll_offset: usize,
    theme: &'a Theme,
    drop_indicator: Option<usize>,
    selected: Option<&'a HashSet<usize>>,
    spinner_tick: usize,
    hover_index: Option<usize>,
    favourites: Option<&'a HashSet<i64>>,
    pointer: Option<(u16, u16)>,
    offline: bool,
}

impl<'a> QueueView<'a> {
    pub fn new(
        entries: &'a [QueueEntry],
        layout: &'a Layout,
        mode: &'a Mode,
        cursor: usize,
        scroll_offset: usize,
        theme: &'a Theme,
    ) -> Self {
        Self {
            entries,
            layout,
            facts: None,
            mode,
            cursor,
            scroll_offset,
            theme,
            drop_indicator: None,
            selected: None,
            spinner_tick: 0,
            hover_index: None,
            favourites: None,
            pointer: None,
            offline: false,
        }
    }

    pub fn with_facts(mut self, facts: &'a Facts, offline: bool) -> Self {
        self.facts = Some(facts);
        self.offline = offline;
        self
    }

    pub fn with_selected(mut self, selected: &'a HashSet<usize>) -> Self {
        self.selected = Some(selected);
        self
    }

    pub fn with_spinner(mut self, tick: usize) -> Self {
        self.spinner_tick = tick;
        self
    }

    pub fn with_drop_indicator(mut self, indicator: Option<usize>) -> Self {
        self.drop_indicator = indicator;
        self
    }

    pub fn with_hover(mut self, hover: &HoverZone) -> Self {
        if let HoverZone::QueueItem(idx) = hover {
            self.hover_index = Some(*idx);
        }
        self
    }

    /// Where the pointer is, so a link under it is drawn underlined.
    pub fn with_pointer(mut self, pointer: Option<(u16, u16)>) -> Self {
        self.pointer = pointer;
        self
    }

    pub fn with_favourites(mut self, favourites: &'a HashSet<i64>) -> Self {
        self.favourites = Some(favourites);
        self
    }

    /// The link under `col`, `row`, with the entry it is for: a heading's
    /// first entry, or the row's own.
    pub fn header_link_at(
        entries: &[QueueEntry],
        layout: &Layout,
        area: Rect,
        scroll_offset: usize,
        col: u16,
        row: u16,
    ) -> Option<(usize, Link)> {
        if !layout.fits(entries) {
            return None;
        }
        let line = *layout.lines.get(line_at(area, scroll_offset, row)?)?;
        let mut buf = Buffer::empty(Rect::new(area.x, row, area.width.max(1), 1));
        let rect = row_rect(area, row);
        let theme = Theme::default();
        let (idx, links) = match line {
            Line::Heading { first, last } => (
                first,
                draw_heading(&mut buf, rect, &theme, &entries[first..=last], None, false),
            ),
            Line::Track(i) | Line::Single(i) => (
                i,
                rows::track(
                    &mut buf,
                    rect,
                    &theme,
                    &track_of(entries, layout.grouped, line, None, false),
                ),
            ),
        };
        links
            .into_iter()
            .find(|(r, _)| r.contains(&col))
            .map(|(_, l)| (idx, l))
    }

    /// The entry drawn on row `y`, if a track is.
    pub fn queue_index_at_y(
        layout: &Layout,
        area: Rect,
        scroll_offset: usize,
        y: u16,
    ) -> Option<usize> {
        match layout.lines.get(line_at(area, scroll_offset, y)?)? {
            Line::Track(i) | Line::Single(i) => Some(*i),
            Line::Heading { .. } => None,
        }
    }

    /// The run of entries the heading on row `y` stands for, inclusive.
    pub fn album_group_at_y(
        layout: &Layout,
        area: Rect,
        scroll_offset: usize,
        y: u16,
    ) -> Option<(usize, usize)> {
        match layout.lines.get(line_at(area, scroll_offset, y)?)? {
            Line::Heading { first, last } => Some((*first, *last)),
            _ => None,
        }
    }

    /// The heading's control under `col`, `row`.
    pub fn control_at(
        area: Rect,
        selected: usize,
        grouped: bool,
        col: u16,
        row: u16,
    ) -> Option<Control> {
        if row != area.y {
            return None;
        }
        controls(area, selected, grouped, area.x)
            .into_iter()
            .find(|(r, _, _)| col >= r.x && col < r.x + r.width)
            .map(|(_, c, _)| c)
    }
}

/// The display line on row `y`: the queue's heading takes the area's first row.
fn line_at(area: Rect, scroll_offset: usize, y: u16) -> Option<usize> {
    let rel = y.checked_sub(area.y)?.checked_sub(1)?;
    Some(scroll_offset + rel as usize)
}

/// The cells a row is drawn in: the area's width, less the row's menu button
/// and the scrollbar.
fn row_rect(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, y, menu_button_x(area).saturating_sub(area.x + 1), 1)
}

/// How many display lines the queue takes.
pub(super) fn display_line_count(layout: &Layout) -> usize {
    layout.len()
}

/// A track row's content, from its entry; `look` adds what the frame knows.
fn track_of<'e>(
    entries: &'e [QueueEntry],
    grouped: bool,
    line: Line,
    look: Option<&Look>,
    current: bool,
) -> rows::Track<'e> {
    let (Line::Track(i) | Line::Single(i)) = line else {
        unreachable!("a heading is not a track");
    };
    let e = &entries[i];
    let single = matches!(line, Line::Single(_));
    let artist = if single {
        Some(if e.artist.is_empty() {
            e.album_artist.as_str()
        } else {
            e.artist.as_str()
        })
    } else {
        (!e.artist.is_empty() && e.artist != e.album_artist).then_some(e.artist.as_str())
    };
    let number = match (single, e.disc, e.track_number) {
        (true, ..) if !grouped => None,
        (true, ..) => Some(String::new()),
        (false, Some(d), Some(n)) if d > 1 => Some(format!("{d}-{n:02}")),
        (false, _, Some(n)) => Some(format!("{n:02}")),
        (false, ..) => Some(String::new()),
    };
    let mut t = rows::Track {
        number,
        title: &e.title,
        artist,
        album: (!grouped).then_some(e.album.as_str()),
        favourite: e.db_id.map(|_| false),
        codec: e.codec.as_deref(),
        duration_ms: e.duration_ms,
        current,
        ..Default::default()
    };
    if let Some(look) = look {
        let playing = e.status == QueueEntryStatus::Playing;
        let (on_server, on_disk) = e
            .db_id
            .and_then(|id| look.facts.and_then(|f| f.sources.get(&id)))
            .copied()
            .unwrap_or((false, !e.path.as_os_str().is_empty() && e.db_id.is_none()));
        t.avail = if e.status == QueueEntryStatus::Downloading {
            Avail::Transferring(
                e.download_progress
                    .and_then(|(done, total)| (total > 0).then(|| done as f64 / total as f64)),
            )
        } else if playing {
            Avail::of(on_server, on_disk, false)
        } else {
            Avail::of(on_server, on_disk, look.offline)
        };
        t.status = match e.status {
            QueueEntryStatus::Playing => Status::Playing,
            QueueEntryStatus::Played => Status::Played,
            QueueEntryStatus::Queued => Status::Queued,
            QueueEntryStatus::Downloading => Status::None,
            QueueEntryStatus::PriorityPending => Status::Pending,
            QueueEntryStatus::Failed => Status::Failed,
        };
        t.dim = e.status == QueueEntryStatus::Played || t.avail == Avail::NotHere;
        t.favourite = e
            .db_id
            .map(|id| look.favourites.is_some_and(|f| f.contains(&id)));
        t.spinner = look.spinner;
    }
    t
}

/// What a frame knows beyond the entries.
struct Look<'a> {
    facts: Option<&'a Facts>,
    favourites: Option<&'a HashSet<i64>>,
    offline: bool,
    spinner: usize,
}

fn draw_heading(
    buf: &mut Buffer,
    rect: Rect,
    theme: &Theme,
    run: &[QueueEntry],
    hover: Option<Link>,
    selected: bool,
) -> Vec<(std::ops::Range<u16>, Link)> {
    let e = &run[0];
    let codec = run
        .iter()
        .map(|t| t.codec.as_deref())
        .reduce(|a, b| if a == b { a } else { None })
        .flatten();
    let ms = run.iter().filter_map(|t| t.duration_ms).sum();
    let facts = rows::facts(e.year.as_deref(), run.len(), ms, codec);
    let artist = if e.album_artist.is_empty() {
        "Unknown Artist"
    } else {
        &e.album_artist
    };
    rows::heading(
        buf,
        rect,
        theme,
        &rows::Heading {
            album: &e.album,
            artist,
            facts: &facts,
            hover,
            selected,
        },
    )
}

impl Widget for QueueView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let rebuilt;
        let layout = if self.layout.fits(self.entries) {
            self.layout
        } else {
            rebuilt = Layout::build(self.entries, self.layout.grouped);
            &rebuilt
        };
        let is_edit = matches!(self.mode, Mode::QueueEdit);
        let theme = self.theme;
        let selected_count = self.selected.map_or(0, HashSet::len);

        // The heading: what the queue is, how long, and its controls.
        let title = match self.facts.and_then(|f| f.lock.as_deref()) {
            Some(name) => format!("playing {name}"),
            None => "queue".to_string(),
        };
        let mut note = format!(" {}", rows::count(self.entries.len(), "track"));
        if layout.total_ms > 0 {
            note.push_str(&format!(" \u{b7} {}", rows::total(layout.total_ms)));
        }
        if is_edit {
            note.push_str(" \u{b7} edit");
        }
        if selected_count > 0 {
            note.push_str(&format!(" \u{b7} {selected_count} selected"));
        }
        note.push(' ');
        let left = heading(buf, area, theme, &fit_title(&title, area.width), &note);
        let shown = if self.entries.is_empty() {
            Vec::new()
        } else {
            controls(area, selected_count, layout.grouped, left + 1)
        };
        let rule_end = shown
            .last()
            .map_or(area.x + area.width, |(r, _, _)| r.x.saturating_sub(1));
        for x in left..rule_end {
            buf[(x, area.y)].set_char('\u{2500}').set_style(theme.rule);
        }
        {
            let pointer = self.pointer;
            for (r, control, label) in shown {
                let mut style = if matches!(control, Control::Grouped(_)) {
                    theme.hint_key
                } else {
                    theme.hint_desc
                };
                if pointer.is_some_and(|(c, row)| row == r.y && c >= r.x && c < r.x + r.width) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                buf.set_string(r.x, r.y, &label, style);
            }
        }

        let inner = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        if inner.height == 0 {
            return;
        }
        if self.entries.is_empty() {
            buf.set_stringn(
                inner.x + 1,
                inner.y,
                "the queue is empty",
                inner.width.saturating_sub(1) as usize,
                theme.heading,
            );
            if inner.height > 1 {
                buf.set_stringn(
                    inner.x + 1,
                    inner.y + 1,
                    "p finds a track, a an album, r an artist",
                    inner.width.saturating_sub(1) as usize,
                    theme.hint_desc,
                );
            }
            return;
        }

        let lines = &layout.lines;
        let hovered_link = self.pointer.and_then(|(col, row)| {
            QueueView::header_link_at(self.entries, layout, area, self.scroll_offset, col, row)
        });
        let selected = |i: usize| self.selected.is_some_and(|s| s.contains(&i));
        let look = Look {
            facts: self.facts,
            favourites: self.favourites,
            offline: self.offline,
            spinner: self.spinner_tick,
        };
        let visible_height = inner.height as usize;
        let start = self.scroll_offset.min(lines.len().saturating_sub(1));
        let end = (start + visible_height).min(lines.len());

        for (row, &line) in lines[start..end].iter().enumerate() {
            let y = inner.y + row as u16;
            let rect = row_rect(area, y);
            match line {
                Line::Heading { first, last } => {
                    let hover = hovered_link.filter(|(i, _)| *i == first).map(|(_, l)| l);
                    let all = (first..=last).all(selected);
                    draw_heading(buf, rect, theme, &self.entries[first..=last], hover, all);
                }
                Line::Track(i) | Line::Single(i) => {
                    let cursor = is_edit && i == self.cursor;
                    let mut t = track_of(
                        self.entries,
                        layout.grouped,
                        line,
                        Some(&look),
                        self.entries[i].status == QueueEntryStatus::Playing,
                    );
                    t.selected = selected(i);
                    t.hovered = self.hover_index == Some(i) && !cursor && !t.selected;
                    t.marker = if cursor {
                        Marker::Cursor
                    } else if self.drop_indicator == Some(i) {
                        Marker::Drop
                    } else if t.selected {
                        Marker::Selected
                    } else {
                        Marker::None
                    };
                    let hover = hovered_link.filter(|(h, _)| *h == i).map(|(_, l)| l);
                    let links = rows::track(buf, rect, theme, &t);
                    if let Some(link) = hover
                        && let Some((r, _)) = links.iter().find(|(_, l)| *l == link)
                        && link != Link::Heart
                    {
                        for x in r.clone() {
                            let cell = &mut buf[(x, y)];
                            cell.set_style(cell.style().add_modifier(Modifier::UNDERLINED));
                        }
                    }
                    // The row's menu, for anyone without a right button: on
                    // the row the pointer is over and on the ones selected.
                    if inner.width > 24 && (self.hover_index == Some(i) || t.selected || cursor) {
                        buf[(menu_button_x(area), y)]
                            .set_char('\u{22ef}')
                            .set_style(theme.hint_key);
                    }
                }
            }
        }

        scrollbar(buf, inner, theme, lines.len(), start);
    }
}

/// The heading's title and note; where they end.
fn heading(buf: &mut Buffer, area: Rect, theme: &Theme, title: &str, note: &str) -> u16 {
    let end = area.x + area.width;
    let mut x = area.x;
    for (text, style) in [(title, theme.heading), (note, theme.hint_desc)] {
        let room = end.saturating_sub(x) as usize;
        let shown = rows::fit(text, room);
        buf.set_stringn(x, area.y, &shown, room, style);
        x += shown.width() as u16;
    }
    x
}

/// The page's title, shortened before the controls are.
fn fit_title(title: &str, width: u16) -> String {
    rows::fit(title, (width / 2) as usize)
}

/// The scrollbar, in eighths of a cell, when the lines overflow.
fn scrollbar(buf: &mut Buffer, inner: Rect, theme: &Theme, total_lines: usize, start: usize) {
    let visible_height = inner.height as usize;
    if total_lines > visible_height && visible_height > 0 && inner.width > 1 {
        let bar_x = inner.x + inner.width - 1;

        // All math in eighths of a cell for sub-pixel precision.
        let vis8 = visible_height * 8;
        let max_scroll = total_lines.saturating_sub(visible_height);

        // Constant thumb size in eighths — depends only on content ratio.
        let thumb_8 = (vis8 * visible_height / total_lines).max(8);

        // Thumb top position in eighths, proportional to scroll progress.
        let track_8 = vis8.saturating_sub(thumb_8);
        let thumb_top_8 = if max_scroll > 0 && track_8 > 0 {
            start * track_8 / max_scroll
        } else {
            0
        };
        let thumb_bot_8 = thumb_top_8 + thumb_8;

        // Lower block elements indexed by eighths filled from bottom (0..=8).
        const BLOCKS: [char; 9] = [
            ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}',
            '\u{2587}', '\u{2588}',
        ];

        let thumb_color = theme.scrollbar_thumb;
        let bg_color = theme.scrollbar_bg;
        let thumb_style = Style::new().fg(thumb_color);
        // Bottom edge: invert the block to fill from top.
        // BLOCKS[8-N] in bg color covers the empty bottom, thumb bg fills the top.
        let bot_style = Style::new().fg(bg_color).bg(thumb_color);

        for row in 0..visible_height {
            let cell_top = row * 8;
            let cell_bot = cell_top + 8;

            let overlap_start = thumb_top_8.max(cell_top);
            let overlap_end = thumb_bot_8.min(cell_bot);
            let eighths = overlap_end.saturating_sub(overlap_start);

            let y = inner.y + row as u16;
            if eighths == 0 {
                // Empty track cell.
                buf[(bar_x, y)]
                    .set_char(' ')
                    .set_style(Style::new().bg(bg_color));
                continue;
            }
            if overlap_start > cell_top && eighths < 8 {
                // Top edge — lower block fills from bottom. Correct.
                buf[(bar_x, y)]
                    .set_char(BLOCKS[eighths])
                    .set_style(Style::new().fg(thumb_color).bg(bg_color));
            } else if overlap_end < cell_bot && eighths < 8 {
                // Bottom edge — flip: draw empty portion in bg, thumb as bg color.
                buf[(bar_x, y)]
                    .set_char(BLOCKS[8 - eighths])
                    .set_style(bot_style);
            } else {
                // Full cell.
                buf[(bar_x, y)].set_char('\u{2588}').set_style(thumb_style);
            }
        }
    }
}

/// Calculate the scroll offset needed to keep a queue index visible.
pub fn scroll_for_cursor(
    layout: &Layout,
    cursor: usize,
    current_scroll: usize,
    visible_height: usize,
) -> usize {
    let cursor_line = layout.line_of(cursor).unwrap_or(0);
    if cursor_line < current_scroll {
        cursor_line
    } else if cursor_line >= current_scroll + visible_height {
        cursor_line.saturating_sub(visible_height) + 1
    } else {
        current_scroll
    }
}

/// Scroll so the cursor is near the top of the visible area.
/// If the cursor's album header is just above, include it.
pub fn scroll_cursor_to_top(layout: &Layout, cursor: usize, visible_height: usize) -> usize {
    let cursor_line = layout.line_of(cursor).unwrap_or(0);
    let top = match cursor_line.checked_sub(1).map(|l| layout.lines[l]) {
        Some(Line::Heading { .. }) => cursor_line - 1,
        _ => cursor_line,
    };
    top.min(layout.len().saturating_sub(visible_height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn entry(n: usize, album: &str) -> QueueEntry {
        QueueEntry {
            playlist_entry_id: None,
            id: QueueItemId::new(),
            db_id: None,
            path: std::path::PathBuf::from(format!("/music/{}.flac", n)),
            title: format!("track {}", n),
            artist: "artist".into(),
            album_artist: "artist".into(),
            album: album.into(),
            year: None,
            codec: None,
            track_number: Some(n as i64 + 1),
            disc: None,
            duration_ms: Some(200_000),
            status: QueueEntryStatus::Queued,
            download_progress: None,
            error: None,
        }
    }

    fn draw(
        entries: &[QueueEntry],
        mode: &Mode,
        cursor: usize,
        offset: usize,
        w: u16,
        h: u16,
    ) -> Terminal<TestBackend> {
        let theme = Theme::default();
        let selected = HashSet::new();
        let layout = Layout::build(entries, true);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| {
                let view = QueueView::new(entries, &layout, mode, cursor, offset, &theme)
                    .with_selected(&selected);
                f.render_widget(view, f.area());
            })
            .unwrap();
        terminal
    }

    /// The top border can leave no room for the empty-state line.
    #[test]
    fn empty_queue_renders_in_a_single_row() {
        for (w, h) in [(1, 1), (2, 1), (3, 1), (1, 2), (80, 1), (80, 24)] {
            draw(&[], &Mode::Normal, 0, 0, w, h);
        }
    }

    #[test]
    fn renders_with_a_scroll_offset_past_the_end() {
        let entries: Vec<_> = (0..5).map(|n| entry(n, "album")).collect();
        draw(&entries, &Mode::Normal, 0, 9_999, 40, 10);
    }

    /// Find the row holding the edit-mode cursor marker.
    fn cursor_row(terminal: &Terminal<TestBackend>, w: u16, h: u16) -> u16 {
        let buf = terminal.backend().buffer();
        (0..h)
            .find(|&y| (0..w).any(|x| buf[(x, y)].symbol() == "\u{258c}"))
            .expect("cursor marker is on screen")
    }

    /// The renderer and the hit-test must agree on which row a track occupies,
    /// including the step where moving the cursor down forces a scroll.
    #[test]
    fn clicked_row_is_the_drawn_row_across_an_edit_scroll_boundary() {
        // 40 tracks over two albums, so header lines shift the mapping.
        let entries: Vec<_> = (0..40)
            .map(|n| entry(n, if n < 17 { "first" } else { "second" }))
            .collect();
        let (w, h) = (60u16, 20u16);
        let area = Rect::new(0, 0, w, h);
        let visible_height = (h - 1) as usize;

        let mut offset = 0usize;
        for cursor in 0..entries.len() {
            offset = scroll_for_cursor(
                &Layout::build(&entries, true),
                cursor,
                offset,
                visible_height,
            );
            let terminal = draw(&entries, &Mode::QueueEdit, cursor, offset, w, h);
            let row = cursor_row(&terminal, w, h);
            assert_eq!(
                QueueView::queue_index_at_y(&Layout::build(&entries, true), area, offset, row),
                Some(cursor),
                "cursor {} drawn on row {} with offset {}",
                cursor,
                row,
                offset
            );
        }
    }

    /// The widget renders exactly the offset it is handed. Scrolling to follow
    /// the cursor is `App`'s job — doing it here too desynced every hit-test.
    #[test]
    fn edit_mode_widget_does_not_scroll_itself() {
        let entries: Vec<_> = (0..40).map(|n| entry(n, "")).collect();
        let (w, h) = (60u16, 20u16);
        // Cursor far below the window: a self-scrolling widget would jump to it.
        let terminal = draw(&entries, &Mode::QueueEdit, 39, 0, w, h);
        let buf = terminal.backend().buffer();
        let first_row: String = (0..w).map(|x| buf[(x, 1)].symbol()).collect();
        assert!(first_row.contains("track 0"), "drew {:?}", first_row);
    }

    #[test]
    fn album_headers_are_not_clickable_as_tracks() {
        let entries: Vec<_> = (0..6)
            .map(|n| entry(n, if n < 3 { "first" } else { "second" }))
            .collect();
        let area = Rect::new(0, 0, 60, 20);
        // Row 1 is the first album header.
        assert_eq!(
            QueueView::queue_index_at_y(&Layout::build(&entries, true), area, 0, 1),
            None
        );
        assert_eq!(
            QueueView::album_group_at_y(&Layout::build(&entries, true), area, 0, 1),
            Some((0, 2))
        );
        assert_eq!(
            QueueView::queue_index_at_y(&Layout::build(&entries, true), area, 0, 2),
            Some(0)
        );
    }

    /// The queue's order after `cmd`, as the player applies a move.
    fn after(entries: &[QueueEntry], cmd: PlayerCommand) -> Vec<QueueItemId> {
        let PlayerCommand::MoveItemsInPlaylist { ids, target, after } = cmd else {
            panic!("a move");
        };
        let mut rest: Vec<_> = entries
            .iter()
            .map(|e| e.id)
            .filter(|id| !ids.contains(id))
            .collect();
        let at = rest
            .iter()
            .position(|id| *id == target)
            .expect("target stays")
            + after as usize;
        rest.splice(at..at, ids);
        rest
    }

    fn picked(entries: &[QueueEntry], idx: &[usize]) -> HashSet<QueueItemId> {
        idx.iter().map(|&i| entries[i].id).collect()
    }

    #[test]
    fn a_selection_lands_where_it_is_placed() {
        let entries: Vec<_> = (0..6).map(|n| entry(n, "album")).collect();
        let id = |i: usize| entries[i].id;
        let moving = picked(&entries, &[1, 2]);
        let down = placement(&entries, &moving, 2).expect("a move");
        assert_eq!(
            after(&entries, down),
            [id(0), id(3), id(1), id(2), id(4), id(5)]
        );
        let top = placement(&entries, &moving, 0).expect("a move");
        assert_eq!(
            after(&entries, top),
            [id(1), id(2), id(0), id(3), id(4), id(5)]
        );
        let past_the_end = placement(&entries, &moving, 99).expect("a move");
        assert_eq!(
            after(&entries, past_the_end),
            [id(0), id(3), id(4), id(5), id(1), id(2)]
        );
    }

    #[test]
    fn a_selection_with_gaps_closes_up_and_never_targets_itself() {
        let entries: Vec<_> = (0..6).map(|n| entry(n, "album")).collect();
        let id = |i: usize| entries[i].id;
        // Placed over its own second track, which the old drag sent to the end.
        let moving = picked(&entries, &[0, 4]);
        let cmd = placement(&entries, &moving, 3).expect("a move");
        assert_eq!(
            after(&entries, cmd),
            [id(1), id(2), id(3), id(0), id(4), id(5)]
        );
    }

    #[test]
    fn a_selection_already_in_place_does_not_move() {
        let entries: Vec<_> = (0..4).map(|n| entry(n, "album")).collect();
        assert!(placement(&entries, &picked(&entries, &[1, 2]), 1).is_none());
        assert!(placement(&entries, &picked(&entries, &[2, 3]), 9).is_none());
        assert!(placement(&entries, &picked(&entries, &[0, 1, 2, 3]), 1).is_none());
        assert!(placement(&entries, &HashSet::new(), 1).is_none());
    }

    #[test]
    fn a_placement_asked_twice_from_the_same_snapshot_is_the_same() {
        // Two key presses in one frame both read the queue as it was.
        let entries: Vec<_> = (0..5).map(|n| entry(n, "album")).collect();
        let moving = picked(&entries, &[1]);
        let first = after(&entries, placement(&entries, &moving, 2).unwrap());
        let second = after(&entries, placement(&entries, &moving, 2).unwrap());
        assert_eq!(first, second);
    }

    #[test]
    fn a_track_without_an_album_ends_the_group_above_it() {
        let mut entries: Vec<_> = (0..3).map(|n| entry(n, "album")).collect();
        entries[1].album.clear();
        let area = Rect::new(0, 0, 60, 20);
        // heading, 0, 1, heading, 2
        assert_eq!(display_line_count(&Layout::build(&entries, true)), 5);
        assert_eq!(
            QueueView::album_group_at_y(&Layout::build(&entries, true), area, 0, 1),
            Some((0, 0))
        );
        assert_eq!(
            QueueView::album_group_at_y(&Layout::build(&entries, true), area, 0, 4),
            Some((2, 2))
        );
    }

    #[test]
    fn the_heading_row_is_not_the_first_track() {
        let entries: Vec<_> = (0..3).map(|n| entry(n, "")).collect();
        let area = Rect::new(0, 0, 60, 20);
        assert_eq!(
            QueueView::queue_index_at_y(&Layout::build(&entries, true), area, 0, 0),
            None
        );
        assert_eq!(
            QueueView::queue_index_at_y(&Layout::build(&entries, true), area, 0, 1),
            Some(0)
        );
    }

    /// A queue with the shapes real ones have: albums, a multi-disc one, a
    /// long and a CJK name, a single, a track without an album, the same
    /// track twice, and every status.
    fn fixture() -> Vec<QueueEntry> {
        let mut out = Vec::new();
        let mut push = |title: &str,
                        artist: &str,
                        album_artist: &str,
                        album: &str,
                        n: i64,
                        disc: Option<i64>,
                        status| {
            let mut e = entry(out.len(), album);
            e.title = title.into();
            e.artist = artist.into();
            e.album_artist = album_artist.into();
            e.track_number = Some(n);
            e.disc = disc;
            e.status = status;
            e.year = Some("2007".into());
            e.codec = Some("flac".into());
            e.db_id = Some(out.len() as i64 + 1);
            e.duration_ms = Some(180_000 + 7_000 * out.len() as u64);
            out.push(e);
        };
        use QueueEntryStatus::*;
        push(
            "Wild Strawberries",
            "Pnau",
            "Pnau",
            "No More Violence",
            1,
            None,
            Played,
        );
        push("Baby", "Pnau", "Pnau", "No More Violence", 2, None, Playing);
        push(
            "With You Forever (featuring a very long list of guests who all sang)",
            "Pnau feat. Everyone",
            "Pnau",
            "No More Violence",
            3,
            None,
            Queued,
        );
        push(
            "\u{6625}\u{306e}\u{6d77}",
            "\u{5742}\u{672c}\u{9f8d}\u{4e00}",
            "\u{5742}\u{672c}\u{9f8d}\u{4e00}",
            "\u{6226}\u{5834}\u{306e}\u{30e1}\u{30ea}\u{30fc}\u{30af}\u{30ea}\u{30b9}\u{30de}\u{30b9}",
            1,
            None,
            Downloading,
        );
        push(
            "Disc two opener",
            "Band",
            "Band",
            "Double",
            1,
            Some(2),
            PriorityPending,
        );
        push(
            "Disc two closer",
            "Band",
            "Band",
            "Double",
            12,
            Some(2),
            Failed,
        );
        push("Single", "Solo", "Solo", "Single", 1, None, Queued);
        push("Loose track", "Someone", "", "", 1, None, Queued);
        push("Baby", "Pnau", "Pnau", "No More Violence", 2, None, Queued);
        out
    }

    fn text(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                let mut line = String::new();
                let mut skip = 0;
                for x in 0..buf.area.width {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let s = buf[(x, y)].symbol();
                    skip = s.width().saturating_sub(1);
                    line.push_str(s);
                }
                line
            })
            .collect()
    }

    fn render(entries: &[QueueEntry], grouped: bool, w: u16, h: u16) -> Buffer {
        let theme = Theme::default();
        let layout = Layout::build(entries, grouped);
        let mut facts = Facts::default();
        for e in entries {
            facts.sources.insert(
                e.db_id.unwrap_or(0),
                (true, e.status == QueueEntryStatus::Played),
            );
        }
        facts.lock = Some("No More Violence".into());
        let favourites: HashSet<i64> = [2].into_iter().collect();
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        QueueView::new(entries, &layout, &Mode::Normal, 0, 0, &theme)
            .with_facts(&facts, false)
            .with_favourites(&favourites)
            .render(area, &mut buf);
        buf
    }

    #[test]
    fn a_single_and_a_loose_track_have_no_heading() {
        let entries = fixture();
        let layout = Layout::build(&entries, true);
        let singles: Vec<usize> = layout
            .lines()
            .iter()
            .filter_map(|l| match l {
                Line::Single(i) => Some(*i),
                _ => None,
            })
            .collect();
        assert_eq!(singles, [6, 7]);
        let headings = layout
            .lines()
            .iter()
            .filter(|l| matches!(l, Line::Heading { .. }))
            .count();
        assert_eq!(headings, 4, "the repeated track opens a run of its own");
        let flat = Layout::build(&entries, false);
        assert!(flat.lines().iter().all(|l| matches!(l, Line::Single(_))));
    }

    #[test]
    fn lengths_and_facts_end_in_one_column() {
        let entries = fixture();
        for (w, h) in [(80, 24), (120, 40), (200, 60), (50, 20)] {
            let buf = render(&entries, true, w, h);
            let lines = text(&buf);
            let shown = &lines[1..=entries.len() + 4];
            let end = shown[1].trim_end().width();
            for l in shown {
                let e = l.trim_end().width();
                // A heading too narrow for its facts leaves them out.
                let facts = l.contains('\u{b7}');
                let track = !l.starts_with(" No More")
                    && !l.starts_with(" Double")
                    && !l.starts_with(" \u{6226}");
                assert!(
                    if track || facts { e == end } else { e <= end },
                    "{w}x{h}: rows end at {end}, not {e}: {l:?}\n{}",
                    lines.join("\n")
                );
            }
        }
    }

    #[test]
    fn the_heading_says_what_is_queued() {
        let entries = fixture();
        let lines = text(&render(&entries, true, 120, 20));
        assert!(
            lines[0].starts_with("playing No More Violence 9 tracks"),
            "{}",
            lines[0]
        );
        assert!(lines[0].contains("by album"), "{}", lines[0]);
        assert!(lines[1].contains("No More Violence  Pnau"), "{}", lines[1]);
        assert!(lines[1].contains("2007 \u{b7} 3 tracks"), "{}", lines[1]);
    }

    /// Writes the queue at each size to `$KOAN_DUMP_DIR` to be read.
    #[test]
    #[ignore]
    fn dump() {
        let Ok(dir) = std::env::var("KOAN_DUMP_DIR") else {
            return;
        };
        let entries = fixture();
        for grouped in [true, false] {
            for (w, h) in [(80, 24), (120, 40), (200, 60), (50, 20)] {
                let lines = text(&render(&entries, grouped, w, h));
                let name = format!(
                    "{dir}/queue-{}-{w}x{h}.txt",
                    if grouped { "albums" } else { "tracks" }
                );
                std::fs::write(name, lines.join("\n")).unwrap();
            }
        }
    }
}
