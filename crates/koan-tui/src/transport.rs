//! The transport, as the Mac's bar lays it out: what is playing on the left,
//! the controls and the seek bar in the middle, and on the right what the
//! output is handed and where it goes. Every part of it is a click as well as
//! a key.

use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use koan_core::audio::dsp::DspStatus;
use koan_core::player::state::{PlaybackState, QueueEntry, Repeat, Sleep, TrackInfo};

use super::theme::Theme;

/// Format audio quality info as a human-readable string.
///
/// Examples: "CD quality", "FLAC 96kHz/24bit", "Opus 48kHz/128kbps", "MP3 320kbps"
///
/// `output_rate` is what the device settled at. Where it differs from the
/// source, the string says so — a device that refused the rate is being fed
/// resampled audio, and the one claim this player cannot afford to leave
/// unqualified is the one about what reaches the DAC. DSP is named for the
/// same reason.
pub fn format_quality(
    info: &TrackInfo,
    output_rate: Option<u32>,
    dsp: Option<&DspStatus>,
) -> String {
    let resampled = match output_rate {
        Some(out) if out != info.sample_rate => format!(" → {}", rate_label(out)),
        _ => String::new(),
    };
    let processed = match dsp {
        Some(d) => format!(" · {}", dsp_label(d)),
        None => String::new(),
    };
    format!("{}{}{}", format_source(info), resampled, processed)
}

pub fn dsp_label(d: &DspStatus) -> &'static str {
    match (d.eq, d.convolution_rate.is_some()) {
        (true, true) => "EQ + FIR",
        (false, true) => "FIR",
        _ => "EQ",
    }
}

/// The format badge the apps show: codec, depth over rate in kHz, and the
/// rate the device was given where it differs.
pub fn badge(info: &TrackInfo, output_rate: Option<u32>) -> String {
    let mut s = info.codec.to_uppercase();
    match info.bit_depth {
        Some(depth) => s.push_str(&format!(" {depth}/{}", khz(info.sample_rate))),
        None => s.push_str(&format!(" {} kHz", khz(info.sample_rate))),
    }
    if info.channels != 2 {
        s.push_str(&format!(" {}ch", info.channels));
    }
    if let Some(out) = output_rate.filter(|&out| out != info.sample_rate) {
        s.push_str(&format!(" → {}", khz(out)));
    }
    s
}

/// 44100 as "44.1", 96000 as "96".
pub fn khz(hz: u32) -> String {
    if hz.is_multiple_of(1000) {
        format!("{}", hz / 1000)
    } else {
        format!("{:.1}", f64::from(hz) / 1000.0)
    }
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

/// `s` cut to `width` cells, with an ellipsis where it was cut.
pub fn fit(s: &str, width: usize) -> Cow<'_, str> {
    if s.width() <= width {
        return Cow::Borrowed(s);
    }
    if width == 0 {
        return Cow::Borrowed("");
    }
    let mut cut = String::new();
    let mut used = 0;
    for c in s.chars() {
        used += c.width().unwrap_or(0);
        if used >= width {
            break;
        }
        cut.push(c);
    }
    cut.push('\u{2026}');
    Cow::Owned(cut)
}

/// The playhead. A cell of its own rather than the end of the played run:
/// twenty seconds into nine hours is a tenth of a percent of the bar, which
/// rounds to no cells at any width a terminal has, and a bar with no mark on
/// it says nothing about where playback is.
const HEAD: &str = "\u{25CF}";

/// The seek bar as cell counts, either side of the playhead: what has played,
/// what can still be reached, and what has not arrived yet. The three plus the
/// head fill the bar exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarRegions {
    pub played: usize,
    pub reachable: usize,
    pub pending: usize,
}

