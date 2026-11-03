//! The Dock's menu on macOS: what is playing, its heart, repeat, shuffle,
//! play and pause, next and previous, the sleep timer, and the devices to
//! play on. Rebuilt from the mirror's Playback and Devices slices only when
//! what it shows changes; the playhead is a slice of its own and never
//! reaches it.

use std::sync::Arc;

use koan_ffi::{KoanEngine, RepeatMode, SleepTimer};
use koan_kit::app::{Action, App, Entity, Global};
// the platform's menu bar and Dock menu, which koan-kit does not draw.
use koan_kit::raw::{Menu, MenuItem};

use super::actions::{Next, Previous, TogglePlayPause};
use crate::mirror::{Devices, Mirror, Playback, Slice};

/// What a Dock menu item does, beyond the app's own actions.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = koan_dock, no_json)]
pub enum Dock {
    Favourite(i64),
    Repeat(RepeatMode),
    Shuffle(bool),
    Sleep(Option<SleepTimer>),
    /// Control another device, or with none this one again.
    PlayOn(Option<String>),
}

/// A Dock menu line that only says something, such as the playing track's
/// title. Nothing listens for it, so AppKit's validation, which asks whether
/// an item's action is handled, shows it disabled; an item marked disabled
/// with an action that is handled stays clickable.
#[derive(Clone, PartialEq, Debug, Default, Action)]
#[action(namespace = koan_dock, no_json)]
pub struct DockLabel;

/// Everything the menu shows: built only when this changes.
#[derive(Clone, PartialEq, Default)]
struct Shown {
    title: Option<String>,
    from: Option<String>,
    track: Option<i64>,
    favourite: bool,
    playing: bool,
    repeat: Option<RepeatMode>,
    shuffle: bool,
    sleeping: bool,
    devices: Vec<(String, String)>,
    target: Option<String>,
}

struct DockMenu {
    engine: Arc<KoanEngine>,
    shown: Option<Shown>,
    /// The favourite tracks' ids as last read, read again when the library
    /// changes or a favourite is toggled.
    favourites: Option<std::collections::HashSet<i64>>,
    /// Bumped by each read, so an answer to an older one is dropped.
    reads: u64,
}

impl Global for DockMenu {}

/// Follows the mirror and answers the menu's items. Once the mirror is
/// installed.
pub fn follow(engine: &Arc<KoanEngine>, cx: &mut App) {
    // the platform's menu bar and Dock menu, which koan-kit does not draw.
    if cx.platform_info().os != koan_kit::raw::OperatingSystem::Mac {
        return;
    }
    cx.set_global(DockMenu {
        engine: engine.clone(),
        shown: None,
        favourites: None,
        reads: 0,
    });
    let mirror = cx.global::<Mirror>();
    let (playback, devices, library) = (
        mirror.playback.clone(),
        mirror.devices.clone(),
        mirror.library.clone(),
    );
    cx.observe(&playback, |_, cx| rebuild(cx)).detach();
    cx.observe(&devices, |_, cx| rebuild(cx)).detach();
    cx.observe(&library, |_, cx| read_favourites(cx)).detach();
    cx.observe_global::<crate::library::feed::FavouritesMoved>(read_favourites)
        .detach();
    let e = engine.clone();
    cx.on_action(move |action: &Dock, cx| run(&e, action, cx));
    read_favourites(cx);
}

fn run(engine: &Arc<KoanEngine>, action: &Dock, cx: &mut App) {
    let engine = engine.clone();
    let action = action.clone();
    cx.spawn(async move |cx| {
        let done = match action {
            Dock::Favourite(id) => engine.toggle_favourite(id).await.map(|_| ()),
            Dock::Repeat(mode) => engine.set_repeat(mode).await,
            Dock::Shuffle(on) => engine.set_shuffle(on).await,
            Dock::Sleep(Some(timer)) => engine.set_sleep_timer(timer).await,
            Dock::Sleep(None) => engine.cancel_sleep_timer().await,
            Dock::PlayOn(id) => engine.control_device(id).await,
        };
        if let Err(e) = done {
            log::warn!("dock menu: {e}");
        }
        // A heart moved: the pages read again, and so does the menu.
        if matches!(action, Dock::Favourite(_)) {
            cx.update(crate::library::feed::favourites_moved);
        }
    })
    .detach();
}

/// Reads the favourite tracks again, then the menu.
fn read_favourites(cx: &mut App) {
    let menu = cx.global_mut::<DockMenu>();
    menu.reads += 1;
    let (engine, read) = (menu.engine.clone(), menu.reads);
    cx.spawn(async move |cx| {
        let Ok(ids) = engine.favourite_track_ids().await else {
            return;
        };
        cx.update(|cx| {
            let menu = cx.global_mut::<DockMenu>();
            if menu.reads != read {
                return;
            }
            menu.favourites = Some(ids.into_iter().collect());
            rebuild(cx);
        });
    })
    .detach();
}

