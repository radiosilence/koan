//! Play on: which kōan the transport, the queue and what is playing show and
//! command. This device, or another of the account's (through the server) or
//! the network's (nearby). Picking another pauses this one, and everything
//! the window shows becomes that device's, as the engine publishes its state
//! as this app's own, until another is picked. Nothing moves until "move
//! here", which sends what the device in view is playing to that row's
//! device and controls it there.
//!
//! What a device on the network that is not the account's may do (playback
//! and the queue only) the engine enforces at the other end; the picker
//! offers the same rows for both.

use koan_ffi::{DeviceInfo, DeviceReach, PlayState};
use koan_kit::app::*;

use crate::components::{Menu, MenuEntry};
use crate::engine::{engine, reported};
use crate::mirror::Mirror;
use crate::notice;
use crate::theme::{Icon, Tone};

/// What the control menu draws, read from the slices as it opens and again
/// whenever the devices move while it is open.
pub struct ControlView {
    pub devices: Vec<DeviceInfo>,
    /// The device controlled; `None` for this one.
    pub target: Option<String>,
    pub reach: Option<DeviceReach>,
    /// This device has a queue to move.
    pub has_queue: bool,
    /// The server passes commands between the account's devices.
    pub through_server: bool,
    /// The system keeps this app off the local network until it is allowed.
    pub network_blocked: bool,
    /// Unix seconds now, for "seen 5 min ago".
    pub now: i64,
}

impl ControlView {
    pub fn read(cx: &App) -> Self {
        let mirror = cx.global::<Mirror>();
        let devices = &mirror.devices.read(cx).0;
        Self {
            devices: devices.devices.clone(),
            target: devices.target.clone(),
            reach: devices.target_reach.clone(),
            has_queue: !mirror.queue.read(cx).items.is_empty(),
            through_server: mirror
                .connection
                .read(cx)
                .0
                .as_ref()
                .is_some_and(|c| c.devices),
            network_blocked: mirror
                .connection
                .read(cx)
                .0
                .as_ref()
                .is_some_and(|c| c.local_network_blocked),
            now: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64),
        }
    }

    pub fn controlled(&self) -> Option<&DeviceInfo> {
        let target = self.target.as_ref()?;
        self.devices.iter().find(|d| &d.id == target)
    }

    /// The device controlled, by name, while it is not this one.
    pub fn controlled_name(&self) -> Option<String> {
        self.target.as_ref().map(|_| {
            self.controlled()
                .map_or_else(|| "another device".to_owned(), |d| d.name.clone())
        })
    }

    /// The device controlled cannot be reached: commands fail until it is
    /// back.
    pub fn out_of_reach(&self) -> bool {
        matches!(self.reach, Some(DeviceReach::OutOfReach { .. }))
    }

    fn can_move(&self, to: Option<&DeviceInfo>) -> bool {
        koan_ffi::can_move_music(to.cloned(), self.controlled().cloned(), self.has_queue)
    }
}

/// What this computer is called in the picker.
const THIS_DEVICE: &str = if cfg!(target_os = "macos") {
    "this mac"
} else {
    "this computer"
};

/// A device's glyph by its platform, as the Mac's picker gives it.
pub fn platform_icon(platform: &str) -> Icon {
    match platform {
        "ios" => Icon::Phone,
        "tvos" => Icon::Television,
        "macos" => Icon::Laptop,
        _ => Icon::Desktop,
    }
}

