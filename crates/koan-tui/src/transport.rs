use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use koan_core::player::state::{PlaybackState, QueueEntry, TrackInfo};

use super::theme::Theme;

/// Format audio quality info as a human-readable string.
///
/// Examples: "CD quality", "FLAC 96kHz/24bit", "Opus 48kHz/128kbps", "MP3 320kbps"
///
/// `output_rate` is what the device settled at. Where it differs from the
/// source, the string says so — a device that refused the rate is being fed
/// resampled audio, and the one claim this player cannot afford to leave
/// unqualified is the one about what reaches the DAC.
#[allow(clippy::manual_is_multiple_of)]
pub fn format_quality(info: &TrackInfo, output_rate: Option<u32>) -> String {
    let resampled = match output_rate {
        Some(out) if out != info.sample_rate => format!(" → {}", rate_label(out)),
        _ => String::new(),
    };
    format!("{}{}", format_source(info), resampled)
}

fn rate_label(rate: u32) -> String {
    match rate {
        44100 => "44.1kHz".to_string(),
        48000 => "48kHz".to_string(),
        88200 => "88.2kHz".to_string(),
        96000 => "96kHz".to_string(),
        176400 => "176.4kHz".to_string(),
        192000 => "192kHz".to_string(),
        352800 => "352.8kHz".to_string(),
        384000 => "384kHz".to_string(),
        _ if rate.is_multiple_of(1000) => format!("{}kHz", rate / 1000),
        _ => format!("{}Hz", rate),
    }
}

fn format_source(info: &TrackInfo) -> String {
    let rate = info.sample_rate;
    let ch = info.channels;
    let rate_str = rate_label(rate);

    // Red-book audio gets its own name.
    match (rate, info.bit_depth, ch) {
        (44100, Some(16), 2) => return format!("{} · CD quality", info.codec),
        (44100, Some(16), 1) => return format!("{} · CD quality (mono)", info.codec),
        _ => {}
    }

    // Detail: bit depth for lossless, bitrate for lossy.
    let detail = if let Some(b) = info.bit_depth {
        format!("/{}bit", b)
    } else if let Some(kbps) = info.bitrate_kbps {
        format!("/{}kbps", kbps)
    } else {
        String::new()
    };

    let ch_str = if ch == 1 {
        "mono"
    } else if ch == 2 {
        "stereo"
    } else {
        return format!("{} {}{}/{}ch", info.codec, rate_str, detail, ch);
    };

    format!("{} {}{} {}", info.codec, rate_str, detail, ch_str)
}

/// The playhead. A cell of its own rather than the end of the played run:
/// twenty seconds into nine hours is a tenth of a percent of the bar, which
/// rounds to no cells at any width a terminal has, and a bar with no mark on
/// it says nothing about where playback is.
const HEAD: &str = "\u{25CF}";

/// The seek bar as cell counts, either side of the playhead: what has played,
/// what can still be reached, and what has not arrived yet. The three plus the
/// head fill the bar exactly.
struct BarRegions {
    played: usize,
    reachable: usize,
    pending: usize,
}

impl BarRegions {
    /// `seekable_ms` is where a seek stops short while a download is still
    /// arriving, and `None` once the whole track is reachable.
    ///
    /// The dimming falls on what has not arrived rather than on what has, so
    /// that a track already on disk — the ordinary case — draws as the
    /// ordinary bar, and a download finishing changes nothing about it. Lit
    /// the other way round, a bar goes dark the moment its transfer completes.
    fn new(bar_width: usize, position_ms: u64, duration_ms: u64, seekable_ms: Option<u64>) -> Self {
        if bar_width == 0 {
            return Self {
                played: 0,
                reachable: 0,
                pending: 0,
            };
        }
        let cells = |ms: u64| {
            let frac = if duration_ms > 0 {
                ms as f64 / duration_ms as f64
            } else {
                0.0
            };
            ((frac * bar_width as f64) as usize).min(bar_width)
        };

        let tail = bar_width - 1; // everything but the head
        let played = cells(position_ms).min(tail);
        let reachable = seekable_ms
            .map_or(bar_width, cells)
            .saturating_sub(played + 1)
            .min(tail - played);
        Self {
            played,
            reachable,
            pending: tail - played - reachable,
        }
    }
}

