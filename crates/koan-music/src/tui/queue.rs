use std::collections::HashSet;
use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use koan_core::player::state::{QueueEntry, QueueEntryStatus};

use super::app::{HoverZone, Mode};
use super::theme::Theme;
use super::transport::format_time;

const SPINNER: &[char] = &[
    '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}', '\u{2827}',
];

pub struct QueueView<'a> {
    entries: &'a [QueueEntry],
    mode: &'a Mode,
    cursor: usize,
    scroll_offset: usize,
    theme: &'a Theme,
    drop_indicator: Option<usize>,
    selected: &'a HashSet<usize>,
    spinner_tick: usize,
    hover_index: Option<usize>,
    favourites: Option<&'a HashSet<PathBuf>>,
}

impl<'a> QueueView<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        entries: &'a [QueueEntry],
        mode: &'a Mode,
        cursor: usize,
        scroll_offset: usize,
        theme: &'a Theme,
        selected: &'a HashSet<usize>,
        spinner_tick: usize,
    ) -> Self {
        Self {
            entries,
            mode,
            cursor,
            scroll_offset,
            theme,
            drop_indicator: None,
            selected,
            spinner_tick,
            hover_index: None,
            favourites: None,
        }
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

    pub fn with_favourites(mut self, favourites: &'a HashSet<PathBuf>) -> Self {
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
        let rel_y = (y.saturating_sub(area.y)) as usize;

        let display_lines = build_display_lines(entries);

        // +1 to account for the block border (TOP) consuming the first row.
        let target_line = scroll_offset + rel_y.saturating_sub(1);
        if target_line < display_lines.len() {
            let (idx, is_header) = display_lines[target_line];
            if is_header { None } else { idx }
        } else {
            None
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
        let rel_y = (y.saturating_sub(area.y)) as usize;
        let display_lines = build_display_lines(entries);
        let target_line = scroll_offset + rel_y.saturating_sub(1);

        if target_line >= display_lines.len() {
            return None;
        }
        let (first_idx, is_header) = display_lines[target_line];
        if !is_header {
            return None;
        }
        let first = first_idx?;
        let key = (&entries[first].album_artist, &entries[first].album);

        // Walk forward from first to find the end of the contiguous group.
        let last = (first..entries.len())
            .take_while(|&i| entries[i].album_artist == *key.0 && entries[i].album == *key.1)
            .last()
            .unwrap_or(first);

        Some((first, last))
    }
}

/// Count total display lines (tracks + album headers) without allocating.
/// Must use identical grouping logic as `build_display_lines`.
pub(super) fn display_line_count(entries: &[QueueEntry]) -> usize {
    let mut count = entries.len();
    let mut current_album_key: Option<(&str, &str)> = None;
    for entry in entries {
        let album_key = (entry.album_artist.as_str(), entry.album.as_str());
        if !entry.album.is_empty() && current_album_key != Some(album_key) {
            current_album_key = Some(album_key);
            count += 1;
        }
    }
    count
}

/// Each display line: (Option<queue_index>, is_header).
/// For headers, the index points to the first entry of that album group
/// (so callers can look up album info for rendering).
fn build_display_lines(entries: &[QueueEntry]) -> Vec<(Option<usize>, bool)> {
    let mut lines = Vec::new();
    let mut current_album_key: Option<(&str, &str)> = None;

    for (i, entry) in entries.iter().enumerate() {
        let album_key = (entry.album_artist.as_str(), entry.album.as_str());
        let show_header = if entry.album.is_empty() {
            false
        } else {
            current_album_key != Some(album_key)
        };

        if show_header {
            current_album_key = Some(album_key);
            lines.push((Some(i), true));
        }

        lines.push((Some(i), false));
    }
    lines
}

impl Widget for QueueView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let is_edit = matches!(self.mode, Mode::QueueEdit);

        let header_spans = if is_edit {
            vec![
                Span::styled(
                    " queue [edit] ",
                    self.theme
                        .album_header_artist
                        .fg(self.theme.warning)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("({}) ", self.entries.len()), self.theme.hint_desc),
            ]
        } else {
            vec![
                Span::styled(" queue ", self.theme.hint_desc),
                Span::styled(format!("({}) ", self.entries.len()), self.theme.hint_desc),
            ]
        };

        let block = Block::default()
            .borders(Borders::TOP)
            .title(Line::from(header_spans));

        let inner = block.inner(area);
        block.render(area, buf);

        if self.entries.is_empty() {
            let line = Line::from(Span::styled(" empty", self.theme.hint_desc));
            buf.set_line(inner.x, inner.y, &line, inner.width);
            return;
        }

        // Build display lines using shared index map to avoid duplicating
        // the album-grouping logic.
        let index_map = build_display_lines(self.entries);
        let mut display_lines: Vec<(Option<usize>, Line)> = Vec::with_capacity(index_map.len());
        for &(entry_idx, is_header) in &index_map {
            if is_header {
                if let Some(i) = entry_idx {
                    display_lines.push((None, render_album_header(&self.entries[i], self.theme)));
                }
            } else if let Some(i) = entry_idx {
                let is_cursor = is_edit && i == self.cursor;
                let is_selected = self.selected.contains(&i);
                let is_drop_target = self.drop_indicator == Some(i);
                let is_hovered = self.hover_index == Some(i) && !is_cursor && !is_selected;
                let is_favourite = self
                    .favourites
                    .is_some_and(|f| f.contains(&self.entries[i].path));
                let line = render_track_line(
                    &self.entries[i],
                    is_cursor,
                    is_selected,
                    is_drop_target,
                    is_hovered,
                    is_favourite,
                    self.theme,
                    self.spinner_tick,
                );
                display_lines.push((Some(i), line));
            }
        }

        // Find which display line the cursor is on for scroll.
        let cursor_display_line = display_lines
            .iter()
            .position(|(idx, _)| *idx == Some(self.cursor))
            .unwrap_or(0);

        let visible_height = inner.height as usize;
        let start = if is_edit {
            let mut s = self.scroll_offset;
            if cursor_display_line < s {
                s = cursor_display_line;
            } else if cursor_display_line >= s + visible_height {
                s = cursor_display_line.saturating_sub(visible_height) + 1;
            }
            s
        } else {
            self.scroll_offset
        };
        let end = (start + visible_height).min(display_lines.len());

        for (row, (_idx, line)) in display_lines
            .iter()
            .skip(start)
            .take(end - start)
            .enumerate()
        {
            buf.set_line(inner.x, inner.y + row as u16, line, inner.width);
        }

        // Scrollbar — only when content overflows.
        // Uses sub-pixel rendering with 1/8th-cell resolution via Unicode block elements.
        let total_lines = display_lines.len();
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

fn render_album_header<'a>(entry: &QueueEntry, theme: &Theme) -> Line<'a> {
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

    let artist_style = theme.album_header_artist;
    let album_style = theme.album_header_album;
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

#[allow(clippy::too_many_arguments)]
fn render_track_line<'a>(
    entry: &QueueEntry,
    is_cursor: bool,
    is_selected: bool,
    is_drop_target: bool,
    is_hovered: bool,
    is_favourite: bool,
    theme: &Theme,
    spinner_tick: usize,
) -> Line<'a> {
    let spin_char = SPINNER[spinner_tick % SPINNER.len()];

    let progress = entry.download_progress.as_ref();

    let status_icon = match entry.status {
        QueueEntryStatus::Queued => Span::raw(" "),
        QueueEntryStatus::Playing => Span::styled(">", theme.track_playing),
        QueueEntryStatus::Played => {
            if is_selected {
                Span::styled(" ", theme.track_selected)
            } else {
                Span::raw(" ")
            }
        }
        QueueEntryStatus::Downloading => {
            if let Some(&(downloaded, total)) = progress {
                if total > 0 {
                    let pct = (downloaded * 100 / total).min(99);
                    Span::styled(format!("{:2}%", pct), theme.spinner)
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
                if total > 0 {
                    let pct = (downloaded * 100 / total).min(99);
                    Span::styled(format!("{:2}%", pct), theme.track_playing)
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

    let artist_part = if !entry.artist.is_empty() && entry.artist != entry.album_artist {
        let artist_style = if is_selected {
            theme.track_selected
        } else if is_hovered {
            theme.track_hover
        } else {
            theme.track_playing
        };
        let sep_style = if is_selected {
            theme.track_selected
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
        Span::styled(">", theme.track_cursor)
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
    } else {
        theme.track_normal
    };

    let num_style = if is_selected {
        theme.track_selected
    } else {
        theme.track_number
    };

    let dur_style = if is_selected {
        theme.track_selected
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
        .position(|(idx, is_header)| *idx == Some(cursor) && !is_header)
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
        .position(|(idx, is_header)| *idx == Some(cursor) && !is_header)
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