/// The control menu: this device, then every other the engine lists, each
/// with what it is doing; the one in view ticked, a "move here" button on
/// each the music can go to.
pub fn control_menu(menu: Menu, view: &ControlView) -> Menu {
    let here = view.target.is_none();
    let mut this = MenuEntry::new(THIS_DEVICE)
        .icon(platform_icon(std::env::consts::OS))
        .rich(if here {
            ""
        } else {
            "paused while you control another device"
        })
        .checked(here)
        .stays_open()
        .on_click(|_, _, cx| control(None, cx));
    if !here && view.can_move(None) {
        this = this.text_action(
            "move here",
            "bring what the other device is playing back here",
            |_, _, cx| move_music(None, cx),
        );
    }
    let mut menu = menu.min_width(Menu::PICKER_WIDTH).section("control");
    // The system keeps the app off the local network until the person allows
    // it, and says nothing otherwise: without this the list would just be
    // short.
    if view.network_blocked {
        menu = menu.item(
            MenuEntry::new("kōan can't see this network")
                .icon(Icon::NetworkBlocked)
                .rich("Allow Local Network for kōan in System Settings → Privacy & Security to find devices here.")
                .warns(true)
                .disabled(true),
        );
    }
    let mut menu = menu.item(this);
    for device in &view.devices {
        let selected = view.target.as_deref() == Some(device.id.as_str());
        let unreachable = device.problem.is_some() || (!device.awake && !device.wakeable);
        let warning = device.waking.is_some() || device.wake_failed.is_some();
        let detail = match &device.owner {
            Some(owner) => format!("shared by {owner} · {}", detail(device, view.now)),
            None => detail(device, view.now),
        };
        // An Apple TV asleep that nothing wakes yet is chosen to set its
        // waking up: the pairing is what makes it reachable.
        let pair_first = unreachable && device.tv_pairable && !device.tv_paired;
        let id = device.id.clone();
        let name = device.name.clone();
        let mut entry = MenuEntry::data(device.name.clone())
            .icon(platform_icon(&device.platform))
            .reach(if device.nearby {
                Icon::Wifi
            } else {
                Icon::Cloud
            })
            .rich(detail)
            .warns(unreachable || warning)
            .checked(selected)
            .disabled(unreachable && !selected && !pair_first)
            .on_click(move |_, window, cx| {
                if pair_first {
                    crate::tv_wake::open(id.clone(), name.clone(), window, cx);
                } else {
                    control(Some(id.clone()), cx);
                }
            });
        // Choosing a device keeps the menu up to show it waking; the pairing
        // sheet needs the menu out of its way.
        if !pair_first {
            entry = entry.stays_open();
        }
        // Secondary actions first, so "move here" takes the last slot on
        // every row it is on.
        // An Apple TV can be paired to be turned on from here, once, for
        // all the account's devices.
        if device.tv_pairable {
            let (id, name, paired) = (device.id.clone(), device.name.clone(), device.tv_paired);
            entry = entry.action(
                if paired {
                    Icon::Close
                } else {
                    Icon::Television
                },
                if paired {
                    "stop waking this TV from here"
                } else {
                    "set up waking this TV from here…"
                },
                move |_, window, cx| {
                    if paired {
                        crate::tv_wake::stop(id.clone(), name.clone(), window, cx);
                    } else {
                        crate::tv_wake::open(id.clone(), name.clone(), window, cx);
                    }
                },
            );
        }
        // A device out of reach can be dropped from the list until it is
        // heard from again.
        if !device.awake {
            let id = device.id.clone();
            entry = entry.action(
                Icon::Clear,
                "forget this device until it is heard from again",
                move |_, _, cx| {
                    let id = id.clone();
                    reported(cx, "forget device", move |e| e.forget_device(id));
                },
            );
        }
        if view.can_move(Some(device)) {
            let to = device.id.clone();
            entry = entry.text_action(
                "move here",
                format!(
                    "send what is playing to {}, and control it there",
                    device.name
                ),
                move |_, _, cx| move_music(Some(to.clone()), cx),
            );
        }
        menu = menu.item(entry);
    }
    if view.devices.is_empty() {
        menu = menu.note(if view.through_server {
            "No other kōan devices. Open kōan on another device signed in to this server, or on this network."
        } else {
            "No other kōan devices on this network. Devices signed in to one kōan server reach each other through it, on any network."
        });
    }
    menu
}

fn control(id: Option<String>, cx: &mut App) {
    reported(cx, "control device", move |e| e.control_device(id));
}

/// Moves the music, then says what became of it where the device's state
/// cannot: refused, waiting for a device asleep, or tracks left behind.
fn move_music(to: Option<String>, cx: &mut App) {
    let view = ControlView::read(cx);
    let name = to
        .as_ref()
        .and_then(|id| view.devices.iter().find(|d| &d.id == id))
        .map_or_else(|| "This device".to_owned(), |d| d.name.clone());
    // Moved here, the answer is the controlled device's, which was asked to
    // send the music on. Read before the move makes this device the one
    // controlled.
    // `None` while this device is in view: the music leaves from here.
    let from = view.target.as_ref().map(|_| {
        view.controlled()
            .map_or_else(|| "The other device".to_owned(), |d| d.name.clone())
    });
    let here = to.is_none();
    let moving = engine(cx).move_music(to);
    cx.spawn(async move |cx| {
        let said = match moving.await {
            Err(e) => Some((e.to_string(), Tone::Bad)),
            Ok(moved) => moved_notice(&moved, here, &name, from.as_deref()),
        };
        if let Some((text, tone)) = said {
            cx.update(|cx| notice::show(text, tone, cx));
        }
    })
    .detach();
}

