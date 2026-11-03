//! The kōan GUI app on the desktop: the engine, one instance per config
//! directory, and the platform's menus, keys and media controls around a
//! window.

use futures_channel::mpsc;
use futures_util::StreamExt as _;
use koan_gui::{app, platform, theme};
use koan_kit::app::*;
use platform::instance::{self, Launch, Opened};

fn main() {
    platform::logging::init();
    platform::runtime_files::adopt();
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
    let application = koan_kit::app::platform::application().with_assets(theme::icons::Assets);
    // Opened again from the Dock or Finder while running: the window, which
    // kōan kept running with it closed opens again.
    application.on_reopen(platform::to_front);
    application.on_open_urls(move |urls| {
        for next in instance::sort_urls(urls) {
            let _ = opened.unbounded_send(next);
        }
    });
    application.run(|cx| {
        // Read while the launch's Apple event is still the current one.
        platform::residency::launched_at_login();
        #[cfg(target_os = "macos")]
        platform::about::icon_unbundled();
        koan_kit::app::platform::init(cx);
        theme::init(cx);
        quit_on_signal(cx);
        cx.spawn(async move |cx: &mut AsyncApp| {
            let engine = match koan_runtime::KoanEngine::open_desktop().await {
                Ok(engine) => engine,
                // A database from a newer koan, a configuration that does not
                // parse, another desktop app on this configuration that got
                // past the single-instance handover: said in a window, in the
                // engine's words, and in the log. The app does not run
                // without its engine.
                Err(e) => {
                    log::error!("could not open the engine: {e}");
                    cx.background_executor()
                        .spawn(async { log::logger().flush() })
                        .await;
                    let message = e.to_string();
                    cx.update(|cx| platform::startup_error::show(message, opening, cx));
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

/// Ctrl-C or SIGTERM from a terminal quits as ⌘Q does, so the session, the
/// queued settings writes and the window's bounds are saved; a second one
/// while that runs ends the process at once.
fn quit_on_signal(cx: &mut App) {
    let (asked, mut asks) = mpsc::unbounded::<()>();
    let again = std::sync::atomic::AtomicBool::new(false);
    let set = ctrlc::set_handler(move || {
        if again.swap(true, std::sync::atomic::Ordering::SeqCst) {
            std::process::exit(130);
        }
        let _ = asked.unbounded_send(());
    });
    if let Err(e) = set {
        log::warn!("signals: {e}; Ctrl-C ends the app without saving");
        return;
    }
    cx.spawn(async move |cx: &mut AsyncApp| {
        if asks.next().await.is_some() {
            log::info!("quitting on a signal");
            cx.update(|cx| cx.quit());
        }
    })
    .detach();
}
