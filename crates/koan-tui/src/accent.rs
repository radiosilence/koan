//! The record's accent: the colour a cover gives the interface while it plays,
//! as the native apps take it. The most colourful hue the cover holds, moved
//! into a band of lightness where it reads on a dark ground.

use std::time::{Duration, Instant};

use image::DynamicImage;

use super::theme::MINT;

type Rgb = (u8, u8, u8);

/// How long the accent takes to arrive. Quick, and never the thing the eye
/// goes to.
const SETTLE: Duration = Duration::from_millis(250);

const HUES: usize = 12;

fn to_hsl((r, g, b): Rgb) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (byte(r), byte(g), byte(b))
}

/// The accent a cover gives, or `None` when it holds no colour worth taking
/// (a greyscale sleeve): the interface keeps its own then.
pub fn from_image(img: &DynamicImage) -> Option<Rgb> {
    let small = img.thumbnail(32, 32).to_rgb8();
    let mut weight = [0.0f32; HUES];
    let mut sum = [[0.0f32; 3]; HUES];
    for px in small.pixels() {
        let rgb = (px[0], px[1], px[2]);
        let (h, s, l) = to_hsl(rgb);
        if s < 0.25 || !(0.12..=0.92).contains(&l) {
            continue;
        }
        let w = s * (1.0 - (2.0 * l - 1.0).abs());
        let bin = ((h / 360.0 * HUES as f32) as usize).min(HUES - 1);
        weight[bin] += w;
        for (acc, v) in sum[bin].iter_mut().zip([px[0], px[1], px[2]]) {
            *acc += w * v as f32;
        }
    }
    let (bin, &best) = weight
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    // Under a few percent of the picture, the hue is an accident of the
    // sleeve's edges.
    if best < small.pixels().len() as f32 * 0.03 {
        return None;
    }
    let mean = (
        (sum[bin][0] / best).round() as u8,
        (sum[bin][1] / best).round() as u8,
        (sum[bin][2] / best).round() as u8,
    );
    let (h, s, _) = to_hsl(mean);
    Some(from_hsl(h, s.clamp(0.45, 0.8), 0.68))
}

/// The accent in effect, easing from the last one to the next.
pub struct Accent {
    from: Rgb,
    to: Rgb,
    since: Instant,
}

impl Default for Accent {
    fn default() -> Self {
        Self {
            from: MINT,
            to: MINT,
            since: Instant::now(),
        }
    }
}

impl Accent {
    /// Where the accent is at `now`.
    pub fn at(&self, now: Instant) -> Rgb {
        let t = (now.saturating_duration_since(self.since).as_secs_f32() / SETTLE.as_secs_f32())
            .min(1.0);
        // Quick out: most of the way in the first half.
        let t = 1.0 - (1.0 - t) * (1.0 - t);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        (
            mix(self.from.0, self.to.0),
            mix(self.from.1, self.to.1),
            mix(self.from.2, self.to.2),
        )
    }

    pub fn target(&self) -> Rgb {
        self.to
    }

    /// Whether the accent is still moving.
    pub fn easing(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.since) < SETTLE
    }

    /// Starts easing to `to`, from wherever the accent is now.
    pub fn set(&mut self, to: Rgb, now: Instant) {
        if to == self.to {
            return;
        }
        self.from = self.at(now);
        self.to = to;
        self.since = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb as P, RgbImage};

    fn flat(rgb: [u8; 3]) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(64, 64, P(rgb)))
    }

    #[test]
    fn a_grey_sleeve_gives_no_accent() {
        assert_eq!(from_image(&flat([128, 128, 128])), None);
        assert_eq!(from_image(&flat([0, 0, 0])), None);
    }

    #[test]
    fn a_red_sleeve_gives_a_red_that_reads_on_dark() {
        let (r, g, b) = from_image(&flat([200, 20, 30])).expect("a red");
        assert!(r > g && r > b);
        let (_, _, l) = to_hsl((r, g, b));
        assert!((0.6..=0.75).contains(&l), "lightness {l}");
    }

    #[test]
    fn the_most_colourful_hue_wins_over_the_most_common() {
        let mut img = RgbImage::from_pixel(64, 64, P([90, 90, 95]));
        for x in 0..32 {
            for y in 0..64 {
                img.put_pixel(x, y, P([20, 60, 220]));
            }
        }
        let (r, _, b) = from_image(&DynamicImage::ImageRgb8(img)).expect("a blue");
        assert!(b > r);
    }

    #[test]
    fn the_accent_eases_and_arrives() {
        let t0 = Instant::now();
        let mut a = Accent::default();
        a.set((255, 0, 0), t0);
        assert_eq!(a.at(t0), MINT);
        assert_eq!(a.at(t0 + SETTLE * 2), (255, 0, 0));
        let mid = a.at(t0 + SETTLE / 2);
        assert!(mid.0 > MINT.0 && mid.0 < 255);
    }
}
