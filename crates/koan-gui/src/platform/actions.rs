//! Every keyboard shortcut and menu command, from one table.
//!
//! The key bindings, the macOS menu bar and the shortcuts sheet are all built
//! from `MENU` and `HOTKEYS`, so what the app does and what it says it does
//! cannot drift. They follow the Mac app's `MenuShortcut` and `Hotkeys`.
//!
//! Two tables because there are two kinds of key. A shortcut with a modifier
//! belongs to the menu bar and works anywhere in the window, a text field's
//! own binding for the same key winning while it is focused, which is the Mac
//! app's "disabled while typing": GPUI ranks a binding by the depth of the
//! context it matched, and the window's `koan` sits above any field's
//! `Input`. A binding with no context would rank deepest of all and take
//! ⌘C and ⌘← from the field. A bare key is live only outside text fields,
//! and is never given to a menu: its menu item, where it has one, takes a
//! separate action, so the key is never the item's key equivalent.
//!
//! Engine-only commands are handled here (`register`). The rest are declared
//! for the shell and the pages to handle with `on_action`; on macOS a menu
//! item with no handler in reach is greyed.

use std::rc::Rc;
use std::sync::Arc;

use koan_kit::app::platform::{KeyBindingContextPredicate, OperatingSystem};
use koan_kit::app::{Action, App, KeyBinding, actions};
use koan_kit::components::list_actions as list;
use koan_runtime::KoanEngine;

/// The key context the shell's root element carries.
pub const CONTEXT: &str = "koan";

/// Where the menus' shortcuts are live: inside the window, and not while a
/// sheet or question over it holds the keyboard (`sheet::MODAL`), so none
/// acts behind a confirmation.
const MENU_CONTEXT: &str = "koan && !Modal";

/// Where bare keys are live: as the menus' shortcuts, and outside any text
/// field or menu, where space would otherwise play behind it.
const BARE_CONTEXT: &str = "koan && !Input && !Modal && !KoanMenu";

/// What the shell's text fields call their context (gpui-base's `Input`).
#[cfg(test)]
const INPUT_CONTEXT: &str = "Input";

/// How far the skip shortcuts move the playhead.
const SKIP_MS: i64 = 10_000;

actions!(
    koan,
    [
        // Playback.
        TogglePlayPause,
        Next,
        Previous,
        SkipForward,
        SkipBack,
        FavouriteCurrentTrack,
        ShowEq,
        // Navigation.
        Search,
        AddMusic,
        Back,
        Forward,
        ShowQueue,
        ShowAlbums,
        ShowArtists,
        ShowFavourites,
        ShowHistory,
        ShowRecentlyPlayed,
        ShowTracks,
        TopOfQueue,
        EndOfQueue,
        // View.
        ToggleSidebar,
        ToggleLyrics,
        Bigger,
        Smaller,
        ActualSize,
        ShowShortcuts,
        ZoomCover,
        ClearSelection,
        FocusNext,
        FocusPrevious,
        NextRegion,
        PreviousRegion,
        // Edit.
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        Delete,
        SelectAll,
        Find,
        // Library and queue.
        Rescan,
        ForceRescan,
        Sync,
        RebuildIndex,
        ClearArtworkCache,
        ClearDownloadedFiles,
        SaveSession,
        ClearQueue,
        // The application.
        About,
        Settings,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        Quit,
    ]
);

/// The bare keys whose commands are also menu items: their own actions, so
/// the menu's Play / Pause and Delete are never given space and ⌫ as key
/// equivalents. Each is handled as its menu twin is.
mod bare {
    koan_kit::app::actions!(koan_bare, [TogglePlayPause, Delete]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Playback,
    Navigation,
    View,
    Edit,
    Library,
    /// Inside a list: the sidebar, the queue's neighbours, a page of tracks.
    Lists,
}

impl Group {
    pub const ALL: [Group; 6] = [
        Group::Playback,
        Group::Navigation,
        Group::View,
        Group::Edit,
        Group::Library,
        Group::Lists,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::Playback => "playback",
            Group::Navigation => "navigation",
            Group::View => "view",
            Group::Edit => "edit",
            Group::Library => "library",
            Group::Lists => "in a list",
        }
    }
}

/// One shortcut: what it is called, the keys that do it, as GPUI writes a
/// keystroke (`secondary-` is ⌘ on macOS and Ctrl elsewhere), and the action.
pub struct Shortcut {
    pub title: &'static str,
    pub keys: &'static [&'static str],
    pub group: Group,
    pub action: fn() -> Box<dyn Action>,
}

