//! What an interruption of the iOS audio session has left to do.
//!
//! The unit iOS stopped does not start again on its own, so every way an
//! interruption can finish has the output rebuilt: an "ended", a route that
//! went away and settled (iOS sends no "ended" for it, as when a voice
//! assistant hands the speaker back), or the app coming back to the front
//! with nothing having ended it.

use std::time::{Duration, Instant};

/// How long a route-disconnect interruption is left to settle.
pub const SETTLE: Duration = Duration::from_secs(1);

/// How recently the route must have gone for an interruption that came with
/// it to stay paused: headphones pulled out do not play on.
const ROUTE_LOST: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Interruption {
    /// One began and nothing has finished it.
    interrupted: bool,
    /// Playing here when it began: what its end resumes. iOS can send several
    /// "began" for one, and the later find the music already paused.
    was_playing: bool,
    route_lost_at: Option<Instant>,
}

impl Interruption {
    pub fn began(&mut self, playing_here: bool) {
        self.interrupted = true;
        self.was_playing |= playing_here;
    }

    pub fn route_lost(&mut self, now: Instant) {
        self.route_lost_at = Some(now);
    }

    /// iOS ended it; whether to resume, if the music is still here.
    pub fn ended(&mut self, should_resume: bool) -> bool {
        self.finish(should_resume)
    }

    /// The wait after a route-disconnect is over. `Some` when the output is
    /// to be rebuilt, with whether to resume; `None` when something else
    /// already finished the interruption.
    pub fn settled(&mut self, now: Instant) -> Option<bool> {
        let lost = self
            .route_lost_at
            .is_some_and(|at| now.duration_since(at) < ROUTE_LOST);
        self.interrupted.then(|| self.finish(!lost))
    }

    /// The app is in front again. `Some` when an interruption nothing ended
    /// left the output stopped; resuming is the person's to do.
    pub fn returned(&mut self) -> Option<bool> {
        self.interrupted.then(|| self.finish(false))
    }

    /// iOS reset its media services, which ends nothing but what would have
    /// been resumed.
    pub fn reset(&mut self) {
        self.was_playing = false;
    }

    fn finish(&mut self, resume: bool) -> bool {
        self.interrupted = false;
        std::mem::take(&mut self.was_playing) && resume
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_disconnect_with_no_ended_rebuilds_and_resumes() {
        let (mut i, now) = (Interruption::default(), Instant::now());
        i.began(true);
        assert_eq!(i.settled(now + SETTLE), Some(true));
    }

    #[test]
    fn events_arriving_while_it_settles_do_not_cancel_it() {
        // The route change that comes with a disconnect: heard, and the wait
        // goes on to its end regardless.
        let (mut i, now) = (Interruption::default(), Instant::now());
        i.began(true);
        i.began(false);
        assert_eq!(i.settled(now + SETTLE), Some(true), "still resumes");
    }

    #[test]
    fn headphones_pulled_out_stay_paused() {
        let (mut i, now) = (Interruption::default(), Instant::now());
        i.began(true);
        i.route_lost(now);
        assert_eq!(i.settled(now + SETTLE), Some(false));
    }

    #[test]
    fn a_route_lost_long_ago_does_not_hold_it_paused() {
        let (mut i, now) = (Interruption::default(), Instant::now());
        i.route_lost(now);
        i.began(true);
        assert_eq!(i.settled(now + ROUTE_LOST), Some(true));
    }

    #[test]
    fn an_ended_finishes_it_before_the_wait_does() {
        let (mut i, now) = (Interruption::default(), Instant::now());
        i.began(true);
        assert!(i.ended(true));
        assert_eq!(i.settled(now + SETTLE), None);
    }

    #[test]
    fn coming_back_rebuilds_only_what_nothing_ended_and_never_resumes() {
        let mut i = Interruption::default();
        assert_eq!(i.returned(), None, "nothing to recover");
        i.began(true);
        assert_eq!(i.returned(), Some(false));
        assert_eq!(i.returned(), None, "once");
    }

    #[test]
    fn nothing_playing_when_it_began_resumes_nothing() {
        let mut i = Interruption::default();
        i.began(false);
        assert!(!i.ended(true));
    }

    #[test]
    fn a_reset_forgets_what_would_have_resumed() {
        let mut i = Interruption::default();
        i.began(true);
        i.reset();
        assert!(!i.ended(true));
    }
}
