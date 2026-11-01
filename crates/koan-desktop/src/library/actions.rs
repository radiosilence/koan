//! What can be done with playable things from any library page: the context
//! menu, the selection bar's verbs, a click on a sleeve. Every one is an
//! engine call awaited off the main thread.

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;
use koan_ffi::Placement;

use super::Lib;
use super::drag::{Playable, sources};
use super::nav::Page;

/// Put `items` in the queue. The returned task ends once the engine has the
/// command, which is when a sleeve's "starting" mark can go.
pub fn queue(lib: &Lib, items: &[Playable], placement: Placement, cx: &mut App) -> Task<()> {
    let engine = lib.engine.clone();
    let sets = sources(items);
    cx.spawn(async move |_| {
        if let Err(e) = engine.queue(sets, placement).await {
            log::warn!("queue: {e}");
        }
    })
}

pub fn play(lib: &Lib, items: &[Playable], shuffled: bool, cx: &mut App) -> Task<()> {
    queue(
        lib,
        items,
        Placement::Replace {
            start_at: None,
            shuffled,
        },
        cx,
    )
}

pub fn add_to_playlist(lib: &Lib, playlist: i64, items: &[Playable], cx: &mut App) {
    let engine = lib.engine.clone();
    let sets = sources(items);
    let feed = lib.feed.clone();
    cx.spawn(async move |cx| {
        if let Err(e) = engine.add_to_playlist(playlist, sets).await {
            log::warn!("add to playlist {playlist}: {e}");
        }
        feed.update(cx, |feed, cx| feed.reload_playlists(cx)).ok();
    })
    .detach();
}

/// Favourite every item, or, when all of them already are, unfavourite them:
/// what one verb over a pick means.
pub fn favourite(lib: &Lib, items: &[Playable], cx: &mut App) {
    let all = items.iter().all(|i| lib.feed.read(cx).is_favourite(*i));
    lib.feed.update(cx, |feed, cx| {
        for item in items {
            if feed.is_favourite(*item) == all {
                feed.toggle_favourite(*item, cx);
            }
        }
    });
}

/// The context menu for `items`: one thing, or a whole pick. `album` is the
/// record a single track belongs to, for "go to album".
pub fn menu(
    menu: PopupMenu,
    lib: &Lib,
    items: Vec<Playable>,
    album: Option<i64>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let single = (items.len() == 1).then(|| items[0]);
    let container =
        items.len() > 1 || matches!(single, Some(Playable::Album(_) | Playable::Artist(_)));
    let favourited = items.iter().all(|i| lib.feed.read(cx).is_favourite(*i));
    let playlists = lib.feed.read(cx).playlists.clone();

    let item =
        |label: &'static str, lib: &Lib, items: &[Playable], f: fn(&Lib, &[Playable], &mut App)| {
            let (lib, items) = (lib.clone(), items.to_vec());
            gpui_kit::component::menu::PopupMenuItem::new(label)
                .on_click(move |_, _, cx| f(&lib, &items, cx))
        };

    let mut menu = menu
        .item(item("play", lib, &items, |lib, items, cx| {
            play(lib, items, false, cx).detach()
        }))
        .item(item("play next", lib, &items, |lib, items, cx| {
            queue(lib, items, Placement::Next, cx).detach()
        }))
        .item(item("add to queue", lib, &items, |lib, items, cx| {
            queue(lib, items, Placement::Append, cx).detach()
        }));
    if container {
        menu = menu.item(item("shuffle", lib, &items, |lib, items, cx| {
            play(lib, items, true, cx).detach()
        }));
    }
    let (sub_lib, sub_items) = (lib.clone(), items.clone());
    menu = menu
        .separator()
        .submenu("add to playlist", window, cx, move |mut menu, _, _| {
            if playlists.is_empty() {
                return menu.label("no playlists");
            }
            for (id, name) in &playlists {
                let (lib, items, id) = (sub_lib.clone(), sub_items.clone(), *id);
                menu = menu.item(
                    gpui_kit::component::menu::PopupMenuItem::new(name.clone())
                        .on_click(move |_, _, cx| add_to_playlist(&lib, id, &items, cx)),
                );
            }
            menu
        });
    menu = menu.item(item(
        if favourited {
            "unfavourite"
        } else {
            "favourite"
        },
        lib,
        &items,
        favourite,
    ));
    match single {
        Some(Playable::Track(id)) => {
            let nav = lib.nav.clone();
            if let Some(album) = album {
                let nav = nav.clone();
                menu = menu.separator().item(
                    gpui_kit::component::menu::PopupMenuItem::new("go to album").on_click(
                        move |_, _, cx| nav.update(cx, |nav, cx| nav.open(Page::Album(album), cx)),
                    ),
                );
            }
            menu = menu.item(
                gpui_kit::component::menu::PopupMenuItem::new("track info")
                    .on_click(move |_, _, cx| nav.update(cx, |nav, cx| nav.show_info(id, cx))),
            );
        }
        Some(Playable::Album(id)) => {
            let nav = lib.nav.clone();
            menu = menu.separator().item(
                gpui_kit::component::menu::PopupMenuItem::new("open album").on_click(
                    move |_, _, cx| nav.update(cx, |nav, cx| nav.open(Page::Album(id), cx)),
                ),
            );
        }
        _ => {}
    }
    menu
}
