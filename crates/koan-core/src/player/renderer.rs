//! Playing to a UPnP renderer: the player's second output.
//!
//! The queue, cursor, history and undo work exactly as they do with local
//! output; what differs is where each track goes and where the playhead is
//! read from. A track is handed over as a URL to the original file. The
//! playhead is the renderer's, extrapolated from where it was last heard to
//! be. A track ends when the renderer moves on to the next one it was given,
//! or stops at the end of this one.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::state::{ItemState, PlaybackState, QueueItemId, RendererClock, TrackInfo};
use super::{Player, PlayerError, Start, media_extension};
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

pub(super) struct RendererOutput {
    session: upnp::session::Session,
    events: crossbeam_channel::Receiver<session::Event>,
    /// What the renderer was told to play.
    current: Option<Slot>,
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
    /// What koan last asked of it, playing or paused, and when. A renderer
    /// still opening a file drops commands; one that contradicts a recent
    /// request is asked again, once, before koan takes its word for it.
    intent: Intent,
    /// Tracks this renderer cannot play, marked failed in the queue for as
    /// long as it is the output.
    refused: Vec<QueueItemId>,
}

struct Intent {
    playing: bool,
    at: Instant,
    /// When it was last asked again, and how many times.
    resent: Option<Instant>,
    resends: u8,
}

/// Between asking a renderer again: long enough for it to finish what it is
/// doing, which is usually opening a file.
const RESEND_GAP: Duration = Duration::from_millis(700);
const MAX_RESENDS: u8 = 4;

/// How long after asking a renderer to play or pause koan holds to what it
/// asked, against an answer that says otherwise.
const INTENT_HOLD: Duration = Duration::from_secs(4);

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

impl RendererOutput {
    fn intend(&mut self, playing: bool) {
        // Counted as just sent: an answer in the next moment predates the
        // renderer acting on it, and is no reason to ask again.
        let now = Instant::now();
        self.intent = Intent {
            playing,
            at: now,
            resent: Some(now),
            resends: 0,
        };
        self.look_at.push(now + RESEND_GAP);
    }

