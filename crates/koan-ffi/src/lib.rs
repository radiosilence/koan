//! In-process bindings for native GUI clients.
//!
//! This is the same facade `koan-server` puts behind GraphQL, minus the wire.
//! Every method is "read `SharedPlayerState` / hit the DB / send a
//! `PlayerCommand`" — the mapping koan-core's helpers already own. A local app
//! sits on top of the audio engine, so it has no business round-tripping HTTP
//! to reach it; GraphQL stays the surface for clients that cannot link the
//! core (the web UI, jukebox remotes).
//!
//! Threading: anything that can block is `async` and runs on a worker thread,
//! so no caller ever holds a thread while koan-core reads a file or waits on a
//! socket. The few methods that stay synchronous read an atomic or two, or
//! hold an in-memory lock for one short pass. See `offload` for where the work goes, and why ordering has a lane of
//! its own.
//!
//! DB connections are borrowed from `koan_core::db::pool`, not opened per call:
//! opening one runs the schema DDL and a WAL checkpoint.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Instant;

use crossbeam_channel::Sender;
use koan_core::audio::viz::VizSnapshot;
use koan_core::config::{self, Config};
use koan_core::db::connection::Database;
use koan_core::db::queries::{self, PersistedQueueItem};
use koan_core::player::Player;
use koan_core::player::commands::PlayerCommand;
use koan_core::player::state::{PlaybackState, PlaylistItem, QueueItemId, SharedPlayerState};
use koan_core::remote::client::SubsonicError;
use uuid::Uuid;

use koan_core::db::queries::RECENT_LIMIT;

mod offload;
mod queue_slice;
mod server_queue;
mod state;
mod types;
pub use state::*;
pub use types::*;

uniffi::setup_scaffolding!();

/// What `fuzzy_search` matches against.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    Track,
    Album,
    Artist,
}

#[derive(uniffi::Record, Debug, Clone)]
pub struct FuzzyMatch {
    pub id: i64,
    /// Pre-joined display text — the same string that was matched against.
    pub name: String,
    pub kind: SearchKind,
}

/// The running sync's progress, written by the sync and read by the watcher.
#[derive(Clone, Default)]
struct SyncMeter(Arc<parking_lot::Mutex<Option<SyncProgress>>>);

impl SyncMeter {
    fn set(&self, p: koan_core::remote::sync::SyncProgress) {
        *self.0.lock() = Some(p.into());
        koan_core::signal::engine_changed().bump();
    }

    fn clear(&self) {
        *self.0.lock() = None;
        koan_core::signal::engine_changed().bump();
    }

    fn get(&self) -> Option<SyncProgress> {
        *self.0.lock()
    }
}

/// Reports how far a long task has got.
///
/// Scans and syncs take anywhere up to a minute, and a spinner that cannot say
/// how far through it is tells the user only that the app has not crashed.
///
/// `advanced` is called from a worker thread, often — implementations must be
/// cheap and must not block. koan throttles the calls so a fifty-thousand-file
/// scan does not cross the FFI fifty thousand times.
#[uniffi::export(with_foreign)]
pub trait ProgressReporter: Send + Sync {
    /// How many items there are, once known. Zero means unknowable.
    fn started(&self, total: u64);
    /// How many are done, and what is being worked on now.
    fn advanced(&self, done: u64, detail: String);
}

/// Send `log` output to `~/.config/koan/koan.log`, the same file the CLI
/// writes.
///
/// Without this every `log::warn!` in koan-core is discarded when the engine is
/// hosted by a GUI, so a favourite that failed to reach the server, or a track
/// that would not decode, leaves nothing behind to look at.
fn init_logging() {
    use std::sync::Mutex;

    struct FileLogger(Mutex<config::LogFile>);

    impl log::Log for FileLogger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Info
        }

        fn log(&self, record: &log::Record) {
            if !self.enabled(record.metadata()) {
                return;
            }
            let Ok(mut file) = self.0.lock() else { return };
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
            file.write(format_args!(
                "[{}] {}: {}",
                now,
                record.level().as_str().to_lowercase(),
                record.args()
            ));
        }

        fn flush(&self) {
            if let Ok(mut file) = self.0.lock() {
                file.flush();
            }
        }
    }

    static LOGGER: std::sync::OnceLock<FileLogger> = std::sync::OnceLock::new();

    let logger = LOGGER.get_or_init(|| FileLogger(Mutex::new(config::LogFile::default())));
    // A second engine in one process is not an error worth failing over.
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

impl Drop for KoanEngine {
    /// Wake the state watcher so it notices there is nothing left to watch.
    ///
    /// It waits with no timeout, holding a weak reference precisely so it does
    /// not keep the engine alive — and a thread parked for ever would never
    /// find out that it had gone.
    fn drop(&mut self) {
        koan_core::signal::engine_changed().bump();
    }
}

/// One client's subscription to the levels of whatever it is showing as
/// playing: this device's analyser, or, while another device is controlled,
/// that device's levels over the link (`koan_core::remote::levels`).
///
/// A cursor over the published frame, in the shape `StateStream` already uses:
/// the value is a whole snapshot, so a subscriber that slept through two
/// publishes wants the newest frame rather than the two it missed.
#[derive(uniffi::Object)]
pub struct VizStream {
    /// Weak, so a client's loop ends when the engine goes rather than holding
    /// the analyser up. The loop *is* the subscription.
    viz: Weak<VizSnapshot>,
    inner: tokio::sync::Mutex<Following>,
}

struct Following {
    local: tokio::sync::watch::Receiver<u64>,
    /// Display frames while the controlled device's bars move.
    remote: tokio::sync::watch::Receiver<u64>,
    /// Anything about the engine, the choice of device among it.
    devices: tokio::sync::watch::Receiver<u64>,
    /// Held while another device is controlled: watching it is what has it
    /// send its levels, and dropping this stops them.
    view: Option<koan_core::remote::levels::View>,
}

impl VizStream {
    fn new(viz: &Arc<VizSnapshot>) -> Arc<Self> {
        // Counted as a reader from here rather than at the first frame: an
        // analyser parked for want of one would otherwise never publish the
        // frame this is about to wait for.
        viz.touch();
        Arc::new(Self {
            viz: Arc::downgrade(viz),
            inner: tokio::sync::Mutex::new(Following {
                local: viz.subscribe(),
                remote: koan_core::remote::levels::remote().ticks().subscribe(),
                devices: koan_core::signal::engine_changed().subscribe(),
                view: None,
            }),
        })
    }
}

#[uniffi::export]
impl VizStream {
    /// The next frame, as three band energies. Waits until there is one.
    ///
    /// `None` once the engine is gone, which ends the caller's loop.
    pub async fn next(&self) -> Option<VizLevels> {
        use koan_core::remote::{devices, levels};
        let mut f = self.inner.lock().await;
        loop {
            let target = devices::target();
            if f.view.as_ref().map(|v| v.target()) != target.as_deref() {
                // The old device let go before the new one is watched, so it
                // is told to stop.
                f.view = None;
                f.view = target.map(|t| levels::remote().view(t));
            }
            // Marked seen *before* the wait, so a frame published between the
            // last answer and this call is returned rather than slept through.
            f.devices.borrow_and_update();
            if f.view.is_some() {
                f.remote.borrow_and_update();
                let Following {
                    remote, devices, ..
                } = &mut *f;
                tokio::select! {
                    tick = remote.changed() => tick.ok()?,
                    moved = devices.changed() => {
                        moved.ok()?;
                        // A link or a connection on the network may have come
                        // up, or the device relinked: ask again where due.
                        levels::remote().ask(false);
                        continue;
                    }
                }
                let (position, playing) = devices::target_playhead().unwrap_or((0, false));
                return Some(levels::remote().sample(position, playing).into());
            }
            let viz = self.viz.upgrade()?;
            viz.touch();
            drop(viz);
            f.local.borrow_and_update();
            let Following { local, devices, .. } = &mut *f;
            tokio::select! {
                frame = local.changed() => {
                    frame.ok()?;
                    return Some(self.viz.upgrade()?.levels().into());
                }
                moved = devices.changed() => moved.ok()?,
            }
        }
    }
}

#[derive(uniffi::Object)]
pub struct KoanEngine {
    state: Arc<SharedPlayerState>,
    tx: Sender<PlayerCommand>,
    /// The analyser's latest frame. Only the three-band summary crosses the
    /// boundary — see `VizStream`.
    viz: Arc<VizSnapshot>,
    /// What the engine publishes and clients read. See `state`.
    out: Arc<state::EngineState>,
    /// Set while the automatic sync is running, so a UI can say so rather than
    /// appearing to do nothing for the minute it takes.
    auto_syncing: Arc<std::sync::atomic::AtomicBool>,
    /// Set while the startup or watched-folder scan is running.
    auto_scanning: Arc<std::sync::atomic::AtomicBool>,
    /// How far the running sync has got, automatic or asked for. Published as
    /// the `Sync` slice.
    sync_progress: SyncMeter,
    /// Raised to stop whichever library task is running. One flag rather than
    /// one per task, because only one runs at a time — they all contend for the
    /// same single database writer.
    cancel_library_task: Arc<std::sync::atomic::AtomicBool>,
    /// Bumped by anything that writes library rows. The watcher turns a change
    /// here into a `Library` slice, so a background scan finishing looks the
    /// same to a client as one it asked for itself.
    library_version: Arc<std::sync::atomic::AtomicU64>,
    /// The queue's `content_version` as last saved, so a save rewrites the
    /// queue only when its contents moved and otherwise saves the position.
    saved_content: std::sync::atomic::AtomicU64,
    /// What the fuzzy searches match against, kept until `library_version`
    /// moves.
    fuzzy: queries::CorpusCache,
    /// Each playlist's earlier states, for undo. The queue's own history is
    /// the player's.
    playlist_history: koan_core::playlists::PlaylistHistory,
    /// The pairing this device is waiting on, until `await_pairing` takes it.
    pairing: parking_lot::Mutex<Option<koan_core::remote::pair::Pending>>,
    /// What ends the wait, by the pairing's id.
    pairing_cancel: parking_lot::Mutex<Option<(String, koan_core::remote::pair::Cancel)>>,
    /// What the app calls this device — the name the person gave it, where
    /// the platform says — for the link and for a pairing to ask under.
    device_name: Option<String>,
}

/// How far a client's own reckoning of the playhead may drift before it is
/// told again. Two frames of a 60Hz seek bar: below this there is nothing on
/// screen to correct.
const PLAYHEAD_TOLERANCE_MS: u64 = 32;

/// The same, for how far a download reaches. The bar it draws is one bar wide
/// and a fifth of a second of audio does not move it.
const SEEKABLE_TOLERANCE_MS: u64 = 200;

/// How long a run of downloads landing is allowed to go before the library
/// says so again. A record is fetched a track at a time a few seconds apart;
/// one reload per track is a reload per few seconds for the length of it.
const LANDING_COALESCE: std::time::Duration = std::time::Duration::from_secs(2);

/// Music sent to another device, not yet known to have arrived.
struct HandedOff {
    left_out: u32,
    /// Where it went; `None` for this device.
    to: Option<String>,
    /// The server's id for the track it should now be on, when known.
    track: Option<String>,
    /// What `to` last reported before the music was sent.
    before: Option<koan_core::remote::link::LinkState>,
    /// For music moved here: the queue entry under this device's cursor
    /// before it was sent.
    cursor_before: Option<QueueItemId>,
    /// The answer to the command that moved it, when it went by a way that
    /// brings one: `None` through the channel when it did not.
    answer: Option<crossbeam_channel::Receiver<Option<koan_core::remote::acks::AckOutcome>>>,
    /// `answer` is the destination's own, to the Play it was sent: done means
    /// the music is there. Otherwise it is from the device asked to send it
    /// on, and only says whether that device did.
    answer_is_arrival: bool,
    /// The source was playing, and plays on if the music was refused.
    was_playing: bool,
}

/// A channel for a command's outcome, and what `send_then` hands it to.
fn outcome_channel() -> (
    koan_core::remote::devices::Then,
    crossbeam_channel::Receiver<Option<koan_core::remote::acks::AckOutcome>>,
) {
    let (tx, rx) = crossbeam_channel::bounded(1);
    (
        Box::new(move |outcome| {
            let _ = tx.send(outcome);
        }),
        rx,
    )
}

impl HandedOff {
    /// Wait for the destination to say it has the music. Sent is not taken:
    /// until it does, the source stays paused with its queue, and the caller
    /// is told the music has not started rather than that it has.
    /// `here` is the entry under this device's cursor and the server's id for
    /// its track, for music moved to this device.
    /// `resume` plays on here when the music was refused.
    fn result(
        &self,
        here: impl Fn() -> Option<(QueueItemId, Option<String>)>,
        resume: impl Fn(),
    ) -> MoveResult {
        use koan_core::remote::acks::AckOutcome;
        use koan_core::remote::devices;
        let not_started = |queued, error| MoveResult {
            left_out: self.left_out,
            started: false,
            queued,
            error,
        };
        if let Some(answer) = &self.answer {
            match answer.recv_timeout(HAND_OFF_ANSWER) {
                Ok(Some(AckOutcome::Done)) if self.answer_is_arrival => {
                    return MoveResult {
                        left_out: self.left_out,
                        started: true,
                        queued: false,
                        error: None,
                    };
                }
                Ok(Some(AckOutcome::Queued)) => return not_started(true, None),
                Ok(Some(
                    AckOutcome::Refused { reason: why } | AckOutcome::Failed { error: why },
                )) => {
                    log::warn!("devices: {:?} did not take the music: {why}", self.to);
                    if self.was_playing {
                        resume();
                    }
                    return not_started(false, Some(why));
                }
                // Asked to send it on, and did: whether it arrived is the
                // destination's to say, below.
                Ok(Some(AckOutcome::Done)) => {}
                // Sent by a way that brings no answer: watch for it instead.
                Ok(None) => {}
                Err(_) if self.answer_is_arrival => return not_started(false, None),
                Err(_) => {}
            }
        }
        let started = self.track.as_deref().is_some_and(|track| match &self.to {
            Some(to) => devices::await_current(to, track, self.before.as_ref(), HAND_OFF_TAKEN),
            None => devices::await_until(
                || arrived_here(self.cursor_before, here(), track),
                HAND_OFF_TAKEN,
            ),
        });
        if !started {
            log::warn!(
                "devices: {:?} has not said it has the music sent to it",
                self.to
            );
        }
        MoveResult {
            left_out: self.left_out,
            started,
            queued: false,
            error: None,
        }
    }
}

/// Whether music moved here has arrived: the cursor is on `track` in an entry
/// that was not under it before. The queue it arrives in is new, so its
/// entries are; a cursor already on that track, in the queue this device kept
/// when it last handed the music on, is not it arriving.
fn arrived_here(
    before: Option<QueueItemId>,
    now: Option<(QueueItemId, Option<String>)>,
    track: &str,
) -> bool {
    now.is_some_and(|(entry, remote)| Some(entry) != before && remote.as_deref() == Some(track))
}

/// How long moving the music waits for the answer to the command that moved
/// it: long enough for every way through (the network, the link, then the
/// server's own wait), so a late answer is still the one acted on.
const HAND_OFF_ANSWER: std::time::Duration = std::time::Duration::from_secs(12);

/// How long moving the music waits for the destination to say it has it.
/// Long enough for a device that needs to sync a track first; a device asleep
/// takes longer, and is reported as not started yet.
const HAND_OFF_TAKEN: std::time::Duration = std::time::Duration::from_secs(8);

#[uniffi::export]
impl KoanEngine {
    /// Spawns the player thread and opens the library. One per process.
    /// `device_name` is what this device calls itself to the account's other
    /// devices; without one, the hostname.
    #[uniffi::constructor]
    pub async fn new(device_name: Option<String>) -> Result<Arc<Self>, KoanError> {
        offload::offload(move || Self::build(device_name)).await
    }
    // --- Transport ---------------------------------------------------------

