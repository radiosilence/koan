//! Where the desktop app meets the operating system: media controls, menus
//! and shortcuts, file drops, one instance per configuration, the log.
//!
//! macOS and Linux. Windows waits on koan-core building there (#1600).

pub mod about;
pub mod actions;
pub mod dock;
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
use koan_ffi::KoanEngine;
use koan_kit::raw::{App, AsyncApp, Global};

use instance::Opened;

/// How long quitting waits for the session and the server's queue to save.
const QUIT_SAVE_FOR: std::time::Duration = std::time::Duration::from_secs(2);

/// Restore the queue, cursor and position the desktop app last saved. The
/// session is the desktop app's own: the Mac app and the terminal UI on the
/// same configuration directory keep theirs.
async fn restore_session(engine: &Arc<KoanEngine>) -> Option<u32> {
    match engine.clone().restore_session().await {
        Ok(count) => Some(count),
        Err(e) => {
            log::warn!("could not restore the session: {e}");
            None
        }
    }
}

/// Links the platform asked the app to open that are not files, such as
/// invites and pairings, oldest first. The shell takes them as they arrive:
/// `cx.observe_global::<Links>` and `Links::take` in the observer.
#[derive(Default)]
pub struct Links(Vec<String>);

impl Global for Links {}

impl Links {
    /// The links waiting, leaving none. Writes the global only when there is
    /// something to take: any write notifies its observers, so an observer
    /// that emptied an empty list would wake itself for ever.
    pub fn take(cx: &mut App) -> Vec<String> {
        if cx.try_global::<Links>().is_none_or(|l| l.0.is_empty()) {
            return Vec::new();
        }
        std::mem::take(&mut cx.global_mut::<Links>().0)
    }
}

/// Everything here that runs for the life of the app, started once the
/// engine is open and before the window is. `opened` carries files and links
/// from the command line, a later launch and the platform's open requests;
/// they are taken once the saved session is restored, so a launch's files
/// land after the queue it left rather than before it.
///
/// Resolves to how many tracks the session restored, or `None` when it could
/// not be read. The player has not necessarily loaded them yet when it does.
/// It can take seconds (with the queue kept on the server, the restore waits
/// for the player), so the window should not wait for it.
/// The shell must not restore the session itself: a second restore appends
/// the saved queue again.
pub fn init(
    engine: &Arc<KoanEngine>,
    mut opened: UnboundedReceiver<Opened>,
    cx: &mut App,
) -> impl Future<Output = Option<u32>> + 'static {
    // As the Mac app does unless it is kept running in the menu bar, which
    // this app has no menu bar item for: closing the last window quits.
    // GPUI's default on macOS keeps the process with nothing to bring back.
    cx.set_quit_mode(koan_kit::raw::QuitMode::LastWindowClosed);
    actions::register(engine, cx);
    about::init(cx);
    shortcuts_sheet::bind(cx);
    menus::set(cx);
    media::start(engine.clone(), cx);
    // As the Mac app quits: the session and the server's queue first, for at
    // most two seconds, so a server out of reach does not hold the quit up;
    // then, after that last save, a UPnP renderer is stopped rather than left
    // to play out from a URL this process serves and stall, settings still
    // being written are waited for, and a shared library's Funnel is taken
    // down. All before this returns: GPUI gives the future it returns 200 ms
    // (`SHUTDOWN_TIMEOUT`), then exits however far it got. The window is gone
    // by then and nothing waits on the thread. The log's last lines follow in
    // the future.
    //
    // Only a session this process restored is saved: one it could not read,
    // or quit before reading, would be replaced by whatever it held instead.
    let restored_ok = Arc::new(AtomicBool::new(false));
    cx.on_app_quit({
        let engine = engine.clone();
        let restored_ok = restored_ok.clone();
        move |cx| {
            if restored_ok.load(Ordering::Acquire) {
                save_on_leaving(&engine);
            } else {
                log::info!("quit: the session was never restored, so it is left as it was");
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
    let (restored, restoring) = futures_channel::oneshot::channel();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let outcome = restore_session(&engine).await;
        if outcome.is_some() {
            restored_ok.store(true, Ordering::Release);
        }
        let _ = restored.send(outcome);
        while let Some(next) = opened.next().await {
            cx.update(|cx| {
                to_front(cx);
                match next {
                    Opened::Paths(paths) => drop::queue_files(&engine, paths, cx),
                    Opened::Links(links) if !links.is_empty() => {
                        cx.default_global::<Links>().0.extend(links)
                    }
                    Opened::Links(_) => {}
                }
            });
        }
    })
    .detach();
    async move { restoring.await.ok().flatten() }
}

/// Save the session and the server's queue, waiting at most `QUIT_SAVE_FOR`.
fn save_on_leaving(engine: &Arc<KoanEngine>) {
    let (saved, waiting) = std::sync::mpsc::channel();
    let saving = engine.clone();
    let spawned = std::thread::Builder::new()
        .name("koan-gui-quit".into())
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
