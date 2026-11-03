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
//!
//! `KOAN_PLACES_DIR` puts all three under one directory instead
//! (`<dir>/state`, `<dir>/runtime`, `<dir>/cache`): what a throwaway
//! configuration's tools and examples set, so removing the throwaway removes
//! everything it wrote.

use std::path::{Path, PathBuf};

use crate::config;

/// The key for the platform's own config directory.
const DEFAULT: &str = "default";

/// The config directory's name among the runtime places: `default` for the
/// platform's own, otherwise its last component and a digest of its full
/// path, so two throwaway directories named alike stay apart. Worked out
/// once per process, with the directory made first, so a path through a
/// symlink (`/tmp` on macOS) keys the same before and after the engine
/// creates it.
pub fn key() -> String {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        let dir = config::config_dir();
        let _ = std::fs::create_dir_all(&dir);
        key_for(&dir, &config::platform_config_dir())
    })
    .clone()
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

/// For a tool run against a throwaway `KOAN_CONFIG_DIR` (the GUI's examples
/// and benches): keeps its runtime places in `places` inside that directory,
/// unless `KOAN_PLACES_DIR` already says where, so deleting the throwaway
/// deletes them too. Does nothing without a `KOAN_CONFIG_DIR`, or with one
/// naming the platform's own configuration, whose places stay where the
/// installed apps keep them. Call it first in `main`, before any thread
/// starts: it sets an environment variable.
pub fn keep_with_config() {
    if std::env::var_os("KOAN_PLACES_DIR").is_some() {
        return;
    }
    let Some(config) = std::env::var_os("KOAN_CONFIG_DIR") else {
        return;
    };
    if let Some(places) = places_with(Path::new(&config), &config::platform_config_dir()) {
        // SAFETY: documented as called before any other thread starts.
        unsafe { std::env::set_var("KOAN_PLACES_DIR", places) };
    }
}

/// Where `keep_with_config` keeps a configuration's places: inside it, unless
/// it is the platform's own.
fn places_with(config: &Path, platform: &Path) -> Option<PathBuf> {
    (key_for(config, platform) != DEFAULT).then(|| config.join("places"))
}

/// `KOAN_PLACES_DIR`'s `kind` directory, where it is set.
fn overridden(kind: &str) -> Option<PathBuf> {
    std::env::var_os("KOAN_PLACES_DIR")
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(dir).join(kind))
}

/// Kept across launches, for this config directory.
pub fn state() -> PathBuf {
    overridden("state").unwrap_or_else(|| under(dirs::state_dir().or_else(dirs::data_dir)))
}

/// Where locks go, for this config directory: cleared at log-out where the
/// platform has such a place.
pub fn runtime() -> PathBuf {
    if let Some(dir) = overridden("runtime") {
        return dir;
    }
    match dirs::runtime_dir() {
        Some(dir) => under(Some(dir)),
        None => state(),
    }
}

/// What may be deleted at any time, for this config directory.
pub fn cache() -> PathBuf {
    overridden("cache").unwrap_or_else(|| under(dirs::cache_dir()))
}

/// Moves `name` from the config directory into `to`, once: when it is still
/// in the config directory, holds something, and is not yet in `to`. Silent;
/// a file that cannot be moved is left where it was and logged, and tried
/// again at the next launch.
pub fn adopt(name: &str, to: &Path) {
    let from = config::config_dir().join(name);
    let dest = to.join(name);
    let holds_something = std::fs::metadata(&from).is_ok_and(|m| m.len() > 0);
    if !holds_something || dest.exists() {
        return;
    }
    let moved = std::fs::create_dir_all(to)
        .and_then(|()| std::fs::rename(&from, &dest).or_else(|_| copy_across(&from, &dest)));
    match moved {
        Ok(()) => log::info!("moved {} to {}", from.display(), dest.display()),
        Err(e) => log::warn!(
            "could not move {} to {}: {e}",
            from.display(),
            dest.display()
        ),
    }
}

/// Across volumes: copied whole beside `dest` and renamed into place, so
/// `dest` never exists half-written, then the original removed.
fn copy_across(from: &Path, dest: &Path) -> std::io::Result<()> {
    let partial = dest.with_extension("part");
    let copied = std::fs::copy(from, &partial)
        .and_then(|_| std::fs::File::open(&partial)?.sync_all())
        .and_then(|()| std::fs::rename(&partial, dest));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    std::fs::remove_file(from)
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
    fn only_a_throwaway_configuration_keeps_its_places_with_it() {
        let platform = tempfile::tempdir().unwrap();
        let throwaway = tempfile::tempdir().unwrap();
        assert_eq!(places_with(platform.path(), platform.path()), None);
        assert_eq!(
            places_with(throwaway.path(), platform.path()),
            Some(throwaway.path().join("places"))
        );
    }

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

    #[test]
    fn an_empty_file_is_not_carried_over() {
        config::isolate_config_for_tests();
        let name = "places-adopt-empty";
        let to = tempfile::tempdir().unwrap();
        std::fs::write(config::config_dir().join(name), "").unwrap();
        adopt(name, to.path());
        assert!(!to.path().join(name).exists());
        std::fs::remove_file(config::config_dir().join(name)).ok();
    }

    #[test]
    fn a_copy_across_volumes_lands_whole_or_not_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let (from, dest) = (dir.path().join("from"), dir.path().join("dest"));
        std::fs::write(&from, "state").unwrap();
        copy_across(&from, &dest).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "state");
        assert!(!from.exists() && !dest.with_extension("part").exists());
        // A missing source leaves nothing behind.
        assert!(copy_across(&from, &dir.path().join("other")).is_err());
        assert!(!dir.path().join("other").exists());
    }
}