    /// Move the cursor to `queue_item_id` and start playing it.
    pub async fn play(self: Arc<Self>, queue_item_id: String) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Play(parse_qid(&queue_item_id)?))).await
    }

    /// The app is in front again, perhaps after iOS suspended it: link now,
    /// prove every connection is alive and look at the network afresh, so
    /// the other devices are there the moment it opens. See
    /// `koan_core::remote::devices::resume`.
    pub fn link_nudge(&self) {
        koan_core::remote::devices::resume();
    }

    /// The app went to the background, or came back. In the background
    /// nothing runs that nobody asked for — see `koan_core::quiet`.
    pub fn set_background(&self, background: bool) {
        koan_core::quiet::set_background(background);
    }

    /// Whether the app is playing: a phone playing in the background stays
    /// findable on the network, one paused in the background does not.
    pub fn set_playing(&self, playing: bool) {
        koan_core::quiet::set_playing(playing);
    }

    /// Stay awake in the background until `release_awake`: for a push, while
    /// the link takes what the server kept.
    pub fn hold_awake(&self) {
        koan_core::quiet::hold();
    }

    pub fn release_awake(&self) {
        koan_core::quiet::release();
    }

    /// Write to koan's log, beside the engine's own lines: for events only the
    /// app sees, such as an audio interruption, where the order relative to
    /// what the engine did is the whole point.
    pub fn log_note(&self, message: String) {
        log::info!("app: {message}");
    }

    /// The accounts on this server that may control this device.
    pub fn device_shares(&self) -> Vec<String> {
        koan_core::remote::devices::shares()
    }

    /// Let the account `grantee` control this device, or with `allow` false
    /// stop letting it: playback and the queue, nothing of this account. The
    /// list in `device_shares` follows once the server has it.
    pub fn share_device(&self, grantee: String, allow: bool) -> Result<(), KoanError> {
        koan_core::remote::devices::share(&grantee, allow)
            .map_err(|message| KoanError::Remote { message })
    }

    /// Where Apple's push service reaches this app, as the OS issued it. The
    /// link sends it to the server, which can then wake the app once iOS has
    /// suspended it. `sandbox` for a development build.
    pub fn set_push_token(&self, token: String, sandbox: bool) {
        koan_core::remote::link::set_push_token(token, sandbox);
    }

    /// Run a command a push notification carried (the JSON under `koan`): what
    /// the server would have sent over the link had iOS not suspended the app.
    /// Also links now, so anything else waiting follows.
    ///
    /// Tracks the library lacks are synced for on the pool first. The lane
    /// every transport call waits on only resolves what is already here.
    pub async fn run_pushed_command(self: Arc<Self>, command: String) -> Result<(), KoanError> {
        koan_core::remote::link::nudge();
        let envelope = match koan_core::remote::acks::Envelope::parse(&command) {
            Ok(envelope) => envelope,
            Err(e) => {
                log::warn!("push: not a command ({e}): {command}");
                return Ok(());
            }
        };
        // Under the id it was sent with: a copy that also came down the link
        // is acted on once, and the answer goes up the link when there is one.
        let Some((cmd, pending)) = koan_core::remote::acks::take(envelope, |ack, outcome| {
            koan_core::remote::link::report(koan_core::remote::link::LinkReport::Ack {
                ack,
                outcome,
            });
        }) else {
            return Ok(());
        };
        let ids = cmd.track_ids().to_vec();
        if !ids.is_empty() {
            let engine = self.clone();
            offload::offload(move || {
                let db = engine.db()?;
                if koan_core::remote::link::resolve_tracks(&db, &ids, true).1 {
                    engine.library_changed();
                }
                Ok::<_, KoanError>(())
            })
            .await?;
        }
        // A sync touches no player state, so it has no place in the lane's order.
        if matches!(
            cmd,
            koan_core::remote::link::LinkCommand::Sync { .. }
                | koan_core::remote::link::LinkCommand::HistoryChanged
                | koan_core::remote::link::LinkCommand::DspProfilesChanged
        ) {
            return offload::offload(move || {
                self.handle_link(
                    cmd,
                    koan_core::remote::link::CommandSource::Account,
                    pending,
                );
                Ok(())
            })
            .await;
        }
        offload::sequenced(move || {
            self.handle_link(
                cmd,
                koan_core::remote::link::CommandSource::Account,
                pending,
            );
            Ok(())
        })
        .await
    }

    pub async fn pause(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Pause)).await
    }

    pub async fn resume(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Resume)).await
    }

    pub async fn stop(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Stop)).await
    }

    /// Space-bar behaviour: pause when playing, resume otherwise.
    pub async fn toggle_play_pause(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || {
            let playing = match koan_core::remote::devices::target_device() {
                Some(d) => d.state.is_some_and(|s| s.playing),
                None => self.state.wants_to_play(),
            };
            if playing {
                self.send(PlayerCommand::Pause)
            } else {
                self.send(PlayerCommand::Resume)
            }
        })
        .await
    }

    pub async fn next(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::NextTrack)).await
    }

    pub async fn previous(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::PrevTrack)).await
    }

    /// Shuffle on or off, here or on the device being controlled. On, the
    /// rest of the queue is reordered at random; off, it goes back as it was.
    pub async fn set_shuffle(self: Arc<Self>, on: bool) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::SetShuffle(on))).await
    }

    /// What follows a track at its end, here or on the device being
    /// controlled.
    pub async fn set_repeat(self: Arc<Self>, mode: RepeatMode) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::SetRepeat(mode.into()))).await
    }

    /// Stop playback after a while or at the end of the track or record,
    /// fading out and pausing, here or on the device being controlled.
    pub async fn set_sleep_timer(self: Arc<Self>, timer: SleepTimer) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::SetSleepTimer(Some(timer.into()))))
            .await
    }

    pub async fn cancel_sleep_timer(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::SetSleepTimer(None))).await
    }

    pub async fn seek(self: Arc<Self>, position_ms: u64) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::Seek(position_ms))).await
    }

    // --- Observable state --------------------------------------------------

    /// Follow the engine's state.
    ///
    /// The one way a client learns anything changed. Each answer is a batch of
    /// whole slices — see `state` for why they are snapshots and why the
    /// cursor cannot lose one. A fresh stream's first answer is the entire
    /// state, so there is nothing to seed from separately.
    pub fn observe(&self) -> Arc<state::StateStream> {
        state::StateStream::new(&self.out)
    }

    /// Forget the transfers that have already settled. Running ones are left
    /// alone — stopping one is a different verb.
    pub fn clear_settled_downloads(&self) {
        self.state.downloads().clear_settled();
    }

    /// The byte counts of every transfer still going, read now.
    ///
    /// For a client drawing progress at its display's rate, which the `Figures`
    /// slice is not: it moves when a rate sample is taken, a few times a
    /// second. Synchronous because a display link has a frame to fill and no
    /// time to await one; it holds the download list's read lock for one pass
    /// over a few dozen entries and touches nothing else.
    pub fn transfer_readings(&self) -> Vec<TransferFigure> {
        self.state
            .downloads()
            .readings()
            .iter()
            .map(TransferFigure::reading)
            .collect()
    }

    /// Whether the player is playing, read from the engine now. For code that
    /// runs with the app in the background, where the mirror, refreshed for
    /// what is on screen, can still say what it said before.
    pub fn is_playing(&self) -> bool {
        self.state.playback_state() == koan_core::player::state::PlaybackState::Playing
    }

    /// Follow the spectrum, one message per analysed frame.
    ///
    /// The analyser is the clock. It runs at the rate the display asks for
    /// (see `set_viz_fps`), publishes a frame when it has one, and publishes
    /// nothing at all when the play head has stopped and the bars have fallen
    /// — so a paused koan delivers no messages, wakes nothing, and the thread
    /// that would have produced them is parked rather than looping.
    pub fn viz_stream(&self) -> Arc<VizStream> {
        VizStream::new(&self.viz)
    }

    /// Run the analyser at the refresh rate of the display it is drawn on.
    ///
    /// koan cannot know that rate and a window can: 60 on one panel, 120 on
    /// another, and it changes when the window is dragged between them. One
    /// atomic store — the analyser picks it up on its next pass, and nothing
    /// wakes for it.
    pub fn set_viz_fps(&self, fps: u8) {
        self.viz.set_fps(fps);
        koan_core::remote::levels::remote().set_fps(fps);
    }

    // --- Queue mutation ----------------------------------------------------

    /// Append tracks. Starts playback if the player was stopped, and kicks off
    /// downloads for anything remote. Returns the new queue item IDs.
    pub async fn add_to_queue(
        self: Arc<Self>,
        track_ids: Vec<i64>,
    ) -> Result<Vec<String>, KoanError> {
        offload::sequenced(move || {
            let db = self.db()?;
            let items = self.build_items(&db, &track_ids);
            if items.is_empty() {
                return Ok(Vec::new());
            }

            let ids: Vec<String> = items.iter().map(|i| i.id.0.to_string()).collect();
            let first = items[0].id;
            let was_stopped = self.state.is_idle();

            self.send(PlayerCommand::AddToPlaylist(items))?;
            // Another device starts what it was sent by itself when stopped.
            if was_stopped && koan_core::remote::devices::target().is_none() {
                let _ = self.tx.send(PlayerCommand::Play(first));
            }

            Ok(ids)
        })
        .await
    }

    /// Replace the queue, starting at `start_at` (default: the first track).
    ///
    /// The index is part of the command rather than a follow-up `play` because
    /// two commands means the first track audibly starts before the jump lands:
    /// clicking track nine of an album flashed track one as playing first.
    /// An index past the end starts at the beginning.
    pub async fn replace_queue(
        self: Arc<Self>,
        track_ids: Vec<i64>,
        start_at: Option<u32>,
    ) -> Result<Vec<String>, KoanError> {
        offload::sequenced(move || {
            let db = self.db()?;
            let items = self.build_items(&db, &track_ids);
            if items.is_empty() {
                self.send(PlayerCommand::ClearPlaylist)?;
                return Ok(Vec::new());
            }

            let ids: Vec<String> = items.iter().map(|i| i.id.0.to_string()).collect();
            self.send(PlayerCommand::ReplacePlaylist {
                items,
                start: start_at.unwrap_or(0) as usize,
                position_ms: 0,
                play: true,
            })?;

            Ok(ids)
        })
        .await
    }

    /// Insert after an existing item — what a drop between two rows means.
    pub async fn insert_after(
        self: Arc<Self>,
        track_ids: Vec<i64>,
        after_queue_item_id: String,
    ) -> Result<Vec<String>, KoanError> {
        offload::sequenced(move || {
            let after = parse_qid(&after_queue_item_id)?;
            let db = self.db()?;
            let items = self.build_items(&db, &track_ids);
            if items.is_empty() {
                return Ok(Vec::new());
            }

            let ids: Vec<String> = items.iter().map(|i| i.id.0.to_string()).collect();
            self.send(PlayerCommand::InsertInPlaylist { items, after })?;

            Ok(ids)
        })
        .await
    }

    /// Removed as one undo step, however many IDs are passed.
    pub async fn remove_from_queue(
        self: Arc<Self>,
        queue_item_ids: Vec<String>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            let ids = parse_qids(&queue_item_ids)?;
            self.send(PlayerCommand::RemoveFromPlaylistBatch(ids))
        })
        .await
    }

    /// Reorder. `after` puts the items below the target rather than above.
    pub async fn move_in_queue(
        self: Arc<Self>,
        queue_item_ids: Vec<String>,
        target_queue_item_id: String,
        after: bool,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            let ids = parse_qids(&queue_item_ids)?;
            let target = parse_qid(&target_queue_item_id)?;
            self.send(PlayerCommand::MoveItemsInPlaylist { ids, target, after })
        })
        .await
    }

    pub async fn clear_queue(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send(PlayerCommand::ClearPlaylist)).await
    }

    /// Take back the last edit to a playlist, or to the queue when none is
    /// named.
    pub async fn undo(self: Arc<Self>, playlist_id: Option<i64>) -> Result<(), KoanError> {
        match playlist_id {
            Some(id) => self.step_playlist(id, true).await,
            None => offload::sequenced(move || self.send(PlayerCommand::Undo)).await,
        }
    }

    /// Put back the last edit taken back, to a playlist or the queue.
    pub async fn redo(self: Arc<Self>, playlist_id: Option<i64>) -> Result<(), KoanError> {
        match playlist_id {
            Some(id) => self.step_playlist(id, false).await,
            None => offload::sequenced(move || self.send(PlayerCommand::Redo)).await,
        }
    }

    // --- Library -----------------------------------------------------------

    /// The library's artists, narrowed by `search`.
    ///
    /// Whole, not paged. This is an in-process call, and a library's artists
    /// are a bounded set — a few thousand records marshalled once beats a
    /// client that has to know how far it has scrolled.
    pub async fn artists(
        self: Arc<Self>,
        search: Option<String>,
        filter: BrowseFilter,
    ) -> Result<Vec<Artist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let played = recent(&filter);
            let rows = queries::list_artists(
                &db.conn,
                &queries::ArtistQuery {
                    search: trimmed(&search),
                    favourites_of: filter.favourites.then_some(queries::LOCAL_USER),
                    // Recently Played's own order, or the search shelf's: the
                    // artist browser has no sort to choose another by.
                    // Without a search, relevance is by name.
                    order: if played.is_some() {
                        queries::ArtistOrder::LastPlayed
                    } else {
                        queries::ArtistOrder::Relevance
                    },
                    played,
                    filter: album_filter(&filter),
                    ..Default::default()
                },
            )
            .map_err(db_err)?;
            Ok(rows.into_iter().map(Artist::from).collect())
        })
        .await
    }

    pub async fn artist(self: Arc<Self>, artist_id: i64) -> Result<Option<Artist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::get_artist(&db.conn, artist_id)
                .map_err(db_err)?
                .map(Artist::from))
        })
        .await
    }

    /// The library's albums, narrowed by `search` and `filter` and ordered by
    /// `sort`.
    ///
    /// Both run in SQL. A client that narrows or sorts what it has already been
    /// handed pays to read and marshal every album in the library on each
    /// keystroke, and has to reimplement in its own language an answer the
    /// database already knows.
    ///
    /// Whole, not paged, for the reason [`Self::artists`] gives.
    ///
    /// `seed` fixes the shuffle under [`AlbumSort::Random`] and is ignored by
    /// every other sort, so that narrowing a shuffled listing does not deal it
    /// again. A new seed is a new shuffle, which is what a reshuffle asks for.
    pub async fn albums(
        self: Arc<Self>,
        artist_id: Option<i64>,
        sort: AlbumSort,
        seed: i64,
        search: Option<String>,
        filter: BrowseFilter,
    ) -> Result<Vec<Album>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::list_albums(
                &db.conn,
                &queries::AlbumQuery {
                    artist_id,
                    search: trimmed(&search),
                    order: album_order(sort, seed),
                    favourites_of: filter.favourites.then_some(queries::LOCAL_USER),
                    played: recent(&filter),
                    filter: album_filter(&filter),
                    ..Default::default()
                },
            )
            .map_err(db_err)?;
            Ok(rows.into_iter().map(Album::from).collect())
        })
        .await
    }

    /// What the browsers' codec and genre filters offer.
    pub async fn browse_choices(self: Arc<Self>) -> Result<BrowseChoices, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(BrowseChoices {
                codecs: queries::album_codecs(&db.conn).map_err(db_err)?,
                genres: queries::genres(&db.conn, GENRES_OFFERED).map_err(db_err)?,
            })
        })
        .await
    }

    pub async fn album(self: Arc<Self>, album_id: i64) -> Result<Option<Album>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::get_album(&db.conn, album_id)
                .map_err(db_err)?
                .map(Album::from))
        })
        .await
    }

    /// Everything a record's page shows, in one call.
    ///
    /// One round trip and one pooled connection per click. As two parallel
    /// calls it would take two connections, and whenever the pool has to open
    /// one (schema DDL and a WAL checkpoint) a click costs about 300ms.
    pub async fn album_page(self: Arc<Self>, album_id: i64) -> Result<AlbumPage, KoanError> {
        offload::offload(move || {
            let waited = std::time::Instant::now();
            let db = self.db()?;
            let pool = waited.elapsed();

            let queried = std::time::Instant::now();
            let album = queries::get_album(&db.conn, album_id)
                .map_err(db_err)?
                .map(Album::from);
            let rows = queries::tracks_for_album(&db.conn, album_id).map_err(db_err)?;
            let tracks = self.decorate(&db, rows);
            let query = queried.elapsed();

            // A canary, not tracing. These are two indexed reads and they take
            // well under a millisecond; anything near a frame means something
            // else had the database, and which half stalled is the whole
            // question when it happens.
            if pool + query > std::time::Duration::from_millis(50) {
                log::warn!("slow album page {album_id}: pool {pool:?}, query {query:?}");
            }
            Ok(AlbumPage { album, tracks })
        })
        .await
    }

    /// Tracks for an album or artist, or a page of the whole library when
    /// neither is given.
    pub async fn tracks(
        self: Arc<Self>,
        album_id: Option<i64>,
        artist_id: Option<i64>,
        sort: TrackSort,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Track>, KoanError> {
        offload::offload(move || self.tracks_blocking(album_id, artist_id, sort, limit, offset))
            .await
    }

    /// A page of the track browser: the library's tracks narrowed by `search`
    /// (the full-text index, as search's tracks are) and `filter`, ordered by
    /// `sort`, with how many pass in all. Paged, unlike the album and artist
    /// listings: a library has tens of thousands of tracks. `filter.lossless`
    /// does not apply to tracks.
    pub async fn track_listing(
        self: Arc<Self>,
        sort: TrackBrowseSort,
        search: Option<String>,
        filter: BrowseFilter,
        limit: u32,
        offset: u32,
    ) -> Result<TrackListing, KoanError> {
        use koan_core::shelves::{self, Shelf};
        offload::offload(move || {
            let db = self.db()?;
            let (order, descending) = match sort {
                TrackBrowseSort::Artist => (queries::TrackOrder::ArtistAlbumDiscTrack, false),
                TrackBrowseSort::Title => (queries::TrackOrder::Title, false),
                TrackBrowseSort::Album => (queries::TrackOrder::Album, false),
                TrackBrowseSort::Duration => (queries::TrackOrder::Duration, false),
                TrackBrowseSort::LastPlayed => (queries::TrackOrder::LastPlayed, true),
                TrackBrowseSort::BestMatch => (queries::TrackOrder::Relevance, false),
            };
            let (user, now) = (queries::LOCAL_USER, shelves::now());
            let listing = shelves::Tracks {
                filter: queries::TrackFilter {
                    search: trimmed(&search)
                        .and_then(|q| Shelf::Search(q).tracks(user, now).filter.search),
                    favourites_of: filter.favourites.then_some(user),
                    played: recent(&filter),
                    codec: trimmed(&filter.codec).map(str::to_owned),
                    genre: trimmed(&filter.genre).map(str::to_owned),
                    year_start: filter.year_from,
                    year_end: filter.year_to,
                    on_device: filter.downloaded || offline(),
                    ..Default::default()
                },
                order,
                descending,
            };
            let total = listing.count(&db.conn).map_err(db_err)?;
            let rows = listing.page(&db.conn, limit, offset).map_err(db_err)?;
            Ok(TrackListing {
                tracks: self.decorate(&db, rows),
                total,
            })
        })
        .await
    }

    pub async fn track(self: Arc<Self>, track_id: i64) -> Result<Option<Track>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let Some(row) = queries::get_track_row(&db.conn, track_id).map_err(db_err)? else {
                return Ok(None);
            };
            Ok(self.decorate(&db, vec![row]).into_iter().next())
        })
        .await
    }

    /// Everything known about a track, for its info view. `None` for a track
    /// that is not in the library.
    pub async fn track_info(
        self: Arc<Self>,
        track_id: i64,
    ) -> Result<Option<TrackInfo>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let Some(row) = queries::get_track_row(&db.conn, track_id).map_err(db_err)? else {
                return Ok(None);
            };
            let Some(track) = self.decorate(&db, vec![row]).into_iter().next() else {
                return Ok(None);
            };
            let uid = queries::uids_for(&db.conn, queries::UidKind::Track, [track_id])
                .map_err(db_err)?
                .remove(&track_id);
            let sources = queries::sources_of_track(&db.conn, track_id)
                .map_err(db_err)?
                .into_iter()
                .map(Into::into)
                .collect();
            let replay_gain = track
                .path
                .as_deref()
                .and_then(|p| koan_core::audio::replaygain::read_tags(std::path::Path::new(p)).ok())
                .map(|rg| {
                    [
                        (
                            "Track gain",
                            rg.track_gain_db.map(|g| format!("{g:+.2} dB")),
                        ),
                        ("Track peak", rg.track_peak.map(|p| format!("{p:.6}"))),
                        (
                            "Album gain",
                            rg.album_gain_db.map(|g| format!("{g:+.2} dB")),
                        ),
                        ("Album peak", rg.album_peak.map(|p| format!("{p:.6}"))),
                    ]
                    .into_iter()
                    .filter_map(|(name, value)| {
                        value.map(|value| InfoField {
                            name: name.into(),
                            value,
                        })
                    })
                    .collect()
                })
                .unwrap_or_default();
            Ok(Some(TrackInfo {
                track,
                uid,
                sources,
                replay_gain,
            }))
        })
        .await
    }

    /// FTS5 search across title, artist, album, genre.
    pub async fn search(
        self: Arc<Self>,
        query: String,
        limit: u32,
    ) -> Result<Vec<Track>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let filter = queries::TrackFilter {
                search: Some(query),
                on_device: offline(),
                ..Default::default()
            };
            let rows = queries::filter_tracks(
                &db.conn,
                &filter,
                queries::TrackOrder::ArtistAlbumDiscTrack,
                false,
                limit,
                0,
            )
            .map_err(db_err)?;
            Ok(self.decorate(&db, rows))
        })
        .await
    }

    /// Nucleo fuzzy match — what the command palette wants. Ranked, best first.
    pub async fn fuzzy_search(
        self: Arc<Self>,
        query: String,
        kind: SearchKind,
        limit: u32,
    ) -> Result<Vec<FuzzyMatch>, KoanError> {
        offload::offload(move || {
            let items = self.corpus(match kind {
                SearchKind::Track => queries::CorpusKind::Track,
                SearchKind::Album => queries::CorpusKind::Album,
                SearchKind::Artist => queries::CorpusKind::Artist,
            })?;
            let texts: Vec<&str> = items.iter().map(|(_, t)| t.as_str()).collect();
            Ok(fuzzy_rank(&texts, &query, limit)
                .into_iter()
                .map(|i| FuzzyMatch {
                    id: items[i].0,
                    name: items[i].1.clone(),
                    kind,
                })
                .collect())
        })
        .await
    }

    pub async fn random_tracks(
        self: Arc<Self>,
        count: u32,
        artist_id: Option<i64>,
    ) -> Result<Vec<Track>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::random_tracks(&db.conn, count, artist_id).map_err(db_err)?;
            Ok(self.decorate(&db, rows))
        })
        .await
    }

    pub async fn library_stats(self: Arc<Self>) -> Result<Stats, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::library_stats(&db.conn).map_err(db_err)?.into())
        })
        .await
    }

    /// Raw image bytes — no base64 round trip, unlike the GraphQL surface,
    /// which has to encode because JSON can't carry binary.
    ///
    /// Embedded tags first, then the remote server. A library synced from
    /// Navidrome has no local files to read art out of, so without the remote
    /// fallback every album is blank. `size` requests a thumbnail; the grid
    /// wants one, the now-playing pane doesn't. Hits the network on the remote
    /// path.
    pub async fn cover_art(
        self: Arc<Self>,
        track_id: i64,
        size: Option<u32>,
    ) -> Result<Option<CoverArt>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let row = queries::get_track_row(&db.conn, track_id).map_err(db_err)?;
            self.cover_art_of(row, size)
        })
        .await
    }

    /// The record's artwork, asked for by the record.
    ///
    /// Every track on an album shares its cover, so the track whose art stands
    /// for the album is resolved in SQL, in the same call that returns the
    /// bytes. A client never lists the album's tracks just to find one id.
    pub async fn album_cover_art(
        self: Arc<Self>,
        album_id: i64,
        size: Option<u32>,
    ) -> Result<Option<CoverArt>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let row = queries::cover_track_for_album(&db.conn, album_id).map_err(db_err)?;
            self.cover_art_of(row, size)
        })
        .await
    }

    /// Why the configured server cannot be used, if it cannot: no credential,
    /// or one the server has refused since.
    ///
    /// `None` means there is nothing to say: either no server is configured, or
    /// the one that is works. A client should not have to watch playback fail
    /// and artwork come back empty to work out that it is signed out — the
    /// engine already knows, and every front end asks the same question. A
    /// refusal heard later reaches the app as `ConnectionInfo::sign_in_refused`.
    pub async fn remote_problem(self: Arc<Self>) -> Option<String> {
        offload::offload(move || koan_core::helpers::remote_problem(&Config::cached())).await
    }

    /// Cached lyrics only — this never hits the network, so it is safe to call
    /// from a view body's task without stalling on LRCLIB.
    pub async fn lyrics(self: Arc<Self>, track_id: i64) -> Result<Option<Lyrics>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(queries::get_cached_lyrics(&db.conn, track_id)
                .map_err(db_err)?
                .map(|(content, synced)| {
                    let lines = if synced {
                        koan_core::lyrics::parse_lrc(&content)
                            .into_iter()
                            .map(|l| LyricLine {
                                time_secs: l.time_secs,
                                text: l.text,
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    Lyrics {
                        content,
                        synced,
                        source: "cache".into(),
                        lines,
                    }
                }))
        })
        .await
    }

    /// Cache, then LRCLIB. Hits the network on a miss; `lyrics()` answers from
    /// the cache alone and returns without one.
    pub async fn fetch_lyrics(self: Arc<Self>, track_id: i64) -> Result<Option<Lyrics>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let Some(row) = queries::get_track_row(&db.conn, track_id).map_err(db_err)? else {
                return Ok(None);
            };
            let duration_secs = row.duration_ms.unwrap_or(0).max(0) as u64 / 1000;
            match koan_core::lyrics::fetch_lyrics(
                &db.conn,
                track_id,
                &row.artist_name,
                &row.title,
                &row.album_title,
                duration_secs,
            ) {
                Ok(l) => Ok(Some(l.into())),
                // A track with no lyrics anywhere is the normal case, not an error.
                Err(_) => Ok(None),
            }
        })
        .await
    }

    // --- Artist info -------------------------------------------------------

    /// Cached only — never the network, so a page can ask on every draw.
    pub async fn artist_info(
        self: Arc<Self>,
        artist_id: i64,
    ) -> Result<Option<ArtistInfo>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(koan_core::artist_info::cached(&db.conn, artist_id)
                .map_err(db_err)?
                .map(ArtistInfo::from))
        })
        .await
    }

    /// The cache while fresh, otherwise MusicBrainz, Wikidata and Wikipedia.
    /// Seconds on a miss; an artist nothing can be found for is the normal
    /// case, not an error.
    pub async fn fetch_artist_info(
        self: Arc<Self>,
        artist_id: i64,
    ) -> Result<Option<ArtistInfo>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(koan_core::artist_info::fetch(&db.conn, artist_id)
                .ok()
                .flatten()
                .map(ArtistInfo::from))
        })
        .await
    }

    /// The artist's photograph. Hits the network; the caller caches it.
    pub async fn artist_image(
        self: Arc<Self>,
        artist_id: i64,
    ) -> Result<Option<CoverArt>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(koan_core::artist_info::image(&db.conn, artist_id)
                .ok()
                .flatten()
                .map(|data| CoverArt {
                    mime: sniff_mime(&data).to_string(),
                    data,
                }))
        })
        .await
    }

    // --- Play history ------------------------------------------------------

    /// Every play, most recent first, narrowed by `search`.
    ///
    /// A list of events, not of tracks: a track played three times is three
    /// entries. Entries whose track has left the library are already gone.
    ///
    /// Whole, not paged, for the reason [`Self::artists`] gives.
    pub async fn play_history(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<PlayHistoryEntry>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::play_history_with_tracks(
                &db.conn,
                queries::LOCAL_USER,
                trimmed(&search),
                None,
                0,
            )
            .map_err(db_err)?;
            let (plays, tracks): (Vec<_>, Vec<_>) = rows
                .into_iter()
                .map(|r| ((r.id, r.played_at, r.listened_ms, r.source), r.track))
                .unzip();
            Ok(self
                .decorate(&db, tracks)
                .into_iter()
                .zip(plays)
                .map(
                    |(track, (id, played_at, listened_ms, source))| PlayHistoryEntry {
                        id,
                        track,
                        played_at,
                        listened_ms,
                        source,
                    },
                )
                .collect())
        })
        .await
    }

    /// What was played lately: the records, artists and tracks of the last
    /// `shelves::RECENT_DAYS`, at most `RECENT_LIMIT` of each, each once and
    /// newest first by its latest play, narrowed by `search`. The shelf, its
    /// window and its order are `koan_core::shelves`'.
    pub async fn recently_played(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<RecentlyPlayed, KoanError> {
        use koan_core::shelves::{self, Shelf};
        offload::offload(move || {
            let db = self.db()?;
            let (user, now) = (queries::LOCAL_USER, shelves::now());
            let search = trimmed(&search);
            let albums = queries::list_albums(
                &db.conn,
                &queries::AlbumQuery {
                    search,
                    limit: Some(RECENT_LIMIT),
                    filter: offline_filter(),
                    ..Shelf::Recent.albums(user, now)
                },
            )
            .map_err(db_err)?;
            let artists = queries::list_artists(
                &db.conn,
                &queries::ArtistQuery {
                    search,
                    limit: Some(RECENT_LIMIT),
                    filter: offline_filter(),
                    ..Shelf::Recent.artists(user, now)
                },
            )
            .map_err(db_err)?;
            // Narrowed after the cap, as before: a substring over the title,
            // artist and record, which the full-text index does not answer.
            let needle = search.map(str::to_lowercase);
            let mut recent = Shelf::Recent.tracks(user, now);
            recent.filter.on_device = offline();
            let tracks: Vec<_> = recent
                .page(&db.conn, RECENT_LIMIT, 0)
                .map_err(db_err)?
                .into_iter()
                .filter(|t| {
                    needle.as_ref().is_none_or(|n| {
                        [&t.title, &t.artist_name, &t.album_title]
                            .iter()
                            .any(|s| s.to_lowercase().contains(n.as_str()))
                    })
                })
                .collect();
            Ok(RecentlyPlayed {
                albums: albums.into_iter().map(Album::from).collect(),
                artists: artists.into_iter().map(Artist::from).collect(),
                tracks: self.decorate(&db, tracks),
            })
        })
        .await
    }

    /// A shelf page: the first few artists, records and tracks on `shelf`,
    /// with how many there are of each. A section's heading opens the library's own
    /// listing with the same shelf as its filter; see `koan_core::shelves`.
    pub async fn shelf_summary(
        self: Arc<Self>,
        shelf: ShelfKind,
    ) -> Result<ShelfSummary, KoanError> {
        use koan_core::shelves::{self, Shelf};
        offload::offload(move || {
            let db = self.db()?;
            let shelf = match &shelf {
                ShelfKind::Favourites => Shelf::Favourites,
                ShelfKind::Recent => Shelf::Recent,
                ShelfKind::Search { query } => Shelf::Search(query),
                ShelfKind::Downloaded => Shelf::Downloaded,
            };
            let s = shelves::summary(
                &db.conn,
                shelf,
                queries::LOCAL_USER,
                shelves::now(),
                offline(),
            )
            .map_err(db_err)?;
            Ok(ShelfSummary {
                artists: s.artists.preview.into_iter().map(Artist::from).collect(),
                artist_total: s.artists.total,
                albums: s.albums.preview.into_iter().map(Album::from).collect(),
                album_total: s.albums.total,
                tracks: self.decorate(&db, s.tracks.preview),
                track_total: s.tracks.total,
            })
        })
        .await
    }

    /// Turn offline mode on or off by hand: the library narrows to what can
    /// play here, as it does on its own when the server cannot be reached.
    pub fn set_offline(&self, on: bool) {
        koan_core::remote::offline::set_manual(on);
        self.bump_library();
    }

    /// Forget specific plays, here and, signed in to a koan server, on every
    /// device on the account. Returns how many entries were removed.
    pub async fn delete_plays(self: Arc<Self>, ids: Vec<i64>) -> Result<u32, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let removed = koan_core::remote::history::forget(&db, &ids).map_err(db_err)?;
            koan_core::player::history::changed();
            Ok(removed as u32)
        })
        .await
    }

    /// Forget every play, here and, signed in to a koan server, on every
    /// device on the account. Returns how many entries were removed.
    pub async fn clear_play_history(self: Arc<Self>) -> Result<u32, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let removed = koan_core::remote::history::clear(&db).map_err(db_err)?;
            koan_core::player::history::changed();
            Ok(removed as u32)
        })
        .await
    }

    // --- Favourites --------------------------------------------------------

    /// Favourited tracks, narrowed by `search`.
    pub async fn favourites(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<Track>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let mut favourites = koan_core::shelves::Shelf::Favourites
                .tracks(queries::LOCAL_USER, koan_core::shelves::now());
            favourites.filter.on_device = offline();
            let rows = favourites.page(&db.conn, u32::MAX, 0).map_err(db_err)?;
            let needle = trimmed(&search).map(str::to_lowercase);
            let rows = rows
                .into_iter()
                .filter(|t| {
                    needle.as_ref().is_none_or(|n| {
                        [&t.title, &t.artist_name, &t.album_title]
                            .iter()
                            .any(|s| s.to_lowercase().contains(n.as_str()))
                    })
                })
                .collect();
            Ok(self.decorate(&db, rows))
        })
        .await
    }

    /// Favourited records, as rows, narrowed by `search`.
    pub async fn favourite_albums(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<Album>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::list_albums(
                &db.conn,
                &queries::AlbumQuery {
                    search: trimmed(&search),
                    filter: offline_filter(),
                    ..koan_core::shelves::Shelf::Favourites
                        .albums(queries::LOCAL_USER, koan_core::shelves::now())
                },
            )
            .map_err(db_err)?;
            Ok(rows.into_iter().map(Album::from).collect())
        })
        .await
    }

    /// Favourited artists, as rows, narrowed by `search`.
    pub async fn favourite_artists(
        self: Arc<Self>,
        search: Option<String>,
    ) -> Result<Vec<Artist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows = queries::list_artists(
                &db.conn,
                &queries::ArtistQuery {
                    search: trimmed(&search),
                    filter: offline_filter(),
                    ..koan_core::shelves::Shelf::Favourites
                        .artists(queries::LOCAL_USER, koan_core::shelves::now())
                },
            )
            .map_err(db_err)?;
            Ok(rows.into_iter().map(Artist::from).collect())
        })
        .await
    }

    /// Returns the new state. Syncs to the remote server in the background when
    /// one is configured.
    pub async fn toggle_favourite(self: Arc<Self>, track_id: i64) -> Result<bool, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::get_track_row(&db.conn, track_id)
                .map_err(db_err)?
                .ok_or_else(|| KoanError::NotFound {
                    message: format!("track {track_id}"),
                })?;
            let now_favourite = queries::toggle_favourite(&db.conn, queries::LOCAL_USER, track_id)
                .map_err(fav_err)?;
            koan_core::helpers::sync_favourite_to_remote(&db, track_id, now_favourite);
            Ok(now_favourite)
        })
        .await
    }

    /// Every favourited track id, for the UI to read row state from one place
    /// rather than from a copy baked into each row when it was fetched.
    pub async fn favourite_track_ids(self: Arc<Self>) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(
                queries::favourite_track_ids_batch(&db.conn, queries::LOCAL_USER)
                    .map_err(db_err)?
                    .into_iter()
                    .collect(),
            )
        })
        .await
    }

    pub async fn favourite_album_ids(self: Arc<Self>) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(
                queries::favourite_album_id_set(&db.conn, queries::LOCAL_USER)
                    .map_err(fav_err)?
                    .into_iter()
                    .collect(),
            )
        })
        .await
    }

    pub async fn favourite_artist_ids(self: Arc<Self>) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            Ok(
                queries::favourite_artist_id_set(&db.conn, queries::LOCAL_USER)
                    .map_err(fav_err)?
                    .into_iter()
                    .collect(),
            )
        })
        .await
    }

    /// Toggle an album favourite. Returns the new state.
    pub async fn toggle_favourite_album(self: Arc<Self>, album_id: i64) -> Result<bool, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::get_album(&db.conn, album_id)
                .map_err(db_err)?
                .ok_or_else(|| KoanError::NotFound {
                    message: format!("album {album_id}"),
                })?;
            let now = queries::toggle_favourite_album(&db.conn, queries::LOCAL_USER, album_id)
                .map_err(fav_err)?;
            koan_core::helpers::sync_collection_favourite_to_remote(
                &db,
                koan_core::helpers::FavouriteKind::Album,
                album_id,
                now,
            );
            Ok(now)
        })
        .await
    }

    /// Toggle an artist favourite. Returns the new state.
    pub async fn toggle_favourite_artist(
        self: Arc<Self>,
        artist_id: i64,
    ) -> Result<bool, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::get_artist(&db.conn, artist_id)
                .map_err(db_err)?
                .ok_or_else(|| KoanError::NotFound {
                    message: format!("artist {artist_id}"),
                })?;
            let now = queries::toggle_favourite_artist(&db.conn, queries::LOCAL_USER, artist_id)
                .map_err(fav_err)?;
            koan_core::helpers::sync_collection_favourite_to_remote(
                &db,
                koan_core::helpers::FavouriteKind::Artist,
                artist_id,
                now,
            );
            Ok(now)
        })
        .await
    }

    // --- Playlists ---------------------------------------------------------
    //
    // A playlist is a named, ordered list of library tracks — the same object
    // Navidrome holds, so the two can be reconciled. Every edit writes locally
    // and then pushes to the server in the background; nothing waits on the
    // network, and a push that never got out is settled by the next sync.

    pub async fn playlists(self: Arc<Self>) -> Result<Vec<Playlist>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            self.refresh_smart(&db, None);
            Ok(queries::list_playlists(&db.conn, queries::LOCAL_USER)
                .map_err(db_err)?
                .into_iter()
                .map(Playlist::from)
                .collect())
        })
        .await
    }

    /// The playlist's tracks, in playlist order. Duplicates are kept — the same
    /// song twice in a row is a thing people do on purpose.
    pub async fn playlist_tracks(
        self: Arc<Self>,
        playlist_id: i64,
    ) -> Result<Vec<PlaylistEntry>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            self.refresh_smart(&db, Some(playlist_id));
            let mut entries = queries::playlist_entries(&db.conn, playlist_id).map_err(db_err)?;
            if offline() {
                entries.retain(|e| e.track.cached_path.is_some() || e.track.path.is_some());
            }
            let ids: Vec<i64> = entries.iter().map(|e| e.id).collect();
            let tracks = self.decorate(&db, entries.into_iter().map(|e| e.track).collect());
            Ok(ids
                .into_iter()
                .zip(tracks)
                .map(|(entry_id, track)| PlaylistEntry { entry_id, track })
                .collect())
        })
        .await
    }

    /// Up to four albums whose covers make the playlist's tile, in playlist
    /// order.
    pub async fn playlist_cover_album_ids(
        self: Arc<Self>,
        playlist_id: i64,
    ) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::playlist_cover_album_ids(&db.conn, playlist_id).map_err(db_err)
        })
        .await
    }

    pub async fn create_playlist(
        self: Arc<Self>,
        name: String,
        track_ids: Vec<i64>,
    ) -> Result<Playlist, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let id = queries::create_playlist(&db.conn, queries::LOCAL_USER, &name, None)
                .map_err(db_err)?;
            if !track_ids.is_empty() {
                queries::add_tracks(&db.conn, id, &track_ids).map_err(db_err)?;
            }
            self.bump_library();
            koan_core::playlists::push_to_remote(id);
            queries::get_playlist(&db.conn, id)
                .map_err(db_err)?
                .map(Playlist::from)
                .ok_or_else(|| KoanError::NotFound {
                    message: format!("playlist {id}"),
                })
        })
        .await
    }

    pub async fn rename_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        name: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            if let Some(list) = queries::get_playlist(&db.conn, playlist_id).map_err(db_err)?
                && list.source_path.is_some()
            {
                return Err(KoanError::BadArgument {
                    message: format!("'{}' is named by its file in the library", list.name),
                });
            }
            queries::rename_playlist(&db.conn, playlist_id, &name).map_err(db_err)?;
            self.bump_library();
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(())
        })
        .await
    }

    pub async fn delete_playlist(self: Arc<Self>, playlist_id: i64) -> Result<bool, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            // Read the server id before the row goes: the delete has to reach
            // the server too, or the next sync brings the playlist back.
            let remote_id = queries::get_playlist(&db.conn, playlist_id)
                .ok()
                .flatten()
                .and_then(|p| p.remote_id);
            let deleted = queries::delete_playlist(&db.conn, playlist_id).map_err(db_err)?;
            if deleted {
                self.bump_library();
                if let Some(remote_id) = remote_id {
                    koan_core::playlists::delete_on_remote(remote_id);
                }
            }
            Ok(deleted)
        })
        .await
    }

    /// Append tracks. Returns how many landed.
    pub async fn add_to_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        track_ids: Vec<i64>,
    ) -> Result<u32, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            fillable(&db, playlist_id)?;
            self.playlist_history
                .record(&db.conn, playlist_id)
                .map_err(db_err)?;
            let locked = self.locked_to(&db, playlist_id);
            let added = queries::add_tracks(&db.conn, playlist_id, &track_ids).map_err(db_err)?;
            self.follow_playlist(&db, playlist_id, locked);
            self.bump_library();
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(added.len() as u32)
        })
        .await
    }

    /// Add tracks at a position rather than at the end — what dropping between
    /// two rows means.
    ///
    /// Both halves happen here rather than as an add followed by a reorder from
    /// the caller: the caller would be reordering against the list as it was
    /// before its own insert landed, and racing the reload that tells it
    /// otherwise.
    pub async fn insert_into_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        track_ids: Vec<i64>,
        at: u32,
    ) -> Result<u32, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            fillable(&db, playlist_id)?;
            self.playlist_history
                .record(&db.conn, playlist_id)
                .map_err(db_err)?;
            let locked = self.locked_to(&db, playlist_id);
            let added = queries::add_tracks(&db.conn, playlist_id, &track_ids).map_err(db_err)?;
            if !added.is_empty() {
                let mut order: Vec<i64> = queries::playlist_entries(&db.conn, playlist_id)
                    .map_err(db_err)?
                    .into_iter()
                    .map(|e| e.id)
                    .filter(|id| !added.contains(id))
                    .collect();
                let at = (at as usize).min(order.len());
                order.splice(at..at, added.iter().copied());
                queries::reorder_entries(&db.conn, playlist_id, &order).map_err(db_err)?;
            }
            self.follow_playlist(&db, playlist_id, locked);
            self.bump_library();
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(added.len() as u32)
        })
        .await
    }

    /// Put the entries in this order. Ids survive, so anything holding one —
    /// a queue item, say — still knows which row it means.
    pub async fn reorder_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        entry_ids: Vec<i64>,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            fillable(&db, playlist_id)?;
            self.playlist_history
                .record(&db.conn, playlist_id)
                .map_err(db_err)?;
            let locked = self.locked_to(&db, playlist_id);
            queries::reorder_entries(&db.conn, playlist_id, &entry_ids).map_err(db_err)?;
            self.follow_playlist(&db, playlist_id, locked);
            self.bump_library();
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(())
        })
        .await
    }

    /// Take entries out. Returns how many went.
    pub async fn remove_from_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        entry_ids: Vec<i64>,
    ) -> Result<u32, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            fillable(&db, playlist_id)?;
            self.playlist_history
                .record(&db.conn, playlist_id)
                .map_err(db_err)?;
            let locked = self.locked_to(&db, playlist_id);
            let removed =
                queries::remove_entries(&db.conn, playlist_id, &entry_ids).map_err(db_err)?;
            self.follow_playlist(&db, playlist_id, locked);
            self.bump_library();
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(removed as u32)
        })
        .await
    }

    /// Shuffle the playlist itself, in place. Distinct from shuffling it into
    /// the queue, which leaves the playlist alone.
    pub async fn shuffle_playlist(self: Arc<Self>, playlist_id: i64) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            fillable(&db, playlist_id)?;
            self.playlist_history
                .record(&db.conn, playlist_id)
                .map_err(db_err)?;
            let mut entries = queries::playlist_entries(&db.conn, playlist_id).map_err(db_err)?;
            koan_core::helpers::shuffle(&mut entries);
            let order: Vec<i64> = entries.iter().map(|e| e.id).collect();
            let locked = self.locked_to(&db, playlist_id);
            queries::reorder_entries(&db.conn, playlist_id, &order).map_err(db_err)?;
            self.follow_playlist(&db, playlist_id, locked);
            self.bump_library();
            koan_core::playlists::push_to_remote(playlist_id);
            Ok(())
        })
        .await
    }

    /// Where the playlists sit in the sidebar, in the order given. Local only —
    /// no server has anywhere to put it.
    pub async fn reorder_playlists(self: Arc<Self>, ids: Vec<i64>) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::reorder_playlists(&db.conn, &ids).map_err(db_err)?;
            self.bump_library();
            Ok(())
        })
        .await
    }

    /// Remember whether this playlist is looked at grouped by album. `None`
    /// follows the app default, which for a playlist is ungrouped: a playlist
    /// is a sequence someone chose, not a shelf of records.
    pub async fn set_playlist_grouped(
        self: Arc<Self>,
        playlist_id: i64,
        grouped: Option<bool>,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            queries::set_playlist_grouped(&db.conn, playlist_id, grouped).map_err(db_err)?;
            self.bump_library();
            Ok(())
        })
        .await
    }

    /// Replace the queue with the playlist and start playing at the entry
    /// `start_entry`, or at the top.
    ///
    /// By entry, not position: the page asking holds its own copy of the
    /// playlist, which a sync may have changed underneath it, and the queue
    /// built here leaves out entries whose track the library has lost. A
    /// position means something different on each side of either.
    ///
    /// `shuffled` orders the queue, not the playlist — the playlist on disk is
    /// untouched.
    pub async fn play_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        start_entry: Option<i64>,
        shuffled: bool,
    ) -> Result<Vec<String>, KoanError> {
        offload::sequenced(move || {
            let db = self.db()?;
            let mut entries = queries::playlist_entries(&db.conn, playlist_id).map_err(db_err)?;
            if shuffled {
                koan_core::helpers::shuffle(&mut entries);
            }
            let track_ids: Vec<i64> = entries.iter().map(|e| e.track.id).collect();

            let mut items = self.build_items(&db, &track_ids);
            // Each queue item remembers the row it came from. The queue is an
            // ephemeral view onto the playlist and may be shuffled, cut about
            // or added to; this is what still says which row is playing, and
            // which of two copies of a song it is.
            //
            // Zipped against the entries whose track actually resolved, not
            // against all of them: a playlist naming a track the library has
            // since lost yields fewer items than entries, and zipping the two
            // directly would hand every entry after the gap the wrong id.
            let resolved: std::collections::HashSet<i64> =
                items.iter().filter_map(|i| i.db_id).collect();
            let kept = entries.iter().filter(|e| resolved.contains(&e.track.id));
            for (item, entry) in items.iter_mut().zip(kept) {
                item.playlist_entry_id = Some(entry.id);
            }
            if items.is_empty() {
                self.send(PlayerCommand::ClearPlaylist)?;
                return Ok(Vec::new());
            }

            // The entry asked for, or if its track is gone, the next one that
            // is still there.
            let start = match start_entry.filter(|_| !shuffled) {
                Some(entry) => entries
                    .iter()
                    .skip_while(|e| e.id != entry)
                    .find_map(|e| items.iter().position(|i| i.playlist_entry_id == Some(e.id)))
                    .unwrap_or(0),
                None => 0,
            };
            let ids: Vec<String> = items.iter().map(|i| i.id.0.to_string()).collect();
            self.send(PlayerCommand::ReplacePlaylist {
                items,
                start,
                position_ms: 0,
                play: true,
            })?;

            Ok(ids)
        })
        .await
    }

    /// Write the playlist out as an extended M3U8.
    ///
    /// Only tracks with a file on this machine can go in it: a playlist file is
    /// a list of paths, and writing stream URLs instead would put the
    /// credentials that authorise them into a file people mail to each other.
    pub async fn export_playlist(
        self: Arc<Self>,
        playlist_id: i64,
        dest_path: String,
    ) -> Result<PlaylistExport, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let summary =
                koan_core::playlists::export_m3u8(&db, playlist_id, Path::new(&dest_path))
                    .map_err(|e| KoanError::BadArgument {
                        message: e.to_string(),
                    })?;
            Ok(PlaylistExport {
                written: summary.written as u32,
                skipped: summary.skipped as u32,
            })
        })
        .await
    }

    // --- Session persistence -----------------------------------------------

    /// Write the queue and position so the next launch can pick them up.
    /// Call it on quit; it is cheap enough to call on a timer too.
    ///
    /// The queue is written only when its contents changed since the last
    /// save. A cursor moving or a download landing changes the queue a client
    /// sees, not what is saved of it, so those save the position alone.
    pub async fn save_session(self: Arc<Self>) -> Result<(), KoanError> {
        use std::sync::atomic::Ordering;
        offload::offload(move || {
            let db = self.db()?;
            // Read before the snapshot: an edit landing in between moves the
            // version again, and the next save writes it.
            let content = self.state.content_version();
            if self.saved_content.load(Ordering::Acquire) == content {
                return self.write_position(&db);
            }
            let (items, cursor) = self.state.snapshot_playlist();
            let persisted: Vec<PersistedQueueItem> = items
                .iter()
                .map(PersistedQueueItem::from_playlist_item)
                .collect();
            let cursor_path = cursor.and_then(|cid| {
                items
                    .iter()
                    .find(|i| i.id == cid)
                    .map(|i| i.path.to_string_lossy().into_owned())
            });
            queries::save_playback_state(
                &db.conn,
                &persisted,
                self.state.play_mode(),
                cursor_path.as_deref(),
                self.state.position_ms(),
                self.state.playback_state() == PlaybackState::Playing,
            )
            .map_err(fav_err)?;
            self.saved_content.store(content, Ordering::Release);
            Ok(())
        })
        .await
    }

    /// Persist where you are, without rewriting the queue.
    ///
    /// Cheap enough to call every second, which is what makes a crash cost a
    /// second of playback rather than the whole session. `save_session` still
    /// runs when the queue changes and on quit.
    pub async fn save_position(self: Arc<Self>) -> Result<(), KoanError> {
        offload::offload(move || self.write_position(&*self.db()?)).await
    }

    /// Whether this device keeps its queue in the account's play queue on the
    /// server, and what the server holds now, for asking before turning it on:
    /// turning it on replaces this device's queue with that one.
    pub async fn server_queue(self: Arc<Self>) -> Result<ServerQueue, KoanError> {
        offload::offload(move || {
            let saved = server_queue::saved()?;
            Ok(ServerQueue {
                on: Config::cached().remote.play_queue,
                saved_tracks: saved.as_ref().map_or(0, |q| q.entry.len() as u32),
                saved_by: saved.map(|q| saved_by(&q.changed_by)).unwrap_or_default(),
            })
        })
        .await
    }

    /// Keep this device's queue in the account's play queue on the server, or
    /// stop. On, the server's queue replaces this device's, or this device's
    /// is saved there when the server has none; off leaves both as they are.
    pub async fn set_server_queue(self: Arc<Self>, on: bool) -> Result<(), KoanError> {
        offload::offload(move || {
            if on {
                match server_queue::saved()? {
                    Some(queue) => self.load_server_queue(&queue, None)?,
                    None => self.save_server_queue()?,
                }
            }
            Config::persist(|cfg| cfg.remote.play_queue = on).map_err(|e| {
                KoanError::BadArgument {
                    message: e.to_string(),
                }
            })?;
            if on {
                server_queue::start(Arc::downgrade(&self), false);
            } else {
                server_queue::stop();
            }
            Ok(())
        })
        .await
    }

    /// Save the queue and playhead to the server now, if this device keeps
    /// them there: the app is going to the background or quitting.
    pub async fn save_server_queue_now(self: Arc<Self>) {
        offload::offload(move || {
            if Config::cached().remote.play_queue
                && let Err(e) = self.save_server_queue()
            {
                log::info!("server queue: not saved: {e}");
            }
        })
        .await
    }

    /// Restore the queue saved by `save_session`, cursor and position included.
    ///
    /// Resumes only if playback was running when the session was saved: closing
    /// a player mid-track and having it pick up where it left off is the point,
    /// while a player that was paused should stay paused rather than start
    /// making noise at whoever opened it.
    ///
    /// Returns the number of items restored.
    pub async fn restore_session(self: Arc<Self>) -> Result<u32, KoanError> {
        // Once this device's own queue is back, the server's may take its
        // place: see `server_queue`.
        let engine = Arc::downgrade(&self);
        let restoring = self.clone();
        let restored = offload::sequenced(move || {
            let db = self.db()?;
            // Before the queue and whether there is one: the mode is the
            // player's, and a queue added under it would be shuffled again.
            let mode = queries::load_play_mode(&db.conn).map_err(fav_err)?;
            self.send_local(PlayerCommand::RestorePlayMode(mode))?;
            let Some(saved) = queries::load_playback_state(&db.conn).map_err(fav_err)? else {
                return Ok(0);
            };

            // Controlling another device, as the last run left it: this one's
            // queue comes back, but not playing over that device's.
            let resume = saved.was_playing && koan_core::remote::devices::target().is_none();

            let items = restore_items(&db, &saved.items);
            if items.is_empty() {
                return Ok(0);
            }

            let count = items.len() as u32;
            // Found among the saved items, not the restored ones: re-resolving
            // can give an item a new path, and `restore_items` keeps one item
            // per saved entry, in order.
            let cursor = saved
                .cursor_path
                .as_ref()
                .and_then(|cp| saved.items.iter().position(|i| &i.path == cp))
                .and_then(|ix| items.get(ix))
                .map(|i| i.id);

            self.send_local(PlayerCommand::AddToPlaylist(items))?;

            if let Some(id) = cursor {
                // The player waits for a track still downloading, and opens it
                // at the position once it can.
                if saved.position_ms > 0 || resume {
                    self.send_local(PlayerCommand::Cue {
                        id,
                        position_ms: saved.position_ms,
                        play: resume,
                    })?;
                } else {
                    self.state.set_cursor(Some(id));
                }
            }

            Ok(count)
        })
        .await;
        if Config::cached().remote.play_queue {
            // Once the player has applied the restore, cue and all, so the
            // server's queue is weighed against the restored one.
            offload::offload(move || {
                if !restoring.applied(server_queue::LANDED) {
                    log::warn!("server queue: the restore did not land; not following");
                    return restored;
                }
                drop(restoring);
                server_queue::start(engine, true);
                restored
            })
            .await
        } else {
            restored
        }
    }

    // --- Output device -----------------------------------------------------

    pub async fn devices(self: Arc<Self>) -> Result<Vec<Device>, KoanError> {
        offload::offload(move || {
            let devices =
                koan_core::audio::list_output_devices().map_err(|e| KoanError::Audio {
                    message: e.to_string(),
                })?;
            Ok(devices
                .into_iter()
                .map(|d| Device {
                    name: d.name,
                    sample_rates: d.sample_rates,
                    kind: d.kind.as_str().to_string(),
                })
                .collect())
        })
        .await
    }

    pub async fn set_device(self: Arc<Self>, name: String) -> Result<(), KoanError> {
        // A renderer still opening for an earlier pick is not used.
        koan_core::upnp::choose();
        offload::sequenced(move || self.send_local(PlayerCommand::SetOutputDevice(name))).await
    }

    /// Albums, artists and tracks deleted since `after` (a `seq` this returned
    /// before; 0 the first time), for a cache keyed by their ids: SQLite reuses
    /// a freed id, and art cached under it would show for another record. Also
    /// those under a folder the library watcher rescanned, whose cover image
    /// beside the tracks may have changed.
    pub async fn art_evictions(self: Arc<Self>, after: i64) -> Result<ArtEvictions, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let rows: Vec<(i64, String, i64)> = db
                .conn
                .prepare("SELECT seq, kind, id FROM art_evictions WHERE seq > ?1 ORDER BY seq")
                .and_then(|mut s| {
                    s.query_map([after], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                        .collect()
                })
                .map_err(db_err)?;
            let mut out = ArtEvictions {
                seq: rows.last().map_or(after, |r| r.0),
                ..Default::default()
            };
            for (_, kind, id) in rows {
                match kind.as_str() {
                    "album" => out.albums.push(id),
                    "artist" => out.artists.push(id),
                    _ => out.tracks.push(id),
                }
            }
            Ok(out)
        })
        .await
    }

    /// Rebuild the audio output where playback is, keeping it paused if it
    /// was: after iOS stops the output for an interruption, the old unit will
    /// not start again.
    pub async fn restart_output(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || self.send_local(PlayerCommand::RestartOutput)).await
    }

    pub async fn clear_device(self: Arc<Self>) -> Result<(), KoanError> {
        koan_core::upnp::choose();
        offload::sequenced(move || self.send_local(PlayerCommand::ClearOutputDevice)).await
    }

    /// The device name persisted in config, or `None` for the system default.
    /// Read from config rather than the player so it survives a restart.
    pub async fn current_device(self: Arc<Self>) -> Option<String> {
        offload::offload(move || Config::cached().playback.output_device.clone()).await
    }

    // --- Devices -----------------------------------------------------------

    /// Forget the device `id`, which is out of reach: out of the list until it
    /// is heard from again. One of this account's is forgotten by the server
    /// too, with its push token, and drops off the account's other devices.
    pub async fn forget_device(self: Arc<Self>, id: String) -> Result<(), KoanError> {
        offload::offload(move || {
            koan_core::remote::devices::forget(&id).map_err(|message| KoanError::Remote { message })
        })
        .await
    }

    /// Control the device `id`, or this one with `None`. Picking a device is
    /// picking where music plays: this one pauses, and the transport, the
    /// queue and what is playing all show that device until another is
    /// picked. Nothing moves; see `move_music`.
    pub async fn control_device(self: Arc<Self>, id: Option<String>) -> Result<(), KoanError> {
        offload::sequenced(move || {
            if let Some(id) = &id {
                koan_core::remote::devices::choosable(id)
                    .map_err(|message| KoanError::Remote { message })?;
            }
            if id.is_some() && self.state.playback_state() == PlaybackState::Playing {
                self.send_local(PlayerCommand::Pause)?;
            }
            koan_core::remote::devices::set_target(id.clone());
            // Asleep: wake it, and let the row say how that is going.
            if let Some(id) = id {
                koan_core::remote::devices::wake(&id);
            }
            Ok(())
        })
        .await
    }

    /// Move the music of the device being controlled to `to` (this device
    /// with `None`): its queue and playhead go there, it pauses, and `to` is
    /// controlled from then on. Says how many tracks were left out because
    /// the server does not have them, which only this device can say, and
    /// whether `to` has said it has the music.
    pub async fn move_music(self: Arc<Self>, to: Option<String>) -> Result<MoveResult, KoanError> {
        use koan_core::remote::{devices, link::LinkCommand};
        let engine = self.clone();
        // Sent on the lane, in order with every other command; waited for off
        // it, so the transport does not stall behind a device that is slow to
        // answer.
        let sent = offload::sequenced(move || {
            let from = devices::target();
            if from == to {
                return Ok(None);
            }
            let sent = match (&from, &to) {
                (None, Some(to)) => Some(
                    engine
                        .hand_off_blocking(to, koan_core::remote::link::CommandSource::Account)?,
                ),
                (Some(from), to) => {
                    let track = devices::current_track(from);
                    let to_id = match to {
                        Some(to) => to.clone(),
                        None => devices::this_id().ok_or_else(|| KoanError::Remote {
                            message: "this device is not set up to be reached".into(),
                        })?,
                    };
                    let before = to.as_deref().and_then(devices::last_report);
                    let cursor_before = engine.state.cursor();
                    let (then, answer) = outcome_channel();
                    devices::send_then(from, LinkCommand::HandOff { to: to_id }, Some(then))
                        .map_err(|message| KoanError::Remote { message })?;
                    Some(HandedOff {
                        left_out: 0,
                        to: to.clone(),
                        track,
                        before,
                        cursor_before,
                        answer: Some(answer),
                        answer_is_arrival: false,
                        was_playing: false,
                    })
                }
                (None, None) => None,
            };
            devices::set_target(to);
            Ok(sent)
        })
        .await?;
        let Some(sent) = sent else {
            return Ok(MoveResult {
                left_out: 0,
                started: true,
                queued: false,
                error: None,
            });
        };
        Ok(offload::offload(move || {
            sent.result(
                || self.under_cursor(),
                || {
                    let _ = self.send_local(PlayerCommand::Resume);
                },
            )
        })
        .await)
    }

    /// Send a link command, as JSON, to the device `id`. For a Live
    /// Activity's buttons, which run while iOS keeps the app's link down: it
    /// goes by the server first, which knows whether the device is linked.
    pub async fn command_device(
        self: Arc<Self>,
        id: String,
        command: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let cmd = koan_core::remote::link::parse_command(&command)
                .map_err(|message| KoanError::BadArgument { message })?;
            koan_core::remote::devices::send_by_server_first(&id, cmd)
                .map_err(|message| KoanError::Remote { message })
        })
        .await
    }

    // --- Renderers ---------------------------------------------------------

    /// Look for UPnP renderers on the network. They arrive in the
    /// `Renderers` slice over the next couple of seconds, and the list follows
    /// the network from then on.
    pub fn search_renderers(&self) {
        koan_core::upnp::discovery::search();
    }

    /// Play to the renderer `udn` in place of this device's own output, or
    /// back here with `None`. The music carries on from where it is. A
    /// renderer is this device's output rather than a device to control, so
    /// picking one stops controlling another koan.
    ///
    /// Not on the ordered lane: opening a session is a few round trips to the
    /// renderer, and app commands queued behind it would wait them out. The
    /// player takes the switch in its own order when it arrives.
    pub async fn play_to_renderer(self: Arc<Self>, udn: Option<String>) -> Result<(), KoanError> {
        // Taken now, in the order the person picked: see `upnp::choose`.
        let choice = koan_core::upnp::choose();
        offload::offload(move || match udn {
            Some(udn) => {
                koan_core::remote::devices::set_target(None);
                koan_core::upnp::connect(&udn, choice, &self.tx)
                    .map_err(|message| KoanError::Audio { message })
            }
            None => {
                koan_core::upnp::disconnect(&self.tx);
                Ok(())
            }
        })
        .await
    }

    /// The app is quitting: stop the renderer playing, if one is, waiting at
    /// most a second and a half for it. Blocks, on purpose: termination does
    /// not wait for a task. Call it after the last save, which records the
    /// session as playing, so the next launch carries on there.
    pub fn release_output(&self) {
        koan_core::player::commands::release_renderer(
            &self.tx,
            std::time::Duration::from_millis(1500),
        );
    }

    /// Set the volume of the renderer being played to, 0–100.
    pub async fn set_renderer_volume(self: Arc<Self>, volume: u8) -> Result<(), KoanError> {
        offload::sequenced(move || self.send_local(PlayerCommand::SetRendererVolume(volume))).await
    }

    /// Play the device in view through `output`: this one, or the device
    /// being controlled, which switches as its own menu would. The music
    /// carries on from where it is.
    ///
    /// Not on the ordered lane: opening a renderer session is a few round
    /// trips, as for `play_to_renderer`.
    pub async fn select_output(self: Arc<Self>, output: OutputChoice) -> Result<(), KoanError> {
        // Taken now, in the order the person picked: see `upnp::choose`.
        let choice = koan_core::upnp::choose();
        match (koan_core::remote::devices::target(), output) {
            // In order with every other command for that device; it takes
            // its own choice when it switches.
            (Some(to), output) => {
                offload::sequenced(move || {
                    self.command_target(
                        &to,
                        koan_core::remote::link::LinkCommand::SetOutput {
                            output: output.into(),
                        },
                    )
                })
                .await
            }
            (None, OutputChoice::Renderer { udn }) => {
                offload::offload(move || {
                    koan_core::upnp::connect(&udn, choice, &self.tx)
                        .map_err(|message| KoanError::Audio { message })
                })
                .await
            }
            (None, output) => {
                offload::sequenced(move || {
                    koan_core::remote::outputs::set(output.into(), choice, &self.tx)
                        .map_err(|message| KoanError::Audio { message })
                })
                .await
            }
        }
    }

    /// List this device's audio devices again, when the system says they
    /// changed or an output menu opens. The `Outputs` slice follows.
    pub async fn refresh_outputs(self: Arc<Self>) {
        offload::offload(koan_core::remote::outputs::refresh_devices).await
    }

    /// Ask the device being controlled to list its outputs again: its output
    /// menu was opened here. Over a route that is up now or not at all, since
    /// a device that is away has nothing new to say and is not worth waking.
    /// Its link state brings back whatever moved.
    pub async fn refresh_controlled_outputs(self: Arc<Self>) {
        offload::offload(|| {
            if let Some(to) = koan_core::remote::devices::target() {
                let cmd = koan_core::remote::link::LinkCommand::RefreshOutputs;
                koan_core::remote::devices::send_live(&to, cmd);
            }
        })
        .await
    }

    /// The volume of the renderer the device in view plays to, 0–100.
    pub async fn set_output_volume(self: Arc<Self>, volume: u8) -> Result<(), KoanError> {
        offload::sequenced(move || match koan_core::remote::devices::target() {
            Some(to) => self.command_target(
                &to,
                koan_core::remote::link::LinkCommand::SetRendererVolume { volume },
            ),
            None => self.send_local(PlayerCommand::SetRendererVolume(volume)),
        })
        .await
    }

    /// Play the output `device` of the device in view through `profile`, or
    /// untouched with `None`. A renderer is named by its UDN.
    pub async fn set_output_preset(
        self: Arc<Self>,
        device: String,
        profile: Option<String>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || match koan_core::remote::devices::target() {
            Some(to) => self.command_target(
                &to,
                koan_core::remote::link::LinkCommand::SetPreset { device, profile },
            ),
            None => self.assign_dsp(profile, &device),
        })
        .await
    }

    /// Where the server should push updates to this app's Live Activity
    /// showing the device `device`; `None` once it has ended.
    pub fn set_live_activity(&self, token: Option<String>, device: Option<String>, sandbox: bool) {
        koan_core::remote::link::set_activity(token.zip(device).map(|(t, d)| (t, d, sandbox)));
    }

    // --- DSP ---------------------------------------------------------------

    pub async fn dsp_overview(self: Arc<Self>) -> DspOverview {
        let device = self.dsp_device();
        self.dsp_overview_for(device).await
    }

    /// The profiles, and what `device` plays: the output in use with `None`.
    /// A renderer is named by its UDN.
    pub async fn dsp_overview_for(self: Arc<Self>, device: Option<String>) -> DspOverview {
        offload::offload(move || {
            let device = device.or_else(|| self.dsp_device());
            let mut overview: DspOverview =
                koan_core::audio::dsp::profiles::overview_for(device).into();
            let playing = self.state.renderer();
            overview.names = overview
                .profiles
                .iter()
                .flat_map(|p| p.devices.iter())
                .chain(overview.device.iter())
                .filter_map(|udn| {
                    let name = match &playing {
                        Some(r) if &r.udn == udn => r.name.clone(),
                        _ => koan_core::upnp::discovery::find(udn)?.name,
                    };
                    Some((udn.clone(), name))
                })
                .collect();
            overview
        })
        .await
    }

    /// Import files, folders or zips as one profile and apply it. `name`
    /// replaces the one taken from the files; `rate` is for coefficients with
    /// none of their own, asked for after a `NeedsSampleRate`. Returns the
    /// profile's name.
    pub async fn dsp_import(
        self: Arc<Self>,
        paths: Vec<String>,
        name: Option<String>,
        rate: Option<u32>,
    ) -> Result<String, KoanError> {
        offload::sequenced(move || {
            let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
            let imported =
                koan_core::audio::dsp::import::import(&paths, rate).map_err(dsp_error)?;
            self.save_dsp(imported, name)
        })
        .await
    }

    /// What importing `paths` will do: a group of presets, or one profile
    /// combined from them all, with a name to suggest.
    pub async fn dsp_import_plan(self: Arc<Self>, paths: Vec<String>) -> DspImportPlan {
        offload::offload(move || {
            let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
            let plan = koan_core::audio::dsp::import::plan(&paths);
            DspImportPlan {
                group: plan.group,
                files: plan.files,
                name: plan.name,
            }
        })
        .await
    }

    /// Import a selection of files. Whole presets become a profile each and
    /// a group of them called `name`, the first playing; parts of one
    /// profile combine into one called `name`. A file refused does not stop
    /// the others; each refusal is reported, and logged.
    pub async fn dsp_import_files(
        self: Arc<Self>,
        paths: Vec<String>,
        name: Option<String>,
        rate: Option<u32>,
    ) -> Result<DspImportSummary, KoanError> {
        offload::sequenced(move || {
            use koan_core::audio::dsp::{
                import::{self, Batch, Outcome},
                profiles,
            };
            let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
            let taken: Vec<String> = koan_core::config::Config::cached()
                .dsp
                .profiles
                .iter()
                .map(|p| p.name.clone())
                .collect();
            let batch = import::import_batch(&paths, rate, &taken).map_err(|e| {
                log::warn!("dsp import: {e}");
                dsp_error(e)
            })?;
            let mut summary = DspImportSummary {
                imported: Vec::new(),
                refused: Vec::new(),
                notes: Vec::new(),
                group: None,
            };
            match batch {
                Batch::One(imported) => {
                    // Never written over: a name already taken is numbered.
                    let wanted = name
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| imported.name.clone());
                    let free = profiles::free_name(&wanted);
                    if free != wanted {
                        summary
                            .notes
                            .push(format!("{wanted} is taken; imported as {free}"));
                    }
                    summary.imported.push(self.save_dsp(imported, Some(free))?)
                }
                Batch::Group(each) => {
                    for item in each {
                        let file = item.file;
                        let saved = match item.outcome {
                            Outcome::Skipped(why) => {
                                summary.notes.push(format!("{file} left out: {why}"));
                                continue;
                            }
                            Outcome::Refused(e) => Err(e.to_string()),
                            Outcome::Imported(i, note) => {
                                summary.notes.extend(note.map(|n| format!("{file}: {n}")));
                                self.save_dsp(i, None).map_err(|e| e.to_string())
                            }
                        };
                        match saved {
                            Ok(name) => summary.imported.push(name),
                            Err(reason) => {
                                log::warn!("dsp import: {file}: {reason}");
                                summary.refused.push(DspImportRefusal { file, reason });
                            }
                        }
                    }
                    if summary.imported.len() > 1 {
                        let wanted = name
                            .filter(|n| !n.trim().is_empty())
                            .unwrap_or_else(|| import::group_name(&summary.imported));
                        let group = profiles::free_name(&wanted);
                        if group != wanted {
                            summary
                                .notes
                                .push(format!("{wanted} is taken; the group is {group}"));
                        }
                        profiles::make_group(&group, &summary.imported)
                            .map_err(|message| KoanError::BadArgument { message })?;
                        self.send_local(PlayerCommand::ReloadDsp)?;
                        summary.group = Some(group);
                    }
                }
            }
            Ok(summary)
        })
        .await
    }

    /// Play `member` of the group `group`, and none of the others.
    pub async fn dsp_select(
        self: Arc<Self>,
        group: String,
        member: String,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::select(&group, &member)
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// Make `name` a group, one layer playing, or a stack of layers.
    pub async fn dsp_set_group(
        self: Arc<Self>,
        name: String,
        group: bool,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::set_group(&name, group)
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// Import text: shared from another app, or pasted.
    pub async fn dsp_import_text(
        self: Arc<Self>,
        text: String,
        name: Option<String>,
        rate: Option<u32>,
    ) -> Result<String, KoanError> {
        offload::sequenced(move || {
            let imported =
                koan_core::audio::dsp::import::import_text(&text, rate).map_err(dsp_error)?;
            self.save_dsp(imported, name)
        })
        .await
    }

    /// AutoEQ's results whose names match `query`, best first. The first
    /// search of the day may fetch the index.
    pub async fn autoeq_search(
        self: Arc<Self>,
        query: String,
        limit: u32,
    ) -> Result<Vec<AutoEqEntry>, KoanError> {
        offload::offload(move || {
            use koan_core::audio::dsp::autoeq;
            let entries = autoeq::index(autoeq::Freshness::Daily)
                .map_err(|message| KoanError::Remote { message })?;
            Ok(autoeq::search(&entries, &query, limit as usize)
                .into_iter()
                .map(Into::into)
                .collect())
        })
        .await
    }

    /// The makers in AutoEQ's index, alphabetically, with how many results
    /// each has.
    pub async fn autoeq_makers(self: Arc<Self>) -> Result<Vec<AutoEqMaker>, KoanError> {
        offload::offload(move || {
            use koan_core::audio::dsp::autoeq;
            let entries = autoeq::index(autoeq::Freshness::Daily)
                .map_err(|message| KoanError::Remote { message })?;
            Ok(autoeq::makers(&entries)
                .into_iter()
                .map(|(name, results)| AutoEqMaker {
                    name,
                    results: results as u32,
                })
                .collect())
        })
        .await
    }

    /// `maker`'s results, by model, AutoEQ's preferred source first within
    /// each.
    pub async fn autoeq_models(
        self: Arc<Self>,
        maker: String,
    ) -> Result<Vec<AutoEqEntry>, KoanError> {
        offload::offload(move || {
            use koan_core::audio::dsp::autoeq;
            let entries = autoeq::index(autoeq::Freshness::Daily)
                .map_err(|message| KoanError::Remote { message })?;
            Ok(autoeq::models(&entries, &maker)
                .into_iter()
                .map(Into::into)
                .collect())
        })
        .await
    }

    /// Install the AutoEQ result `name`, measured by `measured_by`, as a
    /// profile, and play `device` through it if given. Answers with the
    /// profile's name.
    pub async fn autoeq_install(
        self: Arc<Self>,
        name: String,
        measured_by: String,
        device: Option<String>,
    ) -> Result<String, KoanError> {
        // The download can take as long as GitHub does, so it stays off the
        // lane transport commands queue on; only the assignment goes there.
        let profile = offload::offload(move || {
            use koan_core::audio::dsp::autoeq;
            let entries = autoeq::index(autoeq::Freshness::Kept)
                .map_err(|message| KoanError::Remote { message })?;
            let entry = autoeq::find(&entries, &name, Some(&measured_by)).ok_or_else(|| {
                KoanError::NotFound {
                    message: format!("{name} is no longer in AutoEQ's index"),
                }
            })?;
            autoeq::install(entry).map_err(|message| KoanError::Remote { message })
        })
        .await?;
        offload::sequenced(move || {
            match device {
                Some(device) => self.assign_dsp(Some(profile.clone()), &device)?,
                None => self.send_local(PlayerCommand::ReloadDsp)?,
            }
            Ok(profile)
        })
        .await
    }

    /// The AutoEQ result the output in use is, by its name, while it has no
    /// profile and the suggestion has not been turned down. `None` for a
    /// renderer, whose name is the user's to choose.
    pub async fn autoeq_suggestion(self: Arc<Self>) -> Option<AutoEqOffer> {
        use koan_core::audio::dsp::autoeq::{self, Offer};
        offload::offload(move || {
            if self.state.renderer().is_some() {
                return None;
            }
            let device = koan_core::audio::dsp::profiles::current_device()?;
            Some(match autoeq::suggestion(&device).ok().flatten()? {
                Offer::Profile(e) => AutoEqOffer::Profile { entry: (&e).into() },
                Offer::Search(query) => AutoEqOffer::Search { query },
            })
        })
        .await
    }

    /// Stop suggesting an AutoEQ profile for the output in use.
    pub async fn autoeq_dismiss(self: Arc<Self>) -> Result<(), KoanError> {
        offload::sequenced(move || {
            let device = self.dsp_device().ok_or(KoanError::Audio {
                message: "no output device".into(),
            })?;
            koan_core::audio::dsp::autoeq::dismiss(&device)
                .map_err(|message| KoanError::BadArgument { message })
        })
        .await
    }

    /// The targets `name`'s correction can be moved to, for one installed
    /// from AutoEQ whose target is known.
    pub async fn dsp_targets(self: Arc<Self>, name: String) -> Option<DspTargets> {
        offload::offload(move || {
            let t = koan_core::audio::dsp::profiles::target_choices(&name)?;
            Some(DspTargets {
                made_for: t.made_for.map(|m| DspTargetOption {
                    id: m.id.into(),
                    name: m.name.into(),
                    does: m.does.into(),
                    character: m.character.into(),
                }),
                chosen: t.chosen,
                choices: t.choices.into_iter().map(Into::into).collect(),
            })
        })
        .await
    }

    /// Say what `name` is for. A second correction in a chain is refused,
    /// naming the first.
    pub async fn dsp_set_role(
        self: Arc<Self>,
        name: String,
        role: crate::types::DspRole,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::set_role(&name, role.into())
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// The target a ready-made EQ was made for, by id, or `None` when it is
    /// not known, which leaves target switching off.
    pub async fn dsp_set_made_for(
        self: Arc<Self>,
        name: String,
        target: Option<String>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::set_made_for(&name, target.as_deref())
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// The targets for a kind of headphone, in-ear or over-ear, with what
    /// each sounds like: shipped ones, then those added.
    pub async fn dsp_targets_for(self: Arc<Self>, in_ear: bool) -> Vec<DspTargetOption> {
        offload::offload(move || {
            use koan_core::audio::dsp::targets::{self, Ear};
            let ear = if in_ear { Ear::In } else { Ear::Over };
            let mut out: Vec<DspTargetOption> = targets::TARGETS
                .iter()
                .filter(|t| t.ear == ear)
                .map(|t| DspTargetOption {
                    id: t.id.into(),
                    name: t.name.into(),
                    does: t.does.into(),
                    character: t.character.into(),
                })
                .collect();
            out.extend(targets::added().into_iter().map(|a| DspTargetOption {
                id: a.id,
                name: a.name,
                does: String::new(),
                character: String::new(),
            }));
            out
        })
        .await
    }

    /// What correcting the measurement in `text` to `target` would do, before
    /// it is saved: the measurement, the target, the predicted response and
    /// the EQ itself.
    pub async fn dsp_preview_measurement(
        self: Arc<Self>,
        text: String,
        target: String,
    ) -> Result<DspResponse, KoanError> {
        offload::offload(move || {
            koan_core::audio::dsp::profiles::preview_measurement(&text, &target, 48000)
                .map(Into::into)
                .map_err(|message| KoanError::BadArgument { message })
        })
        .await
    }

    /// Measurements on squig.link sites whose name has each word of `query`,
    /// best first.
    pub async fn dsp_squig_search(
        self: Arc<Self>,
        query: String,
    ) -> Result<Vec<SquigHit>, KoanError> {
        offload::offload(move || {
            koan_core::audio::dsp::squig::search(&query, 60)
                .map(|hits| hits.into_iter().map(Into::into).collect())
                .map_err(|message| KoanError::BadArgument { message })
        })
        .await
    }

    /// The measurement `file` on the squig.link site `site`, its channels
    /// averaged, as frequency and level text for `dspSaveMeasured`.
    pub async fn dsp_squig_fetch(
        self: Arc<Self>,
        site: String,
        file: String,
    ) -> Result<String, KoanError> {
        offload::offload(move || {
            use koan_core::audio::dsp::squig;
            let site = squig::site(&site).ok_or(KoanError::BadArgument {
                message: format!("{site} is not a squig.link site koan searches"),
            })?;
            let hit = squig::Hit {
                site,
                brand: String::new(),
                model: file.clone(),
                variant: String::new(),
                file,
            };
            squig::fetch(&hit).map_err(|message| KoanError::BadArgument { message })
        })
        .await
    }

    /// Save the headphone `name`, measured as `text`, corrected to `target`,
    /// crediting `source` where the measurement came from one.
    pub async fn dsp_save_measured(
        self: Arc<Self>,
        name: String,
        text: String,
        in_ear: bool,
        target: String,
        source: Option<String>,
    ) -> Result<String, KoanError> {
        offload::sequenced(move || {
            use koan_core::config::DspEar;
            let ear = if in_ear { DspEar::In } else { DspEar::Over };
            let name = koan_core::audio::dsp::profiles::save_measured_from(
                &name,
                &text,
                ear,
                &target,
                source.as_deref(),
            )
            .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)?;
            Ok(name)
        })
        .await
    }

    /// What splitting the baked EQ `name` would give, with the headphones'
    /// measurement `text` and `target` as neutral: the correction, the
    /// tuning, their sum, and the EQ itself as `original`.
    pub async fn dsp_preview_split(
        self: Arc<Self>,
        name: String,
        text: String,
        target: String,
    ) -> Result<DspResponse, KoanError> {
        offload::offload(move || {
            koan_core::audio::dsp::profiles::preview_split(&name, &text, &target, 48000)
                .map(Into::into)
                .map_err(|message| KoanError::BadArgument { message })
        })
        .await
    }

    /// Split the baked EQ `name` into a correction from the headphones'
    /// measurement and a tuning made against `target`; the outputs that
    /// played it play the two. The new profiles' names, correction first.
    pub async fn dsp_split_baked(
        self: Arc<Self>,
        name: String,
        text: String,
        in_ear: bool,
        target: String,
    ) -> Result<Vec<String>, KoanError> {
        offload::sequenced(move || {
            use koan_core::config::DspEar;
            let ear = if in_ear { DspEar::In } else { DspEar::Over };
            let (correction, tuning) =
                koan_core::audio::dsp::profiles::split_baked(&name, &text, ear, &target)
                    .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)?;
            Ok(vec![correction, tuning])
        })
        .await
    }

    /// Move `name`'s correction to the target `id`, or with `None` back to the
    /// one it was made for.
    pub async fn dsp_choose_target(
        self: Arc<Self>,
        name: String,
        id: Option<String>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::choose_target(&name, id.as_deref())
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// Add a target from a CSV of frequency and level, or a squig.link
    /// export, to choose from for every correction of its kind.
    pub async fn dsp_add_target(
        self: Arc<Self>,
        path: String,
    ) -> Result<DspTargetOption, KoanError> {
        offload::offload(move || {
            let added = koan_core::audio::dsp::targets::add(std::path::Path::new(&path))
                .map_err(|message| KoanError::BadArgument { message })?;
            Ok(DspTargetOption {
                id: added.id,
                name: added.name,
                does: String::new(),
                character: String::new(),
            })
        })
        .await
    }

    /// Make `name` a stack of `layers`, in order, creating it if there is
    /// none. Refused where it could not play.
    pub async fn dsp_set_layers(
        self: Arc<Self>,
        name: String,
        layers: Vec<DspLayerInfo>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            let layers = layers
                .into_iter()
                .map(|l| koan_core::config::DspLayer {
                    profile: l.profile,
                    on: l.on,
                })
                .collect();
            koan_core::audio::dsp::profiles::set_layers(&name, layers)
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// Set filter `index` of `name`, a parametric band, to `kind` at `freq`,
    /// `gain_db` and `q`, held within the ranges a band may have.
    pub async fn dsp_set_band(
        self: Arc<Self>,
        name: String,
        index: u32,
        kind: String,
        freq: f64,
        gain_db: f64,
        q: f64,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::set_band(
                &name,
                index as usize,
                &kind,
                freq,
                gain_db,
                q,
            )
            .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// Add a flat band at 1 kHz to `name`; its index among the filters.
    pub async fn dsp_add_band(self: Arc<Self>, name: String) -> Result<u32, KoanError> {
        offload::sequenced(move || {
            let index = koan_core::audio::dsp::profiles::add_band(&name)
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)?;
            Ok(index as u32)
        })
        .await
    }

    /// Take filter `index` out of `name`.
    pub async fn dsp_remove_filter(
        self: Arc<Self>,
        name: String,
        index: u32,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::remove_filter(&name, index as usize)
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// What the output in use plays: its correction, and the tuning on top
    /// adjusted to the correction's target.
    pub async fn dsp_output_response(self: Arc<Self>, rate: u32) -> Option<DspResponse> {
        let device = self.dsp_device()?;
        self.dsp_output_response_for(device, rate).await
    }

    /// What `device` plays, whether or not it is the output in use.
    pub async fn dsp_output_response_for(
        self: Arc<Self>,
        device: String,
        rate: u32,
    ) -> Option<DspResponse> {
        offload::offload(move || {
            koan_core::audio::dsp::profiles::output_response(&device, rate).map(Into::into)
        })
        .await
    }

    /// Play `tuning` on top of `device`'s correction, or none. Not a
    /// correction, and not on one with a tuning baked in.
    pub async fn dsp_set_tuning(
        self: Arc<Self>,
        device: String,
        tuning: Option<String>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::set_tuning(&device, tuning.as_deref())
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// Make `device`'s tuning `tuning`: EQs in the order they play, each on
    /// or off.
    pub async fn dsp_set_tunings(
        self: Arc<Self>,
        device: String,
        tuning: Vec<DspTuningEntry>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            let list: Vec<(String, bool)> = tuning.into_iter().map(|t| (t.name, t.on)).collect();
            koan_core::audio::dsp::profiles::set_tunings(&device, &list)
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// Save `device`'s correction and tuning as the preset `name`.
    pub async fn dsp_save_preset(
        self: Arc<Self>,
        device: String,
        name: String,
    ) -> Result<String, KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::save_preset(&device, &name)
                .map_err(|message| KoanError::BadArgument { message })
        })
        .await
    }

    /// Set `device` from the preset `name`, or flat with `None`.
    pub async fn dsp_apply_preset(
        self: Arc<Self>,
        device: String,
        name: Option<String>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::apply_preset(&device, name.as_deref())
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// Put `name` back as it was imported.
    pub async fn dsp_revert(self: Arc<Self>, name: String) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::revert(&name)
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// A copy of `name` as it is now, under `new` or "<name> copy". The
    /// copy's name.
    pub async fn dsp_duplicate(
        self: Arc<Self>,
        name: String,
        new: Option<String>,
    ) -> Result<String, KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::duplicate(&name, new.as_deref())
                .map_err(|message| KoanError::BadArgument { message })
        })
        .await
    }

    /// The target the tuning `name` was made against, or `None` when that is
    /// not known, which plays it as it is on any correction.
    pub async fn dsp_set_tuned_for(
        self: Arc<Self>,
        name: String,
        target: Option<String>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::set_tuned_for(&name, target.as_deref())
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// What `name` does to the sound at `rate`, for drawing. `None` for a
    /// profile that is not there or would not play.
    pub async fn dsp_response(self: Arc<Self>, name: String, rate: u32) -> Option<DspResponse> {
        offload::offload(move || {
            koan_core::audio::dsp::profiles::response(&name, rate).map(Into::into)
        })
        .await
    }

    pub async fn dsp_detail(self: Arc<Self>, name: String) -> Option<DspProfileDetail> {
        offload::offload(move || {
            let mut detail: DspProfileDetail =
                koan_core::audio::dsp::profiles::detail(&name)?.into();
            if let Ok(db) = self.db() {
                if detail.everywhere {
                    detail.sync_problem = koan_core::remote::dsp_sync::refusal(&db, &name);
                }
                detail.sync_note = koan_core::remote::dsp_sync::note(&db, &name);
            }
            Some(detail)
        })
        .await
    }

    /// Keep `name` on every device of the account, or on this one alone.
    pub async fn dsp_set_scope(
        self: Arc<Self>,
        name: String,
        everywhere: bool,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || {
            use koan_core::config::DspScope;
            let to = if everywhere {
                DspScope::Everywhere
            } else {
                DspScope::Device
            };
            koan_core::audio::dsp::profiles::set_scope(&name, to)
                .map_err(|message| KoanError::BadArgument { message })
        })
        .await
    }

    pub async fn dsp_rename(self: Arc<Self>, old: String, new: String) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::rename(&old, &new)
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// Play the current output through `profile`, or untouched with `None`.
    pub async fn dsp_assign(self: Arc<Self>, profile: Option<String>) -> Result<(), KoanError> {
        offload::sequenced(move || {
            let device = self.dsp_device().ok_or(KoanError::Audio {
                message: "no output device".into(),
            })?;
            self.assign_dsp(profile, &device)
        })
        .await
    }

    /// Play `device` through `profile`, or untouched with `None`, whether or
    /// not it is the output in use. A renderer is named by its UDN.
    pub async fn dsp_assign_device(
        self: Arc<Self>,
        device: String,
        profile: Option<String>,
    ) -> Result<(), KoanError> {
        offload::sequenced(move || self.assign_dsp(profile, &device)).await
    }

    pub async fn dsp_remove(self: Arc<Self>, name: String) -> Result<(), KoanError> {
        offload::sequenced(move || {
            koan_core::audio::dsp::profiles::remove(&name)
                .map_err(|message| KoanError::BadArgument { message })?;
            self.send_local(PlayerCommand::ReloadDsp)
        })
        .await
    }

    /// The name of the port iOS routes audio to, on each route change: what
    /// profiles are chosen by on a phone. Does nothing elsewhere.
    pub async fn set_audio_route(self: Arc<Self>, name: String) -> Result<(), KoanError> {
        #[cfg(any(target_os = "ios", target_os = "tvos"))]
        {
            offload::sequenced(move || {
                koan_core::audio::ios_backend::set_route(name);
                self.send_local(PlayerCommand::ReloadDsp)
            })
            .await
        }
        #[cfg(not(any(target_os = "ios", target_os = "tvos")))]
        {
            let _ = name;
            Ok(())
        }
    }

    // --- Settings ----------------------------------------------------------

    /// The kōan theme, or the platform's look. Saved at once; it takes effect
    /// on the next launch, since every view is drawn in the theme read at start.
    pub fn set_theme(&self, koan: bool) {
        let theme = if koan { "koan" } else { "system" };
        if let Err(e) = Config::persist(|cfg| cfg.appearance.theme = theme.into()) {
            log::warn!("appearance: theme not saved: {e}");
        }
    }

    /// Colours from the record, or koan's own. Saved at once; the app redraws
    /// from its own copy.
    pub fn set_record_colours(&self, on: bool) {
        if let Err(e) = Config::persist(|cfg| cfg.appearance.record_colours = on) {
            log::warn!("appearance: record_colours not saved: {e}");
        }
    }

    /// The wash under the whole window, or panels on grounds of their own.
    /// Saved at once; the app redraws from its own copy.
    pub fn set_wash_window(&self, on: bool) {
        if let Err(e) = Config::persist(|cfg| cfg.appearance.wash_window = on) {
            log::warn!("appearance: wash_window not saved: {e}");
        }
    }

    /// Show icons beside labels in the kōan theme, or not. Saved at once;
    /// the app redraws from its own copy.
    pub fn set_theme_icons(&self, on: bool) {
        if let Err(e) = Config::persist(|cfg| cfg.appearance.theme_icons = on) {
            log::warn!("appearance: theme_icons not saved: {e}");
        }
    }

    /// How the app is drawn, from `[appearance]`. Read once, as the app opens:
    /// a change takes effect on the next launch.
    pub fn appearance(&self) -> Appearance {
        let cfg = Config::cached();
        Appearance {
            koan: cfg.appearance.theme == "koan",
            icons: cfg.appearance.theme_icons,
            record_colours: cfg.appearance.record_colours,
            wash_window: cfg.appearance.wash_window,
        }
    }

    /// The whole configuration, as the settings window shows it.
    pub async fn settings(self: Arc<Self>) -> Settings {
        offload::offload(move || {
            let cfg = Config::load().unwrap_or_default();
            let cache_dir = cfg.cache_dir();
            let cache_bytes = koan_core::helpers::cache_size_bytes(&cfg);

            let db = self.db().ok();
            Settings {
                library_folders: cfg
                    .library
                    .folders
                    .iter()
                    .map(|p| LibraryFolder {
                        path: p.to_string_lossy().into_owned(),
                        tracks: db
                            .as_ref()
                            .map(|db| koan_core::helpers::tracks_under(db, p))
                            .unwrap_or(0),
                    })
                    .collect(),

                remote_enabled: cfg.remote.enabled,
                remote_url: cfg.remote.url.clone(),
                remote_username: cfg.remote.username.clone(),
                remote_signed_in: koan_core::helpers::remote_credential(&cfg).is_some(),
                remote_tracks: db
                    .as_ref()
                    .map(|db| koan_core::helpers::tracks_from_server(db))
                    .unwrap_or(0),
                download_workers: cfg.remote.download_workers as u32,
                cache_limit: cfg.remote.cache_limit.clone().unwrap_or_default(),
                cache_dir: cache_dir.to_string_lossy().into_owned(),
                cache_bytes,
                auto_sync: cfg.remote.auto_sync,
                auto_sync_interval_mins: cfg.remote.auto_sync_interval_mins,
                play_queue: cfg.remote.play_queue,

                replaygain: match cfg.playback.replaygain {
                    config::ReplayGainMode::Off => "off".into(),
                    config::ReplayGainMode::Track => "track".into(),
                    config::ReplayGainMode::Album => "album".into(),
                },
                pre_amp_db: cfg.playback.pre_amp_db,
                fade_on_pause: cfg.playback.fade_on_pause,
                devices_discoverable: cfg.devices.discoverable,
                devices_addresses: cfg.devices.addresses.clone(),
                devices_nearby_control: match cfg.devices.nearby_control {
                    config::NearbyControl::Full => "full".into(),
                    config::NearbyControl::Playback => "playback".into(),
                },
                devices_keep_running: cfg.devices.keep_running,
            }
        })
        .await
    }

    /// Write the settings back.
    ///
    /// Each setting lands in the file that owns it — `Config::persist` routes
    /// folders and the server account to `config.local.toml` and taste like
    /// ReplayGain to `config.toml`. The password is not here; it goes through
    /// `sign_in_remote`.
    pub async fn update_settings(self: Arc<Self>, s: Settings) -> Result<(), KoanError> {
        offload::offload(move || {
            let limit_before = Config::cached().remote.cache_limit.clone();
            Config::persist(|cfg| {
                cfg.library.folders = s
                    .library_folders
                    .iter()
                    .map(|f| PathBuf::from(&f.path))
                    .collect();

                cfg.remote.enabled = s.remote_enabled;
                cfg.remote.url = s.remote_url.clone();
                cfg.remote.username = s.remote_username.clone();
                cfg.remote.download_workers = s.download_workers.max(1) as usize;
                cfg.remote.cache_limit = (!s.cache_limit.is_empty()).then(|| s.cache_limit.clone());
                cfg.remote.auto_sync = s.auto_sync;
                cfg.remote.auto_sync_interval_mins = s.auto_sync_interval_mins;

                cfg.playback.replaygain = match s.replaygain.as_str() {
                    "track" => config::ReplayGainMode::Track,
                    "album" => config::ReplayGainMode::Album,
                    _ => config::ReplayGainMode::Off,
                };
                cfg.playback.pre_amp_db = s.pre_amp_db;
                cfg.playback.fade_on_pause = s.fade_on_pause;

                cfg.devices.discoverable = s.devices_discoverable;
                cfg.devices.nearby_control = match s.devices_nearby_control.as_str() {
                    "playback" => config::NearbyControl::Playback,
                    _ => config::NearbyControl::Full,
                };
                cfg.devices.keep_running = s.devices_keep_running;
                cfg.devices.addresses = s
                    .devices_addresses
                    .iter()
                    .map(|a| a.trim().to_string())
                    .filter(|a| !a.is_empty())
                    .collect();
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })?;
            koan_core::remote::nearby::reconfigure();
            if Config::cached().remote.cache_limit != limit_before {
                koan_core::remote::queue::cache_limit_changed();
            }
            Ok(())
        })
        .await
    }

    /// Ask the running library task to stop.
    ///
    /// It stops between transactions and keeps what it had already committed —
    /// a cancelled scan is a shorter scan, not an undone one.
    pub fn cancel_library_task(&self) {
        self.cancel_library_task
            .store(true, std::sync::atomic::Ordering::Relaxed);
        // The folder watcher's scans, which this app did not start.
        koan_core::index::lane::cancel_all();
    }

    /// Sign in to a Subsonic/Navidrome server.
    ///
    /// Checked against the server before anything is written; the credentials
    /// then go to `config.local.toml`.
    pub async fn sign_in_remote(
        self: Arc<Self>,
        url: String,
        username: String,
        password: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            koan_core::helpers::set_remote_credentials(&url, &username, &password).map_err(|e| {
                KoanError::BadArgument {
                    message: e.to_string(),
                }
            })
        })
        .await
    }

    /// Sign in to a koan server with an API key the account already holds.
    pub async fn sign_in_remote_with_key(
        self: Arc<Self>,
        url: String,
        username: String,
        api_key: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            koan_core::helpers::set_remote_api_key(&url, &username, &api_key).map_err(|e| {
                KoanError::BadArgument {
                    message: e.to_string(),
                }
            })
        })
        .await
    }

    /// Join a server with an invite: its token traded for an API key of this
    /// device's own, or the password a pasted address carries. Checked against
    /// the server before anything is written.
    pub async fn join_invite(self: Arc<Self>, invite: Invite) -> Result<(), KoanError> {
        offload::offload(move || {
            let invite = koan_core::invite::Invite {
                server: invite.server,
                username: invite.username,
                token: invite.token,
                password: invite.password,
            };
            koan_core::helpers::join_with_invite(&invite).map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })
        })
        .await
    }

    /// Read an invite: the koan.rocks link, `koan://join`, or a server address
    /// with the account in it. `None` for anything else, so a field can offer
    /// to join only when what was pasted is one.
    pub fn parse_invite(&self, link: String) -> Option<Invite> {
        koan_core::invite::Invite::parse(&link).map(Into::into)
    }

    // -- Pairing: signing in a device without a keyboard (`koanPair`) --

    /// Ask the server at `url` to sign this device in, once someone signed in
    /// elsewhere approves it. The code and link come back to show;
    /// `await_pairing` waits for the answer. A pairing already waiting is
    /// given up.
    pub async fn start_pairing(self: Arc<Self>, url: String) -> Result<PairingCode, KoanError> {
        self.cancel_pairing();
        offload::offload(move || {
            let device =
                koan_core::remote::link::LinkIdentity::this_device(self.device_name.clone()).name;
            let pending = koan_core::remote::pair::start(&url, &device).map_err(pair_error)?;
            let code = PairingCode {
                id: pending.id.clone(),
                code: pending.code.clone(),
                link: pending.link.clone(),
            };
            *self.pairing_cancel.lock() = pending.canceller().map(|c| (pending.id.clone(), c));
            *self.pairing.lock() = Some(pending);
            Ok(code)
        })
        .await
    }

    /// Wait for the pairing `start_pairing` opened to be approved, declined
    /// or to lapse. Approved, the app is signed in.
    pub async fn await_pairing(self: Arc<Self>) -> Result<(), KoanError> {
        offload::offload(move || {
            let pending = self
                .pairing
                .lock()
                .take()
                .ok_or_else(|| KoanError::BadArgument {
                    message: "no pairing is waiting".into(),
                })?;
            let id = pending.id.clone();
            let outcome = pending.wait();
            let mut cancel = self.pairing_cancel.lock();
            if cancel.as_ref().is_some_and(|(held, _)| *held == id) {
                *cancel = None;
            }
            outcome.map_err(pair_error)
        })
        .await
    }

    /// Give up the pairing this device is waiting on.
    pub fn cancel_pairing(&self) {
        if let Some((_, cancel)) = self.pairing_cancel.lock().take() {
            cancel.cancel();
        }
        self.pairing.lock().take();
    }

    /// The device waiting on `pair`, an id or the code it shows, on the
    /// signed-in server, and where it asked from.
    pub async fn pairing_info(self: Arc<Self>, pair: String) -> Result<PairingInfo, KoanError> {
        offload::offload(move || {
            koan_core::remote::pair::info(&pair)
                .map(|p| PairingInfo {
                    device: p.device,
                    from: p.from,
                    local: p.local,
                })
                .map_err(pair_error)
        })
        .await
    }

    /// Sign the device waiting on `pair` in as this account. Answers with its
    /// name.
    pub async fn approve_pairing(self: Arc<Self>, pair: String) -> Result<String, KoanError> {
        offload::offload(move || koan_core::remote::pair::approve(&pair).map_err(pair_error)).await
    }

    pub async fn decline_pairing(self: Arc<Self>, pair: String) -> Result<(), KoanError> {
        offload::offload(move || {
            koan_core::remote::pair::decline(&pair)
                .map(drop)
                .map_err(pair_error)
        })
        .await
    }

    /// The account's ListenBrainz connection on the signed-in koan server.
    /// `None` while it has none.
    pub async fn scrobbling_status(
        self: Arc<Self>,
    ) -> Result<Option<ScrobblingConnection>, KoanError> {
        offload::offload(move || {
            koan_core::remote::scrobbling::status()
                .map(|s| s.map(scrobbling_connection))
                .map_err(scrobbling_error)
        })
        .await
    }

    /// Connect the account's ListenBrainz with its user token. The server
    /// checks it with ListenBrainz; a refusal is a `BadArgument` whose
    /// message can be shown as it is.
    pub async fn connect_scrobbling(
        self: Arc<Self>,
        token: String,
    ) -> Result<Option<ScrobblingConnection>, KoanError> {
        offload::offload(move || {
            koan_core::remote::scrobbling::connect(&token)
                .map(|s| s.map(scrobbling_connection))
                .map_err(scrobbling_error)
        })
        .await
    }

    pub async fn disconnect_scrobbling(self: Arc<Self>) -> Result<(), KoanError> {
        offload::offload(move || {
            koan_core::remote::scrobbling::disconnect().map_err(scrobbling_error)
        })
        .await
    }

    /// Read a pairing link (`koan.rocks/pair/#s=…&p=…`). `None` for anything
    /// else.
    pub fn parse_pairing_link(&self, link: String) -> Option<PairingLink> {
        koan_core::remote::pair::PairLink::parse(&link).map(|l| PairingLink {
            server: l.server,
            id: l.id,
        })
    }

    // -- Accounts on the signed-in server: koan servers, admins only --

    /// The server's accounts. Fails on a server that is not koan, or for an
    /// account that is not an admin, which is how the app knows to offer none
    /// of this.
    pub async fn server_accounts(self: Arc<Self>) -> Result<Vec<ServerAccount>, KoanError> {
        offload::offload(move || {
            Ok(account_client()?
                .koan_users()
                .map_err(remote_error)?
                .into_iter()
                .map(|u| ServerAccount {
                    role: AccountRole::parse(&u.role),
                    username: u.username,
                })
                .collect())
        })
        .await
    }

    /// Make an account with a generated password; its invite comes back.
    pub async fn create_server_account(
        self: Arc<Self>,
        username: String,
        role: AccountRole,
    ) -> Result<Invite, KoanError> {
        offload::offload(move || {
            let client = invite_client()?;
            let made = client
                .koan_create_user(&username, role.as_str())
                .map_err(remote_error)?;
            Ok(account_invite(client.base_url(), made).into())
        })
        .await
    }

    /// An invite for an existing account. `reset` also gives it a new
    /// password, which comes back in the invite, signing its devices out.
    pub async fn invite_server_account(
        self: Arc<Self>,
        username: String,
        reset: bool,
    ) -> Result<Invite, KoanError> {
        offload::offload(move || {
            let client = invite_client()?;
            let made = client.koan_invite(&username, reset).map_err(remote_error)?;
            Ok(account_invite(client.base_url(), made).into())
        })
        .await
    }

    pub async fn set_server_account_role(
        self: Arc<Self>,
        username: String,
        role: AccountRole,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            account_client()?
                .koan_set_user_role(&username, role.as_str())
                .map_err(remote_error)
        })
        .await
    }

    /// Where an assistant connects to the signed-in server.
    pub async fn assistants(self: Arc<Self>) -> Result<Assistants, KoanError> {
        offload::offload(move || {
            let client = account_client()?;
            let offers = koan_core::remote::profile::for_auth(client.auth())
                .is_some_and(|p| p.offers(koan_core::remote::profile::MCP));
            if !offers {
                return Err(KoanError::NotFound {
                    message: "this server does not offer assistants".into(),
                });
            }
            let mcp = client.koan_mcp().map_err(remote_error)?;
            Ok(Assistants {
                mcp_url: mcp.url,
                connect_url: mcp.connect,
            })
        })
        .await
    }

    /// The signed-in account's API keys.
    pub async fn api_keys(self: Arc<Self>) -> Result<Vec<ApiKeyInfo>, KoanError> {
        offload::offload(move || {
            let seconds = |iso: Option<String>| {
                iso.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                    .map(|t| t.timestamp())
            };
            Ok(api_keys_client()?
                .koan_api_keys()
                .map_err(remote_error)?
                .into_iter()
                .map(|k| ApiKeyInfo {
                    id: k.id,
                    name: k.name,
                    created: seconds(k.created),
                    last_used: seconds(k.last_used),
                    this_device: k.current,
                })
                .collect())
        })
        .await
    }

    /// Make an API key for another app. The key is in the answer and nowhere
    /// else, ever.
    pub async fn create_api_key(self: Arc<Self>, name: String) -> Result<NewApiKey, KoanError> {
        offload::offload(move || {
            let made = api_keys_client()?
                .koan_create_api_key(&name)
                .map_err(remote_error)?;
            Ok(NewApiKey {
                name: made.name,
                key: made.key.ok_or(KoanError::BadArgument {
                    message: "the server made the key but did not send it".into(),
                })?,
            })
        })
        .await
    }

    /// Revoke one of the account's keys. Not this device's own: that is
    /// signing out.
    pub async fn revoke_api_key(self: Arc<Self>, id: i64) -> Result<(), KoanError> {
        offload::offload(move || {
            api_keys_client()?
                .koan_revoke_api_key(id)
                .map_err(remote_error)
        })
        .await
    }

    /// Give another account a password. Its devices sign out.
    pub async fn set_server_account_password(
        self: Arc<Self>,
        username: String,
        password: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            passwords_client()?
                .koan_set_user_password(&username, &password)
                .map_err(remote_error)
        })
        .await
    }

    /// Change the signed-in account's own password. This device stays signed
    /// in, with a new key; the account's other devices sign out.
    pub async fn change_own_password(
        self: Arc<Self>,
        current: String,
        password: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            passwords_client()?;
            koan_core::helpers::change_own_password(&current, &password).map_err(|e| {
                KoanError::BadArgument {
                    message: e.to_string(),
                }
            })
        })
        .await
    }

    pub async fn delete_server_account(self: Arc<Self>, username: String) -> Result<(), KoanError> {
        offload::offload(move || {
            account_client()?
                .koan_delete_user(&username)
                .map_err(remote_error)
        })
        .await
    }

    /// Forget the server. Leaves the synced library alone — those tracks are
    /// still real, they just cannot be fetched until you sign in again.
    pub async fn sign_out_remote(self: Arc<Self>) -> Result<(), KoanError> {
        offload::offload(move || {
            koan_core::remote::devices::forget_shares();
            Config::persist(|cfg| {
                cfg.remote.enabled = false;
                cfg.remote.password = String::new();
                cfg.remote.api_key = String::new();
                cfg.remote.device_key = String::new();
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })?;
            koan_core::remote::proof::forget();
            koan_core::remote::link::relink();
            koan_core::remote::nearby::readvertise();
            Ok(())
        })
        .await
    }

    /// Forget every track that came from a folder.
    ///
    /// A track the server also has keeps its row and loses only its local path.
    /// Albums and artists left holding nothing go too.
    pub async fn forget_folder(self: Arc<Self>, path: String) -> Result<u64, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let removed =
                koan_core::helpers::forget_folder(&db, Path::new(&path)).map_err(db_err)?;
            self.bump_library();
            Ok(removed)
        })
        .await
    }

    /// Forget everything that only existed on the server.
    ///
    /// A track held locally as well keeps its row and loses its remote id.
    pub async fn forget_remote(self: Arc<Self>) -> Result<u64, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let removed = koan_core::helpers::forget_remote(&db).map_err(db_err)?;
            self.bump_library();
            Ok(removed)
        })
        .await
    }

    /// Drop the library index so the next scan rebuilds it.
    ///
    /// Favourites survive — they key on the file path. Lyrics, play history and
    /// acoustic embeddings do not; they key on row ids that are about to stop
    /// existing.
    pub async fn rebuild_index(self: Arc<Self>) -> Result<RebuildSummary, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let summary = koan_core::helpers::rebuild_index(&db).map_err(db_err)?;
            self.bump_library();
            Ok(RebuildSummary {
                tracks: summary.tracks,
                albums: summary.albums,
                artists: summary.artists,
            })
        })
        .await
    }

    /// Delete every downloaded remote track. The library rows stay.
    pub async fn clear_download_cache(self: Arc<Self>) -> Result<CacheCleared, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let cfg = Config::load().unwrap_or_default();
            let cleared = koan_core::helpers::clear_download_cache(&db, &cfg);
            koan_core::helpers::requeue_cleared_downloads(&self.state);
            self.bump_library();
            Ok(CacheCleared {
                files: cleared.files,
                bytes: cleared.bytes,
            })
        })
        .await
    }

    /// Delete the downloaded copies of just these tracks. The library rows
    /// stay, and they fetch again on demand.
    pub async fn clear_downloads(
        self: Arc<Self>,
        track_ids: Vec<i64>,
    ) -> Result<CacheCleared, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let cleared = koan_core::helpers::clear_downloads_for(&db, &track_ids);
            koan_core::helpers::requeue_cleared_downloads(&self.state);
            self.bump_library();
            Ok(CacheCleared {
                files: cleared.files,
                bytes: cleared.bytes,
            })
        })
        .await
    }

    /// Fetch these tracks into the cache now, without queueing them.
    ///
    /// Downloads are normally a side effect of wanting to play something; this
    /// is for wanting the bytes on the machine and nothing else — before going
    /// somewhere without a server, most obviously. Tracks already downloaded
    /// are skipped, so asking twice costs nothing.
    ///
    /// The transfers get identities of their own rather than borrowing a queue
    /// item's, because there is no queue item: they appear in the download
    /// store and nowhere else.
    pub async fn download_to_cache(self: Arc<Self>, track_ids: Vec<i64>) -> Result<(), KoanError> {
        offload::offload(move || {
            // This device's cache, whichever device is being controlled.
            self.send_local(PlayerCommand::CacheTracks(track_ids))
        })
        .await
    }

    /// Rescans every configured library folder, saying how far it has got.
    /// Minutes, on a large library.
    pub async fn scan_reporting(
        self: Arc<Self>,
        force: bool,
        reporter: Option<Arc<dyn ProgressReporter>>,
    ) -> Result<ScanSummary, KoanError> {
        offload::offload(move || self.scan_blocking(force, reporter)).await
    }

    /// Pull the remote library into the local database: walk it, then
    /// reconcile favourites and playlists. Long and network-bound. What the
    /// Sync button does — koan's own syncs go by `Walk::IfChanged`.
    pub async fn sync_remote(self: Arc<Self>) -> Result<SyncSummary, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let cfg = Config::load().unwrap_or_default();
            let client = koan_core::helpers::subsonic_client(&cfg).ok_or_else(|| {
                KoanError::BadArgument {
                    message: "no remote server configured".into(),
                }
            })?;

            let meter = &self.sync_progress;
            let synced = koan_core::helpers::sync_remote(
                &db,
                &client,
                koan_core::helpers::Walk::Always,
                &cfg.remote.url,
                &cfg.remote.username,
                &|p| meter.set(p),
            );
            meter.clear();
            let synced = synced.map_err(|e| KoanError::Database {
                message: e.to_string(),
            })?;

            self.bump_library();
            Ok(SyncSummary {
                artists: synced.library.artists_synced as u32,
                albums: synced.library.albums_synced as u32,
                tracks: synced.library.tracks_synced as u32,
                albums_failed: synced.library.albums_failed as u32,
                pages_failed: synced.library.pages_failed as u32,
                favourites_pushed: synced.favourites.pushed as u32,
                favourites_imported: synced.favourites.imported as u32,
                playlists_pulled: synced.playlists.pulled as u32,
                playlists_pushed: synced.playlists.pushed as u32,
            })
        })
        .await
    }

    /// Create a public share link for these tracks, on the remote server when
    /// one is configured (which may be a koan server) and served by this koan
    /// otherwise.
    ///
    /// Only tracks the server knows about can go in it — the link points at the
    /// server, so a local-only file has nothing for it to point at. A mixed
    /// selection shares what it can; `skipped` says how much it left out.
    pub async fn create_share(
        self: Arc<Self>,
        track_ids: Vec<i64>,
        description: Option<String>,
    ) -> Result<Share, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let cfg = Config::load().unwrap_or_default();
            koan_core::helpers::create_share(
                &db,
                queries::LOCAL_USER,
                &cfg,
                &koan_core::helpers::ShareTarget::Tracks(track_ids),
                description.as_deref(),
            )
            .map(|outcome| Share {
                url: outcome.url,
                shared: outcome.shared as u32,
                skipped: outcome.skipped as u32,
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })
        })
        .await
    }

    /// Track IDs for an album or an artist, in running order. What the context
    /// menu actions resolve to before touching the queue.
    pub async fn track_ids(
        self: Arc<Self>,
        album_id: Option<i64>,
        artist_id: Option<i64>,
    ) -> Result<Vec<i64>, KoanError> {
        offload::offload(move || {
            Ok(self
                .tracks_blocking(album_id, artist_id, TrackSort::Album, 2000, 0)?
                .into_iter()
                .map(|t| t.id)
                .collect())
        })
        .await
    }

    // --- File organization -------------------------------------------------

    /// Named patterns from `[organize.patterns]`, sorted by name.
    ///
    /// Patterns are config, shared with the CLI and TUI, so this reads them
    /// rather than offering somewhere else to define them.
    pub async fn organize_patterns(self: Arc<Self>) -> Vec<OrganizePattern> {
        offload::offload(move || {
            let cfg = Config::load().unwrap_or_default().organize;
            let mut patterns: Vec<OrganizePattern> = cfg
                .patterns
                .iter()
                .map(|(name, pattern)| OrganizePattern {
                    name: name.clone(),
                    pattern: pattern.clone(),
                    is_default: cfg.default.as_deref() == Some(name.as_str()),
                })
                .collect();
            patterns.sort_by(|a, b| a.name.cmp(&b.name));
            patterns
        })
        .await
    }

    /// Store a named pattern in `config.toml`, replacing one of the same name.
    ///
    /// Writes the base config rather than the local overlay: patterns are a
    /// preference, not a machine fact, and the CLI and TUI read the same list.
    pub async fn save_organize_pattern(
        self: Arc<Self>,
        name: String,
        pattern: String,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            let name = name.trim().to_string();
            if name.is_empty() || pattern.trim().is_empty() {
                return Err(KoanError::BadArgument {
                    message: "a pattern needs both a name and a format string".into(),
                });
            }
            // Parse it before storing it: a pattern that can't be evaluated would
            // sit in the config failing on every future run.
            koan_core::format::parse(&pattern).map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })?;
            Config::persist(|cfg| {
                cfg.organize.patterns.insert(name, pattern);
            })
            .map_err(|e| KoanError::BadArgument {
                message: e.to_string(),
            })
        })
        .await
    }

    /// Read a selection out of the library, ready to have patterns generated
    /// against it.
    ///
    /// This is the expensive half — database rows, album facts, a `stat` per
    /// file, and a tag read for anything the library has never seen — and it
    /// happens once per selection.
    /// `track_ids` of `None` means the whole library.
    pub async fn organize_selection(
        self: Arc<Self>,
        track_ids: Option<Vec<i64>>,
    ) -> Result<Arc<OrganizeSelection>, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let requested = track_ids.as_ref().map(Vec::len);
            let inner =
                koan_core::organize::resolve(&db, track_ids.as_deref()).map_err(organize_err)?;
            Ok(Arc::new(OrganizeSelection { inner, requested }))
        })
        .await
    }

    /// Whether cover art, cue sheets and logs travel with the music.
    pub async fn organize_moves_ancillary(self: Arc<Self>) -> bool {
        offload::offload(move || Config::load().unwrap_or_default().organize.move_ancillary).await
    }

    /// Remember the choice. Written to `config.toml`, so the CLI and TUI
    /// organize the same way the app just did.
    pub async fn set_organize_moves_ancillary(
        self: Arc<Self>,
        enabled: bool,
    ) -> Result<(), KoanError> {
        offload::offload(move || {
            Config::persist(|cfg| cfg.organize.move_ancillary = enabled).map_err(|e| {
                KoanError::BadArgument {
                    message: e.to_string(),
                }
            })
        })
        .await
    }

    /// Carry out the moves, then point the queue at where the files went.
    ///
    /// The rename and the database rows land together, and playback survives it
    /// — a Unix rename keeps the open descriptor. Destructive: run it only for
    /// a plan the user has seen.
    pub async fn organize_execute(
        self: Arc<Self>,
        pattern: String,
        track_ids: Option<Vec<i64>>,
        base_dir: Option<String>,
    ) -> Result<OrganizePlan, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let base = base_dir.map(PathBuf::from);
            let result = match &track_ids {
                Some(ids) => {
                    koan_core::organize::execute_for_tracks(&db, ids, &pattern, base.as_deref())
                }
                None => koan_core::organize::execute(&db, &pattern, base.as_deref()),
            }
            .map_err(organize_err)?;
            self.follow_moved_files(&result);
            self.bump_library();
            Ok(OrganizePlan::build(
                result,
                track_ids.as_ref().map(Vec::len),
            ))
        })
        .await
    }

    /// Index files from anywhere into the library, and return their track IDs.
    ///
    /// This is what a drop from Finder lands on: the files are read for tags,
    /// given library rows where they sit, and handed back as IDs the caller can
    /// queue. They are not moved — organize is what puts them under a library
    /// folder, and it can only do that once they have rows. Directories are
    /// walked recursively. Tag-bound, so it is proportional to the selection.
    pub async fn import_files(
        self: Arc<Self>,
        paths: Vec<String>,
    ) -> Result<ImportSummary, KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
            let result = koan_core::index::scanner::import_paths(&db, &paths);
            self.bump_library();
            Ok(ImportSummary {
                track_ids: result.track_ids,
                added: result.added as u32,
                updated: result.updated as u32,
                errors: result
                    .errors
                    .into_iter()
                    .map(|(p, e)| format!("{}: {e}", p.display()))
                    .collect(),
            })
        })
        .await
    }

    /// Where the library folders point. Shown in settings.
    pub async fn library_folders(self: Arc<Self>) -> Vec<String> {
        offload::offload(move || {
            Config::cached()
                .library
                .folders
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        })
        .await
    }
}

