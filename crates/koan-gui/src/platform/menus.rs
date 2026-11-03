//! The app's menus, in the Mac app's order: one description, which macOS
//! shows as its menu bar and Linux, which has no global menu bar, draws in
//! the window behind the toolbar's kōan button. Each item takes its title
//! from the shortcut table and its keys from the bindings, so the menus, the
//! keys and the shortcuts sheet say the same thing.

use koan_kit::app::platform::{Menu as BarMenu, MenuItem, OsAction, SystemMenuType};
use koan_kit::app::{Action, App, Context, FocusHandle, Window};

use super::actions::{self, APPLICATION, MENU, Shortcut};
use crate::components::{Menu, MenuEntry};

/// One entry of a menu.
pub enum Item {
    Action {
        title: &'static str,
        action: fn() -> Box<dyn Action>,
        /// What the system's own menu item does in its place (Copy, Undo).
        os: Option<OsAction>,
    },
    Separator,
    /// The system's Services submenu, on macOS only.
    Services,
}

pub struct MenuDef {
    pub name: &'static str,
    pub items: Vec<Item>,
}

/// The item for `A`, titled as the table titles it.
fn item<A: Action + Default>() -> Item {
    os_item::<A>(None)
}

fn os_item<A: Action + Default>(os: Option<OsAction>) -> Item {
    let probe: Box<dyn Action> = Box::new(A::default());
    let title = shortcut_for(probe.as_ref())
        .map(|s| s.title)
        .unwrap_or_else(|| probe.name());
    Item::Action {
        title,
        action: || Box::new(A::default()),
        os,
    }
}

/// An item the table has no key for.
fn plain<A: Action + Default>(title: &'static str) -> Item {
    Item::Action {
        title,
        action: || Box::new(A::default()),
        os: None,
    }
}

fn shortcut_for(action: &dyn Action) -> Option<&'static Shortcut> {
    MENU.iter()
        .chain(APPLICATION)
        .find(|s| (s.action)().partial_eq(action))
}

/// Whether an item belongs to macOS alone: hiding apps, Services, and the
/// window's own controls, which Linux draws on the window itself.
fn mac_only(item: &Item) -> bool {
    match item {
        Item::Services => true,
        Item::Separator => false,
        Item::Action { action, .. } => {
            let action = action();
            let theirs: [Box<dyn Action>; 5] = [
                Box::new(actions::Hide),
                Box::new(actions::HideOthers),
                Box::new(actions::ShowAll),
                Box::new(actions::Minimize),
                Box::new(actions::Zoom),
            ];
            theirs.iter().any(|m| m.partial_eq(action.as_ref()))
        }
    }
}

/// Every menu, in order.
pub fn menus() -> Vec<MenuDef> {
    use actions::*;
    vec![
        MenuDef {
            name: "kōan",
            items: vec![
                plain::<About>("About kōan"),
                Item::Separator,
                item::<Settings>(),
                Item::Separator,
                Item::Services,
                Item::Separator,
                item::<Hide>(),
                item::<HideOthers>(),
                plain::<ShowAll>("Show All"),
                Item::Separator,
                item::<Quit>(),
            ],
        },
        MenuDef {
            name: "File",
            items: vec![item::<Search>(), item::<AddMusic>()],
        },
        MenuDef {
            name: "Edit",
            items: vec![
                os_item::<Undo>(Some(OsAction::Undo)),
                os_item::<Redo>(Some(OsAction::Redo)),
                Item::Separator,
                os_item::<Cut>(Some(OsAction::Cut)),
                os_item::<Copy>(Some(OsAction::Copy)),
                os_item::<Paste>(Some(OsAction::Paste)),
                // ⌫ is a bare key: see `actions`.
                plain::<Delete>("Delete"),
                os_item::<SelectAll>(Some(OsAction::SelectAll)),
                Item::Separator,
                item::<Find>(),
            ],
        },
        MenuDef {
            name: "View",
            items: vec![
                item::<ShowQueue>(),
                item::<ShowAlbums>(),
                item::<ShowArtists>(),
                item::<ShowFavourites>(),
                item::<ShowHistory>(),
                item::<ShowRecentlyPlayed>(),
                item::<ShowTracks>(),
                Item::Separator,
                item::<Back>(),
                item::<Forward>(),
                Item::Separator,
                item::<ToggleSidebar>(),
                item::<ToggleLyrics>(),
                Item::Separator,
                item::<Bigger>(),
                item::<Smaller>(),
                item::<ActualSize>(),
            ],
        },
        MenuDef {
            name: "Playback",
            items: vec![
                // Space is a bare key, so the menu shows none: see `actions`.
                plain::<TogglePlayPause>("Play / Pause"),
                item::<Next>(),
                item::<Previous>(),
                Item::Separator,
                item::<SkipForward>(),
                item::<SkipBack>(),
                Item::Separator,
                item::<FavouriteCurrentTrack>(),
                Item::Separator,
                item::<ShowEq>(),
            ],
        },
        MenuDef {
            name: "Queue",
            items: vec![
                plain::<SaveSession>("Save Session"),
                plain::<ClearQueue>("Clear Queue"),
            ],
        },
        MenuDef {
            name: "Library",
            items: vec![
                item::<Rescan>(),
                plain::<ForceRescan>("Force Rescan"),
                plain::<RebuildIndex>("Rebuild Index"),
                Item::Separator,
                plain::<Sync>("Sync"),
                Item::Separator,
                plain::<ClearArtworkCache>("Clear Artwork Cache"),
                plain::<ClearDownloadedFiles>("Clear Downloaded Files"),
            ],
        },
        MenuDef {
            name: "Window",
            items: vec![item::<Minimize>(), plain::<Zoom>("Zoom")],
        },
        MenuDef {
            name: "Help",
            items: vec![item::<ShowShortcuts>()],
        },
    ]
}

