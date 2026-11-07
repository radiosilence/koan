//! A series as the GPU reads it: one keyframe of the data, from what is
//! drawn now to the newest values, packed into a small image the series'
//! program samples.
//!
//! Each value is stored as its share of the series' box (`lo` to `hi`) in
//! 24 bits, across a texel's blue, green and red. The decoding is linear in
//! the channels, so even a filtered sample reads a value between its
//! neighbours rather than garbage, and at a texel's centre it reads the
//! value exactly. Rows: 0 the x of each sample (transformed by the axis's
//! scale), 1 and 2 the y it moves from and to, 3 and 4 an auxiliary value
//! moving with it (a line's length so far, for its dashes; or an area's
//! baseline).

use std::sync::Arc;

use gpui::RenderImage;
use smallvec::SmallVec;

/// Rows of a series' image.
pub const ROWS: u32 = 5;
/// The largest share code: 24 bits.
const FULL: f64 = 16_777_215.0;

/// `share` of a box, 0 to 1, as a texel: blue, green and red, high byte
/// first, in GPUI's BGRA order.
pub fn encode(share: f64) -> [u8; 4] {
    let q = (share.clamp(0.0, 1.0) * FULL).round() as u32;
    [(q >> 16) as u8, (q >> 8) as u8, q as u8, 255]
}

/// What a program reads back from a texel `encode` wrote.
#[cfg(test)]
pub fn decode(texel: [u8; 4]) -> f64 {
    (texel[0] as f64 * 65_536.0 + texel[1] as f64 * 256.0 + texel[2] as f64) / FULL
}

/// A range values are stored as shares of, never empty.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub lo: f64,
    pub hi: f64,
}

impl Span {
    pub fn of(values: impl IntoIterator<Item = f64>) -> Span {
        let (lo, hi) = values
            .into_iter()
            .filter(|v| v.is_finite())
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(v), hi.max(v))
            });
        if !lo.is_finite() {
            return Span { lo: 0.0, hi: 1.0 };
        }
        if hi - lo < 1e-9 {
            let pad = lo.abs().max(1.0) * 1e-6;
            return Span {
                lo: lo - pad,
                hi: hi + pad,
            };
        }
        Span { lo, hi }
    }

    pub fn share(&self, v: f64) -> f64 {
        (v - self.lo) / (self.hi - self.lo)
    }
}

/// One keyframe of a series: its samples' x, the y each moves from and to,
/// and the auxiliary row's from and to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keyframe {
    pub xs: Vec<f64>,
    pub from: Vec<f64>,
    pub to: Vec<f64>,
    pub aux_from: Vec<f64>,
    pub aux_to: Vec<f64>,
}

impl Keyframe {
    /// Standing at `ys`.
    #[cfg(test)]
    pub fn still(xs: Vec<f64>, ys: Vec<f64>) -> Self {
        Keyframe {
            from: ys.clone(),
            to: ys,
            aux_from: vec![0.0; xs.len()],
            aux_to: vec![0.0; xs.len()],
            xs,
        }
    }

    pub fn len(&self) -> usize {
        self.xs.len()
    }

    /// Its y as drawn `phase` of the way from `from` to `to`, sample by
    /// sample: what a new keyframe moves from, read at its own samples'
    /// places with `sample`.
    pub fn drawn(&self, phase: f64) -> Vec<f64> {
        mix(&self.from, &self.to, phase)
    }

    /// The auxiliary row as drawn.
    pub fn drawn_aux(&self, phase: f64) -> Vec<f64> {
        mix(&self.aux_from, &self.aux_to, phase)
    }

    /// The box its y values are stored in.
    pub fn y_span(&self, also: &[f64]) -> Span {
        Span::of(self.from.iter().chain(&self.to).chain(also).copied())
    }

    /// The image the series' programs sample, with the boxes its rows are
    /// stored in: x, y (shared by an area's baseline) and the auxiliary row.
    pub fn image(&self, x: Span, y: Span, aux: Span) -> Arc<RenderImage> {
        let n = self.len().max(1);
        let mut bgra = vec![0u8; n * ROWS as usize * 4];
        let rows: [(&[f64], Span); 5] = [
            (&self.xs, x),
            (&self.from, y),
            (&self.to, y),
            (&self.aux_from, aux),
            (&self.aux_to, aux),
        ];
        for (r, (values, span)) in rows.iter().enumerate() {
            for (i, v) in values.iter().enumerate().take(n) {
                let at = (r * n + i) * 4;
                bgra[at..at + 4].copy_from_slice(&encode(span.share(*v)));
            }
        }
        image(n as u32, ROWS, bgra)
    }
}

fn mix(from: &[f64], to: &[f64], phase: f64) -> Vec<f64> {
    from.iter()
        .zip(to)
        .map(|(f, t)| f + (t - f) * phase)
        .collect()
}

/// An image of `width` by `height` texels from BGRA bytes.
pub fn image(width: u32, height: u32, bgra: Vec<u8>) -> Arc<RenderImage> {
    let frame = image::Frame::new(
        image::RgbaImage::from_raw(width, height, bgra).expect("the image holds its texels"),
    );
    Arc::new(RenderImage::new(SmallVec::from_elem(frame, 1)))
}

