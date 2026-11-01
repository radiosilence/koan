//! The log: `koan.log` in the configuration directory, in the format every
//! koan front end writes, and stderr, for a launch from a terminal.
//!
//! Installed before anything else runs, so the single-instance handover is
//! logged too. The engine's own file logger then finds a logger in place and
//! stands down. Lines are written by a thread of their own, so logging from
//! the UI thread costs it a channel send and no I/O.

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

use koan_core::config;

enum Line {
    /// The file's line, and whether stderr gets it too.
    Write(String, bool),
    Flush(Sender<()>),
}

struct Logger(Sender<Line>);

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let line = format!(
            "[{now}] {}: {}",
            record.level().as_str().to_lowercase(),
            record.args()
        );
        // lofty and symphonia warn about every odd file a scan meets; the
        // file keeps them, the terminal is spared.
        let module = record.module_path().unwrap_or("");
        let noise = record.level() == log::Level::Warn
            && (module.starts_with("lofty") || module.starts_with("symphonia"));
        let _ = self.0.send(Line::Write(line, !noise));
    }

    /// Waits for what has been sent so far to be written.
    fn flush(&self) {
        let (done, written) = channel();
        if self.0.send(Line::Flush(done)).is_ok() {
            let _ = written.recv();
        }
    }
}

pub fn init() {
    static LOGGER: OnceLock<Logger> = OnceLock::new();
    let logger = LOGGER.get_or_init(|| {
        let (lines, writes) = channel::<Line>();
        let spawned = std::thread::Builder::new()
            .name("koan-gui-log".into())
            .spawn(move || {
                let mut file = config::LogFile::default();
                for line in writes {
                    match line {
                        Line::Write(line, echo) => {
                            file.write(format_args!("{line}"));
                            if echo {
                                // stderr without the timestamp, as the CLI prints it.
                                let msg = line.split_once("] ").map_or(&*line, |(_, m)| m);
                                eprintln!("{msg}");
                            }
                        }
                        Line::Flush(done) => {
                            file.flush();
                            let _ = done.send(());
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            eprintln!("log: no writer thread: {e}");
        }
        Logger(lines)
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
