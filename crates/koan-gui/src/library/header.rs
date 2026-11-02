//! The header a record, an artist or a playlist page shares, as the Mac's:
//! the cover, then the square play and the shuffle beside it, the title with
//! its detail under it, then the lesser verbs as compact buttons with their
//! glyphs: play next, queue, copy share link, favourite.

use koan_ffi::Placement;
use koan_kit::app::*;

use super::drag::Playable;
use super::{Lib, actions};
use crate::components::{Button, ButtonKind, PageHeader, PlayButton, ShuffleButton};
use crate::theme::{Icon, Tone};

/// `title` is data (a record's or an artist's name), kept in its case.
pub fn playable_header(
    lib: &Lib,
    item: Playable,
    title: impl Into<SharedString>,
    cx: &App,
) -> PageHeader {
    let title: SharedString = title.into();
    let favourite = lib.feed.read(cx).is_favourite(item);
    let verb = |id: &'static str, icon: Icon, label: &'static str| {
        Button::new(id, ButtonKind::Compact).icon(icon).label(label)
    };
    let (play, shuffle, next, queue, share, heart) = (
        lib.clone(),
        lib.clone(),
        lib.clone(),
        lib.clone(),
        lib.clone(),
        lib.clone(),
    );
    let name = title.clone();
    PageHeader::data(title)
        .lead(
            PlayButton::new("header-play", "play")
                .on_click(move |_, _, cx| actions::play(&play, &[item], false, cx).detach()),
        )
        .lead(
            ShuffleButton::new("header-shuffle", name.clone())
                .on_click(move |_, _, cx| actions::play(&shuffle, &[item], true, cx).detach()),
        )
        .action(
            verb("header-next", Icon::PlayNext, "play next").on_click(move |_, _, cx| {
                actions::queue(&next, &[item], Placement::Next, cx).detach()
            }),
        )
        .action(
            verb("header-queue", Icon::Queue, "queue").on_click(move |_, _, cx| {
                actions::queue(&queue, &[item], Placement::Append, cx).detach()
            }),
        )
        .action(
            verb("header-share", Icon::Share, "copy share link")
                .on_click(move |_, _, cx| copy_share_link(&share, item, name.to_string(), cx)),
        )
        .action(
            verb(
                "header-favourite",
                if favourite {
                    Icon::Favourited
                } else {
                    Icon::Favourite
                },
                if favourite { "favourited" } else { "favourite" },
            )
            .when(favourite, |b| b.tone(Tone::Accent))
            .on_click(move |_, _, cx| {
                heart
                    .feed
                    .update(cx, |feed, cx| feed.toggle_favourite(item, cx))
            }),
        )
}

/// Asks the server for a public link to `item`'s tracks and copies it:
/// pasting it somewhere is the only thing anyone does with one.
fn copy_share_link(lib: &Lib, item: Playable, name: String, cx: &mut App) {
    let engine = lib.engine.clone();
    cx.spawn(async move |cx| {
        // Said either way, on the app's notice line: a link copied in
        // silence, or not at all, looks the same as a click that missed.
        let said = |text: String, tone: Tone, cx: &mut AsyncApp| {
            cx.update(|cx| crate::notice::show(text, tone, cx));
        };
        let ids = match engine.clone().track_ids_of(vec![item.source()]).await {
            Ok(ids) if !ids.is_empty() => ids,
            Ok(_) => return said(format!("{name} has nothing to share"), Tone::Muted, cx),
            Err(e) => {
                log::warn!("share {name}: {e}");
                return said(format!("Couldn't share {name}: {e}"), Tone::Bad, cx);
            }
        };
        match engine.create_share(ids, Some(name.clone())).await {
            Ok(share) => cx.update(|cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(share.url));
                crate::notice::show(format!("A link to {name} is copied"), Tone::Muted, cx);
            }),
            Err(e) => {
                log::warn!("share {name}: {e}");
                said(format!("Couldn't share {name}: {e}"), Tone::Bad, cx);
            }
        }
    })
    .detach();
}
