//! The record accent: the sleeve's hue, its lightness and chroma moved into a
//! band per appearance in OKLCH, so a dark or muddy sleeve gives a clean,
//! bright version of its hue and light mode never goes pastel. Mint when
//! there is no record or no usable hue.
//!
//! `Color.dominant` (apps/macos/Sources/Koan/Support/Palette.swift) and
//! `KoanAccent` (Support/KoanTheme.swift), value for value, as the Android
//! app ports them, so a record gives the same accent on every device.

use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::LazyLock;

use gpui_kit::{Hsla, Rgba};
use parking_lot::Mutex;

use super::tokens::Palette;

/// One appearance's accent, as gamma-encoded sRGB in 0..=1. `reads_as_text`
/// is whether it reaches 4.5:1 on `bg` and `surface`; where it does not, it
/// is used for fills, indicators and rings only, and text stays `ink`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shade {
    pub rgb: [f64; 3],
    pub reads_as_text: bool,
    /// The colour slots it is drawn through, as fills and as text, while the
    /// accent moves on the GPU: what `hsla` and `text_hsla` name. `rgb` is
    /// where the slots are headed.
    pub slot: Option<[Hsla; 2]>,
}

impl Shade {
    pub fn rgba(self) -> Rgba {
        Rgba {
            r: self.rgb[0] as f32,
            g: self.rgb[1] as f32,
            b: self.rgb[2] as f32,
            a: 1.0,
        }
    }

    /// The colour to draw fills, rings and indicators in: the accent's slot
    /// when it has one, so a new record's accent settles with no frame drawn.
    pub fn hsla(self) -> Hsla {
        self.slot
            .map_or_else(|| self.rgba().into(), |[fill, _]| fill)
    }

    /// The colour to draw text in: the accent where it reads as text, `ink`
    /// where it does not.
    pub fn text_hsla(self, ink: Hsla) -> Hsla {
        match self.slot {
            Some([_, text]) => text,
            None if self.reads_as_text => self.rgba().into(),
            None => ink,
        }
    }

