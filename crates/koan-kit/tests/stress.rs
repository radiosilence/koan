//! The animation engine under churn: many threads writing colour slots and
//! shader uniforms while a presenter reads them, storms of retargets, the
//! clocks over long uptimes and a moving present lead, hover storms over many
//! controls, the accent retargeted faster than it can settle, and Reduce
//! Motion turned on mid-animation. Each runs in this binary of its own, since
//! some move the process's present lead; those take `CLOCK` first.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{
    ColorSlot, Context, Hsla, IntoElement, Modifiers, ParentElement, Render, Rgba, ShaderUniforms,
    Styled, TestAppContext, Track, TrackCurve, TrackRepeat, Window, div, point, present_now, px,
    set_present_lead,
};
use koan_kit::components::{
    Animated, AnimatedQuad, Ease, Repeat, ShaderProgram, Toolbar, ToolbarButton,
};
use koan_kit::theme::{self, Accent, Icon, KoanTheme, Look, Motion};
use parking_lot::Mutex;

/// Held by the tests that move the present lead or count the slot table,
/// which every test in the binary shares.
static CLOCK: Mutex<()> = Mutex::new(());

/// A small generator, so a failure replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn rgba(rng: &mut Rng) -> Rgba {
    Rgba {
        r: rng.unit(),
        g: rng.unit(),
        b: rng.unit(),
        a: rng.unit(),
    }
}

fn curve(rng: &mut Rng) -> TrackCurve {
    match rng.below(3) {
        0 => TrackCurve::Linear,
        1 => TrackCurve::Bezier(rng.unit(), rng.unit() * 2.0 - 0.5, rng.unit(), 1.0),
        _ => TrackCurve::Linear,
    }
}

/// How many slots are free: claimed until none is, then let go.
fn free_slots() -> usize {
    let held: Vec<ColorSlot> = std::iter::from_fn(|| ColorSlot::claim(Rgba::default())).collect();
    held.len()
}

fn finite(c: Rgba) -> bool {
    [c.r, c.g, c.b, c.a]
        .iter()
        .all(|v| v.is_finite() && (-1e-3..=1.0 + 1e-3).contains(v))
}

