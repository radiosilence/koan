//! Scrolling the album grid over a large library, drawn through GPUI's
//! headless renderer (Metal on macOS: real atlas uploads, real GPU
//! submission, nothing on screen), at the speeds a trackpad scrolls.
//!
//! KOAN_CONFIG_DIR=DIR/config cargo bench -p koan-gui --bench album_grid
//!
//! `just gui-library DIR` makes such a library. Each frame scrolls by the
//! distance the speed covers in the wall time since the last, so the artwork
//! lanes face the scroll as a person would drive it, however fast the frames
//! come; blank and stand-in tiles are counted alongside GPUI's frame timings.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use gpui::{BenchAppContext, px};
use koan_ffi::KoanEngine;
use koan_gui::library::{self, albums::AlbumGrid, bench::Recorder, browse::Browse};

/// One engine for the process, as the apps have.
fn engine() -> Arc<KoanEngine> {
    static ENGINE: OnceLock<Arc<KoanEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            futures::executor::block_on(KoanEngine::open(None)).expect("the engine opens")
        })
        .clone()
}

fn scroll_at(points_per_second: f32, cx: &mut BenchAppContext) {
    assert!(
        std::env::var_os("KOAN_CONFIG_DIR").is_some(),
        "point KOAN_CONFIG_DIR at a test library's config: never the real one"
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        koan_gui::theme::init(cx);
    });
    let engine = engine();
    let recorder = Rc::new(RefCell::new(Recorder::default()));
    let lib = cx.update(|cx| library::Lib::new(engine, Browse::load(), Some(recorder.clone()), cx));
    let mut window = cx.add_empty_window();
    let grid = window
        .update(|window, cx| window.replace_root(cx, |window, cx| AlbumGrid::new(lib, window, cx)));
    // The listing, then the first screen's sleeves.
    cx.run_until(|| {
        cx.read(|cx| grid.read(cx).ready_on_screen(cx))
            .then_some(())
    });

    let mut last = Instant::now();
    cx.bench_renderer(grid, move |grid, _, cx| {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05);
        last = now;
        grid.scroll_by(px(points_per_second * dt), cx);
    });
    let report = recorder.borrow().summary();
    eprintln!("album_grid {points_per_second} pt/s: {report}");
}

#[gpui::bench]
fn album_grid_1000(cx: &mut BenchAppContext) {
    scroll_at(1000.0, cx);
}

#[gpui::bench]
fn album_grid_3000(cx: &mut BenchAppContext) {
    scroll_at(3000.0, cx);
}

#[gpui::bench]
fn album_grid_8000(cx: &mut BenchAppContext) {
    scroll_at(8000.0, cx);
}

gpui::bench_group!(benches, album_grid_1000, album_grid_3000, album_grid_8000);
gpui::bench_main!(benches);
