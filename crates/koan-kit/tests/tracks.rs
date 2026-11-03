//! Keyframes the GPU interpolates (#1859), as the CPU reckons them: every
//! curve against references worked out independently, the repeats at their
//! wraps, the encoding, when a shader presents, the present clock, and
//! `Animated`'s retargets, Reduce Motion and rebase. The GPU's drawing of the
//! same tracks is held to these in the renderers' own tests
//! (`tracks_draw_where_the_cpu_says`).

use std::time::{Duration, Instant};

use gpui::{Context, IntoElement, ParentElement, Render, Styled, TestAppContext, Window, div, px};
use koan_kit::components::{
    Animated, AnimatedQuad, Ease, Repeat, Repeat as TrackRepeat, ShaderProgram, Track, TrackCurve,
    present_now, spring_length,
};
use koan_kit::theme;

const CLOSE: f32 = 1e-4;

fn segment(curve: TrackCurve, repeat: TrackRepeat) -> Track {
    Track {
        from: 2.0,
        to: 6.0,
        start: 10.0,
        length: 4.0,
        curve,
        repeat,
    }
}

/// CSS's cubic-bezier, worked out in f64 by bisection to the bit.
fn css_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let curve = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..60 {
        let mid = (low + high) / 2.0;
        if curve(x1, x2, mid) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    curve(y1, y2, (low + high) / 2.0)
}

/// A unit-mass spring's displacement, integrated by fourth-order Runge-Kutta
/// in f64: the closed forms have to agree with it.
fn integrated_spring(x0: f64, v0: f64, stiffness: f64, damping: f64, t: f64) -> f64 {
    let accel = |x: f64, v: f64| -stiffness * x - damping * v;
    let (mut x, mut v) = (x0, v0);
    let dt = 1e-5;
    let steps = (t / dt).round() as usize;
    for _ in 0..steps {
        let (k1x, k1v) = (v, accel(x, v));
        let (k2x, k2v) = (
            v + k1v * dt / 2.0,
            accel(x + k1x * dt / 2.0, v + k1v * dt / 2.0),
        );
        let (k3x, k3v) = (
            v + k2v * dt / 2.0,
            accel(x + k2x * dt / 2.0, v + k2v * dt / 2.0),
        );
        let (k4x, k4v) = (v + k3v * dt, accel(x + k3x * dt, v + k3v * dt));
        x += dt / 6.0 * (k1x + 2.0 * k2x + 2.0 * k3x + k4x);
        v += dt / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v);
    }
    x
}

#[test]
fn linear_runs_straight_between_its_ends() {
    let track = segment(TrackCurve::Linear, TrackRepeat::Once);
    assert_eq!(track.at(0.0), 2.0, "before its start it stands at from");
    assert_eq!(track.at(10.0), 2.0);
    assert!((track.at(11.0) - 3.0).abs() < CLOSE);
    assert!((track.at(12.0) - 4.0).abs() < CLOSE);
    assert_eq!(track.at(14.0), 6.0);
    assert_eq!(track.at(100.0), 6.0, "past its end it holds to");
}

#[test]
fn beziers_follow_css() {
    let curves = [
        (0.42, 0.0, 0.58, 1.0),    // ease-in-out
        (0.2, 0.9, 0.3, 1.0),      // the theme's quick-out
        (0.17, 0.67, 0.83, 0.67),  // an arbitrary one
        (0.68, -0.55, 0.27, 1.55), // overshooting both ends
    ];
    for (x1, y1, x2, y2) in curves {
        let track = segment(TrackCurve::Bezier(x1, y1, x2, y2), TrackRepeat::Once);
        assert_eq!(track.at(10.0), 2.0);
        assert_eq!(track.at(14.0), 6.0);
        for share in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let expected =
                2.0 + 4.0 * css_bezier(x1 as f64, y1 as f64, x2 as f64, y2 as f64, share) as f32;
            let got = track.at(10.0 + 4.0 * share as f32);
            assert!(
                (got - expected).abs() < 1e-3,
                "cubic-bezier({x1}, {y1}, {x2}, {y2}) at {share}: {got}, CSS {expected}"
            );
        }
    }
    let in_out = segment(TrackCurve::Bezier(0.42, 0.0, 0.58, 1.0), TrackRepeat::Once);
    assert!(
        (in_out.at(12.0) - 4.0).abs() < 1e-3,
        "ease-in-out is symmetric"
    );
}

