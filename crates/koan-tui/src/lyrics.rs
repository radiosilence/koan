use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};

use koan_core::lyrics::{LrcLine, Lyrics, current_line_index, parse_lrc};

use super::theme::Theme;

/// Lyrics display state, held by App.
#[derive(Default)]
pub struct LyricsState {
    /// Current lyrics result (if any).
    pub result: Option<Lyrics>,
    /// Parsed synced lines (empty if plain lyrics or no lyrics).
    pub lrc_lines: Vec<LrcLine>,
    /// The track path for which we fetched lyrics (to detect track changes).
    pub track_path: Option<std::path::PathBuf>,
    /// Whether a fetch is currently in progress.
    pub fetching: bool,
}

impl LyricsState {
    /// Update lyrics from a fetch result. Parses LRC if synced.
    pub fn set_result(&mut self, result: Option<Lyrics>) {
        if let Some(ref r) = result {
            if r.synced {
                self.lrc_lines = parse_lrc(&r.content);
            } else {
                self.lrc_lines.clear();
            }
        } else {
            self.lrc_lines.clear();
        }
        self.result = result;
        self.fetching = false;
    }
}

/// Widget for rendering lyrics in a side panel.
pub struct LyricsPanel<'a> {
    lyrics: &'a LyricsState,
    position_ms: u64,
    theme: &'a Theme,
    spinner_tick: usize,
}

impl<'a> LyricsPanel<'a> {
    pub fn new(
        lyrics: &'a LyricsState,
        position_ms: u64,
        theme: &'a Theme,
        spinner_tick: usize,
    ) -> Self {
        Self {
            lyrics,
            position_ms,
            theme,
            spinner_tick,
        }
    }
}

impl Widget for LyricsPanel<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::TOP | Borders::LEFT)
            .title(Line::from(Span::styled(" lyrics ", self.theme.hint_desc)));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // Loading state.
        if self.lyrics.fetching {
            const SPINNER: &[char] = &[
                '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
                '\u{2827}',
            ];
            let ch = SPINNER[self.spinner_tick % SPINNER.len()];
            let line = Line::from(Span::styled(
                format!(" {} fetching lyrics...", ch),
                self.theme.hint_desc,
            ));
            buf.set_line(inner.x, inner.y, &line, inner.width);
            return;
        }

        let Some(ref result) = self.lyrics.result else {
            let line = Line::from(Span::styled(" no lyrics", self.theme.hint_desc));
            buf.set_line(inner.x, inner.y, &line, inner.width);
            return;
        };

        if result.synced && !self.lyrics.lrc_lines.is_empty() {
            render_synced(
                &self.lyrics.lrc_lines,
                self.position_ms,
                inner,
                buf,
                self.theme,
            );
        } else {
            render_plain(&result.content, inner, buf, self.theme);
        }
    }
}

/// Render synced lyrics with current-line highlighting.
fn render_synced(lines: &[LrcLine], position_ms: u64, area: Rect, buf: &mut Buffer, theme: &Theme) {
    let position_secs = position_ms as f64 / 1000.0;
    let current = current_line_index(lines, position_secs);
    let visible_height = area.height as usize;

    if visible_height == 0 {
        return;
    }

    // Center the current line in the visible area.
    let current_idx = current.unwrap_or(0);
    let half = visible_height / 2;
    let start = current_idx.saturating_sub(half);

    for (row, i) in (start..lines.len()).enumerate() {
        if row >= visible_height {
            break;
        }

        let is_current = Some(i) == current;
        let style = if is_current {
            theme.track_playing.add_modifier(Modifier::BOLD)
        } else {
            theme.hint_desc
        };

        let prefix = if is_current { "> " } else { "  " };
        let text = truncate_to_width(&lines[i].text, area.width.saturating_sub(2) as usize);
        let line = Line::from(Span::styled(format!("{}{}", prefix, text), style));
        buf.set_line(area.x, area.y + row as u16, &line, area.width);
    }
}

/// Render plain (non-synced) lyrics as scrollable static text.
fn render_plain(content: &str, area: Rect, buf: &mut Buffer, theme: &Theme) {
    let visible_height = area.height as usize;

    for (row, text) in content.lines().enumerate() {
        if row >= visible_height {
            break;
        }
        let text = truncate_to_width(text, area.width.saturating_sub(2) as usize);
        let line = Line::from(Span::styled(format!("  {}", text), theme.track_normal));
        buf.set_line(area.x, area.y + row as u16, &line, area.width);
    }
}

/// Truncate a string to fit within a given display width (simple byte-based).
fn truncate_to_width(s: &str, max_width: usize) -> &str {
    if s.len() <= max_width {
        return s;
    }
    // Find a valid char boundary at or before max_width.
    let mut end = max_width;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
