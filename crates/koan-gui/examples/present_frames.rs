//! What a frame of the app's window costs the GPU at each window size, up
//! to a 4K display and a 6K-backed one: the window drawn whole (a frame the
//! scene changed for) and presented again with only its shaders moving (the
//! wash drifting, a glide), through GPUI's headless Metal renderer. Prints
//! each size's median and p95 GPU time per frame and what the scene holds:
//! its shader layers, which each present composes full screen, and its
//! backdrop blurs. No window is shown and nothing on the screen is read.
//!
//! It opens a real engine, so it refuses to run without a throwaway
//! `KOAN_CONFIG_DIR`; `just gui-present-frames` runs it against the walk's
//! generated library with a queue filled, so the pages run under the bars.
//! `KOAN_PRESENT_SIZE=n` measures the nth size alone (for a Metal System
//! Trace), and `KOAN_PRESENT_LAYERS=1` lists the layers and groups a present
//! composes.

use std::error::Error;
use std::time::Duration;

use koan_gui::{app, root::WindowRoot, shell::Shell, theme};
use koan_kit::app::AppContext;
use koan_runtime::KoanEngine;
// GPUI's headless rendering, its GPU timing and the scene drawn, which only
// tools reach.
use koan_kit::raw::{AnyWindowHandle, HeadlessAppContext, px, size};

/// Window sizes in points, at a scale of 2: the app's usual window, a 4K
/// display filled, and a 6K backing (a 4K panel at "looks like 3360 × 1890").
const SIZES: [(f32, f32); 3] = [(1180.0, 760.0), (1920.0, 1080.0), (3360.0, 1890.0)];
const FRAMES: usize = 120;

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn report(what: &str, mut ms: Vec<f64>) {
    ms.sort_by(f64::total_cmp);
    if ms.is_empty() {
        println!("  {what}: no GPU times (not Metal?)");
        return;
    }
    println!(
        "  {what}: GPU p50 {:.2} ms, p95 {:.2} ms ({} frames)",
        percentile(&ms, 0.5),
        percentile(&ms, 0.95),
        ms.len()
    );
}

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
    let engine = futures::executor::block_on(KoanEngine::open_desktop())?;
    futures::executor::block_on(engine.clone().scan_reporting(false, None))?;
    let ids: Vec<i64> = futures::executor::block_on(engine.clone().random_tracks(400, None))?
        .iter()
        .map(|track| track.id)
        .collect();
    futures::executor::block_on(engine.clone().replace_queue(ids, None, false))?;
    futures::executor::block_on(engine.clone().pause())?;

    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        std::sync::Arc::new(theme::icons::Assets),
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        theme::init(cx);
        app::install(&engine, cx);
        koan_gui::library::bind_keys(cx);
    });

    // `KOAN_PRESENT_SIZE=n` measures only the nth size, for a GPU trace.
    let only = std::env::var("KOAN_PRESENT_SIZE")
        .ok()
        .and_then(|n| n.parse::<usize>().ok());
    for (index, (width, height)) in SIZES.into_iter().enumerate() {
        if only.is_some_and(|only| only != index) {
            continue;
        }
        let window = cx.open_window(size(px(width), px(height)), |window, cx| {
            let shell = cx.new(|cx| Shell::new(window, cx));
            let root = cx.new(|cx| WindowRoot::new(shell, window, cx));
            koan_kit::components::host(root, window, cx)
        })?;
        let window: AnyWindowHandle = window.into();
        cx.update_window(window, |_, window, cx| {
            theme::pin_appearance(Some(true), window, cx)
        })?;
        // The queue, covers, accent and wash arrive from the background.
        cx.allow_parking();
        for _ in 0..3 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(1100));
            cx.update_window(window, |_, window, _| window.refresh())?;
        }
        cx.run_until_parked();
        let (scale, stats) = cx.update_window(window, |_, window, _| {
            let scene = window.rendered_scene();
            if std::env::var_os("KOAN_PRESENT_LAYERS").is_some()
                && let Some(plan) = scene.shader_layers.as_ref()
            {
                for (k, layer) in plan.layers.iter().enumerate() {
                    eprintln!(
                        "layer {k}: {} quads, {} sprites, {} paths, painted {:?}",
                        layer.quads.len(),
                        layer.monochrome_sprites.len() + layer.polychrome_sprites.len(),
                        layer.paths.len(),
                        layer.painted_bounds()
                    );
                }
                for (k, group) in plan.groups.iter().enumerate() {
                    eprintln!(
                        "group {k}: shaders {group:?} {:?}, blur {:?}",
                        scene.shaders[group.clone()]
                            .iter()
                            .map(|s| (s.program.name, s.bounds.size))
                            .collect::<Vec<_>>(),
                        plan.blurs[k]
                    );
                }
            }
            let layers = scene.shader_layers.as_ref().map_or(0, |l| {
                l.layers.iter().filter(|layer| !layer.is_blank()).count()
            });
            (
                window.scale_factor(),
                (layers, scene.shaders.len(), scene.backdrop_blurs.len()),
            )
        })?;
        println!(
            "{}x{} px: {} shader layers drawn, {} shaders, {} backdrop blurs",
            (width * scale) as u32,
            (height * scale) as u32,
            stats.0,
            stats.1,
            stats.2
        );
        // Warm: programs compile and textures grow on the first frames, and
        // two seconds of presents bring the GPU to the clock it keeps under
        // load, rather than the idle one a short burst is measured at.
        for _ in 0..3 {
            cx.capture_screenshot(window)?;
            cx.capture_composed(window, 1)?;
        }
        let warm = std::time::Instant::now();
        while warm.elapsed() < Duration::from_secs(2) {
            cx.capture_composed(window, 30)?;
        }
        koan_kit::raw::take_gpu_times();
        koan_kit::raw::gpu_timing(true);
        for _ in 0..FRAMES {
            cx.capture_screenshot(window)?;
        }
        koan_kit::raw::gpu_timing(false);
        report(
            "drawn whole",
            koan_kit::raw::take_gpu_times()
                .iter()
                .map(|s| s * 1e3)
                .collect(),
        );
        // The layers are held from the warm-up: each present composes them.
        cx.capture_composed(window, 60)?;
        koan_kit::raw::gpu_timing(true);
        cx.capture_composed(window, FRAMES)?;
        koan_kit::raw::gpu_timing(false);
        report(
            "presented again",
            koan_kit::raw::take_gpu_times()
                .iter()
                .map(|s| s * 1e3)
                .collect(),
        );
        cx.update_window(window, |_, window, _| window.remove_window())?;
        cx.run_until_parked();
    }
    Ok(())
}
