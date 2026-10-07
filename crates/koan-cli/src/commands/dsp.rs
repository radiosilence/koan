use std::path::PathBuf;

use koan_core::audio::dsp::autoeq;
use koan_core::audio::dsp::import::{self, ImportError};
use koan_core::audio::dsp::profiles;
use owo_colors::OwoColorize;

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{} {msg}", "error:".red().bold());
    std::process::exit(1);
}

fn device(named: Option<String>) -> String {
    named
        .or_else(profiles::current_device)
        .unwrap_or_else(|| fail("no output device"))
}

pub fn cmd_dsp_list() {
    let o = profiles::overview();
    println!(
        "{} {}{}",
        "output:".cyan(),
        o.device.as_deref().unwrap_or("none").bold(),
        if o.enabled {
            String::new()
        } else {
            format!(" {}", "(dsp off: every profile bypassed)".yellow())
        }
    );
    if o.profiles.is_empty() {
        println!(
            "{}",
            "no profiles — koan dsp import <file, folder or zip>".dimmed()
        );
        return;
    }
    for p in &o.profiles {
        let marker = if o.active.as_ref() == Some(&p.name) {
            " *".yellow().bold().to_string()
        } else if o.tuning.as_ref() == Some(&p.name) {
            " + tuning".yellow().to_string()
        } else {
            String::new()
        };
        println!("{}{marker}", p.name.bold());
        if !p.devices.is_empty() {
            println!("  {} {}", "devices:".dimmed(), p.devices.join(", "));
        }
        if p.layers > 0 {
            println!("  {} {}", "layers:".dimmed(), p.layers);
        }
        if p.bands > 0 {
            println!("  {} {}", "filters:".dimmed(), p.bands);
        }
        if !p.rates.is_empty() {
            let rates: Vec<String> = p.rates.iter().map(|r| format!("{r} Hz")).collect();
            println!("  {} {}", "impulses:".dimmed(), rates.join(", "));
        }
        if let Some(problem) = &p.problem {
            println!("  {} {}", "not loading:".red(), problem);
        }
    }
}

/// Make or update a profile from EQ or filter files, in any format koan reads.
pub fn cmd_dsp_import(
    paths: &[PathBuf],
    name: Option<String>,
    rate: Option<u32>,
    device: Option<String>,
) {
    let imported = match import::import(paths, rate) {
        Ok(i) => i,
        Err(ImportError::NeedsRate(what)) => fail(format!(
            "{what} does not say what sample rate it is at; pass --rate"
        )),
        Err(e) => fail(e),
    };
    let filters = imported.filters.len();
    let rates: Vec<String> = imported
        .impulses
        .iter()
        .map(|i| format!("{} Hz", i.rate))
        .collect();
    let name = profiles::save(imported, name.as_deref()).unwrap_or_else(|e| fail(e));
    let mut what = Vec::new();
    if filters > 0 {
        what.push(format!("{filters} filters"));
    }
    if !rates.is_empty() {
        what.push(format!("impulses at {}", rates.join(", ")));
    }
    println!(
        "{} '{}': {}",
        "imported".green(),
        name.bold(),
        what.join(", ")
    );
    if let Some(device) = device {
        cmd_dsp_use(&name, Some(device));
    }
}

/// Play `device` (the current output if not named) through `name`.
pub fn cmd_dsp_use(name: &str, named: Option<String>) {
    let device = device(named);
    profiles::assign(Some(name), &device).unwrap_or_else(|e| fail(e));
    println!("{} plays through '{}'", device.bold(), name.bold());
}

/// Make `device` (the current output if not named) flat.
pub fn cmd_dsp_clear(named: Option<String>) {
    let device = device(named);
    profiles::apply_preset(&device, None).unwrap_or_else(|e| fail(e));
    println!("{} is flat: it plays untouched", device.bold());
}

/// AutoEQ's results matching `query`, best first, numbered as `install`
/// takes them.
pub fn cmd_dsp_autoeq_search(query: &str, limit: usize, refresh: bool) {
    let freshness = if refresh {
        autoeq::Freshness::Refresh
    } else {
        autoeq::Freshness::Daily
    };
    let entries = autoeq::index(freshness).unwrap_or_else(|e| fail(e));
    let found = autoeq::search(&entries, query, limit);
    if found.is_empty() {
        println!("{}", "no matches".dimmed());
        return;
    }
    let width = found
        .iter()
        .map(|e| e.number.to_string().len())
        .max()
        .unwrap_or(1);
    for e in found {
        println!(
            "{:>width$}  {}  {}",
            e.number.to_string().dimmed(),
            e.name.bold(),
            e.measured_by().dimmed()
        );
    }
}

