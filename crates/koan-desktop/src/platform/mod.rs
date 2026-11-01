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

use futures_channel::mpsc::UnboundedReceiver;
use futures_util::StreamExt as _;
use gpui_kit::{App, AsyncApp, Global};
use koan_ffi::KoanEngine;

use instance::Opened;

/// Links the platform asked the app to open that are not files, such as
/// invites and pairings, oldest first. The shell takes them as they arrive
/// (`cx.observe_global::<Links>`) and drains it.
#[derive(Default)]
pub struct Links(pub Vec<String>);

impl Global for Links {}

/// Everything here that runs for the life of the app, started once the
/// engine is open and before the window is. `opened` carries files and links
/// from the command line, a later launch and the platform's open requests.
pub fn init(engine: &Arc<KoanEngine>, mut opened: UnboundedReceiver<Opened>, cx: &mut App) {
    actions::register(engine, cx);
    shortcuts_sheet::bind(cx);
    menus::set(cx);
    media::start(engine.clone(), cx);
    // Lines the log has not yet written are lost when the process exits.
    cx.on_app_quit(|cx| {
        cx.background_executor()
            .spawn(async { log::logger().flush() })
    })
    .detach();
    let engine = engine.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
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

/// Bring the app and its window to the front.
fn to_front(cx: &mut App) {
    cx.activate(true);
    if let Some(window) = cx.windows().first() {
        let _ = window.update(cx, |_, window, _| window.activate_window());
    }
}
