//! `cargo run -p koan-kit`: the storybook in a window. Its offscreen PNGs
//! are `cargo run -p koan-kit --example storybook -- DIR`.

use gpui_kit::{App, AppContext};
use koan_kit::components::SecondaryWindow;
use koan_kit::storybook::Storybook;
use koan_kit::theme;

fn main() {
    gpui_kit::application()
        .with_assets(theme::icons::Assets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            theme::init(cx);
            let options = SecondaryWindow::new("kōan storybook", "rocks.koan.storybook")
                .size(1280.0, 900.0)
                .min(720.0, 480.0)
                .options(cx);
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                window.on_window_should_close(cx, |_, cx| {
                    cx.quit();
                    true
                });
                cx.new(|cx| Storybook::new(window, cx))
            });
            if let Err(e) = opened {
                eprintln!("the storybook's window did not open: {e}");
                cx.quit();
            }
        });
}
