//! Every storybook page drawn offscreen, dark and light, as
//! `examples/storybook.rs` draws them: none panics, and each draws more than
//! its ground.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{AppContext, HeadlessAppContext, px, size};
use koan_kit::storybook::{Story, Storybook};
use koan_kit::theme;

/// How many distinct colours a page draws: one is a blank ground.
fn colours(shot: &image::RgbaImage) -> usize {
    shot.pixels().map(|p| p.0).collect::<HashSet<_>>().len()
}

// Its own main rather than libtest's harness: the macOS platform must be
// made on the main thread.
fn main() {
    match gpui_platform::current_headless_renderer() {
        Ok(Some(_)) => every_page_draws_in_both_appearances(),
        Ok(None) => println!("storybook: skipped, no headless renderer on this platform"),
        Err(e) => println!("storybook: skipped, no headless renderer: {e}"),
    }
}

fn every_page_draws_in_both_appearances() {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(
        text_system,
        Arc::new(theme::icons::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        theme::init(cx);
    });

    let mut grounds = Vec::new();
    for dark in [true, false] {
        let mut book = None;
        let window = cx
            .open_window(size(px(1280.0), px(1100.0)), |window, cx| {
                let view = cx.new(|cx| Storybook::new(window, cx));
                book = Some(view.clone());
                koan_kit::components::host(view, window, cx)
            })
            .expect("the window");
        let book = book.expect("the storybook");
        cx.update_window(window.into(), |_, window, cx| {
            theme::pin_appearance(Some(dark), window, cx);
        })
        .expect("the window");
        cx.run_until_parked();
        for story in Story::ALL {
            cx.update(|cx| book.update(cx, |book, cx| book.show(story, cx)));
            cx.update_window(window.into(), |_, window, _| window.refresh())
                .expect("the window");
            cx.run_until_parked();
            let shot = cx
                .capture_screenshot(window.into())
                .unwrap_or_else(|e| panic!("{} did not draw: {e}", story.slug()));
            let drawn = colours(&shot);
            assert!(
                drawn > 16,
                "{} ({}) drew {drawn} colours",
                story.slug(),
                if dark { "dark" } else { "light" }
            );
            if story == Story::Type {
                grounds.push(*shot.get_pixel(shot.width() - 2, shot.height() - 2));
            }
        }
    }
    assert_ne!(
        grounds[0], grounds[1],
        "dark and light draw the same ground"
    );
}
