//! Where the desktop app meets the operating system: media controls, menus
//! and shortcuts, file drops, one instance per configuration, the log.
//!
//! macOS and Linux. Windows waits on koan-core building there (#1600).

pub mod actions;
pub mod drop;
pub mod instance;
pub mod logging;
pub mod media;
pub mod menus;
pub mod shortcuts_sheet;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_channel::mpsc::UnboundedReceiver;
use futures_util::StreamExt as _;
use gpui_kit::{App, AsyncApp, Global};
use koan_ffi::KoanEngine;

use instance::Opened;

/// How long quitting waits for the session and the server's queue to save.
const QUIT_SAVE_FOR: std::time::Duration = std::time::Duration::from_secs(2);

/// This process restored the saved session, and so may write it back.
static RESTORED: AtomicBool = AtomicBool::new(false);

/// Restore the queue, cursor and position the last session saved, and from
/// then on save them on quit.
///
/// On one configuration directory the desktop app shares a single saved
/// session with the native app, so a process that never read it must not
/// write it: quitting would leave the other app this one's queue, empty or
/// not, in place of its own.
async fn restore_session(engine: &Arc<KoanEngine>) {
    match engine.clone().restore_session().await {
        Ok(_) => RESTORED.store(true, Ordering::Relaxed),
        Err(e) => log::warn!("could not restore the session: {e}"),
    }
}

/// Links the platform asked the app to open that are not files, such as
/// invites and pairings, oldest first. The shell takes them as they arrive
/// (`cx.observe_global::<Links>`) and drains it.
#[derive(Default)]
pub struct Links(pub Vec<String>);

impl Global for Links {}

/// Everything here that runs for the life of the app, started once the
/// engine is open and before the window is. `opened` carries files and links
/// from the command line, a later launch and the platform's open requests;
/// they are taken once the saved session is restored, so a launch's files
/// land after the queue it left rather than before it.
pub fn init(engine: &Arc<KoanEngine>, mut opened: UnboundedReceiver<Opened>, cx: &mut App) {
    actions::register(engine, cx);
    shortcuts_sheet::bind(cx);
    menus::set(cx);
    media::start(engine.clone(), cx);
    // As the Mac app quits: the session and the server's queue first, if this
    // process restored them, for at most two seconds, so a
    // server out of reach does not hold the quit up;
    // then, after that last save, a UPnP renderer is stopped rather than left
    // to play out from a URL this process serves and stall, settings still
    // being written are waited for, and a shared library's Funnel is taken
    // down. All before this returns: GPUI gives the future it returns 200 ms
    // (`SHUTDOWN_TIMEOUT`), then exits however far it got. The window is gone
    // by then and nothing waits on the thread. The log's last lines follow in
    // the future.
    cx.on_app_quit({
        let engine = engine.clone();
        move |cx| {
            if RESTORED.load(Ordering::Relaxed) {
                save_on_leaving(&engine);
            }
            engine.release_output();
            engine.flush_settings();
            engine.release_funnel();
            cx.background_executor()
                .spawn(async { log::logger().flush() })
        }
    })
    .detach();
    let engine = engine.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        restore_session(&engine).await;
        while let Some(next) = opened.next().await {
            cx.update(|cx| {
                to_front(cx);
                match next {
                    Opened::Paths(paths) => drop::queue_files(&engine, paths, cx),
                    Opened::Links(links) => cx.default_global::<Links>().0.extend(links),
                }
            });
        }
    })
    .detach();
}

/// Save the session and the server's queue, waiting at most `QUIT_SAVE_FOR`.
fn save_on_leaving(engine: &Arc<KoanEngine>) {
    let (saved, waiting) = std::sync::mpsc::channel();
    let saving = engine.clone();
    let spawned = std::thread::Builder::new()
        .name("koan-desktop-quit".into())
        .spawn(move || {
            futures_lite::future::block_on(async {
                if let Err(e) = saving.clone().save_session().await {
                    log::warn!("quit: saving the session: {e}");
                }
                saving.save_server_queue_now().await;
            });
            let _ = saved.send(());
        });
    if spawned.is_ok() && waiting.recv_timeout(QUIT_SAVE_FOR).is_err() {
        log::warn!("quit: the session was still saving after {QUIT_SAVE_FOR:?}");
    }
}

/// Bring the app and its window to the front.
pub(crate) fn to_front(cx: &mut App) {
    cx.activate(true);
    if let Some(window) = cx.windows().first() {
        let _ = window.update(cx, |_, window, _| window.activate_window());
    }
}
