//! kōan's desktop app. For now a window over the library pages, opened on the
//! engine as the native apps open it.

mod library;

use gpui_kit::*;
use koan_ffi::KoanEngine;

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        library::chrome::apply_component_theme(cx);
        library::bind_keys(cx);
        cx.spawn(async move |cx| {
            let engine = match KoanEngine::open(None).await {
                Ok(engine) => engine,
                Err(e) => {
                    eprintln!("koan: the engine would not open: {e}");
                    cx.update(|cx| cx.quit());
                    return;
                }
            };
            let browse = cx
                .background_executor()
                .spawn(async { library::browse::Browse::load() })
                .await;
            cx.update(|cx| {
                let lib = library::Lib::new(engine, browse, cx);
                let bounds = Bounds::centered(None, size(px(1280.0), px(860.0)), cx);
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                };
                if let Err(e) = gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| library::window::LibraryWindow::new(lib, window, cx))
                }) {
                    eprintln!("koan: no window: {e}");
                    cx.quit();
                }
                cx.on_window_closed(|cx, _| {
                    if cx.windows().is_empty() {
                        cx.quit();
                    }
                })
                .detach();
            });
        })
        .detach();
    });
}
