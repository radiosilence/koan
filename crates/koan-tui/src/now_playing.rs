//! The Now Playing page, as the apps' Now Playing sheet lays it out: the
//! cover, the track with its artist and album as links and its heart, the
//! seek bar and the controls, then what the output is handed and where it
//! goes. Beside and below it, what a terminal can show besides: the signal
//! path in full, loudness as EBU R128 measures it, the spectrum, a
//! spectrogram, the lyrics and the EQ's curve. Everything moving on the page
//! is driven by the music.

use std::sync::Arc;

use crossbeam_channel::Receiver;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use koan_core::audio::dsp::DspStatus;
use koan_core::config::{Config, ReplayGainMode};
use koan_core::player::state::{PlaybackState, QueueEntry, QueueItemId, TrackInfo};

use super::app::App;
use super::lyrics::LyricsPanel;
use super::shell::Link;
use super::theme::Theme;
use super::transport::{
    self, Act, Control, MINI_BARS, TransportBar, badge, dsp_label, format_quality, place, press,
    width_of,
};
use super::visualizer::{BrailleGrid, VisualizerWidget, render_spectrogram};

/// Narrower than this and the page stacks the cover over the words.
const SIDE_BY_SIDE: u16 = 90;

/// The widest the words beside the cover run.
const INFO_MAX: u16 = 76;

/// What the page keeps between frames.
#[derive(Default)]
pub struct State {
    pub eq_curve: EqCurve,
    /// The track the true-peak hold belongs to.
    peak_track: Option<QueueItemId>,
}

/// A response curve and the profile it is the curve of.
type Curve = (String, Vec<f64>);

/// The curve the page plots, worked out on a thread of its own whenever the
/// profile, the rate or the profiles it is built from change: building a
/// profile can read and resample impulse files, which no frame can wait for.
/// The last curve stays up until its replacement arrives.
#[derive(Default)]
pub struct EqCurve {
    made_for: Option<(String, u32, Arc<Config>)>,
    /// The curve shown, with the profile it is the curve of.
    total: Option<Curve>,
    pending: Option<Receiver<Option<Curve>>>,
}

impl EqCurve {
    /// The curve to plot and the profile it belongs to, which is the last
    /// one's until a new profile's curve arrives.
    fn get(&mut self, profile: &str, rate: u32, cfg: &Arc<Config>) -> Option<(&str, &[f64])> {
        // Every config write is a new `Arc`; only the profiles shape the curve.
        let current = self.made_for.as_mut().is_some_and(|(p, r, c)| {
            let same = p == profile
                && *r == rate
                && (Arc::ptr_eq(c, cfg) || c.dsp.profiles == cfg.dsp.profiles);
            if same {
                *c = cfg.clone();
            }
            same
        });
        if !current {
            self.made_for = Some((profile.to_owned(), rate, cfg.clone()));
            let (tx, rx) = crossbeam_channel::bounded(1);
            let profile = profile.to_owned();
            std::thread::Builder::new()
                .name("koan-eq-curve".into())
                .spawn(move || {
                    let curve = koan_core::audio::dsp::profiles::response(&profile, rate);
                    tx.send(curve.map(|c| (profile, c.total))).ok();
                })
                .ok();
            self.pending = Some(rx);
        }
        if let Some(rx) = &self.pending
            && let Ok(total) = rx.try_recv()
        {
            self.total = total;
            self.pending = None;
        }
        self.total.as_ref().map(|(p, c)| (p.as_str(), c.as_slice()))
    }
}

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

/// The transport bar, as everything that draws it fills it in.
pub fn transport_for<'a>(
    app: &'a App,
    info: Option<&'a TrackInfo>,
    entry: Option<&'a QueueEntry>,
    levels: Option<[f32; MINI_BARS]>,
) -> TransportBar<'a> {
    let mode = app.state.play_mode();
    let mut bar = TransportBar::new(
        info,
        entry,
        app.state.playback_state(),
        app.state.position_ms(),
        &app.theme,
    )
    .with_seekable_ms(app.state.seek_ceiling_ms());
    bar.output_rate = app.state.output_sample_rate();
    bar.dsp = app.state.dsp();
    bar.sleep = app.state.sleep();
    bar.sleep_fading = app.state.sleep_fading();
    bar.shuffle = mode.shuffle;
    bar.repeat = mode.repeat;
    bar.favourite = entry
        .and_then(|e| e.db_id)
        .map(|id| app.favourites.contains(&id));
    bar.output = Some(output_name(app));
    bar.controlled = app.view.controlled.as_ref().map(|(n, r)| (n.as_str(), r));
    bar.links = Some(&app.links);
    bar.levels = levels;
    bar
}

