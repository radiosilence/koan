// KOAN: this module is koan's addition (#1641): a quad drawn by a fragment
// program the app supplies, with a small block of uniforms the app may write
// from any thread. The uniforms are read when a frame is presented, not when
// it is drawn, and while any shader on screen is live the window presents its
// last frame again each refresh without drawing a new one. Something that
// changes every frame then costs the GPU a quad and the CPU an encode, with no
// layout, prepaint or paint.

#![allow(missing_docs)]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use crate::{AtlasTile, Bounds, ContentMask, DrawOrder, Hsla, Point, Primitive, ScaledPixels};

/// How many plain floats a shader's uniform block holds, written by the app
/// when they change: colours, sizes, flags.
pub const SHADER_PLAIN: usize = 16;
/// How many tracks follow them: values the GPU moves itself.
pub const SHADER_TRACKS: usize = 8;
/// The floats one track takes: `from, to, start, length` and
/// `kind + 4 * repeat, a, b, c`.
pub const TRACK_FLOATS: usize = 8;
/// How many floats a shader's uniform block holds: the plain values, then
/// the tracks (#1859).
pub const SHADER_UNIFORMS: usize = SHADER_PLAIN + SHADER_TRACKS * TRACK_FLOATS;

// KOAN: tracks (#1859). A track is one segment of a moving value, evaluated
// by the program at present time through the prelude's `koan_track`, so the
// CPU writes it when the value's target changes and never per frame. The
// same evaluation is here, for the app to read where a track stands (a
// retarget starts from there) and for the renderer's liveness.

/// How a track moves between its ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrackCurve {
    Linear,
    /// A CSS cubic Bézier timing function, `cubic-bezier(x1, y1, x2, y2)`.
    Bezier(f32, f32, f32, f32),
    /// A damped spring of unit mass, set off with `velocity` (units of the
    /// value per second). Its `length` is how long it takes to settle.
    Spring {
        stiffness: f32,
        damping: f32,
        velocity: f32,
    },
}

/// What a track does at its end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrackRepeat {
    /// Holds `to`.
    Once,
    /// Starts again from `from`.
    Loop,
    /// Goes back to `from`, and on again: a breath.
    Alternate,
}

/// One segment of a value the GPU moves: from `from` to `to` over `length`
/// seconds from `start`, seconds of the shader's time, along `curve`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Track {
    pub from: f32,
    pub to: f32,
    pub start: f32,
    pub length: f32,
    pub curve: TrackCurve,
    pub repeat: TrackRepeat,
}

