//! Recording what got played.
//!
//! History answers "what did I put on, and in what order". A track is written
//! the moment it starts, not once it has been listened to for long enough —
//! putting something on and skipping two seconds in is still a thing you did,
//! and a log with a threshold on it is a log with holes in it.
//!
//! How long it was actually heard for is filled in afterwards, when the needle
//! leaves.
//!
//! The remote server is held to a stricter standard, because its plays feed
//! Last.fm and friends: it is told what is playing when a track starts, and is
//! sent a scrobble only once the track has been heard (see [`counts_as_heard`]).

use std::thread;

use crossbeam_channel::{Sender, TrySendError};

use crate::db::connection::Database;
use crate::db::queries;
use crate::player::state::QueueItemId;

/// A position jump larger than this is a seek, not playback, and buys no
/// credit. The player polls every 50ms, so a real tick is far below it.
const MAX_TICK_MS: u64 = 2_000;

/// Last.fm's floor: a track shorter than this is never scrobbled.
const SCROBBLE_MIN_TRACK_MS: u64 = 30_000;

/// Last.fm's ceiling: four minutes heard counts, however long the track.
const SCROBBLE_ENOUGH_MS: u64 = 4 * 60_000;

/// Events waiting to be written. Bounded because the writer can block on a
/// remote server; a wedged one must not grow this without limit.
const QUEUE_DEPTH: usize = 64;

/// The track under the needle, and how much of it has been heard.
///
/// Time comes from position deltas rather than the wall clock, so a pause
/// contributes nothing and a seek does not credit the stretch it skipped.
#[derive(Debug, Clone)]
pub struct InFlight {
    pub item: QueueItemId,
    track_id: Option<i64>,
    last_position_ms: u64,
    listened_ms: u64,
}

impl InFlight {
    pub fn new(item: QueueItemId, track_id: Option<i64>) -> Self {
        Self {
            item,
            track_id,
            last_position_ms: 0,
            listened_ms: 0,
        }
    }

    /// The library track this is playing, if it is one at all. A file dragged
    /// in from outside the library has nowhere to be recorded.
    pub fn track_id(&self) -> Option<i64> {
        self.track_id
    }

    #[cfg(test)]
    pub fn track_id_for_test(&mut self, track_id: i64) {
        self.track_id = Some(track_id);
    }

    /// Fold a newly observed playback position into the listened total.
    pub fn advance(&mut self, position_ms: u64) {
        let delta = position_ms.saturating_sub(self.last_position_ms);
        if delta <= MAX_TICK_MS {
            self.listened_ms += delta;
        }
        self.last_position_ms = position_ms;
    }

    pub fn listened_ms(&self) -> u64 {
        self.listened_ms
    }
}

/// What the writer thread is told.
#[derive(Debug, Clone, Copy)]
pub enum PlayEvent {
    /// A track started. Written straight away, so history stays in play order
    /// even for tracks that are skipped a moment later.
    Started { track_id: i64 },
    /// The needle left it, having heard this much.
    Finished { track_id: i64, listened_ms: u64 },
}

/// Writes history away from the player thread.
///
/// The player must never wait on a disk write or an HTTP round trip to a
/// remote server, so it hands events over and forgets about them.
pub struct PlayRecorder {
    tx: Sender<PlayEvent>,
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

        let (tx, rx) = crossbeam_channel::bounded::<PlayEvent>(QUEUE_DEPTH);
        thread::Builder::new()
            .name("koan-history".into())
            .spawn(move || {
                let mut writer = Writer::new(db);
                for event in rx {
                    writer.handle(event);
                }
            })
            .map_err(|e| log::warn!("play history disabled — cannot spawn writer: {e}"))
            .ok()?;