#[test]
fn the_slot_table_survives_many_threads_and_frees_what_they_drop() {
    let _clock = CLOCK.lock();
    let free = free_slots();
    let threads: Vec<_> = (0..8)
        .map(|seed| {
            std::thread::spawn(move || {
                let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (seed + 1));
                let mut held: Vec<ColorSlot> = Vec::new();
                for _ in 0..3000 {
                    match rng.below(6) {
                        0 | 1 => held.extend(ColorSlot::claim(rgba(&mut rng))),
                        2 if !held.is_empty() => {
                            held.swap_remove(rng.below(held.len()));
                        }
                        3 if !held.is_empty() => {
                            let track = Track {
                                from: 0.0,
                                to: 1.0,
                                start: 0.0,
                                length: rng.unit() * 0.05,
                                curve: curve(&mut rng),
                                repeat: TrackRepeat::Once,
                            };
                            let slot = &held[rng.below(held.len())];
                            if rng.below(2) == 0 {
                                slot.to(rgba(&mut rng), track);
                            } else {
                                slot.to_oklab(rgba(&mut rng), track);
                            }
                        }
                        _ => {
                            for slot in &held {
                                let shown = slot.at(present_now());
                                assert!(finite(shown), "{shown:?}");
                                let resolved = Rgba::from(slot.hsla(0.5));
                                assert!(finite(resolved), "{resolved:?}");
                            }
                        }
                    }
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("no thread panics");
    }
    assert_eq!(free_slots(), free, "every dropped slot is free again");
}

#[test]
fn uniforms_written_while_a_presenter_reads_them_stay_whole_and_land() {
    let uniforms = Arc::new(ShaderUniforms::new());
    uniforms.set_origin(Instant::now());
    let slots: Arc<Vec<ColorSlot>> = Arc::new(
        (0..4)
            .filter_map(|_| {
                ColorSlot::claim(Rgba {
                    r: 1.0,
                    g: 0.5,
                    b: 0.0,
                    a: 1.0,
                })
            })
            .collect(),
    );
    let writing = Arc::new(AtomicBool::new(true));
    let presenter = {
        let (uniforms, writing) = (uniforms.clone(), writing.clone());
        std::thread::spawn(move || {
            while writing.load(Ordering::Relaxed) {
                uniforms.take_present();
                uniforms.is_live();
                let values = uniforms.snapshot_at(present_now());
                assert!(values.iter().all(|v| v.is_finite()), "{values:?}");
            }
        })
    };
    let mut rng = Rng(7);
    for _ in 0..20_000 {
        match rng.below(4) {
            0 => {
                let index = rng.below(13);
                let color = if rng.below(2) == 0 && !slots.is_empty() {
                    slots[rng.below(slots.len())].hsla(rng.unit())
                } else {
                    Hsla::from(rgba(&mut rng))
                };
                uniforms.set_color(index, color);
            }
            1 => {
                uniforms.set(rng.below(16), rng.unit());
            }
            2 => {
                let start = uniforms.time_at(present_now());
                uniforms.set_track(
                    rng.below(8),
                    Track {
                        from: rng.unit(),
                        to: rng.unit(),
                        start,
                        length: rng.unit() * 0.03,
                        curve: curve(&mut rng),
                        repeat: TrackRepeat::Once,
                    },
                );
            }
            _ => {
                let slot = &slots[rng.below(slots.len())];
                slot.to(
                    rgba(&mut rng),
                    Track {
                        from: 0.0,
                        to: 1.0,
                        start: 0.0,
                        length: rng.unit() * 0.03,
                        curve: TrackCurve::Linear,
                        repeat: TrackRepeat::Once,
                    },
                );
            }
        }
    }
    writing.store(false, Ordering::Relaxed);
    presenter.join().expect("the presenter does not panic");
    // Everything written has an end within 30 ms: once that is past, the
    // shader presents once more at most and then parks.
    std::thread::sleep(Duration::from_millis(80));
    let presents = (0..5).filter(|_| uniforms.take_present()).count();
    assert!(presents <= 1, "{presents} presents after everything landed");
    assert!(!uniforms.is_live(), "the window parks");
}

#[test]
fn a_storm_of_retargets_stays_finite_and_comes_to_rest() {
    let animated = Animated::new();
    let mut rng = Rng(42);
    for i in 0..20_000 {
        let k = rng.below(8);
        let target = rng.unit() * 200.0 - 100.0;
        let length = Duration::from_micros(rng.below(30_000) as u64);
        match rng.below(5) {
            0 => animated.to(k, target, length, Ease::QuickOut),
            1 => animated.to(
                k,
                target,
                Duration::ZERO,
                Ease::Spring {
                    stiffness: 50.0 + rng.unit() * 400.0,
                    damping: 2.0 + rng.unit() * 40.0,
                },
            ),
            2 => animated.segment(
                k,
                target,
                -target,
                Animated::now() - Duration::from_micros(rng.below(50_000) as u64),
                length,
                Ease::InOut,
                Repeat::Once,
            ),
            3 => animated.hold(k, target),
            _ => animated.to(k, target, length, Ease::Bezier(0.3, -0.4, 0.6, 1.6)),
        }
        if i % 97 == 0 {
            std::thread::sleep(Duration::from_micros(300));
        }
        let shown = animated.at(k, Animated::now());
        assert!(
            shown.is_finite() && shown.abs() < 1e4,
            "track {k} at {shown} after {i} writes"
        );
    }
    for k in 0..8 {
        animated.to(k, 0.0, Duration::from_millis(20), Ease::QuickOut);
    }
    std::thread::sleep(Duration::from_millis(60));
    assert!(!animated.moving(), "every track has landed");
    for k in 0..8 {
        assert!(animated.at(k, Animated::now()).abs() < 1e-4);
    }
}

#[test]
fn a_month_of_uptime_rebases_without_a_jump() {
    let animated = Animated::new();
    animated.set_origin(Instant::now() - Duration::from_secs(30 * 24 * 3600));
    animated.segment(
        0,
        0.0,
        100.0,
        Animated::now() - Duration::from_secs(5),
        Duration::from_secs(10),
        Ease::Linear,
        Repeat::Once,
    );
    animated.segment(
        1,
        0.0,
        1.0,
        Animated::now() - Duration::from_secs(1000),
        Duration::from_millis(1500),
        Ease::InOut,
        Repeat::Alternate,
    );
    let before = [0, 1].map(|k| animated.at(k, Animated::now()));
    animated.hold(2, 1.0);
    let after = [0, 1].map(|k| animated.at(k, Animated::now()));
    for (b, a) in before.iter().zip(after) {
        assert!((b - a).abs() < 0.02, "{before:?} then {after:?}");
    }
    assert!(animated.track(0).start.abs() < 60.0);
    assert!(
        animated.track(1).start.abs() < 60.0,
        "a repeat re-anchors near now"
    );
}

#[test]
fn colour_slots_rebase_mid_move_without_a_jump() {
    let _clock = CLOCK.lock();
    let slot = ColorSlot::claim(Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    })
    .expect("a slot");
    let long = Track {
        from: 0.0,
        to: 1.0,
        start: 0.0,
        length: 2.0 * 3600.0,
        curve: TrackCurve::Linear,
        repeat: TrackRepeat::Once,
    };
    slot.to(
        Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        },
        long,
    );
    // An hour and a half on, as the present clock reckons it: the next write
    // to the table moves its origin.
    set_present_lead(Duration::from_secs(5400));
    let before = slot.at(present_now());
    let other = ColorSlot::claim(Rgba::default()).expect("a slot");
    other.to(
        Rgba {
            r: 0.5,
            g: 0.5,
            b: 0.5,
            a: 1.0,
        },
        Track::hold(1.0),
    );
    let after = slot.at(present_now());
    set_present_lead(Duration::ZERO);
    assert!(
        (before.r - 0.75).abs() < 0.01,
        "three quarters through: {before:?}"
    );
    assert!(
        (before.r - after.r).abs() < 1e-3,
        "{before:?} then {after:?}"
    );
    assert!(slot.moving(present_now() + Duration::from_secs(5400)));
}

#[test]
fn the_present_clock_never_runs_backwards_while_the_lead_moves() {
    let _clock = CLOCK.lock();
    let moving = Arc::new(AtomicBool::new(true));
    let leader = {
        let moving = moving.clone();
        std::thread::spawn(move || {
            let mut rng = Rng(3);
            while moving.load(Ordering::Relaxed) {
                set_present_lead(Duration::from_micros(rng.below(25_000) as u64));
            }
        })
    };
    let mut last = present_now();
    for _ in 0..200_000 {
        let now = present_now();
        assert!(now >= last, "the present clock ran backwards");
        last = now;
    }
    moving.store(false, Ordering::Relaxed);
    leader.join().unwrap();
    set_present_lead(Duration::ZERO);
}

/// Forty toolbar buttons, each fading its glyph out over `Motion::LEAVE`
/// through a colour slot as the pointer leaves it.
struct Buttons;

impl Render for Buttons {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size(px(1600.))
            .flex()
            .flex_col()
            .children((0..4).map(|row| {
                let mut toolbar = Toolbar::new();
                for i in 0..10 {
                    toolbar = toolbar.leading(ToolbarButton::new(
                        ("b", (row * 10 + i) as usize),
                        Icon::Back,
                        "back",
                    ));
                }
                toolbar
            }))
    }
}

