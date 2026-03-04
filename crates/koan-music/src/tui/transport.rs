use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use koan_core::player::state::{PlaybackState, QueueEntry, TrackInfo};

use super::theme::Theme;

pub struct TransportBar<'a> {
    track_info: Option<&'a TrackInfo>,
    playing_entry: Option<&'a QueueEntry>,
    playback_state: PlaybackState,
    position_ms: u64,
    theme: &'a Theme,
    ticker_offset: usize,
}

impl<'a> TransportBar<'a> {
    pub fn new(
        track_info: Option<&'a TrackInfo>,
        playing_entry: Option<&'a QueueEntry>,
        playback_state: PlaybackState,
        position_ms: u64,
        theme: &'a Theme,
    ) -> Self {
        Self {
            track_info,
            playing_entry,
            playback_state,
            position_ms,
            theme,
            ticker_offset: 0,
        }
    }

    pub fn with_ticker_offset(mut self, offset: usize) -> Self {
        self.ticker_offset = offset;
        self
    }

    /// Compute the seek bar start (absolute x) and width for the given area.
    /// Call this before render and store the results for click-to-seek.
    pub fn bar_metrics(area: Rect, position_ms: u64, duration_ms: u64) -> (u16, u16) {
        let time_width =
            format!("{}/{}", format_time(position_ms), format_time(duration_ms)).len() as u16;
        let chrome_width = 1 + 2 + 1 + 1 + time_width;
        let bar_start = area.x + 4;
        let bar_width = area.width.saturating_sub(chrome_width);
        (bar_start, bar_width)
    }

    /// Seek from a click using the bar metrics stored from the last render.
    /// This guarantees the click handler uses the exact same bar layout as what's on screen.
    pub fn seek_from_click(
        bar_start: u16,
        bar_width: u16,
        click_x: u16,
        duration_ms: u64,
    ) -> Option<u64> {
        let bar_end = bar_start + bar_width;
        if click_x < bar_start || click_x >= bar_end || bar_width == 0 {
            return None;
        }
        let frac = (click_x - bar_start) as f64 / bar_width as f64;
        Some((frac * duration_ms as f64) as u64)
    }
}

