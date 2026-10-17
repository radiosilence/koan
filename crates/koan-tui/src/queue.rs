use std::collections::HashSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{QueueEntry, QueueEntryStatus, QueueItemId};

use super::app::{HoverZone, Mode};
use super::theme::Theme;
use super::transport::format_time;

const SPINNER: &[char] = &[
    '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}', '\u{2827}',
];

/// The two links in an album heading: who it is by and which record it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderLink {
    Artist,
    Album,
}

/// Where each link sits in the heading's text, as column offsets from the
/// start of its line: the artist, then " \u{2014} ", the year, the record.
fn header_ranges(entry: &QueueEntry) -> [(HeaderLink, std::ops::Range<u16>); 2] {
    let artist = entry.album_artist.chars().count() as u16;
    let year = entry
        .year
        .as_deref()
        .map_or(0, |y| y.chars().count() as u16 + 3);
    let album = entry.album.chars().count() as u16;
    let start = 1;
    let album_start = start + artist + 3 + year;
    [
        (HeaderLink::Artist, start..start + artist),
        (HeaderLink::Album, album_start..album_start + album),
    ]
}

fn link_at(entry: &QueueEntry, area_x: u16, col: u16) -> Option<HeaderLink> {
    let off = col.checked_sub(area_x)?;
    header_ranges(entry)
        .into_iter()
        .find(|(_, r)| r.contains(&off))
        .map(|(l, _)| l)
}

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

pub struct QueueView<'a> {
    entries: &'a [QueueEntry],
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
}

impl<'a> QueueView<'a> {
    pub fn new(
        entries: &'a [QueueEntry],
        mode: &'a Mode,
        cursor: usize,
        scroll_offset: usize,
        theme: &'a Theme,
    ) -> Self {
        Self {
            entries,
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
        }
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

    /// The link under `col`, `row` when that is on an album heading, with the
    /// index of the heading's first entry.
    pub fn header_link_at(
        entries: &[QueueEntry],
        area: Rect,
        scroll_offset: usize,
        col: u16,
        row: u16,
    ) -> Option<(usize, HeaderLink)> {
        header_link_in(
            entries,
            &build_display_lines(entries),
            area,
            scroll_offset,
            col,
            row,
        )
    }

    pub fn with_favourites(mut self, favourites: &'a HashSet<i64>) -> Self {
        self.favourites = Some(favourites);
        self
    }

    /// Given a y-coordinate within the queue area, return the queue entry index
    /// it corresponds to (accounting for album headers and scroll offset).
    pub fn queue_index_at_y(
        entries: &[QueueEntry],
        area: Rect,
        scroll_offset: usize,
        y: u16,
    ) -> Option<usize> {
        let line = line_at(area, scroll_offset, y)?;
        match build_display_lines(entries).get(line) {
            Some(&(idx, false)) => Some(idx),
            _ => None,
        }
    }

    /// If the y-coordinate lands on an album header, return the range of
    /// contiguous queue indices that belong to that album group (inclusive).
    pub fn album_group_at_y(
        entries: &[QueueEntry],
        area: Rect,
        scroll_offset: usize,
        y: u16,
    ) -> Option<(usize, usize)> {
        let line = line_at(area, scroll_offset, y)?;
        let &(first, true) = build_display_lines(entries).get(line)? else {
            return None;
        };
        let key = (&entries[first].album_artist, &entries[first].album);
        let last = (first..entries.len())
            .take_while(|&i| entries[i].album_artist == *key.0 && entries[i].album == *key.1)
            .last()
            .unwrap_or(first);
        Some((first, last))
    }
}

/// The display line on row `y`: the queue's heading takes the area's first row.
fn line_at(area: Rect, scroll_offset: usize, y: u16) -> Option<usize> {
    let rel = y.checked_sub(area.y)?.checked_sub(1)?;
    Some(scroll_offset + rel as usize)
}

fn header_link_in(
    entries: &[QueueEntry],
    lines: &[(usize, bool)],
    area: Rect,
    scroll_offset: usize,
    col: u16,
    row: u16,
) -> Option<(usize, HeaderLink)> {
    let &(idx, true) = lines.get(line_at(area, scroll_offset, row)?)? else {
        return None;
    };
    link_at(&entries[idx], area.x, col).map(|l| (idx, l))
}

/// Whether `entry` opens a new album group after `current`. A track with no
/// album ends the group before it.
fn starts_group<'e>(entry: &'e QueueEntry, current: &mut Option<(&'e str, &'e str)>) -> bool {
    if entry.album.is_empty() {
        *current = None;
        return false;
    }
    let key = Some((entry.album_artist.as_str(), entry.album.as_str()));
    let starts = *current != key;
    *current = key;
    starts
}

