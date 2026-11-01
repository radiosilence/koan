//! The one engine, and running its commands off the main thread.

use std::future::Future;
use std::sync::Arc;

use gpui_kit::{App, Global};
use koan_ffi::{KoanEngine, KoanError};

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
