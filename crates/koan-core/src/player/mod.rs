pub mod commands;
pub mod history;
mod renderer;
mod sleep;
pub mod state;
pub mod undo;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use thiserror::Error;

use crate::audio::{
    analyzer::VizAnalyzer,
    backend::{self, AudioBackend, AudioEngineHandle, BackendError, SampleRateWatch},
    buffer, streaming,
    viz::{VizBuffer, VizSnapshot},
};
use crate::remote::client::PlaybackReportState;
use buffer::PlaybackTimeline;
use commands::{CommandChannel, PlayerCommand};
use history::{InFlight, PlayEvent, PlayRecorder, PlaybackReport};
use state::{
    ItemState, PlayMode, PlaybackSource, PlaybackState, QueueItemId, Repeat, SharedPlayerState,
    Sleep, SleepTimer, TrackInfo,
};
use undo::{UndoEntry, UndoStack};

/// Ring buffer size in samples. ~1s at 192kHz stereo.
pub(crate) const RING_BUFFER_SIZE: usize = 192_000 * 2;

/// Kept back from the end of a track when seeking, so dragging the thumb all
/// the way over lands in the last moment of it rather than in the next track.
const SEEK_END_GUARD_MS: u64 = 500;

/// Past the moment the next track should start, so the wake finds the
/// playhead already in it rather than a hair short.
const BOUNDARY_SLACK: std::time::Duration = std::time::Duration::from_millis(5);

/// How often to look at a fading pause, for the few checks it takes to reach
/// silence.
const FADE_CHECK: std::time::Duration = std::time::Duration::from_millis(50);
/// How long a DSP change waits for its fade before restarting regardless: an
/// output that has stopped calling back never reports silence.
const QUICK_FADE_LIMIT: std::time::Duration = std::time::Duration::from_millis(100);

/// How long output stays stopped before the audio session is given back. Long
/// enough that pausing to answer someone does not hand the speaker to another
/// app; short enough that the app paused before koan came along resumes soon.
const SESSION_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// When to look next for a DSP change's fade, begun at `since`, to have
/// reached silence, for callbacks `period` long. The fade ends a known time
/// after it began: once the callback next runs, up to a period later, and for
/// the fade's length. Woken then, again a period on for a callback that ran
/// late, then at the limit should the output have stopped calling back.
fn dsp_wake(
    since: std::time::Instant,
    period: std::time::Duration,
    now: std::time::Instant,
) -> std::time::Instant {
    let faded = since + crate::audio::fade::QUICK_FADE + period + BOUNDARY_SLACK;
    [faded, faded + period, since + QUICK_FADE_LIMIT]
        .into_iter()
        .find(|&t| t > now)
        .unwrap_or(now)
}

