//! The wash both ways, offscreen, at rest: the CPU's bake (the fallback, and
//! what the wash was before the GPU made it) and the GPU's program from the
//! record's field, for the same sleeve, in both appearances. Writes them side
//! by side into a PNG and prints how far apart they are, per channel.
//!
//! cargo run -p koan-kit --example wash_compare -- target/wash-compare.png

use std::error::Error;
use std::sync::Arc;

use gpui::{
    AppContext, Context, Corners, HeadlessAppContext, IntoElement, ParentElement, Render,
    RenderImage, Styled, Window, canvas, div, point, px, size,
};
use koan_kit::components::{Animated, AnimatedQuad};
use koan_kit::theme::{self, Palette, wash};
use smallvec::SmallVec;

const WIDTH: f32 = 480.0;
const HEIGHT: f32 = 300.0;
/// The wash's overscan at rest (`NEAR` in wash.rs).
const NEAR: f32 = 1.38;

/// A sleeve with something to blur: four colour fields and a bright corner.
fn sleeve() -> wash::Sleeve {
    let side = 128u32;
    let mut rgba = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let pixel = match (x < side / 2, y < side / 2) {
                (true, true) => [200, 40, 30],
                (false, true) => [30, 120, 200],
                (true, false) => [240, 200, 60],
                (false, false) => [20, 90, 50],
            };
            let pixel = if x > 100 && y > 100 {
                [250, 250, 250]
            } else {
                pixel
            };
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
    }
    wash::Sleeve::new(rgba.into(), side, side).expect("a whole sleeve")
}

fn image(baked: image::RgbaImage) -> Arc<RenderImage> {
    Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(baked),
        1,
    )))
}

/// The baked texture where the wash puts it at rest.
struct Baked(Arc<RenderImage>);

impl Render for Baked {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let image = self.0.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let side = w.max(h) * NEAR;
                let centre = bounds.center();
                let square = gpui::Bounds::new(
                    point(centre.x - px(side / 2.0), centre.y - px(side / 2.0)),
                    size(px(side), px(side)),
                );
                window
                    .paint_image(square, square, Corners::default(), image.clone(), 0, false)
                    .ok();
            },
        )
        .size_full()
    }
}

/// The program over the field, at rest, opaque over the ground.
struct Shaded(Arc<RenderImage>, Animated);

impl Render for Shaded {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            AnimatedQuad::new(&wash::WASH, &self.1)
                .image(self.0.clone())
                .size_full(),
        )
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/wash-compare.png".into());
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(theme::icons::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        theme::init(cx);
    });
    let sleeve = sleeve();
    let mut shots = Vec::new();
    for dark in [true, false] {
        let baked = image(wash::bake(&sleeve, dark, 1.0));
        let field = image(wash::field(&sleeve, dark, 1.0));
        let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), move |_, cx| {
            cx.new(|_| Baked(baked))
        })?;
        cx.run_until_parked();
        let before = cx.capture_screenshot(window.into())?;

        let bg = Palette::of(dark).bg.hsla().to_rgb();
        let animated = Animated::new();
        // At rest, whole, laid over the ground: no drift, full opacity.
        animated.set_all(0, &[1.0, 0.0, bg.r, bg.g, bg.b]);
        animated.hold(0, 0.0);
        animated.hold(1, 1.0);
        animated.hold(2, 1.0);
        let shaded = field.clone();
        let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), move |_, cx| {
            cx.new(|_| Shaded(shaded, animated))
        })?;
        cx.run_until_parked();
        let after = cx.capture_screenshot(window.into())?;

        let (mut sum, mut worst, mut n) = (0u64, 0u8, 0u64);
        for (a, b) in before.pixels().zip(after.pixels()) {
            for c in 0..3 {
                let d = a.0[c].abs_diff(b.0[c]);
                sum += d as u64;
                worst = worst.max(d);
                n += 1;
            }
        }
        println!(
            "{}: mean difference {:.2} levels, largest {worst}",
            if dark { "dark" } else { "light" },
            sum as f64 / n as f64
        );
        shots.push((before, after));
    }
    let (w, h) = (shots[0].0.width(), shots[0].0.height());
    let mut sheet = image::RgbaImage::new(w * 2, h * 2);
    for (row, (before, after)) in shots.iter().enumerate() {
        image::imageops::overlay(&mut sheet, before, 0, (row as u32 * h) as i64);
        image::imageops::overlay(&mut sheet, after, w as i64, (row as u32 * h) as i64);
    }
    sheet.save(&out)?;
    println!("{out}: the bake on the left, the program on the right; dark above, light below");
    Ok(())
}
