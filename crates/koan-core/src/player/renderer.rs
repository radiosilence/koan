//! Playing to a UPnP renderer, passed the original file.
//!
//! Two things are kept apart. Who drives the renderer's transport is the
//! `RendererLink` on the player: the control connection, its events and the
//! position it reports, the same whatever the renderer is sent. What it is
//! sent is the session's output: `Output::Passthrough`, the original file,
//! decoded by the renderer, with no decoder or ring here. A processed stream
//! (#642) would be a local output whose engine encodes for the renderer,
//! driven by the same link.
//!
//! The renderer is chosen on the player (`Player::renderer`), and every
//! session opened while it is set plays there. The transport, its run and its track are the same as for
//! local output, and `publish` derives what clients see from them as it does
//! for any session.
//!
//! What differs is where a track goes and where the playhead is read from. A
//! track is handed over as a URL to the original file. The playhead is the
//! renderer's, extrapolated from where it was last heard to be. A track ends
//! when the renderer moves on to the next one it was given, or stops at the
//! end of this one; either way the player carries on as it would at the end
//! of a decode. A renderer is handed whole files only: a track still
//! downloading waits as any track not yet on disk does.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::state::{ItemState, QueueItemId, RendererClock, TrackInfo};
use super::{Output, Player, PlayerError, Run, Session, Source, Transport, media_extension};
use crate::audio::buffer;
use crate::remote::client::PlaybackReportState;
use crate::upnp::{self, Connection, didl, session};

/// How near its end a track that stops counts as finished rather than
/// stopped by someone at the renderer. Covers the drift of the extrapolated
/// clock and how late the event arrives.
const END_TOLERANCE_MS: u64 = 5_000;

/// Slack either side of a renderer's reading before the clock follows it:
/// the reading is a round trip old by the time it is compared.
const READING_SLACK_MS: u64 = 250;

/// After a seek or a load, how long the renderer may go on reporting where it
/// was: most acknowledge a seek before they have moved.
const SETTLE: Duration = Duration::from_secs(2);

/// When to look once more after a seek or a load, past the settle window.
const RECHECK: Duration = Duration::from_secs(6);

/// How near the target a renderer must be for a seek to count as done.
const ON_TARGET_MS: u64 = 1_500;

/// After koan loads or plays a track, how long the renderer's answers may
/// still describe what it was doing before. Several report `STOPPED` while
/// they fetch the start of the file, and Kodi goes on naming the previous URL
/// as playing until the new one has opened.
const START_GRACE: Duration = Duration::from_secs(3);

/// Between asking a renderer again: long enough for it to finish what it is
/// doing, which is usually opening a file.
const RESEND_GAP: Duration = Duration::from_millis(700);
const MAX_RESENDS: u8 = 4;

/// How long after asking a renderer to play or pause koan holds to what it
/// asked, against an answer that says otherwise.
const INTENT_HOLD: Duration = Duration::from_secs(4);

/// The renderer chosen as the output: the open connection that drives its
/// transport and hears what it does, which outlives the sessions played on
/// it. Its watcher (`upnp::session`) is where the renderer's position is
/// read, for any session on it.
pub(super) struct RendererLink {
    session: upnp::session::Session,
    /// The number of the player session now playing here, which the
    /// connection stamps on every event it sends.
    tag: Arc<AtomicU64>,
    /// Tracks this renderer cannot play, marked failed in the queue for as
    /// long as it is the output.
    refused: Vec<QueueItemId>,
}

/// One session passed to the renderer as the original file: what it holds,
/// and what koan is waiting for it to do.
pub(super) struct Passthrough {
    /// What the renderer was told to play.
    current: Slot,
    /// Still playing what koan gave it. False once it stopped at its own
    /// controls or was taken over: resuming loads the track again.
    loaded: bool,
    /// What it was told to play next, through `SetNextAVTransportURI`.
    next: Option<Slot>,
    /// A next track the renderer refused, not offered again: some advertise
    /// the action and fault on it.
    next_refused: Option<QueueItemId>,
    /// Not before this: it refused the next track while still opening the
    /// current one.
    next_retry: Option<Instant>,
    /// Where the track should be once the renderer is playing it. Sent when
    /// it first says it is playing: several ignore a seek while they are
    /// still opening the file, and differ on whether they seek a stopped or
    /// paused transport, but all of them seek a playing one.
    pending_seek: Option<u64>,
    /// Seeks sent towards `pending_seek`: one before `Play`, one as soon as it
    /// plays somewhere else, one more once that has settled, then koan gives
    /// up and follows it.
    seek_tries: u8,
    /// Its position readings are not followed before this: see `SETTLE`.
    settle_until: Instant,
    /// When to ask it where it is without waiting for an event: the end of a
    /// settle window, whose readings were set aside. Renderers send nothing
    /// once they have settled, so without asking, a seek they ignored while
    /// opening the file would never be noticed.
    look_at: Vec<Instant>,
    /// When koan last told it to play.
    started: Instant,
    /// Told to play since the track was loaded. Until it has been, a renderer
    /// is meant to say `STOPPED`, and saying so means nothing.
    played: bool,
    /// When koan last asked it to play or pause; what it asked is the
    /// session's run. A renderer still opening a file drops commands, so for
    /// `INTENT_HOLD` one that contradicts the request is asked again.
    asked: Asked,
}

struct Asked {
    at: Instant,
    resent: Instant,
    resends: u8,
}

struct Slot {
    id: QueueItemId,
    token: String,
    path: PathBuf,
}

/// The extension a file should be served under: what its first bytes say
/// it is, or its own extension when they say nothing.
fn container_of(path: &Path) -> String {
    didl::sniff_extension(path)
        .map(str::to_string)
        .or_else(|| media_extension(path))
        .unwrap_or_default()
}

impl Passthrough {
    fn new(current: Slot, seek_ms: u64) -> Self {
        let now = Instant::now();
        Self {
            current,
            loaded: true,
            next: None,
            next_refused: None,
            next_retry: None,
            pending_seek: (seek_ms > 0).then_some(seek_ms),
            seek_tries: 0,
            settle_until: now,
            look_at: Vec::new(),
            started: now,
            played: false,
            asked: Asked {
                at: now,
                resent: now,
                resends: 0,
            },
        }
    }

    /// Koan has just asked it to play or pause. An answer in the next moment
    /// predates the renderer acting on it, and is no reason to ask again.
    fn asked_now(&mut self) {
        let now = Instant::now();
        self.asked = Asked {
            at: now,
            resent: now,
            resends: 0,
        };
        self.look_at.push(now + RESEND_GAP);
    }

    /// The renderer says it is `reported` (playing or not) against `wanted`,
    /// what koan asked a moment ago. For `INTENT_HOLD` koan's request stands:
    /// the renderer is asked again every `RESEND_GAP`, and looked at again
    /// after each, while its contrary answers are set aside. `true` while
    /// that holds; after it, the renderer's word is taken, since someone may
    /// have used its own controls.
    fn insist(&mut self, link: &RendererLink, reported: bool, wanted: bool) -> bool {
        if reported == wanted || self.asked.at.elapsed() > INTENT_HOLD {
            return false;
        }
        if self.asked.resent.elapsed() >= RESEND_GAP && self.asked.resends < MAX_RESENDS {
            self.asked.resends += 1;
            self.asked.resent = Instant::now();
            log::info!(
                "upnp: renderer is {} against what was asked; asking again",
                if reported { "playing" } else { "not playing" }
            );
            let _ = if wanted {
                self.played = true;
                link.session.play()
            } else {
                link.session.pause()
            };
            self.look_at.push(Instant::now() + RESEND_GAP);
        }
        true
    }

    /// Set its readings aside for `SETTLE`, and look again once it is over
    /// and once more a little later: a renderer that rebuffers after a seek
    /// loses time without saying so.
    fn settle(&mut self) {
        let now = Instant::now();
        self.settle_until = now + SETTLE;
        self.look_at = vec![
            self.settle_until + Duration::from_millis(100),
            now + RECHECK,
        ];
    }

    fn tokens(&self) -> Vec<&str> {
        std::iter::once(&self.current)
            .filter(|_| self.loaded)
            .chain(self.next.iter())
            .map(|s| s.token.as_str())
            .collect()
    }
}

impl Player {
    /// The renderer, and the session playing on it, when there is one.
    fn on_renderer(&mut self) -> Option<(&RendererLink, &mut Session)> {
        let link = self.renderer.as_ref()?;
        match &mut self.transport {
            Transport::Loaded(session) if matches!(session.output, Output::Passthrough(_)) => {
                Some((link, session))
            }
            _ => None,
        }
    }

    fn passthrough(&self) -> Option<&Passthrough> {
        match &self.session()?.output {
            Output::Passthrough(play) => Some(play.as_ref()),
            Output::Local(_) => None,
        }
    }

    #[cfg(test)]
    fn passthrough_mut(&mut self) -> Option<&mut Passthrough> {
        match &mut self.transport {
            Transport::Loaded(Session {
                output: Output::Passthrough(play),
                ..
            }) => Some(play.as_mut()),
            _ => None,
        }
    }

    /// Whether the renderer holds the track, playing or paused.
    #[cfg(test)]
    pub(super) fn renderer_loaded(&self) -> bool {
        self.passthrough().is_some_and(|p| p.loaded)
    }

    /// When the player loop should wake for the renderer: see `look_at`.
    pub(super) fn renderer_deadline(&self) -> Option<Instant> {
        self.passthrough()
            .and_then(|p| p.look_at.iter().min().copied())
    }

    /// What the player loop does for the renderer on every pass: ask where it
    /// is once a settle window ends, and keep the next track handed over.
    pub(super) fn renderer_tick(&mut self) {
        // Read on the player's own thread, not waited for as an event: the
        // loss can be found by a command this thread is running.
        if self.renderer.as_ref().is_some_and(|l| l.session.is_lost()) {
            self.renderer_gone();
            return;
        }
        if let Some((link, session)) = self.on_renderer()
            && let Output::Passthrough(play) = &mut session.output
        {
            let now = Instant::now();
            let due = play.look_at.len();
            play.look_at.retain(|at| *at > now);
            if play.look_at.len() < due {
                link.session.look();
            }
        }
        self.queue_next_on_renderer();
    }

    /// Make `connection`'s renderer the output, or this device with `None`,
    /// and carry on from where the music is, as it was: playing on, paused,
    /// or still waiting for its download.
    pub(super) fn use_renderer(&mut self, connection: Option<Box<Connection>>) {
        // Picked again while it plays: nothing to do, and reconnecting would
        // stop it and load the track again.
        if let (Some(new), Some(link)) = (&connection, &self.renderer)
            && new.session.renderer().udn == link.session.renderer().udn
            && !link.session.is_lost()
        {
            return;
        }
        let carry = self
            .session()
            .map(|s| (s.track.id, self.shared_state.position_ms(), s.run));
        log::info!(
            "upnp: switching output{} to {}",
            carry.map_or(String::new(), |(_, at, run)| format!(
                " at {at}ms, {}",
                if run == Run::Playing {
                    "playing"
                } else {
                    "paused"
                }
            )),
            connection
                .as_ref()
                .map_or("this device", |c| c.session.renderer().name.as_str())
        );

        self.stop_engine();
        self.leave_renderer();

        if let Some(connection) = connection {
            let Connection { session, tag } = *connection;
            log::info!("upnp: playing to {}", session.renderer().name);
            self.shared_state.set_renderer(Some(upnp::Output {
                udn: session.renderer().udn.clone(),
                name: session.renderer().name.clone(),
                volume: session.volume(),
                problem: None,
            }));
            self.renderer = Some(RendererLink {
                session,
                tag,
                refused: Vec::new(),
            });
        }

        if let Some((id, position_ms, run)) = carry {
            self.cue(id, position_ms, run);
        }
    }

