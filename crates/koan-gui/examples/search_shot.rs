//! The search box over the window, drawn offscreen against a throwaway
//! library: the box opened as ⌘K opens it, a query typed, and a PNG of the
//! window with nothing lit and another after ↓, dark and light, in a window
//! of ordinary size and a small one. For checking
//! that each kind is capped, that the rows stay inside the panel and that
//! the keys at its foot match what Return does.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example search_shot -- target/search [QUERY]
//! The query defaults to "record", which every record of `just gui-library`
//! matches.

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use koan_gui::{app, root::WindowRoot, shell::Shell, theme};
use koan_kit::app::AppContext;
use koan_runtime::KoanEngine;
// GPUI's start-up, headless rendering and synthetic keys, which only the
// app's entry and tools reach.
use koan_kit::raw::{AnyWindowHandle, HeadlessAppContext, Keystroke, px, size};

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
    std::fs::create_dir_all(&out)?;
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
        koan_kit::raw::init(cx);
        theme::init(cx);
        app::install(&engine, cx);
        koan_gui::library::bind_keys(cx);
        koan_gui::platform::actions::register(&engine, cx);
    });
    for (name, width, height) in SIZES {
        let window = cx.open_window(size(px(width), px(height)), |window, cx| {
            let shell = cx.new(|cx| Shell::new(window, cx));
            let root = cx.new(|cx| WindowRoot::new(shell, window, cx));
            cx.new(|cx| koan_kit::raw::base::Root::new(root, window, cx))
        })?;
        let window: AnyWindowHandle = window.into();
        cx.allow_parking();
        settle(&mut cx, window)?;
        for dark in [true, false] {
            let look = if dark { "dark" } else { "light" };
            cx.update_window(window, |_, window, cx| {
                theme::pin_appearance(Some(dark), window, cx);
                window.dispatch_action(Box::new(koan_gui::platform::actions::Search), cx);
            })?;
            settle(&mut cx, window)?;
            for c in query.chars() {
                // `Keystroke::parse` keeps its text: each character's,
                // leaked for the length of this run.
                key(&mut cx, window, Box::leak(c.to_string().into_boxed_str()))?;
            }
            settle(&mut cx, window)?;
            cx.capture_screenshot(window)?
                .save(out.join(format!("search-{name}-{look}.png")))?;
            key(&mut cx, window, "down")?;
            settle(&mut cx, window)?;
            cx.capture_screenshot(window)?
                .save(out.join(format!("search-{name}-lit-{look}.png")))?;
            // Down to the last row, "All results…", scrolled into view.
            for _ in 0..20 {
                key(&mut cx, window, "down")?;
            }
            settle(&mut cx, window)?;
            cx.capture_screenshot(window)?
                .save(out.join(format!("search-{name}-last-{look}.png")))?;
            // Escape twice: the query emptied, then the box closed.
            key(&mut cx, window, "escape")?;
            key(&mut cx, window, "escape")?;
            settle(&mut cx, window)?;
        }
    }
    println!("PNGs in {}", out.display());
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
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(300));
        cx.update_window(window, |_, window, _| window.refresh())?;
    }
    cx.run_until_parked();
    Ok(())
}