/// macOS's menu bar.
pub fn set(cx: &mut App) {
    let bar: Vec<BarMenu> = menus()
        .into_iter()
        .map(|menu| {
            BarMenu::new(menu.name).items(menu.items.into_iter().map(|item| match item {
                Item::Action { title, action, os } => MenuItem::Action {
                    name: title.into(),
                    action: action(),
                    os_action: os,
                    checked: false,
                    disabled: false,
                },
                Item::Separator => MenuItem::separator(),
                Item::Services => MenuItem::os_submenu("Services", SystemMenuType::Services),
            }))
        })
        .collect();
    cx.set_menus(bar);
}

/// One item drawn in the window: its title, its keys as this system prints
/// them, and the action dispatched to the window as a key would.
fn entry(
    title: &'static str,
    action: fn() -> Box<dyn Action>,
    caption: impl Fn(&str) -> String,
    from: Option<FocusHandle>,
) -> MenuEntry {
    let keys = shortcut_for(action().as_ref())
        .and_then(|s| s.keys.iter().find(|k| !k.starts_with("cmd-")))
        .map(|k| caption(k));
    // From where the keyboard was when the menu opened, given it back first:
    // from the menu, the action would reach the toolbar and the shell but not
    // the page beneath, whose Delete, Select All, Undo and Redo it is. A page
    // gone since then leaves the dispatch at the window's root, as a key's
    // would be: the choice is never dropped.
    let entry = MenuEntry::new(title).on_click(move |_, window, cx| {
        if let Some(from) = &from {
            window.focus(from, cx);
        }
        window.dispatch_action(action(), cx);
    });
    match keys {
        Some(keys) => entry.keys(keys),
        None => entry,
    }
}

/// `items` drawn in the window, leaving out what is macOS's alone.
fn fill(
    mut menu: Menu,
    items: Vec<Item>,
    caption: impl Fn(&str) -> String + Copy,
    from: &Option<FocusHandle>,
) -> Menu {
    for item in items.into_iter().filter(|i| !mac_only(i)) {
        menu = match item {
            Item::Action { title, action, .. } => {
                menu.item(entry(title, action, caption, from.clone()))
            }
            Item::Separator => menu.separator(),
            Item::Services => menu,
        };
    }
    menu
}

/// The menus drawn in the window, for a system with no menu bar: the kōan
/// menu's own items first, as an app menu's would be, then a submenu for
/// each of the others.
pub fn in_window(menu: Menu, window: &mut Window, cx: &mut Context<Menu>) -> Menu {
    let os = cx.platform_info().os;
    let caption = move |key: &str| actions::caption(key, os);
    // Built as the menu opens, before it takes the keyboard.
    let from = window.focused(cx);
    let mut defs = menus().into_iter();
    let mut menu = match defs.next() {
        Some(app) => fill(menu, app.items, caption, &from),
        None => menu,
    };
    menu = menu.separator();
    for def in defs.filter(|d| !d.items.iter().all(mac_only)) {
        let (name, from) = (def.name, from.clone());
        menu = menu.submenu(name, window, cx, move |sub, _, _| {
            let items = menus()
                .into_iter()
                .find(|d| d.name == name)
                .map(|d| d.items)
                .unwrap_or_default();
            fill(sub, items, caption, &from)
        });
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window's menus drop exactly what is macOS's alone, and keep every
    /// menu that has anything else in it.
    #[test]
    fn linux_keeps_all_but_the_macs_own() {
        let menus = menus();
        let kept: Vec<&str> = menus
            .iter()
            .filter(|d| !d.items.iter().all(mac_only))
            .map(|d| d.name)
            .collect();
        assert_eq!(
            kept,
            [
                "kōan", "File", "Edit", "View", "Playback", "Queue", "Library", "Help"
            ]
        );
        let app: Vec<&str> = menus[0]
            .items
            .iter()
            .filter(|i| !mac_only(i))
            .filter_map(|i| match i {
                Item::Action { title, .. } => Some(*title),
                _ => None,
            })
            .collect();
        assert_eq!(app, ["About kōan", "Settings…", "Quit kōan"]);
    }
}