    /// Open a session on the renderer: `try_open_session` for the renderer
    /// output. Only a file on disk opens here.
    pub(super) fn try_open_on_renderer(
        &mut self,
        id: QueueItemId,
        source: Source,
        info: Option<buffer::StreamInfo>,
        seek_ms: u64,
        start: Run,
    ) -> Result<(), PlayerError> {
        self.stop_engine();
        let Source::File(path) = source else {
            return Err(PlayerError::Renderer(
                "a renderer is handed whole files only".into(),
            ));
        };
        let extension = container_of(&path);
        let link = self.renderer.as_mut().expect("caller checked");
        let Some(mime) = link.session.mime_for(&extension) else {
            let reason = format!(
                "{} cannot play {}",
                link.session.renderer().name,
                extension.to_uppercase()
            );
            log::info!("upnp: skipping {id:?}: {reason}");
            link.refused.push(id);
            self.shared_state
                .update_item_state(id, ItemState::Failed(reason.clone()));
            self.shared_state
                .update_renderer(|o| o.problem = Some(reason.clone()));
            return Err(PlayerError::Unplayable(reason));
        };

        let info = match info {
            Some(info) => info,
            None => buffer::probe_file(&path)?,
        };
        // What the renderer does with the samples is its own business; there
        // is no device rate here to compare against.
        self.shared_state.clear_output_sample_rate();
        self.shared_state.set_position_ms(seek_ms);
        self.on_track_changed(id, seek_ms);
        self.timeline.reset();

        let link = self.renderer.as_ref().expect("caller checked");
        let (token, url, art) = link.session.serve(&path, mime, &extension);
        let metadata = self.didl_for(id, &path, &url, &art, mime, info.duration_ms);
        self.session += 1;
        let link = self.renderer.as_ref().expect("caller checked");
        link.tag.store(self.session, Ordering::Release);
        link.session.retain(&[token.as_str()]);
        log::info!(
            "upnp: {} ← {} ({:?}) {}{}",
            link.session.renderer().name,
            path.display(),
            id,
            mime,
            if seek_ms > 0 {
                format!(" @{seek_ms}ms")
            } else {
                String::new()
            }
        );
        if let Err(e) = link.session.set_uri(&url, &metadata) {
            return self.renderer_failed_to_open(id, path, info, seek_ms, start, e);
        }

        let mut play = Passthrough::new(
            Slot {
                id,
                token,
                path: path.clone(),
            },
            seek_ms,
        );
        play.asked_now();
        if start == Run::Playing {
            // Seeked before it plays, so a renderer that takes it starts in
            // the right place and none of the top is heard. One that ignores
            // it is seeked again once it plays.
            if seek_ms > 0 {
                let _ = link.session.seek(seek_ms);
                play.seek_tries = 1;
            }
            if let Err(e) = link.session.play() {
                return self.renderer_failed_to_open(id, path, info, seek_ms, start, e);
            }
            play.played = true;
            play.started = Instant::now();
            play.settle();
        }
        link.session.look();
        self.shared_state.update_renderer(|o| o.problem = None);
        // Held where it opens until the renderer says it is playing: several
        // take a second or more to start, and a bar that ran from the command
        // would have to be pulled back when the sound began.
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: seek_ms,
            running: None,
        }));
        self.transport = Transport::Loaded(Session {
            track: TrackInfo {
                id,
                path,
                codec: info.codec,
                sample_rate: info.sample_rate,
                bit_depth: info.bit_depth,
                bitrate_kbps: info.bitrate_kbps,
                channels: info.channels,
                duration_ms: info.duration_ms,
            },
            run: start,
            lookahead: Default::default(),
            output: Output::Passthrough(Box::new(play)),
        });
        self.queue_next_on_renderer();
        Ok(())
    }

    /// A track could not be handed to the renderer. One that is gone is left
    /// here, as `renderer_gone` leaves it: the track opens on this device,
    /// paused, where it was to open. Any other failure fails the open.
    fn renderer_failed_to_open(
        &mut self,
        id: QueueItemId,
        path: PathBuf,
        info: buffer::StreamInfo,
        seek_ms: u64,
        start: Run,
        error: upnp::soap::SoapError,
    ) -> Result<(), PlayerError> {
        if !self.renderer.as_ref().is_some_and(|l| l.session.is_lost()) {
            return Err(PlayerError::Renderer(error.to_string()));
        }
        log::info!("upnp: renderer gone while opening {id:?}; carrying on here, paused");
        self.leave_renderer();
        if start == Run::Playing {
            self.report(PlaybackReportState::Paused);
        }
        self.try_open_session(id, Source::File(path), Some(info), seek_ms, Run::Paused)
    }

    /// Stop playing to the renderer: forget the link and put back what it
    /// refused. Whatever is open on it must already have been stopped.
    fn leave_renderer(&mut self) {
        if let Some(old) = self.renderer.take() {
            log::info!("upnp: leaving {}", old.session.renderer().name);
            for id in old.refused {
                self.shared_state.update_item_state(id, ItemState::Ready);
            }
        }
        self.shared_state.set_renderer_clock(None);
        self.shared_state.set_renderer(None);
    }

    fn didl_for(
        &self,
        id: QueueItemId,
        path: &Path,
        url: &str,
        art: &str,
        mime: &str,
        duration_ms: u64,
    ) -> String {
        let item = self.shared_state.get_item(id);
        let title = item
            .as_ref()
            .map(|i| i.title.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let artist = item.as_ref().map(|i| i.artist.clone()).unwrap_or_default();
        let album = item.as_ref().map(|i| i.album.clone()).unwrap_or_default();
        didl::didl(&didl::Item {
            id: &id.0.to_string(),
            title: &title,
            artist: &artist,
            album: &album,
            url,
            art_url: Some(art),
            mime,
            duration_ms: Some(duration_ms)
                .filter(|d| *d > 0)
                .or(item.and_then(|i| i.duration_ms)),
            size: std::fs::metadata(path).ok().map(|m| m.len()),
        })
    }

    /// Hand the renderer the track after this one, if it takes one and the
    /// next track is on disk. Asked after every command, so the queue
    /// changing under a playing track changes what follows it.
    pub(super) fn queue_next_on_renderer(&mut self) {
        let Some(play) = self.passthrough().filter(|p| p.loaded) else {
            return;
        };
        let current = play.current.id;
        let handed = play.next.as_ref().map(|s| s.id);
        let (refused, retry) = (play.next_refused, play.next_retry);
        if !self
            .renderer
            .as_ref()
            .is_some_and(|l| l.session.renderer().gapless)
        {
            return;
        }
        let next = self
            .shared_state
            .lookahead_after(current)
            .and_then(|step| step.chosen);
        if next.as_ref().map(|(id, _)| *id) == handed {
            return;
        }
        // What follows has changed. A renderer cannot be told to forget the
        // track it was handed, so that track's URL stops working, whatever
        // is or is not handed over in its place: one that moves on to it
        // anyway fails to fetch it and stops, which reads as the end of this
        // track.
        if let Some((link, session)) = self.on_renderer()
            && let Output::Passthrough(play) = &mut session.output
            && play.next.take().is_some()
        {
            link.session.retain(&play.tokens());
        }
        if next.is_some() && next.as_ref().map(|(id, _)| *id) == refused {
            return;
        }
        if retry.is_some_and(|at| Instant::now() < at) {
            return;
        }
        let Some((next_id, path)) = next else {
            return;
        };
        let extension = container_of(&path);
        let link = self.renderer.as_ref().expect("checked above");
        let Some(mime) = link.session.mime_for(&extension) else {
            // Left for the end of this track, where it is skipped with a reason.
            return;
        };
        let duration = buffer::probe_file(&path)
            .map(|i| i.duration_ms)
            .unwrap_or(0);
        let (token, url, art) = link.session.serve(&path, mime, &extension);
        let metadata = self.didl_for(next_id, &path, &url, &art, mime, duration);
        let Some((link, session)) = self.on_renderer() else {
            return;
        };
        let Output::Passthrough(play) = &mut session.output else {
            return;
        };
        match link.session.set_next(&url, &metadata) {
            Ok(()) => {
                log::info!(
                    "upnp: next on {} is {:?}",
                    link.session.renderer().name,
                    next_id
                );
                play.next = Some(Slot {
                    id: next_id,
                    token,
                    path,
                });
            }
            Err(_) => {
                if play.started.elapsed() < START_GRACE {
                    // Refused while it opens the current file, as Kodi does:
                    // offered again once that is done.
                    play.next_retry = Some(play.started + START_GRACE);
                    play.look_at.push(play.started + START_GRACE);
                } else {
                    play.next_refused = Some(next_id);
                }
            }
        }
        link.session.retain(&play.tokens());
    }

    /// The end of a renderer session: stop the renderer, if it still holds
    /// the track, and stop serving it. `stop_engine` for this output.
    pub(super) fn halt_renderer(&mut self, play: Passthrough) {
        if let Some(link) = self.renderer.as_ref() {
            if play.loaded {
                let _ = link.session.stop();
            }
            link.session.retain(&[]);
        }
        self.shared_state.set_renderer_clock(None);
    }

    /// Pause on the renderer, after the session's run has been set to paused.
    pub(super) fn pause_renderer(&mut self) {
        let at = self.shared_state.position_ms();
        let Some((link, session)) = self.on_renderer() else {
            return;
        };
        let Output::Passthrough(play) = &mut session.output else {
            return;
        };
        if !play.loaded {
            return;
        }
        play.asked_now();
        if let Err(e) = link.session.pause() {
            log::error!("upnp: pause failed: {e}");
        }
        link.session.look();
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: at,
            running: None,
        }));
    }

    /// Play on the renderer. With the track no longer held there, it is
    /// loaded again where the playhead stands.
    pub(super) fn resume_renderer(&mut self) {
        let at = self.shared_state.position_ms();
        let Some((link, session)) = self.on_renderer() else {
            return;
        };
        let Output::Passthrough(play) = &mut session.output else {
            return;
        };
        session.run = Run::Playing;
        if !play.loaded {
            if let Err(e) = self.restart_current(at) {
                log::error!("upnp: resume failed: {e}");
            }
            return;
        }
        if let Some(seek) = play.pending_seek
            && play.seek_tries == 0
        {
            let _ = link.session.seek(seek);
            play.seek_tries = 1;
        }
        play.asked_now();
        if let Err(e) = link.session.play() {
            log::error!("upnp: resume failed: {e}");
        }
        play.played = true;
        play.started = Instant::now();
        play.settle();
        let at = play.pending_seek.unwrap_or(at);
        link.session.look();
        // Held until it says it is playing, as when a track opens.
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: at,
            running: None,
        }));
        self.wake_analyzer();
        self.report(PlaybackReportState::Playing);
    }

    /// Seek the track the renderer holds, in place. `false` when no renderer
    /// holds one, and the caller restarts the session instead.
    pub(super) fn seek_on_renderer(&mut self, position_ms: u64) -> bool {
        let Some((link, session)) = self.on_renderer() else {
            return false;
        };
        let playing = session.run == Run::Playing;
        let id = session.track.id;
        let Output::Passthrough(play) = &mut session.output else {
            return false;
        };
        if !play.loaded {
            return false;
        }
        play.pending_seek = Some(position_ms);
        if playing {
            match link.session.seek(position_ms) {
                // Sent to a playing renderer, which takes it: only a check
                // once it has settled, not the instant retry a seek before
                // `Play` gets.
                Ok(()) => play.seek_tries = 2,
                // Refused, as renderers do while they open a file: kept as
                // the target and sent again once it says it is playing.
                Err(e) => {
                    log::info!("upnp: seek refused for now: {e}");
                    play.seek_tries = 1;
                }
            }
            play.settle();
            link.session.look();
        } else {
            play.seek_tries = 0;
        }
        self.bank_listening();
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms,
            running: playing.then(Instant::now),
        }));
        self.shared_state.set_position_ms(position_ms);
        self.on_track_changed(id, position_ms);
        true
    }

    pub(super) fn set_renderer_volume(&mut self, volume: u8) {
        let Some(link) = self.renderer.as_ref() else {
            return;
        };
        match link.session.set_volume(volume) {
            Ok(()) => self
                .shared_state
                .update_renderer(|o| o.volume = Some(volume.min(100))),
            Err(e) => log::warn!("upnp: volume refused: {e}"),
        }
    }

    /// What the renderer was heard to do, during the session numbered
    /// `session`. Volume and the renderer going are about the renderer; the
    /// rest is about a session, and is dropped once that is over.
    pub(super) fn on_renderer_event(&mut self, session: u64, event: session::Event) {
        match event {
            // Read from the renderer now playing rather than taken from the
            // event, which may come from a connection already left behind.
            session::Event::Volume(_) => {
                if let Some(volume) = self.renderer.as_ref().map(|l| l.session.volume()) {
                    self.shared_state.update_renderer(|o| o.volume = volume);
                }
            }
            // From any connection, this one or one already left behind: only
            // the renderer now playing, if it is the one lost, is left.
            session::Event::Gone => {
                if self.renderer.as_ref().is_some_and(|l| l.session.is_lost()) {
                    self.renderer_gone();
                }
            }
            session::Event::Snapshot(snapshot) if session == self.session => {
                self.on_renderer_snapshot(snapshot)
            }
            session::Event::Snapshot(_) => {}
        }
    }

    /// The renderer stopped answering or left the network. Leave it, paused
    /// where it was, so that nothing more waits on it; play carries on here.
    fn renderer_gone(&mut self) {
        let Some(link) = self.renderer.as_ref() else {
            return;
        };
        log::info!(
            "upnp: leaving {}, which is gone",
            link.session.renderer().name
        );
        if let Some((_, session)) = self.on_renderer() {
            session.run = Run::Paused;
            // Nothing is sent to it on the way out: it would only time out.
            if let Output::Passthrough(play) = &mut session.output {
                play.loaded = false;
            }
            self.report(PlaybackReportState::Paused);
        }
        self.use_renderer(None);
    }

    /// The renderer is no longer playing what koan gave it: stopped from its
    /// own controls, or taken over by another control point. Keep the place,
    /// paused; play loads the track again there.
    fn renderer_released(&mut self) {
        let at = self.shared_state.position_ms();
        self.bank_listening();
        let Some((link, session)) = self.on_renderer() else {
            return;
        };
        let was_playing = session.run == Run::Playing;
        session.run = Run::Paused;
        let Output::Passthrough(play) = &mut session.output else {
            return;
        };
        play.loaded = false;
        play.next = None;
        play.pending_seek = None;
        link.session.retain(&[]);
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: at,
            running: None,
        }));
        if was_playing {
            self.report(PlaybackReportState::Paused);
        }
    }

    /// The renderer answered where it is. Follow it.
    fn on_renderer_snapshot(&mut self, snap: session::Snapshot) {
        let Some((link, session)) = self.on_renderer() else {
            return;
        };
        let Output::Passthrough(play) = &session.output else {
            return;
        };
        if snap.epoch != link.session.epoch() || !play.loaded {
            return;
        }
        let token = link.session.token_of(&snap.track_uri).map(str::to_string);
        let is_current = token.as_deref() == Some(play.current.token.as_str());
        let is_next = token.is_some() && play.next.as_ref().map(|n| &n.token) == token.as_ref();
        log::info!(
            "upnp: heard {:?} at {}ms on {}",
            snap.transport,
            snap.position_ms.unwrap_or(0),
            if is_current {
                "the current track"
            } else if is_next {
                "the next track"
            } else if token.is_some() {
                "an old track of ours"
            } else if snap.track_uri.is_empty() {
                "nothing"
            } else {
                "something else"
            }
        );

        // Moved on to the track it was given next: gapless, from here.
        if is_next {
            self.renderer_moved_on();
        } else if play.started.elapsed() >= START_GRACE
            && !snap.track_uri.is_empty()
            && !is_current
            && matches!(
                snap.transport,
                session::Transport::Playing | session::Transport::Paused
            )
        {
            log::info!(
                "upnp: renderer is playing something else: {}",
                snap.track_uri
            );
            self.renderer_released();
            return;
        }

        let Some((link, session)) = self.on_renderer() else {
            return;
        };
        let run = session.run;
        let Output::Passthrough(play) = &mut session.output else {
            return;
        };
        let on_current = token.as_deref() == Some(play.current.token.as_str());
        match snap.transport {
            session::Transport::Playing
                if run == Run::Playing && play.pending_seek.is_some() && on_current =>
            {
                let target = play.pending_seek.expect("checked above");
                let at = snap.position_ms.unwrap_or(0);
                if at.abs_diff(target) <= ON_TARGET_MS {
                    // There, near enough: from here its readings are the
                    // playhead like any other.
                    play.pending_seek = None;
                    self.follow_renderer_clock(&snap, true);
                } else {
                    // The first answer that it is playing somewhere else is
                    // acted on at once: the seek sent before `Play` was
                    // ignored, and every moment spent waiting is heard. One
                    // more is tried once that has settled, then koan follows
                    // the renderer wherever it is.
                    let settled = Instant::now() >= play.settle_until;
                    if play.seek_tries < 2 || settled && play.seek_tries < 3 {
                        log::info!("upnp: renderer at {at}ms, seeking to {target}ms");
                        if let Err(e) = link.session.seek(target) {
                            log::warn!("upnp: seek refused: {e}");
                        }
                        play.seek_tries += 1;
                        play.settle();
                        link.session.look();
                        self.shared_state.set_renderer_clock(Some(RendererClock {
                            position_ms: target,
                            running: Some(Instant::now()),
                        }));
                    } else if settled {
                        log::info!("upnp: renderer will not seek; following it from {at}ms");
                        play.pending_seek = None;
                        self.shared_state.set_renderer_clock(Some(RendererClock {
                            position_ms: at,
                            running: Some(snap.at),
                        }));
                    }
                }
            }
            session::Transport::Playing => {
                if play.insist(link, true, run == Run::Playing) {
                    return;
                }
                if run != Run::Playing {
                    // Played from the renderer's own controls.
                    session.run = Run::Playing;
                    self.report(PlaybackReportState::Playing);
                }
                self.follow_renderer_clock(&snap, true);
            }
            session::Transport::Paused => {
                if play.insist(link, false, run == Run::Playing) {
                    return;
                }
                if run == Run::Playing {
                    session.run = Run::Paused;
                    self.report(PlaybackReportState::Paused);
                }
                self.follow_renderer_clock(&snap, false);
            }
            session::Transport::Stopped | session::Transport::NoMedia => {
                if run == Run::Paused {
                    // Loaded paused and never played: stopped is where a
                    // renderer is meant to be. No media means it has let go
                    // of the track, played or not.
                    if !play.played && snap.transport == session::Transport::Stopped {
                        return;
                    }
                    // Stopped at the renderer while paused here. Some forget
                    // the track when they stop, so play must load it again.
                    self.renderer_released();
                    return;
                }
                if play.started.elapsed() < START_GRACE {
                    return;
                }
                let at = self.shared_state.position_ms();
                let duration = self.shared_state.duration_ms();
                if duration > 0 && at + END_TOLERANCE_MS >= duration {
                    log::info!("upnp: track finished on the renderer");
                    // It holds nothing to stop; the next session tells it what
                    // to play.
                    if let Some((_, session)) = self.on_renderer()
                        && let Output::Passthrough(play) = &mut session.output
                    {
                        play.loaded = false;
                    }
                    let next = self.shared_state.advance_cursor_loadable();
                    self.carry_on(next, self.intent());
                } else {
                    log::info!("upnp: renderer stopped at {at}ms");
                    self.renderer_released();
                }
            }
            session::Transport::Transitioning => {}
        }
    }

    /// Take the renderer's reading as the playhead where the clock has
    /// drifted from it.
    ///
    /// A reading of whole seconds is an interval: 12 s means somewhere in
    /// 12.000–12.999. The clock stands while it is inside that interval and
    /// is corrected to its middle when it is not, so rounding never moves the
    /// bar and a real drift is caught within a second. Readings taken while
    /// the renderer settles after a command, or before it has reached a seek
    /// still outstanding, are set aside.
    fn follow_renderer_clock(&mut self, snap: &session::Snapshot, running: bool) {
        let Some(play) = self.passthrough() else {
            return;
        };
        let clock = self.shared_state.renderer_clock();
        let now = self.shared_state.position_ms();
        let moving = clock.is_some_and(|c| c.running.is_some());
        // Settling after a command, or not yet where koan sent it: either way
        // the reading says where it was, not where the music is meant to be.
        let settling = Instant::now() < play.settle_until || play.pending_seek.is_some();
        let correction = snap.position_ms.filter(|_| !settling).and_then(|at| {
            let width = if at % 1000 == 0 { 999 } else { 0 };
            // The clock as it stood when the renderer answered.
            let then = if moving {
                now.saturating_sub(snap.at.elapsed().as_millis() as u64)
            } else {
                now
            };
            let inside = then + READING_SLACK_MS >= at && then <= at + width + READING_SLACK_MS;
            (!inside).then_some(at + width / 2)
        });
        match correction {
            Some(at) => self.shared_state.set_renderer_clock(Some(RendererClock {
                position_ms: at,
                running: running.then_some(snap.at),
            })),
            // Started or stopped on its own: keep the place, change the motion.
            None if moving != running => {
                self.shared_state.set_renderer_clock(Some(RendererClock {
                    position_ms: now,
                    running: running.then(Instant::now),
                }))
            }
            None => {}
        }
    }

    /// The renderer started the track it had been given next.
    fn renderer_moved_on(&mut self) {
        self.bank_listening();
        let Some(next) = self.passthrough().and_then(|p| p.next.as_ref()) else {
            return;
        };
        let (id, path) = (next.id, next.path.clone());
        if self.shared_state.get_item(id).is_none() {
            // Handed over, then taken out of the queue before it played.
            log::info!("upnp: renderer moved on to {id:?}, which is no longer queued");
            self.renderer_released();
            return;
        }
        let info = buffer::probe_file(&path).ok();
        let Some((link, session)) = self.on_renderer() else {
            return;
        };
        let Output::Passthrough(play) = &mut session.output else {
            return;
        };
        let next = play.next.take().expect("checked above");
        play.current = next;
        // A new load, as far as the renderer is concerned: it is opening
        // this file now, which is when it refuses a next track and goes on
        // naming the last one. What was sought, refused or deferred belonged
        // to the track that just ended.
        play.pending_seek = None;
        play.seek_tries = 0;
        play.started = Instant::now();
        play.played = true;
        play.next_refused = None;
        play.next_retry = None;
        // A hand-over starts the track at its top. The reading that showed it
        // may not: Kodi names the new track with the old one's position.
        play.settle();
        link.session.retain(&play.tokens());
        if let Some(info) = info {
            session.track = TrackInfo {
                id,
                path,
                codec: info.codec,
                sample_rate: info.sample_rate,
                bit_depth: info.bit_depth,
                bitrate_kbps: info.bitrate_kbps,
                channels: info.channels,
                duration_ms: info.duration_ms,
            };
        }
        log::info!("upnp: renderer moved on to {id:?}");
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: 0,
            running: Some(Instant::now()),
        }));
        self.shared_state.set_cursor(Some(id));
        self.on_track_changed(id, 0);
        self.queue_next_on_renderer();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::player::commands::PlayerCommand;
    use crate::player::state::{PlaybackState, PlaylistItem};
    use crate::upnp::fake::FakeRenderer;

    const WAV: &str = "http-get:*:audio/wav:*,http-get:*:audio/x-wav:*";

    fn item(path: PathBuf, title: &str) -> PlaylistItem {
        // A library id of its own, which a download is keyed by.
        static NEXT_TRACK: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
        PlaylistItem {
            playlist_entry_id: None,
            id: QueueItemId::new(),
            db_id: Some(NEXT_TRACK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
            path,
            title: title.to_string(),
            artist: "Artist".into(),
            album_artist: String::new(),
            album: "Album".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            state: ItemState::Ready,
        }
    }

    struct Rig {
        player: Player,
        fake: Arc<FakeRenderer>,
        ids: Vec<QueueItemId>,
        _dir: tempfile::TempDir,
    }

    /// A player playing to a fake renderer, with `names` queued as ten-second
    /// WAVs (or MP3s, by extension).
    fn rig(sink: &'static str, gapless: bool, names: &[&str]) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeRenderer::start(sink, gapless, true);
        let mut player = Player::new();
        player.backend = Box::new(crate::player::tests::StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let items: Vec<PlaylistItem> = names
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                if name.ends_with(".mp3") {
                    crate::test_utils::generate_mp3_with_both_tags(&path, name, name);
                } else {
                    crate::test_utils::generate_wav(&path, 8_000, 1, 30.0, 16);
                }
                item(path, name)
            })
            .collect();
        let ids = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));

        let mut rig = Rig {
            player,
            fake,
            ids,
            _dir: dir,
        };
        rig.connect();
        rig
    }

    impl Rig {
        fn connect(&mut self) {
            let connection =
                upnp::open(self.fake.renderer(), &self.player.command_sender()).unwrap();
            self.player
                .process_command(PlayerCommand::UseRenderer(Some(Box::new(connection))));
        }

        /// One pass of the player's loop: a command if one arrives within
        /// `wait`, then what the loop does on every wake. `true` if a command
        /// came.
        fn step(&mut self, wait: Duration) -> bool {
            let came = match self.player.commands.rx.recv_timeout(wait) {
                Ok(cmd) => {
                    self.player.process_command(cmd);
                    true
                }
                Err(_) => false,
            };
            self.player.update_playback_state();
            came
        }

        /// Run the player's loop until `done` holds.
        fn pump_until(&mut self, done: impl Fn(&Player) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !done(&self.player) {
                if Instant::now() >= deadline {
                    panic!(
                        "timed out; renderer saw {:?}; player {:?} at {}ms, cursor {:?}",
                        self.fake.actions(),
                        self.player.shared_state.playback_state(),
                        self.player.shared_state.position_ms(),
                        self.player.shared_state.cursor(),
                    );
                }
                self.step(Duration::from_millis(50));
            }
        }

        fn play_mut(&mut self) -> &mut Passthrough {
            self.player.passthrough_mut().expect("a renderer session")
        }

        fn link(&self) -> &RendererLink {
            self.player.renderer.as_ref().expect("a renderer")
        }

        /// Hear the renderer at `ms` into the track, past the start grace.
        fn at(&mut self, ms: u64) {
            self.settle();
            self.fake.set_position(ms);
            let play = self.play_mut();
            play.started = Instant::now() - START_GRACE;
            play.settle_until = Instant::now();
            self.link().session.look();
            // RelTime is whole seconds.
            let floor = ms / 1000 * 1000;
            self.pump_until(|p| p.shared_state.position_ms() >= floor);
        }

        /// Take whatever answers the load left in flight, inside the grace
        /// window, before the test moves time on past it.
        fn settle(&mut self) {
            while self.step(Duration::from_millis(100)) {}
        }

        /// As if the grace window after the last load had run out.
        fn past_grace(&mut self) {
            self.settle();
            let play = self.play_mut();
            play.started = Instant::now() - START_GRACE;
            play.settle_until = Instant::now();
        }

        /// Run the player's loop until the renderer has been sent `action`
        /// `n` times.
        fn await_count(&mut self, action: &str, n: usize) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.count(action) < n {
                if Instant::now() >= deadline {
                    panic!("never sent {action} ×{n}: {:?}", self.fake.actions());
                }
                self.step(Duration::from_millis(50));
            }
        }

        /// The playback state, as `publish` would put it after a command.
        fn state(&self) -> PlaybackState {
            self.player.publish();
            self.player.shared_state.playback_state()
        }

        fn count(&self, action: &str) -> usize {
            self.fake.actions().iter().filter(|a| *a == action).count()
        }

        /// What koan told the renderer to do, without the questions the
        /// watcher asks it.
        fn commands(&self) -> Vec<String> {
            self.fake
                .actions()
                .into_iter()
                .filter(|a| !a.starts_with("Get"))
                .collect()
        }

        /// The command sent just before the last `action`.
        fn command_before(&self, action: &str) -> (String, Vec<(String, String)>) {
            let commands: Vec<_> = self
                .fake
                .state
                .lock()
                .actions
                .iter()
                .filter(|(a, _)| !a.starts_with("Get"))
                .cloned()
                .collect();
            let at = commands.iter().rposition(|(a, _)| a == action).unwrap();
            commands[at - 1].clone()
        }

        fn last_command(&self) -> (String, Vec<(String, String)>) {
            self.fake
                .state
                .lock()
                .actions
                .iter()
                .rev()
                .find(|(a, _)| !a.starts_with("Get"))
                .cloned()
                .unwrap()
        }
    }

    /// A profile for the rig's local device, halving the level. Handed to
    /// the player directly: changing the process's config would race the rest
    /// of the suite.
    fn with_profile(r: &mut Rig) {
        r.player.dsp_override = Some(Arc::new(
            crate::audio::dsp::Setup::new(vec![], vec![]).with_preamp(-6.0),
        ));
    }

    /// The renderer fetches the original file, so a profile for the local
    /// device has done nothing to what it plays, and the badge must not say
    /// it has.
    #[test]
    fn a_renderer_session_publishes_no_dsp_status() {
        let mut r = rig(WAV, true, &["a.wav"]);
        with_profile(&mut r);

        r.player.process_command(PlayerCommand::UseRenderer(None));
        r.player.play(r.ids[0]);
        r.state();
        let here = r.player.shared_state.dsp().map(|d| d.profile);
        assert_eq!(
            here.as_deref(),
            Some("Half"),
            "played here, it is processed"
        );

        r.connect();
        r.player.play(r.ids[0]);
        r.state();
        assert_eq!(r.player.shared_state.dsp(), None);
    }

    /// A profile change cannot reach what a renderer plays, so it must not
    /// stop and reload the renderer to apply it.
    #[test]
    fn reloading_dsp_leaves_a_renderer_playing() {
        let mut r = rig(WAV, true, &["a.wav"]);
        with_profile(&mut r);
        r.player.play(r.ids[0]);
        r.settle();
        let starts = r.player.playback_starts;
        let before = r.commands();

        r.player.process_command(PlayerCommand::ReloadDsp);
        r.settle();
        assert_eq!(
            r.player.playback_starts, starts,
            "the session was not reopened"
        );
        assert_eq!(r.commands(), before, "the renderer was told nothing");
        assert_eq!(r.state(), PlaybackState::Playing);
    }

    #[test]
    fn a_track_plays_on_the_renderer_from_a_file_it_fetches() {
        let mut r = rig(WAV, true, &["a.wav", "b.wav"]);
        assert_eq!(r.player.shared_state.renderer().unwrap().name, "Fake Amp");
        r.pump_until(|p| p.shared_state.renderer().unwrap().volume == Some(20));

        r.player.play(r.ids[0]);
        assert_eq!(r.state(), PlaybackState::Playing);
        let commands = r.commands();
        assert_eq!(commands[..2], ["SetAVTransportURI", "Play"], "{commands:?}");

        let fetched = r.fake.state.lock().fetched.clone();
        let size = std::fs::metadata(r.player.shared_state.get_item(r.ids[0]).unwrap().path)
            .unwrap()
            .len() as usize;
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].1, size);
        assert!(fetched[0].0.ends_with(".wav"));

        let didl = r
            .fake
            .state
            .lock()
            .actions
            .iter()
            .find(|(a, _)| a == "SetAVTransportURI")
            .and_then(|(_, args)| {
                args.iter()
                    .find(|(k, _)| k == "CurrentURIMetaData")
                    .cloned()
            })
            .unwrap()
            .1;
        assert!(didl.contains("<dc:title>a.wav</dc:title>"));
        assert!(didl.contains("http-get:*:audio/wav:DLNA.ORG_OP=01"));

        // Gapless: the next track was handed over too.
        assert_eq!(r.count("SetNextAVTransportURI"), 1);
    }

    #[test]
    fn the_renderer_moving_on_to_the_next_track_moves_the_cursor() {
        let mut r = rig(WAV, true, &["a.wav", "b.wav"]);
        r.player.play(r.ids[0]);
        r.fake.finish_track();
        let second = r.ids[1];
        r.pump_until(|p| p.shared_state.cursor() == Some(second));
        assert_eq!(r.player.shared_state.track_info().unwrap().id, second);
        assert_eq!(r.state(), PlaybackState::Playing);
        // Moved on by itself: nothing was loaded again.
        assert_eq!(r.count("SetAVTransportURI"), 1);
    }

    #[test]
    fn without_a_next_track_the_end_of_one_starts_the_next() {
        let mut r = rig(WAV, false, &["a.wav", "b.wav"]);
        r.player.play(r.ids[0]);
        assert_eq!(r.count("SetNextAVTransportURI"), 0);
        r.at(29_500);
        r.fake.finish_track();
        let second = r.ids[1];
        r.pump_until(|p| p.shared_state.cursor() == Some(second));
        assert_eq!(r.count("SetAVTransportURI"), 2);
        assert_eq!(r.state(), PlaybackState::Playing);
    }

    #[test]
    fn a_stop_at_the_renderer_mid_track_keeps_the_place() {
        let mut r = rig(WAV, false, &["a.wav", "b.wav"]);
        r.player.play(r.ids[0]);
        r.at(2_000);
        r.fake.press_stop(2_000);
        r.pump_until(|p| p.shared_state.playback_state() == PlaybackState::Paused);
        assert_eq!(r.player.shared_state.cursor(), Some(r.ids[0]));
        assert_eq!(r.count("SetAVTransportURI"), 1);
        assert!((2_000..3_000).contains(&r.player.shared_state.position_ms()));

        // Play loads it again, where it was.
        r.player.resume();
        assert_eq!(r.count("SetAVTransportURI"), 2);
        let seek = r.command_before("Play");
        assert_eq!(seek.0, "Seek");
        assert!(
            seek.1.contains(&("Target".into(), "0:00:02".into())),
            "{seek:?}"
        );
    }

    #[test]
    fn a_track_the_renderer_cannot_play_is_skipped_with_a_reason() {
        let mut r = rig(WAV, false, &["a.mp3", "b.wav"]);
        r.player.play(r.ids[0]);
        assert_eq!(r.player.shared_state.cursor(), Some(r.ids[1]));
        assert_eq!(r.count("SetAVTransportURI"), 1);
        assert!(matches!(
            r.player.shared_state.get_item(r.ids[0]).unwrap().state,
            ItemState::Failed(ref why) if why == "Fake Amp cannot play MP3"
        ));
        assert_eq!(
            r.player.shared_state.renderer().unwrap().problem,
            None,
            "the track that did play clears it"
        );

        // Back on this device, it plays again.
        r.player.pause();
        r.player.use_renderer(None);
        assert_eq!(
            r.player.shared_state.get_item(r.ids[0]).unwrap().state,
            ItemState::Ready
        );
    }

    #[test]
    fn pause_seek_resume_and_volume_reach_the_renderer() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.player.play(r.ids[0]);

        r.player.pause();
        assert_eq!(r.state(), PlaybackState::Paused);
        assert_eq!(r.last_command().0, "Pause");

        // Paused, a seek waits for play: not every renderer seeks a paused
        // transport, and every one seeks a playing one.
        r.player.seek(4_000);
        assert_eq!(r.count("Seek"), 0);
        assert_eq!(r.player.shared_state.position_ms(), 4_000);

        r.player.resume();
        let commands = r.commands();
        assert_eq!(&commands[commands.len() - 2..], ["Seek", "Play"]);
        assert_eq!(r.state(), PlaybackState::Playing);

        r.player.seek(6_000);
        assert_eq!(r.count("Seek"), 2);

        r.player.set_renderer_volume(55);
        assert_eq!(r.fake.state.lock().volume, 55);
        assert_eq!(r.player.shared_state.renderer().unwrap().volume, Some(55));
    }

    #[test]
    fn an_answer_from_before_the_last_command_is_ignored() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.player.play(r.ids[0]);
        r.play_mut().started = Instant::now() - START_GRACE;
        let session = r.player.session;
        r.player.on_renderer_event(
            session,
            session::Event::Snapshot(session::Snapshot {
                epoch: 0,
                transport: session::Transport::Stopped,
                track_uri: String::new(),
                position_ms: Some(0),
                duration_ms: None,
                at: Instant::now(),
            }),
        );
        assert_eq!(r.state(), PlaybackState::Playing);
    }

    #[test]
    fn leaving_the_renderer_stops_it_and_comes_home_paused() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.player.play(r.ids[0]);
        r.player.pause();
        r.player.use_renderer(None);
        assert_eq!(r.last_command().0, "Stop");
        assert!(r.player.shared_state.renderer().is_none());
        assert_eq!(r.state(), PlaybackState::Paused);
        assert!(
            r.player
                .session()
                .is_some_and(|s| matches!(s.output, Output::Local(_))),
            "loaded on the local output"
        );
    }

    /// Start a transfer for queue entry `id` in `state`'s download store,
    /// with enough of it on disk to stream.
    fn start_download(state: &crate::player::state::SharedPlayerState, id: QueueItemId) {
        let item = state.get_item(id).unwrap();
        let track = item.db_id.unwrap();
        state.update_item_state(id, ItemState::Pending);
        let downloads = state.downloads();
        downloads.claim(track, Some(id));
        let feed = downloads.announce(
            track,
            item.title.clone(),
            String::new(),
            item.path.clone(),
            item.path,
        );
        feed.set(crate::player::state::STREAM_THRESHOLD * 2);
        downloads.started(track, 0);
    }

    fn downloading(r: &Rig, id: QueueItemId) {
        start_download(&r.player.shared_state, id);
    }

    /// The transfer for `id` lands, as the download queue would settle it.
    fn landed(r: &mut Rig, id: QueueItemId) {
        let item = r.player.shared_state.get_item(id).unwrap();
        crate::remote::downloads::settle(
            &r.player.shared_state,
            item.db_id.unwrap(),
            &Ok(item.path),
        )
        .announce(&r.player.command_sender());
        while let Ok(cmd) = r.player.commands.rx.try_recv() {
            r.player.process_command(cmd);
        }
    }

    #[test]
    fn a_track_still_downloading_waits_for_the_whole_file_on_a_renderer() {
        let mut r = rig(WAV, false, &["a.wav"]);
        let id = r.ids[0];
        downloading(&r, id);

        r.player.process_command(PlayerCommand::Play(id));
        assert!(r.player.waiting().is_some(), "parked, not streamed");
        // Enough has arrived to stream; a renderer is handed whole files.
        r.player
            .process_command(PlayerCommand::TrackStreamReady(id));
        assert_eq!(
            r.count("SetAVTransportURI"),
            0,
            "nothing sent before it lands"
        );

        landed(&mut r, id);
        assert_eq!(r.count("SetAVTransportURI"), 1);
        assert_eq!(r.state(), PlaybackState::Playing);
    }

    #[test]
    fn a_paused_track_waiting_for_its_download_opens_on_the_renderer_where_it_was() {
        let mut r = rig(WAV, false, &["a.wav"]);
        let id = r.ids[0];
        r.player.process_command(PlayerCommand::UseRenderer(None));
        downloading(&r, id);
        r.player.process_command(PlayerCommand::Cue {
            id,
            position_ms: 3_000,
            play: false,
        });
        assert!(r.player.waiting().is_some());

        r.connect();
        assert_eq!(
            r.count("SetAVTransportURI"),
            0,
            "nothing sent before it lands"
        );
        assert_eq!(
            r.state(),
            PlaybackState::Paused,
            "a paused wait reads as paused"
        );

        landed(&mut r, id);
        assert_eq!(r.count("SetAVTransportURI"), 1);
        assert_eq!(r.state(), PlaybackState::Paused, "still paused");
        assert_eq!(r.player.shared_state.position_ms(), 3_000);
        r.player.process_command(PlayerCommand::Resume);
        let commands = r.commands();
        assert_eq!(&commands[commands.len() - 2..], ["Seek", "Play"]);
    }

    #[test]
    fn a_next_track_removed_from_the_queue_is_not_followed() {
        let mut r = rig(WAV, true, &["a.wav", "b.wav"]);
        r.player.play(r.ids[0]);
        assert_eq!(r.count("SetNextAVTransportURI"), 1);
        let next_uri = r.fake.state.lock().next_uri.clone();

        r.player
            .process_command(PlayerCommand::RemoveFromPlaylist(r.ids[1]));
        r.player.queue_next_on_renderer();
        let token = r
            .player
            .renderer
            .as_ref()
            .unwrap()
            .session
            .token_of(&next_uri)
            .unwrap()
            .to_string();
        assert!(
            !r.player
                .passthrough()
                .unwrap()
                .tokens()
                .contains(&token.as_str()),
            "its URL no longer serves"
        );

        // The renderer moves on to it anyway.
        r.past_grace();
        r.fake.finish_track();
        r.pump_until(|p| p.shared_state.playback_state() == PlaybackState::Paused);
        assert_eq!(r.player.shared_state.cursor(), Some(r.ids[0]));
        assert!(!r.player.renderer_loaded());
    }

    #[test]
    fn another_control_point_taking_the_renderer_over_releases_it() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.player.play(r.ids[0]);
        r.at(2_000);
        r.past_grace();
        r.fake.play_foreign("http://10.0.0.9/song.mp3");
        r.pump_until(|p| p.shared_state.playback_state() == PlaybackState::Paused);
        assert!(!r.player.renderer_loaded());
        assert!((2_000..3_000).contains(&r.player.shared_state.position_ms()));
    }

    #[test]
    fn a_stop_at_the_renderer_while_paused_reloads_on_play() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.player.play(r.ids[0]);
        r.at(4_000);
        r.player.pause();
        r.fake.press_stop(0);
        r.pump_until(|p| !p.renderer_loaded());
        assert_eq!(r.state(), PlaybackState::Paused);

        r.player.resume();
        assert_eq!(r.count("SetAVTransportURI"), 2, "loaded again");
        let seek = r.command_before("Play");
        assert_eq!(seek.0, "Seek");
        assert!(
            seek.1.contains(&("Target".into(), "0:00:04".into())),
            "{seek:?}"
        );
    }

    #[test]
    fn a_renderer_saying_goodbye_is_left_paused() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.player.play(r.ids[0]);
        r.at(2_000);
        let udn = r.fake.renderer().udn;
        crate::upnp::discovery::forget(&udn);
        r.pump_until(|p| p.renderer.is_none());
        assert!(r.player.shared_state.renderer().is_none());
        assert_eq!(r.state(), PlaybackState::Paused);
        assert!(
            r.player
                .session()
                .is_some_and(|s| matches!(s.output, Output::Local(_))),
            "loaded here, paused"
        );
    }

    /// Two tones played to a real renderer, the second handed over as the
    /// next track: a rising pitch with no gap is gapless working.
    ///
    /// `KOAN_UPNP_LOCATION=http://host:port/description.xml cargo test -p
    /// koan-core --lib real_renderer -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a renderer on the network"]
    fn plays_to_a_real_renderer() {
        let Ok(location) = std::env::var("KOAN_UPNP_LOCATION") else {
            eprintln!("KOAN_UPNP_LOCATION is not set");
            return;
        };
        let renderer = upnp::discovery::fetch(&url::Url::parse(&location).unwrap())
            .unwrap()
            .expect("a renderer at that address");
        eprintln!(
            "renderer: {} (gapless: {})",
            renderer.name, renderer.gapless
        );

        let dir = tempfile::tempdir().unwrap();
        let mut player = Player::new();
        let items: Vec<PlaylistItem> = [("low.wav", 440.0), ("high.wav", 660.0)]
            .iter()
            .map(|(name, hz)| {
                let path = dir.path().join(name);
                crate::test_utils::generate_wav_tone(&path, 44_100, *hz, 6.0);
                item(path, name)
            })
            .collect();
        let ids: Vec<QueueItemId> = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));

        let connection = upnp::open(renderer, &player.command_sender()).unwrap();
        player.use_renderer(Some(Box::new(connection)));
        player.play(ids[0]);

        let started = Instant::now();
        let mut moved_on = None;
        while started.elapsed() < Duration::from_secs(25) {
            if let Ok(cmd) = player.commands.rx.recv_timeout(Duration::from_millis(250)) {
                eprintln!("{:>6}ms {cmd:?}", started.elapsed().as_millis());
                player.process_command(cmd);
            }
            player.update_playback_state();
            if moved_on.is_none() && player.shared_state.cursor() == Some(ids[1]) {
                moved_on = Some(started.elapsed());
                eprintln!("moved on to the second tone at {:?}", started.elapsed());
            }
            if moved_on.is_some() && player.shared_state.playback_state() == PlaybackState::Stopped
            {
                break;
            }
        }
        let at = moved_on.expect("the renderer reached the second tone");
        assert!(at > Duration::from_secs(5), "moved on too early: {at:?}");
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Stopped);
    }

    #[test]
    fn a_renderer_still_naming_the_last_track_after_a_load_is_not_a_takeover() {
        let mut r = rig(WAV, false, &["a.wav", "b.wav"]);
        r.fake.lag.store(true, std::sync::atomic::Ordering::Relaxed);
        r.player.play(r.ids[0]);
        r.player.play(r.ids[1]);
        // Its answers name the first track's URL for now.
        r.link().session.look();
        r.pump_until(|p| p.renderer.as_ref().is_some_and(|o| o.session.epoch() > 0));
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            r.step(Duration::from_millis(50));
        }
        assert!(r.player.renderer_loaded(), "still koan's");
        assert_eq!(r.state(), PlaybackState::Playing);
    }

    #[test]
    fn a_renderer_that_ignores_a_seek_before_playing_is_seeked_once_it_plays() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.fake
            .seeks_only_playing
            .store(true, std::sync::atomic::Ordering::Relaxed);
        r.player.cue(r.ids[0], 4_000, Run::Playing);
        assert_eq!(r.count("Seek"), 1, "tried before playing");
        r.play_mut().settle_until = Instant::now();
        r.await_count("Seek", 2);
        r.pump_until(|p| p.passthrough().is_some_and(|o| o.pending_seek.is_none()));
        assert!((4_000..5_000).contains(&r.player.shared_state.position_ms()));
    }

    #[test]
    fn small_differences_from_the_renderer_do_not_move_the_playhead() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.player.play(r.ids[0]);
        r.at(5_000);
        let before = r.player.shared_state.position_ms();
        // The same moment as a renderer reports it: whole seconds.
        r.fake.set_position(before / 1000 * 1000);
        r.link().session.look();
        r.settle();
        assert!(
            r.player.shared_state.position_ms() >= before,
            "did not jump back"
        );
    }

    /// Drives a real renderer through what a listener does, comparing koan's
    /// playhead with the renderer's own answer at each step. Prints a line
    /// per check, and fails where the two disagree by more than they should.
    ///
    /// `KOAN_UPNP_LOCATION=http://host:port/ cargo test -p koan-core --lib
    /// drives_a_real_renderer -- --ignored --nocapture --test-threads=1`
    #[test]
    #[ignore = "needs a renderer on the network"]
    fn drives_a_real_renderer() {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Info)
            .filter_module("koan_core::upnp::discovery", log::LevelFilter::Warn)
            .try_init();
        let Ok(location) = std::env::var("KOAN_UPNP_LOCATION") else {
            eprintln!("KOAN_UPNP_LOCATION is not set");
            return;
        };
        let renderer = upnp::discovery::fetch(&url::Url::parse(&location).unwrap())
            .unwrap()
            .expect("a renderer at that address");
        let avt = renderer.av_transport.clone();
        let http = upnp::soap::client();
        let truth = || {
            let id = [("InstanceID", "0")];
            let info = upnp::soap::call(&http, &avt, "GetTransportInfo", &id).unwrap();
            let pos = upnp::soap::call(&http, &avt, "GetPositionInfo", &id).unwrap();
            (
                upnp::soap::arg(&info, "CurrentTransportState")
                    .unwrap_or_default()
                    .to_string(),
                upnp::soap::arg(&pos, "RelTime")
                    .and_then(upnp::soap::parse_time)
                    .unwrap_or(0),
                upnp::soap::arg(&pos, "TrackURI")
                    .unwrap_or_default()
                    .to_string(),
            )
        };

        let dir = tempfile::tempdir().unwrap();
        let mut player = Player::new();
        player.backend = Box::new(crate::player::tests::StuckBackend {
            rate: 44_100.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let items: Vec<PlaylistItem> = [("a.wav", 330.0), ("b.wav", 440.0), ("c.wav", 550.0)]
            .iter()
            .map(|(name, hz)| {
                let path = dir.path().join(name);
                crate::test_utils::generate_wav_tone(&path, 44_100, *hz, 30.0);
                item(path, name)
            })
            .collect();
        let ids: Vec<QueueItemId> = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));

        let connect = |player: &mut Player| {
            let connection = upnp::open(renderer.clone(), &player.command_sender()).unwrap();
            player.use_renderer(Some(Box::new(connection)));
        };
        // Run the player's renderer loop for `ms`.
        let run = |player: &mut Player, ms: u64| {
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                if let Ok(cmd) = player.commands.rx.recv_timeout(Duration::from_millis(50)) {
                    player.process_command(cmd);
                }
                player.update_playback_state();
            }
        };
        let failures = std::cell::RefCell::new(Vec::new());
        let check = |player: &Player, step: &str, tolerance_ms: u64| {
            let ours = player.shared_state.position_ms();
            let (state, theirs, uri) = truth();
            let cursor = player.shared_state.cursor();
            let which = ids.iter().position(|id| Some(*id) == cursor);
            let diff = ours.abs_diff(theirs);
            let ok = diff <= tolerance_ms;
            eprintln!(
                "{} {step:<40} koan {:>6}ms {:?}/track {:?} | renderer {:>6}ms {state} {}",
                if ok { "ok  " } else { "FAIL" },
                ours,
                player.shared_state.playback_state(),
                which,
                theirs,
                uri.rsplit('/').next().unwrap_or_default(),
            );
            if !ok {
                failures
                    .borrow_mut()
                    .push(format!("{step}: koan {ours}ms, renderer {theirs}ms"));
            }
        };

        connect(&mut player);
        player.cue(ids[0], 10_000, Run::Playing);
        run(&mut player, 4_000);
        check(&player, "opened at 10s", 1_500);
        run(&mut player, 3_000);
        check(&player, "three seconds on", 1_500);

        player.seek(20_000);
        run(&mut player, 3_000);
        check(&player, "seeked to 20s", 1_500);

        player.pause();
        run(&mut player, 2_000);
        check(&player, "paused", 1_200);
        player.resume();
        run(&mut player, 3_000);
        check(&player, "resumed", 1_500);

        player.next_track();
        run(&mut player, 3_000);
        check(&player, "next track", 1_500);
        player.prev_track();
        run(&mut player, 3_000);
        check(&player, "previous track", 1_500);

        player.seek(26_000);
        run(&mut player, 9_000);
        check(&player, "played into the next track", 2_000);
        let moved = player.shared_state.cursor() == Some(ids[1]);
        eprintln!(
            "{} gapless hand-over to the second track",
            if moved { "ok  " } else { "FAIL" }
        );
        if !moved {
            failures
                .borrow_mut()
                .push("did not move on to the second track".into());
        }

        // Out to this device and back, mid-track.
        player.pause();
        let at = player.shared_state.position_ms();
        player.use_renderer(None);
        connect(&mut player);
        player.resume();
        run(&mut player, 4_000);
        let ours = player.shared_state.position_ms();
        eprintln!("     came back at {ours}ms, left at {at}ms");
        check(&player, "back on the renderer", 1_500);
        if ours + 500 < at {
            failures
                .borrow_mut()
                .push(format!("came back at {ours}ms after leaving at {at}ms"));
        }

        player.stop();
        let failures = failures.into_inner();
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// Hammers a real renderer — skips, seeks, pauses and output switches in
    /// quick succession — then checks that koan and the renderer agree on
    /// the track, whether it is playing, and where.
    ///
    /// `KOAN_UPNP_LOCATION=http://host:port/ cargo test -p koan-core --lib
    /// stresses_a_real_renderer -- --ignored --nocapture --test-threads=1`
    #[test]
    #[ignore = "needs a renderer on the network"]
    fn stresses_a_real_renderer() {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Warn)
            .filter_module("koan_core::player::renderer", log::LevelFilter::Info)
            .try_init();
        let Ok(location) = std::env::var("KOAN_UPNP_LOCATION") else {
            eprintln!("KOAN_UPNP_LOCATION is not set");
            return;
        };
        let renderer = upnp::discovery::fetch(&url::Url::parse(&location).unwrap())
            .unwrap()
            .expect("a renderer at that address");
        let avt = renderer.av_transport.clone();
        let http = upnp::soap::client();
        let truth = || {
            let id = [("InstanceID", "0")];
            let info = upnp::soap::call(&http, &avt, "GetTransportInfo", &id).unwrap();
            let pos = upnp::soap::call(&http, &avt, "GetPositionInfo", &id).unwrap();
            (
                upnp::soap::arg(&info, "CurrentTransportState")
                    .unwrap_or_default()
                    .to_string(),
                upnp::soap::arg(&pos, "RelTime")
                    .and_then(upnp::soap::parse_time)
                    .unwrap_or(0),
                upnp::soap::arg(&pos, "TrackURI")
                    .unwrap_or_default()
                    .to_string(),
            )
        };

        let dir = tempfile::tempdir().unwrap();
        let mut player = Player::new();
        player.backend = Box::new(crate::player::tests::StuckBackend {
            rate: 44_100.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let items: Vec<PlaylistItem> = (0..6)
            .map(|i| {
                let path = dir.path().join(format!("t{i}.wav"));
                crate::test_utils::generate_wav_tone(&path, 44_100, 220.0 + 110.0 * i as f32, 40.0);
                item(path, &format!("t{i}"))
            })
            .collect();
        let ids: Vec<QueueItemId> = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));

        let connect = |player: &mut Player| {
            let connection = upnp::open(renderer.clone(), &player.command_sender()).unwrap();
            player.use_renderer(Some(Box::new(connection)));
        };
        let run = |player: &mut Player, ms: u64| {
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                if let Ok(cmd) = player.commands.rx.recv_timeout(Duration::from_millis(20)) {
                    player.process_command(cmd);
                }
                player.update_playback_state();
            }
        };
        let failures = std::cell::RefCell::new(Vec::new());
        let agree = |player: &Player, step: &str| {
            let (state, theirs, uri) = truth();
            let ours = player.shared_state.position_ms();
            let playing = player.shared_state.playback_state();
            let token = player
                .passthrough()
                .filter(|p| p.loaded)
                .map(|p| p.current.token.clone());
            let same_track = token.as_ref().is_some_and(|t| uri.contains(t.as_str()));
            let same_state = match playing {
                PlaybackState::Playing => state == "PLAYING",
                PlaybackState::Paused => state != "PLAYING",
                PlaybackState::Stopped => state != "PLAYING",
            };
            let close = ours.abs_diff(theirs) <= 1_500;
            let ok = same_track && same_state && close;
            let which = ids
                .iter()
                .position(|id| Some(*id) == player.shared_state.cursor());
            eprintln!(
                "{} {step:<34} koan {ours:>6}ms {playing:?} track {which:?} | renderer {theirs:>6}ms {state}{}",
                if ok { "ok  " } else { "FAIL" },
                if same_track { "" } else { " (other track)" }
            );
            if !ok {
                failures.borrow_mut().push(step.to_string());
            }
        };

        connect(&mut player);
        player.play(ids[0]);
        run(&mut player, 3_000);
        agree(&player, "started");

        // Steady playback: the playhead the bar is drawn from only moves on.
        let mut last = player.shared_state.position_ms();
        let mut back = 0u64;
        let mut ahead = 0u64;
        let begun = Instant::now();
        while begun.elapsed() < Duration::from_secs(12) {
            run(&mut player, 50);
            let now = player.shared_state.position_ms();
            back = back.max(last.saturating_sub(now));
            ahead = ahead.max(now.saturating_sub(last).saturating_sub(250));
            last = now;
        }
        eprintln!("     steady play: largest step back {back}ms, largest jump ahead {ahead}ms");
        if back > 0 || ahead > 500 {
            failures.borrow_mut().push(format!(
                "playhead moved back {back}ms / jumped {ahead}ms in steady play"
            ));
        }
        agree(&player, "after twelve seconds");

        for _ in 0..4 {
            player.next_track();
            run(&mut player, 150);
        }
        run(&mut player, 7_000);
        agree(&player, "four quick skips");

        for target in [5_000, 25_000, 12_000, 30_000, 8_000, 18_000] {
            player.seek(target);
            run(&mut player, 120);
        }
        run(&mut player, 7_000);
        agree(&player, "six quick seeks");

        for _ in 0..5 {
            player.pause();
            run(&mut player, 100);
            player.resume();
            run(&mut player, 100);
        }
        run(&mut player, 7_000);
        agree(&player, "pause and resume five times");

        player.seek(10_000);
        run(&mut player, 3_000);
        player.pause();
        run(&mut player, 2_000);
        agree(&player, "paused");
        let left = player.shared_state.position_ms();
        let track = player.shared_state.cursor();

        for _ in 0..3 {
            player.use_renderer(None);
            run(&mut player, 300);
            connect(&mut player);
            run(&mut player, 300);
        }
        player.resume();
        run(&mut player, 4_000);
        agree(&player, "out and back three times");
        let back = player.shared_state.position_ms();
        eprintln!("     left at {left}ms, {back}ms four seconds after coming back");
        if player.shared_state.cursor() != track || back < left || back > left + 5_000 {
            failures
                .borrow_mut()
                .push(format!("came back at {back}ms having left at {left}ms"));
        }

        player.prev_track();
        player.seek(15_000);
        player.pause();
        player.resume();
        run(&mut player, 7_000);
        agree(&player, "back, seek, pause, play at once");

        player.stop();
        let failures = failures.into_inner();
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// The same, through the player's own command loop on its own thread, as
    /// an app drives it: commands in, shared state out.
    ///
    /// `KOAN_UPNP_LOCATION=http://host:port/ cargo test -p koan-core --lib
    /// loops_a_real_renderer -- --ignored --nocapture --test-threads=1`
    #[test]
    #[ignore = "needs a renderer on the network"]
    fn loops_a_real_renderer() {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::Warn)
            .filter_module("koan_core::player::renderer", log::LevelFilter::Info)
            .try_init();
        let Ok(location) = std::env::var("KOAN_UPNP_LOCATION") else {
            eprintln!("KOAN_UPNP_LOCATION is not set");
            return;
        };
        let renderer = upnp::discovery::fetch(&url::Url::parse(&location).unwrap())
            .unwrap()
            .expect("a renderer at that address");
        let udn = renderer.udn.clone();
        let avt = renderer.av_transport.clone();
        upnp::discovery::remember(renderer, Duration::from_secs(600));
        let http = upnp::soap::client();
        let truth = || {
            let id = [("InstanceID", "0")];
            let info = upnp::soap::call(&http, &avt, "GetTransportInfo", &id).unwrap();
            let pos = upnp::soap::call(&http, &avt, "GetPositionInfo", &id).unwrap();
            (
                upnp::soap::arg(&info, "CurrentTransportState")
                    .unwrap_or_default()
                    .to_string(),
                upnp::soap::arg(&pos, "RelTime")
                    .and_then(upnp::soap::parse_time)
                    .unwrap_or(0),
            )
        };

        let dir = tempfile::tempdir().unwrap();
        let mut player = Player::new();
        player.backend = Box::new(crate::player::tests::StuckBackend {
            rate: 44_100.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let items: Vec<PlaylistItem> = (0..4)
            .map(|i| {
                let path = dir.path().join(format!("l{i}.wav"));
                crate::test_utils::generate_wav_tone(&path, 44_100, 260.0 + 90.0 * i as f32, 30.0);
                item(path, &format!("l{i}"))
            })
            .collect();
        let ids: Vec<QueueItemId> = items.iter().map(|i| i.id).collect();
        let state = player.shared_state();
        let tx = player.command_sender();
        let looping = std::thread::spawn(move || player.run());
        let send = |cmd: PlayerCommand| tx.send(cmd).unwrap();
        let wait = |ms: u64| std::thread::sleep(Duration::from_millis(ms));

        let mut failures = Vec::new();
        let mut agree = |step: &str| {
            let (renderer_state, theirs) = truth();
            let ours = state.position_ms();
            let playing = state.playback_state();
            let moving = state.playhead_moving();
            let same_state = (playing == PlaybackState::Playing) == (renderer_state == "PLAYING");
            let ok = same_state && ours.abs_diff(theirs) <= 1_500;
            eprintln!(
                "{} {step:<30} koan {ours:>6}ms {playing:?}{} | renderer {theirs:>6}ms {renderer_state}",
                if ok { "ok  " } else { "FAIL" },
                if moving { "" } else { " (held)" }
            );
            if !ok {
                failures.push(step.to_string());
            }
        };

        send(PlayerCommand::AddToPlaylist(items));
        upnp::connect(&udn, upnp::choose(), &tx).unwrap();
        send(PlayerCommand::Play(ids[0]));
        wait(4_000);
        agree("played");
        send(PlayerCommand::Seek(15_000));
        wait(4_000);
        agree("seeked to 15s");
        send(PlayerCommand::Pause);
        wait(2_000);
        agree("paused");
        send(PlayerCommand::Resume);
        wait(4_000);
        agree("resumed");
        send(PlayerCommand::NextTrack);
        wait(4_000);
        agree("next");
        send(PlayerCommand::Seek(24_000));
        wait(10_000);
        agree("played into the next track");
        upnp::disconnect(&tx);
        wait(1_000);
        let left = state.position_ms();
        upnp::connect(&udn, upnp::choose(), &tx).unwrap();
        wait(5_000);
        let back = state.position_ms();
        agree("out and back while playing");
        eprintln!("     left at {left}ms, {back}ms five seconds after coming back");
        if back + 1_000 < left {
            failures.push(format!("came back at {back}ms having left at {left}ms"));
        }

        // The loop is left running: the player holds a sender of its own
        // (the timeline's), so it never sees the channel close.
        send(PlayerCommand::Stop);
        wait(500);
        drop(looping);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn a_long_run_of_unplayable_tracks_is_walked_without_growing_the_stack() {
        // On a thread with a small stack: a recursion per skipped track
        // overflows it long before the run ends.
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(|| {
                let mut r = rig(WAV, false, &["a.wav"]);
                let dir = tempfile::tempdir().unwrap();
                let mut items: Vec<PlaylistItem> = (0..3_000)
                    .map(|i| item(dir.path().join(format!("{i}.mp3")), "x"))
                    .collect();
                let last = dir.path().join("last.wav");
                crate::test_utils::generate_wav(&last, 8_000, 1, 2.0, 16);
                items.push(item(last, "last"));
                let first = items[0].id;
                let last_id = items.last().unwrap().id;
                r.player.process_command(PlayerCommand::ReplacePlaylist {
                    items: Vec::new(),
                    start: 0,
                    position_ms: 0,
                    play: false,
                });
                r.player
                    .process_command(PlayerCommand::AddToPlaylist(items));
                r.player.play(first);
                assert_eq!(r.player.shared_state.cursor(), Some(last_id));
                assert_eq!(r.count("SetAVTransportURI"), 1);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn a_next_track_replaced_by_one_the_renderer_cannot_play_is_revoked() {
        let mut r = rig(WAV, true, &["a.wav", "b.wav", "c.mp3"]);
        r.player.play(r.ids[0]);
        let handed = r
            .player
            .passthrough()
            .unwrap()
            .next
            .as_ref()
            .unwrap()
            .token
            .clone();
        r.player
            .process_command(PlayerCommand::RemoveFromPlaylist(r.ids[1]));
        r.player.queue_next_on_renderer();
        let output = r.player.passthrough().unwrap();
        assert!(
            output.next.is_none(),
            "nothing the renderer can play follows"
        );
        assert!(
            !output.tokens().contains(&handed.as_str()),
            "the old next no longer serves"
        );
    }

    #[test]
    fn a_track_loaded_paused_is_not_released_by_its_renderer_saying_stopped() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.player.cue(r.ids[0], 3_000, Run::Paused);
        r.play_mut().started = Instant::now() - START_GRACE;
        r.link().session.look();
        r.settle();
        assert!(r.player.renderer_loaded(), "still loaded, waiting for play");
        assert_eq!(r.state(), PlaybackState::Paused);
        r.player.resume();
        assert_eq!(
            r.count("SetAVTransportURI"),
            1,
            "played as loaded, not loaded again"
        );
    }

    #[test]
    fn a_renderer_that_opens_after_a_later_choice_is_not_used() {
        let fake = FakeRenderer::start(WAV, false, true);
        let renderer = fake.renderer();
        let udn = renderer.udn.clone();
        upnp::discovery::remember(renderer, Duration::from_secs(60));
        let (tx, rx) = crossbeam_channel::unbounded();
        let first = upnp::choose();
        let _later = upnp::choose();
        // Its events go to the same channel; only the switch matters here.
        let switched = |rx: &crossbeam_channel::Receiver<PlayerCommand>| {
            rx.try_iter()
                .any(|cmd| matches!(cmd, PlayerCommand::UseRenderer(Some(_))))
        };
        upnp::connect(&udn, first, &tx).unwrap();
        assert!(!switched(&rx), "the earlier pick is dropped");
        let last = upnp::choose();
        upnp::connect(&udn, last, &tx).unwrap();
        assert!(switched(&rx));
    }

    /// `random_use_keeps_the_player_honest` with a renderer as the output:
    /// the same invariants after every step, through switches of output,
    /// the renderer finishing tracks and being stopped at its own controls,
    /// events from sessions already over, and tracks still downloading,
    /// which a renderer must wait for.
    #[test]
    fn random_use_with_a_renderer_keeps_the_player_honest() {
        use crate::player::tests::{Rng, asks_to_play, check_invariants, playlist_ids};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 30.0, 16);

        for seed in 1..=8u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let fake = FakeRenderer::start(WAV, rng.coin(), true);
            let mut player = Player::new();
            player.backend = Box::new(crate::player::tests::StuckBackend {
                rate: 8_000.0,
                asked: Default::default(),
                starts: Default::default(),
            });
            // Items still arriving, with enough on disk to stream: started
            // once they are in the playlist, where the store looks them up.
            let streamed = std::cell::RefCell::new(Vec::new());
            let fresh = |rng: &mut Rng| {
                let mut it = item(path.clone(), "t");
                match rng.below(4) {
                    0 => it.state = ItemState::Pending,
                    1 => {
                        it.state = ItemState::Pending;
                        streamed.borrow_mut().push(it.id);
                    }
                    _ => {}
                }
                it
            };
            let start: Vec<_> = (0..5).map(|_| fresh(&mut rng)).collect();
            player.process_command(PlayerCommand::AddToPlaylist(start));
            let state = player.shared_state.clone();
            let mut started = 0;
            let mut start_streams = || {
                let streamed = streamed.borrow();
                for &id in &streamed[started..] {
                    if state.get_item(id).is_some() {
                        start_download(&state, id);
                    }
                }
                started = streamed.len();
            };
            start_streams();
            let connect = |player: &Player| {
                let connection = upnp::open(fake.renderer(), &player.command_sender()).unwrap();
                PlayerCommand::UseRenderer(Some(Box::new(connection)))
            };
            let cmd = connect(&player);
            player.process_command(cmd);

            let mut history = Vec::new();
            for step in 0..100 {
                let ids = playlist_ids(&player);
                let pick = |rng: &mut Rng| ids.get(rng.below(ids.len())).copied();
                let stale = |player: &Player, session: u64| PlayerCommand::Renderer {
                    session,
                    event: session::Event::Snapshot(session::Snapshot {
                        epoch: player.renderer.as_ref().map_or(0, |l| l.session.epoch()),
                        transport: session::Transport::Stopped,
                        track_uri: String::new(),
                        position_ms: Some(0),
                        duration_ms: None,
                        at: Instant::now(),
                    }),
                };
                let cmd = match rng.below(22) {
                    0 | 1 => pick(&mut rng).map(PlayerCommand::Play),
                    2 => pick(&mut rng).map(|id| PlayerCommand::Cue {
                        id,
                        position_ms: if rng.coin() { 0 } else { 2_000 },
                        play: rng.coin(),
                    }),
                    3 => Some(PlayerCommand::Pause),
                    4 => Some(PlayerCommand::Resume),
                    5 => Some(PlayerCommand::NextTrack),
                    6 => Some(PlayerCommand::PrevTrack),
                    7 => Some(PlayerCommand::Seek(5_000)),
                    8 => pick(&mut rng).map(PlayerCommand::RemoveFromPlaylist),
                    9 => pick(&mut rng).zip(pick(&mut rng)).map(|(id, target)| {
                        PlayerCommand::MoveInPlaylist {
                            id,
                            target,
                            after: rng.coin(),
                        }
                    }),
                    10 => Some(PlayerCommand::AddToPlaylist(vec![fresh(&mut rng)])),
                    11 => Some(PlayerCommand::Undo),
                    12 => {
                        let (items, _) = player.shared_state.snapshot_playlist();
                        let pending: Vec<_> = items
                            .iter()
                            .filter(|i| matches!(i.state, ItemState::Pending))
                            .map(|i| i.id)
                            .collect();
                        pending.get(rng.below(pending.len())).map(|&id| {
                            let item = player.shared_state.get_item(id).unwrap();
                            crate::remote::downloads::settle(
                                &player.shared_state,
                                item.db_id.unwrap(),
                                &Ok(item.path),
                            )
                            .announce(&player.command_sender());
                            player.shared_state.update_item_state(id, ItemState::Ready);
                            PlayerCommand::TrackReady(id)
                        })
                    }
                    13 => pick(&mut rng).map(PlayerCommand::TrackStreamReady),
                    // Switching output, either way.
                    14 => Some(if player.renderer.is_some() && rng.coin() {
                        PlayerCommand::UseRenderer(None)
                    } else {
                        connect(&player)
                    }),
                    // The renderer plays the track out, or someone stops it.
                    15 => {
                        if rng.coin() {
                            fake.finish_track();
                        } else {
                            fake.press_stop(1_000);
                        }
                        if let Some(play) = player.passthrough_mut() {
                            play.started = Instant::now() - START_GRACE;
                            play.settle_until = Instant::now();
                        }
                        None
                    }
                    // What a session already over heard, and an answer from
                    // before the last command of this one.
                    16 => Some(stale(&player, player.session.wrapping_sub(1))),
                    17 => {
                        let mut cmd = stale(&player, player.session);
                        if let PlayerCommand::Renderer {
                            event: session::Event::Snapshot(snap),
                            ..
                        } = &mut cmd
                        {
                            snap.epoch = snap.epoch.wrapping_sub(1);
                        }
                        Some(cmd)
                    }
                    18 => Some(PlayerCommand::DecodeFinished(player.session)),
                    19 => Some(PlayerCommand::ClearPlaylist),
                    _ => Some(PlayerCommand::ReplacePlaylist {
                        items: (0..3).map(|_| fresh(&mut rng)).collect(),
                        start: rng.below(4),
                        position_ms: if rng.coin() { 0 } else { 2_000 },
                        play: rng.coin(),
                    }),
                };
                let label = cmd
                    .as_ref()
                    .map_or("renderer changed".into(), |c| format!("{c:?}"));
                let asked = cmd.as_ref().is_some_and(asks_to_play);
                let wanted_before = player.shared_state.wants_to_play();
                if let Some(cmd) = cmd {
                    player.process_command(cmd);
                }
                start_streams();
                // What arrived meanwhile — the renderer's answers, the decode
                // threads' ends — as the loop would see it.
                let settle = Instant::now() + Duration::from_millis(15);
                while let Ok(sent) = player.commands.rx.recv_deadline(settle) {
                    player.process_command(sent);
                }
                player.update_playback_state();
                history.push(label);
                if let Err(broken) = check_invariants(&player, wanted_before, asked) {
                    let tail = history[history.len().saturating_sub(8)..].join("\n  ");
                    panic!("seed {seed}, step {step}: {broken}\nlast commands:\n  {tail}");
                }
                // What the renderer holds is what the session says is playing.
                if let (Some(play), Some(session)) =
                    (player.passthrough().filter(|p| p.loaded), player.session())
                    && play.current.id != session.track.id
                {
                    panic!("seed {seed}, step {step}: the renderer holds another track");
                }
            }
            player.process_command(PlayerCommand::Stop);
        }
    }

    #[test]
    fn a_next_track_refused_after_a_gapless_move_is_offered_again() {
        use std::sync::atomic::Ordering::Relaxed;
        let mut r = rig(WAV, true, &["a.wav", "b.wav", "c.wav"]);
        r.player.process_command(PlayerCommand::Play(r.ids[0]));
        assert_eq!(r.count("SetNextAVTransportURI"), 1);

        // It refuses the next track while it opens the one it moved on to.
        r.fake.refuses_next.store(true, Relaxed);
        r.fake.finish_track();
        let second = r.ids[1];
        r.pump_until(|p| p.shared_state.cursor() == Some(second));
        assert_eq!(r.count("SetNextAVTransportURI"), 2);
        assert!(
            r.player.passthrough().unwrap().next_refused.is_none(),
            "deferred, not given up on"
        );

        r.fake.refuses_next.store(false, Relaxed);
        r.play_mut().next_retry = Some(Instant::now());
        r.player.update_playback_state();
        assert_eq!(r.count("SetNextAVTransportURI"), 3);
        assert_eq!(
            r.player.passthrough().unwrap().next.as_ref().map(|n| n.id),
            Some(r.ids[2])
        );
    }

    #[test]
    fn a_goodbye_from_a_renderer_left_behind_does_not_drop_its_successor() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.player.process_command(PlayerCommand::Play(r.ids[0]));
        let first_session = r.player.session;
        let other = FakeRenderer::start(WAV, false, true);
        let connection = upnp::open(other.renderer(), &r.player.command_sender()).unwrap();
        r.player
            .process_command(PlayerCommand::UseRenderer(Some(Box::new(connection))));

        // The first renderer's goodbye, arriving after the switch.
        r.player.process_command(PlayerCommand::Renderer {
            session: first_session,
            event: session::Event::Gone,
        });
        assert_eq!(
            r.player.shared_state.renderer().map(|o| o.udn),
            Some(other.renderer().udn)
        );
        assert!(r.player.renderer_loaded());
    }

    #[test]
    fn a_renderer_gone_while_a_track_opens_leaves_it_here_paused() {
        let mut r = rig(WAV, false, &["a.wav", "b.wav"]);
        r.player.process_command(PlayerCommand::Play(r.ids[0]));
        r.settle();
        r.fake.power_off();

        r.player.process_command(PlayerCommand::NextTrack);
        assert!(r.player.renderer.is_none(), "the renderer is left");
        let session = r.player.session().expect("the track is open here");
        assert!(matches!(session.output, Output::Local(_)));
        assert_eq!(session.track.id, r.ids[1]);
        assert_eq!(r.state(), PlaybackState::Paused);
    }

    #[test]
    fn losing_a_renderer_with_the_player_channel_full_does_not_hang() {
        let (done, finished) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let mut r = rig(WAV, false, &["a.wav"]);
            r.player.process_command(PlayerCommand::Play(r.ids[0]));
            r.settle();
            r.fake.power_off();
            // Full, as when commands queue while a request to the renderer
            // waits out its timeout.
            let tx = r.player.command_sender();
            while tx.try_send(PlayerCommand::TrackQueued).is_ok() {}
            // Finds the renderer gone on the player's own thread.
            r.player.process_command(PlayerCommand::Pause);
            r.player.update_playback_state();
            assert!(r.player.renderer.is_none());
            done.send(()).unwrap();
        });
        assert!(
            finished.recv_timeout(Duration::from_secs(15)).is_ok(),
            "the player hung"
        );
    }

    #[test]
    fn picking_the_renderer_already_playing_does_nothing() {
        let mut r = rig(WAV, false, &["a.wav"]);
        r.player.process_command(PlayerCommand::Play(r.ids[0]));
        let loads = r.count("SetAVTransportURI");
        let again = upnp::open(r.fake.renderer(), &r.player.command_sender()).unwrap();
        r.player
            .process_command(PlayerCommand::UseRenderer(Some(Box::new(again))));
        assert_eq!(r.count("SetAVTransportURI"), loads, "not loaded again");
        assert_eq!(r.count("Stop"), 0);
    }
}