macro_rules! shortcut {
    ($title:literal, [$($key:literal),+], $group:ident, $action:path) => {
        Shortcut {
            title: $title,
            keys: &[$($key),+],
            group: Group::$group,
            action: || Box::new($action),
        }
    };
}

/// Everything with a modifier, in the order the sheet lists it. ⌘= is Bigger
/// as well as ⌘+, as in every app with the item; the menu shows the first.
pub const MENU: &[Shortcut] = &[
    shortcut!("Search…", ["secondary-k"], Navigation, Search),
    shortcut!(
        "Find and Play…",
        ["secondary-shift-k"],
        Navigation,
        AddMusic
    ),
    shortcut!("Queue", ["secondary-1"], Navigation, ShowQueue),
    shortcut!("Albums", ["secondary-2"], Navigation, ShowAlbums),
    shortcut!("Artists", ["secondary-3"], Navigation, ShowArtists),
    shortcut!("Favourites", ["secondary-4"], Navigation, ShowFavourites),
    shortcut!("History", ["secondary-5"], Navigation, ShowHistory),
    shortcut!(
        "Recently Played",
        ["secondary-6"],
        Navigation,
        ShowRecentlyPlayed
    ),
    shortcut!("Tracks", ["secondary-7"], Navigation, ShowTracks),
    shortcut!("Back", ["secondary-["], Navigation, Back),
    shortcut!("Forward", ["secondary-]"], Navigation, Forward),
    shortcut!("Next", ["secondary-right"], Playback, Next),
    shortcut!("Previous", ["secondary-left"], Playback, Previous),
    shortcut!("Skip Forward", ["alt-right"], Playback, SkipForward),
    shortcut!("Skip Back", ["alt-left"], Playback, SkipBack),
    shortcut!(
        "Favourite Current Track",
        ["secondary-d"],
        Playback,
        FavouriteCurrentTrack
    ),
    shortcut!("EQ…", ["secondary-alt-e"], Playback, ShowEq),
    shortcut!("Toggle Sidebar", ["ctrl-secondary-s"], View, ToggleSidebar),
    shortcut!("Toggle Lyrics", ["secondary-alt-l"], View, ToggleLyrics),
    shortcut!("Keyboard Shortcuts", ["secondary-/"], View, ShowShortcuts),
    shortcut!("Bigger", ["secondary-+", "secondary-="], View, Bigger),
    shortcut!("Smaller", ["secondary--"], View, Smaller),
    shortcut!("Actual Size", ["secondary-0"], View, ActualSize),
    shortcut!("Undo", ["secondary-z"], Edit, Undo),
    shortcut!("Redo", ["secondary-shift-z"], Edit, Redo),
    shortcut!("Cut", ["secondary-x"], Edit, Cut),
    shortcut!("Copy", ["secondary-c"], Edit, Copy),
    shortcut!("Paste", ["secondary-v"], Edit, Paste),
    shortcut!("Select All", ["secondary-a"], Edit, SelectAll),
    shortcut!("Find", ["secondary-f"], Edit, Find),
    shortcut!(
        "Rescan Local Folders",
        ["secondary-shift-r"],
        Library,
        Rescan
    ),
];

/// The application's own keys: in the menu bar on macOS, and not in the
/// sheet, which lists what kōan does rather than what every app does.
pub const APPLICATION: &[Shortcut] = &[
    shortcut!("Settings…", ["secondary-,"], Library, Settings),
    shortcut!("Hide kōan", ["cmd-h"], Library, Hide),
    shortcut!("Hide Others", ["cmd-alt-h"], Library, HideOthers),
    shortcut!("Minimize", ["cmd-m"], Library, Minimize),
    shortcut!("Quit kōan", ["secondary-q"], Library, Quit),
];