impl Track {
    /// Standing at `value`.
    pub fn hold(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start: 0.0,
            length: 0.0,
            curve: TrackCurve::Linear,
            repeat: TrackRepeat::Once,
        }
    }

    /// Where it stands at `time`, seconds of the shader's time: what the
    /// prelude's `koan_track` draws.
    pub fn at(&self, time: f32) -> f32 {
        if self.length <= 0.0 {
            return self.to;
        }
        let elapsed = (time - self.start).max(0.0);
        if let TrackCurve::Spring {
            stiffness,
            damping,
            velocity,
        } = self.curve
        {
            if elapsed >= self.length {
                return self.to;
            }
            return self.to + spring(self.from - self.to, velocity, stiffness, damping, elapsed);
        }
        let mut phase = elapsed / self.length;
        match self.repeat {
            TrackRepeat::Once => phase = phase.min(1.0),
            TrackRepeat::Loop => phase = phase.fract(),
            TrackRepeat::Alternate => {
                let p = phase.rem_euclid(2.0);
                phase = if p > 1.0 { 2.0 - p } else { p };
            }
        }
        let eased = match self.curve {
            TrackCurve::Linear => phase,
            TrackCurve::Bezier(x1, y1, x2, y2) => bezier(x1, y1, x2, y2, phase),
            TrackCurve::Spring { .. } => phase,
        };
        self.from + (self.to - self.from) * eased
    }

    /// When it stops moving, in seconds of the shader's time; `None` for one
    /// that repeats.
    pub fn end(&self) -> Option<f32> {
        match self.repeat {
            TrackRepeat::Once => Some(self.start + self.length.max(0.0)),
            _ if self.length <= 0.0 => Some(self.start),
            _ => None,
        }
    }

    pub(crate) fn encode(&self) -> [f32; TRACK_FLOATS] {
        let repeat = match self.repeat {
            TrackRepeat::Once => 0.0,
            TrackRepeat::Loop => 1.0,
            TrackRepeat::Alternate => 2.0,
        };
        let (kind, a, b, c) = match self.curve {
            TrackCurve::Linear => (0.0, 0.0, 0.0, 0.0),
            TrackCurve::Bezier(x1, y1, x2, y2) => {
                (1.0, pack_halves(x1, y1), pack_halves(x2, y2), 0.0)
            }
            TrackCurve::Spring {
                stiffness,
                damping,
                velocity,
            } => (2.0, stiffness, damping, velocity),
        };
        [
            self.from,
            self.to,
            self.start,
            self.length,
            kind + 4.0 * repeat,
            a,
            b,
            c,
        ]
    }

    pub(crate) fn decode(f: &[f32]) -> Self {
        let code = f[4] as u32;
        let repeat = match code / 4 {
            1 => TrackRepeat::Loop,
            2 => TrackRepeat::Alternate,
            _ => TrackRepeat::Once,
        };
        let curve = match code % 4 {
            1 => {
                let (x1, y1) = unpack_halves(f[5]);
                let (x2, y2) = unpack_halves(f[6]);
                TrackCurve::Bezier(x1, y1, x2, y2)
            }
            2 => TrackCurve::Spring {
                stiffness: f[5],
                damping: f[6],
                velocity: f[7],
            },
            _ => TrackCurve::Linear,
        };
        Self {
            from: f[0],
            to: f[1],
            start: f[2],
            length: f[3],
            curve,
            repeat,
        }
    }
}

/// How long a spring takes to come within a thousandth of the distance it
/// set off from (or of a ten-thousandth of a unit, for one set off from its
/// end): the first moment a bound on how far it can still be falls under
/// that, for each regime's own motion.
pub fn spring_length(distance: f32, velocity: f32, stiffness: f32, damping: f32) -> f32 {
    let (x0, v0) = (distance as f64, velocity as f64);
    let omega = (stiffness.max(1e-3) as f64).sqrt();
    let zeta = damping as f64 / (2.0 * omega);
    let epsilon = (x0.abs() * 1e-3).max(1e-4);
    // How far from its end it can be at `t`, at most.
    let bound = |t: f64| -> f64 {
        if zeta < 1.0 {
            let wd = omega * (1.0 - zeta * zeta).sqrt();
            let reach = (x0 * x0 + ((v0 + zeta * omega * x0) / wd).powi(2)).sqrt();
            reach * (-zeta * omega * t).exp()
        } else if zeta > 1.0 + 1e-4 {
            let root = (zeta * zeta - 1.0).sqrt();
            let (r1, r2) = (-omega * (zeta - root), -omega * (zeta + root));
            let c1 = (v0 - r2 * x0) / (r1 - r2);
            c1.abs() * (r1 * t).exp() + (x0 - c1).abs() * (r2 * t).exp()
        } else {
            (x0.abs() + (v0 + omega * x0).abs() * t) * (-omega * t).exp()
        }
    };
    let step = 1.0 / 480.0;
    let mut t = 0.0;
    while t < 10.0 && bound(t) > epsilon {
        t += step;
    }
    t as f32
}

