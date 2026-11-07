//! The guard the examples that run a real engine share: they refuse any
//! configuration but a throwaway one.

use std::error::Error;

/// Refuses any configuration but a throwaway one: not the person's own, muted,
/// and holding only generated libraries.
pub fn throwaway() -> Result<(), Box<dyn Error>> {
    let Some(dir) = std::env::var_os("KOAN_CONFIG_DIR") else {
        return Err("set KOAN_CONFIG_DIR to a throwaway configuration (just gui-smoke, just gui-present-frames)".into());
    };
    let dir = std::fs::canonicalize(&dir)?;
    let own = std::env::var_os("HOME")
        .map(|home| std::path::Path::new(&home).join(".config/koan"))
        .and_then(|own| std::fs::canonicalize(own).ok());
    if own.as_deref() == Some(dir.as_path()) {
        return Err("KOAN_CONFIG_DIR is your own configuration".into());
    }
    let config = koan_core::config::Config::load()?;
    if !config.playback.muted {
        return Err("the configuration plays aloud: set [playback] muted = true".into());
    }
    if config.library.folders.is_empty() {
        return Err("the configuration has no library: run `just gui-library`".into());
    }
    for folder in &config.library.folders {
        if !folder.join(".koan-generated").exists() {
            return Err(format!(
                "{} is not a library `just gui-library` generated",
                folder.display()
            )
            .into());
        }
    }
    Ok(())
}
