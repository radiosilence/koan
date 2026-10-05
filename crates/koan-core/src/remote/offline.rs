//! Whether this device is offline: told to be, or cut off from its server.
//!
//! Offline narrows what the library offers to what can play here, so a phone
//! underground shows the music it has rather than a library that fails on
//! every tap. It is on while the person has turned it on, and, on iOS, while
//! the link to the server has been down for `GRACE` with the app awake. The
//! link is dropped every time iOS suspends the app, so a link that is down
//! only counts while the app is in front. It lifts by itself when the link
//! comes back.
//!
//! A server that offers no link (Navidrome) gives no such signal, so only the
//! manual switch applies to it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long the link may be down, with the app awake, before the device is
/// offline: long enough for a reconnect after a hand-off between networks.
pub const GRACE: Duration = Duration::from_secs(8);

static MANUAL: AtomicBool = AtomicBool::new(false);

/// When the link last went down, or since launch while it has never come up.
/// `None` while it is up.
fn down_since() -> &'static Mutex<Option<Instant>> {
    static DOWN: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    DOWN.get_or_init(|| Mutex::new(Some(Instant::now())))
}

/// Turn offline mode on or off by hand.
pub fn set_manual(on: bool) {
    if MANUAL.swap(on, Ordering::AcqRel) != on {
        crate::remote::devices::touch();
    }
}

pub fn manual() -> bool {
    MANUAL.load(Ordering::Acquire)
}

/// The link to the server came up or went down. When it goes down, the
/// device is looked at again once `GRACE` has passed, which is when it may
/// become offline: a timer for that one moment, not a poll.
pub fn link_changed(up: bool) {
    let mut down = down_since().lock().unwrap_or_else(|e| e.into_inner());
    match (up, down.is_some()) {
        (true, true) => *down = None,
        (false, false) => {
            *down = Some(Instant::now());
            let _ = std::thread::Builder::new()
                .name("koan-offline".into())
                .spawn(|| {
                    std::thread::sleep(GRACE);
                    crate::remote::devices::touch();
                });
        }
        _ => {}
    }
}

/// Whether the library is narrowed to what can play here.
pub fn active() -> bool {
    manual() || cut_off()
}

/// Whether this device has lost its server: iOS only, for now, with the app
/// awake and a server that keeps a link.
pub fn cut_off() -> bool {
    if !cfg!(target_os = "ios") || !crate::quiet::awake() {
        return false;
    }
    let remote = &crate::config::Config::cached().remote;
    if !remote.enabled || remote.url.is_empty() {
        return false;
    }
    // A server known not to keep a link never reports one up.
    if crate::remote::profile::current().is_some_and(|p| !p.offers(crate::remote::profile::LINK)) {
        return false;
    }
    down_since()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|at| at.elapsed() >= GRACE)
}
