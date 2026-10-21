//! Pair with, wake and browse for Apple TVs over the Companion protocol,
//! against real hardware.
//!
//! cargo run -p koan-core --example companion -- browse
//! cargo run -p koan-core --example companion -- pair HOST:PORT > tv.json   (type the PIN)
//! cargo run -p koan-core --example companion -- wake HOST:PORT tv.json

use koan_core::remote::companion;
use std::time::Duration;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let addr = || args[1].parse().expect("HOST:PORT");
    match args.first().map(String::as_str) {
        Some("browse") => {
            for tv in companion::browse(Duration::from_secs(3)) {
                println!("{tv:?}");
            }
        }
        Some("pair") => {
            let pairing = companion::begin_pairing(addr()).expect("pairing");
            eprint!("PIN: ");
            let mut pin = String::new();
            std::io::stdin().read_line(&mut pin).unwrap();
            let credential = pairing.finish(pin.trim(), "kōan probe").expect("finish");
            println!("{}", serde_json::to_string(&credential).unwrap());
        }
        Some("wake") => {
            let credential = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap())
                .expect("a credential");
            let started = std::time::Instant::now();
            companion::wake(addr(), &credential, "kōan probe").expect("wake");
            eprintln!("woken in {:?}", started.elapsed());
        }
        _ => eprintln!("browse | pair HOST:PORT | wake HOST:PORT FILE"),
    }
}