    /// With more contrast asked for: the same hue, its lightness moved away
    /// from the ground (lighter in dark mode, darker in light) until it
    /// reaches `minimum` on `bg` and `surface`. Worked out once per shade.
    pub fn held(self, minimum: f64, dark: bool, tokens: Palette) -> Shade {
        // The grounds it is held against are part of what it is: the same
        // shade held on another palette is another shade.
        type Key = ([u64; 3], bool, u64, [u32; 2]);
        static CACHE: LazyLock<Mutex<HashMap<Key, Shade>>> = LazyLock::new(Default::default);
        let key = (
            self.rgb.map(f64::to_bits),
            dark,
            minimum.to_bits(),
            [tokens.bg.0, tokens.surface.0],
        );
        // Kept by colour alone: the slot it is drawn through is the caller's.
        if let Some(held) = CACHE.lock().get(&key) {
            return Shade {
                slot: self.slot,
                ..*held
            };
        }
        let channels = |hex: super::tokens::Hex| hex.channels().map(|v| v as f64 / 255.0);
        let bg = oklch::luminance(channels(tokens.bg));
        let surface = oklch::luminance(channels(tokens.surface));
        let contrast = |a: f64, b: f64| (a.max(b) + 0.05) / (a.min(b) + 0.05);
        let reaches = |rgb: [f64; 3]| {
            let y = oklch::luminance(rgb);
            contrast(y, bg) >= minimum && contrast(y, surface) >= minimum
        };
        let (l, c, h) = oklch::from_srgb(self.rgb);
        let made = (0..=100)
            .map(|i| {
                let step = i as f64 / 100.0;
                let l = if dark {
                    l + (1.0 - l) * step
                } else {
                    l * (1.0 - step)
                };
                oklch::to_srgb(l, c, h)
            })
            .find(|rgb| reaches(*rgb))
            .map_or(self, |rgb| Shade {
                rgb,
                reads_as_text: true,
                slot: self.slot,
            });
        let mut cache = CACHE.lock();
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(key, made);
        made
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accent {
    pub dark: Shade,
    pub light: Shade,
}

/// Lightness bands, per appearance: never dark in dark mode, never pastel in
/// light.
const DARK_BAND: (f64, f64) = (0.70, 0.85);
const LIGHT_BAND: (f64, f64) = (0.45, 0.60);
/// Chroma floor and ceiling: a muddy sleeve gives a clean colour, a neon one
/// does not flare.
const CHROMA: (f64, f64) = (0.10, 0.19);
/// Under this, a sleeve has no hue worth carrying, and the accent is mint.
const NO_HUE: f64 = 0.04;

impl Accent {
    pub const MINT: Accent = Accent {
        dark: Shade {
            rgb: [
                0x7D as f64 / 255.0,
                0xD3 as f64 / 255.0,
                0xA7 as f64 / 255.0,
            ],
            reads_as_text: true,
            slot: None,
        },
        light: Shade {
            rgb: [
                0x1F as f64 / 255.0,
                0x7A as f64 / 255.0,
                0x50 as f64 / 255.0,
            ],
            reads_as_text: true,
            slot: None,
        },
    };

    pub fn shade(&self, dark: bool) -> Shade {
        if dark { self.dark } else { self.light }
    }

    /// The accent for a record's colour, as `dominant` gives it; `None` is no
    /// record, or one with no colour. Worked out once per colour, so the same
    /// record always gives the same value.
    pub fn of(record: Option<[f64; 3]>) -> Accent {
        static CACHE: LazyLock<Mutex<HashMap<[u64; 3], Accent>>> = LazyLock::new(Default::default);
        let Some(record) = record else {
            return Accent::MINT;
        };
        let key = record.map(f64::to_bits);
        if let Some(held) = CACHE.lock().get(&key) {
            return *held;
        }
        let made = Self::make(record);
        let mut cache = CACHE.lock();
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(key, made);
        made
    }

    /// The accent for a sleeve, as RGBA pixels.
    pub fn of_sleeve(rgba: &[u8], width: u32, height: u32) -> Accent {
        Self::of(dominant(rgba, width, height))
    }

    fn make(record: [f64; 3]) -> Accent {
        let (_, c, h) = oklch::from_srgb(record);
        if c < NO_HUE {
            return Accent::MINT;
        }
        let (Some(dark), Some(light)) = (
            shade(h, c, DARK_BAND, true, Palette::DARK),
            shade(h, c, LIGHT_BAND, false, Palette::LIGHT),
        ) else {
            return Accent::MINT;
        };
        Accent { dark, light }
    }
}

/// The most vivid lightness in the band that reads as text, the darkest in
/// dark mode and the lightest in light, or failing that the one that clears
/// 3:1 as a fill.
fn shade(hue: f64, chroma: f64, band: (f64, f64), dark: bool, tokens: Palette) -> Option<Shade> {
    let c = chroma.clamp(CHROMA.0, CHROMA.1);
    let steps: Vec<f64> = (0..=50)
        .map(|i| band.0 + (band.1 - band.0) * i as f64 / 50.0)
        .collect();
    let order: Vec<f64> = if dark {
        steps.clone()
    } else {
        steps.iter().rev().copied().collect()
    };
    let channels = |hex: super::tokens::Hex| hex.channels().map(|v| v as f64 / 255.0);
    let bg = oklch::luminance(channels(tokens.bg));
    let surface = oklch::luminance(channels(tokens.surface));
    let contrast = |a: f64, b: f64| (a.max(b) + 0.05) / (a.min(b) + 0.05);
    for (minimum, text) in [(4.5, true), (3.0, false)] {
        let ls: Box<dyn Iterator<Item = &f64>> = if text {
            Box::new(order.iter())
        } else {
            Box::new(order.iter().rev())
        };
        for &l in ls {
            let rgb = oklch::to_srgb(l, c, hue);
            let y = oklch::luminance(rgb);
            if contrast(y, bg) >= minimum && contrast(y, surface) >= minimum {
                return Some(Shade {
                    rgb,
                    reads_as_text: text,
                    slot: None,
                });
            }
        }
    }
    None
}

/// The colour a record reads as. Not the average of the sleeve, which comes
/// out the same brown-grey for every busy cover as opposite hues cancel: a
/// circular mean of hue weighted by each sample's chroma, so the one strong
/// colour on a mostly black cover wins. Samples are gated on chroma rather
/// than brightness, since thin strokes on black come out of the downscale
/// dark but still coloured.
///
/// The sleeve is first reduced to 32 pixels square, as the Apple apps'
/// thumbnail is, with its colour premultiplied by its alpha: Swift draws the
/// thumbnail into a premultiplied context, so a transparent part of a sleeve
/// reads as black and falls under the black floor.
pub fn dominant(rgba: &[u8], width: u32, height: u32) -> Option<[f64; 3]> {
    const SIDE: u32 = 32;
    if rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let premultiplied;
    let (samples, _) = rgba.as_chunks::<4>();
    let pixels = if samples.iter().all(|p| p[3] == 255) {
        rgba
    } else {
        premultiplied = samples
            .iter()
            .flat_map(|p| {
                let a = p[3] as u32;
                let scale = |c: u8| ((c as u32 * a + 127) / 255) as u8;
                [scale(p[0]), scale(p[1]), scale(p[2]), p[3]]
            })
            .collect::<Vec<u8>>();
        &premultiplied[..]
    };
    let image = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(width, height, pixels)?;
    let small = image::imageops::resize(&image, SIDE, SIDE, image::imageops::FilterType::Triangle);

    let (mut x, mut y, mut saturation, mut total) = (0.0, 0.0, 0.0, 0.0);
    for pixel in small.pixels() {
        let [r, g, b, _] = pixel.0;
        let (hue, sat, value) = hsb(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
        // Near-grey, near-black and blown-out samples say nothing about what
        // colour the record is.
        if sat <= 0.15 || value <= 0.1 || value >= 0.98 {
            continue;
        }
        let weight = sat * value;
        let angle = hue * 2.0 * PI;
        x += angle.cos() * weight;
        y += angle.sin() * weight;
        saturation += sat * weight;
        total += weight;
    }
    if total <= 0.0 {
        return None;
    }
    let mean = (y / total).atan2(x / total) / (2.0 * PI);
    Some(from_hsb(
        if mean < 0.0 { mean + 1.0 } else { mean },
        (saturation / total).clamp(0.55, 0.85),
        0.80,
    ))
}

fn hsb(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let high = r.max(g).max(b);
    let low = r.min(g).min(b);
    let delta = high - low;
    if delta <= 0.0 {
        return (0.0, 0.0, high);
    }
    let mut hue = if high == r {
        ((g - b) / delta) % 6.0
    } else if high == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    hue /= 6.0;
    (
        if hue < 0.0 { hue + 1.0 } else { hue },
        if high == 0.0 { 0.0 } else { delta / high },
        high,
    )
}

fn from_hsb(hue: f64, saturation: f64, brightness: f64) -> [f64; 3] {
    let h = (hue * 6.0) % 6.0;
    let c = brightness * saturation;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = brightness - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// OKLab in polar form: what the accent is tone-mapped in, because its
/// lightness is perceptual and its hue holds still while lightness moves.
pub mod oklch {
    use std::f64::consts::PI;

    /// Lightness, chroma and hue in degrees, from gamma-encoded sRGB in 0..=1.
    pub fn from_srgb([r, g, b]: [f64; 3]) -> (f64, f64, f64) {
        let [l, a, bb] = lab([linear(r), linear(g), linear(b)]);
        let hue = (bb.atan2(a) * 180.0 / PI + 360.0) % 360.0;
        (l, (a * a + bb * bb).sqrt(), hue)
    }

    /// OKLab from linear sRGB.
    pub fn lab([r, g, b]: [f64; 3]) -> [f64; 3] {
        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
        [
            0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
            1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
            0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
        ]
    }

    /// Linear sRGB from OKLab.
    pub fn linear_from_lab([l, a, b]: [f64; 3]) -> [f64; 3] {
        let l3 = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
        let m3 = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
        let s3 = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
        [
            4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3,
            -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3,
            -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3,
        ]
    }

    /// sRGB, with chroma given up until the colour is inside the gamut.
    pub fn to_srgb(l: f64, c: f64, h: f64) -> [f64; 3] {
        let mut chroma = c;
        loop {
            let (a, b) = (
                chroma * (h * PI / 180.0).cos(),
                chroma * (h * PI / 180.0).sin(),
            );
            let lin = linear_from_lab([l, a, b]);
            let inside = lin.iter().all(|v| (-0.0001..=1.0001).contains(v));
            if inside || chroma <= 0.0 {
                return lin.map(gamma);
            }
            chroma -= 0.002;
        }
    }

    pub fn luminance([r, g, b]: [f64; 3]) -> f64 {
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    }

    pub fn linear(v: f64) -> f64 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }

    pub fn gamma(v: f64) -> f64 {
        let c = v.clamp(0.0, 1.0);
        if c <= 0.0031308 {
            12.92 * c
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        }
    }
}

#[cfg(test)]
fn mix_oklab(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    let la = oklch::lab(a.map(oklch::linear));
    let lb = oklch::lab(b.map(oklch::linear));
    let mixed = [0, 1, 2].map(|i| la[i] + (lb[i] - la[i]) * t);
    oklch::linear_from_lab(mixed).map(oklch::gamma)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    /// With more contrast asked for, every hue's accent reaches 7:1 on both
    /// grounds of the contrast palette, light and dark.
    #[test]
    fn a_held_accent_reaches_seven_to_one() {
        let channels = |hex: super::super::tokens::Hex| hex.channels().map(|v| v as f64 / 255.0);
        for hue in (0..360).step_by(15) {
            let record = oklch::to_srgb(0.6, 0.15, hue as f64);
            let accent = Accent::of(Some(record));
            for dark in [true, false] {
                let tokens = Palette::for_look(dark, true);
                let held = accent.shade(dark).held(7.0, dark, tokens);
                let y = oklch::luminance(held.rgb);
                for ground in [tokens.bg, tokens.surface] {
                    let g = oklch::luminance(channels(ground));
                    let ratio = (y.max(g) + 0.05) / (y.min(g) + 0.05);
                    assert!(ratio >= 7.0, "hue {hue}, dark {dark}: {ratio:.2}");
                }
                assert!(held.reads_as_text);
            }
        }
    }

    fn hex(rgb: [f64; 3]) -> u32 {
        let [r, g, b] = rgb.map(|v| (v * 255.0).round() as u32);
        (r << 16) | (g << 8) | b
    }

    fn solid(r: u8, g: u8, b: u8) -> Vec<u8> {
        [r, g, b, 255].repeat(64 * 64)
    }

    #[test]
    fn grey_and_black_sleeves_give_mint() {
        assert_eq!(
            Accent::of_sleeve(&solid(128, 128, 128), 64, 64),
            Accent::MINT
        );
        assert_eq!(Accent::of_sleeve(&solid(0, 0, 0), 64, 64), Accent::MINT);
        assert_eq!(Accent::of(None), Accent::MINT);
    }

    #[test]
    fn a_red_sleeve_stays_red_and_reads() {
        let accent = Accent::of_sleeve(&solid(200, 30, 40), 64, 64);
        let (_, _, hue) = oklch::from_srgb(accent.dark.rgb);
        assert!(!(40.0..340.0).contains(&hue), "hue {hue}");
        assert!(accent.dark.reads_as_text && accent.light.reads_as_text);
        let (l, _, _) = oklch::from_srgb(accent.dark.rgb);
        assert!((0.70..=0.85).contains(&(l + 1e-6)), "dark lightness {l}");
        let (l, _, _) = oklch::from_srgb(accent.light.rgb);
        assert!((0.45..=0.60 + 1e-6).contains(&l), "light lightness {l}");
    }

    #[test]
    fn dominant_matches_the_apple_formula() {
        // A saturated blue sleeve, under the blown-out ceiling: hue 240°,
        // saturation clamped to 0.85, value 0.8.
        let colour = dominant(&solid(0, 0, 200), 64, 64).unwrap();
        assert_eq!(hex(colour), 0x1F1FCC);
    }

    #[test]
    fn transparent_parts_of_a_sleeve_read_as_black() {
        // Transparent red over the left half, opaque blue on the right. Drawn
        // premultiplied, as Swift draws it, the red is black and only the
        // blue counts.
        let side = 64;
        let mut rgba = Vec::with_capacity(side * side * 4);
        for _ in 0..side {
            for x in 0..side {
                rgba.extend(if x < side / 2 {
                    [220, 20, 20, 0]
                } else {
                    [0, 0, 200, 255]
                });
            }
        }
        let colour = dominant(&rgba, side as u32, side as u32).unwrap();
        let (_, _, hue) = oklch::from_srgb(colour);
        let blue = oklch::from_srgb(dominant(&solid(0, 0, 200), 64, 64).unwrap()).2;
        assert!((hue - blue).abs() < 1.0, "hue {hue}, blue {blue}");
    }

    /// The GPU's mix of a slot's ends in OKLab, as the CPU reads the table,
    /// agrees with the accent's own OKLab mix: a record's accent turns
    /// through the hues between, never through grey.
    #[test]
    fn slots_mix_the_accent_in_oklab() {
        use gpui_kit::{ColorSlot, Track, TrackCurve, TrackRepeat};
        let mint = Accent::MINT.dark;
        let blue = Accent::of(Some([0.1, 0.2, 0.9])).dark;
        let slot = ColorSlot::claim(mint.rgba()).expect("a free slot");
        let length = 1.0e6;
        slot.to_oklab(
            blue.rgba(),
            Track {
                from: 0.0,
                to: 1.0,
                start: 0.0,
                length,
                curve: TrackCurve::Linear,
                repeat: TrackRepeat::Once,
            },
        );
        let half = std::time::Instant::now() + std::time::Duration::from_secs_f32(length / 2.0);
        let gpu = slot.at(half);
        let cpu = mix_oklab(mint.rgb, blue.rgb, 0.5);
        for (a, b) in [gpu.r, gpu.g, gpu.b].into_iter().zip(cpu) {
            assert!((a as f64 - b).abs() < 2e-3, "{gpu:?} against {cpu:?}");
        }
    }
}