/// `ys` at `x`, read along the line through `(xs, ys)`, held at its ends.
/// `xs` rise.
pub fn sample(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    let n = xs.len().min(ys.len());
    if n == 0 {
        return 0.0;
    }
    if x <= xs[0] || n == 1 {
        return ys[0];
    }
    if x >= xs[n - 1] {
        return ys[n - 1];
    }
    let i = xs[..n].partition_point(|v| *v <= x).saturating_sub(1);
    let (x0, x1) = (xs[i], xs[i + 1]);
    let t = if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.0 };
    ys[i] + (ys[i + 1] - ys[i]) * t
}

/// The indices worth drawing of `n` samples across `width` device pixels:
/// every one while there are at most two to a pixel, else an even pick that
/// keeps both ends.
pub fn decimated(n: usize, width: f32) -> Vec<usize> {
    let most = ((2.0 * width).ceil() as usize).max(2);
    if n <= most {
        return (0..n).collect();
    }
    let every = n.div_ceil(most);
    let mut picked: Vec<usize> = (0..n).step_by(every).collect();
    if picked.last() != Some(&(n - 1)) {
        picked.push(n - 1);
    }
    picked
}

/// How far along the line through `points` each one is, from the first.
pub fn lengths(points: &[(f64, f64)]) -> Vec<f64> {
    let mut out = Vec::with_capacity(points.len());
    let mut total = 0.0;
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            let q = points[i - 1];
            total += ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt();
        }
        out.push(total);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_survives_its_texel_to_24_bits() {
        for share in [0.0, 1.0, 0.5, 0.123_456_7, 0.999_999] {
            let back = decode(encode(share));
            assert!(
                (back - share).abs() <= 0.5 / FULL + 1e-15,
                "{share} came back {back}"
            );
        }
        assert_eq!(decode(encode(-1.0)), 0.0);
        assert_eq!(decode(encode(2.0)), 1.0);
    }

    /// A filtered read between two texels lands between their values, as
    /// the encoding is linear in the channels.
    #[test]
    fn a_blend_of_texels_reads_a_blend_of_values() {
        let (a, b) = (encode(0.2), encode(0.7));
        let mix = |w: f64| {
            let channel = |k: usize| a[k] as f64 * (1.0 - w) + b[k] as f64 * w;
            (channel(0) * 65_536.0 + channel(1) * 256.0 + channel(2)) / FULL
        };
        assert!((mix(0.25) - 0.325).abs() < 1e-6);
    }

    #[test]
    fn a_span_is_never_empty() {
        let s = Span::of([3.0, 3.0]);
        assert!(s.hi > s.lo);
        assert!((s.share(3.0) - 0.5).abs() < 1e-9);
        assert_eq!(Span::of([]), Span { lo: 0.0, hi: 1.0 });
        let s = Span::of([-2.0, f64::NAN, 6.0]);
        assert_eq!((s.lo, s.hi), (-2.0, 6.0));
    }

    #[test]
    fn values_are_read_along_the_line() {
        let xs = [0.0, 1.0, 3.0];
        let ys = [0.0, 10.0, 30.0];
        assert_eq!(sample(&xs, &ys, -1.0), 0.0);
        assert_eq!(sample(&xs, &ys, 0.5), 5.0);
        assert_eq!(sample(&xs, &ys, 2.0), 20.0);
        assert_eq!(sample(&xs, &ys, 1.0), 10.0);
        assert_eq!(sample(&xs, &ys, 9.0), 30.0);
    }

    /// The next keyframe starts from where the last is drawn, at the new
    /// samples' places.
    #[test]
    fn a_keyframe_is_read_partway() {
        let k = Keyframe {
            xs: vec![0.0, 1.0],
            from: vec![0.0, 0.0],
            to: vec![10.0, 20.0],
            aux_from: vec![0.0, 0.0],
            aux_to: vec![0.0, 0.0],
        };
        assert_eq!(sample(&k.xs, &k.drawn(0.5), 0.0), 5.0);
        assert_eq!(sample(&k.xs, &k.drawn(0.5), 0.5), 7.5);
        // Past its end, as a series drawn ahead of its samples is.
        assert_eq!(sample(&k.xs, &k.drawn(1.5), 1.0), 30.0);
    }

    #[test]
    fn dense_data_is_thinned_to_two_samples_a_pixel() {
        assert_eq!(decimated(100, 300.0).len(), 100);
        let picked = decimated(10_000, 300.0);
        assert!(picked.len() <= 601);
        assert_eq!(picked[0], 0);
        assert_eq!(*picked.last().unwrap(), 9_999);
        assert!(picked.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn lengths_add_up_along_the_line() {
        let l = lengths(&[(0.0, 0.0), (3.0, 4.0), (3.0, 10.0)]);
        assert_eq!(l, [0.0, 5.0, 11.0]);
    }

    #[test]
    fn the_image_holds_each_row() {
        let k = Keyframe::still(vec![0.0, 2.0], vec![1.0, 3.0]);
        let x = Span { lo: 0.0, hi: 2.0 };
        let y = Span { lo: 1.0, hi: 3.0 };
        let image = k.image(x, y, Span { lo: 0.0, hi: 1.0 });
        let bytes = image.as_bytes(0).unwrap();
        assert_eq!(bytes.len(), 2 * ROWS as usize * 4);
        let texel = |row: usize, i: usize| {
            let at = (row * 2 + i) * 4;
            [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
        };
        assert_eq!(decode(texel(0, 1)), 1.0);
        assert_eq!(decode(texel(2, 0)), 0.0);
        assert_eq!(decode(texel(2, 1)), 1.0);
    }
}
