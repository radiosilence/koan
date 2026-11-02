//! Frame-time measurement, built with the `profiler` feature.
//!
//! With `KOAN_GUI_MEASURE=frames` (or `play-frames`) the app counts the
//! frames the platform asks for over twenty idle seconds and how many drew
//! anything, then quits. With `KOAN_GUI_MEASURE=play` the app starts playback
//! and runs on, for
//! its CPU use to be read from outside. With `KOAN_GUI_MEASURE=scroll` the
//! app scrolls the queue through its own
//! window, with input events delivered to the window and nothing sent to
//! the screen, then prints GPUI's frame histograms and quits. Run it against
//! a throwaway `KOAN_CONFIG_DIR` holding a large queue, with
//! `KOAN_DEVICES__NEARBY=false KOAN_DEVICES__DISCOVERABLE=false` so the run
//! neither announces a device nor takes the devices port.
//!
//! With `KOAN_GUI_MEASURE=fling` the app opens the album grid, flings it to
//! the end and back by the same synthetic wheel events, returns to the
//! queue, and prints the process's footprint (macOS's `footprint`) at launch
//! and after each stage, then quits: the memory a long browse leaves behind.
//!
//! Each pass prints the queue row at the top of the list as it ends, so the
//! numbers carry proof that the list moved.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use koan_kit::app::*;
// Scrolling the window by synthetic wheel events, as a person would: input
// GPUI delivers, which only this measuring build makes.
use koan_kit::raw::{PlatformInput, ScrollDelta, ScrollSource, TouchPhase, point, px};
// the profiler's journal, read by the measuring build alone.
use koan_kit::raw::profiler::journal::{ForegroundJournalEntry, IntervalBoundary};

/// Wheel steps per direction, 16 ms apart: about five seconds each way.
const STEPS: usize = 300;
const STEP: Pixels = px(40.);

/// The queue row at the top of the list, kept by the queue as it scrolls.
pub static TOP: AtomicUsize = AtomicUsize::new(0);

pub fn start(cx: &mut App) {
    match std::env::var("KOAN_GUI_MEASURE").as_deref() {
        Ok("scroll") => scroll(cx),
        // The album grid flung to its end and back, then the queue, with the
        // process's footprint read at each stage.
        Ok("fling") => fling(cx),
        // Starts playback and leaves the app to run, for its CPU use to be
        // read from outside while it plays with nobody touching it.
        Ok("play") => play(cx),
        // Frames the platform asked for over twenty idle seconds, and how many
        // of them drew anything: the platform's own cost, read from GPUI's
        // journal rather than from the process's CPU time, which moves with
        // whatever else the machine is doing.
        Ok("frames") => frames(cx, false),
        Ok("play-frames") => frames(cx, true),
        // The same with the app hidden, which stops the window's frames.
        Ok("hidden") => cx.hide(),
        Ok("play-hidden") => {
            play(cx);
            cx.hide();
        }
        _ => {}
    }
}

fn frames(cx: &mut App, playing: bool) {
    if playing {
        play(cx);
    }
    let journal = cx.foreground_journal();
    let background = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        background.timer(Duration::from_secs(5)).await;
        let mut collector = journal.collector();
        let visible_at_start = visible(cx);
        background.timer(Duration::from_secs(20)).await;
        let visible_at_end = visible(cx);
        let (mut signals, mut skipped, mut presented) = (0, 0, 0);
        for entry in collector.collect_unseen().entries {
            match entry {
                ForegroundJournalEntry::PlatformSignal(_) => signals += 1,
                ForegroundJournalEntry::Boundary(IntervalBoundary::FrameSkipped(_)) => {
                    skipped += 1
                }
                ForegroundJournalEntry::Boundary(IntervalBoundary::Presented(_)) => {
                    presented += 1
                }
                _ => {}
            }
        }
        println!(
            "frames over 20 s ({}): {signals} asked for, {skipped} with nothing to draw, {presented} drawn; visible {visible_at_start} -> {visible_at_end}",
            if playing { "playing" } else { "paused" },
        );
        cx.update(|cx| cx.quit());
    })
    .detach();
}

fn visible(cx: &mut AsyncApp) -> bool {
    cx.update(|cx| {
        cx.windows().first().is_some_and(|window| {
            window
                .update(cx, |_, window, _| {
                    window.visibility() == WindowVisibility::Visible
                })
                .unwrap_or(false)
        })
    })
}

