//! Draws the playing bars' shader with known levels through the real
//! renderer, offscreen, into a PNG, and checks each bar stands as high as its
//! level says: proof that the program compiles and the quad lands where it
//! was painted, clipped as it was painted.
//!
//! cargo run -p koan-gui --example shader_shot -- target/shader.png

use std::error::Error;
use std::sync::Arc;

use koan_kit::components::BARS;
use koan_kit::raw::*;

const SIDE: f32 = 120.0;
const LEVELS: [f32; 3] = [1.0, 0.5, 0.25];

struct Bars(Arc<ShaderUniforms>);

impl Render for Bars {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let uniforms = self.0.clone();
        div().size_full().bg(rgb(0x000000)).child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    window.paint_shader(bounds, BARS.gpui(), uniforms.clone())
                },
            )
            .absolute()
            .left(px(10.))
            .top(px(10.))
            .size(px(SIDE)),
        )
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/shader.png".into());
    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(koan_gui::theme::icons::Assets),
        koan_kit::raw::platform::current_headless_renderer,
    );
    let uniforms = Arc::new(ShaderUniforms::new());
    for (i, level) in LEVELS.iter().enumerate() {
        uniforms.set(i, *level);
    }
    // White, bars as wide as their gaps: 20 device pixels each at a scale of 1.
    for (i, v) in [1.0, 1.0, 1.0, 1.0].into_iter().enumerate() {
        uniforms.set(4 + i, v);
    }
    let window = cx.open_window(size(px(140.), px(140.)), {
        let uniforms = uniforms.clone();
        move |window, cx| {
            let scale = window.scale_factor();
            uniforms.set(8, SIDE / 6.0 * scale);
            uniforms.set(9, SIDE / 6.0 * scale);
            uniforms.set(10, 0.0);
            cx.new(|_| Bars(uniforms))
        }
    })?;
    cx.run_until_parked();
    let shot = cx.capture_screenshot(window.into())?;
    shot.save(&out)?;

    // Each bar's column, how far down from the quad's top its first lit pixel
    // is, against the level's share of the quad.
    let scale = shot.width() as f32 / 140.0;
    let top = 10.0 * scale;
    let side = SIDE * scale;
    for (i, level) in LEVELS.iter().enumerate() {
        let x = (10.0 * scale + (i as f32 * 2.0 + 0.5) * side / 6.0) as u32;
        let lit = (0..shot.height()).find(|y| shot.get_pixel(x, *y).0[0] > 128);
        let expected = top + side * (1.0 - level);
        match lit {
            Some(y) if (y as f32 - expected).abs() <= 1.5 => {}
            other => return Err(format!("bar {i}: lit from {other:?}, wanted {expected}").into()),
        }
    }
    println!("{out}: three bars at {LEVELS:?}");
    Ok(())
}
