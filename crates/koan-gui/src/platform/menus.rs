//! The macOS menu bar, in the Mac app's order. Each item takes its title from
//! the shortcut table and its key from the bindings, so the menu, the keys and
//! the shortcuts sheet say the same thing.
//!
//! GPUI draws no menu bar on Linux; the bindings and the sheet stand for it
//! there.

use gpui_kit::{Action, App, Menu, MenuItem, OsAction, SystemMenuType};

use super::actions::{self, APPLICATION, MENU};

/// The item for `A`, titled as the table titles it.
fn item<A: Action + Default>() -> MenuItem {
    os_item::<A>(None)
}

fn os_item<A: Action + Default>(os_action: Option<OsAction>) -> MenuItem {
    let action: Box<dyn Action> = Box::new(A::default());
    let title = MENU
        .iter()
        .chain(APPLICATION)
        .find(|s| (s.action)().partial_eq(action.as_ref()))
        .map(|s| s.title)
        .unwrap_or_else(|| action.name());
    MenuItem::Action {
        name: title.into(),
        action,
        os_action,
        checked: false,
        disabled: false,
    }
}

/// An item the table has no key for.
fn plain(name: &'static str, action: impl Action) -> MenuItem {
    MenuItem::action(name, action)
}

pub fn set(cx: &mut App) {
    use actions::*;
    cx.set_menus([
        Menu::new("kōan").items([
            plain("About kōan", About),
            MenuItem::separator(),
            item::<Settings>(),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            item::<Hide>(),
            item::<HideOthers>(),
            plain("Show All", ShowAll),
            MenuItem::separator(),
            item::<Quit>(),
        ]),
        Menu::new("File").items([item::<Search>(), item::<AddMusic>()]),
        Menu::new("Edit").items([
            os_item::<Undo>(Some(OsAction::Undo)),
            os_item::<Redo>(Some(OsAction::Redo)),
            MenuItem::separator(),
            os_item::<Cut>(Some(OsAction::Cut)),
            os_item::<Copy>(Some(OsAction::Copy)),
            os_item::<Paste>(Some(OsAction::Paste)),
            // ⌫ is a bare key: see `actions`.
            plain("Delete", Delete),
            os_item::<SelectAll>(Some(OsAction::SelectAll)),
            MenuItem::separator(),
            item::<Find>(),
        ]),
        Menu::new("View").items([
            item::<ShowQueue>(),
            item::<ShowAlbums>(),
            item::<ShowArtists>(),
            item::<ShowFavourites>(),
            item::<ShowHistory>(),
            item::<ShowRecentlyPlayed>(),
            item::<ShowTracks>(),
            MenuItem::separator(),
            item::<Back>(),
            item::<Forward>(),
            MenuItem::separator(),
            item::<ToggleLyrics>(),
            MenuItem::separator(),
            item::<Bigger>(),
            item::<Smaller>(),
            item::<ActualSize>(),
        ]),
        Menu::new("Playback").items([
            // Space is a bare key, so the menu shows none: see `actions`.
            plain("Play / Pause", TogglePlayPause),
            item::<Next>(),
            item::<Previous>(),
            MenuItem::separator(),
            item::<SkipForward>(),
            item::<SkipBack>(),
            MenuItem::separator(),
            item::<FavouriteCurrentTrack>(),
            MenuItem::separator(),
            item::<ShowEq>(),
        ]),
        Menu::new("Queue").items([
            plain("Save Session", SaveSession),
            plain("Clear Queue", ClearQueue),
        ]),
        Menu::new("Library").items([
            item::<Rescan>(),
            plain("Force Rescan", ForceRescan),
            plain("Rebuild Index", RebuildIndex),
            MenuItem::separator(),
            plain("Sync", Sync),
            MenuItem::separator(),
            plain("Clear Artwork Cache", ClearArtworkCache),
            plain("Clear Downloaded Files", ClearDownloadedFiles),
        ]),
        Menu::new("Window").items([item::<Minimize>(), plain("Zoom", Zoom)]),
        Menu::new("Help").items([item::<ShowShortcuts>()]),
    ]);
}
