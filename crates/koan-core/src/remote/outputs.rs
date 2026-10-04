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
    /// Its DSP profiles, which any of its outputs can be given, and whether
    /// processing is on at all.
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
    /// The DSP profile it plays through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
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
/// notification, and when an output menu opens. Rings the engine's change
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

/// This device's outputs, as they are now.
pub fn local(state: &SharedPlayerState) -> LinkOutputs {
    let cfg = Config::cached();
    let preset = |device: &str| {
        cfg.dsp
            .profiles
            .iter()
            .find(|p| p.devices.iter().any(|d| d == device))
            .map(|p| p.name.clone())
    };
    let devices = DEVICES
        .lock()
        .get_or_insert_with(list_devices)
        .iter()
        .map(|(name, kind)| LinkOutput {
            preset: preset(name),
            id: name.clone(),
            name: name.clone(),
            kind: kind.clone(),
            ..Default::default()
        })
        .collect();
    let renderers = crate::upnp::discovery::renderers()
        .into_iter()
        .map(|r| LinkOutput {
            preset: preset(r.device_name()),
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
        profiles: cfg.dsp.profiles.iter().map(|p| p.name.clone()).collect(),
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

/// Play `device` through `profile`, or untouched, and apply it where
/// playback is.
pub fn set_preset(
    device: &str,
    profile: Option<&str>,
    player: &crossbeam_channel::Sender<PlayerCommand>,
) -> Result<(), String> {
    crate::audio::dsp::profiles::assign(profile, device)?;
    player
        .send(PlayerCommand::ReloadDsp)
        .map_err(|_| "The player has stopped.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        ] {
            assert!(!cmd.allowed_nearby(), "{cmd:?}");
        }
        assert!(LinkCommand::Pause.allowed_nearby());
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