#[test]
fn springs_match_their_motion() {
    // Under-, critically and over-damped, set off still and moving.
    let springs = [
        (170.0, 12.0, 0.0),
        (100.0, 20.0, 0.0),
        (100.0, 40.0, 0.0),
        (170.0, 12.0, -8.0),
    ];
    for (stiffness, damping, velocity) in springs {
        let track = Track {
            from: 0.0,
            to: 1.0,
            start: 0.0,
            length: spring_length(-1.0, velocity, stiffness, damping),
            curve: TrackCurve::Spring {
                stiffness,
                damping,
                velocity,
            },
            repeat: TrackRepeat::Once,
        };
        assert_eq!(track.at(0.0), 0.0, "it sets off from where it was");
        for t in [0.02, 0.05, 0.1, 0.2, 0.4] {
            let expected = 1.0
                + integrated_spring(-1.0, velocity as f64, stiffness as f64, damping as f64, t)
                    as f32;
            let got = track.at(t as f32);
            assert!(
                (got - expected).abs() < 1e-3,
                "spring k={stiffness} c={damping} v={velocity} at {t}s: {got}, integrated {expected}"
            );
        }
        let settled = 1.0
            + integrated_spring(
                -1.0,
                velocity as f64,
                stiffness as f64,
                damping as f64,
                track.length as f64,
            ) as f32;
        assert!(
            (settled - 1.0).abs() < 2e-3,
            "spring k={stiffness} c={damping}: within a hair of its end by its length ({settled})"
        );
        assert_eq!(
            track.at(track.length + 0.01),
            1.0,
            "and holds its end after"
        );
    }
}

#[test]
fn repeats_wrap_where_they_should() {
    let looping = segment(TrackCurve::Linear, TrackRepeat::Loop);
    assert!((looping.at(13.0) - 5.0).abs() < CLOSE);
    assert!(
        (looping.at(14.0) - 2.0).abs() < CLOSE,
        "a loop starts again at from"
    );
    assert!((looping.at(15.0) - 3.0).abs() < CLOSE);
    assert_eq!(looping.end(), None, "a loop never ends");

    let breath = segment(TrackCurve::Linear, TrackRepeat::Alternate);
    assert!((breath.at(14.0) - 6.0).abs() < CLOSE);
    assert!((breath.at(16.0) - 4.0).abs() < CLOSE, "it comes back");
    assert!((breath.at(18.0) - 2.0).abs() < CLOSE);
    assert!((breath.at(19.0) - 3.0).abs() < CLOSE, "and goes again");
    assert_eq!(breath.end(), None);

    assert_eq!(
        segment(TrackCurve::Linear, TrackRepeat::Once).end(),
        Some(14.0)
    );
    assert_eq!(Track::hold(3.0).at(1e6), 3.0);
}

#[test]
fn tracks_survive_the_uniforms_encoding() {
    let tracks = [
        segment(TrackCurve::Linear, TrackRepeat::Once),
        segment(
            TrackCurve::Bezier(0.68, -0.55, 0.27, 1.55),
            TrackRepeat::Alternate,
        ),
        segment(
            TrackCurve::Spring {
                stiffness: 170.0,
                damping: 12.0,
                velocity: -3.5,
            },
            TrackRepeat::Once,
        ),
        segment(TrackCurve::Linear, TrackRepeat::Loop),
    ];
    for track in &tracks {
        let back = Track::decode(&track.encode());
        assert_eq!(back.from, track.from);
        assert_eq!(back.to, track.to);
        assert_eq!(back.start, track.start);
        assert_eq!(back.length, track.length);
        assert_eq!(back.repeat, track.repeat);
        match (back.curve, track.curve) {
            // Bezier control points travel as half floats.
            (TrackCurve::Bezier(a, b, c, d), TrackCurve::Bezier(e, f, g, h)) => {
                for (x, y) in [(a, e), (b, f), (c, g), (d, h)] {
                    assert!((x - y).abs() < 1e-3, "{x} against {y}");
                }
            }
            (back, expected) => assert_eq!(back, expected),
        }
    }
    // The plain values before the tracks are untouched by them.
    let animated = Animated::new();
    animated.set(3, 0.25);
    animated.hold(0, 9.0);
    assert_eq!(animated.get(3), 0.25);
}

