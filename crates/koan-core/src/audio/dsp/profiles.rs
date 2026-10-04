//! The profiles in `config.local.toml`, as every front end changes them.
//!
//! Imported responses are kept under `dsp/<profile>/` beside the config, one
//! 32-bit float WAV per rate, or a `.cfg` with it where routes mix or delay
//! channels. Whatever they were imported from — a Roon zip, a CamillaDSP
//! setup, a share from another app — is read once and not needed again.

use std::path::{Path, PathBuf};

use super::import::Imported;
use super::{Setup, convolver, raw};
use crate::config::{self, Config, DspProfile};

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub name: String,
    pub devices: Vec<String>,
    pub bands: usize,
    /// Rates there are responses for.
    pub rates: Vec<u32>,
    /// Why the profile would not load, if it would not.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub enabled: bool,
    /// The output device playback goes to.
    pub device: Option<String>,
    /// The profile for that device.
    pub active: Option<String>,
    pub profiles: Vec<Summary>,
}

/// The device the player resolves to: the configured one if it is there, or
/// the system's.
pub fn current_device() -> Option<String> {
    let cfg = Config::cached();
    if let Some(name) = &cfg.playback.output_device
        && crate::audio::list_output_devices()
            .is_ok_and(|devices| devices.iter().any(|d| &d.name == name))
    {
        return Some(name.clone());
    }
    crate::audio::default_output_device().ok().map(|d| d.name)
}

pub fn overview() -> Overview {
    overview_for(current_device())
}

