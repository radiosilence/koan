//! The app: the engine opened in-process, as the Mac app links it, so the UI
//! sits on the audio engine with no daemon between; the session restored;
//! one window.

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
use crate::theme;
use crate::{engine, queue, settings, shell, window_state};

actions!(koan, [Quit, OpenSettings]);

/// How long quitting waits for the session to be written. Writing a large
/// queue takes a moment; a database that never answers must not hold the
/// app open.
const SAVE_ON_QUIT: Duration = Duration::from_secs(5);

/// Runs the app: opens the engine, then the window.
pub fn run() {
    application().with_assets(theme::icons::Assets).run(|cx| {
        gpui_kit::init(cx);
        theme::init(cx);
        cx.bind_keys(shell::bindings());
        cx.bind_keys(queue::bindings());
        cx.bind_keys([
            KeyBinding::new(secondary!("q"), Quit, None),
            KeyBinding::new(secondary!(","), OpenSettings, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &OpenSettings, cx| settings::open(cx));
        cx.set_menus([Menu::new("kōan").items([
            MenuItem::action("settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("quit kōan", Quit),
        ])]);

        cx.spawn(async move |cx| {
            let engine = match KoanEngine::open(None).await {
                Ok(engine) => engine,
                Err(e) => {
                    log::error!("the engine did not open: {e}");
                    cx.update(|cx| cx.quit());
                    return;
                }
            };
            cx.update(|cx| start(engine, cx));
        })
        .detach();
    });
}

/// How long the window waits for the restored queue before opening anyway.
const RESTORE_WAIT: Duration = Duration::from_millis(500);
/// How long the session waits for the restored queue to reach the mirror
/// before it starts saving whatever is there.
const RESTORE_LIMIT: Duration = Duration::from_secs(10);

/// The engine and what follows it, as globals every view reads: the mirror
/// of its state and the cover cache.
pub fn install(engine: &Arc<KoanEngine>, cx: &mut App) {
    let mirror = Mirror::follow(engine, cx);
    cx.set_global(mirror);
    cx.set_global(Engine(engine.clone()));
    Covers::install(cx);
    settings::install(engine.clone(), cx);
}

/// Restores the session and opens the window, neither waiting on the other
/// for long.
///
/// `restore_session` answers once the player has the queue to load, and that
/// can take seconds when the queue is kept on a server. The window opens as
/// soon as the restored rows are in the mirror, so its first frame is the
/// queue that was left, and after `RESTORE_WAIT` whatever has happened.
///
/// The saved session is shared by every koan app on a configuration
/// directory, so it is written only by a process that read it: saving starts
/// once the restore has succeeded and its rows have arrived, and never if it
/// failed.
fn start(engine: Arc<KoanEngine>, cx: &mut App) {
    install(&engine, cx);
    settings::resume(cx);
    let queue = cx.global::<Mirror>().queue.clone();

    let (shown, showing) = futures::channel::oneshot::channel::<()>();
    let restore = engine.clone().restore_session();
    cx.spawn(async move |cx| {
        let restored = match restore.await {
            Ok(count) => count,
            // Dropping `shown` opens the window at once.
            Err(e) => {
                log::warn!("session not restored: {e}");
                return;
            }
        };
        // From when the restore answers, not from launch: a restore that
        // takes seconds must not spend the wait for its rows.
        let limit = cx.background_executor().timer(RESTORE_LIMIT);
        let arrived = cx.update(|cx| rows_arrived(&queue, restored as usize, cx));
        futures::future::select(Box::pin(arrived), limit).await;
        shown.send(()).ok();
        cx.update(Session::follow);
    })
    .detach();

    // The window's last bounds are read while the restore runs, off the main
    // thread.
    let saved = cx
        .background_executor()
        .spawn(async { window_state::read() });
    let timeout = cx.background_executor().timer(RESTORE_WAIT);
    cx.spawn(async move |cx| {
        futures::future::select(showing, timeout).await;
        let saved = saved.await;
        cx.update(|cx| open(saved, cx));
    })
    .detach();
}

/// Resolves once the mirror's queue holds `rows` rows, woken by the queue
/// changing rather than by looking.
fn rows_arrived(
    queue: &Entity<Queue>,
    rows: usize,
    cx: &mut App,
) -> impl Future<Output = ()> + use<> {
    let (arrived, arriving) = futures::channel::oneshot::channel::<()>();
    let mut arrived = Some(arrived);
    let mut check = move |queue: &Queue| {
        if queue.items.len() >= rows
            && let Some(arrived) = arrived.take()
        {
            arrived.send(()).ok();
        }
    };
    check(queue.read(cx));
    let watch = cx.observe(queue, move |queue, cx| check(queue.read(cx)));
    async move {
        arriving.await.ok();
        drop(watch);
    }
}

fn open(saved: Option<window_state::Saved>, cx: &mut App) {
    cx.on_app_quit(move |cx| {
        // Inside the closure, not the future it returns: GPUI gives that
        // future a fifth of a second, which a large queue can outlast.
        // The main window's place; the Settings window keeps none.
        for window in cx.windows() {
            if settings::is_settings_window(window, cx) {
                continue;
            }
            window
                .update(cx, |_, window, _| {
                    window_state::write(window.window_bounds())
                })
                .ok();
        }
        let engine = engine::engine(cx);
        let keeps_session = Session::following(cx);
        if keeps_session {
            let executor = cx.foreground_executor().clone();
            match executor.block_with_timeout(SAVE_ON_QUIT, engine.clone().save_session()) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::warn!("session not saved on quit: {e}"),
                Err(_) => log::warn!("session not saved on quit: no answer in {SAVE_ON_QUIT:?}"),
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

    let bounds = match saved {
        Some(saved) => window_state::place(saved, cx),
        None => WindowBounds::Windowed(Bounds::centered(None, size(px(1180.), px(760.)), cx)),
    };
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
        window.on_window_should_close(cx, |window, cx| {
            window_state::save(window.window_bounds(), cx);
            true
        });
        let shell = cx.new(|cx| Shell::new(window, cx));
        cx.new(|cx| WindowRoot::new(shell, window, cx))
    });
    if let Err(e) = opened {
        log::error!("the window did not open: {e}");
        cx.quit();
    }
    cx.activate(true);
    // `--settings` opens Settings beside the window, for checking it
    // without a keyboard.
    if std::env::args().any(|arg| arg == "--settings") {
        settings::open(cx);
    }
    #[cfg(feature = "profiler")]
    crate::measure::start(cx);
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
