//! The app over the engine `main.rs` opened in-process, as the Mac app links
//! it: its state mirrored, the window opened once the restored session is in
//! hand, and the session kept from then on. The platform (`platform::init`)
//! restores the session and saves it on quit.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use koan_kit::app::*;
use koan_runtime::KoanEngine;

use crate::covers::Covers;
use crate::engine::Engine;
use crate::mirror::{Mirror, Queue};
use crate::root::WindowRoot;
use crate::session::Session;
use crate::shell::Shell;
use crate::{queue, settings, window_state};

/// How long the window waits for the restored queue before opening anyway.
const RESTORE_WAIT: Duration = Duration::from_millis(500);
/// How long the session waits for the restored queue to reach the mirror
/// before it starts saving whatever is there.
const RESTORE_LIMIT: Duration = Duration::from_secs(10);

/// The app's name to the desktop on Linux, for every window it opens.
pub const APP_ID: &str = "koan-gui";

/// The engine and what follows it, as globals every view reads: the mirror
/// of its state and the cover cache.
pub fn install(engine: &Arc<KoanEngine>, cx: &mut App) {
    let mirror = Mirror::follow(engine, cx);
    cx.set_global(mirror);
    crate::transfers::install(cx);
    crate::notice::install(cx);
    cx.set_global(Engine(engine.clone()));
    // Reads the engine's appearance and the mirror's playback.
    crate::rainbow::install(cx);
    crate::tv_wake::install(cx);
    Covers::install(cx);
    crate::levels::install(cx);
    crate::platform::dock::follow(engine, cx);
    settings::install(engine.clone(), cx);
    crate::dsp::init(cx);
    // Reads the mirror set up above.
    crate::library::activity::init(cx);
    // Invites and pairing links opened from outside: Settings answers them.
    // Any waiting from the launch itself are answered now.
    cx.observe_global::<crate::platform::Links>(answer_links)
        .detach();
    answer_links(cx);
    #[cfg(target_os = "ios")]
    crate::platform::ios::started(engine.clone(), cx);
}

/// Hands each link waiting to Settings, as an invite or a pairing; anything
/// else is not koan's and is dropped.
fn answer_links(cx: &mut App) {
    // A phone has one window: a link that came with the launch waits for its
    // shell (`open_window`), or Settings would open a window behind it.
    if crate::mobile::phone() && !crate::mobile::shell_open(cx) {
        return;
    }
    for link in crate::platform::Links::take(cx) {
        let engine = crate::engine::engine(cx);
        let incoming = if let Some(pairing) = engine.parse_pairing_link(link.clone()) {
            settings::Incoming::Pairing {
                server: pairing.server,
                id: pairing.id,
            }
        } else if engine.parse_invite(link.clone()).is_some() {
            settings::Incoming::Invite(link)
        } else {
            log::info!("ignoring a link that is not koan's");
            continue;
        };
        settings::answer(incoming, cx);
    }
}