impl BarRegions {
    /// `seekable_ms` is where a seek stops short while a download is still
    /// arriving, and `None` once the whole track is reachable.
    ///
    /// The dimming falls on what has not arrived rather than on what has, so
    /// that a track already on disk — the ordinary case — draws as the
    /// ordinary bar, and a download finishing changes nothing about it. Lit
    /// the other way round, a bar goes dark the moment its transfer completes.
    pub fn new(
        bar_width: usize,
        position_ms: u64,
        duration_ms: u64,
        seekable_ms: Option<u64>,
    ) -> Self {
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

/// The seek bar, `elapsed ━━━●──── duration`, into `area`'s first row. Both
/// times take the width of the longer, so the bar holds still as they count.
/// Returns where the bar itself is, for a click to seek.
pub fn seek_bar(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    position_ms: u64,
    duration_ms: u64,
    seekable_ms: Option<u64>,
) -> Rect {
    let total = format_time(duration_ms);
    let elapsed = format_time(position_ms.min(duration_ms.max(position_ms)));
    let time_w = total.width().max(elapsed.width());
    let bar_w = area.width.saturating_sub(2 * time_w as u16 + 2);
    if area.width == 0 || area.height == 0 {
        return Rect::default();
    }
    if bar_w < 4 {
        let line = format!("{elapsed} / {total}");
        buf.set_stringn(area.x, area.y, &line, area.width as usize, theme.hint_desc);
        return Rect::default();
    }
    let bar = BarRegions::new(bar_w as usize, position_ms, duration_ms, seekable_ms);
    let line = Line::from(vec![
        Span::styled(format!("{elapsed:>time_w$} "), theme.hint_desc),
        Span::styled("\u{2501}".repeat(bar.played), theme.progress_filled),
        Span::styled(HEAD, theme.progress_filled),
        Span::styled("\u{2500}".repeat(bar.reachable), theme.progress_empty),
        Span::styled(
            "\u{2500}".repeat(bar.pending),
            theme.progress_empty.add_modifier(Modifier::DIM),
        ),
        Span::styled(format!(" {total:<time_w$}"), theme.hint_desc),
    ]);
    buf.set_line(area.x, area.y, &line, area.width);
    Rect::new(area.x + time_w as u16 + 1, area.y, bar_w, 1)
}

/// Seeks from a click on the bar `seek_bar` drew. `seekable_ms` is the
/// engine's own ceiling, so a click past the downloaded extent lands where
/// playback will be rather than being refused.
pub fn seek_from_click(
    bar: Rect,
    click_x: u16,
    duration_ms: u64,
    seekable_ms: Option<u64>,
) -> Option<u64> {
    if bar.width == 0 || click_x < bar.x || click_x >= bar.x + bar.width {
        return None;
    }
    // The middle of the cell, so the head lands on the cell clicked.
    let frac = (f64::from(click_x - bar.x) + 0.5) / f64::from(bar.width);
    let pos = (frac * duration_ms as f64) as u64;
    Some(pos.min(seekable_ms.unwrap_or(u64::MAX)))
}

/// What a click on a part of the transport, or of Now playing, does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    PlayPause,
    Previous,
    Next,
    Shuffle,
    Repeat,
    /// The playing track's heart.
    Favourite,
    Sleep,
    /// The output chooser.
    Output,
    /// The EQ chooser.
    Eq,
    Artist,
    Album,
    /// The cover, full size.
    Cover,
}

/// Where each control was drawn, and the seek bar.
#[derive(Debug, Default, Clone)]
pub struct Hits {
    pub controls: Vec<(Rect, Control)>,
    pub seek: Rect,
}

impl Hits {
    pub fn at(&self, x: u16, y: u16) -> Option<Control> {
        self.controls
            .iter()
            .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .map(|&(_, c)| c)
    }
}

/// A row's pieces, each optionally a control.
pub(crate) type Parts<'a> = Vec<(Span<'a>, Option<Control>)>;

