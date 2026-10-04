use std::path::{Path, PathBuf};

use koan_core::audio;
use koan_core::audio::dsp::{self, Setup};
use koan_core::config::{self, Config, DspProfile};
use owo_colors::OwoColorize;

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{} {msg}", "error:".red().bold());
    std::process::exit(1);
}

fn persist(mutate: impl FnOnce(&mut Config)) {
    if let Err(e) = Config::persist(mutate) {
        fail(format!("saving config: {e}"));
    }
}

/// The device koan plays to: the configured one, else the system default.
fn current_device() -> String {
    let cfg = Config::load_or_default();
    cfg.playback
        .output_device
        .or_else(|| audio::default_output_device().ok().map(|d| d.name))
        .unwrap_or_else(|| fail("no output device"))
}

pub fn cmd_dsp_list() {
    let cfg = Config::load_or_default();
    let device = current_device();
    let active = cfg.dsp.profile_for(&device).map(|p| p.name.clone());
    println!(
        "{} {}{}",
        "output:".cyan(),
        device.bold(),
        if cfg.dsp.enabled {
            String::new()
        } else {
            format!(" {}", "(dsp off: every profile bypassed)".yellow())
        }
    );
    if cfg.dsp.profiles.is_empty() {
        println!(
            "{}",
            "no profiles — koan dsp import <ParametricEQ.txt>".dimmed()
        );
        return;
    }
    for p in &cfg.dsp.profiles {
        let marker = if active.as_ref() == Some(&p.name) {
            " *".yellow().bold().to_string()
        } else {
            String::new()
        };
        println!("{}{marker}", p.name.bold());
        if !p.devices.is_empty() {
            println!("  {} {}", "devices:".dimmed(), p.devices.join(", "));
        }
        match p.preamp_db {
            Some(db) => println!("  {} {db:.1} dB", "preamp:".dimmed()),
            None => println!("  {} {}", "preamp:".dimmed(), "derived".dimmed()),
        }
        if !p.filters.is_empty() {
            println!("  {} {}", "bands:".dimmed(), p.filters.len());
        }
        for path in &p.impulses {
            println!("  {} {}", "impulse:".dimmed(), path.display());
        }
    }
}

/// Make a profile from an AutoEQ / Equalizer APO `ParametricEQ.txt`. The
/// preamp is left to be derived, which also covers convolution added later.
pub fn cmd_dsp_import(file: &Path, name: Option<String>, device: Option<String>) {
    let text =
        std::fs::read_to_string(file).unwrap_or_else(|e| fail(format!("{}: {e}", file.display())));
    let parsed =
        dsp::autoeq::parse(&text).unwrap_or_else(|e| fail(format!("{}: {e}", file.display())));
    if parsed.filters.is_empty() {
        fail(format!("{}: no filters in it", file.display()));
    }
    let name = name.unwrap_or_else(|| {
        file.file_stem()
            .map(|s| {
                s.to_string_lossy()
                    .trim_end_matches(" ParametricEQ")
                    .to_string()
            })
            .unwrap_or_else(|| "imported".into())
    });
    let bands = parsed.filters.len();
    persist(|cfg| {
        let profile = profile_mut(&mut cfg.dsp.profiles, &name);
        profile.filters = parsed.filters;
        profile.preamp_db = None;
    });
    println!(
        "{} '{}' with {bands} bands",
        "imported".green(),
        name.bold()
    );
    if let Some(device) = device {
        cmd_dsp_use(&name, Some(device));
    }
}

/// Give a profile its impulse responses, one WAV per rate.
pub fn cmd_dsp_impulse(name: &str, files: &[PathBuf]) {
    let impulses: Vec<PathBuf> = files
        .iter()
        .map(|f| std::path::absolute(f).unwrap_or_else(|e| fail(format!("{}: {e}", f.display()))))
        .collect();
    let probe = DspProfile {
        name: name.into(),
        impulses: impulses.clone(),
        ..Default::default()
    };
    // Read now, so a file koan cannot use fails here rather than at playback.
    let setup = Setup::load(&probe, &config::config_dir())
        .unwrap_or_else(|e| fail(e))
        .unwrap_or_else(|| fail("no impulse responses given"));
    for rate in [44100, 48000, 88200, 96000, 176400, 192000] {
        let out = setup.output_rate(rate);
        if out == rate {
            println!("  {} Hz {}", rate, "convolved at its own rate".dimmed());
        } else {
            println!("  {} Hz {} {} Hz", rate, "resampled to".yellow(), out);
        }
    }
    persist(|cfg| profile_mut(&mut cfg.dsp.profiles, name).impulses = impulses);
    println!("{} '{}'", "updated".green(), name.bold());
}

/// Play `device` (the current output if not named) through `name`.
pub fn cmd_dsp_use(name: &str, device: Option<String>) {
    let device = device.unwrap_or_else(current_device);
    let cfg = Config::load_or_default();
    if !cfg.dsp.profiles.iter().any(|p| p.name == name) {
        fail(format!("no profile '{name}'"));
    }
    persist(|cfg| {
        for p in &mut cfg.dsp.profiles {
            p.devices.retain(|d| *d != device);
            if p.name == name {
                p.devices.push(device.clone());
            }
        }
    });
    println!("{} plays through '{}'", device.bold(), name.bold());
}

/// Stop processing `device` (the current output if not named).
pub fn cmd_dsp_clear(device: Option<String>) {
    let device = device.unwrap_or_else(current_device);
    persist(|cfg| {
        for p in &mut cfg.dsp.profiles {
            p.devices.retain(|d| *d != device);
        }
    });
    println!("{} plays untouched", device.bold());
}

pub fn cmd_dsp_remove(name: &str) {
    persist(|cfg| cfg.dsp.profiles.retain(|p| p.name != name));
    println!("{} '{}'", "removed".green(), name.bold());
}

pub fn cmd_dsp_enable(enabled: bool) {
    persist(|cfg| cfg.dsp.enabled = enabled);
    println!("dsp {}", if enabled { "on" } else { "off" });
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