impl Widget for TransportBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 {
            return;
        }

        let Some(info) = self.track_info else {
            // No track — render empty transport.
            let line = Line::from(Span::styled(" stopped", self.theme.status_stopped));
            buf.set_line(area.x, area.y, &line, area.width);
            return;
        };

        // Line 1: status icon + seek bar + time
        let status_icon = match self.playback_state {
            PlaybackState::Playing => Span::styled(">>", self.theme.status_playing),
            PlaybackState::Paused => Span::styled("||", self.theme.status_paused),
            PlaybackState::Stopped => Span::styled("[]", self.theme.status_stopped),
        };

        let time_str = format!(
            "{}/{}",
            format_time(self.position_ms),
            format_time(info.duration_ms)
        );

        // Bar width: total - " " - icon(2) - " " - " " - time
        let chrome_width = 1 + 2 + 1 + 1 + time_str.len() as u16;
        let bar_width = area.width.saturating_sub(chrome_width) as usize;

        let progress = if info.duration_ms > 0 {
            ((self.position_ms as f64 / info.duration_ms as f64) * bar_width as f64) as usize
        } else {
            0
        }
        .min(bar_width);

        let filled = "\u{2501}".repeat(progress);
        let remaining = "\u{2500}".repeat(bar_width.saturating_sub(progress));

        let progress_line = Line::from(vec![
            Span::raw(" "),
            status_icon,
            Span::raw(" "),
            Span::styled(filled, self.theme.progress_filled),
            Span::styled(remaining, self.theme.progress_empty),
            Span::raw(" "),
            Span::styled(time_str, self.theme.hint_desc),
        ]);
        buf.set_line(area.x, area.y, &progress_line, area.width);

        // Line 2: Artist — Title (from QueueEntry metadata, or fallback to filename)
        if let Some(entry) = self.playing_entry {
            let mut spans = Vec::new();

            if !entry.artist.is_empty() {
                spans.push(StyledSegment {
                    text: entry.artist.clone(),
                    style: self.theme.track_playing,
                });
                spans.push(StyledSegment {
                    text: " \u{2014} ".into(),
                    style: self.theme.hint_desc,
                });
            }

            spans.push(StyledSegment {
                text: entry.title.clone(),
                style: self.theme.track_normal.add_modifier(Modifier::BOLD),
            });

            let total_width: usize = spans.iter().map(|s| s.text.chars().count()).sum();
            let avail = area.width.saturating_sub(1) as usize; // -1 for leading space

            if total_width <= avail {
                // Fits — render normally.
                let mut ratatui_spans = vec![Span::raw(" ")];
                for seg in &spans {
                    ratatui_spans.push(Span::styled(seg.text.clone(), seg.style));
                }
                let title_line = Line::from(ratatui_spans);
                buf.set_line(area.x, area.y + 1, &title_line, area.width);
            } else {
                // Ticker mode — scroll the title text.
                let separator = "   \u{00B7}   "; // " · "
                let sep_len = separator.chars().count();
                let cycle_len = total_width + sep_len;
                let offset = self.ticker_offset % cycle_len;

                // Build full ticker character buffer with styles.
                let mut chars: Vec<(char, Style)> = Vec::with_capacity(cycle_len);
                for seg in &spans {
                    for c in seg.text.chars() {
                        chars.push((c, seg.style));
                    }
                }
                for c in separator.chars() {
                    chars.push((c, self.theme.hint_desc));
                }

                // Extract a window of `avail` characters starting at `offset`.
                let mut ratatui_spans = vec![Span::raw(" ")];
                let mut run_text = String::new();
                let mut run_style: Option<Style> = None;

                for i in 0..avail {
                    let idx = (offset + i) % cycle_len;
                    let (ch, style) = chars[idx];

                    if run_style == Some(style) {
                        run_text.push(ch);
                    } else {
                        if let Some(s) = run_style {
                            ratatui_spans.push(Span::styled(run_text.clone(), s));
                        }
                        run_text.clear();
                        run_text.push(ch);
                        run_style = Some(style);
                    }
                }
                if let Some(s) = run_style {
                    if !run_text.is_empty() {
                        ratatui_spans.push(Span::styled(run_text, s));
                    }
                }

                let title_line = Line::from(ratatui_spans);
                buf.set_line(area.x, area.y + 1, &title_line, area.width);
            }

            // Line 3: Album (Year) · codec info (if we have enough height)
            if area.height >= 3 {
                let mut album_spans = vec![Span::raw(" ")];

                if !entry.album.is_empty() {
                    album_spans.push(Span::styled(
                        entry.album.clone(),
                        self.theme.album_header_album,
                    ));
                }

                if let Some(ref year) = entry.year {
                    album_spans.push(Span::styled(format!(" ({})", year), self.theme.hint_desc));
                }

                let format_info = format!(
                    " \u{00B7} {} {}Hz/{}bit/{}ch",
                    info.codec, info.sample_rate, info.bit_depth, info.channels
                );
                album_spans.push(Span::styled(format_info, self.theme.hint_desc));

                let album_line = Line::from(album_spans);
                buf.set_line(area.x, area.y + 2, &album_line, area.width);
            }
        } else {
            // Fallback: filename + codec info
            let artist = info
                .path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let format_info = format!(
                "{} {}Hz/{}bit/{}ch",
                info.codec, info.sample_rate, info.bit_depth, info.channels
            );

            let info_line = Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    &artist,
                    self.theme.track_normal.add_modifier(Modifier::BOLD),
                ),
                Span::styled("  ", self.theme.hint_desc),
                Span::styled(format_info, self.theme.hint_desc),
            ]);
            buf.set_line(area.x, area.y + 1, &info_line, area.width);
        }
    }
}

/// Internal helper for ticker: a piece of text with a style.
struct StyledSegment {
    text: String,
    style: Style,
}

pub fn format_time(ms: u64) -> String {
    let secs = ms / 1000;
    let mins = secs / 60;
    let secs = secs % 60;
    if mins >= 60 {
        let hours = mins / 60;
        let mins = mins % 60;
        format!("{}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{}:{:02}", mins, secs)
    }
}