/// Opens the window once the restored session is in the mirror, and keeps
/// the session from then on.
///
/// `restored` is `platform::init`'s restore: the number of tracks restored,
/// or `None` when the session could not be read. It answers once the player
/// has the queue to load, which can take seconds when the queue is kept on a
/// server. The window opens as soon as the restored rows are in the mirror,
/// so its first frame is the queue that was left, and after `RESTORE_WAIT`
/// whatever has happened.
///
/// Saving as it goes starts once the restore has succeeded and its rows have
/// arrived, so the queue it saves is the one it restored. The platform's save
/// on quit is held to the same success: a process that never read the
/// session leaves it as it was.
pub fn start(
    engine: Arc<KoanEngine>,
    restored: impl Future<Output = Option<u32>> + 'static,
    cx: &mut App,
) {
    cx.set_global(MainWindowSlot::default());
    cx.bind_keys(queue::bindings());
    crate::library::bind_keys(cx);
    install(&engine, cx);
    settings::resume(cx);
    cx.on_action(|_: &crate::platform::actions::Settings, cx| settings::open(cx));
    cx.on_action(|_: &crate::platform::actions::SetUp, cx| crate::setup::open(cx));
    let queue = cx.global::<Mirror>().queue.clone();

    let (shown, showing) = futures::channel::oneshot::channel::<()>();
    cx.spawn(async move |cx| {
        // Dropping `shown` opens the window at once.
        let Some(restored) = restored.await else {
            return;
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
    let settings = engine.clone();
    cx.spawn(async move |cx| {
        futures::future::select(showing, timeout).await;
        let saved = saved.await;
        // Opened at login and kept running: the menu bar only, as the Mac
        // app starts then. Otherwise someone opened kōan and wants it.
        let keep = settings.settings().await.devices_keep_running;
        let background = keep && crate::platform::residency::launched_at_login();
        cx.update(|cx| {
            crate::platform::residency::adopt(keep, cx);
            open(saved, !background, cx)
        });
    })
    .detach();
}

/// The main window's handle while it is open, and whether launching has
/// decided whether to open it: nothing opens it again before that, or a
/// file opened at launch would open a second.
#[derive(Default)]
struct MainWindowSlot {
    handle: Option<AnyWindowHandle>,
    /// The shell it draws, for what the app's menus ask of it from any
    /// window.
    shell: Option<WeakEntity<Shell>>,
    ready: bool,
    /// Its saved bounds are being read to open it again: asked again
    /// meanwhile, it is not opened twice.
    opening: bool,
}

impl Global for MainWindowSlot {}

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

/// The main window, opened again where it was left when kōan has been
/// running with it closed, or brought to the front when it is open. Whether
/// it did: before launching has decided about the window, it leaves that to
/// launching.
pub fn show_window(cx: &mut App) -> bool {
    if !cx.try_global::<MainWindowSlot>().is_some_and(|m| m.ready) {
        return false;
    }
    if let Some(window) = main_window(cx) {
        cx.activate(true);
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return true;
    }
    let slot = cx.global_mut::<MainWindowSlot>();
    if std::mem::replace(&mut slot.opening, true) {
        return true;
    }
    let saved = cx
        .background_executor()
        .spawn(async { window_state::read() });
    cx.spawn(async move |cx| {
        let saved = saved.await;
        cx.update(|cx| {
            cx.global_mut::<MainWindowSlot>().opening = false;
            open_window(saved, cx)
        });
    })
    .detach();
    true
}

/// The main window, while it is open. Settings, EQ and About are not it.
/// The main window's shell, while the window is open.
pub fn main_shell(cx: &App) -> Option<Entity<Shell>> {
    main_window(cx)?;
    cx.try_global::<MainWindowSlot>()?.shell.as_ref()?.upgrade()
}

pub fn main_window(cx: &App) -> Option<AnyWindowHandle> {
    let handle = cx.try_global::<MainWindowSlot>()?.handle?;
    cx.windows().contains(&handle).then_some(handle)
}

/// What launching does once the session is in: opens the main window, or
/// with `show` false (a login launch kept running) leaves kōan in the menu
/// bar.
fn open(saved: Option<window_state::Saved>, show: bool, cx: &mut App) {
    cx.on_app_quit(|cx| {
        // Written now: the app is going, and nothing spawned would finish.
        // The main window's place, not another window's.
        if let Some(window) = main_window(cx) {
            window
                .update(cx, |_, window, _| {
                    window_state::write(window.window_bounds())
                })
                .ok();
        }
        async {}
    })
    .detach();
    cx.global_mut::<MainWindowSlot>().ready = true;
    if show {
        open_window(saved, cx);
        // The guide is a window of its own, which a phone has no room for.
        if !crate::mobile::phone() {
            crate::setup::offer_at_launch(cx);
        }
    } else {
        log::info!("opened at login; staying in the menu bar");
        crate::platform::residency::settle_dock(cx);
    }
    // `--settings` opens Settings beside the window, for checking it
    // without a keyboard.
    if std::env::args().any(|arg| arg == "--settings") {
        settings::open(cx);
    }
    #[cfg(feature = "profiler")]
    crate::measure::start(cx);
}

fn open_window(saved: Option<window_state::Saved>, cx: &mut App) {
    let place = saved.map(|saved| window_state::place(saved, cx));
    // The `.desktop` file's StartupWMClass names the app on Linux.
    let opened = crate::components::MainWindow::new("kōan", APP_ID)
        .place(place)
        .min(crate::components::RowMetrics::WINDOW_MIN)
        .buttons_at(crate::components::ToolbarMetrics::WINDOW_BUTTONS_ORIGIN)
        .open(cx, |window, cx| {
            // Closing the last window quits, unless kōan is kept running,
            // and quitting waits for nothing spawned: a file of a few bytes,
            // written now.
            window.on_window_should_close(cx, |window, _| {
                window_state::write(window.window_bounds());
                true
            });
            if crate::mobile::phone() {
                let shell = cx.new(|cx| crate::mobile::PhoneShell::new(window, cx));
                return cx.new(|cx| WindowRoot::new(shell, window, cx));
            }
            let shell = cx.new(|cx| Shell::new(window, cx));
            cx.global_mut::<MainWindowSlot>().shell = Some(shell.downgrade());
            cx.new(|cx| WindowRoot::new(shell, window, cx))
        });
    match opened {
        Ok(handle) => cx.global_mut::<MainWindowSlot>().handle = Some(handle),
        Err(e) => {
            log::error!("the window did not open: {e}");
            cx.quit();
        }
    }
    answer_links(cx);
    crate::platform::residency::window_opening();
    cx.activate(true);
}