/// A resolved selection, held by the caller so a pattern can be generated
/// against it many times without re-reading anything.
///
/// The split is the point. `generate` is pure string work and runs on every
/// keystroke; `check` is the filesystem pass and runs once the typing settles.
/// Neither can change what the other decided, so the fast answer is never
/// wrong — only less complete.
#[derive(uniffi::Object)]
pub struct OrganizeSelection {
    inner: koan_core::organize::ResolvedSelection,
    /// How many tracks were asked for, so the shortfall can be reported.
    requested: Option<usize>,
}

#[uniffi::export]
impl OrganizeSelection {
    /// Turn the pattern into destinations. **Touches no files** — the cost is
    /// a destination per track, which is why it is off-thread like everything
    /// else rather than resolved inline as someone types.
    pub async fn generate(self: Arc<Self>, pattern: String, base_dir: String) -> OrganizePlan {
        offload::offload(move || {
            let result = koan_core::organize::generate(&self.inner, &pattern, Path::new(&base_dir));
            OrganizePlan::build(result, self.requested)
        })
        .await
    }

    /// Generate, then ask the disk the two questions generation cannot answer:
    /// which destinations are already occupied, and what ancillary files travel
    /// with each move. A `stat` per file and a directory read per source
    /// folder, so it runs once the typing settles rather than per keystroke.
    pub async fn check(
        self: Arc<Self>,
        pattern: String,
        base_dir: String,
        move_ancillary: bool,
    ) -> OrganizePlan {
        offload::offload(move || {
            let mut result =
                koan_core::organize::generate(&self.inner, &pattern, Path::new(&base_dir));
            koan_core::organize::check_against_disk(&mut result, move_ancillary);
            OrganizePlan::build(result, self.requested)
        })
        .await
    }