        Some(Self { tx })
    }

    pub fn record(&self, event: PlayEvent) {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                log::warn!("play history writer is behind — dropping {event:?}")
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

/// Owns the connection and the one bit of state history needs: which row the
/// track now playing was written to, so its listening time can land on it.
struct Writer {
    db: Database,
    open: Option<(i64, i64)>,
    /// The track now playing and when it started, in ms since the epoch: a
    /// scrobble is dated to when the listen began, not when it ended.
    started: Option<(i64, u64)>,
}

impl Writer {
    fn new(db: Database) -> Self {
        Self {
            db,
            open: None,
            started: None,
        }
    }

    fn handle(&mut self, event: PlayEvent) {
        match event {
            PlayEvent::Started { track_id } => {
                match queries::record_play(&self.db.conn, track_id, None) {
                    Ok(id) => self.open = Some((id, track_id)),
                    Err(e) => {
                        self.open = None;
                        log::warn!("failed to record play of track {track_id}: {e}");
                    }
                }
                self.started = Some((track_id, now_ms()));
                report_to_remote(&self.db, track_id, Report::NowPlaying);
            }
            PlayEvent::Finished {
                track_id,
                listened_ms,
            } => {
                if let Some((started_track, at_ms)) = self.started.take()
                    && started_track == track_id
                {
                    report_to_remote(&self.db, track_id, Report::Heard { listened_ms, at_ms });
                }
                let Some((id, started)) = self.open.take() else {
                    return;
                };
                if started != track_id {
                    return;
                }
                if let Err(e) =
                    queries::set_listened_ms(&self.db.conn, id, track_id, listened_ms as i64)
                {
                    log::warn!("failed to record listening time for track {track_id}: {e}");
                }
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Whether a listen counts as a play, by Last.fm's rule: half the track or
/// four minutes, whichever comes first, and never a track under thirty
/// seconds. With no known duration only the four minutes can be judged.
fn counts_as_heard(listened_ms: u64, duration_ms: Option<u64>) -> bool {
    match duration_ms.filter(|&d| d > 0) {
        Some(d) => d >= SCROBBLE_MIN_TRACK_MS && listened_ms >= (d / 2).min(SCROBBLE_ENOUGH_MS),
        None => listened_ms >= SCROBBLE_ENOUGH_MS,
    }
}

enum Report {
    /// A track started: shows in the server's Now Playing, counts nothing.
    NowPlaying,
    /// A track ended having been heard for `listened_ms`, starting at `at_ms`.
    Heard { listened_ms: u64, at_ms: u64 },
}

/// Tell the remote server about a play, if the track came from one.
///
/// Best-effort: a server that is down, or a track koan only has locally, both
/// mean nothing to send, and neither is worth surfacing.
fn report_to_remote(db: &Database, track_id: i64, report: Report) {
    let Ok(Some(track)) = queries::get_track_row(&db.conn, track_id) else {
        return;
    };
    let Some(remote_id) = track.remote_id else {
        return;
    };
    let at_ms = match report {
        Report::NowPlaying => None,
        Report::Heard { listened_ms, at_ms } => {
            if !counts_as_heard(listened_ms, track.duration_ms.map(|d| d.max(0) as u64)) {
                return;
            }
            Some(at_ms)
        }
    };
    let cfg = crate::config::Config::load().unwrap_or_default();
    let Some(client) = crate::helpers::subsonic_client(&cfg) else {
        return;
    };
    if let Err(e) = client.scrobble(&remote_id, at_ms) {
        log::warn!("failed to report track {track_id} to remote: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flight() -> InFlight {
        InFlight::new(QueueItemId::new(), Some(1))
    }

    #[test]
    fn listening_accumulates_across_ticks() {
        let mut f = flight();
        for tick in 1..=100 {
            f.advance(tick * 50);
        }
        assert_eq!(f.listened_ms(), 5_000);
    }

    #[test]
    fn a_pause_contributes_nothing() {
        let mut f = flight();
        f.advance(1_000);
        for _ in 0..100 {
            f.advance(1_000);
        }
        assert_eq!(f.listened_ms(), 1_000);
    }

    #[test]
    fn seeking_forward_does_not_credit_the_skipped_stretch() {
        let mut f = flight();
        f.advance(1_000);
        f.advance(280_000); // dragged the seek bar to the end
        f.advance(280_050);
        assert_eq!(f.listened_ms(), 1_050);
    }

    #[test]
    fn seeking_backward_does_not_go_negative_or_double_count() {
        let mut f = flight();
        f.advance(100_000);
        f.advance(0); // back to the start
        f.advance(50);
        assert_eq!(f.listened_ms(), 50);
    }

    #[test]
    fn heard_means_half_the_track() {
        assert!(!counts_as_heard(89_999, Some(180_000)));
        assert!(counts_as_heard(90_000, Some(180_000)));
    }

    #[test]
    fn four_minutes_is_enough_for_a_long_track() {
        assert!(counts_as_heard(240_000, Some(20 * 60_000)));
        assert!(!counts_as_heard(239_999, Some(20 * 60_000)));
    }

    #[test]
    fn a_track_under_thirty_seconds_never_counts() {
        assert!(!counts_as_heard(29_000, Some(29_000)));
    }

    #[test]
    fn a_skip_does_not_count() {
        assert!(!counts_as_heard(2_000, Some(200_000)));
    }

    #[test]
    fn unknown_duration_needs_four_minutes() {
        assert!(!counts_as_heard(120_000, None));
        assert!(counts_as_heard(240_000, None));
    }

    #[test]
    fn starting_mid_track_does_not_credit_the_offset() {
        // Session restore resumes at a saved position.
        let mut f = flight();
        f.advance(120_000);
        f.advance(120_050);
        assert_eq!(f.listened_ms(), 50);
    }
}
