//! The present clock under a moving lead, in a process of its own: the
//! clock never runs back, so a lead moved by an hour leaves it an hour ahead
//! for whatever runs after, which would be every other test's clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::{ColorSlot, Rgba, Track, TrackCurve, TrackRepeat, present_now, set_present_lead};
use parking_lot::Mutex;

static CLOCK: Mutex<()> = Mutex::new(());

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
    // The present clock never runs back, so it stands where the lead took
    // it: a quarter of the move still to go.
    assert!(slot.moving(present_now()));
}

#[test]
fn the_present_clock_never_runs_backwards_while_the_lead_moves() {
    let _clock = CLOCK.lock();
    let moving = Arc::new(AtomicBool::new(true));
    let leader = {
        let moving = moving.clone();
        std::thread::spawn(move || {
            let mut seed = 3u64;
            while moving.load(Ordering::Relaxed) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                set_present_lead(Duration::from_micros(seed % 25_000));
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