    /// How many files resolved to something local. Fewer than were asked for
    /// means the rest are remote-only or gone from disk.
    pub fn count(&self) -> u32 {
        self.inner.len() as u32
    }
}

// --- Internals -------------------------------------------------------------

impl KoanEngine {
    /// Watch shared state and publish what changed.
    ///
    /// Woken rather than timed: every setter on `SharedPlayerState`, the
    /// download store and the library version signal a change, and this waits
    /// in between. A koan with nothing happening does not run this thread at
    /// all.
    ///
    /// What the wake does *not* say is which of them moved: that is still read
    /// off the versions here, on waking, because they are cheap and because a
    /// single wake covers a burst. So a pass batches: whatever moved between
    /// two wakes leaves as at most one message per slice, and a client's cost
    /// is set by how many slices changed and never by how many times they
    /// changed. The expensive snapshots are built only when the version behind
    /// them moved — deriving the whole queue to find it unchanged is the waste
    /// that guard exists to avoid.
    ///
    /// The playhead is the one thing no writer can announce, because it moves
    /// on its own. It is published as an anchor instead: see `state::Anchor`.
    /// Publish the transfers' figures whenever the store takes a reading — a
    /// few times a second while something downloads, never otherwise.
    ///
    /// Its own thread on the store's own signal. Readings are rung there and
    /// not on the engine's, so a transfer in progress wakes this and nothing
    /// else: not the state watcher, not a link, not a subscription.
    fn spawn_figures(self: &Arc<Self>) {
        let engine = Arc::downgrade(self);
        let store = self.state.downloads().clone();
        std::thread::Builder::new()
            .name("koan-figures".into())
            .spawn(move || {
                let mut seen = store.moved().generation();
                let mut last = u64::MAX;
                loop {
                    let Some(engine) = engine.upgrade() else {
                        return;
                    };
                    let figures = store.figures();
                    if figures != last {
                        last = figures;
                        engine.out.publish(StateSlice::Figures {
                            figures: store.all().iter().map(TransferFigure::of).collect(),
                        });
                    }
                    drop(engine);
                    // Bounded, to notice the engine going: nothing rings this
                    // when it does.
                    seen = store
                        .moved()
                        .wait_until(seen, std::time::Duration::from_secs(30));
                }
            })
            .expect("failed to spawn the figures thread");
    }

