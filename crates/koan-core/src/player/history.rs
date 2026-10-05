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
//! Last.fm and similar services: it is told what is playing, and where, as playback
//! starts, pauses, seeks and stops, and is sent a scrobble only once the track
//! has been heard (see [`counts_as_heard`]).

use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, Sender, TrySendError};

use crate::db::connection::Database;
use crate::db::queries;
use crate::player::state::QueueItemId;
use crate::remote::client::PlaybackReportState;

/// Moves whenever the play history does: a play recorded, or plays deleted.
/// Pages derived from history (Recently played, History) reload on it, rather
/// than on a timer.
static VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn version() -> u64 {
    VERSION.load(std::sync::atomic::Ordering::Acquire)
}

/// The history changed: say so to whatever is watching the engine.
pub fn changed() {
    VERSION.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    crate::signal::engine_changed().bump();
}

/// Last.fm's floor: a track shorter than this is never scrobbled.
const SCROBBLE_MIN_TRACK_MS: u64 = 30_000;

/// Last.fm's ceiling: four minutes heard counts, however long the track.
const SCROBBLE_ENOUGH_MS: u64 = 4 * 60_000;

/// Events waiting to be written. Bounded because the writer can block on a
/// remote server; a wedged one must not grow this without limit.
const QUEUE_DEPTH: usize = 64;

/// The server hears at most one `playing` report per this. Dragging the seek
/// bar restarts playback at every step, and only where it lands matters.
const REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// The play under the needle, and how much of it has been heard.
///
/// Time comes from position deltas rather than the wall clock, so a pause
/// contributes nothing and a seek does not credit the stretch it skipped.
#[derive(Debug, Clone)]
pub struct InFlight {
    pub item: QueueItemId,
    /// The timeline boundary this play is, in the session playing it: what
    /// tells a gapless repeat of the item from the pass before it. A session
    /// opened on the same play, by a seek, makes it 0 again.
    pub boundary: usize,
    track_id: Option<i64>,
    last_position_ms: u64,
    listened_ms: u64,
}

