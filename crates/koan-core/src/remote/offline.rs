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

/// Since when the link has been down with the app awake: from launch, from
/// the link dropping, or from the app waking with it down. `None` while it is
/// up.
fn down_since() -> &'static Mutex<Option<Instant>> {
    static DOWN: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    DOWN.get_or_init(|| {
        arm();
        Mutex::new(Some(Instant::now()))
    })
}

/// Look at the device again once `GRACE` has passed, which is when it may
/// become offline: a timer for that one moment, not a poll.
fn arm() {
    let _ = std::thread::Builder::new()
        .name("koan-offline".into())
        .spawn(|| {
            std::thread::sleep(GRACE);
            crate::remote::devices::touch();
        });
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

/// The link to the server came up or went down.
pub fn link_changed(up: bool) {
    let mut down = down_since().lock().unwrap_or_else(|e| e.into_inner());
    match (up, down.is_some()) {
        (true, true) => *down = None,
        (false, false) => {
            *down = Some(Instant::now());
            arm();
        }
        _ => {}
    }
}

/// The app came forward. Time spent suspended does not count towards the
/// grace: the link was hung up on purpose, and gets its chance to come back.
pub fn woke() {
    let mut down = down_since().lock().unwrap_or_else(|e| e.into_inner());
    if down.is_some() {
        *down = Some(Instant::now());
        arm();
    }
}

/// Whether the library is narrowed to what can play here.
pub fn active() -> bool {
    manual() || cut_off()
}

/// Whether this device has lost its server: iOS and tvOS only, for now, with the app
/// awake and a server that keeps a link.
pub fn cut_off() -> bool {
    if !cfg!(any(target_os = "ios", target_os = "tvos")) || !crate::quiet::awake() {
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