/// The displacement of a unit-mass spring `t` seconds after it set off from
/// `x0` with velocity `v0`.
fn spring(x0: f32, v0: f32, stiffness: f32, damping: f32, t: f32) -> f32 {
    let omega = stiffness.max(1e-3).sqrt();
    let zeta = damping / (2.0 * omega);
    if zeta < 1.0 {
        let wd = omega * (1.0 - zeta * zeta).sqrt();
        let envelope = (-zeta * omega * t).exp();
        envelope * (x0 * (wd * t).cos() + (v0 + zeta * omega * x0) / wd * (wd * t).sin())
    } else if zeta > 1.0 + 1e-4 {
        let root = (zeta * zeta - 1.0).sqrt();
        let (r1, r2) = (-omega * (zeta - root), -omega * (zeta + root));
        let c1 = (v0 - r2 * x0) / (r1 - r2);
        c1 * (r1 * t).exp() + (x0 - c1) * (r2 * t).exp()
    } else {
        (x0 + (v0 + omega * x0) * t) * (-omega * t).exp()
    }
}

/// A CSS cubic Bézier timing function: the curve's y where its x is `t`, by
/// Newton's method from `t`, as the prelude's `koan_bezier` finds it.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    if t <= 0.0 || t >= 1.0 {
        return t.clamp(0.0, 1.0);
    }
    let curve = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let slope = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = t;
    for _ in 0..6 {
        let d = slope(x1, x2, s);
        if d.abs() < 1e-5 {
            break;
        }
        s = (s - (curve(x1, x2, s) - t) / d).clamp(0.0, 1.0);
    }
    curve(y1, y2, s)
}

/// Two numbers as half floats in one float's bits: a Bézier's control point.
fn pack_halves(a: f32, b: f32) -> f32 {
    f32::from_bits(to_half(a) as u32 | (to_half(b) as u32) << 16)
}

fn unpack_halves(v: f32) -> (f32, f32) {
    let bits = v.to_bits();
    (from_half(bits as u16), from_half((bits >> 16) as u16))
}

fn to_half(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = bits & 0x7f_ffff;
    if exponent <= 0 {
        return sign;
    }
    if exponent >= 31 {
        return sign | 0x7c00;
    }
    // Rounded to the nearest half.
    // Added, not ORed: a mantissa that rounds up past its top carries into
    // the exponent.
    let rounded = ((exponent as u32) << 10) + ((mantissa + 0x1000) >> 13);
    sign | rounded.min(0x7bff) as u16
}

fn from_half(h: u16) -> f32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let exponent = ((h >> 10) & 0x1f) as u32;
    let mantissa = (h & 0x3ff) as u32;
    if exponent == 0 {
        return f32::from_bits(sign)
            + (mantissa as f32) * 2f32.powi(-24) * if sign != 0 { -1.0 } else { 1.0 };
    }
    f32::from_bits(sign | (exponent + 127 - 15) << 23 | mantissa << 13)
}

/// A fragment program, written once for each renderer.
///
/// Each source defines one function, `shade`, which returns the colour of a
/// pixel of the quad, not premultiplied:
///
/// - Metal: `float4 shade(float2 position, float2 size, float time, constant float *uniforms, texture2d<float> image, float4 tile)`,
///   where `koan_sample(image, tile, uv)` reads the shader's image
///   (`Window::paint_shader_image`) at `uv`, 0 to 1 across it, filtered and
///   kept to its edge
/// - WGSL: `fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, uniforms: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32>`,
///   where `koan_sample(tile, uv)` reads the image the same way
///
/// `position` is the pixel's centre within the quad and `size` the quad's
/// size, both in device pixels; `time` is seconds since the uniforms' origin
/// ([`ShaderUniforms::set_origin`], or [`shader_epoch`] if none was set),
/// reckoned in double precision as the frame is presented, so it stays exact
/// however long the app runs.
/// Programs are compiled the first time they are drawn and kept, by `name`.
#[derive(Debug)]
pub struct ShaderProgram {
    /// Unique among the app's programs: what the compiled program is kept by.
    pub name: &'static str,
    pub msl: &'static str,
    pub wgsl: &'static str,
    /// Run on the CPU once per quad as each frame is presented, given its
    /// uniforms and `time`, to write values every pixel would otherwise
    /// work out alike (a pose, an eased opacity) into the uniforms the
    /// program reads. The app's copy is left as it was.
    pub prepare: Option<fn(&mut [f32; SHADER_UNIFORMS], f32)>,
}

