//! The system's media controls: MPRIS on Linux, Now Playing and the remote
//! commands on macOS, through souvlaki.
//!
//! Driven by a state stream of its own, so it follows the engine whatever the
//! window is doing. The same stream keeps `Playhead`, which the skip
//! shortcuts read, whether or not the platform's controls could be reached. While another device is controlled, `Playback` and
//! `Playhead` describe that device, so the media keys act on it too, as they
//! do in the terminal UI.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_channel::mpsc;
use futures_util::StreamExt as _;
use koan_ffi::{Anchor, KoanEngine, NowPlaying, PlayState, StateSlice};
use koan_kit::raw::{App, AsyncApp, Global};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

/// How far a seek that names only a direction moves the playhead.
const SEEK_STEP_MS: u64 = 10_000;

/// The size of cover handed to the system: Now Playing and the shells that
/// read MPRIS draw it at a few hundred points at most.
const COVER_SIZE: u32 = 600;

/// What the media controls were last told of the track.
#[derive(Clone, Debug, Default, PartialEq)]
struct Shown {
    title: String,
    artist: String,
    album: String,
    duration_ms: Option<u64>,
    track_id: Option<i64>,
}

/// What a media control asks of the player.
#[derive(Clone, Debug, PartialEq)]
enum Command {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    Seek(u64),
    Raise,
    Quit,
}

/// The track as the media controls show it, or `None` when nothing is under
/// the cursor.
fn shown_of(now: &NowPlaying) -> Option<Shown> {
    let entry = now.entry.as_ref()?;
    Some(Shown {
        title: entry.title.clone(),
        artist: entry.artist.clone(),
        album: entry.album.clone(),
        duration_ms: entry
            .duration_ms
            .or((now.duration_ms > 0).then_some(now.duration_ms)),
        track_id: entry.track_id,
    })
}

/// The transport as the media controls show it.
fn playback_of(state: PlayState, anchor: Option<Anchor>, now: Instant) -> MediaPlayback {
    let progress = anchor.map(|a| MediaPosition(Duration::from_millis(a.reckoning(now))));
    match state {
        PlayState::Playing => MediaPlayback::Playing { progress },
        PlayState::Paused => MediaPlayback::Paused { progress },
        PlayState::Stopped => MediaPlayback::Stopped,
    }
}

/// How far a playhead heard again may differ from the one held before it is
/// news: the engine itself republishes past 32 ms.
const PLAYHEAD_TOLERANCE_MS: u64 = 250;

/// Whether a playhead heard at `now` says something the one held does not:
/// a seek, a pause, a stall, rather than the same reckoning sent again.
fn moved(held: Option<Anchor>, position_ms: u64, playing: bool, now: Instant) -> bool {
    held.is_none_or(|a| a.stale(position_ms, playing, now, PLAYHEAD_TOLERANCE_MS))
}

/// What a media control event asks for, given where the playhead is now.
fn command_of(event: MediaControlEvent, position_ms: u64) -> Option<Command> {
    let step = |direction: SeekDirection, by: u64| match direction {
        SeekDirection::Forward => position_ms.saturating_add(by),
        SeekDirection::Backward => position_ms.saturating_sub(by),
    };
    Some(match event {
        MediaControlEvent::Play => Command::Play,
        MediaControlEvent::Pause => Command::Pause,
        MediaControlEvent::Toggle => Command::Toggle,
        MediaControlEvent::Next => Command::Next,
        MediaControlEvent::Previous => Command::Previous,
        MediaControlEvent::Stop => Command::Stop,
        MediaControlEvent::SetPosition(MediaPosition(at)) => Command::Seek(at.as_millis() as u64),
        MediaControlEvent::SeekBy(direction, by) => {
            Command::Seek(step(direction, by.as_millis() as u64))
        }
        MediaControlEvent::Seek(direction) => Command::Seek(step(direction, SEEK_STEP_MS)),
        MediaControlEvent::Raise => Command::Raise,
        MediaControlEvent::Quit => Command::Quit,
        _ => return None,
    })
}

