//! The search box over the window, drawn offscreen against a throwaway
//! library: the box opened as ⌘K opens it, a query typed, and a PNG of the
//! window with nothing lit and another after ↓, dark and light, in a window
//! of ordinary size and a small one. For checking
//! that each kind is capped, that the rows stay inside the panel and that
//! the keys at its foot match what Return does. In the small window the
//! wheel then scrolls the list (`search-small-wheel-*.png`). Before the PNGs,
//! thirty keystrokes are typed and timed, each one's handling and frame
//! drawn, and then the frame its answer lands in, as p50 and max.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example search_shot -- target/search [QUERY [SIZE [LOOK]]]
//! The query defaults to "record", which every record of `just gui-library`
//! matches.

use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use koan_gui::{app, root::WindowRoot, shell::Shell, theme};
use koan_kit::app::input::{Escape, MoveDown};
use koan_kit::app::{Action, AppContext, Modifiers, ScrollEvent};
use koan_runtime::KoanEngine;
// GPUI's start-up, headless rendering and synthetic keys, which only the
// app's entry and tools reach.
use koan_kit::raw::{
    AnyWindowHandle, HeadlessAppContext, Keystroke, PlatformInput, ScrollDelta, ScrollSource,
    TouchPhase, point, px, size,
};

/// An ordinary window and a small one, where the list must scroll inside
/// the panel.
const SIZES: [(&str, f32, f32); 2] = [("large", 1200.0, 800.0), ("small", 800.0, 600.0)];

fn main() -> Result<(), Box<dyn Error>> {
    koan_core::places::keep_with_config();
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().unwrap_or_else(|| "target/search".into()));
    let query = args.next().unwrap_or_else(|| "record".into());
    // One window and one appearance a run: `large`/`small`, `dark`/`light`.
    let only_size = args.next();
    let only_look = args.next();
    std::fs::create_dir_all(&out)?;
    koan_gui::platform::logging::init();
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    futures::executor::block_on(engine.clone().scan_reporting(false, None))?;

    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        theme::init(cx);
        app::install(&engine, cx);
        koan_gui::library::bind_keys(cx);
        koan_gui::platform::actions::register(&engine, cx);
    });
    for (name, width, height) in SIZES {
        if only_size.as_deref().is_some_and(|s| s != name) {
            continue;
        }
        let window = cx.open_window(size(px(width), px(height)), |window, cx| {
            let shell = cx.new(|cx| Shell::new(window, cx));
            let root = cx.new(|cx| WindowRoot::new(shell, window, cx));
            koan_kit::components::host(root, window, cx)
        })?;
        let window: AnyWindowHandle = window.into();
        cx.allow_parking();
        settle(&mut cx, window)?;
        for dark in [true, false] {
            let look = if dark { "dark" } else { "light" };
            if only_look.as_deref().is_some_and(|l| l != look) {
                continue;
            }
            cx.update_window(window, |_, window, cx| {
                theme::pin_appearance(Some(dark), window, cx);
                window.dispatch_action(Box::new(koan_gui::platform::actions::Search), cx);
            })?;
            settle(&mut cx, window)?;
            if dark {
                time_typing(&mut cx, window, name)?;
                // Emptied again for the query below.
                act(&mut cx, window, Box::new(Escape))?;
                settle(&mut cx, window)?;
            }
            for c in query.chars() {
                // `Keystroke::parse` keeps its text: each character's,
                // leaked for the length of this run.
                key(&mut cx, window, Box::leak(c.to_string().into_boxed_str()))?;
            }
            settle(&mut cx, window)?;
            cx.capture_screenshot(window)?
                .save(out.join(format!("search-{name}-{look}.png")))?;
            act(&mut cx, window, Box::new(MoveDown))?;
            settle(&mut cx, window)?;
            cx.capture_screenshot(window)?
                .save(out.join(format!("search-{name}-lit-{look}.png")))?;
            // Down to the last row, "All results…", scrolled into view.
            for _ in 0..20 {
                act(&mut cx, window, Box::new(MoveDown))?;
            }
            settle(&mut cx, window)?;
            cx.capture_screenshot(window)?
                .save(out.join(format!("search-{name}-last-{look}.png")))?;
            if name == "small" {
                // Back to the top, then the wheel over the middle of the
                // list, as a touchpad's swipe sends it.
                for _ in 0..30 {
                    act(&mut cx, window, Box::new(koan_kit::app::input::MoveUp))?;
                }
                settle(&mut cx, window)?;
                for _ in 0..6 {
                    cx.update_window(window, |_, window, cx| {
                        window.dispatch_event(
                            PlatformInput::Scroll(ScrollEvent {
                                position: point(px(width * 0.5), px(height * 0.5)),
                                delta: ScrollDelta::Pixels(point(px(0.), px(-60.))),
                                modifiers: Modifiers::default(),
                                touch_phase: TouchPhase::Moved,
                                source: ScrollSource::Pointer,
                            }),
                            cx,
                        );
                    })?;
                    cx.run_until_parked();
                }
                settle(&mut cx, window)?;
                cx.capture_screenshot(window)?
                    .save(out.join(format!("search-{name}-wheel-{look}.png")))?;
            }
            // Escape twice: the query emptied, then the box closed.
            act(&mut cx, window, Box::new(Escape))?;
            act(&mut cx, window, Box::new(Escape))?;
            settle(&mut cx, window)?;
        }
        cx.update_window(window, |_, window, _| window.remove_window())?;
        cx.run_until_parked();
    }
    println!("PNGs in {}", out.display());
    Ok(())
}