#[gpui_kit::test]
fn a_hover_storm_gives_back_every_colour_slot(cx: &mut TestAppContext) {
    let _clock = CLOCK.lock();
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let free = free_slots();
    let (view, cx) = cx.add_window_view(|_, _| Buttons);
    cx.run_until_parked();
    let mut rng = Rng(11);
    for _ in 0..3000 {
        let at = point(px(rng.below(700) as f32), px(rng.below(220) as f32));
        cx.simulate_mouse_move(at, None, Modifiers::none());
        if rng.below(10) == 0 {
            cx.run_until_parked();
        }
    }
    cx.simulate_mouse_move(point(px(1500.), px(1500.)), None, Modifiers::none());
    cx.run_until_parked();
    // Every leave fade has landed; the next draw lets the slots go.
    std::thread::sleep(Motion::LEAVE + Duration::from_millis(50));
    cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        let _ = cx;
    });
    cx.run_until_parked();
    assert_eq!(free_slots(), free, "a slot outlived its fade");
}

struct Painted;

impl Render for Painted {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        div()
            .size(px(40.))
            .bg(look.accent.hsla())
            .text_color(look.text_color(theme::Tone::Accent))
            .child("a")
    }
}

fn accent(rng: &mut Rng) -> Accent {
    Accent::of(Some([
        rng.unit() as f64,
        rng.unit() as f64,
        rng.unit() as f64,
    ]))
}

