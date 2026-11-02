//! Where a front end keeps what it writes as it runs, outside the config
//! directory: window and session state, locks, temporary files.
//!
//! The config directory holds what a person chooses and may keep under
//! version control (`~/.config/koan` often lives in a dotfiles repository),
//! so runtime files do not belong in it. Each place here is the platform's
//! own directory for its kind, under `koan/<key>`, where the key names the
//! config directory: one set of runtime files per configuration, so a
//! throwaway `KOAN_CONFIG_DIR` keeps its state apart from the person's.
//!
//! - `state`: kept across launches. `$XDG_STATE_HOME` on Linux,
//!   `~/Library/Application Support` on macOS.
//! - `runtime`: locks. `$XDG_RUNTIME_DIR` on Linux, else `state`.
//! - `cache`: anything that may be deleted. `$XDG_CACHE_HOME` on Linux,
//!   `~/Library/Caches` on macOS.

use std::path::{Path, PathBuf};

use crate::config;

/// The key for the platform's own config directory.
const DEFAULT: &str = "default";

/// The config directory's name among the runtime places: `default` for the
/// platform's own, otherwise its last component and a digest of its full
/// path, so two throwaway directories named alike stay apart.
pub fn key() -> String {
    key_for(&config::config_dir(), &config::platform_config_dir())
}

fn key_for(dir: &Path, platform: &Path) -> String {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let dir = canonical(dir);
    if dir == canonical(platform) {
        return DEFAULT.to_string();
    }
    let digest = md5::compute(dir.as_os_str().as_encoded_bytes());
    let name: String = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(32)
        .collect();
    format!("{name}-{}", &format!("{digest:x}")[..12])
}

fn under(base: Option<PathBuf>) -> PathBuf {
    base.unwrap_or_else(std::env::temp_dir)
        .join("koan")
        .join(key())
}

/// Kept across launches, for this config directory.
pub fn state() -> PathBuf {
    under(dirs::state_dir().or_else(dirs::data_dir))
}

/// Where locks go, for this config directory: cleared at log-out where the
/// platform has such a place.
pub fn runtime() -> PathBuf {
    match dirs::runtime_dir() {
        Some(dir) => under(Some(dir)),
        None => state(),
    }
}

/// What may be deleted at any time, for this config directory.
pub fn cache() -> PathBuf {
    under(dirs::cache_dir())
}

/// Moves `name` from the config directory into `to`, once: when it is still
/// in the config directory and not yet in `to`. Silent; a file that cannot be
/// moved is left where it was and logged.
pub fn adopt(name: &str, to: &Path) {
    let from = config::config_dir().join(name);
    let dest = to.join(name);
    if !from.exists() || dest.exists() {
        return;
    }
    let moved = std::fs::create_dir_all(to).and_then(|()| {
        std::fs::rename(&from, &dest).or_else(|_| {
            // Across volumes: copied, then the original removed.
            std::fs::copy(&from, &dest).and_then(|_| std::fs::remove_file(&from))
        })
    });
    match moved {
        Ok(()) => log::info!("moved {} to {}", from.display(), dest.display()),
        Err(e) => log::warn!(
            "could not move {} to {}: {e}",
            from.display(),
            dest.display()
        ),
    }
}

/// Removes a lock file left in the config directory by an earlier version,
/// unless a process still holds it.
pub fn drop_old_lock(name: &str) {
    let path = config::config_dir().join(name);
    let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) else {
        return;
    };
    if file.try_lock().is_ok() {
        drop(file);
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platform_directory_is_the_default_and_others_are_apart() {
        let platform = tempfile::tempdir().unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert_eq!(key_for(platform.path(), platform.path()), DEFAULT);
        let (ka, kb) = (
            key_for(a.path(), platform.path()),
            key_for(b.path(), platform.path()),
        );
        assert_ne!(ka, kb);
        assert_eq!(ka, key_for(a.path(), platform.path()), "stable");
        assert!(
            ka.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        );
    }

    #[test]
    fn adopting_moves_a_file_once() {
        config::isolate_config_for_tests();
        let name = "places-adopt-state";
        let to = tempfile::tempdir().unwrap();
        std::fs::write(config::config_dir().join(name), "old").unwrap();
        adopt(name, to.path());
        assert_eq!(
            std::fs::read_to_string(to.path().join(name)).unwrap(),
            "old"
        );
        assert!(!config::config_dir().join(name).exists());
        // A file already in its new place is never overwritten.
        std::fs::write(config::config_dir().join(name), "older").unwrap();
        adopt(name, to.path());
        assert_eq!(
            std::fs::read_to_string(to.path().join(name)).unwrap(),
            "old"
        );
        std::fs::remove_file(config::config_dir().join(name)).ok();
    }
}