    /// The renderer says it is `playing` (or not) against what koan asked a
    /// moment ago. For `INTENT_HOLD` koan's request stands: the renderer is
    /// asked again every `RESEND_GAP`, and looked at again after each, while
    /// its contrary answers are set aside. `true` while that holds; after it,
    /// the renderer's word is taken, since someone may have used its own
    /// controls.
    fn insist(&mut self, playing: bool) -> bool {
        if self.intent.playing == playing || self.intent.at.elapsed() > INTENT_HOLD {
            return false;
        }
        let due = self
            .intent
            .resent
            .is_none_or(|at| at.elapsed() >= RESEND_GAP);
        if due && self.intent.resends < MAX_RESENDS {
            self.intent.resends += 1;
            self.intent.resent = Some(Instant::now());
            log::info!(
                "upnp: renderer is {} against what was asked; asking again",
                if playing { "playing" } else { "not playing" }
            );
            let _ = if self.intent.playing {
                self.session.play()
            } else {
                self.session.pause()
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
        self.current
            .iter()
            .chain(self.next.iter())
            .map(|s| s.token.as_str())
            .collect()
    }
}

impl Player {
    /// When the player loop should wake for the renderer: see `look_at`.
    pub(super) fn renderer_deadline(&self) -> Option<Instant> {
        self.renderer
            .as_ref()
            .and_then(|r| r.look_at.iter().min().copied())
    }

    /// What the player loop does for the renderer on every pass: keep the
    /// next track handed over, and ask where it is once a settle window ends.
    pub(super) fn renderer_tick(&mut self) {
        if let Some(output) = self.renderer.as_mut() {
            let now = Instant::now();
            let due = output.look_at.len();
            output.look_at.retain(|at| *at > now);
            if output.look_at.len() < due {
                output.session.look();
            }
        }
        self.queue_next_on_renderer();
    }

    pub(super) fn renderer_events(&self) -> Option<crossbeam_channel::Receiver<session::Event>> {
        self.renderer.as_ref().map(|r| r.events.clone())
    }

    /// Whether a track is loaded on the renderer, playing or paused.
    pub(super) fn renderer_loaded(&self) -> bool {
        self.renderer.as_ref().is_some_and(|r| r.current.is_some())
    }

    /// Switch output to `connection`'s renderer, or back to this device with
    /// `None`, and carry on from where the music is, paused if it was.
    pub(super) fn use_renderer(&mut self, connection: Option<Box<Connection>>) {
        let info = self.shared_state.track_info();
        let position_ms = self.shared_state.position_ms();
        let state = self.shared_state.playback_state();
        log::info!(
            "upnp: switching output at {position_ms}ms, {state:?}, to {}",
            connection
                .as_ref()
                .map_or("this device", |c| c.session.renderer().name.as_str())
        );

        self.stop_engine();
        if let Some(old) = self.renderer.take() {
            log::info!("upnp: leaving {}", old.session.renderer().name);
            for id in old.refused {
                self.shared_state.update_item_state(id, ItemState::Ready);
            }
        }
        self.shared_state.set_renderer_clock(None);

        match connection {
            Some(connection) => {
                let Connection { session, events } = *connection;
                log::info!("upnp: playing to {}", session.renderer().name);
                self.shared_state.set_renderer(Some(upnp::Output {
                    udn: session.renderer().udn.clone(),
                    name: session.renderer().name.clone(),
                    volume: None,
                    problem: None,
                }));
                self.renderer = Some(RendererOutput {
                    session,
                    events,
                    current: None,
                    next: None,
                    next_refused: None,
                    next_retry: None,
                    pending_seek: None,
                    seek_tries: 0,
                    settle_until: Instant::now(),
                    look_at: Vec::new(),
                    started: Instant::now(),
                    intent: Intent {
                        playing: false,
                        at: Instant::now(),
                        resent: None,
                        resends: 0,
                    },
                    refused: Vec::new(),
                });
            }
            None => self.shared_state.set_renderer(None),
        }

        // Stopped stays stopped; anything else picks up where it was.
        let (Some(info), PlaybackState::Playing | PlaybackState::Paused) = (info, state) else {
            return;
        };
        self.shared_state.set_position_ms(position_ms);
        if let Err(e) = self.restart_current(&info, position_ms) {
            log::error!("upnp: could not carry on after switching output: {e}");
        }
    }

    /// Open `path` on the renderer at `seek_ms`. The counterpart of
    /// `open_playback`.
    pub(super) fn open_on_renderer(
        &mut self,
        id: QueueItemId,
        path: &Path,
        seek_ms: u64,
        start: Start,
    ) -> Result<(), PlayerError> {
        self.stop_engine();
        let extension = container_of(path);
        let Some(output) = self.renderer.as_mut() else {
            return Err(PlayerError::Renderer("no renderer".into()));
        };
        let Some(mime) = output.session.mime_for(&extension) else {
            let reason = format!(
                "{} cannot play {}",
                output.session.renderer().name,
                extension.to_uppercase()
            );
            log::info!("upnp: skipping {id:?}: {reason}");
            output.refused.push(id);
            self.shared_state
                .update_item_state(id, ItemState::Failed(reason.clone()));
            self.shared_state
                .update_renderer(|o| o.problem = Some(reason.clone()));
            return Err(PlayerError::Unplayable(reason));
        };

        let info = buffer::probe_file(path)?;
        self.shared_state.set_track_info(Some(TrackInfo {
            id,
            path: path.to_path_buf(),
            codec: info.codec.clone(),
            sample_rate: info.sample_rate,
            bit_depth: info.bit_depth,
            bitrate_kbps: info.bitrate_kbps,
            channels: info.channels,
            duration_ms: info.duration_ms,
        }));
        // What the renderer does with the samples is its own business; there
        // is no device rate here to compare against.
        self.shared_state.clear_output_sample_rate();
        self.shared_state.set_position_ms(seek_ms);
        self.on_track_changed(id, seek_ms);

        let output = self.renderer.as_mut().expect("checked above");
        let (token, url, art) = output.session.serve(path, mime, &extension);
        let metadata = self.didl_for(id, path, &url, &art, mime, info.duration_ms);
        let output = self.renderer.as_mut().expect("checked above");
        output.next = None;
        output.pending_seek = None;
        output.current = Some(Slot {
            id,
            token,
            path: path.to_path_buf(),
        });
        let tokens: Vec<&str> = output.tokens();
        output.session.retain(&tokens);

        log::info!(
            "upnp: {} ← {} ({:?}) {}{}",
            output.session.renderer().name,
            path.display(),
            id,
            mime,
            if seek_ms > 0 {
                format!(" @{seek_ms}ms")
            } else {
                String::new()
            }
        );
        output
            .session
            .set_uri(&url, &metadata)
            .map_err(|e| PlayerError::Renderer(e.to_string()))?;
        output.started = Instant::now();
        output.pending_seek = (seek_ms > 0).then_some(seek_ms);
        output.seek_tries = 0;
        output.intend(start == Start::Playing);
        let state = match start {
            Start::Playing => {
                // Seeked before it plays, so a renderer that takes it starts
                // in the right place and none of the top is heard. One that
                // ignores it is seeked again once it plays.
                if seek_ms > 0 {
                    let _ = output.session.seek(seek_ms);
                    output.seek_tries = 1;
                }
                output
                    .session
                    .play()
                    .map_err(|e| PlayerError::Renderer(e.to_string()))?;
                output.started = Instant::now();
                output.settle();
                PlaybackState::Playing
            }
            Start::Paused => PlaybackState::Paused,
        };
        output.session.look();
        self.shared_state.update_renderer(|o| o.problem = None);
        // Held where it opens until the renderer says it is playing: several
        // take a second or more to start, and a bar that ran from the
        // command would have to be pulled back when the sound began.
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: seek_ms,
            running: None,
        }));
        self.shared_state.set_playback_state(state);
        self.queue_next_on_renderer();
        Ok(())
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
        let Some(output) = self.renderer.as_ref() else {
            return;
        };
        let Some(current) = output.current.as_ref() else {
            return;
        };
        if !output.session.renderer().gapless {
            return;
        }
        let next = self.shared_state.peek_next_ready_after(current.id);
        if next.as_ref().map(|(id, _)| *id) == output.next.as_ref().map(|s| s.id) {
            return;
        }
        if next.is_some() && next.as_ref().map(|(id, _)| *id) == output.next_refused {
            return;
        }
        if output.next_retry.is_some_and(|at| Instant::now() < at) {
            return;
        }
        let Some((next_id, path)) = next else {
            // The queue changed and nothing follows now. A renderer cannot be
            // told to forget its next track, so its URL stops working: a
            // renderer that moves on to it anyway fails to fetch it and stops,
            // which reads as the end of this track.
            if let Some(output) = self.renderer.as_mut() {
                output.next = None;
                let tokens: Vec<&str> = output.tokens();
                output.session.retain(&tokens);
            }
            return;
        };
        let extension = container_of(&path);
        let Some(mime) = output.session.mime_for(&extension) else {
            // Left for the end of this track, where it is skipped with a reason.
            return;
        };
        let duration = buffer::probe_file(&path)
            .map(|i| i.duration_ms)
            .unwrap_or(0);
        let output = self.renderer.as_ref().expect("checked above");
        let (token, url, art) = output.session.serve(&path, mime, &extension);
        let metadata = self.didl_for(next_id, &path, &url, &art, mime, duration);
        let output = self.renderer.as_mut().expect("checked above");
        match output.session.set_next(&url, &metadata) {
            Ok(()) => {
                log::info!(
                    "upnp: next on {} is {:?}",
                    output.session.renderer().name,
                    next_id
                );
                output.next = Some(Slot {
                    id: next_id,
                    token,
                    path,
                });
            }
            Err(_) => {
                output.next = None;
                if output.started.elapsed() < START_GRACE {
                    // Refused while it opens the current file, as Kodi does:
                    // offered again once that is done.
                    output.next_retry = Some(output.started + START_GRACE);
                    output.look_at.push(output.started + START_GRACE);
                } else {
                    output.next_refused = Some(next_id);
                }
            }
        }
        let tokens: Vec<&str> = output.tokens();
        output.session.retain(&tokens);
    }

    /// Stop the renderer and forget what it was playing. The counterpart of
    /// tearing down the local engine.
    pub(super) fn halt_renderer(&mut self) {
        if !self.renderer_loaded() {
            return;
        }
        self.bank_listening();
        let output = self.renderer.as_mut().expect("checked above");
        let _ = output.session.stop();
        output.current = None;
        output.next = None;
        output.pending_seek = None;
        output.session.retain(&[]);
        self.shared_state.set_renderer_clock(None);
    }

    pub(super) fn pause_renderer(&mut self) {
        let Some(output) = self.renderer.as_mut() else {
            return;
        };
        if output.current.is_none() {
            return;
        }
        output.intend(false);
        if let Err(e) = output.session.pause() {
            log::error!("upnp: pause failed: {e}");
            return;
        }
        output.session.look();
        let at = self.shared_state.position_ms();
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: at,
            running: None,
        }));
        self.shared_state.set_playback_state(PlaybackState::Paused);
    }

    /// Play what is loaded on the renderer. With nothing loaded, start the
    /// current track where the playhead stands.
    pub(super) fn resume_renderer(&mut self) {
        if !self.renderer_loaded() {
            match self.shared_state.track_info() {
                Some(info) => {
                    let at = self.shared_state.position_ms();
                    self.shared_state.set_playback_state(PlaybackState::Playing);
                    if let Err(e) = self.restart_current(&info, at) {
                        log::error!("upnp: resume failed: {e}");
                    }
                }
                None => {
                    if let Some(id) = self.shared_state.cursor() {
                        self.play(id);
                    }
                }
            }
            return;
        }
        let output = self.renderer.as_mut().expect("checked above");
        if let Some(seek) = output.pending_seek
            && output.seek_tries == 0
        {
            let _ = output.session.seek(seek);
            output.seek_tries = 1;
        }
        output.intend(true);
        if let Err(e) = output.session.play() {
            log::error!("upnp: resume failed: {e}");
            return;
        }
        output.started = Instant::now();
        output.settle();
        let at = output
            .pending_seek
            .unwrap_or_else(|| self.shared_state.position_ms());
        output.session.look();
        // Held until it says it is playing, as when a track opens.
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: at,
            running: None,
        }));
        self.shared_state.set_playback_state(PlaybackState::Playing);
        self.report(PlaybackReportState::Playing);
    }

    /// Seek the track loaded on the renderer. `false` when it is not the one
    /// asked about, and the caller loads it instead.
    pub(super) fn seek_renderer(&mut self, id: QueueItemId, position_ms: u64) -> bool {
        let Some(output) = self.renderer.as_mut() else {
            return false;
        };
        if output.current.as_ref().is_none_or(|c| c.id != id) {
            return false;
        }
        let playing = self.shared_state.playback_state() == PlaybackState::Playing;
        if playing {
            output.pending_seek = Some(position_ms);
            match output.session.seek(position_ms) {
                // Sent to a playing renderer, which takes it: only a check
                // once it has settled, not the instant retry a seek before
                // `Play` gets.
                Ok(()) => output.seek_tries = 2,
                // Refused, as renderers do while they open a file: kept as
                // the target and sent again once it says it is playing.
                Err(e) => {
                    log::info!("upnp: seek refused for now: {e}");
                    output.seek_tries = 1;
                }
            }
            output.settle();
            output.session.look();
        } else {
            output.pending_seek = Some(position_ms);
            output.seek_tries = 0;
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
        let Some(output) = self.renderer.as_ref() else {
            return;
        };
        match output.session.set_volume(volume) {
            Ok(()) => self
                .shared_state
                .update_renderer(|o| o.volume = Some(volume.min(100))),
            Err(e) => log::warn!("upnp: volume refused: {e}"),
        }
    }

    pub(super) fn on_renderer_event(&mut self, event: session::Event) {
        match event {
            session::Event::Volume(volume) => self
                .shared_state
                .update_renderer(|o| o.volume = Some(volume)),
            session::Event::Snapshot(snapshot) => self.on_renderer_snapshot(snapshot),
            session::Event::Gone => self.renderer_gone(),
        }
    }

    /// The renderer stopped answering or left the network. Leave it, paused
    /// where it was, so that nothing more waits on it; play carries on here.
    fn renderer_gone(&mut self) {
        let Some(output) = self.renderer.as_mut() else {
            return;
        };
        log::info!(
            "upnp: leaving {}, which is gone",
            output.session.renderer().name
        );
        // Nothing is sent to it on the way out: it would only time out.
        output.current = None;
        output.next = None;
        if self.shared_state.playback_state() == PlaybackState::Playing {
            self.shared_state.set_playback_state(PlaybackState::Paused);
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
        let Some(output) = self.renderer.as_mut() else {
            return;
        };
        output.current = None;
        output.next = None;
        output.session.retain(&[]);
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: at,
            running: None,
        }));
        if self.shared_state.playback_state() == PlaybackState::Playing {
            self.shared_state.set_playback_state(PlaybackState::Paused);
            self.report(PlaybackReportState::Paused);
        }
    }

    /// The renderer answered where it is. Follow it.
    fn on_renderer_snapshot(&mut self, snap: session::Snapshot) {
        let Some(output) = self.renderer.as_ref() else {
            return;
        };
        if snap.epoch != output.session.epoch() || output.current.is_none() {
            return;
        }
        let token = output.session.token_of(&snap.track_uri).map(str::to_string);
        log::info!(
            "upnp: heard {:?} at {}ms on {}",
            snap.transport,
            snap.position_ms.unwrap_or(0),
            match (&token, &output.current, &output.next) {
                (Some(t), Some(c), _) if *t == c.token => "the current track",
                (Some(t), _, Some(n)) if *t == n.token => "the next track",
                (Some(_), ..) => "an old track of ours",
                (None, ..) if snap.track_uri.is_empty() => "nothing",
                (None, ..) => "something else",
            }
        );

        // Moved on to the track it was given next: gapless, from here.
        if token.is_some() && output.next.as_ref().map(|n| &n.token) == token.as_ref() {
            self.renderer_moved_on();
        } else if output.started.elapsed() >= START_GRACE
            && !snap.track_uri.is_empty()
            && output.current.as_ref().map(|c| &c.token) != token.as_ref()
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

        let Some(output) = self.renderer.as_ref() else {
            return;
        };
        let state = self.shared_state.playback_state();
        match snap.transport {
            session::Transport::Playing
                if state == PlaybackState::Playing
                    && output.pending_seek.is_some()
                    && output.current.as_ref().map(|c| &c.token) == token.as_ref() =>
            {
                let output = self.renderer.as_mut().expect("checked above");
                let target = output.pending_seek.expect("checked above");
                let at = snap.position_ms.unwrap_or(0);
                if at.abs_diff(target) <= ON_TARGET_MS {
                    // There, near enough: from here its readings are the
                    // playhead like any other.
                    output.pending_seek = None;
                    self.follow_renderer_clock(&snap, true);
                } else {
                    // The first answer that it is playing somewhere else is
                    // acted on at once: the seek sent before `Play` was
                    // ignored, and every moment spent waiting is heard. One
                    // more is tried once that has settled, then koan follows
                    // the renderer wherever it is.
                    let settled = Instant::now() >= output.settle_until;
                    if output.seek_tries < 2 || settled && output.seek_tries < 3 {
                        log::info!("upnp: renderer at {at}ms, seeking to {target}ms");
                        if let Err(e) = output.session.seek(target) {
                            log::warn!("upnp: seek refused: {e}");
                        }
                        output.seek_tries += 1;
                        output.settle();
                        output.session.look();
                        self.shared_state.set_renderer_clock(Some(RendererClock {
                            position_ms: target,
                            running: Some(Instant::now()),
                        }));
                    } else if settled {
                        log::info!("upnp: renderer will not seek; following it from {at}ms");
                        output.pending_seek = None;
                        self.shared_state.set_renderer_clock(Some(RendererClock {
                            position_ms: at,
                            running: Some(snap.at),
                        }));
                    }
                }
            }
            session::Transport::Playing => {
                if self.renderer.as_mut().is_some_and(|o| o.insist(true)) {
                    return;
                }
                self.follow_renderer_clock(&snap, true);
                if state != PlaybackState::Playing {
                    // Played from the renderer's own controls.
                    self.shared_state.set_playback_state(PlaybackState::Playing);
                    self.report(PlaybackReportState::Playing);
                }
            }
            session::Transport::Paused => {
                if self.renderer.as_mut().is_some_and(|o| o.insist(false)) {
                    return;
                }
                self.follow_renderer_clock(&snap, false);
                if state == PlaybackState::Playing {
                    self.shared_state.set_playback_state(PlaybackState::Paused);
                    self.report(PlaybackReportState::Paused);
                }
            }
            session::Transport::Stopped | session::Transport::NoMedia => {
                if state == PlaybackState::Paused {
                    // Stopped at the renderer while paused here. Some forget
                    // the track when they stop, so play must load it again.
                    self.renderer_released();
                    return;
                }
                if state != PlaybackState::Playing || output.started.elapsed() < START_GRACE {
                    return;
                }
                let at = self.shared_state.position_ms();
                let duration = self.shared_state.duration_ms();
                if duration > 0 && at + END_TOLERANCE_MS >= duration {
                    log::info!("upnp: track finished on the renderer");
                    self.halt_renderer();
                    self.on_decode_finished();
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
        let Some(output) = self.renderer.as_ref() else {
            return;
        };
        let clock = self.shared_state.renderer_clock();
        let now = self.shared_state.position_ms();
        let moving = clock.is_some_and(|c| c.running.is_some());
        // Settling after a command, or not yet where koan sent it: either way
        // the reading says where it was, not where the music is meant to be.
        let settling = Instant::now() < output.settle_until || output.pending_seek.is_some();
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
        let output = self.renderer.as_mut().expect("caller checked");
        let Some(next) = output.next.take() else {
            return;
        };
        let id = next.id;
        let path = next.path.clone();
        output.current = Some(next);
        // Whatever was being sought belonged to the track that just ended.
        output.pending_seek = None;
        output.seek_tries = 0;
        // A hand-over starts the track at its top. The reading that showed it
        // may not: Kodi names the new track with the old one's position.
        output.settle();
        let tokens: Vec<&str> = output.tokens();
        output.session.retain(&tokens);
        log::info!("upnp: renderer moved on to {id:?}");
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: 0,
            running: Some(Instant::now()),
        }));

        if let Ok(info) = buffer::probe_file(&path) {
            self.shared_state.set_track_info(Some(TrackInfo {
                id,
                path,
                codec: info.codec,
                sample_rate: info.sample_rate,
                bit_depth: info.bit_depth,
                bitrate_kbps: info.bitrate_kbps,
                channels: info.channels,
                duration_ms: info.duration_ms,
            }));
        }
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
    use crate::player::state::PlaylistItem;
    use crate::upnp::fake::FakeRenderer;

    const WAV: &str = "http-get:*:audio/wav:*,http-get:*:audio/x-wav:*";

    fn item(path: PathBuf, title: &str) -> PlaylistItem {
        PlaylistItem {
            playlist_entry_id: None,
            id: QueueItemId::new(),
            db_id: None,
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
            let (tx, events) = crossbeam_channel::unbounded();
            let session = upnp::session::Session::open(self.fake.renderer(), move |e| {
                let _ = tx.send(e);
            })
            .unwrap();
            self.player
                .use_renderer(Some(Box::new(Connection { session, events })));
        }

        /// Run the renderer's events through the player, as its loop does,
        /// until `done` holds.
        fn pump_until(&mut self, done: impl Fn(&Player) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !done(&self.player) {
                let events = self.player.renderer_events().unwrap();
                match events.recv_timeout(Duration::from_millis(50)) {
                    Ok(event) => {
                        self.player.on_renderer_event(event);
                        self.player.renderer_tick();
                    }
                    Err(_) if Instant::now() < deadline => self.player.renderer_tick(),
                    Err(_) => panic!(
                        "timed out; renderer saw {:?}; player {:?} at {}ms, cursor {:?}",
                        self.fake.actions(),
                        self.player.shared_state.playback_state(),
                        self.player.shared_state.position_ms(),
                        self.player.shared_state.cursor(),
                    ),
                }
            }
        }

        /// Hear the renderer at `ms` into the track, past the start grace.
        fn at(&mut self, ms: u64) {
            self.settle();
            self.fake.set_position(ms);
            let output = self.player.renderer.as_mut().unwrap();
            output.started = Instant::now() - START_GRACE;
            output.settle_until = Instant::now();
            output.session.look();
            // RelTime is whole seconds.
            let floor = ms / 1000 * 1000;
            self.pump_until(|p| p.shared_state.position_ms() >= floor);
        }

        /// Take whatever answers the load left in flight, inside the grace
        /// window, before the test moves time on past it.
        fn settle(&mut self) {
            let events = self.player.renderer_events().unwrap();
            while let Ok(event) = events.recv_timeout(Duration::from_millis(100)) {
                self.player.on_renderer_event(event);
            }
        }

        /// As if the grace window after the last load had run out.
        fn past_grace(&mut self) {
            self.settle();
            let output = self.player.renderer.as_mut().unwrap();
            output.started = Instant::now() - START_GRACE;
            output.settle_until = Instant::now();
        }

        /// Run the renderer's answers through until it has been sent
        /// `action` `n` times.
        fn await_count(&mut self, action: &str, n: usize) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.count(action) < n {
                let events = self.player.renderer_events().unwrap();
                match events.recv_deadline(deadline) {
                    Ok(event) => self.player.on_renderer_event(event),
                    Err(_) => panic!("never sent {action} ×{n}: {:?}", self.fake.actions()),
                }
            }
        }

        fn state(&self) -> PlaybackState {
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
        r.player.renderer.as_mut().unwrap().started = Instant::now() - START_GRACE;
        r.player
            .on_renderer_event(session::Event::Snapshot(session::Snapshot {
                epoch: 0,
                transport: session::Transport::Stopped,
                track_uri: String::new(),
                position_ms: Some(0),
                duration_ms: None,
                at: Instant::now(),
            }));
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
            r.player.active_playback.is_some(),
            "loaded on the local output"
        );
    }

    #[test]
    fn switching_to_a_renderer_mid_download_opens_the_track_there_once_it_lands() {
        use crate::remote::downloads::{ByteFeed, Download, DownloadState, store};

        let mut r = rig(WAV, false, &["a.wav"]);
        let id = r.ids[0];
        r.player.use_renderer(None);

        // Paused locally at 3s while the file is still arriving.
        let path = r.player.shared_state.get_item(id).unwrap().path;
        r.player
            .shared_state
            .update_item_state(id, ItemState::Pending);
        let feed = ByteFeed::new();
        feed.set(crate::player::state::STREAM_THRESHOLD * 2);
        store().queued(Download {
            id,
            track_id: 0,
            title: "a".into(),
            artist: String::new(),
            source: path.clone(),
            dest: path.clone(),
            total: 0,
            written: feed.clone(),
            state: DownloadState::Queued,
            bytes_per_second: 0,
        });
        store().started(id, 0, feed);
        r.player.shared_state.set_cursor(Some(id));
        r.player.shared_state.set_track_info(Some(TrackInfo {
            id,
            path,
            codec: "PCM".into(),
            sample_rate: 8_000,
            bit_depth: Some(16),
            bitrate_kbps: None,
            channels: 1,
            duration_ms: 10_000,
        }));
        r.player.shared_state.set_position_ms(3_000);
        r.player
            .shared_state
            .set_playback_state(PlaybackState::Paused);

        r.connect();
        assert_eq!(
            r.count("SetAVTransportURI"),
            0,
            "nothing sent before it lands"
        );
        assert_eq!(r.state(), PlaybackState::Stopped);

        store().finished(id);
        r.player
            .shared_state
            .update_item_state(id, ItemState::Ready);
        r.player.track_ready(id);
        assert_eq!(r.count("SetAVTransportURI"), 1);
        assert_eq!(r.state(), PlaybackState::Paused, "still paused");
        assert_eq!(r.player.shared_state.position_ms(), 3_000);
        r.player.resume();
        let commands = r.commands();
        assert_eq!(&commands[commands.len() - 2..], ["Seek", "Play"]);
        store().withdrawn(id);
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
                .renderer
                .as_ref()
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
        assert!(r.player.active_playback.is_some(), "loaded here, paused");
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

        let (tx, events) = crossbeam_channel::unbounded();
        let session = upnp::session::Session::open(renderer, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        player.use_renderer(Some(Box::new(Connection { session, events })));
        player.play(ids[0]);

        let started = Instant::now();
        let mut moved_on = None;
        while started.elapsed() < Duration::from_secs(25) {
            let events = player.renderer_events().unwrap();
            if let Ok(event) = events.recv_timeout(Duration::from_millis(250)) {
                eprintln!("{:>6}ms {event:?}", started.elapsed().as_millis());
                player.on_renderer_event(event);
                player.queue_next_on_renderer();
            }
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
        r.player.renderer.as_ref().unwrap().session.look();
        r.pump_until(|p| p.renderer.as_ref().is_some_and(|o| o.session.epoch() > 0));
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if let Ok(e) = r
                .player
                .renderer_events()
                .unwrap()
                .recv_timeout(Duration::from_millis(50))
            {
                r.player.on_renderer_event(e);
            }
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
        r.player.cue(r.ids[0], 4_000, Start::Playing);
        assert_eq!(r.count("Seek"), 1, "tried before playing");
        r.player.renderer.as_mut().unwrap().settle_until = Instant::now();
        r.await_count("Seek", 2);
        r.pump_until(|p| {
            p.renderer
                .as_ref()
                .is_some_and(|o| o.pending_seek.is_none())
        });
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
        r.player.renderer.as_ref().unwrap().session.look();
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
            let (tx, events) = crossbeam_channel::unbounded();
            let session = upnp::session::Session::open(renderer.clone(), move |e| {
                let _ = tx.send(e);
            })
            .unwrap();
            player.use_renderer(Some(Box::new(Connection { session, events })));
        };
        // Run the player's renderer loop for `ms`.
        let run = |player: &mut Player, ms: u64| {
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                let Some(events) = player.renderer_events() else {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                };
                if let Ok(event) = events.recv_timeout(Duration::from_millis(50)) {
                    player.on_renderer_event(event);
                }
                player.renderer_tick();
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
        player.cue(ids[0], 10_000, Start::Playing);
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
            let (tx, events) = crossbeam_channel::unbounded();
            let session = upnp::session::Session::open(renderer.clone(), move |e| {
                let _ = tx.send(e);
            })
            .unwrap();
            player.use_renderer(Some(Box::new(Connection { session, events })));
        };
        let run = |player: &mut Player, ms: u64| {
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                if let Some(events) = player.renderer_events()
                    && let Ok(event) = events.recv_timeout(Duration::from_millis(20))
                {
                    player.on_renderer_event(event);
                } else {
                    std::thread::sleep(Duration::from_millis(20));
                }
                player.renderer_tick();
            }
        };
        let failures = std::cell::RefCell::new(Vec::new());
        let agree = |player: &Player, step: &str| {
            let (state, theirs, uri) = truth();
            let ours = player.shared_state.position_ms();
            let playing = player.shared_state.playback_state();
            let token = player
                .renderer
                .as_ref()
                .and_then(|r| r.current.as_ref().map(|c| c.token.clone()));
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
        upnp::connect(&udn, &tx).unwrap();
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
        upnp::connect(&udn, &tx).unwrap();
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
}