/// What the output is: the renderer played to, or the device chosen.
fn output_name(app: &App) -> String {
    match app.state.renderer() {
        Some(r) => r.name,
        None => app
            .cfg
            .playback
            .output_device
            .clone()
            .unwrap_or_else(|| "system output".into()),
    }
}

/// A key and its value, the key in a column of its own.
fn field(buf: &mut Buffer, x: u16, y: u16, w: u16, key: &str, value: Vec<Span<'_>>, theme: &Theme) {
    let mut spans = vec![Span::styled(format!("{key:<10}"), theme.hint_desc)];
    spans.extend(value);
    buf.set_line(x, y, &Line::from(spans), w);
}

/// A loudness in LUFS as a bar over the meter's range, -60 to 0, and its
/// figure.
fn lufs_bar(lufs: f64, width: usize) -> (String, String) {
    let level = if lufs.is_finite() {
        ((lufs + 60.0) / 60.0) as f32
    } else {
        0.0
    };
    let figure = if lufs.is_finite() && lufs > -70.0 {
        format!("{lufs:>6.1}")
    } else {
        "     -".into()
    };
    (meter(level, width), figure)
}

/// A linear peak in dBTP, to a tenth.
fn dbtp(peak: f64) -> String {
    if peak <= 0.0 {
        "-".into()
    } else {
        format!("{:+.1}", 20.0 * peak.log10())
    }
}

