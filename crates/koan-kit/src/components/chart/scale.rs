//! Where a value stands on a chart's axis, the ticks worth marking on it,
//! and which of their labels fit. Plain arithmetic, so the rules are tested
//! without a window.

/// How an axis spaces its values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    Linear,
    /// By their logarithm: each decade the same width. Values at or under
    /// zero stand at the axis's low end.
    Log,
}

impl Scale {
    /// The value as the axis spaces it: itself, or its base-10 logarithm.
    pub fn transform(self, v: f64) -> f64 {
        match self {
            Scale::Linear => v,
            Scale::Log => v.max(f64::MIN_POSITIVE).log10(),
        }
    }

    pub fn invert(self, t: f64) -> f64 {
        match self {
            Scale::Linear => t,
            Scale::Log => 10f64.powf(t),
        }
    }
}

/// An axis's range as an affine map from a transformed value to its share
/// of the plot, 0 at the range's low end and 1 at its high: `a * t + b`.
/// Easing `a` and `b` together moves every value on the axis along a
/// straight line between where the two ranges put it, which is what lets
/// a rescale be drawn by the GPU from keyframes alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Map {
    pub a: f64,
    pub b: f64,
}

impl Map {
    pub fn new(scale: Scale, lo: f64, hi: f64) -> Self {
        let (t0, t1) = (scale.transform(lo), scale.transform(hi));
        let span = t1 - t0;
        let span = if span.abs() < 1e-12 { 1.0 } else { span };
        Map {
            a: 1.0 / span,
            b: -t0 / span,
        }
    }

    pub fn unit(&self, t: f64) -> f64 {
        self.a * t + self.b
    }

    /// The transformed value at share `unit` of the plot.
    pub fn at(&self, unit: f64) -> f64 {
        (unit - self.b) / self.a
    }

    /// Partway from `self` to `to`, `e` of the way.
    pub fn lerp(&self, to: &Map, e: f64) -> Map {
        Map {
            a: self.a + (to.a - self.a) * e,
            b: self.b + (to.b - self.b) * e,
        }
    }

    /// The map for a value stored as its share `s` of `[lo, hi]`: the
    /// coefficients a series' program reads, so the GPU never subtracts
    /// two large numbers.
    pub fn framed(&self, lo: f64, hi: f64) -> (f64, f64) {
        (self.a * (hi - lo), self.a * lo + self.b)
    }
}

/// A place worth marking on an axis, and how much its label matters: rank 0
/// is kept first when labels are thinned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tick {
    pub value: f64,
    pub rank: u8,
}

/// The step of about `target` round intervals across `span`: 1, 2 or 5
/// times a power of ten.
pub fn nice_step(span: f64, target: usize) -> f64 {
    let raw = span.abs() / target.max(1) as f64;
    if raw <= 0.0 || !raw.is_finite() {
        return 1.0;
    }
    let magnitude = 10f64.powf(raw.log10().floor());
    let norm = raw / magnitude;
    let m = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    m * magnitude
}

/// Every multiple of `step` from `lo` to `hi`; those that are multiples of
/// twice the step rank first, so thinning keeps every other one.
pub fn every(lo: f64, hi: f64, step: f64) -> Vec<Tick> {
    if step <= 0.0 || !step.is_finite() || hi < lo {
        return Vec::new();
    }
    let slack = step * 1e-9;
    let first = ((lo - slack) / step).ceil() as i64;
    let last = ((hi + slack) / step).floor() as i64;
    if last - first > 1000 {
        return Vec::new();
    }
    (first..=last)
        .map(|k| Tick {
            // Zero as zero, never as minus zero.
            value: if k == 0 { 0.0 } else { k as f64 * step },
            rank: if k % 2 == 0 { 0 } else { 1 },
        })
        .collect()
}

/// About `target` round ticks across a linear range.
pub fn linear(lo: f64, hi: f64, target: usize) -> Vec<Tick> {
    every(lo, hi, nice_step(hi - lo, target))
}

/// The ticks of a logarithmic range: 1, 2 and 5 of each decade, the
/// decades ranked first, then the 2s, then the 5s.
pub fn log(lo: f64, hi: f64) -> Vec<Tick> {
    if lo <= 0.0 || hi <= lo {
        return Vec::new();
    }
    let (d0, d1) = (lo.log10().floor() as i32, hi.log10().ceil() as i32);
    let mut out = Vec::new();
    for d in d0..=d1 {
        for (m, rank) in [(1.0, 0), (2.0, 1), (5.0, 2)] {
            let v = m * 10f64.powi(d);
            if v >= lo * (1.0 - 1e-9) && v <= hi * (1.0 + 1e-9) {
                out.push(Tick { value: v, rank });
            }
        }
    }
    out
}

/// Which labels to draw, given each label's extent along the axis and its
/// rank: taken by rank, then in order along the axis, each kept unless it
/// would come within `gap` of one already kept. Labels thin, never collide.
pub fn thin(spans: &[(f32, f32)], ranks: &[u8], gap: f32) -> Vec<bool> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| (ranks.get(i).copied().unwrap_or(u8::MAX), i));
    let mut kept = vec![false; spans.len()];
    for i in order {
        let (s, e) = spans[i];
        let clear = (0..spans.len())
            .filter(|&j| kept[j])
            .all(|j| e + gap <= spans[j].0 || spans[j].1 + gap <= s);
        if clear {
            kept[i] = true;
        }
    }
    kept
}

