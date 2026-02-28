use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use koan_core::player::state::{QueueEntry, QueueEntryStatus};

use super::app::Mode;
use super::theme::Theme;
use super::transport::format_time;

const SPINNER_FRAMES: &[char] = &[
    '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}', '\u{2827}',
    '\u{2807}', '\u{280F}',
];

pub struct QueueView<'a> {
    entries: &'a [QueueEntry],
    mode: &'a Mode,
    cursor: usize,
    scroll_offset: usize,
    spinner_tick: usize,
    theme: &'a Theme,
    drag_target: Option<usize>,
}

impl<'a> QueueView<'a> {
    pub fn new(
        entries: &'a [QueueEntry],
        mode: &'a Mode,
        cursor: usize,
        scroll_offset: usize,
        spinner_tick: usize,
        theme: &'a Theme,
    ) -> Self {
        Self {
            entries,
            mode,
            cursor,
            scroll_offset,
            spinner_tick,
            theme,
            drag_target: None,
        }
    }

    pub fn with_drag_target(mut self, target: Option<usize>) -> Self {
        self.drag_target = target;
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

        // Build display lines to figure out which queue index is at this y.
        let display_lines = build_display_lines(entries);

        let target_line = scroll_offset + rel_y;
        if target_line < display_lines.len() {
            display_lines[target_line].0
        } else {
            None
        }
    }
}

/// Each display line: (Option<queue_index>, is_header)
fn build_display_lines(entries: &[QueueEntry]) -> Vec<(Option<usize>, bool)> {
    let mut lines = Vec::new();
    let mut current_album_key: Option<(String, String)> = None;

    for (i, entry) in entries.iter().enumerate() {
        let album_key = (entry.album_artist.clone(), entry.album.clone());
        let show_header = if entry.album.is_empty() {
            false
        } else {
            current_album_key.as_ref() != Some(&album_key)
        };

        if show_header {
            current_album_key = Some(album_key);
            lines.push((None, true));
        }

        lines.push((Some(i), false));
    }
    lines
}

impl Widget for QueueView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let is_edit = matches!(self.mode, Mode::QueueEdit);

        // Build header label.
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

        // Build all display lines.
        let mut display_lines: Vec<(Option<usize>, Line)> = Vec::new();
        let mut current_album_key: Option<(String, String)> = None;

        for (i, entry) in self.entries.iter().enumerate() {
            let album_key = (entry.album_artist.clone(), entry.album.clone());
            let show_header = if entry.album.is_empty() {
                false
            } else {
                current_album_key.as_ref() != Some(&album_key)
            };

            if show_header {
                current_album_key = Some(album_key);
                let header = render_album_header(entry, self.theme);
                display_lines.push((None, header));
            }

            let is_cursor = is_edit && i == self.cursor;
            let is_drag_target = self.drag_target == Some(i);
            let line = render_track_line(
                i,
                entry,
                is_cursor,
                is_drag_target,
                self.spinner_tick,
                self.theme,
            );
            display_lines.push((Some(i), line));
        }

        // Find which display line the cursor is on for scroll.
        let cursor_display_line = display_lines
            .iter()
            .position(|(idx, _)| *idx == Some(self.cursor))
            .unwrap_or(0);

        // Calculate scroll window.
        let visible_height = inner.height as usize;
        let start = if is_edit {
            // Keep cursor visible.
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

    Line::from(vec![
        Span::raw(" "),
        Span::styled(entry.album_artist.clone(), theme.album_header_artist),
        Span::styled(" \u{2014} ", theme.hint_desc),
        Span::styled(year, theme.hint_desc),
        Span::styled(entry.album.clone(), theme.album_header_album),
        Span::styled(codec, theme.hint_desc),
    ])
}

fn render_track_line<'a>(
    _index: usize,
    entry: &QueueEntry,
    is_cursor: bool,
    is_drag_target: bool,
    spinner_tick: usize,
    theme: &Theme,
) -> Line<'a> {
    let status_icon = match entry.status {
        QueueEntryStatus::Queued => Span::raw(" "),
        QueueEntryStatus::Playing => Span::styled(">", theme.track_playing),
        QueueEntryStatus::Downloading => {
            let frame = SPINNER_FRAMES[spinner_tick % SPINNER_FRAMES.len()];
            Span::styled(frame.to_string(), theme.spinner)
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
        vec![
            Span::styled(entry.artist.clone(), theme.track_playing),
            Span::styled(" \u{2014} ", theme.hint_desc),
        ]
    } else {
        vec![]
    };

    let cursor_marker = if is_cursor {
        Span::styled(">", theme.track_cursor)
    } else if is_drag_target {
        Span::styled("\u{2500}", theme.track_playing)
    } else {
        Span::raw(" ")
    };

    let title_style = if is_cursor {
        theme.track_cursor
    } else {
        theme.track_normal
    };

    let mut spans = vec![
        Span::raw("  "),
        cursor_marker,
        status_icon,
        Span::raw(" "),
        Span::styled(track_num, theme.track_number),
        Span::raw(" "),
    ];
    spans.extend(artist_part);
    spans.push(Span::styled(entry.title.clone(), title_style));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(dur, theme.hint_desc));

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
        .position(|(idx, _)| *idx == Some(cursor))
        .unwrap_or(0);

    if cursor_line < current_scroll {
        cursor_line
    } else if cursor_line >= current_scroll + visible_height {
        cursor_line.saturating_sub(visible_height) + 1
    } else {
        current_scroll
    }
}