/// Count total display lines (tracks + album headers) without allocating.
pub(super) fn display_line_count(entries: &[QueueEntry]) -> usize {
    let mut current = None;
    entries.len()
        + entries
            .iter()
            .filter(|e| starts_group(e, &mut current))
            .count()
}

/// Each display line: the queue index it shows, and whether it is the album
/// heading above that entry.
fn build_display_lines(entries: &[QueueEntry]) -> Vec<(usize, bool)> {
    let mut lines = Vec::with_capacity(entries.len() + entries.len() / 8);
    let mut current = None;
    for (i, entry) in entries.iter().enumerate() {
        if starts_group(entry, &mut current) {
            lines.push((i, true));
        }
        lines.push((i, false));
    }
    lines
}

/// How a track row is marked.
#[derive(Clone, Copy)]
struct RowLook {
    cursor: bool,
    selected: bool,
    drop_target: bool,
    hovered: bool,
    favourite: bool,
}

impl Widget for QueueView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let is_edit = matches!(self.mode, Mode::QueueEdit);

        let note = if is_edit {
            format!(" edit \u{b7} {} ", self.entries.len())
        } else {
            format!(" {} ", self.entries.len())
        };
        super::ui::heading(buf, area, self.theme, "queue", &note);
        let inner = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );

        // The top border can consume the only row there is.
        if inner.height == 0 {
            return;
        }

        if self.entries.is_empty() {
            let line = Line::from(Span::styled(" empty", self.theme.hint_desc));
            buf.set_line(inner.x, inner.y, &line, inner.width);
            return;
        }

        let lines = build_display_lines(self.entries);
        let hovered_link = self.pointer.and_then(|(col, row)| {
            header_link_in(self.entries, &lines, area, self.scroll_offset, col, row)
        });
        let selected = |i: usize| self.selected.is_some_and(|s| s.contains(&i));
        let visible_height = inner.height as usize;
        let start = self.scroll_offset.min(lines.len().saturating_sub(1));
        let end = (start + visible_height).min(lines.len());

        for (row, &(i, is_header)) in lines[start..end].iter().enumerate() {
            let y = inner.y + row as u16;
            let entry = &self.entries[i];
            if is_header {
                let hover = hovered_link
                    .filter(|(first, _)| *first == i)
                    .map(|(_, l)| l);
                let line = render_album_header(entry, self.theme, hover);
                buf.set_line(inner.x, y, &line, inner.width);
                continue;
            }
            let cursor = is_edit && i == self.cursor;
            let look = RowLook {
                cursor,
                selected: selected(i),
                drop_target: self.drop_indicator == Some(i),
                hovered: self.hover_index == Some(i) && !cursor && !selected(i),
                favourite: self
                    .favourites
                    .is_some_and(|f| entry.db_id.is_some_and(|id| f.contains(&id))),
            };
            let line = render_track_line(entry, &look, self.theme, self.spinner_tick);
            buf.set_line(inner.x, y, &line, inner.width);
            // The row's menu, for anyone without a right button: on the row
            // the pointer is over and on the ones selected.
            if inner.width > 24 && (self.hover_index == Some(i) || look.selected || cursor) {
                buf[(menu_button_x(area), y)]
                    .set_char('\u{22ef}')
                    .set_style(self.theme.hint_key);
            }
        }

        // Scrollbar — only when content overflows.
        // Uses sub-pixel rendering with 1/8th-cell resolution via Unicode block elements.
        let total_lines = lines.len();
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

            let thumb_color = self.theme.scrollbar_thumb;
            let bg_color = self.theme.scrollbar_bg;
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
}

