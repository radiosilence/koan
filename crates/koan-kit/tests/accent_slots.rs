//! The accent's colour slots, in a process of their own: the slot table is
//! one per process, and the library's tests leave theirs held, since GPUI's
//! test harness never drops its apps.

use std::time::{Duration, Instant};

use koan_kit::components::{ColorSlot, Repeat, Track, TrackCurve};
use koan_kit::theme::Accent;
use koan_kit::theme::accent::oklch;

fn mix_oklab(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    let la = oklch::lab(a.map(oklch::linear));
    let lb = oklch::lab(b.map(oklch::linear));
    let mixed = [0, 1, 2].map(|i| la[i] + (lb[i] - la[i]) * t);
    oklch::linear_from_lab(mixed).map(oklch::gamma)
}

/// The GPU's mix of a slot's ends in OKLab, as the CPU reads the table,
/// agrees with the accent's own OKLab mix: a record's accent turns through
/// the hues between, never through grey.
#[test]
fn slots_mix_the_accent_in_oklab() {
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
            repeat: Repeat::Once,
        },
    );
    let half = Instant::now() + Duration::from_secs_f32(length / 2.0);
    let gpu = slot.at(half);
    let cpu = mix_oklab(mint.rgb, blue.rgb, 0.5);
    for (a, b) in [gpu.r, gpu.g, gpu.b].into_iter().zip(cpu) {
        assert!((a as f64 - b).abs() < 2e-3, "{gpu:?} against {cpu:?}");
    }
}
