//! What the app shows when the engine cannot open (a database a newer kōan
//! has upgraded, another desktop app holding this configuration, a lock that
//! cannot be taken): a window saying what failed and what to do about it,
//! in the engine's own words, as the Mac app's `StartupErrorView` does.
//! Closing it quits; opening kōan again brings it to the front.

use futures::StreamExt as _;
use futures_channel::mpsc::UnboundedReceiver;
use koan_kit::app::*;

use super::instance::Opened;

use crate::components::{EmptyState, SecondaryWindow};
use crate::theme::{Icon, Look};

struct StartupError {
    message: SharedString,
}

impl Render for StartupError {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        div().size_full().bg(look.palette.bg.hsla()).child(
            EmptyState::new(Icon::Warning, "couldn't open your library")
                .detail(self.message.clone())
                .warning(),
        )
    }
}

/// Opens the window for `message`, the engine's error, and quits once it
/// is closed. Another launch handed to this one (`opened`) brings it to the
/// front: there is nothing else for it to open.
pub fn show(message: String, mut opened: UnboundedReceiver<Opened>, cx: &mut App) {
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    let options = SecondaryWindow::new("kōan", crate::app::APP_ID)
        .size(480.0, 300.0)
        .options(cx);
    let message = message.into();
    match cx.open_window(options, |_, cx| cx.new(|_| StartupError { message })) {
        Ok(window) => {
            cx.spawn(async move |cx| {
                while opened.next().await.is_some() {
                    cx.update(|cx| {
                        window
                            .update(cx, |_, window, _| window.activate_window())
                            .ok();
                        cx.activate(true);
                    });
                }
            })
            .detach();
        }
        Err(e) => {
            log::error!("could not show the startup error: {e}");
            cx.quit();
        }
    }
    cx.activate(true);
}