/// The profiles, and which `device` plays through. A front end that knows
/// a renderer is the output names it by its UDN here.
pub fn overview_for(device: Option<String>) -> Overview {
    let cfg = Config::cached();
    let base = config::config_dir();
    Overview {
        enabled: cfg.dsp.enabled,
        active: device
            .as_deref()
            .and_then(|d| cfg.dsp.profile_for(d))
            .map(|p| p.name.clone()),
        device,
        profiles: cfg
            .dsp
            .profiles
            .iter()
            .map(|p| {
                let (rates, problem) = match Setup::load(p, &base) {
                    Ok(setup) => (setup.map(|s| s.rates()).unwrap_or_default(), None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                };
                Summary {
                    name: p.name.clone(),
                    devices: p.devices.clone(),
                    bands: p.filters.len(),
                    rates,
                    problem,
                }
            })
            .collect(),
    }
}

/// Save an import as the profile `name`, or the name it came with. An existing
/// profile of that name keeps its devices, and whichever of its bands and
/// responses the import does not replace — so a room's responses and a
/// headphone EQ can be imported into one profile in turn.
pub fn save(imported: Imported, name: Option<&str>) -> Result<String, String> {
    let name = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(&imported.name)
        .to_string();
    let impulses = if imported.impulses.is_empty() {
        None
    } else {
        Some(write_impulses(&name, &imported)?)
    };
    let filters = (!imported.filters.is_empty()).then_some(imported.filters);
    let source = imported.source;
    Config::persist(|cfg| {
        let profile = profile_mut(&mut cfg.dsp.profiles, &name);
        for s in source {
            if !profile.source.contains(&s) {
                profile.source.push(s);
            }
        }
        if let Some(filters) = filters {
            profile.filters = filters;
            // Derived from the filters, which now include these.
            profile.preamp_db = None;
        }
        if let Some(impulses) = impulses {
            profile.impulses = impulses;
        }
    })
    .map_err(|e| e.to_string())?;
    Ok(name)
}

fn write_impulses(name: &str, imported: &Imported) -> Result<Vec<PathBuf>, String> {
    let rel = Path::new("dsp").join(slug(name));
    let dir = config::config_dir().join(&rel);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for ir in &imported.impulses {
        let shared_rate = imported
            .impulses
            .iter()
            .filter(|i| i.rate == ir.rate)
            .count()
            > 1;
        let stem = match (shared_rate, ir.channels) {
            (true, Some(n)) => format!("{}-{n}ch", ir.rate),
            _ => ir.rate.to_string(),
        };
        let file = match ir.as_channels() {
            Some(channels) => {
                let channels: Vec<Vec<f32>> = channels.into_iter().map(<[f32]>::to_vec).collect();
                let file = format!("{stem}.wav");
                raw::write_wav(&dir.join(&file), ir.rate, &channels).map_err(|e| e.to_string())?;
                file
            }
            None => convolver::write(&dir, &stem, ir)?
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        paths.push(rel.join(file));
    }
    Ok(paths)
}

/// One impulse-response file in a profile, as the detail page shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpulseDetail {
    pub file: String,
    pub rate: u32,
    /// `None` for one response applied to every channel.
    pub channels: Option<usize>,
    pub taps: usize,
    pub routes: usize,
    /// Routes mix channels into others, as crossfeed does, rather than each
    /// channel being convolved on its own.
    pub mixes: bool,
    /// Channels delayed against each other.
    pub delayed: bool,
    /// Where the response peaks: the delay trimmed from playback.
    pub peak_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub name: String,
    pub devices: Vec<String>,
    pub source: Vec<String>,
    pub filters: Vec<crate::config::EqFilter>,
    pub impulses: Vec<ImpulseDetail>,
    /// The gain applied ahead of the filters, at `preamp_rate`.
    pub preamp_db: f64,
    pub preamp_rate: u32,
    /// Set by hand rather than derived.
    pub preamp_set: bool,
    pub problem: Option<String>,
}

/// Everything in the profile `name`.
pub fn detail(name: &str) -> Option<Detail> {
    let cfg = Config::cached();
    let profile = cfg.dsp.profiles.iter().find(|p| p.name == name)?;
    let base = config::config_dir();
    let mut problem = None;
    let mut impulses = Vec::new();
    for path in &profile.impulses {
        match super::load_impulses(path, &base) {
            Ok(irs) => impulses.extend(irs.into_iter().map(|ir| {
                ImpulseDetail {
                    file: path
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    rate: ir.rate,
                    channels: ir.channels,
                    taps: ir.taps(),
                    routes: ir.routes.len(),
                    mixes: ir.mixes(),
                    delayed: ir.in_delays.iter().chain(&ir.out_delays).any(|&d| d > 0),
                    peak_ms: ir.delay() as f64 * 1000.0 / ir.rate as f64,
                }
            })),
            Err(e) => problem = Some(e.to_string()),
        }
    }
    impulses.sort_by_key(|i| i.rate);
    let preamp_rate = impulses
        .iter()
        .map(|i| i.rate)
        .find(|&r| r == 48000)
        .or(impulses.first().map(|i| i.rate))
        .unwrap_or(48000);
    let preamp_db = Setup::load(profile, &base)
        .ok()
        .flatten()
        .map_or(0.0, |s| s.preamp_db(preamp_rate, 2));
    Some(Detail {
        name: profile.name.clone(),
        devices: profile.devices.clone(),
        source: profile.source.clone(),
        filters: profile.filters.clone(),
        impulses,
        preamp_db,
        preamp_rate,
        preamp_set: profile.preamp_db.is_some(),
        problem,
    })
}

/// Rename a profile, moving the responses koan keeps for it.
pub fn rename(old: &str, new: &str) -> Result<(), String> {
    let new = new.trim();
    if new.is_empty() {
        return Err("A profile needs a name".into());
    }
    if new == old {
        return Ok(());
    }
    let cfg = Config::cached();
    if cfg.dsp.profiles.iter().any(|p| p.name == new) {
        return Err(format!("There is already a profile called {new}"));
    }
    if !cfg.dsp.profiles.iter().any(|p| p.name == old) {
        return Err(format!("No profile called {old}"));
    }
    let (from, to) = (
        Path::new("dsp").join(slug(old)),
        Path::new("dsp").join(slug(new)),
    );
    let base = config::config_dir();
    let moved = from != to && base.join(&from).exists();
    if moved {
        if base.join(&to).exists() {
            return Err(format!("{} is in the way", to.display()));
        }
        std::fs::rename(base.join(&from), base.join(&to)).map_err(|e| e.to_string())?;
    }
    Config::persist(|cfg| {
        if let Some(p) = cfg.dsp.profiles.iter_mut().find(|p| p.name == old) {
            p.name = new.to_string();
            if moved {
                for path in &mut p.impulses {
                    if let Ok(rest) = path.strip_prefix(&from) {
                        *path = to.join(rest);
                    }
                }
            }
        }
    })
    .map_err(|e| e.to_string())
}

/// Play `device` through `name`, or untouched with `None`.
pub fn assign(name: Option<&str>, device: &str) -> Result<(), String> {
    if let Some(name) = name
        && !Config::cached().dsp.profiles.iter().any(|p| p.name == name)
    {
        return Err(format!("No profile called {name}"));
    }
    Config::persist(|cfg| {
        for p in &mut cfg.dsp.profiles {
            p.devices.retain(|d| d != device);
            if Some(p.name.as_str()) == name {
                p.devices.push(device.to_string());
            }
        }
    })
    .map_err(|e| e.to_string())
}

/// Delete a profile, and the responses koan keeps for it.
pub fn remove(name: &str) -> Result<(), String> {
    Config::persist(|cfg| cfg.dsp.profiles.retain(|p| p.name != name))
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(config::config_dir().join("dsp").join(slug(name)));
    Ok(())
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    Config::persist(|cfg| cfg.dsp.enabled = enabled).map_err(|e| e.to_string())
}

fn profile_mut<'a>(profiles: &'a mut Vec<DspProfile>, name: &str) -> &'a mut DspProfile {
    let index = match profiles.iter().position(|p| p.name == name) {
        Some(i) => i,
        None => {
            profiles.push(DspProfile {
                name: name.into(),
                ..Default::default()
            });
            profiles.len() - 1
        }
    };
    &mut profiles[index]
}