/// Install an AutoEQ result as a profile, and play `device` through it if
/// named.
pub fn cmd_dsp_autoeq_install(wanted: &str, source: Option<&str>, device: Option<String>) {
    // A number refers to the index search showed, so the copy kept is used
    // however old it is.
    let entries = autoeq::index(autoeq::Freshness::Kept).unwrap_or_else(|e| fail(e));
    let Some(entry) = autoeq::find(&entries, wanted, source) else {
        let near: Vec<String> = autoeq::search(&entries, wanted, 5)
            .iter()
            .map(|e| format!("{} {} ({})", e.number, e.name, e.measured_by()))
            .collect();
        if near.is_empty() {
            fail(format!("nothing in AutoEQ called {wanted}"));
        }
        fail(format!(
            "nothing in AutoEQ called {wanted}{}; closest:\n  {}",
            source.map(|s| format!(" from {s}")).unwrap_or_default(),
            near.join("\n  ")
        ));
    };
    let name = autoeq::install(entry).unwrap_or_else(|e| fail(e));
    println!("{} '{}'", "installed".green(), name.bold());
    if let Some(device) = device {
        cmd_dsp_use(&name, Some(device));
    }
}

/// Show the targets `name` can move to, or move it.
pub fn cmd_dsp_target(name: &str, target: Option<&str>, reset: bool) {
    if target.is_some() || reset {
        profiles::choose_target(name, target).unwrap_or_else(|e| fail(e));
    }
    let Some(t) = profiles::target_choices(name) else {
        fail(format!(
            "{name} has no known target: only corrections installed from AutoEQ can move"
        ));
    };
    let current = t
        .chosen
        .clone()
        .or_else(|| t.made_for.map(|m| m.id.to_owned()))
        .unwrap_or_default();
    match t.made_for {
        Some(m) => println!("{} {}", "made for:".cyan(), m.name.bold()),
        None => println!("{} {}", "made for:".cyan(), "unknown".dimmed()),
    }
    for c in &t.choices {
        let marker = if c.id == current {
            "*".yellow().bold().to_string()
        } else {
            " ".into()
        };
        println!("{marker} {}  {}", c.id.bold(), c.name.dimmed());
        if !c.character.is_empty() {
            println!("    {}", c.character.dimmed());
        }
    }
}

/// Make `name` a stack of `layers`, all on.
pub fn cmd_dsp_stack(name: &str, layers: &[String]) {
    let layers = layers
        .iter()
        .map(|l| koan_core::config::DspLayer {
            profile: l.clone(),
            on: true,
        })
        .collect();
    profiles::set_layers(name, layers).unwrap_or_else(|e| fail(e));
    println!("{} '{}'", "stacked".green(), name.bold());
}

/// Switch `layer` of `stack` on or off.
pub fn cmd_dsp_layer(stack: &str, layer: &str, on: bool) {
    let mut layers = profiles::detail(stack)
        .unwrap_or_else(|| fail(format!("no profile called {stack}")))
        .layers;
    let Some(l) = layers.iter_mut().find(|l| l.profile == layer) else {
        fail(format!("{layer} is not a layer of {stack}"));
    };
    l.on = on;
    profiles::set_layers(stack, layers).unwrap_or_else(|e| fail(e));
    println!("{layer} {} in {stack}", if on { "on" } else { "off" });
}

/// Add a target to choose from.
pub fn cmd_dsp_add_target(path: &std::path::Path) {
    let added = koan_core::audio::dsp::targets::add(path).unwrap_or_else(|e| fail(e));
    println!(
        "{} '{}' as {}",
        "added".green(),
        added.name.bold(),
        added.id
    );
}

/// Correct the headphone `name` from the measurement at `path` to `target`.
pub fn cmd_dsp_measure(path: &std::path::Path, name: &str, in_ear: bool, target: &str) {
    use koan_core::config::DspEar;
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    let ear = if in_ear { DspEar::In } else { DspEar::Over };
    let saved = profiles::save_measured(name, &text, ear, target).unwrap_or_else(|e| fail(e));
    println!(
        "{} '{}', corrected to {}",
        "measured".green(),
        saved.bold(),
        profiles::target_name(target)
    );
}

