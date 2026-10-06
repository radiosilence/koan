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

/// Stop processing `device` (the current output if not named).
pub fn cmd_dsp_clear(named: Option<String>) {
    let device = device(named);
    profiles::assign(None, &device).unwrap_or_else(|e| fail(e));
    println!("{} plays untouched", device.bold());
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

/// squig.link sites' measurements matching `query`, numbered; or the one
/// numbered `pick`, made into a correction.
pub fn cmd_dsp_squig(
    query: &str,
    limit: usize,
    pick: Option<usize>,
    name: Option<&str>,
    in_ear: Option<bool>,
    target: Option<&str>,
) {
    use koan_core::audio::dsp::squig;
    use koan_core::config::DspEar;
    let hits = squig::search(query, limit.max(pick.unwrap_or(0))).unwrap_or_else(|e| fail(e));
    let Some(pick) = pick else {
        for (i, h) in hits.iter().enumerate() {
            let rig = h
                .site
                .rig
                .map(|r| format!(" · {r} rig"))
                .unwrap_or_default();
            println!(
                "{}  {}  {}",
                format!("{:>3}", i + 1).dimmed(),
                h.name().bold(),
                format!("{}{rig}", h.site.label()).dimmed()
            );
        }
        return;
    };
    let hit = hits
        .get(pick.wrapping_sub(1))
        .unwrap_or_else(|| fail(format!("no result numbered {pick}")));
    let target = target.unwrap_or_else(|| fail("--target is needed to make a correction"));
    let in_ear = in_ear
        .or(hit
            .site
            .ear
            .map(|e| e == koan_core::audio::dsp::targets::Ear::In))
        .unwrap_or_else(|| fail("--ear is needed: the site does not say"));
    let text = squig::fetch(hit).unwrap_or_else(|e| fail(e));
    let name = name
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{} {}", hit.brand, hit.model));
    let ear = if in_ear { DspEar::In } else { DspEar::Over };
    let saved = profiles::save_measured(&name, &text, ear, target).unwrap_or_else(|e| fail(e));
    profiles::credit(&saved, &hit.source()).unwrap_or_else(|e| fail(e));
    println!(
        "{} '{}' from {}, corrected to {}",
        "measured".green(),
        saved.bold(),
        hit.source(),
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

pub fn cmd_dsp_remove(name: &str) {
    profiles::remove(name).unwrap_or_else(|e| fail(e));
    println!("{} '{}'", "removed".green(), name.bold());
}

pub fn cmd_dsp_enable(enabled: bool) {
    profiles::set_enabled(enabled).unwrap_or_else(|e| fail(e));
    println!("dsp {}", if enabled { "on" } else { "off" });
}
