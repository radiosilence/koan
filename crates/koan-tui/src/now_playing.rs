//! The Now Playing page: the cover, what is playing, the path the sound takes
//! to the output, the spectrum, and the lyrics. The signal path is the part a
//! terminal can afford to show in full: the source, what the device was given,
//! and what, if anything, stood between them.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use koan_core::audio::dsp::DspStatus;
use koan_core::config::ReplayGainMode;
use koan_core::player::state::{QueueEntry, TrackInfo};

use super::app::App;
use super::lyrics::LyricsPanel;
use super::theme::Theme;
use super::transport::format_quality;
use super::visualizer::{BrailleGrid, VisualizerWidget};

/// Wider than this and the cover has a column of its own.
const COVER_MIN_WIDTH: u16 = 48;

/// What stands between the file and the device. `None` is bit-perfect.
pub fn deviation(
    info: &TrackInfo,
    output_rate: Option<u32>,
    dsp: Option<&DspStatus>,
    replaygain: ReplayGainMode,
) -> Option<String> {
    if dsp.is_some() {
        return Some("processed by dsp".into());
    }
    if let Some(out) = output_rate
        && out != info.sample_rate
    {
        return Some(format!("resampled to {out} Hz"));
    }
    if replaygain != ReplayGainMode::Off {
        return Some("replaygain on".into());
    }
    None
}

/// A level as a bar of `width` cells, to the eighth.
fn meter(level: f32, width: usize) -> String {
    const PARTIAL: [char; 8] = [
        ' ', '\u{258f}', '\u{258e}', '\u{258d}', '\u{258c}', '\u{258b}', '\u{258a}', '\u{2589}',
    ];
    let eighths = (level.clamp(0.0, 1.0) * (width * 8) as f32).round() as usize;
    let (full, part) = (eighths / 8, eighths % 8);
    let mut bar = "\u{2588}".repeat(full);
    if full < width {
        bar.push(PARTIAL[part]);
        bar.push_str(&" ".repeat(width - full - 1));
    }
    bar
}

fn db_label(level: f32) -> String {
    if level < 0.001 {
        "  -\u{221e}".into()
    } else {
        format!("{:>4.0}", 20.0 * level.log10())
    }
}

/// A response curve in braille, `db` against log-spaced frequency, with a
/// zero line, dB labels down the side and a frequency axis under it. The
/// vertical range is the curve's own, at least ±6 dB. The first row says what
/// the curve is.
fn eq_plot(area: Rect, buf: &mut Buffer, theme: &Theme, profile: &str, db: &[f64]) {
    if area.width < 14 || area.height < 5 || db.is_empty() {
        return;
    }
    let heading = format!("eq \u{b7} {profile}");
    buf.set_stringn(
        area.x,
        area.y,
        &heading,
        area.width as usize,
        theme.hint_desc,
    );
    let label_w = 5u16;
    let plot = Rect::new(
        area.x + label_w,
        area.y + 1,
        area.width - label_w,
        area.height - 2,
    );
    let range = db.iter().fold(6.0f64, |m, v| m.max(v.abs())).ceil();
    let mut grid = BrailleGrid::new(plot.width as usize, plot.height as usize);
    let (pw, ph) = (grid.px_width(), grid.px_height());
    let y_of = |v: f64| {
        ((1.0 - (v + range) / (2.0 * range)) * (ph - 1) as f64).clamp(0.0, (ph - 1) as f64)
    };
    let zero = y_of(0.0) as usize;
    for x in (0..pw).step_by(2) {
        grid.set_dot(x, zero, theme.tokens.rule);
    }
    let mut prev: Option<(f32, f32)> = None;
    for x in 0..pw {
        let i = x * (db.len() - 1) / (pw - 1).max(1);
        let point = (x as f32, y_of(db[i]) as f32);
        if let Some((px, py)) = prev {
            grid.draw_line(px, py, point.0, point.1, theme.tokens.accent);
        }
        prev = Some(point);
    }
    grid.render_to(plot, buf);
    let label = |v: f64| format!("{:>+4.0} ", v);
    let style = theme.hint_desc;
    buf.set_string(area.x, plot.y, label(range), style);
    buf.set_string(area.x, plot.y + plot.height / 2, label(0.0), style);
    buf.set_string(area.x, plot.y + plot.height - 1, label(-range), style);
    // The frequency axis: the curve runs 20 Hz to 20 kHz on a log scale.
    let axis_y = area.y + area.height - 1;
    for (hz, text) in [
        (50.0f64, "50"),
        (200.0, "200"),
        (1000.0, "1k"),
        (5000.0, "5k"),
        (15000.0, "15k"),
    ] {
        let t = (hz / 20.0).log10() / (20000.0f64 / 20.0).log10();
        let x = plot.x + (t * plot.width.saturating_sub(1) as f64).round() as u16;
        if x + text.len() as u16 <= plot.x + plot.width {
            buf.set_string(x, axis_y, text, style);
        }
    }
}

