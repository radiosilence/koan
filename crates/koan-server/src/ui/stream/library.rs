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
    /// A warm pass moves the cover cache thousands of times, and each pass
    /// lists the cover directory and counts the library, so at most once a
    /// second.
    const THROTTLE: std::time::Duration = std::time::Duration::from_secs(1);

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The old caches stream drew at most once a second, and a warm pass
    /// moves the cache thousands of times: the Library page's stream keeps
    /// that pace, since each pass lists the cover directory.
    #[test]
    fn the_library_page_draws_at_most_once_a_second() {
        assert!(<Library as Live>::THROTTLE >= std::time::Duration::from_secs(1));
    }
}