#[test]
fn a_track_moves_until_it_lands() {
    let animated = Animated::new();
    animated.segment(
        0,
        0.0,
        1.0,
        Animated::now(),
        Duration::from_millis(150),
        Ease::Linear,
        Repeat::Once,
    );
    assert!(animated.moving(), "moving: the window keeps its refreshes");
    std::thread::sleep(Duration::from_millis(200));
    assert!(!animated.moving(), "landed: the window parks");
    animated.hold(1, 0.5);
    assert!(!animated.moving(), "a hold keeps nothing running");
    animated.segment(
        2,
        0.0,
        1.0,
        Animated::now(),
        Duration::from_millis(50),
        Ease::Linear,
        Repeat::Loop,
    );
    std::thread::sleep(Duration::from_millis(80));
    assert!(animated.moving(), "a repeat keeps presenting");
}

#[test]
fn time_is_reckoned_at_the_present() {
    // The kit's colour source sees each present's time; a frame made now
    // shows then, never before now.
    koan_kit::components::install_color_slots();
    let ahead = Instant::now() + Duration::from_millis(8);
    gpui::refresh_dynamic_colors(ahead);
    let lead = present_now().saturating_duration_since(Instant::now());
    assert!(
        lead > Duration::from_millis(6) && lead <= Duration::from_millis(8),
        "a frame made now shows a refresh on: {lead:?}"
    );
}

#[test]
fn a_retarget_starts_where_the_track_stands() {
    let animated = Animated::new();
    animated.segment(
        0,
        0.0,
        10.0,
        Animated::now() - Duration::from_millis(500),
        Duration::from_secs(1),
        Ease::Linear,
        Repeat::Once,
    );
    let before = animated.at(0, Animated::now());
    animated.to(0, -5.0, Duration::from_secs(1), Ease::QuickOut);
    let after = animated.at(0, Animated::now());
    assert!(
        (before - after).abs() < 0.05,
        "no jump on a retarget: {before} then {after}"
    );
    assert_eq!(animated.track(0).to, -5.0);
}

#[test]
fn a_retargeted_spring_keeps_its_speed() {
    let animated = Animated::new();
    let spring = Ease::Spring {
        stiffness: 170.0,
        damping: 12.0,
    };
    animated.to(0, 1.0, Duration::ZERO, spring);
    std::thread::sleep(Duration::from_millis(60));
    let speed = |animated: &Animated| {
        let now = Animated::now();
        let dt = Duration::from_millis(2);
        (animated.at(0, now + dt) - animated.at(0, now)) / dt.as_secs_f32()
    };
    let (value, velocity) = (animated.at(0, Animated::now()), speed(&animated));
    animated.to(0, 0.0, Duration::ZERO, spring);
    let (value_after, velocity_after) = (animated.at(0, Animated::now()), speed(&animated));
    assert!(
        (value - value_after).abs() < 0.02,
        "{value} then {value_after}"
    );
    // The new target pulls the other way at once; what carries on is the
    // speed it had, less what one sample's span of the new pull takes off.
    assert!(
        (velocity - velocity_after).abs() < velocity.abs() * 0.15 + 0.5,
        "speed {velocity} then {velocity_after}"
    );
}

#[test]
fn an_hour_on_the_clock_moves_and_nothing_jumps() {
    let animated = Animated::new();
    animated.set_origin(Instant::now() - Duration::from_secs(2 * 3600));
    animated.segment(
        0,
        0.0,
        100.0,
        Animated::now() - Duration::from_secs(10),
        Duration::from_secs(40),
        Ease::Linear,
        Repeat::Once,
    );
    let before = animated.at(0, Animated::now());
    // The next write rebases the clock and every track with it.
    animated.hold(1, 0.0);
    let after = animated.at(0, Animated::now());
    assert!((before - after).abs() < 0.1, "{before} then {after}");
    assert!(
        animated.track(0).start.abs() < 60.0,
        "starts are counted from the new origin"
    );
}

