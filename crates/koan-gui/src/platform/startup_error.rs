//! What the app shows when the engine cannot open (a database from a newer
//! koan, a configuration that does not parse, a locked device): a window
//! saying what failed and what to do, in the engine's own words, as the Mac
//! app's `StartupErrorView` does. Closing it quits.

use koan_kit::app::*;

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
/// is closed.
pub fn show(message: String, cx: &mut App) {
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
    if let Err(e) = cx.open_window(options, |_, cx| cx.new(|_| StartupError { message })) {
        log::error!("could not show the startup error: {e}");
        cx.quit();
    }
    cx.activate(true);
}
