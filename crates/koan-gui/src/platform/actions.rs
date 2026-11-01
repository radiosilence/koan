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

use gpui_kit::{Action, App, KeyBinding, KeyBindingContextPredicate, OperatingSystem, actions};
use koan_ffi::KoanEngine;

/// The key context the shell's root element carries.
pub const CONTEXT: &str = "koan";

/// Where bare keys are live: inside the window, outside any text field.
const BARE_CONTEXT: &str = "koan && !Input";

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
        ToggleLyrics,
        Bigger,
        Smaller,
        ActualSize,
        ShowShortcuts,
        ZoomCover,
        ClearSelection,
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
    gpui_kit::actions!(koan_bare, [TogglePlayPause, Delete]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Playback,
    Navigation,
    View,
    Edit,
    Library,
}

impl Group {
    pub const ALL: [Group; 5] = [
        Group::Playback,
        Group::Navigation,
        Group::View,
        Group::Edit,
        Group::Library,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::Playback => "playback",
            Group::Navigation => "navigation",
            Group::View => "view",
            Group::Edit => "edit",
            Group::Library => "library",
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
    shortcut!("Add Music…", ["secondary-shift-k"], Navigation, AddMusic),
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
    shortcut!("Add music", ["p"], Navigation, AddMusic),
    shortcut!("Search library", ["/"], Navigation, Search),
    shortcut!("Albums", ["l", "a"], Navigation, ShowAlbums),
    shortcut!("Artists", ["r"], Navigation, ShowArtists),
    shortcut!("Queue", ["q"], Navigation, ShowQueue),
    shortcut!("Top of the queue", ["g"], Navigation, TopOfQueue),
    shortcut!("End of the queue", ["G"], Navigation, EndOfQueue),
    shortcut!("Lyrics panel", ["L"], View, ToggleLyrics),
    shortcut!("Zoom the cover", ["z"], View, ZoomCover),
    shortcut!("Clear selection", ["escape"], View, ClearSelection),
    shortcut!("Delete", ["backspace"], Edit, bare::Delete),
    shortcut!("This list", ["?"], View, ShowShortcuts),
];

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
        ("escape", _) => "esc".to_string(),
        ("space", _) => "space".to_string(),
        (k, _) if mods.is_empty() => k.to_string(),
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
    let bare = Rc::new(KeyBindingContextPredicate::parse(BARE_CONTEXT).expect("a valid context"));
    let mut bindings = Vec::new();
    // The menu tables first: the macOS menu shows an action's first binding.
    for shortcut in MENU.iter().chain(
        APPLICATION
            .iter()
            .filter(|s| mac || !s.keys[0].starts_with("cmd-")),
    ) {
        for key in shortcut.keys {
            push(
                &mut bindings,
                key,
                (shortcut.action)(),
                Some(window.clone()),
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
    // Delete means whatever the page in front says it means.
    cx.on_action(|_: &bare::Delete, cx| cx.dispatch_action(&Delete));
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
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.minimize_window());
        }
    });
    cx.on_action(|_: &Zoom, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.zoom_window());
        }
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
    command: impl std::future::Future<Output = Result<(), koan_ffi::KoanError>> + 'static,
) {
    cx.foreground_executor()
        .spawn(async move {
            if let Err(e) = command.await {
                log::warn!("{e}");
            }
        })
        .detach();
}

/// Move the playhead by `by_ms` from where it is now.
fn skip(engine: &Arc<KoanEngine>, by_ms: i64, cx: &mut App) {
    let to = super::media::Playhead::now(cx).saturating_add_signed(by_ms);
    send(cx, engine.clone().seek(to));
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
    }

    #[test]
    fn other_captions_spell_their_modifiers() {
        let linux = OperatingSystem::Linux;
        assert_eq!(caption("secondary-shift-k", linux), "Ctrl+Shift+K");
        assert_eq!(caption("alt-left", linux), "Alt+Left");
        assert_eq!(caption("secondary--", linux), "Ctrl+-");
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
                    gpui_kit::Keystroke::parse_with_os(key, os)
                        .unwrap_or_else(|_| panic!("{key} does not parse"));
                }
            }
        }
    }

    /// The binding a keystroke dispatches with focus at `stack`, innermost
    /// last.
    fn winner(bindings: Vec<KeyBinding>, key: &'static str, stack: &[&str]) -> String {
        let keymap = gpui_kit::Keymap::new(bindings);
        let stack: Vec<gpui_kit::KeyContext> = stack
            .iter()
            .map(|c| gpui_kit::KeyContext::parse(c).unwrap())
            .collect();
        let (found, _) =
            keymap.bindings_for_input(&[gpui_kit::Keystroke::parse(key).unwrap()], &stack);
        found
            .first()
            .map(|b| b.action().name().to_string())
            .unwrap_or_default()
    }

    gpui_kit::actions!(field, [FieldCopy, FieldDeleteBack]);

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

    #[test]
    fn no_hotkey_carries_a_modifier() {
        for shortcut in HOTKEYS {
            for key in shortcut.keys {
                let k = gpui_kit::Keystroke::parse_with_os(key, OperatingSystem::Mac).unwrap();
                let m = k.modifiers;
                assert!(
                    !(m.platform || m.control || m.alt),
                    "{key} is not a bare key"
                );
            }
        }
    }
}
