//! The record's accent: the colour a cover gives the interface while it plays,
//! worked out as the native apps do (`docs/design/koan-theme.md`, Accent). The
//! sleeve's hue, as a mean weighted by how colourful each sample is, with its
//! lightness and chroma moved into the dark appearance's band in OKLCH.

use std::time::{Duration, Instant};

use image::DynamicImage;

use super::theme::MINT;

type Rgb = (u8, u8, u8);

/// How long the accent takes to arrive. Quick, and never the thing the eye
/// goes to.
const SETTLE: Duration = Duration::from_millis(250);

/// The dark appearance's band of OKLCH lightness, and the chroma floor and
/// ceiling: a muddy sleeve gives a clean colour, a neon one does not flare.
const LIGHTNESS: (f64, f64) = (0.70, 0.85);
const CHROMA: (f64, f64) = (0.10, 0.19);
/// Under this chroma a sleeve has no hue worth carrying.
const NO_HUE: f64 = 0.04;
/// What the accent is read against: `bg` and `surface`.
const GROUNDS: [u32; 2] = [0x1e1e1e, 0x2a2a2a];

/// Hue, saturation and brightness, each `0.0..=1.0`.
fn hsb(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let high = r.max(g).max(b);
    let delta = high - r.min(g).min(b);
    if delta <= 0.0 {
        return (0.0, 0.0, high);
    }
    let hue = if high == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if high == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    (hue / 6.0, delta / high, high)
}

fn from_hsb(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let c = v * s;
    let h6 = h.rem_euclid(1.0) * 6.0;
    let x = c * (1.0 - (h6.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (r + m, g + m, b + m)
}

/// The colour a record reads as: a circular mean of the samples' hue,
/// weighted by their saturation and brightness, so one strong colour on a
/// mostly black sleeve wins rather than being drowned by the black.
/// Near-grey, near-black and blown-out samples say nothing about it.
fn dominant(img: &DynamicImage) -> Option<(f64, f64, f64)> {
    let small = img.thumbnail(32, 32).to_rgb8();
    let (mut x, mut y, mut saturation, mut total) = (0.0, 0.0, 0.0, 0.0);
    for px in small.pixels() {
        let [r, g, b] = px.0.map(|c| f64::from(c) / 255.0);
        let (hue, sat, value) = hsb(r, g, b);
        if sat <= 0.15 || value <= 0.1 || value >= 0.98 {
            continue;
        }
        let weight = sat * value;
        let angle = hue * std::f64::consts::TAU;
        x += angle.cos() * weight;
        y += angle.sin() * weight;
        saturation += sat * weight;
        total += weight;
    }
    if total <= 0.0 {
        return None;
    }
    let hue = (y / total).atan2(x / total) / std::f64::consts::TAU;
    Some(from_hsb(hue, (saturation / total).clamp(0.55, 0.85), 0.80))
}

fn linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn gamma(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Chroma and hue in degrees of an sRGB colour.
fn oklch(r: f64, g: f64, b: f64) -> (f64, f64) {
    let (r, g, b) = (linear(r), linear(g), linear(b));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
    let bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
    (
        (a * a + bb * bb).sqrt(),
        bb.atan2(a).to_degrees().rem_euclid(360.0),
    )
}

/// sRGB for an OKLCH colour, with chroma given up until it is in the gamut.
fn srgb(l: f64, c: f64, h: f64) -> (f64, f64, f64) {
    let (sin, cos) = h.to_radians().sin_cos();
    let mut chroma = c;
    loop {
        let (a, b) = (chroma * cos, chroma * sin);
        let l3 = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
        let m3 = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
        let s3 = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
        let lin = [
            4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3,
            -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3,
            -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3,
        ];
        if lin.iter().all(|v| (-0.0001..=1.0001).contains(v)) || chroma <= 0.0 {
            return (gamma(lin[0]), gamma(lin[1]), gamma(lin[2]));
        }
        chroma -= 0.002;
    }
}

fn luminance((r, g, b): (f64, f64, f64)) -> f64 {
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn contrast(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The most vivid lightness in the band that reads as text on both grounds,
/// which on a dark ground is the darkest; failing that, the lightest that
/// clears 3:1 as a fill.
fn shade(c: f64, h: f64) -> Option<(f64, f64, f64)> {
    let c = c.clamp(CHROMA.0, CHROMA.1);
    let grounds = GROUNDS.map(|hex| {
        let byte = |shift: u32| f64::from((hex >> shift) & 0xff) / 255.0;
        luminance((byte(16), byte(8), byte(0)))
    });
    let steps = (0..=50).map(|i| LIGHTNESS.0 + (LIGHTNESS.1 - LIGHTNESS.0) * f64::from(i) / 50.0);
    let reads = |l: f64, minimum: f64| {
        let rgb = srgb(l, c, h);
        let y = luminance(rgb);
        grounds
            .iter()
            .all(|&g| contrast(y, g) >= minimum)
            .then_some(rgb)
    };
    steps
        .clone()
        .find_map(|l| reads(l, 4.5))
        .or_else(|| steps.rev().find_map(|l| reads(l, 3.0)))
}

/// The accent a cover gives, or `None` when it holds no colour worth taking
/// (a greyscale sleeve): the interface keeps mint then.
pub fn from_image(img: &DynamicImage) -> Option<Rgb> {
    let (r, g, b) = dominant(img)?;
    let (c, h) = oklch(r, g, b);
    if c < NO_HUE {
        return None;
    }
    let (r, g, b) = shade(c, h)?;
    let byte = |v: f64| (v * 255.0).round() as u8;
    Some((byte(r), byte(g), byte(b)))
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

    fn bytes((r, g, b): Rgb) -> (f64, f64, f64) {
        (
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
        )
    }

    #[test]
    fn a_grey_sleeve_gives_no_accent() {
        assert_eq!(from_image(&flat([128, 128, 128])), None);
        assert_eq!(from_image(&flat([0, 0, 0])), None);
        assert_eq!(from_image(&flat([255, 255, 255])), None);
    }

    #[test]
    fn the_hue_is_kept() {
        for (sleeve, hue) in [([200, 20, 30], 0.0), ([30, 160, 40], 1.0 / 3.0)] {
            let accent = from_image(&flat(sleeve)).expect("a colour");
            let (r, g, b) = bytes(accent);
            let (h, _, _) = hsb(r, g, b);
            let off = (h - hue).abs().min(1.0 - (h - hue).abs());
            assert!(off < 0.05, "{sleeve:?} gave {accent:?}");
        }
    }

    #[test]
    fn every_accent_reads_as_text_on_the_dark_grounds() {
        for sleeve in [[200, 20, 30], [20, 40, 200], [230, 220, 20], [10, 30, 12]] {
            let Some(accent) = from_image(&flat(sleeve)) else {
                continue;
            };
            let y = luminance(bytes(accent));
            for ground in [0x1e as f64 / 255.0, 0x2a as f64 / 255.0] {
                let g = luminance((ground, ground, ground));
                assert!(contrast(y, g) >= 4.5, "{sleeve:?} gave {accent:?}");
            }
        }
    }

    #[test]
    fn a_dark_sleeve_gives_a_bright_version_of_its_hue() {
        let (r, g, b) = bytes(from_image(&flat([40, 10, 90])).expect("a violet"));
        assert!(b > r && b > g);
        assert!(luminance((r, g, b)) > 0.2);
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
