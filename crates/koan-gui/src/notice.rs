//! One line the app says something in, at the sidebar's foot: what became
//! of music moved to another device, or of a command one did not take.
//! A newer notice replaces the one showing; each clears itself after a while.

use std::time::Duration;

use koan_kit::app::*;

use crate::mirror::Mirror;
use crate::theme::Tone;

/// How long a notice stays, as the Mac's toast does.
const SHOWN: Duration = Duration::from_secs(8);

#[derive(Default)]
pub struct Notice {
    pub text: Option<(SharedString, Tone)>,
    /// How many notices have been shown: the line answers each.
    pub said: u64,
    _clear: Option<Task<()>>,
}

impl Global for Notice {}

/// Says `text` in `tone` until a newer notice or the time runs out.
pub fn show(text: impl Into<SharedString>, tone: Tone, cx: &mut App) {
    let clear = cx.spawn(async move |cx| {
        cx.background_executor().timer(SHOWN).await;
        cx.update(|cx| {
            cx.update_global::<Notice, _>(|notice, _| {
                notice.text = None;
                notice._clear = None;
            })
        });
    });
    let said = cx.try_global::<Notice>().map_or(0, |n| n.said) + 1;
    cx.set_global(Notice {
        text: Some((text.into(), tone)),
        said,
        _clear: Some(clear),
    });
}

/// Follows the Connection slice's command notices: a command to another
/// device that was queued for it asleep, refused, or never arrived.
pub fn install(cx: &mut App) {
    cx.set_global(Notice::default());
    let connection = cx.global::<Mirror>().connection.clone();
    let mut last = None;
    cx.observe(&connection, move |connection, cx| {
        let Some(notice) = connection
            .read(cx)
            .0
            .as_ref()
            .and_then(|c| c.command_notice.clone())
        else {
            return;
        };
        if last == Some(notice.seq) {
            return;
        }
        last = Some(notice.seq);
        let device = &notice.device;
        let (text, tone) = if notice.queued {
            (format!("Waiting for {device} to wake"), Tone::Muted)
        } else if notice.refused {
            (
                format!("{device} turned it down: {}", notice.detail),
                Tone::Bad,
            )
        } else if notice.unreached {
            (notice.detail.clone(), Tone::Bad)
        } else {
            (
                format!("{device} couldn't do it: {}", notice.detail),
                Tone::Bad,
            )
        };
        show(text, tone, cx);
    })
    .detach();
}
