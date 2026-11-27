//! The Library page live: the rescan's status as scans start, move and end,
//! whoever started them, and the caches as the warm, a sweep or a scan
//! moves them.

use askama_datastar::Last;

use super::{Changed, Live, Pass, Shown, Topic, last};
use crate::auth::AuthUser;
use crate::share::blocking;
use crate::ui::UiState;
use crate::ui::page::library::{caches, rescan};

pub(in crate::ui) struct Library {
    rescan: Last,
    caches: Last,
}

impl Live for Library {
    fn open(_: &UiState, _: &AuthUser, _: &Shown) -> Self {
        Self {
            rescan: last(),
            caches: last(),
        }
    }

    async fn draw(&mut self, s: &UiState, c: &Changed) -> Pass {
        let mut events = Vec::new();
        if c.topic(Topic::Scans) {
            events.extend(self.rescan.send(&rescan::status(s)));
        }
        if c.topic(Topic::Scans) || c.topic(Topic::Covers) {
            let state = s.clone();
            // Lists the cover directory and reads the library's counts.
            let Some(html) = blocking(move || caches::status(&state)).await else {
                return Pass::Failed;
            };
            events.extend(self.caches.send(&html));
        }
        Pass::Drew(events)
    }
}