/// A directory name for a profile name.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() { "profile".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::dsp::impulse::{Impulse, Route};

    #[test]
    fn an_import_lands_beside_the_config_and_merges_by_name() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());

        let eq = Imported {
            name: "HD 600".into(),
            source: vec![],
            filters: vec![crate::config::EqFilter {
                kind: crate::config::EqFilterKind::Peaking,
                freq: 100.0,
                gain_db: 3.0,
                q: 1.0,
                channels: vec![],
            }],
            impulses: vec![],
        };
        assert_eq!(save(eq, None).unwrap(), "HD 600");
        assert_eq!(slug("HD 600 / Röom"), "hd-600-röom");

        let crossfeed = Impulse {
            rate: 48000,
            channels: Some(2),
            routes: vec![Route {
                ir: vec![1.0],
                inputs: vec![(0, 0.5), (1, 0.5)],
                outputs: vec![(0, 1.0), (1, 1.0)],
            }],
            in_delays: vec![],
            out_delays: vec![],
        };
        let room = Imported {
            name: "whatever".into(),
            source: vec!["whatever-source".into()],
            filters: vec![],
            impulses: vec![Impulse::from_channels(44100, vec![vec![1.0]]), crossfeed],
        };
        save(room, Some("HD 600")).unwrap();
        assign(Some("HD 600"), "Topping E30").unwrap();

        let o = overview();
        let p = &o.profiles[0];
        assert_eq!(o.profiles.len(), 1);
        assert_eq!(p.bands, 1, "the EQ survived the second import");
        assert_eq!(p.rates, vec![44100, 48000]);
        assert_eq!(p.devices, vec!["Topping E30"]);
        assert!(p.problem.is_none(), "{:?}", p.problem);
        assert!(dir.path().join("dsp/hd-600/48000.cfg").exists());

        let d = detail("HD 600").unwrap();
        assert_eq!(d.impulses.len(), 2);
        assert!(!d.impulses[0].mixes);
        assert!(d.impulses[1].mixes, "the crossfeed route mixes");
        assert_eq!(d.source, vec!["whatever-source"]);

        rename("HD 600", "Desk").unwrap();
        assert!(dir.path().join("dsp/desk/48000.cfg").exists());
        let d = detail("Desk").unwrap();
        assert!(d.problem.is_none(), "{:?}", d.problem);
        assert_eq!(d.impulses.len(), 2);
        assert!(rename("Desk", "").is_err());

        remove("Desk").unwrap();
        assert!(!dir.path().join("dsp/desk").exists());
        assert!(overview().profiles.is_empty());
    }
}
