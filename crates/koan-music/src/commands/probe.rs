use std::path::Path;

use koan_core::audio::{buffer, device};
use owo_colors::OwoColorize;

use super::format_time;

pub fn cmd_probe(path: &Path) {
    if !path.exists() {
        eprintln!("{} {}", "not found:".red().bold(), path.display());
        std::process::exit(1);
    }

    match buffer::probe_file(path) {
        Ok(info) => {
            println!("{} {}", "file:".cyan(), path.display());
            println!("{} {}", "codec:".cyan(), info.codec.yellow());
            println!("{} {} Hz", "sample rate:".cyan(), info.sample_rate);
            println!("{} {}", "bit depth:".cyan(), info.bit_depth);
            println!("{} {}", "channels:".cyan(), info.channels);
            println!(
                "{} {} {}",
                "duration:".cyan(),
                format_time(info.duration_ms),
                format!("({}ms)", info.duration_ms).dimmed()
            );
        }
        Err(e) => {
            eprintln!("{} {}", "probe failed:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

pub fn cmd_devices() {
    match device::list_output_devices() {
        Ok(devices) => {
            let default_id = device::default_output_device().ok();
            for dev in &devices {
                let is_default = Some(dev.id) == default_id;
                let marker = if is_default {
                    " *".yellow().bold().to_string()
                } else {
                    String::new()
                };
                println!(
                    "{} {}{}",
                    format!("[{}]", dev.id).dimmed(),
                    if is_default {
                        dev.name.bold().to_string()
                    } else {
                        dev.name.to_string()
                    },
                    marker,
                );
                if !dev.sample_rates.is_empty() {
                    let rates: Vec<String> = dev
                        .sample_rates
                        .iter()
                        .map(|r| format!("{}Hz", *r as u32))
                        .collect();
                    println!("  {} {}", "rates:".dimmed(), rates.join(", ").dimmed());
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}
