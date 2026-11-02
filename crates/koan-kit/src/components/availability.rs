//! Where a track's file is, or how its download is going: the mark every row
//! that says so draws, right-aligned in its slot so a column of them lines
//! up. The Mac's `AvailabilityMark`.

use std::rc::Rc;

use gpui_kit::{AnyElement, App, IntoElement, ParentElement, RenderOnce, Styled, Window, div};

use super::ProgressRing;
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, TypeRole};

/// Reads a running transfer's progress, 0 to 1, or `None` while its size is
/// unknown. Called each frame the mark is drawn, and only then.
pub type ProgressReader = Rc<dyn Fn(&App) -> Option<f32>>;

#[derive(Clone)]
pub enum Availability {
    /// Waiting at the front of the downloads.
    Pending,
    /// Queued behind others.
    Queued,
    Failed,
    /// Offline, with no file here: not a failure, and drawn apart from one.
    NotHere,
    /// Arriving: a ring filling as the bytes land, read from `ProgressReader`
    /// at the display's rate while the mark is on screen; spinning while the
    /// size is unknown.
    Transferring(ProgressReader),
    Stored {
        on_server: bool,
        on_disk: bool,
    },
}

impl Availability {
    /// What the mark means, for the row's tooltip, where it needs saying.
    pub fn help(&self) -> Option<&'static str> {
        match self {
            Availability::Pending => Some("queued for download, next"),
            Availability::Queued => Some("queued for download"),
            Availability::Failed => Some("couldn't be fetched"),
            Availability::NotHere => Some("not on this device"),
            Availability::Transferring(_) => Some("downloading"),
            Availability::Stored { on_server, on_disk } => match (on_server, on_disk) {
                (true, true) => Some("on your server, downloaded"),
                (true, false) => Some("on your server, downloads on play"),
                (false, true) => Some("local file"),
                (false, false) => None,
            },
        }
    }
}

/// `fine`-sized, in `muted` with what is kept here in `ink`, a failure in
/// `bad` and the next download in the accent; a word in place of the glyph
/// when the look hides icons. On a picked row everything but a failure takes
/// `on_selection`.
#[derive(IntoElement)]
pub struct AvailabilityMark {
    state: Option<Availability>,
    picked: bool,
}

impl AvailabilityMark {
    /// `None` draws an empty slot: a track with no file anywhere.
    pub fn new(state: Option<Availability>) -> Self {
        Self {
            state,
            picked: false,
        }
    }

    pub fn picked(mut self, picked: bool) -> Self {
        self.picked = picked;
        self
    }
}

impl RenderOnce for AvailabilityMark {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(window, cx);
        let on = look.palette.on_selection.hsla();
        let pick = |color| if self.picked { on } else { color };
        let ink = pick(look.palette.ink.hsla());
        let muted = pick(look.palette.muted.hsla());
        let quiet = pick(look.palette.muted.hsla().opacity(0.6));
        let accent = pick(look.text_color(crate::theme::Tone::Accent));
        let bad = look.palette.bad.hsla();
        let mark = |icon: Icon, word: &'static str, color| -> AnyElement {
            if look.icons {
                KoanIcon::new(icon)
                    .role(TypeRole::Fine)
                    .color(color)
                    .into_any_element()
            } else {
                div().text_color(color).child(word).into_any_element()
            }
        };
        let inner = self.state.map(|state| match state {
            Availability::Pending => mark(Icon::Downloads, "next", accent),
            Availability::Queued => mark(Icon::Downloads, "queued", muted),
            Availability::Failed => mark(Icon::WarningFilled, "failed", bad),
            Availability::NotHere => mark(Icon::CloudMissing, "away", quiet),
            Availability::Transferring(read) => {
                let progress = read(cx);
                // The bytes land between the row's own redraws: the next
                // frame reads them again, for as long as the mark is drawn.
                if progress.is_some() {
                    window.request_animation_frame();
                }
                ProgressRing::new(progress, muted).into_any_element()
            }
            Availability::Stored { on_server, on_disk } => match (on_server, on_disk) {
                (true, true) => mark(Icon::CloudKept, "kept", ink),
                (true, false) => mark(Icon::Cloud, "server", quiet),
                (false, true) => mark(Icon::OnDevice, "local", ink),
                (false, false) => div().into_any_element(),
            },
        });
        div()
            .flex()
            .justify_end()
            .text_role(TypeRole::Fine, &look)
            .children(inner)
    }
}