fn shown(playback: &Entity<Playback>, devices: &Entity<Slice<Devices>>, cx: &App) -> Shown {
    let now = &playback.read(cx).0;
    let entry = now.entry.as_ref();
    let track = entry.and_then(|e| e.track_id);
    let favourites = cx.global::<DockMenu>().favourites.as_ref();
    let devices = &devices.read(cx).0;
    Shown {
        title: entry.map(|e| e.title.clone()),
        from: entry.map(|e| match (e.artist.is_empty(), e.album.is_empty()) {
            (false, false) => format!("{} — {}", e.artist, e.album),
            (false, true) => e.artist.clone(),
            (true, _) => e.album.clone(),
        }),
        track,
        favourite: track.is_some_and(|t| favourites.is_some_and(|ids| ids.contains(&t))),
        playing: now.state == koan_ffi::PlayState::Playing,
        repeat: Some(now.repeat_mode),
        shuffle: now.shuffle,
        sleeping: now.sleep.is_some(),
        devices: devices
            .devices
            .iter()
            .map(|d| (d.id.clone(), d.name.clone()))
            .collect(),
        target: devices.target.clone(),
    }
}

fn rebuild(cx: &mut App) {
    let mirror = cx.global::<Mirror>();
    let (playback, devices) = (mirror.playback.clone(), mirror.devices.clone());
    let next = shown(&playback, &devices, cx);
    if cx.global::<DockMenu>().shown.as_ref() == Some(&next) {
        return;
    }
    cx.set_dock_menu(items(&next));
    cx.global_mut::<DockMenu>().shown = Some(next);
}

fn choice(name: &str, checked: bool, action: impl Action) -> MenuItem {
    MenuItem::Action {
        name: name.to_owned().into(),
        action: Box::new(action),
        os_action: None,
        checked,
        disabled: false,
    }
}

fn header(name: String) -> MenuItem {
    MenuItem::Action {
        name: name.into(),
        action: Box::new(DockLabel),
        os_action: None,
        checked: false,
        disabled: true,
    }
}

fn items(s: &Shown) -> Vec<MenuItem> {
    let mut items = Vec::new();
    if let Some(title) = &s.title {
        items.push(header(title.clone()));
        if let Some(from) = s.from.as_ref().filter(|f| !f.is_empty()) {
            items.push(header(from.clone()));
        }
        if let Some(track) = s.track {
            items.push(choice("Favourite", s.favourite, Dock::Favourite(track)));
        }
        items.push(MenuItem::separator());
    }
    let repeat = s.repeat.unwrap_or(RepeatMode::Off);
    items.push(MenuItem::submenu(Menu::new("Repeat").items([
        choice(
            "Off",
            repeat == RepeatMode::Off,
            Dock::Repeat(RepeatMode::Off),
        ),
        choice(
            "Queue",
            repeat == RepeatMode::Queue,
            Dock::Repeat(RepeatMode::Queue),
        ),
        choice(
            "One",
            repeat == RepeatMode::One,
            Dock::Repeat(RepeatMode::One),
        ),
    ])));
    items.push(MenuItem::submenu(Menu::new("Shuffle").items([
        choice("Off", !s.shuffle, Dock::Shuffle(false)),
        choice("On", s.shuffle, Dock::Shuffle(true)),
    ])));
    items.push(MenuItem::separator());
    items.push(choice(
        if s.playing { "Pause" } else { "Play" },
        false,
        TogglePlayPause,
    ));
    items.push(choice("Next", false, Next));
    items.push(choice("Previous", false, Previous));
    items.push(MenuItem::separator());
    let mut sleep = vec![
        choice(
            "In 15 Minutes",
            false,
            Dock::Sleep(Some(SleepTimer::After { minutes: 15 })),
        ),
        choice(
            "In 30 Minutes",
            false,
            Dock::Sleep(Some(SleepTimer::After { minutes: 30 })),
        ),
        choice(
            "In 60 Minutes",
            false,
            Dock::Sleep(Some(SleepTimer::After { minutes: 60 })),
        ),
        choice(
            "At the End of the Track",
            false,
            Dock::Sleep(Some(SleepTimer::EndOfTrack)),
        ),
        choice(
            "At the End of the Record",
            false,
            Dock::Sleep(Some(SleepTimer::EndOfRecord)),
        ),
    ];
    if s.sleeping {
        sleep.push(MenuItem::separator());
        sleep.push(choice("Cancel", false, Dock::Sleep(None)));
    }
    items.push(MenuItem::submenu(Menu::new("Sleep").items(sleep)));
    let mut play_on = vec![choice("This Mac", s.target.is_none(), Dock::PlayOn(None))];
    for (id, name) in &s.devices {
        play_on.push(choice(
            name,
            s.target.as_ref() == Some(id),
            Dock::PlayOn(Some(id.clone())),
        ));
    }
    items.push(MenuItem::submenu(Menu::new("Play On").items(play_on)));
    items
}
