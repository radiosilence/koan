//! Recording what actually got played.
//!
//! The player knows a track started; it does not know a track was *listened
//! to*. A queue skipped through end to end is not twenty plays. So time under
//! the needle is accumulated as playback goes, and only banked at the point the
//! needle leaves the track.

use std::thread;

use crossbeam_channel::{Sender, TrySendError};

use crate::db::connection::Database;
use crate::db::queries;
use crate::player::state::QueueItemId;

/// Past this much, a long track counts however much of it is left.
const LONG_PLAY_MS: u64 = 4 * 60 * 1000;

/// How much of a track of unknown length has to be heard.
const UNKNOWN_LENGTH_MS: u64 = 30_000;

/// A position jump larger than this is a seek, not playback, and buys no
/// credit. The player polls every 50ms, so a real tick is far below it.
const MAX_TICK_MS: u64 = 2_000;

/// Plays waiting to be written. Bounded because the writer can block on a
/// remote server; a wedged one must not grow this without limit.
const QUEUE_DEPTH: usize = 64;

/// Whether time spent on a track counts as having played it.
///
/// The scrobbling convention: half the track, or four minutes, whichever comes
/// first. `duration_ms` of 0 means the length was never established — there is
/// no half to take, so it falls back to a flat thirty seconds.
pub fn counts_as_play(listened_ms: u64, duration_ms: u64) -> bool {
    if duration_ms == 0 {
        return listened_ms >= UNKNOWN_LENGTH_MS;
    }
    listened_ms >= (duration_ms / 2).min(LONG_PLAY_MS)
}

/// The track under the needle, and how much of it has been heard.
///
/// Time comes from position deltas rather than the wall clock, so a pause
/// contributes nothing and a seek does not credit the stretch it skipped.
#[derive(Debug, Clone)]
pub struct InFlight {
    pub item: QueueItemId,
    track_id: Option<i64>,
    duration_ms: u64,
    last_position_ms: u64,
    listened_ms: u64,
}

impl InFlight {
    pub fn new(item: QueueItemId, track_id: Option<i64>, duration_ms: u64) -> Self {
        Self {
            item,
            track_id,
            duration_ms,
            last_position_ms: 0,
            listened_ms: 0,
        }
    }

    /// Fold a newly observed playback position into the listened total.
    pub fn advance(&mut self, position_ms: u64) {
        let delta = position_ms.saturating_sub(self.last_position_ms);
        if delta <= MAX_TICK_MS {
            self.listened_ms += delta;
        }
        self.last_position_ms = position_ms;
    }

    /// Correct the track length. A streaming probe runs on a partial file and
    /// can come back short, which would set the threshold too low.
    pub fn set_duration_ms(&mut self, duration_ms: u64) {
        self.duration_ms = duration_ms;
    }

    #[cfg(test)]
    pub fn track_id_for_test(&mut self, track_id: i64) {
        self.track_id = Some(track_id);
    }

    pub fn listened_ms(&self) -> u64 {
        self.listened_ms
    }

    /// The play to record, or None if this was not listened to for long enough
    /// (or is not a library track, and so has nowhere to be recorded).
    pub fn into_play(self) -> Option<Play> {
        let track_id = self.track_id?;
        counts_as_play(self.listened_ms, self.duration_ms).then_some(Play {
            track_id,
            listened_ms: self.listened_ms,
        })
    }
}

/// A play that happened, on its way to the database.
#[derive(Debug, Clone, Copy)]
pub struct Play {
    pub track_id: i64,
    pub listened_ms: u64,
}

/// Writes plays away from the player thread.
///
/// The player must never wait on a disk write or an HTTP round trip to a
/// remote server, so it hands plays over and forgets about them.
pub struct PlayRecorder {
    tx: Sender<Play>,
}

impl PlayRecorder {
    /// Start the writer thread. `None` if the database cannot be opened, which
    /// costs history but must not stop playback.
    pub fn spawn() -> Option<Self> {
        let db = match Database::open_default() {
            Ok(db) => db,
            Err(e) => {
                log::warn!("play history disabled — cannot open database: {e}");
                return None;
            }
        };

        let (tx, rx) = crossbeam_channel::bounded::<Play>(QUEUE_DEPTH);
        thread::Builder::new()
            .name("koan-history".into())
            .spawn(move || {
                for play in rx {
                    write_play(&db, play);
                }
            })
            .map_err(|e| log::warn!("play history disabled — cannot spawn writer: {e}"))
            .ok()?;

        Some(Self { tx })
    }

