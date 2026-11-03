//! How long a record's page takes from being opened to its rows drawn,
//! headlessly: the real shell and engine on a throwaway configuration, a
//! frame forced every 16 ms, nothing on the screen. Opens 30 records idle,
//! then 30 while a forced rescan writes the library. The forced frames hide
//! a window that fails to ask for its own; `KOAN_GUI_MEASURE=album` in the
//! app, shown and left untouched, is the test that it does.
//!
//! KOAN_CONFIG_DIR=… cargo run -p koan-gui --features profiler,runtime-shaders --example album_latency

use std::error::Error;
use std::time::{Duration, Instant};

use koan_gui::navigator::Route;
use koan_gui::{app, root::WindowRoot, shell::Shell, theme};
use koan_kit::app::{AppContext, Entity};
use koan_runtime::{AlbumSort, KoanEngine};
// GPUI's start-up and headless rendering, which only the app's entry and
// tools reach.
use koan_kit::raw::{AnyWindowHandle, HeadlessAppContext, px, size};

fn main() -> Result<(), Box<dyn Error>> {
    koan_core::places::keep_with_config();
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration".into());
    }
    // SAFETY: set before any other thread starts.
    unsafe {
        std::env::set_var("KOAN_DEVICES__NEARBY", "false");
        std::env::set_var("KOAN_DEVICES__DISCOVERABLE", "false");
    }
    let engine = futures::executor::block_on(KoanEngine::open(None))?;
    let albums = futures::executor::block_on(engine.clone().albums(
        None,
        AlbumSort::Title,
        0,
        None,
        Default::default(),
    ))?;
    let ids: Vec<i64> = albums
        .iter()
        .step_by((albums.len() / 30).max(1))
        .map(|a| a.id)
        .take(30)
        .collect();

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
    });
    let mut shell = None;
    let window = cx.open_window(size(px(1200.), px(750.)), |window, cx| {
        let made = cx.new(|cx| Shell::new(window, cx));
        shell = Some(made.clone());
        let root = cx.new(|cx| WindowRoot::new(made, window, cx));
        cx.new(|cx| koan_kit::raw::base::Root::new(root, window, cx))
    })?;
    let window: AnyWindowHandle = window.into();
    let shell: Entity<Shell> = shell.ok_or("the shell was not made")?;
    cx.allow_parking();
    // The engine's first report, and the grid drawn once.
    frames(&mut cx, window, Duration::from_secs(2))?;

    for load in ["idle", "rescan"] {
        if load == "rescan" {
            let engine = engine.clone();
            std::thread::spawn(move || {
                let _ = futures::executor::block_on(engine.scan_reporting(true, None));
            });
            std::thread::sleep(Duration::from_millis(300));
        }
        let mut took = Vec::new();
        for &id in &ids {
            let asked = Instant::now();
            cx.update(|cx| shell.update(cx, |shell, cx| shell.show(Route::Album(id), cx)));
            let landed = loop {
                if let Some((at_id, at)) = koan_gui::measure::last_drawn()
                    && at_id == id
                    && at >= asked
                {
                    break Some(at - asked);
                }
                if asked.elapsed() > Duration::from_secs(20) {
                    break None;
                }
                frames(&mut cx, window, Duration::from_millis(16))?;
            };
            match landed {
                Some(t) => took.push(t),
                None => println!("{load}: album {id}: no rows in 20 s"),
            }
            cx.update(|cx| shell.update(cx, |shell, cx| shell.show(Route::Albums, cx)));
            frames(&mut cx, window, Duration::from_millis(100))?;
        }
        took.sort();
        let ms = |d: &Duration| d.as_secs_f64() * 1000.0;
        if !took.is_empty() {
            println!(
                "{load}: {} records, rows in: fastest {:.1} ms, median {:.1} ms, p90 {:.1} ms, slowest {:.1} ms",
                took.len(),
                ms(&took[0]),
                ms(&took[took.len() / 2]),
                ms(&took[took.len() * 9 / 10]),
                ms(took.last().unwrap()),
            );
        }
    }
    Ok(())
}

/// Draws a frame every 16 ms for `span`, letting the background land between.
fn frames(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    span: Duration,
) -> Result<(), Box<dyn Error>> {
    let until = Instant::now() + span;
    loop {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })?;
        if Instant::now() >= until {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}
