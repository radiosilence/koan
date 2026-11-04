//! Times the spinner's frames offscreen: a window holding one spinner, drawn
//! frame after frame as it asks, through GPUI's headless renderer. Prints
//! the draw times, which are the whole of what a spinning frame costs the
//! CPU: the window has nothing else in it.
//!
//! cargo run --release -p koan-kit --example spinner_frames

use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::*;
use koan_kit::components::Spinner;
use koan_kit::theme;

const FRAMES: usize = 1000;

struct One;

impl Render for One {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(Spinner::new(px(24.0)))
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(theme::icons::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        theme::init(cx);
    });
    let window: AnyWindowHandle = cx
        .open_window(size(px(120.0), px(120.0)), |_, cx| cx.new(|_| One))?
        .into();
    cx.allow_parking();
    cx.run_until_parked();
    for _ in 0..60 {
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))?;
    }
    let mut times: Vec<Duration> = Vec::with_capacity(FRAMES);
    for _ in 0..FRAMES {
        let elapsed = cx.update_window(window, |_, window, cx| {
            let start = Instant::now();
            window.draw(cx).clear(cx);
            start.elapsed()
        })?;
        times.push(elapsed);
        // Frames a display apart, so the loop's phase moves as it would.
        std::thread::sleep(Duration::from_millis(7));
    }
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mean = times.iter().map(|d| ms(*d)).sum::<f64>() / FRAMES as f64;
    println!(
        "{FRAMES} spinner frames: draw mean {mean:.3} ms, p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms",
        ms(times[FRAMES / 2]),
        ms(times[FRAMES * 95 / 100]),
        ms(times[FRAMES - 1]),
    );
    Ok(())
}
