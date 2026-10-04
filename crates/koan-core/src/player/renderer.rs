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
    /// Where a track cued paused opens once it is played. Renderers differ on
    /// whether they will seek a stopped transport, and all of them seek a
    /// playing one.
    pending_seek: Option<u64>,
    /// When koan last told it to play.
    started: Instant,
    /// Tracks this renderer cannot play, marked failed in the queue for as
    /// long as it is the output.
    refused: Vec<QueueItemId>,
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

impl RendererOutput {
    fn tokens(&self) -> Vec<&str> {
        self.current
            .iter()
            .chain(self.next.iter())
            .map(|s| s.token.as_str())
            .collect()
    }
}

impl Player {
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
                    pending_seek: None,
                    started: Instant::now(),
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
        let state = match start {
            Start::Playing => {
                output
                    .session
                    .play()
                    .map_err(|e| PlayerError::Renderer(e.to_string()))?;
                output.started = Instant::now();
                if seek_ms > 0 {
                    let _ = output.session.seek(seek_ms);
                }
                PlaybackState::Playing
            }
            Start::Paused => {
                output.pending_seek = (seek_ms > 0).then_some(seek_ms);
                PlaybackState::Paused
            }
        };
        output.session.look();
        self.shared_state.update_renderer(|o| o.problem = None);
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: seek_ms,
            running: (state == PlaybackState::Playing).then(Instant::now),
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
                output.next_refused = Some(next_id);
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
        let Some(output) = self.renderer.as_ref() else {
            return;
        };
        if output.current.is_none() {
            return;
        }
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
        if let Err(e) = output.session.play() {
            log::error!("upnp: resume failed: {e}");
            return;
        }
        output.started = Instant::now();
        let mut at = self.shared_state.position_ms();
        if let Some(seek) = output.pending_seek.take() {
            let _ = output.session.seek(seek);
            at = seek;
        }
        output.session.look();
        self.shared_state.set_renderer_clock(Some(RendererClock {
            position_ms: at,
            running: Some(Instant::now()),
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
            if let Err(e) = output.session.seek(position_ms) {
                log::warn!("upnp: seek refused: {e}");
                return true;
            }
            output.session.look();
        } else {
            output.pending_seek = Some(position_ms);
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

        // Moved on to the track it was given next: gapless, from here.
        if token.is_some() && output.next.as_ref().map(|n| &n.token) == token.as_ref() {
            self.renderer_moved_on(snap.position_ms.unwrap_or(0));
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
            session::Transport::Playing => {
                let at = snap
                    .position_ms
                    .unwrap_or_else(|| self.shared_state.position_ms());
                self.shared_state.set_renderer_clock(Some(RendererClock {
                    position_ms: at,
                    running: Some(snap.at),
                }));
                if state != PlaybackState::Playing {
                    // Played from the renderer's own controls.
                    self.shared_state.set_playback_state(PlaybackState::Playing);
                    self.report(PlaybackReportState::Playing);
                }
            }
            session::Transport::Paused => {
                let at = snap
                    .position_ms
                    .unwrap_or_else(|| self.shared_state.position_ms());
                self.shared_state.set_renderer_clock(Some(RendererClock {
                    position_ms: at,
                    running: None,
                }));
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

    /// The renderer started the track it had been given next.
    fn renderer_moved_on(&mut self, position_ms: u64) {
        self.bank_listening();
        let output = self.renderer.as_mut().expect("caller checked");
        let Some(next) = output.next.take() else {
            return;
        };
        let id = next.id;
        let path = next.path.clone();
        output.current = Some(next);
        let tokens: Vec<&str> = output.tokens();
        output.session.retain(&tokens);
        log::info!("upnp: renderer moved on to {id:?}");

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
        self.on_track_changed(id, position_ms);
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
                    crate::test_utils::generate_wav(&path, 8_000, 1, 10.0, 16);
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
                match events.recv_deadline(deadline) {
                    Ok(event) => {
                        self.player.on_renderer_event(event);
                        self.player.queue_next_on_renderer();
                    }
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
            self.player.renderer.as_mut().unwrap().started = Instant::now() - START_GRACE;
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
        r.at(9_500);
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
        let seek = r.last_command();
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
        assert_eq!(&commands[commands.len() - 2..], ["Play", "Seek"]);
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
        assert_eq!(&commands[commands.len() - 2..], ["Play", "Seek"]);
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
        let seek = r.last_command();
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
}