    fn spawn_watcher(self: &Arc<Self>) {
        let engine = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("koan-state".into())
            .spawn(move || {
                let mut last_queue = u64::MAX;
                let mut last_store = u64::MAX;
                let mut last_library = u64::MAX;
                let mut last_devices = u64::MAX;
                let mut last_target: Option<String> = None;
                let mut queue = queue_slice::QueueSender::default();
                // The playhead as a client last heard it, and when. What it
                // would believe now is derived from these two, which is what
                // makes publishing again unnecessary until it would be wrong.
                let mut anchor: Option<state::Anchor> = None;
                let mut last_seekable = u64::MAX;
                // Which transfers were running last pass. A transfer leaving
                // this set has landed on disk, which wrote a cached path onto a
                // library row — and nothing else says so, because the download
                // ran in koan-core, which has no notion of that version.
                let mut running: HashSet<i64> = HashSet::new();
                // A transfer landed since the library last said so.
                let mut landed = false;
                let mut last_landing = Instant::now();

                // Where this thread spends the whole of a quiet koan.
                let wake = koan_core::signal::engine_changed();
                // Read before the first pass, not after it: anything that moves
                // while a pass is publishing leaves the generation past this,
                // and the wait at the foot of the loop returns at once rather
                // than sleeping through it.
                let mut seen = wake.generation();
                loop {
                    let Some(engine) = engine.upgrade() else {
                        return; // Engine dropped; so is the app.
                    };
                    let out = &engine.out;

                    // Controlling another device puts its playback, playhead
                    // and queue where this one's go, and a change of device
                    // has every slice said again for the one now shown.
                    let target = koan_core::remote::devices::target();
                    if target != last_target {
                        last_target = target.clone();
                        anchor = None;
                        last_seekable = u64::MAX;
                        last_queue = u64::MAX;
                        last_library = u64::MAX;
                        last_devices = u64::MAX;
                        queue.reset();
                    }
                    let devices_version = koan_core::remote::devices::version();
                    if devices_version != last_devices {
                        last_devices = devices_version;
                        let list = koan_core::remote::devices::list();
                        out.publish(StateSlice::Devices {
                            devices: engine.device_infos(&list),
                            target: target.clone(),
                        });
                        out.publish(StateSlice::Connection {
                            connection: connection_info(),
                        });
                        if let Some(t) = &target {
                            engine.publish_remote(list.iter().find(|d| d.id == *t));
                        }
                    }

                    // A handful of rows, compared whole: published only when a
                    // renderer comes, goes, or the one playing changes.
                    out.publish(StateSlice::Renderers {
                        renderers: koan_core::upnp::discovery::renderers()
                            .into_iter()
                            .map(|r| RendererInfo {
                                busy: koan_core::upnp::discovery::busy(&r.udn),
                                udn: r.udn,
                                name: r.name,
                                manufacturer: r.manufacturer,
                                model: r.model,
                                gapless: r.gapless,
                            })
                            .collect(),
                        output: engine.state.renderer().map(|o| RendererOutput {
                            udn: o.udn,
                            name: o.name,
                            volume: o.volume,
                            problem: o.problem,
                        }),
                    });

                    // The outputs of the device in view, read where its
                    // playback is: this one's own, or what the device being
                    // controlled last published.
                    out.publish(StateSlice::Outputs {
                        outputs: match &target {
                            None => Some(OutputsInfo::of(
                                None,
                                koan_core::remote::outputs::local(&engine.state),
                            )),
                            // Only the account's own devices say, and take
                            // being told.
                            Some(_) => koan_core::remote::devices::target_device()
                                .filter(|d| d.account)
                                .and_then(|d| {
                                    Some(OutputsInfo::of(Some(d.name), d.state?.outputs?))
                                }),
                        },
                    });

                    // Compared whole rather than on a signature of a few named
                    // fields: the output sample rate moves when another client
                    // retunes the device, a stream's duration is corrected once
                    // the download lands, and the seekable extent grows for as
                    // long as the bytes are arriving — none of them moving the
                    // state or the cursor. A signature has to be remembered to
                    // be widened.
                    let snapshot = engine.now_playing_blocking();
                    if target.is_none() {
                        out.publish(StateSlice::Playback {
                            now_playing: NowPlaying {
                                // Position has a slice of its own; leaving it here
                                // would make every tick a change to this one. The
                                // queue version likewise: it rides with the queue,
                                // so an edit there is not a change to what is
                                // playing.
                                position_ms: 0,
                                playlist_version: 0,
                                ..snapshot.clone()
                            },
                        });
                        // Published when a client's own reckoning would be wrong,
                        // not when the number changed — it changes continuously, by
                        // definition, and saying so ten times a second is a stream
                        // that can never go quiet while music plays. A playhead
                        // advancing at one second per second is the one thing a
                        // client can work out for itself; a seek, a pause, a track
                        // boundary and a stall are not, and each of them breaks the
                        // prediction by more than the tolerance below.
                        let playing = snapshot.state == types::PlayState::Playing
                            && engine.state.playhead_moving();
                        let seekable = engine.state.seekable_ms();
                        let now = Instant::now();
                        let adrift = anchor.is_none_or(|held: state::Anchor| {
                            held.stale(snapshot.position_ms, playing, now, PLAYHEAD_TOLERANCE_MS)
                        });
                        // The extent a download reaches grows with every chunk, and
                        // a bar drawn 200ms of audio short of the truth is a bar
                        // nobody can tell from a correct one.
                        let stretched = last_seekable.abs_diff(seekable) > SEEKABLE_TOLERANCE_MS;
                        if adrift || stretched {
                            anchor = Some(state::Anchor {
                                position_ms: snapshot.position_ms,
                                playing,
                                at: now,
                            });
                            last_seekable = seekable;
                            out.reanchor(StateSlice::Playhead {
                                position_ms: snapshot.position_ms,
                                seekable_ms: seekable,
                                playing,
                            });
                        }
                    }

                    // The list's shape: a transfer listed, settled or
                    // forgotten. Its figures are `spawn_figures`'.
                    let store = engine.state.downloads();
                    let store_version = store.version();
                    if store_version != last_store {
                        last_store = store_version;
                        let transfers = store.all();
                        let ids: Vec<i64> = transfers.iter().map(|d| d.track_id).collect();
                        let albums = engine
                            .db()
                            .ok()
                            .and_then(|db| queries::album_ids_for_tracks(&db.conn, &ids).ok())
                            .unwrap_or_default();
                        out.publish(StateSlice::Transfers {
                            transfers: transfers
                                .iter()
                                .map(|d| Transfer::of(d, albums.get(&d.track_id).copied()))
                                .collect(),
                        });
                        let now_running: HashSet<_> = transfers
                            .iter()
                            .filter(|d| !d.state.is_settled())
                            .map(|d| d.track_id)
                            .collect();
                        if running.difference(&now_running).next().is_some() {
                            landed = true;
                        }
                        running = now_running;
                    }
                    // A landing is a library change, but a record arriving is
                    // a dozen of them a few seconds apart, and every client
                    // answers each one by asking for everything again. Said
                    // once the batch is down, or every couple of seconds while
                    // it is still coming — the row for the track that just
                    // landed is not worth a full reload per track.
                    if landed && (running.is_empty() || last_landing.elapsed() > LANDING_COALESCE) {
                        landed = false;
                        last_landing = Instant::now();
                        engine.bump_library();
                    }

                    let library = engine
                        .library_version
                        .load(std::sync::atomic::Ordering::Relaxed);
                    out.publish(StateSlice::Library { version: library });
                    out.publish(StateSlice::History {
                        version: koan_core::player::history::version(),
                    });

                    out.publish(StateSlice::Tasks {
                        scanning: engine
                            .auto_scanning
                            .load(std::sync::atomic::Ordering::Relaxed),
                        syncing: engine
                            .auto_syncing
                            .load(std::sync::atomic::Ordering::Relaxed),
                    });
                    out.publish(StateSlice::Sync {
                        progress: engine.sync_progress.get(),
                    });

                    // Both of the heavy reads, and both guarded. The queue is
                    // derived and joined against the library; the lock is two
                    // indexed reads. Neither can change without one of these
                    // two versions moving.
                    let queue_version = engine.state.playlist_version();
                    let queue_moved = queue_version != last_queue;
                    let library_moved = library != last_library;
                    last_queue = queue_version;
                    last_library = library;
                    if (queue_moved || library_moved) && target.is_none() {
                        let slices = queue
                            .update(&engine.state, library_moved, |ids| engine.queue_joins(ids));
                        for slice in slices {
                            out.publish(slice);
                        }
                    }
                    // A playlist edit moves the library version, and following
                    // means an edit there is an edit to what is playing — so
                    // the lock has to be re-asked on either.
                    if (queue_moved || library_moved) && target.is_none() {
                        out.publish(StateSlice::Lock {
                            lock: engine.queue_lock_blocking(),
                        });
                    }

                    // Dropped before the wait: this thread holds the engine
                    // only for as long as it is reading it, so parking here
                    // cannot be what keeps koan open.
                    drop(engine);
                    seen = wake.wait(seen);
                }
            })
            .ok();
    }