pub fn render(buf: &mut Buffer, app: &mut App, area: Rect) {
    let page = area;
    if page.width < 12 || page.height < 3 {
        return;
    }
    let Some(info) = app.state.track_info() else {
        buf.set_stringn(
            page.x,
            page.y,
            "nothing playing",
            page.width as usize,
            app.theme.hint_desc,
        );
        return;
    };
    let entry = app
        .queue
        .vq_cache
        .entries
        .iter()
        .find(|e| e.id == info.id)
        .cloned();
    if app.now_playing.peak_track != Some(info.id) {
        app.now_playing.peak_track = Some(info.id);
        app.visualizer.true_peak_hold = 0.0;
    }
    let cfg = app.cfg.clone();
    let output_rate = app.state.output_sample_rate();
    let dsp = app.state.dsp();

    // The cover, square in the terminal's cells: beside the words where the
    // page is wide, over them where it is not.
    let wide = page.width >= SIDE_BY_SIDE;
    let squash = app.art.gfx.squash();
    let (max_w, max_h) = if wide {
        (page.width * 2 / 5, page.height * 11 / 20)
    } else {
        (page.width, page.height * 2 / 5)
    };
    let tallest = (f32::from(max_h) * 2.0 / squash) as u16;
    let cover_w = max_w.min(tallest).min(56);
    let cover_h = app.art.gfx.rows_for(cover_w).min(max_h);
    let cover = if wide {
        Rect::new(page.x, page.y, cover_w, cover_h)
    } else {
        Rect::new(
            page.x + (page.width - cover_w) / 2,
            page.y,
            cover_w,
            cover_h,
        )
    };
    if cover.width > 0 && cover.height > 0 {
        app.layout.now_playing_art_area = cover;
        app.art
            .now_playing_art
            .render_to_centered(&mut app.art.gfx, cover, buf);
        if app.art.now_playing_art.cached().is_some() {
            app.layout
                .now_playing
                .controls
                .push((cover, Control::Cover));
        }
    }
    let words = if wide {
        let x = cover.x + cover.width + 3;
        Rect::new(
            x,
            page.y,
            (page.x + page.width).saturating_sub(x).min(INFO_MAX),
            page.height,
        )
    } else {
        let y = cover.y + cover.height + 1;
        Rect::new(
            page.x,
            y,
            page.width.min(INFO_MAX),
            (page.y + page.height).saturating_sub(y),
        )
    };
    let used = draw_words(buf, app, words, &info, entry.as_ref());
    let top = if wide {
        cover.height.max(used)
    } else {
        cover.height + 1 + used
    };

    // Below: the spectrum and the spectrogram beside the lyrics and the EQ.
    let rest_y = page.y + top + 1;
    let rest_h = (page.y + page.height).saturating_sub(rest_y);
    if rest_h < 4 {
        return;
    }
    let rest = Rect::new(page.x, rest_y, page.width, rest_h);
    let (scopes, words_side) = if rest.width >= 70 {
        let right = rest.width * 2 / 5;
        (
            Rect::new(rest.x, rest.y, rest.width - right - 1, rest.height),
            Rect::new(rest.x + rest.width - right, rest.y, right, rest.height),
        )
    } else {
        let half = rest.height / 2;
        (
            Rect::new(rest.x, rest.y, rest.width, half),
            Rect::new(rest.x, rest.y + half, rest.width, rest.height - half),
        )
    };
    if app.viz_config.enabled && scopes.height >= 2 {
        let spectrum_h = (scopes.height / 2).clamp(1, 10);
        let label = |buf: &mut Buffer, y: u16, text: &str| {
            buf.set_stringn(
                scopes.x,
                y,
                text,
                scopes.width as usize,
                app.theme.hint_desc,
            );
        };
        label(buf, scopes.y, "spectrum");
        let spectrum = Rect::new(
            scopes.x,
            scopes.y + 1,
            scopes.width,
            spectrum_h.saturating_sub(1).max(1),
        );
        VisualizerWidget::new(&mut app.visualizer, &app.theme).render(spectrum, buf);
        let gram_y = spectrum.y + spectrum.height + 1;
        let gram_h = (scopes.y + scopes.height).saturating_sub(gram_y + 1);
        if gram_h >= 2 {
            label(buf, gram_y, "spectrogram");
            render_spectrogram(
                &app.visualizer,
                Rect::new(scopes.x, gram_y + 1, scopes.width, gram_h),
                buf,
            );
        }
    }

    let curve = dsp.as_ref().and_then(|d| {
        let rate = output_rate.unwrap_or(info.sample_rate);
        app.now_playing.eq_curve.get(&d.profile, rate, &cfg)
    });
    let plot_h = match curve {
        Some(_) if words_side.height >= 14 => 9,
        _ => 0,
    };
    let lyrics_area = Rect::new(
        words_side.x,
        words_side.y,
        words_side.width,
        words_side.height - plot_h,
    );
    let pos = app.state.position_ms();
    LyricsPanel::new(&app.lyrics, pos, &app.theme, app.spinner_tick).render(lyrics_area, buf);
    if let (Some((profile, curve)), true) = (curve, plot_h > 0) {
        let plot = Rect::new(
            words_side.x + 1,
            lyrics_area.y + lyrics_area.height,
            words_side.width.saturating_sub(1),
            plot_h,
        );
        eq_plot(plot, buf, &app.theme, profile, curve);
        app.layout.eq_hits.push(plot);
    }
}