/// The instant shader time counts from when the uniforms set no origin.
pub fn shader_epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

// KOAN: the present clock (#1859).
static PRESENT_LEAD: AtomicI64 = AtomicI64::new(0);

/// The moment the frame being made now will reach the screen: what a
/// shader's `time` is reckoned at, and what a retarget samples a track at,
/// so motion neither jitters with encode latency nor jumps when retargeted.
///
/// Monotonic: displays at different refresh rates report different leads,
/// and a frame's time never steps back when one link's report follows
/// another's.
pub fn present_now() -> Instant {
    static LATEST: AtomicI64 = AtomicI64::new(i64::MIN);
    let lead = PRESENT_LEAD.load(Ordering::Relaxed).max(0) as u64;
    let at = Instant::now() + std::time::Duration::from_nanos(lead);
    let nanos = since_epoch(at);
    let latest = LATEST.fetch_max(nanos, Ordering::Relaxed).max(nanos);
    shader_epoch() + std::time::Duration::from_nanos(latest.max(0) as u64)
}

/// How far ahead of now a frame encoded now is shown, as the platform's
/// display link reports it.
pub fn set_present_lead(lead: std::time::Duration) {
    PRESENT_LEAD.store(
        lead.as_nanos().min(i64::MAX as u128) as i64,
        Ordering::Relaxed,
    );
}

/// Signed nanoseconds from [`shader_epoch`] to `at`.
fn since_epoch(at: Instant) -> i64 {
    let epoch = shader_epoch();
    match at.checked_duration_since(epoch) {
        Some(after) => after.as_nanos() as i64,
        None => -(epoch.duration_since(at).as_nanos() as i64),
    }
}

/// A shader's uniforms, shared between the app and the renderer: written by
/// the app from any thread, read as each frame is presented.
///
/// A refresh presents the last frame again, with no new frame drawn, where
/// a shader on screen changed since the last present, or on every refresh
/// for a `live` and `continuous` shader, which moves with `time` alone.
/// While any shader on screen is `live` the window keeps its refreshes
/// coming; otherwise it parks, and a change is seen at the next refresh,
/// which `Window::request_present` asks for.
#[derive(Debug)]
pub struct ShaderUniforms {
    values: [AtomicU32; SHADER_UNIFORMS],
    live: AtomicBool,
    continuous: AtomicBool,
    changed: AtomicBool,
    /// Nanoseconds from [`shader_epoch`] to the origin `time` counts from,
    /// negative for an origin before it: an anchor can be older than the
    /// first shader.
    origin: AtomicI64,
    /// Nanoseconds from [`shader_epoch`] after which `live` lapses by
    /// itself; 0 for never. A transition is live for its length and no
    /// longer, with nothing to stop it.
    until: AtomicU64,
    /// A lapse not yet presented: the present that lands a transition at
    /// its end, however early the last refresh before it fell.
    landing: AtomicBool,
    // KOAN: #1859.
    /// Nanoseconds from [`shader_epoch`] until which a track moves; 0 for
    /// none, `u64::MAX` for one that repeats.
    moving_until: AtomicU64,
    /// The present that lands the last track at its end.
    track_landing: AtomicBool,
    /// Per plain uniform, the colour slot the four floats from it draw, plus
    /// one; 0 for a plain colour. Resolved as each present is made, so a
    /// slot's colour moves in the program with no frame drawn.
    colors: [AtomicU16; SHADER_PLAIN],
    /// Whether a slot the uniforms name was moving at the last present: the
    /// present after it stops lands it.
    colors_moved: AtomicBool,
}

