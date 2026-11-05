//! Draws each step of the setup guide offscreen, dark and light, into a PNG
//! apiece: music on this computer and shared, then music on a server. No
//! window is shown and nothing on the screen is read.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR`.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --features runtime-shaders --example setup_shot -- target/setup

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use koan_gui::setup::SetupWindow;
use koan_gui::{app, theme};
use koan_kit::app::AppContext;
use koan_runtime::KoanEngine;
// GPUI's start-up and headless rendering, which only the app's entry and tools reach.
use koan_kit::raw::{HeadlessAppContext, px, size};

const WIDTH: f32 = 760.0;
const HEIGHT: f32 = 760.0;

fn main() -> Result<(), Box<dyn Error>> {
    koan_core::places::keep_with_config();
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/setup".into()),
    );
    std::fs::create_dir_all(&out)?;
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
        theme::init(cx);
        app::install(&engine, cx);
    });
    cx.allow_parking();
    let settle = |cx: &mut HeadlessAppContext| {
        for _ in 0..3 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(400));
        }
        cx.run_until_parked();
    };
    settle(&mut cx);

    for (path, here, share) in [("here", true, true), ("server", false, false)] {
        for dark in [true, false] {
            let mut view = None;
            let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), |window, cx| {
                let setup = cx.new(|cx| SetupWindow::new(window, cx));
                view = Some(setup.clone());
                koan_kit::components::host(setup, window, cx)
            })?;
            let view = view.expect("the guide");
            cx.update_window(window.into(), |_, window, cx| {
                theme::pin_appearance(Some(dark), window, cx)
            })?;
            let appearance = if dark { "dark" } else { "light" };
            let mut index = 0;
            loop {
                settle(&mut cx);
                cx.update_window(window.into(), |_, window, _| window.refresh())?;
                cx.run_until_parked();
                let step = cx.update(|cx| view.read(cx).step());
                let name = step.to_lowercase().replace(' ', "-");
                let file = out.join(format!("setup-{path}-{index}-{name}-{appearance}.png"));
                cx.capture_screenshot(window.into())?.save(&file)?;
                println!("{}", file.display());
                if step == "Finding your way" {
                    break;
                }
                if index == 0 {
                    // The welcome step unchosen, then chosen.
                    view.update(&mut cx, |setup, cx| {
                        setup.choose(here, share);
                        cx.notify();
                    });
                    settle(&mut cx);
                    cx.update_window(window.into(), |_, window, _| window.refresh())?;
                    cx.run_until_parked();
                    let file = out.join(format!("setup-{path}-0-chosen-{appearance}.png"));
                    cx.capture_screenshot(window.into())?.save(&file)?;
                    println!("{}", file.display());
                }
                cx.update_window(window.into(), |_, window, cx| {
                    view.update(cx, |setup, cx| setup.next(window, cx))
                })?;
                index += 1;
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window())?;
        }
    }
    Ok(())
}
