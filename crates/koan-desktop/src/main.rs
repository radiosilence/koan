//! koan-desktop. For now it opens the theme sheet; the app's own window
//! replaces it as the shell lands.

use gpui_kit::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use koan_desktop::{sheet::ThemeSheet, theme};

fn main() {
    gpui_kit::application()
        .with_assets(theme::icons::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            let bounds = Bounds::centered(None, size(px(1180.0), px(1000.0)), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| ThemeSheet::new(window, cx)),
            )
            .expect("the theme sheet opens");
            cx.activate(true);
        });
}
