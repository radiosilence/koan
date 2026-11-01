//! Frame-time measurement, built with the `profiler` feature.
//!
//! With `KOAN_GUI_MEASURE=play` the app starts playback and runs on, for
//! its CPU use to be read from outside. With `KOAN_GUI_MEASURE=scroll` the
//! app scrolls the queue through its own
//! window, with input events delivered to the window and nothing sent to
//! the screen, then prints GPUI's frame histograms and quits. Run it against
//! a throwaway `KOAN_CONFIG_DIR` holding a large queue, with
//! `KOAN_DEVICES__NEARBY=false KOAN_DEVICES__DISCOVERABLE=false` so the run
//! neither announces a device nor takes the devices port.
//!
//! Each pass prints the queue row at the top of the list as it ends, so the
//! numbers carry proof that the list moved.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gpui_kit::*;

/// Wheel steps per direction, 16 ms apart: about five seconds each way.
const STEPS: usize = 300;
const STEP: Pixels = px(40.);

/// The queue row at the top of the list, kept by the queue as it scrolls.
pub static TOP: AtomicUsize = AtomicUsize::new(0);

pub fn start(cx: &mut App) {
    match std::env::var("KOAN_GUI_MEASURE").as_deref() {
        Ok("scroll") => scroll(cx),
        // Starts playback and leaves the app to run, for its CPU use to be
        // read from outside while it plays with nobody touching it.
        Ok("play") => crate::engine::command(cx, "play", |e| e.resume()),
        // The same with the app hidden, which stops the window's frames.
        Ok("hidden") => cx.hide(),
        Ok("play-hidden") => {
            crate::engine::command(cx, "play", |e| e.resume());
            cx.hide();
        }
        _ => {}
    }
}

fn scroll(cx: &mut App) {
    let background = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        background.timer(Duration::from_secs(3)).await;
        let Some(window) = cx.update(|cx| cx.windows().first().copied()) else {
            return;
        };
        let centre = window
            .update(cx, |_, window, _| {
                let size = window.viewport_size();
                point(size.width * 0.6, size.height * 0.5)
            })
            .unwrap_or_default();
        for (direction, pass) in [(-1.0, "down"), (1.0, "up")] {
            for _ in 0..STEPS {
                let event = ScrollEvent {
                    position: centre,
                    delta: ScrollDelta::Pixels(point(px(0.), STEP * direction)),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                    source: ScrollSource::Pointer,
                };
                window
                    .update(cx, |_, window, cx| {
                        window.dispatch_event(PlatformInput::Scroll(event), cx);
                    })
                    .ok();
                background.timer(Duration::from_millis(16)).await;
            }
            let rows = cx.update(|cx| {
                cx.global::<crate::mirror::Mirror>()
                    .queue
                    .read(cx)
                    .rows
                    .len()
            });
            let top = TOP.load(Ordering::Relaxed);
            println!("scrolled {pass}: top row {top} of {rows}");
        }
        window
            .update(cx, |_, window, _| {
                let frames = window.frame_duration_snapshot();
                let ms = |ns: u64| ns as f64 / 1e6;
                for (name, histogram) in [
                    ("draw", &frames.draw_duration_histogram),
                    ("dirty to present", &frames.dirty_to_present_histogram),
                ] {
                    println!(
                        "{name}: {} frames, p50 {:.2} ms, p95 {:.2} ms, p99 {:.2} ms, max {:.2} ms",
                        histogram.len(),
                        ms(histogram.value_at_quantile(0.5)),
                        ms(histogram.value_at_quantile(0.95)),
                        ms(histogram.value_at_quantile(0.99)),
                        ms(histogram.max()),
                    );
                }
            })
            .ok();
        cx.update(|cx| cx.quit());
    })
    .detach();
}
