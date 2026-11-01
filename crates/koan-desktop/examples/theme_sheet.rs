//! Draws the theme sheet offscreen, dark and light side by side, into a PNG:
//! the evidence a change to the theme is checked against. No window is shown
//! and nothing on the screen is read; GPUI's headless renderer draws it.
//!
//! cargo run -p koan-desktop --example theme_sheet -- target/theme-sheet.png

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AppContext, HeadlessAppContext, px, size};
use koan_desktop::{sheet::ThemeSheet, theme};

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 1500.0;

fn main() -> Result<(), Box<dyn Error>> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/theme-sheet.png".into());
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(theme::icons::Assets),
        gpui_kit::platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
    });

    let mut shots = Vec::new();
    for dark in [true, false] {
        let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), |window, cx| {
            let sheet = cx.new(|cx| ThemeSheet::new(window, cx));
            cx.new(|cx| gpui_kit::base::Root::new(sheet, window, cx))
        })?;
        cx.update_window(window.into(), |_, window, cx| {
            theme::pin_appearance(Some(dark), window, cx)
        })?;
        // The sleeve is decoded and baked on the background executor, and the
        // wash dissolves in over two seconds of wall time.
        cx.allow_parking();
        for _ in 0..3 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(1100));
            cx.update_window(window.into(), |_, window, _| window.refresh())?;
        }
        cx.run_until_parked();
        shots.push(cx.capture_screenshot(window.into())?);
    }

    let (w, h) = (shots[0].width(), shots[0].height());
    let mut sheet = image::RgbaImage::new(w * 2, h);
    for (index, shot) in shots.iter().enumerate() {
        image::imageops::replace(&mut sheet, shot, (index as u32 * w) as i64, 0);
    }
    sheet.save(&out)?;
    println!("{out}");
    Ok(())
}