/// The cover as large as the screen allows, with the track and who it is by
/// centred under it, as the apps' artwork viewer has it. A click anywhere or
/// Escape closes it.
pub fn render_zoom(buf: &mut Buffer, app: &mut App, area: Rect) {
    ratatui::widgets::Clear.render(area, buf);
    // A margin all round, and two rows of caption under a gap.
    const CAPTION: u16 = 3;
    let (room_w, room_h) = (
        area.width.saturating_sub(4),
        area.height.saturating_sub(2 + CAPTION),
    );
    if room_w == 0 || room_h == 0 {
        return;
    }
    let tallest = (f32::from(room_h) * 2.0 / app.art.gfx.squash()) as u16;
    let side = room_w.min(tallest).max(1);
    let rows = app.art.gfx.rows_for(side).min(room_h);
    let block = rows + CAPTION;
    let cover = Rect::new(
        area.x + (area.width - side) / 2,
        area.y + (area.height - block) / 2,
        side,
        rows,
    );
    app.art
        .now_playing_art
        .render_to_centered(&mut app.art.gfx, cover, buf);
    let entry = app.state.track_info().and_then(|info| {
        app.queue
            .vq_cache
            .entries
            .iter()
            .find(|e| e.id == info.id)
            .cloned()
    });
    let Some(e) = entry else {
        return;
    };
    let subtitle = [e.artist.as_str(), e.album.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{2014} ");
    let theme = &app.theme;
    for (y, text, style) in [
        (
            cover.y + rows + 1,
            e.title.as_str(),
            theme.heading.add_modifier(Modifier::BOLD),
        ),
        (cover.y + rows + 2, subtitle.as_str(), theme.hint_desc),
    ] {
        let text = super::text::fit(text, area.width.saturating_sub(4) as usize);
        let w = crate::text::width(&text) as u16;
        if y < area.y + area.height {
            buf.set_string(area.x + (area.width - w) / 2, y, &text, style);
        }
    }
}

/// The words beside or under the cover. Returns how many rows they took.
fn draw_words(
    buf: &mut Buffer,
    app: &mut App,
    area: Rect,
    info: &TrackInfo,
    entry: Option<&QueueEntry>,
) -> u16 {
    if area.width < 8 || area.height == 0 {
        return 0;
    }
    let cfg = app.cfg.clone();
    let output_rate = app.state.output_sample_rate();
    let dsp = app.state.dsp();
    let dsp = dsp.as_ref();
    let playing = app.state.playback_state() == PlaybackState::Playing;
    let output = output_name(app);
    let controlled = app.view.controlled.as_ref().map(|(n, r)| (n.as_str(), r));
    let theme = &app.theme;
    let hits = &mut app.layout.now_playing;
    let bottom = area.y + area.height;
    let mut y = area.y;
    let mut next = |gap: u16| {
        let at = y;
        y += 1 + gap;
        (at < bottom).then_some(at)
    };
    let links = &app.links;
    let track = entry.and_then(|e| e.db_id);
    let w = area.width;

    // The track: its title and heart, then the artist and the album.
    let title = entry.map_or_else(
        || {
            info.path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        },
        |e| e.title.clone(),
    );
    if let Some(row) = next(0) {
        let favourite = entry
            .and_then(|e| e.db_id)
            .map(|id| app.favourites.contains(&id));
        let heart = favourite.map(|on| {
            if on {
                (
                    Span::styled("\u{2665}", Style::new().fg(theme.tokens.accent)),
                    press(Control::Favourite),
                )
            } else {
                (
                    Span::styled("\u{2661}", theme.hint_desc),
                    press(Control::Favourite),
                )
            }
        });
        let room = w.saturating_sub(if heart.is_some() { 3 } else { 0 });
        place(
            buf,
            &mut hits.controls,
            links,
            (area.x, row),
            room,
            &[(
                Span::styled(title, theme.heading.add_modifier(Modifier::BOLD)),
                None,
            )],
        );
        if let Some(heart) = heart {
            place(
                buf,
                &mut hits.controls,
                links,
                (area.x + w - 1, row),
                1,
                &[heart],
            );
        }
    }
    if let Some(e) = entry {
        if let Some(row) = next(0) {
            place(
                buf,
                &mut hits.controls,
                links,
                (area.x, row),
                w,
                &[(
                    Span::styled(e.artist.as_str(), Style::new().fg(theme.tokens.accent)),
                    track.map(|id| Act::Go(Link::ArtistOfTrack(id))),
                )],
            );
        }
        if let Some(row) = next(1) {
            let mut parts = vec![(
                Span::styled(e.album.as_str(), theme.track_normal),
                track.map(|id| Act::Go(Link::AlbumOfTrack(id))),
            )];
            if let Some(year) = e.year.as_deref().filter(|y| !y.is_empty()) {
                parts.push((
                    Span::styled(format!(" \u{b7} {year}"), theme.hint_desc),
                    None,
                ));
            }
            place(buf, &mut hits.controls, links, (area.x, row), w, &parts);
        }
    } else {
        next(1);
    }

    // The seek bar, and the controls centred under it.
    let duration = entry
        .and_then(|e| e.duration_ms)
        .unwrap_or(info.duration_ms);
    if let Some(row) = next(0) {
        hits.seek = transport::seek_bar(
            Rect::new(area.x, row, w, 1),
            buf,
            theme,
            app.state.position_ms(),
            duration,
            app.state.seek_ceiling_ms(),
        );
    }
    if let Some(row) = next(1) {
        let mode = app.state.play_mode();
        let ctl = transport::controls(theme, playing, mode.shuffle, mode.repeat, "    ");
        let ctl_w = width_of(&ctl);
        place(
            buf,
            &mut hits.controls,
            links,
            (area.x + w.saturating_sub(ctl_w) / 2, row),
            w,
            &ctl,
        );
    }

    // The signal path: the file, what the device was given, and the EQ.
    let off = deviation(info, output_rate, dsp, cfg.playback.replaygain);
    let value = |s: String| vec![Span::styled(s, theme.track_normal)];
    if let Some(row) = next(0) {
        let source = format!(
            "{} \u{b7} {}",
            badge(info, None),
            format_quality(info, None, None)
        );
        field(buf, area.x, row, w, "source", value(source), theme);
    }
    if let Some(row) = next(0) {
        let rate = output_rate.map_or(String::new(), |r| {
            format!("{} kHz \u{b7} ", transport::khz(r))
        });
        let verdict = match &off {
            None => Span::styled("bit-perfect \u{2713}", Style::new().fg(theme.tokens.accent)),
            Some(why) => Span::styled(why.clone(), Style::new().fg(theme.warning)),
        };
        field(
            buf,
            area.x,
            row,
            w,
            "output",
            vec![Span::styled(rate, theme.track_normal), verdict],
            theme,
        );
    }
    if let Some(row) = next(0) {
        let name = dsp.map_or_else(
            || "off".to_string(),
            |d| format!("{} ({})", d.profile, dsp_label(d)),
        );
        field(buf, area.x, row, w, "eq", value(name), theme);
        app.layout.eq_hits.push(Rect::new(area.x, row, w, 1));
    }
    if let Some(row) = next(1) {
        let set = app.state.sleep().is_some() || app.state.sleep_fading();
        let parts = [
            (
                Span::styled(
                    transport::sleep_label(app.state.sleep(), app.state.sleep_fading()),
                    if set {
                        Style::new().fg(theme.tokens.accent)
                    } else {
                        theme.hint_desc
                    },
                ),
                press(Control::Sleep),
            ),
            (Span::raw("   "), None),
            transport::play_on(theme, controlled),
            (Span::raw("   "), None),
            (
                Span::styled(format!("\u{25D4} {output}"), theme.hint_desc),
                press(Control::Output),
            ),
        ];
        field(buf, area.x, row, w, "play", Vec::new(), theme);
        place(
            buf,
            &mut hits.controls,
            links,
            (area.x + 10, row),
            w.saturating_sub(10),
            &parts,
        );
    }

    // Loudness, momentary and short-term, the true peak held for the track,
    // and each channel's level.
    let bar_w = (w as usize).saturating_sub(10 + 2 + 6 + 5).clamp(4, 40);
    let loudness = app.visualizer.loudness;
    for (key, lufs) in [
        ("momentary", loudness.momentary),
        ("short", loudness.short_term),
    ] {
        let Some(row) = next(0) else { break };
        let lufs = if playing { lufs } else { f64::NEG_INFINITY };
        let (bar, figure) = lufs_bar(lufs, bar_w);
        field(
            buf,
            area.x,
            row,
            w,
            key,
            vec![
                Span::styled(bar, theme.progress_filled),
                Span::styled(format!(" {figure} LUFS"), theme.hint_desc),
            ],
            theme,
        );
    }
    if let Some(row) = next(0) {
        let peak = app.visualizer.true_peak_hold;
        let style = if peak > 1.0 {
            Style::new().fg(theme.tokens.bad)
        } else {
            theme.track_normal
        };
        field(
            buf,
            area.x,
            row,
            w,
            "true peak",
            vec![
                Span::styled(dbtp(peak), style),
                Span::styled(" dBTP, highest this track", theme.hint_desc),
            ],
            theme,
        );
    }
    let [l, r] = app.visualizer.vu_levels;
    for (key, name, level) in [("level", "L", l), ("", "R", r)] {
        let Some(row) = next(0) else { break };
        field(
            buf,
            area.x,
            row,
            w,
            key,
            vec![
                Span::styled(format!("{name} "), theme.hint_desc),
                Span::styled(meter(level, bar_w.saturating_sub(2)), theme.progress_filled),
                Span::styled(format!(" {} dB", db_label(level)), theme.hint_desc),
            ],
            theme,
        );
    }
    y.min(bottom) - area.y
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use koan_core::player::state::{QueueEntryStatus, SharedPlayerState};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// An `App` playing a track from a small queue, apart from any player,
    /// with covers in half blocks so a dump shows them.
    pub(crate) fn playing_app() -> App {
        koan_core::config::isolate_config_for_tests();
        let state = SharedPlayerState::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let entries: Vec<QueueEntry> = (0..6)
            .map(|n| QueueEntry {
                playlist_entry_id: None,
                id: QueueItemId::new(),
                db_id: None,
                path: format!("/music/{n}.flac").into(),
                title: format!("A Track Called Number {n}"),
                artist: "Some Artist".into(),
                album_artist: "Some Artist".into(),
                album: "An Album of Some Length".into(),
                year: Some("2007".into()),
                codec: Some("FLAC".into()),
                track_number: Some(n + 1),
                disc: None,
                duration_ms: Some(213_000),
                status: if n == 0 {
                    QueueEntryStatus::Playing
                } else {
                    QueueEntryStatus::Queued
                },
                download_progress: None,
                error: None,
            })
            .collect();
        state.set_track_info(Some(TrackInfo {
            duration_ms: 213_000,
            ..info(44100)
        }));
        let mut app = App::new(state, koan_core::audio::viz::VizSnapshot::new(), tx, 60);
        if let Some(info) = app.state.track_info() {
            let mut entries = entries;
            entries[0].id = info.id;
            entries[0].path = info.path.clone();
            app.queue.vq_cache.entries = entries;
        }
        app.queue.vq_version = app.state.playlist_version();
        app.art_size = 24;
        app.show_fps = false;
        app.viz_fullscreen = false;
        app.viz_config.enabled = true;
        app
    }

    /// The screen as text, a line a row, the trailing half of each wide
    /// character left out.
    pub(crate) fn screen(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| crate::ui::render(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..h)
            .map(|y| {
                let mut line = String::new();
                let mut skip = 0;
                for x in 0..w {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let sym = buf[(x, y)].symbol();
                    skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
                    line.push_str(sym);
                }
                line
            })
            .collect()
    }

    /// Writes the screen under `target/tui-dumps` when `KOAN_TUI_DUMP` is set,
    /// for reading a layout by eye.
    pub(crate) fn dump(name: &str, lines: &[String]) {
        if std::env::var_os("KOAN_TUI_DUMP").is_none() {
            return;
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-dumps");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.txt")), lines.join("\n")).unwrap();
    }

    #[test]
    fn dumps_now_playing() {
        let mut app = playing_app();
        app.go_page(crate::nav::Page::NowPlaying);
        for (w, h) in [(50, 20), (80, 24), (120, 40), (200, 60)] {
            let lines = screen(&mut app, w, h);
            dump(&format!("now-playing-{w}x{h}"), &lines);
        }
        app.go_page(crate::nav::Page::Queue);
        for (w, h) in [(50, 20), (80, 24), (120, 40), (200, 60)] {
            let lines = screen(&mut app, w, h);
            dump(&format!("queue-{w}x{h}"), &lines);
        }
    }
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
    fn the_curve_survives_config_writes_that_leave_the_profiles_alone() {
        let cfg = Arc::new(Config::default());
        let mut curve = EqCurve {
            made_for: Some(("flat".into(), 44100, cfg.clone())),
            total: Some(("flat".into(), vec![1.0])),
            pending: None,
        };
        let mut unrelated = (*cfg).clone();
        unrelated.visualizer.enabled = !unrelated.visualizer.enabled;
        assert_eq!(
            curve.get("flat", 44100, &Arc::new(unrelated)),
            Some(("flat", &[1.0][..]))
        );
        assert!(curve.pending.is_none(), "nothing is rebuilt");

        let mut edited = (*cfg).clone();
        edited.dsp.profiles.push(koan_core::config::DspProfile {
            name: "flat".into(),
            ..Default::default()
        });
        assert_eq!(
            curve.get("flat", 44100, &Arc::new(edited)),
            Some(("flat", &[1.0][..])),
            "the old curve stays up while the new one is worked out"
        );
        assert!(curve.pending.is_some());
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
