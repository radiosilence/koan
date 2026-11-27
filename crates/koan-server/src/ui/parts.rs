//! What several pages draw: a track's row, a record's tile, a credit, a
//! heart, the browse toolbar, a shelf's previews, and the transport. Each is
//! one template under `templates/parts/`, so the library, the queue and
//! history draw a track the same way.

use askama::Template;
use koan_kit_web as kit;

use crate::html::{Html, Render};

pub(super) mod browse;
pub(super) mod covers;
pub(super) mod credit;
pub(super) mod heart;
pub(super) mod share;
pub(super) mod shelf;
pub(super) mod tile;
pub(super) mod track;

/// The transport buttons: in the bar, or larger on a phone on the queue
/// page. The player's script presses play while music plays.
#[derive(Template)]
#[template(path = "parts/transport.html")]
pub(crate) struct Transport {
    page: bool,
}

impl Transport {
    pub(crate) fn bar() -> Html {
        Self { page: false }.html()
    }

    pub(crate) fn page() -> Html {
        Self { page: true }.html()
    }

    fn row(&self) -> &'static str {
        if self.page {
            "flex flex-wrap items-center gap-2 max-wide:justify-center"
        } else {
            "flex items-center gap-1.5"
        }
    }

    fn buttons(&self) -> [kit::Button<'static>; 3] {
        let (small, big, prev) = if self.page {
            (
                "size-11 max-wide:size-12",
                "size-11 max-wide:size-15",
                "size-11 max-wide:size-12",
            )
        } else {
            ("size-11", "size-11", "size-11 max-wide:hidden")
        };
        [
            kit::Button::quiet("Previous")
                .icon_only(crate::icon::PREVIOUS)
                .class(prev)
                .data("ctl", "prev"),
            kit::Button::bordered("Play or pause")
                .icon_only(crate::icon::PLAY)
                .toggles(crate::icon::PAUSE, false)
                .class(big)
                .data("ctl", "play"),
            kit::Button::quiet("Next")
                .icon_only(crate::icon::NEXT)
                .class(small)
                .data("ctl", "next"),
        ]
    }
}

/// Position and length around the seek bar.
#[derive(Template)]
#[template(path = "parts/scrub.html")]
pub(crate) struct Scrub {
    place: &'static str,
    height: &'static str,
}

impl Scrub {
    pub(crate) fn bar() -> Html {
        Self {
            place: "max-wide:hidden",
            height: "h-4",
        }
        .html()
    }

    pub(crate) fn page() -> Html {
        Self {
            place: "max-w-form max-wide:mt-2 max-wide:text-meta",
            height: "h-11",
        }
        .html()
    }
}

/// "1 track", "2 tracks".
pub(super) fn count(n: impl Into<u64>, one: &str, many: &str) -> String {
    let n = n.into();
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A date's year, where it starts with one.
pub(super) fn year(date: Option<&str>) -> &str {
    date.and_then(|d| d.get(..4)).unwrap_or("")
}