// KOAN: written out, since an array of eighty has no derived default
// (#1859).
impl Default for ShaderUniforms {
    fn default() -> Self {
        Self {
            values: std::array::from_fn(|_| AtomicU32::new(0)),
            live: AtomicBool::new(false),
            continuous: AtomicBool::new(false),
            changed: AtomicBool::new(false),
            origin: AtomicI64::new(0),
            until: AtomicU64::new(0),
            landing: AtomicBool::new(false),
            moving_until: AtomicU64::new(0),
            track_landing: AtomicBool::new(false),
            colors: std::array::from_fn(|_| AtomicU16::new(0)),
            colors_moved: AtomicBool::new(false),
        }
    }
}

impl ShaderUniforms {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the value changed. A colour slot whose four floats `index`
    /// falls in is let go: the value written stands as written.
    pub fn set(&self, index: usize, value: f32) -> bool {
        // KOAN: #1859.
        let mut moved = false;
        for colour in index.saturating_sub(3)..=index.min(SHADER_PLAIN - 1) {
            moved |= self.colors[colour].swap(0, Ordering::Relaxed) != 0;
        }
        if moved {
            self.changed.store(true, Ordering::Relaxed);
        }
        self.write(index, value) || moved
    }

    fn write(&self, index: usize, value: f32) -> bool {
        let bits = value.to_bits();
        let changed = self.values[index].swap(bits, Ordering::Relaxed) != bits;
        if changed {
            self.changed.store(true, Ordering::Relaxed);
        }
        changed
    }

    pub fn get(&self, index: usize) -> f32 {
        f32::from_bits(self.values[index].load(Ordering::Relaxed))
    }

    /// The moment `time` counts from in the program: where an anchor was
    /// taken, so the program reckons from it.
    pub fn set_origin(&self, origin: Instant) -> bool {
        let nanos = since_epoch(origin);
        let changed = self.origin.swap(nanos, Ordering::Relaxed) != nanos;
        if changed {
            self.changed.store(true, Ordering::Relaxed);
        }
        changed
    }

    /// Seconds from the origin to `now`, as the program reads `time`.
    pub fn time_at(&self, now: Instant) -> f32 {
        ((since_epoch(now) - self.origin.load(Ordering::Relaxed)) as f64 / 1e9) as f32
    }

