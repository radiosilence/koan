use std::path::PathBuf;

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

pub fn cmd_dsp_remove(name: &str) {
    profiles::remove(name).unwrap_or_else(|e| fail(e));
    println!("{} '{}'", "removed".green(), name.bold());
}

pub fn cmd_dsp_enable(enabled: bool) {
    profiles::set_enabled(enabled).unwrap_or_else(|e| fail(e));
    println!("dsp {}", if enabled { "on" } else { "off" });
}
