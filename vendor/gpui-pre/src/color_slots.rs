// KOAN: colours that move on the GPU (#1859).
//
// A colour slot is one entry of an app-wide table of moving colours: a
// colour at each end and a track from 0 to 1 between them. Any quad, shadow,
// underline or monochrome sprite may name a slot instead of a fixed colour,
// by an `Hsla` whose hue is 2 or more (`ColorSlot::hsla`): the stock shaders
// read the table at present time and mix the two ends by the track, so a
// colour fading moves on a present with no frame drawn. The alpha of that
// `Hsla` still scales the colour's, so element opacity composes with it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::Mutex;

use crate::{Hsla, Rgba, Track, TrackRepeat, present_now};

/// How many slots the table holds: as many as fit, with the header, in the
/// 16 KiB every renderer guarantees a uniform buffer.
pub const COLOR_SLOTS: usize = 255;
/// Floats per slot: the `from` colour, the `to` colour, then the track.
pub const COLOR_SLOT_FLOATS: usize = 16;
/// Floats before the slots in a frame's table: the time, then padding.
pub const COLOR_SLOTS_HEADER: usize = 4;

/// The hue at which slot hues start.
const SLOT_HUE: f32 = 2.0;

struct Table {
    data: Box<[[f32; COLOR_SLOT_FLOATS]; COLOR_SLOTS]>,
    used: Box<[bool; COLOR_SLOTS]>,
    /// When the slot's track stops moving, in table seconds; `f32::INFINITY`
    /// for one that repeats, and negative for one at rest.
    ends: Box<[f32; COLOR_SLOTS]>,
    origin: Instant,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);
/// Moves whenever a slot is written, so a renderer uploads the table only
/// when it changed and a window presents once more for the change.
static GENERATION: AtomicU64 = AtomicU64::new(1);

fn with_table<R>(f: impl FnOnce(&mut Table) -> R) -> R {
    let mut table = TABLE.lock();
    let table = table.get_or_insert_with(|| Table {
        data: Box::new([[0.0; COLOR_SLOT_FLOATS]; COLOR_SLOTS]),
        used: Box::new([false; COLOR_SLOTS]),
        ends: Box::new([-1.0; COLOR_SLOTS]),
        origin: Instant::now(),
    });
    f(table)
}

fn seconds(table: &Table, at: Instant) -> f32 {
    match at.checked_duration_since(table.origin) {
        Some(after) => after.as_secs_f32(),
        None => -table.origin.duration_since(at).as_secs_f32(),
    }
}

/// One slot of the table, held while something draws with it, and freed
/// when dropped.
#[derive(Debug)]
pub struct ColorSlot(u16);

impl ColorSlot {
    /// A free slot standing at `color`, or `None` when every slot is taken.
    pub fn claim(color: Rgba) -> Option<Self> {
        let index = with_table(|table| {
            let Some(index) = table.used.iter().position(|used| !used) else {
                log::error!("every colour slot is taken: a colour drawn now does not move");
                return None;
            };
            table.used[index] = true;
            table.ends[index] = -1.0;
            let rgba = [color.r, color.g, color.b, color.a];
            let track = Track::hold(1.0).encode();
            let slot = &mut table.data[index];
            slot[0..4].copy_from_slice(&rgba);
            slot[4..8].copy_from_slice(&rgba);
            slot[8..16].copy_from_slice(&track);
            Some(index)
        })?;
        GENERATION.fetch_add(1, Ordering::Relaxed);
        Some(Self(index as u16))
    }

    /// The colour to draw with: this slot, its alpha scaled by `alpha`.
    pub fn hsla(&self, alpha: f32) -> Hsla {
        Hsla {
            h: SLOT_HUE + self.0 as f32,
            s: 0.0,
            l: 0.0,
            a: alpha,
        }
    }

    /// The colour the slot shows at `at`.
    pub fn at(&self, at: Instant) -> Rgba {
        with_table(|table| shown(table, self.0 as usize, at))
    }

    /// Moves from the colour shown now to `to`, along `track`'s curve over
    /// its length, starting now; `track`'s ends and start are replaced.
    pub fn to(&self, to: Rgba, track: Track) {
        let now = present_now();
        let from = self.at(now);
        with_table(|table| {
            rebase(table, now);
            let start = seconds(table, now);
            let track = Track {
                from: 0.0,
                to: 1.0,
                start,
                ..track
            };
            let index = self.0 as usize;
            table.ends[index] = match track.repeat {
                _ if track.length <= 0.0 => -1.0,
                TrackRepeat::Once => start + track.length,
                _ => f32::INFINITY,
            };
            let slot = &mut table.data[index];
            slot[0..4].copy_from_slice(&[from.r, from.g, from.b, from.a]);
            slot[4..8].copy_from_slice(&[to.r, to.g, to.b, to.a]);
            slot[8..16].copy_from_slice(&track.encode());
        });
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }

    /// Moves from the colour shown now to `to` as `to` does, mixed in OKLab:
    /// the hue turns rather than passing through grey, as the accent does
    /// when a record arrives.
    pub fn to_oklab(&self, to: Rgba, track: Track) {
        let now = present_now();
        let from = self.at(now);
        with_table(|table| {
            rebase(table, now);
            let start = seconds(table, now);
            let track = Track {
                from: 0.0,
                to: 1.0,
                start,
                ..track
            };
            let index = self.0 as usize;
            table.ends[index] = match track.repeat {
                _ if track.length <= 0.0 => -1.0,
                TrackRepeat::Once => start + track.length,
                _ => f32::INFINITY,
            };
            let [fl, fa, fb] = srgb_to_oklab([from.r, from.g, from.b]);
            let [tl, ta, tb] = srgb_to_oklab([to.r, to.g, to.b]);
            let mut encoded = track.encode();
            encoded[4] += 16.0;
            let slot = &mut table.data[index];
            slot[0..4].copy_from_slice(&[fl, fa, fb, from.a]);
            slot[4..8].copy_from_slice(&[tl, ta, tb, to.a]);
            slot[8..16].copy_from_slice(&encoded);
        });
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }

