//! Frame-time measurement, built with the `profiler` feature.
//!
//! With `KOAN_GUI_MEASURE=scroll` the app scrolls the queue through its own
//! window, with input events delivered to the window and nothing sent to
//! the screen, then prints GPUI's frame histograms and quits. Run it against
//! a throwaway `KOAN_CONFIG_DIR` holding a large queue.

use std::time::Duration;

use gpui_kit::*;

/// Wheel steps per direction, 16 ms apart: about five seconds each way.
const STEPS: usize = 300;
const STEP: Pixels = px(40.);

pub fn start(cx: &mut App) {
    if std::env::var("KOAN_GUI_MEASURE").as_deref() != Ok("scroll") {
        return;
    }
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
        for direction in [-1.0, 1.0] {
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
