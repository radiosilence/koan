//! The album grid on a real library, narrowed as the toolbar's filter field
//! narrows it: prints how many records it lists before and after a search.
//!
//! KOAN_CONFIG_DIR=DIR/config cargo run -p koan-gui --example filter_check -- WORDS

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::time::{Duration, Instant};

use koan_ffi::KoanEngine;
use koan_gui::library::{self, albums::AlbumGrid, browse::Browse};
use koan_gui::theme;
use koan_kit::app::{AppContext as _, Entity};
// GPUI's start-up and headless rendering, which only the app's entry and tools reach.
use koan_kit::raw::{HeadlessAppContext, px, size};

fn main() -> Result<(), Box<dyn Error>> {
    koan_core::places::keep_with_config();
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    let words = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Record 0001".into());
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
    });
    let lib = cx.update(|cx| {
        // The library pages follow the mirror of the engine's state.
        let mirror = koan_gui::mirror::Mirror::follow(&engine, cx);
        cx.set_global(mirror);
        let navigator = cx.new(|_| koan_gui::navigator::Navigator::default());
        library::Lib::new(engine, Browse::load(), navigator, None, cx)
    });
    let browse = lib.browse.clone();
    let held: Rc<RefCell<Option<Entity<AlbumGrid>>>> = Rc::default();
    let keep = held.clone();
    let window = cx.open_window(size(px(1280.0), px(800.0)), move |window, cx| {
        let grid = cx.new(|cx| AlbumGrid::new(lib, window, cx));
        *keep.borrow_mut() = Some(grid.clone());
        grid
    })?;
    cx.allow_parking();
    let grid = held.borrow().clone().ok_or("the grid opens")?;
    let settle = |cx: &mut HeadlessAppContext| -> Result<usize, Box<dyn Error>> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))?;
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(cx.update(|cx| grid.read(cx).listed()))
    };
    let before = settle(&mut cx)?;
    cx.update(|cx| browse.update(cx, |b, cx| b.set_search(words.clone(), cx)));
    let after = settle(&mut cx)?;
    println!("listed {before}, then {after} for {words:?}");
    Ok(())
}
