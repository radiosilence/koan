//! Tracks: one segment of a moving value, which a shader program evaluates
//! at present time through `koan_track`, so the CPU writes it when the
//! value's target changes and never per frame. The same evaluation is here,
//! for reading where a track stands (a retarget starts from there) and for
//! knowing until when a shader moves.
//!
//! A shader's uniform block (gpui's `ShaderUniforms`, `SHADER_UNIFORMS`
//! floats) is laid out by koan-kit as `SHADER_PLAIN` plain values, then
//! `SHADER_TRACKS` tracks of `TRACK_FLOATS` each.

/// How many plain floats a block holds, written when they change: colours,
/// sizes, flags.
pub const SHADER_PLAIN: usize = 16;
/// How many tracks follow them.
pub const SHADER_TRACKS: usize = 8;
/// The floats one track takes: `from, to, start, length` and
/// `kind + 4 * repeat, a, b, c`.
pub const TRACK_FLOATS: usize = 8;

const _: () = assert!(SHADER_PLAIN + SHADER_TRACKS * TRACK_FLOATS == gpui::SHADER_UNIFORMS);

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

    pub fn encode(&self) -> [f32; TRACK_FLOATS] {
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

    pub fn decode(f: &[f32]) -> Self {
        // 16 and above flags a colour slot mixed in OKLab, as `koan_track`
        // reads it (`code % 16`).
        let code = f[4] as u32 % 16;
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

/// Where track `k` of a present's copy of the uniforms stands at `time`:
/// for a program's `prepare`.
pub fn track_at(u: &[f32; gpui::SHADER_UNIFORMS], k: usize, time: f32) -> f32 {
    let base = SHADER_PLAIN + k * TRACK_FLOATS;
    Track::decode(&u[base..base + TRACK_FLOATS]).at(time)
}

/// `koan_track` and its curves in MSL, prepended to every kit program.
pub(crate) const TRACKS_MSL: &str = r#"
// Tracks, evaluated as `Track::at` does.
float koan_bezier_curve(float a, float b, float s) {
    float u = 1.0 - s;
    return 3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s;
}

float koan_bezier(float x1, float y1, float x2, float y2, float t) {
    if (t <= 0.0 || t >= 1.0) {
        return clamp(t, 0.0, 1.0);
    }
    float s = t;
    for (int i = 0; i < 6; i++) {
        float u = 1.0 - s;
        float d = 3.0 * u * u * x1 + 6.0 * u * s * (x2 - x1) + 3.0 * s * s * (1.0 - x2);
        if (abs(d) < 1e-5) {
            break;
        }
        s = clamp(s - (koan_bezier_curve(x1, x2, s) - t) / d, 0.0, 1.0);
    }
    return koan_bezier_curve(y1, y2, s);
}

float koan_spring(float x0, float v0, float k, float c, float t) {
    float w = sqrt(max(k, 1e-3));
    float z = c / (2.0 * w);
    if (z < 1.0) {
        float wd = w * sqrt(1.0 - z * z);
        return exp(-z * w * t) * (x0 * cos(wd * t) + (v0 + z * w * x0) / wd * sin(wd * t));
    }
    if (z > 1.0001) {
        float root = sqrt(z * z - 1.0);
        float r1 = -w * (z - root);
        float r2 = -w * (z + root);
        float c1 = (v0 - r2 * x0) / (r1 - r2);
        return c1 * exp(r1 * t) + (x0 - c1) * exp(r2 * t);
    }
    return (x0 + (v0 + w * x0) * t) * exp(-w * t);
}

// Track `k` of the uniforms at `time`.
float koan_track(constant float *u, uint k, float time) {
    constant float *t = u + 16 + k * 8;
    float from = t[0];
    float to = t[1];
    float length = t[3];
    if (length <= 0.0) {
        return to;
    }
    float e = max(time - t[2], 0.0);
    uint code = uint(t[4]);
    uint kind = code % 4;
    uint repeat = code / 4;
    if (kind == 2) {
        return e >= length ? to : to + koan_spring(from - to, t[7], t[5], t[6], e);
    }
    float p = e / length;
    if (repeat == 0) {
        p = min(p, 1.0);
    } else if (repeat == 1) {
        p = fract(p);
    } else {
        float q = fmod(p, 2.0);
        p = q > 1.0 ? 2.0 - q : q;
    }
    if (kind == 1) {
        half2 a = as_type<half2>(t[5]);
        half2 b = as_type<half2>(t[6]);
        p = koan_bezier(a.x, a.y, b.x, b.y, p);
    }
    return from + (to - from) * p;
}

// The quad's image at `uv`, 0 to 1 across it: the renderer's sampler, under
// the name the kit's programs call it by.
float4 koan_sample(texture2d<float> image, float4 tile, float2 uv) {
    return sample_image(image, tile, uv);
}
"#;

/// `koan_track` and its curves in WGSL, prepended to every kit program.
pub(crate) const TRACKS_WGSL: &str = r#"
// Tracks, evaluated as `Track::at` does.
fn koan_bezier_curve(a: f32, b: f32, s: f32) -> f32 {
    let u = 1.0 - s;
    return 3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s;
}

fn koan_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    if (t <= 0.0 || t >= 1.0) {
        return clamp(t, 0.0, 1.0);
    }
    var s = t;
    for (var i = 0; i < 6; i++) {
        let u = 1.0 - s;
        let d = 3.0 * u * u * x1 + 6.0 * u * s * (x2 - x1) + 3.0 * s * s * (1.0 - x2);
        if (abs(d) < 1e-5) {
            break;
        }
        s = clamp(s - (koan_bezier_curve(x1, x2, s) - t) / d, 0.0, 1.0);
    }
    return koan_bezier_curve(y1, y2, s);
}

fn koan_spring(x0: f32, v0: f32, k: f32, c: f32, t: f32) -> f32 {
    let w = sqrt(max(k, 1e-3));
    let z = c / (2.0 * w);
    if (z < 1.0) {
        let wd = w * sqrt(1.0 - z * z);
        return exp(-z * w * t) * (x0 * cos(wd * t) + (v0 + z * w * x0) / wd * sin(wd * t));
    }
    if (z > 1.0001) {
        let root = sqrt(z * z - 1.0);
        let r1 = -w * (z - root);
        let r2 = -w * (z + root);
        let c1 = (v0 - r2 * x0) / (r1 - r2);
        return c1 * exp(r1 * t) + (x0 - c1) * exp(r2 * t);
    }
    return (x0 + (v0 + w * x0) * t) * exp(-w * t);
}

// Track `k` of the uniforms at `time`.
fn koan_track(u: array<vec4<f32>, 20>, k: u32, time: f32) -> f32 {
    var blocks = u;
    let ends = blocks[4u + k * 2u];
    let shape = blocks[5u + k * 2u];
    let from = ends.x;
    let to = ends.y;
    let length = ends.w;
    if (length <= 0.0) {
        return to;
    }
    let e = max(time - ends.z, 0.0);
    let code = u32(shape.x);
    let kind = code % 4u;
    let repeat = code / 4u;
    if (kind == 2u) {
        if (e >= length) {
            return to;
        }
        return to + koan_spring(from - to, shape.w, shape.y, shape.z, e);
    }
    var p = e / length;
    if (repeat == 0u) {
        p = min(p, 1.0);
    } else if (repeat == 1u) {
        p = fract(p);
    } else {
        let q = p - 2.0 * floor(p / 2.0);
        p = select(q, 2.0 - q, q > 1.0);
    }
    if (kind == 1u) {
        let a = unpack2x16float(bitcast<u32>(shape.y));
        let b = unpack2x16float(bitcast<u32>(shape.z));
        p = koan_bezier(a.x, a.y, b.x, b.y, p);
    }
    return from + (to - from) * p;
}

// The quad's image at `uv`, 0 to 1 across it: the renderer's sampler, under
// the name the kit's programs call it by.
fn koan_sample(tile: vec4<f32>, uv: vec2<f32>) -> vec4<f32> {
    return sample_image(tile, uv);
}
"#;

#[cfg(test)]
mod prelude_tests {
    use super::{TRACKS_MSL, TRACKS_WGSL};

    /// The kit's programs sample their image as `koan_sample`; the renderer
    /// names its sampler `sample_image`, so the prelude must bridge them or
    /// every sampling program fails to compile and draws nothing.
    #[test]
    fn the_prelude_defines_the_sampler_the_programs_call() {
        assert!(
            TRACKS_MSL
                .contains("float4 koan_sample(texture2d<float> image, float4 tile, float2 uv)")
        );
        assert!(TRACKS_MSL.contains("sample_image(image, tile, uv)"));
        assert!(
            TRACKS_WGSL.contains("fn koan_sample(tile: vec4<f32>, uv: vec2<f32>) -> vec4<f32>")
        );
        assert!(TRACKS_WGSL.contains("sample_image(tile, uv)"));
    }
}
