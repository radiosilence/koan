//! The profiles in `config.local.toml`, as every front end changes them.
//!
//! Imported responses are kept under `dsp/<profile>/` beside the config, one
//! 32-bit float WAV per rate, or a `.cfg` with it where routes mix or delay
//! channels. Whatever they were imported from — a Roon zip, a CamillaDSP
//! setup, a share from another app — is read once and not needed again.

use std::path::{Path, PathBuf};

use super::import::Imported;
use super::{Setup, convolver, raw};
use crate::config::{self, Config, DspEar, DspMeasurement, DspProfile, DspRole, DspScope};

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub name: String,
    pub devices: Vec<String>,
    pub bands: usize,
    /// Profiles it plays first, for a stack.
    pub layers: usize,
    /// Rates there are responses for.
    pub rates: Vec<u32>,
    /// Why the profile would not load, if it would not.
    pub problem: Option<String>,
    pub role: DspRole,
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
                let (rates, problem) = match Setup::load(p, &cfg.dsp.profiles, &base) {
                    Ok(setup) => (setup.map(|s| s.rates()).unwrap_or_default(), None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                };
                Summary {
                    name: p.name.clone(),
                    devices: p.devices.clone(),
                    bands: p.filters.len(),
                    layers: p.layers.len(),
                    rates,
                    problem,
                    role: role(p),
                }
            })
            .collect(),
    }
}

