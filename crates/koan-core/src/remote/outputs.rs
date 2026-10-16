//! What a device plays through, told to the devices controlling it, and
//! changed by them.
//!
//! A device publishes its outputs in its `LinkState`: its own audio devices
//! (on a phone, the route the system chose), the UPnP renderers it can see,
//! which of them plays now, and each one's DSP preset. A device controlling it
//! sends `SetOutput`, `SetRendererVolume` or `SetPreset`, and `set`,
//! `set_volume` and `set_preset` carry them out exactly as the device's own
//! menu does, so the music carries on from where it is, playing or paused.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::player::commands::PlayerCommand;
use crate::player::state::SharedPlayerState;

/// What a device can play through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkOutputs {
    /// Its own audio devices: on a phone, the route the system chose.
    #[serde(default)]
    pub devices: Vec<LinkOutput>,
    /// The UPnP renderers it can see.
    #[serde(default)]
    pub renderers: Vec<LinkOutput>,
    /// What it plays through now.
    #[serde(default)]
    pub current: OutputChoice,
    /// The volume of the renderer it plays to, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    /// Its EQ presets, which any of its outputs can be set from, and
    /// whether processing is on at all (always, since a device is made flat
    /// instead; kept for apps from before).
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub dsp_enabled: bool,
}

/// One output: a device by its name, or a renderer by its UDN.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkOutput {
    pub id: String,
    pub name: String,
    /// How it is connected: `usb`, `bluetooth`, `upnp` and the rest.
    #[serde(default)]
    pub kind: String,
    /// What to say under its name: a renderer's make and model.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Playing or paused for something else: picking it takes it over.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub busy: bool,
    /// The EQ preset it was set from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Its EQ is not a preset as saved: changed since, or never saved. Flat
    /// is neither this nor a preset.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsaved: bool,
}

/// An output to play through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OutputChoice {
    /// The system's default output.
    #[default]
    Default,
    Device {
        name: String,
    },
    Renderer {
        udn: String,
    },
}

/// This device's audio devices, by name and kind, as last listed. Read on
/// every change to the engine, so listed only when they may have changed:
/// see `refresh_devices`.
static DEVICES: parking_lot::Mutex<Option<Vec<(String, String)>>> = parking_lot::Mutex::new(None);

/// List this device's audio devices again: on the platform's device-change
/// notification, when an output menu opens, and on a phone when the route
/// changes. Rings the engine's change
/// signal if they moved, so every view of them follows.
pub fn refresh_devices() {
    let now = list_devices();
    let mut cached = DEVICES.lock();
    if cached.as_ref() != Some(&now) {
        *cached = Some(now);
        drop(cached);
        crate::signal::engine_changed().bump();
    }
}

fn list_devices() -> Vec<(String, String)> {
    crate::audio::list_output_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| (d.name, d.kind.as_str().to_string()))
        .collect()
}

/// Refreshes a controller asked for: one at a time, and at most one more
/// waiting, so a burst of asking costs two at most.
#[derive(Default)]
struct RefreshGate {
    running: bool,
    queued: bool,
}

impl RefreshGate {
    /// Whether to start a refresh now. Asked during one, it is queued.
    fn ask(&mut self) -> bool {
        if self.running {
            self.queued = true;
            return false;
        }
        self.running = true;
        true
    }

    /// A refresh ended. Whether to run the one queued meanwhile.
    fn done(&mut self) -> bool {
        self.running = std::mem::take(&mut self.queued);
        self.running
    }
}

static REFRESH: parking_lot::Mutex<RefreshGate> = parking_lot::Mutex::new(RefreshGate {
    running: false,
    queued: false,
});