    fn cover_art_of(
        &self,
        row: Option<queries::TrackRow>,
        size: Option<u32>,
    ) -> Result<Option<CoverArt>, KoanError> {
        let Some(row) = row else {
            return Ok(None);
        };
        let track_id = row.id;

        if let Some(art) = local_cover_art(&row) {
            return Ok(Some(art));
        }

        let Some(remote_id) = row.remote_id else {
            return Ok(None);
        };
        let cfg = Config::cached();
        // No server configured: this record simply has no art.
        if !cfg.remote.enabled {
            return Ok(None);
        }
        // Configured but unusable — signed out, or the password cannot be
        // read. Reported rather than shrugged off: answering "no art" for
        // every record makes a signed-out client look like a library that
        // has no covers, which is a long way from where the problem is.
        let Some(client) = koan_core::helpers::subsonic_client(&cfg) else {
            return Err(KoanError::Remote {
                message: koan_core::helpers::remote_unavailable(&cfg),
            });
        };
        match client.get_cover_art(&remote_id, size) {
            Ok(data) if !data.is_empty() => {
                let mime = sniff_mime(&data).to_string();
                Ok(Some(CoverArt { data, mime }))
            }
            // The server answered and it has nothing. Normal, and worth
            // remembering: this record has no art and never will.
            Ok(_) | Err(SubsonicError::Api { .. }) | Err(SubsonicError::BadResponse) => Ok(None),
            // A timeout or a dropped connection says nothing about whether
            // art exists. Reported rather than swallowed, so the caller can
            // ask again instead of recording "this album has none" for the
            // rest of the session — and so it appears in the log at all.
            Err(e) => {
                log::warn!("cover art for track {track_id} failed: {e}");
                Err(KoanError::Remote {
                    message: e.to_string(),
                })
            }
        }
    }

    /// The devices as the app shows them, each joined to this library's row
    /// for what it is playing.
    fn device_infos(&self, list: &[koan_core::remote::devices::Device]) -> Vec<DeviceInfo> {
        let playing: Vec<String> = list
            .iter()
            .filter_map(|d| current_entry(d.state.as_ref()?)?.track_id.clone())
            .collect();
        let rows = self.rows_for_remote(&playing);
        list.iter()
            .map(|d| {
                let st = d.state.clone().unwrap_or_default();
                let row = current_entry(&st)
                    .and_then(|e| e.track_id.as_ref())
                    .and_then(|r| rows.get(r));
                DeviceInfo {
                    id: d.id.clone(),
                    name: d.name.clone(),
                    platform: d.platform.clone(),
                    account: d.account,
                    owner: d.owner.clone(),
                    nearby: d.nearby,
                    awake: d.awake,
                    asleep: d.asleep,
                    wakeable: d.wakeable,
                    last_seen: d.last_seen,
                    waking: d.waking.as_ref().and_then(|w| match w {
                        koan_core::remote::devices::Waking::Network => Some("network".into()),
                        koan_core::remote::devices::Waking::Push => Some("push".into()),
                        koan_core::remote::devices::Waking::Notification => {
                            Some("notification".into())
                        }
                        koan_core::remote::devices::Waking::Failed(_) => None,
                    }),
                    wake_failed: d.waking.as_ref().and_then(|w| match w {
                        koan_core::remote::devices::Waking::Failed(why) => Some(why.clone()),
                        _ => None,
                    }),
                    same_library: d.same_library,
                    state: play_state(&st),
                    title: st.title.clone(),
                    artist: st.artist.clone(),
                    album: st.album.clone(),
                    track_id: row.map(|r| r.id),
                    album_id: row.and_then(|r| r.album_id),
                    position_ms: d.position_ms(),
                    duration_ms: st.duration_ms,
                    problem: d.problem.clone(),
                }
            })
            .collect()
    }

    /// Publish another device's playback, playhead and queue as the app's.
    fn publish_remote(&self, device: Option<&koan_core::remote::devices::Device>) {
        let st = device.and_then(|d| d.state.clone()).unwrap_or_default();
        let ids: Vec<String> = st.queue.iter().filter_map(|e| e.track_id.clone()).collect();
        let rows = self.rows_for_remote(&ids);
        let current = st.queue.iter().position(|e| e.current);
        let items: Vec<QueueItem> = st
            .queue
            .iter()
            .enumerate()
            .map(|(n, e)| {
                let row = e.track_id.as_ref().and_then(|r| rows.get(r));
                QueueItem {
                    queue_item_id: e.id.clone().unwrap_or_else(|| format!("remote-{n}")),
                    track_id: row.map(|r| r.id),
                    album_id: row.and_then(|r| r.album_id),
                    title: e.title.clone(),
                    artist: e.artist.clone(),
                    album_artist: row
                        .map_or_else(|| e.artist.clone(), |r| r.album_artist_name.clone()),
                    album: if e.album.is_empty() {
                        row.map(|r| r.album_title.clone()).unwrap_or_default()
                    } else {
                        e.album.clone()
                    },
                    year: None,
                    codec: row.and_then(|r| r.codec.clone()),
                    track_number: row.and_then(|r| r.track_number.map(i64::from)),
                    disc: row.and_then(|r| r.disc.map(i64::from)),
                    duration_ms: (e.duration_ms > 0)
                        .then_some(e.duration_ms)
                        .or_else(|| row.and_then(|r| r.duration_ms.map(|d| d as u64))),
                    status: match current {
                        Some(c) if n == c => EntryStatus::Playing,
                        Some(c) if n < c => EntryStatus::Played,
                        _ => EntryStatus::Queued,
                    },
                    playlist_entry_id: None,
                    failure_reason: None,
                    on_server: e.track_id.is_some(),
                    on_disk: false,
                }
            })
            .collect();
        let entry = current.and_then(|c| items.get(c).cloned());
        let out = &self.out;
        out.publish(StateSlice::Playback {
            now_playing: NowPlaying {
                state: play_state(&st),
                waiting: false,
                position_ms: 0,
                duration_ms: st.duration_ms,
                queue_item_id: entry.as_ref().map(|e| e.queue_item_id.clone()),
                entry,
                format: None,
                playlist_version: 0,
                shuffle: st.shuffle,
                repeat_mode: st.repeat.into(),
                sleep: st.sleep.map(Into::into),
                sleep_fading: st.sleep_fading,
            },
        });
        out.publish(StateSlice::Playhead {
            position_ms: device.map_or(0, |d| d.position_ms()),
            seekable_ms: st.duration_ms,
            playing: st.playing,
        });
        out.publish(StateSlice::Queue {
            version: koan_core::remote::devices::version(),
            items,
        });
        out.publish(StateSlice::Lock { lock: None });
    }

