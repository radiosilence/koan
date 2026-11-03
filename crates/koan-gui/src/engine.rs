//! The one engine, and running its commands off the main thread.

use std::future::Future;
use std::sync::Arc;

use koan_ffi::{KoanEngine, KoanError};
use koan_kit::app::{App, AppContext, Global};

pub struct Engine(pub Arc<KoanEngine>);

impl Global for Engine {}

pub fn engine(cx: &App) -> Arc<KoanEngine> {
    cx.global::<Engine>().0.clone()
}

/// Runs a command. Its result comes back through the state stream like every
/// other change, so a caller only hears of failures, which are logged.
///
/// Polled first on the main thread, in the order commands are made: an
/// engine command takes its place on the engine's ordered lane when it is
/// first polled, so a remove and the undo after it reach the player in that
/// order. The work itself runs on the engine's own threads.
pub fn command<F, Fut, T>(cx: &App, what: &'static str, f: F)
where
    F: FnOnce(Arc<KoanEngine>) -> Fut,
    Fut: Future<Output = Result<T, KoanError>> + 'static,
    T: 'static,
{
    let future = f(engine(cx));
    cx.foreground_executor()
        .spawn(async move {
            if let Err(e) = future.await {
                log::warn!("{what}: {e}");
            }
        })
        .detach();
}

/// `command`, with a failure said in the app's notice line as well as the
/// log, as the Mac app's `attempt` shows it: for a choice the person made
/// and is waiting to see happen, such as choosing a device in Play on.
pub fn reported<F, Fut, T>(cx: &App, what: &'static str, f: F)
where
    F: FnOnce(Arc<KoanEngine>) -> Fut,
    Fut: Future<Output = Result<T, KoanError>> + 'static,
    T: 'static,
{
    let future = f(engine(cx));
    cx.spawn(async move |cx| {
        if let Err(e) = future.await {
            log::warn!("{what}: {e}");
            cx.update(|cx| crate::notice::show(e.to_string(), crate::theme::Tone::Bad, cx));
        }
    })
    .detach();
}