/// Single keys, following the terminal UI where the two apps do the same
/// thing. Live everywhere in the window except a text field.
pub const HOTKEYS: &[Shortcut] = &[
    shortcut!("Play / pause", ["space"], Playback, bare::TogglePlayPause),
    shortcut!("Previous track", ["<"], Playback, Previous),
    shortcut!("Next track", [">", "n"], Playback, Next),
    shortcut!("Back 10 seconds", [","], Playback, SkipBack),
    shortcut!("Forward 10 seconds", ["."], Playback, SkipForward),
    shortcut!(
        "Favourite this track",
        ["f"],
        Playback,
        FavouriteCurrentTrack
    ),
    shortcut!("Find and play", ["p"], Navigation, AddMusic),
    shortcut!("Search library", ["/"], Navigation, Search),
    shortcut!("Albums", ["l", "a"], Navigation, ShowAlbums),
    shortcut!("Artists", ["r"], Navigation, ShowArtists),
    shortcut!("Queue", ["q"], Navigation, ShowQueue),
    shortcut!("Top of the queue", ["g"], Navigation, TopOfQueue),
    shortcut!("End of the queue", ["G"], Navigation, EndOfQueue),
    shortcut!("Lyrics panel", ["L"], View, ToggleLyrics),
    shortcut!("Zoom the cover", ["z"], View, ZoomCover),
    shortcut!("Clear selection", ["escape"], View, ClearSelection),
    shortcut!("Delete", ["backspace", "delete"], Edit, bare::Delete),
    shortcut!("This list", ["?"], View, ShowShortcuts),
    shortcut!("Next control", ["tab"], View, FocusNext),
    shortcut!("Previous control", ["shift-tab"], View, FocusPrevious),
    shortcut!(
        "Sidebar, page and transport in turn",
        ["f6"],
        View,
        NextRegion
    ),
    shortcut!("The same, backwards", ["shift-f6"], View, PreviousRegion),
];

/// Inside a list that holds the keyboard, koan-kit's list keys
/// (`components::list_keys`): bound in the `List` context, outside a text
/// field, so a list's own keys win over the window's.
pub const LISTS: &[Shortcut] = &[
    shortcut!("The row above or below", ["up", "down"], Lists, list::Down),
    shortcut!(
        "The pick, extended up or down",
        ["shift-up", "shift-down"],
        Lists,
        list::ExtendDown
    ),
    shortcut!(
        "In a grid, the tile before or after",
        ["left", "right"],
        Lists,
        list::Right
    ),
    shortcut!(
        "In a grid, the pick extended across",
        ["shift-left", "shift-right"],
        Lists,
        list::ExtendRight
    ),
    shortcut!("The first or last row", ["home", "end"], Lists, list::Last),
    shortcut!(
        "A screenful up or down",
        ["pageup", "pagedown"],
        Lists,
        list::PageDown
    ),
    shortcut!("Play or open the row", ["enter"], Lists, list::Activate),
    shortcut!(
        "Add the row to the pick, or take it out",
        ["ctrl-space"],
        Lists,
        list::Pick
    ),
    shortcut!(
        "The row's menu",
        ["menu", "shift-f10"],
        Lists,
        list::ShowMenu
    ),
];

/// What each list key binds to: a line of `LISTS` can name two keys, one
/// for each way.
fn list_binding(key: &str) -> Box<dyn Action> {
    match key {
        "up" => Box::new(list::Up),
        "left" => Box::new(list::Left),
        "right" => Box::new(list::Right),
        "shift-left" => Box::new(list::ExtendLeft),
        "shift-right" => Box::new(list::ExtendRight),
        "shift-up" => Box::new(list::ExtendUp),
        "shift-down" => Box::new(list::ExtendDown),
        "home" => Box::new(list::First),
        "end" => Box::new(list::Last),
        "pageup" => Box::new(list::PageUp),
        "pagedown" => Box::new(list::PageDown),
        "enter" => Box::new(list::Activate),
        "ctrl-space" => Box::new(list::Pick),
        "menu" | "shift-f10" => Box::new(list::ShowMenu),
        _ => Box::new(list::Down),
    }
}

/// A button's tooltip as the Mac's buttons say it: what it does, then the
/// keys that do the same, "back (⌘[)", from the shortcut tables. Just what it
/// does where no key does.
pub fn help(what: &str, action: &dyn Action, cx: &App) -> String {
    let os = cx.platform_info().os;
    MENU.iter()
        .chain(APPLICATION)
        .chain(HOTKEYS)
        .find(|s| (s.action)().partial_eq(action))
        .and_then(|s| {
            s.keys
                .iter()
                .find(|k| os == OperatingSystem::Mac || !k.starts_with("cmd-"))
        })
        .map_or_else(
            || what.to_owned(),
            |key| format!("{what} ({})", caption(key, os)),
        )
}