/// Thirty keystrokes, each timed from its dispatch through the frame drawn
/// after it, and then the frame its answer lands in: as they come while
/// typing, every keystroke after the first over a list of results.
fn time_typing(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    name: &str,
) -> Result<(), Box<dyn Error>> {
    let strokes: Vec<&'static str> = "record 00"
        .chars()
        .map(|c| match c {
            ' ' => "space",
            c => &*Box::leak(c.to_string().into_boxed_str()),
        })
        .chain(std::iter::repeat_n("backspace", 8))
        .chain("ecord 01".chars().map(|c| match c {
            ' ' => "space",
            c => &*Box::leak(c.to_string().into_boxed_str()),
        }))
        .chain(std::iter::repeat_n("backspace", 5))
        .collect();
    let (mut keys, mut answers) = (Vec::new(), Vec::new());
    for stroke in strokes {
        let keystroke = Keystroke::parse(stroke)?;
        keys.push(cx.update_window(window, |_, window, cx| {
            let start = Instant::now();
            window.dispatch_keystroke(keystroke, cx);
            window.draw(cx).clear(cx);
            start.elapsed()
        })?);
        cx.run_until_parked();
        // The settle (on the harness's clock) and the read, then the frame
        // the answer lands in.
        cx.advance_clock(Duration::from_millis(150));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(150));
        cx.run_until_parked();
        answers.push(cx.update_window(window, |_, window, cx| {
            let start = Instant::now();
            window.draw(cx).clear(cx);
            start.elapsed()
        })?);
        cx.run_until_parked();
    }
    let ms = |mut v: Vec<Duration>| {
        v.sort();
        (
            v[v.len() / 2].as_secs_f64() * 1e3,
            v[v.len() - 1].as_secs_f64() * 1e3,
        )
    };
    let (kp, km) = ms(keys);
    let (ap, am) = ms(answers);
    println!(
        "{name}: keystroke p50 {kp:.2} ms, max {km:.2} ms; answer frame p50 {ap:.2} ms, max {am:.2} ms"
    );
    Ok(())
}

/// An action to whatever holds the keyboard, as its key would send it.
fn act(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    action: Box<dyn Action>,
) -> Result<(), Box<dyn Error>> {
    cx.update_window(window, |_, window, cx| window.dispatch_action(action, cx))?;
    cx.run_until_parked();
    Ok(())
}

fn key(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    stroke: &'static str,
) -> Result<(), Box<dyn Error>> {
    let keystroke = Keystroke::parse(stroke)?;
    cx.update_window(window, |_, window, cx| {
        window.dispatch_keystroke(keystroke, cx);
    })?;
    cx.run_until_parked();
    Ok(())
}

/// Lets what arrives from the background land: answers, covers.
fn settle(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Result<(), Box<dyn Error>> {
    for _ in 0..3 {
        // The harness's timers (the search's settle) keep its own clock.
        cx.advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(300));
        cx.update_window(window, |_, window, _| window.refresh())?;
    }
    cx.run_until_parked();
    Ok(())
}