static PLAIN: ShaderProgram = ShaderProgram {
    name: "koan-test-plain",
    msl: "float4 shade(float2 position, float2 size, float time, constant float *u, texture2d<float> image, float4 tile) { return float4(koan_track(u, 0, time)); }",
    wgsl: "fn shade(position: vec2<f32>, size: vec2<f32>, time: f32, u: array<vec4<f32>, 20>, tile: vec4<f32>) -> vec4<f32> { return vec4<f32>(koan_track(u, 0u, time)); }",
    prepare: None,
};

struct Quad(Animated);

impl Render for Quad {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(20.)).child(
            AnimatedQuad::new(&PLAIN, &self.0)
                .decorative()
                .size(px(10.)),
        )
    }
}

#[gpui::test]
fn reduce_motion_stands_a_decorative_quad_at_its_ends(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::init(cx);
        cx.set_reduce_motion(true);
    });
    let animated = Animated::new();
    animated.segment(
        0,
        0.0,
        1.0,
        Animated::now(),
        Duration::from_secs(1),
        Ease::Linear,
        Repeat::Once,
    );
    let shown = animated.clone();
    cx.add_window(|_, _| Quad(shown));
    cx.run_until_parked();
    assert_eq!(
        animated.track(0).length,
        0.0,
        "written before it was drawn: held"
    );
    assert_eq!(animated.at(0, Animated::now()), 1.0, "at its end");
    animated.to(0, 0.5, Duration::from_secs(1), Ease::InOut);
    assert_eq!(animated.track(0).length, 0.0, "written after: held");
    assert_eq!(animated.at(0, Animated::now()), 0.5);
}

/// A colour naming a slot is read from the slot at each present; a plain
/// write over any of its floats lets the slot go, so what was written stands.
#[test]
fn a_plain_write_over_a_slot_colour_lets_it_go() {
    use gpui::{Rgba, ShaderUniforms};
    use koan_kit::components::ColorSlot;
    let red = Rgba {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    let slot = ColorSlot::claim(red).expect("a free slot");
    let uniforms = ShaderUniforms::new();
    uniforms.set_color(1, slot.hsla(0.5));
    let now = Instant::now();
    assert_eq!(uniforms.presented_at(now).1[4..8], [1.0, 0.0, 0.0, 0.5]);
    uniforms.set(6, 0.25);
    let values = uniforms.presented_at(now).1;
    assert_eq!(values[6], 0.25);
    assert_eq!(values[4], 0.0, "the slot no longer draws over the floats");
}

/// A colour slot mixed in OKLab carries 16 in its track's code; the CPU reads
/// the curve and repeat beneath it, as the shaders do (`code % 16`).
#[test]
fn the_oklab_flag_leaves_the_repeat_as_it_was() {
    for repeat in [TrackRepeat::Once, TrackRepeat::Loop, TrackRepeat::Alternate] {
        let mut encoded = segment(TrackCurve::Bezier(0.2, 0.9, 0.3, 1.0), repeat).encode();
        encoded[4] += 16.0;
        let track = Track::decode(&encoded);
        assert_eq!(track.repeat, repeat);
        assert!(matches!(track.curve, TrackCurve::Bezier(..)));
    }
}

/// A clock run for a length stops by itself (#1741): past it, the quad is
/// neither running nor asking for presents.
#[test]
fn a_clock_run_for_a_length_stops() {
    let animated = Animated::new();
    animated.run_for(Duration::from_millis(20));
    assert!(animated.is_running());
    std::thread::sleep(Duration::from_millis(40));
    assert!(!animated.is_running(), "the clock ran on past its length");
    animated.run(true);
    assert!(animated.is_running(), "run(true) runs it again");
    std::thread::sleep(Duration::from_millis(40));
    assert!(animated.is_running(), "an open-ended run does not lapse");
}