/// Save an import as the profile `name`, or the name it came with. An existing
/// profile of that name keeps its devices, and whichever of its filters and
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
    persist(|cfg| {
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
    pub filters: Vec<crate::config::DspFilter>,
    pub impulses: Vec<ImpulseDetail>,
    /// The gain applied ahead of the filters, at `preamp_rate`.
    pub preamp_db: f64,
    pub preamp_rate: u32,
    /// Set by hand rather than derived.
    pub preamp_set: bool,
    pub problem: Option<String>,
    /// Profiles it plays first, for a stack.
    pub layers: Vec<crate::config::DspLayer>,
    /// Kept on every device of the account, rather than this one alone.
    pub everywhere: bool,
    /// Where it is kept was chosen, rather than following from what it is.
    pub scope_set: bool,
    pub role: DspRole,
    pub role_set: bool,
    /// A chain correcting twice, made before that was refused: which, and
    /// what to do.
    pub corrects_twice: Option<String>,
    /// The chain in a line each: see [`summary`].
    pub corrects: Option<String>,
    pub corrects_baked: bool,
    pub tunings: Vec<String>,
    /// For each of `layers`, what it is for; `None` where it is missing.
    pub layer_roles: Vec<Option<DspRole>>,
    pub measured: bool,
    pub made_for: Option<String>,
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
    // What it plays is adjusted to stay within bounds: say how.
    let mut adjusted = profile.clone().sanitize();
    match super::chain_noted(profile, &cfg.dsp.profiles, &mut Vec::new()) {
        Ok((_, notes)) => {
            for n in notes {
                if !adjusted.contains(&n) {
                    adjusted.push(n);
                }
            }
        }
        Err(e) => problem = Some(e.to_string()),
    }
    let setup = Setup::load(profile, &cfg.dsp.profiles, &base)
        .ok()
        .flatten();
    if let Some(cut) = setup.as_ref().and_then(|s| s.headroom_cut(preamp_rate, 2)) {
        adjusted.push(format!("preamp −{cut:.1} dB for headroom"));
    }
    if !adjusted.is_empty() {
        let note = format!("Adjusted: {}", adjusted.join("; "));
        problem = Some(problem.map_or(note.clone(), |p| format!("{p}. {note}")));
    }
    let preamp_db = setup.map_or(0.0, |s| s.preamp_db(preamp_rate, 2));
    let chain_summary = summary(name).unwrap_or_default();
    Some(Detail {
        name: profile.name.clone(),
        devices: profile.devices.clone(),
        source: profile.source.clone(),
        filters: profile.filters.clone(),
        impulses,
        preamp_db,
        preamp_rate,
        preamp_set: profile.preamp_db.is_some(),
        layers: profile.layers.clone(),
        problem,
        everywhere: scope(profile, &cfg.dsp.profiles) == DspScope::Everywhere,
        scope_set: profile.scope.is_some(),
        role: role(profile),
        role_set: profile.role.is_some(),
        corrects_twice: corrects_twice(profile, &cfg.dsp.profiles),
        corrects: chain_summary.correction,
        corrects_baked: chain_summary.baked,
        tunings: chain_summary.tunings,
        layer_roles: profile
            .layers
            .iter()
            .map(|l| {
                cfg.dsp
                    .profiles
                    .iter()
                    .find(|p| p.name == l.profile)
                    .map(role)
            })
            .collect(),
        measured: profile.measurement.is_some(),
        made_for: profile.target.as_ref().map(|t| t.made_for.clone()),
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
    persist(|cfg| {
        // The stacks that play it follow the new name.
        for l in cfg
            .dsp
            .profiles
            .iter_mut()
            .flat_map(|p| p.layers.iter_mut())
        {
            if l.profile == old {
                l.profile = new.to_string();
            }
        }
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

/// Write the profiles, and say so: what each output plays through is shown
/// on every device that can choose it, here and on the devices controlling
/// this one.
fn persist(mutate: impl FnOnce(&mut crate::config::Config)) -> Result<(), String> {
    Config::persist(mutate).map_err(|e| e.to_string())?;
    crate::signal::engine_changed().bump();
    crate::remote::dsp_sync::changed();
    Ok(())
}

/// Where `name` keeps its files: responses, and an AutoEQ result's CSV.
pub fn dir(name: &str) -> PathBuf {
    config::config_dir().join("dsp").join(slug(name))
}

/// Record the target `name`'s correction was made for, keeping any target
/// chosen in its place.
pub fn set_made_for(name: &str, made_for: Option<&str>) -> Result<(), String> {
    persist(|cfg| {
        if let Some(p) = cfg.dsp.profiles.iter_mut().find(|p| p.name == name) {
            p.target = made_for.map(|m| crate::config::DspTarget {
                made_for: m.to_owned(),
                chosen: p.target.as_ref().and_then(|t| t.chosen.clone()),
            });
        }
    })
    .map_err(|e| e.to_string())
}

/// The headphone `p` corrects, as measured, and the target it is corrected
/// to, for drawing: an AutoEQ result's own two curves, its target moved as a
/// chosen target moves it; or a measurement and its target, levelled at
/// 1 kHz so they are drawn against each other.
fn headphone(p: &DspProfile) -> Option<(super::targets::Curve, super::targets::Curve)> {
    use super::targets;
    let folder = dir(&p.name);
    if let Some(m) = &p.measurement {
        let level = |c: targets::Curve| {
            let k = targets::at(&c, 1000.0);
            c.into_iter()
                .map(|(hz, db)| (hz, db - k))
                .collect::<targets::Curve>()
        };
        return Some((
            level(targets::measurement(&folder)?),
            level(targets::choice_curve(&m.target)?),
        ));
    }
    let (raw, made_for) = targets::autoeq_measurement(&folder)?;
    let moved = p
        .target
        .as_ref()
        .and_then(|t| Some((t.chosen.as_ref()?, &t.made_for)))
        .and_then(|(to, from)| {
            Some(targets::difference(
                &targets::choice_curve(from)?,
                &targets::choice_curve(to)?,
            ))
        });
    Some((
        raw,
        match moved {
            Some(step) => targets::moved(&made_for, &step),
            None => made_for,
        },
    ))
}

/// What `name` offers to move its correction to.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetChoices {
    /// The target an AutoEQ correction or a ready-made EQ was made for;
    /// none for one built from a measurement, which is made for `chosen`.
    pub made_for: Option<&'static super::targets::Target>,
    pub chosen: Option<String>,
    /// The shipped targets for the same kind of headphone, then those added.
    pub choices: Vec<TargetChoice>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TargetChoice {
    pub id: String,
    pub name: String,
    /// What it does, in a few plain words; empty for one added.
    pub does: String,
    /// What it sounds like; empty for one added.
    pub character: String,
}

/// The targets `name` can move to, for a correction installed from AutoEQ
/// whose target is known.
pub fn target_choices(name: &str) -> Option<TargetChoices> {
    use super::targets;
    let cfg = Config::cached();
    let p = cfg.dsp.profiles.iter().find(|p| p.name == name)?;
    let (made_for, chosen, ear) = match (&p.measurement, &p.target) {
        (Some(m), _) => (
            None,
            Some(m.target.clone()),
            match m.ear {
                DspEar::In => targets::Ear::In,
                DspEar::Over => targets::Ear::Over,
            },
        ),
        (None, Some(t)) => {
            let made_for = targets::shipped(&t.made_for)?;
            (Some(made_for), t.chosen.clone(), made_for.ear)
        }
        (None, None) => return None,
    };
    let mut choices: Vec<TargetChoice> = targets::TARGETS
        .iter()
        .filter(|c| c.ear == ear)
        .map(|c| TargetChoice {
            id: c.id.into(),
            name: c.name.into(),
            does: c.does.into(),
            character: c.character.into(),
        })
        .collect();
    choices.extend(targets::added().into_iter().map(|a| TargetChoice {
        id: a.id,
        name: a.name,
        does: String::new(),
        character: String::new(),
    }));
    Some(TargetChoices {
        made_for,
        chosen,
        choices,
    })
}

/// Move `name`'s correction to `chosen`, or with `None` back to the target it
/// was made for.
pub fn choose_target(name: &str, chosen: Option<&str>) -> Result<(), String> {
    let choices =
        target_choices(name).ok_or_else(|| format!("{name} has no target to move from"))?;
    if let Some(c) = chosen
        && !choices.choices.iter().any(|x| x.id == c)
    {
        return Err(format!("No target {c} for {name}"));
    }
    persist(|cfg| {
        let Some(p) = cfg.dsp.profiles.iter_mut().find(|p| p.name == name) else {
            return;
        };
        if let Some(m) = p.measurement.as_mut() {
            // Built from a measurement: it is made for the target chosen.
            if let Some(c) = chosen {
                m.target = c.to_owned();
            }
        } else if let Some(t) = p.target.as_mut() {
            t.chosen = chosen.filter(|c| *c != t.made_for).map(str::to_owned);
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
    persist(|cfg| {
        for p in &mut cfg.dsp.profiles {
            p.devices.retain(|d| d != device);
            if Some(p.name.as_str()) == name {
                p.devices.push(device.to_string());
            }
        }
    })
    .map_err(|e| e.to_string())
}

/// The ranges a band edited by hand is held to: the audible band, what a
/// correction plausibly asks, and Q from very broad to very narrow.
const BAND_HZ: std::ops::RangeInclusive<f64> = 10.0..=22_000.0;
const BAND_DB: std::ops::RangeInclusive<f64> = -30.0..=30.0;
const BAND_Q: std::ops::RangeInclusive<f64> = 0.1..=20.0;

fn clamp(v: f64, r: &std::ops::RangeInclusive<f64>) -> Result<f64, String> {
    if !v.is_finite() {
        return Err("Not a number".into());
    }
    Ok(v.clamp(*r.start(), *r.end()))
}

/// Set filter `index` of `name`, a parametric band, to `kind` at `freq`,
/// `gain_db` and `q`, held within the ranges a band may have. Its channels
/// are kept. Delays, mixes and graphic curves are not edited this way.
pub fn set_band(
    name: &str,
    index: usize,
    kind: &str,
    freq: f64,
    gain_db: f64,
    q: f64,
) -> Result<(), String> {
    use crate::config::{DspFilter, EqFilterKind};
    let kind: EqFilterKind = serde_json::from_value(serde_json::Value::String(kind.into()))
        .map_err(|_| format!("No band type called {kind}"))?;
    let (freq, gain_db, q) = (
        clamp(freq, &BAND_HZ)?,
        clamp(gain_db, &BAND_DB)?,
        clamp(q, &BAND_Q)?,
    );
    let mut found = Err(format!("{name} has no band {}", index + 1));
    persist(|cfg| {
        if let Some(DspFilter::Band(b)) = cfg
            .dsp
            .profiles
            .iter_mut()
            .find(|p| p.name == name)
            .and_then(|p| p.filters.get_mut(index))
        {
            (b.kind, b.freq, b.gain_db, b.q) = (kind, freq, gain_db, q);
            found = Ok(());
        }
    })
    .map_err(|e| e.to_string())?;
    found
}

/// Add a band to `name`: flat, at 1 kHz, for shaping from there. Answers
/// with its index among the profile's filters.
pub fn add_band(name: &str) -> Result<usize, String> {
    use crate::config::{DspFilter, EqFilter, EqFilterKind};
    if !Config::cached().dsp.profiles.iter().any(|p| p.name == name) {
        return Err(format!("No profile called {name}"));
    }
    let mut index = 0;
    persist(|cfg| {
        let p = profile_mut(&mut cfg.dsp.profiles, name);
        p.filters.push(DspFilter::Band(EqFilter {
            kind: EqFilterKind::Peaking,
            freq: 1000.0,
            gain_db: 0.0,
            q: 1.0,
            channels: vec![],
        }));
        index = p.filters.len() - 1;
    })
    .map_err(|e| e.to_string())?;
    Ok(index)
}

/// Take filter `index` out of `name`.
pub fn remove_filter(name: &str, index: usize) -> Result<(), String> {
    let mut found = Err(format!("{name} has no filter {}", index + 1));
    persist(|cfg| {
        if let Some(p) = cfg.dsp.profiles.iter_mut().find(|p| p.name == name)
            && index < p.filters.len()
        {
            p.filters.remove(index);
            found = Ok(());
        }
    })
    .map_err(|e| e.to_string())?;
    found
}

/// What a profile does to the sound, for drawing: every curve on AutoEQ's
/// grid, in dB.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub freqs: Vec<f64>,
    /// Everything it plays, layers, target step and impulse responses
    /// included, on the first channel. The preamp is left out, so the
    /// curve lines up with the bands' own gains; it is `preamp_db`.
    pub total: Vec<f64>,
    /// Each of its own parametric bands alone, in order.
    pub bands: Vec<Vec<f64>>,
    /// Each layer as it plays alone, for a stack.
    pub layers: Vec<LayerResponse>,
    /// For a correction from AutoEQ (or a stack with one among its layers):
    /// the headphone as measured, the target it plays to, and the measurement
    /// with everything here applied.
    pub measurement: Option<Vec<f64>>,
    pub target: Option<Vec<f64>>,
    pub predicted: Option<Vec<f64>>,
    /// The gain ahead of it all at `rate`.
    pub preamp_db: f64,
    /// For a chain with both a correction and tuning: what the correction
    /// does, and what the tuning on top does, so each can be drawn as its
    /// own. `total` is the two together.
    pub correction: Option<Vec<f64>>,
    pub tuning: Option<Vec<f64>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerResponse {
    pub name: String,
    pub on: bool,
    pub db: Vec<f64>,
}

/// What `name` does to the sound at `rate`, from the filters the DSP runs.
/// `None` for a profile that is not there or would not play.
pub fn response(name: &str, rate: u32) -> Option<Response> {
    use super::targets;
    let cfg = Config::cached();
    let all = &cfg.dsp.profiles;
    let profile = all.iter().find(|p| p.name == name)?;
    let freqs = targets::grid();
    let curve = |filters: &[crate::config::DspFilter]| super::response(filters, &freqs, rate);
    let setup = Setup::load(profile, all, &config::config_dir()).ok()?;
    let total = setup
        .as_ref()
        .map_or_else(|| vec![0.0; freqs.len()], |s| s.response(&freqs, rate));
    let bands = profile
        .filters
        .iter()
        .filter(|f| matches!(f, crate::config::DspFilter::Band(_)))
        .map(|f| curve(std::slice::from_ref(f)))
        .collect();
    // The correction alone, where the chain has tuning besides.
    let corrector = corrections_in(profile, all)
        .first()
        .and_then(|c| all.iter().find(|p| &p.name == c))
        .filter(|c| c.name != profile.name || !profile.layers.is_empty());
    let correction = corrector.and_then(|c| {
        let alone = DspProfile {
            layers: Vec::new(),
            ..c.clone()
        };
        Setup::load(&alone, all, &config::config_dir())
            .ok()
            .flatten()
            .map(|s| s.response(&freqs, rate))
    });
    let tuning = correction
        .as_ref()
        .map(|c| {
            total
                .iter()
                .zip(c)
                .map(|(t, c)| t - c)
                .collect::<Vec<f64>>()
        })
        .filter(|t| t.iter().any(|db| db.abs() > 0.05));
    let correction = correction.filter(|_| tuning.is_some());
    let layers = profile
        .layers
        .iter()
        .filter_map(|l| {
            let p = all.iter().find(|p| p.name == l.profile)?;
            let filters = super::chain(p, all, &mut Vec::new()).ok()?;
            Some(LayerResponse {
                name: l.profile.clone(),
                on: l.on,
                db: curve(&filters),
            })
        })
        .collect();

    // The headphone this chain corrects, as measured, and the target it is
    // corrected to: this profile's, or the first of its layers that has one.
    let measured = std::iter::once(profile)
        .chain(
            profile
                .layers
                .iter()
                .filter(|l| l.on)
                .filter_map(|l| all.iter().find(|p| p.name == l.profile)),
        )
        .find_map(headphone);
    let (measurement, target, predicted) = match measured {
        Some((raw, aim)) => {
            let raw: Vec<f64> = freqs.iter().map(|&hz| targets::at(&raw, hz)).collect();
            let aim: Vec<f64> = freqs.iter().map(|&hz| targets::at(&aim, hz)).collect();
            let predicted = raw.iter().zip(&total).map(|(r, t)| r + t).collect();
            (Some(raw), Some(aim), Some(predicted))
        }
        None => (None, None, None),
    };
    let preamp_db = setup.map_or(0.0, |s| s.preamp_db(s.output_rate(rate), 2));
    Some(Response {
        freqs,
        total,
        bands,
        layers,
        measurement,
        target,
        predicted,
        preamp_db,
        correction,
        tuning,
    })
}

/// The stacks `name` is a layer of.
fn stacks_of(cfg: &Config, name: &str) -> Vec<String> {
    cfg.dsp
        .profiles
        .iter()
        .filter(|p| p.layers.iter().any(|l| l.profile == name))
        .map(|p| p.name.clone())
        .collect()
}

/// Where `profile` is kept: as set, or as follows from what it is. A room
/// or speaker correction, with impulse responses, or one for an output that
/// stays put (built in, or an amplifier on the network) is this device's; a
/// headphone correction from AutoEQ is the account's, since headphones move
/// between devices; a stack is the account's when every layer is.
pub fn scope(profile: &DspProfile, all: &[DspProfile]) -> DspScope {
    scope_in(profile, all, &mut Vec::new())
}

fn scope_in(profile: &DspProfile, all: &[DspProfile], seen: &mut Vec<String>) -> DspScope {
    if let Some(scope) = profile.scope {
        return scope;
    }
    if !profile.impulses.is_empty() {
        return DspScope::Device;
    }
    if profile.target.is_some() || profile.measurement.is_some() {
        return DspScope::Everywhere;
    }
    if !profile.layers.is_empty() {
        if seen.contains(&profile.name) {
            return DspScope::Device;
        }
        seen.push(profile.name.clone());
        let everywhere = profile.layers.iter().all(|l| {
            all.iter()
                .find(|p| p.name == l.profile)
                .is_some_and(|p| scope_in(p, all, seen) == DspScope::Everywhere)
        });
        return if everywhere {
            DspScope::Everywhere
        } else {
            DspScope::Device
        };
    }
    if profile.devices.iter().any(|d| stays_put(d)) {
        return DspScope::Device;
    }
    DspScope::Everywhere
}

/// An output that does not travel: built in, an amplifier on the network
/// (named by its UDN), or a phone's own speaker.
fn stays_put(device: &str) -> bool {
    device.starts_with("uuid:")
        || device == "Speaker"
        || crate::audio::list_output_devices().is_ok_and(|devices| {
            devices
                .iter()
                .any(|d| d.name == device && d.kind == crate::audio::backend::OutputKind::BuiltIn)
        })
}

/// The first of `layers` kept on this device alone, which a stack kept
/// everywhere cannot play on the account's other devices.
fn local_layer<'a>(layers: &'a [crate::config::DspLayer], all: &[DspProfile]) -> Option<&'a str> {
    layers
        .iter()
        .find(|l| {
            all.iter()
                .find(|p| p.name == l.profile)
                .is_some_and(|p| scope(p, all) == DspScope::Device)
        })
        .map(|l| l.profile.as_str())
}

fn kept_here(stack: &str, layer: &str) -> String {
    format!(
        "{layer} is kept on this device, so {stack}, which is kept everywhere, \
         could not play it on your other devices. Keep {layer} everywhere, or {stack} on this device"
    )
}

/// Keep `name` everywhere or on this device alone. A stack kept everywhere
/// cannot have a layer kept here: refused either way round.
pub fn set_scope(name: &str, to: DspScope) -> Result<(), String> {
    let cfg = Config::cached();
    let all = &cfg.dsp.profiles;
    let profile = all
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("No profile called {name}"))?;
    match to {
        DspScope::Everywhere => {
            if let Some(layer) = local_layer(&profile.layers, all) {
                return Err(kept_here(name, layer));
            }
        }
        DspScope::Device => {
            if let Some(stack) = all.iter().find(|p| {
                p.layers.iter().any(|l| l.profile == name) && scope(p, all) == DspScope::Everywhere
            }) {
                return Err(format!(
                    "{name} is a layer of {}, which is kept everywhere and would lose it on \
                     your other devices. Keep {} on this device first",
                    stack.name, stack.name
                ));
            }
        }
    }
    persist(|cfg| {
        if let Some(p) = cfg.dsp.profiles.iter_mut().find(|p| p.name == name) {
            p.scope = Some(to);
        }
    })
    .map_err(|e| e.to_string())
}

/// What `profile` is for: as set, or a correction where it was installed
/// from AutoEQ, built from a measurement or said to be made for a target,
/// and a tuning otherwise.
pub fn role(profile: &DspProfile) -> DspRole {
    profile.role.unwrap_or(
        if profile.target.is_some() || profile.measurement.is_some() {
            DspRole::Correction
        } else {
            DspRole::Tuning
        },
    )
}

/// The corrections a chain plays, `profile` and its layers switched on, in
/// the order they play.
pub fn corrections_in(profile: &DspProfile, all: &[DspProfile]) -> Vec<String> {
    fn walk(p: &DspProfile, all: &[DspProfile], seen: &mut Vec<String>, out: &mut Vec<String>) {
        if seen.contains(&p.name) || seen.len() > 64 {
            return;
        }
        seen.push(p.name.clone());
        for l in p.layers.iter().filter(|l| l.on) {
            if let Some(q) = all.iter().find(|q| q.name == l.profile) {
                walk(q, all, seen, out);
            }
        }
        if role(p).corrects() && !out.contains(&p.name) {
            out.push(p.name.clone());
        }
    }
    let mut out = Vec::new();
    walk(profile, all, &mut Vec::new(), &mut out);
    out
}

/// A correction by name and kind, as a warning names it: "Cantor (AutoEQ)".
fn correction_label(p: &DspProfile) -> String {
    let kind = if role(p) == DspRole::Baked {
        "baked"
    } else if p.measurement.is_some() {
        "measured"
    } else if super::targets::autoeq_measurement(&dir(&p.name)).is_some() {
        "AutoEQ"
    } else if p.target.is_some() {
        "ready-made"
    } else {
        "correction"
    };
    format!("{} ({kind})", p.name)
}

/// What a chain correcting more than once says: each undoes the same
/// headphone, so it plays twice the correction. Stacks made before that was
/// refused still play, and say so.
pub fn corrects_twice(profile: &DspProfile, all: &[DspProfile]) -> Option<String> {
    let c = corrections_in(profile, all);
    if c.len() < 2 {
        return None;
    }
    let labels: Vec<String> = c
        .iter()
        .filter_map(|n| all.iter().find(|p| &p.name == n))
        .map(correction_label)
        .collect();
    let (last, rest) = labels.split_last()?;
    Some(format!(
        "This stack corrects twice: {} and {last}. Keep one.",
        rest.join(", ")
    ))
}

/// Why a chain with more than one correction is refused.
pub fn two_corrections(corrections: &[String]) -> String {
    format!(
        "This stack already corrects for {}. Remove that correction first, or add {} as a tuning instead.",
        corrections[0],
        corrections.get(1).map_or("this", String::as_str)
    )
}

/// Say what `name` is for. Always taken, since it is what the profile is:
/// a stack it makes correct twice says so on its page (`corrects_twice`),
/// and gains no further correction.
pub fn set_role(name: &str, to: DspRole) -> Result<(), String> {
    if !Config::cached().dsp.profiles.iter().any(|p| p.name == name) {
        return Err(format!("No profile called {name}"));
    }
    persist(|cfg| {
        if let Some(p) = cfg.dsp.profiles.iter_mut().find(|p| p.name == name) {
            p.role = Some(to);
        }
    })
}

/// A target's name, by id: one that ships, one added, or the id itself.
pub fn target_name(id: &str) -> String {
    use super::targets;
    targets::shipped(id).map_or_else(
        || {
            targets::added()
                .into_iter()
                .find(|a| a.id == id)
                .map_or_else(|| id.to_owned(), |a| a.name)
        },
        |t| t.name.to_owned(),
    )
}

/// What a chain does, in a line each: the headphone it corrects and how,
/// and the tunings on top.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChainSummary {
    pub correction: Option<String>,
    /// The correction has a tuning baked in.
    pub baked: bool,
    pub tunings: Vec<String>,
}

pub fn summary(name: &str) -> Option<ChainSummary> {
    use super::targets;
    let cfg = Config::cached();
    let all = &cfg.dsp.profiles;
    let profile = all.iter().find(|p| p.name == name)?;
    let mut chain: Vec<&DspProfile> = profile
        .layers
        .iter()
        .filter(|l| l.on)
        .filter_map(|l| all.iter().find(|p| p.name == l.profile))
        .collect();
    chain.push(profile);
    let mut out = ChainSummary::default();
    for p in chain {
        match role(p) {
            DspRole::Tuning => {
                if !p.filters.is_empty() || !p.impulses.is_empty() {
                    out.tunings.push(p.name.clone());
                }
                continue;
            }
            DspRole::Baked => {
                if out.correction.is_none() {
                    out.correction = Some(format!("{} (correction + tuning in one)", p.name));
                    out.baked = true;
                }
                continue;
            }
            DspRole::Correction => {}
        }
        let line = if let Some(m) = &p.measurement {
            format!("{} → {} (from measurement)", p.name, target_name(&m.target))
        } else if let Some(t) = &p.target {
            let autoeq = targets::autoeq_measurement(&dir(&p.name)).is_some();
            match (&t.chosen, autoeq) {
                (Some(c), true) => format!(
                    "{} → {} (rebuilt from AutoEQ's measurement)",
                    p.name,
                    target_name(c)
                ),
                (Some(c), false) => format!("{} → {}", p.name, target_name(c)),
                (None, true) => format!("{} → {}", p.name, target_name(&t.made_for)),
                (None, false) => format!(
                    "{}: ready-made EQ (made for {})",
                    p.name,
                    target_name(&t.made_for)
                ),
            }
        } else {
            format!("{}: ready-made EQ (made for an unknown target)", p.name)
        };
        out.correction.get_or_insert(line);
    }
    Some(out)
}

/// A measurement, checked: frequency and level pairs covering the audible
/// band, as squig.link and REW export them.
pub fn read_measurement(text: &str) -> Result<super::targets::Curve, String> {
    super::targets::covering(text, "A measurement")
}

/// What a measurement corrected to a target would do, before anything is
/// saved: the curves the Headphone view draws, and the EQ's own.
pub fn preview_measurement(text: &str, target: &str, rate: u32) -> Result<Response, String> {
    use super::targets;
    let measured = read_measurement(text)?;
    let aim = targets::choice_curve(target).ok_or_else(|| format!("No target {target}"))?;
    let freqs = targets::grid();
    let filters = [config::DspFilter::Graphic(targets::correction(
        &measured, &aim,
    ))];
    let total = super::response(&filters, &freqs, rate);
    let level = |c: &targets::Curve| {
        let k = targets::at(c, 1000.0);
        freqs
            .iter()
            .map(|&hz| targets::at(c, hz) - k)
            .collect::<Vec<f64>>()
    };
    let raw = level(&measured);
    let predicted = raw.iter().zip(&total).map(|(r, t)| r + t).collect();
    Ok(Response {
        bands: Vec::new(),
        layers: Vec::new(),
        measurement: Some(raw),
        target: Some(level(&aim)),
        predicted: Some(predicted),
        preamp_db: 0.0,
        correction: None,
        tuning: None,
        total,
        freqs,
    })
}

/// Save a correction built from a measurement: the headphone `name`,
/// measured as `text`, corrected to `target`. A correction, kept everywhere
/// like any headphone's.
pub fn save_measured(name: &str, text: &str, ear: DspEar, target: &str) -> Result<String, String> {
    use super::targets;
    let name = name.trim();
    if name.is_empty() {
        return Err("A profile needs a name".into());
    }
    if Config::cached().dsp.profiles.iter().any(|p| p.name == name) {
        return Err(format!("There is already a profile called {name}"));
    }
    let measured = read_measurement(text)?;
    if targets::choice_curve(target).is_none() {
        return Err(format!("No target {target}"));
    }
    let folder = dir(name);
    if folder.exists() {
        return Err(format!("{} is in the way", folder.display()));
    }
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    std::fs::write(
        targets::measurement_path(&folder),
        targets::on_grid(&measured),
    )
    .map_err(|e| e.to_string())?;
    persist(|cfg| {
        cfg.dsp.profiles.push(DspProfile {
            name: name.to_owned(),
            role: Some(DspRole::Correction),
            measurement: Some(DspMeasurement {
                ear,
                target: target.to_owned(),
            }),
            ..Default::default()
        })
    })?;
    Ok(name.to_owned())
}

/// Make `name` a stack of `layers`, in order, creating it if there is none.
/// Refused where it could not play: a layer missing, a layer of itself, one
/// with impulse responses, one kept on this device under a stack kept
/// everywhere.
pub fn set_layers(name: &str, layers: Vec<crate::config::DspLayer>) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A profile needs a name".into());
    }
    if let Some(twice) = layers
        .iter()
        .enumerate()
        .find(|(i, l)| layers[..*i].iter().any(|e| e.profile == l.profile))
        .map(|(_, l)| &l.profile)
    {
        return Err(format!("{twice} is in {name} twice; a layer plays once"));
    }
    let cfg = Config::cached();
    let mut all = cfg.dsp.profiles.clone();
    let probe = profile_mut(&mut all, name);
    probe.layers = layers.clone();
    // Every layer, on or off: one switched off now is one switched on later.
    let mut check = probe.clone();
    for l in &mut check.layers {
        l.on = true;
    }
    let corrections = corrections_in(&check, &all);
    // Every correction it had, on or off: a stack that already corrects
    // twice can still be edited, but gains no other.
    let before = cfg
        .dsp
        .profiles
        .iter()
        .find(|p| p.name == name)
        .map(|p| {
            let mut p = p.clone();
            for l in &mut p.layers {
                l.on = true;
            }
            corrections_in(&p, &cfg.dsp.profiles)
        })
        .unwrap_or_default();
    if corrections.len() > 1 && corrections.iter().any(|c| !before.contains(c)) {
        // The one it had first, then the one being added.
        let mut named: Vec<String> = before
            .iter()
            .filter(|c| corrections.contains(c))
            .cloned()
            .collect();
        named.extend(corrections.iter().filter(|c| !before.contains(c)).cloned());
        return Err(two_corrections(&named));
    }
    super::chain(&check, &all, &mut Vec::new()).map_err(|e| e.to_string())?;
    if everywhere_by_choice(&all, name)
        && let Some(layer) = local_layer(&layers, &all)
    {
        return Err(kept_here(name, layer));
    }
    persist(|cfg| profile_mut(&mut cfg.dsp.profiles, name).layers = layers)
        .map_err(|e| e.to_string())
}

/// Delete a profile, and the responses koan keeps for it. Refused while a
/// stack plays it.
pub fn remove(name: &str) -> Result<(), String> {
    let stacks = stacks_of(&Config::cached(), name);
    if !stacks.is_empty() {
        return Err(format!(
            "{name} is a layer of {}; take it out first",
            stacks.join(", ")
        ));
    }
    let shared = Config::cached()
        .dsp
        .profiles
        .iter()
        .any(|p| p.name != name && slug(&p.name) == slug(name));
    persist(|cfg| cfg.dsp.profiles.retain(|p| p.name != name)).map_err(|e| e.to_string())?;
    // Another profile's name may map to the same folder: its files stay.
    if !shared {
        let _ = std::fs::remove_dir_all(dir(name));
    }
    Ok(())
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    persist(|cfg| cfg.dsp.enabled = enabled).map_err(|e| e.to_string())
}

/// Whether `name` is kept everywhere by choice: a stack kept so only because
/// its layers are follows them instead.
fn everywhere_by_choice(all: &[DspProfile], name: &str) -> bool {
    all.iter()
        .find(|p| p.name == name)
        .is_some_and(|p| p.scope == Some(DspScope::Everywhere))
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
            filters: vec![crate::config::DspFilter::Band(crate::config::EqFilter {
                kind: crate::config::EqFilterKind::Peaking,
                freq: 100.0,
                gain_db: 3.0,
                q: 1.0,
                channels: vec![],
            })],
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

    fn band(freq: f64) -> crate::config::DspFilter {
        crate::config::DspFilter::Band(crate::config::EqFilter {
            kind: crate::config::EqFilterKind::Peaking,
            freq,
            gain_db: 3.0,
            q: 1.0,
            channels: vec![],
        })
    }

    fn layer(profile: &str, on: bool) -> crate::config::DspLayer {
        crate::config::DspLayer {
            profile: profile.into(),
            on,
        }
    }

    /// A stack plays its layers that are on, in order, then its own filters;
    /// it follows a layer's rename, and keeps a layer it plays from being
    /// deleted.
    #[test]
    fn a_stack_plays_its_layers_in_order() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        persist(|c| {
            for (name, f) in [
                ("HD 650", 100.0),
                ("Bass +3", 60.0),
                ("Treble tilt", 8000.0),
            ] {
                c.dsp.profiles.push(DspProfile {
                    name: name.into(),
                    filters: vec![band(f)],
                    ..Default::default()
                });
            }
        })
        .unwrap();
        set_layers(
            "Desk",
            vec![
                layer("HD 650", true),
                layer("Bass +3", true),
                layer("Treble tilt", false),
            ],
        )
        .unwrap();
        let played = || {
            let cfg = Config::cached();
            let p = cfg.dsp.profiles.iter().find(|p| p.name == "Desk").unwrap();
            Setup::load(p, &cfg.dsp.profiles, dir.path())
                .unwrap()
                .unwrap()
                .filters
        };
        assert_eq!(played(), vec![band(100.0), band(60.0)]);
        set_layers(
            "Desk",
            vec![
                layer("Bass +3", true),
                layer("HD 650", true),
                layer("Treble tilt", true),
            ],
        )
        .unwrap();
        assert_eq!(played(), vec![band(60.0), band(100.0), band(8000.0)]);
        let summary = overview_for(None);
        assert_eq!(
            summary
                .profiles
                .iter()
                .find(|p| p.name == "Desk")
                .unwrap()
                .layers,
            3
        );

        rename("Bass +3", "Bass").unwrap();
        assert_eq!(detail("Desk").unwrap().layers[0].profile, "Bass");
        assert_eq!(played(), vec![band(60.0), band(100.0), band(8000.0)]);
        let refused = remove("Bass").unwrap_err();
        assert!(refused.contains("Desk"), "{refused}");
    }

    #[test]
    fn a_stack_that_could_not_play_is_refused() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "Room".into(),
                impulses: vec!["dsp/room/48000.wav".into()],
                ..Default::default()
            });
            c.dsp.profiles.push(DspProfile {
                name: "EQ".into(),
                filters: vec![band(100.0)],
                ..Default::default()
            });
        })
        .unwrap();
        assert!(
            set_layers("Desk", vec![layer("Nothing", true)]).is_err(),
            "missing"
        );
        assert!(
            set_layers("Desk", vec![layer("Room", true)]).is_err(),
            "impulses"
        );
        assert!(
            set_layers("Desk", vec![layer("Desk", true)]).is_err(),
            "itself"
        );
        set_layers("Desk", vec![layer("EQ", true)]).unwrap();
        // A cycle through another, even with the way back switched off.
        assert!(
            set_layers("EQ", vec![layer("Desk", false)]).is_err(),
            "cycle"
        );
    }

    /// Shared layers resolve, each where it is played; a ladder of them that
    /// would copy its filters exponentially is refused when set, and as a
    /// stack edited by hand it reports the problem and does not load, rather
    /// than growing the chain until the process aborts.
    #[test]
    fn a_stack_cannot_grow_without_bound() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "D".into(),
                filters: vec![band(100.0)],
                ..Default::default()
            });
        })
        .unwrap();
        // A diamond: B and C both play D.
        set_layers("B", vec![layer("D", true)]).unwrap();
        set_layers("C", vec![layer("D", true)]).unwrap();
        set_layers("A", vec![layer("B", true), layer("C", true)]).unwrap();
        {
            let cfg = Config::cached();
            let a = cfg.dsp.profiles.iter().find(|p| p.name == "A").unwrap();
            let played = Setup::load(a, &cfg.dsp.profiles, dir.path())
                .unwrap()
                .unwrap();
            assert_eq!(
                played.filters,
                vec![band(100.0), band(100.0)],
                "a diamond resolves"
            );
        }
        assert!(
            set_layers("X", vec![layer("D", true), layer("D", true)]).is_err(),
            "twice"
        );

        // A ladder, as a hand edit could leave it: each rung plays the two
        // below, 2^30 copies of D at the top.
        persist(|c| {
            for i in 0..30 {
                let below = |n: i32| {
                    if n < 0 {
                        "D".to_string()
                    } else {
                        format!("L{n}")
                    }
                };
                c.dsp.profiles.push(DspProfile {
                    name: format!("L{i}"),
                    layers: vec![layer(&below(i - 1), true), layer(&below(i - 2), true)],
                    ..Default::default()
                });
            }
        })
        .unwrap();
        let cfg = Config::cached();
        let top = cfg.dsp.profiles.iter().find(|p| p.name == "L29").unwrap();
        let err = Setup::load(top, &cfg.dsp.profiles, dir.path())
            .err()
            .unwrap();
        assert!(err.to_string().contains("L29"), "{err}");
        let summary = overview_for(None);
        let l29 = summary.profiles.iter().find(|p| p.name == "L29").unwrap();
        assert!(l29.problem.is_some(), "Settings shows why it does not play");
        assert!(set_layers("L30", vec![layer("L29", true)]).is_err());
    }

    /// The response is what the DSP plays: a 3 dB peak shows as 3 dB at its
    /// frequency and nothing far from it, a stack is its layers summed, and a
    /// band's own curve is drawn alone.
    #[test]
    fn a_response_is_the_filters_the_dsp_runs() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "Peak".into(),
                filters: vec![band(1000.0)],
                ..Default::default()
            });
            c.dsp.profiles.push(DspProfile {
                name: "Shelf".into(),
                filters: vec![crate::config::DspFilter::Band(crate::config::EqFilter {
                    kind: crate::config::EqFilterKind::LowShelf,
                    freq: 100.0,
                    gain_db: 6.0,
                    q: 0.7,
                    channels: vec![],
                })],
                ..Default::default()
            });
        })
        .unwrap();
        let r = response("Peak", 48000).unwrap();
        let at = |r: &Response, curve: &[f64], hz: f64| {
            let i = r.freqs.iter().position(|f| *f >= hz).unwrap();
            curve[i]
        };
        assert!(
            (at(&r, &r.total, 1000.0) - 3.0).abs() < 0.2,
            "{}",
            at(&r, &r.total, 1000.0)
        );
        assert!(at(&r, &r.total, 20.0).abs() < 0.1);
        assert_eq!(r.bands.len(), 1);
        assert!(r.measurement.is_none());
        assert!(
            r.preamp_db < -2.5,
            "the peak is taken off ahead: {}",
            r.preamp_db
        );

        set_layers("Both", vec![layer("Shelf", true), layer("Peak", true)]).unwrap();
        let r = response("Both", 48000).unwrap();
        assert_eq!(r.layers.len(), 2);
        assert!((at(&r, &r.total, 30.0) - 6.0).abs() < 0.5, "the shelf");
        assert!((at(&r, &r.total, 1000.0) - 3.0).abs() < 0.5, "the peak");
        assert!(r.bands.is_empty(), "no bands of its own");
        set_layers("Both", vec![layer("Shelf", false), layer("Peak", true)]).unwrap();
        let r = response("Both", 48000).unwrap();
        assert!(at(&r, &r.total, 30.0).abs() < 0.3, "the shelf off");
        assert!(!r.layers[0].on);
    }

    /// A band edited by hand is held to its ranges, keeps its channels, and
    /// changes what plays; one added starts flat; one taken out is gone.
    #[test]
    fn bands_are_edited_in_place() {
        use crate::config::{DspFilter, EqFilter, EqFilterKind};
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "Mine".into(),
                filters: vec![DspFilter::Band(EqFilter {
                    kind: EqFilterKind::Peaking,
                    freq: 100.0,
                    gain_db: 3.0,
                    q: 1.0,
                    channels: vec![1],
                })],
                ..Default::default()
            })
        })
        .unwrap();
        set_band("Mine", 0, "low_shelf", 80.0, 99.0, 0.0).unwrap();
        let filters = || detail("Mine").unwrap().filters;
        assert_eq!(
            filters()[0],
            DspFilter::Band(EqFilter {
                kind: EqFilterKind::LowShelf,
                freq: 80.0,
                gain_db: 30.0,
                q: 0.1,
                channels: vec![1],
            })
        );
        assert!(set_band("Mine", 0, "wobble", 80.0, 0.0, 1.0).is_err());
        assert!(set_band("Mine", 0, "peaking", f64::NAN, 0.0, 1.0).is_err());
        assert!(set_band("Mine", 5, "peaking", 80.0, 0.0, 1.0).is_err());
        assert_eq!(add_band("Mine").unwrap(), 1);
        assert_eq!(filters().len(), 2);
        let r = response("Mine", 48000).unwrap();
        assert_eq!(r.bands.len(), 2);
        remove_filter("Mine", 0).unwrap();
        assert_eq!(filters().len(), 1);
        assert!(remove_filter("Mine", 3).is_err());
        assert!(add_band("Nobody").is_err());
    }

    /// A headphone correction travels and a room correction stays; a stack
    /// follows its layers unless set; a stack kept everywhere cannot have a
    /// layer kept here, either way round.
    #[test]
    fn where_a_profile_is_kept() {
        use crate::config::{DspLayer, DspTarget};
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "HD 650".into(),
                filters: vec![band(1000.0)],
                target: Some(DspTarget {
                    made_for: "harman-over-ear-2018".into(),
                    chosen: None,
                }),
                ..Default::default()
            });
            c.dsp.profiles.push(DspProfile {
                name: "Room".into(),
                impulses: vec!["dsp/room/48000.wav".into()],
                ..Default::default()
            });
            c.dsp.profiles.push(DspProfile {
                name: "Bass".into(),
                filters: vec![band(80.0)],
                ..Default::default()
            });
            c.dsp.profiles.push(DspProfile {
                name: "Amp".into(),
                filters: vec![band(80.0)],
                devices: vec!["uuid:5f9ec1b3-ed59-79bb-4530-745e2b1d1b10".into()],
                ..Default::default()
            });
        })
        .unwrap();
        let kept = |name: &str| {
            let cfg = Config::cached();
            let p = cfg
                .dsp
                .profiles
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .clone();
            scope(&p, &cfg.dsp.profiles)
        };
        assert_eq!(kept("HD 650"), DspScope::Everywhere);
        assert_eq!(kept("Room"), DspScope::Device);
        assert_eq!(kept("Bass"), DspScope::Everywhere);
        assert_eq!(
            kept("Amp"),
            DspScope::Device,
            "an amplifier stays where it is"
        );

        let layer = |p: &str| DspLayer {
            profile: p.into(),
            on: true,
        };
        set_layers("Desk", vec![layer("HD 650"), layer("Bass")]).unwrap();
        assert_eq!(kept("Desk"), DspScope::Everywhere);
        set_layers("Speakers", vec![layer("Amp"), layer("Bass")]).unwrap();
        assert_eq!(
            kept("Speakers"),
            DspScope::Device,
            "a stack here may use shared layers"
        );

        set_scope("Desk", DspScope::Everywhere).unwrap();
        let refused = set_layers("Desk", vec![layer("HD 650"), layer("Amp")]).unwrap_err();
        assert!(refused.contains("Amp is kept on this device"), "{refused}");
        let refused = set_scope("Bass", DspScope::Device).unwrap_err();
        assert!(refused.contains("Bass is a layer of Desk"), "{refused}");
        set_scope("Speakers", DspScope::Everywhere).unwrap_err();
        set_scope("Desk", DspScope::Device).unwrap();
        set_scope("Bass", DspScope::Device).unwrap();
        assert_eq!(kept("Bass"), DspScope::Device);
    }

    /// A measurement as text: flat, with `bump` dB at 3 kHz.
    fn measured(bump: f64) -> String {
        let mut text = String::from("frequency,raw\n");
        for hz in super::super::targets::grid() {
            let db = bump * (-((hz / 3000.0).log2() * 3.0).powi(2)).exp();
            text.push_str(&format!("{hz:.2},{db:.2}\n"));
        }
        text
    }

    /// A headphone measured and corrected to a target: a correction, kept
    /// everywhere, playing target minus measurement, said in one line.
    #[test]
    fn a_measurement_is_corrected_to_its_target() {
        use super::super::targets;
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        let text = measured(6.0);
        assert!(
            read_measurement("1000,0\n").is_err(),
            "too little to correct from"
        );
        let preview = preview_measurement(&text, "harman-in-ear-2019", 48000).unwrap();
        assert!(preview.predicted.is_some() && preview.measurement.is_some());

        save_measured("AFUL Performer 8S", &text, DspEar::In, "harman-in-ear-2019").unwrap();
        assert!(
            save_measured("AFUL Performer 8S", &text, DspEar::In, "harman-in-ear-2019").is_err()
        );
        let cfg = Config::cached();
        let p = cfg
            .dsp
            .profiles
            .iter()
            .find(|p| p.name == "AFUL Performer 8S")
            .unwrap()
            .clone();
        assert_eq!(role(&p), DspRole::Correction);
        assert_eq!(scope(&p, &cfg.dsp.profiles), DspScope::Everywhere);
        assert_eq!(
            summary("AFUL Performer 8S").unwrap().correction.as_deref(),
            Some("AFUL Performer 8S → Harman in-ear 2019 (from measurement)")
        );

        // What plays is the correction from the measurement to the target.
        let r = response("AFUL Performer 8S", 48000).unwrap();
        let at = |curve: &[f64], hz: f64| curve[r.freqs.iter().position(|f| *f >= hz).unwrap()];
        let wanted = targets::correction(
            &targets::parse(&text),
            &targets::choice_curve("harman-in-ear-2019").unwrap(),
        );
        let expect = targets::at(&wanted.points, 3000.0);
        assert!(
            (at(&r.total, 3000.0) - expect).abs() < 1.0,
            "{} vs {expect}",
            at(&r.total, 3000.0)
        );
        // Held to ±3 dB in the treble, where rigs disagree.
        assert!(
            wanted
                .points
                .iter()
                .filter(|(hz, _)| *hz >= 10_000.0)
                .all(|(_, db)| db.abs() <= 3.0)
        );

        // Another target: the same measurement, corrected to it.
        let choices = target_choices("AFUL Performer 8S").unwrap();
        assert!(choices.made_for.is_none());
        choose_target("AFUL Performer 8S", Some("harman-in-ear-2019-without-bass")).unwrap();
        assert_eq!(
            summary("AFUL Performer 8S").unwrap().correction.as_deref(),
            Some("AFUL Performer 8S → Harman in-ear 2019, no bass shelf (from measurement)")
        );
    }

    /// One correction to a chain: a second layer is refused, naming the one
    /// already there, and one made so by its role is warned of; tuning on
    /// top is fine.
    #[test]
    fn a_chain_corrects_once() {
        use crate::config::{DspLayer, DspTarget};
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        persist(|c| {
            for (name, target) in [("HD 650", true), ("HD 600", true), ("Warm", false)] {
                c.dsp.profiles.push(DspProfile {
                    name: name.into(),
                    filters: vec![band(100.0)],
                    target: target.then(|| DspTarget {
                        made_for: "harman-over-ear-2018".into(),
                        chosen: None,
                    }),
                    ..Default::default()
                });
            }
        })
        .unwrap();
        let layer = |p: &str| DspLayer {
            profile: p.into(),
            on: true,
        };
        set_layers("Desk", vec![layer("HD 650"), layer("Warm")]).unwrap();
        let s = summary("Desk").unwrap();
        assert_eq!(
            s.correction.as_deref(),
            Some("HD 650: ready-made EQ (made for Harman over-ear 2018)")
        );
        assert_eq!(s.tunings, vec!["Warm".to_string()]);
        let refused = set_layers(
            "Desk",
            vec![layer("HD 650"), layer("Warm"), layer("HD 600")],
        )
        .unwrap_err();
        assert_eq!(
            refused,
            "This stack already corrects for HD 650. Remove that correction first, or add HD 600 as a tuning instead."
        );
        // Said to be a correction, it is one: the stack says so.
        set_role("Warm", DspRole::Correction).unwrap();
        assert_eq!(
            detail("Desk").unwrap().corrects_twice.as_deref(),
            Some("This stack corrects twice: HD 650 (ready-made) and Warm (correction). Keep one.")
        );
        set_role("Warm", DspRole::Tuning).unwrap();
        set_role("HD 600", DspRole::Tuning).unwrap();
        set_layers(
            "Desk",
            vec![layer("HD 650"), layer("Warm"), layer("HD 600")],
        )
        .unwrap();
    }

    /// A correction with a tuning baked in is the chain's correction. A stack
    /// made before a second correction was refused still plays, says so,
    /// and can be edited, but gains no other correction.
    #[test]
    fn a_baked_preset_is_the_correction() {
        use crate::config::{DspLayer, DspTarget};
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        let layer = |p: &str, on: bool| DspLayer {
            profile: p.into(),
            on,
        };
        persist(|c| {
            let autoeq = || {
                Some(DspTarget {
                    made_for: "harman-in-ear-2019".into(),
                    chosen: None,
                })
            };
            c.dsp.profiles.push(DspProfile {
                name: "Cantor".into(),
                filters: vec![band(100.0)],
                target: autoeq(),
                ..Default::default()
            });
            c.dsp.profiles.push(DspProfile {
                name: "HD 600".into(),
                filters: vec![band(100.0)],
                target: autoeq(),
                ..Default::default()
            });
            for name in ["Performer 8S", "Warm"] {
                c.dsp.profiles.push(DspProfile {
                    name: name.into(),
                    filters: vec![band(200.0)],
                    ..Default::default()
                });
            }
            // As 0.59 allowed: two corrections in one stack.
            c.dsp.profiles.push(DspProfile {
                name: "Old".into(),
                layers: vec![layer("Cantor", true), layer("Performer 8S", true)],
                ..Default::default()
            });
        })
        .unwrap();
        // AutoEQ's measurement, kept beside Cantor as an install keeps it.
        let folder = super::dir("Cantor");
        std::fs::create_dir_all(&folder).unwrap();
        let mut csv = String::from("frequency,raw,target\n");
        for hz in super::super::targets::grid() {
            csv.push_str(&format!("{hz:.2},0.0,0.0\n"));
        }
        std::fs::write(super::super::targets::result_path(&folder), csv).unwrap();
        let cfg = Config::cached();
        let named = |n: &str| cfg.dsp.profiles.iter().find(|p| p.name == n).unwrap();
        assert_eq!(
            role(named("Cantor")),
            DspRole::Correction,
            "an AutoEQ install"
        );
        assert_eq!(role(named("Warm")), DspRole::Tuning, "anything else");
        assert_eq!(corrects_twice(named("Old"), &cfg.dsp.profiles), None);

        set_role("Performer 8S", DspRole::Baked).unwrap();
        let d = detail("Old").unwrap();
        assert_eq!(
            d.corrects_twice.as_deref(),
            Some("This stack corrects twice: Cantor (AutoEQ) and Performer 8S (baked). Keep one.")
        );
        assert_eq!(
            d.layer_roles,
            vec![Some(DspRole::Correction), Some(DspRole::Baked)]
        );
        // It still plays, and can be edited towards one correction.
        let cfg = Config::cached();
        let old = cfg.dsp.profiles.iter().find(|p| p.name == "Old").unwrap();
        assert!(super::super::chain(old, &cfg.dsp.profiles, &mut Vec::new()).is_ok());
        set_layers(
            "Old",
            vec![layer("Cantor", true), layer("Performer 8S", false)],
        )
        .unwrap();
        let refused = set_layers(
            "Old",
            vec![
                layer("Cantor", true),
                layer("Performer 8S", false),
                layer("HD 600", true),
            ],
        )
        .unwrap_err();
        assert!(
            refused.starts_with("This stack already corrects"),
            "{refused}"
        );

        // A baked preset is a stack's correction: another is refused.
        set_layers(
            "Desk",
            vec![layer("Performer 8S", true), layer("Warm", true)],
        )
        .unwrap();
        let s = summary("Desk").unwrap();
        assert_eq!(
            s.correction.as_deref(),
            Some("Performer 8S (correction + tuning in one)")
        );
        assert!(s.baked);
        assert_eq!(s.tunings, vec!["Warm".to_string()]);
        let refused = set_layers(
            "Desk",
            vec![
                layer("Performer 8S", true),
                layer("Warm", true),
                layer("Cantor", true),
            ],
        )
        .unwrap_err();
        assert_eq!(
            refused,
            "This stack already corrects for Performer 8S. Remove that correction first, or add Cantor as a tuning instead."
        );
    }

    /// An AutoEQ correction moved to another target is rebuilt from the
    /// measurement AutoEQ kept, rather than played as its bands and a step.
    #[test]
    fn autoeq_moved_to_another_target_is_rebuilt() {
        use crate::config::{DspFilter, DspTarget};
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        config::set_config_dir(dir.path());
        let folder = super::dir("HD 650");
        std::fs::create_dir_all(&folder).unwrap();
        let mut csv = String::from("frequency,raw,target\n");
        for hz in super::super::targets::grid() {
            csv.push_str(&format!("{hz:.2},0.0,0.0\n"));
        }
        std::fs::write(super::super::targets::result_path(&folder), csv).unwrap();
        persist(|c| {
            c.dsp.profiles.push(DspProfile {
                name: "HD 650".into(),
                filters: vec![band(100.0), band(1000.0)],
                target: Some(DspTarget {
                    made_for: "harman-over-ear-2018".into(),
                    chosen: None,
                }),
                ..Default::default()
            })
        })
        .unwrap();
        let chain = || {
            let cfg = Config::cached();
            let p = cfg.dsp.profiles[0].clone();
            super::super::chain(&p, &cfg.dsp.profiles, &mut Vec::new()).unwrap()
        };
        assert_eq!(chain().len(), 2, "AutoEQ's bands, as made");
        choose_target("HD 650", Some("harman-over-ear-2018-without-bass")).unwrap();
        let rebuilt = chain();
        assert_eq!(rebuilt.len(), 1, "{rebuilt:?}");
        assert!(matches!(rebuilt[0], DspFilter::Graphic(_)));
        assert!(
            summary("HD 650")
                .unwrap()
                .correction
                .unwrap()
                .contains("rebuilt from AutoEQ's measurement")
        );
    }
}