    /// Stands at `color`.
    pub fn hold(&self, color: Rgba) {
        self.to(color, Track::hold(1.0));
    }

    /// Whether the slot moves at `at`.
    pub fn moving(&self, at: Instant) -> bool {
        with_table(|table| table.ends[self.0 as usize] > seconds(table, at))
    }
}

impl Drop for ColorSlot {
    fn drop(&mut self) {
        with_table(|table| {
            table.used[self.0 as usize] = false;
            table.ends[self.0 as usize] = -1.0;
        });
    }
}

fn shown(table: &Table, index: usize, at: Instant) -> Rgba {
    let slot = &table.data[index];
    let t = Track::decode(&slot[8..16]).at(seconds(table, at));
    let mix = |k: usize| slot[k] + (slot[k + 4] - slot[k]) * t;
    let mixed = [mix(0), mix(1), mix(2)];
    // KOAN: ends held as OKLab, flagged by 16 in the track's code.
    let [r, g, b] = if slot[12] as u32 >= 16 {
        oklab_to_srgb(mixed)
    } else {
        mixed
    };
    Rgba { r, g, b, a: mix(3) }
}

/// Track starts are seconds from the table's origin, in f32: an hour on, the
/// origin moves to now and every slot with it, so the table keeps its
/// precision for a koan open for days.
fn rebase(table: &mut Table, now: Instant) {
    let shift = seconds(table, now);
    if shift < 3600.0 {
        return;
    }
    table.origin = now;
    for (slot, end) in table.data.iter_mut().zip(table.ends.iter_mut()) {
        slot[10] -= shift;
        if end.is_finite() && *end >= 0.0 {
            *end -= shift;
        }
    }
}

/// The slot a colour names, if it names one.
pub fn color_slot_of(color: &Hsla) -> Option<usize> {
    (color.h >= SLOT_HUE).then(|| ((color.h - SLOT_HUE) as usize).min(COLOR_SLOTS - 1))
}

/// The colour slot `index` shows now, its alpha scaled by `alpha`: what a
/// colour naming a slot is wherever the CPU reads it.
pub fn color_slot_rgba(index: usize, alpha: f32) -> Rgba {
    let mut rgba = with_table(|table| shown(table, index, present_now()));
    rgba.a *= alpha;
    rgba
}

/// Bytes of a frame's table.
pub const COLOR_SLOTS_BYTES: usize = (COLOR_SLOTS_HEADER + COLOR_SLOTS * COLOR_SLOT_FLOATS) * 4;

/// The slots a scene draws with, as a bit set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColorSlotSet(Box<[u64; COLOR_SLOTS.div_ceil(64)]>);

impl ColorSlotSet {
    /// Notes the slot `color` names, if any.
    pub fn note(&mut self, color: &Hsla) {
        if let Some(index) = color_slot_of(color) {
            self.0[index / 64] |= 1 << (index % 64);
        }
    }

    /// Whether the set names no slot.
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|word| *word == 0)
    }

    /// Names no slot.
    pub fn clear(&mut self) {
        *self.0 = [0; COLOR_SLOTS.div_ceil(64)];
    }

    /// Adds every slot `other` names.
    pub fn extend(&mut self, other: &Self) {
        for (a, b) in self.0.iter_mut().zip(other.0.iter()) {
            *a |= *b;
        }
    }

    /// Whether any slot in the set moves at `at`.
    pub fn moving(&self, at: Instant) -> bool {
        if self.is_empty() {
            return false;
        }
        with_table(|table| {
            let now = seconds(table, at);
            self.0.iter().enumerate().any(|(w, word)| {
                let mut bits = *word;
                while bits != 0 {
                    let index = w * 64 + bits.trailing_zeros() as usize;
                    if table.ends[index] > now {
                        return true;
                    }
                    bits &= bits - 1;
                }
                false
            })
        })
    }
}

/// The table as the stock shaders read it at `at`: the time, padding, then
/// every slot. Written into `out`, which is cleared first.
pub fn color_slots_frame(at: Instant, out: &mut Vec<f32>) {
    out.clear();
    with_table(|table| {
        out.push(seconds(table, at));
        out.extend([0.0; COLOR_SLOTS_HEADER - 1]);
        for slot in table.data.iter() {
            out.extend_from_slice(slot);
        }
    });
}

/// Moves whenever any slot is written.
pub fn color_slots_generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

// KOAN: sRGB and OKLab, for slots mixed in OKLab (#1859): the same
// matrices as the stock shaders' `koan_oklab_to_srgb`.
fn srgb_to_oklab(c: [f32; 3]) -> [f32; 3] {
    let linear = c.map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    let [r, g, b] = linear;
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720426 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

fn oklab_to_srgb([l0, a, b]: [f32; 3]) -> [f32; 3] {
    let l = (l0 + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (l0 - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (l0 - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    let linear = [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ];
    linear.map(|v| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.0031308 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    })
}