/// Plays the queue, filling an empty one with the library first, so a run
/// against a fresh configuration has something to play.
fn play(cx: &mut App) {
    let engine = crate::engine::engine(cx);
    let empty = cx
        .global::<crate::mirror::Mirror>()
        .queue
        .read(cx)
        .items
        .is_empty();
    cx.background_executor()
        .spawn(async move {
            if empty {
                let tracks = engine
                    .clone()
                    .random_tracks(100, None)
                    .await
                    .unwrap_or_default();
                let ids = tracks.iter().map(|track| track.id).collect();
                if let Err(e) = engine.clone().replace_queue(ids, None, false).await {
                    log::warn!("measure: queue not filled: {e}");
                }
            } else if let Err(e) = engine.clone().resume().await {
                log::warn!("measure: not resumed: {e}");
            }
        })
        .detach();
}

fn scroll(cx: &mut App) {
    let background = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        background.timer(Duration::from_secs(3)).await;
        let Some(window) = cx.update(|cx| cx.windows().first().copied()) else {
            return;
        };
        let at = centre(window, cx);
        for (direction, pass) in [(-1.0, "down"), (1.0, "up")] {
            for _ in 0..STEPS {
                wheel(window, at, STEP * direction, cx);
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
        frame_times(window, cx);
        cx.update(|cx| cx.quit());
    })
    .detach();
}

/// GPUI's frame histograms for the window so far.
fn frame_times(window: AnyWindowHandle, cx: &mut AsyncApp) {
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
}

/// The window's centre, where the synthetic wheel events land.
fn centre(window: AnyWindowHandle, cx: &mut AsyncApp) -> Point<Pixels> {
    window
        .update(cx, |_, window, _| {
            let size = window.viewport_size();
            point(size.width * 0.6, size.height * 0.5)
        })
        .unwrap_or_default()
}

fn wheel(window: AnyWindowHandle, at: Point<Pixels>, dy: Pixels, cx: &mut AsyncApp) {
    let event = ScrollEvent {
        position: at,
        delta: ScrollDelta::Pixels(point(px(0.), dy)),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    };
    window
        .update(cx, |_, window, cx| {
            window.dispatch_event(PlatformInput::Scroll(event), cx);
        })
        .ok();
}

/// This process's footprint as macOS's `footprint` reports it: what it
/// costs the machine, GPU memory it owns on unified memory included.
fn footprint() -> String {
    let pid = std::process::id().to_string();
    match std::process::Command::new("/usr/bin/footprint")
        .args(["-p", &pid])
        .output()
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout)
            .lines()
            .find(|l| l.contains("Footprint:"))
            .map(|l| l.trim().to_string())
            .unwrap_or_else(|| "footprint: no figure".into()),
        Err(e) => format!("footprint: {e}"),
    }
}

fn fling(cx: &mut App) {
    /// A fling's step: fast enough to cross 5,000 records in seconds.
    const FLING: Pixels = px(240.);
    const FLING_STEPS: usize = 800;
    let background = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        let settle = |secs| background.timer(Duration::from_secs(secs));
        settle(5).await;
        println!("launch: {}", footprint());
        let Some(window) = cx.update(|cx| cx.windows().first().copied()) else {
            return;
        };
        let dispatch = |action: Box<dyn Action>, cx: &mut AsyncApp| {
            window
                .update(cx, |_, window, cx| window.dispatch_action(action, cx))
                .ok();
        };
        dispatch(Box::new(crate::platform::actions::ShowAlbums), cx);
        settle(3).await;
        println!("album grid: {}", footprint());
        let at = centre(window, cx);
        for (direction, pass) in [(-1.0, "end"), (1.0, "start")] {
            for _ in 0..FLING_STEPS {
                wheel(window, at, FLING * direction, cx);
                background.timer(Duration::from_millis(16)).await;
            }
            settle(3).await;
            println!("flung to the {pass}: {}", footprint());
        }
        frame_times(window, cx);
        dispatch(Box::new(crate::platform::actions::ShowQueue), cx);
        settle(5).await;
        println!("back on the queue: {}", footprint());
        cx.update(|cx| cx.quit());
    })
    .detach();
}