pub struct TransportBar<'a> {
    track_info: Option<&'a TrackInfo>,
    playing_entry: Option<&'a QueueEntry>,
    playback_state: PlaybackState,
    position_ms: u64,
    theme: &'a Theme,
    ticker_offset: usize,
    /// How far into the track a seek can land. `None` once the whole track is
    /// reachable, which is every track that is not mid-download.
    seekable_ms: Option<u64>,
    /// What the output device settled at. None until a track has started.
    output_rate: Option<u32>,
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
            seekable_ms: None,
            output_rate: None,
        }
    }

    pub fn with_ticker_offset(mut self, offset: usize) -> Self {
        self.ticker_offset = offset;
        self
    }

    pub fn with_seekable_ms(mut self, seekable_ms: Option<u64>) -> Self {
        self.seekable_ms = seekable_ms;
        self
    }

    pub fn with_output_rate(mut self, rate: Option<u32>) -> Self {
        self.output_rate = rate;
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
    /// `seekable_ms` is the engine's own ceiling, so a click past the downloaded
    /// extent lands where playback will actually be rather than being refused.
    pub fn seek_from_click(
        bar_start: u16,
        bar_width: u16,
        click_x: u16,
        duration_ms: u64,
        seekable_ms: Option<u64>,
    ) -> Option<u64> {
        let bar_end = bar_start + bar_width;
        if click_x < bar_start || click_x >= bar_end || bar_width == 0 {
            return None;
        }
        let frac = (click_x - bar_start) as f64 / bar_width as f64;
        let pos = (frac * duration_ms as f64) as u64;
        Some(pos.min(seekable_ms.unwrap_or(u64::MAX)))
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
            PlaybackState::Playing => Span::styled("\u{25B8}\u{25B8}", self.theme.status_playing),
            PlaybackState::Paused => Span::styled("\u{2016} ", self.theme.status_paused),
            PlaybackState::Stopped => Span::styled("\u{25AA} ", self.theme.status_stopped),
        };

        // Prefer the queue entry's database-sourced duration over the probed
        // duration — probing a partial streaming file can give a wrong value.
        let duration_ms = self
            .playing_entry
            .and_then(|e| e.duration_ms)
            .unwrap_or(info.duration_ms);

        let time_str = format!(
            "{}/{}",
            format_time(self.position_ms),
            format_time(duration_ms)
        );

        // Bar width: total - " " - icon(2) - " " - " " - time
        let chrome_width = 1 + 2 + 1 + 1 + time_str.len() as u16;
        let bar_width = area.width.saturating_sub(chrome_width) as usize;

        let bar = BarRegions::new(bar_width, self.position_ms, duration_ms, self.seekable_ms);

        let mut spans = vec![Span::raw(" "), status_icon, Span::raw(" ")];
        if bar_width > 0 {
            spans.push(Span::styled(
                "\u{2501}".repeat(bar.played),
                self.theme.progress_filled,
            ));
            spans.push(Span::styled(HEAD, self.theme.progress_filled));
            spans.push(Span::styled(
                "\u{2500}".repeat(bar.reachable),
                self.theme.progress_empty,
            ));
            if bar.pending > 0 {
                spans.push(Span::styled(
                    "\u{2500}".repeat(bar.pending),
                    self.theme.progress_empty.add_modifier(Modifier::DIM),
                ));
            }
        }
        spans.push(Span::raw(" "));
        spans.push(Span::styled(time_str, self.theme.hint_desc));

        let progress_line = Line::from(spans);
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
                if let Some(s) = run_style
                    && !run_text.is_empty()
                {
                    ratatui_spans.push(Span::styled(run_text, s));
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

                let format_info = format!(" \u{00B7} {}", format_quality(info, self.output_rate));
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

            let format_info = format_quality(info, self.output_rate);

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

#[cfg(test)]
mod tests {
    use super::*;

    const NINE_HOURS: u64 = 9 * 60 * 60 * 1000;

    fn regions(position_ms: u64, duration_ms: u64, seekable_ms: Option<u64>) -> BarRegions {
        BarRegions::new(60, position_ms, duration_ms, seekable_ms)
    }

    #[test]
    fn the_regions_and_the_head_fill_the_bar() {
        for pos in [0, 1, 30_000, NINE_HOURS / 2, NINE_HOURS] {
            for seekable in [None, Some(0), Some(NINE_HOURS / 3), Some(NINE_HOURS)] {
                let b = regions(pos, NINE_HOURS, seekable);
                assert_eq!(
                    b.played + 1 + b.reachable + b.pending,
                    60,
                    "{pos} {seekable:?}"
                );
            }
        }
    }

    #[test]
    fn a_position_too_small_to_draw_still_has_a_head() {
        // Twenty seconds into nine hours is a tenth of a percent of the bar.
        let b = regions(20_000, NINE_HOURS, None);
        assert_eq!(b.played, 0);
        assert_eq!(b.reachable, 59);
    }

    #[test]
    fn the_head_stays_on_the_bar_at_the_end_of_the_track() {
        let b = regions(NINE_HOURS, NINE_HOURS, None);
        assert_eq!(b.played, 59);
        assert_eq!(b.reachable, 0);
        assert_eq!(b.pending, 0);
    }

    #[test]
    fn a_track_on_disk_has_nothing_dimmed() {
        let b = regions(NINE_HOURS / 2, NINE_HOURS, None);
        assert_eq!(b.pending, 0);
    }

    #[test]
    fn what_has_not_arrived_is_what_is_dimmed() {
        let b = regions(0, 100_000, Some(25_000));
        assert_eq!(b.reachable, 14); // 15 cells reachable, one of them the head
        assert_eq!(b.pending, 45);
    }

    #[test]
    fn a_track_reachable_nowhere_is_dim_the_whole_way() {
        let b = regions(0, 100_000, Some(0));
        assert_eq!(b.played, 0);
        assert_eq!(b.reachable, 0);
        assert_eq!(b.pending, 59);
    }

    /// What the bar actually draws, as one string, for a track twenty seconds
    /// into nine hours — the case a played run alone cannot show.
    fn rendered(position_ms: u64, duration_ms: u64, seekable_ms: Option<u64>) -> String {
        let theme = Theme::default();
        let info = TrackInfo {
            id: koan_core::player::state::QueueItemId::new(),
            path: std::path::PathBuf::from("/x.flac"),
            codec: "FLAC".into(),
            sample_rate: 44100,
            bit_depth: Some(16),
            bitrate_kbps: None,
            channels: 2,
            duration_ms,
        };
        let area = Rect::new(0, 0, 60, 2);
        let mut buf = Buffer::empty(area);
        TransportBar::new(
            Some(&info),
            None,
            PlaybackState::Playing,
            position_ms,
            &theme,
        )
        .with_seekable_ms(seekable_ms)
        .render(area, &mut buf);
        (0..area.width)
            .map(|x| buf[(x, 0)].symbol())
            .collect::<String>()
    }

    #[test]
    fn the_head_is_drawn_where_a_played_run_would_show_nothing() {
        let bar = rendered(20_000, NINE_HOURS, None);
        assert!(bar.contains(HEAD), "{bar}");
        // Four cells of chrome, then the head at the very start of the bar.
        assert_eq!(bar.chars().nth(4).unwrap().to_string(), HEAD, "{bar}");
    }

    #[test]
    fn a_bar_with_no_room_draws_nothing() {
        let b = BarRegions::new(0, 30_000, 100_000, None);
        assert_eq!((b.played, b.reachable, b.pending), (0, 0, 0));
    }
}
