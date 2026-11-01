//! Times the Settings window's frames offscreen: a switch to each tab (a
//! pane made afresh) and scrolling the Server pane. Prints draw times per
//! case.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR`.
//!
//! KOAN_CONFIG_DIR=… cargo run --release -p koan-gui --example settings_frames

use std::error::Error;
use std::time::{Duration, Instant};

use koan_ffi::KoanEngine;
use koan_gui::settings::{SettingsWindow, Tab};
use koan_gui::{app, theme};
use koan_kit::app::*;
// A tool outside the app: GPUI's headless harness and input, driven directly.
use koan_kit::raw::{
    HeadlessAppContext, PlatformInput, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point,
    px, size,
};

const TABS: [Tab; 5] = [
    Tab::Library,
    Tab::Server,
    Tab::Playback,
    Tab::Devices,
    Tab::Appearance,
];

fn report(name: &str, mut times: Vec<Duration>) {
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mean = times.iter().map(|d| ms(*d)).sum::<f64>() / times.len() as f64;
    println!(
        "{name}: {} frames, mean {mean:.2} ms, p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        times.len(),
        ms(times[times.len() / 2]),
        ms(times[times.len() * 95 / 100]),
        ms(*times.last().unwrap()),
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    // geometry and colours computed at runtime, not chosen from the theme.
    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        // geometry and colours computed at runtime, not chosen from the theme.
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        // GPUI's start-up and headless rendering, which only the app's entry and tools reach.
        koan_kit::raw::init(cx);
        theme::init(cx);
        app::install(&engine, cx);
    });
    cx.allow_parking();
    let made = std::rc::Rc::new(std::cell::Cell::new(None));
    let held = made.clone();
    let window = cx.open_window(size(px(900.), px(820.)), move |window, cx| {
        let view = cx.new(|cx| SettingsWindow::new(Tab::Library, window, cx));
        held.set(Some(view.clone()));
        // gpui-component's focus and click extensions, for the shell's own handling.
        cx.new(|cx| koan_kit::raw::base::Root::new(view, window, cx))
    })?;
    let view = made.take().expect("the window was made");
    let window: AnyWindowHandle = window.into();
    for _ in 0..3 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(300));
    }
    cx.run_until_parked();

    // Each switch makes the pane, lays it out and paints it.
    let mut switches = Vec::new();
    for _ in 0..20 {
        for tab in TABS {
            let took = cx.update_window(window, |_, window, cx| {
                view.update(cx, |view, cx| view.choose(tab, window, cx));
                let start = Instant::now();
                window.draw(cx).clear(cx);
                start.elapsed()
            })?;
            switches.push(took);
        }
    }
    report("tab switch", switches);

    // Scrolling the longest pane, a wheel event a frame.
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |view, cx| view.choose(Tab::Server, window, cx));
        window.draw(cx).clear(cx);
    })?;
    let mut scrolls = Vec::new();
    for step in 0..240 {
        let delta = if (step / 60) % 2 == 0 { -12.0 } else { 12.0 };
        let took = cx.update_window(window, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::Scroll(ScrollEvent {
                    position: point(px(450.), px(500.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                    source: ScrollSource::default(),
                }),
                cx,
            );
            let start = Instant::now();
            window.draw(cx).clear(cx);
            start.elapsed()
        })?;
        scrolls.push(took);
    }
    report("scroll", scrolls);

    Ok(())
}