    pub fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Relaxed);
    }

    pub fn is_live(&self) -> bool {
        (self.live.load(Ordering::Relaxed)
            && (!self.lapsed() || self.landing.load(Ordering::Relaxed)))
            // KOAN: a moving track keeps the window presenting (#1859).
            || self.tracks_moving(present_now())
            || self.track_landing.load(Ordering::Relaxed)
            || self.colors_moving(present_now())
            || self.colors_moved.load(Ordering::Relaxed)
    }

    /// Whether a deadline was set and has passed.
    fn lapsed(&self) -> bool {
        let until = self.until.load(Ordering::Relaxed);
        until != 0
            && (Instant::now()
                .saturating_duration_since(shader_epoch())
                .as_nanos() as u64)
                >= until
    }

    /// Live until `deadline` and no longer; `None` for as long as `live` is
    /// set.
    pub fn set_live_until(&self, deadline: Option<Instant>) {
        let nanos = deadline.map_or(0, |d| {
            (d.saturating_duration_since(shader_epoch()).as_nanos() as u64).max(1)
        });
        self.until.store(nanos, Ordering::Relaxed);
        self.landing.store(deadline.is_some(), Ordering::Relaxed);
    }

    /// Presented on every refresh while live, for a shader moved by `time`.
    pub fn set_continuous(&self, continuous: bool) {
        self.continuous.store(continuous, Ordering::Relaxed);
    }

    /// Whether this refresh should present: changed since the last time
    /// this was asked, or live and continuous.
    pub fn take_present(&self) -> bool {
        let changed = self.changed.swap(false, Ordering::Relaxed);
        let landed = self.lapsed() && self.landing.swap(false, Ordering::Relaxed);
        // KOAN: a track moves with time; the present after its end lands it
        // (#1859).
        let now = present_now();
        let moving = self.tracks_moving(now);
        let track_landed = !moving && self.track_landing.swap(false, Ordering::Relaxed);
        let colors = self.colors_moving(now);
        let colors_landed = self.colors_moved.swap(colors, Ordering::Relaxed) && !colors;
        changed
            || landed
            || moving
            || track_landed
            || colors
            || colors_landed
            || (self.is_live() && self.continuous.load(Ordering::Relaxed))
    }

    /// Every value, as a renderer copies them into a frame.
    pub fn snapshot(&self) -> [f32; SHADER_UNIFORMS] {
        std::array::from_fn(|i| self.get(i))
    }

    // KOAN: #1859.
    /// Puts `track` in slot `k`: the program draws it from the next present
    /// on, and the window presents while it moves.
    pub fn set_track(&self, k: usize, track: Track) {
        let base = SHADER_PLAIN + k * TRACK_FLOATS;
        for (i, value) in track.encode().into_iter().enumerate() {
            self.set(base + i, value);
        }
        let origin = self.origin.load(Ordering::Relaxed);
        let until = (0..SHADER_TRACKS)
            .map(|k| self.track(k))
            .filter(|t| t.length > 0.0)
            .map(|t| match t.end() {
                None => u64::MAX,
                Some(end) => (origin + (end as f64 * 1e9) as i64).max(1) as u64,
            })
            .max()
            .unwrap_or(0);
        self.moving_until.store(until, Ordering::Relaxed);
        self.track_landing.store(until != 0, Ordering::Relaxed);
    }

    /// The track in slot `k`.
    pub fn track(&self, k: usize) -> Track {
        let base = SHADER_PLAIN + k * TRACK_FLOATS;
        let floats: [f32; TRACK_FLOATS] = std::array::from_fn(|i| self.get(base + i));
        Track::decode(&floats)
    }

    /// The colour `color` into the four plain uniforms from `index`, as
    /// red, green, blue and alpha. A colour naming a slot is read from the
    /// slot as each present is made, its alpha scaling the slot's. Whether
    /// anything changed.
    pub fn set_color(&self, index: usize, color: Hsla) -> bool {
        let slot = crate::color_slot_of(&color).map_or(0, |slot| slot as u16 + 1);
        let rgba = if slot == 0 {
            crate::Rgba::from(color)
        } else {
            // The alpha, kept to scale the slot's by.
            crate::Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: color.a,
            }
        };
        // Any other colour these floats overlap stands no more.
        let mut changed = false;
        for other in index.saturating_sub(3)..(index + 4).min(SHADER_PLAIN) {
            if other != index {
                changed |= self.colors[other].swap(0, Ordering::Relaxed) != 0;
            }
        }
        for (i, value) in [rgba.r, rgba.g, rgba.b, rgba.a].into_iter().enumerate() {
            changed |= self.write(index + i, value);
        }
        changed |= self.colors[index].swap(slot, Ordering::Relaxed) != slot;
        if changed {
            self.changed.store(true, Ordering::Relaxed);
        }
        changed
    }

    /// Whether a colour slot the uniforms draw is moving at `at`.
    fn colors_moving(&self, at: Instant) -> bool {
        self.colors.iter().any(|slot| {
            let slot = slot.load(Ordering::Relaxed);
            slot != 0 && crate::color_slot_moving(slot as usize - 1, at)
        })
    }

    /// Every value as the program reads it at `now`: a colour naming a slot
    /// replaced by the colour the slot shows then.
    pub fn snapshot_at(&self, now: Instant) -> [f32; SHADER_UNIFORMS] {
        let mut values = self.snapshot();
        for (index, slot) in self.colors.iter().enumerate() {
            let slot = slot.load(Ordering::Relaxed);
            if slot != 0 {
                let rgba = crate::color_slot_rgba_at(slot as usize - 1, values[index + 3], now);
                values[index..index + 4].copy_from_slice(&[rgba.r, rgba.g, rgba.b, rgba.a]);
            }
        }
        values
    }

    /// Whether a track is moving at `now`.
    pub fn tracks_moving(&self, now: Instant) -> bool {
        let until = self.moving_until.load(Ordering::Relaxed);
        until != 0 && (since_epoch(now).max(0) as u64) < until
    }
}