/// What the covers handed to the system are called, in the configuration
/// directory.
const COVER_PREFIX: &str = "gui-cover-";

/// This process's covers: named for it, so sweeping an earlier run's never
/// touches one this run has written.
fn own_prefix() -> String {
    format!("{COVER_PREFIX}{}-", std::process::id())
}

/// Where the cover for the `generation`th track is written for the system to
/// read: a name of its own per track, since a shell that keys its image cache
/// on the URL would otherwise show an earlier track's cover. The one before
/// it is removed once the system has been pointed at this one.
fn cover_path(generation: u64, mime: &str) -> PathBuf {
    let ext = match mime {
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "jpg",
    };
    koan_core::config::config_dir().join(format!("{}{generation}.{ext}", own_prefix()))
}

/// Remove the covers an earlier run left, which a crash or a kill does.
fn sweep_covers() {
    let dir = koan_core::config::config_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let own = own_prefix();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(COVER_PREFIX) && !name.starts_with(&own) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// `path` as a `file://` URL, every byte outside RFC 3986's unreserved set
/// and the path's slashes percent-encoded: Now Playing's `NSURL` refuses a
/// space, and MPRIS's `artUrl` is a URI.
fn file_url(path: &std::path::Path) -> String {
    let mut url = String::from("file://");
    for &byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

/// Write `bytes` where the system will read them, whole or not at all.
fn write_cover(path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    let partial = path.with_extension("part");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

/// The playhead as last heard, for anything that has to work out where it is
/// now.
#[derive(Default)]
pub struct Playhead(pub Option<Anchor>);

impl Global for Playhead {}

impl Playhead {
    /// Where the playhead is now, or zero before anything has been heard.
    pub fn now(cx: &App) -> u64 {
        cx.try_global::<Playhead>()
            .and_then(|p| p.0)
            .map_or(0, |a| a.reckoning(Instant::now()))
    }
}

struct Media {
    /// `None` where the platform's controls cannot be reached.
    controls: Option<MediaControls>,
    shown: Option<Shown>,
    state: PlayState,
    anchor: Option<Anchor>,
    /// Bumped on each new track, so a cover that arrives for an earlier one
    /// is dropped.
    generation: u64,
    /// The cover file the system was last pointed at.
    cover: Option<PathBuf>,
}

impl Media {
    fn tell_metadata(&mut self) {
        let Some(controls) = self.controls.as_mut() else {
            return;
        };
        let url = self.cover.as_deref().map(file_url);
        // Nothing under the cursor clears what was shown.
        let metadata = self
            .shown
            .as_ref()
            .map_or_else(MediaMetadata::default, |shown| MediaMetadata {
                title: Some(&shown.title),
                artist: Some(&shown.artist),
                album: Some(&shown.album),
                cover_url: url.as_deref(),
                duration: shown.duration_ms.map(Duration::from_millis),
            });
        let told = controls.set_metadata(metadata);
        if let Err(e) = told {
            log::warn!("media controls: setting the track: {e:?}");
        }
        // Now Playing's dictionary is replaced whole, the elapsed time with
        // it: without this the position starts again from zero.
        self.tell_playback();
    }

    fn tell_playback(&mut self) {
        let Some(controls) = self.controls.as_mut() else {
            return;
        };
        let playback = playback_of(self.state, self.anchor, Instant::now());
        if let Err(e) = controls.set_playback(playback) {
            log::warn!("media controls: setting the transport: {e:?}");
        }
    }
}

/// Start following the engine, into the system's media controls where they
/// can be reached (a Linux session without a D-Bus session bus has none), for
/// the life of the app.
pub fn start(engine: Arc<KoanEngine>, cx: &mut App) {
    cx.set_global(Playhead::default());
    let (events, asks) = mpsc::unbounded();
    let controls = controls(events);
    let reachable = controls.is_some();
    if reachable {
        cx.background_executor()
            .spawn(async { sweep_covers() })
            .detach();
    }
    let media = Rc::new(RefCell::new(Media {
        controls,
        shown: None,
        state: PlayState::Stopped,
        anchor: None,
        generation: 0,
        cover: None,
    }));

    let stream = engine.observe();
    {
        let (media, engine) = (media.clone(), engine.clone());
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(batch) = stream.next().await {
                for slice in batch {
                    apply(slice, &media, &engine, cx);
                }
            }
        })
        .detach();
    }
    if reachable {
        cx.spawn(async move |cx: &mut AsyncApp| answer(asks, engine, cx).await)
            .detach();
    }
}

fn controls(events: mpsc::UnboundedSender<MediaControlEvent>) -> Option<MediaControls> {
    // MPRIS's name for one of several instances: the terminal UI's is `koan`,
    // and a desktop app on another configuration may be running too.
    let dbus_name = format!("koan.gui.instance{}", std::process::id());
    let config = PlatformConfig {
        dbus_name: &dbus_name,
        display_name: "kōan",
        hwnd: None,
    };
    let mut controls = MediaControls::new(config)
        .inspect_err(|e| log::warn!("media controls unavailable: {e:?}"))
        .ok()?;
    // Called on souvlaki's own thread on Linux, so it only hands the event on.
    controls
        .attach(move |event| {
            let _ = events.unbounded_send(event);
        })
        .inspect_err(|e| log::warn!("media controls unavailable: {e:?}"))
        .ok()?;
    Some(controls)
}

/// Act on the media controls' events until they stop.
async fn answer(
    mut asks: mpsc::UnboundedReceiver<MediaControlEvent>,
    engine: Arc<KoanEngine>,
    cx: &mut AsyncApp,
) {
    while let Some(event) = asks.next().await {
        let position = cx.update(|cx| Playhead::now(cx));
        let Some(command) = command_of(event, position) else {
            continue;
        };
        let engine = engine.clone();
        let sent = match command {
            Command::Play => engine.resume().await,
            Command::Pause => engine.pause().await,
            Command::Toggle => engine.toggle_play_pause().await,
            Command::Next => engine.next().await,
            Command::Previous => engine.previous().await,
            Command::Stop => engine.stop().await,
            Command::Seek(at) => engine.seek(at).await,
            Command::Raise => {
                cx.update(super::to_front);
                Ok(())
            }
            Command::Quit => {
                cx.update(|cx| cx.quit());
                Ok(())
            }
        };
        if let Err(e) = sent {
            log::warn!("media controls: {e}");
        }
    }
}

fn apply(
    slice: StateSlice,
    media: &Rc<RefCell<Media>>,
    engine: &Arc<KoanEngine>,
    cx: &mut AsyncApp,
) {
    match slice {
        StateSlice::Playback { now_playing } => {
            let shown = shown_of(&now_playing);
            let mut m = media.borrow_mut();
            let state_moved = m.state != now_playing.state;
            m.state = now_playing.state;
            if shown != m.shown {
                let new_track = shown.as_ref().and_then(|s| s.track_id)
                    != m.shown.as_ref().and_then(|s| s.track_id);
                m.shown = shown;
                if new_track {
                    m.generation += 1;
                    if let Some(old) = m.cover.take() {
                        remove(old, cx);
                    }
                    let track_id = m.shown.as_ref().and_then(|s| s.track_id);
                    if let (Some(track_id), true) = (track_id, m.controls.is_some()) {
                        fetch_cover(track_id, m.generation, media, engine, cx);
                    }
                }
                m.tell_metadata();
            }
            if state_moved {
                m.tell_playback();
            }
        }
        StateSlice::Playhead {
            position_ms,
            playing,
            ..
        } => {
            let now = Instant::now();
            let mut m = media.borrow_mut();
            // The slice is published again as a download extends how far the
            // track can be dragged, with the playhead where it was reckoned.
            if !moved(m.anchor, position_ms, playing, now) {
                return;
            }
            let anchor = Anchor {
                position_ms,
                playing,
                at: now,
            };
            cx.update(|cx| cx.set_global(Playhead(Some(anchor))));
            m.anchor = Some(anchor);
            m.tell_playback();
        }
        _ => {}
    }
}

fn fetch_cover(
    track_id: i64,
    generation: u64,
    media: &Rc<RefCell<Media>>,
    engine: &Arc<KoanEngine>,
    cx: &AsyncApp,
) {
    let (media, engine) = (media.clone(), engine.clone());
    cx.spawn(async move |cx: &mut AsyncApp| {
        let art = match engine.cover_art(track_id, Some(COVER_SIZE)).await {
            Ok(Some(art)) => art,
            Ok(None) => return,
            Err(e) => {
                log::warn!("media controls: cover for track {track_id}: {e}");
                return;
            }
        };
        let path = cover_path(generation, &art.mime);
        let written = cx
            .background_executor()
            .spawn(async move { write_cover(&path, &art.data).map(|()| path) })
            .await;
        let path = match written {
            Ok(path) => path,
            Err(e) => {
                log::warn!("media controls: writing the cover: {e}");
                return;
            }
        };
        let mut m = media.borrow_mut();
        if m.generation != generation {
            remove(path, cx);
            return;
        }
        if let Some(old) = m.cover.replace(path) {
            remove(old, cx);
        }
        m.tell_metadata();
    })
    .detach();
}

/// Remove a cover file, off the UI thread.
fn remove(path: PathBuf, cx: &AsyncApp) {
    cx.background_executor()
        .spawn(async move {
            let _ = std::fs::remove_file(path);
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use koan_ffi::{EntryStatus, QueueItem, RepeatMode, RunRole};

    fn item(title: &str, duration_ms: Option<u64>) -> QueueItem {
        QueueItem {
            queue_item_id: "q".into(),
            track_id: Some(7),
            album_id: None,
            artist_id: None,
            album_artist_id: None,
            title: title.into(),
            artist: "Polar Bear".into(),
            album_artist: "Polar Bear".into(),
            artist_credit: vec![],
            album_artist_credit: vec![],
            credit_differs: false,
            run: RunRole::Alone,
            album: "Held on the Tips of Fingers".into(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms,
            status: EntryStatus::Playing,
            playlist_entry_id: None,
            failure_reason: None,
            on_server: false,
            on_disk: true,
        }
    }

    fn now_playing(entry: Option<QueueItem>, duration_ms: u64) -> NowPlaying {
        NowPlaying {
            state: PlayState::Playing,
            waiting: false,
            position_ms: 0,
            duration_ms,
            queue_item_id: None,
            entry,
            format: None,
            playlist_version: 0,
            shuffle: false,
            repeat_mode: RepeatMode::Off,
            undo_shuffles: 0,
            queue_shufflable: false,
            sleep: None,
            sleep_fading: false,
            start_failure: None,
        }
    }

    #[test]
    fn the_track_under_the_cursor_is_what_is_shown() {
        let shown = shown_of(&now_playing(Some(item("Beartown", Some(312_000))), 0)).unwrap();
        assert_eq!(shown.title, "Beartown");
        assert_eq!(shown.artist, "Polar Bear");
        assert_eq!(shown.album, "Held on the Tips of Fingers");
        assert_eq!(shown.duration_ms, Some(312_000));
        assert_eq!(shown.track_id, Some(7));
    }

    #[test]
    fn a_track_with_no_length_of_its_own_takes_the_decoders() {
        let shown = shown_of(&now_playing(Some(item("Beartown", None)), 90_000)).unwrap();
        assert_eq!(shown.duration_ms, Some(90_000));
        let unknown = shown_of(&now_playing(Some(item("Beartown", None)), 0)).unwrap();
        assert_eq!(unknown.duration_ms, None);
    }

    #[test]
    fn nothing_under_the_cursor_shows_nothing() {
        assert_eq!(shown_of(&now_playing(None, 0)), None);
    }

    #[test]
    fn the_position_runs_on_while_playing_and_holds_while_paused() {
        let at = Instant::now();
        let later = at + Duration::from_millis(1_500);
        let running = Anchor {
            position_ms: 10_000,
            playing: true,
            at,
        };
        let held = Anchor {
            playing: false,
            ..running
        };
        assert_eq!(
            playback_of(PlayState::Playing, Some(running), later),
            MediaPlayback::Playing {
                progress: Some(MediaPosition(Duration::from_millis(11_500)))
            }
        );
        assert_eq!(
            playback_of(PlayState::Paused, Some(held), later),
            MediaPlayback::Paused {
                progress: Some(MediaPosition(Duration::from_millis(10_000)))
            }
        );
        assert_eq!(
            playback_of(PlayState::Stopped, Some(held), later),
            MediaPlayback::Stopped
        );
        assert_eq!(
            playback_of(PlayState::Paused, None, later),
            MediaPlayback::Paused { progress: None }
        );
    }

    #[test]
    fn a_playhead_sent_again_as_it_was_is_not_news() {
        let at = Instant::now();
        let held = Anchor {
            position_ms: 10_000,
            playing: true,
            at,
        };
        let later = at + Duration::from_millis(2_000);
        assert!(
            !moved(Some(held), 12_000, true, later),
            "the same reckoning"
        );
        assert!(moved(Some(held), 40_000, true, later), "a seek");
        assert!(moved(Some(held), 12_000, false, later), "a pause");
        assert!(moved(None, 0, false, later), "the first");
    }

    #[test]
    fn transport_events_map_to_their_commands() {
        let cases = [
            (MediaControlEvent::Play, Command::Play),
            (MediaControlEvent::Pause, Command::Pause),
            (MediaControlEvent::Toggle, Command::Toggle),
            (MediaControlEvent::Next, Command::Next),
            (MediaControlEvent::Previous, Command::Previous),
            (MediaControlEvent::Stop, Command::Stop),
            (MediaControlEvent::Raise, Command::Raise),
            (MediaControlEvent::Quit, Command::Quit),
        ];
        for (event, command) in cases {
            assert_eq!(command_of(event, 0), Some(command));
        }
    }

    #[test]
    fn a_set_position_is_absolute() {
        let event = MediaControlEvent::SetPosition(MediaPosition(Duration::from_millis(42_000)));
        assert_eq!(command_of(event, 5_000), Some(Command::Seek(42_000)));
    }

    #[test]
    fn a_relative_seek_moves_from_the_playhead_and_stops_at_zero() {
        let forward = MediaControlEvent::SeekBy(SeekDirection::Forward, Duration::from_secs(5));
        assert_eq!(command_of(forward, 20_000), Some(Command::Seek(25_000)));
        let back = MediaControlEvent::SeekBy(SeekDirection::Backward, Duration::from_secs(30));
        assert_eq!(command_of(back, 20_000), Some(Command::Seek(0)));
        let step = MediaControlEvent::Seek(SeekDirection::Forward);
        assert_eq!(command_of(step, 1_000), Some(Command::Seek(11_000)));
    }

    #[test]
    fn volume_and_uris_are_not_ours_to_take() {
        assert_eq!(command_of(MediaControlEvent::SetVolume(0.5), 0), None);
        assert_eq!(
            command_of(MediaControlEvent::OpenUri("file:///x.flac".into()), 0),
            None
        );
    }

    #[test]
    fn a_cover_url_is_percent_encoded() {
        assert_eq!(
            file_url(std::path::Path::new("/Users/x y/kōan #1/c.jpg")),
            "file:///Users/x%20y/k%C5%8Dan%20%231/c.jpg"
        );
    }

    #[test]
    fn covers_are_named_for_this_process() {
        let name = cover_path(1, "image/jpeg");
        assert!(
            name.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&own_prefix())
        );
    }

    #[test]
    fn each_track_has_a_cover_name_of_its_own() {
        let a = cover_path(1, "image/jpeg");
        let c = cover_path(3, "image/jpeg");
        assert_ne!(a, c);
        assert_eq!(a.extension().unwrap(), "jpg");
        assert_eq!(cover_path(2, "image/png").extension().unwrap(), "png");
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(COVER_PREFIX)
        );
    }
}