    pub fn record(&self, play: Play) {
        match self.tx.try_send(play) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                log::warn!("play history writer is behind — dropping a play")
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

fn write_play(db: &Database, play: Play) {
    if let Err(e) = queries::record_play(&db.conn, play.track_id, Some(play.listened_ms as i64)) {
        log::warn!("failed to record play of track {}: {e}", play.track_id);
        return;
    }
    scrobble_to_remote(db, play.track_id);
}

/// Tell the remote server about a play, if the track came from one.
///
/// Best-effort: a server that is down or a track koan only has locally both
/// mean nothing to send, and neither is worth surfacing.
fn scrobble_to_remote(db: &Database, track_id: i64) {
    let Ok(Some(track)) = queries::get_track_row(&db.conn, track_id) else {
        return;
    };
    let Some(remote_id) = track.remote_id else {
        return;
    };
    let cfg = crate::config::Config::load().unwrap_or_default();
    let Some(client) = crate::helpers::subsonic_client(&cfg) else {
        return;
    };
    if let Err(e) = client.scrobble(&remote_id) {
        log::warn!("failed to scrobble track {track_id} to remote: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> QueueItemId {
        QueueItemId::new()
    }

    #[test]
    fn half_a_track_counts() {
        assert!(!counts_as_play(149_000, 300_000));
        assert!(counts_as_play(150_000, 300_000));
    }

    #[test]
    fn four_minutes_counts_however_long_the_track_is() {
        // An hour-long mix should not need half an hour to register.
        let hour = 60 * 60 * 1000;
        assert!(!counts_as_play(LONG_PLAY_MS - 1, hour));
        assert!(counts_as_play(LONG_PLAY_MS, hour));
    }

    #[test]
    fn a_track_of_unknown_length_falls_back_to_thirty_seconds() {
        assert!(!counts_as_play(29_999, 0));
        assert!(counts_as_play(30_000, 0));
    }

    #[test]
    fn listening_accumulates_across_ticks() {
        let mut f = InFlight::new(item(), Some(1), 300_000);
        for tick in 1..=100 {
            f.advance(tick * 50);
        }
        assert_eq!(f.listened_ms(), 5_000);
    }

    #[test]
    fn a_pause_contributes_nothing() {
        let mut f = InFlight::new(item(), Some(1), 300_000);
        f.advance(1_000);
        for _ in 0..100 {
            f.advance(1_000);
        }
        assert_eq!(f.listened_ms(), 1_000);
    }

    #[test]
    fn seeking_forward_does_not_credit_the_skipped_stretch() {
        let mut f = InFlight::new(item(), Some(1), 300_000);
        f.advance(1_000);
        f.advance(280_000); // dragged the seek bar to the end
        f.advance(280_050);
        assert_eq!(f.listened_ms(), 1_050);
        assert!(
            !f.into_play().is_some(),
            "scrubbing to the end is not a play"
        );
    }

    #[test]
    fn seeking_backward_does_not_go_negative_or_double_count() {
        let mut f = InFlight::new(item(), Some(1), 300_000);
        f.advance(100_000);
        f.advance(0); // back to the start
        f.advance(50);
        assert_eq!(f.listened_ms(), 50);
    }

    #[test]
    fn starting_mid_track_does_not_credit_the_offset() {
        // Session restore resumes at a saved position.
        let mut f = InFlight::new(item(), Some(1), 300_000);
        f.advance(120_000);
        f.advance(120_050);
        assert_eq!(f.listened_ms(), 50);
    }

    #[test]
    fn a_track_that_is_not_in_the_library_records_nothing() {
        let mut f = InFlight::new(item(), None, 300_000);
        f.advance(200_000);
        assert!(f.into_play().is_none());
    }

    #[test]
    fn a_finished_track_banks_the_time_it_was_heard_for() {
        let mut f = InFlight::new(item(), Some(7), 300_000);
        let mut at = 0;
        while at < 300_000 {
            at += 500;
            f.advance(at);
        }
        let play = f.into_play().expect("a track played end to end is a play");
        assert_eq!(play.track_id, 7);
        assert_eq!(play.listened_ms, 300_000);
    }
}
