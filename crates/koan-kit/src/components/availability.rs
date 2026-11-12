//! Where a track's file is, or how its download is going: the mark every row
//! that says so draws, right-aligned in its slot so a column of them lines
//! up. The Mac's `AvailabilityMark`.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    StatefulInteractiveElement, Styled, Window, div,
};

use super::{Glide, ProgressRing, SHADED};

/// A running transfer's glide in the colour its ring is drawn in: a glide
/// carries its ring's colour, so a picked row's ring has its own.
pub type RingGlide = Rc<dyn Fn(Hsla, &mut App) -> Glide>;
use crate::theme::{Icon, KoanIcon, KoanStyled, Look, TypeRole, label};

#[derive(Clone)]
pub enum Availability {
    /// Waiting at the front of the downloads: nothing here yet.
    Pending,
    /// Waiting behind others: nothing here yet.
    Queued,
    Failed,
    /// Offline, with no file here: not a failure, and drawn apart from one.
    NotHere,
    /// Arriving: a ring filling as the bytes land, from the transfer's
    /// `Glide` (its share at the last report and its rate), reckoned on the
    /// GPU per present with no frame drawn; turning while the size is
    /// unknown.
    Transferring(RingGlide),
    Stored {
        on_server: bool,
        on_disk: bool,
    },
}

impl Availability {
    /// What the mark means, for the row's tooltip, where it needs saying.
    pub fn help(&self) -> Option<&'static str> {
        match self {
            Availability::Pending => Some("not downloaded yet, next to download"),
            Availability::Queued => Some("not downloaded yet, waiting to download"),
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
///
/// A download waiting to start is the ring a running one fills, empty and
/// faint: the same family as a transfer, and nothing like the kept cloud, so
/// a column of queued tracks is never read as an album on this device.
#[derive(IntoElement)]
pub struct AvailabilityMark {
    state: Option<Availability>,
    picked: bool,
    id: Option<ElementId>,
}

impl AvailabilityMark {
    /// Names the mark, so it says what it means in a tooltip.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// `None` draws an empty slot: a track with no file anywhere.
    pub fn new(state: Option<Availability>) -> Self {
        Self {
            state,
            picked: false,
            id: None,
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
        // The empty ring: the whole circle, faint, nothing filled.
        let waiting = |color: Hsla, word: &'static str| -> AnyElement {
            if look.icons {
                ProgressRing::new(Some(1.0), color).into_any_element()
            } else {
                div().text_color(color).child(word).into_any_element()
            }
        };
        let help = self.state.as_ref().and_then(Availability::help);
        // The file arriving here, seen while the mark shows: a download
        // finishing rings once as its ring gives way to the kept mark.
        // Only a transfer seen running and then kept rings: a rescan or a
        // sync that finds the file here, or a row whose state arrives late,
        // stays still.
        let stage = match self.state {
            Some(Availability::Transferring(_)) => 1,
            Some(Availability::Stored { on_disk: true, .. }) => 2,
            _ => 0,
        };
        let answer = self.id.as_ref().and_then(|id| {
            let answers = super::Answerer::of(id, window, cx);
            if stage == 2 && answers.seen(cx) == Some(1) {
                answers.follow(2, super::Answer::Ring, look.accent.hsla(), window, cx);
            } else {
                answers.note(stage, cx);
            }
            answers.layer(window, cx)
        });
        let inner = self.state.map(|state| match state {
            Availability::Pending => waiting(accent.opacity(0.55), "next"),
            Availability::Queued => waiting(muted.opacity(0.4), "queued"),
            Availability::Failed => mark(Icon::WarningFilled, "failed", bad),
            Availability::NotHere => mark(Icon::CloudMissing, "away", quiet),
            Availability::Transferring(ring) => {
                let glide = ring(muted, cx);
                // Without shaders the ring draws the glide's last value, and
                // the next frame draws it again for as long as it shows.
                if !SHADED {
                    window.request_animation_frame();
                }
                ProgressRing::new(None, muted)
                    .glide(&glide)
                    .into_any_element()
            }
            Availability::Stored { on_server, on_disk } => match (on_server, on_disk) {
                (true, true) => mark(Icon::CloudKept, "kept", ink),
                (true, false) => mark(Icon::Cloud, "server", quiet),
                (false, true) => mark(Icon::OnDevice, "local", ink),
                (false, false) => div().into_any_element(),
            },
        });
        let mark = div()
            .flex()
            .justify_end()
            .text_role(TypeRole::Fine, &look)
            .child(div().relative().children(inner).children(answer));
        match (self.id, help) {
            (Some(id), Some(help)) => mark
                .id(id)
                .tooltip(move |window, cx| super::Tooltip::view(label(help), window, cx))
                .into_any_element(),
            _ => mark.into_any_element(),
        }
    }
}
