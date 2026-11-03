//! Draws koan-kit's download ring from a glide at half way, and a seek bar
//! with its downloaded part, through the real renderer offscreen into a PNG,
//! and checks the ring is lit at three o'clock and dark at nine: proof that
//! the ring's program compiles and fills clockwise from twelve.
//!
//! cargo run -p koan-gui --example ring_shot -- target/ring.png

use std::error::Error;
use std::sync::Arc;
use std::time::Instant;

use koan_kit::components::{Glide, ProgressRing, Slider};
use koan_kit::raw::*;
use koan_kit::theme::TypeRole;

/// The ring's side at the scale of `TypeRole::Title` type.
const WIDTH: f32 = 240.0;

struct Shot {
    ring: Glide,
    reach: Glide,
}

impl Render for Shot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x000000))
            .flex()
            .flex_col()
            .p(px(10.))
            .gap(px(10.))
            .child(
                div().child(
                    ProgressRing::new(None, rgb(0xffffff).into())
                        .role(TypeRole::Display)
                        .glide(&self.ring),
                ),
            )
            .child(
                div()
                    .w(px(WIDTH - 20.))
                    .child(Slider::new("seek", 0.25).compact().reach(&self.reach)),
            )
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/ring.png".into());
    let text_system = koan_kit::raw::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(koan_gui::theme::icons::Assets),
        koan_kit::raw::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        // GPUI's start-up and headless rendering, which only the app's entry and tools reach.
        koan_gui::theme::init(cx);
    });
    let (ring, reach) = (Glide::new(), Glide::new());
    ring.set(0.5, 0.0, Instant::now());
    reach.set(0.6, 0.0, Instant::now());
    let window = cx.open_window(size(px(WIDTH), px(120.)), move |_, cx| {
        cx.new(|_| Shot { ring, reach })
    })?;
    cx.run_until_parked();
    let shot = cx.capture_screenshot(window.into())?;
    shot.save(&out)?;

    // The ring's box starts at the padding; at half way its right half is
    // lit, so the rightmost lit column in the top half is its right edge.
    let scale = shot.width() as f32 / WIDTH;
    let pad = 10.0 * scale;
    let lit = |x: f32, y: f32| shot.get_pixel(x as u32, y as u32).0[0] > 128;
    let side = (0..shot.width())
        .rev()
        .find(|x| {
            (0..shot.height() / 2)
                .any(|y| shot.get_pixel(*x, y).0[0] > 128 && *x < (pad as u32 + 200))
        })
        .map(|right| right as f32 - pad)
        .ok_or("nothing drawn")?;
    let centre = pad + side / 2.0;
    let stroke_at = side * 0.06;
    let (right, left) = (
        lit(pad + side - stroke_at, centre),
        lit(pad + stroke_at, centre),
    );
    if !right || left {
        return Err(format!("ring at half: lit at three o'clock {right}, at nine {left}").into());
    }
    println!("{out}: the ring is half filled, clockwise from twelve");
    Ok(())
}