#[derive(Debug, Error)]
pub enum PlayerError {
    #[error("backend error: {0}")]
    Backend(#[from] BackendError),
    #[error("decode error: {0}")]
    Decode(#[from] buffer::DecodeError),
    #[error("renderer: {0}")]
    Renderer(String),
    /// The renderer cannot play this track. It is marked as such in the
    /// queue and skipped.
    #[error("{0}")]
    Unplayable(String),
}

/// Everything needed to read a track that is still downloading: where it is,
/// how far the transfer has got, and how the container has to be opened.
#[derive(Clone)]
struct StreamSource {
    path: PathBuf,
    bytes_written: Arc<crate::remote::downloads::ByteFeed>,
    total: u64,
    mode: streaming::ProbeMode,
}

/// A file's own extension, lowercased. A download in progress is named
/// `track.m4a.part`, and its extension is the track's, not `part`.
fn media_extension(path: &Path) -> Option<String> {
    crate::remote::download::strip_part_suffix(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

/// Symphonia's format hint for a path — its extension, where it has one.
fn hint_for(path: &Path) -> symphonia::core::formats::probe::Hint {
    let mut hint = symphonia::core::formats::probe::Hint::new();
    if let Some(ext) = media_extension(path) {
        hint.with_extension(&ext);
    }
    hint
}

/// How to open a partial file once the whole description has failed.
///
/// Most containers describe their frames from the front and need an end they
/// can actually reach: FLAC bisects towards the end it is given. Two need the
/// whole file's end instead. Ogg takes the end it is handed as the end of the
/// stream, so a file ending at the write head is a track already over. MP4
/// bounds its top-level boxes by the end, so a `moov` larger than what has
/// arrived overruns it and the file will not open until it has all landed.
/// See `ProbeMode`.
fn lengthless_mode_for(path: &Path) -> streaming::ProbeMode {
    match media_extension(path).as_deref() {
        Some("ogg" | "oga" | "opus" | "spx" | "m4a" | "m4b" | "mp4" | "mov") => {
            streaming::ProbeMode::LengthlessWholeEnd
        }
        _ => streaming::ProbeMode::Lengthless,
    }
}

/// The player controller. Owns the audio pipeline and processes commands.
pub struct Player {
    shared_state: Arc<SharedPlayerState>,
    commands: CommandChannel,
    /// What the player is doing with the track under the cursor.
    transport: Transport,
    timeline: Arc<PlaybackTimeline>,
    viz_buffer: Arc<VizBuffer>,
    viz_snapshot: Arc<VizSnapshot>,
    /// Background FFT analysis thread. Held for its lifetime; dropped on Player drop.
    _viz_analyzer: VizAnalyzer,
    undo_stack: UndoStack,
    /// When Some, undo entries are collected into this buffer instead of pushed
    /// directly onto the undo stack. Flushed on EndUndoBatch.
    batch_buffer: Option<Vec<UndoEntry>>,
    /// Configured output device name. None = system default.
    output_device_name: Option<String>,
    /// Platform audio backend (CoreAudio on macOS and iOS, cpal on Linux).
    backend: Box<dyn AudioBackend>,
    /// How the file currently streaming had to be opened. A seek reopens it and
    /// must not undo what the probe settled on.
    stream_mode: streaming::ProbeMode,
    /// Writes plays away from this thread. None when there is no database to
    /// write to, and in tests, which must not touch the real library.
    history: Option<PlayRecorder>,
    /// This player's download queue, which follows its playlist. Held here so
    /// it lives as long as the player it fetches for. `None` for a player
    /// made without `spawn`, which fetches nothing.
    downloads: Option<crate::remote::queue::DownloadQueue>,
    /// How much of the current track has been heard so far.
    in_flight: Option<InFlight>,
    /// When the silence after a rate switch runs out and the track is heard.
    lead_in_ends: Option<std::time::Instant>,
    /// When the audio session goes back to the system, output having stopped:
    /// see `audio::ios_backend::AudioSession`.
    session_release_at: Option<std::time::Instant>,
    /// Bumped by every session opened and every one torn down, so that what a
    /// torn-down session reports after the fact is recognised as stale.
    session: u64,
    /// Waiting for a pause's fade to reach silence, to hear where it did.
    silence_waiters: Vec<crossbeam_channel::Sender<u64>>,
    /// The DSP setup last loaded, and the config and device it was loaded
    /// for. Reading impulse responses off disk on every seek would be wasted.
    dsp: Option<DspCache>,
    /// A DSP change fading the old processing out, since when: the session
    /// restarts with the new one once that reaches silence. Changes made
    /// meanwhile ride the same restart, which loads whatever is current.
    dsp_restart: Option<std::time::Instant>,
    /// Open the next session's output with a quick fade in: it follows a DSP
    /// change's quick fade out.
    quick_start: bool,
    /// The UPnP renderer chosen as the output, when one is: every session
    /// opened while it is set plays there. Not a transport of its own.
    renderer: Option<renderer::RendererLink>,
    /// Shuffle and repeat. Published by `publish`, which the queue's own
    /// reads follow.
    mode: PlayMode,
    /// The sleep timer, while one is set.
    sleep: Option<SleepSet>,
    /// Its fade, while one runs or full level is being brought back.
    sleep_fade: Option<sleep::SleepFade>,
    /// Paused by hand during a fade: resuming plays at full level.
    sleep_snap_on_resume: bool,
    /// Playback sessions started — lets tests assert how many engine restarts
    /// an operation costs.
    #[cfg(test)]
    playback_starts: usize,
    /// What `dsp_for` answers in tests, in place of reading the profiles
    /// from config: config is the process's, and the suite runs in parallel.
    /// The first is this device's, the second the renderer's.
    #[cfg(test)]
    dsp_override: Option<Arc<crate::audio::dsp::Setup>>,
    #[cfg(test)]
    renderer_dsp_override: Option<Arc<crate::audio::dsp::Setup>>,
    /// The renderer used last time is being looked for, and is to be gone
    /// back to if it turns up before anyone plays or picks an output.
    resume_renderer: bool,
    /// What the session restored at launch asked for while that renderer is
    /// looked for: it waits paused, so nothing starts here that the renderer
    /// might still take, and is played wherever it lands.
    held_run: Option<Run>,
}

/// A sleep timer that is set: what is published, and for one set for a time,
/// the instant the player wakes to end playback.
#[derive(Clone, Copy)]
struct SleepSet {
    sleep: Sleep,
    at: Option<std::time::Instant>,
    /// How long the fade before `at` runs.
    fade: Option<std::time::Duration>,
}

struct DspCache {
    config: Arc<crate::config::Config>,
    device: String,
    setup: Option<Arc<crate::audio::dsp::Setup>>,
}

/// How a pause falls silent.
#[derive(Clone, Copy)]
enum Fade {
    Cut,
    Short,
}

/// Whether a session plays or sits paused, and how a track waited for opens.
/// A fade out is `Paused` with the engine still running until it is silent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Run {
    Playing,
    Paused,
}

/// What the player is doing. Everything it publishes — the playback state,
/// the wait, the track — is derived from this in `publish`, so none of them
/// can disagree with it or with each other.
enum Transport {
    /// Nothing loaded and nothing asked for.
    Idle,
    /// A track asked for that cannot open yet. Nothing opens a track without
    /// one of these or a command naming it.
    Waiting(Waiting),
    /// A session is open, decoding into the ring.
    Loaded(Session),
}

/// A track that cannot open yet, and how it opens once it can: at the start
/// as soon as enough of it has streamed, or at a later position once the whole
/// file is on disk. Streaming it would start it before the position can be
/// reached, and a seek once it can would let the part before it be heard.
#[derive(Clone, Copy)]
struct Waiting {
    id: QueueItemId,
    position_ms: u64,
    start: Run,
}

impl Waiting {
    fn may_stream(&self, id: QueueItemId) -> bool {
        self.id == id && self.position_ms == 0
    }
}

/// An open playback session: the track under the playhead, whether it plays,
/// and the output it plays on.
struct Session {
    /// The track under the playhead. Opened with the first; moved on by a
    /// gapless transition, corrected when a download it streams lands.
    track: TrackInfo,
    run: Run,
    /// The steps the decoder has taken through the queue, in order. Always
    /// empty for a renderer, which has no decoder here to look ahead.
    lookahead: Arc<parking_lot::Mutex<Vec<state::Lookahead>>>,
    output: Output,
}

/// Where a session's sound comes out, and so whether koan decodes it.
enum Output {
    /// Decoded here, into the ring, drained by an engine: this device's own
    /// output.
    Local(Local),
    /// A renderer: handed the original file, which it decodes and plays
    /// gaplessly itself, or a stream decoded and processed here. See
    /// `renderer`.
    Renderer(Box<renderer::Play>),
}

struct Local {
    engine: Box<dyn AudioEngineHandle>,
    decode_handle: buffer::DecodeHandle,
    /// Set when the decoder is reading a download as it arrives.
    stream: Option<LiveStream>,
    /// Keeps the device rate subscription alive for as long as this engine is
    /// the one feeding the DAC. Dropped with it.
    _rate_watch: Option<Box<dyn SampleRateWatch>>,
    /// What the output device's profile does to this session, for the badge.
    dsp: Option<crate::audio::dsp::DspStatus>,
}

impl Session {
    /// The engine, for a session on this device.
    fn engine(&self) -> Option<&dyn AudioEngineHandle> {
        match &self.output {
            Output::Local(local) => Some(local.engine.as_ref()),
            Output::Renderer(_) => None,
        }
    }
}

/// Where a session reads its first track from.
enum Source {
    File(PathBuf),
    Stream(StreamSource),
}

impl Source {
    fn path(&self) -> &Path {
        match self {
            Source::File(path) => path,
            Source::Stream(source) => &source.path,
        }
    }
}

/// A download being decoded as it lands. The reader may be parked at the write
/// head waiting for bytes, so stopping has to tell it to give up and wake it,
/// or the join waits on the network.
struct LiveStream {
    feed: Arc<crate::remote::downloads::ByteFeed>,
    abandoned: Arc<std::sync::atomic::AtomicBool>,
}

impl LiveStream {
    fn abandon(&self) {
        self.abandoned
            .store(true, std::sync::atomic::Ordering::Release);
        self.feed.done();
    }
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Self {
        let viz_buffer = VizBuffer::new();
        let viz_snapshot = VizSnapshot::new();
        let timeline = PlaybackTimeline::new();
        let cfg = crate::config::Config::cached();
        let viz_analyzer = VizAnalyzer::spawn_with_snapshot(
            Arc::clone(&viz_buffer),
            &cfg.visualizer,
            Arc::clone(&viz_snapshot),
            timeline.samples_played_counter(),
        );

        let shared_state = SharedPlayerState::new();
        shared_state.attach_timeline(timeline.clone());
        let commands = CommandChannel::new();
        let tx = commands.tx.clone();
        timeline.on_queued(move || {
            let _ = tx.try_send(PlayerCommand::TrackQueued);
        });

        Self {
            shared_state,
            commands,
            transport: Transport::Idle,
            lead_in_ends: None,
            session_release_at: None,
            dsp: None,
            dsp_restart: None,
            quick_start: false,
            session: 0,
            silence_waiters: Vec::new(),
            renderer: None,
            mode: PlayMode::default(),
            sleep: None,
            sleep_fade: None,
            sleep_snap_on_resume: false,
            timeline,
            viz_buffer,
            viz_snapshot,
            _viz_analyzer: viz_analyzer,
            undo_stack: UndoStack::new(),
            batch_buffer: None,
            output_device_name: cfg.playback.output_device.clone(),
            backend: crate::audio::platform_backend(),
            stream_mode: streaming::ProbeMode::Full,
            history: None,
            downloads: None,
            in_flight: None,
            #[cfg(test)]
            playback_starts: 0,
            #[cfg(test)]
            dsp_override: None,
            #[cfg(test)]
            renderer_dsp_override: None,
            resume_renderer: false,
            held_run: None,
        }
    }

    /// Get a clone of the shared state for UI reads.
    pub fn shared_state(&self) -> Arc<SharedPlayerState> {
        self.shared_state.clone()
    }

    /// Get the playback timeline for UI reads.
    pub fn timeline(&self) -> Arc<PlaybackTimeline> {
        self.timeline.clone()
    }

    /// Get the visualization buffer for the TUI.
    pub fn viz_buffer(&self) -> Arc<VizBuffer> {
        self.viz_buffer.clone()
    }

    /// Get the shared analysis snapshot for the TUI.
    /// The analysis thread writes here; the UI thread reads a clone each frame.
    pub fn viz_snapshot(&self) -> Arc<VizSnapshot> {
        self.viz_snapshot.clone()
    }

    /// Access undo stack (for tests and UI state queries).
    pub fn undo_stack(&self) -> &UndoStack {
        &self.undo_stack
    }

    /// Create an audio engine for a stream, switching the output device to the
    /// source rate first so output is bit-perfect.
    ///
    /// The engine is always configured with the source's own rate and channel
    /// count — the format the decode thread writes into the ring buffer. A
    /// device that cannot take the requested rate (MPEG-2/2.5 MP3 rates are
    /// commonly refused) resamples instead of playing at the wrong speed.
    #[allow(clippy::type_complexity)]
    fn create_engine_for(
        &mut self,
        info: &buffer::StreamInfo,
        consumer: rtrb::Consumer<f32>,
    ) -> Result<(Box<dyn AudioEngineHandle>, Option<Box<dyn SampleRateWatch>>), PlayerError> {
        let device = self.resolve_device()?;
        let device_rate = self.backend.get_device_sample_rate(&device)?;
        // The setup the decode thread was just handed, not a fresh read: a
        // config edited in between would put the engine at another rate.
        let dsp = match &self.dsp {
            Some(cache) if cache.device == device.name => cache.setup.clone(),
            _ => self.dsp_for(&device.name),
        };
        // The rate the decode thread writes at: the source's, unless DSP
        // resamples it to reach an impulse response.
        let source_rate =
            dsp.as_ref()
                .map_or(info.sample_rate, |d| d.output_rate(info.sample_rate)) as f64;

        // Anything read between here and the switch landing would pair this
        // track with the last one's output rate, and a rate switch is not
        // instant. Say nothing instead.
        self.shared_state.clear_output_sample_rate();

        let settled = if (device_rate - source_rate).abs() > 0.1 {
            log::info!(
                "switching device sample rate: {}Hz → {}Hz",
                device_rate,
                source_rate
            );
            match self.backend.set_device_sample_rate(&device, source_rate) {
                Ok(rate) => rate,
                Err(e) => {
                    log::warn!("failed to set device sample rate: {}", e);
                    device_rate
                }
            }
        } else {
            device_rate
        };

        if (settled - source_rate).abs() > 0.1 {
            log::warn!(
                "device stayed at {}Hz (wanted {}Hz) — output is resampled, not bit-perfect",
                settled,
                source_rate
            );
        }

        // The front ends compare this against the source rate to say whether
        // anything had to resample.
        self.shared_state
            .set_output_sample_rate(settled.round() as u32);

        // koan is not the only client of this device. Subscribe so the front
        // ends learn about a rate someone else moved instead of trusting the
        // reading above until the next track happens to build an engine.
        let watch_state = self.shared_state.clone();
        let watch_name = device.name.clone();
        let rate_watch = self.backend.watch_device_sample_rate(
            &device,
            Box::new(move |rate| {
                log::info!("device sample rate changed externally: {rate}Hz on '{watch_name}'");
                watch_state.set_output_sample_rate(rate.round() as u32);
            }),
        );

        let engine = self.backend.create_engine(
            &device,
            source_rate,
            info.channels as u32,
            consumer,
            self.timeline.samples_played_counter(),
        )?;
        // iOS answers 0 until its session has first been activated, and no
        // switch from an unknown rate is one to wait out. A new engine inside the
        // silence, a seek, still has the rest of the relock ahead of it.
        let now = std::time::Instant::now();
        let lead_in = if device_rate > 0.0 && (settled - device_rate).abs() > 0.1 {
            std::time::Duration::from_millis(
                crate::config::Config::cached()
                    .playback
                    .rate_switch_lead_in_ms as u64,
            )
        } else {
            self.lead_in_ends
                .map(|end| end.saturating_duration_since(now))
                .unwrap_or_default()
        };
        self.lead_in_ends = None;
        if !lead_in.is_zero() {
            engine.lead_in((settled * lead_in.as_secs_f64()) as u64);
            self.lead_in_ends = Some(now + lead_in);
        }

        Ok((engine, rate_watch))
    }

    /// The DSP profile for `device`, loaded once per config and device.
    fn dsp_for(&mut self, device: &str) -> Option<Arc<crate::audio::dsp::Setup>> {
        let config = crate::config::Config::cached();
        #[cfg(test)]
        if let Some(setup) = if self
            .renderer
            .as_ref()
            .is_some_and(|l| l.device_name() == device)
        {
            self.renderer_dsp_override.clone()
        } else {
            self.dsp_override.clone()
        } {
            self.dsp = Some(DspCache {
                config,
                device: device.to_string(),
                setup: Some(setup.clone()),
            });
            return Some(setup);
        }
        if let Some(cache) = &self.dsp
            && Arc::ptr_eq(&cache.config, &config)
            && cache.device == device
        {
            return cache.setup.clone();
        }
        let chain = crate::audio::dsp::output_chain(&config.dsp, device);
        let setup = chain.and_then(|chain| {
            crate::audio::dsp::Setup::load(&chain.profile, &chain.all, &crate::config::config_dir())
                .inspect_err(|e| {
                    log::error!(
                        "dsp: profile '{}' not loaded, playing without it: {e}",
                        chain.name
                    )
                })
                .ok()
                .flatten()
                .map(|mut setup| {
                    setup.name = chain.name;
                    Arc::new(setup)
                })
        });
        self.dsp = Some(DspCache {
            config,
            device: device.to_string(),
            setup: setup.clone(),
        });
        setup
    }

    /// Load the profiles again, and restart where playback is if what the
    /// output in use plays through has changed. The setups are compared as
    /// loaded, responses included, since the files can change without the
    /// config doing so. A preset given to another device, from the Play on
    /// menu, changes nothing here and restarts nothing.
    fn reload_dsp(&mut self) {
        let device = match &self.renderer {
            Some(link) => Ok(link.device_name().to_string()),
            None => self.resolve_device().map(|d| d.name),
        };
        let Ok(device) = device else {
            self.dsp = None;
            return;
        };
        let was = self
            .dsp
            .take()
            .filter(|c| c.device == device)
            .map(|c| c.setup);
        let now = self.dsp_for(&device);
        let changed = match was {
            Some(was) => was != now,
            None => now.is_some(),
        };
        if changed {
            self.restart_for_dsp();
        }
    }

    /// Restart where playback is, for a DSP change. Playing here, the old
    /// processing fades out quickly first and the new one fades in, so the
    /// change is a dip of a few milliseconds rather than a cut mid-waveform;
    /// `update_playback_state` restarts once the fade is silent. A change made
    /// while one is fading joins it. The restart opens at the same rate unless
    /// the new setup plays at another, which only a convolution can ask.
    fn restart_for_dsp(&mut self) {
        if self.dsp_restart.is_some() {
            return;
        }
        if let Some(session) = self.session()
            && session.run == Run::Playing
            && let Output::Local(local) = &session.output
            && local.engine.is_running()
        {
            local.engine.fade_out_quickly();
            self.dsp_restart = Some(std::time::Instant::now());
            return;
        }
        self.restart_on_current_track();
    }

    /// ReplayGain and DSP for a session on the output device.
    fn processing(&mut self) -> buffer::Processing {
        let cfg = crate::config::Config::cached();
        let dsp = match self.resolve_device() {
            Ok(device) => self.dsp_for(&device.name),
            Err(_) => None,
        };
        buffer::Processing {
            rg_mode: cfg.playback.replaygain,
            pre_amp_db: cfg.playback.pre_amp_db,
            dsp,
        }
    }

    /// Resolve the output device: use configured device name if set,
    /// falling back to system default if not set or if the named device is unavailable.
    fn resolve_device(&self) -> Result<backend::DeviceInfo, PlayerError> {
        if let Some(ref name) = self.output_device_name {
            match self.backend.list_devices() {
                Ok(devices) => {
                    if let Some(dev) = devices.into_iter().find(|d| d.name == *name) {
                        return Ok(dev);
                    }
                    log::warn!(
                        "configured output device '{}' not found, falling back to default",
                        name,
                    );
                }
                Err(e) => {
                    log::warn!("failed to list devices while resolving '{}': {}", name, e);
                }
            }
        }
        Ok(self.backend.default_device()?)
    }

    /// Switch the output device. Persists to config and restarts the engine
    /// on the current track if playing.
    pub fn set_output_device(&mut self, name: String) {
        log::info!("switching output device to: {}", name);
        self.output_device_name = Some(name.clone());

        if let Err(e) = crate::config::Config::persist(|cfg| {
            cfg.playback.output_device = Some(name);
            cfg.playback.renderer = None;
            cfg.playback.renderer_name = None;
        }) {
            log::error!("failed to save output device config: {}", e);
        }

        if self.renderer.is_some() {
            self.use_renderer(None);
        } else {
            self.restart_on_current_track();
        }
    }

    /// Clear the configured output device, reverting to system default.
    pub fn clear_output_device(&mut self) {
        log::info!("reverting to system default output device");
        self.output_device_name = None;

        if let Err(e) = crate::config::Config::persist(|cfg| {
            cfg.playback.output_device = None;
            cfg.playback.renderer = None;
            cfg.playback.renderer_name = None;
        }) {
            log::error!("failed to save output device config: {}", e);
        }

        if self.renderer.is_some() {
            self.use_renderer(None);
        } else {
            self.restart_on_current_track();
        }
    }

    /// If a track is currently playing or paused, restart playback at the
    /// current position (e.g. after switching output devices). Preserves pause state.
    fn restart_on_current_track(&mut self) {
        let position_ms = self.shared_state.position_ms();
        if let Err(e) = self.restart_current(position_ms) {
            log::error!("failed to restart playback on device switch: {}", e);
        }
    }

    /// Get the current output device name (if configured).
    pub fn output_device_name(&self) -> Option<&str> {
        self.output_device_name.as_deref()
    }

    /// Get a command sender for the UI layer.
    pub fn command_sender(&self) -> crossbeam_channel::Sender<PlayerCommand> {
        self.commands.tx.clone()
    }

    fn session(&self) -> Option<&Session> {
        match &self.transport {
            Transport::Loaded(session) => Some(session),
            _ => None,
        }
    }

    fn waiting(&self) -> Option<Waiting> {
        match self.transport {
            Transport::Waiting(waiting) => Some(waiting),
            _ => None,
        }
    }

    /// Whether the track waited for may open from its download as it lands:
    /// at the start, and not to a renderer, which is handed whole files only.
    fn may_stream(&self, waiting: &Waiting, id: QueueItemId) -> bool {
        waiting.may_stream(id) && self.renderer.is_none()
    }

    fn forget_waiting(&mut self) {
        if matches!(self.transport, Transport::Waiting(_)) {
            self.transport = Transport::Idle;
        }
    }

    /// Tell the front ends what the player is doing, after each command and
    /// each tick. Derived wholly from the transport: nothing else writes the
    /// playback state, the wait or the track.
    fn publish(&self) {
        let state = &self.shared_state;
        let (playback, waiting, track) = match &self.transport {
            Transport::Idle => (PlaybackState::Stopped, false, None),
            Transport::Waiting(waiting) => (
                match waiting.start {
                    Run::Playing => PlaybackState::Stopped,
                    Run::Paused => PlaybackState::Paused,
                },
                true,
                None,
            ),
            Transport::Loaded(session) => (
                match session.run {
                    Run::Playing => PlaybackState::Playing,
                    Run::Paused => PlaybackState::Paused,
                },
                false,
                Some(&session.track),
            ),
        };
        if state.track_info().as_ref() != track {
            state.set_track_info(track.cloned());
        }
        state.set_transport(playback, waiting);
        // Only a session decoded here can have been processed: a renderer
        // handed the original file plays it as it is.
        let dsp = match &self.transport {
            Transport::Loaded(Session {
                output: Output::Local(local),
                ..
            }) => local.dsp.clone(),
            Transport::Loaded(Session {
                output: Output::Renderer(play),
                ..
            }) => play.dsp(),
            _ => None,
        };
        if state.dsp() != dsp {
            state.set_dsp(dsp);
        }
        state.set_play_mode(self.mode);
        state.set_sleep(self.sleep.map(|s| s.sleep));
        state.set_sleep_fading(self.sleep_fading());
    }

    /// What the listener last asked for: to hear something, to have it
    /// paused, or nothing at all. An implicit move — the playing track
    /// removed, a download failed, an undo, the end of the queue's decoding —
    /// carries it over to the track it moves to.
    fn intent(&self) -> Option<Run> {
        match &self.transport {
            Transport::Idle => None,
            Transport::Waiting(waiting) => Some(waiting.start),
            Transport::Loaded(session) => Some(session.run),
        }
    }

    /// Move to `to` as the listener's last request would have it: playing
    /// on, paused, or, with nothing asked for, the cursor alone.
    fn carry_on(&mut self, to: Option<QueueItemId>, intent: Option<Run>) {
        match (to, intent) {
            (Some(id), Some(start)) => self.cue(id, 0, start),
            (Some(id), None) => {
                self.stop_playback_and_clear_state();
                self.shared_state.set_cursor(Some(id));
            }
            (None, _) => self.stop_playback_and_clear_state(),
        }
    }

    /// Play a specific item in the playlist by ID.
    /// Sets cursor, starts playback if Ready or streaming-ready, otherwise waits for TrackReady.
    pub fn play(&mut self, id: QueueItemId) {
        // Asked for by name, so a new play even of the item playing: from the
        // top, not a seek within what was heard.
        if self.in_flight.as_ref().is_some_and(|f| f.item == id) {
            self.finish_play();
        }
        self.forget_waiting();
        self.shared_state.set_cursor(Some(id));

        match self.shared_state.item_playback_source(id) {
            Some(PlaybackSource::Ready(path)) => {
                if let Err(e) = self.start_playback(id, &path, 0, Run::Playing) {
                    log::error!("play failed: {}", e);
                }
            }
            Some(PlaybackSource::Streaming {
                path,
                bytes_written,
                total,
            }) => {
                // Stop what is playing and park here. The probe answers on its
                // own thread; if it cannot, TrackReady starts the track once
                // the whole file has landed. A renderer takes whole files
                // only, so for one there is no probe: TrackReady it is.
                self.park(id, 0, Run::Playing);
                if self.renderer.is_none() {
                    self.probe_stream_for_playback(id, &path, bytes_written, total);
                }
            }
            None => {
                self.park(id, 0, Run::Playing);
                log::info!("play: item {:?} not ready, waiting for TrackReady", id);
            }
        }
    }

    /// Load a track at `position_ms`, playing or paused — where a restored
    /// session or a hand-off picks up.
    ///
    /// A track still downloading waits until it can open — see `Waiting`.
    fn cue(&mut self, id: QueueItemId, position_ms: u64, start: Run) {
        self.forget_waiting();
        self.shared_state.set_cursor(Some(id));
        let Some(PlaybackSource::Ready(path)) = self.shared_state.item_playback_source(id) else {
            if position_ms == 0 && start == Run::Playing {
                self.play(id);
                return;
            }
            self.park(id, position_ms, start);
            log::info!("cue: {id:?} not on disk yet, opening at {position_ms}ms once it is");
            return;
        };
        if let Err(e) = self.start_playback(id, &path, position_ms, start) {
            log::error!("cue failed: {}", e);
            return;
        }
        self.report(match start {
            Run::Playing => PlaybackReportState::Playing,
            Run::Paused => PlaybackReportState::Paused,
        });
    }

    /// Stop what is playing and wait for `id` to become playable. A wait that
    /// is to open paused already reads as paused, so a client offers to play
    /// rather than showing a track on its way.
    fn park(&mut self, id: QueueItemId, position_ms: u64, start: Run) {
        self.report(PlaybackReportState::Stopped);
        self.finish_play();
        self.stop_engine();
        self.timeline.reset();
        self.shared_state.set_position_ms(position_ms);
        self.transport = Transport::Waiting(Waiting {
            id,
            position_ms,
            start,
        });
    }

    fn start_playback(
        &mut self,
        id: QueueItemId,
        path: &Path,
        seek_ms: u64,
        start: Run,
    ) -> Result<(), PlayerError> {
        self.open_session(id, Source::File(path.to_path_buf()), None, seek_ms, start)
    }

    /// Open a session on `id` at `seek_ms`, playing or paused.
    ///
    /// `info` is what is already known of the stream: from the off-thread
    /// probe for a download, or from what is playing for a restart. A file
    /// opened without it is probed here. A failure leaves the player cleanly
    /// stopped; displaying a track that no engine is playing freezes the
    /// position and makes the transport lie.
    fn open_session(
        &mut self,
        id: QueueItemId,
        source: Source,
        info: Option<buffer::StreamInfo>,
        seek_ms: u64,
        start: Run,
    ) -> Result<(), PlayerError> {
        #[cfg(test)]
        {
            self.playback_starts += 1;
        }
        self.forget_waiting();
        let mut result = self.try_open_session(id, source, info, seek_ms, start);
        // A track the renderer cannot play is marked so in the queue and
        // walked past, opening the next the way this one would have opened.
        // A loop rather than a call back through `play`: a long run of them,
        // a DSD library on a renderer without DSD, would grow the stack per
        // track.
        let mut skipped = id;
        while matches!(result, Err(PlayerError::Unplayable(_)))
            && self.shared_state.is_cursor(skipped)
        {
            let Some(next) = self.shared_state.advance_cursor_loadable() else {
                log::info!("upnp: nothing further the renderer can play");
                self.stop_playback_and_clear_state();
                return Ok(());
            };
            match self.shared_state.item_playback_source(next) {
                Some(PlaybackSource::Ready(path)) => {
                    skipped = next;
                    result = self.try_open_session(next, Source::File(path), None, 0, start);
                }
                // Not on disk yet: waited for, like any track not yet loaded.
                _ => {
                    self.cue(next, 0, start);
                    return Ok(());
                }
            }
        }
        if result.is_err() {
            self.stop_playback_and_clear_state();
        }
        self.wake_analyzer();
        result
    }

    fn try_open_session(
        &mut self,
        id: QueueItemId,
        source: Source,
        info: Option<buffer::StreamInfo>,
        seek_ms: u64,
        start: Run,
    ) -> Result<(), PlayerError> {
        if self.renderer.is_some() {
            return self.try_open_on_renderer(id, source, info, seek_ms, start);
        }
        self.stop_engine();
        let mut info = match info {
            Some(info) => info,
            None => buffer::probe_file(source.path())?,
        };
        // A stream opened before its container says how long it is (an Ogg
        // whose last page has not arrived) runs on the library's duration.
        if info.duration_ms == 0
            && let Some(known) = self.shared_state.get_item(id).and_then(|i| i.duration_ms)
        {
            info.duration_ms = known;
        }
        let path = source.path().to_path_buf();
        let streaming = matches!(source, Source::Stream(_));

        let (first, stream) = match source {
            Source::File(path) => (buffer::SourceEntry::from_file(id, path), None),
            Source::Stream(source) => {
                // Held so a seek can reopen the same way without probing again.
                self.stream_mode = source.mode;
                let live = LiveStream {
                    feed: source.bytes_written.clone(),
                    abandoned: Default::default(),
                };
                let status = {
                    let downloading = self.stream_status_fn(id);
                    let abandoned = live.abandoned.clone();
                    Arc::new(move || {
                        if abandoned.load(std::sync::atomic::Ordering::Acquire) {
                            streaming::StreamStatus::Failed
                        } else {
                            downloading()
                        }
                    }) as Arc<dyn Fn() -> streaming::StreamStatus + Send + Sync>
                };
                let entry = buffer::SourceEntry {
                    id,
                    path: source.path.clone(),
                    hint: hint_for(&source.path),
                    make_mss: Box::new(move || {
                        let partial = streaming::PartialFileSource::open(
                            &source.path,
                            source.bytes_written.clone(),
                            source.total,
                            status.clone(),
                            source.mode,
                        )?;
                        Ok(symphonia::core::io::MediaSourceStream::new(
                            Box::new(partial),
                            Default::default(),
                        ))
                    }),
                };
                (entry, Some(live))
            }
        };

        let track = TrackInfo {
            id,
            path: path.clone(),
            codec: info.codec.clone(),
            sample_rate: info.sample_rate,
            bit_depth: info.bit_depth,
            bitrate_kbps: info.bitrate_kbps,
            channels: info.channels,
            duration_ms: info.duration_ms,
        };
        // For seeks, this keeps the bar at the target position instead of
        // flashing to 0 while the new timeline spins up.
        self.shared_state.set_position_ms(seek_ms);
        self.on_track_changed(id, seek_ms);
        log::info!(
            "{}: {} ({:?}) — {} {}Hz/{}ch, {}ms{}",
            if streaming { "streaming" } else { "playing" },
            path.display(),
            id,
            info.codec,
            info.sample_rate,
            info.channels,
            info.duration_ms,
            if seek_ms > 0 {
                format!(" @{}ms", seek_ms)
            } else {
                String::new()
            }
        );

        let (producer, consumer) = rtrb::RingBuffer::new(RING_BUFFER_SIZE);
        self.timeline.reset();
        let lookahead = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let next_track = self.decode_cursor(id, lookahead.clone());
        // Files and downloads in progress open here alike, so both get the
        // output's processing.
        let processing = self.processing();
        self.session += 1;
        let session = self.session;
        let finish_tx = self.commands.tx.clone();
        let decode_handle = buffer::start_decode(
            first,
            producer,
            seek_ms,
            move || {
                let (next_id, next_path) = next_track()?;
                Some(buffer::SourceEntry::from_file(next_id, next_path))
            },
            self.timeline.clone(),
            Some(self.viz_buffer.clone()),
            processing,
            move |stop| {
                commands::send_unless_stopped(
                    &finish_tx,
                    PlayerCommand::DecodeFinished(session),
                    stop,
                );
            },
        )?;

        let (engine, rate_watch) = self.create_engine_for(&info, consumer)?;
        // The setup `create_engine_for` chose the rate by.
        let dsp = self
            .dsp
            .as_ref()
            .and_then(|c| c.setup.as_ref())
            .map(|s| s.status(info.sample_rate));
        // A session opened paused leaves the unit stopped: starting it and
        // stopping it again lets a moment of the track out.
        if start == Run::Playing {
            if std::mem::take(&mut self.quick_start) {
                engine.fade_in_quickly()?;
            } else {
                engine.start()?;
            }
        }
        self.transport = Transport::Loaded(Session {
            track,
            run: start,
            lookahead,
            output: Output::Local(Local {
                engine,
                decode_handle,
                stream,
                _rate_watch: rate_watch,
                dsp,
            }),
        });
        Ok(())
    }

    /// Gapless lookahead: the decode thread keeps its own cursor, separate
    /// from the UI cursor, so it can look ahead through the playlist without
    /// moving what the UI shows as now playing.
    ///
    /// Each step is kept in `steps`, so a queue edit is checked against what
    /// the decoder decided rather than against what it would decide now —
    /// which differs once a download has landed or a file failed to open. The
    /// lock is held across the read and the record: an edit either lands
    /// before the read or finds the step recorded.
    fn decode_cursor(
        &self,
        id: QueueItemId,
        steps: Arc<parking_lot::Mutex<Vec<state::Lookahead>>>,
    ) -> impl Fn() -> Option<(QueueItemId, PathBuf)> + Send + 'static {
        let state = self.shared_state.clone();
        let timeline = self.timeline.clone();
        move || {
            let mut steps = steps.lock();
            let current = match steps.last() {
                None => id,
                Some(step) => step.chosen.as_ref()?.0,
            };
            let mut step = state.lookahead_after(current)?;
            step.boundary = timeline.boundary_count();
            let next = step.chosen.clone();
            steps.push(step);
            next
        }
    }

    /// Probe a partially-downloaded file on its own thread, and start it when
    /// the answer comes back.
    ///
    /// Nothing here waits. Probing reads as much of the container as it takes
    /// to describe itself — for Ogg, its last page, which means the whole
    /// remaining download — and this is the thread that answers play, pause and
    /// seek. So the probe goes elsewhere and its result returns as a command.
    ///
    /// A format that describes itself up front (FLAC, MP3) comes back in
    /// milliseconds and starts early, which is the point of streaming. One that
    /// does not comes back whenever it comes back, by which time the download
    /// has usually landed and `TrackReady` has started the track from disk —
    /// and the late answer is simply dropped. Either way the player kept
    /// answering commands throughout.
    fn probe_stream_for_playback(
        &self,
        id: QueueItemId,
        path: &Path,
        bytes_written: Arc<crate::remote::downloads::ByteFeed>,
        total: u64,
    ) {
        let path = path.to_path_buf();
        let tx = self.commands.tx.clone();
        let hint = hint_for(&path);

        // Abandon the moment the track stops being the one wanted. A probe of
        // a container that needs its tail otherwise reads to the end of a
        // download nobody is waiting for any more, and skipping through a
        // queue that is still caching would leave one doing so per skip.
        let status = {
            let downloading = self.stream_status_fn(id);
            let state = self.shared_state.clone();
            Arc::new(move || {
                if state.is_cursor(id) {
                    downloading()
                } else {
                    streaming::StreamStatus::Failed
                }
            }) as Arc<dyn Fn() -> streaming::StreamStatus + Send + Sync>
        };

        let spawned = thread::Builder::new()
            .name("koan-stream-probe".into())
            .spawn(move || {
                // `wait` says whether a read may sit at the write head for
                // more of the download. The first attempt must not: a
                // container that goes looking for its tail would wait for the
                // whole transfer, and failing at once is how that is detected.
                // The second has no length to go looking with, so whatever it
                // still wants is in front of it and worth waiting for.
                let attempt = |mode, wait: bool| {
                    let open = if wait {
                        streaming::PartialFileSource::open(
                            &path,
                            bytes_written.clone(),
                            total,
                            status.clone(),
                            mode,
                        )
                    } else {
                        streaming::PartialFileSource::open_for_probe(
                            &path,
                            bytes_written.clone(),
                            total,
                            status.clone(),
                            mode,
                        )
                    };
                    open.map_err(buffer::DecodeError::Io).and_then(|source| {
                        let mss = symphonia::core::io::MediaSourceStream::new(
                            Box::new(source),
                            Default::default(),
                        );
                        buffer::probe_source(mss, &hint)
                    })
                };

                // Ask for the whole description first. Neither attempt waits at
                // the write head, so a container that needs bytes which have
                // not arrived fails here rather than reading the transfer out.
                let info = match attempt(streaming::ProbeMode::Full, false) {
                    Ok(info) => Some((info, streaming::ProbeMode::Full)),
                    Err(e) => {
                        // Try again claiming no length. Ogg goes looking for its
                        // last page only when told there is one to find; without
                        // it the track opens now and plays, at the price of
                        // seeking and of the duration that page carries. Both
                        // come back when the download lands.
                        log::info!(
                            "stream probe: {} needs more than has arrived ({}), opening without a length",
                            path.display(),
                            e
                        );
                        let lengthless = lengthless_mode_for(&path);
                        attempt(lengthless, true)
                            .ok()
                            .map(|info| (info, lengthless))
                    }
                };

                match info {
                    Some((info, mode)) => {
                        tx.send(PlayerCommand::StreamProbed {
                            id,
                            info: Box::new(info),
                            mode,
                        })
                        .ok();
                    }
                    // Not a failure of the track: it plays from disk once the
                    // download lands, and the cursor is still parked on it.
                    None => log::info!(
                        "stream probe: {} cannot start early, waiting for the download",
                        path.display()
                    ),
                }
            });

        if let Err(e) = spawned {
            log::warn!("stream probe: could not spawn for {:?}: {}", id, e);
        }
    }

    /// A probe finished. Start the track if it is still the one wanted and
    /// nothing has started it in the meantime.
    fn stream_probed(
        &mut self,
        id: QueueItemId,
        info: buffer::StreamInfo,
        mode: streaming::ProbeMode,
    ) {
        // Moved on, or already open — the download landed first, or the user
        // asked for something else.
        let Some(waiting) = self.waiting().filter(|w| self.may_stream(w, id)) else {
            return;
        };
        if !self.shared_state.is_cursor(id) {
            return;
        }

        match self.shared_state.item_playback_source(id) {
            // The download landed while probing: play it as an ordinary file.
            Some(PlaybackSource::Ready(path)) => {
                if let Err(e) = self.start_playback(id, &path, 0, waiting.start) {
                    log::error!("stream probe: playback failed: {}", e);
                }
            }
            Some(PlaybackSource::Streaming {
                path,
                bytes_written,
                total,
            }) => {
                let source = StreamSource {
                    path,
                    bytes_written,
                    total,
                    mode,
                };
                if let Err(e) =
                    self.open_session(id, Source::Stream(source), Some(info), 0, waiting.start)
                {
                    log::error!("stream probe: streaming playback failed: {}", e);
                }
            }
            None => {}
        }
    }

    /// What the streaming source asks per read to know whether the download is
    /// still going. Asked each time rather than passed once: a transfer can
    /// land, or die, at any point during playback.
    fn stream_status_fn(
        &self,
        id: QueueItemId,
    ) -> Arc<dyn Fn() -> streaming::StreamStatus + Send + Sync> {
        // The item's own state, which a transfer's end writes before it wakes
        // anyone: asked twice per read of a file still arriving, so nothing
        // else is looked up.
        let state = self.shared_state.clone();
        Arc::new(move || match state.item_state(id) {
            Some(ItemState::Ready) => streaming::StreamStatus::Complete,
            Some(ItemState::Failed(_)) => streaming::StreamStatus::Failed,
            _ => streaming::StreamStatus::Downloading,
        })
    }

    /// Seek within the current track, preserving pause state.
    ///
    /// A track still downloading is seekable only as far as its bytes reach, so
    /// the target is clamped to `seekable_ms` and the restart goes back through
    /// the streaming path — reopening a partial file as a plain file would
    /// decode whatever happens to be on disk and end the track early.
    pub fn seek(&mut self, position_ms: u64) {
        let Some(id) = self.session().map(|s| s.track.id) else {
            return;
        };
        // Stop just short of the end rather than falling into the next track.
        let seekable = self.shared_state.seekable_ms();
        if seekable == 0 {
            // Nothing of this track can be reached yet — a partial container
            // that has not said what it is. Restarting it at zero is not what
            // anyone asked for, so the seek is simply declined.
            log::debug!("seek declined: {:?} is not seekable yet", id);
            return;
        }
        let ceiling = seekable.min(
            self.shared_state
                .duration_ms()
                .saturating_sub(SEEK_END_GUARD_MS),
        );
        let clamped = position_ms.min(ceiling);

        // A renderer seeks the track it holds; reloading it would let the
        // top of the track be heard.
        if self.seek_on_renderer(clamped) {
            return;
        }
        if let Err(e) = self.restart_current(clamped) {
            log::error!("seek failed: {}", e);
        }
    }

    /// Restart what is playing at `position_ms`, preserving pause state.
    ///
    /// What a seek does, and what switching output device does, and what going
    /// back from the first track does. All three restart the same track, so all
    /// three resolve the source the same way: from the queue item, never from
    /// the session's path, which names the `.part` file for a track that was still
    /// downloading when it started and is not renamed when the download lands.
    fn restart_current(&mut self, position_ms: u64) -> Result<(), PlayerError> {
        let Some(session) = self.session() else {
            return Ok(());
        };
        let (info, start) = (session.track.clone(), session.run);

        match self.shared_state.item_playback_source(info.id) {
            // A renderer takes whole files only: wait for this one to land,
            // keeping its place and whether it was playing.
            Some(PlaybackSource::Streaming { .. }) if self.renderer.is_some() => {
                self.park(info.id, position_ms, start);
                return Ok(());
            }
            Some(PlaybackSource::Streaming {
                path,
                bytes_written,
                total,
            }) => {
                // No probe: what is playing already said what this is, and
                // reading an Ogg's last page to learn it again would mean
                // waiting for the rest of the download.
                let known = buffer::StreamInfo {
                    codec: info.codec.clone(),
                    sample_rate: info.sample_rate,
                    channels: info.channels,
                    bit_depth: info.bit_depth,
                    bitrate_kbps: info.bitrate_kbps,
                    duration_ms: info.duration_ms,
                };
                let source = StreamSource {
                    path,
                    bytes_written,
                    total,
                    mode: self.stream_mode,
                };
                self.open_session(
                    info.id,
                    Source::Stream(source),
                    Some(known),
                    position_ms,
                    start,
                )?;
            }
            Some(PlaybackSource::Ready(path)) => {
                self.start_playback(info.id, &path, position_ms, start)?;
            }
            None => return Ok(()),
        }

        self.report(match start {
            Run::Playing => PlaybackReportState::Playing,
            Run::Paused => PlaybackReportState::Paused,
        });
        Ok(())
    }

    /// Skip to next track in playlist.
    pub fn next_track(&mut self) {
        match self.shared_state.advance_cursor_loadable() {
            Some(id) => self.play(id),
            None => {
                log::info!("no more tracks in playlist");
                self.stop_playback_and_clear_state();
            }
        }
    }

    /// Go back to previous track.
    pub fn prev_track(&mut self) {
        match self.shared_state.retreat_cursor() {
            Some((id, _)) => self.play(id),
            None => {
                // No previous track — restart current from the beginning.
                if let Err(e) = self.restart_current(0) {
                    log::error!("restart failed: {}", e);
                }
            }
        }
    }

    /// Pause playback, fading out if the config asks for it.
    ///
    /// A fade leaves the unit running until it reaches silence;
    /// `update_playback_state` stops it from there. A track still on its way
    /// opens paused when it arrives.
    pub fn pause(&mut self) {
        let fade = crate::config::Config::cached().playback.fade_on_pause;
        self.pause_with(if fade { Fade::Short } else { Fade::Cut });
    }

    fn pause_with(&mut self, fade: Fade) {
        match &mut self.transport {
            Transport::Idle => return,
            Transport::Waiting(waiting) => {
                waiting.start = Run::Paused;
                return;
            }
            Transport::Loaded(session) => {
                if let Output::Local(local) = &session.output {
                    match fade {
                        Fade::Short => local.engine.fade_out(),
                        Fade::Cut => {
                            if let Err(e) = local.engine.stop() {
                                log::error!("pause failed: {}", e);
                                return;
                            }
                        }
                    }
                }
                session.run = Run::Paused;
            }
        }
        self.pause_renderer();
        self.report(PlaybackReportState::Paused);
    }

    /// Resume playback. Fades back in if the pause faded out.
    ///
    /// With nothing loaded — a session restored stopped, or a start that
    /// failed — there is nothing to resume, and play starts the track under
    /// the cursor instead of doing nothing. A track still on its way keeps the
    /// position it was waiting to open at.
    pub fn resume(&mut self) {
        self.answer_silence();
        if std::mem::take(&mut self.sleep_snap_on_resume) {
            self.set_sleep_gain(1.0, true);
        }
        let session = match &mut self.transport {
            Transport::Idle => {
                if let Some(id) = self.shared_state.cursor() {
                    self.play(id);
                }
                return;
            }
            Transport::Waiting(waiting) => {
                let waiting = *waiting;
                self.cue(waiting.id, waiting.position_ms, Run::Playing);
                return;
            }
            Transport::Loaded(session) => session,
        };
        let Output::Local(local) = &session.output else {
            self.resume_renderer();
            return;
        };
        let engine = &local.engine;
        let resumed = if engine.is_running() || engine.is_silent() {
            self.lead_in_ends = None;
            engine.fade_in()
        } else {
            engine.start()
        };
        if let Err(e) = resumed {
            log::error!("resume failed: {}", e);
            return;
        }
        session.run = Run::Playing;
        self.wake_analyzer();
        self.report(PlaybackReportState::Playing);
    }

    /// Tell the analyser there is about to be something to hear.
    ///
    /// It parks when nothing is playing and nothing is reading, and the one
    /// thing it cannot be signalled from is the play head — that counter is
    /// written by the audio render callback, which may never take a lock. So
    /// the player says so instead, on the two edges where silence ends.
    fn wake_analyzer(&self) {
        self.viz_snapshot.wake();
    }

    /// Stop playback and clear playlist.
    pub fn stop(&mut self) {
        self.shared_state.clear_playlist();
        self.stop_playback_and_clear_state();
    }

    /// Stop the audio engine and decode thread without touching shared state.
    ///
    /// Output stops first, then the decode thread is joined, then the engine
    /// drops: tearing CoreAudio down under a live producer is the end-of-queue
    /// crash (#89).
    fn stop_engine(&mut self) {
        let playback = match std::mem::replace(&mut self.transport, Transport::Idle) {
            Transport::Loaded(playback) => playback,
            other => {
                self.transport = other;
                return;
            }
        };
        self.session += 1;
        // A session ending some other way ends the DSP change's wait with it.
        self.dsp_restart = None;
        self.bank_listening();
        let local = match playback.output {
            Output::Local(local) => local,
            Output::Renderer(play) => {
                self.halt_renderer(*play);
                self.answer_silence();
                return;
            }
        };
        let Local {
            engine,
            mut decode_handle,
            stream,
            ..
        } = local;

        let _ = engine.stop();
        // Stop first, so the failed read the abandon causes reads as a stop
        // rather than a bad source to skip past.
        decode_handle.signal_stop();
        if let Some(stream) = stream {
            stream.abandon();
        }
        decode_handle.stop();
        drop(engine);
        self.answer_silence();
    }

    /// Tell whoever waited for the pause where the playhead came to rest.
    fn answer_silence(&mut self) {
        let position_ms = self.shared_state.position_ms();
        for reply in self.silence_waiters.drain(..) {
            let _ = reply.send(position_ms);
        }
    }

    /// Full stop: tear down engine + clear all display state.
    fn stop_playback_and_clear_state(&mut self) {
        self.forget_waiting();
        self.report(PlaybackReportState::Stopped);
        self.finish_play();
        self.stop_engine();
        self.timeline.reset();
        self.shared_state.set_position_ms(0);
    }

    /// Remove a track from the playlist. If it was the cursor, move to the
    /// track that followed it, as the listener's last request would have it.
    ///
    /// `remove_item` clears the cursor, and an unset cursor means "start from the
    /// top" — so the successor is pinned down by parking the cursor on the removed
    /// track's predecessor first. `None` is correct only when it was the first item.
    pub fn remove_from_playlist(&mut self, id: QueueItemId) {
        let was_cursor = self.shared_state.is_cursor(id);
        let resume_after = was_cursor
            .then(|| self.shared_state.item_before(id))
            .flatten();
        self.shared_state.remove_item(id);
        if was_cursor {
            self.shared_state.set_cursor(resume_after);
            let next = self.shared_state.advance_cursor_loadable();
            self.carry_on(next, self.intent());
        }
    }

    /// A download finished. A track waiting on it opens; one already
    /// streaming from it, playing or paused, re-reads its metadata from the
    /// complete file.
    ///
    /// The item's state is the downloader's to set, before it sends this.
    /// Setting it here as well would turn a duplicate entry's `Failed` back to
    /// `Ready`, pointing at a `.part` file that was deleted.
    pub fn track_ready(&mut self, id: QueueItemId) {
        if !self.shared_state.is_cursor(id) {
            return;
        }

        if let Some(waiting) = self.waiting().filter(|w| w.id == id) {
            log::info!("track_ready: opening {:?}", id);
            self.cue(id, waiting.position_ms, waiting.start);
            return;
        }

        if self.session().is_some_and(|s| s.track.id == id) {
            log::info!(
                "track_ready: download complete while streaming {:?}, refreshing metadata",
                id
            );
            self.refresh_track_metadata(id);
        }
    }

    /// Enough of a download has landed to stream it. Opens the track if it is
    /// waiting to start from the top.
    pub fn track_stream_ready(&mut self, id: QueueItemId) {
        let Some(waiting) = self.waiting().filter(|w| self.may_stream(w, id)) else {
            return;
        };
        if !self.shared_state.is_cursor(id) {
            return;
        }

        match self.shared_state.item_playback_source(id) {
            Some(PlaybackSource::Streaming {
                path,
                bytes_written,
                total,
            }) => {
                log::info!("track_stream_ready: probing partial file for {:?}", id);
                self.probe_stream_for_playback(id, &path, bytes_written, total);
            }
            Some(PlaybackSource::Ready(path)) => {
                // Download finished between threshold and now — just play normally.
                log::info!(
                    "track_stream_ready: track already ready, starting normal playback for {:?}",
                    id
                );
                if let Err(e) = self.start_playback(id, &path, 0, waiting.start) {
                    log::error!("track_stream_ready playback failed: {}", e);
                }
            }
            None => {} // Not enough data yet — wait.
        }
    }

    /// Re-read full lofty metadata for a track after its download completes.
    /// Called from track_ready() when a streaming track finishes downloading.
    /// What the item takes from it is `update_item_metadata`'s call.
    fn refresh_track_metadata(&mut self, id: QueueItemId) {
        use crate::index::metadata;

        let path = match self.shared_state.item_path_if_ready(id) {
            Some(p) => p,
            None => return,
        };

        match metadata::read_metadata(&path) {
            Ok(meta) => {
                self.shared_state.update_item_metadata(
                    id,
                    meta.title,
                    meta.artist,
                    meta.album_artist.unwrap_or_default(),
                    meta.album,
                    meta.duration_ms.map(|d| d as u64),
                );

                // Re-probe the complete file for accurate duration + stream info.
                // The initial probe was done on partial streaming data and may have
                // underestimated duration, causing premature seek clamping or wrong
                // progress bar display.
                //
                // The path is taken over at the same time. Playback started
                // against the `.part` file and the download's last act is to
                // rename it, so what `track_info` holds now names nothing.
                if let Transport::Loaded(session) = &mut self.transport
                    && session.track.id == id
                {
                    let current = &mut session.track;
                    let probed = buffer::probe_file(&path).ok();
                    let duration_ms = probed
                        .as_ref()
                        .map(|s| s.duration_ms)
                        .filter(|d| *d > current.duration_ms)
                        .unwrap_or(current.duration_ms);
                    if duration_ms != current.duration_ms {
                        log::info!(
                            "track_ready: duration corrected {}ms → {}ms",
                            current.duration_ms,
                            duration_ms
                        );
                    }
                    current.duration_ms = duration_ms;
                    current.path = path.clone();
                }

                // Signal UI to re-read cover art and update souvlaki media controls.
                self.shared_state.signal_metadata_refresh();
                log::info!("track_ready: metadata refreshed for {:?}", id);
            }
            Err(e) => {
                log::warn!("track_ready: metadata refresh failed for {:?}: {}", id, e);
            }
        }
    }

    /// A session has opened on `id`. A seek restarts playback of the same
    /// track, so the play in flight carries on when it is the same item —
    /// otherwise scrubbing around a track would enter it into history once
    /// per seek. Everything that ends a play and opens the same item again,
    /// as a new one, closes the old play first: `play`, a repeat at the end
    /// of a session.
    fn on_track_changed(&mut self, id: QueueItemId, position_ms: u64) {
        if let Some(f) = self.in_flight.as_mut().filter(|f| f.item == id) {
            f.jump(position_ms);
            // The session is new, so the play is its first boundary.
            f.boundary = 0;
            return;
        }
        self.begin_play(id, position_ms, 0);
    }

    /// The needle has moved to a new play of `id`, at `boundary` of the
    /// session. Close out the outgoing play and write the new one to history
    /// straight away, so history reads in play order even for a track that
    /// is skipped a moment later.
    fn begin_play(&mut self, id: QueueItemId, position_ms: u64, boundary: usize) {
        self.finish_play();
        let track_id = self.shared_state.item_db_id(id);
        let mut flight = InFlight::new(id, track_id, position_ms);
        flight.boundary = boundary;
        self.in_flight = Some(flight);
        if let (Some(track_id), Some(recorder)) = (track_id, self.history.as_ref()) {
            recorder.record(PlayEvent::Started {
                track_id,
                position_ms,
            });
        }
    }

    /// Count what has played of the track in flight since it was last
    /// counted. Before anything that resets the timeline, which is where the
    /// playhead is read from, and before a track is closed out.
    fn bank_listening(&mut self) {
        // A renderer's playhead is its clock: set only while one plays.
        let renderer_at = self
            .shared_state
            .renderer_clock()
            .map(|_| self.shared_state.position_ms());
        if let Some(f) = self.in_flight.as_mut()
            && let Some(at) = self.timeline.position_in(f.boundary).or(renderer_at)
        {
            f.advance(at);
        }
    }

    /// Tell the remote server where the track playing now stands. A track
    /// starting is reported by `on_track_changed`; this covers what happens
    /// to it afterwards.
    fn report(&self, state: PlaybackReportState) {
        let Some(track_id) = self.in_flight.as_ref().and_then(InFlight::track_id) else {
            return;
        };
        if let Some(recorder) = self.history.as_ref() {
            recorder.record(PlayEvent::Playback(PlaybackReport {
                track_id,
                state,
                position_ms: self.shared_state.position_ms(),
            }));
        }
    }

    /// Tell history how long the current track was heard for. Returns what was
    /// reported, which is how the tests see it.
    fn finish_play(&mut self) -> Option<PlayEvent> {
        self.bank_listening();
        let flight = self.in_flight.take()?;
        let event = PlayEvent::Finished {
            track_id: flight.track_id()?,
            listened_ms: flight.listened_ms(),
        };
        if let Some(recorder) = self.history.as_ref() {
            recorder.record(event);
        }
        Some(event)
    }

    /// What happens without a command: the silence after a rate switch
    /// running out, a pause's fade reaching silence, and the playhead crossing
    /// into the next queued track. Called from the command loop on each wake.
    pub fn update_playback_state(&mut self) {
        self.sleep_tick();
        if let Some(SleepSet { at: Some(at), .. }) = self.sleep
            && std::time::Instant::now() >= at
        {
            self.sleep = None;
            log::info!("sleep timer: time's up");
            // Faded to silence by now: what is cut is not heard.
            if self.intent() == Some(Run::Playing) {
                self.pause_with(Fade::Cut);
            }
            self.end_sleep_fade();
        }

        if self.session().is_some()
            && self
                .lead_in_ends
                .is_some_and(|end| std::time::Instant::now() >= end)
        {
            // The playhead held still through the silence while clients
            // counted on from where it was published; this is where they hear
            // it move.
            self.lead_in_ends = None;
            self.shared_state.changed();
        }

        if let Some(session) = self.session()
            && session.run == Run::Paused
            && let Some(engine) = session.engine()
            && engine.is_running()
            && engine.is_silent()
        {
            if let Err(e) = engine.stop() {
                log::error!("stopping after fade failed: {}", e);
            }
            self.answer_silence();
        }

        if let Some(since) = self.dsp_restart {
            let faded = self
                .session()
                .and_then(Session::engine)
                .is_none_or(|e| e.is_silent());
            if faded || since.elapsed() >= QUICK_FADE_LIMIT {
                self.dsp_restart = None;
                self.quick_start = true;
                self.restart_on_current_track();
                self.quick_start = false;
            }
        }

        self.release_idle_session();
        self.follow_playhead();
        self.renderer_tick();
        self.publish();
    }

    /// A gapless transition moves the playhead into the next track without
    /// anything on this thread asking, so the play is banked, the session's
    /// track moved on and the cursor brought along from here. A play is its
    /// boundary: an item repeated runs into itself, and that is a new play.
    ///
    /// A sleep timer for the end of the track or record stops playback the
    /// moment the next play is heard to start, gaplessly, so with no fade
    /// over a track only just begun.
    fn follow_playhead(&mut self) {
        let before = self.in_flight.as_ref().map(|f| (f.item, f.boundary));
        self.move_with_playhead();
        let after = self.in_flight.as_ref().map(|f| (f.item, f.boundary));
        if let (Some((ended, _)), Some((next, _))) = (before, after)
            && before != after
            && self.sleeps_between(Some(ended), Some(next))
        {
            self.pause_with(Fade::Cut);
            self.end_sleep_fade();
        }
    }

    fn move_with_playhead(&mut self) {
        if self.session().is_none() {
            return;
        }
        let Some(playhead) = self.timeline.playhead() else {
            return;
        };
        let id = playhead.id;
        if self
            .in_flight
            .as_ref()
            .is_none_or(|f| f.item != id || f.boundary != playhead.boundary)
        {
            self.begin_play(id, playhead.position_ms, playhead.boundary);
        }
        let Transport::Loaded(session) = &mut self.transport else {
            return;
        };
        if session.track.id == id {
            return;
        }
        let Some((id, path, info, _)) = self.timeline.current_playback() else {
            return;
        };
        log::info!("timeline: now playing {:?}", id);
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
        self.shared_state.set_cursor(Some(id));
    }

    /// Whether a sleep timer set for the end of the track or record ends
    /// playback where `ended` gives way to `next`, `None` at the end of the
    /// queue. Spent if so.
    fn sleeps_between(&mut self, ended: Option<QueueItemId>, next: Option<QueueItemId>) -> bool {
        let record = |id: Option<QueueItemId>| {
            id.and_then(|id| self.shared_state.get_item(id))
                .map(|i| (i.album, i.album_artist))
        };
        let due = match self.sleep.map(|s| s.sleep) {
            Some(Sleep::EndOfTrack) => true,
            Some(Sleep::EndOfRecord) => next.is_none() || record(ended) != record(next),
            _ => false,
        };
        if due {
            log::info!(
                "sleep timer: stopping at the end of the {}",
                match self.sleep {
                    Some(SleepSet {
                        sleep: Sleep::EndOfTrack,
                        ..
                    }) => "track",
                    _ => "record",
                }
            );
            self.sleep = None;
        }
        due
    }

    /// Set the sleep timer, or cancel it.
    fn set_sleep_timer(&mut self, timer: Option<SleepTimer>) {
        self.restore_sleep_fade();
        self.sleep = timer.map(|timer| match timer {
            SleepTimer::After { minutes } => {
                let after = std::time::Duration::from_secs(u64::from(minutes) * 60);
                let unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    + after;
                SleepSet {
                    sleep: Sleep::At {
                        unix_ms: unix_ms.as_millis() as u64,
                    },
                    at: Some(std::time::Instant::now() + after),
                    fade: Some(sleep::fade_length(minutes)),
                }
            }
            SleepTimer::EndOfTrack => SleepSet {
                sleep: Sleep::EndOfTrack,
                at: None,
                fade: None,
            },
            SleepTimer::EndOfRecord => SleepSet {
                sleep: Sleep::EndOfRecord,
                at: None,
                fade: None,
            },
        });
        log::info!("sleep timer: {:?}", self.sleep.map(|s| s.sleep));
    }

    /// A download a waiting track needs will never land.
    ///
    /// That wait has no end, so walk on to the next item that can still load,
    /// opening it the way the failed one would have opened, or stop cleanly if
    /// there is none. Only a waiting track is affected: one already streaming
    /// sees the failure in its reads and ends the decode, which advances the
    /// queue.
    pub fn track_failed(&mut self, id: QueueItemId) {
        let Some(waiting) = self.waiting().filter(|w| w.id == id) else {
            return;
        };
        if !self.shared_state.is_cursor(id) {
            return;
        }
        log::info!("track {:?} cannot load, moving on", id);
        let next = self.shared_state.advance_cursor_loadable();
        self.carry_on(next, Some(waiting.start));
    }

    /// Decode thread naturally finished (playlist exhausted or error).
    /// Advance to the next playable track; otherwise stop cleanly.
    ///
    /// Ignored from a session already torn down: a play or seek handled after
    /// the message was sent has replaced what it describes.
    fn on_decode_finished(&mut self, session: u64) {
        if session != self.session || self.renderer_stream_finished() {
            return;
        }
        log::info!("decode finished, checking for next track");
        self.track_ended();
    }

    /// The track under the playhead played out, here or on a renderer. Go on
    /// as the play mode says: the same item again under repeat one, the next
    /// otherwise, from the top again when the queue repeats.
    ///
    /// A track that has not finished downloading parks the cursor on it, so its
    /// `TrackReady`/`TrackStreamReady` resumes the queue instead of being
    /// discarded as "not the cursor".
    ///
    /// Opening the item that just ended again — repeating it, or a queue of
    /// one repeating — closes the play that ended first, so the session that
    /// opens is a new play rather than a seek within it. A repeat of something
    /// that played for no time at all — a file that opens but decodes nothing
    /// — would go round forever, so it stops instead.
    fn track_ended(&mut self) {
        self.bank_listening();
        let ended = self.in_flight.as_ref().map(|f| f.item);
        let heard = self.in_flight.as_ref().is_some_and(|f| f.listened_ms() > 0);
        // A track streamed while it downloads, ended with nothing heard and
        // its download still running, did not play: the stream gave out ahead
        // of the bytes. It is waited for and opened from disk when it lands,
        // as a track not yet downloaded is, rather than passed over for the
        // next. Only a track that has failed is moved past.
        if !heard
            && let Some(id) = self.session().map(|s| s.track.id)
            && self.shared_state.is_cursor(id)
            && matches!(self.shared_state.item_state(id), Some(ItemState::Pending))
        {
            log::info!("track {id:?} ended unheard before its download did; waiting for it");
            let start = self.intent().unwrap_or(Run::Playing);
            self.park(id, 0, start);
            return;
        }
        if self.mode.repeat != Repeat::Off && !heard {
            log::info!("track ended with nothing heard; not repeating it");
            self.stop_playback_and_clear_state();
            return;
        }
        let again = (self.mode.repeat == Repeat::One)
            .then(|| self.shared_state.cursor())
            .flatten()
            .filter(|id| {
                self.shared_state
                    .item_state(*id)
                    .is_some_and(|s| !matches!(s, ItemState::Failed(_)))
            });
        let next = again.or_else(|| self.shared_state.advance_cursor_loadable());
        if next.is_some() && next == ended {
            self.finish_play();
        }
        let sleeps = self.intent().is_some() && self.sleeps_between(ended, next);
        let intent = if sleeps {
            Some(Run::Paused)
        } else {
            self.intent()
        };
        self.carry_on(next, intent);
        if sleeps {
            self.end_sleep_fade();
        }
    }

    /// Restart the session at the playhead if the queue no longer follows the
    /// playing track with what the decoder has already queued.
    ///
    /// The decoder looks ahead a few seconds before a track ends, and what it
    /// queued is in the ring. Without this, removing or moving the next track
    /// in that window has no effect, and a track inserted to play next is
    /// skipped. A lookahead not yet taken needs nothing: the decoder reads the
    /// queue when it gets there. One that found the end of the queue counts as
    /// taken, so a track added then follows gaplessly instead of after a stop.
    fn revoke_stale_lookahead(&mut self) {
        if self.session().is_none() {
            return;
        }
        self.update_playback_state();
        let Some(playhead) = self.timeline.playhead() else {
            return;
        };
        let position_ms = playhead.position_ms;
        let Some(session) = self.session().filter(|s| s.track.id == playhead.id) else {
            return;
        };
        let stale = {
            let steps = session.lookahead.lock();
            // The steps the playhead has yet to reach: those that open a
            // boundary past the one it is in. By boundary, not by the item
            // chosen — an item repeated is chosen by every step.
            steps
                .iter()
                .filter(|step| step.boundary > playhead.boundary)
                .any(|step| !self.shared_state.still_follows(step))
        };
        if !stale {
            return;
        }
        log::info!("queue changed under the lookahead, restarting at {position_ms}ms");
        if let Err(e) = self.restart_current(position_ms) {
            log::error!("restart after a queue edit failed: {}", e);
        }
    }

    /// Snapshot items with their predecessors for an undo of "these were removed".
    /// In playlist order, so undo re-inserts each item after a predecessor that
    /// is already back in place.
    fn snapshot_for_undo(
        &self,
        ids: &[QueueItemId],
    ) -> Vec<(Box<state::PlaylistItem>, Option<QueueItemId>)> {
        self.shared_state
            .items_before(ids)
            .into_iter()
            .filter_map(|(id, after)| Some((Box::new(self.shared_state.get_item(id)?), after)))
            .collect()
    }

    /// Route an undo entry to the batch buffer (if batching) or the undo stack.
    fn push_undo(&mut self, entry: UndoEntry) {
        if let Some(ref mut batch) = self.batch_buffer {
            batch.push(entry);
        } else {
            self.undo_stack.push(entry);
        }
    }

    /// Process a single command.
    pub fn process_command(&mut self, cmd: PlayerCommand) {
        // Someone wants to hear something, or has picked where: the renderer
        // remembered from last time is no longer what to go back to. A cue is
        // how a session is restored at launch, which is what the renderer is
        // to carry on with, so it is not one of them.
        // So, too, is one that pauses, stops or replaces the session: what was
        // held for the renderer is no longer wanted anywhere.
        if cmd.asks_to_play() && !matches!(cmd, PlayerCommand::Cue { .. })
            || matches!(
                cmd,
                PlayerCommand::UseRenderer(_)
                    | PlayerCommand::SetOutputDevice(_)
                    | PlayerCommand::ClearOutputDevice
                    | PlayerCommand::Pause
                    | PlayerCommand::PauseAndReport(_)
                    | PlayerCommand::Stop
                    | PlayerCommand::ClearPlaylist
                    | PlayerCommand::ReplacePlaylist { .. }
            )
        {
            self.resume_renderer = false;
            self.held_run = None;
        }
        // The restored session, while the renderer it played on is looked
        // for: opened paused, its run held for wherever it ends up.
        let cmd = match cmd {
            PlayerCommand::Cue {
                id,
                position_ms,
                play: true,
            } if self.resume_renderer => {
                self.held_run = Some(Run::Playing);
                PlayerCommand::Cue {
                    id,
                    position_ms,
                    play: false,
                }
            }
            cmd => cmd,
        };
        let edits_queue = matches!(
            cmd,
            PlayerCommand::AddToPlaylist(_)
                | PlayerCommand::InsertInPlaylist { .. }
                | PlayerCommand::RemoveFromPlaylist(_)
                | PlayerCommand::RemoveFromPlaylistBatch(_)
                | PlayerCommand::MoveInPlaylist { .. }
                | PlayerCommand::MoveItemsInPlaylist { .. }
                | PlayerCommand::ReorderPlaylist(_)
                | PlayerCommand::Undo
                | PlayerCommand::Redo
                | PlayerCommand::SetShuffle(_)
                | PlayerCommand::SetRepeat(_)
                | PlayerCommand::RestorePlayMode(_)
        );
        self.apply_command(cmd);
        if edits_queue {
            self.revoke_stale_lookahead();
        }
        self.publish();
    }

    fn apply_command(&mut self, cmd: PlayerCommand) {
        match cmd {
            PlayerCommand::Play(id) => self.play(id),
            PlayerCommand::Cue {
                id,
                position_ms,
                play,
            } => self.cue(
                id,
                position_ms,
                if play { Run::Playing } else { Run::Paused },
            ),
            PlayerCommand::Pause => {
                self.paused_in_sleep_fade();
                self.pause();
            }
            // Every command before this one has been applied and published:
            // commands are taken one at a time, each published as it ends.
            PlayerCommand::Barrier(reply) => {
                let _ = reply.send(());
            }
            PlayerCommand::PauseAndReport(reply) => {
                self.pause();
                self.silence_waiters.push(reply);
                if self
                    .session()
                    .is_none_or(|p| p.engine().is_none_or(|e| !e.is_running()))
                {
                    self.answer_silence();
                }
            }
            PlayerCommand::Resume => self.resume(),
            PlayerCommand::Stop => self.stop(),
            PlayerCommand::Seek(pos) => self.seek(pos),
            PlayerCommand::NextTrack => self.next_track(),
            PlayerCommand::PrevTrack => self.prev_track(),
            PlayerCommand::AddToPlaylist(items) => {
                let ids: Vec<QueueItemId> = items.iter().map(|i| i.id).collect();
                let whole = self.shared_state.is_empty();
                self.shared_state.add_items(items);
                // Into an empty queue, an add is a queue arriving whole.
                if whole
                    && self.mode.shuffle
                    && let Some(&first) = ids.first()
                {
                    self.shared_state.shuffle_from(first);
                }
                self.push_undo(UndoEntry::Added { ids });
            }
            PlayerCommand::UpdatePaths(updates) => {
                self.shared_state.update_paths(&updates);
                if let Transport::Loaded(session) = &mut self.transport
                    && let Some((_, new_path)) =
                        updates.iter().find(|(id, _)| *id == session.track.id)
                {
                    session.track.path = new_path.clone();
                }
            }
            PlayerCommand::InsertInPlaylist { items, after } => {
                let ids: Vec<QueueItemId> = items.iter().map(|i| i.id).collect();
                self.shared_state.insert_items_after(items, after);
                self.push_undo(UndoEntry::Inserted { ids });
            }
            PlayerCommand::ClearPlaylist => self.clear_playlist(),
            PlayerCommand::ReplacePlaylist {
                items,
                start,
                position_ms,
                play,
            } => {
                if items.is_empty() {
                    self.clear_playlist();
                    return;
                }
                let start_id = items.get(start).unwrap_or(&items[0]).id;
                // Stopped before the swap, as `clear_playlist` does, and the
                // swap one change: see `SharedPlayerState::replace_playlist`.
                self.stop_playback_and_clear_state();
                let (old_items, cursor) = self.shared_state.replace_playlist(items);
                if self.mode.shuffle {
                    self.shared_state.shuffle_from(start_id);
                }
                self.push_undo(UndoEntry::Replaced {
                    items: old_items,
                    cursor,
                });
                self.cue(
                    start_id,
                    position_ms,
                    if play { Run::Playing } else { Run::Paused },
                );
            }
            PlayerCommand::RemoveFromPlaylist(id) => {
                let item = self.shared_state.get_item(id);
                let after = self.shared_state.item_before(id);
                self.remove_from_playlist(id);
                if let Some(item) = item {
                    self.push_undo(UndoEntry::Removed {
                        items: vec![(Box::new(item), after)],
                    });
                }
            }
            PlayerCommand::RemoveFromPlaylistBatch(ids) => {
                // Snapshot before removing anything, and resolve the resume point
                // once: removing one at a time would restart the engine for every
                // deleted track that the cursor lands on along the way.
                let items_with_pos = self.snapshot_for_undo(&ids);
                let intent = self.intent();
                let resume_after = match self.shared_state.cursor() {
                    Some(cursor) if ids.contains(&cursor) => {
                        Some(self.shared_state.surviving_item_before(cursor, &ids))
                    }
                    _ => None,
                };

                self.shared_state.remove_items(&ids);

                if let Some(resume_after) = resume_after {
                    self.shared_state.set_cursor(resume_after);
                    let next = self.shared_state.advance_cursor_loadable();
                    self.carry_on(next, intent);
                }

                if !items_with_pos.is_empty() {
                    self.push_undo(UndoEntry::Removed {
                        items: items_with_pos,
                    });
                }
            }
            PlayerCommand::MoveInPlaylist { id, target, after } => {
                let was_after = self.shared_state.item_before(id);
                self.shared_state.move_item(id, target, after);
                self.push_undo(UndoEntry::Moved { id, was_after });
            }
            PlayerCommand::MoveItemsInPlaylist { ids, target, after } => {
                let entries = self.shared_state.items_before(&ids);
                self.shared_state.move_items(&ids, target, after);
                self.push_undo(UndoEntry::MovedBatch { entries });
            }
            PlayerCommand::ReorderPlaylist(order) => {
                // Undoable like any other move: undoing it puts the queue back
                // and, by doing so, ends the lock — which is the honest result
                // of having rearranged the queue by hand.
                let entries = self.shared_state.items_before(&order);
                self.shared_state.reorder_to(&order);
                self.push_undo(UndoEntry::MovedBatch { entries });
            }
            PlayerCommand::TrackReady(id) => self.track_ready(id),
            PlayerCommand::DecodeFinished(session) => self.on_decode_finished(session),
            // Nothing to do but wake: the loop works out when the playhead
            // reaches the track just queued.
            PlayerCommand::TrackQueued => {}
            PlayerCommand::TrackStreamReady(id) => self.track_stream_ready(id),
            PlayerCommand::StreamProbed { id, info, mode } => self.stream_probed(id, *info, mode),
            PlayerCommand::TrackFailed(id) => self.track_failed(id),
            PlayerCommand::CacheTracks(ids) => {
                if let Some(downloads) = &self.downloads {
                    downloads.cache(ids);
                }
            }
            PlayerCommand::Undo => self.execute_undo(),
            PlayerCommand::Redo => self.execute_redo(),
            PlayerCommand::BeginUndoBatch => {
                self.batch_buffer = Some(Vec::new());
            }
            PlayerCommand::EndUndoBatch => {
                if let Some(entries) = self.batch_buffer.take() {
                    if entries.len() == 1 {
                        // Single entry — push directly, no wrapping.
                        self.undo_stack.push(entries.into_iter().next().unwrap());
                    } else if !entries.is_empty() {
                        self.undo_stack.push(UndoEntry::Batch(entries));
                    }
                }
            }
            PlayerCommand::SetOutputDevice(name) => self.set_output_device(name),
            PlayerCommand::RestartOutput => {
                log::info!("restarting audio output");
                self.restart_on_current_track();
            }
            PlayerCommand::ClearOutputDevice => self.clear_output_device(),
            PlayerCommand::ReloadDsp => self.reload_dsp(),
            PlayerCommand::RouteChanged => {
                crate::audio::follow_route();
                self.reload_dsp();
            }
            PlayerCommand::UseRenderer(connection) => {
                self.remember_renderer(connection.as_deref());
                self.use_renderer(connection);
            }
            PlayerCommand::ResumeRenderer(connection) => {
                if std::mem::take(&mut self.resume_renderer) && self.renderer.is_none() {
                    log::info!(
                        "upnp: back to {}, as last time",
                        connection.session.renderer().name
                    );
                    self.use_renderer(Some(connection));
                    if self.held_run.take() == Some(Run::Playing) {
                        self.resume();
                    }
                } else {
                    log::info!(
                        "upnp: not going back to {}: playback or the output moved first",
                        connection.session.renderer().name
                    );
                }
            }
            PlayerCommand::ResumeRendererMissed => {
                if std::mem::take(&mut self.resume_renderer)
                    && self.held_run.take() == Some(Run::Playing)
                {
                    log::info!("upnp: playing here what was held for the renderer");
                    self.resume();
                }
            }
            PlayerCommand::ReleaseRenderer(reply) => {
                self.release_renderer();
                let _ = reply.send(());
            }
            PlayerCommand::SetRendererVolume(volume) => self.set_renderer_volume(volume),
            PlayerCommand::Renderer { session, event } => self.on_renderer_event(session, event),
            PlayerCommand::SetShuffle(on) => self.set_shuffle(on),
            PlayerCommand::SetRepeat(repeat) => self.mode.repeat = repeat,
            PlayerCommand::RestorePlayMode(mode) => self.mode = mode,
            PlayerCommand::SetSleepTimer(timer) => self.set_sleep_timer(timer),
        }
    }

    /// Turn shuffle on or off, as one undoable step.
    ///
    /// On, the items after the cursor go in a random order, each keeping a
    /// note of where it stood. Off, they go back where they stood, in the
    /// places such items occupy now, so an item added meanwhile stays put.
    /// The queue is the order things play in either way: the lookahead, the
    /// downloads and every remote queue simply follow it.
    fn set_shuffle(&mut self, on: bool) {
        if self.mode.shuffle == on {
            return;
        }
        let order = self.shared_state.shuffle_order();
        if on {
            self.shared_state.shuffle_after_cursor();
        } else {
            self.shared_state.unshuffle();
        }
        self.push_undo(UndoEntry::Shuffled {
            shuffle: self.mode.shuffle,
            order,
        });
        self.mode.shuffle = on;
    }

    /// Stop, then clear the playlist as one undoable step. Playback and display
    /// state go first, without touching the playlist, so the snapshot taken for
    /// undo is of the playlist as it was.
    fn clear_playlist(&mut self) {
        self.stop_playback_and_clear_state();
        let (items, cursor) = self.shared_state.snapshot_playlist();
        self.shared_state.clear_playlist();
        self.push_undo(UndoEntry::Replaced { items, cursor });
    }

    /// Apply an undo/redo entry: mutate the playlist and return the inverse entry.
    fn apply_entry(&mut self, entry: UndoEntry) -> UndoEntry {
        match entry {
            UndoEntry::Added { ids } | UndoEntry::Inserted { ids } => {
                // Undo of "items were added": snapshot them with positions, then remove.
                let items_with_pos = self.snapshot_for_undo(&ids);
                self.shared_state.remove_items(&ids);
                UndoEntry::Removed {
                    items: items_with_pos,
                }
            }
            UndoEntry::Removed { items } => {
                // Undo of "items were removed": re-insert each at its position.
                let mut ids = Vec::with_capacity(items.len());
                for (item, after) in items {
                    ids.push(item.id);
                    self.shared_state.insert_item_at(*item, after);
                }
                UndoEntry::Added { ids }
            }
            UndoEntry::Moved { id, was_after } => {
                let current_after = self.shared_state.item_before(id);
                self.shared_state.move_item_to(id, was_after);
                UndoEntry::Moved {
                    id,
                    was_after: current_after,
                }
            }
            UndoEntry::MovedBatch { entries } => {
                let ids: Vec<QueueItemId> = entries.iter().map(|(id, _)| *id).collect();
                let current_positions = self.shared_state.items_before(&ids);
                self.shared_state.move_items_to(&entries);
                UndoEntry::MovedBatch {
                    entries: current_positions,
                }
            }
            UndoEntry::Replaced { items, cursor } => {
                let (current_items, current_cursor) = self.shared_state.snapshot_playlist();
                self.shared_state.restore_playlist(items, cursor);
                UndoEntry::Replaced {
                    items: current_items,
                    cursor: current_cursor,
                }
            }
            UndoEntry::Shuffled { shuffle, order } => {
                let inverse = UndoEntry::Shuffled {
                    shuffle: self.mode.shuffle,
                    order: self.shared_state.shuffle_order(),
                };
                self.shared_state.restore_shuffle_order(&order);
                self.mode.shuffle = shuffle;
                inverse
            }
            UndoEntry::Batch(entries) => {
                // Apply entries in reverse order, collect inverses.
                let mut inverses: Vec<_> = entries
                    .into_iter()
                    .rev()
                    .map(|e| self.apply_entry(e))
                    .collect();
                inverses.reverse();
                UndoEntry::Batch(inverses)
            }
        }
    }

    /// Put playback back in agreement with the playlist.
    ///
    /// The engine keeps decoding whatever it was on while the playlist changes
    /// underneath it, which an undo can turn into a lie: undoing a replace
    /// restores the queue but leaves the engine playing a track that queue does
    /// not contain. The transport then describes an item nothing can select,
    /// and the decode lookahead — which finds the next track by locating the
    /// current one — has nothing to follow, so the queue ends at the end of the
    /// track instead of carrying on.
    ///
    /// Done once, after the entry is applied, rather than inside each variant:
    /// any undo that takes items away can orphan the engine, not only
    /// `Replaced`. A track waiting to open can be orphaned the same way, and
    /// would otherwise open when its download lands.
    ///
    /// Playing carries on playing and paused stays paused, the same as when
    /// the playing track is removed: an undo is not a reason to start the
    /// music, or to stop it.
    fn reconcile_playback(&mut self) {
        let orphaned = match &self.transport {
            Transport::Idle => false,
            Transport::Waiting(waiting) => self.shared_state.get_item(waiting.id).is_none(),
            Transport::Loaded(session) => self.shared_state.get_item(session.track.id).is_none(),
        };
        if !orphaned {
            return;
        }
        // Pick the restored queue up where its cursor says it was, as the
        // listener had it. The position is not part of what was snapshotted,
        // so the track begins again.
        let cursor = self.shared_state.cursor();
        self.carry_on(cursor, self.intent());
    }

    /// Execute an undo operation, pushing the inverse onto the redo stack.
    fn execute_undo(&mut self) {
        let Some(entry) = self.undo_stack.pop_undo() else {
            return;
        };
        let inverse = self.apply_entry(entry);
        self.undo_stack.push_redo(inverse);
        self.reconcile_playback();
    }

    /// Execute a redo operation, pushing the inverse onto the undo stack.
    fn execute_redo(&mut self) {
        let Some(entry) = self.undo_stack.pop_redo() else {
            return;
        };
        let inverse = self.apply_entry(entry);
        self.undo_stack.push_undo_keep_redo(inverse);
        self.reconcile_playback();
    }

    /// Run the command loop. Blocks until the sender is dropped.
    ///
    /// Asleep until there is something to do: a command, or one of the two
    /// things that happen without one — see `next_wake`. A paused or stopped
    /// player does not wake at all.
    pub fn run(&mut self) {
        use crossbeam_channel::RecvTimeoutError;

        let rx = self.commands.rx.clone();
        loop {
            let received = match self.next_wake() {
                Some(at) => rx.recv_deadline(at),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(cmd) => self.process_command(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.update_playback_state();
        }
        self.stop();
    }

    /// Give the audio session back once output has been stopped for
    /// `SESSION_GRACE`, so another app can play. A resume inside it finds the
    /// session still active. Waiting for a track to arrive with play asked
    /// for counts as playing: the session is wanted again the moment it does.
    fn release_idle_session(&mut self) {
        if self.session_release_due(
            self.wants_output(),
            crate::audio::session_held(),
            std::time::Instant::now(),
        ) {
            log::info!("output idle: releasing the audio session");
            crate::audio::release_session();
        }
    }

    /// Play asked for, or output still running (a pause fading out).
    fn wants_output(&self) -> bool {
        self.intent() == Some(Run::Playing)
            || self
                .session()
                .and_then(Session::engine)
                .is_some_and(|e| e.is_running())
    }

    /// Whether the session is to be released now, starting the grace when
    /// output has just stopped.
    fn session_release_due(&mut self, playing: bool, held: bool, now: std::time::Instant) -> bool {
        if playing || !held {
            self.session_release_at = None;
            return false;
        }
        match self.session_release_at {
            None => {
                self.session_release_at = Some(now + SESSION_GRACE);
                false
            }
            Some(at) if now >= at => {
                self.session_release_at = None;
                true
            }
            Some(_) => false,
        }
    }

    /// When something changes that no command announces: one of the
    /// session's events (`next_event`), a sleep timer's time coming, or the
    /// audio session's release.
    fn next_wake(&self) -> Option<std::time::Instant> {
        [
            self.next_event(),
            self.sleep.and_then(|s| s.at),
            self.sleep_wake(),
            self.session_release_at,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// The playhead reaching the next queued track, the silence after a rate
    /// switch running out, or a pause fading to silence.
    fn next_event(&self) -> Option<std::time::Instant> {
        let session = self.session()?;
        let now = std::time::Instant::now();
        let Output::Local(local) = &session.output else {
            // A stream's track changes are the timeline's, as they are here.
            let next_track = (session.run == Run::Playing && self.streaming_to_renderer())
                .then(|| self.timeline.until_next_track())
                .flatten()
                .map(|left| now + left + BOUNDARY_SLACK);
            return match (next_track, self.renderer_deadline()) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        };
        if let Some(since) = self.dsp_restart {
            return Some(dsp_wake(since, local.engine.period(), now));
        }
        match session.run {
            Run::Playing => {
                let next_track = self
                    .timeline
                    .until_next_track()
                    .map(|left| now + left + BOUNDARY_SLACK);
                match (next_track, self.lead_in_ends) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            }
            Run::Paused if local.engine.is_running() => Some(now + FADE_CHECK),
            Run::Paused => None,
        }
    }

    /// Spawn the player on a background thread, returning the shared state,
    /// timeline, visualization snapshot, and command sender.
    /// Remember the renderer picked as the output, or that this device's own
    /// was, for the next launch. Only a choice is remembered: a renderer that
    /// drops off the network is gone back to next time.
    fn remember_renderer(&self, connection: Option<&crate::upnp::Connection>) {
        let renderer = connection.map(|c| c.session.renderer());
        if let Err(e) = crate::config::Config::persist(|cfg| {
            cfg.playback.renderer = renderer.map(|r| r.udn.clone());
            cfg.playback.renderer_name = renderer.map(|r| r.name.clone());
        }) {
            log::error!("failed to save the output: {e}");
        }
    }

    /// Spawn a player for a process that does not own an output of its own:
    /// `koan serve`, `koan mcp`. It plays where it is told and nowhere else.
    pub fn spawn() -> (
        Arc<SharedPlayerState>,
        Arc<PlaybackTimeline>,
        Arc<VizSnapshot>,
        crossbeam_channel::Sender<PlayerCommand>,
    ) {
        Self::spawn_with(false)
    }

    /// Spawn the player for an app someone listens through: the macOS and iOS
    /// apps and `koan play`. It goes back to the renderer used last time if
    /// that turns up at launch (`upnp::resume`). A headless process must not:
    /// the config is the machine's, and it would take the amplifier from the
    /// app.
    pub fn spawn_for_listening() -> (
        Arc<SharedPlayerState>,
        Arc<PlaybackTimeline>,
        Arc<VizSnapshot>,
        crossbeam_channel::Sender<PlayerCommand>,
    ) {
        Self::spawn_with(true)
    }

    /// The renderer to go back to at launch: the one used last time, for a
    /// player someone listens through.
    fn renderer_to_resume(listening: bool) -> Option<String> {
        listening
            .then(|| crate::config::Config::cached().playback.renderer.clone())
            .flatten()
    }

    fn spawn_with(
        listening: bool,
    ) -> (
        Arc<SharedPlayerState>,
        Arc<PlaybackTimeline>,
        Arc<VizSnapshot>,
        crossbeam_channel::Sender<PlayerCommand>,
    ) {
        // EQ from before presets and tunings of several EQs, brought up to
        // them before anything plays through it.
        if let Err(e) = crate::audio::dsp::profiles::migrate() {
            log::warn!("dsp: not brought up to date: {e}");
        }
        let mut player = Self::new();
        player.history = PlayRecorder::spawn();
        let state = player.shared_state();
        let timeline = player.timeline();
        let viz_snapshot = player.viz_snapshot();
        let tx = player.command_sender();
        // Downloads follow the playlist, so they come with the player rather
        // than being something each front end has to remember to ask for.
        player.downloads = Some(crate::remote::queue::DownloadQueue::spawn(
            tx.clone(),
            state.clone(),
        ));

        if let Some(udn) = Self::renderer_to_resume(listening) {
            player.resume_renderer = true;
            crate::upnp::resume(udn, &tx);
        }

        thread::Builder::new()
            .name("koan-player".into())
            .spawn(move || player.run())
            .expect("failed to spawn player thread");

        (state, timeline, viz_snapshot, tx)
    }
}

#[cfg(test)]
mod tests {
    /// `koan serve` and `koan mcp` read the same config as the app, and must
    /// not go looking for the app's amplifier.
    #[test]
    fn a_headless_player_does_not_go_back_to_a_renderer() {
        let _guard = crate::config::tests::PERSIST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::config::set_config_dir(dir.path());
        crate::config::Config::persist(|cfg| {
            cfg.playback.renderer = Some("uuid:headless-test".into());
        })
        .unwrap();
        assert_eq!(Player::renderer_to_resume(false), None);
    }

    #[test]
    fn a_download_in_progress_is_known_by_its_own_extension() {
        use std::path::Path;
        // `.part` is the transfer's, not the track's.
        assert_eq!(
            lengthless_mode_for(Path::new("/c/t.m4a.part")),
            streaming::ProbeMode::LengthlessWholeEnd
        );
        assert_eq!(
            lengthless_mode_for(Path::new("/c/t.OPUS.part")),
            streaming::ProbeMode::LengthlessWholeEnd
        );
        assert_eq!(
            lengthless_mode_for(Path::new("/c/t.flac.part")),
            streaming::ProbeMode::Lengthless
        );
        assert_eq!(
            media_extension(Path::new("/c/t.m4a.part")).as_deref(),
            Some("m4a")
        );
        assert_eq!(
            media_extension(Path::new("/c/t.mp3")).as_deref(),
            Some("mp3")
        );
    }

    use super::*;
    use state::PlaylistItem;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn make_item(title: &str) -> PlaylistItem {
        PlaylistItem {
            playlist_entry_id: None,
            id: QueueItemId::new(),
            db_id: None,
            path: PathBuf::from(format!("/music/{title}.flac")),
            title: title.to_string(),
            artist: String::new(),
            album_artist: String::new(),
            album: String::new(),
            year: None,
            codec: None,
            track_number: None,
            disc: None,
            duration_ms: None,
            state: ItemState::Ready,
            pre_shuffle: None,
        }
    }

    pub(super) fn playlist_ids(player: &Player) -> Vec<QueueItemId> {
        let (items, _) = player.shared_state.snapshot_playlist();
        items.iter().map(|i| i.id).collect()
    }

    fn playlist_titles(player: &Player) -> Vec<String> {
        let (items, _) = player.shared_state.snapshot_playlist();
        items.iter().map(|i| i.title.clone()).collect()
    }

    fn pending_item(title: &str) -> PlaylistItem {
        PlaylistItem {
            playlist_entry_id: None,
            state: ItemState::Pending,
            ..make_item(title)
        }
    }

    /// A session playing `id` on `engine`, with no decode thread behind it.
    fn test_session(id: QueueItemId, engine: Box<dyn AudioEngineHandle>) -> Session {
        Session {
            track: TrackInfo {
                id,
                path: PathBuf::from("/music/t.flac"),
                codec: String::new(),
                sample_rate: 44_100,
                bit_depth: None,
                bitrate_kbps: None,
                channels: 2,
                duration_ms: 1_000,
            },
            run: Run::Playing,
            lookahead: Default::default(),
            output: Output::Local(Local {
                engine,
                decode_handle: buffer::DecodeHandle::new_for_test(Default::default()),
                stream: None,
                _rate_watch: None,
                dsp: None,
            }),
        }
    }

    /// Stand in for an engine that is playing `id`. The test items have no
    /// files behind them, so `start_playback` can never get far enough to leave
    /// this state on its own.
    fn pretend_playing(player: &mut Player, id: QueueItemId) {
        assert!(
            player.shared_state.get_item(id).is_some(),
            "item is in the queue"
        );
        player.stop_engine();
        player.transport = Transport::Loaded(test_session(
            id,
            Box::new(NullEngine {
                starts: Default::default(),
                running: Default::default(),
                lead_in: Default::default(),
            }),
        ));
        player.shared_state.set_cursor(Some(id));
        player.publish();
    }

    fn playing_id(player: &Player) -> Option<QueueItemId> {
        player.shared_state.track_info().map(|t| t.id)
    }

    /// Build `n` ready items, add them, and return their IDs.
    fn seed(player: &mut Player, n: usize) -> Vec<QueueItemId> {
        let items: Vec<_> = (0..n).map(|i| make_item(&format!("t{i}"))).collect();
        let ids = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));
        ids
    }

    // --- cursor transitions ---

    /// Feed the player a track's worth of playback ticks, as the 50ms poll would.
    fn listen(player: &mut Player, from_ms: u64, to_ms: u64) {
        let mut at = from_ms;
        if let Some(f) = player.in_flight.as_mut() {
            f.advance(at); // the position the needle landed on
        }
        while at < to_ms {
            at = (at + 50).min(to_ms);
            if let Some(f) = player.in_flight.as_mut() {
                f.advance(at);
            }
        }
    }

    fn start(player: &mut Player, track_id: i64) -> QueueItemId {
        let id = QueueItemId::new();
        player.on_track_changed(id, 0);
        // The item is not in a playlist here, so there is no db_id to find.
        player
            .in_flight
            .as_mut()
            .unwrap()
            .track_id_for_test(track_id);
        id
    }

    #[test]
    fn a_gapless_transition_closes_the_outgoing_track_and_opens_the_next() {
        let mut player = Player::new();
        start(&mut player, 11);
        listen(&mut player, 0, 200_000);

        let b = QueueItemId::new();
        player.on_track_changed(b, 0);
        let f = player
            .in_flight
            .as_ref()
            .expect("the next track is counting");
        assert_eq!(f.item, b);
        assert_eq!(f.listened_ms(), 0, "and starts from nothing");
    }

    #[test]
    fn a_track_skipped_seconds_in_is_still_history() {
        let mut player = Player::new();
        start(&mut player, 7);
        listen(&mut player, 0, 2_000);

        let event = player
            .finish_play()
            .expect("putting something on is a thing you did, however briefly");
        assert!(matches!(
            event,
            history::PlayEvent::Finished {
                track_id: 7,
                listened_ms: 2_000
            }
        ));
    }

    #[test]
    fn a_track_is_closed_out_once() {
        let mut player = Player::new();
        start(&mut player, 7);
        listen(&mut player, 0, 200_000);

        assert!(player.finish_play().is_some());
        assert!(player.finish_play().is_none());
    }

    #[test]
    fn seeking_around_a_track_does_not_enter_it_twice() {
        let mut player = Player::new();
        let id = start(&mut player, 7);
        listen(&mut player, 0, 120_000);

        // A seek restarts playback of the same item.
        player.on_track_changed(id, 30_000);
        assert_eq!(
            player.in_flight.as_ref().unwrap().listened_ms(),
            120_000,
            "the seek kept the count rather than restarting it"
        );
        listen(&mut player, 30_000, 40_000);

        let Some(history::PlayEvent::Finished { listened_ms, .. }) = player.finish_play() else {
            panic!("still one play");
        };
        assert_eq!(listened_ms, 130_000);
        assert!(player.finish_play().is_none());
    }

    #[test]
    fn a_track_that_is_not_in_the_library_is_not_recorded() {
        let mut player = Player::new();
        let id = QueueItemId::new();
        player.on_track_changed(id, 0);
        listen(&mut player, 0, 200_000);
        assert!(player.finish_play().is_none());
    }

    #[test]
    fn stopping_closes_out_what_was_heard() {
        let mut player = Player::new();
        start(&mut player, 7);
        listen(&mut player, 0, 150_000);

        player.stop_playback_and_clear_state();
        assert!(player.in_flight.is_none(), "the stop consumed it");
    }

    #[test]
    fn resume_with_nothing_loaded_plays_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 10.0, 16);

        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let item = PlaylistItem {
            db_id: Some(5),
            path,
            ..make_item("t")
        };
        let id = item.id;
        player.process_command(PlayerCommand::AddToPlaylist(vec![item]));
        player.shared_state.set_cursor(Some(id));
        assert!(player.session().is_none());

        player.process_command(PlayerCommand::Resume);
        assert!(player.session().is_some(), "the cursor's track started");
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Playing);
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn the_audio_session_is_released_only_after_output_has_been_idle_a_while() {
        let mut player = Player::new();
        let now = std::time::Instant::now();
        assert!(
            !player.session_release_due(false, true, now),
            "the grace starts"
        );
        assert!(!player.session_release_due(false, true, now + SESSION_GRACE / 2));
        assert!(
            player.session_release_due(false, true, now + SESSION_GRACE),
            "released once it runs out"
        );
        assert_eq!(player.session_release_at, None);

        assert!(!player.session_release_due(false, true, now));
        assert!(
            !player.session_release_due(true, true, now + SESSION_GRACE / 2),
            "playing again inside the grace keeps the session"
        );
        assert_eq!(player.session_release_at, None);
        assert!(
            !player.session_release_due(false, false, now),
            "nothing held"
        );
        assert_eq!(player.session_release_at, None);
    }

    #[test]
    fn waiting_to_play_a_track_keeps_the_audio_session() {
        let mut player = Player::new();
        let item = make_item("t");
        player.transport = Transport::Waiting(Waiting {
            id: item.id,
            position_ms: 0,
            start: Run::Playing,
        });
        assert!(player.wants_output(), "play was asked for");
        let playing = player.wants_output();
        let now = std::time::Instant::now();
        for later in [now, now + SESSION_GRACE * 2] {
            assert!(!player.session_release_due(playing, true, later));
        }
        assert_eq!(player.session_release_at, None);
    }

    #[test]
    fn a_player_with_nothing_coming_has_nothing_to_wake_for() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 10.0, 16);

        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let item = PlaylistItem {
            path,
            ..make_item("t")
        };
        let id = item.id;
        player.process_command(PlayerCommand::AddToPlaylist(vec![item]));
        assert_eq!(player.next_wake(), None, "stopped");

        player.process_command(PlayerCommand::Play(id));
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Playing);
        assert_eq!(
            player.next_wake(),
            None,
            "playing, with no track queued after it"
        );