/// Spans laid along a row from `x`, each optionally a control. Returns the
/// row's width.
pub(crate) fn place(
    buf: &mut Buffer,
    hits: &mut Vec<(Rect, Control)>,
    x: u16,
    y: u16,
    max: u16,
    parts: &[(Span<'_>, Option<Control>)],
) -> u16 {
    let mut at = x;
    for (span, control) in parts {
        let room = (x + max).saturating_sub(at);
        if room == 0 {
            break;
        }
        let text = fit(&span.content, room as usize);
        let w = text.width() as u16;
        buf.set_stringn(at, y, &text, room as usize, span.style);
        if let Some(c) = control {
            hits.push((Rect::new(at, y, w, 1), *c));
        }
        at += w;
    }
    at - x
}

/// The width `parts` take.
pub(crate) fn width_of(parts: &[(Span<'_>, Option<Control>)]) -> u16 {
    parts.iter().map(|(s, _)| s.width() as u16).sum()
}

/// The five transport controls: shuffle and repeat either side of previous,
/// play or pause, and next. Play is the accent, as the apps' outlined button
/// is the one prominent control.
pub(crate) fn controls(
    theme: &Theme,
    playing: bool,
    shuffle: bool,
    repeat: Repeat,
    gap: &'static str,
) -> Parts<'static> {
    let lit = |on: bool| {
        if on {
            Style::new().fg(theme.tokens.accent)
        } else {
            theme.hint_desc
        }
    };
    let repeat_glyph = if repeat == Repeat::One {
        "\u{21BB}1"
    } else {
        "\u{21BB}"
    };
    vec![
        (
            Span::styled("\u{21C4}", lit(shuffle)),
            Some(Control::Shuffle),
        ),
        (Span::raw(gap), None),
        (
            Span::styled("|\u{25C2}", theme.track_normal),
            Some(Control::Previous),
        ),
        (Span::raw(gap), None),
        (
            Span::styled(
                if playing { "\u{2016}" } else { "\u{25B6}" },
                Style::new()
                    .fg(theme.tokens.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Some(Control::PlayPause),
        ),
        (Span::raw(gap), None),
        (
            Span::styled("\u{25B8}|", theme.track_normal),
            Some(Control::Next),
        ),
        (Span::raw(gap), None),
        (
            Span::styled(repeat_glyph, lit(repeat != Repeat::Off)),
            Some(Control::Repeat),
        ),
    ]
}

/// The sleep timer as the apps show it: the moon, and what is left or what it
/// waits for.
pub(crate) fn sleep_label(sleep: Option<Sleep>, fading: bool) -> String {
    let what = match sleep {
        _ if fading => "fading".to_string(),
        Some(Sleep::At { unix_ms }) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            format_time(unix_ms.saturating_sub(now))
        }
        Some(Sleep::EndOfTrack) => "track".into(),
        Some(Sleep::EndOfRecord) => "album".into(),
        None => return "\u{263E}".into(),
    };
    format!("\u{263E} {what}")
}

/// How many bars the transport's own spectrum has.
pub const MINI_BARS: usize = 12;

/// Under this the transport stacks its parts rather than setting them in
/// three zones.
const ZONES_MIN_WIDTH: u16 = 72;

pub struct TransportBar<'a> {
    pub track_info: Option<&'a TrackInfo>,
    pub entry: Option<&'a QueueEntry>,
    pub playback_state: PlaybackState,
    pub position_ms: u64,
    pub theme: &'a Theme,
    /// How far into the track a seek can land. `None` once the whole track is
    /// reachable, which is every track that is not mid-download.
    pub seekable_ms: Option<u64>,
    /// What the output device settled at. None until a track has started.
    pub output_rate: Option<u32>,
    pub dsp: Option<DspStatus>,
    pub sleep: Option<Sleep>,
    pub sleep_fading: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// Whether the playing track is a favourite; `None` for one the library
    /// does not hold.
    pub favourite: Option<bool>,
    /// What the output is called, when it is worth naming.
    pub output: Option<String>,
    /// Whether the artist and album have pages to open.
    pub links: (bool, bool),
    pub levels: Option<[f32; MINI_BARS]>,
}

impl<'a> TransportBar<'a> {
    pub fn new(
        track_info: Option<&'a TrackInfo>,
        entry: Option<&'a QueueEntry>,
        playback_state: PlaybackState,
        position_ms: u64,
        theme: &'a Theme,
    ) -> Self {
        Self {
            track_info,
            entry,
            playback_state,
            position_ms,
            theme,
            seekable_ms: None,
            output_rate: None,
            dsp: None,
            sleep: None,
            sleep_fading: false,
            shuffle: false,
            repeat: Repeat::Off,
            favourite: None,
            output: None,
            links: (false, false),
            levels: None,
        }
    }

    pub fn with_seekable_ms(mut self, seekable_ms: Option<u64>) -> Self {
        self.seekable_ms = seekable_ms;
        self
    }

    /// The duration to draw against: the queue's, from the library, ahead
    /// of the probed one, which a partial streaming file gives short.
    fn duration_ms(&self) -> u64 {
        self.entry
            .and_then(|e| e.duration_ms)
            .or(self.track_info.map(|i| i.duration_ms))
            .unwrap_or(0)
    }

    fn heart(&self) -> Option<(Span<'static>, Option<Control>)> {
        let on = self.favourite?;
        Some(if on {
            (
                Span::styled("\u{2665}", Style::new().fg(self.theme.tokens.accent)),
                Some(Control::Favourite),
            )
        } else {
            (
                Span::styled("\u{2661}", self.theme.hint_desc),
                Some(Control::Favourite),
            )
        })
    }

    /// The title, then the artist and the album as links.
    fn names(&self) -> (String, Vec<(Span<'a>, Option<Control>)>) {
        let theme = self.theme;
        let Some(entry) = self.entry else {
            let title = self
                .track_info
                .and_then(|i| i.path.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            return (title, Vec::new());
        };
        let link = |on: bool| on.then_some(());
        let mut by = Vec::new();
        if !entry.artist.is_empty() {
            by.push((
                Span::styled(entry.artist.as_str(), theme.track_normal),
                link(self.links.1).map(|_| Control::Artist),
            ));
        }
        if !entry.album.is_empty() {
            if !by.is_empty() {
                by.push((Span::styled(" \u{2014} ", theme.hint_desc), None));
            }
            by.push((
                Span::styled(entry.album.as_str(), theme.track_normal),
                link(self.links.0).map(|_| Control::Album),
            ));
        }
        (entry.title.clone(), by)
    }

    /// The right-hand parts: the format badge with its EQ, then the sleep
    /// timer and the output.
    fn trailing(&self) -> (Parts<'static>, Parts<'static>) {
        let theme = self.theme;
        let mut format = Vec::new();
        if let Some(info) = self.track_info {
            format.push((
                Span::styled(badge(info, self.output_rate), theme.hint_desc),
                None,
            ));
        }
        if let Some(d) = &self.dsp {
            format.push((Span::styled(" \u{b7} ", theme.hint_desc), None));
            format.push((
                Span::styled(dsp_label(d), Style::new().fg(theme.tokens.accent)),
                Some(Control::Eq),
            ));
        }
        let set = self.sleep.is_some() || self.sleep_fading;
        let mut route = vec![(
            Span::styled(
                sleep_label(self.sleep, self.sleep_fading),
                if set {
                    Style::new().fg(theme.tokens.accent)
                } else {
                    theme.hint_desc
                },
            ),
            Some(Control::Sleep),
        )];
        if let Some(output) = &self.output {
            route.push((Span::raw("  "), None));
            route.push((
                Span::styled(format!("\u{25D4} {output}"), theme.hint_desc),
                Some(Control::Output),
            ));
        }
        (format, route)
    }

    /// Draws the bar and says where its parts are.
    pub fn draw(self, area: Rect, buf: &mut Buffer) -> Hits {
        let mut hits = Hits::default();
        if area.width == 0 || area.height == 0 {
            return hits;
        }
        let theme = self.theme;
        let Some(_) = self.track_info else {
            buf.set_stringn(
                area.x + 1,
                area.y,
                "nothing playing",
                area.width.saturating_sub(1) as usize,
                theme.status_stopped,
            );
            return hits;
        };
        let playing = self.playback_state == PlaybackState::Playing;
        let duration = self.duration_ms();
        let (title, by) = self.names();
        let mut lead: Vec<(Span<'_>, Option<Control>)> = vec![(
            Span::styled(title, theme.heading.add_modifier(Modifier::BOLD)),
            None,
        )];
        if let Some(heart) = self.heart() {
            lead.push((Span::raw(" "), None));
            lead.push(heart);
        }
        let (format, route) = self.trailing();
        let rows = area.height.min(3);
        let y = |row: u16| area.y + row.min(rows - 1);

        if area.width >= ZONES_MIN_WIDTH {
            // Three zones: the middle given its width, the sides what is left
            // in equal shares, so the controls stay centred.
            let gap = 2;
            let centre_w = (area.width * 2 / 5).clamp(30, 64);
            let side_w = (area.width - centre_w - 2 * gap) / 2;
            let left = area.x + 1;
            let centre = area.x + (area.width - centre_w) / 2;
            let right_end = area.x + area.width - 1;
            let side = side_w.saturating_sub(1);

            // The heart stays beside the title, which gives way to it.
            let heart_w = width_of(&lead[1..]);
            place(
                buf,
                &mut hits.controls,
                left,
                y(0),
                side.saturating_sub(heart_w),
                &lead[..1],
            );
            let title_w = lead[0].0.width().min(side.saturating_sub(heart_w) as usize) as u16;
            place(
                buf,
                &mut hits.controls,
                left + title_w,
                y(0),
                heart_w,
                &lead[1..],
            );
            if rows >= 2 {
                place(buf, &mut hits.controls, left, y(1), side, &by);
            }

            let ctl = controls(theme, playing, self.shuffle, self.repeat, "   ");
            let ctl_w = width_of(&ctl);
            place(
                buf,
                &mut hits.controls,
                centre + centre_w.saturating_sub(ctl_w) / 2,
                y(0),
                centre_w,
                &ctl,
            );
            if rows >= 2 {
                hits.seek = seek_bar(
                    Rect::new(centre, y(1), centre_w, 1),
                    buf,
                    theme,
                    self.position_ms,
                    duration,
                    self.seekable_ms,
                );
            }

            // Right-aligned; what does not fit is dropped rather than cut.
            for (row, parts) in [(0, &format), (1, &route)] {
                let w = width_of(parts);
                if w <= side && rows > row {
                    place(buf, &mut hits.controls, right_end - w, y(row), w, parts);
                }
            }
            if rows >= 3
                && let Some(levels) = self.levels
            {
                mini_spectrum(buf, theme, right_end - MINI_BARS as u16, y(2), &levels);
            }
        } else {
            // Stacked: the names and the controls, the seek bar, then the
            // badge and the output.
            let inner = area.width.saturating_sub(2);
            let left = area.x + 1;
            let ctl = controls(theme, playing, self.shuffle, self.repeat, "  ");
            let ctl_w = width_of(&ctl);
            let room = inner.saturating_sub(ctl_w + 2);
            place(buf, &mut hits.controls, left, y(0), room, &lead);
            if inner >= ctl_w {
                place(
                    buf,
                    &mut hits.controls,
                    left + inner - ctl_w,
                    y(0),
                    ctl_w,
                    &ctl,
                );
            }
            if rows >= 2 {
                hits.seek = seek_bar(
                    Rect::new(left, y(1), inner, 1),
                    buf,
                    theme,
                    self.position_ms,
                    duration,
                    self.seekable_ms,
                );
            }
            if rows >= 3 {
                let fmt_w = width_of(&format);
                let by_room = if fmt_w + 2 <= inner / 2 {
                    place(
                        buf,
                        &mut hits.controls,
                        left + inner - fmt_w,
                        y(2),
                        fmt_w,
                        &format,
                    );
                    inner - fmt_w - 2
                } else {
                    inner
                };
                place(buf, &mut hits.controls, left, y(2), by_room, &by);
            }
        }
        hits
    }
}

impl Widget for TransportBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.draw(area, buf);
    }
}

fn mini_spectrum(buf: &mut Buffer, theme: &Theme, x: u16, y: u16, levels: &[f32; MINI_BARS]) {
    const STEPS: [char; 8] = [
        '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];
    for (i, level) in levels.iter().enumerate() {
        let step = ((level.clamp(0.0, 1.0) * 7.0).round() as usize).min(7);
        if let Some(cell) = buf.cell_mut((x + i as u16, y)) {
            cell.set_char(STEPS[step]).set_style(if step == 0 {
                theme.progress_empty
            } else {
                theme.progress_filled
            });
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

    #[test]
    fn a_bar_with_no_room_draws_nothing() {
        let b = BarRegions::new(0, 30_000, 100_000, None);
        assert_eq!((b.played, b.reachable, b.pending), (0, 0, 0));
    }

    fn info() -> TrackInfo {
        TrackInfo {
            id: koan_core::player::state::QueueItemId::new(),
            path: std::path::PathBuf::from("/x.flac"),
            codec: "FLAC".into(),
            sample_rate: 44100,
            bit_depth: Some(16),
            bitrate_kbps: None,
            channels: 2,
            duration_ms: NINE_HOURS,
        }
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    #[test]
    fn the_head_is_drawn_where_a_played_run_would_show_nothing() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        let bar = seek_bar(area, &mut buf, &theme, 20_000, NINE_HOURS, None);
        let line = row(&buf, 0);
        assert_eq!(buf[(bar.x, 0)].symbol(), HEAD, "{line}");
    }

    #[test]
    fn the_seek_bar_holds_still_as_the_time_counts() {
        let theme = Theme::default();
        let area = Rect::new(0, 0, 50, 1);
        let bars: Vec<Rect> = [0, 9_000, 59_000, 600_000, 3_599_000]
            .into_iter()
            .map(|pos| seek_bar(area, &mut Buffer::empty(area), &theme, pos, 3_600_000, None))
            .collect();
        assert!(bars.windows(2).all(|w| w[0] == w[1]), "{bars:?}");
    }

    #[test]
    fn a_click_puts_the_head_on_the_cell_clicked() {
        let bar = Rect::new(4, 0, 37, 1);
        for duration in [1_000u64, 213_000, NINE_HOURS] {
            for cell in 0..bar.width {
                let pos = seek_from_click(bar, bar.x + cell, duration, None).unwrap();
                let regions = BarRegions::new(bar.width as usize, pos, duration, None);
                assert_eq!(regions.played, cell as usize, "{duration} ms, cell {cell}");
            }
        }
    }

    #[test]
    fn every_drawn_control_is_where_its_glyph_is() {
        let theme = Theme::default();
        let info = info();
        for w in [40u16, 71, 72, 100, 160] {
            let area = Rect::new(0, 0, w, 3);
            let mut buf = Buffer::empty(area);
            let mut bar = TransportBar::new(Some(&info), None, PlaybackState::Playing, 0, &theme);
            bar.favourite = Some(true);
            let hits = bar.draw(area, &mut buf);
            for (r, c) in &hits.controls {
                let text: String = (r.x..r.x + r.width)
                    .map(|x| buf[(x, r.y)].symbol())
                    .collect();
                assert!(
                    !text.trim().is_empty(),
                    "{c:?} at {r:?} is blank, width {w}"
                );
            }
            assert!(
                hits.controls.iter().any(|(_, c)| *c == Control::PlayPause),
                "width {w}"
            );
            assert!(hits.seek.width > 0, "width {w}");
        }
    }

    #[test]
    fn the_controls_are_centred_on_a_wide_bar() {
        let theme = Theme::default();
        let info = info();
        let area = Rect::new(0, 0, 120, 3);
        let mut buf = Buffer::empty(area);
        let hits = TransportBar::new(Some(&info), None, PlaybackState::Paused, 0, &theme)
            .draw(area, &mut buf);
        let xs: Vec<u16> = hits
            .controls
            .iter()
            .filter(|(r, _)| r.y == 0)
            .map(|(r, _)| r.x)
            .collect();
        let (first, last) = (xs[0], *xs.last().unwrap());
        let mid = (first + last).div_ceil(2);
        assert!(mid.abs_diff(60) <= 1, "controls span {first}..{last}");
    }

    #[test]
    fn the_badge_reads_as_the_apps_do() {
        let mut i = info();
        assert_eq!(badge(&i, None), "FLAC 16/44.1");
        assert_eq!(badge(&i, Some(48_000)), "FLAC 16/44.1 → 48");
        i.bit_depth = None;
        i.codec = "opus".into();
        i.sample_rate = 48_000;
        assert_eq!(badge(&i, Some(48_000)), "OPUS 48 kHz");
    }

    #[test]
    fn fit_cuts_by_cells_and_marks_the_cut() {
        assert_eq!(fit("abc", 3), "abc");
        assert_eq!(fit("abcdef", 4), "abc\u{2026}");
        assert!(fit("夜に駆ける夜に駆ける", 7).width() <= 7);
        assert_eq!(fit("abc", 0), "");
    }
}
