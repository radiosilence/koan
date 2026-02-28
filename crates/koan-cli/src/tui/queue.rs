use std::collections::{HashMap, HashSet};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use koan_core::player::state::{QueueEntry, QueueEntryStatus};

use super::app::Mode;
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
    drag_target: Option<usize>,
    selected: &'a HashSet<usize>,
    spinner_tick: usize,
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
            drag_target: None,
            selected,
            spinner_tick,
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

        let display_lines = build_display_lines(entries);

        // +1 to account for the block border (TOP) consuming the first row.
        let target_line = scroll_offset + rel_y.saturating_sub(1);
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

        // Pre-compute whether each album group is fully played.
        let mut album_fully_played: HashMap<(String, String), bool> = HashMap::new();
        for entry in self.entries {
            if !entry.album.is_empty() {
                let key = (entry.album_artist.clone(), entry.album.clone());
                let fully_played = album_fully_played.entry(key).or_insert(true);
                if entry.status != QueueEntryStatus::Played {
                    *fully_played = false;
                }
            }
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
                current_album_key = Some(album_key.clone());
                let played = album_fully_played.get(&album_key).copied().unwrap_or(false);
                let header = render_album_header(entry, played, self.theme);
                display_lines.push((None, header));
            }

            let is_cursor = is_edit && i == self.cursor;
            let is_selected = self.selected.contains(&i);
            let is_drag_target = self.drag_target == Some(i);
            let line = render_track_line(
                entry,
                is_cursor,
                is_selected,
                is_drag_target,
                self.theme,
                self.spinner_tick,
            );
            display_lines.push((Some(i), line));
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
    }
}

fn render_album_header<'a>(entry: &QueueEntry, played: bool, theme: &Theme) -> Line<'a> {
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

    let maybe_dim = |s| if played { theme.dim(s) } else { s };
    let artist_style = maybe_dim(theme.album_header_artist);
    let album_style = maybe_dim(theme.album_header_album);
    let dim = maybe_dim(theme.hint_desc);

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
    is_cursor: bool,
    is_selected: bool,
    is_drag_target: bool,
    theme: &Theme,
    spinner_tick: usize,
) -> Line<'a> {
    let is_played = entry.status == QueueEntryStatus::Played;
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

    let maybe_dim = |s| if is_played { theme.dim(s) } else { s };

    let artist_part = if !entry.artist.is_empty() && entry.artist != entry.album_artist {
        let artist_style = if is_selected {
            theme.track_selected
        } else {
            maybe_dim(theme.track_playing)
        };
        let sep_style = if is_selected {
            theme.track_selected
        } else {
            maybe_dim(theme.hint_desc)
        };
        vec![
            Span::styled(entry.artist.clone(), artist_style),
            Span::styled(" \u{2014} ", sep_style),
        ]
    } else {
        vec![]
    };

    // Visual markers: cursor > selected, with drag insert line.
    let cursor_marker = if is_cursor {
        Span::styled(">", theme.track_cursor)
    } else if is_drag_target {
        Span::styled("\u{2500}", theme.track_playing)
    } else if is_selected {
        Span::styled("\u{2502}", theme.track_selected)
    } else {
        Span::raw(" ")
    };

    let title_style = if is_cursor {
        theme.track_cursor
    } else if is_selected {
        theme.track_selected
    } else {
        maybe_dim(theme.track_normal)
    };

    let num_style = if is_selected {
        theme.track_selected
    } else {
        maybe_dim(theme.track_number)
    };

    let dur_style = if is_selected {
        theme.track_selected
    } else {
        maybe_dim(theme.hint_desc)
    };

    let mut spans = vec![
        Span::raw("  "),
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
