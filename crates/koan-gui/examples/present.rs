//! The app on screen, presenting, while the window is pushed through
//! everything that makes the main thread draw synchronously or take a
//! window's display link away: resizes, key changes as Settings opens and
//! the main window comes forward again, minimising (occlusion), and closing
//! and reopening both windows. The queue plays all the while, so the
//! transport and the playing bars keep the display link presenting.
//! Exits non-zero on a panic or an uncaught exception, which is what it is
//! for: crashes between the present thread and the main thread
//! (radiosilence/zed#4) that no headless run reaches.
//!
//! Opens real windows and brings them forward, so it is for a machine
//! nobody is using: CI's macOS runner. It refuses a configuration that is
//! not a throwaway one, as `smoke` does. `just gui-present` prepares one and
//! runs it.

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use koan_gui::{app, settings, theme};
use koan_kit::app::*;
use koan_runtime::KoanEngine;
// The window's size, which the app's own code never sets.
use koan_kit::raw::size;

const ROUNDS: usize = 12;
const BUDGET: Duration = Duration::from_secs(600);
/// Long enough between steps for the display link to present several
/// frames, so each change lands while it is running.
const BEAT: Duration = Duration::from_millis(250);

fn main() -> Result<(), Box<dyn Error>> {
    koan_core::places::keep_with_config();
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    throwaway()?;
    std::thread::spawn(|| {
        std::thread::sleep(BUDGET);
        eprintln!("present: still running after {BUDGET:?}: hung");
        std::process::exit(124);
    });
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        std::process::exit(101);
    }));

    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    prepare(&engine)?;
    let application = koan_kit::app::platform::application().with_assets(theme::icons::Assets);
    application.run(move |cx| {
        theme::init(cx);
        // No session to restore: the window opens at once.
        app::start(engine, async { None }, cx);
        cx.spawn(async move |cx: &mut AsyncApp| {
            if let Err(e) = churn(cx).await {
                eprintln!("present: {e}");
                std::process::exit(1);
            }
            println!("no panics");
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    Ok(())
}

async fn churn(cx: &mut AsyncApp) -> Result<(), Box<dyn Error>> {
    let beat = |cx: &mut AsyncApp| cx.background_executor().timer(BEAT);
    for _ in 0..40 {
        if cx.update(|cx| app::main_window(cx)).is_some() {
            break;
        }
        beat(cx).await;
    }
    let sizes = [
        (1200.0, 750.0),
        (900.0, 600.0),
        (1440.0, 900.0),
        (1000.0, 700.0),
    ];
    for round in 0..ROUNDS {
        let main = cx
            .update(|cx| app::main_window(cx))
            .ok_or("the main window did not open")?;
        // Resizes, each drawn synchronously, while the link presents.
        for (width, height) in sizes {
            main.update(cx, |_, window, _| {
                window.resize(size(px(width), px(height)))
            })?;
            beat(cx).await;
        }
        // Key moves to Settings and back.
        cx.update(settings::open);
        beat(cx).await;
        main.update(cx, |_, window, _| window.activate_window())?;
        beat(cx).await;
        // Minimised, and occluded with it, then brought back.
        main.update(cx, |_, window, _| window.minimize_window())?;
        beat(cx).await;
        beat(cx).await;
        main.update(cx, |_, window, _| window.activate_window())?;
        beat(cx).await;
        // Settings closed; every other round the main window too, opened
        // again where it was.
        let closing: Vec<AnyWindowHandle> = cx.update(|cx| {
            cx.windows()
                .into_iter()
                .filter(|w| round % 2 == 1 || *w != main)
                .collect()
        });
        for window in closing {
            window
                .update(cx, |_, window, _| window.remove_window())
                .ok();
            beat(cx).await;
        }
        if round % 2 == 1 {
            cx.update(app::show_window);
            beat(cx).await;
        }
        println!("round {}", round + 1);
    }
    Ok(())
}

/// Refuses any configuration but a throwaway one: not the person's own,
/// muted, and holding only generated libraries.
fn throwaway() -> Result<(), Box<dyn Error>> {
    let Some(dir) = std::env::var_os("KOAN_CONFIG_DIR") else {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration (just gui-present)".into());
    };
    let dir = std::fs::canonicalize(&dir)?;
    let own = std::env::var_os("HOME")
        .map(|home| std::path::Path::new(&home).join(".config/koan"))
        .and_then(|own| std::fs::canonicalize(own).ok());
    if own.as_deref() == Some(dir.as_path()) {
        return Err("KOAN_CONFIG_DIR is your own configuration".into());
    }
    let config = koan_core::config::Config::load()?;
    if !config.playback.muted {
        return Err("the configuration plays aloud: set [playback] muted = true".into());
    }
    if config.library.folders.is_empty() {
        return Err("the configuration has no library: run `just gui-library`".into());
    }
    for folder in &config.library.folders {
        if !folder.join(".koan-generated").exists() {
            return Err(format!(
                "{} is not a library `just gui-library` generated",
                folder.display()
            )
            .into());
        }
    }
    Ok(())
}

/// Scans the library and plays it: the transport and the playing bars are
/// what keep the display link presenting.
fn prepare(engine: &Arc<KoanEngine>) -> Result<(), Box<dyn Error>> {
    use futures::executor::block_on;
    block_on(engine.clone().scan_reporting(false, None))?;
    let tracks = block_on(engine.clone().random_tracks(600, None))?;
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    if ids.is_empty() {
        return Err("the library is empty: run `just gui-library` into the config".into());
    }
    block_on(engine.clone().replace_queue(ids, Some(0), false))?;
    Ok(())
}