/// A shader as the scene holds it.
#[derive(Clone, Debug)]
pub struct PaintShader {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub program: &'static ShaderProgram,
    pub uniforms: std::sync::Arc<ShaderUniforms>,
    /// The image the program samples, in the sprite atlas.
    pub image: Option<AtlasTile>,
    // KOAN: the layer group the shader composes, as its image: what was
    // painted inside `Window::paint_layer_group`, drawn into a texture of
    // its own (#1859).
    pub group: Option<usize>,
    // KOAN: where the shader's origin sits in its group's texture: the
    // texture is the visible part of the shader's bounds, its content moved
    // to the texture's origin, or the window's size, its content where it was
    // painted. Taken when the group closes, so an enclosing group moving the
    // shader leaves it as it is (#1878).
    pub group_inset: Point<ScaledPixels>,
}

impl From<PaintShader> for Primitive {
    fn from(shader: PaintShader) -> Self {
        Primitive::Shader(shader)
    }
}

/// One shader as a renderer hands it to the GPU: its quad, its clip and its
/// uniforms and time as they were when the frame was presented. Laid out for
/// both Metal and WGSL: blocks of 16-byte aligned floats.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ShaderInstance {
    /// x, y, width, height.
    pub bounds: [f32; 4],
    /// The content mask: x, y, width, height.
    pub clip: [f32; 4],
    pub uniforms: [f32; SHADER_UNIFORMS],
    /// `time` first; the rest pads the block.
    pub time: [f32; 4],
    /// The image's tile in its atlas texture, in texels: x, y, width, height.
    pub tile: [f32; 4],
}

impl Default for ShaderInstance {
    fn default() -> Self {
        Self {
            bounds: [0.0; 4],
            clip: [0.0; 4],
            uniforms: [0.0; SHADER_UNIFORMS],
            time: [0.0; 4],
            tile: [0.0; 4],
        }
    }
}

impl ShaderInstance {
    pub fn of(shader: &PaintShader, now: Instant) -> Self {
        let time = shader.uniforms.time_at(now);
        let mut uniforms = shader.uniforms.snapshot_at(now);
        if let Some(prepare) = shader.program.prepare {
            prepare(&mut uniforms, time);
        }
        let rect = |b: &Bounds<ScaledPixels>| {
            [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0]
        };
        Self {
            bounds: rect(&shader.bounds),
            clip: rect(&shader.content_mask.bounds),
            uniforms,
            time: [time, 0.0, 0.0, 0.0],
            // A group's texture holds the quad's bounds from its inset
            // (#1878).
            tile: match (&shader.image, shader.group) {
                (Some(tile), _) => {
                    let b = tile.bounds;
                    [
                        b.origin.x.0 as f32,
                        b.origin.y.0 as f32,
                        b.size.width.0 as f32,
                        b.size.height.0 as f32,
                    ]
                }
                (None, Some(_)) => [
                    shader.group_inset.x.0,
                    shader.group_inset.y.0,
                    shader.bounds.size.width.0,
                    shader.bounds.size.height.0,
                ],
                (None, None) => [0.0; 4],
            },
        }
    }
}

#[cfg(test)]
mod half_tests {
    use super::*;

    /// KOAN: a value whose mantissa rounds up carries into the exponent:
    /// just under 0.5 packs as 0.5, not 0.25.
    #[test]
    fn a_rounding_carry_reaches_the_exponent() {
        for v in [0.49995_f32, 0.24999, 0.062499, 0.999_99, 0.3, 0.75] {
            let (a, _) = unpack_halves(pack_halves(v, 0.0));
            assert!((a - v).abs() <= v * 1e-3, "{v} packed as {a}");
        }
    }
}