    /// This library's rows for the server's track ids, keyed by those ids.
    fn rows_for_remote(&self, remote_ids: &[String]) -> HashMap<String, queries::TrackRow> {
        let Ok(db) = self.db() else {
            return HashMap::new();
        };
        let Ok(mut stmt) = db
            .conn
            .prepare_cached("SELECT id FROM tracks WHERE remote_id = ?1")
        else {
            return HashMap::new();
        };
        let ids: Vec<i64> = remote_ids
            .iter()
            .filter_map(|r| stmt.query_row([r], |row| row.get(0)).ok())
            .collect();
        drop(stmt);
        queries::tracks_by_ids(&db.conn, &ids)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| t.remote_id.clone().map(|r| (r, t)))
            .collect()
    }

    /// The library's reading of the queue's tracks, in two statements rather
    /// than one per row.
    ///
    /// A client draws one sleeve per album, and without an ID to group by it
    /// asks for artwork per track — the same image fetched once for every track
    /// on the record. A queue with no database behind it has no album IDs; the
    /// art falls back to the per-track lookup.
    fn queue_joins(&self, track_ids: &[i64]) -> queue_slice::Joins {
        let Ok(db) = self.db() else {
            return queue_slice::Joins::default();
        };
        queue_slice::Joins {
            album_ids: queries::batch::album_ids_for_tracks(&db.conn, track_ids)
                .unwrap_or_default(),
            sources: queries::batch::sources_for_tracks(&db.conn, track_ids).unwrap_or_default(),
        }
    }

    /// What the queue still is, if it is still something.
    ///
    /// While this answers, the queue follows that playlist or record: an edit
    /// there lands here too. It stops answering the moment the queue is
    /// rearranged or added to — which is also when the following stops.
    fn queue_lock_blocking(&self) -> Option<QueueLock> {
        let db = self.db().ok()?;
        match koan_core::playlists::queue_lock(&db, &self.state)? {
            koan_core::playlists::QueueLock::Playlist(id) => queries::get_playlist(&db.conn, id)
                .ok()
                .flatten()
                .map(|p| QueueLock::Playlist {
                    playlist: Playlist::from(p),
                }),
            koan_core::playlists::QueueLock::Album(id) => queries::get_album(&db.conn, id)
                .ok()
                .flatten()
                .map(|a| QueueLock::Album {
                    album: Album::from(a),
                }),
        }
    }

    /// Rewrite the queue to point at where organize put the files.
    ///
    /// The database rows moved with the files, but the playlist is in memory
    /// and still holds the old paths — a queued track would fail to open on the
    /// next play. Only the player may mutate it, so this goes through the
    /// command channel like every other queue change.
    fn follow_moved_files(&self, result: &koan_core::organize::OrganizeResult) {
        let moved: std::collections::HashMap<&Path, &Path> = result
            .moves()
            .filter_map(|e| e.to.as_deref().map(|to| (e.from.as_path(), to)))
            .collect();
        if moved.is_empty() {
            return;
        }

        let (items, _) = self.state.snapshot_playlist();
        let updates: Vec<(QueueItemId, PathBuf)> = items
            .iter()
            .filter_map(|item| {
                moved
                    .get(item.path.as_path())
                    .map(|to| (item.id, to.to_path_buf()))
            })
            .collect();
        if !updates.is_empty() {
            let _ = self.send_local(PlayerCommand::UpdatePaths(updates));
        }
    }

    fn tracks_blocking(
        &self,
        album_id: Option<i64>,
        artist_id: Option<i64>,
        sort: TrackSort,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Track>, KoanError> {
        let db = self.db()?;
        let rows = match (album_id, artist_id) {
            (Some(aid), _) => queries::tracks_for_album(&db.conn, aid),
            (None, Some(aid)) => queries::tracks_for_artist(&db.conn, aid),
            (None, None) => queries::all_tracks_paged(&db.conn, limit, offset),
        }
        .map_err(db_err)?;
        Ok(self.decorate(&db, sort_rows(rows, sort)))
    }

    /// The fuzzy corpus for `kind`, read once per library version.
    fn corpus(&self, kind: queries::CorpusKind) -> Result<queries::Corpus, KoanError> {
        let version = self
            .library_version
            .load(std::sync::atomic::Ordering::Relaxed);
        self.fuzzy
            .get(&self.db()?.conn, kind, version)
            .map_err(db_err)
    }

    /// Say that the library's rows changed. The watcher turns this into a
    /// `Library` slice when it wakes.
    ///
    /// Playlists count. They are rows in the same database and a page showing
    /// one has the same problem a page showing a record has — a second signal
    /// for them would be a second thing to remember to send.
    fn bump_library(&self) {
        self.library_version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        koan_core::signal::engine_changed().bump();
    }

    fn scan_blocking(
        &self,
        force: bool,
        reporter: Option<Arc<dyn ProgressReporter>>,
    ) -> Result<ScanSummary, KoanError> {
        let db = self.db()?;
        let cfg = Config::load().unwrap_or_default();

        let mut summary = ScanSummary {
            added: 0,
            updated: 0,
            removed: 0,
            skipped: 0,
            errors: Vec::new(),
        };
        // `force_remove` stays off: it lifts the brake that stops a failed mount
        // from deleting the library, which is not a call a GUI button should make.
        let opts = koan_core::index::scanner::ScanOptions {
            force,
            force_remove: false,
            cancel: Some(self.cancel_library_task.clone()),
        };
        // A cancel from a previous run must not stop this one before it starts.
        self.cancel_library_task
            .store(false, std::sync::atomic::Ordering::Relaxed);

        let done = std::sync::atomic::AtomicU64::new(0);
        let started = |total: u64| {
            if let Some(reporter) = &reporter {
                reporter.started(total);
            }
        };
        let callback = |event: koan_core::index::scanner::ScanEvent| {
            let Some(reporter) = &reporter else { return };
            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            // Reporting every file would be tens of thousands of trips over
            // the FFI and a redraw for each. Every 64 still looks live.
            if n.is_multiple_of(64) {
                reporter.advanced(n, format!("{} — {}", event.artist, event.title));
            }
        };
        let hook: Option<&dyn Fn(koan_core::index::scanner::ScanEvent)> =
            reporter.as_ref().map(|_| &callback as _);

        let r = koan_core::index::scanner::scan_folders(
            &db,
            &cfg.library.folders,
            opts,
            Some(&started),
            hook,
        );
        summary.added += r.added as u32;
        summary.updated += r.updated as u32;
        summary.removed += r.removed as u32;
        summary.skipped += r.skipped as u32;
        summary.errors.extend(
            r.errors
                .into_iter()
                .map(|(p, e)| format!("{}: {e}", p.display())),
        );
        self.bump_library();
        Ok(summary)
    }

    fn build(device_name: Option<String>) -> Result<Arc<Self>, KoanError> {
        let t0 = std::time::Instant::now();
        init_logging();
        let db_path = config::db_path();
        // Fail fast on a broken library rather than after the audio threads exist.
        let db = Database::open(&db_path).map_err(|e| KoanError::Database {
            message: e.to_string(),
        })?;
        let t_db = t0.elapsed();

        // Before anything can start a download of its own, so this only ever
        // sees files left by a previous run.
        let cfg = Config::load().unwrap_or_default();
        koan_core::helpers::sweep_partial_downloads(&cfg);
        // Before the session is restored, so the queue finds its downloads.
        if let Err(e) = koan_core::helpers::relocate_cached_paths(&db, &cfg.cache_dir()) {
            log::warn!("could not re-root cached paths: {e}");
        }
        drop(db);
        let t_sweep = t0.elapsed();

        let (state, _timeline, viz, tx) = Player::spawn_for_listening();
        let t_player = t0.elapsed();

        // Bumped by the background tasks below as well as by everything the UI
        // asks for, so a sync nobody asked for reaches a client the same way.
        let library_version = Arc::new(std::sync::atomic::AtomicU64::new(0));

        // Finishing is the interesting edge: rows landed while it ran, and the
        // moment it stops is the moment they are all there.
        let finished = {
            let version = library_version.clone();
            move |flag: &std::sync::atomic::AtomicBool, running: bool| {
                if !running && flag.swap(running, std::sync::atomic::Ordering::Relaxed) {
                    version.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else {
                    flag.store(running, std::sync::atomic::Ordering::Relaxed);
                }
                // Either way something a client draws has moved: the row for
                // this task, or the library the task just wrote to.
                koan_core::signal::engine_changed().bump();
            }
        };

        let auto_syncing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sync_progress = SyncMeter::default();
        {
            let flag = auto_syncing.clone();
            let finished = finished.clone();
            let (meter, reading) = (sync_progress.clone(), sync_progress.clone());
            koan_core::helpers::spawn_auto_sync(
                db_path.clone(),
                move |running| {
                    if !running {
                        meter.clear();
                    }
                    finished(&flag, running);
                },
                move |p| reading.set(p),
            );
        }

        // Local files are watched rather than synced on a timer: a folder that
        // has not changed costs nothing to notice, and one that has should show
        // up without being asked.
        let auto_scanning = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let flag = auto_scanning.clone();
            koan_core::helpers::spawn_library_watch(db_path.clone(), move |running| {
                finished(&flag, running);
            });
        }

        let engine = Arc::new(Self {
            state,
            tx,
            viz,
            out: state::EngineState::new(),
            auto_syncing,
            auto_scanning,
            sync_progress,
            cancel_library_task: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            library_version: library_version.clone(),
            saved_content: std::sync::atomic::AtomicU64::new(u64::MAX),
            fuzzy: queries::CorpusCache::default(),
            playlist_history: Default::default(),
            pairing: Default::default(),
            pairing_cancel: Default::default(),
            device_name: device_name.clone(),
        });
        engine.spawn_watcher();
        engine.spawn_figures();
        // A koan server this app syncs from can then tell it what to play, and
        // so can this person's other devices and anyone's on the network.
        let (weak, state) = (Arc::downgrade(&engine), engine.state.clone());
        // What a device controlling this one draws its bars from, while it
        // watches.
        let playhead = engine.state.clone();
        koan_core::remote::levels::feed()
            .provide(engine.viz.clone(), move || playhead.position_ms());
        let held =
            std::sync::Mutex::new(None::<(u64, Vec<koan_core::remote::link::LinkQueueEntry>)>);
        // Profiles of one name from two devices are told apart by it.
        koan_core::remote::dsp_sync::set_device_name(device_name.clone());
        koan_core::remote::devices::start(koan_core::remote::link::Local {
            identity: koan_core::remote::link::LinkIdentity::this_device(device_name),
            // What a command may cost, and where it may go, depends on who
            // sent it: see `handle_link`.
            on_command: Arc::new(move |cmd, source, pending| {
                let weak = weak.clone();
                offload::from_another_device(move || {
                    if let Some(engine) = weak.upgrade() {
                        engine.handle_link(cmd, source, pending);
                    }
                });
            }),
            // The queue is read again only when it has changed: this runs on
            // every change to the engine, and naming its tracks is a query.
            state: Arc::new(move || {
                let item = state.cursor().and_then(|c| state.get_item(c));
                let version = state.playlist_version();
                let mut held = held.lock().unwrap_or_else(|e| e.into_inner());
                if held.as_ref().is_none_or(|(v, _)| *v != version) {
                    *held = Some((version, link_queue(&state)));
                }
                koan_core::remote::link::LinkState {
                    playing: state.playback_state() == PlaybackState::Playing,
                    title: item.as_ref().map(|i| i.title.clone()),
                    artist: item.as_ref().map(|i| i.artist.clone()),
                    album: item.as_ref().map(|i| i.album.clone()),
                    position_ms: state.position_ms(),
                    duration_ms: state.duration_ms(),
                    queue: held.as_ref().map(|(_, q)| q.clone()).unwrap_or_default(),
                    outputs: Some(koan_core::remote::outputs::local(&state)),
                    shuffle: state.play_mode().shuffle,
                    repeat: state.play_mode().repeat,
                    sleep: state.sleep(),
                    sleep_fading: state.sleep_fading(),
                }
            }),
        });
        log::info!(
            "startup: db {:?}, sweep {:?}, player {:?}, built {:?}",
            t_db,
            t_sweep,
            t_player,
            t0.elapsed()
        );
        Ok(engine)
    }

    fn now_playing_blocking(&self) -> NowPlaying {
        let info = self.state.track_info();
        let cursor = self.state.cursor();
        let play_state = self.state.playback_state();
        let entry = cursor
            .and_then(|cid| self.state.get_item(cid))
            .map(|item| QueueItem::from_cursor_item(&item, play_state, self.state.downloads()));

        NowPlaying {
            state: play_state.into(),
            waiting: self.state.is_waiting(),
            position_ms: self.state.position_ms(),
            duration_ms: self.state.duration_ms(),
            queue_item_id: cursor.map(|c| c.0.to_string()),
            entry,
            format: info
                .as_ref()
                .map(|i| StreamFormat::of(i, self.state.output_sample_rate(), self.state.dsp())),
            playlist_version: self.state.playlist_version(),
            shuffle: self.state.play_mode().shuffle,
            repeat_mode: self.state.play_mode().repeat.into(),
            sleep: self.state.sleep().map(Into::into),
            sleep_fading: self.state.sleep_fading(),
        }
    }

    /// A connection for one piece of work, borrowed from the pool.
    ///
    /// Opening one checks the schema and checkpoints the WAL, which contends
    /// with downloads writing.
    fn db(&self) -> Result<koan_core::db::pool::Handle<'static>, KoanError> {
        koan_core::db::pool::shared().get().map_err(db_err)
    }

    fn write_position(&self, db: &Database) -> Result<(), KoanError> {
        let cursor_path = self
            .state
            .cursor_path()
            .map(|p| p.to_string_lossy().into_owned());
        queries::save_playback_position(
            &db.conn,
            self.state.play_mode(),
            cursor_path.as_deref(),
            self.state.position_ms(),
            self.state.playback_state() == PlaybackState::Playing,
        )
        .map_err(fav_err)
    }

    /// Send to the player this app is controlling: this device's own, or
    /// another's, translated into what the link carries.
    fn send(&self, cmd: PlayerCommand) -> Result<(), KoanError> {
        match koan_core::remote::devices::target() {
            Some(target) => self.send_remote(&target, cmd),
            None => self.send_local(cmd),
        }
    }

    /// A player command as the device `to` is sent it. Queue entries are
    /// named by the ids that device reported them under, which are what the
    /// app holds while it shows that device's queue; tracks by the server's
    /// ids, which only a device playing from the same library can resolve.
    fn send_remote(&self, to: &str, cmd: PlayerCommand) -> Result<(), KoanError> {
        use koan_core::remote::link::LinkCommand;
        let entry = |id: QueueItemId| id.0.to_string();
        let tracks =
            |items: &[koan_core::player::state::PlaylistItem]| -> Result<Vec<String>, KoanError> {
                let same =
                    koan_core::remote::devices::target_device().is_some_and(|d| d.same_library);
                if !same {
                    return Err(KoanError::BadArgument {
                        message: "that device plays from a different library".into(),
                    });
                }
                let ids: Vec<i64> = items.iter().filter_map(|i| i.db_id).collect();
                let remote = self.remote_ids(&ids);
                let out: Vec<String> = items
                    .iter()
                    .filter_map(|i| i.db_id.and_then(|id| remote.get(&id).cloned()))
                    .collect();
                if out.is_empty() {
                    return Err(KoanError::BadArgument {
                        message:
                            "none of these are on the server, so the other device cannot play them"
                                .into(),
                    });
                }
                Ok(out)
            };
        let link = match cmd {
            PlayerCommand::Play(id) => LinkCommand::PlayItem { id: entry(id) },
            PlayerCommand::Pause | PlayerCommand::Stop => LinkCommand::Pause,
            PlayerCommand::Resume => LinkCommand::Resume,
            PlayerCommand::Seek(position_ms) => LinkCommand::Seek { position_ms },
            PlayerCommand::NextTrack => LinkCommand::Next,
            PlayerCommand::PrevTrack => LinkCommand::Previous,
            PlayerCommand::SetShuffle(on) => LinkCommand::Shuffle { on },
            PlayerCommand::SetRepeat(mode) => LinkCommand::Repeat { mode },
            PlayerCommand::SetSleepTimer(timer) => LinkCommand::SleepTimer { timer },
            PlayerCommand::AddToPlaylist(items) => LinkCommand::Enqueue {
                track_ids: tracks(&items)?,
            },
            PlayerCommand::InsertInPlaylist { items, after } => LinkCommand::Insert {
                track_ids: tracks(&items)?,
                after: entry(after),
            },
            PlayerCommand::ReplacePlaylist {
                items,
                start,
                position_ms,
                play,
            } => {
                // Where `start` lands once the tracks the server lacks are
                // left out: on it, or on the next one that remains.
                let track_ids = tracks(&items)?;
                let remote =
                    self.remote_ids(&items.iter().filter_map(|i| i.db_id).collect::<Vec<_>>());
                let start_at = items
                    .iter()
                    .take(start)
                    .filter(|i| i.db_id.is_some_and(|id| remote.contains_key(&id)))
                    .count();
                LinkCommand::Play {
                    start_at: start_at.min(track_ids.len().saturating_sub(1)) as u32,
                    track_ids,
                    position_ms,
                    paused: !play,
                    handoff: false,
                }
            }
            PlayerCommand::RemoveFromPlaylist(id) => LinkCommand::RemoveItems {
                ids: vec![entry(id)],
            },
            PlayerCommand::RemoveFromPlaylistBatch(ids) => LinkCommand::RemoveItems {
                ids: ids.into_iter().map(entry).collect(),
            },
            PlayerCommand::MoveInPlaylist { id, target, after } => LinkCommand::MoveItems {
                ids: vec![entry(id)],
                target: entry(target),
                after,
            },
            PlayerCommand::MoveItemsInPlaylist { ids, target, after } => LinkCommand::MoveItems {
                ids: ids.into_iter().map(entry).collect(),
                target: entry(target),
                after,
            },
            PlayerCommand::ClearPlaylist => LinkCommand::Clear,
            PlayerCommand::Undo => LinkCommand::Undo,
            PlayerCommand::Redo => LinkCommand::Redo,
            // Following a playlist, undo batches: bookkeeping of this
            // device's own queue, which the other device keeps for itself.
            PlayerCommand::ReorderPlaylist(_)
            | PlayerCommand::BeginUndoBatch
            | PlayerCommand::EndUndoBatch => return Ok(()),
            // This device's own: its output, its renderer and its DSP, and
            // the player's own events. Named rather than caught by a wildcard,
            // so a command added later is placed here or mapped above, never
            // acted on locally by default while another device is controlled.
            local @ (PlayerCommand::Cue { .. }
            | PlayerCommand::PauseAndReport(_)
            | PlayerCommand::Barrier(_)
            | PlayerCommand::UpdatePaths(_)
            | PlayerCommand::TrackReady(_)
            | PlayerCommand::TrackStreamReady(_)
            | PlayerCommand::StreamProbed { .. }
            | PlayerCommand::TrackFailed(_)
            | PlayerCommand::CacheTracks(_)
            | PlayerCommand::DecodeFinished(_)
            | PlayerCommand::TrackQueued
            | PlayerCommand::SetOutputDevice(_)
            | PlayerCommand::ClearOutputDevice
            | PlayerCommand::ReloadDsp
            | PlayerCommand::RestartOutput
            | PlayerCommand::UseRenderer(_)
            | PlayerCommand::ResumeRenderer(_)
            | PlayerCommand::ResumeRendererMissed
            | PlayerCommand::ReleaseRenderer(_)
            | PlayerCommand::SetRendererVolume(_)
            | PlayerCommand::RestorePlayMode(_)
            | PlayerCommand::Renderer { .. }) => return self.send_local(local),
        };
        koan_core::remote::devices::send(to, link).map_err(|message| KoanError::Remote { message })
    }

    /// The server's ids for these library tracks, where they have one.
    fn remote_ids(&self, ids: &[i64]) -> HashMap<i64, String> {
        let Ok(db) = self.db() else {
            return HashMap::new();
        };
        queries::tracks_by_ids(&db.conn, ids)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| t.remote_id.map(|r| (t.id, r)))
            .collect()
    }

    /// Send to this device's player, whatever the app is controlling.
    /// The output profiles are chosen by: the renderer playing, by its UDN,
    /// or this device's own.
    fn dsp_device(&self) -> Option<String> {
        self.state
            .renderer()
            .map(|r| r.udn)
            .or_else(koan_core::audio::dsp::profiles::current_device)
    }

    /// Send the device being controlled a command of the account's own.
    fn command_target(
        &self,
        to: &str,
        cmd: koan_core::remote::link::LinkCommand,
    ) -> Result<(), KoanError> {
        koan_core::remote::devices::send(to, cmd).map_err(|message| KoanError::Player { message })
    }

    fn assign_dsp(&self, profile: Option<String>, device: &str) -> Result<(), KoanError> {
        koan_core::audio::dsp::profiles::assign(profile.as_deref(), device)
            .map_err(|message| KoanError::BadArgument { message })?;
        self.send_local(PlayerCommand::ReloadDsp)
    }

    fn save_dsp(
        &self,
        imported: koan_core::audio::dsp::import::Imported,
        name: Option<String>,
    ) -> Result<String, KoanError> {
        let name = koan_core::audio::dsp::profiles::save(imported, name.as_deref())
            .map_err(|message| KoanError::BadArgument { message })?;
        self.send_local(PlayerCommand::ReloadDsp)?;
        Ok(name)
    }

    fn send_local(&self, cmd: PlayerCommand) -> Result<(), KoanError> {
        self.tx.send(cmd).map_err(|e| KoanError::Player {
            message: e.to_string(),
        })
    }

    /// Wait until the player has applied and published every command sent to
    /// it so far, up to `within`. False when it has not answered by then.
    fn applied(&self, within: std::time::Duration) -> bool {
        let (reply, done) = crossbeam_channel::bounded(1);
        self.send_local(PlayerCommand::Barrier(reply)).is_ok() && done.recv_timeout(within).is_ok()
    }

    /// Resolve track IDs into playlist items. Skips IDs that aren't in the
    /// library. Remote ones download once the player has them — the download
    /// queue follows the playlist.
    ///
    /// One query for the rows and one config load for the batch. Doing either
    /// per track is what made adding an album — never mind an artist — take
    /// long enough to be worth a progress indicator.
    fn build_items(&self, db: &Database, track_ids: &[i64]) -> Vec<PlaylistItem> {
        let rows = queries::tracks_by_ids(&db.conn, track_ids).unwrap_or_default();
        koan_core::helpers::playlist_items_for_tracks(db, &rows)
    }

    /// Undo or redo a playlist edit: put it back, follow it from the queue if
    /// the queue is following, and tell everyone as any edit would.
    async fn step_playlist(self: Arc<Self>, playlist_id: i64, back: bool) -> Result<(), KoanError> {
        offload::offload(move || {
            let db = self.db()?;
            let locked = self.locked_to(&db, playlist_id);
            let history = &self.playlist_history;
            let moved = if back {
                history.undo(&db.conn, playlist_id)
            } else {
                history.redo(&db.conn, playlist_id)
            }
            .map_err(db_err)?;
            if moved {
                self.follow_playlist(&db, playlist_id, locked);
                self.bump_library();
                koan_core::playlists::push_to_remote(playlist_id);
            }
            Ok(())
        })
        .await
    }

    /// Evaluate smart playlists that are due (one, or every one), and have
    /// the queue follow any it is locked to whose contents moved.
    fn refresh_smart(&self, db: &Database, playlist_id: Option<i64>) {
        let lock = koan_core::playlists::queue_lock(db, &self.state);
        let changed = match playlist_id {
            Some(id) => queries::smart::refresh_if_due(&db.conn, id)
                .map(|moved| if moved { vec![id] } else { Vec::new() }),
            None => queries::smart::refresh_due(&db.conn, queries::LOCAL_USER),
        };
        match changed {
            Ok(changed) if !changed.is_empty() => {
                for id in changed {
                    let locked = lock == Some(koan_core::playlists::QueueLock::Playlist(id));
                    self.follow_playlist(db, id, locked);
                }
                self.bump_library();
            }
            Ok(_) => {}
            Err(e) => log::warn!("smart playlists not refreshed: {e}"),
        }
    }

    /// Whether the queue is still exactly this playlist.
    fn locked_to(&self, db: &Database, playlist_id: i64) -> bool {
        koan_core::playlists::queue_lock(db, &self.state)
            == Some(koan_core::playlists::QueueLock::Playlist(playlist_id))
    }

    /// Make the queue follow a playlist that has just been edited.
    ///
    /// Only when the queue was still exactly that playlist beforehand —
    /// `was_locked` is read *before* the edit lands, because afterwards the two
    /// no longer match and every queue would look diverged.
    ///
    /// The edit is applied to the queue rather than the queue being rebuilt
    /// from the playlist: entries that survived keep their queue items, and
    /// with them their ids, what has played, what is mid-download and what the
    /// cursor is pointing at.
    fn follow_playlist(&self, db: &Database, playlist_id: i64, was_locked: bool) {
        if !was_locked {
            return;
        }
        let Ok(entries) = queries::playlist_entries(&db.conn, playlist_id) else {
            return;
        };
        let (items, _) = self.state.snapshot_playlist();

        // Queue items whose entry has gone from the playlist.
        let live: std::collections::HashSet<i64> = entries.iter().map(|e| e.id).collect();
        let doomed: Vec<QueueItemId> = items
            .iter()
            .filter(|i| i.playlist_entry_id.is_none_or(|e| !live.contains(&e)))
            .map(|i| i.id)
            .collect();
        if !doomed.is_empty() {
            let _ = self.send_local(PlayerCommand::RemoveFromPlaylistBatch(doomed));
        }

        // Entries the queue has never seen.
        let known: std::collections::HashMap<i64, QueueItemId> = items
            .iter()
            .filter_map(|i| i.playlist_entry_id.map(|e| (e, i.id)))
            .collect();
        let missing: Vec<&queries::PlaylistEntry> = entries
            .iter()
            .filter(|e| !known.contains_key(&e.id))
            .collect();
        let mut added = std::collections::HashMap::new();
        if !missing.is_empty() {
            let track_ids: Vec<i64> = missing.iter().map(|e| e.track.id).collect();
            let mut new_items = self.build_items(db, &track_ids);
            for (item, entry) in new_items.iter_mut().zip(&missing) {
                item.playlist_entry_id = Some(entry.id);
                added.insert(entry.id, item.id);
            }
            if !new_items.is_empty() {
                let _ = self.send_local(PlayerCommand::AddToPlaylist(new_items));
            }
        }

        // Then the order, which is what puts the new arrivals in their places —
        // they were appended, because that is the only thing an add can do.
        let order: Vec<QueueItemId> = entries
            .iter()
            .filter_map(|e| known.get(&e.id).or_else(|| added.get(&e.id)).copied())
            .collect();
        if !order.is_empty() {
            let _ = self.send_local(PlayerCommand::ReorderPlaylist(order));
        }
    }

    /// What the server asked of this app over the link. Runs on the link's
    /// thread, which may block: resolving an id the library lacks syncs first,
    /// when `may_sync`.
    fn handle_link(
        &self,
        cmd: koan_core::remote::link::LinkCommand,
        source: koan_core::remote::link::CommandSource,
        pending: Option<koan_core::remote::acks::Pending>,
    ) {
        use koan_core::remote::acks::AckOutcome;
        let outcome = match self.run_link_command(cmd, source) {
            Ok(()) => AckOutcome::Done,
            Err(e) => {
                log::warn!("link: {e}");
                AckOutcome::Failed {
                    error: e.to_string(),
                }
            }
        };
        if let Some(pending) = pending {
            pending.finish(outcome);
        }
    }

    fn run_link_command(
        &self,
        cmd: koan_core::remote::link::LinkCommand,
        source: koan_core::remote::link::CommandSource,
    ) -> Result<(), KoanError> {
        use koan_core::remote::link::{CommandSource, LinkCommand};
        // Only the account may cost a sync: anyone on the network can send a
        // track id this library has never heard of.
        let may_sync = source == CommandSource::Account;
        // Resolving a track the library lacks syncs first; what that brought
        // in has to reach the pages too.
        let resolve_tracks = |db: &Database, ids: &[String]| {
            let (found, synced) = koan_core::remote::link::resolve_tracks(db, ids, may_sync);
            if synced {
                self.library_changed();
            }
            found
        };
        match cmd {
            LinkCommand::Play {
                track_ids,
                start_at,
                position_ms,
                paused,
                handoff,
            } => self.db().and_then(|db| {
                // The music is coming back here: stop controlling whatever
                // this was controlling. A renderer this device was playing to
                // is still its output, so the music resumes there.
                if handoff && koan_core::remote::devices::target().is_some() {
                    log::info!("link: handed the music; taking control back");
                    koan_core::remote::devices::set_target(None);
                }
                let ids = resolve_tracks(&db, &track_ids);
                let items = self.build_items(&db, &ids);
                if items.is_empty() {
                    log::warn!(
                        "link: none of {} tracks are in the library",
                        track_ids.len()
                    );
                    return Ok(());
                }
                let start = (start_at as usize).min(items.len() - 1);
                self.send_local(PlayerCommand::ReplacePlaylist {
                    items,
                    start,
                    position_ms,
                    play: !paused,
                })
            }),
            LinkCommand::PlayItem { id } => {
                let id = parse_qid(&id);
                let (items, _) = self.state.snapshot_playlist();
                match id {
                    Ok(id) if items.iter().any(|i| i.id == id) => {
                        self.send_local(PlayerCommand::Play(id))
                    }
                    _ => Ok(()),
                }
            }
            LinkCommand::RemoveItems { ids } => parse_qids(&ids)
                .and_then(|ids| self.send_local(PlayerCommand::RemoveFromPlaylistBatch(ids))),
            LinkCommand::MoveItems { ids, target, after } => parse_qids(&ids).and_then(|ids| {
                let target = parse_qid(&target)?;
                self.send_local(PlayerCommand::MoveItemsInPlaylist { ids, target, after })
            }),
            LinkCommand::Insert { track_ids, after } => self.db().and_then(|db| {
                let after = parse_qid(&after)?;
                let ids = resolve_tracks(&db, &track_ids);
                let items = self.build_items(&db, &ids);
                if items.is_empty() {
                    return Ok(());
                }
                self.send_local(PlayerCommand::InsertInPlaylist { items, after })?;
                Ok(())
            }),
            LinkCommand::Undo => self.send_local(PlayerCommand::Undo),
            LinkCommand::Redo => self.send_local(PlayerCommand::Redo),
            // Asked for by another device, which learns the outcome from
            // what the destination reports; here it is only logged, off the
            // lane, so later commands do not wait on it.
            LinkCommand::HandOff { to } => self.hand_off_blocking(&to, source).map(|sent| {
                let player = self.tx.clone();
                let _ = std::thread::Builder::new()
                    .name("koan-hand-off".into())
                    .spawn(move || {
                        sent.result(
                            || None,
                            || {
                                let _ = player.send(PlayerCommand::Resume);
                            },
                        )
                    });
            }),
            // Taken off the link before it gets here.
            // From a notification tapped or an outbox: the link's own
            // check, made again.
            LinkCommand::Shared { command } => {
                if command.allowed_playback() {
                    self.run_link_command(*command, CommandSource::Shared)?;
                }
                Ok(())
            }
            // Answered by the link session itself, which holds the watch.
            LinkCommand::Devices { .. }
            | LinkCommand::DeviceKeys { .. }
            | LinkCommand::Shares { .. }
            | LinkCommand::Forgotten { .. }
            | LinkCommand::WatchLevels { .. }
            | LinkCommand::Levels { .. }
            | LinkCommand::Acked { .. } => Ok(()),
            LinkCommand::SetOutput { output } => {
                koan_core::remote::outputs::set(output, koan_core::upnp::choose(), &self.tx)
                    .map_err(|message| KoanError::Audio { message })
            }
            LinkCommand::RefreshOutputs => {
                koan_core::remote::outputs::refresh_for_controller();
                Ok(())
            }
            LinkCommand::SetRendererVolume { volume } => {
                self.send_local(PlayerCommand::SetRendererVolume(volume))
            }
            LinkCommand::SetPreset { device, profile } => self.assign_dsp(profile, &device),
            LinkCommand::Enqueue { track_ids } => self.db().and_then(|db| {
                let ids = resolve_tracks(&db, &track_ids);
                let items = self.build_items(&db, &ids);
                let Some(first) = items.first().map(|i| i.id) else {
                    return Ok(());
                };
                let was_stopped = self.state.is_idle();
                self.send_local(PlayerCommand::AddToPlaylist(items))?;
                if was_stopped {
                    self.send_local(PlayerCommand::Play(first))?;
                }
                Ok(())
            }),
            LinkCommand::JumpTo { track_id } => self.db().and_then(|db| {
                let Some(&local) = resolve_tracks(&db, std::slice::from_ref(&track_id)).first()
                else {
                    log::warn!("link: track {track_id} is not in the library");
                    return Ok(());
                };
                let (items, cursor) = self.state.snapshot_playlist();
                if let Some(item) = items.iter().find(|i| i.db_id == Some(local)) {
                    return self.send_local(PlayerCommand::Play(item.id));
                }
                let mut new = self.build_items(&db, &[local]);
                let Some(item) = new.pop() else { return Ok(()) };
                let id = item.id;
                match cursor {
                    Some(after) => self.send_local(PlayerCommand::InsertInPlaylist {
                        items: vec![item],
                        after,
                    })?,
                    None => self.send_local(PlayerCommand::AddToPlaylist(vec![item]))?,
                }
                self.send_local(PlayerCommand::Play(id))?;
                Ok(())
            }),
            LinkCommand::PlayNext { track_ids } => self.db().and_then(|db| {
                let ids = resolve_tracks(&db, &track_ids);
                let items = self.build_items(&db, &ids);
                if items.is_empty() {
                    return Ok(());
                }
                match self.state.cursor() {
                    Some(after) => {
                        self.send_local(PlayerCommand::InsertInPlaylist { items, after })?
                    }
                    None => self.send_local(PlayerCommand::AddToPlaylist(items))?,
                }
                Ok(())
            }),
            LinkCommand::Remove { track_ids } => self.db().and_then(|db| {
                let ids: std::collections::HashSet<i64> =
                    resolve_tracks(&db, &track_ids).into_iter().collect();
                let (items, _) = self.state.snapshot_playlist();
                let gone: Vec<QueueItemId> = items
                    .iter()
                    .filter(|i| i.db_id.is_some_and(|id| ids.contains(&id)))
                    .map(|i| i.id)
                    .collect();
                if gone.is_empty() {
                    return Ok(());
                }
                self.send_local(PlayerCommand::RemoveFromPlaylistBatch(gone))
            }),
            LinkCommand::Clear => self.send_local(PlayerCommand::ClearPlaylist),
            LinkCommand::Evict { track_ids } => self.db().and_then(|db| {
                let ids = resolve_tracks(&db, &track_ids);
                let rows = queries::tracks_by_ids(&db.conn, &ids).unwrap_or_default();
                for path in rows.iter().filter_map(|t| t.cached_path.as_deref()) {
                    let _ = std::fs::remove_file(path);
                }
                queries::clear_cached_paths_for(&db.conn, &ids).map_err(db_err)?;
                log::info!("link: evicted {} cached tracks", ids.len());
                self.library_changed();
                Ok(())
            }),
            LinkCommand::HistoryChanged => self.db().map(|db| {
                // History and Recently played follow `player::history::changed`,
                // which the sync rings; a track it had to sync for is the
                // library's news too.
                if koan_core::remote::history::sync(&db).library_synced {
                    self.library_changed();
                }
            }),
            LinkCommand::DspProfilesChanged => self.db().and_then(|db| {
                if koan_core::remote::dsp_sync::sync(&db).changed() {
                    // Pages showing profiles follow the library's version.
                    self.library_changed();
                    self.send_local(PlayerCommand::ReloadDsp)?;
                }
                Ok(())
            }),
            LinkCommand::Sync { full } => self.db().map(|db| {
                let walk = if full {
                    koan_core::helpers::Walk::Always
                } else {
                    koan_core::helpers::Walk::IfChanged
                };
                koan_core::remote::link::sync(&db, walk);
                self.library_changed();
            }),
            LinkCommand::Seek { position_ms } => self.send_local(PlayerCommand::Seek(position_ms)),
            LinkCommand::Pause => self.send_local(PlayerCommand::Pause),
            LinkCommand::Resume => self.send_local(PlayerCommand::Resume),
            LinkCommand::Next => self.send_local(PlayerCommand::NextTrack),
            LinkCommand::Previous => self.send_local(PlayerCommand::PrevTrack),
            LinkCommand::Shuffle { on } => self.send_local(PlayerCommand::SetShuffle(on)),
            LinkCommand::Repeat { mode } => self.send_local(PlayerCommand::SetRepeat(mode)),
            LinkCommand::SleepTimer { timer } => {
                self.send_local(PlayerCommand::SetSleepTimer(timer))
            }
        }
    }

    /// Send this device's queue and playhead to `to` and pause here. The
    /// tracks the server does not know are left out, since the other device
    /// could not play them; returns how many.
    /// Send this device's music to `to`, asked by `source`: see
    /// `devices::send_for`, which keeps a hand-off asked by anyone but the
    /// account off the account's own devices.
    fn hand_off_blocking(
        &self,
        to: &str,
        source: koan_core::remote::link::CommandSource,
    ) -> Result<HandedOff, KoanError> {
        use koan_core::remote::link::LinkCommand;
        let (items, cursor) = self.state.snapshot_playlist();
        let remote = self.remote_ids(&items.iter().filter_map(|i| i.db_id).collect::<Vec<_>>());
        let at = cursor
            .and_then(|c| items.iter().position(|i| i.id == c))
            .unwrap_or(0);
        let kept: Vec<(usize, String)> = items
            .iter()
            .enumerate()
            .filter_map(|(n, i)| {
                i.db_id
                    .and_then(|id| remote.get(&id).cloned())
                    .map(|r| (n, r))
            })
            .collect();
        if kept.is_empty() {
            return Err(KoanError::BadArgument {
                message:
                    "nothing in the queue is on the server, so the other device cannot play it"
                        .into(),
            });
        }
        let start_at = kept.iter().position(|(n, _)| *n >= at).unwrap_or(0);
        // Silent here first, and the playhead read where it went silent:
        // anything heard while the command travels would be heard twice.
        let paused = self.state.playback_state() == PlaybackState::Paused;
        let (reply, silent) = crossbeam_channel::bounded(1);
        self.send_local(PlayerCommand::PauseAndReport(reply))?;
        let position_ms = silent
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap_or_else(|_| self.state.position_ms());
        let position_ms = if kept.get(start_at).is_some_and(|(n, _)| *n == at) {
            position_ms
        } else {
            0
        };
        let dropped = (items.len() - kept.len()) as u32;
        let first = kept[start_at].1.clone();
        let before = koan_core::remote::devices::last_report(to);
        let play = LinkCommand::Play {
            track_ids: kept.into_iter().map(|(_, r)| r).collect(),
            start_at: start_at as u32,
            position_ms,
            paused,
            handoff: true,
        };
        let (then, answer) = outcome_channel();
        let sent = koan_core::remote::devices::send_for_then(source, to, play, Some(then));
        if let Err(message) = sent {
            if !paused {
                self.send_local(PlayerCommand::Resume)?;
            }
            return Err(KoanError::Remote { message });
        }
        log::info!("devices: sent the queue to {to}, {dropped} left out");
        Ok(HandedOff {
            left_out: dropped,
            to: Some(to.to_owned()),
            track: Some(first),
            before,
            cursor_before: None,
            answer: Some(answer),
            answer_is_arrival: true,
            was_playing: !paused,
        })
    }

    /// The queue entry under this device's cursor, and the server's id for
    /// its track.
    fn under_cursor(&self) -> Option<(QueueItemId, Option<String>)> {
        let cursor = self.state.cursor()?;
        let remote = self
            .state
            .get_item(cursor)
            .and_then(|item| item.db_id)
            .and_then(|id| self.remote_ids(&[id]).remove(&id));
        Some((cursor, remote))
    }

    /// Rows arrived by some route the UI did not start; have its pages read
    /// them.
    fn library_changed(&self) {
        self.library_version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        koan_core::signal::engine_changed().bump();
    }

    /// Attach favourite state in one pass — one query per listing rather than
    /// one per row. Keyed by track ID because the favourites table stores
    /// whichever path a track happens to have, and a never-cached remote track
    /// only has a URL.
    fn decorate(&self, db: &Database, rows: Vec<queries::TrackRow>) -> Vec<Track> {
        let favs: HashSet<i64> =
            queries::favourite_track_ids_batch(&db.conn, queries::LOCAL_USER).unwrap_or_default();
        rows.into_iter()
            .map(|r| {
                let is_fav = favs.contains(&r.id);
                Track::from_row(r, is_fav)
            })
            .collect()
    }
}