fn moved_notice(
    moved: &koan_ffi::MoveResult,
    here: bool,
    name: &str,
    from: Option<&str>,
) -> Option<(String, Tone)> {
    let source = from.unwrap_or("This device");
    if let Some(error) = &moved.error {
        return Some((
            if here {
                format!("{source} did not send the music here: {error}")
            } else {
                format!("{name} did not take the music: {error}")
            },
            Tone::Bad,
        ));
    }
    if moved.queued {
        return Some((
            if here {
                format!("{source} is asleep. It sends the music here when it wakes.")
            } else {
                format!(
                    "{name} is asleep. It takes the music when it wakes; until then the music is paused where it was."
                )
            },
            Tone::Muted,
        ));
    }
    if !moved.started {
        return Some((
            format!(
                "Sent to {name}, which has not started it yet. It may when it wakes; until then the music is paused where it was."
            ),
            Tone::Muted,
        ));
    }
    // Left behind where the music came from: the device controlled, or this
    // one while it was in view.
    let behind = from.unwrap_or("this device");
    match moved.left_out {
        0 => None,
        1 => Some((
            format!("1 track only on {behind} stayed behind"),
            Tone::Muted,
        )),
        n => Some((
            format!("{n} tracks only on {behind} stayed behind"),
            Tone::Muted,
        )),
    }
}

/// What a device row says under its name: why it cannot be reached, how
/// waking it is going, or what it is playing.
fn detail(device: &DeviceInfo, now: i64) -> String {
    if let Some(problem) = &device.problem {
        return problem.clone();
    }
    if let Some(stage) = &device.waking {
        return match stage.as_str() {
            "tv" => "waking… turning the Apple TV on".into(),
            "network" => "waking… trying it on this network".into(),
            "push" => "waking… sent a wake through your server".into(),
            _ => format!("waking… tap the notification on {}", device.name),
        };
    }
    if !device.awake && !device.asleep {
        return "reconnecting…".into();
    }
    if !device.awake {
        let seen = device
            .last_seen
            .map(|at| format!(" · seen {}", crate::time::ago(at, now)))
            .unwrap_or_default();
        if let Some(failed) = &device.wake_failed {
            return format!("{failed} Asleep{seen}.");
        }
        if device.tv_paired && device.wakeable {
            return format!("asleep{seen}. Choosing it turns the TV on and opens kōan.");
        }
        if device.wakeable {
            return format!(
                "asleep{seen}. Choosing it wakes it; music sent there arrives as a notification to tap."
            );
        }
        if device.tv_pairable {
            return format!("asleep{seen}. Choosing it sets it up to be turned on from here.");
        }
        // A computer drops out when kōan is quit or it is off, and nothing
        // reaches it until someone opens kōan there.
        return if matches!(device.platform.as_str(), "macos" | "linux") {
            format!("not running{seen}. It can't be woken from here.")
        } else {
            format!("asleep{seen}")
        };
    }
    let playing = [
        device.title.clone().unwrap_or_default(),
        device
            .artist_credit
            .iter()
            .map(|p| p.text.as_str())
            .collect::<String>(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" — ");
    let library = if device.same_library {
        ""
    } else {
        " · different library"
    };
    match device.state {
        PlayState::Playing => format!("playing {playing}{library}"),
        PlayState::Paused => format!("paused · {playing}{library}"),
        PlayState::Stopped => format!("idle{library}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(started: bool, queued: bool, left_out: u32) -> koan_ffi::MoveResult {
        koan_ffi::MoveResult {
            left_out,
            started,
            queued,
            error: None,
        }
    }

    #[test]
    fn a_move_says_only_what_the_state_cannot() {
        assert_eq!(
            moved_notice(&moved(true, false, 0), false, "TV", None),
            None
        );
        let (text, tone) = moved_notice(&moved(true, false, 2), false, "TV", None).unwrap();
        assert_eq!(text, "2 tracks only on this device stayed behind");
        assert_eq!(tone, Tone::Muted);
        // Brought here, what stays behind is the other device's.
        let (text, _) =
            moved_notice(&moved(true, false, 1), true, "This device", Some("Phone")).unwrap();
        assert_eq!(text, "1 track only on Phone stayed behind");
        let (text, _) =
            moved_notice(&moved(false, true, 0), true, "This device", Some("Phone")).unwrap();
        assert!(text.starts_with("Phone is asleep"), "{text}");
        let mut refused = moved(false, false, 0);
        refused.error = Some("different library".into());
        let (text, tone) = moved_notice(&refused, false, "TV", Some("Mac")).unwrap();
        assert_eq!(text, "TV did not take the music: different library");
        assert_eq!(tone, Tone::Bad);
    }
}