fn render_album_header<'a>(
    entry: &QueueEntry,
    theme: &Theme,
    hover: Option<HeaderLink>,
) -> Line<'a> {
    let year = entry
        .year
        .as_deref()
        .map(|y| format!("({}) ", y))
        .unwrap_or_default();
    let codec = entry
        .codec
        .as_deref()
        .map(|c| format!(" [{}]", c))
        .unwrap_or_default();

    let underline = |style: ratatui::style::Style, on: bool| {
        if on {
            style.add_modifier(ratatui::style::Modifier::UNDERLINED)
        } else {
            style
        }
    };
    let artist_style = underline(theme.album_header_artist, hover == Some(HeaderLink::Artist));
    let album_style = underline(theme.album_header_album, hover == Some(HeaderLink::Album));
    let dim = theme.hint_desc;

    Line::from(vec![
        Span::raw(" "),
        Span::styled(entry.album_artist.clone(), artist_style),
        Span::styled(" \u{2014} ", dim),
        Span::styled(year, dim),
        Span::styled(entry.album.clone(), album_style),
        Span::styled(codec, dim),
    ])
}

fn render_track_line<'a>(
    entry: &QueueEntry,
    look: &RowLook,
    theme: &Theme,
    spinner_tick: usize,
) -> Line<'a> {
    let RowLook {
        cursor: is_cursor,
        selected: is_selected,
        drop_target: is_drop_target,
        hovered: is_hovered,
        favourite: is_favourite,
    } = *look;
    let spin_char = SPINNER[spinner_tick % SPINNER.len()];

    let progress = entry.download_progress.as_ref();

    let status_icon = match entry.status {
        QueueEntryStatus::Queued => Span::raw(" "),
        QueueEntryStatus::Playing => Span::styled("\u{25b6}", theme.track_playing),
        QueueEntryStatus::Played => {
            if is_selected {
                Span::styled(" ", theme.track_selected)
            } else {
                Span::raw(" ")
            }
        }
        QueueEntryStatus::Downloading => {
            if let Some(&(downloaded, total)) = progress {
                if let Some(pct) = (downloaded * 100).checked_div(total) {
                    Span::styled(format!("{:2}%", pct.min(99)), theme.spinner)
                } else {
                    let kb = downloaded / 1024;
                    Span::styled(format!("{}K", kb), theme.spinner)
                }
            } else {
                Span::styled(format!(" {} ", spin_char), theme.hint_desc)
            }
        }
        QueueEntryStatus::PriorityPending => {
            if let Some(&(downloaded, total)) = progress {
                if let Some(pct) = (downloaded * 100).checked_div(total) {
                    Span::styled(format!("{:2}%", pct.min(99)), theme.track_playing)
                } else {
                    let kb = downloaded / 1024;
                    Span::styled(format!("{}K", kb), theme.track_playing)
                }
            } else {
                Span::styled(format!(">{} ", spin_char), theme.track_playing)
            }
        }
        QueueEntryStatus::Failed => Span::styled("!", theme.failed),
    };

    let track_num = match (entry.disc, entry.track_number) {
        (Some(d), Some(n)) if d > 1 => format!("{}-{:02}", d, n),
        (_, Some(n)) => format!("{:02}", n),
        _ => "  ".into(),
    };

    let dur = entry.duration_ms.map(format_time).unwrap_or_default();

    let is_playing = matches!(entry.status, QueueEntryStatus::Playing);
    let bold = ratatui::style::Modifier::BOLD;
    let artist_part = if !entry.artist.is_empty() && entry.artist != entry.album_artist {
        let artist_style = if is_selected {
            theme.track_selected
        } else if is_hovered {
            theme.track_hover
        } else if is_playing {
            theme.track_playing.add_modifier(bold)
        } else {
            theme.track_playing
        };
        let sep_style = if is_selected {
            theme.track_selected
        } else if is_playing {
            theme.hint_desc.add_modifier(bold)
        } else {
            theme.hint_desc
        };
        vec![
            Span::styled(entry.artist.clone(), artist_style),
            Span::styled(" \u{2014} ", sep_style),
        ]
    } else {
        vec![]
    };

    // Visual markers: cursor > selected, drop indicator.
    let cursor_marker = if is_cursor {
        Span::styled("\u{258c}", theme.bar)
    } else if is_drop_target {
        Span::styled("\u{25be}", theme.hint_key) // ▾ drop indicator
    } else if is_selected {
        Span::styled("\u{2502}", theme.track_selected)
    } else {
        Span::raw(" ")
    };

    // Favourite star in gutter (between padding and cursor marker).
    let fav_marker = if is_favourite {
        Span::styled("\u{2605}", theme.favourite) // ★
    } else {
        Span::raw(" ")
    };

    let title_style = if is_cursor {
        theme.track_cursor
    } else if is_selected {
        theme.track_selected
    } else if is_hovered {
        theme.track_hover
    } else if is_playing {
        theme
            .track_playing
            .add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        theme.track_normal
    };

    let num_style = if is_selected {
        theme.track_selected
    } else if is_playing {
        theme.track_number.add_modifier(bold)
    } else {
        theme.track_number
    };

    let dur_style = if is_selected {
        theme.track_selected
    } else if is_playing {
        theme.hint_desc.add_modifier(bold)
    } else {
        theme.hint_desc
    };

    let mut spans = vec![
        Span::raw(" "),
        fav_marker,
        cursor_marker,
        status_icon,
        Span::raw(" "),
        Span::styled(track_num, num_style),
        Span::raw(" "),
    ];
    spans.extend(artist_part);
    spans.push(Span::styled(entry.title.clone(), title_style));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(dur, dur_style));

    Line::from(spans)
}