/// A controller opened its output menu: list this device's outputs again, on
/// a thread of its own rather than the link's. Cheap by design: on a Mac a
/// CoreAudio query; on a phone or a television the route the app last
/// reported, with no search, since the system knows it directly; renderers
/// from the discovery cache, searched for only once it is stale. What moved
/// reaches the controller in the link state.
pub fn refresh_for_controller() {
    if !REFRESH.lock().ask() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("koan-outputs".into())
        .spawn(|| {
            loop {
                refresh_devices();
                #[cfg(not(any(target_os = "ios", target_os = "tvos")))]
                crate::upnp::discovery::search_if_stale();
                if !REFRESH.lock().done() {
                    break;
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("outputs: no refresh thread: {e}");
        REFRESH.lock().done();
    }
}

/// The preset `device` was set from, and whether its EQ is unsaved. EQ is
/// keyed by the device's name, which on a phone is the route's port name, so
/// the preset published for an output is the one its audio goes through.
fn preset(dsp: &crate::config::DspConfig, device: &str) -> (Option<String>, bool) {
    let flat = dsp.profile_for(device).is_none() && !dsp.tunings.iter().any(|t| t.device == device);
    match crate::audio::dsp::profiles::preset_for(device) {
        Some((name, edited)) => (Some(name), edited),
        None => (None, !flat),
    }
}

/// Audio devices as published: each by its name, which is also its id, since
/// a name is what `SetOutput` and `SetPreset` address it by.
fn device_outputs(devices: &[(String, String)], dsp: &crate::config::DspConfig) -> Vec<LinkOutput> {
    devices
        .iter()
        .map(|(name, kind)| {
            let (preset, unsaved) = preset(dsp, name);
            LinkOutput {
                preset,
                unsaved,
                id: name.clone(),
                name: name.clone(),
                kind: kind.clone(),
                ..Default::default()
            }
        })
        .collect()
}

/// This device's outputs, as they are now.
pub fn local(state: &SharedPlayerState) -> LinkOutputs {
    let cfg = Config::cached();
    let devices = device_outputs(DEVICES.lock().get_or_insert_with(list_devices), &cfg.dsp);
    let renderers = crate::upnp::discovery::renderers()
        .into_iter()
        .map(|r| {
            let (preset, unsaved) = preset(&cfg.dsp, r.device_name());
            LinkOutput {
                preset,
                unsaved,
                busy: crate::upnp::discovery::busy(&r.udn),
                detail: [r.manufacturer.as_str(), r.model.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" "),
                id: r.udn,
                name: r.name,
                kind: "upnp".into(),
            }
        })
        .collect();
    let renderer = state.renderer();
    LinkOutputs {
        devices,
        renderers,
        current: match (&renderer, &cfg.playback.output_device) {
            (Some(r), _) => OutputChoice::Renderer { udn: r.udn.clone() },
            (None, Some(name)) => OutputChoice::Device { name: name.clone() },
            (None, None) => OutputChoice::Default,
        },
        volume: renderer.and_then(|r| r.volume),
        profiles: cfg
            .dsp
            .profiles
            .iter()
            .filter(|p| p.preset)
            .map(|p| p.name.clone())
            .collect(),
        dsp_enabled: cfg.dsp.enabled,
    }
}

/// Play through `output` from now on. `choice` is what `upnp::choose`
/// returned when it was picked, taken before anything that could reorder it:
/// a renderer is a session to open, a few round trips that block here, and a
/// later pick drops it. Everything else reaches the player in order.
pub fn set(
    output: OutputChoice,
    choice: u64,
    player: &crossbeam_channel::Sender<PlayerCommand>,
) -> Result<(), String> {
    let send = |cmd| {
        player
            .send(cmd)
            .map_err(|_| "The player has stopped.".to_string())
    };
    match output {
        OutputChoice::Default => send(PlayerCommand::ClearOutputDevice),
        OutputChoice::Device { name } => send(PlayerCommand::SetOutputDevice(name)),
        OutputChoice::Renderer { udn } => crate::upnp::connect(&udn, choice, player),
    }
}

/// Set `device` from the preset `profile`, or flat, and apply it where
/// playback is. A correction's name, from an app before presets, is chosen
/// as the correction.
pub fn set_preset(
    device: &str,
    profile: Option<&str>,
    player: &crossbeam_channel::Sender<PlayerCommand>,
) -> Result<(), String> {
    match profile {
        None => crate::audio::dsp::profiles::apply_preset(device, None)?,
        Some(name) => crate::audio::dsp::profiles::assign(Some(name), device)?,
    }
    player
        .send(PlayerCommand::ReloadDsp)
        .map_err(|_| "The player has stopped.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DspProfile;
    use crate::remote::link::{LinkCommand, LinkState};

    /// A device's outputs survive the trip through the link, and a device
    /// that publishes none (an older one) reads as having none.
    #[test]
    fn outputs_cross_the_link_whole() {
        let state = LinkState {
            playing: true,
            outputs: Some(LinkOutputs {
                devices: vec![LinkOutput {
                    id: "Topping E30".into(),
                    name: "Topping E30".into(),
                    kind: "usb".into(),
                    preset: Some("Harman".into()),
                    ..Default::default()
                }],
                renderers: vec![LinkOutput {
                    id: "uuid:arcam".into(),
                    name: "Arcam".into(),
                    kind: "upnp".into(),
                    detail: "Arcam SA30".into(),
                    ..Default::default()
                }],
                current: OutputChoice::Renderer {
                    udn: "uuid:arcam".into(),
                },
                volume: Some(30),
                profiles: vec!["Harman".into()],
                dsp_enabled: true,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<LinkState>(&json).unwrap(), state);

        let older: LinkState = serde_json::from_str(r#"{"playing":false}"#).unwrap();
        assert_eq!(older.outputs, None);

        let cmd = LinkCommand::SetOutput {
            output: OutputChoice::Device {
                name: "Topping E30".into(),
            },
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(serde_json::from_str::<LinkCommand>(&json).unwrap(), cmd);
    }

    /// A phone publishes its route by the port's own name, with the preset
    /// that route was set from: what `SetPreset` from another device then
    /// sets it from, under the same name. A route with EQ no preset holds is
    /// unsaved; one with none is flat.
    #[test]
    fn a_route_is_published_by_name_with_its_preset() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::config::set_config_dir(dir.path());
        Config::persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "Qudelix Harman".into(),
                devices: vec!["Qudelix-5K".into(), "Car".into()],
                role: Some(crate::config::DspRole::Correction),
                ..Default::default()
            })
        })
        .unwrap();
        crate::audio::dsp::profiles::save_preset("Qudelix-5K", "Harman").unwrap();
        let dsp = Config::cached().dsp.clone();
        let route = [("Qudelix-5K".to_string(), String::new())];
        assert_eq!(
            device_outputs(&route, &dsp),
            vec![LinkOutput {
                id: "Qudelix-5K".into(),
                name: "Qudelix-5K".into(),
                preset: Some("Harman".into()),
                ..Default::default()
            }]
        );
        let car = [("Car".to_string(), String::new())];
        assert_eq!(device_outputs(&car, &dsp)[0].preset, None);
        assert!(device_outputs(&car, &dsp)[0].unsaved);
        let speaker = [("Speaker".to_string(), String::new())];
        assert_eq!(device_outputs(&speaker, &dsp)[0].preset, None);
        assert!(!device_outputs(&speaker, &dsp)[0].unsaved, "flat");
    }

    /// Anyone on the network may play and pause, but where the sound goes,
    /// how loud the amplifier is and what a preset does are the account's.
    #[test]
    fn a_stranger_on_the_network_cannot_move_the_output() {
        for cmd in [
            LinkCommand::SetOutput {
                output: OutputChoice::Default,
            },
            LinkCommand::SetRendererVolume { volume: 100 },
            LinkCommand::SetPreset {
                device: "Speakers".into(),
                profile: None,
            },
            LinkCommand::RefreshOutputs,
        ] {
            assert!(!cmd.allowed_nearby(), "{cmd:?}");
        }
        assert!(LinkCommand::Pause.allowed_nearby());
    }

    /// An output menu opened on a controller asks the controlled device to
    /// list its outputs again: a command one device gives another, relayed by
    /// the server, and run under Full control.
    #[test]
    fn a_controller_can_ask_for_the_outputs_again() {
        let cmd = LinkCommand::RefreshOutputs;
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(serde_json::from_str::<LinkCommand>(&json).unwrap(), cmd);
        assert!(cmd.relayable());
        assert!(cmd.allowed_playback());
    }

    /// One refresh at a time and one more at most while it runs, however
    /// often controllers ask.
    #[test]
    fn refreshes_for_controllers_coalesce() {
        let mut gate = RefreshGate::default();
        assert!(gate.ask());
        assert!(!gate.ask());
        assert!(!gate.ask());
        // The burst ran once more, then nothing.
        assert!(gate.done());
        assert!(!gate.done());
        assert!(gate.ask());
        assert!(!gate.done());
    }

    /// Asking for the outputs again is said live or not at all: never queued
    /// for a device that is away, never a push to wake one.
    #[test]
    fn a_refresh_is_never_queued_for_an_absent_device() {
        assert!(LinkCommand::RefreshOutputs.live_only());
        assert!(
            LinkCommand::Shared {
                command: Box::new(LinkCommand::RefreshOutputs)
            }
            .live_only()
        );
        assert!(!LinkCommand::Pause.live_only());
        assert!(!crate::remote::devices::send_live(
            "nowhere-at-all",
            LinkCommand::RefreshOutputs
        ));
    }

    /// A device switch reaches the player as the device's own menu sends it.
    #[test]
    fn setting_a_device_output_is_the_menus_switch() {
        let (tx, rx) = crossbeam_channel::unbounded();
        set(
            OutputChoice::Device {
                name: "Topping E30".into(),
            },
            crate::upnp::choose(),
            &tx,
        )
        .unwrap();
        assert!(matches!(
            rx.try_recv(),
            Ok(PlayerCommand::SetOutputDevice(name)) if name == "Topping E30"
        ));
        set(OutputChoice::Default, crate::upnp::choose(), &tx).unwrap();
        assert!(matches!(
            rx.try_recv(),
            Ok(PlayerCommand::ClearOutputDevice)
        ));
    }
}
