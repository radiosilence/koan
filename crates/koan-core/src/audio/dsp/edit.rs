//! The rules a hand edit of an EQ follows, shared by every front end: the
//! ranges a band is held to, the band types that can be chosen and dragged,
//! and how a brush pulls a graphic curve. `profiles::set_band` and
//! `set_curve` hold what they are given to the same ranges, so a front end
//! that clamps to these never shows a value the save would change.

use std::ops::RangeInclusive;

/// The ranges a band edited by hand is held to: the audible band, what a
/// correction plausibly asks, and Q from very broad to very narrow.
pub const BAND_HZ: RangeInclusive<f64> = 10.0..=22_000.0;
pub const BAND_DB: RangeInclusive<f64> = -30.0..=30.0;
pub const BAND_Q: RangeInclusive<f64> = 0.1..=20.0;

/// A band type a hand edit can choose: its config name, the short form a
/// dense table shows, its name in a menu, and whether a graph draws a handle
/// for it, which only types with a gain to drag have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandKind {
    pub id: &'static str,
    pub short: &'static str,
    pub name: &'static str,
    pub draggable: bool,
}

/// In the order a menu lists them.
pub const BAND_KINDS: [BandKind; 8] = [
    kind("peaking", "PK", "Peak", true),
    kind("low_shelf", "LS", "Low shelf", true),
    kind("high_shelf", "HS", "High shelf", true),
    kind("low_pass", "LP", "Low pass", false),
    kind("high_pass", "HP", "High pass", false),
    kind("notch", "NO", "Notch", false),
    kind("band_pass", "BP", "Band pass", false),
    kind("all_pass", "AP", "All pass", false),
];

const fn kind(
    id: &'static str,
    short: &'static str,
    name: &'static str,
    draggable: bool,
) -> BandKind {
    BandKind {
        id,
        short,
        name,
        draggable,
    }
}

/// The band type called `id` in the config, where a hand edit can choose it.
pub fn band_kind(id: &str) -> Option<BandKind> {
    BAND_KINDS.iter().copied().find(|k| k.id == id)
}

/// How much of a graphic curve a stroke pulls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brush {
    /// The nearest point alone.
    Point,
    /// Its neighbours too, less the further they are, over a third of an
    /// octave.
    Narrow,
    /// The same over an octave.
    Wide,
}

/// The curve `points` pulled toward `db` at `hz`. Points the brush barely
/// reaches keep what they had, and what it paints is kept to a hundredth of
/// a dB, so a stroke leaves figures a person can read back.
pub fn paint(points: &[(f64, f64)], hz: f64, db: f64, brush: Brush) -> Vec<(f64, f64)> {
    let octaves = |p: f64| (p / hz).log2().abs();
    let width = match brush {
        Brush::Point => {
            let mut out = points.to_vec();
            let nearest = (0..points.len())
                .min_by(|&a, &b| octaves(points[a].0).total_cmp(&octaves(points[b].0)));
            if let Some(i) = nearest {
                out[i].1 = db;
            }
            return out;
        }
        Brush::Narrow => 1.0 / 3.0,
        Brush::Wide => 1.0,
    };
    points
        .iter()
        .map(|&(p_hz, p_db)| {
            let d = octaves(p_hz) / width;
            let reach = (-d * d).exp();
            if reach <= 0.01 {
                return (p_hz, p_db);
            }
            (p_hz, ((p_db + reach * (db - p_db)) * 100.0).round() / 100.0)
        })
        .collect()
}

/// Q widened or narrowed by a vertical drag of `dy` points from where it
/// stood at `start`: up, a negative `dy`, for a higher Q. Doubles every 60
/// points, held to `BAND_Q`.
pub fn drag_q(start: f64, dy: f64) -> f64 {
    (start * 2f64.powf(-dy / 60.0)).clamp(*BAND_Q.start(), *BAND_Q.end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_brush_moves_only_the_nearest() {
        let curve = [(100.0, 0.0), (200.0, 0.0), (400.0, 0.0)];
        assert_eq!(
            paint(&curve, 210.0, 3.0, Brush::Point),
            vec![(100.0, 0.0), (200.0, 3.0), (400.0, 0.0)]
        );
    }

    #[test]
    fn a_narrow_brush_falls_off_and_rounds_what_it_paints() {
        let curve = [(100.0, 0.0), (1000.0, 0.0), (1100.0, 0.0)];
        let painted = paint(&curve, 1000.0, 6.0, Brush::Narrow);
        // Over three octaves away: untouched.
        assert_eq!(painted[0], (100.0, 0.0));
        assert_eq!(painted[1], (1000.0, 6.0));
        let near = painted[2].1;
        assert!(near > 0.0 && near < 6.0);
        assert_eq!((near * 100.0).round() / 100.0, near);
    }

    #[test]
    fn a_wide_brush_reaches_further_than_a_narrow_one() {
        let curve = [(500.0, 0.0), (1000.0, 0.0)];
        let narrow = paint(&curve, 1000.0, 6.0, Brush::Narrow)[0].1;
        let wide = paint(&curve, 1000.0, 6.0, Brush::Wide)[0].1;
        assert!(wide > narrow);
    }

    #[test]
    fn a_painted_point_off_the_brush_keeps_every_digit() {
        let curve = [(20.0, 1.234_567), (10_000.0, 0.0)];
        assert_eq!(
            paint(&curve, 10_000.0, 3.0, Brush::Narrow)[0],
            (20.0, 1.234_567)
        );
    }

    #[test]
    fn q_doubles_per_sixty_points_up_and_stays_in_range() {
        assert!((drag_q(1.0, -60.0) - 2.0).abs() < 1e-12);
        assert!((drag_q(1.0, 60.0) - 0.5).abs() < 1e-12);
        assert_eq!(drag_q(10.0, -600.0), 20.0);
        assert_eq!(drag_q(1.0, 6000.0), 0.1);
    }

    #[test]
    fn only_types_with_a_gain_are_dragged() {
        let dragged: Vec<_> = BAND_KINDS
            .iter()
            .filter(|k| k.draggable)
            .map(|k| k.id)
            .collect();
        assert_eq!(dragged, ["peaking", "low_shelf", "high_shelf"]);
        assert!(band_kind("graphic").is_none());
    }
}
