//! The app's runtime files outside the config directory, which holds what a
//! person chooses and often lives in a dotfiles repository. State goes to
//! `places::state`, locks to `places::runtime`, temporary files to
//! `places::cache`, each keyed by the config directory.
//!
//! An earlier version wrote them into the config directory; on start-up
//! they are moved to their places, once and silently, so the window, the
//! browsers' sorts and the desktop device's identity survive the upgrade.

use koan_core::places;

/// Called first, before the instance lock or the engine reads any of them.
pub fn adopt() {
    let state = places::state();
    if let Err(e) = std::fs::create_dir_all(&state) {
        log::warn!("state directory {}: {e}", state.display());
    }
    for name in [
        crate::window_state::FILE,
        crate::library::browse::STATE,
        "desktop-devices.json",
        "desktop-device-id",
    ] {
        places::adopt(name, &state);
    }
    // A lock is not moved: one an earlier version still holds stays with
    // it, and the rest are removed.
    for name in ["desktop-device.lock", "gui-instance.lock"] {
        places::drop_old_lock(name);
    }
}
