//! kōan on the desktop: macOS and Linux, drawn with GPUI.
//!
//! A thin client over koan-ffi's `KoanEngine`, linked in-process as the Mac
//! app links it, so the UI sits on the audio engine with no daemon between.

/// A key with the platform's command modifier: ⌘ on macOS, Ctrl elsewhere.
/// GPUI's own `secondary-` needs the OS when the binding is parsed, which
/// `KeyBinding::new` does not pass.
macro_rules! secondary {
    ($key:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $key)
        } else {
            concat!("ctrl-", $key)
        }
    };
}

mod covers;
mod engine;
mod mirror;
mod navigator;
mod queue;
mod root;
mod session;
mod shell;
mod sidebar;
mod theme;
mod time;
mod transport;
mod window_state;

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;
use koan_ffi::KoanEngine;

use crate::covers::Covers;
use crate::engine::Engine;
use crate::mirror::{Mirror, Queue};
use crate::root::WindowRoot;
use crate::session::Session;
use crate::shell::Shell;

actions!(koan, [Quit]);

/// How long quitting waits for the session to be written. Writing a large
/// queue takes a moment; a database that never answers must not hold the
/// app open.
const SAVE_ON_QUIT: Duration = Duration::from_secs(5);

fn main() {
    application().run(|cx| {
        gpui_kit::init(cx);
        theme::Theme::follow(cx.window_appearance(), cx);
        cx.bind_keys(shell::bindings());
        cx.bind_keys(queue::bindings());
        cx.bind_keys([KeyBinding::new(secondary!("q"), Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus([Menu::new("kōan").items([MenuItem::action("quit kōan", Quit)])]);

        cx.spawn(async move |cx| {
            let engine = match KoanEngine::open(None).await {
                Ok(engine) => engine,
                Err(e) => {
                    log::error!("the engine did not open: {e}");
                    cx.update(|cx| cx.quit());
                    return;
                }
            };
            let restored = match engine.clone().restore_session().await {
                Ok(count) => Some(count),
                Err(e) => {
                    log::warn!("session not restored: {e}");
                    None
                }
            };
            cx.update(|cx| start(engine, restored, cx));
        })
        .detach();
    });
}

/// How long the window waits for the restored queue to arrive.
const RESTORE_WAIT: Duration = Duration::from_millis(500);

/// Opens the window once the restored queue is in the mirror, so its first
/// frame is the queue that was left rather than an empty one filling in.
/// `restore_session` answers once the player has the queue to load, not once
/// it has loaded it, so the wait is for the mirror to hold `restored` rows,
/// woken by the queue arriving and given up after `RESTORE_WAIT`.
///
/// `restored` is `None` when the session could not be restored. The saved
/// session is shared by every koan app on a configuration directory, so this
/// one then never writes it: it would replace a session it never read.
fn start(engine: Arc<KoanEngine>, restored: Option<u32>, cx: &mut App) {
    let mirror = Mirror::follow(&engine, cx);
    let queue = mirror.queue.clone();
    cx.set_global(mirror);
    cx.set_global(Engine(engine));
    Covers::install(cx);

    let rows = restored.unwrap_or(0) as usize;
    let arrived = move |queue: &Queue| queue.items.len() >= rows;
    let (ready, waiting) = futures::channel::oneshot::channel::<()>();
    let mut ready = Some(ready);
    let watch = (!arrived(queue.read(cx))).then(|| {
        cx.observe(&queue, move |queue, cx| {
            if arrived(queue.read(cx))
                && let Some(ready) = ready.take()
            {
                ready.send(()).ok();
            }
        })
    });
    let timeout = cx.background_executor().timer(RESTORE_WAIT);
    cx.spawn(async move |cx| {
        if watch.is_some() {
            futures::future::select(waiting, timeout).await;
        }
        drop(watch);
        cx.update(|cx| open(restored.is_some(), cx));
    })
    .detach();
}

fn open(keeps_session: bool, cx: &mut App) {
    if keeps_session {
        Session::follow(cx);
    }

    cx.on_app_quit(move |cx| {
        // Inside the closure, not the future it returns: GPUI gives that
        // future a fifth of a second, which a large queue can outlast.
        for window in cx.windows() {
            window
                .update(cx, |_, window, _| {
                    window_state::save(window.window_bounds())
                })
                .ok();
        }
        let engine = engine::engine(cx);
        if keeps_session {
            let executor = cx.foreground_executor().clone();
            if let Err(e) = executor
                .block_with_timeout(SAVE_ON_QUIT, engine.clone().save_session())
                .unwrap_or(Ok(()))
            {
                log::warn!("session not saved on quit: {e}");
            }
        }
        async move {
            if keeps_session {
                engine.save_server_queue_now().await;
            }
        }
    })
    .detach();

    // One window: closing it quits, as Linux would otherwise
    // keep the process (and the music) running with nothing on screen.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();

    let bounds = window_state::restore(cx).unwrap_or_else(|| {
        WindowBounds::Windowed(Bounds::centered(None, size(px(1180.), px(760.)), cx))
    });
    let options = WindowOptions {
        window_bounds: Some(bounds),
        window_min_size: Some(shell::MIN_SIZE),
        titlebar: Some(titlebar()),
        app_owns_titlebar_drag: cfg!(target_os = "macos"),
        // The `.desktop` file's StartupWMClass.
        app_id: Some("koan-gui".into()),
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        window
            .observe_window_appearance(|window, cx| {
                theme::Theme::follow(window.appearance(), cx);
                window.refresh();
            })
            .detach();
        window.on_window_should_close(cx, |window, _| {
            window_state::save(window.window_bounds());
            true
        });
        let shell = cx.new(|cx| Shell::new(window, cx));
        cx.new(|_| WindowRoot::new(shell))
    });
    if let Err(e) = opened {
        log::error!("the window did not open: {e}");
        cx.quit();
    }
    cx.activate(true);
}

/// On macOS the toolbar runs under the window's buttons. Elsewhere the
/// system draws the title bar where the compositor offers one; where it
/// leaves decorations to the app, the toolbar carries the buttons.
fn titlebar() -> TitlebarOptions {
    if cfg!(target_os = "macos") {
        TitlebarOptions {
            title: Some("kōan".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(12.))),
        }
    } else {
        TitlebarOptions {
            title: Some("kōan".into()),
            ..Default::default()
        }
    }
}
