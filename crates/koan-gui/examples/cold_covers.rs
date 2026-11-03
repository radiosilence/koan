//! How long a screen of records takes to show every sleeve from a cold
//! cache: the album grid in a headless window, its artwork directory emptied
//! first, drawn frame after frame until every tile on screen has its sleeve.
//! Against a local library it measures the pipeline (the disk, decoding, the
//! atlas); against a server's, the fetches too.
//!
//! KOAN_CONFIG_DIR=DIR/config cargo run --release -p koan-gui --example cold_covers

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::time::{Duration, Instant};

use koan_gui::library::{self, albums::AlbumGrid, browse::Browse};
use koan_gui::theme;
use koan_kit::app::{AppContext as _, Entity};
use koan_runtime::KoanEngine;
// GPUI's start-up and headless rendering, which only the app's entry and tools reach.
use koan_kit::raw::{HeadlessAppContext, px, size};

/// 2560 × 1600 points: about a hundred tiles at the grid's sizes.
const WIDTH: f32 = 2560.0;
const HEIGHT: f32 = 1600.0;

fn main() -> Result<(), Box<dyn Error>> {
    koan_core::places::keep_with_config();
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    // A throwaway configuration keeps its artwork in its own cache
    // directory, not beside the config.
    let artwork = koan_core::places::cache().join("artwork");
    std::fs::remove_dir_all(&artwork).ok();
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
        theme::init(cx);
    });
    let lib = cx.update(|cx| {
        // The library pages follow the mirror of the engine's state.
        let mirror = koan_gui::mirror::Mirror::follow(&engine, cx);
        cx.set_global(mirror);
        let navigator = cx.new(|_| koan_gui::navigator::Navigator::default());
        library::Lib::new(engine, Browse::load(), navigator, None, cx)
    });
    let held: Rc<RefCell<Option<Entity<AlbumGrid>>>> = Rc::default();
    let keep = held.clone();
    let start = Instant::now();
    let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), move |window, cx| {
        let grid = cx.new(|cx| AlbumGrid::new(lib, window, cx));
        *keep.borrow_mut() = Some(grid.clone());
        grid
    })?;
    cx.allow_parking();
    let grid = held.borrow().clone().ok_or("the grid opens")?;
    let mut frames = 0u32;
    let listed = loop {
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))?;
        frames += 1;
        let (ready, shown) = cx.update(|cx| {
            let grid = grid.read(cx);
            (grid.ready_on_screen(cx), grid.on_screen())
        });
        if ready && shown > 0 {
            break shown;
        }
        if start.elapsed() > Duration::from_secs(120) {
            return Err("the screen never filled".into());
        }
        std::thread::sleep(Duration::from_millis(4));
    };
    println!(
        "{listed} sleeves on screen from a cold cache in {:.0} ms ({frames} frames)",
        start.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}