        if let Transport::Loaded(session) = &mut player.transport {
            session.engine().unwrap().stop().unwrap();
            session.run = Run::Paused;
        }
        assert_eq!(player.next_wake(), None, "paused");
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_track_cued_paused_never_starts_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 10.0, 16);

        let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: starts.clone(),
        });
        let item = PlaylistItem {
            path,
            ..make_item("t")
        };
        let id = item.id;
        player.process_command(PlayerCommand::AddToPlaylist(vec![item]));

        player.process_command(PlayerCommand::Cue {
            id,
            position_ms: 2_000,
            play: false,
        });
        assert!(player.session().is_some(), "loaded");
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Paused);
        // The decoder says where the seek landed when it queues the track,
        // and the playhead reads that from then on: the start of the packet
        // holding 2s, which can be a little before it.
        loop {
            match player
                .commands
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
            {
                Ok(PlayerCommand::TrackQueued) => break,
                Ok(_) => {}
                Err(e) => panic!("the decoder never queued the track: {e}"),
            }
        }
        let at = player.shared_state.position_ms();
        assert!((1_750..=2_000).contains(&at), "cued at {at}ms");
        assert_eq!(starts.load(Ordering::Relaxed), 0, "not a sample let out");

        // Seeking while paused reopens the track, and stays quiet too.
        player.process_command(PlayerCommand::Seek(4_000));
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Paused);
        assert_eq!(starts.load(Ordering::Relaxed), 0);

        player.process_command(PlayerCommand::Resume);
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Playing);
        assert_eq!(starts.load(Ordering::Relaxed), 1);
        player.process_command(PlayerCommand::Stop);
    }

    /// Load `t.wav` as an item whose download has not landed, under a player
    /// on the fake output.
    fn downloading_wav(dir: &Path) -> (Player, QueueItemId, Arc<std::sync::atomic::AtomicUsize>) {
        let path = dir.join("t.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 10.0, 16);
        let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: starts.clone(),
        });
        let item = PlaylistItem {
            path,
            state: ItemState::Pending,
            ..make_item("t")
        };
        let id = item.id;
        player.process_command(PlayerCommand::AddToPlaylist(vec![item]));
        (player, id, starts)
    }

    /// The download lands, as the downloader says so: the item first, then
    /// the player.
    fn land(player: &mut Player, id: QueueItemId) {
        player.shared_state.update_item_state(id, ItemState::Ready);
        player.process_command(PlayerCommand::TrackReady(id));
    }

    #[test]
    fn track_ready_never_resurrects_an_item_that_failed() {
        let mut player = Player::new();
        let item = PlaylistItem {
            state: ItemState::Failed("gone".into()),
            ..make_item("t")
        };
        let id = item.id;
        player.process_command(PlayerCommand::AddToPlaylist(vec![item]));

        player.process_command(PlayerCommand::TrackReady(id));

        assert!(matches!(
            player.shared_state.get_item(id).map(|i| i.state),
            Some(ItemState::Failed(_))
        ));
    }

    fn await_queued(player: &Player) {
        loop {
            match player
                .commands
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
            {
                Ok(PlayerCommand::TrackQueued) => return,
                Ok(_) => {}
                Err(e) => panic!("the decoder never queued the track: {e}"),
            }
        }
    }

    #[test]
    fn a_cue_on_a_downloading_track_opens_at_its_position_once_it_lands() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, id, starts) = downloading_wav(dir.path());

        player.process_command(PlayerCommand::Cue {
            id,
            position_ms: 6_000,
            play: true,
        });
        assert!(player.session().is_none(), "nothing opened early");
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Stopped);
        assert_eq!(starts.load(Ordering::Relaxed), 0);

        land(&mut player, id);
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Playing);
        assert_eq!(player.playback_starts, 1, "opened once, at the position");
        assert_eq!(starts.load(Ordering::Relaxed), 1);
        await_queued(&player);
        let at = player.shared_state.position_ms();
        assert!((5_750..=6_000).contains(&at), "opened at {at}ms");
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_paused_cue_on_a_downloading_track_lands_paused() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, id, starts) = downloading_wav(dir.path());

        player.process_command(PlayerCommand::Cue {
            id,
            position_ms: 3_000,
            play: false,
        });
        land(&mut player, id);
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Paused);
        assert_eq!(starts.load(Ordering::Relaxed), 0, "not a sample let out");
        await_queued(&player);
        let at = player.shared_state.position_ms();
        assert!((2_750..=3_000).contains(&at), "cued at {at}ms");
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn playing_something_else_forgets_a_waiting_cue() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, id, _) = downloading_wav(dir.path());
        let other = seed(&mut player, 1)[0];

        player.process_command(PlayerCommand::Cue {
            id,
            position_ms: 6_000,
            play: true,
        });
        player.process_command(PlayerCommand::Play(other));
        player.process_command(PlayerCommand::Play(id));
        assert!(player.waiting().is_some_and(|w| w.position_ms == 0));
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_paused_track_whose_download_lands_stays_paused_where_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, id, starts) = downloading_wav(dir.path());
        // Loaded paused mid-track, as a stream is when its download lands.
        player.shared_state.update_item_state(id, ItemState::Ready);
        player.process_command(PlayerCommand::Cue {
            id,
            position_ms: 3_000,
            play: false,
        });
        await_queued(&player);

        player.process_command(PlayerCommand::TrackReady(id));
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Paused);
        assert_eq!(player.playback_starts, 1, "not reopened");
        assert_eq!(starts.load(Ordering::Relaxed), 0, "not a sample let out");
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn pausing_a_track_on_its_way_opens_it_paused() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, id, starts) = downloading_wav(dir.path());

        player.process_command(PlayerCommand::Play(id));
        assert!(player.shared_state.is_waiting());
        assert!(player.shared_state.wants_to_play(), "a toggle pauses it");
        assert!(!player.shared_state.is_idle(), "adding tracks leaves it be");
        player.process_command(PlayerCommand::Pause);
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Paused);
        assert!(!player.shared_state.wants_to_play());

        player.shared_state.update_item_state(id, ItemState::Ready);
        player.process_command(PlayerCommand::TrackReady(id));
        assert!(player.session().is_some(), "loaded");
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Paused);
        assert_eq!(starts.load(Ordering::Relaxed), 0, "not a sample let out");
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_cue_paused_and_resumed_on_its_way_keeps_its_position() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, id, starts) = downloading_wav(dir.path());

        player.process_command(PlayerCommand::Cue {
            id,
            position_ms: 6_000,
            play: true,
        });
        player.process_command(PlayerCommand::Pause);
        player.process_command(PlayerCommand::Resume);
        assert!(player.session().is_none(), "still on its way");
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Stopped);

        player.shared_state.update_item_state(id, ItemState::Ready);
        player.process_command(PlayerCommand::TrackReady(id));
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Playing);
        assert_eq!(starts.load(Ordering::Relaxed), 1);
        await_queued(&player);
        let at = player.shared_state.position_ms();
        assert!((5_750..=6_000).contains(&at), "opened at {at}ms");
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_restored_cursor_does_not_start_when_its_download_lands() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, id, starts) = downloading_wav(dir.path());
        player.shared_state.set_cursor(Some(id));

        player.shared_state.update_item_state(id, ItemState::Ready);
        player.process_command(PlayerCommand::TrackReady(id));
        assert!(player.session().is_none());
        assert_eq!(starts.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn replacing_the_queue_keeps_a_transfer_both_queues_want() {
        // The download queue syncs from its own thread on each playlist
        // change. Replaced as a clear then an add, the playlist was empty in
        // between, and a sync there let go of every waiter: the transfer for a
        // track in both queues was abandoned and started over. The replace
        // must be one change, and that change must still want the track.
        let pending = |title: &str| PlaylistItem {
            db_id: Some(7),
            state: ItemState::Pending,
            ..make_item(title)
        };
        let mut player = Player::new();
        let old = pending("old");
        let old_id = old.id;
        player.process_command(PlayerCommand::AddToPlaylist(vec![old]));
        let store = player.shared_state.downloads().clone();
        store.claim(7, Some(old_id));

        let before = player.shared_state.pending_version();
        player.process_command(PlayerCommand::ReplacePlaylist {
            items: vec![pending("again")],
            start: 0,
            position_ms: 0,
            play: false,
        });
        assert_eq!(
            player.shared_state.pending_version(),
            before + 1,
            "one change, so no reader can see the playlist between two"
        );

        // A sync against the one state it published.
        store.resync(&player.shared_state.pending_downloads());
        assert!(!store.abandoned(7));
    }

    #[test]
    fn a_hand_off_opens_paused_at_its_position_in_one_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 10.0, 16);
        let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: starts.clone(),
        });
        seed(&mut player, 2);
        let item = PlaylistItem {
            path,
            ..make_item("t")
        };
        let id = item.id;

        player.process_command(PlayerCommand::ReplacePlaylist {
            items: vec![make_item("before"), item],
            start: 1,
            position_ms: 4_000,
            play: false,
        });
        assert_eq!(
            playlist_titles(&player),
            vec!["before", "t"],
            "replaced, not added to"
        );
        assert_eq!(player.shared_state.cursor(), Some(id));
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Paused);
        assert_eq!(player.playback_starts, 1, "opened once, at the position");
        assert_eq!(starts.load(Ordering::Relaxed), 0, "not a sample let out");
        await_queued(&player);
        let at = player.shared_state.position_ms();
        assert!((3_750..=4_000).contains(&at), "opened at {at}ms");

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["t0", "t1"], "one undo step");
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_decode_end_from_a_replaced_session_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let items: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|name| {
                let path = dir.path().join(format!("{name}.wav"));
                crate::test_utils::generate_wav(&path, 8_000, 1, 10.0, 16);
                PlaylistItem {
                    path,
                    ..make_item(name)
                }
            })
            .collect();
        let ids: Vec<_> = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));

        player.process_command(PlayerCommand::Play(ids[0]));
        let stale = player.session;
        player.process_command(PlayerCommand::Play(ids[1]));
        player.process_command(PlayerCommand::DecodeFinished(stale));

        assert_eq!(player.shared_state.cursor(), Some(ids[1]));
        assert_eq!(player.playback_starts, 2);
        player.process_command(PlayerCommand::Stop);
    }

    /// Short tracks of one format under an output that never plays: the
    /// decoder queues them all behind the first.
    fn queued_wavs(dir: &Path, names: &[&str]) -> (Player, Vec<QueueItemId>) {
        let (player, ids) = wavs_playing(dir, names, 1.0);
        for _ in &ids {
            await_queued(&player);
        }
        (player, ids)
    }

    /// `names` as WAVs of `seconds` each, the first playing. Long enough ones
    /// fill the ring before the decoder reaches the end of the queue.
    fn wavs_playing(dir: &Path, names: &[&str], seconds: f32) -> (Player, Vec<QueueItemId>) {
        wavs_in(dir, names, seconds, Repeat::Off)
    }

    /// `wavs_playing`, under `repeat` from the start. Each has a library id,
    /// its index plus one, so history records it.
    fn wavs_in(
        dir: &Path,
        names: &[&str],
        seconds: f32,
        repeat: Repeat,
    ) -> (Player, Vec<QueueItemId>) {
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let items: Vec<_> = names
            .iter()
            .zip(1..)
            .map(|(name, track)| {
                let path = dir.join(format!("{name}.wav"));
                crate::test_utils::generate_wav(&path, 8_000, 1, seconds, 16);
                PlaylistItem {
                    path,
                    db_id: Some(track),
                    ..make_item(name)
                }
            })
            .collect();
        let ids: Vec<_> = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::SetRepeat(repeat));
        player.process_command(PlayerCommand::Play(ids[0]));
        (player, ids)
    }

    /// What the decoder has queued after the playhead, once it is at least `n`.
    fn queued_at_least(player: &Player, n: usize) -> Vec<QueueItemId> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let queued = player.timeline.queued_after_playhead();
            if queued.len() >= n {
                return queued;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the decoder queued {queued:?}, never {n}"
            );
            std::thread::yield_now();
        }
    }

    // --- play modes ---

    #[test]
    fn repeating_the_queue_runs_the_last_track_into_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = wavs_in(dir.path(), &["a", "b"], 1.0, Repeat::Queue);

        let queued = queued_at_least(&player, 3);
        assert_eq!(
            queued[..3],
            [ids[1], ids[0], ids[1]],
            "gapless, round again"
        );
        let steps = player.session().unwrap().lookahead.lock().clone();
        assert!(!steps[0].wrapped);
        let wrap = steps.iter().find(|s| s.after == ids[1]).unwrap();
        assert_eq!(wrap.next, Some(ids[0]));
        assert!(wrap.wrapped);
        assert_eq!(player.playback_starts, 1);
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn turning_repeat_off_takes_back_a_wrap_already_queued() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = wavs_in(dir.path(), &["a"], 1.0, Repeat::Queue);
        assert_eq!(queued_at_least(&player, 1)[0], ids[0]);

        player.process_command(PlayerCommand::SetRepeat(Repeat::Off));
        assert_eq!(player.playback_starts, 2, "restarted at the playhead");
        wait_for_lookahead(&player);
        assert!(player.timeline.queued_after_playhead().is_empty());
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn repeating_one_track_queues_it_again_and_each_pass_is_a_play() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = wavs_in(dir.path(), &["a", "b"], 1.0, Repeat::One);
        let (recorder, events) = history::PlayRecorder::capture();
        player.history = Some(recorder);
        // The first pass as a play would have it, now there is somewhere to
        // record it.
        player.in_flight = None;
        player.on_track_changed(ids[0], 0);

        assert_eq!(queued_at_least(&player, 2)[..2], [ids[0], ids[0]]);
        // Into the second pass: one second of 8 kHz mono and a little more.
        player
            .timeline
            .samples_played
            .store(8_400, Ordering::Relaxed);
        player.update_playback_state();

        let flight = player.in_flight.as_ref().unwrap();
        assert_eq!((flight.item, flight.boundary), (ids[0], 1));
        assert_eq!(player.shared_state.cursor(), Some(ids[0]));
        let events: Vec<_> = events.try_iter().collect();
        let started = events
            .iter()
            .filter(|e| matches!(e, PlayEvent::Started { track_id: 1, .. }))
            .count();
        assert_eq!(started, 2, "two plays: {events:?}");
        assert!(
            events.contains(&PlayEvent::Finished {
                track_id: 1,
                listened_ms: 1_000
            }),
            "the first pass banked whole: {events:?}"
        );
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn next_moves_on_from_a_track_repeating() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = wavs_in(dir.path(), &["a", "b"], 1.0, Repeat::One);

        player.process_command(PlayerCommand::NextTrack);
        assert_eq!(playing_id(&player), Some(ids[1]));
        player.process_command(PlayerCommand::NextTrack);
        assert_eq!(
            playing_id(&player),
            Some(ids[0]),
            "round from the last, as repeating does"
        );
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_repeat_of_nothing_heard_stops_rather_than_going_round() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, _) = wavs_in(dir.path(), &["a"], 1.0, Repeat::One);

        // Nothing plays through the stuck output, so the session that ends
        // has nothing heard of it: a file that decodes to nothing.
        player.process_command(PlayerCommand::DecodeFinished(player.session));
        assert!(matches!(player.transport, Transport::Idle));
    }

    // --- sleep timer ---

    fn session_run(player: &Player) -> Option<Run> {
        player.session().map(|s| s.run)
    }

    #[test]
    fn a_sleep_timer_fades_out_at_its_time_and_keeps_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = wavs_playing(dir.path(), &["a", "b"], 1.0);

        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::After {
            minutes: 30,
        })));
        let Some(Sleep::At { unix_ms }) = player.shared_state.sleep() else {
            panic!("published as a time");
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(unix_ms.abs_diff(now + 30 * 60_000) < 5_000);
        let at = player.sleep.and_then(|s| s.at).unwrap();
        assert!(player.next_wake().is_some_and(|w| w <= at), "woken for it");

        // Its time comes, the fade having run down to silence.
        player.sleep.as_mut().unwrap().at = Some(std::time::Instant::now());
        player.update_playback_state();
        assert_eq!(session_run(&player), Some(Run::Paused));
        assert!(
            !player.session().unwrap().engine().unwrap().is_running(),
            "stopped at silence: the fade did the fading"
        );
        assert!(
            player.sleep_fade.is_none(),
            "full level for what plays next"
        );
        assert_eq!(player.shared_state.sleep(), None, "spent");
        assert_eq!(playlist_ids(&player), ids, "the queue as it was");
        assert_eq!(player.shared_state.cursor(), Some(ids[0]));
        player.process_command(PlayerCommand::Stop);
    }

    fn fading_gain(player: &Player) -> Option<f32> {
        match player.sleep_fade {
            Some(sleep::SleepFade::Falling { gain, .. }) => Some(gain),
            _ => None,
        }
    }

    fn db(gain: f32) -> f32 {
        20.0 * gain.log10()
    }

    /// A 15-minute timer fades for 90 s: nothing before the deadline less
    /// that, then down evenly in decibels to silence at the deadline.
    #[test]
    fn a_sleep_timer_fades_over_the_run_up_to_its_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, _) = wavs_playing(dir.path(), &["a"], 1.0);
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::After {
            minutes: 15,
        })));
        let now = std::time::Instant::now();
        let secs = std::time::Duration::from_secs;

        player.sleep.as_mut().unwrap().at = Some(now + secs(91));
        player.update_playback_state();
        assert_eq!(fading_gain(&player), None, "not yet: 91 s out");
        assert!(!player.shared_state.sleep_fading());
        let wake = player.next_wake().unwrap();
        assert!(
            wake <= now + secs(2),
            "woken for the fade's start, a second away"
        );

        player.sleep.as_mut().unwrap().at = Some(now + secs(45));
        player.update_playback_state();
        let half = fading_gain(&player).expect("half way through the fade");
        assert!((db(half) + 30.0).abs() < 1.0, "{} dB", db(half));
        assert!(player.shared_state.sleep_fading(), "published as fading");
        assert!(player.next_wake().unwrap() <= std::time::Instant::now() + secs(1));

        player.sleep.as_mut().unwrap().at = Some(now + std::time::Duration::from_millis(900));
        player.update_playback_state();
        let late = fading_gain(&player).unwrap();
        assert!(
            db(late) < -59.0,
            "all but silent at the end: {} dB",
            db(late)
        );
        player.process_command(PlayerCommand::Stop);
    }

    /// Cancelled half way down, the level comes back up over a second.
    #[test]
    fn cancelling_a_sleep_timer_mid_fade_brings_the_level_back() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, _) = wavs_playing(dir.path(), &["a"], 1.0);
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::After {
            minutes: 15,
        })));
        player.sleep.as_mut().unwrap().at =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(45));
        player.update_playback_state();
        assert!(fading_gain(&player).is_some());

        player.process_command(PlayerCommand::SetSleepTimer(None));
        let Some(sleep::SleepFade::Restoring { from, .. }) = player.sleep_fade else {
            panic!("coming back up: {:?}", player.sleep_fade);
        };
        assert!((db(from) + 30.0).abs() < 1.0, "from where it had got to");
        assert!(!player.shared_state.sleep_fading());
        // A second on.
        if let Some(sleep::SleepFade::Restoring { since, .. }) = player.sleep_fade.as_mut() {
            *since -= std::time::Duration::from_secs(1);
        }
        player.update_playback_state();
        assert!(player.sleep_fade.is_none(), "at full level, and left alone");
        assert_eq!(session_run(&player), Some(Run::Playing));
        player.process_command(PlayerCommand::Stop);
    }

    /// A pause by hand during the fade is someone awake: the timer is off,
    /// and playing again is at full level.
    #[test]
    fn pausing_during_the_fade_cancels_the_timer() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, _) = wavs_playing(dir.path(), &["a"], 1.0);
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::After {
            minutes: 15,
        })));
        player.sleep.as_mut().unwrap().at =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(45));
        player.update_playback_state();
        assert!(fading_gain(&player).is_some());

        player.process_command(PlayerCommand::Pause);
        assert_eq!(player.shared_state.sleep(), None, "cancelled");
        assert!(player.sleep_fade.is_none());
        assert!(player.sleep_snap_on_resume);
        player.process_command(PlayerCommand::Resume);
        assert!(!player.sleep_snap_on_resume, "full level taken on resuming");
        player.process_command(PlayerCommand::Stop);
    }

    /// The end of a track fades over its last minute, or all of it if it is
    /// shorter, as these one-second tracks are; the end of a record only on
    /// its last track.
    #[test]
    fn the_end_of_a_track_or_record_fades_into_the_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, _) = queued_wavs(dir.path(), &["a", "b"]);
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::EndOfTrack)));
        player.update_playback_state();
        assert!(
            fading_gain(&player).is_some(),
            "a one-second track fades whole"
        );

        // Both tracks are on one record (none named), so the first is not
        // its end.
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::EndOfRecord)));
        if let Some(sleep::SleepFade::Restoring { since, .. }) = player.sleep_fade.as_mut() {
            *since -= std::time::Duration::from_secs(1);
        }
        player.update_playback_state();
        assert_eq!(fading_gain(&player), None, "the record goes on");
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_sleep_timer_going_off_while_paused_only_ends() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, _) = wavs_playing(dir.path(), &["a"], 1.0);
        player.process_command(PlayerCommand::Pause);
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::After {
            minutes: 1,
        })));
        player.sleep.as_mut().unwrap().at = Some(std::time::Instant::now());
        player.update_playback_state();
        assert_eq!(player.shared_state.sleep(), None);
        player.process_command(PlayerCommand::Resume);
        assert_eq!(
            session_run(&player),
            Some(Run::Playing),
            "nothing left to go off"
        );
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn cancelling_a_sleep_timer_leaves_playback_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, _) = wavs_playing(dir.path(), &["a"], 1.0);
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::After {
            minutes: 1,
        })));
        player.process_command(PlayerCommand::SetSleepTimer(None));
        assert_eq!(player.shared_state.sleep(), None);
        assert!(player.sleep.is_none());
        player.update_playback_state();
        assert_eq!(session_run(&player), Some(Run::Playing));
        player.process_command(PlayerCommand::Stop);
    }

    /// Gapless: the next track is already playing when the playhead crosses,
    /// and it is stopped there, not part way through the one before.
    #[test]
    fn a_sleep_timer_for_the_end_of_the_track_stops_at_the_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = queued_wavs(dir.path(), &["a", "b"]);
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::EndOfTrack)));
        assert_eq!(player.shared_state.sleep(), Some(Sleep::EndOfTrack));

        // Half way through the first: playing on.
        player
            .timeline
            .samples_played
            .store(4_000, Ordering::Relaxed);
        player.update_playback_state();
        assert_eq!(session_run(&player), Some(Run::Playing));
        assert_eq!(player.shared_state.sleep(), Some(Sleep::EndOfTrack));

        // Into the second.
        player
            .timeline
            .samples_played
            .store(8_400, Ordering::Relaxed);
        player.update_playback_state();
        assert_eq!(player.shared_state.cursor(), Some(ids[1]));
        assert_eq!(session_run(&player), Some(Run::Paused));
        assert!(
            !player.session().unwrap().engine().unwrap().is_running(),
            "stopped at once, nothing of the next track faded through"
        );
        assert_eq!(player.shared_state.sleep(), None);
        assert_eq!(playlist_ids(&player), ids);
        player.process_command(PlayerCommand::Stop);
    }

    /// A track repeating runs into itself, and that is the end of the track
    /// too.
    #[test]
    fn a_sleep_timer_for_the_end_of_the_track_stops_a_track_repeating() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = wavs_in(dir.path(), &["a", "b"], 1.0, Repeat::One);
        assert_eq!(queued_at_least(&player, 1)[0], ids[0]);
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::EndOfTrack)));
        player
            .timeline
            .samples_played
            .store(8_400, Ordering::Relaxed);
        player.update_playback_state();
        assert_eq!(player.shared_state.cursor(), Some(ids[0]));
        assert_eq!(session_run(&player), Some(Run::Paused));
        assert_eq!(player.shared_state.sleep(), None);
        player.process_command(PlayerCommand::Stop);
    }

    /// A track that cannot follow gaplessly ends the decode instead, and the
    /// next opens paused.
    #[test]
    fn a_sleep_timer_for_the_end_of_the_track_opens_the_next_paused() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = wavs_playing(dir.path(), &["a", "b"], 1.0);
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::EndOfTrack)));
        player.process_command(PlayerCommand::DecodeFinished(player.session));
        assert_eq!(player.shared_state.cursor(), Some(ids[1]));
        assert_eq!(player.intent(), Some(Run::Paused));
        assert_eq!(player.shared_state.sleep(), None);
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_sleep_timer_for_the_end_of_the_record_plays_the_record_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let items: Vec<_> = [("a", "Low"), ("b", "Low"), ("c", "Heroes")]
            .iter()
            .map(|(name, album)| {
                let path = dir.path().join(format!("{name}.wav"));
                crate::test_utils::generate_wav(&path, 8_000, 1, 1.0, 16);
                PlaylistItem {
                    path,
                    album: album.to_string(),
                    album_artist: "David Bowie".into(),
                    ..make_item(name)
                }
            })
            .collect();
        let ids: Vec<_> = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::Play(ids[0]));
        player.process_command(PlayerCommand::SetSleepTimer(Some(SleepTimer::EndOfRecord)));

        player.process_command(PlayerCommand::DecodeFinished(player.session));
        assert_eq!(player.shared_state.cursor(), Some(ids[1]));
        assert_eq!(player.intent(), Some(Run::Playing), "the same record");
        assert_eq!(player.shared_state.sleep(), Some(Sleep::EndOfRecord));

        player.process_command(PlayerCommand::DecodeFinished(player.session));
        assert_eq!(player.shared_state.cursor(), Some(ids[2]));
        assert_eq!(player.intent(), Some(Run::Paused), "the next record waits");
        assert_eq!(player.shared_state.sleep(), None);
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn shuffle_on_and_off_again_puts_the_queue_back() {
        let mut player = Player::new();
        let ids = seed(&mut player, 20);
        pretend_playing(&mut player, ids[3]);

        player.process_command(PlayerCommand::SetShuffle(true));
        let shuffled = playlist_ids(&player);
        assert!(player.shared_state.play_mode().shuffle);
        assert_eq!(
            shuffled[..4],
            ids[..4],
            "nothing up to the playing track moves"
        );
        assert_ne!(shuffled, ids);
        let mut sorted = shuffled.clone();
        sorted.sort_by_key(|id| ids.iter().position(|i| i == id));
        assert_eq!(sorted, ids, "the same items");

        let extra = make_item("extra");
        let extra_id = extra.id;
        player.process_command(PlayerCommand::InsertInPlaylist {
            items: vec![extra],
            after: ids[3],
        });
        player.process_command(PlayerCommand::RemoveFromPlaylist(ids[10]));
        player.process_command(PlayerCommand::SetShuffle(false));

        let mut expected = ids.clone();
        expected.remove(10);
        expected.insert(4, extra_id);
        assert_eq!(playlist_ids(&player), expected, "added since stays put");
        assert!(!player.shared_state.play_mode().shuffle);
        assert!(
            player
                .shared_state
                .shuffle_order()
                .iter()
                .all(|(_, pre)| pre.is_none())
        );
    }

    #[test]
    fn a_queue_replaced_while_shuffled_plays_shuffled_from_its_start() {
        let mut player = Player::new();
        seed(&mut player, 3);
        player.process_command(PlayerCommand::SetShuffle(true));

        let items: Vec<_> = (0..20).map(|i| make_item(&format!("n{i}"))).collect();
        let given: Vec<_> = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::ReplacePlaylist {
            items,
            start: 5,
            position_ms: 0,
            play: false,
        });
        let shuffled = playlist_ids(&player);
        assert_eq!(shuffled[0], given[5], "the start first");
        assert_eq!(player.shared_state.cursor(), Some(given[5]));
        assert_ne!(shuffled, given);

        player.process_command(PlayerCommand::SetShuffle(false));
        assert_eq!(
            playlist_ids(&player),
            given,
            "off gives the queue as it came"
        );
    }

    #[test]
    fn a_queue_added_to_an_empty_one_while_shuffled_plays_shuffled() {
        let mut player = Player::new();
        player.process_command(PlayerCommand::SetShuffle(true));
        let given = seed(&mut player, 20);
        assert_eq!(playlist_ids(&player)[0], given[0]);
        assert_ne!(playlist_ids(&player), given);

        player.process_command(PlayerCommand::SetShuffle(false));
        assert_eq!(playlist_ids(&player), given);
    }

    #[test]
    fn shuffle_is_one_undo_step() {
        let mut player = Player::new();
        let ids = seed(&mut player, 10);
        pretend_playing(&mut player, ids[0]);

        player.process_command(PlayerCommand::SetShuffle(true));
        let shuffled = playlist_ids(&player);
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_ids(&player), ids);
        assert!(!player.shared_state.play_mode().shuffle);

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_ids(&player), shuffled);
        assert!(player.shared_state.play_mode().shuffle);
        player.process_command(PlayerCommand::SetShuffle(false));
        assert_eq!(
            playlist_ids(&player),
            ids,
            "the redone shuffle still unwinds"
        );
    }

    #[test]
    fn removing_the_last_track_playing_carries_on_from_the_top_when_repeating() {
        let mut player = Player::new();
        let ids = seed(&mut player, 3);
        player.process_command(PlayerCommand::SetRepeat(Repeat::Queue));
        player.shared_state.set_cursor(Some(ids[2]));

        player.process_command(PlayerCommand::RemoveFromPlaylist(ids[2]));
        assert_eq!(player.shared_state.cursor(), Some(ids[0]));
    }

    #[test]
    fn removing_a_track_the_decoder_queued_takes_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = queued_wavs(dir.path(), &["a", "b", "c"]);
        assert_eq!(player.timeline.queued_after_playhead(), ids[1..]);

        player.process_command(PlayerCommand::RemoveFromPlaylist(ids[1]));
        assert_eq!(player.playback_starts, 2, "restarted at the playhead");
        assert_eq!(player.shared_state.cursor(), Some(ids[0]));
        await_queued(&player);
        await_queued(&player);
        assert_eq!(player.timeline.queued_after_playhead(), vec![ids[2]]);
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_track_inserted_to_play_next_is_not_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = queued_wavs(dir.path(), &["a", "b"]);
        let path = dir.path().join("next.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 1.0, 16);
        let next = PlaylistItem {
            path,
            ..make_item("next")
        };
        let next_id = next.id;

        player.process_command(PlayerCommand::InsertInPlaylist {
            items: vec![next],
            after: ids[0],
        });
        assert_eq!(player.playback_starts, 2);
        for _ in 0..3 {
            await_queued(&player);
        }
        assert_eq!(
            player.timeline.queued_after_playhead(),
            vec![next_id, ids[1]]
        );
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_track_the_decoder_could_not_open_does_not_make_every_edit_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let wav = |name: &str| {
            let path = dir.path().join(format!("{name}.wav"));
            crate::test_utils::generate_wav(&path, 8_000, 1, 30.0, 16);
            PlaylistItem {
                path,
                ..make_item(name)
            }
        };
        // Ready, but with no file behind it: the decoder skips it.
        let items = vec![wav("a"), make_item("missing"), wav("c")];
        let ids: Vec<_> = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::Play(ids[0]));
        await_queued(&player);
        await_queued(&player);

        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("later")]));
        assert_eq!(
            player.playback_starts, 1,
            "nothing the decoder decided changed"
        );
        assert_eq!(player.timeline.queued_after_playhead(), vec![ids[2]]);
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn a_track_added_after_the_decoder_reached_the_end_follows_gaplessly() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = queued_wavs(dir.path(), &["a"]);
        wait_for_lookahead(&player);
        let path = dir.path().join("b.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 1.0, 16);
        let b = PlaylistItem {
            path,
            ..make_item("b")
        };
        let b_id = b.id;

        player.process_command(PlayerCommand::AddToPlaylist(vec![b]));
        assert_eq!(player.playback_starts, 2, "restarted to queue it");
        assert_eq!(player.shared_state.cursor(), Some(ids[0]));
        await_queued(&player);
        await_queued(&player);
        assert_eq!(player.timeline.queued_after_playhead(), vec![b_id]);
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn playing_next_a_track_still_downloading_takes_back_the_lookahead() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = queued_wavs(dir.path(), &["a", "b"]);
        let remote = pending_item("remote");
        let remote_id = remote.id;

        player.process_command(PlayerCommand::InsertInPlaylist {
            items: vec![remote],
            after: ids[0],
        });
        assert_eq!(player.playback_starts, 2, "b is taken back out of the ring");
        await_queued(&player);
        wait_for_lookahead(&player);
        let session = player.session().unwrap();
        let step = session.lookahead.lock().last().cloned().unwrap();
        assert_eq!(step.next, Some(remote_id), "the decoder waits for it");
        assert!(player.timeline.queued_after_playhead().is_empty());
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn gapless_playback_waits_for_a_track_still_downloading() {
        let dir = tempfile::tempdir().unwrap();
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let wav = |name: &str| {
            let path = dir.path().join(format!("{name}.wav"));
            crate::test_utils::generate_wav(&path, 8_000, 1, 1.0, 16);
            PlaylistItem {
                path,
                ..make_item(name)
            }
        };
        let items = vec![wav("a"), pending_item("arriving"), wav("c")];
        let ids: Vec<_> = items.iter().map(|i| i.id).collect();
        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::Play(ids[0]));
        await_queued(&player);
        wait_for_lookahead(&player);

        assert!(
            player.timeline.queued_after_playhead().is_empty(),
            "c is not queued over the track before it"
        );
        // The session drains and ends; the advance parks on the track.
        player.process_command(PlayerCommand::DecodeFinished(player.session));
        assert_eq!(player.shared_state.cursor(), Some(ids[1]));
        assert!(player.waiting().is_some());
        player.process_command(PlayerCommand::Stop);
    }

    /// Until the decoder has taken a step past the playing track.
    fn wait_for_lookahead(player: &Player) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while player
            .session()
            .is_some_and(|s| s.lookahead.lock().is_empty())
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the decoder never looked ahead"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn an_edit_after_what_the_decoder_queued_leaves_playback_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, ids) = wavs_playing(dir.path(), &["a", "b"], 30.0);
        await_queued(&player);
        await_queued(&player);
        let path = dir.path().join("later.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 1.0, 16);

        player.process_command(PlayerCommand::AddToPlaylist(vec![PlaylistItem {
            path,
            ..make_item("later")
        }]));
        assert_eq!(player.playback_starts, 1);
        assert_eq!(player.timeline.queued_after_playhead(), vec![ids[1]]);
        player.process_command(PlayerCommand::Stop);
    }

    #[test]
    fn undoing_the_add_of_a_track_on_its_way_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mut player, _, starts) = downloading_wav(dir.path());
        let path = dir.path().join("added.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 1.0, 16);
        let added = PlaylistItem {
            path,
            state: ItemState::Pending,
            ..make_item("added")
        };
        let added_id = added.id;
        player.process_command(PlayerCommand::AddToPlaylist(vec![added]));
        player.process_command(PlayerCommand::Play(added_id));

        player.process_command(PlayerCommand::Undo);
        assert!(player.waiting().is_none());
        assert_eq!(player.shared_state.playback_state(), PlaybackState::Stopped);

        // Its download lands anyway; nothing asked for it any more.
        player.process_command(PlayerCommand::Redo);
        player
            .shared_state
            .update_item_state(added_id, ItemState::Ready);
        player.process_command(PlayerCommand::TrackReady(added_id));
        assert!(player.session().is_none());
        assert_eq!(starts.load(Ordering::Relaxed), 0);
    }

    /// xorshift64: enough randomness to drive the player, reproducible from
    /// its seed, and no dependency for it.
    pub(super) struct Rng(pub(super) u64);

    impl Rng {
        pub(super) fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        pub(super) fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }

        pub(super) fn coin(&mut self) -> bool {
            self.next() & 1 == 1
        }
    }

    /// Commands a listener can send that ask for sound. Anything else may only
    /// keep playing what was already playing or on its way to.
    pub(super) fn asks_to_play(cmd: &PlayerCommand) -> bool {
        cmd.asks_to_play()
    }

    /// Check the invariants #679 sets out, as far as they can be seen from
    /// outside the audio thread.
    pub(super) fn check_invariants(
        player: &Player,
        wanted_before: bool,
        asked: bool,
    ) -> Result<(), String> {
        let state = &player.shared_state;
        let ids = playlist_ids(player);
        let listed = |id: QueueItemId| ids.contains(&id);
        let mut seen = std::collections::HashSet::new();
        if !ids.iter().all(|id| seen.insert(*id)) {
            return Err("an item is in the playlist twice".into());
        }
        let cursor = state.cursor();
        if cursor.is_some_and(|c| !listed(c)) {
            return Err("the cursor is on an item not in the playlist".into());
        }
        let info = state.track_info();
        match (&player.transport, &info) {
            (Transport::Loaded(_), None) => return Err("loaded, but no track published".into()),
            (Transport::Loaded(_), Some(info)) => {
                if !listed(info.id) {
                    return Err("playing an item not in the playlist".into());
                }
                if cursor != Some(info.id) {
                    return Err("the cursor is not on what is playing".into());
                }
                if !matches!(
                    state.playback_state(),
                    PlaybackState::Playing | PlaybackState::Paused
                ) {
                    return Err("loaded, but published as stopped".into());
                }
            }
            (_, Some(_)) => return Err("a track is published with nothing loaded".into()),
            (Transport::Waiting(w), None) => {
                if cursor != Some(w.id) || !listed(w.id) {
                    return Err("waiting on an item that is not the cursor's".into());
                }
                let expected = match w.start {
                    Run::Playing => PlaybackState::Stopped,
                    Run::Paused => PlaybackState::Paused,
                };
                if state.playback_state() != expected {
                    return Err("a wait published in the wrong state".into());
                }
            }
            (Transport::Idle, None) => {
                if state.playback_state() != PlaybackState::Stopped {
                    return Err("idle, but not published as stopped".into());
                }
            }
        }
        if state.is_waiting() != player.waiting().is_some() {
            return Err("the published wait disagrees with the player".into());
        }
        if player
            .timeline
            .queued_after_playhead()
            .into_iter()
            .any(|id| !listed(id))
        {
            return Err("the decoder has queued an item no longer in the playlist".into());
        }
        if !asked && !wanted_before && state.wants_to_play() {
            return Err("started without being asked".into());
        }
        if state.play_mode() != player.mode {
            return Err("the published mode disagrees with the player".into());
        }
        if !player.mode.shuffle && state.shuffle_order().iter().any(|(_, pre)| pre.is_some()) {
            return Err("shuffle is off, but an item remembers a place to go back to".into());
        }
        if player.mode.repeat == Repeat::Off
            && let Some(session) = player.session()
            && let Some(playhead) = player.timeline.playhead()
            && session.track.id == playhead.id
            && session
                .lookahead
                .lock()
                .iter()
                .filter(|step| step.boundary > playhead.boundary)
                .any(|step| step.wrapped || step.next == Some(step.after))
        {
            return Err("repeat is off, but the decoder has queued a wrap".into());
        }
        Ok(())
    }

    #[test]
    fn random_use_keeps_the_player_honest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 0.5, 16);

        for seed in 1..=40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut player = Player::new();
            player.backend = Box::new(StuckBackend {
                rate: 8_000.0,
                asked: Default::default(),
                starts: Default::default(),
            });
            let fresh = |rng: &mut Rng| {
                let state = match rng.below(3) {
                    0 => ItemState::Pending,
                    _ => ItemState::Ready,
                };
                PlaylistItem {
                    path: path.clone(),
                    state,
                    ..make_item("t")
                }
            };
            let start: Vec<_> = (0..5).map(|_| fresh(&mut rng)).collect();
            player.process_command(PlayerCommand::AddToPlaylist(start));

            let mut history = Vec::new();
            for step in 0..150 {
                let ids = playlist_ids(&player);
                let pick = |rng: &mut Rng| ids.get(rng.below(ids.len())).copied();
                let cmd = match rng.below(23) {
                    0 => pick(&mut rng).map(PlayerCommand::Play),
                    1 => pick(&mut rng).map(|id| PlayerCommand::Cue {
                        id,
                        position_ms: if rng.coin() { 0 } else { 200 },
                        play: rng.coin(),
                    }),
                    2 => Some(PlayerCommand::Pause),
                    3 => Some(PlayerCommand::Resume),
                    4 => Some(PlayerCommand::NextTrack),
                    5 => Some(PlayerCommand::PrevTrack),
                    6 => Some(PlayerCommand::Seek(100)),
                    7 => pick(&mut rng).map(PlayerCommand::RemoveFromPlaylist),
                    8 => Some(PlayerCommand::RemoveFromPlaylistBatch(
                        (0..2).filter_map(|_| pick(&mut rng)).collect(),
                    )),
                    9 => pick(&mut rng).zip(pick(&mut rng)).map(|(id, target)| {
                        PlayerCommand::MoveInPlaylist {
                            id,
                            target,
                            after: rng.coin(),
                        }
                    }),
                    10 => pick(&mut rng).map(|after| PlayerCommand::InsertInPlaylist {
                        items: vec![fresh(&mut rng)],
                        after,
                    }),
                    11 => Some(PlayerCommand::AddToPlaylist(vec![fresh(&mut rng)])),
                    12 => Some(PlayerCommand::Undo),
                    13 => Some(PlayerCommand::Redo),
                    14 | 15 => {
                        let (items, _) = player.shared_state.snapshot_playlist();
                        let pending: Vec<_> = items
                            .iter()
                            .filter(|i| matches!(i.state, ItemState::Pending))
                            .map(|i| i.id)
                            .collect();
                        pending.get(rng.below(pending.len())).map(|&id| {
                            if rng.below(4) == 0 {
                                player
                                    .shared_state
                                    .update_item_state(id, ItemState::Failed("gone".into()));
                                PlayerCommand::TrackFailed(id)
                            } else {
                                player.shared_state.update_item_state(id, ItemState::Ready);
                                PlayerCommand::TrackReady(id)
                            }
                        })
                    }
                    16 => Some(PlayerCommand::DecodeFinished(player.session)),
                    17 => Some(PlayerCommand::DecodeFinished(
                        player.session.wrapping_sub(1),
                    )),
                    18 => Some(PlayerCommand::ClearPlaylist),
                    20 => Some(PlayerCommand::SetShuffle(rng.coin())),
                    21 => Some(PlayerCommand::SetRepeat(
                        [Repeat::Off, Repeat::Queue, Repeat::One][rng.below(3)],
                    )),
                    _ => Some(PlayerCommand::ReplacePlaylist {
                        items: (0..3).map(|_| fresh(&mut rng)).collect(),
                        start: rng.below(4),
                        position_ms: if rng.coin() { 0 } else { 200 },
                        play: rng.coin(),
                    }),
                };
                let Some(cmd) = cmd else { continue };
                let label = format!("{cmd:?}");
                let asked = asks_to_play(&cmd);
                let replaced_shuffled = player.mode.shuffle
                    && matches!(&cmd, PlayerCommand::ReplacePlaylist { items, .. } if items.len() > 1);
                let wanted_before = player.shared_state.wants_to_play();
                player.process_command(cmd);
                // What the decode threads sent meanwhile, as the loop would
                // see it. A full channel would stall a decode thread on its
                // way out.
                while let Ok(sent) = player.commands.rx.try_recv() {
                    player.process_command(sent);
                }
                player.update_playback_state();
                history.push(label);
                let broken = check_invariants(&player, wanted_before, asked)
                    .err()
                    .or_else(|| {
                        (replaced_shuffled
                            && player
                                .shared_state
                                .shuffle_order()
                                .iter()
                                .all(|(_, pre)| pre.is_none()))
                        .then(|| "a queue replaced while shuffled plays in order".to_string())
                    });
                if let Some(broken) = broken {
                    let tail = history[history.len().saturating_sub(8)..].join("\n  ");
                    panic!("seed {seed}, step {step}: {broken}\nlast commands:\n  {tail}");
                }
            }
            player.process_command(PlayerCommand::Stop);
        }
    }

    struct QuickEngine {
        silent: Arc<std::sync::atomic::AtomicBool>,
        quick_outs: Arc<std::sync::atomic::AtomicUsize>,
        outs: Arc<std::sync::atomic::AtomicUsize>,
        period: std::time::Duration,
    }
    impl AudioEngineHandle for QuickEngine {
        fn start(&self) -> Result<(), BackendError> {
            Ok(())
        }
        fn stop(&self) -> Result<(), BackendError> {
            Ok(())
        }
        fn is_running(&self) -> bool {
            true
        }
        fn fade_out(&self) {
            self.outs.fetch_add(1, Ordering::Relaxed);
        }
        fn fade_out_quickly(&self) {
            self.quick_outs.fetch_add(1, Ordering::Relaxed);
        }
        fn fade_in(&self) -> Result<(), BackendError> {
            Ok(())
        }
        fn is_silent(&self) -> bool {
            self.silent.load(Ordering::Relaxed)
        }
        fn period(&self) -> std::time::Duration {
            self.period
        }
    }

    /// A player playing through a `QuickEngine` with callbacks of `frames` at
    /// `rate`, and the engine's silence and fade counts.
    fn quick_player(
        frames: u32,
        rate: u32,
    ) -> (
        Player,
        Arc<std::sync::atomic::AtomicBool>,
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let silent = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let quick_outs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let outs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut player = Player::new();
        player.transport = Transport::Loaded(test_session(
            QueueItemId::new(),
            Box::new(QuickEngine {
                silent: silent.clone(),
                quick_outs: quick_outs.clone(),
                outs: outs.clone(),
                period: std::time::Duration::from_secs_f64(frames as f64 / rate as f64),
            }),
        ));
        (player, silent, quick_outs, outs)
    }

    /// A DSP change while playing fades the output out quickly and restarts
    /// only once that is silent; changes made meanwhile join it, so a run of
    /// them is one dip and one restart.
    #[test]
    fn a_dsp_change_restarts_after_a_quick_fade_and_coalesces() {
        let (mut player, silent, quick_outs, outs) = quick_player(512, 44100);

        player.restart_for_dsp();
        assert_eq!(quick_outs.load(Ordering::Relaxed), 1);
        assert_eq!(outs.load(Ordering::Relaxed), 0, "not the pause's fade");

        // Another change mid-fade: the same dip.
        player.restart_for_dsp();
        assert_eq!(quick_outs.load(Ordering::Relaxed), 1);
        player.update_playback_state();
        assert!(player.dsp_restart.is_some(), "not restarted while audible");

        silent.store(true, Ordering::Relaxed);
        player.update_playback_state();
        assert!(player.dsp_restart.is_none(), "restarted once silent");
        assert!(
            !player.quick_start,
            "the quick start is the restart's alone"
        );
    }

    /// The fade can begin up to one callback after it is asked for, so the
    /// first wake allows a period for that; a callback later still is waited
    /// for one period more, well inside the limit, for the buffers a Mac and
    /// PipeWire use by default.
    #[test]
    fn a_dsp_change_wakes_a_buffer_period_after_its_fade() {
        use crate::audio::fade::QUICK_FADE;
        for (frames, rate) in [(512, 44100), (1024, 48000)] {
            let period = std::time::Duration::from_secs_f64(frames as f64 / rate as f64);
            let (mut player, silent, _, _) = quick_player(frames, rate);
            player.restart_for_dsp();
            let since = player.dsp_restart.expect("waiting on the fade");

            let first = player.next_event().expect("a wake when the fade ends");
            assert_eq!(first, since + QUICK_FADE + period + BOUNDARY_SLACK);
            assert_eq!(dsp_wake(since, period, since), first);

            // Woken then, still sounding: once more, a period on, inside the
            // limit; past that, the limit.
            player.update_playback_state();
            assert!(player.dsp_restart.is_some());
            let second = dsp_wake(since, period, first);
            assert_eq!(second, first + period);
            assert!(
                second < since + QUICK_FADE_LIMIT,
                "{frames} frames: inside the limit"
            );
            assert_eq!(dsp_wake(since, period, second), since + QUICK_FADE_LIMIT);

            silent.store(true, Ordering::Relaxed);
            player.update_playback_state();
            assert!(
                player.dsp_restart.is_none(),
                "{frames} frames: restarted at silence"
            );
        }
    }

    #[test]
    fn a_pause_reports_where_the_fade_went_silent() {
        use std::sync::atomic::AtomicBool;

        struct FadingEngine {
            running: Arc<AtomicBool>,
            silent: Arc<AtomicBool>,
        }
        impl AudioEngineHandle for FadingEngine {
            fn start(&self) -> Result<(), BackendError> {
                self.running.store(true, Ordering::Relaxed);
                Ok(())
            }
            fn stop(&self) -> Result<(), BackendError> {
                self.running.store(false, Ordering::Relaxed);
                Ok(())
            }
            fn is_running(&self) -> bool {
                self.running.load(Ordering::Relaxed)
            }
            fn fade_out(&self) {}
            fn fade_in(&self) -> Result<(), BackendError> {
                Ok(())
            }
            fn is_silent(&self) -> bool {
                self.silent.load(Ordering::Relaxed)
            }
        }

        let running = Arc::new(AtomicBool::new(true));
        let silent = Arc::new(AtomicBool::new(false));
        let mut player = Player::new();
        player.transport = Transport::Loaded(test_session(
            QueueItemId::new(),
            Box::new(FadingEngine {
                running: running.clone(),
                silent: silent.clone(),
            }),
        ));
        player
            .shared_state
            .set_playback_state(PlaybackState::Playing);
        player.shared_state.set_position_ms(5_000);

        let (reply, answer) = crossbeam_channel::bounded(1);
        player.process_command(PlayerCommand::PauseAndReport(reply));

        if crate::config::Config::cached().playback.fade_on_pause {
            assert!(answer.try_recv().is_err(), "not while the fade is audible");
            // The fade plays on, and the playhead with it.
            player.shared_state.set_position_ms(5_150);
            player.update_playback_state();
            assert!(answer.try_recv().is_err());
            silent.store(true, Ordering::Relaxed);
            player.update_playback_state();
            assert!(!running.load(Ordering::Relaxed));
            assert_eq!(answer.try_recv().unwrap(), 5_150);
        } else {
            assert_eq!(answer.try_recv().unwrap(), 5_000);
        }
    }

    #[test]
    fn the_server_hears_each_turn_playback_takes() {
        use PlaybackReportState::{Paused, Playing, Stopped};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.wav");
        crate::test_utils::generate_wav(&path, 8_000, 1, 10.0, 16);

        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: 8_000.0,
            asked: Default::default(),
            starts: Default::default(),
        });
        let (recorder, events) = PlayRecorder::capture();
        player.history = Some(recorder);

        let item = PlaylistItem {
            db_id: Some(5),
            path,
            ..make_item("t")
        };
        let id = item.id;
        player.process_command(PlayerCommand::AddToPlaylist(vec![item]));
        player.process_command(PlayerCommand::Play(id));
        player.process_command(PlayerCommand::Pause);
        player.process_command(PlayerCommand::Seek(4_000));
        // Once the decoder has queued the track, the playhead reads the start
        // of the packet holding 4 s, which can be a little before it; until
        // then it reads the 4 s asked for. Waiting makes the read the same on
        // every run.
        await_queued(&player);
        player.process_command(PlayerCommand::Resume);
        player.process_command(PlayerCommand::Stop);

        let events: Vec<_> = events.try_iter().collect();
        let near_seek = |position_ms: u64| (3_750..=4_000).contains(&position_ms);
        let reports: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                PlayEvent::Playback(r) => Some((r.track_id, r.state, r.position_ms)),
                _ => None,
            })
            .collect();
        assert_eq!(
            events.first(),
            Some(&PlayEvent::Started {
                track_id: 5,
                position_ms: 0
            })
        );
        assert_eq!(reports.len(), 4, "{events:?}");
        assert_eq!(reports[0], (5, Paused, 0));
        for (report, state) in reports[1..].iter().zip([Paused, Playing, Stopped]) {
            assert_eq!((report.0, report.1), (5, state), "{events:?}");
            assert!(near_seek(report.2), "{events:?}");
        }
        assert_eq!(
            events.last(),
            Some(&PlayEvent::Finished {
                track_id: 5,
                listened_ms: 0
            })
        );
    }

    #[test]
    fn removing_the_playing_track_resumes_at_its_successor() {
        let mut player = Player::new();
        let ids = seed(&mut player, 5);
        pretend_playing(&mut player, ids[2]);

        player.process_command(PlayerCommand::RemoveFromPlaylist(ids[2]));

        assert_eq!(
            player.shared_state.cursor(),
            Some(ids[3]),
            "playback must continue at the next track, not restart the queue"
        );
        assert_eq!(player.playback_starts, 1);
    }

    #[test]
    fn removing_the_paused_track_moves_on_paused() {
        let mut player = Player::new();
        let ids = seed(&mut player, 3);
        pretend_playing(&mut player, ids[1]);
        if let Transport::Loaded(session) = &mut player.transport {
            session.run = Run::Paused;
        }

        player.process_command(PlayerCommand::RemoveFromPlaylist(ids[1]));
        assert_eq!(player.shared_state.cursor(), Some(ids[2]));
        assert!(!player.shared_state.wants_to_play(), "still paused");
    }

    #[test]
    fn removing_the_cursor_with_nothing_loaded_starts_nothing() {
        let mut player = Player::new();
        let ids = seed(&mut player, 3);
        player.shared_state.set_cursor(Some(ids[1]));

        player.process_command(PlayerCommand::RemoveFromPlaylist(ids[1]));
        assert_eq!(player.shared_state.cursor(), Some(ids[2]));
        assert_eq!(player.playback_starts, 0);
    }

    #[test]
    fn removing_the_first_playing_track_resumes_at_the_new_first() {
        let mut player = Player::new();
        let ids = seed(&mut player, 3);
        pretend_playing(&mut player, ids[0]);

        player.process_command(PlayerCommand::RemoveFromPlaylist(ids[0]));

        assert_eq!(player.shared_state.cursor(), Some(ids[1]));
    }

    #[test]
    fn next_track_parks_on_a_track_that_has_not_downloaded_yet() {
        let mut player = Player::new();
        let playing = make_item("playing");
        let waiting = pending_item("waiting");
        let later = make_item("later");
        let (playing_id, waiting_id) = (playing.id, waiting.id);
        player.process_command(PlayerCommand::AddToPlaylist(vec![playing, waiting, later]));
        pretend_playing(&mut player, playing_id);

        player.process_command(PlayerCommand::DecodeFinished(player.session));

        assert_eq!(
            player.shared_state.cursor(),
            Some(waiting_id),
            "the cursor parks on the track being fetched"
        );
        assert_eq!(
            player.playback_starts, 0,
            "nothing to play until its bytes land"
        );

        // The download completes. Because the cursor is parked here, the
        // TrackReady actually reaches the player and the queue resumes.
        player
            .shared_state
            .update_item_state(waiting_id, ItemState::Ready);
        player.process_command(PlayerCommand::TrackReady(waiting_id));

        assert_eq!(player.playback_starts, 1);
        assert_eq!(player.shared_state.cursor(), Some(waiting_id));
    }

    /// Play pressed on an album whose first track is still on the server and
    /// whose next two are cached: the first is waited for and played first,
    /// not passed over for the first that happens to be on disk.
    #[test]
    fn an_album_starts_at_its_first_track_however_much_is_cached() {
        let mut player = Player::new();
        let remote = pending_item("remote");
        let cached = [make_item("cached 2"), make_item("cached 3")];
        let first = remote.id;
        player.process_command(PlayerCommand::ReplacePlaylist {
            items: [vec![remote], cached.to_vec()].concat(),
            start: 0,
            position_ms: 0,
            play: true,
        });

        assert_eq!(player.shared_state.cursor(), Some(first));
        assert_eq!(player.playback_starts, 0, "nothing decodes before it lands");

        player
            .shared_state
            .update_item_state(first, ItemState::Ready);
        player.process_command(PlayerCommand::TrackReady(first));
        assert_eq!(player.playback_starts, 1);
        assert_eq!(
            player.shared_state.cursor(),
            Some(first),
            "the first track decodes first"
        );
    }

    /// A stream opened ahead of its download that gives out with nothing
    /// heard has not played: the track is waited for, not skipped.
    #[test]
    fn a_stream_that_ends_unheard_mid_download_waits_for_its_track() {
        let mut player = Player::new();
        let streaming = pending_item("streaming");
        let next = make_item("next");
        let id = streaming.id;
        player.process_command(PlayerCommand::AddToPlaylist(vec![streaming, next]));
        pretend_playing(&mut player, id);

        player.process_command(PlayerCommand::DecodeFinished(player.session));
        assert_eq!(player.shared_state.cursor(), Some(id), "not passed over");
        assert_eq!(player.playback_starts, 0);

        player.shared_state.update_item_state(id, ItemState::Ready);
        player.process_command(PlayerCommand::TrackReady(id));
        assert_eq!(player.playback_starts, 1);
        assert_eq!(player.shared_state.cursor(), Some(id));
    }

    #[test]
    fn a_download_that_cannot_land_moves_the_cursor_on() {
        let mut player = Player::new();
        let waiting = pending_item("waiting");
        let later = make_item("later");
        let (waiting_id, later_id) = (waiting.id, later.id);
        player.process_command(PlayerCommand::AddToPlaylist(vec![waiting, later]));

        player.process_command(PlayerCommand::Play(waiting_id));
        assert_eq!(player.playback_starts, 0, "nothing to play yet");

        // The download gives up. Ready will never come.
        player
            .shared_state
            .update_item_state(waiting_id, ItemState::Failed("remote unavailable".into()));
        player.process_command(PlayerCommand::TrackFailed(waiting_id));

        assert_eq!(
            player.shared_state.cursor(),
            Some(later_id),
            "the queue moves past a track that can never load"
        );
        assert_eq!(player.playback_starts, 1);
    }

    #[test]
    fn a_queue_that_can_never_load_stops_rather_than_waiting() {
        let mut player = Player::new();
        let first = pending_item("first");
        let second = pending_item("second");
        let (first_id, second_id) = (first.id, second.id);
        player.process_command(PlayerCommand::AddToPlaylist(vec![first, second]));

        player.process_command(PlayerCommand::Play(first_id));
        for id in [first_id, second_id] {
            player
                .shared_state
                .update_item_state(id, ItemState::Failed("remote unavailable".into()));
            player.process_command(PlayerCommand::TrackFailed(id));
        }

        assert_eq!(player.playback_starts, 0);
        assert_eq!(
            player.shared_state.playback_state(),
            PlaybackState::Stopped,
            "a stop the UI can see, not an indefinite wait for TrackReady"
        );
    }

    #[test]
    fn a_failure_elsewhere_in_the_queue_leaves_the_cursor_alone() {
        let mut player = Player::new();
        let waiting = pending_item("waiting");
        let other = pending_item("other");
        let (waiting_id, other_id) = (waiting.id, other.id);
        player.process_command(PlayerCommand::AddToPlaylist(vec![waiting, other]));
        player.process_command(PlayerCommand::Play(waiting_id));

        player
            .shared_state
            .update_item_state(other_id, ItemState::Failed("remote unavailable".into()));
        player.process_command(PlayerCommand::TrackFailed(other_id));

        assert_eq!(
            player.shared_state.cursor(),
            Some(waiting_id),
            "a track still downloading keeps the cursor"
        );
    }

    #[test]
    fn batch_delete_containing_the_cursor_restarts_the_engine_once() {
        let mut player = Player::new();
        let ids = seed(&mut player, 5);
        pretend_playing(&mut player, ids[2]);

        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![
            ids[1], ids[2], ids[3],
        ]));

        assert_eq!(playlist_titles(&player), vec!["t0", "t4"]);
        assert_eq!(player.shared_state.cursor(), Some(ids[4]));
        assert_eq!(
            player.playback_starts, 1,
            "one resume for the whole selection, not one per deleted track"
        );
    }

    #[test]
    fn batch_delete_below_the_cursor_leaves_playback_alone() {
        let mut player = Player::new();
        let ids = seed(&mut player, 4);
        player.shared_state.set_cursor(Some(ids[0]));

        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![ids[2], ids[3]]));

        assert_eq!(player.shared_state.cursor(), Some(ids[0]));
        assert_eq!(player.playback_starts, 0);
    }

    #[test]
    fn undo_of_a_batch_delete_restores_the_original_order() {
        // The TUI collects a selection from a HashSet, so the IDs arrive in
        // arbitrary order — scrambled here so a snapshot that trusts that order
        // re-inserts C before B and lands it at the end of the playlist.
        let mut player = Player::new();
        let items = vec![
            make_item("A"),
            make_item("B"),
            make_item("C"),
            make_item("D"),
        ];
        let (b_id, c_id) = (items[1].id, items[2].id);
        player.process_command(PlayerCommand::AddToPlaylist(items));

        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![c_id, b_id]));
        assert_eq!(playlist_titles(&player), vec!["A", "D"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C", "D"]);
    }

    // --- AddToPlaylist undo/redo ---

    #[test]
    fn undo_add_removes_items() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B")];
        let ids: Vec<_> = items.iter().map(|i| i.id).collect();

        player.process_command(PlayerCommand::AddToPlaylist(items));
        assert_eq!(playlist_ids(&player), ids);
        assert!(player.undo_stack().can_undo());

        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());
        assert!(player.undo_stack().can_redo());
    }

    #[test]
    fn redo_add_restores_items() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B")];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_titles(&player), vec!["A", "B"]);
    }

    // --- RemoveFromPlaylist undo/redo ---

    #[test]
    fn undo_remove_restores_item_at_position() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let b_id = items[1].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylist(b_id));
        assert_eq!(playlist_titles(&player), vec!["A", "C"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);
    }

    #[test]
    fn undo_remove_first_item() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B")];
        let a_id = items[0].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylist(a_id));
        assert_eq!(playlist_titles(&player), vec!["B"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B"]);
    }

    #[test]
    fn undo_batch_remove_restores_all() {
        let mut player = Player::new();
        let items = vec![
            make_item("A"),
            make_item("B"),
            make_item("C"),
            make_item("D"),
        ];
        let b_id = items[1].id;
        let c_id = items[2].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        let version_before = player.shared_state.playlist_version();
        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![b_id, c_id]));
        assert_eq!(playlist_titles(&player), vec!["A", "D"]);
        // One bump for the whole batch. Bumping per item is what made clearing
        // a large queue crawl, and every bump wakes every client watching.
        assert_eq!(
            player.shared_state.playlist_version(),
            version_before + 1,
            "batch removal must bump the playlist version exactly once"
        );

        // Single undo restores both
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C", "D"]);
    }

    #[test]
    fn redo_batch_remove() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let a_id = items[0].id;
        let b_id = items[1].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylistBatch(vec![a_id, b_id]));
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_titles(&player), vec!["C"]);
    }

    #[test]
    fn redo_remove() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let b_id = items[1].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylist(b_id));
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_titles(&player), vec!["A", "C"]);
    }

    // --- InsertInPlaylist undo/redo ---

    #[test]
    fn undo_insert_removes_inserted_items() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("C")];
        let a_id = items[0].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));

        let inserted = vec![make_item("B")];
        player.process_command(PlayerCommand::InsertInPlaylist {
            items: inserted,
            after: a_id,
        });
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "C"]);
    }

    // --- MoveInPlaylist undo/redo ---

    #[test]
    fn undo_move_restores_position() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let a_id = items[0].id;
        let c_id = items[2].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));

        // Move A after C: [B, C, A]
        player.process_command(PlayerCommand::MoveInPlaylist {
            id: a_id,
            target: c_id,
            after: true,
        });
        assert_eq!(playlist_titles(&player), vec!["B", "C", "A"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);
    }

    #[test]
    fn redo_move() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let a_id = items[0].id;
        let c_id = items[2].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::MoveInPlaylist {
            id: a_id,
            target: c_id,
            after: true,
        });
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_titles(&player), vec!["B", "C", "A"]);
    }

    // --- MoveItemsInPlaylist (batch) undo/redo ---

    #[test]
    fn undo_batch_move() {
        let mut player = Player::new();
        let items = vec![
            make_item("A"),
            make_item("B"),
            make_item("C"),
            make_item("D"),
        ];
        let a_id = items[0].id;
        let b_id = items[1].id;
        let d_id = items[3].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));

        // Move A,B after D: [C, D, A, B]
        player.process_command(PlayerCommand::MoveItemsInPlaylist {
            ids: vec![a_id, b_id],
            target: d_id,
            after: true,
        });
        assert_eq!(playlist_titles(&player), vec!["C", "D", "A", "B"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C", "D"]);
    }

    // --- ClearPlaylist undo/redo ---

    #[test]
    fn undo_clear_restores_playlist() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::ClearPlaylist);
        assert!(playlist_ids(&player).is_empty());

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);
    }

    /// The bug: replacing the queue starts the new track, and undoing restored
    /// the old queue while leaving the engine on a track that queue no longer
    /// contains — a transport describing a row nobody can see, and a decode
    /// lookahead with nothing to follow.
    #[test]
    fn undoing_a_replace_does_not_leave_the_engine_on_an_orphaned_track() {
        let mut player = Player::new();
        let original = seed(&mut player, 3);
        player.shared_state.set_cursor(Some(original[0]));
        pretend_playing(&mut player, original[0]);

        let replacement = vec![make_item("something else")];
        let orphan = replacement[0].id;
        player.process_command(PlayerCommand::ReplacePlaylist {
            items: replacement,
            start: 0,
            position_ms: 0,
            play: true,
        });
        // What `play()` would have left behind if the file existed.
        pretend_playing(&mut player, orphan);

        player.process_command(PlayerCommand::Undo);

        assert_eq!(playlist_ids(&player), original, "the queue comes back");
        assert!(
            player.shared_state.get_item(orphan).is_none(),
            "and the replacement is gone from it"
        );
        assert!(
            playing_id(&player).is_none_or(|id| player.shared_state.get_item(id).is_some()),
            "so nothing may still be playing out of it"
        );
    }

    /// The same orphaning, reached by undoing an add rather than a replace.
    #[test]
    fn undoing_an_add_does_not_leave_the_engine_on_a_removed_track() {
        let mut player = Player::new();
        seed(&mut player, 2);
        let added = seed(&mut player, 1);
        pretend_playing(&mut player, added[0]);

        player.process_command(PlayerCommand::Undo);

        assert!(player.shared_state.get_item(added[0]).is_none());
        assert!(
            playing_id(&player).is_none_or(|id| player.shared_state.get_item(id).is_some()),
            "the engine cannot be left on the item the undo removed"
        );
    }

    /// An undo that leaves the playing item where it is must not restart it.
    #[test]
    fn undoing_a_move_leaves_playback_alone() {
        let mut player = Player::new();
        let ids = seed(&mut player, 3);
        player.shared_state.set_cursor(Some(ids[0]));
        pretend_playing(&mut player, ids[0]);
        let starts = player.playback_starts;

        player.process_command(PlayerCommand::MoveInPlaylist {
            id: ids[2],
            target: ids[0],
            after: false,
        });
        player.process_command(PlayerCommand::Undo);

        assert_eq!(playlist_ids(&player), ids);
        assert_eq!(playing_id(&player), Some(ids[0]), "still on the same track");
        assert_eq!(player.playback_starts, starts, "and not restarted");
    }

    #[test]
    fn redo_clear() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B")];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::ClearPlaylist);
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B"]);

        player.process_command(PlayerCommand::Redo);
        assert!(playlist_ids(&player).is_empty());
    }

    // --- Multi-step undo/redo ---

    #[test]
    fn multiple_undos_in_sequence() {
        let mut player = Player::new();

        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("A")]));
        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("B")]));
        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("C")]));
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B"]);

        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A"]);

        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());
    }

    #[test]
    fn undo_redo_undo_cycle() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B")];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());

        player.process_command(PlayerCommand::Redo);
        assert_eq!(playlist_titles(&player), vec!["A", "B"]);

        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());
    }

    #[test]
    fn new_action_clears_redo_stack() {
        let mut player = Player::new();
        let items = vec![make_item("A")];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::Undo);
        assert!(player.undo_stack().can_redo());

        // New action should clear redo
        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("B")]));
        assert!(!player.undo_stack().can_redo());
    }

    #[test]
    fn undo_on_empty_stack_is_noop() {
        let mut player = Player::new();
        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());
    }

    #[test]
    fn redo_on_empty_stack_is_noop() {
        let mut player = Player::new();
        player.process_command(PlayerCommand::Redo);
        assert!(playlist_ids(&player).is_empty());
    }

    // --- Non-undoable commands don't push entries ---

    #[test]
    fn playback_commands_not_undoable() {
        let mut player = Player::new();
        player.process_command(PlayerCommand::Pause);
        player.process_command(PlayerCommand::Resume);
        player.process_command(PlayerCommand::NextTrack);
        player.process_command(PlayerCommand::PrevTrack);
        assert!(!player.undo_stack().can_undo());
    }

    #[test]
    fn update_paths_not_undoable() {
        let mut player = Player::new();
        let items = vec![make_item("A")];
        let id = items[0].id;
        player.process_command(PlayerCommand::AddToPlaylist(items));

        let undo_count = player.undo_stack().undo_len();
        player.process_command(PlayerCommand::UpdatePaths(vec![(
            id,
            PathBuf::from("/new/path.flac"),
        )]));
        assert_eq!(player.undo_stack().undo_len(), undo_count);
    }

    // --- Complex scenarios ---

    #[test]
    fn add_remove_undo_undo_produces_original() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let b_id = items[1].id;
        let original_titles = vec!["A", "B", "C"];

        player.process_command(PlayerCommand::AddToPlaylist(items));
        player.process_command(PlayerCommand::RemoveFromPlaylist(b_id));
        assert_eq!(playlist_titles(&player), vec!["A", "C"]);

        // Undo remove → back to A, B, C
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), original_titles);

        // Undo add → empty
        player.process_command(PlayerCommand::Undo);
        assert!(playlist_ids(&player).is_empty());
    }

    #[test]
    fn interleaved_adds_and_moves_undo() {
        let mut player = Player::new();
        let items = vec![make_item("A"), make_item("B"), make_item("C")];
        let a_id = items[0].id;
        let c_id = items[2].id;

        player.process_command(PlayerCommand::AddToPlaylist(items));

        // Move A after C: [B, C, A]
        player.process_command(PlayerCommand::MoveInPlaylist {
            id: a_id,
            target: c_id,
            after: true,
        });
        assert_eq!(playlist_titles(&player), vec!["B", "C", "A"]);

        // Add D: [B, C, A, D]
        player.process_command(PlayerCommand::AddToPlaylist(vec![make_item("D")]));
        assert_eq!(playlist_titles(&player), vec!["B", "C", "A", "D"]);

        // Undo add D: [B, C, A]
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["B", "C", "A"]);

        // Undo move: [A, B, C]
        player.process_command(PlayerCommand::Undo);
        assert_eq!(playlist_titles(&player), vec!["A", "B", "C"]);
    }

    /// Regression test for GitHub #89: AudioEngine must be dropped synchronously
    /// in stop_engine() before the caller changes sample rates. If the engine is
    /// dropped on a background thread, CoreAudio's internal buffer list can be
    /// freed while AudioUnitUninitialize is still tearing it down → crash.
    #[test]
    fn stop_engine_drops_engine_synchronously() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct MockEngine {
            dropped: Arc<AtomicBool>,
        }
        impl AudioEngineHandle for MockEngine {
            fn start(&self) -> Result<(), BackendError> {
                Ok(())
            }
            fn stop(&self) -> Result<(), BackendError> {
                Ok(())
            }
            fn is_running(&self) -> bool {
                false
            }
            fn fade_out(&self) {}
            fn fade_in(&self) -> Result<(), BackendError> {
                Ok(())
            }
            fn is_silent(&self) -> bool {
                false
            }
        }
        impl Drop for MockEngine {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));

        let mut player = Player::new();
        player.transport = Transport::Loaded(test_session(
            QueueItemId::new(),
            Box::new(MockEngine {
                dropped: dropped.clone(),
            }),
        ));

        player.stop_engine();

        // The engine must already be dropped when stop_engine returns.
        // If this fails, the engine was moved to a background thread — the
        // exact race condition that causes the #89 crash.
        assert!(
            dropped.load(Ordering::SeqCst),
            "AudioEngine must be dropped synchronously in stop_engine (GitHub #89)"
        );
    }

    #[test]
    fn abandoning_a_stream_wakes_a_reader_parked_at_the_write_head() {
        let live = LiveStream {
            feed: crate::remote::downloads::ByteFeed::new(),
            abandoned: Default::default(),
        };
        let feed = live.feed.clone();
        let started = std::time::Instant::now();
        let reader = thread::spawn(move || {
            feed.wait_past(
                0,
                std::time::Instant::now() + std::time::Duration::from_secs(30),
            )
        });
        thread::sleep(std::time::Duration::from_millis(50));
        live.abandon();
        reader.join().unwrap();

        assert!(live.abandoned.load(std::sync::atomic::Ordering::Acquire));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    // --- Engine format matches the decoded PCM ---

    /// Backend pinned to one sample rate that refuses every switch, recording
    /// the format the engine is asked for.
    pub(super) struct StuckBackend {
        pub(super) rate: f64,
        pub(super) asked: Arc<std::sync::Mutex<Option<(f64, u32)>>>,
        /// How many times an engine it made was started.
        pub(super) starts: Arc<std::sync::atomic::AtomicUsize>,
    }

    struct NullEngine {
        starts: Arc<std::sync::atomic::AtomicUsize>,
        running: std::sync::atomic::AtomicBool,
        lead_in: Arc<AtomicU64>,
    }
    impl AudioEngineHandle for NullEngine {
        fn start(&self) -> Result<(), BackendError> {
            self.starts.fetch_add(1, Ordering::Relaxed);
            self.running.store(true, Ordering::Relaxed);
            Ok(())
        }
        fn stop(&self) -> Result<(), BackendError> {
            self.running.store(false, Ordering::Relaxed);
            Ok(())
        }
        fn is_running(&self) -> bool {
            self.running.load(Ordering::Relaxed)
        }
        fn fade_out(&self) {}
        fn fade_in(&self) -> Result<(), BackendError> {
            Ok(())
        }
        fn is_silent(&self) -> bool {
            false
        }
        fn lead_in(&self, frames: u64) {
            self.lead_in.store(frames, Ordering::Relaxed);
        }
    }

    impl AudioBackend for StuckBackend {
        fn list_devices(&self) -> Result<Vec<backend::DeviceInfo>, BackendError> {
            Ok(vec![self.default_device()?])
        }
        fn default_device(&self) -> Result<backend::DeviceInfo, BackendError> {
            Ok(backend::DeviceInfo {
                name: "Stuck DAC".into(),
                sample_rates: vec![self.rate],
                platform_id: 0,
                kind: Default::default(),
            })
        }
        fn supported_sample_rates(
            &self,
            _device: &backend::DeviceInfo,
        ) -> Result<Vec<f64>, BackendError> {
            Ok(vec![self.rate])
        }
        fn get_device_sample_rate(
            &self,
            _device: &backend::DeviceInfo,
        ) -> Result<f64, BackendError> {
            Ok(self.rate)
        }
        fn set_device_sample_rate(
            &self,
            _device: &backend::DeviceInfo,
            rate: f64,
        ) -> Result<f64, BackendError> {
            Err(BackendError::UnsupportedSampleRate(rate))
        }
        fn create_engine(
            &self,
            _device: &backend::DeviceInfo,
            sample_rate: f64,
            channels: u32,
            _consumer: rtrb::Consumer<f32>,
            _samples_played: Arc<AtomicU64>,
        ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
            *self.asked.lock().unwrap() = Some((sample_rate, channels));
            Ok(Box::new(NullEngine {
                starts: self.starts.clone(),
                running: Default::default(),
                lead_in: Default::default(),
            }))
        }
    }

    /// Hands the test every ring buffer an engine is made with, so what
    /// reaches the device can be read back.
    struct CaptureBackend {
        consumers: Arc<std::sync::Mutex<Vec<rtrb::Consumer<f32>>>>,
    }

    impl AudioBackend for CaptureBackend {
        fn list_devices(&self) -> Result<Vec<backend::DeviceInfo>, BackendError> {
            Ok(vec![self.default_device()?])
        }
        fn default_device(&self) -> Result<backend::DeviceInfo, BackendError> {
            Ok(backend::DeviceInfo {
                name: "Capture DAC".into(),
                sample_rates: vec![44100.0],
                platform_id: 0,
                kind: Default::default(),
            })
        }
        fn supported_sample_rates(
            &self,
            _device: &backend::DeviceInfo,
        ) -> Result<Vec<f64>, BackendError> {
            Ok(vec![44100.0])
        }
        fn get_device_sample_rate(
            &self,
            _device: &backend::DeviceInfo,
        ) -> Result<f64, BackendError> {
            Ok(44100.0)
        }
        fn set_device_sample_rate(
            &self,
            _device: &backend::DeviceInfo,
            rate: f64,
        ) -> Result<f64, BackendError> {
            Ok(rate)
        }
        fn create_engine(
            &self,
            _device: &backend::DeviceInfo,
            _sample_rate: f64,
            _channels: u32,
            consumer: rtrb::Consumer<f32>,
            _samples_played: Arc<AtomicU64>,
        ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
            self.consumers.lock().unwrap().push(consumer);
            Ok(Box::new(NullEngine {
                starts: Default::default(),
                running: Default::default(),
                lead_in: Default::default(),
            }))
        }
    }

    /// The loudest sample of a session's first `want` samples.
    fn peak_reaching_the_device(
        consumers: &std::sync::Mutex<Vec<rtrb::Consumer<f32>>>,
        want: usize,
    ) -> f32 {
        let mut consumer = consumers.lock().unwrap().pop().expect("an engine was made");
        let mut peak = 0.0f32;
        let mut got = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while got < want && std::time::Instant::now() < deadline {
            let n = consumer.slots();
            if n == 0 {
                std::thread::sleep(std::time::Duration::from_millis(2));
                continue;
            }
            let chunk = consumer.read_chunk(n).unwrap();
            let (a, b) = chunk.as_slices();
            peak = a.iter().chain(b).fold(peak, |p, s| p.max(s.abs()));
            got += n;
            chunk.commit_all();
        }
        assert!(got >= want, "only {got} of {want} samples arrived");
        peak
    }

    /// A file on disk and a download still landing open through the same
    /// session, so both are processed. The streaming path is the one that
    /// used to be its own function, and the easy one to leave behind.
    #[test]
    fn a_profile_processes_files_and_streams_alike() {
        let dir = tempfile::tempdir().unwrap();
        let tone = dir.path().join("tone.wav");
        crate::test_utils::generate_wav_tone(&tone, 44100, 440.0, 0.5);
        let want = 22050;

        let consumers = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut player = Player::new();
        player.backend = Box::new(CaptureBackend {
            consumers: consumers.clone(),
        });
        let mut item = make_item("tone");
        item.path = tone.clone();
        let id = item.id;
        player.shared_state.add_items(vec![item]);
        player.shared_state.set_cursor(Some(id));

        let stream = || {
            let feed = crate::remote::downloads::ByteFeed::new();
            feed.set(std::fs::metadata(&tone).unwrap().len());
            Source::Stream(StreamSource {
                path: tone.clone(),
                bytes_written: feed,
                total: 0,
                mode: streaming::ProbeMode::Full,
            })
        };
        let peaks = |player: &mut Player| {
            let mut out = Vec::new();
            for source in [Source::File(tone.clone()), stream()] {
                player
                    .try_open_session(id, source, None, 0, Run::Playing)
                    .unwrap();
                player.publish();
                out.push((
                    peak_reaching_the_device(&consumers, want),
                    player.shared_state.dsp().map(|d| d.profile),
                ));
                player.stop_engine();
            }
            out
        };

        let untouched = peaks(&mut player);

        player.dsp_override = Some(Arc::new(
            crate::audio::dsp::Setup::new(vec![], vec![]).with_preamp(-6.0206),
        ));
        let processed = peaks(&mut player);

        for (kind, ((before, none), (after, half))) in ["file", "stream"]
            .iter()
            .zip(untouched.iter().zip(&processed))
        {
            assert_eq!(
                none, &None,
                "{kind}: nothing is published without a profile"
            );
            assert_eq!(half.as_deref(), Some("test"), "{kind}: the badge names it");
            assert!(*before > 0.1, "{kind}: the tone reached the device");
            assert!(
                (after / before - 0.5).abs() < 0.01,
                "{kind}: {before} → {after}, not halved"
            );
        }
    }

    /// Replacing the queue while the track playing is paused, with a track
    /// that has still to arrive: nothing sounds until the new one can open,
    /// the old one is never reopened, and the new one is what is published
    /// from the start. The iOS report this answers: a playlist's play button,
    /// pressed over a paused track, was heard as that track carrying on.
    #[test]
    fn a_replaced_queue_never_sounds_the_track_it_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        crate::test_utils::generate_wav_tone(&a, 44100, 440.0, 5.0);
        let consumers = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut player = Player::new();
        player.backend = Box::new(CaptureBackend {
            consumers: consumers.clone(),
        });
        let engines = || consumers.lock().unwrap().len();

        let item_a = PlaylistItem {
            path: a,
            ..make_item("paused")
        };
        let a_id = item_a.id;
        player.process_command(PlayerCommand::AddToPlaylist(vec![item_a]));
        player.process_command(PlayerCommand::Play(a_id));
        player.process_command(PlayerCommand::Pause);
        assert_eq!(engines(), 1);

        let landed = dir.path().join("long.opus");
        let item_b = PlaylistItem {
            db_id: Some(77),
            state: ItemState::Pending,
            duration_ms: Some(32_523_781),
            path: landed.clone(),
            ..make_item("long")
        };
        let b_id = item_b.id;
        player.process_command(PlayerCommand::ReplacePlaylist {
            items: vec![item_b],
            start: 0,
            position_ms: 0,
            play: true,
        });
        player.publish();
        let state = player.shared_state.clone();
        assert_eq!(state.cursor(), Some(b_id), "the new track, at once");
        assert!(
            state.is_waiting(),
            "waiting for it, which a client shows as loading"
        );
        assert_eq!(state.playback_state(), PlaybackState::Stopped);
        assert_eq!(
            state.track_info(),
            None,
            "the paused track is no longer published"
        );
        assert_eq!(engines(), 1, "nothing opened while it is on its way");

        // Its download lands.
        let store = state.downloads().clone();
        store.claim(77, Some(b_id));
        std::fs::write(&landed, include_bytes!("testdata/thirty-seconds.opus")).unwrap();
        // Told to the player below, as the download queue would.
        let _settled = crate::remote::downloads::settle(&state, 77, &Ok(landed));
        player.process_command(PlayerCommand::TrackReady(b_id));
        player.publish();

        assert_eq!(
            engines(),
            2,
            "one engine for the new track, none for the old"
        );
        assert_eq!(state.track_info().map(|t| t.id), Some(b_id));
        assert_eq!(state.playback_state(), PlaybackState::Playing);
        player.process_command(PlayerCommand::Stop);
    }

    /// A long Ogg is opened before its last page has arrived, which is where
    /// Ogg keeps its length, so the stream itself cannot say how long it is.
    /// The library can: a nine-hour remote track showed no duration until
    /// the whole file had downloaded.
    #[test]
    fn a_stream_opened_without_a_length_takes_the_librarys_duration() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = include_bytes!("testdata/thirty-seconds.opus");
        let half = bytes.len() / 2;
        let part = dir.path().join("long.opus.part");
        std::fs::write(&part, &bytes[..half]).unwrap();
        let feed = crate::remote::downloads::ByteFeed::new();
        feed.set(half as u64);

        let mut player = Player::new();
        player.backend = Box::new(CaptureBackend {
            consumers: Default::default(),
        });
        let item = PlaylistItem {
            db_id: Some(9),
            state: ItemState::Pending,
            duration_ms: Some(32_523_781),
            path: dir.path().join("long.opus"),
            ..make_item("long")
        };
        let id = item.id;
        player.shared_state.add_items(vec![item]);
        player.shared_state.set_cursor(Some(id));

        let lengthless = buffer::StreamInfo {
            codec: "Opus".into(),
            sample_rate: 48_000,
            channels: 1,
            bit_depth: None,
            bitrate_kbps: None,
            duration_ms: 0,
        };
        player
            .try_open_session(
                id,
                Source::Stream(StreamSource {
                    path: part,
                    bytes_written: feed,
                    total: bytes.len() as u64,
                    mode: streaming::ProbeMode::Lengthless,
                }),
                Some(lengthless),
                0,
                Run::Playing,
            )
            .unwrap();
        player.publish();

        assert_eq!(player.shared_state.duration_ms(), 32_523_781);
        assert_eq!(
            player.shared_state.track_info().map(|t| t.duration_ms),
            Some(32_523_781)
        );
        player.stop_engine();
    }

    fn engine_format_for(source_rate: u32, channels: u16, device_rate: f64) -> (f64, u32) {
        let asked = Arc::new(std::sync::Mutex::new(None));
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: device_rate,
            asked: asked.clone(),
            starts: Default::default(),
        });

        let info = buffer::StreamInfo {
            codec: "MP3".into(),
            sample_rate: source_rate,
            channels,
            bit_depth: Some(16),
            bitrate_kbps: None,
            duration_ms: 1000,
        };
        let (_producer, consumer) = rtrb::RingBuffer::new(16);
        player
            .create_engine_for(&info, consumer)
            .expect("engine creation should succeed");
        let asked = *asked.lock().unwrap();
        asked.expect("engine was never created")
    }

    #[test]
    fn engine_uses_source_rate_when_device_refuses_switch() {
        // MPEG-2 MP3 rates are routinely rejected by output devices. The engine
        // must still be told the rate the PCM actually is.
        assert_eq!(engine_format_for(22050, 2, 48000.0), (22050.0, 2));
        assert_eq!(engine_format_for(32000, 2, 44100.0), (32000.0, 2));
    }

    #[test]
    fn engine_uses_source_channel_count() {
        assert_eq!(engine_format_for(44100, 1, 44100.0), (44100.0, 1));
    }

    /// The rate the device settled at, as the front ends read it.
    fn settled_rate_for(source_rate: u32, device_rate: f64) -> Option<u32> {
        let mut player = Player::new();
        player.backend = Box::new(StuckBackend {
            rate: device_rate,
            asked: Arc::new(std::sync::Mutex::new(None)),
            starts: Default::default(),
        });
        let state = player.shared_state.clone();

        let info = buffer::StreamInfo {
            codec: "MP3".into(),
            sample_rate: source_rate,
            channels: 2,
            bit_depth: Some(16),
            bitrate_kbps: None,
            duration_ms: 1000,
        };
        let (_producer, consumer) = rtrb::RingBuffer::new(16);
        player
            .create_engine_for(&info, consumer)
            .expect("engine creation should succeed");
        state.output_sample_rate()
    }

    #[test]
    fn settled_device_rate_reaches_the_shared_state() {
        // A device that refuses the switch is being fed resampled audio, and
        // that is the case the front ends have to be able to see. Before this
        // the comparison happened once, in a log line.
        assert_eq!(settled_rate_for(22050, 48000.0), Some(48000));
        // No switch needed, so nothing resampled: the two rates agree.
        assert_eq!(settled_rate_for(44100, 44100.0), Some(44100));
    }

    /// A device that takes its time reclocking, as real hardware does.
    struct SlowBackend {
        observed: Arc<std::sync::Mutex<Vec<Option<u32>>>>,
        state: Arc<SharedPlayerState>,
        lead_in: Arc<AtomicU64>,
    }

    impl AudioBackend for SlowBackend {
        fn list_devices(&self) -> Result<Vec<backend::DeviceInfo>, BackendError> {
            Ok(vec![self.default_device()?])
        }
        fn default_device(&self) -> Result<backend::DeviceInfo, BackendError> {
            Ok(backend::DeviceInfo {
                name: "Slow DAC".into(),
                sample_rates: vec![44100.0, 48000.0],
                platform_id: 0,
                kind: Default::default(),
            })
        }
        fn supported_sample_rates(
            &self,
            _device: &backend::DeviceInfo,
        ) -> Result<Vec<f64>, BackendError> {
            Ok(vec![44100.0, 48000.0])
        }
        fn get_device_sample_rate(
            &self,
            _device: &backend::DeviceInfo,
        ) -> Result<f64, BackendError> {
            Ok(48000.0)
        }
        fn set_device_sample_rate(
            &self,
            _device: &backend::DeviceInfo,
            rate: f64,
        ) -> Result<f64, BackendError> {
            // What a front end polling mid-switch would see.
            self.observed
                .lock()
                .unwrap()
                .push(self.state.output_sample_rate());
            Ok(rate)
        }
        fn create_engine(
            &self,
            _device: &backend::DeviceInfo,
            _sample_rate: f64,
            _channels: u32,
            _consumer: rtrb::Consumer<f32>,
            _samples_played: Arc<AtomicU64>,
        ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
            Ok(Box::new(NullEngine {
                starts: Default::default(),
                running: Default::default(),
                lead_in: self.lead_in.clone(),
            }))
        }
    }

    #[test]
    fn the_previous_rate_is_not_published_while_the_device_reclocks() {
        // A 48 kHz track followed by a 44.1 kHz one: for as long as the switch
        // takes — the better part of a second on USB — the new track's info is
        // published against the old track's output rate. A front end polling in
        // that window used to latch "44.1 → 48" and, since nothing about the
        // codec or the source rate changed afterwards, never let go of it.
        let mut player = Player::new();
        let state = player.shared_state.clone();
        state.set_output_sample_rate(48000);

        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        player.backend = Box::new(SlowBackend {
            observed: observed.clone(),
            state: state.clone(),
            lead_in: Default::default(),
        });

        let info = buffer::StreamInfo {
            codec: "FLAC".into(),
            sample_rate: 44100,
            channels: 2,
            bit_depth: Some(16),
            bitrate_kbps: None,
            duration_ms: 1000,
        };
        let (_producer, consumer) = rtrb::RingBuffer::new(16);
        player
            .create_engine_for(&info, consumer)
            .expect("engine creation should succeed");

        assert_eq!(
            *observed.lock().unwrap(),
            vec![None],
            "mid-switch the output rate must read as unknown, not as the last track's"
        );
        assert_eq!(state.output_sample_rate(), Some(44100));
    }

    /// The lead-in an engine was given, for a track at `source_rate` on a
    /// device sitting at 48 kHz.
    fn lead_in_for(source_rate: u32) -> u64 {
        let mut player = Player::new();
        let lead_in = Arc::new(AtomicU64::new(0));
        player.backend = Box::new(SlowBackend {
            observed: Default::default(),
            state: player.shared_state.clone(),
            lead_in: lead_in.clone(),
        });
        let info = buffer::StreamInfo {
            codec: "FLAC".into(),
            sample_rate: source_rate,
            channels: 2,
            bit_depth: Some(16),
            bitrate_kbps: None,
            duration_ms: 1000,
        };
        let (_producer, consumer) = rtrb::RingBuffer::new(16);
        player
            .create_engine_for(&info, consumer)
            .expect("engine creation should succeed");
        lead_in.load(Ordering::Relaxed)
    }

    #[test]
    fn an_engine_made_inside_the_silence_keeps_the_rest_of_it() {
        // A seek straight after a rate switch: the device is still relocking,
        // though the new engine finds its rate already matching.
        let mut player = Player::new();
        let lead_in = Arc::new(AtomicU64::new(0));
        player.backend = Box::new(SlowBackend {
            observed: Default::default(),
            state: player.shared_state.clone(),
            lead_in: lead_in.clone(),
        });
        let info = |sample_rate| buffer::StreamInfo {
            codec: "FLAC".into(),
            sample_rate,
            channels: 2,
            bit_depth: Some(16),
            bitrate_kbps: None,
            duration_ms: 1000,
        };
        let (_p, consumer) = rtrb::RingBuffer::new(16);
        player.create_engine_for(&info(44100), consumer).unwrap();
        let first = lead_in.swap(0, Ordering::Relaxed);
        assert!(first > 0);

        let (_p, consumer) = rtrb::RingBuffer::new(16);
        player.create_engine_for(&info(48000), consumer).unwrap();
        let carried = lead_in.load(Ordering::Relaxed);
        // Frames at the new engine's rate: what is left of the same second.
        assert!(
            carried > 0 && carried < first * 48000 / 44100,
            "carried {carried} of {first}"
        );
    }

    #[test]
    fn silence_leads_in_only_after_the_device_changed_rate() {
        assert!(
            lead_in_for(44100) > 0,
            "the device is relocking, so the start of the track would be lost"
        );
        assert_eq!(lead_in_for(48000), 0, "no switch, nothing to wait for");
    }

    /// iOS's rates on an engine that plays nothing.
    #[cfg(target_os = "macos")]
    struct IosRates {
        lead_in: Arc<AtomicU64>,
    }

    #[cfg(target_os = "macos")]
    impl AudioBackend for IosRates {
        fn list_devices(&self) -> Result<Vec<backend::DeviceInfo>, BackendError> {
            crate::audio::ios_backend::IosAudioBackend.list_devices()
        }
        fn default_device(&self) -> Result<backend::DeviceInfo, BackendError> {
            crate::audio::ios_backend::IosAudioBackend.default_device()
        }
        fn supported_sample_rates(
            &self,
            device: &backend::DeviceInfo,
        ) -> Result<Vec<f64>, BackendError> {
            crate::audio::ios_backend::IosAudioBackend.supported_sample_rates(device)
        }
        fn get_device_sample_rate(
            &self,
            device: &backend::DeviceInfo,
        ) -> Result<f64, BackendError> {
            crate::audio::ios_backend::IosAudioBackend.get_device_sample_rate(device)
        }
        fn set_device_sample_rate(
            &self,
            device: &backend::DeviceInfo,
            rate: f64,
        ) -> Result<f64, BackendError> {
            crate::audio::ios_backend::IosAudioBackend.set_device_sample_rate(device, rate)
        }
        fn create_engine(
            &self,
            _device: &backend::DeviceInfo,
            _sample_rate: f64,
            _channels: u32,
            _consumer: rtrb::Consumer<f32>,
            _samples_played: Arc<AtomicU64>,
        ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
            Ok(Box::new(NullEngine {
                starts: Default::default(),
                running: Default::default(),
                lead_in: self.lead_in.clone(),
            }))
        }
    }

    /// A DAC that runs at whichever of 44.1 and 48 kHz it is asked for.
    #[cfg(target_os = "macos")]
    struct SwitchingDac;

    #[cfg(target_os = "macos")]
    impl crate::audio::ios_backend::AudioSession for SwitchingDac {
        fn activate(&self, sample_rate: f64) -> Option<f64> {
            Some(self.follow(sample_rate))
        }
        fn follow(&self, sample_rate: f64) -> f64 {
            if sample_rate == 44100.0 {
                44100.0
            } else {
                48000.0
            }
        }
        fn release(&self) {}
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_session_let_go_is_no_rate_switch_to_wait_out() {
        use crate::audio::ios_backend;
        let _session = ios_backend::TEST_SESSION.lock();
        ios_backend::set_session(Arc::new(SwitchingDac));
        let lead_in_at = |sample_rate| {
            let mut player = Player::new();
            let lead_in = Arc::new(AtomicU64::new(0));
            player.backend = Box::new(IosRates {
                lead_in: lead_in.clone(),
            });
            let info = buffer::StreamInfo {
                codec: "FLAC".into(),
                sample_rate,
                channels: 2,
                bit_depth: Some(16),
                bitrate_kbps: None,
                duration_ms: 1000,
            };
            let (_producer, consumer) = rtrb::RingBuffer::new(16);
            player.create_engine_for(&info, consumer).unwrap();
            lead_in.load(Ordering::Relaxed)
        };

        // Held at 48 kHz, a 44.1 kHz track switches the DAC.
        assert!(ios_backend::activate_session(48000.0));
        assert!(lead_in_at(44100) > 0);

        // Played at 48 kHz, then idle long enough to let the session go: the
        // rate it last ran at is no longer the hardware's, and the next
        // track, at 44.1 kHz, starts without a second of silence.
        assert!(ios_backend::activate_session(48000.0));
        ios_backend::release_session();
        assert_eq!(lead_in_at(44100), 0);
    }

    /// Backend that hands its rate-change callback back to the test.
    struct WatchedBackend {
        inner: StuckBackend,
        #[allow(clippy::type_complexity)]
        captured: Arc<std::sync::Mutex<Option<Box<dyn Fn(f64) + Send + Sync>>>>,
    }

    struct NullWatch;
    impl backend::SampleRateWatch for NullWatch {}

    impl AudioBackend for WatchedBackend {
        fn list_devices(&self) -> Result<Vec<backend::DeviceInfo>, BackendError> {
            self.inner.list_devices()
        }
        fn default_device(&self) -> Result<backend::DeviceInfo, BackendError> {
            self.inner.default_device()
        }
        fn supported_sample_rates(
            &self,
            device: &backend::DeviceInfo,
        ) -> Result<Vec<f64>, BackendError> {
            self.inner.supported_sample_rates(device)
        }
        fn get_device_sample_rate(
            &self,
            device: &backend::DeviceInfo,
        ) -> Result<f64, BackendError> {
            self.inner.get_device_sample_rate(device)
        }
        fn set_device_sample_rate(
            &self,
            device: &backend::DeviceInfo,
            rate: f64,
        ) -> Result<f64, BackendError> {
            self.inner.set_device_sample_rate(device, rate)
        }
        fn watch_device_sample_rate(
            &self,
            _device: &backend::DeviceInfo,
            on_change: Box<dyn Fn(f64) + Send + Sync>,
        ) -> Option<Box<dyn backend::SampleRateWatch>> {
            *self.captured.lock().unwrap() = Some(on_change);
            Some(Box::new(NullWatch))
        }
        fn create_engine(
            &self,
            device: &backend::DeviceInfo,
            sample_rate: f64,
            channels: u32,
            consumer: rtrb::Consumer<f32>,
            samples_played: Arc<AtomicU64>,
        ) -> Result<Box<dyn AudioEngineHandle>, BackendError> {
            self.inner
                .create_engine(device, sample_rate, channels, consumer, samples_played)
        }
    }

    #[test]
    fn external_rate_change_reaches_the_shared_state() {
        // The device is shared. Another client moving the rate mid-track used
        // to leave the front ends asserting bit-perfection while the HAL
        // resampled underneath them.
        let captured = Arc::new(std::sync::Mutex::new(None));
        let mut player = Player::new();
        player.backend = Box::new(WatchedBackend {
            inner: StuckBackend {
                rate: 44100.0,
                asked: Arc::new(std::sync::Mutex::new(None)),
                starts: Default::default(),
            },
            captured: captured.clone(),
        });
        let state = player.shared_state.clone();

        let info = buffer::StreamInfo {
            codec: "FLAC".into(),
            sample_rate: 44100,
            channels: 2,
            bit_depth: Some(16),
            bitrate_kbps: None,
            duration_ms: 1000,
        };
        let (_producer, consumer) = rtrb::RingBuffer::new(16);
        player
            .create_engine_for(&info, consumer)
            .expect("engine creation should succeed");
        assert_eq!(state.output_sample_rate(), Some(44100));

        let on_change = captured.lock().unwrap().take().expect("watch registered");
        on_change(48000.0);
        assert_eq!(state.output_sample_rate(), Some(48000));
    }
}
