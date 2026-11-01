//! Draws the app's window offscreen, dark and light side by side, into a
//! PNG: the evidence a change to the shell or the queue is checked against.
//! No window is shown and nothing on the screen is read.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR`. `KOAN_SHOT_TRACKS=n` fills the queue with n of the
//! library's tracks, scanned from that configuration's folders, first, and
//! saves that session.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --example window_shot -- target/window.png

use std::error::Error;
use std::time::Duration;

use gpui_kit::{AppContext, HeadlessAppContext, px, size};
use koan_ffi::KoanEngine;
use koan_gui::{app, root::WindowRoot, shell::Shell, theme};

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/window.png".into());

    let engine = futures::executor::block_on(KoanEngine::open(None))?;
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
        futures::executor::block_on(engine.clone().replace_queue(ids, None, false))?;
        futures::executor::block_on(engine.clone().pause())?;
        // Kept for the app to restore, which is how frame times are measured
        // against a queue of this size.
        futures::executor::block_on(engine.clone().save_session())?;
    }

    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        app::install(&engine, cx);
    });

    let mut shots = Vec::new();
    for dark in [true, false] {
        let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), |window, cx| {
            let shell = cx.new(|cx| Shell::new(window, cx));
            let root = cx.new(|cx| WindowRoot::new(shell, window, cx));
            cx.new(|cx| gpui_kit::base::Root::new(root, window, cx))
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
