//! What the rainbow appearance costs the GPU per frame: each of its
//! full-window layers alone, then all of them as a window draws them, over
//! a window the size the app is used at (1550 × 900 points at a scale of 2),
//! through GPUI's headless Metal renderer. Prints the median and p95 of each
//! frame's time on the GPU, read from its command buffer.
//!
//! cargo run --release -p koan-kit --example prism_frames

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::*;
use koan_kit::components::Prism;
use koan_kit::theme;

const FRAMES: usize = 300;
const ROUNDS: usize = 10;
const WIDTH: f32 = 1550.0;
const HEIGHT: f32 = 900.0;

#[derive(Clone, Copy, Debug)]
enum Layers {
    Nothing,
    Wash,
    Disco,
    Party,
    Ball,
    All,
}

struct Window_ {
    prism: Prism,
    layers: Layers,
}

impl Render for Window_ {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let layer = |quad: AnyElement| div().absolute().size_full().child(quad);
        let p = &self.prism;
        let quads: Vec<AnyElement> = match self.layers {
            Layers::Nothing => vec![],
            Layers::Wash => vec![p.wash().size_full().into_any_element()],
            Layers::Disco => vec![p.disco().size_full().into_any_element()],
            Layers::Party => vec![p.party().size_full().into_any_element()],
            Layers::Ball => vec![
                div()
                    .absolute()
                    .top(px(52.0))
                    .right(px(24.0))
                    .size(px(72.0))
                    .child(p.ball().size_full())
                    .into_any_element(),
            ],
            Layers::All => vec![
                p.wash().size_full().into_any_element(),
                // The page between the layers under it and over it.
                div().size_full().bg(rgb(0x202020)).into_any_element(),
                p.disco().size_full().into_any_element(),
                p.party().size_full().into_any_element(),
                div()
                    .absolute()
                    .top(px(52.0))
                    .right(px(24.0))
                    .size(px(72.0))
                    .child(p.ball().size_full())
                    .into_any_element(),
            ],
        };
        div()
            .relative()
            .size_full()
            .bg(rgb(0x101010))
            .children(quads.into_iter().map(layer))
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn main() -> Result<(), Box<dyn Error>> {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(theme::icons::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        theme::init(cx);
    });
    let prism = Prism::new();
    prism.style(true, false, 2.0);
    prism.ball_at(60.0, 88.0);
    prism.window(WIDTH, HEIGHT);
    prism.run(true);
    prism.hold_bands([0.7, 0.5, 0.4]);
    let all = [
        Layers::Nothing,
        Layers::Wash,
        Layers::Disco,
        Layers::Party,
        Layers::Ball,
        Layers::All,
    ];
    let mut windows = Vec::new();
    for layers in all {
        let window: AnyWindowHandle = cx
            .open_window(size(px(WIDTH), px(HEIGHT)), {
                let prism = prism.clone();
                move |_, cx| cx.new(|_| Window_ { prism, layers })
            })?
            .into();
        cx.run_until_parked();
        // Warm: the programs compile on their first draw.
        for _ in 0..5 {
            cx.capture_screenshot(window)?;
        }
        windows.push(window);
    }
    let scale = cx.update_window(windows[0], |_, window, _| window.scale_factor())?;
    // Rounds through every layer in turn, so a change in the GPU's clock
    // or load falls on all of them alike.
    let mut times: Vec<Vec<f64>> = vec![Vec::new(); all.len()];
    let mut lit = vec![0usize; all.len()];
    for round in 0..ROUNDS {
        for (k, window) in windows.iter().enumerate() {
            gpui::take_gpu_times();
            gpui::gpu_timing(true);
            for frame in 0..FRAMES / ROUNDS {
                // A beat now and then, as music would bring.
                prism.hear([0.9, 0.6, 0.5], Duration::from_millis(33));
                let shot = cx.capture_screenshot(*window)?;
                // What the layers drew, once: pixels off the window's ground.
                if round == ROUNDS - 1 && frame == 0 {
                    lit[k] = shot
                        .pixels()
                        .filter(|p| p.0[..3].iter().any(|c| c.abs_diff(0x10) > 8))
                        .count();
                }
            }
            gpui::gpu_timing(false);
            times[k].extend(gpui::take_gpu_times().iter().map(|s| s * 1e3));
        }
    }
    println!(
        "{}x{} px, {} frames each",
        (WIDTH * scale) as u32,
        (HEIGHT * scale) as u32,
        FRAMES
    );
    for (k, layers) in all.iter().enumerate() {
        let ms = &mut times[k];
        ms.sort_by(f64::total_cmp);
        if ms.is_empty() {
            println!("{layers:?}: no GPU times (not Metal?)");
            continue;
        }
        println!(
            "{layers:?}: GPU p50 {:.2} ms, p95 {:.2} ms; {} pixels lit",
            percentile(ms, 0.5),
            percentile(ms, 0.95),
            lit[k]
        );
    }
    Ok(())
}