impl InFlight {
    /// Entered at `position_ms`, which is where counting starts.
    pub fn new(item: QueueItemId, track_id: Option<i64>, position_ms: u64) -> Self {
        Self {
            item,
            boundary: 0,
            track_id,
            last_position_ms: position_ms,
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

    /// Fold in the playhead having played on to `position_ms`. Only ever
    /// given a position reached by playing from the last one; a seek goes
    /// through `jump`.
    pub fn advance(&mut self, position_ms: u64) {
        self.listened_ms += position_ms.saturating_sub(self.last_position_ms);
        self.last_position_ms = position_ms;
    }

    /// The playhead moved to `position_ms` without playing there.
    pub fn jump(&mut self, position_ms: u64) {
        self.last_position_ms = position_ms;
    }

    pub fn listened_ms(&self) -> u64 {
        self.listened_ms
    }
}

/// Where playback of a track stands, for the remote server's Now Playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackReport {
    pub track_id: i64,
    pub state: PlaybackReportState,
    pub position_ms: u64,
}

/// What the writer thread is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayEvent {
    /// A track started, at `position_ms` when resumed partway. Written straight
    /// away, so history stays in play order even for tracks that are skipped a
    /// moment later.
    Started { track_id: i64, position_ms: u64 },
    /// The track playing was paused, resumed, sought or stopped.
    Playback(PlaybackReport),
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
        let db = match crate::db::pool::shared().get() {
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
                loop {
                    let next = match writer.reports.deadline() {
                        Some(deadline) => rx.recv_deadline(deadline),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match next {
                        Ok(event) => writer.handle(event),
                        Err(RecvTimeoutError::Timeout) => writer.send_due_report(),
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(|e| log::warn!("play history disabled — cannot spawn writer: {e}"))
            .ok()?;

        Some(Self { tx })
    }

    /// A recorder with no writer behind it, and the events it is handed.
    #[cfg(test)]
    pub fn capture() -> (Self, crossbeam_channel::Receiver<PlayEvent>) {
        let (tx, rx) = crossbeam_channel::bounded(QUEUE_DEPTH);
        (Self { tx }, rx)
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

/// Owns the connection, which row the track now playing was written to (so
/// its listening time can land on it), and the playback reports held back.
struct Writer {
    db: crate::db::pool::Handle<'static>,
    open: Option<(i64, i64)>,
    /// The track now playing and when it started, in ms since the epoch: a
    /// scrobble is dated to when the listen began, not when it ended.
    started: Option<(i64, u64)>,
    reports: Coalescer,
}

impl Writer {
    fn new(db: crate::db::pool::Handle<'static>) -> Self {
        Self {
            db,
            open: None,
            started: None,
            reports: Coalescer::default(),
        }
    }

    fn handle(&mut self, event: PlayEvent) {
        match event {
            PlayEvent::Started {
                track_id,
                position_ms,
            } => {
                match queries::record_play(&self.db.conn, queries::LOCAL_USER, track_id, None) {
                    Ok(id) => {
                        self.open = Some((id, track_id));
                        changed();
                    }
                    Err(e) => {
                        self.open = None;
                        log::warn!("failed to record play of track {track_id}: {e}");
                    }
                }
                self.started = Some((track_id, now_ms()));
                self.report(PlaybackReport {
                    track_id,
                    state: PlaybackReportState::Playing,
                    position_ms,
                });
            }
            PlayEvent::Playback(report) => self.report(report),
            PlayEvent::Finished {
                track_id,
                listened_ms,
            } => {
                if let Some((started_track, at_ms)) = self.started.take()
                    && started_track == track_id
                {
                    scrobble_if_heard(&self.db, track_id, listened_ms, at_ms);
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

    fn report(&mut self, report: PlaybackReport) {
        if let Some(report) = self.reports.offer(report, Instant::now()) {
            send_report(&self.db, report);
        }
    }

    fn send_due_report(&mut self) {
        if let Some(report) = self.reports.due(Instant::now()) {
            send_report(&self.db, report);
        }
    }
}

/// Holds back `playing` reports that follow another report too closely,
/// keeping only the newest. `paused` and `stopped` go straight out and
/// supersede whatever was held: they are the state the server is left in.
#[derive(Debug, Default)]
struct Coalescer {
    last_sent: Option<Instant>,
    pending: Option<PlaybackReport>,
}

impl Coalescer {
    /// What to send now, if anything.
    fn offer(&mut self, report: PlaybackReport, now: Instant) -> Option<PlaybackReport> {
        let recent = self
            .last_sent
            .is_some_and(|at| now.duration_since(at) < REPORT_INTERVAL);
        if report.state == PlaybackReportState::Playing && recent {
            self.pending = Some(report);
            return None;
        }
        self.pending = None;
        self.last_sent = Some(now);
        Some(report)
    }

    /// When the held report falls due.
    fn deadline(&self) -> Option<Instant> {
        self.pending?;
        Some(self.last_sent? + REPORT_INTERVAL)
    }

    /// The held report, once it is due.
    fn due(&mut self, now: Instant) -> Option<PlaybackReport> {
        if self.deadline()? > now {
            return None;
        }
        self.last_sent = Some(now);
        self.pending.take()
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

/// A library track's id on the remote server and its duration, if it came
/// from one. A track koan only has locally has nothing to report.
fn remote_track(db: &Database, track_id: i64) -> Option<(String, Option<u64>)> {
    let track = queries::get_track_row(&db.conn, track_id).ok()??;
    let duration_ms = track.duration_ms.map(|d| d.max(0) as u64);
    Some((track.remote_id?, duration_ms))
}

fn remote_client() -> Option<std::sync::Arc<crate::remote::client::SubsonicClient>> {
    let cfg = crate::config::Config::load().unwrap_or_default();
    crate::helpers::subsonic_client(&cfg)
}

/// Best-effort, like everything sent to the server: one that is down is not
/// worth surfacing.
fn send_report(db: &Database, report: PlaybackReport) {
    let Some((remote_id, _)) = remote_track(db, report.track_id) else {
        return;
    };
    let Some(client) = remote_client() else {
        return;
    };
    if let Err(e) = client.report_playback(&remote_id, report.state, report.position_ms) {
        log::warn!(
            "failed to report playback of track {} to remote: {e}",
            report.track_id
        );
    }
}

/// Scrobble a finished listen to the remote server, if it counts as a play.
fn scrobble_if_heard(db: &Database, track_id: i64, listened_ms: u64, at_ms: u64) {
    let Some((remote_id, duration_ms)) = remote_track(db, track_id) else {
        return;
    };
    if !counts_as_heard(listened_ms, duration_ms) {
        return;
    }
    let Some(client) = remote_client() else {
        return;
    };
    if let Err(e) = client.scrobble(&remote_id, at_ms) {
        log::warn!("failed to report track {track_id} to remote: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: PlaybackReportState, position_ms: u64) -> PlaybackReport {
        PlaybackReport {
            track_id: 1,
            state,
            position_ms,
        }
    }

    #[test]
    fn a_seek_bar_drag_sends_where_it_lands() {
        use PlaybackReportState::Playing;
        let mut c = Coalescer::default();
        let t0 = Instant::now();
        assert_eq!(c.offer(report(Playing, 0), t0), Some(report(Playing, 0)));

        for step in 1..=20 {
            let at = t0 + Duration::from_millis(step * 30);
            assert_eq!(c.offer(report(Playing, step * 5_000), at), None);
        }
        assert_eq!(c.deadline(), Some(t0 + REPORT_INTERVAL));
        assert_eq!(c.due(t0 + Duration::from_millis(900)), None, "not yet");
        assert_eq!(
            c.due(t0 + REPORT_INTERVAL),
            Some(report(Playing, 100_000)),
            "only the last step"
        );
        assert_eq!(c.deadline(), None);
    }

    #[test]
    fn playing_after_a_quiet_second_goes_straight_out() {
        use PlaybackReportState::Playing;
        let mut c = Coalescer::default();
        let t0 = Instant::now();
        c.offer(report(Playing, 0), t0);
        assert_eq!(
            c.offer(report(Playing, 60_000), t0 + REPORT_INTERVAL),
            Some(report(Playing, 60_000))
        );
    }

    #[test]
    fn a_pause_or_stop_is_never_held_and_supersedes_a_held_seek() {
        use PlaybackReportState::{Paused, Playing, Stopped};
        let mut c = Coalescer::default();
        let t0 = Instant::now();
        c.offer(report(Playing, 0), t0);
        assert_eq!(c.offer(report(Playing, 30_000), t0), None);
        assert_eq!(
            c.offer(report(Paused, 30_000), t0),
            Some(report(Paused, 30_000))
        );
        assert_eq!(c.deadline(), None, "the held seek is gone");
        assert_eq!(
            c.offer(report(Stopped, 30_000), t0),
            Some(report(Stopped, 30_000))
        );
    }

    fn flight() -> InFlight {
        InFlight::new(QueueItemId::new(), Some(1), 0)
    }

    #[test]
    fn listening_accumulates_across_readings() {
        let mut f = flight();
        for reading in 1..=100 {
            f.advance(reading * 50);
        }
        assert_eq!(f.listened_ms(), 5_000);
    }

    #[test]
    fn an_hour_between_readings_is_an_hour_heard() {
        let mut f = flight();
        f.advance(3_600_000);
        assert_eq!(f.listened_ms(), 3_600_000);
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
        f.jump(280_000); // dragged the seek bar to the end
        f.advance(280_050);
        assert_eq!(f.listened_ms(), 1_050);
    }

    #[test]
    fn seeking_backward_does_not_go_negative_or_double_count() {
        let mut f = flight();
        f.advance(100_000);
        f.jump(0); // back to the start
        f.advance(50);
        assert_eq!(f.listened_ms(), 100_050);
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
        let mut f = InFlight::new(QueueItemId::new(), Some(1), 120_000);
        f.advance(120_050);
        assert_eq!(f.listened_ms(), 50);
    }
}
