//! The kōan GUI app on the desktop: the engine, one instance per config
//! directory, and the platform's menus, keys and media controls around a
//! window.

use futures_channel::mpsc;
use gpui_kit::*;
use koan_gui::{app, platform, theme};
use platform::instance::{self, Launch, Opened};

fn main() {
    platform::logging::init();
    let paths = instance::argv_paths();
    let (opened, opening) = mpsc::unbounded();
    match instance::claim(&paths) {
        Ok(Launch::Handed) => return,
        Ok(Launch::First(server)) => server.serve(opened.clone()),
        // Running alone is better than not running.
        Err(e) => log::warn!("instance: {e}; not answering later launches"),
    }
    if !paths.is_empty() {
        let _ = opened.unbounded_send(Opened::Paths(paths));
    }

    #[cfg(target_os = "macos")]
    platform::about::name_unbundled();
    let application = gpui_kit::application().with_assets(theme::icons::Assets);
    application.on_open_urls(move |urls| {
        for next in instance::sort_urls(urls) {
            let _ = opened.unbounded_send(next);
        }
    });
    application.run(|cx| {
        #[cfg(target_os = "macos")]
        platform::about::icon_unbundled();
        gpui_kit::init(cx);
        theme::init(cx);
        cx.spawn(async move |cx: &mut AsyncApp| {
            let engine = match koan_ffi::KoanEngine::open_desktop().await {
                Ok(engine) => engine,
                // Refused while another desktop app on this configuration is
                // running, which the single-instance handover makes a launch
                // that got past it. Said on stderr and in the log; the app does
                // not run unlinked.
                Err(e) => {
                    log::error!("could not open the engine: {e}");
                    cx.background_executor()
                        .spawn(async { log::logger().flush() })
                        .await;
                    cx.update(|cx| cx.quit());
                    return;
                }
            };
            cx.update(|cx| {
                // The platform restores the session; the window follows what
                // that resolves to.
                let restored = platform::init(&engine, opening, cx);
                app::start(engine, restored, cx);
            });
        })
        .detach();
    });
}
