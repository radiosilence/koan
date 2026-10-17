//! What an EQ does to the sound, drawn in braille on a log-frequency axis:
//! 20 Hz to 20 kHz across, gain up the side. The curves come from the core,
//! computed from the filters the DSP runs; this only draws them.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::theme::Theme;
use super::visualizer::BrailleGrid;

const LOW_HZ: f64 = 20.0;
const HIGH_HZ: f64 = 20_000.0;

/// A line on the graph: its dB on `freqs`, its colour, and its dash, as
/// on and off runs of dots; empty is solid.
pub struct Curve<'a> {
    pub db: &'a [f64],
    pub color: Color,
    pub dash: &'static [usize],
}

/// A band's point: where it sits, drawn over the curves.
pub struct Handle {
    pub hz: f64,
    pub db: f64,
    pub selected: bool,
}

pub struct EqGraph<'a> {
    pub freqs: &'a [f64],
    /// Drawn in order, the last on top.
    pub curves: Vec<Curve<'a>>,
    pub handles: Vec<Handle>,
    /// A dashed rule at the level the curves sit at, the preamp, where it
    /// is not 0 dB.
    pub level: f64,
    /// Said across the top of the plot, as "flat: plays untouched".
    pub note: Option<&'a str>,
}

/// The dashes a stage is drawn with, the same in its row and on the graph.
pub const SOLID: &[usize] = &[];
pub const DASHES: [&[usize]; 4] = [&[5, 3], &[2, 2], &[8, 3, 2, 3], &[1, 3]];

/// Where `hz` falls across `width`, from 0.
pub fn x_of(hz: f64, width: usize) -> usize {
    let t = (hz.clamp(LOW_HZ, HIGH_HZ) / LOW_HZ).log10() / (HIGH_HZ / LOW_HZ).log10();
    (t * width.saturating_sub(1) as f64).round() as usize
}

/// `db` at `hz`, interpolated against log frequency and held past either end.
pub fn at(freqs: &[f64], db: &[f64], hz: f64) -> f64 {
    let n = freqs.len().min(db.len());
    if n == 0 {
        return 0.0;
    }
    if hz <= freqs[0] {
        return db[0];
    }
    if hz >= freqs[n - 1] {
        return db[n - 1];
    }
    let i = freqs[..n].partition_point(|&f| f < hz).max(1);
    let (f0, f1) = (freqs[i - 1], freqs[i]);
    let t = (hz / f0).ln() / (f1 / f0).ln();
    db[i - 1] + t * (db[i] - db[i - 1])
}

/// The dB at the top edge: the tallest curve, at least 6 dB, in steps of 3.
pub fn range(curves: &[Curve<'_>], level: f64, handles: &[Handle]) -> f64 {
    let peak = curves
        .iter()
        .flat_map(|c| c.db.iter())
        .map(|v| v.abs())
        .chain(handles.iter().map(|h| h.db.abs()))
        .chain(std::iter::once(level.abs()))
        .filter(|v| v.is_finite())
        .fold(6.0f64, f64::max);
    (peak / 3.0).ceil() * 3.0
}

impl EqGraph<'_> {
    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.width < 12 || area.height < 3 {
            return;
        }
        let label_w = 4u16;
        let plot = Rect::new(
            area.x + label_w,
            area.y,
            area.width - label_w,
            area.height - 1,
        );
        let range = range(&self.curves, self.level, &self.handles);
        let mut grid = BrailleGrid::new(plot.width as usize, plot.height as usize);
        let (pw, ph) = (grid.px_width(), grid.px_height());
        let y_of = |v: f64| {
            ((1.0 - (v + range) / (2.0 * range)) * (ph - 1) as f64).clamp(0.0, (ph - 1) as f64)
        };
        let rule = theme.tokens.rule;
        // Decades, faint, so a frequency can be read off the curve.
        for hz in [100.0, 1000.0, 10_000.0] {
            let x = x_of(hz, pw);
            for y in (0..ph).step_by(3) {
                grid.set_dot(x, y, rule);
            }
        }
        let zero = y_of(0.0).round() as usize;
        for x in (0..pw).step_by(2) {
            grid.set_dot(x, zero, rule);
        }
        if self.level.abs() >= 0.05 {
            let y = y_of(self.level).round() as usize;
            for x in (0..pw).filter(|x| x % 6 < 3) {
                grid.set_dot(x, y, theme.tokens.muted);
            }
        }
        for curve in &self.curves {
            if curve.db.is_empty() {
                continue;
            }
            let period: usize = curve.dash.iter().sum();
            let mut prev: Option<(f32, f32)> = None;
            for x in 0..pw {
                let hz = LOW_HZ * (HIGH_HZ / LOW_HZ).powf(x as f64 / (pw - 1).max(1) as f64);
                let point = (x as f32, y_of(at(self.freqs, curve.db, hz)) as f32);
                let on = period == 0 || {
                    let mut at = x % period;
                    let mut on = true;
                    for &run in curve.dash {
                        if at < run {
                            break;
                        }
                        at -= run;
                        on = !on;
                    }
                    on
                };
                if on {
                    match prev {
                        Some((px, py)) => grid.draw_line(px, py, point.0, point.1, curve.color),
                        None => {
                            grid.set_dot(x, point.1.round() as usize, curve.color);
                        }
                    }
                    prev = Some(point);
                } else {
                    prev = None;
                }
            }
        }
        grid.render_to(plot, buf);
        for h in &self.handles {
            let cx = plot.x + x_of(h.hz, plot.width as usize) as u16;
            let cy = plot.y + (y_of(h.db) / 4.0).floor().min((plot.height - 1) as f64) as u16;
            let (glyph, style) = if h.selected {
                (
                    "\u{25c9}",
                    Style::new()
                        .fg(theme.tokens.accent)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("\u{25cb}", Style::new().fg(theme.tokens.ink))
            };
            buf.set_string(cx, cy, glyph, style);
        }
        if let Some(note) = self.note {
            let w = note.chars().count() as u16;
            if w + 2 <= plot.width && plot.height > 2 {
                let x = plot.x + (plot.width - w) / 2;
                buf.set_string(x, plot.y + 1, note, theme.hint_desc);
            }
        }
        let style = theme.hint_desc;
        let label = |v: f64| format!("{:>+3.0}", v);
        buf.set_string(area.x, plot.y, label(range), style);
        if plot.height >= 3 {
            buf.set_string(area.x, plot.y + (zero / 4) as u16, label(0.0), style);
        }
        buf.set_string(area.x, plot.y + plot.height - 1, label(-range), style);
        let axis_y = area.y + area.height - 1;
        let mut free = plot.x;
        for (hz, text) in [
            (20.0f64, "20"),
            (50.0, "50"),
            (100.0, "100"),
            (200.0, "200"),
            (500.0, "500"),
            (1000.0, "1k"),
            (2000.0, "2k"),
            (5000.0, "5k"),
            (10_000.0, "10k"),
            (20_000.0, "20k"),
        ] {
            let w = text.len() as u16;
            let centre = plot.x + x_of(hz, plot.width as usize) as u16;
            let x = centre
                .saturating_sub(w / 2)
                .max(plot.x)
                .min(plot.x + plot.width - w);
            if x >= free {
                buf.set_string(x, axis_y, text, style);
                free = x + w + 1;
            }
        }
    }
}

