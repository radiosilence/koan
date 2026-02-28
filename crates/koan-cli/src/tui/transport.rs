use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
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
        }
    }

    /// Calculate seek position from a click's x coordinate within the transport area.
    /// Returns None if no track is playing or click is outside the gauge.
    pub fn seek_from_click(
        area: Rect,
        click_x: u16,
        track_info: &TrackInfo,
        _position_ms: u64,
    ) -> Option<u64> {
        // Must match render layout exactly:
        //   " " icon(2) " " [===bar===] " " time
        // Use duration for time width since position length varies.
        let time_width =
            format!("{}/{}", format_time(0), format_time(track_info.duration_ms)).len() as u16;
        let chrome_width = 1 + 2 + 1 + 1 + time_width;
        let bar_width = area.width.saturating_sub(chrome_width);
        let bar_start = area.x + 4; // " " + icon(2) + " "
        let bar_end = bar_start + bar_width;

        if click_x < bar_start || click_x >= bar_end || bar_width == 0 {
            return None;
        }

        // Use center of cell for sub-cell accuracy.
        let frac = ((click_x - bar_start) as f64 + 0.5) / bar_width as f64;
        Some((frac * track_info.duration_ms as f64) as u64)
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
            let mut spans = vec![Span::raw(" ")];

            if !entry.artist.is_empty() {
                spans.push(Span::styled(entry.artist.clone(), self.theme.track_playing));
                spans.push(Span::styled(" \u{2014} ", self.theme.hint_desc));
            }

            spans.push(Span::styled(
                entry.title.clone(),
                self.theme.track_normal.add_modifier(Modifier::BOLD),
            ));

            let title_line = Line::from(spans);
            buf.set_line(area.x, area.y + 1, &title_line, area.width);

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
