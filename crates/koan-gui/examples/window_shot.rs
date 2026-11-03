//! Draws the app's window offscreen, dark and light side by side, into a
//! PNG: the evidence a change to the shell or the queue is checked against.
//! No window is shown and nothing on the screen is read.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR`. `KOAN_SHOT_TRACKS=n` fills the queue with n of the
//! library's tracks, scanned from that configuration's folders, first, and
//! saves that session. `KOAN_SHOT_ROUTE` (`albums`, `artists`, `tracks`,
//! `favourites`, `recent`, `downloaded`, `album:<id>`, `artist:<id>`,
//! `playlist:<id>`) draws that page instead of the queue. With
//! `KOAN_SHOT_TRACKS`, a playlist of the same tracks is made too, and its id
//! printed.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example window_shot -- target/window.png

use std::error::Error;
use std::time::Duration;

use koan_gui::navigator::Route;
use koan_gui::{app, root::WindowRoot, shell::Shell, theme};
use koan_kit::app::AppContext;
use koan_runtime::KoanEngine;
// GPUI's start-up and headless rendering, which only the app's entry and tools reach.
use koan_kit::raw::{HeadlessAppContext, px, size};

fn route() -> Option<Route> {
    Route::named(&std::env::var("KOAN_SHOT_ROUTE").ok()?)
}

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/window.png".into());

    // A screenshot is no reason to announce a device on the network or take
    // the devices port from a koan already running here.
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    // The desktop app's own device, so a queue saved here is the one the
    // app restores (`KOAN_SHOT_TRACKS`).
    let engine = futures::executor::block_on(KoanEngine::open_desktop())?;
    if let Some(count) = std::env::var("KOAN_SHOT_TRACKS")
        .ok()
        .and_then(|n| n.parse::<usize>().ok())
    {
        futures::executor::block_on(engine.clone().scan_reporting(false, None))?;
        let mut ids: Vec<i64> =
            futures::executor::block_on(engine.clone().random_tracks(100_000, None))?
                .iter()
                .map(|track| track.id)
                .collect();
        ids.sort_unstable();
        let ids: Vec<i64> = ids.iter().cycle().take(count).copied().collect();
        futures::executor::block_on(engine.clone().replace_queue(ids.clone(), None, false))?;
        let playlist =
            futures::executor::block_on(engine.clone().create_playlist("Shot".into(), ids))?;
        println!("playlist {}", playlist.id);
        futures::executor::block_on(engine.clone().pause())?;
        // Kept for the app to restore, which is how frame times are measured
        // against a queue of this size.
        futures::executor::block_on(engine.clone().save_session())?;
    }

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
        koan_gui::library::bind_keys(cx);
    });

    let mut shots = Vec::new();
    for dark in [true, false] {
        let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), |window, cx| {
            let shell = cx.new(|cx| Shell::new(window, cx));
            if let Some(route) = route() {
                shell.update(cx, |shell, cx| shell.show(route, cx));
            }
            let root = cx.new(|cx| WindowRoot::new(shell, window, cx));
            // gpui-component's focus and click extensions, for the shell's own handling.
            cx.new(|cx| koan_kit::raw::base::Root::new(root, window, cx))
        })?;
        cx.update_window(window.into(), |_, window, cx| {
            theme::pin_appearance(Some(dark), window, cx)
        })?;
        // The queue, covers, accent and wash arrive from the background.
        cx.allow_parking();
        for _ in 0..3 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(1100));
            cx.update_window(window.into(), |_, window, _| window.refresh())?;
        }
        cx.run_until_parked();
        shots.push(cx.capture_screenshot(window.into())?);
    }

    let (w, h) = (shots[0].width(), shots[0].height());
    let mut sheet = image::RgbaImage::new(w * 2, h);
    for (index, shot) in shots.iter().enumerate() {
        image::imageops::replace(&mut sheet, shot, (index as u32 * w) as i64, 0);
    }
    sheet.save(&out)?;
    println!("{out}");
    Ok(())
}