/// A curve as one row of braille, for a stage's row: its shape at a glance,
/// on a scale of its own peak, at least 6 dB.
pub fn thumb(area: Rect, buf: &mut Buffer, freqs: &[f64], db: &[f64], color: Color) {
    if area.width < 2 || area.height == 0 || db.is_empty() {
        return;
    }
    let mut grid = BrailleGrid::new(area.width as usize, 1);
    let (pw, ph) = (grid.px_width(), grid.px_height());
    let range = db.iter().fold(6.0f64, |m, v| m.max(v.abs()));
    let mut prev: Option<(f32, f32)> = None;
    for x in 0..pw {
        let hz = LOW_HZ * (HIGH_HZ / LOW_HZ).powf(x as f64 / (pw - 1).max(1) as f64);
        let v = at(freqs, db, hz);
        let y = ((1.0 - (v + range) / (2.0 * range)) * (ph - 1) as f64).clamp(0.0, (ph - 1) as f64);
        let point = (x as f32, y as f32);
        if let Some((px, py)) = prev {
            grid.draw_line(px, py, point.0, point.1, color);
        }
        prev = Some(point);
    }
    grid.render_to(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_grid() -> Vec<f64> {
        (0..120)
            .map(|i| 20.0 * 1000f64.powf(i as f64 / 119.0))
            .collect()
    }

    #[test]
    fn the_axis_is_logarithmic() {
        assert_eq!(x_of(20.0, 101), 0);
        assert_eq!(x_of(20_000.0, 101), 100);
        // 632 Hz is halfway across three decades.
        assert_eq!(x_of(632.0, 101), 50);
        assert_eq!(x_of(1.0, 101), 0, "held to the ends");
    }

    #[test]
    fn a_curve_is_read_between_its_points() {
        let freqs = [100.0, 1000.0];
        let db = [0.0, 10.0];
        assert!((at(&freqs, &db, 316.2) - 5.0).abs() < 0.01);
        assert_eq!(at(&freqs, &db, 10.0), 0.0);
        assert_eq!(at(&freqs, &db, 5000.0), 10.0);
        assert_eq!(at(&[], &[], 5000.0), 0.0);
    }

    #[test]
    fn the_scale_grows_in_steps_of_three() {
        let db = [0.0, 7.2, -2.0];
        let curves = [Curve {
            db: &db,
            color: Color::Reset,
            dash: SOLID,
        }];
        assert_eq!(range(&curves, 0.0, &[]), 9.0);
        assert_eq!(range(&[], 0.0, &[]), 6.0);
        assert_eq!(range(&[], -13.0, &[]), 15.0);
    }

    #[test]
    fn it_draws_in_any_area() {
        let theme = Theme::default();
        let freqs = log_grid();
        let db: Vec<f64> = freqs.iter().map(|f| (f / 1000.0).log10() * 4.0).collect();
        for w in [0u16, 1, 5, 12, 13, 40, 120] {
            for h in [0u16, 1, 2, 3, 4, 12, 40] {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                EqGraph {
                    freqs: &freqs,
                    curves: vec![Curve {
                        db: &db,
                        color: Color::Red,
                        dash: DASHES[2],
                    }],
                    handles: vec![Handle {
                        hz: 1000.0,
                        db: 30.0,
                        selected: true,
                    }],
                    level: -4.0,
                    note: Some("flat: plays untouched"),
                }
                .render(area, &mut buf, &theme);
                thumb(area, &mut buf, &freqs, &db, Color::Red);
            }
        }
    }
}