/// A key as the sheet prints it: ⇧⌘K on macOS, in the order macOS prints
/// modifiers, and Ctrl+Shift+K elsewhere. A bare key is itself.
pub fn caption(key: &str, os: OperatingSystem) -> String {
    let mac = os == OperatingSystem::Mac;
    // A trailing "-" is the minus key, not a separator.
    let (mods, key) = match key.strip_suffix("--") {
        Some(mods) => (mods, "-"),
        None => key.rsplit_once('-').unwrap_or(("", key)),
    };
    let has = |m: &str| mods.split('-').any(|part| part == m);
    let platform = has("cmd") || (mac && has("secondary"));
    let control = has("ctrl") || (!mac && has("secondary"));
    let glyph = match (key, mac) {
        ("left", true) => "←".to_string(),
        ("right", true) => "→".to_string(),
        ("up", true) => "↑".to_string(),
        ("down", true) => "↓".to_string(),
        ("backspace", true) => "⌫".to_string(),
        ("left", false) => "Left".to_string(),
        ("right", false) => "Right".to_string(),
        ("backspace", false) => "Backspace".to_string(),
        ("delete", true) => "⌦".to_string(),
        ("delete", false) => "Delete".to_string(),
        ("escape", _) => "esc".to_string(),
        ("space", _) => "space".to_string(),
        (k, _) if mods.is_empty() => k.to_string(),
        // A key named in a word keeps the case the unmodified one is shown
        // in, "tab" beside "⇧tab"; on Linux it is capitalised, as "Ctrl" is.
        (k, true) if k.chars().count() > 1 => k.to_string(),
        (k, false) if k.chars().count() > 1 => {
            let mut chars = k.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
        (k, _) => k.to_uppercase(),
    };
    if mac {
        let mut out = String::new();
        for (on, sign) in [
            (control, "⌃"),
            (has("alt"), "⌥"),
            (has("shift"), "⇧"),
            (platform, "⌘"),
        ] {
            if on {
                out.push_str(sign);
            }
        }
        out + &glyph
    } else {
        let mut parts = Vec::new();
        for (on, name) in [
            (control, "Ctrl"),
            (has("alt"), "Alt"),
            (has("shift"), "Shift"),
            (platform, "Super"),
        ] {
            if on {
                parts.push(name);
            }
        }
        parts.push(&glyph);
        parts.join("+")
    }
}

/// Bind every shortcut and handle the commands that need only the engine.
pub fn register(engine: &Arc<KoanEngine>, cx: &mut App) {
    let os = cx.platform_info().os;
    let mac = os == OperatingSystem::Mac;
    let window = Rc::new(KeyBindingContextPredicate::parse(CONTEXT).expect("a valid context"));
    let menus = Rc::new(KeyBindingContextPredicate::parse(MENU_CONTEXT).expect("a valid context"));
    let bare = Rc::new(KeyBindingContextPredicate::parse(BARE_CONTEXT).expect("a valid context"));
    let mut bindings = Vec::new();
    // The menu tables first: the macOS menu shows an action's first binding.
    // The application's (quit, hide, settings) stay live over a sheet; the
    // rest wait until it is gone.
    for (shortcut, when) in MENU.iter().map(|s| (s, &menus)).chain(
        APPLICATION
            .iter()
            .filter(|s| mac || !s.keys[0].starts_with("cmd-"))
            .map(|s| (s, &window)),
    ) {
        for key in shortcut.keys {
            push(
                &mut bindings,
                key,
                (shortcut.action)(),
                Some(when.clone()),
                cx,
            );
        }
    }
    for shortcut in HOTKEYS {
        for key in shortcut.keys {
            push(
                &mut bindings,
                key,
                (shortcut.action)(),
                Some(bare.clone()),
                cx,
            );
        }
    }
    // Tab and F6 move between controls in a text field too, where the field
    // has no use for them; the bare table binds them outside fields only.
    // Tab stays inside a question or menu over the window (`step_focus`);
    // F6, which leaves for another region, is not bound there.
    let regions = Rc::new(
        KeyBindingContextPredicate::parse("koan && !Modal && !KoanMenu").expect("a valid context"),
    );
    for (key, action, when) in [
        ("tab", FocusNext.boxed_clone(), &window),
        ("shift-tab", FocusPrevious.boxed_clone(), &window),
        ("f6", NextRegion.boxed_clone(), &regions),
        ("shift-f6", PreviousRegion.boxed_clone(), &regions),
    ] {
        push(&mut bindings, key, action, Some(when.clone()), cx);
    }
    // Not under a menu or a question opened over the list: their keys are
    // their own, as the bare table's are.
    let lists = Rc::new(
        KeyBindingContextPredicate::parse("List && !Input && !Modal && !KoanMenu")
            .expect("a valid context"),
    );
    for shortcut in LISTS {
        for key in shortcut.keys {
            push(
                &mut bindings,
                key,
                list_binding(key),
                Some(lists.clone()),
                cx,
            );
        }
    }
    cx.bind_keys(bindings);

    let toggle = {
        let engine = engine.clone();
        move |cx: &mut App| send(cx, engine.clone().toggle_play_pause())
    };
    cx.on_action({
        let toggle = toggle.clone();
        move |_: &TogglePlayPause, cx| toggle(cx)
    });
    cx.on_action(move |_: &bare::TogglePlayPause, cx| toggle(cx));
    // Delete means whatever the page in front says it means. Global
    // handlers run while GPUI holds the window, so what reaches into it
    // waits until it is handed back.
    cx.on_action(|_: &bare::Delete, cx| cx.defer(|cx| cx.dispatch_action(&Delete)));
    let e = engine.clone();
    cx.on_action(move |_: &Next, cx| send(cx, e.clone().next()));
    let e = engine.clone();
    cx.on_action(move |_: &Previous, cx| send(cx, e.clone().previous()));
    let e = engine.clone();
    cx.on_action(move |_: &SkipForward, cx| skip(&e, SKIP_MS, cx));
    let e = engine.clone();
    cx.on_action(move |_: &SkipBack, cx| skip(&e, -SKIP_MS, cx));
    let e = engine.clone();
    cx.on_action(move |_: &SaveSession, cx| send(cx, e.clone().save_session()));
    let e = engine.clone();
    cx.on_action(move |_: &ClearQueue, cx| send(cx, e.clone().clear_queue()));
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Minimize, cx| {
        cx.defer(|cx| {
            if let Some(window) = cx.active_window() {
                let _ = window.update(cx, |_, window, _| window.minimize_window());
            }
        })
    });
    cx.on_action(|_: &Zoom, cx| {
        cx.defer(|cx| {
            if let Some(window) = cx.active_window() {
                let _ = window.update(cx, |_, window, _| window.zoom_window());
            }
        })
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
}

fn push(
    bindings: &mut Vec<KeyBinding>,
    key: &str,
    action: Box<dyn Action>,
    context: Option<Rc<KeyBindingContextPredicate>>,
    cx: &App,
) {
    match KeyBinding::load(key, action, context, false, None, cx) {
        Ok(binding) => bindings.push(binding),
        Err(e) => log::error!("shortcut {key}: {e}"),
    }
}

/// Run an engine command, logging a refusal.
fn send(
    cx: &mut App,
    command: impl std::future::Future<Output = Result<(), koan_runtime::KoanError>> + 'static,
) {
    cx.foreground_executor()
        .spawn(async move {
            if let Err(e) = command.await {
                log::warn!("{e}");
            }
        })
        .detach();
}

/// Move the playhead by `by_ms` from where it is now, once that is known.
fn skip(engine: &Arc<KoanEngine>, by_ms: i64, cx: &mut App) {
    let Some(now) = super::media::Playhead::now(cx) else {
        return;
    };
    let target = now.saturating_add_signed(by_ms);
    crate::transport::land(target, cx);
    send(cx, engine.clone().seek(target));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_captions_follow_the_order_macos_prints() {
        let mac = OperatingSystem::Mac;
        assert_eq!(caption("secondary-shift-k", mac), "⇧⌘K");
        assert_eq!(caption("secondary-alt-l", mac), "⌥⌘L");
        assert_eq!(caption("alt-left", mac), "⌥←");
        assert_eq!(caption("secondary-right", mac), "⌘→");
        assert_eq!(caption("secondary--", mac), "⌘-");
        assert_eq!(caption("secondary-/", mac), "⌘/");
        assert_eq!(caption("backspace", mac), "⌫");
        assert_eq!(caption("shift-tab", mac), "⇧tab");
        assert_eq!(caption("shift-f6", mac), "⇧f6");
    }

    #[test]
    fn other_captions_spell_their_modifiers() {
        let linux = OperatingSystem::Linux;
        assert_eq!(caption("secondary-shift-k", linux), "Ctrl+Shift+K");
        assert_eq!(caption("alt-left", linux), "Alt+Left");
        assert_eq!(caption("secondary--", linux), "Ctrl+-");
        assert_eq!(caption("shift-tab", linux), "Shift+Tab");
    }

    #[test]
    fn bare_keys_are_themselves() {
        let mac = OperatingSystem::Mac;
        assert_eq!(caption("G", mac), "G");
        assert_eq!(caption("?", mac), "?");
        assert_eq!(caption("space", mac), "space");
        assert_eq!(caption("escape", mac), "esc");
    }

    #[test]
    fn no_key_is_taken_twice() {
        let mut seen = std::collections::HashSet::new();
        for shortcut in MENU.iter().chain(APPLICATION).chain(HOTKEYS) {
            for key in shortcut.keys {
                assert!(seen.insert(*key), "{key} is bound twice");
            }
        }
    }

    #[test]
    fn every_key_parses() {
        for shortcut in MENU.iter().chain(APPLICATION).chain(HOTKEYS) {
            for key in shortcut.keys {
                for os in [OperatingSystem::Mac, OperatingSystem::Linux] {
                    koan_kit::app::Keystroke::parse_with_os(key, os)
                        .unwrap_or_else(|_| panic!("{key} does not parse"));
                }
            }
        }
    }

    /// The binding a keystroke dispatches with focus at `stack`, innermost
    /// last.
    fn winner(bindings: Vec<KeyBinding>, key: &'static str, stack: &[&str]) -> String {
        let keymap = koan_kit::app::platform::Keymap::new(bindings);
        let stack: Vec<koan_kit::app::KeyContext> = stack
            .iter()
            .map(|c| koan_kit::app::KeyContext::parse(c).unwrap())
            .collect();
        let (found, _) =
            keymap.bindings_for_input(&[koan_kit::app::Keystroke::parse(key).unwrap()], &stack);
        found
            .first()
            .map(|b| b.action().name().to_string())
            .unwrap_or_default()
    }

    koan_kit::app::actions!(field, [FieldCopy, FieldDeleteBack]);

    #[test]
    fn a_text_field_keeps_its_own_keys() {
        let bindings = || {
            vec![
                KeyBinding::new("cmd-c", FieldCopy, Some(INPUT_CONTEXT)),
                KeyBinding::new("backspace", FieldDeleteBack, Some(INPUT_CONTEXT)),
                KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
                KeyBinding::new("backspace", bare::Delete, Some(BARE_CONTEXT)),
            ]
        };
        let typing = ["Root", CONTEXT, INPUT_CONTEXT];
        assert_eq!(winner(bindings(), "cmd-c", &typing), "field::FieldCopy");
        assert_eq!(
            winner(bindings(), "backspace", &typing),
            "field::FieldDeleteBack"
        );
        let browsing = ["Root", CONTEXT];
        assert_eq!(winner(bindings(), "cmd-c", &browsing), "koan::Copy");
        assert_eq!(
            winner(bindings(), "backspace", &browsing),
            "koan_bare::Delete"
        );
    }

    koan_kit::app::actions!(modal, [ModalBack, ModalQuit]);

    /// Over a sheet or a question, neither a menu's shortcut nor a bare key
    /// reaches the page behind; the application's own still quit.
    #[test]
    fn nothing_acts_behind_a_sheet() {
        let bindings = || {
            vec![
                KeyBinding::new("cmd-[", ModalBack, Some(MENU_CONTEXT)),
                KeyBinding::new("cmd-q", ModalQuit, Some(CONTEXT)),
                KeyBinding::new("backspace", bare::Delete, Some(BARE_CONTEXT)),
            ]
        };
        let page = ["Root", CONTEXT];
        assert_eq!(winner(bindings(), "cmd-[", &page), "modal::ModalBack");
        assert_eq!(winner(bindings(), "backspace", &page), "koan_bare::Delete");
        for over in [
            ["Root", CONTEXT, koan_kit::components::sheet::MODAL, "Ask"],
            [
                "Root",
                CONTEXT,
                koan_kit::components::sheet::MODAL,
                "Shortcuts",
            ],
        ] {
            assert_eq!(winner(bindings(), "cmd-[", &over), "");
            assert_eq!(winner(bindings(), "backspace", &over), "");
            assert_eq!(winner(bindings(), "cmd-q", &over), "modal::ModalQuit");
        }
    }

    #[test]
    fn no_hotkey_carries_a_modifier() {
        for shortcut in HOTKEYS {
            for key in shortcut.keys {
                let k = koan_kit::app::Keystroke::parse_with_os(key, OperatingSystem::Mac).unwrap();
                let m = k.modifiers;
                assert!(
                    !(m.platform || m.control || m.alt),
                    "{key} is not a bare key"
                );
            }
        }
    }
}
