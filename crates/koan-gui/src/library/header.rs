//! The header a record, an artist or a playlist page shares, as the Mac's:
//! the cover, then the square play and the shuffle beside it, the title with
//! its detail under it, then the lesser verbs as compact buttons with their
//! glyphs: play next, queue, copy share link, favourite.

use koan_kit::app::*;
use std::sync::Arc;

use koan_runtime::Placement;

use super::drag::Playable;
use super::{Lib, actions};
use crate::components::{
    Button, ButtonKind, KoanContextMenu, PageHeader, PlayButton, ShuffleButton,
};
use crate::theme::{Icon, TypeRole};

/// `title` is data (a record's or an artist's name), kept in its case.
pub fn playable_header(
    lib: &Lib,
    item: Playable,
    title: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> PageHeader {
    let title: SharedString = title.into();
    let favourite = lib.feed.read(cx).is_favourite(item);
    let share_unavailable = actions::share_links_unavailable(cx);
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
    let header = PageHeader::data(title)
        .lead(
            PlayButton::new("header-play", "play")
                .on_click(move |_, _, cx| actions::play(&play, &[item], false, cx).detach()),
        )
        .lead(
            ShuffleButton::new("header-shuffle", name.clone())
                .on_click(move |_, _, cx| actions::play(&shuffle, &[item], true, cx).detach()),
        );
    // A phone's header keeps the heart and puts the rest under "more", as
    // the iOS app's does: four verbs in a row do not fit its width.
    if crate::theme::Look::of(window, cx).compact {
        let more = lib.clone();
        return header
            // The heart itself, so it pops and deflates as everywhere else.
            .action(
                crate::components::Heart::new("header-heart", favourite)
                    .role(TypeRole::Body)
                    .on_click(move |_, _, cx| {
                        heart
                            .feed
                            .update(cx, |feed, cx| feed.toggle_favourite(item, cx))
                    }),
            )
            .action(
                div()
                    .koan_menu_on_press("header-more", window, cx, move |m, _, cx| {
                        actions::menu(m, &more, Arc::from([item]), None, Default::default(), cx)
                    })
                    .child(
                        Button::new("header-more-button", ButtonKind::Compact)
                            .icon(Icon::More)
                            .label("more"),
                    ),
            );
    }
    header
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
                .help(
                    share_unavailable.clone().unwrap_or_else(|| {
                        "create a public link on your server and copy it".into()
                    }),
                )
                .disabled(share_unavailable.is_some())
                .on_click(move |_, _, cx| {
                    actions::copy_share_link(
                        share.engine.clone(),
                        vec![item.source()],
                        Some(name.to_string()),
                        cx,
                    )
                }),
        )
        .action(
            crate::components::Heart::new("header-heart", favourite)
                .role(TypeRole::Body)
                .on_click(move |_, _, cx| {
                    heart
                        .feed
                        .update(cx, |feed, cx| feed.toggle_favourite(item, cx))
                }),
        )
}
