//! Scrolling the album grid over a large library, drawn through GPUI's
//! headless renderer (Metal on macOS: real atlas uploads, real GPU
//! submission, nothing on screen), at the speeds a trackpad scrolls.
//!
//! KOAN_CONFIG_DIR=DIR/config cargo bench -p koan-gui --bench album_grid
//!
//! The window is 4K: 1920 × 1080 points at a scale of 2. `just gui-library DIR`
//! makes such a library. Each frame scrolls by the
//! distance the speed covers in the wall time since the last, so the artwork
//! lanes face the scroll as a person would drive it, however fast the frames
//! come; blank and stand-in tiles are counted alongside GPUI's frame timings.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use gpui::{AppContext as _, BenchAppContext, px};
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

/// The grid on a library, ready to scroll, with its tile counter.
fn grid(
    cx: &mut BenchAppContext,
) -> (
    gpui::Entity<AlbumGrid>,
    Rc<RefCell<Recorder>>,
    gpui::Entity<library::covers::Covers>,
) {
    cx.update(|cx| {
        // GPUI's start-up and headless rendering, which only the app's entry and tools reach.
        koan_kit::raw::init(cx);
        koan_gui::theme::init(cx);
    });
    let engine = engine();
    let recorder = Rc::new(RefCell::new(Recorder::default()));
    let lib = cx.update(|cx| {
        // The library pages follow the mirror of the engine's state.
        let mirror = koan_gui::mirror::Mirror::follow(&engine, cx);
        cx.set_global(mirror);
        let navigator = cx.new(|_| koan_gui::navigator::Navigator::default());
        library::Lib::new(
            engine,
            Browse::load(),
            navigator,
            Some(recorder.clone()),
            cx,
        )
    });
    let mut window = cx.add_empty_window();
    // 4K: 1920 × 1080 points at the test window's scale of 2, 3840 × 2160
    // pixels, the size the Mac grid's figures are quoted at.
    let (viewport, scale) = window.update(|window, _| {
        window.resize(gpui::size(px(1920.0), px(1080.0)));
        (window.viewport_size(), window.scale_factor())
    });
    eprintln!("album_grid window: {viewport:?} at scale {scale}");
    let covers = lib.covers.clone();
    let grid = window
        .update(|window, cx| window.replace_root(cx, |window, cx| AlbumGrid::new(lib, window, cx)));
    // The listing, then the first screen's sleeves.
    cx.run_until(|| {
        cx.read(|cx| grid.read(cx).ready_on_screen(cx))
            .then_some(())
    });
    recorder.borrow_mut().reset();
    (grid, recorder, covers)
}

/// The sleeves held in memory, then again after a second and a half at rest:
/// what a scroll leaves behind once it stops.
fn memory_after(
    covers: &gpui::Entity<library::covers::Covers>,
    cx: &mut BenchAppContext,
) -> String {
    let mb = |cx: &BenchAppContext| cx.read(|cx| covers.read(cx).memory_bytes()) as f64 / 1e6;
    let moving = mb(cx);
    let until = Instant::now() + std::time::Duration::from_millis(1500);
    cx.run_until(|| (Instant::now() >= until).then_some(()));
    format!("sleeves {moving:.0} MB, {:.0} MB at rest", mb(cx))
}

fn scroll_at(points_per_second: f32, cx: &mut BenchAppContext) {
    let (grid, recorder, covers) = grid(cx);
    let mut last = Instant::now();
    cx.bench_renderer(grid, move |grid, _, cx| {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05);
        last = now;
        grid.scroll_by(px(points_per_second * dt), cx);
    });
    let report = recorder.borrow().summary();
    let memory = memory_after(&covers, cx);
    eprintln!("album_grid {points_per_second} pt/s: {report}; {memory}");
}

#[gpui::bench(fps = 144)]
fn album_grid_1000(cx: &mut BenchAppContext) {
    scroll_at(1000.0, cx);
}

#[gpui::bench(fps = 144)]
fn album_grid_3000(cx: &mut BenchAppContext) {
    scroll_at(3000.0, cx);
}

#[gpui::bench(fps = 144)]
fn album_grid_8000(cx: &mut BenchAppContext) {
    scroll_at(8000.0, cx);
}

/// A fast flick: about what a trackpad fling reaches at its start.
#[gpui::bench(fps = 144)]
fn album_grid_16000(cx: &mut BenchAppContext) {
    scroll_at(16000.0, cx);
}

/// A jump from the top to the end and back, a frame each: every row new,
/// every sleeve far from anything ranked ahead.
#[gpui::bench(fps = 144)]
fn album_grid_jump(cx: &mut BenchAppContext) {
    let (grid, recorder, _) = grid(cx);
    let mut to_end = false;
    cx.bench_renderer(grid, move |grid, _, cx| {
        to_end = !to_end;
        grid.jump(to_end, cx);
    });
    eprintln!("album_grid jump: {}", recorder.borrow().summary());
}

gpui::bench_group!(
    benches,
    album_grid_1000,
    album_grid_3000,
    album_grid_8000,
    album_grid_16000,
    album_grid_jump
);
// `gpui::bench_main!`, with a test library required first. Without one
// nothing runs, as under `cargo test`: the benches open a real engine, and
// the real configuration is never theirs to open.
#[global_allocator]
static ALLOCATOR: gpui::CountingAllocator<std::alloc::System> =
    gpui::CountingAllocator::new(std::alloc::System);

fn main() {
    if std::env::var_os("KOAN_CONFIG_DIR").is_none() {
        eprintln!("album_grid: skipped; point KOAN_CONFIG_DIR at a test library's config");
        return;
    }
    benches();
    criterion::Criterion::default()
        .configure_from_args()
        .final_summary();
}
