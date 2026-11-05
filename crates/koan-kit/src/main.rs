//! `cargo run -p koan-kit`: the storybook in a window. Its offscreen PNGs
//! are `cargo run -p koan-kit --example storybook -- DIR`. `KOAN_STORY`
//! opens it on a page by its slug (`rainbow`), which is how `just kit-ios`
//! reaches a page on a simulator nothing taps.

use gpui::{App, AppContext};
use koan_kit::components::SecondaryWindow;
use koan_kit::storybook::{Story, Storybook};
use koan_kit::theme;

fn main() {
    gpui_platform::application()
        .with_assets(theme::icons::Assets)
        .run(|cx: &mut App| {
            theme::init(cx);
            let options = SecondaryWindow::new("kōan storybook", "rocks.koan.storybook")
                .size(1280.0, 900.0)
                .min(720.0, 480.0)
                .options(cx);
            let opened = cx.open_window(options, |window, cx| {
                window.on_window_should_close(cx, |_, cx| {
                    cx.quit();
                    true
                });
                let book = cx.new(|cx| Storybook::new(window, cx));
                let opening = std::env::var("KOAN_STORY").ok();
                if let Some(story) = Story::ALL.into_iter().find(|s| Some(s.slug()) == opening) {
                    book.update(cx, |book, cx| book.show(story, cx));
                }
                koan_kit::components::host(book, window, cx)
            });
            if let Err(e) = opened {
                eprintln!("the storybook's window did not open: {e}");
                cx.quit();
            }
        });
}
