//! What can be done with playable things from any library page: the context
//! menu, the selection bar's verbs, a click on a sleeve. Every one is an
//! engine call awaited off the main thread.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::components::{KoanContextMenu, Menu, MenuEntry};
use koan_ffi::Placement;
use koan_kit::app::*;

use super::Lib;
use super::drag::{Playable, sources};
use super::nav::Page;
use crate::theme::{Icon, Tone};

/// How many times music has gone to the queue and to each playlist from
/// here, which the sidebar's rows answer: the music went somewhere off the
/// page, and its place says so.
#[derive(Clone, Default)]
pub struct Arrivals {
    pub queue: u64,
    pub playlists: HashMap<i64, u64>,
}

impl Global for Arrivals {}

impl Arrivals {
    pub fn to(&self, playlist: i64) -> u64 {
        self.playlists.get(&playlist).copied().unwrap_or_default()
    }
}

/// Music reached the queue, or `playlist`.
pub(crate) fn arrived(playlist: Option<i64>, cx: &mut App) {
    let arrivals = cx.default_global::<Arrivals>();
    match playlist {
        None => arrivals.queue += 1,
        Some(id) => *arrivals.playlists.entry(id).or_default() += 1,
    }
}

/// Put `items` in the queue. The returned task ends once the engine has the
/// command, which is when a sleeve's "starting" mark can go.
pub fn queue(lib: &Lib, items: &[Playable], placement: Placement, cx: &mut App) -> Task<()> {
    let engine = lib.engine.clone();
    let sets = sources(items);
    cx.spawn(async move |cx| match engine.queue(sets, placement).await {
        Ok(_) => cx.update(|cx| arrived(None, cx)),
        Err(e) => log::warn!("queue: {e}"),
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

/// Play a list from one of its rows, keeping the rest behind it: a double
/// click in a track list. Where showing the queue once it plays belongs, when
/// the shell has one to show.
pub fn play_list(lib: &Lib, track_ids: Vec<i64>, index: usize, cx: &mut App) {
    let engine = lib.engine.clone();
    cx.spawn(async move |cx| {
        match engine
            .replace_queue(track_ids, Some(index as u32), false)
            .await
        {
            Ok(_) => cx.update(|cx| arrived(None, cx)),
            Err(e) => log::warn!("play: {e}"),
        }
    })
    .detach();
}

pub fn add_to_playlist(lib: &Lib, playlist: i64, items: &[Playable], cx: &mut App) {
    let engine = lib.engine.clone();
    let sets = sources(items);
    let feed = lib.feed.clone();
    cx.spawn(async move |cx| {
        match engine.add_to_playlist(playlist, sets).await {
            Ok(_) => cx.update(|cx| arrived(Some(playlist), cx)),
            Err(e) => log::warn!("add to playlist {playlist}: {e}"),
        }
        feed.update(cx, |feed, cx| feed.reload_playlists(cx));
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

/// The tracks `items` name, in order, each once: what a verb over records
/// and artists acts on.
pub async fn track_ids_of(engine: Arc<koan_ffi::KoanEngine>, items: Vec<Playable>) -> Vec<i64> {
    let mut ids = Vec::new();
    for item in items {
        let found = match item {
            Playable::Track(id) => Ok(vec![id]),
            Playable::Album(id) => engine.clone().track_ids(Some(id), None).await,
            Playable::Artist(id) => engine.clone().track_ids(None, Some(id)).await,
        };
        match found {
            Ok(found) => {
                for id in found {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
            Err(e) => log::warn!("tracks of a pick: {e}"),
        }
    }
    ids
}

/// What a pick is called in a question about it.
fn pick_title(items: &[Playable]) -> String {
    match items {
        [Playable::Track(_)] => "1 track".to_owned(),
        [Playable::Album(_)] => "this album".to_owned(),
        [Playable::Artist(_)] => "this artist's tracks".to_owned(),
        _ => format!("{} picked", items.len()),
    }
}

/// Resolves the pick to its tracks, in order, and opens organise over them.
pub fn organise(lib: &Lib, items: &[Playable], window: &mut Window, cx: &mut App) {
    let engine = lib.engine.clone();
    let items: Vec<Playable> = items.to_vec();
    let title = pick_title(&items);
    window
        .spawn(cx, async move |cx| {
            let ids = track_ids_of(engine, items).await;
            cx.update(|window, cx| crate::organize::open(title, ids, window, cx))
                .ok();
        })
        .detach();
}

/// Fetches the pick's tracks onto this device now, as the Mac's "download
/// to cache" does: wanted for their own sake, behind what is playing.
pub fn download(lib: &Lib, items: &[Playable], cx: &mut App) {
    let (engine, items) = (lib.engine.clone(), items.to_vec());
    cx.background_spawn(async move {
        let ids = track_ids_of(engine.clone(), items).await;
        if let Err(e) = engine.download_to_cache(ids).await {
            log::warn!("download to cache: {e}");
        }
    })
    .detach();
}

/// Asks, then deletes the copies of the pick's tracks downloaded to this
/// device. They stay on the server and download again when played.
pub fn remove_downloads(lib: &Lib, items: &[Playable], window: &mut Window, cx: &mut App) {
    let (engine, items) = (lib.engine.clone(), items.to_vec());
    let title = pick_title(&items);
    window
        .spawn(cx, async move |cx| {
            let ids = track_ids_of(engine.clone(), items).await;
            cx.update(|window, cx| ask_remove_downloads(engine, title, ids, window, cx))
                .ok();
        })
        .detach();
}

/// The question before removing downloads, and the removal: shared by the
/// library's menus and the queue's.
pub fn ask_remove_downloads(
    engine: Arc<koan_ffi::KoanEngine>,
    title: String,
    ids: Vec<i64>,
    window: &mut Window,
    cx: &mut App,
) {
    if ids.is_empty() {
        return;
    }
    crate::components::sheet::confirm(
        crate::components::sheet::Confirm {
            title: "remove downloaded files".into(),
            verb: "remove".into(),
            destructive: true,
            lines: vec![format!("The copies of {title} downloaded to this device are deleted.").into()],
            note: Some("They stay on the server and download again when played. Local files are not touched.".into()),
        },
        window,
        cx,
        move |cx| {
            let (engine, ids) = (engine.clone(), ids.clone());
            cx.background_spawn(async move {
                if let Err(e) = engine.clear_downloads(ids).await {
                    log::warn!("remove downloaded files: {e}");
                }
            })
            .detach();
        },
    );
}

/// Run after a menu's verb: ends the pick the menu was opened over, as each
/// of the selection bar's verbs does.
pub type After = Option<Rc<dyn Fn(&mut App)>>;

/// What a right click landed on: the items the menu acts on.
#[derive(Clone)]
pub struct MenuTarget {
    pub items: Arc<[Playable]>,
    /// The record a single track belongs to, for "go to album".
    pub album: Option<i64>,
    pub after: After,
}

/// What the menu key's menu acts on: the pick when the item under the
/// keyboard's cursor is in it, the item alone otherwise. `album` is the
/// record of a single track, for "go to album"; `after` ends the pick once
/// a verb has run on it, as a right click's menu over the pick does.
pub fn keyed_target(
    selection: &super::selection::Selection,
    item: Playable,
    album: Option<i64>,
    after: After,
) -> MenuTarget {
    let items = selection.dragging(item);
    let single = items.len() == 1;
    MenuTarget {
        items: items.into(),
        album: album.filter(|_| single),
        // What a menu verb on the pick runs after it: on the item alone,
        // nothing.
        after: if single { None } else { after },
    }
}

/// A page with one context menu over its list, rather than one per row: a
/// row notes what was right-clicked, and the list's menu acts on it.
pub trait Menus: Sized + 'static {
    fn lib(&self) -> &Lib;
    fn menu_target(&mut self) -> &mut Option<MenuTarget>;
    fn menu_target_ref(&self) -> &Option<MenuTarget>;
    /// Where the list's keyboard cursor was laid out: what the menu key
    /// opens the menu under.
    fn cursor_spot(&self) -> Option<crate::components::CursorSpot> {
        None
    }
}

/// A row's or tile's right mouse down, noting what the list's menu acts on.
pub fn mark<V: Menus>(
    view: WeakEntity<V>,
    target: MenuTarget,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    super::listen(view, move |this: &mut V, _: &MouseDownEvent, _, _| {
        *this.menu_target() = Some(target.clone());
    })
}

/// The list's one context menu, over whatever its rows marked. A right
/// click on the list clears the mark first, in the capture phase, so one on
/// empty space acts on nothing.
pub fn list_menu<V: Menus, E: InteractiveElement + ParentElement + Styled + 'static>(
    element: E,
    view: WeakEntity<V>,
    window: &mut Window,
    cx: &mut App,
) -> E {
    let clear = view.clone();
    let spot = view.upgrade().and_then(|page| page.read(cx).cursor_spot());
    element
        .capture_any_mouse_down(super::listen(
            clear,
            |this: &mut V, e: &MouseDownEvent, _, _| {
                if e.button == MouseButton::Right {
                    *this.menu_target() = None;
                }
            },
        ))
        .koan_context_menu_at("list-menu", spot, window, cx, move |m, window, cx| {
            let Some(view) = view.upgrade() else { return m };
            let (lib, target) = {
                let page = view.read(cx);
                (page.lib().clone(), page.menu_target_ref().clone())
            };
            match target {
                Some(t) => menu(m, &lib, t.items, t.album, t.after, window, cx),
                None => m,
            }
        })
}

/// The context menu for `items`: one thing, or a whole pick. `album` is the
/// record a single track belongs to, for "go to album".
pub fn menu(
    menu: Menu,
    lib: &Lib,
    items: Arc<[Playable]>,
    album: Option<i64>,
    after: After,
    window: &mut Window,
    cx: &mut Context<Menu>,
) -> Menu {
    let single = (items.len() == 1).then(|| items[0]);
    let container =
        items.len() > 1 || matches!(single, Some(Playable::Album(_) | Playable::Artist(_)));
    let favourited = items.iter().all(|i| lib.feed.read(cx).is_favourite(*i));

    let verb = |label: &'static str, icon: Icon, f: fn(&Lib, &[Playable], &mut App)| {
        let (lib, items, after) = (lib.clone(), items.clone(), after.clone());
        MenuEntry::new(label).icon(icon).on_click(move |_, _, cx| {
            f(&lib, &items, cx);
            if let Some(after) = &after {
                after(cx);
            }
        })
    };

    let mut menu = menu
        .item(verb("play", Icon::Play, |lib, items, cx| {
            play(lib, items, false, cx).detach()
        }))
        .item(verb("play next", Icon::PlayNext, |lib, items, cx| {
            queue(lib, items, Placement::Next, cx).detach()
        }))
        .item(verb("add to queue", Icon::Queue, |lib, items, cx| {
            queue(lib, items, Placement::Append, cx).detach()
        }));
    if container {
        menu = menu.item(verb("shuffle", Icon::Shuffle, |lib, items, cx| {
            play(lib, items, true, cx).detach()
        }));
    }
    menu = playlist_submenu(
        menu.separator(),
        lib,
        items.clone(),
        after.clone(),
        window,
        cx,
    );
    menu = menu.item(verb(
        if favourited {
            "unfavourite"
        } else {
            "favourite"
        },
        if favourited {
            Icon::Favourited
        } else {
            Icon::Favourite
        },
        favourite,
    ));
    let (engine, sets) = (lib.engine.clone(), sources(&items));
    menu = menu.item(
        MenuEntry::new("copy share link")
            .icon(Icon::Share)
            .on_click(move |_, _, cx| copy_share_link(engine.clone(), sets.clone(), None, cx)),
    );
    // Moving the pick's files where a pattern puts them, previewed first.
    let (organise_lib, organise_items, organise_after) =
        (lib.clone(), items.clone(), after.clone());
    menu =
        menu.separator()
            .item(MenuEntry::new("organise…").icon(Icon::Organize).on_click(
                move |_, window, cx| {
                    organise(&organise_lib, &organise_items, window, cx);
                    if let Some(after) = &organise_after {
                        after(cx);
                    }
                },
            ));
    // A remote library's files on this device: fetched now, or let go.
    let (fetch_lib, fetch_items) = (lib.clone(), items.clone());
    let (drop_lib, drop_items) = (lib.clone(), items.clone());
    menu = menu
        .item(
            MenuEntry::new(if container {
                "download all to cache"
            } else {
                "download to cache"
            })
            .icon(Icon::Downloads)
            .on_click(move |_, _, cx| download(&fetch_lib, &fetch_items, cx)),
        )
        .item(
            MenuEntry::new("remove downloaded files…")
                .icon(Icon::Clear)
                .on_click(move |_, window, cx| {
                    remove_downloads(&drop_lib, &drop_items, window, cx)
                }),
        );
    match single {
        Some(Playable::Track(id)) => {
            let nav = lib.nav.clone();
            if let Some(album) = album {
                let nav = nav.clone();
                menu = menu.separator().item(
                    MenuEntry::new("go to album")
                        .icon(Icon::Album)
                        .on_click(move |_, _, cx| {
                            nav.update(cx, |nav, cx| nav.open(Page::Album(album), cx))
                        }),
                );
            }
            menu = menu.item(
                MenuEntry::new("track info")
                    .icon(Icon::Info)
                    .on_click(move |_, _, cx| nav.update(cx, |nav, cx| nav.show_info(id, cx))),
            );
        }
        Some(Playable::Album(id)) => {
            let nav = lib.nav.clone();
            menu =
                menu.separator()
                    .item(MenuEntry::new("open album").icon(Icon::Album).on_click(
                        move |_, _, cx| nav.update(cx, |nav, cx| nav.open(Page::Album(id), cx)),
                    ));
        }
        _ => {}
    }
    menu
}

/// "add to playlist": a new playlist holding `items`, then every playlist
/// that takes tracks. One submenu for the library's menus and the queue's.
pub fn playlist_submenu(
    menu: Menu,
    lib: &Lib,
    items: Arc<[Playable]>,
    after: After,
    window: &mut Window,
    cx: &mut Context<Menu>,
) -> Menu {
    let playlists = lib.feed.read(cx).playlists.clone();
    let lib = lib.clone();
    menu.submenu_icon(
        "add to playlist",
        Icon::Playlist,
        window,
        cx,
        move |mut menu, _, _| {
            {
                let (lib, items, after) = (lib.clone(), items.clone(), after.clone());
                menu = menu.item(MenuEntry::new("new playlist…").icon(Icon::Add).on_click(
                    move |_, window, cx| {
                        super::playlist::new_playlist(
                            lib.engine.clone(),
                            lib.nav.read(cx).navigator(),
                            sources(&items),
                            window,
                            cx,
                        );
                        if let Some(after) = &after {
                            after(cx);
                        }
                    },
                ));
            }
            if !playlists.is_empty() {
                menu = menu.separator();
            }
            for (id, name) in &playlists {
                let (lib, items, after, id) = (lib.clone(), items.clone(), after.clone(), *id);
                menu = menu.item(MenuEntry::data(name.clone()).on_click(move |_, _, cx| {
                    add_to_playlist(&lib, id, &items, cx);
                    if let Some(after) = &after {
                        after(cx);
                    }
                }));
            }
            menu
        },
    )
}

/// Asks the server for a public link to the tracks `sets` name and copies
/// it: pasting it somewhere is the only thing anyone does with one. Only
/// tracks the server holds can go in it; the notice says how many were left
/// out. Said either way on the app's notice line, since a link copied in
/// silence, or not at all, looks the same as a click that missed.
pub fn copy_share_link(
    engine: Arc<koan_ffi::KoanEngine>,
    sets: Vec<koan_ffi::TrackSet>,
    name: Option<String>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        let said = |text: String, tone: Tone, cx: &mut AsyncApp| {
            cx.update(|cx| crate::notice::show(text, tone, cx));
        };
        let what = name.clone().unwrap_or_else(|| "these tracks".to_owned());
        let ids = match engine.clone().track_ids_of(sets).await {
            Ok(ids) if !ids.is_empty() => ids,
            Ok(_) => return said(format!("{what}: nothing to share"), Tone::Muted, cx),
            Err(e) => {
                log::warn!("share {what}: {e}");
                return said(format!("Couldn't share {what}: {e}"), Tone::Bad, cx);
            }
        };
        match engine.create_share(ids, name.clone()).await {
            Ok(share) => cx.update(|cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(share.url));
                let text = if share.skipped > 0 {
                    format!(
                        "A share link is copied: {} of {} tracks; the rest aren't on your server",
                        share.shared,
                        share.shared + share.skipped
                    )
                } else {
                    match &name {
                        Some(name) => format!("A link to {name} is copied"),
                        None => "A share link is copied".to_owned(),
                    }
                };
                crate::notice::show(text, Tone::Muted, cx);
            }),
            Err(e) => {
                log::warn!("share {what}: {e}");
                said(format!("Couldn't share {what}: {e}"), Tone::Bad, cx);
            }
        }
    })
    .detach();
}
