//! What koan may do that nobody asked it to.
//!
//! A phone app in the background runs nothing on its own account: no link to
//! the server, no nearby browse or dial, no sync, no rescans. Each waits here
//! and catches up when the app comes forward. Controlling another device lifts
//! that, since the Live Activity follows it, and so does a push, which holds
//! the app awake while the link takes what the server kept for it.
//!
//! Being found is passive — the system answers for the advert, and a listener
//! waiting in `accept` costs nothing until someone connects — so a phone
//! playing in the background stays listed and controllable. Paused in the
//! background, it is not there at all.
//!
//! Only an app that says it went to the background is ever quiet; the Mac and
//! the CLI never do.

use parking_lot::{Condvar, Mutex};

struct State {
    background: bool,
    playing: bool,
    holds: u32,
    /// What was last acted on, so each edge acts once.
    applied: Applied,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Applied {
    awake: bool,
    findable: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    background: false,
    playing: false,
    holds: 0,
    applied: Applied {
        awake: true,
        findable: true,
    },
});
static CHANGED: Condvar = Condvar::new();
/// Held while an edge is acted on, so two changes in quick succession act in
/// the order they were made.
static APPLYING: Mutex<()> = Mutex::new(());

fn is_awake(s: &State) -> bool {
    !s.background || s.holds > 0 || crate::remote::devices::target().is_some()
}

fn is_findable(s: &State) -> bool {
    is_awake(s) || s.playing
}

/// Whether work nobody asked for may run now.
pub fn awake() -> bool {
    is_awake(&STATE.lock())
}

/// Whether this device should listen and advertise itself on the network.
pub fn findable() -> bool {
    is_findable(&STATE.lock())
}

/// Block until work nobody asked for may run.
pub fn wait_until_awake() {
    let mut s = STATE.lock();
    while !is_awake(&s) {
        CHANGED.wait(&mut s);
    }
}

/// The app went to the background, or came back.
pub fn set_background(background: bool) {
    update(|s| s.background = background);
}

/// Playback started or stopped, as the app sees it.
pub fn set_playing(playing: bool) {
    update(|s| s.playing = playing);
}

/// Stay awake until `release`: a push, for as long as iOS lets it run.
pub fn hold() {
    update(|s| s.holds += 1);
}

pub fn release() {
    update(|s| s.holds = s.holds.saturating_sub(1));
}

/// Something read outside this module changed: the device controlled.
pub fn reapply() {
    update(|_| ());
}

fn update(f: impl FnOnce(&mut State)) {
    let _applying = APPLYING.lock();
    let (before, now) = {
        let mut s = STATE.lock();
        f(&mut s);
        let now = Applied {
            awake: is_awake(&s),
            findable: is_findable(&s),
        };
        CHANGED.notify_all();
        (std::mem::replace(&mut s.applied, now), now)
    };
    if before.findable != now.findable {
        crate::remote::nearby::reconfigure();
    }
    if before.awake != now.awake {
        if now.awake {
            log::info!("quiet: awake");
            crate::remote::nearby::wake();
            crate::remote::devices::resume();
        } else {
            log::info!("quiet: nothing runs until asked");
            crate::remote::link::hang_up();
            crate::remote::nearby::suspend();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(background: bool, playing: bool, holds: u32) -> State {
        State {
            background,
            playing,
            holds,
            applied: Applied {
                awake: true,
                findable: true,
            },
        }
    }

    #[test]
    fn in_front_everything_runs() {
        let s = state(false, false, 0);
        assert!(is_awake(&s) && is_findable(&s));
    }

    #[test]
    fn playing_in_the_background_is_found_but_runs_nothing() {
        let s = state(true, true, 0);
        assert!(!is_awake(&s));
        assert!(is_findable(&s));
    }

    #[test]
    fn paused_in_the_background_is_not_there() {
        let s = state(true, false, 0);
        assert!(!is_awake(&s) && !is_findable(&s));
    }

    #[test]
    fn a_push_holds_it_awake() {
        let s = state(true, false, 1);
        assert!(is_awake(&s) && is_findable(&s));
    }
}