/// The nearest of `points` to `at` within `reach`, by index.
pub fn nearest(points: &[(f32, f32)], at: (f32, f32), reach: f32) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| (i, ((p.0 - at.0).powi(2) + (p.1 - at.1).powi(2)).sqrt()))
        .filter(|(_, d)| *d <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_axis_spaces_decades_evenly() {
        let map = Map::new(Scale::Log, 20.0, 20_000.0);
        let unit = |v: f64| map.unit(Scale::Log.transform(v));
        assert!(unit(20.0).abs() < 1e-12);
        assert!((unit(20_000.0) - 1.0).abs() < 1e-12);
        assert!((unit(200.0) - 1.0 / 3.0).abs() < 1e-12);
        assert!((unit(632.455_532) - 0.5).abs() < 1e-6);
        for v in [25.0, 440.0, 9000.0] {
            let back = Scale::Log.invert(map.at(unit(v)));
            assert!((back - v).abs() / v < 1e-12);
        }
    }

    #[test]
    fn a_linear_axis_maps_its_ends_to_the_plot_edges() {
        let map = Map::new(Scale::Linear, -12.0, 6.0);
        assert!(map.unit(-12.0).abs() < 1e-12);
        assert!((map.unit(6.0) - 1.0).abs() < 1e-12);
        assert!((map.at(0.5) - -3.0).abs() < 1e-12);
    }

    /// Easing the coefficients moves a value in a straight line between
    /// its places under the two ranges, so one track per value matches.
    #[test]
    fn a_rescale_moves_each_value_linearly() {
        let from = Map::new(Scale::Linear, -6.0, 6.0);
        let to = Map::new(Scale::Linear, -24.0, 12.0);
        for v in [-20.0, -3.0, 0.0, 5.0] {
            let (u0, u1) = (from.unit(v), to.unit(v));
            for e in [0.0, 0.3, 0.75, 1.0] {
                let drawn = from.lerp(&to, e).unit(v);
                assert!((drawn - (u0 + (u1 - u0) * e)).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn a_framed_map_reads_shares_of_the_box() {
        let map = Map::new(Scale::Linear, 1000.0, 1001.0);
        let (a, b) = map.framed(1000.25, 1000.75);
        assert!((a * 0.0 + b - 0.25).abs() < 1e-12);
        assert!((a * 1.0 + b - 0.75).abs() < 1e-12);
    }

    #[test]
    fn steps_are_round() {
        assert_eq!(nice_step(10.0, 5), 2.0);
        assert_eq!(nice_step(100.0, 4), 50.0);
        assert_eq!(nice_step(0.9, 9), 0.1);
        assert_eq!(nice_step(36.0, 6), 10.0);
    }

    #[test]
    fn every_multiple_in_range_is_a_tick() {
        let ticks = every(-12.0, 12.0, 6.0);
        let values: Vec<f64> = ticks.iter().map(|t| t.value).collect();
        assert_eq!(values, [-12.0, -6.0, 0.0, 6.0, 12.0]);
        assert_eq!(ticks[2].rank, 0);
        assert_eq!(ticks[1].rank, 1);
        assert!(values.iter().all(|v| !(v.is_sign_negative() && *v == 0.0)));
        // A range's ends that land on a multiple are kept despite rounding.
        assert_eq!(every(0.1 + 0.2, 0.9, 0.3).len(), 3);
    }

    #[test]
    fn the_audio_band_has_the_usual_ticks() {
        let values: Vec<f64> = log(20.0, 20_000.0).iter().map(|t| t.value).collect();
        assert_eq!(
            values,
            [
                20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0
            ]
        );
        let decades: Vec<f64> = log(20.0, 20_000.0)
            .iter()
            .filter(|t| t.rank == 0)
            .map(|t| t.value)
            .collect();
        assert_eq!(decades, [100.0, 1000.0, 10_000.0]);
    }

    #[test]
    fn labels_thin_rather_than_collide() {
        // Ten labels 20 wide every 30: all fit with a gap of 10, but not 12.
        let spans: Vec<(f32, f32)> = (0..10)
            .map(|i| (i as f32 * 30.0, i as f32 * 30.0 + 20.0))
            .collect();
        let ranks = vec![0u8; 10];
        assert!(thin(&spans, &ranks, 10.0).iter().all(|k| *k));
        let kept = thin(&spans, &ranks, 12.0);
        assert_eq!(kept.iter().filter(|k| **k).count(), 5);
        // No two kept labels are closer than the gap.
        let kept_spans: Vec<_> = spans.iter().zip(&kept).filter(|(_, k)| **k).collect();
        for pair in kept_spans.windows(2) {
            assert!(pair[1].0.0 - pair[0].0.1 >= 12.0);
        }
    }

    #[test]
    fn thinning_keeps_the_better_ranked_labels() {
        // Three overlapping labels: the middle one ranks first and wins.
        let spans = [(0.0, 20.0), (15.0, 35.0), (30.0, 50.0)];
        assert_eq!(thin(&spans, &[1, 0, 1], 2.0), [false, true, false]);
        assert_eq!(thin(&spans, &[0, 1, 0], 2.0), [true, false, true]);
    }

    #[test]
    fn the_nearest_point_within_reach_is_hit() {
        let points = [(10.0, 10.0), (40.0, 10.0), (12.0, 30.0)];
        assert_eq!(nearest(&points, (14.0, 12.0), 24.0), Some(0));
        assert_eq!(nearest(&points, (13.0, 26.0), 24.0), Some(2));
        assert_eq!(nearest(&points, (200.0, 200.0), 24.0), None);
        assert_eq!(nearest(&points, (35.0, 10.0), 4.0), None);
    }
}
