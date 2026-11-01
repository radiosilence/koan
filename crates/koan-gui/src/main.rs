//! The kōan GUI app: a window over the library pages, opened on the engine as
//! the native apps open it. `KOAN_GUI_SHEET=1` opens the theme sheet instead.

use std::path::PathBuf;

use gpui_kit::*;
use koan_ffi::KoanEngine;
use koan_gui::{library, sheet::ThemeSheet, theme};

/// Whether the configuration `KOAN_CONFIG_DIR` names is safe to open. An
/// unreleased build migrates the library database it opens, after which the
/// released apps refuse it, so it runs only on a configuration of its own.
fn throwaway_config() -> Result<PathBuf, &'static str> {
    let dir = std::env::var_os("KOAN_CONFIG_DIR")
        .map(PathBuf::from)
        .ok_or("set KOAN_CONFIG_DIR to a test library's config: never the real one")?;
    let real = dirs::home_dir().map(|home| home.join(".config").join("koan"));
    let same = |a: &PathBuf, b: &PathBuf| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    };
    if real.as_ref().is_some_and(|real| same(&dir, real)) {
        return Err("KOAN_CONFIG_DIR names the real configuration: use a test library's");
    }
    Ok(dir)
}

fn main() {
    if let Err(problem) = throwaway_config() {
        eprintln!("koan-gui: {problem}");
        std::process::exit(2);
    }
    gpui_kit::application()
        .with_assets(theme::icons::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            theme::init(cx);
            library::bind_keys(cx);
            if std::env::var_os("KOAN_GUI_SHEET").is_some() {
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
                return;
            }
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
                    let lib = library::Lib::new(engine, browse, None, cx);
                    let bounds = Bounds::centered(None, size(px(1280.0), px(860.0)), cx);
                    let options = WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    };
                    match gpui_kit::open_window(options, cx, |window, cx| {
                        cx.new(|cx| library::window::LibraryWindow::new(lib, window, cx))
                    }) {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("koan: no window: {e}");
                            cx.quit();
                        }
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