/// Rebuild playlist items for a saved queue.
///
/// Anything still in the library is re-resolved so cache paths and downloads
/// are correct; anything that has since gone keeps the copy stored in the
/// snapshot, so a restore never silently drops tracks.
fn restore_items(db: &Database, saved: &[PersistedQueueItem]) -> Vec<PlaylistItem> {
    // By id first: a saved path names the container it was written in, which
    // iOS moves on every update, and a track still downloading when the session
    // was saved has no row with its path at all. The title guards against an
    // id SQLite has since reused for another track.
    let saved_ids: Vec<i64> = saved.iter().filter_map(|i| i.db_id).collect();
    let titles: HashMap<i64, String> = queries::tracks_by_ids(&db.conn, &saved_ids)
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.id, r.title))
        .collect();
    let ids: Vec<Option<i64>> = saved
        .iter()
        .map(|item| {
            item.db_id
                .filter(|id| titles.get(id) == Some(&item.title))
                .or_else(|| {
                    queries::track_id_by_path(&db.conn, &item.path)
                        .ok()
                        .flatten()
                })
        })
        .collect();

    let known: Vec<i64> = ids.iter().flatten().copied().collect();
    let rows = queries::tracks_by_ids(&db.conn, &known).unwrap_or_default();
    let mut resolved = koan_core::helpers::playlist_items_for_tracks(db, &rows).into_iter();

    saved
        .iter()
        .zip(ids)
        .map(|(saved_item, id)| match id.and_then(|_| resolved.next()) {
            Some(item) => PlaylistItem {
                pre_shuffle: saved_item.pre_shuffle,
                ..item
            },
            None => saved_item.to_playlist_item(),
        })
        .collect()
}

fn sort_rows(mut rows: Vec<queries::TrackRow>, sort: TrackSort) -> Vec<queries::TrackRow> {
    match sort {
        // Left alone deliberately. Every query already ORDERs BY album date,
        // title, disc and track — the coherent ordering, and better than
        // anything reconstructible here since TrackRow carries no release date.
        // Re-sorting on (disc, track) alone turned an artist's discography into
        // track 1 of every album, then track 2 of every album.
        TrackSort::Album => {}
        TrackSort::Title => rows.sort_by_key(|r| r.title.to_lowercase()),
        TrackSort::Artist => rows.sort_by_key(|r| r.artist_name.to_lowercase()),
        TrackSort::Duration => rows.sort_by_key(|r| r.duration_ms.unwrap_or(0)),
    }
    rows
}

/// A track's cover from disk: the image beside its file or the art embedded
/// in it (see `koan_core::index::folder_art`).
fn local_cover_art(row: &queries::TrackRow) -> Option<CoverArt> {
    let path = row.path.as_ref().or(row.cached_path.as_ref())?;
    let data = koan_core::index::folder_art::cover_art(Path::new(path))?;
    let mime = sniff_mime(&data).to_string();
    Some(CoverArt { data, mime })
}

fn sniff_mime(data: &[u8]) -> &'static str {
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

fn parse_qid(s: &str) -> Result<QueueItemId, KoanError> {
    Uuid::parse_str(s)
        .map(QueueItemId)
        .map_err(|_| KoanError::BadArgument {
            message: format!("not a queue item id: {s}"),
        })
}

fn parse_qids(ids: &[String]) -> Result<Vec<QueueItemId>, KoanError> {
    ids.iter().map(|s| parse_qid(s)).collect()
}

/// A search term the user typed, or nothing. Whitespace is not a
/// filter, and neither is an empty box.
fn trimmed(search: &Option<String>) -> Option<&str> {
    search.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// How many genres the browsers' genre filter offers, most common first.
const GENRES_OFFERED: u32 = 80;

/// Offline, every listing is narrowed to what can play here.
fn offline() -> bool {
    koan_core::remote::offline::active()
}

/// Nothing narrowed but, offline, to what can play here.
fn offline_filter() -> queries::AlbumFilter<'static> {
    queries::AlbumFilter {
        on_device: offline(),
        ..Default::default()
    }
}

fn album_filter(f: &BrowseFilter) -> queries::AlbumFilter<'_> {
    queries::AlbumFilter {
        lossless: f.lossless,
        codec: trimmed(&f.codec),
        year_from: f.year_from,
        year_to: f.year_to,
        genre: trimmed(&f.genre),
        on_device: f.downloaded || offline(),
    }
}

/// The browser's sort as an order the database can apply.
fn album_order(sort: AlbumSort, seed: i64) -> queries::AlbumOrder {
    match sort {
        AlbumSort::RecentlyAdded => queries::AlbumOrder::RecentlyAdded,
        AlbumSort::Title => queries::AlbumOrder::Title,
        AlbumSort::Artist => queries::AlbumOrder::ArtistThenDate,
        AlbumSort::Year => queries::AlbumOrder::YearDesc,
        AlbumSort::Random => queries::AlbumOrder::Random(seed),
        AlbumSort::LastPlayed => queries::AlbumOrder::LastPlayed,
        AlbumSort::Downloaded => queries::AlbumOrder::Downloaded,
        AlbumSort::BestMatch => queries::AlbumOrder::Relevance,
    }
}

/// The Recently Played shelf's window, when the filter asks for it.
fn recent(f: &BrowseFilter) -> Option<queries::PlayedSince> {
    use koan_core::shelves::{self, Shelf};
    f.recent
        .then(|| {
            Shelf::Recent
                .albums(queries::LOCAL_USER, shelves::now())
                .played
        })
        .flatten()
}

/// Rank `texts` against `query`, best first, and return the indices of the top
/// `limit`. Shared by every fuzzy listing so they rank identically.
fn fuzzy_rank(texts: &[&str], query: &str, limit: u32) -> Vec<usize> {
    use nucleo::pattern::{CaseMatching, Normalization};
    use nucleo::{Config as NucleoConfig, Nucleo};

    let mut nucleo: Nucleo<u32> = Nucleo::new(NucleoConfig::DEFAULT, Arc::new(|| {}), None, 1);
    let injector = nucleo.injector();
    for (i, text) in texts.iter().enumerate() {
        let text = text.to_string();
        injector.push(i as u32, |_val, cols| {
            cols[0] = text.into();
        });
    }

    // Case-insensitive: a phone keyboard capitalises the first letter, and
    // smart case would then read "Spfdj" as a request for that exact casing.
    nucleo
        .pattern
        .reparse(0, query, CaseMatching::Ignore, Normalization::Smart, false);
    // Until the matcher has seen every item. A fixed number of ticks let a
    // slow device snapshot a partial match.
    while nucleo.tick(10).running {}

    let snap = nucleo.snapshot();
    let count = (snap.matched_item_count() as usize).min(limit as usize);
    (0..count as u32)
        .filter_map(|i| snap.get_matched_item(i).map(|item| *item.data as usize))
        .collect()
}

/// Refuse an edit to the contents of a playlist that takes none.
fn fillable(db: &Database, playlist_id: i64) -> Result<(), KoanError> {
    match queries::get_playlist(&db.conn, playlist_id).map_err(db_err)? {
        Some(list) if list.readonly => Err(KoanError::BadArgument {
            message: format!(
                "'{}' is read-only: its rules, its file or its server decide what it holds",
                list.name
            ),
        }),
        Some(_) => Ok(()),
        None => Err(KoanError::NotFound {
            message: format!("playlist {playlist_id}"),
        }),
    }
}

fn db_err(e: impl std::fmt::Display) -> KoanError {
    KoanError::Database {
        message: e.to_string(),
    }
}

/// A pattern that can't be resolved, a library folder that isn't configured and
/// a file that won't move are all things the user can act on, so they come back
/// as bad arguments rather than as an opaque failure. Only the database going
/// wrong is out of their hands.
fn organize_err(e: koan_core::organize::OrganizeError) -> KoanError {
    use koan_core::organize::OrganizeError as E;
    let message = e.to_string();
    match e {
        E::Db(_) | E::Sqlite(_) => KoanError::Database { message },
        _ => KoanError::BadArgument { message },
    }
}

fn fav_err(e: rusqlite::Error) -> KoanError {
    KoanError::Database {
        message: e.to_string(),
    }
}

fn current_entry(
    state: &koan_core::remote::link::LinkState,
) -> Option<&koan_core::remote::link::LinkQueueEntry> {
    state.queue.iter().find(|e| e.current)
}

fn play_state(state: &koan_core::remote::link::LinkState) -> PlayState {
    if state.playing {
        PlayState::Playing
    } else if state.title.is_some() {
        PlayState::Paused
    } else {
        PlayState::Stopped
    }
}

/// The server and the network, for Settings.
fn connection_info() -> ConnectionInfo {
    use koan_core::remote::{devices, nearby, profile};
    let p = profile::current();
    ConnectionInfo {
        server_kind: p.as_ref().and_then(|p| p.kind.clone()),
        server_version: p.as_ref().and_then(|p| p.version.clone()),
        open_subsonic: p.as_ref().is_some_and(|p| p.open_subsonic),
        extensions: p
            .as_ref()
            .map(|p| {
                p.extensions
                    .iter()
                    .map(|(name, versions)| ServerExtension {
                        name: name.clone(),
                        versions: versions.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        devices: p.as_ref().is_some_and(|p| p.offers(profile::DEVICES)),
        linked: devices::linked(),
        listening_port: nearby::listening_port(),
        local_network_blocked: nearby::local_network_blocked(),
        this_device: devices::local()
            .map(|l| l.identity.name.clone())
            .unwrap_or_default(),
        sharing: p.as_ref().is_some_and(|p| p.offers(profile::SHARES)),
        pairing: p.as_ref().is_some_and(|p| p.offers(profile::PAIR)),
        scrobbling: p.as_ref().is_some_and(|p| p.offers(profile::SCROBBLING)),
        shared_with: devices::shares(),
        share_error: devices::share_error(),
        share_accounts: devices::accounts(),
        offline: koan_core::remote::offline::active(),
        offline_manual: koan_core::remote::offline::manual(),
        sign_in_refused: koan_core::helpers::sign_in_refused(&Config::cached()),
        nearby_servers: nearby::servers()
            .into_iter()
            .map(|(url, devices)| NearbyServer { url, devices })
            .collect(),
        command_notice: devices::notice().map(|n| {
            use koan_core::remote::acks::AckOutcome;
            CommandNotice {
                seq: n.seq,
                device: n.device,
                queued: n.outcome == AckOutcome::Queued,
                detail: match n.outcome {
                    AckOutcome::Refused { reason } => reason,
                    AckOutcome::Failed { error } => error,
                    AckOutcome::Done | AckOutcome::Queued => String::new(),
                },
            }
        }),
    }
}

/// Who saved the server's play queue, as a person would name it: a kōan
/// device by its name, another client by what it calls itself.
fn saved_by(changed_by: &str) -> String {
    match changed_by.strip_prefix("koan ") {
        Some(id) if koan_core::remote::devices::this_id().as_deref() == Some(id) => {
            "this device".to_owned()
        }
        Some(id) => koan_core::remote::devices::list()
            .into_iter()
            .find(|d| d.id == id)
            .map_or_else(|| "another kōan device".to_owned(), |d| d.name),
        None if changed_by == "koan" => "a kōan device".to_owned(),
        None => changed_by.to_owned(),
    }
}

/// The queue as the server is told it, with each track's id on the server. At
/// most `LINK_QUEUE_MAX` entries, from a few before the current one.
fn link_queue(state: &SharedPlayerState) -> Vec<koan_core::remote::link::LinkQueueEntry> {
    const LINK_QUEUE_MAX: usize = 300;
    let (window, cursor) = state.playlist_window(20, LINK_QUEUE_MAX);

    let remote: std::collections::HashMap<i64, String> = koan_core::db::pool::shared()
        .get()
        .ok()
        .and_then(|db| {
            let ids: Vec<i64> = window.iter().filter_map(|i| i.db_id).collect();
            queries::tracks_by_ids(&db.conn, &ids).ok()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| t.remote_id.map(|r| (t.id, r)))
        .collect();

    window
        .iter()
        .map(|i| koan_core::remote::link::LinkQueueEntry {
            id: Some(i.id.0.to_string()),
            track_id: i.db_id.and_then(|id| remote.get(&id).cloned()),
            title: i.title.clone(),
            artist: i.artist.clone(),
            album: i.album.clone(),
            duration_ms: i.duration_ms.unwrap_or(0),
            current: Some(i.id) == cursor,
        })
        .collect()
}

/// The account client, on a server whose invites this app can open. Checked
/// before anything is made: a server older than invite tokens would create
/// the account and answer with nothing this app reads.
fn invite_client() -> Result<Arc<koan_core::remote::client::SubsonicClient>, KoanError> {
    let client = account_client()?;
    let offers = koan_core::remote::profile::for_auth(client.auth())
        .is_some_and(|p| p.offers(koan_core::remote::profile::INVITE));
    if !offers {
        return Err(KoanError::BadArgument {
            message: "this server is older than this app: update it to invite people".into(),
        });
    }
    Ok(client)
}

/// The signed-in server's client, when it lists and makes API keys.
fn api_keys_client() -> Result<Arc<koan_core::remote::client::SubsonicClient>, KoanError> {
    let client = account_client()?;
    let offers = koan_core::remote::profile::for_auth(client.auth())
        .is_some_and(|p| p.offers(koan_core::remote::profile::API_KEYS));
    if !offers {
        return Err(KoanError::BadArgument {
            message: "this server is older than this app: update it to manage API keys here".into(),
        });
    }
    Ok(client)
}

/// The signed-in server's client, when it sets passwords.
fn passwords_client() -> Result<Arc<koan_core::remote::client::SubsonicClient>, KoanError> {
    let client = account_client()?;
    let offers = koan_core::remote::profile::for_auth(client.auth())
        .is_some_and(|p| p.offers(koan_core::remote::profile::PASSWORDS));
    if !offers {
        return Err(KoanError::BadArgument {
            message: "this server is older than this app: update it to change passwords here"
                .into(),
        });
    }
    Ok(client)
}

/// The invite a koan server answered with, as a link to the address this
/// client reaches it at.
fn account_invite(
    server: &str,
    made: koan_core::remote::client::KoanInvite,
) -> koan_core::invite::Invite {
    koan_core::invite::Invite::with_token(
        server,
        &made.username,
        &made.token,
        made.password.as_deref(),
    )
}

fn account_client() -> Result<Arc<koan_core::remote::client::SubsonicClient>, KoanError> {
    koan_core::helpers::subsonic_client(&Config::load().unwrap_or_default()).ok_or_else(|| {
        KoanError::BadArgument {
            message: "no remote server configured".into(),
        }
    })
}

/// A pairing that was turned away or lapsed is an answer; a connection that
/// failed is worth retrying.
fn pair_error(e: koan_core::remote::pair::PairError) -> KoanError {
    use koan_core::remote::pair::PairError;
    match e {
        PairError::Remote(e) => remote_error(e),
        e @ PairError::Unsupported => KoanError::NotFound {
            message: e.to_string(),
        },
        e @ (PairError::Connect(_) | PairError::Closed) => KoanError::Remote {
            message: e.to_string(),
        },
        e => KoanError::BadArgument {
            message: e.to_string(),
        },
    }
}

fn scrobbling_connection(
    s: koan_core::remote::client::KoanScrobbleService,
) -> ScrobblingConnection {
    ScrobblingConnection {
        account: s.account,
        pending: s.pending,
        error: s.error,
    }
}

fn scrobbling_error(e: koan_core::remote::scrobbling::ScrobblingError) -> KoanError {
    use koan_core::remote::scrobbling::ScrobblingError;
    match e {
        ScrobblingError::Remote(e) => remote_error(e),
        e @ ScrobblingError::NotSignedIn => KoanError::BadArgument {
            message: e.to_string(),
        },
    }
}

/// A server that answered and refused is a bad request; one that did not
/// answer is worth retrying.
fn remote_error(e: SubsonicError) -> KoanError {
    match e {
        SubsonicError::Api { message, .. } => KoanError::BadArgument { message },
        e => KoanError::Remote {
            message: e.to_string(),
        },
    }
}

#[cfg(test)]
mod hand_off_tests {
    use super::*;

    fn handed(answer: Option<koan_core::remote::acks::AckOutcome>, is_arrival: bool) -> HandedOff {
        let (then, rx) = outcome_channel();
        then(answer);
        HandedOff {
            left_out: 0,
            to: Some("phone".into()),
            track: None,
            before: None,
            cursor_before: None,
            answer: Some(rx),
            answer_is_arrival: is_arrival,
            was_playing: true,
        }
    }

    /// The Play's own answer says how a hand-off went: there, waiting for a
    /// device asleep, or refused, when the music plays on here.
    #[test]
    fn a_hand_off_goes_by_the_plays_answer() {
        use koan_core::remote::acks::AckOutcome;
        let resumed = std::cell::Cell::new(0);
        let resume = || resumed.set(resumed.get() + 1);

        let there = handed(Some(AckOutcome::Done), true).result(|| None, resume);
        assert!(there.started && !there.queued && there.error.is_none());

        let asleep = handed(Some(AckOutcome::Queued), true).result(|| None, resume);
        assert!(!asleep.started && asleep.queued);
        assert_eq!(resumed.get(), 0, "paused, for the device to take on waking");

        let refused = handed(
            Some(AckOutcome::Refused {
                reason: "not allowed".into(),
            }),
            true,
        )
        .result(|| None, resume);
        assert_eq!(refused.error.as_deref(), Some("not allowed"));
        assert_eq!(resumed.get(), 1, "plays on here");

        // A device that sent it on said only that it did: not that it arrived.
        let sent_on = handed(Some(AckOutcome::Done), false).result(|| None, resume);
        assert!(!sent_on.started);
    }

    /// "Move here" after this device handed the music on: its own queue is
    /// still there, cursor on the track it would be sent back. That is not
    /// the music arriving; a new entry for the track is.
    #[test]
    fn music_moved_here_arrives_only_in_a_new_entry() {
        let kept = QueueItemId(uuid::Uuid::now_v7());
        let arrived = QueueItemId(uuid::Uuid::now_v7());
        let t = Some("t".to_string());
        assert!(!arrived_here(Some(kept), Some((kept, t.clone())), "t"));
        assert!(!arrived_here(Some(kept), None, "t"));
        assert!(!arrived_here(
            Some(kept),
            Some((arrived, Some("u".into()))),
            "t"
        ));
        assert!(arrived_here(Some(kept), Some((arrived, t.clone())), "t"));
        assert!(arrived_here(None, Some((arrived, t)), "t"));
    }
}

#[cfg(test)]
mod fuzzy_tests {
    use super::fuzzy_rank;

    #[test]
    fn fuzzy_rank_ignores_case() {
        let texts = [
            "Soulwax — Most of the remixes we've made for other people",
            "SPFDJ",
        ];
        let ranked = fuzzy_rank(&texts, "Spfdj", 10);
        assert_eq!(ranked.first(), Some(&1));
    }

    #[test]
    fn fuzzy_rank_sees_every_item() {
        let mut texts: Vec<String> = (0..20_000).map(|i| format!("Filler artist {i}")).collect();
        texts.push("SPFDJ".into());
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        assert_eq!(fuzzy_rank(&texts, "spfdj", 5), vec![20_000]);
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    /// What the apps' cover cache is handed for a record whose art is only a
    /// `folder.png` beside its files.
    #[test]
    fn the_apps_get_the_image_beside_a_track() {
        let dir = tempfile::tempdir().unwrap();
        let album = dir.path().join("Album").join("CD1");
        std::fs::create_dir_all(&album).unwrap();
        let track = album.join("01.flac");
        std::fs::write(&track, b"no tags").unwrap();
        let png = b"\x89PNG\r\n\x1a\nimage".to_vec();
        std::fs::write(dir.path().join("Album").join("Folder.png"), &png).unwrap();

        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let id = super::restore_tests::track(&db, "One", &track);
        let row = queries::get_track_row(&db.conn, id).unwrap().unwrap();

        let art = local_cover_art(&row).expect("the folder image");
        assert_eq!(art.data, png);
        assert_eq!(art.mime, "image/png");
    }
}

#[cfg(test)]
mod restore_tests {
    use super::*;

    pub(super) fn track(db: &Database, title: &str, path: &Path) -> i64 {
        let meta = queries::TrackMeta {
            title: title.into(),
            artist: "Artist".into(),
            album_artist: Some("Artist".into()),
            album: "Album".into(),
            date: Some("2024".into()),
            disc: Some(1),
            track_number: Some(1),
            genre: None,
            label: None,
            duration_ms: Some(240_000),
            codec: Some("FLAC".into()),
            sample_rate: Some(44100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate: None,
            size_bytes: None,
            mtime: None,
            path: Some(path.to_string_lossy().into_owned()),
            source: "local".into(),
            remote_id: None,
            album_remote_id: None,
            artist_remote_id: None,
            mbid: None,
            album_mbid: None,
            remote_url: None,
            album_added_at: None,
        };
        queries::upsert_track(&db.conn, &meta).unwrap()
    }

    fn saved(title: &str, path: &str, db_id: Option<i64>) -> PersistedQueueItem {
        PersistedQueueItem {
            path: path.into(),
            title: title.into(),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: "Album".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            db_id,
            pre_shuffle: None,
        }
    }

    #[test]
    fn a_saved_queue_restores_by_id_when_its_paths_have_moved() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("koan.db")).unwrap();
        let file = dir.path().join("song.flac");
        std::fs::write(&file, b"audio").unwrap();
        let id = track(&db, "Song", &file);

        let stale = "/var/mobile/Containers/Data/Application/OLD/Library/Caches/koan/a.flac";
        let items = restore_items(
            &db,
            &[
                saved("Song", stale, Some(id)),
                // An id SQLite has since given to another track.
                saved("Other", stale, Some(id)),
                saved("Gone", stale, Some(id + 1000)),
            ],
        );

        assert_eq!(items.len(), 3, "one item per saved entry, in order");
        assert_eq!(items[0].path, file, "found by id, path re-resolved");
        assert_eq!(items[0].db_id, Some(id));
        assert_eq!(
            items[1].path,
            PathBuf::from(stale),
            "title mismatch: kept as saved"
        );
        assert_eq!(
            items[2].path,
            PathBuf::from(stale),
            "unknown id: kept as saved"
        );
    }
}

fn dsp_error(e: koan_core::audio::dsp::import::ImportError) -> KoanError {
    use koan_core::audio::dsp::import::ImportError;
    match e {
        ImportError::NeedsRate(_) => KoanError::NeedsSampleRate {
            message: e.to_string(),
        },
        ImportError::Failed(message) => KoanError::BadArgument { message },
    }
}
