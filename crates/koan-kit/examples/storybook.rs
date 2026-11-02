//! Draws every page of the storybook offscreen, dark and light side by
//! side, one PNG per page: the evidence a change to the theme is checked
//! against. No window is shown and nothing on the screen is read; GPUI's
//! headless renderer draws them. The sample sleeve gives the accent and the
//! wash. A page's name narrows the run to that page.
//!
//! cargo run -p koan-kit --example storybook -- target/storybook [buttons]

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AppContext, HeadlessAppContext, px, size};
use koan_kit::storybook::{Story, Storybook};
use koan_kit::theme;

const WIDTH: f32 = 1280.0;
const HEIGHT: f32 = 1100.0;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().unwrap_or_else(|| "target/storybook".into()));
    let only = args.next();
    std::fs::create_dir_all(&dir)?;
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

    let stories: Vec<Story> = Story::ALL
        .into_iter()
        .filter(|story| only.as_ref().is_none_or(|name| story.slug() == *name))
        .collect();
    if stories.is_empty() {
        let names: Vec<String> = Story::ALL.iter().map(|story| story.slug()).collect();
        return Err(format!("no such page; the pages are {}", names.join(", ")).into());
    }

    let mut shots: Vec<Vec<image::RgbaImage>> = vec![Vec::new(); stories.len()];
    for dark in [true, false] {
        let mut book = None;
        let window = cx.open_window(size(px(WIDTH), px(HEIGHT)), |window, cx| {
            let view = cx.new(|cx| Storybook::new(window, cx));
            book = Some(view.clone());
            cx.new(|cx| gpui_kit::base::Root::new(view, window, cx))
        })?;
        let book = book.expect("the window's view was built");
        cx.update_window(window.into(), |_, window, cx| {
            theme::pin_appearance(Some(dark), window, cx);
            book.update(cx, |book, cx| book.use_sleeve(cx));
        })?;
        // The sleeve is worked out and baked on the background executor, and
        // the wash dissolves in over two seconds of wall time.
        cx.allow_parking();
        for _ in 0..3 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(1100));
            cx.update_window(window.into(), |_, window, _| window.refresh())?;
        }
        for (index, story) in stories.iter().enumerate() {
            cx.update(|cx| book.update(cx, |book, cx| book.show(*story, cx)));
            cx.update_window(window.into(), |_, window, _| window.refresh())?;
            cx.run_until_parked();
            shots[index].push(cx.capture_screenshot(window.into())?);
        }
    }

    for (story, pair) in stories.iter().zip(shots) {
        let (w, h) = (pair[0].width(), pair[0].height());
        let mut page = image::RgbaImage::new(w * 2, h);
        for (index, shot) in pair.iter().enumerate() {
            image::imageops::replace(&mut page, shot, (index as u32 * w) as i64, 0);
        }
        let out = dir.join(format!("{}.png", story.slug()));
        page.save(&out)?;
        println!("{}", out.display());
    }
    Ok(())
}