/// Split the baked EQ `name` into a correction and a tuning.
pub fn cmd_dsp_split(name: &str, path: &std::path::Path, in_ear: bool, target: &str) {
    use koan_core::config::DspEar;
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| fail(format!("{}: {e}", path.display())));
    let ear = if in_ear { DspEar::In } else { DspEar::Over };
    let (correction, tuning) =
        profiles::split_baked(name, &text, ear, target).unwrap_or_else(|e| fail(e));
    println!(
        "{} '{}' into '{}' and '{}', made against {}",
        "split".green(),
        name.bold(),
        correction.bold(),
        tuning.bold(),
        profiles::target_name(target)
    );
}

/// Say what `name` is for: `correction`, `tuning` or `baked`.
pub fn cmd_dsp_role(name: &str, role: &str) {
    use koan_core::config::DspRole;
    let to = match role {
        "correction" => DspRole::Correction,
        "baked" => DspRole::Baked,
        _ => DspRole::Tuning,
    };
    profiles::set_role(name, to).unwrap_or_else(|e| fail(e));
    let what = match to {
        DspRole::Correction => "a neutral correction",
        DspRole::Tuning => "a tuning",
        DspRole::Baked => "a correction with a tuning baked in",
    };
    println!("'{}' is {what}", name.bold());
}

/// The target the ready-made EQ `name` was made for, or `None` for unknown.
pub fn cmd_dsp_made_for(name: &str, target: Option<&str>) {
    profiles::set_made_for(name, target).unwrap_or_else(|e| fail(e));
    match target {
        Some(t) => println!(
            "'{}' was made for {}",
            name.bold(),
            profiles::target_name(t)
        ),
        None => println!("'{}' was made for an unknown target", name.bold()),
    }
}

/// Play `tuning` on top of `device`'s correction (the current output if not
/// named), or none.
pub fn cmd_dsp_tuning(names: &[String], off: &[String], named: Option<String>) {
    let device = device(named);
    let list: Vec<(String, bool)> = names
        .iter()
        .filter(|n| n.as_str() != "none")
        .map(|n| (n.clone(), !off.contains(n)))
        .collect();
    profiles::set_tunings(&device, &list).unwrap_or_else(|e| fail(e));
    if list.is_empty() {
        println!("{} plays no tuning", device.bold());
    } else {
        let names: Vec<String> = list
            .iter()
            .map(|(n, on)| if *on { n.clone() } else { format!("{n} (off)") })
            .collect();
        println!("{} plays {} on top", device.bold(), names.join(", ").bold());
    }
}

/// Save `device`'s correction and tuning as the preset `name`.
pub fn cmd_dsp_preset_save(name: &str, named: Option<String>) {
    let device = device(named);
    let saved = profiles::save_preset(&device, name).unwrap_or_else(|e| fail(e));
    println!(
        "{} '{}' from {}",
        "saved".green(),
        saved.bold(),
        device.bold()
    );
}

/// Set `device` from the preset `name`, or flat.
pub fn cmd_dsp_preset_use(name: Option<&str>, named: Option<String>) {
    let device = device(named);
    profiles::apply_preset(&device, name).unwrap_or_else(|e| fail(e));
    match name {
        Some(n) => println!("{} plays '{}'", device.bold(), n.bold()),
        None => println!("{} plays flat, untouched", device.bold()),
    }
}

/// Put `name` back as it was imported.
pub fn cmd_dsp_revert(name: &str) {
    profiles::revert(name).unwrap_or_else(|e| fail(e));
    println!("'{}' is as imported", name.bold());
}

/// Copy `name` as it is now.
pub fn cmd_dsp_copy(name: &str, new: Option<&str>) {
    let copy = profiles::duplicate(name, new).unwrap_or_else(|e| fail(e));
    println!(
        "{} '{}' as '{}'",
        "copied".green(),
        name.bold(),
        copy.bold()
    );
}

/// Record the target the tuning `name` was made against, or that it is not
/// known.
pub fn cmd_dsp_tuned_for(name: &str, target: Option<&str>) {
    profiles::set_tuned_for(name, target).unwrap_or_else(|e| fail(e));
    match target {
        Some(t) => println!(
            "'{}' was made against {}",
            name.bold(),
            profiles::target_name(t)
        ),
        None => println!("'{}' plays as it is on any correction", name.bold()),
    }
}

pub fn cmd_dsp_remove(name: &str) {
    profiles::remove(name).unwrap_or_else(|e| fail(e));
    println!("{} '{}'", "removed".green(), name.bold());
}