#[gpui_kit::test]
fn the_accent_retargeted_faster_than_it_settles_lands_on_the_last(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let (_, cx) = cx.add_window_view(|_, _| Painted);
    let mut rng = Rng(5);
    let mut last = Accent::MINT;
    for i in 0..500 {
        last = accent(&mut rng);
        cx.update(|_, cx| KoanTheme::set_accent(last, true, cx));
        if i % 25 == 0 {
            cx.run_until_parked();
            let shown = cx.update(|window, cx| Rgba::from(Look::of(window, cx).accent.hsla()));
            assert!(finite(shown), "{shown:?}");
        }
    }
    cx.run_until_parked();
    std::thread::sleep(Motion::DISSOLVE + Duration::from_millis(100));
    let (shown, target) = cx.update(|window, cx| {
        let look = Look::of(window, cx);
        (Rgba::from(look.accent.hsla()), look.accent.rgba())
    });
    for (a, b) in [shown.r, shown.g, shown.b]
        .into_iter()
        .zip([target.r, target.g, target.b])
    {
        assert!((a - b).abs() < 2e-3, "{shown:?}, not {target:?}");
    }
    let _ = last;
}

#[gpui_kit::test]
fn reduce_motion_mid_settle_lands_the_accent(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let (_, cx) = cx.add_window_view(|_, _| Painted);
    let blue = Accent::of(Some([0.1, 0.2, 0.9]));
    cx.update(|_, cx| KoanTheme::set_accent(blue, true, cx));
    cx.run_until_parked();
    cx.update(|window, cx| {
        cx.set_reduce_motion(true);
        window.refresh();
    });
    cx.run_until_parked();
    let (shown, target) = cx.update(|window, cx| {
        let look = Look::of(window, cx);
        (Rgba::from(look.accent.hsla()), look.accent.rgba())
    });
    for (a, b) in [shown.r, shown.g, shown.b]
        .into_iter()
        .zip([target.r, target.g, target.b])
    {
        assert!(
            (a - b).abs() < 2e-3,
            "still turning under Reduce Motion: {shown:?}"
        );
    }
}

static PLAIN: ShaderProgram = ShaderProgram {
    name: "koan-stress-plain",
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

#[gpui_kit::test]
fn reduce_motion_toggled_mid_animation_lands_and_lifts(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });
    let animated = Animated::new();
    let shown = animated.clone();
    let (view, cx) = cx.add_window_view(|_, _| Quad(shown));
    for round in 0..50 {
        animated.segment(
            0,
            0.0,
            1.0,
            Animated::now(),
            Duration::from_secs(5),
            Ease::Linear,
            Repeat::Once,
        );
        cx.run_until_parked();
        let reduce = round % 2 == 0;
        cx.update(|_, cx| {
            cx.set_reduce_motion(reduce);
            view.update(cx, |_, cx| cx.notify());
        });
        cx.run_until_parked();
        if reduce {
            assert_eq!(animated.at(0, Animated::now()), 1.0, "landed at its end");
            assert!(!animated.moving());
        }
    }
    cx.update(|_, cx| cx.set_reduce_motion(false));
    animated.segment(
        0,
        0.0,
        1.0,
        Animated::now(),
        Duration::from_secs(5),
        Ease::Linear,
        Repeat::Once,
    );
    cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
    cx.run_until_parked();
    assert!(animated.moving(), "motion lifted moves again");
}
