//! Colour slots: colours that move on the GPU. A slot is a dynamic colour
//! (gpui's `ColorHandle`) whose value the kit works out at each present from
//! two ends and a track, mixed in sRGB or OKLab, so a colour fading or the
//! accent turning moves with no frame drawn. The kit's colour source
//! (`install`) evaluates every claimed slot at the present's time and says
//! until when they move.
//!
//! It also keeps the kit's present clock: the latest time a present was
//! made for, which a retarget samples a moving value at.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use gpui::{ColorHandle, DYNAMIC_COLORS, Hsla, Rgba};
use parking_lot::Mutex;

use super::Track;

#[derive(Clone, Copy)]
struct Entry {
    from: [f32; 4],
    to: [f32; 4],
    /// From 0 to 1, in seconds from the table's origin.
    track: Track,
    /// Whether the ends are held in OKLab.
    oklab: bool,
}

struct Table {
    entries: [Option<Entry>; DYNAMIC_COLORS],
    origin: Instant,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

fn with_table<R>(f: impl FnOnce(&mut Table) -> R) -> R {
    let mut table = TABLE.lock();
    let table = table.get_or_insert_with(|| Table {
        entries: [None; DYNAMIC_COLORS],
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

/// The latest time a present was made for, in nanoseconds from `epoch`.
static PRESENTED: AtomicU64 = AtomicU64::new(0);

fn epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// When a frame made now reaches the screen, as the last present said:
/// what a retarget samples a moving value at. Never before now.
pub fn present_now() -> Instant {
    let presented = epoch() + std::time::Duration::from_nanos(PRESENTED.load(Ordering::Relaxed));
    presented.max(Instant::now())
}

/// Installs the kit's colour source: every claimed slot evaluated at the
/// present's time. Called once, by `theme::init`.
pub fn install() {
    gpui::set_color_source(|at, colors| {
        let nanos = at.saturating_duration_since(epoch()).as_nanos() as u64;
        PRESENTED.fetch_max(nanos, Ordering::Relaxed);
        with_table(|table| {
            let now = seconds(table, at);
            let mut until: Option<f32> = None;
            for (index, entry) in table.entries.iter().enumerate() {
                let Some(entry) = entry else {
                    continue;
                };
                colors[index] = shown(entry, now);
                if let Some(end) = moving_until(entry, now) {
                    until = Some(until.map_or(end, |u| u.max(end)));
                }
            }
            until.map(|end| table.origin + std::time::Duration::from_secs_f32(end.max(0.0)))
        })
    });
}

/// Until when an entry moves, in table seconds, if it moves at `now`; a
/// repeat moves an hour on at a time.
fn moving_until(entry: &Entry, now: f32) -> Option<f32> {
    if entry.track.length <= 0.0 {
        return None;
    }
    match entry.track.end() {
        Some(end) => (end > now).then_some(end),
        None => Some(now + 3600.0),
    }
}

fn shown(entry: &Entry, now: f32) -> Rgba {
    let t = entry.track.at(now);
    let mix = |k: usize| entry.from[k] + (entry.to[k] - entry.from[k]) * t;
    let mixed = [mix(0), mix(1), mix(2)];
    let [r, g, b] = if entry.oklab {
        oklab_to_srgb(mixed)
    } else {
        mixed
    };
    // As a curve that overshoots mixes past the ends, never past a colour.
    Rgba {
        r: r.clamp(0.0, 1.0),
        g: g.clamp(0.0, 1.0),
        b: b.clamp(0.0, 1.0),
        a: mix(3).clamp(0.0, 1.0),
    }
}

/// Track starts are seconds from the table's origin, in f32: an hour on,
/// the origin moves to now and every entry with it.
fn rebase(table: &mut Table, now: Instant) {
    let shift = seconds(table, now);
    if shift < 3600.0 {
        return;
    }
    table.origin = now;
    for entry in table.entries.iter_mut().flatten() {
        entry.track.start -= shift;
    }
}

/// One colour slot, held while something draws with it and freed when
/// dropped.
#[derive(Debug)]
pub struct ColorSlot(ColorHandle);

impl ColorSlot {
    /// A free slot standing at `color`, or `None` when every slot is taken.
    pub fn claim(color: Rgba) -> Option<Self> {
        let Some(handle) = ColorHandle::claim(color) else {
            log::error!("every colour slot is taken: a colour drawn now does not move");
            return None;
        };
        let rgba = [color.r, color.g, color.b, color.a];
        with_table(|table| {
            table.entries[handle.index()] = Some(Entry {
                from: rgba,
                to: rgba,
                track: Track::hold(1.0),
                oklab: false,
            });
        });
        Some(Self(handle))
    }

    /// The colour to draw with: this slot, its alpha scaled by `alpha`.
    pub fn hsla(&self, alpha: f32) -> Hsla {
        self.0.hsla(alpha)
    }

    /// The colour the slot shows at `at`.
    pub fn at(&self, at: Instant) -> Rgba {
        with_table(|table| {
            let now = seconds(table, at);
            table.entries[self.0.index()]
                .as_ref()
                .map_or(Rgba::default(), |entry| shown(entry, now))
        })
    }

    fn write(&self, to: Rgba, track: Track, oklab: bool) {
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
            let ends = |c: Rgba| {
                if oklab {
                    let [l, a, b] = srgb_to_oklab([c.r, c.g, c.b]);
                    [l, a, b, c.a]
                } else {
                    [c.r, c.g, c.b, c.a]
                }
            };
            table.entries[self.0.index()] = Some(Entry {
                from: ends(from),
                to: ends(to),
                track,
                oklab,
            });
        });
    }

    /// Moves from the colour shown now to `to`, along `track`'s curve over
    /// its length, starting now; `track`'s ends and start are replaced.
    pub fn to(&self, to: Rgba, track: Track) {
        self.write(to, track, false);
    }

    /// Moves from the colour shown now to `to` as `to` does, mixed in OKLab:
    /// the hue turns rather than passing through grey.
    pub fn to_oklab(&self, to: Rgba, track: Track) {
        self.write(to, track, true);
    }

    /// Stands at `color`.
    pub fn hold(&self, color: Rgba) {
        self.write(color, Track::hold(1.0), false);
    }

    /// Whether the slot moves at `at`.
    pub fn moving(&self, at: Instant) -> bool {
        with_table(|table| {
            let now = seconds(table, at);
            table.entries[self.0.index()]
                .as_ref()
                .is_some_and(|entry| moving_until(entry, now).is_some())
        })
    }
}

impl Drop for ColorSlot {
    fn drop(&mut self) {
        with_table(|table| table.entries[self.0.index()] = None);
    }
}

// sRGB and OKLab, for slots mixed in OKLab.
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