/// Calculate the scroll offset needed to keep a queue index visible.
pub fn scroll_for_cursor(
    entries: &[QueueEntry],
    cursor: usize,
    current_scroll: usize,
    visible_height: usize,
) -> usize {
    let display_lines = build_display_lines(entries);
    let cursor_line = display_lines
        .iter()
        .position(|&line| line == (cursor, false))
        .unwrap_or(0);

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
pub fn scroll_cursor_to_top(entries: &[QueueEntry], cursor: usize, visible_height: usize) -> usize {
    let display_lines = build_display_lines(entries);
    let cursor_line = display_lines
        .iter()
        .position(|&line| line == (cursor, false))
        .unwrap_or(0);

    // Include the album header if it's directly above the cursor line.
    let top = if cursor_line > 0 {
        let (_, is_header) = display_lines[cursor_line - 1];
        if is_header {
            cursor_line - 1
        } else {
            cursor_line
        }
    } else {
        cursor_line
    };

    // Don't overscroll past the end.
    let max_scroll = display_lines.len().saturating_sub(visible_height);
    top.min(max_scroll)
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
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| {
                let view =
                    QueueView::new(entries, mode, cursor, offset, &theme).with_selected(&selected);
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
            offset = scroll_for_cursor(&entries, cursor, offset, visible_height);
            let terminal = draw(&entries, &Mode::QueueEdit, cursor, offset, w, h);
            let row = cursor_row(&terminal, w, h);
            assert_eq!(
                QueueView::queue_index_at_y(&entries, area, offset, row),
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
        assert_eq!(QueueView::queue_index_at_y(&entries, area, 0, 1), None);
        assert_eq!(
            QueueView::album_group_at_y(&entries, area, 0, 1),
            Some((0, 2))
        );
        assert_eq!(QueueView::queue_index_at_y(&entries, area, 0, 2), Some(0));
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
        assert_eq!(display_line_count(&entries), 5);
        assert_eq!(
            QueueView::album_group_at_y(&entries, area, 0, 1),
            Some((0, 0))
        );
        assert_eq!(
            QueueView::album_group_at_y(&entries, area, 0, 4),
            Some((2, 2))
        );
    }

    #[test]
    fn the_heading_row_is_not_the_first_track() {
        let entries: Vec<_> = (0..3).map(|n| entry(n, "")).collect();
        let area = Rect::new(0, 0, 60, 20);
        assert_eq!(QueueView::queue_index_at_y(&entries, area, 0, 0), None);
        assert_eq!(QueueView::queue_index_at_y(&entries, area, 0, 1), Some(0));
    }
}