fn row(buf: &mut Buffer, area: Rect, y: u16, key: &str, value: Vec<Span<'_>>, theme: &Theme) {
    if y >= area.y + area.height {
        return;
    }
    let mut spans = vec![Span::styled(format!(" {key:<8}"), theme.hint_desc)];
    spans.extend(value);
    buf.set_line(area.x, y, &Line::from(spans), area.width);
}

pub fn render(buf: &mut Buffer, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    if body.height == 0 {
        return;
    }
    let theme = &app.theme;

    let info = app.state.track_info();
    let entry: Option<QueueEntry> = app
        .queue
        .vq_cache
        .entries
        .iter()
        .find(|e| e.status == koan_core::player::state::QueueEntryStatus::Playing)
        .cloned();

    let Some(info) = info else {
        buf.set_line(
            body.x,
            body.y,
            &Line::from(Span::styled(" nothing playing", theme.hint_desc)),
            body.width,
        );
        return;
    };

    // The cover's column. Cells are about twice as tall as wide.
    let mut art_w = match body.width {
        100.. => 36u16,
        70..=99 => 28,
        COVER_MIN_WIDTH..=69 => 16,
        _ => 0,
    };
    // The cover takes under half the page's height, so the spectrum and the
    // lyrics keep room under it.
    let max_art_h = (body.height * 9 / 20).clamp(1, body.height.saturating_sub(4).max(1));
    if art_w / 2 > max_art_h {
        art_w = max_art_h * 2;
    }
    let art_h = (art_w / 2).max(u16::from(art_w > 0));
    let top_h = art_h.max(8).min(body.height);
    let text_x = body.x + if art_w > 0 { art_w + 2 } else { 0 } + 1;
    let text_w = (body.x + body.width).saturating_sub(text_x);

    if art_w > 0 {
        let art_area = Rect::new(body.x + 1, body.y, art_w, art_h);
        app.layout.now_playing_art_area = art_area;
        let cache = &app.art.now_playing_art;
        cache.render_to(&mut app.art.gfx, art_area, buf);
    }

    let text = Rect::new(text_x, body.y, text_w, top_h);
    let theme = &app.theme;
    if text.width > 4 {
        let (title, artist, album) = match &entry {
            Some(e) => (
                e.title.clone(),
                e.artist.clone(),
                [
                    Some(e.album.clone()),
                    e.year.as_ref().map(|y| format!("({y})")),
                ]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
            ),
            None => (
                info.path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                String::new(),
                String::new(),
            ),
        };
        let w = text.width as usize;
        buf.set_stringn(
            text.x,
            text.y,
            &title,
            w,
            theme.heading.add_modifier(ratatui::style::Modifier::BOLD),
        );
        buf.set_stringn(text.x, text.y + 1, &artist, w, theme.track_playing);
        buf.set_stringn(text.x, text.y + 2, &album, w, theme.hint_desc);

        let output_rate = app.state.output_sample_rate();
        let dsp = app.state.dsp();
        let replaygain = koan_core::config::Config::cached().playback.replaygain;
        let off = deviation(&info, output_rate, dsp.as_ref(), replaygain);
        let value = |s: String| vec![Span::styled(s, theme.track_normal)];
        let sy = text.y + 4;
        let pad = |k: &str, y, v: Vec<Span<'_>>, buf: &mut Buffer| {
            row(buf, text, y, k, v, theme);
        };
        // The text column starts one cell in already; `row` adds its own pad.
        let source = format_quality(&info, None, None);
        pad("source", sy, value(source), buf);
        let output = match (&off, output_rate) {
            (None, Some(rate)) => vec![
                Span::styled(format!("{rate} Hz \u{b7} "), theme.track_normal),
                Span::styled("bit-perfect \u{2713}", theme.track_playing),
            ],
            (None, None) => vec![Span::styled("bit-perfect \u{2713}", theme.track_playing)],
            (Some(why), rate) => vec![
                Span::styled(
                    rate.map_or(String::new(), |r| format!("{r} Hz \u{b7} ")),
                    theme.track_normal,
                ),
                Span::styled(why.clone(), Style::new().fg(theme.warning)),
            ],
        };
        pad("output", sy + 1, output, buf);
        let dsp_text = match &dsp {
            Some(d) => format!(
                "{} ({})",
                d.profile,
                match (d.eq, d.convolution_rate.is_some()) {
                    (true, true) => "EQ + FIR",
                    (false, true) => "FIR",
                    _ => "EQ",
                }
            ),
            None => "none".into(),
        };
        pad("dsp", sy + 2, value(dsp_text), buf);

        let [l, r] = app.visualizer.vu_levels;
        let bar_w = (text.width as usize).saturating_sub(18).clamp(4, 30);
        for (i, (name, level)) in [("L", l), ("R", r)].into_iter().enumerate() {
            pad(
                if i == 0 { "level" } else { "" },
                sy + 4 + i as u16,
                vec![
                    Span::styled(format!("{name} "), theme.hint_desc),
                    Span::styled(meter(level, bar_w), theme.progress_filled),
                    Span::styled(format!(" {} dB", db_label(level)), theme.hint_desc),
                ],
                buf,
            );
        }
    }

    // Below the top block: the spectrum, then lyrics beside the EQ curve.
    let rest_y = body.y + top_h + 1;
    let rest_h = (body.y + body.height).saturating_sub(rest_y);
    if rest_h < 3 {
        return;
    }
    let spectrum_h = (rest_h * 2 / 5).clamp(3, 12).min(rest_h);
    let spectrum = Rect::new(body.x + 1, rest_y, body.width.saturating_sub(2), spectrum_h);
    if app.viz_config.enabled {
        VisualizerWidget::new(&mut app.visualizer, &app.theme).render(spectrum, buf);
    }
    let low_y = rest_y + spectrum_h + 1;
    let low_h = (body.y + body.height).saturating_sub(low_y);
    if low_h < 3 {
        return;
    }
    let low = Rect::new(body.x, low_y, body.width, low_h);

    let curve = app.state.dsp().and_then(|d| {
        let rate = app.state.output_sample_rate().unwrap_or(info.sample_rate);
        koan_core::audio::dsp::profiles::response(&d.profile, rate)
    });
    let (lyrics_area, plot_area) = match &curve {
        Some(_) if low.width >= 70 => {
            let plot_w = (low.width * 2 / 5).min(48);
            (
                Rect::new(low.x, low.y, low.width - plot_w, low.height),
                Some(Rect::new(
                    low.x + low.width - plot_w + 2,
                    low.y,
                    plot_w - 2,
                    low.height.min(12),
                )),
            )
        }
        _ => (low, None),
    };
    let pos = app.state.position_ms();
    LyricsPanel::new(&app.lyrics, pos, &app.theme, app.spinner_tick).render(lyrics_area, buf);
    if let (Some(plot), Some(curve)) = (plot_area, &curve) {
        let profile = app.state.dsp().map(|d| d.profile).unwrap_or_default();
        eq_plot(plot, buf, &app.theme, &profile, &curve.total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use koan_core::player::state::QueueItemId;

    fn info(rate: u32) -> TrackInfo {
        TrackInfo {
            id: QueueItemId::new(),
            path: "/x.flac".into(),
            codec: "FLAC".into(),
            sample_rate: rate,
            bit_depth: Some(16),
            bitrate_kbps: None,
            channels: 2,
            duration_ms: 1000,
        }
    }

    #[test]
    fn an_untouched_stream_is_bit_perfect() {
        let off = ReplayGainMode::Off;
        assert_eq!(deviation(&info(44100), Some(44100), None, off), None);
        assert_eq!(deviation(&info(44100), None, None, off), None);
    }

    #[test]
    fn anything_between_the_file_and_the_device_is_named() {
        let off = ReplayGainMode::Off;
        assert!(
            deviation(&info(44100), Some(48000), None, off)
                .unwrap()
                .contains("48000")
        );
        assert!(deviation(&info(44100), Some(44100), None, ReplayGainMode::Track).is_some());
        let dsp = DspStatus {
            profile: "hd650".into(),
            eq: true,
            convolution_rate: None,
        };
        assert!(
            deviation(&info(44100), Some(44100), Some(&dsp), off)
                .unwrap()
                .contains("dsp")
        );
    }

    #[test]
    fn the_meter_fills_to_the_level() {
        assert_eq!(meter(0.0, 4).trim_end(), "");
        assert_eq!(meter(1.0, 4), "\u{2588}".repeat(4));
        assert_eq!(meter(0.5, 4).chars().count(), 4);
        assert_eq!(meter(2.0, 4), "\u{2588}".repeat(4));
    }
}
